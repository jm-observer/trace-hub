//! 编译期把当前 git commit 短哈希注入为 `GIT_COMMIT` 环境变量，供
//! `option_env!("GIT_COMMIT")` 读取。取不到（非 git 树 / 无 git）时不写，
//! 由调用方兜底为 "unknown"。任何失败都不 panic，不阻塞构建。

use std::process::Command;

fn main() {
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(commit) = commit {
        println!("cargo:rustc-env=GIT_COMMIT={commit}");
    }

    // HEAD 变了就重跑 build.rs（best-effort，路径不存在也无妨）。
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
}
