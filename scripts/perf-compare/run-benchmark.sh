#!/bin/bash
set -euo pipefail

FRAMEWORKS="sz-rust,actix,axum,rocket,poem"
DURATION=30
CONCURRENCY=64
ROUTES="/simple,/json,/user/42,/db"
BENCH_DIR="/www/rust/perf-compare/benchmarks"
RESULTS_DIR="/www/rust/perf-compare"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
RAW_FILE="$RESULTS_DIR/raw-results-${TIMESTAMP}.jsonl"
RESULTS_FILE="$RESULTS_DIR/results-${TIMESTAMP}.json"
ROCKET_RESULT_FILE="$RESULTS_DIR/rocket-build-result.json"
REPORT_DATE=$(date +%Y-%m-%d)
REPORT_FILE="$RESULTS_DIR/framework-comparison-${REPORT_DATE}.md"

while [[ $# -gt 0 ]]; do
    case $1 in
        --frameworks) FRAMEWORKS="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --concurrency) CONCURRENCY="$2"; shift 2 ;;
        --routes) ROUTES="$2"; shift 2 ;;
        *) shift ;;
    esac
done

source ~/.cargo/env 2>/dev/null

echo "=== 同条件性能对比压测 ==="
echo "框架: $FRAMEWORKS"
echo "时长: ${DURATION}s"
echo "并发: $CONCURRENCY"
echo "路由: $ROUTES"
echo ""

mkdir -p "$RESULTS_DIR"
echo "" > "$RAW_FILE"

declare -A PORTS
PORTS[sz-rust]=8401
PORTS[actix]=8402
PORTS[axum]=8403
PORTS[rocket]=8404
PORTS[poem]=8405

declare -A PKG_NAMES
PKG_NAMES[sz-rust]="bench-sz-rust"
PKG_NAMES[actix]="bench-actix"
PKG_NAMES[axum]="bench-axum"
PKG_NAMES[rocket]="bench-rocket"
PKG_NAMES[poem]="bench-poem"

IFS=',' read -ra FW_ARRAY <<< "$FRAMEWORKS"
IFS=',' read -ra ROUTE_ARRAY <<< "$ROUTES"

record_rocket_result() {
    local status="$1"
    local reason="$2"
    local duration_sec="$3"
    local included="$4"
    cat > "$ROCKET_RESULT_FILE" <<EOF
{"framework":"rocket","status":"$status","reason":"$reason","duration_sec":$duration_sec,"included_in_benchmark":$included,"timestamp":"$(date -Iseconds)"}
EOF
    echo "rocket 结论已记录: $ROCKET_RESULT_FILE"
}

aggregate_results() {
    echo "--- 生成汇总 JSON ---"
    if command -v jq &>/dev/null; then
        jq -n \
            --arg ts "$(date -Iseconds)" \
            --arg dur "$DURATION" \
            --arg conc "$CONCURRENCY" \
            '{timestamp:$ts, duration:($dur|tonumber), concurrency:($conc|tonumber), results:{}}' > "$RESULTS_FILE"

        while IFS= read -r line; do
            [ -z "$line" ] && continue
            fw=$(echo "$line" | jq -r '.framework')
            jq --argjson entry "$line" \
               --arg fw "$fw" \
               '.results[$fw] += [$entry]' "$RESULTS_FILE" > "$RESULTS_FILE.tmp"
            mv "$RESULTS_FILE.tmp" "$RESULTS_FILE"
        done < "$RAW_FILE"
    else
        echo '{"timestamp":"'$(date -Iseconds)'","duration":'$DURATION',"concurrency":'$CONCURRENCY',"results":{}}' > "$RESULTS_FILE"
        while IFS= read -r line; do
            [ -z "$line" ] && continue
            fw=$(echo "$line" | sed -n 's/.*"framework":"\([^"]*\)".*/\1/p')
            route=$(echo "$line" | sed -n 's/.*"route":"\([^"]*\)".*/\1/p')
            rps=$(echo "$line" | sed -n 's/.*"rps":\([0-9.]*\).*/\1/p')
            echo "  $fw $route rps=$rps"
        done < "$RAW_FILE"
    fi
    echo "✅ 汇总 JSON: $RESULTS_FILE"
}

generate_report() {
    echo "--- 生成 Markdown 对比报告 ---"
    {
        echo "# 框架性能对比报告"
        echo ""
        echo "> 数据来源：同条件实测（服务器 122.51.216.76，wrk 4.1.0，${CONCURRENCY} 并发 ${DURATION}s）"
        echo "> 生成时间：$(date -Iseconds)"
        echo "> Rust 版本：$(rustc --version 2>/dev/null || echo 'unknown')"
        echo ""
        echo "## RPS 对比（请求数/秒）"
        echo ""
        echo "| 框架 | /simple | /json | /user/42 | /db |"
        echo "|------|---------|-------|----------|------|"
        for fw in "${FW_ARRAY[@]}"; do
            row="| $fw |"
            for route in "${ROUTE_ARRAY[@]}"; do
                rps=$(grep "\"framework\":\"$fw\"" "$RAW_FILE" | grep "\"route\":\"$route\"" | sed -n 's/.*"rps":\([0-9.]*\).*/\1/p' | head -1)
                p99=$(grep "\"framework\":\"$fw\"" "$RAW_FILE" | grep "\"route\":\"$route\"" | sed -n 's/.*"p99":"\([^"]*\)".*/\1/p' | head -1)
                if [ -n "$rps" ]; then
                    row="$row ${rps} (P99=${p99}ms) |"
                else
                    row="$row N/A |"
                fi
            done
            echo "$row"
        done
        echo ""

        echo "## P50/P95/P99 延迟对比（ms）"
        echo ""
        echo "| 框架 | 路由 | P50 | P95 | P99 |"
        echo "|------|------|-----|-----|-----|"
        while IFS= read -r line; do
            [ -z "$line" ] && continue
            fw=$(echo "$line" | sed -n 's/.*"framework":"\([^"]*\)".*/\1/p')
            route=$(echo "$line" | sed -n 's/.*"route":"\([^"]*\)".*/\1/p')
            p50=$(echo "$line" | sed -n 's/.*"p50":"\([^"]*\)".*/\1/p')
            p95=$(echo "$line" | sed -n 's/.*"p95":"\([^"]*\)".*/\1/p')
            p99=$(echo "$line" | sed -n 's/.*"p99":"\([^"]*\)".*/\1/p')
            echo "| $fw | $route | $p50 | $p95 | $p99 |"
        done < "$RAW_FILE"
        echo ""

        echo "## 错误数与 RSS 内存对比"
        echo ""
        echo "| 框架 | 路由 | 错误数 | RSS(MB) |"
        echo "|------|------|--------|---------|"
        while IFS= read -r line; do
            [ -z "$line" ] && continue
            fw=$(echo "$line" | sed -n 's/.*"framework":"\([^"]*\)".*/\1/p')
            route=$(echo "$line" | sed -n 's/.*"route":"\([^"]*\)".*/\1/p')
            errors=$(echo "$line" | sed -n 's/.*"errors":\([0-9]*\).*/\1/p')
            rss=$(echo "$line" | sed -n 's/.*"rss_mb":\([0-9]*\).*/\1/p')
            echo "| $fw | $route | ${errors:-0} | ${rss:-0} |"
        done < "$RAW_FILE"
        echo ""

        if [ -f "$ROCKET_RESULT_FILE" ]; then
            rocket_status=$(sed -n 's/.*"status":"\([^"]*\)".*/\1/p' "$ROCKET_RESULT_FILE")
            rocket_included=$(sed -n 's/.*"included_in_benchmark":\([^,}]*\).*/\1/p' "$ROCKET_RESULT_FILE")
            if [ "$rocket_included" = "false" ]; then
                rocket_reason=$(sed -n 's/.*"reason":"\([^"]*\)".*/\1/p' "$ROCKET_RESULT_FILE")
                echo "## rocket 框架说明"
                echo ""
                echo "> ⚠️ rocket 因${rocket_reason}暂未参与压测"
                echo ""
            fi
        fi

        echo "## 原始数据"
        echo ""
        echo "- 原始结果：\`$RAW_FILE\`"
        echo "- 汇总 JSON：\`$RESULTS_FILE\`"
    } > "$REPORT_FILE"
    echo "✅ Markdown 报告: $REPORT_FILE"
}

for fw in "${FW_ARRAY[@]}"; do
    port=${PORTS[$fw]}
    pkg=${PKG_NAMES[$fw]}
    echo "--- 编译 $fw ($pkg) ---"
    cd "$BENCH_DIR/$fw"

    if [ "$fw" = "rocket" ]; then
        start_time=$(date +%s)
        timeout 1800 cargo build --release 2>&1 | tail -5
        build_exit=$?
        end_time=$(date +%s)
        duration_sec=$((end_time - start_time))

        if [ $build_exit -eq 124 ]; then
            echo "❌ rocket 编译超时（>30 分钟），已终止"
            record_rocket_result "timeout" "编译超时（>30 分钟）" 1800 false
            continue
        elif [ $build_exit -ne 0 ]; then
            echo "❌ rocket 编译失败（退出码 $build_exit）"
            record_rocket_result "failed" "cargo build 失败（退出码 $build_exit）" $duration_sec false
            continue
        else
            echo "✅ rocket 编译成功（${duration_sec}s）"
            record_rocket_result "success" "null" $duration_sec true
        fi
    else
        cargo build --release 2>&1 | tail -5
        if [ ! -f "target/release/$pkg" ]; then
            echo "❌ $fw 编译失败，跳过"
            continue
        fi
        echo "✅ $fw 编译成功"
    fi

    echo "--- 启动 $fw (端口 $port) ---"
    PORT=$port ./target/release/$pkg &
    SERVER_PID=$!
    echo "PID=$SERVER_PID"

    sleep 3

    if ! curl -s "http://127.0.0.1:$port/simple" > /dev/null 2>&1; then
        echo "❌ $fw 启动失败"
        kill $SERVER_PID 2>/dev/null || true
        continue
    fi
    echo "✅ $fw 已启动"

    for route in "${ROUTE_ARRAY[@]}"; do
        echo "--- 压测 $fw $route ---"
        RESULT=$(wrk -t $CONCURRENCY -c $CONCURRENCY -d ${DURATION}s --latency "http://127.0.0.1:$port$route" 2>&1)
        echo "$RESULT"

        RPS=$(echo "$RESULT" | grep "Requests/sec" | awk '{print $2}')
        P50=$(echo "$RESULT" | grep "50%" | awk '{print $2}')
        P95=$(echo "$RESULT" | grep "95%" | awk '{print $2}')
        P99=$(echo "$RESULT" | grep "99%" | awk '{print $2}')
        ERRORS=$(echo "$RESULT" | grep "Non-2xx or 3xx" | awk '{print $2}' || echo "0")
        RSS=$(ps -o rss= -p $SERVER_PID 2>/dev/null | awk '{print int($1/1024)}' || echo "0")

        echo "  RPS=$RPS P50=$P50 P95=$P95 P99=$P99 Errors=$ERRORS RSS=${RSS}MB"

        echo "{\"framework\":\"$fw\",\"route\":\"$route\",\"rps\":${RPS:-0},\"p50\":\"${P50:-0}\",\"p95\":\"${P95:-0}\",\"p99\":\"${P99:-0}\",\"errors\":${ERRORS:-0},\"rss_mb\":${RSS:-0},\"timestamp\":\"$(date -Iseconds)\",\"duration\":$DURATION,\"concurrency\":$CONCURRENCY}" >> "$RAW_FILE"
    done

    kill $SERVER_PID 2>/dev/null || true
    sleep 2
    kill -9 $SERVER_PID 2>/dev/null || true
    echo "✅ $fw 已停止"
    echo ""
done

aggregate_results
generate_report

echo "=== 压测完成 ==="
echo "原始结果: $RAW_FILE"
echo "汇总 JSON: $RESULTS_FILE"
echo "Markdown 报告: $REPORT_FILE"
if [ -f "$ROCKET_RESULT_FILE" ]; then
    echo "rocket 结论: $ROCKET_RESULT_FILE"
fi
exit 0
