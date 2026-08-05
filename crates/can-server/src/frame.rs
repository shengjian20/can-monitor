//! # 帧 JSON 序列化与批量攒批逻辑
//!
//! 与 Web 前端约定的帧 JSON 契约 (三形态 TUI / Web / GUI 统一, 定义见 T17):
//!
//! ```json
//! { "ts": "1750000000000", "id": "0x181", "ext": false, "dir": "rx",
//!   "data": "01 02 03", "protocol": "canopen", "summary": "Pdo { .. }" }
//! ```
//!
//! 字段约定:
//!
//! - `ts`       毫秒时间戳, **字符串** (JS Number 精确整数上限 2^53, u64 会溢出);
//! - `id`       十六进制 CAN ID (小写 `0x` 前缀 + 大写十六进制数字, 如 `0x181`);
//! - `ext`      是否 29 位扩展帧;
//! - `dir`      `"rx"` / `"tx"` 收发方向;
//! - `data`     大写十六进制、空格分隔的数据字节 (如 `"01 02 03"`);
//! - `protocol` 三值之一: `"canopen"` / `"j1939"` / `"raw"`;
//! - `summary`  人类可读摘要 (协议栈解析结果 Debug 输出; raw 帧为空串)。

use std::time::{SystemTime, UNIX_EPOCH};

use can_monitor_core::classifier::{ParsedMessage, StreamItem};
use can_types::Direction;
use serde::{Deserialize, Serialize};

/// 一帧的 Web 侧 JSON 表示 (与 T17 三形态统一契约一致)。
///
/// `Deserialize` 派生供测试 / 后续服务端解析回读使用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameJson {
    /// 毫秒时间戳 (字符串, 防 JS Number 2^53 精度丢失)。
    pub ts: String,
    /// 十六进制 CAN ID (如 `"0x181"` / `"0x18FEF100"`)。
    pub id: String,
    /// 是否 29 位扩展帧。
    pub ext: bool,
    /// 收发方向: `"rx"` / `"tx"`。
    pub dir: String,
    /// 大写十六进制、空格分隔的数据字节 (如 `"01 02 03"`)。
    pub data: String,
    /// 协议类别: `"canopen"` / `"j1939"` / `"raw"`。
    pub protocol: String,
    /// 可读摘要 (协议栈解析结果; raw 帧为空串)。
    pub summary: String,
}

/// 将一条流元素转为帧 JSON (纯函数, 无副作用)。
///
/// 直接消费 [`StreamItem::parsed`] 里的分类结果, **不再次调用分类器** ——
/// 保证整条数据通路 (读帧 → 分类 → 广播 → Web) 每帧恰好被分类一次。
///
/// @param item 广播流中的一条元素 (原始消息 + 一次分类结果)。
/// @return 符合 Web 契约的 [`FrameJson`]。
pub fn frame_to_json(item: &StreamItem) -> FrameJson {
    let frame = &item.msg.frame;
    let ts_ms = frame
        .timestamp()
        .unwrap_or(SystemTime::now())
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let (protocol, summary) = match &item.parsed {
        ParsedMessage::Canopen { msg, .. } => ("canopen".to_string(), format!("{msg:?}")),
        ParsedMessage::J1939 { msg, .. } => ("j1939".to_string(), format!("{msg:?}")),
        ParsedMessage::Raw(_) => ("raw".to_string(), String::new()),
    };

    let data_hex: String = frame
        .data()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ");

    FrameJson {
        ts: ts_ms.to_string(),
        id: format!("0x{:X}", frame.id().raw_id()),
        ext: frame.id().is_extended(),
        dir: match item.msg.direction {
            Direction::Rx => "rx".to_string(),
            Direction::Tx => "tx".to_string(),
        },
        data: data_hex,
        protocol,
        summary,
    }
}

/// 批量攒批器 (纯逻辑, 可单测)。
///
/// WS 转发循环用它攒帧: 每 push 一帧, 达到批量上限返回 `true` 表示应立即
/// 刷出; 未达上限则留在缓冲, 由定时器到点后 [`BatchCollector::take`] 取走。
#[derive(Debug)]
pub struct BatchCollector {
    /// 已攒未刷出的帧。
    pending: Vec<FrameJson>,
    /// 攒满即刷的批量上限。
    max_batch: usize,
}

impl BatchCollector {
    /// 构造攒批器。
    ///
    /// @param max_batch 批量上限 (至少为 1)。
    pub fn new(max_batch: usize) -> Self {
        Self {
            pending: Vec::new(),
            max_batch: max_batch.max(1),
        }
    }

    /// 追加一帧。
    ///
    /// @param json 待攒批的帧 JSON。
    /// @return `true` 表示已达批量上限, 调用方应立即 [`BatchCollector::take`] 刷出。
    pub fn push(&mut self, json: FrameJson) -> bool {
        self.pending.push(json);
        self.pending.len() >= self.max_batch
    }

    /// 取走当前攒下的全部帧 (清空缓冲)。
    ///
    /// @return 攒批缓冲中待刷出的帧。
    pub fn take(&mut self) -> Vec<FrameJson> {
        std::mem::take(&mut self.pending)
    }

    /// 当前攒批内待刷出的帧数。
    ///
    /// @return 缓冲内帧数。
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// 是否无待刷帧。
    ///
    /// @return `true` 表示缓冲为空。
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_monitor_core::classifier::{ParsedMessage, StreamItem};
    use can_types::{BackendKind, CanFrame, CanId, CanMessage};
    use canopen_stack::{CanopenMessage, PdoDirection};
    use j1939_stack::J1939Message;

    /// 构造标准帧。
    fn frame(id: u16, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::new_standard(id).unwrap(), data.to_vec()).unwrap()
    }

    /// 构造扩展帧。
    fn frame_ext(id: u32, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::new_extended(id).unwrap(), data.to_vec()).unwrap()
    }

    /// CANopen TPDO1 (节点 1) 流元素。
    fn item_canopen() -> StreamItem {
        let f = frame(0x181, &[1, 2, 3]);
        StreamItem {
            msg: CanMessage::new(f.clone(), BackendKind::SocketCan, Direction::Rx),
            parsed: ParsedMessage::Canopen {
                frame: f,
                msg: CanopenMessage::Pdo {
                    node: 1,
                    pdo_num: 1,
                    direction: PdoDirection::Tx,
                    data: vec![1, 2, 3],
                },
            },
        }
    }

    /// J1939 直接消息 (PGN 0xFEF1, SA 0) 流元素。
    fn item_j1939() -> StreamItem {
        let f = frame_ext(0x18FEF100, &[0x01, 0x02]);
        StreamItem {
            msg: CanMessage::new(f.clone(), BackendKind::SocketCan, Direction::Rx),
            parsed: ParsedMessage::J1939 {
                frame: f,
                msg: J1939Message::Direct {
                    pgn: 0x00FEF1,
                    source: 0x00,
                    data: vec![0x01, 0x02],
                },
            },
        }
    }

    /// 无法归属的原始帧流元素。
    fn item_raw() -> StreamItem {
        let f = frame(0x101, &[0x0A, 0xFF]);
        StreamItem {
            msg: CanMessage::new(f.clone(), BackendKind::SocketCan, Direction::Rx),
            parsed: ParsedMessage::Raw(f),
        }
    }

    /// CANopen 帧 → JSON 字段契约 (ts 字符串 / id / ext / dir / data / protocol / summary)。
    #[test]
    fn canopen_frame_json_fields() {
        let json = frame_to_json(&item_canopen());
        assert_eq!(json.id, "0x181");
        assert!(!json.ext);
        assert_eq!(json.dir, "rx");
        assert_eq!(json.data, "01 02 03");
        assert_eq!(json.protocol, "canopen");
        assert!(
            json.summary.contains("Pdo"),
            "摘要应含解析结果, 实际: {}",
            json.summary
        );
        // ts 是 u64 毫秒字符串 (防 JS 2^53 溢出)。
        assert!(
            json.ts.parse::<u64>().is_ok(),
            "ts 应为 u64 字符串, 实际: {}",
            json.ts
        );
    }

    /// J1939 帧 → JSON 字段契约 (扩展帧 / j1939 协议 / 摘要)。
    #[test]
    fn j1939_frame_json_fields() {
        let json = frame_to_json(&item_j1939());
        assert_eq!(json.id, "0x18FEF100");
        assert!(json.ext);
        assert_eq!(json.dir, "rx");
        assert_eq!(json.data, "01 02");
        assert_eq!(json.protocol, "j1939");
        assert!(
            json.summary.contains("Direct"),
            "摘要应含解析结果, 实际: {}",
            json.summary
        );
        assert!(json.ts.parse::<u64>().is_ok());
    }

    /// 原始帧 → JSON (protocol=raw, 摘要为空串, 数据仍大写十六进制)。
    #[test]
    fn raw_frame_json_empty_summary() {
        let json = frame_to_json(&item_raw());
        assert_eq!(json.id, "0x101");
        assert_eq!(json.protocol, "raw");
        assert_eq!(json.summary, "");
        assert_eq!(json.data, "0A FF");
        assert!(json.ts.parse::<u64>().is_ok());
    }

    /// 方向字段映射: Tx → "tx"。
    #[test]
    fn tx_direction_maps_to_tx() {
        let mut item = item_raw();
        item.msg.direction = Direction::Tx;
        assert_eq!(frame_to_json(&item).dir, "tx");
    }

    /// 攒批: 达上限才触发刷出, 未达上限留在缓冲。
    #[test]
    fn batch_flushes_at_max() {
        let mut c = BatchCollector::new(3);
        let j = frame_to_json(&item_raw());
        assert!(!c.push(j.clone()), "攒满前不应触发刷出");
        assert!(!c.push(j.clone()));
        assert_eq!(c.len(), 2);
        assert!(c.push(j), "攒满 3 帧应触发刷出");
        let batch = c.take();
        assert_eq!(batch.len(), 3);
        assert!(c.is_empty());
    }

    /// 攒批: 定时器到点刷出取走全部, 之后缓冲为空。
    #[test]
    fn batch_timeout_flush_empties() {
        let mut c = BatchCollector::new(50);
        let j = frame_to_json(&item_j1939());
        c.push(j);
        assert_eq!(c.len(), 1);
        let batch = c.take();
        assert_eq!(batch.len(), 1);
        assert!(c.is_empty());
        // 刷出后再攒不受影响。
        assert!(!c.push(frame_to_json(&item_raw())));
        assert_eq!(c.len(), 1);
    }
}
