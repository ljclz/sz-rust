#!/usr/bin/env python3
"""铁律 4 门禁：禁止 std::fs（统一 tokio::fs）

扫描 packages/*/src 下的生产代码（排除测试上下文与 tests/benches/examples 目录），
命中 std::fs 即报错。复用 check-unwrap.py 的上下文扫描逻辑：
- 排除 `#[cfg(test)]` / `mod tests` / `mod .*_tests` 块
- 排除 `#[test]` / `#[tokio::test]` 函数体
- 排除 tests/ benches/ examples/ 目录

退出码：0 = 无生产 std::fs；1 = 存在（fail-closed）
"""
import os
import re
import sys


def strip_scl(line):
    """去字符串字面量 + // 注释"""
    out, i, n, in_str = [], 0, len(line), None
    while i < n:
        ch = line[i]
        if in_str:
            if ch == chr(92) and i + 1 < n:
                i += 2
                continue
            if ch == in_str:
                in_str = None
            i += 1
            continue
        if ch in ('"', chr(39)):
            in_str = ch
            i += 1
            continue
        if ch == '/' and i + 1 < n and line[i + 1] == '/':
            break
        out.append(ch)
        i += 1
    return ''.join(out)


def scan(path):
    with open(path, encoding='utf-8', errors='replace') as f:
        lines = f.readlines()
    prod = []
    mod_depth = 0
    fn_stack = []  # (is_test_fn, brace_depth)
    pending_test = False
    for i, raw in enumerate(lines):
        line = strip_scl(raw).strip()
        if not line:
            continue
        # 测试模块块（mod tests / mod xxx_tests / #[cfg(test)]）
        if mod_depth == 0 and re.search(r'#\[cfg\(test\)\]|mod \w*tests\b', line):
            mod_depth = max(1, line.count('{'))
            continue
        if mod_depth > 0:
            mod_depth += line.count('{') - line.count('}')
            if mod_depth <= 0:
                mod_depth = 0
            continue
        # 测试函数
        if re.search(r'#\[(tokio::)?test', line):
            pending_test = True
            continue
        if re.search(r'^(pub\s+)?(async\s+)?fn\s|^fn\s', line):
            is_test = pending_test
            pending_test = False
            fn_stack.append([is_test, line.count('{') - line.count('}')])
            continue
        if fn_stack:
            delta = line.count('{') - line.count('}')
            fn_stack[-1][1] += delta
            if fn_stack[-1][1] <= 0:
                fn_stack.pop()
                continue
            if fn_stack[-1][0]:
                continue
        if 'std::fs' in line:
            prod.append((i + 1, raw.strip()[:100]))
    return prod


def main():
    total = 0
# 豁免清单（2026-08-16 用户裁定，理由入 doc-debt DB-2026-08-16-06）：
# - sz-rust-pdf: umya-spreadsheet 第三方库接口要求同步 std::fs::File（库内部实现，非我方可控）
# - sz-rust-cli: 同步命令行工具，无 tokio runtime，铁律 4 意图（不阻塞异步运行时）不适用
EXEMPT_PREFIXES = (
    'packages\\sz-rust-pdf\\',
    'packages\\sz-rust-cli\\',
    # mvc view 渲染引擎：同步公共 API（View trait），async 化=引擎级重构，
    # 债务 DB-2026-08-16-06 登记，专项排期中（2026-08-16 用户裁定豁免）
    'packages\\sz-rust-mvc-facade\\src\\view\\',
    'packages\\sz-rust-mvc-facade\\src\\view.rs',
)


def is_exempt(p: str) -> bool:
    return any(p.startswith(prefix) for prefix in EXEMPT_PREFIXES)


def main():
    total = 0
    findings = []
    for root, dirs, files in os.walk('packages'):
        if 'target' in root or r'\tests' in root or r'\benches' in root or r'\examples' in root:
            continue
        for f in files:
            if not f.endswith('.rs'):
                continue
            p = os.path.join(root, f)
            if is_exempt(p):
                continue
            hits = scan(p)
            if hits:
                total += len(hits)
                for ln, txt in hits:
                    findings.append(f"{p}:{ln}: {txt}")
    print("PROD_STD_FS:", total)
    for f in findings:
        print(" ", f)
    if total > 0:
        print("❌ 铁律 4 违反：生产代码使用 std::fs，统一改为 tokio::fs")
        sys.exit(1)
    print("✅ 铁律 4 合规：生产代码无 std::fs")
    sys.exit(0)


if __name__ == '__main__':
    main()
