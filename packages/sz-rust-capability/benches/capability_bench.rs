use std::sync::Arc;

use async_trait::async_trait;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::{json, Value};
use sz_rust_capability::{CapResult, Capability, CapabilityRegistry, CapabilitySource};

struct BenchCapability {
    cap_name: &'static str,
    cap_tags: &'static [&'static str],
}

#[async_trait]
impl Capability for BenchCapability {
    fn name(&self) -> &'static str {
        self.cap_name
    }
    fn description(&self) -> &'static str {
        "基准测试能力"
    }
    fn schema(&self) -> Value {
        json!({})
    }
    fn tags(&self) -> &[&'static str] {
        self.cap_tags
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        Ok(args)
    }
}

fn bench_register(c: &mut Criterion) {
    c.bench_function("register_single", |b| {
        b.iter(|| {
            let registry = CapabilityRegistry::new();
            let cap = Arc::new(BenchCapability {
                cap_name: "bench_cap",
                cap_tags: &["bench"],
            }) as Arc<dyn Capability>;
            black_box(registry.register(cap));
        })
    });

    c.bench_function("register_100", |b| {
        b.iter(|| {
            let registry = CapabilityRegistry::new();
            for i in 0..100u32 {
                let name: &'static str = Box::leak(format!("cap_{i}").into_boxed_str());
                let cap = Arc::new(BenchCapability {
                    cap_name: name,
                    cap_tags: &["bench"],
                }) as Arc<dyn Capability>;
                registry.register(cap);
            }
            black_box(&registry);
        })
    });
}

fn bench_get(c: &mut Criterion) {
    let registry = CapabilityRegistry::new();
    for i in 0..1000u32 {
        let name: &'static str = Box::leak(format!("cap_{i}").into_boxed_str());
        let cap = Arc::new(BenchCapability {
            cap_name: name,
            cap_tags: &["bench", "test"],
        }) as Arc<dyn Capability>;
        registry.register(cap);
    }

    c.bench_function("get_existing", |b| {
        b.iter(|| {
            black_box(registry.get("cap_500"));
        })
    });

    c.bench_function("get_nonexistent", |b| {
        b.iter(|| {
            black_box(registry.get("nonexistent"));
        })
    });
}

fn bench_find_by_tags(c: &mut Criterion) {
    let registry = CapabilityRegistry::new();
    for i in 0..1000u32 {
        let name: &'static str = Box::leak(format!("cap_{i}").into_boxed_str());
        let tags: &'static [&'static str] = if i % 2 == 0 {
            &["bench", "even"]
        } else {
            &["bench", "odd"]
        };
        let cap = Arc::new(BenchCapability {
            cap_name: name,
            cap_tags: tags,
        }) as Arc<dyn Capability>;
        registry.register(cap);
    }

    c.bench_function("find_by_tags_single", |b| {
        b.iter(|| {
            black_box(registry.find_by_tags(&["bench"], None));
        })
    });

    c.bench_function("find_by_tags_multi_and", |b| {
        b.iter(|| {
            black_box(registry.find_by_tags(&["bench", "even"], None));
        })
    });

    c.bench_function("find_by_tags_with_source", |b| {
        b.iter(|| {
            black_box(registry.find_by_tags(&["bench"], Some(CapabilitySource::Plugin)));
        })
    });
}

fn bench_search(c: &mut Criterion) {
    let registry = CapabilityRegistry::new();
    for i in 0..1000u32 {
        let name: &'static str = Box::leak(format!("search_cap_{i}").into_boxed_str());
        let cap = Arc::new(BenchCapability {
            cap_name: name,
            cap_tags: &["bench"],
        }) as Arc<dyn Capability>;
        registry.register(cap);
    }

    c.bench_function("search_by_name", |b| {
        b.iter(|| {
            black_box(registry.search("search_cap_500"));
        })
    });
}

fn bench_list(c: &mut Criterion) {
    let registry = CapabilityRegistry::new();
    for i in 0..1000u32 {
        let name: &'static str = Box::leak(format!("list_cap_{i}").into_boxed_str());
        let cap = Arc::new(BenchCapability {
            cap_name: name,
            cap_tags: &["bench"],
        }) as Arc<dyn Capability>;
        registry.register(cap);
    }

    c.bench_function("list_all_1000", |b| {
        b.iter(|| {
            black_box(registry.list_all());
        })
    });

    c.bench_function("list_by_source", |b| {
        b.iter(|| {
            black_box(registry.list_by_source(CapabilitySource::Plugin));
        })
    });

    c.bench_function("metrics", |b| {
        b.iter(|| {
            black_box(registry.metrics());
        })
    });
}

criterion_group!(
    benches,
    bench_register,
    bench_get,
    bench_find_by_tags,
    bench_search,
    bench_list,
);
criterion_main!(benches);
