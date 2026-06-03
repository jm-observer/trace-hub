//! SQLite 存储：信封+概要一张表（轻，建树/检索），detail+body 分表（重，懒加载），
//! span_link 单独表。rusqlite 同步，handler 侧用 `spawn_blocking` 包裹。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use rusqlite::{params, Connection};
use trace_model::{SpanLink, SpanRecord};

use crate::views::{DetailView, NodeView, TraceSummary};

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS span (
    span_id        TEXT PRIMARY KEY,
    trace_id       TEXT NOT NULL,
    parent_span_id TEXT,
    service        TEXT NOT NULL,
    kind           TEXT NOT NULL,
    flow_name      TEXT,
    start_ms       INTEGER NOT NULL,
    end_ms         INTEGER NOT NULL,
    status         TEXT NOT NULL,
    summary        TEXT NOT NULL,
    has_detail     INTEGER NOT NULL DEFAULT 0,
    body_truncated INTEGER NOT NULL DEFAULT 0,
    created_at     INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER))
);
CREATE INDEX IF NOT EXISTS idx_span_trace ON span(trace_id, start_ms);
CREATE INDEX IF NOT EXISTS idx_span_start ON span(start_ms);
CREATE TABLE IF NOT EXISTS span_detail (
    span_id       TEXT PRIMARY KEY,
    detail        TEXT,
    request_body  TEXT,
    response_body TEXT
);
CREATE TABLE IF NOT EXISTS span_link (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    span_id         TEXT NOT NULL,
    linked_trace_id TEXT NOT NULL,
    linked_span_id  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_span_link_span ON span_link(span_id);
";

#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
    body_limit: usize,
}

impl Storage {
    pub fn open(path: &Path, body_limit: usize) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建数据库目录失败: {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("打开数据库失败: {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA).context("建表失败")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            body_limit,
        })
    }

    /// 批量落库（一个事务）。同一 span_id 重复推送以最后一次为准（幂等覆盖）。
    pub fn insert_spans(&self, spans: Vec<SpanRecord>) -> anyhow::Result<usize> {
        let mut guard = self.conn.lock().expect("storage mutex poisoned");
        let tx = guard.transaction()?;
        for s in &spans {
            let (req_body, req_trunc) = truncate(&s.request_body, self.body_limit);
            let (resp_body, resp_trunc) = truncate(&s.response_body, self.body_limit);
            let has_detail = !s.detail.is_null() || req_body.is_some() || resp_body.is_some();
            let body_truncated = s.body_truncated || req_trunc || resp_trunc;

            tx.execute(
                "INSERT OR REPLACE INTO span \
                 (span_id, trace_id, parent_span_id, service, kind, flow_name, \
                  start_ms, end_ms, status, summary, has_detail, body_truncated) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    s.span_id,
                    s.trace_id,
                    s.parent_span_id,
                    s.service,
                    s.kind,
                    s.flow_name,
                    s.start_ms,
                    s.end_ms,
                    serde_json::to_string(&s.status)?,
                    serde_json::to_string(&s.summary)?,
                    has_detail as i64,
                    body_truncated as i64,
                ],
            )?;

            if has_detail {
                tx.execute(
                    "INSERT OR REPLACE INTO span_detail \
                     (span_id, detail, request_body, response_body) VALUES (?1,?2,?3,?4)",
                    params![
                        s.span_id,
                        serde_json::to_string(&s.detail)?,
                        req_body,
                        resp_body,
                    ],
                )?;
            }

            // 链接：先清后插，保证幂等覆盖。
            tx.execute(
                "DELETE FROM span_link WHERE span_id = ?1",
                params![s.span_id],
            )?;
            for l in &s.links {
                tx.execute(
                    "INSERT INTO span_link (span_id, linked_trace_id, linked_span_id) \
                     VALUES (?1,?2,?3)",
                    params![s.span_id, l.trace_id, l.span_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(spans.len())
    }

    /// 一条 trace 的全部节点（信封+概要，不含 body），按 start_ms 升序。
    pub fn trace_nodes(&self, trace_id: &str) -> anyhow::Result<Vec<NodeView>> {
        let guard = self.conn.lock().expect("storage mutex poisoned");
        let mut links_by_span = self.load_links(&guard, trace_id)?;

        let mut stmt = guard.prepare(
            "SELECT span_id, trace_id, parent_span_id, service, kind, flow_name, \
                    start_ms, end_ms, status, summary, has_detail, body_truncated \
             FROM span WHERE trace_id = ?1 ORDER BY start_ms",
        )?;
        let rows = stmt.query_map(params![trace_id], |r| {
            let status: String = r.get(8)?;
            let summary: String = r.get(9)?;
            Ok(NodeViewRaw {
                span_id: r.get(0)?,
                trace_id: r.get(1)?,
                parent_span_id: r.get(2)?,
                service: r.get(3)?,
                kind: r.get(4)?,
                flow_name: r.get(5)?,
                start_ms: r.get(6)?,
                end_ms: r.get(7)?,
                status,
                summary,
                has_detail: r.get::<_, i64>(10)? != 0,
                body_truncated: r.get::<_, i64>(11)? != 0,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            let row = row?;
            out.push(NodeView {
                links: links_by_span.remove(&row.span_id).unwrap_or_default(),
                status: parse_json(&row.status),
                summary: parse_json(&row.summary),
                trace_id: row.trace_id,
                span_id: row.span_id,
                parent_span_id: row.parent_span_id,
                service: row.service,
                kind: row.kind,
                flow_name: row.flow_name,
                start_ms: row.start_ms,
                end_ms: row.end_ms,
                has_detail: row.has_detail,
                body_truncated: row.body_truncated,
            });
        }
        Ok(out)
    }

    /// 单节点详情 + body（点击时拉）。
    pub fn span_detail(&self, span_id: &str) -> anyhow::Result<Option<DetailView>> {
        let guard = self.conn.lock().expect("storage mutex poisoned");
        let mut stmt = guard.prepare(
            "SELECT detail, request_body, response_body FROM span_detail WHERE span_id = ?1",
        )?;
        let mut rows = stmt.query(params![span_id])?;
        match rows.next()? {
            Some(r) => {
                let detail: Option<String> = r.get(0)?;
                Ok(Some(DetailView {
                    detail: detail
                        .as_deref()
                        .map(parse_json)
                        .unwrap_or(serde_json::Value::Null),
                    request_body: r.get(1)?,
                    response_body: r.get(2)?,
                }))
            }
            None => Ok(None),
        }
    }

    /// trace 列表（按 trace_id 聚合）。`q` 非空时按 service/kind/summary 子串过滤。
    pub fn list_traces(&self, q: Option<&str>, limit: i64) -> anyhow::Result<Vec<TraceSummary>> {
        let guard = self.conn.lock().expect("storage mutex poisoned");
        let like = q.map(|s| format!("%{s}%"));

        let base = "SELECT trace_id, MIN(start_ms), MAX(end_ms), COUNT(*) FROM span";
        let grouped = if like.is_some() {
            format!(
                "{base} WHERE trace_id IN \
                 (SELECT trace_id FROM span WHERE service LIKE ?1 OR kind LIKE ?1 OR summary LIKE ?1) \
                 GROUP BY trace_id ORDER BY MIN(start_ms) DESC LIMIT ?2"
            )
        } else {
            format!("{base} GROUP BY trace_id ORDER BY MIN(start_ms) DESC LIMIT ?1")
        };

        let mut stmt = guard.prepare(&grouped)?;
        let map_row = |r: &rusqlite::Row| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        };
        let rows: Vec<(String, i64, i64, i64)> = match &like {
            Some(l) => stmt
                .query_map(params![l, limit], map_row)?
                .collect::<rusqlite::Result<_>>()?,
            None => stmt
                .query_map(params![limit], map_row)?
                .collect::<rusqlite::Result<_>>()?,
        };

        let mut out = Vec::with_capacity(rows.len());
        for (trace_id, start_ms, end_ms, span_count) in rows {
            let root = self.root_of(&guard, &trace_id)?;
            out.push(TraceSummary {
                root_service: root.as_ref().map(|r| r.0.clone()),
                root_kind: root.as_ref().map(|r| r.1.clone()),
                title: root.and_then(|r| r.2),
                trace_id,
                start_ms,
                end_ms,
                span_count,
            });
        }
        Ok(out)
    }

    fn root_of(
        &self,
        conn: &Connection,
        trace_id: &str,
    ) -> anyhow::Result<Option<(String, String, Option<String>)>> {
        let mut stmt = conn.prepare(
            "SELECT service, kind, flow_name FROM span \
             WHERE trace_id = ?1 AND parent_span_id IS NULL ORDER BY start_ms LIMIT 1",
        )?;
        let mut rows = stmt.query(params![trace_id])?;
        match rows.next()? {
            Some(r) => Ok(Some((r.get(0)?, r.get(1)?, r.get(2)?))),
            None => Ok(None),
        }
    }

    fn load_links(
        &self,
        conn: &Connection,
        trace_id: &str,
    ) -> anyhow::Result<HashMap<String, Vec<SpanLink>>> {
        let mut stmt = conn.prepare(
            "SELECT span_id, linked_trace_id, linked_span_id FROM span_link \
             WHERE span_id IN (SELECT span_id FROM span WHERE trace_id = ?1)",
        )?;
        let rows = stmt.query_map(params![trace_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                SpanLink {
                    trace_id: r.get(1)?,
                    span_id: r.get(2)?,
                },
            ))
        })?;
        let mut map: HashMap<String, Vec<SpanLink>> = HashMap::new();
        for row in rows {
            let (span_id, link) = row?;
            map.entry(span_id).or_default().push(link);
        }
        Ok(map)
    }
}

struct NodeViewRaw {
    span_id: String,
    trace_id: String,
    parent_span_id: Option<String>,
    service: String,
    kind: String,
    flow_name: Option<String>,
    start_ms: i64,
    end_ms: i64,
    status: String,
    summary: String,
    has_detail: bool,
    body_truncated: bool,
}

/// 按字节上限截断，保证不切断 UTF-8 边界。返回 (截断后, 是否截断)。
fn truncate(body: &Option<String>, limit: usize) -> (Option<String>, bool) {
    match body {
        Some(b) if b.len() > limit => {
            let mut end = limit;
            while end > 0 && !b.is_char_boundary(end) {
                end -= 1;
            }
            (Some(b[..end].to_string()), true)
        }
        other => (other.clone(), false),
    }
}

/// 存库的 JSON 文本解析回 Value；解析失败兜底为字符串，绝不 panic。
fn parse_json(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use trace_model::{SpanRecord, SpanStatus};

    fn mem() -> Storage {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        Storage {
            conn: Arc::new(Mutex::new(conn)),
            body_limit: 16,
        }
    }

    fn rec(trace: &str, span: &str, parent: Option<&str>, kind: &str) -> SpanRecord {
        SpanRecord {
            trace_id: trace.into(),
            span_id: span.into(),
            parent_span_id: parent.map(Into::into),
            service: "zero".into(),
            kind: kind.into(),
            flow_name: None,
            start_ms: 100,
            end_ms: 200,
            status: SpanStatus::Ok,
            summary: serde_json::json!({"k": "v"}),
            detail: serde_json::Value::Null,
            request_body: None,
            response_body: None,
            body_truncated: false,
            links: vec![],
        }
    }

    #[test]
    fn insert_and_read_tree() {
        let st = mem();
        let root = rec("tr1", "s1", None, "user_message");
        let mut child = rec("tr1", "s2", Some("s1"), "llm_call");
        child.detail = serde_json::json!({"tokens": 9});
        child.request_body = Some("hello".into());
        child.response_body = Some("world".into());
        st.insert_spans(vec![root, child]).unwrap();

        let nodes = st.trace_nodes("tr1").unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].span_id, "s1");
        assert_eq!(nodes[0].parent_span_id, None);
        assert_eq!(nodes[1].parent_span_id.as_deref(), Some("s1"));
        assert!(nodes[1].has_detail);
        assert_eq!(nodes[0].summary["k"], "v");

        let detail = st.span_detail("s2").unwrap().unwrap();
        assert_eq!(detail.request_body.as_deref(), Some("hello"));
        assert_eq!(detail.detail["tokens"], 9);
        assert!(
            st.span_detail("s1").unwrap().is_none(),
            "无 detail 的节点返回 None"
        );
    }

    #[test]
    fn body_truncation() {
        let st = mem(); // body_limit = 16
        let mut r = rec("tr2", "sx", None, "llm_call");
        r.request_body = Some("x".repeat(50));
        st.insert_spans(vec![r]).unwrap();
        let nodes = st.trace_nodes("tr2").unwrap();
        assert!(nodes[0].body_truncated);
        let d = st.span_detail("sx").unwrap().unwrap();
        assert_eq!(d.request_body.unwrap().len(), 16);
    }

    #[test]
    fn list_and_search() {
        let st = mem();
        st.insert_spans(vec![rec("trA", "a", None, "user_message")])
            .unwrap();
        st.insert_spans(vec![rec("trB", "b", None, "alarm_submit")])
            .unwrap();
        assert_eq!(st.list_traces(None, 10).unwrap().len(), 2);
        let hit = st.list_traces(Some("alarm"), 10).unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].trace_id, "trB");
        assert_eq!(hit[0].root_kind.as_deref(), Some("alarm_submit"));
    }

    #[test]
    fn idempotent_overwrite() {
        let st = mem();
        st.insert_spans(vec![rec("tr", "s", None, "k1")]).unwrap();
        let mut again = rec("tr", "s", None, "k2");
        again.flow_name = Some("f".into());
        st.insert_spans(vec![again]).unwrap();
        let nodes = st.trace_nodes("tr").unwrap();
        assert_eq!(nodes.len(), 1, "同 span_id 覆盖而非新增");
        assert_eq!(nodes[0].kind, "k2");
    }
}
