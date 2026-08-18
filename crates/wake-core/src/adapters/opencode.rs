use super::parse_utils::*;
use super::sqlite_ro::{open_sqlite_ro, virtual_path, SqliteRo};
use super::{units_from_messages, AgentAdapter};
use crate::models::*;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// OpenCode:`~/.local/share/opencode/opencode.db`。
/// session 表自带 title/directory/tokens;正文在 part 表
/// ({type:text|reasoning|tool},synthetic=注入),message 表只有角色与时间。
/// parent_id 非空的是子代理会话,不进列表。
pub struct OpencodeAdapter {
    db: PathBuf,
    /// rows() 带按会话相关子查询(全 part 表求和),按 db mtime 缓存一轮
    /// 扫描内的重复调用
    rows_cache: Mutex<Option<(i64, Vec<OcRow>)>>,
}

const ROW_SELECT: &str = "SELECT s.id, s.directory, s.title, s.time_created, s.time_updated,
        s.model, s.tokens_input + s.tokens_output + s.tokens_reasoning,
        s.time_archived,
        (SELECT COALESCE(SUM(LENGTH(p.data)), 0) FROM part p WHERE p.session_id = s.id)
 FROM session s";

fn row_from(r: &rusqlite::Row) -> rusqlite::Result<OcRow> {
    Ok(OcRow {
        id: r.get(0)?,
        directory: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        title: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        created_ms: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
        updated_ms: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
        model_json: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        tokens: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
        archived: r.get::<_, Option<i64>>(7)?.is_some(),
        content_len: r.get(8)?,
    })
}

impl OpencodeAdapter {
    pub fn new() -> Self {
        Self {
            db: dirs::home_dir()
                .unwrap_or_default()
                .join(".local")
                .join("share")
                .join("opencode")
                .join("opencode.db"),
            rows_cache: Mutex::new(None),
        }
    }

    fn open(&self) -> Option<SqliteRo> {
        open_sqlite_ro(&self.db, "opencode")
    }

    fn rows(&self) -> Option<Vec<OcRow>> {
        let mtime = std::fs::metadata(&self.db).map(|m| mtime_ms(&m)).unwrap_or(0);
        {
            let cache = self.rows_cache.lock().unwrap();
            if let Some((t, rows)) = cache.as_ref() {
                if *t == mtime {
                    return Some(rows.clone());
                }
            }
        }
        let ro = self.open()?;
        let mut stmt = ro
            .conn
            .prepare(&format!("{ROW_SELECT} WHERE s.parent_id IS NULL"))
            .ok()?;
        let rows = stmt
            .query_map([], |r| row_from(r))
            .ok()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .ok()?;
        *self.rows_cache.lock().unwrap() = Some((mtime, rows.clone()));
        Some(rows)
    }

    fn build_meta(&self, r: &SessionFileRef, row: &OcRow, message_count: i64) -> SessionMeta {
        let title = clean_title_candidate(&row.title);
        let model = serde_json::from_str::<Value>(&row.model_json)
            .ok()
            .and_then(|m| m.get("id").and_then(|v| v.as_str()).map(String::from));
        SessionMeta {
            key: format!("opencode:{}", row.id),
            id: row.id.clone(),
            agent: AgentId::Opencode,
            title: if title.is_empty() { UNTITLED.to_string() } else { title },
            project_path: row.directory.clone(),
            project_name: project_name_of(&row.directory),
            file_path: r.file_path.clone(),
            created_at: if row.created_ms > 0 { row.created_ms } else { r.mtime_ms },
            updated_at: if row.updated_ms > 0 { row.updated_ms } else { r.mtime_ms },
            message_count,
            size_bytes: r.size,
            git_branch: None,
            model,
            tokens_used: if row.tokens > 0 { Some(row.tokens) } else { None },
            archived: row.archived,
            source: None,
            favorite: false,
            pinned: false,
        }
    }

    /// 单会话解析:一次连接,会话行与 message/part 都只查本会话
    fn parse(&self, r: &SessionFileRef) -> Result<(SessionMeta, Vec<TranscriptMessage>, u32)> {
        let ro = self.open().ok_or_else(|| anyhow!("cannot open opencode db"))?;
        let row = ro
            .conn
            .query_row(
                &format!("{ROW_SELECT} WHERE s.id = ?1"),
                [&r.native_id],
                |x| row_from(x),
            )
            .map_err(|_| anyhow!("opencode session {} not in db", r.native_id))?;

        // part 按 (message_id, id) 排序分组;id 前缀时间有序
        let mut parts_by_msg: HashMap<String, Vec<Value>> = HashMap::new();
        {
            let mut stmt = ro.conn.prepare(
                "SELECT message_id, data FROM part WHERE session_id = ?1 ORDER BY message_id, id",
            )?;
            let rows = stmt.query_map([&r.native_id], |p| {
                Ok((p.get::<_, String>(0)?, p.get::<_, String>(1)?))
            })?;
            for (mid, data) in rows.flatten() {
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    parts_by_msg.entry(mid).or_default().push(v);
                }
            }
        }

        let mut messages: Vec<TranscriptMessage> = Vec::new();
        let mut unknown = 0u32;
        let mut stmt = ro.conn.prepare(
            "SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created, id",
        )?;
        let msg_rows = stmt.query_map([&r.native_id], |m| {
            Ok((m.get::<_, String>(0)?, m.get::<_, String>(1)?))
        })?;
        for (mid, data) in msg_rows.flatten() {
            let Ok(md) = serde_json::from_str::<Value>(&data) else {
                unknown += 1;
                continue;
            };
            let role = match md.get("role").and_then(|v| v.as_str()) {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => Role::System,
            };
            let ts = md
                .get("time")
                .and_then(|t| t.get("created"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let mut text_parts: Vec<String> = Vec::new();
            let mut synthetic_parts: Vec<String> = Vec::new();
            let mut thinking_parts: Vec<String> = Vec::new();
            let mut tool_calls: Vec<ToolCallView> = Vec::new();
            for p in parts_by_msg.remove(&mid).unwrap_or_default() {
                match p.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        let t = p.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        if t.trim().is_empty() {
                            continue;
                        }
                        if p.get("synthetic").and_then(|v| v.as_bool()) == Some(true) {
                            synthetic_parts.push(t.trim().to_string());
                        } else {
                            text_parts.push(t.trim().to_string());
                        }
                    }
                    Some("reasoning") => {
                        if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                            if !t.trim().is_empty() {
                                thinking_parts.push(t.trim().to_string());
                            }
                        }
                    }
                    Some("tool") => {
                        let state = p.get("state").cloned().unwrap_or(Value::Null);
                        let input = state.get("input").cloned().unwrap_or(Value::Null);
                        let output = match state.get("output") {
                            Some(Value::String(s)) => Some(s.clone()),
                            Some(v) if !v.is_null() => serde_json::to_string(v).ok(),
                            _ => None,
                        };
                        tool_calls.push(tool_call_view(
                            p.get("callID").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            p.get("tool").and_then(|v| v.as_str()).unwrap_or("tool"),
                            &input,
                            output,
                            state.get("status").and_then(|v| v.as_str()) == Some("error"),
                        ));
                    }
                    Some("step-start") | Some("step-finish") | Some("snapshot")
                    | Some("patch") | Some("file") => {}
                    _ => unknown += 1,
                }
            }
            // 只有注入内容(editor_context 等)的消息归 Meta 折叠
            let (text, kind) = if text_parts.is_empty() && !synthetic_parts.is_empty() {
                (synthetic_parts.join("\n\n"), MessageKind::Meta)
            } else {
                (text_parts.join("\n\n"), MessageKind::Text)
            };
            if text.is_empty() && thinking_parts.is_empty() && tool_calls.is_empty() {
                continue;
            }
            let (clipped, truncated) = clip(&text, MAX_MSG_TEXT);
            messages.push(TranscriptMessage {
                seq: 0,
                role,
                kind,
                text: clipped,
                truncated,
                tool_calls,
                thinking: if thinking_parts.is_empty() {
                    None
                } else {
                    Some(clip(&thinking_parts.join("\n\n"), MAX_TOOL_IO).0)
                },
                timestamp: if ts > 0 { Some(ts) } else { None },
                model: None,
            });
        }
        assign_seq(&mut messages);
        let count = messages.iter().filter(|m| m.kind == MessageKind::Text).count() as i64;
        let meta = self.build_meta(r, &row, count);
        Ok((meta, messages, unknown))
    }
}

#[derive(Clone)]
struct OcRow {
    id: String,
    directory: String,
    title: String,
    created_ms: i64,
    updated_ms: i64,
    model_json: String,
    tokens: i64,
    archived: bool,
    content_len: i64,
}

impl AgentAdapter for OpencodeAdapter {
    fn agent(&self) -> AgentId {
        AgentId::Opencode
    }

    fn detect(&self) -> bool {
        self.db.is_file()
    }

    fn list_session_files(&self) -> Result<Vec<SessionFileRef>> {
        let Some(rows) = self.rows() else {
            return Ok(Vec::new());
        };
        Ok(rows
            .into_iter()
            .filter(|row| row.content_len > 0)
            .map(|row| SessionFileRef {
                agent: AgentId::Opencode,
                native_id: row.id.clone(),
                file_path: virtual_path(&self.db, &row.id),
                mtime_ms: row.updated_ms,
                size: row.content_len,
            })
            .collect())
    }

    fn quick_meta(&self, refs: &[SessionFileRef]) -> Option<HashMap<String, SessionMeta>> {
        let rows = self.rows()?;
        let by_id: HashMap<&str, &OcRow> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
        let mut out = HashMap::new();
        for r in refs {
            if let Some(row) = by_id.get(r.native_id.as_str()) {
                out.insert(r.file_path.clone(), self.build_meta(r, row, 0));
            }
        }
        Some(out)
    }

    fn parse_session(&self, r: &SessionFileRef) -> Result<ParsedSession> {
        let (meta, messages, unknown) = self.parse(r)?;
        let units = units_from_messages(&messages);
        Ok(ParsedSession {
            meta,
            units,
            unknown_line_count: unknown,
        })
    }

    fn parse_transcript(&self, r: &SessionFileRef) -> Result<ParsedTranscript> {
        let (meta, messages, unknown) = self.parse(r)?;
        Ok(ParsedTranscript {
            meta,
            mainline: messages,
            sidechains: Vec::new(),
            unknown_line_count: unknown,
        })
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}
