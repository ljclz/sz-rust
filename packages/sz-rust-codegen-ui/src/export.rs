// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 代码导出（spec §5.27 规则 4，§6.27 规则 3）
//!
//! 文件/压缩包导出，导出代码可编译。

use std::collections::HashMap;

use crate::error::CodegenUiError;

/// 导出格式（spec §6.27 规则 3）
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExportFormat {
    /// 多文件
    Files,
    /// ZIP 压缩包
    Zip,
}

impl ExportFormat {
    /// 从字符串解析
    pub fn parse(s: &str) -> Result<Self, CodegenUiError> {
        match s.to_lowercase().as_str() {
            "files" => Ok(Self::Files),
            "zip" => Ok(Self::Zip),
            _ => Err(CodegenUiError::InvalidParam(format!("未知导出格式: {s}"))),
        }
    }

    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Zip => "zip",
        }
    }
}

/// 导出文件
#[derive(Debug, Clone)]
pub struct ExportFile {
    /// 文件路径（相对）
    pub path: String,
    /// 文件内容
    pub content: String,
}

impl ExportFile {
    /// 创建导出文件
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

/// 导出服务（spec §5.27 规则 4）
pub struct ExportService;

impl ExportService {
    /// 导出代码（spec §5.27 规则 4，§6.27 规则 3）
    ///
    /// # 后置条件
    /// - Files 格式 → 返回文件列表
    /// - Zip 格式 → 返回 ZIP 二进制（简单实现，实际 ZIP 由外部库承担）
    pub fn export(
        files: &HashMap<String, String>,
        format: ExportFormat,
    ) -> Result<Vec<u8>, CodegenUiError> {
        if files.is_empty() {
            return Err(CodegenUiError::InvalidParam("文件列表不能为空".into()));
        }
        match format {
            ExportFormat::Files => {
                let mut result = Vec::new();
                for (path, content) in files {
                    result.extend_from_slice(path.as_bytes());
                    result.extend_from_slice(b"\n");
                    result.extend_from_slice(content.as_bytes());
                    result.extend_from_slice(b"\n---\n");
                }
                Ok(result)
            }
            ExportFormat::Zip => {
                let mut result = Vec::new();
                for (path, content) in files {
                    result.extend_from_slice(b"FILE: ");
                    result.extend_from_slice(path.as_bytes());
                    result.extend_from_slice(b"\n");
                    result.extend_from_slice(content.as_bytes());
                    result.extend_from_slice(b"\n");
                }
                Ok(result)
            }
        }
    }

    /// 检查生成代码是否包含硬编码密钥（spec §5.27 禁止项）
    pub fn check_hardcoded_secrets(code: &str) -> Result<(), CodegenUiError> {
        let suspicious_patterns = [
            "password = \"",
            "secret = \"",
            "api_key = \"",
            "token = \"",
            "private_key = \"",
        ];
        for pattern in &suspicious_patterns {
            if code.to_lowercase().contains(pattern) {
                return Err(CodegenUiError::HardcodedSecret);
            }
        }
        Ok(())
    }

    /// 导出前校验所有文件
    pub fn validate_files(files: &HashMap<String, String>) -> Result<(), CodegenUiError> {
        for (path, content) in files {
            if path.is_empty() {
                return Err(CodegenUiError::InvalidParam("文件路径不能为空".into()));
            }
            Self::check_hardcoded_secrets(content)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_format_from_str() {
        assert_eq!(ExportFormat::parse("files").unwrap(), ExportFormat::Files);
        assert_eq!(ExportFormat::parse("zip").unwrap(), ExportFormat::Zip);
        assert!(ExportFormat::parse("invalid").is_err());
    }

    #[test]
    fn test_export_files() {
        let mut files = HashMap::new();
        files.insert("main.rs".to_string(), "fn main() {}".to_string());
        let result = ExportService::export(&files, ExportFormat::Files).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_export_zip() {
        let mut files = HashMap::new();
        files.insert("main.rs".to_string(), "fn main() {}".to_string());
        let result = ExportService::export(&files, ExportFormat::Zip).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_export_empty() {
        let files = HashMap::new();
        let result = ExportService::export(&files, ExportFormat::Files);
        assert!(result.is_err());
    }

    #[test]
    fn test_check_hardcoded_secrets_ok() {
        let code = "let password = config.get(\"password\");";
        assert!(ExportService::check_hardcoded_secrets(code).is_ok());
    }

    #[test]
    fn test_check_hardcoded_secrets_detected() {
        let code = "let password = \"secret123\";";
        assert!(matches!(
            ExportService::check_hardcoded_secrets(code),
            Err(CodegenUiError::HardcodedSecret)
        ));
    }

    #[test]
    fn test_validate_files() {
        let mut files = HashMap::new();
        files.insert("main.rs".to_string(), "fn main() {}".to_string());
        assert!(ExportService::validate_files(&files).is_ok());
    }
}
