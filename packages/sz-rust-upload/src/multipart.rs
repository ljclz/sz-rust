// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! Multipart 表单文件上传（spec §5.23 规则 1）
//!
//! 基于 axum multipart feature 提供表单文件上传解析。

use std::path::PathBuf;

use crate::error::UploadError;

/// Multipart 上传配置（spec §6.23）
#[derive(Debug, Clone)]
pub struct MultipartConfig {
    /// 单文件大小上限（spec §6.23 规则 1）
    pub max_file_size: u64,
    /// 允许的文件扩展名（空表示不限制）
    pub allowed_extensions: Vec<String>,
    /// 允许的 MIME 类型（空表示不限制）
    pub allowed_mime_types: Vec<String>,
}

impl Default for MultipartConfig {
    fn default() -> Self {
        Self {
            max_file_size: 100 * 1024 * 1024,
            allowed_extensions: Vec::new(),
            allowed_mime_types: Vec::new(),
        }
    }
}

impl MultipartConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置最大文件大小
    pub fn with_max_file_size(mut self, max: u64) -> Self {
        self.max_file_size = max;
        self
    }

    /// 添加允许的扩展名
    pub fn with_extension(mut self, ext: impl Into<String>) -> Self {
        self.allowed_extensions.push(ext.into());
        self
    }

    /// 添加允许的 MIME 类型
    pub fn with_mime_type(mut self, mime: impl Into<String>) -> Self {
        self.allowed_mime_types.push(mime.into());
        self
    }

    /// 校验文件大小（spec §5.23 规则 6）
    pub fn validate_size(&self, size: u64) -> Result<(), UploadError> {
        if size > self.max_file_size {
            return Err(UploadError::SizeExceeded(size, self.max_file_size));
        }
        Ok(())
    }

    /// 校验文件扩展名
    pub fn validate_extension(&self, file_name: &str) -> Result<(), UploadError> {
        if self.allowed_extensions.is_empty() {
            return Ok(());
        }
        let path = PathBuf::from(file_name);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if self.allowed_extensions.iter().any(|allowed| allowed == ext) {
            Ok(())
        } else {
            Err(UploadError::InvalidParam(format!(
                "不支持的文件扩展名: {ext}"
            )))
        }
    }

    /// 校验 MIME 类型
    pub fn validate_mime_type(&self, mime: &str) -> Result<(), UploadError> {
        if self.allowed_mime_types.is_empty() {
            return Ok(());
        }
        if self
            .allowed_mime_types
            .iter()
            .any(|allowed| allowed == mime)
        {
            Ok(())
        } else {
            Err(UploadError::InvalidParam(format!(
                "不支持的 MIME 类型: {mime}"
            )))
        }
    }

    /// 综合校验
    pub fn validate(&self, file_name: &str, size: u64, mime: &str) -> Result<(), UploadError> {
        self.validate_size(size)?;
        self.validate_extension(file_name)?;
        self.validate_mime_type(mime)?;
        Ok(())
    }
}

/// Multipart 上传文件元数据
#[derive(Debug, Clone)]
pub struct MultipartFile {
    /// 文件名
    pub file_name: String,
    /// MIME 类型
    pub mime_type: String,
    /// 文件大小
    pub size: u64,
    /// 文件内容
    pub data: Vec<u8>,
}

impl MultipartFile {
    /// 创建文件元数据
    pub fn new(file_name: impl Into<String>, mime_type: impl Into<String>, data: Vec<u8>) -> Self {
        let size = data.len() as u64;
        Self {
            file_name: file_name.into(),
            mime_type: mime_type.into(),
            size,
            data,
        }
    }

    /// 校验文件名安全性（防目录遍历，铁律 8）
    pub fn validate_file_name(&self) -> Result<(), UploadError> {
        let name = &self.file_name;
        if name.is_empty() {
            return Err(UploadError::InvalidParam("文件名不能为空".into()));
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(UploadError::InvalidParam(format!("非法文件名: {name}")));
        }
        if name.chars().any(|c| c.is_control()) {
            return Err(UploadError::InvalidParam("文件名包含控制字符".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validate_size_ok() {
        let config = MultipartConfig::new().with_max_file_size(1024);
        assert!(config.validate_size(512).is_ok());
    }

    #[test]
    fn test_config_validate_size_exceeded() {
        let config = MultipartConfig::new().with_max_file_size(100);
        let err = config.validate_size(200).unwrap_err();
        assert!(matches!(err, UploadError::SizeExceeded(200, 100)));
    }

    #[test]
    fn test_config_validate_extension() {
        let config = MultipartConfig::new()
            .with_extension("png")
            .with_extension("jpg");
        assert!(config.validate_extension("photo.png").is_ok());
        assert!(config.validate_extension("photo.jpg").is_ok());
        assert!(config.validate_extension("photo.gif").is_err());
    }

    #[test]
    fn test_config_validate_extension_unrestricted() {
        let config = MultipartConfig::new();
        assert!(config.validate_extension("anything.xyz").is_ok());
    }

    #[test]
    fn test_config_validate_mime_type() {
        let config = MultipartConfig::new().with_mime_type("image/png");
        assert!(config.validate_mime_type("image/png").is_ok());
        assert!(config.validate_mime_type("text/html").is_err());
    }

    #[test]
    fn test_config_validate_comprehensive() {
        let config = MultipartConfig::new()
            .with_max_file_size(1024)
            .with_extension("txt")
            .with_mime_type("text/plain");
        assert!(config.validate("file.txt", 100, "text/plain").is_ok());
        assert!(config.validate("file.txt", 2048, "text/plain").is_err());
        assert!(config.validate("file.png", 100, "text/plain").is_err());
    }

    #[test]
    fn test_multipart_file_validate_name() {
        let file = MultipartFile::new("test.txt", "text/plain", vec![1, 2, 3]);
        assert!(file.validate_file_name().is_ok());
    }

    #[test]
    fn test_multipart_file_reject_path_traversal() {
        let file = MultipartFile::new("../etc/passwd", "text/plain", vec![]);
        assert!(file.validate_file_name().is_err());
    }

    #[test]
    fn test_multipart_file_reject_empty_name() {
        let file = MultipartFile::new("", "text/plain", vec![]);
        assert!(file.validate_file_name().is_err());
    }

    #[test]
    fn test_multipart_file_reject_control_chars() {
        let file = MultipartFile::new("test\n.txt", "text/plain", vec![]);
        assert!(file.validate_file_name().is_err());
    }
}
