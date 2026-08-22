//! CLI 错误类型
//!
//! 对齐 PHP `think\console\Command` 的错误处理模式：
//! - PHP 通过返回 `false` 或抛出异常
//! - Rust 使用 `Result<T, CliError>` 统一错误处理

use std::path::PathBuf;
use thiserror::Error;

/// CLI 错误
#[derive(Debug, Error)]
pub enum CliError {
    /// 文件 IO 错误（创建文件、读取模板等）
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// 文件已存在（对齐 PHP `Make::execute` 中 `already exists!` 提示）
    #[error("File already exists: {0}")]
    FileExists(String),

    /// clap 参数解析错误（对齐 PHP `console\Input` 验证失败）
    #[error("Clap error: {0}")]
    Clap(String),

    /// 代码生成错误（模板替换失败等）
    #[error("Generation error: {0}")]
    Generation(String),

    /// 数据库迁移错误
    #[error("Migration error: {0}")]
    Migration(String),

    /// 缓存清理错误
    #[error("Cache error: {0}")]
    Cache(String),

    /// 调度器错误
    #[error("Scheduler error: {0}")]
    Scheduler(String),

    /// 通用错误
    #[error("{0}")]
    Generic(String),

    /// P1-T3: 非法插件名称（不符合 Rust crate 命名规范）
    #[error("Invalid plugin name: {0} (must be lowercase letters, digits, underscores or hyphens, not starting with a digit)")]
    InvalidPluginName(String),

    /// P1-T3: 未知模板类型（附带用户请求的模板名与可用模板列表）
    #[error("Unknown template: '{requested}'. Available templates: {available:?}")]
    UnknownTemplate {
        /// 用户请求的模板类型名
        requested: String,
        /// 可用模板类型列表
        available: Vec<String>,
    },

    /// P1-T3: 字段定义解析错误
    #[error("Field parse error: {0}")]
    FieldParseError(String),

    /// P1-T3: 目标目录已存在（需 --force 覆盖）
    #[error("Directory already exists: {0} (use --force to overwrite)")]
    DirExists(PathBuf),

    /// P1-T3: 模板文件缺失（附带缺失文件列表）
    #[error("Template files missing: {0:?}")]
    TemplateMissing(Vec<String>),

    /// P1-T3: 模板语法错误（含文件名/行号/列号/错误消息）
    #[error("Template syntax error in {file}:{line}:{col}: {msg}")]
    TemplateSyntaxError {
        /// 模板文件名
        file: String,
        /// 行号（1-based）
        line: usize,
        /// 列号（1-based）
        col: usize,
        /// 错误消息
        msg: String,
    },

    /// P1-T3: 模板变量未找到（含变量名/引用文件/行号）
    #[error("Variable not found: '{var}' referenced in {file}:{line}")]
    VarNotFound {
        /// 缺失的变量名
        var: String,
        /// 引用该变量的模板文件名
        file: String,
        /// 行号（1-based）
        line: usize,
    },

    /// P1-T3: cargo check 编译失败（含编译错误列表）
    #[error("Compilation failed: {0:?}")]
    CompileFailed(Vec<String>),

    /// P1-T3: 外键字段不存在于从表字段定义中
    #[error("Foreign key not found: '{0}' is not a field in the slave table")]
    ForeignKeyNotFound(String),

    /// P1-T3: 主表与从表同名
    #[error("Master table and slave table must be different")]
    MasterSlaveSame,
}

impl From<clap::Error> for CliError {
    fn from(e: clap::Error) -> Self {
        CliError::Clap(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_error_display() {
        let err = CliError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert!(err.to_string().contains("IO error"));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn test_file_exists_error_display() {
        let err = CliError::FileExists("/path/to/file".to_string());
        assert!(err.to_string().contains("File already exists"));
        assert!(err.to_string().contains("/path/to/file"));
    }

    #[test]
    fn test_clap_error_conversion() {
        let clap_err = clap::Error::new(clap::error::ErrorKind::InvalidValue);
        let cli_err: CliError = clap_err.into();
        assert!(matches!(cli_err, CliError::Clap(_)));
    }

    #[test]
    fn test_generation_error_display() {
        let err = CliError::Generation("template substitution failed".to_string());
        assert!(err.to_string().contains("Generation error"));
    }

    #[test]
    fn test_migration_error_display() {
        let err = CliError::Migration("database connection failed".to_string());
        assert!(err.to_string().contains("Migration error"));
    }

    #[test]
    fn test_cache_error_display() {
        let err = CliError::Cache("redis connection failed".to_string());
        assert!(err.to_string().contains("Cache error"));
    }

    #[test]
    fn test_scheduler_error_display() {
        let err = CliError::Scheduler("cron parse failed".to_string());
        assert!(err.to_string().contains("Scheduler error"));
    }

    #[test]
    fn test_invalid_plugin_name_display() {
        let err = CliError::InvalidPluginName("my plugin".to_string());
        assert!(err.to_string().contains("Invalid plugin name"));
        assert!(err.to_string().contains("my plugin"));
    }

    #[test]
    fn test_unknown_template_display() {
        let err = CliError::UnknownTemplate {
            requested: "nonexistent".to_string(),
            available: vec!["crud".to_string(), "master-slave".to_string()],
        };
        assert!(err.to_string().contains("Unknown template"));
        assert!(err.to_string().contains("nonexistent"));
        assert!(err.to_string().contains("crud"));
    }

    #[test]
    fn test_field_parse_error_display() {
        let err = CliError::FieldParseError("unexpected ',' at position 5".to_string());
        assert!(err.to_string().contains("Field parse error"));
    }

    #[test]
    fn test_dir_exists_display() {
        let err = CliError::DirExists(PathBuf::from("/path/to/plugin"));
        assert!(err.to_string().contains("Directory already exists"));
        assert!(err.to_string().contains("--force"));
    }

    #[test]
    fn test_template_missing_display() {
        let err = CliError::TemplateMissing(vec!["model.rs.tera".to_string()]);
        assert!(err.to_string().contains("Template files missing"));
        assert!(err.to_string().contains("model.rs.tera"));
    }

    #[test]
    fn test_template_syntax_error_display() {
        let err = CliError::TemplateSyntaxError {
            file: "model.rs.tera".to_string(),
            line: 10,
            col: 5,
            msg: "unexpected token".to_string(),
        };
        let s = err.to_string();
        assert!(s.contains("model.rs.tera"));
        assert!(s.contains("10"));
        assert!(s.contains("5"));
        assert!(s.contains("unexpected token"));
    }

    #[test]
    fn test_var_not_found_display() {
        let err = CliError::VarNotFound {
            var: "plugin_name".to_string(),
            file: "model.rs.tera".to_string(),
            line: 3,
        };
        let s = err.to_string();
        assert!(s.contains("plugin_name"));
        assert!(s.contains("model.rs.tera"));
    }

    #[test]
    fn test_compile_failed_display() {
        let err =
            CliError::CompileFailed(vec!["error[E0277]: trait bound not satisfied".to_string()]);
        assert!(err.to_string().contains("Compilation failed"));
    }

    #[test]
    fn test_foreign_key_not_found_display() {
        let err = CliError::ForeignKeyNotFound("user_id".to_string());
        assert!(err.to_string().contains("Foreign key not found"));
        assert!(err.to_string().contains("user_id"));
    }

    #[test]
    fn test_master_slave_same_display() {
        let err = CliError::MasterSlaveSame;
        assert!(err
            .to_string()
            .contains("Master table and slave table must be different"));
    }
}
