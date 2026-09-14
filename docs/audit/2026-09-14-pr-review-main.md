# PR 审查报告（2026-09-14，branch: main，range: HEAD~3..HEAD）

> 审查时点: `HEAD @ 8d2d72c`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 2 low）

- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），sz300 单元测试移交企业版仓库流程，本次仅测 facade
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），jobs_integration_test 移交企业版仓库流程


## 补充信息

## 变更集
```
 .gitignore                              |   9 +
 CHANGELOG.md                            |  53 ++++
 Cargo.lock                              | 368 +++++++++++++++++++++-
 Cargo.toml                              |   8 +-
 docs/audit/doc-debt.md                  |   2 +-
 packages/sz-rust-cli/Cargo.toml         |   8 +-
 packages/sz-rust-cli/src/cli.rs         |   3 +-
 packages/sz-rust-cli/src/cmd/admin.rs   |  26 +-
 packages/sz-rust-cli/src/cmd/migrate.rs | 533 ++++++++++++++++++++------------
 9 files changed, 785 insertions(+), 225 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

## PR 评审报告

### 最重要的潜在问题

#### 1. 【安全】`std::mem::forget` 阻止 Oracle 连接池 drop 导致资源泄漏

**问题描述**：CHANGELOG 中明确提到"用 `std::mem::forget` 阻止 `OracleBlockingPool` 的 tokio Runtime 在 async 上下文中 drop panic"。这是典型的"用内存泄漏掩盖错误"模式——`mem::forget` 会永久泄漏连接池及其内部的所有资源（连接、线程、内存），在长时间运行的 CLI 进程或服务中会导致不可恢复的资源耗尽。

**修改建议**：
```rust
// 错误做法：mem::forget 泄漏资源
let pool = OraclePoolHandle::new(config)?;
std::mem::forget(pool);

// 正确做法：使用 ManuallyDrop 或显式管理生命周期
// 方案1：将 pool 放入结构体，实现 Drop 时在正确的 runtime 上下文中清理
struct OracleConnectionGuard {
    pool: Option<OraclePoolHandle>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Drop for OracleConnectionGuard {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            rt.block_on(async {
                if let Some(pool) = self.pool.take() {
                    pool.close().await; // 显式关闭
                }
            });
        }
    }
}

// 方案2：如果必须绕过 AnyPool，使用 tokio::runtime::Handle 捕获
// 并在 async 块内显式 drop，而不是 forget
async fn create_oracle_pool(config: &OracleConfig) -> Result<OraclePoolHandle> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let pool = rt.block_on(async { OraclePoolHandle::new(config).await })?;
    // 将 rt 和 pool 一起返回，由调用方管理生命周期
    Ok((pool, rt))
}
```

---

#### 2. 【安全】`#[serde(skip_serializing)]` 与手工 Debug 实现可能遗漏其他泄露路径

**问题描述**：虽然对 `CreateUserRequest`/`UpdateUserRequest` 的 `password` 字段做了 `skip_serializing` 和 Debug 脱敏，但只覆盖了 serde 序列化和 `{:?}` 输出。其他泄露路径包括：`Display` trait、`to_string()`、日志宏中的 `{}` 格式化、错误类型中携带的字段、以及 `Clone` 后传递到其他模块。

**修改建议**：
```rust
// 1. 使用零值安全的类型包装，而非依赖手工实现
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Password(String);

impl Password {
    pub fn new(raw: &str) -> Self {
        Self(raw.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl std::fmt::Display for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

// 2. 在 DTO 中使用包装类型
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    #[serde(skip_serializing)]
    pub password: Password,  // 类型本身保证脱敏
}

// 3. 添加全局防护：在测试中验证
#[cfg(test)]
mod tests {
    #[test]
    fn password_never_leaks() {
        let req = CreateUserRequest {
            username: "test".into(),
            password: Password::new("secret"),
        };
        let debug_str = format!("{:?}", req);
        assert!(!debug_str.contains("secret"));

        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("secret"));
    }
}
```

---

#### 3. 【并发】EnvGuard 文件级 Mutex 串行化测试，但未覆盖所有环境变量修改路径

**问题描述**：CHANGELOG 提到"文件级 tokio Mutex 串行化 9 测试"解决 HOME/USERPROFILE 竞态，但只针对 `plugin_behavior` 测试。如果其他测试文件（或非测试代码）也修改进程级环境变量，仍会存在竞态。且文件级 Mutex 在不同测试二进制之间不共享（每个测试二进制是独立进程）。

**修改建议**：
```rust
// 1. 使用进程级锁（跨测试文件）
// 在测试公共模块中定义：
pub static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

pub fn with_env_guard<T>(envs: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
    let lock = ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()));
    let _guard = lock.lock().unwrap();

    // 保存原始值
    let originals: Vec<(String, Option<String>)> = envs
        .iter()
        .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
        .collect();

    // 设置新值
    for (k, v) in envs {
        std::env::set_var(k, v);
    }

    // 使用 defer 模式恢复
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

    // 恢复原始值
    for (k, original) in originals {
        match original {
            Some(v) => std::env::set_var(&k, v),
            None => std::env::remove_var(&k),
        }
    }

    result.unwrap_or_else(|e| std::panic::resume_unwind(e))
}

// 2. 更彻底的方案：使用 serial_test crate 的 #[serial] 属性
// 在 Cargo.toml 中添加 dev-dependency
// [dev-dependencies]
// serial_test = "3"

// 然后在测试中使用：
#[test]
#[serial]
fn test_plugin_behavior_home_env() {
    // 此测试会与其他 #[serial] 测试串行执行
}

// 3. 最佳实践：避免修改进程级环境变量，改为在测试中注入配置
// 重构代码使配置通过参数传递，而非从环境变量读取
```

---

#### 4. 【可维护性】CHANGELOG 中 `[Unreleased]` 重复出现且日期混乱

**问题描述**：CHANGELOG 中有两个 `[Unreleased]` 区块（2026-09-14 和 2026-09-13），这违反了 Keep a Changelog 规范——`Unreleased` 应该只有一个，且不应有日期。多个 `Unreleased` 区块会导致版本发布时难以确定哪些变更属于哪个版本。

**修改建议**：
```markdown
## [Unreleased]

### Security
- **Admin 请求 DTO password 双层脱敏**：...
- **gitignore 敏感文件防护**：...

### Fixed
- **审查门禁代码修复**：...
- **plugin_behavior 测试环境变量竞态**：...
- **nul 保留名文件**：...

### Changed
- **审查门禁体系适配开源/企业版分离**：...
- **k8s-operator 孤儿 crate 物理删除**：...
- **覆盖率基线更正**：...
- **pay.rs 变异测试排除解除**：...

### Removed
- `sz-rust-k8s-operator`：...

## [0.x.y] - 2026-09-13

### Added
- **CLI 多后端数据库连接支持（Oracle/MSSQL）**：...

### Changed
- **sz-orm-core/sz-orm-sqlx 升级到 v6.9.0**：...
```

---

#### 5. 【性能】Oracle 连接池 `mem::forget` 导致每次 CLI 调用泄漏完整连接池

**问题描述**：CLI 工具（如 `migrate` 命令）通常是短生命周期进程。如果每次执行都 `mem::forget` 一个 Oracle 连接池，意味着每次 CLI 调用都会泄漏：
- 至少一个 tokio Runtime（线程池）
- Oracle 连接池中的所有数据库连接
- 相关内存和文件描述符

对于 CI/CD 中频繁执行的迁移命令，这会导致服务器端连接数持续增长，最终耗尽 Oracle 的进程/会话限制。

**修改建议**：
```rust
// 在 CLI 命令执行路径中，确保连接池在命令结束后被正确清理
pub async fn run_migrate(args: MigrateArgs) -> Result<()> {
    let pool = create_connection(&args).await?;

    // 使用 scopeguard 或 RAII 确保清理
    let result = execute_migrations(&pool, &args).await;

    // 显式关闭连接池
    if let Some(oracle_pool) = pool.as_any().downcast_ref::<OraclePoolHandle>() {
        oracle_pool.close().await?;
    }

    result
}

// 或者使用 tokio::runtime::Handle 在正确的上下文中 drop
pub fn run_migrate_sync(args: MigrateArgs) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let pool = create_connection(&args).await?;
        let result = execute_migrations(&pool, &args).await;
        // 在 async 上下文中正常 drop，不 panic
        drop(pool);
        result
    })
}
```

---

### 整体评分：**5/10**

**评分理由**：

| 维度 | 评分 | 说明 |
|------|------|------|
| 安全性 | 4/10 | 有安全意识（脱敏、gitignore），但 `mem::forget` 是严重反模式 |
| 性能 | 3/10 | Oracle 连接池泄漏会导致长期运行的服务资源耗尽 |
| 可维护性 | 6/10 | CHANGELOG 详细，但结构混乱；测试串行化方案可接受 |
| 并发正确性 | 5/10 | 解决了已知竞态，但方案不彻底（文件级锁不跨进程） |
| 代码质量 | 6/10 | 有测试覆盖和 CI 门禁，但关键路径存在资源管理问题 |

**核心问题**：`std::mem::forget` 的使用是必须修复的阻断项。这不仅影响性能，更是潜在的生产事故源。建议在合并前解决此问题，并补充资源泄漏测试（如连接数断言）。


## 结论
✅ 通过（无 ≥ medium 级别问题）
