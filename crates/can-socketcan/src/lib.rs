//! # can-socketcan — SocketCAN 后端实现
//!
//! 基于 Linux SocketCAN 子系统 ([`socketcan`](https://crates.io/crates/socketcan))
//! 的 [`CanBackend`] 实现,支持经典 CAN (2.0) 与 CAN FD 帧收发。
//!
//! ## 特性
//!
//! - 按 [`BackendConfig::SocketCan`] 打开接口 (`fd=false` 经典 / `fd=true` FD)
//! - 非阻塞读 + 超时语义 (轮询 + 休眠,累计超时返回 [`CanError::Timeout`])
//! - 内核接收时间戳 (SO_TIMESTAMPNS) 优先,不可用时回退 `SystemTime::now()`
//! - 标准帧 / 扩展帧 / FD 帧 (BRS / ESI) / 远程帧完整转换
//!
//! ## 平台
//!
//! SocketCAN 是 Linux 内核特性 (`AF_CAN` 套接字),在其他平台 (macOS / Windows)
//! 上不存在。本 crate 按目标平台条件编译:
//!
//! - `target_os = "linux"`: 真实实现 ([`real`] 模块,依赖 `socketcan` crate)
//! - 其他平台: 降级 stub ([`stub`] 模块),公开 API 完全一致,但
//!   `open` / `read_frame` / `write_frame` 运行时返回 [`CanError::NotFound`],
//!   保证上层 (如 can-monitor) 在非 Linux 平台仍可编译且不会误报成功
//!
//! ## 测试策略
//!
//! - 纯函数单元测试:帧 / ID 转换逻辑不依赖真实硬件 (仅 Linux 编译)
//! - vcan 集成测试:由 `vcan-test` feature 门控 (需要本机存在 `vcan0`)

#[cfg(target_os = "linux")]
mod real;
#[cfg(target_os = "linux")]
pub use real::*;

/// SocketCAN 非 Linux 平台降级模块。
///
/// 仅在不支持 SocketCAN 的平台编译,提供与 [`real`] 相同的公开 API。
#[cfg(not(target_os = "linux"))]
mod stub;
#[cfg(not(target_os = "linux"))]
pub use stub::*;
