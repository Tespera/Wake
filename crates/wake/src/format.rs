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

/// 绝对时间(详情页时间行):yyyy-MM-dd HH:mm:ss
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

/// 进程内不变的 HOME,缓存住:折叠(tilde_path 经 display_file_path 落在
/// 详情页 header,每帧都跑)与手输展开(expand_tilde)共用同一份
fn cached_home() -> Option<&'static str> {
    static HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        dirs::home_dir()
            .map(|h| h.to_string_lossy().to_string())
            .filter(|h| !h.is_empty())
    })
    .as_deref()
}

/// 绝对路径 → `~/…` 形式,**不折叠中段**。数据源面板要如实给出完整路径
/// (display_file_path 那种 `~/a/…/file` 的折叠在那里会把信息吃掉)
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

/// 会话文件路径的展示形态(详情页路径行):SQLite 虚拟路径剥 `#<id>`、
/// HOME 缩成 `~`、深路径折叠中段(根目录 + … + 文件名)。
/// 仅用于展示——Reveal in Finder 仍传原始完整路径。
pub fn display_file_path(path: &str) -> String {
    // 虚拟路径 <db>#<id>:id 不是路径的一部分,展示到库文件为止
    let p = path
        .rsplit_once('#')
        .filter(|(db, _)| db.ends_with(".db"))
        .map(|(db, _)| db)
        .unwrap_or(path);
    let tilde = tilde_path(p);
    let parts: Vec<&str> = tilde.split(std::path::is_separator).collect();
    match (parts.first(), parts.get(1), parts.last()) {
        // 超过 根/次级/…/文件 四段的深路径折叠中段(重拼用本平台主分隔符,
        // Windows 展示 `~\a\…\file` 与系统一致)
        (Some(root), Some(second), Some(file)) if parts.len() > 4 => {
            let s = std::path::MAIN_SEPARATOR;
            format!("{root}{s}{second}{s}…{s}{file}")
        }
        _ => tilde,
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
