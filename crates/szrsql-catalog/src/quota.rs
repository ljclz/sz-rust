//! Multi-tenant resource quota management — Phase 3.10.
//!
//! # Design
//!
//! - **`TenantQuota`** — Per-tenant resource limits:
//!   - `max_connections: Option<usize>` — Max concurrent connections (None = unlimited)
//!   - `max_storage_bytes: Option<u64>` — Max total storage in bytes (None = unlimited)
//!   - `max_tables: Option<usize>` — Max table count (None = unlimited)
//! - **`TenantUsage`** — Current resource usage (mutable, updated as resources are allocated/freed)
//! - **`QuotaManager`** — Manages quotas for multiple tenants:
//!   - `set_quota(tenant_id, quota)` — Set/replace a tenant's quota
//!   - `quota(tenant_id) -> Option<&TenantQuota>` — Get a tenant's quota
//!   - `try_acquire_connection(tenant_id) -> Result<(), QuotaError>` — Acquire a connection slot
//!   - `release_connection(tenant_id)` — Release a connection slot
//!   - `try_consume_storage(tenant_id, bytes) -> Result<(), QuotaError>` — Consume storage
//!   - `release_storage(tenant_id, bytes)` — Release storage
//!   - `try_create_table(tenant_id) -> Result<(), QuotaError>` — Acquire a table slot
//!   - `release_table(tenant_id)` — Release a table slot
//!   - `usage(tenant_id) -> Option<&TenantUsage>` — Get current usage
//!
//! # Quota enforcement semantics
//!
//! - **max_connections**: A tenant exceeding the limit gets `QuotaError::ConnectionLimitExceeded`
//!   on `try_acquire_connection`. Other tenants are unaffected.
//! - **max_storage**: An INSERT that would push storage over the limit gets
//!   `QuotaError::StorageLimitExceeded`. Already-committed data is not rolled back —
//!   the caller (executor) must handle this by aborting the INSERT.
//! - **max_tables**: CREATE TABLE exceeding the limit gets `QuotaError::TableLimitExceeded`.
//!
//! Corresponds to `SzRSQL实施进度.md` Phase 3.10.

use std::collections::HashMap;
use thiserror::Error;

// =====================================================================
//  Quota error
// =====================================================================

/// Quota enforcement error
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QuotaError {
    /// Tenant not registered with the quota manager
    #[error("tenant not registered: {0}")]
    TenantNotRegistered(String),
    /// Connection limit exceeded
    #[error("connection limit exceeded for tenant {tenant}: {current}/{max} (max_connections)")]
    ConnectionLimitExceeded {
        /// Tenant ID
        tenant: String,
        /// Current connection count (before this attempt)
        current: usize,
        /// Max allowed
        max: usize,
    },
    /// Storage limit exceeded
    #[error("storage limit exceeded for tenant {tenant}: {current} + {requested} > {max} bytes")]
    StorageLimitExceeded {
        /// Tenant ID
        tenant: String,
        /// Current storage usage in bytes
        current: u64,
        /// Requested additional bytes
        requested: u64,
        /// Max allowed
        max: u64,
    },
    /// Table count limit exceeded
    #[error("table limit exceeded for tenant {tenant}: {current}/{max} (max_tables)")]
    TableLimitExceeded {
        /// Tenant ID
        tenant: String,
        /// Current table count
        current: usize,
        /// Max allowed
        max: usize,
    },
}

// =====================================================================
//  TenantQuota — per-tenant limits
// =====================================================================

/// Per-tenant resource limits
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TenantQuota {
    /// Max concurrent connections (None = unlimited)
    pub max_connections: Option<usize>,
    /// Max total storage in bytes (None = unlimited)
    pub max_storage_bytes: Option<u64>,
    /// Max table count (None = unlimited)
    pub max_tables: Option<usize>,
}

impl TenantQuota {
    /// Create an empty (unlimited) quota
    pub fn new() -> Self {
        Self::default()
    }

    /// Set max connections
    pub fn with_max_connections(mut self, n: usize) -> Self {
        self.max_connections = Some(n);
        self
    }

    /// Set max storage bytes
    pub fn with_max_storage_bytes(mut self, n: u64) -> Self {
        self.max_storage_bytes = Some(n);
        self
    }

    /// Set max tables
    pub fn with_max_tables(mut self, n: usize) -> Self {
        self.max_tables = Some(n);
        self
    }
}

// =====================================================================
//  TenantUsage — current resource usage
// =====================================================================

/// Current resource usage for a tenant
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TenantUsage {
    /// Active connections
    pub connections: usize,
    /// Storage consumed in bytes
    pub storage_bytes: u64,
    /// Table count
    pub tables: usize,
}

impl TenantUsage {
    /// Create zero usage
    pub fn new() -> Self {
        Self::default()
    }
}

// =====================================================================
//  QuotaManager
// =====================================================================

/// Quota manager — tracks per-tenant quotas and usage
#[derive(Debug, Default, Clone)]
pub struct QuotaManager {
    /// tenant_id → (quota, usage)
    tenants: HashMap<String, (TenantQuota, TenantUsage)>,
}

impl QuotaManager {
    /// Create an empty quota manager
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tenant with a quota (replaces existing quota if any; usage is preserved)
    pub fn set_quota(&mut self, tenant_id: impl Into<String>, quota: TenantQuota) {
        let tenant_id = tenant_id.into();
        self.tenants
            .entry(tenant_id)
            .or_insert_with(|| (TenantQuota::default(), TenantUsage::default()))
            .0 = quota;
    }

    /// Get a tenant's quota
    pub fn quota(&self, tenant_id: &str) -> Option<&TenantQuota> {
        self.tenants.get(tenant_id).map(|(q, _)| q)
    }

    /// Get a tenant's current usage
    pub fn usage(&self, tenant_id: &str) -> Option<&TenantUsage> {
        self.tenants.get(tenant_id).map(|(_, u)| u)
    }

    /// Check if a tenant is registered
    pub fn is_registered(&self, tenant_id: &str) -> bool {
        self.tenants.contains_key(tenant_id)
    }

    /// Number of registered tenants
    pub fn tenant_count(&self) -> usize {
        self.tenants.len()
    }

    /// Try to acquire a connection slot for a tenant
    ///
    /// Returns `QuotaError::ConnectionLimitExceeded` if the limit is reached,
    /// `QuotaError::TenantNotRegistered` if the tenant has no quota set.
    pub fn try_acquire_connection(&mut self, tenant_id: &str) -> Result<(), QuotaError> {
        let (quota, usage) = self
            .tenants
            .get_mut(tenant_id)
            .ok_or_else(|| QuotaError::TenantNotRegistered(tenant_id.to_string()))?;
        if let Some(max) = quota.max_connections {
            if usage.connections >= max {
                return Err(QuotaError::ConnectionLimitExceeded {
                    tenant: tenant_id.to_string(),
                    current: usage.connections,
                    max,
                });
            }
        }
        usage.connections += 1;
        Ok(())
    }

    /// Release a connection slot for a tenant
    ///
    /// Saturates at 0 (no error if usage is already 0).
    pub fn release_connection(&mut self, tenant_id: &str) {
        if let Some((_, usage)) = self.tenants.get_mut(tenant_id) {
            usage.connections = usage.connections.saturating_sub(1);
        }
    }

    /// Try to consume storage for a tenant
    ///
    /// Returns `QuotaError::StorageLimitExceeded` if the limit would be exceeded.
    pub fn try_consume_storage(&mut self, tenant_id: &str, bytes: u64) -> Result<(), QuotaError> {
        let (quota, usage) = self
            .tenants
            .get_mut(tenant_id)
            .ok_or_else(|| QuotaError::TenantNotRegistered(tenant_id.to_string()))?;
        if let Some(max) = quota.max_storage_bytes {
            let new_total = usage.storage_bytes.saturating_add(bytes);
            if new_total > max {
                return Err(QuotaError::StorageLimitExceeded {
                    tenant: tenant_id.to_string(),
                    current: usage.storage_bytes,
                    requested: bytes,
                    max,
                });
            }
        }
        usage.storage_bytes = usage.storage_bytes.saturating_add(bytes);
        Ok(())
    }

    /// Release storage for a tenant
    ///
    /// Saturates at 0 (no error if usage goes negative).
    pub fn release_storage(&mut self, tenant_id: &str, bytes: u64) {
        if let Some((_, usage)) = self.tenants.get_mut(tenant_id) {
            usage.storage_bytes = usage.storage_bytes.saturating_sub(bytes);
        }
    }

    /// Try to acquire a table slot for a tenant (for CREATE TABLE)
    pub fn try_create_table(&mut self, tenant_id: &str) -> Result<(), QuotaError> {
        let (quota, usage) = self
            .tenants
            .get_mut(tenant_id)
            .ok_or_else(|| QuotaError::TenantNotRegistered(tenant_id.to_string()))?;
        if let Some(max) = quota.max_tables {
            if usage.tables >= max {
                return Err(QuotaError::TableLimitExceeded {
                    tenant: tenant_id.to_string(),
                    current: usage.tables,
                    max,
                });
            }
        }
        usage.tables += 1;
        Ok(())
    }

    /// Release a table slot for a tenant (for DROP TABLE)
    pub fn release_table(&mut self, tenant_id: &str) {
        if let Some((_, usage)) = self.tenants.get_mut(tenant_id) {
            usage.tables = usage.tables.saturating_sub(1);
        }
    }

    /// Reset a tenant's usage (e.g. on disconnect / cleanup)
    pub fn reset_usage(&mut self, tenant_id: &str) {
        if let Some((_, usage)) = self.tenants.get_mut(tenant_id) {
            *usage = TenantUsage::default();
        }
    }

    /// Remove a tenant entirely
    pub fn remove_tenant(&mut self, tenant_id: &str) -> Option<(TenantQuota, TenantUsage)> {
        self.tenants.remove(tenant_id)
    }
}
