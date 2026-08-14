use sz_rust_core::plugin::cross_query::{CrossQuery, CrossQueryError};

#[test]
fn test_same_tenant_query() {
    let cq = CrossQuery::new(100);
    assert!(cq.verify_tenant(100).is_ok());
}

#[test]
fn test_cross_tenant_denied() {
    let cq = CrossQuery::new(100);
    let result = cq.verify_tenant(200);
    assert!(result.is_err());
    match result {
        Err(CrossQueryError::PermissionDenied { tenant_id }) => {
            assert_eq!(tenant_id, 100);
        }
        other => panic!("期望 PermissionDenied, 实际: {other:?}"),
    }
}

#[test]
fn test_tenant_filter_injected() {
    let cq = CrossQuery::new(100);
    let args = serde_json::json!({"keyword": "test"});
    let result = cq.inject_tenant_filter(args);
    assert_eq!(result["tenant_id"], 100, "应自动注入 tenant_id");
    assert_eq!(result["keyword"], "test", "原参数应保留");
}

#[test]
fn test_tenant_filter_injected_null() {
    let cq = CrossQuery::new(200);
    let result = cq.inject_tenant_filter(serde_json::Value::Null);
    assert_eq!(result["tenant_id"], 200, "Null 参数也应注入 tenant_id");
}

#[test]
fn test_aggregate_batch_query() {
    let cq = CrossQuery::new(100);
    let queries = vec![
        ("plugin_a.search", serde_json::json!({"q": "hello"})),
        ("plugin_b.list", serde_json::json!({"page": 1})),
        ("plugin_c.stats", serde_json::json!({})),
    ];
    let result = cq.aggregate(&queries);
    assert_eq!(result["tenant_id"], 100, "聚合查询应包含 tenant_id");
    let queries_arr = result["queries"].as_array().expect("queries 应为数组");
    assert_eq!(queries_arr.len(), 3, "应有 3 个查询");
    for q in queries_arr {
        assert_eq!(q["args"]["tenant_id"], 100, "每个查询应注入 tenant_id");
    }
}

#[test]
fn test_tenant_id_getter() {
    let cq = CrossQuery::new(42);
    assert_eq!(cq.tenant_id(), 42);
}
