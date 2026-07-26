//! Phase 3.11 / 3.14 集成测试入口 — 多租户隔离 + RBAC/RLS 组合验证。
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.11 / Phase 3.14。

#[path = "integration/multitenant.rs"]
mod multitenant;

#[path = "integration/rbac_rls.rs"]
mod rbac_rls;
