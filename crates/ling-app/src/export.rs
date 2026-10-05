//! 全量数据导出 / 导入：把整个 SQLite（角色、特质、会话、消息、三层记忆、近况）
//! 序列化为一个 JSON 备份文件，导入时整体替换现有数据。
//!
//! 纯逻辑 + 仓储读写均在此层完成；所有数据库访问走 ling-core Store API
//! （其内部一律参数绑定）。层间挂接（场景→事件、事件→弧光）在 JSON 中
//! 用数组下标表达，避免暴露数据库自增 id。

use ling_core::store::Store;
use serde::{Deserialize, Serialize};

use crate::timeline;

/// 备份格式版本（结构变更时递增，导入侧只接受已知版本）
pub const BUNDLE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportBundle {
    pub version: u32,
    pub exported_at: i64,
    pub characters: Vec<ExpCharacter>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpCharacter {
    pub name: String,
    pub title: String,
    pub origin: String,
    pub faction: String,
    pub created_at: i64,
    #[serde(default)]
    pub traits: Vec<ExpTrait>,
    #[serde(default)]
    pub sessions: Vec<ExpSession>,
    #[serde(default)]
    pub scenes: Vec<ExpScene>,
    #[serde(default)]
    pub events: Vec<ExpEvent>,
    #[serde(default)]
    pub arcs: Vec<ExpArc>,
    #[serde(default)]
    pub timeline: Vec<ExpTimeline>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpTrait {
    pub name: String,
    pub description: String,
    pub activity: f64,
    pub dormant: bool,
    pub last_touched: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpSession {
    pub title: String,
    pub created_at: i64,
    #[serde(default)]
    pub messages: Vec<ExpMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpMessage {
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpScene {
    pub content: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub created_at: i64,
    /// 归属事件在 `events` 数组中的下标（None = 未归属）
    #[serde(default)]
    pub event_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpEvent {
    pub summary: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub start_time: i64,
    pub end_time: i64,
    /// 归属弧光在 `arcs` 数组中的下标（None = 未归属）
    #[serde(default)]
    pub arc_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpArc {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub start_time: i64,
    pub end_time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpTimeline {
    pub content: String,
    pub created_at: i64,
}

/// 从 Store 导出全量数据
pub fn export_bundle(store: &Store) -> Result<ExportBundle, String> {
    let characters = store.list_characters().map_err(db_err)?;
    let mut out = Vec::with_capacity(characters.len());
    for c in characters {
        let traits: Vec<ExpTrait> = store
            .list_traits(c.id)
            .map_err(db_err)?
            .into_iter()
            .map(|t| ExpTrait {
                name: t.name,
                description: t.description,
                activity: t.activity,
                dormant: t.dormant,
                last_touched: t.last_touched,
            })
            .collect();

        let mut sessions = Vec::new();
        for s in store.list_sessions(c.id).map_err(db_err)? {
            let messages = store
                .list_messages(s.id)
                .map_err(db_err)?
                .into_iter()
                .map(|m| ExpMessage {
                    role: m.role,
                    content: m.content,
                    created_at: m.created_at,
                })
                .collect();
            sessions.push(ExpSession {
                title: s.title,
                created_at: s.created_at,
                messages,
            });
        }

        // 先导弧光（记录 id→下标），再导事件（记录 id→下标），最后场景引用两者下标
        let arcs = store.list_arcs(c.id).map_err(db_err)?;
        let arc_index: std::collections::HashMap<i64, usize> = arcs
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id, i))
            .collect();
        let arcs: Vec<ExpArc> = arcs
            .into_iter()
            .map(|a| ExpArc {
                title: a.title,
                summary: a.summary,
                keywords: a.keywords,
                start_time: a.start_time,
                end_time: a.end_time,
            })
            .collect();

        let events_rows = store.list_events(c.id).map_err(db_err)?;
        let event_index: std::collections::HashMap<i64, usize> = events_rows
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id, i))
            .collect();
        let events: Vec<ExpEvent> = events_rows
            .into_iter()
            .map(|e| ExpEvent {
                summary: e.summary,
                keywords: e.keywords,
                start_time: e.start_time,
                end_time: e.end_time,
                arc_index: e.arc_id.and_then(|id| arc_index.get(&id).copied()),
            })
            .collect();

        let scenes: Vec<ExpScene> = store
            .list_scenes(c.id)
            .map_err(db_err)?
            .into_iter()
            .map(|s| ExpScene {
                content: s.content,
                keywords: s.keywords,
                created_at: s.created_at,
                event_index: s.event_id.and_then(|id| event_index.get(&id).copied()),
            })
            .collect();

        let timeline_items = timeline::list_entries(store, i64::MAX)
            .map_err(db_err)?
            .into_iter()
            .filter(|t| t.character_id == c.id)
            .map(|t| ExpTimeline {
                content: t.content,
                created_at: t.created_at,
            })
            .collect();

        out.push(ExpCharacter {
            name: c.name,
            title: c.title,
            origin: c.origin,
            faction: c.faction,
            created_at: c.created_at,
            traits,
            sessions,
            scenes,
            events,
            arcs,
            timeline: timeline_items,
        });
    }
    Ok(ExportBundle {
        version: BUNDLE_VERSION,
        exported_at: crate::util::now_ts(),
        characters: out,
    })
}

/// 清空全部数据（角色级联删除其会话 / 消息 / 特质 / 记忆 / 近况）
pub fn clear_all(store: &Store) -> Result<(), String> {
    let ids: Vec<i64> = store
        .list_characters()
        .map_err(db_err)?
        .into_iter()
        .map(|c| c.id)
        .collect();
    for id in ids {
        store.delete_character(id).map_err(db_err)?;
    }
    Ok(())
}

/// 把备份导入 Store：整体替换（先清空再写入），返回中文摘要。
pub fn apply_bundle(store: &Store, bundle: &ExportBundle) -> Result<String, String> {
    if bundle.version != BUNDLE_VERSION {
        return Err(format!(
            "不支持的备份版本 {}（当前支持 {}）",
            bundle.version, BUNDLE_VERSION
        ));
    }
    if bundle.characters.len() > 500 {
        return Err("备份中的角色数量异常，已拒绝导入".to_string());
    }
    clear_all(store)?;

    let mut total_sessions = 0usize;
    let mut total_messages = 0usize;
    for c in &bundle.characters {
        let cid = store
            .insert_character(&c.name, &c.title, &c.origin, &c.faction, c.created_at)
            .map_err(db_err)?;
        for t in &c.traits {
            store
                .upsert_trait(cid, &t.name, &t.description, t.activity, t.dormant, t.last_touched)
                .map_err(db_err)?;
        }
        for s in &c.sessions {
            if s.messages.iter().any(|m| {
                !matches!(m.role.as_str(), "user" | "assistant" | "system")
                    || m.content.len() > 2_000_000
            }) {
                return Err(format!("角色「{}」的会话包含非法消息，已中止导入", c.name));
            }
            let sid = store
                .create_session(cid, &s.title, s.created_at)
                .map_err(db_err)?;
            total_sessions += 1;
            for m in &s.messages {
                store
                    .append_message(sid, &m.role, &m.content, m.created_at)
                    .map_err(db_err)?;
                total_messages += 1;
            }
        }
        // 弧光 → 事件 → 场景（后建者可引用先建者的新 id）
        let mut new_arc_ids: Vec<i64> = Vec::new();
        for a in &c.arcs {
            new_arc_ids.push(
                store
                    .insert_arc(cid, &a.title, &a.summary, &a.keywords, a.start_time, a.end_time)
                    .map_err(db_err)?,
            );
        }
        let mut new_event_ids: Vec<i64> = Vec::new();
        for e in &c.events {
            new_event_ids.push(
                store
                    .insert_event(cid, &e.summary, &e.keywords, e.start_time, e.end_time)
                    .map_err(db_err)?,
            );
        }
        // 事件挂弧光
        for (idx, e) in c.events.iter().enumerate() {
            if let Some(arc_idx) = e.arc_index {
                let (Some(&arc_id), Some(&event_id)) =
                    (new_arc_ids.get(arc_idx), new_event_ids.get(idx))
                else {
                    continue;
                };
                store.attach_events_to_arc(arc_id, &[event_id]).map_err(db_err)?;
            }
        }
        // 场景挂事件
        for s in &c.scenes {
            let scene_id = store
                .insert_scene(cid, &s.content, &s.keywords, s.created_at)
                .map_err(db_err)?;
            if let Some(ev_idx) = s.event_index {
                if let Some(&event_id) = new_event_ids.get(ev_idx) {
                    store
                        .attach_scenes_to_event(event_id, &[scene_id])
                        .map_err(db_err)?;
                }
            }
        }
        for t in &c.timeline {
            timeline::insert_entry(store, cid, &t.content, t.created_at).map_err(db_err)?;
        }
    }
    Ok(format!(
        "导入完成：{} 位角色 / {} 个会话 / {} 条消息",
        bundle.characters.len(),
        total_sessions,
        total_messages
    ))
}

fn db_err(e: rusqlite::Error) -> String {
    format!("数据库错误：{e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一份覆盖全部表结构的样本库
    fn sample_store() -> Store {
        let store = Store::open_in_memory().unwrap();
        timeline::ensure_table(&store).unwrap();
        let cid = store
            .insert_character("玄铃", "守夜人", "北境", "霜环", 1_000)
            .unwrap();
        store
            .upsert_trait(cid, "谨慎", "凡事三思", 0.8, false, 1_000)
            .unwrap();
        let sid = store.create_session(cid, "第一夜", 1_100).unwrap();
        store.append_message(sid, "user", "夜色如何？", 1_101).unwrap();
        store.append_message(sid, "assistant", "风很大，星星很亮。", 1_102).unwrap();

        let s1 = store
            .insert_scene(cid, "星图布局开始", &["星图".into()], 1_200)
            .unwrap();
        let s2 = store
            .insert_scene(cid, "星图布局完成", &["星图".into()], 1_300)
            .unwrap();
        let e1 = store
            .insert_event(cid, "完成星图布局", &["星图".into()], 1_200, 1_300)
            .unwrap();
        store.attach_scenes_to_event(e1, &[s1, s2]).unwrap();
        let a1 = store
            .insert_arc(cid, "星图之弧", "从布局到渲染", &["星图".into()], 1_200, 1_300)
            .unwrap();
        store.attach_events_to_arc(a1, &[e1]).unwrap();
        // 一条不归属任何事件的场景，验证 None 挂接
        store.insert_scene(cid, "独立一幕", &[], 1_400).unwrap();
        timeline::insert_entry(&store, cid, "清晨擦拭星盘。", 1_500).unwrap();
        store
    }

    #[test]
    fn export_then_import_roundtrip() {
        let source = sample_store();
        let bundle = export_bundle(&source).unwrap();
        assert_eq!(bundle.version, BUNDLE_VERSION);
        assert_eq!(bundle.characters.len(), 1);
        let c = &bundle.characters[0];
        assert_eq!(c.name, "玄铃");
        assert_eq!(c.traits.len(), 1);
        assert_eq!(c.sessions[0].messages.len(), 2);
        assert_eq!(c.scenes.len(), 3);
        assert_eq!(c.events.len(), 1);
        assert_eq!(c.arcs.len(), 1);
        assert_eq!(c.timeline.len(), 1);
        // 挂接关系用下标表达
        assert_eq!(c.events[0].arc_index, Some(0));
        assert_eq!(c.scenes[0].event_index, Some(0));
        assert_eq!(c.scenes[2].event_index, None);

        // 导入到全新库（含已有脏数据，应被整体替换）
        let target = Store::open_in_memory().unwrap();
        timeline::ensure_table(&target).unwrap();
        let old = target.insert_character("旧角色", "", "", "", 1).unwrap();
        store_unused(&target, old);
        let report = apply_bundle(&target, &bundle).unwrap();
        assert!(report.contains("1 位角色"));

        // 再导出 → 与原备份内容一致（时间戳字段原样保留）
        let rebundle = export_bundle(&target).unwrap();
        assert_eq!(rebundle.characters, bundle.characters);
    }

    fn store_unused(store: &Store, cid: i64) {
        store.upsert_trait(cid, "待清理", "", 0.5, false, 1).unwrap();
    }

    #[test]
    fn import_rejects_unknown_version_and_bad_roles() {
        let store = Store::open_in_memory().unwrap();
        timeline::ensure_table(&store).unwrap();
        let mut bad_version = ExportBundle {
            version: 99,
            exported_at: 0,
            characters: vec![],
        };
        assert!(apply_bundle(&store, &bad_version).is_err());

        bad_version.version = BUNDLE_VERSION;
        bad_version.characters = vec![ExpCharacter {
            name: "甲".into(),
            title: String::new(),
            origin: String::new(),
            faction: String::new(),
            created_at: 1,
            traits: vec![],
            sessions: vec![ExpSession {
                title: "s".into(),
                created_at: 1,
                messages: vec![ExpMessage {
                    role: "intruder".into(),
                    content: "x".into(),
                    created_at: 1,
                }],
            }],
            scenes: vec![],
            events: vec![],
            arcs: vec![],
            timeline: vec![],
        }];
        let err = apply_bundle(&store, &bad_version).unwrap_err();
        assert!(err.contains("非法消息"));
    }

    #[test]
    fn clear_all_removes_everything() {
        let store = sample_store();
        clear_all(&store).unwrap();
        assert!(store.list_characters().unwrap().is_empty());
        assert!(timeline::list_entries(&store, 10).unwrap().is_empty());
    }

    #[test]
    fn json_serialization_roundtrip() {
        let bundle = export_bundle(&sample_store()).unwrap();
        let json = serde_json::to_string_pretty(&bundle).unwrap();
        let parsed: ExportBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, bundle);
    }
}
