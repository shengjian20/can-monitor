//! # Tauri 命令
//!
//! 供前端 `invoke()` 调用的 IPC 命令集合, 中文注释说明每个命令的职责。
//!
//! ## 设备发现
//!
//! 经 `can_devices::DeviceManager::list_devices()` 聚合 SocketCAN + USBVCI。
//!
//! ## 帧 JSON 契约
//!
//! Channel 推送的帧 JSON 与 Web 端 (T15/T16) 统一:
//! ```json
//! { "ts": "1691234567890", "id": "0x181", "ext": false,
//!   "dir": "rx", "data": "01 02 03", "protocol": "canopen",
//!   "summary": "TPDO1 node 1" }
//! ```
//! - `ts`: u64 毫秒时间戳, 字符串防 JS 2^53 溢出
//! - `id`: 十六进制 CAN ID 字符串
//! - `ext`: 是否扩展帧 (29 位)
//! - `data`: 大写十六进制空格分隔
//! - `protocol`: "canopen" / "j1939" / "raw"

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use can_monitor_core::bus::MonitorBus;
use can_monitor_core::classifier::{FrameClassifier, ParsedMessage, StreamItem};
use can_types::{
    BackendConfig, BackendKind, CanBackend, CanDeviceInfo, CanFrame, CanId, Direction,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use crate::state::{ChannelTask, TauriState};

// ─── JSON DTO ───────────────────────────────────────────────────

/// 设备信息 JSON (前端 list_devices 返回)。
#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfoJson {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub driver: String,
    pub model: String,
    pub available: bool,
    /// 设备类型码 (厂商定义; SocketCAN 等无此概念的后端为 0)。
    pub device_type: u32,
}

impl From<&CanDeviceInfo> for DeviceInfoJson {
    fn from(d: &CanDeviceInfo) -> Self {
        Self {
            id: d.id.clone(),
            name: d.name.clone(),
            kind: format!("{:?}", d.kind),
            driver: d.driver.clone(),
            model: d.details.model.clone(),
            available: d.available,
            device_type: d.device_type,
        }
    }
}

/// Channel 帧推送 JSON。
#[derive(Debug, Clone, Serialize)]
pub struct FrameJson {
    /// 毫秒时间戳 (字符串, 防 JS 2^53 溢出)。
    pub ts: String,
    /// 十六进制 CAN ID。
    pub id: String,
    /// 是否扩展帧。
    pub ext: bool,
    /// 收发方向: "rx" / "tx"。
    pub dir: String,
    /// 大写十六进制空格分隔数据。
    pub data: String,
    /// 协议类别: "canopen" / "j1939" / "raw"。
    pub protocol: String,
    /// 可读摘要。
    pub summary: String,
}

/// 监控状态 JSON (get_status 返回)。
#[derive(Debug, Clone, Serialize)]
pub struct StatusJson {
    /// 是否正在监控。
    pub running: bool,
    /// 已读帧总数。
    pub total: u64,
    /// CANopen 帧数。
    pub canopen: u64,
    /// J1939 帧数。
    pub j1939: u64,
    /// 后端错误数。
    pub error: u64,
    /// 丢弃帧数 (消费者队列满)。
    pub dropped: u64,
}

/// send_frame 输入 JSON。
#[derive(Debug, Clone, Deserialize)]
pub struct SendFrameRequest {
    /// CAN ID (十进制或 "0x" 前缀十六进制)。
    pub id: String,
    /// 是否扩展帧。
    pub ext: bool,
    /// 数据: 空格分隔十六进制 ("01 02 0A")。
    pub data: String,
}

// ─── 辅助函数 ───────────────────────────────────────────────────

/// 将 StreamItem 转为帧 JSON (供 Channel 推送)。
///
/// 直接使用 reader 已分类的 ParsedMessage, 绝不重复 classify。
fn stream_item_to_json(item: &StreamItem) -> FrameJson {
    let frame = &item.msg.frame;
    let ts_ms = frame
        .timestamp()
        .unwrap_or(SystemTime::now())
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let (protocol, summary) = match &item.parsed {
        ParsedMessage::Canopen { msg, .. } => ("canopen".to_string(), format!("{:?}", msg)),
        ParsedMessage::J1939 { msg, .. } => ("j1939".to_string(), format!("{:?}", msg)),
        ParsedMessage::Raw(_) => ("raw".to_string(), String::new()),
    };

    let data_hex: String = frame
        .data()
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ");

    FrameJson {
        ts: ts_ms.to_string(),
        id: format!("0x{:X}", frame.id().raw_id()),
        ext: frame.id().is_extended(),
        dir: match item.msg.direction {
            Direction::Rx => "rx".to_string(),
            Direction::Tx => "tx".to_string(),
        },
        data: data_hex,
        protocol,
        summary,
    }
}

/// 聚合所有已实现的 DeviceDiscoverer, 返回设备列表。
///
/// 经 `can_devices::DeviceManager` 聚合 SocketCAN + USBVCI。
fn aggregate_devices() -> Vec<can_types::CanDeviceInfo> {
    can_devices::DeviceManager::list_devices()
}

/// 解析设备 ID 字符串, 返回 (backend_kind, 参数)。
///
/// 格式:
/// - "socketcan" / "socketcan:can0" → SocketCAN
/// - "usbvci" / "usbvci:0" → USBVCI
/// - "none" → 无后端
fn parse_device_id(device_id: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = device_id.splitn(2, ':').collect();
    let backend = parts[0].to_lowercase();
    let param = if parts.len() > 1 {
        parts[1].to_string()
    } else {
        match backend.as_str() {
            "socketcan" => "can0".to_string(),
            "usbvci" => "0".to_string(),
            "none" => String::new(),
            _ => return Err(format!("未知设备类型: {backend}")),
        }
    };
    Ok((backend, param))
}

// ─── 命令 ───────────────────────────────────────────────────────

/// 列出所有已发现的 CAN 设备。
#[tauri::command]
pub fn list_devices() -> Result<Vec<DeviceInfoJson>, String> {
    Ok(aggregate_devices()
        .iter()
        .map(DeviceInfoJson::from)
        .collect())
}

/// 启动监控: 创建 MonitorBus + 后端 reader, 并开启监控。
///
/// @param device_id 设备标识 (如 "socketcan:can0", "usbvci:0", "none")。
#[tauri::command]
pub fn start_monitor(device_id: String, state: State<'_, TauriState>) -> Result<(), String> {
    // 先停掉已有 bus (如果有)。
    {
        let mut bus_guard = state
            .bus
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(bus) = bus_guard.take() {
            bus.shutdown();
        }
    }

    let (backend_name, param) = parse_device_id(&device_id)?;

    // 创建新 bus。
    let (bus, _rx, _err_rx) = MonitorBus::new();

    // 根据后端类型构造并启动 reader。
    match backend_name.as_str() {
        "socketcan" => {
            let config = BackendConfig::SocketCan {
                iface: param.clone(),
                fd: false,
            };
            let backend = can_socketcan::SocketCanBackend::open(&config)
                .map_err(|e| format!("打开 SocketCAN 后端失败 ({param}): {e}"))?;
            // 创建新分类器供 bus reader 使用 (FrameClassifier 不实现 Clone, 用 default)。
            let classifier = Arc::new(std::sync::Mutex::new(FrameClassifier::default()));
            bus.start_reader(backend, classifier, BackendKind::SocketCan)
                .map_err(|e| format!("启动 SocketCAN reader 失败: {e}"))?;
        }
        "usbvci" => {
            let device_index: u32 = param
                .parse()
                .map_err(|_| format!("USBVCI 设备索引无效: {param}"))?;
            let config = BackendConfig::UsbVci {
                // 配置类型仅作探测候选之一, 后端按 find 板卡信息映射首选 (2E_U=21)。
                device_type: can_usbvci::VCI_USBCAN_2E_U,
                device_index,
                channel: 0,
            };
            let backend = can_usbvci::UsbVciBackend::open(&config)
                .map_err(|e| format!("打开 USBCAN 后端失败 (index={device_index}): {e}"))?;
            let classifier = Arc::new(std::sync::Mutex::new(FrameClassifier::default()));
            bus.start_reader(backend, classifier, BackendKind::UsbVci)
                .map_err(|e| format!("启动 USBCAN reader 失败: {e}"))?;
        }
        "none" => {
            // 无后端: bus 就绪但不启动 reader。
        }
        other => return Err(format!("未知后端: {other}")),
    }

    // 开启监控。
    bus.set_monitoring(true);

    // 写入状态。
    let mut bus_guard = state
        .bus
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *bus_guard = Some(bus);
    Ok(())
}

/// 停止监控: shutdown bus 并清理所有 Channel 推送任务。
#[tauri::command]
pub fn stop_monitor(state: State<'_, TauriState>) -> Result<(), String> {
    // 停止 bus。
    {
        let mut bus_guard = state
            .bus
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(bus) = bus_guard.take() {
            bus.shutdown();
        }
    }

    // 停止所有 Channel 推送任务。
    let mut tasks = state
        .channel_tasks
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    for (_, task) in tasks.drain() {
        task.stop.store(true, Ordering::Relaxed);
        let _ = task.handle.join();
    }

    Ok(())
}

/// 订阅帧流: 创建 Channel 推送任务, 帧经 tauri::ipc::Channel 实时推送到前端。
///
/// @param on_frame 前端传入的 Channel, 用于接收帧 JSON。
/// @return channel_id (供前端后续引用)。
#[tauri::command]
pub fn subscribe_frames(
    on_frame: Channel<FrameJson>,
    state: State<'_, TauriState>,
) -> Result<u64, String> {
    // 获取 bus 的订阅接收端。
    let (_consumer_id, rx) = {
        let bus_guard = state
            .bus
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        match bus_guard.as_ref() {
            Some(bus) => bus.subscribe(),
            None => return Err("监控未启动, 请先调用 start_monitor".to_string()),
        }
    };

    let channel_id = state.next_channel_id.fetch_add(1, Ordering::Relaxed);
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_flag);

    // 推送线程: 从 bus 订阅端读 StreamItem → 转 JSON → Channel.send。
    // StreamItem 已由 reader 分类, 不再重复 classify。
    let handle = thread::Builder::new()
        .name(format!("can-channel-{channel_id}"))
        .spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(item) => {
                        let json = stream_item_to_json(&item);
                        if on_frame.send(json).is_err() {
                            break;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(|e| format!("启动 Channel 推送线程失败: {e}"))?;

    // 记录任务句柄。
    let mut tasks = state
        .channel_tasks
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    tasks.insert(
        channel_id,
        ChannelTask {
            handle,
            stop: stop_flag,
        },
    );

    Ok(channel_id)
}

/// 取消订阅: 停止指定 Channel 推送任务。
///
/// @param channel_id subscribe_frames 返回的 ID。
#[tauri::command]
pub fn unsubscribe_frames(channel_id: u64, state: State<'_, TauriState>) -> Result<(), String> {
    let mut tasks = state
        .channel_tasks
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if let Some(task) = tasks.remove(&channel_id) {
        task.stop.store(true, Ordering::Relaxed);
        let _ = task.handle.join();
    }
    Ok(())
}

/// 从原始帧字段构建 [`CanFrame`] (供 `send_frame` 使用, 抽离便于单元测试)。
///
/// 与 REST 端 `parse_frame` 同构; 标准帧分支用 [`CanId::new_standard_checked()`]
/// **先查 11 位范围再转型**, 拒绝 `as u16` 截断先于校验的静默超界值
/// (如 0x10000 / 0x1FFF0000)。
///
/// @param id   原始 CAN ID 字符串 (十进制或 `0x` 前缀十六进制)。
/// @param ext  是否扩展帧。
/// @param data 空格分隔十六进制数据。
/// @return 构建成功的 [`CanFrame`]; ID 超界 / 格式非法返回中文错误。
fn build_frame(id: &str, ext: bool, data: &str) -> Result<CanFrame, String> {
    // 解析 ID。
    let raw_id: u32 = if id.starts_with("0x") || id.starts_with("0X") {
        u32::from_str_radix(&id[2..], 16)
    } else {
        id.parse()
    }
    .map_err(|_| format!("CAN ID 格式无效: {id}"))?;

    let can_id = if ext {
        CanId::new_extended(raw_id)
    } else {
        CanId::new_standard_checked(raw_id)
    }
    .map_err(|e| format!("CAN ID 错误: {e}"))?;

    // 解析数据。
    let data_bytes: Vec<u8> = data
        .split_whitespace()
        .map(|s| u8::from_str_radix(s, 16).map_err(|_| format!("数据字节无效: {s}")))
        .collect::<Result<Vec<_>, _>>()?;

    CanFrame::new(can_id, data_bytes).map_err(|e| format!("构造帧失败: {e}"))
}

/// 发送一帧到总线。
///
/// @param frame 帧 JSON (id + ext + data)。
#[tauri::command]
pub fn send_frame(frame: SendFrameRequest, state: State<'_, TauriState>) -> Result<(), String> {
    let can_frame = build_frame(&frame.id, frame.ext, &frame.data)?;

    let bus_guard = state
        .bus
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    match bus_guard.as_ref() {
        Some(bus) => bus
            .send_frame(can_frame)
            .map_err(|e| format!("发送帧失败: {e}")),
        None => Err("监控未启动".to_string()),
    }
}

/// 获取监控状态 (running, 计数器)。
#[tauri::command]
pub fn get_status(state: State<'_, TauriState>) -> Result<StatusJson, String> {
    let bus_guard = state
        .bus
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    match bus_guard.as_ref() {
        Some(bus) => Ok(StatusJson {
            running: bus.is_monitoring(),
            total: bus.total_frames(),
            canopen: bus.canopen_count(),
            j1939: bus.j1939_count(),
            error: bus.error_count(),
            dropped: bus.dropped_frames(),
        }),
        None => Ok(StatusJson {
            running: false,
            total: 0,
            canopen: 0,
            j1939: 0,
            error: 0,
            dropped: 0,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 标准帧 ID 超出 11 位范围 → Err (回归 F2 P1: 不得静默截断)。
    #[test]
    fn build_frame_rejects_out_of_range_standard_id() {
        for id in ["0x1FFF0000", "0x10000", "0x107FF", "0x800"] {
            let err = build_frame(id, false, "01 02 03").unwrap_err();
            assert!(
                err.contains("CAN ID"),
                "标准帧 ID {id} 应被拒绝并返回中文错误: {err}"
            );
        }
    }

    /// 合法标准帧 ID (≤0x7FF) 仍接受。
    #[test]
    fn build_frame_accepts_valid_standard_id() {
        let frame = build_frame("0x181", false, "01 02 03").unwrap();
        assert_eq!(frame.id().raw_id(), 0x181);
        assert!(!frame.id().is_extended());
    }

    /// 合法扩展帧 ID (≤0x1FFFFFFF) 仍接受, 行为不变。
    #[test]
    fn build_frame_accepts_valid_extended_id() {
        let frame = build_frame("0x18FF1234", true, "01 02 03").unwrap();
        assert_eq!(frame.id().raw_id(), 0x18FF1234);
        assert!(frame.id().is_extended());
    }
}
