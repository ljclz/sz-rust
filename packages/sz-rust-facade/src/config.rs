// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Config 静态门面
//!
//! 对齐 PHP `think\facade\Config`，委托全局 `sz_rust_infra_facade::config::AppConfig` 单例。

use std::sync::OnceLock;

use sz_rust_infra_facade::config::AppConfig;

static CONFIG_INSTANCE: OnceLock<AppConfig> = OnceLock::new();

/// Config 静态门面（对齐 PHP `think\facade\Config`）
///
/// 委托全局 `OnceCell<AppConfig>` 单例。需调用 [`Config::init`] 初始化。
pub struct Config;

impl Config {
    /// 初始化全局配置实例
    pub fn init(config: AppConfig) {
        let _ = CONFIG_INSTANCE.set(config);
    }

    /// 获取全局配置实例引用
    pub fn instance() -> Result<&'static AppConfig, crate::FacadeError> {
        CONFIG_INSTANCE
            .get()
            .ok_or(crate::FacadeError::NotInitialized("Config::init"))
    }

    /// 是否已初始化
    pub fn is_initialized() -> bool {
        CONFIG_INSTANCE.get().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_init_and_get() {
        // OnceLock is global; just verify init + get works
        let cfg = AppConfig::default();
        Config::init(cfg);
        assert!(Config::is_initialized());
        let _ = Config::instance().unwrap();
    }
}
