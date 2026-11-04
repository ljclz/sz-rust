#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 SZ-Rust Team
"""依赖可复现性门禁（P0-1）。

背景：sz-orm 家族通过 `../sz-orm` 路径依赖引入，历史上出现"Cargo.toml 声明
10.12.0、Cargo.lock 实际解析 10.13.0"的漂移，且干净检出无法构建。

本脚本检查：
1. 所有 `path = "../sz-orm/..."` 依赖目标存在；
2. 每个 path 依赖都能在 Cargo.lock 中解析到版本，且满足 Cargo.toml 的版本要求；
3. 兄弟仓库 HEAD 等于 `.sz-orm-commit` 中固定的提交（本地开发若为 WIP 会失败，
   这是有意为之：需要显式更新 pin 文件并提交）；
4. Cargo.lock 中 sz-orm 家族版本一致。

退出码：0 通过；1 失败；2 环境缺失（兄弟仓库不存在）。
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
CARGO_TOML = REPO_ROOT / "Cargo.toml"
CARGO_LOCK = REPO_ROOT / "Cargo.lock"
PIN_FILE = REPO_ROOT / ".sz-orm-commit"
SZ_ORM_DIR = REPO_ROOT.parent / "sz-orm"

PATH_DEP_RE = re.compile(
    r'^(?P<name>[A-Za-z0-9_-]+)\s*=\s*\{(?P<attrs>[^}]*path\s*=\s*"(?P<path>[^"]+)"[^}]*)\}',
    re.MULTILINE,
)
VERSION_RE = re.compile(r'version\s*=\s*"([^"]+)"')
LOCK_BLOCK_RE = re.compile(r"\[\[package\]\]\s*(.*?)(?=\n\[\[package\]\]|\Z)", re.DOTALL)
LOCK_NAME_RE = re.compile(r'^name\s*=\s*"([^"]+)"', re.MULTILINE)
LOCK_VERSION_RE = re.compile(r'^version\s*=\s*"([^"]+)"', re.MULTILINE)


def parse_lock() -> dict[str, str]:
    text = CARGO_LOCK.read_text(encoding="utf-8")
    versions: dict[str, str] = {}
    for block in LOCK_BLOCK_RE.findall(text):
        name = LOCK_NAME_RE.search(block)
        version = LOCK_VERSION_RE.search(block)
        if name and version:
            versions[name.group(1)] = version.group(1)
    return versions


def parse_semver(value: str) -> tuple[int, int, int] | None:
    m = re.match(r"^(\d+)\.(\d+)\.(\d+)$", value.strip())
    if not m:
        return None
    return int(m.group(1)), int(m.group(2)), int(m.group(3))


def satisfies(required: str, resolved: str) -> bool:
    """简化 semver 校验：支持 =x.y.z 与默认 caret（同 major，且 >= 声明值）。"""
    req = required.strip()
    exact = req.startswith("=")
    req = req.lstrip("=^~ ")
    r = parse_semver(req)
    v = parse_semver(resolved)
    if r is None or v is None:
        return True  # 非精确版本表达式不阻断
    if exact:
        return r == v
    if r[0] > 0:
        return v[0] == r[0] and v >= r
    return v == r


def git_head(path: Path) -> str | None:
    try:
        out = subprocess.run(
            ["git", "-C", str(path), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if out.returncode != 0:
        return None
    return out.stdout.strip()


def member_cargo_files() -> list[Path]:
    return sorted((REPO_ROOT / "packages").glob("**/Cargo.toml"))


def is_used_by_member(name: str, files: list[Path]) -> bool:
    patterns = (
        re.compile(rf"^{re.escape(name)}\s*=\s*\{{?\s*workspace\s*=\s*true"),
        re.compile(rf"^{re.escape(name)}\.workspace\s*=\s*true"),
    )
    for path in files:
        text = path.read_text(encoding="utf-8", errors="ignore")
        if any(p.search(text) for p in patterns):
            return True
    return False


def main() -> int:
    if not SZ_ORM_DIR.is_dir():
        print(f"[repro] FAIL: 缺少兄弟仓库 {SZ_ORM_DIR}", file=sys.stderr)
        print(
            "[repro] 修复：git clone <sz-orm-repo> ../sz-orm && "
            "git -C ../sz-orm checkout $(cat .sz-orm-commit)",
            file=sys.stderr,
        )
        return 2

    text = CARGO_TOML.read_text(encoding="utf-8")
    path_deps = list(PATH_DEP_RE.finditer(text))
    sz_orm_deps = [m for m in path_deps if m.group("path").startswith("../sz-orm/")]
    lock = parse_lock()
    members = member_cargo_files()
    failures: list[str] = []
    warnings: list[str] = []

    for m in sz_orm_deps:
        name = m.group("name")
        rel = m.group("path")
        target = (REPO_ROOT / rel).resolve()
        if not target.exists():
            failures.append(f"{name}: path 目标不存在 {target}")
            continue
        if name not in lock:
            if is_used_by_member(name, members):
                failures.append(f"{name}: 成员已引用但 Cargo.lock 中缺少解析结果")
            else:
                warnings.append(f"{name}: workspace 声明但无成员使用（未进入 lock）")
            continue
        version_match = VERSION_RE.search(m.group("attrs"))
        if version_match and not satisfies(version_match.group(1), lock[name]):
            failures.append(
                f"{name}: 声明 {version_match.group(1)} 不满足实际解析 {lock[name]}"
            )

    family = {k: v for k, v in lock.items() if k.startswith("sz-orm")}
    if family and len(set(family.values())) > 1:
        failures.append(f"sz-orm 家族版本不一致: {family}")

    pin = PIN_FILE.read_text(encoding="utf-8").strip() if PIN_FILE.exists() else ""
    head = git_head(SZ_ORM_DIR)
    if not pin:
        failures.append(".sz-orm-commit 缺失或为空")
    elif head is None:
        print("[repro] WARN: 兄弟仓库不是 git 工作树，无法校验提交固定")
    elif head != pin:
        failures.append(f"sz-orm HEAD {head} != 固定提交 {pin}")

    if failures:
        print("[repro] FAIL:", file=sys.stderr)
        for item in failures:
            print(f"  - {item}", file=sys.stderr)
        return 1

    for item in warnings:
        print(f"[repro] WARN: {item}")

    print(
        f"[repro] PASS: {len(sz_orm_deps)} 个 sz-orm path 依赖可解析，"
        f"版本 {sorted(set(family.values()))}，提交 {pin[:12]}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
