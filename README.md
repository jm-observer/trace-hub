# trace-hub

独立的「全生命周期 / 全链路」追踪后端 —— 各业务服务（zero、alarm-server、douyin）
通过共享客户端把 **span + 完整 body** 异步推来，按 `trace_id` 组成**主流程 + 子流程**
的因果树，在 Web UI 上以「每节点概要 → 点击详情」的方式展示。

与 `system-prompt-show`(sps，LLM 流量的 MITM 抓包工具) 互补而非替代：trace-hub 看
**跨服务广度（含闹钟/下载等非 LLM 长服务）**，且 body 是一等公民。

设计见 [docs/DESIGN.md](docs/DESIGN.md)。

## 结构

```
crates/
  trace-hub/     # 后端：ingest + SQLite + query API + Web UI（建设中）
```

> 共享契约（SpanRecord / TraceContext / IngestRequest / W3C traceparent）已内联进
> `custom-utils` 的 `trace` feature（`custom_utils::trace`），作为客户端与后端的单一事实源；
> 原独立 `trace-model` crate 已移除。

## 构建

```bash
cargo test            # 跑契约层单测
cargo run -p trace-hub
```
