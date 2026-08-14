use serde::Deserialize;
use std::fmt;
use std::time::Duration;

/// 应用根配置（聚合服务器与数据库配置）
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    /// HTTP 服务器配置
    pub server: ServerConfig,
    /// MySQL 数据库配置
    pub database: DatabaseConfig,
}

/// HTTP 服务器配置
#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    /// 监听端口
    pub port: u16,
    /// 监听地址
    pub host: String,
}

/// MySQL 数据库配置
#[derive(Deserialize, Clone)]
pub struct DatabaseConfig {
    /// 数据库主机地址
    pub host: String,
    /// 数据库端口
    pub port: u16,
    /// 数据库名
    pub database: String,
    /// 数据库用户名
    pub username: String,
    /// 数据库密码
    pub password: String,
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// PostgreSQL 数据库配置
#[derive(Deserialize, Clone)]
pub struct PgDatabaseConfig {
    /// 数据库主机地址
    pub host: String,
    /// 数据库端口
    pub port: u16,
    /// 数据库名
    pub database: String,
    /// 数据库用户名
    pub username: String,
    /// 数据库密码
    pub password: String,
}

impl fmt::Debug for PgDatabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgDatabaseConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// 从环境变量加载配置（生产安全要求：密钥不硬编码）
///
/// 环境变量：
/// - `SZ300_DB_HOST` (默认 127.0.0.1)
/// - `SZ300_DB_PORT` (默认 3306)
/// - `SZ300_DB_NAME` (默认 sz300)
/// - `SZ300_DB_USER` (默认 root)
/// - `SZ300_DB_PASSWORD` (必填)
/// - `SZ300_SERVER_HOST` (默认 0.0.0.0)
/// - `SZ300_SERVER_PORT` (默认 8300)
pub fn load_config() -> anyhow::Result<AppConfig> {
    let db_password = std::env::var("SZ300_DB_PASSWORD").map_err(|_| {
        anyhow::anyhow!("SZ300_DB_PASSWORD 环境变量未设置 — 请在启动前设置数据库密码")
    })?;

    Ok(AppConfig {
        server: ServerConfig {
            port: std::env::var("SZ300_SERVER_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8300),
            host: std::env::var("SZ300_SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
        },
        database: DatabaseConfig {
            host: std::env::var("SZ300_DB_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            port: std::env::var("SZ300_DB_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3306),
            database: std::env::var("SZ300_DB_NAME").unwrap_or_else(|_| "sz300".into()),
            username: std::env::var("SZ300_DB_USER").unwrap_or_else(|_| "root".into()),
            password: db_password,
        },
    })
}

/// PostgreSQL 连接配置（从环境变量读取）
///
/// 环境变量：
/// - `SZ300_PG_HOST` (默认 127.0.0.1)
/// - `SZ300_PG_PORT` (默认 5432)
/// - `SZ300_PG_NAME` (默认 sz300)
/// - `SZ300_PG_USER` (默认 postgres)
/// - `SZ300_PG_PASSWORD` (必填)
pub fn pg_config() -> anyhow::Result<PgDatabaseConfig> {
    let pg_password = std::env::var("SZ300_PG_PASSWORD").map_err(|_| {
        anyhow::anyhow!("SZ300_PG_PASSWORD 环境变量未设置 — 请在启动前设置 PostgreSQL 密码")
    })?;

    Ok(PgDatabaseConfig {
        host: std::env::var("SZ300_PG_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        port: std::env::var("SZ300_PG_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5432),
        database: std::env::var("SZ300_PG_NAME").unwrap_or_else(|_| "sz300".into()),
        username: std::env::var("SZ300_PG_USER").unwrap_or_else(|_| "postgres".into()),
        password: pg_password,
    })
}

// ============================================================================
// ShutdownConfig — 优雅关闭超时配置化
// ============================================================================

/// 优雅关闭配置
#[derive(Debug, Clone)]
pub struct ShutdownConfig {
    /// HTTP 服务器优雅关闭超时（默认 30s）
    pub shutdown_timeout: Duration,
    /// MQTT 消费者关闭超时（None 时取 shutdown_timeout）
    pub mqtt_shutdown_timeout: Option<Duration>,
    /// 超时后强制中止剩余任务
    pub force_abort_on_timeout: bool,
}

impl Default for ShutdownConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout: Duration::from_secs(30),
            mqtt_shutdown_timeout: None,
            force_abort_on_timeout: true,
        }
    }
}

impl ShutdownConfig {
    /// 从环境变量读取关闭配置
    ///
    /// - `SZ300_SHUTDOWN_TIMEOUT`：关闭超时（秒，默认 30）
    /// - `SZ300_MQTT_SHUTDOWN_TIMEOUT`：MQTT 关闭超时（秒，可选）
    /// - `SZ300_FORCE_ABORT_ON_TIMEOUT`：超时强制中止（默认 true）
    pub fn from_env() -> Self {
        let shutdown_timeout = std::env::var("SZ300_SHUTDOWN_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(30));

        let mqtt_shutdown_timeout = std::env::var("SZ300_MQTT_SHUTDOWN_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Duration::from_secs);

        let force_abort_on_timeout = std::env::var("SZ300_FORCE_ABORT_ON_TIMEOUT")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);

        Self {
            shutdown_timeout,
            mqtt_shutdown_timeout,
            force_abort_on_timeout,
        }
    }

    /// MQTT 关闭超时（未单独配置时取 shutdown_timeout）
    pub fn mqtt_timeout(&self) -> Duration {
        self.mqtt_shutdown_timeout.unwrap_or(self.shutdown_timeout)
    }
}

// ============================================================================
// RateLimitProductionConfig — 限流中间件生产配置（T4）
// ============================================================================

/// 限流生产配置
///
/// 阈值基于 v0.7.0 压测 sz-rust /hello 64 并发 RPS=157,526，
/// capacity=2000 约为峰值 1/75
#[derive(Debug, Clone)]
pub struct RateLimitProductionConfig {
    /// 令牌桶容量（默认 2000）
    pub capacity: u64,
    /// 每秒补充速率（默认 1000）
    pub refill_per_second: f64,
    /// 排除路径（健康检查 + metrics，不被限流）
    pub exclude_paths: Vec<String>,
    /// key 前缀
    pub key_prefix: String,
}

impl Default for RateLimitProductionConfig {
    fn default() -> Self {
        Self {
            capacity: 2000,
            refill_per_second: 1000.0,
            exclude_paths: vec![
                "/health".into(),
                "/health/ready".into(),
                "/health/startup".into(),
                "/metrics".into(),
            ],
            key_prefix: "sz300:rl".into(),
        }
    }
}

impl RateLimitProductionConfig {
    /// 从环境变量读取
    ///
    /// - `SZ300_RATE_LIMIT_CAPACITY`：令牌桶容量（默认 2000）
    /// - `SZ300_RATE_LIMIT_REFILL`：每秒补充速率（默认 1000）
    pub fn from_env() -> Self {
        let capacity = std::env::var("SZ300_RATE_LIMIT_CAPACITY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2000);

        let refill_per_second = std::env::var("SZ300_RATE_LIMIT_REFILL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1000.0);

        if std::env::var("SZ300_RATE_LIMIT_CAPACITY").is_err() {
            tracing::warn!("RATE_LIMIT_DEFAULT_CONFIG: 使用默认 capacity=2000");
        }

        Self {
            capacity,
            refill_per_second,
            ..Default::default()
        }
    }
}

// ============================================================================
// CircuitBreakerProductionConfig — 熔断中间件生产配置（T5）
// ============================================================================

/// 熔断生产配置
///
/// 基于 v0.7.0 压测错误率 0%、P99=1.79ms，
/// error_threshold=0.5（50% 错误率触发）
#[derive(Debug, Clone)]
pub struct CircuitBreakerProductionConfig {
    /// 错误率阈值（0.0-1.0，默认 0.5）
    pub error_threshold: f64,
    /// 冷却时间（默认 10s）
    pub cooldown: Duration,
    /// 半开探测请求数（默认 5）
    pub probe_requests: u32,
    /// 统计窗口（默认 60s）
    pub stat_window: Duration,
}

impl Default for CircuitBreakerProductionConfig {
    fn default() -> Self {
        Self {
            error_threshold: 0.5,
            cooldown: Duration::from_secs(10),
            probe_requests: 5,
            stat_window: Duration::from_secs(60),
        }
    }
}

impl CircuitBreakerProductionConfig {
    /// 从环境变量读取
    pub fn from_env() -> Self {
        let error_threshold = std::env::var("SZ300_CIRCUIT_BREAKER_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.5);

        let cooldown = std::env::var("SZ300_CIRCUIT_BREAKER_COOLDOWN")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(10));

        let probe_requests = std::env::var("SZ300_CIRCUIT_BREAKER_PROBE_REQUESTS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5);

        let stat_window = std::env::var("SZ300_CIRCUIT_BREAKER_STAT_WINDOW")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(60));

        if std::env::var("SZ300_CIRCUIT_BREAKER_THRESHOLD").is_err() {
            tracing::warn!("CIRCUIT_BREAKER_DEFAULT_CONFIG: 使用默认 error_threshold=0.5");
        }

        Self {
            error_threshold,
            cooldown,
            probe_requests,
            stat_window,
        }
    }

    /// 校验配置有效性
    pub fn validate(&self) -> Result<(), String> {
        if self.error_threshold <= 0.0 || self.error_threshold > 1.0 {
            return Err(format!(
                "error_threshold must be in (0, 1], got {}",
                self.error_threshold
            ));
        }
        if self.cooldown == Duration::ZERO {
            return Err("cooldown must be > 0".to_string());
        }
        if self.probe_requests == 0 {
            return Err("probe_requests must be > 0".to_string());
        }
        if self.stat_window == Duration::ZERO {
            return Err("stat_window must be > 0".to_string());
        }
        Ok(())
    }
}

// ============================================================================
// MetricsAuthConfig — metrics 端点访问控制配置（T7）
// ============================================================================

/// Metrics 鉴权配置
#[derive(Clone)]
pub struct MetricsAuthConfig {
    /// 允许的 IP 白名单（CIDR 或具体 IP）
    pub allowed_ips: Vec<String>,
    /// Bearer token 鉴权（可选）
    pub bearer_token: Option<String>,
    /// 是否启用鉴权（默认 true）
    pub enabled: bool,
}

impl fmt::Debug for MetricsAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetricsAuthConfig")
            .field("allowed_ips", &self.allowed_ips)
            .field("bearer_token", &"[REDACTED]")
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl Default for MetricsAuthConfig {
    fn default() -> Self {
        Self {
            allowed_ips: Vec::new(),
            bearer_token: None,
            enabled: true,
        }
    }
}

impl MetricsAuthConfig {
    /// 从环境变量读取
    ///
    /// - `SZ300_METRICS_ALLOWED_IPS`：逗号分隔的 IP 白名单
    /// - `SZ300_METRICS_BEARER_TOKEN`：Bearer token
    /// - `SZ300_METRICS_AUTH_ENABLED`：是否启用（默认 true）
    pub fn from_env() -> Self {
        let allowed_ips = std::env::var("SZ300_METRICS_ALLOWED_IPS")
            .ok()
            .map(|s| s.split(',').map(|ip| ip.trim().to_string()).collect())
            .unwrap_or_default();

        let bearer_token = std::env::var("SZ300_METRICS_BEARER_TOKEN").ok();

        let enabled = std::env::var("SZ300_METRICS_AUTH_ENABLED")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        Self {
            allowed_ips,
            bearer_token,
            enabled,
        }
    }

    /// 校验生产环境配置
    ///
    /// `env=production` 且 enabled=true 但未配置任何鉴权 → 返回错误
    pub fn validate_production(&self, env: &str) -> Result<(), String> {
        if env != "production" || !self.enabled {
            return Ok(());
        }
        if self.allowed_ips.is_empty() && self.bearer_token.is_none() {
            return Err(
                "生产环境 metrics 端点必须配置鉴权（IP 白名单或 Bearer token）".to_string(),
            );
        }
        Ok(())
    }

    /// 判断请求是否允许访问 metrics
    pub fn is_allowed(&self, bearer_header: Option<&str>, client_ip: Option<&str>) -> bool {
        if !self.enabled {
            return true;
        }

        // Bearer token 校验
        if let Some(ref expected_token) = self.bearer_token {
            if let Some(header) = bearer_header {
                let token = header
                    .strip_prefix("Bearer ")
                    .or_else(|| header.strip_prefix("bearer "))
                    .unwrap_or("");
                if token == expected_token {
                    return true;
                }
            }
        }

        // IP 白名单校验
        if !self.allowed_ips.is_empty() {
            if let Some(ip) = client_ip {
                if self.allowed_ips.iter().any(|allowed| allowed == ip) {
                    return true;
                }
            }
        }

        false
    }
}

// ============================================================================
// HealthCheckConfig — 健康检查端点配置化（T8）
// ============================================================================

/// 健康检查配置
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    /// readiness 检查项（可选：db/redis/mqtt，默认 ["db"]）
    pub readiness_checks: Vec<String>,
    /// 检查超时（默认 2s）
    pub check_timeout: Duration,
    /// liveness 是否检查依赖（默认 false，仅检查进程存活）
    pub liveness_check_dependencies: bool,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            readiness_checks: vec!["db".into()],
            check_timeout: Duration::from_secs(2),
            liveness_check_dependencies: false,
        }
    }
}

impl HealthCheckConfig {
    /// 从环境变量读取
    ///
    /// - `SZ300_READINESS_CHECKS`：逗号分隔（如 db,redis,mqtt）
    /// - `SZ300_HEALTH_CHECK_TIMEOUT`：检查超时秒数（默认 2）
    pub fn from_env() -> Self {
        let readiness_checks = std::env::var("SZ300_READINESS_CHECKS")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|c| c.trim().to_string())
                    .filter(|c| matches!(c.as_str(), "db" | "redis" | "mqtt"))
                    .collect::<Vec<_>>()
            })
            .filter(|v: &Vec<String>| !v.is_empty())
            .unwrap_or_else(|| vec!["db".into()]);

        let check_timeout = std::env::var("SZ300_HEALTH_CHECK_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(2));

        Self {
            readiness_checks,
            check_timeout,
            ..Default::default()
        }
    }

    /// 判断是否需要检查某项
    pub fn should_check(&self, item: &str) -> bool {
        self.readiness_checks.iter().any(|c| c == item)
    }
}

/// 存储配置（对齐 PHP config/filesystem.php）
///
/// 从环境变量读取存储驱动与路径配置：
/// - `SZ300_STORAGE_DRIVER`：存储驱动（local/aliyun/huawei，默认 local）
/// - `SZ300_STORAGE_PATH`：本地存储路径（默认 ./uploads）
#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// 存储驱动类型（local/aliyun/huawei）
    pub driver: String,
    /// 本地存储根路径
    pub path: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            driver: "local".to_string(),
            path: "./uploads".to_string(),
        }
    }
}

impl StorageConfig {
    /// 从环境变量加载存储配置
    pub fn from_env() -> Self {
        Self {
            driver: std::env::var("SZ300_STORAGE_DRIVER").unwrap_or_else(|_| "local".to_string()),
            path: std::env::var("SZ300_STORAGE_PATH").unwrap_or_else(|_| "./uploads".to_string()),
        }
    }
}
