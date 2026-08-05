//! build.rs — can-usbvci 平台化链接配置 (Task 6 平台 gating; Task 10 动态加载默认)
//!
//! build 脚本按 `target_os` 分支:
//!
//! - `linux`: 默认**动态加载模式** (Task 10) —— 构建期不再链接 `libcontrolcan.so`,
//!   由运行时 libloading 按名解析 (resolve_library: `CAN_USBVCI_LIB` → exe 同目录 →
//!   LD_LIBRARY_PATH/rpath)。仅当 `CAN_USBVCI_LINK_MODE=static` 时保留静态链接
//!   (`libcontrolcan.a` + libusb-0.1 + pthread) 并设 `usbvci_static_link` cfg,
//!   后端 RealVciOps 此时直接引用 extern 块符号。按目标架构在
//!   `third_party/controlcan/` 中选择供应商库目录 (x86_64 / aarch64);
//!   其他架构 (如 armv7 / riscv64) 打 warning 并跳过链接。
//! - `windows`: 不链接 — 运行时经 LoadLibrary 动态加载 `ControlCAN.dll`, 构建期无需供应商库。
//! - `macos`: 不链接 — macOS 无供应商库, 作为 mock 逃生舱 (仅 `mock` feature 测试可用)。
//! - 未知 OS: 打 warning 并跳过链接。
//!
//! 链接模式由 `CAN_USBVCI_LINK_MODE` 环境变量 (或 `--cfg CAN_USBVCI_LINK_MODE="..."` RUSTFLAGS,
//! 经 `CARGO_CFG_CAN_USBVCI_LINK_MODE` 传递) 控制, 取值:
//!
//! - `dynamic` (默认): 不链接任何 vendor 库, 运行时 libloading 加载 `.so`/`.dll`。
//!   `so` 作为历史别名同样进入该模式 (旧脚本/文档写 `so`, 行为一致)。
//! - `static`: 链接 `libcontrolcan.a` + 外部旧版 `libusb` (0.1 API: `usb_init` /
//!   `usb_find_busses` / `usb_bulk_read` 等) + `pthread`, 并设 `usbvci_static_link` cfg
//!   (backend 直接引用 extern 符号)。需要系统安装 libusb-dev
//!   (提供 `/usr/include/usb.h`, 不是 libusb-1.0-0-dev)。
//!
//! 供应商库路径: `<workspace-root>/third_party/controlcan/`, 由 Task 5 的
//! `scripts/fetch-vendor.sh` 抓取。目录布局 (对称):
//! - `third_party/controlcan/aarch64/`   → aarch64 (ARM平台/64bit)
//! - `third_party/controlcan/x86_64/`    → x86_64 (x86平台/64位linux系统)

use std::env;
use std::path::{Path, PathBuf};

/// 解析 Linux 目标架构对应的供应商库子目录; 未知架构返回 `None` (不 panic)。
fn vendor_lib_dir(vendor_root: &Path) -> Option<PathBuf> {
    // cargo-zigbuild 的 `aarch64-unknown-linux-gnu.2.23` 会在 TARGET 里带 glibc 后缀,
    // 先剥掉 `.N.N` 后缀再匹配前缀。
    let target = env::var("TARGET").expect("cargo 未设置 TARGET");
    let arch = target.split('.').next().unwrap_or(&target);

    if arch.starts_with("x86_64") {
        Some(vendor_root.join("x86_64"))
    } else if arch.starts_with("aarch64") {
        Some(vendor_root.join("aarch64"))
    } else {
        None
    }
}

fn main() {
    // 声明自定义 cfg, 避免未设置时 rustc 的 unexpected_cfgs 告警 (1.80+ 默认开启)。
    println!("cargo:rustc-check-cfg=cfg(usbvci_static_link)");

    // mock feature: 不链接真实 controlcan 库 (cargo 对启用的 feature 注入 CARGO_FEATURE_<NAME> 环境变量),
    // 使 `cargo test --features mock` 在没有供应商库 / 无硬件的主机上也能构建运行 (MockVciOps 桩不引用 FFI 符号)。
    if env::var("CARGO_FEATURE_MOCK").is_ok() {
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_MOCK");
        println!("can-usbvci: mock feature 已启用, 跳过真实 controlcan 库链接");
        return;
    }

    // 无论平台都保留链接模式 env 的 rerun 跟踪。
    println!("cargo:rerun-if-env-changed=CAN_USBVCI_LINK_MODE");

    // 平台 gating: 按 target_os 分支, 任何平台都不 panic。
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        // Windows: Task 10 运行时动态加载 (LoadLibrary), 构建期不链接任何供应商库。
        "windows" => {
            println!("can-usbvci: Windows 平台 → 动态加载模式 (Task 10), 构建期不链接");
        }
        // macOS: 无供应商库, mock 逃生舱 — 仅 mock feature 测试可用, 构建期不链接。
        "macos" => {
            println!("can-usbvci: macOS 平台 → mock 逃生舱, 构建期不链接 (仅 mock 测试可用)");
        }
        // Linux: 默认动态加载, static 模式保留静态链接。
        "linux" => link_linux(),
        // 未知平台: warning + 不链接。
        other => {
            let display = if other.is_empty() {
                env::var("TARGET").unwrap_or_else(|_| "<未知>".to_owned())
            } else {
                other.to_owned()
            };
            println!(
                "cargo:warning=can-usbvci: 未知目标平台 {display:?}, 跳过 controlcan 链接 \
                 (真实设备仅支持 Linux x86_64/aarch64; 无硬件测试请用 mock feature)"
            );
        }
    }
}

/// Linux 链接入口: 按架构找供应商库, 按链接模式处理。
fn link_linux() {
    // 供应商库根目录 = workspace 根 (CARGO_MANIFEST_DIR/../..) + third_party/controlcan
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo 未设置 CARGO_MANIFEST_DIR"));
    let vendor_root = manifest_dir
        .join("..")
        .join("..")
        .join("third_party")
        .join("controlcan");

    // 未知架构: warning + 不链接 (不 panic)。
    let Some(lib_dir) = vendor_lib_dir(&vendor_root) else {
        let target = env::var("TARGET").unwrap_or_else(|_| "<未知>".to_owned());
        println!(
            "cargo:warning=can-usbvci: Linux 目标架构 {target:?} 无供应商库 (仅提供 x86_64 与 aarch64), \
             跳过链接; 无硬件测试请用 mock feature"
        );
        return;
    };
    // 转成绝对路径: 动态加载模式会把该目录写进 rpath (帮助测试二进制找到 .so),
    // 相对路径在运行时按 CWD 解析, 不可靠。注意 std::path::absolute 不会消解 `..`, 须用
    // canonicalize (目录存在, 一定能解析)。
    let lib_dir = std::fs::canonicalize(&lib_dir).unwrap_or(lib_dir);
    println!("cargo:rerun-if-changed={}", lib_dir.display());

    // 链接模式: 环境变量优先, 其次 CARGO_CFG_* (来自 RUSTFLAGS --cfg), 默认 "dynamic"
    let link_mode = env::var("CAN_USBVCI_LINK_MODE")
        .or_else(|_| env::var("CARGO_CFG_CAN_USBVCI_LINK_MODE"))
        .unwrap_or_else(|_| "dynamic".to_owned());

    // 非法取值不 panic: 打印 warning 并回退默认 dynamic (build.rs 任何平台不 panic)。
    let effective_mode = match link_mode.as_str() {
        // 静态链接保留: 链接 libcontrolcan.a + libusb(0.1) + pthread, 并设 cfg 让
        // backend 直接引用 extern 块符号 (不依赖运行时 .so)。
        "static" => {
            link_static(&lib_dir);
            println!("cargo:rustc-cfg=usbvci_static_link");
            "static"
        }
        // 动态加载 (默认): 不 emit link-lib, 由 libloading 运行时解析 .so。
        // "so" 是历史别名 (旧文档默认值), 语义一致。
        "dynamic" | "so" => {
            prepare_dynamic(&lib_dir);
            if link_mode == "so" {
                "so (历史别名 → 动态加载)"
            } else {
                "dynamic"
            }
        }
        other => {
            println!(
                "cargo:warning=can-usbvci: CAN_USBVCI_LINK_MODE 取值无效: {other:?} \
                 (仅支持 \"dynamic\" / \"static\", 历史值 \"so\" 等同 dynamic), 回退默认 dynamic"
            );
            prepare_dynamic(&lib_dir);
            "dynamic"
        }
    };

    println!(
        "can-usbvci: 链接模式={effective_mode}, 供应商库目录={}",
        lib_dir.display()
    );
}

/// 动态加载模式: 不 emit 任何 link-lib (构建期不依赖 vendor 库存在); 若 .so 在仓库内,
/// 把目录写进 rpath 帮助本 crate 的测试二进制运行时找到库。仅本 crate 自身目标生效,
/// 依赖方 (如 can-monitor) 的 rpath 由各自 build.rs 注入。
fn prepare_dynamic(lib_dir: &Path) {
    let so = lib_dir.join("libcontrolcan.so");
    if so.is_file() {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    }
}

fn fail_missing(kind: &str, path: &Path) -> ! {
    panic!(
        "can-usbvci: 未找到 controlcan {kind} 库: {}\n\
         请先执行 scripts/fetch-vendor.sh 从 SDK 拷贝供应商库 (Linux资料包V1.45/二次开发库文件)。\n\
         或改用默认动态加载模式 (CAN_USBVCI_LINK_MODE 留空/dynamic, 运行时加载 .so)。",
        path.display()
    );
}

/// `static` 模式: 静态库 + 外部 libusb(0.1) + pthread。
fn link_static(lib_dir: &Path) {
    let a = lib_dir.join("libcontrolcan.a");
    if !a.is_file() {
        fail_missing("静态(.a)", &a);
    }
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=controlcan");
    // 旧版 libusb-0.1 (提供 usb_init/usb_bulk_read 等符号), 与 libusb-1.0 符号不匹配, 不可混用。
    println!("cargo:rustc-link-lib=dylib=usb");
    println!("cargo:rustc-link-lib=dylib=pthread");
}
