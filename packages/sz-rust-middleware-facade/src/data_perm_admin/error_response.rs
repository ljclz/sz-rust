// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 统一错误响应 — `ApiErrorResponse`
//!
//! 按 `DataScopeError::error_code()` 映射 HTTP 状态码，
//! 保证管理后台所有端点返回结构一致的 JSON 错误体。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sz_rust_orm_facade::data_scope::error::DataScopeError;

/// 统一 API 错误响应体
#[derive(Debug, Serialize)]
pub struct ApiErrorResponse {
    /// 错误码（对齐 `DataScopeError::error_code()`）
    pub code: String,
    /// 人类可读错误消息
    pub message: String,
    /// 当前世代号（用于客户端冲突重试）
    pub generation: u64,
    /// 额外细节（字段级错误、冲突信息等）
    pub details: serde_json::Value,
}

impl ApiErrorResponse {
    /// 从 `DataScopeError` 构造错误响应
    pub fn from_error(err: DataScopeError, generation: u64) -> Self {
        let code = err.error_code().to_string();
        let message = err.to_string();
        let details = match &err {
            DataScopeError::GenerationConflict { current } => serde_json::json!({
                "current_generation": current,
            }),
            DataScopeError::ConfigFileTooLarge { size, limit } => serde_json::json!({
                "size": size,
                "limit": limit,
            }),
            DataScopeError::ConfigParseError { line, msg } => serde_json::json!({
                "line": line,
                "msg": msg,
            }),
            DataScopeError::RateLimited { retry_after_secs } => serde_json::json!({
                "retry_after_secs": retry_after_secs,
            }),
            _ => serde_json::Value::Null,
        };
        Self {
            code,
            message,
            generation,
            details,
        }
    }

    /// 错误码 → HTTP 状态码映射
    pub fn status_for_code(code: &str) -> StatusCode {
        match code {
            "AUTH_REQUIRED" => StatusCode::UNAUTHORIZED,
            "ADMIN_REQUIRED" => StatusCode::FORBIDDEN,
            "RATE_LIMITED" => StatusCode::TOO_MANY_REQUESTS,
            "REQUEST_BODY_INVALID" | "CONFIG_PARSE_ERROR" => StatusCode::BAD_REQUEST,
            "RULE_NOT_FOUND" | "POLICY_NOT_FOUND" | "CONFIG_FILE_NOT_FOUND" => {
                StatusCode::NOT_FOUND
            }
            "GENERATION_CONFLICT" => StatusCode::CONFLICT,
            "CONFIG_FILE_TOO_LARGE" => StatusCode::PAYLOAD_TOO_LARGE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiErrorResponse {
    fn into_response(self) -> Response {
        let status = Self::status_for_code(&self.code);
        (status, axum::Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_mapping_all_codes() {
        // 401
        assert_eq!(
            ApiErrorResponse::status_for_code("AUTH_REQUIRED"),
            StatusCode::UNAUTHORIZED
        );
        // 403
        assert_eq!(
            ApiErrorResponse::status_for_code("ADMIN_REQUIRED"),
            StatusCode::FORBIDDEN
        );
        // 429
        assert_eq!(
            ApiErrorResponse::status_for_code("RATE_LIMITED"),
            StatusCode::TOO_MANY_REQUESTS
        );
        // 400
        assert_eq!(
            ApiErrorResponse::status_for_code("REQUEST_BODY_INVALID"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ApiErrorResponse::status_for_code("CONFIG_PARSE_ERROR"),
            StatusCode::BAD_REQUEST
        );
        // 404
        assert_eq!(
            ApiErrorResponse::status_for_code("RULE_NOT_FOUND"),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiErrorResponse::status_for_code("POLICY_NOT_FOUND"),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiErrorResponse::status_for_code("CONFIG_FILE_NOT_FOUND"),
            StatusCode::NOT_FOUND
        );
        // 409
        assert_eq!(
            ApiErrorResponse::status_for_code("GENERATION_CONFLICT"),
            StatusCode::CONFLICT
        );
        // 413
        assert_eq!(
            ApiErrorResponse::status_for_code("CONFIG_FILE_TOO_LARGE"),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        // 500（其他）
        assert_eq!(
            ApiErrorResponse::status_for_code("DATA_SCOPE_NO_USER_CONTEXT"),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            ApiErrorResponse::status_for_code("RULE_FIELD_MISSING"),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            ApiErrorResponse::status_for_code("PATH_NOT_ALLOWED"),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn test_from_error_generation_conflict_details() {
        let err = DataScopeError::GenerationConflict { current: 7 };
        let resp = ApiErrorResponse::from_error(err, 10);
        assert_eq!(resp.code, "GENERATION_CONFLICT");
        assert_eq!(resp.generation, 10);
        assert_eq!(resp.details["current_generation"], 7);
    }

    #[test]
    fn test_from_error_config_file_too_large_details() {
        let err = DataScopeError::ConfigFileTooLarge {
            size: 100,
            limit: 50,
        };
        let resp = ApiErrorResponse::from_error(err, 0);
        assert_eq!(resp.code, "CONFIG_FILE_TOO_LARGE");
        assert_eq!(resp.details["size"], 100);
        assert_eq!(resp.details["limit"], 50);
    }

    #[test]
    fn test_from_error_config_parse_error_details() {
        let err = DataScopeError::ConfigParseError {
            line: 3,
            msg: "bad token".into(),
        };
        let resp = ApiErrorResponse::from_error(err, 0);
        assert_eq!(resp.code, "CONFIG_PARSE_ERROR");
        assert_eq!(resp.details["line"], 3);
        assert_eq!(resp.details["msg"], "bad token");
    }

    #[test]
    fn test_from_error_rate_limited_details() {
        let err = DataScopeError::RateLimited {
            retry_after_secs: 30,
        };
        let resp = ApiErrorResponse::from_error(err, 0);
        assert_eq!(resp.code, "RATE_LIMITED");
        assert_eq!(resp.details["retry_after_secs"], 30);
    }

    #[test]
    fn test_from_error_null_details_for_plain_variants() {
        let err = DataScopeError::RuleNotFound("r1".into());
        let resp = ApiErrorResponse::from_error(err, 5);
        assert_eq!(resp.code, "RULE_NOT_FOUND");
        assert!(resp.details.is_null());
        assert_eq!(resp.generation, 5);
    }

    #[tokio::test]
    async fn test_into_response_status_and_body() {
        let err = DataScopeError::RuleNotFound("r1".into());
        let resp = ApiErrorResponse::from_error(err, 5);
        let response = resp.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "RULE_NOT_FOUND");
        assert_eq!(json["generation"], 5);
    }
}
