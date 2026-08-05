//! # can-server — CAN 监控 Web 服务 (axum)
//!
//! 提供 HTTP + WebSocket 服务: `GET /ws` 端点把 [`MonitorBus`] 广播流中的
//! [`StreamItem`](can_monitor_core::classifier::StreamItem) 以**批量帧数组**推给
//! 浏览器端 (帧 JSON 契约见 [`frame`] 模块, 三形态 TUI / Web / GUI 统一)。
//!
//! ## 架构分层
//!
//! - [`frame`]: 帧 → JSON 纯函数 + 批量攒批逻辑 (可单测, 不依赖异步);
//! - [`ws`]: WebSocket 端点 + 同步→异步桥接线程 + 攒批转发循环。
//!
//! REST 设备列表 / send / start / stop 由后续任务 (T16) 在此基座上扩展。
//!
//! ## 依赖约束
//!
//! 本 crate 是核心流 (can-monitor-core) 的**异步出口**: core 保持同步且不引入
//! tokio, 所有异步边界 (HTTP / WebSocket / 定时刷批) 收敛在本 crate 内。

pub mod frame;
pub mod ws;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use can_monitor_core::bus::MonitorBus;
use tokio::net::TcpListener;

pub use frame::{frame_to_json, BatchCollector, FrameJson};

/// 在指定地址启动 HTTP + WebSocket 服务 (阻塞直到服务退出)。
///
/// @param addr 监听地址 (测试可用 `127.0.0.1:0` 随机端口)。
/// @param bus  共享的消息总线。
/// @return 绑定或运行期间出错返回 [`io::Error`]。
pub async fn serve(addr: SocketAddr, bus: Arc<MonitorBus>) -> io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    serve_listener(listener, bus).await
}

/// 在已绑定的监听器上启动服务。
///
/// 与 [`serve`] 的区别: 监听器由调用方绑定, 便于先取到实际端口再启动
/// (测试典型用法: `TcpListener::bind("127.0.0.1:0")` → `local_addr()` → `serve_listener`)。
///
/// @param listener 已绑定的 tokio 监听器。
/// @param bus      共享的消息总线。
/// @return 运行期间出错返回 [`io::Error`]。
pub async fn serve_listener(listener: TcpListener, bus: Arc<MonitorBus>) -> io::Result<()> {
    let app = ws::router(bus);
    axum::serve(listener, app)
        .await
        .map_err(|e| io::Error::other(e.to_string()))
}
