//! # TUI 模块
//!
//! 基于 ratatui 的终端用户界面模块, 提供:
//! - [`app::App`](crate::tui::app::App): TUI 应用主结构, 含事件循环与三区布局渲染;
//! - `stream` : 报文流列表组件;
//! - `status` : 状态栏组件;
//! - `send`   : 手动发送面板。

/// TUI 应用主结构与事件循环。
pub mod app;

/// CANopen 下发面板 (NMT / SDO 读写 / 原始帧)。
pub mod send;

/// 状态栏纯渲染组件。
pub mod status;

/// 报文流列表组件 (Table 渲染 / 滚动 / 高亮)。
pub mod stream;
