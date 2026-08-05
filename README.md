# can-monitor

基于 ratatui 的 CAN 总线监控 TUI。双后端 (SocketCAN / USB-CAN),双协议解析 (CANopen / J1939),面向嵌入式调试与产线诊断场景。

## 功能特性

- **三区 TUI 布局**:消息区 (报文流表格) + 状态栏 (3 行) + 帮助行 (1 行),支持滚动与高亮
- **监控开关默认关闭**:启动即静默,按 `SPACE`/`s` 开启后才开始消费总线帧;关闭时完全不触碰后端
- **双后端**:Linux SocketCAN (经典 CAN + CAN FD) / ZLGCAN USB-CAN (经典 CAN 仅),通过 `--backend` 切换
- **协议解析**:
  - 11 位标准帧 → CANopen (CiA 301):NMT / SDO / PDO / EMCY / SYNC / TIME / 心跳,含节点健康监控
  - 29 位扩展帧 → J1939:PGN 位域解析 + TP.BAM 多包重组 (最大 1785 字节,DM1/DM2 诊断分类)
- **过滤与高亮**:按 ID 范围 / 协议 / 方向组合过滤 (AND),按 ID 或协议命中高亮规则
- **candump 兼容日志**:`candump -L` 格式 `(秒.微秒) 接口名 ID#数据`,按 `l` 切换
- **CANopen 下发面板** (`x`):NMT 节点控制 / SDO 读 / SDO 写 / 原始帧,表单输入,发往总线
- **热插拔**:USB-CAN 设备拔出后自动重连 (≥2s 重枚举,最多 5 次)

## 快速开始

### 1. 容器开发环境

项目自带 devcontainer,已装好完整工具链 (rustc 1.97.1、zig、cargo-zigbuild、can-utils、交叉编译工具链、patchelf、libusb-dev)。

1. 用 VSCode 打开项目根目录
2. 提示 "Reopen in Container" 时确认 (或命令面板执行 `Dev Containers: Reopen in Container`)
3. 容器配置了 `NET_ADMIN` 能力并暴露 `/dev/bus/usb`,`postCreateCommand` 会自动创建 `vcan0` / `vcan1`

### 2. 编译 (本机)

```bash
cargo build --release
# 二进制: target/release/can-monitor
```

注意: `can-usbvci` crate 需要供应商库 `third_party/controlcan/`,缺失时构建会失败并提示先执行:

```bash
bash scripts/fetch-vendor.sh
```

### 3. 本地 vcan0 测试 (无硬件)

```bash
bash scripts/vcan-setup.sh          # 创建 vcan0 / vcan1 (幂等)
cargo run --release -- --backend socketcan --iface vcan0
```

再开一个终端用 can-utils 灌帧:

```bash
cansend vcan0 181#01020304          # CANopen TPDO1 节点 1
cansend vcan0 18FEF100#0102030405   # J1939 CCVS1 (PGN 0xFEF1)
cansend vcan0 140#00                # 未分配 COB-ID → Raw
```

TUI 内按 `SPACE` 开始监控。日志功能可加 `--log-file /tmp/can.log`。

### 4. 平台部署 (aarch64 / RK3588)

见下方「交叉编译」;部署时二进制与 `libcontrolcan.so` 需一起拷贝,并设置 `LD_LIBRARY_PATH`。

## 交叉编译

目标平台: aarch64 (RK3588 等),Ubuntu 16.04,glibc 2.23 (实测机 `jz@172.22.2.242`)。

```bash
bash scripts/build-cross.sh                 # 构建整个 workspace
bash scripts/build-cross.sh -p can-monitor  # 只构建 can-monitor (透传 cargo 参数)
```

产物: `target/aarch64-unknown-linux-gnu/release/can-monitor`

脚本 `scripts/build-cross.sh` 自动完成:

1. `rustup target add aarch64-unknown-linux-gnu` (幂等)
2. **SONAME 修复**:供应商 `libcontrolcan.so` 无 SONAME,直接用 zig/lld 链接会把解析后的绝对路径写进 `DT_NEEDED`,部署平台无法用 `LD_LIBRARY_PATH` 覆盖。脚本用 `patchelf --set-soname libcontrolcan.so` 补上 (幂等,无 patchelf 时仅警告)
3. `cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.23` 构建
4. 双重验证: `file` 输出必须含 `aarch64`;`readelf -V` 最大 GLIBC 版本必须 ≤ 2.23

要点:

- `.2.23` 后缀指定 glibc 版本,**不能省略** (zig 默认 glibc 2.28,与 Ubuntu 16.04 不兼容)
- `.cargo/config.toml` 保持最小配置 (留空),不要手动设 `linker = "zig"` (与 zigbuild 冲突)
- 部署: `scp` 二进制 + `libcontrolcan.so` 到平台,`LD_LIBRARY_PATH=/tmp /tmp/can-monitor --help` 验证 (平台无系统 libcontrolcan.so,必须随 .so 部署)

## 命令行参数

```
can-monitor [选项]

--backend <socketcan|usbvci|none>  后端类型 (默认 none)
--iface <name>                     SocketCAN 接口名 (默认 can0)
--fd                               启用 CANFD (仅 SocketCAN 后端生效)
--log-file <path>                  日志文件路径 (candump -L 格式,追加写入)
--help, -h                         显示帮助
```

示例:

```bash
can-monitor --backend socketcan --iface can0 --log-file /tmp/can.log
can-monitor --backend usbvci --iface can0          # USB-CAN 时 --iface 仅作显示
can-monitor --backend none                         # 仅查看 TUI,不接后端
```

## 快捷键

| 按键 | 功能 |
|------|------|
| `q` | 退出 |
| `SPACE` / `s` | 切换监控开关 (默认关闭) |
| `f` | 切换过滤开关 (过滤条件经代码配置: ID 范围 / 协议 / 方向) |
| `l` | 切换日志记录 (需 `--log-file` 已配置) |
| `x` | 打开 CANopen 下发面板 |
| `↑` / `↓` | 滚动消息列表 |
| `PageUp` / `PageDown` | 翻页 (10 行) |
| `End` | 回到最新帧 (恢复尾部跟随) |

下发面板内按键:

| 按键 | 功能 |
|------|------|
| `Esc` | 取消关闭面板 |
| `Tab` | 切换服务类型 / 切换字段 |
| `Enter` | 确认类型 / 发送帧 |
| `1`-`4` | 直接选择服务类型 (NMT / SDO读 / SDO写 / 原始帧) |
| `0-9 a-f` | 输入十六进制字符 |
| `Backspace` | 删除末尾字符 |

## USB-CAN 说明

USB-CAN 后端 (crate `can-usbvci`) 通过 ZLGCAN VCI 驱动访问设备,支持 USBCAN-I/II、USBCAN-E-U/2E-U 系列 (当前 CLI 以 `VCI_USBCAN2` + 通道 0 打开)。

- **仅经典 CAN**:USBCAN 硬件不支持 CANFD,FD 帧会返回 `Unsupported`。SocketCAN 后端才支持 FD (`--fd`)
- **波特率**: 固定 500kbps (定时器 0x00/0x1C),验收码 0、屏蔽码全 F (接收所有 ID),滤波关闭,正常模式
- **供应商库**: `scripts/fetch-vendor.sh` 从 SDK `Linux资料包V1.45/二次开发库文件` 拷贝到 `third_party/controlcan/` (aarch64 在根目录,x86_64 在 `x86_64/` 子目录)。`third_party/` 已被 .gitignore 忽略,不提交 git
- **链接模式** (环境变量 `CAN_USBVCI_LINK_MODE`):
  - `so` (默认): 链接 `libcontrolcan.so`。该 .so 内嵌 libusb-0.1 全部符号,运行时无需任何外部依赖,部署最省事
  - `static`: 链接 `libcontrolcan.a` + 系统旧版 `libusb` (0.1 API) + `pthread`,需要 `libusb-dev` (提供 `/usr/include/usb.h`,不是 libusb-1.0)
- **USB 权限**: 容器内由 devcontainer 的 `--device=/dev/bus/usb` 暴露总线;宿主机上需确保运行用户对 `/dev/bus/usb/*/*` 设备节点有读写权限 (自行配置 udev 规则或以 root 运行)。仓库未内置 udev 规则文件
- **无设备时测试**: `cargo test -p can-usbvci --features mock` 用内存模拟设备跑完整后端行为测试,不链接真实库、不需要硬件

## 测试

```bash
cargo test                     # 全部 crate 单元测试 (默认 feature)
cargo test --features mock     # can-usbvci mock 测试 (无需供应商库/硬件)
cargo test -p can-socketcan --features vcan-test   # vcan0 集成测试 (需本机 vcan0)
```

各 crate 测试规模: can-types 13,can-socketcan 10 (另 3 个 vcan 门控),can-usbvci 12 (mock 下 18),canopen-stack 19,j1939-stack 12 + 1 文档测试,can-monitor 107。

端到端验证 (记录于 `.omo/evidence/`):

- **Task 20 / vcan0 容器内 e2e**: 混合协议流显示、过滤、candump 日志、监控开关计数联动、NMT 下发、非法输入 (harness: `.omo/e2e/harness.py`)
- **Task 21 / RK3588 平台实测** (`jz@172.22.2.242`,can0 loopback): 启动 OFF → 监控 ON → RX 帧显示 → 下发面板 TX 被对端捕获 → 干净退出 → 日志 candump 格式

## 目录结构

```
crates/
  can-types/      协议无关契约层 (CanBackend / CanFrame / CanId / CanError)
  can-socketcan/  SocketCAN 后端 (经典 + FD)
  can-usbvci/     ZLGCAN USB-CAN 后端 (VCI FFI 绑定 + 双链接模式)
  canopen-stack/  CANopen (CiA 301) 解析与下发服务
  j1939-stack/    J1939 解析服务 (含 TP 多包重组)
  can-monitor/    TUI 主程序 (bus / classifier / filter / logger / tui)
scripts/
  fetch-vendor.sh 拷贝 USB-CAN 供应商库
  vcan-setup.sh   创建 vcan0 / vcan1
  build-cross.sh  aarch64 交叉编译 (SONAME 修复 + 产物验证)
.devcontainer/    开发容器 (工具链 + 交叉编译环境)
third_party/      controlcan 供应商库 (git 忽略)
docs/             架构文档
```

架构设计与扩展指南见 [docs/architecture.md](docs/architecture.md)。
