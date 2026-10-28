// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Repository 集成测试 — 需要 PostgreSQL
//! 运行: cargo test -p sz-rust-marketplace --test repository_integration -- --ignored

use sz_rust_marketplace::repository::{
    DeveloperRepository, InstallRecord, InstallRepository, Plugin, PluginRepository, PluginVersion,
    ReviewRecord, ReviewRepository, VersionRepository,
};

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
    let row: (i64,) = sqlx::query_as("INSERT INTO developers (username, email, public_key, is_reviewer) VALUES ($1, $2, 'pk_test', $3) RETURNING id")
        .bind(username)
        .bind(format!("{username}@test.com"))
        .bind(is_reviewer)
        .fetch_one(pool).await.expect("创建 developer 失败");
    row.0
}

fn make_plugin(name: &str, developer_id: i64) -> Plugin {
    Plugin {
        id: 0,
        name: name.to_string(),
        identifier: format!("com.test.{name}"),
        title: format!("Test {name}"),
        author: "tester".to_string(),
        homepage: None,
        license: "Apache-2.0".to_string(),
        description: Some("test plugin".to_string()),
        tags: vec!["test".to_string()],
        price: 0.0,
        developer_id,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

fn make_version(plugin_id: i64, version: &str) -> PluginVersion {
    PluginVersion {
        id: 0,
        plugin_id,
        version: version.to_string(),
        archive_key: format!("plugins/{version}.tar.gz"),
        sha256: "a".repeat(64),
        signature: "sig".to_string(),
        review_status: "pending".to_string(),
        changelog: Some("init".to_string()),
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
#[ignore]
async fn test_plugin_repository_crud() {
    let pool = setup_pool().await;
    let repo = PluginRepository::new(pool.clone());
    let dev_id = create_developer(&pool, "dev1", false).await;

    let plugin = make_plugin("myplugin", dev_id);
    let created = repo.create(&plugin).await.expect("create 失败");
    assert!(created.id > 0);
    assert_eq!(created.name, "myplugin");

    let found = repo.find_by_id(created.id).await.expect("find_by_id 失败");
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "myplugin");

    let by_name = repo
        .find_by_name("myplugin")
        .await
        .expect("find_by_name 失败");
    assert!(by_name.is_some());
    assert_eq!(by_name.unwrap().identifier, "com.test.myplugin");

    let not_found = repo.find_by_name("nonexistent").await.unwrap();
    assert!(not_found.is_none());

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_plugin_repository_search() {
    let pool = setup_pool().await;
    let repo = PluginRepository::new(pool.clone());
    let dev_id = create_developer(&pool, "dev2", false).await;

    let mut p1 = make_plugin("search_target", dev_id);
    p1.tags = vec!["search".to_string()];
    p1.description = Some("searchable plugin".to_string());
    repo.create(&p1).await.unwrap();

    let p2 = make_plugin("other_plugin", dev_id);
    repo.create(&p2).await.unwrap();

    let by_keyword = repo.search(Some("search"), None, 10, 0).await.unwrap();
    assert!(!by_keyword.is_empty());
    let names: Vec<&str> = by_keyword.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"search_target"));

    let by_tag = repo.search(None, Some("search"), 10, 0).await.unwrap();
    assert!(!by_tag.is_empty());

    let empty = repo.search(Some("zzz_nomatch"), None, 10, 0).await.unwrap();
    assert!(empty.is_empty());

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_version_repository_crud() {
    let pool = setup_pool().await;
    let plugin_repo = PluginRepository::new(pool.clone());
    let version_repo = VersionRepository::new(pool.clone());
    let dev_id = create_developer(&pool, "dev3", false).await;

    let plugin = plugin_repo
        .create(&make_plugin("ver_plugin", dev_id))
        .await
        .unwrap();
    let v1 = make_version(plugin.id, "1.0.0");
    let created = version_repo.create(&v1).await.expect("version create 失败");
    assert!(created.id > 0);
    assert_eq!(created.version, "1.0.0");

    let found = version_repo.find_by_id(created.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().version, "1.0.0");

    let latest = version_repo.find_latest_by_plugin(plugin.id).await.unwrap();
    assert!(latest.is_some());

    let v2 = make_version(plugin.id, "2.0.0");
    version_repo.create(&v2).await.unwrap();
    let latest2 = version_repo.find_latest_by_plugin(plugin.id).await.unwrap();
    assert!(latest2.is_some());

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_version_repository_find_pending_and_update() {
    let pool = setup_pool().await;
    let plugin_repo = PluginRepository::new(pool.clone());
    let version_repo = VersionRepository::new(pool.clone());
    let dev_id = create_developer(&pool, "dev4", false).await;

    let plugin = plugin_repo
        .create(&make_plugin("pending_plugin", dev_id))
        .await
        .unwrap();
    version_repo
        .create(&make_version(plugin.id, "1.0.0"))
        .await
        .unwrap();

    let pending = version_repo.find_pending(10, 0).await.unwrap();
    assert!(!pending.is_empty());

    let updated = version_repo
        .update_review_status(pending[0].id, "approved")
        .await
        .unwrap();
    assert_eq!(updated.review_status, "approved");

    let pending_after = version_repo.find_pending(10, 0).await.unwrap();
    let ids: Vec<i64> = pending_after.iter().map(|v| v.id).collect();
    assert!(!ids.contains(&pending[0].id));

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_review_repository_crud() {
    let pool = setup_pool().await;
    let plugin_repo = PluginRepository::new(pool.clone());
    let version_repo = VersionRepository::new(pool.clone());
    let review_repo = ReviewRepository::new(pool.clone());
    let dev_id = create_developer(&pool, "dev5", false).await;
    let reviewer_id = create_developer(&pool, "reviewer1", true).await;

    let plugin = plugin_repo
        .create(&make_plugin("review_plugin", dev_id))
        .await
        .unwrap();
    let version = version_repo
        .create(&make_version(plugin.id, "1.0.0"))
        .await
        .unwrap();

    let review = ReviewRecord {
        id: 0,
        version_id: version.id,
        reviewer_id,
        decision: "approve".to_string(),
        comment: Some("looks good".to_string()),
        created_at: chrono::Utc::now(),
    };
    let created = review_repo
        .create(&review)
        .await
        .expect("review create 失败");
    assert!(created.id > 0);
    assert_eq!(created.decision, "approve");

    let reviews = review_repo.find_by_version(version.id).await.unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].comment.as_deref(), Some("looks good"));

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_install_repository_create() {
    let pool = setup_pool().await;
    let plugin_repo = PluginRepository::new(pool.clone());
    let version_repo = VersionRepository::new(pool.clone());
    let install_repo = InstallRepository::new(pool.clone());
    let dev_id = create_developer(&pool, "dev6", false).await;
    let installer_id = create_developer(&pool, "installer1", false).await;

    let plugin = plugin_repo
        .create(&make_plugin("install_plugin", dev_id))
        .await
        .unwrap();
    let version = version_repo
        .create(&make_version(plugin.id, "1.0.0"))
        .await
        .unwrap();

    let install = InstallRecord {
        id: 0,
        plugin_id: plugin.id,
        version_id: version.id,
        installer_id,
        installed_at: chrono::Utc::now(),
    };
    let created = install_repo
        .create(&install)
        .await
        .expect("install create 失败");
    assert!(created.id > 0);
    assert_eq!(created.plugin_id, plugin.id);

    cleanup(&pool).await;
}

#[tokio::test]
#[ignore]
async fn test_developer_repository_find() {
    let pool = setup_pool().await;
    let repo = DeveloperRepository::new(pool.clone());
    let dev_id = create_developer(&pool, "dev_find", true).await;

    let by_id = repo.find_by_id(dev_id).await.unwrap();
    assert!(by_id.is_some());
    let dev = by_id.unwrap();
    assert_eq!(dev.username, "dev_find");
    assert!(dev.is_reviewer);

    let by_username = repo.find_by_username("dev_find").await.unwrap();
    assert!(by_username.is_some());
    assert_eq!(by_username.unwrap().id, dev_id);

    let not_found = repo.find_by_username("nonexistent_user").await.unwrap();
    assert!(not_found.is_none());

    cleanup(&pool).await;
}
