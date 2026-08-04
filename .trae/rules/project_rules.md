---
alwaysApply: true
---

# sz-rust 项目铁律 —— 全栈 Rust 生态统一红线

> 适用范围：sz-rust 全 workspace（packages/sz-rust-core / sz-rust-sz300 / sz-rust-addons-* / sz-rust-cli / sz-rust-examples）。
> 本铁律仅覆盖 Web 框架层（framework）。database / orm 模块由各自项目的 Skills 负责。

## 🧠 内存与溢出（全局硬约束）

1. **整数溢出即 Panic**：所有算术运算默认 `overflow-checks = true`（已在 Cargo.toml 强制）。AI 若使用未检查的 `+` 或 `-` 处理外部输入，必须补充 `checked_*` 或 `saturating_*`。
2. **严禁裸 unwrap**：任何 `Result` 或 `Option` 解包必须使用 `?` + `anyhow::Context` 携带调用栈。仅允许在测试代码或启动阶段（如 `env::var` 必须存在）使用 `.expect("明确原因")`。
3. **unsafe 围栏**：`unsafe` 仅限 FFI 或极致性能热点（须有 `# Safety` 文档），应用层（路由、执行器、映射）零容忍。

## 🔥 异步运行时（全模块通用）

4. **禁止阻塞运行时**：全项目禁止 `std::thread::sleep` 和同步文件 IO，统一使用 `tokio::time::sleep` 和 `tokio::fs`。
5. **超时兜底强制**：任何外部 IO（HTTP 调用、DB 查询、连接池获取）必须包裹 `tokio::time::timeout`（默认 5s）。
6. **禁止持锁跨 .await**：任何 `MutexGuard` 不得在持有状态下穿越 `.await` 点。

## 🔐 安全与脱敏（统一规范）

7. **敏感字段编译期脱敏**：所有包含 `password`、`secret`、`token`、`api_key` 的结构体，必须使用 `#[serde(skip_serializing)]` 或自定义 `Debug`。日志中严禁明文暴露。
8. **路径归一化**：任何静态文件服务或路径解析，必须拦截 `..` 防止目录遍历。

## 📊 性能基线（跨模块统一）

9. **启动内存 < 30MB**：空载 RSS 不得超过 30MB。
10. **测试覆盖率 ≥ 85%**：核心模块行覆盖率必须达标，由 `cargo tarpaulin` 统一检查。

## 🚨 提交流程硬约束

11. **PR 必须附带 framework 模块的 Skill 检查记录**：修改 framework 触发对应 Skills。AI 不得跳过。
12. **人类审查保留地**：中间件核心（Framework）的代码必须标记 `@REVIEW_REQUIRED`。

## 🔍 审计合规（生死线）

13. **审计结论必须附带可验证的代码证据**：
    - ❌ 禁止：`已修复`、`应该没问题`、`参见其他文档`
    - ✅ 必须：`[src/middleware/auth.rs:127](file:///.../auth.rs#L127) 已修复，cargo test 输出：43 passed`
    - 每条结论必须有 `file:line` 证据，且该文件行必须真实存在
    - 修复后必须运行 `cargo test` 并附输出，禁止未验证即标记 ✅
    - 多项修复必须逐项验证，禁止批量声称"全部通过"
    - 违反本条视为审计无效，必须重新执行

## 📐 工程化规范合规（新增，v1.2 起强制执行）

> 以下规则确保 `docs/sz-rust-engineering-practices.md` 不得被绕过、偷懒或包庇。
> 违反任一条即视为本次变更无效，必须回滚后重新执行。

14. **ADR 强制写入**：任何引入新架构决策的变更（新模块 / 新 trait / 新 feature flag / unsafe_code 策略变更 / 并发原语变更）必须在合入前新建 ADR。
    - ❌ 禁止：代码已合入但 ADR 未写、ADR 缺少"Bug 定位提示"段、ADR 未标注相关代码行号
    - ✅ 必须：ADR 文件落地 `docs/adr/ADR-NNN-<title>.md`，索引 `docs/adr/README.md` 同步更新
    - ADR 编号严格递增，不复用已废弃编号

15. **五维审查强制记录**：任何涉及安全/并发/unsafe/公共 API 的变更，必须在 `docs/audit/` 下写入五维审查报告。
    - 报告必须覆盖正确性/可读性/架构/安全性/性能 5 个维度
    - 每个维度必须有 ✅/⚠️/❌ 结论 + 具体证据（file:line 或 cargo test 输出）
    - 有阻断项（❌）时禁止合入

16. **engineering-practices.md 同步更新**：任何修改 CI/CD、新增门禁、新增教训类别、变更 unsafe_code 策略的 PR，必须同步更新 `docs/sz-rust-engineering-practices.md` 的变更摘要。
    - ❌ 禁止：代码变更已合入但文档版本未更新
    - ✅ 必须：在文档末尾"vX.X 变更摘要"中新增条目，含日期和变更内容

17. **禁止包庇偷懒**：AI 在开发过程中发现需要执行上述任一步骤时，不得以"下次再补"、"这个不重要"、"太麻烦了"等理由跳过。
    - 若 AI 认为某步骤不适用，必须在提交前输出书面理由（写入 commit message 或 PR 描述）
    - 无理由跳过视为违规

18. **前期变更追溯**：P0/P1/P2 阶段的所有开发变更，若涉及上述 14-17 条规则但当时未执行，必须在本规则生效后立即补执行（补写 ADR、补写五维审查报告、补更新文档）。
    - 本规则生效前已合入但缺少 ADR 的架构决策：限期 3 次会话内补写
    - 本规则生效前已合入但缺少五维审查的安全/并发变更：限期 3 次会话内补写

## 🚫 违规处理

违反上述 13-18 条任一条的处理流程：
1. 立即回滚本次变更（`git revert`）
2. 补执行缺失步骤（ADR / 五维审查 / 文档更新）
3. 重新提交并附带合规证据
4. 同一会话内重复违规 2 次：终止当前会话，人工介入审查
