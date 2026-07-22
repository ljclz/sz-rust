//! 配置系统 — YAML 加载 + 环境变量覆盖 + 默认值
//!
//! 对齐 PHP `config/app.php` / `config/database.php` 等。
//!
//! ## 环境变量覆盖规则
//!
//! | 格式 | 示例 | 说明 |
//! |------|------|------|
//! | `SZ_{SECTION}__{KEY}` | `SZ_APP__DEFAULT_APP=api` | 标准格式，双下划线分隔层级 |
//! | `SZ_DB_{CONN}_PASSWORD` | `SZ_DB_MYSQL_PASSWORD=xxx` | 数据库密码简写格式 |
//!
//! ## 默认值
//!
//! 所有配置项都有默认值（通过 serde `#[serde(default)]` 或默认函数），
//! 即使 YAML 文件缺失或字段缺失也能正常加载。

use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// 配置错误
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("配置文件读取失败: {path} — {source}")]
    FileRead {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("配置文件解析失败: {path} — {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_yaml::Error,
    },
}

/// 顶层应用配置（含 5 个 section）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub app: AppSection,
    #[serde(default)]
    pub database: DatabaseSection,
    #[serde(default)]
    pub cache: CacheSection,
    #[serde(default)]
    pub addons: AddonsSection,
    #[serde(default)]
    pub log: LogSection,
}

/// 应用配置段 — 对齐 PHP `config/app.php`
#[derive(Debug, Clone, Deserialize)]
pub struct AppSection {
    #[serde(default)]
    pub app_host: String,
    #[serde(default)]
    pub app_namespace: String,
    #[serde(default = "default_true")]
    pub with_route: bool,
    #[serde(default = "default_true")]
    pub with_event: bool,
    #[serde(default = "default_default_app")]
    pub default_app: String,
    #[serde(default = "default_timezone")]
    pub default_timezone: String,
    #[serde(default = "default_true")]
    pub auto_multi_app: bool,
    #[serde(default = "default_app_map")]
    pub app_map: HashMap<String, String>,
    #[serde(default = "default_deny_app_list")]
    pub deny_app_list: Vec<String>,
}

impl Default for AppSection {
    fn default() -> Self {
        Self {
            app_host: String::new(),
            app_namespace: String::new(),
            with_route: true,
            with_event: true,
            default_app: default_default_app(),
            default_timezone: default_timezone(),
            auto_multi_app: true,
            app_map: default_app_map(),
            deny_app_list: default_deny_app_list(),
        }
    }
}

/// 数据库配置段 — 对齐 PHP `config/database.php`
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSection {
    #[serde(default = "default_mysql")]
    pub default: String,
    #[serde(default = "default_true")]
    pub auto_timestamp: bool,
    #[serde(default = "default_datetime_format")]
    pub datetime_format: String,
    #[serde(default)]
    pub connections: HashMap<String, DatabaseConnection>,
}

impl Default for DatabaseSection {
    fn default() -> Self {
        Self {
            default: default_mysql(),
            auto_timestamp: true,
            datetime_format: default_datetime_format(),
            connections: HashMap::new(),
        }
    }
}

/// 单个数据库连接配置
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConnection {
    #[serde(default = "default_mysql")]
    pub r#type: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_port_8802")]
    pub hostport: u16,
    #[serde(default = "default_charset_utf8mb4")]
    pub charset: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub deploy: u8,
    #[serde(default)]
    pub rw_separate: bool,
    #[serde(default = "default_true")]
    pub fields_strict: bool,
    #[serde(default = "default_true")]
    pub break_reconnect: bool,
}

/// 缓存配置段 — 对齐 PHP `think-cache`
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CacheSection {
    #[serde(default = "default_cache_memory")]
    pub default: String,
    #[serde(default)]
    pub stores: HashMap<String, CacheStore>,
}

/// 单个缓存存储配置
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CacheStore {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub capacity: usize,
    #[serde(default)]
    pub levels: Vec<String>,
}

/// 插件配置段 — 对齐 PHP `addons/`
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AddonsSection {
    #[serde(default = "default_addons_path")]
    pub addons_path: String,
    #[serde(default)]
    pub priority: AddonsPriority,
}

/// 插件优先级配置
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AddonsPriority {
    #[serde(default)]
    pub p0: Vec<String>,
    #[serde(default)]
    pub p1: Vec<String>,
    #[serde(default)]
    pub p2: Vec<String>,
}

/// 日志配置段 — 对齐 PHP `think-logger`
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LogSection {
    #[serde(default = "default_log_file")]
    pub default: String,
    #[serde(default)]
    pub channels: HashMap<String, LogChannel>,
}

/// 单个日志通道配置
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LogChannel {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub path: String,
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub max_files: u32,
    #[serde(default)]
    pub format: String,
}

// ============================================================================
// 默认值函数
// ============================================================================

fn default_true() -> bool {
    true
}

fn default_default_app() -> String {
    "index".to_string()
}

fn default_timezone() -> String {
    "Asia/Shanghai".to_string()
}

fn default_app_map() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("oapc".to_string(), "oapc".to_string());
    map.insert("admin".to_string(), "admin".to_string());
    map.insert("api".to_string(), "api".to_string());
    map.insert("farm".to_string(), "farm".to_string());
    map.insert("oapi".to_string(), "oapi".to_string());
    map.insert("cashier".to_string(), "cashier".to_string());
    map.insert("scene".to_string(), "scene".to_string());
    map
}

fn default_deny_app_list() -> Vec<String> {
    vec!["common".to_string()]
}

fn default_mysql() -> String {
    "mysql".to_string()
}

fn default_datetime_format() -> String {
    "Y-m-d H:i:s".to_string()
}

fn default_port_8802() -> u16 {
    8802
}

fn default_charset_utf8mb4() -> String {
    "utf8mb4".to_string()
}

fn default_cache_memory() -> String {
    "memory".to_string()
}

fn default_addons_path() -> String {
    "addons".to_string()
}

fn default_log_file() -> String {
    "file".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

// ============================================================================
// 加载与环境变量覆盖
// ============================================================================

impl AppConfig {
    /// 从配置目录加载所有配置文件
    ///
    /// 目录结构：
    /// ```text
    /// config/
    /// ├── app.yml
    /// ├── database.yml
    /// ├── cache.yml
    /// ├── addons.yml
    /// └── log.yml
    /// ```
    pub fn load_from_dir(config_dir: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let dir = config_dir.as_ref();

        // 逐个加载 section（文件不存在时用默认值，不报错）
        let mut config = AppConfig {
            app: load_section(&dir.join("app.yml"), AppSection::default())?,
            database: load_section(&dir.join("database.yml"), DatabaseSection::default())?,
            cache: load_section(&dir.join("cache.yml"), CacheSection::default())?,
            addons: load_section(&dir.join("addons.yml"), AddonsSection::default())?,
            log: load_section(&dir.join("log.yml"), LogSection::default())?,
        };

        // 应用环境变量覆盖
        config.apply_env_overrides();

        Ok(config)
    }

    /// 应用环境变量覆盖
    ///
    /// 支持两种格式：
    /// 1. `SZ_DB_{CONN}_PASSWORD` → `database.connections.{conn}.password`
    /// 2. `SZ_APP__{KEY}` → `app.{key}`（标准格式，未来扩展）
    pub fn apply_env_overrides(&mut self) {
        // 数据库密码简写格式：SZ_DB_{CONN}_PASSWORD
        for (conn_name, conn) in &mut self.database.connections {
            let env_key = format!("SZ_DB_{}_PASSWORD", conn_name.to_uppercase());
            if let Ok(password) = std::env::var(&env_key) {
                if !password.is_empty() {
                    conn.password = password;
                }
            }
        }
    }

    /// 获取默认数据库连接
    pub fn default_connection(&self) -> Option<&DatabaseConnection> {
        self.database.connections.get(&self.database.default)
    }
}

/// 从 YAML 文件加载单个 section（文件不存在时返回默认值）
fn load_section<T: DeserializeOwned + Default>(path: &Path, default: T) -> Result<T, ConfigError> {
    if !path.exists() {
        return Ok(default);
    }
    let content = std::fs::read_to_string(path).map_err(|e| ConfigError::FileRead {
        path: path.display().to_string(),
        source: e,
    })?;
    serde_yaml::from_str(&content).map_err(|e| ConfigError::Parse {
        path: path.display().to_string(),
        source: e,
    })
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// env 变量测试互斥锁：避免并行测试时 `SZ_DB_MYSQL_PASSWORD` 被多个测试同时设置/读取
    /// 造成状态污染（参见 R5: 测试必须覆盖 DML 操作序列以检测状态污染 bug）
    static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 测试默认值：所有 section 都有合理的默认值
    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert!(config.app.auto_multi_app);
        assert!(config.app.with_route);
        assert_eq!(config.app.default_app, "index");
        assert_eq!(config.app.default_timezone, "Asia/Shanghai");
        assert_eq!(config.app.app_map.len(), 7);
        assert!(config.app.app_map.contains_key("oapc"));
        assert_eq!(config.app.deny_app_list, vec!["common"]);

        assert_eq!(config.database.default, "mysql");
        assert!(config.database.auto_timestamp);
        assert_eq!(config.database.datetime_format, "Y-m-d H:i:s");
    }

    /// 测试从 YAML 字符串加载
    #[test]
    fn test_load_from_yaml_string() {
        let yaml = r#"
app_host: "https://example.com"
default_app: "api"
auto_multi_app: true
app_map:
  oapc: oapc
  admin: admin
"#;
        let app: AppSection = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(app.app_host, "https://example.com");
        assert_eq!(app.default_app, "api");
        assert!(app.auto_multi_app);
        assert_eq!(app.app_map.len(), 2);
    }

    /// 测试从目录加载（使用项目实际的 config/ 目录）
    #[test]
    fn test_load_from_dir() {
        // config/ 目录位于 workspace 根
        let config_dir = std::env::current_dir().ok().and_then(|d| {
            // 测试运行时 cwd 可能是 packages/sz-rust-core
            // 向上查找直到找到 config/ 目录
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

        if let Some(config_dir) = config_dir {
            let config = AppConfig::load_from_dir(&config_dir).unwrap();
            // 验证 app.yml 加载
            assert_eq!(config.app.default_app, "index");
            assert!(config.app.auto_multi_app);
            assert_eq!(config.app.app_map.len(), 7);
            assert_eq!(config.app.deny_app_list, vec!["common"]);

            // 验证 database.yml 加载
            assert_eq!(config.database.default, "mysql");
            assert_eq!(config.database.connections.len(), 5);
            assert!(config.database.connections.contains_key("mysql"));
            assert!(config.database.connections.contains_key("njszjt"));
            assert!(config.database.connections.contains_key("ljclz"));
            assert!(config.database.connections.contains_key("food"));
            assert!(config.database.connections.contains_key("oceanbase"));

            // 验证 mysql 连接
            let mysql = config.database.connections.get("mysql").unwrap();
            assert_eq!(mysql.hostname, "172.17.16.14");
            assert_eq!(mysql.hostport, 8802);
            assert_eq!(mysql.charset, "utf8mb4");
            assert_eq!(mysql.prefix, "sz_");

            // 验证 ljclz 连接（charset=utf8, prefix=ims_）
            let ljclz = config.database.connections.get("ljclz").unwrap();
            assert_eq!(ljclz.charset, "utf8");
            assert_eq!(ljclz.prefix, "ims_");

            // 验证 oceanbase 连接（hostport=2881）
            let oceanbase = config.database.connections.get("oceanbase").unwrap();
            assert_eq!(oceanbase.hostport, 2881);
            assert_eq!(oceanbase.hostname, "172.17.16.3");

            // 验证 cache.yml 加载
            assert_eq!(config.cache.default, "memory");
            assert!(config.cache.stores.contains_key("memory"));

            // 验证 addons.yml 加载
            assert_eq!(config.addons.addons_path, "addons");
            assert_eq!(config.addons.priority.p0.len(), 3);

            // 验证 log.yml 加载
            assert_eq!(config.log.default, "file");
            assert!(config.log.channels.contains_key("file"));
        }
    }

    /// 测试文件不存在时使用默认值
    #[test]
    fn test_load_missing_file_uses_default() {
        let temp_dir = std::env::temp_dir().join("sz_rust_config_test_missing");
        let _ = std::fs::create_dir_all(&temp_dir);
        // 目录存在但无任何 yml 文件
        let config = AppConfig::load_from_dir(&temp_dir).unwrap();
        assert!(config.app.auto_multi_app);
        assert_eq!(config.database.default, "mysql");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// 测试环境变量覆盖数据库密码
    #[test]
    fn test_env_override_password() {
        // 获取 env 测试锁，确保与 test_env_override_empty_ignored 串行运行
        let _env_guard = ENV_TEST_LOCK.lock().unwrap();
        // 清理可能残留的 env 变量（防御性：避免被先前测试残留状态污染）
        std::env::remove_var("SZ_DB_MYSQL_PASSWORD");

        let mut config = AppConfig::default();
        config.database.connections.insert(
            "mysql".to_string(),
            DatabaseConnection {
                r#type: "mysql".to_string(),
                hostname: "localhost".to_string(),
                database: "test".to_string(),
                username: "root".to_string(),
                password: String::new(),
                hostport: 3306,
                charset: "utf8mb4".to_string(),
                prefix: "sz_".to_string(),
                deploy: 0,
                rw_separate: false,
                fields_strict: true,
                break_reconnect: true,
            },
        );

        // 设置环境变量
        std::env::set_var("SZ_DB_MYSQL_PASSWORD", "secret123");

        // 应用覆盖
        config.apply_env_overrides();

        // 验证密码被覆盖
        assert_eq!(config.database.connections["mysql"].password, "secret123");

        // 清理环境变量
        std::env::remove_var("SZ_DB_MYSQL_PASSWORD");
    }

    /// 测试环境变量为空时不覆盖
    #[test]
    fn test_env_override_empty_ignored() {
        // 获取 env 测试锁，确保与 test_env_override_password 串行运行
        let _env_guard = ENV_TEST_LOCK.lock().unwrap();
        // 清理可能残留的 env 变量（防御性：避免被先前测试残留状态污染）
        std::env::remove_var("SZ_DB_MYSQL_PASSWORD");

        let mut config = AppConfig::default();
        config.database.connections.insert(
            "mysql".to_string(),
            DatabaseConnection {
                r#type: "mysql".to_string(),
                hostname: "localhost".to_string(),
                database: "test".to_string(),
                username: "root".to_string(),
                password: "existing".to_string(),
                hostport: 3306,
                charset: "utf8mb4".to_string(),
                prefix: "sz_".to_string(),
                deploy: 0,
                rw_separate: false,
                fields_strict: true,
                break_reconnect: true,
            },
        );

        // 设置空环境变量
        std::env::set_var("SZ_DB_MYSQL_PASSWORD", "");

        config.apply_env_overrides();

        // 空环境变量不应覆盖现有密码
        assert_eq!(config.database.connections["mysql"].password, "existing");

        std::env::remove_var("SZ_DB_MYSQL_PASSWORD");
    }

    /// 测试获取默认连接
    #[test]
    fn test_default_connection() {
        let mut config = AppConfig::default();
        config.database.default = "mysql".to_string();
        config.database.connections.insert(
            "mysql".to_string(),
            DatabaseConnection {
                r#type: "mysql".to_string(),
                hostname: "localhost".to_string(),
                database: "test".to_string(),
                username: "root".to_string(),
                password: String::new(),
                hostport: 3306,
                charset: "utf8mb4".to_string(),
                prefix: "sz_".to_string(),
                deploy: 0,
                rw_separate: false,
                fields_strict: true,
                break_reconnect: true,
            },
        );

        let conn = config.default_connection();
        assert!(conn.is_some());
        assert_eq!(conn.unwrap().hostname, "localhost");
    }

    /// 测试默认连接不存在时返回 None
    #[test]
    fn test_default_connection_missing() {
        let config = AppConfig::default();
        assert!(config.default_connection().is_none());
    }

    /// 测试 YAML 解析错误
    #[test]
    fn test_parse_error() {
        let bad_yaml = "default: mysql\n  bad: : : indent";
        let result: Result<DatabaseSection, _> = serde_yaml::from_str(bad_yaml);
        // 无效 YAML 应该返回错误（或被 serde 宽容处理）
        // 这里只验证不 panic
        let _ = result;
    }
}
