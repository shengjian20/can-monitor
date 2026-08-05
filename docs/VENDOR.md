# 供应商 SDK 来源与许可说明

> 本文档声明本项目所依赖的第三方供应商 SDK 的来源、版本与分发策略, 供公开仓库发布前审计。
> 状态: 已随 Task 23 (公开发布文档) 定稿 — 分发策略见下;再分发许可的最终确认以供应商 (ZLG) 的 EULA 为准。

## ControlCAN / ZLGCAN USB-CAN SDK

- **供应商**: 周立功 (ZLG) / 广州致远电子 — USB-CAN 系列设备 VCI 驱动与 SDK
- **来源包**: `CAN分析仪资料20250624_Linux/` (仓库根目录下随附的厂商资料压缩包, 本地工作目录)
- **版本**: Linux 资料包 **V1.45** (位于 `CAN分析仪资料20250624_Linux/CAN分析仪资料20250618_Linux/Linux资料包V1.45/`)
- **仓库内使用方式**: `scripts/fetch-vendor.sh` 从 SDK `Linux资料包V1.45/二次开发库文件` 拷贝必要文件到 `third_party/controlcan/` (aarch64/ + x86_64/ 对称布局), `third_party/` 跟随源码提交发布

### 分发策略 (Task 23 定稿)

- [x] **不提交** `CAN分析仪资料20250624_Linux/` (334MB, 212 个文件, 含 >50MB 的 Windows 安装包) 到公开仓库 (用户决策, 已在 .gitignore 之外保持 untracked)
- [x] `third_party/controlcan/` 中厂商二进制 `libcontrolcan.so` / `.a` **随源码提交** (用户决策);再分发许可以 ZLG SDK 最终用户许可协议为准, 使用方需自行确认
- [x] 二进制产物无 SONAME (`libcontrolcan.so`) — 交叉编译脚本 `build-cross.sh` 用 `patchelf --set-soname` 修复, 见 README「交叉编译」

### 大文件清单 (>50MB, 来源包内)

| 文件 | 大小 |
|------|------|
| `调试工具/原厂调试工具/USB_CAN TOOLSetup V9.114.exe` | 105 MB |
| `调试工具/周立功ZLG调试工具/CANPro协议分析平台V1.50/CANPro_Setup1.50.2.367.exe` | 53 MB |

> 若公开仓库需要保留来源包, 建议改用外部下载链接 + Git LFS 或仅保留编译所需子集, 避免将厂商安装包直接入库。
