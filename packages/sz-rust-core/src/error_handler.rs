//! 错误处理 — 404/500 标准化 JSON 响应
//!
//! 对齐 PHP 异常处理：所有错误响应统一返回 `{code:0, msg, data:{}}` JSON 结构。
//!
//! ## 功能
//!
//! - [`not_found_handler`]：404 处理器，返回 `{code:0, msg:"Not Found", data:{}}`
//! - [`internal_error_handler`]：500 处理器，返回 `{code:0, msg:"Internal Error", data:{}}`
//! - [`method_not_allowed_handler`]：405 处理器
//! - [`error_router`]：预配置的错误处理 Router（包含 fallback + 500 处理）
//! - [`HandleError`]：自定义错误类型，可转换为标准 JSON 响应
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::error_handler::error_router;
//! use axum::Router;
//!
//! let app: Router = Router::new()
//!     .route("/api", axum::routing::get(|| async { "ok" }))
//!     .merge(error_router());
//! ```

use crate::response::ApiResponse;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::json;

/// 404 处理器 — 返回 `{code:0, msg:"Not Found", data:{}}`
///
/// 用于 `Router::fallback`。
pub async fn not_found_handler() -> Response {
    let resp = ApiResponse::error("Not Found");
    let body = resp.to_json_string();
    (
        StatusCode::NOT_FOUND,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// 500 处理器 — 返回 `{code:0, msg:"Internal Error", data:{}}`
pub async fn internal_error_handler() -> Response {
    let resp = ApiResponse::error("Internal Error");
    let body = resp.to_json_string();
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// 405 处理器 — 返回 `{code:0, msg:"Method Not Allowed", data:{}}`
pub async fn method_not_allowed_handler() -> Response {
    let resp = ApiResponse::error("Method Not Allowed");
    let body = resp.to_json_string();
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// 400 处理器 — 返回 `{code:0, msg:"Bad Request", data:{}}`
pub async fn bad_request_handler() -> Response {
    let resp = ApiResponse::error("Bad Request");
    let body = resp.to_json_string();
    (
        StatusCode::BAD_REQUEST,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// 401 处理器 — 返回 `{code:-1, msg:"Unauthorized", data:{}}`
///
/// 对齐 PHP `BaseException` 中的未登录 code=-1。
pub async fn unauthorized_handler() -> Response {
    let resp = ApiResponse::error_with_code(-1, "Unauthorized", json!({}));
    let body = resp.to_json_string();
    (
        StatusCode::UNAUTHORIZED,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// 403 处理器 — 返回 `{code:0, msg:"Forbidden", data:{}}`
pub async fn forbidden_handler() -> Response {
    let resp = ApiResponse::error("Forbidden");
    let body = resp.to_json_string();
    (
        StatusCode::FORBIDDEN,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// 422 处理器 — 返回 `{code:0, msg:"Unprocessable Entity", data:{}}`
pub async fn unprocessable_entity_handler() -> Response {
    let resp = ApiResponse::error("Unprocessable Entity");
    let body = resp.to_json_string();
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// 构建指定状态码的错误响应
pub fn error_response(status: StatusCode, msg: &str) -> Response {
    let resp = ApiResponse::error(msg);
    let body = resp.to_json_string();
    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// 构建带自定义业务 code 的错误响应
pub fn error_response_with_code(status: StatusCode, code: i32, msg: &str) -> Response {
    let resp = ApiResponse::error_with_code(code, msg, json!({}));
    let body = resp.to_json_string();
    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// 自定义错误类型，可转换为标准 JSON 响应
#[derive(Debug, Clone)]
pub struct HandleError {
    /// HTTP 状态码
    pub status: StatusCode,
    /// 业务错误码
    pub code: i32,
    /// 错误信息
    pub msg: String,
}

impl HandleError {
    /// 创建一个新的错误实例
    pub fn new(status: StatusCode, code: i32, msg: impl Into<String>) -> Self {
        Self {
            status,
            code,
            msg: msg.into(),
        }
    }

    /// 创建 404 未找到错误
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, 0, msg)
    }

    /// 创建 500 服务器内部错误
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, 0, msg)
    }

    /// 创建 400 错误请求错误
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, 0, msg)
    }

    /// 创建 401 未授权错误
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, -1, msg)
    }

    /// 创建 403 禁止访问错误
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, 0, msg)
    }
}

impl IntoResponse for HandleError {
    fn into_response(self) -> Response {
        error_response_with_code(self.status, self.code, &self.msg)
    }
}

/// 创建预配置的错误处理 Router
///
/// - 设置 404 fallback
/// - 其他错误码通过 `HandleError` 或单独的处理器处理
pub fn error_router() -> Router {
    Router::new().fallback(not_found_handler)
}

/// 创建 404 fallback Router（用于 `Router::merge`）
pub fn fallback_router() -> Router {
    Router::new().fallback(not_found_handler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    async fn fetch_json(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        (status, json)
    }

    async fn send_get(router: Router, uri: &str) -> Response {
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        router.oneshot(req).await.unwrap()
    }

    // ====================================================================
    // not_found_handler
    // ====================================================================

    #[tokio::test]
    async fn test_not_found_handler_status_code() {
        let resp = not_found_handler().await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_not_found_handler_body() {
        let resp = not_found_handler().await;
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "Not Found");
        assert!(json["data"].is_object());
    }

    #[tokio::test]
    async fn test_not_found_handler_content_type() {
        let resp = not_found_handler().await;
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
    }

    // ====================================================================
    // internal_error_handler
    // ====================================================================

    #[tokio::test]
    async fn test_internal_error_handler() {
        let resp = internal_error_handler().await;
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "Internal Error");
    }

    // ====================================================================
    // method_not_allowed_handler
    // ====================================================================

    #[tokio::test]
    async fn test_method_not_allowed_handler() {
        let resp = method_not_allowed_handler().await;
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "Method Not Allowed");
    }

    // ====================================================================
    // bad_request_handler
    // ====================================================================

    #[tokio::test]
    async fn test_bad_request_handler() {
        let resp = bad_request_handler().await;
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "Bad Request");
    }

    // ====================================================================
    // unauthorized_handler
    // ====================================================================

    #[tokio::test]
    async fn test_unauthorized_handler() {
        let resp = unauthorized_handler().await;
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // 对齐 PHP BaseException: code=-1
        assert_eq!(json["code"], -1);
        assert_eq!(json["msg"], "Unauthorized");
    }

    // ====================================================================
    // forbidden_handler
    // ====================================================================

    #[tokio::test]
    async fn test_forbidden_handler() {
        let resp = forbidden_handler().await;
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "Forbidden");
    }

    // ====================================================================
    // unprocessable_entity_handler
    // ====================================================================

    #[tokio::test]
    async fn test_unprocessable_entity_handler() {
        let resp = unprocessable_entity_handler().await;
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "Unprocessable Entity");
    }

    // ====================================================================
    // error_response / error_response_with_code
    // ====================================================================

    #[tokio::test]
    async fn test_error_response_custom() {
        let resp = error_response(StatusCode::NOT_FOUND, "资源不存在");
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "资源不存在");
    }

    #[tokio::test]
    async fn test_error_response_with_code_custom() {
        let resp = error_response_with_code(StatusCode::BAD_REQUEST, 1001, "参数错误");
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["code"], 1001);
        assert_eq!(json["msg"], "参数错误");
    }

    // ====================================================================
    // HandleError 类型
    // ====================================================================

    #[tokio::test]
    async fn test_handle_error_not_found() {
        let err = HandleError::not_found("用户不存在");
        let resp = err.into_response();
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "用户不存在");
    }

    #[tokio::test]
    async fn test_handle_error_internal() {
        let err = HandleError::internal("数据库错误");
        let resp = err.into_response();
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "数据库错误");
    }

    #[tokio::test]
    async fn test_handle_error_bad_request() {
        let err = HandleError::bad_request("参数错误");
        let resp = err.into_response();
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "参数错误");
    }

    #[tokio::test]
    async fn test_handle_error_unauthorized() {
        let err = HandleError::unauthorized("请先登录");
        let resp = err.into_response();
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["code"], -1);
        assert_eq!(json["msg"], "请先登录");
    }

    #[tokio::test]
    async fn test_handle_error_forbidden() {
        let err = HandleError::forbidden("无权限");
        let resp = err.into_response();
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "无权限");
    }

    #[tokio::test]
    async fn test_handle_error_custom() {
        let err = HandleError::new(StatusCode::CONFLICT, 4091, "冲突");
        let resp = err.into_response();
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json["code"], 4091);
        assert_eq!(json["msg"], "冲突");
    }

    #[test]
    fn test_handle_error_clone_debug() {
        let err = HandleError::not_found("test");
        let cloned = err.clone();
        assert_eq!(cloned.msg, "test");
        let debug = format!("{err:?}");
        assert!(debug.contains("HandleError"));
    }

    // ====================================================================
    // Router 集成
    // ====================================================================

    #[tokio::test]
    async fn test_error_router_404_fallback() {
        let router: Router = Router::new()
            .route("/api", axum::routing::get(|| async { "ok" }))
            .merge(error_router());

        // 已注册路由
        let resp = send_get(router.clone(), "/api").await;
        assert_eq!(resp.status(), StatusCode::OK);

        // 未注册路由 → 404
        let resp = send_get(router, "/nonexistent").await;
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "Not Found");
    }

    #[tokio::test]
    async fn test_fallback_router_404() {
        let router: Router = Router::new()
            .route("/api", axum::routing::get(|| async { "ok" }))
            .merge(fallback_router());

        let resp = send_get(router, "/nonexistent").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ====================================================================
    // 作为 handler 返回值
    // ====================================================================

    #[tokio::test]
    async fn test_handle_error_as_handler_return() {
        async fn handler() -> Result<String, HandleError> {
            Err(HandleError::not_found("用户不存在"))
        }

        let router: Router = Router::new().route("/users/{id}", axum::routing::get(handler));

        let req = Request::builder()
            .method(Method::GET)
            .uri("/users/999")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["msg"], "用户不存在");
    }

    #[tokio::test]
    async fn test_handle_error_success_path() {
        async fn handler() -> Result<String, HandleError> {
            Ok("success".to_string())
        }

        let router: Router = Router::new().route("/ok", axum::routing::get(handler));

        let resp = send_get(router, "/ok").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ====================================================================
    // 集成测试：与 ApiResponse 协同
    // ====================================================================

    #[tokio::test]
    async fn test_api_response_vs_handle_error_consistency() {
        // ApiResponse 错误 → HTTP 200（业务错误）
        let api_resp = ApiResponse::error("业务错误");
        let api_response: Response = api_resp.into_response();
        let (api_status, api_json) = fetch_json(api_response).await;
        assert_eq!(api_status, StatusCode::OK);
        assert_eq!(api_json["code"], 0);

        // HandleError 错误 → HTTP 4xx/5xx
        let handle_err = HandleError::bad_request("参数错误");
        let err_response: Response = handle_err.into_response();
        let (err_status, err_json) = fetch_json(err_response).await;
        assert_eq!(err_status, StatusCode::BAD_REQUEST);
        assert_eq!(err_json["code"], 0);

        // 两者 code 都是 0（业务失败），但 HTTP 状态码不同
        // - ApiResponse::error：HTTP 200 + code=0（PHP 风格）
        // - HandleError::bad_request：HTTP 400 + code=0（REST 风格）
    }

    // ====================================================================
    // 各种 HTTP 状态码覆盖
    // ====================================================================

    #[tokio::test]
    async fn test_all_error_handlers_return_json() {
        let handlers: Vec<(Response, StatusCode, &str)> = vec![
            (
                not_found_handler().await,
                StatusCode::NOT_FOUND,
                "Not Found",
            ),
            (
                internal_error_handler().await,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Error",
            ),
            (
                method_not_allowed_handler().await,
                StatusCode::METHOD_NOT_ALLOWED,
                "Method Not Allowed",
            ),
            (
                bad_request_handler().await,
                StatusCode::BAD_REQUEST,
                "Bad Request",
            ),
            (
                forbidden_handler().await,
                StatusCode::FORBIDDEN,
                "Forbidden",
            ),
            (
                unprocessable_entity_handler().await,
                StatusCode::UNPROCESSABLE_ENTITY,
                "Unprocessable Entity",
            ),
        ];

        for (resp, expected_status, expected_msg) in handlers {
            let (status, json) = fetch_json(resp).await;
            assert_eq!(status, expected_status);
            assert_eq!(json["code"], 0);
            assert_eq!(json["msg"], expected_msg);
            assert!(json["data"].is_object());
        }
    }

    #[tokio::test]
    async fn test_unauthorized_returns_code_minus_one() {
        // 对齐 PHP BaseException
        let resp = unauthorized_handler().await;
        let (_, json) = fetch_json(resp).await;
        assert_eq!(json["code"], -1);
    }

    // 捕获 fallback_router -> Default::default() 变异体
    #[tokio::test]
    async fn test_fallback_router_returns_404_for_arbitrary_path() {
        let router = fallback_router();
        let resp = send_get(router, "/any/path").await;
        let (status, json) = fetch_json(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["msg"], "Not Found");
    }
}
