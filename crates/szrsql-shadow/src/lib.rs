//! SzRSQL 影子流量回放 — 上线前最后一道防线
//!
//! 对应 `docs/SHADOW_REPORT.md` P0-1 任务。
//!
//! # 设计目标
//!
//! 1. **流量录制**：从 SQL 文件 / pg_stat_statements / 慢查询日志中提取 SQL 序列，
//!    序列化为 JSONL 格式存储，便于回放与共享。
//! 2. **影子回放**：读取 JSONL 流量文件，同时在 PG 18 和 szrsql 上执行，
//!    返回 PG 18 结果给客户端（不影响业务），同时记录 szrsql 的结果用于比对。
//! 3. **结果比对**：逐条 SQL 比对行数、列数、每行每列的值。
//! 4. **延迟统计**：记录 PG 18 和 szrsql 各自的执行延迟，计算 P50/P95/P99。
//! 5. **报告生成**：汇总统计 + 差异详情，输出 JSON 报告。
//!
//! # 用法
//!
//! ```bash
//! # 1. 录制：从 SQL 文件生成 JSONL 流量
//! szrsql-shadow record --input queries.sql --output traffic.jsonl
//!
//! # 2. 回放：在 PG 18 + szrsql 上执行，比对结果
//! szrsql-shadow replay --input traffic.jsonl \
//!   --pg-url "postgresql://postgres:postgres@127.0.0.1:5432/postgres" \
//!   --report report.json
//! ```
//!
//! # 注意
//!
//! - 当前实现不包含完整 pgwire TCP 代理（避免 szrsql 协议层不完整导致代理失效）
//! - 影子回放基于 SQL 文件 / JSONL 文件，覆盖真实流量下的语义一致性验证
//! - 比对逻辑与 `crates/szrsql-sql/tests/sql_compare.rs` 一致，但增加了延迟统计与报告生成

#![allow(dead_code)]

pub mod compare;
pub mod recorder;
pub mod replay;
pub mod report;

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
