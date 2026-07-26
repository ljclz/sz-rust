//! Phase 3.16 集成测试入口 — SQL 完整链路集成测试。
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.16。
//! 通过 `#[path]` 引用 `tests/integration/sql_full.rs` 子模块。

#[path = "integration/sql_full.rs"]
mod sql_full;
