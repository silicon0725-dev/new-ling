//! UI 装配层：把 slint 回调接到后台任务，把后台事件泵回 UI。
//!
//! 所有回调都运行在 slint UI 线程；跨线程状态经由 [`crate::state::AppShared`]
//! 与 `Arc<Mutex<Inner>>`（仅 UI 线程访问）管理。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ling_core::character::{Character, NEW_TRAIT_ACTIVITY};
use ling_core::store::CharacterRow;
use ling_core::world::WorldClock;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use crate::card;
use crate::chat;
use crate::ops;
use crate::settings::{self, ApiSettings};
use crate::state::{AppEvent, AppShared};
use crate::starmap::StarmapCmd;
use crate::timeline;
use crate::util;

slint::include_modules!();

/// UI 线程内部状态（仅被 UI 线程访问）
struct Inner {
    chars: Vec<CharacterRow>,
    current_character: i64,
    current_session: i64,
    pending_card: Option<card::CleanCard>,
    pending_action: Option<PendingAction>,
    starmap_char: i64,
    toast_until: Option<Instant>,
    world_clock: WorldClock,
    world_accum: f64,
    last_tick: Instant,
}

impl Inner {
    fn new() -> Self {
        Self {
            chars: Vec::new(),
            current_character: -1,
            current_session: -1,
            pending_card: None,
            pending_action: None,
            starmap_char: -1,
            toast_until: None,
            world_clock: WorldClock::new(),
            world_accum: 0.0,
            last_tick: Instant::now(),
        }
    }

    fn char_index(&self, id: i64) -> Option<usize> {
        self.chars.iter().position(|c| c.id == id)
    }
}

/// 需要二次确认的操作
enum PendingAction {
    DeleteCharacter(i64),
    ClearData,
    ImportData(String),
}

/// 装配上下文：克隆进各回调与定时器
#[derive(Clone)]
pub struct Ctx {
    ui_weak: Weak<App>,
    shared: Arc<AppShared>,
    runtime: Arc<tokio::runtime::Runtime>,
    starmap_tx: Sender<StarmapCmd>,
    inner: Arc<Mutex<Inner>>,
}

impl Ctx {
    /// 取回 UI 句柄（装配后组件存活于整个事件循环期间）
    fn ui(&self) -> App {
        self.ui_weak
            .upgrade()
            .expect("UI 组件应在事件循环期间存活")
    }
}

/// 装配：注册全部回调与三个定时器，返回需要保活的定时器列表。
#[allow(clippy::too_many_arguments)]
pub fn attach(
    ui: &App,
    shared: Arc<AppShared>,
    runtime: Arc<tokio::runtime::Runtime>,
    starmap_tx: Sender<StarmapCmd>,
    event_rx: std::sync::mpsc::Receiver<AppEvent>,
    frame_rx: std::sync::mpsc::Receiver<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    starmap_enabled: Arc<AtomicBool>,
) -> Vec<slint::Timer> {
    let ctx = Ctx {
        ui_weak: ui.as_weak(),
        shared,
        runtime,
        starmap_tx,
        inner: Arc::new(Mutex::new(Inner::new())),
    };

    register_callbacks(&ctx);
    ctx.bootstrap();

    let mut timers = Vec::new();

    // 事件泵：后台任务 → UI
    let pump = slint::Timer::default();
    {
        let ctx = ctx.clone();
        pump.start(slint::TimerMode::Repeated, Duration::from_millis(40), move || {
            while let Ok(event) = event_rx.try_recv() {
                handle_event(&ctx, event);
            }
        });
    }
    timers.push(pump);

    // 帧泵：星图线程 → Image（只保留最新一帧）
    let frames = slint::Timer::default();
    {
        let weak = ui.as_weak();
        frames.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
            let mut latest = None;
            while let Ok(buf) = frame_rx.try_recv() {
                latest = Some(buf);
            }
            if let (Some(buf), Some(ui)) = (latest, weak.upgrade()) {
                ui.set_starmap_frame(slint::Image::from_rgba8(buf));
            }
        });
    }
    timers.push(frames);

    // 慢速节拍：世界时钟、星图启停、Toast 过期
    let slow = slint::Timer::default();
    {
        let ctx = ctx.clone();
        let enabled = starmap_enabled;
        slow.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
            slow_tick(&ctx, &enabled);
        });
    }
    timers.push(slow);

    timers
}

// ==================== 回调注册 ====================

fn register_callbacks(ctx: &Ctx) {
    let ui_handle = ctx.ui();
    let ui = &ui_handle;

    {
        let ctx = ctx.clone();
        ui.on_goto_settings(move || ctx.ui().set_current_page(4));
    }

    // —— 对话 ——
    {
        let ctx = ctx.clone();
        ui.on_select_chat_character(move |index| ctx.select_chat_character(index as usize));
    }
    {
        let ctx = ctx.clone();
        ui.on_new_session(move || ctx.new_session());
    }
    {
        let ctx = ctx.clone();
        ui.on_select_session(move |index| ctx.select_session(index as usize));
    }
    {
        let ctx = ctx.clone();
        ui.on_send_message(move |text| ctx.send_message(&text));
    }

    // —— 角色 ——
    {
        let ctx = ctx.clone();
        ui.on_new_character(move || ctx.new_character());
    }
    {
        let ctx = ctx.clone();
        ui.on_save_character(move || ctx.save_character());
    }
    {
        let ctx = ctx.clone();
        ui.on_edit_character(move |id| ctx.edit_character(id as i64));
    }
    {
        let ctx = ctx.clone();
        ui.on_delete_character(move |id| ctx.confirm_delete_character(id as i64));
    }
    {
        let ctx = ctx.clone();
        ui.on_start_forge(move || ctx.start_forge());
    }
    {
        let ctx = ctx.clone();
        ui.on_confirm_forge(move || ctx.confirm_forge());
    }

    // —— 领地 ——
    {
        let ctx = ctx.clone();
        ui.on_generate_timeline(move || ctx.generate_timeline());
    }

    // —— 星图 ——
    {
        let ctx = ctx.clone();
        ui.on_starmap_select_character(move |index| {
            ctx.select_starmap_character(index as usize)
        });
    }
    {
        let ctx = ctx.clone();
        ui.on_starmap_reload(move || ctx.reload_starmap());
    }
    {
        let ctx = ctx.clone();
        ui.on_starmap_clicked(move |vw, vh, x, y| {
            let _ = ctx.starmap_tx.send(StarmapCmd::Pick { vw, vh, x, y });
        });
    }

    // —— 设置 ——
    {
        let ctx = ctx.clone();
        ui.on_save_settings(move || ctx.save_settings());
    }
    {
        let ctx = ctx.clone();
        ui.on_test_connection(move || ctx.test_connection());
    }
    {
        let ctx = ctx.clone();
        ui.on_export_data(move || {
            ops::spawn_export(ctx.shared.clone(), settings::data_dir());
        });
    }
    {
        let ctx = ctx.clone();
        ui.on_import_data(move |path| {
            let path = path.to_string();
            ctx.ask_confirm(
                "导入将整体替换现有数据，且不可撤销。确认继续？".to_string(),
                PendingAction::ImportData(path),
            );
        });
    }
    {
        let ctx = ctx.clone();
        ui.on_clear_data(move || {
            ctx.ask_confirm(
                "确认清空全部数据？所有角色、会话、记忆与近况都将被删除（模型配置保留）。"
                    .to_string(),
                PendingAction::ClearData,
            );
        });
    }
    {
        let ctx = ctx.clone();
        ui.on_confirm_accept(move || ctx.confirm_accept());
    }
    {
        let ctx = ctx.clone();
        ui.on_confirm_cancel(move || ctx.confirm_cancel());
    }
}

// ==================== Ctx 行为 ====================

impl Ctx {
    fn toast(&self, msg: impl Into<String>) {
        self.ui().set_toast_text(SharedString::from(msg.into()));
        self.inner.lock().unwrap().toast_until = Some(Instant::now() + Duration::from_secs(4));
    }

    /// 在当前消息模型（VecModel 视图）上执行操作
    fn with_messages<R>(&self, f: impl FnOnce(&VecModel<ChatBubble>) -> R) -> Option<R> {
        self.ui()
            .get_chat_messages()
            .as_any()
            .downcast_ref::<VecModel<ChatBubble>>()
            .map(f)
    }

    /// 在当前会话模型上执行操作
    fn with_sessions<R>(&self, f: impl FnOnce(&VecModel<SessionItem>) -> R) -> Option<R> {
        self.ui()
            .get_chat_sessions()
            .as_any()
            .downcast_ref::<VecModel<SessionItem>>()
            .map(f)
    }

    fn load_character(&self, id: i64) -> Option<Character> {
        let store = self.shared.store.lock().unwrap();
        let row = store.get_character(id).ok().flatten()?;
        let traits = store.list_traits(id).unwrap_or_default();
        Some(chat::assemble_character(
            row.id,
            &row.name,
            &row.title,
            &row.origin,
            &row.faction,
            &traits,
            util::now_ts(),
        ))
    }

    /// 启动时装配初始状态
    fn bootstrap(&self) {
        let dir = settings::data_dir();
        self.ui()
            .set_data_dir_text(SharedString::from(dir.to_string_lossy().into_owned()));
        let config = self.shared.settings_snapshot();
        self.ui().set_settings_base_url(config.base_url.trim().into());
        self.ui().set_settings_model(config.model.trim().into());
        self.ui().set_settings_api_key(config.api_key.trim().into());
        self.ui().set_api_ready(config.is_ready());

        self.refresh_characters();
        self.refresh_timeline();

        // 默认选中第一位角色（对话与星图共用）
        let first = self
            .inner
            .lock()
            .unwrap()
            .chars
            .first()
            .map(|c| c.id)
            .unwrap_or(-1);
        if first >= 0 {
            self.select_chat_character_by_id(first);
            let _ = self.starmap_tx.send(StarmapCmd::Load(first));
        } else {
            self.ui().set_starmap_running(false);
            self.ui().set_starmap_status(
                "尚无角色可装载：先在「角色」页创建或铸造，星图将随之点亮。".into(),
            );
        }
    }

    // —— 刷新 ——

    /// 全量刷新角色列表（角色页 / 对话选择 / 领地入住 / 星图选择共用）
    fn refresh_characters(&self) {
        let now = util::now_ts();
        let (rows, items) = {
            let store = self.shared.store.lock().unwrap();
            let rows = store.list_characters().unwrap_or_default();
            let items: Vec<CharacterItem> = rows
                .iter()
                .map(|row| {
                    let traits = store.list_traits(row.id).unwrap_or_default();
                    let character = chat::assemble_character(
                        row.id,
                        &row.name,
                        &row.title,
                        &row.origin,
                        &row.faction,
                        &traits,
                        now,
                    );
                    to_item(&character)
                })
                .collect();
            (rows, items)
        };
        let options: Vec<SharedString> = rows
            .iter()
            .map(|c| {
                let label = if c.title.is_empty() {
                    c.name.clone()
                } else {
                    format!("{} · {}", c.name, c.title)
                };
                SharedString::from(label)
            })
            .collect();

        self.ui().set_characters(ModelRc::new(VecModel::from(items.clone())));
        self.ui().set_residents(ModelRc::new(VecModel::from(items)));
        self.ui()
            .set_chat_character_options(ModelRc::new(VecModel::from(options.clone())));
        self.ui()
            .set_starmap_character_options(ModelRc::new(VecModel::from(options)));

        {
            let mut inner = self.inner.lock().unwrap();
            inner.chars = rows;
            let chat_idx = inner
                .char_index(inner.current_character)
                .map(|i| i as i32)
                .unwrap_or(-1);
            let sm_idx = inner
                .char_index(inner.starmap_char)
                .map(|i| i as i32)
                .unwrap_or(-1);
            drop(inner);
            self.ui().set_chat_character_index(chat_idx);
            self.ui().set_starmap_character_index(sm_idx);
        }
        self.refresh_chat_header();
    }

    /// 顶部角色条（姓名 / 称号 / 活跃特质）
    fn refresh_chat_header(&self) {
        let id = self.inner.lock().unwrap().current_character;
        if id < 0 {
            self.ui().set_chat_character_name("".into());
            self.ui().set_chat_character_title("".into());
            self.ui().set_chat_trait_line("".into());
            return;
        }
        let Some(character) = self.load_character(id) else {
            return;
        };
        let line: Vec<String> = character
            .active_traits()
            .iter()
            .take(6)
            .map(|t| t.name.clone())
            .collect();
        self.ui()
            .set_chat_character_name(character.identity.name.clone().into());
        self.ui()
            .set_chat_character_title(character.identity.title.clone().into());
        self.ui().set_chat_trait_line(line.join(" · ").into());
    }

    fn refresh_sessions(&self) {
        let id = self.inner.lock().unwrap().current_character;
        if id < 0 {
            self.ui()
                .set_chat_sessions(ModelRc::new(VecModel::<SessionItem>::from(Vec::new())));
            self.inner.lock().unwrap().current_session = -1;
            self.ui().set_chat_session_index(-1);
            self.set_messages(Vec::new());
            return;
        }
        let sessions = {
            let store = self.shared.store.lock().unwrap();
            store.list_sessions(id).unwrap_or_default()
        };
        let now = util::now_ts();
        let items: Vec<SessionItem> = sessions
            .iter()
            .map(|s| SessionItem {
                id: s.id as i32,
                title: SharedString::from(s.title.clone()),
                time: SharedString::from(util::rel_time(s.created_at, now)),
            })
            .collect();
        self.ui().set_chat_sessions(ModelRc::new(VecModel::from(items)));
        // 默认选中最近一个会话
        if !sessions.is_empty() {
            self.select_session(sessions.len() - 1);
        } else {
            self.inner.lock().unwrap().current_session = -1;
            self.ui().set_chat_session_index(-1);
            self.set_messages(Vec::new());
        }
    }

    fn set_messages(&self, bubbles: Vec<ChatBubble>) {
        self.ui()
            .set_chat_messages(ModelRc::new(VecModel::from(bubbles)));
    }

    // —— 对话动作 ——

    fn select_chat_character_by_id(&self, id: i64) {
        if self.inner.lock().unwrap().char_index(id).is_none() {
            return;
        }
        self.inner.lock().unwrap().current_character = id;
        self.refresh_chat_header();
        self.refresh_sessions();
    }

    fn select_chat_character(&self, index: usize) {
        let id = {
            let inner = self.inner.lock().unwrap();
            inner.chars.get(index).map(|c| c.id).unwrap_or(-1)
        };
        self.select_chat_character_by_id(id);
    }

    fn new_session(&self) {
        self.inner.lock().unwrap().current_session = -1;
        self.ui().set_chat_session_index(-1);
        self.set_messages(Vec::new());
    }

    fn select_session(&self, index: usize) {
        let found = {
            let inner = self.inner.lock().unwrap();
            let char_id = inner.current_character;
            if char_id < 0 {
                None
            } else {
                let store = self.shared.store.lock().unwrap();
                store
                    .list_sessions(char_id)
                    .unwrap_or_default()
                    .into_iter()
                    .nth(index)
            }
        };
        let Some(session) = found else {
            return;
        };
        let messages = {
            let store = self.shared.store.lock().unwrap();
            store.list_messages(session.id).unwrap_or_default()
        };
        let bubbles: Vec<ChatBubble> = messages
            .iter()
            .map(|m| ChatBubble {
                is_user: m.role == "user",
                content: SharedString::from(m.content.clone()),
                streaming: false,
                failed: false,
            })
            .collect();
        self.inner.lock().unwrap().current_session = session.id;
        self.ui().set_chat_session_index(index as i32);
        self.set_messages(bubbles);
    }

    fn send_message(&self, text: &str) {
        let text = text.trim().to_string();
        if text.is_empty() || self.ui().get_chat_busy() {
            return;
        }
        let (character_id, mut session_id) = {
            let inner = self.inner.lock().unwrap();
            (inner.current_character, inner.current_session)
        };
        if character_id < 0 {
            self.toast("请先在左侧选择一位角色");
            return;
        }
        if !self.shared.settings_snapshot().is_ready() {
            self.toast("尚未配置模型 API：请前往「设置」页完成配置");
            return;
        }

        let now = util::now_ts();
        // 「新对话」占位 → 落库真正的会话
        if session_id < 0 {
            let title = util::truncate_chars(&text, 16);
            let created = {
                let store = self.shared.store.lock().unwrap();
                store.create_session(character_id, &title, now)
            };
            match created {
                Ok(id) => {
                    session_id = id;
                    let count = self.with_sessions(|vm| {
                        vm.push(SessionItem {
                            id: session_id as i32,
                            title: SharedString::from(title),
                            time: SharedString::from("刚刚"),
                        });
                        vm.row_count()
                    });
                    if let Some(count) = count {
                        self.ui().set_chat_session_index((count - 1) as i32);
                    }
                    self.inner.lock().unwrap().current_session = session_id;
                }
                Err(e) => {
                    self.toast(format!("创建会话失败：{e}"));
                    return;
                }
            }
        }

        // 用户消息入库 + 气泡（用户 + 待填充的助手气泡）
        {
            let store = self.shared.store.lock().unwrap();
            if let Err(e) = store.append_message(session_id, "user", &text, now) {
                self.toast(format!("消息入库失败：{e}"));
                return;
            }
        }
        self.with_messages(|vm| {
            vm.push(ChatBubble {
                is_user: true,
                content: SharedString::from(text.clone()),
                streaming: false,
                failed: false,
            });
            vm.push(ChatBubble {
                is_user: false,
                content: SharedString::from(""),
                streaming: true,
                failed: false,
            });
        });
        self.ui().set_chat_input_text("".into());
        self.ui().set_chat_busy(true);
        chat::spawn(
            &self.runtime,
            self.shared.clone(),
            character_id,
            session_id,
            text,
        );
    }

    // —— 角色动作 ——

    fn new_character(&self) {
        self.ui().set_editing_character_id(-1);
        self.ui().set_editor_name("".into());
        self.ui().set_editor_title("".into());
        self.ui().set_editor_origin("".into());
        self.ui().set_editor_faction("".into());
        self.ui().set_editor_traits("".into());
    }

    fn edit_character(&self, id: i64) {
        let data = {
            let store = self.shared.store.lock().unwrap();
            store.get_character(id).ok().flatten().map(|row| {
                let traits = store
                    .list_traits(id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| (t.name, t.description))
                    .collect::<Vec<_>>();
                (row, traits)
            })
        };
        let Some((row, traits)) = data else {
            return;
        };
        self.ui().set_editing_character_id(id as i32);
        self.ui().set_editor_name(row.name.into());
        self.ui().set_editor_title(row.title.into());
        self.ui().set_editor_origin(row.origin.into());
        self.ui().set_editor_faction(row.faction.into());
        self.ui()
            .set_editor_traits(SharedString::from(util::traits_text(&traits)));
    }

    fn save_character(&self) {
        let name = self.ui().get_editor_name().trim().to_string();
        if name.is_empty() {
            self.toast("角色姓名不能为空");
            return;
        }
        let title = self.ui().get_editor_title().trim().to_string();
        let origin = self.ui().get_editor_origin().trim().to_string();
        let faction = self.ui().get_editor_faction().trim().to_string();
        let traits = util::parse_traits_text(&self.ui().get_editor_traits());

        match self.write_character(&name, &title, &origin, &faction, &traits) {
            Ok(id) => {
                self.ui().set_editing_character_id(id as i32);
                self.toast("角色已保存");
                self.refresh_characters();
                self.reload_starmap();
            }
            Err(msg) => self.toast(msg),
        }
    }

    /// 写入角色档案并同步特质（保留既有活跃度；新特质用初始活跃度；删除列表外特质）
    fn write_character(
        &self,
        name: &str,
        title: &str,
        origin: &str,
        faction: &str,
        traits: &[(String, String)],
    ) -> Result<i64, String> {
        let now = util::now_ts();
        let store = self.shared.store.lock().unwrap();
        let editing = self.ui().get_editing_character_id() as i64;
        let id = if editing >= 0 {
            let updated = store
                .update_character(editing, name, title, origin, faction)
                .map_err(|e| format!("保存失败：{e}"))?;
            if !updated {
                return Err("角色已不存在，可能刚被删除".to_string());
            }
            editing
        } else {
            store
                .insert_character(name, title, origin, faction, now)
                .map_err(|e| format!("创建失败：{e}"))?
        };
        let existing = store.list_traits(id).unwrap_or_default();
        for (t_name, t_desc) in traits {
            let prev = existing
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(t_name));
            let (activity, dormant, touched) = match prev {
                Some(p) => (p.activity, p.dormant, p.last_touched),
                None => (NEW_TRAIT_ACTIVITY as f64, false, now),
            };
            store
                .upsert_trait(id, t_name, t_desc, activity, dormant, touched)
                .map_err(|e| format!("特质写入失败：{e}"))?;
        }
        for t in &existing {
            if !traits.iter().any(|(n, _)| n.eq_ignore_ascii_case(&t.name)) {
                let _ = store.delete_trait(id, &t.name);
            }
        }
        Ok(id)
    }

    fn confirm_delete_character(&self, id: i64) {
        let name = {
            let inner = self.inner.lock().unwrap();
            inner
                .chars
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
        };
        let Some(name) = name else {
            return;
        };
        self.ask_confirm(
            format!("删除角色「{name}」及其全部会话、记忆与近况？该操作不可撤销。"),
            PendingAction::DeleteCharacter(id),
        );
    }

    fn start_forge(&self) {
        let material = self.ui().get_forge_material().trim().to_string();
        if material.is_empty() {
            self.toast("请先粘贴角色设定素材");
            return;
        }
        if !self.shared.settings_snapshot().is_ready() {
            self.toast("尚未配置模型 API：请前往「设置」页完成配置");
            return;
        }
        if self.ui().get_forge_busy() {
            return;
        }
        self.ui().set_forge_busy(true);
        self.ui().set_forge_preview("".into());
        ops::spawn_forge(&self.runtime, self.shared.clone(), material);
    }

    fn confirm_forge(&self) {
        let card = {
            let mut inner = self.inner.lock().unwrap();
            inner.pending_card.take()
        };
        let Some(card) = card else {
            self.toast("请先完成一次成功的铸造");
            return;
        };
        let now = util::now_ts();
        let inserted = {
            let store = self.shared.store.lock().unwrap();
            store
                .insert_character(&card.name, &card.title, &card.origin, &card.faction, now)
                .map(|id| {
                    for (name, desc) in &card.traits {
                        let _ = store.upsert_trait(
                            id,
                            name,
                            desc,
                            NEW_TRAIT_ACTIVITY as f64,
                            false,
                            now,
                        );
                    }
                    id
                })
        };
        match inserted {
            Ok(_) => {
                self.ui().set_forge_material("".into());
                self.ui().set_forge_preview("".into());
                self.toast(format!("角色「{}」已入库", card.name));
                self.refresh_characters();
                self.reload_starmap();
            }
            Err(e) => {
                self.inner.lock().unwrap().pending_card = Some(card);
                self.toast(format!("入库失败：{e}"));
            }
        }
    }

    // —— 领地 ——

    fn generate_timeline(&self) {
        if self.ui().get_timeline_busy() {
            return;
        }
        if !self.shared.settings_snapshot().is_ready() {
            self.toast("尚未配置模型 API：请前往「设置」页完成配置");
            return;
        }
        self.ui().set_timeline_busy(true);
        ops::spawn_timeline(&self.runtime, self.shared.clone());
    }

    fn refresh_timeline(&self) {
        let entries = {
            let store = self.shared.store.lock().unwrap();
            timeline::list_entries(&store, 200).unwrap_or_default()
        };
        let now = util::now_ts();
        let items: Vec<TimelineItem> = entries
            .into_iter()
            .map(|e| TimelineItem {
                id: e.id as i32,
                character_name: SharedString::from(e.character_name),
                content: SharedString::from(e.content),
                time: SharedString::from(util::rel_time(e.created_at, now)),
            })
            .collect();
        self.ui().set_timeline(ModelRc::new(VecModel::from(items)));
    }

    // —— 星图 ——

    fn select_starmap_character(&self, index: usize) {
        let id = {
            let inner = self.inner.lock().unwrap();
            inner.chars.get(index).map(|c| c.id).unwrap_or(-1)
        };
        if id < 0 {
            return;
        }
        self.inner.lock().unwrap().starmap_char = id;
        self.ui().set_starmap_character_index(index as i32);
        let _ = self.starmap_tx.send(StarmapCmd::Load(id));
    }

    fn reload_starmap(&self) {
        let id = {
            let mut inner = self.inner.lock().unwrap();
            if inner.starmap_char < 0 {
                inner.starmap_char = inner.chars.first().map(|c| c.id).unwrap_or(-1);
            }
            inner.starmap_char
        };
        if id >= 0 {
            let _ = self.starmap_tx.send(StarmapCmd::Load(id));
        }
    }

    // —— 设置 ——

    fn save_settings(&self) {
        let config = ApiSettings {
            base_url: self.ui().get_settings_base_url().trim().to_string(),
            api_key: self.ui().get_settings_api_key().trim().to_string(),
            model: self.ui().get_settings_model().trim().to_string(),
        };
        if let Err(msg) = config.quick_validate() {
            self.toast(msg);
            return;
        }
        let path = settings::data_dir().join("settings.json");
        if let Err(e) = config.save(&path) {
            self.toast(format!("配置保存失败：{e}"));
            return;
        }
        let ready = config.is_ready();
        let masked = if config.api_key.is_empty() {
            "未设置".to_string()
        } else {
            settings::mask_key(&config.api_key)
        };
        *self.shared.settings.lock().unwrap() = config;
        self.ui().set_api_ready(ready);
        self.toast(format!("配置已保存（API Key：{masked}，仅保存在本机数据目录）"));
    }

    fn test_connection(&self) {
        if !self.shared.settings_snapshot().is_ready() {
            self.toast("请先填写并保存接口地址 / API Key / 模型");
            return;
        }
        self.ui().set_conn_testing(true);
        self.ui().set_conn_result("正在测试……".into());
        ops::spawn_conn_test(&self.runtime, self.shared.clone());
    }

    // —— 确认对话框 ——

    fn ask_confirm(&self, text: String, action: PendingAction) {
        self.inner.lock().unwrap().pending_action = Some(action);
        self.ui().set_confirm_text(SharedString::from(text));
        self.ui().set_confirm_visible(true);
    }

    fn confirm_cancel(&self) {
        self.inner.lock().unwrap().pending_action = None;
        self.ui().set_confirm_visible(false);
    }

    fn confirm_accept(&self) {
        let action = self.inner.lock().unwrap().pending_action.take();
        self.ui().set_confirm_visible(false);
        match action {
            Some(PendingAction::DeleteCharacter(id)) => {
                let gone = {
                    let store = self.shared.store.lock().unwrap();
                    store.delete_character(id).unwrap_or(false)
                };
                if gone {
                    {
                        let mut inner = self.inner.lock().unwrap();
                        if inner.current_character == id {
                            inner.current_character = -1;
                            inner.current_session = -1;
                        }
                        if inner.starmap_char == id {
                            inner.starmap_char = -1;
                        }
                    }
                    self.toast("角色已删除");
                    self.refresh_characters();
                    self.refresh_sessions();
                    self.refresh_timeline();
                    self.reload_starmap();
                } else {
                    self.toast("角色已不存在");
                }
            }
            Some(PendingAction::ClearData) => {
                ops::spawn_clear(self.shared.clone());
            }
            Some(PendingAction::ImportData(path)) => {
                ops::spawn_import(self.shared.clone(), path);
            }
            None => {}
        }
    }
}

// ==================== 事件泵 ====================

fn handle_event(ctx: &Ctx, event: AppEvent) {
    match event {
        AppEvent::Toast(msg) => ctx.toast(msg),
        AppEvent::ChatDelta { session_id, delta } => {
            let viewing = ctx.inner.lock().unwrap().current_session == session_id;
            if viewing {
                ctx.with_messages(|vm| {
                    let len = vm.row_count();
                    if len > 0 {
                        if let Some(mut bubble) = vm.row_data(len - 1) {
                            if bubble.streaming {
                                let mut text = bubble.content.to_string();
                                text.push_str(&delta);
                                bubble.content = SharedString::from(text);
                                vm.set_row_data(len - 1, bubble);
                            }
                        }
                    }
                });
            }
        }
        AppEvent::ChatFinished { session_id, error } => {
            ctx.ui().set_chat_busy(false);
            let viewing = ctx.inner.lock().unwrap().current_session == session_id;
            if viewing {
                ctx.with_messages(|vm| {
                    let len = vm.row_count();
                    if len > 0 {
                        if let Some(mut bubble) = vm.row_data(len - 1) {
                            match &error {
                                Some(msg) => {
                                    bubble.streaming = false;
                                    bubble.failed = true;
                                    bubble.content = SharedString::from(format!(
                                        "请求失败：{}",
                                        util::truncate_chars(msg, 300)
                                    ));
                                }
                                None => bubble.streaming = false,
                            }
                            vm.set_row_data(len - 1, bubble);
                        }
                    }
                });
            }
            if let Some(msg) = &error {
                ctx.toast(util::truncate_chars(msg, 120));
            }
            // 会话时间 / 特质演化刷新
            ctx.refresh_characters();
        }
        AppEvent::ForgeDone(result) => {
            ctx.ui().set_forge_busy(false);
            match result {
                Ok(forged) => match card::validate_forged(&forged) {
                    Ok(clean) => {
                        let preview = card::preview_text(&clean);
                        ctx.ui().set_forge_preview(SharedString::from(preview));
                        ctx.inner.lock().unwrap().pending_card = Some(clean);
                    }
                    Err(msg) => ctx.toast(msg),
                },
                Err(msg) => {
                    ctx.toast(format!("铸造失败：{}", util::truncate_chars(&msg, 200)));
                }
            }
        }
        AppEvent::TimelineDone { ok, failed } => {
            ctx.ui().set_timeline_busy(false);
            ctx.refresh_timeline();
            if failed == 0 && ok > 0 {
                ctx.toast(format!("已生成 {ok} 条近况"));
            } else if ok > 0 || failed > 0 {
                ctx.toast(format!("近况生成完成：成功 {ok} · 失败 {failed}"));
            }
        }
        AppEvent::ConnTestDone(result) => {
            ctx.ui().set_conn_testing(false);
            match result {
                Ok(msg) => ctx.ui().set_conn_result(SharedString::from(msg)),
                Err(msg) => ctx.ui().set_conn_result(SharedString::from(format!(
                    "连接失败：{}",
                    util::truncate_chars(&msg, 260)
                ))),
            }
        }
        AppEvent::ExportDone(result) => match result {
            Ok(path) => {
                ctx.ui().set_export_path_text(SharedString::from(path.clone()));
                ctx.toast(format!("导出成功：{path}"));
            }
            Err(msg) => ctx.toast(msg),
        },
        AppEvent::ImportDone(result) => {
            let msg = match result {
                Ok(msg) => msg,
                Err(msg) => msg,
            };
            ctx.toast(msg);
            {
                let mut inner = ctx.inner.lock().unwrap();
                inner.current_character = inner.chars.first().map(|c| c.id).unwrap_or(-1);
                inner.current_session = -1;
                inner.starmap_char = inner.current_character;
            }
            ctx.refresh_characters();
            ctx.refresh_sessions();
            ctx.refresh_timeline();
            ctx.reload_starmap();
        }
        AppEvent::ClearDone(result) => match result {
            Ok(()) => {
                ctx.toast("数据已清空（模型配置保留）");
                {
                    let mut inner = ctx.inner.lock().unwrap();
                    inner.current_character = -1;
                    inner.current_session = -1;
                    inner.starmap_char = -1;
                }
                ctx.refresh_characters();
                ctx.refresh_sessions();
                ctx.refresh_timeline();
                ctx.set_messages(Vec::new());
                ctx.ui().set_starmap_running(false);
                ctx.ui().set_starmap_status("尚无角色可装载。".into());
                ctx.ui().set_starmap_detail_title("未选中星点".into());
                ctx.ui()
                    .set_starmap_detail_body("点击星图中的星点，可在此查看详情。".into());
            }
            Err(msg) => ctx.toast(msg),
        },
        AppEvent::StarmapDetail { title, body } => {
            ctx.ui().set_starmap_detail_title(SharedString::from(title));
            ctx.ui().set_starmap_detail_body(SharedString::from(body));
        }
        AppEvent::StarmapStatus { running, text } => {
            ctx.ui().set_starmap_running(running);
            ctx.ui().set_starmap_status(SharedString::from(text));
        }
    }
}

/// 慢速节拍：世界时钟推进、星图启停、Toast 过期
fn slow_tick(ctx: &Ctx, starmap_enabled: &AtomicBool) {
    starmap_enabled.store(ctx.ui().get_current_page() == 3, Ordering::Relaxed);

    let clock_text = {
        let mut inner = ctx.inner.lock().unwrap();
        let now = Instant::now();
        let dt = now.duration_since(inner.last_tick).as_secs_f64();
        inner.last_tick = now;
        inner.world_accum += dt * inner.world_clock.seconds_per_real_second as f64;
        let whole = inner.world_accum.floor();
        if whole >= 1.0 {
            inner.world_clock.advance_real_seconds(whole as i64);
            inner.world_accum -= whole;
        }
        let text = format!(
            "第 {} 世界日 · {} · {}（现实 1 秒 = 世界 72 秒）",
            inner.world_clock.day_index() + 1,
            inner.world_clock.season().chinese_name(),
            inner.world_clock.day_phase().chinese_name(),
        );
        if let Some(deadline) = inner.toast_until {
            if Instant::now() >= deadline {
                inner.toast_until = None;
                ctx.ui().set_toast_text("".into());
            }
        }
        text
    };
    ctx.ui().set_world_clock_text(SharedString::from(clock_text));
}

// ==================== 小工具 ====================

/// 领域角色 → 列表卡片项
fn to_item(character: &Character) -> CharacterItem {
    let traits: Vec<String> = character
        .active_traits()
        .iter()
        .take(6)
        .map(|t| t.name.clone())
        .collect();
    CharacterItem {
        id: character.id as i32,
        name: SharedString::from(character.identity.name.clone()),
        title: SharedString::from(character.identity.title.clone()),
        origin: SharedString::from(character.identity.origin.clone()),
        faction: SharedString::from(character.identity.faction.clone()),
        trait_line: SharedString::from(traits.join(" · ")),
    }
}
