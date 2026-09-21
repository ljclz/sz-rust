#!/usr/bin/env python3
"""批量添加源文件许可证头。

为每个 .rs 文件顶部添加 SPDX 许可证标识注释。
- 开源版（apache）：SPDX-License-Identifier: Apache-2.0
- 企业版（commercial）：SPDX-License-Identifier: LicenseRef-SZ-Commercial

跳过已含 SPDX-License-Identifier 的文件。跳过 target/ 目录。

用法:
    python scripts/add_license_header.py --project-root . --license-type apache --dry-run
    python scripts/add_license_header.py --project-root /www/rust/sz-rust-enterprise --license-type commercial --apply
"""

import argparse
import sys
from pathlib import Path

APACHE_HEADER = (
    "// SPDX-License-Identifier: Apache-2.0\n"
    "// Copyright (c) 2024-2026 SZ-Rust Team\n"
    "//\n"
)

COMMERCIAL_HEADER = (
    "// SPDX-License-Identifier: LicenseRef-SZ-Commercial\n"
    "// Copyright (c) 2024-2026 SZ-Rust Team\n"
    "//\n"
)

SPDX_MARKER = "SPDX-License-Identifier"
SKIP_DIRS = {"target", ".git", "node_modules"}


def has_license_header(path: Path) -> bool:
    try:
        with path.open("r", encoding="utf-8", errors="replace") as f:
            for _ in range(5):
                line = f.readline()
                if not line:
                    break
                if SPDX_MARKER in line:
                    return True
        return False
    except (OSError, UnicodeDecodeError):
        return True


def add_header_to_file(path: Path, header: str) -> bool:
    if has_license_header(path):
        return False
    original = path.read_text(encoding="utf-8-sig", errors="replace")
    path.write_text(header + original, encoding="utf-8")
    return True


def find_rs_files(project_root: Path) -> list[Path]:
    result = []
    for p in project_root.rglob("*.rs"):
        if any(skip in p.parts for skip in SKIP_DIRS):
            continue
        result.append(p)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description="批量添加源文件许可证头")
    parser.add_argument("--project-root", type=Path, required=True)
    parser.add_argument(
        "--license-type",
        choices=["apache", "commercial"],
        required=True,
    )
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()

    dry_run = not args.apply or args.dry_run
    header = APACHE_HEADER if args.license_type == "apache" else COMMERCIAL_HEADER

    root = args.project_root.resolve()
    if not root.exists():
        print(f"项目根目录不存在: {root}", file=sys.stderr)
        return 1

    rs_files = find_rs_files(root)
    added = 0
    skipped = 0

    for f in rs_files:
        if has_license_header(f):
            skipped += 1
            continue
        added += 1
        if not dry_run:
            add_header_to_file(f, header)

    mode = "DRY-RUN" if dry_run else "APPLIED"
    print(f"[{mode}] license-type={args.license_type}")
    print(f"  扫描 .rs 文件: {len(rs_files)}")
    print(f"  新增许可证头: {added}")
    print(f"  已含头跳过:   {skipped}")

    return 0


if __name__ == "__main__":
    sys.exit(main())