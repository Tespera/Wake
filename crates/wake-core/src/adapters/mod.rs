pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod dsh;
pub mod gemini;
pub mod grok;
pub mod kimi;
pub mod kiro;
pub mod opencode;
pub mod pi;

pub(crate) mod parse_utils;
mod sqlite_ro;

use crate::models::*;
use anyhow::Result;
use std::path::Path;

/// agent 数据源适配器。列表扫描与详情解析共用同一核心解析器,
/// 保证 FTS 的 seq 与详情页消息序号一致(搜索跳转依赖)。
pub trait AgentAdapter: Send + Sync {
    fn agent(&self) -> AgentId;
    /// 本机是否有这家的数据。由 data_roots 派生,**不要覆写**——它必须与
    /// 面板逐路径的 exists() 同一判据,手写版本(is_dir/is_file)与之打架
    /// 正是 2026-08-24 数轮 review 反复修的源头之一
    fn detect(&self) -> bool {
        self.data_roots().iter().any(|p| p.exists())
    }
    /// 枚举全部会话文件。契约是"枚举必须廉价、绝不做全量解析":多数家纯 stat,
    /// SQLite 型跑元数据查询,dsh 读有界首行(子代理标志只存在于文件头)。
    /// 故障就地降级为空列表,不外溢炸掉整轮扫描。
    fn list_session_files(&self) -> Result<Vec<SessionFileRef>>;
    /// watcher 事件路径 → 本 adapter 的会话文件引用;None = 非会话文件
    /// (边车、子代理转录等)。默认:非空 .jsonl,stem 即 native_id。
    /// 各家的路径布局知识收敛在此,watcher 不再硬编码任何 agent 特例。
    fn file_ref(&self, path: &Path) -> Option<SessionFileRef> {
        parse_utils::default_file_ref(self.agent(), path)
    }
    /// 快路径:不解析文件直接给出 meta(Codex 走 state DB)。None = 无快路径
    fn quick_meta(&self, _refs: &[SessionFileRef]) -> Option<std::collections::HashMap<String, SessionMeta>> {
        None
    }
    /// quick 与 parsed 的合并策略:默认 parsed 为准、quick 补缺。
    /// Codex 覆写(state DB 的 title 是用户手动命名,优先级更高)。
    fn merge_quick_meta(&self, mut parsed: SessionMeta, quick: &SessionMeta) -> SessionMeta {
        if parsed.source.is_none() {
            parsed.source = quick.source.clone();
        }
        if parsed.model.is_none() {
            parsed.model = quick.model.clone();
        }
        if parsed.tokens_used.is_none() {
            parsed.tokens_used = quick.tokens_used;
        }
        parsed
    }
    /// 全解析:meta + FTS 单元
    fn parse_session(&self, r: &SessionFileRef) -> Result<ParsedSession>;
    /// 详情解析
    fn parse_transcript(&self, r: &SessionFileRef) -> Result<ParsedTranscript>;
    /// 加载 sidechain 消息(仅 Claude/Cursor subagents)
    fn load_sidechain(&self, _r: &SessionFileRef, _sidechain_id: &str) -> Result<Vec<TranscriptMessage>> {
        Ok(Vec::new())
    }
    /// 会话在磁盘上的全部归属路径(删除时一并 trash)。默认仅主文件;
    /// 有边车/目录布局的 adapter 覆写。
    fn session_paths(&self, meta: &SessionMeta) -> Vec<String> {
        vec![meta.file_path.clone()]
    }
    /// 本家会话文件所在的根位置(目录,或 SQLite 型的库文件),**不论当前存不存在**。
    /// 这是路径的**唯一事实源**:watch_paths 由它派生,"Scanned locations" 面板
    /// 直接展示它,按路径前缀统计会话数也依赖它——故语义定死为"其子树(或其本身)
    /// 拥有本家 session 文件的位置",凭据/配置/索引这类不产生会话的文件不列。
    /// 新增 adapter 必须实现:没有默认值,漏了编译就过不去
    fn data_roots(&self) -> Vec<std::path::PathBuf>;
    /// 文件监听根目录。默认 = data_roots 中现存的目录,十三家实测全部吻合:
    /// 目录型给出自己的 root,SQLite 型的根是库文件、天然筛空(watcher 只认
    /// .jsonl,库变更靠启动/手动刷新),codex 的 sessions + archived 一并覆盖。
    /// 只有当监听范围确实不同于数据根时才覆写——否则一次根路径搬迁
    /// (如 CODEX_HOME / XDG_DATA_HOME)就要在两处各改一遍,漏一处则静默失去实时更新
    fn watch_paths(&self) -> Vec<std::path::PathBuf> {
        self.data_roots().into_iter().filter(|p| p.is_dir()).collect()
    }
}

/// 环境变量指定的数据根。**只能读进程环境**:从 Dock 启动的 GUI 不继承用户
/// shell 的 env,kooky 处理 CODEX_HOME 时同样只能做到这一步("the best
/// available")。所以调用方拿它当**候选**而非唯一真相,默认路径仍要探。
/// 空值视作未设。
/// **调用方必须把返回值当候选**:探到真实数据(目录型看会话子目录、SQLite 型
/// 看库文件)才采信,否则回落默认位置——变量指向一个存在但空的目录时,不该让
/// 整家会话凭空消失
pub(crate) fn env_dir(key: &str) -> Option<std::path::PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
}

/// 全量十三家 roster,**不按 detect 过滤**。这是全应用唯一的构造点:
/// scanner/watcher/resume/Session locations 面板共享 Workbench 启动时的
/// 同一份实例。缺根的家由各自 list_session_files 降级为 Ok(空)(scanner
/// 对 Err 会 `?` 截断整轮,新 adapter 必须维持这条降级约定,contract 测试
/// 有卡)。**不要为任何用途二次构造 roster**:根路径是构造时刻对 env
/// (CODEX_HOME/XDG_DATA_HOME)与文件系统的快照,两份实例可能解析出不同的
/// 根,UI 就会展示一个扫描器并不在读的路径
pub fn create_adapters() -> Vec<Box<dyn AgentAdapter>> {
    vec![
        Box::new(claude::ClaudeAdapter::new()),
        Box::new(codex::CodexAdapter::new()),
        Box::new(copilot::CopilotAdapter::new()),
        Box::new(cursor::CursorAdapter::new()),
        Box::new(opencode::OpencodeAdapter::new()),
        Box::new(kiro::KiroAdapter::new()),
        Box::new(gemini::GeminiAdapter::new()),
        Box::new(pi::PiAdapter::new()),
        Box::new(pi::PiAdapter::omp()),
        Box::new(grok::GrokAdapter::new()),
        Box::new(kimi::KimiAdapter::new()),
        Box::new(antigravity::AntigravityAdapter::new()),
        Box::new(dsh::DshAdapter::new()),
    ]
}

/// 从解析后的消息派生 FTS 单元(text + tool 名称/输入摘要)
pub(crate) fn units_from_messages(messages: &[TranscriptMessage]) -> Vec<IndexUnit> {
    messages
        .iter()
        .filter(|m| m.kind == MessageKind::Text)
        .filter_map(|m| {
            let mut parts = vec![m.text.clone()];
            for tc in &m.tool_calls {
                parts.push(format!("{} {}", tc.name, tc.input_preview));
            }
            let text = parse_utils::clip(&parts.join("\n"), MAX_MSG_TEXT).0;
            if text.trim().is_empty() {
                None
            } else {
                Some(IndexUnit {
                    seq: m.seq,
                    sidechain_id: None,
                    role: m.role,
                    timestamp: m.timestamp,
                    text,
                })
            }
        })
        .collect()
}
