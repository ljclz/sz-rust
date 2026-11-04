// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 降级响应（P3-3）
//!
//! 熔断打开时返回配置的降级响应（非错误）。

use crate::error::GatewayError;

/// 降级响应配置
///
/// 熔断打开时返回此响应，而非直接返回错误。
#[derive(Debug, Clone)]
pub struct DegradeResponse {
    /// HTTP 状态码
    pub status: u16,
    /// 响应体
    pub body: String,
}

impl Default for DegradeResponse {
    fn default() -> Self {
        Self {
            status: 503,
            body: r#"{"error":"service_degraded","message":"service temporarily unavailable"}"#
                .to_string(),
        }
    }
}

impl DegradeResponse {
    /// 创建降级响应
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    /// 构建降级错误
    pub fn to_error(&self) -> GatewayError {
        GatewayError::CircuitBroken(format!("degraded: status={}", self.status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_degrade_response() {
        let dr = DegradeResponse::default();
        assert_eq!(dr.status, 503);
        assert!(dr.body.contains("service_degraded"));
    }

    #[test]
    fn test_custom_degrade_response() {
        let dr = DegradeResponse::new(429, "rate limited");
        assert_eq!(dr.status, 429);
        assert_eq!(dr.body, "rate limited");
    }

    #[test]
    fn test_to_error() {
        let dr = DegradeResponse::new(503, "unavailable");
        let err = dr.to_error();
        assert!(err.to_string().contains("degraded"));
    }
}
