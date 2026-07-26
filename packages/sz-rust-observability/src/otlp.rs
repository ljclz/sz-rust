//! OTLP exporter — 导出 traces/metrics 到 OpenTelemetry Collector
//!
//! 启用方式：在 sz300 的 Cargo.toml 中启用 `otlp` feature
//! 配置方式：通过环境变量或 `OtlpConfig` 结构指定 Collector 地址
//!
//! ## 支持的协议
//!
//! - **gRPC**（默认）：通过 tonic 客户端导出，端口 4317
//! - **HTTP**：通过 reqwest 客户端导出，端口 4318（需启用 `otlp-http` feature）
//!
//! ## 资源属性
//!
//! 自动附加以下资源属性（OpenTelemetry 语义约定）：
//! - `service.name` — 服务名称
//! - `service.version` — 服务版本
//! - `service.instance.id` — 实例 ID（自动生成 UUID）
//! - `host.name` — 主机名
//! - `deployment.environment` — 部署环境（从 `OTEL_SERVICE_ENV` 读取）
//!
//! ## 配置
//!
//! 通过环境变量配置（对齐 OpenTelemetry 规范）：
//! - `OTEL_EXPORTER_OTLP_ENDPOINT` — Collector 端点（默认 `http://localhost:4317`）
//! - `OTEL_EXPORTER_OTLP_PROTOCOL` — 协议（`grpc` / `http/protobuf`，默认 `grpc`）
//! - `OTEL_EXPORTER_OTLP_TIMEOUT` — 超时（毫秒，默认 5000）
//! - `OTEL_SERVICE_NAME` — 服务名称（默认 `sz300-server`）
//! - `OTEL_SERVICE_ENV` — 部署环境（默认 `development`）
//! - `OTEL_RESOURCE_ATTRIBUTES` — 额外资源属性（key=value,key=value）
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_observability::otlp::{OtlpConfig, init_otlp_tracer};
//!
//! let config = OtlpConfig::from_env();
//! init_otlp_tracer(&config).expect("OTLP 初始化失败");
//! ```

#![cfg(feature = "otlp")]

use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::runtime::Tokio;
use opentelemetry_sdk::trace::TracerProvider;
use std::sync::Once;
use std::time::Duration;

static INIT: Once = Once::new();

/// OTLP 传输协议
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtlpProtocol {
    /// gRPC 协议（默认，端口 4317）
    Grpc,
    /// HTTP/protobuf 协议（端口 4318）
    HttpProtobuf,
}

impl Default for OtlpProtocol {
    fn default() -> Self {
        Self::Grpc
    }
}

impl OtlpProtocol {
    /// 从环境变量 `OTEL_EXPORTER_OTLP_PROTOCOL` 解析协议
    ///
    /// 支持的值（大小写不敏感）：
    /// - `grpc` → `Grpc`
    /// - `http/protobuf` / `http` → `HttpProtobuf`
    pub fn from_env() -> Self {
        match std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "http/protobuf" | "http" => Self::HttpProtobuf,
            _ => Self::Grpc,
        }
    }

    /// 返回协议的默认端口号
    pub fn default_port(&self) -> u16 {
        match self {
            Self::Grpc => 4317,
            Self::HttpProtobuf => 4318,
        }
    }
}

/// OTLP 配置
#[derive(Debug, Clone)]
pub struct OtlpConfig {
    /// OTLP Collector 端点地址（默认 `http://localhost:4317`）
    pub endpoint: String,
    /// 传输协议
    pub protocol: OtlpProtocol,
    /// 服务名称
    pub service_name: String,
    /// 服务版本
    pub service_version: String,
    /// 服务实例 ID（不设置则自动生成）
    pub service_instance_id: Option<String>,
    /// 主机名
    pub host_name: Option<String>,
    /// 部署环境（如 `production` / `staging` / `development`）
    pub deployment_environment: String,
    /// 导出超时（毫秒）
    pub timeout_ms: u64,
    /// 额外的资源属性
    pub extra_resource_attributes: Vec<(String, String)>,
}

impl Default for OtlpConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl OtlpConfig {
    /// 从环境变量构造配置（对齐 OpenTelemetry 规范）
    ///
    /// 读取的环境变量：
    /// - `OTEL_EXPORTER_OTLP_ENDPOINT`（默认 `http://localhost:4317`）
    /// - `OTEL_EXPORTER_OTLP_PROTOCOL`（默认 `grpc`）
    /// - `OTEL_EXPORTER_OTLP_TIMEOUT`（默认 5000）
    /// - `OTEL_SERVICE_NAME`（默认 `sz300-server`）
    /// - `OTEL_SERVICE_ENV`（默认 `development`）
    /// - `OTEL_RESOURCE_ATTRIBUTES`（key=value,key=value 格式）
    pub fn from_env() -> Self {
        let protocol = OtlpProtocol::from_env();
        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").unwrap_or_else(|_| {
            format!("http://localhost:{}", protocol.default_port())
        });
        let timeout_ms = std::env::var("OTEL_EXPORTER_OTLP_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5000);
        let service_name =
            std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "sz300-server".to_string());
        let deployment_environment =
            std::env::var("OTEL_SERVICE_ENV").unwrap_or_else(|_| "development".to_string());

        let extra_resource_attributes = parse_resource_attributes(
            &std::env::var("OTEL_RESOURCE_ATTRIBUTES").unwrap_or_default(),
        );

        Self {
            endpoint,
            protocol,
            service_name,
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            service_instance_id: None,
            host_name: hostname(),
            deployment_environment,
            timeout_ms,
            extra_resource_attributes,
        }
    }

    /// 设置端点
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// 设置协议
    pub fn with_protocol(mut self, protocol: OtlpProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// 设置服务名称
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// 设置服务实例 ID
    pub fn with_service_instance_id(mut self, id: impl Into<String>) -> Self {
        self.service_instance_id = Some(id.into());
        self
    }

    /// 设置部署环境
    pub fn with_deployment_environment(mut self, env: impl Into<String>) -> Self {
        self.deployment_environment = env.into();
        self
    }

    /// 设置超时
    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// 添加额外的资源属性
    pub fn with_resource_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_resource_attributes.push((key.into(), value.into()));
        self
    }

    /// 构建资源属性（OpenTelemetry 语义约定）
    fn build_resource(&self) -> Resource {
        let mut kvs: Vec<KeyValue> = vec![
            KeyValue::new("service.name", self.service_name.clone()),
            KeyValue::new("service.version", self.service_version.clone()),
            KeyValue::new("deployment.environment", self.deployment_environment.clone()),
        ];

        if let Some(instance_id) = &self.service_instance_id {
            kvs.push(KeyValue::new("service.instance.id", instance_id.clone()));
        } else {
            // 自动生成实例 ID（使用进程 ID + 启动时间戳）
            let instance_id = format!("pid-{}-{}", std::process::id(), startup_timestamp());
            kvs.push(KeyValue::new("service.instance.id", instance_id));
        }

        if let Some(host) = &self.host_name {
            kvs.push(KeyValue::new("host.name", host.clone()));
        }

        for (key, value) in &self.extra_resource_attributes {
            kvs.push(KeyValue::new(key.clone(), value.clone()));
        }

        Resource::new(kvs)
    }

    /// 构建 gRPC SpanExporter
    fn build_grpc_exporter(&self) -> Result<SpanExporter, Box<dyn std::error::Error + Send + Sync>> {
        let exporter = SpanExporter::builder()
            .with_tonic()
            .with_endpoint(self.endpoint.clone())
            .with_timeout(Duration::from_millis(self.timeout_ms))
            .build()?;
        Ok(exporter)
    }

    /// 构建 HTTP/protobuf SpanExporter
    #[cfg(feature = "otlp-http")]
    fn build_http_exporter(&self) -> Result<SpanExporter, Box<dyn std::error::Error + Send + Sync>> {
        let exporter = SpanExporter::builder()
            .with_http()
            .with_endpoint(self.endpoint.clone())
            .with_timeout(Duration::from_millis(self.timeout_ms))
            .build()?;
        Ok(exporter)
    }

    /// 根据 `protocol` 构建对应类型的 SpanExporter
    fn build_exporter(&self) -> Result<SpanExporter, Box<dyn std::error::Error + Send + Sync>> {
        match self.protocol {
            OtlpProtocol::Grpc => self.build_grpc_exporter(),
            #[cfg(feature = "otlp-http")]
            OtlpProtocol::HttpProtobuf => self.build_http_exporter(),
            #[cfg(not(feature = "otlp-http"))]
            OtlpProtocol::HttpProtobuf => {
                // 未启用 otlp-http feature 时回退到 gRPC
                tracing::warn!(
                    "OTLP HTTP 协议未启用（缺少 otlp-http feature），回退到 gRPC"
                );
                self.build_grpc_exporter()
            }
        }
    }
}

/// 初始化 OTLP tracer provider
///
/// 调用后，全局 tracer 将通过 OTLP 导出 spans 到 Collector。
/// 此函数只能初始化一次（使用 `std::sync::Once` 保证）。
///
/// # 错误
///
/// - exporter 构建失败时返回错误
/// - 注意：由于使用 `Once`，若首次调用失败，后续调用不会重试，将返回 `Ok(())`
///
/// # 用法
///
/// ```ignore
/// use sz_rust_observability::otlp::{OtlpConfig, init_otlp_tracer};
///
/// let config = OtlpConfig::from_env();
/// init_otlp_tracer(&config).expect("OTLP 初始化失败");
/// ```
pub fn init_otlp_tracer(
    config: &OtlpConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut result: Result<(), Box<dyn std::error::Error + Send + Sync>> = Ok(());

    INIT.call_once(|| {
        let resource = config.build_resource();

        match config.build_exporter() {
            Ok(exporter) => {
                let provider = TracerProvider::builder()
                    .with_batch_exporter(exporter, Tokio)
                    .with_resource(resource)
                    .build();
                global::set_tracer_provider(provider);
                tracing::info!(
                    endpoint = %config.endpoint,
                    protocol = ?config.protocol,
                    service_name = %config.service_name,
                    "OTLP tracer provider initialized"
                );
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    endpoint = %config.endpoint,
                    "OTLP exporter build failed"
                );
                result = Err(format!("OTLP tracer 初始化失败: {}", e).into());
            }
        }
    });

    result
}

/// 优雅关闭 OTLP exporter — 刷新未发送的 spans
///
/// 应在应用退出时调用，确保所有已采集的 span 已导出到 Collector。
///
/// # 用法
///
/// ```ignore
/// use sz_rust_observability::otlp::{init_otlp_tracer, shutdown_otlp, OtlpConfig};
///
/// init_otlp_tracer(&OtlpConfig::from_env()).unwrap();
/// // ... 应用运行 ...
/// shutdown_otlp();
/// ```
pub fn shutdown_otlp() {
    global::shutdown_tracer_provider();
}

/// 解析 `OTEL_RESOURCE_ATTRIBUTES` 环境变量
///
/// 格式：`key1=value1,key2=value2,...`
/// 空白符会被 trim，空条目会被跳过。
fn parse_resource_attributes(raw: &str) -> Vec<(String, String)> {
    raw.split(',')
        .filter_map(|pair| {
            let mut iter = pair.splitn(2, '=');
            let key = iter.next()?.trim();
            let value = iter.next()?.trim();
            if key.is_empty() || value.is_empty() {
                None
            } else {
                Some((key.to_string(), value.to_string()))
            }
        })
        .collect()
}

/// 获取主机名
///
/// 优先级：`OTEL_HOST_NAME` 环境变量 > `HOSTNAME`（Unix）/ `COMPUTERNAME`（Windows）
fn hostname() -> Option<String> {
    std::env::var("OTEL_HOST_NAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
}

/// 进程启动时间戳（秒）
fn startup_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// 环境变量测试串行化锁
    ///
    /// 由于 `cargo test` 默认并发运行测试，环境变量读写存在竞态条件。
    /// 此 Mutex 保证所有依赖环境变量的测试串行执行。
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> &'static Mutex<()> {
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    /// 清理所有 OTEL 相关环境变量
    fn cleanup_otel_env() {
        for var in [
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_EXPORTER_OTLP_PROTOCOL",
            "OTEL_EXPORTER_OTLP_TIMEOUT",
            "OTEL_SERVICE_NAME",
            "OTEL_SERVICE_ENV",
            "OTEL_RESOURCE_ATTRIBUTES",
            "OTEL_HOST_NAME",
        ] {
            std::env::remove_var(var);
        }
    }

    #[test]
    fn test_parse_resource_attributes_basic() {
        let attrs = parse_resource_attributes("key1=value1,key2=value2");
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0], ("key1".to_string(), "value1".to_string()));
        assert_eq!(attrs[1], ("key2".to_string(), "value2".to_string()));
    }

    #[test]
    fn test_parse_resource_attributes_with_spaces() {
        let attrs = parse_resource_attributes(" key1 = value1 , key2 = value2 ");
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0], ("key1".to_string(), "value1".to_string()));
        assert_eq!(attrs[1], ("key2".to_string(), "value2".to_string()));
    }

    #[test]
    fn test_parse_resource_attributes_empty() {
        assert!(parse_resource_attributes("").is_empty());
    }

    #[test]
    fn test_parse_resource_attributes_missing_value() {
        let attrs = parse_resource_attributes("key1=value1,key2,key3=value3");
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0], ("key1".to_string(), "value1".to_string()));
        assert_eq!(attrs[1], ("key3".to_string(), "value3".to_string()));
    }

    #[test]
    fn test_parse_resource_attributes_empty_value() {
        let attrs = parse_resource_attributes("key1=,key2=value2");
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0], ("key2".to_string(), "value2".to_string()));
    }

    #[test]
    fn test_otlp_protocol_default() {
        assert_eq!(OtlpProtocol::default(), OtlpProtocol::Grpc);
    }

    #[test]
    fn test_otlp_protocol_default_port() {
        assert_eq!(OtlpProtocol::Grpc.default_port(), 4317);
        assert_eq!(OtlpProtocol::HttpProtobuf.default_port(), 4318);
    }

    #[test]
    fn test_otlp_protocol_from_env_default() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        // 不设置环境变量时，默认为 gRPC
        assert_eq!(OtlpProtocol::from_env(), OtlpProtocol::Grpc);
    }

    #[test]
    fn test_otlp_protocol_from_env_grpc() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc");
        assert_eq!(OtlpProtocol::from_env(), OtlpProtocol::Grpc);
    }

    #[test]
    fn test_otlp_protocol_from_env_http() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf");
        assert_eq!(OtlpProtocol::from_env(), OtlpProtocol::HttpProtobuf);
    }

    #[test]
    fn test_otlp_protocol_from_env_http_short() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", "http");
        assert_eq!(OtlpProtocol::from_env(), OtlpProtocol::HttpProtobuf);
    }

    #[test]
    fn test_otlp_protocol_from_env_case_insensitive() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", "GRPC");
        assert_eq!(OtlpProtocol::from_env(), OtlpProtocol::Grpc);
    }

    #[test]
    fn test_otlp_protocol_from_env_invalid_falls_back_to_grpc() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", "invalid");
        assert_eq!(OtlpProtocol::from_env(), OtlpProtocol::Grpc);
    }

    #[test]
    fn test_otlp_config_default() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        let config = OtlpConfig::default();
        assert!(!config.endpoint.is_empty());
        assert!(!config.service_name.is_empty());
        assert!(!config.service_version.is_empty());
        assert!(!config.deployment_environment.is_empty());
        assert!(config.timeout_ms > 0);
    }

    #[test]
    fn test_otlp_config_builder_chain() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        let config = OtlpConfig::default()
            .with_endpoint("http://collector:4317")
            .with_protocol(OtlpProtocol::Grpc)
            .with_service_name("test-service")
            .with_service_instance_id("instance-1")
            .with_deployment_environment("staging")
            .with_timeout_ms(10000)
            .with_resource_attribute("team", "platform");

        assert_eq!(config.endpoint, "http://collector:4317");
        assert_eq!(config.protocol, OtlpProtocol::Grpc);
        assert_eq!(config.service_name, "test-service");
        assert_eq!(config.service_instance_id, Some("instance-1".to_string()));
        assert_eq!(config.deployment_environment, "staging");
        assert_eq!(config.timeout_ms, 10000);
        assert_eq!(config.extra_resource_attributes.len(), 1);
        assert_eq!(config.extra_resource_attributes[0], ("team".to_string(), "platform".to_string()));
    }

    #[test]
    fn test_otlp_config_build_resource_basic() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        let config = OtlpConfig::default()
            .with_service_name("my-service")
            .with_service_instance_id("instance-abc")
            .with_deployment_environment("production");

        let resource = config.build_resource();

        // Resource::iter() 返回 (&Key, &Value) 迭代器
        // 验证必填字段
        let service_name = resource
            .iter()
            .find(|(k, _)| k.as_str() == "service.name")
            .map(|(_, v)| v.as_str());
        assert_eq!(service_name.as_deref(), Some("my-service"));

        let env = resource
            .iter()
            .find(|(k, _)| k.as_str() == "deployment.environment")
            .map(|(_, v)| v.as_str());
        assert_eq!(env.as_deref(), Some("production"));

        let instance_id = resource
            .iter()
            .find(|(k, _)| k.as_str() == "service.instance.id")
            .map(|(_, v)| v.as_str());
        assert_eq!(instance_id.as_deref(), Some("instance-abc"));
    }

    #[test]
    fn test_otlp_config_build_resource_auto_instance_id() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        let config = OtlpConfig::default().with_service_name("auto-instance");
        let resource = config.build_resource();

        let instance_id = resource
            .iter()
            .find(|(k, _)| k.as_str() == "service.instance.id")
            .map(|(_, v)| v.as_str());

        assert!(instance_id.is_some(), "未设置 instance_id 时应自动生成");
        let id_str = instance_id.unwrap();
        assert!(
            id_str.starts_with("pid-"),
            "自动生成的 instance_id 应以 'pid-' 开头，实际: {}",
            id_str
        );
    }

    #[test]
    fn test_otlp_config_build_resource_extra_attributes() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        let config = OtlpConfig::default()
            .with_resource_attribute("team", "platform")
            .with_resource_attribute("region", "us-west-2");

        let resource = config.build_resource();

        let team = resource
            .iter()
            .find(|(k, _)| k.as_str() == "team")
            .map(|(_, v)| v.as_str());
        assert_eq!(team.as_deref(), Some("platform"));

        let region = resource
            .iter()
            .find(|(k, _)| k.as_str() == "region")
            .map(|(_, v)| v.as_str());
        assert_eq!(region.as_deref(), Some("us-west-2"));
    }

    #[test]
    fn test_otlp_config_from_env_reads_env_vars() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317");
        std::env::set_var("OTEL_EXPORTER_OTLP_TIMEOUT", "3000");
        std::env::set_var("OTEL_SERVICE_NAME", "env-service");
        std::env::set_var("OTEL_SERVICE_ENV", "staging");
        std::env::set_var(
            "OTEL_RESOURCE_ATTRIBUTES",
            "team=platform,region=us-west-2",
        );

        let config = OtlpConfig::from_env();

        assert_eq!(config.endpoint, "http://collector:4317");
        assert_eq!(config.timeout_ms, 3000);
        assert_eq!(config.service_name, "env-service");
        assert_eq!(config.deployment_environment, "staging");
        assert_eq!(config.extra_resource_attributes.len(), 2);
    }

    #[test]
    fn test_otlp_config_from_env_defaults() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();

        let config = OtlpConfig::from_env();

        assert_eq!(config.endpoint, "http://localhost:4317");
        assert_eq!(config.protocol, OtlpProtocol::Grpc);
        assert_eq!(config.timeout_ms, 5000);
        assert_eq!(config.service_name, "sz300-server");
        assert_eq!(config.deployment_environment, "development");
        assert!(config.extra_resource_attributes.is_empty());
    }

    #[test]
    fn test_otlp_config_from_env_invalid_timeout_falls_back() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();
        std::env::set_var("OTEL_EXPORTER_OTLP_TIMEOUT", "not-a-number");
        let config = OtlpConfig::from_env();
        assert_eq!(config.timeout_ms, 5000, "无效的超时值应回退到默认 5000ms");
    }

    #[test]
    fn test_hostname_returns_value_or_none() {
        // hostname() 在 Windows 上应返回 COMPUTERNAME，在 Unix 上调用 gethostname
        // 此测试仅验证不 panic
        let _ = hostname();
    }

    #[test]
    fn test_startup_timestamp_nonzero() {
        let ts = startup_timestamp();
        // 启动时间戳应大于 2024-01-01（1704067200）
        assert!(ts > 1704067200, "启动时间戳应大于 2024-01-01，实际: {}", ts);
    }
}
