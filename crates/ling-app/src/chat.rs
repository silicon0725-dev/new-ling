//! 对话后台任务：组装上下文 → LLM 流式增量 → 持久化与特质演化。

use std::sync::Arc;

use futures_util::StreamExt;
use ling_core::ai::{ChatMessage, OpenAiClient};
use ling_core::character::{Character, Identity};
use ling_core::memory::{KeywordRetriever, Scene};
use ling_core::store::{MessageRow, TraitRow};

use crate::prompt;
use crate::state::{AppEvent, AppShared};
use crate::util;

/// 发给 LLM 的历史消息条数上限（过旧的不带，控制上下文长度）
const HISTORY_LIMIT: usize = 24;
/// 单条消息送入 LLM 前的字符截断上限
const MESSAGE_CHAR_CAP: usize = 4000;
/// 场景记忆速记的字符上限
const SCENE_CHAR_CAP: usize = 300;

/// 从仓储行装配 ling-core 角色（应用衰减，得到当前活跃度）
pub fn assemble_character(
    id: i64,
    name: &str,
    title: &str,
    origin: &str,
    faction: &str,
    traits: &[TraitRow],
    now: i64,
) -> Character {
    let mut character = Character::new(id, Identity::new(name, title, origin, faction), now);
    for t in traits {
        let mut ct = ling_core::character::CharacterTrait::new(&t.name, &t.description, t.last_touched);
        ct.activity = t.activity.clamp(0.0, 1.0) as f32;
        ct.dormant = t.dormant;
        character.traits.push(ct);
    }
    character.advance_to(now);
    character
}

/// 组装发送给 LLM 的消息序列：system 提示 + 筛选后的历史 + 本轮用户输入。
/// 历史中的 system 行（不落库，理论不出现）会被跳过；每条按字符截断。
pub fn build_llm_messages(
    system: String,
    history: &[MessageRow],
    user_text: &str,
) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage::system(system)];
    for row in history.iter().rev().take(HISTORY_LIMIT).rev() {
        match row.role.as_str() {
            "user" => messages.push(ChatMessage::user(util::truncate_chars(
                &row.content,
                MESSAGE_CHAR_CAP,
            ))),
            "assistant" => messages.push(ChatMessage::assistant(util::truncate_chars(
                &row.content,
                MESSAGE_CHAR_CAP,
            ))),
            _ => {}
        }
    }
    messages.push(ChatMessage::user(util::truncate_chars(
        user_text,
        MESSAGE_CHAR_CAP,
    )));
    messages
}

/// 一轮对话的场景速记（作为最小记忆单元入库）
pub fn scene_snippet(user_text: &str, assistant_text: &str) -> String {
    util::truncate_chars(
        &format!(
            "用户：{}\n灵：{}",
            util::truncate_chars(user_text.trim(), 120),
            util::truncate_chars(assistant_text.trim(), 180)
        ),
        SCENE_CHAR_CAP,
    )
}

/// 场景记忆的关键词：从用户输入分词去重取前 6 个
pub fn scene_keywords(user_text: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for token in KeywordRetriever::tokenize(user_text) {
        if !seen.contains(&token) {
            seen.push(token);
        }
        if seen.len() >= 6 {
            break;
        }
    }
    seen
}

/// 启动一轮流式对话：增量经事件总线回传 UI，结束后统一持久化。
pub fn spawn(
    runtime: &tokio::runtime::Runtime,
    shared: Arc<AppShared>,
    character_id: i64,
    session_id: i64,
    user_text: String,
) {
    runtime.spawn(async move {
        let settings = shared.settings_snapshot();
        let client = match OpenAiClient::new(
            settings.base_url.trim(),
            settings.api_key.trim(),
            settings.model.trim(),
        ) {
            Ok(client) => client,
            Err(e) => {
                shared.emit(AppEvent::ChatFinished {
                    session_id,
                    error: Some(e.to_string()),
                });
                return;
            }
        };

        // —— 组装上下文（短暂持锁，读取后立即释放）——
        let now = util::now_ts();
        let (character, scenes, history) = {
            let store = shared.store.lock().unwrap();
            let Some(row) = store.get_character(character_id).ok().flatten() else {
                drop(store);
                shared.emit(AppEvent::ChatFinished {
                    session_id,
                    error: Some("角色已不存在".to_string()),
                });
                return;
            };
            let traits = store.list_traits(row.id).unwrap_or_default();
            let character = assemble_character(
                row.id,
                &row.name,
                &row.title,
                &row.origin,
                &row.faction,
                &traits,
                now,
            );
            let scenes: Vec<Scene> = store
                .list_scenes(character.id)
                .unwrap_or_default()
                .into_iter()
                .map(|s| Scene {
                    id: s.id,
                    content: s.content,
                    keywords: s.keywords,
                    timestamp: s.created_at,
                })
                .collect();
            let history = store.list_messages(session_id).unwrap_or_default();
            (character, scenes, history)
        };

        let memories = prompt::recall_memories(&scenes, &user_text, prompt::MEMORY_LIMIT);
        let system = prompt::build_system_prompt(&character, &memories);
        let messages = build_llm_messages(system, &history, &user_text);

        // —— 流式请求 ——
        let mut reply = String::new();
        let stream_result = client.chat_stream(&messages).await;
        let result = match stream_result {
            Ok(mut stream) => {
                let mut outcome: Result<(), String> = Ok(());
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(delta) => {
                            reply.push_str(&delta);
                            shared.emit(AppEvent::ChatDelta {
                                session_id,
                                delta,
                            });
                        }
                        Err(e) => {
                            outcome = Err(e.to_string());
                            break;
                        }
                    }
                }
                outcome
            }
            Err(e) => Err(e.to_string()),
        };

        // —— 结束后的持久化（仅在拿到非空回复时）——
        match result {
            Ok(()) if !reply.trim().is_empty() => {
                let now = util::now_ts();
                let store = shared.store.lock().unwrap();
                if let Err(e) = store.append_message(session_id, "assistant", &reply, now) {
                    shared.emit(AppEvent::ChatFinished {
                        session_id,
                        error: Some(format!("回复入库失败：{e}")),
                    });
                    return;
                }
                let _ = store.insert_scene(
                    character.id,
                    &scene_snippet(&user_text, &reply),
                    &scene_keywords(&user_text),
                    now,
                );
                // 对话中真正被提及的特质被「唤醒/回涌」；把演化后的状态写回
                let combined = format!("{user_text}{reply}").to_lowercase();
                let mentioned: Vec<&str> = character
                    .traits
                    .iter()
                    .filter(|t| combined.contains(&t.name.to_lowercase()))
                    .map(|t| t.name.as_str())
                    .collect();
                let mut evolved = character.clone();
                evolved.mention_traits(&mentioned, now);
                for t in &evolved.traits {
                    let _ = store.upsert_trait(
                        evolved.id,
                        &t.name,
                        &t.description,
                        t.activity as f64,
                        t.dormant,
                        t.last_touched,
                    );
                }
                shared.emit(AppEvent::ChatFinished {
                    session_id,
                    error: None,
                });
            }
            Ok(()) => {
                shared.emit(AppEvent::ChatFinished {
                    session_id,
                    error: Some("模型返回了空回复".to_string()),
                });
            }
            Err(msg) => {
                shared.emit(AppEvent::ChatFinished {
                    session_id,
                    error: Some(msg),
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str, id: i64) -> MessageRow {
        MessageRow {
            id,
            session_id: 1,
            role: role.to_string(),
            content: content.to_string(),
            created_at: id,
        }
    }

    #[test]
    fn llm_messages_order_and_filter() {
        let history = vec![
            msg("user", "你好", 1),
            msg("assistant", "夜里风大。", 2),
            msg("system", "不应出现", 3),
            msg("user", "讲讲星图。", 4),
        ];
        let messages = build_llm_messages("系统提示".to_string(), &history, "最新输入");
        // system 提示 + 过滤后的 3 条历史 + 本轮输入
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[0].content, "系统提示");
        assert_eq!(messages[1].content, "你好");
        assert_eq!(messages[2].content, "夜里风大。");
        assert_eq!(messages[3].content, "讲讲星图。");
        assert_eq!(messages.last().unwrap().role, "user");
        assert_eq!(messages.last().unwrap().content, "最新输入");
        assert!(!messages.iter().any(|m| m.content == "不应出现"));
    }

    #[test]
    fn llm_messages_truncate_long_content() {
        let long = "长".repeat(9000);
        let history = vec![msg("user", &long, 1)];
        let messages = build_llm_messages("s".to_string(), &history, "hi");
        let sent = &messages[1].content;
        assert!(sent.chars().count() <= 4001, "超长内容应被截断");
        assert!(sent.ends_with('…'));
    }

    #[test]
    fn snippet_and_keywords() {
        let snippet = scene_snippet("今晚的星图好看吗", "好看，猎户座正升起。");
        assert!(snippet.contains("用户：今晚的星图好看吗"));
        assert!(snippet.contains("灵：好看"));
        // 关键词来自分词：中文单字 + ASCII 词，且去重
        let kws = scene_keywords("星图 布局 layout 星图");
        assert!(kws.len() <= 6);
        assert!(kws.contains(&"星".to_string()));
        assert!(kws.contains(&"layout".to_string()));
        assert_eq!(kws.iter().filter(|k| *k == "星").count(), 1, "关键词去重");
    }

    #[test]
    fn assemble_character_applies_decay() {
        let traits = vec![TraitRow {
            id: 1,
            character_id: 7,
            name: "谨慎".into(),
            description: String::new(),
            activity: 0.2,
            dormant: false,
            last_touched: 1_000,
        }];
        let now = 1_000 + 30 * 86_400;
        let c = assemble_character(7, "玄铃", "守夜人", "北境", "霜环", &traits, now);
        assert_eq!(c.identity.name, "玄铃");
        assert!(c.traits[0].dormant, "长期未触及且低活跃度应休眠");
    }
}
