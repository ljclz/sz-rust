// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户热更新管理器 — 封装租户/隔离表/配置的 CRUD，
//! 统一处理校验、世代号自增、广播、审计、指标。

use chrono::Utc;
use std::sync::Arc;

use crate::data_scope::ext::audit::{AuditEvent, AuditLogger};
use crate::data_scope::ext::generation::PolicyGeneration;
use crate::data_scope::ext::notifier::{ChangeEvent, ChangeNotifier};
use crate::data_scope::metrics::DataScopeMetrics;
use crate::tenant::config::{ConfigSource, TenantConfig, TenantConfigRegistry, TenantConfigType};
use crate::tenant::error::TenantError;
use crate::tenant::record::{Tenant, TenantListFilter, TenantRecordRegistry};
use crate::tenant::scoped_table::{TenantScopedTable, TenantScopedTableRegistry};
use crate::tenant::status::TenantStatus;

/// 租户热更新管理器
pub struct TenantHotReloadManager {
    tenant_registry: Arc<TenantRecordRegistry>,
    scoped_table_registry: Arc<TenantScopedTableRegistry>,
    config_registry: Arc<TenantConfigRegistry>,
    generation: PolicyGeneration,
    notifier: ChangeNotifier,
    metrics: Arc<DataScopeMetrics>,
    audit: Arc<dyn AuditLogger>,
}

impl TenantHotReloadManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tenant_registry: Arc<TenantRecordRegistry>,
        scoped_table_registry: Arc<TenantScopedTableRegistry>,
        config_registry: Arc<TenantConfigRegistry>,
        generation: PolicyGeneration,
        notifier: ChangeNotifier,
        metrics: Arc<DataScopeMetrics>,
        audit: Arc<dyn AuditLogger>,
    ) -> Self {
        Self {
            tenant_registry,
            scoped_table_registry,
            config_registry,
            generation,
            notifier,
            metrics,
            audit,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.current()
    }

    pub fn tenant_count(&self) -> usize {
        self.tenant_registry.count()
    }

    pub fn active_tenant_count(&self) -> usize {
        self.tenant_registry.count_active()
    }

    fn refresh_active_count(&self) {
        self.metrics
            .set_tenant_active_count(self.tenant_registry.count_active() as u64);
    }

    // ====================================================================
    // 租户 CRUD 与状态流转
    // ====================================================================

    pub async fn create_tenant(&self, name: String) -> Result<Tenant, TenantError> {
        let tenant = self.tenant_registry.create(name)?;
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::TenantStatusChanged {
            generation: gen,
            tenant_id: tenant.tenant_id,
            from: "none".into(),
            to: tenant.status.as_str().into(),
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "create_tenant".into(),
                target_type: "tenant".into(),
                target_id: tenant.tenant_id.to_string(),
                before: None,
                after: Some(format!("{:?}", tenant)),
                timestamp: Utc::now(),
            })
            .await;
        self.refresh_active_count();
        Ok(tenant)
    }

    pub async fn update_tenant(
        &self,
        tenant_id: i64,
        name: Option<String>,
    ) -> Result<Tenant, TenantError> {
        let before = self
            .tenant_registry
            .get(tenant_id)
            .ok_or(TenantError::TenantNotFound(tenant_id))?;
        let tenant = self.tenant_registry.update(tenant_id, name)?;
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::TenantStatusChanged {
            generation: gen,
            tenant_id,
            from: before.status.as_str().into(),
            to: tenant.status.as_str().into(),
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "update_tenant".into(),
                target_type: "tenant".into(),
                target_id: tenant_id.to_string(),
                before: Some(format!("{:?}", before)),
                after: Some(format!("{:?}", tenant)),
                timestamp: Utc::now(),
            })
            .await;
        Ok(tenant)
    }

    pub async fn suspend_tenant(&self, tenant_id: i64) -> Result<Tenant, TenantError> {
        self.change_tenant_status(tenant_id, TenantStatus::Suspended, "suspend_tenant")
            .await
    }

    pub async fn activate_tenant(&self, tenant_id: i64) -> Result<Tenant, TenantError> {
        self.change_tenant_status(tenant_id, TenantStatus::Active, "activate_tenant")
            .await
    }

    pub async fn disable_tenant(&self, tenant_id: i64) -> Result<Tenant, TenantError> {
        self.change_tenant_status(tenant_id, TenantStatus::Disabled, "disable_tenant")
            .await
    }

    async fn change_tenant_status(
        &self,
        tenant_id: i64,
        target: TenantStatus,
        operation: &str,
    ) -> Result<Tenant, TenantError> {
        let before = self
            .tenant_registry
            .get(tenant_id)
            .ok_or(TenantError::TenantNotFound(tenant_id))?;
        let tenant = self.tenant_registry.update_status(tenant_id, target)?;
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::TenantStatusChanged {
            generation: gen,
            tenant_id,
            from: before.status.as_str().into(),
            to: tenant.status.as_str().into(),
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: operation.into(),
                target_type: "tenant".into(),
                target_id: tenant_id.to_string(),
                before: Some(before.status.as_str().into()),
                after: Some(tenant.status.as_str().into()),
                timestamp: Utc::now(),
            })
            .await;
        self.refresh_active_count();
        Ok(tenant)
    }

    pub async fn delete_tenant(&self, tenant_id: i64) -> Result<Tenant, TenantError> {
        let before = self
            .tenant_registry
            .get(tenant_id)
            .ok_or(TenantError::TenantNotFound(tenant_id))?;
        let tenant = self.tenant_registry.soft_delete(tenant_id)?;
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::TenantStatusChanged {
            generation: gen,
            tenant_id,
            from: before.status.as_str().into(),
            to: "deleted".into(),
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "delete_tenant".into(),
                target_type: "tenant".into(),
                target_id: tenant_id.to_string(),
                before: Some(format!("{:?}", before)),
                after: Some(format!("{:?}", tenant)),
                timestamp: Utc::now(),
            })
            .await;
        self.refresh_active_count();
        Ok(tenant)
    }

    pub fn get_tenant(&self, tenant_id: i64) -> Option<Tenant> {
        self.tenant_registry.get(tenant_id)
    }

    pub fn list_tenants(&self, filter: &TenantListFilter) -> Vec<Tenant> {
        self.tenant_registry.list(filter)
    }

    // ====================================================================
    // 租户隔离表清单 CRUD
    // ====================================================================

    pub async fn register_scoped_table(&self, table: TenantScopedTable) -> Result<(), TenantError> {
        let table_name = table.table_name.clone();
        self.scoped_table_registry.register(table);
        let gen = self.generation.bump();
        self.notifier
            .broadcast(ChangeEvent::TenantScopedTableChanged {
                generation: gen,
                table_name: table_name.clone(),
                action: "registered",
                timestamp: Utc::now(),
            });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "register_scoped_table".into(),
                target_type: "scoped_table".into(),
                target_id: table_name,
                before: None,
                after: None,
                timestamp: Utc::now(),
            })
            .await;
        Ok(())
    }

    pub async fn unregister_scoped_table(
        &self,
        table_name: &str,
    ) -> Result<TenantScopedTable, TenantError> {
        let removed = self
            .scoped_table_registry
            .unregister(table_name)
            .ok_or(TenantError::TenantNotFound(0))?;
        let gen = self.generation.bump();
        self.notifier
            .broadcast(ChangeEvent::TenantScopedTableChanged {
                generation: gen,
                table_name: table_name.to_string(),
                action: "unregistered",
                timestamp: Utc::now(),
            });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "unregister_scoped_table".into(),
                target_type: "scoped_table".into(),
                target_id: table_name.to_string(),
                before: Some(format!("{:?}", removed)),
                after: None,
                timestamp: Utc::now(),
            })
            .await;
        Ok(removed)
    }

    pub fn list_scoped_tables(&self) -> Vec<TenantScopedTable> {
        self.scoped_table_registry.list()
    }

    // ====================================================================
    // 租户配置与全局配置 CRUD
    // ====================================================================

    pub async fn set_tenant_config(
        &self,
        tenant_id: i64,
        key: impl Into<String>,
        value: impl Into<String>,
        config_type: TenantConfigType,
    ) -> Result<TenantConfig, TenantError> {
        let key = key.into();
        let config =
            self.config_registry
                .set_tenant_config(tenant_id, key.clone(), value, config_type)?;
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::TenantConfigChanged {
            generation: gen,
            tenant_id,
            config_key: key,
            is_global: false,
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "set_tenant_config".into(),
                target_type: "tenant_config".into(),
                target_id: format!("{}:{}", tenant_id, config.config_key),
                before: None,
                after: Some(format!("{:?}", config)),
                timestamp: Utc::now(),
            })
            .await;
        Ok(config)
    }

    pub async fn delete_tenant_config(
        &self,
        tenant_id: i64,
        key: &str,
    ) -> Result<TenantConfig, TenantError> {
        let removed = self
            .config_registry
            .delete_tenant_config(tenant_id, key)
            .ok_or(TenantError::TenantNotFound(tenant_id))?;
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::TenantConfigChanged {
            generation: gen,
            tenant_id,
            config_key: key.to_string(),
            is_global: false,
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "delete_tenant_config".into(),
                target_type: "tenant_config".into(),
                target_id: format!("{}:{}", tenant_id, key),
                before: Some(format!("{:?}", removed)),
                after: None,
                timestamp: Utc::now(),
            })
            .await;
        Ok(removed)
    }

    pub fn get_tenant_config(
        &self,
        tenant_id: i64,
        key: &str,
    ) -> Option<(serde_json::Value, ConfigSource)> {
        self.config_registry.get(tenant_id, key)
    }

    pub fn list_tenant_configs(&self, tenant_id: i64) -> Vec<(TenantConfig, ConfigSource)> {
        self.config_registry.list_tenant_configs(tenant_id)
    }

    pub async fn set_global_config(
        &self,
        key: impl Into<String>,
        value: impl Into<String>,
        config_type: TenantConfigType,
    ) -> Result<TenantConfig, TenantError> {
        let key = key.into();
        let config = self
            .config_registry
            .set_global_config(key.clone(), value, config_type)?;
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::TenantConfigChanged {
            generation: gen,
            tenant_id: 0,
            config_key: key,
            is_global: true,
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "set_global_config".into(),
                target_type: "global_config".into(),
                target_id: config.config_key.clone(),
                before: None,
                after: Some(format!("{:?}", config)),
                timestamp: Utc::now(),
            })
            .await;
        Ok(config)
    }

    pub fn list_global_configs(&self) -> Vec<TenantConfig> {
        self.config_registry.list_global_configs()
    }

    /// 内部引用：隔离表注册表（供 ConfigLoader 使用）
    pub fn scoped_table_registry(&self) -> &Arc<TenantScopedTableRegistry> {
        &self.scoped_table_registry
    }

    /// 内部引用：世代号
    pub fn generation_handle(&self) -> &PolicyGeneration {
        &self.generation
    }

    /// 内部引用：变更通知器
    pub fn notifier(&self) -> &ChangeNotifier {
        &self.notifier
    }

    /// 内部引用：指标
    pub fn metrics(&self) -> &Arc<DataScopeMetrics> {
        &self.metrics
    }

    /// 记审计日志（供 ConfigLoader 调用）
    pub async fn audit_record(&self, operation: &str, target_id: &str) {
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: operation.into(),
                target_type: "config".into(),
                target_id: target_id.to_string(),
                before: None,
                after: None,
                timestamp: Utc::now(),
            })
            .await;
    }

    /// 广播 ConfigReloaded 事件
    pub fn broadcast_config_reloaded(&self, gen: u64, path: &str) {
        self.notifier.broadcast(ChangeEvent::ConfigReloaded {
            generation: gen,
            target_table: path.to_string(),
            timestamp: Utc::now(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_scope::ext::audit::TracingAuditLogger;

    fn make_manager() -> TenantHotReloadManager {
        TenantHotReloadManager::new(
            Arc::new(TenantRecordRegistry::new()),
            Arc::new(TenantScopedTableRegistry::new()),
            Arc::new(TenantConfigRegistry::new()),
            PolicyGeneration::new(),
            ChangeNotifier::new(64),
            Arc::new(DataScopeMetrics::new()),
            Arc::new(TracingAuditLogger),
        )
    }

    #[tokio::test]
    async fn test_create_tenant_success() {
        let mgr = make_manager();
        let tenant = mgr.create_tenant("acme".into()).await.unwrap();
        assert_eq!(tenant.tenant_id, 1);
        assert_eq!(tenant.status, TenantStatus::Active);
        assert!(mgr.generation() >= 1);
    }

    #[tokio::test]
    async fn test_create_duplicate_name() {
        let mgr = make_manager();
        mgr.create_tenant("acme".into()).await.unwrap();
        let err = mgr.create_tenant("acme".into()).await.unwrap_err();
        assert_eq!(err.error_code(), "TENANT_NAME_DUPLICATE");
    }

    #[tokio::test]
    async fn test_suspend_and_activate() {
        let mgr = make_manager();
        let t = mgr.create_tenant("acme".into()).await.unwrap();

        let suspended = mgr.suspend_tenant(t.tenant_id).await.unwrap();
        assert_eq!(suspended.status, TenantStatus::Suspended);

        let activated = mgr.activate_tenant(t.tenant_id).await.unwrap();
        assert_eq!(activated.status, TenantStatus::Active);
    }

    #[tokio::test]
    async fn test_disable_then_cannot_activate() {
        let mgr = make_manager();
        let t = mgr.create_tenant("acme".into()).await.unwrap();

        mgr.disable_tenant(t.tenant_id).await.unwrap();

        let err = mgr.activate_tenant(t.tenant_id).await.unwrap_err();
        assert_eq!(err.error_code(), "INVALID_STATUS_TRANSITION");
    }

    #[tokio::test]
    async fn test_delete_only_disabled() {
        let mgr = make_manager();
        let t = mgr.create_tenant("acme".into()).await.unwrap();

        let err = mgr.delete_tenant(t.tenant_id).await.unwrap_err();
        assert_eq!(err.error_code(), "TENANT_NOT_DISABLED");

        mgr.disable_tenant(t.tenant_id).await.unwrap();
        let deleted = mgr.delete_tenant(t.tenant_id).await.unwrap();
        assert!(deleted.deleted_at.is_some());
    }

    #[tokio::test]
    async fn test_generation_monotonic() {
        let mgr = make_manager();
        let g0 = mgr.generation();
        mgr.create_tenant("a".into()).await.unwrap();
        let g1 = mgr.generation();
        mgr.create_tenant("b".into()).await.unwrap();
        let g2 = mgr.generation();
        assert!(g1 > g0);
        assert!(g2 > g1);
    }

    #[tokio::test]
    async fn test_register_scoped_table_broadcasts() {
        let mgr = make_manager();
        let mut rx = mgr.notifier().subscribe();

        mgr.register_scoped_table(TenantScopedTable::new("orders"))
            .await
            .unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            ChangeEvent::TenantScopedTableChanged {
                table_name, action, ..
            } => {
                assert_eq!(table_name, "orders");
                assert_eq!(action, "registered");
            }
            _ => panic!("expected TenantScopedTableChanged"),
        }
        assert_eq!(mgr.list_scoped_tables().len(), 1);
    }

    #[tokio::test]
    async fn test_unregister_scoped_table() {
        let mgr = make_manager();
        mgr.register_scoped_table(TenantScopedTable::new("orders"))
            .await
            .unwrap();
        assert_eq!(mgr.list_scoped_tables().len(), 1);

        let removed = mgr.unregister_scoped_table("orders").await.unwrap();
        assert_eq!(removed.table_name, "orders");
        assert_eq!(mgr.list_scoped_tables().len(), 0);
    }

    #[tokio::test]
    async fn test_set_tenant_config_broadcasts() {
        let mgr = make_manager();
        let mut rx = mgr.notifier().subscribe();

        mgr.set_tenant_config(1, "theme", "dark", TenantConfigType::String)
            .await
            .unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            ChangeEvent::TenantConfigChanged {
                tenant_id,
                config_key,
                is_global,
                ..
            } => {
                assert_eq!(tenant_id, 1);
                assert_eq!(config_key, "theme");
                assert!(!is_global);
            }
            _ => panic!("expected TenantConfigChanged"),
        }
    }

    #[tokio::test]
    async fn test_set_global_config() {
        let mgr = make_manager();
        mgr.set_global_config("theme", "dark", TenantConfigType::String)
            .await
            .unwrap();

        let configs = mgr.list_global_configs();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].config_key, "theme");
        assert!(configs[0].is_global);
    }

    #[tokio::test]
    async fn test_delete_config_cache_invalidation() {
        let mgr = make_manager();
        mgr.set_global_config("theme", "dark", TenantConfigType::String)
            .await
            .unwrap();
        mgr.set_tenant_config(1, "theme", "light", TenantConfigType::String)
            .await
            .unwrap();

        let (value, source) = mgr.get_tenant_config(1, "theme").unwrap();
        assert_eq!(value, serde_json::Value::String("light".into()));
        assert_eq!(source, ConfigSource::Tenant);

        mgr.delete_tenant_config(1, "theme").await.unwrap();

        let (value, source) = mgr.get_tenant_config(1, "theme").unwrap();
        assert_eq!(value, serde_json::Value::String("dark".into()));
        assert_eq!(source, ConfigSource::Global);
    }

    #[tokio::test]
    async fn test_active_tenant_count_updates() {
        let mgr = make_manager();
        mgr.create_tenant("a".into()).await.unwrap();
        mgr.create_tenant("b".into()).await.unwrap();
        assert_eq!(mgr.active_tenant_count(), 2);

        let t = mgr.get_tenant(1).unwrap();
        mgr.suspend_tenant(t.tenant_id).await.unwrap();
        assert_eq!(mgr.active_tenant_count(), 1);
        assert_eq!(mgr.metrics().tenant_active_count(), 1);
    }
}
