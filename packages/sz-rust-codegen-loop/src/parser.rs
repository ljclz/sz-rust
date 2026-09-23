// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 需求解析器：自然语言需求 → 结构化代码生成任务。

use crate::error::CodegenError;
use regex::Regex;

/// 目标编程语言。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    TypeScript,
    Python,
    Go,
    Java,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rust => write!(f, "rust"),
            Self::TypeScript => write!(f, "typescript"),
            Self::Python => write!(f, "python"),
            Self::Go => write!(f, "go"),
            Self::Java => write!(f, "java"),
        }
    }
}

/// 目标框架。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framework {
    Axum,
    Actix,
    Express,
    FastApi,
    Gin,
    Spring,
    None,
}

impl std::fmt::Display for Framework {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Axum => write!(f, "axum"),
            Self::Actix => write!(f, "actix"),
            Self::Express => write!(f, "express"),
            Self::FastApi => write!(f, "fastapi"),
            Self::Gin => write!(f, "gin"),
            Self::Spring => write!(f, "spring"),
            Self::None => write!(f, "none"),
        }
    }
}

/// 结构化代码生成任务。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodegenTask {
    pub language: Language,
    pub framework: Framework,
    pub feature: String,
    pub constraints: Vec<String>,
    pub raw_requirement: String,
}

/// 从自然语言需求解析出结构化代码生成任务。
///
/// # 示例
///
/// ```
/// use sz_rust_codegen_loop::parser::{parse_requirement, Language, Framework};
/// let task = parse_requirement("生成一个用户登录 API").unwrap();
/// assert_eq!(task.language, Language::Rust);
/// assert_eq!(task.framework, Framework::Axum);
/// assert!(task.feature.contains("login"));
/// ```
pub fn parse_requirement(input: &str) -> Result<CodegenTask, CodegenError> {
    if input.trim().is_empty() {
        return Err(CodegenError::Parse("empty requirement".into()));
    }

    let language = detect_language(input);
    let framework = detect_framework(input, &language);
    let feature = extract_feature(input);
    let constraints = extract_constraints(input);

    Ok(CodegenTask {
        language,
        framework,
        feature,
        constraints,
        raw_requirement: input.to_string(),
    })
}

fn detect_language(input: &str) -> Language {
    let lower = input.to_lowercase();
    if lower.contains("typescript") || lower.contains("ts") || lower.contains("前端") {
        Language::TypeScript
    } else if lower.contains("python") || lower.contains("django") || lower.contains("flask") {
        Language::Python
    } else if lower.contains("go ") || lower.contains("golang") || lower.contains("gin") {
        Language::Go
    } else if lower.contains("java") || lower.contains("spring") {
        Language::Java
    } else {
        Language::Rust
    }
}

fn detect_framework(input: &str, lang: &Language) -> Framework {
    let lower = input.to_lowercase();
    match lang {
        Language::Rust => {
            if lower.contains("actix") {
                Framework::Actix
            } else {
                Framework::Axum
            }
        }
        Language::TypeScript => Framework::Express,
        Language::Python => Framework::FastApi,
        Language::Go => Framework::Gin,
        Language::Java => Framework::Spring,
    }
}

fn extract_feature(input: &str) -> String {
    let lower = input.to_lowercase();

    let patterns: Vec<(&str, &str)> = vec![
        ("登录", "login"),
        ("注册", "register"),
        ("认证", "auth"),
        ("授权", "authorization"),
        ("用户", "user"),
        ("订单", "order"),
        ("支付", "payment"),
        ("商品", "product"),
        ("api", "api"),
        ("crud", "crud"),
        ("导出", "export"),
        ("导入", "import"),
        ("搜索", "search"),
        ("上传", "upload"),
        ("下载", "download"),
    ];

    let mut features: Vec<String> = Vec::new();
    for (cn, en) in &patterns {
        if lower.contains(cn) {
            features.push(en.to_string());
        }
    }

    let re = Regex::new(r"\b([a-z_]+)\s+api\b").unwrap();
    if let Some(caps) = re.captures(&lower) {
        features.push(caps[1].to_string());
    }

    if features.is_empty() {
        "generic".to_string()
    } else {
        features.join("_")
    }
}

fn extract_constraints(input: &str) -> Vec<String> {
    let mut constraints = Vec::new();
    let lower = input.to_lowercase();

    if lower.contains("异步") || lower.contains("async") {
        constraints.push("async".into());
    }
    if lower.contains("安全") || lower.contains("secure") {
        constraints.push("secure".into());
    }
    if lower.contains("高性能") || lower.contains("high performance") {
        constraints.push("high_performance".into());
    }
    if lower.contains("restful") || lower.contains("rest") {
        constraints.push("restful".into());
    }
    if lower.contains("分页") || lower.contains("pagination") {
        constraints.push("pagination".into());
    }

    constraints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_login_api() {
        let task = parse_requirement("生成一个用户登录 API").unwrap();
        assert_eq!(task.language, Language::Rust);
        assert_eq!(task.framework, Framework::Axum);
        assert!(task.feature.contains("login"));
    }

    #[test]
    fn parse_register_api() {
        let task = parse_requirement("生成一个用户注册 API").unwrap();
        assert!(task.feature.contains("register"));
    }

    #[test]
    fn parse_typescript() {
        let task = parse_requirement("用 TypeScript 生成一个用户 API").unwrap();
        assert_eq!(task.language, Language::TypeScript);
        assert_eq!(task.framework, Framework::Express);
    }

    #[test]
    fn parse_python() {
        let task = parse_requirement("用 Python FastAPI 生成支付 API").unwrap();
        assert_eq!(task.language, Language::Python);
        assert_eq!(task.framework, Framework::FastApi);
        assert!(task.feature.contains("payment"));
    }

    #[test]
    fn parse_go() {
        let task = parse_requirement("用 Go gin 生成订单 API").unwrap();
        assert_eq!(task.language, Language::Go);
        assert_eq!(task.framework, Framework::Gin);
        assert!(task.feature.contains("order"));
    }

    #[test]
    fn parse_with_constraints() {
        let task = parse_requirement("生成一个安全的异步用户登录 API").unwrap();
        assert!(task.constraints.contains(&"async".to_string()));
        assert!(task.constraints.contains(&"secure".to_string()));
    }

    #[test]
    fn parse_empty_errors() {
        assert!(parse_requirement("").is_err());
        assert!(parse_requirement("   ").is_err());
    }

    #[test]
    fn parse_restful_constraint() {
        let task = parse_requirement("生成一个 RESTful 用户 API").unwrap();
        assert!(task.constraints.contains(&"restful".to_string()));
    }

    #[test]
    fn parse_pagination_constraint() {
        let task = parse_requirement("生成一个分页商品列表 API").unwrap();
        assert!(task.constraints.contains(&"pagination".to_string()));
        assert!(task.feature.contains("product"));
    }

    #[test]
    fn parse_preserves_raw() {
        let input = "生成一个用户登录 API";
        let task = parse_requirement(input).unwrap();
        assert_eq!(task.raw_requirement, input);
    }

    #[test]
    fn parse_multiple_features() {
        let task = parse_requirement("生成用户登录注册 API").unwrap();
        assert!(task.feature.contains("login"));
        assert!(task.feature.contains("register"));
    }
}
