// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 多租户中间件 — 解析、状态校验、鉴权

pub mod platform_admin_guard;
pub mod resolver;
pub mod status_guard;
pub mod tenant_config_guard;

/// JWT 解码后的 tenant_id（由 auth 中间件注入到 request extensions）
#[derive(Debug, Clone, Copy)]
pub struct JwtTenantId(pub i64);
