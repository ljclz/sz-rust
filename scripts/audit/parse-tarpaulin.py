#!/usr/bin/env python3
"""解析 tarpaulin JSON 报告，输出指定 crate 的 per-file 覆盖率汇总。

用法: python parse-tarpaulin.py <tarpaulin-report.json> [crate 名过滤]
"""
import json
import sys


def main() -> None:
    if len(sys.argv) < 2:
        print("用法: python parse-tarpaulin.py <report.json> [filter]")
        sys.exit(1)
    report_path = sys.argv[1]
    filt = sys.argv[2] if len(sys.argv) > 2 else "sz-rust-sz300"

    data = json.load(open(report_path, encoding="utf-8"))
    files = data.get("files", [])
    total_cov = total_lines = 0
    rows = []
    for f in files:
        raw = f.get("name") or f.get("path") or ""
        if isinstance(raw, list):
            name = "/".join(raw).replace("\\", "/")
        else:
            name = str(raw).replace("\\", "/")
        if filt not in name:
            continue
        stats = f.get("coverage", {})
        covered = stats.get("covered_lines", f.get("covered", 0))
        total = stats.get("total_lines", f.get("coverable", 0))
        rows.append((name, covered, total))
        total_cov += covered
        total_lines += total

    rows.sort(key=lambda x: -(x[2] or 0))
    for name, covered, total in rows:
        pct = f"{covered / total * 100:.1f}%" if total else "0 lines"
        print(f"{name}: {covered}/{total} = {pct}")
    if total_lines:
        print(
            f"=== {filt} 总计: {total_cov}/{total_lines} = {total_cov / total_lines * 100:.1f}%"
        )
    else:
        print("无匹配文件")


if __name__ == "__main__":
    main()
