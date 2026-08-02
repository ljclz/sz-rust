//! pgwire 协议消息合规性检查模块。
//!
//! 验证 SzRSQL 的 pgwire 消息字段与 PostgreSQL 前端/后端协议 v3.0 规范一致。
//!
//! # 检查内容
//!
//! - SQLSTATE 格式（5 字符）
//! - Severity 字符串（大写首字母）
//! - StartupMessage 协议版本号
//! - 认证机制名称
//! - 消息类型标识符
//!
//! # 参考
//!
//! - PostgreSQL 协议规范：<https://www.postgresql.org/docs/current/protocol.html>
//! - PostgreSQL 消息格式：<https://www.postgresql.org/docs/current/protocol-message-formats.html>

use crate::CompatStatus;
use serde::{Deserialize, Serialize};
use szrsql_protocol::pgwire::auth::SCRAM_MECHANISM;
use szrsql_protocol::pgwire::message::{Severity, SqlState};
use szrsql_protocol::pgwire::startup::PROTOCOL_VERSION_3_0;

/// 单项协议合规性检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolConformanceResult {
    /// 检查项名称
    pub name: String,
    /// 期望值（PG 规范）
    pub expected: String,
    /// 实际值（SzRSQL 实现）
    pub actual: String,
    /// 兼容性状态
    pub status: CompatStatus,
    /// 详细说明
    pub detail: String,
}

/// 协议合规性检查套件
pub struct ProtocolConformance;

impl ProtocolConformance {
    /// 运行全部协议合规性检查
    pub fn run_all() -> Vec<ProtocolConformanceResult> {
        vec![
            Self::check_sqlstate_format(),
            Self::check_sqlstate_successful_completion(),
            Self::check_severity_error(),
            Self::check_severity_fatal(),
            Self::check_severity_warning(),
            Self::check_severity_notice(),
            Self::check_protocol_version(),
            Self::check_scram_mechanism_name(),
        ]
    }

    /// 检查 SQLSTATE 格式：必须为 5 字符
    fn check_sqlstate_format() -> ProtocolConformanceResult {
        let sample = SqlState::SYNTAX_ERROR.as_str();
        let expected = "5 字符长度字符串".to_string();
        let actual = format!("\"{sample}\" (长度 {})", sample.len());
        let ok = sample.len() == 5;

        ProtocolConformanceResult {
            name: "SQLSTATE 格式（5 字符）".to_string(),
            expected,
            actual,
            status: if ok {
                CompatStatus::Pass
            } else {
                CompatStatus::Fail
            },
            detail: if ok {
                "SQLSTATE 符合 PG 规范的 5 字符格式".to_string()
            } else {
                format!("SQLSTATE 长度 {} 不符合 PG 规范（应为 5）", sample.len())
            },
        }
    }

    /// 检查成功完成的 SQLSTATE = "00000"
    fn check_sqlstate_successful_completion() -> ProtocolConformanceResult {
        let actual = SqlState::SUCCESSFUL_COMPLETION.as_str();
        let expected = "00000".to_string();
        let ok = actual == expected;

        ProtocolConformanceResult {
            name: "SUCCESSFUL_COMPLETION = 00000".to_string(),
            expected,
            actual: actual.to_string(),
            status: if ok {
                CompatStatus::Pass
            } else {
                CompatStatus::Fail
            },
            detail: if ok {
                "成功完成码与 PG 官方一致".to_string()
            } else {
                "成功完成码与 PG 官方不一致".to_string()
            },
        }
    }

    /// 检查 Severity::Error = "ERROR"
    fn check_severity_error() -> ProtocolConformanceResult {
        let actual = Severity::Error.as_str();
        let expected = "ERROR".to_string();
        let ok = actual == expected;

        ProtocolConformanceResult {
            name: "Severity::Error = \"ERROR\"".to_string(),
            expected,
            actual: actual.to_string(),
            status: if ok {
                CompatStatus::Pass
            } else {
                CompatStatus::Fail
            },
            detail: if ok {
                "ERROR 严重性与 PG 官方一致".to_string()
            } else {
                "ERROR 严重性与 PG 官方不一致".to_string()
            },
        }
    }

    /// 检查 Severity::Fatal = "FATAL"
    fn check_severity_fatal() -> ProtocolConformanceResult {
        let actual = Severity::Fatal.as_str();
        let expected = "FATAL".to_string();
        let ok = actual == expected;

        ProtocolConformanceResult {
            name: "Severity::Fatal = \"FATAL\"".to_string(),
            expected,
            actual: actual.to_string(),
            status: if ok {
                CompatStatus::Pass
            } else {
                CompatStatus::Fail
            },
            detail: if ok {
                "FATAL 严重性与 PG 官方一致".to_string()
            } else {
                "FATAL 严重性与 PG 官方不一致".to_string()
            },
        }
    }

    /// 检查 Severity::Warning = "WARNING"
    fn check_severity_warning() -> ProtocolConformanceResult {
        let actual = Severity::Warning.as_str();
        let expected = "WARNING".to_string();
        let ok = actual == expected;

        ProtocolConformanceResult {
            name: "Severity::Warning = \"WARNING\"".to_string(),
            expected,
            actual: actual.to_string(),
            status: if ok {
                CompatStatus::Pass
            } else {
                CompatStatus::Fail
            },
            detail: if ok {
                "WARNING 严重性与 PG 官方一致".to_string()
            } else {
                "WARNING 严重性与 PG 官方不一致".to_string()
            },
        }
    }

    /// 检查 Severity::Notice = "NOTICE"
    fn check_severity_notice() -> ProtocolConformanceResult {
        let actual = Severity::Notice.as_str();
        let expected = "NOTICE".to_string();
        let ok = actual == expected;

        ProtocolConformanceResult {
            name: "Severity::Notice = \"NOTICE\"".to_string(),
            expected,
            actual: actual.to_string(),
            status: if ok {
                CompatStatus::Pass
            } else {
                CompatStatus::Fail
            },
            detail: if ok {
                "NOTICE 严重性与 PG 官方一致".to_string()
            } else {
                "NOTICE 严重性与 PG 官方不一致".to_string()
            },
        }
    }

    /// 检查协议版本号 = 196608 (3.0)
    fn check_protocol_version() -> ProtocolConformanceResult {
        let actual = PROTOCOL_VERSION_3_0;
        let expected: i32 = 196_608; // (3 << 16) | 0
        let ok = actual == expected;

        ProtocolConformanceResult {
            name: "协议版本号 = 196608 (v3.0)".to_string(),
            expected: expected.to_string(),
            actual: actual.to_string(),
            status: if ok {
                CompatStatus::Pass
            } else {
                CompatStatus::Fail
            },
            detail: if ok {
                "协议版本号与 PG v3.0 一致".to_string()
            } else {
                format!("协议版本号 {actual} 与 PG v3.0 ({expected}) 不一致")
            },
        }
    }

    /// 检查 SCRAM-SHA-256 机制名称
    fn check_scram_mechanism_name() -> ProtocolConformanceResult {
        let actual = SCRAM_MECHANISM;
        let expected = "SCRAM-SHA-256";
        let ok = actual == expected;

        ProtocolConformanceResult {
            name: "SCRAM 机制名称 = \"SCRAM-SHA-256\"".to_string(),
            expected: expected.to_string(),
            actual: actual.to_string(),
            status: if ok {
                CompatStatus::Pass
            } else {
                CompatStatus::Fail
            },
            detail: if ok {
                "SCRAM 机制名称与 RFC 5802 一致".to_string()
            } else {
                "SCRAM 机制名称与 RFC 5802 不一致".to_string()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_returns_nonempty() {
        let results = ProtocolConformance::run_all();
        assert!(!results.is_empty(), "应返回至少一项检查结果");
    }

    #[test]
    fn all_checks_should_pass() {
        let results = ProtocolConformance::run_all();
        for r in &results {
            assert_eq!(
                r.status,
                CompatStatus::Pass,
                "协议合规检查项 \"{}\" 应通过：{}",
                r.name,
                r.detail
            );
        }
    }

    #[test]
    fn sqlstate_format_check_present() {
        let results = ProtocolConformance::run_all();
        assert!(
            results.iter().any(|r| r.name.contains("SQLSTATE")),
            "应包含 SQLSTATE 格式检查"
        );
    }

    #[test]
    fn protocol_version_check_present() {
        let results = ProtocolConformance::run_all();
        assert!(
            results.iter().any(|r| r.name.contains("协议版本号")),
            "应包含协议版本号检查"
        );
    }
}
