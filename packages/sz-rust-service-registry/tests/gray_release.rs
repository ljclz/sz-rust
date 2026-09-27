// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P3-2 灰度发布集成测试

#![cfg(feature = "gray-release")]

use std::collections::HashMap;

use sz_rust_service_registry::{
    GrayRelease, GrayReleaseConfig, HealthStats, InstanceStatus, MetadataFilter, RollbackDecision,
    ServiceInstance,
};

fn make_instance(id: &str, release: Option<&str>, status: InstanceStatus) -> ServiceInstance {
    let mut inst = ServiceInstance::new("svc", "127.0.0.1", 8080);
    inst.instance_id = id.to_string();
    if let Some(r) = release {
        inst.metadata.insert("release".to_string(), r.to_string());
    }
    inst.status = status;
    inst
}

#[test]
fn test_gray_release_10_percent_to_new_version() {
    let instances = vec![
        make_instance("old-1", None, InstanceStatus::Healthy),
        make_instance("old-2", None, InstanceStatus::Healthy),
        make_instance("canary-1", Some("canary"), InstanceStatus::Healthy),
    ];
    let cfg = GrayReleaseConfig::new(0.1, 0.5, 3);
    let gr = GrayRelease::new(cfg);
    let routed = gr.route(&instances);

    assert_eq!(routed.len(), 3, "all healthy instances routed");
    let canary = routed.iter().find(|i| i.instance_id == "canary-1").unwrap();
    let old = routed.iter().find(|i| i.instance_id == "old-1").unwrap();
    assert!(canary.weight <= old.weight, "canary weight should be lower");
}

#[test]
fn test_gray_release_excludes_unhealthy() {
    let instances = vec![
        make_instance("old-1", None, InstanceStatus::Healthy),
        make_instance("old-2", None, InstanceStatus::Unhealthy),
        make_instance("canary-1", Some("canary"), InstanceStatus::Maintenance),
    ];
    let gr = GrayRelease::new(GrayReleaseConfig::default());
    let routed = gr.route(&instances);
    assert_eq!(routed.len(), 1, "only healthy instance routed");
    assert_eq!(routed[0].instance_id, "old-1");
}

#[test]
fn test_gray_release_rollback_on_high_failure_rate() {
    let gr = GrayRelease::new(GrayReleaseConfig::new(0.1, 0.5, 3));
    let stats = HealthStats {
        failure_rate: 0.7,
        consecutive_failures: 5,
    };
    let decision = gr.check_rollback(&stats);
    assert!(matches!(decision, RollbackDecision::Rollback { .. }));
}

#[test]
fn test_gray_release_no_rollback_on_low_failure_rate() {
    let gr = GrayRelease::new(GrayReleaseConfig::new(0.1, 0.5, 3));
    let stats = HealthStats {
        failure_rate: 0.2,
        consecutive_failures: 1,
    };
    assert_eq!(gr.check_rollback(&stats), RollbackDecision::Continue);
}

#[test]
fn test_gray_release_no_rollback_avoids_jitter() {
    let gr = GrayRelease::new(GrayReleaseConfig::new(0.1, 0.5, 3));
    let stats = HealthStats {
        failure_rate: 0.9,
        consecutive_failures: 2,
    };
    assert_eq!(
        gr.check_rollback(&stats),
        RollbackDecision::Continue,
        "high failure rate but low consecutive failures should not rollback"
    );
}

#[test]
fn test_metadata_filter_by_tag() {
    let mut inst1 = ServiceInstance::new("svc", "127.0.0.1", 8080);
    inst1
        .metadata
        .insert("region".to_string(), "us".to_string());
    let mut inst2 = ServiceInstance::new("svc", "127.0.0.2", 8080);
    inst2
        .metadata
        .insert("region".to_string(), "eu".to_string());

    let mut tags = HashMap::new();
    tags.insert("region".to_string(), "us".to_string());

    let filter = MetadataFilter::new();
    let result = filter.filter(&[inst1, inst2], &tags);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].host, "127.0.0.1");
}

#[test]
fn test_metadata_filter_multiple_tags() {
    let mut inst1 = ServiceInstance::new("svc", "127.0.0.1", 8080);
    inst1
        .metadata
        .insert("region".to_string(), "us".to_string());
    inst1
        .metadata
        .insert("version".to_string(), "2".to_string());
    let mut inst2 = ServiceInstance::new("svc", "127.0.0.2", 8080);
    inst2
        .metadata
        .insert("region".to_string(), "us".to_string());
    inst2
        .metadata
        .insert("version".to_string(), "1".to_string());

    let mut tags = HashMap::new();
    tags.insert("region".to_string(), "us".to_string());
    tags.insert("version".to_string(), "2".to_string());

    let filter = MetadataFilter::new();
    let result = filter.filter(&[inst1, inst2], &tags);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].host, "127.0.0.1");
}

#[test]
fn test_metadata_filter_excludes_unhealthy() {
    let mut inst = ServiceInstance::new("svc", "127.0.0.1", 8080);
    inst.metadata.insert("region".to_string(), "us".to_string());
    inst.status = InstanceStatus::Unhealthy;

    let mut tags = HashMap::new();
    tags.insert("region".to_string(), "us".to_string());

    let filter = MetadataFilter::new();
    let result = filter.filter(&[inst], &tags);
    assert!(result.is_empty(), "unhealthy instance should not be routed");
}

#[test]
fn test_gray_release_zero_weight_only_old() {
    let instances = vec![
        make_instance("old-1", None, InstanceStatus::Healthy),
        make_instance("canary-1", Some("canary"), InstanceStatus::Healthy),
    ];
    let cfg = GrayReleaseConfig::new(0.0, 0.5, 3);
    let gr = GrayRelease::new(cfg);
    let routed = gr.route(&instances);
    assert_eq!(routed.len(), 1);
    assert_eq!(routed[0].instance_id, "old-1");
}

#[test]
fn test_gray_release_full_weight_only_new() {
    let instances = vec![
        make_instance("old-1", None, InstanceStatus::Healthy),
        make_instance("canary-1", Some("canary"), InstanceStatus::Healthy),
    ];
    let cfg = GrayReleaseConfig::new(1.0, 0.5, 3);
    let gr = GrayRelease::new(cfg);
    let routed = gr.route(&instances);
    assert_eq!(routed.len(), 1);
    assert_eq!(routed[0].instance_id, "canary-1");
}

#[test]
fn test_gray_release_empty_instances() {
    let gr = GrayRelease::new(GrayReleaseConfig::default());
    let routed = gr.route(&[]);
    assert!(routed.is_empty());
}

#[test]
fn test_metadata_filter_empty_tags_all_healthy() {
    let instances = vec![
        make_instance("a", None, InstanceStatus::Healthy),
        make_instance("b", None, InstanceStatus::Healthy),
    ];
    let filter = MetadataFilter::new();
    let result = filter.filter(&instances, &HashMap::new());
    assert_eq!(result.len(), 2);
}
