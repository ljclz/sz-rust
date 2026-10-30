# tasks.md — SSO 单点登录 + Refresh Token 双 Token 机制 · 编码任务规划

> **项目**：sz-rust（对标 ThinkPHP 8 的 Rust Web 框架，axum 0.8 + SZ-ORM）
> **版本**：v0.6.1 → v0.6.2（semver 兼容，仅新增 API）
> **任务规划版本**：tasks-v1.0
> **创建日期**：2026-08-07
> **基于规格**：[spec.md](./spec.md)（spec-v1.0）
> **基于设计**：[design.md](./design.md)（design-v1.0）
> **目标 crate**：`sz-rust-auth-facade`（新增 `refresh.rs` + `sso.rs`）、`sz-rust-middleware-facade`（新增 `sso_middleware.rs`）

---

## 0. 现状基线（代码证据，已实际验证）

| 项 | 现状 | 证据（file:line） |
|----|------|-------------------|
| workspace 版本 | `0.6.1`，edition 2021，rust-version 1.81 | `Cargo.toml:36-38` |
| workspace 已有依赖 | tokio, async-trait, axum 0.8, serde, tracing, parking_lot, thiserror 2, chrono, uuid 1 (v4+serde), reqwest 0.12, sha2 0.10, base64 0.22 | `Cargo.toml:55-131` |
| workspace 缺失依赖 | `hmac`, `subtle`（需新增） | grep 全 workspace 无命中 |
| auth-facade 模块 | 仅 `gateway / oauth / wechat` + `redis_gateway`(feature) | `sz-rust-auth-facade/src/lib.rs:30-38` |
| auth-facade Cargo.toml | 仅有 parking_lot/serde/serde_json/hex/sha1/thiserror/redis/futures | `sz-rust-auth-facade/Cargo.toml:14-30` |
| middleware-facade 模块 | auth/builder/chain/circuit_breaker/cors/csrf/handler_as_middleware/jwt_blacklist/log/order/rate_limit/request_scope/sanctum/tower_compat/trace | `sz-rust-middleware-facade/src/lib.rs:26-40` |
| middleware-facade Cargo.toml | 已有 axum/tower/sha2/base64/chrono/tracing/thiserror + sz-rust-{orm,cache,http,infra}-facade；缺 sz-rust-auth-facade/uuid/reqwest | `sz-rust-middleware-facade/Cargo.toml:14-36` |
| JwtBlacklist API | `new(cache, config)` / `default_with_memory_cache()` / `revoke(token, ttl) -> Result<bool, JwtBlacklistError>` / `is_revoked(token) -> Result<bool, JwtBlacklistError>` — **同步方法** | `sz-rust-middleware-facade/src/jwt_blacklist.rs:82-162` |
| sz300 refresh 空实现 | `refresh` 返回 `json!({})` | `sz-rust-sz300/src/controllers/auth.rs:172-176` |
| workspace lint | `unsafe_code = "forbid"`（通过 `[lints] workspace = true` 继承） | 各 crate `Cargo.toml` 末尾 `[lints] workspace = true` |

---

## 1. 任务总览与依赖拓扑

```
T0 (前置: workspace 依赖)
  │
  ├──▶ T1 (基础设施: Cargo.toml + lib.rs + RefreshTokenError + SsoClaims + SsoJwtCodec)
  │      │
  │      ├──▶ T2 (RefreshTokenStore trait + Memory/Cache 实现)
  │      │      │
  │      │      ├──▶ T3 (RefreshTokenVerifier)
  │      │      │      │
  │      │      │      ├──▶ T4 (RefreshTokenIssuer: issue/rotate/revoke)
  │      │      │      │      │
  │      │      │      │      ├──▶ T5 (RefreshTokenRevoker)
  │      │      │      │      │      │
  │      │      │      │      │      ├──▶ T6 (SsoService 核心逻辑 + trait 抽象)
  │      │      │      │      │      │      │
  │      │      │      │      │      │      ├──▶ T7 (SsoCenter axum HTTP 端点, feature=axum)
  │      │      │      │      │      │      │      │
  │      │      │      │      │      │      │      └──▶ T9 (sz300 集成: 替换空实现 + 初始化)
  │      │      │      │      │      │      │
  │      │      │      │      │      │      └──▶ T8 (sso_middleware: 本地验签 + 远程校验 feature)
  │      │      │      │      │      │             │
  │      │      │      │      │      │             └──▶ T9 (sz300 集成)
  │      │      │      │      │      │
  │      │      │      │      │      └──▶ T7
  │      │      │      │      │
  │      │      │      │      └──▶ T5
  │      │      │      │
  │      │      │      └──▶ T5
  │      │      │
  │      │      └──▶ T4
  │      │
  │      └──▶ T3
  │
  └──▶ T10 (单元测试 + 边界测试)
         │
         ├──▶ T11 (集成测试 + 基准测试)
         │      │
         │      └──▶ T12 (文档 + CHANGELOG + 版本 bump + semver-checks + crates.io)
```

**任务清单**：

| 任务 | 标题 | 预估行数 | 预估耗时 | 依赖 |
|------|------|---------|---------|------|
| T0 | workspace 依赖准备 | +2 行 | 5 min | — |
| T1 | 基础设施（Cargo.toml + lib.rs + Error + SsoClaims + SsoJwtCodec） | ~180 行 | 60 min | T0 |
| T2 | RefreshTokenStore trait + Memory/Cache 实现 | ~120 行 | 30 min | T1 |
| T3 | RefreshTokenVerifier | ~100 行 | 30 min | T1, T2 |
| T4 | RefreshTokenIssuer（issue/rotate/revoke） | ~180 行 | 60 min | T1, T2, T3 |
| T5 | RefreshTokenRevoker | ~60 行 | 20 min | T1, T2, T4 |
| T6 | SsoService 核心逻辑 + trait 抽象 | ~150 行 | 40 min | T4, T5 |
| T7 | SsoCenter axum HTTP 端点（feature=axum） | ~200 行 | 50 min | T6 |
| T8 | sso_middleware（本地验签 + 远程校验 feature） | ~280 行 | 70 min | T6 |
| T9 | sz300 集成（替换空实现 + 初始化） | ~30 行 | 20 min | T7, T8 |
| T10 | 单元测试 + 边界测试 | ~400 行 | 90 min | T1-T8 |
| T11 | 集成测试 + 基准测试 | ~280 行 | 60 min | T10 |
| T12 | 文档 + CHANGELOG + 版本 bump + semver-checks + crates.io | ~50 行 | 30 min | T11 |

**总计**：~2040 行新增代码，~10.5 小时预估耗时。

---

## 2. 任务详细规划

### T0 · workspace 依赖准备

**目标**：在 workspace 根 `Cargo.toml` 的 `[workspace.dependencies]` 新增 `hmac` 和 `subtle`（RustCrypto audited crate，与 sz-orm-auth 使用相同的密码学实现）。

**输入**：design.md §8.3
**输出**：`Cargo.toml`（workspace 根）新增 2 行
**验证**：`cargo check --workspace` 通过

**步骤**：

1. 编辑 `E:\vue\test\鲜视达\rust\sz-rust\Cargo.toml`，在 `[workspace.dependencies]` 段（约 Line 55 附近）新增：

```toml
# 【新增】RustCrypto audited — HS256 签名（SsoJwtCodec）
hmac = "0.12"
# 【新增】RustCrypto audited — 常量时间比较（防时序攻击，对齐 sz-orm-auth jwt.rs:148）
subtle = "2"
```

2. 验证：

```powershell
cargo check --workspace
```

**风险**：无（仅新增 workspace 依赖声明，未被任何 crate 引用前不影响构建）。
**回滚**：删除新增的 2 行。

---

### T1 · 基础设施（Cargo.toml + lib.rs + RefreshTokenError + SsoClaims + SsoJwtCodec）

**目标**：搭建 `sz-rust-auth-facade/src/refresh.rs` 基础设施：错误类型、SsoClaims 结构体、SsoJwtCodec 编解码器。

**输入**：design.md §2.1（SsoClaims + SsoJwtCodec）、§4.1（RefreshTokenError）、§8.1（Cargo.toml）
**输出**：
- `packages/sz-rust-auth-facade/Cargo.toml`（修改）
- `packages/sz-rust-auth-facade/src/lib.rs`（修改，+2 行）
- `packages/sz-rust-auth-facade/src/refresh.rs`（新增，~180 行）

**验证**：
- `cargo check -p sz-rust-auth-facade` 通过
- `cargo test -p sz-rust-auth-facade --lib refresh` 通过（T1 自带的 codec 单元测试）
- `cargo clippy -p sz-rust-auth-facade -- -D warnings` 零警告
- `cargo doc -p sz-rust-auth-facade --no-deps` 零警告

**依赖**：T0

**步骤**：

#### 步骤 1.1：修改 `sz-rust-auth-facade/Cargo.toml`

在 `[dependencies]` 段新增（对齐 design.md §8.1）：

```toml
[dependencies]
# 现有
parking_lot = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
hex = { workspace = true }
sha1 = { workspace = true }
thiserror = { workspace = true }
redis = { workspace = true, optional = true }
futures = { workspace = true, optional = true }

# 【新增】SsoJwtCodec 依赖（HS256 签名，复用 RustCrypto audited crate）
hmac = { workspace = true }
sha2 = { workspace = true }
base64 = { workspace = true }
subtle = { workspace = true }
uuid = { workspace = true }
async-trait = { workspace = true }
tracing = { workspace = true }
chrono = { workspace = true }

# 【新增】axum 集成（可选，feature = "axum"）
axum = { workspace = true, optional = true }

# 【新增】远程校验（可选，feature = "remote-validate"）
reqwest = { workspace = true, optional = true }

# 【新增】复用现有中间件（JwtBlacklist）与 Cache
sz-rust-middleware-facade = { workspace = true }
sz-rust-cache-facade = { workspace = true }
sz-rust-orm-facade = { workspace = true }
```

在 `[features]` 段新增：

```toml
[features]
redis-gateway = ["redis", "futures"]
# 【新增】axum 集成（SsoCenter HTTP 端点）
axum = ["dep:axum"]
# 【新增】远程校验
remote-validate = ["dep:reqwest"]
```

#### 步骤 1.2：修改 `sz-rust-auth-facade/src/lib.rs`

在 Line 32（`pub mod wechat;` 之后）新增：

```rust
pub mod gateway;
pub mod oauth;
pub mod wechat;

/// Refresh Token 双 Token 机制（SsoJwtCodec + Issuer + Verifier + Revoker + Store）
pub mod refresh;

/// SSO 认证中心（SsoService + axum 路由，需启用 `axum` feature）
pub mod sso;
```

> **注意**：`pub mod sso;` 在 T6/T7 才填充内容，T1 阶段先创建空文件 `src/sso.rs`（仅模块文档注释），避免编译错误。

#### 步骤 1.3：创建 `sz-rust-auth-facade/src/sso.rs`（占位）

```rust
//! SSO 认证中心
//!
//! T6/T7 阶段填充内容。当前为占位模块。
```

#### 步骤 1.4：创建 `sz-rust-auth-facade/src/refresh.rs`（T1 部分：Error + SsoClaims + SsoJwtCodec）

按 design.md §2.1 + §4.1 实现。关键代码骨架：

```rust
//! Refresh Token 双 Token 机制
//!
//! 对齐 spec.md FR-1 ~ FR-4，design.md §2.1 ~ §2.6。
//!
//! ## 核心组件
//!
//! - [`SsoJwtCodec`]：JWT HS256 编解码（支持 token_type/jti/ver 自定义 claim）
//! - [`SsoClaims`]：JWT claims 结构体（JwtClaims 超集）
//! - [`RefreshTokenIssuer`]：签发 + 轮换
//! - [`RefreshTokenVerifier`]：校验
//! - [`RefreshTokenRevoker`]：撤销
//! - [`RefreshTokenStore`]：存储抽象 trait

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

// ── 错误类型（design.md §4.1） ──

/// Refresh Token 错误类型
#[derive(Debug, thiserror::Error)]
pub enum RefreshTokenError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("token expired")]
    Expired,
    #[error("wrong token type: expected {expected}, got {actual}")]
    WrongTokenType { expected: String, actual: String },
    #[error("token revoked")]
    Revoked,
    #[error("issuer mismatch: expected {expected}, got {actual}")]
    IssuerMismatch { expected: String, actual: String },
    #[error("token version mismatch: token ver={token_ver}, current ver={current_ver}")]
    VersionMismatch { token_ver: u64, current_ver: u64 },
    #[error("refresh token reuse detected, all tokens for user revoked")]
    ReuseDetected,
    #[error("service unavailable")]
    ServiceUnavailable,
    #[error("cache error: {0}")]
    Cache(String),
    #[error("jwt error: {0}")]
    Jwt(String),
    #[error("user not found")]
    UserNotFound,
}

// ── SsoClaims（design.md §2.1） ──

fn default_token_type() -> String {
    "access".to_string()
}

/// SSO JWT claims — JwtClaims 的超集，新增 token_type / jti / ver
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SsoClaims {
    pub sub: String,
    pub exp: i64,
    pub iat: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    #[serde(default = "default_token_type")]
    pub token_type: String,
    #[serde(default)]
    pub jti: String,
    #[serde(default)]
    pub ver: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
}

impl SsoClaims {
    pub fn access(user_id: i64, exp: i64, issuer: &str, ver: u64) -> Self { /* ... */ }
    pub fn refresh(user_id: i64, exp: i64, issuer: &str, ver: u64, jti: String) -> Self { /* ... */ }
    pub fn is_expired(&self) -> bool { chrono::Utc::now().timestamp() >= self.exp }
    pub fn is_access(&self) -> bool { self.token_type == "access" }
    pub fn is_refresh(&self) -> bool { self.token_type == "refresh" }
}

// ── SsoJwtCodec（design.md §2.1） ──

type HmacSha256 = Hmac<Sha256>;

/// SSO JWT 编解码器 — HS256 签名/验签
pub struct SsoJwtCodec {
    secret: String,
}

impl SsoJwtCodec {
    pub fn new(secret: impl Into<String>) -> Self {
        Self { secret: secret.into() }
    }

    pub fn encode(&self, claims: &SsoClaims) -> Result<String, RefreshTokenError> {
        // header = {"alg":"HS256","typ":"JWT"}（base64url no-pad）
        // payload = serde_json::to_string(claims) → base64url no-pad
        // signature = HMAC-SHA256(secret, header.payload) → base64url no-pad
        // 返回 "header.payload.signature"
    }

    pub fn decode(&self, token: &str) -> Result<SsoClaims, RefreshTokenError> {
        // 1. split token by '.' → 3 parts（不足 3 部分 → InvalidSignature）
        // 2. base64url decode header → 校验 alg == "HS256"（拒绝 none/RS256 等）
        // 3. base64url decode payload → serde_json::from_str → SsoClaims
        // 4. 重算签名 → subtle::ConstantTimeEq 比较（防时序攻击）
        // 5. 校验 exp（is_expired → Expired）
    }
}

impl std::fmt::Debug for SsoJwtCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SsoJwtCodec")
            .field("secret", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
```

#### 步骤 1.5：T1 自带单元测试（嵌入 `refresh.rs` 尾部 `#[cfg(test)] mod tests`）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sso_jwt_codec_encode_decode_roundtrip() {
        let codec = SsoJwtCodec::new("test-secret");
        let claims = SsoClaims::access(1, chrono::Utc::now().timestamp() + 900, "iss", 0);
        let token = codec.encode(&claims).unwrap();
        let decoded = codec.decode(&token).unwrap();
        assert_eq!(decoded, claims);
    }

    #[test]
    fn test_sso_jwt_codec_rejects_wrong_secret() { /* ... */ }

    #[test]
    fn test_sso_jwt_codec_rejects_expired() { /* ... */ }

    #[test]
    fn test_sso_jwt_codec_debug_redacts_secret() {
        let codec = SsoJwtCodec::new("super-secret");
        let debug_str = format!("{:?}", codec);
        assert!(!debug_str.contains("super-secret"));
        assert!(debug_str.contains("[REDACTED]"));
    }

    #[test]
    fn test_sso_claims_access_vs_refresh() { /* ... */ }
    #[test]
    fn test_sso_claims_default_token_type() { /* ... */ }
}
```

#### 步骤 1.6：验证

```powershell
$env:CARGO_INCREMENTAL=0
cargo check -p sz-rust-auth-facade
cargo test -p sz-rust-auth-facade --lib refresh
cargo clippy -p sz-rust-auth-facade -- -D warnings
cargo doc -p sz-rust-auth-facade --no-deps
```

**风险点**：
1. **base64 padding**：必须用 `URL_SAFE_NO_PAD`（对齐 sz-orm-auth jwt.rs:218），否则 decode 旧 token 失败。
2. **常量时间比较**：签名比较必须用 `subtle::ConstantTimeEq`，禁止用 `==`（防时序攻击）。
3. **unsafe_code = forbid**：hmac/sha2/subtle 内部可能使用 unsafe，但 workspace lint 仅约束本 crate 代码，依赖 crate 的 unsafe 不受影响（验证：`cargo build` 不报 forbid）。

**回滚**：删除 `refresh.rs`，还原 `lib.rs` 和 `Cargo.toml`。

---

### T2 · RefreshTokenStore trait + Memory/Cache 实现

**目标**：实现存储抽象 trait 及两个默认实现（内存 + Cache）。

**输入**：design.md §2.3、§5.1-§5.3
**输出**：`refresh.rs` 追加 ~120 行
**验证**：`cargo test -p sz-rust-auth-facade --lib refresh::store` 通过
**依赖**：T1

**步骤**：

#### 步骤 2.1：在 `refresh.rs` 追加 RefreshTokenStore trait + 实现

```rust
use std::collections::HashMap;
use std::sync::Arc;

/// Refresh Token 存储抽象（design.md §2.3）
///
/// 职责：维护 `user_id → token_version`（用于用户级撤销，O(1) 撤销所有）
#[async_trait::async_trait]
pub trait RefreshTokenStore: Send + Sync {
    async fn get_version(&self, user_id: i64) -> Result<u64, RefreshTokenError>;
    async fn increment_version(&self, user_id: i64) -> Result<u64, RefreshTokenError>;
}

/// 内存实现（单进程，测试用）
pub struct MemoryRefreshTokenStore {
    inner: Arc<parking_lot::RwLock<HashMap<i64, u64>>>,
}

impl MemoryRefreshTokenStore {
    pub fn new() -> Self {
        Self { inner: Arc::new(parking_lot::RwLock::new(HashMap::new())) }
    }
}

impl Default for MemoryRefreshTokenStore {
    fn default() -> Self { Self::new() }
}

#[async_trait::async_trait]
impl RefreshTokenStore for MemoryRefreshTokenStore {
    async fn get_version(&self, user_id: i64) -> Result<u64, RefreshTokenError> {
        Ok(self.inner.read().get(&user_id).copied().unwrap_or(0))
    }
    async fn increment_version(&self, user_id: i64) -> Result<u64, RefreshTokenError> {
        let mut guard = self.inner.write();
        let new_ver = guard.entry(user_id).and_modify(|v| *v += 1).or_insert(1);
        Ok(*new_ver)
    }
}

/// Cache 实现（多进程共享，基于 sz_rust_cache_facade::Cache）
pub struct CacheRefreshTokenStore {
    cache: Arc<sz_rust_cache_facade::Cache>,
    key_prefix: String,
}

impl CacheRefreshTokenStore {
    pub fn new(cache: Arc<sz_rust_cache_facade::Cache>, key_prefix: impl Into<String>) -> Self {
        Self { cache, key_prefix: key_prefix.into() }
    }
    fn make_key(&self, user_id: i64) -> String {
        format!("{}:{}", self.key_prefix, user_id)
    }
}
```

> **注意 CacheRefreshTokenStore 实现细节**：需先确认 `sz_rust_cache_facade::Cache` 的 `get`/`set` 签名与 `Value` 类型。design.md §5.3 假设 `Cache::get` 返回 `Option<Value>`，但需实际读取 `sz-rust-cache-facade/src/lib.rs` 确认。**若 Cache API 与 design 假设不符，在此步骤修正实现，并在 tasks.md 备注**。

#### 步骤 2.2：单元测试

```rust
#[tokio::test]
async fn test_memory_store_get_version_default() {
    let store = MemoryRefreshTokenStore::new();
    assert_eq!(store.get_version(1).await.unwrap(), 0);
}

#[tokio::test]
async fn test_memory_store_increment() {
    let store = MemoryRefreshTokenStore::new();
    assert_eq!(store.increment_version(1).await.unwrap(), 1);
    assert_eq!(store.increment_version(1).await.unwrap(), 2);
    assert_eq!(store.get_version(1).await.unwrap(), 2);
}

#[tokio::test]
async fn test_cache_store_get_set() { /* ... */ }
```

#### 步骤 2.3：验证

```powershell
cargo test -p sz-rust-auth-facade --lib refresh::store
```

**风险**：Cache API 与 design 假设不符 → 步骤 2.1 修正。
**回滚**：删除 T2 追加代码。

---

### T3 · RefreshTokenVerifier

**目标**：实现 refresh token + access token 校验器，含完整校验链（FR-2.1）。

**输入**：design.md §2.5
**输出**：`refresh.rs` 追加 ~100 行
**验证**：`cargo test -p sz-rust-auth-facade --lib refresh::verifier` 通过
**依赖**：T1, T2

**步骤**：

#### 步骤 3.1：在 `refresh.rs` 追加 RefreshTokenVerifier

```rust
use sz_rust_middleware_facade::jwt_blacklist::JwtBlacklist;

/// Refresh Token 校验器（design.md §2.5）
pub struct RefreshTokenVerifier {
    codec: SsoJwtCodec,
    blacklist: JwtBlacklist,
    store: Arc<dyn RefreshTokenStore>,
    issuer: String,
}

impl RefreshTokenVerifier {
    pub fn new(
        codec: SsoJwtCodec,
        blacklist: JwtBlacklist,
        store: Arc<dyn RefreshTokenStore>,
        issuer: impl Into<String>,
    ) -> Self {
        Self { codec, blacklist, store, issuer: issuer.into() }
    }

    /// 校验 refresh token（FR-2.1 校验链）
    #[tracing::instrument(skip(self, token))]
    pub async fn verify(&self, token: &str) -> Result<SsoClaims, RefreshTokenError> {
        self.verify_inner(token, "refresh").await
    }

    /// 校验 access token（用于 sso_middleware 本地验签）
    #[tracing::instrument(skip(self, token))]
    pub async fn verify_access(&self, token: &str) -> Result<SsoClaims, RefreshTokenError> {
        self.verify_inner(token, "access").await
    }

    async fn verify_inner(&self, token: &str, expected_type: &str) -> Result<SsoClaims, RefreshTokenError> {
        // (a) JWT 签名 + 过期 → codec.decode
        let claims = self.codec.decode(token)?;
        // (c) token_type 校验
        if claims.token_type != expected_type {
            return Err(RefreshTokenError::WrongTokenType {
                expected: expected_type.to_string(),
                actual: claims.token_type.clone(),
            });
        }
        // (e) 签发人校验
        if claims.iss.as_deref() != Some(self.issuer.as_str()) {
            return Err(RefreshTokenError::IssuerMismatch {
                expected: self.issuer.clone(),
                actual: claims.iss.clone().unwrap_or_default(),
            });
        }
        // (d) 黑名单校验（fail-closed）
        match self.blacklist.is_revoked(token) {
            Ok(true) => return Err(RefreshTokenError::Revoked),
            Ok(false) => {}
            Err(e) => {
                tracing::error!(error = %e, "blacklist check failed, fail-closed");
                return Err(RefreshTokenError::Cache(e.to_string()));
            }
        }
        // (f) 版本号校验
        if let Some(uid) = claims.user_id {
            let current_ver = self.store.get_version(uid).await?;
            if claims.ver != current_ver {
                return Err(RefreshTokenError::VersionMismatch {
                    token_ver: claims.ver,
                    current_ver,
                });
            }
        }
        Ok(claims)
    }
}
```

> **注意**：`JwtBlacklist::is_revoked` 是同步方法（jwt_blacklist.rs:128），返回 `Result<bool, JwtBlacklistError>`。需将 `JwtBlacklistError` 转为 `RefreshTokenError::Cache`。`JwtBlacklistError` 目前只有 `Cache(String)` 变体，转换直接 `RefreshTokenError::Cache(e.to_string())`。

#### 步骤 3.2：单元测试

```rust
#[tokio::test]
async fn test_verifier_rejects_access_as_refresh() { /* AC-1.3 */ }
#[tokio::test]
async fn test_verifier_rejects_refresh_as_access() { /* AC-1.3 */ }
#[tokio::test]
async fn test_verifier_rejects_wrong_issuer() { /* FR-2.1(e) */ }
#[tokio::test]
async fn test_verifier_rejects_revoked() { /* FR-2.1(d) */ }
#[tokio::test]
async fn test_verifier_rejects_version_mismatch() { /* 版本号不匹配 */ }
```

#### 步骤 3.3：验证

```powershell
cargo test -p sz-rust-auth-facade --lib refresh::verifier
cargo clippy -p sz-rust-auth-facade -- -D warnings
```

**风险**：`JwtBlacklist::is_revoked` 同步调用在 async fn 中阻塞 — 实际上 `is_revoked` 仅查内存 Cache（`parking_lot::Mutex`），不涉及 IO，阻塞可忽略。若未来 Cache 切换 Redis 驱动需重新评估。
**回滚**：删除 T3 追加代码。

---

### T4 · RefreshTokenIssuer（issue/rotate/revoke）

**目标**：实现签发器 + 轮换器，含复用攻击检测（NFR-2.5）。

**输入**：design.md §2.2（TokenPair + Config）、§2.4（Issuer）
**输出**：`refresh.rs` 追加 ~180 行
**验证**：`cargo test -p sz-rust-auth-facade --lib refresh::issuer` 通过
**依赖**：T1, T2, T3

**步骤**：

#### 步骤 4.1：在 `refresh.rs` 追加 TokenPair + RefreshTokenConfig

```rust
/// 双 Token 返回值（design.md §2.2）
#[derive(Debug, Clone, serde::Serialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: i64,
    pub refresh_expires_at: i64,
    pub ver: u64,
}

/// Refresh Token 配置（design.md §2.2）
#[derive(Debug, Clone)]
pub struct RefreshTokenConfig {
    pub access_token_ttl: std::time::Duration,
    pub refresh_token_ttl: std::time::Duration,
    pub issuer: String,
}

impl Default for RefreshTokenConfig {
    fn default() -> Self {
        Self {
            access_token_ttl: std::time::Duration::from_secs(900),
            refresh_token_ttl: std::time::Duration::from_secs(604_800),
            issuer: "sz-rust-sso".to_string(),
        }
    }
}
```

#### 步骤 4.2：在 `refresh.rs` 追加 RefreshTokenIssuer

```rust
/// Refresh Token 签发器 + 轮换器（design.md §2.4）
pub struct RefreshTokenIssuer {
    codec: SsoJwtCodec,
    blacklist: JwtBlacklist,
    store: Arc<dyn RefreshTokenStore>,
    config: RefreshTokenConfig,
}

impl RefreshTokenIssuer {
    pub fn new(
        codec: SsoJwtCodec,
        blacklist: JwtBlacklist,
        store: Arc<dyn RefreshTokenStore>,
        config: RefreshTokenConfig,
    ) -> Self {
        Self { codec, blacklist, store, config }
    }

    /// 签发双 Token（FR-1.1）
    #[tracing::instrument(skip(self, username), fields(user_id = user_id))]
    pub async fn issue(&self, user_id: i64, username: &str) -> Result<TokenPair, RefreshTokenError> {
        let now = chrono::Utc::now().timestamp();
        let ver = self.store.get_version(user_id).await?;
        let access_exp = now + self.config.access_token_ttl.as_secs() as i64;
        let refresh_exp = now + self.config.refresh_token_ttl.as_secs() as i64;
        let access_claims = SsoClaims::access(user_id, access_exp, &self.config.issuer, ver);
        let refresh_jti = uuid::Uuid::new_v4().to_string();
        let refresh_claims = SsoClaims::refresh(user_id, refresh_exp, &self.config.issuer, ver, refresh_jti);
        let access_token = self.codec.encode(&access_claims)?;
        let refresh_token = self.codec.encode(&refresh_claims)?;
        Ok(TokenPair {
            access_token, refresh_token,
            access_expires_at: access_exp,
            refresh_expires_at: refresh_exp,
            ver,
        })
    }

    /// 轮换 Token（FR-3.1 + NFR-2.5 复用攻击检测）
    #[tracing::instrument(skip(self, refresh_token))]
    pub async fn rotate(&self, old_refresh_token: &str) -> Result<TokenPair, RefreshTokenError> {
        let verifier = RefreshTokenVerifier::new(
            self.codec.clone(), // 注意：SsoJwtCodec 需实现 Clone
            self.blacklist.clone(),
            self.store.clone(),
            self.config.issuer.clone(),
        );
        // 1. 验证旧 refresh token
        let claims = match verifier.verify(old_refresh_token).await {
            Ok(c) => c,
            Err(RefreshTokenError::Revoked) => {
                // ★ 复用攻击检测（NFR-2.5）★
                if let Some(uid) = claims_user_id_from_token(&self.codec, old_refresh_token) {
                    let _ = self.store.increment_version(uid).await?;
                    tracing::warn!(user_id = uid, "refresh token reuse detected, all tokens revoked");
                }
                return Err(RefreshTokenError::ReuseDetected);
            }
            Err(e) => return Err(e),
        };
        let user_id = claims.user_id.ok_or(RefreshTokenError::InvalidCredentials)?;
        // 2. 将旧 refresh token 加入黑名单（FR-3.2，TTL = 剩余有效期）
        let remaining_ttl = std::time::Duration::from_secs(
            (claims.exp - chrono::Utc::now().timestamp()).max(0) as u64
        );
        self.blacklist.revoke(old_refresh_token, Some(remaining_ttl))
            .map_err(|e| RefreshTokenError::Cache(e.to_string()))?;
        // 3. 签发新双 Token（ver 不变）
        self.issue(user_id, &claims.sub).await
    }

    /// 撤销单个 refresh token（FR-4.1，幂等 FR-4.3）
    #[tracing::instrument(skip(self, refresh_token))]
    pub async fn revoke(&self, refresh_token: &str) -> Result<(), RefreshTokenError> {
        match self.codec.decode(refresh_token) {
            Ok(claims) => {
                if claims.is_expired() {
                    return Ok(()); // 已过期，幂等 no-op（FR-4.3）
                }
                let remaining_ttl = std::time::Duration::from_secs(
                    (claims.exp - chrono::Utc::now().timestamp()).max(0) as u64
                );
                self.blacklist.revoke(refresh_token, Some(remaining_ttl))
                    .map_err(|e| RefreshTokenError::Cache(e.to_string()))?;
                Ok(())
            }
            Err(RefreshTokenError::Expired) => Ok(()), // 已过期，幂等
            Err(e) => Err(e),
        }
    }

    /// 撤销用户所有 token（increment_version）
    #[tracing::instrument(skip(self))]
    pub async fn revoke_all_for_user(&self, user_id: i64) -> Result<(), RefreshTokenError> {
        self.store.increment_version(user_id).await?;
        Ok(())
    }
}
```

> **关键修正点**：
> 1. `SsoJwtCodec` 需实现 `Clone`（`secret: String` 已是 Clone）— 在 T1 步骤 1.4 补 `#[derive(Clone)]` 或手动 impl。
> 2. 复用攻击检测中，`claims_user_id_from_token` 是辅助函数：当 `verify` 返回 `Revoked` 时，需重新 decode token 获取 user_id（因为 verify 在黑名单检查阶段就返回了，claims 已丢失）。实现：`fn claims_user_id_from_token(codec, token) -> Option<i64> { codec.decode(token).ok().and_then(|c| c.user_id) }`。
> 3. **事务性顺序**（NFR-4.3）：rotate 采用「先验证 → 先黑名单 → 后签发」顺序，固定不变。

#### 步骤 4.3：单元测试

```rust
#[tokio::test]
async fn test_issuer_issue_returns_pair() { /* FR-1.1 */ }
#[tokio::test]
async fn test_issuer_issue_access_ttl() { /* AC-1.1: exp - iat ≈ 900 */ }
#[tokio::test]
async fn test_issuer_issue_refresh_ttl() { /* AC-1.1: exp - iat ≈ 604800 */ }
#[tokio::test]
async fn test_issuer_rotate_returns_new_pair() { /* FR-3.1 */ }
#[tokio::test]
async fn test_issuer_rotate_revokes_old() { /* AC-1.2: 旧 token 再用 → Revoked */ }
#[tokio::test]
async fn test_issuer_rotate_reuse_detection() { /* NFR-2.5: 复用 → ReuseDetected + 撤销所有 */ }
#[tokio::test]
async fn test_issuer_revoke_idempotent() { /* FR-4.3 */ }
#[tokio::test]
async fn test_issuer_revoke_expired_is_noop() { /* FR-4.3 */ }
```

#### 步骤 4.4：验证

```powershell
cargo test -p sz-rust-auth-facade --lib refresh::issuer
cargo clippy -p sz-rust-auth-facade -- -D warnings
```

**风险**：
1. **复用攻击检测的 claims 丢失**：verify 在黑名单阶段返回 `Revoked` 时，claims 未返回。需用辅助函数重新 decode。**已在步骤 4.2 修正**。
2. **SsoJwtCodec Clone**：需在 T1 补 Clone 实现。**已标注**。
3. **并发 rotate**（NFR-4.3）：多个并发 rotate 同一 token，可能都通过 verify 但只有一个能成功 revoke（JwtBlacklist::revoke 返回 `Ok(false)` 表示已存在）。当前实现：第一个 rotate 成功，后续 rotate 在 verify 阶段命中黑名单 → ReuseDetected。这是安全行为（并发轮换视为复用攻击）。

**回滚**：删除 T4 追加代码。

---

### T5 · RefreshTokenRevoker

**目标**：实现撤销器（复用 JwtBlacklist + Store）。

**输入**：design.md §2.6
**输出**：`refresh.rs` 追加 ~60 行
**验证**：`cargo test -p sz-rust-auth-facade --lib refresh::revoker` 通过
**依赖**：T1, T2, T4

**步骤**：

#### 步骤 5.1：在 `refresh.rs` 追加 RefreshTokenRevoker

```rust
/// Refresh Token 撤销器（design.md §2.6）
///
/// 复用 JwtBlacklist（FR-4.2），不新建独立黑名单存储
pub struct RefreshTokenRevoker {
    blacklist: JwtBlacklist,
    store: Arc<dyn RefreshTokenStore>,
    codec: SsoJwtCodec,
}

impl RefreshTokenRevoker {
    pub fn new(
        blacklist: JwtBlacklist,
        store: Arc<dyn RefreshTokenStore>,
        codec: SsoJwtCodec,
    ) -> Self {
        Self { blacklist, store, codec }
    }

    /// 撤销单个 token（加入黑名单，幂等 FR-4.3）
    #[tracing::instrument(skip(self, token))]
    pub async fn revoke(&self, token: &str) -> Result<(), RefreshTokenError> {
        match self.codec.decode(token) {
            Ok(claims) => {
                if claims.is_expired() {
                    return Ok(());
                }
                let remaining_ttl = std::time::Duration::from_secs(
                    (claims.exp - chrono::Utc::now().timestamp()).max(0) as u64
                );
                self.blacklist.revoke(token, Some(remaining_ttl))
                    .map_err(|e| RefreshTokenError::Cache(e.to_string()))?;
                Ok(())
            }
            Err(RefreshTokenError::Expired) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// 撤销用户所有 token（increment_version）
    #[tracing::instrument(skip(self))]
    pub async fn revoke_all_for_user(&self, user_id: i64) -> Result<(), RefreshTokenError> {
        self.store.increment_version(user_id).await?;
        Ok(())
    }
}
```

#### 步骤 5.2：单元测试 + 验证

```powershell
cargo test -p sz-rust-auth-facade --lib refresh::revoker
```

**风险**：无（逻辑与 T4::revoke 一致，独立结构体便于 SsoService 组合）。
**回滚**：删除 T5 追加代码。

---

### T6 · SsoService 核心逻辑 + trait 抽象

**目标**：实现 SsoService（不依赖 axum）+ UserAuthenticator/UserInfoProvider trait。

**输入**：design.md §2.7（SsoService + trait）
**输出**：`sso.rs` 追加 ~150 行（替换 T1 占位内容）
**验证**：`cargo test -p sz-rust-auth-facade --lib sso` 通过
**依赖**：T4, T5

**步骤**：

#### 步骤 6.1：重写 `sso.rs`

按 design.md §2.7 实现 `UserAuthenticator` / `UserInfoProvider` trait、`UserInfo` / `SsoLoginRequest` / `SsoLoginResponse` / `SsoValidateResponse` 结构体、`SsoService` 结构体及 `login` / `refresh` / `revoke` / `validate` / `me` 方法。

关键骨架：

```rust
use crate::refresh::*;
use std::sync::Arc;

/// 用户认证抽象（由业务层实现）
#[async_trait::async_trait]
pub trait UserAuthenticator: Send + Sync {
    async fn authenticate(&self, username: &str, password: &str) -> Result<(i64, String), RefreshTokenError>;
}

/// 用户信息查询抽象
#[async_trait::async_trait]
pub trait UserInfoProvider: Send + Sync {
    async fn get_user_info(&self, user_id: i64) -> Result<UserInfo, RefreshTokenError>;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UserInfo { /* ... */ }

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SsoLoginRequest { pub username: String, pub password: String }

#[derive(Debug, Clone, serde::Serialize)]
pub struct SsoLoginResponse { /* ... */ }

#[derive(Debug, Clone, serde::Serialize)]
pub struct SsoValidateResponse { pub valid: bool, pub user_id: i64, pub expires_at: i64 }

/// SSO 认证中心核心逻辑（不依赖 axum）
pub struct SsoService {
    issuer: RefreshTokenIssuer,
    verifier: RefreshTokenVerifier,
    revoker: RefreshTokenRevoker,
    authenticator: Arc<dyn UserAuthenticator>,
    user_info_provider: Arc<dyn UserInfoProvider>,
}

impl SsoService {
    pub fn new(/* ... */) -> Self { /* ... */ }
    pub async fn login(&self, req: SsoLoginRequest) -> Result<SsoLoginResponse, RefreshTokenError> { /* ... */ }
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, RefreshTokenError> { /* ... */ }
    pub async fn revoke(&self, token: &str) -> Result<(), RefreshTokenError> { /* ... */ }
    pub async fn validate(&self, access_token: &str) -> Result<SsoValidateResponse, RefreshTokenError> { /* ... */ }
    pub async fn me(&self, access_token: &str) -> Result<UserInfo, RefreshTokenError> { /* ... */ }
}
```

#### 步骤 6.2：单元测试（用 mock UserAuthenticator/UserInfoProvider）

```rust
#[tokio::test]
async fn test_sso_service_login_success() { /* FR-5.2 */ }
#[tokio::test]
async fn test_sso_service_login_empty_credentials() { /* FR-1.4 */ }
#[tokio::test]
async fn test_sso_service_refresh_success() { /* FR-5.3 */ }
#[tokio::test]
async fn test_sso_service_revoke_success() { /* AC-1.4 */ }
#[tokio::test]
async fn test_sso_service_validate_valid() { /* FR-5.5 */ }
#[tokio::test]
async fn test_sso_service_me_returns_user_info() { /* ... */ }
```

#### 步骤 6.3：验证

```powershell
cargo test -p sz-rust-auth-facade --lib sso
cargo clippy -p sz-rust-auth-facade -- -D warnings
```

**风险**：mock trait 实现需注意 `Send + Sync` bound。
**回滚**：还原 `sso.rs` 为 T1 占位内容。

---

### T7 · SsoCenter axum HTTP 端点（feature=axum）

**目标**：在 `sso.rs` 的 `#[cfg(feature = "axum")]` 下实现 axum Router + 5 个 handler。

**输入**：design.md §2.7（axum_routes）、§4.2-§4.3（错误映射）
**输出**：`sso.rs` 追加 ~200 行
**验证**：`cargo test -p sz-rust-auth-facade --features axum --lib sso::axum_routes` 通过
**依赖**：T6

**步骤**：

#### 步骤 7.1：在 `sso.rs` 追加 axum 集成

```rust
#[cfg(feature = "axum")]
pub mod axum_routes {
    use super::*;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::response::Response;
    use axum::Json;
    use std::sync::Arc;

    /// SSO 认证中心 axum Router
    pub fn sso_router(state: Arc<SsoService>) -> axum::Router {
        axum::Router::new()
            .route("/sso/login", axum::routing::post(login_handler))
            .route("/sso/refresh", axum::routing::post(refresh_handler))
            .route("/sso/revoke", axum::routing::post(revoke_handler))
            .route("/sso/validate", axum::routing::get(validate_handler))
            .route("/sso/me", axum::routing::get(me_handler))
            .with_state(state)
    }

    fn apply_security_headers(resp: Response) -> Response {
        // 设置 Cache-Control: no-store, Pragma: no-cache（FR-5.6）
    }

    fn extract_bearer(headers: &HeaderMap) -> Option<String> {
        // 从 Authorization: Bearer <token> 提取（FR-5.7）
    }

    async fn login_handler(State(svc): State<Arc<SsoService>>, Json(req): Json<SsoLoginRequest>) -> Response { /* ... */ }
    async fn refresh_handler(State(svc): State<Arc<SsoService>>, headers: HeaderMap) -> Response { /* ... */ }
    async fn revoke_handler(State(svc): State<Arc<SsoService>>, headers: HeaderMap) -> Response { /* ... */ }
    async fn validate_handler(State(svc): State<Arc<SsoService>>, headers: HeaderMap) -> Response { /* ... */ }
    async fn me_handler(State(svc): State<Arc<SsoService>>, headers: HeaderMap) -> Response { /* ... */ }
}

/// 将 RefreshTokenError 转换为 HTTP 响应（design.md §4.3）
#[cfg(feature = "axum")]
pub fn refresh_error_to_response(err: RefreshTokenError) -> Response {
    // 按 design.md §4.2 表映射 (http_status, code, msg)
    // 设置安全头
}
```

#### 步骤 7.2：单元测试（用 axum::test + mock SsoService）

```powershell
cargo test -p sz-rust-auth-facade --features axum --lib sso::axum_routes
```

**风险**：axum 0.8 API 变化（`axum::extract::Request` vs `axum::http::Request`）— 需对齐 workspace axum 0.8。
**回滚**：删除 `#[cfg(feature = "axum")]` 段。

---

### T8 · sso_middleware（本地验签 + 远程校验 feature）

**目标**：在 `sz-rust-middleware-facade` 新增 `sso_middleware.rs`，实现本地验签 + 远程校验（feature gate）。

**输入**：design.md §2.8、§6.1-§6.3
**输出**：
- `packages/sz-rust-middleware-facade/Cargo.toml`（修改）
- `packages/sz-rust-middleware-facade/src/lib.rs`（修改，+1 行）
- `packages/sz-rust-middleware-facade/src/sso_middleware.rs`（新增，~280 行）

**验证**：`cargo test -p sz-rust-middleware-facade --lib sso_middleware` 通过
**依赖**：T6

**步骤**：

#### 步骤 8.1：修改 `sz-rust-middleware-facade/Cargo.toml`

```toml
[dependencies]
# 现有（不修改）
# ...

# 【新增】SSO 中间件依赖
sz-rust-auth-facade = { workspace = true }
uuid = { workspace = true }

# 【新增】远程校验（可选）
reqwest = { workspace = true, optional = true }

[features]
# 【新增】远程校验
remote-validate = ["dep:reqwest", "sz-rust-auth-facade/remote-validate"]
```

#### 步骤 8.2：修改 `sz-rust-middleware-facade/src/lib.rs`

在 Line 39（`pub mod trace;` 之前或之后）新增：

```rust
/// SSO 业务系统中间件（本地验签 / 远程校验）
pub mod sso_middleware;
```

#### 步骤 8.3：创建 `sso_middleware.rs`

按 design.md §2.8（SsoMiddlewareConfig + sso_middleware）、§6.1（本地验签流程）、§6.2（远程校验流程）、§6.3（白名单匹配）实现。

关键骨架：

```rust
use crate::auth::AuthenticatedUser;
use crate::jwt_blacklist::JwtBlacklist;
use sz_rust_auth_facade::refresh::{RefreshTokenStore, RefreshTokenError, SsoJwtCodec};
use std::sync::Arc;

/// SSO 中间件配置
pub enum SsoMiddlewareConfig {
    Local {
        secret: String,
        issuer: String,
        blacklist: JwtBlacklist,
        store: Arc<dyn RefreshTokenStore>,
        allow_all_action: Vec<String>,
    },
    #[cfg(feature = "remote-validate")]
    Remote {
        endpoint: String,
        timeout: std::time::Duration,
        cache: Arc<sz_rust_cache_facade::Cache>,
        cache_ttl: std::time::Duration,
        allow_all_action: Vec<String>,
        client: Arc<reqwest::Client>,
    },
}

// Debug 实现：secret 脱敏（NFR-2.6）

/// SSO 业务系统中间件
#[tracing::instrument(skip_all)]
pub async fn sso_middleware(
    axum::extract::State(config): axum::extract::State<SsoMiddlewareConfig>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    match &config {
        SsoMiddlewareConfig::Local { .. } => sso_middleware_local(&config, req, next).await,
        #[cfg(feature = "remote-validate")]
        SsoMiddlewareConfig::Remote { .. } => sso_middleware_remote(&config, req, next).await,
    }
}

async fn sso_middleware_local(config: &SsoMiddlewareConfig::Local, req: Request, next: Next) -> Response { /* design.md §6.1 */ }
#[cfg(feature = "remote-validate")]
async fn sso_middleware_remote(config: &SsoMiddlewareConfig::Remote, req: Request, next: Next) -> Response { /* design.md §6.2 */ }
fn is_route_allowed(route_uri: &str, allow_list: &[String]) -> bool { /* design.md §6.3 */ }
fn extract_bearer_token(req: &Request) -> Option<String> { /* ... */ }
```

> **注意**：需确认 `AuthenticatedUser` 的定义位置和字段。design.md 引用 `auth.rs:390`，需实际读取 `sz-rust-middleware-facade/src/auth.rs` 确认 `AuthenticatedUser` 结构体。

#### 步骤 8.4：单元测试

```rust
#[tokio::test]
async fn test_middleware_local_valid_access() { /* AC-1.5 */ }
#[tokio::test]
async fn test_middleware_local_missing_token() { /* FR-6.3 */ }
#[tokio::test]
async fn test_middleware_local_expired() { /* FR-6.3 */ }
#[tokio::test]
async fn test_middleware_local_refresh_rejected() { /* FR-6.5 */ }
#[tokio::test]
async fn test_middleware_local_whitelist() { /* AC-1.6 */ }
#[tokio::test]
async fn test_middleware_local_whitelist_wildcard() { /* FR-6.4 */ }
#[tokio::test]
async fn test_middleware_local_blacklist_fail_closed() { /* AC-2.7 */ }
#[tokio::test]
async fn test_middleware_local_version_mismatch() { /* ... */ }
#[tokio::test]
async fn test_middleware_local_wrong_issuer() { /* FR-6.3 */ }
```

远程校验测试（feature `remote-validate`）：

```rust
#[cfg(feature = "remote-validate")]
#[tokio::test]
async fn test_middleware_remote_valid() { /* AC-1.7 */ }
#[cfg(feature = "remote-validate")]
#[tokio::test]
async fn test_middleware_remote_cache_hit() { /* AC-1.7 */ }
#[cfg(feature = "remote-validate")]
#[tokio::test]
async fn test_middleware_remote_timeout() { /* FR-7.4 */ }
```

#### 步骤 8.5：验证

```powershell
cargo test -p sz-rust-middleware-facade --lib sso_middleware
cargo test -p sz-rust-middleware-facade --features remote-validate --lib sso_middleware
cargo clippy -p sz-rust-middleware-facade --all-features -- -D warnings
```

**风险**：
1. **AuthenticatedUser 字段**：需确认 `auth.rs` 中 `AuthenticatedUser` 的确切定义。
2. **axum 0.8 middleware 签名**：`axum::middleware::Next` 的 `run` 方法签名在 0.7 → 0.8 有变化。
3. **reqwest mock**：远程校验测试需 mock HTTP（用 `wiremock` 或 `httpmock`），确认 dev-dependencies 是否已有。

**回滚**：删除 `sso_middleware.rs`，还原 `lib.rs` 和 `Cargo.toml`。

---

### T9 · sz300 集成（替换空实现 + 初始化）

**目标**：替换 `sz-rust-sz300/src/controllers/auth.rs:172` 空实现，新增 SsoService 初始化。

**输入**：design.md §9.4、spec.md NFR-3.3
**输出**：
- `packages/sz-rust-sz300/src/controllers/auth.rs`（修改，~10 行）
- `packages/sz-rust-sz300/src/services/auth_service.rs`（修改，新增 SsoService 初始化）
- `packages/sz-rust-sz300/Cargo.toml`（可能新增 sz-rust-auth-facade axum feature 依赖）

**验证**：`cargo build -p sz-rust-sz300` 通过，`/auth/refresh` 端点返回真实 TokenPair
**依赖**：T7, T8

**步骤**：

#### 步骤 9.1：修改 `controllers/auth.rs:172-176`

**现状**：
```rust
pub async fn refresh(State(_state): State<AppState>, req: Request<Body>) -> Response {
    let _data = req;
    let ctrl = AuthController;
    ctrl.render_success("ok", json!({}))
}
```

**变更后**（对齐 design.md §9.4）：
```rust
pub async fn refresh(State(state): State<AppState>, req: Request<Body>) -> Response {
    let refresh_token = match extract_bearer_from_request(&req) {
        Some(t) => t,
        None => return AuthController.render_error("缺少必要的参数,请重新登陆!"),
    };
    match state.sso_service.refresh(&refresh_token).await {
        Ok(pair) => AuthController.render_success("ok", json!({
            "access_token": pair.access_token,
            "refresh_token": pair.refresh_token,
            "access_expires_at": pair.access_expires_at,
            "refresh_expires_at": pair.refresh_expires_at,
        })),
        Err(e) => refresh_error_to_response(e),
    }
}
```

> **注意**：需确认 `AppState` 是否有 `sso_service` 字段。若无，需在 `AppState` 定义处新增。`extract_bearer_from_request` 和 `refresh_error_to_response` 需确认是否已有或需新增辅助函数。

#### 步骤 9.2：在 `services/auth_service.rs` 新增 SsoService 初始化

```rust
pub fn init_sso_service(/* AppState 或必要依赖 */) -> Arc<SsoService> {
    let codec = SsoJwtCodec::new(/* secret from config */);
    let blacklist = JwtBlacklist::default_with_memory_cache();
    let store = Arc::new(MemoryRefreshTokenStore::new());
    let config = RefreshTokenConfig::default();
    let issuer = RefreshTokenIssuer::new(codec.clone(), blacklist.clone(), store.clone(), config);
    let verifier = RefreshTokenVerifier::new(codec.clone(), blacklist.clone(), store.clone(), "sz-rust-sso");
    let revoker = RefreshTokenRevoker::new(blacklist, store, codec);
    let authenticator = Arc::new(/* 现有 JwtAuthenticator 适配 */);
    let user_info_provider = Arc::new(/* 现有 UserService 适配 */);
    Arc::new(SsoService::new(issuer, verifier, revoker, authenticator, user_info_provider))
}
```

#### 步骤 9.3：验证

```powershell
cargo build -p sz-rust-sz300
cargo clippy -p sz-rust-sz300 -- -D warnings
```

**风险**：
1. **AppState 结构**：需确认 `AppState` 定义，可能需新增 `sso_service: Arc<SsoService>` 字段。
2. **UserAuthenticator 适配**：现有 `JwtAuthenticator` 可能不直接实现 `UserAuthenticator` trait，需新增适配器。
3. **HTTP 路由不变**（NFR-3.3）：仅替换函数体，路由注册不变。

**回滚**：还原 `auth.rs:172-176` 为空实现。

---

### T10 · 单元测试 + 边界测试

**目标**：补全所有单元测试 + 边界测试（AC-3.4），覆盖率 ≥ 90%。

**输入**：design.md §7.1-§7.5
**输出**：`refresh.rs` / `sso.rs` / `sso_middleware.rs` 的 `#[cfg(test)] mod tests` 追加 ~400 行
**验证**：`cargo test -p sz-rust-auth-facade --all-features` + `cargo test -p sz-rust-middleware-facade --all-features` 全通过
**依赖**：T1-T8

**步骤**：

#### 步骤 10.1：边界测试（design.md §7.5，AC-3.4）

```rust
#[test]
fn test_boundary_empty_token() { /* (a) 空 Token → InvalidSignature */ }
#[test]
fn test_boundary_tampered_token() { /* (b) 篡改 payload 1 字节 → InvalidSignature */ }
#[test]
fn test_boundary_expired_1s() { /* (c) 过期 1 秒 → Expired */ }
#[test]
fn test_boundary_missing_token_type() { /* (d) token_type 缺失 → 默认 "access" */ }
#[tokio::test]
async fn test_boundary_blacklist_timeout() { /* (e) 黑名单故障 → 503 fail-closed */ }
#[tokio::test]
async fn test_boundary_revoke_idempotent() { /* (f) 撤销后幂等再次撤销 → Ok */ }
#[tokio::test]
async fn test_boundary_concurrent_rotate() { /* (g) 并发 100 个轮换请求 */ }
```

#### 步骤 10.2：补全 design.md §7.1-§7.4 列出的所有测试

逐一对照 design.md §7.1（refresh.rs 22 个测试）、§7.2（sso.rs 8 个测试）、§7.3（sso_middleware.rs 10 个测试）、§7.4（远程校验 5 个测试），确保无遗漏。

#### 步骤 10.3：覆盖率验证

```powershell
cargo tarpaulin -p sz-rust-auth-facade --all-features --out html
# 检查 refresh.rs / sso.rs 覆盖率 ≥ 90%
cargo tarpaulin -p sz-rust-middleware-facade --all-features --out html
# 检查 sso_middleware.rs 覆盖率 ≥ 90%
```

> **注意**：`cargo-tarpaulin` 需单独安装（`cargo install cargo-tarpaulin`）。若环境未安装，跳过此步，改用 `cargo test` + 人工审查。

**风险**：tarpaulin 在 Windows 支持有限（主要支持 Linux）。若 Windows 不可用，可在 CI 或 WSL 中运行。
**回滚**：删除新增测试。

---

### T11 · 集成测试 + 基准测试

**目标**：端到端集成测试 + criterion 基准测试（NFR-1.1, NFR-1.2）。

**输入**：design.md §7.6-§7.7
**输出**：
- `packages/sz-rust-auth-facade/tests/sso_integration.rs`（新增，~200 行）
- `packages/sz-rust-auth-facade/benches/sso_bench.rs`（新增，~80 行）
- `packages/sz-rust-auth-facade/Cargo.toml`（新增 `[[bench]]` + dev-dependencies criterion）

**验证**：`cargo test -p sz-rust-auth-facade --test sso_integration` + `cargo bench -p sz-rust-auth-facade --bench sso_bench` 通过
**依赖**：T10

**步骤**：

#### 步骤 11.1：集成测试（design.md §7.6）

```rust
// tests/sso_integration.rs
#[tokio::test]
async fn test_full_sso_flow() {
    // 1. 启动 SsoCenter + 业务系统（sso_middleware）
    // 2. POST /sso/login → TokenPair
    // 3. GET /api/data with access_token → 200
    // 4. POST /sso/refresh → 新 TokenPair
    // 5. GET /api/data with new access_token → 200
    // 6. GET /api/data with old access_token → 401
    // 7. POST /sso/revoke → 200
    // 8. POST /sso/refresh with revoked → 401
}

#[tokio::test]
async fn test_reuse_attack_full_flow() {
    // 1. 登录 → TokenPair
    // 2. rotate(refresh) → TokenPair2
    // 3. rotate(old_refresh) → 401 ReuseDetected
    // 4. GET /api/data with access_token (from step 1) → 401
}
```

#### 步骤 11.2：基准测试（design.md §7.7）

```rust
// benches/sso_bench.rs
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_local_validate(c: &mut Criterion) { /* NFR-1.1: p99 < 1μs */ }
fn bench_rotate(c: &mut Criterion) { /* NFR-1.2: p99 < 50μs */ }

criterion_group!(benches, bench_local_validate, bench_rotate);
criterion_main!(benches);
```

在 `Cargo.toml` 新增：

```toml
[[bench]]
name = "sso_bench"
harness = false

[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
```

> **注意**：`criterion` 可能不在 workspace.dependencies，需新增或直接内联版本。

#### 步骤 11.3：验证

```powershell
cargo test -p sz-rust-auth-facade --features axum --test sso_integration
cargo bench -p sz-rust-auth-facade --bench sso_bench -- --quick
```

**风险**：集成测试需启动 axum server，端口冲突需用随机端口（`127.0.0.1:0`）。
**回滚**：删除 `tests/sso_integration.rs` 和 `benches/sso_bench.rs`。

---

### T12 · 文档 + CHANGELOG + 版本 bump + semver-checks + crates.io

**目标**：更新文档、版本号、发布到 crates.io。

**输入**：spec.md NFR-3.1、AC-2.4、AC-2.5
**输出**：
- `CHANGELOG.md`（修改）
- `Cargo.toml`（workspace version 0.6.1 → 0.6.2）
- crates.io 发布

**验证**：
- `cargo semver-checks check-release` 通过
- `sz-pay` 项目升级后编译通过（AC-2.5）
- crates.io 发布成功

**依赖**：T11

**步骤**：

#### 步骤 12.1：版本 bump

修改 `Cargo.toml`（workspace 根）Line 36：

```toml
version = "0.6.2"  # was "0.6.1"
```

#### 步骤 12.2：CHANGELOG

在 `CHANGELOG.md` 顶部新增：

```markdown
## [0.6.2] - 2026-08-07

### Added
- **SSO 单点登录 + Refresh Token 双 Token 机制**（`sz-rust-auth-facade`）
  - 新增 `refresh` 模块：`SsoJwtCodec`、`SsoClaims`、`RefreshTokenIssuer`、`RefreshTokenVerifier`、`RefreshTokenRevoker`、`RefreshTokenStore` trait + `MemoryRefreshTokenStore` / `CacheRefreshTokenStore` 实现
  - 新增 `sso` 模块：`SsoService`、`UserAuthenticator` / `UserInfoProvider` trait、axum 路由（feature = "axum"）
  - 新增 `remote-validate` feature：远程 Token 校验
- **SSO 业务系统中间件**（`sz-rust-middleware-facade`）
  - 新增 `sso_middleware` 模块：本地验签（默认）+ 远程校验（feature = "remote-validate"）
- 基准测试 `sso_bench`：本地验签 p99 < 1μs，Token 轮换 p99 < 50μs

### Changed
- `sz-rust-sz300/controllers/auth.rs:172`：`refresh` 空实现替换为调用 `SsoService::refresh`（HTTP 路由不变，对前端透明）

### Security
- Refresh Token 复用攻击检测：检测到复用即撤销用户所有 Token（NFR-2.5）
- fail-closed：黑名单故障时返回 503，禁止放行（NFR-4.1）
- JWT secret Debug 脱敏（NFR-2.6）
- tracing 日志 skip 敏感参数（C-12）
```

#### 步骤 12.3：semver-checks

```powershell
cargo install cargo-semver-checks  # 若未安装
cargo semver-checks check-release -p sz-rust-auth-facade
cargo semver-checks check-release -p sz-rust-middleware-facade
```

#### 步骤 12.4：sz-pay 兼容验证（AC-2.5）

```powershell
# 在 sz-pay 项目中
cd E:\vue\test\sz-pay
# 修改 Cargo.toml: sz-rust = "0.6.2"
cargo build
```

#### 步骤 12.5：crates.io 发布

```powershell
cargo publish -p sz-rust-auth-facade --dry-run
cargo publish -p sz-rust-auth-facade
cargo publish -p sz-rust-middleware-facade --dry-run
cargo publish -p sz-rust-middleware-facade
```

> **注意**：发布前确认 `服务器信息.md` 中的 crates.io token 配置。发布顺序：先 auth-facade（middleware-facade 依赖它），再 middleware-facade。

**风险**：
1. **semver 破坏**：若 cargo-semver-checks 报 breaking change，需检查是否误改公开 API。
2. **sz-pay 编译失败**：若 sz-pay 用了被改的 API（理论上不会，因 NFR-3.1 保证仅新增），需适配。
3. **crates.io 发布失败**：版本号已存在 / token 失效 / 依赖未先发布。

**回滚**：
- 版本号回退 0.6.1
- crates.io 已发布版本无法删除，但可 yank（`cargo yank`）

---

## 3. 风险登记册与回滚策略

| 风险 ID | 风险描述 | 等级 | 影响任务 | 缓解措施 | 回滚策略 |
|---------|---------|------|---------|---------|---------|
| R1 | `hmac`/`subtle` 版本与 workspace 不兼容 | 低 | T0, T1 | 锁定 workspace 版本（hmac 0.12, subtle 2） | 删除 workspace 依赖声明 |
| R2 | base64 padding 不一致导致 decode 旧 token 失败 | 中 | T1 | 强制 `URL_SAFE_NO_PAD`（对齐 sz-orm-auth jwt.rs:218） | 修正编码策略 |
| R3 | `SsoJwtCodec` 未实现 Clone 导致 T4 编译失败 | 低 | T4 | T1 补 `#[derive(Clone)]` | 补 Clone |
| R4 | 复用攻击检测中 claims 丢失 | 中 | T4 | 辅助函数 `claims_user_id_from_token` 重新 decode | 已在步骤 4.2 修正 |
| R5 | `JwtBlacklist::is_revoked` 同步调用阻塞 async | 低 | T3, T8 | 实际仅查内存 Cache，阻塞可忽略 | 若未来切 Redis 需改 async |
| R6 | `Cache` API 与 design 假设不符 | 中 | T2 | 步骤 2.1 实际读取 cache-facade 确认后修正 | 修正实现 |
| R7 | `AuthenticatedUser` 定义不符 | 低 | T8 | 实际读取 auth.rs 确认 | 修正引用 |
| R8 | axum 0.8 API 变化 | 中 | T7, T8 | 对齐 workspace axum 0.8 文档 | 修正 handler 签名 |
| R9 | reqwest mock 测试缺 dev-dependency | 低 | T8 | 新增 `wiremock` 或 `httpmock` dev-dependency | 跳过远程校验测试 |
| R10 | tarpaulin Windows 不支持 | 低 | T10 | CI/WSL 中运行 | 用 `cargo test` + 人工审查替代 |
| R11 | 集成测试端口冲突 | 低 | T11 | 用 `127.0.0.1:0` 随机端口 | 修正端口配置 |
| R12 | semver 破坏 | 中 | T12 | cargo-semver-checks 验证 | 检查并修正公开 API |
| R13 | sz-pay 编译失败 | 中 | T12 | 仅新增 API，不改现有签名 | 适配 sz-pay |
| R14 | crates.io 发布失败 | 中 | T12 | dry-run 先验证 | yank 已发布版本 |
| R15 | 并发 rotate 误判复用攻击 | 中 | T4 | 安全优先策略（ADR-4），tracing 告警区分 | 文档说明行为 |

---

## 4. 总体验证清单

### 4.1 编译验证

```powershell
$env:CARGO_INCREMENTAL=0
cargo check --workspace --all-features
cargo build --workspace --all-features
```

**预期**：零错误，零 `unsafe_code` 警告（workspace `forbid` 生效）。

### 4.2 测试验证

```powershell
cargo test --workspace --all-features
cargo test -p sz-rust-auth-facade --all-features
cargo test -p sz-rust-middleware-facade --all-features
cargo test -p sz-rust-auth-facade --test sso_integration --features axum
```

**预期**：全部通过，含边界测试（AC-3.4）。

### 4.3 Clippy 验证

```powershell
cargo clippy --workspace --all-features -- -D warnings
```

**预期**：零警告（AC-3.2）。

### 4.4 格式化验证

```powershell
cargo fmt --all -- --check
```

**预期**：零差异。若有差异，运行 `cargo fmt --all` 修正。

### 4.5 rustdoc 验证

```powershell
cargo doc --workspace --all-features --no-deps
```

**预期**：零警告，所有公开 API 有 rustdoc 注释（AC-3.3）。

### 4.6 semver 验证

```powershell
cargo semver-checks check-release --workspace
```

**预期**：通过，无 breaking change（AC-2.4）。

### 4.7 基准验证

```powershell
cargo bench -p sz-rust-auth-facade --bench sso_bench -- --quick
```

**预期**：
- `local_validate` p99 < 1μs（AC-2.1）
- `rotate` p99 < 50μs（AC-2.2）

### 4.8 sz-pay 兼容验证

```powershell
# 在 E:\vue\test\sz-pay
cargo build
```

**预期**：成功，无编译错误（AC-2.5）。

### 4.9 覆盖率验证（可选，Linux/WSL）

```powershell
cargo tarpaulin -p sz-rust-auth-facade --all-features
cargo tarpaulin -p sz-rust-middleware-facade --all-features
```

**预期**：`refresh.rs` / `sso.rs` / `sso_middleware.rs` 行覆盖 ≥ 90%（AC-3.1）。

### 4.10 安全验证

- [ ] `grep -r "unsafe" packages/sz-rust-auth-facade/src/refresh.rs` → 零命中
- [ ] `grep -r "unsafe" packages/sz-rust-auth-facade/src/sso.rs` → 零命中
- [ ] `grep -r "unsafe" packages/sz-rust-middleware-facade/src/sso_middleware.rs` → 零命中
- [ ] Debug 输出不含 secret（`test_sso_jwt_codec_debug_redacts_secret` 通过）
- [ ] tracing 日志不含 token 明文（`#[tracing::instrument(skip(self, token))]` 全部标注）
- [ ] fail-closed：黑名单故障 → 503（`test_middleware_local_blacklist_fail_closed` 通过）

---

## 5. 执行顺序与里程碑

| 里程碑 | 任务 | 交付物 | 验证命令 |
|--------|------|--------|---------|
| M1: 基础设施 | T0, T1 | `refresh.rs`（Error + SsoClaims + SsoJwtCodec） | `cargo test -p sz-rust-auth-facade --lib refresh` |
| M2: 存储与校验 | T2, T3 | Store trait + Verifier | `cargo test -p sz-rust-auth-facade --lib refresh::{store,verifier}` |
| M3: 签发与撤销 | T4, T5 | Issuer + Revoker | `cargo test -p sz-rust-auth-facade --lib refresh::{issuer,revoker}` |
| M4: SSO 中心 | T6, T7 | SsoService + axum 路由 | `cargo test -p sz-rust-auth-facade --features axum --lib sso` |
| M5: 中间件 | T8 | sso_middleware | `cargo test -p sz-rust-middleware-facade --all-features --lib sso_middleware` |
| M6: 集成 | T9 | sz300 refresh 替换 | `cargo build -p sz-rust-sz300` |
| M7: 测试 | T10, T11 | 单元 + 边界 + 集成 + 基准 | `cargo test --workspace --all-features` |
| M8: 发布 | T12 | 0.6.2 + crates.io | `cargo semver-checks check-release` + `cargo publish` |

**关键路径**：T0 → T1 → T2 → T3 → T4 → T6 → T7 → T9 → T10 → T11 → T12

**可并行任务**：
- T5 可与 T6 并行（T5 仅依赖 T1/T2/T4）
- T8 可与 T7 并行（T8 依赖 T6，T7 依赖 T6）
- T10 的各模块测试可并行编写

---

## 6. 验收标准映射（tasks → spec AC）

| spec AC | 对应任务 | 验证方式 |
|---------|---------|---------|
| AC-1.1 登录签发双 Token | T4 `test_issuer_issue_*` + T6 `test_sso_service_login_success` | 单元测试 |
| AC-1.2 Token 轮换 | T4 `test_issuer_rotate_*` | 单元测试 |
| AC-1.3 Token 类型隔离 | T3 `test_verifier_rejects_*` | 单元测试 |
| AC-1.4 撤销生效 | T5 + T6 `test_sso_service_revoke_success` | 单元测试 |
| AC-1.5 SSO 中间件本地验签 | T8 `test_middleware_local_valid_access` | 中间件测试 |
| AC-1.6 SSO 中间件白名单 | T8 `test_middleware_local_whitelist*` | 中间件测试 |
| AC-1.7 远程校验 | T8 `test_middleware_remote_*` | 远程校验测试 |
| AC-2.1 本地验签性能 | T11 `bench_local_validate` | `cargo bench` |
| AC-2.2 Token 轮换性能 | T11 `bench_rotate` | `cargo bench` |
| AC-2.3 无 unsafe | 全任务 | `cargo build --all-features`（workspace forbid） |
| AC-2.4 semver 兼容 | T12 | `cargo semver-checks` |
| AC-2.5 sz-pay 兼容 | T12 | `cargo build` in sz-pay |
| AC-2.6 日志脱敏 | T1 `test_sso_jwt_codec_debug_redacts_secret` + tracing skip 审查 | 单元测试 + 代码审查 |
| AC-2.7 fail-closed | T8 `test_middleware_local_blacklist_fail_closed` | 中间件测试 |
| AC-2.8 复用攻击检测 | T4 `test_issuer_rotate_reuse_detection` | 单元测试 |
| AC-3.1 测试覆盖 ≥ 90% | T10 | `cargo tarpaulin` |
| AC-3.2 Clippy | 全任务 | `cargo clippy --all-features -- -D warnings` |
| AC-3.3 rustdoc | 全任务 | `cargo doc --all-features --no-deps` |
| AC-3.4 边界测试 | T10 `test_boundary_*` | 边界测试套件 |

---

## 7. 变更记录

| 日期 | 版本 | 变更 | 作者 |
|------|------|------|------|
| 2026-08-07 | tasks-v1.0 | 初稿，基于 spec-v1.0 + design-v1.0 + 代码现状验证生成 | spec-task-agent |