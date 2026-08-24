//! Store 写入/搜索/删除语义的往返测试(临时库,不碰真实索引)。
//! 覆盖 CLAUDE.md 不变量 3:tombstone 防复活、user_data 独立表重建不丢。

use wake_core::db::Store;
use wake_core::models::*;

fn temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("test.db")).expect("open store");
    (dir, store)
}

fn meta(key: &str, title: &str) -> SessionMeta {
    SessionMeta {
        key: key.to_string(),
        id: key.split(':').nth(1).unwrap_or(key).to_string(),
        agent: AgentId::ClaudeCode,
        title: title.to_string(),
        project_path: "/tmp/proj".into(),
        project_name: "proj".into(),
        file_path: format!("/tmp/fixtures/{key}.jsonl"),
        created_at: 1_700_000_000_000,
        updated_at: 1_700_000_100_000,
        message_count: 2,
        size_bytes: 128,
        git_branch: None,
        model: None,
        tokens_used: None,
        archived: false,
        source: None,
        favorite: false,
        pinned: false,
    }
}

fn unit(seq: i64, role: Role, text: &str) -> IndexUnit {
    IndexUnit {
        seq,
        sidechain_id: None,
        role,
        timestamp: Some(1_700_000_000_000 + seq),
        text: text.to_string(),
    }
}

#[test]
fn search_roundtrip_hits_correct_seq() {
    let (_dir, store) = temp_store();
    let m = meta("claude-code:s1", "测试会话");
    let units = vec![
        unit(0, Role::User, "请帮我实现二维码扫描"),
        unit(3, Role::Assistant, "好的,用 useEffect( 挂载扫描器"),
    ];
    store.write_session(&m, m.updated_at, &units).unwrap();

    // 中文 trigram
    let (hits, degraded) = store.search("二维码", &[], None, 10).unwrap();
    assert!(!degraded, "3 码点应走 FTS 不降级");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].session.key, "claude-code:s1");
    assert_eq!(hits[0].seq, 0, "命中 seq 必须等于写入时的消息 seq");

    // 代码子串
    let (hits, _) = store.search("useEffect(", &[], None, 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].seq, 3);

    // <3 码点降级 LIKE
    let (hits, degraded) = store.search("好的", &[], None, 10).unwrap();
    assert!(degraded, "2 码点应降级");
    assert_eq!(hits.len(), 1);
}

#[test]
fn tombstone_primitives() {
    let (_dir, store) = temp_store();
    let m = meta("codex:s2", "会被删除的会话");
    store.write_session(&m, m.updated_at, &[]).unwrap();
    assert!(store.get_session("codex:s2").unwrap().is_some());

    // remove_session(tombstone=true) 后按 file_path 记墓碑。
    // 注意分层:write_meta_only 是纯写入原语、不查墓碑——防复活由
    // scanner 两条路径先过 is_tombstoned 保证(端到端见 scanner_finale.rs)
    store.remove_session("codex:s2", true).unwrap();
    assert!(store.get_session("codex:s2").unwrap().is_none());
    assert!(store.is_tombstoned(&m.file_path));
    assert!(!store.is_tombstoned("/tmp/other.jsonl"));
}

#[test]
fn user_data_survives_rebuild() {
    let (_dir, store) = temp_store();
    let m = meta("claude-code:s3", "收藏的会话");
    store.write_session(&m, m.updated_at, &[]).unwrap();
    store.set_user_data("claude-code:s3", Some(true), Some(true)).unwrap();

    // 重建索引(sessions/messages 清空重来)后,收藏/置顶必须还在
    store.rebuild_all().unwrap();
    store.write_session(&m, m.updated_at, &[]).unwrap();
    let got = store.get_session("claude-code:s3").unwrap().unwrap();
    assert!(got.favorite, "重建后收藏丢失 = user_data 未独立");
    assert!(got.pinned, "重建后置顶丢失 = user_data 未独立");
}

#[test]
fn list_sessions_filters_and_counts() {
    let (_dir, store) = temp_store();
    let mut a = meta("claude-code:s4", "A 会话");
    let mut b = meta("codex:s5", "B 会话");
    b.agent = AgentId::Codex;
    a.updated_at = 2_000;
    b.updated_at = 1_000;
    store.write_session(&a, a.updated_at, &[]).unwrap();
    store.write_session(&b, b.updated_at, &[]).unwrap();

    let all = SessionFilter {
        agents: vec![],
        project_path: None,
        favorite_only: false,
        include_archived: false,
        title_query: None,
        sort: SortKey::Updated,
        ascending: false,
        limit: 10,
        offset: 0,
    };
    let (sessions, total) = store.list_sessions(&all).unwrap();
    assert_eq!(total, 2);
    assert_eq!(sessions[0].key, "claude-code:s4", "默认按 updated 降序");

    let only_codex = SessionFilter { agents: vec![AgentId::Codex], ..all };
    let (sessions, total) = store.list_sessions(&only_codex).unwrap();
    assert_eq!(total, 1);
    assert_eq!(sessions[0].key, "codex:s5");
}

#[test]
fn path_counts_respect_agent_and_boundary() {
    // Session locations 面板的计数按数据根归属。两条真实风险:
    // ① 自定义 CODEX_HOME/XDG_DATA_HOME 可以落在别家根之下,只比路径前缀
    //    不看 agent,会把这家的会话整批记到别家行上;
    // ② 裸 starts_with 没有边界,`…/sessions` 会连 `…/sessions-old` 一起吞。
    let (_d, store) = temp_store();
    let mut claude = meta("claude-code:a", "claude one");
    claude.file_path = "/home/u/.claude/projects/a.jsonl".into();
    // codex 的根被搬进了 claude 的树下(CODEX_HOME 允许这么设)
    let mut codex = meta("codex:b", "codex one");
    codex.agent = AgentId::Codex;
    codex.file_path = "/home/u/.claude/projects/codex/sessions/b.jsonl".into();
    // 同名前缀的兄弟目录:不该算进 `…/sessions`
    let mut sibling = meta("codex:c", "codex sibling");
    sibling.agent = AgentId::Codex;
    sibling.file_path = "/home/u/.claude/projects/codex/sessions-old/c.jsonl".into();
    store
        .write_meta_only(&[(claude, 0), (codex, 0), (sibling, 0)])
        .expect("seed sessions");

    let counts = store
        .counts_by_path_prefix(&[
            ("claude-code".into(), "/home/u/.claude/projects".into()),
            ("codex".into(), "/home/u/.claude/projects/codex/sessions".into()),
        ])
        .expect("counts");
    assert_eq!(counts[0], 1, "codex 的会话不该被记到 claude 行");
    assert_eq!(counts[1], 1, "sessions-old 不该算进 sessions");
}

/// 自定义 location 的持久化:与收藏/置顶同层级的用户数据,重复添加幂等
#[test]
fn custom_roots_roundtrip() {
    let (_dir, store) = temp_store();
    store.add_custom_root("codex", "/tmp/a").unwrap();
    store.add_custom_root("codex", "/tmp/a").unwrap(); // 幂等
    store.add_custom_root("claude-code", "/tmp/b").unwrap();

    let mut roots = store.list_custom_roots().unwrap();
    roots.sort();
    assert_eq!(
        roots,
        vec![
            ("claude-code".to_string(), "/tmp/b".to_string()),
            ("codex".to_string(), "/tmp/a".to_string()),
        ]
    );

    store.remove_custom_root("codex", "/tmp/a").unwrap();
    assert_eq!(
        store.list_custom_roots().unwrap(),
        vec![("claude-code".to_string(), "/tmp/b".to_string())]
    );

    // 预设移除是按 agent 压制,幂等
    store.add_removed_default("codex").unwrap();
    store.add_removed_default("codex").unwrap();
    assert_eq!(store.list_removed_defaults().unwrap(), vec!["codex".to_string()]);

    // 编辑的原子替换:自定义换路径 / 预设改自定义 / 换 agent,全走单事务
    store.add_custom_root("grok", "/tmp/g1").unwrap();
    store.replace_location("grok", Some("/tmp/g1"), "grok", "/tmp/g2").unwrap();
    assert_eq!(
        store.list_custom_roots().unwrap(),
        vec![
            ("claude-code".to_string(), "/tmp/b".to_string()),
            ("grok".to_string(), "/tmp/g2".to_string()),
        ]
    );
    store.replace_location("kiro", None, "kiro", "/tmp/k").unwrap();
    assert!(store.list_removed_defaults().unwrap().contains(&"kiro".to_string()));
    store.replace_location("grok", Some("/tmp/g2"), "cursor", "/tmp/cur").unwrap();
    let roots = store.list_custom_roots().unwrap();
    assert!(roots.iter().any(|(a, p)| a == "cursor" && p == "/tmp/cur"));
    assert!(!roots.iter().any(|(a, _)| a == "grok"));

    // Restore defaults 语义:自定义与预设移除一把双清
    store.add_custom_root("grok", "/tmp/c").unwrap();
    store.clear_location_overrides().unwrap();
    assert!(store.list_custom_roots().unwrap().is_empty());
    assert!(store.list_removed_defaults().unwrap().is_empty());
}

/// 增量写入的胜者裁决在写事务内:败方副本(旧 mtime、异路径)一字不写,
/// 反超后按规则接管(2026-08-24 Codex review)
#[test]
fn guarded_write_respects_winner() {
    let (_dir, store) = temp_store();
    let mut winner = meta("codex:g", "胜者");
    winner.file_path = "/live/g.jsonl".into();
    store.write_session(&winner, 9, &[]).unwrap();

    let mut loser = meta("codex:g", "败方");
    loser.file_path = "/backup/g.jsonl".into();
    assert!(!store.write_session_guarded(&loser, 5, &[]).unwrap(), "败方不该写入");
    assert_eq!(store.get_session("codex:g").unwrap().unwrap().file_path, "/live/g.jsonl");

    assert!(store.write_session_guarded(&loser, 12, &[]).unwrap(), "反超应接管");
    assert_eq!(store.get_session("codex:g").unwrap().unwrap().file_path, "/backup/g.jsonl");
}
