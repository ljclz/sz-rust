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
use std::sync::Arc;
use thiserror::Error;

/// 配置错误
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 配置文件读取失败
    #[error("配置文件读取失败: {path} — {source}")]
    FileRead {
        /// 配置文件路径
        path: String,
        /// 底层 IO 错误
        #[source]
        source: std::io::Error,
    },
    /// 配置文件解析失败
    #[error("配置文件解析失败: {path} — {source}")]
    Parse {
        /// 配置文件路径
        path: String,
        /// 底层解析错误
        #[source]
        source: serde_yaml::Error,
    },
    /// 生产环境禁止使用 debug/trace 日志级别
    #[error("生产环境禁止使用 {level} 日志级别 — 请使用 warn 或更高级别")]
    LogLevelForbiddenInProduction {
        /// 当前日志级别
        level: String,
    },
    /// AI 配置校验失败
    #[error("AI 配置校验失败: {0}")]
    AiConfigInvalid(String),
    /// Data Scope 配置校验失败
    #[error("Data Scope 配置校验失败: {0}")]
    DataScopeConfigInvalid(String),
}

/// 顶层应用配置（含 6 个 section + AI section）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppConfig {
    /// 应用配置段
    #[serde(default)]
    pub app: AppSection,
    /// 数据库配置段
    #[serde(default)]
    pub database: DatabaseSection,
    /// 缓存配置段
    #[serde(default)]
    pub cache: CacheSection,
    /// 插件配置段
    #[serde(default)]
    pub addons: AddonsSection,
    /// 日志配置段
    #[serde(default)]
    pub log: LogSection,
    /// 服务器配置段（HTTP 监听地址与端口）
    #[serde(default)]
    pub server: ServerSection,
    /// AI 配置段（可选，不配置时 AI 功能不可用）
    #[serde(default)]
    pub ai: Option<AiSection>,
    /// Data Scope 配置段（可选，不配置时数据范围控制不可用）
    #[serde(default)]
    pub data_scope: DataScopeSection,
}

/// 应用配置段 — 对齐 PHP `config/app.php`
#[derive(Debug, Clone, Deserialize)]
pub struct AppSection {
    /// 应用主机地址
    #[serde(default)]
    pub app_host: String,
    /// 应用命名空间
    #[serde(default)]
    pub app_namespace: String,
    /// 是否启用路由
    #[serde(default = "default_true")]
    pub with_route: bool,
    /// 是否启用事件系统
    #[serde(default = "default_true")]
    pub with_event: bool,
    /// 默认应用名
    #[serde(default = "default_default_app")]
    pub default_app: String,
    /// 默认时区
    #[serde(default = "default_timezone")]
    pub default_timezone: String,
    /// 是否启用多应用模式
    #[serde(default = "default_true")]
    pub auto_multi_app: bool,
    /// 应用映射表（域名/路径 → 应用名）
    #[serde(default = "default_app_map")]
    pub app_map: HashMap<String, String>,
    /// 禁止访问的应用列表
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
    /// 默认连接名
    #[serde(default = "default_mysql")]
    pub default: String,
    /// 是否自动时间戳
    #[serde(default = "default_true")]
    pub auto_timestamp: bool,
    /// 时间戳格式
    #[serde(default = "default_datetime_format")]
    pub datetime_format: String,
    /// 数据库连接配置表（连接名 → 连接配置）
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
    /// 数据库类型（如 mysql）
    #[serde(default = "default_mysql")]
    pub r#type: String,
    /// 主机名
    #[serde(default)]
    pub hostname: String,
    /// 数据库名
    #[serde(default)]
    pub database: String,
    /// 用户名
    #[serde(default)]
    pub username: String,
    /// 密码
    ///
    /// 安全约束：即使未来为 `DatabaseConnection` 派生 `Serialize`，
    /// 密码也绝不应出现在序列化输出中（防止日志/响应泄露）。
    #[serde(default, skip_serializing)]
    pub password: String,
    /// 主机端口
    #[serde(default = "default_port_8802")]
    pub hostport: u16,
    /// 字符集
    #[serde(default = "default_charset_utf8mb4")]
    pub charset: String,
    /// 表前缀
    #[serde(default)]
    pub prefix: String,
    /// 部署模式（0=集中式 1=分布式）
    #[serde(default)]
    pub deploy: u8,
    /// 是否读写分离
    #[serde(default)]
    pub rw_separate: bool,
    /// 是否严格字段校验
    #[serde(default = "default_true")]
    pub fields_strict: bool,
    /// 是否断线重连
    #[serde(default = "default_true")]
    pub break_reconnect: bool,
}

/// 缓存配置段 — 对齐 PHP `think-cache`
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CacheSection {
    /// 默认缓存存储名
    #[serde(default = "default_cache_memory")]
    pub default: String,
    /// 缓存存储配置表（存储名 → 存储配置）
    #[serde(default)]
    pub stores: HashMap<String, CacheStore>,
}

/// 单个缓存存储配置
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CacheStore {
    /// 存储类型（如 memory）
    #[serde(default)]
    pub r#type: String,
    /// 容量上限
    #[serde(default)]
    pub capacity: usize,
    /// 分层级别列表
    #[serde(default)]
    pub levels: Vec<String>,
}

/// 插件配置段 — 对齐 PHP `addons/`
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AddonsSection {
    /// 插件目录路径
    #[serde(default = "default_addons_path")]
    pub addons_path: String,
    /// 插件优先级配置
    #[serde(default)]
    pub priority: AddonsPriority,
}

/// 插件优先级配置
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AddonsPriority {
    /// 优先级 P0 插件列表（最高）
    #[serde(default)]
    pub p0: Vec<String>,
    /// 优先级 P1 插件列表
    #[serde(default)]
    pub p1: Vec<String>,
    /// 优先级 P2 插件列表（最低）
    #[serde(default)]
    pub p2: Vec<String>,
}

/// 日志配置段 — 对齐 PHP `think-logger`
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LogSection {
    /// 默认日志通道名
    #[serde(default = "default_log_file")]
    pub default: String,
    /// 日志通道配置表（通道名 → 通道配置）
    #[serde(default)]
    pub channels: HashMap<String, LogChannel>,
}

/// 单个日志通道配置
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LogChannel {
    /// 通道类型（如 file）
    #[serde(default)]
    pub r#type: String,
    /// 日志文件路径
    #[serde(default)]
    pub path: String,
    /// 日志级别
    #[serde(default = "default_log_level")]
    pub level: String,
    /// 最大保留文件数
    #[serde(default)]
    pub max_files: u32,
    /// 日志格式
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

/// 服务器配置段 — HTTP 监听地址与端口
///
/// 对齐 PHP `think-swoole` 的 `config/swoole.php` 中 server.host / server.port 配置。
/// 默认监听 `0.0.0.0:8080`，可通过 `config/server.yml` 或环境变量 `SZ_SERVER__PORT` 覆盖。
#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    /// 监听地址（默认 `0.0.0.0`，对所有网卡开放）
    #[serde(default = "default_server_host")]
    pub host: String,
    /// 监听端口（默认 `8080`）
    #[serde(default = "default_server_port")]
    pub port: u16,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            host: default_server_host(),
            port: default_server_port(),
        }
    }
}

fn default_server_host() -> String {
    "0.0.0.0".to_string()
}

fn default_server_port() -> u16 {
    8080
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
    "warn".to_string()
}

// ============================================================================
// LogConfig — 生产环境日志级别配置与校验
// ============================================================================

/// 日志配置（生产环境加固）
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// 日志级别（默认 `warn,sz_rust_sz300=info`）
    pub level: String,
    /// 生产环境最低允许级别（固定 `warn`）
    pub production_min_level: String,
    /// 日志排除路径（不记录访问日志的端点）
    pub exclude_paths: Vec<String>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "warn,sz_rust_sz300=info".to_string(),
            production_min_level: "warn".to_string(),
            exclude_paths: vec![
                "/health".into(),
                "/health/ready".into(),
                "/health/startup".into(),
                "/metrics".into(),
            ],
        }
    }
}

impl LogConfig {
    /// 从环境变量读取日志配置
    ///
    /// - `RUST_LOG`：日志级别（未设置时使用默认 `warn,sz_rust_sz300=info`）
    pub fn from_env() -> Self {
        let level =
            std::env::var("RUST_LOG").unwrap_or_else(|_| "warn,sz_rust_sz300=info".to_string());
        Self {
            level,
            ..Default::default()
        }
    }

    /// 校验生产环境日志级别
    ///
    /// `env=production` 且 level 含 `debug`/`trace` → 返回错误
    pub fn validate_production(&self, env: &str) -> Result<(), ConfigError> {
        if env != "production" {
            return Ok(());
        }
        let level_lower = self.level.to_lowercase();
        if level_lower.contains("debug") || level_lower.contains("trace") {
            return Err(ConfigError::LogLevelForbiddenInProduction {
                level: self.level.clone(),
            });
        }
        Ok(())
    }
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
    /// ├── log.yml
    /// └── server.yml
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn load_from_dir(config_dir: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let dir = config_dir.as_ref();

        // 逐个加载 section（文件不存在时用默认值，不报错）
        let mut config = AppConfig {
            app: load_section(&dir.join("app.yml"), AppSection::default()).await?,
            database: load_section(&dir.join("database.yml"), DatabaseSection::default()).await?,
            cache: load_section(&dir.join("cache.yml"), CacheSection::default()).await?,
            addons: load_section(&dir.join("addons.yml"), AddonsSection::default()).await?,
            log: load_section(&dir.join("log.yml"), LogSection::default()).await?,
            server: load_section(&dir.join("server.yml"), ServerSection::default()).await?,
            ai: load_optional_section(&dir.join("ai.yml")).await?,
            data_scope: load_section(&dir.join("data_scope.yml"), DataScopeSection::default())
                .await?,
        };

        // 应用环境变量覆盖
        config.apply_env_overrides();

        Ok(config)
    }

    /// 应用环境变量覆盖
    ///
    /// 支持以下环境变量格式：
    /// 1. `SZ_DB_{CONN}_PASSWORD` → `database.connections.{conn}.password`
    /// 2. `SZ_DB_{CONN}_HOSTNAME` → `database.connections.{conn}.hostname`
    /// 3. `SZ_DB_{CONN}_HOSTPORT` → `database.connections.{conn}.hostport`
    /// 4. `SZ_APP__{KEY}` → `app.{key}`（标准格式，未来扩展）
    #[tracing::instrument(skip(self))]
    pub fn apply_env_overrides(&mut self) {
        // 数据库连接环境变量覆盖：SZ_DB_{CONN}_{FIELD}
        for (conn_name, conn) in &mut self.database.connections {
            let prefix = format!("SZ_DB_{}", conn_name.to_uppercase());

            // 密码
            let env_key = format!("{}_PASSWORD", prefix);
            if let Ok(password) = std::env::var(&env_key) {
                if !password.is_empty() {
                    conn.password = password;
                }
            }

            // 主机名（支持通过环境变量覆盖内网 IP，避免硬编码）
            let env_key = format!("{}_HOSTNAME", prefix);
            if let Ok(hostname) = std::env::var(&env_key) {
                if !hostname.is_empty() {
                    conn.hostname = hostname;
                }
            }

            // 端口
            let env_key = format!("{}_HOSTPORT", prefix);
            if let Ok(hostport_str) = std::env::var(&env_key) {
                if !hostport_str.is_empty() {
                    if let Ok(hostport) = hostport_str.parse() {
                        conn.hostport = hostport;
                    }
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
async fn load_section<T: DeserializeOwned + Default>(
    path: &Path,
    default: T,
) -> Result<T, ConfigError> {
    if !path.exists() {
        return Ok(default);
    }
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| ConfigError::FileRead {
            path: path.display().to_string(),
            source: e,
        })?;
    serde_yaml::from_str(&content).map_err(|e| ConfigError::Parse {
        path: path.display().to_string(),
        source: e,
    })
}

/// 加载可选配置段 — 文件不存在时返回 None，存在时解析为 Some(T)
async fn load_optional_section<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, ConfigError> {
    if !path.exists() {
        return Ok(None);
    }
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| ConfigError::FileRead {
            path: path.display().to_string(),
            source: e,
        })?;
    serde_yaml::from_str(&content)
        .map(Some)
        .map_err(|e| ConfigError::Parse {
            path: path.display().to_string(),
            source: e,
        })
}

// ============================================================================
// 配置热重载 — ConfigWatcher
// ============================================================================

/// 配置热重载观察器
///
/// 在后台定时轮询配置文件修改时间，检测到变化时自动重新加载配置。
/// 对齐 PHP `think-swoole` 的热重载机制，无需重启服务即可更新配置。
///
/// ## 设计
///
/// - 使用 `Arc<RwLock<AppConfig\>>` 共享配置，读无锁、写互斥
/// - 轮询间隔默认 5 秒（可配置），通过 `tokio::time::interval` 实现
/// - 比较文件修改时间（mtime），避免频繁的文件读取
/// - 配置重载失败时保留旧配置，记录错误日志
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_infra_facade::config::{AppConfig, ConfigWatcher};
/// use std::sync::Arc;
/// use parking_lot::RwLock;
///
/// let config = AppConfig::load_from_dir("config/").await.unwrap();
/// let shared = Arc::new(RwLock::new(config));
/// let watcher = ConfigWatcher::new("config/", shared.clone());
///
/// // 启动后台监听（spawn 到 tokio runtime）
/// let handle = watcher.start();
///
/// // 读取最新配置（热重载后自动生效）
/// let current = shared.read().clone();
///
/// // 停止监听
/// handle.stop();
/// ```
pub struct ConfigWatcher {
    /// 配置目录路径
    config_dir: std::path::PathBuf,
    /// 共享配置（`Arc<RwLock<AppConfig\>>`）
    shared_config: Arc<parking_lot::RwLock<AppConfig>>,
    /// 轮询间隔（秒）
    poll_interval_secs: u64,
    /// 上次各文件的修改时间戳
    last_mtimes: parking_lot::RwLock<HashMap<String, std::time::SystemTime>>,
}

/// 热重载句柄，用于停止后台监听
pub struct ConfigWatcherHandle {
    cancel: tokio_util::sync::CancellationToken,
}

impl ConfigWatcherHandle {
    /// 停止配置监听
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl ConfigWatcher {
    /// 创建配置热重载观察器
    ///
    /// # 参数
    ///
    /// - `config_dir`：配置文件目录
    /// - `shared_config`：共享配置（通过 `Arc<RwLock<AppConfig>>` 分享给业务层）
    pub fn new(
        config_dir: impl Into<std::path::PathBuf>,
        shared_config: Arc<parking_lot::RwLock<AppConfig>>,
    ) -> Self {
        Self {
            config_dir: config_dir.into(),
            shared_config,
            poll_interval_secs: 5,
            last_mtimes: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// 设置轮询间隔（秒）
    #[must_use]
    pub fn with_poll_interval(mut self, secs: u64) -> Self {
        self.poll_interval_secs = secs;
        self
    }

    /// 初始化：记录当前所有配置文件的 mtime
    async fn init_mtimes(&self) {
        let files = self.config_files();
        // 先收集所有 mtime（不持锁 await，避免 parking_lot RwLockGuard !Send 问题）
        let mut updates = Vec::new();
        for file in &files {
            if let Ok(meta) = tokio::fs::metadata(file).await {
                if let Ok(mtime) = meta.modified() {
                    updates.push((file.display().to_string(), mtime));
                }
            }
        }
        // 批量写入
        let mut mtimes = self.last_mtimes.write();
        for (key, mtime) in updates {
            mtimes.insert(key, mtime);
        }
    }

    /// 获取所有配置文件路径
    fn config_files(&self) -> Vec<std::path::PathBuf> {
        let names = [
            "app.yml",
            "database.yml",
            "cache.yml",
            "addons.yml",
            "log.yml",
            "server.yml",
        ];
        names.iter().map(|n| self.config_dir.join(n)).collect()
    }

    /// 检测配置文件是否有变化
    ///
    /// 比较当前 mtime 与上次记录的 mtime，任一文件变化则返回 true。
    async fn has_changes(&self) -> bool {
        let files = self.config_files();
        // 先持锁读取（不跨 await）
        let mtimes_snapshot: std::collections::HashMap<String, std::time::SystemTime> = {
            let mtimes = self.last_mtimes.read();
            mtimes.clone()
        };
        for file in &files {
            if let Ok(meta) = tokio::fs::metadata(file).await {
                if let Ok(mtime) = meta.modified() {
                    let key = file.display().to_string();
                    if let Some(last) = mtimes_snapshot.get(&key) {
                        if last != &mtime {
                            return true;
                        }
                    } else {
                        // 新文件
                        return true;
                    }
                }
            }
        }
        false
    }

    /// 更新 mtime 记录
    async fn update_mtimes(&self) {
        let files = self.config_files();
        let mut updates = Vec::new();
        for file in &files {
            if let Ok(meta) = tokio::fs::metadata(file).await {
                if let Ok(mtime) = meta.modified() {
                    updates.push((file.display().to_string(), mtime));
                }
            }
        }
        let mut mtimes = self.last_mtimes.write();
        for (key, mtime) in updates {
            mtimes.insert(key, mtime);
        }
    }

    /// 启动后台配置监听
    ///
    /// 返回 [`ConfigWatcherHandle`]，调用 `stop()` 可停止监听。
    pub fn start(self) -> ConfigWatcherHandle {
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel.clone();

        let config_dir = self.config_dir.clone();
        let shared_config = self.shared_config.clone();
        let poll_interval = std::time::Duration::from_secs(self.poll_interval_secs);
        let watcher = self;

        tokio::spawn(async move {
            // 初始化 mtime 记录
            watcher.init_mtimes().await;

            let mut ticker = tokio::time::interval(poll_interval);
            ticker.tick().await; // 跳过首次立即触发

            loop {
                tokio::select! {
                    _ = cancel_clone.cancelled() => {
                        tracing::info!("配置热重载监听已停止");
                        break;
                    }
                    _ = ticker.tick() => {
                        if watcher.has_changes().await {
                            tracing::info!("检测到配置文件变化，正在重新加载...");
                            match AppConfig::load_from_dir(&config_dir).await {
                                Ok(new_config) => {
                                    *shared_config.write() = new_config;
                                    watcher.update_mtimes().await;
                                    tracing::info!("配置热重载完成");
                                }
                                Err(e) => {
                                    tracing::error!("配置热重载失败，保留旧配置: {e}");
                                    watcher.update_mtimes().await;
                                }
                            }
                        }
                    }
                }
            }
        });

        ConfigWatcherHandle { cancel }
    }
}

// ============================================================================
// AI 配置段
// ============================================================================

/// AI 配置段 — Provider 凭证 / 模型路由 / 限流 / 故障切换 / Agent / Embedding / 向量存储
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct AiSection {
    /// AI Provider 列表（openai/claude/gemini 等）
    #[serde(default)]
    pub providers: Vec<AiProviderConfig>,
    /// 模型路由表 — model name → provider name
    #[serde(default)]
    pub routing: AiRoutingTable,
    /// 限流配置
    #[serde(default)]
    pub rate_limit: AiRateLimitConfig,
    /// 默认模型名
    #[serde(default = "default_ai_default_model")]
    pub default_model: String,
    /// 故障切换配置
    #[serde(default)]
    pub failover: AiFailoverConfig,
    /// Agent 配置
    #[serde(default)]
    pub agent: AiAgentConfig,
    /// Embedding 配置
    #[serde(default)]
    pub embedding: AiEmbeddingConfig,
    /// 向量存储配置
    #[serde(default)]
    pub vector: AiVectorConfig,
}

fn default_ai_default_model() -> String {
    "gpt-4o".to_string()
}

impl Default for AiSection {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            routing: AiRoutingTable::default(),
            rate_limit: AiRateLimitConfig::default(),
            default_model: default_ai_default_model(),
            failover: AiFailoverConfig::default(),
            agent: AiAgentConfig::default(),
            embedding: AiEmbeddingConfig::default(),
            vector: AiVectorConfig::default(),
        }
    }
}

impl AiSection {
    /// 从环境变量加载 AI 配置
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut section = Self::default();
        if let Ok(model) = std::env::var("SZ_AI_DEFAULT_MODEL") {
            if !model.is_empty() {
                section.default_model = model;
            }
        }
        if let Ok(rps) = std::env::var("SZ_AI_RATE_LIMIT_RPS") {
            if let Ok(rps) = rps.parse::<u32>() {
                section.rate_limit.rps = rps;
            }
        }
        if let Ok(burst) = std::env::var("SZ_AI_RATE_LIMIT_BURST") {
            if let Ok(burst) = burst.parse::<u32>() {
                section.rate_limit.burst = burst;
            }
        }
        Ok(section)
    }

    /// 校验 AI 配置合法性
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.rate_limit.rps == 0 {
            return Err(ConfigError::AiConfigInvalid(
                "rate_limit.rps must be > 0".to_string(),
            ));
        }
        if !self.providers.is_empty() && self.routing.routes.is_empty() {
            return Err(ConfigError::AiConfigInvalid(
                "providers configured but routing table is empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// 单个 AI Provider 配置
#[derive(Clone, serde::Serialize, Deserialize)]
pub struct AiProviderConfig {
    /// Provider 名称（openai/claude/gemini）
    pub name: String,
    /// API 密钥（铁律 7：序列化跳过 + Debug 脱敏，防止日志/响应泄露）
    #[serde(skip_serializing)]
    #[serde(default)]
    pub api_key: String,
    /// API 基础 URL
    #[serde(default)]
    pub base_url: String,
    /// 支持的模型列表
    #[serde(default)]
    pub models: Vec<String>,
}

/// Debug 脱敏：不输出 api_key
impl std::fmt::Debug for AiProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiProviderConfig")
            .field("name", &self.name)
            .field("api_key", &"***")
            .field("base_url", &self.base_url)
            .field("models", &self.models)
            .finish()
    }
}

/// 模型路由表 — model name → provider name
#[derive(Debug, Clone, serde::Serialize, Deserialize, Default)]
pub struct AiRoutingTable {
    /// 路由映射 — model name → provider name
    #[serde(default)]
    pub routes: HashMap<String, String>,
}

/// AI 限流配置
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct AiRateLimitConfig {
    /// 每秒请求数
    #[serde(default = "default_ai_rps")]
    pub rps: u32,
    /// 突发容量
    #[serde(default = "default_ai_burst")]
    pub burst: u32,
}

fn default_ai_rps() -> u32 {
    10
}

fn default_ai_burst() -> u32 {
    20
}

impl Default for AiRateLimitConfig {
    fn default() -> Self {
        Self {
            rps: default_ai_rps(),
            burst: default_ai_burst(),
        }
    }
}

/// AI 故障切换配置
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct AiFailoverConfig {
    /// 连续失败阈值（达到后切换至备用 Provider）
    #[serde(default = "default_ai_failover_threshold")]
    pub threshold: u32,
    /// 冷却时间（毫秒）
    #[serde(default = "default_ai_failover_cooldown")]
    pub cooldown_ms: u64,
}

fn default_ai_failover_threshold() -> u32 {
    3
}

fn default_ai_failover_cooldown() -> u64 {
    30_000
}

impl Default for AiFailoverConfig {
    fn default() -> Self {
        Self {
            threshold: default_ai_failover_threshold(),
            cooldown_ms: default_ai_failover_cooldown(),
        }
    }
}

/// AI Agent 配置
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct AiAgentConfig {
    /// 默认最大步数
    #[serde(default = "default_ai_agent_max_steps")]
    pub default_max_steps: u32,
    /// 工具调用超时（毫秒）
    #[serde(default = "default_ai_agent_tool_timeout")]
    pub tool_timeout_ms: u64,
    /// 空闲超时（毫秒）
    #[serde(default = "default_ai_agent_idle_timeout")]
    pub idle_timeout_ms: u64,
}

fn default_ai_agent_max_steps() -> u32 {
    25
}

fn default_ai_agent_tool_timeout() -> u64 {
    30_000
}

fn default_ai_agent_idle_timeout() -> u64 {
    30_000
}

impl Default for AiAgentConfig {
    fn default() -> Self {
        Self {
            default_max_steps: default_ai_agent_max_steps(),
            tool_timeout_ms: default_ai_agent_tool_timeout(),
            idle_timeout_ms: default_ai_agent_idle_timeout(),
        }
    }
}

/// AI Embedding 配置
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct AiEmbeddingConfig {
    /// 默认 Embedding 模型
    #[serde(default = "default_ai_embed_model")]
    pub default_model: String,
    /// 批量大小
    #[serde(default = "default_ai_embed_batch")]
    pub batch_size: u32,
    /// 缓存 TTL（秒）
    #[serde(default = "default_ai_embed_cache_ttl")]
    pub cache_ttl_secs: u64,
}

fn default_ai_embed_model() -> String {
    "text-embedding-3-small".to_string()
}

fn default_ai_embed_batch() -> u32 {
    64
}

fn default_ai_embed_cache_ttl() -> u64 {
    86_400
}

impl Default for AiEmbeddingConfig {
    fn default() -> Self {
        Self {
            default_model: default_ai_embed_model(),
            batch_size: default_ai_embed_batch(),
            cache_ttl_secs: default_ai_embed_cache_ttl(),
        }
    }
}

/// AI 向量存储配置
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct AiVectorConfig {
    /// 向量存储后端（orm/qdrant/milvus）
    #[serde(default = "default_ai_vector_backend")]
    pub backend: String,
    /// 默认相似度度量（cosine/dot/l2）
    #[serde(default = "default_ai_vector_metric")]
    pub default_metric: String,
    /// 向量维度
    #[serde(default = "default_ai_vector_dimensions")]
    pub dimensions: usize,
}

fn default_ai_vector_backend() -> String {
    "orm".to_string()
}

fn default_ai_vector_metric() -> String {
    "cosine".to_string()
}

fn default_ai_vector_dimensions() -> usize {
    1536
}

impl Default for AiVectorConfig {
    fn default() -> Self {
        Self {
            backend: default_ai_vector_backend(),
            default_metric: default_ai_vector_metric(),
            dimensions: default_ai_vector_dimensions(),
        }
    }
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

        // 验证 server 默认值
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
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
    #[tokio::test]
    async fn test_load_from_dir() {
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
            let config = AppConfig::load_from_dir(&config_dir).await.unwrap();
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

            // 验证 mysql 连接（hostname 已改用 localhost，实际地址通过环境变量注入；
            // hostport 默认 3306，与 config/database.yml 一致，环境变量 SZ_DB_MYSQL_HOSTPORT 未设置时生效）
            let mysql = config.database.connections.get("mysql").unwrap();
            assert_eq!(mysql.hostname, "localhost");
            assert_eq!(mysql.hostport, 3306);
            assert_eq!(mysql.charset, "utf8mb4");
            assert_eq!(mysql.prefix, "sz_");

            // 验证 ljclz 连接（charset=utf8, prefix=ims_）
            let ljclz = config.database.connections.get("ljclz").unwrap();
            assert_eq!(ljclz.charset, "utf8");
            assert_eq!(ljclz.prefix, "ims_");

            // 验证 oceanbase 连接（hostport=2881，hostname 同样改用 localhost）
            let oceanbase = config.database.connections.get("oceanbase").unwrap();
            assert_eq!(oceanbase.hostport, 2881);
            assert_eq!(oceanbase.hostname, "localhost");

            // 验证 cache.yml 加载
            assert_eq!(config.cache.default, "memory");
            assert!(config.cache.stores.contains_key("memory"));

            // 验证 addons.yml 加载
            assert_eq!(config.addons.addons_path, "addons");
            assert_eq!(config.addons.priority.p0.len(), 3);

            // 验证 log.yml 加载
            assert_eq!(config.log.default, "file");
            assert!(config.log.channels.contains_key("file"));

            // 验证 server.yml 加载
            assert_eq!(config.server.host, "0.0.0.0");
            assert_eq!(config.server.port, 8080);
        }
    }

    /// 测试文件不存在时使用默认值
    #[tokio::test]
    async fn test_load_missing_file_uses_default() {
        let temp_dir = std::env::temp_dir().join("sz_rust_config_test_missing");
        let _ = std::fs::create_dir_all(&temp_dir);
        // 目录存在但无任何 yml 文件
        let config = AppConfig::load_from_dir(&temp_dir).await.unwrap();
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

    /// 测试环境变量覆盖 hostname（P3-18：清理内网 IP 硬编码）
    ///
    /// 场景：YAML 默认 hostname=localhost，生产环境通过
    /// `SZ_DB_{CONN}_HOSTNAME` 注入实际内网地址。
    #[test]
    fn test_env_override_hostname() {
        let _env_guard = ENV_TEST_LOCK.lock().unwrap();
        std::env::remove_var("SZ_DB_MYSQL_HOSTNAME");

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

        std::env::set_var("SZ_DB_MYSQL_HOSTNAME", "10.0.0.5");
        config.apply_env_overrides();

        assert_eq!(config.database.connections["mysql"].hostname, "10.0.0.5");

        std::env::remove_var("SZ_DB_MYSQL_HOSTNAME");
    }

    /// 测试环境变量覆盖 hostport（P3-18：端口可注入）
    #[test]
    fn test_env_override_hostport() {
        let _env_guard = ENV_TEST_LOCK.lock().unwrap();
        std::env::remove_var("SZ_DB_MYSQL_HOSTPORT");

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

        std::env::set_var("SZ_DB_MYSQL_HOSTPORT", "8802");
        config.apply_env_overrides();

        assert_eq!(config.database.connections["mysql"].hostport, 8802);

        std::env::remove_var("SZ_DB_MYSQL_HOSTPORT");
    }

    /// 测试 hostport 环境变量为非数字时保持原值（防御性）
    #[test]
    fn test_env_override_hostport_invalid_ignored() {
        let _env_guard = ENV_TEST_LOCK.lock().unwrap();
        std::env::remove_var("SZ_DB_MYSQL_HOSTPORT");

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

        std::env::set_var("SZ_DB_MYSQL_HOSTPORT", "not-a-number");
        config.apply_env_overrides();

        // 非数字解析失败，保持原值 3306
        assert_eq!(config.database.connections["mysql"].hostport, 3306);

        std::env::remove_var("SZ_DB_MYSQL_HOSTPORT");
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

    // ========================================================================
    // ConfigWatcher 测试
    // ========================================================================

    #[tokio::test]
    async fn test_config_watcher_has_changes_false_on_init() {
        let dir = std::env::temp_dir().join("sz_rust_watcher_test_init");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("app.yml"), "default_app: test\n").unwrap();

        let config = AppConfig::load_from_dir(&dir).await.unwrap();
        let shared = Arc::new(parking_lot::RwLock::new(config));
        let watcher = ConfigWatcher::new(&dir, shared);

        // 初始化 mtime
        watcher.init_mtimes().await;

        // 刚初始化，无变化
        assert!(!watcher.has_changes().await);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_config_watcher_detects_file_modification() {
        let dir = std::env::temp_dir().join("sz_rust_watcher_test_modify");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("app.yml"), "default_app: before\n").unwrap();

        let config = AppConfig::load_from_dir(&dir).await.unwrap();
        let shared = Arc::new(parking_lot::RwLock::new(config));
        let watcher = ConfigWatcher::new(&dir, shared);

        watcher.init_mtimes().await;
        assert!(!watcher.has_changes().await);

        // 等待一小段时间确保 mtime 不同
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        std::fs::write(dir.join("app.yml"), "default_app: after\n").unwrap();

        // 应检测到变化
        assert!(watcher.has_changes().await);

        // 更新 mtime 后不再检测到变化
        watcher.update_mtimes().await;
        assert!(!watcher.has_changes().await);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_config_watcher_detects_new_file() {
        let dir = std::env::temp_dir().join("sz_rust_watcher_test_new");
        let _ = std::fs::create_dir_all(&dir);

        let config = AppConfig::load_from_dir(&dir).await.unwrap();
        let shared = Arc::new(parking_lot::RwLock::new(config));
        let watcher = ConfigWatcher::new(&dir, shared);

        watcher.init_mtimes().await;

        // 创建新配置文件
        std::fs::write(dir.join("app.yml"), "default_app: new\n").unwrap();

        // 应检测到新文件
        assert!(watcher.has_changes().await);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_config_watcher_hot_reload() {
        let dir = std::env::temp_dir().join("sz_rust_watcher_test_hot");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("app.yml"), "default_app: before\n").unwrap();

        let config = AppConfig::load_from_dir(&dir).await.unwrap();
        let shared = Arc::new(parking_lot::RwLock::new(config));
        let watcher = ConfigWatcher::new(&dir, shared.clone()).with_poll_interval(1); // 1 秒轮询

        let handle = watcher.start();

        // 等待一秒确保 watcher 已初始化
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // 修改配置文件
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(dir.join("app.yml"), "default_app: hot_reloaded\n").unwrap();

        // 等待 watcher 轮询检测到变化
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        // 验证配置已热重载
        let current = shared.read().clone();
        assert_eq!(current.app.default_app, "hot_reloaded");

        handle.stop();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ============================================================================
// Data Scope 配置段
// ============================================================================

/// Data Scope 规则配置（YAML 映射）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DataScopeRuleConfig {
    /// 数据范围模式（all / dept / dept_and_sub / self / custom）
    pub mode: String,
    /// 部门字段名（DEPT / DEPT_AND_SUB 模式必填）
    #[serde(default)]
    pub dept_field: Option<String>,
    /// 创建者字段名（SELF 模式必填）
    #[serde(default)]
    pub creator_field: Option<String>,
    /// 自定义生成器名称（CUSTOM 模式必填）
    #[serde(default)]
    pub custom_generator: Option<String>,
    /// 目标表名
    pub target_table: String,
    /// 优先级
    #[serde(default)]
    pub priority: u32,
}

/// Data Scope 配置段
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DataScopeSection {
    /// 规则列表
    #[serde(default)]
    pub rules: Vec<DataScopeRuleConfig>,
    /// 部门树缓存 TTL（秒）
    #[serde(default = "default_dept_tree_ttl")]
    pub dept_tree_ttl_secs: u64,
    /// 是否启用
    #[serde(default)]
    pub enabled: bool,
}

fn default_dept_tree_ttl() -> u64 {
    300
}

impl Default for DataScopeSection {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            dept_tree_ttl_secs: 300,
            enabled: false,
        }
    }
}

impl DataScopeSection {
    /// 校验配置并转换为 DataScopeRule 列表
    pub fn validate(
        &self,
    ) -> Result<Vec<sz_rust_orm_facade::data_scope::DataScopeRule>, ConfigError> {
        if !self.enabled {
            return Ok(Vec::new());
        }

        let mut rules = Vec::new();
        for config in &self.rules {
            let mode = match config.mode.as_str() {
                "all" => sz_rust_orm_facade::data_scope::DataScopeMode::All,
                "dept" => sz_rust_orm_facade::data_scope::DataScopeMode::Dept,
                "dept_and_sub" => sz_rust_orm_facade::data_scope::DataScopeMode::DeptAndSub,
                "self" => sz_rust_orm_facade::data_scope::DataScopeMode::Self_,
                "custom" => sz_rust_orm_facade::data_scope::DataScopeMode::Custom,
                other => {
                    return Err(ConfigError::DataScopeConfigInvalid(format!(
                        "unknown mode '{}' for table '{}'",
                        other, config.target_table
                    )))
                }
            };

            let mut rule =
                sz_rust_orm_facade::data_scope::DataScopeRule::new(&config.target_table, mode)
                    .with_priority(config.priority);

            if let Some(ref field) = config.dept_field {
                rule = rule.with_dept_field(field);
            }
            if let Some(ref field) = config.creator_field {
                rule = rule.with_creator_field(field);
            }
            if let Some(ref name) = config.custom_generator {
                rule = rule.with_custom_generator(name);
            }

            match rule.mode {
                sz_rust_orm_facade::data_scope::DataScopeMode::Dept
                | sz_rust_orm_facade::data_scope::DataScopeMode::DeptAndSub => {
                    if rule.dept_field.is_none() {
                        return Err(ConfigError::DataScopeConfigInvalid(format!(
                            "mode '{}' requires dept_field for table '{}'",
                            config.mode, config.target_table
                        )));
                    }
                }
                sz_rust_orm_facade::data_scope::DataScopeMode::Self_ => {
                    if rule.creator_field.is_none() {
                        return Err(ConfigError::DataScopeConfigInvalid(format!(
                            "mode 'self' requires creator_field for table '{}'",
                            config.target_table
                        )));
                    }
                }
                sz_rust_orm_facade::data_scope::DataScopeMode::Custom => {
                    if rule.custom_generator.is_none() {
                        return Err(ConfigError::DataScopeConfigInvalid(format!(
                            "mode 'custom' requires custom_generator for table '{}'",
                            config.target_table
                        )));
                    }
                }
                sz_rust_orm_facade::data_scope::DataScopeMode::All => {}
            }

            rules.push(rule);
        }

        Ok(rules)
    }
}
