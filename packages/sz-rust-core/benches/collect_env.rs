// P1-6: Bench 环境元数据采集工具
// 运行: cargo bench --package sz-rust-core --bench collect_env

use criterion::{criterion_group, criterion_main, Criterion};
use serde::Serialize;
use std::time::SystemTime;

#[derive(Serialize)]
pub struct BenchEnvironment {
    pub cpu_model: String,
    pub os_name: String,
    pub rustc_version: String,
    pub opt_level: String,
    pub lto: String,
    pub codegen_units: String,
    pub timestamp: String,
}

impl BenchEnvironment {
    pub fn collect() -> Self {
        Self {
            cpu_model: std::env::var("PROCESSOR_IDENTIFIER")
                .unwrap_or_else(|_| "unknown".to_string()),
            os_name: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            rustc_version: env!("CARGO_PKG_RUST_VERSION", "unknown").to_string(),
            opt_level: "dev".to_string(),
            lto: "off".to_string(),
            codegen_units: "16".to_string(),
            timestamp: format!("{:?}", SystemTime::now()),
        }
    }

    pub fn write_to_path(&self, path: &std::path::Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
    }
}

fn bench_collect_env(c: &mut Criterion) {
    c.bench_function("collect_environment", |b| {
        b.iter(|| {
            let env = BenchEnvironment::collect();
            std::hint::black_box(&env);
        })
    });
}

criterion_group!(benches, bench_collect_env);
criterion_main!(benches);
