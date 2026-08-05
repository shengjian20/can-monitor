//! # can-monitor-tauri — 桌面入口
//!
//! 桌面平台 (Linux / macOS / Windows) 的 `main` 函数。
//! 调用 [`can_monitor_tauri_lib::run`] 启动 Tauri 主循环。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    can_monitor_tauri_lib::run();
}
