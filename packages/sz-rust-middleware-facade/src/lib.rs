//! sz-rust-middleware-facade — 中间件层（P3 解耦）
//!
//! 从 sz-rust-core 提取的 Tower 中间件簇（对齐 PHP ThinkPHP 中间件体系）：
//!
//! - [`auth`]：JWT 校验 + 白名单跳过（**@REVIEW_REQUIRED，安全关键**）
//! - [`sanctum`]：个人访问令牌认证（**@REVIEW_REQUIRED，安全关键**）
//! - [`jwt_blacklist`]：JWT 黑名单 / 注销列表（**@REVIEW_REQUIRED，安全关键**）
//! - [`builder`] / [`chain`] / [`order`] / [`handler_as_middleware`] / [`tower_compat`]：中间件组装
//! - [`cors`] / [`csrf`] / [`rate_limit`] / [`log`] / [`trace`]：横切能力
//! - [`request_scope`]：请求作用域（ScopeId 类型定义于此，解 container 双向环）
//! - [`log`]：日志门面（LogFacade / LogLevel）
//!
//! ## 请求作用域（环消除设计）
//!
//! `ScopeId`（`pub type ScopeId = u64`）原定义于 sz-rust-core 的 `container` 模块，
//! 与 `request_scope::current_scope_id()` 形成 container ↔ request_scope 双向依赖。
//! P3 将 `ScopeId` 定义迁移至本 crate，sz-rust-core 的 `container` 通过
//! `pub use sz_rust_middleware_facade::ScopeId` 重导出——依赖方向变为单向：
//! `core(container) → middleware-facade`，环消除。
//!
//! sz-rust-core 通过 `pub use sz_rust_middleware_facade as middleware` 保留向后兼容路径。

/// 请求作用域 ID（原定义于 sz-rust-core::container，P3 迁移至此消除双向环）
pub type ScopeId = u64;

pub mod audit_log;
pub mod auth;
pub mod body_size_limit;
pub mod builder;
pub mod chain;
pub mod circuit_breaker;
pub mod cors;
pub mod csrf;
pub mod handler_as_middleware;
pub mod ip_access_control;
pub mod jwt_blacklist;
pub mod log;
pub mod order;
pub mod rate_limit;
pub mod request_scope;
pub mod sanctum;
pub mod security_headers;
pub mod security_metrics;
pub mod security_section;
pub mod sso_middleware;
pub mod tower_compat;
pub mod trace;

/// Security 配置段 re-export（应用层直接 `use sz_rust_middleware_facade::SecuritySection`）
pub use security_section::{SecurityConfigError, SecuritySection};
