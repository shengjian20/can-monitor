//! # can-monitor-server — CAN 监控 Web 服务 (axum)
//!
//! 提供 HTTP + WebSocket 服务:
//!
//! - `GET /ws`: 把 [`MonitorBus`] 广播流中的
//!   [`StreamItem`](can_monitor_core::classifier::StreamItem) 以**批量帧数组**推给
//!   浏览器端 (帧 JSON 契约见 [`frame`] 模块, 三形态 TUI / Web / GUI 统一);
//! - REST 端点 (见 [`rest`]): 设备列表 / 监控开关 / 帧发送 / 状态查询;
//! - 静态文件: 托管 `web/dist` (相对仓库根), 目录不存在则跳过该路由
//!   (T18 前端落地前的容错)。
//!
//! ## 架构分层
//!
//! - [`frame`]: 帧 → JSON 纯函数 + 批量攒批逻辑 (可单测, 不依赖异步);
//! - [`ws`]: WebSocket 端点 + 同步→异步桥接线程 + 攒批转发循环;
//! - [`rest`]: REST 端点 + 写门控 (send 仅 `write_enabled` 时可用, 否则 403)。
//!
//! ## 写安全门控 (Metis 安全锁定)
//!
//! 帧发送属于**写操作**: 仅当 [`AppState::write_enabled`] 为真 (即 CLI 以
//! `--web-write` 启动) 时 `POST /api/send` 才可用, 否则返回 HTTP 403。
//! 服务默认只读, 且监听地址默认绑定本机回环 (见 [`parse_bind_addr`], 拒绝
//! 非 127.0.0.1 / localhost 绑定, 不提供 LAN / 0.0.0.0 暴露)。
//!
//! ## 依赖约束
//!
//! 本 crate 是核心流 (can-monitor-core) 的**异步出口**: core 保持同步且不引入
//! tokio, 所有异步边界 (HTTP / WebSocket / 定时刷批) 收敛在本 crate 内。

pub mod frame;
pub mod rest;
pub mod ws;

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use can_monitor_core::bus::MonitorBus;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;

pub use frame::{frame_to_json, BatchCollector, FrameJson};

/// axum 应用共享状态。
///
/// - `bus`: 与 TUI 主线程共享的 [`MonitorBus`] (Arc 持有, 读侧订阅 / 写侧发帧);
/// - `write_enabled`: 写门控 — `false` 时 `POST /api/send` 恒返回 403。
#[derive(Clone)]
pub struct AppState {
    /// 共享消息总线 (监控开关 / 计数器 / 帧发送)。
    pub bus: Arc<MonitorBus>,
    /// 写模式开关 (仅 `--web-write` 启动时为真)。
    pub write_enabled: bool,
}

/// 在指定地址启动 HTTP + WebSocket 服务 (阻塞直到服务退出)。
///
/// @param addr          监听地址 (测试可用 `127.0.0.1:0` 随机端口)。
/// @param bus           共享的消息总线。
/// @param write_enabled 写门控 (false 时 `/api/send` 返回 403)。
/// @return 绑定或运行期间出错返回 [`io::Error`]。
pub async fn serve(addr: SocketAddr, bus: Arc<MonitorBus>, write_enabled: bool) -> io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    serve_listener(listener, bus, write_enabled).await
}

/// 在已绑定的监听器上启动服务。
///
/// 与 [`serve`] 的区别: 监听器由调用方绑定, 便于先取到实际端口再启动
/// (测试典型用法: `TcpListener::bind("127.0.0.1:0")` → `local_addr()` → `serve_listener`)。
///
/// @param listener      已绑定的 tokio 监听器。
/// @param bus           共享的消息总线。
/// @param write_enabled 写门控 (false 时 `/api/send` 返回 403)。
/// @return 运行期间出错返回 [`io::Error`]。
pub async fn serve_listener(
    listener: TcpListener,
    bus: Arc<MonitorBus>,
    write_enabled: bool,
) -> io::Result<()> {
    let app = router(bus, write_enabled);
    axum::serve(listener, app)
        .await
        .map_err(|e| io::Error::other(e.to_string()))
}

/// 构造完整应用 Router: `/ws` 批量帧流 + REST 端点 + 静态文件回退。
///
/// 静态文件: `web/dist` 相对仓库根目录, 该目录存在时挂
/// [`ServeDir`] 作为 fallback (未匹配 REST/WS 的请求走静态文件);
/// 不存在则跳过 (T18 前端落地前不报错)。
///
/// @param bus           共享的消息总线。
/// @param write_enabled 写门控。
/// @return 已注入状态的 axum [`Router`]。
pub fn router(bus: Arc<MonitorBus>, write_enabled: bool) -> Router {
    let state = AppState { bus, write_enabled };

    let mut app = Router::new()
        .route("/ws", get(ws::ws_handler))
        .route("/api/devices", get(rest::get_devices))
        .route("/api/monitor/start", post(rest::start_monitor))
        .route("/api/monitor/stop", post(rest::stop_monitor))
        .route("/api/send", post(rest::send))
        .route("/api/status", get(rest::status))
        .layer(DefaultBodyLimit::max(1024 * 64)) // 帧体极小, 收紧请求体上限
        .with_state(state);

    // 静态文件: 相对仓库根; 不存在则跳过 (T18 落地前容错)。
    let dist = Path::new("web/dist");
    if dist.is_dir() {
        app = app.fallback_service(ServeDir::new(dist));
    }
    app
}

/// 解析 Web 监听地址 (Metis 安全锁定: **仅允许本机回环**)。
///
/// 格式 `host:port`; host 仅接受 `127.0.0.1` / `localhost` (大小写不敏感),
/// 其余一律拒绝 (包括 `0.0.0.0`、局域网 / 公网地址), 不提供 LAN 绑定。
/// `localhost` 归一化为 `127.0.0.1` (不做 DNS 解析)。
///
/// @param addr 监听地址字符串 (如 `"127.0.0.1:8080"` / `"localhost:8080"`)。
/// @return 解析成功返回回环 [`SocketAddr`]; 非法 host / 端口 / 格式返回错误描述。
pub fn parse_bind_addr(addr: &str) -> std::result::Result<SocketAddr, String> {
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| format!("地址缺少端口, 应为 host:port: {addr}"))?;
    let port: u16 = port
        .parse()
        .map_err(|_| format!("端口无效: {port} (应为 1-65535 的数字)"))?;
    match host.to_ascii_lowercase().as_str() {
        "127.0.0.1" | "localhost" => Ok(SocketAddr::from(([127, 0, 0, 1], port))),
        other => Err(format!(
            "仅允许绑定本机回环地址 (127.0.0.1 / localhost), 拒绝: {other} (Metis 安全锁定, 不提供 LAN 绑定)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合法回环地址: IPv4 字面量。
    #[test]
    fn bind_addr_accepts_loopback_ipv4() {
        let addr = parse_bind_addr("127.0.0.1:8080").unwrap();
        assert_eq!(addr, SocketAddr::from(([127, 0, 0, 1], 8080)));
    }

    /// 合法回环地址: localhost 主机名归一化为 127.0.0.1 (大小写不敏感)。
    #[test]
    fn bind_addr_accepts_localhost() {
        let addr = parse_bind_addr("LocalHost:9000").unwrap();
        assert_eq!(addr, SocketAddr::from(([127, 0, 0, 1], 9000)));
    }

    /// 拒绝非回环绑定: 0.0.0.0 (监听所有网卡)。
    #[test]
    fn bind_addr_rejects_wildcard() {
        let err = parse_bind_addr("0.0.0.0:8080").unwrap_err();
        assert!(err.contains("拒绝"), "错误应说明拒绝: {err}");
    }

    /// 拒绝非回环绑定: 局域网 / 公网地址。
    #[test]
    fn bind_addr_rejects_lan_and_public() {
        assert!(parse_bind_addr("192.168.1.1:8080").is_err());
        assert!(parse_bind_addr("10.0.0.5:8080").is_err());
        assert!(parse_bind_addr("8.8.8.8:8080").is_err());
    }

    /// 拒绝非法端口与缺失端口格式。
    #[test]
    fn bind_addr_rejects_bad_port() {
        assert!(parse_bind_addr("127.0.0.1:abc").is_err());
        assert!(parse_bind_addr("127.0.0.1:70000").is_err());
        assert!(parse_bind_addr("127.0.0.1").is_err(), "缺少端口应报错");
    }
}
