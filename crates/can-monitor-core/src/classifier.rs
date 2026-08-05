//! # 帧分类器
//!
//! 将后端读到的原始 CAN 帧按帧格式分发给对应协议栈解析, 产出统一的
//! [`ParsedMessage`](crate::classifier::ParsedMessage):
//!
//! - **11 位标准帧** → CANopen 栈 ([`CanopenService`](canopen_stack::CanopenService))
//! - **29 位扩展帧** → J1939 栈 ([`J1939Service`](j1939_stack::J1939Service))
//! - **无法归属** (远程帧 / 未分配 COB-ID / 孤儿传输层帧)
//!   → [`ParsedMessage::Raw`](crate::classifier::ParsedMessage::Raw)
//!
//! 分类器只依赖 `can-types` 的帧类型与两个协议栈, 不依赖任何具体后端,
//! 因此可在 SocketCAN / USBCAN 等不同后端之上复用同一套分类逻辑。

use std::time::{Duration, Instant};

use can_types::{CanFrame, CanMessage};
use canopen_stack::{CanopenMessage, CanopenService};
use j1939_stack::{J1939Message, J1939Service};

/// 默认 CANopen 心跳超时 (1 秒)。
const DEFAULT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(1);

/// 帧所属协议类别, 供过滤与统计使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// 无法归属任何已知协议栈的原始帧。
    Raw,
    /// CANopen (CiA 301) 协议帧 (11 位标准帧)。
    Canopen,
    /// J1939 协议帧 (29 位扩展帧)。
    J1939,
}

/// 一条帧的分类结果。
///
/// 同时携带原始帧与 (若识别成功) 对应协议栈的解析消息, 便于上层
/// 既能访问协议语义, 也能拿到原始载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedMessage {
    /// 原始帧: 未归属任何协议栈。
    Raw(CanFrame),
    /// CANopen 消息。
    ///
    /// @param frame 原始帧。
    /// @param msg   协议栈解析结果。
    Canopen {
        /// 原始帧。
        frame: CanFrame,
        /// CANopen 解析结果。
        msg: CanopenMessage,
    },
    /// J1939 消息。
    ///
    /// @param frame 原始帧。
    /// @param msg   协议栈解析结果。
    J1939 {
        /// 原始帧。
        frame: CanFrame,
        /// J1939 解析结果。
        msg: J1939Message,
    },
}

impl ParsedMessage {
    /// 获取本条消息的协议类别。
    ///
    /// @return 与消息变体对应的 [`Protocol`]。
    pub fn protocol(&self) -> Protocol {
        match self {
            ParsedMessage::Raw(_) => Protocol::Raw,
            ParsedMessage::Canopen { .. } => Protocol::Canopen,
            ParsedMessage::J1939 { .. } => Protocol::J1939,
        }
    }
}

/// 流中的一条完整报文元素。
///
/// 广播流 (由 [`bus::MonitorBus`](crate::bus::MonitorBus) 的 reader 线程发布)
/// 的元素类型: 原始统一消息与其**一次**分类结果打包在一起下发。所有消费者
/// (TUI / Web / Tauri) 直接消费本结构, 用
/// [`StreamItem::parsed`] 做协议过滤 / 高亮 / 显示, 用
/// [`StreamItem::msg`] 做方向判断 / 日志 / 序列化 —— **不需要 (也不允许)
/// 再次调用 [`FrameClassifier::classify`]**, 从而保证每帧在整个数据通路中
/// 恰好被分类一次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamItem {
    /// 原始统一消息 (方向恒为 [`Direction::Rx`](can_types::Direction::Rx))。
    pub msg: CanMessage,
    /// 本条消息的分类结果 (供过滤 / 高亮 / 协议显示)。
    pub parsed: ParsedMessage,
}

/// 帧分类器。
///
/// 内部持有 CANopen 与 J1939 两个协议服务。J1939 服务有状态 (维护传输层
/// 重组会话), 因此 [`FrameClassifier::classify`] 需要可变借用 `&mut self`,
/// 同一实例须串行喂帧。
#[derive(Debug)]
pub struct FrameClassifier {
    /// CANopen 协议服务 (含节点心跳健康监控)。
    canopen: CanopenService,
    /// J1939 协议服务 (含 TP 多包重组会话)。
    j1939: J1939Service,
}

impl Default for FrameClassifier {
    /// 使用 1 秒心跳超时构造分类器, 等价于
    /// [`FrameClassifier::new`](Self::new) 传入 1 秒。
    fn default() -> Self {
        Self::new(DEFAULT_HEARTBEAT_TIMEOUT)
    }
}

impl FrameClassifier {
    /// 构造分类器并指定 CANopen 心跳超时。
    ///
    /// @param heartbeat_timeout CANopen 心跳超时时长 (影响节点健康监控,
    ///                          见 [`CanopenService::new`])。
    /// @return 分类器实例。
    pub fn new(heartbeat_timeout: Duration) -> Self {
        Self {
            canopen: CanopenService::new(heartbeat_timeout),
            j1939: J1939Service::new(),
        }
    }

    /// 解析一帧并返回分类结果。
    ///
    /// 分发规则:
    /// - **11 位标准帧** → 交给 CANopen 栈; 栈返回 [`CanopenMessage::Unknown`]
    ///   (未分配 COB-ID) 时视为无法识别, 归为 [`ParsedMessage::Raw`];
    /// - **29 位扩展帧** → 交给 J1939 栈; 栈返回 `None` (孤儿 TP.DT 等) 时
    ///   归为 [`ParsedMessage::Raw`];
    /// - **其余情况** (远程标准帧等) → [`ParsedMessage::Raw`]。
    ///
    /// 顺带把 CANopen 心跳消息喂给内部健康监控, 使
    /// [`FrameClassifier::node_state`] 可查询节点状态。
    ///
    /// @param frame 待分类的帧。
    /// @return 分类结果 [`ParsedMessage`]。
    pub fn classify(&mut self, frame: &CanFrame) -> ParsedMessage {
        if frame.id().is_standard() {
            match CanopenService::parse(frame) {
                Some(CanopenMessage::Unknown) => {
                    // 未分配 COB-ID 不属于 CiA 301 预定义连接集, 视为原始帧。
                    ParsedMessage::Raw(frame.clone())
                }
                Some(msg) => {
                    // 跟踪心跳, 使节点健康状态可用 (非心跳消息被内部忽略)。
                    self.canopen.observe(&msg, Instant::now());
                    ParsedMessage::Canopen {
                        frame: frame.clone(),
                        msg,
                    }
                }
                None => {
                    // 远程帧等 CANopen 栈拒绝的输入。
                    ParsedMessage::Raw(frame.clone())
                }
            }
        } else {
            // 非标准帧均为 29 位扩展帧, 交给 J1939 栈。
            match self.j1939.parse(frame) {
                Some(msg) => ParsedMessage::J1939 {
                    frame: frame.clone(),
                    msg,
                },
                None => ParsedMessage::Raw(frame.clone()),
            }
        }
    }

    /// 获取一帧将被分类到的协议类别 (与 [`FrameClassifier::classify`] 结果一致)。
    ///
    /// 供过滤使用: 无需取回整条 [`ParsedMessage`], 只关心协议归属。注意
    /// J1939 解析有状态, 因此本方法也需要可变借用。
    ///
    /// @param frame 待判断的帧。
    /// @return 帧对应的 [`Protocol`]。
    pub fn protocol(&mut self, frame: &CanFrame) -> Protocol {
        self.classify(frame).protocol()
    }

    /// 查询节点最近报告的 CANopen 运行状态。
    ///
    /// @param node 节点号 (1..=127)。
    /// @return 从未收到心跳或节点号非法时为 `None`。
    pub fn node_state(&self, node: u8) -> Option<canopen_stack::NmtState> {
        self.canopen.node_state(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_types::{CanFrame, CanId};
    use canopen_stack::{NmtState, PdoDirection};

    /// 构造标准帧。
    fn frame(id: u16, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::new_standard(id).unwrap(), data.to_vec()).unwrap()
    }

    /// 构造扩展帧。
    fn frame_ext(id: u32, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::new_extended(id).unwrap(), data.to_vec()).unwrap()
    }

    /// 0x181 标准帧 → Canopen TPDO1 (节点 1)。
    #[test]
    fn standard_tpdo1_is_canopen() {
        let mut c = FrameClassifier::default();
        let parsed = c.classify(&frame(0x181, &[1, 2, 3]));
        assert_eq!(parsed.protocol(), Protocol::Canopen);
        match &parsed {
            ParsedMessage::Canopen { frame, msg } => {
                assert_eq!(frame.id().raw_id(), 0x181);
                assert_eq!(
                    msg,
                    &CanopenMessage::Pdo {
                        node: 1,
                        pdo_num: 1,
                        direction: PdoDirection::Tx,
                        data: vec![1, 2, 3],
                    }
                );
            }
            other => panic!("期望 Canopen, 实际 {other:?}"),
        }
    }

    /// 0x18FEF100 扩展帧 → J1939 直接消息 (PGN 0xFEF1, SA 0)。
    #[test]
    fn extended_ccvs1_is_j1939() {
        let mut c = FrameClassifier::default();
        let parsed = c.classify(&frame_ext(0x18FEF100, &[0x01, 0x02]));
        assert_eq!(parsed.protocol(), Protocol::J1939);
        match &parsed {
            ParsedMessage::J1939 { frame, msg } => {
                assert_eq!(frame.id().raw_id(), 0x18FEF100);
                match msg {
                    J1939Message::Direct { pgn, source, data } => {
                        assert_eq!(*pgn, 0x00FEF1);
                        assert_eq!(*source, 0x00);
                        assert_eq!(data, &[0x01, 0x02]);
                    }
                    other => panic!("期望 Direct, 实际 {other:?}"),
                }
            }
            other => panic!("期望 J1939, 实际 {other:?}"),
        }
    }

    /// 未知 11 位帧 (未分配 COB-ID 0x101) → Raw。
    #[test]
    fn unknown_standard_is_raw() {
        let mut c = FrameClassifier::default();
        let parsed = c.classify(&frame(0x101, &[1]));
        assert_eq!(parsed.protocol(), Protocol::Raw);
        match parsed {
            ParsedMessage::Raw(f) => assert_eq!(f.id().raw_id(), 0x101),
            other => panic!("期望 Raw, 实际 {other:?}"),
        }
    }

    /// 未知 29 位帧 (孤儿 TP.DT, 无会话可归属) → Raw。
    #[test]
    fn unknown_extended_is_raw() {
        let mut c = FrameClassifier::default();
        let parsed = c.classify(&frame_ext(0x1CEBFF80, &[1, 2, 3, 4, 5, 6, 7, 8]));
        assert_eq!(parsed.protocol(), Protocol::Raw);
        match parsed {
            ParsedMessage::Raw(f) => assert_eq!(f.id().raw_id(), 0x1CEBFF80),
            other => panic!("期望 Raw, 实际 {other:?}"),
        }
    }

    /// 远程标准帧 → Raw (CANopen 栈拒绝远程帧)。
    #[test]
    fn remote_standard_is_raw() {
        let mut f = frame(0x701, &[]);
        f.set_remote(true);
        let mut c = FrameClassifier::default();
        assert_eq!(c.classify(&f).protocol(), Protocol::Raw);
    }

    /// 心跳帧经 classify 后进入健康监控, 可查询节点状态。
    #[test]
    fn heartbeat_feeds_node_health() {
        let mut c = FrameClassifier::new(Duration::from_secs(1));
        let parsed = c.classify(&frame(0x705, &[0x05]));
        assert!(matches!(parsed, ParsedMessage::Canopen { .. }));
        assert_eq!(c.node_state(5), Some(NmtState::Operational));
    }
}
