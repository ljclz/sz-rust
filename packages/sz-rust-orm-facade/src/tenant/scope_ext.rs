// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户隔离扩展 trait — 复用 DataScopeExt::with_data_scope_conditions 机制
//!
//! TenantScopeExt 以 DataScopeExt 为 supertrait，通过 `with_data_scope_conditions`
//! 追加 `WHERE tenant_id = ?` 条件，实现行级租户隔离。

use async_trait::async_trait;

use super::context::TenantContext;
use super::error::TenantError;
use super::scoped_table::TenantScopedTableRegistry;
use crate::data_scope::ext::DataScopeExt;
use crate::data_scope::metrics::DataScopeMetrics;
use crate::repository::{WhereCondition, WhereOp};
use crate::Value;

/// 租户隔离扩展 trait
///
/// 以 `DataScopeExt` 为 supertrait，复用 `with_data_scope_conditions` 机制。
/// 平台管理员 bypass；全局表（未注册或 enabled=false）不附加条件；
/// 租户隔离表自动追加 `WHERE tenant_id = ?`。
#[async_trait]
pub trait TenantScopeExt: DataScopeExt {
    /// 异步注入租户隔离条件
    ///
    /// - `ctx.is_platform_admin()` → bypass，记录 `tenant_isolation_bypass` 指标（AC-09）
    /// - `ctx.tenant_id() <= 0` → 返回 `InvalidTenantId`（AC-30）
    /// - `registry.get_tenant_field(table_name)` 返回 None → 不附加条件（AC-11）
    /// - 否则构造 `WHERE tenant_field = tenant_id` 并调用 `with_data_scope_conditions`（AC-01）
    async fn tenant_scope_async(
        self,
        ctx: &TenantContext,
        registry: &TenantScopedTableRegistry,
        table_name: &str,
        metrics: &DataScopeMetrics,
    ) -> Result<Self, TenantError>
    where
        Self: Sized,
    {
        if ctx.is_platform_admin() {
            metrics.record_tenant_isolation_bypass();
            return Ok(self);
        }

        if ctx.tenant_id() <= 0 {
            return Err(TenantError::InvalidTenantId(ctx.tenant_id()));
        }

        let tenant_field = match registry.get_tenant_field(table_name) {
            Some(field) => field,
            None => return Ok(self),
        };

        let condition = WhereCondition::new(tenant_field, WhereOp::Eq, Value::I64(ctx.tenant_id()));
        metrics.record_tenant_request();
        Ok(self.with_data_scope_conditions(&[condition]))
    }
}

/// 为所有实现 DataScopeExt 的类型自动实现 TenantScopeExt
impl<T: DataScopeExt> TenantScopeExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_scope::ext::DataScopeExt;
    use crate::data_scope::metrics::DataScopeMetrics;
    use crate::repository::{WhereCondition, WhereOp};
    use crate::tenant::context::TenantResolveSource;
    use crate::Value;
    use async_trait::async_trait;

    #[derive(Debug)]
    struct MockQueryBuilder {
        conditions: Vec<WhereCondition>,
    }

    #[async_trait]
    impl DataScopeExt for MockQueryBuilder {
        fn with_data_scope_conditions(mut self, conditions: &[WhereCondition]) -> Self {
            self.conditions = conditions.to_vec();
            self
        }
    }

    fn make_registry_with_table() -> TenantScopedTableRegistry {
        let registry = TenantScopedTableRegistry::new();
        registry.register(crate::tenant::scoped_table::TenantScopedTable::new(
            "orders",
        ));
        registry
    }

    #[tokio::test]
    async fn test_platform_admin_bypass() {
        let ctx = TenantContext::new(1, true, TenantResolveSource::Jwt);
        let registry = make_registry_with_table();
        let metrics = DataScopeMetrics::new();
        let builder = MockQueryBuilder { conditions: vec![] };

        let result = builder
            .tenant_scope_async(&ctx, &registry, "orders", &metrics)
            .await
            .unwrap();

        assert!(result.conditions.is_empty());
        assert_eq!(metrics.tenant_isolation_bypass_total(), 1);
    }

    #[tokio::test]
    async fn test_invalid_tenant_id_rejected() {
        let ctx = TenantContext::new(0, false, TenantResolveSource::Header);
        let registry = make_registry_with_table();
        let metrics = DataScopeMetrics::new();
        let builder = MockQueryBuilder { conditions: vec![] };

        let err = builder
            .tenant_scope_async(&ctx, &registry, "orders", &metrics)
            .await
            .unwrap_err();

        assert_eq!(err.error_code(), "INVALID_TENANT_ID");
    }

    #[tokio::test]
    async fn test_global_table_no_condition() {
        let ctx = TenantContext::new(1, false, TenantResolveSource::Header);
        let registry = TenantScopedTableRegistry::new();
        let metrics = DataScopeMetrics::new();
        let builder = MockQueryBuilder { conditions: vec![] };

        let result = builder
            .tenant_scope_async(&ctx, &registry, "nonexistent_table", &metrics)
            .await
            .unwrap();

        assert!(result.conditions.is_empty());
    }

    #[tokio::test]
    async fn test_tenant_scoped_adds_condition() {
        let ctx = TenantContext::new(42, false, TenantResolveSource::Header);
        let registry = make_registry_with_table();
        let metrics = DataScopeMetrics::new();
        let builder = MockQueryBuilder { conditions: vec![] };

        let result = builder
            .tenant_scope_async(&ctx, &registry, "orders", &metrics)
            .await
            .unwrap();

        assert_eq!(result.conditions.len(), 1);
        assert_eq!(result.conditions[0].field, "tenant_id");
        assert_eq!(result.conditions[0].op, WhereOp::Eq);
        assert_eq!(result.conditions[0].value, Value::I64(42));
        assert_eq!(metrics.tenant_request_total(), 1);
    }

    #[tokio::test]
    async fn test_disabled_table_no_condition() {
        let ctx = TenantContext::new(1, false, TenantResolveSource::Header);
        let registry = TenantScopedTableRegistry::new();
        registry.register(
            crate::tenant::scoped_table::TenantScopedTable::new("orders").with_enabled(false),
        );
        let metrics = DataScopeMetrics::new();
        let builder = MockQueryBuilder { conditions: vec![] };

        let result = builder
            .tenant_scope_async(&ctx, &registry, "orders", &metrics)
            .await
            .unwrap();

        assert!(result.conditions.is_empty());
    }
}
