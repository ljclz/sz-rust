//! Phase 5.12 集成测试入口 — 优化器回归 Fuzz。
//!
//! 对应 `SzRSQL实施进度.md` Phase 5.12。
//! 通过 `#[path]` 引用 `tests/fuzz/optimizer_fuzz.rs` 子模块。

#[path = "fuzz/optimizer_fuzz.rs"]
mod optimizer_fuzz;
