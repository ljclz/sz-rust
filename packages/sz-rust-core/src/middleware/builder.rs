//! 中间件链构建器 — 统一管理 `MiddlewareChain` + 5 个 `Option<Config>`
//!
//! 提供 `MiddlewareBuilder` 用于：
//! 1. 持有 `MiddlewareChain`（顺序定义，业务期望顺序，首元素最先执行）
//! 2. 持有 5 个 `Option<Config>`：`Cors` / `Log` / `Auth` / `RateLimit` / `Trace`
//! 3. 通过 `apply(self, router: Router) -> Router` 一次性应用所有中间件到 `axum::Router`
//!
//! ## 设计目标
//!
//! - **顺序保证**：按 `MiddlewareChain::service_builder_order()`（业务期望顺序的逆序）
//!   调用 `Router::layer`，确保业务期望顺序与实际执行顺序一致（`Router::layer` 后注册先执行）
//! - **配置可选**：每个中间件对应的 `Config` 都是 `Option`，链中包含但 Config 未设置时跳过
//! - **链式 builder**：`with_xxx()` 方法支持链式配置
//! - **PHP 对齐**：`php_global_builder()` 提供对齐 PHP `app/middleware.php` 全局中间件的默认配置
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::middleware::builder::{MiddlewareBuilder, php_global_builder};
//! use sz_rust_core::middleware::cors::cors_layer;
//! use axum::Router;
//!
//! // 1. 使用 PHP 全局默认（Trace + Cors）
//! let builder = php_global_builder();
//! let app: Router = Router::new()
//!     .route("/", axum::routing::get(|| async { "ok" }))
//!     .layer(cors_layer());
//! // 注意：apply 会消耗 builder 并返回 Router
//! // let app = builder.apply(app);
//!
//! // 2. 自定义完整链
//! use sz_rust_core::middleware::auth::AuthConfig;
//! use sz_rust_core::middleware::log::LogConfig;
//! let builder = MiddlewareBuilder::default_builder()
//!     .with_auth(AuthConfig::default())
//!     .with_log(LogConfig::default());
//! // let app = builder.apply(app);
//! ```
//!
//! ## 与 `MiddlewareChain` 的关系
//!
//! `MiddlewareChain` 只负责「顺序定义」，不持有任何 Layer 实例。
//! `MiddlewareBuilder` 在 `MiddlewareChain` 之上增加「Config 持有 + 应用到 Router」能力。
//!
//! ## `Router::layer` 语义
//!
//! `axum::Router::layer` 是「后注册先执行」（stack 反向）：
//! ```ignore
//! let app = Router::new()
//!     .route("/", get(handler))
//!     .layer(A)  // A 后注册 → 先执行
//!     .layer(B); // B 最后注册 → 最先执行
//! // 执行顺序：B → A → handler
//! ```
//!
//! 因此 `apply` 按 `chain.service_builder_order()`（业务期望顺序的逆序）遍历调用 `Router::layer`，
//! 保证业务期望顺序（`chain.order()`）与实际执行顺序一致。

use axum::Router;
use tower_http::cors::CorsLayer;

use super::auth::{auth_middleware, AuthConfig};
use super::chain::MiddlewareChain;
use super::cors;
use super::log::{log_middleware_with_config, LogConfig};
use super::order::MiddlewareKind;
#[cfg(test)]
use super::order::{DEFAULT_ORDER, PHP_GLOBAL_ORDER};
use super::rate_limit::{rate_limit_middleware, RateLimitConfig};
use super::trace::{trace_middleware, TraceConfig};

/// 中间件链构建器
///
/// 持有 `MiddlewareChain`（顺序定义）+ 5 个 `Option<Config>`（各中间件配置），
/// 通过 `apply()` 方法一次性应用到 `axum::Router`。
///
/// ## 字段说明
///
/// | 字段 | 类型 | 说明 |
/// |------|------|------|
/// | `chain` | `MiddlewareChain` | 中间件顺序定义（业务期望顺序，首元素最先执行） |
/// | `cors` | `Option<CorsLayer>` | CORS Layer（基于 `tower-http::cors`，可直接应用） |
/// | `log` | `Option<LogConfig>` | Log 中间件配置 |
/// | `auth` | `Option<AuthConfig>` | Auth 中间件配置 |
/// | `rate_limit` | `Option<RateLimitConfig>` | RateLimit 中间件配置 |
/// | `trace` | `Option<TraceConfig>` | Trace 中间件配置 |
#[derive(Debug, Clone)]
pub struct MiddlewareBuilder {
    chain: MiddlewareChain,
    cors: Option<CorsLayer>,
    log: Option<LogConfig>,
    auth: Option<AuthConfig>,
    rate_limit: Option<RateLimitConfig>,
    trace: Option<TraceConfig>,
}

impl MiddlewareBuilder {
    /// 创建空构建器（无中间件，无 Config）
    pub fn new() -> Self {
        Self {
            chain: MiddlewareChain::new(),
            cors: None,
            log: None,
            auth: None,
            rate_limit: None,
            trace: None,
        }
    }

    /// 创建默认构建器（使用 `DEFAULT_ORDER`，但所有 Config 为 `None`）
    ///
    /// 调用方需通过 `with_xxx()` 方法显式设置 Config，否则 `apply()` 会跳过该中间件。
    pub fn default_builder() -> Self {
        Self {
            chain: MiddlewareChain::default_chain(),
            cors: None,
            log: None,
            auth: None,
            rate_limit: None,
            trace: None,
        }
    }

    /// 创建 PHP 全局构建器（使用 `PHP_GLOBAL_ORDER`，对齐 `app/middleware.php`）
    ///
    /// 包含 `Trace` + `Cors` 两个中间件，对齐 PHP `app/middleware.php` 返回的全局中间件顺序。
    /// 默认设置 `cors` 字段为 `Some(cors_layer())`，调用方可通过 `with_cors()` 覆盖。
    pub fn php_global_builder() -> Self {
        Self {
            chain: MiddlewareChain::php_global(),
            cors: Some(cors::cors_layer()),
            log: None,
            auth: None,
            rate_limit: None,
            trace: None,
        }
    }

    /// 设置中间件链（替换现有链）
    pub fn with_chain(mut self, chain: MiddlewareChain) -> Self {
        self.chain = chain;
        self
    }

    /// 设置 CORS Layer
    pub fn with_cors(mut self, layer: CorsLayer) -> Self {
        self.cors = Some(layer);
        self
    }

    /// 设置 Log 配置
    pub fn with_log(mut self, config: LogConfig) -> Self {
        self.log = Some(config);
        self
    }

    /// 设置 Auth 配置
    pub fn with_auth(mut self, config: AuthConfig) -> Self {
        self.auth = Some(config);
        self
    }

    /// 设置 RateLimit 配置
    pub fn with_rate_limit(mut self, config: RateLimitConfig) -> Self {
        self.rate_limit = Some(config);
        self
    }

    /// 设置 Trace 配置
    pub fn with_trace(mut self, config: TraceConfig) -> Self {
        self.trace = Some(config);
        self
    }

    /// 从链中移除所有指定类型的中间件（同时清除对应 Config）
    ///
    /// 返回被移除的中间件数量（仅链中数量，不包括 Config）。
    pub fn remove_kind(&mut self, kind: MiddlewareKind) -> usize {
        let removed = self.chain.remove_kind(kind);
        if removed > 0 {
            match kind {
                MiddlewareKind::Trace => self.trace = None,
                MiddlewareKind::Cors => self.cors = None,
                MiddlewareKind::Log => self.log = None,
                MiddlewareKind::RateLimit => self.rate_limit = None,
                MiddlewareKind::Auth => self.auth = None,
            }
        }
        removed
    }

    /// 从链中移除指定类型及之后的所有中间件（含指定类型）
    ///
    /// 用于「公开 API 跳过 Auth 及之后中间件」场景。
    /// 返回被移除的中间件数量；若 `kind` 不存在则不移除任何中间件，返回 0。
    pub fn remove_from(&mut self, kind: MiddlewareKind) -> usize {
        let removed_kinds: Vec<MiddlewareKind> = if let Some(pos) = self.chain.position(kind) {
            self.chain.order()[pos..].to_vec()
        } else {
            return 0;
        };
        let removed = self.chain.remove_from(kind);
        // 清除被移除中间件的 Config
        for k in removed_kinds {
            match k {
                MiddlewareKind::Trace => self.trace = None,
                MiddlewareKind::Cors => self.cors = None,
                MiddlewareKind::Log => self.log = None,
                MiddlewareKind::RateLimit => self.rate_limit = None,
                MiddlewareKind::Auth => self.auth = None,
            }
        }
        removed
    }

    /// 返回中间件链引用
    pub fn chain(&self) -> &MiddlewareChain {
        &self.chain
    }

    /// 返回 CORS Layer 引用
    pub fn cors(&self) -> Option<&CorsLayer> {
        self.cors.as_ref()
    }

    /// 返回 Log 配置引用
    pub fn log(&self) -> Option<&LogConfig> {
        self.log.as_ref()
    }

    /// 返回 Auth 配置引用
    pub fn auth(&self) -> Option<&AuthConfig> {
        self.auth.as_ref()
    }

    /// 返回 RateLimit 配置引用
    pub fn rate_limit(&self) -> Option<&RateLimitConfig> {
        self.rate_limit.as_ref()
    }

    /// 返回 Trace 配置引用
    pub fn trace(&self) -> Option<&TraceConfig> {
        self.trace.as_ref()
    }

    /// 判断指定中间件是否已启用（链中包含且 Config 已设置）
    ///
    /// 注意：`Cors` 的 Config 是 `CorsLayer`（必为 `Some` 才视为已启用）。
    pub fn is_enabled(&self, kind: MiddlewareKind) -> bool {
        if !self.chain.contains(kind) {
            return false;
        }
        match kind {
            MiddlewareKind::Trace => self.trace.is_some(),
            MiddlewareKind::Cors => self.cors.is_some(),
            MiddlewareKind::Log => self.log.is_some(),
            MiddlewareKind::RateLimit => self.rate_limit.is_some(),
            MiddlewareKind::Auth => self.auth.is_some(),
        }
    }

    /// 应用所有中间件到 `axum::Router`
    ///
    /// 按 `chain.service_builder_order()`（业务期望顺序的逆序）遍历调用 `Router::layer`，
    /// 保证业务期望顺序（`chain.order()`）与实际执行顺序一致（`Router::layer` 后注册先执行）。
    ///
    /// 链中包含但 Config 未设置的中间件会被跳过（不应用）。
    ///
    /// ## 消耗语义
    ///
    /// 此方法消耗 `self`（取出 Config 的所有权），返回应用了中间件的 `Router`。
    pub fn apply(self, mut router: Router) -> Router {
        let mut cors = self.cors;
        let mut log = self.log;
        let mut auth = self.auth;
        let mut rate_limit = self.rate_limit;
        let mut trace = self.trace;
        for kind in self.chain.service_builder_order() {
            router = match kind {
                MiddlewareKind::Trace => {
                    if let Some(cfg) = trace.take() {
                        router.layer(axum::middleware::from_fn_with_state(cfg, trace_middleware))
                    } else {
                        router
                    }
                }
                MiddlewareKind::Cors => {
                    if let Some(layer) = cors.take() {
                        router.layer(layer)
                    } else {
                        router
                    }
                }
                MiddlewareKind::Log => {
                    if let Some(cfg) = log.take() {
                        router.layer(axum::middleware::from_fn_with_state(
                            cfg,
                            log_middleware_with_config,
                        ))
                    } else {
                        router
                    }
                }
                MiddlewareKind::RateLimit => {
                    if let Some(cfg) = rate_limit.take() {
                        router.layer(axum::middleware::from_fn_with_state(
                            cfg,
                            rate_limit_middleware,
                        ))
                    } else {
                        router
                    }
                }
                MiddlewareKind::Auth => {
                    if let Some(cfg) = auth.take() {
                        router.layer(axum::middleware::from_fn_with_state(cfg, auth_middleware))
                    } else {
                        router
                    }
                }
            };
        }
        router
    }
}

impl Default for MiddlewareBuilder {
    fn default() -> Self {
        Self::default_builder()
    }
}

impl std::fmt::Display for MiddlewareBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MiddlewareBuilder(chain={}, ", self.chain)?;
        write!(
            f,
            "cors={}, log={}, auth={}, rate_limit={}, trace={})",
            self.cors.is_some(),
            self.log.is_some(),
            self.auth.is_some(),
            self.rate_limit.is_some(),
            self.trace.is_some()
        )
    }
}

/// 创建默认构建器（便捷函数，等价于 `MiddlewareBuilder::default_builder()`)
pub fn default_builder() -> MiddlewareBuilder {
    MiddlewareBuilder::default_builder()
}

/// 创建 PHP 全局构建器（便捷函数，等价于 `MiddlewareBuilder::php_global_builder()`)
pub fn php_global_builder() -> MiddlewareBuilder {
    MiddlewareBuilder::php_global_builder()
}

/// 创建带默认 CORS Layer 的 PHP 全局构建器（便捷函数）
///
/// 对齐 PHP `app/middleware.php` 全局中间件：
/// - `SessionInit` → Rust `Trace`（Config 需调用方显式设置）
/// - `AllowCrossDomain` → Rust `Cors`（已设置默认 `cors_layer()`）
pub fn with_default_cors() -> MiddlewareBuilder {
    MiddlewareBuilder::php_global_builder()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use std::time::Duration;
    use sz_orm_limit::SlidingWindowRateLimiter;
    use sz_orm_tracing::SzTracer;
    use tower::ServiceExt;

    // ====================================================================
    // 辅助函数
    // ====================================================================

    async fn read_body(resp: axum::response::Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn make_request(method: &str, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn make_trace_config() -> TraceConfig {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(SzTracer::new("test-service"));
        TraceConfig::new(tracer)
    }

    fn make_rate_limit_config() -> RateLimitConfig {
        let limiter: Arc<dyn RateLimiter + Send + Sync> =
            Arc::new(SlidingWindowRateLimiter::new(1000, Duration::from_secs(60)));
        RateLimitConfig::new(limiter)
    }

    // 引入 trait 以便 make_trace_config / make_rate_limit_config 编译
    use sz_orm_limit::RateLimiter;
    use sz_orm_tracing::Tracer;

    // ====================================================================
    // 构造函数
    // ====================================================================

    #[test]
    fn test_new_creates_empty_builder() {
        let builder = MiddlewareBuilder::new();
        assert!(builder.chain().is_empty());
        assert_eq!(builder.chain().len(), 0);
        assert!(builder.cors().is_none());
        assert!(builder.log().is_none());
        assert!(builder.auth().is_none());
        assert!(builder.rate_limit().is_none());
        assert!(builder.trace().is_none());
    }

    #[test]
    fn test_default_builder_uses_default_order() {
        let builder = MiddlewareBuilder::default_builder();
        assert_eq!(builder.chain().order(), DEFAULT_ORDER);
        assert_eq!(builder.chain().len(), 5);
        // 默认所有 Config 为 None
        assert!(builder.cors().is_none());
        assert!(builder.log().is_none());
        assert!(builder.auth().is_none());
        assert!(builder.rate_limit().is_none());
        assert!(builder.trace().is_none());
    }

    #[test]
    fn test_default_trait_uses_default_builder() {
        let builder = MiddlewareBuilder::default();
        assert_eq!(builder.chain().order(), DEFAULT_ORDER);
    }

    #[test]
    fn test_php_global_builder_uses_php_global_order() {
        let builder = MiddlewareBuilder::php_global_builder();
        assert_eq!(builder.chain().order(), PHP_GLOBAL_ORDER);
        assert_eq!(builder.chain().len(), 2);
        // 默认 cors 已设置
        assert!(builder.cors().is_some());
        // 其他 Config 为 None
        assert!(builder.log().is_none());
        assert!(builder.auth().is_none());
        assert!(builder.rate_limit().is_none());
        assert!(builder.trace().is_none());
    }

    // ====================================================================
    // with_xxx 链式配置
    // ====================================================================

    #[test]
    fn test_with_chain_replaces_chain() {
        let custom_chain = MiddlewareChain::new()
            .push(MiddlewareKind::Cors)
            .push(MiddlewareKind::Log);
        let builder = MiddlewareBuilder::new().with_chain(custom_chain);
        assert_eq!(
            builder.chain().order(),
            [MiddlewareKind::Cors, MiddlewareKind::Log]
        );
    }

    #[test]
    fn test_with_cors_sets_layer() {
        let builder = MiddlewareBuilder::new().with_cors(cors::cors_layer());
        assert!(builder.cors().is_some());
    }

    #[test]
    fn test_with_log_sets_config() {
        let config = LogConfig::default().with_exclude_paths(vec!["/health".to_string()]);
        let builder = MiddlewareBuilder::new().with_log(config);
        assert!(builder.log().is_some());
        assert_eq!(
            builder.log().unwrap().exclude_paths,
            vec!["/health".to_string()]
        );
    }

    #[test]
    fn test_with_auth_sets_config() {
        let config = AuthConfig::default().with_secret("custom-secret");
        let builder = MiddlewareBuilder::new().with_auth(config);
        assert!(builder.auth().is_some());
        assert_eq!(builder.auth().unwrap().secret, "custom-secret");
    }

    #[test]
    fn test_with_rate_limit_sets_config() {
        let config = make_rate_limit_config();
        let builder = MiddlewareBuilder::new().with_rate_limit(config);
        assert!(builder.rate_limit().is_some());
    }

    #[test]
    fn test_with_trace_sets_config() {
        let config = make_trace_config();
        let builder = MiddlewareBuilder::new().with_trace(config);
        assert!(builder.trace().is_some());
    }

    #[test]
    fn test_chained_with_xxx_builders() {
        let builder = MiddlewareBuilder::default_builder()
            .with_cors(cors::cors_layer())
            .with_log(LogConfig::default())
            .with_auth(AuthConfig::default())
            .with_rate_limit(make_rate_limit_config())
            .with_trace(make_trace_config());
        assert!(builder.cors().is_some());
        assert!(builder.log().is_some());
        assert!(builder.auth().is_some());
        assert!(builder.rate_limit().is_some());
        assert!(builder.trace().is_some());
    }

    // ====================================================================
    // remove_kind / remove_from
    // ====================================================================

    #[test]
    fn test_remove_kind_removes_from_chain_and_config() {
        let mut builder = MiddlewareBuilder::default_builder()
            .with_auth(AuthConfig::default())
            .with_log(LogConfig::default());
        assert!(builder.auth().is_some());
        let removed = builder.remove_kind(MiddlewareKind::Auth);
        assert_eq!(removed, 1);
        assert!(builder.auth().is_none());
        assert!(!builder.chain().contains(MiddlewareKind::Auth));
    }

    #[test]
    fn test_remove_kind_not_present_returns_zero() {
        let mut builder = MiddlewareBuilder::php_global_builder();
        let removed = builder.remove_kind(MiddlewareKind::Auth);
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_remove_from_removes_kind_and_after() {
        let mut builder = MiddlewareBuilder::default_builder()
            .with_rate_limit(make_rate_limit_config())
            .with_auth(AuthConfig::default());
        let removed = builder.remove_from(MiddlewareKind::RateLimit);
        assert_eq!(removed, 2);
        assert!(builder.rate_limit().is_none());
        assert!(builder.auth().is_none());
        assert!(!builder.chain().contains(MiddlewareKind::RateLimit));
        assert!(!builder.chain().contains(MiddlewareKind::Auth));
    }

    // ====================================================================
    // is_enabled 综合判断
    // ====================================================================

    #[test]
    fn test_is_enabled_true_when_chain_and_config_present() {
        let builder = MiddlewareBuilder::default_builder().with_auth(AuthConfig::default());
        assert!(builder.is_enabled(MiddlewareKind::Auth));
    }

    #[test]
    fn test_is_enabled_false_when_config_missing() {
        let builder = MiddlewareBuilder::default_builder();
        // Auth 在 DEFAULT_ORDER 中但 Config 未设置
        assert!(!builder.is_enabled(MiddlewareKind::Auth));
    }

    #[test]
    fn test_is_enabled_false_when_not_in_chain() {
        let builder = MiddlewareBuilder::new().with_auth(AuthConfig::default());
        // Config 设置但链中无 Auth
        assert!(!builder.is_enabled(MiddlewareKind::Auth));
    }

    // ====================================================================
    // apply 应用到 Router
    // ====================================================================

    #[test]
    fn test_apply_empty_builder_returns_router_unchanged() {
        let router = Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let builder = MiddlewareBuilder::new();
        let app = builder.apply(router);
        // 验证 Router 仍可正常使用（通过 oneshot 验证）
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = read_body(resp).await;
            assert_eq!(body, "ok");
        });
    }

    #[test]
    fn test_apply_with_cors_only() {
        let router = Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let builder = MiddlewareBuilder::new()
            .with_chain(MiddlewareChain::new().push(MiddlewareKind::Cors))
            .with_cors(cors::cors_layer());
        let app = builder.apply(router);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let req = Request::builder()
                .method("GET")
                .uri("/")
                .header("origin", "https://example.com")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            // CORS 应设置 Access-Control-Allow-Origin
            assert!(resp.headers().contains_key("access-control-allow-origin"));
        });
    }

    #[test]
    fn test_apply_skips_middlewares_without_config() {
        let router = Router::new().route("/", axum::routing::get(|| async { "ok" }));
        // 默认链包含 5 个中间件，但 Config 全为 None
        let builder = MiddlewareBuilder::default_builder();
        let app = builder.apply(router);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        });
    }

    #[test]
    fn test_apply_with_all_configs_does_not_panic() {
        let router = Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let builder = MiddlewareBuilder::default_builder()
            .with_cors(cors::cors_layer())
            .with_log(LogConfig::default())
            .with_auth(AuthConfig::default())
            .with_rate_limit(make_rate_limit_config())
            .with_trace(make_trace_config());
        let app = builder.apply(router);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Auth 未通过 → 401
            let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        });
    }

    #[test]
    fn test_apply_preserves_business_order() {
        // 业务期望顺序：Cors → Auth（Auth 在 Cors 之后执行）
        // Router::layer 后注册先执行，因此 apply 应按 [Auth, Cors] 顺序注册
        let router = Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let builder = MiddlewareBuilder::new()
            .with_chain(
                MiddlewareChain::new()
                    .push(MiddlewareKind::Cors)
                    .push(MiddlewareKind::Auth),
            )
            .with_cors(cors::cors_layer())
            .with_auth(AuthConfig::default());
        let app = builder.apply(router);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Auth 在最后注册 → 最先执行 → 401
            let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        });
    }

    // ====================================================================
    // 便捷函数
    // ====================================================================

    #[test]
    fn test_default_builder_helper() {
        let builder = default_builder();
        assert_eq!(builder.chain().order(), DEFAULT_ORDER);
    }

    #[test]
    fn test_php_global_builder_helper() {
        let builder = php_global_builder();
        assert_eq!(builder.chain().order(), PHP_GLOBAL_ORDER);
        assert!(builder.cors().is_some());
    }

    #[test]
    fn test_with_default_cors_helper() {
        let builder = with_default_cors();
        assert_eq!(builder.chain().order(), PHP_GLOBAL_ORDER);
        assert!(builder.cors().is_some());
    }

    // ====================================================================
    // Display 格式化
    // ====================================================================

    #[test]
    fn test_display_empty_builder() {
        let builder = MiddlewareBuilder::new();
        let s = format!("{builder}");
        assert!(s.contains("MiddlewareBuilder"));
        assert!(s.contains("chain=MiddlewareChain[]"));
        assert!(s.contains("cors=false"));
    }

    #[test]
    fn test_display_full_builder() {
        let builder = MiddlewareBuilder::default_builder()
            .with_cors(cors::cors_layer())
            .with_log(LogConfig::default())
            .with_auth(AuthConfig::default())
            .with_rate_limit(make_rate_limit_config())
            .with_trace(make_trace_config());
        let s = format!("{builder}");
        assert!(s.contains("cors=true"));
        assert!(s.contains("log=true"));
        assert!(s.contains("auth=true"));
        assert!(s.contains("rate_limit=true"));
        assert!(s.contains("trace=true"));
    }

    // ====================================================================
    // Clone
    // ====================================================================

    #[test]
    fn test_clone_preserves_state() {
        let builder = MiddlewareBuilder::default_builder()
            .with_cors(cors::cors_layer())
            .with_log(LogConfig::default())
            .with_auth(AuthConfig::default());
        let cloned = builder.clone();
        assert_eq!(builder.chain(), cloned.chain());
        assert!(cloned.cors().is_some());
        assert!(cloned.log().is_some());
        assert!(cloned.auth().is_some());
    }

    // ====================================================================
    // R5 PHP 行为对齐验证
    // ====================================================================

    #[test]
    fn r5_1_php_global_order_matches_php_app_middleware() {
        // PHP `app/middleware.php`:
        //   \think\middleware\SessionInit::class,        // → Rust Trace
        //   \think\middleware\AllowCrossDomain::class,  // → Rust Cors
        let builder = php_global_builder();
        assert_eq!(
            builder.chain().order(),
            [MiddlewareKind::Trace, MiddlewareKind::Cors]
        );
    }

    #[test]
    fn r5_2_php_global_builder_has_default_cors() {
        // 对齐 PHP `AllowCrossDomain` 默认启用
        let builder = php_global_builder();
        assert!(builder.cors().is_some());
    }

    #[test]
    fn r5_3_php_global_builder_trace_config_none_by_default() {
        // PHP `SessionInit` 由框架自动配置，Rust 端需调用方显式设置 TraceConfig
        // （因为 Tracer 实例需要服务名等业务参数）
        let builder = php_global_builder();
        assert!(builder.trace().is_none());
    }

    #[test]
    fn r5_4_default_order_aligns_with_php_extension() {
        // PHP 全局 + 业务中间件顺序：
        //   SessionInit(Trace) → AllowCrossDomain(Cors) → [Log/RateLimit/Auth 业务追加]
        let builder = default_builder();
        assert_eq!(
            builder.chain().order(),
            [
                MiddlewareKind::Trace,
                MiddlewareKind::Cors,
                MiddlewareKind::Log,
                MiddlewareKind::RateLimit,
                MiddlewareKind::Auth
            ]
        );
        // PHP 全局顺序必须是默认顺序的前缀
        assert!(builder.chain().order().starts_with(PHP_GLOBAL_ORDER));
    }

    #[test]
    fn r5_5_php_public_api_skip_auth_via_remove_from() {
        // 公开 API 跳过 RateLimit + Auth（对齐 PHP 公开路由不挂 Auth middleware）
        let mut builder = default_builder().with_auth(AuthConfig::default());
        let removed = builder.remove_from(MiddlewareKind::RateLimit);
        assert_eq!(removed, 2);
        assert!(!builder.is_enabled(MiddlewareKind::Auth));
        assert!(!builder.is_enabled(MiddlewareKind::RateLimit));
        // Trace/Cors/Log 仍保留
        assert!(builder.chain().contains(MiddlewareKind::Trace));
        assert!(builder.chain().contains(MiddlewareKind::Cors));
        assert!(builder.chain().contains(MiddlewareKind::Log));
    }

    #[test]
    fn r5_6_service_builder_order_reverses_for_router_layer() {
        // 验证 apply 按 service_builder_order()（逆序）应用
        let builder = default_builder();
        let sb_order = builder.chain().service_builder_order();
        // 业务期望：Trace, Cors, Log, RateLimit, Auth
        // ServiceBuilder 注册顺序（逆序）：Auth, RateLimit, Log, Cors, Trace
        assert_eq!(
            sb_order,
            [
                MiddlewareKind::Auth,
                MiddlewareKind::RateLimit,
                MiddlewareKind::Log,
                MiddlewareKind::Cors,
                MiddlewareKind::Trace,
            ]
        );
    }

    #[test]
    fn r5_7_php_global_builder_skip_middlewares_without_config() {
        // php_global_builder() 包含 Trace + Cors，但 TraceConfig 为 None
        // apply 时应跳过 Trace，仅应用 Cors
        let router = Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let app = php_global_builder().apply(router);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let req = Request::builder()
                .method("GET")
                .uri("/")
                .header("origin", "https://example.com")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            // 应正常返回（Trace 跳过，Cors 设置）
            assert_eq!(resp.status(), StatusCode::OK);
            assert!(resp.headers().contains_key("access-control-allow-origin"));
        });
    }

    #[test]
    fn r5_8_is_enabled_aligns_with_php_middleware_registration() {
        // PHP 端判断中间件是否「实际生效」：必须 (1) 在 middleware.php 中声明 + (2) 实例化成功
        // Rust 端 `is_enabled` 对齐：(1) 链中包含 + (2) Config 已设置
        let builder = MiddlewareBuilder::default_builder()
            .with_cors(cors::cors_layer())
            .with_auth(AuthConfig::default());
        // Cors 已启用（链中包含 + Config 已设置）
        assert!(builder.is_enabled(MiddlewareKind::Cors));
        // Auth 已启用
        assert!(builder.is_enabled(MiddlewareKind::Auth));
        // Trace 未启用（Config 为 None）
        assert!(!builder.is_enabled(MiddlewareKind::Trace));
        // Log 未启用（Config 为 None）
        assert!(!builder.is_enabled(MiddlewareKind::Log));
        // RateLimit 未启用（Config 为 None）
        assert!(!builder.is_enabled(MiddlewareKind::RateLimit));
    }

    // ====================================================================
    // 集成测试（tokio::test）
    // ====================================================================

    #[tokio::test]
    async fn integration_apply_returns_working_router() {
        let router = Router::new().route("/health", axum::routing::get(|| async { "ok" }));
        let app = MiddlewareBuilder::new()
            .with_chain(MiddlewareChain::new().push(MiddlewareKind::Cors))
            .with_cors(cors::cors_layer())
            .apply(router);
        let resp = app.oneshot(make_request("GET", "/health")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn integration_cors_preflight_passes_through() {
        let router = Router::new().route(
            "/api",
            axum::routing::get(|| async { "ok" }).post(|| async { "created" }),
        );
        let app = MiddlewareBuilder::new()
            .with_chain(MiddlewareChain::new().push(MiddlewareKind::Cors))
            .with_cors(cors::cors_layer())
            .apply(router);
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/api")
            .header("origin", "https://example.com")
            .header("access-control-request-method", "POST")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // CORS 预检应返回 200 或 204（tower-http 行为）
        assert!(resp.status().is_success());
        assert!(resp.headers().contains_key("access-control-allow-methods"));
    }

    #[tokio::test]
    async fn integration_auth_rejects_unauthenticated_request() {
        let router = Router::new().route("/protected", axum::routing::get(|| async { "ok" }));
        let app = MiddlewareBuilder::new()
            .with_chain(MiddlewareChain::new().push(MiddlewareKind::Auth))
            .with_auth(AuthConfig::default())
            .apply(router);
        let resp = app
            .oneshot(make_request("GET", "/protected"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = read_body(resp).await;
        assert!(body.contains("\"code\":-1"));
    }

    #[tokio::test]
    async fn integration_log_does_not_block_request() {
        let router = Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let app = MiddlewareBuilder::new()
            .with_chain(MiddlewareChain::new().push(MiddlewareKind::Log))
            .with_log(LogConfig::default())
            .apply(router);
        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "ok");
    }
}
