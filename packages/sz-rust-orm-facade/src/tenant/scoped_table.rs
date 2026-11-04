// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户隔离表注册表 — 标记哪些表需要 tenant_id 自动注入

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantScopedTable {
    pub table_name: String,
    pub tenant_field: String,
    pub enabled: bool,
}

impl TenantScopedTable {
    pub fn new(table_name: impl Into<String>) -> Self {
        Self {
            table_name: table_name.into(),
            tenant_field: "tenant_id".to_string(),
            enabled: true,
        }
    }

    pub fn with_tenant_field(mut self, field: impl Into<String>) -> Self {
        self.tenant_field = field.into();
        self
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

pub struct TenantScopedTableRegistry {
    tables: DashMap<String, TenantScopedTable>,
}

impl TenantScopedTableRegistry {
    pub fn new() -> Self {
        Self {
            tables: DashMap::new(),
        }
    }

    pub fn register(&self, table: TenantScopedTable) {
        self.tables.insert(table.table_name.clone(), table);
    }

    pub fn unregister(&self, table_name: &str) -> Option<TenantScopedTable> {
        self.tables.remove(table_name).map(|(_, v)| v)
    }

    pub fn is_tenant_scoped(&self, table_name: &str) -> bool {
        self.tables
            .get(table_name)
            .map(|t| t.enabled)
            .unwrap_or(false)
    }

    pub fn get_tenant_field(&self, table_name: &str) -> Option<String> {
        self.tables.get(table_name).and_then(|t| {
            if t.enabled {
                Some(t.tenant_field.clone())
            } else {
                None
            }
        })
    }

    pub fn list(&self) -> Vec<TenantScopedTable> {
        self.tables.iter().map(|t| t.clone()).collect()
    }

    pub fn count(&self) -> usize {
        self.tables.len()
    }

    pub fn invalidate_all(&self) {
        self.tables.clear();
    }
}

impl Default for TenantScopedTableRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_upsert() {
        let registry = TenantScopedTableRegistry::new();
        registry.register(TenantScopedTable::new("orders"));
        assert_eq!(registry.count(), 1);

        registry.register(TenantScopedTable::new("orders").with_tenant_field("tid"));
        assert_eq!(registry.count(), 1);
        assert_eq!(registry.get_tenant_field("orders").unwrap(), "tid");
    }

    #[test]
    fn test_is_tenant_scoped_enabled_flag() {
        let registry = TenantScopedTableRegistry::new();
        registry.register(TenantScopedTable::new("orders").with_enabled(false));
        assert!(!registry.is_tenant_scoped("orders"));

        registry.register(TenantScopedTable::new("orders").with_enabled(true));
        assert!(registry.is_tenant_scoped("orders"));
    }

    #[test]
    fn test_get_tenant_field_default() {
        let registry = TenantScopedTableRegistry::new();
        registry.register(TenantScopedTable::new("orders"));
        assert_eq!(registry.get_tenant_field("orders").unwrap(), "tenant_id");
    }

    #[test]
    fn test_invalidate_all() {
        let registry = TenantScopedTableRegistry::new();
        registry.register(TenantScopedTable::new("orders"));
        registry.register(TenantScopedTable::new("employees"));
        assert_eq!(registry.count(), 2);

        registry.invalidate_all();
        assert_eq!(registry.count(), 0);
        assert!(!registry.is_tenant_scoped("orders"));
    }
}
