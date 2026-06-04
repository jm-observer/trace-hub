import React, { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ReactFlow, Background, Controls, MiniMap, Handle, Position,
  useNodesState, useEdgesState,
} from '@xyflow/react'
import { listTraces, getTrace, getSpan, clearAll } from './api.js'
import { layoutGraph } from './layout.js'

// ── kind → 节点行内概要文本；未注册走通用兜底（与后端 UI 约定一致）──
const RENDERERS = {
  user_message: (n) => n.summary?.text ?? '(消息)',
  router_decision: (n) => `${n.summary?.decision ?? ''}`,
  llm_call: (n) => `${n.summary?.model ?? 'llm'}`,
  agent_task: (n) => n.flow_name ?? 'task',
  alarm_submit: (n) => `⏰ ${n.summary?.once_at ?? n.summary?.cron ?? ''} ${n.summary?.name ?? ''}`,
  alarm_fire: (n) => `⏰ ${n.summary?.name ?? ''} ×${n.summary?.attempts ?? ''}`,
  douyin_done: (n) => `${n.summary?.callback_kind ?? ''}`,
  tts: (n) => `🔊 ${n.summary?.voice_id ?? ''} (${n.summary?.text_len ?? 0})`,
  delivery: (n) => `→ ${n.summary?.channel ?? ''} ${n.summary?.recipient ?? ''}`,
}

// ── 列表项「类型」中文标签（侧栏简短显示用；未登记的 kind 原样显示）──
const KIND_LABELS = {
  llm_call: '大模型请求',
  ws_stream: '实时会话',
  asr_transcribe: 'ASR 转写',
  asr_segment: 'ASR 分段',
  audio_decode: '音频解码',
  asr_decode: 'ASR 推理',
  vad_segment: 'VAD 分段',
  tts: 'TTS 合成',
  user_message: '用户消息',
  router_decision: '路由决策',
  agent_task: 'Agent 任务',
  alarm_submit: '闹钟提交',
  alarm_fire: '闹钟触发',
  delivery: '消息投递',
  douyin_done: '抖音完成',
}
function labelOfKind(k) { return KIND_LABELS[k] || k || '未知' }
function fmtTime(ms) {
  if (!ms) return ''
  const d = new Date(ms)
  const pad = (n) => String(n).padStart(2, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}
function kvInline(o) {
  if (!o || typeof o !== 'object') return String(o ?? '')
  return Object.entries(o)
    .map(([k, v]) => `${k}=${typeof v === 'object' ? JSON.stringify(v) : v}`)
    .join(' · ')
}
function summaryText(n) {
  const r = RENDERERS[n.kind]
  if (r) { try { return r(n) } catch { /* fall through */ } }
  return kvInline(n.summary)
}

// ── 把后端的 flat node 列表转成 React Flow 的 nodes/edges + dagre 布局 ──
function buildGraph(nodes) {
  const ids = new Set(nodes.map((n) => n.span_id))
  const rfNodes = nodes.map((n) => ({
    id: n.span_id, type: 'span', position: { x: 0, y: 0 },
    data: { node: n, summary: summaryText(n) },
  }))
  const treeEdges = []
  const linkEdges = []
  nodes.forEach((n) => {
    if (n.parent_span_id && ids.has(n.parent_span_id)) {
      treeEdges.push({
        id: `t-${n.parent_span_id}-${n.span_id}`,
        source: n.parent_span_id, target: n.span_id,
        style: { stroke: '#3a4250' },
      })
    }
    ;(n.links || []).forEach((l, i) => {
      if (ids.has(l.span_id)) {
        linkEdges.push({
          id: `l-${n.span_id}-${l.span_id}-${i}`,
          source: l.span_id, target: n.span_id, label: 'link',
          style: { stroke: '#d2a8ff', strokeDasharray: '5 5' },
        })
      }
    })
  })
  const positioned = layoutGraph(rfNodes, treeEdges)
  return { nodes: positioned, edges: [...treeEdges, ...linkEdges] }
}

function SpanNode({ data, selected }) {
  const n = data.node
  const ok = n.status === 'ok'
  const cls =
    'span-node' + (n.flow_name ? ' flow' : '') + (ok ? '' : ' err') + (selected ? ' sel' : '')
  return (
    <div className={cls}>
      <Handle type="target" position={Position.Top} style={{ opacity: 0 }} />
      <div className="sn-head">
        <span className="sn-kind">{n.flow_name || n.kind}</span>
        <span className="sn-svc">{n.service}</span>
      </div>
      <div className="sn-sum">{data.summary}</div>
      <div className="sn-meta">
        <span className="sn-time">{fmtTime(n.start_ms)}</span>
        {' · '}<span className={ok ? 'ok' : 'er'}>{ok ? 'ok' : 'err'}</span>
        {' · '}{Math.max(0, n.end_ms - n.start_ms)}ms
        {n.has_detail ? ' · ⓘ' : ''}
        {n.links && n.links.length ? ' · ⇄' : ''}
      </div>
      <Handle type="source" position={Position.Bottom} style={{ opacity: 0 }} />
    </div>
  )
}
const nodeTypes = { span: SpanNode }

function kvTable(o) {
  if (!o || typeof o !== 'object') return <pre>{JSON.stringify(o)}</pre>
  const ent = Object.entries(o)
  if (!ent.length) return <div className="muted">（无）</div>
  return (
    <table className="kv"><tbody>
      {ent.map(([k, v]) => (
        <tr key={k}>
          <td className="k">{k}</td>
          <td>{typeof v === 'object' ? JSON.stringify(v) : String(v)}</td>
        </tr>
      ))}
    </tbody></table>
  )
}

// ── LLM 请求体解析：从 OpenAI/Anthropic chat 调用还原 messages / tools / params。
//    失败时返回 null（调用方走 raw 兜底）。
function parseLlmRequest(raw) {
  if (typeof raw !== 'string' || !raw.trim()) return null
  let obj
  try { obj = JSON.parse(raw) } catch { return null }
  if (!obj || typeof obj !== 'object') return null
  const messages = Array.isArray(obj.messages) ? obj.messages : null
  if (!messages) return null
  const { messages: _m, tools, ...rest } = obj
  return { messages, tools: Array.isArray(tools) ? tools : null, params: rest }
}

// ── LLM 响应体聚合：把 OpenAI streaming 的 chunk 数组合成一条 assistant 消息。
//    支持的输入形状：
//    1) chunk 数组（每项有 choices[0].delta.content / tool_calls；末尾带 usage）
//    2) 单条 chat.completion 对象（choices[0].message.{content,tool_calls}）
//    3) 其它 → 返回 null 走 raw 兜底
function aggregateLlmResponse(raw) {
  if (typeof raw !== 'string' || !raw.trim()) return null
  let obj
  try { obj = JSON.parse(raw) } catch { return null }
  // 单条 chat.completion
  if (obj && !Array.isArray(obj) && obj.choices && obj.choices[0] && obj.choices[0].message) {
    const m = obj.choices[0].message
    return {
      content: typeof m.content === 'string' ? m.content : '',
      tool_calls: Array.isArray(m.tool_calls) ? m.tool_calls : [],
      usage: obj.usage || null,
      finish_reason: obj.choices[0].finish_reason || null,
      model: obj.model || null,
      n_chunks: 1,
    }
  }
  // streaming chunk 数组
  if (!Array.isArray(obj) || obj.length === 0) return null
  let content = ''
  const toolCallsById = new Map() // index|id → {id, type, function:{name, arguments(str)}}
  let usage = null
  let finishReason = null
  let model = null
  for (const c of obj) {
    if (!c || typeof c !== 'object') continue
    if (!model && c.model) model = c.model
    if (c.usage) usage = c.usage
    const ch = Array.isArray(c.choices) ? c.choices[0] : null
    if (!ch) continue
    if (ch.finish_reason) finishReason = ch.finish_reason
    const d = ch.delta
    if (!d) continue
    if (typeof d.content === 'string') content += d.content
    if (Array.isArray(d.tool_calls)) {
      for (const tc of d.tool_calls) {
        const key = tc.id ?? (tc.index != null ? `idx:${tc.index}` : null)
        if (key == null) continue
        const cur = toolCallsById.get(key) || { id: tc.id || '', type: tc.type || 'function', function: { name: '', arguments: '' } }
        if (tc.id && !cur.id) cur.id = tc.id
        if (tc.type && !cur.type) cur.type = tc.type
        if (tc.function) {
          if (tc.function.name) cur.function.name = (cur.function.name || '') + tc.function.name
          if (typeof tc.function.arguments === 'string') cur.function.arguments += tc.function.arguments
        }
        toolCallsById.set(key, cur)
      }
    }
  }
  return {
    content,
    tool_calls: [...toolCallsById.values()],
    usage,
    finish_reason: finishReason,
    model,
    n_chunks: obj.length,
  }
}

// ── 消息块（system / user / assistant / tool）渲染。可折叠；长 content 收起。
function MessageBlock({ msg, idx, lastUserIdx }) {
  const role = msg.role || 'unknown'
  const text = typeof msg.content === 'string'
    ? msg.content
    : Array.isArray(msg.content)
      ? msg.content.map((p) => p.text || '').join('')
      : msg.content == null ? '' : (JSON.stringify(msg.content) ?? '')
  const isLong = text.length > 400
  const [open, setOpen] = useState(!isLong)
  const isLastUser = idx === lastUserIdx
  return (
    <div className={'msg msg-' + role + (isLastUser ? ' last-user' : '')} data-msg-idx={idx}>
      <div className="msg-head">
        <span className={'msg-role role-' + role}>{role}</span>
        <span className="msg-len muted">{text.length} chars</span>
        {isLong && (
          <button className="btn-tiny" onClick={() => setOpen(!open)}>{open ? '折叠' : '展开'}</button>
        )}
      </div>
      {open && <pre className="msg-body">{text}</pre>}
      {msg.tool_calls && msg.tool_calls.length > 0 && (
        <div className="msg-tools">
          {msg.tool_calls.map((tc, i) => (
            <div key={i} className="tool-call">
              <span className="tc-name">tool · {tc.function?.name || tc.name || '?'}</span>
              <pre className="tc-args">{tc.function?.arguments || ''}</pre>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

// ── LLM 请求 / 响应专用面板。kind=llm_call 时启用，其它走 raw。
function LlmBodies({ requestRaw, responseRaw }) {
  const req = useMemo(() => parseLlmRequest(requestRaw), [requestRaw])
  const resp = useMemo(() => aggregateLlmResponse(responseRaw), [responseRaw])
  const [showRawReq, setShowRawReq] = useState(false)
  const [showRawResp, setShowRawResp] = useState(false)

  // 找最后一条 user 消息位置，用于"跳到最后用户消息"
  const lastUserIdx = useMemo(() => {
    if (!req?.messages) return -1
    for (let i = req.messages.length - 1; i >= 0; i--) if (req.messages[i].role === 'user') return i
    return -1
  }, [req])

  const jumpToLastUser = useCallback(() => {
    if (lastUserIdx < 0) return
    const el = document.querySelector(`[data-msg-idx="${lastUserIdx}"]`)
    if (!el) return
    el.scrollIntoView({ behavior: 'smooth', block: 'center' })
    el.classList.add('flash')
    setTimeout(() => el.classList.remove('flash'), 1800)
  }, [lastUserIdx])

  return (
    <>
      {requestRaw != null && (
        <>
          <h3>
            请求
            <button className="btn-tiny right" onClick={() => setShowRawReq((v) => !v)}>
              {showRawReq ? '结构化' : '原始 JSON'}
            </button>
            {!showRawReq && req && lastUserIdx >= 0 && (
              <button className="btn-tiny right" onClick={jumpToLastUser} title="平滑滚动到 messages 数组里最后一条 role=user">
                ↧ 跳到最后用户消息
              </button>
            )}
          </h3>
          {showRawReq || !req ? (
            <pre>{requestRaw}</pre>
          ) : (
            <div className="llm-req">
              {req.params && Object.keys(req.params).length > 0 && (
                <div className="llm-params">
                  {Object.entries(req.params).filter(([k]) => k !== 'stream').map(([k, v]) => (
                    <span key={k} className="param-chip"><b>{k}</b>: {typeof v === 'object' ? JSON.stringify(v) : String(v)}</span>
                  ))}
                </div>
              )}
              {req.tools && (
                <details className="tools-block">
                  <summary>tools ({req.tools.length})</summary>
                  <pre>{JSON.stringify(req.tools, null, 2)}</pre>
                </details>
              )}
              <div className="msg-list">
                {req.messages.map((m, i) => (
                  <MessageBlock key={i} msg={m} idx={i} lastUserIdx={lastUserIdx} />
                ))}
              </div>
            </div>
          )}
        </>
      )}
      {responseRaw != null && (
        <>
          <h3>
            响应（聚合）
            <button className="btn-tiny right" onClick={() => setShowRawResp((v) => !v)}>
              {showRawResp ? '聚合' : '原始 JSON'}
            </button>
          </h3>
          {showRawResp || !resp ? (
            <pre>{responseRaw}</pre>
          ) : (
            <div className="llm-resp">
              <div className="llm-params">
                {resp.model && <span className="param-chip"><b>model</b>: {resp.model}</span>}
                {resp.finish_reason && <span className="param-chip"><b>finish</b>: {resp.finish_reason}</span>}
                {resp.n_chunks > 1 && <span className="param-chip"><b>chunks</b>: {resp.n_chunks}</span>}
                {resp.usage && Object.entries(resp.usage).filter(([, v]) => v != null).map(([k, v]) => (
                  <span key={k} className="param-chip"><b>{k}</b>: {typeof v === 'object' ? JSON.stringify(v) : String(v)}</span>
                ))}
              </div>
              {resp.content && (
                <div className="msg msg-assistant">
                  <div className="msg-head"><span className="msg-role role-assistant">assistant</span><span className="msg-len muted">{resp.content.length} chars</span></div>
                  <pre className="msg-body">{resp.content}</pre>
                </div>
              )}
              {resp.tool_calls && resp.tool_calls.length > 0 && (
                <div className="msg-tools">
                  {resp.tool_calls.map((tc, i) => (
                    <div key={i} className="tool-call">
                      <span className="tc-name">tool · {tc.function?.name || '?'} {tc.id && <code className="muted">{tc.id}</code>}</span>
                      <pre className="tc-args">{(() => {
                        try { return JSON.stringify(JSON.parse(tc.function?.arguments || '{}'), null, 2) }
                        catch { return tc.function?.arguments || '' }
                      })()}</pre>
                    </div>
                  ))}
                </div>
              )}
              {!resp.content && (!resp.tool_calls || resp.tool_calls.length === 0) && (
                <div className="muted">（无 content / tool_calls；点上方"原始 JSON"查看）</div>
              )}
            </div>
          )}
        </>
      )}
    </>
  )
}

function Detail({ sel }) {
  const n = sel.node
  const d = sel.detail
  const hasDetail = d && d.detail && typeof d.detail === 'object' && Object.keys(d.detail).length > 0
  const isLlm = n.kind === 'llm_call'
  return (
    <div>
      <div className="muted">
        {n.service} / {n.kind} / span {n.span_id.slice(0, 12)}…
        {n.body_truncated ? ' · body 截断' : ''}
      </div>
      <h3>概要</h3>
      {kvTable(n.summary)}
      {hasDetail && (<><h3>detail</h3><pre>{JSON.stringify(d.detail, null, 2)}</pre></>)}
      {d && isLlm && (d.request_body != null || d.response_body != null) ? (
        <LlmBodies requestRaw={d.request_body} responseRaw={d.response_body} />
      ) : (
        <>
          {d && d.request_body != null && (<><h3>body · request</h3><pre>{d.request_body}</pre></>)}
          {d && d.response_body != null && (<><h3>body · response</h3><pre>{d.response_body}</pre></>)}
        </>
      )}
    </div>
  )
}

// 自动刷新间隔（毫秒）。4s 是肉眼"近实时"与轮询开销的折中。
const AUTO_REFRESH_MS = 4000

export default function App() {
  const [traces, setTraces] = useState([])
  const [q, setQ] = useState('')
  const [traceId, setTraceId] = useState(null)
  const [nodes, setNodes, onNodesChange] = useNodesState([])
  const [edges, setEdges, onEdgesChange] = useEdgesState([])
  const [sel, setSel] = useState(null)
  const [status, setStatus] = useState('')
  // 自动刷新开关：默认开。用户在搜索框聚焦时暂停（避免输入时列表跳动）。
  const [autoOn, setAutoOn] = useState(true)
  const [searchFocused, setSearchFocused] = useState(false)
  // 用 ref 持当前 query，避免 effect 把它进依赖后每键一字重启 interval。
  const qRef = React.useRef(q)
  useEffect(() => { qRef.current = q }, [q])

  const refreshList = useCallback(async (query) => {
    try {
      const { traces } = await listTraces(query)
      setTraces(traces)
      setStatus(`${traces.length} traces`)
    } catch (e) {
      setStatus('加载失败: ' + e.message)
    }
  }, [])
  useEffect(() => { refreshList('') }, [refreshList])

  // 自动轮询刷新列表（不动选中的 trace；getTrace 的拓扑由 openTrace 单次拉取）。
  useEffect(() => {
    if (!autoOn || searchFocused) return
    const id = setInterval(() => { refreshList(qRef.current) }, AUTO_REFRESH_MS)
    return () => clearInterval(id)
  }, [autoOn, searchFocused, refreshList])

  const openTrace = useCallback(async (id) => {
    setTraceId(id)
    setSel(null)
    try {
      const { nodes } = await getTrace(id)
      const g = buildGraph(nodes)
      setNodes(g.nodes)
      setEdges(g.edges)
    } catch (e) {
      setStatus('打开失败: ' + e.message)
    }
  }, [setNodes, setEdges])

  const onClear = useCallback(async () => {
    if (!window.confirm('清空全部 trace 记录？此操作不可撤销。')) return
    try {
      const { deleted } = await clearAll()
      setTraceId(null)
      setSel(null)
      setNodes([])
      setEdges([])
      await refreshList('')
      setStatus(`已清空 ${deleted} 条 span`)
    } catch (e) {
      setStatus('清空失败: ' + e.message)
    }
  }, [refreshList, setNodes, setEdges])

  const onNodeClick = useCallback(async (_e, rfNode) => {
    const n = rfNode.data.node
    setNodes((ns) => ns.map((x) => ({ ...x, selected: x.id === rfNode.id })))
    let detail = null
    if (n.has_detail) { try { detail = await getSpan(n.span_id) } catch { /* ignore */ } }
    setSel({ node: n, detail })
  }, [setNodes])

  return (
    <div className="app">
      <div className="col sidebar">
        <header>
          <b>trace-hub</b> <span className="muted">{status}</span>
          <button className="clear-btn" onClick={onClear} title="清空全部记录">清空</button>
        </header>
        <input
          placeholder="搜索 service/kind/概要…回车"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') refreshList(q) }}
          onFocus={() => setSearchFocused(true)}
          onBlur={() => setSearchFocused(false)}
        />
        <div className="auto-toggle">
          <label title="每 4s 自动拉取最新列表；搜索框聚焦时暂停以免输入抖动">
            <input
              type="checkbox"
              checked={autoOn}
              onChange={(e) => setAutoOn(e.target.checked)}
            />
            自动刷新（4s）{searchFocused && autoOn ? '· 已暂停' : ''}
          </label>
        </div>
        <div>
          {traces.map((t) => {
            const svc = t.root_service ?? '?'
            const kindLabel = t.title || labelOfKind(t.root_kind)
            return (
              <div
                key={t.trace_id}
                className={'trace-item' + (t.trace_id === traceId ? ' sel' : '')}
                onClick={() => openTrace(t.trace_id)}
              >
                <div className="ti-head">
                  <span className={'svc-chip svc-' + svc.replace(/[^a-z0-9]/gi, '-')}>{svc}</span>
                  <span className="kind-label">{kindLabel}</span>
                </div>
                <div className="ti-meta">
                  <code title={t.trace_id}>{t.trace_id.slice(0, 12)}…</code>
                  <span>· {t.span_count} spans</span>
                  <span>· {fmtTime(t.start_ms)}</span>
                </div>
              </div>
            )
          })}
        </div>
      </div>

      <div className="col graph">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onNodeClick={onNodeClick}
          fitView
          minZoom={0.2}
          proOptions={{ hideAttribution: true }}
        >
          <Background />
          <Controls />
          <MiniMap pannable zoomable />
        </ReactFlow>
      </div>

      <div className="col detail">
        {sel ? <Detail sel={sel} /> : <div className="muted">选择左侧 trace，点节点看详情</div>}
      </div>
    </div>
  )
}
