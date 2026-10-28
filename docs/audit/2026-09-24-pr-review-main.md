# PR 审查报告（2026-09-24，branch: main，range: origin/main..HEAD）

> 审查时点: `HEAD @ ff531ca`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 1 high / 1 medium / 0 low）

- [medium] `diff` **whitespace-error**: 空白/冲突标记错误: packages/sz-rust-sz300/scripts/concurrent_test.ps1:50: new blank line at EOF.
- [high] `integration` **integration-failure**:


## 补充信息

## 变更集
```
 .cargo/config.toml                                 |   13 +-
 .githooks/pre-commit                               |    5 +
 .github/workflows/ci.yml                           |   19 +
 .github/workflows/compile-time-monitor.yml         |   75 ++
 .github/workflows/i18n-check.yml                   |   13 +
 CHANGELOG.md                                       |   93 ++
 Cargo.lock                                         |  293 ++++-
 Cargo.toml                                         |  129 ++-
 docker-compose.yml                                 |   87 +-
 docs/2026-09-24-framework-maturity-assessment.md   |  188 ++++
 docs/adr/0022-framework-business-boundary.md       |   57 +
 docs/developer/grpc-streaming-guide.md             |   82 ++
 docs/developer/integration-testing.md              |  116 ++
 docs/developer/sccache-guide.md                    |  209 ++++
 docs/developer/wasm-deployment-guide.md            |  116 ++
 docs/migration/v1.2.0-to-v1.3.0.md                 |  118 ++
 docs/thinkphp-api-mapping.md                       |   90 ++
 i18n/en-us/errors.json                             |   32 +
 i18n/zh-cn/errors.json                             |   32 +
 .../migrations/0001_create_sz_agent_executions.sql |   23 +
 packages/sz-rust-ai-facade/src/agent/delegate.rs   |  193 ++++
 packages/sz-rust-ai-facade/src/agent/hitl.rs       |  219 ++++
 packages/sz-rust-ai-facade/src/agent/mod.rs        |   19 +
 packages/sz-rust-ai-facade/src/agent/multi_step.rs |  314 ++++++
 .../sz-rust-ai-facade/src/agent/persistence.rs     |  377 +++++++
 packages/sz-rust-ai-facade/src/agent/recovery.rs   |  202 ++++
 .../sz-rust-ai-facade/src/llm/builtin_models.rs    |  138 +++
 packages/sz-rust-ai-facade/src/llm/fallback.rs     |  316 ++++++
 packages/sz-rust-ai-facade/src/llm/mod.rs          |    9 +-
 .../sz-rust-ai-facade/src/llm/model_registry.rs    |  227 ++++
 packages/sz-rust-ai-facade/src/llm/router.rs       |  310 +++++-
 packages/sz-rust-ai-facade/src/llm/test_support.rs |   38 +
 packages/sz-rust-api-gateway/Cargo.toml            |   35 +
 packages/sz-rust-api-gateway/src/config.rs         |  272 +++++
 packages/sz-rust-api-gateway/src/error.rs          |   42 +
 packages/sz-rust-api-gateway/src/forwarder.rs      |  370 +++++++
 packages/sz-rust-api-gateway/src/grpc_streaming.rs |  106 ++
 packages/sz-rust-api-gateway/src/lib.rs            |   32 +
 packages/sz-rust-api-gateway/src/middleware.rs     |  417 +++++++
 packages/sz-rust-api-gateway/src/protocol_grpc.rs  |  145 +++
 packages/sz-rust-api-gateway/src/protocol_http.rs  |  223 ++++
 packages/sz-rust-api-gateway/src/router_engine.rs  |  291 +++++
 packages/sz-rust-api-gateway/tests/integration.rs  |  287 +++++
 packages/sz-rust-auth-facade/src/jwt.rs            |  190 ++++
 packages/sz-rust-auth-facade/src/lib.rs            |    6 +
 packages/sz-rust-cli/src/cmd/make.rs               |  190 ++++
 packages/sz-rust-codegen-loop/Cargo.toml           |   32 +
 packages/sz-rust-codegen-loop/src/error.rs         |   47 +
 packages/sz-rust-codegen-loop/src/file_guard.rs    |  132 +++
 packages/sz-rust-codegen-loop/src/generator.rs     |  192 ++++
 packages/sz-rust-codegen-loop/src/human_review.rs  |   84 ++
 packages/sz-rust-codegen-loop/src/lib.rs           |   22 +
 packages/sz-rust-codegen-loop/src/loop.rs          |  168 +++
 packages/sz-rust-codegen-loop/src/parser.rs        |  341 ++++++
 packages/sz-rust-codegen-loop/src/security.rs      |  132 +++
 packages/sz-rust-codegen-loop/src/validator.rs     |  134 +++
 packages/sz-rust-codegen-loop/tests/e2e.rs         |   53 +
 packages/sz-rust-config-center/Cargo.toml          |   38 +
 packages/sz-rust-config-center/src/consul.rs       |  170 +++
 packages/sz-rust-config-center/src/crypto.rs       |  120 ++
 packages/sz-rust-config-center/src/error.rs        |   42 +
 packages/sz-rust-config-center/src/gray_matcher.rs |  129 +++
 packages/sz-rust-config-center/src/health_check.rs |   97 ++
 packages/sz-rust-config-center/src/lib.rs          |   47 +
 packages/sz-rust-config-center/src/nacos.rs        |  160 +++
 packages/sz-rust-config-center/src/source.rs       |  108 ++
 packages/sz-rust-config-center/src/version.rs      |  129 +++
 packages/sz-rust-core/Cargo.toml                   |   14 +
 packages/sz-rust-core/src/lib.rs                   |   21 +
 packages/sz-rust-distributed-tx/Cargo.toml         |   28 +
 .../migrations/0001_create_sz_dtx_log.sql          |   20 +
 packages/sz-rust-distributed-tx/src/error.rs       |  114 ++
 packages/sz-rust-distributed-tx/src/lib.rs         |   18 +
 packages/sz-rust-distributed-tx/src/persistence.rs |  352 ++++++
 packages/sz-rust-distributed-tx/src/saga.rs        |  539 +++++++++
 packages/sz-rust-distributed-tx/src/tcc.rs         |  437 ++++++++
 .../tests/saga_integration.rs                      |  371 +++++++
 packages/sz-rust-examples/Cargo.toml               |   17 +
 packages/sz-rust-examples/src/bin/wasm_basic.rs    |   23 +
 .../sz-rust-examples/src/bin/wasm_edge_deploy.rs   |   28 +
 .../sz-rust-examples/src/bin/wasm_host_function.rs |   24 +
 packages/sz-rust-examples/src/bin/wasm_sandbox.rs  |   29 +
 packages/sz-rust-facade/Cargo.toml                 |   43 +
 packages/sz-rust-facade/benches/facade_bench.rs    |  101 ++
 packages/sz-rust-facade/src/cache.rs               |  130 +++
 packages/sz-rust-facade/src/config.rs              |   50 +
 packages/sz-rust-facade/src/db.rs                  |   57 +
 packages/sz-rust-facade/src/error.rs               |   40 +
 packages/sz-rust-facade/src/event.rs               |   73 ++
 packages/sz-rust-facade/src/lib.rs                 |   54 +
 packages/sz-rust-facade/src/log.rs                 |   73 ++
 packages/sz-rust-facade/src/mock.rs                |   87 ++
 packages/sz-rust-facade/src/queue.rs               |   86 ++
 packages/sz-rust-facade/src/request.rs             |   70 ++
 packages/sz-rust-facade/src/response.rs            |   98 ++
 packages/sz-rust-http-facade/Cargo.toml            |    2 +
 packages/sz-rust-http-facade/src/error.rs          |  413 ++++++-
 .../sz-rust-http-facade/src/error_code_registry.rs |  388 +++++++
 packages/sz-rust-http-facade/src/lib.rs            |    4 +-
 packages/sz-rust-http-facade/src/openapi.rs        |   82 +-
 packages/sz-rust-infra-facade/Cargo.toml           |    1 +
 packages/sz-rust-infra-facade/src/config.rs        |  468 +++++++-
 .../sz-rust-infra-facade/src/debug_collector.rs    |  395 +++++++
 packages/sz-rust-infra-facade/src/debug_page.rs    |  229 ++++
 packages/sz-rust-infra-facade/src/lib.rs           |    1 +
 .../sz-rust-infra-facade/tests/config_compat.rs    |  381 +++++++
 packages/sz-rust-middleware-facade/Cargo.toml      |    1 +
 packages/sz-rust-middleware-facade/src/lib.rs      |    9 +
 .../sz-rust-middleware-facade/src/panic_guard.rs   |  368 +++++++
 .../sz-rust-middleware-facade/src/sz300_compat.rs  |  628 +++++++++++
 packages/sz-rust-observability/src/lib.rs          |    4 +
 packages/sz-rust-observability/src/log_level.rs    |  290 +++++
 packages/sz-rust-observability/src/memory_guard.rs |  267 +++++
 packages/sz-rust-ops-api/Cargo.toml                |   27 +
 packages/sz-rust-ops-api/src/admin_guard.rs        |  171 +++
 packages/sz-rust-ops-api/src/gray_release.rs       |  394 +++++++
 packages/sz-rust-ops-api/src/lib.rs                |   39 +
 packages/sz-rust-ops-api/src/routes.rs             |  208 ++++
 packages/sz-rust-orm-facade/Cargo.toml             |    9 +
 packages/sz-rust-orm-facade/src/pool_scaler.rs     |  122 +++
 packages/sz-rust-service-registry/Cargo.toml       |   35 +
 packages/sz-rust-service-registry/src/consul.rs    |  319 ++++++
 packages/sz-rust-service-registry/src/error.rs     |   46 +
 .../sz-rust-service-registry/src/health_check.rs   |   97 ++
 .../sz-rust-service-registry/src/kubernetes.rs     |  267 +++++
 packages/sz-rust-service-registry/src/lib.rs       |   41 +
 .../sz-rust-service-registry/src/load_balancer.rs  |  361 ++++++
 .../sz-rust-service-registry/src/local_cache.rs    |  164 +++
 packages/sz-rust-service-registry/src/nacos.rs     |  326 ++++++
 packages/sz-rust-service-registry/src/registry.rs  |  316 ++++++
 .../tests/fault_injection.rs                       |  224 ++++
 packages/sz-rust-state-facade/Cargo.toml           |    3 +
 packages/sz-rust-state-facade/src/i18n.rs          |  336 ++++++
 .../sz-rust-state-facade/src/i18n/middleware.rs    |  123 +++
 packages/sz-rust-sz300/Cargo.toml                  |   66 ++
 packages/sz-rust-sz300/benches/api_bench.rs        |  178 +++
 packages/sz-rust-sz300/config/app.yml              |   10 +
 packages/sz-rust-sz300/config/database.yml         |    6 +
 packages/sz-rust-sz300/config/mqtt.yml             |    7 +
 packages/sz-rust-sz300/docs/index.html             |   32 +
 packages/sz-rust-sz300/docs/openapi.yaml           | 1145 ++++++++++++++++++++
 packages/sz-rust-sz300/docs/performance-testing.md |  195 ++++
 packages/sz-rust-sz300/migrations/001_init.sql     |  183 ++++
 packages/sz-rust-sz300/migrations/001_init_pg.sql  |  177 +++
 packages/sz-rust-sz300/scripts/baseline_test.ps1   |   78 ++
 packages/sz-rust-sz300/scripts/concurrent_test.ps1 |   50 +
 packages/sz-rust-sz300/scripts/load_test.ps1       |  329 ++++++
 packages/sz-rust-sz300/src/config.rs               |  112 ++
 packages/sz-rust-sz300/src/controllers/admin.rs    |  105 ++
 packages/sz-rust-sz300/src/controllers/auth.rs     |  259 +++++
 packages/sz-rust-sz300/src/controllers/common.rs   |  195 ++++
 packages/sz-rust-sz300/src/controllers/device.rs   |  333 ++++++
 packages/sz-rust-sz300/src/controllers/file.rs     |   96 ++
 .../sz-rust-sz300/src/controllers/file_serve.rs    |   63 ++
 packages/sz-rust-sz300/src/controllers/health.rs   |  114 ++
 packages/sz-rust-sz300/src/controllers/merchant.rs |  262 +++++
 packages/sz-rust-sz300/src/controllers/mod.rs      |   20 +
 packages/sz-rust-sz300/src/controllers/order.rs    |  254 +++++
 packages/sz-rust-sz300/src/controllers/product.rs  |  268 +++++
 packages/sz-rust-sz300/src/db.rs                   |   52 +
 packages/sz-rust-sz300/src/i18n_error.rs           |   51 +
 packages/sz-rust-sz300/src/lib.rs                  |   41 +
 packages/sz-rust-sz300/src/main.rs                 |  208 ++++
 .../src/middleware/auth_middleware.rs              |   44 +
 packages/sz-rust-sz300/src/middleware/mod.rs       |    4 +
 .../sz-rust-sz300/src/middleware/role_guard.rs     |  183 ++++
 packages/sz-rust-sz300/src/models/ai_category.rs   |  112 ++
 packages/sz-rust-sz300/src/models/category.rs      |   93 ++
 packages/sz-rust-sz300/src/models/device.rs        |  159 +++
 packages/sz-rust-sz300/src/models/market.rs        |  126 +++
 packages/sz-rust-sz300/src/models/merchant.rs      |  156 +++
 packages/sz-rust-sz300/src/models/merchant_user.rs |  204 ++++
 packages/sz-rust-sz300/src/models/mod.rs           |   28 +
 packages/sz-rust-sz300/src/models/operate_log.rs   |  119 ++
 packages/sz-rust-sz300/src/models/order.rs         |  172 +++
 packages/sz-rust-sz300/src/models/order_item.rs    |  134 +++
 packages/sz-rust-sz300/src/models/ota_version.rs   |  156 +++
 packages/sz-rust-sz300/src/models/product.rs       |  164 +++
 packages/sz-rust-sz300/src/models/settlement.rs    |  140 +++
 packages/sz-rust-sz300/src/models/system_config.rs |   93 ++
 packages/sz-rust-sz300/src/openapi.rs              |  415 +++++++
 packages/sz-rust-sz300/src/router.rs               |  103 ++
 .../sz-rust-sz300/src/services/auth_service.rs     |  264 +++++
 .../sz-rust-sz300/src/services/device_service.rs   |  313 ++++++
 .../sz-rust-sz300/src/services/file_service.rs     |  103 ++
 .../sz-rust-sz300/src/services/health_service.rs   |   40 +
 .../sz-rust-sz300/src/services/merchant_service.rs |  236 ++++
 packages/sz-rust-sz300/src/services/mod.rs         |  128 +++
 .../sz-rust-sz300/src/services/mqtt_listener.rs    |   92 ++
 .../sz-rust-sz300/src/services/mqtt_service.rs     |  272 +++++
 .../sz-rust-sz300/src/services/order_service.rs    |  274 +++++
 .../sz-rust-sz300/src/services/product_service.rs  |  292 +++++
 packages/sz-rust-sz300/src/state.rs                |   16 +
 packages/sz-rust-sz300/tests/config_test.rs        |   79 ++
 .../sz-rust-sz300/tests/db_integration_test.rs     |  941 ++++++++++++++++
 packages/sz-rust-sz300/tests/metrics_test.rs       |   75 ++
 packages/sz-rust-sz300/tests/mqtt_dispatch_test.rs |  100 ++
 packages/sz-rust-sz300/tests/redaction_test.rs     |   72 ++
 .../sz-rust-sz300/tests/service_coverage_test.rs   |  157 +++
 packages/sz-rust-sz300/tests/services_test.rs      |  463 ++++++++
 .../sz-rust-sz300/tests/windows_memory_baseline.rs |  206 ++++
 packages/sz-rust-testkit/Cargo.toml                |   33 +
 packages/sz-rust-testkit/src/error.rs              |   13 +
 packages/sz-rust-testkit/src/factory.rs            |  145 +++
 packages/sz-rust-testkit/src/fixture.rs            |  105 ++
 packages/sz-rust-testkit/src/http_client.rs        |  139 +++
 packages/sz-rust-testkit/src/lib.rs                |   34 +
 packages/sz-rust-testkit/src/mock.rs               |  103 ++
 packages/sz-rust-testkit/src/test_case.rs          |   95 ++
 scripts/audit/error_code_conflict.js               |   87 ++
 scripts/check-boundary.sh                          |   83 ++
 scripts/check-compile-time.sh                      |   95 +-
 scripts/check-feature-conflicts.sh                 |   78 ++
 scripts/check-i18n-completeness.sh                 |   42 +
 scripts/compile-time-baseline.json                 |   48 +-
 scripts/sccache-stats.sh                           |   97 ++
 scripts/sz300-migration-verify.sh                  |  120 ++
 scripts/sz300-test-baseline.json                   |    1 +
 src/lib.rs                                         |    4 +
 219 files changed, 32576 insertions(+), 54 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）（缓存命中 6fa1c64b88d11f2c）

## 评审意见

### 1. sccache 集成导致编译失败（critical）

**问题描述**
`.cargo/config.toml` 中全局设置 `rustc-wrapper = "sccache"`，但未确保所有环境（本地、CI）都安装了 sccache。已发现 `compile-error` 和 `test-failure` 均因 `sccache` 执行失败。虽然 `.githooks/pre-commit` 做了降级处理，但 CI 工作流（如 `ci.yml`）未安装 sccache，导致 `cargo test` 等命令直接失败。

**修改建议**
- 在 CI 中统一安装 sccache，或使用条件判断动态设置 `RUSTC_WRAPPER`。
- 更稳妥的做法：不在全局配置中强制启用，而是通过环境变量或 CI 脚本显式设置，并允许未安装时自动回退。

```yaml
# .github/workflows/ci.yml 中增加步骤
- name: Install sccache
  run: |
    if ! command -v sccache >/dev/null 2>&1; then
      cargo install sccache
    fi
    echo "RUSTC_WRAPPER=sccache" >> $GITHUB_ENV
```

或修改 `.cargo/config.toml`，使用 `cfg` 条件（但 Cargo 不支持），建议改为在 CI 脚本中设置环境变量，而非全局配置。

---

### 2. 生产代码 18 处裸 `unwrap`（high）

**问题描述**
已发现 `bare-unwrap` 问题，生产代码存在 18 处裸 `unwrap`，可能导致 panic。PR 中未涉及修复，但作为资深工程师应指出。

**修改建议**
- 使用 `expect` 提供上下文，或改用 `?` 传播错误。
- 对于不可失败场景，使用 `unwrap_or_else` 或 `ok_or` 等安全方式。

```rust
// 原代码
let value = map.get("key").unwrap();

// 建议
let value = map.get("key").ok_or_else(|| anyhow!("key not found"))?;
```

---

### 3. 敏感字段暴露（critical）

**问题描述**
已发现 `sensitive-field` 问题，2 处敏感字段可能泄露（如 API key、密码）。PR 中未处理。

**修改建议**
- 使用 `secrecy` crate 包装敏感数据，避免 Debug/Display 泄露。
- 确保日志和错误信息不输出敏感字段。

```rust
use secrecy::{Secret, ExposeSecret};

struct Config {
    api_key: Secret<String>,
}

impl Debug for Config {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.debug_struct("Config")
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}
```

---

### 4. 无断言测试（low）

**问题描述**
24 个测试无断言，形同虚设，无法验证行为。

**修改建议**
- 为每个测试补充明确的断言，或删除无意义的测试。
- 使用 `assert!`、`assert_eq!` 等验证预期结果。

```rust
#[test]
fn test_add() {
    let result = add(2, 3);
    assert_eq!(result, 5);
}
```

---

### 5. 空白/冲突标记错误（medium）

**问题描述**
`concurrent_test.ps1:50` 存在空白/冲突标记错误（如 `<<<<<<<` 残留），影响可维护性。

**修改建议**
- 检查并清理冲突标记，确保文件格式正确。
- 在 CI 中添加 whitespace 检查（如 `git diff --check`）。

```bash
# 在 pre-commit 或 CI 中
git diff --check --exit-code
```

---

## 整体评分：4/10

**理由**：
- 存在 critical 级编译失败和敏感字段泄露，阻断发布。
- 大量裸 `unwrap` 和无断言测试降低代码质量。
- 新增的 CI 工作流存在重复和潜在路径问题，但方向正确。
- 建议优先修复 sccache 集成和敏感字段问题，再处理其他项。


## 结论
❌ **阻塞**: 2 个 ≥ medium 级别问题，禁止合入
