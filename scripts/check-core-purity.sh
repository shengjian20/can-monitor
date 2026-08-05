#!/usr/bin/env bash
# 检查 can-monitor-core 的依赖纯度:
#   1) 源码中不得出现 UI 库代码引用 (ratatui / crossterm / tui::), 注释行除外
#   2) 直接依赖必须在白名单内: can-types / canopen-stack / j1939-stack / crossbeam-channel
# 返回: 0 = 通过 (打印 OK); 非 0 = 违规 (打印违规内容)
set -euo pipefail

CORE_SRC="crates/can-monitor-core/src"
WHITELIST='can-monitor-core|can-types|canopen-stack|j1939-stack|crossbeam-channel'

# 1) 代码级 UI 依赖检查: 排除 // 注释行 (//  ///  //!) 后搜索 ratatui/crossterm/tui::
code_hits=$(grep -rn --include='*.rs' -E 'ratatui|crossterm|tui::' "$CORE_SRC" \
    | grep -vE '^\s*[^:]*:[0-9]+:\s*(//|///|//!)' || true)
if [ -n "$code_hits" ]; then
    echo "NON-COMMENT UI REFERENCE IN CORE:"
    echo "$code_hits"
    exit 1
fi

# 2) 直接依赖白名单: cargo tree --depth 1 只允许白名单条目
tree=$(cargo tree -p can-monitor-core --depth 1 -e normal 2>/dev/null)
if [ -z "$tree" ]; then
    echo "ERROR: cargo tree -p can-monitor-core returned nothing (workspace broken?)"
    exit 1
fi
bad=$(echo "$tree" | grep -E '^[│└├ ]*[a-zA-Z-]+ v' | grep -vE "$WHITELIST" || true)
if [ -n "$bad" ]; then
    echo "NON-WHITELIST DIRECT DEP:"
    echo "$bad"
    exit 1
fi

echo "core purity OK: 0 UI code refs, whitelist deps only"
