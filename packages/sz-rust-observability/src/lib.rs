//! SZ-Rust 可观测性模块
//!
//! 提供 Prometheus 指标注册、SLO 燃烧率监控等能力，
//! 与 `sz-rust-core` 配合形成完整的可观测性闭环。
//!
//! # 核心能力
//!
//! ## 1. MetricsRegistry（默认启用）
//!
//! 统一的指标注册中心，支持 Counter / Gauge / Histogram 三种类型，
//! 内置线程安全（Counter/Gauge 使用 `AtomicU64`，Histogram 使用 `parking_lot::Mutex`），
//! 可通过 [`MetricsRegistry::render`] 输出 Prometheus 文本格式。
//!
//! ## 2. Prometheus exporter（feature = "prometheus"）
//!
//! 在指定端口暴露 `/metrics` HTTP 端点，供 Prometheus 拉取。
//!
//! ## 3. OTLP exporter（feature = "otlp"）
//!
//! 通过 OpenTelemetry OTLP 协议将 traces 导出到 Collector。
//!
//! ## 4. SLO 燃烧率
//!
//! 基于 Google SRE Workbook 第 5 章的 4 窗口多燃烧率告警策略
//! （Page 1h/5m + Ticket 6h/30m），详见 [`slo`] 模块。
//!
//! # 快速入门
//!
//! ```
//! use sz_rust_observability::{MetricsRegistry, MetricType};
//!
//! // 创建指标注册中心
//! let registry = MetricsRegistry::new();
//!
//! // 注册指标
//! let counter = registry.register_counter("sz_rust_requests_total", "Total requests");
//! let gauge = registry.register_gauge("sz_rust_active_connections", "Active connections");
//! let histogram = registry.register_histogram(
//!     "sz_rust_request_duration_seconds",
//!     "Request duration in seconds",
//!     vec![0.001, 0.01, 0.1, 1.0, 10.0],
//! );
//!
//! // 更新指标
//! counter.inc();
//! gauge.set(5.0);
//! histogram.observe(0.025);
//!
//! // 输出 Prometheus 文本格式
//! let output = registry.render();
//! println!("{}", output);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub mod slo;

pub use slo::{SloBurnRate, SloConfig, SloMonitor};

/// 指标类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricType {
    /// 单调递增计数器（如总请求数）
    Counter,
    /// 可增可减的瞬时值（如当前连接数）
    Gauge,
    /// 直方图（如请求延迟分布）
    Histogram,
}

impl MetricType {
    /// 返回 Prometheus 文本格式中的 TYPE 字符串
    fn as_str(&self) -> &'static str {
        match self {
            MetricType::Counter => "counter",
            MetricType::Gauge => "gauge",
            MetricType::Histogram => "histogram",
        }
    }
}

/// 指标元数据
#[derive(Debug, Clone)]
pub struct MetricMeta {
    /// 指标名（如 `sz_rust_requests_total`）
    pub name: String,
    /// 帮助文本
    pub help: String,
    /// 指标类型
    pub metric_type: MetricType,
}

/// 原子 f64 递增（基于 compare-exchange 循环）
fn atomic_f64_add(atom: &AtomicU64, delta: f64) {
    let mut current = atom.load(Ordering::Relaxed);
    loop {
        let new_bits = (f64::from_bits(current) + delta).to_bits();
        match atom.compare_exchange_weak(current, new_bits, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

/// 原子 f64 加载
fn atomic_f64_load(atom: &AtomicU64) -> f64 {
    f64::from_bits(atom.load(Ordering::Relaxed))
}

/// 计数器（单调递增）
///
/// 线程安全，内部使用 `AtomicU64` 存储值（通过 `f64::to_bits` / `f64::from_bits` 转换）。
pub struct Counter {
    name: String,
    help: String,
    value: AtomicU64,
    labels: HashMap<String, String>,
}

impl Counter {
    /// 递增 1
    pub fn inc(&self) {
        self.inc_by(1.0);
    }

    /// 递增指定值（`delta` 应为非负数）
    pub fn inc_by(&self, delta: f64) {
        atomic_f64_add(&self.value, delta);
    }

    /// 当前值
    pub fn value(&self) -> f64 {
        atomic_f64_load(&self.value)
    }

    /// 指标名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 帮助文本
    pub fn help(&self) -> &str {
        &self.help
    }

    /// 渲染为 Prometheus 文本格式（单行，不含 HELP/TYPE 头）
    pub fn render(&self) -> String {
        let v = self.value();
        render_metric_line(&self.name, &self.labels, v)
    }
}

/// Gauge（可增可减）
///
/// 线程安全，内部使用 `AtomicU64` 存储值。
pub struct Gauge {
    name: String,
    help: String,
    value: AtomicU64,
    labels: HashMap<String, String>,
}

impl Gauge {
    /// 设置值
    pub fn set(&self, value: f64) {
        self.value.store(value.to_bits(), Ordering::Relaxed);
    }

    /// 递增 1
    pub fn inc(&self) {
        self.inc_by(1.0);
    }

    /// 递增指定值
    pub fn inc_by(&self, delta: f64) {
        atomic_f64_add(&self.value, delta);
    }

    /// 递减指定值
    pub fn dec_by(&self, delta: f64) {
        atomic_f64_add(&self.value, -delta);
    }

    /// 当前值
    pub fn value(&self) -> f64 {
        atomic_f64_load(&self.value)
    }

    /// 指标名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 帮助文本
    pub fn help(&self) -> &str {
        &self.help
    }

    /// 渲染为 Prometheus 文本格式（单行，不含 HELP/TYPE 头）
    pub fn render(&self) -> String {
        let v = self.value();
        render_metric_line(&self.name, &self.labels, v)
    }
}

/// 直方图内部状态
struct HistogramState {
    /// 各 bucket 的累计计数（与 `buckets` 一一对应）
    counts: Vec<u64>,
    /// 所有观察值之和
    sum: f64,
    /// 总观察次数
    count: u64,
}

/// 直方图（延迟分布等）
///
/// 线程安全，内部使用 `parking_lot::Mutex` 保护状态。
/// bucket 边界在注册时自动排序并追加 `+Inf`。
pub struct Histogram {
    name: String,
    help: String,
    /// bucket 边界（已排序，末尾为 `+Inf`）
    buckets: Vec<f64>,
    state: Mutex<HistogramState>,
}

impl Histogram {
    /// 观察一个值
    ///
    /// 将值累加到所有 `le >= value` 的 bucket 中（累计直方图语义），
    /// 并更新 `_sum` 和 `_count`。
    pub fn observe(&self, value: f64) {
        let mut state = self.state.lock();
        for (i, bucket) in self.buckets.iter().enumerate() {
            if value <= *bucket {
                state.counts[i] += 1;
            }
        }
        state.sum += value;
        state.count += 1;
    }

    /// 总观察次数
    pub fn count(&self) -> u64 {
        self.state.lock().count
    }

    /// 所有观察值之和
    pub fn sum(&self) -> f64 {
        self.state.lock().sum
    }

    /// 指标名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 帮助文本
    pub fn help(&self) -> &str {
        &self.help
    }

    /// 渲染为 Prometheus 文本格式（含 `_bucket{le="..."}` / `_sum` / `_count`）
    pub fn render(&self) -> String {
        let state = self.state.lock();
        let mut output = String::new();
        for (i, bucket) in self.buckets.iter().enumerate() {
            let le = format_bucket_bound(*bucket);
            output.push_str(&format!(
                "{}_bucket{{le=\"{}\"}} {}\n",
                self.name, le, state.counts[i]
            ));
        }
        output.push_str(&format!("{}_sum {}\n", self.name, state.sum));
        output.push_str(&format!("{}_count {}\n", self.name, state.count));
        output
    }
}

/// 格式化 bucket 边界（`+Inf` 特殊处理）
fn format_bucket_bound(bound: f64) -> String {
    if bound.is_infinite() {
        "+Inf".to_string()
    } else {
        format!("{}", bound)
    }
}

/// 渲染单行指标（含可选 labels）
fn render_metric_line(name: &str, labels: &HashMap<String, String>, value: f64) -> String {
    if labels.is_empty() {
        format!("{} {}\n", name, value)
    } else {
        let parts: Vec<String> = labels
            .iter()
            .map(|(k, v)| format!("{}=\"{}\"", k, v.replace('"', "\\\"")))
            .collect();
        format!("{}{{{}}} {}\n", name, parts.join(","), value)
    }
}

/// 指标注册中心
///
/// 统一管理 Counter / Gauge / Histogram 指标，支持 Prometheus 文本格式输出。
/// 线程安全，可在多线程环境中共享。
pub struct MetricsRegistry {
    counters: Mutex<HashMap<String, Arc<Counter>>>,
    gauges: Mutex<HashMap<String, Arc<Gauge>>>,
    histograms: Mutex<HashMap<String, Arc<Histogram>>>,
    metas: Mutex<Vec<MetricMeta>>,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    /// 创建空注册中心
    pub fn new() -> Self {
        Self {
            counters: Mutex::new(HashMap::new()),
            gauges: Mutex::new(HashMap::new()),
            histograms: Mutex::new(HashMap::new()),
            metas: Mutex::new(Vec::new()),
        }
    }

    /// 注册 Counter（无标签）
    pub fn register_counter(&self, name: &str, help: &str) -> Arc<Counter> {
        self.register_counter_with_labels(name, help, HashMap::new())
    }

    /// 注册带标签的 Counter
    pub fn register_counter_with_labels(
        &self,
        name: &str,
        help: &str,
        labels: HashMap<String, String>,
    ) -> Arc<Counter> {
        let key = metric_key(name, &labels);
        let mut counters = self.counters.lock();
        if let Some(c) = counters.get(&key) {
            return c.clone();
        }
        let counter = Arc::new(Counter {
            name: name.to_string(),
            help: help.to_string(),
            value: AtomicU64::new(0.0f64.to_bits()),
            labels,
        });
        counters.insert(key, counter.clone());

        let mut metas = self.metas.lock();
        metas.push(MetricMeta {
            name: name.to_string(),
            help: help.to_string(),
            metric_type: MetricType::Counter,
        });
        counter
    }

    /// 注册 Gauge（无标签）
    pub fn register_gauge(&self, name: &str, help: &str) -> Arc<Gauge> {
        self.register_gauge_with_labels(name, help, HashMap::new())
    }

    /// 注册带标签的 Gauge
    pub fn register_gauge_with_labels(
        &self,
        name: &str,
        help: &str,
        labels: HashMap<String, String>,
    ) -> Arc<Gauge> {
        let key = metric_key(name, &labels);
        let mut gauges = self.gauges.lock();
        if let Some(g) = gauges.get(&key) {
            return g.clone();
        }
        let gauge = Arc::new(Gauge {
            name: name.to_string(),
            help: help.to_string(),
            value: AtomicU64::new(0.0f64.to_bits()),
            labels,
        });
        gauges.insert(key, gauge.clone());

        let mut metas = self.metas.lock();
        metas.push(MetricMeta {
            name: name.to_string(),
            help: help.to_string(),
            metric_type: MetricType::Gauge,
        });
        gauge
    }

    /// 注册 Histogram
    ///
    /// `buckets` 为 bucket 边界（如 `vec![0.001, 0.01, 0.1, 1.0]`），
    /// 会自动排序并追加 `+Inf` bucket。
    pub fn register_histogram(&self, name: &str, help: &str, buckets: Vec<f64>) -> Arc<Histogram> {
        let mut histograms = self.histograms.lock();
        if let Some(h) = histograms.get(name) {
            return h.clone();
        }
        // 排序 + 追加 +Inf
        let mut all_buckets = buckets;
        all_buckets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if !all_buckets.contains(&f64::INFINITY) {
            all_buckets.push(f64::INFINITY);
        }
        let count = all_buckets.len();
        let histogram = Arc::new(Histogram {
            name: name.to_string(),
            help: help.to_string(),
            buckets: all_buckets,
            state: Mutex::new(HistogramState {
                counts: vec![0; count],
                sum: 0.0,
                count: 0,
            }),
        });
        histograms.insert(name.to_string(), histogram.clone());

        let mut metas = self.metas.lock();
        metas.push(MetricMeta {
            name: name.to_string(),
            help: help.to_string(),
            metric_type: MetricType::Histogram,
        });
        histogram
    }

    /// 渲染所有指标为 Prometheus 文本格式
    ///
    /// 输出格式：
    /// ```text
    /// # HELP {name} {help}
    /// # TYPE {name} counter|gauge|histogram
    /// {metric_lines}
    /// ```
    pub fn render(&self) -> String {
        let mut output = String::new();

        // 输出 HELP/TYPE 头（去重）
        let metas = self.metas.lock();
        let mut seen: HashSet<&str> = HashSet::new();
        for meta in metas.iter() {
            if !seen.insert(meta.name.as_str()) {
                continue;
            }
            output.push_str(&format!("# HELP {} {}\n", meta.name, meta.help));
            output.push_str(&format!(
                "# TYPE {} {}\n",
                meta.name,
                meta.metric_type.as_str()
            ));
        }
        drop(metas);

        // 输出 Counter 值
        let counters = self.counters.lock();
        for c in counters.values() {
            output.push_str(&c.render());
        }
        drop(counters);

        // 输出 Gauge 值
        let gauges = self.gauges.lock();
        for g in gauges.values() {
            output.push_str(&g.render());
        }
        drop(gauges);

        // 输出 Histogram 值
        let histograms = self.histograms.lock();
        for h in histograms.values() {
            output.push_str(&h.render());
        }

        output
    }
}

/// 生成指标存储键（name + labels 序列化）
fn metric_key(name: &str, labels: &HashMap<String, String>) -> String {
    if labels.is_empty() {
        name.to_string()
    } else {
        // 按 key 排序保证键确定性
        let mut pairs: Vec<(&String, &String)> = labels.iter().collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        let parts: Vec<String> = pairs.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        format!("{}|{}", name, parts.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_basic() {
        let registry = MetricsRegistry::new();
        let counter = registry.register_counter("test_counter", "Test counter");
        counter.inc();
        counter.inc_by(2.5);
        assert!((counter.value() - 3.5).abs() < 1e-9);
    }

    #[test]
    fn test_counter_name_and_help() {
        let registry = MetricsRegistry::new();
        let counter = registry.register_counter("ops_total", "Total operations");
        assert_eq!(counter.name(), "ops_total");
        assert_eq!(counter.help(), "Total operations");
    }

    #[test]
    fn test_counter_with_labels() {
        let registry = MetricsRegistry::new();
        let mut labels = HashMap::new();
        labels.insert("method".to_string(), "GET".to_string());
        labels.insert("status".to_string(), "200".to_string());

        let counter =
            registry.register_counter_with_labels("http_requests_total", "HTTP requests", labels);
        counter.inc();
        let output = registry.render();
        assert!(output.contains("http_requests_total{"));
        assert!(output.contains("method=\"GET\""));
        assert!(output.contains("status=\"200\""));
        assert!(output.contains("} 1"));
    }

    #[test]
    fn test_gauge_basic() {
        let registry = MetricsRegistry::new();
        let gauge = registry.register_gauge("test_gauge", "Test gauge");
        gauge.set(10.0);
        gauge.inc();
        gauge.dec_by(3.0);
        assert!((gauge.value() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn test_gauge_inc_by_and_dec_by() {
        let registry = MetricsRegistry::new();
        let gauge = registry.register_gauge("temp_celsius", "Temperature");
        gauge.set(20.0);
        gauge.inc_by(5.5);
        assert!((gauge.value() - 25.5).abs() < 1e-9);
        gauge.dec_by(10.0);
        assert!((gauge.value() - 15.5).abs() < 1e-9);
    }

    #[test]
    fn test_gauge_with_labels() {
        let registry = MetricsRegistry::new();
        let mut labels = HashMap::new();
        labels.insert("host".to_string(), "node-1".to_string());
        let gauge = registry.register_gauge_with_labels("cpu_usage", "CPU usage ratio", labels);
        gauge.set(0.75);
        let output = registry.render();
        assert!(output.contains("cpu_usage{"));
        assert!(output.contains("host=\"node-1\""));
        assert!(output.contains("} 0.75"));
    }

    #[test]
    fn test_histogram_basic() {
        let registry = MetricsRegistry::new();
        let histogram =
            registry.register_histogram("test_histogram", "Test histogram", vec![0.1, 0.5, 1.0]);
        histogram.observe(0.05);
        histogram.observe(0.2);
        histogram.observe(0.6);
        histogram.observe(1.5);

        assert_eq!(histogram.count(), 4);
        assert!((histogram.sum() - 2.35).abs() < 1e-9);
    }

    #[test]
    fn test_histogram_inf_bucket_equals_count() {
        let registry = MetricsRegistry::new();
        let histogram = registry.register_histogram("latency", "Latency", vec![0.01, 0.1, 1.0]);
        histogram.observe(0.005);
        histogram.observe(0.05);
        histogram.observe(0.5);
        histogram.observe(5.0);

        let output = histogram.render();
        // +Inf bucket 必须等于 count（Prometheus 规范）
        assert!(output.contains("latency_bucket{le=\"+Inf\"} 4"));
        assert!(output.contains("latency_count 4"));
    }

    #[test]
    fn test_histogram_bucket_sorted() {
        let registry = MetricsRegistry::new();
        // 乱序输入
        let histogram =
            registry.register_histogram("sorted_hist", "Sorted", vec![1.0, 0.01, 0.1, 10.0]);
        let output = histogram.render();
        // 验证 bucket 顺序为升序
        let le_001 = output.find("le=\"0.01\"").unwrap();
        let le_01 = output.find("le=\"0.1\"").unwrap();
        let le_1 = output.find("le=\"1\"").unwrap();
        let le_10 = output.find("le=\"10\"").unwrap();
        let le_inf = output.find("le=\"+Inf\"").unwrap();
        assert!(le_001 < le_01);
        assert!(le_01 < le_1);
        assert!(le_1 < le_10);
        assert!(le_10 < le_inf);
    }

    #[test]
    fn test_render_prometheus_format() {
        let registry = MetricsRegistry::new();
        let counter = registry.register_counter("ops_total", "Total operations");
        let gauge = registry.register_gauge("conn_active", "Active connections");
        let histogram =
            registry.register_histogram("latency_seconds", "Latency in seconds", vec![0.01, 0.1]);

        counter.inc_by(10.0);
        gauge.set(5.0);
        histogram.observe(0.005);
        histogram.observe(0.05);
        histogram.observe(0.5);

        let output = registry.render();
        assert!(output.contains("# HELP ops_total Total operations"));
        assert!(output.contains("# TYPE ops_total counter"));
        assert!(output.contains("ops_total 10"));
        assert!(output.contains("# TYPE conn_active gauge"));
        assert!(output.contains("conn_active 5"));
        assert!(output.contains("# TYPE latency_seconds histogram"));
        assert!(output.contains("latency_seconds_bucket{le=\"0.01\"} 1"));
        assert!(output.contains("latency_seconds_bucket{le=\"0.1\"} 2"));
        assert!(output.contains("latency_seconds_bucket{le=\"+Inf\"} 3"));
        assert!(output.contains("latency_seconds_sum 0.555"));
        assert!(output.contains("latency_seconds_count 3"));
    }

    #[test]
    fn test_registry_duplicate_registration_returns_same_handle() {
        let registry = MetricsRegistry::new();
        let c1 = registry.register_counter("dup_counter", "Dup");
        let c2 = registry.register_counter("dup_counter", "Dup");
        // 同名同 labels 应返回同一实例
        assert!(Arc::ptr_eq(&c1, &c2));
        c1.inc_by(5.0);
        assert!((c2.value() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_metric_type_as_str() {
        assert_eq!(MetricType::Counter.as_str(), "counter");
        assert_eq!(MetricType::Gauge.as_str(), "gauge");
        assert_eq!(MetricType::Histogram.as_str(), "histogram");
    }

    #[test]
    fn test_metric_meta_fields() {
        let meta = MetricMeta {
            name: "test_metric".to_string(),
            help: "A test metric".to_string(),
            metric_type: MetricType::Counter,
        };
        assert_eq!(meta.name, "test_metric");
        assert_eq!(meta.help, "A test metric");
        assert_eq!(meta.metric_type, MetricType::Counter);
    }

    #[test]
    fn test_empty_registry_render() {
        let registry = MetricsRegistry::new();
        let output = registry.render();
        assert!(output.is_empty());
    }

    #[test]
    fn test_counter_thread_safety() {
        let registry = MetricsRegistry::new();
        let counter = registry.register_counter("threaded_counter", "Threaded");
        let counter_clone = counter.clone();
        let handle = std::thread::spawn(move || {
            for _ in 0..1000 {
                counter_clone.inc();
            }
        });
        for _ in 0..1000 {
            counter.inc();
        }
        handle.join().unwrap();
        assert!((counter.value() - 2000.0).abs() < 1e-9);
    }
}
