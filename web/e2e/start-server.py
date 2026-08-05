#!/usr/bin/env python3
"""启动 can-monitor Web 服务器 (pty 包装)。

ratatui TUI 需要 TTY, 非 TTY 环境 (如无 pty 的管道 spawn) 会 panic (ENOTTY)。
本脚本用 pty.fork() 给 can-monitor 一个伪终端, 保证 TUI 正常初始化,
同时 Web 服务 (--web-write) 照常提供 REST + WS + 静态 web/dist。

用法: python3 start-server.py [pidfile]
  pidfile 默认为 /tmp/canmonitor-e2e.pid, 写入 can-monitor 子进程 PID (供 teardown 精确 kill)。
"""
import os
import pty
import sys

BIN = "./target/debug/can-monitor"
ARGS = [
    BIN,
    "--backend", "none",
    "--web-write",
    "--web-port", "127.0.0.1:8080",
]
PIDFILE = sys.argv[1] if len(sys.argv) > 1 else "/tmp/canmonitor-e2e.pid"

pid, fd = pty.fork()
if pid == 0:
    # 子进程: exec 服务器二进制 (stdout=stderr=pty 从端)。
    os.execv(BIN, ARGS)
    os._exit(127)  # exec 失败时才到达

# 父进程: 记录子 PID, 排空 pty 主端输出。
# 不排空的话, ratatui 写满 pty 缓冲后 child 会阻塞, 探活将超时。
with open(PIDFILE, "w") as f:
    f.write(str(pid))

try:
    while True:
        data = os.read(fd, 4096)
        if not data:
            break
except OSError:
    # EIO: 子进程退出, 正常结束。
    pass

try:
    os.close(fd)
except OSError:
    pass
os._exit(0)
