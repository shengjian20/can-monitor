//! SocketCAN 真实实现 (仅 Linux)。
//!
//! 直接调用 Linux `AF_CAN` 套接字 syscall (经 `socketcan` crate 封装),
//! 仅在 `target_os = "linux"` 时编译。

use std::fs;
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use can_types::{
    BackendConfig, CanBackend, CanDeviceInfo, CanError, CanFrame, CanId, DeviceDetails,
    DeviceDiscoverer, DeviceKind, Result,
};
use socketcan::id::FdFlags;
use socketcan::{CanAnyFrame, CanFdSocket, CanSocket, EmbeddedFrame, Socket, SocketOptions};

/// 非阻塞轮询间隔 (秒)。
///
/// 每次 [`SocketCanBackend::read_frame`] 探测无帧后的休眠时间,越小延迟越低、CPU 占用越高。
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// 底层 SocketCAN 套接字的类型。
///
/// 经典 CAN 与 FD 使用不同的套接字类型,统一用一个枚举承载。
enum SocketKind {
    /// 经典 CAN 2.0 套接字。
    Classic(CanSocket),
    /// CAN FD 套接字。
    Fd(CanFdSocket),
}

/// SocketCAN 后端。
///
/// 通过 Linux SocketCAN 网络接口 (如 `vcan0` / `can0`) 收发 CAN 帧。
/// 由 [`CanBackend::open`] 构造,内部持有经典或 FD 套接字。
pub struct SocketCanBackend {
    /// 底层套接字 (`None` 表示已关闭)。
    socket: Option<SocketKind>,
}

impl SocketCanBackend {
    /// 尝试读取一帧 (非阻塞,单次探测)。
    ///
    /// 读取失败会原样返回底层 `io::Error` 转换后的 [`CanError`]
    /// (无数据时为 `WouldBlock`),由上层 [`CanBackend::read_frame`] 决定是否重试。
    ///
    /// @return 成功返回协议无关的 [`CanFrame`];底层错误 (含 `WouldBlock`) 返回相应 [`CanError`]。
    fn read_once(&self) -> Result<CanFrame> {
        let socket = self
            .socket
            .as_ref()
            .ok_or(CanError::Protocol("后端已关闭"))?;
        match socket {
            SocketKind::Classic(s) => {
                let (frame, ts) = s.read_frame_with_timestamps()?;
                convert_classic_frame(frame, socket_ts_or_now(ts))
            }
            SocketKind::Fd(s) => {
                let (frame, ts) = s.read_frame_with_timestamps()?;
                convert_any_frame(frame, socket_ts_or_now(ts))
            }
        }
    }
}

impl CanBackend for SocketCanBackend {
    /// 按配置打开 SocketCAN 后端。
    ///
    /// @param config 后端配置,仅支持 [`BackendConfig::SocketCan`]。
    /// @return 成功返回已打开的 [`SocketCanBackend`];配置不是 SocketCan 返回
    ///         [`CanError::Unsupported`],接口不存在返回 [`CanError::NotFound`]。
    fn open(config: &BackendConfig) -> Result<Self> {
        match config {
            BackendConfig::SocketCan { iface, fd } => {
                let socket = if *fd {
                    let s = CanFdSocket::open(iface).map_err(map_open_error)?;
                    // 尽力启用内核接收时间戳,失败则回退 SystemTime::now()
                    let _ = s.set_recv_timestamp(true);
                    SocketKind::Fd(s)
                } else {
                    let s = CanSocket::open(iface).map_err(map_open_error)?;
                    let _ = s.set_recv_timestamp(true);
                    SocketKind::Classic(s)
                };
                // 切换为非阻塞模式,供 read_frame 轮询 + 超时使用
                match &socket {
                    SocketKind::Classic(s) => s.set_nonblocking(true).map_err(map_open_error)?,
                    SocketKind::Fd(s) => s.set_nonblocking(true).map_err(map_open_error)?,
                }
                Ok(SocketCanBackend {
                    socket: Some(socket),
                })
            }
            _ => Err(CanError::Unsupported("仅支持 SocketCan 后端配置")),
        }
    }

    /// 从总线读取一帧,支持超时。
    ///
    /// 采用非阻塞探测 + 短休眠轮询:底层返回 `WouldBlock` 时休眠
    /// `POLL_INTERVAL` (1ms) 后重试,累计超过 `timeout` 返回 [`CanError::Timeout`]。
    ///
    /// @param timeout 阻塞等待一帧的最长时间。
    /// @return 成功返回收到的 [`CanFrame`];超时返回 [`CanError::Timeout`]。
    fn read_frame(&mut self, timeout: Duration) -> Result<CanFrame> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.read_once() {
                Ok(frame) => return Ok(frame),
                Err(CanError::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(CanError::Timeout);
                    }
                    thread::sleep(POLL_INTERVAL);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// 向总线写入一帧。
    ///
    /// @param frame 待发送的帧。
    /// @return 成功返回 `Ok(())`;经典接口写 FD 帧返回 [`CanError::Unsupported`],
    ///         数据超长返回 [`CanError::FrameTooLong`],底层失败返回 [`CanError::Io`]。
    fn write_frame(&mut self, frame: &CanFrame) -> Result<()> {
        let socket = self
            .socket
            .as_ref()
            .ok_or(CanError::Protocol("后端已关闭"))?;
        match socket {
            SocketKind::Classic(s) => {
                if frame.is_fd() {
                    return Err(CanError::Unsupported("经典 CAN 接口不支持 FD 帧"));
                }
                let out = to_socketcan_classic(frame)?;
                s.write_frame(&out)?;
            }
            SocketKind::Fd(s) => {
                let out = to_socketcan_any(frame)?;
                s.write_frame(&out)?;
            }
        }
        Ok(())
    }

    /// 关闭后端并释放套接字。
    ///
    /// @return 成功返回 `Ok(())` (套接字立即被丢弃关闭)。
    fn close(&mut self) -> Result<()> {
        self.socket = None;
        Ok(())
    }
}

// ===== 错误映射 =====

/// 将底层 `io::Error` 映射为 [`CanError`]。
///
/// 接口不存在的 `NotFound` 语义 (接口名无效 / 未加载 CAN 驱动) 映射为
/// [`CanError::NotFound`],其余错误保留为 [`CanError::Io`]。
///
/// @param e 底层 IO 错误。
/// @return 映射后的 [`CanError`]。
fn map_open_error(e: io::Error) -> CanError {
    match e.kind() {
        io::ErrorKind::NotFound => CanError::NotFound,
        _ => CanError::Io(e),
    }
}

/// 从接收时间戳结构提取 `SystemTime`。
///
/// 优先使用内核交付的 socket 层时间戳;未启用或内核未交付时回退到当前系统时间。
///
/// @param ts socketcan 返回的时间戳集合。
/// @return 用于填充帧时间戳的 [`SystemTime`]。
fn socket_ts_or_now(ts: socketcan::CanTimestamps) -> SystemTime {
    ts.socket.unwrap_or_else(SystemTime::now)
}

// ===== ID 转换 =====

/// 将 socketcan 的 ID 转换为协议无关的 [`CanId`]。
///
/// @param id socketcan (embedded_can) 的 ID。
/// @return 转换后的 [`CanId`]。
fn from_socketcan_id(id: socketcan::Id) -> Result<CanId> {
    match id {
        socketcan::Id::Standard(sid) => CanId::new_standard(sid.as_raw()),
        socketcan::Id::Extended(eid) => CanId::new_extended(eid.as_raw()),
    }
}

/// 将协议无关的 [`CanId`] 转换为 socketcan 的 ID。
///
/// @param id 协议无关的 [`CanId`]。
/// @return 转换后的 socketcan ID;ID 越界返回 [`CanError::InvalidId`]。
fn to_socketcan_id(id: CanId) -> Result<socketcan::Id> {
    if id.is_extended() {
        socketcan::ExtendedId::new(id.raw_id())
            .map(socketcan::Id::Extended)
            .ok_or(CanError::InvalidId)
    } else {
        socketcan::StandardId::new(id.raw_id() as u16)
            .map(socketcan::Id::Standard)
            .ok_or(CanError::InvalidId)
    }
}

// ===== 读方向转换 (socketcan → can-types) =====

/// 将 socketcan 经典帧转换为协议无关的 [`CanFrame`]。
///
/// @param frame socketcan 经典帧 (数据 / 远程 / 错误)。
/// @param ts    帧接收时间戳。
/// @return 转换后的 [`CanFrame`];收到错误帧返回 [`CanError::BusError`]。
fn convert_classic_frame(frame: socketcan::CanFrame, ts: SystemTime) -> Result<CanFrame> {
    let mut out = match frame {
        socketcan::CanFrame::Data(df) => {
            CanFrame::new(from_socketcan_id(df.id())?, df.data().to_vec())?
        }
        socketcan::CanFrame::Remote(rf) => {
            let mut f = CanFrame::new(from_socketcan_id(rf.id())?, Vec::new())?;
            f.set_remote(true);
            f
        }
        socketcan::CanFrame::Error(_) => return Err(CanError::BusError),
    };
    out.set_timestamp(ts);
    Ok(out)
}

/// 将 socketcan 任意帧 (经典 / FD) 转换为协议无关的 [`CanFrame`]。
///
/// @param frame socketcan 任意帧 (普通 / 远程 / 错误 / FD)。
/// @param ts    帧接收时间戳。
/// @return 转换后的 [`CanFrame`];收到错误帧返回 [`CanError::BusError`]。
fn convert_any_frame(frame: CanAnyFrame, ts: SystemTime) -> Result<CanFrame> {
    let mut out = match frame {
        CanAnyFrame::Normal(df) => CanFrame::new(from_socketcan_id(df.id())?, df.data().to_vec())?,
        CanAnyFrame::Remote(rf) => {
            let mut f = CanFrame::new(from_socketcan_id(rf.id())?, Vec::new())?;
            f.set_remote(true);
            f
        }
        CanAnyFrame::Error(_) => return Err(CanError::BusError),
        CanAnyFrame::Fd(fdf) => CanFrame::new_fd(
            from_socketcan_id(fdf.id())?,
            fdf.data().to_vec(),
            fdf.is_brs(),
            fdf.is_esi(),
        )?,
    };
    out.set_timestamp(ts);
    Ok(out)
}

// ===== 写方向转换 (can-types → socketcan) =====

/// 将协议无关的 [`CanFrame`] 转换为 socketcan 经典帧。
///
/// @param frame 协议无关的 [`CanFrame`] (非 FD)。
/// @return 转换后的 socketcan 经典帧;数据超长返回 [`CanError::FrameTooLong`]。
fn to_socketcan_classic(frame: &CanFrame) -> Result<socketcan::CanFrame> {
    let id = to_socketcan_id(frame.id())?;
    if frame.is_remote() {
        let rf =
            socketcan::CanRemoteFrame::new_remote(id, frame.len()).ok_or(CanError::FrameTooLong)?;
        Ok(socketcan::CanFrame::Remote(rf))
    } else {
        let df = socketcan::CanDataFrame::new(id, frame.data()).ok_or(CanError::FrameTooLong)?;
        Ok(socketcan::CanFrame::Data(df))
    }
}

/// 将协议无关的 [`CanFrame`] 转换为 socketcan 任意帧 (经典 / FD)。
///
/// @param frame 协议无关的 [`CanFrame`]。
/// @return 转换后的 socketcan 任意帧;数据超长返回 [`CanError::FrameTooLong`]。
fn to_socketcan_any(frame: &CanFrame) -> Result<CanAnyFrame> {
    let id = to_socketcan_id(frame.id())?;
    if frame.is_fd() {
        let flags = fd_flags(frame);
        let fdf = socketcan::CanFdFrame::with_flags(id, frame.data(), flags)
            .ok_or(CanError::FrameTooLong)?;
        Ok(CanAnyFrame::Fd(fdf))
    } else if frame.is_remote() {
        let rf =
            socketcan::CanRemoteFrame::new_remote(id, frame.len()).ok_or(CanError::FrameTooLong)?;
        Ok(CanAnyFrame::Remote(rf))
    } else {
        let df = socketcan::CanDataFrame::new(id, frame.data()).ok_or(CanError::FrameTooLong)?;
        Ok(CanAnyFrame::Normal(df))
    }
}

/// 根据帧的 BRS / ESI 标志构造 FD 标志位集合。
///
/// @param frame 协议无关的 [`CanFrame`]。
/// @return socketcan 的 FD 标志位集合。
fn fd_flags(frame: &CanFrame) -> FdFlags {
    let mut flags = FdFlags::empty();
    if frame.brs() {
        flags |= FdFlags::BRS;
    }
    if frame.esi() {
        flags |= FdFlags::ESI;
    }
    flags
}

// ===== 设备发现 (sysfs 扫描) =====

/// `ARPHRD_CAN` 协议号 (Linux `if_arp.h`)。
///
/// `can` / `vcan` / `slcan` 接口的 `/sys/class/net/<if>/type` 文件内容均为 280,
/// 是 sysfs 判定 CAN 接口的权威依据。
const ARPHRD_CAN: u32 = 280;

/// 已知的 CAN 接口名前缀,作为 `type` 文件缺失时的 fallback 判定。
const CAN_IFACE_PREFIXES: [&str; 3] = ["can", "vcan", "slcan"];

/// Linux sysfs 网络接口目录。
const SYSFS_NET_DIR: &str = "/sys/class/net";

/// SocketCAN 设备发现器。
///
/// 扫描 Linux sysfs (`/sys/class/net`) 下所有网络接口目录,过滤出 CAN 接口
/// (vcan / can / slcan),构造协议无关的 [`CanDeviceInfo`] 列表。
/// 纯 sysfs 扫描,不引入额外系统库或第三方设备枚举依赖,
/// 保证跨平台 (非 Linux 平台走降级 stub 实现) 依赖面不变。
pub struct SocketCanDiscoverer;

impl DeviceDiscoverer for SocketCanDiscoverer {
    /// 枚举当前系统上的 SocketCAN 接口。
    ///
    /// @return CAN 接口的 [`CanDeviceInfo`] 列表;系统无任何 CAN 接口
    ///         (或 sysfs 目录不存在 / 不可读) 时返回空列表,不 panic。
    fn list_devices() -> Vec<CanDeviceInfo> {
        scan_sysfs(Path::new(SYSFS_NET_DIR))
    }
}

/// 便捷函数:枚举当前系统上的 SocketCAN 接口。
///
/// 等价于 [`SocketCanDiscoverer`] 实现 [`DeviceDiscoverer::list_devices`] 的结果。
///
/// @return 当前系统可发现的 SocketCAN 接口列表 (无则空列表)。
pub fn list_devices() -> Vec<CanDeviceInfo> {
    SocketCanDiscoverer::list_devices()
}

/// 扫描给定 sysfs 网络目录,返回其中全部 CAN 接口的设备信息。
///
/// 纯函数:不访问真实系统路径,扫描逻辑可注入任意目录 (测试用临时目录
/// 写入 fake `type` / `operstate` / `device/driver` 文件)。
///
/// @param net_dir sysfs 网络接口目录 (如 `/sys/class/net`)。
/// @return CAN 接口的 [`CanDeviceInfo`] 列表,按接口名排序;
///         目录不存在 / 不可读时返回空列表 (不 panic)。
fn scan_sysfs(net_dir: &Path) -> Vec<CanDeviceInfo> {
    let mut devices = Vec::new();
    let Ok(entries) = fs::read_dir(net_dir) else {
        return devices;
    };
    for entry in entries.flatten() {
        let iface_dir = entry.path();
        if !is_can_interface(&iface_dir) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let available = read_file(&iface_dir.join("operstate")).is_some_and(|s| s.trim() == "up");
        let driver = read_driver(&iface_dir);
        devices.push(CanDeviceInfo {
            id: name.clone(),
            name,
            kind: DeviceKind::SocketCan,
            driver,
            details: DeviceDetails::with_model("SocketCAN"),
            available,
            device_type: 0,
        });
    }
    devices.sort_by(|a, b| a.id.cmp(&b.id));
    devices
}

/// 判定一个 sysfs 接口目录是否属于 CAN 接口。
///
/// 权威依据:接口的 `type` 文件内容 == [`ARPHRD_CAN`] (280),真实 CAN 控制器、
/// `vcan` 虚拟接口与 `slcan` 串口 CAN 均报告该协议号;`type` 文件缺失时
/// 回退按接口名前缀 (`can` / `vcan` / `slcan`) 判定。
///
/// @param iface_dir sysfs 接口目录 (如 `/sys/class/net/vcan0`)。
/// @return 是 CAN 接口返回 `true`。
fn is_can_interface(iface_dir: &Path) -> bool {
    if let Some(ty) = read_file(&iface_dir.join("type")).and_then(|s| s.trim().parse::<u32>().ok())
    {
        return ty == ARPHRD_CAN;
    }
    let name = iface_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    CAN_IFACE_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// 读取接口对应的内核驱动名。
///
/// 真实硬件 CAN 控制器在 `/sys/class/net/<if>/device/driver` 有驱动符号链接
/// (如 `mcp251x` / `gs_usb`);vcan 等虚拟接口无该链接,回退默认 `"socketcan"`。
///
/// @param iface_dir sysfs 接口目录。
/// @return 内核驱动名;无法读取时返回 `"socketcan"`。
fn read_driver(iface_dir: &Path) -> String {
    fs::read_link(iface_dir.join("device").join("driver"))
        .ok()
        .and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "socketcan".to_string())
}

/// 读取 sysfs 文本文件内容。
///
/// @param path 文件路径。
/// @return 文件内容;读取失败 (文件不存在 / 权限不足) 返回 `None`。
fn read_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use socketcan::{CanDataFrame, CanFdFrame, CanRemoteFrame, ExtendedId, Id, StandardId};

    /// 固定时间戳用于断言转换后的时间戳回填。
    fn fixed_ts() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    // ---- 标准帧转换 ----

    /// 标准帧 (11 位 ID, 8 字节数据) 应完整转换。
    #[test]
    fn classic_standard_frame_roundtrip() {
        let id = StandardId::new(0x123).unwrap();
        let df = CanDataFrame::new(id, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let ts = fixed_ts();
        let frame = convert_classic_frame(socketcan::CanFrame::Data(df), ts).unwrap();

        assert!(frame.id().is_standard());
        assert_eq!(frame.id().raw_id(), 0x123);
        assert_eq!(frame.data(), &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(frame.len(), 8);
        assert!(!frame.is_fd());
        assert!(!frame.is_remote());
        assert_eq!(frame.timestamp(), Some(ts));
    }

    /// 扩展帧 (29 位 ID) 应完整转换。
    #[test]
    fn classic_extended_frame_roundtrip() {
        let id = ExtendedId::new(0x1F_FFFF).unwrap();
        let df = CanDataFrame::new(id, &[0xAA, 0xBB]).unwrap();
        let frame = convert_classic_frame(socketcan::CanFrame::Data(df), fixed_ts()).unwrap();

        assert!(frame.id().is_extended());
        assert_eq!(frame.id().raw_id(), 0x1F_FFFF);
        assert_eq!(frame.data(), &[0xAA, 0xBB]);
    }

    /// 远程帧应标记 `remote=true` 且数据为空。
    #[test]
    fn classic_remote_frame_roundtrip() {
        let id = StandardId::new(0x456).unwrap();
        let rf = CanRemoteFrame::new_remote(id, 2).unwrap();
        let frame = convert_classic_frame(socketcan::CanFrame::Remote(rf), fixed_ts()).unwrap();

        assert!(frame.is_remote());
        assert_eq!(frame.id().raw_id(), 0x456);
        assert!(frame.data().is_empty());
    }

    /// 错误帧应映射为 [`CanError::BusError`]。
    #[test]
    fn classic_error_frame_maps_to_bus_error() {
        let ef = socketcan::CanErrorFrame::new_error(0, &[]).unwrap();
        let res = convert_classic_frame(socketcan::CanFrame::Error(ef), fixed_ts());
        assert_eq!(res, Err(CanError::BusError));
    }

    // ---- FD 帧转换 ----

    /// FD 帧 (fd / brs / esi 标志, 64 字节) 应完整转换。
    #[test]
    fn fd_frame_roundtrip() {
        let id = ExtendedId::new(0x18FF_1234).unwrap();
        let fdf = CanFdFrame::with_flags(id, &[0x11; 64], FdFlags::BRS | FdFlags::ESI).unwrap();
        let frame = convert_any_frame(CanAnyFrame::Fd(fdf), fixed_ts()).unwrap();

        assert!(frame.is_fd());
        assert!(frame.brs());
        assert!(frame.esi());
        assert_eq!(frame.id().raw_id(), 0x18FF_1234);
        assert_eq!(frame.len(), 64);
        assert_eq!(frame.data(), &[0x11; 64]);
    }

    /// FD 套接字读回经典帧应走普通数据帧路径。
    #[test]
    fn fd_socket_receives_classic_frame() {
        let id = StandardId::new(0x321).unwrap();
        let df = CanDataFrame::new(id, &[9, 8, 7]).unwrap();
        let frame = convert_any_frame(CanAnyFrame::Normal(df), fixed_ts()).unwrap();

        assert!(!frame.is_fd());
        assert_eq!(frame.id().raw_id(), 0x321);
        assert_eq!(frame.data(), &[9, 8, 7]);
    }

    // ---- ID 转换 ----

    /// ID 双向转换应保持原始值与扩展标志。
    #[test]
    fn id_bidirectional_conversion() {
        let std_id = CanId::new_standard(0x7FF).unwrap();
        assert_eq!(
            from_socketcan_id(to_socketcan_id(std_id).unwrap()).unwrap(),
            std_id
        );

        let ext_id = CanId::new_extended(0x1FFF_FFFF).unwrap();
        assert_eq!(
            from_socketcan_id(to_socketcan_id(ext_id).unwrap()).unwrap(),
            ext_id
        );

        // 0x800 及以下的标准值应保持标准而非误判为扩展
        let low_ext = CanId::new_extended(0x100).unwrap();
        match to_socketcan_id(low_ext).unwrap() {
            Id::Extended(_) => {}
            _ => panic!("0x100 扩展 ID 被错误转换"),
        }
    }

    // ---- 写方向转换 ----

    /// 经典写转换:标准帧 → socketcan 数据帧,ID / 数据一致。
    #[test]
    fn write_classic_standard_frame() {
        let id = CanId::new_standard(0x123).unwrap();
        let frame = CanFrame::new(id, vec![1, 2, 3]).unwrap();
        match to_socketcan_classic(&frame).unwrap() {
            socketcan::CanFrame::Data(df) => {
                assert_eq!(from_socketcan_id(df.id()).unwrap(), id);
                assert_eq!(df.data(), &[1, 2, 3]);
            }
            other => panic!("期望数据帧, 实际 {other:?}"),
        }
    }

    /// 经典写转换:远程帧 → socketcan 远程帧。
    #[test]
    fn write_classic_remote_frame() {
        let id = CanId::new_standard(0x456).unwrap();
        let mut frame = CanFrame::new(id, Vec::new()).unwrap();
        frame.set_remote(true);
        match to_socketcan_classic(&frame).unwrap() {
            socketcan::CanFrame::Remote(rf) => {
                assert_eq!(from_socketcan_id(rf.id()).unwrap(), id);
            }
            other => panic!("期望远程帧, 实际 {other:?}"),
        }
    }

    /// FD 写转换:FD 帧 → socketcan FD 帧,brs / esi 标志保留。
    #[test]
    fn write_fd_frame() {
        let id = CanId::new_extended(0x1F).unwrap();
        let frame = CanFrame::new_fd(id, vec![0x55; 64], true, true).unwrap();
        match to_socketcan_any(&frame).unwrap() {
            CanAnyFrame::Fd(fdf) => {
                assert!(fdf.is_brs());
                assert!(fdf.is_esi());
                assert_eq!(fdf.data(), &[0x55; 64]);
            }
            other => panic!("期望 FD 帧, 实际 {other:?}"),
        }
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// 创建唯一的临时目录 (前缀含测试标签与进程号,避免并行测试互撞)。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "can-socketcan-discover-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 在临时 net 目录中创建 fake 接口目录 (写 `type` / `operstate` / `uevent`)。
    fn add_iface(net_dir: &Path, name: &str, ty: &str, operstate: &str) -> PathBuf {
        let dir = net_dir.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("type"), format!("{ty}\n")).unwrap();
        fs::write(dir.join("operstate"), format!("{operstate}\n")).unwrap();
        fs::write(dir.join("uevent"), format!("INTERFACE={name}\nIFINDEX=1\n")).unwrap();
        dir
    }

    /// 扫描应识别 vcan / can / slcan,排除 eth;`available` 取自 `operstate`。
    #[test]
    fn scan_sysfs_detects_can_ifaces_and_skips_eth() {
        let net = temp_dir("mock");
        add_iface(&net, "vcan0", "280", "up");
        add_iface(&net, "can0", "280", "down");
        add_iface(&net, "slcan1", "280", "up");
        add_iface(&net, "eth0", "1", "up");

        let devices = scan_sysfs(&net);
        assert_eq!(devices.len(), 3);
        // 结果按接口名排序
        assert_eq!(devices[0].id, "can0");
        assert_eq!(devices[1].id, "slcan1");
        assert_eq!(devices[2].id, "vcan0");

        let vcan0 = &devices[2];
        assert_eq!(vcan0.kind, DeviceKind::SocketCan);
        assert_eq!(vcan0.driver, "socketcan");
        assert_eq!(vcan0.details.model, "SocketCAN");
        assert!(vcan0.available);
        assert!(!devices[0].available, "can0 operstate=down 应标记不可用");
    }

    /// 目录不存在时返回空列表,不 panic。
    #[test]
    fn scan_sysfs_missing_dir_returns_empty() {
        let net = std::env::temp_dir().join("can-socketcan-does-not-exist-xyz");
        let devices = scan_sysfs(&net);
        assert!(devices.is_empty(), "目录不存在应返回空列表且不 panic");
    }

    /// 空目录返回空列表。
    #[test]
    fn scan_sysfs_empty_dir_returns_empty() {
        let net = temp_dir("empty");
        assert!(scan_sysfs(&net).is_empty());
    }

    /// `type` 文件缺失时按接口名前缀回退识别。
    #[test]
    fn scan_sysfs_prefix_fallback_when_type_missing() {
        let net = temp_dir("fallback");
        let d = net.join("can5");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("operstate"), "up\n").unwrap();
        let e = net.join("ether0");
        fs::create_dir_all(&e).unwrap();
        fs::write(e.join("operstate"), "up\n").unwrap();

        let devices = scan_sysfs(&net);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "can5");
    }

    /// 硬件驱动符号链接存在时,`driver` 应取链接目标 basename。
    #[test]
    fn scan_sysfs_reads_real_hw_driver_from_symlink() {
        let net = temp_dir("driver");
        let d = add_iface(&net, "can0", "280", "up");
        let driver_link = d.join("device").join("driver");
        fs::create_dir_all(driver_link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/sys/bus/spi/drivers/mcp251x", &driver_link).unwrap();

        let devices = scan_sysfs(&net);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].driver, "mcp251x");
    }

    /// 本机实扫:有 CAN 接口则列表非空,无则空列表;均不 panic。
    #[test]
    fn list_devices_scans_real_sysfs_without_panic() {
        let devices = SocketCanDiscoverer::list_devices();
        let net = Path::new(SYSFS_NET_DIR);
        let has_can = net
            .read_dir()
            .map(|it| it.flatten().any(|e| is_can_interface(&e.path())))
            .unwrap_or(false);
        if has_can {
            assert!(
                devices.iter().any(|d| d.kind == DeviceKind::SocketCan),
                "本机存在 CAN 接口, 应被列出"
            );
        } else {
            assert!(devices.is_empty(), "本机无 CAN 接口, 应为空列表");
        }
    }
}

#[cfg(all(test, feature = "vcan-test"))]
mod vcan_tests {
    use super::*;
    use can_types::BackendConfig;

    /// vcan0 上写入标准帧后,另一后端应能读回 (loopback)。
    #[test]
    fn vcan_write_read_loopback() {
        let cfg = BackendConfig::SocketCan {
            iface: "vcan0".into(),
            fd: false,
        };
        let mut writer = SocketCanBackend::open(&cfg).expect("打开 vcan0 写端失败");
        let mut reader = SocketCanBackend::open(&cfg).expect("打开 vcan0 读端失败");

        let id = CanId::new_standard(0x5A5).unwrap();
        let frame = CanFrame::new(id, vec![0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        writer.write_frame(&frame).expect("写帧失败");

        let got = reader.read_frame(Duration::from_secs(2)).expect("读帧超时");
        assert_eq!(got.id(), id);
        assert_eq!(got.data(), &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    /// 无帧时读超时应返回 [`CanError::Timeout`]。
    #[test]
    fn vcan_read_timeout() {
        let cfg = BackendConfig::SocketCan {
            iface: "vcan0".into(),
            fd: false,
        };
        let mut backend = SocketCanBackend::open(&cfg).expect("打开 vcan0 失败");
        let res = backend.read_frame(Duration::from_millis(50));
        assert_eq!(res, Err(CanError::Timeout));
    }

    /// FD 模式打开 vcan0 并读写 FD 帧。
    #[test]
    fn vcan_fd_write_read_loopback() {
        let cfg = BackendConfig::SocketCan {
            iface: "vcan0".into(),
            fd: true,
        };
        let mut writer = SocketCanBackend::open(&cfg).expect("打开 vcan0 FD 写端失败");
        let mut reader = SocketCanBackend::open(&cfg).expect("打开 vcan0 FD 读端失败");

        let id = CanId::new_extended(0x1F).unwrap();
        let frame = CanFrame::new_fd(id, vec![0xAA; 32], true, false).unwrap();
        writer.write_frame(&frame).expect("写 FD 帧失败");

        let got = reader
            .read_frame(Duration::from_secs(2))
            .expect("读 FD 帧超时");
        assert_eq!(got.id(), id);
        assert_eq!(got.data(), &[0xAA; 32]);
        assert!(got.is_fd());
    }
}
