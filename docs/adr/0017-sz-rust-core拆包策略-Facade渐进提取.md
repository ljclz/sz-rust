# ADR-017：sz-rust-core 拆包策略（Facade 渐进提取）

> **状态**：已完成
> **日期**：2026-08-03（更新）
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-007（addon 插件化）、v0.3.0 综合能力评估报告
> **相关代码**：`packages/sz-rust-{orm,http,cache,state,infra,auth,pay}-facade/`

## 背景

v0.3.0 综合能力评估报告指出：

> `sz-rust-core` 过于庞大（57K LOC，49 个模块），建议拆分为 `sz-rust-http`、`sz-rust-orm`、`sz-rust-auth` 等子包

大型单体 crate 的问题：
- **编译时间**：任何改动触发全量重编，增量编译收益有限
- **依赖耦合**：下游业务包被迫引入不需要的依赖
- **职责不清**：HTTP 层、ORM 层、认证层混在同一 crate
- **并行开发**：多人同时修改同一 crate 容易产生冲突

## 决策

采用 **Facade 渐进提取**策略：

1. 从 `sz-rust-core/src/` 中提取高内聚、低耦合的模块为独立 crate
2. 新 crate 命名为 `sz-rust-<domain>-facade`，通过 `pub use X as <module>` 重导出
3. `sz-rust-core` 保留向后兼容的 `crate::<module>::*` 路径，内部代码无需改动
4. 下游业务包可选择直接依赖 facade crate 以减少编译耦合

### 已提取的 Facade

| Crate | 提取模块 | LOC | 外部依赖 | 状态 |
|-------|---------|-----|---------|------|
| `sz-rust-orm-facade` | `orm.rs` + 子模块 | ~692 | sz-orm-* 全家桶 | ✅ 完成 |
| `sz-rust-http-facade` | `response.rs` + `error.rs` + `request.rs` | ~3,047 | axum, serde, regex, tracing, http-body-util, tower | ✅ 完成 |
| `sz-rust-cache-facade` | `cache.rs` + `cache/memcached.rs` | ~7,068 | sz-rust-orm-facade, parking_lot, serde, md-5 | ✅ 完成 |
| `sz-rust-state-facade` | `session/cookie/env/event/i18n/mail/notify` | ~7,468 | axum, chrono, parking_lot, serde, thiserror | ✅ 完成 |
| `sz-rust-infra-facade` | `config/validate/static_files/upload/debug_page` | ~18,382 | axum, serde_yml, image, ab_glyph, sz-orm-storage, tower-http | ✅ 完成 |
| `sz-rust-auth-facade` | `wechat/oauth/gateway` | ~4,221 | parking_lot, serde, sha1, thiserror | ✅ 完成 |
| `sz-rust-pay-facade` | `pay` | ~1,506 | parking_lot, serde, serde_json, thiserror | ✅ 完成 |
| **合计提取** | **7 个 facade，20 个模块** | **~42,384** | — | ✅ |
| `sz-rust-core` 剩余 | 27 个模块 | ~23,595 | — | — |

sz-rust-core 从 57K LOC 降至 23.6K LOC（**−58.7%**），workspace 全量 4,954 测试 0 失败（含 7 个新增并发/混沌测试）。

### 访问路径

```rust
// 通过 sz-rust-core 访问（向后兼容，内部模块无需改动）
use sz_rust_core::orm::repository::Repository;
use sz_rust_core::http::response::ApiResponse;
use sz_rust_core::http::error::BaseException;
use sz_rust_core::http::request::fetch_post_data;
use sz_rust_core::cache::CacheDriver;
use sz_rust_core::state::session::SessionStore;
use sz_rust_core::infra::validate::Validator;
use sz_rust_core::auth::wechat::WechatSdk;
use sz_rust_core::pay::{PayOrder, PayProvider};

// 直接依赖 facade crate（推荐新业务使用，减少编译耦合）
use sz_rust_orm_facade::repository::Repository;
use sz_rust_http_facade::response::ApiResponse;
use sz_rust_cache_facade::CacheDriver;
use sz_rust_state_facade::session::SessionStore;
use sz_rust_infra_facade::validate::Validator;
use sz_rust_auth_facade::wechat::WechatSdk;
use sz_rust_pay_facade::pay::{PayOrder, PayProvider};
```

## 决策替代方案

### 方案 A：一次性大拆包

将 `sz-rust-core` 一次性拆为 `sz-rust-http`、`sz-rust-orm`、`sz-rust-auth`、`sz-rust-cache` 等多个 crate。

**拒绝原因**：
- **模块耦合复杂**：49 个模块之间存在大量交叉依赖（如 `view.rs` 依赖 `crate::response::respond_html`，`guard.rs` 依赖 `crate::middleware::auth`）
- **风险高**：一次性改动面过大，回归测试成本高
- **API 重设计**：部分模块需要重新设计公共 API 才能解耦，需要充分的 ADR 讨论

### 方案 B：保持现状，仅文档分层

不实际拆分 crate，仅通过模块文档划分职责边界。

**拒绝原因**：
- **编译时间无改善**：57K LOC 单 crate 编译时间仍长
- **依赖无法裁剪**：下游业务包仍需引入全部依赖
- **评估报告明确要求**：评估报告将此列为 P2 优先项

### 方案 C：Facade 渐进提取（已采用）

**采纳原因**：
- **零破坏性**：通过 `pub use` 重导出，现有代码路径 `crate::orm::*` 完全不变
- **可逆**：每步提取独立验证，失败可回退
- **增量收益**：每提取一个 facade，下游即可选择直接依赖以减耦
- **编译加速**：facade crate 独立编译，下游业务包可跳过 sz-rust-core 的全量编译

## 提取标准

模块提取为独立 facade crate 的条件：

1. **零 `crate::` 依赖**：模块不引用 `sz-rust-core` 中其他模块（或仅依赖已提取的 facade）
2. **外部依赖可控**：引入的新依赖数量合理，不会导致依赖爆炸
3. **职责内聚**：模块内部功能高度相关（如 response/error/request 均属 HTTP 基础层）
4. **测试独立**：模块测试不依赖 `sz-rust-core` 的内部实现

## 已知限制

### 已全部提取的模块（P3 完成）

> **2026-08-03 P3 更新**：原"暂无法提取"的 6 个模块已全部提取完成（见 ADR-019），
> 采用**依赖簇**分批方案：orm-ext 簇（model/hooks/relation）→ router 簇（routing/router/
> websocket_route/openapi）→ middleware 簇（14 个中间件 + log，先解 container↔request_scope 环）
> → mvc 簇（view/controller/guard）。core 以 `pub use <facade> as <module>` 重导出，下游零改动。

| 模块 | 原阻塞原因 | 提取结果 |
|------|-----------|---------|
| `view` | 调用 `respond_html` + DI 容器 + cache | ✅ sz-rust-mvc-facade（实测不依赖 Container 类型；respond_html 来自 http-facade） |
| `controller` | 依赖 request/response/validate | ✅ sz-rust-mvc-facade（http-facade + infra-facade + orm-facade） |
| `guard` | 依赖 middleware::auth + error | ✅ sz-rust-mvc-facade（middleware 簇先行提取，依赖方向 mvc→middleware 无环） |
| `hooks` | 直接依赖 sz-orm-core | ✅ sz-rust-orm-ext-facade（经 orm-facade 间接依赖） |
| `model` | 直接依赖 sz-orm-core | ✅ sz-rust-orm-ext-facade（经 orm-facade 间接依赖） |
| `routing` | 依赖 router + guard + middleware | ✅ sz-rust-router-facade（router/websocket_route/openapi 同簇带走，零路径修改） |

> **2026-08-03 更新**：`pay` 模块已从阻塞列表中移除，已成功提取为 `sz-rust-pay-facade`（1,506 LOC，零内部依赖）。
> P3 后 sz-rust-core 由 57K LOC 降至 ~9.2K（−83.9%）。

### 模块间交叉依赖

部分模块（如 `guard` → `middleware::auth`，`controller` → `request`/`response`）存在单向依赖，
这些依赖通过 facade 重导出后仍可正常工作（`crate::middleware::auth` 和 `crate::http::response` 均在 sz-rust-core 内可访问）。

### 依赖方向不一致问题

> **2026-08-03 已修复**：`sz-rust-infra-facade` 的 `sz-orm-storage` 直接依赖已统一为通过 `sz-rust-orm-facade` 间接依赖。所有 facade 的 orm 依赖路径现已一致。

## 后续步骤

1. ~~**提取 `pay` 模块**~~ ✅ 已完成（2026-08-03，`sz-rust-pay-facade`，1,506 LOC，零内部依赖）
2. ~~**继续提取剩余模块**~~ ✅ P3 全部完成（2026-08-03）：orm-ext / router / middleware / mvc 四簇 11 个 facade
3. ~~**解耦阻塞模块**~~ ✅ P3 完成（view/controller/guard/hooks/model/routing 全部提取，见 ADR-019）
4. **下游迁移**：引导 `sz-rust-sz300` 等业务包直接依赖 facade crate，验证编译加速收益
5. **依赖治理**：引入 `cargo deny` / `cargo udeps` / `cargo hakari` 到 CI，防止依赖膨胀和循环依赖（deny/udeps 已就绪）
6. **编译时间基线**：建立拆包前后的编译时间对比数据，量化收益
7. **facade 独立发布**：定义每个 facade crate 的独立版本号策略，支持按需发布到 crates.io
