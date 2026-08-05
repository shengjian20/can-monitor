//! SocketCAN 非 Linux 平台降级 stub。
//!
//! SocketCAN 是 Linux 内核特性 (`AF_CAN` 套接字),在 macOS / Windows 上不存在,
//! 本模块仅在 `target_os != "linux"` 时编译。
//!
//! 公开 API 与 Linux 真实实现 (`crate::real`) **完全一致** ([`SocketCanBackend`]
//! 实现 [`CanBackend`]),保证上层 (如 can-monitor) 在非 Linux 平台仍能编译;
//! 所有打开 / 读写操作运行时返回 [`CanError::NotFound`],不 panic、不虚构功能,
//! 避免上层误以为 SocketCAN 可用。

use std::time::Duration;

use can_types::{
    BackendConfig, CanBackend, CanDeviceInfo, CanError, CanFrame, DeviceDiscoverer, Result,
};

/// SocketCAN 后端 (非 Linux 平台降级占位)。
///
/// SocketCAN 仅 Linux 可用。此占位实现保持与 Linux 版相同的类型形状与
/// [`CanBackend`] 行为签名,但所有打开 / 读写操作返回 [`CanError::NotFound`]。
pub struct SocketCanBackend {
    /// 私有字段,禁止外部直接构造 (与 Linux 实现的不可直接构造语义一致)。
    _private: (),
}

impl CanBackend for SocketCanBackend {
    /// 打开 SocketCAN 后端。
    ///
    /// 非 Linux 平台不支持 SocketCAN,始终返回 [`CanError::NotFound`]。
    ///
    /// @param config 后端配置。
    /// @return 恒为 [`CanError::NotFound`] (SocketCAN 仅 Linux 可用);
    ///         非 SocketCan 配置返回 [`CanError::Unsupported`]。
    fn open(config: &BackendConfig) -> Result<Self> {
        match config {
            BackendConfig::SocketCan { .. } => Err(CanError::NotFound),
            _ => Err(CanError::Unsupported("仅支持 SocketCan 后端配置")),
        }
    }

    /// 从总线读取一帧。
    ///
    /// 非 Linux 平台无 SocketCAN,始终返回 [`CanError::NotFound`]。
    ///
    /// @param _timeout 阻塞等待一帧的最长时间 (忽略)。
    /// @return 恒为 [`CanError::NotFound`] (SocketCAN 仅 Linux 可用)。
    fn read_frame(&mut self, _timeout: Duration) -> Result<CanFrame> {
        Err(CanError::NotFound)
    }

    /// 向总线写入一帧。
    ///
    /// 非 Linux 平台无 SocketCAN,始终返回 [`CanError::NotFound`]。
    ///
    /// @param _frame 待发送的帧 (忽略)。
    /// @return 恒为 [`CanError::NotFound`] (SocketCAN 仅 Linux 可用)。
    fn write_frame(&mut self, _frame: &CanFrame) -> Result<()> {
        Err(CanError::NotFound)
    }

    /// 关闭后端。
    ///
    /// stub 未持有任何资源,恒成功。
    ///
    /// @return 恒为 `Ok(())`。
    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

/// SocketCAN 设备发现器 (非 Linux 平台降级占位)。
///
/// 与 Linux 真实实现 (`crate::real::SocketCanDiscoverer`) 同名同 API;非 Linux
/// 平台无 SocketCAN 子系统,恒返回空列表 (不 panic,不虚构设备)。
pub struct SocketCanDiscoverer;

impl DeviceDiscoverer for SocketCanDiscoverer {
    /// 非 Linux 平台无可发现的 SocketCAN 设备。
    ///
    /// @return 恒为空列表 (SocketCAN 仅 Linux 可用)。
    fn list_devices() -> Vec<CanDeviceInfo> {
        Vec::new()
    }
}

/// 非 Linux 平台无可发现的 SocketCAN 设备,恒返回空列表 (不 panic)。
pub fn list_devices() -> Vec<CanDeviceInfo> {
    SocketCanDiscoverer::list_devices()
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_types::CanId;

    /// 非 Linux 平台打开 SocketCAN 配置应返回 NotFound (SocketCAN 仅 Linux 可用)。
    #[test]
    fn stub_open_returns_not_found() {
        let cfg = BackendConfig::SocketCan {
            iface: "can0".into(),
            fd: false,
        };
        assert!(matches!(
            SocketCanBackend::open(&cfg),
            Err(CanError::NotFound)
        ));
    }

    /// 非 Linux 平台读 / 写应返回 NotFound 且不 panic。
    #[test]
    fn stub_read_write_return_not_found() {
        let mut backend = SocketCanBackend { _private: () };
        assert!(matches!(
            backend.read_frame(Duration::from_secs(1)),
            Err(CanError::NotFound)
        ));

        let frame = CanFrame::new(CanId::new_standard(0x123).unwrap(), vec![1, 2, 3]).unwrap();
        assert!(matches!(
            backend.write_frame(&frame),
            Err(CanError::NotFound)
        ));
    }

    /// close 应恒成功 (stub 无资源可释放)。
    #[test]
    fn stub_close_ok() {
        let mut backend = SocketCanBackend { _private: () };
        assert!(backend.close().is_ok());
    }

    /// 非 Linux 平台设备发现应返回空列表且不 panic。
    #[test]
    fn stub_list_devices_empty() {
        assert!(SocketCanDiscoverer::list_devices().is_empty());
        assert!(list_devices().is_empty());
    }
}
