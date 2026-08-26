use chrono::{Local, TimeZone};

pub fn relative_time(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    let now = chrono::Utc::now().timestamp_millis();
    let diff = now - ts;
    const MIN: i64 = 60_000;
    if diff < MIN {
        "now".to_string()
    } else if diff < 60 * MIN {
        format!("{}m", diff / MIN)
    } else if diff < 24 * 60 * MIN {
        format!("{}h", diff / (60 * MIN))
    } else if diff < 7 * 24 * 60 * MIN {
        format!("{}d", diff / (24 * 60 * MIN))
    } else {
        Local
            .timestamp_millis_opt(ts)
            .single()
            .map(|d| d.format("%m-%d").to_string())
            .unwrap_or_default()
    }
}

/// 详情页时间信息：保留到秒，避免相对时间丢失会话的精确时间上下文。
pub fn abs_date(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    Local
        .timestamp_millis_opt(ts)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

pub fn fmt_tokens(n: Option<i64>) -> String {
    match n {
        None | Some(0) => String::new(),
        Some(n) if n >= 1_000_000_000 => format!("{:.1}B", n as f64 / 1e9),
        Some(n) if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        Some(n) if n >= 1_000 => format!("{:.1}K", n as f64 / 1e3),
        Some(n) => n.to_string(),
    }
}

/// 进程内不变的 HOME,缓存住:数据源路径折叠与手输展开(expand_tilde)
/// 共用同一份。
fn cached_home() -> Option<&'static str> {
    static HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        // 与 adapter 侧同一个 HOME(wake_core::adapters::home_dir 的
        // WAKE_HOME 开关):两边不一致时,改道过的数据根不会折成 `~`
        std::env::var_os("WAKE_HOME")
            .map(std::path::PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(dirs::home_dir)
            .map(|h| h.to_string_lossy().to_string())
            .filter(|h| !h.is_empty())
    })
    .as_deref()
}

/// 绝对路径 → `~/…` 形式,**不折叠中段**。数据源面板要如实给出完整路径。
pub fn tilde_path(p: &str) -> String {
    // 边界必须落在分隔符上:裸 starts_with 会把 HOME 的同名前缀兄弟目录
    // 也折叠掉(HOME=/Users/al 时 /Users/al-data → "~-data",一个并不存在
    // 的 HOME 相对路径)。自定义 CODEX_HOME / XDG_DATA_HOME 让这种根变得可能。
    // 分隔符判定走 std::path::is_separator:Windows 上 `\` 与 `/` 都算
    match cached_home() {
        Some(h) => match p.strip_prefix(h) {
            Some(rest) if rest.is_empty() || rest.starts_with(std::path::is_separator) => {
                format!("~{rest}")
            }
            _ => p.to_string(),
        },
        None => p.to_string(),
    }
}

/// 手输路径的 `~` 前缀展开(tilde_path 的逆;仅前缀,边界同样落在分隔符上
/// ——Windows 用户手输 `~\foo` 同样认)
pub fn expand_tilde(p: &str) -> String {
    match p.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(std::path::is_separator) => {
            match cached_home() {
                Some(h) => format!("{h}{rest}"),
                None => p.to_string(),
            }
        }
        _ => p.to_string(),
    }
}

/// 首行截断预览
pub fn one_line(s: &str, max_chars: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = joined.chars().collect();
    if chars.len() > max_chars {
        let mut t: String = chars[..max_chars].iter().collect();
        t.push('…');
        t
    } else {
        joined
    }
}
