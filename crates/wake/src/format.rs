use chrono::{Local, TimeZone};

/// 详情页时间信息：保留到秒，避免相对时间丢失会话的精确时间上下文。
/// 列表与元信息带用的紧凑相对时间。扫读锚点要短:会话流第二行要同时
/// 放下 agent 图标、项目 chip、消息数,`23 min ago` 这类长形式只能从
/// 项目名身上砍宽度。完整时间在 tooltip 里。
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

/// epoch ms → "Mar 2026"(Insights 副标题);无效或 ≤0 给空串。
/// ts→字符串一律住本模块,渲染层不直接碰 chrono
pub fn month_year(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    Local
        .timestamp_millis_opt(ts)
        .single()
        .map(|dt| dt.format("%b %Y").to_string())
        .unwrap_or_default()
}

/// 千分位分组(Insights 大数字用):1234567 → "1,234,567"
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
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

/// 按显示宽度截断并补省略号。CJK / 全角记 2 格,其余记 1 格。
///
/// 代替 gpui 的 `truncate()`:后者只在布局时拿到 `known_dimensions.width`
/// 或 `AvailableSpace::Definite` 才画省略号(`elements/text.rs:357`),
/// 虚拟列表行与 flex 子项都拿不到,文字会按 max-content 铺开再被
/// `overflow_hidden` 硬裁在半个字上。
/// 字节数 → 人读的体积。只给 KB / MB 两档:放大预览里这是辅助信息,
/// 精确到字节没有意义,而 GB 级的图不会出现在会话里。
pub fn human_bytes(n: usize) -> String {
    const KB: f64 = 1024.;
    const MB: f64 = KB * 1024.;
    let n = n as f64;
    if n >= MB {
        format!("{:.1} MB", n / MB)
    } else {
        format!("{:.0} KB", (n / KB).max(1.))
    }
}

pub fn clip_display(s: &str, cells: usize) -> String {
    let width = |c: char| {
        if (c as u32) >= 0x1100 && !c.is_ascii() {
            2
        } else {
            1
        }
    };
    let total: usize = s.chars().map(width).sum();
    if total <= cells {
        return s.to_string();
    }
    // `…` 是全角,占两格;只留一格会让结果超出容器、省略号被裁掉
    let budget = cells.saturating_sub(2);
    let mut used = 0;
    let mut out = String::new();
    for c in s.chars() {
        let w = width(c);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
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

#[cfg(test)]
mod clip_tests {
    use super::clip_display;

    #[test]
    fn keeps_short_text_untouched() {
        assert_eq!(clip_display("hello", 20), "hello");
        assert_eq!(clip_display("中文标题", 20), "中文标题");
    }

    #[test]
    fn cjk_counts_as_two_cells() {
        assert_eq!(clip_display("中文标题", 6), "中文…");
    }

    #[test]
    fn ascii_counts_as_one_cell() {
        assert_eq!(clip_display("abcdefgh", 5), "abc…");
    }

    #[test]
    fn never_splits_a_char() {
        // 省略号 2 格 + 内容 3 格,第二个汉字放不下
        assert_eq!(clip_display("中文标题", 5), "中…");
    }

    #[test]
    fn reserves_room_for_the_ellipsis() {
        let width = |s: &str| -> usize {
            s.chars()
                .map(|c| {
                    if (c as u32) >= 0x1100 && !c.is_ascii() {
                        2
                    } else {
                        1
                    }
                })
                .sum()
        };
        for cells in 4..40 {
            assert!(width(&clip_display("一二三四五六七八九十", cells)) <= cells);
            assert!(width(&clip_display("abcdefghijklmnopqrst", cells)) <= cells);
        }
    }
}
