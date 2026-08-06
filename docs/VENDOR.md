# 供应商 SDK 来源与许可说明

> 本文档声明本项目所依赖的第三方供应商 SDK 的来源、版本与分发策略, 供公开仓库发布前审计。
> 状态: 已随 Task 23 (公开发布文档) 定稿 — 分发策略见下;再分发许可的最终确认以供应商 (ZLG) 的 EULA 为准。

## ControlCAN / ZLGCAN USB-CAN SDK

- **供应商**: 周立功 (ZLG) / 广州致远电子 — USB-CAN 系列设备 VCI 驱动与 SDK
- **来源包**: `CAN分析仪资料20250624_Linux/` (仓库根目录下随附的厂商资料压缩包, 本地工作目录)
- **版本**: Linux 资料包 **V1.45** (位于 `CAN分析仪资料20250624_Linux/CAN分析仪资料20250618_Linux/Linux资料包V1.45/`)
- **仓库内使用方式**: `scripts/fetch-vendor.sh` 从 SDK `Linux资料包V1.45/二次开发库文件` 拷贝必要文件到 `third_party/controlcan/` (aarch64/ + x86_64/ + win64/ 对称布局), `third_party/` 跟随源码提交发布

### 仓库内布局

```
third_party/controlcan/
  aarch64/    libcontrolcan.{a,so}   (ARM 平台/64bit linux, SDK V1.45)
  x86_64/     libcontrolcan.so       (x86 平台/64bit linux, libusbcan_v351, 固件 3.51 专用)
  win64/      ControlCAN.dll         (Windows x64, PE32+)
  controlcan.h                       (架构无关头文件)
```

> **x86_64 驱动替换 (v0.1.4)**: `third_party/controlcan/x86_64/libcontrolcan.so` 已由 Linux 资料包 V1.45 原版替换为 **libusbcan_v351** (固件 **3.51** 专用驱动)。详见下方「x86_64 驱动替换」一节。

### x86_64 驱动替换: libusbcan_v351 (固件 3.51 专用)

- **背景**: CAN-Linux 设备 (固件 3.51) 用 SDK V1.45 的 `libcontrolcan.so` 无法打开 (`Device or resource busy`);用户提供的 `libusbcan_v351` 配合设备类型 4 (VCI_USBCAN2) + 标准序列号**完全打通** — 真机实测 OpenDevice/InitCAN/StartCAN/Transmit (0x181, SendType=1) 全部返回 1, 二进制 `--backend usbvci` 无后端错误
- **来源路径**: `/media/raw/filespace/test/can_service/usbcan_driver/lib/` (用户提供的 v351 驱动目录, 固件 3.51 专用)
- **md5**: `2ec9b05066ba44b67cfec7d535f99763` (已替换入库的 `third_party/controlcan/x86_64/libcontrolcan.so`)
- **动态依赖**: `readelf -d` 确认 NEEDED `libusb-1.0.so.0` + `libc.so.6`, glibc 需求 ≤ **2.14** → **目标系统需安装 `libusb-1.0`** (宿主已装; Ubuntu: `apt install libusb-1.0-0`)
- **aarch64 保持 SDK V1.45**: 用户未提供 v351 的 arm64 版 (来源目录只有 x86_64 库), 故 `third_party/controlcan/aarch64/` 保持 Linux 资料包 V1.45 原版库 (md5 `59a2d704ccf3756fd11a4871b92ade5c`)。若后续获得 v351 arm64 版再对称替换

- Windows `ControlCAN.dll` 来源: SDK `二次开发库文件/x64(64bit)/ControlCAN.dll` (sha256 `6d151f92217983c39a6690ded76b41f86ebad7570bcc27fc9d13f7141425b1e3`), 随源码提交并打入 Tauri 发行包资源 (落位 `$RESOURCE/ControlCAN.dll` == exe 所在目录 → 运行时 exe-dir-first 加载命中, 开箱即用)
- **Windows `usbcan64.dll`**: 不在 SDK 归档内 — `硬件驱动程序(手动安装).rar` 只含驱动安装文件 (`*.inf` / `*.sys` / WinUSB·WDF 联合安装器), `usbcan64.dll` 由驱动安装器写入 `System32`。**无需随发行包携带**: 用户安装厂商驱动后自动就绪 (ControlCAN.dll 调用链不需要用户手动放置该 DLL)
- macOS: 无需厂商库 (mock 逃生舱, 无 macOS 供应商库)

### 分发策略 (Task 23 定稿)

- [x] **不提交** `CAN分析仪资料20250624_Linux/` (334MB, 212 个文件, 含 >50MB 的 Windows 安装包) 到公开仓库 (用户决策, 已在 .gitignore 忽略)
- [x] `third_party/controlcan/` 中厂商二进制 `libcontrolcan.so` / `.a` / `ControlCAN.dll` **随源码提交** (用户决策);再分发许可以 ZLG SDK 最终用户许可协议为准, 使用方需自行确认
- [x] 二进制产物无 SONAME (`libcontrolcan.so`) — 交叉编译脚本 `build-cross.sh` 用 `patchelf --set-soname` 修复, 见 README「交叉编译」

### 大文件清单 (>50MB, 来源包内)

| 文件 | 大小 |
|------|------|
| `调试工具/原厂调试工具/USB_CAN TOOLSetup V9.114.exe` | 105 MB |
| `调试工具/周立功ZLG调试工具/CANPro协议分析平台V1.50/CANPro_Setup1.50.2.367.exe` | 53 MB |

> 若公开仓库需要保留来源包, 建议改用外部下载链接 + Git LFS 或仅保留编译所需子集, 避免将厂商安装包直接入库。
