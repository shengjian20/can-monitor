//! # 流广播器 (单生产者 → 多消费者)
//!
//! 为上层多个前端 (TUI / Web / Tauri) 提供**一对多**消息分发: 一个生产者
//! (reader 线程) 通过
//! [`StreamBroadcaster::publish`](crate::broadcaster::StreamBroadcaster::publish)
//! 广播消息, 每个消费者通过
//! [`StreamBroadcaster::subscribe`](crate::broadcaster::StreamBroadcaster::subscribe)
//! 拿到**独立的有界队列**接收端。
//!
//! ## 背压策略 (核心约束: 生产者永不阻塞)
//!
//! 每消费者队列采用 `crossbeam_channel::bounded` (默认
//! [`DEFAULT_QUEUE_CAPACITY`](crate::broadcaster::DEFAULT_QUEUE_CAPACITY)
//! = 1024)。生产者发布时对每个消费者执行 **`try_send` (非阻塞)**:
//! - 队列未满 → 投递成功, `consumed` 计数 +1;
//! - 队列已满 (消费者慢) → **丢弃新帧**, `dropped` 计数 +1, 绝不阻塞生产者;
//! - 接收端已关闭 (消费者 drop) → 惰性移除该消费者。
//!
//! 因此慢消费者最多影响自己的可见窗口 (丢失最新帧), 不会拖垮 reader 线程。
//!
//! ## 线程安全
//!
//! 内部用 `Mutex<HashMap<ConsumerId, Sender<T>>>` 维护消费者表, 计数器全部走
//! 原子变量; 发布路径持锁时间极短且只做非阻塞 `try_send`, 适合高频 CAN 帧流。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender, TrySendError};

/// 消费者唯一标识。
pub type ConsumerId = u64;

/// 消费者队列默认容量 (有界, 防止无界增长导致内存泄漏)。
pub const DEFAULT_QUEUE_CAPACITY: usize = 1024;

/// 流广播器。
///
/// 单生产者多消费者的一对多分发通道: 生产者调用 [`StreamBroadcaster::publish`]
/// 广播, 每个消费者经 [`StreamBroadcaster::subscribe`] 订阅独立的有界队列。
/// 消费者接收端被 drop 后, 发布时惰性回收, 无需显式反注册。
pub struct StreamBroadcaster<T> {
    /// 消费者表: 标识 → 各自队列的发送端。
    consumers: Mutex<HashMap<ConsumerId, Sender<T>>>,
    /// 消费者标识分配器 (自增)。
    next_id: AtomicU64,
    /// 已发布消息总数。
    published: AtomicU64,
    /// 成功投递到消费者队列的消息总数 (含被消费/待消费的)。
    consumed: AtomicU64,
    /// 因消费者队列已满而丢弃的消息总数。
    dropped: AtomicU64,
}

impl<T> StreamBroadcaster<T>
where
    T: Clone,
{
    /// 创建空广播器 (无消费者)。
    ///
    /// 计数器归零。
    ///
    /// @return 广播器实例。
    pub fn new() -> Self {
        Self {
            consumers: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            published: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// 订阅广播流, 使用默认队列容量。
    ///
    /// 每个订阅者获得**独立的有界队列**, 互不影响; 消费者接收端被 drop 后,
    /// 下一次 [`StreamBroadcaster::publish`] 会惰性回收其发送端。
    ///
    /// @return 二元组: (消费者标识, 该消费者的消息接收端)。
    pub fn subscribe(&self) -> (ConsumerId, Receiver<T>) {
        self.subscribe_with_capacity(DEFAULT_QUEUE_CAPACITY)
    }

    /// 订阅广播流, 指定消费者队列容量。
    ///
    /// 用于测试小容量队列的丢弃行为, 或为高频消费者预留更大缓冲。
    ///
    /// @param capacity 消费者队列容量 (有界)。
    /// @return 二元组: (消费者标识, 该消费者的消息接收端)。
    pub fn subscribe_with_capacity(&self, capacity: usize) -> (ConsumerId, Receiver<T>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = crossbeam_channel::bounded(capacity);
        self.consumers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(id, tx);
        (id, rx)
    }

    /// 广播一条消息到所有消费者 (绝不阻塞)。
    ///
    /// 对每个消费者执行非阻塞 `try_send`:
    /// - 队列未满 → 投递成功, `consumed` +1;
    /// - 队列已满 → 丢弃该帧, `dropped` +1;
    /// - 接收端已关闭 → 惰性移除该消费者。
    ///
    /// 生产者在慢消费者存在时仍以恒定速率运行。
    ///
    /// @param msg 待广播的消息 (克隆投递给每个消费者)。
    pub fn publish(&self, msg: &T) {
        self.published.fetch_add(1, Ordering::Relaxed);
        let mut consumers = self
            .consumers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut stale: Vec<ConsumerId> = Vec::new();
        for (&id, tx) in consumers.iter() {
            match tx.try_send(msg.clone()) {
                Ok(()) => {
                    self.consumed.fetch_add(1, Ordering::Relaxed);
                }
                Err(TrySendError::Full(_)) => {
                    // 消费者队列已满: 丢弃新帧, 不阻塞生产者。
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => {
                    // 消费者已 drop: 标记惰性清理。
                    stale.push(id);
                }
            }
        }
        for id in stale {
            consumers.remove(&id);
        }
    }

    /// 显式取消订阅。
    ///
    /// 移除消费者并使其接收端在后续发布中不再收到消息。
    ///
    /// @param id 消费者标识。
    /// @return `true` 表示该消费者存在并被移除; `false` 表示不存在 (或已回收)。
    pub fn unsubscribe(&self, id: ConsumerId) -> bool {
        self.consumers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&id)
            .is_some()
    }

    /// 当前存活消费者数量。
    ///
    /// @return 消费者表大小。
    pub fn subscriber_count(&self) -> usize {
        self.consumers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
    }

    /// 已发布消息总数。
    ///
    /// @return 自创建以来 [`StreamBroadcaster::publish`] 调用次数。
    pub fn published(&self) -> u64 {
        self.published.load(Ordering::Relaxed)
    }

    /// 成功投递 (消费) 计数。
    ///
    /// @return 成功进入消费者队列的消息总数。
    pub fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Relaxed)
    }

    /// 丢弃计数。
    ///
    /// @return 因消费者队列已满而丢弃的消息总数。
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl<T: Clone> Default for StreamBroadcaster<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// 等待条件成立 (带 5 秒超时, 避免测试挂死)。
    fn wait_until(cond: impl FnMut() -> bool) {
        let mut cond = cond;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "等待条件超时");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// (1) 慢消费者 A 不消费, 消费者 B 正常收到所有广播帧。
    #[test]
    fn slow_consumer_does_not_affect_fast_consumer() {
        let b: StreamBroadcaster<u64> = StreamBroadcaster::new();
        let (_a_id, _a_rx) = b.subscribe(); // A: 不消费
        let (_b_id, b_rx) = b.subscribe(); // B: 正常消费

        for i in 0..10 {
            b.publish(&i);
        }

        // B 收到全部 10 帧; A 未消费但队列未满, 无丢弃。
        let received: Vec<u64> = b_rx.try_iter().collect();
        assert_eq!(received.len(), 10);
        assert_eq!(received, (0..10).collect::<Vec<u64>>());
        assert_eq!(b.dropped(), 0);
        assert_eq!(b.subscriber_count(), 2);
    }

    /// (2) A 队列满后 drop 计数增长且发布不阻塞 (B 仍收到后续全部帧)。
    #[test]
    fn full_queue_drops_and_publisher_never_blocks() {
        let b: StreamBroadcaster<u64> = StreamBroadcaster::new();
        let (_a_id, _a_rx) = b.subscribe_with_capacity(4); // 小容量: 快速填满
        let (_b_id, b_rx) = b.subscribe();

        // 发布 100 帧: A 只保留 4 帧 (丢弃 96), B 保留全部。
        for i in 0..100 {
            b.publish(&i);
        }

        assert_eq!(b.published(), 100);
        assert_eq!(b.dropped(), 96);
        assert_eq!(b.consumed(), 104); // A 成功 4 + B 成功 100
        assert_eq!(b_rx.try_iter().count(), 100);
        // 若发布路径曾阻塞, 下面计数不可能同时成立 —— 发布已同步返回即证未阻塞。
        assert_eq!(b.published(), 100);
    }

    /// (3) 多消费者同时订阅, 各自独立收流。
    #[test]
    fn multiple_consumers_subscribe_simultaneously() {
        let b: StreamBroadcaster<u64> = StreamBroadcaster::new();
        const N: usize = 5;
        let mut rxs = Vec::new();
        for _ in 0..N {
            let (_id, rx) = b.subscribe();
            rxs.push(rx);
        }
        assert_eq!(b.subscriber_count(), N);

        for i in 0..3 {
            b.publish(&i);
        }
        for rx in &rxs {
            let got: Vec<u64> = rx.try_iter().collect();
            assert_eq!(got, vec![0, 1, 2]);
        }
    }

    /// (4) unsubscribe 后不再收到; 重复取消返回 false。
    #[test]
    fn unsubscribe_stops_delivery() {
        let b: StreamBroadcaster<u64> = StreamBroadcaster::new();
        let (id, rx) = b.subscribe();

        b.publish(&1);
        assert_eq!(rx.try_iter().count(), 1);

        assert!(b.unsubscribe(id));
        assert!(!b.unsubscribe(id)); // 已不存在
        assert_eq!(b.subscriber_count(), 0);

        b.publish(&2);
        assert_eq!(rx.try_iter().count(), 0); // 不再投递
    }

    /// (5) 默认消费者队列有界, 长度 ≤ 默认容量 1024。
    #[test]
    fn default_queue_is_bounded_at_1024() {
        let b: StreamBroadcaster<u64> = StreamBroadcaster::new();
        let (_id, rx) = b.subscribe();

        for i in 0..2000 {
            b.publish(&i);
        }

        // 未消费时队列上限 = 默认容量。
        assert_eq!(rx.len(), DEFAULT_QUEUE_CAPACITY);
        assert_eq!(rx.len(), 1024);
        assert!(rx.len() <= 1024);
        assert_eq!(b.dropped(), 2000 - DEFAULT_QUEUE_CAPACITY as u64);
        assert_eq!(b.published(), 2000);
    }

    /// 消费者接收端 drop 后, 发布时惰性回收 (无需显式 unsubscribe)。
    #[test]
    fn dropped_receiver_is_lazily_reclaimed() {
        let b: StreamBroadcaster<u64> = StreamBroadcaster::new();
        let (id, rx) = b.subscribe();
        assert_eq!(b.subscriber_count(), 1);

        drop(rx);
        b.publish(&1); // 触发惰性清理
        assert_eq!(b.subscriber_count(), 0);
        assert!(!b.unsubscribe(id)); // 已被回收
    }

    /// 跨线程: 一个线程持续发布, 另一线程消费 —— 验证无死锁/无阻塞。
    #[test]
    fn publish_from_worker_thread() {
        let b: Arc<StreamBroadcaster<u64>> = Arc::new(StreamBroadcaster::new());
        let (_id, rx) = b.subscribe();
        let b2 = Arc::clone(&b);

        let publisher = std::thread::spawn(move || {
            for i in 0..500 {
                b2.publish(&i);
            }
        });
        publisher.join().unwrap();

        wait_until(|| rx.len() >= 500);
        assert_eq!(rx.try_iter().count(), 500);
    }
}
