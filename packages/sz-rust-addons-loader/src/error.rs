// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 插件加载器错误类型
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think\addons` 抛出的 `HttpException`：
//!
//! | PHP 异常 | Rust 错误 | 说明 |
//! |----------|----------|------|
//! | `HttpException(500, 'addon can not be empty')` | `AddonNotFound` | 插件名缺失 |
//! | `HttpException(404, 'addon %s not found')` | `AddonNotFound` | 插件不存在 |
//! | `HttpException(500, 'addon %s is disabled')` | `AddonDisabled` | 插件已禁用 |
//! | `HttpException(404, 'addon controller %s not found')` | `ControllerNotFound` | 控制器不存在 |
//! | `HttpException(404, 'addon action %s not found')` | `ActionNotFound` | 操作不存在 |

use thiserror::Error;

/// 插件加载器错误
#[derive(Debug, Error)]
pub enum AddonLoaderError {
    /// 插件不存在（对齐 PHP `HttpException(404, 'addon %s not found')`）
    #[error("addon '{0}' not found")]
    AddonNotFound(String),

    /// 插件已禁用（对齐 PHP `HttpException(500, 'addon %s is disabled')`）
    #[error("addon '{0}' is disabled")]
    AddonDisabled(String),

    /// 控制器不存在（对齐 PHP `HttpException(404, 'addon controller %s not found')`）
    #[error("addon controller '{0}' not found")]
    ControllerNotFound(String),

    /// 操作不存在（对齐 PHP `HttpException(404, 'addon action %s not found')`）
    #[error("addon action '{0}' not found")]
    ActionNotFound(String),

    /// 插件清单解析失败（Plugin.php 中 `$info` 数组格式错误）
    #[error("failed to parse manifest for addon '{addon}': {reason}")]
    ManifestParse {
        /// 插件名
        addon: String,
        /// 失败原因
        reason: String,
    },

    /// 插件目录扫描失败（IO 错误）
    #[error("failed to scan addons directory '{path}': {source}")]
    ScanDir {
        /// 目录路径
        path: String,
        /// 底层 IO 错误
        source: std::io::Error,
    },

    /// 文件读取失败
    #[error("failed to read file '{path}': {source}")]
    ReadFile {
        /// 文件路径
        path: String,
        /// 底层 IO 错误
        source: std::io::Error,
    },

    /// 自动加载类映射失败（对齐 PHP `spl_autoload_register` 找不到文件时返回 false）
    #[error("autoload failed: class '{class}' not mapped to any file")]
    AutoloadMiss {
        /// 类名（如 `addons\operate\Plugin`）
        class: String,
    },

    /// 钩子注册失败
    #[error("failed to register hook '{hook}' for addon '{addon}'")]
    HookRegister {
        /// 钩子名
        hook: String,
        /// 插件名
        addon: String,
    },

    /// 路由解析失败
    #[error("failed to parse route '{url}': {reason}")]
    RouteParse {
        /// URL
        url: String,
        /// 失败原因
        reason: String,
    },
}

impl From<std::io::Error> for AddonLoaderError {
    fn from(err: std::io::Error) -> Self {
        AddonLoaderError::ReadFile {
            path: "<unknown>".to_string(),
            source: err,
        }
    }
}

/// 插件加载器 Result 别名
pub type AddonLoaderResult<T> = Result<T, AddonLoaderError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addon_not_found_display() {
        let err = AddonLoaderError::AddonNotFound("operate".to_string());
        assert_eq!(err.to_string(), "addon 'operate' not found");
    }

    #[test]
    fn test_addon_disabled_display() {
        let err = AddonLoaderError::AddonDisabled("test".to_string());
        assert_eq!(err.to_string(), "addon 'test' is disabled");
    }

    #[test]
    fn test_controller_not_found_display() {
        let err = AddonLoaderError::ControllerNotFound("admin.Order".to_string());
        assert_eq!(err.to_string(), "addon controller 'admin.Order' not found");
    }

    #[test]
    fn test_action_not_found_display() {
        let err = AddonLoaderError::ActionNotFound("index".to_string());
        assert_eq!(err.to_string(), "addon action 'index' not found");
    }

    #[test]
    fn test_manifest_parse_display() {
        let err = AddonLoaderError::ManifestParse {
            addon: "operate".to_string(),
            reason: "missing $info array".to_string(),
        };
        assert!(err.to_string().contains("operate"));
        assert!(err.to_string().contains("missing $info array"));
    }

    #[test]
    fn test_scan_dir_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such directory");
        let err = AddonLoaderError::ScanDir {
            path: "/addons".to_string(),
            source: io_err,
        };
        assert!(err.to_string().contains("/addons"));
    }

    #[test]
    fn test_read_file_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = AddonLoaderError::ReadFile {
            path: "/addons/operate/Plugin.php".to_string(),
            source: io_err,
        };
        assert!(err.to_string().contains("Plugin.php"));
    }

    #[test]
    fn test_autoload_miss_display() {
        let err = AddonLoaderError::AutoloadMiss {
            class: "addons\\operate\\Plugin".to_string(),
        };
        assert!(err.to_string().contains("addons\\operate\\Plugin"));
    }

    #[test]
    fn test_hook_register_display() {
        let err = AddonLoaderError::HookRegister {
            hook: "AddonsInit".to_string(),
            addon: "operate".to_string(),
        };
        assert!(err.to_string().contains("AddonsInit"));
        assert!(err.to_string().contains("operate"));
    }

    #[test]
    fn test_route_parse_display() {
        let err = AddonLoaderError::RouteParse {
            url: "/addons/operate".to_string(),
            reason: "missing controller".to_string(),
        };
        assert!(err.to_string().contains("/addons/operate"));
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::other("test");
        let err: AddonLoaderError = io_err.into();
        assert!(matches!(err, AddonLoaderError::ReadFile { .. }));
    }

    #[test]
    fn test_result_alias_ok() {
        let result: AddonLoaderResult<i32> = Ok(42);
        match result {
            Ok(v) => assert_eq!(v, 42),
            Err(_) => panic!("expected Ok"),
        }
    }

    #[test]
    fn test_result_alias_err() {
        let result: AddonLoaderResult<i32> = Err(AddonLoaderError::AddonNotFound("x".to_string()));
        assert!(result.is_err());
    }
}
