// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CLI DB 集成测试 — 需要 PostgreSQL
//! 运行: cargo test -p sz-rust-cli --test cli_db_integration -- --ignored --test-threads=1

use clap::Parser;
use sz_rust_cli::cmd::admin::AdminCommand;
use sz_rust_cli::cmd::migrate::{execute_migrate, execute_status_full, MigrateArgs};
use sz_rust_cli::cmd::seed::{execute_seed, SeedArgs};

const PG_URL: &str = "postgres://lewuli:JkbC2jsaWAYDe2Gz@127.0.0.1:5433/marketplace_test";

/// SDET P0-1/P1-4：连接串可由 MARKETPLACE_PG_URL 环境变量覆盖（CI 注入 5432 service），
/// 未设置时保持本机 SSH 隧道 5433 默认值，行为与修复前一致
fn pg_url() -> String {
    std::env::var("MARKETPLACE_PG_URL").unwrap_or_else(|_| PG_URL.to_string())
}

async fn cleanup() {
    let url = pg_url();
    let pool = sqlx::PgPool::connect(&url).await.expect("PG 连接失败");
    let _ = sqlx::query("DROP TABLE IF EXISTS install_records CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS review_records CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS plugin_versions CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS plugins CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS developers CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS __migrations CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS role_permissions CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS user_roles CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS permissions CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS menus CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS roles CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS users CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS configs CASCADE")
        .execute(&pool)
        .await;
    let _ = sqlx::query("DROP TABLE IF EXISTS operation_logs CASCADE")
        .execute(&pool)
        .await;
}

fn create_migration_dir(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("001_create_users_up.sql"),
        "CREATE TABLE users (id BIGSERIAL PRIMARY KEY, name VARCHAR(255));",
    )
    .unwrap();
    std::fs::write(
        dir.join("001_create_users_down.sql"),
        "DROP TABLE IF EXISTS users;",
    )
    .unwrap();
    std::fs::write(
        dir.join("002_add_index_up.sql"),
        "CREATE INDEX idx_users_name ON users (name);",
    )
    .unwrap();
    std::fs::write(
        dir.join("002_add_index_down.sql"),
        "DROP INDEX IF EXISTS idx_users_name;",
    )
    .unwrap();
}

#[tokio::test]
#[ignore]
async fn test_migrate_online_applies_migrations() {
    cleanup().await;
    let temp = tempfile::tempdir().unwrap();
    create_migration_dir(temp.path());

    let args = MigrateArgs {
        rollback: false,
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: false,
        url: Some(pg_url()),
    };
    let result = execute_migrate(&args).await;
    // AnyPool adapter 的 execute_with_params 已落地：迁移 SQL 与 __migrations
    // 记录都应写入成功（旧断言期望 err 已过期，run 41040631858 实测 migrate 成功）。
    assert!(result.is_ok(), "migrate 应成功: {:?}", result.err());

    // 迁移 SQL 本身已执行（users 表已创建）
    let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
    let exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_name = 'users')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(exists.0, "users 表应已创建（SQL 已执行）");

    let applied: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM __migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(applied.0, 2, "__migrations 应记录 2 条已应用版本");
}

#[tokio::test]
#[ignore]
async fn test_migrate_online_no_pending() {
    cleanup().await;
    let temp = tempfile::tempdir().unwrap();
    create_migration_dir(temp.path());

    let args = MigrateArgs {
        rollback: false,
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: false,
        url: Some(pg_url()),
    };
    // 第一次 migrate 应用全部迁移
    let first = execute_migrate(&args).await;
    assert!(first.is_ok(), "首次 migrate 应成功: {:?}", first.err());
    // 第二次调用没有待应用迁移，也应正常返回
    let result = execute_migrate(&args).await;
    assert!(
        result.is_ok(),
        "无待迁移时 migrate 应成功: {:?}",
        result.err()
    );
}

#[tokio::test]
#[ignore]
async fn test_migrate_online_rollback() {
    cleanup().await;
    let temp = tempfile::tempdir().unwrap();
    create_migration_dir(temp.path());

    let args = MigrateArgs {
        rollback: false,
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: false,
        url: Some(pg_url()),
    };
    // migrate 成功应用全部迁移
    let result = execute_migrate(&args).await;
    assert!(result.is_ok(), "migrate 应成功: {:?}", result.err());

    let rollback_args = MigrateArgs {
        rollback: true,
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: false,
        url: Some(pg_url()),
    };
    let result = execute_migrate(&rollback_args).await;
    assert!(result.is_ok(), "rollback 应成功: {:?}", result.err());
}

#[tokio::test]
#[ignore]
async fn test_migrate_status_online() {
    cleanup().await;
    let temp = tempfile::tempdir().unwrap();
    create_migration_dir(temp.path());

    let result = execute_status_full(
        temp.path().to_str().unwrap(),
        "postgres",
        false,
        Some(pg_url().as_str()),
    )
    .await;
    assert!(result.is_ok(), "status 在线模式应成功: {:?}", result.err());
}

#[tokio::test]
#[ignore]
async fn test_migrate_status_online_after_apply() {
    cleanup().await;
    let temp = tempfile::tempdir().unwrap();
    create_migration_dir(temp.path());

    let args = MigrateArgs {
        rollback: false,
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: false,
        url: Some(pg_url()),
    };
    // 先应用全部迁移，status 才能比对出已应用版本
    let applied = execute_migrate(&args).await;
    assert!(applied.is_ok(), "migrate 应成功: {:?}", applied.err());

    // status 在线模式应成功
    let result = execute_status_full(
        temp.path().to_str().unwrap(),
        "postgres",
        true,
        Some(pg_url().as_str()),
    )
    .await;
    assert!(result.is_ok(), "status 在线模式应成功: {:?}", result.err());
}

#[derive(Parser)]
struct AdminCli {
    #[command(subcommand)]
    cmd: AdminCommand,
}

#[tokio::test]
#[ignore]
async fn test_admin_migrate_online() {
    cleanup().await;
    let cli = AdminCli::parse_from([
        "sz-rust",
        "migrate",
        "--url",
        pg_url().as_str(),
        "--db-type",
        "postgres",
    ]);
    let result = sz_rust_cli::cmd::admin::execute(&cli.cmd).await;
    assert!(result.is_ok(), "admin migrate 应成功: {:?}", result.err());

    let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
    let exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_name = 'roles')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(exists.0, "roles 表应已创建");

    let exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_name = 'users')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(exists.0, "users 表应已创建");
}

#[tokio::test]
#[ignore]
async fn test_admin_migrate_show_sql_offline() {
    let cli = AdminCli::parse_from(["sz-rust", "migrate", "--show-sql"]);
    let result = sz_rust_cli::cmd::admin::execute(&cli.cmd).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[ignore]
async fn test_admin_init_offline() {
    let cli = AdminCli::parse_from([
        "sz-rust",
        "init",
        "--username",
        "testadmin",
        "--password",
        "testpass",
    ]);
    let result = sz_rust_cli::cmd::admin::execute(&cli.cmd).await;
    assert!(result.is_ok());
}
fn create_seed_dir(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("001_create_test_table.sql"),
        "CREATE TABLE IF NOT EXISTS seed_test (id INT, name VARCHAR(50));",
    )
    .unwrap();
    std::fs::write(
        dir.join("002_insert_data.sql"),
        "INSERT INTO seed_test VALUES (1, 'hello');",
    )
    .unwrap();
}

#[test]
#[ignore]
fn test_seed_online_executes_sql() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { cleanup().await });

    let temp = tempfile::tempdir().unwrap();
    create_seed_dir(temp.path());

    let args = SeedArgs {
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: false,
        url: Some(pg_url()),
        class: None,
    };
    let result = execute_seed(&args);
    assert!(result.is_ok(), "seed 在线模式应成功: {:?}", result.err());

    rt.block_on(async {
        let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM seed_test")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, 1, "应插入 1 条数据");
        let _ = sqlx::query("DROP TABLE IF EXISTS seed_test CASCADE")
            .execute(&pool)
            .await;
    });
}

#[test]
#[ignore]
fn test_seed_online_with_show_sql() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { cleanup().await });

    let temp = tempfile::tempdir().unwrap();
    create_seed_dir(temp.path());

    let args = SeedArgs {
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: true,
        url: Some(pg_url()),
        class: None,
    };
    let result = execute_seed(&args);
    assert!(result.is_ok(), "seed 在线模式 show_sql 应成功");

    rt.block_on(async {
        let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
        let _ = sqlx::query("DROP TABLE IF EXISTS seed_test CASCADE")
            .execute(&pool)
            .await;
    });
}

#[test]
#[ignore]
fn test_seed_online_with_class_filter() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { cleanup().await });

    let temp = tempfile::tempdir().unwrap();
    create_seed_dir(temp.path());

    let args = SeedArgs {
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: false,
        url: Some(pg_url()),
        class: Some("001_create_test_table".to_string()),
    };
    let result = execute_seed(&args);
    assert!(result.is_ok(), "seed 在线模式 class filter 应成功");

    rt.block_on(async {
        let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
        let exists: (bool,) = sqlx::query_as(
            "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_name = 'seed_test')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(exists.0, "seed_test 表应已创建");
        let _ = sqlx::query("DROP TABLE IF EXISTS seed_test CASCADE")
            .execute(&pool)
            .await;
    });
}
#[test]
#[ignore]
fn test_migrate_online_with_show_sql() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { cleanup().await });

    let temp = tempfile::tempdir().unwrap();
    create_migration_dir(temp.path());

    let args = MigrateArgs {
        rollback: false,
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: true,
        url: Some(pg_url()),
    };
    let result = rt.block_on(async { execute_migrate(&args).await });
    assert!(
        result.is_ok(),
        "show_sql 下 migrate 应成功: {:?}",
        result.err()
    );

    rt.block_on(async {
        let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
        let _ = sqlx::query("DROP TABLE IF EXISTS users CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS __migrations CASCADE")
            .execute(&pool)
            .await;
    });
}

#[test]
#[ignore]
fn test_migrate_status_online_with_show_sql() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { cleanup().await });

    let temp = tempfile::tempdir().unwrap();
    create_migration_dir(temp.path());

    let result = rt.block_on(async {
        execute_status_full(
            temp.path().to_str().unwrap(),
            "postgres",
            true,
            Some(pg_url().as_str()),
        )
        .await
    });
    assert!(result.is_ok(), "migrate:status 在线模式 show_sql 应成功");

    rt.block_on(async {
        let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
        let _ = sqlx::query("DROP TABLE IF EXISTS __migrations CASCADE")
            .execute(&pool)
            .await;
    });
}

#[test]
#[ignore]
fn test_migrate_online_rollback_with_show_sql() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { cleanup().await });

    let temp = tempfile::tempdir().unwrap();
    create_migration_dir(temp.path());

    let args = MigrateArgs {
        rollback: false,
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: false,
        url: Some(pg_url()),
    };
    let result = rt.block_on(async { execute_migrate(&args).await });
    assert!(result.is_ok(), "migrate 应成功: {:?}", result.err());

    let args = MigrateArgs {
        rollback: true,
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "postgres".to_string(),
        show_sql: true,
        url: Some(pg_url()),
    };
    let result = rt.block_on(async { execute_migrate(&args).await });
    assert!(result.is_ok(), "rollback 应成功: {:?}", result.err());

    rt.block_on(async {
        let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
        let _ = sqlx::query("DROP TABLE IF EXISTS users CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS __migrations CASCADE")
            .execute(&pool)
            .await;
    });
}

#[test]
#[ignore]
fn test_admin_migrate_online_with_show_sql() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { cleanup().await });

    let temp = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(temp.path()).unwrap();

    let cli = sz_rust_cli::Cli::parse_from([
        "sz-rust",
        "admin",
        "migrate",
        "--url",
        pg_url().as_str(),
        "--show-sql",
    ]);
    let result = rt.block_on(async { cli.execute().await });
    std::env::set_current_dir(&original).unwrap();
    assert!(
        result.is_ok(),
        "admin migrate --show-sql 在线应成功: {:?}",
        result.err()
    );

    rt.block_on(async {
        let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
        let _ = sqlx::query("DROP TABLE IF EXISTS roles CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS permissions CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS menus CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS users CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS configs CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS operation_logs CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS role_permissions CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS user_roles CASCADE")
            .execute(&pool)
            .await;
    });
}
#[test]
#[ignore]
fn test_admin_init_online() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { cleanup().await });

    let temp = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(temp.path()).unwrap();

    let cli = sz_rust_cli::Cli::parse_from([
        "sz-rust",
        "admin",
        "init",
        "--url",
        pg_url().as_str(),
        "--username",
        "admin",
        "--password",
        "admin123",
    ]);
    let result = rt.block_on(async { cli.execute().await });
    std::env::set_current_dir(&original).unwrap();
    assert!(result.is_err(), "execute_with_params 未实现应失败");

    rt.block_on(async {
        let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
        let _ = sqlx::query("DROP TABLE IF EXISTS roles CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS permissions CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS users CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS role_permissions CASCADE")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DROP TABLE IF EXISTS user_roles CASCADE")
            .execute(&pool)
            .await;
    });
}

#[test]
#[ignore]
fn test_seed_online_oracle_unsupported() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path()).unwrap();
    std::fs::write(
        temp.path().join("001_test.sql"),
        "INSERT INTO t VALUES (1);",
    )
    .unwrap();

    let args = SeedArgs {
        path: temp.path().to_str().unwrap().to_string(),
        db_type: "oracle".to_string(),
        show_sql: false,
        url: Some("oracle://user:pass@localhost:1521/db".to_string()),
        class: None,
    };
    let result = execute_seed(&args);
    assert!(result.is_err(), "Oracle seed 在线模式应不支持");
    assert!(result.unwrap_err().to_string().contains("not supported"));
}
