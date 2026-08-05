#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Task 21 peer: 平台 can0 帧收发辅助 (无 can-utils 的替代).

用法:
    python3 task21_peer.py send <id_hex> <data_hex>
    python3 task21_peer.py recv <seconds> <outfile>
"""
import socket
import struct
import sys
import time


def can_frame(canid: int, data: bytes) -> bytes:
    return struct.pack("=IB3x8s", canid & 0x1FFFFFFF, len(data), data.ljust(8, b"\x00"))


def do_send(id_hex: str, data_hex: str):
    s = socket.socket(socket.AF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
    s.bind(("can0",))
    canid = int(id_hex, 16)
    data = bytes.fromhex(data_hex)
    fr = can_frame(canid, data)
    n = s.send(fr)
    print("sent %s#%s (%d bytes)" % (id_hex.upper(), data_hex.upper(), n), flush=True)
    s.close()


def do_recv(seconds: float, outfile: str):
    s = socket.socket(socket.AF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
    s.bind(("can0",))
    s.settimeout(0.2)
    start = time.time()
    lines = []
    with open(outfile, "w") as f:
        while time.time() - start < seconds:
            try:
                d = s.recv(16)
            except socket.timeout:
                continue
            canid, dlc, data = struct.unpack("=IB3x8s", d)
            if canid & 0x20000000:
                continue  # 错误帧
            eff = bool(canid & 0x80000000)  # EFF 标志
            raw_id = canid & 0x1FFFFFFF
            idstr = "%08X" % raw_id if eff else "%03X" % raw_id
            line = "%s#%s" % (idstr, data[:dlc].hex().upper())
            lines.append(line)
            f.write(line + "\n")
            f.flush()
            print("rx " + line, flush=True)
    s.close()


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "send" and len(sys.argv) == 4:
        do_send(sys.argv[2], sys.argv[3])
    elif cmd == "recv" and len(sys.argv) == 4:
        do_recv(float(sys.argv[2]), sys.argv[3])
    else:
        print("usage: task21_peer.py send <id_hex> <data_hex> | recv <sec> <outfile>")
        sys.exit(2)
