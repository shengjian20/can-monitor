//! # CLI 参数解析
//!
//! 纯 std 手写命令行解析 (不依赖 clap), 供上层 UI 复用。
//! 参数面: `--backend <socketcan|usbvci[:index][:type]|none>`、`--iface <name>`、
//! `--fd`、`--log-file <path>`、`--web-write`、`--web-port <addr>`、
//! `--help` / `-h`。
//!
//! usbvci 支持显式设备参数: `--backend usbvci` (自动探测)、`usbvci:21`
//! (显式类型 21)、`usbvci:0:21` (索引 0 + 类型 21), 由 `can-monitor` 主程序
//! 解析, 本 crate 仅原样保存 `backend` 字符串。

/// CLI 参数解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliArgs {
    /// 后端类型: "socketcan" / "usbvci" / "none"。
    pub backend: String,
    /// SocketCAN 接口名。
    pub iface: String,
    /// 是否启用 CANFD。
    pub fd: bool,
    /// 日志文件路径 (可选)。
    pub log_file: Option<String>,
    /// 是否启用 Web 写模式 (同时启动 HTTP 服务, 默认关闭)。
    pub web_write: bool,
    /// Web 服务监听地址 (可选; 缺省用 127.0.0.1:8080)。
    pub web_port: Option<String>,
}

/// 默认 CLI 参数: 后端 none、接口 can0、非 FD、无日志文件、Web 写关闭。
impl Default for CliArgs {
    fn default() -> Self {
        Self {
            backend: "none".to_string(),
            iface: "can0".to_string(),
            fd: false,
            log_file: None,
            web_write: false,
            web_port: None,
        }
    }
}

/// 从参数迭代器解析 CLI 参数。
///
/// @param args 参数迭代器 (不含程序名)。
/// @return 解析结果 [`CliArgs`]; 遇到 `--help` 或无效参数返回 `None`。
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Option<CliArgs> {
    let mut result = CliArgs::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                return None;
            }
            "--backend" => {
                if let Some(val) = args.next() {
                    result.backend = val;
                }
            }
            "--iface" => {
                if let Some(val) = args.next() {
                    result.iface = val;
                }
            }
            "--fd" => {
                result.fd = true;
            }
            "--log-file" => {
                if let Some(val) = args.next() {
                    result.log_file = Some(val);
                }
            }
            "--web-write" => {
                result.web_write = true;
            }
            "--web-port" => {
                if let Some(val) = args.next() {
                    result.web_port = Some(val);
                }
            }
            _ => {
                eprintln!("未知参数: {arg}");
                print_usage();
                return None;
            }
        }
    }

    Some(result)
}

/// 打印 CLI 用法说明。
fn print_usage() {
    eprintln!(
        "用法: can-monitor [选项]\n\
         \n\
         选项:\n\
         --backend <socketcan|usbvci[:ind][:type]|none>  后端类型 (默认 none)\n\
         --iface <name>                     SocketCAN 接口名 (默认 can0)\n\
         --fd                               启用 CANFD\n\
         --log-file <path>                  日志文件路径\n\
         --web-write                        启用 Web 写模式 (同时启动 HTTP 服务; 默认只读)\n\
         --web-port <host:port>             Web 服务监听地址 (默认 127.0.0.1:8080; 仅限本机回环)\n\
         --help, -h                         显示此帮助"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI 参数解析: 默认值。
    #[test]
    fn parse_args_default() {
        let args: Vec<String> = vec![];
        let result = parse_args(args).unwrap();
        assert_eq!(result.backend, "none");
        assert_eq!(result.iface, "can0");
        assert!(!result.fd);
        assert_eq!(result.log_file, None);
        assert!(!result.web_write, "Web 写模式默认关闭 (安全)");
        assert_eq!(result.web_port, None);
    }

    /// CLI 参数解析: Web 标志 (--web-write / --web-port)。
    #[test]
    fn parse_args_web_flags() {
        let args: Vec<String> = vec![
            "--web-write".into(),
            "--web-port".into(),
            "127.0.0.1:9090".into(),
        ];
        let result = parse_args(args).unwrap();
        assert!(result.web_write);
        assert_eq!(result.web_port, Some("127.0.0.1:9090".into()));
    }

    /// CLI 参数解析: 完整参数。
    #[test]
    fn parse_args_full() {
        let args: Vec<String> = vec![
            "--backend".into(),
            "socketcan".into(),
            "--iface".into(),
            "vcan0".into(),
            "--fd".into(),
            "--log-file".into(),
            "/tmp/test.log".into(),
        ];
        let result = parse_args(args).unwrap();
        assert_eq!(result.backend, "socketcan");
        assert_eq!(result.iface, "vcan0");
        assert!(result.fd);
        assert_eq!(result.log_file, Some("/tmp/test.log".into()));
    }

    /// CLI 参数解析: --help 返回 None。
    #[test]
    fn parse_args_help() {
        let args: Vec<String> = vec!["--help".into()];
        assert!(parse_args(args).is_none());
    }

    /// CLI 参数解析: -h 返回 None。
    #[test]
    fn parse_args_h() {
        let args: Vec<String> = vec!["-h".into()];
        assert!(parse_args(args).is_none());
    }

    /// CLI 参数解析: 未知参数返回 None。
    #[test]
    fn parse_args_unknown() {
        let args: Vec<String> = vec!["--unknown".into()];
        assert!(parse_args(args).is_none());
    }
}
