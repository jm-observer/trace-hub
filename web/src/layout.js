import dagre from '@dagrejs/dagre'

export const NODE_W = 230
export const NODE_H = 64

// 用 dagre 对 span 树做自上而下布局；仅用「父子树边」参与布局，
// 跨 trace 的 link 边不参与（避免把布局拉乱），只作虚线展示。
export function layoutGraph(rfNodes, treeEdges) {
  const g = new dagre.graphlib.Graph()
  g.setDefaultEdgeLabel(() => ({}))
  g.setGraph({ rankdir: 'TB', nodesep: 28, ranksep: 56, marginx: 16, marginy: 16 })
  rfNodes.forEach((n) => g.setNode(n.id, { width: NODE_W, height: NODE_H }))
  treeEdges.forEach((e) => g.setEdge(e.source, e.target))
  dagre.layout(g)
  return rfNodes.map((n) => {
    const p = g.node(n.id)
    return { ...n, position: { x: p.x - NODE_W / 2, y: p.y - NODE_H / 2 } }
  })
}
