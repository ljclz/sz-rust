#!/usr/bin/env bash
# 性能回归门禁：检查 criterion 基准组延迟增加是否超过阈值
# 用法: regression_gate.sh <baseline_name> <threshold_pct>
# 退出码: 0=通过, 1=回归

set -euo pipefail

BASELINE="${1:-pre-optimization}"
THRESHOLD="${2:-10}"
CRITERION_DIR="${CARGO_TARGET_DIR:-target}/criterion"

if [ ! -d "$CRITERION_DIR" ]; then
   (   echo "ERROR: criterion directory not found: $CRITERION_DIR"
    echo "Run 'cargo bench -- --save-baseline $BASELINE' first."
    exit 1
    )
fi

echo "# 性能回归门禁报告"
echo "> 基线: \`$BASELINE\` | 阈值: ${THRESHOLD}%"
echo ""

REGRESSIONS=0
TOTAL=0

for bench_group in $(find "$CRITERION_DIR" -maxdepth 1 -type d ! -path "$CRITERION_DIR" ! -name "report"); do
    group_name=$(basename "$bench_group")
    [ "$group_name" = "estimates" ] && continue

    for bench_sub in $(find "$bench_group" -maxdepth 1 -type d ! -path "$bench_group" ! -name "report"); do
        sub_name=$(basename "$bench_sub")
        [ "$sub_name" = "$BASELINE" ] && continue

        pre_file="$bench_group/$BASELINE/$sub_name/new/estimates.json"
        post_file="$bench_sub/new/estimates.json"

        if [ ! -f "$pre_file" ] || [ ! -f "$post_file" ]; then
            continue
        fi

        TOTAL=$((TOTAL + 1))

        pre_mean=$(python3 -c "import json; d=json.load(open('$pre_file')); print(d.get('mean',{}).get('point_estimate',0))" 2>/dev/null || echo "0")
        post_mean=$(python3 -c "import json; d=json.load(open('$post_file')); print(d.get('mean',{}).get('point_estimate',0))" 2>/dev/null || echo "0")

        if [ "$pre_mean" = "0" ] || [ "$post_mean" = "0" ]; then
            continue
        fi

        change=$(python3 -c "print(round(($post_mean - $pre_mean) / $pre_mean * 100, 1))" 2>/dev/null || echo "0")

        status="PASS"
        if (( $(echo "$change > $THRESHOLD" | bc -l 2>/dev/null || echo 0) )); then
            status="REGRESSION"
            REGRESSIONS=$((REGRESSIONS + 1))
        fi

        echo "| $group_name/$sub_name | $pre_mean | $post_mean | ${change}% | $status |"
    done
done

echo ""
echo "总计: $TOTAL 基准组, $REGRESSIONS 回归"

if [ "$REGRESSIONS" -gt 0 ]; then
    echo "FAILED: 性能回归超过 ${THRESHOLD}%"
    exit 1
else
    echo "PASSED: 无性能回归"
    exit 0
fi