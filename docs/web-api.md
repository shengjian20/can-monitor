# Web API 文档

本文档是 `can-monitor-server` 对外暴露的 HTTP + WebSocket 契约。Web 前端 (`web/`) 与 GUI (`src-tauri/`) 共用同一套帧 JSON 格式,浏览器形态经本 API 通信,Tauri 形态经 IPC 通信但数据形状一致。

## 0. 启动与安全

Web 服务由 CLI 以 `--web-write` 拉起 (REST + WebSocket + 静态页面同一端口):

```bash
cargo run -- --backend none --web-write          # 默认 127.0.0.1:8080
cargo run -- --backend socketcan --iface can0 --web-write --web-port 127.0.0.1:9000
```

- **默认只读**: 不带 `--web-write` 时服务不会启动 (CLI 仅在写模式拉起)
- **仅限回环**: 监听地址只接受 `127.0.0.1` / `localhost` (`parse_bind_addr` 强制),拒绝 `0.0.0.0`、局域网与公网地址,不提供 LAN 绑定
- **请求体上限**: 64KB (`DefaultBodyLimit`),帧体远小于此
- **静态托管**: `web/dist` 存在时作为未匹配路由的 fallback;不存在则 `GET /` 404

## 1. 端点一览

| 方法 | 路径 | 作用 | 写操作 |
|------|------|------|--------|
| GET  | `/api/devices`        | 设备列表 (SocketCAN + USBCAN 聚合) | 否 |
| POST | `/api/monitor/start`  | 开启监控 (body: `{"device_id": ...}`) | 否 (读侧控制) |
| POST | `/api/monitor/stop`   | 关闭监控 | 否 (读侧控制) |
| POST | `/api/send`           | 发送一帧 (body: `{id, ext, data}`) | **是 — 需写模式** |
| GET  | `/api/status`         | 状态快照 (running / 计数器) | 否 |
| GET  | `/ws`                 | WebSocket 批量帧流 | 否 |

> `device_id` 格式: `socketcan` / `socketcan:can0` / `usbvci` / `usbvci:0` / `none`。服务端只做格式校验,不重新打开设备 (总线后端在 CLI 启动时已固定)。

## 2. REST 端点详情

### 2.1 `GET /api/devices`

返回设备 JSON 数组,无设备时为空数组 `[]` (不 panic)。

```json
[
  {
    "id": "can0",
    "name": "can0",
    "kind": "SocketCan",
    "driver": "socketcan",
    "model": "SocketCAN",
    "available": true
  },
  {
    "id": "0",
    "name": "USBCAN-II",
    "kind": "UsbVci",
    "driver": "usbvci",
    "model": "USBCAN-II",
    "available": false
  }
]
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | string | 设备唯一标识 (`socketcan:can0` 格式中的参数部分) |
| `name` | string | 面向用户的显示名 |
| `kind` | string | `"SocketCan"` / `"UsbVci"` / `"Other(...)"` (Debug 输出) |
| `driver` | string | 后端驱动标识 |
| `model` | string | 设备型号 (VCI `str_hw_Type` 或 `"SocketCAN"`) |
| `available` | bool | 当前是否已连接且可打开 |

### 2.2 `POST /api/monitor/start`

请求:

```json
{ "device_id": "socketcan:can0" }
```

响应:

- `200`: `{"ok": true, "device": "socketcan"}`
- `400`: `{"error": "未知设备类型: bogus"}` (device_id 格式非法)

### 2.3 `POST /api/monitor/stop`

无请求体 (传 `"{}"` 也接受)。响应:

- `200`: `{"ok": true, "running": false}`

### 2.4 `POST /api/send`

请求:

```json
{ "id": "0x123", "ext": false, "data": "01 02 0A" }
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | string | CAN ID,十进制或 `0x` 前缀十六进制 |
| `ext` | bool | 是否 29 位扩展帧 |
| `data` | string | 空格分隔的十六进制字节 (如 `"01 02 0A"`),最多 8 字节 |

响应:

| 状态码 | 含义 | body |
|--------|------|------|
| `200` | 发送成功 | `{"ok": true}` |
| `403` | **写未启用** (服务未以 `--web-write` 启动) | `{"error": "写入未启用: 服务需以 --web-write 启动才允许发送帧"}` |
| `400` | 帧格式非法 (ID 越界 / 数据字节非 hex / 数据 > 8 字节) | `{"error": "CAN ID 错误: ..."}` 等 |
| `500` | 总线错误 (发送队列满等) | `{"error": "..."}` |

### 2.5 `GET /api/status`

```json
{
  "running": true,
  "total": 1234,
  "canopen": 800,
  "j1939": 300,
  "error": 0,
  "dropped": 17
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `running` | bool | 是否正在监控 |
| `total` | u64 | 已读帧总数 |
| `canopen` | u64 | CANopen 帧计数 |
| `j1939` | u64 | J1939 帧计数 |
| `error` | u64 | 后端错误计数 |
| `dropped` | u64 | 丢弃帧计数 (慢消费者队列满) |

## 3. WebSocket 帧流 (`GET /ws`)

### 3.1 连接

```
ws://127.0.0.1:8080/ws
```

客户端连入后,服务器订阅广播流,以**批量 JSON 数组**下行推送帧。单向下行流: 客户端消息除 Ping (回 Pong) 外被忽略,Close 正常关闭。

### 3.2 批量与心跳

- 攒满 **50 帧**立即刷出,或每 **30ms** 到点刷出 (两者取先)
- 无帧时发送**空数组 `[]`** 作为心跳 (保活 + 让前端渲染循环稳定走 tick)
- 连接断开时: 服务器退订广播 → 桥接线程退出 → 连接关闭,无泄漏

```json
[
  { "ts": "1750000000000", "id": "0x181", "ext": false, "dir": "rx",
    "data": "01 02 03", "protocol": "canopen", "summary": "Pdo { node: 1, ... }" },
  { "ts": "1750000000123", "id": "0x18FEF100", "ext": true, "dir": "rx",
    "data": "01 02", "protocol": "j1939", "summary": "Direct { pgn: 65265, ... }" }
]
```

### 3.3 帧 JSON schema

一帧的字段 (三形态统一契约,Web REST 层、WS 层、Tauri Channel 一致):

```jsonc
{
  "ts": "1750000000000",      // string — 毫秒时间戳 (u64), 必须用字符串 (JS Number 精确上限 2^53, u64 会溢出)
  "id": "0x181",              // string — 十六进制 CAN ID, 小写 0x 前缀 + 大写十六进制数字 (如 0x18FEF100)
  "ext": false,               // bool  — 是否 29 位扩展帧
  "dir": "rx",                // string — 收发方向: "rx" / "tx"
  "data": "01 02 03",         // string — 大写十六进制、空格分隔的数据字节, 标准帧最多 8 字节
  "protocol": "canopen",      // string — "canopen" / "j1939" / "raw" 三值之一
  "summary": "Pdo { ... }"    // string — 人类可读摘要 (协议栈解析结果 Debug 输出; raw 帧为空串)
}
```

| 字段 | 类型 | 必填 | 约束 |
|------|------|------|------|
| `ts` | string | ✅ | u64 毫秒,字符串编码 |
| `id` | string | ✅ | 十六进制,`0x` 前缀 |
| `ext` | bool | ✅ | |
| `dir` | string | ✅ | `"rx"` / `"tx"` |
| `data` | string | ✅ | 大写 hex 空格分隔 |
| `protocol` | string | ✅ | 枚举: `canopen` / `j1939` / `raw` |
| `summary` | string | ✅ | raw 帧恒为空串 |

协议判定: 11 位标准帧 → CANopen;29 位扩展帧 → J1939;其余 → raw。分类发生在 reader 线程 (单次分类,`StreamItem`),本层不重新分类。

## 4. 错误码约定

- 成功响应 body 为 JSON 对象;错误响应统一 `{"error": "<消息>"}` + 对应 HTTP 状态码
- `400` 请求体/参数格式非法,`403` 写门禁拒绝,`500` 服务端 (总线) 错误
- `POST /api/send` 的 403 在**解析请求体之前**判定,与请求内容无关 (安全: 未启用写模式不处理任何写意图)

## 5. 前端参考实现

浏览器端契约实现见 `web/src/api.ts` (`HttpApi`): base URL `http://{hostname}:8080/api`,WS `ws://{hostname}:8080/ws`;批量数组逐帧回调,`[]` 心跳自然跳过。Tauri 端实现见 `src-tauri/src/commands.rs` (7 个 `#[tauri::command]`),帧经 `tauri::ipc::Channel` 单帧推送。

## 6. 兼容性约定

- **破坏性变更** (字段改名 / 增删 / 类型变化) 需同步修改: `frame.rs` (服务端序列化)、`commands.rs` (Tauri)、`api.ts` / `types.ts` (前端)、本文档,并跑三形态对齐验证 (T27 流程)
- 新增只读端点向后兼容;修改 `device_id` 白名单需同步 docs/devices.md
