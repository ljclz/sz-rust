//! App 容器 — DB/Cache/Log 单例
//!
//! 对齐 PHP `app()` 容器，持有全局配置和单例。
//!
//! ## 设计
//!
//! - 基于 `OnceCell` 实现全局单例（线程安全，初始化一次后只读）
//! - Phase 0：持有 `AppConfig` + 5 个 DB 连接配置 + Cache/Log 占位
//! - Phase 1+：接入 SZ-ORM `Pool`，替换 `DatabaseConnection` 为真正的连接池
//! - Phase 6：接入 Cache facade
//! - Phase 0.7：接入日志系统
//!
//! ## PHP 对齐
//!
//! ```php
//! // PHP 中的 app() 容器
//! $app = app();
//! $db = $app->db;  // 数据库连接
//! $cache = $app->cache;  // 缓存
//! $log = $app->log;  // 日志
//! ```

use crate::config::{AppConfig, DatabaseConnection};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

/// 全局 App 容器单例
static APP: OnceLock<App> = OnceLock::new();

/// App 容器（全局单例）
///
/// 持有应用配置和各子系统单例。通过 [`App::instance()`] 获取全局实例，
/// 通过 [`App::init()`] 初始化。
pub struct App {
    /// 应用配置（只读，初始化后不可变）
    config: AppConfig,
    /// 数据库连接配置（5 个：mysql/njszjt/ljclz/food/oceanbase）
    /// Phase 1+ 将替换为 SZ-ORM `Pool` 实例
    db_connections: HashMap<String, DatabaseConnection>,
    /// Cache 单例占位（Phase 6 接入真正的 Cache facade）
    cache: RwLock<Option<String>>,
    /// Log 单例占位（Phase 0.7 接入 sz-orm-logger + tracing）
    log: RwLock<Option<String>>,
}

impl App {
    /// 构造 App 实例（不注册到全局单例）
    ///
    /// 用于测试或显式持有实例的场景。生产代码应使用 [`App::init()`] 注册全局单例。
    pub fn new(config: AppConfig) -> App {
        let db_connections = config.database.connections.clone();
        App {
            config,
            db_connections,
            cache: RwLock::new(None),
            log: RwLock::new(None),
        }
    }

    /// 初始化全局 App 容器
    ///
    /// 只能调用一次，重复调用返回已有实例。
    ///
    /// ```rust,ignore
    /// use sz_rust_core::container::App;
    /// use sz_rust_core::config::AppConfig;
    ///
    /// let config = AppConfig::load_from_dir("config").unwrap();
    /// let app = App::init(config);
    /// ```
    pub fn init(config: AppConfig) -> &'static App {
        APP.get_or_init(|| App::new(config))
    }

    /// 获取全局 App 容器实例
    ///
    /// 必须先调用 [`App::init()`] 初始化，否则返回 `None`。
    pub fn instance() -> Option<&'static App> {
        APP.get()
    }

    /// 获取应用配置
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// 获取数据库连接配置
    ///
    /// 对齐 PHP `Db::connect('mysql')`。
    ///
    /// Phase 0：返回 `DatabaseConnection` 配置。
    /// Phase 1+：将替换为 SZ-ORM `Pool` 实例。
    pub fn db_connection(&self, name: &str) -> Option<&DatabaseConnection> {
        self.db_connections.get(name)
    }

    /// 获取所有数据库连接名称
    pub fn db_connection_names(&self) -> Vec<&str> {
        self.db_connections.keys().map(|s| s.as_str()).collect()
    }

    /// 获取默认数据库连接配置
    pub fn default_db_connection(&self) -> Option<&DatabaseConnection> {
        self.db_connection(&self.config.database.default)
    }

    /// 设置 Cache 单例（Phase 6 将替换为真正的 Cache facade）
    pub fn set_cache(&self, cache: impl Into<String>) {
        let mut guard = self.cache.write();
        *guard = Some(cache.into());
    }

    /// 获取 Cache 单例
    pub fn cache(&self) -> Option<String> {
        self.cache.read().clone()
    }

    /// 设置 Log 单例（Phase 0.7 将替换为真正的日志系统）
    pub fn set_log(&self, log: impl Into<String>) {
        let mut guard = self.log.write();
        *guard = Some(log.into());
    }

    /// 获取 Log 单例
    pub fn log(&self) -> Option<String> {
        self.log.read().clone()
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("config", &self.config)
            .field(
                "db_connections",
                &self.db_connections.keys().collect::<Vec<_>>(),
            )
            .field("cache", &self.cache.read().is_some())
            .field("log", &self.log.read().is_some())
            .finish()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    /// 构造测试用的 mysql 连接配置
    fn make_mysql_conn() -> DatabaseConnection {
        DatabaseConnection {
            r#type: "mysql".to_string(),
            hostname: "172.17.16.14".to_string(),
            database: "shop".to_string(),
            username: "shop".to_string(),
            password: String::new(),
            hostport: 8802,
            charset: "utf8mb4".to_string(),
            prefix: "sz_".to_string(),
            deploy: 0,
            rw_separate: false,
            fields_strict: true,
            break_reconnect: true,
        }
    }

    /// 测试 App 全局单例初始化和获取
    ///
    /// 注：OnceLock 全局状态不可重置，因此此测试只验证 init/instance 的契约，
    /// 不验证 init 后的 config 内容（config 内容由其他测试用 `App::new()` 验证）。
    #[test]
    fn test_app_init_and_instance() {
        let config = AppConfig::default();
        let app = App::init(config);

        // 验证 instance() 返回同一实例
        let app2 = App::instance();
        assert!(app2.is_some());
        assert!(std::ptr::eq(app, app2.unwrap()));

        // 再次 init 应返回同一实例（不覆盖）
        let config2 = AppConfig::default();
        let app3 = App::init(config2);
        assert!(std::ptr::eq(app, app3));
    }

    /// 测试数据库连接配置获取
    #[test]
    fn test_db_connection() {
        let mut config = AppConfig::default();
        config
            .database
            .connections
            .insert("mysql".to_string(), make_mysql_conn());

        let app = App::new(config);
        let conn = app.db_connection("mysql");
        assert!(conn.is_some());
        let conn = conn.unwrap();
        assert_eq!(conn.hostname, "172.17.16.14");
        assert_eq!(conn.hostport, 8802);
        assert_eq!(conn.prefix, "sz_");
    }

    /// 测试不存在的数据库连接返回 None
    #[test]
    fn test_db_connection_not_found() {
        let config = AppConfig::default();
        let app = App::new(config);
        assert!(app.db_connection("nonexistent").is_none());
    }

    /// 测试默认数据库连接获取
    #[test]
    fn test_default_db_connection() {
        let mut config = AppConfig::default();
        config.database.default = "mysql".to_string();
        config
            .database
            .connections
            .insert("mysql".to_string(), make_mysql_conn());

        let app = App::new(config);
        let conn = app.default_db_connection();
        assert!(conn.is_some());
        assert_eq!(conn.unwrap().database, "shop");
    }

    /// 测试 Cache 单例设置和获取
    #[test]
    fn test_cache() {
        let config = AppConfig::default();
        let app = App::new(config);

        // 初始为 None
        assert!(app.cache().is_none());

        // 设置后可获取
        app.set_cache("memory_cache");
        assert_eq!(app.cache(), Some("memory_cache".to_string()));
    }

    /// 测试 Log 单例设置和获取
    #[test]
    fn test_log() {
        let config = AppConfig::default();
        let app = App::new(config);

        // 初始为 None
        assert!(app.log().is_none());

        // 设置后可获取
        app.set_log("file_logger");
        assert_eq!(app.log(), Some("file_logger".to_string()));
    }

    /// 测试 db_connection_names 返回所有连接名
    #[test]
    fn test_db_connection_names() {
        let mut config = AppConfig::default();
        config
            .database
            .connections
            .insert("mysql".to_string(), make_mysql_conn());
        config.database.connections.insert(
            "njszjt".to_string(),
            DatabaseConnection {
                r#type: "mysql".to_string(),
                hostname: "172.17.16.14".to_string(),
                database: "njszjt".to_string(),
                username: "njszjt".to_string(),
                password: String::new(),
                hostport: 8802,
                charset: "utf8mb4".to_string(),
                prefix: "soci_".to_string(),
                deploy: 0,
                rw_separate: false,
                fields_strict: true,
                break_reconnect: true,
            },
        );

        let app = App::new(config);
        let mut names = app.db_connection_names();
        names.sort();
        assert_eq!(names, vec!["mysql", "njszjt"]);
    }

    /// 测试从实际配置文件加载 5 个数据库连接
    ///
    /// 直接验证 `AppConfig::load_from_dir()` 能加载 5 个连接配置，
    /// 不通过 `App::init()`（避免 OnceLock 全局状态污染）。
    #[test]
    fn test_load_5_db_connections() {
        // 查找 config 目录（从当前目录向上 5 级查找）
        let config_dir = std::env::current_dir().ok().and_then(|d| {
            let mut current = d.clone();
            for _ in 0..5 {
                if current.join("config").exists() {
                    return Some(current.join("config"));
                }
                if let Some(parent) = current.parent() {
                    current = parent.to_path_buf();
                } else {
                    break;
                }
            }
            None
        });

        // 没有 config 目录时跳过（不是所有运行环境都有配置文件）
        let Some(config_dir) = config_dir else {
            eprintln!("跳过：未找到 config 目录");
            return;
        };

        let config = AppConfig::load_from_dir(&config_dir).unwrap();

        // 验证 5 个数据库连接配置
        let names: Vec<&str> = config
            .database
            .connections
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert!(
            names.len() >= 5,
            "应有 5 个数据库连接，实际 {}: {:?}",
            names.len(),
            names
        );

        // 验证每个连接配置存在
        assert!(config.database.connections.contains_key("mysql"));
        assert!(config.database.connections.contains_key("njszjt"));
        assert!(config.database.connections.contains_key("ljclz"));
        assert!(config.database.connections.contains_key("food"));
        assert!(config.database.connections.contains_key("oceanbase"));

        // 验证默认连接（hostname 已改用 localhost，实际地址通过环境变量注入）
        assert_eq!(config.database.default, "mysql");
        let default_conn = config.database.connections.get("mysql").unwrap();
        assert_eq!(default_conn.hostname, "localhost");
        assert_eq!(default_conn.hostport, 8802);
        assert_eq!(default_conn.prefix, "sz_");
    }
}
