// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use sz_rust_sz300::config::RateLimitProductionConfig;

#[test]
fn test_rate_limit_default_config() {
    let config = RateLimitProductionConfig::default();
    assert_eq!(config.capacity, 2000);
    assert_eq!(config.refill_per_second, 1000.0);
    assert!(config.exclude_paths.contains(&"/health".to_string()));
    assert!(config.exclude_paths.contains(&"/health/ready".to_string()));
    assert!(config.exclude_paths.contains(&"/health/startup".to_string()));
    assert!(config.exclude_paths.contains(&"/metrics".to_string()));
}

#[test]
fn test_rate_limit_from_env_sequential() {
    // 测试默认�?
    std::env::remove_var("SZ300_RATE_LIMIT_CAPACITY");
    std::env::remove_var("SZ300_RATE_LIMIT_REFILL");
    let config = RateLimitProductionConfig::from_env();
    assert_eq!(config.capacity, 2000);
    assert_eq!(config.refill_per_second, 1000.0);

    // 测试自定义�?
    std::env::set_var("SZ300_RATE_LIMIT_CAPACITY", "5000");
    std::env::set_var("SZ300_RATE_LIMIT_REFILL", "2500.5");
    let config = RateLimitProductionConfig::from_env();
    assert_eq!(config.capacity, 5000);
    assert_eq!(config.refill_per_second, 2500.5);

    // 清理
    std::env::remove_var("SZ300_RATE_LIMIT_CAPACITY");
    std::env::remove_var("SZ300_RATE_LIMIT_REFILL");
}
