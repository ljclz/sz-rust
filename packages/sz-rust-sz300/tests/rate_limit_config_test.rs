mod common;
use common::EnvGuard;
use sz_rust_sz300::config::RateLimitProductionConfig;

const RL_VARS: &[&str] = &["SZ300_RATE_LIMIT_CAPACITY", "SZ300_RATE_LIMIT_REFILL"];

#[test]
fn test_rate_limit_default_config() {
    let config = RateLimitProductionConfig::default();
    assert_eq!(config.capacity, 2000);
    assert_eq!(config.refill_per_second, 1000.0);
    assert!(config.exclude_paths.contains(&"/health".to_string()));
    assert!(config.exclude_paths.contains(&"/health/ready".to_string()));
    assert!(config
        .exclude_paths
        .contains(&"/health/startup".to_string()));
    assert!(config.exclude_paths.contains(&"/metrics".to_string()));
}

#[test]
fn test_rate_limit_from_env_default() {
    let _g = EnvGuard::clean(RL_VARS);
    let config = RateLimitProductionConfig::from_env();
    assert_eq!(config.capacity, 2000);
    assert_eq!(config.refill_per_second, 1000.0);
}

#[test]
fn test_rate_limit_from_env_custom() {
    let _g1 = EnvGuard::set("SZ300_RATE_LIMIT_CAPACITY", "5000");
    let _g2 = EnvGuard::set("SZ300_RATE_LIMIT_REFILL", "2500.5");
    let config = RateLimitProductionConfig::from_env();
    assert_eq!(config.capacity, 5000);
    assert_eq!(config.refill_per_second, 2500.5);
}
