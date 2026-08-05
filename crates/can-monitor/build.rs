//! build.rs — 为 can-monitor 二进制注入 libcontrolcan.so 的 rpath
//!
//! 为什么在这里: cargo 的 build script 链接参数只作用于发出指令的 crate 自身的目标,
//! 不会传播到依赖方 (cargo 源码 add_native_deps 仅对 LinkArgTarget::Cdylib 放行跨包,
//! 见 rust-lang/cargo#9562)。can-usbvci 是纯 lib crate, 它的 `rustc-link-arg` 无法把
//! rpath 写进本 crate 的最终二进制; 而本 crate (can-monitor) 自己有 bin 目标,
//! 在自己的 build.rs 里发出 `rustc-link-arg` 才能生效。
//!
//! 仅 `so` 链接模式需要 (static 模式把 controlcan 静态链入, 无运行时 .so 加载)。
//! 供应商库缺失时静默跳过 —— 真正的报错由 can-usbvci 的 build.rs 给出。

use std::env;
use std::path::PathBuf;

fn main() {
    // 供应商 .so 目录 = workspace 根 (CARGO_MANIFEST_DIR/../..) + third_party/controlcan,
    // 与 can-usbvci/build.rs 保持同一套 arch 规则。
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo 未设置 CARGO_MANIFEST_DIR"));
    let vendor_root = manifest_dir
        .join("..")
        .join("..")
        .join("third_party")
        .join("controlcan");

    // cargo-zigbuild 的 `aarch64-unknown-linux-gnu.2.23` 会带 glibc 后缀, 先剥掉再匹配前缀。
    let target = env::var("TARGET").expect("cargo 未设置 TARGET");
    let arch = target.split('.').next().unwrap_or(&target);
    let lib_dir = if arch.starts_with("x86_64") {
        vendor_root.join("x86_64")
    } else if arch.starts_with("aarch64") {
        vendor_root
    } else {
        return; // 非 x86_64/aarch64, 无供应商库, 跳过
    };

    // static 模式无运行时 .so 加载, 不需要 rpath。
    let link_mode = env::var("CAN_USBVCI_LINK_MODE")
        .or_else(|_| env::var("CARGO_CFG_CAN_USBVCI_LINK_MODE"))
        .unwrap_or_else(|_| "so".to_owned());
    if link_mode == "static" {
        return;
    }

    if !lib_dir.join("libcontrolcan.so").is_file() {
        return; // 供应商库未抓取, 跳过 (报错由 can-usbvci 的 build.rs 负责)
    }

    // 绝对路径: 相对 rpath 在运行时按 CWD 解析不可靠; canonicalize 消解 `..` 与符号链接。
    let lib_dir = std::fs::canonicalize(&lib_dir).unwrap_or(lib_dir);

    println!("cargo:rerun-if-env-changed=CAN_USBVCI_LINK_MODE");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
}
