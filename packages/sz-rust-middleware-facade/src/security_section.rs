//! Security 配置段 — 聚合 4 个安全中间件配置
//!
//! 对齐 spec §6.1-6.4 + design §2.2.2。
//!
//! 定义在 `sz-rust-middleware-facade` 内以避免与 `sz-rust-infra-facade` 的循环依赖
//! （middleware-facade → infra-facade 已存在，反向依赖会成环）。
//!
//! 应用层通过 `SecuritySection` 从 YAML 加载安全配置，再分别传入
//! `MiddlewareBuilder::with_security_headers()` 等方法。

use serde::Deserialize;

use crate::audit_log::AuditLogConfig;
use crate::body_size_limit::BodySizeLimitConfig;
use crate::ip_access_control::IpAccessControlConfig;
use crate::security_headers::SecurityHeadersConfig;

/// Security 配置段（聚合 4 个安全中间件配置）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SecuritySection {
    /// 安全响应头配置
    #[serde(default)]
    pub headers: SecurityHeadersConfig,
    /// IP 访问控制配置
    #[serde(default)]
    pub ip_access: IpAccessControlConfig,
    /// 安全审计日志配置
    #[serde(default)]
    pub audit: AuditLogConfig,
    /// 请求体大小限制配置
    #[serde(default)]
    pub body_size: BodySizeLimitConfig,
}

impl SecuritySection {
    /// 校验安全配置段
    ///
    /// 校验规则：
    /// - `ip_access.enabled` 时所有 `ip_list` 条目可解析为 `ipnet::IpNet`（fail-fast）
    /// - `ip_list.len() > 10000` 时记录 `tracing::warn` 建议使用 CIDR 聚合
    /// - `audit.sample_rate` ∈ [0.0, 1.0]
    /// - `body_size.enabled` 时 `max_body_size > 0`
    pub fn validate(&self) -> Result<(), SecurityConfigError> {
        if self.ip_access.enabled {
            for entry in &self.ip_access.ip_list {
                if entry.parse::<ipnet::IpNet>().is_err() {
                    entry.parse::<std::net::IpAddr>().map_err(|e| {
                        SecurityConfigError::InvalidIpOrCidr {
                            entry: entry.clone(),
                            source: e,
                        }
                    })?;
                }
            }
            if self.ip_access.ip_list.len() > 10000 {
                tracing::warn!(
                    count = self.ip_access.ip_list.len(),
                    "ip_list 超过 10000 条，建议使用 CIDR 聚合减少匹配开销"
                );
            }
        }

        if self.audit.sample_rate < 0.0 || self.audit.sample_rate > 1.0 {
            return Err(SecurityConfigError::SampleRateOutOfRange {
                value: self.audit.sample_rate,
            });
        }

        if self.body_size.enabled && self.body_size.max_body_size == 0 {
            return Err(SecurityConfigError::MaxBodySizeZero);
        }

        Ok(())
    }
}

/// 安全配置校验错误
#[derive(Debug, thiserror::Error)]
pub enum SecurityConfigError {
    /// IP 或 CIDR 解析失败
    #[error("IP/CIDR 解析失败: {entry} — {source}")]
    InvalidIpOrCidr {
        /// 无法解析的条目
        entry: String,
        /// 底层解析错误
        #[source]
        source: std::net::AddrParseError,
    },
    /// 采样率越界
    #[error("audit.sample_rate 越界: {value}，应在 [0.0, 1.0]")]
    SampleRateOutOfRange {
        /// 越界的值
        value: f64,
    },
    /// max_body_size 为 0
    #[error("body_size.max_body_size 为 0，启用时必须大于 0")]
    MaxBodySizeZero,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_section() {
        let section = SecuritySection::default();
        assert!(section.headers.enabled, "headers 默认启用");
        assert!(!section.ip_access.enabled, "ip_access 默认不启用");
        assert!(!section.audit.enabled, "audit 默认不启用");
        assert!(!section.body_size.enabled, "body_size 默认不启用");
    }

    #[test]
    fn test_validate_default_passes() {
        let section = SecuritySection::default();
        assert!(section.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_cidr_fails() {
        let mut section = SecuritySection::default();
        section.ip_access.enabled = true;
        section.ip_access.ip_list = vec!["not-a-valid-cidr".to_string()];
        let err = section.validate().unwrap_err();
        assert!(matches!(err, SecurityConfigError::InvalidIpOrCidr { .. }));
    }

    #[test]
    fn test_validate_valid_cidr_passes() {
        let mut section = SecuritySection::default();
        section.ip_access.enabled = true;
        section.ip_access.ip_list = vec![
            "10.0.0.0/8".to_string(),
            "192.168.1.1".to_string(),
            "::1/128".to_string(),
        ];
        assert!(section.validate().is_ok());
    }

    #[test]
    fn test_validate_sample_rate_out_of_range() {
        let mut section = SecuritySection::default();
        section.audit.sample_rate = 1.5;
        let err = section.validate().unwrap_err();
        assert!(matches!(
            err,
            SecurityConfigError::SampleRateOutOfRange { value: 1.5 }
        ));
    }

    #[test]
    fn test_validate_sample_rate_negative() {
        let mut section = SecuritySection::default();
        section.audit.sample_rate = -0.1;
        let err = section.validate().unwrap_err();
        assert!(matches!(
            err,
            SecurityConfigError::SampleRateOutOfRange { .. }
        ));
    }

    #[test]
    fn test_validate_max_body_size_zero() {
        let mut section = SecuritySection::default();
        section.body_size.enabled = true;
        section.body_size.max_body_size = 0;
        let err = section.validate().unwrap_err();
        assert!(matches!(err, SecurityConfigError::MaxBodySizeZero));
    }

    #[test]
    fn test_validate_disabled_body_size_zero_ok() {
        let mut section = SecuritySection::default();
        section.body_size.enabled = false;
        section.body_size.max_body_size = 0;
        assert!(section.validate().is_ok());
    }
}
