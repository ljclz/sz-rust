#!/usr/bin/env bash
#
# SZ-Rust 火焰图生成脚本
#
# 调用 perf record + flamegraph 生成 SVG 火焰图，用于 P3 性能优化瓶颈定位。
#
# 用法:
#   ./scripts/flamegraph.sh <binary> [duration_seconds] [output_svg]
#
# 参数:
#   binary          目标二进制路径（如 target/release/sz-pay-server）
#   duration        采样时长（秒，默认 10）
#   output_svg      输出 SVG 路径（默认 flamegraph_<binary>_<timestamp>.svg）
#
# 依赖:
#   - perf（Linux perf 工具）
#   - flamegraph（cargo install flamegraph 或 perf-flamegraph 包）
#
# Windows 回退:
#   Windows 不支持 perf，脚本会提示使用 cargo-flamegraph 或在 Linux 环境生成。
#
# 示例:
#   # Linux 环境生成 30 秒火焰图
#   ./scripts/flamegraph.sh target/release/sz-pay-server 30
#
#   # 指定输出路径
#   ./scripts/flamegraph.sh target/release/sz-pay-server 10 ./my_flamegraph.svg

set -euo pipefail

# ============================================================================
# 平台检测
# ============================================================================

OS="$(uname -s 2>/dev/null || echo Windows)"

if [[ "$OS" == "Windows"* || "$OS" == "MINGW"* || "$OS" == "MSYS"* ]]; then
    echo "============================================================"
    echo "SZ-Rust Flamegraph — Windows 环境不支持 perf"
    echo "============================================================"
    echo ""
    echo "Windows 平台无法直接使用 perf record 生成火焰图。"
    echo ""
    echo "可选方案:"
    echo "  1. 在 WSL2 (Linux 子系统) 中运行此脚本"
    echo "  2. 使用 cargo-flamegraph (需安装 flamegraph crate):"
    echo "     cargo install flamegraph"
    echo "     cargo flamegraph --bin <binary> --output <svg>"
    echo "  3. 在 Linux 服务器 (122.51.216.76) 上生成火焰图"
    echo ""
    echo "推荐方案: 在 WSL2 或 Linux 服务器上运行此脚本。"
    echo "============================================================"
    exit 1
fi

# ============================================================================
# 参数解析
# ============================================================================

if [[ $# -lt 1 ]]; then
    echo "用法: $0 <binary> [duration_seconds] [output_svg]"
    echo ""
    echo "参数:"
    echo "  binary          目标二进制路径"
    echo "  duration        采样时长（秒，默认 10）"
    echo "  output_svg      输出 SVG 路径（默认 flamegraph_<binary>_<timestamp>.svg）"
    echo ""
    echo "示例:"
    echo "  $0 target/release/sz-pay-server 30"
    exit 1
fi

BINARY="$1"
DURATION="${2:-10}"
TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
BINARY_NAME="$(basename "$BINARY")"
OUTPUT_SVG="${3:-flamegraph_${BINARY_NAME}_${TIMESTAMP}.svg}"

# ============================================================================
# 依赖检查
# ============================================================================

echo "[flamegraph] 检查依赖..."

if ! command -v perf &>/dev/null; then
    echo "[flamegraph] 错误: perf 未安装"
    echo "  Debian/Ubuntu: sudo apt install linux-perf"
    echo "  CentOS/RHEL:   sudo yum install perf"
    exit 1
fi

# 检查 flamegraph 脚本（perf-flamegraph 或 cargo install flamegraph）
FLAMEGRAPH_SCRIPT=""
if command -v flamegraph &>/dev/null; then
    FLAMEGRAPH_SCRIPT="flamegraph"
elif [[ -x "/usr/local/bin/flamegraph.pl" ]]; then
    FLAMEGRAPH_SCRIPT="/usr/local/bin/flamegraph.pl"
elif [[ -x "$(dirname "$(command -v perf)")/flamegraph.pl" ]]; then
    FLAMEGRAPH_SCRIPT="$(dirname "$(command -v perf)")/flamegraph.pl"
fi

if [[ -z "$FLAMEGRAPH_SCRIPT" ]]; then
    echo "[flamegraph] 错误: flamegraph 脚本未找到"
    echo "  安装方式:"
    echo "    1. cargo install flamegraph"
    echo "    2. 从 https://github.com/brendangregg/FlameGraph 下载 flamegraph.pl"
    exit 1
fi

# ============================================================================
# 检查目标二进制
# ============================================================================

if [[ ! -x "$BINARY" ]]; then
    echo "[flamegraph] 错误: 二进制不存在或不可执行: $BINARY"
    echo "  请先运行 cargo build --release 编译目标二进制"
    exit 1
fi

# ============================================================================
# 生成火焰图
# ============================================================================

PERF_DATA="perf_${BINARY_NAME}_${TIMESTAMP}.data"
PERF_FOLDED="perf_${BINARY_NAME}_${TIMESTAMP}.folded"

echo "[flamegraph] 目标二进制: $BINARY"
echo "[flamegraph] 采样时长:   ${DURATION}s"
echo "[flamegraph] 输出 SVG:   $OUTPUT_SVG"
echo "[flamegraph] perf 数据:  $PERF_DATA"
echo ""

# 1. perf record 采样
echo "[flamegraph] [1/3] perf record 采样中..."
perf record -F 99 -p "$(pgrep -f "$BINARY_NAME" | head -1 || echo 0)" -g -- sleep "$DURATION" -o "$PERF_DATA" 2>/dev/null || {
    echo "[flamegraph] perf record 失败，尝试直接运行二进制..."
    perf record -F 99 -g -- "$BINARY" &
    PERF_PID=$!
    sleep "$DURATION"
    kill -INT $PERF_PID 2>/dev/null || true
    wait $PERF_PID 2>/dev/null || true
}

# 2. 折叠调用栈
echo "[flamegraph] [2/3] 折叠调用栈..."
perf script -i "$PERF_DATA" 2>/dev/null | stackcollapse-perf.pl > "$PERF_FOLDED" 2>/dev/null || \
    perf script -i "$PERF_DATA" 2>/dev/null | "$FLAMEGRAPH_SCRIPT" --fold > "$PERF_FOLDED" 2>/dev/null || {
    echo "[flamegraph] 折叠调用栈失败"
    exit 1
}

# 3. 生成火焰图 SVG
echo "[flamegraph] [3/3] 生成火焰图 SVG..."
"$FLAMEGRAPH_SCRIPT" "$PERF_FOLDED" > "$OUTPUT_SVG" 2>/dev/null || {
    # flamegraph crate 模式: flamegraph --fold < input > output
    "$FLAMEGRAPH_SCRIPT" < "$PERF_FOLDED" > "$OUTPUT_SVG" 2>/dev/null || {
        echo "[flamegraph] 生成 SVG 失败"
        exit 1
    }
}

# ============================================================================
# 清理临时文件
# ============================================================================

rm -f "$PERF_DATA" "$PERF_FOLDED"

# ============================================================================
# 输出结果
# ============================================================================

echo ""
echo "[flamegraph] 火焰图生成完成!"
echo "  SVG 路径: $OUTPUT_SVG"
echo "  文件大小: $(du -h "$OUTPUT_SVG" | cut -f1)"
echo ""
echo "  在浏览器中打开 SVG 查看火焰图:"
echo "    file://$(pwd)/$OUTPUT_SVG"
echo ""
echo "  热路径分析建议:"
echo "    1. 关注最宽的火焰柱（CPU 占比最高的调用栈）"
echo "    2. 搜索 sz_rust_core / sz_rust_router_facade 等关键模块"
echo "    3. 标注 parse_path / capitalize_first / JSON 序列化等热点函数"