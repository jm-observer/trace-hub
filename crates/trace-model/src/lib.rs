//! trace-model —— trace-hub 体系的共享数据契约。
//!
//! 客户端（各业务服务经 `custom-utils::trace`）与后端（`trace-hub`）都依赖本
//! crate，作为 span/trace 数据结构与 W3C traceparent 传播格式的**单一事实源**，
//! 避免两侧字段漂移。
//!
//! 核心概念：
//! - 一条完整生命周期 = 一棵树 = 同一个 `trace_id`。
//! - 主流程 / 子流程 / 节点都是 [`SpanRecord`]，靠 `span_id` / `parent_span_id`
//!   建树；有子节点的 span 即「（子）流程」，叶子即「节点」。
//! - 跨异步（如闹钟到点回调）：一次性场景续用同 `trace_id`（[`TraceContext::continued`]）；
//!   周期场景新起 `trace_id` 并用 [`SpanLink`] 指回原 span。
//! - **信封固定、载荷开放**：信封字段（见 [`SpanRecord`]）供建树/排序/检索；
//!   `summary` / `detail` 是开放 JSON，由各 `kind` 自定，后端不解析其内部结构，
//!   故新增节点类型无需后端改表。

pub mod context;
pub mod record;

pub use context::{gen_span_id, gen_trace_id, TraceContext};
pub use record::{IngestRequest, SpanLink, SpanRecord, SpanStatus};
