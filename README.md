# can-monitor

[![CI](https://github.com/raw/can-monitor/actions/workflows/ci.yml/badge.svg)](https://github.com/raw/can-monitor/actions/workflows/ci.yml)
<!-- CI 徽章为占位: 仓库公开且 GitHub Actions 跑通后自动生效。若公开后仓库地址不同, 请同步替换 raw/can-monitor 为实际 owner/repo。 -->

English: [README.en.md](README.en.md)

跨平台的 CAN 总线监控工具,一套核心,三种形态:

- **TUI** — 终端实时监控 (ratatui),面向嵌入式调试与产线诊断
- **Web** — 浏览器界面 (React SPA + REST/WebSocket),本地回环服务
- **GUI** — 桌面应用 (Tauri v2),Web 界面的原生壳

双后端 (Linux SocketCAN / 周立功 USB-CAN),双协议解析 (CANopen CiA 301 / SAE J1939),支持设备自动发现、帧过滤、协议高亮、CANopen 下发与 candump 兼容日志。

> **截图**: 暂无。仓库公开后补 TUI / Web / GUI 三形态的实机截图。

## 功能特性

**三形态**

| 形态 | 技术栈 | 启动方式 |
|------|--------|----------|
| TUI | Rust + ratatui + crossterm | `cargo run -- --backend none` |
| Web | Rust (axum REST/WS) + React SPA | `cargo run -- --backend none --web-write` 后访问 `http://127.0.0.1:8080` |
| GUI | Tauri v2 (Web 前端 + Rust 后端) | `cd src-tauri && cargo tauri dev` |

> **远程使用 (无屏幕工控机)**: 本机无屏幕, 通过 SSH 远程操作三种形态 — GUI 走 X11 转发, TUI 普通 SSH 即用, Web 走端口转发 (无头最佳)。见 [docs/ssh-x11.md](docs/ssh-x11.md)。

**总线后端**

- **Linux SocketCAN**: 经典 CAN + CAN FD (`--fd`),经 `/sys/class/net` 自动发现 `can0` / `vcan0` 等接口
- **周立功 USB-CAN**: USBCAN-I/II、USBCAN-E-U/2E-U 等 VCI 兼容设备,`ControlCAN.dll` / `libcontrolcan.so` 运行时动态加载,支持热插拔自动重连 (≥2s 重枚举,最多 5 次)。**仅经典 CAN,不支持 CANFD** (硬件限制)
- **无设备模式** (`--backend none`): 无需任何硬件即可跑通 TUI / Web 界面与调试

**协议解析**

- 11 位标准帧 → **CANopen** (CiA 301): NMT / SDO / PDO / EMCY / SYNC / TIME / 心跳,含节点健康监控与下发面板
- 29 位扩展帧 → **J1939**: PGN 位域解析 + TP.BAM 多包重组 (最大 1785 字节,DM1/DM2 诊断分类)
- 其余帧 → Raw

**工程特性**

- 单次分类: 每帧在 reader 线程内恰好分类一次,结果随 `StreamItem` 广播给所有消费端 (TUI / Web / GUI),不重复计算
- 有界队列 fan-out: 广播层每消费者独立 1024 有界队列,慢消费者丢新帧计数 (`dropped`),**绝不阻塞 reader**
- 设备发现抽象: `DeviceDiscoverer` trait,`can-devices` 聚合 SocketCAN + USB-CAN 统一列表
- Web 写门控: `POST /api/send` 仅在 `--web-write` 启动时可用,默认只读;监听地址仅限本机回环
- 三平台 CI: Ubuntu / Windows / macOS 交叉检查,Linux 全量门禁 (test / clippy / fmt / core 纯度)
- 215 个 cargo 单元测试 + 7 个 Playwright e2e

## 快速开始

前置要求: Rust (stable, 建议 ≥ 1.88)。Linux 上构建 TUI/Web 形态无需额外系统依赖;GUI 形态需要 webkit2gtk / dbus (见下文)。

### 形态一: TUI (终端)

```bash
# 无硬件, 直接看界面
cargo run -- --backend none

# 接 SocketCAN 接口 (如 can0 或虚拟 vcan0)
cargo run -- --backend socketcan --iface can0

# 无硬件时可用 vcan0 联调 (需要 can-utils 灌帧)
bash scripts/vcan-setup.sh
cansend vcan0 181#01020304          # 另一个终端: CANopen TPDO1
cargo run -- --backend socketcan --iface vcan0
```

TUI 内按 `SPACE` 开始监控,`q` 退出。快捷键速查见下方「快捷键」。

### 形态二: Web (浏览器)

```bash
# 1. 构建前端 (只需一次, 产物 web/dist 由后端托管)
cd web && npm install && npm run build && cd ..

# 2. 启动后端 (REST + WebSocket + 静态页面)
cargo run -- --backend none --web-write
```

浏览器打开 <http://127.0.0.1:8080>。`--web-write` 才允许通过界面发送帧;去掉该标志后 Web 服务不会启动 (CLI 仅在写模式下拉起服务)。

前端开发模式 (热更新):

```bash
# 终端 1: 后端 (REST/WS 数据源, 监听 8080)
cargo run -- --backend none --web-write
# 终端 2: Vite 开发服务器 (页面在 1420, 数据仍走 8080)
cd web && npm run dev
# 浏览器打开 http://localhost:1420
```

### 形态三: GUI (桌面)

```bash
cd src-tauri && cargo tauri dev
```

Linux 需要系统依赖: `libwebkit2gtk-4.1-dev`、`libayatana-appindicator3-dev`、`librsvg2-dev` (及 `libdbus-1-dev`)。也可用仓库自带 devcontainer (已装好全部工具链)。GUI 前端与 Web 形态共用同一 React 代码 (`web/`),运行时经 Tauri IPC (invoke + Channel) 通信,接口与 Web 形态一一对应。

### 形态四: Docker 容器 (ghcr.io)

镜像发布在 GitHub Container Registry: `ghcr.io/shengjian20/can-monitor`,由 `.github/workflows/docker.yml` 在 tag `v*` / main 推送时自动构建。镜像内含 can-monitor 二进制 (TUI + Web)、Web 前端静态资源,以及 `libcontrolcan.so` (与二进制同目录 + `CAN_USBVCI_LIB` 双保险,USB-CAN **开箱即用**)。

```bash
# 拉取
docker pull ghcr.io/shengjian20/can-monitor:latest

# 无设备模式 (推荐先试): TUI 前台 + Web 界面
docker run --rm -it --network host ghcr.io/shengjian20/can-monitor:latest --backend none
# 浏览器打开 http://localhost:8080 使用 Web 界面 (TUI 在终端中)

# SocketCAN + 宿主机 vcan0 (宿主机先执行 bash scripts/vcan-setup.sh 建 vcan0)
docker run --rm -it --network host --cap-add NET_ADMIN \
  ghcr.io/shengjian20/can-monitor:latest --backend socketcan --iface vcan0

# 后台 Web 服务 (headless; TUI 需要 TTY, 故用 -dt 分配伪终端)
docker run --rm -dt --network host ghcr.io/shengjian20/can-monitor:latest --backend none
```

> **为什么用 `--network host` 而不是 `-p` 端口映射?** Web 服务受安全锁定仅绑定本机回环 (`127.0.0.1`,拒绝 `0.0.0.0`,Metis),且 Web 前端固定调用 `:8080` — 端口映射无法从容器外穿透到容器内的回环地址。`--network host` 让容器共享宿主机网络,Web 直接落在宿主机回环 `8080`,浏览器访问 `http://localhost:8080` 即开即用。默认 CMD 以 `--web-port 127.0.0.1:8088` 启动 (对应 `EXPOSE 8088`),仅在容器内访问时使用该端口。
>
> **需要 TTY**: 程序以 TUI 为主 (`can-monitor` 总会拉起终端界面,即便同时起了 Web),无 TTY 时 ratatui 初始化失败退出。交互运行用 `-it`,后台运行用 `-dt` 分配伪终端。
>
> **USB-CAN 设备透传**: `--backend usbvci` 需透传 USB 设备,例如 `--device /dev/bus/usb/001/002` 或 `--privileged` (容器内需对 `/dev/bus/usb/*/*` 有读写权限,见上节)。

本地构建镜像 (多阶段: node 构建前端 → rust 构建二进制 → debian:bookworm-slim 运行镜像): `docker build -t can-monitor:dev .`

## 设备支持

| 后端 | 平台 | 接口/设备 | 自动发现 | CANFD | 备注 |
|------|------|-----------|----------|-------|------|
| SocketCAN | Linux | `can0` / `vcan0` 等网络接口 | ✅ sysfs 扫描 (`/sys/class/net`) | ✅ (`--fd`) | 列表含 `can*` / `vcan*` / `slcan*` 前缀 |
| USB-CAN | Linux (x86_64 / aarch64) | USBCAN-I/II、USBCAN-E-U/2E-U 等 VCI 兼容设备 | ✅ `VCI_FindUsbDevice2` | ❌ 硬件不支持 | 固定 500kbps,动态加载 `ControlCAN.dll`/`libcontrolcan.so` |
| 无设备 | 任意 | — | — | — | `--backend none` 调试模式,不接总线 |

- **CANFD 说明**: USB-CAN 硬件不支持 CANFD,FD 帧返回 `Unsupported`;仅 SocketCAN 后端经 `--fd` 启用
- **供应商库**: `can-usbvci` 需要的 ControlCAN 库来自供应商随附的 `CAN分析仪资料20250624_Linux` 目录 (Linux 资料包 V1.45),已按 `third_party/controlcan/{aarch64,x86_64,win64}/` 对称布局随源码提交 (含 Windows `ControlCAN.dll`),并已打进 Tauri 发行包 → **开箱即用**, 用户拿到安装包/可执行文件即可连接 USBCAN-II, 无需自行寻找厂商库。来源与分发说明见 [docs/VENDOR.md](docs/VENDOR.md);重新拷贝可执行 `bash scripts/fetch-vendor.sh`
- **Windows usbcan64.dll**: 该 DLL 由厂商驱动安装器写入 System32 (驱动安装后自动就绪), SDK 与仓库均不单独携带
- **USB 权限**: 运行用户需对 `/dev/bus/usb/*/*` 有读写权限 (自行配置 udev 规则或以 root 运行);仓库未内置 udev 规则文件
- 扩展新设备 (第三方适配器 / 测试桩): 见 [docs/devices.md](docs/devices.md) 设备扩展指南

## 交叉编译 (aarch64 / RK3588)

目标平台: aarch64,glibc ≥ 2.23 (实测 Ubuntu 16.04 / RK3588)。

```bash
bash scripts/build-cross.sh                 # 构建整个 workspace
bash scripts/build-cross.sh -p can-monitor  # 只构建 can-monitor
```

产物: `target/aarch64-unknown-linux-gnu/release/can-monitor`。脚本自动完成 `rustup target add`、供应商 `.so` 的 SONAME 修复 (`patchelf --set-soname libcontrolcan.so`)、`cargo zigbuild` 构建与双重验证 (`file` 架构 + `readelf -V` glibc ≤ 2.23)。

部署: 二进制与 `libcontrolcan.so` 需一起拷贝,运行时设置 `LD_LIBRARY_PATH` (动态加载模式先查 exe 同目录,再查 `LD_LIBRARY_PATH`,最后交系统搜索)。详见 [docs/architecture.md](docs/architecture.md)。

## 命令行参数

```
can-monitor [选项]

--backend <socketcan|usbvci|none>  后端类型 (默认 none)
--iface <name>                     SocketCAN 接口名 (默认 can0)
--fd                               启用 CANFD (仅 SocketCAN 后端生效)
--log-file <path>                  日志文件路径 (candump -L 格式,追加写入)
--web-write                        启用 Web 写模式 (同时启动 HTTP 服务; 默认只读)
--web-port <host:port>             Web 服务监听地址 (默认 127.0.0.1:8080; 仅限本机回环)
--help, -h                         显示帮助
```

示例:

```bash
can-monitor --backend socketcan --iface can0 --log-file /tmp/can.log
can-monitor --backend usbvci --iface can0          # USB-CAN 时 --iface 仅作显示
can-monitor --backend none --web-write             # 无硬件 + Web 界面
can-monitor --backend none                         # 仅查看 TUI,不接后端
```

## 快捷键 (TUI)

| 按键 | 功能 |
|------|------|
| `q` | 退出 |
| `SPACE` / `s` | 切换监控开关 (默认关闭) |
| `f` | 切换过滤开关 |
| `l` | 切换日志记录 (需 `--log-file` 已配置) |
| `x` | 打开 CANopen 下发面板 |
| `↑` / `↓` | 滚动消息列表 |
| `PageUp` / `PageDown` | 翻页 (10 行) |
| `End` | 回到最新帧 (恢复尾部跟随) |

下发面板内: `Esc` 取消,`Tab` 切换字段,`Enter` 确认/发送,`1`-`4` 选择服务 (NMT / SDO读 / SDO写 / 原始帧),`0-9 a-f` 输入十六进制。

## 测试

```bash
cargo test                                   # 全部 crate 单元测试 (215)
cargo test -p can-usbvci --features mock     # USB-CAN mock 测试 (无需硬件/供应商库)
cargo test -p can-socketcan --features vcan-test  # vcan0 集成测试 (需本机 vcan0)
cd web && npm run test:e2e                   # Playwright e2e (7 用例, 需后端已起 --web-write)
```

质量门禁: `cargo clippy --workspace --all-targets -- -D warnings` 零告警、`cargo fmt --check`、`bash scripts/check-core-purity.sh` (core 无 UI 依赖、依赖白名单)。

## 目录结构

```
crates/
  can-types/            协议无关契约层 (CanBackend / CanFrame / CanId / CanDeviceInfo / DeviceDiscoverer)
  can-socketcan/        Linux SocketCAN 后端 (经典 + FD, sysfs 设备发现)
  can-usbvci/           ZLGCAN USB-CAN 后端 (VCI 动态加载 + 设备发现)
  can-devices/          设备发现聚合层 (SocketCAN + USBCAN 统一列表)
  canopen-stack/        CANopen (CiA 301) 解析与下发服务
  j1939-stack/          J1939 解析服务 (含 TP 多包重组)
  can-monitor-core/     核心流 (bus / 单次分类 / 过滤 / 日志 / 广播 fan-out / CLI 解析)
  can-monitor-server/   Web 服务 (axum REST + WebSocket 批量帧流 + 静态托管)
  can-monitor/          TUI 主程序
src-tauri/              Tauri v2 桌面 GUI (7 个 IPC 命令 + Channel 帧流)
web/                    React SPA (Vite + TS, Tauri/浏览器双模式自动切换)
scripts/                构建/部署/测试脚本
third_party/controlcan/ 供应商库 (aarch64/ + x86_64/ + win64/,随源码提交)
docs/                   架构 / 设备扩展 / Web API / 供应商说明
```

## 文档

- [docs/architecture.md](docs/architecture.md) — 架构: core 分层、fan-out 广播、单次分类、三形态接线
- [docs/devices.md](docs/devices.md) — 设备扩展指南: 实现 `DeviceDiscoverer` + `CanBackend` + `BackendConfig` 接入新设备
- [docs/web-api.md](docs/web-api.md) — Web API: REST 端点 / WebSocket 契约 / 帧 JSON schema / 错误码
- [docs/ssh-x11.md](docs/ssh-x11.md) — 无屏幕工控机远程使用指南: SSH + X11 转发 / 端口转发 + USB-CAN udev 规则
- [docs/VENDOR.md](docs/VENDOR.md) — 供应商 SDK 来源与分发说明

## 已知限制

- USB-CAN 后端仅经典 CAN,不支持 CANFD (硬件限制)
- 无硬件时 (`--backend none`) 帧流仅验证到 WS/REST 层,无真实总线数据;vcan0 可覆盖无硬件联调
- Tauri GUI 在 CI 中构建验证,未在本机图形环境实际运行 (7 个 IPC 命令与 Web 前端逐一对应,见 T27 对齐记录)
- `can-socketcan` crate 的 rustdoc 存在 7 条 pre-existing intra-doc 链接警告 (历史基线,非本次引入)

## License

MIT — 见 [LICENSE](LICENSE)。
