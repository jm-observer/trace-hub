//! 运行配置（配置文件驱动）。
//!
//! 配置来自 `<workspace>/config.toml`（workspace 由 custom-utils 解析，默认
//! `~/.config/trace-hub`）。文件不存在时自动写入一份默认配置，方便首次部署后
//! 直接编辑、重启生效。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// 配置文件名（位于 workspace 内）。
const CONFIG_FILE: &str = "config.toml";

/// 磁盘上的配置（TOML 结构）。字段缺省时回落到 [`Default`]，
/// 因此手写的配置文件可以只覆盖关心的字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct FileConfig {
    /// 监听地址。
    bind: String,
    /// 数据库路径。相对路径相对于 workspace；绝对路径原样使用。
    db: String,
    /// 单个 body 的字节上限（body 是一等公民）。超出则截断 + 标记。
    body_limit: usize,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:9100".to_string(),
            db: "spans.db".to_string(),
            body_limit: 1_000_000,
        }
    }
}

/// 解析后的运行配置。
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub db_path: PathBuf,
    pub body_limit: usize,
}

impl Config {
    /// 从 `<workspace>/config.toml` 加载。文件不存在时先写入一份默认配置再加载。
    pub fn load(workspace: &Path) -> anyhow::Result<Self> {
        let path = workspace.join(CONFIG_FILE);

        let file: FileConfig = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
            toml::from_str(&text).with_context(|| format!("解析配置文件失败: {}", path.display()))?
        } else {
            // 未经 install 直接运行时也能自举：先建 workspace 再写默认配置。
            std::fs::create_dir_all(workspace)
                .with_context(|| format!("创建 workspace 失败: {}", workspace.display()))?;
            let def = FileConfig::default();
            let text = toml::to_string_pretty(&def).context("序列化默认配置失败")?;
            std::fs::write(&path, &text)
                .with_context(|| format!("写入默认配置失败: {}", path.display()))?;
            log::info!("配置文件不存在，已写入默认配置: {}", path.display());
            def
        };

        let bind: SocketAddr = file
            .bind
            .parse()
            .with_context(|| format!("非法 bind 地址: {}", file.bind))?;

        let db_path = {
            let p = PathBuf::from(&file.db);
            if p.is_absolute() {
                p
            } else {
                workspace.join(p)
            }
        };

        Ok(Self {
            bind,
            db_path,
            body_limit: file.body_limit,
        })
    }
}
