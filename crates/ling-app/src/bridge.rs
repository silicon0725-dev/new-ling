//! UI 装配层：把 slint 回调接到后台任务，把后台事件泵回 UI。
//!
//! 所有回调都运行在 slint UI 线程；跨线程状态经由 [`crate::state::AppShared`]
//! 与 `Arc<Mutex<Inner>>`（仅 UI 线程访问）管理。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ling_core::character::{Character, NEW_TRAIT_ACTIVITY};
use ling_core::store::CharacterRow;
use ling_core::world::WorldClock;
use rusqlite::params;
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

/// 编辑器「已装载快照」：用于未保存变更检测（P2-6）
#[derive(Debug, Clone, Default, PartialEq)]
struct EditorSnapshot {
    name: String,
    title: String,
    origin: String,
    faction: String,
    traits: String,
}

/// UI 线程内部状态（仅被 UI 线程访问）
struct Inner {
    chars: Vec<CharacterRow>,
    current_character: i64,
    current_session: i64,
    /// 「新对话」待落库态（P1-8）：会话列表置顶草稿项，首条消息落库后替换
    session_pending: bool,
    pending_card: Option<card::CleanCard>,
    pending_action: Option<PendingAction>,
    starmap_char: i64,
    world_clock: WorldClock,
    world_accum: f64,
    last_tick: Instant,
    // —— Toast 简单队列（P2-8）：当前条 + 等待条 ——
    toast_until: Option<Instant>,
    toast_queue: VecDeque<(String, bool)>,
    // —— 慢速节拍计数：驱动呼吸相位（每 7 tick = 1400ms 翻转）与会话时间刷新 ——
    tick: u32,
    // —— 编辑器未保存保护（P2-6）——
    editor_loaded: EditorSnapshot,
}

impl Inner {
    fn new() -> Self {
        Self {
            chars: Vec::new(),
            current_character: -1,
            current_session: -1,
            session_pending: false,
            pending_card: None,
            pending_action: None,
            starmap_char: -1,
            world_clock: WorldClock::new(),
            world_accum: 0.0,
            last_tick: Instant::now(),
            toast_until: None,
            toast_queue: VecDeque::new(),
            tick: 0,
            editor_loaded: EditorSnapshot::default(),
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
    DeleteSession(i64),
    /// 编辑器存在未保存修改，确认后执行切换（New = 新建，Edit = 编辑该角色，Reset = 清空字段）
    EditorSwitch(EditorSwitch),
}

enum EditorSwitch {
    New,
    Edit(i64),
    Reset,
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

    // 帧泵：星图线程 → Image（只保留最新一帧；首帧用于解除遮罩）
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
                ui.set_starmap_has_frame(true);
            }
        });
    }
    timers.push(frames);

    // 慢速节拍：世界时钟、星图启停、Toast 过期、呼吸相位、会话时间刷新
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
        ui.on_select_session(move |id| ctx.select_session(id));
    }
    {
        let ctx = ctx.clone();
        ui.on_delete_session(move |id| ctx.confirm_delete_session(id));
    }
    {
        let ctx = ctx.clone();
        ui.on_send_message(move |text| ctx.send_message(&text));
    }
    {
        let ctx = ctx.clone();
        ui.on_stop_generation(move || ctx.stop_generation());
    }
    {
        let ctx = ctx.clone();
        ui.on_retry_message(move |index| ctx.retry_message(index as usize));
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
        ui.on_reset_editor_form(move || ctx.reset_editor_form());
    }
    {
        let ctx = ctx.clone();
        ui.on_update_traits_hint(move |text| {
            let (hint, bad) = util::traits_hint(&text);
            let ui = ctx.ui();
            ui.set_editor_traits_hint(SharedString::from(hint));
            ui.set_editor_traits_hint_bad(bad);
        });
    }
    {
        let ctx = ctx.clone();
        ui.on_start_forge(move || ctx.start_forge());
    }
    {
        let ctx = ctx.clone();
        ui.on_confirm_forge(move || ctx.confirm_forge());
    }
    {
        let ctx = ctx.clone();
        ui.on_forge_to_editor(move || ctx.forge_to_editor());
    }
    {
        let ctx = ctx.clone();
        ui.on_dismiss_forge_error(move || ctx.ui().set_forge_error("".into()));
    }

    // —— 领地 ——
    {
        let ctx = ctx.clone();
        ui.on_generate_timeline(move || ctx.generate_timeline());
    }
    {
        let ctx = ctx.clone();
        ui.on_open_character_chat(move |id| ctx.open_character_chat(id));
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
        ui.on_starmap_clicked(move |fx, fy| {
            let _ = ctx.starmap_tx.send(StarmapCmd::Pick { fx, fy });
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
            ctx.ui().set_data_op_busy(true);
            ops::spawn_export(ctx.shared.clone(), settings::data_dir());
        });
    }
    {
        let ctx = ctx.clone();
        ui.on_import_data(move |path| {
            let path = path.to_string();
            ctx.ask_confirm(
                "导入数据".to_string(),
                "导入将整体替换现有数据，且不可撤销。确认继续？".to_string(),
                PendingAction::ImportData(path),
            );
        });
    }
    {
        let ctx = ctx.clone();
        ui.on_clear_data(move || {
            ctx.ask_confirm(
                "清空数据".to_string(),
                "确认清空全部数据？所有角色、会话、记忆与近况都将被删除（模型配置保留）。".to_string(),
                PendingAction::ClearData,
            );
        });
    }
    {
        let ctx = ctx.clone();
        ui.on_copy_export_path(move || ctx.copy_export_path());
    }
    {
        let ctx = ctx.clone();
        ui.on_confirm_accept(move || ctx.confirm_accept());
    }
    {
        let ctx = ctx.clone();
        ui.on_confirm_cancel(move || ctx.confirm_cancel());
    }
    {
        let ctx = ctx.clone();
        ui.on_dismiss_toast(move || ctx.advance_toast());
    }
}

// ==================== Ctx 行为 ====================

impl Ctx {
    /// 普通提示 Toast（4 秒；排队展示）
    fn toast(&self, msg: impl Into<String>) {
        self.enqueue_toast(msg.into(), false, Duration::from_secs(4));
    }

    /// 错误类 Toast（6 秒驻留 + 红点；排队展示）
    fn toast_error(&self, msg: impl Into<String>) {
        self.enqueue_toast(msg.into(), true, Duration::from_secs(6));
    }

    fn enqueue_toast(&self, msg: String, is_error: bool, ttl: Duration) {
        let mut inner = self.inner.lock().unwrap();
        if inner.toast_until.is_some() {
            inner.toast_queue.push_back((msg, is_error));
            return;
        }
        inner.toast_until = Some(Instant::now() + ttl);
        drop(inner);
        let ui = self.ui();
        ui.set_toast_text(SharedString::from(msg));
        ui.set_toast_is_error(is_error);
    }

    /// Toast 到期 / 被点击关闭：清空当前条并立即展示下一条（如有）
    fn advance_toast(&self) {
        let next = {
            let mut inner = self.inner.lock().unwrap();
            inner.toast_until = None;
            inner.toast_queue.pop_front()
        };
        let ui = self.ui();
        match next {
            Some((msg, is_error)) => {
                let mut inner = self.inner.lock().unwrap();
                inner.toast_until = Some(Instant::now() + Duration::from_secs(4));
                drop(inner);
                ui.set_toast_text(SharedString::from(msg));
                ui.set_toast_is_error(is_error);
            }
            None => {
                ui.set_toast_text("".into());
                ui.set_toast_is_error(false);
            }
        }
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
        let ui = self.ui();
        ui.set_settings_base_url(config.base_url.trim().into());
        ui.set_settings_model(config.model.trim().into());
        ui.set_settings_api_key(config.api_key.trim().into());
        ui.set_api_ready(config.is_ready());
        drop(ui);

        self.refresh_characters();
        self.refresh_timeline();
        self.refresh_traits_hint();

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
            self.send_starmap_load(first);
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

        let ui = self.ui();
        ui.set_characters(ModelRc::new(VecModel::from(items.clone())));
        ui.set_residents(ModelRc::new(VecModel::from(items)));
        ui.set_chat_character_options(ModelRc::new(VecModel::from(options.clone())));
        ui.set_starmap_character_options(ModelRc::new(VecModel::from(options)));

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
            ui.set_chat_character_index(chat_idx);
            ui.set_starmap_character_index(sm_idx);
        }
        self.refresh_chat_header();
    }

    /// 顶部角色条（姓名 / 称号 / 活跃特质）
    fn refresh_chat_header(&self) {
        let id = self.inner.lock().unwrap().current_character;
        if id < 0 {
            let ui = self.ui();
            ui.set_chat_character_name("".into());
            ui.set_chat_character_title("".into());
            ui.set_chat_trait_line("".into());
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
        let ui = self.ui();
        ui.set_chat_character_name(character.identity.name.clone().into());
        ui.set_chat_character_title(character.identity.title.clone().into());
        ui.set_chat_trait_line(line.join(" · ").into());
    }

    /// 重建会话列表模型并恢复选择：
    /// - 待落库态（session_pending）→ 「新对话」草稿项置顶并高亮（P1-8，切换角色也不丢）；
    /// - 已选会话仍存在 → 原位选中；否则回退到最近一个会话。
    fn refresh_sessions(&self) {
        let (char_id, current, pending) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.current_character,
                inner.current_session,
                inner.session_pending,
            )
        };
        if char_id < 0 {
            self.ui()
                .set_chat_sessions(ModelRc::new(VecModel::<SessionItem>::from(Vec::new())));
            self.inner.lock().unwrap().current_session = -1;
            let ui = self.ui();
            ui.set_chat_session_index(-1);
            drop(ui);
            self.set_messages(Vec::new());
            return;
        }
        let sessions = {
            let store = self.shared.store.lock().unwrap();
            store.list_sessions(char_id).unwrap_or_default()
        };
        let now = util::now_ts();
        let mut items: Vec<SessionItem> = sessions
            .iter()
            .map(|s| SessionItem {
                id: s.id as i32,
                title: SharedString::from(s.title.clone()),
                time: SharedString::from(util::rel_time(s.created_at, now)),
            })
            .collect();

        let ui = self.ui();
        if pending {
            // 「新对话」待落库：占位项置顶并高亮，消息清空
            items.insert(
                0,
                SessionItem {
                    id: -1,
                    title: SharedString::from("新对话"),
                    time: SharedString::from("—"),
                },
            );
            self.inner.lock().unwrap().current_session = -1;
            ui.set_chat_session_index(0);
            drop(ui);
            self.set_messages(Vec::new());
            self.ui()
                .set_chat_sessions(ModelRc::new(VecModel::from(items)));
            return;
        }

        // 选择保持：原会话仍存在则原位选中，否则回退最近一个
        let pick: Option<i64> = match current {
            id if id >= 0 => sessions
                .iter()
                .map(|s| s.id)
                .find(|&sid| sid == current)
                .or_else(|| sessions.last().map(|s| s.id)),
            _ => sessions.last().map(|s| s.id),
        };
        match pick {
            Some(sid) => {
                self.inner.lock().unwrap().current_session = sid;
                let pos = sessions.iter().position(|s| s.id == sid).unwrap_or(0);
                ui.set_chat_session_index(pos as i32);
                drop(ui);
                self.ui()
                    .set_chat_sessions(ModelRc::new(VecModel::from(items)));
                self.reload_session_messages(sid);
            }
            None => {
                self.inner.lock().unwrap().current_session = -1;
                ui.set_chat_session_index(-1);
                drop(ui);
                self.set_messages(Vec::new());
                self.ui()
                    .set_chat_sessions(ModelRc::new(VecModel::from(items)));
            }
        }
    }

    /// 会话时间行随慢速节拍定期重算（P2-5；不触碰选择与占位项）
    fn refresh_session_times(&self) {
        let char_id = self.inner.lock().unwrap().current_character;
        if char_id < 0 {
            return;
        }
        let sessions = {
            let store = self.shared.store.lock().unwrap();
            store.list_sessions(char_id).unwrap_or_default()
        };
        let now = util::now_ts();
        self.with_sessions(|vm| {
            for row in 0..vm.row_count() {
                let Some(item) = vm.row_data(row) else { continue };
                if item.id < 0 {
                    continue;
                }
                let Some(sess) = sessions.iter().find(|s| s.id as i32 == item.id) else {
                    continue;
                };
                let fresh = util::rel_time(sess.created_at, now);
                if item.time.as_str() != fresh {
                    let mut updated = item;
                    updated.time = SharedString::from(fresh);
                    vm.set_row_data(row, updated);
                }
            }
        });
    }

    /// 从 DB 重载指定会话的消息（整体替换模型；P0-2 的对账通道）
    fn reload_session_messages(&self, session_id: i64) {
        let messages = {
            let store = self.shared.store.lock().unwrap();
            store.list_messages(session_id).unwrap_or_default()
        };
        let now = util::now_ts();
        let bubbles: Vec<ChatBubble> = messages
            .iter()
            .map(|m| ChatBubble {
                is_user: m.role == "user",
                content: SharedString::from(m.content.clone()),
                streaming: false,
                failed: false,
                time: SharedString::from(util::rel_time(m.created_at, now)),
            })
            .collect();
        self.set_messages(bubbles);
    }

    fn set_messages(&self, bubbles: Vec<ChatBubble>) {
        self.ui()
            .set_chat_messages(ModelRc::new(VecModel::from(bubbles)));
    }

    /// 特质提示行（bootstrap 时初始化一次，此后由 changed 回调驱动）
    fn refresh_traits_hint(&self) {
        let (hint, bad) = util::traits_hint(&self.ui().get_editor_traits());
        let ui = self.ui();
        ui.set_editor_traits_hint(SharedString::from(hint));
        ui.set_editor_traits_hint_bad(bad);
    }

    // —— 对话动作 ——

    fn select_chat_character_by_id(&self, id: i64) {
        if self.inner.lock().unwrap().char_index(id).is_none() {
            return;
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.current_character = id;
            // 换角色：回到该角色的最近会话（待落库草稿不跨角色保留）
            inner.current_session = -1;
            inner.session_pending = false;
        }
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

    /// 新对话：进入「待落库」态（列表置顶「新对话 · 草稿」项）
    fn new_session(&self) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.current_session = -1;
            inner.session_pending = true;
        }
        self.refresh_sessions();
    }

    /// 选中会话（参数为会话 id；-1 = 「新对话」草稿）
    fn select_session(&self, id: i32) {
        if id == -1 {
            self.new_session();
            return;
        }
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
                    .find(|s| s.id as i32 == id)
            }
        };
        let Some(session) = found else {
            return;
        };
        {
            let mut inner = self.inner.lock().unwrap();
            inner.current_session = session.id;
            inner.session_pending = false;
        }
        self.reload_session_messages(session.id);
        // 定位模型行（可能存在草稿项，故按 id 查找）
        let row = self.with_sessions(|vm| {
            (0..vm.row_count()).find(|&r| {
                vm.row_data(r)
                    .map(|it| it.id == id)
                    .unwrap_or(false)
            })
        });
        if let Some(row) = row.flatten() {
            self.ui().set_chat_session_index(row as i32);
        }
    }

    fn confirm_delete_session(&self, id: i32) {
        self.ask_confirm(
            "删除会话".to_string(),
            "删除该会话及其全部消息？该操作不可撤销。".to_string(),
            PendingAction::DeleteSession(id as i64),
        );
    }

    /// 删除会话（messages 随外键级联删除；参数绑定 SQL，见 timeline.rs 同款做法）
    fn delete_session(&self, id: i64) {
        let deleted = {
            let store = self.shared.store.lock().unwrap();
            store
                .conn()
                .execute("DELETE FROM sessions WHERE id = ?1", params![id])
                .map(|n| n > 0)
                .unwrap_or(false)
        };
        if deleted {
            {
                let mut inner = self.inner.lock().unwrap();
                if inner.current_session == id {
                    // 删除的是当前会话：回到「新对话」草稿态
                    inner.current_session = -1;
                    inner.session_pending = true;
                }
            }
            self.refresh_sessions();
            self.toast("会话已删除");
        } else {
            self.toast("会话已不存在");
        }
    }

    fn send_message(&self, text: &str) {
        let text = text.trim().to_string();
        if text.is_empty() || self.ui().get_chat_busy() {
            return;
        }
        let (character_id, mut session_id, pending) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.current_character,
                inner.current_session,
                inner.session_pending,
            )
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
        // 「新对话」草稿 → 落库真正的会话，并替换草稿项（P1-8）
        if pending || session_id < 0 {
            let title = util::truncate_chars(&text, 16);
            let created = {
                let store = self.shared.store.lock().unwrap();
                store.create_session(character_id, &title, now)
            };
            match created {
                Ok(id) => {
                    session_id = id;
                    let count = self.with_sessions(|vm| {
                        // 移除可能存在的草稿项（id == -1）
                        if let Some(pos) = (0..vm.row_count()).find(|&r| {
                            vm.row_data(r).map(|it| it.id == -1).unwrap_or(false)
                        }) {
                            vm.remove(pos);
                        }
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
                    let mut inner = self.inner.lock().unwrap();
                    inner.current_session = session_id;
                    inner.session_pending = false;
                }
                Err(e) => {
                    self.toast_error(format!("创建会话失败：{e}"));
                    return;
                }
            }
        }

        // 用户消息入库 + 气泡（用户 + 待填充的助手气泡）
        {
            let store = self.shared.store.lock().unwrap();
            if let Err(e) = store.append_message(session_id, "user", &text, now) {
                self.toast_error(format!("消息入库失败：{e}"));
                return;
            }
        }
        self.with_messages(|vm| {
            vm.push(ChatBubble {
                is_user: true,
                content: SharedString::from(text.clone()),
                streaming: false,
                failed: false,
                time: SharedString::from("刚刚"),
            });
            vm.push(ChatBubble {
                is_user: false,
                content: SharedString::from(""),
                streaming: true,
                failed: false,
                time: SharedString::from(""),
            });
        });
        let ui = self.ui();
        ui.set_chat_input_text("".into());
        ui.set_chat_busy(true);
        ui.set_chat_stick_bottom(true); // 用户主动发送：贴底跟随新内容
        drop(ui);
        // 新一轮请求：清除上一轮可能残留的中止旗标
        self.shared.chat_cancel.store(false, Ordering::Relaxed);
        chat::spawn(
            &self.runtime,
            self.shared.clone(),
            character_id,
            session_id,
            text,
        );
    }

    /// 停止流式生成：置中止旗标，流式循环在下一个增量处退出并保留已生成部分
    fn stop_generation(&self) {
        if self.ui().get_chat_busy() {
            self.shared.chat_cancel.store(true, Ordering::Relaxed);
        }
    }

    /// 重试失败的轮次：删除失败轮次的用户消息（连同错误气泡）后原样重发，不重复入库
    fn retry_message(&self, index: usize) {
        if self.ui().get_chat_busy() {
            return;
        }
        let session_id = {
            let inner = self.inner.lock().unwrap();
            if inner.session_pending {
                -1
            } else {
                inner.current_session
            }
        };
        if session_id < 0 {
            return;
        }
        // 从错误气泡向前找最近的用户气泡
        let found = self.with_messages(|vm| {
            let mut user_text: Option<String> = None;
            let mut cut_from: Option<usize> = None;
            for row in (0..=index.min(vm.row_count().saturating_sub(1))).rev() {
                let Some(item) = vm.row_data(row) else { continue };
                if item.is_user {
                    user_text = Some(item.content.to_string());
                    cut_from = Some(row);
                    break;
                }
            }
            user_text.zip(cut_from)
        });
        let Some((user_text, cut_from)) = found.flatten() else {
            return;
        };

        // 从 DB 删除该条用户消息（按「当前会话最后一条 user 消息」定位，内容须一致；
        // 数据与视图不一致时不删除，宁可少删不可误删）
        {
            let store = self.shared.store.lock().unwrap();
            let last_user: Option<(i64, String)> = store
                .conn()
                .query_row(
                    "SELECT id, content FROM messages
                     WHERE session_id = ?1 AND role = 'user'
                     ORDER BY id DESC LIMIT 1",
                    params![session_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .ok();
            if let Some((mid, content)) = last_user {
                if content == user_text {
                    let _ = store
                        .conn()
                        .execute("DELETE FROM messages WHERE id = ?1", params![mid]);
                }
            }
        }

        // 视图：截掉失败轮次（用户气泡 + 错误气泡）
        self.with_messages(|vm| {
            while vm.row_count() > cut_from {
                vm.remove(vm.row_count() - 1);
            }
        });
        self.send_message(&user_text);
    }

    // —— 角色动作 ——

    /// 编辑器当前字段快照（用于未保存变更检测）
    fn editor_current(&self) -> EditorSnapshot {
        let ui = self.ui();
        EditorSnapshot {
            name: ui.get_editor_name().trim().to_string(),
            title: ui.get_editor_title().trim().to_string(),
            origin: ui.get_editor_origin().trim().to_string(),
            faction: ui.get_editor_faction().trim().to_string(),
            traits: ui.get_editor_traits().trim().to_string(),
        }
    }

    /// 未保存变更守卫：有差异时弹确认框，确认后执行目标动作（P2-6）
    fn guarded_editor_switch(&self, switch: EditorSwitch) {
        if self.editor_current() == self.inner.lock().unwrap().editor_loaded {
            self.apply_editor_switch(&switch);
            return;
        }
        self.ask_confirm(
            "放弃未保存的修改".to_string(),
            "编辑器中存在尚未保存的修改，切换后将丢失。确认放弃？".to_string(),
            PendingAction::EditorSwitch(switch),
        );
    }

    fn apply_editor_switch(&self, switch: &EditorSwitch) {
        match switch {
            EditorSwitch::New => self.load_editor(None),
            EditorSwitch::Edit(id) => self.load_editor(Some(*id)),
            EditorSwitch::Reset => self.clear_editor_fields(),
        }
    }

    /// 把角色装载进编辑器（None = 新建空表单），并记录快照
    fn load_editor(&self, id: Option<i64>) {
        let ui = self.ui();
        let snapshot = match id {
            Some(id) => {
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
                ui.set_editing_character_id(id as i32);
                ui.set_editor_name(row.name.clone().into());
                ui.set_editor_title(row.title.clone().into());
                ui.set_editor_origin(row.origin.clone().into());
                ui.set_editor_faction(row.faction.clone().into());
                ui.set_editor_traits(SharedString::from(util::traits_text(&traits)));
                EditorSnapshot {
                    name: row.name.trim().to_string(),
                    title: row.title.trim().to_string(),
                    origin: row.origin.trim().to_string(),
                    faction: row.faction.trim().to_string(),
                    traits: util::traits_text(&traits).trim().to_string(),
                }
            }
            None => {
                ui.set_editing_character_id(-1);
                ui.set_editor_name("".into());
                ui.set_editor_title("".into());
                ui.set_editor_origin("".into());
                ui.set_editor_faction("".into());
                ui.set_editor_traits("".into());
                EditorSnapshot::default()
            }
        };
        ui.set_editor_name_touched(false);
        self.inner.lock().unwrap().editor_loaded = snapshot;
    }

    /// 仅清空字段、不重置编辑目标（修复「清空」把编辑态悄悄变成新建的陷阱）
    fn clear_editor_fields(&self) {
        let ui = self.ui();
        ui.set_editor_name("".into());
        ui.set_editor_title("".into());
        ui.set_editor_origin("".into());
        ui.set_editor_faction("".into());
        ui.set_editor_traits("".into());
        ui.set_editor_name_touched(false);
        self.inner.lock().unwrap().editor_loaded = EditorSnapshot::default();
    }

    fn new_character(&self) {
        self.guarded_editor_switch(EditorSwitch::New);
    }

    fn edit_character(&self, id: i64) {
        self.guarded_editor_switch(EditorSwitch::Edit(id));
    }

    fn reset_editor_form(&self) {
        self.guarded_editor_switch(EditorSwitch::Reset);
    }

    fn save_character(&self) {
        let name = self.ui().get_editor_name().trim().to_string();
        self.ui().set_editor_name_touched(true);
        if name.is_empty() {
            // 行内错误已由 FieldLabel 变体显示，这里补 Toast 提示
            self.toast("角色姓名不能为空");
            return;
        }
        let title = self.ui().get_editor_title().trim().to_string();
        let origin = self.ui().get_editor_origin().trim().to_string();
        let faction = self.ui().get_editor_faction().trim().to_string();
        let traits_text = self.ui().get_editor_traits().trim().to_string();
        let traits = util::parse_traits_text(&traits_text);

        match self.write_character(&name, &title, &origin, &faction, &traits) {
            Ok(id) => {
                {
                    // 保存成功后同步快照，避免随后切换被误判「有未保存修改」
                    self.inner.lock().unwrap().editor_loaded = EditorSnapshot {
                        name,
                        title,
                        origin,
                        faction,
                        traits: traits_text,
                    };
                }
                let ui = self.ui();
                ui.set_editing_character_id(id as i32);
                drop(ui);
                self.toast("角色已保存");
                self.refresh_characters();
                self.reload_starmap();
            }
            Err(msg) => self.toast_error(msg),
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
            inner.chars.iter().find(|c| c.id == id).map(|c| c.name.clone())
        };
        let Some(name) = name else {
            return;
        };
        self.ask_confirm(
            "删除角色".to_string(),
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
        let ui = self.ui();
        ui.set_forge_busy(true);
        ui.set_forge_preview("".into());
        ui.set_forge_error("".into());
        drop(ui);
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
                .inspect(|&id| {
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
                })
        };
        match inserted {
            Ok(_) => {
                let ui = self.ui();
                ui.set_forge_material("".into());
                ui.set_forge_preview("".into());
                drop(ui);
                self.toast(format!("角色「{}」已入库", card.name));
                self.refresh_characters();
                self.reload_starmap();
            }
            Err(e) => {
                self.inner.lock().unwrap().pending_card = Some(card);
                self.toast_error(format!("入库失败：{e}"));
            }
        }
    }

    /// 把铸造预览送入上方编辑器（入库前微调）
    fn forge_to_editor(&self) {
        let card = {
            let inner = self.inner.lock().unwrap();
            inner.pending_card.clone()
        };
        let Some(card) = card else {
            self.toast("请先完成一次成功的铸造");
            return;
        };
        let traits_text = util::traits_text(&card.traits);
        let ui = self.ui();
        ui.set_editing_character_id(-1);
        ui.set_editor_name(SharedString::from(card.name.clone()));
        ui.set_editor_title(SharedString::from(card.title.clone()));
        ui.set_editor_origin(SharedString::from(card.origin.clone()));
        ui.set_editor_faction(SharedString::from(card.faction.clone()));
        ui.set_editor_traits(SharedString::from(traits_text.clone()));
        ui.set_editor_name_touched(false);
        drop(ui);
        self.inner.lock().unwrap().editor_loaded = EditorSnapshot {
            name: card.name.trim().to_string(),
            title: card.title.trim().to_string(),
            origin: card.origin.trim().to_string(),
            faction: card.faction.trim().to_string(),
            traits: traits_text.trim().to_string(),
        };
        self.toast("已送入编辑器：可微调后保存");
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
        let ui = self.ui();
        ui.set_timeline_busy(true);
        ui.set_timeline_progress_text("".into());
        drop(ui);
        ops::spawn_timeline(&self.runtime, self.shared.clone());
    }

    /// 领地居民卡 → 跳对话页并选中该角色（P2-3「领地→角色」动线）
    fn open_character_chat(&self, id: i32) {
        if self.inner.lock().unwrap().char_index(id as i64).is_none() {
            return;
        }
        self.select_chat_character_by_id(id as i64);
        self.ui().set_current_page(0);
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
        self.send_starmap_load(id);
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
            self.send_starmap_load(id);
        } else {
            // 无角色可装载：给出明确反馈而非静默无效（UX 2.4-5）
            self.toast("尚无角色可装载：先在「角色」页创建或铸造");
        }
    }

    /// 装载星图：复位缩放 / 平移 / 选中光圈与首帧旗标（遮罩保持到新帧到达）
    fn send_starmap_load(&self, id: i64) {
        let ui = self.ui();
        ui.set_starmap_has_frame(false);
        ui.set_starmap_zoom_index(1);
        ui.set_starmap_pan_x(0.0);
        ui.set_starmap_pan_y(0.0);
        ui.set_starmap_sel_x(-1.0);
        ui.set_starmap_sel_y(-1.0);
        ui.set_starmap_detail_title("未选中星点".into());
        ui.set_starmap_detail_body(
            "点击星图中的星点，可在此查看对应的角色 / 特质 / 记忆详情。".into(),
        );
        drop(ui);
        let _ = self.starmap_tx.send(StarmapCmd::Load(id));
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
            self.toast_error(format!("配置保存失败：{e}"));
            return;
        }
        let ready = config.is_ready();
        let masked = if config.api_key.is_empty() {
            "未设置（匿名网关）".to_string()
        } else {
            settings::mask_key(&config.api_key)
        };
        *self.shared.settings.lock().unwrap() = config;
        let ui = self.ui();
        ui.set_api_ready(ready);
        drop(ui);
        self.toast(format!("配置已保存（API Key：{masked}，仅保存在本机数据目录）"));
    }

    /// 测试连通性：读取界面当前输入现场构造配置——所见即所测（P1-3）
    fn test_connection(&self) {
        let config = ApiSettings {
            base_url: self.ui().get_settings_base_url().trim().to_string(),
            api_key: self.ui().get_settings_api_key().trim().to_string(),
            model: self.ui().get_settings_model().trim().to_string(),
        };
        if let Err(msg) = config.quick_validate() {
            self.toast(msg);
            return;
        }
        let ui = self.ui();
        ui.set_conn_testing(true);
        ui.set_conn_state(1);
        ui.set_conn_result("正在测试……".into());
        drop(ui);
        ops::spawn_conn_test(&self.runtime, self.shared.clone(), config);
    }

    /// 复制最近导出路径到系统剪贴板（P1-4）
    fn copy_export_path(&self) {
        let path = self.ui().get_export_path_text().to_string();
        if path.is_empty() {
            return;
        }
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(path)) {
            Ok(()) => self.toast("已复制导出路径"),
            Err(e) => self.toast_error(format!("复制失败：{e}")),
        }
    }

    // —— 确认对话框 ——

    fn ask_confirm(&self, title: String, text: String, action: PendingAction) {
        self.inner.lock().unwrap().pending_action = Some(action);
        let ui = self.ui();
        ui.set_confirm_title(SharedString::from(title));
        ui.set_confirm_text(SharedString::from(text));
        ui.set_confirm_visible(true);
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
                self.ui().set_data_op_busy(true);
                ops::spawn_clear(self.shared.clone());
            }
            Some(PendingAction::ImportData(path)) => {
                self.ui().set_data_op_busy(true);
                ops::spawn_import(self.shared.clone(), path);
            }
            Some(PendingAction::DeleteSession(id)) => {
                self.delete_session(id);
            }
            Some(PendingAction::EditorSwitch(switch)) => {
                self.apply_editor_switch(&switch);
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
            // 仅在正在查看该会话且末行为流式气泡时更新；
            // 其余情况（如切换了会话）等待 ChatFinished 的整表重载对账（P0-2）
            let viewing = ctx.inner.lock().unwrap().current_session == session_id
                && !ctx.inner.lock().unwrap().session_pending;
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
        AppEvent::ChatFinished {
            session_id,
            error,
            stopped,
        } => {
            let ui = ctx.ui();
            ui.set_chat_busy(false);
            drop(ui);
            ctx.shared.chat_cancel.store(false, Ordering::Relaxed);
            let viewing = ctx.inner.lock().unwrap().current_session == session_id
                && !ctx.inner.lock().unwrap().session_pending;
            if viewing {
                // P0-2：从 DB 整体重载当前会话（替换「末行启发式」），绝不改写用户气泡；
                // 失败信息以独立错误气泡呈现（可重试）
                ctx.reload_session_messages(session_id);
                if error.is_some() {
                    let msg = error.clone().unwrap_or_default();
                    ctx.with_messages(|vm| {
                        vm.push(ChatBubble {
                            is_user: false,
                            content: SharedString::from(format!(
                                "请求失败：{}",
                                util::truncate_chars(&msg, 300)
                            )),
                            streaming: false,
                            failed: true,
                            time: SharedString::from(""),
                        });
                    });
                }
            }
            if stopped {
                ctx.toast("已停止生成（保留已生成部分）");
            }
            if let Some(msg) = &error {
                ctx.toast_error(util::truncate_chars(msg, 120));
            }
            // 会话时间 / 特质演化刷新
            ctx.refresh_characters();
        }
        AppEvent::ForgeDone(result) => {
            let ui = ctx.ui();
            ui.set_forge_busy(false);
            drop(ui);
            match result {
                Ok(forged) => match card::validate_forged(&forged) {
                    Ok(clean) => {
                        let preview = card::preview_text(&clean);
                        ctx.ui().set_forge_preview(SharedString::from(preview));
                        ctx.inner.lock().unwrap().pending_card = Some(clean);
                    }
                    Err(msg) => {
                        // 常驻错误行 + Toast 双通道（P1-7）
                        ctx.ui().set_forge_error(SharedString::from(msg.clone()));
                        ctx.toast_error(msg);
                    }
                },
                Err(msg) => {
                    let text = format!("铸造失败：{}", util::truncate_chars(&msg, 200));
                    ctx.ui().set_forge_error(SharedString::from(text.clone()));
                    ctx.toast_error(text);
                }
            }
        }
        AppEvent::TimelineProgress { index, total } => {
            ctx.ui().set_timeline_progress_text(SharedString::from(format!(
                "生成中 · 第 {index}/{total} 位"
            )));
        }
        AppEvent::TimelineDone { ok, failed } => {
            let ui = ctx.ui();
            ui.set_timeline_busy(false);
            ui.set_timeline_progress_text("".into());
            drop(ui);
            ctx.refresh_timeline();
            if failed == 0 && ok > 0 {
                ctx.toast(format!("已生成 {ok} 条近况"));
            } else if ok > 0 || failed > 0 {
                ctx.toast(format!("近况生成完成：成功 {ok} · 失败 {failed}"));
            }
        }
        AppEvent::ConnTestDone(result) => {
            let ui = ctx.ui();
            ui.set_conn_testing(false);
            match result {
                Ok(msg) => {
                    ui.set_conn_state(2);
                    ui.set_conn_result(SharedString::from(msg));
                }
                Err(msg) => {
                    ui.set_conn_state(3);
                    ui.set_conn_result(SharedString::from(format!(
                        "连接失败：{}",
                        util::truncate_chars(&msg, 260)
                    )));
                }
            }
        }
        AppEvent::ExportDone(result) => match result {
            Ok(path) => {
                let ui = ctx.ui();
                ui.set_data_op_busy(false);
                ui.set_export_path_text(SharedString::from(path.clone()));
                drop(ui);
                ctx.toast(format!("导出成功：{path}"));
            }
            Err(msg) => {
                ctx.ui().set_data_op_busy(false);
                ctx.toast_error(msg);
            }
        },
        AppEvent::ImportDone(result) => {
            let ui = ctx.ui();
            ui.set_data_op_busy(false);
            drop(ui);
            match &result {
                Ok(msg) => ctx.toast(msg.clone()),
                Err(msg) => ctx.toast_error(msg.clone()),
            }
            // 先刷新角色列表再选中：取导入后的首角色，而非导入前的脏值（与 bootstrap 顺序对齐）
            ctx.refresh_characters();
            {
                let mut inner = ctx.inner.lock().unwrap();
                inner.current_character = inner.chars.first().map(|c| c.id).unwrap_or(-1);
                inner.current_session = -1;
                inner.session_pending = false;
                inner.starmap_char = inner.current_character;
            }
            ctx.refresh_sessions();
            ctx.refresh_timeline();
            ctx.reload_starmap();
        }
        AppEvent::ClearDone(result) => match result {
            Ok(()) => {
                let ui = ctx.ui();
                ui.set_data_op_busy(false);
                drop(ui);
                ctx.toast("数据已清空（模型配置保留）");
                {
                    let mut inner = ctx.inner.lock().unwrap();
                    inner.current_character = -1;
                    inner.current_session = -1;
                    inner.session_pending = false;
                    inner.starmap_char = -1;
                }
                let ui = ctx.ui();
                ctx.refresh_characters();
                ctx.refresh_sessions();
                ctx.refresh_timeline();
                ctx.set_messages(Vec::new());
                ui.set_starmap_running(false);
                ui.set_starmap_has_frame(false);
                ui.set_starmap_stats("".into());
                ui.set_starmap_status("尚无角色可装载。".into());
                ui.set_starmap_detail_title("未选中星点".into());
                ui.set_starmap_detail_body("点击星图中的星点，可在此查看详情。".into());
                ui.set_starmap_sel_x(-1.0);
                ui.set_starmap_sel_y(-1.0);
            }
            Err(msg) => {
                ctx.ui().set_data_op_busy(false);
                ctx.toast_error(msg);
            }
        },
        AppEvent::StarmapDetail {
            title,
            body,
            px,
            py,
        } => {
            let ui = ctx.ui();
            if title.is_empty() {
                // 点击空白：复位详情与光圈（P1-5）
                ui.set_starmap_detail_title("未选中星点".into());
                ui.set_starmap_detail_body(
                    "点击星图中的星点，可在此查看对应的角色 / 特质 / 记忆详情。".into(),
                );
                ui.set_starmap_sel_x(-1.0);
                ui.set_starmap_sel_y(-1.0);
            } else {
                ui.set_starmap_detail_title(SharedString::from(title));
                ui.set_starmap_detail_body(SharedString::from(body));
                ui.set_starmap_sel_x(px);
                ui.set_starmap_sel_y(py);
            }
        }
        AppEvent::StarmapStatus {
            running,
            text,
            stats,
        } => {
            let ui = ctx.ui();
            ui.set_starmap_running(running);
            ui.set_starmap_status(SharedString::from(text));
            ui.set_starmap_stats(SharedString::from(stats));
            if running {
                // 装载后新帧未到：遮罩保持到首帧，避免首帧空窗（UX 2.4-4）
                ui.set_starmap_has_frame(false);
            }
        }
    }
}

/// 慢速节拍：世界时钟推进、星图启停、Toast 过期、呼吸相位翻转、会话时间刷新
fn slow_tick(ctx: &Ctx, starmap_enabled: &AtomicBool) {
    starmap_enabled.store(ctx.ui().get_current_page() == 3, Ordering::Relaxed);

    let (clock_text, toast_expired, tick) = {
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
        inner.tick = inner.tick.wrapping_add(1);
        let text = format!(
            "第 {} 世界日 · {} · {}（现实 1 秒 = 世界 72 秒）",
            inner.world_clock.day_index() + 1,
            inner.world_clock.season().chinese_name(),
            inner.world_clock.day_phase().chinese_name(),
        );
        let expired = match inner.toast_until {
            Some(deadline) if Instant::now() >= deadline => {
                inner.toast_until = None;
                true
            }
            _ => false,
        };
        (text, expired, inner.tick)
    };
    let ui = ctx.ui();
    ui.set_world_clock_text(SharedString::from(clock_text));

    if toast_expired {
        ctx.advance_toast();
    }

    // 呼吸相位：每 7 个 tick（1400ms）翻转一次，驱动 StreamingDots / 徽标点 / 光圈 / 品牌徽标
    if tick % 7 == 0 {
        let tokens = ui.global::<Tokens>();
        tokens.set_glow_phase(!tokens.get_glow_phase());
    }

    // 会话相对时间：每 15 秒（75 tick）重算一次（P2-5）
    if tick % 75 == 0 {
        ctx.refresh_session_times();
    }
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
