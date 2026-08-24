//! 会话恢复 / 系统集成服务。策略层(恢复命令拼装、CLI 解析、打开/选中/
//! 删除的平台无关流程)在本文件;macos.rs / linux.rs / windows.rs 只提供
//! 原语(起终端、开目录、选中文件、进废纸篓、弹对话框、写剪贴板),各端
//! 导出同形接口(TerminalApp 变体集合各异,UI 只遍历不点名)。
//!
//! POSIX 双端(macOS/Linux)共享本文件的 login shell 探测与 posix_quote
//! 拼装;Windows 的三个前提全不成立(无 login shell、引号不是 POSIX 规则、
//! 命令方言按终端宿主分 cmd/PowerShell 两派),这三件事经 probe_clis /
//! compose_command / launch_in 三个接缝交回 windows.rs,策略流程本身不分叉。

use crate::models::{AgentId, SessionMeta};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as platform;

// 公共层默认走 POSIX 假设,新平台进来必须给出自己的模块并重审 probe_clis /
// compose_command / launch_in 三个接缝(windows.rs 是完整先例),不能静默沿用
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
compile_error!("wake terminal services: unsupported platform — add a platform module (POSIX assumptions throughout)");

pub use platform::{ensure_app_icons, installed_terminals, terminals_for, TerminalApp};

#[derive(Debug, Clone)]
pub struct ResumeOutcome {
    pub ok: bool,
    pub command: String,
    pub error: Option<String>,
}

/// GUI 进程 PATH 不全(macOS/Linux 缺 ~/.local/bin 等),批量解析并缓存
static CLI_CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// 解析 PATH 用的 login shell:macOS 固定 zsh(系统默认);Linux 尊重 $SHELL
/// (bash/zsh/fish 对 `-lic` 与 `command -v` 语义一致),缺省 /bin/bash
#[cfg(not(target_os = "windows"))]
fn login_shell() -> String {
    if cfg!(target_os = "macos") {
        return "/bin/zsh".to_string();
    }
    std::env::var("SHELL")
        .ok()
        .filter(|s| s.starts_with('/'))
        .unwrap_or_else(|| "/bin/bash".to_string())
}

/// 批量探测缺失 bin 的绝对路径(POSIX:login shell 里 `command -v`,把
/// 用户 rc 文件加进 PATH 的目录一并覆盖)。Windows 的 GUI 进程 PATH 来自
/// 注册表、天然完整,探测在 windows.rs 用 PATH×PATHEXT 纯 Rust 遍历。
#[cfg(not(target_os = "windows"))]
fn probe_clis(missing: &[&str]) -> HashMap<String, String> {
    let script = missing
        .iter()
        .map(|b| format!("printf '%s\\t' {b}; command -v {b} || echo"))
        .collect::<Vec<_>>()
        .join("; ");
    let out = Command::new(login_shell()).args(["-lic", &script]).output();
    let stdout = out
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let mut found = HashMap::new();
    for line in stdout.lines() {
        if let Some((name, path)) = line.split_once('\t') {
            if path.starts_with('/') {
                found.insert(name.trim().to_string(), path.trim().to_string());
            }
        }
    }
    found
}

#[cfg(target_os = "windows")]
use windows::probe_clis;

fn resolve_clis(bins: &[&str]) -> HashMap<String, Option<String>> {
    let mut cache = CLI_CACHE.lock().unwrap();
    let map = cache.get_or_insert_with(HashMap::new);
    let missing: Vec<&str> = bins.iter().filter(|b| !map.contains_key(**b)).copied().collect();
    if !missing.is_empty() {
        let found = probe_clis(&missing);
        for b in missing {
            map.insert(b.to_string(), found.get(b).cloned());
        }
    }
    bins.iter()
        .map(|b| (b.to_string(), map.get(*b).cloned().flatten()))
        .collect()
}

/// 单 bin 解析(命中缓存则不起 shell)
fn resolve_cli(bin: &str) -> Option<String> {
    resolve_clis(&[bin]).get(bin).cloned().flatten()
}

pub fn cli_path(agent: AgentId) -> Option<String> {
    agent_bin(agent).and_then(resolve_cli)
}

/// 会话级二进制:OpenCode v2 beta 与 v1 并存、装为 `opencode2`,
/// v2 会话(source = "opencode2")必须由它恢复,其余会话走 agent 默认 bin
fn session_bin(meta: &SessionMeta) -> Option<&'static str> {
    if meta.agent == AgentId::Opencode && meta.source.as_deref() == Some("opencode2") {
        Some("opencode2")
    } else {
        agent_bin(meta.agent)
    }
}

pub fn agent_bin(agent: AgentId) -> Option<&'static str> {
    match agent {
        AgentId::ClaudeCode => Some("claude"),
        AgentId::Codex => Some("codex"),
        AgentId::Copilot => Some("copilot"),
        AgentId::Cursor => Some("cursor-agent"),
        AgentId::Opencode => Some("opencode"),
        AgentId::Kiro => Some("kiro"),
        AgentId::Gemini => Some("gemini"),
        AgentId::Pi => Some("pi"),
        AgentId::Omp => Some("omp"),
        AgentId::Grok => Some("grok"),
        AgentId::Kimi => Some("kimi"),
        AgentId::Antigravity => Some("agy"),
        // dsh 官方唯一分发形态是 npx(README 只有 `npx @deepseek-ai/dsh web`,
        // 不发全局命令);包名由 resume_args 作首参带上
        AgentId::Dsh => Some("npx"),
    }
}

fn resume_args(agent: AgentId, id: &str) -> Option<(Vec<String>, bool)> {
    match agent {
        AgentId::ClaudeCode => Some((vec!["--resume".into(), id.into()], true)),
        AgentId::Codex => Some((vec!["resume".into(), id.into()], false)),
        AgentId::Copilot => Some((vec![format!("--resume={id}")], false)),
        AgentId::Cursor => Some((vec!["--resume".into(), id.into()], false)),
        // 参数形制与 kooky 的 resume 集成一致(空格/等号是各家 CLI 实测约束)
        // OpenCode 两代 CLI 同为 --session;v2 会话由 session_bin 换 opencode2
        AgentId::Opencode => Some((vec!["--session".into(), id.into()], false)),
        AgentId::Pi => Some((vec!["--session".into(), id.into()], false)),
        AgentId::Omp => Some((vec!["--resume".into(), id.into()], false)),
        AgentId::Grok => Some((vec!["--resume".into(), id.into()], false)),
        AgentId::Kimi => Some((vec!["--session".into(), id.into()], false)),
        AgentId::Antigravity => Some((vec![format!("--conversation={id}")], false)),
        // dsh 官方 tui bundle 未发布(rc.8 shipped profile 只有 web/headless,
        // help 里的 --profile tui --resume 当下无消费端),web 是唯一交互
        // surface 且无 per-session 深链——resume 退而求其次:cd 到会话 cwd
        // 按官方原样 `npx @deepseek-ai/dsh web` 拉起(workspace 由启动目录
        // 决定),会话在 UI 里即点即续。官方发布 tui bundle 后切回定点 resume
        AgentId::Dsh => Some((vec!["@deepseek-ai/dsh".into(), "web".into()], true)),
        _ => None,
    }
}

/// POSIX 单引号 quote
#[cfg(not(target_os = "windows"))]
pub fn posix_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:=".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// 展示/剪贴板/启动共用的一条可执行命令。POSIX 双端就是实际启动串;
/// Windows 版(windows.rs)是 PowerShell 方言的"手动可粘"形态,实际启动
/// 由 launch_in 按终端宿主重拼。
#[cfg(not(target_os = "windows"))]
fn compose_command(cli: &str, args: &[String], cwd: Option<&str>) -> String {
    let core = std::iter::once(cli)
        .chain(args.iter().map(|s| s.as_str()))
        .map(posix_quote)
        .collect::<Vec<_>>()
        .join(" ");
    match cwd {
        Some(dir) => format!("cd {} && {core}", posix_quote(dir)),
        None => core,
    }
}

#[cfg(target_os = "windows")]
use windows::compose_command;

/// 起终端跑恢复命令。POSIX 双端直接投喂拼好的 command;Windows 忽略
/// command、拿结构化件重拼(cmd 宿主 cmd 方言、其余 PowerShell 方言)——
/// 一条字符串塞不进两种引号规则,接缝只能开在这里。
#[cfg(not(target_os = "windows"))]
fn launch_in(
    term: TerminalApp,
    _cli: &str,
    _args: &[String],
    _cwd: Option<&str>,
    command: &str,
) -> anyhow::Result<()> {
    platform::launch_shell(term, command)
}

#[cfg(target_os = "windows")]
fn launch_in(
    term: TerminalApp,
    cli: &str,
    args: &[String],
    cwd: Option<&str>,
    _command: &str,
) -> anyhow::Result<()> {
    windows::launch_shell(term, cli, args, cwd)
}

/// 保守 percent-encode(RFC 3986 unreserved 之外全编;keep_slash 供
/// file:// URL 保路径分隔)。POSIX 两端共用,编码集只此一份。
#[cfg(not(target_os = "windows"))]
pub(crate) fn percent_encode(s: &str, keep_slash: bool) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

pub fn resume_session_in(meta: &SessionMeta, term: TerminalApp) -> ResumeOutcome {
    // 深链类目标(macOS Kooky)由平台整锅接管,不走 shell 命令构建;
    // 新增非 shell 目标在平台的 deep_link_resume 里声明,这里无需加旁路
    if let Some(outcome) = platform::deep_link_resume(meta, term) {
        return outcome;
    }
    let Some((args, requires_cwd)) = resume_args(meta.agent, &meta.id) else {
        return ResumeOutcome {
            ok: false,
            command: String::new(),
            error: Some(format!("Resume isn't supported for {} yet", meta.agent.display_name())),
        };
    };
    let bin = session_bin(meta);
    let Some(cli) = bin.and_then(resolve_cli) else {
        return ResumeOutcome {
            ok: false,
            command: String::new(),
            error: Some(format!(
                "Command {} not found — is it installed?",
                bin.unwrap_or("?")
            )),
        };
    };
    let cwd_ok = !meta.project_path.is_empty() && Path::new(&meta.project_path).is_dir();
    let cwd = cwd_ok.then(|| meta.project_path.as_str());
    let command = compose_command(&cli, &args, cwd);
    if requires_cwd && !cwd_ok {
        let hint = clipboard_fallback(&command);
        return ResumeOutcome {
            ok: false,
            command,
            error: Some(format!("Project directory no longer exists: {}. {hint}", meta.project_path)),
        };
    }

    let result = launch_in(term, &cli, &args, cwd, &command);
    match result {
        Ok(()) => ResumeOutcome {
            ok: true,
            command,
            error: None,
        },
        Err(e) => {
            let hint = clipboard_fallback(&command);
            ResumeOutcome {
                ok: false,
                command,
                error: Some(format!("Couldn't open terminal ({e}). {hint}")),
            }
        }
    }
}

/// 失败兜底通知的后半句:剪贴板写成了才说 copied(Linux 可能三个剪贴板
/// 工具都不在),没写成把命令本体给出来——error 是失败通知唯一展示面
/// (workbench 只渲染 error 文案),命令不能只活在 ResumeOutcome.command 里
fn clipboard_fallback(command: &str) -> String {
    if platform::copy_to_clipboard(command) {
        "Command copied to clipboard — paste to run.".to_string()
    } else {
        format!("Run manually: {command}")
    }
}

/// 删除会话文件到系统回收站(可恢复)。虚拟路径 `<db>#<id>` 与已消失的
/// 文件在此过滤(不变量 3:SQLite 型只 tombstone),平台原语只收真实路径。
/// 部分失败语义:平台实现可能删到一半报错(macOS 逐文件、Linux/Windows
/// 批量),调用方按"整批可疑"处理即可——已进回收站的文件可恢复,无害。
pub fn trash_paths(paths: &[String]) -> anyhow::Result<()> {
    let existing: Vec<&str> = paths
        .iter()
        .map(|s| s.as_str())
        .filter(|p| Path::new(p).exists())
        .collect();
    if existing.is_empty() {
        return Ok(());
    }
    platform::trash_existing(&existing)
}

/// 致命错误提示。GPUI 窗口还没起来时这是唯一能让用户看见的通道,
/// 否则应用就是无提示秒退;stderr 始终先落一份。
pub fn show_fatal_alert(message: &str) {
    eprintln!("[wake] fatal: {message}");
    platform::alert_dialog(message);
}

/// 在文件管理器里打开这个位置:目录直接进入,文件则退回在父目录中选中它
/// ——SQLite 型的数据源是库文件,直接交给 opener 会把它丢给默认应用打开
pub fn open_in_file_manager(path: &str) {
    if Path::new(path).is_dir() {
        platform::open_dir(path);
    } else {
        reveal_in_file_manager(path);
    }
}

/// 选中文件。调用点都是 UI 线程的 on_click,而平台原语可能长阻塞
/// (Linux 的 D-Bus ShowItems 会等文件管理器 activation 冷启)——统一
/// 甩给短命线程,UI 零等待。
pub fn reveal_in_file_manager(path: &str) {
    let real = crate::adapters::sqlite_ro::strip_virtual_path(path).to_string();
    std::thread::spawn(move || platform::reveal_path(&real));
}

/// 把 text 经管道写给 `bin args…` 的 stdin(POSIX 剪贴板工具用),按退出码
/// 报成败——"copied to clipboard" 的用户提示以此为据,不能 spawn 成功就算数。
/// 三家 Linux 工具与 pbcopy 都在拿到内容后自行 fork 常驻,wait 即刻返回。
/// Windows 不走子进程管道(clip.exe 按控制台 codepage 解码,非 ASCII 必乱),
/// windows.rs 直接调 Win32 剪贴板。
#[cfg(not(target_os = "windows"))]
pub(crate) fn pipe_to(bin: &str, args: &[&str], text: &str) -> bool {
    use std::io::Write;
    let Ok(mut child) = Command::new(bin).args(args).stdin(std::process::Stdio::piped()).spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// 起子进程并**收尸**,spawn 失败上抛。`status()` 不能用:把调用线程阻塞
/// 到进程结束(文件管理器冷启上百毫秒);裸 `spawn()` 丢掉 Child 也不行:
/// Unix 上 Child 的 Drop 不 wait,实测点几次就攒几个 `<defunct>`,直到
/// Wake 退出才回收。故 spawn 后交给一个短命线程 wait。Windows 无僵尸进程
/// 概念,但 wait 同样及时归还进程句柄,三端共用。
fn spawn_and_reap(mut cmd: Command) -> std::io::Result<()> {
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
