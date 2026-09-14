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
| DB-2026-08-20-05 | tasks.md §3.5 | packages/sz-rust-k8s-operator/src/reconcile.rs | **reconcile 错误传播测试缺口**：`reconcile` 函数中 `Err(e) => return Err(e.into())` 分支未被测试覆盖。需引入 trait `ReconcileApi` 抽象 `Api<Deployment>` 与 `Api<Service>`，便于 mock 错误传播 | 2026-08-20 | 2026-09-03 | RESOLVED（2026-09-14 随 ADR-038 执行收口闭环：孤儿 crate 无消费者无生产入口，物理删除目录完成决策执行，测试缺口随 crate 移除消灭；22 测试保留 git 历史。选择移除而非补测试——ADR-038 已于 08-21 决策，补测试是对已决移除项的无谓投入） |
| DB-2026-09-03-01 | AI 评审 P1 采纳（2026-09-03 全量审查） | .cargo/mutants.toml + packages/sz-rust-core/src/pay.rs | **pay.rs 变异测试退出条件**：pay.rs（支付聚合层，1506 行 15 测试）因运行时长超预算被临时排除出变异测试（`**/pay.rs` glob）。资金逻辑变异体存活代价最高，禁止长期排除。退出条件：pay.rs 行覆盖 ≥75% 后必须从 mutants.toml exclude_globs 移除并重跑 `cargo mutants -p sz-rust-core` | 2026-09-03 | 2026-09-30 | RESOLVED（2026-09-14 闭环：P2 拆包后真实支付逻辑在 pay-facade/src/pay.rs = **95.98%** ≥ 75%（llvm-cov 基线）；`**/pay.rs` 移出 exclude_globs，`cargo mutants -p sz-rust-pay-facade` 实跑 **60 变异体 = 54 caught + 6 unviable，杀率 100%（0 存活）**；另清理仓库根目录 Windows 保留名垃圾文件 `nul`（曾致 cargo-mutants 源码复制 os error 87）） |
| DB-2026-09-03-02 | AI 评审 P3 裁定（2026-09-03 全量审查） | .trae/settings.json | **rust-analyzer targetDir 机器相关路径保留裁定**：AI 建议删除 `rust-analyzer.cargo.targetDir=F:\cargo-target-ra`（理由：Linux/macOS 无效）。不采纳——用户铁律严禁写 C 盘（RA 默认写 %LOCALAPPDATA%），相对路径会写 E 盘（曾爆盘），F 盘独立目录防 RA 与命令行指纹互踩。与 jobs-gauntlet.sh F 盘路径同一先例。跨平台团队扩大时再评估 | 2026-09-03 | - | RESOLVED（裁定：保留，理由已记录） |
| DB-2026-09-13-03 | 审查门禁修复提交（2026-09-13：admin.rs unwrap / user_service.rs password 脱敏 / multi_tenant_demo unwrap / pr-review.sh sz300 守卫） | CHANGELOG.md | **CHANGELOG 未同步**：本轮 4 项修复未写入 CHANGELOG [Unreleased]——工作区 CHANGELOG.md 存在并行开发（Oracle 接入）未提交改动，为避免混入他人 WIP 未触碰该文件。并行 WIP 合入做文档同步时补记 | 2026-09-13 | 2026-09-27 | RESOLVED（2026-09-14 闭环：Oracle WIP 确认为已完成工作（服务器 5 后端实测通过）后合入主线 4a63a5a，CHANGELOG 新增 [Unreleased] - 2026-09-14 段补记全部审查修复（Security/Fixed/Changed/Removed 四类），补记提交随本轮审查归档，SHA 见本表变更标识列同批 git log） |
| DB-2026-09-13-04 | AI 评审采纳项（2026-09-13 全量审查，deepseek-v4-flash） | 多文档（doc-code-consistency.js 检出的 129 处） | **幻影交付声称批量核验**：129 处需核验声称（主要为 sz-rust-k8s-operator 存在于 packages/ 但非 workspace member、sz-rust-operator/sdd-agent 历史引用的回退标注）。每次审查重复出现，需一次性分批裁定：企业版归属标注 `[enterprise]` / 历史回退标注确认 / 误导性引用删除 | 2026-09-13 | 2026-10-13 | RESOLVED（2026-09-13 当日闭环：① 核验企业版仓库实存 7 个 addons 插件 → 脚本新增 ENTERPRISE_DELIVERED_CRATES 类别（sz300+7 addons 引用降级 WARN），NON_CRATE_NAMES 补 sz-rust-skills/engineering-practices/sdd；② 3 个 CI workflow 显式修复（coverage.yml 移除 4 个幻影 -p、release.yml 发布目标 sz300-server→sz-rust-cli、docker job 按迁移先例禁用）；③ ADR-036 幻影路径移除 + README 双语包目录树企业版标注；doc-code 门禁 ERROR 129→0，exit 0） |

| DB-2026-08-16-05 | 外部 AI 全量代码审计（2026-08-16） | 多 crate 源码 | **R001-R007 安全风险清单**（审计结论 9/12 真实）：R001 mem_pool transmute 生命周期延长（227/234，arena 标准模式 + ADR-037 已收紧，不修）；R002 MCP SQL 表名/列名未验证（494/522/542，只构建不执行，低风险）；R003 from_utf8_unchecked（144，有安全论证）；R004 std::fs 铁律 4 违反（**已修**：sysinfo 用户修 + addons-loader registry/manifest 本轮修）；R005 admin 系统信息暴露（有 RoleGuard，不修）；R006 hot_reload unsafe（294，feature 默认关闭，有 Safety 文档）；R007 SIMD unsafe（x86_64 保证） | 2026-08-16 | 2026-08-19 | RESOLVED（2026-08-19 人工复核全部完成：R004 已修 `PROD_STD_FS=0`；R001 arena 标准模式 `mem_pool.rs:227` unsafe fn 契约 + ADR-037；R002 `validate_identifier` `mcp/lib.rs:74-98` 白名单校验阻止注入；R003 `from_utf8_unchecked` `mem_pool.rs:144` 输入为合法 &str 副本；R005 `admin_role_guard` `router.rs:173` 保护；R006 `hot_reload.rs:82` feature-gated 默认关闭；R007 `simd_safe.rs:8` 不暴露 unsafe 到公开 API） |
| DB-2026-08-16-06 | check-std-fs.py 门禁上线（2026-08-16） | infra-facade/mvc-facade/pdf/cli | **生产 std::fs 债务 30 处**（铁律 4，门禁全报阻塞）：infra-facade upload（image.rs 7/storage.rs 4/upload.rs 1，同步公共 API save，async 化=公共 API 变更）；mvc-facade view（layout 2/inheritance 1/view.rs 1，同步渲染链）；pdf（excel_import 2/csv_export 1，umya-spreadsheet 第三方库接口要求同步 File，**建议豁免**）；cli（make 4/cache 3/seed 2，同步命令行工具无 tokio runtime，**建议铁律 4 增 CLI 豁免条款**） | 2026-08-16 | 2026-08-19 | RESOLVED（2026-08-16 专项完成：infra-facade upload 全部 async 化（save/open/text/wrap_text/measure_text/move_to/hash 链/trait 5 引擎 set_upload_file+by_real/from_uploaded_file/from_real_path/md5/sha1/hash_name/compute_file_hash/debug_page），门禁 0 命中；pdf/cli/mvc view 用户裁定豁免（EXEMPT 列表，理由入脚本注释）；mvc view 引擎级 async 化单独排期） |

---

## 覆盖率豁免债务

> 此章节记录豁免覆盖率门槛的 crate 的未覆盖行清单与补齐计划。
> 对应 spec.md 5.6.1 规则 5（豁免审批规则）。

| crate_name | 未覆盖行数 | 豁免理由 | 补齐计划 | 预计完成时间 |
|------------|-----------|---------|---------|------------|
| sz-rust-examples | 1389/1418（2.05%） | 示例/演示代码（quick_start/crud_demo/addon-hot-reload），无生产入口，断言价值为零；随框架 API 演进需持续重写的演示材料 | 保持与框架 API 同步；不作为覆盖率目标 | 长期豁免（每 3 迭代复审） |

---

> 初始创建于 2026-08-09，对应 P1 任务 2.3（文档同步强制规则约束）。