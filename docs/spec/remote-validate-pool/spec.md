# spec.md — 远程校验连接池优化（RemoteValidateConfig）

> **项目**：sz-rust（对标 ThinkPHP 8 的 Rust Web 框架，axum 0.8 + SZ-ORM）
> **版本**：v0.6.3 → v0.6.4（semver 兼容，仅新增 API，不破坏现有公开签名）
> **规格版本**：spec-v1.0
> **创建日期**：2026-08-08
> **基于规格**：[sso-refresh-token/spec.md](../sso-refresh-token/spec.md)（spec-v1.0，FR-7 远程校验）
> **需求来源**：v0.6.3 `RemoteValidateConfig::new()` 使用 `.expect()` panic、连接池参数硬编码、缺少 builder / from_client 等灵活配置入口
> **目标 crate**：`sz-rust-middleware-facade`（`src/sso_middleware.rs`，feature gate `remote-validate`）
> **不修改**：上游 `sz-orm` 仓库、`sz-orm-auth` crate、feature gate `remote-validate` 定义、`sso_middleware_remote` 中间件签名

---

## 0. 现状基线（基于代码证据，非猜测）

| 能力 | 现状 | 证据 |
|------|------|------|
| `RemoteValidateConfig` 结构体 | ✅ 已有 | `sso_middleware.rs:170` — `endpoint / timeout / client / allow_all_action` |
| `new()` 构造函数 | ⚠️ 会 panic | `sso_middleware.rs:193` — `.expect("failed to build reqwest client")` |
| `pool_max_idle_per_host` | ⚠️ 硬编码 32 | `sso_middleware.rs:191` — 无法配置 |
| `pool_idle_timeout` | ❌ 缺失 | 未调用 `reqwest::ClientBuilder::pool_idle_timeout` |
| `tcp_keepalive` | ❌ 缺失 | 未调用 `reqwest::ClientBuilder::tcp_keepalive` |
| `tcp_nodelay` | ❌ 缺失 | 未调用 `reqwest::ClientBuilder::tcp_nodelay` |
| builder 模式 | ❌ 缺失 | 无 `RemoteValidateConfigBuilder` |
| `from_client` 方法 | ❌ 缺失 | 无法外部注入预配置 `reqwest::Client` |
| `new_checked` / `new_or_default` | ❌ 缺失 | 无失败安全构造入口 |
| feature gate `remote-validate` | ✅ 已有 | `Cargo.toml:46` — `remote-validate = ["dep:reqwest"]` |

**结论**：本规格仅优化 `RemoteValidateConfig` 的构造与连接池配置，不改动中间件执行逻辑（`sso_middleware_remote` 签名与行为不变）。

---

## 1. 范围

### 1.1 In-Scope（本次交付）

| 编号 | 能力 | 落点 |
|------|------|------|
| FR-1 | `new_checked()` 返回 `Result<Self, RefreshTokenError>`（失败安全构造） | `sso_middleware.rs` |
| FR-2 | `new()` 保留原签名（向后兼容），内部委托 `new_checked().expect()` | 同上 |
| FR-3 | `new_or_default()` 便捷方法（构造失败回退默认 Client） | 同上 |
| FR-4 | `RemoteValidateConfigBuilder` 链式 builder | 同上 |
| FR-5 | 连接池参数可配置（`pool_max_idle_per_host` / `pool_idle_timeout` / `tcp_keepalive` / `tcp_nodelay`） | 同上 |
| FR-6 | `from_client()` 方法（外部注入预配置 `reqwest::Client`） | 同上 |

### 1.2 Out-of-Scope（明确不做，附理由）

| 项 | 理由 |
|----|------|
| 修改 `sso_middleware_remote` 签名 | 中间件已稳定，仅消费 `Arc<RemoteValidateConfig>`，无需改动 |
| 修改 feature gate `remote-validate` 定义 | 约束明确要求保持不变 |
| 修改上游 `sz-orm` / `sz-orm-auth` | 项目铁律：不修改上游仓库 |
| `new()` 改为返回 `Result` | 属 breaking change，破坏 semver；改用 `new_checked` 承载失败安全语义 |
| 新增 HTTPS / TLS 配置 | 传输层安全属 `reqwest::Client` 外部职责，`from_client` 已可注入预配置 Client |
| 连接池 metrics / 监控 | 属可观测性扩展，非本次连接池调优范围 |

---

## 2. 术语与约定

| 术语 | 定义 |
|------|------|
| **连接池调优参数** | 影响 `reqwest::Client` 内置连接池行为的配置项：`pool_max_idle_per_host`、`pool_idle_timeout`、`tcp_keepalive`、`tcp_nodelay` |
| **失败安全构造** | 构造函数返回 `Result`，Client 构建失败时返回 `Err` 而非 panic |
| **builder 模式** | 通过链式方法逐步填充配置字段，最终 `.build()` 产出 `RemoteValidateConfig` |

**默认参数**（可通过 builder 覆盖）：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `pool_max_idle_per_host` | 32 | 每 host 最大空闲连接数（对齐 v0.6.3 现有行为） |
| `pool_idle_timeout` | 90 秒 | 空闲连接超时（`reqwest` 默认 90s） |
| `tcp_keepalive` | None | TCP keepalive，None 表示使用系统默认 |
| `tcp_nodelay` | true | 禁用 Nagle 算法，降低小包延迟 |

---

## 3. 功能需求（EARS 格式）

> EARS 语法：Ubiquitous `The {system} shall {response}.` / Event-driven `When {trigger}, the {system} shall {response}.` / Unwanted `If {trigger}, then the {system} shall {response}.` / Optional feature `Where {feature is included}, the {system} shall {response}.`

### 3.1 FR-1 `new_checked` 失败安全构造

**FR-1.1**（Optional feature）
> Where feature `remote-validate` is enabled, the `RemoteValidateConfig::new_checked(endpoint, timeout, allow_all_action)` shall 返回 `Result<Self, RefreshTokenError>`，`reqwest::Client::build()` 失败时返回 `Err(RefreshTokenError::ServiceUnavailable)` 而非 panic.

**FR-1.2**（Unwanted）
> If `endpoint` 为空字符串, then the `new_checked` shall 返回 `Err(RefreshTokenError::InvalidConfig)`（新增错误变体），不构造 Client.

### 3.2 FR-2 `new` 向后兼容

**FR-2.1**（Ubiquitous）
> The `RemoteValidateConfig::new(endpoint, timeout, allow_all_action)` shall 保留 v0.6.3 的签名 `fn new(...) -> Self`，内部委托 `new_checked(...).expect("failed to build reqwest client")`，保持现有调用方零改动.

### 3.3 FR-3 `new_or_default` 便捷回退

**FR-3.1**（Event-driven）
> When `new_checked` 构造失败, the `new_or_default(endpoint, timeout, allow_all_action)` shall 回退使用 `reqwest::Client::new()`（默认配置）构造 `RemoteValidateConfig` 并返回 `Self`，记录 `tracing::warn!` 告警日志（含失败原因，不含敏感字段）.

### 3.4 FR-4 Builder 模式

**FR-4.1**（Ubiquitous）
> The `RemoteValidateConfigBuilder` shall 提供链式方法：`endpoint()` / `timeout()` / `allow_all_action()` / `pool_max_idle_per_host()` / `pool_idle_timeout()` / `tcp_keepalive()` / `tcp_nodelay()`，最终 `.build()` 返回 `Result<RemoteValidateConfig, RefreshTokenError>`.

**FR-4.2**（Ubiquitous）
> The `RemoteValidateConfigBuilder` shall 为所有连接池参数提供默认值（见第 2 节），未显式设置的字段使用默认值，保证 `.build()` 在仅设置 `endpoint` 后即可成功.

**FR-4.3**（Unwanted）
> If builder 未设置 `endpoint`, then the `.build()` shall 返回 `Err(RefreshTokenError::InvalidConfig)`，`endpoint` 为必填项.

### 3.5 FR-5 连接池参数可配置

**FR-5.1**（Ubiquitous）
> The `RemoteValidateConfigBuilder::build()` shall 将 `pool_max_idle_per_host` / `pool_idle_timeout` / `tcp_keepalive` / `tcp_nodelay` 四个参数透传至 `reqwest::ClientBuilder` 对应方法，未设置项使用默认值.

**FR-5.2**（State-driven）
> While `pool_idle_timeout` 设为 `None`, the builder shall 不调用 `reqwest::ClientBuilder::pool_idle_timeout`（使用 reqwest 内置默认 90s）.

**FR-5.3**（State-driven）
> While `tcp_keepalive` 设为 `None`, the builder shall 不调用 `reqwest::ClientBuilder::tcp_keepalive`（使用系统默认）.

### 3.6 FR-6 `from_client` 外部注入

**FR-6.1**（Event-driven）
> When 调用方通过 `from_client(endpoint, timeout, allow_all_action, client)` 传入预配置的 `reqwest::Client`, the `RemoteValidateConfig` shall 直接持有该 Client（`Arc` 复用），跳过内部 `ClientBuilder` 流程.

**FR-6.2**（Ubiquitous）
> The `from_client` shall 不覆盖传入 Client 的任何配置（timeout / 连接池参数等均由调用方负责），本方法仅负责组装 `RemoteValidateConfig` 字段.

---

## 4. 非功能需求（EARS 格式）

### 4.1 兼容性

**NFR-1.1**（Ubiquitous）
> The 本次变更 shall 不修改 `RemoteValidateConfig` 现有公开字段（`endpoint` / `timeout` / `allow_all_action` 为 `pub`，`client` 为私有），semver minor 升级（0.6.3 → 0.6.4）.

**NFR-1.2**（Ubiquitous）
> The 现有 `new()` 调用方 shall 零改动编译通过，`new()` 行为与 v0.6.3 一致（成功返回 `Self`，失败 panic）.

**NFR-1.3**（Ubiquitous）
> The `sso_middleware_remote` 中间件 shall 不感知 `RemoteValidateConfig` 的构造方式（统一通过 `Arc<RemoteValidateConfig>` 消费），构造优化对中间件透明.

### 4.2 安全

**NFR-2.1**（Ubiquitous）
> The `RemoteValidateConfig` 的 `Debug` 实现 shall 不输出 `reqwest::Client` 内部状态（Client 无敏感信息，但避免意外泄漏连接池句柄），仅输出 `endpoint / timeout / allow_all_action / 池参数`.

**NFR-2.2**（Ubiquitous）
> The 所有 `async fn` shall 满足 `Send + 'static`（项目铁律）.

### 4.3 可维护性

**NFR-3.1**（Ubiquitous）
> The `RefreshTokenError` shall 新增 `InvalidConfig` 变体（`#[error("invalid config: {0}")]`），承载 `new_checked` / builder 的配置校验失败，不复用 `ServiceUnavailable` 混淆语义.

**NFR-3.2**（Ubiquitous）
> The 新增 API shall 在 `#[cfg(feature = "remote-validate")]` 下编译，feature 未启用时零代码、零依赖.

---

## 5. 验收清单

| 编号 | 验收项 | 验证方式 |
|------|--------|----------|
| AC-1 | `new_checked` 在 Client 构建失败时返回 `Err`，不 panic | 单元测试：mock 致命配置触发 `build()` 失败 |
| AC-2 | `new()` 行为与 v0.6.3 一致（成功返回 `Self`） | 回归测试：现有 `new()` 调用编译通过且返回有效配置 |
| AC-3 | `new_or_default` 在失败时回退默认 Client 并记录 warn 日志 | 单元测试 + 日志断言 |
| AC-4 | builder 链式配置四项连接池参数均透传至 `ClientBuilder` | 单元测试：构造后通过 `from_client` 反向校验（或文档化行为测试） |
| AC-5 | builder 未设 `endpoint` 时 `.build()` 返回 `Err(InvalidConfig)` | 单元测试 |
| AC-6 | `from_client` 持有外部 Client，不触发内部 `ClientBuilder` | 单元测试：传入自定义 Client，校验 `config.client` 与传入一致 |
| AC-7 | feature `remote-validate` 关闭时编译通过，无 `reqwest` 符号 | `cargo build --no-default-features` 编译验证 |
| AC-8 | 所有 `async fn` 满足 `Send + 'static` | `cargo clippy` + 编译期 Send 约束验证 |