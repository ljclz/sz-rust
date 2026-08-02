use crate::controllers::{auth, device, file, file_serve, health, merchant, order, product};
use crate::middleware::auth_middleware;
use crate::openapi;
use crate::state::AppState;
use axum::{middleware, routing::get, routing::post, Router};
use sz_rust_core::middleware::cors::cors_layer;
use sz_rust_core::middleware::csrf::csrf_middleware;

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

/// 创建应用路由表，注册所有业务路由并叠加 CORS + CSRF + JWT 鉴权中间件
///
/// ## 中间件执行顺序（外层→内层）
///
/// 1. `cors_layer`：CORS 跨域处理（最外层，确保预检请求直接返回）
/// 2. `csrf_middleware`：CSRF 双提交 Cookie 校验（公开路径自动放行）
/// 3. `auth_middleware`：JWT 校验（公开路径自动放行）
/// 4. 业务 handler
///
/// 在 axum/tower 中，`.layer(A).layer(B)` 的执行顺序为 B → A → handler。
/// 因此此处注册顺序为 auth_middleware 在前（内层），csrf_middleware 在后（外层），
/// cors_layer 最后注册（最外层）。
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // API 文档（Swagger UI / Redoc / OpenAPI JSON）
        .route("/api-docs", get(openapi::swagger_ui))
        .route("/api-docs/redoc", get(openapi::redoc))
        .route("/api-docs/openapi.json", get(openapi::openapi_json))
        // 健康检查 + 可观测性
        .route("/health", get(health::check))
        .route("/health/ready", get(health::readiness))
        .route("/health/startup", get(health::startup))
        .route("/metrics", get(health::metrics))
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
        // 静态文件服务
        .route("/uploads/{*path}", get(file_serve::serve_file))
        // JWT 鉴权中间件（公开路径自动跳过）— 内层
        .layer(middleware::from_fn(auth_middleware::auth_middleware))
        // CSRF 防护中间件（双提交 Cookie 模式）
        // 公开路径（/health、/metrics、/api/v1/auth/login、/api/v1/auth/refresh）自动放行
        .layer(middleware::from_fn(csrf_middleware))
        // CORS 跨域中间件（最外层，确保预检 OPTIONS 请求不进入鉴权流程）
        // 默认 Allow-Origin: * + 不带 Allow-Credentials（安全默认）
        .layer(cors_layer())
        // 注入共享状态
        .with_state(state)
}
