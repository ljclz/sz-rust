# sz-rust 项目 AI 工作指南

- 语言：Rust 2024 Edition
- 架构：Cargo Workspace
  - `packages/sz-rust-core`：Web 框架核心（路由、中间件、DI 容器、模板、缓存等）
  - `packages/sz-rust-sz300`：sz300 业务应用
  - `packages/sz-rust-addons-*`：插件包
  - `packages/sz-rust-cli`：命令行工具
  - `packages/sz-rust-examples`：示例

## 统一约束

- 所有 `async fn` 必须 `Send + 'static`。
- 禁止在任何 crate 中使用 `std::fs`，统一 `tokio::fs`。
- 敏感字段自动脱敏（`#[serde(skip_serializing)]`）。

## 关键约束

- 任何 `WHERE` 条件必须经过参数化绑定验证（由 sz-orm Skills 负责）。
- 默认禁止 `SELECT *`，必须显式列投影（防止字段变更导致崩溃）。
- N+1 检测：循环体内出现 `fetch_related` 视为违规。

## 触发门禁

在 Trae 对话中输入：`@sz-rust-qa 执行全量安全门禁`

## Skills 索引

| Skill | 触发场景 | 模式 |
|-------|---------|------|
| [sz-rust-framework-routing](.trae/skills/sz-rust-framework-routing/SKILL.md) | 修改 router | auto |
| [sz-rust-framework-middleware](.trae/skills/sz-rust-framework-middleware/SKILL.md) | 新增中间件 | auto |
| [sz-rust-framework-di](.trae/skills/sz-rust-framework-di/SKILL.md) | 修改 container | auto |
| [sz-rust-framework-config](.trae/skills/sz-rust-framework-config/SKILL.md) | 修改 config 或 static 服务 | manual |
| [sz-rust-framework-load](.trae/skills/sz-rust-framework-load/SKILL.md) | 性能压测 | auto |
| [sz-rust-test-coverage](.trae/skills/sz-rust-test-coverage/SKILL.md) | 业务代码变更 | auto |
| [sz-rust-performance-check](.trae/skills/sz-rust-performance-check/SKILL.md) | hot path 变更 | auto |
| [sz-rust-doc-check](.trae/skills/sz-rust-doc-check/SKILL.md) | pub API 变更 | auto |
| [sz-rust-migration](.trae/skills/sz-rust-migration/SKILL.md) | schema 变更 | manual |
| [sz-rust-deploy](.trae/skills/sz-rust-deploy/SKILL.md) | release 分支 | manual |
| [sz-rust-orm-query](.trae/skills/sz-rust-orm-query/SKILL.md) | repository 变更 | auto |
| [sz-rust-n-plus-one](.trae/skills/sz-rust-n-plus-one/SKILL.md) | 循环+DB 调用 | auto |
| [sz-rust-auth-guard](.trae/skills/sz-rust-auth-guard/SKILL.md) | 路由/中间件变更 | auto |
| [sz-rust-error-handling](.trae/skills/sz-rust-error-handling/SKILL.md) | Result 处理变更 | auto |
| [sz-rust-ci-cd](.trae/skills/sz-rust-ci-cd/SKILL.md) | CI/Docker 变更 | auto |

`preCommitCheck` 已配置前 4 个 auto Skill 为提交前必跑项。

## 强制铁律

见 [.trae/rules/project_rules.md](.trae/rules/project_rules.md)（`alwaysApply: true`，自动注入）。
共 23 条生死线，覆盖内存溢出、异步运行时、安全脱敏、性能基线、提交流程、文档同步、防幻影交付。

## 工作流约定

1. **任务开始前**：先读 `.trae/rules/project_rules.md`，明确 23 条生死线。
2. **代码修改后**：根据修改范围触发对应 Skill（router 改 → routing；中间件改 → middleware；container 改 → di；config/static 改 → config；性能验证 → load）。
3. **提交前**：`.trae/settings.json` 的 `preCommitCheck` 会自动触发 4 个 auto Skill，必须全部通过。
4. **结果反馈**：Skill 失败时，Agent 必须输出「变异点 / 混沌失败 / 死锁」的具体证据，禁止仅说"已修复"。
5. **完成声称证据链（防幻影交付，铁律 23）**：任何「已完成 / 已实现 / 测试通过」声称必须附三样证据，缺一不可：
   - **交付物路径**：crate/文件必须存在于本仓库（`packages/` 目录 + `Cargo.toml` members 为准），或显式标注所属仓库（如"企业版仓库"）
   - **验证命令真实输出**：如 `cargo test -p <crate>` 实际运行结果，禁止转述未运行的数字
   - **变更标识**：commit SHA / PR 编号
   - 禁止在代码未合入时更新 CHANGELOG / implementation-progress 状态为「已完成」；文档状态只能由合入动作在同一提交内触发
   - 提交前必须运行 `node scripts/audit/doc-code-consistency.js`（CI 门禁 19），文档声称的 `sz-rust-*` crate 必须存在于 workspace，否则构建失败

## 文档同步强制约束

> 对应 project_rules.md 规则 19-22，v1.3 起强制执行。

- **触发维度**：pub API 变更 / 性能基线变更 / CI-CD 配置变更 / 架构决策变更 / 安全机制变更 / 并发模型变更 / unsafe 策略变更 / 新增/删除 crate / 测试数量变更
- **受影响文档集**：README.md / README.en.md / CHANGELOG.md / roadmap.md / ADR / 审计报告 / engineering-practices.md / framework-comparison 报告 / project-assessment 报告 / 各 crate README
- **合入前义务**：同一提交中同步更新所有受影响文档，禁止"先提交后补文档"
- **数字可溯源性**：所有数字必须附来源标注（命令输出/测试报告/文件行号），禁止"估算""约""大概"
- **文档欠债追踪**：欠债记录到 `docs/audit/doc-debt.md`，限期 3 次会话内补齐

## 文档导航

- [使用指南](docs/sz-rust-skills使用指南.md) — Skill 安装、触发、自定义扩展
- [工程化实践](docs/sz-rust-engineering-practices.md) — CI/CD、Docker、K8s
- [ADR 集合](docs/adr/) — 20 个架构决策记录
- [审计报告](docs/audit/2026-07-25-综合深度审计报告.md) — v2 综合深度审计（89.4/100）
