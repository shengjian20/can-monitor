//! # 帧过滤引擎
//!
//! 在帧分类**之后**对 [`ParsedMessage`] 进行筛选与高亮, 提供:
//!
//! - [`FrameFilter`]: 按 ID 范围 / 协议类型 / 收发方向组合过滤;
//! - [`HighlightRule`] 与 [`Highlighter`]: 按 ID 或协议命中高亮规则, 返回
//!   UI 无关的 [`HighlightStyle`] 枚举, 由 TUI 层 (任务 16) 映射到 ratatui
//!   颜色。
//!
//! ## 设计要点
//!
//! - **过滤在分类后**: 协议条件依赖 [`ParsedMessage::protocol`], 因此推荐
//!   用 [`FrameFilter::matches_parsed`]; 仅持有原始帧/消息时可用
//!   [`FrameFilter::matches_frame`] 与 [`FrameFilter::matches`]。
//! - **UI 依赖隔离**: 本模块不引用 ratatui, 颜色以 [`HighlightStyle`] 枚举
//!   表达, 避免过滤引擎与终端渲染耦合。
//! - **简单条件组合**: 各条件之间为 AND 关系, 不做复杂 DSL。

use can_types::{CanFrame, CanMessage, Direction};

use crate::classifier::{ParsedMessage, Protocol};

/// 高亮样式 (UI 无关的简化表示)。
///
/// 不直接依赖 ratatui, 由 TUI 层 (任务 16) 将每个变体映射为具体颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightStyle {
    /// 无高亮 (默认样式)。
    Default,
    /// 黄色。
    Yellow,
    /// 青色。
    Cyan,
    /// 绿色。
    Green,
    /// 红色。
    Red,
}

/// 帧过滤条件集合。
///
/// 各条件之间为 **AND** 关系: 只有同时满足所有已设置条件 (且 [`enabled`] 为
/// `true`) 的帧才通过过滤。未设置的条件 (字段为 `None`) 视为不限制。
///
/// [`enabled`]: FrameFilter::enabled
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameFilter {
    /// ID 起始-结束范围 (含边界), `None` = 不过滤。
    id_range: Option<(u32, u32)>,
    /// 协议类型, `None` = 全部。
    protocol: Option<Protocol>,
    /// 收发方向, `None` = 全部。
    direction: Option<Direction>,
    /// 总开关, `false` 时全部通过。
    enabled: bool,
    /// 高亮引擎 (管理 ID / 协议高亮规则)。
    highlighter: Highlighter,
}

impl Default for FrameFilter {
    /// 全不过滤的默认过滤器, 等价于 [`FrameFilter::new`]。
    fn default() -> Self {
        Self::new()
    }
}

impl FrameFilter {
    /// 构造一个全不过滤的过滤器 (`enabled = false`, 所有条件未设置)。
    ///
    /// @return 过滤器实例。
    pub fn new() -> Self {
        Self {
            id_range: None,
            protocol: None,
            direction: None,
            enabled: false,
            highlighter: Highlighter::new(),
        }
    }

    /// 设置 ID 过滤范围 (含边界)。
    ///
    /// 若 `start > end` 则自动交换二者, 保证范围始终有效。
    ///
    /// @param start 起始 ID (含)。
    /// @param end   结束 ID (含)。
    /// @return `&mut self` 以便链式调用。
    pub fn set_id_range(&mut self, start: u32, end: u32) -> &mut Self {
        self.id_range = Some(if start <= end {
            (start, end)
        } else {
            (end, start)
        });
        self
    }

    /// 清除 ID 范围过滤条件。
    ///
    /// @return `&mut self` 以便链式调用。
    pub fn clear_id_range(&mut self) -> &mut Self {
        self.id_range = None;
        self
    }

    /// 设置协议类型过滤条件。
    ///
    /// @param protocol 期望匹配的 [`Protocol`]。
    /// @return `&mut self` 以便链式调用。
    pub fn set_protocol(&mut self, protocol: Protocol) -> &mut Self {
        self.protocol = Some(protocol);
        self
    }

    /// 清除协议类型过滤条件。
    ///
    /// @return `&mut self` 以便链式调用。
    pub fn clear_protocol(&mut self) -> &mut Self {
        self.protocol = None;
        self
    }

    /// 设置收发方向过滤条件。
    ///
    /// @param direction 期望匹配的 [`Direction`]。
    /// @return `&mut self` 以便链式调用。
    pub fn set_direction(&mut self, direction: Direction) -> &mut Self {
        self.direction = Some(direction);
        self
    }

    /// 清除收发方向过滤条件。
    ///
    /// @return `&mut self` 以便链式调用。
    pub fn clear_direction(&mut self) -> &mut Self {
        self.direction = None;
        self
    }

    /// 设置总开关。
    ///
    /// @param enabled `true` 时启用过滤, `false` 时全部通过。
    /// @return `&mut self` 以便链式调用。
    pub fn set_enabled(&mut self, enabled: bool) -> &mut Self {
        self.enabled = enabled;
        self
    }

    /// 获取当前 ID 过滤范围。
    ///
    /// @return `Some((start, end))` 或 `None` (不过滤)。
    pub fn id_range(&self) -> Option<(u32, u32)> {
        self.id_range
    }

    /// 获取当前协议过滤条件。
    ///
    /// @return 已设置的 [`Protocol`] 或 `None` (不过滤)。
    pub fn protocol(&self) -> Option<Protocol> {
        self.protocol
    }

    /// 获取当前方向过滤条件。
    ///
    /// @return 已设置的 [`Direction`] 或 `None` (不过滤)。
    pub fn direction(&self) -> Option<Direction> {
        self.direction
    }

    /// 查询过滤总开关状态。
    ///
    /// @return `true` 表示过滤启用。
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 获取高亮引擎引用。
    ///
    /// @return 高亮引擎。
    pub fn highlighter(&self) -> &Highlighter {
        &self.highlighter
    }

    /// 获取高亮引擎可变引用。
    ///
    /// @return 可变高亮引擎。
    pub fn highlighter_mut(&mut self) -> &mut Highlighter {
        &mut self.highlighter
    }

    /// 判断一条统一消息是否通过过滤。
    ///
    /// 检查 `enabled`、ID 范围 (来自 [`CanMessage::frame`]) 与收发方向
    /// ([`CanMessage::direction`])。**不检查协议** — 原始帧不携带协议类别,
    /// 协议过滤请使用 [`FrameFilter::matches_parsed`]。
    ///
    /// @param msg 待判断的消息。
    /// @return `true` 表示通过 (或过滤未启用)。
    pub fn matches(&self, msg: &CanMessage) -> bool {
        if !self.enabled {
            return true;
        }
        self.id_range_matches(msg.frame.id().raw_id()) && self.direction_matches(msg.direction)
    }

    /// 判断一条已分类消息是否通过过滤 (推荐入口)。
    ///
    /// 检查 `enabled`、ID 范围与协议类型。过滤发生在分类之后, 直接使用
    /// [`ParsedMessage::protocol`] 判断协议, 无需重复分类。方向信息不在
    /// [`ParsedMessage`] 中, 如需同时过滤方向请配合 [`FrameFilter::matches`]
    /// 使用。
    ///
    /// @param parsed 待判断的分类结果。
    /// @return `true` 表示通过 (或过滤未启用)。
    pub fn matches_parsed(&self, parsed: &ParsedMessage) -> bool {
        if !self.enabled {
            return true;
        }
        let raw_id = parsed_frame(parsed).id().raw_id();
        self.id_range_matches(raw_id) && self.protocol_matches(parsed.protocol())
    }

    /// 判断一帧原始帧是否通过过滤。
    ///
    /// 仅检查 `enabled` 与 ID 范围 (帧本身不携带方向/协议信息)。
    ///
    /// @param frame 待判断的帧。
    /// @return `true` 表示通过 (或过滤未启用)。
    pub fn matches_frame(&self, frame: &CanFrame) -> bool {
        if !self.enabled {
            return true;
        }
        self.id_range_matches(frame.id().raw_id())
    }

    /// 判断原始 ID 是否落在已设置的范围内 (含边界)。
    fn id_range_matches(&self, raw_id: u32) -> bool {
        match self.id_range {
            Some((start, end)) => raw_id >= start && raw_id <= end,
            None => true,
        }
    }

    /// 判断协议是否匹配已设置的过滤条件。
    fn protocol_matches(&self, protocol: Protocol) -> bool {
        match self.protocol {
            Some(expected) => expected == protocol,
            None => true,
        }
    }

    /// 判断方向是否匹配已设置的过滤条件。
    fn direction_matches(&self, direction: Direction) -> bool {
        match self.direction {
            Some(expected) => expected == direction,
            None => true,
        }
    }
}

/// 单条高亮规则。
///
/// 通过 ID 或协议 (或两者同时) 命中一条已分类消息, 命中则返回规则指定的
/// [`HighlightStyle`]。字段公开, 便于 TUI 层直接构建规则列表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightRule {
    /// 命中时匹配的帧 ID (原始 ID 数值); `None` 表示不按 ID 匹配。
    pub id_match: Option<u32>,
    /// 命中时匹配的协议类型; `None` 表示不按协议匹配。
    pub protocol_match: Option<Protocol>,
    /// 命中时使用的高亮样式。
    pub style: HighlightStyle,
}

impl HighlightRule {
    /// 构造一条仅指定样式、暂不设匹配条件的规则。
    ///
    /// 配合 [`HighlightRule::with_id`] / [`HighlightRule::with_protocol`]
    /// 使用链式构建。
    ///
    /// @param style 命中时的高亮样式。
    /// @return 规则实例。
    pub fn new(style: HighlightStyle) -> Self {
        Self {
            id_match: None,
            protocol_match: None,
            style,
        }
    }

    /// 设置按 ID 匹配并返回自身。
    ///
    /// @param id 命中的帧 ID。
    /// @return 构建中的规则。
    pub fn with_id(mut self, id: u32) -> Self {
        self.id_match = Some(id);
        self
    }

    /// 设置按协议匹配并返回自身。
    ///
    /// @param protocol 命中的协议类型。
    /// @return 构建中的规则。
    pub fn with_protocol(mut self, protocol: Protocol) -> Self {
        self.protocol_match = Some(protocol);
        self
    }

    /// 判断一条已分类消息是否命中本规则。
    ///
    /// 设置了多个条件时须**同时**满足 (AND)。未设置任何条件时永不命中。
    ///
    /// @param parsed 待判断的分类结果。
    /// @return `true` 表示命中。
    pub fn matches(&self, parsed: &ParsedMessage) -> bool {
        if let Some(id) = self.id_match {
            if parsed_frame(parsed).id().raw_id() != id {
                return false;
            }
        }
        if let Some(protocol) = self.protocol_match {
            if parsed.protocol() != protocol {
                return false;
            }
        }
        self.id_match.is_some() || self.protocol_match.is_some()
    }

    /// 判断一条已分类消息的高亮样式。
    ///
    /// 命中则返回 [`self.style`](HighlightRule::style), 未命中返回
    /// [`HighlightStyle::Default`]。
    ///
    /// @param parsed 待判断的分类结果。
    /// @return 命中规则时的高亮样式, 否则 `Default`。
    pub fn highlight_for(&self, parsed: &ParsedMessage) -> HighlightStyle {
        if self.matches(parsed) {
            self.style
        } else {
            HighlightStyle::Default
        }
    }
}

/// 多规则高亮引擎。
///
/// 按添加顺序逐条评估规则, **先命中者优先** (返回其样式); 全部未命中返回
/// [`HighlightStyle::Default`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlighter {
    /// 按优先级排列的规则列表。
    rules: Vec<HighlightRule>,
}

impl Default for Highlighter {
    /// 构造一个不含任何规则的高亮引擎。
    fn default() -> Self {
        Self { rules: Vec::new() }
    }
}

impl Highlighter {
    /// 构造一个不含任何规则的高亮引擎。
    ///
    /// @return 高亮引擎实例。
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一条高亮规则 (排在已有规则之后, 优先级较低)。
    ///
    /// @param rule 待追加的规则。
    /// @return `&mut self` 以便链式调用。
    pub fn add(&mut self, rule: HighlightRule) -> &mut Self {
        self.rules.push(rule);
        self
    }

    /// 清空所有高亮规则。
    ///
    /// @return `&mut self` 以便链式调用。
    pub fn clear(&mut self) -> &mut Self {
        self.rules.clear();
        self
    }

    /// 获取按优先级排列的规则列表。
    ///
    /// @return 规则切片。
    pub fn rules(&self) -> &[HighlightRule] {
        &self.rules
    }

    /// 计算一条已分类消息的高亮样式。
    ///
    /// 首个命中规则的样式即结果; 无命中返回 [`HighlightStyle::Default`]。
    ///
    /// @param parsed 待判断的分类结果。
    /// @return 高亮样式。
    pub fn highlight_for(&self, parsed: &ParsedMessage) -> HighlightStyle {
        for rule in &self.rules {
            if rule.matches(parsed) {
                return rule.style;
            }
        }
        HighlightStyle::Default
    }
}

/// 从分类结果中取出原始帧引用。
///
/// @param parsed 分类结果。
/// @return 原始帧引用。
fn parsed_frame(parsed: &ParsedMessage) -> &CanFrame {
    match parsed {
        ParsedMessage::Raw(frame) => frame,
        ParsedMessage::Canopen { frame, .. } => frame,
        ParsedMessage::J1939 { frame, .. } => frame,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_types::{BackendKind, CanId};
    use canopen_stack::CanopenMessage;
    use j1939_stack::J1939Message;

    /// 构造标准帧。
    fn frame(id: u16, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::new_standard(id).unwrap(), data.to_vec()).unwrap()
    }

    /// 构造扩展帧。
    fn frame_ext(id: u32, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::new_extended(id).unwrap(), data.to_vec()).unwrap()
    }

    /// 按协议构造分类结果 (原始帧复用参数)。
    fn parsed(protocol: Protocol, frame: CanFrame) -> ParsedMessage {
        match protocol {
            Protocol::Raw => ParsedMessage::Raw(frame),
            Protocol::Canopen => ParsedMessage::Canopen {
                frame,
                msg: CanopenMessage::Unknown,
            },
            Protocol::J1939 => ParsedMessage::J1939 {
                frame,
                msg: J1939Message::Direct {
                    pgn: 0,
                    source: 0,
                    data: vec![0xAA],
                },
            },
        }
    }

    /// 构造标准 ID 的 Canopen 分类结果。
    fn parsed_canopen(id: u16) -> ParsedMessage {
        parsed(Protocol::Canopen, frame(id, &[1, 2]))
    }

    /// 构造统一消息。
    fn msg(id: u16, direction: Direction) -> CanMessage {
        CanMessage::new(frame(id, &[1]), BackendKind::None, direction)
    }

    /// 启用并只设置 ID 范围 [0x100, 0x1FF] 的过滤器。
    fn filter_id_range() -> FrameFilter {
        let mut f = FrameFilter::new();
        f.set_enabled(true).set_id_range(0x100, 0x1FF);
        f
    }

    /// ID 范围含边界: 0x100 与 0x1FF 均匹配。
    #[test]
    fn id_range_matches_inclusive_boundaries() {
        let f = filter_id_range();
        assert!(f.matches_frame(&frame(0x100, &[])));
        assert!(f.matches_frame(&frame(0x1FF, &[])));
        assert!(f.matches_frame(&frame(0x180, &[])));
    }

    /// ID 范围外不匹配: 0x080 低于起点。
    #[test]
    fn id_range_rejects_outside() {
        let f = filter_id_range();
        assert!(!f.matches_frame(&frame(0x080, &[])));
        assert!(!f.matches_frame(&frame(0x200, &[])));
    }

    /// set_id_range 对 start > end 自动交换。
    #[test]
    fn id_range_swaps_reversed_bounds() {
        let mut f = FrameFilter::new();
        f.set_enabled(true).set_id_range(0x1FF, 0x100);
        assert_eq!(f.id_range(), Some((0x100, 0x1FF)));
        assert!(f.matches_frame(&frame(0x180, &[])));
    }

    /// 协议过滤: Canopen 只匹配 Canopen 消息。
    #[test]
    fn protocol_filter_selects_canopen_only() {
        let mut f = FrameFilter::new();
        f.set_enabled(true).set_protocol(Protocol::Canopen);
        assert!(f.matches_parsed(&parsed_canopen(0x181)));
        assert!(!f.matches_parsed(&parsed(Protocol::Raw, frame(0x181, &[1]))));
        assert!(!f.matches_parsed(&parsed(
            Protocol::J1939,
            frame_ext(0x18FEF100, &[1])
        )));
    }

    /// 方向过滤: Rx 只匹配 Rx 消息。
    #[test]
    fn direction_filter_selects_rx_only() {
        let mut f = FrameFilter::new();
        f.set_enabled(true).set_direction(Direction::Rx);
        assert!(f.matches(&msg(0x181, Direction::Rx)));
        assert!(!f.matches(&msg(0x181, Direction::Tx)));
    }

    /// 组合条件: ID 范围 + 协议同时满足才通过 (AND)。
    #[test]
    fn combined_id_range_and_protocol() {
        let mut f = FrameFilter::new();
        f.set_enabled(true).set_id_range(0x180, 0x1FF).set_protocol(Protocol::Canopen);
        // ID 与协议均匹配。
        assert!(f.matches_parsed(&parsed_canopen(0x181)));
        // ID 命中但协议不匹配。
        assert!(!f.matches_parsed(&parsed(Protocol::Raw, frame(0x181, &[1]))));
        // 协议命中但 ID 越界。
        assert!(!f.matches_parsed(&parsed_canopen(0x080)));
    }

    /// enabled = false 时全部通过, 即使设置了过滤条件。
    #[test]
    fn disabled_passes_everything() {
        let f = FrameFilter::new();
        assert!(f.matches(&msg(0x000, Direction::Tx)));
        assert!(f.matches_parsed(&parsed_canopen(0x000)));
        assert!(f.matches_frame(&frame_ext(0x1CEBFF80, &[])));

        // 设置条件后再关闭总开关, 依然全部通过。
        let mut f = FrameFilter::new();
        f.set_enabled(true)
            .set_id_range(0x100, 0x1FF)
            .set_protocol(Protocol::Canopen)
            .set_direction(Direction::Rx);
        f.set_enabled(false);
        assert!(f.matches(&msg(0x080, Direction::Tx)));
        assert!(f.matches_parsed(&parsed(Protocol::J1939, frame_ext(0x18FEF100, &[1]))));
    }

    /// 高亮: ID 命中规则返回对应样式。
    #[test]
    fn highlight_id_rule_style() {
        let rule = HighlightRule::new(HighlightStyle::Yellow).with_id(0x181);
        assert_eq!(rule.highlight_for(&parsed_canopen(0x181)), HighlightStyle::Yellow);
        // 非命中 ID → Default。
        assert_eq!(rule.highlight_for(&parsed_canopen(0x182)), HighlightStyle::Default);
    }

    /// 高亮: 协议命中规则返回对应样式。
    #[test]
    fn highlight_protocol_rule_style() {
        let rule = HighlightRule::new(HighlightStyle::Green).with_protocol(Protocol::J1939);
        let j1939 = parsed(Protocol::J1939, frame_ext(0x18FEF100, &[1]));
        assert_eq!(rule.highlight_for(&j1939), HighlightStyle::Green);
        // 非命中协议 → Default。
        assert_eq!(rule.highlight_for(&parsed_canopen(0x181)), HighlightStyle::Default);
    }

    /// 高亮引擎: 先命中者优先, 全部未命中 → Default。
    #[test]
    fn highlighter_first_match_wins() {
        let mut h = Highlighter::new();
        h.add(HighlightRule::new(HighlightStyle::Cyan).with_id(0x181))
            .add(HighlightRule::new(HighlightStyle::Red).with_protocol(Protocol::Canopen));
        // 第一条 (ID 0x181) 命中, Cyan 优先于协议规则。
        assert_eq!(h.highlight_for(&parsed_canopen(0x181)), HighlightStyle::Cyan);
        // 第一条未命中, 协议规则生效。
        assert_eq!(h.highlight_for(&parsed_canopen(0x182)), HighlightStyle::Red);
        // 全部未命中 → Default。
        assert_eq!(h.highlight_for(&parsed(Protocol::Raw, frame(0x080, &[1]))), HighlightStyle::Default);
        assert!(h.rules().len() == 2);
    }

    /// 无匹配条件的规则永不命中。
    #[test]
    fn rule_without_conditions_never_matches() {
        let rule = HighlightRule::new(HighlightStyle::Red);
        assert_eq!(rule.highlight_for(&parsed_canopen(0x181)), HighlightStyle::Default);
        assert!(!rule.matches(&parsed_canopen(0x181)));
    }
}
