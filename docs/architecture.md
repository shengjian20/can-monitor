# 架构设计

本文档面向维护者,说明 can-monitor v2 的模块划分、数据流与扩展方式。内容与当前代码一一对应,新增功能前建议先通读对应模块。

v2 的核心变化: **core 提取** (总线/分类/过滤/日志/广播独立成 `can-monitor-core`)、**fan-out 广播层** (一读多播)、**单次分类** (`StreamItem` 进流)、**三形态** (TUI / Web / GUI) 共用同一核心流。

## 1. 分层总览

```
┌──────────────────────────────────────────────────────────────────────┐
│                          三形态前端 (UI)                               │
│                                                                      │
│  can-monitor (TUI)      can-monitor-server (REST/WS)    src-tauri    │
│  ratatui + crossterm    axum + tokio (异步出口)         Tauri v2 GUI  │
│        │                        │                          │          │
│        └────────────┬───────────┴──────────────┬───────────┘          │
│                     ▼                          ▼                      │
│  ┌────────────────────────────────────────────────────────────────┐   │
│  │              can-monitor-core (核心流, 同步, 无 UI)              │   │
│  │                                                                 │   │
│  │  MonitorBus (reader 线程 + 监控开关 + 计数器)                    │   │
│  │    │  读帧 → classify 恰好一次 → StreamItem                      │   │
│  │    ▼                                                            │   │
│  │  StreamBroadcaster (fan-out 广播, 每消费者有界队列)               │   │
│  │  FrameClassifier / FrameFilter / CandumpLogger / cli            │   │
│  └───────────────────────────────┬────────────────────────────────┘   │
│                                  ▲                                   │
│  ┌───────────────────────────────┴────────────────────────────────┐   │
│  │                    后端层 (实现 CanBackend)                      │   │
│  │  can-socketcan (Linux)     can-usbvci (ZLG USB-CAN, VCI 动态库)  │   │
│  │  └ 各实现 DeviceDiscoverer → can-devices 聚合                    │   │
│  └────────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  契约层: can-types (CanBackend / CanFrame / CanId / CanError /       │
│                   CanDeviceInfo / DeviceDiscoverer)                   │
│  协议栈: canopen-stack (CiA 301) / j1939-stack (SAE J1939)            │
└──────────────────────────────────────────────────────────────────────┘
```

依赖方向单向向下: 前端 → `can-monitor-core` → 后端/协议栈 → `can-types`。后端 crate 之间、协议栈 crate 之间互不依赖。

- **`can-types`**: 协议无关契约层,零依赖 (纯 std)。`CanBackend` trait、`CanFrame`/`CanId`/`CanError`、`BackendConfig`/`BackendKind`、`CanMessage`、设备抽象 (`CanDeviceInfo` / `DeviceDetails` / `DeviceKind` / `DeviceDiscoverer`)
- **后端层**: `can-socketcan` (Linux SocketCAN,经典 + FD,sysfs 设备发现)、`can-usbvci` (ZLG VCI,动态加载,`VCI_FindUsbDevice2` 枚举)
- **`can-devices`**: 纯聚合层,取 SocketCAN + USBCAN 设备列表并集 (SocketCAN 在前),不触碰硬件
- **`can-monitor-core`**: 核心流。**约束**: 同步、无 UI 依赖 (`scripts/check-core-purity.sh` 门禁)、依赖白名单 = `{can-types, canopen-stack, j1939-stack, crossbeam-channel}`
- **`can-monitor-server`**: 唯一的异步出口 (axum + tokio)。REST 端点 + WebSocket 批量帧流 + 静态托管 `web/dist`。core 保持同步,所有异步边界收敛在本 crate
- **`can-monitor`**: TUI 主程序 (lib + bin)。`--web-write` 时后台拉起 server crate 的服务,与 TUI 共享同一 `Arc<MonitorBus>`
- **`src-tauri`**: Tauri v2 GUI。7 个 IPC 命令 (`list_devices` / `start_monitor` / `stop_monitor` / `subscribe_frames` / `unsubscribe_frames` / `send_frame` / `get_status`) + `tauri::ipc::Channel` 帧流
- **`web/`**: React SPA。运行时检测环境 (`isTauri()`) 自动切换 TauriApi (invoke + Channel) 与 HttpApi (fetch + WebSocket)

## 2. 数据流 (接收路径)

### 2.1 单次分类 (StreamItem)

核心设计: 每帧在 reader 线程内**恰好被分类一次**,分类结果随帧一起广播,所有消费端直接取用,不再各自持有分类器。

```
后端 read_frame (100ms 超时)
  → reader 线程 (MonitorBus::start_reader)
      → FrameClassifier::classify(&frame)     ← 恰好一次
      → StreamItem { msg: CanMessage, parsed: ParsedMessage }
      → broadcast.publish(&item)              ← 广播给所有消费者
```

- `StreamItem` 定义在 `classifier.rs`,`derive(Debug, Clone, PartialEq, Eq)`
- 消费端 (TUI / Web 桥接 / Tauri Channel 推送) 直接用 `item.parsed` 映射协议与摘要,**不重新分类**
- 全 workspace 生产代码中 `.classify(` 仅出现在 reader 线程一处 (可用 grep 验证)
- CANopen / J1939 计数在 reader 内与发布共用同一次 classify 结果
- J1939 的 TP 多包重组有状态 (`J1939Service` 维护重组会话),`classify` 因此是 `&mut self`,reader 线程内用 `Arc<Mutex<FrameClassifier>>` 串行化 (锁中毒时 `into_inner` 恢复)

### 2.2 fan-out 广播 (有界队列,慢消费者丢帧)

`StreamBroadcaster<T>` (泛型,单生产者 → 多消费者) 是 v2 的转发核心:

- 内部 `Mutex<HashMap<ConsumerId, Sender<T>>>`,`ConsumerId = u64`,自增分配
- **每消费者独立 `crossbeam_channel::bounded(1024)` 队列** (默认容量,可调)
- `publish(&T)` 只做 `try_send`: 成功 → `consumed++`;队列满 → `dropped++` (**丢弃新帧,绝不阻塞**);接收端已断 → 惰性移除
- 消费者 `Receiver` drop 后,下一次 publish 惰性回收 (无需显式 unsubscribe;显式 `unsubscribe(id)` 也可)
- 计数器 `published` / `consumed` / `dropped` 全 Atomic,状态栏与 `GET /api/status` 的 `dropped` 字段即来自这里

```
                ┌──────────────────────────────┐
                │  StreamBroadcaster<StreamItem>│
reader publish ─▶  Mutex<HashMap<ConsumerId,   │
                │         Sender<StreamItem>>>  │
                └───────┬───────┬───────┬───────┘
                        │       │       │
                 bounded(1024)  │       │
                        │       │       │
                   TUI 消费者   │   WS 桥接线程    Tauri Channel 推送
                   (app.rs)     │   (server/ws)   (src-tauri commands)
                        │       │       │
                   满 → dropped++ (不阻塞 reader)
```

TUI 的消费端在 `MonitorBus::new()` 时内部订阅默认消费者,把接收端作为三元组返回 (`MonitorBus, Receiver<StreamItem>, Receiver<String>`),因此 **TUI 对广播层完全无感**。

### 2.3 发送路径 (写方向)

```
TUI 下发面板 (x 键) / POST /api/send / GUI send_frame
  → bus.send_frame(frame): try_send 到发送 channel (容量 64, 满则报错)
  → reader 线程每轮先 drain 发送队列 → backend.write_frame
```

发送与监控开关解耦: 监控关闭时 reader 线程仍处理发送队列,因此"只下发不收帧"可用。

### 2.4 错误路径

```
reader 线程 (读错误 / 发送失败)
  → err channel (String, 容量 64, 满则丢弃)
  → TUI 状态栏第三行 (⚠ 红字) / GET /api/status 的 error 计数
```

## 3. 三形态接线

### 3.1 TUI (`can-monitor`)

`main.rs` 的 `run()`: 解析 CLI (`can_monitor_core::cli`) → `MonitorBus::new()` → 按 `--backend` 打开后端并 `start_reader` → `App::new(Arc::clone(&bus), rx, err_rx, filter)` → `--web-write` 时后台线程拉起 Web 服务 (同一 `Arc<MonitorBus>`) → `app.run()` (50ms 轮询事件循环)。

### 3.2 Web (`can-monitor-server`)

- `router()` 构建: `GET /ws` + 5 个 REST 端点 + `DefaultBodyLimit` (64KB) + 静态回退 (仅当 `web/dist` 存在)
- **WS 桥接** (同步 → 异步): 每连接一个 `std::thread` 桥接线程,`recv_timeout(30ms)` 读 crossbeam 接收端 → 逐帧 `frame_to_json` → `tokio mpsc` (容量 256,满丢新帧) → WS 任务 `tokio::select!` 三方 (mpsc / 30ms 定时器 / socket.recv()),攒批 50 帧或 30ms 刷出,无帧发 `"[]"` 心跳
- **生命周期**: WS 断开 → `bus.unsubscribe` → Sender drop → 桥接线程退出 → mpsc 关闭 → WS 任务收 `None` 关连接,双向收敛,无泄漏
- **安全**: 写门控 `write_enabled` (false 时 `POST /api/send` 恒 403);监听地址仅回环 (`parse_bind_addr` 拒绝 0.0.0.0 / 局域网 / 公网)

### 3.3 GUI (`src-tauri`)

- `subscribe_frames`: 订阅广播流,每订阅一个 `channel_id`,推送线程 `recv_timeout(100ms)` 读 StreamItem → `stream_item_to_json` → `Channel.send`,stop 标志 + JoinHandle 统一清理
- 后端切换: `start_monitor` 按 `device_id` (socketcan/usbvci/none) 创建新 bus 并 `set_monitoring(true)`,先 shutdown 旧 bus
- 与 Web 形态共用同一套帧 JSON 契约 (见 docs/web-api.md),Channel 推送单帧,WS 推送批量数组

## 4. Workspace 结构

Cargo workspace 共 9 个成员 + `src-tauri` 独立目录 (`resolver = "2"`,`[profile.release]` 启用 `lto` / `strip` / `codegen-units = 1`)。src-tauri 不在根 workspace members 中,由 tauri CLI 独立管理 (其 `frontendDist` 指向 `../web/dist`,`devUrl` 为 `http://localhost:1420`)。

| crate | 职责 | 关键公共 item |
|-------|------|--------------|
| `can-types` | 契约层 | `CanBackend`, `CanFrame`, `CanId`, `CanError`, `BackendConfig`/`BackendKind`, `CanMessage`, `CanDeviceInfo`/`DeviceDiscoverer` |
| `can-socketcan` | SocketCAN 后端 (经典 + FD) | `SocketCanBackend`, `SocketCanDiscoverer`, `list_devices()` |
| `can-usbvci` | ZLG USB-CAN 后端 (VCI 动态加载) | `UsbVciBackend`, `UsbVciDiscoverer`, 常量 `VCI_USBCAN2` |
| `can-devices` | 设备发现聚合 | `DeviceManager::list_devices()` |
| `canopen-stack` | CANopen (CiA 301) 解析 + 下发 | `CanopenService`, `NmtCommand`, `NmtState`, `CanopenMessage` |
| `j1939-stack` | J1939 解析 + TP 重组 | `J1939Service`, `J1939Header`, `J1939Message`, PGN 常量 |
| `can-monitor-core` | 核心流 | `MonitorBus`, `StreamBroadcaster`, `StreamItem`/`FrameClassifier`, `FrameFilter`, `CandumpLogger`, `cli` |
| `can-monitor-server` | Web 服务 (异步出口) | `serve()`/`router()`, `FrameJson`, `frame_to_json`, `parse_bind_addr`, REST/WS 模块 |
| `can-monitor` | TUI 主程序 | `tui::app::App` |
| `src-tauri` | 桌面 GUI (独立 workspace) | 7 个 `#[tauri::command]` + `TauriState` |

## 5. 后端扩展

见 [docs/devices.md](docs/devices.md) — 完整的新设备接入指南 (实现 `CanBackend` + `DeviceDiscoverer`,扩展 `BackendConfig`,接入 `can-devices` 聚合)。

## 6. 协议栈替换

两个协议栈 crate 遵循同一隔离原则: **公共 API 不泄漏底层库类型**。

| crate | 底层库 (pin) | 隔离层 | 泄漏检查 |
|-------|-------------|--------|----------|
| `canopen-stack` | `canopen-host =0.6.1` (关 tokio feature) | `HeartbeatWatcher` 私有封装 `nmt::HeartbeatMonitor`;`NmtState::from_canopen` 转换 | 公共 API 只用 `NmtCommand` / `NmtState` / `CanopenMessage` + `can-types` 类型 |
| `j1939-stack` | `sae-j1939-host =0.4.0` | `Reassembler` 私有字段;`J1939Header` / `J1939Message` / PGN 常量 | 无任何 sae 类型出现在公共签名 |

替换步骤 (以 canopen-stack 为例): 升级 pin 版本 → 只改 crate 内部适配 (`HeartbeatWatcher` 方法) → 同步 `NmtState::from_canopen` 映射 → `cargo test -p canopen-stack` 全过即视为兼容。注意 `Reassembler<1785, 8>` 的 const 泛型 (最大 1785 字节,最多 8 个并发会话) 与调用方自持时钟约定。

## 7. 供应商库与交叉编译

### 7.1 USB-CAN 供应商库

```
third_party/controlcan/
  aarch64/    libcontrolcan.{a,so}   (ARM 平台/64bit)
  x86_64/     libcontrolcan.{a,so}   (x86 平台/64bit linux)
  controlcan.h                       (架构无关头文件)
```

- v2 的默认链接模式是**动态加载**: `can-usbvci` 用 libloading 在运行时 `dlopen("libcontrolcan.so")`,解析 13 个 `VCI_*` 符号 (`extern "system"` ABI)。二进制 `readelf -d` 无 VCI 符号引用,部署时只需 .so 可达
- 加载优先级: `CAN_USBVCI_LIB` 环境变量 → exe 同目录 (deploy 场景) → 裸文件名交 OS 搜索 (`DT_RUNPATH` 会被 dlopen 查询,因此仓库内构建无需 `LD_LIBRARY_PATH`)
- 静态模式 (环境变量 `CAN_USBVCI_LINK_MODE=static`): 链接 `libcontrolcan.a` + 旧版 `libusb` (0.1 API),需要 `/usr/include/usb.h`
- `mock` feature: 不链接真实库,`MockVciOps` 桩替换 FFI 调用,无硬件/无库主机可测
- 来源: 供应商随附 `CAN分析仪资料20250624_Linux` (Linux 资料包 V1.45),`scripts/fetch-vendor.sh` 拷贝入库。分发说明见 docs/VENDOR.md

### 7.2 交叉编译 (aarch64)

- `scripts/build-cross.sh`: `rustup target add` → `patchelf --set-soname libcontrolcan.so` (供应商 .so 无 SONAME,不补则部署平台 `LD_LIBRARY_PATH` 无效) → `cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.23` → 双重验证 (`file` 含 aarch64;`readelf -V` 最大 GLIBC ≤ 2.23)
- `.2.23` 后缀指定 glibc 版本,不能省略 (zig 默认 2.28,与 Ubuntu 16.04 不兼容)
- 供应商 `.a`/`.so` 是 glibc 链接的,必须用 glibc target (不是 musl)
- 部署: 二进制 + `libcontrolcan.so` 一起拷贝,`LD_LIBRARY_PATH` 指向 .so 目录

## 8. 测试策略

| 层级 | 位置 | 运行方式 | 依赖 |
|------|------|----------|------|
| 单元测试 | 各 crate `#[cfg(test)]` | `cargo test --workspace` (215) | 无 (纯逻辑 / mock) |
| mock 后端 | `can-usbvci` mock feature | `cargo test -p can-usbvci --features mock` (23) | 无硬件、无供应商库 |
| vcan 集成 | `can-socketcan` vcan-test feature | `cargo test -p can-socketcan --features vcan-test` | 本机 vcan0 |
| Web e2e | `web/e2e/` Playwright | `cd web && npm run test:e2e` (7) | 后端已起 `--web-write` |

关键质量门: 全 workspace `cargo test` + `clippy -D warnings` 零告警 + `cargo fmt --check` + `scripts/check-core-purity.sh` (core 0 UI 引用 + 依赖白名单)。CI (`.github/workflows/ci.yml`) 在三平台跑 `cargo check`,Linux 跑全量门禁。

已知基线: `can-socketcan` 的 rustdoc 有 7 条 pre-existing intra-doc 链接警告 (历史基线,新 crate 贡献 0)。
