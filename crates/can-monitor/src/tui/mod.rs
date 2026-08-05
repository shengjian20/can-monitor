//! # TUI 模块
//!
//! 基于 ratatui 的终端用户界面模块, 提供:
//! - [`app::App`] : TUI 应用主结构, 含事件循环与三区布局渲染。
//!
//! 后续任务将在此模块下扩展:
//! - `stream` : 报文流列表组件 (Task 16)
//! - `status` : 状态栏组件 (Task 17)
//! - `send`   : 手动发送面板 (Task 18)

/// TUI 应用主结构与事件循环。
pub mod app;

/// CANopen 下发面板 (NMT / SDO 读写 / 原始帧)。
pub mod send;

/// 状态栏纯渲染组件 (计划任务 17)。
pub mod status;

/// 报文流列表组件 (Table 渲染 / 滚动 / 高亮)。
pub mod stream;
