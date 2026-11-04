// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! 多 worker runtime 构建
//!
//! 根据 worker 数量构建 tokio multi-thread runtime，用于 serve 命令启动 HTTP 服务。

use crate::error::CliError;

/// 校验 worker 数量合法性
///
/// - `workers == 0` → 错误
/// - `workers > 1024` → 错误
pub fn validate_workers(workers: u16) -> Result<(), CliError> {
    if workers == 0 {
        return Err(CliError::Generic("worker 数量必须 >= 1".to_string()));
    }
    if workers > 1024 {
        return Err(CliError::Generic("worker 数量超过上限 1024".to_string()));
    }
    Ok(())
}

/// 构建 multi-thread tokio runtime
///
/// 指定 worker 线程数，启用所有功能（IO + time + 等）。
pub fn build_runtime(workers: u16) -> Result<tokio::runtime::Runtime, CliError> {
    validate_workers(workers)?;
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers as usize)
        .enable_all()
        .build()
        .map_err(|e| CliError::Generic(format!("tokio runtime 构建失败: {e}")))
}

/// 解析有效 worker 数
///
/// 优先级：CLI 参数 > 配置文件 > CPU 核心数
pub fn resolve_workers(cli_workers: Option<u16>, config_workers: u16) -> u16 {
    let w = cli_workers.unwrap_or(config_workers);
    if w == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get() as u16)
            .unwrap_or(1)
    } else {
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_workers_zero_rejected() {
        assert!(validate_workers(0).is_err());
    }

    #[test]
    fn test_validate_workers_one_ok() {
        assert!(validate_workers(1).is_ok());
    }

    #[test]
    fn test_validate_workers_over_limit_rejected() {
        assert!(validate_workers(1025).is_err());
    }

    #[test]
    fn test_validate_workers_at_limit_ok() {
        assert!(validate_workers(1024).is_ok());
    }

    #[test]
    fn test_build_runtime_one_worker() {
        assert!(build_runtime(1).is_ok());
    }

    #[test]
    fn test_build_runtime_zero_rejected() {
        assert!(build_runtime(0).is_err());
    }

    #[test]
    fn test_build_runtime_two_workers() {
        assert!(build_runtime(2).is_ok());
    }

    #[test]
    fn test_resolve_workers_cli_priority() {
        assert_eq!(resolve_workers(Some(4), 8), 4);
    }

    #[test]
    fn test_resolve_workers_config_fallback() {
        assert_eq!(resolve_workers(None, 8), 8);
    }

    #[test]
    fn test_resolve_workers_zero_falls_to_cpu() {
        let resolved = resolve_workers(None, 0);
        assert!(resolved >= 1, "CPU 核心数至少为 1");
    }
}
