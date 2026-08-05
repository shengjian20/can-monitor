#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Task 20 e2e harness — CAN monitor TUI 端到端集成测试 (容器内 vcan0).

用法 (容器内, 先 vcan-setup.sh):
    python3 /workspaces/can_monitor/.omo/e2e/harness.py [scenario|all]

场景:
    mixed       场景1: 混合协议显示 (CANopen/J1939/Raw)
    filter      场景2: 过滤开关切换
    log         场景3: 日志文件 (candump -L 格式)
    toggle      场景4: 监控开关联动 (帧计数)
    nmt         场景5: CANopen NMT 下发 (candump 捕获 000#0101)
    invalid     场景6: 非法输入 (错误提示, 不 panic)

证据输出到 /workspaces/can_monitor/.omo/evidence/task-20-*.txt
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

BIN = "/workspaces/can_monitor/target/debug/can-monitor"
VENDOR = "/workspaces/can_monitor/third_party/controlcan/x86_64"
EVIDENCE = "/workspaces/can_monitor/.omo/evidence"
ROWS, COLS = 40, 120


class Screen:
    """迷你 ANSI 终端解码器: 将 pty 原始输出还原为字符网格 (最后一帧可见文本)。"""

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
                    return  # 序列未完整, 等待更多数据
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
        # SGR (m) / 光标显隐 (?25h/l) 等其余序列仅影响样式, 不影响文本网格。

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
            # 宽字符占 2 格, 终端自然前进 2 列; 后随的绝对光标定位因此能对齐。
            if width == 2 and self.cc + 1 < self.cols:
                self.grid[self.cr][self.cc + 1] = "\u200b"  # 占位, 防止残留
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


class E2E:
    """单场景运行器: pty 启 TUI + 定时按键/cansend + 快照断言。"""

    def __init__(self, name, args, actions, tmo=12.0):
        self.name = name
        self.args = args
        self.actions = sorted(actions, key=lambda a: a[0])
        self.tmo = tmo
        self.raw = b""
        self.snaps = {}
        self.exitcode = None

    def run(self):
        os.environ["LD_LIBRARY_PATH"] = VENDOR  # 容器内 rpath 指向宿主机路径, 需手动指定
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
            os.execv(BIN, [BIN] + self.args)
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
                    subprocess.run(
                        payload, shell=True,
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    )
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

        # 排空残余输出。
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
    """文本中是否含独立空白分隔的 token (避免时间戳子串误报)。"""
    return re.search(r"(^|\s)" + re.escape(tok) + r"(\s|$)", text) is not None


def counts(text):
    m = re.search(r"帧:(\d+)", text)
    return int(m.group(1)) if m else None


def snap_text(e2e, snap):
    t = e2e.snaps.get(snap)
    return t if t is not None else "(无快照)"


def scenario_mixed():
    """场景1: 混合协议显示 — CANopen TPDO1 + CANopen SDO + J1939 + Raw 同屏。"""
    e = E2E("mixed", ["--backend", "socketcan", "--iface", "vcan0"], [
        (0.7, "key", b" "),
        (1.2, "shell", "cansend vcan0 181#0102030405060708"),
        (1.6, "shell", "cansend vcan0 18FEF100#12345678"),
        (2.0, "shell", "cansend vcan0 5A4#00"),
        (2.4, "shell", "cansend vcan0 140#00"),
        (3.2, "snap", "final"),
        (3.6, "key", b"q"),
    ], tmo=8).run()
    t = snap_text(e, "final")
    checks = [
        ("exitcode==0", e.exitcode == 0),
        ("监控:ON", "监控:ON" in t),
        ("CANopen 帧 181 显示", has_token(t, "181")),
        ("J1939 扩展帧 18FEF100 显示", "18FEF100" in t),
        ("5A4 显示 (CANopen SDO)", has_token(t, "5A4")),
        ("Raw 帧 140 显示", has_token(t, "140")),
        ("CANopen 协议标记 TPDO1 n1", "TPDO1 n1" in t),
        ("J1939 协议标记 PGN FEF1", "PGN FEF1" in t),
        ("5A4 解析为 SDO n36", "SDO n36" in t),
        ("数据 01 02 03 04 05 06 07 08", "01 02 03 04 05 06 07 08" in t),
        ("数据 12 34 56 78", "12 34 56 78" in t),
        ("计数 帧:4 CANopen:2 J1939:1", counts(t) == 4 and "CANopen:2" in t and "J1939:1" in t),
    ]
    return e, checks, t, [("raw", "task-20-mixed-protocol.txt", t)]


def scenario_filter():
    """场景2: 过滤开关 f 切换不 panic, 状态栏联动。"""
    e = E2E("filter", ["--backend", "socketcan", "--iface", "vcan0"], [
        (0.7, "key", b" "),
        (1.1, "shell", "cansend vcan0 123#AABB"),
        (1.5, "key", b"f"),
        (2.0, "snap", "filter_on"),
        (2.3, "key", b"f"),
        (2.8, "snap", "filter_off"),
        (3.2, "key", b"q"),
    ], tmo=7).run()
    on_t = snap_text(e, "filter_on")
    off_t = snap_text(e, "filter_off")
    checks = [
        ("exitcode==0", e.exitcode == 0),
        ("过滤 ON 状态栏", "过滤:ON" in on_t),
        ("过滤 OFF 状态栏", "过滤:OFF" in off_t),
        ("过滤 ON 时帧仍显示", "AA BB" in on_t),
    ]
    body = "--- 过滤:ON ---\n" + on_t + "\n\n--- 过滤:OFF ---\n" + off_t
    return e, checks, body, [("raw", "task-20-filter.txt", body)]


def scenario_log():
    """场景3: 日志文件 — candump -L 格式, 关日志后不再写入。"""
    # 日志以追加模式打开, 必须先于 TUI 启动清空 (TUI 启动即创建文件)。
    subprocess.run("rm -f /tmp/e2e.log", shell=True)
    e = E2E("log", ["--backend", "socketcan", "--iface", "vcan0", "--log-file", "/tmp/e2e.log"], [
        (0.7, "key", b" "),
        (1.1, "shell", "cansend vcan0 181#0102030405060708"),
        (1.5, "shell", "cansend vcan0 18FEF100#12345678"),
        (1.9, "key", b"l"),
        (2.3, "shell", "cansend vcan0 5A4#00"),
        (2.8, "snap", "log_off"),
        (3.2, "key", b"q"),
    ], tmo=7).run()
    t = snap_text(e, "log_off")
    log = ""
    try:
        with open("/tmp/e2e.log", "r", encoding="utf-8", errors="replace") as f:
            log = f.read()
    except OSError as err:
        log = f"(读取日志失败: {err})"
    fmt_ok = re.search(r"^\(\d+\.\d{6}\) vcan0 181#0102030405060708$", log, re.M) is not None
    checks = [
        ("exitcode==0", e.exitcode == 0),
        ("日志为 candump -L 格式 (181 帧)", fmt_ok),
        ("日志含 J1939 帧 18FEF100#12345678", "18FEF100#12345678" in log),
        ("关日志后 5A4 帧未写入", "5A4#00" not in log),
        ("TUI 状态栏 日志:OFF", "日志:OFF" in t),
    ]
    body = "=== /tmp/e2e.log (candump -L) ===\n" + log
    return e, checks, body, [("log", "task-20-log.txt", body)]


def scenario_toggle():
    """场景4: 监控开关 — OFF 不计数, ON 计数, 再 OFF 冻结。"""
    e = E2E("toggle", ["--backend", "socketcan", "--iface", "vcan0"], [
        (0.6, "snap", "off_a"),
        (1.0, "shell", "cansend vcan0 123#0102"),
        (1.4, "shell", "cansend vcan0 123#0304"),
        (1.8, "snap", "off_b"),
        (2.2, "key", b" "),
        (2.8, "snap", "on_c"),
        (3.2, "shell", "cansend vcan0 123#0506"),
        (3.6, "shell", "cansend vcan0 123#0708"),
        (4.2, "snap", "on_d"),
        (4.6, "key", b" "),
        (5.0, "shell", "cansend vcan0 123#090A"),
        (5.4, "shell", "cansend vcan0 123#0B0C"),
        (6.0, "snap", "off_e"),
        (6.5, "key", b"q"),
    ], tmo=9).run()
    ta = snap_text(e, "off_a")
    tb = snap_text(e, "off_b")
    tc = snap_text(e, "on_c")
    td = snap_text(e, "on_d")
    te = snap_text(e, "off_e")
    checks = [
        ("exitcode==0", e.exitcode == 0),
        ("初始 OFF 且 帧:0", "监控:OFF" in ta and counts(ta) == 0),
        ("OFF 期发送帧不计数 (仍 0)", "监控:OFF" in tb and counts(tb) == 0),
        ("ON 后消费积压帧 帧:2", "监控:ON" in tc and counts(tc) == 2),
        ("ON 期新帧计数 帧:4", "监控:ON" in td and counts(td) == 4),
        ("再 OFF 后计数冻结 帧:4", "监控:OFF" in te and counts(te) == 4),
    ]
    body = (
        "--- OFF 初始 ---\n" + ta + "\n\n--- OFF 发帧后 ---\n" + tb
        + "\n\n--- ON 积压消费 ---\n" + tc + "\n\n--- ON 新帧 ---\n" + td
        + "\n\n--- 再 OFF 冻结 ---\n" + te
    )
    return e, checks, body, [("raw", "task-20-toggle.txt", body)]


def scenario_nmt():
    """场景5: CANopen NMT 下发 — 键盘流程构造 NMT START node1, candump 捕获 000#0101。"""
    logf = "/tmp/candump_nmt.log"
    subprocess.run(f"rm -f {logf}; candump vcan0 -L > {logf} 2>&1 &", shell=True)
    time.sleep(0.4)
    e = E2E("nmt", ["--backend", "socketcan", "--iface", "vcan0"], [
        (0.7, "key", b" "),
        (1.1, "key", b"x"),
        (1.5, "key", b"\r"),
        (1.9, "key", b"1"),
        (2.3, "key", b"\t"),
        (2.7, "key", b"1"),
        (3.1, "key", b"\r"),
        (3.9, "snap", "sent"),
        (4.3, "key", b"q"),
    ], tmo=8).run()
    subprocess.run("pkill -f 'candump vcan0' 2>/dev/null; true", shell=True)
    candump = ""
    try:
        with open(logf, "r", encoding="utf-8", errors="replace") as f:
            candump = f.read()
    except OSError as err:
        candump = f"(读取 candump 失败: {err})"
    t = snap_text(e, "sent")
    checks = [
        ("exitcode==0", e.exitcode == 0),
        ("candump 捕获 NMT 帧 000#0101", re.search(r"vcan0\s+000#0101", candump) is not None),
        ("candump 帧格式为 candump -L", re.search(r"^\(\d+\.\d+\) vcan0 000#0101", candump, re.M) is not None),
        ("发送后面板已关闭", "CANopen 下发" not in t),
        ("发送后 TUI 无 panic 仍在运行", "监控:ON" in t),
    ]
    body = "=== candump vcan0 -L 捕获 ===\n" + candump + "\n=== TUI 快照 (发送后) ===\n" + t
    return e, checks, body, [("nmt", "task-20-nmt-send.txt", body)]


def scenario_invalid():
    """场景6: 非法输入 — NMT 节点 abc, 显示错误不 panic, Esc 关闭。"""
    e = E2E("invalid", ["--backend", "socketcan", "--iface", "vcan0"], [
        (0.7, "key", b"x"),
        (1.1, "key", b"\r"),
        (1.5, "key", b"\t"),
        (1.9, "key", b"a"),
        (2.1, "key", b"b"),
        (2.3, "key", b"c"),
        (2.8, "key", b"\r"),
        (3.6, "snap", "err"),
        (4.1, "key", b"\x1b"),
        (4.5, "snap", "closed"),
        (5.0, "key", b"q"),
    ], tmo=8).run()
    err_t = snap_text(e, "err")
    closed_t = snap_text(e, "closed")
    checks = [
        ("exitcode==0 (不 panic)", e.exitcode == 0),
        ("错误提示 节点ID 'abc' 不是有效数字", "节点ID 'abc' 不是有效数字" in err_t),
        ("错误时面板仍打开", "CANopen 下发" in err_t),
        ("Esc 后面板关闭", "CANopen 下发" not in closed_t),
    ]
    body = "--- 错误提示 ---\n" + err_t + "\n\n--- Esc 关闭后 ---\n" + closed_t
    return e, checks, body, [("invalid", "task-20-invalid-input.txt", body)]


SCENARIOS = {
    "mixed": scenario_mixed,
    "filter": scenario_filter,
    "log": scenario_log,
    "toggle": scenario_toggle,
    "nmt": scenario_nmt,
    "invalid": scenario_invalid,
}


def save_evidence(name, body, fname):
    os.makedirs(EVIDENCE, exist_ok=True)
    path = os.path.join(EVIDENCE, fname)
    with open(path, "w", encoding="utf-8") as f:
        f.write(body)
    return path


def main():
    which = sys.argv[1] if len(sys.argv) > 1 else "all"
    names = list(SCENARIOS) if which == "all" else [which]
    overall = True
    for name in names:
        fn = SCENARIOS.get(name)
        if fn is None:
            print(f"[{name}] 未知场景 (可用: {', '.join(SCENARIOS)})")
            overall = False
            continue
        print(f"\n========== 场景: {name} ==========")
        try:
            e, checks, body, evidence = fn()
        except Exception as exc:
            print(f"运行异常: {exc!r}")
            overall = False
            continue
        passed = all(ok for _, ok in checks)
        overall = overall and passed
        for label, ok in checks:
            print(f"  [{'PASS' if ok else 'FAIL'}] {label}")
        for _, fname, text in evidence:
            path = save_evidence(name, text, fname)
            print(f"  证据: {path}")
        print(f"  ==> 场景 {name}: {'PASS' if passed else 'FAIL'} (exit={e.exitcode})")
        # 附上带行号的末帧快照文本 (调试用)。
        last_snap = e.snaps[list(e.snaps)[-1]] if e.snaps else ""
        print("  --- 末帧快照 ---")
        for i, line in enumerate(last_snap.splitlines()[:30], 1):
            print(f"  {i:3d}| {line}")
    print(f"\n===== 总结果: {'ALL PASS' if overall else 'SOME FAILED'} =====")
    sys.exit(0 if overall else 1)


if __name__ == "__main__":
    main()
