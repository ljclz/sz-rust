// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::time::Duration;
use sz_rust_cache_facade::RedisConfig;

#[test]
fn test_redis_config_debug_redacts_password() {
    let config = RedisConfig {
        host: "127.0.0.1".to_string(),
        port: 6379,
        password: "redis_secret_password_789".to_string(),
        select: 0,
        timeout: Duration::ZERO,
        expire: None,
        persistent: false,
        prefix: String::new(),
        tag_prefix: "tag:".to_string(),
        enable_tls: false,
        tls_ca_cert_path: None,
    };

    let debug_output = format!("{:?}", config);
    assert!(
        !debug_output.contains("redis_secret_password_789"),
        "Debug output should not contain password value: {}",
        debug_output
    );
    assert!(
        debug_output.contains("[REDACTED]"),
        "Debug output should contain [REDACTED]: {}",
        debug_output
    );
}

#[test]
fn test_redis_config_debug_preserves_other_fields() {
    let config = RedisConfig {
        host: "192.168.1.200".to_string(),
        port: 6380,
        password: "hidden".to_string(),
        select: 2,
        timeout: Duration::from_secs(3),
        expire: Some(Duration::from_secs(3600)),
        persistent: true,
        prefix: "myapp:".to_string(),
        tag_prefix: "mytag:".to_string(),
        enable_tls: true,
        tls_ca_cert_path: Some("/etc/ssl/redis-ca.pem".to_string()),
    };

    let debug_output = format!("{:?}", config);
    assert!(
        debug_output.contains("192.168.1.200"),
        "host should be visible"
    );
    assert!(debug_output.contains("6380"), "port should be visible");
    assert!(debug_output.contains("myapp:"), "prefix should be visible");
    assert!(
        debug_output.contains("mytag:"),
        "tag_prefix should be visible"
    );
    assert!(
        !debug_output.contains("hidden"),
        "password should be redacted: {}",
        debug_output
    );
}

#[test]
fn test_redis_config_default_redacts_empty_password() {
    let config = RedisConfig::default();
    let debug_output = format!("{:?}", config);
    assert!(
        debug_output.contains("[REDACTED]"),
        "Even empty password should show [REDACTED]: {}",
        debug_output
    );
}
