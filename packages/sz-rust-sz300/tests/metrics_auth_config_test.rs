mod common;
use common::EnvGuard;
use sz_rust_sz300::config::MetricsAuthConfig;

const MA_VARS: &[&str] = &[
    "SZ300_METRICS_ALLOWED_IPS",
    "SZ300_METRICS_BEARER_TOKEN",
    "SZ300_METRICS_AUTH_ENABLED",
];

#[test]
fn test_metrics_auth_default_enabled() {
    let config = MetricsAuthConfig::default();
    assert!(config.enabled);
    assert!(config.allowed_ips.is_empty());
    assert!(config.bearer_token.is_none());
}

#[test]
fn test_metrics_auth_from_env_default() {
    let _g = EnvGuard::clean(MA_VARS);
    let config = MetricsAuthConfig::from_env();
    assert!(config.enabled);
    assert!(config.allowed_ips.is_empty());
    assert!(config.bearer_token.is_none());
}

#[test]
fn test_metrics_auth_from_env_with_token() {
    let _g = EnvGuard::set("SZ300_METRICS_BEARER_TOKEN", "my-secret-token");
    let config = MetricsAuthConfig::from_env();
    assert_eq!(config.bearer_token.as_deref(), Some("my-secret-token"));
}

#[test]
fn test_metrics_auth_from_env_with_ips() {
    let _g = EnvGuard::set(
        "SZ300_METRICS_ALLOWED_IPS",
        "10.0.0.1,10.0.0.2,192.168.1.0/24",
    );
    let config = MetricsAuthConfig::from_env();
    assert_eq!(config.allowed_ips.len(), 3);
    assert!(config.allowed_ips.contains(&"10.0.0.1".to_string()));
    assert!(config.allowed_ips.contains(&"10.0.0.2".to_string()));
}

#[test]
fn test_metrics_auth_is_allowed_with_correct_token() {
    let config = MetricsAuthConfig {
        bearer_token: Some("secret".to_string()),
        ..Default::default()
    };
    assert!(config.is_allowed(Some("Bearer secret"), None));
}

#[test]
fn test_metrics_auth_is_allowed_with_wrong_token() {
    let config = MetricsAuthConfig {
        bearer_token: Some("secret".to_string()),
        ..Default::default()
    };
    assert!(!config.is_allowed(Some("Bearer wrong"), None));
}

#[test]
fn test_metrics_auth_is_allowed_with_no_token() {
    let config = MetricsAuthConfig {
        bearer_token: Some("secret".to_string()),
        ..Default::default()
    };
    assert!(!config.is_allowed(None, None));
}

#[test]
fn test_metrics_auth_is_allowed_with_ip_whitelist() {
    let config = MetricsAuthConfig {
        allowed_ips: vec!["10.0.0.1".to_string()],
        ..Default::default()
    };
    assert!(config.is_allowed(None, Some("10.0.0.1")));
    assert!(!config.is_allowed(None, Some("10.0.0.2")));
}

#[test]
fn test_metrics_auth_is_allowed_with_cidr_ip_whitelist() {
    let config = MetricsAuthConfig {
        allowed_ips: vec!["10.0.0.0/8".to_string(), "192.168.1.0/24".to_string()],
        ..Default::default()
    };
    // CIDR 内放行
    assert!(config.is_allowed(None, Some("10.1.2.3")));
    assert!(config.is_allowed(None, Some("192.168.1.99")));
    // CIDR 外拒绝
    assert!(!config.is_allowed(None, Some("11.0.0.1")));
    assert!(!config.is_allowed(None, Some("192.168.2.1")));
}

#[test]
fn test_metrics_auth_is_allowed_with_ipv6_cidr_ip_whitelist() {
    let config = MetricsAuthConfig {
        allowed_ips: vec!["2001:db8::/32".to_string()],
        ..Default::default()
    };
    assert!(config.is_allowed(None, Some("2001:db8:1::1")));
    assert!(!config.is_allowed(None, Some("2001:db9::1")));
}

#[test]
fn test_metrics_auth_is_allowed_rejects_invalid_cidr() {
    let config = MetricsAuthConfig {
        allowed_ips: vec!["10.0.0.0/999".to_string(), "not-an-ip".to_string()],
        ..Default::default()
    };
    assert!(!config.is_allowed(None, Some("10.0.0.1")));
}

#[test]
fn test_metrics_auth_is_allowed_disabled() {
    let config = MetricsAuthConfig {
        enabled: false,
        ..Default::default()
    };
    assert!(config.is_allowed(None, None));
}

#[test]
fn test_metrics_auth_validate_production_rejects_no_auth() {
    let config = MetricsAuthConfig::default();
    let result = config.validate_production("production");
    assert!(result.is_err());
}

#[test]
fn test_metrics_auth_validate_production_accepts_with_token() {
    let config = MetricsAuthConfig {
        bearer_token: Some("secret".to_string()),
        ..Default::default()
    };
    let result = config.validate_production("production");
    assert!(result.is_ok());
}

#[test]
fn test_metrics_auth_validate_production_non_production_allows_no_auth() {
    let config = MetricsAuthConfig::default();
    let result = config.validate_production("development");
    assert!(result.is_ok());
}

#[test]
fn test_metrics_auth_debug_redacts_bearer_token() {
    let config = MetricsAuthConfig {
        bearer_token: Some("super-secret-token".to_string()),
        ..Default::default()
    };
    let debug_output = format!("{:?}", config);
    assert!(
        !debug_output.contains("super-secret-token"),
        "Debug should not contain token: {debug_output}"
    );
    assert!(debug_output.contains("[REDACTED]"));
}
