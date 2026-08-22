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
    /// 是否信任代理头（安全修复 M-2，默认 false）
    ///
    /// `true`：信任 `X-Forwarded-For`（仅限可信反向代理之后部署）；
    /// `false`（默认）：不信任代理头，防客户端伪造绕过限流。
    pub trust_proxy_headers: bool,
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
            trust_proxy_headers: false,
        }
    }
}

impl RateLimitProductionConfig {
    /// 从环境变量读取
    ///
    /// - `SZ300_RATE_LIMIT_CAPACITY`：令牌桶容量（默认 2000）
    /// - `SZ300_RATE_LIMIT_REFILL`：每秒补充速率（默认 1000）
    /// - `SZ300_RATE_LIMIT_TRUST_PROXY`：是否信任代理头（默认 false，安全修复 M-2）
    pub fn from_env() -> Self {
        let capacity = std::env::var("SZ300_RATE_LIMIT_CAPACITY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2000);

        let refill_per_second = std::env::var("SZ300_RATE_LIMIT_REFILL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1000.0);

        let trust_proxy_headers = std::env::var("SZ300_RATE_LIMIT_TRUST_PROXY")
            .ok()
            .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
            .unwrap_or(false);

        if trust_proxy_headers {
            tracing::warn!(
                "RATE_LIMIT_TRUST_PROXY=true：信任 X-Forwarded-For（请确认部署在可信反向代理之后，否则限流可被伪造头绕过）"
            );
        }

        if std::env::var("SZ300_RATE_LIMIT_CAPACITY").is_err() {
            tracing::warn!("RATE_LIMIT_DEFAULT_CONFIG: 使用默认 capacity=2000");
        }

        Self {
            capacity,
            refill_per_second,
            trust_proxy_headers,
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

        // IP 白名单校验（支持具体 IP 与 CIDR 前缀匹配）
        if !self.allowed_ips.is_empty() {
            if let Some(ip) = client_ip {
                if self
                    .allowed_ips
                    .iter()
                    .any(|allowed| ip_matches(ip, allowed))
                {
                    return true;
                }
            }
        }

        false
    }
}

/// IP 匹配：精确相等或 CIDR 前缀匹配（v4 /v6 均支持）
fn ip_matches(ip: &str, allowed: &str) -> bool {
    if ip == allowed {
        return true;
    }
    let (cidr, prefix_str) = match allowed.split_once('/') {
        Some(pair) => pair,
        None => return false,
    };
    let Ok(addr) = ip.parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(net) = cidr.parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(bits) = prefix_str.parse::<u32>() else {
        return false;
    };
    match (addr, net, bits) {
        (std::net::IpAddr::V4(a), std::net::IpAddr::V4(n), b) if b <= 32 => {
            let mask = if b == 0 { 0 } else { u32::MAX << (32 - b) };
            (u32::from_be_bytes(a.octets()) & mask) == (u32::from_be_bytes(n.octets()) & mask)
        }
        (std::net::IpAddr::V6(a), std::net::IpAddr::V6(n), b) if b <= 128 => {
            let mask = if b == 0 { 0 } else { u128::MAX << (128 - b) };
            (u128::from_be_bytes(a.octets()) & mask) == (u128::from_be_bytes(n.octets()) & mask)
        }
        _ => false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 环境变量是进程级全局状态，所有读写 SZ300_* 的测试必须持有此锁串行执行。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_all_sz300_env() {
        let keys = [
            "SZ300_DB_PASSWORD",
            "SZ300_DB_HOST",
            "SZ300_DB_PORT",
            "SZ300_DB_NAME",
            "SZ300_DB_USER",
            "SZ300_SERVER_HOST",
            "SZ300_SERVER_PORT",
            "SZ300_PG_PASSWORD",
            "SZ300_PG_HOST",
            "SZ300_PG_PORT",
            "SZ300_PG_NAME",
            "SZ300_PG_USER",
            "SZ300_SHUTDOWN_TIMEOUT",
            "SZ300_MQTT_SHUTDOWN_TIMEOUT",
            "SZ300_FORCE_ABORT_ON_TIMEOUT",
            "SZ300_RATE_LIMIT_CAPACITY",
            "SZ300_RATE_LIMIT_REFILL",
            "SZ300_RATE_LIMIT_TRUST_PROXY",
            "SZ300_CIRCUIT_BREAKER_THRESHOLD",
            "SZ300_CIRCUIT_BREAKER_COOLDOWN",
            "SZ300_CIRCUIT_BREAKER_PROBE_REQUESTS",
            "SZ300_CIRCUIT_BREAKER_STAT_WINDOW",
            "SZ300_METRICS_ALLOWED_IPS",
            "SZ300_METRICS_BEARER_TOKEN",
            "SZ300_METRICS_AUTH_ENABLED",
            "SZ300_READINESS_CHECKS",
            "SZ300_HEALTH_CHECK_TIMEOUT",
            "SZ300_STORAGE_DRIVER",
            "SZ300_STORAGE_PATH",
        ];
        for k in keys {
            std::env::remove_var(k);
        }
    }

    // ---- DatabaseConfig / PgDatabaseConfig Debug 脱敏 ----

    #[test]
    fn database_config_debug_redacts_password() {
        let cfg = DatabaseConfig {
            host: "127.0.0.1".into(),
            port: 3306,
            database: "sz300".into(),
            username: "root".into(),
            password: "secret123".into(),
        };
        let dbg = format!("{:?}", cfg);
        assert!(dbg.contains("[REDACTED]"), "password 应脱敏: {dbg}");
        assert!(!dbg.contains("secret123"), "明文密码不应出现: {dbg}");
        assert!(dbg.contains("root"), "username 应可见: {dbg}");
    }

    #[test]
    fn pg_database_config_debug_redacts_password() {
        let cfg = PgDatabaseConfig {
            host: "127.0.0.1".into(),
            port: 5432,
            database: "sz300".into(),
            username: "postgres".into(),
            password: "pg_secret".into(),
        };
        let dbg = format!("{:?}", cfg);
        assert!(dbg.contains("[REDACTED]"), "password 应脱敏: {dbg}");
        assert!(!dbg.contains("pg_secret"), "明文密码不应出现: {dbg}");
    }

    // ---- load_config ----

    #[test]
    fn load_config_missing_password_returns_err() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        let result = load_config();
        assert!(result.is_err(), "缺少 SZ300_DB_PASSWORD 应返回错误");
    }

    #[test]
    fn load_config_defaults_with_password() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_DB_PASSWORD", "testpass");
        let cfg = load_config().expect("有密码应成功");
        assert_eq!(cfg.server.port, 8300);
        assert_eq!(cfg.server.host, "0.0.0.0");
        assert_eq!(cfg.database.host, "127.0.0.1");
        assert_eq!(cfg.database.port, 3306);
        assert_eq!(cfg.database.database, "sz300");
        assert_eq!(cfg.database.username, "root");
        assert_eq!(cfg.database.password, "testpass");
    }

    #[test]
    fn load_config_env_override() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_DB_PASSWORD", "p");
        std::env::set_var("SZ300_DB_HOST", "10.0.0.1");
        std::env::set_var("SZ300_DB_PORT", "3307");
        std::env::set_var("SZ300_DB_NAME", "mydb");
        std::env::set_var("SZ300_DB_USER", "admin");
        std::env::set_var("SZ300_SERVER_HOST", "127.0.0.1");
        std::env::set_var("SZ300_SERVER_PORT", "9000");
        let cfg = load_config().unwrap();
        assert_eq!(cfg.database.host, "10.0.0.1");
        assert_eq!(cfg.database.port, 3307);
        assert_eq!(cfg.database.database, "mydb");
        assert_eq!(cfg.database.username, "admin");
        assert_eq!(cfg.server.host, "127.0.0.1");
        assert_eq!(cfg.server.port, 9000);
    }

    #[test]
    fn load_config_invalid_port_falls_back() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_DB_PASSWORD", "p");
        std::env::set_var("SZ300_DB_PORT", "not-a-number");
        std::env::set_var("SZ300_SERVER_PORT", "bad");
        let cfg = load_config().unwrap();
        assert_eq!(cfg.database.port, 3306, "无效端口应回退默认");
        assert_eq!(cfg.server.port, 8300);
    }

    // ---- pg_config ----

    #[test]
    fn pg_config_missing_password_returns_err() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        assert!(pg_config().is_err());
    }

    #[test]
    fn pg_config_defaults_with_password() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_PG_PASSWORD", "pgpass");
        let cfg = pg_config().unwrap();
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 5432);
        assert_eq!(cfg.database, "sz300");
        assert_eq!(cfg.username, "postgres");
        assert_eq!(cfg.password, "pgpass");
    }

    #[test]
    fn pg_config_env_override() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_PG_PASSWORD", "p");
        std::env::set_var("SZ300_PG_HOST", "pg.example");
        std::env::set_var("SZ300_PG_PORT", "5433");
        std::env::set_var("SZ300_PG_NAME", "otherdb");
        std::env::set_var("SZ300_PG_USER", "pguser");
        let cfg = pg_config().unwrap();
        assert_eq!(cfg.host, "pg.example");
        assert_eq!(cfg.port, 5433);
        assert_eq!(cfg.database, "otherdb");
        assert_eq!(cfg.username, "pguser");
    }

    // ---- ShutdownConfig ----

    #[test]
    fn shutdown_config_default() {
        let cfg = ShutdownConfig::default();
        assert_eq!(cfg.shutdown_timeout, Duration::from_secs(30));
        assert!(cfg.mqtt_shutdown_timeout.is_none());
        assert!(cfg.force_abort_on_timeout);
    }

    #[test]
    fn shutdown_config_from_env_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        let cfg = ShutdownConfig::from_env();
        assert_eq!(cfg.shutdown_timeout, Duration::from_secs(30));
        assert!(cfg.mqtt_shutdown_timeout.is_none());
        assert!(cfg.force_abort_on_timeout);
    }

    #[test]
    fn shutdown_config_from_env_override() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_SHUTDOWN_TIMEOUT", "60");
        std::env::set_var("SZ300_MQTT_SHUTDOWN_TIMEOUT", "15");
        std::env::set_var("SZ300_FORCE_ABORT_ON_TIMEOUT", "false");
        let cfg = ShutdownConfig::from_env();
        assert_eq!(cfg.shutdown_timeout, Duration::from_secs(60));
        assert_eq!(cfg.mqtt_shutdown_timeout, Some(Duration::from_secs(15)));
        assert!(!cfg.force_abort_on_timeout);
    }

    #[test]
    fn shutdown_config_mqtt_timeout_fallback() {
        let cfg = ShutdownConfig {
            shutdown_timeout: Duration::from_secs(45),
            mqtt_shutdown_timeout: None,
            force_abort_on_timeout: true,
        };
        assert_eq!(cfg.mqtt_timeout(), Duration::from_secs(45));
    }

    #[test]
    fn shutdown_config_mqtt_timeout_explicit() {
        let cfg = ShutdownConfig {
            shutdown_timeout: Duration::from_secs(45),
            mqtt_shutdown_timeout: Some(Duration::from_secs(10)),
            force_abort_on_timeout: true,
        };
        assert_eq!(cfg.mqtt_timeout(), Duration::from_secs(10));
    }

    // ---- RateLimitProductionConfig ----

    #[test]
    fn rate_limit_config_default() {
        let cfg = RateLimitProductionConfig::default();
        assert_eq!(cfg.capacity, 2000);
        assert_eq!(cfg.refill_per_second, 1000.0);
        assert!(!cfg.trust_proxy_headers);
        assert!(cfg.exclude_paths.contains(&"/health".to_string()));
        assert!(cfg.exclude_paths.contains(&"/metrics".to_string()));
        assert_eq!(cfg.key_prefix, "sz300:rl");
    }

    #[test]
    fn rate_limit_config_from_env_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        let cfg = RateLimitProductionConfig::from_env();
        assert_eq!(cfg.capacity, 2000);
        assert_eq!(cfg.refill_per_second, 1000.0);
        assert!(!cfg.trust_proxy_headers);
    }

    #[test]
    fn rate_limit_config_from_env_override() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_RATE_LIMIT_CAPACITY", "5000");
        std::env::set_var("SZ300_RATE_LIMIT_REFILL", "2500.5");
        std::env::set_var("SZ300_RATE_LIMIT_TRUST_PROXY", "true");
        let cfg = RateLimitProductionConfig::from_env();
        assert_eq!(cfg.capacity, 5000);
        assert_eq!(cfg.refill_per_second, 2500.5);
        assert!(cfg.trust_proxy_headers);
    }

    #[test]
    fn rate_limit_config_trust_proxy_with_1() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_RATE_LIMIT_TRUST_PROXY", "1");
        let cfg = RateLimitProductionConfig::from_env();
        assert!(cfg.trust_proxy_headers);
    }

    // ---- CircuitBreakerProductionConfig ----

    #[test]
    fn circuit_breaker_config_default() {
        let cfg = CircuitBreakerProductionConfig::default();
        assert_eq!(cfg.error_threshold, 0.5);
        assert_eq!(cfg.cooldown, Duration::from_secs(10));
        assert_eq!(cfg.probe_requests, 5);
        assert_eq!(cfg.stat_window, Duration::from_secs(60));
    }

    #[test]
    fn circuit_breaker_config_from_env_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        let cfg = CircuitBreakerProductionConfig::from_env();
        assert_eq!(cfg.error_threshold, 0.5);
        assert_eq!(cfg.cooldown, Duration::from_secs(10));
    }

    #[test]
    fn circuit_breaker_config_from_env_override() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_CIRCUIT_BREAKER_THRESHOLD", "0.3");
        std::env::set_var("SZ300_CIRCUIT_BREAKER_COOLDOWN", "20");
        std::env::set_var("SZ300_CIRCUIT_BREAKER_PROBE_REQUESTS", "10");
        std::env::set_var("SZ300_CIRCUIT_BREAKER_STAT_WINDOW", "120");
        let cfg = CircuitBreakerProductionConfig::from_env();
        assert_eq!(cfg.error_threshold, 0.3);
        assert_eq!(cfg.cooldown, Duration::from_secs(20));
        assert_eq!(cfg.probe_requests, 10);
        assert_eq!(cfg.stat_window, Duration::from_secs(120));
    }

    #[test]
    fn circuit_breaker_config_validate_ok() {
        let cfg = CircuitBreakerProductionConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn circuit_breaker_config_validate_threshold_zero() {
        let cfg = CircuitBreakerProductionConfig {
            error_threshold: 0.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn circuit_breaker_config_validate_threshold_over_one() {
        let cfg = CircuitBreakerProductionConfig {
            error_threshold: 1.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn circuit_breaker_config_validate_threshold_one_ok() {
        let cfg = CircuitBreakerProductionConfig {
            error_threshold: 1.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn circuit_breaker_config_validate_zero_cooldown() {
        let cfg = CircuitBreakerProductionConfig {
            cooldown: Duration::ZERO,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn circuit_breaker_config_validate_zero_probe() {
        let cfg = CircuitBreakerProductionConfig {
            probe_requests: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn circuit_breaker_config_validate_zero_stat_window() {
        let cfg = CircuitBreakerProductionConfig {
            stat_window: Duration::ZERO,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    // ---- MetricsAuthConfig ----

    #[test]
    fn metrics_auth_config_default() {
        let cfg = MetricsAuthConfig::default();
        assert!(cfg.allowed_ips.is_empty());
        assert!(cfg.bearer_token.is_none());
        assert!(cfg.enabled);
    }

    #[test]
    fn metrics_auth_config_debug_redacts_token() {
        let cfg = MetricsAuthConfig {
            allowed_ips: vec!["10.0.0.1".into()],
            bearer_token: Some("secret-token".into()),
            enabled: true,
        };
        let dbg = format!("{:?}", cfg);
        assert!(dbg.contains("[REDACTED]"), "bearer_token 应脱敏: {dbg}");
        assert!(!dbg.contains("secret-token"), "明文 token 不应出现: {dbg}");
    }

    #[test]
    fn metrics_auth_config_from_env_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        let cfg = MetricsAuthConfig::from_env();
        assert!(cfg.allowed_ips.is_empty());
        assert!(cfg.bearer_token.is_none());
        assert!(cfg.enabled);
    }

    #[test]
    fn metrics_auth_config_from_env_override() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_METRICS_ALLOWED_IPS", "10.0.0.1, 10.0.0.2");
        std::env::set_var("SZ300_METRICS_BEARER_TOKEN", "abc123");
        std::env::set_var("SZ300_METRICS_AUTH_ENABLED", "false");
        let cfg = MetricsAuthConfig::from_env();
        assert_eq!(cfg.allowed_ips, vec!["10.0.0.1", "10.0.0.2"]);
        assert_eq!(cfg.bearer_token, Some("abc123".into()));
        assert!(!cfg.enabled);
    }

    #[test]
    fn metrics_auth_config_validate_production_no_auth() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        let cfg = MetricsAuthConfig::default();
        assert!(
            cfg.validate_production("production").is_err(),
            "生产环境无鉴权应报错"
        );
    }

    #[test]
    fn metrics_auth_config_validate_production_with_ip() {
        let cfg = MetricsAuthConfig {
            allowed_ips: vec!["10.0.0.1".into()],
            ..Default::default()
        };
        assert!(cfg.validate_production("production").is_ok());
    }

    #[test]
    fn metrics_auth_config_validate_production_with_token() {
        let cfg = MetricsAuthConfig {
            bearer_token: Some("t".into()),
            ..Default::default()
        };
        assert!(cfg.validate_production("production").is_ok());
    }

    #[test]
    fn metrics_auth_config_validate_non_production_ok() {
        let cfg = MetricsAuthConfig::default();
        assert!(cfg.validate_production("development").is_ok());
    }

    #[test]
    fn metrics_auth_config_validate_disabled_ok() {
        let cfg = MetricsAuthConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(cfg.validate_production("production").is_ok());
    }

    #[test]
    fn metrics_auth_is_allowed_disabled() {
        let cfg = MetricsAuthConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(cfg.is_allowed(None, None), "禁用时应允许所有请求");
    }

    #[test]
    fn metrics_auth_is_allowed_bearer_match() {
        let cfg = MetricsAuthConfig {
            bearer_token: Some("secret".into()),
            ..Default::default()
        };
        assert!(cfg.is_allowed(Some("Bearer secret"), None));
        assert!(cfg.is_allowed(Some("bearer secret"), None));
    }

    #[test]
    fn metrics_auth_is_allowed_bearer_mismatch() {
        let cfg = MetricsAuthConfig {
            bearer_token: Some("secret".into()),
            ..Default::default()
        };
        assert!(!cfg.is_allowed(Some("Bearer wrong"), None));
        assert!(!cfg.is_allowed(Some("no-prefix"), None));
    }

    #[test]
    fn metrics_auth_is_allowed_ip_exact() {
        let cfg = MetricsAuthConfig {
            allowed_ips: vec!["10.0.0.1".into()],
            ..Default::default()
        };
        assert!(cfg.is_allowed(None, Some("10.0.0.1")));
        assert!(!cfg.is_allowed(None, Some("10.0.0.2")));
    }

    #[test]
    fn metrics_auth_is_allowed_ip_cidr_v4() {
        let cfg = MetricsAuthConfig {
            allowed_ips: vec!["10.0.0.0/24".into()],
            ..Default::default()
        };
        assert!(cfg.is_allowed(None, Some("10.0.0.100")));
        assert!(!cfg.is_allowed(None, Some("10.0.1.1")));
    }

    #[test]
    fn metrics_auth_is_allowed_ip_cidr_v6() {
        let cfg = MetricsAuthConfig {
            allowed_ips: vec!["2001:db8::/32".into()],
            ..Default::default()
        };
        assert!(cfg.is_allowed(None, Some("2001:db8:1::1")));
        assert!(!cfg.is_allowed(None, Some("2001:db9::1")));
    }

    #[test]
    fn metrics_auth_is_allowed_no_credentials() {
        let cfg = MetricsAuthConfig::default();
        assert!(!cfg.is_allowed(None, None), "启用但无任何鉴权配置应拒绝");
    }

    #[test]
    fn metrics_auth_is_allowed_invalid_ip() {
        let cfg = MetricsAuthConfig {
            allowed_ips: vec!["10.0.0.0/24".into()],
            ..Default::default()
        };
        assert!(!cfg.is_allowed(None, Some("not-an-ip")));
    }

    #[test]
    fn metrics_auth_is_allowed_invalid_cidr() {
        let cfg = MetricsAuthConfig {
            allowed_ips: vec!["bad-cidr/notbits".into()],
            ..Default::default()
        };
        assert!(!cfg.is_allowed(None, Some("10.0.0.1")));
    }

    // ---- HealthCheckConfig ----

    #[test]
    fn health_check_config_default() {
        let cfg = HealthCheckConfig::default();
        assert_eq!(cfg.readiness_checks, vec!["db".to_string()]);
        assert_eq!(cfg.check_timeout, Duration::from_secs(2));
        assert!(!cfg.liveness_check_dependencies);
    }

    #[test]
    fn health_check_config_from_env_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        let cfg = HealthCheckConfig::from_env();
        assert_eq!(cfg.readiness_checks, vec!["db".to_string()]);
        assert_eq!(cfg.check_timeout, Duration::from_secs(2));
    }

    #[test]
    fn health_check_config_from_env_override() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_READINESS_CHECKS", "db, redis, mqtt, invalid");
        std::env::set_var("SZ300_HEALTH_CHECK_TIMEOUT", "5");
        let cfg = HealthCheckConfig::from_env();
        assert_eq!(cfg.readiness_checks, vec!["db", "redis", "mqtt"]);
        assert_eq!(cfg.check_timeout, Duration::from_secs(5));
    }

    #[test]
    fn health_check_config_from_env_empty_checks() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_READINESS_CHECKS", "invalid,unknown");
        let cfg = HealthCheckConfig::from_env();
        assert_eq!(
            cfg.readiness_checks,
            vec!["db".to_string()],
            "全无效项应回退默认"
        );
    }

    #[test]
    fn health_check_should_check() {
        let cfg = HealthCheckConfig {
            readiness_checks: vec!["db".into(), "redis".into()],
            ..Default::default()
        };
        assert!(cfg.should_check("db"));
        assert!(cfg.should_check("redis"));
        assert!(!cfg.should_check("mqtt"));
    }

    // ---- StorageConfig ----

    #[test]
    fn storage_config_default() {
        let cfg = StorageConfig::default();
        assert_eq!(cfg.driver, "local");
        assert_eq!(cfg.path, "./uploads");
    }

    #[test]
    fn storage_config_from_env_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        let cfg = StorageConfig::from_env();
        assert_eq!(cfg.driver, "local");
        assert_eq!(cfg.path, "./uploads");
    }

    #[test]
    fn storage_config_from_env_override() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_all_sz300_env();
        std::env::set_var("SZ300_STORAGE_DRIVER", "aliyun");
        std::env::set_var("SZ300_STORAGE_PATH", "/data/uploads");
        let cfg = StorageConfig::from_env();
        assert_eq!(cfg.driver, "aliyun");
        assert_eq!(cfg.path, "/data/uploads");
    }
}
