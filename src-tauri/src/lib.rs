//! # can-monitor-tauri — Tauri v2 桌面应用库
//!
//! 供 main.rs (桌面) 与 mobile entry point 复用的 `run()` 函数。
//! 注册状态、命令, 并启动 Tauri 主循环。

pub mod commands;
pub mod state;

use state::TauriState;

/// 启动 Tauri 应用。
///
/// 注册 [`TauriState`] 与全部命令, 运行主事件循环。
/// T18 (web 前端) 落地前, `cargo tauri dev` 因 web/ 不存在会失败;
/// `cargo tauri build` 同理。Rust 侧 `cargo check -p can-monitor-tauri`
/// 不依赖前端, 可独立验证。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(TauriState::new())
        .invoke_handler(tauri::generate_handler![
            commands::list_devices,
            commands::start_monitor,
            commands::stop_monitor,
            commands::subscribe_frames,
            commands::unsubscribe_frames,
            commands::send_frame,
            commands::get_status,
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
