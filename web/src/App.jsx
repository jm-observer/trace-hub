import React, { useCallback, useEffect, useState } from 'react'
import {
  ReactFlow, Background, Controls, MiniMap, Handle, Position,
  useNodesState, useEdgesState,
} from '@xyflow/react'
import { listTraces, getTrace, getSpan } from './api.js'
import { layoutGraph } from './layout.js'

// ── kind → 节点行内概要文本；未注册走通用兜底（与后端 UI 约定一致）──
const RENDERERS = {
  user_message: (n) => n.summary?.text ?? '(消息)',
  router_decision: (n) => `${n.summary?.decision ?? ''}`,
  llm_call: (n) => `${n.summary?.model ?? 'llm'} · ${n.summary?.dur_ms ?? '?'}ms`,
  agent_task: (n) => n.flow_name ?? 'task',
  alarm_submit: (n) => `⏰ ${n.summary?.once_at ?? n.summary?.cron ?? ''} ${n.summary?.name ?? ''}`,
  alarm_fire: (n) => `⏰ ${n.summary?.name ?? ''} ×${n.summary?.attempts ?? ''}`,
  douyin_done: (n) => `${n.summary?.callback_kind ?? ''}`,
  tts: (n) => `🔊 ${n.summary?.voice_id ?? ''} (${n.summary?.text_len ?? 0})`,
  delivery: (n) => `→ ${n.summary?.channel ?? ''} ${n.summary?.recipient ?? ''}`,
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
        <span className={ok ? 'ok' : 'er'}>{ok ? 'ok' : 'err'}</span>
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

function Detail({ sel }) {
  const n = sel.node
  const d = sel.detail
  const hasDetail = d && d.detail && typeof d.detail === 'object' && Object.keys(d.detail).length > 0
  return (
    <div>
      <div className="muted">
        {n.service} / {n.kind} / span {n.span_id.slice(0, 12)}…
        {n.body_truncated ? ' · body 截断' : ''}
      </div>
      <h3>概要</h3>
      {kvTable(n.summary)}
      {hasDetail && (<><h3>detail</h3><pre>{JSON.stringify(d.detail, null, 2)}</pre></>)}
      {d && d.request_body != null && (<><h3>body · request</h3><pre>{d.request_body}</pre></>)}
      {d && d.response_body != null && (<><h3>body · response</h3><pre>{d.response_body}</pre></>)}
    </div>
  )
}

export default function App() {
  const [traces, setTraces] = useState([])
  const [q, setQ] = useState('')
  const [traceId, setTraceId] = useState(null)
  const [nodes, setNodes, onNodesChange] = useNodesState([])
  const [edges, setEdges, onEdgesChange] = useEdgesState([])
  const [sel, setSel] = useState(null)
  const [status, setStatus] = useState('')

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
        <header><b>trace-hub</b> <span className="muted">{status}</span></header>
        <input
          placeholder="搜索 service/kind/概要…回车"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') refreshList(q) }}
        />
        <div>
          {traces.map((t) => (
            <div
              key={t.trace_id}
              className={'trace-item' + (t.trace_id === traceId ? ' sel' : '')}
              onClick={() => openTrace(t.trace_id)}
            >
              <div className="t">{t.title ?? t.root_kind ?? t.trace_id}</div>
              <div className="m">
                {t.root_service ?? ''} · {t.span_count} spans · {t.trace_id.slice(0, 12)}…
              </div>
            </div>
          ))}
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
