//! HTTP 路由与 handler。query API 见 `docs/DESIGN.md` §6。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use custom_utils::trace::IngestRequest;

use crate::error::AppError;
use crate::storage::Storage;

/// 单页 UI（内嵌）：trace 列表 + 流程树 + 节点详情。
async fn ui() -> Html<&'static str> {
    Html(include_str!("ui/index.html"))
}

pub fn router(storage: Storage) -> Router {
    Router::new()
        .route("/", get(ui))
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/spans", post(ingest))
        .route("/v1/traces", get(list_traces))
        .route("/v1/traces/{trace_id}", get(get_trace))
        .route("/v1/spans/{span_id}", get(get_span))
        .with_state(storage)
}

/// ingest：收一批 span 落库。返回 `{accepted: N}`。
async fn ingest(
    State(st): State<Storage>,
    Json(req): Json<IngestRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let n = tokio::task::spawn_blocking(move || st.insert_spans(req.spans)).await??;
    Ok(Json(json!({ "accepted": n })))
}

#[derive(Debug, Deserialize)]
struct ListParams {
    q: Option<String>,
    limit: Option<i64>,
}

/// trace 列表 / 搜索。
async fn list_traces(
    State(st): State<Storage>,
    Query(p): Query<ListParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    let limit = p.limit.unwrap_or(100).clamp(1, 1000);
    let q = p.q.clone();
    let traces = tokio::task::spawn_blocking(move || st.list_traces(q.as_deref(), limit)).await??;
    Ok(Json(json!({ "traces": traces })))
}

/// 一条 trace 的整棵树（信封 + 概要，不含 body）。
async fn get_trace(
    State(st): State<Storage>,
    Path(trace_id): Path<String>,
) -> Result<Response, AppError> {
    let nodes = tokio::task::spawn_blocking(move || st.trace_nodes(&trace_id)).await??;
    if nodes.is_empty() {
        return Ok((StatusCode::NOT_FOUND, "trace not found").into_response());
    }
    Ok(Json(json!({ "nodes": nodes })).into_response())
}

/// 单节点详情 + body（点击节点时拉）。
async fn get_span(
    State(st): State<Storage>,
    Path(span_id): Path<String>,
) -> Result<Response, AppError> {
    let detail = tokio::task::spawn_blocking(move || st.span_detail(&span_id)).await??;
    match detail {
        Some(d) => Ok(Json(d).into_response()),
        None => Ok((StatusCode::NOT_FOUND, "span detail not found").into_response()),
    }
}
