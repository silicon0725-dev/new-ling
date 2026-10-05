//! 星图后台线程：按帧驱动 ling-render（步进布局 + 离屏渲染），
//! 把 RGBA 帧转成 `slint::SharedPixelBuffer` 送回 UI 线程成像；
//! 同时承担节点拾取（点击帧坐标 → 最近星点 → 详情面板 + 选中光圈坐标）。
//!
//! 视口映射约定：显示端的 contain / 缩放 / 平移全部在 slint 侧完成，
//! UI 把点击位置反解为**帧像素坐标**后传入；本模块只做帧内拾取，
//! 命中时把星点帧坐标随详情事件回传，slint 侧据此定位选中光圈。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use ling_core::character::Character;
use ling_core::memory::{MemoryArc, MemoryEvent};
use ling_render::geometry::{ViewFit, WorldBounds, padding_px};
use ling_render::visual::node_radius_px;
use ling_render::{StarSnapshot, StarmapRenderer};

use crate::chat::assemble_character;
use crate::state::{AppEvent, AppShared};
use crate::util;

/// 离屏帧尺寸（显示端等比缩放，与拾取数学解耦；ui/app.slint 星图视口按此反解坐标）
pub const FRAME_WIDTH: u32 = 1180;
pub const FRAME_HEIGHT: u32 = 760;

/// UI → 星图线程的命令
pub enum StarmapCmd {
    /// 装载指定角色的星图（节点 = 角色 + 特质 + 弧光 + 事件记忆）
    Load(i64),
    /// 视口内点击拾取（slint 已按缩放 / 平移反解为帧像素坐标）
    Pick { fx: f32, fy: f32 },
}

/// 星点元信息（拾取后展示在详情面板）
#[derive(Debug, Clone)]
struct NodeMeta {
    title: String,
    body: String,
}

/// RGBA8 字节缓冲 → slint 像素缓冲（尺寸不符时返回 None）。
/// 字节序与 slint::Rgba8Pixel 内存布局一致，可直接按字节装载。
fn to_pixel_buffer(
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return None;
    }
    Some(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        rgba, width, height,
    ))
}

/// 从仓储快照构建星图节点元信息（顺序与 StarSnapshot::from_character_memory 一致：
/// 角色 → 特质 → 弧光 → 事件）
fn build_meta(character: &Character, events: &[MemoryEvent], arcs: &[MemoryArc], now: i64) -> Vec<NodeMeta> {
    let mut meta = Vec::with_capacity(1 + character.traits.len() + arcs.len() + events.len());
    let id = &character.identity;
    let title_line = if id.title.is_empty() { "称号：未定" } else { &id.title };
    let origin_line = if id.origin.is_empty() { "出身未述" } else { &id.origin };
    let faction_line = if id.faction.is_empty() { "无阵营" } else { &id.faction };
    meta.push(NodeMeta {
        title: format!("角色 · {}", id.name),
        body: format!(
            "{title_line}\n{origin_line} · {faction_line}\n特质 {} 项 · 弧光 {} 条 · 事件 {} 条",
            character.traits.len(),
            arcs.len(),
            events.len(),
        ),
    });
    for t in &character.traits {
        meta.push(NodeMeta {
            title: format!("特质 · {}", t.name),
            body: format!(
                "{}\n活跃度 {}%{}",
                if t.description.is_empty() { "（待对话中补充）" } else { &t.description },
                (t.activity * 100.0).round() as u32,
                if t.dormant { " · 已休眠" } else { "" },
            ),
        });
    }
    for a in arcs {
        meta.push(NodeMeta {
            title: format!("弧光 · {}", a.title),
            body: format!(
                "{}\n{} → {}",
                a.summary,
                util::rel_time(a.start_time, now),
                util::rel_time(a.end_time, now),
            ),
        });
    }
    for e in events {
        let keywords = if e.keywords.is_empty() {
            "（无）".to_string()
        } else {
            e.keywords.join("、")
        };
        meta.push(NodeMeta {
            title: format!("事件 · {}", util::truncate_chars(&e.summary, 18)),
            body: format!(
                "关键词：{keywords}\n发生于 {}",
                util::rel_time(e.start_time, now),
            ),
        });
    }
    meta
}

/// 启动星图线程。`frame_tx` 为容量 1 的同步通道（UI 消费慢时丢旧帧保最新）。
pub fn spawn(
    shared: Arc<AppShared>,
    cmd_rx: Receiver<StarmapCmd>,
    frame_tx: SyncSender<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    enabled: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("ling-starmap".into())
        .spawn(move || run(shared, cmd_rx, frame_tx, enabled))
        .expect("启动星图线程失败")
}

fn run(
    shared: Arc<AppShared>,
    cmd_rx: Receiver<StarmapCmd>,
    frame_tx: SyncSender<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    enabled: Arc<AtomicBool>,
) {
    let mut renderer: Option<StarmapRenderer> = None;
    // 与渲染器内部一致的规整快照副本（节点类别 / 权重，用于拾取半径）
    let mut clean: Option<StarSnapshot> = None;
    let mut meta: Vec<NodeMeta> = Vec::new();
    let frame_duration = Duration::from_millis(33);

    loop {
        // —— 处理命令 ——
        loop {
            match cmd_rx.try_recv() {
                Ok(StarmapCmd::Load(character_id)) => {
                    handle_load(&shared, &mut renderer, &mut clean, &mut meta, character_id);
                }
                Ok(StarmapCmd::Pick { fx, fy }) => {
                    if let (Some(r), Some(snap)) = (&renderer, &clean) {
                        match pick(r, snap, &meta, fx, fy) {
                            Some((title, body, px, py)) => {
                                shared.emit(AppEvent::StarmapDetail { title, body, px, py });
                            }
                            // 点击空白：发「未选中」事件，UI 复位详情面板并隐藏光圈
                            None => {
                                shared.emit(AppEvent::StarmapDetail {
                                    title: String::new(),
                                    body: String::new(),
                                    px: -1.0,
                                    py: -1.0,
                                });
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }

        // —— 帧驱动 ——
        let active = enabled.load(Ordering::Relaxed);
        if active {
            if let Some(r) = renderer.as_mut() {
                if r.node_count() > 0 {
                    r.step_and_draw();
                    if let Ok(rgba) = r.read_pixels_rgba8() {
                        if let Some(buffer) = to_pixel_buffer(&rgba, r.width(), r.height()) {
                            // 容量 1：UI 消费慢时丢弃本帧（下一帧很快到来）
                            let _ = frame_tx.try_send(buffer);
                        }
                    }
                }
            }
            std::thread::sleep(frame_duration);
        } else {
            std::thread::sleep(Duration::from_millis(120));
        }
    }
}

/// 装载（或重载）角色星图；GPU 不可用时发送友好状态而不崩溃。
fn handle_load(
    shared: &AppShared,
    renderer: &mut Option<StarmapRenderer>,
    clean: &mut Option<StarSnapshot>,
    meta: &mut Vec<NodeMeta>,
    character_id: i64,
) {
    let now = util::now_ts();
    // —— 快照领域数据 ——
    let loaded = {
        let store = shared.store.lock().unwrap();
        let Some(row) = store.get_character(character_id).ok().flatten() else {
            shared.emit(AppEvent::StarmapStatus {
                running: false,
                text: "该角色已不存在，星图未装载。".to_string(),
                stats: String::new(),
            });
            return;
        };
        let traits = store.list_traits(row.id).unwrap_or_default();
        let character =
            assemble_character(row.id, &row.name, &row.title, &row.origin, &row.faction, &traits, now);

        let events: Vec<MemoryEvent> = store
            .list_events(row.id)
            .unwrap_or_default()
            .into_iter()
            .map(|e| MemoryEvent {
                id: e.id,
                summary: e.summary,
                keywords: e.keywords,
                scene_ids: Vec::new(),
                start_time: e.start_time,
                end_time: e.end_time,
            })
            .collect();
        let arcs: Vec<MemoryArc> = store
            .list_arcs(row.id)
            .unwrap_or_default()
            .into_iter()
            .map(|a| {
                let event_ids = store
                    .events_of_arc(a.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|e| e.id)
                    .collect();
                MemoryArc {
                    id: a.id,
                    title: a.title,
                    summary: a.summary,
                    keywords: a.keywords,
                    event_ids,
                    start_time: a.start_time,
                    end_time: a.end_time,
                }
            })
            .collect();
        (character, events, arcs)
    };
    let (character, events, arcs) = loaded;
    *meta = build_meta(&character, &events, &arcs, now);
    let snapshot = StarSnapshot::from_character_memory(&character, &events, &arcs);
    let sanitized = snapshot.clone().sanitized();

    // —— 惰性创建渲染器 ——
    if renderer.is_none() {
        match StarmapRenderer::new(FRAME_WIDTH, FRAME_HEIGHT) {
            Ok(r) => *renderer = Some(r),
            Err(e) => {
                shared.emit(AppEvent::StarmapStatus {
                    running: false,
                    text: format!("无法初始化 GPU 渲染器（{e}）：星图页暂不可用，其余功能不受影响。"),
                    stats: String::new(),
                });
                return;
            }
        }
    }
    if let Some(r) = renderer.as_mut() {
        r.load_snapshot(snapshot);
        *clean = Some(sanitized);
        let stats = format!("{} 节点 · {} 连线", r.node_count(), r.edge_count());
        shared.emit(AppEvent::StarmapStatus {
            running: true,
            text: format!("星图已装载：{stats}"),
            stats,
        });
    }
}

/// 拾取：帧像素坐标 → 最近星点（含可点击半径余量）→ 元信息 + 星点帧坐标
fn pick(
    renderer: &StarmapRenderer,
    snapshot: &StarSnapshot,
    meta: &[NodeMeta],
    px: f32,
    py: f32,
) -> Option<(String, String, f32, f32)> {
    let stars = &renderer.simulation().stars;
    if stars.is_empty() {
        return None;
    }
    // 复刻渲染器每帧的视口映射（分位数包围盒 + padding）
    let viewport = [FRAME_WIDTH as f32, FRAME_HEIGHT as f32];
    let min_dim = viewport[0].min(viewport[1]);
    let bounds = WorldBounds::of_positions_trimmed(
        stars.iter().map(|s| (s.x as f32, s.y as f32)),
        0.10,
    );
    let fit = ViewFit::compute(bounds, viewport, padding_px(min_dim));

    let mut best: Option<(f32, usize)> = None;
    for (i, star) in stars.iter().enumerate() {
        let [nx, ny] = fit.world_to_pixel(star.x as f32, star.y as f32);
        let dist = ((nx - px).powi(2) + (ny - py).powi(2)).sqrt();
        let node = snapshot.nodes.get(i);
        let radius = node
            .map(|n| node_radius_px(n.kind, n.weight, min_dim))
            .unwrap_or(6.0)
            + 8.0;
        if dist <= radius && best.map(|(d, _)| dist < d).unwrap_or(true) {
            best = Some((dist, i));
        }
    }
    let idx = best?.1;
    let m = meta.get(idx).cloned().unwrap_or(NodeMeta {
        title: "未知星点".to_string(),
        body: "该星点暂无关联信息。".to_string(),
    });
    // 命中星点的屏幕（帧）坐标，供 UI 侧定位选中光圈
    let [nx, ny] = fit.world_to_pixel(stars[idx].x as f32, stars[idx].y as f32);
    Some((m.title, m.body, nx, ny))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_constants_match_ui_viewport() {
        // ui/app.slint 的星图视口按这两个常量做坐标反解，改动必须两侧同步
        assert_eq!(FRAME_WIDTH, 1180);
        assert_eq!(FRAME_HEIGHT, 760);
    }

    #[test]
    fn pixel_buffer_rejects_mismatched_input() {
        assert!(to_pixel_buffer(&[0; 16], 2, 2).is_some());
        assert!(to_pixel_buffer(&[0; 15], 2, 2).is_none());
    }
}
