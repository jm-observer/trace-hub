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
