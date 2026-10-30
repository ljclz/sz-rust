# PR 审查报告（2026-09-22，branch: main，range: origin/main..HEAD）

> 审查时点: `HEAD @ 1dc852f`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 2 low）

- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），sz300 单元测试移交企业版仓库流程，本次仅测 facade
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），jobs_integration_test 移交企业版仓库流程


## 补充信息

## 变更集
```
 Cargo.lock                                         |   1 +
 docs/CHANGELOG.md                                  |  24 +-
 packages/sz-rust-cli/Cargo.toml                    |   1 +
 packages/sz-rust-cli/src/cargo_checker.rs          |  51 ++
 packages/sz-rust-cli/src/cli.rs                    |  57 +-
 packages/sz-rust-cli/src/cmd/admin.rs              |  42 ++
 packages/sz-rust-cli/src/cmd/make.rs               | 396 ++++++++++++++
 packages/sz-rust-cli/src/cmd/migrate.rs            |  61 +++
 packages/sz-rust-cli/src/cmd/serve.rs              | 191 +++++++
 packages/sz-rust-cli/src/cmd/serve/watcher.rs      |  59 ++
 packages/sz-rust-cli/src/console.rs                |  22 +
 packages/sz-rust-cli/src/validator.rs              |  27 +
 packages/sz-rust-cli/tests/cli_db_integration.rs   | 609 +++++++++++++++++++++
 packages/sz-rust-marketplace/src/client.rs         | 156 ++++++
 .../tests/repository_integration.rs                | 320 +++++++++++
 .../tests/service_integration.rs                   | 502 +++++++++++++++++
 .../sz-rust-marketplace/tests/web_integration.rs   | 580 ++++++++++++++++++++
 packages/sz-rust-marketplace/tests/web_tests.rs    |  66 +++
 scripts/pg_tunnel.js                               |  31 ++
 19 files changed, 3184 insertions(+), 12 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）（缓存命中 9393ea0d7ecf5f33）

## 评审意见

### 1. 【Critical】编译错误未解决，集成测试无法编译通过

**相关静态问题**：
- `error: unused variable: 'version_id'`
- `error: could not compile 'sz-rust-marketplace' (test "service_integration") due to 1 previous error`
- `warning: build failed, waiting for other jobs to finish...`

**问题说明**
PR 中 `sz-rust-marketplace` 的集成测试存在未使用变量 `version_id`，导致整个 crate 编译失败。这直接阻塞了所有测试的执行，也意味着 CHANGELOG 中声称的覆盖率数据（91.51%）尚未被实际验证，存在“虚假通过”风险。

**影响**
- CI 无法通过，任何功能变更都无法安全合并。
- 覆盖率声明缺乏可信依据，门禁拒绝是合理的。

**修改建议**
删除未使用的变量，或将其替换为 `_version_id`。若该变量原本应用于后续断言，请补全使用逻辑。

```rust
// service_integration.rs（示例修复）
// 原代码（可能形式）：
let version_id = service.create_version().await?;

// 修复方式一：直接删除
service.create_version().await?;

// 修复方式二：使用变量（例如用于断言）
let version_id = service.create_version().await?;
assert!(version_id.is_valid());
```

---

### 2. 【Medium】存在无断言测试，测试有效性不足

**相关静态问题**：
- `low|gate|assertion-value|❌ 发现 6 个无断言测试`

**问题说明**
静态检查发现 6 个测试没有任何 `assert` 或等价验证。这类“空洞测试”即使执行也不会捕获回归，只会增加维护成本。从 diff 中可见新增测试大多有断言，但仍有存量或新增无断言测试被遗漏。

**影响**
- 单测丧失回归保护能力，代码重构时错误可能被漏检。
- 违反“铁律 10/23”门禁，CI 无法通过。

**修改建议**
为每个无断言测试补充真实的期望验证；若测试目标无输出，可显式触发副作用并断言其结果（如返回 `Result` 应当为 `Ok`、日志应输出特定内容）。

```rust
// 无断言示例（当前）：
#[tokio::test]
async fn test_plugin_login_without_token() {
    let cli = Cli { /* ... */ };
    let _ = cli.execute().await;
}

// 修改后（补充断言）：
#[tokio::test]
async fn test_plugin_login_without_token() {
    let cli = Cli { /* ... */ };
    let result = cli.execute().await;
    assert!(result.is_err(), "缺少 token 应当返回错误");
    assert!(result.unwrap_err().to_string().contains("token is required"));
}
```

---

### 3. 【Medium】集成测试依赖真实 PostgreSQL，存在安全与并发风险

**相关静态问题**
CHANGELOG 中明示“repository.rs / service.rs 使用真实 PG 18 进行集成测试”，这引入了外部服务依赖。

**问题说明**
- 测试连接到真实 PostgreSQL（通过 SSH 隧道），可能污染生产/共享数据。
- 多条测试并行执行时，若共享同一数据库，可能造成数据竞争和相互干扰。
- SSH 隧道和真实 DB 使测试环境复杂，难以在 CI 中复现，导致测试 flaky。

**影响**
- 并发测试可能产生错误结果，降低测试可靠性。
- 需要额外配置安全凭据，存在凭据泄露风险。
- CI 环境若无相应 DB 服务，测试将失败，阻碍交付。

**修改建议**
使用 `sqlx::test` 宏或测试容器（如 testcontainers）为每个测试创建独立数据库实例。若必须使用共享数据库，请确保每个测试运行在隔离事务中，并在测试结束后回滚。

```rust
// 推荐：使用 sqlx 官方测试宏（需要新增 dev-dependency）
#[sqlx::test]
async fn test_repository_crud(pool: PgPool) {
    let repo = Repository::new(pool.clone());
    // 每个测试自动获得独立数据库，完成后自动销毁
    let id = repo.insert(...).await.unwrap();
    assert!(repo.get(id).await.is_some());
}
```

---

### 4. 【Medium】`cargo_checker` 测试执行真实 `cargo check`，性能差且脆弱

**相关静态问题**
PR 在 `cargo_checker.rs` 新增了两个测试，它们调用 `CargoChecker::check(temp.path())`，这会实际运行 `cargo` 命令，编译临时项目。

**问题说明**
- 每次测试都要启动 cargo、解析依赖、编译，耗时可能从数秒到数十秒，拖慢测试套件。
- 依赖网络（拉取 crates.io 索引），离线条件下测试失败。
- 真实 `cargo` 的版本和宿主环境会影响结果，导致测试结果不稳定。
- 这类测试更适合作为集成测试（`tests/`）或标记为 `#[ignore]`。

**影响**
- 开发者运行单元测试时体验差（等待时间长）。
- CI 中可能因网络/环境问题产生 `flaky` 失败。

**修改建议**
重构 `CargoChecker` 以支持注入命令执行器，测试中用伪造的进程输出替代真实 `cargo`。

```rust
// 在 CargoChecker 中引入 command runner trait
trait CommandRunner {
    fn run(&self, cmd: &mut std::process::Command) -> std::process::Output;
}

struct RealRunner;
impl CommandRunner for RealRunner {
    fn run(&self, cmd: &mut std::process::Command) -> std::process::Output {
        cmd.output().expect("failed to run command")
    }
}

struct MockRunner { output: std::process::Output }
impl CommandRunner for MockRunner {
    fn run(&self, _cmd: &mut std::process::Command) -> std::process::Output {
        self.output.clone()
    }
}

// 测试中：
#[tokio::test]
async fn test_check_simple_project_success() {
    let output = std::process::Output {
        status: std::process::ExitStatus::from_raw(0),
        stdout: b"".to_vec(),
        stderr: b"".to_vec(),
    };
    let checker = CargoChecker::new(Arc::new(MockRunner { output }));
    let result = checker.check(temp.path()).await;
    assert!(result.is_ok());
}
```

---

### 5. 【Low】直接添加 `sqlx` 依赖，存在重复依赖与版本冲突风险

**相关静态问题**
Cargo.lock 中新增 `sqlx`，同时 `sz-rust-cli/Cargo.toml` 添加 `sqlx = { workspace = true }`。

**问题说明**
工作区中已有 `sz-orm-sqlx`，它本身极大概率依赖 `sqlx`。现在又在 CLI 中直接依赖 `sqlx`，可能导致：
- 工作区中 `sqlx` 出现两个不同版本（一个经由 `sz-orm-sqlx` 间接引入，一个直接引入），增加编译时间与二进制体积。
- 未来若需要升级 `sqlx` 版本，必须同步处理所有依赖方，容易遗漏。

**影响**
- 可维护性下降，依赖树复杂化。
- 若版本冲突，编译器可能报错或导致运行时错误。

**修改建议**
尽量复用 `sz-orm-sqlx` 中导出的类型，避免直接暴露 `sqlx`。若确实需要直接使用 `sqlx` 的 API（如 `PgPool`），请确认它来自与 `sz-orm-sqlx` 相同的版本，并统一管理。

```toml
# 在 sz-rust-cli/Cargo.toml 中，优先使用 sz-rust-orm-facade 重导出的 sqlx 类型
sz-rust-orm-facade = { workspace = true }
# 避免直接添加 sqlx，除非有明确的版本对齐需求
# sqlx = { workspace = true }  # 建议移除
```

如果必须保留，则应在 workspace 根 `Cargo.toml` 中声明统一版本：

```toml
# workspace.dependencies
sqlx = { version = "0.8", features = ["postgres", "runtime-tokio"] }
```

---

## 整体评分：4 / 10

**评分依据**：
- 致命扣分：存在编译错误，CI 无法通过；覆盖率声明真实性存疑。
- 主要加分：PR 显著提升了测试覆盖率和产物质量（如幻影测试修复），方向正确。
- 其他扣分：无断言测试、外部 DB 依赖、测试稳定性问题仍未妥善解决。

因此，该 PR 当前状态 **不可合并**，建议修复上述问题后重新评审。


## 结论
✅ 通过（无 ≥ medium 级别问题）
