# tasks.md — 远程校验连接池优化（RemoteValidateConfig）

> **基于**：[spec.md](./spec.md) · [design.md](./design.md)
> **目标 crate**：`sz-rust-middleware-facade`（`src/sso_middleware.rs`，feature `remote-validate`）
> **版本**：v0.6.3 → v0.6.4（semver minor，仅新增 API，不破坏现有签名）
> **任务规模**：8 个子任务（T0–T7），小优化

---

## 1. 错误类型与配置载体扩展

### 1.1 RefreshTokenError 新增 InvalidConfig 变体（T0）
- [ ] 在 `sz-rust-auth-facade/src/refresh.rs` 的 `RefreshTokenError` 枚举末尾新增 `InvalidConfig(String)` 变体，标注 `#[error("invalid config: {0}")]`，保持 `#[derive(Debug, thiserror::Error)]` 派生不变（NFR-3.1）
- [ ] 验证现有错误处理代码编译通过，`InvalidConfig` 不与 `ServiceUnavailable` 语义混淆

### 1.2 PoolConfig 结构体与 Default 实现（T1）
- [ ] 在 `sso_middleware.rs` 的 `#[cfg(feature = "remote-validate")]` 下新增 `pub struct PoolConfig`，字段：`pool_max_idle_per_host: usize` / `pool_idle_timeout: Option<Duration>` / `tcp_keepalive: Option<Duration>` / `tcp_nodelay: bool`，派生 `#[derive(Debug, Clone)]`
- [ ] 为 `PoolConfig` 实现 `Default`，默认值 `32 / Some(90s) / None / true`（对齐 spec §2）

---

## 2. RemoteValidateConfig 构造 API 扩展

### 2.1 新增失败安全构造、回退、外部注入与 Builder（T2）
- [ ] 在 `RemoteValidateConfig` 新增 `pub pool_config: PoolConfig` 字段；实现自定义 `Debug`，仅输出 `endpoint / timeout / allow_all_action / pool_config`，不输出 `client`（NFR-2.1）
- [ ] 实现 `new_checked(endpoint, timeout, allow_all_action) -> Result<Self, RefreshTokenError>`：空 endpoint → `Err(InvalidConfig)`；`ClientBuilder::build` 失败 → `Err(ServiceUnavailable)`；成功时透传四项池参数（`pool_idle_timeout=None` / `tcp_keepalive=None` 时不调用对应方法，FR-5.2 / FR-5.3）
- [ ] 实现 `new_or_default(...)`：`new_checked` 失败时 `tracing::warn!`（含原因，不含敏感字段）+ 回退 `reqwest::Client::new()`（FR-3.1）
- [ ] 实现 `from_client(endpoint, timeout, allow_all_action, client) -> Self`：直接持有外部 Client，跳过内部 `ClientBuilder`，不覆盖其配置（FR-6.1 / FR-6.2）
- [ ] 实现 `RemoteValidateConfigBuilder`：链式方法 `endpoint()` / `timeout()` / `allow_all_action()` / `pool_max_idle_per_host()` / `pool_idle_timeout()` / `tcp_keepalive()` / `tcp_nodelay()`；`.build()` 未设 endpoint → `Err(InvalidConfig)`，否则委托 `new_checked`（FR-4.1 ~ FR-4.3）

### 2.2 向后兼容：new() 委托 new_checked（T3）
- [ ] 将 `RemoteValidateConfig::new`（`sso_middleware.rs:184-200`）改为 `Self::new_checked(...).expect("failed to build reqwest client")`，保留原签名 `fn new(...) -> Self`，调用方零改动（FR-2.1 / NFR-1.2）
- [ ] 验证 `sso_middleware_remote` 中间件签名与行为不变（消费 `Arc<RemoteValidateConfig>`，构造方式对中间件透明，NFR-1.3）

---

## 3. 测试与质量门禁

### 3.1 单元测试（T4）
- [ ] 在 `sso_middleware.rs` 的 `#[cfg(test)]` 模块新增测试：`test_new_checked_empty_endpoint`、`test_new_checked_valid`、`test_new_backward_compat`、`test_new_or_default_fallback`、`test_builder_missing_endpoint`、`test_builder_full_config`、`test_builder_default_pool_config`、`test_from_client_holds_external`、`test_pool_config_default`、`test_debug_no_client_leak`（对应 design §2.4 清单）
- [ ] 验证 `cargo test -p sz-rust-middleware-facade --features remote-validate` 全部通过，覆盖 AC-1 ~ AC-6、NFR-2.1

### 3.2 clippy 与回归测试（T5）
- [ ] 运行 `cargo clippy -p sz-rust-middleware-facade --features remote-validate -- -D warnings`，零警告；确认所有 `async fn` 满足 `Send + 'static`（AC-8）
- [ ] 运行 `cargo build -p sz-rust-middleware-facade --no-default-features` 验证 feature 关闭时编译通过、无 `reqwest` 符号（AC-7）
- [ ] 回归：现有 `new()` 调用方（sz-rust-sz300 等）编译通过，行为与 v0.6.3 一致（AC-2）

---

## 4. 版本与发布

### 4.1 CHANGELOG 与版本 bump（T6）
- [ ] 在 `packages/sz-rust-middleware-facade/Cargo.toml` 将版本从 `0.6.3` 改为 `0.6.4`
- [ ] 在 `CHANGELOG.md` 新增 `## [0.6.4] - 2026-08-08` 段，记录新增 API：`new_checked` / `new_or_default` / `from_client` / `RemoteValidateConfigBuilder` / `PoolConfig` / `RefreshTokenError::InvalidConfig`，标注 semver 兼容

### 4.2 发布到 crates.io（T7）
- [ ] 运行 `cargo publish -p sz-rust-middleware-facade --dry-run` 验证打包无误，再执行正式 `cargo publish -p sz-rust-middleware-facade`
- [ ] 验证 `cargo search sz-rust-middleware-facade` 显示 0.6.4；内部项目 sz-pay（`E:\vue\test\sz-pay`）更新依赖版本并编译通过

---

## 5. 验收确认

- [ ] 逐项核对 spec.md §5 验收清单 AC-1 ~ AC-8 全部通过，每项附 `file:line` 证据
- [ ] 确认未修改 Out-of-Scope 项（`sso_middleware_remote` 签名、feature gate `remote-validate` 定义、上游 `sz-orm` / `sz-orm-auth`）