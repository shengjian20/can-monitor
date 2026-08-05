//! # can-monitor — CAN 监控终端应用
//!
//! 基于 ratatui 的 CAN 总线监控工具, 支持 SocketCAN / USBCAN 后端,
//! 提供实时报文流显示、协议解析与帧过滤。

use std::process;
use std::sync::{Arc, Mutex};

use can_types::{BackendConfig, BackendKind, CanBackend};
use can_monitor::bus::MonitorBus;
use can_monitor::classifier::FrameClassifier;
use can_monitor::filter::FrameFilter;
use can_monitor::tui::app::{parse_args, App};

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

    match cli.backend.as_str() {
        "socketcan" => {
            let config = BackendConfig::SocketCan {
                iface: cli.iface,
                fd: cli.fd,
            };
            let backend = can_socketcan::SocketCanBackend::open(&config)
                .map_err(|e| format!("打开 SocketCAN 后端失败: {e}"))?;
            bus.start_reader(backend, Arc::clone(&classifier), BackendKind::SocketCan);
        }
        "usbvci" => {
            let config = BackendConfig::UsbVci {
                device_type: can_usbvci::VCI_USBCAN2,
                channel: 0,
            };
            let backend = can_usbvci::UsbVciBackend::open(&config)
                .map_err(|e| format!("打开 USBCAN 后端失败: {e}"))?;
            bus.start_reader(backend, Arc::clone(&classifier), BackendKind::UsbVci);
        }
        "none" => {
            // 测试模式: 不启动 reader, TUI 直接可用。
        }
        other => {
            return Err(format!("未知后端: {other} (可选: socketcan, usbvci, none)").into());
        }
    }

    // 日志文件 (可选, Task 13 已实现 logger, 此处预留接口)。
    if let Some(_log_path) = &cli.log_file {
        // TODO: 接入 CandumpLogger (Task 20 集成)。
    }

    let filter = FrameFilter::new();
    let mut app = App::new(bus, classifier, rx, err_rx, filter);
    app.run()?;

    Ok(())
}
