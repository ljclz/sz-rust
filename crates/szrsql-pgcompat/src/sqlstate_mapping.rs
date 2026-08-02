//! SQLSTATE 错误码映射验证模块。
//!
//! 验证 SzRSQL 的 `SqlState` 常量与 PostgreSQL 官方 SQLSTATE 列表一致。
//!
//! # 参考
//!
//! - PostgreSQL SQLSTATE 列表：<https://www.postgresql.org/docs/current/errcodes-appendix.html>
//! - SzRSQL 实现：`szrsql_protocol::pgwire::message::SqlState`

use crate::CompatStatus;
use serde::{Deserialize, Serialize};
use szrsql_protocol::pgwire::message::SqlState;

/// 单项 SQLSTATE 映射检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlStateMappingResult {
    /// 检查项名称（如 "syntax_error"）
    pub name: String,
    /// SzRSQL 常量名（如 "SYNTAX_ERROR"）
    pub szrsql_constant: String,
    /// 期望的 PostgreSQL SQLSTATE 值（5 字符）
    pub expected_pg_code: String,
    /// SzRSQL 实际返回的 SQLSTATE 值
    pub actual_szrsql_code: String,
    /// 兼容性状态
    pub status: CompatStatus,
    /// 详细说明
    pub detail: String,
}

/// SQLSTATE 映射验证套件
pub struct SqlStateMapping;

impl SqlStateMapping {
    /// 运行全部 SQLSTATE 映射检查
    pub fn run_all() -> Vec<SqlStateMappingResult> {
        // (常量名, SzRSQL SqlState, PostgreSQL 官方 SQLSTATE, 说明)
        let cases: &[(&str, SqlState, &str, &str)] = &[
            (
                "successful_completion",
                SqlState::SUCCESSFUL_COMPLETION,
                "00000",
                "成功完成",
            ),
            ("syntax_error", SqlState::SYNTAX_ERROR, "42601", "语法错误"),
            (
                "invalid_authorization_specification",
                SqlState::INVALID_AUTHORIZATION_SPECIFICATION,
                "28000",
                "无效的授权规范",
            ),
            (
                "protocol_violation",
                SqlState::PROTOCOL_VIOLATION,
                "08P01",
                "协议违反",
            ),
            (
                "connection_exception",
                SqlState::CONNECTION_EXCEPTION,
                "08000",
                "连接异常",
            ),
            (
                "feature_not_supported",
                SqlState::FEATURE_NOT_SUPPORTED,
                "0A000",
                "不支持的功能",
            ),
            (
                "internal_error",
                SqlState::INTERNAL_ERROR,
                "XX000",
                "内部错误",
            ),
            (
                "undefined_table",
                SqlState::UNDEFINED_TABLE,
                "42P01",
                "未定义表",
            ),
            (
                "undefined_column",
                SqlState::UNDEFINED_COLUMN,
                "42703",
                "未定义列",
            ),
            (
                "duplicate_table",
                SqlState::DUPLICATE_TABLE,
                "42P07",
                "重复表",
            ),
            (
                "foreign_key_violation",
                SqlState::FOREIGN_KEY_VIOLATION,
                "23503",
                "外键约束违反",
            ),
            (
                "check_violation",
                SqlState::CHECK_VIOLATION,
                "23514",
                "CHECK 约束违反",
            ),
            (
                "invalid_text_representation",
                SqlState::INVALID_TEXT_REPRESENTATION,
                "22P02",
                "无效的文本表示",
            ),
            (
                "undefined_object",
                SqlState::UNDEFINED_OBJECT,
                "42704",
                "未定义对象",
            ),
            (
                "duplicate_object",
                SqlState::DUPLICATE_OBJECT,
                "42710",
                "重复对象",
            ),
            (
                "invalid_transaction_state",
                SqlState::INVALID_TRANSACTION_STATE,
                "25000",
                "无效事务状态",
            ),
        ];

        cases
            .iter()
            .map(|(name, szrsql_state, expected, desc)| {
                Self::check_one(name, szrsql_state, expected, desc)
            })
            .collect()
    }

    /// 验证单个 SQLSTATE 映射
    fn check_one(
        name: &str,
        szrsql_state: &SqlState,
        expected_pg: &str,
        desc: &str,
    ) -> SqlStateMappingResult {
        let actual = szrsql_state.as_str();
        let constant_name = Self::constant_name_for(szrsql_state);

        // 校验 SQLSTATE 格式：必须是 5 字符
        let format_ok = actual.len() == 5;

        // 校验与 PostgreSQL 官方一致
        let value_ok = actual == expected_pg;

        let (status, detail) = if !format_ok {
            (
                CompatStatus::Fail,
                format!("SQLSTATE 格式错误：长度 {}（应为 5）", actual.len()),
            )
        } else if !value_ok {
            (
                CompatStatus::Fail,
                format!("SQLSTATE 值不匹配：SzRSQL={actual}, PG={expected_pg}"),
            )
        } else {
            (
                CompatStatus::Pass,
                format!("SQLSTATE 一致：{actual}（{desc}）"),
            )
        };

        SqlStateMappingResult {
            name: name.to_string(),
            szrsql_constant: constant_name,
            expected_pg_code: expected_pg.to_string(),
            actual_szrsql_code: actual.to_string(),
            status,
            detail,
        }
    }

    /// 获取 SqlState 对应的常量名（用于报告展示）
    fn constant_name_for(state: &SqlState) -> String {
        let code = state.as_str();
        match code {
            "00000" => "SUCCESSFUL_COMPLETION".to_string(),
            "42601" => "SYNTAX_ERROR".to_string(),
            "28000" => "INVALID_AUTHORIZATION_SPECIFICATION".to_string(),
            "08P01" => "PROTOCOL_VIOLATION".to_string(),
            "08000" => "CONNECTION_EXCEPTION".to_string(),
            "0A000" => "FEATURE_NOT_SUPPORTED".to_string(),
            "XX000" => "INTERNAL_ERROR".to_string(),
            "42P01" => "UNDEFINED_TABLE".to_string(),
            "42703" => "UNDEFINED_COLUMN".to_string(),
            "42P07" => "DUPLICATE_TABLE".to_string(),
            "23503" => "FOREIGN_KEY_VIOLATION".to_string(),
            "23514" => "CHECK_VIOLATION".to_string(),
            "22P02" => "INVALID_TEXT_REPRESENTATION".to_string(),
            "42704" => "UNDEFINED_OBJECT".to_string(),
            "42710" => "DUPLICATE_OBJECT".to_string(),
            "25000" => "INVALID_TRANSACTION_STATE".to_string(),
            _ => format!("UNKNOWN({code})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_returns_nonempty() {
        let results = SqlStateMapping::run_all();
        assert!(!results.is_empty(), "应返回至少一项检查结果");
        assert!(results.len() >= 16, "应覆盖至少 16 个 SQLSTATE 常量");
    }

    #[test]
    fn all_results_have_5char_codes() {
        let results = SqlStateMapping::run_all();
        for r in &results {
            assert_eq!(
                r.expected_pg_code.len(),
                5,
                "PG SQLSTATE 应为 5 字符: {}",
                r.name
            );
            assert_eq!(
                r.actual_szrsql_code.len(),
                5,
                "SzRSQL SQLSTATE 应为 5 字符: {}",
                r.name
            );
        }
    }

    #[test]
    fn syntax_error_maps_correctly() {
        let results = SqlStateMapping::run_all();
        let syntax = results
            .iter()
            .find(|r| r.name == "syntax_error")
            .expect("应包含 syntax_error");
        assert_eq!(syntax.expected_pg_code, "42601");
        assert_eq!(syntax.actual_szrsql_code, "42601");
        assert_eq!(syntax.status, CompatStatus::Pass);
    }

    #[test]
    fn all_pg_official_codes_match() {
        let results = SqlStateMapping::run_all();
        for r in &results {
            assert_eq!(
                r.status,
                CompatStatus::Pass,
                "SQLSTATE {} 应与 PG 官方一致（SzRSQL={}, PG={}）",
                r.name,
                r.actual_szrsql_code,
                r.expected_pg_code
            );
        }
    }
}
