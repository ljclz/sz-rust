// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 统一模板引擎 trait（spec 5.7.3）
//!
//! 提供 Askama（编译期）与 Tera（运行时）的统一接口。

#![forbid(unsafe_code)]

use std::collections::HashMap;

use serde_json::Value;

/// 模板引擎类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineType {
    /// Askama 编译期模板
    Askama,
    /// Tera 运行时模板
    Tera,
}

/// 模板错误
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    /// 模板未找到
    #[error("模板未找到: {0}")]
    NotFound(String),
    /// 模板渲染失败
    #[error("模板渲染失败: {0}")]
    Render(String),
    /// 模板编译失败
    #[error("模板编译失败: {0}")]
    Compile(String),
    /// 类型不匹配
    #[error("类型不匹配: {0}")]
    TypeMismatch(String),
    /// 缓存已满
    #[error("缓存已满")]
    CacheFull,
}

/// 类型安全数据接口
pub trait TypedData: Send + Sync {
    /// 转换为 serde_json::Value
    fn to_value(&self) -> Value;
}

/// 自动转义配置
#[derive(Debug, Clone)]
pub struct AutoEscapeConfig {
    /// 是否启用自动转义（默认 true，spec 5.7.6）
    pub enabled: bool,
    /// 需要转义的文件后缀
    pub extensions: Vec<String>,
}

impl Default for AutoEscapeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extensions: vec!["html".to_string(), "htm".to_string()],
        }
    }
}

/// 统一模板引擎 trait（spec 5.7.3）
#[async_trait::async_trait]
pub trait TemplateEngine: Send + Sync {
    /// 渲染模板
    async fn render(&self, template: &str, data: &dyn TypedData) -> Result<String, TemplateError>;

    /// 引擎类型
    fn engine_type(&self) -> EngineType;
}

/// 简单 JSON 数据包装（实现 TypedData）
pub struct JsonData(pub Value);

impl TypedData for JsonData {
    fn to_value(&self) -> Value {
        self.0.clone()
    }
}

/// HashMap 数据包装
pub struct MapData(pub HashMap<String, Value>);

impl TypedData for MapData {
    fn to_value(&self) -> Value {
        Value::Object(self.0.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }
}
