//! # UsbVciBackend — USBCAN (VCI) 后端实现
//!
//! 在 [`crate::ffi`] 的 `extern "system"` 绑定之上实现 [`CanBackend`] trait, 提供
//! USBCAN-I/II、USBCAN-E-U/2E-U 等 ZLGCAN 设备的经典 CAN 收发能力。
//!
//! ## 设计要点
//!
//! - **动态加载** (Task 10): [`RealVciOps`] 默认经 libloading 运行时加载
//!   `libcontrolcan.so` (Linux) / `ControlCAN.dll` (Windows) 并解析全部 13 个 VCI
//!   符号, 构建期不再链接 `.so`; 库缺失返回 [`CanError::Io`] 友好错误而非 panic。
//!   `CAN_USBVCI_LINK_MODE=static` 时走静态链接 (符号直接取 extern 块)。
//! - **串行化**: vendor 库非线程安全, 所有 VCI 调用 (open/init/start/transmit/
//!   receive/get_rx_num/close) 一律在内部 `Mutex<()>` 临界区内执行。
//! - **轮询接收**: `read_frame` 按 vendor 轮询节奏 (30ms) 查询驱动接收缓冲,
//!   有帧则批量 `VCI_Receive` 拉取并缓冲, 无帧则睡眠后重试, 累计超时返回
//!   [`CanError::Timeout`]。
//! - **热插拔**: VCI 调用返回"设备消失"信号时关闭旧句柄、等待 ≥2s 设备重新枚举,
//!   重试重开; 超过次数上限返回 [`CanError::DeviceUnplugged`]。
//! - **时间戳**: 驱动的 `TimeStamp` 是自由计数器而非墙钟, 故用
//!   `SystemTime::now()` 填充 `CanFrame` 时间戳。
//!
//! ## 测试策略
//!
//! VCI 调用被抽象为内部 trait [`VciOps`]: 默认实现 [`RealVciOps`] 直调 FFI;
//! `mock` feature 下用 [`MockVciOps`] 注入内存模拟设备, 全部行为 (轮询/超时/
//! 并发/热插拔/帧转换) 用单元测试验证, 无需真实硬件。

use std::collections::VecDeque;
use std::ffi::CStr;
#[cfg(all(not(feature = "mock"), not(usbvci_static_link)))]
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use can_types::{
    BackendConfig, CanBackend, CanDeviceInfo, CanError, CanFrame, CanId, DeviceDetails,
    DeviceDiscoverer, DeviceKind, Result, MAX_EXTENDED_ID, MAX_STANDARD_ID,
};

#[cfg(not(feature = "mock"))]
use libloading::Library;

#[cfg(not(feature = "mock"))]
use core::ffi::c_void;

#[cfg(not(feature = "mock"))]
use crate::ffi::STATUS_OK;
use crate::ffi::{
    VCI_BOARD_INFO, VCI_CAN_OBJ, VCI_INIT_CONFIG, VCI_USBCAN1, VCI_USBCAN2, VCI_USBCAN_2E_U,
    VCI_USBCAN_E_U,
};

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// 轮询节奏: 驱动接收缓冲为空时每次等待的时长 (vendor 轮询节奏)。
///
/// 官方建议 "每隔 30ms 调用一次 VCI_Receive 为宜" (接口函数库说明书 2.2.10),
/// 保持 30ms。
const POLL_INTERVAL: Duration = Duration::from_millis(30);

/// 单次批量接收的最大帧数 (`VCI_Receive` 的 Len, 即 `pReceive` 数组长度)。
///
/// 官方建议 "一般 pReceive 数组大小与 Len 都设置大于 2000, 如: 2500 为宜"
/// (接口函数库说明书 2.2.10) —— 硬件每通道约 2000 帧接收缓存, 单次取够整批,
/// 避免忙时缓冲溢出。
const MAX_RX_BATCH: usize = 2500;

/// 发送帧的 `SendType`: 单次发送 (只发一次, 失败不自动重发)。
///
/// 官方建议 "二次开发, 建议 SendType=1, 提高发送的响应速度" (接口函数库说明书
/// 2.1.3); 0 = 正常发送 (失败自动重发, 重发时间 4 秒)。
const SEND_TYPE_SINGLE: u8 = 1;

/// 热插拔重连前等待设备重新枚举的最小时长 (≥2s)。
#[cfg(not(feature = "mock"))]
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// 热插拔重连的尝试次数上限。
#[cfg(not(feature = "mock"))]
const RECONNECT_ATTEMPTS: u32 = 5;

/// 500kbps 波特率定时器 0 (16MHz 晶振, ZLGCAN 标准波特率表)。
const TIMING0_500K: u8 = 0x00;

/// 500kbps 波特率定时器 1 (16MHz 晶振, ZLGCAN 标准波特率表)。
const TIMING1_500K: u8 = 0x1C;

/// 空 `VCI_CAN_OBJ` 常量, 用于初始化批量接收缓冲区。
const EMPTY_CAN_OBJ: VCI_CAN_OBJ = VCI_CAN_OBJ {
    ID: 0,
    TimeStamp: 0,
    TimeFlag: 0,
    SendType: 0,
    RemoteFlag: 0,
    ExternFlag: 0,
    DataLen: 0,
    Data: [0; 8],
    Reserved: [0; 3],
};

/// 空 `VCI_BOARD_INFO` 常量, 用于初始化设备枚举缓冲区
/// (`VCI_FindUsbDevice2` 由驱动按设备数回填, 调用方须先清零)。
const EMPTY_BOARD_INFO: VCI_BOARD_INFO = VCI_BOARD_INFO {
    hw_Version: 0,
    fw_Version: 0,
    dr_Version: 0,
    in_Version: 0,
    irq_Num: 0,
    can_Num: 0,
    str_Serial_Num: [0; 20],
    str_hw_Type: [0; 40],
    Reserved: [0; 4],
};

/// `VCI_FindUsbDevice2` 枚举缓冲容量 (栈上板卡信息数组长度)。
#[cfg(any(not(feature = "mock"), test))]
const MAX_BOARD_INFO_CAP: usize = 16;

// ---------------------------------------------------------------------------
// 设备类型映射
// ---------------------------------------------------------------------------

/// 将厂商板卡信息的硬件类型字符串映射为 VCI 设备类型码。
///
/// ZLGCAN 驱动要求 `VCI_OpenDevice` 传入与设备型号匹配的 `DeviceType`, 传错会
/// 打开失败甚至触发固件异常 (实测: USBCAN-2E_U 用 [`VCI_USBCAN2`] (4) 打开会出现
/// SET_CONFIGURATION busy + 固件看门狗崩溃重枚举)。`VCI_FindUsbDevice2` 返回的
/// `str_hw_Type` 是当前最可靠的型号来源, 据此推导设备类型。
///
/// **仅 discoverer/list 路径使用**: 后端 open 的探测不再调用 find (usbfs 接口
/// claim 自冲突, 见 [`UsbVciBackend::probe_and_open`]), 本映射用于设备发现时
/// 填充 [`CanDeviceInfo::device_type`]。
///
/// 映射规则 (顺序敏感, 先匹配先返回):
/// - 型号含 `"2E_U"` → [`VCI_USBCAN_2E_U`] (21)。
/// - 型号含 `"USBCAN2"` 或 `"CANalyst"` → [`VCI_USBCAN2`] (4)。
/// - 型号含 `"USBCAN1"` → [`VCI_USBCAN1`] (3)。
/// - 型号含 `"E_U"` → [`VCI_USBCAN_E_U`] (20) —— 须在 `"2E_U"` 分支之后判断
///   (2E_U 型号也含 `E_U`, 先判 `2E_U` 才能得到 21)。
/// - 其余型号 → [`VCI_USBCAN2`] (4) 默认。
///
/// 厂商文档通读结论 (v2-vendor-doc-review): 官方全部样例 (C/Python/QT/多卡) 均用
/// [`VCI_USBCAN2`] (4) 打开设备, 含 Linux 驱动下上报 hw_type=`"CAN-Linux"` 的
/// 2E_U 设备 —— 故 `"CAN-Linux"` 不再特判为 21, 落到默认 4; 仅型号显式含
/// `"2E_U"` 才映射 21。
///
/// @param hw_type 厂商 `str_hw_Type` 字段提取的硬件类型字符串 (如 `"CAN-Linux"`)。
/// @return 对应的 VCI 设备类型码。
pub fn map_hw_type_to_device_type(hw_type: &str) -> u32 {
    if hw_type.contains("2E_U") {
        VCI_USBCAN_2E_U
    } else if hw_type.contains("USBCAN2") || hw_type.contains("CANalyst") {
        VCI_USBCAN2
    } else if hw_type.contains("USBCAN1") {
        VCI_USBCAN1
    } else if hw_type.contains("E_U") {
        VCI_USBCAN_E_U
    } else {
        VCI_USBCAN2
    }
}

// ---------------------------------------------------------------------------
// VCI 调用抽象
// ---------------------------------------------------------------------------

/// VCI 底层调用抽象。
///
/// 将 [`UsbVciBackend`] 依赖的 7 个 VCI 操作封装为 trait 方法, 使上层逻辑
/// 可注入测试桩替换真实 FFI 调用。方法签名携带设备三元组
/// `(device_type, device_ind, channel)`, 由后端在调用时传入。
///
/// `Send + Sync`: 后端可能被放入 `Arc<Mutex<_>>` 跨线程共享, mock 实现内部
/// 也使用互斥状态, 故 trait 对象须可跨线程传递。
pub(crate) trait VciOps: Send + Sync {
    /// 打开设备。
    ///
    /// @param device_type 设备类型码。
    /// @param device_ind  设备索引 (0 起)。
    /// @return 成功返回 `Ok(())`; 设备不存在返回 [`CanError::NotFound`]。
    fn open(&self, device_type: u32, device_ind: u32) -> Result<()>;

    /// 关闭设备。
    ///
    /// @param device_type 设备类型码。
    /// @param device_ind  设备索引。
    /// @return 成功返回 `Ok(())` (设备可能已消失, 关闭失败也视为成功)。
    fn close(&self, device_type: u32, device_ind: u32) -> Result<()>;

    /// 初始化 CAN 通道。
    ///
    /// @param device_type 设备类型码。
    /// @param device_ind  设备索引。
    /// @param channel     通道号。
    /// @param config      初始化配置 (验收码 / 波特率 / 滤波 / 模式)。
    /// @return 成功返回 `Ok(())`; 失败返回 [`CanError::Protocol`]。
    fn init_can(
        &self,
        device_type: u32,
        device_ind: u32,
        channel: u32,
        config: &VCI_INIT_CONFIG,
    ) -> Result<()>;

    /// 启动 CAN 通道通信。
    ///
    /// @param device_type 设备类型码。
    /// @param device_ind  设备索引。
    /// @param channel     通道号。
    /// @return 成功返回 `Ok(())`; 失败返回 [`CanError::Protocol`]。
    fn start_can(&self, device_type: u32, device_ind: u32, channel: u32) -> Result<()>;

    /// 发送一帧。
    ///
    /// @param device_type 设备类型码。
    /// @param device_ind  设备索引。
    /// @param channel     通道号。
    /// @param objs        待发送的驱动帧数组。
    /// @return 实际发送的帧数 (0 表示失败, 由调用方判定)。
    fn transmit(
        &self,
        device_type: u32,
        device_ind: u32,
        channel: u32,
        objs: &mut [VCI_CAN_OBJ],
    ) -> Result<usize>;

    /// 从驱动接收缓冲批量取帧 (非阻塞)。
    ///
    /// @param device_type 设备类型码。
    /// @param device_ind  设备索引。
    /// @param channel     通道号。
    /// @param objs        驱动帧输出缓冲区, 长度即本次最多取帧数。
    /// @return 实际取到的帧数 (0 表示当前无帧)。
    fn receive(
        &self,
        device_type: u32,
        device_ind: u32,
        channel: u32,
        objs: &mut [VCI_CAN_OBJ],
    ) -> Result<usize>;

    /// 查询驱动接收缓冲中的待读帧数。
    ///
    /// @param device_type 设备类型码。
    /// @param device_ind  设备索引。
    /// @param channel     通道号。
    /// @return 待读帧数; 设备消失返回 [`CanError::DeviceUnplugged`]。
    fn get_rx_num(&self, device_type: u32, device_ind: u32, channel: u32) -> Result<usize>;

    /// 枚举当前接入的 USB 设备 (全局操作, 不区分设备三元组)。
    ///
    /// @param out 驱动回填的板卡信息缓冲区 (容量须足够容纳全部设备)。
    /// @return 找到的设备数量, 不会超过 `out.len()`; 无设备返回 0。
    #[allow(dead_code)]
    fn find_usb_devices(&self, out: &mut [VCI_BOARD_INFO]) -> u32;
}

/// 13 个 VCI 函数指针表 (符号表)。
///
/// 两种来源: 动态模式经 libloading 从已加载库按名解析; 静态链接模式
/// (build.rs 收到 `CAN_USBVCI_LINK_MODE=static` 时设 `usbvci_static_link` cfg,
/// 并链接 `libcontrolcan.a`) 直接取 [`crate::ffi`] extern 块函数项。
///
/// 尚未被 [`VciOps`] 消费的符号 (ReadBoardInfo/SetReference/ClearBuffer/ResetCAN/
/// UsbDeviceReset/FindUsbDevice2) 供 Task 12 设备发现等后续功能使用, 现由
/// `real-ffi` smoke test 验证可解析。
#[cfg(not(feature = "mock"))]
#[allow(dead_code)]
macro_rules! vci_symbols {
    ($( $field:ident => $fn:ident ( $($ty:ty),* $(,)? ) ),* $(,)?) => {
        #[allow(dead_code)]
        struct VciSymbols {
            $( $field: unsafe extern "system" fn($($ty),*) -> u32, )*
        }

        impl VciSymbols {
            /// 静态链接来源: 直接引用 extern 块函数项 (符号已在链接期解析)。
            #[cfg(usbvci_static_link)]
            fn from_extern() -> Self {
                Self {
                    $( $field: crate::ffi::$fn, )*
                }
            }

            /// 动态加载来源: 从已加载的动态库按符号名解析全部函数指针。
            ///
            /// # Safety
            /// `lib` 必须保持加载, 覆盖返回的符号表的使用期 (调用方持有 `Library` 句柄)。
            #[cfg(not(usbvci_static_link))]
            unsafe fn from_library(lib: &Library) -> Result<Self> {
                Ok(Self {
                    $( $field: *lib.get::<unsafe extern "system" fn($($ty),*) -> u32>(
                        concat!(stringify!($fn), "\0").as_bytes(),
                    )
                    .map_err(|e| {
                        CanError::Io(std::io::Error::other(format!(
                            "解析 VCI 符号 {} 失败: {e}; 供应商库可能版本不完整 \
                             (需 Linux 资料包 V1.45 的 libcontrolcan.so / ControlCAN.dll)",
                            stringify!($fn)
                        )))
                    })? ,)*
                })
            }
        }
    };
}

#[cfg(not(feature = "mock"))]
vci_symbols! {
    open_device => VCI_OpenDevice (u32, u32, u32),
    close_device => VCI_CloseDevice (u32, u32),
    init_can => VCI_InitCAN (u32, u32, u32, *mut VCI_INIT_CONFIG),
    read_board_info => VCI_ReadBoardInfo (u32, u32, *mut VCI_BOARD_INFO),
    set_reference => VCI_SetReference (u32, u32, u32, u32, *mut c_void),
    get_receive_num => VCI_GetReceiveNum (u32, u32, u32),
    clear_buffer => VCI_ClearBuffer (u32, u32, u32),
    start_can => VCI_StartCAN (u32, u32, u32),
    reset_can => VCI_ResetCAN (u32, u32, u32),
    transmit => VCI_Transmit (u32, u32, u32, *mut VCI_CAN_OBJ, u32),
    receive => VCI_Receive (u32, u32, u32, *mut VCI_CAN_OBJ, u32, i32),
    usb_device_reset => VCI_UsbDeviceReset (u32, u32, u32),
    find_usb_device2 => VCI_FindUsbDevice2 (*mut VCI_BOARD_INFO),
}

/// 真实 VCI 实现: 解析/加载动态库并解析全部 13 个符号后调用。
///
/// - 默认 (动态加载): [`RealVciOps::try_new`] 用 libloading 加载
///   `libcontrolcan.so` (Linux) / `ControlCAN.dll` (Windows) 并解析符号; 库或符号
///   缺失返回 [`CanError::Io`] 友好错误, 绝不 panic。
/// - 静态链接 (cfg `usbvci_static_link`, 见 build.rs): 符号直接取 extern 块函数项。
///
/// 仅在非 `mock` 构建下编译 —— `mock` 下编译会引用真实库符号导致链接失败。
#[cfg(not(feature = "mock"))]
pub(crate) struct RealVciOps {
    /// 13 个 VCI 函数指针 (调用统一走这里, 静态/动态来源一致)。
    syms: VciSymbols,
    /// 动态库句柄: 仅动态模式持有, 防止库被卸载后符号悬垂。
    #[allow(dead_code)]
    _library: Option<Library>,
}

#[cfg(not(feature = "mock"))]
impl RealVciOps {
    /// 构造真实 ops: 解析库路径 → 加载 → 解析 13 个符号。
    ///
    /// @return 成功返回可用 ops; 库/符号缺失返回 [`CanError::Io`] (带可操作提示), 不 panic。
    pub(crate) fn try_new() -> Result<Self> {
        #[cfg(usbvci_static_link)]
        {
            Ok(Self {
                syms: VciSymbols::from_extern(),
                _library: None,
            })
        }

        #[cfg(not(usbvci_static_link))]
        {
            let filename = resolve_library()?;
            let library = load_library(&filename)?;
            // SAFETY: `library` 句柄由本结构体 `_library` 字段持有, 存活期覆盖所有符号调用。
            let syms = unsafe { VciSymbols::from_library(&library)? };
            Ok(Self {
                syms,
                _library: Some(library),
            })
        }
    }
}

/// VCI 动态库文件名: Linux 为 `libcontrolcan.so`, Windows 为 `ControlCAN.dll`。
#[cfg(not(feature = "mock"))]
#[cfg(not(usbvci_static_link))]
fn library_filename() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "ControlCAN.dll"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "libcontrolcan.so"
    }
}

/// 解析 VCI 动态库路径, 优先级: `CAN_USBVCI_LIB` 环境变量 → 可执行文件同目录 →
/// Linux Tauri 打包资源目录 (见 find_tauri_resource_lib, 仅 Linux) → 系统搜索路径
/// (Linux 经 dlopen 的 LD_LIBRARY_PATH / rpath / ldconfig; Windows 经
/// LoadLibrary 的 exe 目录 / System32 / PATH)。
///
/// @return 最终交给 `Library::new` 的路径或文件名。
#[cfg(not(feature = "mock"))]
#[cfg(not(usbvci_static_link))]
fn resolve_library() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("CAN_USBVCI_LIB") {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(library_filename());
            if candidate.is_file() {
                return Ok(candidate);
            }
            #[cfg(target_os = "linux")]
            {
                // Tauri 打包回退: 发行包资源在 usr/lib/can-monitor/<arch>/, 相对 exe
                // 目录为 ../lib/can-monitor/<arch>/ (AppImage 挂载后与 deb 布局一致)。
                if let Some(resource) = find_tauri_resource_lib(dir) {
                    return Ok(resource);
                }
            }
        }
    }
    Ok(PathBuf::from(library_filename()))
}

/// 在 Tauri Linux 打包资源目录中定位供应商库。
///
/// 发行包 (AppImage / deb) 的目录布局: 可执行文件在 `usr/bin/`, 而 tauri.conf.json
/// 的 `resources` 把供应商库打进 `usr/lib/can-monitor/<arch>/libcontrolcan.so`
/// (AppImage 挂载后与 deb 安装布局相同)。dlopen 不会搜索该目录, 故按相对 exe 目录的
/// `../lib/can-monitor/<arch>/` 优先、平铺的 `../lib/can-monitor/` 兜底的顺序拼接候选
/// 路径, `is_file()` 命中即返回。
///
/// @param exe_dir 可执行文件所在目录 (通常为 `usr/bin`)。
/// @return 命中返回库文件路径; 未命中返回 `None`, 由调用方回退到 dlopen 系统搜索。
#[cfg(target_os = "linux")]
#[cfg(not(feature = "mock"))]
#[cfg(not(usbvci_static_link))]
fn find_tauri_resource_lib(exe_dir: &Path) -> Option<PathBuf> {
    // 编译期架构目录名, 与 tauri.conf.json 的 resources 映射键一致。
    let arch_dir = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let candidates = [
        format!("lib/can-monitor/{arch_dir}"),
        "lib/can-monitor".to_string(),
    ];
    for rel in &candidates {
        let candidate = exe_dir.join("..").join(rel).join(library_filename());
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 加载 VCI 动态库。
///
/// @param filename 路径或文件名 (文件名时按 OS 搜索路径解析)。
/// @return 成功返回库句柄; 加载失败返回 [`CanError::Io`] (带部署提示), 不 panic。
#[cfg(not(feature = "mock"))]
#[cfg(not(usbvci_static_link))]
fn load_library(filename: &Path) -> Result<Library> {
    // SAFETY: 库构造会执行其加载与初始化例程 (dlopen/LoadLibrary); 供应商库为普通 C 库,
    // 无需要调用方配合的初始化前置条件。
    unsafe { Library::new(filename) }.map_err(|e| {
        CanError::Io(std::io::Error::other(format!(
            "加载 VCI 动态库 {} 失败: {e}; 请确认供应商库已随二进制部署 \
             (Linux: libcontrolcan.so 经 LD_LIBRARY_PATH/rpath; Windows: ControlCAN.dll), \
             或设置 CAN_USBVCI_LIB 显式指定路径",
            filename.display()
        )))
    })
}

#[cfg(not(feature = "mock"))]
impl VciOps for RealVciOps {
    fn open(&self, device_type: u32, device_ind: u32) -> Result<()> {
        // SAFETY: 纯标量参数, 函数指针来自已加载库/静态链接, ABI 为 extern "system"。
        let status = unsafe { (self.syms.open_device)(device_type, device_ind, 0) };
        if status == u32::from(STATUS_OK) {
            Ok(())
        } else {
            Err(CanError::NotFound)
        }
    }

    fn close(&self, device_type: u32, device_ind: u32) -> Result<()> {
        // SAFETY: 纯标量参数。
        // 设备可能已被拔出, 关闭失败不可恢复, 忽略返回值。
        let _ = unsafe { (self.syms.close_device)(device_type, device_ind) };
        Ok(())
    }

    fn init_can(
        &self,
        device_type: u32,
        device_ind: u32,
        channel: u32,
        config: &VCI_INIT_CONFIG,
    ) -> Result<()> {
        let mut config = *config;
        // SAFETY: config 为栈上 `repr(C)` 结构体的可变拷贝, 指针在本次调用内有效。
        let status = unsafe {
            (self.syms.init_can)(device_type, device_ind, channel, &mut config as *mut _)
        };
        if status == u32::from(STATUS_OK) {
            Ok(())
        } else {
            Err(CanError::Protocol("VCI_InitCAN 失败"))
        }
    }

    fn start_can(&self, device_type: u32, device_ind: u32, channel: u32) -> Result<()> {
        // SAFETY: 纯标量参数。
        let status = unsafe { (self.syms.start_can)(device_type, device_ind, channel) };
        if status == u32::from(STATUS_OK) {
            Ok(())
        } else {
            Err(CanError::Protocol("VCI_StartCAN 失败"))
        }
    }

    fn transmit(
        &self,
        device_type: u32,
        device_ind: u32,
        channel: u32,
        objs: &mut [VCI_CAN_OBJ],
    ) -> Result<usize> {
        // SAFETY: objs 为调用方持有的可变切片, 指针与长度匹配, 本次调用内有效。
        let sent = unsafe {
            (self.syms.transmit)(
                device_type,
                device_ind,
                channel,
                objs.as_mut_ptr(),
                objs.len() as u32,
            )
        };
        Ok(sent as usize)
    }

    fn receive(
        &self,
        device_type: u32,
        device_ind: u32,
        channel: u32,
        objs: &mut [VCI_CAN_OBJ],
    ) -> Result<usize> {
        // SAFETY: objs 为调用方持有的可变切片, 指针与长度匹配; WaitTime=0 非阻塞。
        let got = unsafe {
            (self.syms.receive)(
                device_type,
                device_ind,
                channel,
                objs.as_mut_ptr(),
                objs.len() as u32,
                0,
            )
        };
        Ok(got as usize)
    }

    fn get_rx_num(&self, device_type: u32, device_ind: u32, channel: u32) -> Result<usize> {
        // SAFETY: 纯标量参数。
        let num = unsafe { (self.syms.get_receive_num)(device_type, device_ind, channel) };
        // 设备拔出时驱动返回 0xFFFFFFFF (已知 ZLGCAN 行为), 其余情况均为合法帧数。
        if num == u32::MAX {
            Err(CanError::DeviceUnplugged)
        } else {
            Ok(num as usize)
        }
    }

    fn find_usb_devices(&self, out: &mut [VCI_BOARD_INFO]) -> u32 {
        // SAFETY: out 为调用方持有的可变切片, 指针与长度匹配, 本次调用内有效;
        // VCI_FindUsbDevice2 按找到的设备数回填数组, 返回值截断到切片容量。
        unsafe { (self.syms.find_usb_device2)(out.as_mut_ptr()) }.min(out.len() as u32)
    }
}

// ---------------------------------------------------------------------------
// 后端
// ---------------------------------------------------------------------------

/// USBCAN (VCI) 经典 CAN 后端。
///
/// 通过 `VciOps` (VCI 调用抽象, 真实 FFI 或 mock 桩) 访问驱动, 支持轮询接收、
/// 互斥串行化与热插拔重连。
/// 仅支持经典 CAN (标准帧 / 扩展帧 / 远程帧), CANFD 帧会返回
/// [`CanError::Unsupported`]。
pub struct UsbVciBackend {
    /// 有效设备类型码 (探测成功后固定, 如 `VCI_USBCAN2`)。
    ///
    /// 由 [`CanBackend::open`] 经安全探测确定 (不做 find, 见 [`Self::probe_and_open`]):
    /// 按 [配置类型, USBCAN2, 2E_U] 去重后的顺序依次尝试 `VCI_OpenDevice`,
    /// 首个完整打开成功者即有效类型。构造时 (`new_with`) 暂存配置值, 后续
    /// init/start/transmit/receive/close 一律使用该字段而非配置值。
    device_type: u32,
    /// 设备索引号 (0 起, 区分同一类型的多台设备)。
    device_ind: u32,
    /// CAN 通道号 (0 / 1)。
    channel: u32,
    /// VCI 调用抽象 (真实 FFI 或 mock 桩)。
    ops: Box<dyn VciOps>,
    /// 串行化所有 VCI 调用的互斥锁 (vendor 库非线程安全)。
    vci_mutex: Mutex<()>,
    /// 已从驱动批量取回、尚未交给调用方的帧缓冲。
    rx_buffer: VecDeque<CanFrame>,
    /// 热插拔重连前等待设备重新枚举的时长。
    reconnect_delay: Duration,
    /// 热插拔重连尝试次数上限。
    reconnect_attempts: u32,
}

impl UsbVciBackend {
    /// 以指定 VCI 抽象与重连参数构造后端实例 (不执行打开)。
    ///
    /// 供 [`CanBackend::open`] 与 mock 测试共用; 测试可注入 [`MockVciOps`]
    /// 并通过缩小 `reconnect_delay` / `reconnect_attempts` 加速热插拔场景。
    ///
    /// @param device_type      设备类型码。
    /// @param device_index     设备索引 (0 起)。
    /// @param channel          通道号。
    /// @param ops              VCI 调用抽象。
    /// @param reconnect_delay  重连等待时长。
    /// @param reconnect_attempts 重连尝试次数。
    /// @return 已构造但未打开的后端实例。
    #[cfg(any(not(feature = "mock"), test))]
    pub(crate) fn new_with(
        device_type: u32,
        device_index: u32,
        channel: u32,
        ops: Box<dyn VciOps>,
        reconnect_delay: Duration,
        reconnect_attempts: u32,
    ) -> Self {
        Self {
            device_type,
            device_ind: device_index,
            channel,
            ops,
            vci_mutex: Mutex::new(()),
            rx_buffer: VecDeque::new(),
            reconnect_delay,
            reconnect_attempts,
        }
    }

    /// 获取 VCI 互斥锁, 中毒 (某线程持锁 panic) 时恢复而非传播 panic。
    ///
    /// 串行化要求高于失败性: 持锁线程崩溃后其他线程仍应能继续驱动访问。
    ///
    /// @return 锁住的 [`MutexGuard`]。
    fn lock_vci(&self) -> MutexGuard<'_, ()> {
        self.vci_mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 完整打开流程: `VCI_OpenDevice` → `VCI_InitCAN` (接收所有帧) → `VCI_StartCAN`。
    ///
    /// 每次 VCI 调用均持互斥锁。初始化为 500kbps、验收码 0 / 屏蔽码全 F
    /// (接收所有 ID)、滤波关闭、正常模式。
    ///
    /// @return 成功返回 `Ok(())`; 任一步失败返回对应 [`CanError`]。
    fn reopen(&self) -> Result<()> {
        self.open_with_type(self.device_type)
    }

    /// 以指定设备类型执行一次完整打开 (OpenDevice → InitCAN → StartCAN)。
    ///
    /// 供 [`Self::reopen`] (沿用有效类型重连) 与 [`Self::probe_and_open`]
    /// (逐候选探测) 共用。失败的探测类型**不做 close** —— 对脆弱固件 (如 2E_U
    /// 用错类型 4 打开会触发 SET_CONFIGURATION busy + 看门狗崩溃重枚举), 错误
    /// 类型上的任何后续 VCI 调用都可能再次引发固件崩溃, 故失败即弃, 不再 touch
    /// 该类型。
    ///
    /// @param device_type 本次尝试的设备类型码。
    /// @return 成功返回 `Ok(())` (设备保持打开并完成初始化)。
    fn open_with_type(&self, device_type: u32) -> Result<()> {
        {
            let _guard = self.lock_vci();
            self.ops.open(device_type, self.device_ind)?;
        }
        let init = VCI_INIT_CONFIG {
            AccCode: 0,
            AccMask: 0xFFFF_FFFF,
            Reserved: 0,
            Filter: 0,
            Timing0: TIMING0_500K,
            Timing1: TIMING1_500K,
            Mode: 0,
        };
        {
            let _guard = self.lock_vci();
            self.ops
                .init_can(device_type, self.device_ind, self.channel, &init)?;
        }
        {
            let _guard = self.lock_vci();
            self.ops
                .start_can(device_type, self.device_ind, self.channel)?;
        }
        Ok(())
    }

    /// 按安全探测顺序尝试打开设备并完成初始化, 确定有效设备类型。
    ///
    /// **不做 find**: 候选顺序 (去重后) = 配置类型 → [`VCI_USBCAN2`] →
    /// [`VCI_USBCAN_2E_U`]。第一个完整打开成功的候选即写入 `self.device_type` 并
    /// 返回; 全部失败返回 [`CanError::NotFound`]。
    ///
    /// 不再调用 `VCI_FindUsbDevice2` 的原因: find 会在本进程占用设备接口 0 且
    /// 不释放, 紧随其后的 `VCI_OpenDevice` (内部 SET_CONFIGURATION) 因接口已被
    /// **自身**占用而报 "Device or resource busy" (内核日志 `usbfs: interface 0
    /// claimed by usbfs while ... sets config #1`) → open 失败。型号→类型映射
    /// (见 [`map_hw_type_to_device_type`]) 已仅保留在 discoverer/list 路径,
    /// open 只按固定候选顺序探测。
    ///
    /// 探测过程中失败的候选类型一律不做 close (见 [`Self::open_with_type`])。
    ///
    /// @return 成功返回 `Ok(())`; 全部候选失败返回 [`CanError::NotFound`]。
    #[cfg(any(not(feature = "mock"), test))]
    fn probe_and_open(&mut self) -> Result<()> {
        for candidate in self.probe_candidates() {
            if self.open_with_type(candidate).is_ok() {
                self.device_type = candidate;
                return Ok(());
            }
        }
        Err(CanError::NotFound)
    }

    /// 生成安全探测的候选设备类型列表 (已去重, 保持首次出现顺序)。
    ///
    /// 顺序: 配置类型 (0 视为"未指定/自动", 跳过) → USBCAN2 → 2E_U。
    /// 官方样例全部用 [`VCI_USBCAN2`] (4) 打开设备 (含 hw_type="CAN-Linux" 的
    /// 2E_U), 故默认 4 先于 2E_U (21); 显式指定 2E_U 时配置类型仍最先尝试。
    /// 同一类型只出现一次, 避免对脆弱固件重复尝试。
    ///
    /// @return 去重后的候选类型列表。
    #[cfg(any(not(feature = "mock"), test))]
    fn probe_candidates(&self) -> Vec<u32> {
        let mut candidates = Vec::with_capacity(3);
        if self.device_type != 0 {
            candidates.push(self.device_type);
        }
        candidates.push(VCI_USBCAN2);
        candidates.push(VCI_USBCAN_2E_U);
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|&t| seen.insert(t));
        candidates
    }

    /// 在互斥锁内关闭设备。
    ///
    /// @return 成功返回 `Ok(())` (关闭失败同样视为成功, 见 [`VciOps::close`])。
    fn close_device(&self) -> Result<()> {
        let _guard = self.lock_vci();
        self.ops.close(self.device_type, self.device_ind)
    }

    /// 处理热插拔: 关闭旧句柄, 等待设备重新枚举, 重试重开。
    ///
    /// 先忽略结果关闭旧句柄 (设备可能已消失), 随后循环 `reconnect_attempts` 次:
    /// 每次先等待 `reconnect_delay` (真实配置 ≥2s) 再执行 [`Self::reopen`]。
    ///
    /// @return 重连成功返回 `Ok(())`; 超过次数上限返回 [`CanError::DeviceUnplugged`]。
    fn reconnect(&self) -> Result<()> {
        let _ = self.close_device();
        for _ in 0..self.reconnect_attempts {
            thread::sleep(self.reconnect_delay);
            if self.reopen().is_ok() {
                return Ok(());
            }
        }
        Err(CanError::DeviceUnplugged)
    }
}

impl CanBackend for UsbVciBackend {
    /// 按配置打开 USBCAN 后端。
    ///
    /// **不做 find** (见 [`Self::probe_and_open`]: find 的 usbfs 接口 claim 与
    /// OpenDevice 的 SET_CONFIGURATION 同进程自冲突): 按 [配置类型, USBCAN2, 2E_U]
    /// 去重后的顺序依次尝试 `VCI_OpenDevice` —— 第一个完整打开成功的类型即有效
    /// 设备类型, 后续 init/start/transmit/receive/close 全部使用它。失败的探测
    /// 类型不做 close (防止脆弱固件二次崩溃), 绝不 panic。
    ///
    /// @param config 后端配置, 仅支持 [`BackendConfig::UsbVci`] (device_type
    ///               仅为探测候选之一, 可为 0 表示自动跳过)。
    /// @return 成功返回已打开的 [`UsbVciBackend`]; 配置不是 UsbVci 返回
    ///         [`CanError::Unsupported`], 打开/初始化失败返回对应 [`CanError`]。
    fn open(config: &BackendConfig) -> Result<Self> {
        let (device_type, device_index, channel) = match config {
            BackendConfig::UsbVci {
                device_type,
                device_index,
                channel,
            } => (*device_type, *device_index, *channel),
            BackendConfig::SocketCan { .. } => {
                return Err(CanError::Unsupported("SocketCAN 配置不适用于 USBCAN 后端"));
            }
            BackendConfig::None => return Err(CanError::Unsupported("未配置后端 (None)")),
        };

        #[cfg(not(feature = "mock"))]
        {
            // 运行时加载 VCI 动态库并解析全部符号; 库缺失返回友好错误, 不 panic。
            let ops = RealVciOps::try_new()?;
            let mut backend = Self::new_with(
                device_type,
                device_index,
                channel,
                Box::new(ops),
                RECONNECT_DELAY,
                RECONNECT_ATTEMPTS,
            );
            // 安全探测 (无 find): 配置类型 → USBCAN2 → 2E_U, 首成功即有效类型。
            backend.probe_and_open()?;
            Ok(backend)
        }
        #[cfg(feature = "mock")]
        {
            let _ = (device_type, device_index, channel);
            Err(CanError::Unsupported(
                "mock feature 下不可打开真实 USBCAN 设备 (测试请经 new_with 注入 MockVciOps)",
            ))
        }
    }

    /// 从总线读取一帧, 支持超时与热插拔重连。
    ///
    /// 优先返回内部缓冲帧 (批量取回); 无缓冲时轮询驱动接收队列, 设备拔出
    /// ([`CanError::DeviceUnplugged`]) 自动重连, 累计超过 `timeout` 返回
    /// [`CanError::Timeout`]。
    ///
    /// @param timeout 阻塞等待一帧的最长时间。
    /// @return 成功返回收到的 [`CanFrame`]; 超时返回 [`CanError::Timeout`]。
    fn read_frame(&mut self, timeout: Duration) -> Result<CanFrame> {
        let mut deadline = Instant::now() + timeout;
        loop {
            // 1) 优先返回已缓冲的帧 (不触碰 VCI, 无需持锁)。
            if let Some(frame) = self.rx_buffer.pop_front() {
                return Ok(frame);
            }

            // 2) 查询驱动接收缓冲 (VCI 调用, 须持互斥锁)。
            let rx_num = {
                let _guard = self.lock_vci();
                self.ops
                    .get_rx_num(self.device_type, self.device_ind, self.channel)
            };
            match rx_num {
                Err(CanError::DeviceUnplugged) => {
                    // 设备消失 → 关闭旧句柄并重试重开; 重连成功则重置超时窗口继续。
                    if self.reconnect().is_err() {
                        return Err(CanError::DeviceUnplugged);
                    }
                    deadline = Instant::now() + timeout;
                    continue;
                }
                Err(err) => return Err(err),
                Ok(0) => {}
                Ok(num) => {
                    // 批量取帧填入驱动帧缓冲区 (VCI 调用, 持锁)。
                    let batch = num.min(MAX_RX_BATCH);
                    let mut objs = [EMPTY_CAN_OBJ; MAX_RX_BATCH];
                    let count = {
                        let _guard = self.lock_vci();
                        self.ops.receive(
                            self.device_type,
                            self.device_ind,
                            self.channel,
                            &mut objs[..batch],
                        )
                    };
                    let count = match count {
                        Err(CanError::DeviceUnplugged) => {
                            if self.reconnect().is_err() {
                                return Err(CanError::DeviceUnplugged);
                            }
                            deadline = Instant::now() + timeout;
                            continue;
                        }
                        Err(err) => return Err(err),
                        Ok(count) => count,
                    };
                    // 驱动帧 → 协议无关帧, 时间戳用墙钟填充 (驱动 TimeStamp 非墙钟)。
                    for obj in &objs[..count] {
                        let mut frame = vci_obj_to_frame(obj)?;
                        frame.set_timestamp(SystemTime::now());
                        self.rx_buffer.push_back(frame);
                    }
                    // 已填缓冲, 下一轮直接从缓冲取帧返回。
                    continue;
                }
            }

            // 3) 超时检查与轮询睡眠 (30ms, 不超过剩余超时)。
            let now = Instant::now();
            if now >= deadline {
                return Err(CanError::Timeout);
            }
            thread::sleep(deadline.saturating_duration_since(now).min(POLL_INTERVAL));
        }
    }

    /// 向总线写入一帧。
    ///
    /// @param frame 待发送的帧。
    /// @return 成功返回 `Ok(())`; CANFD 帧返回 [`CanError::Unsupported`],
    ///         驱动未接收返回 [`CanError::Protocol`], 底层失败返回对应 [`CanError`]。
    fn write_frame(&mut self, frame: &CanFrame) -> Result<()> {
        let mut obj = frame_to_vci_obj(frame)?;
        let sent = {
            let _guard = self.lock_vci();
            self.ops.transmit(
                self.device_type,
                self.device_ind,
                self.channel,
                std::slice::from_mut(&mut obj),
            )?
        };
        if sent == 0 {
            Err(CanError::Protocol("VCI_Transmit 未发送任何帧"))
        } else {
            Ok(())
        }
    }

    /// 关闭后端并释放设备句柄。
    ///
    /// @return 成功返回 `Ok(())` (关闭失败同样视为成功, 见 `close_device`)。
    fn close(&mut self) -> Result<()> {
        self.close_device()
    }
}

// ---------------------------------------------------------------------------
// 帧转换
// ---------------------------------------------------------------------------

/// 将驱动帧转换为协议无关帧。
///
/// `ExternFlag` → 扩展/标准 ID, `RemoteFlag` → 远程帧, `DataLen`/`Data` → 数据区
/// (`DataLen` 超 8 时按 8 截断, 防御驱动异常)。时间戳由调用方另行填充。
///
/// @param obj 驱动返回的 `VCI_CAN_OBJ`。
/// @return 转换后的 [`CanFrame`]; 非法 ID 返回 [`CanError::Protocol`]。
fn vci_obj_to_frame(obj: &VCI_CAN_OBJ) -> Result<CanFrame> {
    // 先按位宽范围检查, 再构造 CanId — 避免 `as u16` 截断在检查之前发生
    // (如 0x10000 会静默截断为 0x000)。
    let id = if obj.ExternFlag != 0 {
        if obj.ID > MAX_EXTENDED_ID {
            return Err(CanError::Protocol("VCI 返回非法扩展 CAN ID"));
        }
        CanId::new_extended(obj.ID).map_err(|_| CanError::Protocol("VCI 返回非法扩展 CAN ID"))?
    } else {
        if obj.ID > MAX_STANDARD_ID {
            return Err(CanError::Protocol("VCI 返回非法标准 CAN ID"));
        }
        CanId::new_standard(obj.ID as u16)
            .map_err(|_| CanError::Protocol("VCI 返回非法标准 CAN ID"))?
    };

    let data_len = (obj.DataLen as usize).min(obj.Data.len());
    let mut frame = CanFrame::new(id, obj.Data[..data_len].to_vec())?;
    if obj.RemoteFlag != 0 {
        frame.set_remote(true);
    }
    Ok(frame)
}

/// 将协议无关帧转换为驱动帧。
///
/// 发送帧 `SendType` 固定为 [`SEND_TYPE_SINGLE`] (1, 单次发送) —— 官方二次开发
/// 建议 (接口函数库说明书 2.1.3), 避免失败帧占用 4 秒自动重发窗口。
///
/// @param frame 待发送的 [`CanFrame`]。
/// @return 填充好的 `VCI_CAN_OBJ`; CANFD 帧返回 [`CanError::Unsupported`]。
fn frame_to_vci_obj(frame: &CanFrame) -> Result<VCI_CAN_OBJ> {
    if frame.is_fd() {
        return Err(CanError::Unsupported("CANFD 帧: USBCAN 硬件仅支持经典 CAN"));
    }
    let mut obj = EMPTY_CAN_OBJ;
    obj.SendType = SEND_TYPE_SINGLE;
    obj.ID = frame.id().raw_id();
    obj.ExternFlag = u8::from(frame.id().is_extended());
    obj.RemoteFlag = u8::from(frame.is_remote());
    obj.DataLen = frame.len() as u8;
    obj.Data[..frame.len()].copy_from_slice(frame.data());
    Ok(obj)
}

// ---------------------------------------------------------------------------
// 设备发现 (VCI_FindUsbDevice2 枚举)
// ---------------------------------------------------------------------------

/// USBCAN (VCI) 设备发现器。
///
/// 通过 VCI 动态库的 `VCI_FindUsbDevice2` 枚举当前接入的 USBCAN 设备
/// (USBCAN-I/II、USBCAN-E-U/2E-U 等), 构造协议无关的 [`CanDeviceInfo`] 列表,
/// 型号取自板卡信息的 `str_hw_Type` 字段。库未加载 / 无设备时返回空列表,
/// 绝不 panic。
pub struct UsbVciDiscoverer;

impl DeviceDiscoverer for UsbVciDiscoverer {
    /// 枚举当前接入的 USBCAN 设备。
    ///
    /// 真实构建经 `RealVciOps` 动态加载 VCI 库并调用 `VCI_FindUsbDevice2`;
    /// mock feature 下返回 2 台模拟设备 (与 `MockVciOps::find_usb_devices`
    /// 数据源一致), 供聚合层 (can-devices) 无硬件测试。
    ///
    /// @return 设备列表; 库未加载 / 无设备时返回空列表, 不 panic。
    fn list_devices() -> Vec<CanDeviceInfo> {
        #[cfg(feature = "mock")]
        {
            mock_list_devices()
        }
        #[cfg(not(feature = "mock"))]
        {
            real_list_devices()
        }
    }
}

/// 真实枚举: 加载 VCI 库 (失败返回空列表, 不 panic) 后调用 `VCI_FindUsbDevice2`。
///
/// @return 设备列表; 库缺失 / 符号缺失 / 无设备均返回空列表。
#[cfg(not(feature = "mock"))]
fn real_list_devices() -> Vec<CanDeviceInfo> {
    let Ok(ops) = RealVciOps::try_new() else {
        return Vec::new();
    };
    list_devices_with(&ops)
}

/// mock 枚举: 返回 2 台模拟设备 (数据源与 `MockVciOps::find_usb_devices` 相同)。
///
/// @return 2 台模拟设备的列表。
#[cfg(feature = "mock")]
fn mock_list_devices() -> Vec<CanDeviceInfo> {
    let mock = [mock_board_info("USBCAN-II"), mock_board_info("USBCAN-E-U")];
    mock.iter()
        .enumerate()
        .map(|(index, info)| board_info_to_device_info(info, index))
        .collect()
}

/// 经任意 [`VciOps`] 抽象执行设备枚举 (真实 FFI 或 mock 桩均可注入)。
///
/// 栈上分配 [`MAX_BOARD_INFO_CAP`] 个板卡信息槽位, 调用 `VCI_FindUsbDevice2`
/// 按返回值截断, 无设备时返回空列表 (不 panic)。
///
/// @param ops 设备枚举用的 VCI 调用抽象。
/// @return 设备列表。
#[cfg(any(not(feature = "mock"), test))]
fn list_devices_with(ops: &dyn VciOps) -> Vec<CanDeviceInfo> {
    let mut infos = [EMPTY_BOARD_INFO; MAX_BOARD_INFO_CAP];
    let count = ops.find_usb_devices(&mut infos) as usize;
    let count = count.min(infos.len());
    infos[..count]
        .iter()
        .enumerate()
        .map(|(index, info)| board_info_to_device_info(info, index))
        .collect()
}

/// 驱动板卡信息 → 协议无关设备信息。
///
/// @param info  驱动回填的板卡信息。
/// @param index 设备索引 (0 起, 即设备在枚举结果中的序号)。
/// @return 填好的 [`CanDeviceInfo`]。
fn board_info_to_device_info(info: &VCI_BOARD_INFO, index: usize) -> CanDeviceInfo {
    let model = board_info_model(info);
    CanDeviceInfo {
        id: index.to_string(),
        name: model.clone(),
        kind: DeviceKind::UsbVci,
        driver: "usbvci".to_string(),
        details: DeviceDetails::with_model(model.clone()),
        available: true,
        device_type: map_hw_type_to_device_type(&model),
    }
}

/// 从板卡信息 `str_hw_Type` 提取型号字符串。
///
/// C 字符数组以 NUL 结尾: 先用 `CStr::from_bytes_until_nul` 截断到首个 NUL;
/// 数组填满无 NUL 或含非法 UTF-8 时 lossy 转换, 绝不 panic。
///
/// @param info 驱动回填的板卡信息。
/// @return 型号文本 (空型号返回空字符串)。
fn board_info_model(info: &VCI_BOARD_INFO) -> String {
    let raw: Vec<u8> = info.str_hw_Type.iter().map(|&b| b as u8).collect();
    match CStr::from_bytes_until_nul(&raw) {
        Ok(cstr) => cstr.to_string_lossy().into_owned(),
        Err(_) => String::from_utf8_lossy(&raw).into_owned(),
    }
}

// ---------------------------------------------------------------------------
// Mock 设备 (仅 test + feature = "mock", 避免非测试 mock 构建产生未使用告警)
// ---------------------------------------------------------------------------

/// 内存模拟的 USBCAN 设备状态。
///
/// 维护驱动接收队列与在线标志, 记录打开次数与最近一次批量接收容量,
/// 供测试断言轮询、批量与热插拔行为。
#[cfg(all(test, feature = "mock"))]
#[derive(Debug, Default)]
pub(crate) struct MockDevice {
    /// 驱动接收队列 (FIFO)。
    pub(crate) rx_queue: VecDeque<VCI_CAN_OBJ>,
    /// 设备在线标志 (`false` 模拟拔出)。
    pub(crate) online: bool,
    /// 累计打开次数 (断言重连触发)。
    pub(crate) open_count: u32,
    /// 最近一次 `receive` 请求的容量 (断言批量取帧)。
    pub(crate) last_receive_cap: usize,
    /// `VCI_FindUsbDevice2` 回填的板卡信息数据源 (断言探测首选类型)。
    pub(crate) board_infos: Vec<VCI_BOARD_INFO>,
    /// 每次 open 尝试的设备类型 (断言探测顺序)。
    pub(crate) open_history: Vec<u32>,
    /// 每次 close 调用的设备类型 (断言失败候选不 close)。
    pub(crate) close_history: Vec<u32>,
    /// 指定该设备类型的 open 恒失败 (返回 NotFound); None 表示全部成功。
    pub(crate) fail_open_for: Option<u32>,
}

#[cfg(all(test, feature = "mock"))]
impl MockDevice {
    /// 构造一个初始在线的模拟设备。
    ///
    /// @return 在线、空队列、零打开次数, 枚举默认 2 台设备的 [`MockDevice`]。
    pub(crate) fn new() -> Self {
        Self {
            online: true,
            board_infos: vec![mock_board_info("USBCAN-II"), mock_board_info("USBCAN-E-U")],
            ..Self::default()
        }
    }
}

/// 构造一台模拟板卡信息 (仅型号字段填入, 其余清零)。
///
/// mock 模式下的 `VCI_FindUsbDevice2` 回填结果, 与 [`MockVciOps::find_usb_devices`]
/// 及 `UsbVciDiscoverer::list_devices` 的 mock 分支共用同一数据源。
#[cfg(feature = "mock")]
fn mock_board_info(model: &str) -> VCI_BOARD_INFO {
    let mut info = EMPTY_BOARD_INFO;
    info.can_Num = 2;
    for (dst, src) in info.str_hw_Type.iter_mut().zip(model.as_bytes()) {
        *dst = *src as i8;
    }
    info
}

/// VCI 调用 mock 实现: 读写内存模拟设备, 不触碰任何 FFI 符号。
///
/// 在线时按队列语义返回帧数; 离线时 [`VciOps::get_rx_num`] 等返回
/// [`CanError::DeviceUnplugged`], 模拟驱动上报设备消失。
#[cfg(all(test, feature = "mock"))]
#[derive(Debug, Clone)]
pub(crate) struct MockVciOps {
    /// 共享设备状态 (mock 需跨调用 / 跨线程变更)。
    pub(crate) state: std::sync::Arc<Mutex<MockDevice>>,
}

#[cfg(all(test, feature = "mock"))]
impl VciOps for MockVciOps {
    fn open(&self, device_type: u32, _device_ind: u32) -> Result<()> {
        let mut device = self.state.lock().expect("mock 状态锁中毒");
        device.open_history.push(device_type);
        if !device.online {
            return Err(CanError::NotFound);
        }
        if device.fail_open_for == Some(device_type) {
            return Err(CanError::NotFound);
        }
        device.open_count += 1;
        Ok(())
    }

    fn close(&self, device_type: u32, _device_ind: u32) -> Result<()> {
        self.state
            .lock()
            .expect("mock 状态锁中毒")
            .close_history
            .push(device_type);
        Ok(())
    }

    fn init_can(
        &self,
        _device_type: u32,
        _device_ind: u32,
        _channel: u32,
        _config: &VCI_INIT_CONFIG,
    ) -> Result<()> {
        if self.state.lock().expect("mock 状态锁中毒").online {
            Ok(())
        } else {
            Err(CanError::NotFound)
        }
    }

    fn start_can(&self, _device_type: u32, _device_ind: u32, _channel: u32) -> Result<()> {
        if self.state.lock().expect("mock 状态锁中毒").online {
            Ok(())
        } else {
            Err(CanError::NotFound)
        }
    }

    fn transmit(
        &self,
        _device_type: u32,
        _device_ind: u32,
        _channel: u32,
        objs: &mut [VCI_CAN_OBJ],
    ) -> Result<usize> {
        let device = self.state.lock().expect("mock 状态锁中毒");
        if !device.online {
            return Err(CanError::DeviceUnplugged);
        }
        Ok(objs.len())
    }

    fn receive(
        &self,
        _device_type: u32,
        _device_ind: u32,
        _channel: u32,
        objs: &mut [VCI_CAN_OBJ],
    ) -> Result<usize> {
        let mut device = self.state.lock().expect("mock 状态锁中毒");
        if !device.online {
            return Err(CanError::DeviceUnplugged);
        }
        device.last_receive_cap = objs.len();
        let count = device.rx_queue.len().min(objs.len());
        for (dst, src) in objs.iter_mut().zip(device.rx_queue.drain(..count)) {
            *dst = src;
        }
        Ok(count)
    }

    fn get_rx_num(&self, _device_type: u32, _device_ind: u32, _channel: u32) -> Result<usize> {
        let device = self.state.lock().expect("mock 状态锁中毒");
        if !device.online {
            Err(CanError::DeviceUnplugged)
        } else {
            Ok(device.rx_queue.len())
        }
    }

    fn find_usb_devices(&self, out: &mut [VCI_BOARD_INFO]) -> u32 {
        let device = self.state.lock().expect("mock 状态锁中毒");
        let count = device.board_infos.len().min(out.len());
        out[..count].copy_from_slice(&device.board_infos[..count]);
        count as u32
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

/// 纯帧转换测试 (无需 mock feature, 不依赖设备)。
#[cfg(test)]
mod conversion_tests {
    use super::*;

    fn frame(id: u32, ext: bool, data: &[u8]) -> VCI_CAN_OBJ {
        let mut obj = EMPTY_CAN_OBJ;
        obj.ID = id;
        obj.ExternFlag = u8::from(ext);
        obj.DataLen = data.len() as u8;
        obj.Data[..data.len()].copy_from_slice(data);
        obj
    }

    /// 标准帧: ExternFlag=0 → 标准 ID, 数据按 DataLen 截取。
    #[test]
    fn vci_obj_to_frame_standard() {
        let obj = frame(0x123, false, &[1, 2, 3]);
        let f = vci_obj_to_frame(&obj).unwrap();
        assert_eq!(f.id(), CanId::new_standard(0x123).unwrap());
        assert!(f.id().is_standard());
        assert_eq!(f.data(), &[1, 2, 3]);
        assert!(!f.is_remote());
    }

    /// 扩展帧: ExternFlag=1 → 扩展 ID (0x1FFFFF 正常解析)。
    #[test]
    fn vci_obj_to_frame_extended() {
        let obj = frame(0x1FF_FFFF, true, &[]);
        let f = vci_obj_to_frame(&obj).unwrap();
        assert_eq!(f.id(), CanId::new_extended(0x1FF_FFFF).unwrap());
        assert!(f.id().is_extended());
        assert!(f.is_empty());
    }

    /// 远程帧: RemoteFlag=1 → 远程标志置位。
    #[test]
    fn vci_obj_to_frame_remote() {
        let mut obj = frame(0x456, false, &[0xAA]);
        obj.RemoteFlag = 1;
        let f = vci_obj_to_frame(&obj).unwrap();
        assert!(f.is_remote());
    }

    /// DataLen 越界 (9) 防御性截断为 8 字节, 不 panic。
    #[test]
    fn vci_obj_to_frame_overlong_data_capped() {
        let mut obj = frame(0x100, false, &[0u8; 8]);
        obj.DataLen = 9;
        let f = vci_obj_to_frame(&obj).unwrap();
        assert_eq!(f.len(), 8);
    }

    /// 标准帧 → 驱动帧: ID/ExternFlag/DataLen/Data 逐字段正确, 且 SendType 为
    /// 官方建议的单次发送 (1)。
    #[test]
    fn frame_to_vci_obj_standard() {
        let f = CanFrame::new(CanId::new_standard(0x456).unwrap(), vec![9, 8, 7]).unwrap();
        let obj = frame_to_vci_obj(&f).unwrap();
        assert_eq!(obj.ID, 0x456);
        assert_eq!(obj.ExternFlag, 0);
        assert_eq!(obj.RemoteFlag, 0);
        assert_eq!(obj.SendType, SEND_TYPE_SINGLE);
        assert_eq!(obj.DataLen, 3);
        assert_eq!(&obj.Data[..3], &[9, 8, 7]);
    }

    /// 扩展 + 远程帧 → 驱动帧: 两标志同时置位。
    #[test]
    fn frame_to_vci_obj_extended_remote() {
        let mut f = CanFrame::new(CanId::new_extended(0x1FF_FFFF).unwrap(), vec![]).unwrap();
        f.set_remote(true);
        let obj = frame_to_vci_obj(&f).unwrap();
        assert_eq!(obj.ID, 0x1FF_FFFF);
        assert_eq!(obj.ExternFlag, 1);
        assert_eq!(obj.RemoteFlag, 1);
        assert_eq!(obj.DataLen, 0);
    }

    /// CANFD 帧 → 驱动帧: 返回 Unsupported (硬件仅经典 CAN)。
    #[test]
    fn frame_to_vci_obj_rejects_fd() {
        let f =
            CanFrame::new_fd(CanId::new_standard(1).unwrap(), vec![0u8; 16], true, false).unwrap();
        assert!(matches!(
            frame_to_vci_obj(&f),
            Err(CanError::Unsupported(_))
        ));
    }

    /// hw_type → 设备类型映射全分支 (与 `map_hw_type_to_device_type` 规则表一致)。
    #[test]
    fn map_hw_type_to_device_type_full_table() {
        // 官方样例全部用 4 打开 hw_type="CAN-Linux" 设备 (厂商文档 C3) → 映射 4,
        // 不再特判 21。
        assert_eq!(map_hw_type_to_device_type("CAN-Linux"), VCI_USBCAN2);
        // 含 "2E_U" → 21 (含 E_U 的 2E_U 型号必须先被此分支接住)。
        assert_eq!(map_hw_type_to_device_type("USBCAN_2E_U"), VCI_USBCAN_2E_U);
        assert_eq!(map_hw_type_to_device_type("USBCAN-2E_U"), VCI_USBCAN_2E_U);
        // 含 "USBCAN2" / "CANalyst" → 4。
        assert_eq!(map_hw_type_to_device_type("USBCAN2"), VCI_USBCAN2);
        assert_eq!(
            map_hw_type_to_device_type("USBCAN-II (USBCAN2)"),
            VCI_USBCAN2
        );
        assert_eq!(map_hw_type_to_device_type("CANalyst-II"), VCI_USBCAN2);
        // 含 "USBCAN1" → 3。
        assert_eq!(map_hw_type_to_device_type("USBCAN1"), VCI_USBCAN1);
        // 含 "E_U" → 20。
        assert_eq!(map_hw_type_to_device_type("USBCAN_E_U"), VCI_USBCAN_E_U);
        // "2E_U" 与 "E_U" 同含时, 2E_U 分支 (21) 优先于 E_U 分支 (20)。
        assert_eq!(
            map_hw_type_to_device_type("USBCAN_2E_U with E_U suffix"),
            VCI_USBCAN_2E_U
        );
        // 其余型号 → 默认 4 (含空串与未知型号, 连字符型号 "USBCAN-II"/"USBCAN-E-U"
        // 不匹配任何字面子串, 同样走默认 4)。
        assert_eq!(map_hw_type_to_device_type(""), VCI_USBCAN2);
        assert_eq!(map_hw_type_to_device_type("unknown-device"), VCI_USBCAN2);
        assert_eq!(map_hw_type_to_device_type("USBCAN-II"), VCI_USBCAN2);
        assert_eq!(map_hw_type_to_device_type("USBCAN-E-U"), VCI_USBCAN2);
    }
}

/// 后端行为测试 (需 mock feature: 注入 MockVciOps, 不链接真实库)。
#[cfg(all(test, feature = "mock"))]
mod mock_tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    /// 构造注入 mock 的后端, 返回后端与共享设备状态。
    ///
    /// @param delay    重连等待 (测试用短时长加速)。
    /// @param attempts 重连尝试次数。
    /// @return (后端, 设备状态句柄)。
    fn make_backend(delay: Duration, attempts: u32) -> (UsbVciBackend, Arc<Mutex<MockDevice>>) {
        let device = Arc::new(Mutex::new(MockDevice::new()));
        let ops = MockVciOps {
            state: device.clone(),
        };
        let backend = UsbVciBackend::new_with(
            crate::ffi::VCI_USBCAN2,
            0,
            0,
            Box::new(ops),
            delay,
            attempts,
        );
        (backend, device)
    }

    /// 构造一帧驱动帧 (标准帧, 指定 ID 与数据)。
    fn obj(id: u32, data: &[u8]) -> VCI_CAN_OBJ {
        let mut obj = EMPTY_CAN_OBJ;
        obj.ID = id;
        obj.DataLen = data.len() as u8;
        obj.Data[..data.len()].copy_from_slice(data);
        obj
    }

    /// 轮询 + 批量: 2 帧 → 第一次读批量取回并缓冲, 两次 read 各得 1 帧, 顺序一致。
    #[test]
    fn poll_delivers_buffered_frames_in_order() {
        let (mut backend, device) = make_backend(Duration::from_millis(20), 2);
        {
            let mut device = device.lock().unwrap();
            device.rx_queue.push_back(obj(0x100, &[1, 2]));
            device.rx_queue.push_back(obj(0x200, &[3, 4, 5]));
        }
        let f1 = backend.read_frame(Duration::from_secs(1)).unwrap();
        assert_eq!(f1.id(), CanId::new_standard(0x100).unwrap());
        assert_eq!(f1.data(), &[1, 2]);
        assert!(f1.timestamp().is_some());

        let f2 = backend.read_frame(Duration::from_secs(1)).unwrap();
        assert_eq!(f2.id(), CanId::new_standard(0x200).unwrap());
        assert_eq!(f2.data(), &[3, 4, 5]);
        // 两帧应经一次批量 receive 取回 (首次 read 即拉全)。
        assert_eq!(device.lock().unwrap().last_receive_cap, 2);
    }

    /// 超时: 驱动始终无帧 → 轮询满超时窗口返回 Timeout。
    #[test]
    fn timeout_when_driver_empty() {
        let (mut backend, _device) = make_backend(Duration::from_millis(20), 2);
        let err = backend.read_frame(Duration::from_millis(200)).unwrap_err();
        assert_eq!(err, CanError::Timeout);
    }

    /// mutex 串行化: 多线程并发读写不 panic, 写全成功、读至少拿到一帧。
    #[test]
    fn concurrent_read_write_no_panic() {
        let (backend, device) = make_backend(Duration::from_millis(20), 2);
        {
            let mut device = device.lock().unwrap();
            for i in 0..16u32 {
                device.rx_queue.push_back(obj(0x300 + i, &[i as u8]));
            }
        }
        let shared = Arc::new(Mutex::new(backend));
        let mut handles = Vec::new();
        for tid in 0..4u32 {
            let shared = Arc::clone(&shared);
            handles.push(thread::spawn(move || {
                if tid % 2 == 0 {
                    // 读线程: 尽力读取, 超时视为正常 (帧数有限)。
                    let mut got = 0u32;
                    for _ in 0..8 {
                        let mut backend = shared.lock().unwrap();
                        if backend.read_frame(Duration::from_millis(300)).is_ok() {
                            got += 1;
                        }
                    }
                    got
                } else {
                    // 写线程: mock transmit 恒成功。
                    let mut sent = 0u32;
                    for i in 0..8u16 {
                        let mut backend = shared.lock().unwrap();
                        let frame =
                            CanFrame::new(CanId::new_standard(0x500 + i).unwrap(), vec![i as u8])
                                .unwrap();
                        if backend.write_frame(&frame).is_ok() {
                            sent += 1;
                        }
                    }
                    sent
                }
            }));
        }
        let mut total_got = 0u32;
        let mut total_sent = 0u32;
        for (tid, handle) in handles.into_iter().enumerate() {
            let result = handle.join().expect("并发线程 panic");
            if tid % 2 == 0 {
                total_got += result;
            } else {
                total_sent += result;
            }
        }
        // 写线程 (2 × 8 帧) 应全部成功; 读线程应至少拿到一帧 (16 帧足够分)。
        assert_eq!(total_sent, 16, "写帧应全部成功");
        assert!(total_got > 0, "读线程应至少拿到一帧");
    }

    /// 热插拔: 设备拔出后重新插入 → 触发重连, 重连成功拿到新帧。
    #[test]
    fn hotplug_reconnect_after_device_returns() {
        let (mut backend, device) = make_backend(Duration::from_millis(60), 5);
        // 初始在线, 预置一帧, 首次读取成功。
        device
            .lock()
            .unwrap()
            .rx_queue
            .push_back(obj(0x111, &[0xAA]));
        let f1 = backend.read_frame(Duration::from_millis(500)).unwrap();
        assert_eq!(f1.id(), CanId::new_standard(0x111).unwrap());

        // 拔出设备。
        let open_before = {
            let mut device = device.lock().unwrap();
            device.online = false;
            device.open_count
        };

        // 300ms 后设备重新插入并发出新帧。
        let replug = Arc::clone(&device);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let mut device = replug.lock().unwrap();
            device.online = true;
            device.rx_queue.push_back(obj(0x222, &[0xBB]));
        });

        // 读取应经 关闭→等待→重开 拿到新帧 (重连成功重置超时窗口)。
        let f2 = backend.read_frame(Duration::from_secs(3)).unwrap();
        assert_eq!(f2.id(), CanId::new_standard(0x222).unwrap());
        assert_eq!(f2.data(), &[0xBB]);
        assert!(f2.timestamp().is_some());
        // 重连应触发再次 open。
        assert!(device.lock().unwrap().open_count > open_before);
    }

    /// 热插拔: 设备持续离线 → 重试耗尽返回 DeviceUnplugged。
    #[test]
    fn hotplug_returns_unplugged_after_retries_exhausted() {
        let (mut backend, device) = make_backend(Duration::from_millis(30), 2);
        device.lock().unwrap().online = false;
        let err = backend.read_frame(Duration::from_secs(2)).unwrap_err();
        assert_eq!(err, CanError::DeviceUnplugged);
    }

    /// open 配置匹配: SocketCAN 配置 → Unsupported (mock 下同样生效)。
    #[test]
    fn open_rejects_non_usbvci_config() {
        // 用 .err() 而非 .unwrap_err(): UsbVciBackend 未实现 Debug, unwrap_err 需要它。
        let err = UsbVciBackend::open(&BackendConfig::SocketCan {
            iface: "vcan0".to_string(),
            fd: false,
        })
        .err()
        .expect("open 应返回错误");
        assert!(matches!(err, CanError::Unsupported(_)));
    }

    /// 构造注入 mock 的后端与设备, 供探测顺序测试定制板卡信息 / open 失败类型。
    ///
    /// @return (后端, 设备状态句柄)。
    fn make_probe_backend(device_type: u32) -> (UsbVciBackend, Arc<Mutex<MockDevice>>) {
        let device = Arc::new(Mutex::new(MockDevice::new()));
        let ops = MockVciOps {
            state: device.clone(),
        };
        let backend = UsbVciBackend::new_with(
            device_type,
            0,
            0,
            Box::new(ops),
            Duration::from_millis(20),
            2,
        );
        (backend, device)
    }

    /// 探测顺序 (无 find): 配置类型 21 一次成功 → 默认候选 4/21 不再尝试
    /// (配置 21 与默认 2E_U 去重后只出现一次), 失败候选不 close。
    #[test]
    fn probe_config_type_first_succeeds_immediately() {
        let (mut backend, device) = make_probe_backend(VCI_USBCAN_2E_U);
        backend.probe_and_open().expect("探测应成功");
        {
            let device = device.lock().unwrap();
            // 候选 [21(配置), 4, 21(2E_U 去重跳过)] → 配置类型 21 一次成功。
            assert_eq!(device.open_history, vec![21]);
            // 无失败候选 → close 历史必须为空。
            assert!(device.close_history.is_empty());
        }
        assert_eq!(backend.device_type, 21);
    }

    /// 探测顺序 (无 find): 配置类型 4 先于默认候选被尝试 (4 与默认 USBCAN2 去重)。
    #[test]
    fn probe_config_type_prioritized_over_defaults() {
        let (mut backend, device) = make_probe_backend(VCI_USBCAN2);
        backend.probe_and_open().expect("探测应成功");
        {
            let device = device.lock().unwrap();
            // 候选 [4(配置), 4(USBCAN2 去重跳过), 21] → 配置类型 4 先试, 一次成功。
            assert_eq!(device.open_history, vec![4]);
        }
        assert_eq!(backend.device_type, 4);
    }

    /// 探测顺序 (无 find): 未指定类型 (0) 时跳过该候选, 默认 USBCAN2 (4) 先于
    /// 2E_U (21) (官方样例全部用 4 打开); 失败候选 (4) 不做 close。
    #[test]
    fn probe_without_find_tries_usbcan2_before_2e_u() {
        let (mut backend, device) = make_probe_backend(0); // 配置类型未指定
        device.lock().unwrap().fail_open_for = Some(VCI_USBCAN2); // 4 打开失败
        backend.probe_and_open().expect("探测应回退到 21");
        {
            let device = device.lock().unwrap();
            // 0 被跳过; 候选 [4, 21] 依次尝试, 4 先于 21。
            assert_eq!(device.open_history, vec![VCI_USBCAN2, VCI_USBCAN_2E_U]);
            // 失败的候选 (4) 不应被 close (防脆弱固件二次崩溃)。
            assert!(device.close_history.is_empty());
        }
        assert_eq!(backend.device_type, VCI_USBCAN_2E_U);
    }

    /// 探测去重: 配置类型 21 失败 → 回退候选 21 (2E_U) 去重跳过, 直接落到 4;
    /// 失败候选不做 close。
    #[test]
    fn probe_dedupes_config_and_default_candidates() {
        let (mut backend, device) = make_probe_backend(VCI_USBCAN_2E_U);
        device.lock().unwrap().fail_open_for = Some(VCI_USBCAN_2E_U); // 配置类型 21 打开失败
        backend.probe_and_open().expect("探测应回退成功");
        {
            let device = device.lock().unwrap();
            // 候选 [21(配置), 4, 21(2E_U, 去重跳过)] → 只尝试 21 一次, 再落到 4。
            assert_eq!(device.open_history, vec![21, 4]);
            // 失败的 21 不应被 close (防脆弱固件二次崩溃)。
            assert!(device.close_history.is_empty());
        }
        assert_eq!(backend.device_type, VCI_USBCAN2);
    }

    /// 探测失败: 全部候选 (21, 4) 打开失败 → NotFound, 且失败候选始终不 close。
    #[test]
    fn probe_all_fail_returns_not_found() {
        let (mut backend, device) = make_probe_backend(VCI_USBCAN_2E_U);
        device.lock().unwrap().online = false; // open 恒失败
        let err = backend.probe_and_open().expect_err("探测应失败");
        assert_eq!(err, CanError::NotFound);
        {
            let device = device.lock().unwrap();
            // 候选 [21(配置), 4, 21(2E_U 去重跳过)] → 全部失败。
            assert_eq!(device.open_history, vec![21, 4]);
            assert!(device.close_history.is_empty(), "失败候选不得 close");
        }
    }
}

/// 设备发现测试 (需 mock feature): 经 MockVciOps 注入模拟设备, 验证枚举逻辑。
#[cfg(all(test, feature = "mock"))]
mod discoverer_tests {
    use super::*;
    use can_types::DeviceKind;
    use std::sync::Arc;

    /// mock 枚举应返回 2 台设备: 型号取自 `str_hw_Type`, kind/driver/available 正确。
    #[test]
    fn mock_find_returns_two_devices_with_models() {
        let device = Arc::new(Mutex::new(MockDevice::new()));
        let ops = MockVciOps {
            state: device.clone(),
        };
        let devices = list_devices_with(&ops);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, "0");
        assert_eq!(devices[0].name, "USBCAN-II");
        assert_eq!(devices[0].kind, DeviceKind::UsbVci);
        assert_eq!(devices[0].driver, "usbvci");
        assert_eq!(devices[0].details.model, "USBCAN-II");
        assert!(devices[0].available);
        // hw_type 映射的设备类型: "USBCAN-II"/"USBCAN-E-U" 不匹配任何字面子串 → 默认 4。
        assert_eq!(devices[0].device_type, crate::ffi::VCI_USBCAN2);
        assert_eq!(devices[1].id, "1");
        assert_eq!(devices[1].details.model, "USBCAN-E-U");
        assert_eq!(devices[1].device_type, crate::ffi::VCI_USBCAN2);
    }

    /// discoverer/list 路径同步: hw_type "CAN-Linux" (Linux 驱动下 2E_U 设备上报)
    /// 映射到默认设备类型 4 (官方样例全用 4 打开), 而非 21。
    #[test]
    fn discoverer_maps_can_linux_to_usbcan2() {
        let device = Arc::new(Mutex::new(MockDevice::new()));
        {
            let mut device = device.lock().unwrap();
            device.board_infos = vec![mock_board_info("CAN-Linux")];
        }
        let ops = MockVciOps {
            state: device.clone(),
        };
        let devices = list_devices_with(&ops);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].details.model, "CAN-Linux");
        assert_eq!(devices[0].device_type, crate::ffi::VCI_USBCAN2);
    }

    /// 枚举对容量不足的输出缓冲区截断到缓冲容量, 不 panic。
    #[test]
    fn mock_find_with_tiny_buffer_no_panic() {
        let device = Arc::new(Mutex::new(MockDevice::new()));
        let ops = MockVciOps {
            state: device.clone(),
        };
        let mut infos = [EMPTY_BOARD_INFO; 1];
        let count = ops.find_usb_devices(&mut infos);
        assert_eq!(count, 1, "容量不足时应截断到缓冲容量");
    }

    /// `str_hw_Type` 含非法 UTF-8 字节时 lossy 转换, 不 panic。
    #[test]
    fn model_lossy_on_non_utf8() {
        let mut info = mock_board_info("USBCAN-II");
        info.str_hw_Type[9] = 0xFFu8 as i8; // 非法 UTF-8 首字节
        let model = board_info_model(&info);
        assert!(
            model.starts_with("USBCAN"),
            "lossy 应保留 ASCII 前缀, 得到 {model:?}"
        );
    }

    /// `str_hw_Type` 填满 40 字节无 NUL 时按全数组 lossy 转换, 不 panic。
    #[test]
    fn model_full_array_without_nul() {
        let info = VCI_BOARD_INFO {
            str_hw_Type: [b'X' as i8; 40],
            ..EMPTY_BOARD_INFO
        };
        let model = board_info_model(&info);
        assert_eq!(model.len(), 40, "无 NUL 时应取全 40 字节");
    }

    /// discoverer 公开入口 (mock feature) 返回 2 台模拟设备, 不 panic。
    #[test]
    fn discoverer_list_devices_in_mock() {
        let devices = UsbVciDiscoverer::list_devices();
        assert_eq!(devices.len(), 2);
        assert!(devices.iter().all(|d| d.kind == DeviceKind::UsbVci));
    }
}

/// 真实库加载 smoke test (feature = "real-ffi"): 验证 libloading 动态加载路径。
///
/// 本机 third_party/controlcan/<arch>/libcontrolcan.so 存在时真实加载并解析全部
/// 13 个符号 (本任务硬性要求); 库缺失时加载测试打印 SKIP 通过 —— "库缺失返回友好
/// 错误" 由 `missing_library_*` 用不存在的路径专门断言。
#[cfg(all(
    test,
    feature = "real-ffi",
    not(feature = "mock"),
    not(usbvci_static_link)
))]
mod real_ffi_tests {
    use super::*;

    /// 供应商库路径 = workspace 根 (CARGO_MANIFEST_DIR/../..) + third_party/controlcan/<arch>。
    fn vendor_lib_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("third_party")
            .join("controlcan")
            .join(std::env::consts::ARCH)
            .join("libcontrolcan.so")
    }

    /// 加载本机供应商库并解析全部 13 个符号, 且函数指针可真实调用 (无设备时
    /// OpenDevice 返回失败, 不 panic)。
    #[test]
    fn loads_vendor_lib_and_resolves_all_13_symbols() {
        let lib_path = vendor_lib_path();
        if !lib_path.is_file() {
            eprintln!("SKIP: 供应商库不存在, 跳过真实加载: {}", lib_path.display());
            return;
        }
        let library = load_library(&lib_path).expect("应能加载供应商库");
        // SAFETY: library 句柄在本测试存活期内有效。
        let syms = unsafe { VciSymbols::from_library(&library) }.expect("应能解析全部 13 个符号");
        let pointers: [usize; 13] = [
            syms.open_device as usize,
            syms.close_device as usize,
            syms.init_can as usize,
            syms.read_board_info as usize,
            syms.set_reference as usize,
            syms.get_receive_num as usize,
            syms.clear_buffer as usize,
            syms.start_can as usize,
            syms.reset_can as usize,
            syms.transmit as usize,
            syms.receive as usize,
            syms.usb_device_reset as usize,
            syms.find_usb_device2 as usize,
        ];
        assert!(
            pointers.iter().all(|p| *p != 0),
            "全部 13 个符号应解析为非空函数指针"
        );
        // 真实调用一次: 无设备时本 .so 返回 0xFFFFFFFF (已知 ZLGCAN "无设备/错误" 哨兵值,
        // 与 VCI_GetReceiveNum 拔出语义一致); 返回 0/1 亦属正常。重点是库加载 + 符号
        // 真实可调, 不 panic。
        let status = unsafe { (syms.open_device)(crate::ffi::VCI_USBCAN2, 0, 0) };
        assert!(
            matches!(status, 0 | 1 | u32::MAX),
            "OpenDevice 应返回 0/1/0xFFFFFFFF, 得到 {status}"
        );
        // 真实调用枚举符号: 本机无设备时应返回 0 (或不超过缓冲容量的设备数),
        // 证明 VCI_FindUsbDevice2 可解析且可调用, 不 panic、不回填越界。
        let mut infos = [EMPTY_BOARD_INFO; MAX_BOARD_INFO_CAP];
        let found = unsafe { (syms.find_usb_device2)(infos.as_mut_ptr()) };
        assert!(
            found == 0 || found <= MAX_BOARD_INFO_CAP as u32,
            "FindUsbDevice2 应返回 0 (无设备) 或不超过缓冲容量的设备数, 得到 {found}"
        );
    }

    /// CAN_USBVCI_LIB 环境变量注入 → resolve_library 优先返回该路径。
    #[test]
    fn resolve_library_prefers_env_override() {
        std::env::set_var("CAN_USBVCI_LIB", "/tmp/vci-lib-candidate.so");
        let path = resolve_library().expect("解析库路径不应失败");
        std::env::remove_var("CAN_USBVCI_LIB");
        assert_eq!(path, PathBuf::from("/tmp/vci-lib-candidate.so"));
    }

    /// 库缺失 → load_library 返回 CanError::Io (带部署提示), 不 panic。
    #[test]
    fn missing_library_returns_friendly_error() {
        let err = load_library(Path::new(
            "/nonexistent/definitely-missing-libcontrolcan.so",
        ))
        .expect_err("缺失库应返回 Err");
        assert!(
            matches!(err, CanError::Io(_)),
            "应返回 CanError::Io, 得到 {err:?}"
        );
    }

    /// 库缺失 → RealVciOps::try_new 返回 Err 而非 panic。
    #[test]
    fn try_new_returns_error_for_missing_library() {
        std::env::set_var(
            "CAN_USBVCI_LIB",
            "/nonexistent/definitely-missing-libcontrolcan.so",
        );
        let result = RealVciOps::try_new();
        std::env::remove_var("CAN_USBVCI_LIB");
        assert!(result.is_err(), "库缺失时 try_new 应返回 Err, 而非 panic");
    }
}

/// Tauri Linux 打包资源目录回退测试 (非 mock / 非 static 链接): 验证发行包布局下
/// resolve_library 能从 exe 的 `../lib/can-monitor/<arch>/` 资源目录定位供应商库。
#[cfg(all(
    test,
    target_os = "linux",
    not(feature = "mock"),
    not(usbvci_static_link)
))]
mod tauri_resource_tests {
    use super::*;

    /// 创建唯一的临时根目录 (前缀含标签与进程号, 避免并行测试互撞)。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "can-usbvci-tauri-resolve-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// exe 目录 = 临时根/bin: 其父目录即本测试独占的根, `../lib/can-monitor/..`
    /// 不会与并行测试的产物互撞。
    fn exe_dir(root: &Path) -> PathBuf {
        let dir = root.join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 编译期架构目录名, 与 find_tauri_resource_lib 内部映射保持一致。
    fn arch_dir() -> &'static str {
        if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        }
    }

    /// 在 `exe_dir/../<rel>/libcontrolcan.so` 造一个假供应商库, 返回其路径。
    fn plant_lib(exe_dir: &Path, rel: &str) -> PathBuf {
        let lib = exe_dir.join("..").join(rel).join("libcontrolcan.so");
        std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
        std::fs::write(&lib, b"fake controlcan lib").unwrap();
        lib
    }

    /// 命中 <arch>/ 子目录: `../lib/can-monitor/<arch>/libcontrolcan.so` 存在即返回。
    #[test]
    fn find_tauri_resource_lib_hits_arch_subdir() {
        let root = temp_dir("arch");
        let exe = exe_dir(&root);
        let expected = plant_lib(&exe, &format!("lib/can-monitor/{}", arch_dir()));
        let found = find_tauri_resource_lib(&exe).expect("应命中 <arch>/ 资源目录");
        assert_eq!(found, expected);
    }

    /// <arch>/ 与平铺目录同时存在 → <arch>/ 优先命中。
    #[test]
    fn find_tauri_resource_lib_prefers_arch_over_flat() {
        let root = temp_dir("prefer");
        let exe = exe_dir(&root);
        let arch_lib = plant_lib(&exe, &format!("lib/can-monitor/{}", arch_dir()));
        let _flat_lib = plant_lib(&exe, "lib/can-monitor");
        let found = find_tauri_resource_lib(&exe).expect("应命中资源目录");
        assert_eq!(found, arch_lib);
    }

    /// 平铺兜底: 仅 `../lib/can-monitor/libcontrolcan.so` 存在 → 命中平铺路径。
    #[test]
    fn find_tauri_resource_lib_falls_back_to_flat_dir() {
        let root = temp_dir("flat");
        let exe = exe_dir(&root);
        let expected = plant_lib(&exe, "lib/can-monitor");
        let found = find_tauri_resource_lib(&exe).expect("应命中平铺资源目录");
        assert_eq!(found, expected);
    }

    /// 资源目录缺库 → 返回 None (resolve_library 回退到 dlopen 系统搜索)。
    #[test]
    fn find_tauri_resource_lib_returns_none_when_missing() {
        let root = temp_dir("missing");
        let exe = exe_dir(&root);
        assert!(
            find_tauri_resource_lib(&exe).is_none(),
            "资源目录无库时应返回 None"
        );
    }

    /// 全链路: exe 同目录与资源目录均无库 → resolve_library 回退到裸文件名
    /// (交 dlopen 的 LD_LIBRARY_PATH / rpath / 系统搜索)。
    #[test]
    fn resolve_library_falls_back_to_bare_filename() {
        std::env::remove_var("CAN_USBVCI_LIB");
        let path = resolve_library().expect("解析库路径不应失败");
        assert_eq!(path, PathBuf::from("libcontrolcan.so"));
    }
}
