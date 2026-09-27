// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P4-4 内存泄漏检测集成测试

#![cfg(feature = "leak-detect")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sz_rust_observability::{
    DropCounter, GrowthTrend, LeakDetectConfig, LeakDetector, ResourceKind, TrackedResource,
};

#[tokio::test]
async fn test_connection_pool_leak_detected() {
    let cfg = LeakDetectConfig::new(
        Duration::from_millis(300),
        Duration::from_millis(50),
        0.1,
        100,
    );
    let mut detector = LeakDetector::new(cfg);

    let report = detector
        .detect(|idx| async move {
            let mut counts = HashMap::new();
            counts.insert(ResourceKind::ConnectionPool, 5 + idx as usize);
            counts
        })
        .await;

    assert!(report.leaked, "should detect connection pool leak");
    assert_eq!(report.trend, GrowthTrend::MonotonicGrowth);
    assert!(report
        .root_cause
        .as_deref()
        .unwrap_or("")
        .contains("connection"));
    assert!(report
        .resources
        .iter()
        .any(|r| r.kind == ResourceKind::ConnectionPool));
}

#[tokio::test]
async fn test_async_task_leak_detected() {
    let cfg = LeakDetectConfig::new(
        Duration::from_millis(300),
        Duration::from_millis(50),
        0.1,
        100,
    );
    let mut detector = LeakDetector::new(cfg);

    let report = detector
        .detect(|idx| async move {
            let mut counts = HashMap::new();
            counts.insert(ResourceKind::AsyncTask, 2 + idx as usize * 2);
            counts
        })
        .await;

    assert!(report.leaked, "should detect async task leak");
    assert!(report.root_cause.as_deref().unwrap_or("").contains("task"));
}

#[tokio::test]
async fn test_cache_leak_detected() {
    let cfg = LeakDetectConfig::new(
        Duration::from_millis(300),
        Duration::from_millis(50),
        0.1,
        100,
    );
    let mut detector = LeakDetector::new(cfg);

    let report = detector
        .detect(|idx| async move {
            let mut counts = HashMap::new();
            counts.insert(ResourceKind::CacheEntry, 10 + idx as usize * 3);
            counts
        })
        .await;

    assert!(report.leaked, "should detect cache leak");
    assert!(report.root_cause.as_deref().unwrap_or("").contains("cache"));
}

#[tokio::test]
async fn test_no_leak_stable_resources() {
    let cfg = LeakDetectConfig::new(
        Duration::from_millis(200),
        Duration::from_millis(50),
        0.1,
        100,
    );
    let mut detector = LeakDetector::new(cfg);

    let report = detector
        .detect(|_| async move {
            let mut counts = HashMap::new();
            counts.insert(ResourceKind::ConnectionPool, 5);
            counts.insert(ResourceKind::AsyncTask, 3);
            counts.insert(ResourceKind::CacheEntry, 10);
            counts
        })
        .await;

    assert!(
        !report.leaked,
        "stable resources should not be flagged as leak"
    );
    assert_eq!(report.trend, GrowthTrend::Stable);
}

#[tokio::test]
async fn test_cleanup_no_temp_files_remaining() {
    let temp_dir = std::env::temp_dir().join("sz_rust_leak_integration_test");
    tokio::fs::create_dir_all(&temp_dir).await.unwrap();

    for i in 0..5 {
        let path = temp_dir.join(format!("tmp_{i}.txt"));
        tokio::fs::write(&path, "leak test data").await.unwrap();
    }

    let detector = LeakDetector::new(LeakDetectConfig::default());
    let result = detector.cleanup(temp_dir.to_str().unwrap()).await;
    assert!(result.is_ok());

    let mut remaining = 0;
    let mut entries = tokio::fs::read_dir(&temp_dir).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        if entry.path().is_file() {
            remaining += 1;
        }
    }
    assert_eq!(remaining, 0, "no temp files should remain");

    let _ = tokio::fs::remove_dir(&temp_dir).await;
}

#[tokio::test]
async fn test_drop_counter_tracks_leak() {
    let counter = Arc::new(DropCounter::new("test_resource"));

    let leaked = TrackedResource::new(42, counter.clone());
    assert_eq!(counter.created(), 1);
    assert_eq!(counter.dropped(), 0);
    assert!(counter.has_leak());

    drop(leaked);
    assert_eq!(counter.dropped(), 1);
    assert!(!counter.has_leak());
}

#[tokio::test]
async fn test_drop_counter_multiple_resources() {
    let counter = Arc::new(DropCounter::new("multi"));

    let resources: Vec<_> = (0..10)
        .map(|i| TrackedResource::new(i, counter.clone()))
        .collect();
    assert_eq!(counter.created(), 10);
    assert_eq!(counter.leaked(), 10);

    drop(resources);
    assert_eq!(counter.dropped(), 10);
    assert!(!counter.has_leak());
}

#[tokio::test]
async fn test_fluctuating_not_flagged_as_leak() {
    let cfg = LeakDetectConfig::new(
        Duration::from_millis(300),
        Duration::from_millis(50),
        0.1,
        100,
    );
    let mut detector = LeakDetector::new(cfg);
    let counter = Arc::new(AtomicUsize::new(0));

    let report = detector
        .detect(|_| {
            let c = counter.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                let mut counts = HashMap::new();
                counts.insert(ResourceKind::ConnectionPool, 5 + (n % 3));
                counts
            }
        })
        .await;

    assert!(
        !report.leaked,
        "fluctuating resources should not be flagged"
    );
}
