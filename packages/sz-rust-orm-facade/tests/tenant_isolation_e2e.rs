// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T12.1 租户隔离 E2E 集成测试 — AC-01~AC-11/27/30
//!
//! 覆盖验收标准：
//! - AC-01: 查询自动追加 WHERE tenant_id = ?
//! - AC-02: 写入自动填充 tenant_id
//! - AC-03: UPDATE 追加 tenant_id 条件
//! - AC-04: DELETE 追加 tenant_id 条件
//! - AC-09: 平台管理员绕过
//! - AC-10: 租户隔离 + 部门权限取交集
//! - AC-11: 全局表不附加条件
//! - AC-27: JOIN 双表均包含 tenant_id
//! - AC-30: tenant_id ≤ 0 拒绝
//! - 隔离泄漏检测：租户1查询绝不返回租户2数据（含并发）

use async_trait::async_trait;
use std::sync::Arc;
use sz_rust_orm_facade::data_scope::ext::DataScopeExt;
use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::hooks::HookContext;
use sz_rust_orm_facade::repository::{WhereCondition, WhereOp};
use sz_rust_orm_facade::tenant::context::{TenantContext, TenantResolveSource};
use sz_rust_orm_facade::tenant::error::TenantError;
use sz_rust_orm_facade::tenant::scope_ext::TenantScopeExt;
use sz_rust_orm_facade::tenant::scoped_table::{TenantScopedTable, TenantScopedTableRegistry};
use sz_rust_orm_facade::tenant::write_hook::TenantWriteHook;
use sz_rust_orm_facade::Value;

// ============================================================================
// MockQueryBuilder — 模拟查询构建器，记录注入的 WHERE 条件
// ============================================================================

#[derive(Debug, Clone)]
struct MockQueryBuilder {
    _table: String,
    conditions: Vec<WhereCondition>,
    is_update: bool,
    is_delete: bool,
}

impl MockQueryBuilder {
    fn select(table: &str) -> Self {
        Self {
            _table: table.to_string(),
            conditions: vec![],
            is_update: false,
            is_delete: false,
        }
    }

    fn update(table: &str) -> Self {
        Self {
            _table: table.to_string(),
            conditions: vec![],
            is_update: true,
            is_delete: false,
        }
    }

    fn delete(table: &str) -> Self {
        Self {
            _table: table.to_string(),
            conditions: vec![],
            is_update: false,
            is_delete: true,
        }
    }

    fn find_condition(&self, field: &str) -> Option<&WhereCondition> {
        self.conditions.iter().find(|c| c.field == field)
    }
}

#[async_trait]
impl DataScopeExt for MockQueryBuilder {
    fn with_data_scope_conditions(mut self, conditions: &[WhereCondition]) -> Self {
        self.conditions.extend(conditions.iter().cloned());
        self
    }
}

// ============================================================================
// MockJoinQueryBuilder — 模拟 JOIN 查询，主表+关联表各持条件列表
// ============================================================================

#[derive(Debug, Clone)]
struct MockJoinQueryBuilder {
    _main_table: String,
    join_table: String,
    main_conditions: Vec<WhereCondition>,
    join_conditions: Vec<WhereCondition>,
}

impl MockJoinQueryBuilder {
    fn new(main: &str, join: &str) -> Self {
        Self {
            _main_table: main.to_string(),
            join_table: join.to_string(),
            main_conditions: vec![],
            join_conditions: vec![],
        }
    }
}

#[async_trait]
impl DataScopeExt for MockJoinQueryBuilder {
    fn with_data_scope_conditions(mut self, conditions: &[WhereCondition]) -> Self {
        self.main_conditions.extend(conditions.iter().cloned());
        self
    }
}

impl MockJoinQueryBuilder {
    fn apply_join_tenant_scope(
        mut self,
        ctx: &TenantContext,
        registry: &TenantScopedTableRegistry,
        metrics: &DataScopeMetrics,
    ) -> Result<Self, TenantError> {
        if ctx.is_platform_admin() {
            metrics.record_tenant_isolation_bypass();
            return Ok(self);
        }
        if ctx.tenant_id() <= 0 {
            return Err(TenantError::InvalidTenantId(ctx.tenant_id()));
        }
        if let Some(field) = registry.get_tenant_field(&self.join_table) {
            let cond = WhereCondition::new(
                format!("{}.{}", self.join_table, field),
                WhereOp::Eq,
                Value::I64(ctx.tenant_id()),
            );
            self.join_conditions.push(cond);
        }
        Ok(self)
    }
}

// ============================================================================
// 辅助构造函数
// ============================================================================

fn make_registry(tables: &[&str]) -> TenantScopedTableRegistry {
    let registry = TenantScopedTableRegistry::new();
    for &t in tables {
        registry.register(TenantScopedTable::new(t));
    }
    registry
}

fn make_metrics() -> DataScopeMetrics {
    DataScopeMetrics::new()
}

fn make_tenant_ctx(tenant_id: i64) -> TenantContext {
    TenantContext::new(tenant_id, false, TenantResolveSource::Header)
}

fn make_admin_ctx(tenant_id: i64) -> TenantContext {
    TenantContext::new(tenant_id, true, TenantResolveSource::Jwt)
}

// ============================================================================
// AC-01: 查询自动追加 WHERE tenant_id = ?
// ============================================================================

#[tokio::test]
async fn it_ac01_query_auto_appends_tenant_id() {
    let registry = make_registry(&["orders"]);
    let metrics = make_metrics();
    let ctx = make_tenant_ctx(1);

    let builder = MockQueryBuilder::select("orders");
    let result = builder
        .tenant_scope_async(&ctx, &registry, "orders", &metrics)
        .await
        .unwrap();

    let cond = result.find_condition("tenant_id").unwrap();
    assert_eq!(cond.op, WhereOp::Eq);
    assert_eq!(cond.value, Value::I64(1));
    assert_eq!(metrics.tenant_request_total(), 1);
}

// ============================================================================
// AC-02: 写入自动填充 tenant_id（TenantWriteHook）
// ============================================================================

#[tokio::test]
async fn it_ac02_write_auto_fills_tenant_id() {
    let mut ctx = HookContext::new();
    ctx.set_meta("tenant_id", "42");

    TenantWriteHook::before_insert(&mut ctx).unwrap();

    assert_eq!(ctx.tenant_id, Some(42));
}

#[tokio::test]
async fn it_ac02_write_no_tenant_id_in_meta_noop() {
    let mut ctx = HookContext::new();
    TenantWriteHook::before_insert(&mut ctx).unwrap();
    assert_eq!(ctx.tenant_id, None);
}

#[tokio::test]
async fn it_ac02_write_existing_tenant_id_not_overwritten() {
    let mut ctx = HookContext::new();
    ctx.tenant_id = Some(99);
    ctx.set_meta("tenant_id", "42");

    TenantWriteHook::before_insert(&mut ctx).unwrap();

    assert_eq!(ctx.tenant_id, Some(99));
}

// ============================================================================
// AC-03: UPDATE 追加 tenant_id 条件
// ============================================================================

#[tokio::test]
async fn it_ac03_update_appends_tenant_id() {
    let registry = make_registry(&["orders"]);
    let metrics = make_metrics();
    let ctx = make_tenant_ctx(3);

    let builder = MockQueryBuilder::update("orders");
    let result = builder
        .tenant_scope_async(&ctx, &registry, "orders", &metrics)
        .await
        .unwrap();

    assert!(result.is_update);
    let cond = result.find_condition("tenant_id").unwrap();
    assert_eq!(cond.value, Value::I64(3));
}

// ============================================================================
// AC-04: DELETE 追加 tenant_id 条件
// ============================================================================

#[tokio::test]
async fn it_ac04_delete_appends_tenant_id() {
    let registry = make_registry(&["orders"]);
    let metrics = make_metrics();
    let ctx = make_tenant_ctx(4);

    let builder = MockQueryBuilder::delete("orders");
    let result = builder
        .tenant_scope_async(&ctx, &registry, "orders", &metrics)
        .await
        .unwrap();

    assert!(result.is_delete);
    let cond = result.find_condition("tenant_id").unwrap();
    assert_eq!(cond.value, Value::I64(4));
}

// ============================================================================
// AC-09: 平台管理员绕过租户隔离
// ============================================================================

#[tokio::test]
async fn it_ac09_platform_admin_bypass() {
    let registry = make_registry(&["orders"]);
    let metrics = make_metrics();
    let ctx = make_admin_ctx(1);

    let builder = MockQueryBuilder::select("orders");
    let result = builder
        .tenant_scope_async(&ctx, &registry, "orders", &metrics)
        .await
        .unwrap();

    assert!(result.conditions.is_empty());
    assert_eq!(metrics.tenant_isolation_bypass_total(), 1);
    assert_eq!(metrics.tenant_request_total(), 0);
}

// ============================================================================
// AC-10: 租户隔离 + 部门权限取交集 WHERE tenant_id = ? AND dept_id = ?
// ============================================================================

#[tokio::test]
async fn it_ac10_tenant_and_dept_intersection() {
    let registry = make_registry(&["orders"]);
    let metrics = make_metrics();
    let ctx = make_tenant_ctx(1);

    let dept_condition = WhereCondition::new("dept_id", WhereOp::Eq, Value::I64(5));

    let builder = MockQueryBuilder::select("orders").with_data_scope_conditions(&[dept_condition]);

    let result = builder
        .tenant_scope_async(&ctx, &registry, "orders", &metrics)
        .await
        .unwrap();

    assert_eq!(result.conditions.len(), 2);
    assert!(result.find_condition("dept_id").is_some());
    assert!(result.find_condition("tenant_id").is_some());
}

// ============================================================================
// AC-11: 全局表（未注册）不附加 tenant_id 条件
// ============================================================================

#[tokio::test]
async fn it_ac11_global_table_no_condition() {
    let registry = make_registry(&["orders"]);
    let metrics = make_metrics();
    let ctx = make_tenant_ctx(1);

    let builder = MockQueryBuilder::select("global_config");
    let result = builder
        .tenant_scope_async(&ctx, &registry, "global_config", &metrics)
        .await
        .unwrap();

    assert!(result.conditions.is_empty());
}

#[tokio::test]
async fn it_ac11_disabled_table_no_condition() {
    let registry = TenantScopedTableRegistry::new();
    registry.register(TenantScopedTable::new("orders").with_enabled(false));
    let metrics = make_metrics();
    let ctx = make_tenant_ctx(1);

    let builder = MockQueryBuilder::select("orders");
    let result = builder
        .tenant_scope_async(&ctx, &registry, "orders", &metrics)
        .await
        .unwrap();

    assert!(result.conditions.is_empty());
}

// ============================================================================
// AC-27: JOIN 双表均包含 tenant_id
// ============================================================================

#[tokio::test]
async fn it_ac27_join_both_tables_tenant_id() {
    let registry = make_registry(&["orders", "order_items"]);
    let metrics = make_metrics();
    let ctx = make_tenant_ctx(7);

    let builder = MockJoinQueryBuilder::new("orders", "order_items");
    let result = builder
        .tenant_scope_async(&ctx, &registry, "orders", &metrics)
        .await
        .unwrap();

    let main_cond = result
        .main_conditions
        .iter()
        .find(|c| c.field == "tenant_id")
        .unwrap();
    assert_eq!(main_cond.value, Value::I64(7));

    let result = result
        .apply_join_tenant_scope(&ctx, &registry, &metrics)
        .unwrap();

    let join_cond = result
        .join_conditions
        .iter()
        .find(|c| c.field == "order_items.tenant_id")
        .unwrap();
    assert_eq!(join_cond.value, Value::I64(7));
}

// ============================================================================
// AC-30: tenant_id ≤ 0 拒绝
// ============================================================================

#[tokio::test]
async fn it_ac30_zero_tenant_id_rejected() {
    let registry = make_registry(&["orders"]);
    let metrics = make_metrics();
    let ctx = TenantContext::new(0, false, TenantResolveSource::Header);

    let builder = MockQueryBuilder::select("orders");
    let err = builder
        .tenant_scope_async(&ctx, &registry, "orders", &metrics)
        .await
        .unwrap_err();

    assert_eq!(err.error_code(), "INVALID_TENANT_ID");
}

#[tokio::test]
async fn it_ac30_negative_tenant_id_rejected() {
    let registry = make_registry(&["orders"]);
    let metrics = make_metrics();
    let ctx = TenantContext::new(-1, false, TenantResolveSource::Header);

    let builder = MockQueryBuilder::select("orders");
    let err = builder
        .tenant_scope_async(&ctx, &registry, "orders", &metrics)
        .await
        .unwrap_err();

    assert_eq!(err.error_code(), "INVALID_TENANT_ID");
}

// ============================================================================
// 隔离泄漏检测：租户1查询绝不返回租户2数据（含并发）
// ============================================================================

#[tokio::test]
async fn it_tenant_isolation_no_leak() {
    let registry = make_registry(&["orders"]);
    let metrics = make_metrics();

    let ctx1 = make_tenant_ctx(1);
    let ctx2 = make_tenant_ctx(2);

    let builder1 = MockQueryBuilder::select("orders");
    let result1 = builder1
        .tenant_scope_async(&ctx1, &registry, "orders", &metrics)
        .await
        .unwrap();

    let builder2 = MockQueryBuilder::select("orders");
    let result2 = builder2
        .tenant_scope_async(&ctx2, &registry, "orders", &metrics)
        .await
        .unwrap();

    let cond1 = result1.find_condition("tenant_id").unwrap();
    let cond2 = result2.find_condition("tenant_id").unwrap();

    assert_eq!(cond1.value, Value::I64(1));
    assert_eq!(cond2.value, Value::I64(2));
    assert_ne!(cond1.value, cond2.value);
}

#[tokio::test]
async fn it_tenant_isolation_no_leak_concurrent() {
    let registry = Arc::new(make_registry(&["orders"]));
    let metrics = Arc::new(make_metrics());

    let mut handles = Vec::new();
    for tenant_id in 1..=10 {
        let registry = registry.clone();
        let metrics = metrics.clone();
        handles.push(tokio::spawn(async move {
            let ctx = TenantContext::new(tenant_id, false, TenantResolveSource::Header);
            let builder = MockQueryBuilder::select("orders");
            let result = builder
                .tenant_scope_async(&ctx, &registry, "orders", &metrics)
                .await
                .unwrap();
            let cond = result.find_condition("tenant_id").unwrap();
            assert_eq!(cond.value, Value::I64(tenant_id));
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(metrics.tenant_request_total(), 10);
}

// ============================================================================
// 自定义 tenant_field 验证
// ============================================================================

#[tokio::test]
async fn it_custom_tenant_field() {
    let registry = TenantScopedTableRegistry::new();
    registry.register(TenantScopedTable::new("orders").with_tenant_field("org_id"));
    let metrics = make_metrics();
    let ctx = make_tenant_ctx(5);

    let builder = MockQueryBuilder::select("orders");
    let result = builder
        .tenant_scope_async(&ctx, &registry, "orders", &metrics)
        .await
        .unwrap();

    let cond = result.find_condition("org_id").unwrap();
    assert_eq!(cond.value, Value::I64(5));
    assert!(result.find_condition("tenant_id").is_none());
}

// ============================================================================
// 多租户场景：不同租户操作不同表
// ============================================================================

#[tokio::test]
async fn it_multi_tenant_different_tables() {
    let registry = make_registry(&["orders", "invoices"]);
    let metrics = make_metrics();

    let ctx1 = make_tenant_ctx(1);
    let ctx2 = make_tenant_ctx(2);

    let builder1 = MockQueryBuilder::select("orders");
    let result1 = builder1
        .tenant_scope_async(&ctx1, &registry, "orders", &metrics)
        .await
        .unwrap();

    let builder2 = MockQueryBuilder::select("invoices");
    let result2 = builder2
        .tenant_scope_async(&ctx2, &registry, "invoices", &metrics)
        .await
        .unwrap();

    let cond1 = result1.find_condition("tenant_id").unwrap();
    let cond2 = result2.find_condition("tenant_id").unwrap();

    assert_eq!(cond1.value, Value::I64(1));
    assert_eq!(cond2.value, Value::I64(2));
    assert_eq!(metrics.tenant_request_total(), 2);
}

// ============================================================================
// 平台管理员跨租户查询验证
// ============================================================================

#[tokio::test]
async fn it_platform_admin_cross_tenant_query() {
    let registry = make_registry(&["orders"]);
    let metrics = make_metrics();

    let admin_ctx = make_admin_ctx(0);

    let builder = MockQueryBuilder::select("orders");
    let result = builder
        .tenant_scope_async(&admin_ctx, &registry, "orders", &metrics)
        .await
        .unwrap();

    assert!(result.conditions.is_empty());
    assert_eq!(metrics.tenant_isolation_bypass_total(), 1);
}

// ============================================================================
// TenantContext 序列化/反序列化 roundtrip（跨服务传递）
// ============================================================================

#[tokio::test]
async fn it_tenant_context_serde_roundtrip() {
    let ctx = TenantContext::new(42, true, TenantResolveSource::Jwt);
    let json = serde_json::to_string(&ctx).unwrap();
    let deserialized: TenantContext = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.tenant_id(), 42);
    assert!(deserialized.is_platform_admin());
    assert_eq!(deserialized.resolve_source(), TenantResolveSource::Jwt);
}

// ============================================================================
// TenantError → DataScopeError 转换验证
// ============================================================================

#[tokio::test]
async fn it_tenant_error_to_data_scope_error_conversion() {
    use sz_rust_orm_facade::data_scope::error::DataScopeError;

    let err: DataScopeError = TenantError::TenantIdRequired.into();
    assert_eq!(err.error_code(), "TENANT_ID_REQUIRED");

    let err: DataScopeError = TenantError::InvalidTenantId(0).into();
    assert_eq!(err.error_code(), "INVALID_TENANT_ID");

    let err: DataScopeError = TenantError::TenantNotFound(5).into();
    assert_eq!(err.error_code(), "TENANT_NOT_FOUND");
}
