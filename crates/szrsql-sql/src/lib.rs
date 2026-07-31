//! SzRSQL SQL 解析器与执行器。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.7 节。
//!
//! Phase 3.1 交付物：
//! - `ast.rs` — SzRSQL 内部 AST 数据结构
//! - `parser.rs` — PG SQL → sqlparser-rs AST → SzRSQL AST 转换器
//!
//! Phase 3.2 交付物：
//! - `plan.rs` — AST → LogicalPlan 转换器（含 Catalog 抽象 + InMemoryCatalog）
//!
//! Phase 3.3 交付物：
//! - `expr.rs` — 表达式求值器（算术/比较/逻辑/函数/CASE/CAST/IN/BETWEEN/LIKE/IS NULL/EXISTS）
//!
//! Phase 3.4 交付物：
//! - `executor.rs` — 火山模型执行器（SeqScan + IndexScan + Filter + Projection + Limit + Distinct）

#![allow(dead_code)]

pub mod ast;
pub mod bitmap_index;
pub mod brin;
pub mod check_constraint;
pub mod collation;
pub mod cursor;
pub mod dialect;
pub mod executor;
pub mod expr;
pub mod extended_index;
pub mod fdw;
pub mod for_update;
pub mod foreign_key;
pub mod fulltext_v2;
pub mod gist;
pub mod grouping_sets;
pub mod json;
pub mod lateral;
pub mod materialized_view;
pub mod online_ddl;
pub mod parser;
pub mod partial_covering_index;
pub mod partition;
pub mod plan;
pub mod plpgsql;
pub mod plpgsql_interp;
pub mod recursive_cte;
pub mod savepoint;
pub mod spatial;
pub mod trigger;
pub mod udf;

#[cfg(test)]
mod array_type_tests;
#[cfg(test)]
mod check_constraint_tests;
#[cfg(test)]
mod cte_tests;
#[cfg(test)]
mod enum_type_tests;
#[cfg(test)]
mod executor_tests;
#[cfg(test)]
mod mvcc_integration_tests;
#[cfg(test)]
mod expr_tests;
#[cfg(test)]
mod extended_index_tests;
#[cfg(test)]
mod flashback_tests;
#[cfg(test)]
mod foreign_key_tests;
#[cfg(test)]
mod fulltext_type_tests;
#[cfg(test)]
mod generated_column_tests;
#[cfg(test)]
mod listen_notify_tests;
#[cfg(test)]
mod materialized_view_aggregate_tests;
#[cfg(test)]
mod materialized_view_concurrent_fuzz_tests;
#[cfg(test)]
mod materialized_view_group_aggregate_tests;
#[cfg(test)]
mod materialized_view_incremental_tests;
#[cfg(test)]
mod materialized_view_query_rewrite_tests;
#[cfg(test)]
mod materialized_view_simple_tests;
#[cfg(test)]
mod materialized_view_tests;
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod parser_tests;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod plpgsql_coverage_tests;
#[cfg(test)]
mod prepare_tests;
#[cfg(test)]
mod replace_tests;
#[cfg(test)]
mod returning_tests;
#[cfg(test)]
mod savepoint_tests;
#[cfg(test)]
mod sequence_tests;
#[cfg(test)]
mod set_op_tests;
#[cfg(test)]
mod show_set_tests;
#[cfg(test)]
mod temp_table_tests;
#[cfg(test)]
mod trigger_tests;
#[cfg(test)]
mod upsert_tests;
#[cfg(test)]
mod window_tests;

/// 返回 crate 版本号，供 workspace 骨架冒烟测试使用。
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
}
