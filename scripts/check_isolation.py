#!/usr/bin/env python3
"""隔离检查脚本：验证开源版/企业版仓库的物理隔离。

检查规则：
- repo_type=oss: packages/ 下不得存在企业版 crate 目录
- repo_type=enterprise: 企业版 crate 内不得包含开源核心 crate 的 fork/修改副本

接口签名（对应 design.md §2.2.2 接口 1）：
    入参: project_root, repo_type (oss/enterprise), enterprise_crates, oss_core_crates
    出参: CheckResult（含违规项 file:line 证据）

用法:
    python scripts/check_isolation.py --project-root . --repo-type oss
    python scripts/check_isolation.py --project-root /www/rust/sz-rust-enterprise --repo-type enterprise
"""

import argparse
import sys
from dataclasses import dataclass
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

OSS_CORE_CRATES = {
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
    "sz-rust-examples",
    "sz-rust-addon-hot-reload-example",
    "sz-rust-facade-tests",
    "sz-rust-k8s-operator",
}


@dataclass
class Violation:
    file: str
    line: int
    description: str

    def __str__(self) -> str:
        return f"  {self.file}:{self.line}  {self.description}"


def check_oss_isolation(project_root: Path) -> list[Violation]:
    """检查开源仓库不得包含企业版 crate 目录。"""
    violations = []
    packages_dir = project_root / "packages"
    if not packages_dir.exists():
        return [Violation(str(project_root), 0, "packages/ 目录不存在")]

    for sub in packages_dir.iterdir():
        if not sub.is_dir():
            continue
        if sub.name in ENTERPRISE_CRATES:
            violations.append(
                Violation(
                    file=str(sub),
                    line=0,
                    description=f"开源仓库不得包含企业版 crate: {sub.name}",
                )
            )

    cargo_toml = project_root / "Cargo.toml"
    if cargo_toml.exists():
        for idx, line in enumerate(cargo_toml.read_text(encoding="utf-8").splitlines(), 1):
            for ec in ENTERPRISE_CRATES:
                if f"packages/{ec}" in line and "members" not in line:
                    if "members" in line or '"' in line:
                        violations.append(
                            Violation(
                                file=str(cargo_toml),
                                line=idx,
                                description=f"开源仓库 Cargo.toml 引用企业版 crate: {ec}",
                            )
                        )

    return violations


def check_enterprise_isolation(project_root: Path) -> list[Violation]:
    """检查企业版仓库不得包含开源核心 crate 的 fork 副本。"""
    violations = []
    packages_dir = project_root / "packages"
    if not packages_dir.exists():
        return [Violation(str(project_root), 0, "packages/ 目录不存在")]

    for sub in packages_dir.iterdir():
        if not sub.is_dir():
            continue
        if sub.name in OSS_CORE_CRATES:
            violations.append(
                Violation(
                    file=str(sub),
                    line=0,
                    description=f"企业版仓库不得包含开源核心 crate fork: {sub.name}",
                )
            )

    return violations


def main() -> int:
    parser = argparse.ArgumentParser(description="隔离检查：验证开源/企业版物理隔离")
    parser.add_argument("--project-root", type=Path, required=True)
    parser.add_argument(
        "--repo-type",
        choices=["oss", "enterprise"],
        required=True,
    )
    args = parser.parse_args()

    root = args.project_root.resolve()
    if not root.exists():
        print(f"项目根目录不存在: {root}", file=sys.stderr)
        return 2

    if args.repo_type == "oss":
        violations = check_oss_isolation(root)
    else:
        violations = check_enterprise_isolation(root)

    if violations:
        print(f"隔离检查失败（{len(violations)} 项违规）:")
        for v in violations:
            print(v)
        return 1

    print("isolation check passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())