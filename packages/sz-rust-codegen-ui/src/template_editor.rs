// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 代码模板编辑（spec §5.27 规则 2，§6.27 规则 2）
//!
//! 模板可自定义 + 语法校验。

use crate::error::CodegenUiError;

/// 代码生成模板（spec §5.27 规则 2）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeTemplate {
    /// 模板 ID
    pub id: String,
    /// 模板名称
    pub name: String,
    /// 模板内容
    pub content: String,
    /// 输出路径
    pub output_path: String,
}

impl CodeTemplate {
    /// 创建模板
    pub fn new(id: impl Into<String>, name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            content: content.into(),
            output_path: String::new(),
        }
    }

    /// 设置输出路径
    pub fn with_output_path(mut self, path: impl Into<String>) -> Self {
        self.output_path = path.into();
        self
    }

    /// 校验模板（spec §6.27 规则 2）
    pub fn validate(&self) -> Result<(), CodegenUiError> {
        if self.id.is_empty() {
            return Err(CodegenUiError::InvalidParam("模板 ID 不能为空".into()));
        }
        if self.name.is_empty() {
            return Err(CodegenUiError::InvalidParam("模板名称不能为空".into()));
        }
        if self.content.is_empty() {
            return Err(CodegenUiError::InvalidParam("模板内容不能为空".into()));
        }
        Self::validate_syntax(&self.content)?;
        Ok(())
    }

    /// 简单语法校验：检查模板变量 `{{ }}` 是否闭合
    pub fn validate_syntax(content: &str) -> Result<(), CodegenUiError> {
        let open_count = content.matches("{{").count();
        let close_count = content.matches("}}").count();
        if open_count != close_count {
            return Err(CodegenUiError::RenderFailed(format!(
                "模板语法错误: {{ }} 不匹配 ({{ = {open_count}, }} = {close_count})"
            )));
        }
        Ok(())
    }
}

/// 模板变量
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemplateVariable {
    /// 变量名
    pub name: String,
    /// 描述
    pub description: String,
    /// 默认值
    pub default_value: Option<String>,
}

impl TemplateVariable {
    /// 创建变量
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            default_value: None,
        }
    }

    /// 设置默认值
    pub fn with_default(mut self, value: impl Into<String>) -> Self {
        self.default_value = Some(value.into());
        self
    }
}

/// 简单模板渲染（替换 `{{ var }}` 占位符）
pub fn render_template(
    template: &str,
    variables: &std::collections::HashMap<String, String>,
) -> Result<String, CodegenUiError> {
    CodeTemplate::validate_syntax(template)?;
    let mut result = template.to_string();
    for (key, value) in variables {
        let placeholder = format!("{{{{ {key} }}}}");
        result = result.replace(&placeholder, value);
        let placeholder_no_space = format!("{{{{{key}}}}}");
        result = result.replace(&placeholder_no_space, value);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_template_validate() {
        let template = CodeTemplate::new("t1", "用户模型", "struct {{ name }} {}");
        assert!(template.validate().is_ok());
    }

    #[test]
    fn test_template_validate_empty_id() {
        let template = CodeTemplate::new("", "name", "content");
        assert!(template.validate().is_err());
    }

    #[test]
    fn test_template_validate_syntax_error() {
        let template = CodeTemplate::new("t1", "name", "{{ unclosed");
        assert!(template.validate().is_err());
    }

    #[test]
    fn test_render_template() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "User".to_string());
        let result = render_template("struct {{ name }} {}", &vars).unwrap();
        assert_eq!(result, "struct User {}");
    }

    #[test]
    fn test_render_template_no_space() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "User".to_string());
        let result = render_template("struct {{name}} {}", &vars).unwrap();
        assert_eq!(result, "struct User {}");
    }

    #[test]
    fn test_render_template_multiple_vars() {
        let mut vars = HashMap::new();
        vars.insert("table".to_string(), "users".to_string());
        vars.insert("field".to_string(), "id".to_string());
        let result = render_template("SELECT {{ field }} FROM {{ table }}", &vars).unwrap();
        assert_eq!(result, "SELECT id FROM users");
    }
}
