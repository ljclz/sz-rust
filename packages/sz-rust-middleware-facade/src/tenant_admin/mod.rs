// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户管理 API — 13 个 REST 端点

pub mod handlers;
pub mod router;

pub use router::{build_tenant_admin_router, TenantAdminApiState};
