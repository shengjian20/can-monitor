//! # j1939-stack — J1939 协议解析服务
//!
//! 面向用户态 raw CAN 帧流提供 J1939 协议解析与传输层重组能力,与 CANopen
//! 栈 (11 位标准帧) 在同一 socket 上共存:本服务仅消费 29 位扩展帧,标准帧
//! 直接返回 `None`,由上层按帧类型分发。
//!
//! 核心能力:
//! - [`J1939Service::parse_id`] — 29 位 ID 位域解析 (优先级 / PGN / 源地址 / 目的地址)
//! - [`J1939Service::parse`] — 单帧分发:直接消息、TP.CM (PGN `0xEC00`)、
//!   TP.DT (PGN `0xEB00`),并驱动 TP.BAM 多包消息重组
//! - [`J1939Message`] — 统一解析结果 (`Transport` / `Direct` / `Diagnostic`)
//! - 常用 PGN 常量表 (见 [`PGN_TP_CM`] 等)
//!
//! 传输层重组委托给 `sae-j1939-host` 0.4.0 (MIT OR Apache-2.0) 的
//! `tp::Reassembler` (BAM + RTS/CTS,最大 1785 字节)。本 crate 的公共 API
//! **不泄漏** `sae-j1939-host` / `sae-j1939-rs` 的任何类型,只暴露自有类型。
//! 本栈不实现 ETP 扩展传输协议 (>1785 字节)。

use std::collections::HashMap;
use std::time::Instant;

use can_types::{CanFrame, CanId};
use sae_j1939_host::sae_j1939_rs::{
    pgn as sae_pgn, tp::{Reassembler, Rx, TpCm, TpDt},
    Address, Id,
};

// ---------------------------------------------------------------------------
// 常用 PGN 常量表 (取值经 sae-j1939-host 的 PGN codec 换算,公共 API 只暴露 u32)
// ---------------------------------------------------------------------------

/// 传输协议 — 连接管理 (TP.CM),J1939-21。
pub const PGN_TP_CM: u32 = sae_pgn::TP_CM.as_u32();
/// 传输协议 — 数据传送 (TP.DT),J1939-21。
pub const PGN_TP_DT: u32 = sae_pgn::TP_DT.as_u32();
/// 请求 (Request),J1939-21 — 请求另一 ECU 发送指定 PGN。
pub const PGN_REQUEST: u32 = sae_pgn::REQUEST.as_u32();
/// 应答 (Acknowledgement),J1939-21 — 对请求的 ACK / NACK。
pub const PGN_ACK: u32 = sae_pgn::ACKNOWLEDGEMENT.as_u32();
/// 地址声明 (Address Claimed),J1939-81。
pub const PGN_ADDRESS_CLAIMED: u32 = sae_pgn::ADDRESS_CLAIMED.as_u32();
/// DM1 — 当前激活的诊断故障码, J1939-73。
pub const PGN_DM1: u32 = sae_pgn::DM1.as_u32();
/// DM2 — 历史激活的诊断故障码, J1939-73。
pub const PGN_DM2: u32 = sae_pgn::DM2.as_u32();
/// EEC1 (电子发动机控制器 1) — 发动机转速与扭矩, J1939-71。
pub const PGN_ENGINE_SPEED: u32 = sae_pgn::EEC1.as_u32();
/// EEC2 — 踏板位置与发动机负荷, J1939-71。
pub const PGN_ENGINE_LOAD: u32 = sae_pgn::EEC2.as_u32();
/// 发动机温度 1 (ET1) — 冷却液 / 燃油 / 机油温度, J1939-71。
pub const PGN_ENGINE_TEMPERATURE_1: u32 = sae_pgn::ENGINE_TEMPERATURE_1.as_u32();
/// 巡航控制 / 车速 1 (CCVS1) — 轮速车速, J1939-71。
pub const PGN_CCVS1: u32 = sae_pgn::CRUISE_CONTROL_VEHICLE_SPEED.as_u32();
/// ECU 标识 (ECU Identification), J1939-71 — 常以 BAM 多包方式传送。
pub const PGN_ECU_IDENTIFICATION: u32 = sae_pgn::ECU_IDENTIFICATION.as_u32();
/// 软件标识 (Software Identification), J1939-71 — 常以 BAM 多包方式传送。
pub const PGN_SOFTWARE_IDENTIFICATION: u32 = sae_pgn::SOFTWARE_IDENTIFICATION.as_u32();
/// Proprietary A (厂商私有), J1939-21 — PDU1 点对点。
pub const PGN_PROPRIETARY_A: u32 = sae_pgn::PROPRIETARY_A.as_u32();

/// 判定某 PGN 是否属于诊断类参数组 (DM1 / DM2)。
///
/// @param pgn 待判定的参数组号。
/// @return `true` 表示 DM1 或 DM2。
fn is_diagnostic_pgn(pgn: u32) -> bool {
    pgn == PGN_DM1 || pgn == PGN_DM2
}

// ---------------------------------------------------------------------------
// 类型定义
// ---------------------------------------------------------------------------

/// 解析后的 J1939 帧头信息。
///
/// 由 [`J1939Service::parse_id`] 从 29 位扩展帧 ID 中位域解析得到。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct J1939Header {
    /// 3 位消息优先级 (bit 26-28),0 最高、7 最低。
    pub priority: u8,
    /// 参数组号 (PDU1 已归一化,低字节为 0;PDU2 含组扩展字节)。
    pub pgn: u32,
    /// 源地址 (SA, bit 0-7)。
    pub source_addr: u8,
    /// 目的地址 (仅 PDU1 / PF<0xF0 时有效,取自 PS 字节);PDU2 广播为 `None`。
    pub dest_addr: Option<u8>,
    /// 是否为广播帧 (PDU2,或 PDU1 且目的地址为 `0xFF`)。
    pub is_broadcast: bool,
}

/// J1939 解析结果消息。
///
/// 按帧类型与参数组分类:
/// - [`J1939Message::Transport`]:传输协议活动 (TP.CM / TP.DT),重组完成时
///   [`J1939Message::Transport::reassembled`] 携带完整负载。
/// - [`J1939Message::Direct`]:单帧直接消息 (长度 ≤ 8 字节)。
/// - [`J1939Message::Diagnostic`]:诊断类直接消息 (DM1 / DM2 单帧)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum J1939Message {
    /// 传输协议消息 (多包重组过程或结果)。
    ///
    /// @param pgn          被传输消息的参数组号 (TP.CM 中声明 / 会话记录)。
    /// @param source       发送方源地址。
    /// @param total_len    被传输消息的总字节数。
    /// @param reassembled  重组完成的完整负载;尚未完成时为 `None`。
    Transport {
        /// 被传输消息的参数组号。
        pgn: u32,
        /// 发送方源地址。
        source: u8,
        /// 被传输消息的总字节数。
        total_len: usize,
        /// 重组完成的完整负载,未完成时为 `None`。
        reassembled: Option<Vec<u8>>,
    },
    /// 单帧直接消息 (未使用传输协议的参数组)。
    ///
    /// @param pgn     参数组号。
    /// @param source  发送方源地址。
    /// @param data    帧数据区 (≤ 8 字节)。
    Direct {
        /// 参数组号。
        pgn: u32,
        /// 发送方源地址。
        source: u8,
        /// 帧数据区内容。
        data: Vec<u8>,
    },
    /// 诊断类单帧消息 (DM1 / DM2 直接发送)。
    ///
    /// @param pgn     参数组号 (DM1 / DM2)。
    /// @param source  发送方源地址。
    /// @param data    帧数据区内容。
    Diagnostic {
        /// 参数组号。
        pgn: u32,
        /// 发送方源地址。
        source: u8,
        /// 帧数据区内容。
        data: Vec<u8>,
    },
}

/// 内部传输层会话元数据。
///
/// 与 `Reassembler` 的缓冲区互为镜像:缓冲区负责真正的字节填充,
/// 本表负责记录"正在传送哪个 PGN / 总长多少",使部分到达的 TP.DT 也能
/// 上报有意义的 `Transport` 信息。
#[derive(Debug, Clone, Copy)]
struct TransportSession {
    /// 被传输消息的参数组号。
    pgn: u32,
    /// 被传输消息的总字节数。
    total_len: usize,
}

/// J1939 协议解析服务。
///
/// 有状态:维护 TP 重组会话 (按源地址索引,与 J1939-21"每源同时仅一个会话"
/// 的约束一致) 与超时清理。用法:
///
/// ```
/// use can_types::{CanFrame, CanId};
/// use j1939_stack::J1939Service;
///
/// let mut service = J1939Service::new();
/// let id = CanId::new_extended(0x18F00480).unwrap();
/// let frame = CanFrame::new(id, vec![0x00; 8]).unwrap();
/// assert!(service.parse(&frame).is_some());
/// ```
///
/// 同一实例需串行喂帧;`parse` 内部按真实墙钟推进超时,
/// 也可通过 [`J1939Service::tick`] 显式推进 (便于测试)。
#[derive(Debug)]
pub struct J1939Service {
    /// TP 重组器:每源一个会话,最大 1785 字节,最多 8 个并发会话。
    reassembler: Reassembler<1785, 8>,
    /// 会话元数据镜像 (源地址 → 会话)。
    sessions: HashMap<u8, TransportSession>,
    /// 会话无新包的超时阈值 (毫秒),默认 2000。
    timeout_ms: u16,
    /// 上一次推进超时的时间点。
    last_tick: Instant,
}

impl Default for J1939Service {
    /// 默认构造,等价于 [`J1939Service::new`]。
    fn default() -> Self {
        Self::new()
    }
}

impl J1939Service {
    /// 构造解析服务,使用默认重组超时 (2000 ms)。
    ///
    /// @return 已初始化、无活动会话的 [`J1939Service`]。
    pub fn new() -> Self {
        Self {
            reassembler: Reassembler::new(),
            sessions: HashMap::new(),
            timeout_ms: 2000,
            last_tick: Instant::now(),
        }
    }

    /// 构造解析服务并指定重组会话超时。
    ///
    /// @param timeout_ms 会话在无新 TP.DT 包时被丢弃的阈值 (毫秒)。
    /// @return 已初始化、使用给定超时的 [`J1939Service`]。
    pub fn with_timeout(timeout_ms: u16) -> Self {
        Self {
            timeout_ms,
            ..Self::new()
        }
    }

    /// 解析 29 位扩展帧 ID 的位域。
    ///
    /// 仅处理扩展帧;标准 (11 位) 帧返回 `None`。位域布局
    /// (J1939-21):优先级 bit 26-28, EDP bit 25, DP bit 24, PF bit 16-23,
    /// PS bit 8-15, SA bit 0-7。PGN 计算:
    /// - PF < `0xF0` (PDU1): `PGN = DP<<16 | PF<<8`,PS 为**目的地址**;
    /// - PF ≥ `0xF0` (PDU2): `PGN = DP<<16 | PF<<8 | PS` (PS 是组扩展,属 PGN)。
    ///
    /// @param id 待解析的 CAN 标识符。
    /// @return 解析成功的 [`J1939Header`];标准帧或非法 ID 返回 `None`。
    pub fn parse_id(id: &CanId) -> Option<J1939Header> {
        if !id.is_extended() {
            return None;
        }
        let raw = id.raw_id();
        if raw > can_types::MAX_EXTENDED_ID {
            return None;
        }
        // 合法扩展帧必然在 29 位范围内,掩码构造避免出错分支。
        let jid = Id::new_masked(raw);
        let dest_addr = jid.destination_address().map(|a| a.as_u8());
        Some(J1939Header {
            priority: jid.priority().as_u8(),
            pgn: jid.pgn().as_u32(),
            source_addr: jid.source_address().as_u8(),
            dest_addr,
            is_broadcast: jid.is_broadcast(),
        })
    }

    /// 解析一帧 CAN 数据并推进重组状态机。
    ///
    /// 分发规则:PGN `0xEC00` → TP.CM 处理;PGN `0xEB00` → TP.DT 处理;
    /// 其余 PGN → 直接消息 (DM1/DM2 归为 `Diagnostic`)。调用时先按真实
    /// 墙钟推进超时,丢弃超时会话后再处理当前帧。
    ///
    /// @param frame 待解析的 CAN 帧。
    /// @return 解析结果 [`J1939Message`];非扩展帧、未知传输帧或
    ///         无法归属的孤儿 TP.DT 返回 `None`。
    pub fn parse(&mut self, frame: &CanFrame) -> Option<J1939Message> {
        // 先按真实墙钟推进超时,丢弃空闲过久的会话。
        let now = Instant::now();
        let elapsed_ms = (now - self.last_tick)
            .as_millis()
            .min(u16::MAX as u128) as u16;
        self.tick(elapsed_ms);
        self.last_tick = now;

        let header = Self::parse_id(&frame.id())?;
        match header.pgn {
            PGN_TP_CM => self.handle_tp_cm(frame, &header),
            PGN_TP_DT => self.handle_tp_dt(frame, &header),
            _ if is_diagnostic_pgn(header.pgn) => Some(J1939Message::Diagnostic {
                pgn: header.pgn,
                source: header.source_addr,
                data: frame.data().to_vec(),
            }),
            _ => Some(J1939Message::Direct {
                pgn: header.pgn,
                source: header.source_addr,
                data: frame.data().to_vec(),
            }),
        }
    }

    /// 显式推进重组会话超时,丢弃空闲超过阈值的会话。
    ///
    /// 正常情况下由 [`J1939Service::parse`] 自动调用;暴露出来便于测试
    /// 与批处理场景 (如按固定节拍清扫会话)。
    ///
    /// @param elapsed_ms 距上次推进经过的毫秒数。
    /// @return 本次被丢弃的会话数量。
    pub fn tick(&mut self, elapsed_ms: u16) -> usize {
        let mut dropped = 0;
        let timeout = self.timeout_ms;
        self.reassembler
            .tick_with_timeout(elapsed_ms, timeout, |addr, _| {
                // 广播会话无回程通道,此处不需要发送任何响应。
                self.sessions.remove(&addr.as_u8());
                dropped += 1;
            });
        dropped
    }

    /// 当前活动会话数量。
    ///
    /// @return 进行中的传输层重组会话数。
    pub fn active_sessions(&self) -> usize {
        self.sessions.len()
    }

    /// 处理 TP.CM 连接管理帧。
    ///
    /// BAM / RTS 声明会注册新会话 (RTS 需要的 CTS 应答被忽略,本服务为
    /// 只读监视器,不回写总线);CTS / EOM / Abort 等发端消息不注册会话,
    /// 返回 `None`。
    ///
    /// @param frame  包含 TP.CM 负载的帧。
    /// @param header 已解析的帧头。
    /// @return 注册成功返回 `Transport` (未完成);无效或非建会话消息返回 `None`。
    fn handle_tp_cm(&mut self, frame: &CanFrame, header: &J1939Header) -> Option<J1939Message> {
        let payload = pad8(frame.data());
        let cm = TpCm::decode(&payload).ok()?;
        let source = Address::new(header.source_addr);
        let _ = self.reassembler.on_tp_cm(source, &cm);
        if !self.reassembler.is_receiving_from(source) {
            // 未被接受: 超界 BAM、无空闲槽位或非建会话消息。
            return None;
        }
        let pgn = cm.pgn().as_u32();
        let total_len = cm_size(&cm)? as usize;
        self.sessions.insert(
            header.source_addr,
            TransportSession { pgn, total_len },
        );
        Some(J1939Message::Transport {
            pgn,
            source: header.source_addr,
            total_len,
            reassembled: None,
        })
    }

    /// 处理 TP.DT 数据传送帧。
    ///
    /// @param frame  包含 TP.DT 负载的帧。
    /// @param header 已解析的帧头。
    /// @return 重组中返回 `Transport` (未完成);末包返回 `Transport` (已完成);
    ///         孤儿 TP.DT (无会话可归属) 返回 `None`。
    fn handle_tp_dt(&mut self, frame: &CanFrame, header: &J1939Header) -> Option<J1939Message> {
        if frame.data().is_empty() {
            return None;
        }
        let payload = pad8(frame.data());
        let dt = TpDt::decode(&payload);
        let source = Address::new(header.source_addr);
        match self.reassembler.on_tp_dt(source, &dt) {
            Rx::Message { pgn, data, .. } => {
                // 会话在完成时即被 Reassembler 释放。
                let meta = self.sessions.remove(&header.source_addr);
                let total_len = meta.map_or(data.len(), |m| m.total_len);
                Some(J1939Message::Transport {
                    pgn: pgn.as_u32(),
                    source: header.source_addr,
                    total_len,
                    reassembled: Some(data.to_vec()),
                })
            }
            _ => {
                if self.reassembler.is_receiving_from(source) {
                    // 包被接受,重组仍在进行。
                    let meta = self.sessions.get(&header.source_addr).copied();
                    Some(J1939Message::Transport {
                        pgn: meta.map_or(PGN_TP_DT, |m| m.pgn),
                        source: header.source_addr,
                        total_len: meta.map_or(frame.data().len(), |m| m.total_len),
                        reassembled: None,
                    })
                } else {
                    // 乱序 / 重复包导致会话被丢弃,或本来就是孤儿包。
                    self.sessions
                        .remove(&header.source_addr)
                        .map(|meta| J1939Message::Transport {
                            pgn: meta.pgn,
                            source: header.source_addr,
                            total_len: meta.total_len,
                            reassembled: None,
                        })
                }
            }
        }
    }
}

/// 将帧数据补齐为 8 字节的 TP 负载,不足部分以 `0xFF` 填充。
///
/// @param data 帧数据区。
/// @return 定长 8 字节负载数组。
fn pad8(data: &[u8]) -> [u8; 8] {
    let mut buf = [0xFF; 8];
    let n = data.len().min(8);
    buf[..n].copy_from_slice(&data[..n]);
    buf
}

/// 从 TP.CM 中取出被传输消息的总长度。
///
/// 仅 BAM / RTS / EOM 变体携带长度;CTS / Abort 无此字段,返回 `None`。
///
/// @param cm 已解码的 TP.CM 消息。
/// @return 消息总字节数;无法提供时为 `None`。
fn cm_size(cm: &TpCm) -> Option<u16> {
    match *cm {
        TpCm::Bam { size, .. }
        | TpCm::Rts { size, .. }
        | TpCm::EndOfMsgAck { size, .. } => Some(size),
        TpCm::Cts { .. } | TpCm::Abort { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一帧扩展 CAN 帧。
    fn frame(raw_id: u32, data: Vec<u8>) -> CanFrame {
        let id = CanId::new_extended(raw_id).unwrap();
        CanFrame::new(id, data).unwrap()
    }

    /// 标准帧 (11 位) 应被解析服务拒绝。
    #[test]
    fn standard_frame_is_rejected() {
        let id = CanId::new_standard(0x123).unwrap();
        assert!(J1939Service::parse_id(&id).is_none());

        let frame = CanFrame::new(id, vec![0u8; 8]).unwrap();
        let mut service = J1939Service::new();
        assert!(service.parse(&frame).is_none());
    }

    /// 29 位 ID 位域解析: 0x18FEF100 → priority 6, PGN 0xFEF1, SA 0, 广播。
    #[test]
    fn parses_pdu2_broadcast_id() {
        let header = J1939Service::parse_id(&CanId::new_extended(0x18FEF100).unwrap()).unwrap();
        assert_eq!(header.priority, 6);
        assert_eq!(header.pgn, 0x00FEF1);
        assert_eq!(header.source_addr, 0x00);
        assert_eq!(header.dest_addr, None);
        assert!(header.is_broadcast);
    }

    /// PDU1 帧: 0x0CEFC011 → PF=0xEF<0xF0, PGN 0xEF00, PS 0xC0 是目的地址, SA 0x11。
    ///
    /// 注: 任务描述写 "PGN=0xEFC0" 与位域规则 (PF<0xF0 时 PS 不计入 PGN) 矛盾;
    /// 此处以 J1939 标准位域规则为准,PS 作为目的地址处理。
    #[test]
    fn parses_pdu1_addressed_id() {
        let header = J1939Service::parse_id(&CanId::new_extended(0x0CEFC011).unwrap()).unwrap();
        assert_eq!(header.priority, 3);
        assert_eq!(header.pgn, 0x00EF00); // Proprietary A, PS 不并入 PGN
        assert_eq!(header.dest_addr, Some(0xC0));
        assert_eq!(header.source_addr, 0x11);
        assert!(!header.is_broadcast);
    }

    /// PDU2 帧: 0x18FF1017 → PF=0xFF≥0xF0, PGN 含组扩展字节 → 0xFF10, SA 0x17。
    #[test]
    fn parses_pdu2_with_group_extension() {
        let header = J1939Service::parse_id(&CanId::new_extended(0x18FF1017).unwrap()).unwrap();
        assert_eq!(header.priority, 6);
        assert_eq!(header.pgn, 0x00FF10); // Proprietary B, PS=0x10 并入 PGN
        assert_eq!(header.source_addr, 0x17);
        assert_eq!(header.dest_addr, None);
        assert!(header.is_broadcast);
    }

    /// PGN 计算边界: PF=0xEF (PDU1, 不带组扩展) vs PF=0xF0 (PDU2, 带组扩展)。
    #[test]
    fn pgn_boundary_pf_ef_vs_f0() {
        // PF=0xEF: PS 是目的地址, 不入 PGN。
        let pdu1 = J1939Service::parse_id(&CanId::new_extended(0x0CEF2380).unwrap()).unwrap();
        assert_eq!(pdu1.pgn, 0x00EF00);
        assert_eq!(pdu1.dest_addr, Some(0x23));
        assert!(!pdu1.is_broadcast);

        // PF=0xF0: PS=0x04 是组扩展, 入 PGN → 0xF004 (EEC1)。
        let pdu2 = J1939Service::parse_id(&CanId::new_extended(0x18F00480).unwrap()).unwrap();
        assert_eq!(pdu2.pgn, 0x00F004);
        assert_eq!(pdu2.dest_addr, None);
        assert!(pdu2.is_broadcast);

        // 数据页位: DP=1 时 PGN 高位为 0x01。
        let dp = J1939Service::parse_id(&CanId::new_extended(0x19F04080).unwrap()).unwrap();
        assert_eq!(dp.pgn, 0x0001F040);
    }

    /// 常用 PGN 常量值核对。
    #[test]
    fn pgn_constants_have_expected_values() {
        assert_eq!(PGN_TP_CM, 0x00EC00);
        assert_eq!(PGN_TP_DT, 0x00EB00);
        assert_eq!(PGN_REQUEST, 0x00EA00);
        assert_eq!(PGN_ACK, 0x00E800);
        assert_eq!(PGN_DM1, 0x00FECA);
        assert_eq!(PGN_DM2, 0x00FECB);
        assert_eq!(PGN_ENGINE_SPEED, 0x00F004);
        assert_eq!(PGN_ENGINE_LOAD, 0x00F003);
        assert_eq!(PGN_CCVS1, 0x00FEF1);
    }

    /// 直接消息: PGN 0xF004 (EEC1) 数据原样返回。
    #[test]
    fn direct_message_returns_data() {
        let mut service = J1939Service::new();
        let msg = service
            .parse(&frame(0x18F00480, vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]))
            .unwrap();
        match msg {
            J1939Message::Direct { pgn, source, data } => {
                assert_eq!(pgn, 0x00F004);
                assert_eq!(source, 0x80);
                assert_eq!(data, vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);
            }
            other => panic!("期望 Direct, 实际 {other:?}"),
        }
    }

    /// 直接 DM1 单帧应归为 Diagnostic。
    #[test]
    fn direct_diagnostic_message_is_classified() {
        let mut service = J1939Service::new();
        let msg = service.parse(&frame(0x18FECA80, vec![0x41, 0x02, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF])).unwrap();
        match msg {
            J1939Message::Diagnostic { pgn, source, data } => {
                assert_eq!(pgn, PGN_DM1);
                assert_eq!(source, 0x80);
                assert_eq!(data.len(), 8);
            }
            other => panic!("期望 Diagnostic, 实际 {other:?}"),
        }
    }

    /// BAM 重组: 3 个 TP.DT 分包 (共 20 字节) 完整重组。
    #[test]
    fn reassembles_a_20_byte_bam() {
        let mut service = J1939Service::new();
        let payload: Vec<u8> = (1..=20).collect();

        // TP.CM BAM: [0x20, size_lo, size_hi, packets, 0xFF, pgn 低中高]
        let cm = service
            .parse(&frame(0x1CECFF80, vec![0x20, 0x14, 0x00, 0x03, 0xFF, 0xC5, 0xFD, 0x00]))
            .unwrap();
        match cm {
            J1939Message::Transport { pgn, total_len, reassembled, .. } => {
                assert_eq!(pgn, PGN_ECU_IDENTIFICATION);
                assert_eq!(total_len, 20);
                assert!(reassembled.is_none());
            }
            other => panic!("期望 Transport, 实际 {other:?}"),
        }
        assert_eq!(service.active_sessions(), 1);

        // TP.DT 包 1-3: data[0]=包序号, data[1..8]=负载 7 字节。
        let dt = |seq: u8, chunk: &[u8]| {
            let mut d = vec![seq];
            d.extend_from_slice(chunk);
            frame(0x1CEBFF80, d)
        };
        let p1 = service.parse(&dt(1, &payload[0..7])).unwrap();
        assert!(matches!(
            p1,
            J1939Message::Transport { reassembled: None, .. }
        ));
        // 包 2
        let p2 = service.parse(&dt(2, &payload[7..14])).unwrap();
        assert!(matches!(
            p2,
            J1939Message::Transport { reassembled: None, .. }
        ));
        // 包 3 (6 字节) — 重组完成。
        match service.parse(&dt(3, &payload[14..20])).unwrap() {
            J1939Message::Transport { pgn, source, total_len, reassembled } => {
                assert_eq!(pgn, PGN_ECU_IDENTIFICATION);
                assert_eq!(source, 0x80);
                assert_eq!(total_len, 20);
                assert_eq!(reassembled, Some(payload));
            }
            other => panic!("期望重组完成, 实际 {other:?}"),
        }
        assert_eq!(service.active_sessions(), 0);
    }

    /// 包号乱序: BAM 会话被丢弃, 消息永远不会重组。
    #[test]
    fn out_of_order_bam_packet_drops_session() {
        let mut service = J1939Service::new();

        // 声明 21 字节 (3 包) 的 DM1 BAM。
        service.parse(&frame(0x1CECFF80, vec![0x20, 0x15, 0x00, 0x03, 0xFF, 0xCA, 0xFE, 0x00]));
        // 先到包 3 → 乱序, 会话被丢弃。
        let res = service.parse(&frame(0x1CEBFF80, vec![0x03, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]));
        match res {
            Some(J1939Message::Transport { reassembled: None, .. }) => {}
            other => panic!("期望乱序包丢弃 Transport, 实际 {other:?}"),
        }
        assert_eq!(service.active_sessions(), 0);
        // 随后的包 1 找不到会话 → 孤儿, 返回 None。
        assert!(service
            .parse(&frame(0x1CEBFF80, vec![0x01, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]))
            .is_none());
    }

    /// 超时清理: 会话在超过阈值无新包后丢弃。
    #[test]
    fn idle_session_times_out_and_is_dropped() {
        let mut service = J1939Service::with_timeout(2000);

        // 注册 BAM 会话。
        service.parse(&frame(0x1CECFF80, vec![0x20, 0x0C, 0x00, 0x02, 0xFF, 0xC5, 0xFD, 0x00]));
        assert_eq!(service.active_sessions(), 1);

        // 2.5 s 无新包 → 超时丢弃 1 个会话。
        assert_eq!(service.tick(2500), 1);
        assert_eq!(service.active_sessions(), 0);

        // 迟到的 TP.DT 找不到会话 → None。
        let res = service.parse(&frame(0x1CEBFF80, vec![0x01; 8]));
        assert!(res.is_none());
    }

    /// 未超时 (1 s) 时会话保留, 迟到包仍能继续重组。
    #[test]
    fn session_survives_below_timeout_threshold() {
        let mut service = J1939Service::with_timeout(2000);
        service.parse(&frame(0x1CECFF80, vec![0x20, 0x0C, 0x00, 0x02, 0xFF, 0xC5, 0xFD, 0x00]));

        assert_eq!(service.tick(1000), 0);
        assert_eq!(service.active_sessions(), 1);

        // 第一包到达, 重组继续。
        let res = service
            .parse(&frame(0x1CEBFF80, vec![0x01, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10]))
            .unwrap();
        assert!(matches!(
            res,
            J1939Message::Transport { reassembled: None, .. }
        ));
    }
}
