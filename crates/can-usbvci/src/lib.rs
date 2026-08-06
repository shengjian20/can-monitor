//! # can-usbvci — ZLGCAN (周立功) USBCAN 系列 USB-CAN 适配器绑定
//!
//! 通过 VCI 动态库 ([`libcontrolcan`](https://www.zlg.cn/) 由 `scripts/fetch-vendor.sh` 抓取到
//! `third_party/controlcan/`) 访问 USBCAN-I/II、USBCAN-E-U/2E-U 等设备。
//!
//! ## 职责划分
//!
//! - **FFI 绑定层**: 常量、`repr(C)` 结构体与 13 个 `extern "system"` 函数声明
//!   (Windows 上 stdcall, 其余平台与 C 等价), 以及 `build.rs` 的加载模式配置。
//! - **`UsbVciBackend` 后端**: 在绑定之上实现 `can-types` 定义的后端 trait
//!   (经典 CAN 收发、轮询接收、互斥串行化与热插拔重连), 见
//!   [`UsbVciBackend`]。
//! - **`UsbVciDiscoverer` 设备发现**: 经 `VCI_FindUsbDevice2` 枚举当前接入的
//!   USBCAN 设备 (型号取自 `str_hw_Type`), 实现 `can-types` 的设备发现 trait,
//!   库未加载 / 无设备时返回空列表, 见 [`UsbVciDiscoverer`]。
//! - **mock 测试**: `mock` feature 下通过 `MockVciOps` 桩替换 FFI 调用,
//!   无需真实硬件即可验证后端行为。
//!
//! ## 加载模式 (build.rs + 运行时, Task 10 起)
//!
//! - **dynamic (默认)**: 构建期不链接任何 vendor 库, 运行时用 `libloading` 加载
//!   `libcontrolcan.so` (Linux) / `ControlCAN.dll` (Windows) 并解析全部 13 个符号。
//!   库路径解析顺序: `CAN_USBVCI_LIB` 环境变量 → 可执行文件同目录 → 系统搜索路径
//!   (Linux LD_LIBRARY_PATH / rpath; Windows exe 目录 / System32 / PATH)。
//!   库缺失返回 [`can_types::CanError::Io`] 友好错误, 绝不 panic。
//! - **static**: 设 `CAN_USBVCI_LINK_MODE=static` 后构建期链接 `libcontrolcan.a` +
//!   旧版 `libusb` (0.1 API) + `pthread`, 符号直接引用 extern 块 (不依赖运行时 .so),
//!   需要 libusb-dev。
//!
//! 供应商库缺失时默认构建不再失败 (动态加载把失败推迟到运行时并给出可操作提示);
//! `mock` feature 下跳过全部加载逻辑, 可在无供应商库 / 无硬件的主机上构建测试。
//!
//! ## FFI 约定
//!
//! 所有 VCI 函数均为 vendor 约定 ABI (`extern "system"`), 必须在 `unsafe` 块中调用;
//! 由调用方保证传入指针 (如 `*mut VCI_CAN_OBJ`) 的有效性与生存期。跨 FFI 边界使用
//! `repr(C)` 结构体。unsafe 仅限本 crate 的 FFI 调用层。

mod backend;
mod ffi;

pub use backend::{map_hw_type_to_device_type, UsbVciBackend, UsbVciDiscoverer};

pub use ffi::{
    VCI_ClearBuffer, VCI_CloseDevice, VCI_FindUsbDevice2, VCI_GetReceiveNum, VCI_InitCAN,
    VCI_OpenDevice, VCI_ReadBoardInfo, VCI_Receive, VCI_ResetCAN, VCI_SetReference, VCI_StartCAN,
    VCI_Transmit, VCI_UsbDeviceReset, STATUS_ERR, STATUS_OK, VCI_BOARD_INFO, VCI_CAN_OBJ,
    VCI_FILTER_RECORD, VCI_INIT_CONFIG, VCI_USBCAN1, VCI_USBCAN2, VCI_USBCAN2A, VCI_USBCAN_2E_U,
    VCI_USBCAN_E_U,
};
