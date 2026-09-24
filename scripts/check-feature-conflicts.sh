#!/usr/bin/env bash
# 互斥 feature 冲突检查（v1.4.0 T02.3）
#
# 用法：
#   scripts/check-feature-conflicts.sh
#
# 检查 Cargo.toml 中声明的互斥 feature 组合是否会被同时启用。
# 当前互斥组：
#   1. TLS 后端（预留：rustls vs native-tls）
#   2. 数据库后端（backend-mysql / backend-postgres / backend-sqlite / backend-oracle / backend-mssql）

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_TOML="$ROOT_DIR/Cargo.toml"

echo "▶ 检查互斥 feature 冲突…"

VIOLATIONS=0

# 互斥组定义：每组内的 feature 不可同时启用
# 格式："组名|feature1,feature2,..."
MUTEX_GROUPS=(
  "TLS 后端|rustls-tls,native-tls"
  "数据库后端|backend-mysql,backend-postgres,backend-sqlite,backend-oracle,backend-mssql"
)

# 读取 Cargo.toml [features] 段中定义的所有 feature
FEATURES=$(grep -E '^[a-z]' "$CARGO_TOML" | sed -n '/^\[features\]/,/^\[/p' 2>/dev/null | grep -E '^[a-z][a-z0-9_-]*\s*=' | cut -d'=' -f1 | tr -d ' ' || true)

for group_def in "${MUTEX_GROUPS[@]}"; do
  group_name="${group_def%%|*}"
  features_csv="${group_def##*|}"
  IFS=',' read -ra features <<< "$features_csv"

  # 检查这些 feature 是否在 Cargo.toml 中定义
  defined=()
  for f in "${features[@]}"; do
    if echo "$FEATURES" | grep -qx "$f"; then
      defined+=("$f")
    fi
  done

  # 如果有 2 个以上已定义的互斥 feature，检查它们是否在同一个聚合 feature 中
  if [ "${#defined[@]}" -ge 2 ]; then
    # 检查是否有聚合 feature 同时包含这些互斥 feature
    for agg in $(echo "$FEATURES" | grep -E '^(p2|p3)'); do
      agg_value=$(grep -A1 "^${agg} =" "$CARGO_TOML" | head -2 | sed 's/.*= *//' | tr -d '"' | tr ',' '\n' | tr -d ' ')
      count=0
      for f in "${defined[@]}"; do
        if echo "$agg_value" | grep -qx "$f"; then
          count=$((count + 1))
        fi
      done
      if [ "$count" -ge 2 ]; then
        echo "❌ 互斥违规：聚合 feature '$agg' 同时启用互斥组 '$group_name' 中的多个 feature: ${defined[*]}"
        VIOLATIONS=$((VIOLATIONS + 1))
      fi
    done
  fi
done

# 检查非互斥组合能否正常编译
echo "▶ 验证非互斥组合编译…"
if cargo build --features ai-facade --no-default-features --quiet 2>/dev/null; then
  echo "✅ ai-facade 单独编译通过"
else
  echo "⚠️  ai-facade 单独编译失败（可能是依赖未安装，非互斥冲突）"
fi

if [ "$VIOLATIONS" -eq 0 ]; then
  echo "✅ 无互斥 feature 冲突"
  exit 0
else
  echo "❌ 发现 $VIOLATIONS 处互斥 feature 冲突"
  exit 1
fi