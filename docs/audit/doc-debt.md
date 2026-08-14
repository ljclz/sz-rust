# 文档欠债清单

> 本文件追踪所有未同步更新的文档欠债。
> 对应 project_rules.md 规则 22，v1.3 起强制执行。

## 使用说明

1. **何时新增条目**：代码变更合入后发现相关文档未同步更新时，立即在此文件新增一行
2. **如何标记 RESOLVED**：欠债补齐后，将状态改为 `RESOLVED` 并附补齐日期
3. **OVERDUE 自动判定**：当前日期超过限期补齐时间且状态仍为 `PENDING` 时，状态自动判定为 `OVERDUE`

## 欠债清单

| 欠债 ID | 变更标识（commit SHA / PR 编号） | 受影响文档 | 欠债项描述 | 产生时间 | 限期补齐时间 | 状态 |
|---------|-------------------------------|-----------|-----------|---------|------------|------|
| DB-2026-08-13-01 | 审计报告 `2026-08-13-文档已实现但生产零调用审计报告.md` | CHANGELOG.md [Unreleased] / docs/2026-08-12-p1-p4-delivery-summary.md / docs/audit/2026-08-13-p012-gaps-complete-summary.md / docs/implementation-progress.md | 幻影交付：`sz-rust-marketplace`/`sz-rust-visual`/`sz-rust-sdd-agent`/`sz-rust-migration` 4 个 crate 声称「全部完成 + 741 tests」但代码不存在（或属企业版仓库），需核验后回退/标注 | 2026-08-13 | 2026-08-18 | PENDING |
| DB-2026-08-13-02 | 审计报告 `2026-08-13-文档已实现但生产零调用审计报告.md` | README.md 核心特性（13-38 行）与对标表（102-127 行） | 10 个 crate（tracing/pdf/operator/wasm/rag/workflow/ai-facade/capability/addons-loader/addons-*）与 20+ 模块（限流/熔断/SSE/SLO/视图/上传引擎等）声称「已落地」但生产路径零调用，需补充「生产接入状态」或降级表述 | 2026-08-13 | 2026-08-18 | PENDING |

---

> 初始创建于 2026-08-09，对应 P1 任务 2.3（文档同步强制规则约束）。