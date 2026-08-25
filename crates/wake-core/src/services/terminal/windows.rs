//! Windows 平台原语:终端宿主(Windows Terminal / PowerShell / cmd /
//! Alacritty / WezTerm)、回收站(trash crate → IFileOperation)、资源管理器
//! 进入/选中、Win32 剪贴板与 MessageBox。接口与 macos.rs / linux.rs 同形,
//! 策略在 mod.rs;mod.rs 的三个 POSIX 前提(login shell、posix_quote、单一
//! shell 方言)在本端由 probe_clis / compose_command / launch_shell 接管。
//!
//! 方言分两派:cmd 宿主用 cmd 方言内联注入(raw_arg 直达,无 argv 引号层);
//! 其余宿主一律装 PowerShell 会话——脚本全单引号、不含双引号,经宿主的
//! argv 重引号往返无损(cmd 方言的内层双引号过不了 wt 这类会重拼命令行的
//! 宿主)。展示/剪贴板形态同为 PowerShell 方言:现代 Windows 的默认 shell,
//! pwsh / Windows PowerShell / wt 默认 profile 粘贴均可跑。

use super::{resolve_cli, resolve_clis, spawn_and_reap, ResumeOutcome};
use crate::models::{AgentId, SessionMeta};
use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows_sys::Win32::System::Threading::{CREATE_NEW_CONSOLE, CREATE_NO_WINDOW};

/// Open In 下拉的目标终端(Windows 家族)。wt/pwsh/powershell/cmd 覆盖
/// 系统自带面(cmd 与 Windows PowerShell 必装,探测恒真),第三方只列
/// 装了的。Ghostty/kitty 无 Windows 发行版,不列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalApp {
    /// Windows Terminal(wt)——Win11 默认终端宿主
    WindowsTerminal,
    /// PowerShell 7+(pwsh),与内置 Windows PowerShell 是两个产品
    Pwsh,
    WindowsPowershell,
    Cmd,
    Alacritty,
    WezTerm,
}

impl TerminalApp {
    /// 声明序即偏好序(installed_terminals 保序,UI 回退取首个)
    const ALL: [TerminalApp; 6] = [
        TerminalApp::WindowsTerminal,
        TerminalApp::Pwsh,
        TerminalApp::WindowsPowershell,
        TerminalApp::Cmd,
        TerminalApp::Alacritty,
        TerminalApp::WezTerm,
    ];

    /// 命名对齐 wt 自己的 profile 名("PowerShell" = 7+,"Windows PowerShell"
    /// = 内置 5.1),用户在 wt 里见到的就是这两个词
    pub fn display_name(&self) -> &'static str {
        match self {
            TerminalApp::WindowsTerminal => "Windows Terminal",
            TerminalApp::Pwsh => "PowerShell",
            TerminalApp::WindowsPowershell => "Windows PowerShell",
            TerminalApp::Cmd => "Command Prompt",
            TerminalApp::Alacritty => "Alacritty",
            TerminalApp::WezTerm => "WezTerm",
        }
    }

    /// 稳定短 id(图标缓存文件名、last-used 记忆用)。恰好也全部是可执行名,
    /// 探测与启动直接复用(PATHEXT 扩展名由 probe_clis 补)
    pub fn id(&self) -> &'static str {
        match self {
            TerminalApp::WindowsTerminal => "wt",
            TerminalApp::Pwsh => "pwsh",
            TerminalApp::WindowsPowershell => "powershell",
            TerminalApp::Cmd => "cmd",
            TerminalApp::Alacritty => "alacritty",
            TerminalApp::WezTerm => "wezterm",
        }
    }
}

/// PATH × PATHEXT 纯 Rust 遍历,语义即 CreateProcess 的查找规则。不走
/// where.exe:它把输出编码成控制台 codepage,非 ASCII 用户名的路径经
/// from_utf8_lossy 必坏;env::var/Path 全程 Unicode,无此折损,也免掉
/// GUI 进程起 console 子进程的窗口闪现。无扩展名裸文件(npm 的 bash
/// shim)不参与候选,天然滤掉。
pub(super) fn probe_clis(missing: &[&str]) -> HashMap<String, String> {
    let mut found = HashMap::new();
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|e| e.starts_with('.'))
        .map(str::to_string)
        .collect();
    // dirs 外层单遍 PATH、命中即从 pending 摘除:PATH 目录序 × PATHEXT 序的
    // 首个命中语义不变,但 is_file(Windows 上每次都是一次 CreateFileW,还要
    // 过 Defender)只付 O(PATH),不是 O(PATH × bins);候选文件名先算好,
    // 循环里零 format!
    let mut pending: Vec<(&str, Vec<String>)> = missing
        .iter()
        .map(|w| (*w, exts.iter().map(|e| format!("{w}{e}")).collect()))
        .collect();
    for dir in std::env::split_paths(&path_var) {
        if pending.is_empty() {
            break;
        }
        if dir.as_os_str().is_empty() {
            continue;
        }
        pending.retain(|(want, names)| {
            for name in names {
                let cand = dir.join(name);
                // WindowsApps 的 app-execution alias(wt.exe)是 0 字节
                // reparse 文件,is_file 为真、可直接 spawn
                if cand.is_file() {
                    found.insert(want.to_string(), cand.to_string_lossy().to_string());
                    return false;
                }
            }
            true
        });
    }
    found
}

/// PowerShell 单引号字面量:唯一转义是 ' → ''。`$`、反引号、反斜杠全是
/// 普通字符,且命令行经 CreateProcessW 以 UTF-16 直达,无 codepage 折损
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// cmd 方言 quote:整体双引号包裹(Windows 文件名不允许 `"`,无内层转义
/// 面);`%` 在引号内仍会展开是 cmd 关不掉的固有行为,路径/会话 id 命中
/// 概率视为零。裸词直通条件比 POSIX 版多放行 `\`(盘符路径)。
fn cmd_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:\\=".contains(c))
    {
        return s.to_string();
    }
    format!("\"{s}\"")
}

/// 展示/剪贴板形态 = PowerShell 方言:`Set-Location -LiteralPath 'dir';
/// & 'cli' 'args…'`。-LiteralPath 防路径里的 `[ ]` 被当通配符;分隔用 `;`
/// 而非 `&&`——Windows PowerShell 5.1 不认 `&&`,而 cwd 已在 mod.rs 验过
/// 存在,cd 失败只剩竞态窗口。cmd 用户是唯一粘不动的群体,而 cmd 宿主由
/// launch_shell 直接注入、不经剪贴板,失败兜底选覆盖面最大的方言。
pub(super) fn compose_command(cli: &str, args: &[String], cwd: Option<&str>) -> String {
    let core = ps_call(cli, args);
    match cwd {
        Some(dir) => format!("Set-Location -LiteralPath {}; {core}", ps_quote(dir)),
        None => core,
    }
}

/// 纯调用段(无 cd、无分号):`& 'cli' 'args…'`。wt 宿主用它配合 `-d`
/// 传工作目录——wt 对命令行做无视引号的 `;` 分面板切分,脚本里带分号
/// 必被腰斩,cd 只能交给 wt 自己的 -d。
fn ps_call(cli: &str, args: &[String]) -> String {
    let mut core = format!("& {}", ps_quote(cli));
    for a in args {
        core.push(' ');
        core.push_str(&ps_quote(a));
    }
    core
}

/// wt 命令行的 `;` 转义(wt 文档规定 `\;`;引号不豁免,对 option 值同样
/// 生效)。路径/id 里出现分号是合法但极罕见的形态,统一转义兜住。
fn wt_escape(s: &str) -> String {
    s.replace(';', "\\;")
}

/// NUL 结尾的 UTF-16(*W 系 API 的入参形制;NUL 是防越界读的护栏,
/// 所有 Win32 宽字符串都走这一份)
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Windows 无深链类恢复目标(Kooky 仅 macOS),全部走 shell 命令
pub(super) fn deep_link_resume(_meta: &SessionMeta, _term: TerminalApp) -> Option<ResumeOutcome> {
    None
}

/// 已安装终端(启动后不变,进程内缓存;PATH 遍历是纯文件系统操作,首扫
/// 即毫秒级)
pub fn installed_terminals() -> &'static [TerminalApp] {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Vec<TerminalApp>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let bins: Vec<&str> = TerminalApp::ALL.iter().map(|t| t.id()).collect();
        let found = resolve_clis(&bins);
        TerminalApp::ALL
            .into_iter()
            .filter(|t| matches!(found.get(t.id()), Some(Some(_))))
            .collect()
    })
}

/// 某会话可用的恢复目标(Windows 无按 agent 过滤的目标,一律全量)
pub fn terminals_for(_agent: AgentId) -> Vec<TerminalApp> {
    installed_terminals().to_vec()
}

/// wt/第三方宿主内装的 shell:优先 pwsh(用户装了 7+ 就是它的默认),
/// 缺席退必装的 Windows PowerShell
fn powershell_bin() -> anyhow::Result<String> {
    resolve_cli("pwsh")
        .or_else(|| resolve_cli("powershell"))
        .ok_or_else(|| anyhow::anyhow!("PowerShell not found"))
}

/// PowerShell 会话前缀,`-Command` 后接脚本;`-NoExit` 即 keep-open
const PS_SESSION: [&str; 3] = ["-NoLogo", "-NoExit", "-Command"];

/// 按宿主方言起终端。keep-open 与 POSIX 的 `exec $SHELL` 同位:cmd 用
/// `/K`,PowerShell 用 `-NoExit`,命令跑完留在交互提示符。command 即
/// mod.rs 经 compose_command 拼好的 PowerShell 形态,PowerShell 系宿主
/// 直接复用;cmd 宿主方言不同,只能从结构化件重拼。
pub(super) fn launch_shell(
    term: TerminalApp,
    cli: &str,
    args: &[String],
    cwd: Option<&str>,
    command: &str,
) -> anyhow::Result<()> {
    let bin = term.id();
    let exe = resolve_cli(bin).ok_or_else(|| anyhow::anyhow!("{bin} not found"))?;
    let mut cmd = Command::new(&exe);
    match term {
        TerminalApp::Cmd => {
            // cmd 方言内联注入(**不得**改用 command:那是 PowerShell 方言)。
            // raw_arg 绕过 std 的 argv 引号规则——cmd 不做 argv 解析,std 把
            // 整串再包一层引号反而会毁掉内层结构;串首是 `cd`/裸词而非引号,
            // cmd 的引号保全规则生效,内层 "…" 原样送达
            let mut line = String::from("/K ");
            if let Some(dir) = cwd {
                // /d:跨盘符 cd 也生效(会话在 D: 而 cmd 起在 C: 的情形)
                line.push_str(&format!("cd /d {} && ", cmd_quote(dir)));
            }
            line.push_str(&cmd_quote(cli));
            for a in args {
                line.push(' ');
                line.push_str(&cmd_quote(a));
            }
            cmd.raw_arg(line);
        }
        TerminalApp::Pwsh | TerminalApp::WindowsPowershell => {
            cmd.args(PS_SESSION).arg(command);
        }
        TerminalApp::WindowsTerminal => {
            // wt 装 PowerShell 会话。工作目录走 wt 自己的 -d,脚本只剩纯调用
            // 段(见 ps_call);所有透传参数过 wt_escape——wt 的 `;` 切分
            // 无视引号,不转义的分号会把命令行腰斩成两个面板
            if let Some(dir) = cwd {
                cmd.args(["-d", &wt_escape(dir)]);
            }
            cmd.arg(wt_escape(&powershell_bin()?));
            cmd.args(PS_SESSION).arg(wt_escape(&ps_call(cli, args)));
        }
        TerminalApp::Alacritty | TerminalApp::WezTerm => {
            // 第三方宿主装 PowerShell 会话:脚本全单引号、无内层双引号,经
            // 宿主的 argv 重引号(std → 宿主 → CreateProcess)往返无损;
            // 两家 argv 直传、无 wt 那套 `;` 语义,cd 留在脚本里
            if term == TerminalApp::WezTerm {
                cmd.args(["start", "--"]);
            } else {
                cmd.arg("-e");
            }
            cmd.arg(powershell_bin()?);
            cmd.args(PS_SESSION).arg(command);
        }
    }
    // 控制台宿主开自己的新窗即终端本体;wt/第三方的 CLI stub 是 console
    // 子系统,NO_WINDOW 压掉从 GUI 进程起动时闪现的空控制台(对 GUI 子进程
    // 无效果)
    cmd.creation_flags(match term {
        TerminalApp::Cmd | TerminalApp::Pwsh | TerminalApp::WindowsPowershell => CREATE_NEW_CONSOLE,
        TerminalApp::WindowsTerminal | TerminalApp::Alacritty | TerminalApp::WezTerm => {
            CREATE_NO_WINDOW
        }
    });
    spawn_and_reap(cmd)?;
    Ok(())
}

/// 终端图标提取:Windows 得走 SHGetFileInfo/ExtractIcon + HICON→PNG 编码
/// 一整条 GDI 链,v1 不做——UI 对无图标的终端行本就有无图兜底。与 Linux
/// 同策略,顺手在启动的 background 线程里预热 installed_terminals。
pub fn ensure_app_icons(_cache_dir: &Path) -> HashMap<String, PathBuf> {
    let _ = installed_terminals();
    HashMap::new()
}

/// Win32 剪贴板(CF_UNICODETEXT),UTF-16 直达。别家占着剪贴板时
/// OpenClipboard 会瞬时失败(剪贴板管理器常见),小步重试几轮;仍失败
/// 返回 false——调用方(clipboard_fallback)据此决定还敢不敢说 "copied"。
pub(super) fn copy_to_clipboard(text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    // windows-sys 里它归 Win32_System_Ole feature,为一个钉死的小常量不拉
    const CF_UNICODETEXT: u32 = 13;

    let wide = wide(text);
    let bytes = wide.len() * 2;
    unsafe {
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !opened {
            return false;
        }
        let ok = 'set: {
            if EmptyClipboard() == 0 {
                break 'set false;
            }
            let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if hmem.is_null() {
                break 'set false;
            }
            let dst = GlobalLock(hmem);
            if dst.is_null() {
                // hmem 就地泄漏:一次性小块,不为它多拉 GlobalFree 绑定
                break 'set false;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), dst.cast::<u8>(), bytes);
            GlobalUnlock(hmem);
            // 成功后内存归系统所有,不得再碰
            !SetClipboardData(CF_UNICODETEXT, hmem).is_null()
        };
        CloseClipboard();
        ok
    }
}

/// 批量删进回收站(trash crate → IFileOperation + FOF_ALLOWUNDO,资源管理
/// 器里可恢复;COM 初始化由 trash 自理;收 mod.rs 已过滤的真实路径)
pub(super) fn trash_existing(paths: &[&str]) -> anyhow::Result<()> {
    trash::delete_all(paths).map_err(|e| anyhow::anyhow!("Failed to move to Recycle Bin: {e}"))
}

/// 致命错误对话框:MessageBoxW。release 构建挂 windows 子系统,stderr
/// 无处可去,这是 GPUI 窗口起不来时唯一的可见通道。
pub(super) fn alert_dialog(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    };
    let text = wide(message);
    let caption = wide("Wake can't start");
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND,
        );
    }
}

/// 在资源管理器里进入目录(explorer 常驻单实例、前台进程即刻退出,
/// 且退出码恒非零,spawn 成功即算送达)
pub(super) fn open_dir(path: &str) {
    let mut cmd = Command::new("explorer.exe");
    cmd.arg(path);
    let _ = spawn_and_reap(cmd);
}

/// 选中文件(收 mod.rs 已剥好虚拟后缀的真实路径)。`/select,"path"` 必须
/// 整体一个参数且引号形制固定——explorer 自解析命令行、不吃 argv 转义,
/// raw_arg 原样直达。
pub(super) fn reveal_path(path: &str) {
    let mut cmd = Command::new("explorer.exe");
    cmd.raw_arg(format!("/select,\"{path}\""));
    let _ = spawn_and_reap(cmd);
}
