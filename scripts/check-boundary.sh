#!/usr/bin/env bash
# 框架/业务边界依赖检查（v1.4.0 T07.2）
#
# 用法：
#   scripts/check-boundary.sh
#
# 基于 cargo metadata 依赖图分析，检测框架 crate 是否反向依赖 sz-rust-sz300。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

PYTHON=""
for cand in python3 python; do
  if command -v "$cand" >/dev/null 2>&1 && "$cand" -c 'import json' >/dev/null 2>&1; then
    PYTHON="$cand"
    break
  fi
done
if [ -z "$PYTHON" ]; then
  echo "❌ 需要 Python 3" >&2
  exit 1
fi

echo "▶ 框架/业务边界依赖检查…"
echo "  规则：框架层 crate 不得依赖 sz-rust-sz300"

cd "$ROOT_DIR"
TMP_META="$(mktemp)"
RUSTC_WRAPPER="" cargo metadata --format-version 1 > "$TMP_META" 2>/dev/null
"$PYTHON" - "$TMP_META" <<'PY'
import json, sys

with open(sys.argv[1], 'r', encoding='utf-8') as f:
    data = json.load(f)
packages = {p["name"]: p for p in data["packages"]}
workspace_members = set(p["name"] for p in data["packages"])

BUSINESS_CRATE = "sz-rust-sz300"
FRAMEWORK_PREFIX = "sz-rust-"
TOOL_CRATES = {"sz-rust-testkit", "sz-rust-cli", "sz-rust-examples", "sz-rust-facade-tests", "sz-rust"}

violations = []
framework_crates = []
business_crates = []
tool_crates = []

for name in workspace_members:
    if name == BUSINESS_CRATE:
        business_crates.append(name)
    elif name in TOOL_CRATES:
        tool_crates.append(name)
    elif name.startswith(FRAMEWORK_PREFIX):
        framework_crates.append(name)

for pkg_name, pkg in packages.items():
    if pkg_name not in workspace_members:
        continue
    if pkg_name in TOOL_CRATES or pkg_name == BUSINESS_CRATE:
        continue

    for dep in pkg.get("dependencies", []):
        if dep["name"] == BUSINESS_CRATE:
            violations.append((pkg_name, dep["name"]))

print(f"  框架层 crate 数：{len(framework_crates)}")
print(f"  业务层 crate 数：{len(business_crates)}")
print(f"  工具层 crate 数：{len(tool_crates)}")
print()

if violations:
    print("❌ 发现反向依赖违规：")
    for fw, biz in violations:
        print(f"    框架 crate '{fw}' → 业务 crate '{biz}'")
    print()
    print("  修复建议：将共享能力提取至框架层，业务层通过 facade 引用")
    sys.exit(1)
else:
    print("✅ 无反向依赖：所有框架 crate 均未引用 sz-rust-sz300")
    sys.exit(0)
PY
rm -f "$TMP_META"