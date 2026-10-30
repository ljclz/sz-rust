#!/bin/bash
set -euo pipefail

echo "=== 服务器配置评估 ==="
echo ""

echo "--- CPU 信息 ---"
lscpu | grep -E "Model name|CPU\(s\)|Thread|Core|Socket" || true
echo ""

echo "--- 内存信息 ---"
free -h
echo ""

echo "--- OS 信息 ---"
cat /etc/os-release | head -5
echo ""

echo "--- 内核版本 ---"
uname -r
echo ""

echo "--- 磁盘空间 ---"
df -h / /www 2>/dev/null || df -h /
echo ""

echo "--- Rust 版本 ---"
source ~/.cargo/env 2>/dev/null
rustc --version 2>/dev/null || echo "Rust 未安装"
echo ""

echo "--- wrk 版本 ---"
wrk --version 2>&1 | head -1 || echo "wrk 未安装"
echo ""

echo "--- k6 版本 ---"
k6 version 2>&1 || echo "k6 未安装"
echo ""

echo "--- 空闲资源基线 ---"
echo "CPU 使用率: $(top -bn1 | grep "Cpu(s)" | awk '{print $2 + $4}')%"
echo "内存使用: $(free | grep Mem | awk '{printf "%.1f%%", $3/$2 * 100}')"
echo ""

echo "=== 配置评估结果 ==="
CPU_CORES=$(nproc)
MEM_MB=$(free -m | grep Mem | awk '{print $2}')

echo "CPU 核数: $CPU_CORES"
echo "内存: ${MEM_MB}MB"

if [ "$CPU_CORES" -ge 2 ] && [ "$MEM_MB" -ge 2048 ]; then
    echo "✅ 服务器配置满足最低要求（2 核 / 2GB）"
    exit 0
else
    echo "⚠️ 服务器配置不足，最低要求：2 核 CPU / 2GB 内存"
    echo "当前：${CPU_CORES} 核 / ${MEM_MB}MB"
    exit 140
fi