#!/usr/bin/env bash
# ADR 覆盖率检查脚本
#
# 用途：CI 中检查 ADR 覆盖率是否达标（≥ 0.15）
# 用法：./scripts/check-adr-coverage.sh
#
# 检查项：
# 1. ADR 数量与核心模块数量的比例 ≥ 0.15
# 2. 每个 ADR 包含必填字段（状态/日期/相关代码/背景/决策/后果/注意事项/Bug定位提示）
# 3. ADR 编号严格递增，无跳号

set -euo pipefail

ADR_DIR="docs/adr"
# 必填字段（缺失即 FAIL）
REQUIRED_FIELDS=("状态" "日期" "决策")
# 建议字段（缺失仅 WARN，不阻塞 CI）
SUGGESTED_FIELDS=("相关代码" "背景" "后果" "注意事项" "Bug 定位提示")
MIN_DENSITY=0.15

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

error_count=0
warning_count=0

echo "=========================================="
echo "ADR 覆盖率检查"
echo "=========================================="
echo ""

# 1. 检查 ADR 目录是否存在
if [ ! -d "$ADR_DIR" ]; then
    echo -e "${RED}[FAIL] ADR 目录不存在: $ADR_DIR${NC}"
    exit 1
fi

# 2. 统计 ADR 数量
adr_count=$(find "$ADR_DIR" -name "ADR-*.md" -o -name "*.md" ! -name "README.md" | wc -l | tr -d ' ')
echo "[INFO] ADR 数量: $adr_count"

if [ "$adr_count" -eq 0 ]; then
    echo -e "${RED}[FAIL] 未找到任何 ADR 文件${NC}"
    exit 1
fi

# 3. 统计核心模块数量
# 定义：sz-rust-core/src/ 下的顶层模块（.rs 文件 + 子目录），与 ADR README 的"模块"定义一致
core_src="packages/sz-rust-core/src"
file_count=$(find "$core_src" -maxdepth 1 -name "*.rs" | wc -l | tr -d ' ')
dir_count=$(find "$core_src" -maxdepth 1 -type d ! -name src | wc -l | tr -d ' ')
module_count=$((file_count + dir_count))
echo "[INFO] 核心模块数量: $module_count（$file_count 文件 + $dir_count 子目录）"

# 4. 计算 ADR 密度
# 密度 = ADR 数量 / 核心模块数量
# 目标 ≥ 0.15（即每 7 个模块至少有 1 个 ADR）
density=$(awk "BEGIN {printf \"%.3f\", $adr_count / $module_count}")
echo "[INFO] ADR 密度: $density（目标 ≥ $MIN_DENSITY）"

if awk "BEGIN {exit !($density < $MIN_DENSITY)}"; then
    echo -e "${RED}[FAIL] ADR 密度不足: $density < $MIN_DENSITY${NC}"
    echo "     建议: 为核心模块补充 ADR（路由/中间件/控制器/认证/缓存/配置等）"
    error_count=$((error_count + 1))
else
    echo -e "${GREEN}[PASS] ADR 密度达标${NC}"
fi

echo ""
echo "=========================================="
echo "ADR 格式检查"
echo "=========================================="
echo ""

# 5. 检查每个 ADR 的必填字段
for adr_file in "$ADR_DIR"/ADR-*.md "$ADR_DIR"/[0-9]*.md; do
    [ -f "$adr_file" ] || continue

    filename=$(basename "$adr_file")
    missing_required=()
    missing_suggested=()

    for field in "${REQUIRED_FIELDS[@]}"; do
        if ! grep -q "$field" "$adr_file"; then
            missing_required+=("$field")
        fi
    done

    for field in "${SUGGESTED_FIELDS[@]}"; do
        if ! grep -q "$field" "$adr_file"; then
            missing_suggested+=("$field")
        fi
    done

    if [ ${#missing_required[@]} -gt 0 ]; then
        echo -e "${RED}[FAIL] $filename 缺少必填字段: ${missing_required[*]}${NC}"
        error_count=$((error_count + 1))
    else
        echo -e "${GREEN}[PASS] $filename 必填字段完整${NC}"
    fi

    if [ ${#missing_suggested[@]} -gt 0 ]; then
        echo -e "${YELLOW}[WARN] $filename 缺少建议字段: ${missing_suggested[*]}${NC}"
        warning_count=$((warning_count + 1))
    fi

    # 检查是否包含"决策替代方案"段（建议有，非强制）
    # 兼容两种标题："决策替代方案" 或 "替代方案"
    if ! grep -qE "决策替代方案|^[#]+ *替代方案" "$adr_file"; then
        echo -e "${YELLOW}[WARN] $filename 缺少"决策替代方案"段（建议补充）${NC}"
        warning_count=$((warning_count + 1))
    fi
done

echo ""
echo "=========================================="
echo "ADR 编号连续性检查"
echo "=========================================="
echo ""

# 6. 检查 ADR 编号连续性
expected_num=1
for adr_file in $(find "$ADR_DIR" -name "*.md" ! -name "README.md" | sort); do
    filename=$(basename "$adr_file")
    # 提取编号（支持 ADR-NNN 或 NNN- 两种格式）
    num=$(echo "$filename" | grep -oE '[0-9]+' | head -1)

    if [ -z "$num" ]; then
        echo -e "${YELLOW}[WARN] $filename 无法提取编号，跳过${NC}"
        continue
    fi

    # 移除前导零
    num=$((10#$num))

    if [ "$num" -lt "$expected_num" ]; then
        # 允许已存在的旧 ADR（不报错）
        continue
    elif [ "$num" -gt "$expected_num" ]; then
        echo -e "${YELLOW}[WARN] ADR 编号跳号: 期望 $expected_num，实际 $num${NC}"
        warning_count=$((warning_count + 1))
    fi

    if [ "$num" -ge "$expected_num" ]; then
        expected_num=$((num + 1))
    fi
done

echo ""
echo "=========================================="
echo "检查汇总"
echo "=========================================="
echo ""
echo "ADR 数量: $adr_count"
echo "核心模块数量: $module_count"
echo "ADR 密度: $density（目标 ≥ $MIN_DENSITY）"
echo "错误数: $error_count"
echo "警告数: $warning_count"
echo ""

if [ "$error_count" -gt 0 ]; then
    echo -e "${RED}[FAIL] ADR 覆盖率检查未通过，共 $error_count 个错误${NC}"
    exit 1
else
    echo -e "${GREEN}[PASS] ADR 覆盖率检查通过${NC}"
    if [ "$warning_count" -gt 0 ]; then
        echo -e "${YELLOW}       存在 $warning_count 个警告，建议修复${NC}"
    fi
    exit 0
fi
