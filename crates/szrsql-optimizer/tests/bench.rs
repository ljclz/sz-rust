//! Phase 5.11 集成测试入口 — TPC-H 前 10 条查询基准。
//!
//! 对应 `SzRSQL实施进度.md` Phase 5.11。
//! 通过 `#[path]` 引用 `tests/bench/tpch.rs` 子模块。

#[path = "bench/tpch.rs"]
mod tpch;
