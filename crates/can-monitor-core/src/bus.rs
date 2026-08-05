//! # 消息总线
//!
//! 在后台 reader 线程中持续从后端读帧、分类, 并通过
//! [`StreamBroadcaster`](crate::broadcaster::StreamBroadcaster) 广播
//! 到每个消费者 (每消费者独立有界队列), 同时维护监控开关与各类计数器。
//!
//! ## 监控开关语义
//!
//! 监控开关 **默认关闭**: reader 线程只在
//! [`MonitorBus::set_monitoring`](crate::bus::MonitorBus::set_monitoring)
//! 显式开启后才开始消费后端帧; 关闭时线程休眠轮询开关, **不触碰后端**。广播
//! 采用有界队列 + `try_send` (非阻塞): 消费者慢时丢弃新帧, **绝不阻塞 reader**。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use can_types::{BackendKind, CanBackend, CanError, CanFrame, CanMessage, Direction};
use crossbeam_channel::{Receiver, Sender};

use crate::broadcaster::{ConsumerId, StreamBroadcaster};
use crate::classifier::{FrameClassifier, ParsedMessage, StreamItem};

/// 错误 channel 容量。
const ERROR_CHANNEL_CAPACITY: usize = 64;
/// 发送 channel 容量 (帧下发队列)。
const SEND_CHANNEL_CAPACITY: usize = 64;
/// reader 单次读帧阻塞上限。
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// 监控关闭时线程轮询开关的休眠间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 消息总线。
///
/// 由 [`MonitorBus::new`] 创建, 返回总线本体与供消费的两条 channel 接收端:
/// - `Receiver<StreamItem>`: 默认消费者的消息流 (元素为原始消息 + 一次分类
///   结果的打包 [`StreamItem`], 内部经 [`StreamBroadcaster`] 广播, 可再经
///   [`MonitorBus::subscribe`] 订阅更多消费者);
/// - `Receiver<String>`: reader 线程遇到的后端错误描述。
///
/// 总线本身不依赖具体后端: reader 线程通过 [`CanBackend`] trait 泛型接入。
pub struct MonitorBus {
    /// 消息广播器 (每消费者独立有界队列, reader 发布, 多前端消费)。
    broadcast: Arc<StreamBroadcaster<StreamItem>>,
    /// 错误投递发送端。
    err_tx: Sender<String>,
    /// 帧发送端 (TUI 下发面板 → reader 线程 → 后端写入)。
    send_tx: Sender<CanFrame>,
    /// 帧发送接收端 (reader 线程消费)。
    send_rx: Receiver<CanFrame>,
    /// 监控开关 (默认关闭)。
    running: Arc<AtomicBool>,
    /// 线程停机标志 (置真后 reader 线程退出)。
    shutdown: Arc<AtomicBool>,
    /// 已读帧总数。
    total_frames: Arc<AtomicU64>,
    /// 已识别为 CANopen 的帧数。
    canopen_count: Arc<AtomicU64>,
    /// 已识别为 J1939 的帧数。
    j1939_count: Arc<AtomicU64>,
    /// 后端错误数。
    error_count: Arc<AtomicU64>,
}

impl MonitorBus {
    /// 创建消息总线。
    ///
    /// 监控开关初始为**关闭** (需 [`MonitorBus::set_monitoring`] 显式开启);
    /// 各计数器归零。总线内部使用 [`StreamBroadcaster`] 广播: 创建时自动订阅
    /// 一个默认消费者, 其接收端作为三元组返回; 上层可经
    /// [`MonitorBus::subscribe`] 追加更多消费者。
    ///
    /// @return 三元组: (总线, 默认消费者消息接收端, 错误接收端)。
    pub fn new() -> (MonitorBus, Receiver<StreamItem>, Receiver<String>) {
        let broadcast = Arc::new(StreamBroadcaster::new());
        let (_default_id, rx) = broadcast.subscribe();
        let (err_tx, err_rx) = crossbeam_channel::bounded(ERROR_CHANNEL_CAPACITY);
        let (send_tx, send_rx) = crossbeam_channel::bounded(SEND_CHANNEL_CAPACITY);
        (
            MonitorBus {
                broadcast,
                err_tx,
                send_tx,
                send_rx,
                running: Arc::new(AtomicBool::new(false)),
                shutdown: Arc::new(AtomicBool::new(false)),
                total_frames: Arc::new(AtomicU64::new(0)),
                canopen_count: Arc::new(AtomicU64::new(0)),
                j1939_count: Arc::new(AtomicU64::new(0)),
                error_count: Arc::new(AtomicU64::new(0)),
            },
            rx,
            err_rx,
        )
    }

    /// 启动后台 reader 线程。
    ///
    /// 线程循环行为:
    /// - **监控关闭**时休眠轮询开关, 不读取后端 (不消费帧);
    /// - **监控开启**时以 `READ_TIMEOUT` (100ms) 阻塞读一帧, **分类恰好一次**,
    ///   将分类结果与原始消息打包为 [`StreamItem`] (方向恒为
    ///   [`Direction::Rx`]) **广播**到所有消费者
    ///   (每消费者独立有界队列, 队列满丢弃新帧, 绝不阻塞 reader),
    ///   并累计对应计数器;
    /// - 读取**超时** ([`CanError::Timeout`]) 视为正常, 直接继续下一轮;
    /// - 其他**后端错误**累计 `error_count` 并写入错误 channel 后继续。
    ///
    /// 调用 [`MonitorBus::shutdown`] 后线程在下一个循环检查点退出。
    ///
    /// @param backend    已打开的后端 (线程取得所有权)。
    /// @param classifier 共享的帧分类器 (线程内加锁串行使用)。
    /// @param source     消息来源后端种类 (标记在每条消息上)。
    /// @return 成功 `Ok(())`; 线程创建失败返回错误描述。
    pub fn start_reader<B>(
        &self,
        backend: B,
        classifier: Arc<Mutex<FrameClassifier>>,
        source: BackendKind,
    ) -> std::result::Result<(), String>
    where
        B: CanBackend + Send + 'static,
    {
        let broadcast = Arc::clone(&self.broadcast);
        let err_tx = self.err_tx.clone();
        let send_rx = self.send_rx.clone();
        let running = Arc::clone(&self.running);
        let shutdown = Arc::clone(&self.shutdown);
        let total = Arc::clone(&self.total_frames);
        let canopen = Arc::clone(&self.canopen_count);
        let j1939 = Arc::clone(&self.j1939_count);
        let errors = Arc::clone(&self.error_count);

        thread::Builder::new()
            .name("can-monitor-reader".to_string())
            .spawn(move || {
                let mut backend = backend;
                while !shutdown.load(Ordering::Relaxed) {
                    // 先排空发送队列, 保证监控关闭时下发的帧仍能写入后端。
                    while let Ok(frame) = send_rx.try_recv() {
                        if let Err(e) = backend.write_frame(&frame) {
                            errors.fetch_add(1, Ordering::Relaxed);
                            let _ = err_tx.try_send(format!("发送帧失败: {e}"));
                        }
                    }
                    if !running.load(Ordering::Relaxed) {
                        thread::sleep(POLL_INTERVAL);
                        continue;
                    }
                    match backend.read_frame(READ_TIMEOUT) {
                        Ok(frame) => {
                            total.fetch_add(1, Ordering::Relaxed);
                            // 分类恰好一次 (reader 是流路径上唯一的 classify 调用点,
                            // 消费者直接消费下面的 StreamItem, 不再重复分类)。
                            let parsed = classifier
                                .lock()
                                .unwrap_or_else(|poison| poison.into_inner())
                                .classify(&frame);
                            match &parsed {
                                ParsedMessage::Canopen { .. } => {
                                    canopen.fetch_add(1, Ordering::Relaxed);
                                }
                                ParsedMessage::J1939 { .. } => {
                                    j1939.fetch_add(1, Ordering::Relaxed);
                                }
                                ParsedMessage::Raw(_) => {}
                            }
                            let item = StreamItem {
                                msg: CanMessage::new(frame, source, Direction::Rx),
                                parsed,
                            };
                            // 广播: try_send 语义, 消费者慢时丢弃新帧, 绝不阻塞。
                            broadcast.publish(&item);
                        }
                        Err(CanError::Timeout) => {}
                        Err(e) => {
                            errors.fetch_add(1, Ordering::Relaxed);
                            let _ = err_tx.try_send(format!("后端读取错误: {e}"));
                        }
                    }
                }
            })
            .map(|_| ())
            .map_err(|e| format!("启动 reader 线程失败: {e}"))
    }

    /// 设置监控开关。
    ///
    /// @param enabled 开启 (`true`) 后 reader 线程开始消费后端帧; 关闭
    ///                (`false`) 后线程休眠, 不再读取后端。
    pub fn set_monitoring(&self, enabled: bool) {
        self.running.store(enabled, Ordering::Relaxed);
    }

    /// 查询监控开关状态。
    ///
    /// @return `true` 表示正在监控。
    pub fn is_monitoring(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// 请求 reader 线程退出。
    ///
    /// 置位停机标志, reader 线程在下一个循环检查点退出。
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// 已读帧总数。
    ///
    /// @return 自启动以来成功从后端读到的帧数。
    pub fn total_frames(&self) -> u64 {
        self.total_frames.load(Ordering::Relaxed)
    }

    /// 已识别为 CANopen 的帧数。
    ///
    /// @return 分类为 [`ParsedMessage::Canopen`] 的帧数。
    pub fn canopen_count(&self) -> u64 {
        self.canopen_count.load(Ordering::Relaxed)
    }

    /// 已识别为 J1939 的帧数。
    ///
    /// @return 分类为 [`ParsedMessage::J1939`] 的帧数。
    pub fn j1939_count(&self) -> u64 {
        self.j1939_count.load(Ordering::Relaxed)
    }

    /// 后端错误数。
    ///
    /// @return 读取后端时发生非超时错误的次数。
    pub fn error_count(&self) -> u64 {
        self.error_count.load(Ordering::Relaxed)
    }

    /// 订阅广播流, 获取独立的消息接收端 (供多前端 / 多消费者使用)。
    ///
    /// 每个消费者拥有独立有界队列; 消费者慢时丢弃新帧, 不影响 reader 线程。
    ///
    /// @return 二元组: (消费者标识, 该消费者的消息接收端)。
    pub fn subscribe(&self) -> (ConsumerId, Receiver<StreamItem>) {
        self.broadcast.subscribe()
    }

    /// 订阅广播流并指定消费者队列容量 (有界)。
    ///
    /// 用于为高频消费者预留更大缓冲, 或测试小容量队列的丢弃行为。
    ///
    /// @param capacity 消费者队列容量。
    /// @return 二元组: (消费者标识, 该消费者的消息接收端)。
    pub fn subscribe_with_capacity(&self, capacity: usize) -> (ConsumerId, Receiver<StreamItem>) {
        self.broadcast.subscribe_with_capacity(capacity)
    }

    /// 显式取消订阅 (或直接 drop 接收端, 发布时惰性回收)。
    ///
    /// @param id 消费者标识。
    /// @return `true` 表示该消费者存在并被移除。
    pub fn unsubscribe(&self, id: ConsumerId) -> bool {
        self.broadcast.unsubscribe(id)
    }

    /// 因消费者队列已满而丢弃的帧总数。
    ///
    /// @return 广播路径丢弃计数 (供状态栏 / UI 展示慢消费者压力)。
    pub fn dropped_frames(&self) -> u64 {
        self.broadcast.dropped()
    }

    /// 成功投递到消费者队列的帧总数。
    ///
    /// @return 广播路径成功投递计数。
    pub fn consumed_frames(&self) -> u64 {
        self.broadcast.consumed()
    }

    /// 向总线发送一帧 (TUI 下发面板调用)。
    ///
    /// 帧通过 channel 投递到 reader 线程, 由 reader 调用后端写入; channel 满时
    /// 返回错误 (非阻塞)。
    ///
    /// @param frame 待发送的 CAN 帧。
    /// @return 成功 `Ok(())`; channel 满或已关闭返回错误描述。
    pub fn send_frame(&self, frame: CanFrame) -> std::result::Result<(), String> {
        self.send_tx
            .try_send(frame)
            .map_err(|e| format!("发送队列: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_types::{BackendConfig, CanFrame};
    use std::collections::VecDeque;
    use std::time::Instant;

    /// 构造标准帧。
    fn frame(id: u16, data: &[u8]) -> CanFrame {
        CanFrame::new(can_types::CanId::new_standard(id).unwrap(), data.to_vec()).unwrap()
    }

    /// 构造扩展帧。
    fn frame_ext(id: u32, data: &[u8]) -> CanFrame {
        CanFrame::new(can_types::CanId::new_extended(id).unwrap(), data.to_vec()).unwrap()
    }

    /// 等待条件成立 (带 5 秒超时, 避免测试挂死)。
    fn wait_until(cond: impl FnMut() -> bool) {
        let mut cond = cond;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "等待条件超时");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// 测试桩后端: 按队列依次返回预置帧 (或错误), 队列空时阻塞到超时。
    #[derive(Clone)]
    struct MockBackend {
        frames: Arc<Mutex<VecDeque<can_types::Result<CanFrame>>>>,
    }

    impl MockBackend {
        /// 用预置帧序列构造测试桩。
        fn new(frames: Vec<can_types::Result<CanFrame>>) -> Self {
            Self {
                frames: Arc::new(Mutex::new(frames.into_iter().collect())),
            }
        }

        /// 向队列尾部追加一条结果 (帧或错误)。
        fn push(&self, result: can_types::Result<CanFrame>) {
            self.frames.lock().unwrap().push_back(result);
        }

        /// 队列中剩余可读的结果数量。
        fn remaining(&self) -> usize {
            self.frames.lock().unwrap().len()
        }
    }

    impl CanBackend for MockBackend {
        fn open(_config: &BackendConfig) -> can_types::Result<Self> {
            Ok(Self {
                frames: Arc::new(Mutex::new(VecDeque::new())),
            })
        }

        fn read_frame(&mut self, timeout: Duration) -> can_types::Result<CanFrame> {
            let mut q = self.frames.lock().unwrap();
            match q.pop_front() {
                Some(result) => result,
                None => {
                    drop(q);
                    thread::sleep(timeout);
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

    /// 监控开关默认关闭, 计数器归零。
    #[test]
    fn default_monitoring_off() {
        let (bus, _rx, _err_rx) = MonitorBus::new();
        assert!(!bus.is_monitoring());
        assert_eq!(bus.total_frames(), 0);
        assert_eq!(bus.canopen_count(), 0);
        assert_eq!(bus.j1939_count(), 0);
        assert_eq!(bus.error_count(), 0);
    }

    /// set_monitoring 开关切换生效。
    #[test]
    fn monitoring_switch_toggles() {
        let (bus, _rx, _err_rx) = MonitorBus::new();
        bus.set_monitoring(true);
        assert!(bus.is_monitoring());
        bus.set_monitoring(false);
        assert!(!bus.is_monitoring());
    }

    /// 开启监控后 reader 消费帧并正确分类; 关闭后停止消费, 计数冻结。
    #[test]
    fn reader_consumes_and_stops() {
        let (bus, rx, _err_rx) = MonitorBus::new();
        let backend = MockBackend::new(vec![
            Ok(frame(0x181, &[1, 2, 3])),    // CANopen TPDO1
            Ok(frame_ext(0x18FEF100, &[1])), // J1939 (PGN 0xFEF1)
            Ok(frame(0x101, &[1])),          // 未知 11 位 → Raw
        ]);
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        bus.start_reader(backend.clone(), classifier, BackendKind::SocketCan)
            .unwrap();

        // 默认关闭: 不应消费任何帧。
        thread::sleep(Duration::from_millis(200));
        assert_eq!(bus.total_frames(), 0);
        assert_eq!(backend.remaining(), 3);

        // 开启监控: 消费全部 3 帧并送达消息 channel。
        bus.set_monitoring(true);
        wait_until(|| rx.len() >= 3);

        assert_eq!(bus.total_frames(), 3);
        assert_eq!(bus.canopen_count(), 1);
        assert_eq!(bus.j1939_count(), 1);
        assert_eq!(bus.error_count(), 0);
        assert_eq!(backend.remaining(), 0);

        // 校验消息内容: 流元素同时携带原始消息与分类结果。
        let received: Vec<StreamItem> = rx.try_iter().collect();
        assert_eq!(received.len(), 3);
        assert_eq!(received[0].msg.frame.id().raw_id(), 0x181);
        assert_eq!(received[0].msg.source, BackendKind::SocketCan);
        assert_eq!(received[0].msg.direction, Direction::Rx);
        // 分类结果随流元素下发。
        assert_eq!(
            received[0].parsed.protocol(),
            crate::classifier::Protocol::Canopen
        );
        assert_eq!(
            received[1].parsed.protocol(),
            crate::classifier::Protocol::J1939
        );
        assert_eq!(
            received[2].parsed.protocol(),
            crate::classifier::Protocol::Raw
        );

        // 关闭监控: 追加帧不被消费, 计数冻结。
        bus.set_monitoring(false);
        backend.push(Ok(frame_ext(0x18F00480, &[9, 9])));
        backend.push(Ok(frame(0x181, &[7])));
        thread::sleep(Duration::from_millis(200));
        assert_eq!(bus.total_frames(), 3);
        assert_eq!(backend.remaining(), 2);

        bus.shutdown();
    }

    /// 单帧 → 恰好一次 classify: 喂 1 帧心跳, 节点健康状态只推进一次。
    ///
    /// reader 线程是整条流 (读帧 → 分类 → 发布 → 消费) 上唯一的 classify
    /// 调用点 (消费者已不持有分类器), 因此一帧必然恰好被分类一次。此处用心跳的
    /// observable 副作用 (节点健康状态) 验证分类确实发生且只推进到对应状态。
    #[test]
    fn single_frame_classifies_exactly_once() {
        let (bus, rx, _err_rx) = MonitorBus::new();
        // 单帧: CANopen 心跳 node5 → Operational。
        let backend = MockBackend::new(vec![Ok(frame(0x705, &[0x05]))]);
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        bus.start_reader(backend, Arc::clone(&classifier), BackendKind::SocketCan)
            .unwrap();

        bus.set_monitoring(true);
        wait_until(|| !rx.is_empty());

        // 恰好一帧进入流, 分类结果随元素下发。
        let items: Vec<StreamItem> = rx.try_iter().collect();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].msg.frame.id().raw_id(), 0x705);
        assert_eq!(
            items[0].parsed.protocol(),
            crate::classifier::Protocol::Canopen
        );

        // 分类只发生了一次: 心跳被 observe 后节点状态推进到 Operational。
        let classifier = classifier.lock().unwrap();
        assert_eq!(
            classifier.node_state(5),
            Some(canopen_stack::NmtState::Operational)
        );
        // 未喂心跳的节点不产生任何状态。
        assert_eq!(classifier.node_state(6), None);

        bus.shutdown();
    }

    /// 后端错误会计数并写入错误 channel, 随后仍能继续读帧。
    #[test]
    fn backend_errors_are_reported() {
        let (bus, rx, err_rx) = MonitorBus::new();
        let backend = MockBackend::new(vec![
            Err(CanError::BusError),
            Ok(frame_ext(0x18F00480, &[1, 2])),
        ]);
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        bus.start_reader(backend, classifier, BackendKind::UsbVci)
            .unwrap();

        bus.set_monitoring(true);
        wait_until(|| !rx.is_empty());

        assert_eq!(bus.error_count(), 1);
        assert_eq!(bus.total_frames(), 1);
        assert_eq!(bus.j1939_count(), 1);

        let errs: Vec<String> = err_rx.try_iter().collect();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("总线错误"));

        bus.shutdown();
    }

    /// 慢消费者不消费 → 其队列满后 drop 计数增长, 且 reader 永不阻塞
    /// (仍能读完所有后续帧, 其他消费者照常收流)。
    #[test]
    fn slow_consumer_never_blocks_reader() {
        let (bus, _slow_rx, _err_rx) = MonitorBus::new(); // 慢消费者: 默认接收端, 不消费
                                                          // 快消费者: 大容量队列, 排除 "排空线程暂时落后导致自身丢帧" 的竞态。
        let (_fast_id, fast_rx) = bus.subscribe_with_capacity(100_000);

        // 构造 3000 帧 (远超默认队列容量 1024, 足以填满慢消费者队列)。
        let mut results = Vec::new();
        for i in 0..3000u32 {
            results.push(Ok(frame(0x181, &i.to_le_bytes())));
        }
        let backend = MockBackend::new(results);
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        bus.start_reader(backend.clone(), classifier, BackendKind::SocketCan)
            .unwrap();

        // 快消费者在线程中排空所有帧 (带超时, 避免测试失败时挂死)。
        let drainer = std::thread::spawn(move || {
            let mut count = 0;
            let deadline = Instant::now() + Duration::from_secs(10);
            while count < 3000 && Instant::now() < deadline {
                match fast_rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(_) => count += 1,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
            count
        });

        bus.set_monitoring(true);

        // reader 读完全部 3000 帧 (若广播路径阻塞过, 这里必然超时)。
        wait_until(|| bus.total_frames() >= 3000);
        assert_eq!(backend.remaining(), 0);

        // 快消费者收到全部 3000 帧。
        assert_eq!(drainer.join().unwrap(), 3000);

        // 慢消费者队列满, 丢弃 3000 - 1024 = 1976 帧。
        assert_eq!(
            bus.dropped_frames(),
            3000 - crate::broadcaster::DEFAULT_QUEUE_CAPACITY as u64
        );
        assert_eq!(
            bus.consumed_frames(),
            3000 + crate::broadcaster::DEFAULT_QUEUE_CAPACITY as u64
        );

        bus.shutdown();
    }
}
