# 多设备会话管理（P1）需求规格

## 1. 概述

### 1.1 功能名称
多设备会话管理（Multi-Device Session Management）

### 1.2 版本
v0.6.5 → v0.6.6（semver 兼容，新增方法 + 可选配置，不破坏现有 API）

### 1.3 一句话描述
支持用户多设备同时登录，每设备独立 Token 对，可按设备查询/撤销，实现"在线设备管理"。

### 1.4 动机
- **当前痛点**：同一用户多次登录会签发多个独立 Token 对，但无法区分设备、无法按设备撤销、无法查看在线设备列表
- **业界实践**：Auth0、Okta、GitHub、Google 均支持多设备会话管理，用户可在安全设置中查看并撤销特定设备
- **目标**：登录时绑定设备信息，支持按设备查询/撤销/心跳更新，实现精细化会话管理

## 2. 利益相关者

| 角色 | 关注点 |
|------|--------|
| 框架使用者 | 精细化会话管理，按设备撤销，安全审计 |
| 终端用户 | 查看自己在线设备列表，踢出可疑设备 |
| 安全审计 | 设备指纹、IP、User-Agent 记录，异常登录检测 |
| sz-pay 兼容性 | 现有 login/validate/revoke API 不变，新功能通过新方法启用 |

## 3. 现有实现基线

### 3.1 SsoClaims（当前）
```rust
// refresh.rs:93
pub struct SsoClaims {
    pub sub: String,       // 用户名
    pub exp: i64,          // 过期时间
    pub iat: i64,          // 签发时间
    pub iss: Option<String>,  // 签发人
    pub user_id: Option<i64>, // 用户 ID
    pub token_type: String,   // "access" / "refresh"
    pub jti: String,          // JWT ID
    pub ver: u64,             // 版本号（用户级撤销）
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
}
```
- **无设备信息**：无法区分不同设备的 Token

### 3.2 SsoService::login（当前）
```rust
// sso.rs:108
pub async fn login(&self, username: &str, password: &str) -> Result<LoginResponse, RefreshTokenError>
```
- **无设备参数**：不记录设备信息

### 3.3 RefreshTokenStore trait（当前）
```rust
// refresh.rs:326
pub trait RefreshTokenStore: Send + Sync {
    async fn get_version(&self, user_id: i64) -> Result<u64, RefreshTokenError>;
    async fn increment_version(&self, user_id: i64) -> Result<u64, RefreshTokenError>;
}
```
- **仅维护版本号**：无设备会话记录

### 3.4 RefreshTokenIssuer::issue（当前）
```rust
// refresh.rs:527
pub async fn issue(&self, user_id: i64, username: &str) -> Result<TokenPair, RefreshTokenError>
```
- **无设备绑定**：签发的 Token 不含设备 ID

## 4. 需求（EARS 格式）

### 4.1 设备信息模型

#### REQ-001: DeviceInfo 结构体
**THE SYSTEM SHALL** 提供 `DeviceInfo` 结构体，包含 `device_id: String`（设备唯一标识）、`device_type: Option<String>`（设备类型：web/ios/android/pc）、`user_agent: Option<String>`（浏览器/客户端 UA）、`ip: Option<String>`（登录 IP）、`device_name: Option<String>`（设备名称，如"iPhone 15 Pro"）

#### REQ-002: DeviceInfo 序列化
**WHEN** `DeviceInfo` 被序列化为 JSON
**THE SYSTEM SHALL** 所有 `Option` 字段为 `None` 时序列化为 `null`（使用 `skip_serializing_if = "Option::is_none"` 不输出，保持 JSON 精简）

#### REQ-003: device_id 自动生成
**WHEN** 调用 `DeviceInfo::new()` 不提供 device_id
**THE SYSTEM SHALL** 自动生成 UUID v4 作为 device_id

### 4.2 设备会话存储

#### REQ-004: DeviceSessionStore trait
**THE SYSTEM SHALL** 提供 `DeviceSessionStore` trait，包含：
- `register_session(user_id, device_id, device_info, jti) -> Result<()>` — 注册设备会话
- `get_sessions(user_id) -> Result<Vec<DeviceSession>>` — 查询用户所有在线设备
- `get_session(user_id, device_id) -> Result<Option<DeviceSession>>` — 查询特定设备会话
- `revoke_session(user_id, device_id) -> Result<Option<String>>` — 撤销设备会话，返回被撤销的 jti（用于加入黑名单）
- `update_last_active(user_id, device_id) -> Result<()>` — 更新设备最后活跃时间
- `cleanup_expired(user_id, ttl_secs) -> Result<Vec<String>>` — 清理过期会话，返回被清理的 jti 列表

#### REQ-005: DeviceSession 结构体
**THE SYSTEM SHALL** `DeviceSession` 包含 `device_id: String`、`device_info: DeviceInfo`、`jti: String`、`created_at: i64`、`last_active: i64`

#### REQ-006: MemoryDeviceSessionStore
**THE SYSTEM SHALL** 提供 `MemoryDeviceSessionStore`（内存实现，测试用），使用 `Arc<RwLock<HashMap<(i64, String), DeviceSession>>>` 存储

#### REQ-007: RedisDeviceSessionStore（feature gate `redis-store`）
**WHEN** 启用 `redis-store` feature
**THE SYSTEM SHALL** 提供 `RedisDeviceSessionStore`，使用 Redis Hash 存储：key = `sso:sessions:{user_id}`，field = `{device_id}`，value = JSON(DeviceSession)

### 4.3 SsoClaims 扩展

#### REQ-008: SsoClaims 新增 device_id 字段
**THE SYSTEM SHALL** 在 `SsoClaims` 新增 `device_id: Option<String>` 字段，使用 `#[serde(default, skip_serializing_if = "Option::is_none")]`

#### REQ-009: 向后兼容
**WHEN** 现有 Token（无 device_id）被校验
**THE SYSTEM SHALL** `device_id` 反序列化为 `None`，校验行为不变

### 4.4 登录扩展

#### REQ-010: login_with_device 方法
**THE SYSTEM SHALL** 新增 `SsoService::login_with_device(username, password, device_info) -> Result<LoginResponse, RefreshTokenError>`，签发的 Token claims 包含 `device_id`

#### REQ-011: login 方法不变
**WHEN** 用户调用现有 `SsoService::login(username, password)`
**THE SYSTEM SHALL** 行为与 v0.6.5 完全一致，签发的 Token `device_id = None`

#### REQ-012: 登录注册会话
**WHEN** `login_with_device` 成功
**THE SYSTEM SHALL** 调用 `DeviceSessionStore::register_session` 注册设备会话，记录 device_id、device_info、refresh_token 的 jti

### 4.5 设备管理 API

#### REQ-013: list_devices 方法
**THE SYSTEM SHALL** 新增 `SsoService::list_devices(user_id) -> Result<Vec<DeviceSession>, RefreshTokenError>`，返回用户所有在线设备

#### REQ-014: revoke_device 方法
**THE SYSTEM SHALL** 新增 `SsoService::revoke_device(user_id, device_id) -> Result<(), RefreshTokenError>`：
1. 从 `DeviceSessionStore` 查询设备会话，获取 jti
2. 将 jti 加入黑名单
3. 从 `DeviceSessionStore` 删除会话
4. 不递增版本号（仅撤销该设备，不影响其他设备）

#### REQ-015: revoke_all 保持不变
**WHEN** 用户调用 `revoke_all(user_id)`
**THE SYSTEM SHALL** 递增版本号（撤销所有设备所有 Token），并清空 `DeviceSessionStore` 中该用户的所有会话

#### REQ-016: update_device_active 方法
**THE SYSTEM SHALL** 新增 `SsoService::update_device_active(user_id, device_id) -> Result<(), RefreshTokenError>`，更新设备最后活跃时间（心跳）

#### REQ-017: cleanup_expired_devices 方法
**THE SYSTEM SHALL** 新增 `SsoService::cleanup_expired_devices(user_id, ttl_secs) -> Result<usize, RefreshTokenError>`，清理过期设备会话，返回清理数量

### 4.6 校验扩展

#### REQ-018: validate 更新设备活跃
**WHEN** `validate` 或 `validate_with_renewal` 校验通过
**AND** Token 包含 `device_id`
**AND** `SsoService` 持有 `DeviceSessionStore`
**THE SYSTEM SHALL** 调用 `update_last_active(user_id, device_id)` 更新设备活跃时间
**AND** 不影响校验结果（更新失败时仅 warn 日志，不中断校验）

#### REQ-019: validate 无 device_id 不更新
**WHEN** Token 不包含 `device_id`（v0.6.5 之前签发的 Token）
**THE SYSTEM SHALL** 不调用 `update_last_active`，校验行为不变

### 4.7 刷新扩展

#### REQ-020: refresh 更新会话 jti
**WHEN** `refresh` 成功轮换 refreshToken
**AND** 新 Token 包含 `device_id`
**THE SYSTEM SHALL** 更新 `DeviceSessionStore` 中该设备会话的 jti 为新 refreshToken 的 jti
**AND** 更新 `last_active`

### 4.8 axum HTTP 端点

#### REQ-021: POST /sso/login 支持 device_info
**WHEN** `/sso/login` 请求体包含 `device_info` 字段
**THE SYSTEM SHALL** 调用 `login_with_device` 签发绑定设备的 Token
**AND** 响应中包含 `device_id`

#### REQ-022: GET /sso/devices/:user_id 端点
**THE SYSTEM SHALL** 新增 `GET /sso/devices/:user_id` 端点，返回用户所有在线设备列表

#### REQ-023: POST /sso/devices/revoke 端点
**THE SYSTEM SHALL** 新增 `POST /sso/devices/revoke` 端点，请求体包含 `user_id` 和 `device_id`，撤销指定设备会话

#### REQ-024: POST /sso/devices/heartbeat 端点
**THE SYSTEM SHALL** 新增 `POST /sso/devices/heartbeat` 端点，请求体包含 `user_id` 和 `device_id`，更新设备活跃时间

### 4.9 安全约束

#### REQ-025: device_id 不可伪造
**WHEN** Token 的 `device_id` 与 `DeviceSessionStore` 中记录的不匹配
**THE SYSTEM SHALL** 不影响校验结果（device_id 仅用于会话管理，不用于校验）

#### REQ-026: revoke_device 不影响其他设备
**WHEN** `revoke_device(user_id, device_a)` 被调用
**THE SYSTEM SHALL** 设备 B 的 Token 仍然有效
**AND** 不递增版本号

#### REQ-027: 设备数量限制
**WHEN** 用户注册新设备会话
**AND** 已有设备数量超过 `max_devices`（默认 10）
**THE SYSTEM SHALL** 撤销最旧设备的会话（LRU 淘汰）
**AND** 输出 `tracing::warn!` 日志

### 4.10 可观测性

#### REQ-028: 设备注册日志
**WHEN** 新设备会话注册
**THE SYSTEM SHALL** 输出 `tracing::info!` 日志，包含 user_id、device_id、device_type、ip

#### REQ-029: 设备撤销日志
**WHEN** 设备会话被撤销
**THE SYSTEM SHALL** 输出 `tracing::info!` 日志，包含 user_id、device_id、reason（manual/expired/lru）

## 5. 非功能需求

### 5.1 性能
- `list_devices` 内存实现 < 1μs（HashMap 查询）
- `revoke_device` 内存实现 < 5μs（HashMap 删除 + 黑名单写入）
- Redis 实现 `list_devices` < 1ms（HGETALL）

### 5.2 兼容性
- `SsoService::login` 签名不变，行为不变
- `SsoClaims` 新增 `device_id` 为 `Option`，serde 反序列化兼容
- `DeviceSessionStore` 为可选依赖，不启用时 `login_with_device` 等方法返回 `Err(NotConfigured)`
- 现有 Token（无 device_id）继续有效

### 5.3 安全
- device_id 使用 UUID v4，不可预测
- revoke_device 仅撤销该设备 Token，不影响其他设备
- 设备数量限制防止 Token 泛滥

### 5.4 可测试性
- 所有逻辑可通过单元测试验证（MemoryDeviceSessionStore）
- 边界用例：max_devices=1、device_id 为空、同一设备重复登录

## 6. 约束

### 6.1 不引入新依赖
- 复用现有 uuid、chrono、serde、parking_lot

### 6.2 不破坏 sz-pay 兼容性
- `SsoService::login` 签名不变
- `SsoClaims` 新字段为 `Option`，serde 反序列化兼容
- `DeviceSessionStore` 为可选，默认不启用

### 6.3 feature gate
- `DeviceSessionStore` trait 和 `MemoryDeviceSessionStore` 始终可用（无网络依赖）
- `RedisDeviceSessionStore` 在 `redis-store` feature 下

## 7. 验收标准

| ID | 验收条件 |
|----|---------|
| AC-001 | `DeviceInfo::new()` 自动生成 UUID v4 device_id |
| AC-002 | `login_with_device` 签发的 Token 包含 device_id |
| AC-003 | `login` 签发的 Token 不包含 device_id（None） |
| AC-004 | `list_devices` 返回用户所有在线设备 |
| AC-005 | `revoke_device` 撤销指定设备，其他设备不受影响 |
| AC-006 | `revoke_all` 清空所有设备会话 |
| AC-007 | `update_device_active` 更新 last_active |
| AC-008 | `cleanup_expired_devices` 清理过期会话 |
| AC-009 | 设备数量超过 max_devices 时 LRU 淘汰 |
| AC-010 | `validate` 校验通过时更新设备活跃时间 |
| AC-011 | 现有 Token（无 device_id）校验行为不变 |
| AC-012 | `refresh` 更新会话 jti |
| AC-013 | `/sso/devices/:user_id` 端点返回设备列表 |
| AC-014 | `/sso/devices/revoke` 端点撤销设备 |
| AC-015 | `SsoService::login` 行为与 v0.6.5 一致 |
| AC-016 | 全 workspace `cargo test` 通过 |
| AC-017 | sz-pay 测试通过 |
| AC-018 | clippy 0 warning |

## 8. 范围外

- 设备指纹识别（浏览器指纹、设备硬件指纹）
- 异常登录检测（异地登录告警）
- 设备二次验证（新设备登录需短信/邮箱验证）
- 设备会话持久化到数据库（仅 Redis + 内存）
- 设备地理定位（IP → 地理位置）