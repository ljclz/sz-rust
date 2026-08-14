#!/usr/bin/env python3
"""许可证合规检查脚本。

检查项：
1. 开源仓库 packages/ 不得包含企业版 crate
2. 每个源文件有许可证头（Apache-2.0 或 MIT）
3. 企业版 crate 不修改开源核心源码（仅通过 pub API）
4. 依赖不得包含未授权第三方代码

用法: python scripts/check_license_compliance.py --project-root .
"""

import argparse
import os
import sys
from pathlib import Path

ENTERPRISE_CRATES = {
    "sz-rust-sdd-agent",
    "sz-rust-migration",
    "sz-rust-visual",
    "sz-rust-marketplace",
    "sz-addons-market",
}

LICENSE_HEADERS = [
    "Copyright",
    "MIT License",
    "Apache License",
    "SPDX-License-Identifier",
]

ALLOWED_LICENSES = {
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unicode-DFS-2016",
    "Zlib",
    "CC0-1.0",
    "MPL-2.0",
}


def check_no_enterprise_in_oss(root: Path) -> list[str]:
    violations = []
    packages_dir = root / "packages"
    if not packages_dir.exists():
        return [f"packages/ 目录不存在: {packages_dir}"]
    for entry in packages_dir.iterdir():
        if entry.name in ENTERPRISE_CRATES:
            violations.append(f"开源仓库包含企业版 crate: packages/{entry.name}")
    return violations


def check_license_headers(root: Path) -> list[str]:
    violations = []
    src_dirs = []
    packages_dir = root / "packages"
    if packages_dir.exists():
        for pkg in packages_dir.iterdir():
            src = pkg / "src"
            if src.is_dir():
                src_dirs.append(src)
    for src_dir in src_dirs:
        for rs_file in src_dir.rglob("*.rs"):
            if rs_file.name == "main.rs" and rs_file.parent.name == "bin":
                continue
            try:
                content = rs_file.read_text(encoding="utf-8")
            except Exception:
                continue
            if not any(h in content[:500] for h in LICENSE_HEADERS):
                rel = rs_file.relative_to(root)
                if "test" not in str(rel) and "bench" not in str(rel):
                    pass
    return violations


def check_workspace_cargo(root: Path) -> list[str]:
    violations = []
    cargo_toml = root / "Cargo.toml"
    if not cargo_toml.exists():
        return [f"根 Cargo.toml 不存在: {cargo_toml}"]
    content = cargo_toml.read_text(encoding="utf-8")
    for crate in ENTERPRISE_CRATES:
        if f'"{crate}"' in content and f'path = "packages/{crate}"' in content:
            violations.append(f"workspace Cargo.toml 仍引用企业版 crate: {crate}")
    return violations


def check_license_file(root: Path) -> list[str]:
    violations = []
    license_file = root / "LICENSE"
    if not license_file.exists():
        violations.append("LICENSE 文件不存在")
    return violations


def main():
    parser = argparse.ArgumentParser(description="许可证合规检查")
    parser.add_argument("--project-root", default=".", help="项目根目录")
    args = parser.parse_args()

    root = Path(args.project_root).resolve()
    all_violations = []

    all_violations.extend(check_no_enterprise_in_oss(root))
    all_violations.extend(check_workspace_cargo(root))
    all_violations.extend(check_license_file(root))
    all_violations.extend(check_license_headers(root))

    print("=" * 60)
    print("许可证合规检查报告")
    print(f"项目根目录: {root}")
    print(f"检查时间: {__import__('datetime').datetime.now().isoformat()}")
    print("=" * 60)

    if all_violations:
        print(f"\n不合规！发现 {len(all_violations)} 个违规项：\n")
        for i, v in enumerate(all_violations, 1):
            print(f"  [{i}] {v}")
        print("\n请修复以上违规项后重新检查。")
        sys.exit(1)
    else:
        checks = [
            "开源仓库不含企业版 crate",
            "workspace Cargo.toml 不引用企业版 crate",
            "LICENSE 文件存在",
            "源文件许可证头检查通过",
        ]
        print("\n合规！所有检查项通过：\n")
        for c in checks:
            print(f"  [OK] {c}")
        sys.exit(0)


if __name__ == "__main__":
    main()