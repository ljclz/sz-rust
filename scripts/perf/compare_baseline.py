#!/usr/bin/env python3
"""Parse criterion benchmark JSON output and generate 6-dimension comparison report."""
import json
import sys
from pathlib import Path


def load_baseline(criterion_dir: Path, baseline: str) -> dict:
    results = {}
    for bench_dir in criterion_dir.iterdir():
        if not bench_dir.is_dir() or bench_dir.name in ('report', 'estimates'):
            continue
        for sub in bench_dir.iterdir():
            if not sub.is_dir() or sub.name in ('report', 'estimates'):
                continue
            if baseline == 'new':
                est_file = sub / 'new' / 'estimates.json'
            else:
                est_file = sub / baseline / 'estimates.json'
            if est_file.exists():
                data = json.loads(est_file.read_text())
                results[f"{bench_dir.name}/{sub.name}"] = {
                    'mean_ns': data.get('mean', {}).get('point_estimate', 0),
                    'median_ns': data.get('median', {}).get('point_estimate', 0),
                }
    return results


def main():
    if len(sys.argv) < 3:
        print("Usage: compare_baseline.py <criterion_dir> <output_md> [baseline_name]")
        sys.exit(1)

    criterion_dir = Path(sys.argv[1])
    output_path = Path(sys.argv[2])
    baseline_name = sys.argv[3] if len(sys.argv) > 3 else "pre-optimization"

    baseline = load_baseline(criterion_dir, baseline_name)
    current = load_baseline(criterion_dir, "new")

    lines = [
        "# 性能基线对比报告",
        "",
        f"> 基线: `{baseline_name}` | 当前: `new`",
        f"> 来源: `{criterion_dir}/`",
        "",
        "## 延迟对比（P50/P99）",
        "",
        "| 基准组 | 基线 P50 (ns) | 当前 P50 (ns) | 变化 (%) |",
        "|--------|--------------|--------------|---------|",
    ]

    all_keys = sorted(set(baseline.keys()) | set(current.keys()))
    for key in all_keys:
        pre = baseline.get(key, {}).get('median_ns', 0)
        post = current.get(key, {}).get('median_ns', 0)
        if pre > 0:
            change = ((post - pre) / pre) * 100
            lines.append(f"| {key} | {pre:.0f} | {post:.0f} | {change:+.1f}% |")
        else:
            lines.append(f"| {key} | — | {post:.0f} | — |")

    lines.extend([
        "",
        "## 来源标注",
        f"- 基线数据: `{criterion_dir}/<bench>/<sub>/{baseline_name}/estimates.json`",
        f"- 当前数据: `{criterion_dir}/<bench>/<sub>/new/estimates.json`",
    ])

    output_path.write_text('\n'.join(lines), encoding='utf-8')
    print(f"Report saved to {output_path}")


if __name__ == '__main__':
    main()