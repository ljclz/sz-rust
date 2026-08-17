# 文档重组变更记录

> 重组日期：2026-08-09
> 重组范围：docs/audit/、docs/benchmarks/、docs/ 根目录
> 对应 spec：`.codeartsdoer/specs/docs_restructure_followup/spec.md`
> 对应 design：`.codeartsdoer/specs/docs_restructure_followup/design.md`

---

## 操作统计

| 操作类型 | 数量 |
|---------|------|
| 移动（git mv / mv） | 30 份 |
| 合并 | 2 份 → 1 份 |
| 删除 | 2 份 |
| 新建 | 2 份（benchmarks/README.md + 本文件） |
| 交叉引用更新 | 14 处（8 个文件） |

---

## 变更明细

### 模块 A：归档目录结构统一化

| 序号 | 操作 | 原路径 | 新路径 | 原因 |
|------|------|--------|--------|------|
| 1 | move | docs/audit/archive/2026-07-22-初始审计.md | docs/audit/archive/2026-07/2026-07-22-初始审计.md | 07月文档归档 |
| 2 | move | docs/audit/archive/2026-07-23-框架审计清单.md | docs/audit/archive/2026-07/ | 07月文档归档 |
| 3 | move | docs/audit/archive/2026-07-23-审计验证报告.md | docs/audit/archive/2026-07/ | 07月文档归档 |
| 4 | move | docs/audit/archive/2026-07-23-ThinkPHP性能对比.md | docs/audit/archive/2026-07/ | 07月文档归档 |
| 5-15 | move | （其余 11 份 07 月文档） | docs/audit/archive/2026-07/ | 07月文档归档 |
| 16-24 | move | （9 份 08 月文档） | docs/audit/archive/2026-08/ | 08月文档归档 |

### 模块 B：过时文档清理与归档

| 序号 | 操作 | 原路径 | 新路径 | 原因 |
|------|------|--------|--------|------|
| 25 | move | docs/audit/合规债务总账-v0.2.1至当前.md | docs/audit/archive/2026-08/ | 17/17 合规债务已清偿 |
| 26 | move | docs/roadmap-implementation.md | docs/audit/archive/2026-08/ | 部分过时，任务状态需核实 |
| 27 | move | docs/roadmap.md | docs/audit/archive/2026-08/ | P0-P4 已完成，仅 P2-1 待上游配合 |

### 模块 C：性能报告统一归档

| 序号 | 操作 | 原路径 | 新路径 | 原因 |
|------|------|--------|--------|------|
| 28 | move | docs/benchmark-report.md | docs/benchmarks/benchmark-report.md | 性能报告统一归档 |
| 29 | move | docs/bench-coverage-matrix.md | docs/benchmarks/bench-coverage-matrix.md | 性能报告统一归档 |
| 30 | move | docs/audit/archive/2026-08/2026-08-07-框架性能对比报告.md | docs/benchmarks/2026-08-07-framework-comparison.md | 性能报告统一归档 |

### 模块 D：同日评估报告合并

| 序号 | 操作 | 原路径 | 新路径 | 原因 |
|------|------|--------|--------|------|
| 31 | merge | docs/audit/2026-08-09-项目深度评估报告.md + docs/audit/2026-08-09-框架性能对比报告.md | docs/audit/archive/2026-08/2026-08-09-项目深度评估与框架对比报告.md | 同日报告合并 |
| 32 | delete | docs/audit/2026-08-09-项目深度评估报告.md | — | 合并后删除原文档 |
| 33 | delete | docs/audit/2026-08-09-框架性能对比报告.md | — | 合并后删除原文档 |

### 模块 E：劣势清单修正

已解决劣势（3 项）从劣势部分迁移到"已解决历史问题"章节：
1. 文档国际化不足 → 已解决（2026-08-09）
2. 性能基准未全面对比 → 已解决（2026-08-09）
3. Redis 存储后端不完整 → 已解决（2026-08-09）

### 模块 F：多维度对比补全

补全 4 个维度对比表（4.3 生态/4.4 易用性/4.5 安全性/4.6 综合评估），数据来源：
- crates.io API（采集日期 2026-08-09）
- GitHub API（采集日期 2026-08-09）
- 代码库统计 + 官方文档

### 模块 G：交叉引用更新

| 文件 | 行号 | 替换内容 |
|------|------|---------|
| docs/spec/production-validation/spec.md | :5 | 旧评估报告路径 → 新合并报告路径 |
| README.md | :183-184 | 两个旧报告链接 → 一个合并报告链接 |
| README.en.md | :179-180 | 两个旧报告链接 → 一个合并报告链接 |
| CHANGELOG.md | :22 | 旧对比报告路径 → 新合并报告路径 |
| docs/audit/README.md | :13-15 | 当前权威报告更新 |
| docs/audit/archive/2026-08/2026-08-06-项目综合状态报告.md | :37,64 | docs/roadmap.md → 新归档路径 |
| docs/sz-rust-engineering-practices.md | :551 | docs/benchmark-report.md → docs/benchmarks/benchmark-report.md |
| docs/audit/archive/2026-08/2026-08-03-基于实测的能力评测报告.md | :52 | 同上 |
| docs/audit/archive/2026-08/2026-08-03-P2拆包开发过程审查与优化报告.md | :227 | 同上 |
| .trae/rules/project_rules.md | :81 | 受影响文档清单更新 |

---

## 待人工确认清单

以下 3 份非日期文档无法自动识别归档月份，需人工确认：

| 文件 | 当前位置 | 建议 |
|------|---------|------|
| capability-v0.3.0.md | docs/audit/archive/ | 归档到 2026-08/（v0.3.0 能力评估） |
| sz-rust项目状态报告.md | docs/audit/archive/ | 归档到 2026-08/（项目状态报告） |
| 功能基线清单.md | docs/audit/archive/ | 归档到 2026-07/（功能基线，早期文档） |

---

## 归档规则

1. **按月归档**：`docs/audit/archive/{YYYY-MM}/` 月目录，所有历史审计报告按文档日期归档
2. **月目录命名**：必须匹配 `^\d{4}-\d{2}$` 正则（如 `2026-07`、`2026-08`）
3. **archive 根目录**：仅存放月目录，禁止散落文档（非日期文档除外，待人工确认）
4. **audit 根目录**：仅保留当前活跃文档（README.md、doc-debt.md、restructure-changelog-*.md）与 archive/ 子目录
5. **性能报告**：统一归档到 `docs/benchmarks/`，禁止散落在 docs/ 根目录或 docs/audit/
6. **过时文档**：归档时在文件头部注入归档说明（`> ⚠️ 已归档，原因：XXX，归档日期：YYYY-MM-DD`）