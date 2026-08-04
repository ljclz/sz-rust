#!/usr/bin/env bash
# 铁律 2 自动门禁：检测生产代码裸 .unwrap()（排除 mod tests / #[test] / #[tokio::test] / 字符串 / 注释 / tests·benches·examples 目录）
# 退出码：0 = 合规；1 = 发现生产 unwrap（违规）
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
OUT=$(python scripts/check-unwrap.py)
TOTAL=$(echo "$OUT" | grep -oE 'AUTHORITATIVE_PROD_UNWRAP: [0-9]+' | awk '{print $2}')
if [ "$TOTAL" = "0" ]; then
  echo "✅ 铁律 2 合规：生产代码无裸 unwrap"
  exit 0
else
  echo "❌ 铁律 2 违规：生产代码发现 $TOTAL 处裸 unwrap"
  echo "$OUT"
  exit 1
fi
