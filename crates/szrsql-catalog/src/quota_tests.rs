//! Phase 3.10 multi-tenant resource quota tests.
//!
//! Coverage:
//! - TenantQuota builders (3): new / chained with_* / default unlimited
//! - QuotaManager registration (3): set_quota / quota / is_registered / tenant_count
//! - Connection quota (5): acquire below limit / acquire at limit / release + reacquire / unregistered / unlimited (None)
//! - Storage quota (5): consume below limit / consume exact fit / consume over limit / release + reconsume / unlimited
//! - Table quota (4): create below limit / create at limit / release + recreate / unlimited
//! - Cross-tenant isolation (1): tenant A limit does not affect tenant B
//! - Stress scenario (2): tenant A at limit, tenant B operates normally / tenant A blocked on storage, tenant B INSERT works
//! - Reset / remove (2): reset_usage clears usage / remove_tenant unregisters
//! - Saturating release (1): release more than acquired saturates at 0
//!
//! 26 test cases.

use super::quota::{QuotaError, QuotaManager, TenantQuota};

// =====================================================================
//  TenantQuota builders (3)
// =====================================================================

#[test]
fn test_quota_new_unlimited() {
    let q = TenantQuota::new();
    assert_eq!(q.max_connections, None);
    assert_eq!(q.max_storage_bytes, None);
    assert_eq!(q.max_tables, None);
}

#[test]
fn test_quota_chained_builders() {
    let q = TenantQuota::new()
        .with_max_connections(10)
        .with_max_storage_bytes(1024)
        .with_max_tables(5);
    assert_eq!(q.max_connections, Some(10));
    assert_eq!(q.max_storage_bytes, Some(1024));
    assert_eq!(q.max_tables, Some(5));
}

#[test]
fn test_quota_default_unlimited() {
    let q = TenantQuota::default();
    assert_eq!(q.max_connections, None);
    assert_eq!(q.max_storage_bytes, None);
    assert_eq!(q.max_tables, None);
}

// =====================================================================
//  QuotaManager registration (3)
// =====================================================================

#[test]
fn test_manager_set_quota() {
    let mut mgr = QuotaManager::new();
    assert!(!mgr.is_registered("t1"));

    mgr.set_quota("t1", TenantQuota::new().with_max_connections(5));
    assert!(mgr.is_registered("t1"));
    assert_eq!(mgr.tenant_count(), 1);
    assert_eq!(mgr.quota("t1").unwrap().max_connections, Some(5));
}

#[test]
fn test_manager_set_quota_replaces_quota_preserves_usage() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_connections(2));
    // Acquire 1 connection
    mgr.try_acquire_connection("t1").unwrap();
    assert_eq!(mgr.usage("t1").unwrap().connections, 1);

    // Replace quota — usage should be preserved
    mgr.set_quota("t1", TenantQuota::new().with_max_connections(5));
    assert_eq!(mgr.quota("t1").unwrap().max_connections, Some(5));
    assert_eq!(mgr.usage("t1").unwrap().connections, 1);
}

#[test]
fn test_manager_unregistered_tenant_returns_none() {
    let mgr = QuotaManager::new();
    assert!(mgr.quota("t1").is_none());
    assert!(mgr.usage("t1").is_none());
    assert!(!mgr.is_registered("t1"));
}

// =====================================================================
//  Connection quota (5)
// =====================================================================

#[test]
fn test_connection_acquire_below_limit() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_connections(3));

    mgr.try_acquire_connection("t1").unwrap();
    mgr.try_acquire_connection("t1").unwrap();
    assert_eq!(mgr.usage("t1").unwrap().connections, 2);
}

#[test]
fn test_connection_acquire_at_limit_rejected() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_connections(2));

    mgr.try_acquire_connection("t1").unwrap();
    mgr.try_acquire_connection("t1").unwrap();

    let result = mgr.try_acquire_connection("t1");
    assert_eq!(
        result,
        Err(QuotaError::ConnectionLimitExceeded {
            tenant: "t1".into(),
            current: 2,
            max: 2,
        })
    );
    // Usage unchanged after failed acquire
    assert_eq!(mgr.usage("t1").unwrap().connections, 2);
}

#[test]
fn test_connection_release_then_reacquire() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_connections(1));

    mgr.try_acquire_connection("t1").unwrap();
    assert_eq!(mgr.usage("t1").unwrap().connections, 1);

    // At limit
    assert!(mgr.try_acquire_connection("t1").is_err());

    // Release → can reacquire
    mgr.release_connection("t1");
    assert_eq!(mgr.usage("t1").unwrap().connections, 0);
    mgr.try_acquire_connection("t1").unwrap();
    assert_eq!(mgr.usage("t1").unwrap().connections, 1);
}

#[test]
fn test_connection_unregistered_tenant_error() {
    let mut mgr = QuotaManager::new();
    let result = mgr.try_acquire_connection("t1");
    assert_eq!(result, Err(QuotaError::TenantNotRegistered("t1".into())));
}

#[test]
fn test_connection_unlimited_quota() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new()); // no limits

    for _ in 0..1000 {
        mgr.try_acquire_connection("t1").unwrap();
    }
    assert_eq!(mgr.usage("t1").unwrap().connections, 1000);
}

// =====================================================================
//  Storage quota (5)
// =====================================================================

#[test]
fn test_storage_consume_below_limit() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_storage_bytes(1024));

    mgr.try_consume_storage("t1", 500).unwrap();
    assert_eq!(mgr.usage("t1").unwrap().storage_bytes, 500);

    mgr.try_consume_storage("t1", 400).unwrap();
    assert_eq!(mgr.usage("t1").unwrap().storage_bytes, 900);
}

#[test]
fn test_storage_consume_exact_fit() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_storage_bytes(1024));

    mgr.try_consume_storage("t1", 1024).unwrap();
    assert_eq!(mgr.usage("t1").unwrap().storage_bytes, 1024);
}

#[test]
fn test_storage_consume_over_limit_rejected() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_storage_bytes(1024));

    mgr.try_consume_storage("t1", 500).unwrap();
    let result = mgr.try_consume_storage("t1", 600); // 500 + 600 = 1100 > 1024
    assert_eq!(
        result,
        Err(QuotaError::StorageLimitExceeded {
            tenant: "t1".into(),
            current: 500,
            requested: 600,
            max: 1024,
        })
    );
    // Usage unchanged after failed consume
    assert_eq!(mgr.usage("t1").unwrap().storage_bytes, 500);
}

#[test]
fn test_storage_release_then_reconsume() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_storage_bytes(1024));

    mgr.try_consume_storage("t1", 800).unwrap();
    mgr.release_storage("t1", 500);
    assert_eq!(mgr.usage("t1").unwrap().storage_bytes, 300);

    // Now we can consume 700 more (300 + 700 = 1000 <= 1024)
    mgr.try_consume_storage("t1", 700).unwrap();
    assert_eq!(mgr.usage("t1").unwrap().storage_bytes, 1000);
}

#[test]
fn test_storage_unlimited_quota() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new());

    mgr.try_consume_storage("t1", u64::MAX / 2).unwrap();
    assert_eq!(mgr.usage("t1").unwrap().storage_bytes, u64::MAX / 2);
}

// =====================================================================
//  Table quota (4)
// =====================================================================

#[test]
fn test_table_create_below_limit() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_tables(3));

    mgr.try_create_table("t1").unwrap();
    mgr.try_create_table("t1").unwrap();
    assert_eq!(mgr.usage("t1").unwrap().tables, 2);
}

#[test]
fn test_table_create_at_limit_rejected() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_tables(1));

    mgr.try_create_table("t1").unwrap();
    let result = mgr.try_create_table("t1");
    assert_eq!(
        result,
        Err(QuotaError::TableLimitExceeded {
            tenant: "t1".into(),
            current: 1,
            max: 1,
        })
    );
}

#[test]
fn test_table_release_then_recreate() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_tables(1));

    mgr.try_create_table("t1").unwrap();
    assert!(mgr.try_create_table("t1").is_err());

    mgr.release_table("t1"); // DROP TABLE
    assert_eq!(mgr.usage("t1").unwrap().tables, 0);
    mgr.try_create_table("t1").unwrap();
}

#[test]
fn test_table_unlimited_quota() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new());

    for _ in 0..100 {
        mgr.try_create_table("t1").unwrap();
    }
    assert_eq!(mgr.usage("t1").unwrap().tables, 100);
}

// =====================================================================
//  Cross-tenant isolation (1)
// =====================================================================

#[test]
fn test_cross_tenant_isolation_limits_independent() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("tA", TenantQuota::new().with_max_connections(2));
    mgr.set_quota("tB", TenantQuota::new().with_max_connections(5));

    // tA hits its limit
    mgr.try_acquire_connection("tA").unwrap();
    mgr.try_acquire_connection("tA").unwrap();
    assert!(mgr.try_acquire_connection("tA").is_err());

    // tB is unaffected — can still acquire up to 5
    mgr.try_acquire_connection("tB").unwrap();
    mgr.try_acquire_connection("tB").unwrap();
    mgr.try_acquire_connection("tB").unwrap();
    assert_eq!(mgr.usage("tB").unwrap().connections, 3);
}

// =====================================================================
//  Stress scenarios (2)
// =====================================================================

#[test]
fn test_stress_tenant_a_at_limit_tenant_b_normal() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota(
        "tA",
        TenantQuota::new()
            .with_max_connections(1)
            .with_max_storage_bytes(100),
    );
    mgr.set_quota(
        "tB",
        TenantQuota::new()
            .with_max_connections(10)
            .with_max_storage_bytes(10000),
    );

    // tA: 1 connection + 100 bytes → at limit
    mgr.try_acquire_connection("tA").unwrap();
    mgr.try_consume_storage("tA", 100).unwrap();

    // tA: new connection rejected
    assert!(mgr.try_acquire_connection("tA").is_err());
    // tA: more storage rejected
    assert!(mgr.try_consume_storage("tA", 1).is_err());

    // tB: operates normally
    mgr.try_acquire_connection("tB").unwrap();
    mgr.try_acquire_connection("tB").unwrap();
    mgr.try_consume_storage("tB", 5000).unwrap();
    assert_eq!(mgr.usage("tB").unwrap().connections, 2);
    assert_eq!(mgr.usage("tB").unwrap().storage_bytes, 5000);
}

#[test]
fn test_stress_tenant_a_blocked_on_storage_tenant_b_insert_works() {
    let mut mgr = QuotaManager::new();
    // tA: tiny storage quota (simulating near-full tenant)
    mgr.set_quota("tA", TenantQuota::new().with_max_storage_bytes(50));
    mgr.set_quota("tB", TenantQuota::new().with_max_storage_bytes(1_000_000));

    // tA: 40 bytes used, INSERT of 20 more bytes rejected (40 + 20 = 60 > 50)
    mgr.try_consume_storage("tA", 40).unwrap();
    assert!(mgr.try_consume_storage("tA", 20).is_err());

    // tB: INSERT 500KB succeeds
    mgr.try_consume_storage("tB", 500_000).unwrap();
    assert_eq!(mgr.usage("tB").unwrap().storage_bytes, 500_000);
}

// =====================================================================
//  Reset / remove (2)
// =====================================================================

#[test]
fn test_reset_usage_clears_counters() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota(
        "t1",
        TenantQuota::new()
            .with_max_connections(5)
            .with_max_storage_bytes(1024)
            .with_max_tables(3),
    );

    mgr.try_acquire_connection("t1").unwrap();
    mgr.try_acquire_connection("t1").unwrap();
    mgr.try_consume_storage("t1", 500).unwrap();
    mgr.try_create_table("t1").unwrap();

    assert_eq!(mgr.usage("t1").unwrap().connections, 2);
    assert_eq!(mgr.usage("t1").unwrap().storage_bytes, 500);
    assert_eq!(mgr.usage("t1").unwrap().tables, 1);

    mgr.reset_usage("t1");

    assert_eq!(mgr.usage("t1").unwrap().connections, 0);
    assert_eq!(mgr.usage("t1").unwrap().storage_bytes, 0);
    assert_eq!(mgr.usage("t1").unwrap().tables, 0);

    // Quota is preserved after reset
    assert_eq!(mgr.quota("t1").unwrap().max_connections, Some(5));
}

#[test]
fn test_remove_tenant_unregisters() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota("t1", TenantQuota::new().with_max_connections(5));
    mgr.try_acquire_connection("t1").unwrap();

    let removed = mgr.remove_tenant("t1");
    assert!(removed.is_some());
    assert_eq!(removed.unwrap().0.max_connections, Some(5));

    assert!(!mgr.is_registered("t1"));
    assert!(mgr.quota("t1").is_none());
    assert!(mgr.usage("t1").is_none());
}

// =====================================================================
//  Saturating release (1)
// =====================================================================

#[test]
fn test_release_saturates_at_zero() {
    let mut mgr = QuotaManager::new();
    mgr.set_quota(
        "t1",
        TenantQuota::new()
            .with_max_connections(5)
            .with_max_storage_bytes(1024)
            .with_max_tables(5),
    );

    // Release without any prior acquire — saturates at 0
    mgr.release_connection("t1");
    mgr.release_storage("t1", 100);
    mgr.release_table("t1");

    assert_eq!(mgr.usage("t1").unwrap().connections, 0);
    assert_eq!(mgr.usage("t1").unwrap().storage_bytes, 0);
    assert_eq!(mgr.usage("t1").unwrap().tables, 0);

    // Acquire 1, then release 5 — saturates at 0
    mgr.try_acquire_connection("t1").unwrap();
    mgr.release_connection("t1");
    mgr.release_connection("t1");
    mgr.release_connection("t1");
    assert_eq!(mgr.usage("t1").unwrap().connections, 0);

    // Release on unregistered tenant — no-op (no panic)
    mgr.release_connection("unregistered");
    mgr.release_storage("unregistered", 100);
    mgr.release_table("unregistered");
}
