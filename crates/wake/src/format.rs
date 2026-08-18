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
