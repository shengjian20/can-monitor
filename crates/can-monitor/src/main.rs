//! # can-monitor — CAN 监控终端应用
//!
//! 基于 ratatui 的 CAN 总线监控工具, 支持 SocketCAN / USBCAN 后端,
//! 提供实时报文流显示、协议解析与帧过滤。

use std::process;
use std::sync::{Arc, Mutex};

use can_monitor::tui::app::App;
use can_monitor_core::bus::MonitorBus;
use can_monitor_core::classifier::FrameClassifier;
use can_monitor_core::cli::{parse_args, CliArgs};
use can_monitor_core::filter::FrameFilter;
use can_monitor_core::logger::CandumpLogger;
use can_types::{BackendConfig, BackendKind, CanBackend};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cli) = parse_args(args) else {
        process::exit(0);
    };

    if let Err(e) = run(cli) {
        eprintln!("错误: {e}");
        process::exit(1);
    }
}

fn run(cli: CliArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (bus, rx, err_rx) = MonitorBus::new();
    // Arc 共享: TUI App 与 Web 服务 (--web-write) 持有同一总线实例。
    let bus = Arc::new(bus);
    let classifier = Arc::new(Mutex::new(FrameClassifier::default()));

    let (backend_name, iface_name) = match cli.backend.as_str() {
        "socketcan" => {
            let iface = cli.iface.clone();
            let config = BackendConfig::SocketCan {
                iface: cli.iface,
                fd: cli.fd,
            };
            let backend = can_socketcan::SocketCanBackend::open(&config)
                .map_err(|e| format!("打开 SocketCAN 后端失败: {e}"))?;
            bus.start_reader(backend, Arc::clone(&classifier), BackendKind::SocketCan)
                .map_err(|e| format!("启动 SocketCAN reader 失败: {e}"))?;
            ("SocketCAN".to_string(), iface)
        }
        "none" => ("None".to_string(), cli.iface),
        spec if spec == "usbvci" || spec.starts_with("usbvci:") => {
            // 支持 `--backend usbvci` / `usbvci:21` / `usbvci:0:21` 三种形式
            // (显式指定设备类型), 见 parse_usbvci_backend。
            let (device_index, device_type) = parse_usbvci_backend(spec)?;
            let config = BackendConfig::UsbVci {
                // device_type 为探测首候选 (0=未指定); 后端无 find 探测,
                // 依次尝试 [本值, 2E_U, USBCAN2] 去重, 首个成功即有效类型。
                device_type,
                device_index,
                channel: 0,
            };
            let backend = can_usbvci::UsbVciBackend::open(&config)
                .map_err(|e| format!("打开 USBCAN 后端失败: {e}"))?;
            bus.start_reader(backend, Arc::clone(&classifier), BackendKind::UsbVci)
                .map_err(|e| format!("启动 USBCAN reader 失败: {e}"))?;
            ("USBCAN".to_string(), cli.iface)
        }
        other => {
            return Err(format!("未知后端: {other} (可选: socketcan, usbvci, none)").into());
        }
    };

    let filter = FrameFilter::new();
    let mut app = App::new(Arc::clone(&bus), rx, err_rx, filter);
    app.set_backend_name(backend_name);
    app.set_iface_name(iface_name);

    if let Some(log_path) = &cli.log_file {
        let path = std::path::Path::new(log_path);
        let logger = CandumpLogger::new(path).map_err(|e| format!("打开日志文件失败: {e}"))?;
        app.set_logger(logger);
    }

    // Web 服务: 仅 `--web-write` 时启动; `--web-port` 提供时先校验 (Metis 安全锁定)。
    // 校验总是执行 (即便未启动服务), 非法地址立即报错退出。
    if cli.web_write || cli.web_port.is_some() {
        let addr_str = cli.web_port.as_deref().unwrap_or("127.0.0.1:8080");
        let addr = can_monitor_server::parse_bind_addr(addr_str)
            .map_err(|e| format!("--web-port 无效: {e}"))?;
        if cli.web_write {
            spawn_web_server(addr, Arc::clone(&bus), cli.web_write);
        }
    }

    app.run()?;

    // 干净退出: 冲刷日志缓冲并关闭文件, 避免丢缓冲 (Task 21 实测发现)。
    app.close_logger()?;
    Ok(())
}

/// 解析 `--backend usbvci[:index][:type]` 形式的显式设备参数。
///
/// - `usbvci`      → 索引 0, 类型默认 [`VCI_USBCAN_2E_U`] (21, 本机 2E-U 设备);
///   后端按 [21, 4] 候选探测, 仍可回退。
/// - `usbvci:21`   → 索引 0, 类型 21 (单段 = 显式指定类型)。
/// - `usbvci:0:21` → 索引 0, 类型 21 (两段 = [索引, 类型])。
///
/// @param spec `--backend` 参数原文 (前缀必为 "usbvci")。
/// @return (device_index, device_type); 段非数字 / 段数超限返回错误描述。
fn parse_usbvci_backend(spec: &str) -> Result<(u32, u32), String> {
    let segments: Vec<&str> = spec.split(':').collect();
    if segments.len() > 3 {
        return Err(format!(
            "非法 usbvci 参数: {spec} (格式: usbvci[:index][:type])"
        ));
    }
    let mut index = 0u32;
    let mut device_type = can_usbvci::VCI_USBCAN_2E_U;
    match segments.get(1) {
        None => {}
        Some(seg) if segments.len() == 2 => {
            device_type = seg
                .parse()
                .map_err(|_| format!("非法设备类型: {seg} (--backend {spec})"))?;
        }
        Some(seg) => {
            index = seg
                .parse()
                .map_err(|_| format!("非法设备索引: {seg} (--backend {spec})"))?;
            let type_seg = segments[2];
            device_type = type_seg
                .parse()
                .map_err(|_| format!("非法设备类型: {type_seg} (--backend {spec})"))?;
        }
    }
    Ok((index, device_type))
}

/// 在后台线程启动 tokio runtime + axum Web 服务。
///
/// 独立多线程 runtime (enable_all) 与 TUI 主线程解耦; 服务绑定失败仅打印
/// 错误, 不中断 TUI (WS / REST / 静态文件同一 Router)。
///
/// @param addr          监听地址 (已通过回环校验)。
/// @param bus           共享消息总线 (与 TUI 同持 Arc)。
/// @param write_enabled 写门控 (--web-write 时为真)。
fn spawn_web_server(addr: std::net::SocketAddr, bus: Arc<MonitorBus>, write_enabled: bool) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("创建 Web 服务 tokio runtime 失败: {e}");
                return;
            }
        };
        if let Err(e) = rt.block_on(can_monitor_server::serve(addr, bus, write_enabled)) {
            eprintln!("Web 服务错误 ({addr}): {e}");
        }
    });
}
