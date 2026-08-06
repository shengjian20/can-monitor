# 注意: 未使用 `# syntax=docker/dockerfile:1` 指令 — 它会额外拉取 docker/dockerfile:1
# 前端镜像 (auth.docker.io), 在某些受限网络 (仅 registry 镜像可达) 下构建失败。
# BuildKit 内置前端已足够, 无需显式指定。
# ============================================================================
# can-monitor — 多阶段 Docker 构建 (TUI + Web 一体)
#
# 产物:
#   /usr/local/bin/can-monitor      TUI + 内嵌 Web 服务 (axum) 的可执行文件
#   /usr/local/bin/libcontrolcan.so 周立功 USB-CAN 运行时库 (与二进制同目录)
#   /app/web/dist                   Web 前端静态资源 (由 node 阶段构建)
#
# 关键决策:
#   1. can-monitor-server 是「库」crate (axum Web 服务内嵌于 can-monitor 二进制),
#      不存在独立的 can-monitor-server 可执行文件 — 本镜像只有一个二进制。
#   2. can-usbvci 默认「动态加载」模式 (CAN_USBVCI_LINK_MODE 缺省 dynamic):
#      构建期不链接厂商库, 运行时经 libloading 按
#      CAN_USBVCI_LIB 环境变量 → exe 同目录 → 系统搜索 的顺序解析 .so。
#      → 把 libcontrolcan.so 放到与二进制同目录 /usr/local/bin/ 即可开箱即用,
#        再设 ENV CAN_USBVCI_LIB 双保险。
#   3. libcontrolcan.so 的 ldd 只依赖 libc/libpthread (无 libudev),
#      debian:bookworm-slim 开箱满足, 无需额外系统库。
#   4. Web 服务受安全锁定仅绑定本机回环 (Metis), 且 Web 前端固定调用 :8080 —
#      因此容器内 Web 默认监听 127.0.0.1:8088 (见 EXPOSE), 从宿主机访问需
#      --network host (回环落在宿主机), 端口映射 (-p) 无法穿透, 见 README。
# ============================================================================

# ---- Stage 1: Web 前端静态资源 -------------------------------------------
FROM node:22-bookworm-slim AS web-builder
WORKDIR /build
# 先拷清单以复用依赖层缓存; 其余文件变化不触发 npm ci
COPY web/package.json web/package-lock.json ./
COPY web/tsconfig.json web/vite.config.ts web/index.html ./
COPY web/src ./src
RUN npm ci && npm run build

# ---- Stage 2: Rust 构建 ----------------------------------------------------
FROM rust:1.97-bookworm AS builder
WORKDIR /src
# 依赖层缓存: 仅 Cargo 清单变化时重跑依赖编译
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# can-usbvci / can-monitor 的 build.rs 需要 third_party/controlcan 存在
# (dynamic 模式仅 canonicalize + 注入 rpath, 不链接 — 无需 libusb-dev 等系统库)
COPY third_party ./third_party
# --locked: 用提交的 Cargo.lock 保证可复现
RUN cargo build --release --locked -p can-monitor

# ---- Stage 3: 运行时 -------------------------------------------------------
FROM debian:bookworm-slim AS runtime
# curl: 供健康检查/容器内调试 (体积可忽略)
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
# Web 静态资源: 服务器按进程 CWD 相对路径 web/dist 托管 (web 形态开箱即用)
COPY --from=web-builder /build/dist /app/web/dist
# 两个二进制产物级文件与可执行文件同目录:
#   libcontrolcan.so 与 can-monitor 同目录 → can-usbvci exe-dir 优先解析直接命中
COPY --from=builder /src/target/release/can-monitor /usr/local/bin/can-monitor
COPY third_party/controlcan/x86_64/libcontrolcan.so /usr/local/bin/libcontrolcan.so

# 双保险: resolve_library 第一优先级的环境变量指向 .so
ENV CAN_USBVCI_LIB=/usr/local/bin/libcontrolcan.so

# Web 端口 (容器内监听 127.0.0.1:8088; 宿主机访问用 --network host, 见 README)
EXPOSE 8088

# --web-write 固定由 ENTRYPOINT 注入: 用户在 CMD 位置追加的参数 (如 --backend none)
# 不会覆盖掉它, 保证 Web 服务始终随容器启动
ENTRYPOINT ["can-monitor", "--web-write"]
# 默认后端 socketcan: 需要容器以 --cap-add NET_ADMIN 运行并准备好 vcan 接口
# (宿主机 vcan0 经 --network host 共享, 见 scripts/vcan-setup.sh);
# 无设备/纯 Web 演示改用: docker run ... <镜像> --backend none
# 注意: 程序是 TUI 优先, 运行需要 TTY — 交互用 -it, 后台用 -dt (分配伪终端)
CMD ["--backend", "socketcan", "--web-port", "127.0.0.1:8088"]
