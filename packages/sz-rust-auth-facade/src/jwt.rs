// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! JWT Audience 校验模块
//!
//! 提供 [`JwtAudienceValidator`]，根据配置的期望 audience 列表校验 JWT `aud` 字段。
//!
//! # 语义
//!
//! - 未配置任何期望 audience → 跳过校验（[`Ok`]）
//! - 配置了期望 audience 但 token `aud` 缺失 → 记录 doc-debt 告警并返回 [`JwtAudienceError::Missing`]
//! - `aud` 与任一期望值匹配 → [`Ok`]
//! - `aud` 不匹配 → [`JwtAudienceError::Mismatch`]
//!
//! # 待上游
//!
//! sz-orm-auth 尚未补全 `aud` 字段解析时，配置了 audience 但传入 `None` 会触发
//! [`tracing::warn`] 记录 doc-debt，提示 audience 校验未生效。
//!
//! # 示例
//!
//! ```
//! use sz_rust_auth_facade::JwtAudienceValidator;
//!
//! let validator = JwtAudienceValidator::new(vec!["api".to_string()]);
//! assert!(validator.validate(Some("api")).is_ok());
//! assert!(validator.validate(Some("web")).is_err());
//! ```

/// JWT Audience 校验错误
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JwtAudienceError {
    /// 调用方要求强制校验但 validator 未配置任何期望 audience
    #[error("audience validation not configured")]
    NotConfigured,
    /// 配置了期望 audience 但 token 的 `aud` 字段缺失
    #[error("aud claim missing while audience validation is configured")]
    Missing,
    /// token `aud` 与任一期望 audience 不匹配
    #[error("aud mismatch: expected one of {expected:?}, got {actual}")]
    Mismatch {
        /// 期望的 audience 列表
        expected: Vec<String>,
        /// 实际的 audience
        actual: String,
    },
}

/// JWT Audience 校验器
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtAudienceValidator {
    expected: Vec<String>,
}

impl JwtAudienceValidator {
    /// 构造校验器，`expected` 为空表示不启用 audience 校验。
    #[inline]
    #[must_use]
    pub fn new(expected: Vec<String>) -> Self {
        Self { expected }
    }

    /// 是否已配置 audience 校验。
    #[inline]
    #[must_use]
    pub fn is_configured(&self) -> bool {
        !self.expected.is_empty()
    }

    /// 校验 token 的 `aud` 字段。
    ///
    /// 未配置期望 audience 时直接放行（[`Ok`]）；配置后缺失或不匹配则报错。
    pub fn validate(&self, claims_aud: Option<&str>) -> Result<(), JwtAudienceError> {
        if self.expected.is_empty() {
            return Ok(());
        }
        match claims_aud {
            None => {
                // 待上游：sz-orm-auth 未补 aud 字段时校验未生效，记录 doc-debt
                tracing::warn!(
                    target: "sz_rust_auth_facade::jwt",
                    "aud claim missing while audience validation configured; \
                     upstream sz-orm-auth may not populate aud field (doc-debt T048)"
                );
                Err(JwtAudienceError::Missing)
            }
            Some(aud) if self.expected.iter().any(|e| e.as_str() == aud) => Ok(()),
            Some(aud) => Err(JwtAudienceError::Mismatch {
                expected: self.expected.clone(),
                actual: aud.to_string(),
            }),
        }
    }

    /// 严格校验：未配置期望 audience 时返回 [`JwtAudienceError::NotConfigured`]，
    /// 其余语义同 [`validate`](Self::validate)。
    pub fn validate_strict(&self, claims_aud: Option<&str>) -> Result<(), JwtAudienceError> {
        if self.expected.is_empty() {
            return Err(JwtAudienceError::NotConfigured);
        }
        self.validate(claims_aud)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_skips_validation() {
        let v = JwtAudienceValidator::new(vec![]);
        assert!(!v.is_configured());
        assert!(v.validate(None).is_ok());
        assert!(v.validate(Some("anything")).is_ok());
    }

    #[test]
    fn configured_match_passes() {
        let v = JwtAudienceValidator::new(vec!["api".into(), "web".into()]);
        assert!(v.is_configured());
        assert!(v.validate(Some("api")).is_ok());
        assert!(v.validate(Some("web")).is_ok());
    }

    #[test]
    fn configured_mismatch_fails() {
        let v = JwtAudienceValidator::new(vec!["api".into()]);
        let err = v.validate(Some("web")).unwrap_err();
        assert_eq!(
            err,
            JwtAudienceError::Mismatch {
                expected: vec!["api".into()],
                actual: "web".into(),
            }
        );
    }

    #[test]
    fn configured_missing_aud_fails() {
        let v = JwtAudienceValidator::new(vec!["api".into()]);
        let err = v.validate(None).unwrap_err();
        assert_eq!(err, JwtAudienceError::Missing);
    }

    #[test]
    fn strict_unconfigured_fails() {
        let v = JwtAudienceValidator::new(vec![]);
        assert_eq!(
            v.validate_strict(None).unwrap_err(),
            JwtAudienceError::NotConfigured
        );
        assert_eq!(
            v.validate_strict(Some("x")).unwrap_err(),
            JwtAudienceError::NotConfigured
        );
    }

    #[test]
    fn strict_configured_behaves_like_validate() {
        let v = JwtAudienceValidator::new(vec!["api".into()]);
        assert!(v.validate_strict(Some("api")).is_ok());
        assert_eq!(
            v.validate_strict(Some("web")).unwrap_err(),
            JwtAudienceError::Mismatch {
                expected: vec!["api".into()],
                actual: "web".into(),
            }
        );
        assert_eq!(
            v.validate_strict(None).unwrap_err(),
            JwtAudienceError::Missing
        );
    }

    #[test]
    fn empty_string_aud_is_valid_value() {
        let v = JwtAudienceValidator::new(vec!["".into()]);
        assert!(v.validate(Some("")).is_ok());
        assert!(matches!(
            v.validate(Some("x")).unwrap_err(),
            JwtAudienceError::Mismatch { .. }
        ));
    }

    #[test]
    fn clone_eq() {
        let v = JwtAudienceValidator::new(vec!["api".into()]);
        assert_eq!(v, v.clone());
    }
}
