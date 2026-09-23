#!/usr/bin/env python3
"""Detect lock guards held across .await calls."""
import re
import sys
from pathlib import Path

LOCK_PATTERNS = [
    r'\bMutexGuard\b',
    r'\bRwLockReadGuard\b',
    r'\bRwLockWriteGuard\b',
    r'\.write\(\)',
    r'\.read\(\)',
    r'\.lock\(\)',
]


def scan_file(filepath: Path) -> list:
    violations = []
    try:
        lines = filepath.read_text(encoding='utf-8', errors='ignore').splitlines()
    except Exception:
        return violations

    lock_active = False
    lock_line = 0
    lock_var = None

    for i, line in enumerate(lines, 1):
        stripped = line.lstrip()

        for pattern in LOCK_PATTERNS:
            if re.search(pattern, stripped) and not stripped.startswith('//'):
                lock_active = True
                lock_line = i
                match = re.search(r'let\s+(?:mut\s+)?(\w+)', stripped)
                if match:
                    lock_var = match.group(1)
                break

        if lock_active and '.await' in stripped and not stripped.startswith('//'):
            violations.append({
                'file': str(filepath),
                'line': i,
                'type': 'LOCK_ACROSS_AWAIT',
                'detail': f'Lock acquired at L{lock_line} held across .await: {stripped.strip()[:80]}',
            })

        if lock_active and stripped and not stripped.startswith('//'):
            if stripped.startswith('}') or stripped.startswith('drop(') or (lock_var and f'drop({lock_var})' in stripped):
                lock_active = False
                lock_var = None
    return violations


def main():
    if len(sys.argv) < 2:
        print("Usage: detect_lock_across_await.py <source_dir>")
        sys.exit(1)

    src_dir = Path(sys.argv[1])
    all_violations = []

    for rs_file in src_dir.rglob('*.rs'):
        if 'target' in rs_file.parts:
            continue
        all_violations.extend(scan_file(rs_file))

    if all_violations:
        print(f"Found {len(all_violations)} lock-across-await violation(s):")
        for v in all_violations:
            print(f"  {v['file']}:{v['line']} [{v['type']}] {v['detail']}")
        sys.exit(1)
    else:
        print("No lock-across-await violations found.")
        sys.exit(0)


if __name__ == '__main__':
    main()