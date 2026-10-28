// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//
//! 基准套件 criterion 基准（SDET P2-1）
//!
//! 此前 benches/ 仅含 baseline.json，无任何 Rust 基准目标，属"名不副实"。
//! 本文件为套件核心逻辑（回归计算 / 结果校验 / 配置校验 / 基线序列化往返）
//! 提供真实 criterion 基准目标。

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use sz_rust_benchmark_suite::{BaselineFile, BenchmarkResult, BenchmarkSuite, BenchmarkSuiteConfig};

/// 回归百分比计算（套件核心热路径）
fn bench_calc_regression_pct(c: &mut Criterion) {
    c.bench_function("calc_regression_pct", |b| {
        b.iter(|| BenchmarkSuite::calc_regression_pct(black_box(100.0), black_box(112.5)).unwrap())
    });
}

/// 基准结果批量校验（防幻影结果门禁）
fn bench_validate_results(c: &mut Criterion) {
    c.bench_function("validate_results_50", |b| {
        b.iter(|| {
            let results: Vec<BenchmarkResult> = (0..50)
                .map(|i| BenchmarkResult::new(format!("bench_{i}"), 1_000.0 + i as f64, 1e6, 0))
                .collect();
            BenchmarkSuite::validate_results(black_box(&results)).is_ok()
        })
    });
}

/// 配置校验
fn bench_config_validate(c: &mut Criterion) {
    c.bench_function("config_validate", |b| {
        b.iter(|| BenchmarkSuiteConfig::default().validate().is_ok())
    });
}

/// BaselineFile JSON 序列化往返（基线管理 I/O 前置成本）
fn bench_baseline_json_round_trip(c: &mut Criterion) {
    c.bench_function("baseline_json_round_trip", |b| {
        b.iter(|| {
            let results: Vec<BenchmarkResult> = (0..100)
                .map(|i| BenchmarkResult::new(format!("bench_{i}"), 1_000.0 + i as f64, 1e6, 0))
                .collect();
            let file = BaselineFile {
                version: "v2.0.0".to_string(),
                results,
            };
            let json = serde_json::to_string(&file).unwrap();
            let parsed: BaselineFile = serde_json::from_str(&json).unwrap();
            parsed.results.len()
        })
    });
}

criterion_group!(
    benches,
    bench_calc_regression_pct,
    bench_validate_results,
    bench_config_validate,
    bench_baseline_json_round_trip
);
criterion_main!(benches);
