#!/usr/bin/env bash
# 交叉编译脚本: 目标 aarch64-unknown-linux-gnu.2.23 (glibc 2.23)
#
# 依赖: cargo-zigbuild (cargo install cargo-zigbuild), zig
# 测试平台: jz@172.22.2.242 (aarch64, Ubuntu 16.04, glibc 2.23)
#
# 用法:
#   scripts/build-cross.sh             # 构建整个 workspace / 默认 crate
#   scripts/build-cross.sh -p <crate>  # 只构建指定 crate (透传 cargo 参数)
set -euo pipefail

TARGET="aarch64-unknown-linux-gnu.2.23"
OUT_BIN="target/aarch64-unknown-linux-gnu/release/can-monitor"

# 确保交叉编译目标已安装 (幂等)
rustup target add aarch64-unknown-linux-gnu >/dev/null

# zigbuild 构建 (透传剩余参数, 例如 -p <crate>)
cargo zigbuild --release --target "$TARGET" "$@"

if [ ! -f "$OUT_BIN" ]; then
    echo "错误: 产物不存在: $OUT_BIN" >&2
    exit 1
fi

# 验证 1: file 输出必须包含 aarch64
file_out="$(file "$OUT_BIN")"
echo "$file_out"
if ! grep -q "aarch64" <<<"$file_out"; then
    echo "错误: 产物不是 aarch64 架构" >&2
    exit 1
fi

# 验证 2: 最大 GLIBC_ 版本必须 <= 2.23 (glibc 2.23 兼容硬约束)
if command -v readelf >/dev/null 2>&1; then
    max_ver="$(readelf -V "$OUT_BIN" | grep -o 'GLIBC_[0-9.]*' | grep -o '[0-9.]*$' | sort -u -V | tail -1)"
    if [ -z "$max_ver" ]; then
        # 没有动态 glibc 依赖(纯静态), 视为满足要求
        echo "成功: 产物无 GLIBC 动态依赖 (纯静态链接)"
    elif printf '%s\n2.23\n' "$max_ver" | sort -V | tail -1 | grep -q '^2\.23$'; then
        echo "成功: 最大 GLIBC 版本 $max_ver <= 2.23"
    else
        echo "错误: 最大 GLIBC 版本 $max_ver > 2.23, 目标平台 (Ubuntu 16.04) 不兼容" >&2
        exit 1
    fi
else
    echo "警告: 未找到 readelf, 跳过 GLIBC 版本验证" >&2
fi

echo "交叉编译成功: $OUT_BIN"
