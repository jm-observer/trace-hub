//! 传播上下文（W3C Trace Context 兼容）。
//!
//! `traceparent` 格式：`{version}-{trace_id}-{span_id}-{flags}`，本实现固定
//! version=`00`、trace_id 32 hex、span_id 16 hex、flags 2 hex（bit0 = sampled）。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 生成 32 hex 的 trace_id（128-bit，对齐 W3C trace-id）。
pub fn gen_trace_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// 生成 16 hex 的 span_id（64-bit，对齐 W3C parent-id）。取 uuid 前 8 字节。
pub fn gen_span_id() -> String {
    let bytes = Uuid::new_v4().into_bytes();
    let mut s = String::with_capacity(16);
    for b in &bytes[..8] {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// 进程间/进程内传播的 trace 上下文。
///
/// `span_id` 表示「当前活跃 span」。出站时把它作为 traceparent 的 parent-id
/// 写出；下游 [`from_traceparent`](TraceContext::from_traceparent) 解析后再
/// [`child`](TraceContext::child) 出本地 span（其 `parent_span_id` 即上游 span）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub sampled: bool,
}

impl TraceContext {
    /// 一条新生命周期的根：新 trace_id + 新 span，无父。
    pub fn root() -> Self {
        Self {
            trace_id: gen_trace_id(),
            span_id: gen_span_id(),
            parent_span_id: None,
            sampled: true,
        }
    }

    /// 同步下游 / 进程内子跳：**复用 trace_id**，开新 span，父 = 当前 span。
    pub fn child(&self) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            span_id: gen_span_id(),
            parent_span_id: Some(self.span_id.clone()),
            sampled: self.sampled,
        }
    }

    /// 一次性异步续接（如 once 闹钟到点回调）：**复用给定 trace_id**，开新 span，
    /// 父 = 提交时的 span。配合载荷里往返的 traceparent 使用。
    pub fn continued(trace_id: impl Into<String>, parent_span_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: gen_span_id(),
            parent_span_id: Some(parent_span_id.into()),
            sampled: true,
        }
    }

    /// 出站注入：`00-{trace_id}-{span_id}-{flags}`。
    pub fn to_traceparent(&self) -> String {
        let flags = if self.sampled { "01" } else { "00" };
        format!("00-{}-{}-{}", self.trace_id, self.span_id, flags)
    }

    /// 入站解析：返回**远端当前 span** 的上下文（`parent_span_id = None`）。
    /// 本地随后调用 [`child`](TraceContext::child) 开本地 span。非法格式返回 `None`。
    pub fn from_traceparent(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.trim().split('-').collect();
        let [version, trace_id, span_id, flags] = parts.as_slice() else {
            return None;
        };
        if *version != "00" || trace_id.len() != 32 || span_id.len() != 16 || flags.len() != 2 {
            return None;
        }
        if !is_hex(trace_id) || !is_hex(span_id) || !is_hex(flags) {
            return None;
        }
        // 全零的 trace_id / span_id 在 W3C 规范里非法。
        if trace_id.bytes().all(|b| b == b'0') || span_id.bytes().all(|b| b == b'0') {
            return None;
        }
        let sampled = u8::from_str_radix(flags, 16)
            .map(|b| b & 1 == 1)
            .unwrap_or(false);
        Some(Self {
            trace_id: (*trace_id).to_string(),
            span_id: (*span_id).to_string(),
            parent_span_id: None,
            sampled,
        })
    }
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_has_no_parent_and_well_formed_ids() {
        let r = TraceContext::root();
        assert!(r.parent_span_id.is_none());
        assert_eq!(r.trace_id.len(), 32);
        assert_eq!(r.span_id.len(), 16);
        assert!(is_hex(&r.trace_id) && is_hex(&r.span_id));
    }

    #[test]
    fn child_keeps_trace_id_and_links_parent() {
        let r = TraceContext::root();
        let c = r.child();
        assert_eq!(c.trace_id, r.trace_id, "子流程复用同一 trace_id");
        assert_ne!(c.span_id, r.span_id);
        assert_eq!(c.parent_span_id.as_deref(), Some(r.span_id.as_str()));
    }

    #[test]
    fn continued_reuses_trace_id_with_given_parent() {
        let c = TraceContext::continued("a".repeat(32), "b".repeat(16));
        assert_eq!(c.trace_id, "a".repeat(32));
        assert_eq!(c.parent_span_id.as_deref(), Some("bbbbbbbbbbbbbbbb"));
        assert_eq!(c.span_id.len(), 16);
        assert_ne!(c.span_id, "b".repeat(16));
    }

    #[test]
    fn traceparent_round_trip() {
        let r = TraceContext::root();
        let tp = r.to_traceparent();
        let back = TraceContext::from_traceparent(&tp).expect("parse");
        assert_eq!(back.trace_id, r.trace_id);
        assert_eq!(back.span_id, r.span_id);
        assert!(back.sampled);
        // 解析结果代表远端当前 span，本地 child 的 parent 应指向它。
        let local = back.child();
        assert_eq!(local.parent_span_id.as_deref(), Some(r.span_id.as_str()));
        assert_eq!(local.trace_id, r.trace_id);
    }

    #[test]
    fn rejects_malformed_traceparent() {
        assert!(TraceContext::from_traceparent("garbage").is_none());
        assert!(TraceContext::from_traceparent("00-short-bbbbbbbbbbbbbbbb-01").is_none());
        assert!(TraceContext::from_traceparent("01-{}-{}-01").is_none());
        // 全零非法
        let zero = format!("00-{}-{}-01", "0".repeat(32), "0".repeat(16));
        assert!(TraceContext::from_traceparent(&zero).is_none());
    }
}
