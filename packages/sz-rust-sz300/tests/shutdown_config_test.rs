mod common;
use common::EnvGuard;
use std::time::Duration;
use sz_rust_sz300::config::ShutdownConfig;

const SD_VARS: &[&str] = &[
    "SZ300_SHUTDOWN_TIMEOUT",
    "SZ300_MQTT_SHUTDOWN_TIMEOUT",
    "SZ300_FORCE_ABORT_ON_TIMEOUT",
];

#[test]
fn test_shutdown_config_default_timeout_30s() {
    let config = ShutdownConfig::default();
    assert_eq!(config.shutdown_timeout, Duration::from_secs(30));
}

#[test]
fn test_shutdown_config_default_mqtt_timeout_falls_back_to_shutdown() {
    let config = ShutdownConfig::default();
    assert_eq!(config.mqtt_timeout(), Duration::from_secs(30));
}

#[test]
fn test_shutdown_config_mqtt_timeout_uses_explicit_value() {
    let config = ShutdownConfig {
        shutdown_timeout: Duration::from_secs(30),
        mqtt_shutdown_timeout: Some(Duration::from_secs(10)),
        force_abort_on_timeout: true,
    };
    assert_eq!(config.mqtt_timeout(), Duration::from_secs(10));
}

#[test]
fn test_shutdown_config_from_env_default() {
    let _g = EnvGuard::clean(SD_VARS);
    let config = ShutdownConfig::from_env();
    assert_eq!(config.shutdown_timeout, Duration::from_secs(30));
    assert!(config.mqtt_shutdown_timeout.is_none());
    assert!(config.force_abort_on_timeout);
}

#[test]
fn test_shutdown_config_from_env_custom_timeout() {
    let _g1 = EnvGuard::set("SZ300_SHUTDOWN_TIMEOUT", "60");
    let _g2 = EnvGuard::set("SZ300_MQTT_SHUTDOWN_TIMEOUT", "15");
    let config = ShutdownConfig::from_env();
    assert_eq!(config.shutdown_timeout, Duration::from_secs(60));
    assert_eq!(config.mqtt_timeout(), Duration::from_secs(15));
}

#[test]
fn test_shutdown_config_force_abort_false() {
    let _g = EnvGuard::set("SZ300_FORCE_ABORT_ON_TIMEOUT", "false");
    let config = ShutdownConfig::from_env();
    assert!(!config.force_abort_on_timeout);
}
