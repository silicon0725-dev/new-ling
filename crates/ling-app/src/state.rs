//! 跨线程共享的应用状态与 UI 事件总线。
//!
//! - `Store`（rusqlite 连接非 Sync）由 `Arc<Mutex<_>>` 包裹，所有线程短暂加锁使用；
//! - 后台任务通过 `std::sync::mpsc` 把事件发回 UI 线程，由 bridge 的定时器统一泵送。

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use ling_core::ai::ForgedCharacter;
use ling_core::store::Store;

use crate::settings::ApiSettings;

/// 后台任务 → UI 线程的事件
#[derive(Debug)]
pub enum AppEvent {
    /// 顶部轻提示
    Toast(String),
    /// 对话流式增量
    ChatDelta { session_id: i64, delta: String },
    /// 对话一轮结束（`error` 携带中文错误说明；`stopped` = 用户主动停止且已保留部分回复）
    ChatFinished {
        session_id: i64,
        error: Option<String>,
        stopped: bool,
    },
    /// 「铸造」完成
    ForgeDone(Result<ForgedCharacter, String>),
    /// 「生成近况」单角色进度（1 起计数）
    TimelineProgress { index: usize, total: usize },
    /// 「生成近况」批量完成
    TimelineDone { ok: usize, failed: usize },
    /// 连通性测试完成
    ConnTestDone(Result<String, String>),
    /// 导出完成（携带文件路径）
    ExportDone(Result<String, String>),
    /// 导入完成
    ImportDone(Result<String, String>),
    /// 清空数据完成
    ClearDone(Result<(), String>),
    /// 星图节点详情（点击拾取结果；`px/py` 为选中星点的帧像素坐标，
    /// 未命中时 title 为空、`px < 0`，UI 侧据此复位详情与光圈）
    StarmapDetail {
        title: String,
        body: String,
        px: f32,
        py: f32,
    },
    /// 星图装载 / 运行状态（`stats` 为「N 节点 · M 连线」常驻运行数据）
    StarmapStatus {
        running: bool,
        text: String,
        stats: String,
    },
}

/// 全局共享句柄
pub struct AppShared {
    pub store: Arc<Mutex<Store>>,
    pub settings: Arc<Mutex<ApiSettings>>,
    pub event_tx: Sender<AppEvent>,
    /// 对话流式生成的中止旗标：发送前置 false，用户点「停止」置 true
    pub chat_cancel: Arc<AtomicBool>,
}

impl AppShared {
    pub fn new(store: Store, settings: ApiSettings, event_tx: Sender<AppEvent>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            settings: Arc::new(Mutex::new(settings)),
            event_tx,
            chat_cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 发送 UI 事件（接收端必然存活到事件循环结束，忽略发送失败）
    pub fn emit(&self, event: AppEvent) {
        let _ = self.event_tx.send(event);
    }

    /// 快照当前模型配置
    pub fn settings_snapshot(&self) -> ApiSettings {
        self.settings.lock().unwrap().clone()
    }
}
