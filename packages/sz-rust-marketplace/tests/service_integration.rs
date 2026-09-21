// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Service 集成测试 — 需要 PostgreSQL
//! 运行: cargo test -p sz-rust-marketplace --test service_integration -- --ignored

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use sz_rust_addons_loader::manifest::AddonManifest;
use sz_rust_marketplace::error::{MarketplaceError, MarketplaceResult};
use sz_rust_marketplace::manifest::MarketplaceManifest;
use sz_rust_marketplace::repository::{
    DeveloperRepository, PluginRepository, ReviewRepository, VersionRepository,
};
use sz_rust_marketplace::service::{
    MarketplaceService, PublishRequest, ReviewDecision, ReviewRequest, SearchRequest,
};
use sz_rust_marketplace::storage::ObjectStore;

struct MockObjectStore;

#[async_trait]
impl ObjectStore for MockObjectStore {
    async fn upload(&self, _key: &str, data: Bytes) -> MarketplaceResult<String> {
        Ok(sz_rust_marketplace::signature::SignatureService::sha256_checksum(&data))
    }
    async fn download(&self, _key: &str, _range: Option<(u64, u64)>) -> MarketplaceResult<Bytes> {
        Ok(Bytes::new())
    }
    async fn delete(&self, _key: &str) -> MarketplaceResult<()> {
        Ok(())
    }
}

async fn setup_pool() -> sqlx::PgPool {
    let url = std::env::var("MARKETPLACE_PG_URL").unwrap_or_else(|_| {
        "postgres://lewuli:JkbC2jsaWAYDe2Gz@127.0.0.1:5433/marketplace_test".to_string()
    });
    let pool = sqlx::PgPool::connect(&url).await.expect("连接 PG 失败");
    cleanup(&pool).await;
    migrate(&pool).await;
    pool
}

async fn cleanup(pool: &sqlx::PgPool) {
    let _ = sqlx::query("DROP TABLE IF EXISTS install_records CASCADE")
        .execute(pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS review_records CASCADE")
        .execute(pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS plugin_versions CASCADE")
        .execute(pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS plugins CASCADE")
        .execute(pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS developers CASCADE")
        .execute(pool)
        .await;
}

async fn migrate(pool: &sqlx::PgPool) {
    let migrations = [
        include_str!("../migrations/001_create_developers.sql"),
        include_str!("../migrations/002_create_plugins.sql"),
        include_str!("../migrations/003_create_plugin_versions.sql"),
        include_str!("../migrations/004_create_review_records.sql"),
        include_str!("../migrations/005_create_install_records.sql"),
    ];
    for (i, sql) in migrations.iter().enumerate() {
        for stmt in sql.split(';').filter(|s| !s.trim().is_empty()) {
            sqlx::query(stmt)
                .execute(pool)
                .await
                .unwrap_or_else(|e| panic!("迁移 {} 失败: {e}", i + 1));
        }
    }
}

async fn create_developer(pool: &sqlx::PgPool, username: &str, is_reviewer: bool) -> i64 {
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO developers (username, email, public_key, is_reviewer) VALUES ($1, $2, 'pk_test', $3) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@test.com"))
    .bind(is_reviewer)
    .fetch_one(pool).await.expect("创建 developer 失败");
    row.0
}

fn make_manifest(name: &str, version: &str) -> MarketplaceManifest {
    MarketplaceManifest {
        base: AddonManifest {
            name: name.to_string(),
            title: format!("Test {name}"),
            identifier: format!("com.test.{name}"),
            icon: String::new(),
            author: "tester".to_string(),
            version: version.to_string(),
            admin: String::new(),
            status: 1,
            addon_path: std::path::PathBuf::new(),
        },
        description: Some("test plugin".to_string()),
        tags: vec!["test".to_string()],
        capabilities: vec![],
        dependencies: vec![],
        license: "Apache-2.0".to_string(),
        homepage: None,
        price: 0.0,
        signature: String::new(),
        status: sz_rust_marketplace::manifest::ReviewStatus::Pending,
    }
}

fn make_service(pool: &sqlx::PgPool) -> MarketplaceService {
    MarketplaceService::new(
        PluginRepository::new(pool.clone()),
        VersionRepository::new(pool.clone()),
        ReviewRepository::new(pool.clone()),
        DeveloperRepository::new(pool.clone()),
        Arc::new(MockObjectStore),
    )
}

#[tokio::test]
#[ignore]
async fn test_service_publish_new_plugin() {
    let pool = setup_pool().await;
    let service = make_service(&pool);
    let dev_id = create_developer(&pool, "pub_dev1", false).await;

    let req = PublishRequest {
        manifest: make_manifest("pub_plugin1", "1.0.0"),
        archive: Bytes::from(b"test archive".to_vec()),
        developer_id: dev_id,
    };
    let version_id = service.publish(req).await.expect("publish 失败");
    assert!(version_id > 0);

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_service_publish_version_conflict() {
    let pool = setup_pool().await;
    let service = make_service(&pool);
    let dev_id = create_developer(&pool, "pub_dev2", false).await;

    let req1 = PublishRequest {
        manifest: make_manifest("pub_plugin2", "1.0.0"),
        archive: Bytes::from(b"v1".to_vec()),
        developer_id: dev_id,
    };
    service.publish(req1).await.unwrap();

    let req2 = PublishRequest {
        manifest: make_manifest("pub_plugin2", "1.0.0"),
        archive: Bytes::from(b"v1".to_vec()),
        developer_id: dev_id,
    };
    let result = service.publish(req2).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        MarketplaceError::VersionConflict(_) => {}
        other => panic!("期望 VersionConflict，得到 {other:?}"),
    }

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_service_publish_version_increasing() {
    let pool = setup_pool().await;
    let service = make_service(&pool);
    let dev_id = create_developer(&pool, "pub_dev3", false).await;

    let req1 = PublishRequest {
        manifest: make_manifest("pub_plugin3", "1.0.0"),
        archive: Bytes::from(b"v1".to_vec()),
        developer_id: dev_id,
    };
    service.publish(req1).await.unwrap();

    let req2 = PublishRequest {
        manifest: make_manifest("pub_plugin3", "2.0.0"),
        archive: Bytes::from(b"v2".to_vec()),
        developer_id: dev_id,
    };
    let vid2 = service.publish(req2).await.expect("publish 2.0.0 失败");
    assert!(vid2 > 0);

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_service_search() {
    let pool = setup_pool().await;
    let service = make_service(&pool);
    let dev_id = create_developer(&pool, "pub_dev4", false).await;

    service
        .publish(PublishRequest {
            manifest: make_manifest("searchable_plugin", "1.0.0"),
            archive: Bytes::from(b"data".to_vec()),
            developer_id: dev_id,
        })
        .await
        .unwrap();

    let results = service
        .search(SearchRequest {
            keyword: Some("searchable".to_string()),
            tag: None,
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap();
    assert!(!results.is_empty());
    let names: Vec<&str> = results.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"searchable_plugin"));

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_service_review_approve() {
    let pool = setup_pool().await;
    let service = make_service(&pool);
    let dev_id = create_developer(&pool, "pub_dev5", false).await;
    let reviewer_id = create_developer(&pool, "reviewer5", true).await;

    let version_id = service
        .publish(PublishRequest {
            manifest: make_manifest("review_plugin", "1.0.0"),
            archive: Bytes::from(b"data".to_vec()),
            developer_id: dev_id,
        })
        .await
        .unwrap();

    let result = service
        .review(ReviewRequest {
            version_id,
            reviewer_id,
            developer_id: dev_id,
            decision: ReviewDecision::Approve,
            comment: Some("approved".to_string()),
        })
        .await;
    assert!(result.is_ok());

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_service_review_self_review_rejected() {
    let pool = setup_pool().await;
    let service = make_service(&pool);
    let dev_id = create_developer(&pool, "pub_dev6", true).await;

    let version_id = service
        .publish(PublishRequest {
            manifest: make_manifest("self_review_plugin", "1.0.0"),
            archive: Bytes::from(b"data".to_vec()),
            developer_id: dev_id,
        })
        .await
        .unwrap();

    let result = service
        .review(ReviewRequest {
            version_id,
            reviewer_id: dev_id,
            developer_id: dev_id,
            decision: ReviewDecision::Approve,
            comment: None,
        })
        .await;
    assert!(result.is_err());
    match result.unwrap_err() {
        MarketplaceError::SelfReview { .. } => {}
        other => panic!("期望 SelfReview，得到 {other:?}"),
    }

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_service_review_non_reviewer_rejected() {
    let pool = setup_pool().await;
    let service = make_service(&pool);
    let dev_id = create_developer(&pool, "pub_dev7", false).await;
    let non_reviewer_id = create_developer(&pool, "non_reviewer7", false).await;

    let version_id = service
        .publish(PublishRequest {
            manifest: make_manifest("nr_plugin", "1.0.0"),
            archive: Bytes::from(b"data".to_vec()),
            developer_id: dev_id,
        })
        .await
        .unwrap();

    let result = service
        .review(ReviewRequest {
            version_id,
            reviewer_id: non_reviewer_id,
            developer_id: dev_id,
            decision: ReviewDecision::Approve,
            comment: None,
        })
        .await;
    assert!(result.is_err());
    match result.unwrap_err() {
        MarketplaceError::NotReviewer(_) => {}
        other => panic!("期望 NotReviewer，得到 {other:?}"),
    }

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_service_review_non_pending_version() {
    let pool = setup_pool().await;
    let service = make_service(&pool);
    let dev_id = create_developer(&pool, "pub_dev8", false).await;
    let reviewer_id = create_developer(&pool, "reviewer8", true).await;

    let version_id = service
        .publish(PublishRequest {
            manifest: make_manifest("np_plugin", "1.0.0"),
            archive: Bytes::from(b"data".to_vec()),
            developer_id: dev_id,
        })
        .await
        .unwrap();

    service
        .review(ReviewRequest {
            version_id,
            reviewer_id,
            developer_id: dev_id,
            decision: ReviewDecision::Approve,
            comment: None,
        })
        .await
        .unwrap();

    let result = service
        .review(ReviewRequest {
            version_id,
            reviewer_id,
            developer_id: dev_id,
            decision: ReviewDecision::Reject,
            comment: None,
        })
        .await;
    assert!(result.is_err());
    match result.unwrap_err() {
        MarketplaceError::VersionNotPending { .. } => {}
        other => panic!("期望 VersionNotPending，得到 {other:?}"),
    }

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_service_download_archive() {
    let pool = setup_pool().await;
    let service = make_service(&pool);
    let result = service.download_archive("nonexistent_key").await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());

    cleanup(&pool).await;
}
