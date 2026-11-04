// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Log 静态门面
//!
//! 对齐 PHP `think\facade\Log`，委托全局 `sz_rust_middleware_facade::log::LogFacade` 单例。

use std::sync::OnceLock;

use sz_rust_middleware_facade::log::{LogFacade, LogLevel};

static LOG_INSTANCE: OnceLock<LogFacade> = OnceLock::new();

/// Log 静态门面（对齐 PHP `think\facade\Log`）
///
/// 委托全局 `OnceCell<LogFacade>` 单例。需调用 [`Log::init`] 初始化。
pub struct Log;

impl Log {
    /// 初始化全局日志实例（需要 LogSection 配置）
    pub fn init(facade: LogFacade) {
        let _ = LOG_INSTANCE.set(facade);
    }

    /// 获取全局日志实例引用
    fn instance() -> Result<&'static LogFacade, crate::FacadeError> {
        LOG_INSTANCE
            .get()
            .ok_or(crate::FacadeError::NotInitialized("Log::init"))
    }

    /// 记录日志（对齐 PHP `Log::log($level, $message)`）
    pub fn log(level: LogLevel, msg: &str) -> Result<(), crate::FacadeError> {
        Self::instance().map(|f| f.log(level, msg))
    }

    /// DEBUG 日志（对齐 PHP `Log::debug($message)`）
    pub fn debug(msg: &str) -> Result<(), crate::FacadeError> {
        Self::instance().map(|f| f.debug(msg))
    }

    /// INFO 日志（对齐 PHP `Log::info($message)`）
    pub fn info(msg: &str) -> Result<(), crate::FacadeError> {
        Self::instance().map(|f| f.info(msg))
    }

    /// WARN 日志（对齐 PHP `Log::warning($message)`）
    pub fn warn(msg: &str) -> Result<(), crate::FacadeError> {
        Self::instance().map(|f| f.warn(msg))
    }

    /// ERROR 日志（对齐 PHP `Log::error($message)`）
    pub fn error(msg: &str) -> Result<(), crate::FacadeError> {
        Self::instance().map(|f| f.error(msg))
    }

    /// 是否已初始化
    pub fn is_initialized() -> bool {
        LOG_INSTANCE.get().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_not_initialized() {
        assert!(!Log::is_initialized());
        let result = Log::info("test");
        assert!(result.is_err());
    }
}
