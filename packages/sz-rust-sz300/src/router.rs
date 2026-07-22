use axum::{Router, routing::post, routing::get, middleware};
use crate::controllers::{health, auth, merchant, product, device, order, file, file_serve};
use crate::middleware::auth_middleware;
use crate::state::AppState;

pub fn create_router(state: AppState) -> Router {
    Router::new()
        // 健康检查
        .route("/health", get(health::check))
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
        // 文件上传（Phase 5.5）
        .route("/api/v1/file/upload", post(file::upload))
        .route("/api/v1/file/upload_multipart", post(file::upload_multipart))
        // 静态文件服务
        .route("/uploads/{*path}", get(file_serve::serve_file))
        // JWT 鉴权中间件（公开路径自动跳过）
        .layer(middleware::from_fn(auth_middleware::auth_middleware))
        // 注入共享状态
        .with_state(state)
}
