//! Span 记录与 ingest 载荷。
//!
//! 设计要点（**特别注意异构**）：信封字段固定，`summary` / `detail` 为开放
//! JSON，由各 `kind` 自定。后端只索引信封、把 summary/detail 当 blob 存，故
//! 新增节点类型（新子流程 / 新长服务）无需后端改表。

use serde::{Deserialize, Serialize};

/// span 完成状态。默认（externally tagged）序列化：`"ok"` 或 `{"error":"..."}`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    Ok,
    Error(String),
}

/// 跨 trace 的关联（周期触发等场景：新 trace 指回原 (trace_id, span_id)）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanLink {
    pub trace_id: String,
    pub span_id: String,
}

/// 一个节点 / （子）流程的记录。客户端异步推送、后端落库的基本单元。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanRecord {
    // ── 固定信封：建树 / 排序 / 检索 ──
    pub trace_id: String,
    pub span_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    /// 发出此 span 的服务名（"zero" | "alarm-server" | "douyin" ...）。
    pub service: String,
    /// 节点类型，驱动 UI 的概要/详情渲染器选择。
    pub kind: String,
    /// 容器 span 的子流程名（如 "闹钟设置"）；叶子节点可为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_name: Option<String>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub status: SpanStatus,

    // ── 概要：小，随树加载，渲染在节点上（各 kind 自定字段）──
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub summary: serde_json::Value,

    // ── 详情：大，点击才拉（各 kind 自定字段）──
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub detail: serde_json::Value,
    /// LLM 类节点的请求/响应原文（后端单独分表懒加载）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<String>,
    /// body 是否因超 `body_limit` 被截断。
    #[serde(default)]
    pub body_truncated: bool,

    // ── 跨 trace 关联 ──
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<SpanLink>,
}

/// `POST /v1/spans` 的请求体。用结构体包裹便于未来加批次级字段（如发送方版本）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRequest {
    pub spans: Vec<SpanRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> SpanRecord {
        SpanRecord {
            trace_id: "a".repeat(32),
            span_id: "b".repeat(16),
            parent_span_id: Some("c".repeat(16)),
            service: "zero".into(),
            kind: "llm_call".into(),
            flow_name: None,
            start_ms: 1,
            end_ms: 2,
            status: SpanStatus::Ok,
            summary: json!({ "model": "claude-opus-4-8", "dur_ms": 1 }),
            detail: json!({ "tokens": 123 }),
            request_body: Some("req".into()),
            response_body: Some("resp".into()),
            body_truncated: false,
            links: vec![],
        }
    }

    #[test]
    fn span_record_round_trip() {
        let rec = sample();
        let s = serde_json::to_string(&rec).unwrap();
        let back: SpanRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back.trace_id, rec.trace_id);
        assert_eq!(back.kind, "llm_call");
        assert_eq!(back.summary["model"], "claude-opus-4-8");
        assert_eq!(back.response_body.as_deref(), Some("resp"));
    }

    #[test]
    fn heterogeneous_summary_detail_serialize() {
        // 不同 kind 概要/详情字段完全不同，均可序列化。
        let alarm = SpanRecord {
            kind: "alarm_submit".into(),
            flow_name: Some("闹钟设置".into()),
            summary: json!({ "once_at": "2026-06-03T22:45:00", "name": "洗澡提醒" }),
            detail: json!({ "alarm_id": "al_1", "callback_body": { "text": "..." } }),
            request_body: None,
            response_body: None,
            ..sample()
        };
        let s = serde_json::to_string(&alarm).unwrap();
        assert!(s.contains("once_at"));
        assert!(s.contains("闹钟设置"));
        // 未携带 body 时字段被跳过
        assert!(!s.contains("request_body"));
    }

    #[test]
    fn status_error_serialization() {
        let s = serde_json::to_string(&SpanStatus::Ok).unwrap();
        assert_eq!(s, "\"ok\"");
        let e = serde_json::to_string(&SpanStatus::Error("boom".into())).unwrap();
        assert_eq!(e, "{\"error\":\"boom\"}");
    }

    #[test]
    fn ingest_request_round_trip() {
        let req = IngestRequest {
            spans: vec![sample(), sample()],
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: IngestRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.spans.len(), 2);
    }
}
