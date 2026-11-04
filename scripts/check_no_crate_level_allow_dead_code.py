#!/usr/bin/env python3
"""检查所有 crate 的 lib.rs/main.rs 顶部不得包含 #![allow(dead_code)]。

接口签名（对应 design.md §2.2.2 接口 3）：
    入参: project_root
    出参: 违规项列表，含 file:line 证据

用法:
    python scripts/check_no_crate_level_allow_dead_code.py --project-root .
    python scripts/check_no_crate_level_allow_dead_code.py --project-root /www/rust/sz-rust-enterprise
"""

import argparse
import re
import sys
from pathlib import Path

CRATE_LEVEL_ALLOW_RE = re.compile(r"^#!\s*\[\s*allow\s*\(\s*dead_code\s*\)\s*\]")
SKIP_DIRS = {"target", ".git", "node_modules"}


def find_crate_roots(project_root: Path) -> list[Path]:
    """查找所有 crate 的 src/lib.rs 和 src/main.rs。"""
    result = []
    packages_dir = project_root / "packages"
    if not packages_dir.exists():
        return result
    for sub in packages_dir.iterdir():
        if not sub.is_dir():
            continue
        if any(skip in sub.parts for skip in SKIP_DIRS):
            continue
        src_dir = sub / "src"
        if not src_dir.is_dir():
            continue
        for name in ("lib.rs", "main.rs"):
            f = src_dir / name
            if f.exists():
                result.append(f)
    return result


def check_no_crate_level_allow_dead_code(root: Path) -> list[tuple[str, int]]:
    """检查 crate 根文件顶部不得含 #![allow(dead_code)]。"""
    violations = []
    for crate_file in find_crate_roots(root):
        try:
            lines = crate_file.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        for idx, line in enumerate(lines[:30], 1):
            if CRATE_LEVEL_ALLOW_RE.match(line.strip()):
                violations.append((str(crate_file), idx))
                break
    return violations


def main() -> int:
    parser = argparse.ArgumentParser(
        description="检查 crate 级 #![allow(dead_code)] 违规"
    )
    parser.add_argument("--project-root", type=Path, required=True)
    args = parser.parse_args()

    root = args.project_root.resolve()
    if not root.exists():
        print(f"项目根目录不存在: {root}", file=sys.stderr)
        return 2

    violations = check_no_crate_level_allow_dead_code(root)

    if violations:
        print(f"发现 {len(violations)} 处 crate 级 #![allow(dead_code)] 违规:")
        for file, line in violations:
            print(f"  {file}:{line}  #![allow(dead_code)] found")
        return 1

    print("no crate-level allow(dead_code)")
    return 0


if __name__ == "__main__":
    sys.exit(main())