// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P4-3 HybridSearch 基准性能测试

use criterion::{criterion_group, criterion_main, Criterion};
use sz_rust_rag::hybrid_search::{HybridSearch, HybridSearchConfig, RawSearchResult};

fn make_results(n: usize) -> Vec<RawSearchResult> {
    (0..n)
        .map(|i| RawSearchResult {
            content: format!("content-{i}"),
            score: 0.9 - i as f64 * 0.01,
            doc_id: format!("doc-{i}"),
            fragment_position: i,
            knowledge_base_id: "kb-1".to_string(),
            tenant_id: "tenant-1".to_string(),
        })
        .collect()
}

fn bench_hybrid_search(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("hybrid_search");

    for n in [10, 50, 100, 500] {
        group.bench_function(format!("fusion_{n}_results"), |b| {
            b.to_async(&rt).iter(|| async {
                let config = HybridSearchConfig {
                    topk: 10,
                    ..HybridSearchConfig::default()
                };
                let search = HybridSearch::new(config).unwrap();
                let vector_results = make_results(n);
                let keyword_results = make_results(n);

                let _ = search
                    .search(
                        "test query",
                        |_| async { Ok(vector_results.clone()) },
                        |_| async { Ok(keyword_results.clone()) },
                    )
                    .await;
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_hybrid_search);
criterion_main!(benches);
