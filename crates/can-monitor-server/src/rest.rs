//! # REST 端点
//!
//! 追加在 axum Router 上的 REST API (与 `/ws` 批量帧流共存):
//!
//! | 端点 | 方法 | 作用 |
//! |------|------|------|
//! | `/api/devices` | GET  | 设备列表 (经 can-devices 聚合 SocketCAN + USBCAN) |
//! | `/api/monitor/start` | POST | 开启监控 (body: `{"device_id": "socketcan:can0"}`) |
//! | `/api/monitor/stop`  | POST | 关闭监控 |
//! | `/api/send`   | POST | 发送一帧 (body: `{id, ext, data}`) — **仅写模式可用** |
//! | `/api/status` | GET  | 状态快照 (running / total / canopen / j1939 / error / dropped) |
//!
//! ## 写门控 (安全)
//!
//! `POST /api/send` 属于写操作: 仅当 [`AppState::write_enabled`](crate::AppState::write_enabled)
//! 为真 (CLI `--web-write`) 时可用, 否则一律返回 HTTP 403, 与请求内容无关。
//! start / stop 只控制监控开关 (读侧), **永远可用**。

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use can_types::{CanDeviceInfo, CanFrame, CanId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::AppState;

/// 设备信息 JSON (`GET /api/devices` 返回元素)。
///
/// 与 Tauri `list_devices` 返回结构一致 (id/name/kind/driver/model/available)。
#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfoJson {
    /// 设备唯一标识 (如 `socketcan:can0` / `usbvci:0`)。
    pub id: String,
    /// 面向用户的显示名称。
    pub name: String,
    /// 设备种类 (如 `"SocketCan"` / `"UsbVci"`)。
    pub kind: String,
    /// 后端驱动标识。
    pub driver: String,
    /// 设备型号。
    pub model: String,
    /// 当前是否可用。
    pub available: bool,
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
        }
    }
}

/// 监控状态 JSON (`GET /api/status` 返回)。
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

/// `POST /api/send` 请求体。
#[derive(Debug, Clone, Deserialize)]
pub struct SendFrameRequest {
    /// CAN ID (十进制或 `0x` 前缀十六进制)。
    pub id: String,
    /// 是否扩展帧。
    pub ext: bool,
    /// 数据: 空格分隔十六进制 (如 `"01 02 0A"`)。
    pub data: String,
}

/// `POST /api/monitor/start` 请求体。
#[derive(Debug, Clone, Deserialize)]
pub struct StartMonitorRequest {
    /// 设备标识 (如 `"socketcan:can0"` / `"usbvci:0"` / `"none"`)。
    pub device_id: String,
}

/// 解析设备 ID 字符串, 返回 (backend_kind, 参数)。
///
/// 格式:
/// - `"socketcan"` / `"socketcan:can0"` → SocketCAN
/// - `"usbvci"` / `"usbvci:0"` → USBVCI
/// - `"none"` → 无后端
///
/// 仅做格式校验 (总线后端在 CLI 启动时已固定), 不重新打开设备。
fn parse_device_id(device_id: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = device_id.splitn(2, ':').collect();
    let backend = parts[0].to_lowercase();
    if !matches!(backend.as_str(), "socketcan" | "usbvci" | "none") {
        return Err(format!("未知设备类型: {backend}"));
    }
    let param = if parts.len() > 1 {
        parts[1].to_string()
    } else {
        match backend.as_str() {
            "socketcan" => "can0".to_string(),
            "usbvci" => "0".to_string(),
            _ => String::new(),
        }
    };
    Ok((backend, param))
}

/// 解析发送帧请求为 [`CanFrame`]。
fn parse_frame(req: &SendFrameRequest) -> Result<CanFrame, String> {
    let raw_id: u32 = if req.id.starts_with("0x") || req.id.starts_with("0X") {
        u32::from_str_radix(&req.id[2..], 16)
    } else {
        req.id.parse()
    }
    .map_err(|_| format!("CAN ID 格式无效: {}", req.id))?;

    let can_id = if req.ext {
        CanId::new_extended(raw_id)
    } else {
        CanId::new_standard_checked(raw_id)
    }
    .map_err(|e| format!("CAN ID 错误: {e}"))?;

    let data: Vec<u8> = req
        .data
        .split_whitespace()
        .map(|s| u8::from_str_radix(s, 16).map_err(|_| format!("数据字节无效: {s}")))
        .collect::<Result<Vec<_>, _>>()?;

    CanFrame::new(can_id, data).map_err(|e| format!("构造帧失败: {e}"))
}

/// 统一 JSON 响应类型: 状态码 + JSON 对象。
type ApiResult = (StatusCode, Json<Value>);

/// 构造成功响应。
fn ok(v: Value) -> ApiResult {
    (StatusCode::OK, Json(v))
}

/// 构造错误响应。
fn err(status: StatusCode, message: impl Into<String>) -> ApiResult {
    (status, Json(json!({ "error": message.into() })))
}

/// `GET /api/devices`: 聚合两端后端的设备列表。
///
/// @return 设备 JSON 数组 (无设备时为空数组, 不 panic)。
pub async fn get_devices() -> Json<Vec<DeviceInfoJson>> {
    Json(
        can_devices::DeviceManager::list_devices()
            .iter()
            .map(DeviceInfoJson::from)
            .collect(),
    )
}

/// `POST /api/monitor/start`: 开启监控 (读侧控制, 永远可用)。
///
/// 校验 `device_id` 格式后置位总线监控开关。总线后端已在 CLI 启动时打开,
/// 此处不做设备重开 (与 TUI 共享同一总线)。
///
/// @param state 共享应用状态。
/// @param req   设备选择请求体。
/// @return 200 (已开启) / 400 (device_id 格式非法)。
pub async fn start_monitor(
    State(state): State<AppState>,
    Json(req): Json<StartMonitorRequest>,
) -> ApiResult {
    match parse_device_id(&req.device_id) {
        Ok((kind, _)) => {
            state.bus.set_monitoring(true);
            ok(json!({ "ok": true, "device": kind }))
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// `POST /api/monitor/stop`: 关闭监控 (读侧控制, 永远可用)。
///
/// @param state 共享应用状态。
/// @return 200 (已关闭)。
pub async fn stop_monitor(State(state): State<AppState>) -> ApiResult {
    state.bus.set_monitoring(false);
    ok(json!({ "ok": true, "running": false }))
}

/// `POST /api/send`: 发送一帧到总线 (**仅写模式可用**)。
///
/// 安全门控: [`AppState::write_enabled`] 为假时恒返回 **403**, 与请求内容无关。
/// 解析成功且总线接受后返回 200; 帧格式非法返回 400; 总线队列满返回 500。
///
/// @param state 共享应用状态。
/// @param req   帧请求体 (id / ext / data)。
/// @return 200 发送成功 / 403 写未启用 / 400 帧格式非法 / 500 总线错误。
pub async fn send(State(state): State<AppState>, Json(req): Json<SendFrameRequest>) -> ApiResult {
    // 写门控: 未启用写模式时直接拒绝, 不解析请求内容。
    if !state.write_enabled {
        return err(
            StatusCode::FORBIDDEN,
            "写入未启用: 服务需以 --web-write 启动才允许发送帧",
        );
    }
    match parse_frame(&req) {
        Ok(frame) => match state.bus.send_frame(frame) {
            Ok(()) => ok(json!({ "ok": true })),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        },
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// `GET /api/status`: 监控状态快照。
///
/// @param state 共享应用状态。
/// @return 状态 JSON (running 与各计数器)。
pub async fn status(State(state): State<AppState>) -> Json<StatusJson> {
    Json(StatusJson {
        running: state.bus.is_monitoring(),
        total: state.bus.total_frames(),
        canopen: state.bus.canopen_count(),
        j1939: state.bus.j1939_count(),
        error: state.bus.error_count(),
        dropped: state.bus.dropped_frames(),
    })
}
