//! 可观测性指标集成测试
//!
//! 验证 `sz_rust_observability::MetricsRegistry` 的 Counter / Gauge / Histogram
//! 指标注册与 Prometheus 文本格式渲染行为。不依赖数据库连接。

use sz_rust_observability::MetricsRegistry;

#[test]
fn test_metrics_registry_render() {
    let registry = MetricsRegistry::new();
    let counter = registry.register_counter("test_requests_total", "Test counter");
    counter.inc();
    counter.inc_by(5.0);

    let output = registry.render();
    assert!(output.contains("test_requests_total"));
    assert!(output.contains("6")); // 1 + 5
}

#[test]
fn test_metrics_gauge() {
    let registry = MetricsRegistry::new();
    let gauge = registry.register_gauge("test_active", "Test gauge");
    gauge.set(42.0);

    let output = registry.render();
    assert!(output.contains("test_active"));
    assert!(output.contains("42"));
}

#[test]
fn test_metrics_histogram() {
    let registry = MetricsRegistry::new();
    let hist = registry.register_histogram(
        "test_duration",
        "Test histogram",
        vec![0.1, 1.0, 10.0],
    );
    hist.observe(0.5);
    hist.observe(2.0);

    let output = registry.render();
    assert!(output.contains("test_duration"));
    assert!(output.contains("count"));
}

#[test]
fn test_metrics_counter_help_and_type() {
    let registry = MetricsRegistry::new();
    let _counter = registry.register_counter("ops_total", "Total operations");
    let output = registry.render();
    // 验证 Prometheus 文本格式包含 HELP/TYPE 头
    assert!(output.contains("# HELP ops_total Total operations"));
    assert!(output.contains("# TYPE ops_total counter"));
}

#[test]
fn test_metrics_empty_registry_render() {
    let registry = MetricsRegistry::new();
    let output = registry.render();
    assert!(output.is_empty());
}

#[test]
fn test_metrics_histogram_inf_bucket() {
    let registry = MetricsRegistry::new();
    let hist = registry.register_histogram("latency", "Latency", vec![0.01, 0.1, 1.0]);
    hist.observe(0.005);
    hist.observe(0.05);
    hist.observe(0.5);
    hist.observe(5.0);

    let output = registry.render();
    // +Inf bucket 必须等于 count（Prometheus 规范）
    assert!(output.contains("latency_bucket{le=\"+Inf\"} 4"));
    assert!(output.contains("latency_count 4"));
}
