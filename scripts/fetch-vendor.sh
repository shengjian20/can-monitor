#!/usr/bin/env bash
# fetch-vendor.sh — 拷贝 USBCAN(controlcan) 供应商库到 third_party/controlcan/
#
# 来源: SDK "Linux资料包V1.45/二次开发库文件"
#   - ARM平台/64bit/        → third_party/controlcan/aarch64/  (aarch64)
#   - x86平台/64位linux系统/ → third_party/controlcan/x86_64/  (x86_64)
#   - controlcan.h          → third_party/controlcan/controlcan.h
#   - x64(64bit)/ControlCAN.dll → third_party/controlcan/win64/ (Windows x64)
#
# 注意: 树莓派/64bit 是 32 位 ARM 二进制, 不要使用。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

SDK="/media/raw/filespace/test/can_monitor/Linux资料包V1.45/二次开发库文件"
WIN64_SDK="/media/raw/filespace/test/can_monitor/CAN分析仪资料20250624_Linux/CAN分析仪资料20250618_Linux/二次开发库文件/x64(64bit)"
DEST="$PROJECT_ROOT/third_party/controlcan"

ARM_DIR="$SDK/ARM平台/64bit"
X86_DIR="$SDK/x86平台/64位linux系统"

# 校验源存在
for p in "$SDK/controlcan.h" "$ARM_DIR/libcontrolcan.a" "$ARM_DIR/libcontrolcan.so" "$X86_DIR/libcontrolcan.a" "$X86_DIR/libcontrolcan.so" "$WIN64_SDK/ControlCAN.dll"; do
    if [[ ! -f "$p" ]]; then
        echo "ERROR: 源文件不存在: $p" >&2
        exit 1
    fi
done

mkdir -p "$DEST/aarch64" "$DEST/x86_64" "$DEST/win64"

cp "$ARM_DIR/libcontrolcan.a"  "$DEST/aarch64/libcontrolcan.a"
cp "$ARM_DIR/libcontrolcan.so" "$DEST/aarch64/libcontrolcan.so"
cp "$SDK/controlcan.h"         "$DEST/controlcan.h"
cp "$X86_DIR/libcontrolcan.a"  "$DEST/x86_64/libcontrolcan.a"
cp "$X86_DIR/libcontrolcan.so" "$DEST/x86_64/libcontrolcan.so"
cp "$WIN64_SDK/ControlCAN.dll" "$DEST/win64/ControlCAN.dll"

echo "已拷贝到 $DEST:"
echo "  [aarch64] libcontrolcan.a  <- $ARM_DIR"
echo "  [aarch64] libcontrolcan.so <- $ARM_DIR"
echo "  [header]  controlcan.h     <- $SDK"
echo "  [x86_64]  libcontrolcan.a  <- $X86_DIR"
echo "  [x86_64]  libcontrolcan.so <- $X86_DIR"
echo "  [win64]   ControlCAN.dll   <- $WIN64_SDK"
echo "完成"
