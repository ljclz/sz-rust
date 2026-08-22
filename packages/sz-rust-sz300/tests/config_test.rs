//! 配置加载集成测试
//!
//! 验证 `sz_rust_sz300::config` 模块从环境变量加载配置的行为。
//! 不依赖真实数据库连接，仅测试配置解析逻辑。
//!
//! ## 测试隔离
//!
//! 涉及 `std::env::set_var` / `std::env::remove_var` 的测试通过
//! `ENV_TEST_LOCK` 互斥锁串行运行，避免并行测试间的环境变量竞争。

mod common;
use common::EnvGuard;
use sz_rust_sz300::config;

#[test]
fn test_load_config_requires_password() {
    let _g = EnvGuard::clean(&["SZ300_DB_PASSWORD"]);
    let result = config::load_config();
    assert!(result.is_err());
}

#[test]
fn test_load_config_with_env() {
    let _g = EnvGuard::set("SZ300_DB_PASSWORD", "test_password");
    let cfg = config::load_config().expect("load_config with DB_PASSWORD should succeed");
    assert_eq!(cfg.database.password, "test_password");
    assert_eq!(cfg.server.host, "0.0.0.0");
    assert_eq!(cfg.server.port, 8300);
}

#[test]
fn test_load_config_db_defaults() {
    let _g = EnvGuard::set("SZ300_DB_PASSWORD", "pw");
    let cfg = config::load_config().expect("load_config with DB_PASSWORD should succeed");
    assert_eq!(cfg.database.host, "127.0.0.1");
    assert_eq!(cfg.database.port, 3306);
    assert_eq!(cfg.database.database, "sz300");
    assert_eq!(cfg.database.username, "root");
}

#[test]
fn test_pg_config_requires_password() {
    let _g = EnvGuard::clean(&["SZ300_PG_PASSWORD"]);
    let result = config::pg_config();
    assert!(result.is_err());
}

#[test]
fn test_pg_config_with_env() {
    let _g = EnvGuard::set("SZ300_PG_PASSWORD", "pg_test");
    let cfg = config::pg_config().expect("pg_config with PG_PASSWORD should succeed");
    assert_eq!(cfg.password, "pg_test");
    assert_eq!(cfg.host, "127.0.0.1");
    assert_eq!(cfg.port, 5432);
}

#[test]
fn test_pg_config_defaults() {
    let _g = EnvGuard::set("SZ300_PG_PASSWORD", "pw");
    let cfg = config::pg_config().expect("pg_config with PG_PASSWORD should succeed");
    assert_eq!(cfg.database, "sz300");
    assert_eq!(cfg.username, "postgres");
}
