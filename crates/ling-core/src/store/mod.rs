//! 持久化仓储层：rusqlite（bundled 内嵌 SQLite）。
//!
//! 安全约定：
//! - 所有 SQL 均为静态字符串 + prepared statement + `params!` 参数绑定；
//! - 严禁使用 format! / 字符串拼接组装 SQL（外部输入只经参数绑定进入数据库）；
//! - 结构变更走 [`migrations`] 的编号迁移。

pub mod migrations;

use rusqlite::{Connection, OptionalExtension, params};

/// 角色行
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterRow {
    pub id: i64,
    pub name: String,
    pub title: String,
    pub origin: String,
    pub faction: String,
    pub created_at: i64,
}

/// 会话行
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: i64,
    pub character_id: i64,
    pub title: String,
    pub created_at: i64,
}

/// 消息行
#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    pub id: i64,
    pub session_id: i64,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

/// 特质行
#[derive(Debug, Clone, PartialEq)]
pub struct TraitRow {
    pub id: i64,
    pub character_id: i64,
    pub name: String,
    pub description: String,
    pub activity: f64,
    pub dormant: bool,
    pub last_touched: i64,
}

/// 场景记忆行
#[derive(Debug, Clone, PartialEq)]
pub struct SceneRow {
    pub id: i64,
    pub character_id: i64,
    pub content: String,
    pub keywords: Vec<String>,
    /// 归属事件（聚合后回填）
    pub event_id: Option<i64>,
    pub created_at: i64,
}

/// 事件记忆行
#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub id: i64,
    pub character_id: i64,
    pub summary: String,
    pub keywords: Vec<String>,
    /// 归属弧光（聚合后回填）
    pub arc_id: Option<i64>,
    pub start_time: i64,
    pub end_time: i64,
}

/// 弧光记忆行
#[derive(Debug, Clone, PartialEq)]
pub struct ArcRow {
    pub id: i64,
    pub character_id: i64,
    pub title: String,
    pub summary: String,
    pub keywords: Vec<String>,
    pub start_time: i64,
    pub end_time: i64,
}

/// 语义图事实行（LMR）：记忆/特质/关系统一为 SPO + scope
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticFactRow {
    pub id: i64,
    pub character_id: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    /// self / user / world
    pub scope: String,
    /// 数值型语义（特质强度 0..=1）；非数值事实为 None
    pub value: Option<f64>,
    pub confidence: f64,
    pub importance: f64,
    /// 来源事件（证据链主锚点）
    pub source_event: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub valid_from: Option<i64>,
    pub valid_until: Option<i64>,
}

/// 证据行：一条语义事实由多条场景支撑/削弱
#[derive(Debug, Clone, PartialEq)]
pub struct FactEvidenceRow {
    pub id: i64,
    pub fact_id: i64,
    pub scene_id: i64,
    /// 正 = 确认，负 = 削弱
    pub delta: f64,
    pub created_at: i64,
}

/// 关键词列表 → JSON 文本（存库格式）
fn keywords_to_json(keywords: &[String]) -> String {
    serde_json::to_string(keywords).unwrap_or_else(|_| "[]".to_string())
}

/// JSON 文本 → 关键词列表（损坏数据按空列表兜底）
fn keywords_from_json(raw: String) -> Vec<String> {
    serde_json::from_str(&raw).unwrap_or_default()
}

fn row_to_character(row: &rusqlite::Row<'_>) -> rusqlite::Result<CharacterRow> {
    Ok(CharacterRow {
        id: row.get(0)?,
        name: row.get(1)?,
        title: row.get(2)?,
        origin: row.get(3)?,
        faction: row.get(4)?,
        created_at: row.get(5)?,
    })
}

/// 存储门面：持有连接，提供各表的 CRUD
pub struct Store {
    conn: Connection,
}

impl Store {
    /// 打开（或创建）磁盘数据库，开启外键并迁移到最新版本
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// 内存数据库（测试 / 临时数据）
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> rusqlite::Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        migrations::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// 手动触发迁移（幂等）
    pub fn migrate(&mut self) -> rusqlite::Result<()> {
        migrations::migrate(&mut self.conn)
    }

    /// 当前 schema 版本
    pub fn user_version(&self) -> rusqlite::Result<i64> {
        self.conn.query_row("PRAGMA user_version", [], |row| row.get(0))
    }

    /// 只读访问底层连接（自定义查询仍须遵守参数绑定约定）
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    // —— 角色 ——

    /// 新建角色，返回 id
    pub fn insert_character(
        &self,
        name: &str,
        title: &str,
        origin: &str,
        faction: &str,
        created_at: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO characters (name, title, origin, faction, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![name, title, origin, faction, created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_character(&self, id: i64) -> rusqlite::Result<Option<CharacterRow>> {
        self.conn
            .query_row(
                "SELECT id, name, title, origin, faction, created_at
                 FROM characters WHERE id = ?1",
                params![id],
                row_to_character,
            )
            .optional()
    }

    pub fn list_characters(&self) -> rusqlite::Result<Vec<CharacterRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, title, origin, faction, created_at FROM characters ORDER BY id",
        )?;
        let rows = stmt.query_map([], row_to_character)?;
        rows.collect()
    }

    /// 删除角色（级联删除其会话/消息/特质/记忆），返回是否删除了行
    pub fn delete_character(&self, id: i64) -> rusqlite::Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM characters WHERE id = ?1", params![id])?;
        Ok(affected > 0)
    }

    // —— 会话与消息 ——

    pub fn create_session(
        &self,
        character_id: i64,
        title: &str,
        created_at: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO sessions (character_id, title, created_at) VALUES (?1, ?2, ?3)",
            params![character_id, title, created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_sessions(&self, character_id: i64) -> rusqlite::Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, character_id, title, created_at
             FROM sessions WHERE character_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![character_id], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                character_id: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// 追加消息；role 合法性由表 CHECK 约束兜底，内容只经参数绑定入库
    pub fn append_message(
        &self,
        session_id: i64,
        role: &str,
        content: &str,
        created_at: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO messages (session_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![session_id, role, content, created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_messages(&self, session_id: i64) -> rusqlite::Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, content, created_at
             FROM messages WHERE session_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    // —— 特质 ——

    /// 写入/更新特质（同一角色同名特质幂等覆盖），返回特质 id
    pub fn upsert_trait(
        &self,
        character_id: i64,
        name: &str,
        description: &str,
        activity: f64,
        dormant: bool,
        last_touched: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO traits (character_id, name, description, activity, dormant, last_touched)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (character_id, name) DO UPDATE SET
                 description  = excluded.description,
                 activity     = excluded.activity,
                 dormant      = excluded.dormant,
                 last_touched = excluded.last_touched",
            params![character_id, name, description, activity, dormant, last_touched],
        )?;
        // ON CONFLICT 更新路径下 last_insert_rowid 不可靠，回查真实 id
        self.conn.query_row(
            "SELECT id FROM traits WHERE character_id = ?1 AND name = ?2",
            params![character_id, name],
            |row| row.get(0),
        )
    }

    pub fn list_traits(&self, character_id: i64) -> rusqlite::Result<Vec<TraitRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, character_id, name, description, activity, dormant, last_touched
             FROM traits WHERE character_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![character_id], |row| {
            Ok(TraitRow {
                id: row.get(0)?,
                character_id: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                activity: row.get(4)?,
                dormant: row.get(5)?,
                last_touched: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    // —— 三层记忆 ——

    /// 写入场景记忆（尚未归属任何事件），返回 id
    pub fn insert_scene(
        &self,
        character_id: i64,
        content: &str,
        keywords: &[String],
        created_at: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO memory_scenes (character_id, content, keywords, event_id, created_at)
             VALUES (?1, ?2, ?3, NULL, ?4)",
            params![character_id, content, keywords_to_json(keywords), created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 写入事件记忆（尚未归属任何弧光），返回 id
    pub fn insert_event(
        &self,
        character_id: i64,
        summary: &str,
        keywords: &[String],
        start_time: i64,
        end_time: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO memory_events (character_id, summary, keywords, arc_id, start_time, end_time)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
            params![character_id, summary, keywords_to_json(keywords), start_time, end_time],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 写入弧光记忆，返回 id
    pub fn insert_arc(
        &self,
        character_id: i64,
        title: &str,
        summary: &str,
        keywords: &[String],
        start_time: i64,
        end_time: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO memory_arcs (character_id, title, summary, keywords, start_time, end_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                character_id,
                title,
                summary,
                keywords_to_json(keywords),
                start_time,
                end_time
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 把若干场景挂到事件上（scene → event 聚合结果的回填）
    pub fn attach_scenes_to_event(&self, event_id: i64, scene_ids: &[i64]) -> rusqlite::Result<()> {
        let mut stmt = self
            .conn
            .prepare("UPDATE memory_scenes SET event_id = ?1 WHERE id = ?2")?;
        for scene_id in scene_ids {
            stmt.execute(params![event_id, scene_id])?;
        }
        Ok(())
    }

    /// 把若干事件挂到弧光上（event → arc 聚合结果的回填）
    pub fn attach_events_to_arc(&self, arc_id: i64, event_ids: &[i64]) -> rusqlite::Result<()> {
        let mut stmt = self
            .conn
            .prepare("UPDATE memory_events SET arc_id = ?1 WHERE id = ?2")?;
        for event_id in event_ids {
            stmt.execute(params![arc_id, event_id])?;
        }
        Ok(())
    }

    /// LMR：写入（或更新）一条语义事实。
    /// 同一 (character, subject, predicate, object, scope) 视为同一事实：
    /// 已存在则合并 value/confidence/importance 并刷新 updated_at，返回既有 id。
    pub fn upsert_semantic_fact(
        &self,
        character_id: i64,
        subject: &str,
        predicate: &str,
        object: &str,
        scope: &str,
        value: Option<f64>,
        confidence: f64,
        importance: f64,
        source_event: Option<i64>,
        now: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO semantic_facts (character_id, subject, predicate, object, scope,
                value, confidence, importance, source_event, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
             ON CONFLICT (character_id, subject, predicate, object, scope) DO UPDATE SET
                value = COALESCE(?6, value),
                confidence = MAX(confidence, ?7),
                importance = MAX(importance, ?8),
                source_event = COALESCE(?9, source_event),
                updated_at = ?10",
            params![character_id, subject, predicate, object, scope,
                    value, confidence, importance, source_event, now],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM semantic_facts
             WHERE character_id = ?1 AND subject = ?2 AND predicate = ?3 AND object = ?4 AND scope = ?5",
            params![character_id, subject, predicate, object, scope],
            |row| row.get(0),
        )?)
    }

    /// LMR：为语义事实追加证据（正=确认 负=削弱），并按证据增量重算置信度
    pub fn add_fact_evidence(
        &self,
        fact_id: i64,
        scene_id: i64,
        delta: f64,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO fact_evidence (fact_id, scene_id, delta, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![fact_id, scene_id, delta, now],
        )?;
        self.conn.execute(
            "UPDATE semantic_facts SET
                confidence = MIN(1.0, MAX(0.05, 0.5 + (SELECT COALESCE(SUM(delta), 0) FROM fact_evidence WHERE fact_id = ?1))),
                updated_at = ?2
             WHERE id = ?1",
            params![fact_id, now],
        )?;
        Ok(())
    }

    /// LMR：列出某角色的语义事实（按重要度降序）
    pub fn list_semantic_facts(&self, character_id: i64) -> rusqlite::Result<Vec<SemanticFactRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, character_id, subject, predicate, object, scope, value, confidence,
                    importance, source_event, created_at, updated_at, valid_from, valid_until
             FROM semantic_facts WHERE character_id = ?1 AND valid_until IS NULL
             ORDER BY importance DESC, updated_at DESC",
        )?;
        let rows = stmt
            .query_map(params![character_id], |row| {
                Ok(SemanticFactRow {
                    id: row.get(0)?,
                    character_id: row.get(1)?,
                    subject: row.get(2)?,
                    predicate: row.get(3)?,
                    object: row.get(4)?,
                    scope: row.get(5)?,
                    value: row.get(6)?,
                    confidence: row.get(7)?,
                    importance: row.get(8)?,
                    source_event: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    valid_from: row.get(12)?,
                    valid_until: row.get(13)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// LMR：列出某条语义事实的证据链
    pub fn list_fact_evidence(&self, fact_id: i64) -> rusqlite::Result<Vec<FactEvidenceRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, fact_id, scene_id, delta, created_at
             FROM fact_evidence WHERE fact_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map(params![fact_id], |row| {
                Ok(FactEvidenceRow {
                    id: row.get(0)?,
                    fact_id: row.get(1)?,
                    scene_id: row.get(2)?,
                    delta: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// LMR：粒度衰减——按所属事件的重要度推进场景 decay_level（0=raw, 1=summary, 2=micro）。
    /// 阈值与 [`crate::memory::lmr::decay_level`] 纯函数一致：重要度越高速率越慢，层级封顶 2，不删除数据。
    pub fn apply_scene_decay(&self, now: i64, half_life_secs: i64) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE memory_scenes SET decay_level = MIN(2, CAST(
                MAX(?1 - created_at, 0)
              / MAX(?2 / (0.5 + COALESCE((SELECT importance FROM memory_events
                                          WHERE id = memory_scenes.event_id), 0.5)), 1.0)
              AS INTEGER))
             WHERE created_at <= ?1",
            params![now, half_life_secs],
        )
    }

    pub fn list_scenes(&self, character_id: i64) -> rusqlite::Result<Vec<SceneRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, character_id, content, keywords, event_id, created_at
             FROM memory_scenes WHERE character_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![character_id], |row| {
            Ok(SceneRow {
                id: row.get(0)?,
                character_id: row.get(1)?,
                content: row.get(2)?,
                keywords: keywords_from_json(row.get(3)?),
                event_id: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn list_events(&self, character_id: i64) -> rusqlite::Result<Vec<EventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, character_id, summary, keywords, arc_id, start_time, end_time
             FROM memory_events WHERE character_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![character_id], |row| {
            Ok(EventRow {
                id: row.get(0)?,
                character_id: row.get(1)?,
                summary: row.get(2)?,
                keywords: keywords_from_json(row.get(3)?),
                arc_id: row.get(4)?,
                start_time: row.get(5)?,
                end_time: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn list_arcs(&self, character_id: i64) -> rusqlite::Result<Vec<ArcRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, character_id, title, summary, keywords, start_time, end_time
             FROM memory_arcs WHERE character_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![character_id], |row| {
            Ok(ArcRow {
                id: row.get(0)?,
                character_id: row.get(1)?,
                title: row.get(2)?,
                summary: row.get(3)?,
                keywords: keywords_from_json(row.get(4)?),
                start_time: row.get(5)?,
                end_time: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    /// 某事件下的全部场景（按 id 序）
    pub fn scenes_of_event(&self, event_id: i64) -> rusqlite::Result<Vec<SceneRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, character_id, content, keywords, event_id, created_at
             FROM memory_scenes WHERE event_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![event_id], |row| {
            Ok(SceneRow {
                id: row.get(0)?,
                character_id: row.get(1)?,
                content: row.get(2)?,
                keywords: keywords_from_json(row.get(3)?),
                event_id: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// 某弧光下的全部事件（按 id 序）
    pub fn events_of_arc(&self, arc_id: i64) -> rusqlite::Result<Vec<EventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, character_id, summary, keywords, arc_id, start_time, end_time
             FROM memory_events WHERE arc_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![arc_id], |row| {
            Ok(EventRow {
                id: row.get(0)?,
                character_id: row.get(1)?,
                summary: row.get(2)?,
                keywords: keywords_from_json(row.get(3)?),
                arc_id: row.get(4)?,
                start_time: row.get(5)?,
                end_time: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    // —— 角色档案更新 / 特质删除（应用层编辑器所需的纯新增方法）——

    /// 更新角色身份档案（姓名 / 称号 / 出身 / 阵营），返回是否更新了行。
    /// SQL 为静态语句，所有字段一律参数绑定。
    pub fn update_character(
        &self,
        id: i64,
        name: &str,
        title: &str,
        origin: &str,
        faction: &str,
    ) -> rusqlite::Result<bool> {
        let affected = self.conn.execute(
            "UPDATE characters SET name = ?2, title = ?3, origin = ?4, faction = ?5 WHERE id = ?1",
            params![id, name, title, origin, faction],
        )?;
        Ok(affected > 0)
    }

    /// 删除角色的一个特质，返回是否删除了行。
    pub fn delete_trait(&self, character_id: i64, name: &str) -> rusqlite::Result<bool> {
        let affected = self.conn.execute(
            "DELETE FROM traits WHERE character_id = ?1 AND name = ?2",
            params![character_id, name],
        )?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Store {
        Store::open_in_memory().expect("内存数据库初始化失败")
    }

    #[test]
    fn character_crud_roundtrip() {
        let store = mem();
        let id = store
            .insert_character("玄铃", "守夜人", "北境", "霜环", 1_000)
            .unwrap();
        let row = store.get_character(id).unwrap().expect("应当能读到角色");
        assert_eq!(row.name, "玄铃");
        assert_eq!(row.title, "守夜人");
        assert_eq!(row.origin, "北境");
        assert_eq!(row.faction, "霜环");
        assert_eq!(row.created_at, 1_000);
        assert_eq!(store.list_characters().unwrap().len(), 1);

        assert!(store.delete_character(id).unwrap());
        assert!(store.get_character(id).unwrap().is_none());
        // 重复删除返回 false
        assert!(!store.delete_character(id).unwrap());
    }

    #[test]
    fn trait_upsert_updates_same_row() {
        let store = mem();
        let cid = store.insert_character("玄铃", "", "", "", 1).unwrap();
        let first = store
            .upsert_trait(cid, "谨慎", "三思后行", 0.5, false, 100)
            .unwrap();
        let second = store
            .upsert_trait(cid, "谨慎", "更加三思", 0.9, true, 200)
            .unwrap();
        assert_eq!(first, second, "同名特质应更新同一行");
        let traits = store.list_traits(cid).unwrap();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].description, "更加三思");
        assert!((traits[0].activity - 0.9).abs() < 1e-9);
        assert!(traits[0].dormant);
        assert_eq!(traits[0].last_touched, 200);
    }

    #[test]
    fn messages_keep_order_and_role_is_constrained() {
        let store = mem();
        let cid = store.insert_character("玄铃", "", "", "", 1).unwrap();
        let sid = store.create_session(cid, "第一夜", 10).unwrap();
        store.append_message(sid, "user", "你好", 11).unwrap();
        store.append_message(sid, "assistant", "夜里风大。", 12).unwrap();
        let msgs = store.list_messages(sid).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "你好");
        assert_eq!(msgs[1].role, "assistant");
        // 非法 role 被 CHECK 约束拒绝（注入串也只被当作普通文本参数）
        assert!(
            store
                .append_message(sid, "intruder'; DROP TABLE messages;--", "x", 13)
                .is_err()
        );
        assert_eq!(store.list_messages(sid).unwrap().len(), 2);
    }

    #[test]
    fn sessions_scoped_by_character() {
        let store = mem();
        let c1 = store.insert_character("甲", "", "", "", 1).unwrap();
        let c2 = store.insert_character("乙", "", "", "", 1).unwrap();
        store.create_session(c1, "s1", 1).unwrap();
        store.create_session(c1, "s2", 2).unwrap();
        store.create_session(c2, "s3", 3).unwrap();
        assert_eq!(store.list_sessions(c1).unwrap().len(), 2);
        assert_eq!(store.list_sessions(c2).unwrap().len(), 1);
    }

    #[test]
    fn memory_layers_link_scene_event_arc() {
        let store = mem();
        let cid = store.insert_character("玄铃", "", "", "", 1).unwrap();
        let s1 = store
            .insert_scene(cid, "星图布局开始", &["星图".to_string()], 100)
            .unwrap();
        let s2 = store
            .insert_scene(cid, "星图布局完成", &["星图".to_string()], 200)
            .unwrap();
        // 第三条场景不挂接任何事件
        let s3 = store
            .insert_scene(cid, "领地四季", &["领地".to_string()], 300)
            .unwrap();

        let e1 = store
            .insert_event(cid, "完成星图布局", &["星图".to_string()], 100, 200)
            .unwrap();
        let e2 = store
            .insert_event(cid, "查看领地", &["领地".to_string()], 300, 300)
            .unwrap();
        store.attach_scenes_to_event(e1, &[s1, s2]).unwrap();

        let a1 = store
            .insert_arc(cid, "星图之弧", "从布局到渲染", &["星图".to_string()], 100, 200)
            .unwrap();
        store.attach_events_to_arc(a1, &[e1]).unwrap();

        // 一个事件包含多个场景
        let scenes = store.scenes_of_event(e1).unwrap();
        assert_eq!(
            scenes.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![s1, s2]
        );
        // 一条弧光包含该事件
        let events = store.events_of_arc(a1).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, e1);
        // 列表视图携带挂接信息与关键词
        let all_scenes = store.list_scenes(cid).unwrap();
        assert_eq!(all_scenes.len(), 3);
        assert_eq!(all_scenes[0].event_id, Some(e1));
        assert_eq!(all_scenes[2].id, s3);
        assert_eq!(all_scenes[2].event_id, None);
        assert_eq!(all_scenes[2].keywords, vec!["领地".to_string()]);
        let all_events = store.list_events(cid).unwrap();
        assert_eq!(all_events[0].arc_id, Some(a1));
        assert_eq!(all_events[1].arc_id, None);
        let arcs = store.list_arcs(cid).unwrap();
        assert_eq!(arcs.len(), 1);
        assert_eq!(arcs[0].title, "星图之弧");
        assert_eq!(arcs[0].keywords, vec!["星图".to_string()]);
        let _ = e2; // 保留一条未挂接的事件
    }

    #[test]
    fn deleting_character_cascades() {
        let store = mem();
        let cid = store.insert_character("甲", "", "", "", 1).unwrap();
        let sid = store.create_session(cid, "s", 1).unwrap();
        store.append_message(sid, "user", "hi", 2).unwrap();
        store.upsert_trait(cid, "勇敢", "", 0.5, false, 2).unwrap();
        store.insert_scene(cid, "场景", &[], 3).unwrap();

        assert!(store.delete_character(cid).unwrap());
        assert!(store.list_sessions(cid).unwrap().is_empty());
        assert!(store.list_messages(sid).unwrap().is_empty());
        assert!(store.list_traits(cid).unwrap().is_empty());
        assert!(store.list_scenes(cid).unwrap().is_empty());
    }

    #[test]
    fn migrations_are_idempotent() {
        let mut store = mem();
        let v1 = store.user_version().unwrap();
        assert_eq!(v1, migrations::latest_version());
        store.migrate().unwrap();
        assert_eq!(store.user_version().unwrap(), v1);
    }

    #[test]
    fn lmr_semantic_fact_upsert_and_evidence() {
        let store = mem();
        let cid = store
            .insert_character("璃", "", "", "", 1)
            .unwrap();
        // 首次写入 → 新建
        let id = store
            .upsert_semantic_fact(cid, "璃", "likes", "astronomy", "self", None, 0.6, 0.7, None, 100)
            .unwrap();
        assert_eq!(
            store.list_semantic_facts(cid).unwrap().len(),
            1,
            "同一 SPO+scope 幂等合并"
        );
        // 再写一次（不同置信度）→ 不产生新行
        store
            .upsert_semantic_fact(cid, "璃", "likes", "astronomy", "self", None, 0.9, 0.7, None, 200)
            .unwrap();
        assert_eq!(store.list_semantic_facts(cid).unwrap().len(), 1);
        // 证据链：两条确认 → 置信度重算
        let sid = store.insert_scene(cid, "聊星空", &[], 300).unwrap();
        let sid2 = store.insert_scene(cid, "又聊星空", &[], 400).unwrap();
        store.add_fact_evidence(id, sid, 0.2, 300).unwrap();
        store.add_fact_evidence(id, sid2, 0.15, 400).unwrap();
        let ev = store.list_fact_evidence(id).unwrap();
        assert_eq!(ev.len(), 2);
        let fact = store
            .list_semantic_facts(cid)
            .unwrap()
            .into_iter()
            .find(|f| f.id == id)
            .unwrap();
        assert!(
            (fact.confidence - 0.85).abs() < 1e-9,
            "confidence 应为 0.85，实际 {}（证据 {:?}）",
            fact.confidence,
            ev.iter().map(|e| e.delta).collect::<Vec<_>>()
        );
        // 删角色 → 级联清空语义图
        assert!(store.delete_character(cid).unwrap());
        assert!(store.list_semantic_facts(cid).unwrap().is_empty());
    }

    #[test]
    fn lmr_scene_decay_progresses_with_age() {
        let store = mem();
        let cid = store
            .insert_character("璃", "", "", "", 1)
            .unwrap();
        let half_life = 1000;
        let eid = store.insert_event(cid, "事件", &[], 0, 0).unwrap();
        store
            .conn
            .execute("UPDATE memory_events SET importance = 0.5 WHERE id = ?1", params![eid])
            .unwrap();
        let sid = store.insert_scene(cid, "场景", &[], 0).unwrap();
        store.attach_scenes_to_event(eid, &[sid]).unwrap();
        // 未到期：raw
        store.apply_scene_decay(half_life - 1, half_life).unwrap();
        let lvl = |store: &Store| -> i64 {
            store
                .conn
                .query_row(
                    "SELECT decay_level FROM memory_scenes WHERE id = ?1",
                    params![sid],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(lvl(&store), 0);
        // 超过一个半衰期：summary
        store.apply_scene_decay(half_life * 2, half_life).unwrap();
        assert!(lvl(&store) >= 1);
        // 远超：封顶 micro
        store.apply_scene_decay(half_life * 9, half_life).unwrap();
        assert_eq!(lvl(&store), 2);
    }
}
