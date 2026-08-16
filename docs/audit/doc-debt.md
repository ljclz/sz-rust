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
| DB-2026-08-13-01 | 审计报告 `2026-08-13-文档已实现但生产零调用审计报告.md` | CHANGELOG.md [Unreleased] / docs/2026-08-12-p1-p4-delivery-summary.md / docs/audit/2026-08-13-p012-gaps-complete-summary.md / docs/implementation-progress.md | **幻影交付（已定性：纯虚构）**：`sz-rust-marketplace`/`sz-rust-visual`/`sz-rust-sdd-agent`/`sz-rust-migration` 4 个 crate 声称「全部完成 + 741 tests」。2026-08-14 核验：开源版与企业版仓库（`E:/vue/test/鲜视达/rust/sz-rust-enterprise`，gitlab.com/sz-rust-enterprise）git 历史中均**从未存在**（0 次提交），企业版 packages 仅 7 个 addons 插件。CHANGELOG [Unreleased] 2026-08-13 条目需回退为未完成 | 2026-08-13 | 2026-08-18 | RESOLVED（定性完成：纯虚构，需回退 CHANGELOG 条目——见下一条跟踪） |
| DB-2026-08-13-02 | 审计报告 `2026-08-13-文档已实现但生产零调用审计报告.md` | README.md 核心特性（13-38 行）与对标表（102-127 行） | 10 个 crate（tracing/pdf/operator/wasm/rag/workflow/ai-facade/capability/addons-loader/addons-*）与 20+ 模块（限流/熔断/SSE/SLO/视图/上传引擎等）声称「已落地」但生产路径零调用，需补充「生产接入状态」或降级表述 | 2026-08-13 | 2026-08-18 | RESOLVED（2026-08-14：README.md + README.en.md 核心特性区已为所有条目补充生产接入状态标注，附 file:line 调用点证据；审计报告新增「2026-08-14 状态更新」章节标注 d3c831f 后已接入/仍零调用清单） |
| DB-2026-08-14-03 | d3c831f（sz-orm 3.5.0 升级遗留） | 各 crate 源码 | **预存 clippy 债务**：`cargo clippy --workspace --all-targets -D warnings` 有 93 处错误（security_headers 6 / mcp 2 / sso_bench 2 / core plugin 44 / workflow 14 / cli 12 / sz300 4 / addons 6 / rag 3 / ai-facade 17 等），含 deprecated `Query` 迁移、`impl can be derived`、missing_docs、`field assignment outside initializer` 等。与 2026-08-14 空洞测试提交无关（文件 hash 与 d3c831f 一致），pre-commit 钩子 3/3 clippy 门禁因此失败 | 2026-08-14 | 2026-08-14 | RESOLVED（881e1a1 全部修复，clippy 0 error） |

| DB-2026-08-16-04 | pr-review 全量门禁上线（fbdeed8 前脚本 / 本轮 15 项门禁） | 各 crate 源码 | **生产代码裸 unwrap 债务（铁律 2）**：`scripts/check-unwrap.py` 检出 AUTHORITATIVE_PROD_UNWRAP=51 处（此前 pre-commit 钩子仅警告不阻塞；全量门禁纳入后每次 `/sz-rust-review` 以 high 阻塞）。既有债务（非本次变更引入），需专项清偿（参照 93 处 clippy 债务模式） | 2026-08-16 | 2026-08-19 | RESOLVED（2026-08-16 专项清偿：51 处全部修复，AUTHORITATIVE_PROD_UNWRAP=0；含 lock 中毒 13 处→unwrap_or_else(into_inner)、启动阶段 bind/serve→expect、测试辅助与生产必有值→expect；另修 perf-compare 11 处；附带修复 core container 测试预存漂移 8802→3306） |

| DB-2026-08-16-05 | 外部 AI 全量代码审计（2026-08-16） | 多 crate 源码 | **R001-R007 安全风险清单**（审计结论 9/12 真实）：R001 mem_pool transmute 生命周期延长（227/234，arena 标准模式 + ADR-037 已收紧，不修）；R002 MCP SQL 表名/列名未验证（494/522/542，只构建不执行，低风险）；R003 from_utf8_unchecked（144，有安全论证）；R004 std::fs 铁律 4 违反（**已修**：sysinfo 用户修 + addons-loader registry/manifest 本轮修）；R005 admin 系统信息暴露（有 RoleGuard，不修）；R006 hot_reload unsafe（294，feature 默认关闭，有 Safety 文档）；R007 SIMD unsafe（x86_64 保证） | 2026-08-16 | 2026-08-19 | PENDING（R004 已修；其余为论证/保护项，待人工复核） |
| DB-2026-08-16-06 | check-std-fs.py 门禁上线（2026-08-16） | infra-facade/mvc-facade/pdf/cli | **生产 std::fs 债务 30 处**（铁律 4，门禁全报阻塞）：infra-facade upload（image.rs 7/storage.rs 4/upload.rs 1，同步公共 API save，async 化=公共 API 变更）；mvc-facade view（layout 2/inheritance 1/view.rs 1，同步渲染链）；pdf（excel_import 2/csv_export 1，umya-spreadsheet 第三方库接口要求同步 File，**建议豁免**）；cli（make 4/cache 3/seed 2，同步命令行工具无 tokio runtime，**建议铁律 4 增 CLI 豁免条款**） | 2026-08-16 | 2026-08-19 | PENDING（已修 addons-loader 生产路径；infra/mvc 需专项 async 化；pdf/cli 待豁免裁定） |

---

> 初始创建于 2026-08-09，对应 P1 任务 2.3（文档同步强制规则约束）。