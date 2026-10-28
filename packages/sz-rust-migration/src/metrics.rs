// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::sync::OnceLock;

use prometheus::{Counter, Gauge, Histogram, HistogramOpts, Opts, Registry};

static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(Registry::new)
}

pub struct MigrationMetrics {
    analysis_duration: Histogram,
    generation_retries: Counter,
    validation_failures: Counter,
    coverage_percent: Gauge,
}

impl MigrationMetrics {
    pub fn new() -> Self {
        let reg = registry();

        let analysis_duration = Histogram::with_opts(HistogramOpts::new(
            "migration_analysis_duration_seconds",
            "TP6 analysis phase duration in seconds",
        ))
        .expect("histogram creation");
        let generation_retries = Counter::with_opts(Opts::new(
            "migration_generation_retries_total",
            "Total code generation compile-fix retries",
        ))
        .expect("counter creation");
        let validation_failures = Counter::with_opts(Opts::new(
            "migration_validation_failures_total",
            "Total incremental validation failures",
        ))
        .expect("counter creation");
        let coverage_percent = Gauge::with_opts(Opts::new(
            "migration_coverage_percent",
            "Migration coverage percentage",
        ))
        .expect("gauge creation");

        let _ = reg.register(Box::new(analysis_duration.clone()));
        let _ = reg.register(Box::new(generation_retries.clone()));
        let _ = reg.register(Box::new(validation_failures.clone()));
        let _ = reg.register(Box::new(coverage_percent.clone()));

        Self {
            analysis_duration,
            generation_retries,
            validation_failures,
            coverage_percent,
        }
    }

    pub fn observe_analysis_duration(&self, secs: f64) {
        self.analysis_duration.observe(secs);
    }

    pub fn inc_generation_retries(&self) {
        self.generation_retries.inc();
    }

    pub fn inc_validation_failures(&self) {
        self.validation_failures.inc();
    }

    pub fn set_coverage_percent(&self, percent: f64) {
        self.coverage_percent.set(percent);
    }

    pub fn registry() -> &'static Registry {
        registry()
    }
}

impl Default for MigrationMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_creation_no_panic() {
        let _m1 = MigrationMetrics::new();
        let _m2 = MigrationMetrics::new();
    }

    #[test]
    fn observe_and_inc() {
        let m = MigrationMetrics::new();
        m.observe_analysis_duration(1.5);
        m.inc_generation_retries();
        m.inc_generation_retries();
        m.inc_validation_failures();
        m.set_coverage_percent(85.0);
        // 断言指标值真实写入（断言价值门禁：拒绝无断言空洞测试）
        assert_eq!(m.generation_retries.get(), 2.0);
        assert_eq!(m.validation_failures.get(), 1.0);
        assert_eq!(m.coverage_percent.get(), 85.0);
    }
}
