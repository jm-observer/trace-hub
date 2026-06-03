# 三服务埋点指南（施工第 4 步）

> 前置：依赖分发决策（DESIGN §7.6，A 发布 / B 临时 path / C 独立 crate）已定，三服务能拿到
> `custom-utils` 的 `trace` feature。本指南给出**每个服务改哪、加什么**。

## 0. 通用接入（每个服务一次）

```toml
# Cargo.toml：让本服务的 custom-utils 带 trace feature（具体来源依 §7.6 决策）
custom-utils = { ..., features = ["...", "trace"] }
```

```rust
// main.rs 启动处（tokio 运行时内）调一次：
custom_utils::trace::init(custom_utils::trace::TraceConfig::new(
    std::env::var("TRACE_HUB_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:9100/v1/spans".into()),
    "zero", // 本服务名
));
```

埋点 API（全部非阻塞）：`record_span` / `record_llm_call` / `inject_traceparent` / `extract_traceparent` / `now_ms` / `TraceContext::{root,child,continued}`。

---

## 1. zero（主中间层 · trace 起点 · 最高价值）

> ⚠️ zero 改 `custom-utils` 来源前先确认与 zero-nova 的 git-tag 耦合（见 [[project_zero_nova_custom_utils_coupling]]），
> 否则 cargo 解析失败。

| 位置 | 动作 | 产出 span |
|---|---|---|
| `crates/channel-gateway/src/lib.rs` `build_inbound_message`(~223) | 普通入站 `TraceContext::root()`；回调入站若 `metadata.traceparent` 存在则 `from_traceparent`→`continued`。挂到 `InboundMessage`（加 `trace` 字段，参照旧 RFC Plan 1） | 主流程根 `user_message` |
| `src/event_handler.rs` route 决策处(~462) | `ctx.child()`，`record_span(kind="router_decision", summary={decision})` | `router_decision` |
| `crates/bridge-claw/src/lib.rs` LLM 调用处（nova send 点，grep 现有 `x-session-id` 注入） | 调用前后取时间，`record_llm_call(LlmCall{ctx:child, model, request_body, response_body, start_ms, end_ms, status})`；同处 `inject_traceparent` 到出站头（sps 也能抓到） | `llm_call`（带 body） |
| alarm/download tool 提交处（OrchestrateTask hook，注入 `[Now]/[Delivery]` 同处） | 把 `ctx.to_traceparent()` 注入 prompt `[Trace]` 行 + 让 skill 抄进 `callback_body.metadata.traceparent`（或 bridge 侧确定性注入，DESIGN 风险项 1） | 子流程容器 `flow_name` |
| 投递处 `send_text_reply` | `record_span(kind="delivery", summary={channel,recipient})` | `delivery` |

## 2. alarm-server（timer-util · 长异步另一端 · 无 LLM body）✅ 已接入并 live 验证

> 已落地（option B：`custom-utils` 改 `path="../../custom-utils"` + `trace` feature）：
> - `main.rs::init_trace()`：仅当设 `TRACE_HUB_ENDPOINT` 时启用（未设零影响）。
> - `callback.rs`：`fire_trace_ctx()` 从 `callback_body.metadata.traceparent` 取回 trace（once→`continued` 同 trace_id；cron→新 trace+`SpanLink`）；`fire_callback` 循环结束后 `record_span(kind="alarm_fire", flow_name="闹钟触发")`；`send_request` 注入 `traceparent` 到回调头。
> - 验证：建 once 闹钟（内嵌 traceparent）→ 回调成功 → trace-hub 出现同 trace_id 的 `alarm_fire`、parent=嵌入 span、status=ok。


| 位置 | 动作 | 产出 span |
|---|---|---|
| `src/main.rs` | `trace::init(.., "alarm-server")` | — |
| `src/handlers.rs` `create_alarm`(~32) | 从请求头 `extract_traceparent`（zero inject 的），把 traceparent **存进 alarm 记录**（随 callback_body 已带也可）；`record_span(kind="alarm_created", flow_name="闹钟设置")` | `alarm_created` |
| `src/callback.rs` `fire_callback`(~39) | 读回存的 traceparent → `continued`（once）或 `root+link`（cron）；`record_span(kind="alarm_fire", flow_name="闹钟触发")`；`inject_traceparent` 到回调 POST 头 | `alarm_fire`（跨异步续接/link） |

> once vs cron 的 trace_id 选择见 DESIGN §3：once 续 `continued` 同 trace_id；cron 用新 trace_id + `SpanLink` 回指设置 span。

## 3. douyin worker（github-commit-info/crates/douyin · 下载长任务）

| 位置 | 动作 | 产出 span |
|---|---|---|
| `main.rs` | `trace::init(.., "douyin")` | — |
| 下载提交入口 | `extract_traceparent`（zero 提交时 inject）；`record_span(kind="download_submit", summary={url})` | `download_submit` |
| 下载完成 / 回调 zero 处 | `record_span(kind="download_done", summary={file,size,dur})`；回调 zero 时 `inject_traceparent` + 写 `callback_body.metadata.traceparent` | `download_done` |

---

## 验证（每个服务接入后）

1. 起 trace-hub（`cargo run -p trace-hub`）。
2. 服务设 `TRACE_HUB_ENDPOINT` 指向它，跑一条真实流程。
3. 打开 `http://<trace-hub>:9100/`，按 trace 看主流程+子流程树，点 LLM 节点看 body。
4. 跨异步：设一个 1 分钟后的 once 闹钟，确认「闹钟触发」子流程挂在**同一条 trace** 下。

## 注意

- 所有 `record_*` 非阻塞、失败即弃，**埋点不会影响业务**；可先在非关键路径试点再铺开。
- 跨进程务必成对 `inject`(出站) / `extract`(入站)，漏一跳则该跳之后断链（DESIGN §「链条只和最不配合的那一跳一样连续」）。
- `kind` 新增无需改 trace-hub；UI 未注册的 kind 走通用兜底渲染。
