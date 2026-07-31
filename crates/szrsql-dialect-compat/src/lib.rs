//! SzRSQL 多方言兼容性测试套件。
//!
//! 本 crate 提供 SzRSQL 与 MySQL / Oracle / SQL Server / SQLite 四大主流数据库的
//! 兼容性验证能力，并包含跨方言对抗性边界测试。
//!
//! # 模块组织
//!
//! - [`mysql`]：MySQL 兼容性测试（语法/类型/函数/运算符/DDL 选项）
//! - [`oracle`]：Oracle 兼容性测试（ROWNUM/DECODE/NVL/序列/DUAL 等）
//! - [`sqlserver`]：SQL Server 兼容性测试（TOP/ISNULL/GETDATE/T-SQL 类型等）
//! - [`sqlite`]：SQLite 兼容性测试（PRAGMA/AUTOINCREMENT/WITHOUT ROWID 等）
//! - [`adversarial`]：跨方言对抗性边界测试（注入/溢出/深嵌套/方言混淆）
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_dialect_compat::DialectCompatReport;
//!
//! let report = DialectCompatReport::run_all();
//! println!("{}", report.summary());
//! ```
//!
//! # 设计原则
//!
//! - **基于实测**：每项检查都调用 `parse_with_dialect` 实际解析 SQL，不靠人工估算
//! - **分类清晰**：按方言×类别（语法/类型/函数/运算符/DDL）二维组织
//! - **覆盖全面**：每方言至少 50+ 检查项，覆盖核心功能与边界场景
//! - **对抗性**：专门模块测试 SQL 注入、栈溢出、方言混淆等安全场景

#![allow(dead_code)]

pub mod adversarial;
pub mod mysql;
pub mod oracle;
pub mod sqlserver;
pub mod sqlite;

pub use adversarial::{AdversarialTest, AdversarialTestResult, AdversarialCategory};
pub use mysql::{MysqlCompat, MysqlCompatResult, MysqlCategory};
pub use oracle::{OracleCompat, OracleCompatResult, OracleCategory};
pub use sqlserver::{SqlserverCompat, SqlserverCompatResult, SqlserverCategory};
pub use sqlite::{SqliteCompat, SqliteCompatResult, SqliteCategory};

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

    /// 是否算作通过（Pass 或 Partial）
    pub fn is_passed(self) -> bool {
        matches!(self, Self::Pass | Self::Partial)
    }
}

/// 单项兼容性检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatItem {
    /// 检查项名称
    pub name: String,
    /// 检查项分类
    pub category: String,
    /// 检查结果状态
    pub status: CompatStatus,
    /// 详细说明（失败原因或通过备注）
    pub detail: String,
}

/// 方言兼容性报告汇总
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialectCompatReport {
    /// MySQL 兼容性检查项
    pub mysql: Vec<MysqlCompatResult>,
    /// Oracle 兼容性检查项
    pub oracle: Vec<OracleCompatResult>,
    /// SQL Server 兼容性检查项
    pub sqlserver: Vec<SqlserverCompatResult>,
    /// SQLite 兼容性检查项
    pub sqlite: Vec<SqliteCompatResult>,
    /// 对抗性边界测试项
    pub adversarial: Vec<AdversarialTestResult>,
}

impl DialectCompatReport {
    /// 运行全部方言兼容性 + 对抗性测试并生成报告
    pub fn run_all() -> Self {
        Self {
            mysql: MysqlCompat::run_all(),
            oracle: OracleCompat::run_all(),
            sqlserver: SqlserverCompat::run_all(),
            sqlite: SqliteCompat::run_all(),
            adversarial: AdversarialTest::run_all(),
        }
    }

    /// 返回所有检查项的扁平化列表
    pub fn items(&self) -> Vec<CompatItem> {
        let mut items = Vec::new();

        for r in &self.mysql {
            items.push(CompatItem {
                name: r.name.clone(),
                category: format!("MySQL/{:?}", r.category),
                status: r.status,
                detail: r.detail.clone(),
            });
        }
        for r in &self.oracle {
            items.push(CompatItem {
                name: r.name.clone(),
                category: format!("Oracle/{:?}", r.category),
                status: r.status,
                detail: r.detail.clone(),
            });
        }
        for r in &self.sqlserver {
            items.push(CompatItem {
                name: r.name.clone(),
                category: format!("SqlServer/{:?}", r.category),
                status: r.status,
                detail: r.detail.clone(),
            });
        }
        for r in &self.sqlite {
            items.push(CompatItem {
                name: r.name.clone(),
                category: format!("SQLite/{:?}", r.category),
                status: r.status,
                detail: r.detail.clone(),
            });
        }
        for r in &self.adversarial {
            items.push(CompatItem {
                name: r.name.clone(),
                category: format!("Adversarial/{:?}", r.category),
                status: r.status,
                detail: r.detail.clone(),
            });
        }

        items
    }

    /// 总检查项数
    pub fn total_count(&self) -> usize {
        self.mysql.len()
            + self.oracle.len()
            + self.sqlserver.len()
            + self.sqlite.len()
            + self.adversarial.len()
    }

    /// 通过项数（含部分通过）
    pub fn passed_count(&self) -> usize {
        self.items().iter().filter(|i| i.status.is_passed()).count()
    }

    /// 完全通过项数
    pub fn full_pass_count(&self) -> usize {
        self.items()
            .iter()
            .filter(|i| i.status == CompatStatus::Pass)
            .count()
    }

    /// 各方言通过率（含部分通过）
    pub fn dialect_rates(&self) -> Vec<(&'static str, usize, usize, f64)> {
        fn calc<T: DialectResult>(v: &[T]) -> (usize, usize, f64) {
            let total = v.len();
            let pass = v.iter().filter(|r| r.status().is_passed()).count();
            let rate = if total == 0 {
                0.0
            } else {
                (pass as f64 / total as f64) * 100.0
            };
            (pass, total, rate)
        }
        let m = calc(&self.mysql);
        let o = calc(&self.oracle);
        let s = calc(&self.sqlserver);
        let q = calc(&self.sqlite);
        let a = calc(&self.adversarial);
        vec![
            ("MySQL", m.0, m.1, m.2),
            ("Oracle", o.0, o.1, o.2),
            ("SQL Server", s.0, s.1, s.2),
            ("SQLite", q.0, q.1, q.2),
            ("Adversarial", a.0, a.1, a.2),
        ]
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

        let mut s = format!(
            "SzRSQL 多方言兼容性报告\n\
             =======================\n\
             总检查项: {total}\n\
             通过(含部分): {passed} ({rate:.1}%)\n\
             完全通过: {full} ({full_rate:.1}%)\n\
             \n\
             各方言通过率:\n"
        );
        for (name, pass, total, rate) in self.dialect_rates() {
            s.push_str(&format!("  - {name:<12}: {pass}/{total} ({rate:.1}%)\n"));
        }
        s
    }

    /// 序列化为 JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// 方言测试结果通用接口（用于 `dialect_rates` 计算）
pub trait DialectResult {
    /// 返回检查结果状态
    fn status(&self) -> CompatStatus;
}

impl DialectResult for MysqlCompatResult {
    fn status(&self) -> CompatStatus {
        self.status
    }
}

impl DialectResult for OracleCompatResult {
    fn status(&self) -> CompatStatus {
        self.status
    }
}

impl DialectResult for SqlserverCompatResult {
    fn status(&self) -> CompatStatus {
        self.status
    }
}

impl DialectResult for SqliteCompatResult {
    fn status(&self) -> CompatStatus {
        self.status
    }
}

impl DialectResult for AdversarialTestResult {
    fn status(&self) -> CompatStatus {
        self.status
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
    fn dialect_report_runs_all_categories() {
        let report = DialectCompatReport::run_all();
        assert!(!report.mysql.is_empty(), "mysql 检查项不应为空");
        assert!(!report.oracle.is_empty(), "oracle 检查项不应为空");
        assert!(!report.sqlserver.is_empty(), "sqlserver 检查项不应为空");
        assert!(!report.sqlite.is_empty(), "sqlite 检查项不应为空");
        assert!(!report.adversarial.is_empty(), "adversarial 检查项不应为空");
    }

    #[test]
    fn dialect_report_summary_contains_stats() {
        let report = DialectCompatReport::run_all();
        let summary = report.summary();
        assert!(summary.contains("总检查项"));
        assert!(summary.contains("MySQL"));
        assert!(summary.contains("Oracle"));
        assert!(summary.contains("SQL Server"));
        assert!(summary.contains("SQLite"));
        assert!(summary.contains("Adversarial"));
    }

    #[test]
    fn dialect_report_json_serializable() {
        let report = DialectCompatReport::run_all();
        let json = report.to_json().expect("JSON 序列化应成功");
        assert!(json.contains("\"mysql\""));
        assert!(json.contains("\"oracle\""));
        assert!(json.contains("\"sqlserver\""));
        assert!(json.contains("\"sqlite\""));
        assert!(json.contains("\"adversarial\""));
    }
}
