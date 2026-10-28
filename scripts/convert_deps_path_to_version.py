#!/usr/bin/env python3
"""依赖路径转换工具：将 Cargo.toml 中开源核心的 path 依赖改为 version 依赖。

用途：开源/企业版物理分离后，企业版 crate 对开源核心的依赖应从 path 改为 version（从 crates.io 拉取）。
本脚本遍历 Cargo.toml 的 [dependencies] / [dev-dependencies] / [workspace.dependencies] 段，
将匹配 oss_core_crates 名的 path 依赖改为 `version = "<target_version>"`。

接口签名（对应 design.md §2.2.2 接口 4）：
    入参: cargo_toml_path, oss_core_crates (set[str]), target_version (str, default "1.2")
    出参: 变更日志 list[ChangeRecord]，每条含 crate_name, file, line, old, new

用法:
    python scripts/convert_deps_path_to_version.py --cargo-toml packages/sz-rust-sz300/Cargo.toml --dry-run
    python scripts/convert_deps_path_to_version.py --cargo-toml Cargo.toml --target-version 1.2
    python scripts/convert_deps_path_to_version.py --project-root /www/rust/sz-rust-enterprise --apply
"""

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

DEFAULT_OSS_CORE_CRATES = {
    "sz-rust-core",
    "sz-rust-macros",
    "sz-rust-mvc-facade",
    "sz-rust-middleware-facade",
    "sz-rust-router-facade",
    "sz-rust-http-facade",
    "sz-rust-state-facade",
    "sz-rust-cache-facade",
    "sz-rust-orm-facade",
    "sz-rust-orm-ext-facade",
    "sz-rust-auth-facade",
    "sz-rust-pay-facade",
    "sz-rust-infra-facade",
    "sz-rust-ai-facade",
    "sz-rust-observability",
    "sz-rust-capability",
    "sz-rust-addons-loader",
    "sz-rust-tracing",
    "sz-rust-pdf",
    "sz-rust-workflow",
    "sz-rust-rag",
    "sz-rust-wasm",
    "sz-rust-vector-db",
    "sz-rust-frontend-codegen",
    "sz-rust-cli",
    "sz-rust-mcp",
    "sz-rust-config",
    "sz-rust-template",
    "sz-rust-i18n",
    "sz-rust-test-harness",
}

DEPENDENCY_SECTION_RE = re.compile(
    r"^\[(workspace\.dependencies|dependencies|dev-dependencies|build-dependencies)\]$"
)
SIMPLE_DEP_RE = re.compile(r'^([a-zA-Z0-9_-]+)\s*=\s*\{([^}]*)\}')
INLINE_PATH_RE = re.compile(r'\bpath\s*=\s*"[^"]*"')
WORKSPACE_TRUE_RE = re.compile(r'\bworkspace\s*=\s*true')


@dataclass
class ChangeRecord:
    crate_name: str
    file: str
    line: int
    old: str
    new: str

    def __str__(self) -> str:
        return (
            f"  {self.file}:{self.line}  {self.crate_name}\n"
            f"    - {self.old}\n"
            f"    + {self.new}"
        )


def _is_oss_core(name: str, oss_core_crates: set[str]) -> bool:
    return name in oss_core_crates


def _convert_line(
    line: str,
    crate_name: str,
    target_version: str,
) -> str | None:
    """若该行是 path 依赖且 crate_name 属于开源核心，返回转换后的行；否则返回 None。"""
    stripped = line.lstrip()
    indent = line[: len(line) - len(stripped)]

    if not SIMPLE_DEP_RE.match(stripped):
        return None

    if not INLINE_PATH_RE.search(stripped):
        return None

    if "workspace = true" in stripped and "path" not in stripped.split("workspace")[0]:
        return None

    new_inline = f'{crate_name} = "{{ version = "{target_version}" }}'
    if stripped.startswith(f"{crate_name} = {{"):
        return f"{indent}{new_inline}"
    return f"{indent}{crate_name} = \"{target_version}\""


def convert_cargo_toml(
    cargo_toml_path: Path,
    oss_core_crates: set[str],
    target_version: str,
    dry_run: bool = False,
) -> list[ChangeRecord]:
    """转换单个 Cargo.toml 文件中的 path 依赖为 version 依赖。"""
    if not cargo_toml_path.exists():
        raise FileNotFoundError(f"Cargo.toml 不存在: {cargo_toml_path}")

    lines = cargo_toml_path.read_text(encoding="utf-8").splitlines(keepends=True)
    changes: list[ChangeRecord] = []
    in_dep_section = False

    for idx, raw in enumerate(lines):
        line_no = idx + 1
        line = raw.rstrip("\n").rstrip("\r")

        section_match = DEPENDENCY_SECTION_RE.match(line.strip())
        if section_match:
            in_dep_section = True
            continue
        if line.strip().startswith("[") and not section_match:
            in_dep_section = False
            continue
        if not in_dep_section:
            continue

        m = SIMPLE_DEP_RE.match(line.lstrip())
        if not m:
            continue
        crate_name = m.group(1)
        if not _is_oss_core(crate_name, oss_core_crates):
            continue

        new_line = _convert_line(line, crate_name, target_version)
        if new_line is None:
            continue

        changes.append(
            ChangeRecord(
                crate_name=crate_name,
                file=str(cargo_toml_path),
                line=line_no,
                old=line.strip(),
                new=new_line.strip(),
            )
        )
        if not dry_run:
            lines[idx] = new_line + ("\n" if raw.endswith("\n") else "")

    if not dry_run and changes:
        cargo_toml_path.write_text("".join(lines), encoding="utf-8")

    return changes


def find_cargo_tomls(project_root: Path) -> list[Path]:
    """查找项目下所有 Cargo.toml（根 + packages/*/Cargo.toml）。"""
    result = [project_root / "Cargo.toml"]
    packages_dir = project_root / "packages"
    if packages_dir.exists():
        for sub in packages_dir.iterdir():
            if sub.is_dir():
                ct = sub / "Cargo.toml"
                if ct.exists():
                    result.append(ct)
    return [p for p in result if p.exists()]


def main() -> int:
    parser = argparse.ArgumentParser(
        description="将 Cargo.toml 中开源核心的 path 依赖改为 version 依赖"
    )
    parser.add_argument(
        "--cargo-toml",
        type=Path,
        help="单个 Cargo.toml 路径（与 --project-root 二选一）",
    )
    parser.add_argument(
        "--project-root",
        type=Path,
        help="项目根目录（批量转换根 + packages/*/Cargo.toml）",
    )
    parser.add_argument(
        "--target-version",
        default="1.2",
        help="目标 version（默认 1.2，等价 >=1.2.0,<2.0.0）",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="仅输出变更预览，不实际修改文件",
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="实际执行修改（默认 dry-run，需显式 --apply 或 --dry-run 之一）",
    )
    args = parser.parse_args()

    if not args.cargo_toml and not args.project_root:
        parser.error("必须指定 --cargo-toml 或 --project-root")
    if args.cargo_toml and args.project_root:
        parser.error("--cargo-toml 与 --project-root 互斥")

    dry_run = not args.apply or args.dry_run

    if args.cargo_toml:
        cargo_tomls = [args.cargo_toml.resolve()]
    else:
        cargo_tomls = find_cargo_tomls(args.project_root.resolve())

    total_changes = 0
    for ct in cargo_tomls:
        try:
            changes = convert_cargo_toml(
                ct,
                DEFAULT_OSS_CORE_CRATES,
                args.target_version,
                dry_run=dry_run,
            )
        except FileNotFoundError as e:
            print(f"跳过: {e}", file=sys.stderr)
            continue

        if changes:
            print(f"\n{'[DRY-RUN] ' if dry_run else ''}{ct}:")
            for c in changes:
                print(c)
            total_changes += len(changes)

    if total_changes == 0:
        print("无变更：所有开源核心依赖已为 version 声明或无 path 依赖。")
    else:
        mode = "DRY-RUN 预览" if dry_run else "已写入"
        print(f"\n共 {total_changes} 处变更（{mode}）。")

    return 0


if __name__ == "__main__":
    sys.exit(main())