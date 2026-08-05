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
//! | 按键            | 功能                    |
//! |----------------|------------------------|
//! | `q`            | 退出                    |
//! | 空格 / `s`     | 切换监控开关             |
//! | `f`            | 切换过滤开关             |
//! | `l`            | 切换日志记录             |
//! | `x`            | 打开 CANopen 下发面板    |
//! | ↑ / ↓          | 滚动消息列表             |
//! | PageUp/PageDown| 翻页                    |
//! | End            | 跟随尾部 (最新帧)        |

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use can_types::CanMessage;
use can_monitor_core::bus::MonitorBus;
use can_monitor_core::classifier::FrameClassifier;
use can_monitor_core::filter::FrameFilter;
use can_monitor_core::logger::CandumpLogger;
use can_monitor_core::crossbeam_channel::Receiver;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::send::SendPanel;
use crate::tui::status::{render_status_bar, StatusBarData};
use crate::tui::stream::MessageStream;

/// 消息窗口最大帧数 (防内存泄漏)。
const MAX_MESSAGES: usize = 1000;

/// 显示用消息, 携带原始消息与可选的分类结果。
#[derive(Debug, Clone)]
pub struct DisplayMessage {
    /// 原始统一消息。
    pub raw: CanMessage,
    /// 分类结果 (用于高亮与协议过滤)。
    ///
    /// 生产路径下 `classify` 恒返回 `Some`, 此处保持 `Option` 仅为
    /// 测试构造未分类消息方便 (如 [`crate::tui::stream`] 的测试桩)。
    pub parsed: Option<can_monitor_core::classifier::ParsedMessage>,
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
    /// 报文流列表组件 (Table 渲染 / 滚动 / 高亮)。
    message_stream: MessageStream,
    /// 消息窗口 (环形缓冲, 最多 [`MAX_MESSAGES`] 帧)。
    messages: VecDeque<DisplayMessage>,
    /// 监控开关状态 (默认关闭)。
    monitoring: bool,
    /// 退出标志。
    should_quit: bool,
    /// 最近一条错误信息。
    last_error: Option<String>,
    /// 后端类型名称 (状态栏显示)。
    backend_name: String,
    /// 接口名 (状态栏显示)。
    iface_name: String,
    /// 日志记录器 (可选, 由 main 传入)。
    logger: Option<CandumpLogger>,
    /// 日志开关 (独立于监控开关)。
    logging_enabled: bool,
    /// 日志写入失败累计次数 (用于状态栏计数, 避免静默丢帧)。
    logger_errors: u64,
    /// CANopen 下发面板。
    send_panel: SendPanel,
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

/// 默认 CLI 参数: 后端 none、接口 can0、非 FD、无日志文件。
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
            message_stream: MessageStream::new(),
            messages: VecDeque::with_capacity(MAX_MESSAGES),
            monitoring: false,
            should_quit: false,
            last_error: None,
            backend_name: "None".to_string(),
            iface_name: "can0".to_string(),
            logger: None,
            logging_enabled: false,
            logger_errors: 0,
            send_panel: SendPanel::new(),
        }
    }

    /// 设置后端类型名称 (状态栏显示)。
    ///
    /// @param name 后端名称 (如 "SocketCAN" / "USBCAN" / "None")。
    /// @return `&mut self` 以便链式调用。
    pub fn set_backend_name(&mut self, name: String) -> &mut Self {
        self.backend_name = name;
        self
    }

    /// 设置接口名 (状态栏显示)。
    ///
    /// @param iface 接口名 (如 "can0" / "vcan0")。
    /// @return `&mut self` 以便链式调用。
    pub fn set_iface_name(&mut self, iface: String) -> &mut Self {
        self.iface_name = iface;
        self
    }

    /// 挂载日志记录器 (由 main 在有 `--log-file` 时调用)。
    ///
    /// @param logger 已创建的 [`CandumpLogger`]。
    /// @return `&mut self` 以便链式调用。
    pub fn set_logger(&mut self, logger: CandumpLogger) -> &mut Self {
        self.logger = Some(logger);
        self.logging_enabled = true;
        self
    }

    /// 冲刷并关闭日志记录器 (退出前调用, 避免缓冲丢失)。
    ///
    /// @return 成功 `Ok(())`; 冲刷失败返回错误描述。
    pub fn close_logger(&mut self) -> std::result::Result<(), String> {
        if let Some(ref mut logger) = self.logger {
            logger.close().map_err(|e| format!("关闭日志文件失败: {e}"))
        } else {
            Ok(())
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
            terminal.draw(|frame| self.render(frame))?;

            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key(key);
                }
            }

            self.drain_messages();
            self.drain_errors();

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

        // 面板可见时, 所有按键路由到下发面板 (避免与主视图快捷键冲突)。
        if self.send_panel.is_visible() {
            self.send_panel.handle_key(key);
            // 面板处于 FillFields 且按 Enter 后无错误, 尝试发送。
            if self.send_panel.ready_to_send() {
                let send_result = self.send_panel.try_send(|frame| self.bus.send_frame(frame));
                if send_result.is_ok() {
                    self.send_panel.close();
                } else if let Err(msg) = send_result {
                    self.send_panel.show_error(msg);
                }
            }
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
            KeyCode::Char('f') => {
                let enabled = self.filter.is_enabled();
                self.filter.set_enabled(!enabled);
            }
            KeyCode::Char('l') => {
                if self.logger.is_some() {
                    self.logging_enabled = !self.logging_enabled;
                    if let Some(ref mut logger) = self.logger {
                        logger.set_enabled(self.logging_enabled);
                        if let Err(e) = logger.flush() {
                            self.last_error = Some(format!("日志冲刷失败: {e}"));
                        }
                    }
                }
            }
            KeyCode::Char('x') => {
                self.send_panel.open();
            }
            KeyCode::Up => {
                self.message_stream.previous_row(self.messages.len());
            }
            KeyCode::Down => {
                self.message_stream.next_row(self.messages.len());
            }
            KeyCode::PageUp => {
                let page = 10;
                self.message_stream.page_up(self.messages.len(), page);
            }
            KeyCode::PageDown => {
                let page = 10;
                self.message_stream.page_down(self.messages.len(), page);
            }
            KeyCode::End => {
                self.message_stream.end(self.messages.len());
            }
            _ => {}
        }
    }

    /// 从消息 channel 拉取所有待处理消息, 过滤后推入窗口。
    ///
    /// 若日志记录器已挂载且开启, 同步记录每帧到日志文件。
    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            // 日志记录 (在过滤前, 记录原始帧)。
            if let Some(ref mut logger) = self.logger {
                if self.logging_enabled {
                    if let Err(e) = logger.log_frame(&msg.frame, &self.iface_name) {
                        // 写盘失败: 累计错误并上报状态栏, 不静默丢弃。
                        self.logger_errors += 1;
                        self.last_error = Some(format!("日志写入失败: {e}"));
                    }
                }
            }

            if !self.filter.matches(&msg) {
                continue;
            }

            // 分类消息 (供高亮与协议过滤使用)。
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
    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();

        // 三区垂直布局: 消息区 (Min) + 状态栏 (Length 5) + 帮助行 (Length 1)。
        let layout = Layout::vertical([
            Constraint::Min(10),   // 消息区: 至少 10 行
            Constraint::Length(5), // 状态栏: 2 边框 + 状态/计数/错误 3 行
            Constraint::Length(1), // 帮助行: 1 行
        ]);
        let chunks = layout.split(area);

        self.render_messages(frame, chunks[0]);
        self.render_status(frame, chunks[1]);
        self.render_help(frame, chunks[2]);

        // 发送面板浮动渲染 (Hidden 时内部直接返回)。
        self.send_panel.render(frame, area);
    }

    /// 渲染消息区。
    ///
    /// 监控关闭时显示占位提示; 开启后用
    /// [`MessageStream`](crate::tui::stream::MessageStream) 渲染 Table。
    ///
    /// @param frame ratatui 帧。
    /// @param area  消息区矩形。
    fn render_messages(&mut self, frame: &mut Frame, area: Rect) {
        if !self.monitoring {
            let block = Block::default()
                .title("CAN 消息")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
            let placeholder = Paragraph::new("监控已关闭 — 按 SPACE 开始监控")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(placeholder, area);
            return;
        }

        let highlighter = self.filter.highlighter();
        self.message_stream
            .render(frame, area, self.messages.make_contiguous(), highlighter);
    }

    /// 渲染状态栏 (使用 [`StatusBarData`] + [`render_status_bar`])。
    ///
    /// @param frame ratatui 帧。
    /// @param area  状态栏矩形。
    fn render_status(&self, frame: &mut Frame, area: Rect) {
        let data = StatusBarData {
            backend: &self.backend_name,
            iface: &self.iface_name,
            monitoring: self.monitoring,
            total_frames: self.bus.total_frames(),
            canopen_count: self.bus.canopen_count(),
            j1939_count: self.bus.j1939_count(),
            error_count: self.bus.error_count() + self.logger_errors,
            filter_enabled: self.filter.is_enabled(),
            logger_enabled: self.logger.as_ref().map(|l| l.is_enabled()),
            last_error: self.last_error.as_deref(),
        };
        render_status_bar(&data, frame, area);
    }

    /// 渲染帮助行。
    ///
    /// @param frame ratatui 帧。
    /// @param area  帮助行矩形。
    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let help_text = "q:退出 SPACE/S:监控 f:过滤 l:日志 ↑↓:滚动 End:尾随 x:CANopen";
        let paragraph = Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray));
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

    /// App::new 默认 logger = None, logging_enabled = false。
    #[test]
    fn app_default_no_logger() {
        let (bus, rx, err_rx) = MonitorBus::new();
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        let filter = FrameFilter::new();
        let app = App::new(bus, classifier, rx, err_rx, filter);
        assert!(app.logger.is_none());
        assert!(!app.logging_enabled);
    }

    /// App::new 默认后端名 "None", 接口 "can0"。
    #[test]
    fn app_default_backend_info() {
        let (bus, rx, err_rx) = MonitorBus::new();
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        let filter = FrameFilter::new();
        let app = App::new(bus, classifier, rx, err_rx, filter);
        assert_eq!(app.backend_name, "None");
        assert_eq!(app.iface_name, "can0");
    }

    /// set_backend_name / set_iface_name 设置后可查询。
    #[test]
    fn app_set_backend_and_iface() {
        let (bus, rx, err_rx) = MonitorBus::new();
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        let filter = FrameFilter::new();
        let mut app = App::new(bus, classifier, rx, err_rx, filter);
        app.set_backend_name("SocketCAN".to_string());
        app.set_iface_name("vcan0".to_string());
        assert_eq!(app.backend_name, "SocketCAN");
        assert_eq!(app.iface_name, "vcan0");
    }

    /// 'f' 键切换过滤开关。
    #[test]
    fn key_f_toggles_filter() {
        let (bus, rx, err_rx) = MonitorBus::new();
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        let filter = FrameFilter::new();
        let mut app = App::new(bus, classifier, rx, err_rx, filter);

        // 初始: 过滤关闭。
        assert!(!app.filter.is_enabled());

        // 按 f: 过滤开启。
        let key_f = KeyEvent::new(KeyCode::Char('f'), crossterm::event::KeyModifiers::NONE);
        app.handle_key(key_f);
        assert!(app.filter.is_enabled());

        // 再按 f: 过滤关闭。
        app.handle_key(KeyEvent::new(
            KeyCode::Char('f'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(!app.filter.is_enabled());
    }

    /// 'l' 键无 logger 时无操作。
    #[test]
    fn key_l_no_logger_noop() {
        let (bus, rx, err_rx) = MonitorBus::new();
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        let filter = FrameFilter::new();
        let mut app = App::new(bus, classifier, rx, err_rx, filter);

        let key_l = KeyEvent::new(KeyCode::Char('l'), crossterm::event::KeyModifiers::NONE);
        app.handle_key(key_l);
        // 无 panic, logging_enabled 仍为 false。
        assert!(!app.logging_enabled);
    }

    /// 'l' 键有 logger 时切换日志开关。
    #[test]
    fn key_l_with_logger_toggles() {
        let (bus, rx, err_rx) = MonitorBus::new();
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        let filter = FrameFilter::new();
        let mut app = App::new(bus, classifier, rx, err_rx, filter);

        // 挂载 logger (临时文件)。
        let path = std::env::temp_dir().join(format!("test-{}-key_l.log", std::process::id()));
        let logger = can_monitor_core::logger::CandumpLogger::new(&path).unwrap();
        app.set_logger(logger);
        assert!(app.logging_enabled);

        // 按 l: 关闭日志。
        let key_l = KeyEvent::new(KeyCode::Char('l'), crossterm::event::KeyModifiers::NONE);
        app.handle_key(key_l);
        assert!(!app.logging_enabled);

        // 再按 l: 开启日志。
        app.handle_key(KeyEvent::new(
            KeyCode::Char('l'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(app.logging_enabled);

        // 清理。
        if let Some(ref mut logger) = app.logger {
            let _ = logger.close();
        }
        let _ = std::fs::remove_file(&path);
    }

    /// 'x' 键不 panic (占位)。
    #[test]
    fn key_x_no_panic() {
        let (bus, rx, err_rx) = MonitorBus::new();
        let classifier = Arc::new(Mutex::new(FrameClassifier::default()));
        let filter = FrameFilter::new();
        let mut app = App::new(bus, classifier, rx, err_rx, filter);

        let key_x = KeyEvent::new(KeyCode::Char('x'), crossterm::event::KeyModifiers::NONE);
        app.handle_key(key_x);
        // 不 panic 即可。
    }
}
