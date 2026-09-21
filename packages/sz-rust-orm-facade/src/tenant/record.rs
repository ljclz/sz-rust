// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户记录与注册表 — 内存注册表，持久化由业务层 Repository 负责

use chrono::Utc;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, Ordering};

use super::error::TenantError;
use super::status::TenantStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub tenant_id: i64,
    pub name: String,
    pub status: TenantStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Tenant {
    pub fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            tenant_id: 0,
            name: name.into(),
            status: TenantStatus::Active,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    pub fn with_tenant_id(mut self, id: i64) -> Self {
        self.tenant_id = id;
        self
    }
}

#[derive(Debug, Default, Clone)]
pub struct TenantListFilter {
    pub status: Option<TenantStatus>,
}

pub struct TenantRecordRegistry {
    tenants: DashMap<i64, Tenant>,
    name_index: DashMap<String, i64>,
    next_id: AtomicI64,
}

impl TenantRecordRegistry {
    pub fn new() -> Self {
        Self {
            tenants: DashMap::new(),
            name_index: DashMap::new(),
            next_id: AtomicI64::new(1),
        }
    }

    pub fn create(&self, name: String) -> Result<Tenant, TenantError> {
        if self.name_index.contains_key(&name) {
            return Err(TenantError::TenantNameDuplicate(name));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let now = Utc::now();
        let tenant = Tenant {
            tenant_id: id,
            name: name.clone(),
            status: TenantStatus::Active,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        };
        self.tenants.insert(id, tenant.clone());
        self.name_index.insert(name, id);
        Ok(tenant)
    }

    pub fn get(&self, tenant_id: i64) -> Option<Tenant> {
        self.tenants.get(&tenant_id).map(|t| t.clone())
    }

    pub fn get_by_name(&self, name: &str) -> Option<Tenant> {
        self.name_index
            .get(name)
            .and_then(|id| self.tenants.get(&*id).map(|t| t.clone()))
    }

    pub fn update(&self, tenant_id: i64, name: Option<String>) -> Result<Tenant, TenantError> {
        let mut entry = self
            .tenants
            .get_mut(&tenant_id)
            .ok_or(TenantError::TenantNotFound(tenant_id))?;

        if let Some(new_name) = name {
            if new_name != entry.name {
                if self.name_index.contains_key(&new_name) {
                    return Err(TenantError::TenantNameDuplicate(new_name));
                }
                self.name_index.remove(&entry.name);
                self.name_index.insert(new_name.clone(), tenant_id);
                entry.name = new_name;
            }
        }
        entry.updated_at = Utc::now();
        Ok(entry.clone())
    }

    pub fn update_status(
        &self,
        tenant_id: i64,
        target: TenantStatus,
    ) -> Result<Tenant, TenantError> {
        let mut entry = self
            .tenants
            .get_mut(&tenant_id)
            .ok_or(TenantError::TenantNotFound(tenant_id))?;

        let new_status = entry.status.transition_to(target)?;
        entry.status = new_status;
        entry.updated_at = Utc::now();
        Ok(entry.clone())
    }

    pub fn soft_delete(&self, tenant_id: i64) -> Result<Tenant, TenantError> {
        let mut entry = self
            .tenants
            .get_mut(&tenant_id)
            .ok_or(TenantError::TenantNotFound(tenant_id))?;

        if entry.status != TenantStatus::Disabled {
            return Err(TenantError::TenantNotDisabled(tenant_id));
        }

        entry.deleted_at = Some(Utc::now());
        self.name_index.remove(&entry.name);
        Ok(entry.clone())
    }

    pub fn list(&self, filter: &TenantListFilter) -> Vec<Tenant> {
        self.tenants
            .iter()
            .filter(|t| t.deleted_at.is_none())
            .filter(|t| filter.status.map_or(true, |s| t.status == s))
            .map(|t| t.clone())
            .collect()
    }

    pub fn count(&self) -> usize {
        self.tenants
            .iter()
            .filter(|t| t.deleted_at.is_none())
            .count()
    }

    pub fn count_active(&self) -> usize {
        self.tenants
            .iter()
            .filter(|t| t.deleted_at.is_none() && t.status == TenantStatus::Active)
            .count()
    }
}

impl Default for TenantRecordRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_tenant_auto_id() {
        let registry = TenantRecordRegistry::new();
        let t1 = registry.create("acme".into()).unwrap();
        let t2 = registry.create("globex".into()).unwrap();
        assert_eq!(t1.tenant_id, 1);
        assert_eq!(t2.tenant_id, 2);
        assert_eq!(t1.status, TenantStatus::Active);
    }

    #[test]
    fn test_create_duplicate_name() {
        let registry = TenantRecordRegistry::new();
        registry.create("acme".into()).unwrap();
        let result = registry.create("acme".into());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().error_code(), "TENANT_NAME_DUPLICATE");
    }

    #[test]
    fn test_update_status_valid() {
        let registry = TenantRecordRegistry::new();
        let t = registry.create("acme".into()).unwrap();
        let updated = registry
            .update_status(t.tenant_id, TenantStatus::Suspended)
            .unwrap();
        assert_eq!(updated.status, TenantStatus::Suspended);
    }

    #[test]
    fn test_soft_delete_only_disabled() {
        let registry = TenantRecordRegistry::new();
        let t = registry.create("acme".into()).unwrap();
        let result = registry.soft_delete(t.tenant_id);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().error_code(), "TENANT_NOT_DISABLED");

        registry
            .update_status(t.tenant_id, TenantStatus::Disabled)
            .unwrap();
        let deleted = registry.soft_delete(t.tenant_id).unwrap();
        assert!(deleted.deleted_at.is_some());
    }

    #[test]
    fn test_list_filter_by_status() {
        let registry = TenantRecordRegistry::new();
        registry.create("acme".into()).unwrap();
        let t2 = registry.create("globex".into()).unwrap();
        registry
            .update_status(t2.tenant_id, TenantStatus::Suspended)
            .unwrap();

        let active = registry.list(&TenantListFilter {
            status: Some(TenantStatus::Active),
        });
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "acme");

        let suspended = registry.list(&TenantListFilter {
            status: Some(TenantStatus::Suspended),
        });
        assert_eq!(suspended.len(), 1);
        assert_eq!(suspended[0].name, "globex");
    }

    #[test]
    fn test_count_active() {
        let registry = TenantRecordRegistry::new();
        registry.create("acme".into()).unwrap();
        let t2 = registry.create("globex".into()).unwrap();
        registry
            .update_status(t2.tenant_id, TenantStatus::Suspended)
            .unwrap();
        assert_eq!(registry.count_active(), 1);
        assert_eq!(registry.count(), 2);
    }
}
