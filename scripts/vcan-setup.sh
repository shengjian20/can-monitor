#!/usr/bin/env bash
# vcan-setup.sh — 创建 vcan0 / vcan1 虚拟 CAN 接口 (双通道测试)
#
# 幂等: 已存在的接口跳过, 打印当前状态。
# 依赖: ip (iproute2), 内核 vcan 模块; 容器内以 root 运行, 无需 sudo。

set -euo pipefail

modprobe vcan 2>/dev/null || true

ensure_vcan() {
    local dev="$1"
    if ip link show "$dev" >/dev/null 2>&1; then
        # 存在但可能未 up
        if ! ip link show "$dev" | grep -q "state UP"; then
            ip link set up "$dev"
            echo "$dev: 已存在 (重新 up)"
        else
            echo "$dev: 已存在并 up (跳过)"
        fi
    else
        ip link add dev "$dev" type vcan
        ip link set up "$dev"
        echo "$dev: 已创建并 up"
    fi
}

ensure_vcan vcan0
ensure_vcan vcan1

echo "--- 当前 vcan 接口 ---"
ip link show | grep -A1 "^[0-9]*: vcan" || true
