//! 运行配置（环境变量驱动，含合理默认）。

use std::net::SocketAddr;
use std::path::PathBuf;

/// 默认监听地址。
const DEFAULT_BIND: &str = "0.0.0.0:9100";
/// 单个 body 的字节上限（body 是一等公民，给较宽松默认）。超出则截断 + 标记。
const DEFAULT_BODY_LIMIT: usize = 1_000_000;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub db_path: PathBuf,
    pub body_limit: usize,
}

impl Config {
    /// 从环境变量加载：
    /// - `TRACE_HUB_BIND`（默认 `0.0.0.0:9100`）
    /// - `TRACE_HUB_DB`（默认 `~/.trace-hub/spans.db`）
    /// - `TRACE_HUB_BODY_LIMIT`（默认 1_000_000 字节）
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = std::env::var("TRACE_HUB_BIND")
            .unwrap_or_else(|_| DEFAULT_BIND.to_string())
            .parse()?;
        let db_path = match std::env::var("TRACE_HUB_DB") {
            Ok(p) => PathBuf::from(p),
            Err(_) => default_db_path()?,
        };
        let body_limit = std::env::var("TRACE_HUB_BODY_LIMIT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_BODY_LIMIT);
        Ok(Self {
            bind,
            db_path,
            body_limit,
        })
    }
}

/// `~/.trace-hub/spans.db`（仿 sps 的 `~/.system-prompt-show/` 约定）。
fn default_db_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| anyhow::anyhow!("无法定位 home 目录（HOME / USERPROFILE 均未设置）"))?;
    Ok(PathBuf::from(home).join(".trace-hub").join("spans.db"))
}
