#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 SZ-Rust Team
#
# 安全 Gauntlet 验证脚本 — 路由绕过 / 畸形输入 / 并发 / panic 恢复 / 容器与配置安全
#
# 用法：
#   bash scripts/audit/security-gauntlet.sh
#
# 行为：
#   1. 运行 `cargo test -p sz-rust-sz300 security_gauntlet -- --nocapture`
#   2. 解析测试结果，输出 PASS/FAIL 摘要
#   3. 全部通过退出码 0，任一失败退出码 1
#
# 清理：
#   测试使用 127.0.0.1 随机端口 + 系统临时目录（sz300-upload-gauntlet-*），
#   测试结束自动清理；无残留进程或文件。

set -u

cd "$(dirname "$0")/../.." || exit 2

# 兼容 Windows Git Bash 下的 cargo 路径
if ! command -v cargo >/dev/null 2>&1; then
  for p in /c/Users/Administrator/.cargo/bin "$HOME/.cargo/bin"; do
    if [ -x "$p/cargo.exe" ] || [ -x "$p/cargo" ]; then
      export PATH="$p:$PATH"
      break
    fi
  done
fi

# 禁用 sccache wrapper（若本机未安装 sccache）
export RUSTC_WRAPPER="${RUSTC_WRAPPER:-}"

echo "==> 运行 sz-rust 安全 Gauntlet 测试（真实 TCP + 原始 HTTP 请求）..."
OUTPUT="$(cargo test -p sz-rust-sz300 security_gauntlet -- --nocapture 2>&1)"
STATUS=$?

# 统计测试结果
TOTAL_PASS=$(printf '%s\n' "$OUTPUT" | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s+0}')
TOTAL_FAIL=$(printf '%s\n' "$OUTPUT" | grep -oE '[0-9]+ failed' | awk '{s+=$1} END {print s+0}')

echo ""
echo "=============================================="
echo " 安全 Gauntlet 结果"
echo "=============================================="
printf '通过: %s 项\n' "${TOTAL_PASS:-0}"
printf '失败: %s 项\n' "${TOTAL_FAIL:-0}"

if [ "$STATUS" -ne 0 ] || [ "${TOTAL_FAIL:-0}" -ne 0 ]; then
  echo "结果: FAIL"
  echo ""
  # 输出失败测试名
  printf '%s\n' "$OUTPUT" | grep -E "FAILED|panicked|assertion" | head -40
  exit 1
fi

echo "结果: PASS"
exit 0