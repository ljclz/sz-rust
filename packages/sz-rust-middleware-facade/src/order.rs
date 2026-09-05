// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 中间件顺序定义 — 对齐 PHP `app/middleware.php`
//!
//! PHP 端 `app/middleware.php` 全局中间件顺序：
//!
//! ```php
//! return [
//!     \think\middleware\SessionInit::class,
//!     \think\middleware\AllowCrossDomain::class,
//! ];
//! ```
//!
//! 业务层中间件（如 `app\oapc\middleware\Auth`）通过应用级 `app/<app>/middleware.php`
//! 追加，执行顺序在全局中间件之后。
//!
//! ## Rust 端映射
//!
//! | PHP 中间件 | Rust 中间件 | 实现阶段 |
//! |------------|-------------|---------|
//! | `SessionInit` | `Trace`（生成 request_id，复用 sz-orm-tracing） | ✅ |
//! | `AllowCrossDomain` | `Cors`（已实现于 `cors.rs`） | ✅ |
//! | `app\oapc\middleware\Auth` | `Auth`（JWT 校验，复用 sz-orm-auth） | ✅ |
//! | （PHP 端无） | `Log`（请求/响应日志） | ✅ |
//! | （PHP 端无） | `RateLimit`（限流，复用 sz-orm-limit） | ✅ |
//!
//! ## 执行顺序约定
//!
//! Rust 端使用 `tower::ServiceBuilder`，layer 是「后注册先执行」（stack 反向）。
//! 本模块定义的 `DEFAULT_ORDER` 表示「业务期望的执行顺序」——数组首元素最先执行。
//! `MiddlewareChain` 内部会按需反转以适配 `ServiceBuilder::layer` 语义。

use std::fmt;

/// 中间件类型枚举
///
/// 对齐 PHP 中间件 + sz-rust 自定义中间件。每个变体对应一个具体的 Layer 实现，
/// 由 `auth.rs` / `log.rs` / `cors.rs` / `rate_limit.rs` 等模块逐个实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MiddlewareKind {
    /// 追踪 span（生成 request_id，对齐 PHP `SessionInit` 的「请求初始化」语义）
    ///
    /// **最先执行**：包裹所有后续中间件，确保 request_id 在所有日志/追踪中可用。
    Trace,
    /// CORS 跨域预处理（对齐 PHP `AllowCrossDomain`）
    ///
    /// **第二执行**：OPTIONS 预检请求直接返回，不进入业务逻辑。
    Cors,
    /// 请求/响应日志（对齐 PHP `think-logger`）
    ///
    /// **第三执行**：在限流/鉴权之前记录所有请求（包括被拒绝的）。
    Log,
    /// 限流（复用 sz-orm-limit）
    ///
    /// **第四执行**：在鉴权之前限流，避免无效请求消耗鉴权开销。
    RateLimit,
    /// JWT 鉴权（对齐 PHP `app\<app>\middleware\Auth`，复用 sz-orm-auth）
    ///
    /// **第五执行**：通过限流后进行鉴权，未登录返回 NotLogin(-1)。
    Auth,
    /// 安全响应头注入（X-Frame-Options / X-Content-Type-Options / HSTS / CSP / Referrer-Policy / Permissions-Policy）
    ///
    /// **响应阶段执行**：在响应构建时注入安全头，不阻塞请求处理。
    SecurityHeaders,
    /// IP 访问控制（白/黑名单 + CIDR 网段匹配）
    ///
    /// **安全门控阶段**：在鉴权之前拒绝不可信 IP，fail-close/fail-open 可选。
    IpAccessControl,
    /// 安全审计日志（结构化 JSON 事件 + 敏感字段脱敏 + 采样率）
    ///
    /// **最后执行**：在鉴权之后审计，可关联 user_id。
    AuditLog,
    /// 请求体大小限制（全局+分路径上限 + Content-Length 与 Body 双校验）
    ///
    /// **早期执行**：在所有业务逻辑之前拒绝超大请求，消耗最少。
    BodySizeLimit,
}

impl MiddlewareKind {
    /// 返回中间件的人类可读名称（用于日志和测试）
    pub fn as_str(self) -> &'static str {
        match self {
            MiddlewareKind::Trace => "trace",
            MiddlewareKind::Cors => "cors",
            MiddlewareKind::Log => "log",
            MiddlewareKind::RateLimit => "rate_limit",
            MiddlewareKind::Auth => "auth",
            MiddlewareKind::SecurityHeaders => "security_headers",
            MiddlewareKind::IpAccessControl => "ip_access_control",
            MiddlewareKind::AuditLog => "audit_log",
            MiddlewareKind::BodySizeLimit => "body_size_limit",
        }
    }

    /// 返回中间件在 PHP 端的对应物（用于文档对齐验证）
    pub fn php_counterpart(self) -> &'static str {
        match self {
            MiddlewareKind::Trace => "\\think\\middleware\\SessionInit",
            MiddlewareKind::Cors => "\\think\\middleware\\AllowCrossDomain",
            MiddlewareKind::Log => "(none, sz-rust 自研，对齐 think-logger)",
            MiddlewareKind::RateLimit => "(none, sz-rust 自研)",
            MiddlewareKind::Auth => "app\\<app>\\middleware\\Auth",
            MiddlewareKind::SecurityHeaders => "(none, sz-rust 自研，对齐 think-security)",
            MiddlewareKind::IpAccessControl => "(none, sz-rust 自研，对齐 think-security)",
            MiddlewareKind::AuditLog => "(none, sz-rust 自研，对齐 think-logger)",
            MiddlewareKind::BodySizeLimit => "(none, sz-rust 自研)",
        }
    }
}

impl fmt::Display for MiddlewareKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 默认中间件顺序（业务期望的执行顺序，数组首元素最先执行）
///
/// 对齐 PHP `app/middleware.php` 全局中间件 + sz-rust 业务中间件约定：
///
/// 1. `Trace` — 生成 request_id（对齐 PHP `SessionInit` 请求初始化语义）
/// 2. `BodySizeLimit` — 请求体大小限制（最早拦截超大请求，消耗最少）
/// 3. `IpAccessControl` — IP 访问控制（安全门控，拒绝不可信 IP）
/// 4. `SecurityHeaders` — 安全响应头注入（响应阶段注入，不阻塞请求）
/// 5. `Cors` — 跨域预处理（对齐 PHP `AllowCrossDomain`）
/// 6. `Log` — 请求日志（sz-rust 自研，PHP 端无全局日志中间件）
/// 7. `RateLimit` — 限流（sz-rust 自研，PHP 端无全局限流中间件）
/// 8. `Auth` — JWT 鉴权（对齐 PHP `app\<app>\middleware\Auth`）
/// 9. `AuditLog` — 安全审计日志（最后执行，可关联 user_id）
///
/// ## 顺序设计理由
///
/// - `Trace` 最先：确保 request_id 在所有后续中间件的日志中可用
/// - `BodySizeLimit` 第二：最早拒绝超大请求，消耗最少资源
/// - `IpAccessControl` 第三：安全门控，在鉴权前拒绝不可信 IP
/// - `SecurityHeaders` 第四：响应阶段注入安全头，不阻塞请求处理
/// - `Cors` 第五：OPTIONS 预检请求直接返回，不消耗后续中间件资源
/// - `Log` 第六：记录所有请求（包括被限流/鉴权拒绝的），用于审计
/// - `RateLimit` 第七：在鉴权之前限流，避免无效请求消耗鉴权开销
/// - `Auth` 第八：通过限流后进行鉴权，未登录返回 NotLogin(-1)
/// - `AuditLog` 第九：最后执行，可关联鉴权后的 user_id
pub const DEFAULT_ORDER: &[MiddlewareKind] = &[
    MiddlewareKind::Trace,
    MiddlewareKind::BodySizeLimit,
    MiddlewareKind::IpAccessControl,
    MiddlewareKind::SecurityHeaders,
    MiddlewareKind::Cors,
    MiddlewareKind::Log,
    MiddlewareKind::RateLimit,
    MiddlewareKind::Auth,
    MiddlewareKind::AuditLog,
];

/// PHP 全局中间件顺序（对齐 `app/middleware.php`）
///
/// PHP 端 `app/middleware.php` 返回的数组顺序：
/// 1. `SessionInit` → Rust `Trace`
/// 2. `AllowCrossDomain` → Rust `Cors`
///
/// 业务层中间件（如 `Auth`）通过应用级 `app/<app>/middleware.php` 追加，
/// 执行顺序在全局中间件之后。
pub const PHP_GLOBAL_ORDER: &[MiddlewareKind] = &[MiddlewareKind::Trace, MiddlewareKind::Cors];

#[cfg(test)]
mod tests {
    use super::*;

    // ====================================================================
    // MiddlewareKind 枚举
    // ====================================================================

    #[test]
    fn test_middleware_kind_as_str() {
        assert_eq!(MiddlewareKind::Trace.as_str(), "trace");
        assert_eq!(MiddlewareKind::Cors.as_str(), "cors");
        assert_eq!(MiddlewareKind::Log.as_str(), "log");
        assert_eq!(MiddlewareKind::RateLimit.as_str(), "rate_limit");
        assert_eq!(MiddlewareKind::Auth.as_str(), "auth");
        assert_eq!(MiddlewareKind::SecurityHeaders.as_str(), "security_headers");
        assert_eq!(
            MiddlewareKind::IpAccessControl.as_str(),
            "ip_access_control"
        );
        assert_eq!(MiddlewareKind::AuditLog.as_str(), "audit_log");
        assert_eq!(MiddlewareKind::BodySizeLimit.as_str(), "body_size_limit");
    }

    #[test]
    fn test_middleware_kind_display() {
        assert_eq!(MiddlewareKind::Trace.to_string(), "trace");
        assert_eq!(MiddlewareKind::Cors.to_string(), "cors");
        assert_eq!(MiddlewareKind::Log.to_string(), "log");
        assert_eq!(MiddlewareKind::RateLimit.to_string(), "rate_limit");
        assert_eq!(MiddlewareKind::Auth.to_string(), "auth");
        assert_eq!(
            MiddlewareKind::SecurityHeaders.to_string(),
            "security_headers"
        );
        assert_eq!(
            MiddlewareKind::IpAccessControl.to_string(),
            "ip_access_control"
        );
        assert_eq!(MiddlewareKind::AuditLog.to_string(), "audit_log");
        assert_eq!(MiddlewareKind::BodySizeLimit.to_string(), "body_size_limit");
    }

    #[test]
    fn test_middleware_kind_php_counterpart() {
        // 对齐 PHP 全局中间件
        assert_eq!(
            MiddlewareKind::Trace.php_counterpart(),
            "\\think\\middleware\\SessionInit"
        );
        assert_eq!(
            MiddlewareKind::Cors.php_counterpart(),
            "\\think\\middleware\\AllowCrossDomain"
        );
        assert_eq!(
            MiddlewareKind::Auth.php_counterpart(),
            "app\\<app>\\middleware\\Auth"
        );
    }

    #[test]
    fn test_middleware_kind_eq_hash() {
        // PartialEq + Eq + Hash 支持用作 HashMap key
        use std::collections::HashSet;
        let set: HashSet<MiddlewareKind> = [
            MiddlewareKind::Trace,
            MiddlewareKind::Cors,
            MiddlewareKind::Trace,
        ]
        .into_iter()
        .collect();
        assert_eq!(set.len(), 2);
        assert!(set.contains(&MiddlewareKind::Trace));
        assert!(set.contains(&MiddlewareKind::Cors));
        assert!(!set.contains(&MiddlewareKind::Auth));
    }

    #[test]
    fn test_middleware_kind_clone_copy() {
        let kind = MiddlewareKind::Cors;
        let cloned = kind; // Copy 语义
        assert_eq!(kind, cloned);
    }

    // ====================================================================
    // DEFAULT_ORDER 默认顺序
    // ====================================================================

    #[test]
    fn test_default_order_length() {
        assert_eq!(DEFAULT_ORDER.len(), 9);
    }

    #[test]
    fn test_default_order_trace_first() {
        // Trace 必须最先执行（包裹所有后续中间件）
        assert_eq!(DEFAULT_ORDER.first(), Some(&MiddlewareKind::Trace));
    }

    #[test]
    fn test_default_order_audit_log_last() {
        // AuditLog 必须最后执行（可关联鉴权后的 user_id）
        assert_eq!(DEFAULT_ORDER.last(), Some(&MiddlewareKind::AuditLog));
    }

    #[test]
    fn test_default_order_cors_before_log() {
        // CORS 必须在 Log 之前（OPTIONS 预检直接返回，不记录日志）
        let cors_idx = DEFAULT_ORDER
            .iter()
            .position(|k| *k == MiddlewareKind::Cors)
            .expect("Cors must be in DEFAULT_ORDER");
        let log_idx = DEFAULT_ORDER
            .iter()
            .position(|k| *k == MiddlewareKind::Log)
            .expect("Log must be in DEFAULT_ORDER");
        assert!(cors_idx < log_idx, "Cors must execute before Log");
    }

    #[test]
    fn test_default_order_rate_limit_before_auth() {
        // RateLimit 必须在 Auth 之前（避免无效请求消耗鉴权开销）
        let rate_limit_idx = DEFAULT_ORDER
            .iter()
            .position(|k| *k == MiddlewareKind::RateLimit)
            .expect("RateLimit must be in DEFAULT_ORDER");
        let auth_idx = DEFAULT_ORDER
            .iter()
            .position(|k| *k == MiddlewareKind::Auth)
            .expect("Auth must be in DEFAULT_ORDER");
        assert!(
            rate_limit_idx < auth_idx,
            "RateLimit must execute before Auth"
        );
    }

    #[test]
    fn test_default_order_no_duplicates() {
        // 默认顺序中不应有重复中间件
        use std::collections::HashSet;
        let set: HashSet<MiddlewareKind> = DEFAULT_ORDER.iter().copied().collect();
        assert_eq!(
            set.len(),
            DEFAULT_ORDER.len(),
            "DEFAULT_ORDER has duplicates"
        );
    }

    #[test]
    fn test_default_order_contains_all_kinds() {
        // 默认顺序应包含所有中间件类型
        for kind in [
            MiddlewareKind::Trace,
            MiddlewareKind::Cors,
            MiddlewareKind::Log,
            MiddlewareKind::RateLimit,
            MiddlewareKind::Auth,
            MiddlewareKind::SecurityHeaders,
            MiddlewareKind::IpAccessControl,
            MiddlewareKind::AuditLog,
            MiddlewareKind::BodySizeLimit,
        ] {
            assert!(
                DEFAULT_ORDER.contains(&kind),
                "DEFAULT_ORDER missing {kind}"
            );
        }
    }

    // ====================================================================
    // PHP_GLOBAL_ORDER PHP 全局顺序对齐
    // ====================================================================

    #[test]
    fn test_php_global_order_length() {
        // PHP app/middleware.php 返回 2 个全局中间件
        assert_eq!(PHP_GLOBAL_ORDER.len(), 2);
    }

    #[test]
    fn test_php_global_order_matches_php_app_middleware() {
        // 对齐 PHP `app/middleware.php`：
        //   \think\middleware\SessionInit::class,
        //   \think\middleware\AllowCrossDomain::class,
        assert_eq!(PHP_GLOBAL_ORDER[0], MiddlewareKind::Trace); // SessionInit → Trace
        assert_eq!(PHP_GLOBAL_ORDER[1], MiddlewareKind::Cors); // AllowCrossDomain → Cors
    }

    #[test]
    fn test_php_global_order_is_subset_of_default() {
        // PHP 全局中间件必须包含在 DEFAULT_ORDER 中
        // （安全中间件插入在全局中间件之间，不再保证前缀关系）
        for kind in PHP_GLOBAL_ORDER {
            assert!(
                DEFAULT_ORDER.contains(kind),
                "DEFAULT_ORDER missing PHP global middleware {kind}"
            );
        }
    }
}
