//! trace-hub 后端入口：ingest + SQLite 存储 + query API。
//!
//! 配置见 [`config::Config`]（环境变量）。后续步骤：Web UI（流程树 + 概要→详情）。

mod config;
mod error;
mod storage;
mod views;
mod web;

use config::Config;
use storage::Storage;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cfg = Config::from_env()?;
    log::info!(
        "trace-hub starting: bind={} db={} body_limit={}",
        cfg.bind,
        cfg.db_path.display(),
        cfg.body_limit
    );

    let storage = Storage::open(&cfg.db_path, cfg.body_limit)?;
    let app = web::router(storage);

    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    log::info!("listening on http://{}", cfg.bind);
    axum::serve(listener, app).await?;
    Ok(())
}
