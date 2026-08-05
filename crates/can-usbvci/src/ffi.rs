//! # ZLGCAN (周立功) USBCAN 系列 VCI FFI 绑定层
//!
//! 本模块手工编写 (不用 bindgen), 逐字对照供应商头文件
//! `Linux资料包V1.45/二次开发库文件/controlcan.h` (v2.02 20190609, 共 104 行, 13 个 EXTERN_C 函数)。
//!
//! ## C 类型映射约定 (Linux x86_64 / aarch64, LP64)
//!
//! | C 类型           | 宏定义            | Rust 类型     |
//! |------------------|-------------------|---------------|
//! | `DWORD`          | `unsigned int`    | `u32`         |
//! | `UINT` / `UINT32`| `unsigned int`    | `u32`         |
//! | `ULONG`          | `unsigned int`    | `u32`         |
//! | `USHORT`         | `unsigned short int` | `u16`      |
//! | `BYTE` / `UCHAR` | `unsigned char`   | `u8`          |
//! | `CHAR`           | `char`            | `i8`          |
//! | `INT`            | `int`             | `i32`         |
//! | `BOOL`           | `BYTE`            | `u8`          |
//! | `PVOID` / `LPVOID` | `void*`         | `*mut c_void` |
//!
//! > 注意: `ULONG` 在供应商头文件里被定义为 `unsigned int` (32 位), 并非 LP64 的 64 位 `unsigned long`,
//! > 因此 `VCI_GetReceiveNum` / `VCI_Transmit` / `VCI_Receive` 的返回值一律用 `u32`。

// 字段名刻意保留 C 头文件原样 (hw_Version/DataLen/Reserved 等), 保证与 C 侧逐字对应,
// 便于对照 controlcan.h 排查布局问题, 故禁止 snake_case lint。
#![allow(non_snake_case)]

use core::ffi::c_void;

// ---------------------------------------------------------------------------
// 接口卡类型定义 (controlcan.h L7-L12)
// ---------------------------------------------------------------------------

/// 接口卡类型: USBCAN-I
pub const VCI_USBCAN1: u32 = 3;
/// 接口卡类型: USBCAN-II
pub const VCI_USBCAN2: u32 = 4;
/// 接口卡类型: USBCAN-II A (与 USBCAN2 同值, 头文件 L9)
pub const VCI_USBCAN2A: u32 = 4;
/// 接口卡类型: USBCAN-E-U
pub const VCI_USBCAN_E_U: u32 = 20;
/// 接口卡类型: USBCAN-2E-U
pub const VCI_USBCAN_2E_U: u32 = 21;

// ---------------------------------------------------------------------------
// 函数调用返回状态值 (controlcan.h L15-L16)
// ---------------------------------------------------------------------------

/// 操作成功
pub const STATUS_OK: u8 = 1;
/// 操作失败
pub const STATUS_ERR: u8 = 0;

// ---------------------------------------------------------------------------
// 数据类型 (controlcan.h L34-L75)
// ---------------------------------------------------------------------------

/// ZLGCAN 系列接口卡信息 (对应 `_VCI_BOARD_INFO`)。
///
/// C 布局 (gcc, x86_64/aarch64): size=80, align=2。
/// 字段按 C 对齐规则排列, `Reserved` 因 2 字节对齐落在 offset 72 (而非 71)。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VCI_BOARD_INFO {
    /// 硬件版本号
    pub hw_Version: u16,
    /// 固件版本号
    pub fw_Version: u16,
    /// 驱动版本号
    pub dr_Version: u16,
    /// 接口版本号
    pub in_Version: u16,
    /// 中断号
    pub irq_Num: u16,
    /// CAN 通道数
    pub can_Num: u8,
    /// 设备序列号字符串 (20 字节, 非 NUL 终止安全)
    pub str_Serial_Num: [i8; 20],
    /// 硬件类型字符串 (40 字节, 非 NUL 终止安全)
    pub str_hw_Type: [i8; 40],
    /// 保留字段
    pub Reserved: [u16; 4],
}

/// CAN 信息帧 (对应 `_VCI_CAN_OBJ`)。
///
/// C 布局 (gcc, x86_64/aarch64): size=24, align=4。
/// `Data[8]` 从 offset 13 起 (C 标准允许非对齐数组), `Reserved[3]` 从 offset 21 起,
/// 末尾无对齐填充, 总尺寸恰为 24。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VCI_CAN_OBJ {
    /// 报文 ID
    pub ID: u32,
    /// 时间戳 (由驱动填写, 单位依驱动而定)
    pub TimeStamp: u32,
    /// 时间戳是否有效
    pub TimeFlag: u8,
    /// 发送标志 (0=正常发送)
    pub SendType: u8,
    /// 是否是远程帧
    pub RemoteFlag: u8,
    /// 是否是扩展帧
    pub ExternFlag: u8,
    /// 数据长度 DLC (0-8)
    pub DataLen: u8,
    /// CAN 帧数据
    pub Data: [u8; 8],
    /// 保留字段
    pub Reserved: [u8; 3],
}

/// CAN 初始化配置 (对应 `_INIT_CONFIG`)。
///
/// C 布局 (gcc, x86_64/aarch64): size=16, align=4。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VCI_INIT_CONFIG {
    /// 验收码
    pub AccCode: u32,
    /// 验收屏蔽码
    pub AccMask: u32,
    /// 保留字段
    pub Reserved: u32,
    /// 滤波方式 (0=接收所有帧, 1=只接收标准帧, 2=只接收扩展帧)
    pub Filter: u8,
    /// 定时器 0 (波特率配置)
    pub Timing0: u8,
    /// 定时器 1 (波特率配置)
    pub Timing1: u8,
    /// 工作模式 (0=正常, 1=只听, 2=自测)
    pub Mode: u8,
}

/// 滤波记录 (对应 `_VCI_FILTER_RECORD`, controlcan.h L71-L75)。
///
/// C 布局 (gcc, x86_64/aarch64): size=12, align=4。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VCI_FILTER_RECORD {
    /// 是否为扩展帧
    pub ExtFrame: u32,
    /// 起始 ID
    pub Start: u32,
    /// 结束 ID
    pub End: u32,
}

// ---------------------------------------------------------------------------
// VCI API (controlcan.h L83-L101, 共 13 个 EXTERN_C 函数)
// ---------------------------------------------------------------------------
//
// 所有函数均为 vendor 约定 ABI, 须在 unsafe 块中调用。设备句柄通过 (DeviceType, DeviceInd)
// 二元组索引, 无需预分配句柄对象。
//
// ABI 约定 (Task 10 起为 extern "system"): Windows 上是 stdcall (与供应商 ControlCAN.dll
// 一致, 符号名无前缀修饰); 非 Windows 平台上 system 与 C 等价。函数不直接调用 ——
// 默认 (动态加载) 模式下由 [`crate::backend`] 的 libloading 符号表按名解析, 这里保留
// 声明作为 ABI 契约与符号清单; 静态链接模式 (usbvci_static_link) 下才直接引用本块符号。

unsafe extern "system" {
    /// 打开设备。
    ///
    /// - `DeviceType`: 接口卡类型, 见 [`VCI_USBCAN1`] 等常量
    /// - `DeviceInd`: 设备索引号 (0 开始)
    /// - `Reserved`: 保留, 填 0
    ///
    /// 返回 [`STATUS_OK`] 成功, [`STATUS_ERR`] 失败。
    pub fn VCI_OpenDevice(DeviceType: u32, DeviceInd: u32, Reserved: u32) -> u32;

    /// 关闭设备。
    ///
    /// 返回 [`STATUS_OK`] 成功, [`STATUS_ERR`] 失败。
    pub fn VCI_CloseDevice(DeviceType: u32, DeviceInd: u32) -> u32;

    /// 初始化 CAN 通道。
    ///
    /// `pInitConfig` 指向 [`VCI_INIT_CONFIG`], 由调用方保证有效且生存期覆盖本调用。
    ///
    /// 返回 [`STATUS_OK`] 成功, [`STATUS_ERR`] 失败。
    pub fn VCI_InitCAN(
        DeviceType: u32,
        DeviceInd: u32,
        CANInd: u32,
        pInitConfig: *mut VCI_INIT_CONFIG,
    ) -> u32;

    /// 读取设备信息 (版本号、序列号、硬件类型)。
    ///
    /// `pInfo` 指向调用方提供的 [`VCI_BOARD_INFO`] 缓冲区。
    ///
    /// 返回 [`STATUS_OK`] 成功, [`STATUS_ERR`] 失败。
    pub fn VCI_ReadBoardInfo(DeviceType: u32, DeviceInd: u32, pInfo: *mut VCI_BOARD_INFO) -> u32;

    /// 设置参考量 (如滤波、单次发送超时等, 由 `RefType` 区分)。
    ///
    /// `pData` 指向按 `RefType` 约定解释的数据 (如 [`VCI_FILTER_RECORD`] 数组)。
    ///
    /// 返回 [`STATUS_OK`] 成功, [`STATUS_ERR`] 失败。
    pub fn VCI_SetReference(
        DeviceType: u32,
        DeviceInd: u32,
        CANInd: u32,
        RefType: u32,
        pData: *mut c_void,
    ) -> u32;

    /// 查询接收缓冲区中待读取的帧数。
    ///
    /// 返回缓冲区中的帧数 (可为 0)。
    pub fn VCI_GetReceiveNum(DeviceType: u32, DeviceInd: u32, CANInd: u32) -> u32;

    /// 清空指定通道的收发缓冲区。
    ///
    /// 返回 [`STATUS_OK`] 成功, [`STATUS_ERR`] 失败。
    pub fn VCI_ClearBuffer(DeviceType: u32, DeviceInd: u32, CANInd: u32) -> u32;

    /// 启动 CAN 通道通信。
    ///
    /// 返回 [`STATUS_OK`] 成功, [`STATUS_ERR`] 失败。
    pub fn VCI_StartCAN(DeviceType: u32, DeviceInd: u32, CANInd: u32) -> u32;

    /// 复位 CAN 通道。
    ///
    /// 返回 [`STATUS_OK`] 成功, [`STATUS_ERR`] 失败。
    pub fn VCI_ResetCAN(DeviceType: u32, DeviceInd: u32, CANInd: u32) -> u32;

    /// 发送 CAN 帧。
    ///
    /// `pSend` 指向 [`VCI_CAN_OBJ`] 数组, `Len` 为数组长度。
    /// 返回实际发送的帧数 (0 表示失败)。
    pub fn VCI_Transmit(
        DeviceType: u32,
        DeviceInd: u32,
        CANInd: u32,
        pSend: *mut VCI_CAN_OBJ,
        Len: u32,
    ) -> u32;

    /// 接收 CAN 帧。
    ///
    /// `pReceive` 指向调用方提供的 [`VCI_CAN_OBJ`] 缓冲区, `Len` 为缓冲区容量,
    /// `WaitTime` 为等待毫秒数 (负数=无限等待)。
    /// 返回实际接收的帧数 (0 表示超时或失败)。
    pub fn VCI_Receive(
        DeviceType: u32,
        DeviceInd: u32,
        CANInd: u32,
        pReceive: *mut VCI_CAN_OBJ,
        Len: u32,
        WaitTime: i32,
    ) -> u32;

    /// 复位 USB 设备。
    ///
    /// 返回 [`STATUS_OK`] 成功, [`STATUS_ERR`] 失败。
    pub fn VCI_UsbDeviceReset(DevType: u32, DevIndex: u32, Reserved: u32) -> u32;

    /// 查找 USB 设备。
    ///
    /// `pInfo` 指向 [`VCI_BOARD_INFO`] 数组 (容量至少为设备数)。
    /// 返回找到的 USB 设备数量。
    pub fn VCI_FindUsbDevice2(pInfo: *mut VCI_BOARD_INFO) -> u32;
}

// ---------------------------------------------------------------------------
// 布局测试
// ---------------------------------------------------------------------------
//
// 断言值来自 gcc 13 (x86_64) 对同一结构体的实测编译结果 (offsetof/sizeof/_Alignof),
// aarch64 (LP64) 布局一致。若某平台断言失败, 以实际 C 编译结果为准修正。

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    #[test]
    fn constants_match_header() {
        assert_eq!(VCI_USBCAN1, 3);
        assert_eq!(VCI_USBCAN2, 4);
        assert_eq!(VCI_USBCAN2A, 4);
        assert_eq!(VCI_USBCAN_E_U, 20);
        assert_eq!(VCI_USBCAN_2E_U, 21);
        assert_eq!(STATUS_OK, 1);
        assert_eq!(STATUS_ERR, 0);
    }

    #[test]
    fn vci_board_info_layout() {
        // gcc: size=80 align=2
        assert_eq!(size_of::<VCI_BOARD_INFO>(), 80);
        assert_eq!(align_of::<VCI_BOARD_INFO>(), 2);
        assert_eq!(offset_of!(VCI_BOARD_INFO, hw_Version), 0);
        assert_eq!(offset_of!(VCI_BOARD_INFO, fw_Version), 2);
        assert_eq!(offset_of!(VCI_BOARD_INFO, dr_Version), 4);
        assert_eq!(offset_of!(VCI_BOARD_INFO, in_Version), 6);
        assert_eq!(offset_of!(VCI_BOARD_INFO, irq_Num), 8);
        assert_eq!(offset_of!(VCI_BOARD_INFO, can_Num), 10);
        assert_eq!(offset_of!(VCI_BOARD_INFO, str_Serial_Num), 11);
        assert_eq!(offset_of!(VCI_BOARD_INFO, str_hw_Type), 31);
        // Reserved 为 [u16;4], 2 字节对齐 → offset 72 (非 71)
        assert_eq!(offset_of!(VCI_BOARD_INFO, Reserved), 72);
    }

    #[test]
    fn vci_can_obj_layout() {
        // gcc: size=24 align=4 (4+4+1+1+1+1+1+8+3, 无尾部填充)
        assert_eq!(size_of::<VCI_CAN_OBJ>(), 24);
        assert_eq!(align_of::<VCI_CAN_OBJ>(), 4);
        assert_eq!(offset_of!(VCI_CAN_OBJ, ID), 0);
        assert_eq!(offset_of!(VCI_CAN_OBJ, TimeStamp), 4);
        assert_eq!(offset_of!(VCI_CAN_OBJ, TimeFlag), 8);
        assert_eq!(offset_of!(VCI_CAN_OBJ, SendType), 9);
        assert_eq!(offset_of!(VCI_CAN_OBJ, RemoteFlag), 10);
        assert_eq!(offset_of!(VCI_CAN_OBJ, ExternFlag), 11);
        assert_eq!(offset_of!(VCI_CAN_OBJ, DataLen), 12);
        assert_eq!(offset_of!(VCI_CAN_OBJ, Data), 13);
        assert_eq!(offset_of!(VCI_CAN_OBJ, Reserved), 21);
    }

    #[test]
    fn vci_init_config_layout() {
        // gcc: size=16 align=4
        assert_eq!(size_of::<VCI_INIT_CONFIG>(), 16);
        assert_eq!(align_of::<VCI_INIT_CONFIG>(), 4);
        assert_eq!(offset_of!(VCI_INIT_CONFIG, AccCode), 0);
        assert_eq!(offset_of!(VCI_INIT_CONFIG, AccMask), 4);
        assert_eq!(offset_of!(VCI_INIT_CONFIG, Reserved), 8);
        assert_eq!(offset_of!(VCI_INIT_CONFIG, Filter), 12);
        assert_eq!(offset_of!(VCI_INIT_CONFIG, Timing0), 13);
        assert_eq!(offset_of!(VCI_INIT_CONFIG, Timing1), 14);
        assert_eq!(offset_of!(VCI_INIT_CONFIG, Mode), 15);
    }

    #[test]
    fn vci_filter_record_layout() {
        // gcc: size=12 align=4
        assert_eq!(size_of::<VCI_FILTER_RECORD>(), 12);
        assert_eq!(align_of::<VCI_FILTER_RECORD>(), 4);
        assert_eq!(offset_of!(VCI_FILTER_RECORD, ExtFrame), 0);
        assert_eq!(offset_of!(VCI_FILTER_RECORD, Start), 4);
        assert_eq!(offset_of!(VCI_FILTER_RECORD, End), 8);
    }
}
