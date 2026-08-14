# spec.md — SSO 单点登录 + Refresh Token 双 Token 机制

> **项目**：sz-rust（对标 ThinkPHP 8 的 Rust Web 框架，axum 0.8 + SZ-ORM）
> **版本**：v0.6.1 → v0.6.2（semver 兼容，仅新增 API）
> **规格版本**：spec-v1.0
> **创建日期**：2026-08-07
> **需求来源**：[从零实现 SSO 单点登录：JWT 双 Token + 远程校验完整方案](https://mp.weixin.qq.com/s/L2JI56kWLp53u-3xznbhLg)
> **目标 crate**：`sz-rust-auth-facade`（新增 `refresh.rs` + `sso.rs`）、`sz-rust-middleware-facade`（新增 `sso_middleware.rs`）
> **不修改**：上游 `sz-orm` 仓库、`sz-orm-auth` crate

---

## 0. 现状基线（基于代码证据，非猜测）

| 能力 | 现状 | 证据 |
|------|------|------|
| JWT HS256 签发/校验 | ✅ 已有 | `sz-rust-orm-facade/src/lib.rs:93` 重导出 `JwtEncoder / JwtClaims` |
| JWT 认证器 | ✅ 已有 | `sz-rust-sz300/src/services/auth_service.rs:8` `JwtAuthenticator` |
| JWT 中间件（白名单 + 签发人校验） | ✅ 已有 | `sz-rust-middleware-facade/src/auth.rs:305` `auth_middleware` |
| JWT 注销黑名单（Cache 存储） | ✅ 已有 | `sz-rust-middleware-facade/src/jwt_blacklist.rs:76` `JwtBlacklist` |
| Sanctum 个人访问令牌 | ✅ 已有 | `sz-rust-middleware-facade/src/sanctum.rs` |
| RBAC 权限检查 | ✅ 已有 | `sz-orm-auth::RbacAuthorizer` |
| OAuth2 客户端 | ✅ 已有 | `sz-rust-auth-facade/src/oauth.rs` |
| 微信 SDK | ✅ 已有 | `sz-rust-auth-facade/src/wechat.rs` |
| WebSocket Gateway | ✅ 已有 | `sz-rust-auth-facade/src/gateway.rs` |
| **Refresh Token 接口** | ❌ **空实现** | `sz-rust-sz300/src/controllers/auth.rs:172` `refresh` 返回空 `json!({})` |
| **SSO 单点登录** | ❌ **全项目无 SSO 代码** | `sz-rust-auth-facade/src/lib.rs:30-32` 仅 `gateway / oauth / wechat` 三模块 |
| **远程 Token 校验** | ❌ **仅 gRPC facade，无认证远程校验** | `sz-rust-orm-facade/src/grpc.rs:42` 为 ORM gRPC，与认证无关 |
| **JWT `aud`（接收人）校验** | ⚠️ 待 sz-orm-auth 升级 | `sz-rust-middleware-facade/src/auth.rs:81` 注释明确标注「延迟到后续」 |

**结论**：本规格仅新增「Refresh Token 双 Token 机制 + SSO 认证中心 + SSO 中间件 + 可选远程校验」四块能力，不重复造已有轮子。

---

## 1. 范围

### 1.1 In-Scope（本次交付）

| 编号 | 能力 | 落点 |
|------|------|------|
| FR-1 | Refresh Token 签发 | `sz-rust-auth-facade/src/refresh.rs`（新增） |
| FR-2 | Refresh Token 校验 | 同上 |
| FR-3 | Token 轮换（Rotation） | 同上 |
| FR-4 | Refresh Token 撤销 / 黑名单 | 同上，复用 `JwtBlacklist` |
| FR-5 | SSO 认证中心（登录/签发/校验/刷新/用户信息） | `sz-rust-auth-facade/src/sso.rs`（新增） |
| FR-6 | SSO 业务系统中间件（本地验签默认） | `sz-rust-middleware-facade/src/sso_middleware.rs`（新增） |
| FR-7 | 远程 Token 校验（可选 feature `remote-validate`） | `sz-rust-auth-facade/src/sso.rs` 内 `#[cfg(feature = "remote-validate")]` |

### 1.2 Out-of-Scope（明确不做，附理由）

| 项 | 理由 |
|----|------|
| AES 传输加密 | TLS 已覆盖传输层加密，应用层 AES 属重复造轮子且增加密钥管理负担 |
| Token 放 URL 参数 | 已有中间件统一从 `Authorization: Bearer <token>` 读取（`auth.rs:317`），URL 参数有日志泄漏风险 |
| `OkHttpClient` 每次 new | Rust 使用 `reqwest::Client` 单例（`Arc` 复用连接池），不存在此问题 |
| JWT `aud` 校验 | 依赖 `sz-orm-auth` 升级 `JwtClaims` 增加 `aud` 字段，属上游变更，本规格不修改上游 |
| 修改 `sz-orm` / `sz-orm-auth` | 项目铁律：不修改上游仓库 |

---

## 2. 术语与约定

| 术语 | 定义 |
|------|------|
| **accessToken** | 短期 JWT，用于业务 API 鉴权，默认 15 分钟过期 |
| **refreshToken** | 长期 JWT，仅用于换取新的 accessToken，默认 7 天过期，可撤销 |
| **Token 轮换（Rotation）** | 每次刷新同时签发新 accessToken + 新 refreshToken，旧 refreshToken 立即失效 |
| **SSO 认证中心** | 集中负责登录、签发、校验、刷新、撤销 Token 的服务 |
| **业务系统** | 接入 SSO 的下游应用（如 sz-pay），自身不持有签发能力 |
| **本地验签** | 业务系统与认证中心共享 JWT secret，业务系统本地 `JwtEncoder::decode` 验签，零网络开销 |
| **远程校验** | 密钥不共享场景，业务系统通过 HTTP/gRPC 调用认证中心校验端点 |
| **Token 黑名单** | 已撤销的 Token 集合，复用 `JwtBlacklist`（基于 `Cache` 存储） |

**默认参数**（可通过配置覆盖）：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `access_token_ttl` | 900 秒（15 min） | accessToken 有效期 |
| `refresh_token_ttl` | 604800 秒（7 天） | refreshToken 有效期 |
| `refresh_token_reuse_window` | 0 秒 | 旧 refreshToken 轮换后立即失效（无复用窗口，最严格） |
| `sso_issuer` | `"sz-rust-sso"` | JWT `iss` claim |
| `remote_validate_endpoint` | `"/sso/validate"` | 远程校验 HTTP 端点 |
| `remote_validate_timeout` | 3 秒 | 远程校验超时 |

---

## 3. 功能需求（EARS 格式）

> EARS 语法：
> - Ubiquitous：`The {system} shall {response}.`
> - Event-driven：`When {trigger}, the {system} shall {response}.`
> - State-driven：`While {state}, the {system} shall {response}.`
> - Optional feature：`Where {feature is included}, the {system} shall {response}.`
> - Unwanted：`If {trigger}, then the {system} shall {response}.`

### 3.1 FR-1 Refresh Token 签发

**FR-1.1**（Event-driven）
> When 用户在 SSO 认证中心通过用户名密码认证成功, the `RefreshTokenIssuer` shall 同时签发一个 accessToken（`exp = now + access_token_ttl`）和一个 refreshToken（`exp = now + refresh_token_ttl`，`token_type = "refresh"` claim）并返回 `TokenPair { access_token, refresh_token, access_expires_at, refresh_expires_at }`.

**FR-1.2**（Ubiquitous）
> The `RefreshTokenIssuer` shall 使用 `sz_orm_auth::jwt::JwtEncoder` 签发 refreshToken，复用现有 HS256 算法与 secret，不引入新签名算法.

**FR-1.3**（Ubiquitous）
> The `RefreshTokenIssuer` shall 在 refreshToken 的 JWT claims 中设置 `token_type = "refresh"` 自定义 claim，以区分 accessToken（`token_type = "access"`）.

**FR-1.4**（Unwanted）
> If 用户名或密码为空, then the `RefreshTokenIssuer` shall 返回 `Err(RefreshTokenError::InvalidCredentials)` 且不执行任何 DB 查询.

**FR-1.5**（Unwanted）
> If DB 查询用户记录失败或超时, then the `RefreshTokenIssuer` shall 返回 `Err(RefreshTokenError::ServiceUnavailable)` 且不向客户端泄漏内部错误细节.

### 3.2 FR-2 Refresh Token 校验

**FR-2.1**（Event-driven）
> When `RefreshTokenVerifier::verify(refresh_token)` 被调用, the `RefreshTokenVerifier` shall 执行以下校验链：(a) JWT 签名有效 → (b) 未过期 → (c) `token_type == "refresh"` → (d) 不在黑名单中 → (e) 签发人匹配 `sso_issuer`.

**FR-2.2**（Unwanted）
> If 任一校验步骤失败, then the `RefreshTokenVerifier` shall 返回对应的细分错误（`InvalidSignature` / `Expired` / `WrongTokenType` / `Revoked` / `IssuerMismatch`），不返回笼统的「校验失败」.

**FR-2.3**（Ubiquitous）
> The `RefreshTokenVerifier` shall 拒绝将 accessToken 用作 refreshToken（通过 `token_type` claim 区分），防止 Token 类型混用攻击.

### 3.3 FR-3 Token 轮换（Rotation）

**FR-3.1**（Event-driven）
> When 客户端持有效 refreshToken 调用刷新端点, the `RefreshTokenIssuer::rotate(refresh_token)` shall 签发全新的 `TokenPair`（新 accessToken + 新 refreshToken），并将旧 refreshToken 加入黑名单.

**FR-3.2**（State-driven）
> While `refresh_token_reuse_window == 0`（默认）, the `RefreshTokenIssuer` shall 在轮换后立即将旧 refreshToken 加入黑名单，旧 refreshToken 的任何后续使用都返回 `Err(RefreshTokenError::Revoked)`.

**FR-3.3**（Unwanted）
> If 客户端使用已轮换的旧 refreshToken 再次调用刷新端点, then the `RefreshTokenIssuer` shall 返回 `Err(RefreshTokenError::Revoked)` 并记录安全告警日志（`tracing::warn!`，含 `user_id` 与 `token_jti`，不含 token 明文）.

**FR-3.4**（Ubiquitous）
> The `RefreshTokenIssuer` shall 在每次轮换时为 refreshToken 生成唯一的 `jti`（JWT ID，UUID v4），用于黑名单精确定位与审计.

### 3.4 FR-4 Refresh Token 撤销 / 黑名单

**FR-4.1**（Event-driven）
> When 用户调用撤销端点（`/sso/revoke`）或退出登录, the `RefreshTokenRevoker::revoke(refresh_token)` shall 将该 refreshToken 的 `jti` 加入 `JwtBlacklist`，TTL 设为 refreshToken 的剩余有效期（避免黑名单无限增长）.

**FR-4.2**（Ubiquitous）
> The `RefreshTokenRevoker` shall 复用现有 `sz_rust_middleware_facade::jwt_blacklist::JwtBlacklist` 实现，不新建独立的黑名单存储.

**FR-4.3**（State-driven）
> While refreshToken 已过期, the `RefreshTokenRevoker::revoke` shall 直接返回 `Ok(())`（幂等），不写入黑名单（已过期 Token 天然失效，无需占用存储）.

**FR-4.4**（Event-driven）
> When 用户撤销 refreshToken 时, the `RefreshTokenRevoker` shall 同时撤销该用户当前关联的 accessToken（通过 `user_id + token_type=access` 维度的黑名单条目），实现「一处撤销，处处失效」.

### 3.5 FR-5 SSO 认证中心

**FR-5.1**（Ubiquitous）
> The `SsoCenter` shall 提供以下端点：`POST /sso/login`（登录签发双 Token）、`POST /sso/refresh`（轮换 Token）、`POST /sso/revoke`（撤销 Token）、`GET /sso/validate`（校验 Token）、`GET /sso/me`（获取用户信息）.

**FR-5.2**（Event-driven）
> When 客户端调用 `POST /sso/login` 且认证成功, the `SsoCenter` shall 返回 HTTP 200 与 JSON `{ "code": 0, "msg": "登录成功", "data": { "access_token", "refresh_token", "access_expires_at", "refresh_expires_at", "user_id", "username" } }`.

**FR-5.3**（Event-driven）
> When 客户端调用 `POST /sso/refresh` 持有效 refreshToken, the `SsoCenter` shall 返回新的 `TokenPair`，HTTP 200.

**FR-5.4**（Unwanted）
> If refreshToken 校验失败, then the `SsoCenter` shall 返回 HTTP 401 与 `{ "code": -1, "msg": "<细分错误描述>" }`，并设置响应头 `Cache-Control: no-store`.

**FR-5.5**（Event-driven）
> When 客户端调用 `GET /sso/validate` 持有效 accessToken, the `SsoCenter` shall 返回 HTTP 200 与 `{ "valid": true, "user_id": <id>, "expires_at": <unix_ts> }`.

**FR-5.6**（Ubiquitous）
> The `SsoCenter` shall 在所有 Token 相关响应中设置安全响应头：`Cache-Control: no-store`、`Pragma: no-cache`，防止 Token 被中间缓存存储.

**FR-5.7**（Ubiquitous）
> The `SsoCenter` shall 通过 `Authorization: Bearer <token>` Header 接收 Token，不从 URL 查询参数、Cookie、请求体读取 Token.

### 3.6 FR-6 SSO 业务系统中间件（本地验签默认）

**FR-6.1**（Ubiquitous）
> The `sso_middleware` shall 默认执行本地验签：使用与 SSO 认证中心共享的 JWT secret 调用 `JwtEncoder::decode`，零网络开销.

**FR-6.2**（Event-driven）
> When 请求到达业务系统且路由不在白名单中, the `sso_middleware` shall 从 `Authorization` Header 提取 accessToken，执行本地验签 + 黑名单查询，通过后将 `AuthenticatedUser { user_id }` 注入 request extensions.

**FR-6.3**（Unwanted）
> If accessToken 缺失、过期、签名无效、签发人不匹配、在黑名单中、或 `token_type != "access"`, then the `sso_middleware` shall 返回 HTTP 401 与 `BaseException::not_login`（复用现有 `auth.rs:331` 的错误响应格式）.

**FR-6.4**（State-driven）
> While 路由在白名单中（`allow_all_action` 列表，支持 `*` 通配符）, the `sso_middleware` shall 直接放行，不执行任何 Token 校验.

**FR-6.5**（Ubiquitous）
> The `sso_middleware` shall 拒绝使用 refreshToken 访问业务 API（通过 `token_type == "access"` 校验），refreshToken 仅用于 `/sso/refresh` 端点.

### 3.7 FR-7 远程 Token 校验（可选 feature `remote-validate`）

**FR-7.1**（Optional feature）
> Where feature `remote-validate` is enabled, the `sso_middleware` shall 支持远程校验模式：业务系统不持有 JWT secret，通过 HTTP 调用 SSO 认证中心的 `GET /sso/validate` 端点校验 accessToken.

**FR-7.2**（Optional feature）
> Where feature `remote-validate` is enabled and `RemoteValidateConfig.cache_ttl > 0`, the `sso_middleware` shall 在本地 `Cache` 缓存远程校验结果（key = `sha256(token)`，TTL = `cache_ttl`，默认 30 秒），减少远程调用次数.

**FR-7.3**（Optional feature）
> Where feature `remote-validate` is enabled, the `RemoteValidateTransport` shall 复用单个 `reqwest::Client` 实例（`Arc<reqwest::Client>`，内置连接池），禁止每次校验新建 Client.

**FR-7.4**（Unwanted）
> If 远程校验端点超时（超过 `remote_validate_timeout`）或返回非 2xx, then the `sso_middleware` shall 返回 HTTP 503 与 `BaseException`（`msg = "认证服务暂时不可用"`），不将请求放行.

**FR-7.5**（Optional feature）
> Where feature `remote-validate` is NOT enabled, the `sso_middleware` shall 编译期排除所有远程校验代码与 `reqwest` 依赖，零运行时开销.

---

## 4. 非功能需求（EARS 格式）

### 4.1 性能

**NFR-1.1**（Ubiquitous）
> The 本地验签路径（`sso_middleware` 本地模式）shall 在 p99 下单次 Token 校验耗时 < 1μs（不含网络 IO，仅 JWT decode + 黑名单查询），基准测试 `benches/sso_bench.rs` 须持续验证.

**NFR-1.2**（Ubiquitous）
> The `RefreshTokenIssuer::rotate` shall 在 p99 下完成 Token 轮换（签发 2 个 JWT + 1 次黑名单写入）耗时 < 50μs.

**NFR-1.3**（Optional feature）
> Where feature `remote-validate` is enabled and cache miss, the 远程校验 shall 在 `remote_validate_timeout`（默认 3 秒）内完成，超时即返回 503，不阻塞业务请求.

**NFR-1.4**（Ubiquitous）
> The `reqwest::Client` 单例 shall 在进程生命周期内复用，连接池大小默认 32，可通过 `RemoteValidateConfig.pool_max_idle` 配置.

### 4.2 安全

**NFR-2.1**（Ubiquitous）
> The `RefreshTokenIssuer` shall 拒绝签发 `token_type` 缺失的 JWT，所有签发的 Token 必须显式包含 `token_type` claim.

**NFR-2.2**（Ubiquitous）
> The 所有 Token 相关响应 shall 通过 `Cache-Control: no-store` 头防止中间缓存存储 Token.

**NFR-2.3**（Ubiquitous）
> The `SsoCenter` 的日志 shall 仅记录 Token 的 `jti`（JWT ID）与 `user_id`，禁止记录 Token 明文、secret、用户密码（即使哈希）.

**NFR-2.4**（Ubiquitous）
> The `RefreshTokenIssuer` 签发的 refreshToken shall 通过 `HttpOnly; Secure; SameSite=Strict` Cookie 下发（可选模式），或通过 JSON 响应体下发（默认模式），禁止通过 URL 参数下发.

**NFR-2.5**（Unwanted）
> If 检测到 refreshToken 复用攻击（同一 `jti` 在轮换后再次出现）, then the `SsoCenter` shall 撤销该用户的所有 Token（access + refresh），记录安全告警，并返回 401.

**NFR-2.6**（Ubiquitous）
> The `AuthConfig`（含 JWT secret）的 `Debug` 实现 shall 将 `secret` 字段脱敏为 `"[REDACTED]"`（对齐现有 `auth.rs:179` 的 `Debug` 实现）.

**NFR-2.7**（Ubiquitous）
> The 所有 `async fn` shall 满足 `Send + 'static`（项目铁律），可在 tokio 多线程运行时安全调度.

### 4.3 兼容性

**NFR-3.1**（Ubiquitous）
> The 本次新增的 API shall 不修改 `sz-rust-auth-facade` 现有公开 API（`gateway` / `oauth` / `wechat` 模块签名不变），semver minor 版本升级（0.6.1 → 0.6.2）.

**NFR-3.2**（Ubiquitous）
> The `sz-rust-middleware-facade` 现有 `auth_middleware` shall 保持不变，`sso_middleware` 为新增独立中间件，不替换 `auth_middleware`.

**NFR-3.3**（Ubiquitous）
> The `sz-rust-sz300` 的 `controllers/auth.rs:172` 空实现的 `refresh` 函数 shall 改为调用 `RefreshTokenIssuer::rotate`，保持 HTTP 路由 `/auth/refresh` 不变，对前端透明.

**NFR-3.4**（Ubiquitous）
> The 现有 `sz-pay` 项目（路径 `E:\vue\test\sz-pay`）shall 在升级 sz-rust 至 0.6.2 后无需修改业务代码即可编译通过.

### 4.4 可靠性

**NFR-4.1**（State-driven）
> While 黑名单存储（`Cache`）暂时不可用, the `sso_middleware` shall fail-closed（拒绝所有请求，返回 503），禁止 fail-open 放行.

**NFR-4.2**（Ubiquitous）
> The `RefreshTokenRevoker::revoke` shall 幂等：对同一 refreshToken 多次撤销返回相同 `Ok(())`，不报错.

**NFR-4.3**（Ubiquitous）
> The `RefreshTokenIssuer::rotate` shall 在签发新 Token 与加入黑名单之间使用事务性顺序（先黑名单后签发或先签发后黑名单二选一并固定），防止并发轮换导致 Token 泄漏.

---

## 5. 约束条件

### 5.1 项目铁律（来自 `AGENTS.md` 与 `.trae/rules/project_rules.md`）

| 编号 | 约束 | 验证方式 |
|------|------|----------|
| C-1 | 所有 `async fn` 必须 `Send + 'static` | 编译期 trait bound 检查 |
| C-2 | 禁止 `std::fs`，统一 `tokio::fs` | `clippy` 自定义 lint 或 grep 检查 |
| C-3 | 敏感字段 `#[serde(skip_serializing)]` 自动脱敏 | 代码审查 + 序列化测试 |
| C-4 | 不引入新 `unsafe` 代码 | `workspace.lints.rust.unsafe_code = "forbid"` |
| C-5 | 不破坏 sz-rust 公开 API（semver 兼容） | `cargo-semver-checks` |
| C-6 | 不修改上游 `sz-orm` 仓库 | PR diff 不含 `sz-orm/` 路径 |
| C-7 | `workspace.unsafe_code = "forbid"`，个别包单独 allow | `Cargo.toml` lint 配置 |

### 5.2 本规格特有约束

| 编号 | 约束 |
|------|------|
| C-8 | 新增代码仅落在 `sz-rust-auth-facade/src/{refresh,sso}.rs` 与 `sz-rust-middleware-facade/src/sso_middleware.rs`，不散落到其他 crate |
| C-9 | 远程校验依赖 `reqwest` 仅在 `remote-validate` feature 下引入，默认构建零网络依赖 |
| C-10 | Refresh Token 存储抽象为 `RefreshTokenStore` trait，默认提供 `MemoryRefreshTokenStore` 与 `CacheRefreshTokenStore`（基于现有 `Cache`），不强制依赖 Redis |
| C-11 | 所有新增公开 API 须有 rustdoc 注释 + 至少 1 个单元测试（对齐项目现有风格） |
| C-12 | `tracing` 日志 span 须使用 `#[tracing::instrument(skip(secret, token))]` 跳过敏感参数（对齐 `auth_service.rs:56`） |

---

## 6. 验收标准（EARS 格式，可测试）

### 6.1 功能验收

**AC-1.1** 登录签发双 Token
> When 调用 `POST /sso/login` 提供正确用户名密码, then 响应 JSON `data` 字段 shall 同时包含 `access_token`（解码后 `token_type == "access"`，`exp - iat ≈ 900`）与 `refresh_token`（解码后 `token_type == "refresh"`，`exp - iat ≈ 604800`）.

**AC-1.2** Token 轮换
> When 持有效 `refresh_token` 调用 `POST /sso/refresh`, then 响应 shall 返回全新的 `TokenPair`，且旧 `refresh_token` 再次调用刷新 shall 返回 401 `Revoked`.

**AC-1.3** Token 类型隔离
> When 使用 `access_token` 调用 `POST /sso/refresh`, then 响应 shall 返回 401 `WrongTokenType`，禁止 accessToken 用作 refreshToken.
> When 使用 `refresh_token` 访问业务 API（经 `sso_middleware`）, then 响应 shall 返回 401，禁止 refreshToken 用作 accessToken.

**AC-1.4** 撤销生效
> When 调用 `POST /sso/revoke` 撤销 refreshToken 后，再使用该 refreshToken 调用 `POST /sso/refresh`, then 响应 shall 返回 401 `Revoked`.

**AC-1.5** SSO 中间件本地验签
> When 业务系统配置 `SsoMiddlewareConfig::local(secret)` 且请求持有效 accessToken, then `sso_middleware` shall 本地验签通过，`request.extensions().get::<AuthenticatedUser>()` 返回 `Some(AuthenticatedUser { user_id })`.

**AC-1.6** SSO 中间件白名单
> When 请求路由匹配 `allow_all_action` 列表（含 `*` 通配符）, then `sso_middleware` shall 直接放行，不校验 Token.

**AC-1.7** 远程校验（feature `remote-validate`）
> Where feature `remote-validate` enabled and 业务系统配置 `SsoMiddlewareConfig::remote(endpoint)`, then `sso_middleware` shall 调用远程 `GET /sso/validate`，校验通过后放行.
> Where `cache_ttl > 0`, then 相同 Token 在 TTL 内的重复校验 shall 命中本地缓存，不发起远程调用（通过 mock transport 断言调用次数 == 1）.

### 6.2 非功能验收

**AC-2.1** 本地验签性能
> The `cargo bench --bench sso_bench -- local_validate` shall 报告 p99 < 1μs（在标准开发机 i7-12700H @ 2.3GHz 基准上）.

**AC-2.2** Token 轮换性能
> The `cargo bench --bench sso_bench -- rotate` shall 报告 p99 < 50μs.

**AC-2.3** 无 unsafe
> The `cargo build --all-features` shall 零 `unsafe_code` 警告（workspace `forbid` 生效）.

**AC-2.4** semver 兼容
> The `cargo semver-checks check-release` shall 通过，无 breaking change.

**AC-2.5** sz-pay 兼容
> The `sz-pay` 项目在 `Cargo.toml` 中将 `sz-rust` 升级至 `0.6.2` 后，`cargo build` shall 成功，无编译错误.

**AC-2.6** 日志脱敏
> When 启用 `tracing` 日志并触发 Token 轮换, then 日志输出 shall 不包含 Token 明文、JWT secret、用户密码，仅包含 `jti`、`user_id`、错误类型.

**AC-2.7** fail-closed
> When 黑名单 `Cache` 故意返回错误, then `sso_middleware` shall 返回 503，不放行任何请求.

**AC-2.8** 复用攻击检测
> When 同一 `jti` 的 refreshToken 在轮换后再次用于刷新, then `SsoCenter` shall 撤销该用户所有 Token，返回 401，并记录 `tracing::warn!` 告警含 `user_id` 与 `jti`.

### 6.3 代码质量验收

**AC-3.1** 测试覆盖
> The 新增 `refresh.rs` / `sso.rs` / `sso_middleware.rs` shall 单元测试覆盖率 ≥ 90%（行覆盖），通过 `cargo tarpaulin` 验证.

**AC-3.2** Clippy
> The `cargo clippy --all-features -- -D warnings` shall 零警告.

**AC-3.3** rustdoc
> The `cargo doc --all-features --no-deps` shall 零警告，所有公开 API 有 rustdoc 注释.

**AC-3.4** 边界测试
> The 测试套件 shall 包含以下边界用例：(a) 空 Token、(b) 篡改的 Token、(c) 过期 1 秒的 Token、(d) `token_type` 缺失的 Token、(e) 黑名单查询超时、(f) refreshToken 撤销后幂等再次撤销、(g) 并发 100 个轮换请求.

---

## 7. API 草案（供 design.md 细化）

```rust
// sz-rust-auth-facade/src/refresh.rs

pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: i64,
    pub refresh_expires_at: i64,
}

pub struct RefreshTokenConfig {
    pub access_token_ttl: Duration,    // 默认 900s
    pub refresh_token_ttl: Duration,   // 默认 604800s
    pub issuer: String,                // 默认 "sz-rust-sso"
    // secret 通过 JwtEncoder 注入，不在此结构
}

pub struct RefreshTokenIssuer {
    encoder: JwtEncoder,
    blacklist: JwtBlacklist,
    config: RefreshTokenConfig,
}

impl RefreshTokenIssuer {
    pub fn new(encoder: JwtEncoder, blacklist: JwtBlacklist, config: RefreshTokenConfig) -> Self;
    pub async fn issue(&self, user_id: i64, username: &str) -> Result<TokenPair, RefreshTokenError>;
    pub async fn rotate(&self, old_refresh_token: &str) -> Result<TokenPair, RefreshTokenError>;
    pub async fn revoke(&self, refresh_token: &str) -> Result<(), RefreshTokenError>;
}

pub struct RefreshTokenVerifier { /* ... */ }
impl RefreshTokenVerifier {
    pub fn new(encoder: JwtEncoder, blacklist: JwtBlacklist, issuer: String) -> Self;
    pub async fn verify(&self, token: &str) -> Result<RefreshTokenClaims, RefreshTokenError>;
}

#[derive(Debug, thiserror::Error)]
pub enum RefreshTokenError {
    #[error("invalid credentials")] InvalidCredentials,
    #[error("invalid signature")]   InvalidSignature,
    #[error("token expired")]       Expired,
    #[error("wrong token type")]    WrongTokenType,
    #[error("token revoked")]       Revoked,
    #[error("issuer mismatch")]     IssuerMismatch,
    #[error("service unavailable")] ServiceUnavailable,
    #[error("cache error: {0}")]    Cache(String),
}
```

```rust
// sz-rust-middleware-facade/src/sso_middleware.rs

pub enum SsoMiddlewareConfig {
    Local { secret: String, issuer: String, blacklist: JwtBlacklist, allow_all_action: Vec<String> },
    #[cfg(feature = "remote-validate")]
    Remote { endpoint: String, timeout: Duration, cache: Arc<Cache>, cache_ttl: Duration, allow_all_action: Vec<String> },
}

pub async fn sso_middleware(
    State(config): State<SsoMiddlewareConfig>,
    req: Request,
    next: Next,
) -> Response;
```

---

## 8. 依赖与影响分析

### 8.1 新增依赖

| crate | 版本 | feature | 用途 |
|-------|------|---------|------|
| `uuid` | workspace 已有 | `v4` | refreshToken `jti` 生成 |
| `reqwest` | workspace 已有 | optional, gated by `remote-validate` | 远程校验 HTTP 客户端 |

### 8.2 影响的现有文件

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `sz-rust-auth-facade/src/lib.rs` | 新增 `pub mod refresh; pub mod sso;` | 模块声明 |
| `sz-rust-auth-facade/Cargo.toml` | 新增 `remote-validate` feature | 可选依赖 |
| `sz-rust-middleware-facade/src/lib.rs` | 新增 `pub mod sso_middleware;` | 模块声明 |
| `sz-rust-sz300/src/controllers/auth.rs:172` | 替换空实现 | `refresh` 调用 `RefreshTokenIssuer::rotate` |
| `sz-rust-sz300/src/services/auth_service.rs` | 新增 SSO 初始化 | 复用 `init_auth` 扩展 |

### 8.3 不影响的现有文件

- `sz-rust-middleware-facade/src/auth.rs`（现有 JWT 中间件保持不变）
- `sz-rust-middleware-facade/src/jwt_blacklist.rs`（复用，不修改）
- `sz-rust-middleware-facade/src/sanctum.rs`（独立，不修改）
- `sz-rust-auth-facade/src/{oauth,wechat,gateway,redis_gateway}.rs`（不修改）

---

## 9. 风险与缓解

| 风险 | 等级 | 缓解 |
|------|------|------|
| refreshToken 复用攻击（被盗后并发轮换） | 高 | FR-3.3 + NFR-2.5：检测到复用即撤销用户所有 Token |
| 黑名单存储故障导致 fail-open | 高 | NFR-4.1：fail-closed，返回 503 |
| 远程校验单点故障 | 中 | FR-7.2：本地缓存 + 超时降级；可配多 endpoint（后续） |
| JWT secret 泄漏 | 高 | NFR-2.6：Debug 脱敏；C-12：tracing skip；secret 仅 env 注入 |
| Token 在日志中泄漏 | 中 | NFR-2.3：仅记 `jti` + `user_id` |
| semver 破坏 | 中 | NFR-3.1 + AC-2.4：cargo-semver-checks 验证 |

---

## 10. 后续延伸（不在本次交付，仅记录）

- JWT `aud`（接收人）校验：待 `sz-orm-auth` 升级 `JwtClaims` 后补充
- SSO 多认证中心联邦（OIDC Federation）：未来需求
- Token 续期无感刷新（前端拦截器 + silent refresh）：前端职责，本规格仅提供后端能力
- 跨域 SSO（CORS + Cookie `SameSite=None`）：视前端架构而定

---

## 11. 变更记录

| 日期 | 版本 | 变更 | 作者 |
|------|------|------|------|
| 2026-08-07 | spec-v1.0 | 初稿，基于代码现状与参考文章生成 | spec-requirement-agent |