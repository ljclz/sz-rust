use sz_rust_infra_facade::config::{ConfigError, LogConfig};

#[test]
fn test_default_log_level_is_warn() {
    let config = LogConfig::default();
    assert!(
        config.level.starts_with("warn"),
        "default level should start with 'warn', got: {}",
        config.level
    );
}

#[test]
fn test_log_config_exclude_paths_contains_health_and_metrics() {
    let config = LogConfig::default();
    assert!(config.exclude_paths.contains(&"/health".to_string()));
    assert!(config.exclude_paths.contains(&"/metrics".to_string()));
}

#[test]
fn test_validate_production_rejects_debug() {
    let config = LogConfig {
        level: "debug".to_string(),
        ..Default::default()
    };
    let result = config.validate_production("production");
    assert!(matches!(
        result,
        Err(ConfigError::LogLevelForbiddenInProduction { .. })
    ));
}

#[test]
fn test_validate_production_rejects_trace() {
    let config = LogConfig {
        level: "trace".to_string(),
        ..Default::default()
    };
    let result = config.validate_production("production");
    assert!(matches!(
        result,
        Err(ConfigError::LogLevelForbiddenInProduction { .. })
    ));
}

#[test]
fn test_validate_production_accepts_warn() {
    let config = LogConfig {
        level: "warn".to_string(),
        ..Default::default()
    };
    let result = config.validate_production("production");
    assert!(result.is_ok());
}

#[test]
fn test_validate_production_accepts_error() {
    let config = LogConfig {
        level: "error".to_string(),
        ..Default::default()
    };
    let result = config.validate_production("production");
    assert!(result.is_ok());
}

#[test]
fn test_validate_production_non_production_allows_debug() {
    let config = LogConfig {
        level: "debug".to_string(),
        ..Default::default()
    };
    let result = config.validate_production("development");
    assert!(result.is_ok());
}

#[test]
fn test_validate_production_non_production_allows_empty_env() {
    let config = LogConfig {
        level: "trace".to_string(),
        ..Default::default()
    };
    let result = config.validate_production("");
    assert!(result.is_ok());
}
