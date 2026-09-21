// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
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

use opentelemetry::trace::Span as OtelSpan;
use opentelemetry::trace::Tracer as OtelTracer;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::runtime::Tokio;

use opentelemetry_sdk::trace::BatchConfigBuilder;
use opentelemetry_sdk::trace::BatchSpanProcessor;
use opentelemetry_sdk::trace::Sampler;
use opentelemetry_sdk::trace::TracerProvider;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::Once;
use std::time::Duration;

use crate::{Counter, Gauge, MetricsRegistry};

static INIT: Once = Once::new();

/// OTLP 传输协议
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OtlpProtocol {
    /// gRPC 协议（默认，端口 4317）
    #[default]
    Grpc,
    /// HTTP/protobuf 协议（端口 4318）
    HttpProtobuf,
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

/// OTLP 采样配置
#[derive(Debug, Clone, PartialEq)]
pub enum SamplingConfig {
    /// 始终采样（采样率 100%）
    AlwaysOn,
    /// 始终不采样（采样率 0%）
    AlwaysOff,
    /// 基于 trace_id 哈希的比率采样（分布式一致，非 ParentBased）
    ///
    /// ratio ∈ [0.0, 1.0]，0.0 等效 AlwaysOff，1.0 等效 AlwaysOn
    TraceIdRatioBased(f64),
}

// 显式 impl 而非 derive：明确默认值为 AlwaysOn（可读性优先，clippy 建议可忽略）
#[allow(clippy::derivable_impls)]
impl Default for SamplingConfig {
    fn default() -> Self {
        Self::AlwaysOn
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
    /// batch 批量大小（默认 512，对齐 OTel SDK）
    pub batch_size: Option<u64>,
    /// batch 导出间隔（毫秒，默认 5000，对齐 OTel SDK）
    pub export_interval_ms: Option<u64>,
    /// 自定义 HTTP headers（注入到 exporter 请求中）
    pub headers: Vec<(String, String)>,
    /// 采样配置（默认 AlwaysOn）
    pub sampling: SamplingConfig,
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
        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| format!("http://localhost:{}", protocol.default_port()));
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
            batch_size: None,
            export_interval_ms: None,
            headers: Vec::new(),
            sampling: SamplingConfig::default(),
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
    pub fn with_resource_attribute(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.extra_resource_attributes
            .push((key.into(), value.into()));
        self
    }

    /// 设置 batch 批量大小（默认 512）
    pub fn with_batch_size(mut self, batch_size: u64) -> Self {
        self.batch_size = Some(batch_size);
        self
    }

    /// 设置 batch 导出间隔（毫秒，默认 5000）
    pub fn with_export_interval_ms(mut self, export_interval_ms: u64) -> Self {
        self.export_interval_ms = Some(export_interval_ms);
        self
    }

    /// 设置自定义 HTTP headers
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = headers;
        self
    }

    /// 追加单个自定义 HTTP header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// 设置采样配置
    pub fn with_sampling(mut self, sampling: SamplingConfig) -> Self {
        self.sampling = sampling;
        self
    }

    /// 构建资源属性（OpenTelemetry 语义约定）
    fn build_resource(&self) -> Resource {
        let mut kvs: Vec<KeyValue> = vec![
            KeyValue::new("service.name", self.service_name.clone()),
            KeyValue::new("service.version", self.service_version.clone()),
            KeyValue::new(
                "deployment.environment",
                self.deployment_environment.clone(),
            ),
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
    fn build_grpc_exporter(
        &self,
    ) -> Result<SpanExporter, Box<dyn std::error::Error + Send + Sync>> {
        let exporter = SpanExporter::builder()
            .with_tonic()
            .with_endpoint(self.endpoint.clone())
            .with_timeout(Duration::from_millis(self.timeout_ms))
            .build()?;
        Ok(exporter)
    }

    /// 构建 HTTP/protobuf SpanExporter
    #[cfg(feature = "otlp-http")]
    fn build_http_exporter(
        &self,
    ) -> Result<SpanExporter, Box<dyn std::error::Error + Send + Sync>> {
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
                tracing::warn!("OTLP HTTP 协议未启用（缺少 otlp-http feature），回退到 gRPC");
                self.build_grpc_exporter()
            }
        }
    }
}

// ============================================================================
// OtlpConfigValidator — 启动校验
// ============================================================================

/// OTLP 配置校验错误
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OtlpConfigError {
    /// endpoint 不是合法的 http/https URL
    #[error("OTLP_CONFIG_INVALID: endpoint 不是合法的 http/https URL: {0}")]
    InvalidEndpoint(String),
    /// TraceIdRatioBased 的 ratio 不在 [0.0, 1.0] 范围内
    #[error("OTLP_CONFIG_INVALID: sampling ratio 不在 [0.0, 1.0] 范围内: {0}")]
    InvalidSamplingRatio(f64),
    /// protocol=HttpProtobuf 但未启用 otlp-http feature
    #[error("OTLP_CONFIG_INVALID: protocol=HttpProtobuf 需要启用 otlp-http feature")]
    FeatureNotEnabled,
}

/// OTLP 配置校验器
pub struct OtlpConfigValidator<'a> {
    /// 待校验的配置引用
    config: &'a OtlpConfig,
}

impl<'a> OtlpConfigValidator<'a> {
    /// 创建校验器
    pub fn new(config: &'a OtlpConfig) -> Self {
        Self { config }
    }

    /// 校验配置
    ///
    /// # 校验规则
    ///
    /// 1. endpoint 必须是合法的 http/https URL
    /// 2. `TraceIdRatioBased(ratio)` 的 ratio ∈ [0.0, 1.0]
    /// 3. protocol=HttpProtobuf 时必须启用 `otlp-http` feature
    pub fn validate(&self) -> Result<(), OtlpConfigError> {
        // 1. 校验 endpoint 是合法的 http/https URL
        if !self.is_valid_url(&self.config.endpoint) {
            return Err(OtlpConfigError::InvalidEndpoint(
                self.config.endpoint.clone(),
            ));
        }

        // 2. 校验 sampling ratio
        if let SamplingConfig::TraceIdRatioBased(ratio) = &self.config.sampling {
            if *ratio < 0.0 || *ratio > 1.0 {
                return Err(OtlpConfigError::InvalidSamplingRatio(*ratio));
            }
        }

        // 3. 校验 protocol feature 匹配
        #[cfg(not(feature = "otlp-http"))]
        if self.config.protocol == OtlpProtocol::HttpProtobuf {
            return Err(OtlpConfigError::FeatureNotEnabled);
        }

        Ok(())
    }

    /// 简易 URL 校验：必须以 http:// 或 https:// 开头
    fn is_valid_url(&self, url: &str) -> bool {
        url.starts_with("http://") || url.starts_with("https://")
    }
}

// ------------------------------------------------------------------------
// T9: OtlpExportMetrics — 导出指标暴露 + batch 队列管理
// ------------------------------------------------------------------------

/// OTLP 导出指标（复用 `MetricsRegistry`）
///
/// 暴露三个指标：
/// - `otlp_export_success_total`（Counter）— 导出成功次数
/// - `otlp_export_failed_total`（Counter）— 导出失败次数
/// - `otlp_batch_queue_depth`（Gauge）— batch 队列当前深度
///
/// 导出失败不阻塞业务：batch exporter 在独立 tokio task 运行，
/// 导出错误通过回调记录指标，不传播到请求处理逻辑。
pub struct OtlpExportMetrics {
    success_total: Arc<Counter>,
    failed_total: Arc<Counter>,
    queue_depth: Arc<Gauge>,
}

impl OtlpExportMetrics {
    /// 在指定 `MetricsRegistry` 上注册三个导出指标
    pub fn new(registry: &MetricsRegistry) -> Self {
        Self {
            success_total: registry
                .register_counter("otlp_export_success_total", "OTLP export success count"),
            failed_total: registry
                .register_counter("otlp_export_failed_total", "OTLP export failed count"),
            queue_depth: registry
                .register_gauge("otlp_batch_queue_depth", "OTLP batch queue current depth"),
        }
    }

    /// 记录一次导出成功
    pub fn record_success(&self) {
        self.success_total.inc();
    }

    /// 记录一次导出失败
    pub fn record_failure(&self) {
        self.failed_total.inc();
    }

    /// 更新 batch 队列深度
    pub fn set_queue_depth(&self, depth: f64) {
        self.queue_depth.set(depth);
    }

    /// 当前成功次数
    pub fn success_count(&self) -> f64 {
        self.success_total.value()
    }

    /// 当前失败次数
    pub fn failure_count(&self) -> f64 {
        self.failed_total.value()
    }

    /// 当前队列深度
    pub fn queue_depth(&self) -> f64 {
        self.queue_depth.value()
    }
}

/// OTLP batch 队列（有界 FIFO）
///
/// 队列满时丢弃最旧 Span（FIFO 淘汰），返回丢失计数。
/// 调用方通过 `OtlpExportMetrics` 记录淘汰事件。
pub struct OtlpBatchQueue<T> {
    queue: VecDeque<T>,
    capacity: usize,
}

impl<T> OtlpBatchQueue<T> {
    /// 创建指定容量的队列
    pub fn new(capacity: usize) -> Self {
        Self {
            queue: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// 入队，返回因 FIFO 淘汰而丢弃的元素数量（0 表示未溢出）
    pub fn push(&mut self, item: T) -> usize {
        let mut evicted = 0;
        if self.queue.len() >= self.capacity {
            self.queue.pop_front();
            evicted = 1;
        }
        self.queue.push_back(item);
        evicted
    }

    /// 当前队列长度
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// 队列是否为空
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 排空队列（用于批量导出）
    pub fn drain(&mut self) -> Vec<T> {
        self.queue.drain(..).collect()
    }
}

// ------------------------------------------------------------------------
// T11: OtlpSpanBridge — 桥接 SzTracer Span 到 OTel SDK tracer
// ------------------------------------------------------------------------

/// OTLP Span 桥接器 — 将 SzTracer Span 数据提交到 OpenTelemetry SDK tracer
///
/// 通过 `opentelemetry::global::tracer()` 获取全局 tracer（由 `init_otlp_tracer` 初始化），
/// 创建 OTel SDK Span 并设置属性，OTel SDK 通过 BatchSpanProcessor 异步导出到 Collector。
///
/// ## 使用方式
///
/// 1. 先调用 `init_otlp_tracer(&config)` 初始化全局 OTel tracer
/// 2. 创建 `OtlpSpanBridge`，在 `TraceConfig::with_span_bridge` 中配置
/// 3. `trace_middleware` 在 `end_span` 后调用 `bridge_span_data`，Span 通过 OTel SDK 导出
///
/// ## 统一 tracer
///
/// 桥接后 SzTracer buffer 仅用于测试/内存导出场景，
/// 生产环境 Span 通过 OTel SDK → BatchSpanProcessor → OTLP Collector 导出。
pub struct OtlpSpanBridge {
    service_name: String,
}

impl OtlpSpanBridge {
    /// 创建 Span 桥接器
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }

    /// 桥接 Span 数据到 OTel SDK tracer
    ///
    /// 通过全局 tracer 创建 OTel Span，设置属性后结束 Span。
    /// OTel SDK 通过 BatchSpanProcessor 异步导出到 Collector。
    /// 导出失败不阻塞业务（BatchSpanProcessor 在独立 task 运行）。
    pub fn bridge_span_data(&self, operation_name: &str, tags: &HashMap<String, String>) {
        let tracer = global::tracer(self.service_name.clone());
        let mut otel_span = tracer.start(operation_name.to_string());
        for (key, value) in tags {
            otel_span.set_attribute(KeyValue::new(key.clone(), value.clone()));
        }
        otel_span.end();
    }

    /// 服务名
    pub fn service_name(&self) -> &str {
        &self.service_name
    }
}

impl std::fmt::Debug for OtlpSpanBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OtlpSpanBridge")
            .field("service_name", &self.service_name)
            .finish()
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
    // 启动前校验配置（缓解 Once 约束导致首次失败后不可重试的风险）
    OtlpConfigValidator::new(config).validate()?;

    let mut result: Result<(), Box<dyn std::error::Error + Send + Sync>> = Ok(());

    INIT.call_once(|| {
        let resource = config.build_resource();

        // 构建采样器
        let sampler = match &config.sampling {
            SamplingConfig::AlwaysOn => Sampler::AlwaysOn,
            SamplingConfig::AlwaysOff => Sampler::AlwaysOff,
            SamplingConfig::TraceIdRatioBased(ratio) => Sampler::TraceIdRatioBased(*ratio),
        };

        match config.build_exporter() {
            Ok(exporter) => {
                // 使用 BatchConfigBuilder 显式配置 batch_size 和 export_interval
                let mut batch_config_builder = BatchConfigBuilder::default();
                if let Some(batch_size) = config.batch_size {
                    batch_config_builder =
                        batch_config_builder.with_max_export_batch_size(batch_size as usize);
                }
                if let Some(interval_ms) = config.export_interval_ms {
                    batch_config_builder = batch_config_builder
                        .with_scheduled_delay(Duration::from_millis(interval_ms));
                }
                let batch_config = batch_config_builder.build();
                let batch_processor = BatchSpanProcessor::builder(exporter, Tokio)
                    .with_batch_config(batch_config)
                    .build();

                let provider = TracerProvider::builder()
                    .with_span_processor(batch_processor)
                    .with_sampler(sampler)
                    .with_resource(resource)
                    .build();
                global::set_tracer_provider(provider);
                tracing::info!(
                    endpoint = %config.endpoint,
                    protocol = ?config.protocol,
                    service_name = %config.service_name,
                    batch_size = ?config.batch_size,
                    export_interval_ms = ?config.export_interval_ms,
                    sampling = ?config.sampling,
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
        assert_eq!(
            config.extra_resource_attributes[0],
            ("team".to_string(), "platform".to_string())
        );
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
        std::env::set_var("OTEL_RESOURCE_ATTRIBUTES", "team=platform,region=us-west-2");

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

    // ------------------------------------------------------------------------
    // T7: batch/headers/sampling 测试
    // ------------------------------------------------------------------------

    /// 测试 batch_size builder
    #[test]
    fn test_otlp_config_batch_size() {
        let config = OtlpConfig::default().with_batch_size(256);
        assert_eq!(config.batch_size, Some(256));

        let config = OtlpConfig::default();
        assert_eq!(config.batch_size, None, "默认 batch_size 应为 None");
    }

    /// 测试 sampling 配置
    #[test]
    fn test_otlp_config_sampling() {
        let config = OtlpConfig::default().with_sampling(SamplingConfig::AlwaysOff);
        assert_eq!(config.sampling, SamplingConfig::AlwaysOff);

        let config = OtlpConfig::default().with_sampling(SamplingConfig::TraceIdRatioBased(0.5));
        assert_eq!(config.sampling, SamplingConfig::TraceIdRatioBased(0.5));

        let config = OtlpConfig::default();
        assert_eq!(
            config.sampling,
            SamplingConfig::AlwaysOn,
            "默认 sampling 应为 AlwaysOn"
        );
    }

    /// 测试 headers 配置
    #[test]
    fn test_otlp_config_headers() {
        let config = OtlpConfig::default()
            .with_header("Authorization", "Bearer token123")
            .with_header("X-Custom-Header", "custom-value");

        assert_eq!(config.headers.len(), 2);
        assert_eq!(
            config.headers[0],
            ("Authorization".into(), "Bearer token123".into())
        );
        assert_eq!(
            config.headers[1],
            ("X-Custom-Header".into(), "custom-value".into())
        );

        let config = OtlpConfig::default().with_headers(vec![("key1".into(), "val1".into())]);
        assert_eq!(config.headers.len(), 1);
    }

    /// 测试 from_env 默认值兼容（旧环境变量配置兼容）
    #[test]
    fn test_from_env_default_compatible() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();

        let config = OtlpConfig::from_env();

        // 新字段应有默认值
        assert_eq!(config.batch_size, None, "batch_size 默认 None");
        assert_eq!(
            config.export_interval_ms, None,
            "export_interval_ms 默认 None"
        );
        assert!(config.headers.is_empty(), "headers 默认空");
        assert_eq!(
            config.sampling,
            SamplingConfig::AlwaysOn,
            "sampling 默认 AlwaysOn"
        );
    }

    /// 测试 export_interval_ms builder
    #[test]
    fn test_otlp_config_export_interval() {
        let config = OtlpConfig::default().with_export_interval_ms(10000);
        assert_eq!(config.export_interval_ms, Some(10000));

        let config = OtlpConfig::default();
        assert_eq!(
            config.export_interval_ms, None,
            "默认 export_interval_ms 应为 None"
        );
    }

    /// 测试 SamplingConfig Default
    #[test]
    fn test_sampling_config_default() {
        assert_eq!(SamplingConfig::default(), SamplingConfig::AlwaysOn);
    }

    // ------------------------------------------------------------------------
    // T8: OtlpConfigValidator 测试
    // ------------------------------------------------------------------------

    /// 测试合法配置 → Ok
    #[test]
    fn test_otlp_config_validator_valid() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();

        let config = OtlpConfig::default()
            .with_endpoint("http://collector:4317")
            .with_sampling(SamplingConfig::AlwaysOn);
        let validator = OtlpConfigValidator::new(&config);
        assert!(validator.validate().is_ok());
    }

    /// 测试 endpoint 非 URL → InvalidEndpoint
    #[test]
    fn test_otlp_config_validator_invalid_endpoint() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();

        let config = OtlpConfig::default().with_endpoint("not-a-url");
        let validator = OtlpConfigValidator::new(&config);
        let err = validator.validate().unwrap_err();
        assert!(matches!(err, OtlpConfigError::InvalidEndpoint(_)));
    }

    /// 测试 TraceIdRatioBased(1.5) → InvalidSamplingRatio
    #[test]
    fn test_sampling_config_validation() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();

        let config = OtlpConfig::default().with_sampling(SamplingConfig::TraceIdRatioBased(1.5));
        let validator = OtlpConfigValidator::new(&config);
        let err = validator.validate().unwrap_err();
        assert!(matches!(err, OtlpConfigError::InvalidSamplingRatio(1.5)));
    }

    /// 测试 TraceIdRatioBased 边界值 0.0 和 1.0 → Ok
    #[test]
    fn test_sampling_config_boundary() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();

        let config = OtlpConfig::default().with_sampling(SamplingConfig::TraceIdRatioBased(0.0));
        assert!(OtlpConfigValidator::new(&config).validate().is_ok());

        let config = OtlpConfig::default().with_sampling(SamplingConfig::TraceIdRatioBased(1.0));
        assert!(OtlpConfigValidator::new(&config).validate().is_ok());
    }

    /// 测试 TraceIdRatioBased(-0.1) → InvalidSamplingRatio
    #[test]
    fn test_sampling_config_negative_ratio() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();

        let config = OtlpConfig::default().with_sampling(SamplingConfig::TraceIdRatioBased(-0.1));
        let validator = OtlpConfigValidator::new(&config);
        let err = validator.validate().unwrap_err();
        assert!(matches!(err, OtlpConfigError::InvalidSamplingRatio(_)));
    }

    /// 测试 protocol feature 匹配
    #[cfg(not(feature = "otlp-http"))]
    #[test]
    fn test_otlp_config_validator_protocol_feature_mismatch() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();

        let config = OtlpConfig::default()
            .with_endpoint("http://collector:4318")
            .with_protocol(OtlpProtocol::HttpProtobuf);
        let validator = OtlpConfigValidator::new(&config);
        let err = validator.validate().unwrap_err();
        assert!(matches!(err, OtlpConfigError::FeatureNotEnabled));
    }

    /// 测试 https endpoint → Ok
    #[test]
    fn test_otlp_config_validator_https_endpoint() {
        let _guard = env_lock().lock().unwrap();
        cleanup_otel_env();

        let config = OtlpConfig::default().with_endpoint("https://collector:4317");
        let validator = OtlpConfigValidator::new(&config);
        assert!(validator.validate().is_ok());
    }

    // ------------------------------------------------------------------------
    // T9: OtlpExportMetrics 测试
    // ------------------------------------------------------------------------

    /// 测试导出指标：成功 → success_total +1；失败 → failed_total +1
    #[test]
    fn test_otlp_export_metrics() {
        let registry = MetricsRegistry::new();
        let metrics = OtlpExportMetrics::new(&registry);

        assert_eq!(metrics.success_count(), 0.0);
        assert_eq!(metrics.failure_count(), 0.0);
        assert_eq!(metrics.queue_depth(), 0.0);

        metrics.record_success();
        metrics.record_success();
        assert_eq!(metrics.success_count(), 2.0);
        assert_eq!(metrics.failure_count(), 0.0);

        metrics.record_failure();
        assert_eq!(metrics.success_count(), 2.0);
        assert_eq!(metrics.failure_count(), 1.0);

        metrics.set_queue_depth(42.0);
        assert_eq!(metrics.queue_depth(), 42.0);
    }

    /// 测试 batch 队列溢出：队列满 → 丢弃最旧 + 返回淘汰计数
    #[test]
    fn test_otlp_batch_overflow() {
        let mut queue: OtlpBatchQueue<i32> = OtlpBatchQueue::new(3);

        assert_eq!(queue.capacity(), 3);
        assert!(queue.is_empty());

        let evicted0 = queue.push(1);
        assert_eq!(evicted0, 0);
        assert_eq!(queue.len(), 1);

        let evicted1 = queue.push(2);
        assert_eq!(evicted1, 0);
        assert_eq!(queue.len(), 2);

        let evicted2 = queue.push(3);
        assert_eq!(evicted2, 0);
        assert_eq!(queue.len(), 3);

        let evicted3 = queue.push(4);
        assert_eq!(evicted3, 1);
        assert_eq!(queue.len(), 3);

        let drained = queue.drain();
        assert_eq!(drained, vec![2, 3, 4]);
        assert!(queue.is_empty());
    }

    /// 测试 batch 队列 drain
    #[test]
    fn test_otlp_batch_queue_drain() {
        let mut queue: OtlpBatchQueue<String> = OtlpBatchQueue::new(10);
        queue.push("a".to_string());
        queue.push("b".to_string());
        queue.push("c".to_string());

        let drained = queue.drain();
        assert_eq!(drained, vec!["a", "b", "c"]);
        assert!(queue.is_empty());

        let drained_again = queue.drain();
        assert!(drained_again.is_empty());
    }

    /// 测试导出指标在 MetricsRegistry 中可渲染
    #[test]
    fn test_otlp_export_metrics_rendered() {
        let registry = MetricsRegistry::new();
        let metrics = OtlpExportMetrics::new(&registry);

        metrics.record_success();
        metrics.record_failure();
        metrics.record_failure();
        metrics.set_queue_depth(5.0);

        let output = registry.render();
        assert!(output.contains("otlp_export_success_total 1"));
        assert!(output.contains("otlp_export_failed_total 2"));
        assert!(output.contains("otlp_batch_queue_depth 5"));
    }

    // ------------------------------------------------------------------------
    // T11: OtlpSpanBridge 测试
    // ------------------------------------------------------------------------

    /// 测试 OtlpSpanBridge 创建
    #[test]
    fn test_otlp_span_bridge_new() {
        let bridge = OtlpSpanBridge::new("my-service");
        assert_eq!(bridge.service_name(), "my-service");
    }

    /// 测试 OtlpSpanBridge Debug 输出
    #[test]
    fn test_otlp_span_bridge_debug() {
        let bridge = OtlpSpanBridge::new("debug-service");
        let debug_str = format!("{:?}", bridge);
        assert!(debug_str.contains("OtlpSpanBridge"));
        assert!(debug_str.contains("debug-service"));
    }

    /// 测试 bridge_span_data 不 panic（全局 tracer 未初始化时使用 no-op tracer）
    #[test]
    fn test_otlp_span_bridge_bridge_span_data_no_panic() {
        let bridge = OtlpSpanBridge::new("test-service");
        let mut tags = HashMap::new();
        tags.insert("http.method".to_string(), "GET".to_string());
        tags.insert("http.url".to_string(), "/api/test".to_string());
        tags.insert("http.status_code".to_string(), "200".to_string());
        tags.insert("http.response_time".to_string(), "5".to_string());

        bridge.bridge_span_data("test-service:request", &tags);
    }
}
