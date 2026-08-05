//! # can-usbvci — ZLGCAN (周立功) USBCAN 系列 USB-CAN 适配器绑定
//!
//! 通过 VCI 动态库 ([`libcontrolcan`](https://www.zlg.cn/) 由 `scripts/fetch-vendor.sh` 抓取到
//! `third_party/controlcan/`) 访问 USBCAN-I/II、USBCAN-E-U/2E-U 等设备。
//!
//! ## 职责划分
//!
//! - **本 crate (Task 8)**: 仅提供 FFI 绑定层 —— 常量、`repr(C)` 结构体与 13 个
//!   `extern "C"` 函数声明, 以及 `build.rs` 双链接模式 (`.so` 默认 / `.a`+libusb 备选)。
//! - **`UsbVciBackend` 后端** (Task 11): 在绑定之上实现
//!   `can-types` 定义的后端 trait, 此处不实现。
//!
//! ## 链接模式 (build.rs)
//!
//! 由环境变量 `CAN_USBVCI_LINK_MODE` 控制, 取值:
//!
//! - `so` (默认): 链接 `libcontrolcan.so`, 内嵌 libusb-0.1 符号, 无需外部依赖;
//! - `static`: 链接 `libcontrolcan.a` + 旧版 `libusb` (0.1 API) + `pthread`, 需要 libusb-dev。
//!
//! 供应商库不存在时构建会失败并提示先运行 `scripts/fetch-vendor.sh`。
//!
//! ## FFI 约定
//!
//! 所有 VCI 函数均为 C ABI, 必须在 `unsafe` 块中调用; 由调用方保证传入指针
//! (如 `*mut VCI_CAN_OBJ`) 的有效性与生存期。跨 FFI 边界使用 `repr(C)` 结构体。

mod ffi;

pub use ffi::{
    VCI_USBCAN1, VCI_USBCAN2, VCI_USBCAN2A, VCI_USBCAN_E_U, VCI_USBCAN_2E_U,
    STATUS_ERR, STATUS_OK,
    VCI_BOARD_INFO, VCI_CAN_OBJ, VCI_FILTER_RECORD, VCI_INIT_CONFIG,
    VCI_ClearBuffer, VCI_CloseDevice, VCI_FindUsbDevice2, VCI_GetReceiveNum,
    VCI_InitCAN, VCI_OpenDevice, VCI_ReadBoardInfo, VCI_Receive, VCI_ResetCAN,
    VCI_SetReference, VCI_StartCAN, VCI_Transmit, VCI_UsbDeviceReset,
};
