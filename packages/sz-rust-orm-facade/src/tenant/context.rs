// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户上下文 — 从 HTTP 请求解析后注入 request extensions

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantResolveSource {
    Header,
    Jwt,
    Path,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantContext {
    pub tenant_id: i64,
    pub is_platform_admin: bool,
    pub resolve_source: TenantResolveSource,
}

impl TenantContext {
    pub fn new(
        tenant_id: i64,
        is_platform_admin: bool,
        resolve_source: TenantResolveSource,
    ) -> Self {
        Self {
            tenant_id,
            is_platform_admin,
            resolve_source,
        }
    }

    pub fn tenant_id(&self) -> i64 {
        self.tenant_id
    }

    pub fn is_platform_admin(&self) -> bool {
        self.is_platform_admin
    }

    pub fn resolve_source(&self) -> TenantResolveSource {
        self.resolve_source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_context_new_and_getters() {
        let ctx = TenantContext::new(1, true, TenantResolveSource::Jwt);
        assert_eq!(ctx.tenant_id(), 1);
        assert!(ctx.is_platform_admin());
        assert_eq!(ctx.resolve_source(), TenantResolveSource::Jwt);
    }

    #[test]
    fn test_tenant_context_serialize_roundtrip() {
        let ctx = TenantContext::new(2, false, TenantResolveSource::Header);
        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: TenantContext = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tenant_id, 2);
        assert!(!deserialized.is_platform_admin);
        assert_eq!(deserialized.resolve_source, TenantResolveSource::Header);
    }
}
