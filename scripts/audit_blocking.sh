#!/usr/bin/env bash
# =============================================================================
# spawn_blocking 审计脚本 — P3 任务 6.2
#
# 扫描 workspace 所有 .rs 文件，检测 async fn 内的阻塞调用：
#   - std::fs::* （应改用 tokio::fs）
#   - std::thread::sleep （应改用 tokio::time::sleep）
#   - std::process::Command::wait （应改用 tokio::process）
#   - std::net::* 阻塞 IO （应改用 tokio::net）
#   - .lock() 长时间持锁（需评估是否 spawn_blocking）
#
# 输出审计报告：file:line + 阻塞调用类型 + 建议
#
# 用法：
#   bash scripts/audit_blocking.sh           # 扫描全 workspace
#   bash scripts/audit_blocking.sh packages/  # 扫描指定目录
# =============================================================================

set -euo pipefail

# 颜色输出
RED='\033[0;31m'
YELLOW='\033[1;33m'
GREEN='\033[0;32m'
NC='\033[0m'

# 扫描根目录（默认为 workspace 根）
SCAN_DIR="${1:-.}"
REPORT_FILE="docs/audit/blocking_audit_$(date +%Y%m%d_%H%M%S).md"

# 阻塞调用模式列表
# 格式："模式|类型|建议"
PATTERNS=(
    "std::fs::|std::fs 阻塞文件 IO|改用 tokio::fs"
    "std::thread::sleep|std::thread::sleep 阻塞睡眠|改用 tokio::time::sleep"
    "std::process::Command|std::process 阻塞进程|改用 tokio::process::Command"
    "std::net::TcpStream|std::net 阻塞 TCP|改用 tokio::net::TcpStream"
    "std::net::UdpSocket|std::net 阻塞 UDP|改用 tokio::net::UdpSocket"
    "std::net::TcpListener|std::net 阻塞 Listener|改用 tokio::net::TcpListener"
    "thread::sleep|thread::sleep 阻塞睡眠|改用 tokio::time::sleep"
    "blocking_lock|blocking_lock 阻塞锁|改用 async lock 或 spawn_blocking"
)

echo -e "${YELLOW}=== spawn_blocking 审计开始 ===${NC}"
echo "扫描目录: ${SCAN_DIR}"
echo ""

# 创建报告目录
mkdir -p "$(dirname "${REPORT_FILE}")"

# 报告头
{
    echo "# spawn_blocking 审计报告"
    echo ""
    echo "- **审计时间**: $(date '+%Y-%m-%d %H:%M:%S')"
    echo "- **扫描目录**: \`${SCAN_DIR}\`"
    echo ""
    echo "## 违规项列表"
    echo ""
} > "${REPORT_FILE}"

VIOLATION_COUNT=0

for pattern_entry in "${PATTERNS[@]}"; do
    IFS='|' read -r pattern type suggestion <<< "${pattern_entry}"

    # 使用 ripgrep 搜索（如果可用），否则回退到 grep
    if command -v rg &> /dev/null; then
        matches=$(rg -n --no-heading "${pattern}" "${SCAN_DIR}" --glob "*.rs" 2>/dev/null || true)
    else
        matches=$(grep -rn --include="*.rs" "${pattern}" "${SCAN_DIR}" 2>/dev/null || true)
    fi

    if [[ -n "${matches}" ]]; then
        echo -e "${RED}[违规] ${type}${NC}"
        echo "" >> "${REPORT_FILE}"
        echo "### ${type}" >> "${REPORT_FILE}"
        echo "" >> "${REPORT_FILE}"
        echo "| 文件:行号 | 代码 |" >> "${REPORT_FILE}"
        echo "|-----------|------|" >> "${REPORT_FILE}"

        while IFS= read -r line; do
            file_line=$(echo "${line}" | cut -d: -f1-2)
            code_content=$(echo "${line}" | cut -d: -f3- | sed 's/|/\\|/g' | head -c 120)
            echo -e "  ${file_line}: ${code_content}"
            echo "| \`${file_line}\` | \`${code_content}\` |" >> "${REPORT_FILE}"
            ((VIOLATION_COUNT++))
        done <<< "${matches}"

        echo -e "  建议: ${suggestion}"
        echo "" >> "${REPORT_FILE}"
        echo "**建议**: ${suggestion}" >> "${REPORT_FILE}"
        echo ""
    fi
done

# 额外检查：async fn 内的阻塞调用（启发式检测）
echo ""
echo -e "${YELLOW}=== async fn 内阻塞调用启发式检测 ===${NC}"
echo ""

# 检测 async fn 体内是否直接调用了已知阻塞模式
ASYNC_PATTERNS=(
    "std::fs::read"
    "std::fs::write"
    "std::fs::File::open"
    "std::fs::File::create"
    "std::thread::sleep"
    "std::process::Command"
)

ASYNC_VIOLATION_COUNT=0

{
    echo "## async fn 内阻塞调用（启发式）"
    echo ""
    echo "> 注意：此检测为启发式，可能存在误报。需人工确认调用是否在 async fn 体内。"
    echo ""
} >> "${REPORT_FILE}"

for pattern in "${ASYNC_PATTERNS[@]}"; do
    if command -v rg &> /dev/null; then
        matches=$(rg -n --no-heading "${pattern}" "${SCAN_DIR}" --glob "*.rs" 2>/dev/null || true)
    else
        matches=$(grep -rn --include="*.rs" "${pattern}" "${SCAN_DIR}" 2>/dev/null || true)
    fi

    if [[ -n "${matches}" ]]; then
        while IFS= read -r line; do
            file_line=$(echo "${line}" | cut -d: -f1-2)
            code_content=$(echo "${line}" | cut -d: -f3- | sed 's/|/\\|/g' | head -c 120)
            echo -e "${YELLOW}  [需确认] ${file_line}: ${code_content}${NC}"
            echo "- \`${file_line}\`: \`${code_content}\`" >> "${REPORT_FILE}"
            ((ASYNC_VIOLATION_COUNT++))
        done <<< "${matches}"
    fi
done

# 汇总
echo ""
echo -e "${YELLOW}=== 审计汇总 ===${NC}"
echo "直接违规项: ${VIOLATION_COUNT}"
echo "async fn 内待确认项: ${ASYNC_VIOLATION_COUNT}"
echo "报告已保存至: ${REPORT_FILE}"

{
    echo ""
    echo "## 汇总"
    echo ""
    echo "- **直接违规项**: ${VIOLATION_COUNT}"
    echo "- **async fn 内待确认项**: ${ASYNC_VIOLATION_COUNT}"
    echo ""
    if [[ ${VIOLATION_COUNT} -eq 0 && ${ASYNC_VIOLATION_COUNT} -eq 0 ]]; then
        echo "## 结论"
        echo ""
        echo "✅ 未检测到阻塞调用违规项。workspace 异步代码符合规范。"
    else
        echo "## 结论"
        echo ""
        echo "⚠️ 检测到 ${VIOLATION_COUNT} 个直接违规 + ${ASYNC_VIOLATION_COUNT} 个待确认项。"
        echo "请逐项审查并修复：将阻塞调用改用 \`tokio::task::spawn_blocking\` 或异步等价 API。"
    fi
} >> "${REPORT_FILE}"

if [[ ${VIOLATION_COUNT} -eq 0 && ${ASYNC_VIOLATION_COUNT} -eq 0 ]]; then
    echo -e "${GREEN}✅ 审计通过，无违规项${NC}"
    exit 0
else
    echo -e "${RED}⚠️ 发现 ${VIOLATION_COUNT} 个违规 + ${ASYNC_VIOLATION_COUNT} 个待确认项${NC}"
    exit 1
fi