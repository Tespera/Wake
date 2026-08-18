//! 扫描终态契约(CLAUDE.md 不变量 6):run_scan 无论正常结束、Err 提前返回
//! 还是 adapter 出错,都必须发出一次 scanning=false 的终态进度事件——
//! UI 的模态刷新弹窗只认这个事件收场,收不到就永久锁死。

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use wake_core::adapters::AgentAdapter;
use wake_core::db::Store;
use wake_core::models::*;
use wake_core::scanner::{run_scan, ScanEvents, ScanProgress};

/// 收集全部进度事件,供断言终态
struct Recorder(Mutex<Vec<ScanProgress>>);

impl ScanEvents for Recorder {
    fn on_progress(&self, p: &ScanProgress) {
        self.0.lock().unwrap().push(p.clone());
    }
    fn on_sessions_changed(&self) {}
}

/// 枚举文件即失败的 adapter,模拟数据源不可读
struct FailingAdapter;

impl AgentAdapter for FailingAdapter {
    fn agent(&self) -> AgentId {
        AgentId::ClaudeCode
    }
    fn detect(&self) -> bool {
        true
    }
    fn list_session_files(&self) -> Result<Vec<SessionFileRef>> {
        bail!("simulated data source failure")
    }
    fn parse_session(&self, _: &SessionFileRef) -> Result<ParsedSession> {
        bail!("unreachable")
    }
    fn parse_transcript(&self, _: &SessionFileRef) -> Result<ParsedTranscript> {
        bail!("unreachable")
    }
    fn watch_paths(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }
}

fn assert_terminal_event(events: &[ScanProgress], ctx: &str) {
    let last = events.last().unwrap_or_else(|| panic!("{ctx}: 无任何进度事件"));
    assert!(
        !last.scanning,
        "{ctx}: 最后一个事件 scanning 仍为 true,UI 刷新弹窗将永久锁死"
    );
}

fn temp_store(dir: &Path) -> Arc<Store> {
    Arc::new(Store::open(&dir.join("scan.db")).expect("open store"))
}

#[test]
fn finale_fires_on_empty_scan() {
    let dir = tempfile::tempdir().unwrap();
    let store = temp_store(dir.path());
    let adapters: Vec<Box<dyn AgentAdapter>> = Vec::new();
    let rec = Recorder(Mutex::new(Vec::new()));

    let _ = run_scan(&adapters, &store, &rec, false);
    assert_terminal_event(&rec.0.lock().unwrap(), "空 adapter 列表");
}

#[test]
fn finale_fires_when_adapter_fails() {
    let dir = tempfile::tempdir().unwrap();
    let store = temp_store(dir.path());
    let adapters: Vec<Box<dyn AgentAdapter>> = vec![Box::new(FailingAdapter)];
    let rec = Recorder(Mutex::new(Vec::new()));

    // list_session_files 报错的路径:无论 run_scan 返回 Ok/Err,终态事件必须送达
    let _ = run_scan(&adapters, &store, &rec, false);
    assert_terminal_event(&rec.0.lock().unwrap(), "adapter 枚举失败");
}

/// 固定提供一个会话(含 quickMeta 快路径)的 adapter,
/// 模拟 Codex state DB 中删除后仍残留的行
struct SeedAdapter {
    r: SessionFileRef,
    meta: SessionMeta,
}

impl AgentAdapter for SeedAdapter {
    fn agent(&self) -> AgentId {
        AgentId::Codex
    }
    fn detect(&self) -> bool {
        true
    }
    fn list_session_files(&self) -> Result<Vec<SessionFileRef>> {
        Ok(vec![self.r.clone()])
    }
    fn quick_meta(
        &self,
        _refs: &[SessionFileRef],
    ) -> Option<std::collections::HashMap<String, SessionMeta>> {
        let mut m = std::collections::HashMap::new();
        m.insert(self.r.file_path.clone(), self.meta.clone());
        Some(m)
    }
    fn parse_session(&self, _: &SessionFileRef) -> Result<ParsedSession> {
        Ok(ParsedSession {
            meta: self.meta.clone(),
            units: Vec::new(),
            unknown_line_count: 0,
        })
    }
    fn parse_transcript(&self, _: &SessionFileRef) -> Result<ParsedTranscript> {
        bail!("transcript not needed in scan")
    }
    fn watch_paths(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }
}

/// 不变量 3 端到端:删除(trash+tombstone)后,数据源仍枚举同一文件的
/// 下一次全量扫描不得让会话复活
#[test]
fn tombstoned_session_does_not_resurrect_on_rescan() {
    let dir = tempfile::tempdir().unwrap();
    let store = temp_store(dir.path());
    let meta = SessionMeta {
        key: "codex:ghost".into(),
        id: "ghost".into(),
        agent: AgentId::Codex,
        title: "残留会话".into(),
        project_path: "/tmp/p".into(),
        project_name: "p".into(),
        file_path: "/tmp/fixtures/ghost.jsonl".into(),
        created_at: 1,
        updated_at: 2,
        message_count: 0,
        size_bytes: 1,
        git_branch: None,
        model: None,
        tokens_used: None,
        archived: false,
        source: None,
        favorite: false,
        pinned: false,
    };
    let r = SessionFileRef {
        agent: AgentId::Codex,
        native_id: "ghost".into(),
        file_path: meta.file_path.clone(),
        mtime_ms: 2,
        size: 1,
    };
    let adapters: Vec<Box<dyn AgentAdapter>> =
        vec![Box::new(SeedAdapter { r, meta: meta.clone() })];
    let rec = Recorder(Mutex::new(Vec::new()));

    run_scan(&adapters, &store, &rec, true).unwrap();
    assert!(store.get_session("codex:ghost").unwrap().is_some(), "首扫应写入");

    store.remove_session("codex:ghost", true).unwrap();
    run_scan(&adapters, &store, &rec, true).unwrap();
    assert!(
        store.get_session("codex:ghost").unwrap().is_none(),
        "tombstoned 会话重扫后复活 = 不变量 3 破坏"
    );
}
