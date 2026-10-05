//! 通用小工具：时间戳、相对时间、字符截断、特质文本解析、导出文件名时间戳。

/// 当前 Unix 秒
pub fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 本地可读时间戳（导出文件名用，形如 20261005-1530）
pub fn local_file_stamp() -> String {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
    )
}

/// 特质编辑文本的行内统计：返回 (提示文案, 是否存在缺冒号行)。
/// 每个非空行应形如「特质名：描述」（半角/全角冒号均可）。
pub fn traits_hint(text: &str) -> (String, bool) {
    let mut count = 0usize;
    let mut bad_line = 0usize;
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        count += 1;
        let missing_colon = line.split_once('：').or_else(|| line.split_once(':')).is_none();
        if missing_colon && bad_line == 0 {
            bad_line = idx + 1;
        }
    }
    if bad_line > 0 {
        (
            format!("已识别 {count} 条特质 · 第 {bad_line} 行缺少冒号，将被记为无名特质"),
            true,
        )
    } else if count > 0 {
        (format!("已识别 {count} 条特质"), false)
    } else {
        (String::new(), false)
    }
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

    #[test]
    fn traits_hint_counts_and_flags_bad_lines() {
        // 空 / 仅空白：不显示提示
        assert_eq!(traits_hint("  \n \n"), (String::new(), false));
        // 全部规范（半角冒号亦可）
        let (hint, bad) = traits_hint("谨慎：凡事三思\n冷静:临危不乱");
        assert_eq!(hint, "已识别 2 条特质");
        assert!(!bad);
        // 缺冒号行：提示首个坏行（解析器会把整行当作特质名，故提示用户）
        let (hint, bad) = traits_hint("谨慎：凡事三思\n\n坚韧执着不肯回头\n冷静：临危不乱");
        assert!(bad);
        assert!(hint.contains("第 3 行缺少冒号"), "提示首个坏行：{hint}");
        assert!(hint.contains("已识别 3 条特质"), "统计仍包含坏行：{hint}");
    }

    #[test]
    fn local_file_stamp_shape() {
        let stamp = local_file_stamp();
        // 形如 20261005-1530：8 位日期 + '-' + 4 位时间
        let parts: Vec<&str> = stamp.split('-').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert!(parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())));
    }
}
