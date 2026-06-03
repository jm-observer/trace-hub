//! query API 的响应视图类型。

use serde::Serialize;
use trace_model::SpanLink;

/// 树节点视图：信封 + 概要（**不含 body / detail**），随整棵树一起返回。
#[derive(Debug, Serialize)]
pub struct NodeView {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub service: String,
    pub kind: String,
    pub flow_name: Option<String>,
    pub start_ms: i64,
    pub end_ms: i64,
    /// 原样回传存库的 status JSON（`"ok"` 或 `{"error":"..."}`）。
    pub status: serde_json::Value,
    pub summary: serde_json::Value,
    /// 是否存有 body / detail（前端据此决定「点击查看详情」是否可用）。
    pub has_detail: bool,
    pub body_truncated: bool,
    pub links: Vec<SpanLink>,
}

/// 单节点详情视图：点击节点时按需拉取的大载荷。
#[derive(Debug, Serialize)]
pub struct DetailView {
    pub detail: serde_json::Value,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
}

/// trace 列表项（按 trace_id 聚合）。
#[derive(Debug, Serialize)]
pub struct TraceSummary {
    pub trace_id: String,
    pub root_service: Option<String>,
    pub root_kind: Option<String>,
    /// 根 span 的 flow_name（无则 None），用作列表标题。
    pub title: Option<String>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub span_count: i64,
}
