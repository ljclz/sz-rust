# Token 自动续期（v0.6.5）实现任务清单

> 对齐 `spec.md` v0.6.5 与 `design.md` §2.10 任务分解（T0-T10）。
> 遵循 sz-rust 铁律：所有 `async fn` 必须 `Send + 'static`、禁止 `std::fs`、敏感字段 `#[serde(skip_serializing)]`、不引入新依赖。
> 关键路径：T0 → T2 → T4 → T5 → T8 → T9 → T10（7 个任务，总预估 7.5h）。

---

## 1. 续期配置与核心签发能力

**写作说明**：本组对应设计文档 T0 / T1 / T2，构建续期功能的配置载体、结果载体与签发原语。三者均无 IO，是后续编排逻辑的基础。

### 1.1 [T0] 实现 RenewalConfig 配置结构与判定算法
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` 新增 `RenewalConfig` 结构体，包含 `enabled: bool`、`renewal_threshold: chrono::Duration`、`renewal_ratio: f64`、`access_token_ttl: chrono::Duration` 四个公开字段，派生 `Debug, Clone, serde::Serialize, serde::Deserialize`
- [ ] 为 `RenewalConfig` 实现 `Default`，返回 `enabled=true`、`renewal_threshold=300s`、`renewal_ratio=0.2`、`access_token_ttl=900s`（满足 REQ-001 / AC-001）
- [ ] 实现 `RenewalConfig::should_renew(&self, remaining_ttl: i64) -> bool`，算法为：`enabled=false` 返回 false；`threshold_secs==0` 时返回 `remaining_ttl > 0`（满足 REQ-003）；否则 `remaining_ttl < max(threshold_secs, access_token_ttl * ratio)`（满足 REQ-005/006）
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 顶部 `pub use` 重导出 `RenewalConfig`，保持与现有 `RefreshTokenConfig` 导出风格一致
- [ ] **验收**：`RenewalConfig::default()` 字段值符合 AC-001；`should_renew` 在 threshold=0 / ratio=0.0 / ratio=1.0 / TTL 恰好等于阈值 四种边界下行为符合 REQ-003/004 与设计文档 §2.4.1 边界表

### 1.2 [T1] 定义 RenewedToken 续期结果载体
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` 新增 `RenewedToken` 结构体，包含 `access_token: String` 与 `expires_at: i64` 两个公开字段，派生 `Debug, Clone`
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 重导出 `RenewedToken`
- [ ] **验收**：类型可被 `SsoService::validate_with_renewal` 返回值引用（`Option<RenewedToken>`），避免 exp 重复计算（设计文档 §2.2.2.6 优化决策）
- [ ] **依赖**：T0

### 1.3 [T2] 实现 RefreshTokenIssuer::renew_access 续期签发方法
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` 的 `RefreshTokenIssuer` impl 块新增 `pub fn renew_access(&self, old_claims: &SsoClaims) -> Result<(String, i64), RefreshTokenError>`，声明为同步方法（非 async，设计文档 §2.1.3.2 决策 5）
- [ ] 方法内部：`now = chrono::Utc::now().timestamp()`；`new_exp = now + self.config.access_token_ttl.num_seconds()`；`new_jti = uuid::Uuid::new_v4().to_string()`
- [ ] 手工构造 `SsoClaims`，从 `old_claims` 复制 `sub / iss / user_id / ver / roles / permissions`，更新 `exp=new_exp / iat=now / jti=new_jti / token_type="access"`（**禁止调用 `SsoClaims::access`**，否则丢失 roles/permissions，设计文档 §1.2.4 关键差异）
- [ ] 调用 `self.codec.encode(&new_claims)?` 签发新 token，返回 `Ok((new_token, new_exp))`
- [ ] **禁止**调用 `self.store.get_version`（REQ-012）、`self.blacklist.revoke`（REQ-011）、签发 refreshToken（REQ-010）
- [ ] **验收**：新 token 的 `ver / user_id / roles / permissions / sub / iss` 与原 token 一致（REQ-009 / AC-007）；`exp ≈ now + access_token_ttl`（AC-006）；`jti` 为新 UUID v4 且与原 jti 不同；不触发 store/blacklist 调用（AC-008 / AC-009）
- [ ] **依赖**：T0

---

## 2. SsoService 续期集成

**写作说明**：本组对应设计文档 T3 / T4，将续期配置注入 SsoService，并编排"校验 → TTL 判定 → 续期签发"主流程。`validate` 方法签名与行为保持不变（REQ-015 / AC-013）。

### 2.1 [T3] SsoService 持有续期配置与链式 setter
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 的 `SsoService` 结构体新增 `renewal_config: RenewalConfig` 私有字段
- [ ] 修改 `SsoService::new` 构造函数，**签名不变**（4 参数：issuer / verifier / revoker / user_auth），内部初始化 `renewal_config: RenewalConfig::default()`（满足 REQ-013）
- [ ] 新增 `pub fn with_renewal_config(&mut self, config: RenewalConfig) -> &mut Self`，替换 `self.renewal_config` 并返回 `&mut self` 支持链式调用（满足 REQ-014）
- [ ] **验收**：`SsoService::new(...)` 编译通过且 `renewal_config` 为 default；`with_renewal_config` 链式调用可编译；现有 `validate` 方法签名与行为零变更（AC-013）
- [ ] **依赖**：T0

### 2.2 [T4] 实现 SsoService::validate_with_renewal 主流程
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 的 `SsoService` impl 块新增 `#[tracing::instrument(skip(self, access_token))] pub async fn validate_with_renewal(&self, access_token: &str) -> Result<(SsoClaims, Option<RenewedToken>), RefreshTokenError>`
- [ ] 流程：1) `let claims = self.verifier.verify_access(access_token).await?;`（复用校验链，不绕过黑名单/版本/过期，满足 REQ-021/022/023 / AC-005）；2) `if !self.renewal_config.enabled { return Ok((claims, None)); }`（REQ-007 / AC-004）；3) `remaining_ttl = claims.exp - now`；4) `if self.renewal_config.should_renew(remaining_ttl)` 触发续期
- [ ] 续期触发时调用 `self.issuer.renew_access(&claims)?`，构造 `RenewedToken { access_token, expires_at: new_exp }`，输出 `tracing::debug!(user_id, old_jti, new_jti, old_exp, new_exp, "access token renewed")`（满足 REQ-025/026，**禁止输出 token 明文**），返回 `Ok((claims, Some(renewed)))`
- [ ] 不续期时返回 `Ok((claims, None))`（REQ-006）
- [ ] 确保 `async fn` 满足 `Send + 'static`：唯一 `.await` 在 `verify_access`（已满足）
- [ ] **验收**：TTL < 阈值返回 `Some`（AC-002）；TTL >= 阈值返回 `None`（AC-003）；`enabled=false` 始终 `None`（AC-004）；校验失败返回 `Err` 且不签发（AC-005 / REQ-008）；返回的 claims 与 `validate` 一致（REQ-009）
- [ ] **依赖**：T1、T2、T3

---

## 3. axum HTTP 端点续期增强

**写作说明**：本组对应设计文档 T5，增强 `/sso/validate` 端点响应，新增 `new_access_token` 与 `new_access_expires_at` 字段，向后兼容（REQ-016/017/018）。

### 3.1 [T5] ValidateResponse 增强 + validate handler 改用续期方法
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 的 `ValidateResponse` 结构体新增 `new_access_token: Option<String>` 与 `new_access_expires_at: Option<i64>` 两个字段，**不使用 `skip_serializing_if`**（设计文档 §2.2.2.5 方案 B），不续期时序列化为 `null`（满足 REQ-017）
- [ ] 修改 `validate` handler（`sso.rs:274`），将 `sso.validate(&params.token)` 改为 `sso.validate_with_renewal(&params.token).await`
- [ ] `Ok((claims, renewed))` 分支：`valid=true`、`user_id=claims.user_id.unwrap_or(0)`、`expires_at=claims.exp`；`new_access_token=renewed.as_ref().map(|r| r.access_token.clone())`；`new_access_expires_at=renewed.map(|r| r.expires_at)`（避免重复计算 exp，复用 `RenewedToken.expires_at`）
- [ ] `Err(err)` 分支保持现有 `error_response(err)` 不变
- [ ] 保留响应头 `Cache-Control: no-store` + `Pragma: no-cache`（设计文档 §2.5.3，复用现有 `success_response`）
- [ ] **验收**：续期时 JSON `data` 含非空 `new_access_token` 与 `new_access_expires_at`（AC-010 / REQ-016）；不续期时两字段为 `null`（AC-011 / REQ-017）；旧客户端忽略新字段行为不变（AC-013 / REQ-018）；HTTP 状态码 200/401 不变
- [ ] **依赖**：T4

---

## 4. 中间件续期响应头注入

**写作说明**：本组对应设计文档 T6，为 `sso_middleware` 增加本地续期能力，通过响应头 `X-Renewed-Access-Token` / `X-Renewed-Expires-At` 注入新 token。默认 `renewal_config=None` 保证向后兼容（REQ-019/020）。

### 4.1 [T6] SsoMiddlewareConfig 续期配置 + 中间件响应头注入
- [ ] 在 `packages/sz-rust-middleware-facade/src/sso_middleware.rs` 的 `SsoMiddlewareConfig` 结构体新增 `renewal_config: Option<RenewalConfig>` 字段
- [ ] 新增 `SsoMiddlewareConfig::local_with_renewal` 方法，参数为现有 5 参数 + `renewal_config: Option<RenewalConfig>`，内部构造 `Self` 并传入 `renewal_config`
- [ ] 新增 `SsoMiddlewareConfig::local_memory_with_renewal` 方法，签名同理扩展
- [ ] 修改现有 `local` / `local_memory` 方法，**签名不变**，内部委托 `local_with_renewal(..., None)` / `local_memory_with_renewal(..., None)`（设计文档 §2.2.2.7，保证 sz-pay 零改动）
- [ ] 修改 `sso_middleware` 函数：校验通过后，在 `next.run(req).await` **之前**判定续期并签发（`config.renewal_config.as_ref()` 存在且 `enabled` 且 `should_renew(remaining_ttl)`），构造 `SsoClaims`（复用 `old_claims` 的 `sub/iss/user_id/ver/roles/permissions`，更新 `exp/iat/jti/token_type`），调用 `config.codec.encode(&new_claims)`（**不构造 `RefreshTokenIssuer`**，设计文档 §2.1.3.3）
- [ ] 在 `next.run(req).await` **之后**通过 `response.headers_mut().insert("X-Renewed-Access-Token", new_token.parse().unwrap())` 与 `X-Renewed-Expires-At` 注入响应头（响应对象此时才存在）
- [ ] `encode` 失败时静默降级（不注入响应头，不中断请求，设计文档 §2.1.3.3 关键设计点）
- [ ] `renewal_config=None` 或 `enabled=false` 时行为与 v0.6.4 完全一致，不注入任何续期头（REQ-020）
- [ ] **验收**：续期触发时响应含 `X-Renewed-Access-Token` 与 `X-Renewed-Expires-At`（AC-012 / REQ-019）；不续期或禁用时无 `X-Renewed-*` 头（REQ-020）；续期不影响响应体（REQ-019）；`local` / `local_memory` 现有调用方零改动
- [ ] **依赖**：T0

---

## 5. 单元测试

**写作说明**：本组对应设计文档 T7，覆盖 `RenewalConfig`（11 用例）、`renew_access`（11 用例）、`validate_with_renewal`（9 用例），共 31 个单元测试用例，无需 Redis / 数据库。

### 5.1 [T7] 编写续期核心逻辑单元测试
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` 的 `#[cfg(test)] mod tests` 新增 `RenewalConfig` 测试：`test_renewal_config_default`（AC-001）、`test_should_renew_disabled`（AC-004）、`test_should_renew_threshold_zero`（REQ-003）、`test_should_renew_threshold_zero_expired`（REQ-003 边界）、`test_should_renew_ratio_zero`（REQ-004）、`test_should_renew_ratio_one`（REQ-004）、`test_should_renew_below_threshold`（AC-002）、`test_should_renew_above_threshold`（AC-003）、`test_should_renew_at_exact_threshold`（边界严格小于）、`test_should_renew_ratio_dominant`、`test_should_renew_threshold_dominant`（阈值计算）
- [ ] 在 `refresh.rs` tests mod 新增 `renew_access` 测试：`test_renew_access_preserves_user_id`、`test_renew_access_preserves_ver`（AC-007/008）、`test_renew_access_preserves_roles_permissions`（REQ-009）、`test_renew_access_new_jti`（UUID v4 唯一）、`test_renew_access_new_exp`（AC-006）、`test_renew_access_token_type_access`、`test_renew_access_no_refresh_token`（REQ-010）、`test_renew_access_no_store_call`（mock 计数，AC-008）、`test_renew_access_no_blacklist_call`（AC-009/REQ-011）、`test_renew_access_new_token_valid`（`verify_access(new_token)` 成功）、`test_renew_access_old_token_still_valid`（REQ-011）
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 的 tests mod 新增 `validate_with_renewal` 测试：`test_validate_with_renewal_triggers`（AC-002）、`test_validate_with_renewal_no_trigger`（AC-003）、`test_validate_with_renewal_disabled`（AC-004）、`test_validate_with_renewal_invalid_token`（AC-005）、`test_validate_with_renewal_expired_token`（REQ-023）、`test_validate_with_renewal_revoked_token`（REQ-021）、`test_validate_with_renewal_version_mismatch`（REQ-022）、`test_validate_with_renewal_preserves_claims`（REQ-009）、`test_validate_unchanged`（AC-013/REQ-015）
- [ ] 使用现有测试基础设施（mock store / blacklist），不引入新依赖
- [ ] **验收**：31 个用例全部通过；覆盖设计文档 §2.9.1 全部用例表；`cargo test -p sz-rust-auth-facade` 绿色
- [ ] **依赖**：T0、T2、T4

---

## 6. 集成测试

**写作说明**：本组对应设计文档 T8，覆盖 axum `/sso/validate` 端点续期响应（4 用例）与中间件续期响应头（4 用例），共 8 个集成测试用例。

### 6.1 [T8] 编写 axum 端点与中间件集成测试
- [ ] 在 `packages/sz-rust-auth-facade/src/sso.rs` 的 axum 测试 mod 新增端点集成测试：`test_validate_endpoint_renewal_response`（续期时 JSON 含非空 `new_access_token`，AC-010）、`test_validate_endpoint_no_renewal_response`（不续期时 `null`，AC-011）、`test_validate_endpoint_backward_compat`（旧客户端忽略新字段，AC-013）、`test_validate_endpoint_cache_control`（响应含 `Cache-Control: no-store`）
- [ ] 在 `packages/sz-rust-middleware-facade/src/sso_middleware.rs` 的测试 mod 新增中间件集成测试：`test_middleware_renewal_header`（续期时响应含 `X-Renewed-Access-Token`，AC-012）、`test_middleware_no_renewal_header`（不续期时无 `X-Renewed-*` 头，REQ-020）、`test_middleware_renewal_disabled`（`renewal_config=None` 时无续期头，REQ-020）、`test_middleware_renewal_preserves_body`（续期不影响响应体，REQ-019）
- [ ] 使用 `axum::test` 或现有测试辅助构造 `Router` + mock store/blacklist
- [ ] **验收**：8 个用例全部通过；覆盖设计文档 §2.9.2 / §2.9.3 全部用例表；`cargo test -p sz-rust-auth-facade --features axum` 与 `cargo test -p sz-rust-middleware-facade` 绿色
- [ ] **依赖**：T5、T6

---

## 7. 边界测试与性能基准

**写作说明**：本组对应设计文档 T9，覆盖极端边界组合（threshold=0 / ratio=0.0 / ratio=1.0 / TTL 恰好等于阈值）与性能基准（不续期 < 100ns、续期 ≈ 856ns）。

### 7.1 [T9] 边界用例测试与性能基准
- [ ] 在 `packages/sz-rust-auth-facade/src/refresh.rs` tests mod 新增边界组合测试：threshold=0 + ratio=0.0、threshold=0 + ratio=1.0、ratio=1.0 + ttl=900（总是续期）、TTL 恰好等于阈值（严格小于，返回 false）、续期后立即再 `validate_with_renewal`（新 token TTL 充足不续期）、`revoke_all` 后续期 token（`VersionMismatch`）、并发续期同一 token（多次续期均有效，无 store 写入竞争）
- [ ] 在 `packages/sz-rust-auth-facade/benches/` 新增 criterion 性能基准：`validate_with_renewal_no_renew`（与 `validate` 差异 < 100ns）、`validate_with_renewal_with_renew`（额外开销 ≈ 856ns，一次 encode）、`should_renew_pure_calc`（< 10ns）
- [ ] 复用现有 criterion 依赖（不引入新依赖，spec.md §6.1）；若 workspace 无 criterion，则在单元测试中用 `std::time::Instant` 粗粒断言
- [ ] **验收**：边界用例全部通过，符合设计文档 §2.9.4 边界表与 §2.4.1 阈值计算表；性能基准满足 spec.md §5.1（不续期 < 100ns、续期 ≈ encode 基准）
- [ ] **依赖**：T7、T8

---

## 8. 全量门禁与验收

**写作说明**：本组对应设计文档 T10，执行全 workspace 测试、clippy 零警告、sz-pay 5139 测试，确认 semver 兼容与 sz-pay 零改动。

### 8.1 [T10] 执行全量门禁与兼容性验证
- [ ] 在 workspace 根目录执行 `cargo test --workspace --all-features`，确认全 workspace 测试通过（AC-014）
- [ ] 执行 `cargo clippy --workspace --all-features -- -D warnings`，确认 0 warning（AC-016）
- [ ] 执行 `cargo fmt --all -- --check`，确认格式无变更
- [ ] 在 `E:\vue\test\sz-pay` 执行 `cargo test`，确认 sz-pay 5139 测试全部通过（AC-015），验证 `SsoService::validate` 签名不变、`SsoMiddlewareConfig::local` 签名不变、`ValidateResponse` 新字段被 JSON 解析器忽略（设计文档 §2.8.2）
- [ ] 检查 `packages/sz-rust-auth-facade/Cargo.toml` 版本号从 0.6.4 bump 至 0.6.5（minor bump，semver 兼容，设计文档 §2.8.3）
- [ ] 主动扫描同类问题：确认未引入新 crate 依赖（spec.md §6.1）、未使用 `std::fs`、所有新增 `async fn` 满足 `Send + 'static`、敏感字段脱敏约束保持
- [ ] **验收**：AC-014 / AC-015 / AC-016 全部通过；sz-pay 零改动零编译错误；版本号 0.6.5；spec.md 全部 26 条 REQ + 16 条 AC 覆盖
- [ ] **依赖**：T9

---

## 任务依赖与并行机会

**依赖图（设计文档 §2.10.1）：**

```
T0 ──┬──> T1 ──> T4
     ├──> T2 ──> T4
     └──> T3 ──> T4
T4 ──> T5 ──> T8
T0 ──> T6 ──> T8
T0/T2/T4 ──> T7
T7/T8 ──> T9 ──> T10
```

**关键路径**：T0 → T2 → T4 → T5 → T8 → T9 → T10（7 个任务）

**并行机会（设计文档 §2.10.4）：**
- T1 与 T2 可并行（均仅依赖 T0）
- T3 与 T2 可并行（均仅依赖 T0）
- T6 与 T4 / T5 可并行（T6 仅依赖 T0）
- T7 与 T8 部分可并行（T7 依赖 T4，T8 依赖 T5/T6）

**总预估**：7.5h