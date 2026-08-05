//! # TUI 应用主结构
//!
//! 基于 ratatui 的 CAN 监控终端应用, 提供:
//! - 三区布局: 消息区 (≥60%) + 状态栏 (3 行) + 帮助行 (1 行)
//! - crossterm 事件轮询 (50ms 间隔)
//! - 监控开关 (默认关闭, 空格/S 切换)
//! - 消息窗口 (VecDeque, 上限 1000 帧)
//!
//! ## 快捷键
//!
//! | 按键       | 功能                       |
//! |-----------|---------------------------|
//! | `q`       | 退出                       |
//! | 空格 / `s` | 切换监控开关                |

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use can_types::{CanMessage, Direction};
use crossbeam_channel::Receiver;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::bus::MonitorBus;
use crate::classifier::FrameClassifier;
use crate::filter::FrameFilter;

/// 消息窗口最大帧数 (防内存泄漏)。
const MAX_MESSAGES: usize = 1000;

/// 显示用消息, 携带原始消息与可选的分类结果。
#[derive(Debug, Clone)]
pub struct DisplayMessage {
    /// 原始统一消息。
    pub raw: CanMessage,
    /// 分类结果 (用于后续高亮显示, Task 16 细化)。
    pub parsed: Option<crate::classifier::ParsedMessage>,
}

/// TUI 应用主结构。
///
/// 持有消息总线、接收 channel、过滤器、消息窗口与 UI 状态,
/// 通过 [`App::run`] 驱动事件循环。监控开关**默认关闭**。
pub struct App {
    /// 消息总线 (监控开关控制)。
    bus: MonitorBus,
    /// 帧分类器 (共享, 用于将原始帧分类为 [`ParsedMessage`])。
    classifier: Arc<Mutex<FrameClassifier>>,
    /// 消息接收端。
    rx: Receiver<CanMessage>,
    /// 错误接收端。
    err_rx: Receiver<String>,
    /// 帧过滤器。
    filter: FrameFilter,
    /// 消息窗口 (环形缓冲, 最多 [`MAX_MESSAGES`] 帧)。
    messages: VecDeque<DisplayMessage>,
    /// 监控开关状态 (默认关闭)。
    monitoring: bool,
    /// 退出标志。
    should_quit: bool,
    /// 最近一条错误信息。
    last_error: Option<String>,
}

/// CLI 参数解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliArgs {
    /// 后端类型: "socketcan" / "usbvci" / "none"。
    pub backend: String,
    /// SocketCAN 接口名。
    pub iface: String,
    /// 是否启用 CANFD。
    pub fd: bool,
    /// 日志文件路径 (可选)。
    pub log_file: Option<String>,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            backend: "none".to_string(),
            iface: "can0".to_string(),
            fd: false,
            log_file: None,
        }
    }
}

/// 从参数迭代器解析 CLI 参数。
///
/// @param args 参数迭代器 (不含程序名)。
/// @return 解析结果 [`CliArgs`]; 遇到 `--help` 或无效参数返回 `None`。
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Option<CliArgs> {
    let mut result = CliArgs::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                return None;
            }
            "--backend" => {
                if let Some(val) = args.next() {
                    result.backend = val;
                }
            }
            "--iface" => {
                if let Some(val) = args.next() {
                    result.iface = val;
                }
            }
            "--fd" => {
                result.fd = true;
            }
            "--log-file" => {
                if let Some(val) = args.next() {
                    result.log_file = Some(val);
                }
            }
            _ => {
                eprintln!("未知参数: {arg}");
                print_usage();
                return None;
            }
        }
    }

    Some(result)
}

/// 打印 CLI 用法说明。
fn print_usage() {
    eprintln!(
        "用法: can-monitor [选项]\n\
         \n\
         选项:\n\
         --backend <socketcan|usbvci|none>  后端类型 (默认 none)\n\
         --iface <name>                     SocketCAN 接口名 (默认 can0)\n\
         --fd                               启用 CANFD\n\
         --log-file <path>                  日志文件路径\n\
         --help, -h                         显示此帮助"
    );
}

impl App {
    /// 创建 TUI 应用。
    ///
    /// 监控开关初始为**关闭**。
    ///
    /// @param bus        消息总线。
    /// @param classifier 帧分类器 (共享)。
    /// @param rx         消息接收端。
    /// @param err_rx     错误接收端。
    /// @param filter     帧过滤器。
    /// @return 应用实例。
    pub fn new(
        bus: MonitorBus,
        classifier: Arc<Mutex<FrameClassifier>>,
        rx: Receiver<CanMessage>,
        err_rx: Receiver<String>,
        filter: FrameFilter,
    ) -> Self {
        Self {
            bus,
            classifier,
            rx,
            err_rx,
            filter,
            messages: VecDeque::with_capacity(MAX_MESSAGES),
            monitoring: false,
            should_quit: false,
            last_error: None,
        }
    }

    /// 运行 TUI 事件循环。
    ///
    /// 初始化终端 (ratatui::init), 进入事件循环, 退出时清理终端状态
    /// (ratatui::restore)。
    ///
    /// @return 成功返回 `Ok(())`; 终端操作失败返回 IO 错误。
    pub fn run(&mut self) -> io::Result<()> {
        let mut terminal = ratatui::init();

        loop {
            // 渲染当前状态。
            terminal.draw(|frame| self.render(frame))?;

            // 轮询键盘事件 (50ms 超时, 不阻塞渲染)。
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key(key);
                }
            }

            // 处理消息 channel。
            self.drain_messages();

            // 处理错误 channel。
            self.drain_errors();

            // 检查退出标志。
            if self.should_quit {
                break;
            }
        }

        ratatui::restore();
        Ok(())
    }

    /// 处理键盘事件 (仅处理 [`KeyEventKind::Press`])。
    ///
    /// @param key 键盘事件。
    fn handle_key(&mut self, key: KeyEvent) {
        // 只处理 Press 事件, 忽略 Release / Repeat (crossterm 0.28 三态)。
        if key.kind != KeyEventKind::Press {
            return;
        }

        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
            }
            KeyCode::Char(' ') | KeyCode::Char('s') => {
                self.monitoring = !self.monitoring;
                self.bus.set_monitoring(self.monitoring);
            }
            _ => {}
        }
    }

    /// 从消息 channel 拉取所有待处理消息, 过滤后推入窗口。
    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            // 使用 matches 过滤 (基于 ID 范围 + 方向)。
            if !self.filter.matches(&msg) {
                continue;
            }

            // 分类消息 (用于后续高亮显示, Task 16 细化)。
            let parsed = self
                .classifier
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .classify(&msg.frame);

            // 推入消息窗口, 超限时移除最旧的。
            if self.messages.len() >= MAX_MESSAGES {
                self.messages.pop_front();
            }
            self.messages.push_back(DisplayMessage {
                raw: msg,
                parsed: Some(parsed),
            });
        }
    }

    /// 从错误 channel 拉取所有待处理错误。
    fn drain_errors(&mut self) {
        while let Ok(err) = self.err_rx.try_recv() {
            self.last_error = Some(err);
        }
    }

    /// 渲染三区布局。
    ///
    /// - 消息区: 占大部分空间 (min 60%), 显示消息列表或占位提示。
    /// - 状态栏: 3 行, 显示监控状态与计数。
    /// - 帮助行: 1 行, 显示快捷键说明。
    ///
    /// @param frame ratatui 帧。
    fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        // 三区垂直布局: 消息区 (Min) + 状态栏 (Length 3) + 帮助行 (Length 1)。
        let layout = Layout::vertical([
            Constraint::Min(10),   // 消息区: 至少 10 行
            Constraint::Length(3), // 状态栏: 3 行
            Constraint::Length(1), // 帮助行: 1 行
        ]);
        let chunks = layout.split(area);

        self.render_messages(frame, chunks[0]);
        self.render_status(frame, chunks[1]);
        self.render_help(frame, chunks[2]);
    }

    /// 渲染消息区。
    ///
    /// 监控关闭时显示占位提示; 开启后显示消息列表。
    ///
    /// @param frame ratatui 帧。
    /// @param area  消息区矩形。
    fn render_messages(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title("CAN 消息")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        if !self.monitoring {
            let placeholder = Paragraph::new("监控已关闭 — 按 SPACE 开始监控")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(placeholder, area);
            return;
        }

        if self.messages.is_empty() {
            let waiting = Paragraph::new("等待消息...")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(waiting, area);
            return;
        }

        // 构建消息行列表 (最新消息在上)。
        let lines: Vec<Line> = self
            .messages
            .iter()
            .rev()
            .map(|msg| {
                let id = msg.raw.frame.id();
                let id_str = if id.is_extended() {
                    format!("{:08X}", id.raw_id())
                } else {
                    format!("{:03X}", id.raw_id())
                };
                let data: String = msg
                    .raw
                    .frame
                    .data()
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect();
                let dir = match msg.raw.direction {
                    Direction::Rx => "Rx",
                    Direction::Tx => "Tx",
                };
                Line::from(vec![
                    Span::styled(format!("{id_str} "), Style::default().fg(Color::Yellow)),
                    Span::styled(format!("{dir} "), Style::default().fg(Color::Cyan)),
                    Span::raw(data),
                ])
            })
            .collect();

        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, area);
    }

    /// 渲染状态栏。
    ///
    /// @param frame ratatui 帧。
    /// @param area  状态栏矩形。
    fn render_status(&self, frame: &mut Frame, area: Rect) {
        let status_style = if self.monitoring {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        };

        let status_text = if self.monitoring {
            "监控: ON"
        } else {
            "监控: OFF"
        };

        let total = self.bus.total_frames();
        let canopen = self.bus.canopen_count();
        let j1939 = self.bus.j1939_count();
        let errors = self.bus.error_count();

        let block = Block::default()
            .title("状态")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let line = Line::from(vec![
            Span::styled(format!("{status_text}  "), status_style),
            Span::raw(format!(
                "帧: {total}  CANopen: {canopen}  J1939: {j1939}  错误: {errors}"
            )),
        ]);

        let paragraph = Paragraph::new(line).block(block);
        frame.render_widget(paragraph, area);
    }

    /// 渲染帮助行。
    ///
    /// @param frame ratatui 帧。
    /// @param area  帮助行矩形。
    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let help_text = "q:退出 SPACE/S:切换监控";
        let paragraph =
            Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
    }

    /// 获取监控开关状态。
    ///
    /// @return `true` 表示正在监控。
    pub fn is_monitoring(&self) -> bool {
        self.monitoring
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI 参数解析: 默认值。
    #[test]
    fn parse_args_default() {
        let args: Vec<String> = vec![];
        let result = parse_args(args).unwrap();
        assert_eq!(result.backend, "none");
        assert_eq!(result.iface, "can0");
        assert!(!result.fd);
        assert_eq!(result.log_file, None);
    }

    /// CLI 参数解析: 完整参数。
    #[test]
    fn parse_args_full() {
        let args: Vec<String> = vec![
            "--backend".into(),
            "socketcan".into(),
            "--iface".into(),
            "vcan0".into(),
            "--fd".into(),
            "--log-file".into(),
            "/tmp/test.log".into(),
        ];
        let result = parse_args(args).unwrap();
        assert_eq!(result.backend, "socketcan");
        assert_eq!(result.iface, "vcan0");
        assert!(result.fd);
        assert_eq!(result.log_file, Some("/tmp/test.log".into()));
    }

    /// CLI 参数解析: --help 返回 None。
    #[test]
    fn parse_args_help() {
        let args: Vec<String> = vec!["--help".into()];
        assert!(parse_args(args).is_none());
    }

    /// CLI 参数解析: -h 返回 None。
    #[test]
    fn parse_args_h() {
        let args: Vec<String> = vec!["-h".into()];
        assert!(parse_args(args).is_none());
    }

    /// CLI 参数解析: 未知参数返回 None。
    #[test]
    fn parse_args_unknown() {
        let args: Vec<String> = vec!["--unknown".into()];
        assert!(parse_args(args).is_none());
    }

    /// App::new 默认 monitoring = false。
    #[test]
    fn app_default_monitoring_off() {
        let (bus, rx, err_rx) = MonitorBus::new();
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        let filter = FrameFilter::new();
        let app = App::new(bus, classifier, rx, err_rx, filter);
        assert!(!app.is_monitoring());
    }
}
