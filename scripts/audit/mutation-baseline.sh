#!/usr/bin/env bash
# 变异测试基线（cargo-mutants 27.1.0）
#
# 用法:
#   scripts/audit/mutation-baseline.sh                # 跑全部 4 个基线 crate
#   scripts/audit/mutation-baseline.sh <crate> [...]  # 只跑指定 crate（可多个）
#
# 支持 crate 与 feature 映射:
#   sz-rust-distributed-tx -> dtx-parallel
#   sz-rust-service-registry -> all-features（consul/nacos/kubernetes/gray-release 全覆盖）
#   sz-rust-api-gateway    -> gateway-multidim
#   sz-rust-observability   -> leak-detect
#
# 配置说明:
#   - -j 1 --timeout 180：nacos 重 reqwest 测试在 -j 2 下导致进程崩溃
#     （exit 1073807364 = 0x40010004），详见 docs/audit/mutation-debt.md 崩溃诊断。
set -uo pipefail

# sccache 不在时避免 RUSTC_WRAPPER 残留（CI 门禁）
if ! command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER=""
fi

# Windows 保留名文件 nul 会破坏 cargo-mutants 源码拷贝（os error 87）
if [ -e ./nul ]; then
  echo "WARN: 检测到保留名文件 ./nul，移除后继续"
  rm -f -- ./nul
fi

declare -A FEATURES=(
  [sz-rust-distributed-tx]="dtx-parallel"
  [sz-rust-service-registry]="all-features"
  [sz-rust-api-gateway]="gateway-multidim"
  [sz-rust-observability]="leak-detect"
)
ALL_CRATES=(sz-rust-distributed-tx sz-rust-service-registry sz-rust-api-gateway sz-rust-observability)

CRATES=("$@")
if [ ${#CRATES[@]} -eq 0 ]; then
  CRATES=("${ALL_CRATES[@]}")
fi

echo "=== 变异测试基线启动: $(date +%H:%M:%S) ==="
for crate in "${CRATES[@]}"; do
  features="${FEATURES[$crate]:-}"
  if [ -z "$features" ]; then
    echo "ERROR: 未知 crate '$crate'（支持: ${ALL_CRATES[*]}）"
    exit 2
  fi
  echo "=== [$crate] features=$features ==="
  if [ "$features" = "all-features" ]; then
    cargo mutants -p "$crate" --all-features --timeout 180 -j 1 2>&1
  else
    cargo mutants -p "$crate" --features "$features" --timeout 180 -j 1 2>&1
  fi
  rc=$?
  echo "${crate^^}_RC=$rc"
done
echo "=== 变异测试基线结束: $(date +%H:%M:%S) ==="