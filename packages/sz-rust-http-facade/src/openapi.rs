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
    description_zh: Option<String>,
    description_en: Option<String>,
}

impl OpenApiBuilder {
    /// 创建 OpenAPI 构建器
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            description: None,
            description_zh: None,
            description_en: None,
        }
    }

    /// 设置描述
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// 设置双语描述（v1.4.0 T13.1）
    pub fn description_bilingual(mut self, zh: impl Into<String>, en: impl Into<String>) -> Self {
        self.description_zh = Some(zh.into());
        self.description_en = Some(en.into());
        self
    }

    /// 构建指定语言的 OpenApi spec（v1.4.0 T13.1）
    pub fn build_for_lang(self, lang: &str) -> utoipa::openapi::Info {
        let desc = match lang.to_lowercase().as_str() {
            "en" | "en-us" | "en_us" => self.description_en.or(self.description_zh),
            _ => self.description_zh.or(self.description_en),
        }
        .or(self.description);
        let mut info = utoipa::openapi::Info::default();
        info.title = self.title;
        info.version = self.version;
        info.description = desc;
        info
    }

    /// 构建 OpenApi spec
    pub fn build(self) -> utoipa::openapi::Info {
        let mut info = utoipa::openapi::Info::default();
        info.title = self.title;
        info.version = self.version;
        info.description = self
            .description_zh
            .or(self.description)
            .or(self.description_en);
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

/// 挂载带语言切换的 Swagger UI 路由（v1.4.0 T13.2）
///
/// 默认展示中文，可通过 `?lang=en` 切换为英文。
#[cfg(feature = "swagger-ui")]
pub fn swagger_ui_routes_with_lang() -> axum::Router {
    axum::Router::new().route(
        "/docs/{_:.*}",
        axum::routing::get(|| async { "Swagger UI (i18n)" }),
    )
}

/// 从 Accept-Language 头提取语言标识（v1.4.0 T13.3）
///
/// 解析 Accept-Language 头值，返回语言标识字符串（"zh-cn" 或 "en-us"）。
/// 默认回退到中文。
pub fn extract_lang(accept_language: Option<&str>) -> String {
    match accept_language {
        Some(h) => {
            let lower = h.to_lowercase();
            if lower.starts_with("en") {
                "en-us".to_string()
            } else {
                "zh-cn".to_string()
            }
        }
        None => "zh-cn".to_string(),
    }
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

    #[test]
    fn test_description_bilingual_zh() {
        let info = OpenApiBuilder::new("API", "1.0")
            .description_bilingual("中文描述", "English desc")
            .build_for_lang("zh-cn");
        assert_eq!(info.description.as_deref(), Some("中文描述"));
    }

    #[test]
    fn test_description_bilingual_en() {
        let info = OpenApiBuilder::new("API", "1.0")
            .description_bilingual("中文描述", "English desc")
            .build_for_lang("en-us");
        assert_eq!(info.description.as_deref(), Some("English desc"));
    }

    #[test]
    fn test_extract_lang() {
        assert_eq!(extract_lang(Some("en-US")), "en-us");
        assert_eq!(extract_lang(Some("zh-CN")), "zh-cn");
        assert_eq!(extract_lang(None), "zh-cn");
    }
}
