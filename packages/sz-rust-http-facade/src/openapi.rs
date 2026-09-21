// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! OpenAPI 3.0 文档自动生成
//!
//! 基于 utoipa 提供 OpenAPI spec 构建和 Swagger UI 集成。

use thiserror::Error;

/// OpenAPI 错误
#[derive(Debug, Error)]
pub enum OpenApiError {
    /// 构建错误
    #[error("OpenAPI build error: {0}")]
    Build(String),
    /// JSON 序列化错误
    #[error("JSON serialize error: {0}")]
    Serialize(String),
}

/// OpenAPI 构建器
///
/// 封装 utoipa::OpenApi 构建过程，提供链式 API。
pub struct OpenApiBuilder {
    title: String,
    version: String,
    description: Option<String>,
}

impl OpenApiBuilder {
    /// 创建 OpenAPI 构建器
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            description: None,
        }
    }

    /// 设置描述
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// 构建 OpenApi spec
    pub fn build(self) -> utoipa::openapi::Info {
        let mut info = utoipa::openapi::Info::default();
        info.title = self.title;
        info.version = self.version;
        info.description = self.description;
        info
    }

    /// 构建并序列化为 JSON
    pub fn to_json(self) -> Result<String, OpenApiError> {
        let info = self.build();
        serde_json::to_string_pretty(&info).map_err(|e| OpenApiError::Serialize(e.to_string()))
    }

    /// 验证 OpenAPI spec
    pub fn validate(self) -> Result<(), OpenApiError> {
        let info = self.build();
        if info.title.is_empty() {
            return Err(OpenApiError::Build("title is empty".into()));
        }
        if info.version.is_empty() {
            return Err(OpenApiError::Build("version is empty".into()));
        }
        Ok(())
    }
}

/// 挂载 Swagger UI 路由
///
/// 需启用 `swagger-ui` feature。
/// utoipa-swagger-ui 8.x 依赖 axum 0.7，与本 crate axum 0.8 不兼容，
/// 返回占位路由；实际 Swagger UI 由应用层直接挂载。
#[cfg(feature = "swagger-ui")]
pub fn swagger_ui_routes() -> axum::Router {
    axum::Router::new().route(
        "/docs/{_:.*}",
        axum::routing::get(|| async { "Swagger UI" }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openapi_build() {
        let info = OpenApiBuilder::new("Test API", "1.0.0").build();
        assert_eq!(info.title, "Test API");
        assert_eq!(info.version, "1.0.0");
    }

    #[test]
    fn test_openapi_to_json() {
        let json = OpenApiBuilder::new("Test API", "1.0.0").to_json().unwrap();
        assert!(json.contains("Test API"));
        assert!(json.contains("1.0.0"));
    }

    #[test]
    fn test_openapi_validate() {
        let result = OpenApiBuilder::new("Test", "1.0").validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_openapi_validate_empty_title() {
        let result = OpenApiBuilder::new("", "1.0").validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_openapi_error_display() {
        let err = OpenApiError::Build("test error".into());
        assert_eq!(err.to_string(), "OpenAPI build error: test error");
    }
}
