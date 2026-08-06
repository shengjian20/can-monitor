# Changelog

can-monitor 的变更记录, 遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/), 版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。发布流程见 [docs/release-process.md](docs/release-process.md)。

## [Unreleased]

## [0.1.4] - 2026-08-06

### Fixed

- **USB-CAN 驱动换 libusbcan_v351 (固件 3.51 专用)**: x86_64 的 `libcontrolcan.so` 由 SDK V1.45 原版替换为用户提供的 v351 驱动 — 修复 CAN-Linux 设备 (固件 3.51) 用 V1.45 库打开失败 (`Device or resource busy`) 的问题;真机实测 OpenDevice(4)/InitCAN/StartCAN/Transmit(0x181, SendType=1) 全部返回 1, `--backend usbvci` 打开成功 (md5 `2ec9b05066ba44b67cfec7d535f99763`)。**依赖 `libusb-1.0`**: 目标系统需已安装 `libusb-1.0.so.0` (Ubuntu: `apt install libusb-1.0-0`)。aarch64 无 v351 arm64 版, 保持 SDK V1.45 库

## [0.1.3] - 2026-08-06

### Fixed

- **USB 权限 udev 规则对齐官方**: 规则改为厂商官方格式 `SUBSYSTEMS`/`ATTRS`(双 S 匹配父设备) + `GROUP="users"` + `MODE="0777"`, 并保留 `ID_MM_DEVICE_IGNORE=1` (f5ce9b9)
- **third_party 厂商库换回 SDK V1.45 原版**: `libcontrolcan.so` 原为异版, 替换为 Linux 资料包 V1.45 原版库 (x86_64/aarch64, md5 双验证) (456c982)
- **CAN-Linux 设备类型对齐官方样例 VCI_USBCAN2**: 默认设备类型由 2E_U(21) 改回 USBCAN2(4), 与厂商官方样例一致; 探测顺序 [配置, 4, 21] 去重 (c1cfc6b)
- **SendType=1 单次发送 + Receive 批量 2500**: 发送帧显式 `SendType=SEND_TYPE_SINGLE(1)` (官方 2.1.3 建议), Receive 批量接收数组 64→2500 (c1cfc6b)

### Changed

- **arm64 CLI/Web 构建 glibc 2.17 双兼容**: cli-web job 的 aarch64 产物改用 `cargo-zigbuild` + glibc 2.17 目标, 单包同时兼容 Ubuntu 16.04 (glibc 2.23) 与 24.04 (glibc 2.39); 构建后 readelf 断言 max GLIBC < 2.24, 超限 fail (abd6ff7)

### Docs

- **Linux arm64 支持矩阵**: 16.04 用 Web 形态 GUI (浏览器访问包内 Web 界面), 24.04 用原生 Tauri GUI (webkit2gtk-4.1) (45147a0)

## [0.1.2] - 2026-08-06

### Added

- **CLI / Web 二进制发布资产**: 发布矩阵新增 `cli-web` job, 三平台产出 CLI + Web 二进制压缩包 (tar.gz / zip), 随 Release 一同发布, 无 GUI 场景可直接下载二进制使用

### Fixed

- **USBCAN 2E_U 设备类型自动探测**: 设备类型改由板卡信息自动识别 (2E_U = 21), 不再硬编码默认类型 (d284b51)
- **USBCAN open 路径修复**: 移除 open 前的 find 自冲突 (避免 usbfs claim 冲突), 设备号显式写入 device_id (3a6ab94)

## [0.1.1] - 2026-08-06

### Added

- **arm64 (aarch64) 原生构建**: 发布矩阵新增 `ubuntu-24.04-arm` runner, Linux 同时产出 x86_64 与 aarch64 的 AppImage / deb / rpm

### Fixed

- **AppImage / GUI 内厂商库加载修复**: 打包应用内 `libcontrolcan.so` 加载失败, 增加 Linux Tauri 资源目录回退 (随安装包分发, USB-CAN 开箱即用)
- **USB-CAN 设备枚举权限**: udev 规则入库, 按文档 (docs/ssh-x11.md) 配置后无需 root 即可枚举 USB-CAN 设备

## [0.1.0] - 2026-08-06

跨平台 v2 重构 (can-monitor-tui → can-monitor):

- **can-monitor-core 分层**: 核心流独立为 crate (bus / 单次分类 / 过滤 / 日志 / fan-out 广播 / CLI 解析), TUI / Web / GUI 三形态共享同一核心
- **TUI + Web + GUI 三形态**: 终端 TUI (ratatui) / Web (axum REST + WebSocket + React SPA) / 桌面 GUI (Tauri v2)
- **SocketCAN + USBCAN 设备发现**: SocketCAN 经 sysfs 扫描, 周立功 USB-CAN 经 VCI 动态加载枚举, can-devices 聚合统一列表
- **CANopen + J1939 解析**: CANopen (CiA 301) 与 SAE J1939 (含 TP 多包重组)
- **三平台 CI**: Ubuntu / Windows / macOS 交叉检查 + Linux 全量门禁 (test / clippy / fmt / core 纯度)
- **Docker 镜像**: ghcr.io 自动构建发布

[Unreleased]: https://github.com/shengjian20/can-monitor/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/shengjian20/can-monitor/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/shengjian20/can-monitor/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/shengjian20/can-monitor/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/shengjian20/can-monitor/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/shengjian20/can-monitor/releases/tag/v0.1.0
