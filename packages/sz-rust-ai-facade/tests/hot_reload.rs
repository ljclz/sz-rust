// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T1.9 配置热更新端到端测试

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use sz_rust_ai_facade::llm::router::ModelRouter;

#[tokio::test]
async fn it_config_hot_reload_apply_update() {
    let router = ModelRouter::new(HashMap::new(), "gpt-4".to_string());

    let old_model = router.default_model();
    assert_eq!(old_model, "gpt-4");

    let start = Instant::now();
    router.apply_update(HashMap::new(), "gpt-4-turbo".to_string());
    let elapsed = start.elapsed();

    let new_model = router.default_model();
    assert_eq!(new_model, "gpt-4-turbo");
    assert!(elapsed.as_millis() < 100, "apply_update took {:?}", elapsed);
}

#[tokio::test]
async fn it_config_hot_reload_concurrent_access() {
    let router = Arc::new(ModelRouter::new(HashMap::new(), "gpt-4".to_string()));
    let count = Arc::new(AtomicU32::new(0));

    let mut handles = Vec::new();
    for _ in 0..50 {
        let r = router.clone();
        let c = count.clone();
        handles.push(tokio::spawn(async move {
            let model = r.default_model();
            if !model.is_empty() {
                c.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(count.load(Ordering::SeqCst), 50);
}
