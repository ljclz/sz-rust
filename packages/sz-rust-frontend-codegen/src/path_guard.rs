// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 路径穿越防护

use std::path::Path;

use crate::error::FrontendCodegenError;

/// 路径守卫
pub struct PathGuard;

impl PathGuard {
    /// 校验路径在 base_dir 内，拒绝路径穿越
    pub fn validate(path: &Path, base_dir: &Path) -> Result<(), FrontendCodegenError> {
        let path_str = path.to_string_lossy();
        if path_str.contains('\0') {
            return Err(FrontendCodegenError::TemplatePathTraversal(format!(
                "路径含控制字符: {path_str}"
            )));
        }
        if path.is_absolute() {
            return Err(FrontendCodegenError::TemplatePathTraversal(format!(
                "拒绝绝对路径: {path_str}"
            )));
        }
        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(FrontendCodegenError::TemplatePathTraversal(format!(
                    "拒绝路径穿越（含 ..）: {path_str}"
                )));
            }
        }
        let canonical_base = base_dir
            .canonicalize()
            .unwrap_or_else(|_| base_dir.to_path_buf());
        let full = canonical_base.join(path);
        if !full.starts_with(&canonical_base) {
            return Err(FrontendCodegenError::TemplatePathTraversal(format!(
                "路径超出 base_dir: {path_str}"
            )));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_valid_relative_path() {
        let base = PathBuf::from(".");
        let path = Path::new("templates/index.html");
        assert!(PathGuard::validate(path, &base).is_ok());
    }

    #[test]
    fn test_valid_nested_relative_path() {
        let base = PathBuf::from(".");
        let path = Path::new("templates/sub/dir/file.html");
        assert!(PathGuard::validate(path, &base).is_ok());
    }

    #[test]
    fn test_null_byte_rejected() {
        let base = PathBuf::from(".");
        let path = Path::new("templates\0evil.html");
        let result = PathGuard::validate(path, &base);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            FrontendCodegenError::TemplatePathTraversal(_)
        ));
        assert!(err.to_string().contains("控制字符"));
    }

    #[test]
    fn test_absolute_path_rejected() {
        let base = PathBuf::from(".");
        let path = if cfg!(windows) {
            Path::new("C:\\Windows\\system32")
        } else {
            Path::new("/etc/passwd")
        };
        let result = PathGuard::validate(path, &base);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            FrontendCodegenError::TemplatePathTraversal(_)
        ));
        assert!(err.to_string().contains("绝对路径"));
    }

    #[test]
    fn test_parent_dir_rejected() {
        let base = PathBuf::from(".");
        let path = Path::new("../secret.txt");
        let result = PathGuard::validate(path, &base);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            FrontendCodegenError::TemplatePathTraversal(_)
        ));
        assert!(err.to_string().contains(".."));
    }

    #[test]
    fn test_parent_dir_nested_rejected() {
        let base = PathBuf::from(".");
        let path = Path::new("templates/../../secret.txt");
        let result = PathGuard::validate(path, &base);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FrontendCodegenError::TemplatePathTraversal(_)
        ));
    }

    #[test]
    fn test_current_dir_allowed() {
        let base = PathBuf::from(".");
        let path = Path::new("./templates/index.html");
        assert!(PathGuard::validate(path, &base).is_ok());
    }

    #[test]
    fn test_empty_path_allowed() {
        let base = PathBuf::from(".");
        let path = Path::new("");
        assert!(PathGuard::validate(path, &base).is_ok());
    }
}
