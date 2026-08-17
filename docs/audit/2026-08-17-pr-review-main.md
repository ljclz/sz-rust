# PR 审查报告（2026-08-17，branch: main，range: HEAD~5..HEAD）

> 审查时点: `HEAD @ 183422f`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 1 medium / 1 low）

- [medium] `diff` **whitespace-error**: 空白/冲突标记错误: docs/audit/2026-08-17-pr-review-main.md:11: trailing whitespace. +- [medium] `diff` **whitespace-error**: 空白/冲突标记错误: scripts/collect-baseline.ps1:36: trailing whitespace. +     scripts/collect-baseline.ps1:38: trailing whitespace.  scripts/collect-baseline.ps1:36: trailing whitespace.
- [low] `gate` **assertion-value**:   [WARN] packages/sz-rust-cache-facade/src/lib.rs:4775 测试 test_tag_clear_empty_no_error() 无断言宏但有 1 处 u


## 补充信息

## 变更集
```
 .cargo/config.toml                                 |    7 +
 .github/workflows/ci.yml                           |   73 +-
 .github/workflows/coverage.yml                     |  241 +++-
 Cargo.lock                                         |    5 +
 docs/audit/2026-08-17-pr-review-main.md            |  226 +++
 docs/audit/coverage-exemption.md                   |   16 +
 docs/audit/coverage-priority.json                  |  328 +++++
 docs/audit/doc-debt.md                             |   11 +
 docs/audit/events.jsonl                            |    4 +
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
 130 files changed, 16760 insertions(+), 110 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）（缓存命中 c19d201eb2f33bc9）



<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
</head>
<body>
<h2>🔍 Code Review: sz-rust CI 覆盖率门禁升级</h2>
<p><strong>评分：6/10</strong> — CI 架构改进有价值，但遗留了关键代码质量问题，且引入了不必要的运行时依赖。</p>
<hr/>
<h3>🚨 关键问题清单 (Top 4)</h3>
<h4>1. [严重] 并发测试失败被“规避”而非“修复” (Concurrency)</h4>
<ul>
<li><strong>现象</strong>：静态清单显示 <code>test_concurrent_workers_no_duplicate_execution</code> Panic 失败。但在 CI 配置中，针对 <code>sz-rust-sz300</code> 的 DB 集成测试使用了 <code>--test-threads=1</code> 强制串行执行。</li>
<li><strong>风险</strong>：这是典型的“掩盖问题”。强制串行虽然能让 CI 变绿，但掩盖了 Worker 调度逻辑中可能存在的竞态条件（Race Condition）。如果生产环境是并发的，测试串行无法保证生产安全。</li>
<li><strong>建议</strong>：移除 <code>--test-threads=1</code>，修复代码中的并发锁或状态管理，确保测试在并发下通过。</li>
</ul>
<pre><code class="language-diff">-      - name: Run sz300 DB integration coverage
-        run: |
-          cargo llvm-cov -p sz-rust-sz300 \
-            --ignore-tests-in-target \
-            --cobertura --output-path cobertura-sz300-db.xml \
-            -- --ignored --test-threads=1  # ❌ 掩盖并发问题
+      - name: Run sz300 DB integration coverage
+        run: |
+          cargo llvm-cov -p sz-rust-sz300 \
+            --cobertura --output-path cobertura-sz300-db.xml \
+            -- --ignored  # ✅ 恢复并发测试，倒逼代码修复
</code></pre>

<h4>2. [高危] 生产代码裸 unwrap (Safety)</h4>
<ul>
<li><strong>现象</strong>：清单明确指出生产代码存在 2 处裸 <code>unwrap()</code>。</li>
<li><strong>风险</strong>：在异步任务或外部接口调用中，<code>unwrap()</code> 会导致整个进程 Panic 崩溃，违反 Rust 生产环境“铁律”。</li>
<li><strong>建议</strong>：必须替换为 <code>match</code> 或 <code>?</code> 传播错误。</li>
</ul>
<pre><code class="language-rust">// ❌ 错误示范
let config = load_config().unwrap();

// ✅ 修正建议
let config = match load_config() {
    Ok(c) => c,
    Err(e) => {
        log::error!("Failed to load config: {}", e);
        return Err(AppError::ConfigLoadFailed(e));
    }
};
</code></pre>

<h4>3. [中] CI 引入 Node.js 运行时依赖 (Maintainability)</h4>
<ul>
<li><strong>现象</strong>：为了合并 Cobertura XML 报告和进行 Per-crate 校验，CI 引入了 <code>actions/setup-node</code> 和自定义 JS 脚本 (<code>cobertura-merger.js</code>)。</li>
<li><strong>风险</strong>：Rust 项目引入 Node.js 运行时增加了环境复杂度和攻击面。如果 JS 脚本解析 XML 失败（如编码问题），CI 会误报。且维护两套语言栈的构建逻辑成本较高。</li>
<li><strong>建议</strong>：尽量使用 <code>cargo-llvm-cov</code> 原生的 <code>--merge</code> 功能，或者编写一个小型 Rust 二进制工具来完成合并，保持技术栈纯粹。</li>
</ul>

<h4>4. [低] 脚本空白字符与规范 (Style)</h4>
<ul>
<li><strong>现象</strong>：<code>scripts/collect-baseline.ps1</code> 存在 trailing whitespace。</li>
<li><strong>建议</strong>：配置编辑器自动去除尾部空格，或在 CI 中增加 <code>check-trailing-whitespace</code> 步骤。</li>
</ul>
<hr/>
<h3>📝 总结建议</h3>
<ol>
<li><strong>立即修复</strong>：处理 2 处生产代码 <code>unwrap()</code>，这是稳定性红线。</li>
<li><strong>核心攻坚</strong>：不要接受 <code>--test-threads=1</code> 作为并发测试的解决方案，需深入排查 <code>sz-rust-sz300</code> 中的锁竞争或共享状态问题。</li>
<li><strong>架构优化</strong>：评估是否可以用 Rust 工具链替代 CI 中的 Node.js 脚本，减少依赖。</li>
</ol>
</body>
</html>


## 结论
❌ **阻塞**: 1 个 ≥ medium 级别问题，禁止合入
