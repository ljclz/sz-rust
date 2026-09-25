# PR 审查报告（2026-09-25，branch: main，range: origin/main..HEAD）

> 审查时点: `HEAD @ a3b1772`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 1 high / 1 medium / 0 low）

- [medium] `workspace` **fmt**: 格式不合格: Diff in \\?\E:\vue\test\鲜视达\rust\sz-rust\packages\sz-rust-http-facade\src\openapi.rs:48:      }
- [high] `integration` **integration-failure**: 集成测试挂起超 10 分钟被强制终止（timeout rc=124，疑似 MySQL 事务/锁路径阻塞，已通过的测试: 9 个）


## 补充信息

## 变更集
```
 .cargo/config.toml                                 |   13 +-
 .githooks/pre-commit                               |    5 +
 .github/workflows/ci.yml                           |   23 +
 .github/workflows/compile-time-monitor.yml         |   75 ++
 .github/workflows/i18n-check.yml                   |   13 +
 CHANGELOG.md                                       |   93 ++
 Cargo.lock                                         | 1335 +++++++++++---------
 Cargo.toml                                         |  164 ++-
 docker-compose.yml                                 |   87 +-
 docs/2026-09-24-framework-maturity-assessment.md   |  188 +++
 docs/adr/0022-framework-business-boundary.md       |   57 +
 docs/audit/2026-09-24-gate-block-report.md         |   95 ++
 docs/audit/2026-09-24-pr-review-main.md            |  359 ++++++
 docs/audit/2026-09-25-pr-review-main.md            |  368 ++++++
 docs/audit/events.jsonl                            |    4 +
 docs/developer/grpc-streaming-guide.md             |   82 ++
 docs/developer/integration-testing.md              |  116 ++
 docs/developer/sccache-guide.md                    |  209 +++
 docs/developer/wasm-deployment-guide.md            |  116 ++
 docs/migration/v1.2.0-to-v1.3.0.md                 |  118 ++
 docs/thinkphp-api-mapping.md                       |   90 ++
 i18n/en-us/errors.json                             |   32 +
 i18n/zh-cn/errors.json                             |   32 +
 .../sz-rust-addons-admin/tests/integration_test.rs |   69 +-
 .../migrations/0001_create_sz_agent_executions.sql |   23 +
 packages/sz-rust-ai-facade/src/agent/delegate.rs   |  193 +++
 packages/sz-rust-ai-facade/src/agent/hitl.rs       |  219 ++++
 packages/sz-rust-ai-facade/src/agent/mod.rs        |   19 +
 packages/sz-rust-ai-facade/src/agent/multi_step.rs |  314 +++++
 .../sz-rust-ai-facade/src/agent/persistence.rs     |  377 ++++++
 packages/sz-rust-ai-facade/src/agent/recovery.rs   |  202 +++
 .../sz-rust-ai-facade/src/llm/builtin_models.rs    |  138 ++
 packages/sz-rust-ai-facade/src/llm/fallback.rs     |  326 +++++
 packages/sz-rust-ai-facade/src/llm/mod.rs          |    9 +-
 .../sz-rust-ai-facade/src/llm/model_registry.rs    |  227 ++++
 packages/sz-rust-ai-facade/src/llm/router.rs       |  313 ++++-
 packages/sz-rust-ai-facade/src/llm/test_support.rs |   38 +
 packages/sz-rust-api-gateway/Cargo.toml            |   35 +
 packages/sz-rust-api-gateway/src/config.rs         |  272 ++++
 packages/sz-rust-api-gateway/src/error.rs          |   42 +
 packages/sz-rust-api-gateway/src/forwarder.rs      |  370 ++++++
 packages/sz-rust-api-gateway/src/grpc_streaming.rs |  106 ++
 packages/sz-rust-api-gateway/src/lib.rs            |   32 +
 packages/sz-rust-api-gateway/src/middleware.rs     |  417 ++++++
 packages/sz-rust-api-gateway/src/protocol_grpc.rs  |  145 +++
 packages/sz-rust-api-gateway/src/protocol_http.rs  |  223 ++++
 packages/sz-rust-api-gateway/src/router_engine.rs  |  291 +++++
 packages/sz-rust-api-gateway/tests/integration.rs  |  287 +++++
 packages/sz-rust-auth-facade/src/jwt.rs            |  190 +++
 packages/sz-rust-auth-facade/src/lib.rs            |    6 +
 packages/sz-rust-cli/Cargo.toml                    |    4 +-
 packages/sz-rust-cli/src/cmd/make.rs               |  190 +++
 packages/sz-rust-codegen-loop/Cargo.toml           |   32 +
 packages/sz-rust-codegen-loop/src/error.rs         |   47 +
 packages/sz-rust-codegen-loop/src/file_guard.rs    |  132 ++
 packages/sz-rust-codegen-loop/src/generator.rs     |  192 +++
 packages/sz-rust-codegen-loop/src/human_review.rs  |   84 ++
 packages/sz-rust-codegen-loop/src/lib.rs           |   22 +
 packages/sz-rust-codegen-loop/src/loop.rs          |  168 +++
 packages/sz-rust-codegen-loop/src/parser.rs        |  341 +++++
 packages/sz-rust-codegen-loop/src/security.rs      |  132 ++
 packages/sz-rust-codegen-loop/src/validator.rs     |  134 ++
 packages/sz-rust-codegen-loop/tests/e2e.rs         |   53 +
 packages/sz-rust-config-center/Cargo.toml          |   38 +
 packages/sz-rust-config-center/src/consul.rs       |  170 +++
 packages/sz-rust-config-center/src/crypto.rs       |  120 ++
 packages/sz-rust-config-center/src/error.rs        |   42 +
 packages/sz-rust-config-center/src/gray_matcher.rs |  129 ++
 packages/sz-rust-config-center/src/health_check.rs |   97 ++
 packages/sz-rust-config-center/src/lib.rs          |   47 +
 packages/sz-rust-config-center/src/nacos.rs        |  160 +++
 packages/sz-rust-config-center/src/source.rs       |  108 ++
 packages/sz-rust-config-center/src/version.rs      |  129 ++
 packages/sz-rust-core/Cargo.toml                   |   14 +
 packages/sz-rust-core/src/lib.rs                   |   21 +
 packages/sz-rust-distributed-tx/Cargo.toml         |   28 +
 .../migrations/0001_create_sz_dtx_log.sql          |   20 +
 packages/sz-rust-distributed-tx/src/error.rs       |  114 ++
 packages/sz-rust-distributed-tx/src/lib.rs         |   18 +
 packages/sz-rust-distributed-tx/src/persistence.rs |  352 ++++++
 packages/sz-rust-distributed-tx/src/saga.rs        |  539 ++++++++
 packages/sz-rust-distributed-tx/src/tcc.rs         |  437 +++++++
 .../tests/saga_integration.rs                      |  371 ++++++
 packages/sz-rust-examples/Cargo.toml               |   17 +
 packages/sz-rust-examples/src/bin/wasm_basic.rs    |   23 +
 .../sz-rust-examples/src/bin/wasm_edge_deploy.rs   |   28 +
 .../sz-rust-examples/src/bin/wasm_host_function.rs |   24 +
 packages/sz-rust-examples/src/bin/wasm_sandbox.rs  |   29 +
 packages/sz-rust-facade/Cargo.toml                 |   43 +
 packages/sz-rust-facade/benches/facade_bench.rs    |  101 ++
 packages/sz-rust-facade/src/cache.rs               |  130 ++
 packages/sz-rust-facade/src/config.rs              |   50 +
 packages/sz-rust-facade/src/db.rs                  |   57 +
 packages/sz-rust-facade/src/error.rs               |   40 +
 packages/sz-rust-facade/src/event.rs               |   73 ++
 packages/sz-rust-facade/src/lib.rs                 |   54 +
 packages/sz-rust-facade/src/log.rs                 |   73 ++
 packages/sz-rust-facade/src/mock.rs                |   87 ++
 packages/sz-rust-facade/src/queue.rs               |   86 ++
 packages/sz-rust-facade/src/request.rs             |   70 +
 packages/sz-rust-facade/src/response.rs            |   98 ++
 packages/sz-rust-http-facade/Cargo.toml            |    2 +
 packages/sz-rust-http-facade/src/error.rs          |  413 +++++-
 .../sz-rust-http-facade/src/error_code_registry.rs |  388 ++++++
 packages/sz-rust-http-facade/src/lib.rs            |    4 +-
 packages/sz-rust-http-facade/src/openapi.rs        |   82 +-
 packages/sz-rust-infra-facade/Cargo.toml           |    1 +
 packages/sz-rust-infra-facade/src/config.rs        |  468 ++++++-
 .../sz-rust-infra-facade/src/debug_collector.rs    |  395 ++++++
 packages/sz-rust-infra-facade/src/debug_page.rs    |  229 ++++
 packages/sz-rust-infra-facade/src/lib.rs           |    1 +
 .../sz-rust-infra-facade/tests/config_compat.rs    |  381 ++++++
 packages/sz-rust-middleware-facade/Cargo.toml      |    1 +
 packages/sz-rust-middleware-facade/src/lib.rs      |    9 +
 .../sz-rust-middleware-facade/src/panic_guard.rs   |  368 ++++++
 .../sz-rust-middleware-facade/src/sz300_compat.rs  |  628 +++++++++
 packages/sz-rust-observability/src/lib.rs          |    4 +
 packages/sz-rust-observability/src/log_level.rs    |  290 +++++
 packages/sz-rust-observability/src/memory_guard.rs |  267 ++++
 packages/sz-rust-ops-api/Cargo.toml                |   27 +
 packages/sz-rust-ops-api/src/admin_guard.rs        |  171 +++
 packages/sz-rust-ops-api/src/gray_release.rs       |  400 ++++++
 packages/sz-rust-ops-api/src/lib.rs                |   39 +
 packages/sz-rust-ops-api/src/routes.rs             |  208 +++
 packages/sz-rust-orm-facade/Cargo.toml             |    9 +
 packages/sz-rust-orm-facade/src/pool_scaler.rs     |  122 ++
 packages/sz-rust-service-registry/Cargo.toml       |   35 +
 packages/sz-rust-service-registry/src/consul.rs    |  319 +++++
 packages/sz-rust-service-registry/src/error.rs     |   46 +
 .../sz-rust-service-registry/src/health_check.rs   |   97 ++
 .../sz-rust-service-registry/src/kubernetes.rs     |  267 ++++
 packages/sz-rust-service-registry/src/lib.rs       |   41 +
 .../sz-rust-service-registry/src/load_balancer.rs  |  363 ++++++
 .../sz-rust-service-registry/src/local_cache.rs    |  164 +++
 packages/sz-rust-service-registry/src/nacos.rs     |  326 +++++
 packages/sz-rust-service-registry/src/registry.rs  |  316 +++++
 .../tests/fault_injection.rs                       |  224 ++++
 packages/sz-rust-state-facade/Cargo.toml           |    3 +
 packages/sz-rust-state-facade/src/i18n.rs          |  336 +++++
 .../sz-rust-state-facade/src/i18n/middleware.rs    |  123 ++
 packages/sz-rust-sz300/Cargo.toml                  |   66 +
 packages/sz-rust-sz300/benches/api_bench.rs        |  178 +++
 packages/sz-rust-sz300/config/app.yml              |   10 +
 packages/sz-rust-sz300/config/database.yml         |    6 +
 packages/sz-rust-sz300/config/mqtt.yml             |    7 +
 packages/sz-rust-sz300/docs/index.html             |   32 +
 packages/sz-rust-sz300/docs/openapi.yaml           | 1145 +++++++++++++++++
 packages/sz-rust-sz300/docs/performance-testing.md |  195 +++
 packages/sz-rust-sz300/migrations/001_init.sql     |  183 +++
 packages/sz-rust-sz300/migrations/001_init_pg.sql  |  177 +++
 packages/sz-rust-sz300/scripts/baseline_test.ps1   |   78 ++
 packages/sz-rust-sz300/scripts/concurrent_test.ps1 |   49 +
 packages/sz-rust-sz300/scripts/load_test.ps1       |  329 +++++
 packages/sz-rust-sz300/src/config.rs               |  138 ++
 packages/sz-rust-sz300/src/controllers/admin.rs    |  118 ++
 packages/sz-rust-sz300/src/controllers/auth.rs     |  259 ++++
 packages/sz-rust-sz300/src/controllers/common.rs   |  195 +++
 packages/sz-rust-sz300/src/controllers/device.rs   |  333 +++++
 packages/sz-rust-sz300/src/controllers/file.rs     |   96 ++
 .../sz-rust-sz300/src/controllers/file_serve.rs    |   63 +
 packages/sz-rust-sz300/src/controllers/health.rs   |  114 ++
 packages/sz-rust-sz300/src/controllers/merchant.rs |  262 ++++
 packages/sz-rust-sz300/src/controllers/mod.rs      |   20 +
 packages/sz-rust-sz300/src/controllers/order.rs    |  254 ++++
 packages/sz-rust-sz300/src/controllers/product.rs  |  268 ++++
 packages/sz-rust-sz300/src/db.rs                   |   52 +
 packages/sz-rust-sz300/src/i18n_error.rs           |   51 +
 packages/sz-rust-sz300/src/lib.rs                  |   41 +
 packages/sz-rust-sz300/src/main.rs                 |  208 +++
 .../src/middleware/auth_middleware.rs              |   44 +
 packages/sz-rust-sz300/src/middleware/mod.rs       |    4 +
 .../sz-rust-sz300/src/middleware/role_guard.rs     |  199 +++
 packages/sz-rust-sz300/src/models/ai_category.rs   |  112 ++
 packages/sz-rust-sz300/src/models/category.rs      |   93 ++
 packages/sz-rust-sz300/src/models/device.rs        |  159 +++
 packages/sz-rust-sz300/src/models/market.rs        |  126 ++
 packages/sz-rust-sz300/src/models/merchant.rs      |  156 +++
 packages/sz-rust-sz300/src/models/merchant_user.rs |  204 +++
 packages/sz-rust-sz300/src/models/mod.rs           |   28 +
 packages/sz-rust-sz300/src/models/operate_log.rs   |  119 ++
 packages/sz-rust-sz300/src/models/order.rs         |  172 +++
 packages/sz-rust-sz300/src/models/order_item.rs    |  134 ++
 packages/sz-rust-sz300/src/models/ota_version.rs   |  156 +++
 packages/sz-rust-sz300/src/models/product.rs       |  164 +++
 packages/sz-rust-sz300/src/models/settlement.rs    |  140 ++
 packages/sz-rust-sz300/src/models/system_config.rs |   93 ++
 packages/sz-rust-sz300/src/openapi.rs              |  415 ++++++
 packages/sz-rust-sz300/src/router.rs               |  103 ++
 .../sz-rust-sz300/src/services/auth_service.rs     |  264 ++++
 .../sz-rust-sz300/src/services/device_service.rs   |  313 +++++
 .../sz-rust-sz300/src/services/file_service.rs     |  103 ++
 .../sz-rust-sz300/src/services/health_service.rs   |   40 +
 .../sz-rust-sz300/src/services/merchant_service.rs |  236 ++++
 packages/sz-rust-sz300/src/services/mod.rs         |  128 ++
 .../sz-rust-sz300/src/services/mqtt_listener.rs    |   92 ++
 .../sz-rust-sz300/src/services/mqtt_service.rs     |  272 ++++
 .../sz-rust-sz300/src/services/order_service.rs    |  274 ++++
 .../sz-rust-sz300/src/services/product_service.rs  |  292 +++++
 packages/sz-rust-sz300/src/state.rs                |   16 +
 packages/sz-rust-sz300/tests/config_test.rs        |   79 ++
 .../sz-rust-sz300/tests/db_integration_test.rs     |  941 ++++++++++++++
 packages/sz-rust-sz300/tests/metrics_test.rs       |   75 ++
 packages/sz-rust-sz300/tests/mqtt_dispatch_test.rs |  100 ++
 packages/sz-rust-sz300/tests/redaction_test.rs     |   72 ++
 .../sz-rust-sz300/tests/service_coverage_test.rs   |  157 +++
 packages/sz-rust-sz300/tests/services_test.rs      |  463 +++++++
 .../sz-rust-sz300/tests/windows_memory_baseline.rs |  206 +++
 packages/sz-rust-testkit/Cargo.toml                |   33 +
 packages/sz-rust-testkit/src/error.rs              |   13 +
 packages/sz-rust-testkit/src/factory.rs            |  147 +++
 packages/sz-rust-testkit/src/fixture.rs            |  106 ++
 packages/sz-rust-testkit/src/http_client.rs        |  143 +++
 packages/sz-rust-testkit/src/lib.rs                |   34 +
 packages/sz-rust-testkit/src/mock.rs               |  106 ++
 packages/sz-rust-testkit/src/test_case.rs          |   95 ++
 scripts/audit/error_code_conflict.js               |   87 ++
 scripts/audit/pr-review.sh                         |   16 +-
 scripts/check-boundary.sh                          |   83 ++
 scripts/check-compile-time.sh                      |   95 +-
 scripts/check-feature-conflicts.sh                 |   78 ++
 scripts/check-i18n-completeness.sh                 |   42 +
 scripts/compile-time-baseline.json                 |   48 +-
 scripts/sccache-stats.sh                           |   97 ++
 scripts/sz300-migration-verify.sh                  |  120 ++
 scripts/sz300-test-baseline.json                   |    1 +
 src/lib.rs                                         |    4 +
 226 files changed, 34040 insertions(+), 671 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）（缓存命中 bcbdbab8f5b10fcb）

## PR 评审报告

### 1. 最重要的潜在问题

#### 1.1 sccache 配置可能导致本地构建失败（可维护性/可靠性）
`.cargo/config.toml` 中硬编码 `rustc-wrapper = "sccache"`，但未安装 sccache 的开发者执行 `cargo build` 会直接报错。虽然 `.githooks/pre-commit` 和 CI 中做了降级处理，但其他场景（如 IDE 插件、手动命令）仍会失败。此外，`RUSTC_WRAPPER` 环境变量在 CI 中被置空，但 `.cargo/config.toml` 的优先级高于环境变量？实际上 Cargo 的配置优先级是：命令行 > 环境变量 > config.toml，所以 CI 中设置 `RUSTC_WRAPPER=""` 可以覆盖，但本地用户若未设置环境变量则会被 config.toml 影响。

**建议**：使用 Cargo 的 `[target.'cfg(...)'.rustc-wrapper]` 条件配置，或通过 `env` 变量动态检测。更稳妥的方式是移除 config.toml 中的硬编码，改为在 CI 和本地通过环境变量或脚本显式设置。

```toml
# .cargo/config.toml
# 移除 rustc-wrapper 硬编码，改为注释说明
# [build]
# rustc-wrapper = "sccache"  # 已移除，请通过环境变量 RUSTC_WRAPPER 启用
```

或使用条件配置（但 Cargo 不支持基于命令存在的条件），建议在 `build.rs` 或 CI 脚本中检测。

#### 1.2 集成测试挂起可能与新增 sz300 workspace 引入的数据库锁有关（并发/安全）
已发现问题：集成测试挂起超时，疑似 MySQL 事务/锁路径阻塞。PR 中新增了 `packages/sz-rust-sz300/`，可能引入了新的数据库连接或事务逻辑。虽然 diff 截断，但需要重点审查该模块的数据库访问代码，尤其是事务隔离级别、锁等待超时设置、连接池配置等。

**建议**：在 sz300 模块中检查所有数据库操作，确保：
- 使用 `SELECT ... FOR UPDATE` 时设置合理的锁等待超时（如 `innodb_lock_wait_timeout`）。
- 事务中避免长时间持有连接，及时提交/回滚。
- 连接池配置合理（如 `max_connections`、`acquire_timeout`）。

示例（Rust + sqlx）：
```rust
// 设置锁等待超时
sqlx::query("SET innodb_lock_wait_timeout = 5").execute(&pool).await?;
```

#### 1.3 新增 CI 工作流 `compile-time-monitor.yml` 存在资源浪费和潜在失败点（性能/可维护性）
- 使用 `cargo install sccache` 每次运行都编译安装，耗时且可能失败（网络问题）。
- 未使用 `Swatinem/rust-cache` 缓存依赖，而是手动缓存 registry，效率低。
- 阈值 `--threshold-percent=10` 可能过于严格，导致 CI 不稳定。

**建议**：使用官方 `Swatinem/rust-cache` 并预装 sccache（通过 GitHub Actions 的 `mozilla-actions/sccache-action`），或直接复用主 CI 的缓存。同时将阈值调整为更宽松的值（如 20%）。

```yaml
- uses: mozilla-actions/sccache-action@v0.0.3
- uses: Swatinem/rust-cache@v2
  with:
    workspaces: ". -> target"
```

#### 1.4 新增脚本 `check-boundary.sh` 和 `check-i18n-completeness.sh` 可能未处理错误导致 CI 误报（可维护性）
这些脚本在 CI 中直接运行，若脚本本身有 bug 或依赖缺失，会导致 CI 失败，但可能掩盖真正的问题。需要确保脚本有清晰的退出码和错误信息。

**建议**：在脚本开头添加 `set -euo pipefail`，并输出明确的错误日志。例如：

```bash
#!/usr/bin/env bash
set -euo pipefail
# 检查依赖
command -v jq >/dev/null || { echo "jq required"; exit 1; }
# 执行检查...
```

#### 1.5 编译时间监控工作流与主 CI 的 `RUSTC_WRAPPER` 冲突（并发/一致性）
主 CI 中设置 `RUSTC_WRAPPER: ""` 禁用 sccache，但 compile-time-monitor 中又启用 sccache，两者可能对同一仓库产生不同的编译缓存，导致结果不一致。且该工作流在 PR 上运行，可能增加 CI 负担。

**建议**：将编译时间监控改为定时任务（如 nightly）或仅对 main 分支运行，避免在每次 PR 上重复执行。

### 2. 具体修改建议（代码示例）

#### 2.1 修复 sccache 配置
修改 `.cargo/config.toml`，移除硬编码，改为注释说明，并在 CI 和本地脚本中显式设置：

```toml
# .cargo/config.toml
# [build]
# rustc-wrapper = "sccache"  # 已移除，请通过环境变量 RUSTC_WRAPPER 启用
```

在 CI 中（`.github/workflows/ci.yml`）保留 `RUSTC_WRAPPER: ""`，在本地开发时可通过 `export RUSTC_WRAPPER=sccache` 启用。

#### 2.2 优化 compile-time-monitor.yml
使用 `mozilla-actions/sccache-action` 和 `Swatinem/rust-cache`，并调整触发条件：

```yaml
- uses: actions/checkout@v4
- uses: dtolnay/rust-toolchain@stable
- uses: mozilla-actions/sccache-action@v0.0.3
- uses: Swatinem/rust-cache@v2
  with:
    workspaces: ". -> target"
- name: Measure compile time
  run: bash scripts/check-compile-time.sh --phases --threshold-percent=20
  env:
    RUSTC_WRAPPER: sccache
```

#### 2.3 集成测试挂起排查
在 sz300 模块的数据库访问代码中，添加锁等待超时和连接池配置：

```rust
// 在创建连接池时设置
let pool = MySqlPoolOptions::new()
    .max_connections(10)
    .acquire_timeout(Duration::from_secs(5))
    .connect(&database_url)
    .await?;

// 执行事务时设置锁等待
let mut tx = pool.begin().await?;
sqlx::query("SET innodb_lock_wait_timeout = 5").execute(&mut *tx).await?;
// ... 事务操作
tx.commit().await?;
```

### 3. 整体评分

**6.5 / 10**

**理由**：
- 优点：引入了 sccache 缓存、编译时间监控、i18n 和边界检查，提升了工程化水平。
- 缺点：sccache 配置存在兼容性风险；新增 CI 工作流资源浪费；集成测试挂起问题未解决，且可能与新增 sz300 模块相关；部分脚本健壮性不足。整体改进方向正确，但细节需打磨，且核心问题（测试挂起）未在 PR 中处理，因此评分中等偏上。


## 结论
❌ **阻塞**: 2 个 ≥ medium 级别问题，禁止合入
