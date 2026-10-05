//! 对话 system prompt 组装：角色卡 + 活跃特质 + 记忆召回。
//!
//! 纯逻辑模块：输入 ling-core 领域对象，输出提示词文本，便于内联单测。

use ling_core::character::Character;
use ling_core::memory::retriever::Retriever;
use ling_core::memory::{KeywordRetriever, Scene};

/// 每次注入 system prompt 的记忆条数上限
pub const MEMORY_LIMIT: usize = 5;

/// 依用户最新消息召回相关场景记忆：按关键词相似度降序，过滤零分，至多 `limit` 条。
pub fn recall_memories(scenes: &[Scene], query: &str, limit: usize) -> Vec<Scene> {
    let retriever = KeywordRetriever;
    let mut scored: Vec<(f32, &Scene)> = scenes
        .iter()
        .map(|s| (retriever.similarity(query, &s.content), s))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .filter(|(score, _)| *score > 0.0)
        .take(limit)
        .map(|(_, s)| s.clone())
        .collect()
}

/// 组装 system prompt：角色身份 → 活跃特质（按鲜明程度降序）→ 相关记忆。
/// 已休眠特质不注入；无记忆时省略记忆分区。
pub fn build_system_prompt(character: &Character, memories: &[Scene]) -> String {
    let id = &character.identity;
    let mut prompt = String::new();
    prompt.push_str("你将扮演以下角色，与用户进行沉浸式角色扮演对话。始终保持入戏，不要跳出角色。\n\n");

    prompt.push_str("【角色卡】\n");
    prompt.push_str(&format!("姓名：{}\n", id.name));
    if !id.title.is_empty() {
        prompt.push_str(&format!("称号：{}\n", id.title));
    }
    if !id.origin.is_empty() {
        prompt.push_str(&format!("出身：{}\n", id.origin));
    }
    if !id.faction.is_empty() {
        prompt.push_str(&format!("阵营：{}\n", id.faction));
    }

    let active: Vec<&ling_core::character::CharacterTrait> = character.active_traits();
    if !active.is_empty() {
        prompt.push_str("\n【活跃特质】按当前性格鲜明程度排序：\n");
        for trait_ in active.iter().take(6) {
            if trait_.description.is_empty() {
                prompt.push_str(&format!("- {}\n", trait_.name));
            } else {
                prompt.push_str(&format!("- {}：{}\n", trait_.name, trait_.description));
            }
        }
    }

    if !memories.is_empty() {
        prompt.push_str("\n【相关记忆】与用户最新消息相关的往昔片段（背景参考）：\n");
        for scene in memories {
            let brief = crate::util::truncate_chars(scene.content.trim(), 120);
            prompt.push_str(&format!("· {brief}\n"));
        }
    }

    prompt.push_str(&format!(
        "\n要求：以「{}」的身份自然回应；语言风格贴合上述人设与特质；\
         记忆仅作背景，除非用户提及不要主动罗列。",
        id.name
    ));
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use ling_core::character::{Character, Identity};

    const T0: i64 = 1_700_000_000;

    fn hero() -> Character {
        let mut c = Character::new(1, Identity::new("玄铃", "守夜人", "北境", "霜环"), T0);
        c.emerge_trait("谨慎", "凡事三思", T0);
        c.emerge_trait("坚韧", "九头牛拉不回", T0);
        c
    }

    fn scene(id: i64, content: &str, ts: i64) -> Scene {
        Scene {
            id,
            content: content.to_string(),
            keywords: Vec::new(),
            timestamp: ts,
        }
    }

    #[test]
    fn prompt_contains_persona_and_traits() {
        let c = hero();
        let p = build_system_prompt(&c, &[]);
        assert!(p.contains("玄铃"));
        assert!(p.contains("守夜人"));
        assert!(p.contains("北境"));
        assert!(p.contains("霜环"));
        assert!(p.contains("谨慎"));
        assert!(p.contains("凡事三思"));
        assert!(!p.contains("【相关记忆】"), "无记忆时应省略记忆分区");
    }

    #[test]
    fn prompt_excludes_dormant_traits_and_includes_memories() {
        let mut c = hero();
        c.emerge_trait("旧伤", "", T0);
        // 「旧伤」长期未被提及 → 休眠
        if let Some(t) = c.traits.iter_mut().find(|t| t.name == "旧伤") {
            t.activity = 0.05;
            t.last_touched = T0 - 30 * 86_400;
        }
        c.advance_to(T0);
        let memories = vec![scene(9, "在星图前彻夜守候，等一颗星亮起。", T0)];
        let p = build_system_prompt(&c, &memories);
        assert!(!p.contains("旧伤"), "休眠特质不应注入");
        assert!(p.contains("星图前彻夜守候"));
    }

    #[test]
    fn recall_ranks_and_limits() {
        let scenes = vec![
            scene(1, "星图布局收敛", 1),
            scene(2, "星图渲染完成", 2),
            scene(3, "领地四季变化", 3),
        ];
        let hits = recall_memories(&scenes, "星图布局", 2);
        assert!(!hits.is_empty());
        assert!(hits[0].content.contains("布局"), "最相关的记忆应排第一");
        assert!(hits.iter().all(|s| s.content.contains("星图")), "零分记忆被过滤");
        assert!(recall_memories(&scenes, "星图", 1).len() <= 1);
        // 全部字符均不重叠的查询 → 无召回
        assert!(recall_memories(&scenes, "你好呀", 5).is_empty());
    }
}
