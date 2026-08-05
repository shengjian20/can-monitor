//! # can-monitor-core — CAN 监控核心库
//!
//! 协议无关的 CAN 监控核心逻辑, 供上层 UI (TUI / Web / GUI) 复用。当前包含:
//! - [`classifier`] : 帧分类器, 将原始帧分发到 CANopen / J1939 协议栈;
//! - [`bus`] : 消息总线, 后台 reader 线程 + 有界 channel 数据通路;
//! - [`logger`] : candump -L 兼容的 CAN 帧日志记录器;
//! - [`filter`] : 帧过滤引擎 (过滤条件 + ID 高亮规则)。
//!
//! ## 核心数据流
//!
//! 后端读帧 → [`bus::MonitorBus`] 的 reader 线程 → [`classifier::FrameClassifier`]
//! 分类 → 封装为 [`CanMessage`](can_types::CanMessage) 投递到有界 channel,
//! 供上层轮询消费。监控开关默认关闭, 需显式开启后才会消费后端帧。
//!
//! ## 约束
//!
//! 本 crate 不依赖任何 UI 库 (ratatui / crossterm), 不包含 unsafe 代码,
//! 上层界面层负责把 [`filter::HighlightStyle`] 等纯枚举映射为具体 UI 样式。

/// 协议无关的帧分类器模块。
pub mod classifier;

/// 消息总线与后台读取线程模块。
pub mod bus;

/// candump 兼容的日志记录器模块。
pub mod logger;

/// 帧过滤引擎 (过滤条件 + ID 高亮规则)。
pub mod filter;

/// 纯 std 的 CLI 参数解析 (无 clap 依赖)。
pub mod cli;

/// 重新导出 crossbeam-channel, 供上层 crate 复用 channel 类型
/// (如 [`bus::MonitorBus::new`] 返回的 `Receiver` 接收端)。
pub use crossbeam_channel;
