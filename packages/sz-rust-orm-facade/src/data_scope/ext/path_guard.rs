// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 路径白名单校验 — PathGuard

use crate::data_scope::error::DataScopeError;
use std::path::{Path, PathBuf};

/// 路径白名单校验器
pub struct PathGuard {
    allowed_dirs: Vec<PathBuf>,
}

impl PathGuard {
    /// 创建校验器，对入参目录做 canonicalize，失败则跳过
    pub fn new(allowed_dirs: Vec<PathBuf>) -> Self {
        let allowed = allowed_dirs
            .into_iter()
            .filter_map(|d| d.canonicalize().ok())
            .collect();
        Self {
            allowed_dirs: allowed,
        }
    }

    /// 校验路径是否在白名单内，返回规范化后的路径
    ///
    /// 拒绝包含 `..` 的路径穿越尝试；规范化入参路径后检查是否在任一白名单目录内。
    pub fn validate(&self, path: &str) -> Result<PathBuf, DataScopeError> {
        // 拒绝路径穿越
        if path.contains("..") {
            return Err(DataScopeError::PathNotAllowed(path.to_string()));
        }
        let candidate = Path::new(path);
        let canonical = candidate
            .canonicalize()
            .map_err(|_| DataScopeError::PathNotAllowed(path.to_string()))?;
        for dir in &self.allowed_dirs {
            if canonical.starts_with(dir) {
                return Ok(canonical);
            }
        }
        Err(DataScopeError::PathNotAllowed(path.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_path_traversal_rejected() {
        let guard = PathGuard::new(vec![PathBuf::from(".")]);
        assert!(guard.validate("a/../../etc/passwd").is_err());
        assert!(guard.validate("../secret").is_err());
        assert!(guard.validate("foo/..").is_err());
    }

    #[test]
    fn test_valid_path_accepted() {
        let cwd = env::current_dir().unwrap();
        let guard = PathGuard::new(vec![cwd.clone()]);
        // Cargo.toml 在包根目录真实存在
        let target = cwd.join("Cargo.toml");
        let path_str = target.to_str().unwrap();
        let result = guard.validate(path_str);
        assert!(result.is_ok(), "valid path should be accepted");
    }

    #[test]
    fn test_absolute_path_outside_whitelist_rejected() {
        let cwd = env::current_dir().unwrap();
        // 父目录是绝对路径、不含 ".."，且不在 cwd 白名单内
        let parent = cwd.parent().unwrap().to_path_buf();
        let guard = PathGuard::new(vec![cwd]);
        let path_str = parent.to_str().unwrap();
        let result = guard.validate(path_str);
        assert!(result.is_err(), "path outside whitelist should be rejected");
    }
}
