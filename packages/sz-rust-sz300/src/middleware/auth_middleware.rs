use crate::services::auth_service;
use axum::{
    body::Body,
    http::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

pub async fn auth_middleware(req: Request<Body>, next: Next) -> Response {
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = auth_header.strip_prefix("Bearer ").unwrap_or("");

    // /health, /api/v1/auth/* 等公开路径跳过鉴权
    let path = req.uri().path();
    if path == "/health" || path.starts_with("/api/v1/auth/") {
        return next.run(req).await;
    }

    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "未授权").into_response();
    }

    match auth_service::verify_token(token) {
        Ok(_user) => next.run(req).await,
        Err(_) => (StatusCode::UNAUTHORIZED, "令牌无效或已过期").into_response(),
    }
}
