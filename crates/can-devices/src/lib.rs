//! # can-devices — 跨后端 CAN 设备发现聚合层
//!
//! 聚合两个后端的设备发现结果, 对外提供统一的设备列表:
//!
//! - **SocketCAN**: 经 [can_socketcan::SocketCanDiscoverer] 扫描 Linux sysfs
//!   (`/sys/class/net`, 见 can-socketcan Task 9) 枚举 CAN 网络接口;
//! - **USBCAN**: 经 [can_usbvci::UsbVciDiscoverer] 调用 VCI 动态库的
//!   `VCI_FindUsbDevice2` (见 can-usbvci Task 12) 枚举 USB-CAN 适配器。
//!
//! 本 crate 只做**纯聚合** (取并集 + 排序), 不直接触碰硬件; 各后端的枚举
//! 能力保留在各自 crate。任何后端的库未加载 / 无设备时该后端返回空列表,
//! 聚合结果同样不 panic。聚合顺序固定为 SocketCAN 在前、USBCAN 在后,
//! 与 [can_types::DeviceKind] 的枚举顺序一致。
//!
//! 聚合层不感知具体后端: 后续接入新后端时, 只需在其 crate 实现
//! [can_types::DeviceDiscoverer] 并在 [`DeviceManager::list_devices`] 中追加即可。

use can_types::{CanDeviceInfo, DeviceDiscoverer};

/// 设备管理器: 聚合所有已注册后端的设备发现结果。
///
/// 当前聚合 SocketCAN 与 USBCAN 两个后端, 顺序为 SocketCAN 在前。
/// 同时实现 [can_types::DeviceDiscoverer], 可直接注册进上层设备注册表
/// (与 T11 的发现抽象保持一致)。
pub struct DeviceManager;

impl DeviceManager {
    /// 枚举当前全部可发现设备。
    ///
    /// 聚合 [can_socketcan::SocketCanDiscoverer::list_devices] 与
    /// [can_usbvci::UsbVciDiscoverer::list_devices] 的结果 (SocketCAN 在前);
    /// 各后端内部已保证空列表 / 库未加载时不 panic。
    ///
    /// @return 聚合后的设备列表; 两后端均无设备时返回空列表, 不 panic。
    pub fn list_devices() -> Vec<CanDeviceInfo> {
        Self::merge(
            can_socketcan::SocketCanDiscoverer::list_devices(),
            can_usbvci::UsbVciDiscoverer::list_devices(),
        )
    }

    /// 合并两个后端的设备列表: SocketCAN 在前, USBCAN 在后。
    ///
    /// 纯函数, 不访问任何硬件 / 库; 测试通过注入模拟结果验证聚合顺序,
    /// 生产路径由 [`DeviceManager::list_devices`] 调用。
    ///
    /// @param socketcan SocketCAN 后端枚举结果 (可为空)。
    /// @param usbvci    USBCAN 后端枚举结果 (可为空)。
    /// @return 合并后的设备列表。
    pub fn merge(socketcan: Vec<CanDeviceInfo>, usbvci: Vec<CanDeviceInfo>) -> Vec<CanDeviceInfo> {
        let mut devices = Vec::with_capacity(socketcan.len() + usbvci.len());
        devices.extend(socketcan);
        devices.extend(usbvci);
        devices
    }
}

impl DeviceDiscoverer for DeviceManager {
    /// 枚举当前全部可发现设备 (等价于 [`DeviceManager::list_devices`])。
    ///
    /// @return 聚合后的设备列表; 无设备时返回空列表, 不 panic。
    fn list_devices() -> Vec<CanDeviceInfo> {
        DeviceManager::list_devices()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_types::{DeviceDetails, DeviceKind};

    /// 构造一台模拟 SocketCAN 设备。
    fn mock_socketcan(id: &str) -> CanDeviceInfo {
        CanDeviceInfo {
            id: id.to_string(),
            name: id.to_string(),
            kind: DeviceKind::SocketCan,
            driver: "socketcan".to_string(),
            details: DeviceDetails::with_model("SocketCAN"),
            available: true,
        }
    }

    /// 构造一台模拟 USBCAN 设备。
    fn mock_usbvci(id: &str, model: &str) -> CanDeviceInfo {
        CanDeviceInfo {
            id: id.to_string(),
            name: model.to_string(),
            kind: DeviceKind::UsbVci,
            driver: "usbvci".to_string(),
            details: DeviceDetails::with_model(model),
            available: true,
        }
    }

    /// mock usbvci + 空 socketcan → 聚合结果恰为 usbvci 列表, 型号保留。
    #[test]
    fn merge_empty_socketcan_with_usbvci() {
        let usbvci = vec![
            mock_usbvci("0", "USBCAN-II"),
            mock_usbvci("1", "USBCAN-E-U"),
        ];
        let merged = DeviceManager::merge(vec![], usbvci);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].kind, DeviceKind::UsbVci);
        assert_eq!(merged[0].details.model, "USBCAN-II");
        assert_eq!(merged[1].details.model, "USBCAN-E-U");
    }

    /// 两后端均有设备 → SocketCAN 在前, USBCAN 在后, 顺序稳定。
    #[test]
    fn merge_socketcan_first() {
        let socketcan = vec![mock_socketcan("vcan0"), mock_socketcan("can0")];
        let usbvci = vec![mock_usbvci("0", "USBCAN-II")];
        let merged = DeviceManager::merge(socketcan, usbvci);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].id, "vcan0");
        assert_eq!(merged[1].id, "can0");
        assert_eq!(merged[0].kind, DeviceKind::SocketCan);
        assert_eq!(merged[1].kind, DeviceKind::SocketCan);
        assert_eq!(merged[2].kind, DeviceKind::UsbVci);
        assert_eq!(merged[2].details.model, "USBCAN-II");
    }

    /// 两后端均无设备 → 空列表, 不 panic。
    #[test]
    fn merge_both_empty() {
        assert!(DeviceManager::merge(vec![], vec![]).is_empty());
    }

    /// 本机实扫: 调用聚合入口不 panic; 结果 kind 均合法。
    ///
    /// 本机无 USB-CAN 设备时 usbvci 返回空列表 (库已加载但枚举 0 台),
    /// socketcan 按本机实际接口扫描; 有无设备均属正确行为。
    #[test]
    fn list_devices_scans_host_without_panic() {
        let devices = DeviceManager::list_devices();
        for d in &devices {
            assert!(matches!(d.kind, DeviceKind::SocketCan | DeviceKind::UsbVci));
            assert!(!d.name.is_empty(), "设备应有非空名称");
        }
    }

    /// DeviceDiscoverer trait 入口与固有方法结果一致 (registry 兼容)。
    #[test]
    fn trait_entry_matches_inherent_method() {
        assert_eq!(
            <DeviceManager as DeviceDiscoverer>::list_devices().len(),
            DeviceManager::list_devices().len()
        );
    }
}
