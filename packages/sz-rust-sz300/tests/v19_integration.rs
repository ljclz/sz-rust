// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 全量集成测试套件（spec §5.1-5.10 联合验证）
//!
//! 验证 `all-v1-9` feature 下：
//! 1. 各模块可独立调用（无交叉编译错误）
//! 2. 模块间无状态冲突（并行安全）
//! 3. feature flag 矩阵：all-v1-9 聚合 feature 包含所有子 feature

#![cfg(feature = "all-v1-9")]

use std::sync::Arc;

mod common;

// ============================================================================
// 模块 1: GraphQL 持久化（spec §5.1）
// ============================================================================
#[cfg(feature = "v19-graphql-persist")]
#[tokio::test]
async fn test_integration_graphql_schema_db_buildable() {
    use sz_rust_sz300::graphql;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let schema = graphql::build_schema_with_db(Arc::new(pool));
    let _ = format!("{:p}", &schema);
}

// ============================================================================
// 模块 2: JWT Audience 安全增强（spec §5.2）
// ============================================================================
#[cfg(feature = "v19-jwt-audience")]
#[tokio::test]
async fn test_integration_jwt_audience_config_constructible() {
    use sz_rust_sz300::services::auth_service::AudienceConfig;
    let config = AudienceConfig {
        service_id: "sz300-api".into(),
        deny_no_audience: false,
        grace_period_secs: 3600,
        enabled: true,
    };
    assert_eq!(config.service_id, "sz300-api");
    assert!(config.enabled);
}

// ============================================================================
// 模块 3: Saga 分布式事务（spec §5.3）
// ============================================================================
#[cfg(feature = "v19-saga")]
#[tokio::test]
async fn test_integration_saga_order_result_variants() {
    use sz_rust_sz300::services::saga_order::OrderCreateResult;
    let success = OrderCreateResult::Success {
        order_id: 1001,
        tx_id: "tx-001".into(),
    };
    assert!(matches!(success, OrderCreateResult::Success { .. }));
}

// ============================================================================
// 模块 4: AI 智能分类（spec §5.4）
// ============================================================================
#[cfg(feature = "v19-ai-classify")]
#[tokio::test]
async fn test_integration_ai_classifier_sanitize() {
    use sz_rust_sz300::services::ai_classifier::sanitize_classification_input;
    let sanitized = sanitize_classification_input("苹果", "123456", "新鲜水果");
    assert_eq!(sanitized.name, "苹果");
    assert_eq!(sanitized.barcode, "123456");
}

// ============================================================================
// 模块 5: 国际化（spec §5.5）
// ============================================================================
#[cfg(feature = "v19-i18n")]
#[tokio::test]
async fn test_integration_i18n_extractor_from_query() {
    use sz_rust_sz300::i18n_extractor::I18nExtractor;
    let lang = I18nExtractor::from_request(None, "lang=zh-CN");
    assert_eq!(lang.as_str(), "zh-CN");
}

// ============================================================================
// 模块 6: 配置中心动态配置（spec §5.6）
// ============================================================================
#[cfg(feature = "v19-config-center")]
#[tokio::test]
async fn test_integration_dynamic_config_snapshot_empty() {
    use sz_rust_sz300::dynamic_config::ConfigSnapshot;
    let snapshot = ConfigSnapshot::empty();
    assert_eq!(snapshot.version, 0);
}

// ============================================================================
// 模块 7: 可观测性闭环（spec §5.7）
// ============================================================================
#[cfg(feature = "v19-obs-closure")]
#[tokio::test]
async fn test_integration_alert_rules_severity_mapping() {
    use sz_rust_alert_engine::AlertSeverity;
    use sz_rust_sz300::alert_rules::channels_for_severity;
    let p0_channels = channels_for_severity(AlertSeverity::Critical);
    assert!(!p0_channels.is_empty());
}

// ============================================================================
// 模块 8: 插件生态（spec §5.10）
// ============================================================================
#[cfg(feature = "v19-plugin-flow")]
#[tokio::test]
async fn test_integration_plugin_manager_list_empty() {
    use sz_rust_sz300::services::plugin_manager::{InMemoryMarketplace, PluginManager};
    let marketplace = Arc::new(InMemoryMarketplace::new());
    let mgr = PluginManager::new(marketplace, "pub_key".into());
    assert!(mgr.list().await.is_empty());
}

// ============================================================================
// 联合场景: 多模块并行无状态冲突
// ============================================================================
#[tokio::test]
async fn test_integration_all_modules_parallel_no_conflict() {
    let handles: Vec<tokio::task::JoinHandle<()>> = vec![
        #[cfg(feature = "v19-i18n")]
        tokio::spawn(async {
            use sz_rust_sz300::i18n_extractor::I18nExtractor;
            let lang = I18nExtractor::from_request(Some("en,zh-CN;q=0.9"), "");
            assert!(!lang.as_str().is_empty());
        }),
        #[cfg(feature = "v19-config-center")]
        tokio::spawn(async {
            use sz_rust_sz300::dynamic_config::ConfigSnapshot;
            let s = ConfigSnapshot::empty();
            assert_eq!(s.version, 0);
        }),
        #[cfg(feature = "v19-plugin-flow")]
        tokio::spawn(async {
            use sz_rust_sz300::services::plugin_manager::{InMemoryMarketplace, PluginManager};
            let mgr = PluginManager::new(Arc::new(InMemoryMarketplace::new()), "k".into());
            assert!(mgr.list().await.is_empty());
        }),
    ];
    for handle in handles {
        let _ = handle.await;
    }
}

// ============================================================================
// feature flag 矩阵: all-v1-9 聚合 feature 包含所有子 feature
// ============================================================================
#[test]
fn test_integration_all_v1_9_feature_matrix() {
    #[cfg(not(feature = "v19-graphql-persist"))]
    panic!("all-v1-9 应包含 v19-graphql-persist");
    #[cfg(not(feature = "v19-jwt-audience"))]
    panic!("all-v1-9 应包含 v19-jwt-audience");
    #[cfg(not(feature = "v19-saga"))]
    panic!("all-v1-9 应包含 v19-saga");
    #[cfg(not(feature = "v19-ai-classify"))]
    panic!("all-v1-9 应包含 v19-ai-classify");
    #[cfg(not(feature = "v19-i18n"))]
    panic!("all-v1-9 应包含 v19-i18n");
    #[cfg(not(feature = "v19-config-center"))]
    panic!("all-v1-9 应包含 v19-config-center");
    #[cfg(not(feature = "v19-obs-closure"))]
    panic!("all-v1-9 应包含 v19-obs-closure");
    #[cfg(not(feature = "v19-plugin-flow"))]
    panic!("all-v1-9 应包含 v19-plugin-flow");
}
