//! # can-types — 协议无关的 CAN 帧类型与后端抽象
//!
//! 本 crate 是整个 CAN 监控架构的**核心契约层**,不依赖任何具体后端
//! (SocketCAN / USBCAN 等),也不引入异步,所有后端 crate 都以本 crate 为唯一依赖。
//!
//! 提供以下抽象:
//! - [`CanId`] : 11 位标准帧 / 29 位扩展帧标识符
//! - [`CanFrame`] : 协议无关的 CAN 帧 (含 CANFD 标志与时间戳)
//! - [`CanError`] : 统一错误类型
//! - [`BackendConfig`] / [`BackendKind`] : 后端配置与后端种类
//! - [`CanMessage`] / [`FrameSource`] : 统一消息与帧来源抽象
//! - [`CanBackend`] : 所有具体后端必须实现的底层 trait
//! - [`CanDeviceInfo`] / [`DeviceDiscoverer`] : 设备发现抽象 (设备列表 / 动态加载)
//!
//! 仅依赖标准库 (`std`)。

use std::fmt;
use std::io;
use std::time::{Duration, SystemTime};

/// 最大标准帧 (11 位) CAN ID 值 (0x7FF)。
pub const MAX_STANDARD_ID: u32 = 0x7FF;

/// 最大扩展帧 (29 位) CAN ID 值 (0x1FFF_FFFF)。
pub const MAX_EXTENDED_ID: u32 = 0x1FFF_FFFF;

/// 标准 CAN 帧数据区最大长度 (字节)。
pub const MAX_STANDARD_DATA_LEN: usize = 8;

/// CANFD 帧数据区最大长度 (字节)。
pub const MAX_FD_DATA_LEN: usize = 64;

/// crate 级统一结果类型。
///
/// 所有后端操作 (打开 / 读写 / 关闭) 统一返回该类型,错误见 [`CanError`]。
pub type Result<T> = std::result::Result<T, CanError>;

/// 统一错误类型。
///
/// 覆盖后端生命周期中可能出现的全部错误类别:接口不存在、总线错误、底层 IO、
/// 协议 / 功能不支持、非法参数、超时与设备热插拔。
#[derive(Debug)]
pub enum CanError {
    /// 设备或接口不存在 (如 SocketCAN 网络接口名无效、USBCAN 设备未连接)。
    NotFound,
    /// CAN 总线错误 (总线关闭、位错误、仲裁丢失等)。
    BusError,
    /// 底层 IO 错误 (系统调用失败、驱动读写失败等)。
    Io(io::Error),
    /// 协议层错误,携带静态字符串描述。
    Protocol(&'static str),
    /// 当前后端不支持的操作,携带静态字符串描述。
    Unsupported(&'static str),
    /// 非法的 CAN ID (超出 11 / 29 位范围)。
    InvalidId,
    /// 非法的 CANopen 节点号 (超出 1..=127 或 0..=127 语义范围)。
    InvalidNode,
    /// 帧数据过长 (标准帧 > 8 字节,或 CANFD > 64 字节)。
    FrameTooLong,
    /// 操作超时 (如 [`CanBackend::read_frame`] 在超时窗口内未收到帧)。
    Timeout,
    /// 设备已拔出 (USB 热插拔移除)。
    DeviceUnplugged,
}

impl PartialEq for CanError {
    /// 判等比较。
    ///
    /// `Io` 变体按 `io::ErrorKind` 比较 (底层 `io::Error` 未实现 `PartialEq`)。
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CanError::NotFound, CanError::NotFound)
            | (CanError::BusError, CanError::BusError)
            | (CanError::InvalidId, CanError::InvalidId)
            | (CanError::InvalidNode, CanError::InvalidNode)
            | (CanError::FrameTooLong, CanError::FrameTooLong)
            | (CanError::Timeout, CanError::Timeout)
            | (CanError::DeviceUnplugged, CanError::DeviceUnplugged) => true,
            (CanError::Io(a), CanError::Io(b)) => a.kind() == b.kind(),
            (CanError::Protocol(a), CanError::Protocol(b)) => a == b,
            (CanError::Unsupported(a), CanError::Unsupported(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for CanError {
    /// 人类可读的错误描述。
    ///
    /// @return 各错误类别的中文描述字符串。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CanError::NotFound => write!(f, "设备或接口不存在"),
            CanError::BusError => write!(f, "CAN 总线错误"),
            CanError::Io(e) => write!(f, "IO 错误: {e}"),
            CanError::Protocol(msg) => write!(f, "协议错误: {msg}"),
            CanError::Unsupported(msg) => write!(f, "不支持的操作: {msg}"),
            CanError::InvalidId => write!(f, "无效的 CAN ID (超出 11/29 位范围)"),
            CanError::InvalidNode => write!(f, "无效的 CANopen 节点号"),
            CanError::FrameTooLong => write!(f, "帧数据过长 (标准帧 > 8 字节, CANFD > 64 字节)"),
            CanError::Timeout => write!(f, "操作超时"),
            CanError::DeviceUnplugged => write!(f, "设备已拔出"),
        }
    }
}

impl std::error::Error for CanError {
    /// 返回底层错误来源。
    ///
    /// @return 仅 [`CanError::Io`] 携带下层 `io::Error` 来源,其余返回 `None`。
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CanError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for CanError {
    /// 将 `io::Error` 转换为 [`CanError::Io`],便于后端内部使用 `?` 传播错误。
    ///
    /// @param e 底层 IO 错误。
    /// @return 包装后的 [`CanError::Io`]。
    fn from(e: io::Error) -> Self {
        CanError::Io(e)
    }
}

/// CAN 标识符。
///
/// 统一表示 11 位标准帧与 29 位扩展帧两种 ID,构造时做范围校验,
/// 非法取值返回 [`CanError::InvalidId`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CanId {
    /// 原始 ID 数值 (标准帧 0x000 ~ 0x7FF,扩展帧 0x00000000 ~ 0x1FFFFFFF)。
    id: u32,
    /// 是否为 29 位扩展帧。
    extended: bool,
}

impl CanId {
    /// 构造标准帧 (11 位) CAN ID。
    ///
    /// @param id 11 位标识符,合法范围 0x000 ~ 0x7FF。
    /// @return 成功返回标准帧 [`CanId`];超出 0x7FF 返回 [`CanError::InvalidId`]。
    pub fn new_standard(id: u16) -> Result<Self> {
        if u32::from(id) > MAX_STANDARD_ID {
            return Err(CanError::InvalidId);
        }
        Ok(CanId {
            id: u32::from(id),
            extended: false,
        })
    }

    /// 从 u32 原始值安全构造标准帧 (11 位) CAN ID。
    ///
    /// 与 [`new_standard`] 的区别: 入参是 u32, 内部**先查 11 位范围再转型**,
    /// 避免 `as u16` 截断先于范围校验 (如 0x10000 会被静默截断为 0x0000,
    /// 0x1FFF0000 截为 0x0000)。供 REST / Tauri 等外部输入边界使用。
    ///
    /// @param id 可能超出 11 位范围的 u32 原始值。
    /// @return 成功返回标准帧 [`CanId`];超出 0x7FF 返回 [`CanError::InvalidId`]。
    pub fn new_standard_checked(id: u32) -> Result<Self> {
        if id > MAX_STANDARD_ID {
            return Err(CanError::InvalidId);
        }
        Self::new_standard(id as u16)
    }

    /// 构造扩展帧 (29 位) CAN ID。
    ///
    /// @param id 29 位标识符,合法范围 0x00000000 ~ 0x1FFFFFFF。
    /// @return 成功返回扩展帧 [`CanId`];超出 0x1FFFFFFF 返回 [`CanError::InvalidId`]。
    pub fn new_extended(id: u32) -> Result<Self> {
        if id > MAX_EXTENDED_ID {
            return Err(CanError::InvalidId);
        }
        Ok(CanId { id, extended: true })
    }

    /// 获取原始 ID 数值。
    ///
    /// @return 无扩展标志的原始 32 位 ID 数值。
    pub fn raw_id(&self) -> u32 {
        self.id
    }

    /// 判断是否为 29 位扩展帧。
    ///
    /// @return `true` 表示扩展帧,`false` 表示标准帧。
    pub fn is_extended(&self) -> bool {
        self.extended
    }

    /// 判断是否为 11 位标准帧。
    ///
    /// @return `true` 表示标准帧,`false` 表示扩展帧。
    pub fn is_standard(&self) -> bool {
        !self.extended
    }
}

impl fmt::Display for CanId {
    /// 以十六进制形式输出 ID (标准帧前缀 "11b",扩展帧前缀 "29b")。
    ///
    /// @return 如 `11b:0x7FF` / `29b:0x1FFFFFFF`。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.extended { "29b" } else { "11b" };
        write!(f, "{kind}:0x{:X}", self.id)
    }
}

/// 协议无关的 CAN 帧。
///
/// 统一表示标准帧与 CANFD 帧:携带 ID、数据区、可选接收时间戳以及
/// CANFD / BRS / ESI / RTR 等标志位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanFrame {
    /// 帧标识符 (标准 / 扩展)。
    id: CanId,
    /// 数据区 (标准帧 ≤ 8 字节,CANFD ≤ 64 字节)。
    data: Vec<u8>,
    /// 帧接收时间戳 (由后端填充,`None` 表示未知)。
    timestamp: Option<SystemTime>,
    /// CANFD 帧标志 (支持可变速率数据段)。
    fd: bool,
    /// BRS 标志 (位速率切换,仅在 FD 帧中有效)。
    brs: bool,
    /// ESI 标志 (错误状态指示,由发送节点填充)。
    esi: bool,
    /// 远程帧 (RTR) 标志。
    remote: bool,
}

impl CanFrame {
    /// 构造标准 CAN 帧。
    ///
    /// @param id   帧标识符。
    /// @param data 数据区,长度必须 ≤ [`MAX_STANDARD_DATA_LEN`] (8 字节)。
    /// @return 成功返回标准帧;数据超长返回 [`CanError::FrameTooLong`]。
    pub fn new(id: CanId, data: Vec<u8>) -> Result<Self> {
        if data.len() > MAX_STANDARD_DATA_LEN {
            return Err(CanError::FrameTooLong);
        }
        Ok(CanFrame {
            id,
            data,
            timestamp: None,
            fd: false,
            brs: false,
            esi: false,
            remote: false,
        })
    }

    /// 构造 CANFD 帧。
    ///
    /// @param id   帧标识符。
    /// @param data 数据区,长度必须 ≤ [`MAX_FD_DATA_LEN`] (64 字节)。
    /// @param brs  是否启用位速率切换。
    /// @param esi  错误状态指示标志。
    /// @return 成功返回 FD 帧;数据超长返回 [`CanError::FrameTooLong`]。
    pub fn new_fd(id: CanId, data: Vec<u8>, brs: bool, esi: bool) -> Result<Self> {
        if data.len() > MAX_FD_DATA_LEN {
            return Err(CanError::FrameTooLong);
        }
        Ok(CanFrame {
            id,
            data,
            timestamp: None,
            fd: true,
            brs,
            esi,
            remote: false,
        })
    }

    /// 获取帧标识符。
    ///
    /// @return 帧的 [`CanId`]。
    pub fn id(&self) -> CanId {
        self.id
    }

    /// 获取数据区长度。
    ///
    /// @return 数据区字节数 (标准帧 ≤ 8,FD 帧 ≤ 64)。
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 判断数据区是否为空。
    ///
    /// @return `true` 表示零长度数据帧。
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 获取数据区内容。
    ///
    /// @return 数据区字节切片。
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// 获取接收时间戳。
    ///
    /// @return 帧接收时的 [`SystemTime`],未填充时为 `None`。
    pub fn timestamp(&self) -> Option<SystemTime> {
        self.timestamp
    }

    /// 设置接收时间戳。
    ///
    /// @param ts 帧接收时间。
    pub fn set_timestamp(&mut self, ts: SystemTime) {
        self.timestamp = Some(ts);
    }

    /// 判断是否为 CANFD 帧。
    ///
    /// @return `true` 表示 FD 帧,`false` 表示标准帧。
    pub fn is_fd(&self) -> bool {
        self.fd
    }

    /// 获取 BRS (位速率切换) 标志。
    ///
    /// @return `true` 表示数据段使用更高的位速率。
    pub fn brs(&self) -> bool {
        self.brs
    }

    /// 获取 ESI (错误状态指示) 标志。
    ///
    /// @return `true` 表示发送节点处于错误状态。
    pub fn esi(&self) -> bool {
        self.esi
    }

    /// 判断是否为远程帧。
    ///
    /// @return `true` 表示 RTR 远程请求帧。
    pub fn is_remote(&self) -> bool {
        self.remote
    }

    /// 设置远程帧标志。
    ///
    /// @param remote 是否标记为远程帧。
    pub fn set_remote(&mut self, remote: bool) {
        self.remote = remote;
    }
}

/// 后端种类。
///
/// 标记一条消息 / 一次操作来自哪种后端,便于上层统一处理与路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    /// Linux SocketCAN (vcan / can 网络接口)。
    SocketCan,
    /// USBCAN (VCI) 设备。
    UsbVci,
    /// 无后端 (测试桩 / 未配置)。
    None,
}

/// 帧收发方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// 接收方向 (来自总线)。
    Rx,
    /// 发送方向 (发往总线)。
    Tx,
}

/// 后端配置。
///
/// 以配置值形式描述"如何打开一个后端",由具体后端在 [`CanBackend::open`] 时解释。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendConfig {
    /// SocketCAN 后端。
    ///
    /// @param iface 网络接口名 (如 `vcan0` / `can0`)。
    /// @param fd    是否启用 CANFD (FD 模式)。
    SocketCan {
        /// 网络接口名。
        iface: String,
        /// 是否启用 CANFD。
        fd: bool,
    },
    /// USBCAN (VCI) 后端。
    ///
    /// @param device_type  设备类型 (厂商定义的 USBCAN 设备类型码; **0 = 未指定**,
    ///                     打开时跳过该候选, 由后端按 [2E_U(21), USBCAN2(4)] 探测)。
    /// @param device_index 设备索引 (0 起, 区分同一类型的多台设备)。
    /// @param channel      通道号 (0 / 1)。
    UsbVci {
        /// USBCAN 设备类型码。
        ///
        /// 0 表示"未指定/自动": 后端 `open` 探测时跳过该候选, 直接按默认顺序
        /// [2E_U(21), USBCAN2(4)] 尝试; 非 0 时作为探测首候选 (仍可回退)。
        device_type: u32,
        /// 设备索引 (0 起; 同类型多台设备时用于区分, 默认 0)。
        device_index: u32,
        /// 通道号。
        channel: u32,
    },
    /// 无后端 (测试 / 无设备场景)。
    None,
}

/// 统一消息。
///
/// 内部各组件之间传递的标准消息单元:一帧数据 + 其来源后端 + 收发方向。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanMessage {
    /// 载荷 CAN 帧。
    pub frame: CanFrame,
    /// 消息来源后端。
    pub source: BackendKind,
    /// 消息收发方向。
    pub direction: Direction,
}

impl CanMessage {
    /// 构造统一消息。
    ///
    /// @param frame     消息载荷帧。
    /// @param source    消息来源后端种类。
    /// @param direction 收发方向。
    /// @return 组装完成的 [`CanMessage`]。
    pub fn new(frame: CanFrame, source: BackendKind, direction: Direction) -> Self {
        CanMessage {
            frame,
            source,
            direction,
        }
    }
}

/// 帧来源抽象。
///
/// 任何能够持续产生 [`CanMessage`] 的实体 (具体后端、环形缓冲、通道接收端等)
/// 都实现该 trait,供上层监控逻辑统一消费。
pub trait FrameSource {
    /// 从该来源读取一条消息。
    ///
    /// @param timeout 阻塞等待上限。
    /// @return 一条 [`CanMessage`];超时返回 [`CanError::Timeout`]。
    fn next_message(&mut self, timeout: Duration) -> Result<CanMessage>;
}

/// 后端抽象 trait。
///
/// 所有具体后端 (SocketCAN / USBCAN / 测试桩) 必须实现本 trait,从而对上层
/// 提供统一的生命周期与收发接口。设计保持同步 + 超时语义,不引入异步。
pub trait CanBackend {
    /// 按配置打开后端。
    ///
    /// @param config 后端打开配置 (接口名 / 设备类型等)。
    /// @return 成功返回已打开的后端实例;失败返回相应 [`CanError`]
    ///         (如接口不存在 → [`CanError::NotFound`])。
    fn open(config: &BackendConfig) -> Result<Self>
    where
        Self: Sized;

    /// 从总线读取一帧。
    ///
    /// @param timeout 阻塞等待一帧的最长时间。
    /// @return 成功返回收到的 [`CanFrame`];等待超过 `timeout` 返回
    ///         [`CanError::Timeout`],设备消失返回 [`CanError::DeviceUnplugged`]。
    fn read_frame(&mut self, timeout: Duration) -> Result<CanFrame>;

    /// 向总线写入一帧。
    ///
    /// @param frame 待发送的帧。
    /// @return 成功返回 `Ok(())`;失败返回相应 [`CanError`]。
    fn write_frame(&mut self, frame: &CanFrame) -> Result<()>;

    /// 关闭后端并释放资源。
    ///
    /// @return 成功返回 `Ok(())`;关闭过程出错返回相应 [`CanError`]。
    fn close(&mut self) -> Result<()>;
}

/// 设备种类。
///
/// 描述一台被发现设备所属的后端种类,便于上层对设备列表统一分类展示与路由。
/// 预留 `Other` 变体作为扩展点,未来接入新后端时无需破坏既有穷举匹配。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    /// Linux SocketCAN (vcan / can 网络接口)。
    SocketCan,
    /// USBCAN (VCI) 设备。
    UsbVci,
    /// 其他自定义后端种类。
    Other(String),
}

/// 设备详情。
///
/// 携带设备型号等附加信息,由后端在发现时填充;字段可随后端能力扩展。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceDetails {
    /// 设备型号字符串 (如 `VCI_BOARD_INFO.str_hw_Type` 的型号文本)。
    pub model: String,
}

impl DeviceDetails {
    /// 构造不含附加信息的空设备详情。
    ///
    /// @return 空 [`DeviceDetails`]。
    pub fn new() -> Self {
        DeviceDetails::default()
    }

    /// 构造带型号字符串的设备详情。
    ///
    /// @param model 设备型号文本 (如 `VCI_BOARD_INFO.str_hw_Type`)。
    /// @return 携带指定型号的 [`DeviceDetails`]。
    pub fn with_model(model: impl Into<String>) -> Self {
        DeviceDetails {
            model: model.into(),
        }
    }
}

/// 设备信息。
///
/// 设备发现的结果条目,描述一台可被后端识别的 CAN 设备。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanDeviceInfo {
    /// 设备唯一标识 (如 SocketCAN 接口名 / VCI 设备序号)。
    pub id: String,
    /// 面向用户的设备显示名称。
    pub name: String,
    /// 设备种类。
    pub kind: DeviceKind,
    /// 后端驱动标识 (如后端 crate 名)。
    pub driver: String,
    /// 设备附加详情 (型号等)。
    pub details: DeviceDetails,
    /// 设备当前是否可用 (已连接且可打开)。
    pub available: bool,
    /// 设备类型码 (厂商定义, 如 USBCAN 的 VCI 设备类型 4/21)。
    ///
    /// 由后端在发现时填充;不适用该概念的后端 (如 SocketCAN) 保持默认值 `0`。
    pub device_type: u32,
}

/// 设备发现抽象。
///
/// 任何能够枚举当前可用设备的后端都实现该 trait,供上层 (设备列表 /
/// 动态加载 / 设备管理) 统一调用;空列表表示当前无可发现设备。
pub trait DeviceDiscoverer {
    /// 枚举当前可发现的设备。
    ///
    /// @return 当前可发现的设备列表;未接入后端时返回空列表。
    fn list_devices() -> Vec<CanDeviceInfo>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    // ---- CanId 测试 ----

    /// 标准帧边界值 0x7FF 应构造成功。
    #[test]
    fn standard_id_boundary_ok() {
        assert!(CanId::new_standard(0x7FF).is_ok());
        assert!(CanId::new_standard(0).is_ok());
    }

    /// 扩展帧边界值 0x1FFFFFFF 应构造成功。
    #[test]
    fn extended_id_boundary_ok() {
        assert!(CanId::new_extended(MAX_EXTENDED_ID).is_ok());
        assert!(CanId::new_extended(0).is_ok());
    }

    /// 标准帧超界 (0x800) 应返回 InvalidId。
    #[test]
    fn standard_id_out_of_range_err() {
        assert_eq!(CanId::new_standard(0x800), Err(CanError::InvalidId));
        assert_eq!(CanId::new_standard(u16::MAX), Err(CanError::InvalidId));
    }

    /// `new_standard_checked`: 低 16 位落入合法区间但 bit≥16 置位的值 (如
    /// 0x10000-0x107FF、0x1FFF0000) 必须拒绝, 而非被 `as u16` 静默截断接受。
    #[test]
    fn new_standard_checked_rejects_truncating_overflow() {
        for id in [0x10000u32, 0x107FF, 0x1FFF_0000, 0x800, u32::MAX] {
            assert_eq!(
                CanId::new_standard_checked(id),
                Err(CanError::InvalidId),
                "0x{id:X} 应先查界再转型, 不得静默截断"
            );
        }
    }

    /// `new_standard_checked`: 合法 11 位值行为与 `new_standard` 一致。
    #[test]
    fn new_standard_checked_accepts_valid_ids() {
        for id in [0u32, 0x181, 0x7FF] {
            let can_id = CanId::new_standard_checked(id).unwrap();
            assert_eq!(can_id.raw_id(), id);
            assert!(can_id.is_standard());
        }
    }

    /// 扩展帧超界 (0x20000000) 应返回 InvalidId。
    #[test]
    fn extended_id_out_of_range_err() {
        assert_eq!(
            CanId::new_extended(MAX_EXTENDED_ID + 1),
            Err(CanError::InvalidId)
        );
        assert_eq!(CanId::new_extended(u32::MAX), Err(CanError::InvalidId));
    }

    /// 11 / 29 位标志位应正确设置。
    #[test]
    fn id_bit_flag_check() {
        let std_id = CanId::new_standard(0x123).unwrap();
        assert!(std_id.is_standard());
        assert!(!std_id.is_extended());
        assert_eq!(std_id.raw_id(), 0x123);

        let ext_id = CanId::new_extended(0x1FFFFF).unwrap();
        assert!(ext_id.is_extended());
        assert!(!ext_id.is_standard());
        assert_eq!(ext_id.raw_id(), 0x1FFFFF);
    }

    // ---- CanFrame 测试 ----

    /// 标准帧数据 8 字节 (边界) 应构造成功。
    #[test]
    fn standard_frame_8_bytes_ok() {
        let id = CanId::new_standard(0x123).unwrap();
        let data = vec![0u8; 8];
        let frame = CanFrame::new(id, data.clone()).unwrap();
        assert_eq!(frame.len(), 8);
        assert_eq!(frame.data(), data.as_slice());
        assert!(!frame.is_fd());
    }

    /// 标准帧数据 9 字节应返回 FrameTooLong。
    #[test]
    fn standard_frame_9_bytes_err() {
        let id = CanId::new_standard(0x123).unwrap();
        assert_eq!(CanFrame::new(id, vec![0u8; 9]), Err(CanError::FrameTooLong));
    }

    /// FD 帧数据 64 字节 (边界) 应构造成功。
    #[test]
    fn fd_frame_64_bytes_ok() {
        let id = CanId::new_extended(0x1F).unwrap();
        let frame = CanFrame::new_fd(id, vec![0u8; 64], true, true).unwrap();
        assert_eq!(frame.len(), 64);
        assert!(frame.is_fd());
        assert!(frame.brs());
        assert!(frame.esi());
    }

    /// FD 帧数据 65 字节应返回 FrameTooLong。
    #[test]
    fn fd_frame_65_bytes_err() {
        let id = CanId::new_extended(0x1F).unwrap();
        assert_eq!(
            CanFrame::new_fd(id, vec![0u8; 65], false, false),
            Err(CanError::FrameTooLong)
        );
    }

    /// 字段访问:len / data / id / 标志位 / 时间戳。
    #[test]
    fn frame_field_access() {
        let id = CanId::new_standard(0x456).unwrap();
        let mut frame = CanFrame::new(id, vec![1, 2, 3]).unwrap();
        assert_eq!(frame.id(), id);
        assert_eq!(frame.len(), 3);
        assert_eq!(frame.data(), &[1, 2, 3]);
        assert!(!frame.is_remote());
        assert!(!frame.brs());
        assert!(!frame.esi());
        assert!(!frame.is_fd());
        assert_eq!(frame.timestamp(), None);

        frame.set_remote(true);
        assert!(frame.is_remote());

        let ts = SystemTime::now();
        frame.set_timestamp(ts);
        assert_eq!(frame.timestamp(), Some(ts));
    }

    // ---- CanError 测试 ----

    /// Display 应输出可读的中文描述。
    #[test]
    fn error_display_impl() {
        assert_eq!(CanError::NotFound.to_string(), "设备或接口不存在");
        assert_eq!(
            CanError::InvalidId.to_string(),
            "无效的 CAN ID (超出 11/29 位范围)"
        );
        assert_eq!(CanError::Timeout.to_string(), "操作超时");
        assert_eq!(CanError::BusError.to_string(), "CAN 总线错误");
        assert_eq!(CanError::Protocol("bad").to_string(), "协议错误: bad");
        assert_eq!(CanError::Unsupported("x").to_string(), "不支持的操作: x");
    }

    /// 应为 std::error::Error 实现,且 Io 变体暴露 source。
    #[test]
    fn error_trait_impl() {
        let e: Box<dyn std::error::Error> = Box::new(CanError::DeviceUnplugged);
        assert_eq!(e.to_string(), "设备已拔出");

        let io_err = io::Error::other("底层失败");
        let can_err = CanError::Io(io_err);
        assert!(can_err.source().is_some());
        // 非 Io 变体没有 source。
        assert!(CanError::NotFound.source().is_none());
    }

    /// io::Error 应能通过 From 转换。
    #[test]
    fn io_error_conversion() {
        let io_err = io::Error::new(io::ErrorKind::TimedOut, "timeout");
        let converted: CanError = io_err.into();
        match converted {
            CanError::Io(inner) => assert_eq!(inner.kind(), io::ErrorKind::TimedOut),
            other => panic!("期望 Io 变体,实际: {other}"),
        }
    }
}
