#!/usr/bin/env python3
"""许可证合规检查脚本。

检查项（按 mode 选择）：
- header: 检查每个 .rs 文件顶部含正确的许可证声明注释
- field:  检查 Cargo.toml license 字段与 expected_license 一致
- all:    同时执行 header 与 field 检查

用法:
    python scripts/check_license_compliance.py --project-root . --mode all --expected-license Apache-2.0
    python scripts/check_license_compliance.py --project-root /www/rust/sz-rust-enterprise --mode all --expected-license LicenseRef-SZ-Commercial
"""

import argparse
import re
import sys
from pathlib import Path

ENTERPRISE_CRATES = {
    "sz-rust-sz300",
    "sz-rust-addons-crm",
    "sz-rust-addons-ecommerce",
    "sz-rust-addons-cms",
    "sz-rust-addons-operate",
    "sz-rust-addons-erp",
    "sz-rust-addons-forum",
    "sz-rust-addons-im",
}

SPDX_MARKER = "SPDX-License-Identifier"
SKIP_DIRS = {"target", ".git", "node_modules"}

LICENSE_FIELD_RE = re.compile(r'^license\s*=\s*"([^"]*)"')
LICENSE_WORKSPACE_RE = re.compile(r"^license\.workspace\s*=\s*true")


def find_rs_files(root: Path) -> list[Path]:
    result = []
    packages_dir = root / "packages"
    if not packages_dir.exists():
        return result
    for p in packages_dir.rglob("*.rs"):
        if any(skip in p.parts for skip in SKIP_DIRS):
            continue
        result.append(p)
    return result


def find_cargo_tomls(root: Path) -> list[Path]:
    result = [root / "Cargo.toml"]
    packages_dir = root / "packages"
    if packages_dir.exists():
        for sub in packages_dir.iterdir():
            if sub.is_dir():
                ct = sub / "Cargo.toml"
                if ct.exists():
                    result.append(ct)
    return [p for p in result if p.exists()]


def check_header(root: Path, expected_license: str) -> list[tuple[str, int, str]]:
    violations = []
    expected_spdx = f"SPDX-License-Identifier: {expected_license}"
    for rs_file in find_rs_files(root):
        try:
            with rs_file.open("r", encoding="utf-8", errors="replace") as f:
                header_lines = [f.readline() for _ in range(5)]
        except OSError:
            continue
        found_spdx = False
        found_correct = False
        for line in header_lines:
            if not line:
                break
            if SPDX_MARKER in line:
                found_spdx = True
                if expected_spdx in line:
                    found_correct = True
                break
        if not found_spdx:
            violations.append((str(rs_file), 1, "missing license header"))
        elif not found_correct:
            violations.append((str(rs_file), 1, f"license header mismatch: expected {expected_license}"))
    return violations


def check_field(root: Path, expected_license: str) -> list[tuple[str, int, str]]:
    violations = []
    root_cargo = root / "Cargo.toml"
    if root_cargo.exists():
        for idx, line in enumerate(root_cargo.read_text(encoding="utf-8").splitlines(), 1):
            m = LICENSE_FIELD_RE.match(line.strip())
            if m:
                wl = m.group(1)
                if wl != expected_license:
                    violations.append((str(root_cargo), idx, f'license = "{wl}", expected "{expected_license}"'))
                break
    for ct in find_cargo_tomls(root):
        if ct == root_cargo:
            continue
        has_workspace = False
        has_direct = False
        direct_license = None
        direct_line = 0
        for idx, line in enumerate(ct.read_text(encoding="utf-8").splitlines(), 1):
            if LICENSE_WORKSPACE_RE.match(line.strip()):
                has_workspace = True
                break
            m = LICENSE_FIELD_RE.match(line.strip())
            if m:
                has_direct = True
                direct_license = m.group(1)
                direct_line = idx
                break
        if has_workspace:
            continue
        if has_direct:
            if direct_license != expected_license:
                violations.append((str(ct), direct_line, f'license = "{direct_license}", expected "{expected_license}"'))
        else:
            violations.append((str(ct), 0, "missing license field"))
    return violations


def main() -> int:
    parser = argparse.ArgumentParser(description="许可证合规检查")
    parser.add_argument("--project-root", default=".", help="项目根目录")
    parser.add_argument("--mode", choices=["header", "field", "all"], default="all")
    parser.add_argument("--expected-license", default="Apache-2.0")
    args = parser.parse_args()

    root = Path(args.project_root).resolve()
    if not root.exists():
        print(f"项目根目录不存在: {root}", file=sys.stderr)
        return 2

    all_violations = []
    if args.mode in ("header", "all"):
        all_violations.extend(check_header(root, args.expected_license))
    if args.mode in ("field", "all"):
        all_violations.extend(check_field(root, args.expected_license))

    if all_violations:
        print(f"不合规！发现 {len(all_violations)} 个违规项：")
        for i, (file, line, desc) in enumerate(all_violations, 1):
            print(f"  [{i}] {file}:{line}  {desc}")
        return 1

    print(f"合规！{args.mode} 检查通过（期望许可证: {args.expected_license}）。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
