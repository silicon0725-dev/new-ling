//! 编号迁移：SQL 内嵌为常量，按版本号顺序执行，进度记录在 `PRAGMA user_version`。
//!
//! 约定：只增不改——已发布的迁移不允许修改，新的结构变更追加新版本。
//! 这里的 SQL 全部是静态常量，不存在任何外部输入拼接。

use rusqlite::Connection;

/// 迁移列表：(版本号, 名称, SQL)
pub const MIGRATIONS: &[(i64, &str, &str)] = &[(
    1,
    "初始建表：角色 / 会话 / 消息 / 特质 / 三层记忆",
    r#"
CREATE TABLE characters (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    title       TEXT    NOT NULL DEFAULT '',
    origin      TEXT    NOT NULL DEFAULT '',
    faction     TEXT    NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL
);

CREATE TABLE sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    title         TEXT    NOT NULL DEFAULT '',
    created_at    INTEGER NOT NULL
);

CREATE TABLE messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT    NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    content     TEXT    NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE TABLE traits (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    description   TEXT    NOT NULL DEFAULT '',
    activity      REAL    NOT NULL DEFAULT 0.5,
    dormant       INTEGER NOT NULL DEFAULT 0,
    last_touched  INTEGER NOT NULL,
    UNIQUE (character_id, name)
);

CREATE TABLE memory_arcs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    title         TEXT    NOT NULL,
    summary       TEXT    NOT NULL DEFAULT '',
    keywords      TEXT    NOT NULL DEFAULT '[]',
    start_time    INTEGER NOT NULL,
    end_time      INTEGER NOT NULL
);

CREATE TABLE memory_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    summary       TEXT    NOT NULL,
    keywords      TEXT    NOT NULL DEFAULT '[]',
    arc_id        INTEGER REFERENCES memory_arcs(id) ON DELETE SET NULL,
    start_time    INTEGER NOT NULL,
    end_time      INTEGER NOT NULL
);

CREATE TABLE memory_scenes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    content       TEXT    NOT NULL,
    keywords      TEXT    NOT NULL DEFAULT '[]',
    event_id      INTEGER REFERENCES memory_events(id) ON DELETE SET NULL,
    created_at    INTEGER NOT NULL
);

CREATE INDEX idx_messages_session ON messages (session_id, id);
CREATE INDEX idx_traits_character ON traits (character_id);
CREATE INDEX idx_scenes_event     ON memory_scenes (event_id);
CREATE INDEX idx_events_arc       ON memory_events (arc_id);
"#,
    ),
    (
    2,
    "LMR：语义图（事实/特质/关系统一）+ 证据链 + 记忆三表示与衰减字段",
    r#"
CREATE TABLE semantic_facts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    subject       TEXT    NOT NULL,
    predicate     TEXT    NOT NULL,
    object        TEXT    NOT NULL,
    scope         TEXT    NOT NULL DEFAULT 'self' CHECK (scope IN ('self', 'user', 'world')),
    value         REAL,
    confidence    REAL    NOT NULL DEFAULT 0.5,
    importance    REAL    NOT NULL DEFAULT 0.5,
    source_event  INTEGER REFERENCES memory_events(id) ON DELETE SET NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    valid_from    INTEGER,
    valid_until   INTEGER,
    UNIQUE (character_id, subject, predicate, object, scope)
);
CREATE INDEX idx_semantic_char ON semantic_facts (character_id, scope);
CREATE INDEX idx_semantic_spo  ON semantic_facts (subject, predicate);

CREATE TABLE fact_evidence (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id    INTEGER NOT NULL REFERENCES semantic_facts(id) ON DELETE CASCADE,
    scene_id   INTEGER NOT NULL REFERENCES memory_scenes(id) ON DELETE CASCADE,
    delta      REAL    NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_evidence_fact ON fact_evidence (fact_id);

ALTER TABLE memory_scenes ADD COLUMN micro_summary TEXT NOT NULL DEFAULT '';
ALTER TABLE memory_scenes ADD COLUMN decay_level   INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memory_events ADD COLUMN micro_summary TEXT NOT NULL DEFAULT '';
ALTER TABLE memory_events ADD COLUMN importance    REAL    NOT NULL DEFAULT 0.5;
ALTER TABLE memory_events ADD COLUMN confidence    REAL    NOT NULL DEFAULT 0.5;
"#,
    ),
];

/// 当前应有的最新版本号
pub fn latest_version() -> i64 {
    MIGRATIONS.last().map(|(v, _, _)| *v).unwrap_or(0)
}

/// 执行所有未应用的迁移（幂等）
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let tx = conn.transaction()?;
    for (version, _name, sql) in MIGRATIONS.iter().filter(|(v, _, _)| *v > current) {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
    }
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_ordered_from_one() {
        let mut expected = 0;
        for (version, _, _) in MIGRATIONS {
            expected += 1;
            assert_eq!(*version, expected, "迁移版本必须从 1 起连续递增");
        }
        assert!(latest_version() >= 1);
    }
}
