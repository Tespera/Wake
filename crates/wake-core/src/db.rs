use crate::models::*;
use anyhow::{Context as _, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value TEXT);

CREATE TABLE IF NOT EXISTS sessions (
  key            TEXT PRIMARY KEY,
  agent_id       TEXT NOT NULL,
  native_id      TEXT NOT NULL,
  title          TEXT NOT NULL DEFAULT '',
  project_path   TEXT NOT NULL DEFAULT '',
  project_name   TEXT NOT NULL DEFAULT '',
  git_branch     TEXT,
  created_at     INTEGER DEFAULT 0,
  updated_at     INTEGER DEFAULT 0,
  message_count  INTEGER DEFAULT 0,
  tokens_used    INTEGER,
  model          TEXT,
  source         TEXT,
  archived       INTEGER DEFAULT 0,
  file_path      TEXT NOT NULL UNIQUE,
  file_size      INTEGER DEFAULT 0,
  file_mtime     INTEGER DEFAULT 0,
  unknown_lines  INTEGER DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_agent   ON sessions(agent_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_path, updated_at DESC);

CREATE TABLE IF NOT EXISTS messages (
  id           INTEGER PRIMARY KEY,
  session_key  TEXT NOT NULL,
  sidechain_id TEXT,
  seq          INTEGER NOT NULL,
  role         TEXT, ts INTEGER,
  text         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_key);

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  text,
  content='messages', content_rowid='id',
  tokenize="trigram case_sensitive 0"
);

CREATE TABLE IF NOT EXISTS user_data (
  session_key TEXT PRIMARY KEY,
  favorite    INTEGER DEFAULT 0,
  pinned      INTEGER DEFAULT 0,
  updated_at  INTEGER
);

CREATE TABLE IF NOT EXISTS tombstones (
  file_path  TEXT PRIMARY KEY,
  key        TEXT,
  deleted_at INTEGER
);

CREATE TABLE IF NOT EXISTS custom_roots (
  agent    TEXT NOT NULL,
  path     TEXT NOT NULL,
  added_at INTEGER,
  PRIMARY KEY (agent, path)
);

CREATE TABLE IF NOT EXISTS removed_defaults (
  agent      TEXT PRIMARY KEY,
  removed_at INTEGER
);
"#;

fn open_conn(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_millis(3000))?;
    conn.execute_batch(DDL)
        .context("failed to initialize SQLite schema")?;
    // tombstones.key 迁移(2026-08-24 加列,老库无此列;重复加列报错即忽略):
    // 墓碑按逻辑会话(key)+物理路径双轨屏蔽,多 location 副本不得复活已删会话
    let _ = conn.execute("ALTER TABLE tombstones ADD COLUMN key TEXT", []);
    Ok(conn)
}

/// 打开索引库;打不开就把它连同 WAL/SHM 一起挪到 `.corrupt` 旁路再建一个空的。
/// 索引本来就能从磁盘全量重扫恢复,重建的真实损失只有 user_data(收藏/置顶)
/// 与 custom_roots(自定义 location)——而它远好过 GUI 无提示秒退。
/// 返回的 `Some(_)` 是给用户看的说明文案。
pub fn open_or_rebuild(path: &Path) -> Result<(Store, Option<String>)> {
    let first = match Store::open(path) {
        Ok(store) => return Ok((store, None)),
        Err(e) => e,
    };
    // 三件套一起挪:留下 WAL 或 SHM 任何一个,新库都会接着读旧日志
    let backup = std::path::PathBuf::from(format!("{}.corrupt", path.display()));
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::rename(path, &backup);
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let store = Store::open(path)
        .with_context(|| format!("rebuild failed after: {first}"))?;
    Ok((
        store,
        Some(format!(
            "Index was damaged and has been rebuilt — stars, pins and custom \
             locations are gone. The old file is kept at {}",
            backup.display()
        )),
    ))
}

/// 读写分连接(WAL 单写多读);Connection 非 Sync,各自套 Mutex
pub struct Store {
    write: Mutex<Connection>,
    read: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            write: Mutex::new(open_conn(path)?),
            read: Mutex::new(open_conn(path)?),
        })
    }

    // ---------- 写路径(扫描器/用户操作) ----------

    pub fn write_session(&self, meta: &SessionMeta, file_mtime: i64, units: &[IndexUnit]) -> Result<()> {
        let mut conn = self.write.lock().unwrap();
        let tx = conn.transaction()?;
        write_session_tx(&tx, meta, file_mtime, units)?;
        tx.commit()?;
        Ok(())
    }

    /// 增量写入的并发安全版:胜者比较与写入**同一事务**——先查后写分开时,
    /// 败方副本的事件能与全量扫描交错、后发落库,让 file_path 违背 mtime 裁决
    /// (2026-08-24 Codex review)。返回 false = 本次是败方副本,一字未写
    pub fn write_session_guarded(
        &self,
        meta: &SessionMeta,
        file_mtime: i64,
        units: &[IndexUnit],
    ) -> Result<bool> {
        let mut conn = self.write.lock().unwrap();
        let tx = conn.transaction()?;
        let cur: Option<(String, i64)> = tx
            .query_row(
                "SELECT file_path, file_mtime FROM sessions WHERE key = ?1",
                params![meta.key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((cur_path, cur_mtime)) = cur {
            let loses = cur_path != meta.file_path
                && (cur_mtime > file_mtime
                    || (cur_mtime == file_mtime && cur_path.as_str() < meta.file_path.as_str()));
            if loses {
                return Ok(false); // 事务未提交即弃
            }
        }
        write_session_tx(&tx, meta, file_mtime, units)?;
        tx.commit()?;
        Ok(true)
    }

    pub fn write_meta_only(&self, metas: &[(SessionMeta, i64)]) -> Result<()> {
        let mut conn = self.write.lock().unwrap();
        let tx = conn.transaction()?;
        for (meta, mtime) in metas {
            upsert_session(&tx, meta, *mtime)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_session(&self, key: &str, tombstone: bool) -> Result<()> {
        let mut conn = self.write.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let file_path: Option<String> = tx
                .query_row("SELECT file_path FROM sessions WHERE key = ?1", params![key], |r| r.get(0))
                .optional()?;
            let mut sel = tx.prepare_cached("SELECT id, text FROM messages WHERE session_key = ?1")?;
            let rows: Vec<(i64, String)> = sel
                .query_map(params![key], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut fts_del =
                tx.prepare_cached("INSERT INTO messages_fts(messages_fts, rowid, text) VALUES ('delete', ?1, ?2)")?;
            for (id, text) in rows {
                fts_del.execute(params![id, text])?;
            }
            tx.execute("DELETE FROM messages WHERE session_key = ?1", params![key])?;
            tx.execute("DELETE FROM sessions WHERE key = ?1", params![key])?;
            if tombstone {
                if let Some(fp) = file_path {
                    tx.execute(
                        "INSERT OR REPLACE INTO tombstones(file_path, key, deleted_at) VALUES (?1, ?2, ?3)",
                        params![fp, key, now_ms()],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 路径 → 现行 key(watcher 增量的易主清理用)
    pub fn key_for_path(&self, file_path: &str) -> Result<Option<String>> {
        let conn = self.read.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT key FROM sessions WHERE file_path = ?1",
                params![file_path],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// 按路径删行,返回被删会话的 key——watcher 用它触发幸存副本上位
    ///(同 key 的另一 location 副本接管,Codex review P2)
    pub fn remove_by_path(&self, file_path: &str) -> Result<Option<String>> {
        let key: Option<String> = {
            let conn = self.read.lock().unwrap();
            conn.query_row(
                "SELECT key FROM sessions WHERE file_path = ?1",
                params![file_path],
                |r| r.get(0),
            )
            .optional()?
        };
        if let Some(k) = &key {
            self.remove_session(k, false)?;
        }
        Ok(key)
    }

    pub fn set_user_data(&self, key: &str, favorite: Option<bool>, pinned: Option<bool>) -> Result<()> {
        let conn = self.write.lock().unwrap();
        conn.execute(
            "INSERT INTO user_data(session_key, favorite, pinned, updated_at)
             VALUES (?1, COALESCE(?2, 0), COALESCE(?3, 0), ?4)
             ON CONFLICT(session_key) DO UPDATE SET
               favorite = COALESCE(?2, user_data.favorite),
               pinned   = COALESCE(?3, user_data.pinned),
               updated_at = excluded.updated_at",
            params![key, favorite.map(|v| v as i64), pinned.map(|v| v as i64), now_ms()],
        )?;
        Ok(())
    }

    // ---------- 自定义 location(Session locations 面板的 Add location) ----------

    /// 与收藏/置顶同层级的用户数据:索引重扫不动它,只有索引文件本体损坏
    /// 重建才丢(open_or_rebuild 的提示文案已列入)
    pub fn list_custom_roots(&self) -> Result<Vec<(String, String)>> {
        let conn = self.read.lock().unwrap();
        let mut stmt =
            conn.prepare_cached("SELECT agent, path FROM custom_roots ORDER BY added_at, path")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.flatten().collect())
    }

    pub fn add_custom_root(&self, agent: &str, path: &str) -> Result<()> {
        let conn = self.write.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO custom_roots(agent, path, added_at) VALUES (?1, ?2, ?3)",
            params![agent, path, now_ms()],
        )?;
        Ok(())
    }

    /// location 配置一次取齐(自定义根 + 被移除预设),解析成模型层类型;
    /// 未识别的 agent 名(库被降级版本写过)静默跳过。GUI 与 scan CLI 共用
    pub fn location_overrides(&self) -> (Vec<(AgentId, std::path::PathBuf)>, Vec<AgentId>) {
        let customs = self
            .list_custom_roots()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(a, p)| AgentId::from_str(&a).map(|a| (a, std::path::PathBuf::from(p))))
            .collect();
        let removed = self
            .list_removed_defaults()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| AgentId::from_str(&a))
            .collect();
        (customs, removed)
    }

    /// 编辑 location 的全形态原子写入(2026-08-24 Codex review:分开自动提交
    /// 时第二步失败会把配置改成半生效)。旧单元:自定义 = 删记录,预设 =
    /// 压默认;新单元一律记自定义——含换 agent 的编辑,全在一个事务里
    pub fn replace_location(
        &self,
        old_agent: &str,
        old_custom_path: Option<&str>,
        new_agent: &str,
        new_path: &str,
    ) -> Result<()> {
        let mut conn = self.write.lock().unwrap();
        let tx = conn.transaction()?;
        match old_custom_path {
            Some(p) => {
                tx.execute(
                    "DELETE FROM custom_roots WHERE agent = ?1 AND path = ?2",
                    params![old_agent, p],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT OR IGNORE INTO removed_defaults(agent, removed_at) VALUES (?1, ?2)",
                    params![old_agent, now_ms()],
                )?;
            }
        }
        tx.execute(
            "INSERT OR IGNORE INTO custom_roots(agent, path, added_at) VALUES (?1, ?2, ?3)",
            params![new_agent, new_path, now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 恢复初始:清空全部 location 偏离(自定义 + 被移除的预设)
    pub fn clear_location_overrides(&self) -> Result<()> {
        let conn = self.write.lock().unwrap();
        conn.execute_batch("DELETE FROM custom_roots; DELETE FROM removed_defaults;")?;
        Ok(())
    }

    /// 预设 location 的移除是"压制该家默认实例"而非删路径——默认根随
    /// env(CODEX_HOME 等)在构造时活解析,不能物化落库,故只记偏离
    pub fn list_removed_defaults(&self) -> Result<Vec<String>> {
        let conn = self.read.lock().unwrap();
        let mut stmt = conn.prepare_cached("SELECT agent FROM removed_defaults ORDER BY agent")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.flatten().collect())
    }

    pub fn add_removed_default(&self, agent: &str) -> Result<()> {
        let conn = self.write.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO removed_defaults(agent, removed_at) VALUES (?1, ?2)",
            params![agent, now_ms()],
        )?;
        Ok(())
    }

    pub fn remove_custom_root(&self, agent: &str, path: &str) -> Result<()> {
        let conn = self.write.lock().unwrap();
        conn.execute(
            "DELETE FROM custom_roots WHERE agent = ?1 AND path = ?2",
            params![agent, path],
        )?;
        Ok(())
    }

    pub fn rebuild_all(&self) -> Result<()> {
        let conn = self.write.lock().unwrap();
        conn.execute_batch(
            "DELETE FROM messages; DELETE FROM messages_fts; DELETE FROM sessions;",
        )?;
        Ok(())
    }

    // ---------- 读路径(UI 查询) ----------

    pub fn known_files(&self) -> Result<HashMap<String, (i64, i64, String)>> {
        let conn = self.read.lock().unwrap();
        let mut stmt = conn.prepare_cached("SELECT file_path, file_mtime, file_size, key FROM sessions")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (path, v) = row?;
            map.insert(path, v);
        }
        Ok(map)
    }

    /// 逻辑会话级墓碑:同 key 的任何副本(别的 location 里的拷贝)都不得
    /// 让已删会话复活(2026-08-24 Codex review P1,不变量 3 的多副本延伸)
    pub fn is_key_tombstoned(&self, key: &str) -> bool {
        let conn = self.read.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM tombstones WHERE key = ?1",
            params![key],
            |_| Ok(()),
        )
        .optional()
        .map(|o| o.is_some())
        .unwrap_or(false)
    }

    pub fn is_tombstoned(&self, file_path: &str) -> bool {
        let conn = self.read.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM tombstones WHERE file_path = ?1",
            params![file_path],
            |_| Ok(()),
        )
        .optional()
        .map(|o| o.is_some())
        .unwrap_or(false)
    }

    pub fn list_sessions(&self, f: &SessionFilter) -> Result<(Vec<SessionMeta>, i64)> {
        let mut wheres: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if !f.agents.is_empty() {
            let ph = f.agents.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            wheres.push(format!("s.agent_id IN ({ph})"));
            for a in &f.agents {
                args.push(Box::new(a.as_str().to_string()));
            }
        }
        if let Some(p) = &f.project_path {
            wheres.push("s.project_path = ?".into());
            args.push(Box::new(p.clone()));
        }
        if f.favorite_only {
            wheres.push("COALESCE(u.favorite, 0) = 1".into());
        }
        if !f.include_archived {
            wheres.push("s.archived = 0".into());
        }
        if let Some(q) = f.title_query.as_deref().filter(|q| !q.trim().is_empty()) {
            wheres.push("(s.title LIKE ? ESCAPE '\\' OR s.project_name LIKE ? ESCAPE '\\')".into());
            let like = format!("%{}%", escape_like(q.trim()));
            args.push(Box::new(like.clone()));
            args.push(Box::new(like));
        }
        let where_sql = if wheres.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", wheres.join(" AND "))
        };
        let order_col = match f.sort {
            SortKey::Updated => "s.updated_at",
            SortKey::Created => "s.created_at",
            SortKey::Messages => "s.message_count",
        };
        let order_dir = if f.ascending { "ASC" } else { "DESC" };

        let conn = self.read.lock().unwrap();
        let total: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM sessions s LEFT JOIN user_data u ON u.session_key = s.key {where_sql}"
            ),
            rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())),
            |r| r.get(0),
        )?;

        let sql = format!(
            "SELECT {SESSION_COLS} FROM sessions s LEFT JOIN user_data u ON u.session_key = s.key
             {where_sql}
             ORDER BY COALESCE(u.pinned,0) DESC, {order_col} {order_dir} LIMIT ? OFFSET ?"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let limit = if f.limit > 0 { f.limit } else { 500 };
        args.push(Box::new(limit));
        args.push(Box::new(f.offset));
        let rows = stmt.query_map(
            rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())),
            row_to_meta,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok((out, total))
    }

    pub fn get_session(&self, key: &str) -> Result<Option<SessionMeta>> {
        let conn = self.read.lock().unwrap();
        let sql = format!(
            "SELECT {SESSION_COLS} FROM sessions s LEFT JOIN user_data u ON u.session_key = s.key WHERE s.key = ?1"
        );
        Ok(conn.query_row(&sql, params![key], row_to_meta).optional()?)
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectInfo>> {
        let conn = self.read.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT project_path, project_name, COUNT(*), MAX(updated_at)
             FROM sessions WHERE archived = 0
             GROUP BY project_path ORDER BY MAX(updated_at) DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ProjectInfo {
                path: r.get(0)?,
                name: r.get(1)?,
                session_count: r.get(2)?,
                last_active: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn starred_count(&self) -> Result<i64> {
        let conn = self.read.lock().unwrap();
        Ok(conn.query_row(
            // archived 过滤与 agent_counts/list_projects 同口径,徽标数 = 点开后可见数
            "SELECT COUNT(*) FROM user_data u JOIN sessions s ON s.key = u.session_key WHERE u.favorite = 1 AND s.archived = 0",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn agent_counts(&self) -> Result<HashMap<String, i64>> {
        let conn = self.read.lock().unwrap();
        let mut stmt =
            conn.prepare_cached("SELECT agent_id, COUNT(*) FROM sessions WHERE archived = 0 GROUP BY agent_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut map = HashMap::new();
        for r in rows {
            let (k, v) = r?;
            map.insert(k, v);
        }
        Ok(map)
    }

    /// 各数据源目录下的会话数(Session locations 面板用):一次扫表按
    /// **(agent, 数据根)** 归属,免去每个目录一次往返。**不过滤 archived**
    /// ——归档目录本就该显示自己的量,那正是 agent_counts(WHERE archived = 0)
    /// 看不见的那部分。
    /// 必须连 agent 一起比,且边界走 adapters::path_owns:CODEX_HOME / XDG_DATA_HOME
    /// 允许把一家的数据根搬进另一家的树下,只认裸路径前缀会把整批会话静默
    /// 记到别家行上
    pub fn counts_by_path_prefix(&self, sources: &[(String, String)]) -> Result<Vec<i64>> {
        let conn = self.read.lock().unwrap();
        let mut stmt = conn.prepare_cached("SELECT agent_id, file_path FROM sessions")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut counts = vec![0i64; sources.len()];
        for row in rows {
            let (agent, path) = row?;
            if let Some(i) = sources
                .iter()
                .position(|(a, root)| *a == agent && crate::adapters::path_owns(root, &path))
            {
                counts[i] += 1;
            }
        }
        Ok(counts)
    }

    /// 全文搜索:trigram MATCH(每段 ≥3 码点)或 LIKE 降级。返回 (hits, degraded)
    pub fn search(
        &self,
        q: &str,
        agents: &[AgentId],
        project_path: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<SearchHit>, bool)> {
        let segs: Vec<&str> = q.split_whitespace().filter(|s| !s.is_empty()).collect();
        if segs.is_empty() {
            return Ok((Vec::new(), false));
        }
        let degraded = segs.iter().any(|s| s.chars().count() < 3);

        let mut filter_sql = String::new();
        let mut filter_args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if !agents.is_empty() {
            let ph = agents.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            filter_sql.push_str(&format!(" AND s.agent_id IN ({ph})"));
            for a in agents {
                filter_args.push(Box::new(a.as_str().to_string()));
            }
        }
        if let Some(p) = project_path {
            filter_sql.push_str(" AND s.project_path = ?");
            filter_args.push(Box::new(p.to_string()));
        }

        let conn = self.read.lock().unwrap();
        let mut raw: Vec<(String, i64, Option<String>, String, Option<i64>, String)> = Vec::new();

        if !degraded {
            let match_expr = segs
                .iter()
                .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" AND ");
            let sql = format!(
                "SELECT m.session_key, m.seq, m.sidechain_id, m.role, m.ts,
                        snippet(messages_fts, 0, ?, ?, '…', 16)
                 FROM messages_fts
                 JOIN messages m ON m.id = messages_fts.rowid
                 JOIN sessions s ON s.key = m.session_key
                 WHERE messages_fts MATCH ?{filter_sql}
                 ORDER BY bm25(messages_fts) LIMIT ?"
            );
            let mut stmt = conn.prepare_cached(&sql)?;
            let mut all_args: Vec<Box<dyn rusqlite::ToSql>> = vec![
                Box::new(HL_OPEN.to_string()),
                Box::new(HL_CLOSE.to_string()),
                Box::new(match_expr),
            ];
            all_args.extend(filter_args);
            all_args.push(Box::new(limit));
            let rows = stmt.query_map(
                rusqlite::params_from_iter(all_args.iter().map(|b| b.as_ref())),
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )?;
            for r in rows {
                raw.push(r?);
            }
        } else {
            let like_where = segs
                .iter()
                .map(|_| "m.text LIKE ? ESCAPE '\\'")
                .collect::<Vec<_>>()
                .join(" AND ");
            let sql = format!(
                "SELECT m.session_key, m.seq, m.sidechain_id, m.role, m.ts, m.text
                 FROM messages m JOIN sessions s ON s.key = m.session_key
                 WHERE {like_where}{filter_sql}
                 ORDER BY m.ts DESC LIMIT ?"
            );
            let mut stmt = conn.prepare_cached(&sql)?;
            let mut all_args: Vec<Box<dyn rusqlite::ToSql>> = segs
                .iter()
                .map(|s| Box::new(format!("%{}%", escape_like(s))) as Box<dyn rusqlite::ToSql>)
                .collect();
            all_args.extend(filter_args);
            all_args.push(Box::new(limit));
            let rows = stmt.query_map(
                rusqlite::params_from_iter(all_args.iter().map(|b| b.as_ref())),
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )?;
            for r in rows {
                let (k, seq, sc, role, ts, text): (String, i64, Option<String>, String, Option<i64>, String) = r?;
                raw.push((k, seq, sc, role, ts, make_like_snippet(&text, segs[0])));
            }
        }

        // 补齐 session meta
        let mut hits = Vec::new();
        let sql = format!(
            "SELECT {SESSION_COLS} FROM sessions s LEFT JOIN user_data u ON u.session_key = s.key WHERE s.key = ?1"
        );
        for (key, seq, sidechain_id, role, ts, snippet) in raw {
            if let Some(session) = conn.query_row(&sql, params![key], row_to_meta).optional()? {
                hits.push(SearchHit {
                    session,
                    seq,
                    sidechain_id,
                    role,
                    snippet,
                    timestamp: ts,
                });
            }
        }
        Ok((hits, degraded))
    }
}

const SESSION_COLS: &str = "s.key, s.agent_id, s.native_id, s.title, s.project_path, s.project_name,
    s.git_branch, s.created_at, s.updated_at, s.message_count, s.tokens_used, s.model, s.source,
    s.archived, s.file_path, s.file_size, COALESCE(u.favorite,0), COALESCE(u.pinned,0)";

fn row_to_meta(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionMeta> {
    let agent_str: String = r.get(1)?;
    Ok(SessionMeta {
        key: r.get(0)?,
        agent: AgentId::from_str(&agent_str).unwrap_or(AgentId::ClaudeCode),
        id: r.get(2)?,
        title: r.get(3)?,
        project_path: r.get(4)?,
        project_name: r.get(5)?,
        git_branch: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
        message_count: r.get(9)?,
        tokens_used: r.get(10)?,
        model: r.get(11)?,
        source: r.get(12)?,
        archived: r.get::<_, i64>(13)? == 1,
        file_path: r.get(14)?,
        size_bytes: r.get(15)?,
        favorite: r.get::<_, i64>(16)? == 1,
        pinned: r.get::<_, i64>(17)? == 1,
    })
}

/// write_session / write_session_guarded 共用的事务内核
fn write_session_tx(
    tx: &rusqlite::Transaction<'_>,
    meta: &SessionMeta,
    file_mtime: i64,
    units: &[IndexUnit],
) -> Result<()> {
    // FTS external content 需要显式 delete 旧行
    let mut sel = tx.prepare_cached("SELECT id, text FROM messages WHERE session_key = ?1")?;
    let rows: Vec<(i64, String)> = sel
        .query_map(params![meta.key], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(sel);
    let mut fts_del = tx.prepare_cached(
        "INSERT INTO messages_fts(messages_fts, rowid, text) VALUES ('delete', ?1, ?2)",
    )?;
    for (id, text) in rows {
        fts_del.execute(params![id, text])?;
    }
    drop(fts_del);
    tx.execute("DELETE FROM messages WHERE session_key = ?1", params![meta.key])?;

    upsert_session(tx, meta, file_mtime)?;

    let mut ins_msg = tx.prepare_cached(
        "INSERT INTO messages(session_key, sidechain_id, seq, role, ts, text) VALUES (?1,?2,?3,?4,?5,?6)",
    )?;
    let mut ins_fts = tx.prepare_cached("INSERT INTO messages_fts(rowid, text) VALUES (?1, ?2)")?;
    for u in units {
        ins_msg.execute(params![
            meta.key,
            u.sidechain_id,
            u.seq,
            u.role.as_str(),
            u.timestamp,
            u.text
        ])?;
        let rowid = tx.last_insert_rowid();
        ins_fts.execute(params![rowid, u.text])?;
    }
    Ok(())
}

fn upsert_session(tx: &rusqlite::Transaction<'_>, m: &SessionMeta, file_mtime: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO sessions(key, agent_id, native_id, title, project_path, project_name,
           git_branch, created_at, updated_at, message_count, tokens_used, model, source,
           archived, file_path, file_size, file_mtime, unknown_lines)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,0)
         ON CONFLICT(key) DO UPDATE SET
           title=excluded.title, project_path=excluded.project_path,
           project_name=excluded.project_name, git_branch=excluded.git_branch,
           created_at=excluded.created_at, updated_at=excluded.updated_at,
           message_count=excluded.message_count, tokens_used=excluded.tokens_used,
           model=excluded.model, source=excluded.source, archived=excluded.archived,
           file_path=excluded.file_path, file_size=excluded.file_size,
           file_mtime=excluded.file_mtime",
        params![
            m.key,
            m.agent.as_str(),
            m.id,
            m.title,
            m.project_path,
            m.project_name,
            m.git_branch,
            m.created_at,
            m.updated_at,
            m.message_count,
            m.tokens_used,
            m.model,
            m.source,
            m.archived as i64,
            m.file_path,
            m.size_bytes,
            file_mtime,
        ],
    )?;
    Ok(())
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn make_like_snippet(text: &str, first_seg: &str) -> String {
    let lower = text.to_lowercase();
    let seg_lower = first_seg.to_lowercase();
    let Some(byte_idx) = lower.find(&seg_lower) else {
        return text.chars().take(120).collect();
    };
    // 定位到字符边界安全的窗口
    let chars: Vec<char> = text.chars().collect();
    let char_idx = text[..byte_idx].chars().count();
    let seg_len = first_seg.chars().count();
    let start = char_idx.saturating_sub(40);
    let end = (char_idx + seg_len + 80).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..char_idx]);
    out.push(HL_OPEN);
    out.extend(&chars[char_idx..(char_idx + seg_len).min(chars.len())]);
    out.push(HL_CLOSE);
    out.extend(&chars[(char_idx + seg_len).min(chars.len())..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 索引库路径:macOS 为 ~/Library/Application Support/wake,Linux 为
/// ~/.local/share/wake,Windows 为 %LOCALAPPDATA%\wake
/// (从旧 vibex 路径一次性迁移,保留收藏等 user_data)。
///
/// Windows 取 data_local_dir 而非 data_dir:后者是漫游 %APPDATA%,而本库
/// 开 WAL——WAL 要 -shm 共享内存映射,重定向到网络盘的漫游目录上根本打不开
/// (域环境 Folder Redirection 是标配),Wake 会在启动即致命退出;何况这是
/// 可随时重建的索引,几百 MB 跟着登录/注销来回同步纯属浪费(2026-08-25 review)
pub fn default_db_path() -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    let data = dirs::data_local_dir();
    #[cfg(not(target_os = "windows"))]
    let data = dirs::data_dir();
    let data = data.unwrap_or_else(|| std::path::PathBuf::from("."));
    let dir = data.join("wake");
    let db = dir.join("wake.db");
    if !db.exists() {
        let old_db = data.join("vibex").join("vibex-rs.db");
        if old_db.exists() {
            let _ = std::fs::create_dir_all(&dir);
            for suffix in ["", "-wal", "-shm"] {
                let src = data.join("vibex").join(format!("vibex-rs.db{suffix}"));
                if src.exists() {
                    let _ = std::fs::copy(&src, dir.join(format!("wake.db{suffix}")));
                }
            }
        }
    }
    db
}


