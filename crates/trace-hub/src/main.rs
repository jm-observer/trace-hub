//! trace-hub 后端入口：ingest + SQLite 存储 + query API + Web UI。
//!
//! 经 custom-utils `updater` 接入 Linux 部署栈：
//! - `trace-hub`（无子命令）：以服务模式运行，workspace 默认 `~/.config/trace-hub`。
//! - `trace-hub install [--dry-run|-n] [-w <path>] [--port <p>]`：rootless 安装 user
//!   systemd 服务；`--port` 写进生成 unit 的 `Environment=TRACE_HUB_PORT=<p>`，服务据此
//!   覆盖 config 端口（见 [`config::Config`]）。缺省时回落到 [`DEFAULT_PORT`]。
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

/// install 时未显式 `--port` 时写进 unit 的默认端口，与 config 默认 `0.0.0.0:9100` 对齐。
const DEFAULT_PORT: &str = "9100";

/// trace-hub 的 Linux 部署描述：自更新源（GitHub `jm-observer/trace-hub`）、
/// rootless user-systemd 安装、`~/.config/trace-hub` workspace 与看门狗，单一事实源。
///
/// `port` 写进生成 unit `[Service]` 段的 `Environment=TRACE_HUB_PORT=<port>`，服务启动
/// 时据此覆盖 config 里 bind 的端口（解析逻辑在 [`config::Config::load`]）。
fn service(port: &str) -> LinuxService {
    LinuxService::new(
        "trace-hub",               // app：unit 名 + ~/.config/trace-hub
        "jm-observer",             // GitHub owner
        "trace-hub",               // GitHub repo
        env!("CARGO_PKG_VERSION"), // 当前版本
    )
    .description("Trace Hub —— 全生命周期追踪后端")
    .env("TRACE_HUB_PORT", port) // install 写进 unit；serve 时覆盖 config 端口
    .watchdog_sec(30) // Type=notify + WatchdogSec=30；spawn_watchdog 发 READY=1 并心跳
}

/// 解析 install 的 `--port <p>`/`-p <p>`：缺省回落 [`DEFAULT_PORT`]；非 1..=65535 端口
/// 直接报错（宁可 install 失败也不写一个坏 unit）。custom-utils 的 `util_args` 非 pub，
/// 故这里直接扫 `std::env::args()`（与其 `arg_value` 同语义：取标志后的下一个实参）。
/// 仅 install 路径需要写进 unit env，Run/update 用默认即可。
fn resolve_port() -> anyhow::Result<String> {
    let mut take_next = false;
    for arg in std::env::args() {
        if take_next {
            let trimmed = arg.trim();
            trimmed
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("非法 --port（{arg}），需 1..=65535"))?;
            return Ok(trimmed.to_string());
        }
        take_next = arg == "--port" || arg == "-p";
    }
    Ok(DEFAULT_PORT.to_string())
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

    // install 时把 `--port`（或默认 9100）写进 unit 的 Environment=TRACE_HUB_PORT；
    // 该 env 同样挂在 Run 路径的 svc 上但无副作用（serve 自身读进程内的 env）。
    let svc = service(&resolve_port()?);
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
