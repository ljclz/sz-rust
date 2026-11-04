#!/usr/bin/env python3
"""Detect blocking calls in async context (std::fs, std::thread::sleep, sync IO)."""
import re
import sys
from pathlib import Path

BLOCKING_PATTERNS = [
    r'std::thread::sleep',
    r'std::fs::read',
    r'std::fs::write',
    r'std::fs::File',
    r'std::io::Read',
    r'std::io::Write',
    r'std::io::stdin',
    r'\bblock_on\b',
    r'\bstd::process::Command\b',
]


def scan_file(filepath: Path) -> list:
    violations = []
    try:
        lines = filepath.read_text(encoding='utf-8', errors='ignore').splitlines()
    except Exception:
        return violations

    in_async = False
    fn_start = 0

    for i, line in enumerate(lines, 1):
        stripped = line.lstrip()

        if re.search(r'\basync\s+fn\b', stripped):
            in_async = True
            fn_start = i

        if stripped and not stripped.startswith('//'):
            if re.search(r'^\bfn\b', stripped) or stripped.startswith('}'):
                if stripped.startswith('}'):
                    in_async = False

        if in_async and not stripped.startswith('//'):
            for pattern in BLOCKING_PATTERNS:
                if re.search(pattern, stripped):
                    violations.append({
                        'file': str(filepath),
                        'line': i,
                        'type': 'BLOCKING_CALL',
                        'detail': f'Blocking call in async context (fn started L{fn_start}): {stripped.strip()[:80]}',
                    })
    return violations


def main():
    if len(sys.argv) < 2:
        print("Usage: detect_blocking_call.py <source_dir>")
        sys.exit(1)

    src_dir = Path(sys.argv[1])
    all_violations = []

    for rs_file in src_dir.rglob('*.rs'):
        if 'target' in rs_file.parts or 'tests' in rs_file.parts:
            continue
        all_violations.extend(scan_file(rs_file))

    if all_violations:
        print(f"Found {len(all_violations)} blocking call violation(s):")
        for v in all_violations:
            print(f"  {v['file']}:{v['line']} [{v['type']}] {v['detail']}")
        sys.exit(1)
    else:
        print("No blocking call violations found.")
        sys.exit(0)


if __name__ == '__main__':
    main()