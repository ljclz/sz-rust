# PR 审查报告（2026-09-20，branch: main，range: origin/main..HEAD）

> 审查时点: `HEAD @ d416e93`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 2 low）

- [low] `gate` **doc-code-consistency**: ❌ 发现 3 处幻影交付声称（违反铁律 23），请核实：crate 是否应存在、是否需标注企业版�
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），sz300 单元测试移交企业版仓库流程，本次仅测 facade
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），jobs_integration_test 移交企业版仓库流程


## 补充信息

## 变更集
```
 CHANGELOG.md                                       |   9 +
 Cargo.lock                                         |  84 ++---
 Cargo.toml                                         |  34 +-
 README.md                                          |   1 +
 docs/CHANGELOG.md                                  |  90 ++++++
 ...256\236\351\231\205\350\257\204\344\274\260.md" | 208 ++++++++++++
 ...257\271\346\257\224\345\210\206\346\236\220.md" | 112 +++++++
 packages/sz-rust-ai-facade/src/agent/engine.rs     | 291 +++++++++++++++++
 packages/sz-rust-ai-facade/src/common/audit.rs     | 121 +++++++
 packages/sz-rust-ai-facade/src/common/metrics.rs   |  93 ++++++
 packages/sz-rust-ai-facade/src/embedding/local.rs  | 133 ++++++++
 packages/sz-rust-ai-facade/src/facade.rs           |   1 +
 packages/sz-rust-ai-facade/src/rag/citation.rs     |  48 +++
 packages/sz-rust-ai-facade/src/rag/pipeline.rs     | 336 +++++++++++++++++++
 packages/sz-rust-ai-facade/src/rag/reranker.rs     |   8 +-
 packages/sz-rust-ai-facade/tests/real_api_test.rs  |  35 +-
 packages/sz-rust-capability/src/capability.rs      |  99 ++++++
 packages/sz-rust-capability/src/facade.rs          | 144 +++++++++
 packages/sz-rust-cli/Cargo.toml                    |  28 +-
 packages/sz-rust-cli/src/cli.rs                    | 105 ++++++
 packages/sz-rust-cli/src/cmd/admin.rs              |  72 +++--
 packages/sz-rust-cli/src/cmd/migrate.rs            |  25 +-
 packages/sz-rust-cli/src/cmd/mod.rs                |   2 +
 packages/sz-rust-cli/src/cmd/serve.rs              | 360 +++++++++++++++++++++
 packages/sz-rust-cli/src/cmd/serve/access_log.rs   | 107 ++++++
 packages/sz-rust-cli/src/cmd/serve/runtime.rs      | 105 ++++++
 packages/sz-rust-cli/src/cmd/serve/signal.rs       | 123 +++++++
 packages/sz-rust-cli/src/cmd/serve/watcher.rs      | 121 +++++++
 packages/sz-rust-cli/tests/admin_serve_wiring.rs   | 179 ++++++++++
 packages/sz-rust-cli/tests/serve_data_scope_e2e.rs |  77 +++++
 packages/sz-rust-cli/tests/serve_health_wiring.rs  | 124 +++++++
 .../sz-rust-cli/tests/serve_production_wiring.rs   | 242 ++++++++++++++
 packages/sz-rust-cli/tests/serve_tenant_e2e.rs     |  79 +++++
 packages/sz-rust-cli/tests/serve_tls_wiring.rs     | 165 ++++++++++
 packages/sz-rust-core/src/server.rs                |  77 ++++-
 packages/sz-rust-examples/Cargo.toml               |   1 +
 packages/sz-rust-examples/src/bin/admin_demo.rs    | 152 ++++++---
 .../sz-rust-examples/src/bin/data_scope_demo.rs    |  76 +++++
 .../sz-rust-examples/src/bin/multi_tenant_demo.rs  |   8 +-
 packages/sz-rust-frontend-codegen/src/config.rs    | 261 +++++++++++++++
 packages/sz-rust-frontend-codegen/src/error.rs     | 152 +++++++++
 .../sz-rust-frontend-codegen/src/metadata/model.rs | 122 +++++++
 .../src/metadata/validation.rs                     | 124 +++++++
 .../sz-rust-frontend-codegen/src/model_parser.rs   | 298 +++++++++++++++++
 .../sz-rust-frontend-codegen/src/path_guard.rs     |  91 ++++++
 packages/sz-rust-infra-facade/src/config.rs        |  18 ++
 packages/sz-rust-marketplace/src/client.rs         |  11 +-
 packages/sz-rust-marketplace/src/error.rs          | 101 ++++++
 packages/sz-rust-marketplace/src/lockfile.rs       |  92 ++++++
 packages/sz-rust-marketplace/src/main.rs           |  11 +-
 packages/sz-rust-marketplace/src/service.rs        |   1 +
 packages/sz-rust-marketplace/src/storage.rs        |  66 ++++
 52 files changed, 5256 insertions(+), 167 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

## PR 评审报告

### 1. 最重要的潜在问题

#### 1.1 依赖版本跳跃（6.x → 7.6.0）未提供迁移说明或兼容性测试
- **风险**：`Cargo.lock` 中 `sz-orm-*` 系列从 6.x 直接升级到 7.6.0，属于主版本升级，可能包含破坏性 API 变更。若项目代码仍使用旧 API，编译或运行时可能失败。PR 中未提及任何迁移步骤或新增测试来验证兼容性。
- **影响**：可能导致整个项目无法构建或运行时行为异常，影响所有依赖这些 crate 的模块。

#### 1.2 幻影交付声称（文档与代码不一致）
- **风险**：CHANGELOG 中声称新增了 `sz-rust serve` 子命令、`build_router_with_admin` 函数及测试，但问题清单指出存在 3 处幻影交付声称（铁律 23）。若实际代码未实现或实现不完整，会误导使用者，且测试可能未真正覆盖。
- **影响**：维护者或用户基于文档使用不存在的功能，导致运行时错误；同时破坏项目可信度。

#### 1.3 `sz300` 不在 workspace members 但涉及集成测试
- **风险**：问题清单指出 `sz-rust-sz300` 不在 workspace members，且 `jobs_integration_test` 被跳过。若 PR 中涉及 `sz300` 相关改动（如新增测试或依赖），这些改动将无法在 CI 中验证，可能引入未发现的缺陷。
- **影响**：核心功能（如 admin 接线）可能依赖 `sz300`，但测试被跳过，导致回归风险。

#### 1.4 `--with-admin` 标志可能引入未授权访问风险
- **风险**：新增的 `serve --with-admin` 会加载 21 个 admin 端点和 17 个 Capability。若未在路由层强制认证/授权，任何访问者都可能调用管理接口，造成严重安全漏洞。
- **影响**：生产环境一旦误用，可导致数据泄露或系统被接管。

#### 1.5 `build_router_with_admin` 声称“纯函数”但可能隐含全局状态
- **风险**：CHANGELOG 强调该函数为纯函数，但若实现中依赖全局配置、环境变量或共享连接池，则并非纯函数，难以测试和推理，且可能引入并发问题。
- **影响**：测试可能因环境差异而失败，或在高并发下出现状态竞争。

---

### 2. 具体修改建议

#### 2.1 针对依赖版本升级
- **建议**：在 PR 描述中明确列出破坏性变更，并添加迁移测试。至少运行一次全量测试，并检查 `Cargo.toml` 中依赖约束是否允许 7.x（如 `^6.0` 会阻止升级，需改为 `^7.0`）。
- **代码示例**（在 `Cargo.toml` 中）：
  ```toml
  [dependencies]
  sz-orm-core = "7.6.0"  # 明确指定新版本
  ```
  并在 CI 中增加 `cargo test --all-features` 确保兼容。

#### 2.2 针对幻影交付声称
- **建议**：核实 `serve` 子命令和 `build_router_with_admin` 是否真实存在。若未实现，请移除 CHANGELOG 中的相关条目；若已实现，请补充代码引用和测试文件路径。
- **代码示例**（检查实现是否存在）：
  ```bash
  grep -r "build_router_with_admin" packages/sz-rust-cli/src/
  grep -r "serve" packages/sz-rust-cli/src/main.rs
  ```
  若缺失，则删除 CHANGELOG 中对应段落。

#### 2.3 针对 `sz300` 不在 workspace
- **建议**：将 `sz-rust-sz300` 加入 workspace members，或明确说明其独立于开源仓库，并在 CI 中跳过相关测试时添加注释。若 `sz300` 是闭源组件，应确保其接口通过 trait 抽象，避免开源代码直接依赖。
- **代码示例**（在根 `Cargo.toml` 中）：
  ```toml
  [workspace]
  members = [
      "packages/sz-rust-cli",
      "packages/sz-rust-examples",
      # 如果 sz300 是开源的，添加：
      # "packages/sz-rust-sz300",
  ]
  ```
  若不开源，则在 CI 配置中显式跳过并记录原因。

#### 2.4 针对 `--with-admin` 安全风险
- **建议**：在 `build_router_with_admin` 中强制要求认证中间件，并默认拒绝未授权访问。至少添加一个 `require_auth` 层，或要求传入 `roles` 参数时校验权限。
- **代码示例**（在路由构建时）：
  ```rust
  pub fn build_router_with_admin(
      pool: Pool,
      roles: Vec<Role>,
  ) -> (Router, usize) {
      let mut router = Router::new();
      // 添加认证中间件
      router = router.layer(middleware::from_fn(require_auth));
      // 注册 admin 端点
      router = router.nest("/api/admin", admin_routes(pool, roles));
      // ...
  }
  ```
  并确保 `require_auth` 检查 JWT 或 session。

#### 2.5 针对“纯函数”声明
- **建议**：将 `build_router_with_admin` 改为接受所有依赖作为参数（如 `pool`、`roles`、`config`），避免隐式全局状态。若必须使用全局，则明确标注 `unsafe` 或使用 `OnceLock`。
- **代码示例**：
  ```rust
  pub fn build_router_with_admin(
      pool: Pool,
      roles: Vec<Role>,
      config: &AppConfig,  // 显式传入配置
  ) -> (Router, usize) {
      // 使用 config 而非环境变量
  }
  ```

---

### 3. 整体评分

**评分：5/10**

- **加分项**：CHANGELOG 更新详细，新增测试意图明确；依赖升级可能带来新特性。
- **减分项**：存在文档与代码不一致、依赖升级风险未评估、安全漏洞隐患、测试覆盖缺失等关键问题，且未提供迁移说明。这些问题可能导致项目无法构建或生产事故，需在合并前解决。


## 结论
✅ 通过（无 ≥ medium 级别问题）
