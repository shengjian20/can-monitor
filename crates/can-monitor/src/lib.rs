//! # can-monitor — CAN 监控主程序库
//!
//! 提供监控主程序的可复用库部分。核心解析与总线逻辑已提取至
//! [`can_monitor_core`]: 帧分类器 (`classifier`)、消息总线 (`bus`)、
//! candump 日志 (`logger`) 与帧过滤引擎 (`filter`)。本 crate 仅保留
//! 基于 ratatui 的终端用户界面模块 [`tui`]。
//!
//! ## 核心数据流
//!
//! 后端读帧 → [`can_monitor_core::bus::MonitorBus`] 的 reader 线程 →
//! [`can_monitor_core::classifier::FrameClassifier`] 分类 →
//! 封装为 [`CanMessage`](can_types::CanMessage) 投递到有界 channel,
//! 由 [`tui`] 层轮询消费。监控开关默认关闭, 需显式开启后才会消费后端帧。

/// 基于 ratatui 的终端用户界面模块。
pub mod tui;
