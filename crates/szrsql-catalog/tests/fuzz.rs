//! Phase 3.15 集成测试入口 — 权限系统 Fuzz。
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.15。
//! 通过 `#[path]` 引用 `tests/fuzz/auth_fuzz.rs` 子模块。

#[path = "fuzz/auth_fuzz.rs"]
mod auth_fuzz;
