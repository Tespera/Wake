use crate::adapters::AgentAdapter;
use crate::db::Store;
use crate::models::*;
use crate::scanner::{scan_files, ScanEvents};
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

/// 监听 adapter 数据目录,800ms 去抖后做单文件增量。
/// 返回的 handle drop 时停止监听。
pub struct SessionWatcher {
    _watcher: notify::RecommendedWatcher,
    _thread: std::thread::JoinHandle<()>,
}

/// 事件路径 → 归属 agent:取**最长匹配根**,不是第一个命中。env 自定义根
/// (CODEX_HOME/XDG_DATA_HOME)可以落在别家数据树内,此时事件路径同时匹配
/// 两个根,按 roster 顺序取首个会把事件分给外层那家——file_ref 又对 .jsonl
/// 很宽松,会以错误的 agent 入库(错 key + 同 file_path 还会触发 UNIQUE 冲突)。
/// 更深的根必然是更具体的归属;Path::starts_with 按组件比较,同名前缀兄弟
/// 目录不会误匹配
pub fn resolve_watch_agent(roots: &[(PathBuf, AgentId)], path: &Path) -> Option<AgentId> {
    roots
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, agent)| *agent)
}

pub fn start_watcher(
    adapters: Arc<Vec<Box<dyn AgentAdapter>>>,
    store: Arc<Store>,
    events: Arc<dyn ScanEvents>,
) -> Option<SessionWatcher> {
    let mut roots: Vec<(PathBuf, AgentId)> = Vec::new();
    for a in adapters.iter() {
        for p in a.watch_paths() {
            roots.push((p, a.agent()));
        }
    }
    if roots.is_empty() {
        return None;
    }

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx).ok()?;
    for (root, _) in &roots {
        let _ = watcher.watch(root, RecursiveMode::Recursive);
    }

    let thread = std::thread::spawn(move || {
        let resolve_agent = |path: &Path| resolve_watch_agent(&roots, path);

        let mut pending: HashMap<PathBuf, AgentId> = HashMap::new();
        let mut removed: Vec<PathBuf> = Vec::new();
        loop {
            // 等首个事件(阻塞),然后 800ms 窗口收敛
            let first = match rx.recv() {
                Ok(e) => e,
                Err(_) => break, // watcher dropped
            };
            let mut batch = vec![first];
            let deadline = std::time::Instant::now() + Duration::from_millis(800);
            while let Ok(ev) = rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
                batch.push(ev);
                if std::time::Instant::now() >= deadline {
                    break;
                }
            }

            for ev in batch.into_iter().flatten() {
                for path in ev.paths {
                    if matches!(ev.kind, notify::EventKind::Remove(_)) {
                        pending.remove(&path);
                        removed.push(path.clone());
                    } else if let Some(agent) = resolve_agent(&path) {
                        pending.insert(path.clone(), agent);
                    }
                }
            }

            for path in removed.drain(..) {
                let _ = store.remove_by_path(&path.to_string_lossy());
                events.on_sessions_changed();
            }

            // 路径是否本 agent 的会话文件、native_id 怎么取,统一问 adapter
            let refs: Vec<SessionFileRef> = pending
                .drain()
                .filter_map(|(path, agent)| {
                    adapters
                        .iter()
                        .find(|a| a.agent() == agent)
                        .and_then(|a| a.file_ref(&path))
                })
                .collect();
            if !refs.is_empty() {
                scan_files(&adapters, &store, events.as_ref(), refs);
            }
        }
    });

    Some(SessionWatcher {
        _watcher: watcher,
        _thread: thread,
    })
}
