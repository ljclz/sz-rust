#![cfg(feature = "redis-store")]

use sz_rust_auth_facade::redis_store::{RedisConfig, TlsConfig, TlsConfigError};

#[test]
fn test_rediss_url_enables_tls() {
    let config = RedisConfig::from_url("rediss://127.0.0.1:6379");
    assert!(config.is_tls_enabled());
}

#[test]
fn test_redis_url_without_tls_flag() {
    let config = RedisConfig::from_url("redis://127.0.0.1:6379");
    assert!(!config.is_tls_enabled());
}

#[test]
fn test_enable_tls_flag_overrides_url() {
    let mut config = RedisConfig::from_url("redis://127.0.0.1:6379");
    config.enable_tls = true;
    assert!(config.is_tls_enabled());
}

#[test]
fn test_validate_production_tls_rejects_plaintext() {
    let config = RedisConfig::from_url("redis://127.0.0.1:6379");
    let result = config.validate_production_tls("production");
    assert!(matches!(result, Err(TlsConfigError::RedisTlsRequired)));
}

#[test]
fn test_validate_production_tls_accepts_tls_url() {
    let config = RedisConfig::from_url("rediss://127.0.0.1:6379");
    let result = config.validate_production_tls("production");
    assert!(result.is_ok());
}

#[test]
fn test_validate_production_tls_non_production_allows_plaintext() {
    let config = RedisConfig::from_url("redis://127.0.0.1:6379");
    let result = config.validate_production_tls("development");
    assert!(result.is_ok());
}

#[test]
fn test_validate_production_tls_rejects_accept_invalid() {
    let config = RedisConfig {
        url: "rediss://127.0.0.1:6379".to_string(),
        enable_tls: true,
        tls_config: Some(TlsConfig {
            ca_cert_path: "/tmp/ca.pem".to_string(),
            client_cert_path: None,
            client_key_path: None,
            sni: None,
            accept_invalid_cert: true,
        }),
        ..Default::default()
    };
    let result = config.validate_production_tls("production");
    assert!(matches!(
        result,
        Err(TlsConfigError::AcceptInvalidForbiddenInProduction)
    ));
}

#[test]
fn test_tls_config_from_env_returns_none_when_no_ca_cert() {
    std::env::remove_var("SZ300_REDIS_CA_CERT_PATH");
    let config = TlsConfig::from_env();
    assert!(config.is_none());
}

#[test]
fn test_tls_config_from_env_returns_some_when_ca_cert_set() {
    std::env::set_var("SZ300_REDIS_CA_CERT_PATH", "/tmp/ca.pem");
    std::env::set_var("SZ300_REDIS_SNI", "redis.example.com");
    let config = TlsConfig::from_env();
    assert!(config.is_some());
    let config = config.unwrap();
    assert_eq!(config.ca_cert_path, "/tmp/ca.pem");
    assert_eq!(config.sni.as_deref(), Some("redis.example.com"));
    std::env::remove_var("SZ300_REDIS_CA_CERT_PATH");
    std::env::remove_var("SZ300_REDIS_SNI");
}

#[test]
fn test_redis_config_debug_redacts_url_password() {
    let config = RedisConfig::from_url("redis://:secret_password@127.0.0.1:6379");
    let debug_output = format!("{:?}", config);
    assert!(
        !debug_output.contains("secret_password"),
        "Debug should not contain password: {}",
        debug_output
    );
}

#[tokio::test]
async fn test_validate_ca_cert_rejects_invalid_pem() {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "this is not a PEM file").unwrap();
    let path = tmp.path().to_str().unwrap().to_string();

    let config = TlsConfig {
        ca_cert_path: path,
        client_cert_path: None,
        client_key_path: None,
        sni: None,
        accept_invalid_cert: false,
    };
    let result = config.validate_ca_cert().await;
    assert!(matches!(result, Err(TlsConfigError::TlsCertInvalid(_))));
}
