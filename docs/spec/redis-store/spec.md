# spec.md — Redis 存储后端（RedisRefreshTokenStore + RedisTokenBlacklist）

> **项目**：sz-rust（对标 ThinkPHP 8 的 Rust Web 框架，axum 0.8 + SZ-ORM）
> **版本**：v0.6.2 → v0.6.3（semver 兼容，仅新增 API + 可选 feature）
> **规格版本**：spec-v1.0
> **创建日期**：2026-08-08
> **基于规格**：[sso-refresh-token/spec.md](../sso-refresh-token/spec.md)（spec-v1.0）
> **需求来源**：SSO Refresh Token 双 Token 机制 v0.6.2 已发布，当前存储后端仅有 Memory 实现，生产环境需要 Redis 持久化
> **目标 crate**：`sz-rust-auth-facade`（新增 `redis_store.rs`，feature gate `redis-store`）
> **不修改**：上游 `sz-orm` 仓库、`sz-orm-auth` crate、现有 `RefreshTokenStore` / `TokenBlacklist` trait 签名、现有 `MemoryRefreshTokenStore` / `MemoryTokenBlacklist` 实现

---

## 0. 现状基线（基于代码证据，非猜测）

| 能力 | 现状 | 证据 |
|------|------|------|
| `RefreshTokenStore` trait | ✅ 已定义 | `sz-rust-auth-facade/src/refresh.rs:322` — `get_version` + `increment_version` |
| `TokenBlacklist` trait | ✅ 已定义 | `sz-rust-auth-facade/src/refresh.rs:369` — `revoke` + `is_revoked` |
| `MemoryRefreshTokenStore` | ✅ 已实现 | `sz-rust-auth-facade/src/refresh.rs:331` — 基于 `HashMap` + `RwLock`，单进程 |
| `MemoryTokenBlacklist` | ✅ 已实现 | `sz-rust-auth-facade/src/refresh.rs:378` — 基于 `HashMap` + `RwLock`，单进程 |
| `RefreshTokenError::ServiceUnavailable` | ✅ 已存在 | `sz-rust-auth-facade/src/refresh.rs:67` — `#[error("service unavailable")]` |
| workspace `redis` 依赖 | ✅ 已配置 | `Cargo.toml:144` — `redis = { version = "0.27", features = ["aio", "tokio-comp", "connection-manager"] }` |
| auth-facade `redis` optional 依赖 | ✅ 已存在 | `sz-rust-auth-facade/Cargo.toml:22` — `redis = { workspace = true, optional = true }`（当前用于 `redis-gateway` feature） |
| `redis-gateway` feature | ✅ 已存在 | `sz-rust-auth-facade/Cargo.toml:47` — `redis-gateway = ["redis", "futures"]` |
| `RefreshTokenIssuer` / `Verifier` / `Revoker` | ✅ 已实现 | `sz-rust-auth-facade/src/refresh.rs:499/420/622` — 通过 `Arc<dyn RefreshTokenStore>` + `Arc<dyn TokenBlacklist>` 注入，存储可替换 |
| **Redis `RefreshTokenStore` 实现** | ❌ **缺失** | 全项目无 `RedisRefreshTokenStore` |
| **Redis `TokenBlacklist` 实现** | ❌ **缺失** | 全项目无 `RedisTokenBlacklist` |
| **`redis-store` feature** | ❌ **缺失** | `Cargo.toml:45-51` 仅有 `redis-gateway` / `axum` / `remote-validate` |
| **`RedisConfig`** | ❌ **缺失** | 无 Redis 连接配置结构体 |

**结论**：本规格仅新增「`RedisRefreshTokenStore` + `RedisTokenBlacklist` + `RedisConfig` + `redis-store` feature」四块能力，不修改现有 trait 定义与 Memory 实现，不重复造已有轮子。现有 `RefreshTokenIssuer` / `Verifier` / `Revoker` 通过 `Arc<dyn Trait>` 注入存储，替换为 Redis 实现无需改动上层逻辑（零侵入）。

---

## 1. 范围

### 1.1 In-Scope（本次交付）

| 编号 | 能力 | 落点 |
|------|------|------|
| FR-1 | `RedisRefreshTokenStore`（实现 `RefreshTokenStore` trait） | `sz-rust-auth-facade/src/redis_store.rs`（新增） |
| FR-2 | `RedisTokenBlacklist`（实现 `TokenBlacklist` trait） | 同上 |
| FR-3 | Redis 连接管理（`ConnectionManager`，自动重连 + 连接池复用） | 同上 |
| FR-4 | `RedisConfig`（url / key 前缀 / 超时等配置） | 同上 |
| FR-5 | `redis-store` feature gate（默认不启用，保持零网络依赖） | `sz-rust-auth-facade/Cargo.toml`（新增 feature） |
| FR-6 | Redis 集群支持（可选，`redis-cluster` feature） | 同上 |

### 1.2 Out-of-Scope（明确不做，附理由）

| 项 | 理由 |
|----|------|
| 修改 `RefreshTokenStore` / `TokenBlacklist` trait 签名 | 破坏 semver，且 Memory 实现已稳定；Redis 实现只需满足现有 trait |
| 修改 `MemoryRefreshTokenStore` / `MemoryTokenBlacklist` | 测试用内存实现保持不变，生产用 Redis 实现，二者并存 |
| 修改上游 `sz-orm` / `sz-orm-auth` | 项目铁律：不修改上游仓库 |
| Redis Sentinel 哨兵高可用 | 属运维层面，连接 URL 可指向 Sentinel 代理，无需应用层支持；后续延伸 |
| Redis 持久化配置（RDB / AOF） | 运维职责，应用层仅消费 Redis 服务，不管理持久化策略 |
| 修改 `RefreshTokenIssuer` / `Verifier` / `Revoker` | 已通过 `Arc<dyn Trait>` 注入存储，替换实现无需改动上层（零侵入） |
| Redis Lua 脚本 | `INCR` + `SETEX` + `EXISTS` 均为 Redis 原子命令，无需 Lua |
| 修改 `sz-rust-middleware-facade` | 黑名单 trait 在 auth-facade 定义，中间件通过 trait object 消费，不感知具体实现 |

---

## 2. 术语与约定

| 术语 | 定义 |
|------|------|
| **RedisRefreshTokenStore** | 基于 Redis 的 `RefreshTokenStore` 实现，维护 `user_id → token_version` 映射，用于用户级 Token 撤销 |
| **RedisTokenBlacklist** | 基于 Redis 的 `TokenBlacklist` 实现，存储已撤销 Token 的 `jti`，支持 TTL 自动过期 |
| **ConnectionManager** | `redis` crate 提供的连接管理器，内置自动重连与连接池复用，线程安全（`Send + Sync + Clone`） |
| **token_version（版本号）** | 每个用户的 Token 版本号，签发时嵌入 JWT `ver` claim，撤销该用户所有 Token 时 `INCR` 递增使旧 Token 失效 |
| **jti（JWT ID）** | JWT 唯一标识（UUID v4），用于黑名单精确定位单个 Token |
| **key 命名空间** | Redis key 前缀约定：版本号 `sso:ver:{user_id}`，黑名单 `sso:bl:{jti}` |
| **INCR 原子递增** | Redis `INCR` 命令天然原子，无需额外锁，保证并发递增安全 |
| **SETEX 带 TTL 写入** | Redis `SETEX key ttl value`，写入同时设置过期时间，黑名单条目到期自动清理 |
| **EXISTS 存在性检查** | Redis `EXISTS key`，检查 key 是否存在，用于黑名单查询 |
| **fail-closed** | Redis 故障时返回 `ServiceUnavailable` 拒绝请求，禁止 fail-open 放行（对齐 NFR-4.1） |

**默认参数**（可通过 `RedisConfig` 覆盖）：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `url` | `redis://127.0.0.1:6379` | Redis 连接 URL（支持 `redis://:password@host:port/db`） |
| `key_prefix_ver` | `"sso:ver"` | 版本号 key 前缀，完整 key = `{prefix}:{user_id}` |
| `key_prefix_bl` | `"sso:bl"` | 黑名单 key 前缀，完整 key = `{prefix}:{jti}` |
| `connection_timeout` | 3 秒 | 建立连接超时 |
| `command_timeout` | 2 秒 | 单条命令执行超时 |
| `blacklist_value` | `"1"` | 黑名单 key 的占位值（仅用 EXISTS 判断存在性，值无语义） |

---

## 3. 功能需求（EARS 格式）

> EARS 语法：
> - Ubiquitous：`The {system} shall {response}.`
> - Event-driven：`When {trigger}, the {system} shall {response}.`
> - State-driven：`While {state}, the {system} shall {response}.`
> - Optional feature：`Where {feature is included}, the {system} shall {response}.`
> - Unwanted：`If {trigger}, then the {system} shall {response}.`

### 3.1 FR-1 RedisRefreshTokenStore

**FR-1.1**（Optional feature）
> Where feature `redis-store` is enabled, the `RedisRefreshTokenStore` shall 实现 `RefreshTokenStore` trait（`sz-rust-auth-facade/src/refresh.rs:322`），提供 `get_version` 与 `increment_version` 方法，可作为 `Arc<dyn RefreshTokenStore>` 注入 `RefreshTokenIssuer` / `Verifier` / `Revoker`.

**FR-1.2**（Event-driven）
> When `RedisRefreshTokenStore::get_version(user_id)` 被调用, the `RedisRefreshTokenStore` shall 执行 Redis `GET sso:ver:{user_id}`，若 key 存在返回解析后的 `u64` 版本号，若 key 不存在返回 `0`（与 `MemoryRefreshTokenStore` 行为一致，对齐 `refresh.rs:353`）.

**FR-1.3**（Event-driven）
> When `RedisRefreshTokenStore::increment_version(user_id)` 被调用, the `RedisRefreshTokenStore` shall 执行 Redis `INCR sso:ver:{user_id}`（原子递增），返回递增后的新版本号 `u64`.

**FR-1.4**（Ubiquitous）
> The `RedisRefreshTokenStore` shall 使用 key 格式 `sso:ver:{user_id}`（`key_prefix_ver` 可通过 `RedisConfig` 配置覆盖），禁止使用裸 `user_id` 作为 key（避免与其他业务 key 冲突）.

**FR-1.5**（Unwanted）
> If Redis `GET` / `INCR` 操作失败（连接断开、超时、协议错误）, then the `RedisRefreshTokenStore` shall 返回 `Err(RefreshTokenError::ServiceUnavailable)`，不返回 Redis 原始错误给上层（防止内部细节泄漏，对齐 `refresh.rs:67`）.

**FR-1.6**（Ubiquitous）
> The `RedisRefreshTokenStore::increment_version` shall 依赖 Redis `INCR` 命令的原子性保证并发安全，禁止使用 `GET` + 本地加 1 + `SET` 的非原子序列（防止并发递增丢失更新）.

**FR-1.7**（State-driven）
> While 用户首次调用 `increment_version(user_id)` 且 key `sso:ver:{user_id}` 不存在, the Redis `INCR` shall 将 key 初始化为 `1` 并返回 `1`（Redis `INCR` 对不存在的 key 视为 `0` 再递增，与 `MemoryRefreshTokenStore` 行为一致，对齐 `refresh.rs:358`）.

### 3.2 FR-2 RedisTokenBlacklist

**FR-2.1**（Optional feature）
> Where feature `redis-store` is enabled, the `RedisTokenBlacklist` shall 实现 `TokenBlacklist` trait（`sz-rust-auth-facade/src/refresh.rs:369`），提供 `revoke` 与 `is_revoked` 方法，可作为 `Arc<dyn TokenBlacklist>` 注入 `RefreshTokenIssuer` / `Verifier` / `Revoker`.

**FR-2.2**（Event-driven）
> When `RedisTokenBlacklist::is_revoked(jti)` 被调用, the `RedisTokenBlacklist` shall 执行 Redis `EXISTS sso:bl:{jti}`，key 存在返回 `true`，key 不存在返回 `false`.

**FR-2.3**（Event-driven）
> When `RedisTokenBlacklist::revoke(jti, ttl_secs)` 被调用且 `ttl_secs > 0`, the `RedisTokenBlacklist` shall 执行 Redis `SETEX sso:bl:{jti} {ttl_secs} 1`，将 jti 加入黑名单并设置 TTL，到期后 Redis 自动删除该 key（无需应用层清理）.

**FR-2.4**（State-driven）
> While `ttl_secs == 0`, the `RedisTokenBlacklist::revoke` shall 直接返回 `Ok(())`（幂等，不写入 Redis），因为 TTL 为 0 意味着 Token 已过期，天然失效无需占用存储（对齐 `MemoryTokenBlacklist` 行为，`refresh.rs:399-403`）.

**FR-2.5**（Ubiquitous）
> The `RedisTokenBlacklist` shall 使用 key 格式 `sso:bl:{jti}`（`key_prefix_bl` 可通过 `RedisConfig` 配置覆盖），禁止使用裸 `jti` 作为 key.

**FR-2.6**（Unwanted）
> If Redis `EXISTS` / `SETEX` 操作失败, then the `RedisTokenBlacklist` shall 返回 `Err(RefreshTokenError::ServiceUnavailable)`，不返回 Redis 原始错误.

**FR-2.7**（Ubiquitous）
> The `RedisTokenBlacklist::revoke` 的 `ttl_secs` shall 由调用方（`RefreshTokenRevoker::revoke` / `RefreshTokenIssuer::rotate`）传入，值为 Token 的剩余有效期（`exp - now`），保证黑名单 TTL 与 Token 剩余 TTL 一致，避免黑名单无限增长（对齐 sso spec FR-4.1）.

**FR-2.8**（Ubiquitous）
> The `RedisTokenBlacklist::revoke` shall 幂等：对同一 `jti` 多次撤销返回相同 `Ok(())`，不报错（Redis `SETEX` 覆盖写入天然幂等，对齐 sso spec NFR-4.2）.

### 3.3 FR-3 Redis 连接管理

**FR-3.1**（Ubiquitous）
> The `RedisRefreshTokenStore` / `RedisTokenBlacklist` shall 使用 `redis::aio::ConnectionManager` 管理连接，复用其内置的自动重连与连接池能力，禁止每次操作新建连接.

**FR-3.2**（Ubiquitous）
> The `ConnectionManager` shall 满足 `Send + Sync + Clone`（`redis` crate 保证），可在 tokio 多线程运行时跨任务共享，对齐项目铁律 C-1（所有 `async fn` 必须 `Send + 'static`）.

**FR-3.3**（Event-driven）
> When `RedisConfig::connect()` 被调用, the `RedisConfig` shall 解析 `url` 并创建 `ConnectionManager`，连接失败时返回 `Err(RefreshTokenError::ServiceUnavailable)`.

**FR-3.4**（State-driven）
> While Redis 连接断开, the `ConnectionManager` shall 自动重连（由 `redis` crate `connection-manager` feature 提供），应用层无需手动重连，后续命令在重连成功后自动恢复.

**FR-3.5**（Unwanted）
> If 创建 `ConnectionManager` 失败（URL 格式错误、无法连接、认证失败）, then `RedisConfig::connect()` shall 返回 `Err(RefreshTokenError::ServiceUnavailable)`，不 panic.

**FR-3.6**（Ubiquitous）
> The `RedisRefreshTokenStore` / `RedisTokenBlacklist` shall 内部持有 `ConnectionManager`（`Clone` 即可，`ConnectionManager` 内部 `Arc` 引用计数共享同一连接池），禁止持有裸 `Connection`（非线程安全）.

### 3.4 FR-4 RedisConfig

**FR-4.1**（Ubiquitous）
> The `RedisConfig` shall 包含以下字段：`url: String`、`key_prefix_ver: String`、`key_prefix_bl: String`、`connection_timeout: Duration`、`command_timeout: Duration`.

**FR-4.2**（Ubiquitous）
> The `RedisConfig::default()` shall 返回：`url = "redis://127.0.0.1:6379"`、`key_prefix_ver = "sso:ver"`、`key_prefix_bl = "sso:bl"`、`connection_timeout = 3s`、`command_timeout = 2s`.

**FR-4.3**（Ubiquitous）
> The `RedisConfig` 的 `Debug` 实现 shall 将 `url` 中嵌入的密码脱敏为 `redis://:[REDACTED]@host:port/db`（对齐 `SsoJwtCodec` 的 secret 脱敏，`refresh.rs:270-276`，对齐项目铁律 C-3 敏感字段自动脱敏）.

**FR-4.4**（Event-driven）
> When `RedisConfig::connect()` 被调用, the `RedisConfig` shall 使用 `connection_timeout` 作为建立连接的超时，使用 `command_timeout` 作为单条命令执行的超时（通过 `redis::AsyncConnectOptions` 或 `tokio::time::timeout` 包装）.

**FR-4.5**（Ubiquitous）
> The `RedisConfig` shall 提供 `from_url(url: impl Into<String>) -> Self` 便捷构造方法，其余字段使用默认值.

### 3.5 FR-5 redis-store feature gate

**FR-5.1**（Ubiquitous）
> The `sz-rust-auth-facade/Cargo.toml` shall 新增 `redis-store` feature，默认不启用（`default = []` 保持不变），启用时引入 `redis` workspace 依赖.

**FR-5.2**（Optional feature）
> Where feature `redis-store` is enabled, the `sz-rust-auth-facade` shall 编译 `redis_store.rs` 模块并导出 `RedisRefreshTokenStore` / `RedisTokenBlacklist` / `RedisConfig` 公开 API.

**FR-5.3**（Optional feature）
> Where feature `redis-store` is NOT enabled, the `sz-rust-auth-facade` shall 编译期排除 `redis_store.rs` 模块与 `redis` 依赖（通过 `#[cfg(feature = "redis-store")]`），零 Redis 运行时开销.

**FR-5.4**（Ubiquitous）
> The `redis-store` feature shall 复用 auth-facade 已有的 `redis = { workspace = true, optional = true }` 依赖声明（`Cargo.toml:22`），不新增重复依赖声明，feature 定义为 `redis-store = ["dep:redis"]`.

**FR-5.5**（Ubiquitous）
> The `redis-store` feature 与现有 `redis-gateway` feature shall 独立正交，可单独启用或同时启用，互不影响（二者均依赖 `redis` optional 依赖，Cargo feature unification 自动处理）.

### 3.6 FR-6 Redis 集群支持（可选）

**FR-6.1**（Optional feature）
> Where feature `redis-cluster` is enabled, the `RedisConfig` shall 支持 Redis Cluster 连接（`redis://` URL 指向集群任一节点，使用 `redis::cluster::ClusterClient`），`ConnectionManager` 自动路由到对应分片.

**FR-6.2**（Optional feature）
> Where feature `redis-cluster` is NOT enabled, the `RedisConfig` shall 仅支持单节点 / Sentinel 代理连接，不引入 `ClusterClient` 依赖.

**FR-6.3**（Ubiquitous）
> The `redis-cluster` feature shall 隐含 `redis-store` feature（`redis-cluster = ["redis-store", "redis/cluster"]`），集群支持建立在存储后端之上.

---

## 4. 非功能需求（EARS 格式）

### 4.1 性能

**NFR-1.1**（Ubiquitous）
> The `RedisRefreshTokenStore::get_version` / `increment_version` shall 在局域网（RTT < 1ms）环境下 p99 响应时间 < 5ms（含网络 IO + Redis 命令执行），基准测试 `benches/redis_store_bench.rs` 须持续验证.

**NFR-1.2**（Ubiquitous）
> The `RedisTokenBlacklist::is_revoked` / `revoke` shall 在局域网环境下 p99 响应时间 < 5ms.

**NFR-1.3**（Ubiquitous）
> The `RedisRefreshTokenStore::increment_version` shall 依赖 Redis `INCR` 命令的原子性，在并发 100 个 `increment_version` 调用下，最终版本号 shall 等于初始值 + 100（无丢失更新），通过并发测试验证.

**NFR-1.4**（Ubiquitous）
> The `ConnectionManager` shall 在进程生命周期内复用连接池，禁止每次操作新建连接（`ConnectionManager` 内部 `Arc` 共享，`Clone` 仅增加引用计数）.

**NFR-1.5**（State-driven）
> While `command_timeout` 到达且 Redis 命令未返回, the 操作 shall 被取消并返回 `Err(RefreshTokenError::ServiceUnavailable)`，不无限阻塞调用方.

### 4.2 安全

**NFR-2.1**（Ubiquitous）
> The `RedisConfig` 的 `Debug` 实现 shall 将 `url` 中的密码脱敏为 `[REDACTED]`，禁止在日志 / 错误信息中泄漏 Redis 密码（对齐 `SsoJwtCodec` 脱敏，`refresh.rs:270`，对齐项目铁律 C-3）.

**NFR-2.2**（Ubiquitous）
> The `RedisRefreshTokenStore` / `RedisTokenBlacklist` 的 `tracing` 日志 shall 仅记录 `user_id` / `jti` / 操作类型 / 错误类型，禁止记录 Redis 连接 URL（含密码）、Token 明文（对齐 sso spec NFR-2.3）.

**NFR-2.3**（Ubiquitous）
> The 所有 `async fn` shall 满足 `Send + 'static`（项目铁律 C-1），可在 tokio 多线程运行时安全调度，`ConnectionManager` 满足 `Send + Sync`.

**NFR-2.4**（Ubiquitous）
> The `RedisRefreshTokenStore` / `RedisTokenBlacklist` shall 不引入新 `unsafe` 代码（对齐项目铁律 C-4，`workspace.lints.rust.unsafe_code = "forbid"`）.

**NFR-2.5**（Ubiquitous）
> The `RedisConfig` 的 `url` shall 通过环境变量或配置文件注入，禁止硬编码在生产代码中（测试代码除外）.

### 4.3 兼容性

**NFR-3.1**（Ubiquitous）
> The 本次新增的 API shall 不修改 `sz-rust-auth-facade` 现有公开 API（`refresh` / `sso` / `gateway` / `oauth` / `wechat` 模块签名不变），semver minor 版本升级（0.6.2 → 0.6.3）.

**NFR-3.2**（Ubiquitous）
> The `RefreshTokenStore` / `TokenBlacklist` trait 签名 shall 保持不变，`RedisRefreshTokenStore` / `RedisTokenBlacklist` 仅作为新增实现，不修改 trait 定义.

**NFR-3.3**（Ubiquitous）
> The 现有 `MemoryRefreshTokenStore` / `MemoryTokenBlacklist` shall 保持不变，与 Redis 实现并存，测试场景继续使用 Memory 实现（零 Redis 依赖）.

**NFR-3.4**（Ubiquitous）
> The 现有 `sz-pay` 项目（路径 `E:\vue\test\sz-pay`）shall 在升级 sz-rust 至 0.6.3 后无需修改业务代码即可编译通过（未启用 `redis-store` feature 时零影响）.

**NFR-3.5**（Ubiquitous）
> The `RedisRefreshTokenStore::get_version` 对不存在的 user_id shall 返回 `0`，与 `MemoryRefreshTokenStore` 行为完全一致（对齐 `refresh.rs:353`），保证上层 `RefreshTokenIssuer` / `Verifier` 逻辑无需区分存储后端.

**NFR-3.6**（Ubiquitous）
> The `cargo build`（不启用 `redis-store` feature）shall 零 `redis` 依赖编译，与 v0.6.2 构建产物完全一致（feature gate 隔离验证）.

### 4.4 可靠性

**NFR-4.1**（State-driven）
> While Redis 服务不可用（连接拒绝、超时、网络分区）, the `RedisRefreshTokenStore` / `RedisTokenBlacklist` shall 返回 `Err(RefreshTokenError::ServiceUnavailable)`，上层 `sso_middleware` fail-closed 拒绝所有请求（对齐 sso spec NFR-4.1），禁止 fail-open 放行.

**NFR-4.2**（State-driven）
> While Redis 连接临时断开, the `ConnectionManager` shall 自动重连（由 `redis` crate `connection-manager` feature 提供），重连成功后后续命令自动恢复，应用层无需手动干预.

**NFR-4.3**（Ubiquitous）
> The `RedisTokenBlacklist` 的黑名单条目 shall 通过 Redis `SETEX` 的 TTL 机制自动过期清理，无需应用层定时扫描清理（对齐 sso spec FR-4.1「避免黑名单无限增长」）.

**NFR-4.4**（Ubiquitous）
> The `RedisRefreshTokenStore::increment_version` shall 依赖 Redis `INCR` 原子性，天然幂等语义（每次调用版本号 +1），无需应用层加锁.

**NFR-4.5**（Unwanted）
> If Redis `INCR` 后版本号溢出 `u64::MAX`, then the `RedisRefreshTokenStore` shall 返回 `Err(RefreshTokenError::ServiceUnavailable)`（实际不可能发生，`u64::MAX` ≈ 1.8e19，但需显式处理防止 panic）.

---

## 5. 约束条件

### 5.1 项目铁律（来自 `AGENTS.md` 与 `.trae/rules/project_rules.md`）

| 编号 | 约束 | 验证方式 |
|------|------|----------|
| C-1 | 所有 `async fn` 必须 `Send + 'static` | 编译期 trait bound 检查 |
| C-2 | 禁止 `std::fs`，统一 `tokio::fs` | `clippy` 自定义 lint 或 grep 检查 |
| C-3 | 敏感字段 `#[serde(skip_serializing)]` / `Debug` 脱敏 | 代码审查 + 序列化测试 |
| C-4 | 不引入新 `unsafe` 代码 | `workspace.lints.rust.unsafe_code = "forbid"` |
| C-5 | 不破坏 sz-rust 公开 API（semver 兼容） | `cargo-semver-checks` |
| C-6 | 不修改上游 `sz-orm` 仓库 | PR diff 不含 `sz-orm/` 路径 |
| C-7 | `workspace.unsafe_code = "forbid"` | `Cargo.toml` lint 配置 |

### 5.2 本规格特有约束

| 编号 | 约束 |
|------|------|
| C-8 | 新增代码仅落在 `sz-rust-auth-facade/src/redis_store.rs` 与 `sz-rust-auth-facade/Cargo.toml`（新增 feature），不散落到其他 crate |
| C-9 | `redis` 依赖仅在 `redis-store` feature 下引入，默认构建零 Redis 依赖（对齐 sso spec C-9 精神） |
| C-10 | 不修改 `RefreshTokenStore` / `TokenBlacklist` trait 定义，Redis 实现仅作为新增 trait impl |
| C-11 | 所有新增公开 API 须有 rustdoc 注释 + 至少 1 个单元测试（对齐项目现有风格，sso spec C-11） |
| C-12 | `tracing` 日志 span 须使用 `#[tracing::instrument(skip(self))]` 跳过 `ConnectionManager` 等非结构化参数（对齐 sso spec C-12） |
| C-13 | Redis 操作失败统一映射为 `RefreshTokenError::ServiceUnavailable`，禁止泄漏 Redis 原始错误（`redis::RedisError`）给上层 |
| C-14 | `INCR` 操作必须使用 Redis 原子命令，禁止 `GET` + 本地加 1 + `SET` 非原子序列 |
| C-15 | 黑名单 TTL 必须由调用方传入（Token 剩余有效期），禁止 Redis 实现自行计算 TTL |

---

## 6. 验收标准（EARS 格式，可测试）

### 6.1 功能验收

**AC-1.1** RedisRefreshTokenStore get_version 默认值
> When 创建 `RedisRefreshTokenStore` 并调用 `get_version(1)`（Redis 中无 `sso:ver:1` key）, then 返回 shall 等于 `Ok(0)`，与 `MemoryRefreshTokenStore` 行为一致.

**AC-1.2** RedisRefreshTokenStore increment_version 原子递增
> When 连续调用 `increment_version(1)` 三次, then 返回 shall 依次为 `Ok(1)` / `Ok(2)` / `Ok(3)`，且 `get_version(1)` 返回 `Ok(3)`.

**AC-1.3** RedisRefreshTokenStore 不同用户隔离
> When 对 user_id=1 调用 `increment_version` 一次，对 user_id=2 调用两次, then `get_version(1) == Ok(1)` 且 `get_version(2) == Ok(2)`，用户间版本号独立.

**AC-1.4** RedisRefreshTokenStore key 格式
> When 调用 `increment_version(42)`, then Redis 中 shall 存在 key `sso:ver:42`（通过 `redis-cli EXISTS sso:ver:42` 验证返回 1）.

**AC-1.5** RedisTokenBlacklist revoke + is_revoked
> When 调用 `revoke("jti-abc", 3600)` 后调用 `is_revoked("jti-abc")`, then 返回 shall 等于 `Ok(true)`；对未撤销的 `is_revoked("jti-xyz")` 返回 `Ok(false)`.

**AC-1.6** RedisTokenBlacklist TTL 过期
> When 调用 `revoke("jti-tmp", 1)` 并等待 2 秒后调用 `is_revoked("jti-tmp")`, then 返回 shall 等于 `Ok(false)`（Redis TTL 过期自动删除）.

**AC-1.7** RedisTokenBlacklist TTL 为 0 跳过写入
> When 调用 `revoke("jti-zero", 0)`, then 返回 shall 等于 `Ok(())` 且 Redis 中不存在 key `sso:bl:jti-zero`（幂等跳过）.

**AC-1.8** RedisTokenBlacklist 幂等撤销
> When 对同一 `jti` 连续调用 `revoke("jti-idem", 3600)` 两次, then 两次均返回 `Ok(())`，不报错.

**AC-1.9** RedisTokenBlacklist key 格式
> When 调用 `revoke("jti-789", 3600)`, then Redis 中 shall 存在 key `sso:bl:jti-789`（通过 `redis-cli EXISTS sso:bl:jti-789` 验证）.

**AC-1.10** RedisConfig 默认值
> When 调用 `RedisConfig::default()`, then `url == "redis://127.0.0.1:6379"`、`key_prefix_ver == "sso:ver"`、`key_prefix_bl == "sso:bl"`、`connection_timeout == 3s`、`command_timeout == 2s`.

**AC-1.11** RedisConfig Debug 脱敏
> When 创建 `RedisConfig { url: "redis://:secret-pass@host:6379/0", .. }` 并执行 `format!("{:?}", config)`, then 输出 shall 不包含 `secret-pass`，密码部分显示为 `[REDACTED]`.

**AC-1.12** RedisConfig 自定义 key 前缀
> When 创建 `RedisConfig { key_prefix_ver: "myapp:ver", key_prefix_bl: "myapp:bl", .. }` 并调用 `increment_version(1)`, then Redis 中 shall 存在 key `myapp:ver:1`（非默认 `sso:ver:1`）.

**AC-1.13** feature gate 隔离
> Where `cargo build` 不启用 `redis-store` feature, then 编译产物 shall 不包含 `redis` crate 依赖（通过 `cargo tree --no-default-features` 验证无 `redis` 节点）.
> Where 启用 `redis-store` feature, then `RedisRefreshTokenStore` / `RedisTokenBlacklist` / `RedisConfig` shall 可正常编译与使用.

**AC-1.14** 与上层零侵入集成
> When 将 `RefreshTokenIssuer` 的 `store` 从 `MemoryRefreshTokenStore` 替换为 `RedisRefreshTokenStore`、`blacklist` 从 `MemoryTokenBlacklist` 替换为 `RedisTokenBlacklist`, then `issue` / `rotate` / `revoke` / `verify_access` / `verify_refresh` 行为 shall 与 Memory 实现完全一致（通过同一套上层测试用例验证）.

### 6.2 非功能验收

**AC-2.1** Redis 操作性能
> The `cargo bench --bench redis_store_bench` shall 报告 `get_version` / `increment_version` / `is_revoked` / `revoke` 在局域网环境下 p99 < 5ms.

**AC-2.2** INCR 原子性（并发测试）
> When 100 个 tokio 任务并发对同一 `user_id` 调用 `increment_version`, then 最终 `get_version` 返回值 shall 等于初始值 + 100（无丢失更新）.

**AC-2.3** Redis 故障 fail-closed
> When Redis 服务停止后调用 `get_version` / `increment_version` / `is_revoked` / `revoke`, then 均返回 `Err(RefreshTokenError::ServiceUnavailable)`，不 panic，不返回 Redis 原始错误.

**AC-2.4** ConnectionManager 自动重连
> When Redis 服务停止后重新启动, then `ConnectionManager` shall 自动重连，后续命令恢复正常返回，无需重建 `RedisRefreshTokenStore` / `RedisTokenBlacklist` 实例.

**AC-2.5** 命令超时
> When Redis 命令执行超过 `command_timeout`（模拟慢查询或网络分区）, then 操作 shall 返回 `Err(RefreshTokenError::ServiceUnavailable)`，不无限阻塞.

**AC-2.6** 无 unsafe
> The `cargo build --features redis-store` shall 零 `unsafe_code` 警告（workspace `forbid` 生效）.

**AC-2.7** semver 兼容
> The `cargo semver-checks check-release` shall 通过，无 breaking change（仅新增 API + 可选 feature）.

**AC-2.8** sz-pay 兼容
> The `sz-pay` 项目在 `Cargo.toml` 中将 `sz-rust` 升级至 `0.6.3` 后（不启用 `redis-store` feature），`cargo build` shall 成功，无编译错误.

**AC-2.9** 日志脱敏
> When 启用 `tracing` 日志并触发 Redis 操作, then 日志输出 shall 不包含 Redis URL（含密码）、Token 明文，仅包含 `user_id` / `jti` / 操作类型 / 错误类型.

**AC-2.10** 默认构建零影响
> The `cargo build --no-default-features` shall 与 v0.6.2 构建产物完全一致（`redis_store.rs` 被 `#[cfg(feature = "redis-store")]` 排除）.

### 6.3 代码质量验收

**AC-3.1** 测试覆盖
> The 新增 `redis_store.rs` shall 单元测试覆盖率 ≥ 90%（行覆盖），通过 `cargo tarpaulin --features redis-store` 验证；单元测试使用 mock Redis（`redis::Connection` 或 `MockRedis`），不依赖真实 Redis 实例.

**AC-3.2** 集成测试（真实 Redis，可选）
> The `redis_store.rs` shall 包含 `#[cfg(test)] mod integration_tests` 集成测试，通过 `REDIS_URL` 环境变量控制是否运行（未设置时跳过，`#[ignore]` 或环境变量门控），对真实 Redis 实例验证端到端行为.

**AC-3.3** Clippy
> The `cargo clippy --all-features -- -D warnings` shall 零警告.

**AC-3.4** rustdoc
> The `cargo doc --all-features --no-deps` shall 零警告，所有公开 API（`RedisRefreshTokenStore` / `RedisTokenBlacklist` / `RedisConfig`）有 rustdoc 注释.

**AC-3.5** 边界测试
> The 测试套件 shall 包含以下边界用例：(a) `user_id = 0` / 负数（若 trait 允许 i64）、(b) 空 `jti` 字符串、(c) 超长 `jti`（UUID v4 正常长度）、(d) `ttl_secs = 0` 跳过写入、(e) `ttl_secs = u64::MAX`（Redis 最大 TTL 限制）、(f) Redis 连接断开中途操作、(g) 并发 100 个 `increment_version` 同一用户、(h) 并发 100 个 `revoke` 同一 jti、(i) `key_prefix` 含特殊字符（`:` / `/`）.

**AC-3.6** 与 Memory 实现行为一致性测试
> The 测试套件 shall 包含一组「存储后端抽象测试」（`fn test_store_contract<S: RefreshTokenStore>()` 泛型测试），对 `MemoryRefreshTokenStore` 与 `RedisRefreshTokenStore` 执行同一套断言，验证二者行为完全一致（对齐 NFR-3.5）.

---

## 7. API 草案（供 design.md 细化）

```rust
// sz-rust-auth-facade/src/redis_store.rs
// 全模块受 #[cfg(feature = "redis-store")] 门控

use std::sync::Arc;
use std::time::Duration;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use crate::refresh::{RefreshTokenStore, TokenBlacklist, RefreshTokenError};

/// Redis 连接配置
///
/// 对齐 spec.md FR-4。`Debug` 实现对 `url` 中的密码脱敏（NFR-2.1）。
#[derive(Clone)]
pub struct RedisConfig {
    /// Redis 连接 URL（支持 `redis://[ :password@]host[:port][/db]`）
    pub url: String,
    /// 版本号 key 前缀（默认 "sso:ver"）
    pub key_prefix_ver: String,
    /// 黑名单 key 前缀（默认 "sso:bl"）
    pub key_prefix_bl: String,
    /// 建立连接超时（默认 3s）
    pub connection_timeout: Duration,
    /// 单条命令执行超时（默认 2s）
    pub command_timeout: Duration,
}

impl RedisConfig {
    /// 默认配置：`redis://127.0.0.1:6379`，前缀 `sso:ver` / `sso:bl`
    pub fn default() -> Self;

    /// 从 URL 创建配置，其余字段使用默认值
    pub fn from_url(url: impl Into<String>) -> Self;

    /// 创建 ConnectionManager（FR-3.3）
    ///
    /// 连接失败返回 `Err(RefreshTokenError::ServiceUnavailable)`（FR-3.5）
    pub async fn connect(&self) -> Result<ConnectionManager, RefreshTokenError>;
}

impl std::fmt::Debug for RedisConfig {
    /// `url` 中的密码脱敏为 `[REDACTED]`（FR-4.3 / NFR-2.1）
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

/// 基于 Redis 的 RefreshTokenStore 实现
///
/// 维护 `user_id → token_version`，key = `{key_prefix_ver}:{user_id}`。
/// `increment_version` 使用 Redis `INCR` 原子递增（FR-1.6）。
///
/// # 线程安全
///
/// `ConnectionManager` 满足 `Send + Sync + Clone`，内部 `Arc` 共享连接池。
pub struct RedisRefreshTokenStore {
    conn: ConnectionManager,
    key_prefix_ver: String,
    command_timeout: Duration,
}

impl RedisRefreshTokenStore {
    /// 从 RedisConfig 创建
    pub async fn new(config: &RedisConfig) -> Result<Self, RefreshTokenError>;

    /// 从已有 ConnectionManager 创建（复用连接池）
    pub fn with_conn(
        conn: ConnectionManager,
        key_prefix_ver: impl Into<String>,
        command_timeout: Duration,
    ) -> Self;
}

#[async_trait::async_trait]
impl RefreshTokenStore for RedisRefreshTokenStore {
    /// `GET sso:ver:{user_id}`，不存在返回 0（FR-1.2）
    async fn get_version(&self, user_id: i64) -> Result<u64, RefreshTokenError>;

    /// `INCR sso:ver:{user_id}`，原子递增（FR-1.3 / FR-1.6）
    async fn increment_version(&self, user_id: i64) -> Result<u64, RefreshTokenError>;
}

/// 基于 Redis 的 TokenBlacklist 实现
///
/// key = `{key_prefix_bl}:{jti}`。
/// `revoke` 使用 `SETEX`（FR-2.3），`is_revoked` 使用 `EXISTS`（FR-2.2）。
pub struct RedisTokenBlacklist {
    conn: ConnectionManager,
    key_prefix_bl: String,
    command_timeout: Duration,
}

impl RedisTokenBlacklist {
    /// 从 RedisConfig 创建
    pub async fn new(config: &RedisConfig) -> Result<Self, RefreshTokenError>;

    /// 从已有 ConnectionManager 创建（复用连接池）
    pub fn with_conn(
        conn: ConnectionManager,
        key_prefix_bl: impl Into<String>,
        command_timeout: Duration,
    ) -> Self;
}

#[async_trait::async_trait]
impl TokenBlacklist for RedisTokenBlacklist {
    /// `SETEX sso:bl:{jti} {ttl} 1`（FR-2.3），ttl=0 跳过（FR-2.4）
    async fn revoke(&self, jti: &str, ttl_secs: u64) -> Result<(), RefreshTokenError>;

    /// `EXISTS sso:bl:{jti}`（FR-2.2）
    async fn is_revoked(&self, jti: &str) -> Result<bool, RefreshTokenError>;
}

/// 便捷构造：同时创建 RedisRefreshTokenStore + RedisTokenBlacklist（共享同一 ConnectionManager）
pub async fn create_redis_stores(
    config: &RedisConfig,
) -> Result<(RedisRefreshTokenStore, RedisTokenBlacklist), RefreshTokenError>;
```

```toml
# sz-rust-auth-facade/Cargo.toml（新增 feature）

[features]
# ... 现有 features ...
# Redis 存储后端（RefreshTokenStore + TokenBlacklist 的 Redis 实现）
redis-store = ["dep:redis"]
# Redis 集群支持（可选，隐含 redis-store）
redis-cluster = ["redis-store", "redis/cluster"]
```

---

## 8. 依赖与影响分析

### 8.1 新增依赖

| crate | 版本 | feature | 用途 |
|-------|------|---------|------|
| `redis` | workspace 已有（`0.27`） | optional, gated by `redis-store` | Redis 客户端（`ConnectionManager` + `AsyncCommands`） |

> **注意**：`redis` 依赖已在 `sz-rust-auth-facade/Cargo.toml:22` 声明为 `optional = true`（当前用于 `redis-gateway` feature）。本规格新增 `redis-store` feature 复用该声明，不新增重复依赖声明（FR-5.4）。

### 8.2 影响的现有文件

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `sz-rust-auth-facade/src/lib.rs` | 新增 `#[cfg(feature = "redis-store")] pub mod redis_store;` | 模块声明（feature gate） |
| `sz-rust-auth-facade/Cargo.toml` | 新增 `redis-store` / `redis-cluster` feature | feature 定义（FR-5.1 / FR-6.1） |
| `sz-rust-auth-facade/src/redis_store.rs` | **新增文件** | `RedisRefreshTokenStore` + `RedisTokenBlacklist` + `RedisConfig` |

### 8.3 不影响的现有文件

- `sz-rust-auth-facade/src/refresh.rs`（`RefreshTokenStore` / `TokenBlacklist` trait + Memory 实现保持不变）
- `sz-rust-auth-facade/src/sso.rs`（SSO 认证中心通过 `Arc<dyn Trait>` 注入，不感知具体实现）
- `sz-rust-middleware-facade/src/sso_middleware.rs`（中间件通过 trait object 消费，不感知具体实现）
- `sz-rust-middleware-facade/src/{auth,jwt_blacklist,sanctum}.rs`（不修改）
- `sz-rust-auth-facade/src/{oauth,wechat,gateway,redis_gateway}.rs`（不修改）
- `sz-rust-sz300/`（业务应用通过配置选择存储后端，不强制修改；可选更新 `auth_service.rs` 初始化逻辑使用 Redis 实现）

### 8.4 对现有 SSO 机制的影响

| SSO 能力 | 影响方式 |
|----------|----------|
| `RefreshTokenIssuer::issue` | 无代码变更，运行时通过 `Arc<dyn RefreshTokenStore>` 注入 Redis 实现 |
| `RefreshTokenIssuer::rotate` | 无代码变更，`increment_version`（复用攻击检测）自动走 Redis `INCR` |
| `RefreshTokenVerifier::verify` | 无代码变更，`get_version` + `is_revoked` 自动走 Redis |
| `RefreshTokenRevoker::revoke` | 无代码变更，`revoke`（黑名单写入）自动走 Redis `SETEX` |
| `RefreshTokenRevoker::revoke_all` | 无代码变更，`increment_version` 自动走 Redis `INCR` |

**结论**：Redis 存储后端对上层 SSO 逻辑**零侵入**，仅通过依赖注入替换实现，符合开闭原则。

---

## 9. 风险与缓解

| 风险 | 等级 | 缓解 |
|------|------|------|
| Redis 单点故障导致 SSO 不可用 | 高 | NFR-4.1：fail-closed 返回 503；运维层面部署 Redis Sentinel / Cluster 高可用 |
| Redis 网络延迟拖慢 Token 校验 | 中 | NFR-1.1：p99 < 5ms 基线；`command_timeout` 超时降级；本地验签路径（JWT decode）不依赖 Redis，仅黑名单 / 版本号查询走 Redis |
| Redis 连接池耗尽 | 中 | `ConnectionManager` 内置连接池管理；`Clone` 仅增加 `Arc` 引用计数，不新建连接 |
| `INCR` 版本号溢出 `u64::MAX` | 极低 | NFR-4.5：显式处理溢出返回 `ServiceUnavailable`（实际 `u64::MAX` ≈ 1.8e19 不可能达到） |
| 黑名单 TTL 与 Token 实际过期不一致 | 中 | FR-2.7：TTL 由调用方（`RefreshTokenRevoker`）传入 `exp - now`，Redis 实现不自行计算；时钟漂移在秒级 TTL 下可忽略 |
| Redis 密码在日志 / Debug 输出中泄漏 | 高 | NFR-2.1 / FR-4.3：`RedisConfig::Debug` 脱敏；`tracing` 不记录 URL |
| feature unification 导致 `redis` 依赖被意外启用 | 低 | FR-5.3：`#[cfg(feature = "redis-store")]` 编译期隔离；`cargo tree --no-default-features` 验证 |
| semver 破坏 | 中 | NFR-3.1 / AC-2.7：`cargo-semver-checks` 验证；仅新增 API + 可选 feature |
| 集群模式下 `INCR` 跨分片 | 中 | FR-6.1：Redis Cluster 对同一 key 的 `INCR` 自动路由到对应分片（`hash slot`），原子性保证不变 |

---

## 10. 后续延伸（不在本次交付，仅记录）

- **Redis Sentinel 高可用**：连接 URL 指向 Sentinel 代理即可，无需应用层支持；如需原生 Sentinel 客户端，后续评估 `redis::sentinel` 支持
- **Redis Cluster 原生支持**：FR-6 已预留 `redis-cluster` feature，本次可仅实现单节点，集群作为可选增强
- **Redis 连接池监控指标**：暴露 `ConnectionManager` 的连接池使用率、重连次数等 metrics（对齐 `sz-rust-observability`）
- **多 Redis 实例隔离**：不同业务（SSO / 缓存 / 队列）使用不同 Redis 实例或不同 DB（`redis://host:port/0` vs `/1`），通过 `RedisConfig.url` 配置
- **黑名单批量查询优化**：当前 `is_revoked` 单次 `EXISTS`，若需批量校验多个 jti 可用 `MGET` / pipeline 优化（当前无此需求）
- **版本号持久化备份**：Redis RDB / AOF 持久化保证版本号不丢失，运维层面配置，应用层不感知

---

## 11. 变更记录

| 日期 | 版本 | 变更 | 作者 |
|------|------|------|------|
| 2026-08-08 | spec-v1.0 | 初稿，基于 sso-refresh-token spec-v1.0 与现有 `refresh.rs` 代码证据生成 | spec-requirement-agent |