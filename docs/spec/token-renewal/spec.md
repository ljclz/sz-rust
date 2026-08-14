# Token 自动续期（v0.6.5）需求规格

## 1. 概述

### 1.1 功能名称
Token 自动续期（Access Token Auto-Renewal on Validation）

### 1.2 版本
v0.6.4 → v0.6.5（semver 兼容，新增方法 + 可选配置，不破坏现有 API）

### 1.3 一句话描述
在 `validate`（校验 accessToken）时，如果 accessToken 剩余 TTL 低于阈值，自动签发新 accessToken 并随响应返回，客户端无需主动调用 refresh 端点即可实现无感续期。

### 1.4 动机
- **当前痛点**：accessToken 默认 15 分钟过期，客户端必须在过期前主动调用 `/sso/refresh`，增加客户端复杂度和网络往返
- **业界实践**：OAuth2.0 / OIDC 主流实现（Auth0、Okta、AWS Cognito）均支持 validate-time silent renewal
- **目标**：客户端只需在每次请求携带 accessToken，中间件/validate 端点自动返回续期后的新 token，实现"无感续期"

## 2. 利益相关者

| 角色 | 关注点 |
|------|--------|
| 框架使用者（sz300 等业务应用） | 减少客户端 token 管理代码，降低 refresh 端点调用频率 |
| 终端用户 | 无感续期，不会因 token 过期被突然踢出 |
| 安全审计 | 续期不签发新 refreshToken，不递增版本号，不撤销旧 accessToken |
| sz-pay 兼容性 | 现有 validate API 不变，新功能通过新方法 + 可选配置启用 |

## 3. 现有实现基线

### 3.1 SsoService::validate（当前）
```rust
// packages/sz-rust-auth-facade/src/sso.rs:144
pub async fn validate(&self, access_token: &str) -> Result<SsoClaims, RefreshTokenError>
```
- 仅校验 accessToken，返回 SsoClaims
- 不签发新 token

### 3.2 SsoJwtCodec::encode
```rust
// packages/sz-rust-auth-facade/src/refresh.rs:208
pub fn encode(&self, claims: &SsoClaims) -> Result<String, RefreshTokenError>
```
- 可直接复用签发新 accessToken

### 3.3 SsoClaims::access
```rust
// packages/sz-rust-auth-facade/src/refresh.rs:125
pub fn access(user_id: i64, username: &str, exp: i64, issuer: &str, ver: u64) -> Self
```
- 可构造新 accessToken 的 claims

### 3.4 RefreshTokenConfig
```rust
// packages/sz-rust-auth-facade/src/refresh.rs:300
pub struct RefreshTokenConfig {
    pub access_token_ttl: chrono::Duration,   // 默认 900 秒
    pub refresh_token_ttl: chrono::Duration,  // 默认 604800 秒
    pub issuer: String,                        // 默认 "sz-rust-sso"
}
```

### 3.5 axum validate 端点（当前）
```rust
// packages/sz-rust-auth-facade/src/sso.rs:274
async fn validate(...) -> Response {
    // 返回 ValidateResponse { valid, user_id, expires_at }
}
```

## 4. 需求（EARS 格式）

### 4.1 RenewalConfig 配置结构

#### REQ-001: RenewalConfig 默认值
**WHEN** 用户使用 `RenewalConfig::default()` 创建配置
**THE SYSTEM SHALL** 设置 `renewal_threshold` 为 300 秒（5 分钟）、`renewal_ratio` 为 0.2（剩余 TTL < 20% 时触发续期）
**AND** `enabled` 为 `true`

#### REQ-002: RenewalConfig 禁用续期
**WHEN** 用户设置 `RenewalConfig { enabled: false, .. }`
**THE SYSTEM SHALL** `validate_with_renewal` 方法始终返回 `None` 作为新 token，行为等同于 `validate`

#### REQ-003: renewal_threshold 边界
**WHEN** `renewal_threshold` 设置为 0
**THE SYSTEM SHALL** 只要 accessToken 未过期就触发续期（每次 validate 都续期）
**AND** 不返回错误

#### REQ-004: renewal_ratio 边界
**WHEN** `renewal_ratio` 设置为 0.0 或 1.0
**THE SYSTEM SHALL** 接受该值（0.0 = 永不续期，1.0 = 总是续期）
**AND** 不返回错误

### 4.2 validate_with_renewal 方法

#### REQ-005: 续期触发条件
**WHEN** accessToken 校验通过
**AND** 剩余 TTL（`claims.exp - now`）< `max(renewal_threshold, access_token_ttl * renewal_ratio)`
**THE SYSTEM SHALL** 签发新 accessToken（同一 user_id / username / ver / issuer / roles / permissions）
**AND** 返回 `Ok((claims, Some(new_access_token)))`

#### REQ-006: 不续期条件
**WHEN** accessToken 校验通过
**AND** 剩余 TTL >= `max(renewal_threshold, access_token_ttl * renewal_ratio)`
**THE SYSTEM SHALL** 返回 `Ok((claims, None))`，不签发新 token

#### REQ-007: 续期禁用
**WHEN** `RenewalConfig.enabled == false`
**AND** accessToken 校验通过
**THE SYSTEM SHALL** 返回 `Ok((claims, None))`，不检查 TTL

#### REQ-008: 校验失败不续期
**WHEN** accessToken 校验失败（过期 / 签名无效 / 黑名单 / 版本不匹配等）
**THE SYSTEM SHALL** 返回 `Err(RefreshTokenError)`，不签发任何新 token

#### REQ-009: 新 accessToken 属性
**WHEN** 续期触发并签发新 accessToken
**THE SYSTEM SHALL** 新 token 的 `exp = now + access_token_ttl`
**AND** 新 token 的 `iat = now`
**AND** 新 token 的 `jti` 为新生成的 UUID v4
**AND** 新 token 的 `token_type = "access"`
**AND** 新 token 的 `user_id / sub / ver / iss / roles / permissions` 与原 token 相同

#### REQ-010: 不签发新 refreshToken
**WHEN** 续期触发
**THE SYSTEM SHALL** 不签发新 refreshToken
**AND** 不修改 refreshToken 存储中的版本号

#### REQ-011: 不撤销旧 accessToken
**WHEN** 续期触发并签发新 accessToken
**THE SYSTEM SHALL** 不将旧 accessToken 的 jti 加入黑名单
**AND** 旧 accessToken 在过期前仍然有效

#### REQ-012: 不递增版本号
**WHEN** 续期触发
**THE SYSTEM SHALL** 不调用 `store.increment_version()`
**AND** 新 token 的 `ver` 与原 token 相同

### 4.3 SsoService 集成

#### REQ-013: SsoService 持有 RenewalConfig
**WHEN** 创建 `SsoService::new(issuer, verifier, revoker, user_auth)`
**THE SYSTEM SHALL** 使用 `RenewalConfig::default()` 作为默认续期配置

#### REQ-014: SsoService::with_renewal_config
**WHEN** 用户调用 `SsoService::with_renewal_config(config)` 设置续期配置
**THE SYSTEM SHALL** 更新内部续期配置
**AND** 返回 `&mut Self` 以支持链式调用

#### REQ-015: validate 方法不变
**WHEN** 用户调用现有 `SsoService::validate(access_token)`
**THE SYSTEM SHALL** 行为与 v0.6.4 完全一致，不触发续期
**AND** 返回 `Result<SsoClaims, RefreshTokenError>`

### 4.4 axum HTTP 端点增强

#### REQ-016: validate 端点响应增强
**WHEN** `/sso/validate?token=xxx` 请求到达
**AND** 续期触发
**THE SYSTEM SHALL** 响应 JSON `data` 字段包含 `new_access_token`（String）和 `new_access_expires_at`（i64 Unix 时间戳）
**AND** 原有字段 `valid / user_id / expires_at` 保持不变

#### REQ-017: validate 端点不续期时响应
**WHEN** `/sso/validate?token=xxx` 请求到达
**AND** 续期未触发（TTL 充足或禁用）
**THE SYSTEM SHALL** `new_access_token` 为 `null`、`new_access_expires_at` 为 `null`
**AND** 原有字段保持不变

#### REQ-018: validate 端点向后兼容
**WHEN** 客户端不解析 `new_access_token` / `new_access_expires_at` 字段
**THE SYSTEM SHALL** 客户端行为不受影响（JSON 新字段被忽略）

### 4.5 中间件响应头注入

#### REQ-019: 中间件续期响应头
**WHEN** `sso_middleware` 校验通过且续期触发
**AND** `SsoMiddlewareConfig.renewal_config.enabled == true`
**THE SYSTEM SHALL** 在响应头中注入 `X-Renewed-Access-Token`（新 accessToken）和 `X-Renewed-Expires-At`（Unix 时间戳）
**AND** 不影响响应体

#### REQ-020: 中间件续期禁用
**WHEN** `SsoMiddlewareConfig.renewal_config.enabled == false` 或 `renewal_config` 为 `None`
**THE SYSTEM SHALL** 不注入续期响应头
**AND** 中间件行为与 v0.6.4 一致

### 4.6 安全约束

#### REQ-021: 续期不绕过黑名单
**WHEN** accessToken 的 jti 在黑名单中
**THE SYSTEM SHALL** 返回 `Err(RefreshTokenError::Revoked)`，不签发新 token

#### REQ-022: 续期不绕过版本检查
**WHEN** accessToken 的 `ver` 与 store 中当前版本不匹配
**THE SYSTEM SHALL** 返回 `Err(RefreshTokenError::VersionMismatch)`，不签发新 token

#### REQ-023: 续期不绕过过期检查
**WHEN** accessToken 已过期（`exp < now`）
**THE SYSTEM SHALL** 返回 `Err(RefreshTokenError::Expired)`，不签发新 token

#### REQ-024: 续期 token 不含敏感信息
**WHEN** 新 accessToken 被签发
**THE SYSTEM SHALL** 新 token 的 claims 不包含密码、密钥等敏感字段
**AND** `SsoClaims` 的 `#[serde(skip_serializing)]` 约束保持不变

### 4.7 可观测性

#### REQ-025: 续期事件日志
**WHEN** 续期触发
**THE SYSTEM SHALL** 输出 `tracing::debug!` 日志，包含 `user_id`、`old_jti`、`new_jti`、`old_exp`、`new_exp` 字段
**AND** 不输出 token 明文

#### REQ-026: 续期不触发告警
**WHEN** 续期触发（正常流程）
**THE SYSTEM SHALL** 不输出 `tracing::warn!` 或 `tracing::error!` 日志
**AND** 不与复用攻击检测的 `warn!` 混淆

## 5. 非功能需求

### 5.1 性能
- `validate_with_renewal` 在不续期时，性能与 `validate` 相比额外开销 < 100ns（仅一次时间比较）
- `validate_with_renewal` 在续期时，额外开销 = 一次 `SsoJwtCodec::encode`（基准测试中 encode 856ns）
- 不引入新的 async 调用（续期不查 store / blacklist，复用 validate 已有结果）

### 5.2 兼容性
- `SsoService::validate` 签名不变，行为不变
- `ValidateResponse` 新增字段使用 `Option<String>` / `Option<i64>`，serde 序列化为 `null` 时向后兼容
- `RenewalConfig` 默认启用，但 `validate` 方法不使用续期（仅 `validate_with_renewal` 使用）
- 中间件默认 `renewal_config = None`，不改变现有行为

### 5.3 安全
- 续期不延长 refreshToken 生命周期
- 续期不绕过任何校验步骤（黑名单 / 版本 / 过期 / 签名 / token_type）
- 新 accessToken 与原 accessToken 共享同一 ver，撤销用户所有 Token 时同时失效

### 5.4 可测试性
- 所有续期逻辑可通过单元测试验证（无需 Redis / 数据库）
- 边界用例：threshold=0、ratio=0.0、ratio=1.0、TTL 恰好等于阈值
- 集成测试：axum 端点续期响应、中间件响应头

## 6. 约束

### 6.1 不引入新依赖
- 复用现有 `uuid`、`chrono`、`hmac`、`sha2`、`base64`
- 不引入新 crate

### 6.2 不破坏 sz-pay 兼容性
- `SsoService::validate` 签名不变
- `ValidateResponse` 新字段为 `Option`，serde 反序列化兼容
- 中间件默认不启用续期

### 6.3 feature gate
- 续期核心逻辑不需要新 feature gate（纯计算，无网络依赖）
- axum 端点增强在现有 `axum` feature 下
- 中间件响应头在现有 `remote-validate` feature 下（或无 feature gate，取决于实现）

## 7. 验收标准

| ID | 验收条件 |
|----|---------|
| AC-001 | `RenewalConfig::default()` 返回 threshold=300s, ratio=0.2, enabled=true |
| AC-002 | `validate_with_renewal` 在 TTL < 阈值时返回 `Some(new_token)` |
| AC-003 | `validate_with_renewal` 在 TTL >= 阈值时返回 `None` |
| AC-004 | `validate_with_renewal` 在 `enabled=false` 时始终返回 `None` |
| AC-005 | `validate_with_renewal` 在校验失败时返回 `Err` |
| AC-006 | 新 accessToken 的 exp = now + access_token_ttl |
| AC-007 | 新 accessToken 的 ver 与原 token 相同 |
| AC-008 | 续期不调用 `store.increment_version()` |
| AC-009 | 续期不将旧 jti 加入黑名单 |
| AC-010 | `/sso/validate` 端点续期时返回 `new_access_token` 字段 |
| AC-011 | `/sso/validate` 端点不续期时 `new_access_token` 为 `null` |
| AC-012 | 中间件续期时注入 `X-Renewed-Access-Token` 响应头 |
| AC-013 | `SsoService::validate` 行为与 v0.6.4 一致 |
| AC-014 | 全 workspace `cargo test` 通过 |
| AC-015 | sz-pay 5139 测试通过 |
| AC-016 | clippy 0 warning |

## 8. 范围外

- refreshToken 自动续期（仅 accessToken）
- 续期事件持久化审计日志（仅 tracing 日志）
- 续期限流（rate limiting）
- 客户端 SDK（仅服务端实现）
- 续期通知推送（WebSocket / SSE）