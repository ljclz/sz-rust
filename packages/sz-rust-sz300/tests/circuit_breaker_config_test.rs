mod common;
use common::EnvGuard;
use std::time::Duration;
use sz_rust_sz300::config::CircuitBreakerProductionConfig;

const CB_VARS: &[&str] = &[
    "SZ300_CIRCUIT_BREAKER_THRESHOLD",
    "SZ300_CIRCUIT_BREAKER_COOLDOWN",
    "SZ300_CIRCUIT_BREAKER_PROBE_REQUESTS",
    "SZ300_CIRCUIT_BREAKER_STAT_WINDOW",
];

#[test]
fn test_circuit_breaker_default_config() {
    let config = CircuitBreakerProductionConfig::default();
    assert_eq!(config.error_threshold, 0.5);
    assert_eq!(config.cooldown, Duration::from_secs(10));
    assert_eq!(config.probe_requests, 5);
    assert_eq!(config.stat_window, Duration::from_secs(60));
}

#[test]
fn test_circuit_breaker_from_env_default() {
    let _g = EnvGuard::clean(CB_VARS);
    let config = CircuitBreakerProductionConfig::from_env();
    assert_eq!(config.error_threshold, 0.5);
    assert_eq!(config.cooldown, Duration::from_secs(10));
}

#[test]
fn test_circuit_breaker_from_env_custom() {
    let _g1 = EnvGuard::set("SZ300_CIRCUIT_BREAKER_THRESHOLD", "0.3");
    let _g2 = EnvGuard::set("SZ300_CIRCUIT_BREAKER_COOLDOWN", "20");
    let _g3 = EnvGuard::set("SZ300_CIRCUIT_BREAKER_PROBE_REQUESTS", "10");
    let _g4 = EnvGuard::set("SZ300_CIRCUIT_BREAKER_STAT_WINDOW", "120");
    let config = CircuitBreakerProductionConfig::from_env();
    assert_eq!(config.error_threshold, 0.3);
    assert_eq!(config.cooldown, Duration::from_secs(20));
    assert_eq!(config.probe_requests, 10);
    assert_eq!(config.stat_window, Duration::from_secs(120));
}

#[test]
fn test_circuit_breaker_validate_valid() {
    let config = CircuitBreakerProductionConfig::default();
    assert!(config.validate().is_ok());
}

#[test]
fn test_circuit_breaker_validate_rejects_zero_threshold() {
    let config = CircuitBreakerProductionConfig {
        error_threshold: 0.0,
        ..Default::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_circuit_breaker_validate_rejects_threshold_above_one() {
    let config = CircuitBreakerProductionConfig {
        error_threshold: 1.5,
        ..Default::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_circuit_breaker_validate_rejects_zero_cooldown() {
    let config = CircuitBreakerProductionConfig {
        cooldown: Duration::ZERO,
        ..Default::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_circuit_breaker_validate_rejects_zero_probe_requests() {
    let config = CircuitBreakerProductionConfig {
        probe_requests: 0,
        ..Default::default()
    };
    assert!(config.validate().is_err());
}
