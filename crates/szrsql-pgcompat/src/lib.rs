//! SzRSQL PostgreSQL 兼容性测试套件。
//!
//! 本 crate 提供 SzRSQL 与 PostgreSQL 的兼容性验证能力，覆盖四个维度：
//!
//! 1. **SQL 语法兼容性**（[`sql_syntax`]）：验证 SzRSQL 解析器能否正确解析
//!    PostgreSQL 特有语法（DDL/DML/函数/类型/运算符）。
//! 2. **SQLSTATE 错误码映射**（[`sqlstate_mapping`]）：验证 SzRSQL 的 `SqlState`
//!    常量与 PostgreSQL 官方 SQLSTATE 列表一致。
//! 3. **数据类型映射**（[`data_type_mapping`]）：验证 PostgreSQL 数据类型名
//!    与 SzRSQL `ColumnType` 的映射关系。
//! 4. **协议消息合规性**（[`protocol_conformance`]）：验证 pgwire 消息字段
//!    与 PostgreSQL 前端/后端协议 v3.0 规范一致。
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_pgcompat::CompatReport;
//!
//! let report = CompatReport::run_all();
//! println!("{}", report.summary());
//! assert!(report.passed_count() >= report.total_count() / 2);
//! ```
//!
//! # 参考
//!
//! - PostgreSQL 协议规范：<https://www.postgresql.org/docs/current/protocol.html>
//! - PostgreSQL SQLSTATE 列表：<https://www.postgresql.org/docs/current/errcodes-appendix.html>
//! - PostgreSQL 数据类型：<https://www.postgresql.org/docs/current/datatype.html>

#![allow(dead_code)]

pub mod data_type_mapping;
pub mod protocol_conformance;
pub mod sql_syntax;
pub mod sqlstate_mapping;

pub use data_type_mapping::{DataTypeMapping, DataTypeMappingResult};
pub use protocol_conformance::{ProtocolConformance, ProtocolConformanceResult};
pub use sql_syntax::{SqlSyntaxCompat, SqlSyntaxResult, SyntaxCategory};
pub use sqlstate_mapping::{SqlStateMapping, SqlStateMappingResult};

use serde::{Deserialize, Serialize};

/// 兼容性检查结果状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompatStatus {
    /// 完全兼容
    Pass,
    /// 部分兼容（核心功能可用，但有限制）
    Partial,
    /// 不兼容（解析失败或行为不一致）
    Fail,
    /// 未实现（功能尚未开发）
    NotImplemented,
}

impl CompatStatus {
    /// 返回状态的字符串表示
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Partial => "PARTIAL",
            Self::Fail => "FAIL",
            Self::NotImplemented => "NOT_IMPLEMENTED",
        }
    }
}

/// 单项兼容性检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatItem {
    /// 检查项名称（如 "SELECT with LIMIT"）
    pub name: String,
    /// 检查项分类（如 "DML" / "SQLSTATE" / "DataType" / "Protocol"）
    pub category: String,
    /// 检查结果状态
    pub status: CompatStatus,
    /// 详细说明（失败原因或通过备注）
    pub detail: String,
}

/// 兼容性报告汇总
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatReport {
    /// SQL 语法兼容性检查项
    pub syntax: Vec<SqlSyntaxResult>,
    /// SQLSTATE 映射检查项
    pub sqlstate: Vec<SqlStateMappingResult>,
    /// 数据类型映射检查项
    pub data_types: Vec<DataTypeMappingResult>,
    /// 协议合规性检查项
    pub protocol: Vec<ProtocolConformanceResult>,
}

impl CompatReport {
    /// 运行全部四类兼容性检查并生成报告
    pub fn run_all() -> Self {
        Self {
            syntax: SqlSyntaxCompat::run_all(),
            sqlstate: SqlStateMapping::run_all(),
            data_types: DataTypeMapping::run_all(),
            protocol: ProtocolConformance::run_all(),
        }
    }

    /// 返回所有检查项的扁平化列表（按类别分组）
    pub fn items(&self) -> Vec<CompatItem> {
        let mut items = Vec::new();

        for r in &self.syntax {
            items.push(CompatItem {
                name: r.name.clone(),
                category: format!("Syntax/{:?}", r.category),
                status: r.status,
                detail: r.detail.clone(),
            });
        }

        for r in &self.sqlstate {
            items.push(CompatItem {
                name: r.name.clone(),
                category: "SQLSTATE".to_string(),
                status: r.status,
                detail: r.detail.clone(),
            });
        }

        for r in &self.data_types {
            items.push(CompatItem {
                name: r.name.clone(),
                category: "DataType".to_string(),
                status: r.status,
                detail: r.detail.clone(),
            });
        }

        for r in &self.protocol {
            items.push(CompatItem {
                name: r.name.clone(),
                category: "Protocol".to_string(),
                status: r.status,
                detail: r.detail.clone(),
            });
        }

        items
    }

    /// 总检查项数
    pub fn total_count(&self) -> usize {
        self.syntax.len() + self.sqlstate.len() + self.data_types.len() + self.protocol.len()
    }

    /// 通过项数（含部分通过）
    pub fn passed_count(&self) -> usize {
        self.items()
            .iter()
            .filter(|i| i.status == CompatStatus::Pass || i.status == CompatStatus::Partial)
            .count()
    }

    /// 完全通过项数
    pub fn full_pass_count(&self) -> usize {
        self.items()
            .iter()
            .filter(|i| i.status == CompatStatus::Pass)
            .count()
    }

    /// 生成文本摘要
    pub fn summary(&self) -> String {
        let total = self.total_count();
        let passed = self.passed_count();
        let full = self.full_pass_count();
        let rate = if total == 0 {
            0.0
        } else {
            (passed as f64 / total as f64) * 100.0
        };
        let full_rate = if total == 0 {
            0.0
        } else {
            (full as f64 / total as f64) * 100.0
        };

        format!(
            "SzRSQL PostgreSQL 兼容性报告\n\
             ============================\n\
             总检查项: {total}\n\
             通过(含部分): {passed} ({rate:.1}%)\n\
             完全通过: {full} ({full_rate:.1}%)\n\
             \n\
             分类统计:\n\
             - SQL 语法: {syntax_total} 项 (通过 {syntax_pass})\n\
             - SQLSTATE: {sqlstate_total} 项 (通过 {sqlstate_pass})\n\
             - 数据类型: {dtype_total} 项 (通过 {dtype_pass})\n\
             - 协议合规: {proto_total} 项 (通过 {proto_pass})\n",
            syntax_total = self.syntax.len(),
            syntax_pass = self
                .syntax
                .iter()
                .filter(|r| r.status == CompatStatus::Pass || r.status == CompatStatus::Partial)
                .count(),
            sqlstate_total = self.sqlstate.len(),
            sqlstate_pass = self
                .sqlstate
                .iter()
                .filter(|r| r.status == CompatStatus::Pass || r.status == CompatStatus::Partial)
                .count(),
            dtype_total = self.data_types.len(),
            dtype_pass = self
                .data_types
                .iter()
                .filter(|r| r.status == CompatStatus::Pass || r.status == CompatStatus::Partial)
                .count(),
            proto_total = self.protocol.len(),
            proto_pass = self
                .protocol
                .iter()
                .filter(|r| r.status == CompatStatus::Pass || r.status == CompatStatus::Partial)
                .count(),
        )
    }

    /// 序列化为 JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// 返回 crate 版本号
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_nonempty() {
        assert!(!version().is_empty());
    }

    #[test]
    fn compat_report_runs_all_categories() {
        let report = CompatReport::run_all();
        assert!(!report.syntax.is_empty(), "syntax 检查项不应为空");
        assert!(!report.sqlstate.is_empty(), "sqlstate 检查项不应为空");
        assert!(!report.data_types.is_empty(), "data_types 检查项不应为空");
        assert!(!report.protocol.is_empty(), "protocol 检查项不应为空");
    }

    #[test]
    fn compat_report_summary_contains_stats() {
        let report = CompatReport::run_all();
        let summary = report.summary();
        assert!(summary.contains("总检查项"));
        assert!(summary.contains("SQL 语法"));
        assert!(summary.contains("SQLSTATE"));
        assert!(summary.contains("数据类型"));
        assert!(summary.contains("协议合规"));
    }

    #[test]
    fn compat_report_json_serializable() {
        let report = CompatReport::run_all();
        let json = report.to_json().expect("JSON 序列化应成功");
        assert!(json.contains("\"syntax\""));
        assert!(json.contains("\"sqlstate\""));
        assert!(json.contains("\"data_types\""));
        assert!(json.contains("\"protocol\""));
    }
}
