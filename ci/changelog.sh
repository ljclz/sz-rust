#!/usr/bin/env bash
#
# SzRSQL CHANGELOG 自动生成脚本
#
# 用法：
#   ./ci/changelog.sh                  # 生成当前未发布版本的变更日志
#   ./ci/changelog.sh v0.1.0           # 生成 v0.1.0 到 HEAD 的变更日志
#   ./ci/changelog.sh v0.1.0 v0.2.0    # 生成 v0.1.0 到 v0.2.0 的变更日志
#
# 输出：Markdown 格式的变更日志片段，可直接粘贴到 CHANGELOG.md
#
# 依赖：git、bash 4+
#
# 规则：
#   - commit message 前缀分类：
#     feat:     → Added
#     fix:      → Fixed
#     perf:     → Changed
#     refactor: → Changed
#     docs:     → Documentation（不输出，文档变更不记录在 CHANGELOG）
#     test:     → Internal（不输出）
#     chore:    → Internal（不输出）
#     ci:       → Internal（不输出）
#   - BREAKING CHANGE: → 在各分类下标记 **BREAKING**
#   - 跳过 merge commit（Merge pull request #...）

set -euo pipefail

# 颜色输出（仅在终端启用）
if [[ -t 1 ]]; then
    YELLOW='\033[0;33m'
    GREEN='\033[0;32m'
    RED='\033[0;31m'
    NC='\033[0m'
else
    YELLOW=''
    GREEN=''
    RED=''
    NC=''
fi

# 解析参数
if [[ $# -eq 0 ]]; then
    # 无参数：获取最新 tag 到 HEAD 的变更
    LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || echo "")
    if [[ -z "$LAST_TAG" ]]; then
        RANGE="HEAD"
        VERSION="Unreleased"
    else
        RANGE="${LAST_TAG}..HEAD"
        VERSION="Unreleased"
    fi
elif [[ $# -eq 1 ]]; then
    LAST_TAG="$1"
    RANGE="${LAST_TAG}..HEAD"
    VERSION="Unreleased"
else
    FROM_TAG="$1"
    TO_TAG="$2"
    RANGE="${FROM_TAG}..${TO_TAG}"
    VERSION="${TO_TAG}"
fi

echo -e "${YELLOW}Generating changelog for range: ${RANGE}${NC}" >&2

# 获取 commit 列表（按时间倒序）
COMMITS=$(git log --pretty=format:"%H|%s|%an|%ad" --date=short --no-merges $RANGE 2>/dev/null || echo "")

if [[ -z "$COMMITS" ]]; then
    echo -e "${RED}No commits found in range ${RANGE}${NC}" >&2
    exit 1
fi

# 分类收集 commit
declare -a ADDED_COMMITS
declare -a FIXED_COMMITS
declare -a CHANGED_COMMITS
declare -a REMOVED_COMMITS
declare -a SECURITY_COMMITS
declare -a BREAKING_COMMITS

while IFS='|' read -r hash subject author date; do
    # 跳过空 commit
    [[ -z "$hash" ]] && continue

    # 提取 conventional commit 类型
    type=$(echo "$subject" | grep -oE '^[a-z]+(\([^)]+\))?!?:' | sed 's/!.*//' | sed 's/^(.*//' | sed 's/:.*//')

    # 检测 BREAKING CHANGE
    is_breaking=false
    if echo "$subject" | grep -qE '^[a-z]+(\([^)]+\))?!:'; then
        is_breaking=true
    fi
    # 也检查 commit body 中的 BREAKING CHANGE:
    body_breaking=$(git log -1 --pretty=format:"%b" "$hash" | grep -i "^BREAKING CHANGE:" || true)
    if [[ -n "$body_breaking" ]]; then
        is_breaking=true
    fi

    # 清理 subject（去掉前缀）
    clean_subject=$(echo "$subject" | sed -E 's/^[a-z]+(\([^)]+\))?!?: //')

    # 格式化为 changelog 条目
    entry="- ${clean_subject} (${hash:0:7})"

    if $is_breaking; then
        BREAKING_COMMITS+=("$entry")
    fi

    case "$type" in
        feat)
            ADDED_COMMITS+=("$entry")
            ;;
        fix)
            FIXED_COMMITS+=("$entry")
            ;;
        perf|refactor)
            CHANGED_COMMITS+=("$entry")
            ;;
        remove|revert)
            REMOVED_COMMITS+=("$entry")
            ;;
        security)
            SECURITY_COMMITS+=("$entry")
            ;;
        docs|test|chore|ci|build|style)
            # 不记录在 CHANGELOG
            ;;
        *)
            # 未识别的类型，归到 Changed
            CHANGED_COMMITS+=("- ${subject} (${hash:0:7})")
            ;;
    esac
done <<< "$COMMITS"

# 输出 Markdown
echo ""
echo "## [${VERSION}]"
echo ""

# BREAKING CHANGES 优先显示
if [[ ${#BREAKING_COMMITS[@]} -gt 0 ]]; then
    echo "### ⚠️ BREAKING CHANGES"
    echo ""
    for commit in "${BREAKING_COMMITS[@]}"; do
        echo "$commit"
    done
    echo ""
fi

if [[ ${#ADDED_COMMITS[@]} -gt 0 ]]; then
    echo "### Added"
    echo ""
    for commit in "${ADDED_COMMITS[@]}"; do
        echo "$commit"
    done
    echo ""
fi

if [[ ${#CHANGED_COMMITS[@]} -gt 0 ]]; then
    echo "### Changed"
    echo ""
    for commit in "${CHANGED_COMMITS[@]}"; do
        echo "$commit"
    done
    echo ""
fi

if [[ ${#FIXED_COMMITS[@]} -gt 0 ]]; then
    echo "### Fixed"
    echo ""
    for commit in "${FIXED_COMMITS[@]}"; do
        echo "$commit"
    done
    echo ""
fi

if [[ ${#REMOVED_COMMITS[@]} -gt 0 ]]; then
    echo "### Removed"
    echo ""
    for commit in "${REMOVED_COMMITS[@]}"; do
        echo "$commit"
    done
    echo ""
fi

if [[ ${#SECURITY_COMMITS[@]} -gt 0 ]]; then
    echo "### Security"
    echo ""
    for commit in "${SECURITY_COMMITS[@]}"; do
        echo "$commit"
    done
    echo ""
fi

# 统计信息
TOTAL_COMMITS=$(echo "$COMMITS" | wc -l | tr -d ' ')
ADDED_COUNT=${#ADDED_COMMITS[@]}
FIXED_COUNT=${#FIXED_COMMITS[@]}
CHANGED_COUNT=${#CHANGED_COMMITS[@]}
REMOVED_COUNT=${#REMOVED_COMMITS[@]}
SECURITY_COUNT=${#SECURITY_COMMITS[@]}
BREAKING_COUNT=${#BREAKING_COMMITS[@]}

echo -e "${GREEN}Changelog generated:${NC}" >&2
echo -e "  Total commits:   ${TOTAL_COMMITS}" >&2
echo -e "  Added:           ${ADDED_COUNT}" >&2
echo -e "  Changed:         ${CHANGED_COUNT}" >&2
echo -e "  Fixed:           ${FIXED_COUNT}" >&2
echo -e "  Removed:         ${REMOVED_COUNT}" >&2
echo -e "  Security:        ${SECURITY_COUNT}" >&2
echo -e "  Breaking:        ${BREAKING_COUNT}" >&2
