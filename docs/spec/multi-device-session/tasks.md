# 多设备会话管理（P1）任务清单

> **对齐需求**：`docs/spec/multi-device-session/spec.md`（REQ-001 ~ REQ-029，AC-001 ~ AC-018）
> **对齐设计**：`docs/spec/multi-device-session/design.md`（任务分解 T0 ~ T19）
> **代码基线**：`packages/sz-rust-auth-facade/src/{refresh.rs, sso.rs, redis_store.rs, lib.rs, Cargo.toml}`
> **影响范围**：`sz-rust-auth-facade` 单 crate，不引入新依赖，不破坏 sz-pay 兼容性
> **关键约束**：全部 `async fn` 必须 `Send + 'static`；禁止 `std::fs`；不引入新依赖；clippy 0 warning
> **预估工作量**：~4.5 人日
> **关键路径**：T0 → T2 → T5 → T6 → T13 → T16 → T19

---

## 1. 设备会话领域模型与存储抽象

**写作说明**：本组建立多设备会话管理的领域模型基础与存储抽象，是所有后续任务的前置依赖。

### 1.1 定义设备信息与会话领域模型（T0）
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` 新增 `DeviceInfo` 结构体，包含 `device_id: String`、`device_type: Option<String>`、`user_agent: Option<String>`、`ip: Option<String>`、`device_name: Option<String>`，派生 `Debug, Clone, Serialize, Deserialize, PartialEq`，所有 `Option` 字段标注 `#[serde(default, skip_serializing_if = "Option::is_none")]`
- [ ] 为 `DeviceInfo` 实现 `new()`（自动生成 UUID v4 作为 device_id，其余字段 None）、`with_device_id(id: impl Into<String>)`（显式指定 device_id）、`Default`（委托 `new()`）
- [ ] 在 `refresh.rs` 新增 `DeviceSession` 结构体，包含 `device_id: String`、`device_info: DeviceInfo`、`jti: String`、`created_at: i64`、`last_active: i64`，派生 `Debug, Clone, Serialize, Deserialize, PartialEq`
- [ ] 在 `refresh.rs` 新增 `DeviceSessionConfig` 结构体，包含 `max_devices: usize`（默认 10），实现 `Default`；构造时将 `max_devices` clamp 到 `[1, 100]` 范围，超出时输出 `tracing::warn!` 日志
- **涉及文件**：`packages/sz-rust-auth-facade/src/refresh.rs`
- **验收条件**：AC-001（`DeviceInfo::new()` 生成 UUID v4 格式 device_id）；REQ-001、REQ-002、REQ-003、REQ-005
- **依赖任务**：无

### 1.2 定义设备会话存储抽象 trait（T2）
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` 新增 `#[async_trait::async_trait] pub trait DeviceSessionStore: Send + Sync`，包含 8 个异步方法：`register_session(user_id, device_id, device_info, jti) -> Result<()>`、`get_sessions(user_id) -> Result<Vec<DeviceSession>>`、`get_session(user_id, device_id) -> Result<Option<DeviceSession>>`、`revoke_session(user_id, device_id) -> Result<Option<String>>`、`update_last_active(user_id, device_id) -> Result<()>`、`update_session_jti(user_id, device_id, new_jti) -> Result<()>`、`cleanup_expired(user_id, ttl_secs) -> Result<Vec<String>>`、`clear_user_sessions(user_id) -> Result<Vec<String>>`，统一返回 `Result<T, RefreshTokenError>`
- **涉及文件**：`packages/sz-rust-auth-facade/src/refresh.rs`
- **验收条件**：REQ-004；trait 方法签名与现有 `RefreshTokenStore` 风格一致（返回 `Result<T, RefreshTokenError>`，使用 `async_trait` 宏）；满足 `Send + Sync`
- **依赖任务**：T0

---

## 2. SsoClaims 契约扩展（兼容性基础）

**写作说明**：在 JWT claims 中携带 device_id，是设备绑定能力的契约基础，必须保证旧 Token 反序列化兼容。

### 2.1 SsoClaims 新增 device_id 字段（T1）
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` 的 `SsoClaims` 结构体新增 `device_id: Option<String>` 字段，标注 `#[serde(default, skip_serializing_if = "Option::is_none")]`，保证旧 Token（无 device_id 字段）反序列化为 `None`
- [ ] 修改 `SsoClaims::access(...)` 和 `SsoClaims::refresh(...)` 构造器，补充 `device_id: None` 默认值
- [ ] 修改 `RefreshTokenIssuer::renew_access(old_claims)`，复制 `device_id: old_claims.device_id.clone()`，保证续期 Token 保留设备绑定
- **涉及文件**：`packages/sz-rust-auth-facade/src/refresh.rs`
- **验收条件**：AC-011（现有 Token 无 device_id 校验行为不变）；REQ-008、REQ-009；旧 Token JSON 反序列化 `device_id = None` 不报错
- **依赖任务**：T0

---

## 3. 设备会话内存存储实现

**写作说明**：提供测试与单进程场景的内存存储实现，无网络依赖，始终可用。

### 3.1 实现 MemoryDeviceSessionStore（T3）
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` 新增 `MemoryDeviceSessionStore`，内部使用 `Arc<parking_lot::RwLock<HashMap<(i64, String), DeviceSession>>>` 存储，实现 `new()` 和 `Default`
- [ ] 为 `MemoryDeviceSessionStore` 实现 `DeviceSessionStore` trait 全部 8 方法：`register_session`（upsert 语义，覆盖同 device_id 旧会话）、`get_sessions`（按 user_id 过滤）、`get_session`、`revoke_session`（remove 并返回 jti）、`update_last_active`、`update_session_jti`（更新 jti + last_active）、`cleanup_expired`（按 `last_active + ttl < now` 过滤删除）、`clear_user_sessions`（删除该 user 所有会话并返回 jti 列表）
- **涉及文件**：`packages/sz-rust-auth-facade/src/refresh.rs`
- **验收条件**：REQ-006；`register_session` 后 `get_session` 返回 `Some`；`revoke_session` 后 `get_session` 返回 `None`；`cleanup_expired` 后无过期会话；所有方法满足 `Send + Sync` + `async fn Send + 'static`
- **依赖任务**：T2

---

## 4. Token 签发设备绑定能力

**写作说明**：扩展 Token 签发器，支持签发携带 device_id 的双 Token，不破坏现有 `issue` 签名。

### 4.1 RefreshTokenIssuer 签发带设备 ID 的 Token（T4）
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` 提取私有方法 `issue_inner(user_id, username, device_id: Option<&str>) -> Result<TokenPair, RefreshTokenError>`，将现有 `issue` 内部逻辑迁入，构造 claims 时 `device_id: device_id.map(|s| s.to_string())`
- [ ] 将现有 `issue(user_id, username)` 重构为委托 `issue_inner(user_id, username, None)`，签名不变
- [ ] 新增 `issue_with_device(user_id, username, device_id) -> Result<TokenPair, RefreshTokenError>`，委托 `issue_inner(user_id, username, Some(device_id))`，标注 `#[tracing::instrument(skip(self), fields(user_id = user_id, device_id = device_id))]`
- [ ] 新增内部方法 `issue_with_device_and_jti(user_id, username, device_id) -> Result<(TokenPair, String), RefreshTokenError>`，返回 TokenPair 及 refresh_token 的 jti（避免 `login_with_device` 二次解码）
- **涉及文件**：`packages/sz-rust-auth-facade/src/refresh.rs`
- **验收条件**：REQ-010；`issue` 签名与行为不变（sz-pay 兼容）；`issue_with_device` 签发的 Token claims.device_id = Some
- **依赖任务**：T1

---

## 5. SsoService 多设备会话管理能力

**写作说明**：在服务编排层实现多设备会话管理的核心业务 API，是本功能的主体。

### 5.1 SsoService 注入设备存储与配置（T5）
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 的 `SsoService` 结构体新增 `device_store: Option<Arc<dyn DeviceSessionStore>>` 和 `device_config: DeviceSessionConfig` 字段
- [ ] 修改 `SsoService::new`（签名不变），内部初始化 `device_store = None`、`device_config = DeviceSessionConfig::default()`
- [ ] 新增 `with_device_store(store: Arc<dyn DeviceSessionStore>, config: DeviceSessionConfig) -> &mut Self` 链式配置方法
- **涉及文件**：`packages/sz-rust-auth-facade/src/sso.rs`
- **验收条件**：`SsoService::new` 签名不变（sz-pay 兼容）；未调用 `with_device_store` 时 `device_store = None`
- **依赖任务**：T2、T0

### 5.2 实现登录并绑定设备 login_with_device（T6）
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 新增 `login_with_device(username, password, device_info) -> Result<LoginResponse, RefreshTokenError>`，标注 `#[tracing::instrument(skip(self, password), fields(username = username, device_id = device_info.device_id))]`
- [ ] 实现流程：空串校验 → `user_auth.authenticate` → `issuer.issue_with_device_and_jti` → 若 `device_store.is_some()` 则执行 LRU 淘汰（按 `last_active` 升序排序，淘汰超 `max_devices` 的最旧设备，撤销 jti 加入黑名单）→ `register_session` → `tracing::info!` 设备注册日志
- [ ] 同设备重复登录采用覆盖语义（不撤销旧 refresh_token，仅覆盖会话记录）
- **涉及文件**：`packages/sz-rust-auth-facade/src/sso.rs`
- **验收条件**：AC-002（签发的 Token 包含 device_id）、AC-009（设备数超 max_devices 时 LRU 淘汰）；REQ-010、REQ-012、REQ-027、REQ-028
- **依赖任务**：T5、T4、T3

### 5.3 实现设备查询/撤销/心跳/清理 API（T7）
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 新增 `list_devices(user_id) -> Result<Vec<DeviceSession>, RefreshTokenError>`，委托 `device_store.get_sessions`，未配置 store 时返回 `Err(InvalidConfig("device session store not configured"))`
- [ ] 新增 `revoke_device(user_id, device_id) -> Result<(), RefreshTokenError>`：查会话取 jti → 黑名单写入（TTL = refresh_token_ttl）→ 删会话；不递增版本号；设备不存在返回 `Ok(())`（幂等）；输出 `tracing::info!` 撤销日志（reason="manual"）
- [ ] 新增 `update_device_active(user_id, device_id) -> Result<(), RefreshTokenError>`，委托 `device_store.update_last_active`
- [ ] 新增 `cleanup_expired_devices(user_id, ttl_secs) -> Result<usize, RefreshTokenError>`，委托 `device_store.cleanup_expired`，对返回 jti 批量加黑名单（TTL = ttl_secs），返回清理数量
- **涉及文件**：`packages/sz-rust-auth-facade/src/sso.rs`
- **验收条件**：AC-004（list_devices 返回所有在线设备）、AC-005（revoke_device 仅撤销目标设备）、AC-007（update_device_active 更新 last_active）、AC-008（cleanup_expired_devices 清理过期会话）；REQ-013~017、REQ-026
- **依赖任务**：T5

### 5.4 增强现有 validate/refresh/revoke_all 设备联动（T8）
- [ ] 修改 `SsoService::validate`（签名不变）：校验通过后，若 `claims.device_id.is_some() && self.device_store.is_some() && claims.user_id.is_some()`，best-effort 调用 `update_last_active`（失败仅 `tracing::warn!` 不中断校验）
- [ ] 修改 `SsoService::validate_with_renewal`（签名不变）：同 `validate` 的 best-effort 活跃更新；续期签发的新 Token 复制 `device_id`
- [ ] 修改 `SsoService::refresh`（签名不变）：轮换成功后，若新 Token 含 device_id，best-effort 调用 `update_session_jti` 更新会话 jti 与 last_active（失败仅 warn）
- [ ] 修改 `SsoService::revoke_all`（签名不变）：递增版本号后，若 `device_store.is_some()`，best-effort 调用 `clear_user_sessions` 并对返回 jti 批量加黑名单
- [ ] `SsoService::login` 行为完全不变（签发的 Token device_id = None，不注册设备会话）
- **涉及文件**：`packages/sz-rust-auth-facade/src/sso.rs`
- **验收条件**：AC-010（validate 通过时更新设备活跃）、AC-011（无 device_id 的 Token 校验行为不变）、AC-012（refresh 更新会话 jti）、AC-015（login 行为与 v0.6.5 一致）；REQ-015、REQ-018、REQ-019、REQ-020
- **依赖任务**：T5

---

## 6. Redis 设备会话存储实现

**写作说明**：提供生产级 Redis 持久化存储实现，在 `redis-store` feature gate 下启用。

### 6.1 RedisConfig 扩展设备会话 key 前缀（T9）
- [x] 在 `packages/sz-rust-auth-facade/src/redis_store.rs` 的 `RedisConfig` 新增 `key_prefix_sessions: String` 字段，标注 `#[serde(default = "default_key_prefix_sessions")]`，默认值 `"sso:sessions"`
- [x] 新增私有函数 `default_key_prefix_sessions() -> String` 返回 `"sso:sessions".to_string()`
- [x] 确认 `Debug` 实现对新字段脱敏处理（若现有 Debug 手动实现需补充）
- **涉及文件**：`packages/sz-rust-auth-facade/src/redis_store.rs`
- **验收条件**：REQ-007；旧配置反序列化时 `key_prefix_sessions` 取默认值（serde 兼容）
- **依赖任务**：无

### 6.2 实现 RedisDeviceSessionStore（T10）
- [x] 在 `packages/sz-rust-auth-facade/src/redis_store.rs` 新增 `RedisDeviceSessionStore`（feature gate `redis-store`），持有 `ConnectionManager` 和 `RedisConfig`，实现 `new(config) -> Result<Self, RefreshTokenError>`（复用现有连接建立逻辑）
- [x] 实现 `DeviceSessionStore` trait 全部 8 方法，Redis 命令映射：`register_session`→HSET、`get_sessions`→HGETALL、`get_session`→HGET、`revoke_session`→HGET+HDEL、`update_last_active`→HGET+HSET（读-改-写）、`update_session_jti`→HGET+HSET、`cleanup_expired`→HGETALL+HDEL(pipeline)、`clear_user_sessions`→HGETALL+DEL
- [x] key 格式：`{key_prefix_sessions}:{user_id}`，field 为 `{device_id}`，value 为 `serde_json(DeviceSession)`
- [x] 所有命令统一 `tokio::time::timeout(config.command_timeout, ...)`，错误映射：超时→`ServiceUnavailable`、Redis 错误→`Cache(format!(...))`、JSON 反序列化失败→`Cache(format!(...))`
- **涉及文件**：`packages/sz-rust-auth-facade/src/redis_store.rs`
- **验收条件**：REQ-007；与 `RedisRefreshTokenStore` / `RedisTokenBlacklist` 模式一致（timeout + 错误映射 + ConnectionManager 共享）
- **依赖任务**：T2、T9

### 6.3 新增 Redis 三元组工厂方法（T11）
- [x] 在 `packages/sz-rust-auth-facade/src/redis_store.rs` 新增 `create_redis_stores_with_devices(config) -> Result<(Arc<dyn RefreshTokenStore>, Arc<dyn TokenBlacklist>, Arc<dyn DeviceSessionStore>), RefreshTokenError>`（feature gate `redis-store`），共享 ConnectionManager
- [x] 保留现有 `create_redis_stores` 2 元组工厂不变（兼容性）
- **涉及文件**：`packages/sz-rust-auth-facade/src/redis_store.rs`
- **验收条件**：现有 `create_redis_stores` 签名不变；新工厂返回的三元组共享同一连接池
- **依赖任务**：T10

---

## 7. 模块导出与 axum HTTP 端点

**写作说明**：确认类型导出并扩展 HTTP 端点，对外暴露多设备会话管理能力。

### 7.1 确认 lib.rs 模块导出（T12）
- [ ] 确认 `packages/sz-rust-auth-facade/src/lib.rs` 中 `refresh` / `sso` / `redis_store` 模块已导出新类型（`DeviceInfo`、`DeviceSession`、`DeviceSessionConfig`、`DeviceSessionStore`、`MemoryDeviceSessionStore`、`RedisDeviceSessionStore`），无需新增 mod 声明（类型在现有 mod 内），仅需确认 `pub use` 覆盖
- **涉及文件**：`packages/sz-rust-auth-facade/src/lib.rs`
- **验收条件**：外部 crate 可 `use sz_rust_auth_facade::DeviceInfo` 等访问新类型
- **依赖任务**：T0~T11

### 7.2 axum 端点扩展设备管理路由（T13）
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 的 `LoginRequest` 新增 `device_info: Option<DeviceInfo>` 字段（`#[serde(default)]`），`login` handler 分支：有 device_info 调用 `login_with_device`，无则调用 `login`
- [ ] 新增 `GET /sso/devices/:user_id` 端点（`list_devices` handler），返回 `DeviceListResponse { devices, count }`
- [ ] 新增 `POST /sso/devices/revoke` 端点（`revoke_device` handler），请求体 `DeviceRevokeRequest { user_id, device_id }`，返回 `DeviceRevokeResponse { revoked: true }`
- [ ] 新增 `POST /sso/devices/heartbeat` 端点（`heartbeat` handler），请求体 `DeviceHeartbeatRequest { user_id, device_id }`，返回 `DeviceHeartbeatResponse { updated: true }`
- [ ] 在 `sso_routes()` 链式追加 3 条 `.route()`，保留现有 5 路由不变
- **涉及文件**：`packages/sz-rust-auth-facade/src/sso.rs`
- **验收条件**：AC-013（`/sso/devices/:user_id` 返回设备列表）、AC-014（`/sso/devices/revoke` 撤销设备）；REQ-021、REQ-022、REQ-023、REQ-024；现有端点行为不变
- **依赖任务**：T6、T7

---

## 8. 单元测试

**写作说明**：覆盖领域模型、内存存储与服务层核心逻辑，无外部依赖。

### 8.1 领域模型与内存存储单元测试（T14）
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` tests 新增用例：`test_device_info_new_generates_uuid`（验证 UUID v4 格式）、`test_device_info_with_device_id`、`test_device_info_serde_skip_none`（Option None 时 JSON 不输出）、`test_sso_claims_device_id_default_none`（旧 Token 反序列化 device_id=None）、`test_sso_claims_device_id_roundtrip`
- [ ] 新增 `MemoryDeviceSessionStore` CRUD 测试：`test_memory_store_register_get_revoke`、`test_memory_store_cleanup_expired`、`test_memory_store_clear_user_sessions`、`test_memory_store_update_session_jti`
- **涉及文件**：`packages/sz-rust-auth-facade/src/refresh.rs`
- **验收条件**：AC-001、AC-003、AC-011；REQ-002；全部用例通过
- **依赖任务**：T0~T4

### 8.2 SsoService 设备 API 单元测试（T15）
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` tests 新增用例：`test_login_with_device_token_has_device_id`、`test_login_token_no_device_id`、`test_list_devices_returns_all`、`test_revoke_device_only_affects_target`、`test_revoke_device_no_version_increment`、`test_revoke_all_clears_device_sessions`、`test_update_device_active_updates_last_active`、`test_cleanup_expired_devices`
- [ ] 新增 LRU 淘汰测试：`test_lru_eviction_on_max_devices`、`test_lru_eviction_max_devices_1`（边界）
- [ ] 新增联动测试：`test_validate_updates_device_active`、`test_validate_no_device_id_no_update`、`test_refresh_updates_session_jti`、`test_same_device_relogin_overwrites_session`、`test_device_methods_without_store_return_err`
- **涉及文件**：`packages/sz-rust-auth-facade/src/sso.rs`
- **验收条件**：AC-002~AC-012 全覆盖；全部用例通过
- **依赖任务**：T6~T8

---

## 9. 集成测试与端点测试

**写作说明**：验证 HTTP 端点与 Redis 存储的端到端正确性。

### 9.1 axum 端点测试（T16）
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` tests（feature `axum`）新增用例：`test_login_endpoint_with_device_info`（POST /sso/login 带 device_info 返回含 device_id 的 Token）、`test_login_endpoint_without_device_info_backward_compat`（不带 device_info 行为不变）、`test_devices_endpoint_returns_list`、`test_devices_revoke_endpoint`、`test_devices_heartbeat_endpoint`
- **涉及文件**：`packages/sz-rust-auth-facade/src/sso.rs`
- **验收条件**：AC-013、AC-014、AC-015；全部用例通过
- **依赖任务**：T13

### 9.2 Redis 集成测试（T17）
- [x] 在 `packages/sz-rust-auth-facade/src/redis_store.rs` tests（feature `redis-store`）新增用例：`test_redis_device_store_register_get`、`test_redis_device_store_get_sessions`、`test_redis_device_store_revoke`、`test_redis_device_store_cleanup_expired`、`test_redis_device_store_clear_user`
- [x] 使用 `#[cfg(test)]` + 环境变量 `REDIS_URL` 跳过策略，无 Redis 时标记 `#[ignore]`
- **涉及文件**：`packages/sz-rust-auth-facade/src/redis_store.rs`
- **验收条件**：有 Redis 实例时全部用例通过；无 Redis 时 `#[ignore]` 跳过
- **依赖任务**：T10、T11

---

## 10. 性能基准与全 workspace 验证

**写作说明**：性能达标验证与最终质量门禁，确保不破坏现有功能。

### 10.1 性能基准（T18）
- [ ] 在 `packages/sz-rust-auth-facade/benches/sso_bench.rs` 新增基准：`list_devices`（Memory, 10 devices，目标 < 1μs）、`revoke_device`（Memory，目标 < 5μs）、`login_with_device`（Memory，与 `login` 差异 < 5μs）、`validate` 无 device_store（与 v0.6.5 一致，零开销）
- **涉及文件**：`packages/sz-rust-auth-facade/benches/sso_bench.rs`
- **验收条件**：性能基准达标（list_devices < 1μs、revoke_device < 5μs、validate 无 store 零开销）
- **依赖任务**：T3、T6、T7

### 10.2 全 workspace 验证（T19）
- [ ] 执行 `cargo test --workspace` 全通过（AC-016）
- [ ] 执行 `cargo test --workspace --features axum` 全通过
- [ ] 执行 `cargo test --workspace --features redis-store` 全通过（需 Redis 实例）
- [ ] 执行 `cargo clippy --workspace --all-features -- -D warnings` 0 warning（AC-018）
- [ ] 执行 sz-pay 测试通过（AC-017），确认 sz-pay 无需任何代码改动即可升级
- **涉及文件**：全 workspace + `E:\vue\test\sz-pay`
- **验收条件**：AC-016（全 workspace cargo test 通过）、AC-017（sz-pay 测试通过）、AC-018（clippy 0 warning）
- **依赖任务**：T0~T18

---

## 任务依赖图

```
T0 ──┬──> T1 ──> T4 ──┐
     ├──> T2 ──┬──> T3 ──┐
     │         ├──> T5 ──┼──> T6 ──┐
     │         │         ├──> T7 ──┤
     │         │         ├──> T8 ──┤
     │         └──> T10 ─┤         ├──> T13 ──> T16
     └──> T9 ──> T10     │         │
                         │         ├──> T14
                         ├──> T11 ─┤
                         │         ├──> T15
                         │         ├──> T17
                         │         ├──> T18
                         └──> T12 ─┴──> T19
```

**关键路径**：T0 → T2 → T5 → T6 → T13 → T16 → T19

---

## 验收标准对照表

| AC ID | 验收条件 | 覆盖任务 |
|-------|---------|---------|
| AC-001 | `DeviceInfo::new()` 自动生成 UUID v4 device_id | T0、T14 |
| AC-002 | `login_with_device` 签发的 Token 包含 device_id | T6、T15 |
| AC-003 | `login` 签发的 Token 不包含 device_id（None） | T14、T15 |
| AC-004 | `list_devices` 返回用户所有在线设备 | T7、T15 |
| AC-005 | `revoke_device` 撤销指定设备，其他设备不受影响 | T7、T15 |
| AC-006 | `revoke_all` 清空所有设备会话 | T8、T15 |
| AC-007 | `update_device_active` 更新 last_active | T7、T15 |
| AC-008 | `cleanup_expired_devices` 清理过期会话 | T7、T15 |
| AC-009 | 设备数量超过 max_devices 时 LRU 淘汰 | T6、T15 |
| AC-010 | `validate` 校验通过时更新设备活跃时间 | T8、T15 |
| AC-011 | 现有 Token（无 device_id）校验行为不变 | T1、T8、T14、T15 |
| AC-012 | `refresh` 更新会话 jti | T8、T15 |
| AC-013 | `/sso/devices/:user_id` 端点返回设备列表 | T13、T16 |
| AC-014 | `/sso/devices/revoke` 端点撤销设备 | T13、T16 |
| AC-015 | `SsoService::login` 行为与 v0.6.5 一致 | T8、T16 |
| AC-016 | 全 workspace `cargo test` 通过 | T19 |
| AC-017 | sz-pay 测试通过 | T19 |
| AC-018 | clippy 0 warning | T19 |