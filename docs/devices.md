# 设备扩展指南

本文档说明如何为 can-monitor 接入新的 CAN 设备后端 (第三方 USB 适配器、PCI/SPI 板卡、测试桩等)。内容基于 v2 的设备抽象 (Task 9/10/11/12 落地)。

## 1. 抽象全景

设备接入涉及三个层,每个层只有一个接口要对接:

```
┌─────────────────────────────────────────────┐
│ 上层消费方 (不需要知道你的后端)                 │
│  - TUI: 启动时按 --backend 打开后端            │
│  - Web:  GET /api/devices  + /api/monitor/start│
│  - GUI:  list_devices / start_monitor          │
└────────────────────┬────────────────────────┘
                     │
┌────────────────────▼────────────────────────┐
│ can-devices::DeviceManager (聚合层)           │
│  list_devices() = 并集(各后端 list_devices)   │
└────────────────────┬────────────────────────┘
                     │ 实现 DeviceDiscoverer
┌────────────────────▼────────────────────────┐
│ 你的新 crate (can-xxx)                       │
│  - impl CanBackend        (收发帧)           │
│  - impl DeviceDiscoverer  (枚举设备)          │
└────────────────────┬────────────────────────┘
                     │ 依赖
┌────────────────────▼────────────────────────┐
│ can-types (契约层, 唯一依赖)                  │
│  CanBackend / CanDeviceInfo / BackendConfig  │
└─────────────────────────────────────────────┘
```

## 2. 核心接口

### 2.1 `CanBackend` (帧收发,必实现)

```rust
pub trait CanBackend {
    /// 按配置打开后端。接口不存在 → NotFound; 数据超长 → FrameTooLong。
    fn open(config: &BackendConfig) -> Result<Self> where Self: Sized;
    /// 阻塞读一帧, 最多等 timeout。超时 → Timeout; 设备消失 → DeviceUnplugged。
    fn read_frame(&mut self, timeout: Duration) -> Result<CanFrame>;
    /// 写一帧到总线。不支持的帧 → Unsupported。
    fn write_frame(&mut self, frame: &CanFrame) -> Result<()>;
    /// 关闭并释放资源。
    fn close(&mut self) -> Result<()>;
}
```

实现要点:

- **同步 + 超时语义**: `read_frame` 必须支持 `timeout` (上层 reader 以 100ms 窗口轮询,超时是正常路径,返回 `CanError::Timeout`)
- **错误映射** (统一用 `can_types::Result<T>` / `CanError`):

  | 场景 | 返回 |
  |------|------|
  | 设备/接口不存在 | `CanError::NotFound` |
  | 设备热拔出 | `CanError::DeviceUnplugged` |
  | 总线错误 (位错误/仲裁丢失) | `CanError::BusError` |
  | 本后端不支持的操作 | `CanError::Unsupported("...")` |
  | ID 越界 (标准 > 0x7FF / 扩展 > 0x1FFFFFFF) | `CanError::InvalidId` |
  | 数据超长 (标准 > 8 字节) | `CanError::FrameTooLong` |
  | 底层 IO 失败 | `CanError::Io(io::Error)` |

- **帧转换**: 在 crate 内部把后端帧转成 `can-types` 的 `CanFrame`。参考范式: `can-socketcan` 的 `convert_*` / `to_*` 函数、`can-usbvci` 的 `vci_obj_to_frame`
- **参考实现**:
  - `crates/can-socketcan/src/real.rs` — 最简后端 (Linux netlink/socket)
  - `crates/can-usbvci/src/backend.rs` — FFI 后端 (VCI 动态加载 + 热插拔重连),`VciOps` trait 注入 mock 的测试范式
  - `crates/can-monitor-core/src/bus.rs` 的 `MockBackend` — 测试桩

### 2.2 `BackendConfig` (打开配置,扩展点)

```rust
pub enum BackendConfig {
    SocketCan { iface: String, fd: bool },
    UsbVci { device_type: u32, device_index: u32, channel: u32 },
    None,
}
```

- `open(&config)` 在 `open` 内 `match` 出自己对应的变体;若你的后端需要新配置字段,在 `BackendConfig` 加变体 (这是破坏性变更,注意编译错误会指出所有 match 点)
- 上层入口 (`main.rs` 的 `--backend` 匹配、`rest.rs` / `commands.rs` 的 `parse_device_id`) 各有一处后端名白名单,接入时需同步扩展 (见 §4)

### 2.3 `DeviceDiscoverer` (设备枚举,建议实现)

```rust
pub trait DeviceDiscoverer {
    /// 枚举当前可发现的设备。无设备 / 库未加载 → 空列表, 不 panic。
    fn list_devices() -> Vec<CanDeviceInfo>;
}
```

```rust
pub struct CanDeviceInfo {
    pub id: String,          // 唯一标识, 如 "can0" / "0" (设备索引)
    pub name: String,        // 面向用户的显示名
    pub kind: DeviceKind,    // SocketCan / UsbVci / Other(String)
    pub driver: String,      // 后端驱动标识 (如 crate 名)
    pub details: DeviceDetails, // 目前只有 model 字段
    pub available: bool,     // 已连接且可打开
}
```

实现要点:

- `DeviceDetails::with_model("...")` 是**关联函数**,不是方法 (不能链式 `.new().with_model(...)`)
- 新种类的设备用 `DeviceKind::Other("your_kind")` 或新增变体 (新增变体会破坏既有穷举 match,优先 `Other`)
- 参考实现:
  - `can-socketcan` 的 `SocketCanDiscoverer`: 扫描 `/sys/class/net`,`type == 280` (ARPHRD_CAN) 判定 + 前缀 fallback,`available` = `operstate == "up"`,`driver` 读 device/driver symlink
  - `can-usbvci` 的 `UsbVciDiscoverer`: 调 `VCI_FindUsbDevice2` 填充 `[VCI_BOARD_INFO; 16]` 缓冲,返回数 `.min(len)` 截断,`str_hw_Type` (CStr) 作为 model

## 3. 接入聚合层 (can-devices)

`DeviceManager::list_devices()` 目前聚合 SocketCAN + USBCAN。新增后端时在 `crates/can-devices/src/lib.rs` 追加:

```rust
pub fn list_devices() -> Vec<CanDeviceInfo> {
    Self::merge_many(vec![
        can_socketcan::SocketCanDiscoverer::list_devices(),
        can_usbvci::UsbVciDiscoverer::list_devices(),
        can_yourdevice::YourDiscoverer::list_devices(),   // ← 新增
    ])
}
```

- 聚合是**纯函数** (取并集 + 排序),不触碰硬件;各后端内部保证空列表不 panic
- 上层 (Web `GET /api/devices` / Tauri `list_devices` / TUI) 都经 `DeviceManager`,接入聚合层后三形态自动可见
- **测试注意**: 聚合测试用纯函数注入模拟数据,不要让聚合 crate 开启某后端的 mock feature (feature unification 会污染整个 workspace 的真实后端测试)

## 4. 接入上层入口 (三处)

CLI / REST / Tauri 各有一处后端名路由,格式均为 `name[:param]`:

| 入口 | 位置 | 需要改什么 |
|------|------|-----------|
| TUI CLI | `crates/can-monitor/src/main.rs` | `cli.backend.as_str()` 分支: 构造 `BackendConfig` → `open` → `bus.start_reader(backend, classifier, BackendKind::?)` (若需新 `BackendKind` 变体) → 状态栏显示名 |
| REST | `crates/can-monitor-server/src/rest.rs` | `parse_device_id` 的后端名白名单 (`matches!(backend, "socketcan"\|"usbvci"\|"none")`) 加入新名字;默认参数映射 |
| Tauri | `src-tauri/src/commands.rs` | 同样的 `parse_device_id` 白名单 + `start_monitor` 的 match 分支 |

注意 REST/Tauri 的 `parse_device_id` **只做格式校验**,不重新打开设备 (总线后端在启动时已固定,Web/GUI 只切换监控开关)。

## 5. 热插拔与重连 (USB 设备参考)

`can-usbvci` 的热插拔处理是现成范式:

- 用厂商 API 的"设备不存在"哨兵值检测拔出 (VCI: `VCI_GetReceiveNum` 返回 `0xFFFFFFFF` 即拔出),映射为 `CanError::DeviceUnplugged`
- 上层 reader 收到 `DeviceUnplugged` 后做 **≥2s 延迟重连,最多 5 次**,重枚举期间不崩溃
- `DeviceDiscoverer::list_devices()` 的 `available` 字段反映设备当前是否在位,上层据此置灰/隐藏设备项

## 6. 测试要求

- **纯转换逻辑**: 帧 ↔ `CanFrame` 转换写无硬件单元测试 (参考 `can-usbvci` 的 FFI 布局测试)
- **后端行为**: 用 trait 抽象注入测试桩。两个现成范式:
  - `can-usbvci` 的 `VciOps` (内部 trait) + `MockVciOps` 桩,经 `mock` feature 切换
  - `can-monitor-core` 的 `MockBackend` (实现 `CanBackend`)
- **枚举逻辑**: `SocketCanDiscoverer` 用 `std::env::temp_dir` 造 fake `/sys` 目录注入扫描函数
- 新增后端后跑全量门禁: `cargo test --workspace` + `clippy -D warnings` + `cargo fmt --check` + `scripts/check-core-purity.sh` (若动了 core)

## 7. 检查清单

- [ ] 新 crate 依赖 `can-types`,加入根 workspace `members`
- [ ] `CanBackend::open` 解释对应 `BackendConfig` 变体 (或新增变体),错误映射符合 §2.1 表
- [ ] `read_frame` 支持 timeout 语义;`write_frame` 对不支持能力返回 `Unsupported`
- [ ] `DeviceDiscoverer::list_devices` 空列表不 panic;`available` / `model` 字段填充正确
- [ ] `can-devices::DeviceManager::list_devices` 追加聚合
- [ ] 三处上层入口 (main.rs / rest.rs / commands.rs) 的后端名白名单已扩展
- [ ] 单元测试覆盖转换、行为 (mock) 与枚举逻辑
- [ ] 更新本文件设备表与 README 设备支持表 (含 CANFD 支持与否的如实声明)
