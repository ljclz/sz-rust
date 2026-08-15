use crate::controllers::{
    ai, auth, capabilities, device, file, file_serve, health, merchant, order, product, view,
};
use crate::middleware::auth_middleware;
use crate::middleware::metrics_auth::{metrics_auth_middleware, ClientIp};
use crate::openapi;
use crate::state::AppState;
use axum::extract::connect_info::ConnectInfo;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use axum::{middleware, routing::get, routing::post, Router};
use std::sync::Arc;
use sz_rust_core::middleware::cors::cors_layer;
use sz_rust_core::middleware::csrf::csrf_middleware;

use sz_rust_middleware_facade::circuit_breaker::{
    circuit_breaker_middleware, CircuitBreaker, CircuitBreakerConfig,
};
use sz_rust_middleware_facade::rate_limit::{rate_limit_middleware, token_bucket_config};

#[cfg(feature = "admin")]
use crate::controllers::admin;
#[cfg(feature = "admin")]
use crate::middleware::role_guard::admin_role_guard;

/// 公开路径白名单 — 跳过 JWT 鉴权的路径（精确匹配，避免前缀绕过）
///
/// 安全说明（2026-07-26 P1 修复）：
/// - 旧版使用 `path.starts_with("/api/v1/auth/")` 前缀匹配，会绕过 `/api/v1/auth/me`、
///   `/api/v1/auth/logout` 等需要鉴权的接口
/// - 新版改为精确匹配，仅 `/api/v1/auth/login` 与 `/api/v1/auth/refresh` 跳过鉴权
pub const PUBLIC_PATHS: &[&str] = &[
    "/health",
    "/health/ready",
    "/health/startup",
    "/metrics",
    "/api/v1/auth/login",
    "/api/v1/auth/refresh",
    "/api-docs",
    "/api-docs/redoc",
    "/api-docs/openapi.json",
];

/// 判断路径是否在公开白名单中（精确匹配，避免前缀绕过）
pub fn is_public_path(path: &str) -> bool {
    PUBLIC_PATHS.contains(&path)
}

/// metrics 端点子路由 — 独立访问控制（T7 MetricsAuthConfig 接线）
///
/// /metrics 在公开路径白名单中（跳过业务 JWT），因此必须由本层独立鉴权：
/// - Bearer token（`SZ300_METRICS_BEARER_TOKEN`）
/// - IP 白名单（`SZ300_METRICS_ALLOWED_IPS`，支持 CIDR）
/// - `SZ300_METRICS_AUTH_ENABLED=false` 可显式关闭（仅限非生产）
///
/// 用独立 Router + route_layer 实现，确保中间件只作用于 /metrics，不污染业务路由。
/// 额外挂一层 `connect_info_to_client_ip` 桥接中间件：生产环境由
/// `into_make_service_with_connect_info` 把 `ConnectInfo<SocketAddr>` 写入 extensions，
/// 本层将其转为 [`ClientIp`]，供 `metrics_auth_middleware` 读取。
pub fn metrics_router() -> Router<AppState> {
    let metrics_auth_cfg = crate::config::MetricsAuthConfig::from_env();
    Router::<AppState>::new()
        .route("/metrics", get(health::metrics))
        // 桥接：ConnectInfo<SocketAddr> → ClientIp（避免多 axum 版本 TypeId 不匹配）
        .layer(middleware::from_fn(connect_info_to_client_ip))
        .route_layer(middleware::from_fn_with_state(
            metrics_auth_cfg,
            metrics_auth_middleware,
        ))
}

/// 将 axum 的 `ConnectInfo<SocketAddr>` 转换为 [`ClientIp`] 写入 extensions。
///
/// 生产环境 `into_make_service_with_connect_info` 在建立连接时注入 `ConnectInfo`，
/// 本中间件读取后以自定义 `ClientIp` 类型重新写入，使 `metrics_auth_middleware`
/// 不直接依赖 `ConnectInfo` 提取器，从而规避依赖图中 axum 0.7/0.8 多版本导致的
/// `Extensions::get` TypeId 不匹配问题。
async fn connect_info_to_client_ip(
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    request
        .extensions_mut()
        .insert(ClientIp(addr.ip().to_string()));
    next.run(request).await
}

/// 创建应用路由表，注册所有业务路由并叠加 CORS + CSRF + JWT 鉴权中间件
///
/// ## 中间件执行顺序（外层→内层）
///
/// 1. `cors_layer`：CORS 跨域处理（最外层，确保预检请求直接返回）
/// 2. `csrf_middleware`：CSRF 双提交 Cookie 校验（公开路径自动放行）
/// 3. `rate_limit_middleware`：限流（令牌桶，健康检查/metrics 排除）
/// 4. `circuit_breaker_middleware`：熔断器（Open 态返回 503）
/// 5. `auth_middleware`：JWT 校验（公开路径自动放行）
/// 6. 业务 handler
///
/// 在 axum/tower 中，`.layer(A).layer(B)` 的执行顺序为 B → A → handler。
/// 因此此处注册顺序为 auth_middleware 在前（内层），csrf_middleware 在后（外层），
/// cors_layer 最后注册（最外层）。
pub fn create_router(state: AppState) -> Router {
    // mut 仅在 admin feature（router.nest）下需要
    #[cfg_attr(not(feature = "admin"), allow(unused_mut))]
    let mut router = Router::new()
        // API 文档（Swagger UI / Redoc / OpenAPI JSON）
        .route("/api-docs", get(openapi::swagger_ui))
        .route("/api-docs/redoc", get(openapi::redoc))
        .route("/api-docs/openapi.json", get(openapi::openapi_json))
        // 健康检查 + 可观测性（/metrics 走独立子路由，见 metrics_router()）
        .route("/health", get(health::check))
        .route("/health/ready", get(health::readiness))
        .route("/health/startup", get(health::startup))
        // 认证（公开接口）
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/refresh", post(auth::refresh))
        .route("/api/v1/auth/me", post(auth::me))
        .route("/api/v1/auth/logout", post(auth::logout))
        // 商户管理
        .route("/api/v1/merchant/list", post(merchant::list))
        .route("/api/v1/merchant/info", post(merchant::info))
        .route("/api/v1/merchant/create", post(merchant::create))
        .route("/api/v1/merchant/update", post(merchant::update))
        .route("/api/v1/merchant/delete", post(merchant::delete))
        // 商品管理
        .route("/api/v1/product/list", post(product::list))
        .route("/api/v1/product/info", post(product::info))
        .route("/api/v1/product/create", post(product::create))
        .route("/api/v1/product/update", post(product::update))
        .route("/api/v1/product/delete", post(product::delete))
        // 设备管理
        .route("/api/v1/device/list", post(device::list))
        .route("/api/v1/device/info", post(device::info))
        .route("/api/v1/device/bind", post(device::bind))
        .route("/api/v1/device/unbind", post(device::unbind))
        .route("/api/v1/device/ota", post(device::trigger_ota))
        .route("/api/v1/device/status_report", post(device::status_report))
        // 订单管理
        .route("/api/v1/order/list", post(order::list))
        .route("/api/v1/order/info", post(order::info))
        .route("/api/v1/order/create", post(order::create))
        // 文件上传
        .route("/api/v1/file/upload", post(file::upload))
        .route(
            "/api/v1/file/upload_multipart",
            post(file::upload_multipart),
        )
        // AI 聊天接口
        .route("/api/v1/ai/chat", post(ai::chat))
        // Capability 注册表查询（AI Agent 能力发现）
        .route("/api/v1/capabilities/list", post(capabilities::list))
        // 视图模板渲染
        .route("/page/{template}", get(view::render_page))
        // 静态文件服务
        .route("/uploads/{*path}", get(file_serve::serve_file));

    // Admin Monitor API（admin feature 门控）
    // 需要 admin 角色才能访问（role_guard 在 auth_middleware 之上叠加角色检查）
    #[cfg(feature = "admin")]
    {
        router = router.nest(
            "/api/admin",
            Router::new()
                .route("/server/info", get(admin::server_info))
                .route("/db/pool", get(admin::db_pool))
                .route("/redis/info", get(admin::redis_info))
                .layer(middleware::from_fn(admin_role_guard))
                .with_state(state.clone()),
        );
    }

    // 接入 ecommerce 插件路由（/api/ecommerce/*）
    // ecommerce handler 通过闭包捕获 EcommerceState，不依赖 AppState
    let ec_state = sz_rust_addons_ecommerce::EcommerceState::default();
    let builder = sz_rust_core::router::RouterBuilder::with_router(router);
    let builder = sz_rust_addons_ecommerce::register_routes(builder, ec_state);
    let router = builder.build();

    // 限流配置（令牌桶，从环境变量读取阈值，健康检查/metrics 排除）
    let rl_config = crate::config::RateLimitProductionConfig::from_env();
    let rate_limit_layer = token_bucket_config(rl_config.capacity, rl_config.refill_per_second)
        .with_exclude_paths(rl_config.exclude_paths)
        .with_key_prefix(rl_config.key_prefix)
        .with_trust_proxy_headers(rl_config.trust_proxy_headers);

    // 熔断器配置（从环境变量读取阈值）
    let cb_config = crate::config::CircuitBreakerProductionConfig::from_env();
    cb_config.validate().expect("熔断配置非法");
    let circuit_breaker = Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
        error_threshold: cb_config.error_threshold,
        cooldown: cb_config.cooldown,
        probe_requests: cb_config.probe_requests,
        stat_window: cb_config.stat_window,
    }));

    router
        .merge(metrics_router())
        // JWT 鉴权中间件（公开路径自动跳过）— 内层
        // 保留自研版：middleware-facade auth 签名不兼容（from_fn vs from_fn_with_state）
        .layer(middleware::from_fn(
            #[allow(deprecated)]
            auth_middleware::auth_middleware,
        ))
        // 熔断器中间件（位于限流之后、auth 之前，Open 态返回 503）
        .layer(middleware::from_fn_with_state(
            circuit_breaker,
            circuit_breaker_middleware,
        ))
        // 限流中间件（令牌桶，auth 之前限流避免无效请求消耗鉴权开销）
        .layer(middleware::from_fn_with_state(
            rate_limit_layer,
            rate_limit_middleware,
        ))
        // CSRF 防护中间件（双提交 Cookie 模式）
        // 公开路径（/health、/metrics、/api/v1/auth/login、/api/v1/auth/refresh）自动放行
        .layer(middleware::from_fn(csrf_middleware))
        // CORS 跨域中间件（最外层，确保预检 OPTIONS 请求不进入鉴权流程）
        // 默认 Allow-Origin: * + 不带 Allow-Credentials（安全默认）
        .layer(cors_layer())
        // 注入共享状态
        .with_state(state)
}


