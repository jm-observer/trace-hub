//! trace-hub 后端入口：ingest + SQLite 存储 + query API + Web UI。
//!
//! 经 custom-utils `updater` 接入 Linux 部署栈：
//! - `trace-hub`（无子命令）：以服务模式运行，workspace 默认 `~/.config/trace-hub`。
//! - `trace-hub install [--dry-run|-n] [-w <path>]`：rootless 安装 user systemd 服务。
//! - `trace-hub update [--force|-f]`：从 GitHub release 自更新 `~/.local/bin/trace-hub`。
//! - `trace-hub --version|-V` / `--help|-h`。
//!
//! 运行配置见 [`config::Config`]（环境变量，db 默认落在 workspace 内）。

mod config;
mod error;
mod storage;
mod views;
mod web;

use std::path::PathBuf;

use config::Config;
use custom_utils::updater::{CliAction, LinuxService};
use storage::Storage;

/// trace-hub 的 Linux 部署描述：自更新源（GitHub `jm-observer/trace-hub`）、
/// rootless user-systemd 安装、`~/.config/trace-hub` workspace 与看门狗，单一事实源。
fn service() -> LinuxService {
    LinuxService::new(
        "trace-hub",               // app：unit 名 + ~/.config/trace-hub
        "jm-observer",             // GitHub owner
        "trace-hub",               // GitHub repo
        env!("CARGO_PKG_VERSION"), // 当前版本
    )
    .description("Trace Hub —— 全生命周期追踪后端")
    .watchdog_sec(30) // Type=notify + WatchdogSec=30；spawn_watchdog 发 READY=1 并心跳
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // custom-utils 日志服务：默认彩色输出到 stdout（journald 友好）；启用 `prod`
    // feature 时落盘到 ~/log/trace-hub（按天/10MB 轮转、留 10 个）+ ~/etc/trace-hub.toml
    // 动态级别。LoggerHandle 须存活至进程结束。
    let _logger = custom_utils::logger::logger_feature(
        "trace-hub",
        log::LevelFilter::Debug, // dev（非 prod）控制台级别
        log::LevelFilter::Info,  // prod 文件级别
        true,                    // 每次启动重置 ~/etc/trace-hub.toml
    )
    .build(); // 须 .build() 才真正启动；返回的 LoggerHandle 须存活至进程结束

    let svc = service();
    match svc.handle_cli().await? {
        // 无部署子命令：以服务模式运行。workspace 由 custom-utils 解析（honor `-w`）。
        CliAction::Run { workspace } => {
            let _wd = svc.spawn_watchdog(); // systemd READY=1 + 看门狗心跳（非 systemd 下自禁用）
            serve(workspace).await?;
        }
        // 库不做 stdout/exit：install --dry-run / --version / --help 把文本交回这里打印。
        CliAction::DryRun(t) | CliAction::Version(t) | CliAction::Help(t) => println!("{t}"),
        // install / update 已执行完成（经 `log` 输出）。
        CliAction::Handled => {}
    }
    Ok(())
}

/// 服务模式：在解析好的 `workspace` 下加载配置文件并提供 HTTP 服务。
async fn serve(workspace: PathBuf) -> anyhow::Result<()> {
    let cfg = Config::load(&workspace)?;
    log::info!(
        "trace-hub starting: bind={} db={} body_limit={} workspace={}",
        cfg.bind,
        cfg.db_path.display(),
        cfg.body_limit,
        workspace.display()
    );

    let storage = Storage::open(&cfg.db_path, cfg.body_limit)?;
    let app = web::router(storage);

    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    log::info!("listening on http://{}", cfg.bind);
    axum::serve(listener, app).await?;
    Ok(())
}
