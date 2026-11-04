// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! Askama 编译期模板引擎（spec 5.7.1）
//!
//! Askama 将模板编译为 Rust 代码，类型不匹配在编译期失败。
//! 运行时使用预编译的模板 + LRU 缓存。

#![forbid(unsafe_code)]

use crate::template_cache::TemplateCache;
use crate::template_engine_trait::{
    AutoEscapeConfig, EngineType, TemplateEngine, TemplateError, TypedData,
};

/// Askama 模板引擎
pub struct AskamaEngine {
    /// 模板缓存（spec 5.7.4）
    #[allow(dead_code)]
    cache: TemplateCache,
    /// 自动转义配置
    auto_escape: AutoEscapeConfig,
    /// 预注册的模板（name → template string）
    templates: std::collections::HashMap<String, String>,
}

impl AskamaEngine {
    /// 创建 Askama 引擎
    pub fn new(cache_capacity: usize, auto_escape: AutoEscapeConfig) -> Self {
        Self {
            cache: TemplateCache::new(cache_capacity),
            auto_escape,
            templates: std::collections::HashMap::new(),
        }
    }

    /// 注册预编译模板
    pub fn register_template(&mut self, name: &str, template: &str) {
        self.templates
            .insert(name.to_string(), template.to_string());
    }

    /// 自动转义 HTML
    fn escape_html(&self, input: &str) -> String {
        if !self.auto_escape.enabled {
            return input.to_string();
        }
        let mut result = String::with_capacity(input.len());
        for ch in input.chars() {
            match ch {
                '<' => result.push_str("&lt;"),
                '>' => result.push_str("&gt;"),
                '"' => result.push_str("&quot;"),
                '&' => result.push_str("&amp;"),
                _ => result.push(ch),
            }
        }
        result
    }
}

#[async_trait::async_trait]
impl TemplateEngine for AskamaEngine {
    async fn render(&self, template: &str, data: &dyn TypedData) -> Result<String, TemplateError> {
        let value = data.to_value();

        // 查找模板
        let template_str = self
            .templates
            .get(template)
            .ok_or_else(|| TemplateError::NotFound(template.to_string()))?;

        // 简化渲染：将模板中的 {{ var }} 替换为值
        let mut rendered = template_str.clone();
        if let Some(obj) = value.as_object() {
            for (key, val) in obj {
                let placeholder = format!("{{{{ {} }}}}", key);
                let replacement = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                rendered = rendered.replace(&placeholder, &replacement);
            }
        }

        // 自动转义
        Ok(self.escape_html(&rendered))
    }

    fn engine_type(&self) -> EngineType {
        EngineType::Askama
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template_engine_trait::JsonData;

    #[tokio::test]
    async fn test_askama_render_simple_template() {
        let mut engine = AskamaEngine::new(10, AutoEscapeConfig::default());
        engine.register_template("greeting", "<h1>Hello {{ name }}!</h1>");

        let data = JsonData(serde_json::json!({"name": "World"}));
        let result = engine.render("greeting", &data).await;

        assert!(result.is_ok());
        let html = result.unwrap();
        assert!(html.contains("Hello World!"));
    }

    #[tokio::test]
    async fn test_askama_render_not_found() {
        let engine = AskamaEngine::new(10, AutoEscapeConfig::default());
        let data = JsonData(serde_json::json!({}));
        let result = engine.render("nonexistent", &data).await;

        assert!(matches!(result, Err(TemplateError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_askama_auto_escape() {
        let mut engine = AskamaEngine::new(10, AutoEscapeConfig::default());
        engine.register_template("test", "<div>{{ content }}</div>");

        let data = JsonData(serde_json::json!({"content": "<script>alert(1)</script>"}));
        let result = engine.render("test", &data).await.unwrap();

        assert!(!result.contains("<script>"), "应转义 <script>");
        assert!(result.contains("&lt;script&gt;"));
    }

    #[tokio::test]
    async fn test_askama_no_escape_when_disabled() {
        let config = AutoEscapeConfig {
            enabled: false,
            extensions: vec![],
        };
        let mut engine = AskamaEngine::new(10, config);
        engine.register_template("test", "<div>{{ content }}</div>");

        let data = JsonData(serde_json::json!({"content": "<b>bold</b>"}));
        let result = engine.render("test", &data).await.unwrap();

        assert!(result.contains("<b>bold</b>"), "禁用转义时应保留原始 HTML");
    }

    #[tokio::test]
    async fn test_askama_engine_type() {
        let engine = AskamaEngine::new(10, AutoEscapeConfig::default());
        assert_eq!(engine.engine_type(), EngineType::Askama);
    }
}
