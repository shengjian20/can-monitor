# Changelog

can-monitor 的变更记录, 遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/), 版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。发布流程见 [docs/release-process.md](docs/release-process.md)。

## [Unreleased]

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

[Unreleased]: https://github.com/shengjian20/can-monitor/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/shengjian20/can-monitor/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/shengjian20/can-monitor/releases/tag/v0.1.0
