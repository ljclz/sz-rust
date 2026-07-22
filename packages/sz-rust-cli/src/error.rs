//! CLI 错误类型
//!
//! 对齐 PHP `think\console\Command` 的错误处理模式：
//! - PHP 通过返回 `false` 或抛出异常
//! - Rust 使用 `Result<T, CliError>` 统一错误处理

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
}
