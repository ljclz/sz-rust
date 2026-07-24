//! 配置加载集成测试
//!
//! 验证 `sz_rust_sz300::config` 模块从环境变量加载配置的行为。
//! 不依赖真实数据库连接，仅测试配置解析逻辑。
//!
//! ## 测试隔离
//!
//! 涉及 `std::env::set_var` / `std::env::remove_var` 的测试通过
//! `ENV_TEST_LOCK` 互斥锁串行运行，避免并行测试间的环境变量竞争。

use sz_rust_sz300::config;

/// env 测试互斥锁 — 确保所有修改环境变量的测试串行运行
static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_load_config_requires_password() {
    let _env_guard = ENV_TEST_LOCK.lock().unwrap();
    // 移除环境变量后应返回错误
    std::env::remove_var("SZ300_DB_PASSWORD");
    let result = config::load_config();
    assert!(result.is_err());
}

#[test]
fn test_load_config_with_env() {
    let _env_guard = ENV_TEST_LOCK.lock().unwrap();
    std::env::set_var("SZ300_DB_PASSWORD", "test_password");
    let cfg = config::load_config().unwrap();
    assert_eq!(cfg.database.password, "test_password");
    assert_eq!(cfg.server.host, "0.0.0.0");
    assert_eq!(cfg.server.port, 8300);
    // 清理
    std::env::remove_var("SZ300_DB_PASSWORD");
}

#[test]
fn test_load_config_db_defaults() {
    let _env_guard = ENV_TEST_LOCK.lock().unwrap();
    std::env::set_var("SZ300_DB_PASSWORD", "pw");
    let cfg = config::load_config().unwrap();
    // 验证默认值
    assert_eq!(cfg.database.host, "127.0.0.1");
    assert_eq!(cfg.database.port, 3306);
    assert_eq!(cfg.database.database, "sz300");
    assert_eq!(cfg.database.username, "root");
    std::env::remove_var("SZ300_DB_PASSWORD");
}

#[test]
fn test_pg_config_requires_password() {
    let _env_guard = ENV_TEST_LOCK.lock().unwrap();
    std::env::remove_var("SZ300_PG_PASSWORD");
    let result = config::pg_config();
    assert!(result.is_err());
}

#[test]
fn test_pg_config_with_env() {
    let _env_guard = ENV_TEST_LOCK.lock().unwrap();
    std::env::set_var("SZ300_PG_PASSWORD", "pg_test");
    let cfg = config::pg_config().unwrap();
    assert_eq!(cfg.password, "pg_test");
    assert_eq!(cfg.host, "127.0.0.1");
    assert_eq!(cfg.port, 5432);
    std::env::remove_var("SZ300_PG_PASSWORD");
}

#[test]
fn test_pg_config_defaults() {
    let _env_guard = ENV_TEST_LOCK.lock().unwrap();
    std::env::set_var("SZ300_PG_PASSWORD", "pw");
    let cfg = config::pg_config().unwrap();
    assert_eq!(cfg.database, "sz300");
    assert_eq!(cfg.username, "postgres");
    std::env::remove_var("SZ300_PG_PASSWORD");
}
