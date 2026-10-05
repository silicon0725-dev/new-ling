//! 「灵」桌面应用入口：初始化数据目录 / 数据库 / 异步运行时 / 星图线程，
//! 装配 slint UI 并进入事件循环。

mod bridge;
mod card;
mod chat;
mod export;
mod ops;
mod prompt;
mod settings;
mod state;
mod starmap;
mod timeline;
mod util;

use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;

use slint::ComponentHandle;
use state::{AppEvent, AppShared};

fn main() {
    // —— 数据目录与持久化 ——
    let dir = settings::data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("无法创建数据目录 {dir:?}：{e}");
        std::process::exit(1);
    }
    let store = match ling_core::store::Store::open(&dir.join("ling.db")) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("打开数据库失败：{e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = timeline::ensure_table(&store) {
        eprintln!("初始化近况表失败：{e}");
        std::process::exit(1);
    }
    let config = settings::ApiSettings::load(&dir.join("settings.json"));

    // —— 通道与共享状态 ——
    let (event_tx, event_rx) = mpsc::channel::<AppEvent>();
    let (frame_tx, frame_rx) = mpsc::sync_channel(1);
    let (cmd_tx, cmd_rx) = mpsc::channel::<starmap::StarmapCmd>();
    let shared = Arc::new(AppShared::new(store, config, event_tx));

    // —— 异步运行时与星图渲染线程 ——
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("启动异步运行时失败"),
    );
    let starmap_enabled = Arc::new(AtomicBool::new(false));
    let _starmap_handle = starmap::spawn(
        shared.clone(),
        cmd_rx,
        frame_tx,
        starmap_enabled.clone(),
    );

    // —— UI ——
    let ui = bridge::App::new().unwrap_or_else(|e| {
        eprintln!("初始化界面失败：{e}");
        std::process::exit(1);
    });
    let _timers = bridge::attach(
        &ui,
        shared,
        runtime,
        cmd_tx,
        event_rx,
        frame_rx,
        starmap_enabled,
    );
    // —— 可选冒烟自检：LING_SMOKE_NAV=1 时自动逐页巡检（含星图 GPU 帧渲染）后正常退出，
    //    用于无人工介入环境的「五页可导航、无 panic」验收；正常使用不受影响。
    if std::env::var_os("LING_SMOKE_NAV").is_some() {
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            for page in 0..=5 {
                let w = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = w.upgrade() {
                        ui.set_current_page(page);
                    }
                });
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
            let w = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = w.upgrade() {
                    let _ = ui.hide();
                }
            });
        });
    }

    if let Err(e) = ui.run() {
        eprintln!("事件循环异常退出：{e}");
        std::process::exit(1);
    }
}
