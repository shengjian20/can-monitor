# SSH + X11 远程使用指南 (无屏幕工控机)

> 场景: 本机为无屏幕的 Linux 工控机 (Xorg 运行于 `:1`, 3840x1080, 但无物理显示器)。日常操作为远程 SSH 接入。
> 目标形态有三种, 对 X11 的依赖各不相同, 按需选择:

| 形态 | 需要 X 吗 | 推荐接入方式 |
|------|-----------|--------------|
| **A. GUI** (Tauri 桌面 AppImage) | 需要, 走 X11 转发 | `ssh -X -C` + 客户端 X server |
| **B. TUI** (终端实时监控) | 不需要 | 普通 `ssh` |
| **C. Web** (浏览器界面) | 不需要 (无头模式最佳) | `ssh -L` 端口转发 |

服务端已就绪的事实 (本机, 2026-08 确认): `sshd` 已开 `X11Forwarding yes`; 已装 `xauth` / `xdpyinfo` / `xclock`; USB-CAN (CANalyst-II, `04d8:0053`) 的 udev 规则已装好, 普通用户可直接访问。

---

## A. GUI (Tauri 桌面应用, AppImage)

### 1. 客户端: 开启 X11 转发

| 客户端平台 | 做法 |
|-----------|------|
| Linux / macOS | 直接 `ssh -X -C user@ip` (系统自带 X server; 本机图形会话即显示器) |
| Windows (推荐) | **MobaXterm**: 内置 X server, 新建 SSH 会话即自动转发 X11, 开箱即用 |
| Windows (WSL2) | 安装 **VcXsrv** 或 **X410**, 在 WSL2 内 `ssh -X -C user@ip` (X410 需开启 Public Access) |

`-X` 是不受信任转发 (安全); `-C` 启用压缩 (GUI 走网络时能省不少带宽)。

连接后先验证转发是否成功:

```bash
echo $DISPLAY          # 应显示 localhost:10.0 之类, 而不是空
xclock                 # 能弹出时钟窗口即说明 X11 转发通了
```

### 2. 下载并运行 AppImage

> **前置条件**: AppImage 发布在 GitHub Release **v0.1.0**, 目前仍是 **draft** 状态。需先在 GitHub 上 Publish 该 release, 资产才可公开下载。

```bash
# 在工控机上 (需要 GitHub 账号已登录 gh CLI)
gh release download v0.1.0 -p '*AppImage'

chmod +x can-monitor_0.1.0_amd64.AppImage
./can-monitor_0.1.0_amd64.AppImage
```

AppImage 已内置厂商 ControlCAN 库, **开箱即用**, 无需另装依赖。GUI 通过 X11 转发渲染到客户端屏幕。

### 3. 排障

| 现象 | 原因 | 处理 |
|------|------|------|
| 服务端没有 `xauth` | 未安装 | 服务端 `sudo apt install xauth` (本机已装) |
| `X11 forwarding request failed on channel 0` | 服务端 `sshd_config` 未开 `X11Forwarding` 或无 `xauth` | 服务端确认 `X11Forwarding yes` 并重启 sshd; 安装 xauth |
| `DISPLAY` 未设置 / 为空 | 客户端没加 `-X` / `-Y`, 或客户端 ssh 配置禁了转发 | 显式 `ssh -X -C user@ip`; 也可在 `~/.ssh/config` 写 `ForwardX11 yes` |
| `connect local Unix socket /tmp/.X11-unix/X10: No such file or directory` | 客户端本机没有 X server (Linux 本机无图形会话, 或 Windows 没开 VcXsrv/X410) | 客户端安装/启动 X server; Windows 用 MobaXterm 则无需额外配置 |
| AppImage 报 FUSE 相关错误 (`fusermount` 不存在) | 系统无 FUSE | 改用解包运行: `./can-monitor_0.1.0_amd64.AppImage --appimage-extract-and-run` |
| 窗口能开但黑屏 / 渲染异常 | WebKitGTK 在无 GPU 环境的 EGL 问题 | 设 `WEBKIT_DISABLE_DMABUF_RENDERER=1` 强制软件渲染 (本机 Release CI 亦用此变量) |

---

## B. TUI (终端实时监控)

无需任何 X 依赖, 普通 SSH 即可:

```bash
ssh user@ip
cd ~/can_monitor   # 仓库目录按实际调整

# 方式 1: 源码运行
cargo run -p can-monitor -- --backend usbvci

# 方式 2: 已构建的二进制 (target/release/can-monitor 等)
./target/release/can-monitor --backend usbvci
```

说明:
- SSH 会话自带 TTY, 满足 ratatui 的终端要求; 但**不要在非 TTY 环境直接跑** (如后台 `nohup`, 会初始化失败退出)。Web 形态才是无头场景的答案。
- 后端选择: 无硬件调试用 `--backend none`; SocketCAN 用 `--backend socketcan --iface can0`; USB-CAN 用 `--backend usbvci`。
- 界面内按 `SPACE` 开始监控, `q` 退出。

---

## C. Web (浏览器界面, 无头模式最佳)

工控机无屏幕时**推荐**用此形态: Web 服务只绑定本机回环, 通过 SSH 端口转发访问, 全程零 X 依赖。

### 1. 客户端: 端口转发

```bash
ssh -L 8088:localhost:8088 user@ip
```

保持该会话不退出, 浏览器打开 <http://localhost:8088> 即连到工控机的 Web 界面。

### 2. 服务端: 启动 Web 服务

```bash
can-monitor --web-write --backend usbvci --web-port 127.0.0.1:8088
```

- `--web-write`: 开启写模式, Web 界面才允许发送帧 (默认只读)。
- `--backend usbvci`: 连接 USB-CAN (CANalyst-II)。**注意 CLI 的 `--backend` 只接受 `socketcan|usbvci|none` 三种取值**, `usbvci:0` 是 Web 界面 / REST API 里的设备 id 格式 (如 `POST /api/monitor/start`, body `{"device_id":"usbvci:0"}`), 不要混用。
- `--web-port 127.0.0.1:8088`: 监听地址 (安全锁定, 仅接受本机回环; 默认 127.0.0.1:8080)。`-L 8088` 与这里的 `8088` 对应。
- 也可用已构建二进制直接跑 (无 TUI 干扰); 若在 TTY 下运行会同时拉起 TUI, 不影响 Web 服务。

---

## USB 权限 (udev 规则)

本机已装好规则: 节点 `/dev/bus/usb/*/*` 为 `crw-rw-rw-`, 非 root 用户可直接访问 USB-CAN。规则文件已随仓库提供: `scripts/99-usbcan.rules`。

换到新机器时:

```bash
sudo cp scripts/99-usbcan.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

重新插拔设备后生效。规则内容 (同 J-Link 模式, 放行 USB-CAN 全权限):

```
# CANalyst-II (USB-CAN, 04d8:0053) — 非 root 用户可访问 (同 J-Link 模式)
SUBSYSTEM=="usb", ATTR{idVendor}=="04d8", ATTR{idProduct}=="0053", MODE="0666"
```

---

## 安全提示

- **X11 转发有信任边界**: `-X` 是不受信任转发, X client 受 X security 扩展约束, 推荐; `-Y` 是受信任转发, 应用可访问客户端整个 X server (可读屏、注入按键), **仅在必须且可信时才用**。
- 图形会话走网络时会放大风险 (画面可能被截获), 仅在可信网络使用。
- **推荐用 SSH key 认证** (`ssh-keygen` + `ssh-copy-id user@ip`), 避免密码; 配合 `-C` 压缩。
- Web 形态的 `--web-port` 只能绑定本机回环, 天然不对外; 端口转发由 SSH 加密隧道承载, 安全。
- 不要关闭 `X11Forwarding` 的 `xauth` 校验 (keep `xauth` 路径默认), 否则等同放开转发不受限。

---

## 相关文档

- [README.md](../README.md) — 项目总览, 三种形态的本地启动方式
- [docs/web-api.md](web-api.md) — Web REST / WebSocket 接口契约
- [docs/architecture.md](architecture.md) — 架构与三形态接线
