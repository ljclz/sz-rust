#!/usr/bin/env python3
"""Detect serial .await calls that could potentially be parallelized."""
import re
import sys
from pathlib import Path


def scan_file(filepath: Path) -> list:
    violations = []
    try:
        lines = filepath.read_text(encoding='utf-8', errors='ignore').splitlines()
    except Exception:
        return violations

    await_lines = []
    current_fn = None
    fn_start = 0

    for i, line in enumerate(lines, 1):
        stripped = line.lstrip()

        fn_match = re.search(r'\b(?:async\s+)?fn\s+(\w+)', stripped)
        if fn_match:
            if len(await_lines) >= 2:
                violations.append({
                    'file': str(filepath),
                    'line': fn_start,
                    'type': 'SERIAL_AWAIT',
                    'detail': f'{len(await_lines)} serial awaits in fn {current_fn} (L{fn_start}-L{i-1}), consider parallelizing',
                })
            current_fn = fn_match.group(1)
            fn_start = i
            await_lines = []

        if '.await' in stripped and not stripped.startswith('//'):
            await_lines.append(i)

    if current_fn and len(await_lines) >= 2:
        violations.append({
            'file': str(filepath),
            'line': fn_start,
            'type': 'SERIAL_AWAIT',
            'detail': f'{len(await_lines)} serial awaits in fn {current_fn} (L{fn_start}-L{len(lines)}), consider parallelizing',
        })
    return violations


def main():
    if len(sys.argv) < 2:
        print("Usage: detect_serial_await.py <source_dir>")
        sys.exit(1)

    src_dir = Path(sys.argv[1])
    all_violations = []

    for rs_file in src_dir.rglob('*.rs'):
        if 'target' in rs_file.parts:
            continue
        all_violations.extend(scan_file(rs_file))

    if all_violations:
        print(f"Found {len(all_violations)} serial await candidate(s):")
        for v in all_violations:
            print(f"  {v['file']}:{v['line']} [{v['type']}] {v['detail']}")
    else:
        print("No serial await candidates found.")
    sys.exit(0)


if __name__ == '__main__':
    main()