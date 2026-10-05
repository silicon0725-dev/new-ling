//! 各类后台操作：铸造、近况生成、连通性测试、导出 / 导入 / 清空。
//!
//! LLM 调用全部复用 ling-core::ai（自带 SSRF 防护：仅 http/https、
//! 拒绝环回 / 私有 / 保留网段、不跟随重定向）；数据库访问全部走
//! ling-core::store（参数绑定）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ling_core::ai::{ChatMessage, OpenAiClient};

use crate::chat::assemble_character;
use crate::state::{AppEvent, AppShared};
use crate::timeline;
use crate::util;

/// 构造客户端（配置由调用方保证非空）
fn build_client(shared: &AppShared) -> Result<OpenAiClient, String> {
    let s = shared.settings_snapshot();
    OpenAiClient::new(s.base_url.trim(), s.api_key.trim(), s.model.trim())
        .map_err(|e| e.to_string())
}

/// 「铸造」：粘贴素材 → LLM 提炼结构化角色卡
pub fn spawn_forge(runtime: &tokio::runtime::Runtime, shared: Arc<AppShared>, material: String) {
    runtime.spawn(async move {
        let client = match build_client(&shared) {
            Ok(c) => c,
            Err(e) => {
                shared.emit(AppEvent::ForgeDone(Err(e)));
                return;
            }
        };
        let result = client.forge_character(&material).await.map_err(|e| e.to_string());
        shared.emit(AppEvent::ForgeDone(result));
    });
}

/// 「生成近况」：为每位入住角色依人设与最近记忆撰写一条生活记录
pub fn spawn_timeline(runtime: &tokio::runtime::Runtime, shared: Arc<AppShared>) {
    runtime.spawn(async move {
        // —— 先快照全部角色与记忆（短暂持锁）——
        let now = util::now_ts();
        let snapshots: Vec<(ling_core::character::Character, Vec<ling_core::memory::Scene>)> = {
            let store = shared.store.lock().unwrap();
            store
                .list_characters()
                .unwrap_or_default()
                .into_iter()
                .map(|row| {
                    let traits = store.list_traits(row.id).unwrap_or_default();
                    let character = assemble_character(
                        row.id, &row.name, &row.title, &row.origin, &row.faction, &traits, now,
                    );
                    let scenes = store
                        .list_scenes(row.id)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|s| ling_core::memory::Scene {
                            id: s.id,
                            content: s.content,
                            keywords: s.keywords,
                            timestamp: s.created_at,
                        })
                        .collect();
                    (character, scenes)
                })
                .collect()
        };
        if snapshots.is_empty() {
            shared.emit(AppEvent::Toast("领地还没有入住角色".to_string()));
            return;
        }
        let client = match build_client(&shared) {
            Ok(c) => c,
            Err(e) => {
                shared.emit(AppEvent::Toast(e));
                shared.emit(AppEvent::TimelineDone { ok: 0, failed: 0 });
                return;
            }
        };

        let mut ok = 0usize;
        let mut failed = 0usize;
        for (character, scenes) in &snapshots {
            let messages = timeline::timeline_messages(character, scenes);
            match client.chat(&messages).await {
                Ok(text) => {
                    let content = text.trim().to_string();
                    if content.is_empty() {
                        failed += 1;
                        continue;
                    }
                    let store = shared.store.lock().unwrap();
                    if timeline::insert_entry(&store, character.id, &content, util::now_ts())
                        .is_ok()
                    {
                        ok += 1;
                    } else {
                        failed += 1;
                    }
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }
        shared.emit(AppEvent::TimelineDone { ok, failed });
    });
}

/// 连通性测试：发一条极短消息验证 baseURL / apiKey / model 可用
pub fn spawn_conn_test(runtime: &tokio::runtime::Runtime, shared: Arc<AppShared>) {
    runtime.spawn(async move {
        let settings = shared.settings_snapshot();
        let result = match OpenAiClient::new(
            settings.base_url.trim(),
            settings.api_key.trim(),
            settings.model.trim(),
        ) {
            Ok(client) => {
                let probe = client
                    .with_request_timeout(Duration::from_secs(20))
                    .chat(&[ChatMessage::user("请只回复两个字：连通")])
                    .await;
                match probe {
                    Ok(reply) => Ok(format!(
                        "连接成功 · 模型 {} · 返回：{}",
                        settings.model.trim(),
                        util::truncate_chars(reply.trim(), 40)
                    )),
                    Err(e) => Err(e.to_string()),
                }
            }
            Err(e) => Err(e.to_string()),
        };
        shared.emit(AppEvent::ConnTestDone(result));
    });
}

/// 导出全量数据到数据目录（独立线程执行，避免阻塞 UI）
pub fn spawn_export(shared: Arc<AppShared>, dir: PathBuf) {
    std::thread::spawn(move || {
        let result = (|| -> Result<String, String> {
            let bundle = {
                let store = shared.store.lock().unwrap();
                crate::export::export_bundle(&store)?
            };
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建导出目录失败：{e}"))?;
            let path = dir.join(format!("灵-全量备份-{}.json", util::now_ts()));
            let json = serde_json::to_string_pretty(&bundle)
                .map_err(|e| format!("序列化失败：{e}"))?;
            std::fs::write(&path, json).map_err(|e| format!("写入导出文件失败：{e}"))?;
            Ok(path.to_string_lossy().into_owned())
        })();
        shared.emit(AppEvent::ExportDone(result));
    });
}

/// 从指定 JSON 文件导入（整体替换现有数据）
pub fn spawn_import(shared: Arc<AppShared>, path: String) {
    std::thread::spawn(move || {
        let result = (|| -> Result<String, String> {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("读取备份文件失败：{e}"))?;
            let bundle: crate::export::ExportBundle =
                serde_json::from_str(&text).map_err(|e| format!("备份文件解析失败：{e}"))?;
            let store = shared.store.lock().unwrap();
            crate::export::apply_bundle(&store, &bundle)
        })();
        shared.emit(AppEvent::ImportDone(result));
    });
}

/// 清空全部数据（保留模型配置）
pub fn spawn_clear(shared: Arc<AppShared>) {
    std::thread::spawn(move || {
        let result = {
            let store = shared.store.lock().unwrap();
            crate::export::clear_all(&store)
        };
        shared.emit(AppEvent::ClearDone(result));
    });
}
