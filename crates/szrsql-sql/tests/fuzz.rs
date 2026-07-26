//! Phase 3.17 集成测试入口 — SQL 正确性 Fuzz。
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.17。
//! 通过 `#[path]` 引用 `tests/fuzz/sql_fuzz.rs` 子模块。

#[path = "fuzz/sql_fuzz.rs"]
mod sql_fuzz;
