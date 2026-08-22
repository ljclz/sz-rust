use std::sync::Arc;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, Criterion};
use serde_json::{json, Value};
use sz_rust_capability::{AllowAll, CapResult, Capability, CapabilityRegistry, CapabilitySource};
use tokio::runtime::Runtime;

struct BenchCap {
    name: &'static str,
    tags: &'static [&'static str],
}

#[async_trait]
impl Capability for BenchCap {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        "基准能力"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "x": { "type": "number" } },
            "required": ["x"]
        })
    }
    fn tags(&self) -> &[&'static str] {
        self.tags
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        Ok(args)
    }
}

fn make_registry(n: usize) -> CapabilityRegistry {
    let registry = CapabilityRegistry::new();
    let tags: &'static [&'static str] = Box::leak(vec!["bench"].into_boxed_slice());
    for i in 0..n {
        let name: &'static str = Box::leak(format!("cap_{i}").into_boxed_str());
        let cap = Arc::new(BenchCap { name, tags }) as Arc<dyn Capability>;
        registry.register(cap);
    }
    registry
}

fn bench_register(c: &mut Criterion) {
    c.bench_function("cap_register", |b| {
        b.iter(|| {
            let registry = CapabilityRegistry::new();
            let cap = Arc::new(BenchCap {
                name: "reg_cap",
                tags: &["bench"],
            }) as Arc<dyn Capability>;
            registry.register(cap);
        });
    });
}

fn bench_find_by_tags(c: &mut Criterion) {
    let registry = make_registry(1000);
    c.bench_function("cap_find_by_tags_1000", |b| {
        b.iter(|| {
            let caps = registry.find_by_tags(&["bench"], None);
            criterion::black_box(caps);
        });
    });
}

fn bench_call_with_validation(c: &mut Criterion) {
    let registry = make_registry(1);
    registry.set_permission_checker(Arc::new(AllowAll));
    let rt = Runtime::new().expect("创建 tokio runtime");
    c.bench_function("cap_call_with_validation_and_permission", |b| {
        b.to_async(&rt).iter(|| async {
            let result = registry
                .call_with_tenant("cap_0", json!({"x": 42}), 1)
                .await;
            let _ = criterion::black_box(result);
        });
    });
}

fn bench_call_without_validation(c: &mut Criterion) {
    let registry = make_registry(1);
    let rt = Runtime::new().expect("创建 tokio runtime");
    c.bench_function("cap_call_without_checker", |b| {
        b.to_async(&rt).iter(|| async {
            let result = registry.call("cap_0", json!({"x": 42})).await;
            let _ = criterion::black_box(result);
        });
    });
}

criterion_group!(
    benches,
    bench_register,
    bench_find_by_tags,
    bench_call_with_validation,
    bench_call_without_validation,
);
criterion_main!(benches);
