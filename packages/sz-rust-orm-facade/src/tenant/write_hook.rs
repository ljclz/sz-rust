// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户写入钩子 — 从 HookContext.metadata 提取 tenant_id 并填充到 HookContext.tenant_id
//!
//! 复用 `HookContext.tenant_id` 字段（spec 9.4，AC-02），禁止定义并行填充机制。

use crate::hooks::{HookContext, HookResult};

/// 租户写入钩子 — 在 before_insert 时从 metadata 提取 tenant_id 填充到 HookContext
///
/// 中间件将 tenant_id 注入到 `HookContext.metadata["tenant_id"]`，
/// `TenantWriteHook::before_insert` 从 metadata 提取并设置 `ctx.tenant_id`。
pub struct TenantWriteHook;

impl TenantWriteHook {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TenantWriteHook {
    fn default() -> Self {
        Self::new()
    }
}

/// TenantWriteHook 不实现 Hookable trait（它不是实体），
/// 而是提供一个静态方法供业务实体的 `before_insert` 调用。
impl TenantWriteHook {
    /// 从 HookContext.metadata 提取 tenant_id 并填充到 ctx.tenant_id
    ///
    /// 中间件通过 `ctx.set_meta("tenant_id", tenant_id.to_string())` 注入，
    /// 此方法提取并设置 `ctx.tenant_id = Some(tenant_id)`。
    pub fn before_insert(ctx: &mut HookContext) -> HookResult<()> {
        if ctx.tenant_id.is_some() {
            return Ok(());
        }
        if let Some(raw) = ctx.get_meta("tenant_id") {
            if let Ok(tenant_id) = raw.parse::<i64>() {
                ctx.tenant_id = Some(tenant_id);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_before_insert_fills_tenant_id() {
        let mut ctx = HookContext::new();
        ctx.set_meta("tenant_id", "42");

        TenantWriteHook::before_insert(&mut ctx).unwrap();

        assert_eq!(ctx.tenant_id, Some(42));
    }

    #[test]
    fn test_no_tenant_id_in_meta_noop() {
        let mut ctx = HookContext::new();

        TenantWriteHook::before_insert(&mut ctx).unwrap();

        assert_eq!(ctx.tenant_id, None);
    }

    #[test]
    fn test_already_set_tenant_id_not_overwritten() {
        let mut ctx = HookContext::new();
        ctx.tenant_id = Some(99);
        ctx.set_meta("tenant_id", "42");

        TenantWriteHook::before_insert(&mut ctx).unwrap();

        assert_eq!(ctx.tenant_id, Some(99));
    }

    #[test]
    fn test_invalid_tenant_id_in_meta_noop() {
        let mut ctx = HookContext::new();
        ctx.set_meta("tenant_id", "abc");

        TenantWriteHook::before_insert(&mut ctx).unwrap();

        assert_eq!(ctx.tenant_id, None);
    }
}
