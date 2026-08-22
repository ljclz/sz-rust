use sz_rust_sz300::config::{DatabaseConfig, PgDatabaseConfig};

#[test]
fn test_database_config_debug_redacts_password() {
    let config = DatabaseConfig {
        host: "127.0.0.1".to_string(),
        port: 3306,
        database: "sz300".to_string(),
        username: "root".to_string(),
        password: "super_secret_password_123".to_string(),
    };

    let debug_output = format!("{:?}", config);
    assert!(
        !debug_output.contains("super_secret_password_123"),
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
fn test_pg_database_config_debug_redacts_password() {
    let config = PgDatabaseConfig {
        host: "127.0.0.1".to_string(),
        port: 5432,
        database: "sz300".to_string(),
        username: "root".to_string(),
        password: "pg_secret_password_456".to_string(),
    };

    let debug_output = format!("{:?}", config);
    assert!(
        !debug_output.contains("pg_secret_password_456"),
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
fn test_database_config_debug_preserves_other_fields() {
    let config = DatabaseConfig {
        host: "192.168.1.100".to_string(),
        port: 3307,
        database: "testdb".to_string(),
        username: "admin".to_string(),
        password: "hidden".to_string(),
    };

    let debug_output = format!("{:?}", config);
    assert!(
        debug_output.contains("192.168.1.100"),
        "host should be visible"
    );
    assert!(debug_output.contains("3307"), "port should be visible");
    assert!(
        debug_output.contains("testdb"),
        "database should be visible"
    );
    assert!(debug_output.contains("admin"), "username should be visible");
}
