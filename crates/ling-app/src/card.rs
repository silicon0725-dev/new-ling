//! 「铸造」产物（模型输出的角色卡 JSON）的解析校验与清洗。
//!
//! ling-core::ai 已负责把模型输出宽松解析为 [`ForgedCharacter`]；
//! 这里做入库前的最后一道校验：姓名必填、字段去空白、特质去空名、数量封顶。

use ling_core::ai::ForgedCharacter;

/// 清洗后的角色卡（可直接入库）
#[derive(Debug, Clone, PartialEq)]
pub struct CleanCard {
    pub name: String,
    pub title: String,
    pub origin: String,
    pub faction: String,
    /// (特质名, 描述)
    pub traits: Vec<(String, String)>,
}

/// 单字段去空白 + 长度封顶
fn clean_field(value: &str, max_chars: usize) -> String {
    crate::util::truncate_chars(value.trim(), max_chars)
}

/// 校验并清洗铸造结果：
/// - 姓名去空白后非空（否则报中文错误，提示补充素材）；
/// - 各字段截断到安全长度；
/// - 丢弃无名特质，特质数量至多 [`MAX_TRAITS`] 条。
pub const MAX_TRAITS: usize = 12;

pub fn validate_forged(forged: &ForgedCharacter) -> Result<CleanCard, String> {
    let name = clean_field(&forged.name, 24);
    if name.is_empty() {
        return Err("模型没有给出角色姓名：请补充素材中关于姓名的描述后重试".to_string());
    }
    let mut traits: Vec<(String, String)> = Vec::new();
    for seed in &forged.traits {
        let t_name = clean_field(&seed.name, 16);
        if t_name.is_empty() {
            continue;
        }
        let desc = clean_field(&seed.description, 120);
        match traits.iter_mut().find(|(n, _)| *n == t_name) {
            Some(slot) => {
                if slot.1.is_empty() {
                    slot.1 = desc;
                }
            }
            None => traits.push((t_name, desc)),
        }
        if traits.len() >= MAX_TRAITS {
            break;
        }
    }
    Ok(CleanCard {
        name,
        title: clean_field(&forged.title, 32),
        origin: clean_field(&forged.origin, 60),
        faction: clean_field(&forged.faction, 60),
        traits,
    })
}

/// 预览文本（确认入库前展示给用户）
pub fn preview_text(card: &CleanCard) -> String {
    let title_part = if card.title.is_empty() { String::new() } else { format!(" · {}", card.title) };
    let faction_part = if card.faction.is_empty() { String::new() } else { format!("（{}）", card.faction) };
    let mut lines = vec![format!("姓名：{}{}{}", card.name, title_part, faction_part)];
    if !card.origin.is_empty() {
        lines.push(format!("出身：{}", card.origin));
    }
    if !card.traits.is_empty() {
        lines.push("特质：".to_string());
        for (n, d) in &card.traits {
            if d.is_empty() {
                lines.push(format!("  · {n}"));
            } else {
                lines.push(format!("  · {n}：{d}"));
            }
        }
    } else {
        lines.push("特质：模型未提炼出特质，可入库后手工补充。".to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ling_core::ai::TraitSeed;

    fn forged(name: &str, traits: Vec<(&str, &str)>) -> ForgedCharacter {
        ForgedCharacter {
            name: name.to_string(),
            title: "  守夜人  ".to_string(),
            origin: "北境".to_string(),
            faction: "霜环".to_string(),
            traits: traits
                .into_iter()
                .map(|(n, d)| TraitSeed {
                    name: n.to_string(),
                    description: d.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn accepts_and_trims_valid_card() {
        let card = validate_forged(&forged(" 玄铃 ", vec![("谨慎", "凡事三思"), ("", "无名应被丢弃")]))
            .unwrap();
        assert_eq!(card.name, "玄铃");
        assert_eq!(card.title, "守夜人");
        assert_eq!(card.traits.len(), 1);
        assert_eq!(card.traits[0], ("谨慎".to_string(), "凡事三思".to_string()));
    }

    #[test]
    fn rejects_missing_name() {
        let err = validate_forged(&forged("   ", vec![])).unwrap_err();
        assert!(err.contains("姓名"));
    }

    #[test]
    fn caps_trait_count_and_merges_duplicates() {
        let many: Vec<(&str, &str)> = (0..20).map(|i| (format!("特质{i}").leak() as &str, "")).collect();
        // 构造重名 + 超量特质
        let mut seeds: Vec<(&str, &str)> = vec![("谨慎", ""), ("谨慎", "凡事三思")];
        seeds.extend(many);
        let card = validate_forged(&forged("玄铃", seeds)).unwrap();
        assert!(card.traits.len() <= MAX_TRAITS);
        assert_eq!(card.traits.len(), MAX_TRAITS, "应恰好保留上限数量");
        assert_eq!(card.traits[0].1, "凡事三思", "重名特质合并描述");
    }

    #[test]
    fn preview_lists_traits() {
        let card = validate_forged(&forged("玄铃", vec![("谨慎", "凡事三思")])).unwrap();
        let text = preview_text(&card);
        assert!(text.contains("玄铃"));
        assert!(text.contains("凡事三思"));
    }
}
