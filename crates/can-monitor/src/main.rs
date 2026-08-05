//! # can-monitor — CAN 监控终端应用
//!
//! 基于 ratatui 的 CAN 总线监控工具, 支持 SocketCAN / USBCAN 后端,
//! 提供实时报文流显示、协议解析与帧过滤。

use std::process;
use std::sync::{Arc, Mutex};

use can_monitor::tui::app::{parse_args, App};
use can_monitor_core::bus::MonitorBus;
use can_monitor_core::classifier::FrameClassifier;
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

fn run(cli: can_monitor::tui::app::CliArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (bus, rx, err_rx) = MonitorBus::new();
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
        "usbvci" => {
            let config = BackendConfig::UsbVci {
                device_type: can_usbvci::VCI_USBCAN2,
                channel: 0,
            };
            let backend = can_usbvci::UsbVciBackend::open(&config)
                .map_err(|e| format!("打开 USBCAN 后端失败: {e}"))?;
            bus.start_reader(backend, Arc::clone(&classifier), BackendKind::UsbVci)
                .map_err(|e| format!("启动 USBCAN reader 失败: {e}"))?;
            ("USBCAN".to_string(), cli.iface)
        }
        "none" => ("None".to_string(), cli.iface),
        other => {
            return Err(format!("未知后端: {other} (可选: socketcan, usbvci, none)").into());
        }
    };

    let filter = FrameFilter::new();
    let mut app = App::new(bus, classifier, rx, err_rx, filter);
    app.set_backend_name(backend_name);
    app.set_iface_name(iface_name);

    if let Some(log_path) = &cli.log_file {
        let path = std::path::Path::new(log_path);
        let logger = CandumpLogger::new(path).map_err(|e| format!("打开日志文件失败: {e}"))?;
        app.set_logger(logger);
    }

    app.run()?;

    // 干净退出: 冲刷日志缓冲并关闭文件, 避免丢缓冲 (Task 21 实测发现)。
    app.close_logger()?;
    Ok(())
}
