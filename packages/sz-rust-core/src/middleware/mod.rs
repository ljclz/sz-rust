//! 中间件模块 — CORS/Auth/Log/RateLimit/Trace + 链构建器 + Handler=Middleware 统一设计
//!
//! 基于 tower::Layer，对齐 PHP 中间件行为。
//!
//! ## 模块结构
//!
//! | 模块 | 内容 | 实现阶段 |
//! |------|------|---------|
//! | [`order`] | `MiddlewareKind` 枚举 + `DEFAULT_ORDER` / `PHP_GLOBAL_ORDER` 常量 | ✅ |
//! | [`chain`] | `MiddlewareChain` 构建器（顺序定义 + 验证） | ✅ |
//! | [`handler_as_middleware`] | Handler=Middleware 双向转换器（对齐 Salvo 设计） | ✅ |
//! | [`cors`] | CORS 中间件（基于 `tower-http::cors`，对齐 PHP `app\CrossDomain`） | ✅ |
//! | [`csrf`] | CSRF 中间件（双提交 Cookie 模式，2026-07-25 新增） | 安全修复 ✅ |
//! | [`auth`] | Auth 中间件（JWT 校验，复用 sz-orm-auth） | ✅ |
//! | [`log`] | Log 中间件（请求/响应日志，对齐 PHP `think-logger`） | ✅ |
//! | [`rate_limit`] | RateLimit 中间件（复用 sz-orm-limit） | ✅ |
//! | [`trace`] | Trace 中间件（W3C TraceContext 传播，复用 sz-orm-tracing） | ✅ |
//! | [`builder`] | `MiddlewareBuilder` 链构建器（持有 `MiddlewareChain` + 5 个 `Option<Config>`） | ✅ |
//! | [`tower_compat`] | `TowerCompat` 包装器（兼容 tower-http Compression/Timeout/TraceLayer） | ✅ |
//!
//! ## PHP 端中间件对齐
//!
//! PHP `app/middleware.php` 全局中间件顺序：
//!
//! ```php
//! return [
//!     \think\middleware\SessionInit::class,    // → Rust Trace
//!     \think\middleware\AllowCrossDomain::class, // → Rust Cors
//! ];
//! ```
//!
//! 业务层中间件（如 `app\oapc\middleware\Auth`）通过应用级 `app/<app>/middleware.php`
//! 追加，执行顺序在全局中间件之后。
//!
//! Rust 端 [`order::DEFAULT_ORDER`] 定义了完整默认顺序（5 个中间件），
//! [`chain::MiddlewareChain`] 提供链构建器。
//!
//! ## 执行顺序约定
//!
//! Rust 端使用 `tower::ServiceBuilder`，layer 是「后注册先执行」（stack 反向）。
//! `MiddlewareChain::order()` 返回业务期望顺序（首元素最先执行），
//! `MiddlewareChain::service_builder_order()` 返回 `ServiceBuilder` 注册顺序（逆序）。

pub mod auth;
pub mod builder;
pub mod chain;
pub mod cors;
pub mod csrf;
pub mod handler_as_middleware;
pub mod jwt_blacklist;
pub mod log;
pub mod order;
pub mod rate_limit;
pub mod request_scope;
pub mod sanctum;
pub mod tower_compat;
pub mod trace;
