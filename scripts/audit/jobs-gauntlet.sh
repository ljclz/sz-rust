#!/usr/bin/env bash
# 可靠任务队列（sz-rust-orm-facade::jobs）gauntlet 入口脚本
# 用法: bash scripts/audit/jobs-gauntlet.sh
# 说明: 需要本机 MySQL（127.0.0.1:3306 root/test123 sz_orm_test）跑集成测试层；
#       变异测试约 7 分钟。fail-closed：任一层非零退出即失败。
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-F:/cargo-target}"
FAIL=0

echo "▶ 1/8 单元测试（facade lib）"
cargo test -p sz-rust-orm-facade --lib -j 2 || FAIL=1

echo "▶ 2/8 单元测试（sz300 全量）"
cargo test -p sz-rust-sz300 -j 2 || FAIL=1

echo "▶ 3/8 集成测试（真实 MySQL，--ignored）"
cargo test -p sz-rust-sz300 --test jobs_integration_test -- --ignored || FAIL=1

echo "▶ 4/8 静态检查（workspace all-targets -D warnings）"
cargo clippy --workspace --all-targets -j 2 -- -D warnings || FAIL=1

echo "▶ 5/8 格式检查"
cargo fmt --all --check || FAIL=1

echo "▶ 6/8 变异测试（cargo-mutants，全 crate）"
cargo mutants -p sz-rust-orm-facade --timeout 120 -j 2 || FAIL=1

echo "▶ 7/8 变更行覆盖率（lib + 集成合并，jobs.rs ≥75% 行覆盖）"
cargo llvm-cov -p sz-rust-orm-facade --lib --no-report -j 2 || FAIL=1
cargo llvm-cov -p sz-rust-sz300 --test jobs_integration_test --no-report -j 2 -- --ignored || FAIL=1
cargo llvm-cov report --fail-under-file-lines 75 || FAIL=1

echo "▶ 8/8 审计门禁"
node scripts/audit/sensitive-field-audit.js || FAIL=1
node scripts/audit/feature-consistency.js || FAIL=1

if [ "$FAIL" -ne 0 ]; then
  echo "❌ gauntlet 存在失败层"
  exit 1
fi
echo "✅ gauntlet 全部通过"
