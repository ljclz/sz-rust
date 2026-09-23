// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Response 静态门面
//!
//! 对齐 PHP `think\Response`，委托 `sz_rust_http_facade::response` 模块。

use serde_json::Value;

use sz_rust_http_facade::response::ApiResponse;

/// Response 静态门面（对齐 PHP `think\Response`）
///
/// 提供 `ApiResponse` 构造和 HTTP Response 渲染的静态方法。
pub struct Response;

impl Response {
    /// 创建成功响应（对齐 PHP `renderSuccess`）
    pub fn success(data: Value, msg: impl Into<String>) -> ApiResponse {
        ApiResponse::success(data, msg)
    }

    /// 创建空成功响应
    pub fn success_empty() -> ApiResponse {
        ApiResponse::success_empty()
    }

    /// 创建错误响应（对齐 PHP `renderError`）
    pub fn error(msg: impl Into<String>) -> ApiResponse {
        ApiResponse::error(msg)
    }

    /// 创建带数据的错误响应
    pub fn error_with_data(msg: impl Into<String>, data: Value) -> ApiResponse {
        ApiResponse::error_with_data(msg, data)
    }

    /// 创建带自定义错误码的错误响应
    pub fn error_with_code(code: i32, msg: impl Into<String>, data: Value) -> ApiResponse {
        ApiResponse::error_with_code(code, msg, data)
    }

    /// 创建新响应
    pub fn create(code: i32, msg: impl Into<String>, data: Value) -> ApiResponse {
        ApiResponse::new(code, msg, data)
    }

    /// 渲染 JSON HTTP Response
    pub fn render_json(code: i32, msg: impl Into<String>, data: Value) -> axum::response::Response {
        sz_rust_http_facade::response::render_json(code, msg, data)
    }

    /// 渲染成功 JSON HTTP Response
    pub fn render_success(data: Value, msg: impl Into<String>) -> axum::response::Response {
        sz_rust_http_facade::response::render_success(data, msg)
    }

    /// 渲染错误 JSON HTTP Response
    pub fn render_error(msg: impl Into<String>) -> axum::response::Response {
        sz_rust_http_facade::response::render_error(msg)
    }
}

// 重导出 ApiResponse
pub use sz_rust_http_facade::response::ApiResponse as ApiResponseDto;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_response_success() {
        let resp = Response::success(json!({"id": 1}), "ok");
        assert_eq!(resp.code, 1);
        assert_eq!(resp.msg, "ok");
    }

    #[test]
    fn test_response_error() {
        let resp = Response::error("failed");
        assert_eq!(resp.code, 0);
        assert_eq!(resp.msg, "failed");
    }

    #[test]
    fn test_response_error_with_code() {
        let resp = Response::error_with_code(-1, "未登录", json!({}));
        assert_eq!(resp.code, -1);
    }

    #[test]
    fn test_response_to_json_string() {
        let resp = Response::success(json!("data"), "");
        let json_str = resp.to_json_string();
        assert!(json_str.contains("\"code\":1"));
    }
}
