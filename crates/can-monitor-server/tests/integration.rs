//! # 集成测试: WS 批量帧流
//!
//! 场景: 起服务到 ephemeral 端口 (127.0.0.1:0) + 内存 FakeBackend 喂 2 帧
//! (CANopen 0x181 / J1939 0x18FEF100) → tokio-tungstenite 客户端连 `/ws` →
//! 断言收到的批量 JSON 数组各字段符合契约, 并验证空闲时收到空数组心跳。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use can_monitor_core::bus::MonitorBus;
use can_monitor_core::classifier::FrameClassifier;
use can_monitor_server::serve_listener;
use can_monitor_server::FrameJson;
use can_types::{BackendConfig, BackendKind, CanBackend, CanError, CanFrame, CanId};
use futures_util::StreamExt;
use tokio::net::TcpListener;

/// 最小内存假后端: read_frame 从队列弹帧, 队列空则阻塞到超时。
///
/// 与 can-monitor-core 测试模块里的 MockBackend 同构 (该实现仅在 core 的
/// `#[cfg(test)]` 内, 不可复用), 这里在 can-server 内独立实现一个最小版本。
struct FakeBackend {
    frames: Mutex<VecDeque<CanFrame>>,
}

impl FakeBackend {
    /// 用预置帧序列构造假后端。
    fn new(frames: Vec<CanFrame>) -> Self {
        Self {
            frames: Mutex::new(frames.into_iter().collect()),
        }
    }
}

impl CanBackend for FakeBackend {
    fn open(_config: &BackendConfig) -> can_types::Result<Self> {
        Ok(Self {
            frames: Mutex::new(VecDeque::new()),
        })
    }

    fn read_frame(&mut self, timeout: Duration) -> can_types::Result<CanFrame> {
        match self.frames.lock().unwrap().pop_front() {
            Some(frame) => Ok(frame),
            None => {
                std::thread::sleep(timeout);
                Err(CanError::Timeout)
            }
        }
    }

    fn write_frame(&mut self, _frame: &CanFrame) -> can_types::Result<()> {
        Ok(())
    }

    fn close(&mut self) -> can_types::Result<()> {
        Ok(())
    }
}

/// 构造标准帧。
fn frame(id: u16, data: &[u8]) -> CanFrame {
    CanFrame::new(CanId::new_standard(id).unwrap(), data.to_vec()).unwrap()
}

/// 构造扩展帧。
fn frame_ext(id: u32, data: &[u8]) -> CanFrame {
    CanFrame::new(CanId::new_extended(id).unwrap(), data.to_vec()).unwrap()
}

/// 起服务 + 2 帧假后端 → 连 /ws → 收到批量 JSON, 断言字段契约。
#[tokio::test]
async fn ws_pushes_batched_frames_matching_contract() {
    // 总线 + 2 帧 FakeBackend (CANopen + J1939)。
    let (bus, _default_rx, _err_rx) = MonitorBus::new();
    let backend = FakeBackend::new(vec![
        frame(0x181, &[1, 2, 3]),             // → CANopen TPDO1
        frame_ext(0x18FEF100, &[0x01, 0x02]), // → J1939 Direct (PGN 0xFEF1)
    ]);
    let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
    bus.start_reader(backend, classifier, BackendKind::SocketCan)
        .unwrap();

    // ephemeral 端口启动服务。
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bus = Arc::new(bus);
    let server = tokio::spawn(serve_listener(listener, Arc::clone(&bus), false));

    // tokio-tungstenite 客户端连 /ws。
    let url = format!("ws://{addr}/ws");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // 开启监控, 累积读取直到收到 2 帧 (可能跨多个批量 / 心跳)。
    bus.set_monitoring(true);
    let mut frames: Vec<FrameJson> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while frames.len() < 2 && tokio::time::Instant::now() < deadline {
        match ws.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                // 空数组心跳不贡献帧。
                let batch: Vec<FrameJson> = serde_json::from_str(&text).unwrap();
                frames.extend(batch);
            }
            Some(Ok(_)) | None => break,
            Some(Err(_)) => break,
        }
    }

    // 断言收到两帧且字段符合 T17 契约。
    assert_eq!(frames.len(), 2, "应收到 2 帧, 实际: {frames:?}");

    let canopen = frames
        .iter()
        .find(|f| f.protocol == "canopen")
        .expect("应收到 CANopen 帧");
    assert_eq!(canopen.id, "0x181");
    assert!(!canopen.ext);
    assert_eq!(canopen.dir, "rx");
    assert_eq!(canopen.data, "01 02 03");
    assert!(
        canopen.summary.contains("Pdo"),
        "摘要应含解析结果: {}",
        canopen.summary
    );
    assert!(
        canopen.ts.parse::<u64>().is_ok(),
        "ts 应为 u64 毫秒字符串: {}",
        canopen.ts
    );

    let j1939 = frames
        .iter()
        .find(|f| f.protocol == "j1939")
        .expect("应收到 J1939 帧");
    assert_eq!(j1939.id, "0x18FEF100");
    assert!(j1939.ext);
    assert_eq!(j1939.dir, "rx");
    assert_eq!(j1939.data, "01 02");
    assert!(
        j1939.summary.contains("Direct"),
        "摘要应含解析结果: {}",
        j1939.summary
    );
    assert!(j1939.ts.parse::<u64>().is_ok());

    // 空闲心跳: 两帧消费完后继续读, 应收到空数组 ([])。
    let mut saw_heartbeat = false;
    let heartbeat_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < heartbeat_deadline {
        match ws.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                if text.trim() == "[]" {
                    saw_heartbeat = true;
                    break;
                }
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => break,
        }
    }
    assert!(saw_heartbeat, "空闲时应收到空数组心跳");

    // 清理: 关闭 reader 与服务。
    bus.shutdown();
    server.abort();
}
