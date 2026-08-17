# PR 审查报告（2026-08-17，branch: main，range: HEAD~6..HEAD）

> 审查时点: `HEAD @ f426358`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 1 medium / 1 low）

- [medium] `diff` **whitespace-error**: 空白/冲突标记错误: scripts/collect-baseline.ps1:46: trailing whitespace. +         
- [low] `gate` **assertion-value**:   [WARN] packages/sz-rust-cache-facade/src/lib.rs:4775 测试 test_tag_clear_empty_no_error() 无断言宏但有 1 处 u


## 补充信息

## 变更集
```
 .cargo/config.toml                                 |    7 +
 .github/workflows/ci.yml                           |   73 +-
 .github/workflows/coverage.yml                     |  241 +++-
 Cargo.lock                                         |    5 +
 docs/audit/2026-08-17-pr-review-main.md            |  228 +++
 docs/audit/coverage-exemption.md                   |   16 +
 docs/audit/coverage-priority.json                  |  328 +++++
 docs/audit/doc-debt.md                             |   11 +
 docs/audit/events.jsonl                            |    5 +
 ...211\247\350\241\214\346\214\207\345\215\227.md" |   22 +
 packages/sz-rust-addons-cms/Cargo.toml             |    5 +
 packages/sz-rust-addons-cms/src/capability/mod.rs  |  261 ++++
 .../sz-rust-addons-cms/src/controller/article.rs   |  266 ++++
 .../sz-rust-addons-cms/src/controller/category.rs  |  132 ++
 packages/sz-rust-addons-cms/src/controller/tag.rs  |   88 ++
 packages/sz-rust-addons-cms/src/lib.rs             |  304 +++-
 packages/sz-rust-addons-cms/src/model/article.rs   |   52 +
 packages/sz-rust-addons-cms/src/model/category.rs  |   43 +
 packages/sz-rust-addons-cms/src/model/tag.rs       |   31 +
 packages/sz-rust-addons-crm/tests/coverage_test.rs | 1048 ++++++++++++++
 packages/sz-rust-addons-ecommerce/Cargo.toml       |    4 +
 .../sz-rust-addons-ecommerce/src/capability/mod.rs |  196 +++
 .../tests/integration_test.rs                      |  226 +++
 packages/sz-rust-ai-facade/src/llm/claude.rs       |    2 +-
 packages/sz-rust-ai-facade/src/llm/gemini.rs       |    2 +-
 packages/sz-rust-ai-facade/src/llm/openai.rs       |    2 +-
 .../sz-rust-ai-facade/tests/agent_engine_test.rs   |  262 ++++
 .../sz-rust-ai-facade/tests/agent_trace_test.rs    |   81 ++
 packages/sz-rust-ai-facade/tests/audit_test.rs     |   99 ++
 packages/sz-rust-ai-facade/tests/citation_test.rs  |   30 +
 .../tests/claude_provider_test.rs                  |  253 ++++
 .../sz-rust-ai-facade/tests/facade_error_test.rs   |   89 ++
 .../tests/gemini_provider_test.rs                  |  278 ++++
 .../sz-rust-ai-facade/tests/llm_fixture_test.rs    |  186 +++
 .../tests/local_embedding_test.rs                  |   81 ++
 packages/sz-rust-ai-facade/tests/metrics_test.rs   |   80 ++
 .../tests/openai_embedding_test.rs                 |   38 +
 .../tests/openai_provider_test.rs                  |  334 +++++
 packages/sz-rust-ai-facade/tests/real_api_test.rs  |  170 +++
 .../sz-rust-ai-facade/tests/sse_adapter_test.rs    |  105 ++
 packages/sz-rust-cli/src/cli.rs                    |  198 +++
 packages/sz-rust-cli/src/cmd/cache.rs              |   46 +
 packages/sz-rust-cli/src/cmd/make.rs               |  320 ++++-
 packages/sz-rust-cli/src/cmd/plugin.rs             |  134 ++
 packages/sz-rust-cli/src/cmd/scheduler.rs          |   50 +
 packages/sz-rust-cli/src/console.rs                |   25 +
 packages/sz-rust-cli/src/context_builder.rs        |   98 ++
 packages/sz-rust-cli/src/safety_validator.rs       |   87 ++
 packages/sz-rust-cli/src/template_engine.rs        |  109 ++
 .../tests/coverage_tests.rs                        | 1449 ++++++++++++++++++++
 packages/sz-rust-mcp/src/lib.rs                    |  461 +++++++
 packages/sz-rust-mcp/tests/tool_test.rs            |  332 +++++
 .../src/admin/sysinfo_collector.rs                 |   35 +-
 .../sz-rust-orm-facade/src/data_scope/cache.rs     |   13 +
 .../sz-rust-orm-facade/src/data_scope/custom.rs    |   17 +
 .../sz-rust-orm-facade/src/data_scope/evaluator.rs |   10 +
 packages/sz-rust-orm-facade/src/data_scope/ext.rs  |   85 ++
 .../sz-rust-orm-facade/src/data_scope/metrics.rs   |   13 +
 .../src/data_scope/modes/custom.rs                 |   28 +
 .../src/data_scope/modes/dept_and_sub.rs           |   13 +
 packages/sz-rust-orm-facade/src/data_scope/rule.rs |    9 +
 packages/sz-rust-orm-facade/src/jobs.rs            |  243 ++++
 packages/sz-rust-orm-facade/src/pool_scaler.rs     |   19 +
 packages/sz-rust-orm-facade/src/query_cache.rs     |   99 ++
 packages/sz-rust-rag/src/capability.rs             |   33 +
 packages/sz-rust-rag/src/chunking.rs               |   30 +
 packages/sz-rust-rag/src/config.rs                 |   51 +
 packages/sz-rust-rag/src/corpus.rs                 |   46 +
 packages/sz-rust-rag/src/error.rs                  |   62 +
 packages/sz-rust-rag/src/redact.rs                 |   27 +
 packages/sz-rust-rag/src/rule.rs                   |   57 +
 packages/sz-rust-rag/src/search.rs                 |  214 +++
 packages/sz-rust-rag/src/store.rs                  |   41 +
 packages/sz-rust-rag/src/template.rs               |   62 +
 packages/sz-rust-rag/src/term.rs                   |   67 +
 packages/sz-rust-rag/src/vectorize.rs              |  253 ++++
 packages/sz-rust-rag/src/warning.rs                |   24 +
 packages/sz-rust-router-facade/src/simd_str.rs     |   16 +
 packages/sz-rust-sz300/src/config.rs               |  620 +++++++++
 packages/sz-rust-sz300/src/controllers/admin.rs    |    2 +-
 packages/sz-rust-sz300/src/controllers/ai.rs       |  140 ++
 packages/sz-rust-sz300/src/controllers/auth.rs     |  302 ++++
 .../sz-rust-sz300/src/controllers/capabilities.rs  |   73 +
 packages/sz-rust-sz300/src/controllers/device.rs   |  388 ++++++
 packages/sz-rust-sz300/src/controllers/file.rs     |  119 ++
 .../sz-rust-sz300/src/controllers/file_serve.rs    |   47 +
 packages/sz-rust-sz300/src/controllers/health.rs   |  110 ++
 packages/sz-rust-sz300/src/controllers/merchant.rs |  285 ++++
 packages/sz-rust-sz300/src/controllers/order.rs    |  197 +++
 packages/sz-rust-sz300/src/controllers/product.rs  |  323 +++++
 packages/sz-rust-sz300/src/controllers/view.rs     |   58 +
 packages/sz-rust-sz300/src/db.rs                   |   34 +
 .../src/middleware/auth_middleware.rs              |  127 ++
 .../sz-rust-sz300/src/middleware/metrics_auth.rs   |  117 ++
 packages/sz-rust-sz300/src/models/ai_category.rs   |  133 ++
 packages/sz-rust-sz300/src/models/category.rs      |  120 ++
 packages/sz-rust-sz300/src/models/device.rs        |  162 +++
 packages/sz-rust-sz300/src/models/market.rs        |  139 ++
 packages/sz-rust-sz300/src/models/merchant.rs      |  152 ++
 packages/sz-rust-sz300/src/models/merchant_user.rs |  141 ++
 packages/sz-rust-sz300/src/models/operate_log.rs   |  140 ++
 packages/sz-rust-sz300/src/models/order.rs         |  160 +++
 packages/sz-rust-sz300/src/models/order_item.rs    |  134 ++
 packages/sz-rust-sz300/src/models/ota_version.rs   |  148 ++
 packages/sz-rust-sz300/src/models/product.rs       |  151 ++
 packages/sz-rust-sz300/src/models/settlement.rs    |  144 ++
 packages/sz-rust-sz300/src/models/system_config.rs |  121 ++
 packages/sz-rust-sz300/src/openapi.rs              |   86 ++
 packages/sz-rust-sz300/src/router.rs               |   58 +
 .../sz-rust-sz300/src/services/auth_service.rs     |  112 ++
 .../sz-rust-sz300/src/services/device_service.rs   |   59 +
 .../sz-rust-sz300/src/services/file_service.rs     |   91 ++
 .../sz-rust-sz300/src/services/health_service.rs   |   14 +
 .../sz-rust-sz300/src/services/merchant_service.rs |   61 +
 packages/sz-rust-sz300/src/services/mod.rs         |  120 ++
 .../sz-rust-sz300/src/services/mqtt_listener.rs    |   63 +
 .../sz-rust-sz300/src/services/mqtt_service.rs     |  140 ++
 .../sz-rust-sz300/src/services/order_service.rs    |   47 +
 .../sz-rust-sz300/src/services/product_service.rs  |   67 +
 packages/sz-rust-sz300/src/state.rs                |   41 +
 packages/sz-rust-workflow/src/definition/parser.rs |    2 +-
 .../sz-rust-workflow/src/repository/in_memory.rs   |    2 +-
 scripts/audit/cobertura-merger.js                  |   87 ++
 scripts/audit/coverage-gap-locator.js              |  149 ++
 scripts/audit/per-crate-coverage.js                |  127 ++
 scripts/audit/pr-review.sh                         |    7 +-
 scripts/collect-baseline.ps1                       |   78 ++
 scripts/coverage-local.sh                          |   77 ++
 scripts/summarize-baseline.ps1                     |   23 +
 tarpaulin.toml                                     |   15 +
 130 files changed, 16763 insertions(+), 110 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）



<![](file://c:\Users\51099\.cursor\mcp\oswatch\logs\20260127_071700.png)><![](file://c:\Users\51099\.cursor\mcp\oswatch\logs\20260127_071700.png)## PR 评审报告：覆盖率基础设施重构 (CI-001~007)

### 1. 潜在问题清单

| 优先级 | 类别 | 问题描述 | 涉及文件 |
| :--- | :--- | :--- | :--- |
| **P0** | **可靠性** | **自定义 XML 合并脚本风险**：引入 `cobertura-merger.js` 手动合并覆盖率报告。Cobertura 格式对路径分隔符敏感，且 `cargo-llvm-cov` 原生支持 workspace 级别聚合，手动合并极易因路径不一致导致数据丢失或脚本崩溃，进而阻塞所有 PR。 | `scripts/audit/cobertura-merger.js`, `.github/workflows/ci.yml` |
| **P1** | **性能/成本** | **CI 资源过度消耗**：`coverage.yml` 拆分为 P0-P3 四个并行 Job，且 `ci.yml` 的 coverage 任务同时启动了 MySQL + Postgres 两个重型容器。若无精准的分片逻辑（确保 P0-P3 互斥且全覆盖），这将导致 CI 分钟数翻倍，且可能因 GitHub Actions 并发限制导致排队。 | `.github/workflows/coverage.yml`, `.github/workflows/ci.yml` |
| **P1** | **可维护性** | **Commit Message 耦合业务逻辑**：通过解析 Commit Message (`[coverage-stage:S1]`) 来动态调整覆盖率阈值。这是一种隐式耦合，极易因合并提交（Merge Commit）包含多个标签或格式不规范而导致阈值解析错误（例如始终走默认 85% 或意外降级）。 | `.github/workflows/ci.yml` (Parse coverage stage step) |
| **P2** | **代码质量** | **无效测试未修复**：静态扫描发现 `test_tag_clear_empty_no_error` (lib.rs:4775) 缺少断言。本次 PR 重构了测试运行方式，但未修复此“永远通过”的无效测试，会导致覆盖率数据虚高（测试跑了但没验证逻辑）。 | `packages/sz-rust-cache-facade/src/lib.rs` |
| **P2** | **规范性** | **脚本包含冲突标记/空白**：`collect-baseline.ps1` 第 46 行存在 trailing whitespace 或潜在的冲突标记残留。PowerShell 对特殊字符敏感，可能导致基线收集脚本在 Windows Runner 上执行失败。 | `scripts/collect-baseline.ps1` |

---

### 2. 修改建议

#### 建议 1：移除自定义合并脚本，使用原生 Workspace 聚合
**理由**：`cargo-llvm-cov` 的 `--workspace` 标志已经能够正确处理多 crate 的覆盖率收集。拆分为多次运行再合并不仅增加了 Node.js 依赖，还引入了路径处理的脆弱性。如果必须拆分（为了并行速度），请确保使用 `cargo llvm-cov --no-report` 分片运行，最后统一运行一次 `cargo llvm-cov --report --cobertura` 生成最终报告。

**修改示例**：
```yaml
# .github/workflows/ci.yml
# 移除手动合并步骤，改为分片收集，最后统一生成报告
- name: Run coverage (No Report)
  run: cargo llvm-cov --workspace --no-report --exclude sz-orm-macros

- name: Generate Final Report
  run: cargo llvm-cov --report --cobertura --output-path cobertura-workspace.xml --fail-under-lines $THRESHOLD
```

#### 建议 2：解耦阈值配置
**理由**：不要依赖 Commit Message 解析。建议使用 GitHub Repository Variables 或 Environment Secrets 管理阈值，或者简单地保持固定阈值。如果确实需要阶段性放宽，使用 GitHub 的 `environment` 保护规则更合适。

**修改示例**：
```yaml
# 移除复杂的 case 解析逻辑
- name: Set Threshold
  id: cov-stage
  run: |
    # 优先读取环境变量，否则默认 85
    echo "threshold=${COVERAGE_THRESHOLD:-85}" >> "$GITHUB_OUTPUT"
```

#### 建议 3：修复无效测试与脚本规范
**理由**：确保覆盖率数据的真实性。

**修改示例 (Rust)**：
```rust
// packages/sz-rust-cache-facade/src/lib.rs
#[test]
fn test_tag_clear_empty_no_error() {
    let mut cache = Cache::new();
    // 补充断言：确保空操作不 panic 且状态符合预期
    assert!(cache.tag_clear("").is_ok()); 
    assert_eq!(cache.count(), 0);
}
```

**修改示例 (PowerShell)**：
```powershell
# scripts/collect-baseline.ps1:46
# 移除行尾多余空白
```

---

### 3. 整体评分

| 维度 | 评分 | 说明 |
| :--- | :--- | :--- |
| **架构设计** | 6/10 | 工具链从 tarpaulin 迁移到 llvm-cov 是正确的方向，但引入 JS 脚本合并 XML 是架构倒退。 |
| **稳定性** | 5/10 | 复杂的 CI 依赖链（DB + Node + Rust + 自定义脚本）显著增加了构建失败的概率。 |
| **可维护性** | 6/10 | Commit Message 解析逻辑增加了理解成本；分片策略需要持续维护以防遗漏新 Crate。 |
| **安全性** | 9/10 | 未发现明显的安全漏洞，权限控制合理。 |

**总评：6 / 10**

**总结**：本次 PR 在工具链升级方面做出了积极尝试，解决了跨平台覆盖率统计的痛点。但**过度工程化**的合并方案和脆弱的阈值控制逻辑引入了新的风险点。建议在合入前简化报告生成流程（回归原生工具能力），并修复静态扫描发现的测试质量问题，以确保“覆盖率门禁”本身的可信度。


## 结论
❌ **阻塞**: 1 个 ≥ medium 级别问题，禁止合入
