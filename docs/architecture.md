# 架构设计

本文档面向维护者,说明 can-monitor 的模块划分、数据流与扩展方式。所有内容与当前代码一一对应,新增功能前建议先通读对应模块。

## 1. Workspace 结构

Cargo workspace 共 6 个成员 (`resolver = "2"`),`[profile.release]` 启用 `lto` / `strip` / `codegen-units = 1` (小体积发布产物)。

| crate | 职责 | 关键公共 item | 依赖 |
|-------|------|--------------|------|
| `can-types` | 协议无关契约层:后端抽象、帧/ID/错误类型、统一消息 | `CanBackend` trait, `CanFrame`, `CanId`, `CanError`, `BackendConfig`/`BackendKind`, `CanMessage`, `FrameSource` | 纯 std,零依赖 |
| `can-socketcan` | Linux SocketCAN 后端 (经典 + FD) | `SocketCanBackend` | `socketcan =3.6.2` (关默认 feature) |
| `can-usbvci` | ZLGCAN USB-CAN 后端 (VCI FFI + 双链接模式) | `UsbVciBackend`, 常量 `VCI_USBCAN2` 等 | `can-types`; `mock` feature 切换 |
| `canopen-stack` | CANopen (CiA 301) 解析 + 帧构造 + 心跳监控 | `CanopenService` (`parse` / `nmt_frame` / `sdo_read_frame` / `sdo_write_frame` / `sync_frame` / `observe`), `NmtCommand`, `NmtState`, `CanopenMessage` | `canopen-host =0.6.1` (关默认 feature,私有封装) |
| `j1939-stack` | J1939 解析 + TP 多包重组 | `J1939Service` (`parse_id` / `parse` / `tick`), `J1939Header`, `J1939Message`, PGN 常量 | `sae-j1939-host =0.4.0` (私有封装) |
| `can-monitor` | TUI 主程序 (lib + bin) | `MonitorBus`, `FrameClassifier`, `FrameFilter`/`Highlighter`, `CandumpLogger`, `tui::app::App` | 其余 5 个 crate + `ratatui =0.30.2` + `crossterm 0.28` + `crossbeam-channel 0.5` |

依赖方向单向向下: `can-monitor` → 协议栈/后端 → `can-types`。后端 crate 之间互不依赖,协议栈 crate 之间互不依赖。MSRV: ratatui 0.30.2 要求 Rust ≥ 1.88 (容器内 1.97.1)。

## 2. 数据流

### 2.1 接收路径 (读方向)

```
┌──────────────┐   CanBackend::read_frame(100ms)   ┌──────────────────────────┐
│  后端        │ ─────────────────────────────────► │  reader 线程              │
│ SocketCan    │                                    │  MonitorBus::start_reader │
│ UsbVci       │                                    │                          │
│ (none 跳过)  │                                    │  FrameClassifier::classify│
└──────────────┘                                    │  ├─ 11 位标准帧 → CANopen │
                                                    │  └─ 29 位扩展帧 → J1939   │
                                                    └───────────┬──────────────┘
                                                                │ CanMessage
                                                                │ (方向恒为 Rx,带来源后端)
                                                                ▼
                                                  有界 channel (1024,满时阻塞 = 背压)
                                                                │
                                                                ▼
                                          ┌───────────────────────────────────┐
                                          │ TUI 事件循环 (App::run, 50ms 轮询) │
                                          │ drain_messages:                    │
                                          │  1. 日志 logger (过滤前,原始帧)     │
                                          │  2. 过滤 filter.matches (ID+方向)   │
                                          │  3. 分类结果缓存                    │
                                          │  4. 推入消息窗口 (VecDeque ≤ 1000)  │
                                          └───────────────────────────────────┘
                                                                │
                                                                ▼
                                                   MessageStream 渲染 Table
                                                   (倒序: 最新在上,按高亮规则着色)
```

要点:

- **监控开关默认关闭**: `MonitorBus` 的 `running` 标志初始为 `false`,reader 线程只在 `set_monitoring(true)` 后才调用 `read_frame`;关闭时以 20ms 间隔轮询标志,**不触碰后端** (不消费帧,计数冻结)
- **读取超时** (`CanError::Timeout`, 100ms 窗口) 视为正常,静默进入下一轮;其他后端错误累加 `error_count` 并写入错误 channel (容量 64),由状态栏第三行展示
- **有界 channel 即背压**: 投递 channel 容量 1024,TUI 消费慢时 reader 的 `send` 阻塞,不会无界堆积内存
- **J1939 有状态**: `J1939Service` 维护 TP 重组会话,`classify` 因此要求 `&mut self`,reader 线程内用 `Arc<Mutex<FrameClassifier>>` 串行化 (锁中毒时 `into_inner` 恢复)

### 2.2 下发路径 (写方向)

```
SendPanel (x 键打开)
  → App.handle_key 路由全部按键到面板
  → Enter 验证 → SendPanel::try_send(|f| bus.send_frame(f))
  → bus.send_frame: try_send 到发送 channel (容量 64,满则报错)
  → reader 线程每轮先 drain 发送 channel → backend.write_frame
  → 写失败: error_count++ 并写错误 channel
```

下发与监控开关解耦:监控关闭时 reader 线程也会先处理发送队列,因此"只下发不收帧"的场景可用 (例如对无响应节点发 NMT)。

### 2.3 错误路径

```
reader 线程 (读错误 / 发送失败)
  → err channel (String, 容量 64, 满则丢弃)
  → App::drain_errors → last_error → 状态栏第三行 (⚠ 红字)
```

## 3. 后端扩展指南 (新增 CanBackend 实现)

所有后端实现 `can-types` 的 `CanBackend` trait,上层 (bus / TUI) 通过 trait 对象或泛型接入,不感知具体后端。

```rust
pub trait CanBackend {
    fn open(config: &BackendConfig) -> Result<Self> where Self: Sized;
    fn read_frame(&mut self, timeout: Duration) -> Result<CanFrame>;
    fn write_frame(&mut self, frame: &CanFrame) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}
```

新增一个后端 (例如第三方 USB 适配器或测试桩) 的步骤:

1. **新建 crate** `crates/can-xxx`,依赖 `can-types` (唯一契约),加入 workspace `members`
2. **实现 trait**: `open` 解释 `BackendConfig` 对应变体 (或新增变体),`read_frame` 必须支持 `timeout` 语义 (超时返回 `CanError::Timeout`),`write_frame` 对不支持的能力返回 `CanError::Unsupported`
3. **错误映射**: 接口/设备不存在 → `CanError::NotFound`;设备拔出 → `CanError::DeviceUnplugged` (usbvci 由 `VCI_GetReceiveNum` 返回 `0xFFFFFFFF` 检测,并做 ≥2s 延迟重连,上限 5 次)
4. **帧转换**: 在 crate 内部把后端帧转成 `can-types` 的 `CanFrame` (参考 `can-socketcan` 的 `convert_*` / `to_*` 函数与 `can-usbvci` 的 `vci_obj_to_frame`),ID 越界返回 `CanError::InvalidId`,数据超长返回 `CanError::FrameTooLong`
5. **接入 main.rs**: 在 `run()` 的 `cli.backend.as_str()` 分支里按新后端名字 open + `bus.start_reader(backend, classifier, BackendKind::...)` (注意 `BackendKind` 需加对应变体),并设置状态栏显示名
6. **测试**: 纯转换逻辑写无硬件单元测试;后端行为用 trait 抽象注入测试桩 (`can-usbvci` 的 `VciOps` / `can-monitor` bus 的 `MockBackend` 是两个现成范式)

## 4. 协议栈替换指南

两个协议栈 crate 都遵循同一隔离原则:**公共 API 不泄漏底层库类型**,升级底层库时公共接口零改动。

| crate | 底层库 (pin 版本) | 隔离层 | 泄漏检查 |
|-------|------------------|--------|----------|
| `canopen-stack` | `canopen-host =0.6.1` (关 `tokio` feature) | `HeartbeatWatcher` 私有封装 `nmt::HeartbeatMonitor`;状态经 `NmtState::from_canopen` 转换 | 公共 API 只用 `NmtCommand` / `NmtState` / `CanopenMessage` + `can-types` 类型 |
| `j1939-stack` | `sae-j1939-host =0.4.0` | `Reassembler` 私有字段;公共 API 只暴露 `J1939Header` / `J1939Message` / `J1939Service` 与 u32 PGN 常量 | 无任何 sae 类型出现在公共签名 |

替换 `canopen-host` 的步骤:

1. 升级 Cargo.toml 中 pin 的版本 (pre-1.0 API 不稳定,必须锁精确版本)
2. 只改 `canopen-stack` 内部: `HeartbeatWatcher::new/record/state/is_alive/timed_out` 适配新 API
3. `NmtState::from_canopen` 若新版本改名/变值,同步调整映射
4. `cargo test -p canopen-stack` 全过即视为兼容 (公共接口未变,上层不受影响)

替换 `sae-j1939-host` 同理,注意 `Reassembler<1785, 8>` 的 const 泛型 (最大 1785 字节,最多 8 个并发会话) 与 `tick_with_timeout` 的调用方自持时钟约定。

## 5. 供应商库架构 (USB-CAN)

`can-usbvci` 的 `build.rs` 按 `TARGET` 架构自动选择库目录:

```
third_party/controlcan/aarch64/   → aarch64 (ARM平台/64bit 拷贝而来)
third_party/controlcan/x86_64/    → x86_64 (x86平台/64位linux系统)
third_party/controlcan/controlcan.h → 架构无关头文件
```

- TARGET 前缀匹配 (`x86_64*` → `x86_64/`, `aarch64*` → `aarch64/`),先 `split('.')` 剥掉 cargo-zigbuild 的 `.2.23` glibc 后缀再匹配;其他架构构建直接 panic 提示
- 目录对称布局: 两个架构同级子目录;`third_party/` 跟随源码提交发布 (不再 gitignore)
- 库目录用 `std::fs::canonicalize` 转绝对路径 (相对路径在运行时按 CWD 解析,不可靠)
- **双链接模式** (环境变量 `CAN_USBVCI_LINK_MODE`,缺省 `so`):
  - `so`: `rustc-link-lib=dylib=controlcan` + 注入 rpath。该 .so 内嵌 libusb-0.1 符号 (`readelf -d` 仅 NEEDED `libpthread` + `libc`),无外部依赖
  - `static`: `rustc-link-lib=static=controlcan` + `dylib=usb` (旧版 libusb 0.1 API) + `dylib=pthread`
- **rpath 注入位置**: build script 的 `rustc-link-arg` 只作用于发出 crate 自身的目标,不会传播到依赖方二进制 (cargo 仅对 Cdylib 放行,rust-lang/cargo#9562)。因此 rpath 由 **can-monitor 自己的 build.rs** 注入最终二进制;`can-usbvci` 的 rpath 只覆盖自身 crate 的测试目标
- `mock` feature 时 build.rs 提前 return,不链接真实库,`MockVciOps` 桩替换 FFI 调用,无硬件/无库主机也能构建测试

## 6. 交叉编译与部署

### 6.1 原理

- 用 cargo-zigbuild (zig 作为 C 后端) 编译 `aarch64-unknown-linux-gnu.2.23`;`.2.23` 后缀让 zig 生成最大 GLIBC 2.23 的产物,兼容 Ubuntu 16.04
- 供应商 `.a`/`.so` 是 glibc 链接的,必须用 glibc target (不是 musl)
- `.cargo/config.toml` 保持最小配置,不手动设 linker (与 zigbuild 冲突)

### 6.2 流程

```bash
# 1. 交叉编译 (脚本内做 SONAME 修复 + 双重验证)
bash scripts/build-cross.sh

# 2. 部署到平台 (需带 .so)
scp target/aarch64-unknown-linux-gnu/release/can-monitor jz@172.22.2.242:/tmp/
scp third_party/controlcan/aarch64/libcontrolcan.so jz@172.22.2.242:/tmp/
ssh jz@172.22.2.242 "chmod +x /tmp/can-monitor && LD_LIBRARY_PATH=/tmp /tmp/can-monitor --help"
```

### 6.3 SONAME 坑 (必读)

供应商 `libcontrolcan.so` **没有 SONAME**。zig/lld 链接时会把它解析后的绝对路径 (如 `/workspaces/can_monitor/third_party/controlcan/aarch64/libcontrolcan.so`) 写进 `DT_NEEDED`;glibc 对含 `/` 的 NEEDED 按字面路径打开,**LD_LIBRARY_PATH 不生效**,部署平台必然报 "cannot open shared object file"。

修复: `patchelf --set-soname libcontrolcan.so <so>` (幂等) 补 SONAME,重链后 `DT_NEEDED` 变成纯文件名,部署时 `LD_LIBRARY_PATH` 正常覆盖。该步骤已内置于 `scripts/build-cross.sh` (有 patchelf 则执行,无则警告)。

## 7. 测试策略

| 层级 | 位置 | 运行方式 | 依赖 |
|------|------|----------|------|
| 单元测试 | 各 crate `#[cfg(test)]` | `cargo test` | 无 (纯逻辑 / mock) |
| mock 后端测试 | `can-usbvci` mock feature | `cargo test -p can-usbvci --features mock` | 无硬件、无供应商库 |
| vcan 集成测试 | `can-socketcan` `vcan-test` feature | `cargo test -p can-socketcan --features vcan-test` | 本机存在 vcan0 |
| e2e (容器内) | `.omo/e2e/harness.py` | 见 Task 20 记录 | 单个 `docker run` 内 vcan-setup + TUI + cansend |
| 平台实测 | RK3588 can0/can1 | `pty-over-ssh` harness (`.omo/e2e/task21_platform.py`) | 平台可达,`ip` 命令配置接口 |

已解锁的关键质量门: 全 workspace `cargo test` + `clippy -D warnings` 零告警 (`can-usbvci` 需分别验证 mock / 非 mock 两种构建,FFI 类型比较错误只在非 mock 下暴露)。
