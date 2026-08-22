mod common;
use common::EnvGuard;
use std::time::Duration;
use sz_rust_sz300::config::HealthCheckConfig;

const HC_VARS: &[&str] = &["SZ300_READINESS_CHECKS", "SZ300_HEALTH_CHECK_TIMEOUT"];

#[test]
fn test_health_check_default_config() {
    let config = HealthCheckConfig::default();
    assert_eq!(config.readiness_checks, vec!["db".to_string()]);
    assert_eq!(config.check_timeout, Duration::from_secs(2));
    assert!(!config.liveness_check_dependencies);
}

#[test]
fn test_health_check_from_env_default() {
    let _g = EnvGuard::clean(HC_VARS);
    let config = HealthCheckConfig::from_env();
    assert_eq!(config.readiness_checks, vec!["db".to_string()]);
    assert_eq!(config.check_timeout, Duration::from_secs(2));
}

#[test]
fn test_health_check_from_env_custom_checks() {
    let _g = EnvGuard::set("SZ300_READINESS_CHECKS", "db,redis,mqtt");
    let config = HealthCheckConfig::from_env();
    assert_eq!(config.readiness_checks.len(), 3);
    assert!(config.should_check("db"));
    assert!(config.should_check("redis"));
    assert!(config.should_check("mqtt"));
}

#[test]
fn test_health_check_from_env_filters_unknown_checks() {
    let _g = EnvGuard::set("SZ300_READINESS_CHECKS", "db,unknown,redis");
    let config = HealthCheckConfig::from_env();
    assert!(config.should_check("db"));
    assert!(config.should_check("redis"));
    assert!(!config.should_check("unknown"));
}

#[test]
fn test_health_check_from_env_custom_timeout() {
    let _g = EnvGuard::set("SZ300_HEALTH_CHECK_TIMEOUT", "5");
    let config = HealthCheckConfig::from_env();
    assert_eq!(config.check_timeout, Duration::from_secs(5));
}

#[test]
fn test_health_check_should_check() {
    let config = HealthCheckConfig {
        readiness_checks: vec!["db".into(), "redis".into()],
        ..Default::default()
    };
    assert!(config.should_check("db"));
    assert!(config.should_check("redis"));
    assert!(!config.should_check("mqtt"));
}

#[test]
fn test_health_check_from_env_empty_checks_falls_back_to_default() {
    let _g = EnvGuard::set("SZ300_READINESS_CHECKS", "");
    let config = HealthCheckConfig::from_env();
    assert_eq!(config.readiness_checks, vec!["db".to_string()]);
}
