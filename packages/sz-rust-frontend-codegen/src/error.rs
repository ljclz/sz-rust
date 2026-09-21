// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 错误类型定义

use thiserror::Error;

/// 前端代码生成错误
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FrontendCodegenError {
    /// 模型目录不存在
    #[error("模型目录不存在: {0}")]
    ModelDirNotFound(String),

    /// 模型解析错误
    #[error("模型解析错误: {0}")]
    ModelParseError(String),

    /// 缺少模型
    #[error("未指定任何模型，请通过 --model 参数或 .codegen.toml 配置指定")]
    MissingModel,

    /// 模板目录不存在
    #[error("模板目录不存在: {0}")]
    TemplateDirNotFound(String),

    /// 模板缺失
    #[error("模板缺失: {0}")]
    TemplateMissing(String),

    /// 模板语法错误
    #[error("模板语法错误: {0}")]
    TemplateSyntaxError(String),

    /// 模板渲染错误
    #[error("模板渲染错误: {0}")]
    TemplateRenderError(String),

    /// 模板路径穿越
    #[error("检测到路径穿越攻击: {0}")]
    TemplatePathTraversal(String),

    /// 模板继承循环
    #[error("模板继承循环: {0}")]
    TemplateInheritanceCycle(String),

    /// 未知过滤器
    #[error("未知过滤器: {0}")]
    UnknownFilter(String),

    /// 不支持的 UI 库
    #[error("不支持的 UI 库: {0}")]
    UnsupportedUiLibrary(String),

    /// 框架冲突
    #[error("框架冲突: {0}")]
    FrameworkConflict(String),

    /// 文件写入错误
    #[error("文件写入错误: {0}")]
    FileWriteError(String),

    /// 输出目录非空
    #[error("输出目录非空: {0}，使用 --force 强制覆盖")]
    OutputDirNotEmpty(String),

    /// 配置解析错误
    #[error("配置解析错误: {0}")]
    ConfigParseError(String),

    /// IO 错误
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    /// 通用错误
    #[error("{0}")]
    Generic(String),
}

impl FrontendCodegenError {
    /// 返回错误码（格式 `FE_CODEGEN_{CATEGORY}_{REASON}`）
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::ModelDirNotFound(_) => "FE_CODEGEN_MODEL_DIR_NOT_FOUND",
            Self::ModelParseError(_) => "FE_CODEGEN_MODEL_PARSE_ERROR",
            Self::MissingModel => "FE_CODEGEN_MODEL_MISSING",
            Self::TemplateDirNotFound(_) => "FE_CODEGEN_TEMPLATE_DIR_NOT_FOUND",
            Self::TemplateMissing(_) => "FE_CODEGEN_TEMPLATE_MISSING",
            Self::TemplateSyntaxError(_) => "FE_CODEGEN_TEMPLATE_SYNTAX_ERROR",
            Self::TemplateRenderError(_) => "FE_CODEGEN_TEMPLATE_RENDER_ERROR",
            Self::TemplatePathTraversal(_) => "FE_CODEGEN_TEMPLATE_PATH_TRAVERSAL",
            Self::TemplateInheritanceCycle(_) => "FE_CODEGEN_TEMPLATE_INHERITANCE_CYCLE",
            Self::UnknownFilter(_) => "FE_CODEGEN_FILTER_UNKNOWN",
            Self::UnsupportedUiLibrary(_) => "FE_CODEGEN_UI_LIBRARY_UNSUPPORTED",
            Self::FrameworkConflict(_) => "FE_CODEGEN_FRAMEWORK_CONFLICT",
            Self::FileWriteError(_) => "FE_CODEGEN_FILE_WRITE_ERROR",
            Self::OutputDirNotEmpty(_) => "FE_CODEGEN_OUTPUT_DIR_NOT_EMPTY",
            Self::ConfigParseError(_) => "FE_CODEGEN_CONFIG_PARSE_ERROR",
            Self::Io(_) => "FE_CODEGEN_IO_ERROR",
            Self::Generic(_) => "FE_CODEGEN_GENERIC",
        }
    }
}

impl From<tera::Error> for FrontendCodegenError {
    fn from(err: tera::Error) -> Self {
        Self::TemplateRenderError(err.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_model_dir_not_found() {
        let err = FrontendCodegenError::ModelDirNotFound("/path".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_DIR_NOT_FOUND");
    }

    #[test]
    fn test_error_code_model_parse_error() {
        let err = FrontendCodegenError::ModelParseError("syntax".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_PARSE_ERROR");
    }

    #[test]
    fn test_error_code_missing_model() {
        let err = FrontendCodegenError::MissingModel;
        assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_MISSING");
    }

    #[test]
    fn test_error_code_template_dir_not_found() {
        let err = FrontendCodegenError::TemplateDirNotFound("/path".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_DIR_NOT_FOUND");
    }

    #[test]
    fn test_error_code_template_missing() {
        let err = FrontendCodegenError::TemplateMissing("list.html".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_MISSING");
    }

    #[test]
    fn test_error_code_template_syntax_error() {
        let err = FrontendCodegenError::TemplateSyntaxError("{% bad".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_SYNTAX_ERROR");
    }

    #[test]
    fn test_error_code_template_render_error() {
        let err = FrontendCodegenError::TemplateRenderError("fail".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_RENDER_ERROR");
    }

    #[test]
    fn test_error_code_template_path_traversal() {
        let err = FrontendCodegenError::TemplatePathTraversal("../etc".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_PATH_TRAVERSAL");
    }

    #[test]
    fn test_error_code_template_inheritance_cycle() {
        let err = FrontendCodegenError::TemplateInheritanceCycle("a->b->a".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_INHERITANCE_CYCLE");
    }

    #[test]
    fn test_error_code_unknown_filter() {
        let err = FrontendCodegenError::UnknownFilter("badfilter".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_FILTER_UNKNOWN");
    }

    #[test]
    fn test_error_code_unsupported_ui_library() {
        let err = FrontendCodegenError::UnsupportedUiLibrary("Bootstrap".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_UI_LIBRARY_UNSUPPORTED");
    }

    #[test]
    fn test_error_code_framework_conflict() {
        let err = FrontendCodegenError::FrameworkConflict("vue+react".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_FRAMEWORK_CONFLICT");
    }

    #[test]
    fn test_error_code_file_write_error() {
        let err = FrontendCodegenError::FileWriteError("/out/file".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_FILE_WRITE_ERROR");
    }

    #[test]
    fn test_error_code_output_dir_not_empty() {
        let err = FrontendCodegenError::OutputDirNotEmpty("/out".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_OUTPUT_DIR_NOT_EMPTY");
    }

    #[test]
    fn test_error_code_config_parse_error() {
        let err = FrontendCodegenError::ConfigParseError("bad toml".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_CONFIG_PARSE_ERROR");
    }

    #[test]
    fn test_error_code_io_error() {
        let err = FrontendCodegenError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert_eq!(err.error_code(), "FE_CODEGEN_IO_ERROR");
    }

    #[test]
    fn test_error_code_generic() {
        let err = FrontendCodegenError::Generic("something".to_string());
        assert_eq!(err.error_code(), "FE_CODEGEN_GENERIC");
    }

    #[test]
    fn test_display_model_dir_not_found() {
        let err = FrontendCodegenError::ModelDirNotFound("/models".to_string());
        assert_eq!(err.to_string(), "模型目录不存在: /models");
    }

    #[test]
    fn test_display_missing_model() {
        let err = FrontendCodegenError::MissingModel;
        assert!(err.to_string().contains("未指定任何模型"));
    }

    #[test]
    fn test_display_template_path_traversal() {
        let err = FrontendCodegenError::TemplatePathTraversal("../etc/passwd".to_string());
        assert!(err.to_string().contains("路径穿越攻击"));
        assert!(err.to_string().contains("../etc/passwd"));
    }

    #[test]
    fn test_display_output_dir_not_empty() {
        let err = FrontendCodegenError::OutputDirNotEmpty("/out".to_string());
        assert!(err.to_string().contains("输出目录非空"));
        assert!(err.to_string().contains("--force"));
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err: FrontendCodegenError = io_err.into();
        assert!(matches!(err, FrontendCodegenError::Io(_)));
        assert_eq!(err.error_code(), "FE_CODEGEN_IO_ERROR");
    }

    #[test]
    fn test_from_tera_error() {
        let tera_err = tera::Error::msg("template broken");
        let err: FrontendCodegenError = tera_err.into();
        assert!(matches!(err, FrontendCodegenError::TemplateRenderError(_)));
        assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_RENDER_ERROR");
        assert!(err.to_string().contains("template broken"));
    }
}
