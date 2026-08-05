//! # CANopen 下发面板
//!
//! 提供 NMT / SDO 读 / SDO 写 / 原始帧四种下发服务的表单输入与发送。
//!
//! ## 状态机
//!
//! ```text
//! Hidden → SelectType → FillFields → (验证 → 发送) → Hidden
//!                              ↑                          ↑
//!                            Esc                        成功
//! ```
//!
//! ## 快捷键 (面板可见时)
//!
//! | 按键        | 功能                              |
//! |------------|-----------------------------------|
//! | `x`        | 打开面板                          |
//! | `Esc`      | 取消关闭                          |
//! | `Tab`      | 切换字段                          |
//! | `Enter`    | 确认类型 / 发送帧                 |
//! | `0-9/a-f`  | 输入十六进制字符                   |
//! | `Backspace`| 删除末尾字符                      |

use can_types::CanFrame;
use canopen_stack::{CanopenService, NmtCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// 面板固定宽度 (字符)。
const PANEL_WIDTH: u16 = 52;
/// 面板最小高度 (字符)。
const PANEL_MIN_HEIGHT: u16 = 3;

/// NMT 命令选项 (下拉列表)。
const NMT_OPTIONS: &[(&str, NmtCommand)] = &[
    ("1:START", NmtCommand::StartRemoteNode),
    ("2:STOP", NmtCommand::StopRemoteNode),
    ("3:PREOP", NmtCommand::EnterPreOperational),
    ("4:RESET", NmtCommand::ResetNode),
    ("5:RSTCOMM", NmtCommand::ResetCommunication),
];

/// 下发服务类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceType {
    /// NMT 节点控制。
    Nmt,
    /// SDO 上传 (读) 请求。
    SdoRead,
    /// SDO 下载 (写) 请求。
    SdoWrite,
    /// 原始 CAN 帧。
    Raw,
}

/// 服务类型列表 (用于选择循环)。
const SERVICE_TYPES: &[ServiceType] = &[
    ServiceType::Nmt,
    ServiceType::SdoRead,
    ServiceType::SdoWrite,
    ServiceType::Raw,
];

impl ServiceType {
    /// 显示标签。
    pub fn label(self) -> &'static str {
        match self {
            ServiceType::Nmt => "NMT",
            ServiceType::SdoRead => "SDO读",
            ServiceType::SdoWrite => "SDO写",
            ServiceType::Raw => "原始帧",
        }
    }
}

/// 文本输入字段。
#[derive(Debug, Clone)]
pub struct TextField {
    /// 当前输入内容。
    value: String,
    /// 显示标签。
    pub label: String,
    /// 占位提示。
    placeholder: String,
}

impl TextField {
    /// 创建文本输入字段。
    ///
    /// @param label       字段标签。
    /// @param placeholder 占位提示。
    /// @return 新建的字段。
    pub fn new(label: impl Into<String>, placeholder: impl Into<String>) -> Self {
        Self {
            value: String::new(),
            label: label.into(),
            placeholder: placeholder.into(),
        }
    }

    /// 追加一个字符到末尾。
    ///
    /// @param ch 待追加字符。
    pub fn push(&mut self, ch: char) {
        self.value.push(ch);
    }

    /// 删除末尾字符。
    pub fn pop(&mut self) {
        self.value.pop();
    }

    /// 获取当前输入值。
    ///
    /// @return 输入字符串切片。
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// 清空输入。
    pub fn clear(&mut self) {
        self.value.clear();
    }
}

/// 下拉选择字段 (整数索引循环)。
#[derive(Debug, Clone)]
pub struct SelectField {
    /// 当前选中索引。
    index: usize,
    /// 选项总数。
    count: usize,
    /// 显示标签。
    pub label: String,
}

impl SelectField {
    /// 创建选择字段。
    ///
    /// @param label  字段标签。
    /// @param count  选项总数。
    /// @return 新建的字段 (默认选中第一项)。
    pub fn new(label: impl Into<String>, count: usize) -> Self {
        Self {
            index: 0,
            count,
            label: label.into(),
        }
    }

    /// 切换到下一个选项 (循环)。
    pub fn next(&mut self) {
        self.index = (self.index + 1) % self.count;
    }

    /// 获取当前选中索引。
    ///
    /// @return 选项索引。
    pub fn value(&self) -> usize {
        self.index
    }

    /// 重置为第一项。
    pub fn reset(&mut self) {
        self.index = 0;
    }
}

/// 下发面板状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelState {
    /// 面板隐藏。
    Hidden,
    /// 选择服务类型。
    SelectType,
    /// 填写表单字段。
    FillFields,
}

/// 下发面板。
///
/// 管理表单状态、字段输入与帧构造, 由 [`super::app::App`] 驱动按键与渲染。
pub struct SendPanel {
    /// 当前状态。
    pub state: PanelState,
    /// 当前选中的服务类型。
    pub service_type: ServiceType,
    /// NMT 命令选择。
    pub nmt_cmd: SelectField,
    /// 文本输入字段列表。
    pub fields: Vec<TextField>,
    /// 当前聚焦字段索引 (在 `fields` 中)。
    pub active_field: usize,
    /// 错误提示。
    pub error: Option<String>,
    /// Enter 按下且验证通过, 等待 App 调用 try_send。
    ready_to_send: bool,
}

impl SendPanel {
    /// 创建下发面板 (初始隐藏)。
    ///
    /// @return 面板实例。
    pub fn new() -> Self {
        Self {
            state: PanelState::Hidden,
            service_type: ServiceType::Nmt,
            nmt_cmd: SelectField::new("命令", NMT_OPTIONS.len()),
            fields: Vec::new(),
            active_field: 0,
            error: None,
            ready_to_send: false,
        }
    }

    /// 面板是否可见。
    ///
    /// @return `true` 表示面板正在显示。
    pub fn is_visible(&self) -> bool {
        self.state != PanelState::Hidden
    }

    /// 打开面板 (进入类型选择)。
    pub fn open(&mut self) {
        self.state = PanelState::SelectType;
        self.service_type = ServiceType::Nmt;
        self.error = None;
        self.ready_to_send = false;
    }

    /// 关闭面板 (返回隐藏)。
    pub fn close(&mut self) {
        self.state = PanelState::Hidden;
        self.error = None;
        self.ready_to_send = false;
    }

    /// 查询是否 Enter 验证通过, 等待发送。
    ///
    /// @return `true` 表示 App 应调用 [`SendPanel::try_send`]。
    pub fn ready_to_send(&self) -> bool {
        self.ready_to_send
    }

    /// 显示错误信息 (由 App 调用 try_send 失败后回写)。
    ///
    /// @param msg 错误描述。
    pub fn show_error(&mut self, msg: String) {
        self.error = Some(msg);
        self.ready_to_send = false;
    }

    /// 确认类型选择, 进入字段填写。
    pub fn confirm_type(&mut self) {
        self.fields = build_fields(self.service_type);
        self.nmt_cmd.reset();
        self.active_field = 0;
        self.error = None;
        self.state = PanelState::FillFields;
    }

    /// 尝试构建帧并发送。
    ///
    /// @param send 发送闭包 (接受 CanFrame, 返回 Result)。
    /// @return 成功 `Ok(())`; 验证或发送失败 `Err(msg)`。
    pub fn try_send(&self, send: impl FnOnce(CanFrame) -> Result<(), String>) -> Result<(), String> {
        let frame = build_frame(self.service_type, &self.nmt_cmd, &self.fields)?;
        send(frame)
    }

    /// 处理键盘事件, 返回 `true` 表示已消费。
    ///
    /// @param key  键盘事件。
    /// @return `true` 表示面板消费了此按键。
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press || self.state == PanelState::Hidden {
            return false;
        }

        match self.state {
            PanelState::Hidden => false,
            PanelState::SelectType => self.handle_select_type(key),
            PanelState::FillFields => self.handle_fill_fields(key),
        }
    }

    /// 处理类型选择阶段按键。
    fn handle_select_type(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.close();
                true
            }
            KeyCode::Enter => {
                self.confirm_type();
                true
            }
            KeyCode::Tab => {
                let idx = SERVICE_TYPES
                    .iter()
                    .position(|&t| t == self.service_type)
                    .unwrap_or(0);
                self.service_type = SERVICE_TYPES[(idx + 1) % SERVICE_TYPES.len()];
                true
            }
            KeyCode::Char(c) => {
                if let Some(idx) = c.to_digit(10) {
                    let idx = idx as usize;
                    if idx >= 1 && idx <= SERVICE_TYPES.len() {
                        self.service_type = SERVICE_TYPES[idx - 1];
                    }
                }
                true
            }
            _ => true,
        }
    }

    /// 处理字段填写阶段按键。
    ///
    /// NMT 服务有一个虚拟的"命令"槽位 (索引 0, 由 [`Self::nmt_cmd`] 承载,
    /// 类型行显示), 其后才是真正的文本字段。因此 NMT 的 Tab 循环范围是
    /// `fields.len() + 1` (命令槽 + 字段), 其他服务直接循环文本字段。
    fn handle_fill_fields(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.close();
                true
            }
            KeyCode::Tab => {
                // NMT: 槽位数 = 命令槽 + 字段数; 其他: 字段数。
                let slots = if self.service_type == ServiceType::Nmt {
                    self.fields.len() + 1
                } else {
                    self.fields.len()
                };
                self.active_field = (self.active_field + 1) % slots;
                self.error = None;
                true
            }
            KeyCode::Enter => {
                // 验证并构建帧; 成功则标记 ready_to_send, 失败则显示错误。
                match build_frame(self.service_type, &self.nmt_cmd, &self.fields) {
                    Ok(_) => {
                        self.error = None;
                        self.ready_to_send = true;
                        true
                    }
                    Err(msg) => {
                        self.error = Some(msg);
                        self.ready_to_send = false;
                        true
                    }
                }
            }
            KeyCode::Backspace => {
                // NMT 的命令槽 (0) 无文本可删, 只处理字段槽位。
                if self.service_type == ServiceType::Nmt {
                    if self.active_field >= 1 {
                        if let Some(f) = self.fields.get_mut(self.active_field - 1) {
                            f.pop();
                        }
                    }
                } else if let Some(f) = self.fields.get_mut(self.active_field) {
                    f.pop();
                }
                self.error = None;
                true
            }
            KeyCode::Char(c) if is_hex_char(c) => {
                if self.service_type == ServiceType::Nmt {
                    if self.active_field == 0 {
                        // 命令槽: 数字 1-5 选择 NMT 命令。
                        if let Some(d) = c.to_digit(10) {
                            let d = d as usize;
                            if d >= 1 && d <= NMT_OPTIONS.len() {
                                self.nmt_cmd.index = d - 1;
                            }
                        }
                    } else if let Some(f) = self.fields.get_mut(self.active_field - 1) {
                        f.push(c);
                    }
                } else if let Some(f) = self.fields.get_mut(self.active_field) {
                    f.push(c);
                }
                self.error = None;
                true
            }
            _ => true,
        }
    }

    /// 渲染面板 (浮动居中)。
    ///
    /// @param frame ratatui 帧。
    /// @param area  终端全区域。
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if self.state == PanelState::Hidden {
            return;
        }

        let height = self.calc_height();
        let panel = centered_rect(PANEL_WIDTH, height, area);

        frame.render_widget(Clear, panel);

        let block = Block::default()
            .title("CANopen 下发")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(panel);
        frame.render_widget(block, panel);

        match self.state {
            PanelState::SelectType => self.render_type_selector(frame, inner),
            PanelState::FillFields => self.render_form(frame, inner),
            PanelState::Hidden => {}
        }
    }

    /// 计算面板所需高度。
    fn calc_height(&self) -> u16 {
        match self.state {
            PanelState::Hidden => 0,
            PanelState::SelectType => 5,
            PanelState::FillFields => {
                let n = self.fields.len() as u16;
                // 类型行 + 字段 + 错误行 + 提示行
                1 + n + 2
            }
        }
        .max(PANEL_MIN_HEIGHT)
    }

    /// 渲染类型选择界面。
    fn render_type_selector(&self, frame: &mut Frame, area: Rect) {
        let layout = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ]);
        let chunks = layout.split(area);

        let types_line = Line::from(Span::styled(
            format!(
                " 1:NMT  2:SDO读  3:SDO写  4:原始帧  [当前: {}]",
                self.service_type.label()
            ),
            Style::default().fg(Color::White),
        ));
        frame.render_widget(Paragraph::new(types_line), chunks[0]);

        let hint = Line::from(Span::styled(
            " Tab:切换  Enter:确认  Esc:取消",
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(Paragraph::new(hint), chunks[2]);
    }

    /// 渲染表单。
    fn render_form(&self, frame: &mut Frame, area: Rect) {
        let n = self.fields.len();
        let mut constraints: Vec<Constraint> = vec![Constraint::Length(1)]; // 类型行
        for _ in 0..n {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Length(1)); // 错误行
        constraints.push(Constraint::Length(1)); // 提示行

        let layout = Layout::vertical(constraints);
        let chunks = layout.split(area);

        // 类型行。
        let type_label = match self.service_type {
            ServiceType::Nmt => {
                let cmd_name = NMT_OPTIONS
                    .get(self.nmt_cmd.value())
                    .map(|(name, _)| *name)
                    .unwrap_or("?");
                format!("  [NMT] 命令: {cmd_name}")
            }
            other => format!("  [{}]", other.label()),
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                type_label,
                Style::default().fg(Color::Yellow),
            ))),
            chunks[0],
        );

        // 字段行。
        for (i, field) in self.fields.iter().enumerate() {
            // NMT 的槽位 0 是命令槽 (类型行显示), 字段从槽位 1 开始。
            let slot = if self.service_type == ServiceType::Nmt {
                i + 1
            } else {
                i
            };
            let is_active = slot == self.active_field;
            let style = if is_active {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let cursor = if is_active { "▸" } else { " " };
            let display_value = if field.value.is_empty() && !is_active {
                &field.placeholder
            } else {
                &field.value
            };
            let line = Line::from(vec![
                Span::styled(format!("{cursor}{}: ", field.label), style),
                Span::raw(display_value.to_string()),
            ]);
            frame.render_widget(Paragraph::new(line), chunks[1 + i]);
        }

        // 错误行。
        let err_idx = 1 + n;
        if let Some(err) = &self.error {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("  ⚠ {err}"),
                    Style::default().fg(Color::Red),
                ))),
                chunks[err_idx],
            );
        }

        // 提示行。
        let hint_idx = 2 + n;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " Tab:切换字段  Enter:发送  Esc:取消",
                Style::default().fg(Color::DarkGray),
            ))),
            chunks[hint_idx],
        );
    }
}

impl Default for SendPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// 按当前服务类型构建表单字段列表。
pub fn build_fields(st: ServiceType) -> Vec<TextField> {
    match st {
        ServiceType::Nmt => vec![TextField::new("节点ID", "1-127")],
        ServiceType::SdoRead => vec![
            TextField::new("节点ID", "1-127"),
            TextField::new("索引", "0x1000"),
            TextField::new("子索引", "0-255"),
        ],
        ServiceType::SdoWrite => vec![
            TextField::new("节点ID", "1-127"),
            TextField::new("索引", "0x1000"),
            TextField::new("子索引", "0-255"),
            TextField::new("数据", "01020304"),
        ],
        ServiceType::Raw => vec![
            TextField::new("ID", "0x123"),
            TextField::new("数据", "01020304"),
        ],
    }
}

/// 从表单状态构造 CAN 帧。
pub fn build_frame(
    st: ServiceType,
    nmt_cmd: &SelectField,
    fields: &[TextField],
) -> Result<CanFrame, String> {
    match st {
        ServiceType::Nmt => {
            let node = parse_node_id(fields.first().ok_or("缺少节点ID")?)?;
            let cmd = NMT_OPTIONS
                .get(nmt_cmd.value())
                .map(|(_, cmd)| *cmd)
                .ok_or("无效的 NMT 命令")?;
            CanopenService::nmt_frame(cmd, node).map_err(|e| format!("构造帧失败: {e}"))
        }
        ServiceType::SdoRead => {
            let node = parse_node_id(&fields[0])?;
            let index = parse_hex_u16(&fields[1], "索引")?;
            let subindex = parse_hex_u8(&fields[2], "子索引")?;
            CanopenService::sdo_read_frame(node, index, subindex)
                .map_err(|e| format!("构造帧失败: {e}"))
        }
        ServiceType::SdoWrite => {
            let node = parse_node_id(&fields[0])?;
            let index = parse_hex_u16(&fields[1], "索引")?;
            let subindex = parse_hex_u8(&fields[2], "子索引")?;
            let data = parse_hex_bytes(&fields[3], "数据")?;
            if data.is_empty() || data.len() > 8 {
                return Err("数据长度须为 1-8 字节".to_string());
            }
            CanopenService::sdo_write_frame(node, index, subindex, &data)
                .map_err(|e| format!("构造帧失败: {e}"))
        }
        ServiceType::Raw => {
            let raw_id = parse_hex_u32(&fields[0], "ID")?;
            let data = parse_hex_bytes(&fields[1], "数据")?;
            if data.len() > 8 {
                return Err("数据长度须 ≤ 8 字节".to_string());
            }
            if raw_id > 0x1FFF_FFFF {
                return Err("ID 超出 29 位范围".to_string());
            }
            let id = if raw_id > 0x7FF {
                can_types::CanId::new_extended(raw_id)
            } else {
                can_types::CanId::new_standard(raw_id as u16)
            }
            .map_err(|e| format!("无效 ID: {e}"))?;
            CanFrame::new(id, data).map_err(|e| format!("构造帧失败: {e}"))
        }
    }
}

/// 判断字符是否为合法十六进制输入 (0-9, a-f, A-F)。
pub fn is_hex_char(c: char) -> bool {
    c.is_ascii_hexdigit()
}

/// 从文本字段解析节点号 (十进制 1-127)。
pub fn parse_node_id(field: &TextField) -> Result<u8, String> {
    let s = field.as_str().trim();
    if s.is_empty() {
        return Err("节点ID不能为空".to_string());
    }
    let val: u8 = s
        .parse()
        .map_err(|_| format!("节点ID '{}' 不是有效数字", s))?;
    if val == 0 || val > 127 {
        return Err("节点ID须为 1-127".to_string());
    }
    Ok(val)
}

/// 从文本字段解析 u16 十六进制值 (支持 `0x` 前缀)。
pub fn parse_hex_u16(field: &TextField, name: &str) -> Result<u16, String> {
    let s = field.as_str().trim();
    if s.is_empty() {
        return Err(format!("{name}不能为空"));
    }
    let stripped = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"));
    let hex_str = stripped.unwrap_or(s);
    u16::from_str_radix(hex_str, 16).map_err(|_| format!("{name} '{}' 不是有效十六进制", s))
}

/// 从文本字段解析 u8 十六进制值 (支持 `0x` 前缀)。
pub fn parse_hex_u8(field: &TextField, name: &str) -> Result<u8, String> {
    let s = field.as_str().trim();
    if s.is_empty() {
        return Err(format!("{name}不能为空"));
    }
    let stripped = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"));
    let hex_str = stripped.unwrap_or(s);
    u8::from_str_radix(hex_str, 16).map_err(|_| format!("{name} '{}' 不是有效十六进制", s))
}

/// 从文本字段解析 u32 十六进制值 (支持 `0x` 前缀)。
pub fn parse_hex_u32(field: &TextField, name: &str) -> Result<u32, String> {
    let s = field.as_str().trim();
    if s.is_empty() {
        return Err(format!("{name}不能为空"));
    }
    let stripped = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"));
    let hex_str = stripped.unwrap_or(s);
    u32::from_str_radix(hex_str, 16).map_err(|_| format!("{name} '{}' 不是有效十六进制", s))
}

/// 从文本字段解析十六进制字节序列 (连续 hex 字符, 如 "01020A")。
pub fn parse_hex_bytes(field: &TextField, name: &str) -> Result<Vec<u8>, String> {
    let s = field.as_str().trim().replace(' ', "");
    if s.is_empty() {
        return Err(format!("{name}不能为空"));
    }
    if !s.len().is_multiple_of(2) {
        return Err(format!("{name}长度须为偶数 (每 2 位 hex = 1 字节)"));
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    for chunk in s.as_bytes().chunks(2) {
        let hex = std::str::from_utf8(chunk).map_err(|_| format!("{name}含非法字符"))?;
        let byte = u8::from_str_radix(hex, 16)
            .map_err(|_| format!("{name} '{}' 含非十六进制字符", hex))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

/// 计算居中浮动矩形。
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center);
    let horizontal = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center);
    let [v_area] = vertical.areas(area);
    let [h_area] = horizontal.areas(v_area);
    h_area
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 输入解析测试 ----

    #[test]
    fn parse_node_id_valid() {
        let f = TextField::new("节点ID", "");
        let mut f1 = f.clone();
        f1.push('1');
        assert_eq!(parse_node_id(&f1).unwrap(), 1);

        let mut f127 = f;
        for c in "127".chars() {
            f127.push(c);
        }
        assert_eq!(parse_node_id(&f127).unwrap(), 127);
    }

    #[test]
    fn parse_node_id_invalid() {
        let mut f = TextField::new("节点ID", "");
        f.push('0');
        assert!(parse_node_id(&f).is_err());

        let mut f = TextField::new("节点ID", "");
        for c in "128".chars() {
            f.push(c);
        }
        assert!(parse_node_id(&f).is_err());

        let mut f = TextField::new("节点ID", "");
        f.push('G');
        assert!(parse_node_id(&f).is_err());
    }

    #[test]
    fn parse_hex_u16_valid() {
        let mut f = TextField::new("索引", "");
        for c in "0x1000".chars() {
            f.push(c);
        }
        assert_eq!(parse_hex_u16(&f, "索引").unwrap(), 0x1000);

        let mut f = TextField::new("索引", "");
        for c in "2010".chars() {
            f.push(c);
        }
        assert_eq!(parse_hex_u16(&f, "索引").unwrap(), 0x2010);
    }

    #[test]
    fn parse_hex_u16_invalid() {
        let mut f = TextField::new("索引", "");
        f.push('G');
        assert!(parse_hex_u16(&f, "索引").is_err());

        let f = TextField::new("索引", "");
        assert!(parse_hex_u16(&f, "索引").is_err());
    }

    #[test]
    fn parse_hex_bytes_valid() {
        let mut f = TextField::new("数据", "");
        for c in "01020A0B".chars() {
            f.push(c);
        }
        assert_eq!(parse_hex_bytes(&f, "数据").unwrap(), vec![0x01, 0x02, 0x0A, 0x0B]);
    }

    #[test]
    fn parse_hex_bytes_odd_length() {
        let mut f = TextField::new("数据", "");
        for c in "012".chars() {
            f.push(c);
        }
        assert!(parse_hex_bytes(&f, "数据").is_err());
    }

    #[test]
    fn parse_hex_bytes_invalid_chars() {
        let mut f = TextField::new("数据", "");
        for c in "GGHH".chars() {
            f.push(c);
        }
        assert!(parse_hex_bytes(&f, "数据").is_err());
    }

    // ---- 表单状态机测试 ----

    #[test]
    fn state_machine_hidden_to_select() {
        let mut panel = SendPanel::new();
        assert!(!panel.is_visible());
        panel.open();
        assert!(panel.is_visible());
        assert_eq!(panel.state, PanelState::SelectType);
    }

    #[test]
    fn state_machine_select_to_fill() {
        let mut panel = SendPanel::new();
        panel.open();
        let key = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        panel.handle_key(key);
        assert_eq!(panel.state, PanelState::FillFields);
    }

    #[test]
    fn state_machine_esc_closes() {
        let mut panel = SendPanel::new();
        panel.open();
        let esc = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
        panel.handle_key(esc);
        assert!(!panel.is_visible());

        panel.open();
        let enter = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        panel.handle_key(enter);
        assert!(panel.is_visible());
        panel.handle_key(esc);
        assert!(!panel.is_visible());
    }

    #[test]
    fn state_machine_type_switch() {
        let mut panel = SendPanel::new();
        panel.open();
        assert_eq!(panel.service_type, ServiceType::Nmt);

        let tab = KeyEvent::new(KeyCode::Tab, crossterm::event::KeyModifiers::NONE);
        panel.handle_key(tab);
        assert_eq!(panel.service_type, ServiceType::SdoRead);

        panel.handle_key(tab);
        assert_eq!(panel.service_type, ServiceType::SdoWrite);

        panel.handle_key(tab);
        assert_eq!(panel.service_type, ServiceType::Raw);

        panel.handle_key(tab);
        assert_eq!(panel.service_type, ServiceType::Nmt);
    }

    #[test]
    fn state_machine_type_digit_select() {
        let mut panel = SendPanel::new();
        panel.open();

        let key3 = KeyEvent::new(KeyCode::Char('3'), crossterm::event::KeyModifiers::NONE);
        panel.handle_key(key3);
        assert_eq!(panel.service_type, ServiceType::SdoWrite);
    }

    /// 键盘全流程: NMT 命令槽 (槽位 0) 与节点ID 字段 (槽位 1) 经 Tab 切换,
    /// 输入后可构造出 NMT 帧。回归测试: 曾因槽位溢出无法键盘输入节点ID。
    #[test]
    fn nmt_keyboard_flow_sends_start_node1() {
        let mut panel = SendPanel::new();
        panel.open();
        let key = |c: KeyCode| KeyEvent::new(c, crossterm::event::KeyModifiers::NONE);

        // 打开面板 → 确认 NMT → 槽位 0 (命令), 数字 1 = START。
        panel.handle_key(key(KeyCode::Enter));
        assert_eq!(panel.state, PanelState::FillFields);
        assert_eq!(panel.active_field, 0);
        panel.handle_key(key(KeyCode::Char('1')));
        assert_eq!(panel.nmt_cmd.value(), 0);

        // Tab → 槽位 1 (节点ID 字段), 输入 "1"。
        panel.handle_key(key(KeyCode::Tab));
        assert_eq!(panel.active_field, 1);
        panel.handle_key(key(KeyCode::Char('1')));
        assert_eq!(panel.fields[0].as_str(), "1");

        // Enter → 验证通过, 可发送; 帧为 NMT START node 1。
        panel.handle_key(key(KeyCode::Enter));
        assert!(panel.ready_to_send());
        let mut sent = None;
        panel
            .try_send(|f| {
                sent = Some(f);
                Ok(())
            })
            .expect("NMT 帧应可发送");
        let frame = sent.expect("应捕获发送帧");
        assert_eq!(frame.id().raw_id(), 0x000);
        assert_eq!(frame.data(), &[0x01, 0x01]);
    }

    // ---- 帧构造测试 ----

    #[test]
    fn build_nmt_start_node1() {
        let mut panel = SendPanel::new();
        panel.open();
        panel.service_type = ServiceType::Nmt;
        panel.confirm_type();
        panel.fields[0].push('1');

        let result = panel.try_send(|_| Ok(()));
        assert!(result.is_ok());
    }

    #[test]
    fn build_nmt_frame_id_data() {
        let cmd = SelectField::new("命令", NMT_OPTIONS.len());
        let mut node_field = TextField::new("节点ID", "");
        node_field.push('1');
        let frame = build_frame(ServiceType::Nmt, &cmd, &[node_field]).unwrap();
        assert_eq!(frame.id().raw_id(), 0x000);
        assert_eq!(frame.data(), &[1, 1]);
    }

    #[test]
    fn build_sdo_read_frame() {
        let mut panel = SendPanel::new();
        panel.open();
        panel.service_type = ServiceType::SdoRead;
        panel.confirm_type();
        for c in "5".chars() {
            panel.fields[0].push(c);
        }
        for c in "1017".chars() {
            panel.fields[1].push(c);
        }
        panel.fields[2].push('0');

        let frame = build_frame(
            panel.service_type,
            &panel.nmt_cmd,
            &panel.fields,
        )
        .unwrap();
        assert_eq!(frame.id().raw_id(), 0x605);
        assert_eq!(frame.data(), &[0x40, 0x17, 0x10, 0x00, 0, 0, 0, 0]);
    }

    #[test]
    fn build_sdo_write_frame() {
        let mut panel = SendPanel::new();
        panel.open();
        panel.service_type = ServiceType::SdoWrite;
        panel.confirm_type();
        panel.fields[0].push('1');
        for c in "2010".chars() {
            panel.fields[1].push(c);
        }
        panel.fields[2].push('0');
        for c in "0102".chars() {
            panel.fields[3].push(c);
        }

        let frame = build_frame(
            panel.service_type,
            &panel.nmt_cmd,
            &panel.fields,
        )
        .unwrap();
        assert_eq!(frame.id().raw_id(), 0x601);
        assert_eq!(frame.data(), &[0x2B, 0x10, 0x20, 0x00, 0x01, 0x02, 0x00, 0x00]);
    }

    #[test]
    fn build_raw_frame_standard() {
        let mut panel = SendPanel::new();
        panel.open();
        panel.service_type = ServiceType::Raw;
        panel.confirm_type();
        for c in "123".chars() {
            panel.fields[0].push(c);
        }
        for c in "AABB".chars() {
            panel.fields[1].push(c);
        }

        let frame = build_frame(
            panel.service_type,
            &panel.nmt_cmd,
            &panel.fields,
        )
        .unwrap();
        assert_eq!(frame.id().raw_id(), 0x123);
        assert!(frame.id().is_standard());
        assert_eq!(frame.data(), &[0xAA, 0xBB]);
    }

    #[test]
    fn build_raw_frame_extended() {
        let mut panel = SendPanel::new();
        panel.open();
        panel.service_type = ServiceType::Raw;
        panel.confirm_type();
        for c in "1FFFFFFF".chars() {
            panel.fields[0].push(c);
        }
        panel.fields[1].push('0');
        panel.fields[1].push('1');

        let frame = build_frame(
            panel.service_type,
            &panel.nmt_cmd,
            &panel.fields,
        )
        .unwrap();
        assert_eq!(frame.id().raw_id(), 0x1FFF_FFFF);
        assert!(frame.id().is_extended());
    }

    // ---- 非法输入测试 (不 panic, 返回错误) ----

    #[test]
    fn invalid_node_returns_error() {
        let mut panel = SendPanel::new();
        panel.open();
        panel.service_type = ServiceType::Nmt;
        panel.confirm_type();
        panel.fields[0].push('0');

        let result = panel.try_send(|_| Ok(()));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("1-127"));
    }

    #[test]
    fn empty_index_returns_error() {
        let mut panel = SendPanel::new();
        panel.open();
        panel.service_type = ServiceType::SdoRead;
        panel.confirm_type();
        panel.fields[0].push('1');

        let result = panel.try_send(|_| Ok(()));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("索引"));
    }

    #[test]
    fn invalid_hex_data_returns_error() {
        let mut panel = SendPanel::new();
        panel.open();
        panel.service_type = ServiceType::SdoWrite;
        panel.confirm_type();
        panel.fields[0].push('1');
        for c in "1000".chars() {
            panel.fields[1].push(c);
        }
        panel.fields[2].push('0');
        for c in "GGGG".chars() {
            panel.fields[3].push(c);
        }

        let result = panel.try_send(|_| Ok(()));
        assert!(result.is_err());
    }

    #[test]
    fn odd_hex_length_returns_error() {
        let mut panel = SendPanel::new();
        panel.open();
        panel.service_type = ServiceType::SdoWrite;
        panel.confirm_type();
        panel.fields[0].push('1');
        for c in "1000".chars() {
            panel.fields[1].push(c);
        }
        panel.fields[2].push('0');
        for c in "012".chars() {
            panel.fields[3].push(c);
        }

        let result = panel.try_send(|_| Ok(()));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("偶数"));
    }

    #[test]
    fn overlong_data_returns_error() {
        let mut panel = SendPanel::new();
        panel.open();
        panel.service_type = ServiceType::SdoWrite;
        panel.confirm_type();
        panel.fields[0].push('1');
        for c in "1000".chars() {
            panel.fields[1].push(c);
        }
        panel.fields[2].push('0');
        for c in "010203040506070809".chars() {
            panel.fields[3].push(c);
        }

        let result = panel.try_send(|_| Ok(()));
        assert!(result.is_err());
    }

    #[test]
    fn raw_id_too_large_returns_error() {
        let id_field = TextField::new("ID", "");
        let mut id_f = id_field.clone();
        for c in "2FFFFFFF".chars() {
            id_f.push(c);
        }
        let mut data_f = TextField::new("数据", "");
        data_f.push('0');
        data_f.push('1');

        let cmd = SelectField::new("命令", NMT_OPTIONS.len());
        let result = build_frame(ServiceType::Raw, &cmd, &[id_f, data_f]);
        assert!(result.is_err());
    }

    // ---- 辅助工具测试 ----

    #[test]
    fn text_input_push_and_pop() {
        let mut f = TextField::new("test", "");
        f.push('a');
        f.push('b');
        assert_eq!(f.as_str(), "ab");
        f.pop();
        assert_eq!(f.as_str(), "a");
        f.pop();
        assert!(f.as_str().is_empty());
        f.pop();
        assert!(f.as_str().is_empty());
    }

    #[test]
    fn text_input_clear() {
        let mut f = TextField::new("test", "");
        f.push('a');
        f.push('b');
        f.clear();
        assert!(f.as_str().is_empty());
    }

    #[test]
    fn select_field_cycle() {
        let mut s = SelectField::new("test", 3);
        assert_eq!(s.value(), 0);
        s.next();
        assert_eq!(s.value(), 1);
        s.next();
        assert_eq!(s.value(), 2);
        s.next();
        assert_eq!(s.value(), 0);
        s.reset();
        assert_eq!(s.value(), 0);
    }

    #[test]
    fn hex_char_recognition() {
        assert!(is_hex_char('0'));
        assert!(is_hex_char('9'));
        assert!(is_hex_char('a'));
        assert!(is_hex_char('f'));
        assert!(is_hex_char('A'));
        assert!(is_hex_char('F'));
        assert!(!is_hex_char('g'));
        assert!(!is_hex_char('G'));
        assert!(!is_hex_char(' '));
        assert!(!is_hex_char('-'));
    }
}
