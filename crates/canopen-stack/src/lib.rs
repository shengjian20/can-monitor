//! # canopen-stack — CANopen (CiA 301) 协议解析与下发服务
//!
//! 本 crate 在 [`can-types`](https://docs.rs/can-types) 的 [`CanFrame`] 之上提供
//! CANopen 协议层, 供监控 / 下发界面消费:
//!
//! - **帧解析** [`CanopenService::parse`] : 按 COB-ID (CiA 301 预定义连接集)
//!   将任意 11 位标准帧分类为 NMT / SYNC / EMCY / TIME / PDO / SDO / 心跳,
//!   并提取节点号、索引、子索引等字段;
//! - **帧构造** [`CanopenService::nmt_frame`] / [`CanopenService::sdo_read_frame`] /
//!   [`CanopenService::sdo_write_frame`] : 生成 NMT 命令帧与 SDO 读写请求帧;
//! - **节点健康监控** [`CanopenService::observe`] : 封装
//!   [`canopen-host`](https://crates.io/crates/canopen-host) 的 `nmt::HeartbeatMonitor`,
//!   跟踪各节点心跳超时。
//!
//! ## 依赖隔离
//!
//! 本 crate 依赖 `canopen-host` (固定 `=0.6.1`, pre-1.0 API 不稳定), 但 **不把其类型
//! 泄漏到公共 API**: 公共接口仅使用本 crate 自有的 [`NmtCommand`] / [`NmtState`] /
//! [`CanopenMessage`] 与 `can-types` 的 [`CanFrame`]。`canopen-host` 仅被私有封装
//! (用于心跳监控), 便于未来升级该依赖。
//!
//! CANopen 运行在 11 位标准帧上; 本 crate **不声称支持 CANFD** 上的 CANopen,
//! 解析层对扩展帧 / 远程帧返回 `None`。

use std::time::{Duration, Instant};

use can_types::{CanError, CanFrame, CanId, Result};

/// NMT 节点控制通道 COB-ID (`0x000`, 主站 → 从站广播)。
pub const NMT_COB_ID: u16 = 0x000;

/// SYNC 同步对象 COB-ID (`0x080`, 广播)。
pub const SYNC_COB_ID: u16 = 0x080;

/// EMCY 紧急对象 COB-ID 基址 (`0x080 + node`, 节点 1..=127)。
pub const EMCY_COB_BASE: u16 = 0x080;

/// TIME 时间戳对象 COB-ID (`0x100`, 广播)。
pub const TIME_COB_ID: u16 = 0x100;

/// TPDO COB-ID 基址 (`0x180 + (n-1)*0x100 + node`, n = 1..=4)。
pub const TPDO_COB_BASE: u16 = 0x180;

/// RPDO COB-ID 基址 (`0x200 + (n-1)*0x100 + node`, n = 1..=4)。
pub const RPDO_COB_BASE: u16 = 0x200;

/// SDO 响应 (服务器 → 客户端) COB-ID 基址 (`0x580 + node`)。
pub const SDO_RESPONSE_COB_BASE: u16 = 0x580;

/// SDO 请求 (客户端 → 服务器) COB-ID 基址 (`0x600 + node`)。
pub const SDO_REQUEST_COB_BASE: u16 = 0x600;

/// 心跳 / 启动 (boot-up) COB-ID 基址 (`0x700 + node`)。
pub const HEARTBEAT_COB_BASE: u16 = 0x700;

/// 从 COB-ID 提取节点号时的低 7 位掩码。
pub const NODE_MASK: u16 = 0x7F;

/// 有效设备节点号上界 (CiA 301: 节点 1..=127, 0 保留给 NMT 广播)。
pub const MAX_NODE: u8 = 127;

/// NMT 节点控制命令 (CiA 301 §7.2.8.2)。
///
/// 判别值即线上命令字节; [`NmtCommand::Unknown`] 用于保留无法识别的命令字节。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NmtCommand {
    /// 启动远程节点 → 进入 Operational。
    StartRemoteNode = 0x01,
    /// 停止远程节点 → 进入 Stopped。
    StopRemoteNode = 0x02,
    /// 进入 Pre-operational。
    EnterPreOperational = 0x80,
    /// 复位节点 → 重新初始化 (应用 + 通信)。
    ResetNode = 0x81,
    /// 复位通信 → 重新初始化 (仅通信)。
    ResetCommunication = 0x82,
    /// 无法识别的命令字节 (保留原始值)。
    Unknown(u8),
}

impl NmtCommand {
    /// 从命令字节解码 NMT 命令。
    ///
    /// @param byte 线上命令字节。
    /// @return 已识别的命令; 未识别值返回 [`NmtCommand::Unknown`]。
    pub fn from_byte(byte: u8) -> Self {
        match byte {
            0x01 => NmtCommand::StartRemoteNode,
            0x02 => NmtCommand::StopRemoteNode,
            0x80 => NmtCommand::EnterPreOperational,
            0x81 => NmtCommand::ResetNode,
            0x82 => NmtCommand::ResetCommunication,
            other => NmtCommand::Unknown(other),
        }
    }

    /// 编码为线上命令字节。
    ///
    /// @return 命令对应的线上字节。
    pub fn to_byte(self) -> u8 {
        match self {
            NmtCommand::StartRemoteNode => 0x01,
            NmtCommand::StopRemoteNode => 0x02,
            NmtCommand::EnterPreOperational => 0x80,
            NmtCommand::ResetNode => 0x81,
            NmtCommand::ResetCommunication => 0x82,
            NmtCommand::Unknown(b) => b,
        }
    }
}

/// 节点的 NMT 运行状态 (CiA 301 §7.2.8.3)。
///
/// 判别值即心跳帧中的状态字节 (位 7 为节点守护 toggle, 解码时忽略);
/// [`NmtState::Unknown`] 用于保留无法识别的状态值。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NmtState {
    /// 初始化中 (boot-up 消息, 值 0)。
    Initialising = 0x00,
    /// Stopped: 仅 NMT 与错误控制活跃。
    Stopped = 0x04,
    /// Operational: 全部通信对象 (含 PDO) 活跃。
    Operational = 0x05,
    /// Pre-operational: SDO 活跃, PDO 不活跃。
    PreOperational = 0x7F,
    /// 无法识别的状态值 (保留原始字节, 已掩码位 7)。
    Unknown(u8),
}

impl NmtState {
    /// 从心跳状态字节解码 NMT 状态 (忽略位 7 toggle)。
    ///
    /// @param byte 线上状态字节。
    /// @return 已识别的状态; 未识别值返回 [`NmtState::Unknown`] (已掩码位 7)。
    pub fn from_byte(byte: u8) -> Self {
        match byte & 0x7F {
            0x00 => NmtState::Initialising,
            0x04 => NmtState::Stopped,
            0x05 => NmtState::Operational,
            0x7F => NmtState::PreOperational,
            other => NmtState::Unknown(other),
        }
    }

    /// 编码为线上状态字节。
    ///
    /// @return 状态对应的线上字节。
    pub fn to_byte(self) -> u8 {
        match self {
            NmtState::Initialising => 0x00,
            NmtState::Stopped => 0x04,
            NmtState::Operational => 0x05,
            NmtState::PreOperational => 0x7F,
            NmtState::Unknown(b) => b,
        }
    }

    /// 由 `canopen-host` 的状态类型转换 (隔离层内部使用)。
    fn from_canopen(state: canopen_host::canopen_rs::NmtState) -> Self {
        match state {
            canopen_host::canopen_rs::NmtState::Initialising => NmtState::Initialising,
            canopen_host::canopen_rs::NmtState::Stopped => NmtState::Stopped,
            canopen_host::canopen_rs::NmtState::Operational => NmtState::Operational,
            canopen_host::canopen_rs::NmtState::PreOperational => NmtState::PreOperational,
        }
    }
}

/// PDO 传输方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdoDirection {
    /// 发送方向: TPDO, 节点向总线发布数据。
    Tx,
    /// 接收方向: RPDO, 节点从总线消费数据。
    Rx,
}

/// 解析一帧后得到的 CANopen 消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanopenMessage {
    /// NMT 节点控制命令 (COB-ID `0x000`)。
    Nmt {
        /// 命令。
        cmd: NmtCommand,
        /// 目标节点号 (0 = 广播)。
        node: u8,
    },
    /// 心跳 / 启动 (boot-up) 帧 (COB-ID `0x700 + node`)。
    Heartbeat {
        /// 节点号。
        node: u8,
        /// 报告的状态。
        state: NmtState,
    },
    /// SDO 请求 (客户端 → 服务器, COB-ID `0x600 + node`)。
    SdoRequest {
        /// 节点号。
        node: u8,
        /// 对象字典索引。
        index: u16,
        /// 子索引。
        subindex: u8,
        /// 完整 8 字节 SDO 载荷 (含命令字节)。
        data: Vec<u8>,
    },
    /// SDO 响应 (服务器 → 客户端, COB-ID `0x580 + node`)。
    SdoResponse {
        /// 节点号。
        node: u8,
        /// 对象字典索引。
        index: u16,
        /// 子索引。
        subindex: u8,
        /// 完整 8 字节 SDO 载荷 (含命令字节)。
        data: Vec<u8>,
    },
    /// PDO 过程数据 (TPDO `0x180/0x280/0x380/0x480 + node`,
    /// RPDO `0x200/0x300/0x400/0x500 + node`)。
    Pdo {
        /// 节点号。
        node: u8,
        /// PDO 编号 (1..=4)。
        pdo_num: u8,
        /// 传输方向。
        direction: PdoDirection,
        /// PDO 数据 (≤ 8 字节)。
        data: Vec<u8>,
    },
    /// 紧急对象 (EMCY, COB-ID `0x080 + node`)。
    Emcy {
        /// 节点号。
        node: u8,
        /// 16 位紧急错误码 (载荷字节 0..2, 小端)。
        code: u16,
    },
    /// SYNC 同步帧 (COB-ID `0x080`)。
    Sync,
    /// TIME 时间戳帧 (COB-ID `0x100`)。
    Time,
    /// 未分配 / 无法识别的 COB-ID。
    Unknown,
}

/// COB-ID 分类结果 (解析中间态)。
enum MsgClass {
    /// NMT 节点控制。
    Nmt,
    /// SYNC 同步。
    Sync,
    /// EMCY 紧急。
    Emcy,
    /// TIME 时间戳。
    Time,
    /// TPDO n (n = 1..=4)。
    Tpdo(u8),
    /// RPDO n (n = 1..=4)。
    Rpdo(u8),
    /// SDO 请求。
    SdoRequest,
    /// SDO 响应。
    SdoResponse,
    /// 心跳。
    Heartbeat,
    /// 未分配。
    Unknown,
}

/// 按 CiA 301 预定义连接集将 11 位 COB-ID 分类。
///
/// @param raw 原始 11 位 COB-ID。
/// @return 对应的消息类别。
fn classify(raw: u16) -> MsgClass {
    match raw {
        0x000 => MsgClass::Nmt,
        0x080 => MsgClass::Sync,
        0x081..=0x0FF => MsgClass::Emcy,
        0x100 => MsgClass::Time,
        0x180..=0x1FF => MsgClass::Tpdo(1),
        0x200..=0x27F => MsgClass::Rpdo(1),
        0x280..=0x2FF => MsgClass::Tpdo(2),
        0x300..=0x37F => MsgClass::Rpdo(2),
        0x380..=0x3FF => MsgClass::Tpdo(3),
        0x400..=0x47F => MsgClass::Rpdo(3),
        0x480..=0x4FF => MsgClass::Tpdo(4),
        0x500..=0x57F => MsgClass::Rpdo(4),
        0x580..=0x5FF => MsgClass::SdoResponse,
        0x600..=0x67F => MsgClass::SdoRequest,
        0x700..=0x77F => MsgClass::Heartbeat,
        _ => MsgClass::Unknown,
    }
}

/// 从 SDO 载荷提取对象索引 (字节 1..2, 小端)。
///
/// @param data SDO 载荷。
/// @return 载荷不足 3 字节时返回 0。
fn sdo_index(data: &[u8]) -> u16 {
    if data.len() >= 3 {
        u16::from_le_bytes([data[1], data[2]])
    } else {
        0
    }
}

/// 心跳监控器 (私有封装 `canopen-host` 的 `nmt::HeartbeatMonitor`)。
///
/// 只在本 crate 内部使用, 公共接口经 [`CanopenService`] 转换为我方类型,
/// 不把 `canopen-host` 类型泄漏到公共 API。
#[derive(Debug)]
struct HeartbeatWatcher {
    /// 底层监控器。
    inner: canopen_host::nmt::HeartbeatMonitor,
}

impl HeartbeatWatcher {
    /// 新建监控器, 指定心跳超时。
    ///
    /// @param timeout 超过该时长未收到心跳即视为节点离线。
    /// @return 监控器实例。
    fn new(timeout: Duration) -> Self {
        Self {
            inner: canopen_host::nmt::HeartbeatMonitor::new(timeout),
        }
    }

    /// 记录一条心跳。
    ///
    /// @param node  节点号。
    /// @param state 心跳报告的状态。
    /// @param now   心跳接收时刻。
    fn record(&mut self, node: u8, state: NmtState, now: Instant) {
        // 交由 canopen-host 校验节点范围并解码状态; 未识别状态被其忽略。
        let cob_id = HEARTBEAT_COB_BASE + u16::from(node);
        self.inner.on_frame(cob_id, &[state.to_byte()], now);
    }

    /// 查询节点最近报告的状态。
    ///
    /// @param node 节点号。
    /// @return 从未收到心跳或节点号非法时为 `None`。
    fn state(&self, node: u8) -> Option<NmtState> {
        let node_id = canopen_host::canopen_rs::NodeId::new(node).ok()?;
        self.inner.state(node_id).map(NmtState::from_canopen)
    }

    /// 判断节点在 `now` 时刻是否在线。
    ///
    /// @param node 节点号。
    /// @param now  判定时刻。
    /// @return 超时窗口内收到过心跳返回 `true`。
    fn is_alive(&self, node: u8, now: Instant) -> bool {
        let Ok(node_id) = canopen_host::canopen_rs::NodeId::new(node) else {
            return false;
        };
        self.inner.is_alive(node_id, now)
    }

    /// 返回在 `now` 时刻已超时 (离线) 的节点列表。
    ///
    /// @param now 判定时刻。
    /// @return 节点号列表 (可能为空)。
    fn timed_out(&self, now: Instant) -> Vec<u8> {
        self.inner.timed_out(now).map(|n| n.raw()).collect()
    }
}

/// CANopen 协议服务。
///
/// 提供三组能力:
/// - **帧解析** [`CanopenService::parse`] 与 **帧构造**
///   [`CanopenService::nmt_frame`] / [`CanopenService::sdo_read_frame`] /
///   [`CanopenService::sdo_write_frame`] / [`CanopenService::sync_frame`]
///   (无状态, 直接调用关联函数);
/// - **节点健康监控** [`CanopenService::observe`] /
///   [`CanopenService::is_alive`] / [`CanopenService::node_state`] /
///   [`CanopenService::silent_nodes`] (有状态, 封装 `canopen-host` 心跳监控)。
#[derive(Debug)]
pub struct CanopenService {
    /// 心跳监控器。
    heartbeat: HeartbeatWatcher,
}

impl CanopenService {
    /// 创建服务并指定心跳超时。
    ///
    /// @param heartbeat_timeout 心跳超时时长。
    /// @return 服务实例。
    pub fn new(heartbeat_timeout: Duration) -> Self {
        Self {
            heartbeat: HeartbeatWatcher::new(heartbeat_timeout),
        }
    }

    /// 解析一帧 CAN 数据为 CANopen 消息。
    ///
    /// 仅处理 11 位标准数据帧 (CANopen 使用标准帧); 扩展帧与远程帧返回 `None`。
    /// 分类依据 CiA 301 预定义连接集:
    ///
    /// | COB-ID 区间        | 类型                          |
    /// |--------------------|-------------------------------|
    /// | `0x000`            | NMT 节点控制                  |
    /// | `0x080`            | SYNC                          |
    /// | `0x081..=0x0FF`    | EMCY (`0x080 + node`)         |
    /// | `0x100`            | TIME                          |
    /// | `0x180..=0x1FF`    | TPDO1 (`0x180 + node`)        |
    /// | `0x200..=0x27F`    | RPDO1 (`0x200 + node`)        |
    /// | `0x280..=0x2FF`    | TPDO2                          |
    /// | `0x300..=0x37F`    | RPDO2                          |
    /// | `0x380..=0x3FF`    | TPDO3                          |
    /// | `0x400..=0x47F`    | RPDO3                          |
    /// | `0x480..=0x4FF`    | TPDO4                          |
    /// | `0x500..=0x57F`    | RPDO4                          |
    /// | `0x580..=0x5FF`    | SDO 响应 (`0x580 + node`)     |
    /// | `0x600..=0x67F`    | SDO 请求 (`0x600 + node`)     |
    /// | `0x700..=0x77F`    | 心跳 (`0x700 + node`)         |
    /// | 其余标准帧          | [`CanopenMessage::Unknown`]   |
    ///
    /// @param frame 待解析的 CAN 帧。
    /// @return 解析结果; 非标准帧 / 远程帧返回 `None`。
    pub fn parse(frame: &CanFrame) -> Option<CanopenMessage> {
        let id = frame.id();
        if !id.is_standard() || frame.is_remote() {
            return None;
        }
        let raw = id.raw_id() as u16;
        let node = (raw & NODE_MASK) as u8;
        let data = frame.data();
        match classify(raw) {
            MsgClass::Nmt => Some(CanopenMessage::Nmt {
                cmd: NmtCommand::from_byte(data.first().copied().unwrap_or(0)),
                node: data.get(1).copied().unwrap_or(0),
            }),
            MsgClass::Sync => Some(CanopenMessage::Sync),
            MsgClass::Emcy => Some(CanopenMessage::Emcy {
                node,
                code: match data {
                    [a, b, ..] => u16::from_le_bytes([*a, *b]),
                    _ => 0,
                },
            }),
            MsgClass::Time => Some(CanopenMessage::Time),
            MsgClass::Tpdo(num) => Some(CanopenMessage::Pdo {
                node,
                pdo_num: num,
                direction: PdoDirection::Tx,
                data: data.to_vec(),
            }),
            MsgClass::Rpdo(num) => Some(CanopenMessage::Pdo {
                node,
                pdo_num: num,
                direction: PdoDirection::Rx,
                data: data.to_vec(),
            }),
            MsgClass::SdoRequest => Some(CanopenMessage::SdoRequest {
                node,
                index: sdo_index(data),
                subindex: data.get(3).copied().unwrap_or(0),
                data: data.to_vec(),
            }),
            MsgClass::SdoResponse => Some(CanopenMessage::SdoResponse {
                node,
                index: sdo_index(data),
                subindex: data.get(3).copied().unwrap_or(0),
                data: data.to_vec(),
            }),
            MsgClass::Heartbeat => Some(CanopenMessage::Heartbeat {
                node,
                state: NmtState::from_byte(data.first().copied().unwrap_or(0)),
            }),
            MsgClass::Unknown => Some(CanopenMessage::Unknown),
        }
    }

    /// 构造 NMT 节点控制命令帧 (COB-ID `0x000`, 数据 `[命令字节, 目标节点]`)。
    ///
    /// @param command NMT 命令。
    /// @param node    目标节点号; `0` 表示广播, 合法范围 0..=127。
    /// @return 命令帧; 节点号非法 (≥ 128) 返回 [`CanError::InvalidId`]。
    pub fn nmt_frame(command: NmtCommand, node: u8) -> Result<CanFrame> {
        if !(0..=MAX_NODE).contains(&node) {
            return Err(CanError::InvalidId);
        }
        let data = vec![command.to_byte(), node];
        CanFrame::new(CanId::new_standard(NMT_COB_ID)?, data)
    }

    /// 构造 SDO 上传 (读) 请求帧 (COB-ID `0x600 + node`)。
    ///
    /// 数据区为 8 字节标准 SDO 请求: 命令字节 `0x40` + 索引 (小端) + 子索引 + 补零。
    ///
    /// @param node     目标节点号 (1..=127)。
    /// @param index    对象字典索引。
    /// @param subindex 子索引。
    /// @return 请求帧; 节点号非法返回 [`CanError::InvalidId`]。
    pub fn sdo_read_frame(node: u8, index: u16, subindex: u8) -> Result<CanFrame> {
        let mut payload = vec![0x40];
        payload.extend_from_slice(&index.to_le_bytes());
        payload.push(subindex);
        payload.resize(8, 0);
        sdo_frame(SDO_REQUEST_COB_BASE, node, payload)
    }

    /// 构造 SDO 下载 (写) 请求帧 (COB-ID `0x600 + node`)。
    ///
    /// - 数据 1..=4 字节: 快速传输 (expedited) 帧, 命令字节 `0x20 | ((4-n)<<2) | 0x03`;
    /// - 数据 0 或 5..=8 字节: 分段传输发起帧, 命令字节 `0x21` + 总长度 (小端)。
    ///
    /// @param node     目标节点号 (1..=127)。
    /// @param index    对象字典索引。
    /// @param subindex 子索引。
    /// @param data     待写入数据 (≤ 8 字节)。
    /// @return 请求帧; 节点号非法返回 [`CanError::InvalidId`],
    ///         数据超 8 字节返回 [`CanError::FrameTooLong`]。
    pub fn sdo_write_frame(node: u8, index: u16, subindex: u8, data: &[u8]) -> Result<CanFrame> {
        let payload = match data.len() {
            1..=4 => {
                let mut p = vec![0x20 | (((4 - data.len()) as u8) << 2) | 0x03];
                p.extend_from_slice(&index.to_le_bytes());
                p.push(subindex);
                p.extend_from_slice(data);
                p.resize(8, 0);
                p
            }
            0 | 5..=8 => {
                let mut p = vec![0x21];
                p.extend_from_slice(&index.to_le_bytes());
                p.push(subindex);
                p.extend_from_slice(&(data.len() as u32).to_le_bytes());
                p
            }
            _ => return Err(CanError::FrameTooLong),
        };
        sdo_frame(SDO_REQUEST_COB_BASE, node, payload)
    }

    /// 构造 SYNC 同步帧 (COB-ID `0x080`, 空数据)。
    ///
    /// 恒不失败 (ID 与数据长度均在合法范围内)。
    ///
    /// @return SYNC 帧。
    pub fn sync_frame() -> CanFrame {
        CanFrame::new(CanId::new_standard(SYNC_COB_ID).expect("0x080 是合法标准 ID"), Vec::new())
            .expect("空数据帧长度合法")
    }

    /// 记录一条消息, 更新节点健康状态 (仅心跳消息生效)。
    ///
    /// 监控循环将 [`CanopenService::parse`] 的结果喂入本方法, 即可跟踪各节点心跳。
    ///
    /// @param msg 已解析的消息。
    /// @param now 消息接收时刻。
    pub fn observe(&mut self, msg: &CanopenMessage, now: Instant) {
        if let CanopenMessage::Heartbeat { node, state } = msg {
            self.heartbeat.record(*node, *state, now);
        }
    }

    /// 查询节点最近报告的状态。
    ///
    /// @param node 节点号。
    /// @return 从未收到心跳或节点号非法时为 `None`。
    pub fn node_state(&self, node: u8) -> Option<NmtState> {
        self.heartbeat.state(node)
    }

    /// 判断节点在 `now` 时刻是否在线。
    ///
    /// @param node 节点号。
    /// @param now  判定时刻。
    /// @return 超时窗口内收到过心跳返回 `true`。
    pub fn is_alive(&self, node: u8, now: Instant) -> bool {
        self.heartbeat.is_alive(node, now)
    }

    /// 返回在 `now` 时刻已心跳超时 (疑似离线) 的节点列表。
    ///
    /// @param now 判定时刻。
    /// @return 节点号列表 (可能为空)。
    pub fn silent_nodes(&self, now: Instant) -> Vec<u8> {
        self.heartbeat.timed_out(now)
    }
}

/// 构造 SDO 通道上的帧 (公共基址校验 + 节点校验)。
///
/// @param cob_base COB-ID 基址 (请求 `0x600` / 响应 `0x580`)。
/// @param node     节点号 (1..=127)。
/// @param payload  8 字节 SDO 载荷。
/// @return 帧; 节点号非法返回 [`CanError::InvalidId`]。
fn sdo_frame(cob_base: u16, node: u8, payload: Vec<u8>) -> Result<CanFrame> {
    if !(1..=MAX_NODE).contains(&node) {
        return Err(CanError::InvalidId);
    }
    let id = CanId::new_standard(cob_base + u16::from(node))?;
    CanFrame::new(id, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷构造测试帧。
    fn frame(id: u16, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::new_standard(id).unwrap(), data.to_vec()).unwrap()
    }

    // ---- 解析: 已知帧 ----

    /// 0x000 + [1,1] → NMT 启动节点 1。
    #[test]
    fn parse_nmt_start_node1() {
        let msg = CanopenService::parse(&frame(0x000, &[1, 1])).unwrap();
        assert_eq!(
            msg,
            CanopenMessage::Nmt {
                cmd: NmtCommand::StartRemoteNode,
                node: 1,
            }
        );
    }

    /// 0x701 + [5] → 节点 1 心跳 Operational。
    #[test]
    fn parse_heartbeat_node1_operational() {
        let msg = CanopenService::parse(&frame(0x701, &[5])).unwrap();
        assert_eq!(
            msg,
            CanopenMessage::Heartbeat {
                node: 1,
                state: NmtState::Operational,
            }
        );
    }

    /// 0x601 + [0x22,0x10,0x20,1,2,3,4,0] → SDO 请求, 索引 0x2010 子 1。
    #[test]
    fn parse_sdo_request_index_2010() {
        let data = [0x22, 0x10, 0x20, 1, 2, 3, 4, 0];
        let msg = CanopenService::parse(&frame(0x601, &data)).unwrap();
        assert_eq!(
            msg,
            CanopenMessage::SdoRequest {
                node: 1,
                index: 0x2010,
                subindex: 1,
                data: data.to_vec(),
            }
        );
    }

    /// 0x181 + [8 字节] → TPDO1 节点 1。
    #[test]
    fn parse_tpdo1_node1() {
        let data = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let msg = CanopenService::parse(&frame(0x181, &data)).unwrap();
        assert_eq!(
            msg,
            CanopenMessage::Pdo {
                node: 1,
                pdo_num: 1,
                direction: PdoDirection::Tx,
                data: data.to_vec(),
            }
        );
    }

    /// 0x081 + [0x10,0,...] → 节点 1 EMCY, 错误码 0x0010。
    #[test]
    fn parse_emcy_node1() {
        let msg = CanopenService::parse(&frame(0x081, &[0x10, 0x00, 0, 0, 0, 0, 0, 0])).unwrap();
        assert_eq!(msg, CanopenMessage::Emcy { node: 1, code: 0x0010 });
    }

    /// 0x080 / 0x100 → SYNC / TIME。
    #[test]
    fn parse_sync_and_time() {
        assert_eq!(
            CanopenService::parse(&frame(0x080, &[])),
            Some(CanopenMessage::Sync)
        );
        assert_eq!(
            CanopenService::parse(&frame(0x100, &[1, 2, 3, 4, 5, 6])),
            Some(CanopenMessage::Time)
        );
    }

    /// RPDO1 (0x205) 与 TPDO2 (0x283) 分类正确。
    #[test]
    fn parse_rpdo1_and_tpdo2() {
        let msg = CanopenService::parse(&frame(0x205, &[1, 2, 3])).unwrap();
        assert_eq!(
            msg,
            CanopenMessage::Pdo {
                node: 5,
                pdo_num: 1,
                direction: PdoDirection::Rx,
                data: vec![1, 2, 3],
            }
        );
        let msg = CanopenService::parse(&frame(0x283, &[])).unwrap();
        assert_eq!(
            msg,
            CanopenMessage::Pdo {
                node: 3,
                pdo_num: 2,
                direction: PdoDirection::Tx,
                data: vec![],
            }
        );
    }

    /// 0x581 → SDO 响应, 索引 0x1000。
    #[test]
    fn parse_sdo_response_index_1000() {
        let data = [0x43, 0x00, 0x10, 0x00, 0x92, 0x01, 0x00, 0x00];
        let msg = CanopenService::parse(&frame(0x581, &data)).unwrap();
        assert_eq!(
            msg,
            CanopenMessage::SdoResponse {
                node: 1,
                index: 0x1000,
                subindex: 0,
                data: data.to_vec(),
            }
        );
    }

    /// 未分配 COB-ID (0x101, 0x7FF) → Unknown。
    #[test]
    fn parse_unassigned_cob_id_unknown() {
        assert_eq!(
            CanopenService::parse(&frame(0x101, &[1])),
            Some(CanopenMessage::Unknown)
        );
        assert_eq!(
            CanopenService::parse(&frame(0x7FF, &[1])),
            Some(CanopenMessage::Unknown)
        );
    }

    /// 扩展帧与远程帧 → None。
    #[test]
    fn parse_extended_or_remote_is_none() {
        let ext = CanFrame::new(CanId::new_extended(0x181).unwrap(), vec![1]).unwrap();
        assert_eq!(CanopenService::parse(&ext), None);

        let mut remote = frame(0x701, &[]);
        remote.set_remote(true);
        assert_eq!(CanopenService::parse(&remote), None);
    }

    // ---- 帧构造 ----

    /// nmt_frame(START, 1) → ID 0x000, 数据 [1,1]。
    #[test]
    fn build_nmt_frame() {
        let f = CanopenService::nmt_frame(NmtCommand::StartRemoteNode, 1).unwrap();
        assert_eq!(f.id().raw_id(), 0x000);
        assert_eq!(f.data(), &[1, 1]);
    }

    /// 广播节点 (0) 构造为 [命令, 0]。
    #[test]
    fn build_nmt_broadcast() {
        let f = CanopenService::nmt_frame(NmtCommand::StartRemoteNode, 0).unwrap();
        assert_eq!(f.data(), &[1, 0]);
    }

    /// sdo_read_frame(5, 0x1017, 0) → ID 0x605, 命令字节 0x40。
    #[test]
    fn build_sdo_read_frame() {
        let f = CanopenService::sdo_read_frame(5, 0x1017, 0).unwrap();
        assert_eq!(f.id().raw_id(), 0x605);
        assert_eq!(f.data(), &[0x40, 0x17, 0x10, 0x00, 0, 0, 0, 0]);
    }

    /// sdo_write_frame(1, 0x2010, 0, [1,2]) → ID 0x601, 数据含命令字节 0x2B。
    #[test]
    fn build_sdo_write_frame() {
        let f = CanopenService::sdo_write_frame(1, 0x2010, 0, &[1, 2]).unwrap();
        assert_eq!(f.id().raw_id(), 0x601);
        assert_eq!(f.data(), &[0x2B, 0x10, 0x20, 0x00, 0x01, 0x02, 0x00, 0x00]);
    }

    /// 4 字节快速下载与 canopen-rs 已知帧 (0x23) 对齐。
    #[test]
    fn build_sdo_write_u32_matches_known_frame() {
        let f = CanopenService::sdo_write_frame(1, 0x2000, 0, &[0x78, 0x56, 0x34, 0x12]).unwrap();
        assert_eq!(f.data(), &[0x23, 0x00, 0x20, 0x00, 0x78, 0x56, 0x34, 0x12]);
    }

    /// 6 字节写 → 分段传输发起帧 (0x21 + 长度)。
    #[test]
    fn build_sdo_write_segmented_initiate() {
        let f = CanopenService::sdo_write_frame(1, 0x2000, 0, &[9, 8, 7, 6, 5, 4]).unwrap();
        assert_eq!(f.data(), &[0x21, 0x00, 0x20, 0x00, 6, 0, 0, 0]);
    }

    /// 节点号非法 → InvalidId。
    #[test]
    fn build_with_invalid_node_rejected() {
        assert_eq!(
            CanopenService::sdo_read_frame(0, 0x1000, 0),
            Err(CanError::InvalidId)
        );
        assert_eq!(
            CanopenService::sdo_write_frame(200, 0x1000, 0, &[1]),
            Err(CanError::InvalidId)
        );
        assert_eq!(
            CanopenService::nmt_frame(NmtCommand::StopRemoteNode, 128),
            Err(CanError::InvalidId)
        );
    }

    /// 写数据超 8 字节 → FrameTooLong。
    #[test]
    fn build_sdo_write_overlong_rejected() {
        assert_eq!(
            CanopenService::sdo_write_frame(1, 0x1000, 0, &[0; 9]),
            Err(CanError::FrameTooLong)
        );
    }

    // ---- 节点健康监控 (canopen-host 集成) ----

    /// 心跳经 observe 记录后, 可查询存活 / 状态 / 超时节点。
    #[test]
    fn heartbeat_monitor_tracks_node() {
        let mut svc = CanopenService::new(Duration::from_secs(1));
        let now = Instant::now();

        let msg = CanopenService::parse(&frame(0x705, &[0x05])).unwrap();
        assert_eq!(
            msg,
            CanopenMessage::Heartbeat {
                node: 5,
                state: NmtState::Operational,
            }
        );
        svc.observe(&msg, now);

        assert!(svc.is_alive(5, now));
        assert_eq!(svc.node_state(5), Some(NmtState::Operational));
        assert!(!svc.is_alive(5, now + Duration::from_secs(2)));
        assert_eq!(svc.silent_nodes(now + Duration::from_secs(2)), vec![5]);
        assert_eq!(svc.node_state(9), None);
    }
}
