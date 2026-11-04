// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! CSP/HSTS/X-Frame-Options 安全头中间件（spec §5.19）
//!
//! 所有响应自动注入安全头。

use serde::{Deserialize, Serialize};

use crate::error::SecurityHeaderError;

/// X-Frame-Options（spec §6.19 规则 3）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum XFrameOptions {
    /// DENY — 完全禁止嵌入
    Deny,
    /// SAMEORIGIN — 同源可嵌入
    SameOrigin,
}

impl XFrameOptions {
    /// 转为 header 值
    pub fn as_str(&self) -> &'static str {
        match self {
            XFrameOptions::Deny => "DENY",
            XFrameOptions::SameOrigin => "SAMEORIGIN",
        }
    }
}

/// 安全头配置（spec §6.19）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityHeadersConfig {
    /// CSP 策略（默认 script-src 'self'，spec §5.19 规则 1）
    pub csp: String,
    /// HSTS max-age（默认 >= 31536000，spec §5.19 规则 2）
    pub hsts_max_age: u64,
    /// 是否含 preload
    pub hsts_preload: bool,
    /// X-Frame-Options（默认 DENY，spec §5.19 规则 3）
    pub x_frame_options: XFrameOptions,
    /// 是否启用 HSTS（关闭需显式配置，spec §5.19 禁止项）
    pub hsts_enabled: bool,
    /// X-Content-Type-Options
    pub x_content_type_options: bool,
    /// Referrer-Policy
    pub referrer_policy: String,
}

impl Default for SecurityHeadersConfig {
    fn default() -> Self {
        Self {
            csp: "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'".to_string(),
            hsts_max_age: 31536000,
            hsts_preload: true,
            x_frame_options: XFrameOptions::Deny,
            hsts_enabled: true,
            x_content_type_options: true,
            referrer_policy: "strict-origin-when-cross-origin".to_string(),
        }
    }
}

impl SecurityHeadersConfig {
    /// 验证配置（spec §6.19 规则 2）
    pub fn validate(&self) -> Result<(), SecurityHeaderError> {
        if self.hsts_enabled && self.hsts_max_age < 31536000 {
            return Err(SecurityHeaderError::InvalidConfig(format!(
                "HSTS max-age 必须 >= 31536000（1年），当前: {}",
                self.hsts_max_age
            )));
        }
        if self.csp.is_empty() {
            return Err(SecurityHeaderError::InvalidConfig(
                "CSP 策略不能为空".to_string(),
            ));
        }
        Ok(())
    }

    /// 生成 HSTS header 值
    pub fn hsts_header(&self) -> Option<String> {
        if !self.hsts_enabled {
            return None;
        }
        let mut value = format!("max-age={}", self.hsts_max_age);
        if self.hsts_preload {
            value.push_str("; preload");
        }
        Some(value)
    }

    /// 生成所有安全头键值对
    pub fn headers(&self) -> Vec<(String, String)> {
        let mut result = Vec::with_capacity(5);
        result.push(("Content-Security-Policy".to_string(), self.csp.clone()));
        if let Some(hsts) = self.hsts_header() {
            result.push(("Strict-Transport-Security".to_string(), hsts));
        }
        result.push((
            "X-Frame-Options".to_string(),
            self.x_frame_options.as_str().to_string(),
        ));
        if self.x_content_type_options {
            result.push(("X-Content-Type-Options".to_string(), "nosniff".to_string()));
        }
        result.push(("Referrer-Policy".to_string(), self.referrer_policy.clone()));
        result
    }
}

/// 安全头中间件
pub struct SecurityHeadersMiddleware {
    config: SecurityHeadersConfig,
}

impl SecurityHeadersMiddleware {
    /// 创建安全头中间件
    pub fn new(config: SecurityHeadersConfig) -> Result<Self, SecurityHeaderError> {
        config.validate()?;
        Ok(Self { config })
    }

    /// 创建默认配置中间件
    pub fn with_defaults() -> Result<Self, SecurityHeaderError> {
        Self::new(SecurityHeadersConfig::default())
    }

    /// 注入安全头到响应（spec §5.19 规则 1/2/3）
    pub fn apply(&self, headers: &mut Vec<(String, String)>) {
        for (key, value) in self.config.headers() {
            headers.push((key, value));
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &SecurityHeadersConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_valid() {
        let config = SecurityHeadersConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_hsts_too_short() {
        let config = SecurityHeadersConfig {
            hsts_max_age: 100,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_empty_csp() {
        let config = SecurityHeadersConfig {
            csp: "".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_hsts_disabled_short_max_age() {
        let config = SecurityHeadersConfig {
            hsts_enabled: false,
            hsts_max_age: 100,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_hsts_header() {
        let config = SecurityHeadersConfig::default();
        let hsts = config.hsts_header().unwrap();
        assert!(hsts.contains("max-age=31536000"));
        assert!(hsts.contains("preload"));
    }

    #[test]
    fn test_hsts_header_disabled() {
        let config = SecurityHeadersConfig {
            hsts_enabled: false,
            ..Default::default()
        };
        assert!(config.hsts_header().is_none());
    }

    #[test]
    fn test_headers_contains_all() {
        let config = SecurityHeadersConfig::default();
        let headers = config.headers();
        let keys: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"Content-Security-Policy"));
        assert!(keys.contains(&"Strict-Transport-Security"));
        assert!(keys.contains(&"X-Frame-Options"));
        assert!(keys.contains(&"X-Content-Type-Options"));
        assert!(keys.contains(&"Referrer-Policy"));
    }

    #[test]
    fn test_xframe_options_as_str() {
        assert_eq!(XFrameOptions::Deny.as_str(), "DENY");
        assert_eq!(XFrameOptions::SameOrigin.as_str(), "SAMEORIGIN");
    }

    #[test]
    fn test_middleware_apply() {
        let middleware = SecurityHeadersMiddleware::with_defaults().unwrap();
        let mut headers = Vec::new();
        middleware.apply(&mut headers);
        assert!(headers.len() >= 5);
        assert!(headers.iter().any(|(k, _)| k == "Content-Security-Policy"));
    }

    #[test]
    fn test_middleware_invalid_config() {
        let config = SecurityHeadersConfig {
            csp: "".to_string(),
            ..Default::default()
        };
        assert!(SecurityHeadersMiddleware::new(config).is_err());
    }

    #[test]
    fn test_custom_config() {
        let config = SecurityHeadersConfig {
            csp: "default-src 'none'".to_string(),
            hsts_max_age: 63072000,
            hsts_preload: false,
            x_frame_options: XFrameOptions::SameOrigin,
            hsts_enabled: true,
            x_content_type_options: false,
            referrer_policy: "no-referrer".to_string(),
        };
        let middleware = SecurityHeadersMiddleware::new(config).unwrap();
        let mut headers = Vec::new();
        middleware.apply(&mut headers);
        let csp = headers
            .iter()
            .find(|(k, _)| k == "Content-Security-Policy")
            .unwrap();
        assert_eq!(csp.1, "default-src 'none'");
        let xfo = headers
            .iter()
            .find(|(k, _)| k == "X-Frame-Options")
            .unwrap();
        assert_eq!(xfo.1, "SAMEORIGIN");
        assert!(!headers.iter().any(|(k, _)| k == "X-Content-Type-Options"));
    }
}
