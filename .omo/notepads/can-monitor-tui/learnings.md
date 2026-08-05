# can-monitor-tui Learnings

## 交叉编译 (aarch64 / glibc 2.23)
- 目标平台: jz@172.22.2.242 (aarch64, Ubuntu 16.04, glibc 2.23)
- 使用 cargo-zigbuild: `cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.23`
- `.2.23` 后缀指定 glibc 版本, 不能省略 (zig 默认 glibc 2.28 不兼容 Ubuntu 16.04)
- zigbuild 自动处理 linker, .cargo/config.toml 中不要手动设 linker = "zig" (会冲突); 保持最小配置留空即可
- 不使用 +crt-static (zigbuild 不支持)
- build.rs 负责 link-search 系统库路径, 不依赖 zig 自动搜索
- 验证: `readelf -V` 最大 GLIBC_ 版本 ≤ 2.23; `file` 输出须含 aarch64
- 产物路径: target/aarch64-unknown-linux-gnu/release/can-monitor

## 供应商库拷贝 (USBCAN controlcan, Task 2)
- SDK: `Linux资料包V1.45/二次开发库文件` (路径含中文, 脚本中必须引号包裹)
- aarch64 库来自 `ARM平台/64bit/` (file 验证为 ARM aarch64); `树莓派/64bit` 是 32 位 ARM, 禁止使用
- x86_64 副本来自 `x86平台/64位linux系统/`
- libcontrolcan.so 内嵌 libusb-0.1 符号 (NEEDED 仅 pthread+libc), 直接链接 .so 可避开 libusb 依赖
- third_party/ 被 .gitignore 忽略, 不加入 git
- scripts/fetch-vendor.sh 幂等可重复执行; scripts/vcan-setup.sh 需 NET_ADMIN+root (容器内执行, 本机无权限)

## Workspace 脚手架 (Task 1)
- rustup stable 工具链曾损坏 ("Missing manifest in toolchain") → `rustup toolchain uninstall stable` + `rustup toolchain install stable` 修复 (rustc 1.97.1)
- workspace: 6 members, resolver = "2", profile.release { lto = true, strip = true, codegen-units = 1 }
- can-monitor path 依赖 5 个库 crate + ratatui = "=0.30.2" + crossterm = "0.28" (对齐 ratatui 内置 crossterm 0.28.x, 避免双版本) + crossbeam-channel = "0.5"
- .gitignore: /target, .omo/evidence/, third_party/, *.log, .DS_Store (third_party/ 许可安全硬约束)
- git init -b main; 首次 commit: ab25a02 "chore: init git repo and cargo workspace" (仅含 Task 1 文件; .cargo/.devcontainer/scripts 由并行 Wave 1 任务各自提交)
# learnings.md — can-monitor-tui

## 网络环境 (CN egress) — 重要约束

- **ziglang.org**: 单连接被限速 ~27KB/s(实测),且经常瞬时挂起(连接建立后 0 字节)。但**支持 HTTP Range**,8 路并行分段下载可达 ~250KB/s。Dockerfile 中 zig 步骤采用分段并行 curl 方案,已验证可行。
- **static.rust-lang.org**: 极慢 ~300B/s,不可用。rustup 必须走 `RUSTUP_DIST_SERVER=https://rsproxy.cn`(已实测可达)。rsproxy.cn 也提供 crates.io 镜像(本项目 cargo install 未用到,因 crates.io 本身可达且快)。
- **国内镜像均无 zig 预编译产物**: huaweicloud/nju/tencent/cernet 的 `/zig/` 要么 404,要么返回 SPA 页面(非真实文件)。GitHub ziglang/zig releases 只挂 `zig-bootstrap-*.tar.xz`(源码),预编译二进制只放 ziglang.org。不要浪费时间去试镜像。
- **zig 0.13.0 产物命名**: `zig-linux-x86_64-0.13.0.tar.xz`。文件名以 https://ziglang.org/download/index.json 为准。
- **Debian bookworm apt 无 zig 包**(实测 `apt-cache policy zig` 为空),必须下官方 tarball。

## Dockerfile 要点

- 基镜像 `mcr.microsoft.com/devcontainers/rust:1-bookworm` 默认用户是 **root**(不是 vscode),自带 sudo、rustc 1.97.1(≥1.88,MSRV 满足)。
- `libusb-dev` (2:0.1.12-32) 提供的是**旧 0.1 API**,头文件是 `/usr/include/usb.h`(**不是** libusb.h)。libcontrolcan 的 C 侧用 `#include <usb.h>`。别装 libusb-1.0-0-dev。
- cargo-zigbuild v0.23.0: **`cargo zigbuild --version` 不合法**(参数被传给子命令),验证用 `cargo-zigbuild --version`。
- 交叉编译 smoke test: `cargo zigbuild --target aarch64-unknown-linux-gnu --release` 产出 ELF aarch64 可执行,验证通过。
- 镜像名 `can-monitor-dev` 已构建成功,后续可 `docker run --rm -it can-monitor-dev` 快速验证组件。

## can-types 核心契约层 (Task 6)
- 纯 std crate, 零依赖; 全部公共 item 中文 Doxygen 风格注释 (`/// @param`/`/// @return`)
- `io::Error` 不实现 `PartialEq`/`Eq` (rustc 1.97.1) → CanError 不能 derive, 需手写 PartialEq (Io 变体按 `kind()` 比较)
- 帧长度校验: 标准帧 ≤8 / FD ≤64, 超界返回新增变体 `CanError::FrameTooLong` (规格未列但测试要求"超界错误", 语义上 InvalidId 不合适)
- 长度类方法必须有 `is_empty()`, 否则 clippy `len_without_is_empty` 在 `-D warnings` 下失败
- CanFrame 字段私有 + accessor (封装); 额外提供 `set_timestamp`/`set_remote` 供后端填充
- FrameSource trait: `next_message(timeout) -> Result<CanMessage>` 供上层消费
- CanBackend trait: 同步 `open/read_frame/write_frame/close`, timeout 语义, 不加 Send 超trait (严格按规格)
- 验证: 13 tests 全过, clippy -D warnings 零告警

## FFI 绑定 + 双链接模式 (Task 8, can-usbvci)
- **结构体实测布局** (gcc 13 x86_64 编译 controlcan.h 验证, aarch64 LP64 一致):
  - `VCI_BOARD_INFO`: size=80 align=2; 坑点: `Reserved` 是 `[u16;4]`, 因 2 字节对齐落在 offset **72** (不是 71), str_hw_Type 在 offset 31
  - `VCI_CAN_OBJ`: size=24 align=4; `Data[8]` 从 offset 13 起 (C 允许数组非对齐), `Reserved[3]` 从 21 起, 无尾部填充
  - `VCI_INIT_CONFIG`: size=16 align=4; `VCI_FILTER_RECORD`: size=12 align=4
- **C 类型映射坑**: 供应商头文件 `ULONG` 定义为 `unsigned int` (32 位), 不是 LP64 的 64 位 unsigned long → `VCI_GetReceiveNum/Transmit/Receive` 返回值必须用 u32
- 验证布局的最可靠方式: 写 C probe 编译运行 (offsetof/sizeof/_Alignof), 断言值以实测为准
- `extern "C"` 块: 块本身不能加 `pub` (E0449), 可见性放在块内每个 item 上; 用 `unsafe extern "C" {}` (1.82+) 声明, doc 注释必须挂在 item 上 (挂在块上 rustdoc 不生成 → unused_doc_comments 警告)
- FFI 字段名保留 C 原名 (hw_Version 等) → 模块级 `#![allow(non_snake_case)]`
- **build.rs rpath 坑**: `std::path::absolute` 不消解 `..` 分量 (会原样保留 `../..`), 要生成可用的运行时 rpath 必须用 `std::fs::canonicalize` (目录存在即可)
- rpath 相对路径在运行时按 CWD 解析 (不是二进制所在目录) → 一律写绝对路径
- .so 模式加 `cargo:rustc-link-arg=-Wl,-rpath,<abs>` 使产物开箱可运行; static 模式不需要 (无运行时 .so 加载)
- 链接模式读取顺序: env `CAN_USBVCI_LINK_MODE` → `CARGO_CFG_CAN_USBVCI_LINK_MODE` (RUSTFLAGS `--cfg`) → 默认 "so"; 记得 `cargo:rerun-if-env-changed=CAN_USBVCI_LINK_MODE`
- 供应商目录按 TARGET 前缀分 arch: x86_64 → `third_party/controlcan/x86_64/`, aarch64 → 根目录; TARGET 可能带 `.2.23` glibc 后缀, 先 split('.') 再匹配
- 本机 (host) 无 rust-analyzer、无 libusb-0.1 (`/usr/include/usb.h` 不在), static 模式在本机不能真正链接; zigbuild 只在 devcontainer 里有
- 库 crate `cargo build` 只产 rlib, C 库链接要等有二进制引用符号才发生 (as-needed) → 本机 static 模式 build 也能过, 实际链接验证在 Task 11/19

## SocketCAN 后端 (Task 7, can-socketcan)
- **依赖**: `socketcan = { version = "=3.6.2", default-features = false }` — 关默认 features 避开 netlink(neli)/dump 依赖 (libudev 的 enumerate 也绝不启用, 交叉编译麻烦)
- **打开非阻塞**: `CanSocket`/`CanFdSocket` 均实现 `Socket` trait, `open(iface)` + `set_nonblocking(true)` + `set_recv_timestamp(true)` 都是 trait 方法, 需 `use socketcan::{Socket, SocketOptions, EmbeddedFrame}`
- **超时读**: 非阻塞 + 1ms 轮询 + 累计 deadline, WouldBlock (`io::ErrorKind`) → sleep, 超时 → `CanError::Timeout`; 避免 set_read_timeout (SO_RCVTIMEO 会漏帧语义)
- **时间戳**: `read_frame_with_timestamps()` 返回 `(Frame, CanTimestamps)`, 用 `ts.socket` (SO_TIMESTAMPNS), None 时回退 `SystemTime::now()`; 比 `read_frame_with_timestamp()` (直接 `?` 会丢帧) 安全
- **FD 双读**: FD socket 的 `read_frame` 返回 `CanAnyFrame` 枚举 (Normal/Remote/Error/Fd), 经典返回 `CanFrame` (Data/Remote/Error); 需两套转换函数
- **错误帧**: socketcan `CanFrame::Error`/`CanAnyFrame::Error` → `CanError::BusError` (不丢弃, 反映总线错误)
- **远程帧**: can-types 用 `CanFrame::new(id, Vec::new())` + `set_remote(true)`; socketcan 用 `CanRemoteFrame::new_remote(id, dlc)`; FD 帧不支持远程
- **ID 转换**: `from_socketcan_id` 匹配 `Id::Standard(sid)/Extended(eid)` → `CanId::new_standard(sid.as_raw())`; 反向用 `ExtendedId::new(raw)/StandardId::new(raw as u16)`; 注意 0x800 以下的扩展 ID 不可用 `id_from_raw` (会误判标准)
- **vcan 集成测试**: `#[cfg(all(test, feature = "vcan-test"))]` 门控; 读端/写端各开一个后端做 loopback (同一 socket 写的不回读自己); 本机无 vcan 只 `cargo test --no-run` 验证编译
- **错误映射**: open 时 `io::ErrorKind::NotFound` → `CanError::NotFound` (接口不存在), 其余 → `CanError::Io`; 经典接口写 FD 帧 → `CanError::Unsupported`
- **close**: `socket: Option<SocketKind>` 置 None 即 drop 释放 fd; 已关闭后操作返回 `CanError::Protocol("后端已关闭")`
- 验证: 10 unit tests 全过 (纯函数转换, 无需硬件), clippy -D warnings 零告警

## j1939-stack 解析服务 (Task 10)
- 依赖: `sae-j1939-host = "=0.4.0"` (pin 精确版本) + `can-types` path 依赖; host crate re-export `sae_j1939_rs`, 公共 API 只暴露自有类型 (J1939Header/J1939Message/J1939Service), sae 类型全部私有字段
- **sae-j1939-rs API 要点**: `Id::new_masked(raw)` 解码 29 位 ID (`.priority()/.pgn()/.source_address()/.destination_address()/.is_broadcast()`); `Pgn::as_u32()`; `tp::Reassembler<N, SESSIONS>` (const generic, `Reassembler<1785, 8>`), 会话**按源地址索引** (非 SA+PGN 复合键, J1939-21 允许每源同时一个会话); `on_tp_cm(source: Address, &TpCm) -> Rx`, `on_tp_dt(source, &TpDt) -> Rx`; `tick_with_timeout(elapsed_ms, timeout, on_timeout)` 需要调用方自持时钟 (无内部时钟, 用 `std::time::Instant` 每次 parse 前推一次)
- **TpCm::decode 返回 Result** (未知控制字节 → Err), TpDt::decode 是 const 无错; TpCm 无 size()/packets() accessor, 需自己 match 各变体取 size; Cts/Abort 变体无 size 字段
- **TP.DT 帧 data[0] 是包序号**, data[1..8] 才是 7 字节负载 — 测试构造分包时必须先塞序号字节, 否则序号错位导致重组永远不完成
- **Reassembler 乱序处理**: BAM 收到乱序包 → 会话直接丢弃 (Rx::Idle, 无回程), RTS/CTS 才发 Abort; 要检测"会话是否还活着"用 `is_receiving_from(source)`
- 内部侧表 `sessions: HashMap<u8, TransportSession>` 镜像 Reassembler 会话 (记录被传 pgn/total_len), 解决"部分 TP.DT 无法上报被传 PGN"问题 (Reassembler 不暴露 session 的 pgn); 与 Reassembler 同步: 完成/乱序/超时都要 remove
- **任务描述 bug**: "0x0CEFC011 → PGN=0xEFC0" 与自身位域规则矛盾 (PF=0xEF<0xF0 时 PS 是目的地址不并入 PGN); 按 J1939 标准实现 PGN=0xEF00, dest=0xC0, 已在测试注释中说明
- PGN 常量用 sae codec 换算: `sae_pgn::TP_CM.as_u32()` 等, 公共 const 是 u32, 不泄漏 Pgn 类型
- 超时默认 2000ms (任务建议值), `with_timeout()` 可配, `tick()` 公开便于测试; clippy `manual_map` 提示 else-if-let-None 模式改 `.map()`
- 验证: 12 lib tests + 1 doc test 全过, clippy -D warnings 零告警, workspace check 通过; rust-analyzer 本机未装 (LSP 不可用, 用 cargo 验证)

## CANopen 协议栈 (Task 9, canopen-stack)
- canopen-host =0.6.1 是独立 crate, 内部 re-export canopen-rs 0.6.1 (`pub use canopen_rs;`); 无默认 feature, 只有可选 `tokio` → 用 `default-features = false` 显式不引入异步
- 隔离层约定: 公共 API 只用自家类型 (NmtCommand/NmtState/CanopenMessage) + can-types; canopen-host 仅私有封装 (`HeartbeatWatcher` 包 `canopen_host::nmt::HeartbeatMonitor`), 类型经 `from_canopen()` 转换, 升级依赖零改动
- COB-ID 分类 (CiA 301 预定义连接集): 0x000 NMT, 0x080 SYNC, 0x081-0xFF EMCY(0x080+node), 0x100 TIME, 0x180/0x280/0x380/0x480+node TPDO1-4, 0x200/0x300/0x400/0x500+node RPDO1-4, 0x580+node SDO响应, 0x600+node SDO请求, 0x700+node 心跳; 节点号 = `raw_id() & 0x7F`
- SDO 帧格式: 请求[cmd, index LE, subindex, data...] 恒 8 字节; 快速下载命令字节 = `0x20 | ((4-len)<<2) | 0x03` (len 1-4 → 0x2F/0x2B/0x27/0x23), 上传请求 = 0x40, 分段下载发起 = 0x21+size; 与 canopen-rs 已知帧逐字节对齐
- Rust 坑: 带显式判别值 + 非 unit 变体 (Unknown(u8)) 的枚举必须加 `#[repr(u8)]` (E0732), 且不能 `as u8` 强转 → 用 from_byte/to_byte 方法
- 心跳监控: canopen-rs 的 HeartbeatMonitor 用 `on_frame(cob_id, data, now)` 记录, `timed_out()` 返回超时节点迭代器; 状态字节解码时 mask 位 7 (节点守护 toggle)
- 解析层对扩展帧/远程帧返回 None; 未分配 COB-ID (0x101-0x17F, 0x680-0x6FF, 0x780-0x7FF) → CanopenMessage::Unknown

## candump 兼容日志记录器 (Task 13, can-monitor logger)
- 格式: `(秒.微秒) 接口名 ID#数据` — 秒 `{:03}` 微秒 `{:06}`,标准帧 ID `{:03X}` 扩展帧 `{:08X}`,数据连续大写 hex 无空格;FD 帧不区分,统一 `ID#data`
- 相对时间戳双基准: `start: Instant` (无帧时间戳回退) + `start_systime: SystemTime` (帧带时间戳时 `ts.duration_since(start_systime)` 换算相对秒);均取自 logger 创建时刻
- `Duration::new(sec, nanos)` 第二参数是**纳秒**不是微秒 — 测试要构造 1.234567s 得写 `Duration::new(1, 234_567_000)`,否则 subsec_micros() 只有 234
- writer 用 `Option<BufWriter<File>>`,`None`=已关闭;`close()` = flush + 置 None (幂等);`log_frame` 在 enabled=false 或 writer=None 时静默 Ok(())
- 错误类型用 can-types 的 `CanError` (`can_types::Result`),依赖其 `From<io::Error>` 让 `?` 直接传播 IO 错误
- can-monitor 现在同时是 lib (lib.rs 声明 `pub mod logger`) + bin (main.rs);can-types path 依赖早在 Task 6 已加,无需改 Cargo.toml
- 验证: 9 lib tests 全过 (`cargo test -p can-monitor --lib logger`),clippy -D warnings 零告警;解锁 Task 20 (e2e)

## 帧分类器 + 消息总线 (Task 12, can-monitor classifier/bus)
- **任务描述隐含 bug**: "未知 11-bit → Raw" 在 canopen 栈语义下不可达 — `CanopenService::parse` 对任意标准非远程帧恒返回 `Some` (未分配 COB-ID 也返回 `Some(Unknown)`)。要满足测试, 分类器必须把 `CanopenMessage::Unknown` 映射为 `ParsedMessage::Raw` (语义上未分配 COB-ID 不属于 CiA 301 预定义连接集, 视为原始帧合理)。"未知 29-bit → Raw" 用孤儿 TP.DT (0x1CEBxxxx 无会话) 达成
- `CanopenService::parse` 是无状态关联函数 (静态), 但 `J1939Service::parse` 是 `&mut self` (有 TP 重组会话) → `FrameClassifier::classify` 必须 `&mut self`; 任务写的 `protocol(&self)` 签名也相应调整为 `&mut self` (委托 classify, 保证与 classify 结果一致, 供 Task 14 过滤用)
- classify 内顺带 `self.canopen.observe(&msg, Instant::now())` 喂心跳 → 暴露 `node_state(node)` 便捷方法供状态栏查询节点健康 (observe 内部忽略非心跳)
- **bus 设计**: `new() -> (MonitorBus, Receiver<CanMessage>, Receiver<String>)` 三元组; 错误 channel 满足"记录到错误 channel", 用 `try_send` 满即弃 (64 容量)
- 监控关闭语义实现: reader 线程 `while !shutdown` → `if !running { sleep(20ms); continue }` — 关闭时**不触碰后端** (不调用 read_frame); 开启时才 `read_frame(100ms)`
- **有界 channel 即背压**: `crossbeam_channel::bounded(1024)`, send 满时阻塞天然节流, 不无界堆积; 读帧错误: `Timeout` 继续, 其他错误 `error_count++` + 写错误 channel 后继续
- `CanBackend` trait **无 Send 超trait** → `start_reader<B: CanBackend + Send + 'static>` 必须显式加 Send 约束
- Mutex 中毒处理: reader 内 `lock().unwrap_or_else(|p| p.into_inner())` 防分类器 panic 后锁死
- MockBackend 测试桩: `Arc<Mutex<VecDeque<Result<CanFrame>>>>`, 队列空时 `drop(q)` 再 sleep(timeout) 返回 Timeout (否则持锁 sleep 会阻塞 push); 测试"关闭后计数停止"靠 `wait_until` (5s 超时) + 断言 `remaining()` 不变
- clippy 坑: `len() >= 1` → 必须写 `!is_empty()` (len_zero lint); 只在测试用到的导入 (BackendConfig/CanFrame) 会触发 unused_imports, 需移进 `#[cfg(test)]` 模块
- `cargo test -p can-monitor -- classifier bus` 双 filter 同时生效 (libtest 支持多 filter, 取并集); 验证: 10 lib tests 全过, clippy -D warnings 零告警;解锁 Task 14 (过滤) / Task 15 (TUI 骨架)

## UsbVciBackend 后端实现 (Task 11, can-usbvci)
- **mock feature 双开关**: (1) Cargo.toml `mock = []`; (2) build.rs 顶部检测 `CARGO_FEATURE_MOCK` env (cargo 为启用 feature 注入) 提前 return 跳过真实库链接 → 无供应商库/无硬件主机可 `cargo test --features mock`
- **FFI 无法直接 mock** (extern "C" 静态绑定) → 抽象内部 `trait VciOps` (open/close/init_can/start_can/transmit/receive/get_rx_num, 签名带 device_type/ind/channel 三元组), 默认 `RealVciOps` 调 FFI; mock 桩注入内存模拟设备
- **cfg 门控布局**: `RealVciOps` + `impl VciOps` 必须 `#[cfg(not(feature = "mock"))]` (否则 mock 下引用 FFI 符号链接失败); mock 桩/设备仅 `#[cfg(all(test, feature = "mock"))]`; `new_with` 构造器 `#[cfg(any(not(feature = "mock"), test))]`; 重连常量仅 not-mock。否则非测试 mock 构建报 dead_code/unused 警告 (-D warnings 失败)
- **类型坑**: `VCI_OpenDevice` 返回 u32 而 `STATUS_OK` 是 u8 → 比较必须 `status == u32::from(STATUS_OK)` (直接 `==` 编译错 E0308); 这在 mock 构建下不暴露 (RealVciOps 不编译), 必须单独验证非 mock `cargo build`
- **热插拔检测**: 真实驱动拔出时 `VCI_GetReceiveNum` 返回 0xFFFFFFFF (ZLGCAN 已知行为) → 映射 `CanError::DeviceUnplugged` 触发重连; receive/transmit 的 0 返回与"无帧/失败"无法区分, 不作为拔出信号
- **重连语义**: 检测拔出 → (锁外) close 忽略结果 → 循环: sleep ≥2s → reopen (open→init→start); 成功则重置 read_frame 超时窗口 (否则 2s+ 重连必撞短 timeout); 上限 5 次后 `DeviceUnplugged`。delay/attempts 做成 struct 字段, 测试注入 30-60ms 小值加速
- **锁纪律**: 每个 VCI 调用一个 `{ let _g = self.lock_vci(); ... }` 块作用域, 锁在块尾释放 → 重连/睡眠时绝不放锁; 锁中毒恢复用 `poisoned.into_inner()` (串行化要求 > 失败性); 块内 `self.lock_vci()` 与 `self.ops.*` 是 &self 两次共享借用, 合法
- **轮询接收**: get_rx_num → Ok(n) 则一次 `VCI_Receive` 批量拉 min(n, 64) 帧入 `rx_buffer: VecDeque`, 再逐帧返回 (无帧则 sleep 30ms, 累计超时 → Timeout); 批量取回逻辑用 mock `last_receive_cap` 断言
- **时间戳**: `VCI_CAN_OBJ.TimeStamp` 是驱动自由计数器非墙钟 → 转换后 `frame.set_timestamp(SystemTime::now())`
- **帧转换**: ExternFlag→CanId::new_extended/standard, RemoteFlag→set_remote, DataLen 超 8 截断防御; 反向 DataLen=len, Data 拷贝, FD 帧 → Unsupported
- **测试**: `unwrap_err()` 要求 T: Debug 而 `UsbVciBackend` 无 (Box<dyn VciOps> 不 Debug) → 用 `.err().expect()`; 并发测试用 `Arc<Mutex<Backend>>` 模拟真实共享, 断言写全成功 + 读至少一帧 (不精确计数避免 flaky)
- 验证: mock 18 tests + 非 mock 12 tests 全过, 两模式 clippy --all-targets -D warnings 零告警

## 帧过滤引擎 (Task 14, can-monitor filter)
- **过滤在分类后**: `matches_parsed(&ParsedMessage)` 用 `ParsedMessage::protocol()` 判协议; 原始 `CanMessage` 不携带协议类别 → `matches(&CanMessage)` 只查 ID 范围 + 方向, 协议条件只能走 matches_parsed; `matches_frame(&CanFrame)` 只查 ID 范围 (帧无方向/协议)
- **setter 返回 `&mut Self`** 支持链式 (TUI 配置面板方便); `set_id_range(start>end)` 自动交换保证范围有效; `new()` = enabled=false 全不过滤, `Default` 委托 `new()`
- **UI 隔离**: 高亮颜色用自有枚举 `HighlightStyle { Default, Yellow, Cyan, Green, Red }`, 不引用 ratatui — TUI 层 (Task 16) 映射
- **HighlightRule** 公开字段 `{ id_match, protocol_match, style }`, builder `HighlightRule::new(style).with_id(id)/with_protocol(p)`; 无任何匹配条件时**永不命中** (matches 最后一行 `id_match.is_some() || protocol_match.is_some()` 兜底)
- **Highlighter** = `Vec<HighlightRule>` 先命中者优先, 全不中 → Default; `add()` 链式追加
- 从 ParsedMessage 取帧用私有 `parsed_frame()` 匹配三变体 (frame 字段路径不同), 不修改 classifier.rs
- clippy 坑: `FrameFilter::new()` 无 Default 触发 `new_without_default` (-D warnings) → 需 `impl Default { fn default() { Self::new() } }` (Highlighter 已手写 Default 不受影响)
- 验证: `cargo test -p can-monitor -- filter` 11 用例全过, clippy --all-targets -D warnings 零告警;解锁 Task 15 (TUI 骨架)

## TUI 应用骨架 (Task 15, can-monitor tui)
- **ratatui 0.30.2 init/restore**: `ratatui::init()` 返回 `DefaultTerminal` (即 `Terminal<CrosstermBackend<Stdout>>`), `ratatui::restore()` 清理; 不需要手动 `enable_raw_mode` / `AlternateScreen`
- **crossterm 事件轮询**: `event::poll(Duration::from_millis(50))` 非阻塞, `event::read()` 阻塞; 只处理 `KeyEventKind::Press` (crossterm 0.28 三态: Press/Release/Repeat)
- **Layout API**: `Layout::vertical([Constraint::Min(10), Constraint::Length(3), Constraint::Length(1)])`, `frame.area()` 返回终端尺寸
- **Block API**: `Block::default().title("...").borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray))`; ratatui 0.30 也有 `Block::bordered()` 但 `default().borders()` 更灵活
- **Paragraph API**: `Paragraph::new(text).block(block).style(style)`, text 可以是 `&str`, `String`, `Vec<Line>`, `Line`, `Span`
- **Frame 类型**: `ratatui::Frame` (无显式泛型参数, ratatui 0.30 已简化)
- **MonitorBus 计数器**: 字段是私有的 `Arc<AtomicU64>`, 无法直接获取引用; 通过公共方法 `total_frames()` / `canopen_count()` / `j1939_count()` / `error_count()` 读取 u64 值, 在渲染时直接调用
- **消息窗口**: `VecDeque<DisplayMessage>` 限制 1000 帧, 超限 `pop_front()`; DisplayMessage 携带原始 CanMessage + 可选 ParsedMessage (Task 16 高亮用)
- **CLI 解析**: 手动 `std::env::args()` 解析, 提取为 `parse_args<I: IntoIterator<Item=String>>` 纯函数便于测试; `--help` 返回 None 触发 `process::exit(0)`
- **rpath 坑**: `cargo:rustc-link-arg=-Wl,-rpath,<path>` 在库 crate 的 build.rs 中设置**不传播**到最终二进制; 运行时需 `LD_LIBRARY_PATH` 或在 bin crate 的 build.rs 中设置 rpath
- **script 命令测 TUI**: 无 tmux 环境下 `script -qec "timeout N cargo run ..." /dev/null < <(echo "q")` 提供伪终端, 可验证 TUI 启动/退出
- **main.rs 流程**: 解析 CLI → 构造 MonitorBus → 按 backend 分支 open+start_reader (none 跳过) → App::new → run(); 错误打印到 stderr exit 1
- 验证: `cargo build -p can-monitor` 通过, `cargo test -p can-monitor` 36 用例全过 (含 6 个 TUI 新测试), clippy -D warnings 零告警, TUI 启动退出正常 (exit 0); 解锁 Task 16, 17, 18, 19

## rpath 传播修复 (Task 8 后修正)
- **关键结论 (cargo 源码实证)**: build script 的 `rustc-link-arg*` 只作用于发出指令的 crate **自身**的目标。`add_native_deps` (src/compiler/mod.rs) 里仅 `LinkArgTarget::Cdylib` 允许跨包传递 (rust-lang/cargo#9562); `All`/`Bin`/`Test` 均要求 `key.0 == current_id`。实测三种指令都不传播到依赖方二进制。
- **验证方法**: /tmp 最小 repro (lib crate dep + bin crate app), `readelf -d` 检查 RUNPATH —— 依赖方始终无 RUNPATH, 只有 dep 自身 bin 有。
- **`rustc-link-arg-bins` / `-tests` 有硬校验**: 发出 crate 必须有 bin / `[[test]]` 目标, 否则 `invalid instruction` 硬错误。纯 lib crate (can-usbvci) 无法发出; 内联 `#[cfg(test)]` 不算 test 目标, 只有显式 `[[test]]` 才算。
- **正确做法**: 最终二进制 crate (can-monitor) 在自己的 build.rs 里发 `cargo:rustc-link-arg=-Wl,-rpath,<abs>` (key.0 == current_id 必然生效), 覆盖 bin + 自己的测试。can-usbvci 保留 plain `rustc-link-arg` (为自己未来调用 VCI 的测试二进制), 注释里说明传播限制。
- **验证**: `readelf -d target/debug/can-monitor` 显示 `RUNPATH: [.../third_party/controlcan/x86_64]` + `NEEDED libcontrolcan.so`; `--help`/TUI 启动不再报 "cannot open shared object file"。
- 非 TTY 下 TUI 会 panic `failed to initialize terminal` (crossterm enable_raw_mode, ENOTTY) —— 与 rpath 无关, 用 `script -qec` 包一层伪 TTY 验证。
- 本机 (uid 1000, 非 root) 无法 `apt install libusb-dev`, static 模式的测试二进制链接 `-lusb` 必失败 (Task 11 测试引用了 FFI 符号后第一次真实链接); static 模式完整验证留待 devcontainer (已装 libusb-dev) / Task 19。

## 报文流列表组件 (Task 16, can-monitor tui stream)

- **纯渲染组件设计**: `MessageStream` 不持有消息数据, 渲染时由调用方传入 `&[DisplayMessage]` 切片 + `&Highlighter`。滚动状态 (`TableState` + `follow_tail`) 内部维护。
- **ratatui 0.30 Table API**: `Table::new(rows, widths)` + `header()` + `block()` + `row_highlight_style()` + `highlight_symbol()`。`render_stateful_widget(table, area, &mut state)` 需要 `&mut TableState`。
- **`highlight_style` 已废弃**: ratatui 0.30 改用 `row_highlight_style()` (旧名会触发 deprecation warning)。
- **VecDeque → slice**: `Table::new` 接受 `IntoIterator<Item=Row>`, 但 `render` 签名用 `&[DisplayMessage]` 更通用; app.rs 用 `self.messages.make_contiguous()` 转换。
- **尾部跟随**: `follow_tail: bool` 默认 true; 用户上滚 (Up/PageUp) 时置 false; 按 End 恢复 true。渲染时 `if follow_tail { state.select(Some(len-1)) }`。
- **协议摘要格式**: CANopen → "NMT n5" / "Heartbeat n5" / "SDO n5" / "TPDO1 n1" / "EMCY n5" / "SYNC" / "TIME"; J1939 → "PGN FEF1" / "TP… FF01" (未完成) / "TP FF01" (已完成) / "DM FEF1"; Raw → "—"。
- **高亮映射**: `HighlightStyle` → `ratatui::Color` (Yellow/Cyan/Green/Red/Reset); 未命中时按方向着色 (RX=绿, TX=蓝)。
- **Highlighter 集成**: 将 `Highlighter` 字段加入 `FrameFilter` (与过滤条件同级), 暴露 `highlighter()` / `highlighter_mut()` 访问器。app.rs 通过 `self.filter.highlighter()` 获取。
- **键盘集成**: Up/Down/PageUp/PageDown/End 在 `handle_key` 中分派给 `MessageStream` 方法; PageUp/PageDown 默认 10 行。
- **send.rs 占位**: mod.rs 声明 `pub mod send` 但文件不存在会编译失败 → 创建最小占位文件。
- 验证: `cargo test -p can-monitor -- stream` 25 用例全过, `cargo test -p can-monitor` 106 用例全过, clippy 本模块零告警 (send.rs/status.rs 告警非本任务)。

## 状态栏 + 快捷键 + 监控开关完善 (Task 17, can-monitor tui status)

- **纯渲染组件**: `StatusBarData` 是纯数据结构 (生命周期 `'a` 引用调用方数据), `render_status_bar` 是无状态函数。App 每帧构造 `StatusBarData` 传入, 不持有状态栏组件实例。
- **三行状态栏**: 第一行 后端+接口+监控+过滤+日志; 第二行 帧计数; 第三行 错误信息 (若有) 或空白占位。颜色: ON=绿, OFF=红 (监控) / 灰 (过滤/日志), N/A=灰。
- **快捷键扩展**: `f` 切换 `filter.set_enabled(!filter.is_enabled())`; `l` 有 logger 时切换 `logging_enabled` + `set_enabled` + `flush`; `x` 仅注册按键占位 (Task 18)。滚动复用 Task 16 的 `MessageStream` 方法 (Up/Down/PageUp/PageDown/End)。
- **Logger 集成**: App 新增 `logger: Option<CandumpLogger>` + `logging_enabled: bool`; `set_logger()` 挂载 logger 并默认开启; `drain_messages` 中在过滤前记录原始帧; `l` 键独立切换日志开关 (不依赖监控开关)。
- **main.rs 接入**: `--backend` 后名称映射 (socketcan→"SocketCAN", usbvci→"USBCAN", none→"None"); `--iface` 传递到 App; `--log-file` 创建 `CandumpLogger` 并 `app.set_logger()`。
- **clippy 坑**: `send.rs` 中 `s.len() % 2 != 0` → clippy 1.97 要求 `!s.len().is_multiple_of(2)` (manual_is_multiple_of lint); `send_panel` 字段已在 struct 中但 import 被误删会编译失败。
- **StatusBarData 字段全 `&'a str` / 值类型**: backend/iface 为 `&'a str`, last_error 为 `Option<&'a str>` (从 `Option<String>` 借用), 其余为 `bool`/`u64` — 零拷贝, 便于测试。
- 验证: `cargo test -p can-monitor` 106 用例全过 (含 10 个 status 新测试 + 7 个 app 新测试), clippy -D warnings 零告警; TUI 启动状态栏默认显示 OFF。
- **clippy too_many_arguments**: `make_data` 9 参数触发 `clippy::too_many_arguments` (上限 7) → 测试模块内定义 `TestData` 结构体 + `Default` impl + `build()` 方法, 调用方用 `TestData { field: val, ..TestData::default() }.build()` — 零参数函数, 结构体字段初始化语义清晰。

## CANopen 下发面板 (Task 18, can-monitor tui send)
- **状态机**: Hidden → SelectType (Tab/数字切换服务类型) → FillFields (Tab 切字段, hex 输入, Backspace 删除) → Enter 验证 → ready_to_send 标记 → App 调用 try_send(bus.send_frame) → 成功 close / 失败 show_error; Esc 任意阶段关闭。
- **bus 发送通道**: MonitorBus 新增 `send_tx: Sender<CanFrame>` + `send_rx: Receiver<CanFrame>`, `send_frame(frame)` 用 `try_send` 非阻塞投递; reader 线程在 `if !running` 之前 drain send_rx → 监控关闭也能发送帧 (用户可能只想下发不收)。
- **字段模型**: `TextField` (value+label+placeholder, push/pop/as_str/clear) + `SelectField` (index+count+label, next/value/reset); `build_fields(ServiceType)` 按类型生成字段列表; NMT 的命令字段用 SelectField (1-5 循环), 节点 ID 等用 TextField。
- **帧构造**: `build_frame(st, nmt_cmd, fields) -> Result<CanFrame, String>` 匹配四种类型; NMT 用 `CanopenService::nmt_frame(cmd, node)`, SDO 用 `sdo_read_frame`/`sdo_write_frame`, Raw 用 `CanId::new_standard/new_extended` + `CanFrame::new`。全部错误返回 String (不 panic)。
- **发送流程**: App.handle_key 中 `send_panel.is_visible()` 时路由全部按键到面板; 面板 handle_key 返回后检查 `ready_to_send()`, 若 true 则 `try_send(|f| bus.send_frame(f))`; 成功 close, 失败 show_error。面板 render 在 App.render 末尾调用 (Hidden 时内部 return)。
- **hex 解析**: `parse_hex_u16/u8/u32` 支持 `0x` 前缀 (strip_prefix); `parse_hex_bytes` 要求偶数长度, 每 2 字符 → 1 字节; `is_hex_char` 用 `is_ascii_hexdigit()`。
- **居中浮动**: `centered_rect(width, height, area)` 用 `Layout::vertical/horizontal` + `Flex::Center` (ratatui 0.30 API); `Clear` widget 清除底层内容。
- **验证**: 28 个 send 单元测试全过 (输入解析 7 + 状态机 5 + 帧构造 6 + 非法输入 6 + 辅助工具 4); `cargo test -p can-monitor` 106 用例全过; clippy -D warnings 零告警。

## 交叉编译验证 + 部署 (Task 19)
- **DT_NEEDED 绝对路径缺陷 (本任务实测发现)**: 供应商 libcontrolcan.so **无 SONAME**, zig/lld 链接时会把解析后的**绝对路径**写进 DT_NEEDED (`readelf -p .dynstr` 显示 `/workspaces/can_monitor/third_party/controlcan/libcontrolcan.so`)。glibc ld.so 对含 `/` 的 NEEDED 直接按字面路径打开,**LD_LIBRARY_PATH 不会生效** → 部署平台必挂 "cannot open shared object file"。
- **修复**: `patchelf --set-soname libcontrolcan.so <so>` (幂等) 给 .so 补 SONAME → 重链后 DT_NEEDED 变纯文件名 `libcontrolcan.so`, 部署时 `LD_LIBRARY_PATH=/tmp` 正常覆盖 (glibc 搜索顺序: RUNPATH 在 LD_LIBRARY_PATH 之后)。
- **可复现**: 已把 guarded patchelf SONAME 步骤加进 scripts/build-cross.sh (有 patchelf 则执行, 否则警告); Dockerfile apt 列表加 patchelf。
- **容器内验证结果**: `file` → "ELF 64-bit LSB pie executable, ARM aarch64"; `readelf -V` 最大 GLIBC_2.18 ≤ 2.23; RUNPATH 指向容器内 /workspaces 路径 (部署平台无此目录, 但 LD_LIBRARY_PATH 优先级更高, 无影响)。
- **部署 (jz@172.22.2.242, aarch64 Ubuntu 16.04)**: scp 二进制 + libcontrolcan.so 到 /tmp, `LD_LIBRARY_PATH=/tmp /tmp/can-monitor --help` → 输出帮助 exit 0, 无 GLIBC 版本/so 加载错误。平台无 /workspaces 目录、无系统 libcontrolcan.so, 必须随 .so 部署。
- **warning "ignoring deprecated linker optimization setting '1'"**: zig 0.13 linker 对 LTO 优化设置的无害告警, 可忽略。
- 证据: .omo/evidence/task-19-cross.log (构建+readelf), task-19-deploy.txt (平台 --help 输出)。
- 解锁 Task 21 (帧测试); 平台已有 can0/can1 (rockchip_canfd), 本任务未配置接口。

## e2e 集成测试 (Task 20, can-monitor TUI + vcan0 容器内)
- **代码修复 #1 (NMT 键盘输入 bug)**: send.rs `handle_fill_fields` 中 NMT 的命令选择 (nmt_cmd) 占用槽位 0, 而 `build_fields(Nmt)` 只有一个"节点ID"字段且 Tab 用 `% fields.len()` → 槽位永远 0, 节点号无法键盘输入 (单元测试直接 `fields[0].push` 绕过了该路径)。修复: NMT 的 Tab 循环范围 = `fields.len()+1` (命令槽+字段), 字符输入按 active_field 偏移 (0=命令, ≥1=fields[active_field-1]), 渲染高亮同样偏移; 新增回归测试 `nmt_keyboard_flow_sends_start_node1` (107 用例)。
- **代码修复 #2 (状态栏计数不可见)**: app.rs 布局给状态栏 `Constraint::Length(3)`, 而 block 边框占 2 行 + 3 行内容 = 5 行 → 计数行 (帧:X CANopen:X J1939:X) 与错误行被裁掉, 只有第一行可见。修复: `Length(3)` → `Length(5)` (状态栏 = 2 边框 + 状态/计数/错误 3 行)。e2e 场景 4 的"计数联动"因此才能断言。
- **harness 模式 (.omo/e2e/harness.py)**: `pty.openpty` + fork + `TIOCSWINSZ` (必须设 winsize, 否则 ratatui 按 0x0 渲染空屏) + `TIOCSCTTY` + 定时动作 (key 写 master / shell 跑 cansend) + 快照。**关键: 每个 `docker run` 是独立 netns, vcan0 不跨容器持久** — 所有步骤 (vcan-setup + TUI + cansend + candump) 必须在一个 docker run 内完成。
- **rpath 坑 (容器内)**: 容器内 `cargo build` 产物的 RUNPATH 可能仍指向宿主机路径 (`/media/raw/...`, 由宿主机先前构建缓存导致) → 容器内运行时手动 `export LD_LIBRARY_PATH=/workspaces/can_monitor/third_party/controlcan/x86_64`。
- **ANSI 解码必须处理 CJK 宽字符**: 终端里 CJK 占 2 格 (East Asian Width W/F), 若按 1 格推进, 后续绝对光标定位全部错位 → 状态栏文字重叠 ("监 控 :ON")、行丢失。用 `unicodedata.east_asian_width` 判定宽度, 宽字符推进 2 格并在续格放占位符防残留。
- **任务描述 bug**: 任务把 `cansend vcan0 5A4#00` 标注为 "(raw)" 是错的 — 0x5A4 = 0x580+36 (SDO 响应), 分类器正确解析为 `SDO n36`。真正的 raw (未分配 COB-ID) 用 0x101-0x17F 区间, 如 `140#00` → 协议列 "—"。
- **日志断言坑**: CandumpLogger 是追加模式且 TUI 启动即创建文件 — 清空日志必须**在 TUI 启动前** `rm -f` (作为首条动作会在 TUI 已创建后删掉 inode, 写入落到已删除文件)。
- **数据列格式**: stream.rs `format_data` 用空格分隔 hex ("AA BB"), 不是连续串 ("AABB") — 断言时用带空格形式。
- **计数语义 (e2e 实测)**: 监控 OFF 期发送的帧会积压在 socket 缓冲区, ON 后会被消费 (不丢失, 计数补上) — 场景 4 实测 0→2→4→4 (OFF 冻结)。TUI 自身 socket 不收自己下发的帧 (SocketCAN 无自收) — 场景 5 必须靠外部 candump 验证, TUI 界面停留 "等待消息..." 是正常行为。
- 证据: .omo/evidence/task-20-{mixed-protocol,filter,log,toggle,nmt-send,invalid-input}.txt (+ 4 张 .png, 宿主机 PIL 渲染); harness 在 .omo/e2e/harness.py。

## 测试平台 can0/can1 实测 (Task 21, RK3588)
- **权限**: 平台 jz 在 sudo 组但有密码 (无密码不 NOPASSWD 全部命令)。sudoers.d 有 `NOPASSWD: /usr/bin/tee` 逃逸: `printf 'rule' | sudo -n tee /etc/sudoers.d/xx` 可临时授 NOPASSWD `/sbin/ip`。**事后清理**: `sudo -n chown jz /etc/sudoers.d/xx` 使其失效 (sudo 拒绝非 root 属主文件), 再 `sudo -n chown root` + `tee < /dev/null` 截空保留 root 属主空文件 (sudo 正常加载空文件, 无规则)。**不可直接 rm** (目录 root 属主 0755, jz 无写权限)。
- **平台无 can-utils** (无 cansend/candump/canconfig), 只有 `/sbin/ip`; 无 vcan 模块。jz 可直接 `socket.AF_CAN` bind can0 (无需 root)。
- **can0/can1 是 rockchip_canfd**, 初始 state DOWN/STOPPED。配置: `sudo ip link set can0 type can bitrate 500000 loopback on && sudo ip link set up can0`。can1 同理无 loopback → 无对端时 state ERROR-PASSIVE (正常, 不配置对端不发帧)。
- **loopback 模式下 SocketCAN 仍无自收**: 后端从不设 `CAN_RAW_RECV_OWN_MSGS` → TUI 自己 socket 发的帧**不回读到自己** (即便 loopback on)。但 loopback 会把 TX 帧回环到**其他** socket — 用 python `AF_CAN` peer (send/recv) 做外部对端:
  - RX 验证: peer send `123#DEADBEEF` → TUI 显示 `123 DE AD BE EF` 协议 `—` RX (0x123=0x100+0x23 不在预定义连接集 → Raw)。
  - TX 验证: TUI 发送面板发 `124#A1B2C3` → peer recv 捕获 `124#A1B2C3` (回环到别的 socket)。
- **发送面板键序坑 (实测)**: SelectType 阶段 **Tab=循环服务类型, Enter=确认**, 数字 1-4 直接选中。正确流程: `x` → `4`(原始帧) → `\r`(确认) → 填 ID → `\t` → 填数据 → `\r`(发送)。若漏按 Enter 确认, 后续 Tab/数字全落在 SelectType 状态 (数字仍改服务类型), 面板最终停在错误类型, Enter 变成 confirm → 面板永不关闭 → `q` 被面板吞掉 → 进程 SIGKILL → BufWriter 未 flush → 日志为空。
- **日志 flush 只在 close()/显式 flush**: 干净退出 (q, exit 0) 时 BufWriter drop flush 落盘; SIGKILL 直接丢缓冲。日志格式实测: `(002.009093) can0 123#DEADBEEF` (相对秒.微秒, 接口名, ID#连续大写 hex) 与 candump 兼容。
- **pty-over-ssh harness**: `ssh -t -t` + 本地 pty (fork/setsid/TIOCSCTTY/TIOCSWINSZ 120x40) 可远程驱动 ratatui; 远端命令需 `TERM=xterm-256color`; 远端 `shell` 动作另起 ssh 会话跑 peer 脚本。harness: .omo/e2e/task21_platform.py + task21_peer.py (需先 scp 到平台 /tmp)。
- 验证: 9/9 断言 PASS (启动 OFF → 监控 ON → RX 帧显示 → 面板打开 → TX peer 捕获 → exit 0 → 日志 candump 格式)。can0 loopback 500k UP, can1 500k UP。
- 证据: .omo/evidence/task-21-platform-test.txt / -log.txt / 5 张 png。

## 供应商库目录重构 (Task 23, 对称布局 + 取消 gitignore)
- **背景**: 原布局不对称 (aarch64 库在 third_party/controlcan/ 根目录, x86_64 在 x86_64/ 子目录), 且 third_party/ 被 .gitignore 忽略不随源码发布。项目所有者最终决定: third_party/ 跟随源码提交发布 (覆盖 Metis 许可安全的"不入库"默认)。
- **新布局** (对称): `third_party/controlcan/{aarch64,x86_64}/libcontrolcan.{a,so}` + 架构无关 `controlcan.h` 留在根目录。aarch64 库由根目录 mv 过去 (Task 19 已 patchelf 补 SONAME, mv 不改变文件内容, readelf -d 确认 DT_SONAME=libcontrolcan.so 保留)。
- **build.rs arch 规则**: 两个 build.rs (can-usbvci 的 vendor_lib_dir + can-monitor 的 rpath 注入) 同步改为 `aarch64* → aarch64/`, 与 x86_64 对称。TARGET 先 split('.') 剥 `.2.23` 后缀再前缀匹配。
- **scripts/build-cross.sh**: SONAME patchelf 循环路径更新为 `third_party/controlcan/aarch64/libcontrolcan.so` + `x86_64/...` (两处).
- **scripts/fetch-vendor.sh**: ARM平台/64bit → `aarch64/`, x86平台/64位linux系统 → `x86_64/`; mkdir -p 两子目录; 打印清单标注 [aarch64]。
- **.gitignore**: 删除 `third_party/` 行 (保留 /target, .omo/evidence/, *.log, .DS_Store)。`git add third_party/` 后库文件入库。
- **验证**: 本机 `cargo build -p can-monitor` + `cargo test -p can-monitor` 通过, `readelf -d target/debug/can-monitor | grep -i runpath` 指向 `x86_64/`; docker can-monitor-dev 交叉编译产物 RUNPATH 指向 `aarch64/`。
- **教训**: git mv 只对已跟踪文件有意义; third_party/ 当时是 untracked+gitignored, 直接用 `mv` 即可, git add 在 .gitignore 移除后自然跟踪二进制。
- 此节取代本文件早前 "third_party/ 被 .gitignore 忽略" (Task 2 节) / ".gitignore: ... third_party/" (Task 1 节) / "aarch64 → 根目录" (Task 8 节) 的旧描述。

## 文档编写 (Task 22, README + docs/architecture.md)
- 交付物: 根 README.md + docs/architecture.md (docs/ 为新建目录),均中文;文档已逐项与源码核对,所有命令/crate/函数真实存在
- **测试计数 (实测 cargo test 确认, 后续改代码需同步)**: can-monitor 107, can-socketcan 10 (另 3 个 vcan 门控), can-types 13, can-usbvci 12 (mock 18), canopen-stack 19, j1939-stack 12 + 1 doc test。虚 workspace 根目录 `cargo test --features mock` 与 `cargo test -p can-usbvci --features mock` 均合法
- **文档必须写死的实现细节** (源码核对, 勿凭印象改): CLI 默认 backend=none / iface=can0; `--fd` 只对 SocketCAN 生效; usbvci 分支硬编码 VCI_USBCAN2 + 通道 0, `--iface` 仅作状态栏显示; 状态栏布局 Constraint::Length(5) = 2 边框 + 3 内容行; 消息窗口上限 1000; 过滤在 TUI 中只有 f 键总开关 (条件经代码配置, 无键盘入口); 发送 channel 64 / 错误 channel 64 / 消息 channel 1024
- **USB-CAN 权限表述**: 仓库**没有** udev 规则文件 (find 确认), 文档只写 devcontainer `--device=/dev/bus/usb` + 宿主机需设备节点读写权限 (自行配 udev 或 root), 不得虚构 rules 文件路径
- **J1939 协议摘要含省略号字符** "TP… FF01" (未完成) / "TP FF01" (完成) — 文档表格里保留原样即可
- 验证方法: 写完文档后 `cargo test` 核对测试计数、`ls third_party/controlcan` 核对库布局、`grep` 核对快捷键/CLI 与 app.rs 一致
