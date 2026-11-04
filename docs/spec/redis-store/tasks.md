# tasks.md — Redis 存储后端编码任务规划

> **项目**：sz-rust（axum 0.8 + SZ-ORM，对标 ThinkPHP 8 的 Rust Web 框架）
> **版本**：v0.6.2 → v0.6.3（semver 兼容，仅新增 API + 可选 feature）
> **任务规划版本**：tasks-v1.0
> **创建日期**：2026-08-08
> **基于文档**：[spec.md](./spec.md)（spec-v1.0）+ [design.md](./design.md)（design-v1.0）
> **目标 crate**：`sz-rust-auth-facade`（新增 `redis_store.rs` + feature gate `redis-store` / `redis-cluster`）
> **不修改**：上游 `sz-orm` 仓库、`sz-orm-auth` crate、现有 `RefreshTokenStore` / `TokenBlacklist` trait 签名、现有 `MemoryRefreshTokenStore` / `MemoryTokenBlacklist` 实现、`RefreshTokenIssuer` / `Verifier` / `Revoker`

---

## 0. 任务总览

### 0.1 任务清单

| 任务 ID | 任务名称 | 类型 | 预估工时 | 里程碑 | 可并行 |
|---------|----------|------|----------|--------|--------|
| T0 | Cargo.toml feature gate 配置 | 编码 | 0.5h | M1 | — |
| T1 | lib.rs 模块声明 feature gate | 编码 | 0.3h | M1 | 依赖 T0 |
| T2 | RedisConfig 结构体 + Debug 脱敏 | 编码 | 1.5h | M1 | 依赖 T0 |
| T3 | RedisRefreshTokenStore 实现 | 编码 | 2h | M1 | 依赖 T2 |
| T4 | RedisTokenBlacklist 实现 | 编码 | 2h | M1 | 依赖 T2（与 T3 并行） |
| T5 | create_redis_stores 便捷工厂 | 编码 | 0.5h | M1 | 依赖 T3、T4 |
| T6 | 单元测试（mock Redis） | 测试 | 2h | M2 | 依赖 T2-T5 |
| T7 | 契约测试（泛型，对齐 Memory） | 测试 | 1h | M2 | 依赖 T3、T4（与 T6 并行） |
| T8 | 集成测试（真实 Redis） | 测试 | 2h | M2 | 依赖 T6 |
| T9 | 边界测试（断连 / 超时 / 并发） | 测试 | 2h | M2 | 依赖 T8 |
| T10 | 基准测试（criterion bench） | 测试 | 1h | M2 | 依赖 T3、T4（与 T8 并行） |
| T11 | 代码质量验收（clippy / rustdoc / tarpaulin） | 验证 | 1h | M3 | 依赖 T6-T10 |
| T12 | semver 兼容性验证 | 验证 | 0.5h | M3 | 依赖 T11 |
| T13 | sz-pay 兼容性验证 | 验证 | 0.5h | M3 | 依赖 T12（与 T12 串行） |
| T14 | feature gate 隔离验证 | 验证 | 0.5h | M3 | 依赖 T11（与 T12 并行） |
| T15 | 文档更新（rustdoc + CHANGELOG + README） | 文档 | 1h | M4 | 依赖 T11 |
| T16 | 版本 bump（0.6.2 → 0.6.3） | 发布 | 0.3h | M4 | 依赖 T15 |
| T17 | 发布到 crates.io | 发布 | 0.5h | M4 | 依赖 T16 |

**总预估工时**：约 19.6 小时（含验证与发布）

### 0.2 依赖拓扑图

```
T0 (Cargo.toml feature)
├── T1 (lib.rs 模块声明)
└── T2 (RedisConfig)
    ├── T3 (RedisRefreshTokenStore) ──┐
    │                                  ├── T5 (create_redis_stores)
    └── T4 (RedisTokenBlacklist) ──────┘
        │
        ├── T7 (契约测试) ─────────────┐
        └── T10 (基准测试) ────────────┤
                                         │
T3 + T4 + T5 ── T6 (单元测试) ── T8 (集成测试) ── T9 (边界测试)
                                         │
T6 + T7 + T8 + T9 + T10 ── T11 (代码质量验收)
                            ├── T12 (semver 验证) ── T13 (sz-pay 兼容)
                            └── T14 (feature 隔离验证)
                                         │
T11 + T12 + T13 + T14 ── T15 (文档更新) ── T16 (版本 bump) ── T17 (发布)
```

### 0.3 里程碑定义

| 里程碑 | 定义 | 包含任务 | 验收标准 |
|--------|------|----------|----------|
| **M1: 基础实现完成** | 所有新增代码编写完成，`cargo build --features redis-store` 编译通过 | T0-T5 | 编译零错误零警告，`RedisRefreshTokenStore` / `RedisTokenBlacklist` / `RedisConfig` 可实例化 |
| **M2: 测试通过** | 单元测试 + 契约测试 + 集成测试 + 边界测试 + 基准测试全部通过 | T6-T10 | `cargo test --features redis-store` 全绿，覆盖率 ≥ 90%，基准测试 p99 < 5ms |
| **M3: 集成验证** | 代码质量、semver 兼容、sz-pay 兼容、feature 隔离全部验证通过 | T11-T14 | clippy 零警告，rustdoc 零警告，`cargo-semver-checks` 通过，sz-pay 编译成功，默认构建零 Redis 依赖 |
| **M4: 发布** | 文档更新 + 版本 bump + crates.io 发布完成 | T15-T17 | CHANGELOG 更新，版本 0.6.3，crates.io 发布成功，`cargo publish --dry-run` 通过 |

---

## 1. 基础设施任务（M1: 基础实现完成）

### 1.1 T0: Cargo.toml feature gate 配置

- [ ] 在 `packages/sz-rust-auth-facade/Cargo.toml` 的 `[features]` 段新增 `redis-store` feature，定义为 `redis-store = ["dep:redis"]`，复用现有 `redis = { workspace = true, optional = true }` 依赖声明（`Cargo.toml:22`），不新增重复依赖
- [ ] 在 `[features]` 段新增 `redis-cluster` feature，定义为 `redis-cluster = ["redis-store", "redis/cluster"]`，隐含 `redis-store` + 启用 `redis` crate 的 `cluster` feature
- [ ] 验证 `default = []` 保持不变（默认不启用 `redis-store`，零 Redis 依赖）
- [ ] 验证 `redis-store` 与现有 `redis-gateway` feature 正交，可单独启用或同时启用（FR-5.5）

**输入**：现有 `Cargo.toml`（`redis-gateway` / `axum` / `remote-validate` feature）
**输出**：新增 2 行 feature 定义
**验收标准**：`cargo build --features redis-store` 编译通过（此时 `redis_store.rs` 尚未创建，仅验证 feature 定义正确）；`cargo tree --no-default-features` 无 `redis` 节点（AC-1.13）

### 1.2 T1: lib.rs 模块声明 feature gate

- [ ] 在 `packages/sz-rust-auth-facade/src/lib.rs` 追加 `#[cfg(feature = "redis-store")] pub mod redis_store;`，对齐现有 `redis_gateway` 模块的 feature gate 模式（`lib.rs:43-44`）
- [ ] 添加模块级 rustdoc 注释：`/// Redis 存储后端（需启用 `redis-store` feature）`，说明提供 `RedisRefreshTokenStore` / `RedisTokenBlacklist` / `RedisConfig`

**输入**：T0 完成（feature 已定义）
**输出**：`lib.rs` 新增 2-3 行（含注释）
**验收标准**：`cargo build --features redis-store` 报 `redis_store.rs` 不存在错误（预期，T2 创建文件后消除）；`cargo build`（默认）不编译 `redis_store` 模块

### 1.3 T2: RedisConfig 结构体 + Debug 脱敏

- [ ] 创建 `packages/sz-rust-auth-facade/src/redis_store.rs` 文件，文件头添加模块级 rustdoc 注释说明对齐 spec.md FR-1 ~ FR-6
- [ ] 定义 `RedisConfig` 结构体，字段：`url: String`、`key_prefix_ver: String`、`key_prefix_bl: String`、`connection_timeout: Duration`、`command_timeout: Duration`（FR-4.1），派生 `Clone`
- [ ] 实现 `RedisConfig::default()` 返回：`url = "redis://127.0.0.1:6379"`、`key_prefix_ver = "sso:ver"`、`key_prefix_bl = "sso:bl"`、`connection_timeout = 3s`、`command_timeout = 2s`（FR-4.2 / AC-1.10）
- [ ] 实现 `RedisConfig::from_url(url: impl Into<String>) -> Self` 便捷构造，仅设置 `url`，其余字段使用默认值（FR-4.5）
- [ ] 手动实现 `std::fmt::Debug for RedisConfig`，将 `url` 中嵌入的密码脱敏为 `redis://:[REDACTED]@host:port/db`（FR-4.3 / NFR-2.1 / AC-1.11），复用 `SsoJwtCodec` 脱敏模式（`refresh.rs:270-276`，`finish_non_exhaustive()`）
- [ ] 实现 `RedisConfig::connect(&self) -> Result<ConnectionManager, RefreshTokenError>` 异步方法，解析 `url` 创建 `redis::Client`，调用 `client.get_async_connection_manager()`，用 `tokio::time::timeout(connection_timeout, ...)` 包装建连，连接失败 / 超时 / 认证失败统一返回 `Err(RefreshTokenError::ServiceUnavailable)`（FR-3.3 / FR-3.5）
- [ ] 为所有公开 API 添加 rustdoc 注释（C-11），包括参数说明、返回值、错误条件、示例代码

**输入**：T0/T1 完成；workspace `redis` 依赖已含 `connection-manager` feature
**输出**：`redis_store.rs` 新文件，约 80-100 行
**验收标准**：`cargo build --features redis-store` 编译通过；`RedisConfig::default()` 字段值正确；`format!("{:?}", RedisConfig::from_url("redis://:secret@host:6379"))` 不含 `secret`，含 `[REDACTED]`

### 1.4 T3: RedisRefreshTokenStore 实现

- [ ] 定义 `RedisRefreshTokenStore` 结构体，字段：`conn: ConnectionManager`、`key_prefix_ver: String`、`command_timeout: Duration`（design.md §2.2.2.2），派生 `Clone`（`ConnectionManager` 已 `Clone`）
- [ ] 实现 `RedisRefreshTokenStore::new(config: &RedisConfig) -> Result<Self, RefreshTokenError>` 异步方法，内部调用 `config.connect()` 获取 `ConnectionManager`，组装实例（design.md §2.2.2.2）
- [ ] 实现 `RedisRefreshTokenStore::with_conn(conn: ConnectionManager, key_prefix_ver: impl Into<String>, command_timeout: Duration) -> Self`，复用已有 `ConnectionManager`（多 store 共享连接池）
- [ ] 使用 `#[async_trait::async_trait]` 实现 `RefreshTokenStore` trait（trait 定义在 `refresh.rs:322`，不修改）：
  - [ ] `get_version(&self, user_id: i64) -> Result<u64, RefreshTokenError>`：构造 key `{key_prefix_ver}:{user_id}`，执行 `tokio::time::timeout(command_timeout, conn.get::<_, Option<String>>(key))`，超时 / 命令失败返回 `Err(ServiceUnavailable)`（FR-1.5），key 不存在返回 `Ok(0)`（FR-1.2 / AC-1.1，对齐 `MemoryRefreshTokenStore` `refresh.rs:353`），key 存在解析为 `u64`，解析失败返回 `Err(Cache(String))`
  - [ ] `increment_version(&self, user_id: i64) -> Result<u64, RefreshTokenError>`：构造 key，执行 `tokio::time::timeout(command_timeout, conn.incr::<_, i64>(key))`（Redis `INCR` 原子递增，FR-1.3 / FR-1.6 / C-14），超时 / 命令失败返回 `Err(ServiceUnavailable)`，`INCR` 返回 `i64`，负数返回 `Err(ServiceUnavailable)`（理论不可能），`> u64::MAX` 返回 `Err(ServiceUnavailable)`（NFR-4.5 溢出处理），否则 `Ok(new_ver as u64)`
- [ ] 添加 `#[tracing::instrument(skip(self), fields(user_id = user_id))]` 日志标注（C-12），仅记录 `user_id` / 操作类型 / 错误类型，不记录 Redis URL（NFR-2.2）
- [ ] 为所有公开 API 添加 rustdoc 注释，说明线程安全保证（`ConnectionManager` 满足 `Send + Sync + Clone`）、原子性保证（`INCR` 单命令原子）、行为对齐 `MemoryRefreshTokenStore`

**输入**：T2 完成（`RedisConfig` 可用）
**输出**：`redis_store.rs` 新增约 60-80 行
**验收标准**：`cargo build --features redis-store` 编译通过；`RedisRefreshTokenStore` 满足 `Send + Sync`（C-1）；所有 `async fn` 返回 `Send + 'static` Future

### 1.5 T4: RedisTokenBlacklist 实现

- [ ] 定义 `RedisTokenBlacklist` 结构体，字段：`conn: ConnectionManager`、`key_prefix_bl: String`、`command_timeout: Duration`（design.md §2.2.2.3），派生 `Clone`
- [ ] 实现 `RedisTokenBlacklist::new(config: &RedisConfig) -> Result<Self, RefreshTokenError>` 异步方法，调用 `config.connect()` 获取 `ConnectionManager`
- [ ] 实现 `RedisTokenBlacklist::with_conn(conn: ConnectionManager, key_prefix_bl: impl Into<String>, command_timeout: Duration) -> Self`，复用已有 `ConnectionManager`
- [ ] 使用 `#[async_trait::async_trait]` 实现 `TokenBlacklist` trait（trait 定义在 `refresh.rs:369`，不修改）：
  - [ ] `revoke(&self, jti: &str, ttl_secs: u64) -> Result<(), RefreshTokenError>`：若 `ttl_secs == 0` 直接返回 `Ok(())`（FR-2.4 / AC-1.7 幂等跳过，节省存储）；否则构造 key `{key_prefix_bl}:{jti}`，执行 `tokio::time::timeout(command_timeout, conn.set_ex::<_, _, ()>(key, "1", ttl_secs))`（Redis `SETEX` 带 TTL 写入，FR-2.3），超时 / 命令失败返回 `Err(ServiceUnavailable)`（FR-2.6），成功返回 `Ok(())`（`SETEX` 覆盖写入天然幂等，FR-2.8）
  - [ ] `is_revoked(&self, jti: &str) -> Result<bool, RefreshTokenError>`：构造 key，执行 `tokio::time::timeout(command_timeout, conn.exists::<_, bool>(key))`（Redis `EXISTS` 存在性检查，FR-2.2），超时 / 命令失败返回 `Err(ServiceUnavailable)`，成功返回 `Ok(exists)`（TTL 过期 key 自动不存在，AC-1.6）
- [ ] 添加 `#[tracing::instrument(skip(self), fields(jti = jti))]` 日志标注（C-12），仅记录 `jti` / 操作类型 / 错误类型
- [ ] 为所有公开 API 添加 rustdoc 注释，说明 TTL 由调用方传入（FR-2.7 / C-15）、幂等性（FR-2.8）、自动过期（NFR-4.3）

**输入**：T2 完成（`RedisConfig` 可用）
**输出**：`redis_store.rs` 新增约 60-80 行
**验收标准**：`cargo build --features redis-store` 编译通过；`RedisTokenBlacklist` 满足 `Send + Sync`；`revoke("jti", 0)` 直接返回 `Ok(())` 不调用 Redis

### 1.6 T5: create_redis_stores 便捷工厂

- [ ] 实现 `create_redis_stores(config: &RedisConfig) -> Result<(RedisRefreshTokenStore, RedisTokenBlacklist), RefreshTokenError>` 异步函数，调用 `config.connect()` 创建单一 `ConnectionManager`，用 `with_conn` 构造 store + blacklist 对，二者共享同一连接池（design.md §2.2.2.4）
- [ ] 添加 rustdoc 注释说明共享 `ConnectionManager` 避免双连接池开销

**输入**：T3、T4 完成
**输出**：`redis_store.rs` 新增约 10-15 行
**验收标准**：`cargo build --features redis-store` 编译通过；返回的 store 与 blacklist 持有同一 `ConnectionManager`（`Clone` 共享，非新建连接）

---

## 2. 测试任务（M2: 测试通过）

### 2.1 T6: 单元测试（mock Redis，不依赖真实实例）

- [ ] 在 `redis_store.rs` 末尾添加 `#[cfg(test)] mod unit_tests` 模块，使用 `redis` crate 的同步 `Connection` 或内嵌 mock（不依赖真实 Redis 实例，AC-3.1）
- [ ] 编写 `test_redis_config_default`：验证 `default()` 返回正确默认值（AC-1.10）
- [ ] 编写 `test_redis_config_from_url`：验证 `from_url` 仅设置 `url`，其余字段默认（FR-4.5）
- [ ] 编写 `test_redis_config_debug_redacts_password`：验证 `Debug` 输出不含密码，含 `[REDACTED]`（AC-1.11 / NFR-2.1）
- [ ] 编写 `test_redis_config_debug_no_password`：验证无密码 URL 的 `Debug` 正常输出（NFR-2.1 边界）
- [ ] 编写 `test_key_construction_ver`：验证 key = `{prefix}:{user_id}` 拼接正确（AC-1.4 / FR-1.4）
- [ ] 编写 `test_key_construction_bl`：验证 key = `{prefix}:{jti}` 拼接正确（AC-1.9 / FR-2.5）
- [ ] 编写 `test_key_construction_custom_prefix`：验证自定义前缀生效（AC-1.12）
- [ ] 编写 `test_error_mapping_connection_failed`：验证连接失败 → `ServiceUnavailable`（FR-3.5 / AC-2.3，使用不可达 URL 如 `redis://127.0.0.1:1`）
- [ ] 编写 `test_error_mapping_command_timeout`：验证命令超时 → `ServiceUnavailable`（NFR-1.5 / AC-2.5，设置极短 `command_timeout` 如 1ms）
- [ ] 编写 `test_revoke_ttl_zero_skipped`：验证 `ttl=0` 直接返回 `Ok(())`，不调用 Redis（AC-1.7 / FR-2.4，通过 mock 验证未发起 `SETEX` 命令）

**输入**：T2-T5 完成
**输出**：`redis_store.rs` 新增约 100-150 行测试代码
**验收标准**：`cargo test --features redis-store --lib redis_store::unit_tests` 全绿；不依赖真实 Redis 实例（无 `REDIS_URL` 环境变量也能运行）；覆盖率 ≥ 90%（AC-3.1）

### 2.2 T7: 契约测试（泛型，对齐 Memory 与 Redis 行为）

- [ ] 在 `redis_store.rs` 测试模块中定义 `async fn test_store_contract<S: RefreshTokenStore>(store: S)` 泛型函数，执行：不存在返回 0（AC-1.1）、连续递增返回 1/2/3（AC-1.2）、不同用户隔离（AC-1.3）
- [ ] 定义 `async fn test_blacklist_contract<B: TokenBlacklist>(bl: B)` 泛型函数，执行：未撤销返回 false、revoke 后返回 true（AC-1.5）、幂等撤销（AC-1.8）
- [ ] 对 `MemoryRefreshTokenStore` / `MemoryTokenBlacklist` 执行契约测试（不依赖 Redis，直接运行）
- [ ] 对 `RedisRefreshTokenStore` / `RedisTokenBlacklist` 执行契约测试（`REDIS_URL` 环境变量门控，未设置时 `#[ignore]` 跳过，AC-3.2）

**输入**：T3、T4 完成
**输出**：`redis_store.rs` 新增约 50-80 行测试代码
**验收标准**：`cargo test --features redis-store test_store_contract` 对 Memory 实现全绿；Redis 实现测试在 `REDIS_URL` 设置时全绿；验证二者行为完全一致（AC-3.6 / NFR-3.5）

### 2.3 T8: 集成测试（真实 Redis，REDIS_URL 环境变量门控）

- [ ] 在 `redis_store.rs` 测试模块中添加 `#[cfg(test)] mod integration_tests`，所有测试用 `#[ignore]` 或环境变量门控（`std::env::var("REDIS_URL").is_ok()`），未设置时跳过（AC-3.2）
- [ ] 编写 `test_get_version_default`：创建 store，调用 `get_version(1)`，断言返回 `Ok(0)`（AC-1.1）
- [ ] 编写 `test_increment_version_atomic`：连续调用 `increment_version(1)` 三次，断言返回 1/2/3，`get_version(1)` 返回 3（AC-1.2）
- [ ] 编写 `test_different_users_isolated`：对 user_id=1 递增一次，user_id=2 递增两次，断言 `get_version(1) == 1` 且 `get_version(2) == 2`（AC-1.3）
- [ ] 编写 `test_revoke_and_is_revoked`：`revoke("jti-abc", 3600)` 后 `is_revoked("jti-abc")` 返回 true，未撤销的 `is_revoked("jti-xyz")` 返回 false（AC-1.5）
- [ ] 编写 `test_blacklist_ttl_expiry`：`revoke("jti-tmp", 1)` + `tokio::time::sleep(2s)` + `is_revoked("jti-tmp")` 返回 false（AC-1.6，Redis TTL 过期自动删除）
- [ ] 编写 `test_revoke_idempotent`：对同一 `jti` 连续 `revoke` 两次，均返回 `Ok(())`（AC-1.8）
- [ ] 编写 `test_key_format_ver`：调用 `increment_version(42)`，通过 `redis-cli EXISTS sso:ver:42` 验证 key 存在（AC-1.4）
- [ ] 编写 `test_key_format_bl`：调用 `revoke("jti-789", 3600)`，验证 key `sso:bl:jti-789` 存在（AC-1.9）
- [ ] 编写 `test_custom_key_prefix`：使用自定义前缀 `myapp:ver` / `myapp:bl`，验证 key 格式正确（AC-1.12）
- [ ] 每个集成测试在运行前清理 Redis 中相关 key（`DEL sso:ver:*` / `DEL sso:bl:*`），避免测试间状态污染

**输入**：T6 完成（单元测试已验证基础逻辑）
**输出**：`redis_store.rs` 新增约 150-200 行测试代码
**验收标准**：`REDIS_URL=redis://127.0.0.1:6379 cargo test --features redis-store -- --ignored integration_tests` 全绿（需本地 Redis 实例）；未设置 `REDIS_URL` 时测试自动跳过

### 2.4 T9: 边界测试（断连 / 超时 / 并发）

- [ ] 编写 `test_concurrent_incr_atomicity`：100 个 tokio 任务并发对同一 `user_id` 调用 `increment_version`，最终 `get_version` 返回值 = 初始值 + 100（AC-2.2 / NFR-1.3，验证 `INCR` 原子性无丢失更新）
- [ ] 编写 `test_concurrent_revoke_same_jti`：100 个 tokio 任务并发对同一 `jti` 调用 `revoke`，均返回 `Ok(())`，不报错（AC-3.5 (h)）
- [ ] 编写 `test_fail_closed_redis_down`：启动测试后停止 Redis 服务，调用 `get_version` / `increment_version` / `is_revoked` / `revoke`，均返回 `Err(ServiceUnavailable)`，不 panic，不返回 Redis 原始错误（AC-2.3 / NFR-4.1）
- [ ] 编写 `test_auto_reconnect`：停止 Redis 后重新启动，`ConnectionManager` 自动重连，后续命令恢复正常返回，无需重建 store / blacklist 实例（AC-2.4 / NFR-4.2）
- [ ] 编写 `test_command_timeout`：设置极短 `command_timeout`（如 1ms）模拟慢查询 / 网络分区，操作返回 `Err(ServiceUnavailable)`，不无限阻塞（AC-2.5 / NFR-1.5）
- [ ] 编写 `test_user_id_zero`：`get_version(0)` / `increment_version(0)` 正常工作（AC-3.5 (a)）
- [ ] 编写 `test_user_id_negative`：`get_version(i64::MIN)` / `increment_version(i64::MAX)` 正常工作（AC-3.5 (b)）
- [ ] 编写 `test_empty_jti`：`revoke("", 3600)` / `is_revoked("")` 不 panic（AC-3.5 (c)，上层已过滤空 jti，但实现须健壮）
- [ ] 编写 `test_long_jti`：UUID v4 长度（36 字符）jti 正常工作（AC-3.5 (d)）
- [ ] 编写 `test_ttl_u64_max`：`revoke("jti", u64::MAX)` 处理 Redis TTL 上限（Redis `SETEX` TTL 为 i64，超限须返回 `Err` 或饱和处理，AC-3.5 (e)）
- [ ] 编写 `test_key_prefix_special_chars`：`key_prefix` 含 `:` / `/` 等特殊字符，key 拼接正确，不破坏 Redis key 解析（AC-3.5 (i)）

**输入**：T8 完成（集成测试基础设施可用）
**输出**：`redis_store.rs` 新增约 150-200 行测试代码
**验收标准**：`REDIS_URL=redis://127.0.0.1:6379 cargo test --features redis-store -- --ignored boundary_tests` 全绿；并发测试无丢失更新；fail-closed 测试不 panic

### 2.5 T10: 基准测试（criterion bench）

- [ ] 创建 `packages/sz-rust-auth-facade/benches/redis_store_bench.rs` 文件
- [ ] 编写 `bench_get_version`：基准测试 `RedisRefreshTokenStore::get_version`，验证 p99 < 5ms（NFR-1.1 / AC-2.1）
- [ ] 编写 `bench_increment_version`：基准测试 `RedisRefreshTokenStore::increment_version`，验证 p99 < 5ms（NFR-1.1）
- [ ] 编写 `bench_is_revoked`：基准测试 `RedisTokenBlacklist::is_revoked`，验证 p99 < 5ms（NFR-1.2）
- [ ] 编写 `bench_revoke`：基准测试 `RedisTokenBlacklist::revoke`，验证 p99 < 5ms（NFR-1.2）
- [ ] 在 `Cargo.toml` 新增 `[[bench]] name = "redis_store_bench" harness = false`，门控 `required-features = ["redis-store"]`
- [ ] 基准测试通过 `REDIS_URL` 环境变量门控，未设置时跳过（避免 CI 无 Redis 实例失败）

**输入**：T3、T4 完成
**输出**：`benches/redis_store_bench.rs` 新文件，约 80-100 行；`Cargo.toml` 新增 `[[bench]]` 段
**验收标准**：`REDIS_URL=redis://127.0.0.1:6379 cargo bench --bench redis_store_bench --features redis-store` 运行成功，p99 < 5ms（局域网环境）

---

## 3. 验证任务（M3: 集成验证）

### 3.1 T11: 代码质量验收

- [ ] 运行 `cargo clippy --all-features -- -D warnings`（workspace 范围），确认零警告（AC-3.3）
- [ ] 运行 `cargo doc --all-features --no-deps`（auth-facade），确认零警告，所有公开 API 有 rustdoc 注释（AC-3.4）
- [ ] 运行 `cargo tarpaulin --features redis-store --packages sz-rust-auth-facade`，确认 `redis_store.rs` 行覆盖率 ≥ 90%（AC-3.1）
- [ ] 运行 `cargo build --features redis-store`，确认零 `unsafe_code` 警告（workspace `forbid` 生效，AC-2.6 / C-4）
- [ ] 扫描新增代码确认无 `std::fs` 使用（统一 `tokio::fs`，C-2，本任务无文件 IO 应天然满足）
- [ ] 扫描新增代码确认无 `unsafe` 块（C-4 / NFR-2.4）

**输入**：T6-T10 完成（所有测试通过）
**输出**：验收报告（命令输出 + 覆盖率报告）
**验收标准**：clippy 零警告；rustdoc 零警告；覆盖率 ≥ 90%；无 unsafe；无 std::fs

### 3.2 T12: semver 兼容性验证

- [ ] 运行 `cargo semver-checks check-release --package sz-rust-auth-facade`，确认无 breaking change（AC-2.7 / C-5）
- [ ] 验证仅新增 API（`RedisRefreshTokenStore` / `RedisTokenBlacklist` / `RedisConfig` / `create_redis_stores`）+ 可选 feature（`redis-store` / `redis-cluster`），不修改现有公开 API（NFR-3.1）
- [ ] 验证 `RefreshTokenStore` / `TokenBlacklist` trait 签名不变（NFR-3.2）
- [ ] 验证 `MemoryRefreshTokenStore` / `MemoryTokenBlacklist` 保持不变（NFR-3.3）

**输入**：T11 完成
**输出**：`cargo-semver-checks` 报告
**验收标准**：`cargo-semver-checks` 通过；无 breaking change 标记

### 3.3 T13: sz-pay 兼容性验证

- [ ] 在 `E:\vue\test\sz-pay` 项目中，将 `sz-rust` 依赖版本升级至 `0.6.3`（或本地 path 依赖指向更新后的 workspace）
- [ ] 运行 `cargo build`（不启用 `redis-store` feature），确认编译成功，无编译错误（AC-2.8 / NFR-3.4）
- [ ] 运行 `cargo test`（sz-pay 现有测试），确认行为不变
- [ ] 验证 sz-pay 未启用 `redis-store` feature 时零 Redis 依赖引入

**输入**：T12 完成（semver 验证通过）
**输出**：sz-pay 编译 / 测试报告
**验收标准**：sz-pay `cargo build` 成功；`cargo test` 全绿；零 Redis 依赖

### 3.4 T14: feature gate 隔离验证

- [ ] 运行 `cargo build --no-default-features --packages sz-rust-auth-facade`，确认与 v0.6.2 构建产物完全一致（`redis_store.rs` 被 `#[cfg(feature = "redis-store")]` 排除，AC-2.10 / NFR-3.6）
- [ ] 运行 `cargo tree --no-default-features --packages sz-rust-auth-facade`，确认无 `redis` 节点（AC-1.13）
- [ ] 运行 `cargo build --features redis-store --packages sz-rust-auth-facade`，确认 `redis_store` 模块编译，`RedisRefreshTokenStore` 等可用
- [ ] 运行 `cargo build --features redis-gateway --packages sz-rust-auth-facade`，确认 `redis_gateway` 模块编译，`redis_store` 模块不编译（feature 正交）
- [ ] 运行 `cargo build --features redis-store,redis-gateway --packages sz-rust-auth-facade`，确认两个模块均编译，`redis` crate 仅编译一次（feature unification，FR-5.5）
- [ ] 运行 `cargo build --features redis-cluster --packages sz-rust-auth-facade`，确认集群模式编译（FR-6）

**输入**：T11 完成
**输出**：各 feature 组合的编译报告
**验收标准**：默认构建零 Redis 依赖；`redis-store` / `redis-gateway` 正交；`redis-cluster` 隐含 `redis-store`

---

## 4. 文档与发布任务（M4: 发布）

### 4.1 T15: 文档更新

- [ ] 在 `packages/sz-rust-auth-facade/src/redis_store.rs` 顶部添加模块级 rustdoc 注释，包含：模块职责说明、feature gate 用法、关键设计点（`INCR` 原子性 / `SETEX` TTL / fail-closed）、使用示例代码
- [ ] 为 `RedisConfig` / `RedisRefreshTokenStore` / `RedisTokenBlacklist` / `create_redis_stores` 添加完整 rustdoc 注释（参数 / 返回值 / 错误 / 示例）
- [ ] 在项目根 `CHANGELOG.md`（若存在）或新建 `packages/sz-rust-auth-facade/CHANGELOG.md` 添加 v0.6.3 变更记录：新增 `redis-store` feature、新增 `RedisRefreshTokenStore` / `RedisTokenBlacklist` / `RedisConfig` / `create_redis_stores` API、新增 `redis-cluster` feature
- [ ] 更新 `packages/sz-rust-auth-facade/Cargo.toml` 的 `description` 字段（若需要补充 Redis 存储后端能力说明）
- [ ] 在 `docs/spec/redis-store/` 目录下更新 spec.md / design.md 的「实现状态」标记（若文档含状态字段）

**输入**：T11-T14 完成（所有验证通过）
**输出**：rustdoc 注释完善；CHANGELOG 更新；文档状态标记
**验收标准**：`cargo doc --all-features --no-deps` 零警告且文档完整；CHANGELOG 含 v0.6.3 条目

### 4.2 T16: 版本 bump

- [ ] 在 workspace 根 `Cargo.toml` 将 `version = "0.6.2"` 改为 `version = "0.6.3"`（workspace 版本统一管理，所有 crate 同步升级）
- [ ] 运行 `cargo build --all-features` 确认版本 bump 后编译通过
- [ ] 运行 `cargo test --all-features` 确认版本 bump 后测试全绿
- [ ] 验证 `packages/sz-rust-auth-facade/Cargo.toml` 的 `version.workspace = true` 继承正确

**输入**：T15 完成
**输出**：workspace 版本 0.6.3
**验收标准**：`cargo build --all-features` 成功；`cargo test --all-features` 全绿

### 4.3 T17: 发布到 crates.io

- [ ] 运行 `cargo publish --dry-run --features redis-store -p sz-rust-auth-facade`，确认 dry-run 通过（无缺失文件、无未提交变更）
- [ ] 检查 `packages/sz-rust-auth-facade/Cargo.toml` 的 `license` / `repository` / `homepage` / `description` / `keywords` / `categories` 字段完整（crates.io 元数据要求）
- [ ] 读取 `E:\vue\test\鲜视达\服务器信息.md` 获取 crates.io 发布凭证（API token）
- [ ] 运行 `cargo login <token>` 配置 crates.io 凭证
- [ ] 运行 `cargo publish --features redis-store -p sz-rust-auth-facade` 发布到 crates.io
- [ ] 验证 crates.io 上 `sz-rust-auth-facade` 0.6.3 版本可访问，`cargo add sz-rust-auth-facade@0.6.3 --features redis-store` 可拉取
- [ ] 通知 sz-pay 项目可升级至 0.6.3（sz-pay 路径 `E:\vue\test\sz-pay`，不强制升级，由 sz-pay 维护者决定时机）

**输入**：T16 完成（版本 bump）
**输出**：crates.io 发布成功
**验收标准**：`cargo publish --dry-run` 通过；crates.io 0.6.3 版本可访问；`cargo add` 可拉取

---

## 5. 风险清单

### 5.1 技术风险

| 风险 ID | 风险描述 | 等级 | 影响 | 缓解措施 | 验证方式 | 关联任务 |
|---------|----------|------|------|----------|----------|----------|
| R-T1 | Redis 连接建立失败（URL 错误 / 认证失败 / 网络不可达） | 中 | `RedisConfig::connect()` 返回 `Err`，store / blacklist 无法创建 | 统一映射为 `ServiceUnavailable`（FR-3.5）；运维确保 Redis 可达 | T6 `test_error_mapping_connection_failed` | T2 |
| R-T2 | Redis 运行中断连（网络分区 / Redis 宕机） | 高 | 所有 Token 校验失败，SSO 不可用 | fail-closed 返回 `ServiceUnavailable` → 503（NFR-4.1）；`ConnectionManager` 自动重连（NFR-4.2）；运维部署 Sentinel / Cluster 高可用 | T9 `test_fail_closed_redis_down` / `test_auto_reconnect` | T9 |
| R-T3 | Redis 命令超时（慢查询 / 网络延迟） | 中 | Token 校验延迟，用户体验下降 | `command_timeout` 超时取消返回 `ServiceUnavailable`（NFR-1.5）；默认 2s，可配置调大 | T9 `test_command_timeout` / T10 基准测试 | T9 / T10 |
| R-T4 | `INCR` 并发丢失更新 | 高 | 版本号不连续，撤销所有 Token 失效 | 使用 Redis `INCR` 原子命令（C-14），禁止 `GET+1+SET` 非原子序列 | T9 `test_concurrent_incr_atomicity`（100 并发验证） | T3 / T9 |
| R-T5 | `INCR` 版本号溢出 `u64::MAX` | 极低 | 版本号溢出 panic | 显式处理溢出返回 `ServiceUnavailable`（NFR-4.5）；实际不可能（`u64::MAX` ≈ 1.8e19） | T3 溢出处理代码 | T3 |
| R-T6 | 黑名单 TTL 与 Token 实际过期不一致 | 中 | 黑名单提前过期（放行已撤销 Token）或无限增长 | TTL 由调用方传入 `exp - now`（FR-2.7 / C-15），Redis 实现不自行计算 | 代码审查 + T8 集成测试 | T4 / T8 |
| R-T7 | Redis 连接池耗尽 | 中 | 新操作阻塞，无法获取连接 | `ConnectionManager` 内置连接池管理，`Clone` 仅 `Arc` 引用计数（NFR-1.4）；不每次新建连接 | 代码审查 + 基准测试 | T3 / T4 / T10 |
| R-T8 | Redis Cluster `INCR` 跨分片原子性 | 中 | 集群模式下 `INCR` 原子性破坏 | Redis Cluster 对同一 key 单命令自动路由到对应分片，原子性不变（FR-6.1） | 集群模式集成测试（可选） | T0 / T14 |

### 5.2 集成风险

| 风险 ID | 风险描述 | 等级 | 影响 | 缓解措施 | 验证方式 | 关联任务 |
|---------|----------|------|------|----------|----------|----------|
| R-I1 | feature unification 意外启用 `redis` 依赖 | 低 | 默认构建引入 Redis 依赖，违背零依赖原则 | `#[cfg(feature = "redis-store")]` 编译期隔离（FR-5.3）；`cargo tree --no-default-features` 验证 | T14 feature 隔离验证 | T14 |
| R-I2 | semver 破坏（意外修改现有 API） | 中 | 下游项目（sz-pay）编译失败 | 仅新增 API + 可选 feature，不修改 trait 签名 / Memory 实现（NFR-3.1 / NFR-3.2 / NFR-3.3）；`cargo-semver-checks` 验证 | T12 semver 验证 + T13 sz-pay 兼容验证 | T12 / T13 |
| R-I3 | `redis-store` 与 `redis-gateway` feature 冲突 | 低 | 同时启用时编译失败或行为异常 | 二者共享 `redis` optional 依赖但连接管理方式不同（`redis_gateway` 同步 / `redis_store` 异步），互不影响（FR-5.5）；Cargo feature unification 自动处理 | T14 feature 组合编译验证 | T14 |
| R-I4 | workspace 版本 bump 影响其他 crate | 中 | workspace 内其他 crate 被迫升级 | workspace 版本统一管理，所有 crate 同步升级；`cargo build --all-features` 验证全局编译 | T16 版本 bump 后全量编译 | T16 |
| R-I5 | sz-pay 升级后行为变化 | 低 | sz-pay 业务逻辑受影响 | sz-pay 未启用 `redis-store` feature 时零影响（NFR-3.4）；`cargo build` + `cargo test` 验证 | T13 sz-pay 兼容验证 | T13 |

### 5.3 测试风险

| 风险 ID | 风险描述 | 等级 | 影响 | 缓解措施 | 验证方式 | 关联任务 |
|---------|----------|------|------|----------|----------|----------|
| R-Q1 | 集成测试依赖真实 Redis 实例，CI 环境无 Redis | 中 | 集成测试在 CI 跳过，覆盖率不足 | `REDIS_URL` 环境变量门控，未设置时 `#[ignore]` 跳过（AC-3.2）；单元测试用 mock 不依赖真实 Redis；CI 可选配置 Redis 服务 | T8 / T9 环境变量门控 | T8 / T9 |
| R-Q2 | mock Redis 行为与真实 Redis 不一致 | 中 | 单元测试通过但集成测试失败 | 单元测试仅覆盖配置 / key 构造 / 错误映射等纯逻辑；Redis 交互行为由集成测试覆盖；契约测试对齐 Memory 与 Redis 行为 | T6 单元测试 + T7 契约测试 + T8 集成测试分层 | T6 / T7 / T8 |
| R-Q3 | 并发测试时序不确定导致 flaky | 中 | 并发测试偶尔失败 | 使用 `INCR` 原子性保证确定性结果（最终版本号 = 初始 + N）；避免依赖时序的断言 | T9 `test_concurrent_incr_atomicity` 最终值断言 | T9 |
| R-Q4 | TTL 过期测试因时间精度 flaky | 低 | `test_blacklist_ttl_expiry` 偶尔失败 | TTL 设置 1s，sleep 2s，留足余量；避免 TTL = sleep 的临界值 | T8 `test_blacklist_ttl_expiry` | T8 |
| R-Q5 | 基准测试结果受环境噪声影响 | 低 | p99 < 5ms 阈值偶尔超标 | 基准测试在局域网环境运行；阈值留余量（5ms 对局域网 Redis 足 10x 余量）；记录历史数据对比 | T10 基准测试 | T10 |
| R-Q6 | 自动重连测试难以稳定复现 | 中 | `test_auto_reconnect` flaky | 使用 `redis::aio::ConnectionManager` 内置重连机制（NFR-4.2）；测试中显式停止 / 启动 Redis 服务，等待重连成功后断言；设置重连超时 | T9 `test_auto_reconnect` | T9 |

### 5.4 安全风险

| 风险 ID | 风险描述 | 等级 | 影响 | 缓解措施 | 验证方式 | 关联任务 |
|---------|----------|------|------|----------|----------|----------|
| R-S1 | Redis 密码在日志 / Debug 输出泄漏 | 高 | 攻击者获取 Redis 密码，访问 / 篡改 Token 状态 | `Debug` 脱敏 URL 密码为 `[REDACTED]`（NFR-2.1）；`tracing` 不记录 URL（NFR-2.2） | T6 `test_redis_config_debug_redacts_password` | T2 / T6 |
| R-S2 | fail-open 放行已撤销 Token | 高 | 已撤销 Token 继续有效，安全漏洞 | fail-closed 策略：Redis 故障返回 `ServiceUnavailable` → 503 拒绝（NFR-4.1） | T9 `test_fail_closed_redis_down` | T9 |
| R-S3 | Redis 数据被篡改（版本号被恶意重置） | 中 | 已撤销 Token 重新有效 | Redis 运维安全（AUTH / 网络隔离 / TLS）；应用层无法防御，依赖运维 | 运维层面，非本任务范围 | — |

---

## 6. 验收检查清单（对齐 spec.md §6）

### 6.1 功能验收（AC-1.x）

- [ ] AC-1.1 `get_version` 默认值返回 0（T8 `test_get_version_default`）
- [ ] AC-1.2 `increment_version` 原子递增 1/2/3（T8 `test_increment_version_atomic`）
- [ ] AC-1.3 不同用户版本号隔离（T8 `test_different_users_isolated`）
- [ ] AC-1.4 key 格式 `sso:ver:{user_id}`（T8 `test_key_format_ver`）
- [ ] AC-1.5 `revoke` + `is_revoked` 端到端（T8 `test_revoke_and_is_revoked`）
- [ ] AC-1.6 TTL 过期自动删除（T8 `test_blacklist_ttl_expiry`）
- [ ] AC-1.7 `ttl=0` 跳过写入（T6 `test_revoke_ttl_zero_skipped` + T8）
- [ ] AC-1.8 幂等撤销（T8 `test_revoke_idempotent`）
- [ ] AC-1.9 key 格式 `sso:bl:{jti}`（T8 `test_key_format_bl`）
- [ ] AC-1.10 `RedisConfig::default()` 值正确（T6 `test_redis_config_default`）
- [ ] AC-1.11 `Debug` 脱敏密码（T6 `test_redis_config_debug_redacts_password`）
- [ ] AC-1.12 自定义 key 前缀（T6 `test_key_construction_custom_prefix` + T8）
- [ ] AC-1.13 feature gate 隔离（T14）
- [ ] AC-1.14 与上层零侵入集成（T7 契约测试 + T13 sz-pay 兼容）

### 6.2 非功能验收（AC-2.x）

- [ ] AC-2.1 Redis 操作 p99 < 5ms（T10 基准测试）
- [ ] AC-2.2 `INCR` 原子性 100 并发（T9 `test_concurrent_incr_atomicity`）
- [ ] AC-2.3 Redis 故障 fail-closed（T9 `test_fail_closed_redis_down`）
- [ ] AC-2.4 `ConnectionManager` 自动重连（T9 `test_auto_reconnect`）
- [ ] AC-2.5 命令超时返回 `ServiceUnavailable`（T9 `test_command_timeout`）
- [ ] AC-2.6 无 unsafe（T11 代码质量验收）
- [ ] AC-2.7 semver 兼容（T12 `cargo-semver-checks`）
- [ ] AC-2.8 sz-pay 兼容（T13）
- [ ] AC-2.9 日志脱敏（T11 代码审查 + T6）
- [ ] AC-2.10 默认构建零影响（T14 `cargo build --no-default-features`）

### 6.3 代码质量验收（AC-3.x）

- [ ] AC-3.1 单元测试覆盖率 ≥ 90%（T11 `cargo tarpaulin`）
- [ ] AC-3.2 集成测试 `REDIS_URL` 门控（T8 / T9）
- [ ] AC-3.3 Clippy 零警告（T11）
- [ ] AC-3.4 rustdoc 零警告（T11 / T15）
- [ ] AC-3.5 边界测试全覆盖（T9）
- [ ] AC-3.6 契约测试对齐 Memory 与 Redis（T7）

---

## 7. 任务依赖关系详表

| 任务 | 前置依赖 | 后续依赖 | 可并行任务 | 关键路径 |
|------|----------|----------|------------|----------|
| T0 | — | T1, T2 | — | ✅ |
| T1 | T0 | T2 | — | ✅ |
| T2 | T0, T1 | T3, T4 | — | ✅ |
| T3 | T2 | T5, T7, T10 | T4 | ✅ |
| T4 | T2 | T5, T7, T10 | T3 | — |
| T5 | T3, T4 | T6 | — | ✅ |
| T6 | T3, T4, T5 | T8, T11 | T7, T10 | ✅ |
| T7 | T3, T4 | T11 | T6, T8, T10 | — |
| T8 | T6 | T9, T11 | T7, T10 | ✅ |
| T9 | T8 | T11 | T10 | ✅ |
| T10 | T3, T4 | T11 | T6, T7, T8, T9 | — |
| T11 | T6, T7, T8, T9, T10 | T12, T14, T15 | — | ✅ |
| T12 | T11 | T13, T15 | T14 | ✅ |
| T13 | T12 | T15 | T14 | — |
| T14 | T11 | T15 | T12, T13 | — |
| T15 | T11, T12, T13, T14 | T16 | — | ✅ |
| T16 | T15 | T17 | — | ✅ |
| T17 | T16 | — | — | ✅ |

**关键路径**：T0 → T1 → T2 → T3 → T5 → T6 → T8 → T9 → T11 → T12 → T15 → T16 → T17（约 16.3h）

**可并行优化**：
- T3 与 T4 并行（均依赖 T2，互不依赖）
- T7 / T10 与 T6 并行（均依赖 T3/T4，不依赖 T5）
- T8 / T10 与 T7 并行（T8 依赖 T6，T10 依赖 T3/T4，T7 依赖 T3/T4）
- T14 与 T12 / T13 并行（均依赖 T11）

**优化后关键路径预估**：约 14h（并行节省 2-3h）

---

## 8. 变更记录

| 日期 | 版本 | 变更 | 作者 |
|------|------|------|------|
| 2026-08-08 | tasks-v1.0 | 初稿，基于 redis-store spec-v1.0 + design-v1.0 生成，覆盖 T0-T17 共 18 个任务 | spec-task-agent |