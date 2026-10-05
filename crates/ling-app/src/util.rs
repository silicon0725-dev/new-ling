//! 通用小工具：时间戳、相对时间、字符截断、特质文本解析。

/// 当前 Unix 秒
pub fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 相对时间（避免引入时区依赖）：刚刚 / N 分钟前 / N 小时前 / N 天前
pub fn rel_time(ts: i64, now: i64) -> String {
    let diff = (now - ts).max(0);
    if diff < 60 {
        "刚刚".to_string()
    } else if diff < 3600 {
        format!("{} 分钟前", diff / 60)
    } else if diff < 86_400 {
        format!("{} 小时前", diff / 3600)
    } else {
        format!("{} 天前", diff / 86_400)
    }
}

/// 按字符数截断，超长补省略号
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max).collect();
        cut.push('…');
        cut
    }
}

/// 解析特质编辑文本：每行一条「特质名：描述」（半角/全角冒号均可），
/// 忽略空行；重名行合并到首次出现处（描述为空时补充）。
pub fn parse_traits_text(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (name, desc) = match line.split_once('：').or_else(|| line.split_once(':')) {
            Some((n, d)) => (n.trim().to_string(), d.trim().to_string()),
            None => (line.to_string(), String::new()),
        };
        if name.is_empty() {
            continue;
        }
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => {
                if slot.1.is_empty() {
                    slot.1 = desc;
                }
            }
            None => out.push((name, desc)),
        }
    }
    out
}

/// 特质列表 → 编辑文本（与 parse_traits_text 互逆）
pub fn traits_text(items: &[(String, String)]) -> String {
    items
        .iter()
        .map(|(n, d)| {
            if d.is_empty() {
                n.clone()
            } else {
                format!("{n}：{d}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_time_buckets() {
        let now = 1_000_000;
        assert_eq!(rel_time(now - 10, now), "刚刚");
        assert_eq!(rel_time(now - 61, now), "1 分钟前");
        assert_eq!(rel_time(now - 7200, now), "2 小时前");
        assert_eq!(rel_time(now - 3 * 86_400, now), "3 天前");
    }

    #[test]
    fn truncate_appends_ellipsis() {
        assert_eq!(truncate_chars("灵", 5), "灵");
        assert_eq!(truncate_chars("星图布局", 2), "星图…");
    }

    #[test]
    fn traits_text_roundtrip() {
        let src = "谨慎：凡事三思\n坚韧\n 枧木：执着 \n\n";
        let parsed = parse_traits_text(src);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], ("谨慎".to_string(), "凡事三思".to_string()));
        assert_eq!(parsed[1], ("坚韧".to_string(), "".to_string()));
        assert_eq!(parsed[2], ("枧木".to_string(), "执着".to_string()));
        // 互逆
        let text = traits_text(&parsed);
        assert_eq!(parse_traits_text(&text), parsed);
    }

    #[test]
    fn traits_text_supports_half_width_colon() {
        let parsed = parse_traits_text("冷静:临危不乱");
        assert_eq!(
            parsed,
            vec![("冷静".to_string(), "临危不乱".to_string())]
        );
    }
}
