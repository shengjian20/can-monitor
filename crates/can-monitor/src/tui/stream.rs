//! # 报文流列表组件
//!
//! 纯渲染组件 [`MessageStream`](crate::tui::stream::MessageStream), 以
//! [`ratatui::widgets::Table`] 展示 CAN 报文列表, 支持:
//!
//! - **列**: 时间戳 / ID / DLC / 数据字节 / 协议摘要 / 收发方向
//! - **滚动**: 尾部自动跟随 (`follow_tail`); 用户上滚暂停, 按 `End` 恢复
//! - **高亮**: 接收 [`can_monitor_core::filter::Highlighter`], 按命中规则为行着色
//!
//! 组件本身不持有消息数据, 渲染时由调用方传入 `&[DisplayMessage]` 切片。

use canopen_stack::CanopenMessage;
use j1939_stack::J1939Message;
use ratatui::layout::Constraint;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};

use can_types::Direction;

use can_monitor_core::classifier::ParsedMessage;
use can_monitor_core::filter::{HighlightStyle, Highlighter};
use crate::tui::app::DisplayMessage;

/// 报文流列表组件。
///
/// 纯渲染组件, 不持有消息数据。滚动状态 (选中行 / 尾部跟随) 内部维护,
/// 渲染时由调用方传入消息切片与高亮引擎。
#[derive(Debug)]
pub struct MessageStream {
    /// ratatui 表格滚动状态。
    state: TableState,
    /// 是否自动跟随最新帧 (默认 `true`)。
    follow_tail: bool,
}

impl Default for MessageStream {
    /// 默认构造: 无选中行, 尾部跟随开启。
    fn default() -> Self {
        Self {
            state: TableState::default(),
            follow_tail: true,
        }
    }
}

impl MessageStream {
    /// 构造报文流组件 (尾部跟随默认开启)。
    ///
    /// @return 组件实例。
    pub fn new() -> Self {
        Self::default()
    }

    /// 查询尾部跟随状态。
    ///
    /// @return `true` 表示新帧会自动滚到底部。
    pub fn is_follow_tail(&self) -> bool {
        self.follow_tail
    }

    /// 设置尾部跟随。
    ///
    /// @param follow `true` 启用, `false` 暂停。
    pub fn set_follow_tail(&mut self, follow: bool) {
        self.follow_tail = follow;
    }

    /// 恢复尾部跟随 (等价于 `set_follow_tail(true)`)。
    pub fn follow(&mut self) {
        self.follow_tail = true;
    }

    /// 选中下一行 (滚动条下移)。
    ///
    /// @param len 消息窗口总行数。
    pub fn next_row(&mut self, len: usize) {
        if len == 0 {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => {
                if i >= len - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
        self.follow_tail = false;
    }

    /// 选中上一行 (滚动条上移)。
    ///
    /// @param len 消息窗口总行数。
    pub fn previous_row(&mut self, len: usize) {
        if len == 0 {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    len - 1
                } else {
                    i - 1
                }
            }
            None => len.saturating_sub(1),
        };
        self.state.select(Some(i));
        self.follow_tail = false;
    }

    /// 跳转到最后一行并恢复尾部跟随。
    ///
    /// @param len 消息窗口总行数。
    pub fn end(&mut self, len: usize) {
        if len == 0 {
            self.state.select(None);
        } else {
            self.state.select(Some(len - 1));
        }
        self.follow_tail = true;
    }

    /// 翻页向下 (PageDown)。
    ///
    /// @param len    消息窗口总行数。
    /// @param page   单页可见行数。
    pub fn page_down(&mut self, len: usize, page: usize) {
        if len == 0 {
            return;
        }
        let page = page.max(1);
        let i = match self.state.selected() {
            Some(i) => (i + page).min(len - 1),
            None => (page - 1).min(len - 1),
        };
        self.state.select(Some(i));
        self.follow_tail = false;
    }

    /// 翻页向上 (PageUp)。
    ///
    /// @param len    消息窗口总行数。
    /// @param page   单页可见行数。
    pub fn page_up(&mut self, len: usize, page: usize) {
        if len == 0 {
            return;
        }
        let page = page.max(1);
        let i = match self.state.selected() {
            Some(i) => i.saturating_sub(page),
            None => 0,
        };
        self.state.select(Some(i));
        self.follow_tail = false;
    }

    /// 渲染报文流表格。
    ///
    /// 最新消息在最上方 (列表倒序)。若 `follow_tail` 为 `true`, 自动选中
    /// 最新行; 否则保持用户当前选中。
    ///
    /// @param frame     ratatui 帧。
    /// @param area      渲染区域。
    /// @param messages  消息窗口 (正序: 最旧在前, 最新在后)。
    /// @param highlight 高亮引擎。
    pub fn render(
        &mut self,
        frame: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        messages: &[DisplayMessage],
        highlight: &Highlighter,
    ) {
        let block = Block::default()
            .title("CAN 消息")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        if messages.is_empty() {
            let placeholder = ratatui::widgets::Paragraph::new("等待消息...")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(placeholder, area);
            return;
        }

        let len = messages.len();

        // 尾部跟随: 新帧到达时自动选中最新行 (最后一行, 即列表顶部)。
        if self.follow_tail {
            self.state.select(Some(len - 1));
        }

        // 构建表格行 (倒序: 最新在上)。
        let rows: Vec<Row> = messages
            .iter()
            .rev()
            .map(|msg| {
                let style = row_style(msg, highlight);
                let cells = format_row_cells(msg);
                let row = Row::new(cells);
                row.style(style)
            })
            .collect();

        let header = Row::new(["时间戳", "ID", "DLC", "数据", "协议", "方向"])
            .style(
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
            .height(1);

        let widths = [
            Constraint::Length(14), // 时间戳 sec.usec
            Constraint::Length(10), // ID (标准 3 位 / 扩展 8 位 + 前缀)
            Constraint::Length(4),  // DLC
            Constraint::Min(20),    // 数据 hex
            Constraint::Length(16), // 协议摘要
            Constraint::Length(4),  // 方向
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .block(block)
            .row_highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(table, area, &mut self.state);
    }
}

/// 将一条消息格式化为表格各列 Cell。
///
/// 列顺序: 时间戳 / ID / DLC / 数据 hex / 协议摘要 / 方向。
///
/// @param msg 显示消息。
/// @return 6 个 Cell。
fn format_row_cells(msg: &DisplayMessage) -> Vec<Cell<'static>> {
    vec![
        Cell::from(format_timestamp(msg)),
        Cell::from(format_id(msg)),
        Cell::from(format_dlc(msg)),
        Cell::from(format_data(msg)),
        Cell::from(format_protocol(msg)),
        Cell::from(format_direction(msg)),
    ]
}

/// 格式化时间戳列。
///
/// 帧自带时间戳时显示 `sec.usec` (相对时间), 否则显示 `—`。
///
/// @param msg 显示消息。
/// @return 时间戳文本。
fn format_timestamp(msg: &DisplayMessage) -> String {
    match msg.raw.frame.timestamp() {
        Some(ts) => {
            // 使用 SystemTime 距 UNIX_EPOCH 的绝对秒, 截取低 6 位 + 微秒。
            match ts.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => {
                    let sec = d.as_secs() % 1_000_000;
                    let usec = d.subsec_micros();
                    format!("{sec:06}.{usec:06}")
                }
                Err(_) => "—".to_string(),
            }
        }
        None => "—".to_string(),
    }
}

/// 格式化 ID 列。
///
/// 标准帧 3 位大写 hex, 扩展帧 8 位大写 hex。
///
/// @param msg 显示消息。
/// @return ID 文本。
fn format_id(msg: &DisplayMessage) -> String {
    let id = msg.raw.frame.id();
    if id.is_extended() {
        format!("{:08X}", id.raw_id())
    } else {
        format!("{:03X}", id.raw_id())
    }
}

/// 格式化 DLC 列 (数据长度)。
///
/// @param msg 显示消息。
/// @return DLC 文本。
fn format_dlc(msg: &DisplayMessage) -> String {
    format!("{}", msg.raw.frame.data().len())
}

/// 格式化数据列 (空格分隔的大写 hex 字节)。
///
/// @param msg 显示消息。
/// @return hex 数据文本。
fn format_data(msg: &DisplayMessage) -> String {
    msg.raw
        .frame
        .data()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 格式化协议摘要列。
///
/// CANopen: "NMT" / "Heartbeat n" / "SDO n" / "TPDOx n" / "RPDOx n" / "EMCY n" / "SYNC" / "TIME"
/// J1939:   "PGN PPPP" / "TP PGN PPPP" / "DM PGN"
/// Raw:     "—"
///
/// @param msg 显示消息。
/// @return 协议摘要文本。
fn format_protocol(msg: &DisplayMessage) -> String {
    match &msg.parsed {
        Some(ParsedMessage::Canopen { msg, .. }) => match msg {
            CanopenMessage::Nmt { cmd: _, node } => {
                format!("NMT n{node}")
            }
            CanopenMessage::Heartbeat { node, state: _ } => {
                format!("Heartbeat n{node}")
            }
            CanopenMessage::SdoRequest { node, .. } => {
                format!("SDO n{node}")
            }
            CanopenMessage::SdoResponse { node, .. } => {
                format!("SDO n{node}")
            }
            CanopenMessage::Pdo {
                node,
                pdo_num,
                direction,
                ..
            } => {
                let dir_str = match direction {
                    canopen_stack::PdoDirection::Tx => "T",
                    canopen_stack::PdoDirection::Rx => "R",
                };
                format!("{dir_str}PDO{pdo_num} n{node}")
            }
            CanopenMessage::Emcy { node, .. } => {
                format!("EMCY n{node}")
            }
            CanopenMessage::Sync => "SYNC".to_string(),
            CanopenMessage::Time => "TIME".to_string(),
            CanopenMessage::Unknown => "—".to_string(),
        },
        Some(ParsedMessage::J1939 { msg, .. }) => match msg {
            J1939Message::Direct { pgn, .. } => {
                format!("PGN {pgn:04X}")
            }
            J1939Message::Transport {
                pgn, reassembled, ..
            } => {
                if reassembled.is_some() {
                    format!("TP {pgn:04X}")
                } else {
                    format!("TP… {pgn:04X}")
                }
            }
            J1939Message::Diagnostic { pgn, .. } => {
                format!("DM {pgn:04X}")
            }
        },
        Some(ParsedMessage::Raw(_)) | None => "—".to_string(),
    }
}

/// 格式化方向列。
///
/// @param msg 显示消息。
/// @return "RX" 或 "TX"。
fn format_direction(msg: &DisplayMessage) -> &'static str {
    match msg.raw.direction {
        Direction::Rx => "RX",
        Direction::Tx => "TX",
    }
}

/// 计算一行消息的高亮样式。
///
/// 优先使用 [`Highlighter`] 规则命中结果; 未命中使用方向默认色 (RX=绿, TX=蓝)。
///
/// @param msg       显示消息。
/// @param highlight 高亮引擎。
/// @return ratatui 样式。
fn row_style(msg: &DisplayMessage, highlight: &Highlighter) -> Style {
    // 尝试高亮规则命中。
    if let Some(ref parsed) = msg.parsed {
        let hl = highlight.highlight_for(parsed);
        let color = highlight_color(hl);
        if color != Color::Reset {
            return Style::default().fg(color);
        }
    }
    // 未命中时按方向着色。
    match msg.raw.direction {
        Direction::Rx => Style::default().fg(Color::Green),
        Direction::Tx => Style::default().fg(Color::Blue),
    }
}

/// 将 [`HighlightStyle`] 映射为 `ratatui::Color`。
///
/// @param style 高亮样式。
/// @return 对应颜色; `Default` 返回 `Color::Reset` (使用默认)。
pub fn highlight_color(style: HighlightStyle) -> Color {
    match style {
        HighlightStyle::Default => Color::Reset,
        HighlightStyle::Yellow => Color::Yellow,
        HighlightStyle::Cyan => Color::Cyan,
        HighlightStyle::Green => Color::Green,
        HighlightStyle::Red => Color::Red,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_monitor_core::filter::{HighlightRule, Highlighter};
    use can_types::{BackendKind, CanFrame, CanId, CanMessage};

    /// 构造标准帧 DisplayMessage (无时间戳, 无 parsed)。
    fn dm_raw(id: u16, data: &[u8], dir: Direction) -> DisplayMessage {
        let frame = CanFrame::new(CanId::new_standard(id).unwrap(), data.to_vec()).unwrap();
        let msg = CanMessage::new(frame, BackendKind::None, dir);
        DisplayMessage {
            raw: msg,
            parsed: None,
        }
    }

    /// ID 格式化: 标准帧 3 位大写 hex。
    #[test]
    fn format_id_standard_3hex() {
        let dm = dm_raw(0x181, &[], Direction::Rx);
        assert_eq!(format_id(&dm), "181");
    }

    /// ID 格式化: 扩展帧 8 位大写 hex。
    #[test]
    fn format_id_extended_8hex() {
        let frame = CanFrame::new(CanId::new_extended(0x18FEF100).unwrap(), vec![1]).unwrap();
        let msg = CanMessage::new(frame, BackendKind::None, Direction::Rx);
        let dm = DisplayMessage {
            raw: msg,
            parsed: None,
        };
        assert_eq!(format_id(&dm), "18FEF100");
    }

    /// DLC 格式化。
    #[test]
    fn format_dlc_value() {
        let dm = dm_raw(0x100, &[1, 2, 3], Direction::Rx);
        assert_eq!(format_dlc(&dm), "3");
    }

    /// 数据格式化: 空格分隔大写 hex。
    #[test]
    fn format_data_hex() {
        let dm = dm_raw(0x100, &[0xAB, 0xCD, 0x01], Direction::Rx);
        assert_eq!(format_data(&dm), "AB CD 01");
    }

    /// 方向格式化。
    #[test]
    fn format_direction_rx_tx() {
        let rx = dm_raw(0x100, &[], Direction::Rx);
        let tx = dm_raw(0x100, &[], Direction::Tx);
        assert_eq!(format_direction(&rx), "RX");
        assert_eq!(format_direction(&tx), "TX");
    }

    /// HighlightStyle → Color 映射。
    #[test]
    fn highlight_color_mapping() {
        assert_eq!(highlight_color(HighlightStyle::Default), Color::Reset);
        assert_eq!(highlight_color(HighlightStyle::Yellow), Color::Yellow);
        assert_eq!(highlight_color(HighlightStyle::Cyan), Color::Cyan);
        assert_eq!(highlight_color(HighlightStyle::Green), Color::Green);
        assert_eq!(highlight_color(HighlightStyle::Red), Color::Red);
    }

    /// 初始状态: follow_tail = true, 无选中。
    #[test]
    fn initial_state_follow_tail() {
        let ms = MessageStream::new();
        assert!(ms.is_follow_tail());
        assert!(ms.state.selected().is_none());
    }

    /// next_row 后 follow_tail = false。
    #[test]
    fn next_row_disables_follow() {
        let mut ms = MessageStream::new();
        ms.next_row(10);
        assert!(!ms.is_follow_tail());
        assert_eq!(ms.state.selected(), Some(0));
    }

    /// previous_row 在 0 行时环绕到末尾。
    #[test]
    fn previous_row_wraps() {
        let mut ms = MessageStream::new();
        ms.next_row(5); // selected=0
        ms.previous_row(5); // selected=4
        assert_eq!(ms.state.selected(), Some(4));
    }

    /// end() 恢复 follow_tail = true 并选中末行。
    #[test]
    fn end_restores_follow() {
        let mut ms = MessageStream::new();
        ms.next_row(10);
        assert!(!ms.is_follow_tail());
        ms.end(10);
        assert!(ms.is_follow_tail());
        assert_eq!(ms.state.selected(), Some(9));
    }

    /// end() 空列表清除选中。
    #[test]
    fn end_empty_clears() {
        let mut ms = MessageStream::new();
        ms.end(0);
        assert!(ms.state.selected().is_none());
    }

    /// page_down / page_up 正常翻页。
    #[test]
    fn page_navigation() {
        let mut ms = MessageStream::new();
        ms.page_down(20, 5);
        assert_eq!(ms.state.selected(), Some(4));
        ms.page_down(20, 5);
        assert_eq!(ms.state.selected(), Some(9));
        ms.page_up(20, 5);
        assert_eq!(ms.state.selected(), Some(4));
    }

    /// page_down 不超界。
    #[test]
    fn page_down_clamp() {
        let mut ms = MessageStream::new();
        ms.page_down(3, 10);
        assert_eq!(ms.state.selected(), Some(2));
    }

    /// set_follow_tail 手动设置。
    #[test]
    fn set_follow_tail_manual() {
        let mut ms = MessageStream::new();
        assert!(ms.is_follow_tail());
        ms.set_follow_tail(false);
        assert!(!ms.is_follow_tail());
        ms.set_follow_tail(true);
        assert!(ms.is_follow_tail());
    }

    /// 协议摘要: CANopen TPDO。
    #[test]
    fn protocol_summary_canopen_tpdo() {
        use canopen_stack::PdoDirection;
        let frame = CanFrame::new(CanId::new_standard(0x181).unwrap(), vec![1]).unwrap();
        let msg = CanMessage::new(frame, BackendKind::None, Direction::Rx);
        let dm = DisplayMessage {
            raw: msg,
            parsed: Some(ParsedMessage::Canopen {
                frame: CanFrame::new(CanId::new_standard(0x181).unwrap(), vec![1]).unwrap(),
                msg: CanopenMessage::Pdo {
                    node: 1,
                    pdo_num: 1,
                    direction: PdoDirection::Tx,
                    data: vec![1],
                },
            }),
        };
        assert_eq!(format_protocol(&dm), "TPDO1 n1");
    }

    /// 协议摘要: J1939 Direct。
    #[test]
    fn protocol_summary_j1939_direct() {
        let frame = CanFrame::new(CanId::new_extended(0x18FEF100).unwrap(), vec![1]).unwrap();
        let msg = CanMessage::new(frame, BackendKind::None, Direction::Rx);
        let dm = DisplayMessage {
            raw: msg,
            parsed: Some(ParsedMessage::J1939 {
                frame: CanFrame::new(CanId::new_extended(0x18FEF100).unwrap(), vec![1]).unwrap(),
                msg: J1939Message::Direct {
                    pgn: 0xFEF1,
                    source: 0x00,
                    data: vec![1],
                },
            }),
        };
        assert_eq!(format_protocol(&dm), "PGN FEF1");
    }

    /// 协议摘要: Raw → "—"。
    #[test]
    fn protocol_summary_raw_dash() {
        let dm = dm_raw(0x101, &[1], Direction::Rx);
        assert_eq!(format_protocol(&dm), "—");
    }

    /// 时间戳无帧时间戳时返回 "—"。
    #[test]
    fn timestamp_none_dash() {
        let dm = dm_raw(0x100, &[], Direction::Rx);
        assert_eq!(format_timestamp(&dm), "—");
    }

    /// row_style: 高亮规则命中时使用规则色。
    #[test]
    fn row_style_highlight_hit() {
        let mut h = Highlighter::new();
        h.add(HighlightRule::new(HighlightStyle::Yellow).with_id(0x181));

        let frame = CanFrame::new(CanId::new_standard(0x181).unwrap(), vec![1]).unwrap();
        let msg = CanMessage::new(frame, BackendKind::None, Direction::Rx);
        let dm = DisplayMessage {
            raw: msg,
            parsed: Some(ParsedMessage::Raw(
                CanFrame::new(CanId::new_standard(0x181).unwrap(), vec![1]).unwrap(),
            )),
        };
        let style = row_style(&dm, &h);
        assert_eq!(style.fg, Some(Color::Yellow));
    }

    /// row_style: 高亮未命中时按方向着色 (RX=绿)。
    #[test]
    fn row_style_direction_fallback() {
        let h = Highlighter::new();
        let dm = dm_raw(0x100, &[], Direction::Rx);
        let style = row_style(&dm, &h);
        assert_eq!(style.fg, Some(Color::Green));
    }

    /// row_style: 高亮未命中时按方向着色 (TX=蓝)。
    #[test]
    fn row_style_direction_tx_blue() {
        let h = Highlighter::new();
        let dm = dm_raw(0x100, &[], Direction::Tx);
        let style = row_style(&dm, &h);
        assert_eq!(style.fg, Some(Color::Blue));
    }

    /// 协议摘要: CANopen SDO。
    #[test]
    fn protocol_summary_canopen_sdo() {
        let frame = CanFrame::new(CanId::new_standard(0x601).unwrap(), vec![0x40]).unwrap();
        let msg = CanMessage::new(frame, BackendKind::None, Direction::Tx);
        let dm = DisplayMessage {
            raw: msg,
            parsed: Some(ParsedMessage::Canopen {
                frame: CanFrame::new(CanId::new_standard(0x601).unwrap(), vec![0x40]).unwrap(),
                msg: CanopenMessage::SdoRequest {
                    node: 1,
                    index: 0x1000,
                    subindex: 0,
                    data: vec![0x40, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00],
                },
            }),
        };
        assert_eq!(format_protocol(&dm), "SDO n1");
    }

    /// 协议摘要: CANopen NMT。
    #[test]
    fn protocol_summary_canopen_nmt() {
        use canopen_stack::NmtCommand;
        let frame = CanFrame::new(CanId::new_standard(0x000).unwrap(), vec![0x01, 0x05]).unwrap();
        let msg = CanMessage::new(frame, BackendKind::None, Direction::Tx);
        let dm = DisplayMessage {
            raw: msg,
            parsed: Some(ParsedMessage::Canopen {
                frame: CanFrame::new(CanId::new_standard(0x000).unwrap(), vec![0x01, 0x05])
                    .unwrap(),
                msg: CanopenMessage::Nmt {
                    cmd: NmtCommand::StartRemoteNode,
                    node: 5,
                },
            }),
        };
        assert_eq!(format_protocol(&dm), "NMT n5");
    }

    /// 协议摘要: J1939 Transport (未完成)。
    #[test]
    fn protocol_summary_j1939_transport_incomplete() {
        let frame = CanFrame::new(CanId::new_extended(0x18FF0100).unwrap(), vec![1]).unwrap();
        let msg = CanMessage::new(frame, BackendKind::None, Direction::Rx);
        let dm = DisplayMessage {
            raw: msg,
            parsed: Some(ParsedMessage::J1939 {
                frame: CanFrame::new(CanId::new_extended(0x18FF0100).unwrap(), vec![1]).unwrap(),
                msg: J1939Message::Transport {
                    pgn: 0xFF01,
                    source: 0x00,
                    total_len: 100,
                    reassembled: None,
                },
            }),
        };
        assert_eq!(format_protocol(&dm), "TP… FF01");
    }

    /// 协议摘要: J1939 Transport (已完成)。
    #[test]
    fn protocol_summary_j1939_transport_complete() {
        let frame = CanFrame::new(CanId::new_extended(0x18FF0100).unwrap(), vec![1]).unwrap();
        let msg = CanMessage::new(frame, BackendKind::None, Direction::Rx);
        let dm = DisplayMessage {
            raw: msg,
            parsed: Some(ParsedMessage::J1939 {
                frame: CanFrame::new(CanId::new_extended(0x18FF0100).unwrap(), vec![1]).unwrap(),
                msg: J1939Message::Transport {
                    pgn: 0xFF01,
                    source: 0x00,
                    total_len: 100,
                    reassembled: Some(vec![0u8; 100]),
                },
            }),
        };
        assert_eq!(format_protocol(&dm), "TP FF01");
    }
}
