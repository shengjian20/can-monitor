//! # Tauri 应用状态
//!
//! 持有 [`MonitorBus`] 与 Channel 推送任务的 JoinHandle, 供命令闭包共享访问。
//! Mutex 毒锁时用 [`into_inner`] 恢复, 避免 panic。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use can_monitor_core::bus::MonitorBus;

/// Channel 推送任务句柄, stop 时清理。
pub(crate) struct ChannelTask {
    /// 推送线程的 JoinHandle (stop 时 join 或 drop)。
    pub handle: JoinHandle<()>,
    /// 停止标志: 置 true 后推送线程退出。
    pub stop: Arc<std::sync::atomic::AtomicBool>,
}

/// Tauri 全局状态。
///
/// 由 [`tauri::Builder::manage`] 注册, 命令通过 [`tauri::State`] 访问。
pub(crate) struct TauriState {
    /// 消息总线 (None 表示未启动监控)。
    pub bus: Mutex<Option<MonitorBus>>,
    /// Channel 推送任务表 (channel_id → 任务句柄)。
    pub channel_tasks: Mutex<HashMap<u64, ChannelTask>>,
    /// Channel ID 分配器。
    pub next_channel_id: std::sync::atomic::AtomicU64,
}

impl TauriState {
    /// 创建初始状态 (bus 为 None)。
    pub fn new() -> Self {
        Self {
            bus: Mutex::new(None),
            channel_tasks: Mutex::new(HashMap::new()),
            next_channel_id: std::sync::atomic::AtomicU64::new(0),
        }
    }
}
