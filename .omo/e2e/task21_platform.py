#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Task 21 platform harness — RK3588 实测 (jz@172.22.2.242, can0 loopback).

流程:
  1. 预置: can0 loopback 500k UP (ssh sudo), scp task21_peer.py
  2. 启 TUI: ssh -t 运行 /tmp/can-monitor --backend socketcan --iface can0 --log-file /tmp/can-monitor.log
  3. 监控 ON (space) → 外部 peer 发 123#DEADBEEF → 断言 TUI 显示帧
  4. TX 路径: TUI 发送面板发 Raw 帧, 外部 peer recv 捕获 (loopback 回环)
  5. q 退出 → 断言平台日志 /tmp/can-monitor.log 含 candump 格式帧

证据: .omo/evidence/task-21-platform-test.txt / .png, task-21-platform-log.txt
"""
import fcntl
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import time
import unicodedata

HOST = "jz@172.22.2.242"
ROWS, COLS = 40, 120
EVIDENCE = "/media/raw/filespace/test/can_monitor/.omo/evidence"


class Screen:
    """迷你 ANSI 终端解码器 (同 Task 20 harness)。"""

    def __init__(self, rows=ROWS, cols=COLS):
        self.rows, self.cols = rows, cols
        self.grid = [[" "] * cols for _ in range(rows)]
        self.cr, self.cc = 0, 0
        self.buf = ""

    def feed(self, data: bytes):
        self.buf += data.decode("utf-8", "replace")
        while self.buf:
            if self.buf.startswith("\x1b["):
                m = re.match(r"\x1b\[([0-9;?]*)([A-Za-z])", self.buf)
                if not m:
                    return
                self._csi(m.group(1), m.group(2))
                self.buf = self.buf[m.end():]
            elif self.buf.startswith("\x1b"):
                if len(self.buf) < 2:
                    return
                self.buf = self.buf[2:]
            else:
                ch = self.buf[0]
                self.buf = self.buf[1:]
                self._put(ch)

    def _num(self, ps, i, d=1):
        try:
            v = int(ps[i]) if i < len(ps) and ps[i] else d
            return v
        except ValueError:
            return d

    def _csi(self, params, cmd):
        ps = [p for p in params.split(";")] if params else []
        if cmd in ("H", "f"):
            self.cr = max(min(self._num(ps, 0, 1) - 1, self.rows - 1), 0)
            self.cc = max(min(self._num(ps, 1, 1) - 1, self.cols - 1), 0)
        elif cmd == "A":
            self.cr = max(self.cr - self._num(ps, 0), 0)
        elif cmd == "B":
            self.cr = min(self.cr + self._num(ps, 0), self.rows - 1)
        elif cmd == "C":
            self.cc = min(self.cc + self._num(ps, 0), self.cols - 1)
        elif cmd == "D":
            self.cc = max(self.cc - self._num(ps, 0), 0)
        elif cmd == "G":
            self.cc = max(min(self._num(ps, 0, 1) - 1, self.cols - 1), 0)
        elif cmd == "J":
            mode = ps[0] if ps and ps[0] else "0"
            if mode in ("0",):
                for c in range(self.cc, self.cols):
                    self.grid[self.cr][c] = " "
            elif mode == "1":
                for c in range(0, self.cc + 1):
                    self.grid[self.cr][c] = " "
            elif mode == "2":
                self.grid = [[" "] * self.cols for _ in range(self.rows)]
        elif cmd == "K":
            for c in range(self.cc, self.cols):
                self.grid[self.cr][c] = " "
        elif cmd == "h" and params.startswith("?1049"):
            self.grid = [[" "] * self.cols for _ in range(self.rows)]
            self.cr = self.cc = 0
        elif cmd == "l" and params.startswith("?1049"):
            self.grid = [[" "] * self.cols for _ in range(self.rows)]
            self.cr = self.cc = 0

    def _put(self, ch):
        if ch == "\r":
            self.cc = 0
            return
        if ch == "\n":
            self.cr = min(self.cr + 1, self.rows - 1)
            return
        if ch == "\t":
            self.cc = min(self.cc + (8 - self.cc % 8), self.cols - 1)
            return
        if ord(ch) < 32:
            return
        width = 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1
        if 0 <= self.cr < self.rows and self.cc < self.cols:
            self.grid[self.cr][self.cc] = ch
            if width == 2 and self.cc + 1 < self.cols:
                self.grid[self.cr][self.cc + 1] = "\u200b"
        self.cc += width
        if self.cc >= self.cols:
            self.cc = 0
            self.cr = min(self.cr + 1, self.rows - 1)

    def text(self):
        lines = []
        for r in self.grid:
            line = "".join(r).replace("\u200b", "").rstrip()
            lines.append(line)
        while lines and lines[-1] == "":
            lines.pop()
        return "\n".join(lines)


def run_ssh(cmd, tmo=30):
    """本地执行远程命令 (非 pty), 返回 stdout 文本。"""
    p = subprocess.run(
        ["ssh", "-o", "ConnectTimeout=10", HOST, cmd],
        capture_output=True, text=True, timeout=tmo,
    )
    return p.stdout + p.stderr, p.returncode


class PTYHarness:
    """经 ssh -t 在远端启 TUI, 本地 pty 驱动按键 + 定时快照。"""

    def __init__(self, remote_cmd, actions, tmo=25.0):
        self.remote_cmd = remote_cmd
        self.actions = sorted(actions, key=lambda a: a[0])
        self.tmo = tmo
        self.raw = b""
        self.snaps = {}
        self.exitcode = None

    def run(self):
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        pid = os.fork()
        if pid == 0:
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
            os.dup2(slave, 0)
            os.dup2(slave, 1)
            os.dup2(slave, 2)
            if slave > 2:
                os.close(slave)
            os.execv("/usr/bin/ssh", [
                "/usr/bin/ssh",
                "-o", "ConnectTimeout=10",
                "-o", "StrictHostKeyChecking=no",
                "-t", "-t", HOST, self.remote_cmd,
            ])
        os.close(slave)
        os.set_blocking(master, False)

        screen = Screen()
        start = time.time()
        ai = 0
        while True:
            now = time.time() - start
            if now >= self.tmo:
                break
            while ai < len(self.actions) and self.actions[ai][0] <= now:
                t, kind, payload = self.actions[ai]
                ai += 1
                if kind == "key":
                    try:
                        os.write(master, payload)
                    except OSError:
                        pass
                elif kind == "shell":
                    subprocess.Popen(payload, shell=True,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                elif kind == "snap":
                    self.snaps[payload] = screen.text()
            r, _, _ = select.select([master], [], [], 0.05)
            if r:
                try:
                    data = os.read(master, 8192)
                except OSError:
                    data = b""
                if data:
                    self.raw += data
                    screen.feed(data)
            try:
                wpid, status = os.waitpid(pid, os.WNOHANG)
                if wpid == pid:
                    self.exitcode = os.waitstatus_to_exitcode(status)
                    break
            except ChildProcessError:
                break

        if self.exitcode is None:
            try:
                os.kill(pid, signal.SIGKILL)
                _, status = os.waitpid(pid, 0)
                self.exitcode = os.waitstatus_to_exitcode(status)
            except Exception:
                self.exitcode = None

        while True:
            r, _, _ = select.select([master], [], [], 0.05)
            if not r:
                break
            try:
                data = os.read(master, 8192)
            except OSError:
                break
            if not data:
                break
            self.raw += data
            screen.feed(data)
        self.screen = screen
        try:
            os.close(master)
        except OSError:
            pass
        return self


def has_token(text, tok):
    return re.search(r"(^|\s)" + re.escape(tok) + r"(\s|$)", text) is not None


def counts(text):
    m = re.search(r"帧:(\d+)", text)
    return int(m.group(1)) if m else None


def main():
    os.makedirs(EVIDENCE, exist_ok=True)
    # --- 0. 预置: 配置 can0 loopback 500k UP + 部署 peer 脚本 ---
    pre, rc = run_ssh("sudo -n /sbin/ip link set can0 type can bitrate 500000 loopback on && "
                      "sudo -n /sbin/ip link set can0 up && "
                      "ip -details link show can0 | head -5 && "
                      "rm -f /tmp/can-monitor.log /tmp/task21_rx.txt")
    print("== 预置 can0 ==")
    print(pre)
    subprocess.run(["scp", "-q",
                    "/media/raw/filespace/test/can_monitor/.omo/e2e/task21_peer.py",
                    f"{HOST}:/tmp/task21_peer.py"], check=True)

    remote_cmd = ("cd /tmp && LD_LIBRARY_PATH=/tmp TERM=xterm-256color "
                  "/tmp/can-monitor --backend socketcan --iface can0 "
                  "--log-file /tmp/can-monitor.log")

    # peer recv 后台启动: 监听 can0, 捕获 TUI 发送面板发出的帧 (loopback 回环到其他 socket)
    recv_proc = subprocess.Popen(
        ["ssh", "-o", "ConnectTimeout=10", HOST,
         "python3 /tmp/task21_peer.py recv 12 /tmp/task21_rx.txt"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    actions = [
        (0.8, "snap", "start"),
        (1.2, "key", b" "),                    # 监控 ON
        (1.8, "snap", "mon_on"),
        (2.0, "shell", "ssh -o ConnectTimeout=10 %s 'python3 /tmp/task21_peer.py send 123 DEADBEEF'" % HOST),
        (3.0, "snap", "rx_frame"),
        # --- TX: 发送面板发 Raw 帧 124#A1B2C3 ---
        # 流程: x 打开 → 4 选 原始帧 → Enter 确认 → 填 ID → Tab → 填数据 → Enter 发送
        (3.4, "key", b"x"),                    # 打开发送面板 (SelectType)
        (3.8, "snap", "send_panel"),
        (4.0, "key", b"4"),                    # 选择 原始帧
        (4.2, "key", b"\r"),                   # 确认类型 → FillFields
        (4.4, "key", b"124"),                  # ID
        (4.8, "key", b"\t"),                   # 切到数据字段
        (5.0, "key", b"A1B2C3"),               # 数据
        (5.4, "key", b"\r"),                   # Enter: 发送 (成功后面板自动关闭)
        (6.2, "snap", "tx_sent"),
        (6.8, "key", b"q"),                    # 退出 (面板已关闭, q 生效)
    ]
    e = PTYHarness(remote_cmd, actions, tmo=14).run()
    try:
        recv_proc.wait(timeout=15)
    except subprocess.TimeoutExpired:
        recv_proc.kill()

    start_t = e.snaps.get("start", "")
    mon_t = e.snaps.get("mon_on", "")
    rx_t = e.snaps.get("rx_frame", "")
    panel_t = e.snaps.get("send_panel", "")
    tx_t = e.snaps.get("tx_sent", "")

    # 平台日志 + peer 捕获
    log_out, log_rc = run_ssh("cat /tmp/can-monitor.log 2>&1")
    rx_out, rx_rc = run_ssh("cat /tmp/task21_rx.txt 2>&1")

    checks = [
        ("TUI 启动 (监控 OFF)", "监控:OFF" in start_t),
        ("监控 ON 状态栏", "监控:ON" in mon_t),
        ("RX 帧 123 显示", has_token(rx_t, "123")),
        ("RX 数据 DE AD BE EF", "DE AD BE EF" in rx_t),
        ("发送面板打开", "原始帧" in panel_t or "发送" in panel_t),
        ("TX 帧 124 显示于 peer 捕获", "124#A1B2C3" in rx_out),
        ("TUI 退出 exit 0", e.exitcode == 0),
        ("平台日志含 123#DEADBEEF", "123#DEADBEEF" in log_out),
        ("日志 candump 格式", bool(re.search(r"\(\d{3}\.\d{6}\) can0 \w+#", log_out))),
    ]

    body = []
    body.append("=== 状态: TUI 启动 (默认监控 OFF) ===")
    body.append(start_t or "(空)")
    body.append("\n=== 状态: 监控 ON ===")
    body.append(mon_t or "(空)")
    body.append("\n=== 状态: 外部发送 123#DEADBEEF 后 ===")
    body.append(rx_t or "(空)")
    body.append("\n=== 状态: 发送面板打开 ===")
    body.append(panel_t or "(空)")
    body.append("\n=== 状态: 面板发送 124#A1B2C3 后 (TUI 自 socket 不收自帧) ===")
    body.append(tx_t or "(空)")
    body.append("\n=== peer recv 捕获 (TUI 下发帧经 loopback 回环) ===")
    body.append(rx_out or "(无)")
    body.append("\n=== 平台日志 /tmp/can-monitor.log ===")
    body.append(log_out or "(无)")
    body.append("\n=== 断言 ===")
    for name, ok in checks:
        body.append(("[PASS] " if ok else "[FAIL] ") + name)
    body.append("\nTUI exitcode: %s" % e.exitcode)
    all_ok = all(ok for _, ok in checks)
    body.append("\nOVERALL: %s" % ("PASS" if all_ok else "FAIL"))
    text = "\n".join(body)

    with open(os.path.join(EVIDENCE, "task-21-platform-test.txt"), "w") as f:
        f.write(text)
    with open(os.path.join(EVIDENCE, "task-21-platform-log.txt"), "w") as f:
        f.write("=== 平台日志 /tmp/can-monitor.log ===\n")
        f.write(log_out + "\n")
        f.write("\n=== peer recv 捕获 ===\n")
        f.write(rx_out + "\n")

    print(text)
    # PNG 渲染 (可选, 用 PIL 画文本快照)
    try:
        from PIL import Image, ImageDraw, ImageFont
        for name, snap in [("start", start_t), ("mon_on", mon_t), ("rx_frame", rx_t),
                           ("send_panel", panel_t), ("tx_sent", tx_t)]:
            if not snap:
                continue
            img = Image.new("RGB", (COLS * 8, 400), (10, 12, 16))
            d = ImageDraw.Draw(img)
            try:
                font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", 14)
            except Exception:
                font = ImageFont.load_default()
            d.text((4, 4), snap, fill=(220, 225, 235), font=font)
            img.save(os.path.join(EVIDENCE, f"task-21-platform-{name}.png"))
    except Exception as e:
        print("PNG render skipped:", e)

    sys.exit(0 if all_ok else 1)


if __name__ == "__main__":
    main()
