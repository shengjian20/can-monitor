//! # WebSocket 帧流端点
//!
//! `GET /ws`: 客户端连入后, 服务器订阅 [`MonitorBus`] 广播流, 把
//! [`StreamItem`] 经桥接线程
//! 转换为 [`FrameJson`], 再以**批量数组**推给浏览器:
//! 每 `FLUSH_INTERVAL` (30ms) 到点刷一次, 或攒满 `BATCH_MAX` (50) 帧立即刷出。
//!
//! ## 桥接设计 (同步 → 异步)
//!
//! ```text
//! crossbeam 广播接收端 (同步) ──桥接线程──▶ tokio mpsc (异步) ──▶ WS 转发任务
//! ```
//!
//! - **桥接线程** (`std::thread`): 阻塞 `recv_timeout(30ms)` 读 crossbeam 接收端,
//!   每收到一帧转 JSON 后 `try_send` 进 mpsc 队列 —— 队列满时丢弃新帧 (与广播层
//!   "慢消费者丢弃" 语义一致), **绝不阻塞** reader / 桥接线程;
//! - **WS 转发任务** (`async`): `tokio::select!` 同时监听 mpsc、30ms 定时器与
//!   客户端消息; 攒批刷出或发空数组心跳。
//!
//! ## 生命周期与清理
//!
//! - WS 任务退出 (客户端断开 / 发送失败 / mpsc 关闭) → 调用
//!   [`MonitorBus::unsubscribe`] 清理订阅 → crossbeam 发送端被 drop → 桥接线程
//!   在 `recv_timeout` 返回 `Disconnected` 后退出 → 其持有的 mpsc Sender 被 drop
//!   → WS 任务收 `None` 关闭连接;
//! - 反向: 桥接线程先退出 (广播端断开) → mpsc 关闭 → WS 任务收 `None` 退出。
//!
//! 帧从广播队列到 WS 发出, 全程**不重新分类** (直接用 `StreamItem.parsed`)。

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use can_monitor_core::bus::MonitorBus;
use can_monitor_core::classifier::StreamItem;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::{interval, MissedTickBehavior};

use crate::frame::{frame_to_json, BatchCollector, FrameJson};

/// 批量刷出间隔 (与 Web 端 30ms 渲染帧率匹配, 兼顾实时性与批处理开销)。
const FLUSH_INTERVAL: Duration = Duration::from_millis(30);
/// 批量刷出上限 (攒满即刷, 不等定时器)。
const BATCH_MAX: usize = 50;
/// 桥接 mpsc 队列容量 (WS 任务短暂落后时暂存; 满则丢新帧, 语义同广播层)。
const BRIDGE_QUEUE_CAPACITY: usize = 256;
/// 桥接线程阻塞读 crossbeam 的超时 (近似"非阻塞"轮询语义, 可及时感知断开)。
const BRIDGE_POLL_TIMEOUT: Duration = Duration::from_millis(30);

/// 构造带 `/ws` 路由的服务 Router (共享总线作为 axum State)。
///
/// @param bus 共享的消息总线 (读侧订阅广播流)。
/// @return 已注入状态的 axum [`Router`]。
pub fn router(bus: Arc<MonitorBus>) -> Router {
    Router::new().route("/ws", get(ws_handler)).with_state(bus)
}

/// `GET /ws` 升级处理器。
///
/// 把连接升级为 WebSocket 后, 在独立任务中运行会话循环
/// (升级回调由 axum 经 `tokio::spawn` 分离执行, 不阻塞请求处理)。
async fn ws_handler(State(bus): State<Arc<MonitorBus>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| ws_session(socket, bus))
}

/// 单个 WS 会话: 建桥接线程 + 运行攒批转发循环。
///
/// @param socket 已升级的 WebSocket 连接。
/// @param bus    共享消息总线。
async fn ws_session(socket: WebSocket, bus: Arc<MonitorBus>) {
    // 订阅广播流, 取得该消费者的独立接收端 (有界队列, 慢则丢帧)。
    let (consumer_id, rx) = bus.subscribe();
    let (bridge_tx, mut mpsc_rx) = mpsc::channel::<FrameJson>(BRIDGE_QUEUE_CAPACITY);

    // 桥接线程: 同步 crossbeam 接收端 → 异步 mpsc 队列。
    let bridge = match thread::Builder::new()
        .name("can-ws-bridge".to_string())
        .spawn(move || bridge_loop(rx, bridge_tx))
    {
        Ok(handle) => handle,
        Err(_) => {
            // 线程创建失败: 立即退订, 不 panic。
            bus.unsubscribe(consumer_id);
            return;
        }
    };

    // 攒批转发循环 (socket / mpsc / 定时器三方选择)。
    let mut socket = socket;
    let mut collector = BatchCollector::new(BATCH_MAX);
    let mut ticker = interval(FLUSH_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // 桥接线程送来的新帧。
            maybe = mpsc_rx.recv() => {
                match maybe {
                    Some(json) => {
                        // 攒批: 达上限立即刷出, 否则等定时器到点。
                        if collector.push(json)
                            && socket.send(Message::Text(batch_text(&mut collector).into())).await.is_err()
                        {
                            break; // 发送失败 → 客户端已断开。
                        }
                    }
                    None => break, // 桥接线程已退出 (广播端断开) → 关闭连接。
                }
            }
            // 定时器到点: 刷出攒批; 无帧则发空数组心跳保活。
            _ = ticker.tick() => {
                let payload = if collector.is_empty() {
                    // 心跳/空批量: 让前端定时器稳定走渲染循环, 同时保活连接
                    // (选择发送空数组而非静默, 便于浏览器区分"连接活着但无新帧")。
                    "[]".to_string()
                } else {
                    batch_text(&mut collector)
                };
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    break;
                }
            }
            // 客户端消息: 回 Pong, 其余忽略; 关闭/出错则退出。
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Ping(p))) => {
                        if socket.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break, // 客户端主动关闭。
                    Some(Ok(_)) => {}                            // 其他消息忽略 (单向下行流)。
                    Some(Err(_)) => break,
                }
            }
        }
    }

    // 清理: 退订广播 (crossbeam 发送端 drop) → 桥接线程随后退出。
    bus.unsubscribe(consumer_id);
    // 丢弃 JoinHandle (桥接线程已由 crossbeam 断开驱动退出, 无需 join)。
    drop(bridge);
}

/// 桥接线程主体: 阻塞读 crossbeam 广播接收端, 逐帧转 JSON 推入 mpsc。
///
/// @param rx  crossbeam 广播接收端 (本连接专属消费者队列)。
/// @param tx  tokio mpsc 发送端 (WS 任务消费)。
fn bridge_loop(rx: Receiver<StreamItem>, tx: mpsc::Sender<FrameJson>) {
    loop {
        match rx.recv_timeout(BRIDGE_POLL_TIMEOUT) {
            Ok(item) => {
                let json = frame_to_json(&item);
                match tx.try_send(json) {
                    Ok(()) => {}
                    // WS 落后: 队列满, 丢弃新帧 (与广播层慢消费者语义一致)。
                    Err(TrySendError::Full(_)) => {}
                    // WS 已断开: 接收端被 drop, 桥接线程退出。
                    Err(TrySendError::Closed(_)) => break,
                }
            }
            Err(RecvTimeoutError::Timeout) => continue,
            // 广播端断开 (本连接被退订 / 总线销毁): 退出, 随后 drop mpsc Sender。
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// 序列化攒批缓冲为 JSON 数组文本并清空缓冲。
///
/// @param collector 攒批器 (其缓冲被取走并序列化)。
/// @return WS 文本帧载荷 (序列化失败回退空数组, 不 panic)。
fn batch_text(collector: &mut BatchCollector) -> String {
    serde_json::to_string(&collector.take()).unwrap_or_else(|_| "[]".to_string())
}
