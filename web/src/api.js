// 同源调用 trace-hub query API（UI 由后端在 `/` 提供）。
async function api(path) {
  const r = await fetch(path)
  if (!r.ok) throw new Error(`${path} → ${r.status}`)
  return r.json()
}

export const listTraces = (q) =>
  api('/v1/traces' + (q ? '?q=' + encodeURIComponent(q) : ''))
export const getTrace = (id) => api('/v1/traces/' + encodeURIComponent(id))
export const getSpan = (id) => api('/v1/spans/' + encodeURIComponent(id))

// 清空全部记录。返回 { deleted: N }。
export async function clearAll() {
  const r = await fetch('/v1/clear', { method: 'DELETE' })
  if (!r.ok) throw new Error(`/v1/clear → ${r.status}`)
  return r.json()
}
