//! # 状态栏组件
//!
//! 纯渲染组件, 由 [`App`](super::app::App) 每帧传入数据后渲染三行状态栏:
//!
//! - 第一行: 后端类型 + 接口名 + 监控开关 + 过滤开关 + 日志开关
//! - 第二行: 帧计数 (总帧 / CANopen / J1939 / 错误)
//! - 第三行: 最近错误信息 (若有)
//!
//! 颜色约定:
//! - 监控 ON → 绿色, OFF → 红色
//! - 过滤 ON → 绿色, OFF → 灰色
//! - 日志 ON → 绿色, OFF → 灰色, N/A → 灰色

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// 状态栏显示数据。
///
/// 纯数据结构, 由 [`App`](super::app::App) 每帧填充后传入渲染函数。
/// 不持有任何引用到总线或过滤器, 便于单元测试。
pub struct StatusBarData<'a> {
    /// 后端类型名称 (如 "SocketCAN" / "USBCAN" / "None")。
    pub backend: &'a str,
    /// 接口名 (如 "can0" / "vcan0")。
    pub iface: &'a str,
    /// 监控开关状态。
    pub monitoring: bool,
    /// 已读帧总数。
    pub total_frames: u64,
    /// CANopen 帧数。
    pub canopen_count: u64,
    /// J1939 帧数。
    pub j1939_count: u64,
    /// 后端错误帧数。
    pub error_count: u64,
    /// 过滤总开关状态。
    pub filter_enabled: bool,
    /// 日志开关状态; `None` 表示未配置日志。
    pub logger_enabled: Option<bool>,
    /// 最近一条错误信息。
    pub last_error: Option<&'a str>,
}

impl<'a> StatusBarData<'a> {
    /// 格式化监控开关文本与颜色。
    ///
    /// @return `(文本, 颜色)` 元组: ON 绿色 / OFF 红色。
    pub fn monitoring_text(&self) -> (&'static str, Color) {
        if self.monitoring {
            ("ON", Color::Green)
        } else {
            ("OFF", Color::Red)
        }
    }

    /// 格式化过滤开关文本与颜色。
    ///
    /// @return `(文本, 颜色)` 元组: ON 绿色 / OFF 灰色。
    pub fn filter_text(&self) -> (&'static str, Color) {
        if self.filter_enabled {
            ("ON", Color::Green)
        } else {
            ("OFF", Color::DarkGray)
        }
    }

    /// 格式化日志开关文本与颜色。
    ///
    /// @return `(文本, 颜色)` 元组: ON 绿色 / OFF 灰色 / N/A 灰色。
    pub fn logger_text(&self) -> (&'static str, Color) {
        match self.logger_enabled {
            Some(true) => ("ON", Color::Green),
            Some(false) => ("OFF", Color::DarkGray),
            None => ("N/A", Color::DarkGray),
        }
    }

    /// 格式化帧计数摘要行。
    ///
    /// @return 形如 `帧:123 CANopen:45 J1939:6 错误:0` 的文本。
    pub fn counts_text(&self) -> String {
        format!(
            "帧:{} CANopen:{} J1939:{} 错误:{}",
            self.total_frames, self.canopen_count, self.j1939_count, self.error_count
        )
    }
}

/// 渲染状态栏 (3 行 block)。
///
/// 第一行: 后端 + 接口 + 监控 + 过滤 + 日志。
/// 第二行: 帧计数摘要。
/// 第三行: 最近错误 (若有) 或空白占位。
///
/// @param data 状态栏数据。
/// @param frame ratatui 帧。
/// @param area  状态栏矩形区域。
pub fn render_status_bar(data: &StatusBarData, frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .title("状态")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let (mon_text, mon_color) = data.monitoring_text();
    let (filt_text, filt_color) = data.filter_text();
    let (log_text, log_color) = data.logger_text();

    // 第一行: 后端 接口 | 监控:ON/OFF | 过滤:ON/OFF | 日志:ON/OFF
    let line1 = Line::from(vec![
        Span::styled(format!("{} ", data.backend), Style::default().fg(Color::Yellow)),
        Span::styled(data.iface, Style::default().fg(Color::Cyan)),
        Span::raw("  "),
        Span::styled("监控:", Style::default().fg(Color::White)),
        Span::styled(mon_text, Style::default().fg(mon_color)),
        Span::raw("  "),
        Span::styled("过滤:", Style::default().fg(Color::White)),
        Span::styled(filt_text, Style::default().fg(filt_color)),
        Span::raw("  "),
        Span::styled("日志:", Style::default().fg(Color::White)),
        Span::styled(log_text, Style::default().fg(log_color)),
    ]);

    // 第二行: 帧计数
    let line2 = Line::from(vec![Span::raw(data.counts_text())]);

    // 第三行: 错误信息或空白占位
    let line3 = match data.last_error {
        Some(err) => Line::from(vec![
            Span::styled(format!("⚠ {err}"), Style::default().fg(Color::Red)),
        ]),
        None => Line::from(vec![Span::raw("")]),
    };

    let paragraph = Paragraph::new(vec![line1, line2, line3]).block(block);
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestData {
        backend: &'static str,
        iface: &'static str,
        monitoring: bool,
        total: u64,
        canopen: u64,
        j1939: u64,
        errors: u64,
        filter: bool,
        logger: Option<bool>,
    }

    impl Default for TestData {
        fn default() -> Self {
            Self {
                backend: "None",
                iface: "can0",
                monitoring: false,
                total: 0,
                canopen: 0,
                j1939: 0,
                errors: 0,
                filter: false,
                logger: None,
            }
        }
    }

    impl TestData {
        fn build(self) -> StatusBarData<'static> {
            StatusBarData {
                backend: self.backend,
                iface: self.iface,
                monitoring: self.monitoring,
                total_frames: self.total,
                canopen_count: self.canopen,
                j1939_count: self.j1939,
                error_count: self.errors,
                filter_enabled: self.filter,
                logger_enabled: self.logger,
                last_error: None,
            }
        }
    }

    /// 监控关闭: 文本 OFF, 颜色红色。
    #[test]
    fn monitoring_off_returns_red() {
        let data = TestData::default().build();
        let (text, color) = data.monitoring_text();
        assert_eq!(text, "OFF");
        assert_eq!(color, Color::Red);
    }

    /// 监控开启: 文本 ON, 颜色绿色。
    #[test]
    fn monitoring_on_returns_green() {
        let data = TestData {
            backend: "SocketCAN",
            iface: "vcan0",
            monitoring: true,
            total: 100,
            canopen: 50,
            j1939: 30,
            errors: 2,
            filter: true,
            logger: Some(true),
        }
        .build();
        let (text, color) = data.monitoring_text();
        assert_eq!(text, "ON");
        assert_eq!(color, Color::Green);
    }

    /// 帧计数格式化: 标准值。
    #[test]
    fn counts_text_format_standard() {
        let data = TestData {
            monitoring: true,
            total: 1234,
            canopen: 567,
            j1939: 89,
            errors: 3,
            ..TestData::default()
        }
        .build();
        let text = data.counts_text();
        assert_eq!(text, "帧:1234 CANopen:567 J1939:89 错误:3");
    }

    /// 帧计数格式化: 全零。
    #[test]
    fn counts_text_format_zeros() {
        let data = TestData::default().build();
        assert_eq!(data.counts_text(), "帧:0 CANopen:0 J1939:0 错误:0");
    }

    /// 过滤开启: ON 绿色。
    #[test]
    fn filter_on_returns_green() {
        let data = TestData { filter: true, ..TestData::default() }.build();
        let (text, color) = data.filter_text();
        assert_eq!(text, "ON");
        assert_eq!(color, Color::Green);
    }

    /// 过滤关闭: OFF 灰色。
    #[test]
    fn filter_off_returns_dark_gray() {
        let data = TestData::default().build();
        let (text, color) = data.filter_text();
        assert_eq!(text, "OFF");
        assert_eq!(color, Color::DarkGray);
    }

    /// 日志未配置: N/A 灰色。
    #[test]
    fn logger_none_returns_na() {
        let data = TestData::default().build();
        let (text, color) = data.logger_text();
        assert_eq!(text, "N/A");
        assert_eq!(color, Color::DarkGray);
    }

    /// 日志开启: ON 绿色。
    #[test]
    fn logger_on_returns_green() {
        let data = TestData { logger: Some(true), ..TestData::default() }.build();
        let (text, color) = data.logger_text();
        assert_eq!(text, "ON");
        assert_eq!(color, Color::Green);
    }

    /// 日志关闭: OFF 灰色。
    #[test]
    fn logger_off_returns_dark_gray() {
        let data = TestData { logger: Some(false), ..TestData::default() }.build();
        let (text, color) = data.logger_text();
        assert_eq!(text, "OFF");
        assert_eq!(color, Color::DarkGray);
    }

    /// 大计数不会溢出或截断。
    #[test]
    fn counts_text_large_values() {
        let data = TestData {
            monitoring: true,
            total: u64::MAX,
            canopen: u64::MAX,
            j1939: u64::MAX,
            errors: u64::MAX,
            ..TestData::default()
        }
        .build();
        let text = data.counts_text();
        assert!(text.contains(&u64::MAX.to_string()));
    }
}
