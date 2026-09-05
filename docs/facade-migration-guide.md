# sz-rust Facade 迁移指南

> **适用版本**：v0.3.0+
> **目标读者**：`sz-rust-sz300`、`sz-rust-addons-*` 等业务包维护者

## 背景

v0.3.0 起，`sz-rust-core` 通过 **Facade 渐进提取**拆分为 7 个独立 facade crate：

| Facade | 职责 | LOC |
|--------|------|-----|
| `sz-rust-orm-facade` | ORM 全家桶统一入口（Repository、QueryBuilder、缓存等） | ~692 |
| `sz-rust-http-facade` | HTTP 基础层（response / error / request） | ~3,047 |
| `sz-rust-cache-facade` | 多级缓存（Memory / Redis / Memcached / MultiLevel） | ~7,068 |
| `sz-rust-state-facade` | 应用状态（session / cookie / env / event / i18n / mail / notify） | ~7,468 |
| `sz-rust-infra-facade` | 基础设施（config / validate / static_files / upload / debug_page） | ~18,382 |
| `sz-rust-auth-facade` | 认证与网关（wechat / oauth / gateway） | ~4,221 |
| `sz-rust-pay-facade` | 支付抽象（PayProvider / PayOrder / MemoryPayProvider） | ~1,506 |

`sz-rust-core` 从 57K LOC 降至 23.6K LOC（**−58.7%**）。

## 迁移策略

### 策略 A：零改动（向后兼容）

`sz-rust-core` 保留了所有旧的 `crate::<module>::*` 路径：

```rust
// 以下代码无需任何改动，继续有效：
use sz_rust_core::orm::repository::Repository;
use sz_rust_core::http::response::ApiResponse;
use sz_rust_core::cache::CacheDriver;
use sz_rust_core::state::session::SessionStore;
use sz_rust_core::infra::validate::Validator;
use sz_rust_core::auth::wechat::WechatSdk;
use sz_rust_core::pay::{PayOrder, PayProvider};
```

**适用场景**：现有业务包不急于减耦，等待后续迭代逐步迁移。

### 策略 B：直接依赖 facade crate（推荐新业务）

新业务包或愿意减耦的现有业务包，直接依赖 facade crate：

```toml
# Cargo.toml — 移除 sz-rust-core 的间接依赖
[dependencies]
sz-rust-orm-facade = "0.3.0"
sz-rust-http-facade = "0.3.0"
sz-rust-cache-facade = "0.3.0"
sz-rust-state-facade = "0.3.0"
sz-rust-infra-facade = "0.3.0"
sz-rust-auth-facade = "0.3.0"
sz-rust-pay-facade = "0.3.0"
```

```rust
// 代码中替换 import 路径：
use sz_rust_orm_facade::repository::Repository;
use sz_rust_http_facade::response::ApiResponse;
use sz_rust_cache_facade::CacheDriver;
use sz_rust_state_facade::session::SessionStore;
use sz_rust_infra_facade::validate::Validator;
use sz_rust_auth_facade::wechat::WechatSdk;
use sz_rust_pay_facade::pay::{PayOrder, PayProvider};
```

**收益**：
- 编译时只拉取实际用到的 facade，跳过 `sz-rust-core` 的 23.6K LOC
- 依赖图更清晰，facade 独立编译，增量编译收益更高
- 未来 facade 独立发布到 crates.io 后可按需升级

## 分步迁移检查表

### Step 1：分析当前依赖

```bash
# 查看当前 Cargo.toml 中 sz-rust-core 的使用范围
grep -rn "sz_rust_core::" src/ --include="*.rs" | \
  sed 's/.*sz_rust_core::\([^:]*\)::.*/\1/' | sort -u
```

### Step 2：按需替换 facade 依赖

根据 Step 1 的输出，只引入实际用到的 facade：

| 使用 `sz_rust_core::` 的模块 | 替换为 facade |
|----------------------------|--------------|
| `orm::*` | `sz-rust-orm-facade` |
| `http::*` / `response` / `error` / `request` | `sz-rust-http-facade` |
| `cache::*` | `sz-rust-cache-facade` |
| `state::*` / `session` / `cookie` / `env` / `event` / `i18n` / `mail` / `notify` | `sz-rust-state-facade` |
| `infra::*` / `config` / `validate` / `static_files` / `upload` / `debug_page` | `sz-rust-infra-facade` |
| `auth::*` / `wechat` / `oauth` / `gateway` | `sz-rust-auth-facade` |
| `pay::*` | `sz-rust-pay-facade` |

### Step 3：验证编译

```bash
# 移除 Cargo.toml 中的 sz-rust-core（如果不再需要）
# cargo remove sz-rust-core  # 或用编辑器手动删除

cargo check --all-targets
cargo test --all-targets
```

### Step 4：验证功能

运行业务包的集成测试，确保 facade 路径替换后功能一致：

```bash
cargo test --test integration
```

## 常见迁移问题

### Q1：我的业务包同时用了 `sz-rust-core` 和其他 facade，会有版本冲突吗？

不会。所有 facade 使用 `version.workspace = true`，与 `sz-rust-core` 版本一致（v0.3.0）。

### Q2：迁移后 `sz_rust_core::pay` 还能用吗？

可以。`sz-rust-core` 通过 `pub use sz_rust_pay_facade::pay;` 重导出，两条路径等价：

```rust
// 等价，任选其一：
use sz_rust_core::pay::{PayOrder, PayProvider};
use sz_rust_pay_facade::pay::{PayOrder, PayProvider};
```

### Q3：facade crate 的 feature flag 如何处理？

facade crate 的 feature 通过 `sz-rust-core` 透传。例如：

```toml
# 通过 sz-rust-core 启用 graphql
sz-rust-core = { version = "0.3.0", features = ["graphql"] }

# 或直接通过 orm-facade 启用
sz-rust-orm-facade = { version = "0.3.0", features = ["graphql"] }
```

### Q4：迁移后编译时间能提升多少？

取决于业务包实际使用的 facade 数量。粗略估算：

| 场景 | 编译 LOC 减少 | 预期加速 |
|------|-------------|---------|
| 仅用 ORM | ~23K LOC | 30-50% |
| 仅用 HTTP + Cache | ~10K LOC | 40-60% |
| 全量使用 | ~0 LOC | 无变化（向后兼容路径） |

## 回滚方案

如果迁移后遇到问题，可随时回退到策略 A（零改动）：

```toml
# 恢复 sz-rust-core 依赖，移除 facade 直接依赖
[dependencies]
sz-rust-core = "0.3.0"
# 移除 sz-rust-*-facade 的直接依赖
```

代码无需改动，`sz_rust_core::<module>::*` 路径自动恢复。

## 附录：完整 import 对照表

| 旧路径（`sz_rust_core::*`） | 新路径（facade 直接依赖） |
|---------------------------|------------------------|
| `sz_rust_core::orm::repository::Repository` | `sz_rust_orm_facade::repository::Repository` |
| `sz_rust_core::http::response::ApiResponse` | `sz_rust_http_facade::response::ApiResponse` |
| `sz_rust_core::http::error::BaseException` | `sz_rust_http_facade::error::BaseException` |
| `sz_rust_core::cache::CacheDriver` | `sz_rust_cache_facade::CacheDriver` |
| `sz_rust_core::state::session::SessionStore` | `sz_rust_state_facade::session::SessionStore` |
| `sz_rust_core::infra::validate::Validator` | `sz_rust_infra_facade::validate::Validator` |
| `sz_rust_core::auth::wechat::WechatSdk` | `sz_rust_auth_facade::wechat::WechatSdk` |
| `sz_rust_core::pay::PayProvider` | `sz_rust_pay_facade::pay::PayProvider` |
