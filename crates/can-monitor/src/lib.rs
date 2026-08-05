//! # can-monitor — CAN 监控主程序库
//!
//! 提供监控主程序的可复用库部分。当前包含:
//! - [`classifier`] : 帧分类器, 将原始帧分发到 CANopen / J1939 协议栈;
//! - [`bus`] : 消息总线, 后台 reader 线程 + 有界 channel 数据通路;
//! - [`logger`] : candump -L 兼容的 CAN 帧日志记录器 (计划任务 13)。
//!
//! ## 核心数据流
//!
//! 后端读帧 → [`bus::MonitorBus`] 的 reader 线程 → [`classifier::FrameClassifier`]
//! 分类 → 封装为 [`CanMessage`](can_types::CanMessage) 投递到有界 channel,
//! 供 TUI 层轮询消费。监控开关默认关闭, 需显式开启后才会消费后端帧。

/// 协议无关的帧分类器模块。
pub mod classifier;

/// 消息总线与后台读取线程模块。
pub mod bus;

/// candump 兼容的日志记录器模块。
pub mod logger;

/// 帧过滤引擎 (过滤条件 + ID 高亮规则)。
pub mod filter;
