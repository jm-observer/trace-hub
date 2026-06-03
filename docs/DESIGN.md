# trace-hub 设计（概要）

> 创建 2026-06-03。独立的「全生命周期 / 全链路」追踪后端 + 配套客户端埋点。
> body 为一等公民（区别于 OTel/Jaeger 把大 body 视为反模式），自有时间线 UI。

## 1. 定位

- **是什么**：一个**独立项目**（类似 Jaeger 的角色：自有 ingest + 存储 + 时间线 UI），
  各业务服务通过共享客户端把「span + body」**异步、非阻塞**地推来。
- **不是什么**：不寄生在 `system-prompt-show`(sps) 上（sps 保持 MITM 调试定位不扩展）；
  不依赖 OpenTelemetry SDK / Jaeger；不强制 `tracing` crate。
- **与 sps 的关系**：sps = LLM 流量的被动 MITM 抓包（保留）；trace-hub = 主动推送的
  跨服务生命周期 + body，二者独立。

## 2. 三个部件

```
业务服务(zero / alarm-server / douyin)            trace-hub（本仓）
  │  custom-utils::trace 客户端（路线 B：              ┌──────────────┐
  │  有界队列 + 后台异步 POST，零阻塞）                │ ingest /v1/spans │
  └── 推 SpanRecord(含 body) ─────────────────────►  │ SQLite 存储    │
        traceparent 跨进程/跨异步传播                  │ query API      │
                                                      │ Web UI(树+详情) │
                                                      └──────────────┘
```

| 部件 | 位置 | 状态 |
|---|---|---|
| 共享契约 `trace-model` | `trace-hub/crates/trace-model` | ✅ 已建（SpanRecord/TraceContext/traceparent） |
| 后端 `trace-hub` | `trace-hub/crates/trace-hub` | ✅ ingest + SQLite + query API（端到端冒烟通过） |
| 客户端 `trace` feature | `custom-utils`（路线 B） | ✅ 已建（init/record_span/record_llm_call/inject/extract，9 测试绿） |
| Web UI | `trace-hub`（内嵌 `src/ui/index.html`，路由 `/`） | ✅ 已建（列表+流程树+节点详情+kind 渲染器+通用兜底+搜索） |
| 三服务埋点 | alarm-server ✅(option B path 依赖，live 验证跨异步同 trace) / douyin 🚧 / zero 🚧(缓，nova 耦合) | 进行中 |

## 3. 核心模型：一棵「流程树」

**一个 `trace_id` = 一条完整生命周期 = 一棵树。** 详见 `trace-model` 的 doc。

- **主流程**=树根 span；**子流程**=有子节点的中间 span（带 `flow_name`，如「闹钟设置」）；
  **节点**=叶子 span。三者同为 `SpanRecord`，靠 `span_id`/`parent_span_id` 建树。
- **trace_id 关系**：父流程与子流程**共享同一个 trace_id**；父子关系由 span_id 表达，
  子流程**不另起 trace_id**。
- **跨异步**：
  - 一次性（once 闹钟 / 单次下载）→ 续用同 `trace_id`（`TraceContext::continued`）。
  - 周期（cron 闹钟）→ 新 `trace_id` + `SpanLink` 指回设置时的 span（否则一棵树无限膨胀）。

### 闹钟（once）示例
```
▣ 主流程 trace=tr_A  「帮我设个闹钟」            [user_message]
├─ ● 路由决策                                    [router_decision]
├─ ▣ 子流程：闹钟设置 (flow_name)
│   ├─ ● Agent LLM 调用  ⟶ 详情含 body          [llm_call]
│   └─ ● alarm-cli 提交  once_at/alarm_id        [alarm_submit]
╎    ⌛ link（traceparent 经 callback_body 往返）
└─ ▣ 子流程：闹钟触发（2h 后，同 tr_A）
    ├─ ● 回调入站                                [callback_inbound]
    ├─ ● Agent LLM 调用  ⟶ 详情含 body          [llm_call]
    └─ ● 投递用户                                [delivery]
```

## 4. 异构概要/详情（**特别注意点**）

**信封固定、载荷开放**：
- 信封字段（trace_id/span_id/parent/service/kind/flow_name/时间/status/links）固定，
  供后端建树/排序/检索/索引。
- `summary`（小，随树加载，渲染在节点上）与 `detail`（大，点击才拉）+ body 是**开放 JSON**，
  由各 `kind` 自定。后端**不解析其内部**，当 blob 存 → 新增节点类型无需后端改表。
- UI 用 **`kind` → 渲染器** 注册表渲染概要/详情；**必须有通用兜底渲染器**（未知 kind
  也能展示：summary 平铺 KV、detail 倒 JSON），保证体系从第一天完整，自定义渲染器为渐进增强。

## 5. 客户端（路线 B，落在 custom-utils）

机制：`OnceCell<Sender>` + 有界 mpsc + 后台 tokio 任务攒批 POST。

- `record_*()` 用 `try_send` 非阻塞入队，满了即丢 + 计数；**绝不阻塞、绝不上抛、
  trace-hub 挂了不影响业务**。
- 后台任务带超时 POST `/v1/spans`，失败仅 `log::debug!` 后丢弃。
- 传播：`inject(ctx, headers)` 出站写 traceparent；`extract(headers)` 入站解析；
  跨异步把 `ctx.to_traceparent()` 塞进持久载荷（`callback_body.metadata.traceparent`）往返。
- feature 门控，不开启零影响（压低 custom-utils ↔ zero-nova 版本耦合波及面）。

## 6. 后端契约（query API 草案，待实现细化）

| 端点 | 用途 |
|---|---|
| `POST /v1/spans` | ingest（收 `IngestRequest`） |
| `GET /v1/traces/:trace_id` | 整棵树（信封 + summary，不含 body） |
| `GET /v1/spans/:span_id` | 单节点 detail + body |
| `GET /v1/traces?q=…` | trace 列表 / 搜索 |
| `GET /v1/events` | SSE 实时刷新 |

存储（SQLite）：`span`（信封+summary，索引 trace_id）/ `span_detail`（detail+body，懒加载）/
`span_link`。保留期 + body 大小上限。

## 7. 待定项（施工中逐个敲定）

1. **trace-model 分发**：当前 `path` 依赖；custom-utils 为已发布 crate，接入 `trace`
   feature 时需把 trace-model 改为 git-tag 依赖（参照 zero-nova 模式）。
2. 概要由客户端装好（推荐）vs 后端派生 —— 当前定为客户端装好。
3. 子流程边界：结构自然形成（推荐）vs 显式 `start_flow()` —— 当前定为结构 + `flow_name`。
4. UI 先做折叠树还是时间轴瀑布。
5. 服务名/时钟对齐（跨机时间线）/ body 默认全量 vs 截断。
6. **【阻塞第 4 步】依赖分发**：zero / alarm-server / douyin 均经 crates.io 版本依赖
   `custom-utils 0.14.x`，拿不到本地的 `trace` feature。三选一：
   - **A 发布**：发布 `trace-model` + `custom-utils 0.15`（含 trace），三服务 bump 版本 + 开
     `trace` feature。最正规，但需 crates.io 发布 + 协调 zero-nova（git-tag 钉住 custom-utils）。
   - **B 临时 path/git 依赖**：三服务 `custom-utils` 改指本地 path/git（含 trace）。最快，
     但改了生产依赖来源、可能影响各自 CI。可逆。
   - **C 独立 trace 客户端 crate**：把客户端从 custom-utils 拆成独立小 crate，三服务直接依赖，
     绕开 custom-utils 发布与 zero-nova 耦合。与「客户端落 custom-utils」的原定决策相悖，备选。

## 施工顺序

1. ✅ trace-hub 脚手架 + `trace-model` 契约
2. ✅ trace-hub 后端：ingest + SQLite + query API
3. ✅ custom-utils `trace` feature（路线 B 客户端）
5. ✅ Web UI（流程树 + 概要→详情）—— 内嵌单页，路由 `/`
4. 🚧 三服务埋点（zero / alarm-server / douyin）—— **阻塞于依赖分发决策（§7.6）**

### 客户端 API（custom-utils `trace` feature）
```rust
custom_utils::trace::init(TraceConfig::new("http://g10:9100/v1/spans", "zero"));
let ctx = parent.child();                       // 同 trace_id 新 span
trace::inject_traceparent(&ctx, &mut headers);  // 出站
let ctx = trace::extract_traceparent(|h| ...);  // 入站
trace::record_llm_call(LlmCall { ctx, model, request_body, response_body, start_ms, end_ms, status });
trace::record_span(SpanRecord { .. });          // 任意 kind
```

