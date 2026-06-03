# trace-hub web（React Flow 前端）

trace-hub 的图形界面：Vite + React + [React Flow](https://reactflow.dev) 流程图 + dagre 自动布局。

`vite-plugin-singlefile` 把 JS/CSS 全部内联，**产出单个自包含 `index.html`**，输出到
`../crates/trace-hub/src/ui/index.html`，由 Rust `include_str!` 嵌进二进制——保住
「单二进制 + 离线自包含」（运行时不连任何 CDN）。

## 开发 / 构建
```bash
cd web
npm install
npm run dev      # 本地开发（需 trace-hub 后端在 :9100，dev 代理或同源）
npm run build    # 产出 ../crates/trace-hub/src/ui/index.html（提交此产物）
```
> `src/ui/index.html` 是**构建产物**（已提交，使 `cargo build` 无需 npm）。改 UI 改 `web/`
> 后 `npm run build` 重新生成，再 `cargo build`。

## 视图
- 左：trace 列表 + 搜索
- 中：React Flow 流程图（节点=span，实线=父子，虚线=跨 trace link；按 kind 上色，状态着色）
- 右：点节点看概要 / detail / request·response body
