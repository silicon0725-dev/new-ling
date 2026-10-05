//! 领地「近况」：LLM 消息构造（纯函数）+ 近况时间线表的读写。
//!
//! 近况表由应用层建在 ling-core Store 的同一 SQLite 里（`app_timeline`），
//! 所有 SQL 均为静态语句 + `params!` 参数绑定，外键随角色删除级联清理。

use ling_core::ai::ChatMessage;
use ling_core::character::Character;
use ling_core::memory::Scene;
use ling_core::store::Store;
use rusqlite::params;

/// 一条近况记录
#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub id: i64,
    pub character_id: i64,
    pub character_name: String,
    pub content: String,
    pub created_at: i64,
}

/// 建表（幂等）。DDL 为静态语句，不含任何外部输入。
pub fn ensure_table(store: &Store) -> rusqlite::Result<()> {
    store.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS app_timeline (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
            content       TEXT    NOT NULL,
            created_at    INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_app_timeline_character ON app_timeline (character_id, id);",
    )
}

/// 追加一条近况，返回 id
pub fn insert_entry(
    store: &Store,
    character_id: i64,
    content: &str,
    created_at: i64,
) -> rusqlite::Result<i64> {
    store.conn().execute(
        "INSERT INTO app_timeline (character_id, content, created_at) VALUES (?1, ?2, ?3)",
        params![character_id, content, created_at],
    )?;
    Ok(store.conn().last_insert_rowid())
}

/// 最近近况（新→旧，带角色名，至多 limit 条）
pub fn list_entries(store: &Store, limit: i64) -> rusqlite::Result<Vec<TimelineEntry>> {
    let mut stmt = store.conn().prepare(
        "SELECT t.id, t.character_id, c.name, t.content, t.created_at
         FROM app_timeline t JOIN characters c ON c.id = t.character_id
         ORDER BY t.id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(TimelineEntry {
            id: row.get(0)?,
            character_id: row.get(1)?,
            character_name: row.get(2)?,
            content: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    rows.collect()
}

/// 「生成近况」的消息序列（纯函数，便于测试）：
/// 系统提示要求以第三人称写一段 60~120 字的生活记录；用户消息携带人设与最近记忆。
pub fn timeline_messages(character: &Character, scenes: &[Scene]) -> Vec<ChatMessage> {
    let id = &character.identity;
    let mut persona = format!(
        "姓名：{}；称号：{}；出身：{}；阵营：{}。",
        id.name, id.title, id.origin, id.faction
    );
    let active: Vec<String> = character
        .active_traits()
        .iter()
        .take(4)
        .map(|t| t.name.clone())
        .collect();
    if !active.is_empty() {
        persona.push_str(&format!("性格特质：{}。", active.join("、")));
    }

    let mut memories = String::new();
    for scene in scenes.iter().rev().take(5) {
        memories.push_str(&format!(
            "· {}\n",
            crate::util::truncate_chars(scene.content.trim(), 100)
        ));
    }
    let user = if memories.is_empty() {
        format!("{persona}\n\n最近没有留下记忆片段。请只依据人设，想象他/她今天在领地中做的一件小事。")
    } else {
        format!("{persona}\n\n最近的记忆片段：\n{memories}\n请依据人设与这些记忆，写下他/她今天在领地中的生活近况。")
    };

    vec![
        ChatMessage::system(
            "你是领地生活记录官。根据角色人设与最近的记忆，用中文第三人称写一段 \
             60~120 字的生活近况：一件具体的小事，口吻克制、有画面感。\
             只输出近况正文，不要标题、不要列表、不要解释。",
        ),
        ChatMessage::user(user),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ling_core::character::{Character, Identity};

    fn hero() -> Character {
        let mut c = Character::new(3, Identity::new("玄铃", "守夜人", "北境", "霜环"), 1_000);
        c.emerge_trait("谨慎", "凡事三思", 1_000);
        c
    }

    #[test]
    fn messages_carry_persona_and_memories() {
        let scenes = vec![Scene {
            id: 1,
            content: "在星图前守候到深夜".to_string(),
            keywords: vec![],
            timestamp: 2_000,
        }];
        let msgs = timeline_messages(&hero(), &scenes);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert!(msgs[1].content.contains("玄铃"));
        assert!(msgs[1].content.contains("守夜人"));
        assert!(msgs[1].content.contains("谨慎"));
        assert!(msgs[1].content.contains("星图前守候到深夜"));
    }

    #[test]
    fn messages_without_memories_still_work() {
        let msgs = timeline_messages(&hero(), &[]);
        assert!(msgs[1].content.contains("最近没有留下记忆片段"));
    }

    #[test]
    fn timeline_table_crud_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        ensure_table(&store).unwrap();
        ensure_table(&store).unwrap(); // 幂等
        let cid = store
            .insert_character("玄铃", "守夜人", "北境", "霜环", 1)
            .unwrap();
        insert_entry(&store, cid, "清晨擦拭星盘，午后在檐下看雪。", 100).unwrap();
        insert_entry(&store, cid, "夜里为守夜灯添了油。", 200).unwrap();
        let entries = list_entries(&store, 10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].content, "夜里为守夜灯添了油。", "新记录在前");
        assert_eq!(entries[0].character_name, "玄铃");
        // 随角色级联删除
        store.delete_character(cid).unwrap();
        assert!(list_entries(&store, 10).unwrap().is_empty());
    }
}
