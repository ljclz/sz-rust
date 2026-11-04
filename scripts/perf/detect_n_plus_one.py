#!/usr/bin/env python3
"""Detect N+1 query patterns: DB calls inside loops."""
import re
import sys
from pathlib import Path

DB_CALL_PATTERNS = [
    r'\bfetch_related\b',
    r'\bfind_one\b',
    r'\bfind_by_id\b',
    r'\.find\(',
]
LOOP_PATTERNS = [r'\bfor\b', r'\bwhile\b']


def scan_file(filepath: Path) -> list:
    violations = []
    try:
        lines = filepath.read_text(encoding='utf-8', errors='ignore').splitlines()
    except Exception:
        return violations

    in_loop = False
    loop_depth = 0
    loop_start = 0

    for i, line in enumerate(lines, 1):
        stripped = line.lstrip()
        indent = len(line) - len(stripped)

        if any(re.search(p, stripped) for p in LOOP_PATTERNS) and '{' not in stripped:
            if 'impl' in stripped or 'for' in stripped and ('Repository' in stripped or 'Trait' in stripped or 'where' in stripped):
                continue
            in_loop = True
            loop_depth = indent
            loop_start = i
        elif in_loop and indent <= loop_depth and stripped and not stripped.startswith('//'):
            if stripped.startswith('}') or stripped.startswith(')'):
                in_loop = False

        if in_loop and not stripped.startswith('//'):
            for pattern in DB_CALL_PATTERNS:
                if re.search(pattern, stripped):
                    violations.append({
                        'file': str(filepath),
                        'line': i,
                        'type': 'N+1_QUERY',
                        'detail': f'DB call in loop (started L{loop_start}): {stripped.strip()[:80]}',
                    })
    return violations


def main():
    if len(sys.argv) < 2:
        print("Usage: detect_n_plus_one.py <source_dir>")
        sys.exit(1)

    src_dir = Path(sys.argv[1])
    all_violations = []

    for rs_file in src_dir.rglob('*.rs'):
        if 'target' in rs_file.parts or 'tests' in rs_file.parts:
            continue
        all_violations.extend(scan_file(rs_file))

    if all_violations:
        print(f"Found {len(all_violations)} N+1 query violation(s):")
        for v in all_violations:
            print(f"  {v['file']}:{v['line']} [{v['type']}] {v['detail']}")
        sys.exit(1)
    else:
        print("No N+1 query violations found.")
        sys.exit(0)


if __name__ == '__main__':
    main()