//! build.rs — can-usbvci 平台化链接配置 (Task 6: 平台 gating, 任何平台不 panic)
//!
//! build 脚本按 `target_os` 分支:
//!
//! - `linux`: 走现有双链接模式 (`so` 默认 / `static` 可选)。按目标架构在
//!   `third_party/controlcan/` 中选择供应商库目录 (x86_64 / aarch64);
//!   其他架构 (如 armv7 / riscv64) 打 warning 并跳过链接。
//! - `windows`: 不链接 — Task 10 将改为运行时动态加载 (`LoadLibrary`), 构建期无需供应商库。
//! - `macos`: 不链接 — macOS 无供应商库, 作为 mock 逃生舱 (仅 `mock` feature 测试可用)。
//! - 未知 OS: 打 warning 并跳过链接。
//!
//! 链接模式由 `CAN_USBVCI_LINK_MODE` 环境变量 (或 `--cfg CAN_USBVCI_LINK_MODE="..."` RUSTFLAGS,
//! 经 `CARGO_CFG_CAN_USBVCI_LINK_MODE` 传递) 控制, 取值:
//!
//! - `so` (默认): 链接 `libcontrolcan.so`。该 .so 内嵌 libusb-0.1 全部符号,
//!   `readelf -d` 显示 NEEDED 仅 `libpthread.so.0` + `libc.so.6`, 因此无需任何外部依赖。
//! - `static`: 链接 `libcontrolcan.a` + 外部旧版 `libusb` (0.1 API: `usb_init` /
//!   `usb_find_busses` / `usb_bulk_read` 等) + `pthread`。需要系统安装 libusb-dev
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
        // Windows: Task 10 改为运行时动态加载 (LoadLibrary), 构建期不链接任何供应商库。
        "windows" => {
            println!("can-usbvci: Windows 平台 → 动态加载模式 (Task 10), 构建期不链接");
        }
        // macOS: 无供应商库, mock 逃生舱 — 仅 mock feature 测试可用, 构建期不链接。
        "macos" => {
            println!("can-usbvci: macOS 平台 → mock 逃生舱, 构建期不链接 (仅 mock 测试可用)");
        }
        // Linux: 走现有双链接模式。
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

/// Linux 链接入口: 按架构找供应商库, 按链接模式链接。
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
    // 转成绝对路径: 链接搜索路径与 rpath 都会原样写进产物, 相对路径在运行时按 CWD 解析,
    // 不可靠 (尤其跨目录启动二进制时)。注意 std::path::absolute 不会消解 `..`, 须用
    // canonicalize (库目录存在, 一定能解析)。
    let lib_dir = std::fs::canonicalize(&lib_dir).unwrap_or(lib_dir);
    println!("cargo:rerun-if-changed={}", lib_dir.display());

    // 链接模式: 环境变量优先, 其次 CARGO_CFG_* (来自 RUSTFLAGS --cfg), 默认 "so"
    let link_mode = env::var("CAN_USBVCI_LINK_MODE")
        .or_else(|_| env::var("CARGO_CFG_CAN_USBVCI_LINK_MODE"))
        .unwrap_or_else(|_| "so".to_owned());

    // 非法取值不再 panic: 打印 warning 并回退默认 so (本任务承诺 build.rs 任何平台不 panic)。
    let effective_mode;
    match link_mode.as_str() {
        "so" => {
            link_shared(&lib_dir);
            effective_mode = "so";
        }
        "static" => {
            link_static(&lib_dir);
            effective_mode = "static";
        }
        other => {
            println!(
                "cargo:warning=can-usbvci: CAN_USBVCI_LINK_MODE 取值无效: {other:?} \
                 (仅支持 \"so\" 或 \"static\"), 回退默认 so"
            );
            link_shared(&lib_dir);
            effective_mode = "so";
        }
    }

    println!(
        "can-usbvci: 链接模式={effective_mode}, 供应商库目录={}",
        lib_dir.display()
    );
}

fn fail_missing(kind: &str, path: &Path) -> ! {
    panic!(
        "can-usbvci: 未找到 controlcan {kind} 库: {}\n\
         请先执行 scripts/fetch-vendor.sh 从 SDK 拷贝供应商库 (Linux资料包V1.45/二次开发库文件)。\n\
         或改用 CAN_USBVCI_LINK_MODE=so 链接自带 libusb 符号的共享库。",
        path.display()
    );
}

/// `so` 模式: 自包含共享库, 无外部依赖。
fn link_shared(lib_dir: &Path) {
    let so = lib_dir.join("libcontrolcan.so");
    if !so.is_file() {
        fail_missing("共享(.so)", &so);
    }
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=controlcan");
    // 供应商 .so 位于仓库内非标准路径, 把目录写入 rpath, 使本 crate 自身目标
    // (如未来调用 VCI 函数的测试二进制) 运行时不依赖 ldconfig。仅 .so 模式需要。
    //
    // 重要: cargo 的 build script 链接参数只作用于发出指令的 crate 自身的目标
    // (cargo 源码 add_native_deps: 仅 LinkArgTarget::Cdylib 允许跨包传递, 见
    // rust-lang/cargo#9562)。因此这里的 rpath 不会传播到依赖方 (如 can-monitor)
    // 的最终二进制 —— 那由 can-monitor/build.rs 另行注入。
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
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
