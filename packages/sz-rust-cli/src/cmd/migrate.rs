// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! `migrate` / `migrate:status` 命令 — 整合 `sz-orm-core::migration`
//!
//! ## PHP 对齐
//!
//! PHP `migrate:status` 输出表格：
//! ```text
//! +---------+------------------+---------------------+
//! | Version | Migration Name   | Run Time            |
//! +---------+------------------+---------------------+
//! | 001     | create_users     | 2024-01-01 00:00:00 |
//! | 002     | add_index        | Pending             |
//! +---------+------------------+---------------------+
//! ```
//!
//! ## 整合说明
//!
//! 本模块使用 `sz_orm_core::migration::FileMigrationResolver`（sz-orm-core，经 sz-rust-orm-facade 透传） 解析迁移目录，
//! 对齐 sz-orm 的迁移文件命名约定（`<version>_<name>_up.sql` / `<version>_<name>_down.sql`）。
//!
//! ### 离线模式（默认）
//!
//! 不连接数据库，仅解析并列出迁移文件。`migrate` 命令输出"将执行的 SQL"，
//! `migrate:status` 输出迁移列表（状态统一显示 `Pending*`，因离线无法确定执行历史）。
//!
//! ### 在线模式（提供 `--url` 时启用）
//!
//! 通过 `Migrator::migrate()` 执行真实迁移，需要注入 `MigrationContext::connection`。
//! CLI 通过 `sz-orm-sqlx` 建立 PostgreSQL/MySQL/SQLite 连接池，包装为
//! `Box<dyn Connection>` 注入 `MigrationContext`。
//! `migrate:status` 在线模式下从 `__migrations` 表查询已应用版本，显示真实状态。

use std::path::{Path, PathBuf};

use clap::Args;
use sz_rust_core::orm::migration::{FileMigrationResolver, Migration, MigrationResolver};
use sz_rust_core::orm::{Connection, DbType};

use crate::error::CliError;

/// `migrate` 命令参数
///
/// 对齐 PHP `php think migrate` / `php think migrate:rollback`。
#[derive(Args, Debug)]
pub struct MigrateArgs {
    /// 回滚最后一批迁移（对齐 PHP `migrate:rollback`）
    #[arg(long)]
    pub rollback: bool,

    /// 迁移目录（默认 `migrations`）
    #[arg(short = 'p', long, default_value = "migrations")]
    pub path: String,

    /// 数据库类型（默认 `postgres`，对齐 sz-orm `DbType`）
    ///
    /// 影响迁移解析的方言处理。支持值：
    /// `mysql` / `postgres` / `sqlite` / `oracle` / `mssql` /
    /// `oceanbase` / `dameng` / `kingbase` 等（详见 `DbType::from_str`）。
    #[arg(long, default_value = "postgres")]
    pub db_type: String,

    /// 打印每个迁移的 SQL 内容（dry-run 模式，便于审查）
    #[arg(long)]
    pub show_sql: bool,

    /// 数据库连接 URL（启用在线模式）
    ///
    /// 提供时连接数据库执行真实迁移；省略时为离线模式（仅列出待执行的 SQL）。
    /// 格式示例：
    /// - PostgreSQL: `postgres://user:pass@host:5432/dbname`
    /// - MySQL: `mysql://user:pass@host:3306/dbname`
    /// - SQLite: `sqlite://path/to/database.db`
    #[arg(long)]
    pub url: Option<String>,
}

/// 执行 migrate 命令
///
/// - 无 `--rollback`：执行所有待迁移（对齐 `php think migrate`）
/// - 有 `--rollback`：回滚最后一批迁移（对齐 `php think migrate:rollback`）
///
/// # 模式
///
/// - **离线模式**（默认，未提供 `--url`）：仅解析迁移目录并打印待执行内容
/// - **在线模式**（提供 `--url`）：连接数据库执行真实迁移
pub async fn execute_migrate(args: &MigrateArgs) -> Result<(), CliError> {
    let path = PathBuf::from(&args.path);

    if !path.exists() {
        return Err(CliError::Migration(format!(
            "Migration directory not found: {}",
            path.display()
        )));
    }

    let db_type = DbType::from_str(&args.db_type)
        .ok_or_else(|| CliError::Migration(format!("Unknown database type: {}", args.db_type)))?;

    let migrations = resolve_migrations(&path, db_type)?;

    if migrations.is_empty() {
        println!("No migrations found in: {}", path.display());
        return Ok(());
    }

    match &args.url {
        None => execute_migrate_offline(args, &migrations),
        Some(url) => execute_migrate_online(args, &migrations, url, db_type).await,
    }
}

/// 离线模式执行 migrate（仅打印，不连库）
fn execute_migrate_offline(args: &MigrateArgs, migrations: &[Migration]) -> Result<(), CliError> {
    if args.rollback {
        println!("Rolling back last batch in: {}", args.path);
        if let Some(last) = migrations.last() {
            println!("  Would rollback: {} ({})", last.version, last.name);
            if args.show_sql {
                println!("{}", print_sql_block("SQL DOWN", &last.sql_down));
            }
        }
        println!("Note: Actual rollback requires database connection (offline mode).");
    } else {
        println!("Running migrations in: {}", args.path);
        for m in migrations {
            println!("  Would apply: {} ({})", m.version, m.name);
            if args.show_sql {
                println!("{}", print_sql_block("SQL UP", &m.sql_up));
            }
        }
        println!(
            "Total: {} migration(s). Note: Actual execution requires database connection (offline mode).",
            migrations.len()
        );
    }
    Ok(())
}

/// 在线模式执行 migrate（连接数据库真实执行）
async fn execute_migrate_online(
    args: &MigrateArgs,
    migrations: &[Migration],
    url: &str,
    db_type: DbType,
) -> Result<(), CliError> {
    let mut conn = create_connection(url, db_type).await?;

    if args.rollback {
        // 回滚最后一个迁移
        let last = migrations
            .last()
            .ok_or_else(|| CliError::Migration("No migrations to rollback".to_string()))?;
        println!("Rolling back: {} ({})", last.version, last.name);
        if args.show_sql {
            println!("{}", print_sql_block("SQL DOWN", &last.sql_down));
        }
        if !last.sql_down.is_empty() {
            let sql = prepare_sql_for_db(&last.sql_down, db_type);
            conn.execute(&sql)
                .await
                .map_err(|e| CliError::Migration(format!("Rollback failed: {}", e)))?;
        }
        // 从 __migrations 表删除记录
        delete_migration_record(&mut conn, &last.version, db_type).await?;
        println!("Rollback completed: {} ({})", last.version, last.name);
    } else {
        // 确保 __migrations 表存在
        ensure_migrations_table(&mut conn, db_type).await?;

        // 查询已应用版本
        let applied = fetch_applied_versions(&mut conn, db_type).await?;

        // 过滤出待执行的迁移
        let pending: Vec<&Migration> = migrations
            .iter()
            .filter(|m| !applied.contains(&m.version))
            .collect();

        if pending.is_empty() {
            println!("No pending migrations. Database is up to date.");
            return Ok(());
        }

        println!("Running {} pending migration(s):", pending.len());

        let mut applied_count = 0;
        for m in &pending {
            if args.show_sql {
                println!("{}", print_sql_block("SQL UP", &m.sql_up));
            }
            let sql = prepare_sql_for_db(&m.sql_up, db_type);

            conn.execute(&sql).await.map_err(|e| {
                CliError::Migration(format!("Migration {} failed: {}", m.version, e))
            })?;
            insert_migration_record(&mut conn, &m.version, &m.name, db_type).await?;
            println!("  Applied: {}", m.version);
            applied_count += 1;
        }
        println!("Migration completed: {} applied.", applied_count);
    }

    Ok(())
}

/// 执行 migrate:status 命令（兼容入口，使用默认 `postgres` 方言）
///
/// 对齐 PHP `php think migrate:status`，输出表格格式的迁移状态。
///
/// 等价于 [`execute_status_with`] 传入 `db_type="postgres"`、`show_sql=false`、`url=None`。
pub async fn execute_status(path: &str) -> Result<(), CliError> {
    execute_status_full(path, "postgres", false, None).await
}

/// 执行 migrate:status 命令（完整参数）
///
/// # 参数
///
/// - `path`：迁移目录
/// - `db_type_str`：数据库类型字符串（由 `DbType::from_str` 解析）
/// - `show_sql`：是否打印每个迁移的 SQL 内容
pub async fn execute_status_with(
    path: &str,
    db_type_str: &str,
    show_sql: bool,
) -> Result<(), CliError> {
    execute_status_full(path, db_type_str, show_sql, None).await
}

/// 执行 migrate:status 命令（完整参数，含在线模式）
pub async fn execute_status_full(
    path: &str,
    db_type_str: &str,
    show_sql: bool,
    url: Option<&str>,
) -> Result<(), CliError> {
    let path_buf = PathBuf::from(path);

    if !path_buf.exists() {
        return Err(CliError::Migration(format!(
            "Migration directory not found: {}",
            path_buf.display()
        )));
    }

    let db_type = DbType::from_str(db_type_str)
        .ok_or_else(|| CliError::Migration(format!("Unknown database type: {}", db_type_str)))?;

    let migrations = resolve_migrations(&path_buf, db_type)?;

    if migrations.is_empty() {
        println!("No migrations found in: {}", path_buf.display());
        return Ok(());
    }

    // 在线模式：查询数据库已应用版本
    let applied_versions = if let Some(url) = url {
        let mut conn = create_connection(url, db_type).await?;
        ensure_migrations_table(&mut conn, db_type).await?;
        fetch_applied_versions(&mut conn, db_type).await?
    } else {
        std::collections::HashSet::new()
    };

    // 表格输出（对齐 PHP migrate:status 格式）
    println!(
        "{:<15} {:<30} {:<20}",
        "Version", "Migration Name", "Status"
    );
    println!("{}", "-".repeat(65));

    for m in &migrations {
        let status = if applied_versions.contains(&m.version) {
            "Applied"
        } else if url.is_some() {
            "Pending"
        } else {
            "Pending*"
        };
        println!("{:<15} {:<30} {:<20}", m.version, m.name, status);
        if show_sql {
            println!("{}", print_sql_block("SQL UP", &m.sql_up));
            println!("{}", print_sql_block("SQL DOWN", &m.sql_down));
        }
    }

    println!();
    if url.is_some() {
        let applied = migrations
            .iter()
            .filter(|m| applied_versions.contains(&m.version))
            .count();
        println!(
            "Total: {} migration(s), {} applied, {} pending.",
            migrations.len(),
            applied,
            migrations.len() - applied
        );
    } else {
        println!("* Status cannot be determined without database connection (offline mode).");
    }

    Ok(())
}

/// 创建数据库连接（支持 5 后端：PostgreSQL/MySQL/SQLite/Oracle/MSSQL）
///
/// DSN scheme 自动识别后端：`postgres://` / `mysql://` / `sqlite:` /
/// `oracle://` / `mssql://`。`db_type` 仅用于错误诊断，实际后端由 DSN 决定。
///
/// - MSSQL：绕过 AnyPool，直接用 MssqlPoolHandle 添加 TLS 配置
/// - Oracle：绕过 AnyPool，用 `std::mem::forget` 阻止 OraclePoolHandle drop，
///   避免 sz-orm-oracle 内部独立 tokio Runtime 在 async 上下文中 drop 时 panic
async fn create_connection(url: &str, db_type: DbType) -> Result<Box<dyn Connection>, CliError> {
    use std::sync::Arc;
    use sz_orm_sqlx::any_driver::AnyPool;

    if db_type == DbType::SqlServer {
        use sz_orm_mssql::{MssqlConnectionFactory, MssqlPoolHandle};
        use sz_rust_core::orm::ConnectionFactory;

        let rest = url
            .strip_prefix("mssql://")
            .or_else(|| url.strip_prefix("sqlserver://"))
            .ok_or_else(|| CliError::Migration("Invalid MSSQL DSN".to_string()))?;
        let (userinfo, hostinfo) = rest
            .split_once('@')
            .ok_or_else(|| CliError::Migration("MSSQL DSN missing @".to_string()))?;
        let (username, password) = userinfo
            .split_once(':')
            .ok_or_else(|| CliError::Migration("MSSQL DSN missing password".to_string()))?;
        let (host_port, database) = hostinfo
            .split_once('/')
            .ok_or_else(|| CliError::Migration("MSSQL DSN missing database".to_string()))?;
        let (host, port) = host_port.split_once(':').unwrap_or((host_port, "1433"));
        let ado = format!(
            "Server={host},{port};Database={database};User Id={username};Password={password};\
             Encrypt=false;TrustServerCertificate=true;"
        );
        let pool = MssqlPoolHandle::connect(&ado)
            .await
            .map_err(|e| CliError::Migration(format!("MSSQL connect failed: {e}")))?;
        let factory = MssqlConnectionFactory::new(Arc::new(pool));
        let conn = factory
            .create()
            .await
            .map_err(|e| CliError::Migration(format!("MSSQL acquire failed: {e}")))?;
        return Ok(conn);
    }

    if db_type == DbType::Oracle {
        use sz_orm_oracle::{OracleConnectionFactory, OraclePoolHandle};
        use sz_rust_core::orm::ConnectionFactory;

        let rest = url
            .strip_prefix("oracle://")
            .ok_or_else(|| CliError::Migration("Invalid Oracle DSN".to_string()))?;
        let (userinfo, hostinfo) = rest
            .split_once('@')
            .ok_or_else(|| CliError::Migration("Oracle DSN missing @".to_string()))?;
        let (username, password) = userinfo
            .split_once(':')
            .ok_or_else(|| CliError::Migration("Oracle DSN missing password".to_string()))?;
        let pool = OraclePoolHandle::connect(username, password, hostinfo)
            .map_err(|e| CliError::Migration(format!("Oracle connect failed: {e}")))?;
        let pool_arc = Arc::new(pool);
        let factory = OracleConnectionFactory::new(pool_arc.clone());
        std::mem::forget(pool_arc);
        let conn = factory
            .create()
            .await
            .map_err(|e| CliError::Migration(format!("Oracle acquire failed: {e}")))?;
        return Ok(conn);
    }

    let pool = AnyPool::connect(url)
        .await
        .map_err(|e| CliError::Migration(format!("{:?} connect failed: {}", db_type, e)))?;
    let conn = pool
        .create()
        .await
        .map_err(|e| CliError::Migration(format!("{:?} acquire failed: {}", db_type, e)))?;
    Ok(Box::new(conn))
}

/// 确保 __migrations 表存在
async fn ensure_migrations_table(
    conn: &mut Box<dyn Connection>,
    db_type: DbType,
) -> Result<(), CliError> {
    let sql: &str = match db_type {
        DbType::PostgreSQL | DbType::Sqlite => {
            "CREATE TABLE IF NOT EXISTS __migrations (
                version VARCHAR(255) PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                batch INTEGER NOT NULL,
                executed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            )"
        }
        DbType::MySQL => {
            "CREATE TABLE IF NOT EXISTS __migrations (
                version VARCHAR(255) PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                batch INT NOT NULL,
                executed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            )"
        }
        DbType::Oracle => {
            "CREATE TABLE \"__migrations\" (\
                version VARCHAR2(255) PRIMARY KEY,\
                name VARCHAR2(255) NOT NULL,\
                batch NUMBER(10) NOT NULL,\
                executed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP\
            )"
        }
        DbType::SqlServer => {
            "CREATE TABLE __migrations (\
                 version NVARCHAR(255) PRIMARY KEY,\
                 name NVARCHAR(255) NOT NULL,\
                 batch INT NOT NULL,\
                 executed_at DATETIME2 NOT NULL DEFAULT CURRENT_TIMESTAMP\
             )"
        }
        _ => {
            return Err(CliError::Migration(format!(
                "Cannot ensure __migrations table for db_type {:?}",
                db_type
            )))
        }
    };
    let result = conn.execute(sql).await;
    match result {
        Ok(_) => Ok(()),
        Err(e) if db_type == DbType::SqlServer => {
            let err_msg = format!("{}", e);
            if err_msg.contains("2714") || err_msg.contains("already exists") {
                Ok(())
            } else {
                Err(CliError::Migration(format!(
                    "Failed to create __migrations table: {}",
                    e
                )))
            }
        }
        Err(e) if db_type == DbType::Oracle => {
            let err_msg = format!("{}", e);
            if err_msg.contains("ORA-00955") || err_msg.contains("already exists") {
                Ok(())
            } else {
                Err(CliError::Migration(format!(
                    "Failed to create __migrations table: {}",
                    e
                )))
            }
        }
        Err(e) => Err(CliError::Migration(format!(
            "Failed to create __migrations table: {}",
            e
        ))),
    }
}

/// 返回 __migrations 表名（Oracle 需双引号包裹，因 `__` 前缀在 Oracle 中非法）
fn migrations_table_name(db_type: DbType) -> &'static str {
    match db_type {
        DbType::Oracle => "\"__migrations\"",
        _ => "__migrations",
    }
}

/// Oracle 不允许 SQL 末尾分号，执行前去除
fn prepare_sql_for_db(sql: &str, db_type: DbType) -> String {
    if db_type == DbType::Oracle {
        sql.trim_end().trim_end_matches(';').to_string()
    } else {
        sql.to_string()
    }
}

/// 查询已应用的迁移版本
///
/// 执行 `SELECT version FROM __migrations` 查询已应用的迁移版本集合。
/// 用于 `migrate:status` 在线模式区分已应用/未应用迁移。
async fn fetch_applied_versions(
    conn: &mut Box<dyn Connection>,
    db_type: DbType,
) -> Result<std::collections::HashSet<String>, CliError> {
    let table = migrations_table_name(db_type);
    let sql = format!("SELECT version FROM {}", table);
    let rows = conn
        .query(&sql)
        .await
        .map_err(|e| CliError::Migration(format!("Failed to query __migrations: {}", e)))?;

    let mut versions = std::collections::HashSet::new();
    for row in &rows {
        use sz_rust_core::orm::Value;
        let val = row.get("version").or_else(|| row.get("VERSION"));
        if let Some(val) = val {
            match val {
                Value::String(s) => versions.insert(s.clone()),
                Value::I64(i) => versions.insert(i.to_string()),
                Value::I32(i) => versions.insert(i.to_string()),
                _ => false,
            };
        }
    }
    Ok(versions)
}

/// 删除 __migrations 表中的迁移记录
///
/// 参数化绑定防 SQL 注入（铁律 §1）：`version` 虽源自迁移文件名而非用户输入，
/// 仍统一走 `execute_with_params` 参数化路径，杜绝任何拼接风险。
/// 插入迁移记录到 __migrations 表
async fn insert_migration_record(
    conn: &mut Box<dyn Connection>,
    version: &str,
    name: &str,
    db_type: DbType,
) -> Result<(), CliError> {
    if !matches!(
        db_type,
        DbType::PostgreSQL | DbType::Sqlite | DbType::MySQL | DbType::Oracle | DbType::SqlServer
    ) {
        return Ok(());
    }
    use sz_rust_core::orm::Value;
    let table = migrations_table_name(db_type);
    let sql = format!(
        "INSERT INTO {} (version, name, batch) VALUES (?, ?, 1)",
        table
    );
    conn.execute_with_params(
        &sql,
        &[
            Value::String(version.to_string()),
            Value::String(name.to_string()),
        ],
    )
    .await
    .map_err(|e| CliError::Migration(format!("Failed to insert migration record: {}", e)))?;
    conn.commit()
        .await
        .map_err(|e| CliError::Migration(format!("Failed to commit: {}", e)))?;
    Ok(())
}

async fn delete_migration_record(
    conn: &mut Box<dyn Connection>,
    version: &str,
    db_type: DbType,
) -> Result<(), CliError> {
    if !matches!(
        db_type,
        DbType::PostgreSQL | DbType::Sqlite | DbType::MySQL | DbType::Oracle | DbType::SqlServer
    ) {
        return Ok(());
    }
    use sz_rust_core::orm::Value;
    let table = migrations_table_name(db_type);
    let sql = format!("DELETE FROM {} WHERE version = ?", table);
    conn.execute_with_params(&sql, &[Value::String(version.to_string())])
        .await
        .map_err(|e| CliError::Migration(format!("Failed to delete migration record: {}", e)))?;
    conn.commit()
        .await
        .map_err(|e| CliError::Migration(format!("Failed to commit: {}", e)))?;
    Ok(())
}

/// 解析迁移目录，返回排序后的迁移列表
///
/// 整合 [`FileMigrationResolver`]，对齐 sz-orm 的迁移文件命名约定。
///
/// # 错误
///
/// - [`CliError::Migration`]：目录读取失败或迁移文件解析失败
fn resolve_migrations(path: &Path, db_type: DbType) -> Result<Vec<Migration>, CliError> {
    let resolver = FileMigrationResolver::new(path.to_path_buf());
    resolver
        .resolve(db_type)
        .map_err(|e| CliError::Migration(format!("Failed to resolve migrations: {}", e)))
}

/// 打印 SQL 代码块（带标题分隔符）
///
/// 格式：
/// ```text
///   --- <title> ---
///   <sql content>
///   ----------------
/// ```
/// 返回格式化后的 SQL 代码块字符串（空 SQL 返回空串），由调用方输出。
fn print_sql_block(title: &str, sql: &str) -> String {
    if sql.is_empty() {
        return String::new();
    }
    let mut out = format!("  --- {} ---\n", title);
    for line in sql.lines() {
        out.push_str(&format!("  {}\n", line));
    }
    out.push_str(&format!("  {}\n", "-".repeat(title.len() + 8)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    /// 创建测试用迁移文件（`<version>_<name>_up.sql` + `<version>_<name>_down.sql`）
    fn create_test_migration(dir: &Path, version: &str, name: &str) {
        let up_name = format!("{}_{}_up.sql", version, name);
        let down_name = format!("{}_{}_down.sql", version, name);

        let up_path = dir.join(up_name);
        let down_path = dir.join(down_name);

        let mut up_file = fs::File::create(&up_path).unwrap();
        writeln!(up_file, "-- {} up", name).unwrap();

        let mut down_file = fs::File::create(&down_path).unwrap();
        writeln!(down_file, "-- {} down", name).unwrap();
    }

    #[test]
    fn test_resolve_migrations_empty() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let result = resolve_migrations(&path, DbType::PostgreSQL).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_resolve_migrations_with_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();

        create_test_migration(&path, "001", "create_users");
        create_test_migration(&path, "002", "add_index");

        let result = resolve_migrations(&path, DbType::PostgreSQL).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].version, "001");
        assert_eq!(result[0].name, "create_users");
        assert_eq!(result[1].version, "002");
        assert_eq!(result[1].name, "add_index");
    }

    #[test]
    fn test_resolve_migrations_returns_sql_content() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();

        let up_path = path.join("001_init_up.sql");
        let down_path = path.join("001_init_down.sql");
        fs::write(&up_path, "CREATE TABLE users (id INT);").unwrap();
        fs::write(&down_path, "DROP TABLE users;").unwrap();

        let result = resolve_migrations(&path, DbType::PostgreSQL).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].sql_up.contains("CREATE TABLE users"));
        assert!(result[0].sql_down.contains("DROP TABLE users"));
    }

    #[test]
    fn test_resolve_migrations_supports_multiple_db_types() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        create_test_migration(&path, "001", "init");

        let mysql_result = resolve_migrations(&path, DbType::MySQL).unwrap();
        let pg_result = resolve_migrations(&path, DbType::PostgreSQL).unwrap();

        assert_eq!(mysql_result.len(), 1);
        assert_eq!(pg_result.len(), 1);
    }

    #[tokio::test]
    async fn test_execute_status_nonexistent_dir() {
        let result = execute_status("/nonexistent/path/migrations").await;
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[tokio::test]
    async fn test_execute_status_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_str().unwrap();
        let result = execute_status(path).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_status_with_migrations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        create_test_migration(&path, "001", "create_users");

        let path_str = temp.path().to_str().unwrap();
        let result = execute_status(path_str).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_status_with_invalid_db_type() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_str().unwrap();
        let result = execute_status_with(path, "invalid_db_type", false).await;
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[tokio::test]
    async fn test_execute_status_with_show_sql() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();

        let up_path = path.join("001_init_up.sql");
        let down_path = path.join("001_init_down.sql");
        fs::write(&up_path, "CREATE TABLE users (id INT);").unwrap();
        fs::write(&down_path, "DROP TABLE users;").unwrap();

        let path_str = temp.path().to_str().unwrap();
        let result = execute_status_with(path_str, "postgres", true).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_migrate_nonexistent_dir() {
        let args = MigrateArgs {
            rollback: false,
            path: "/nonexistent/migrations".to_string(),
            db_type: "postgres".to_string(),
            show_sql: false,
            url: None,
        };
        let result = execute_migrate(&args).await;
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[tokio::test]
    async fn test_execute_migrate_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        let args = MigrateArgs {
            rollback: false,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "postgres".to_string(),
            show_sql: false,
            url: None,
        };
        let result = execute_migrate(&args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_migrate_with_files_offline() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        create_test_migration(&path, "001", "create_users");

        let args = MigrateArgs {
            rollback: false,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "postgres".to_string(),
            show_sql: false,
            url: None,
        };
        let result = execute_migrate(&args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_migrate_with_show_sql_offline() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();

        let up_path = path.join("001_init_up.sql");
        let down_path = path.join("001_init_down.sql");
        fs::write(&up_path, "CREATE TABLE users (id INT);").unwrap();
        fs::write(&down_path, "DROP TABLE users;").unwrap();

        let args = MigrateArgs {
            rollback: false,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "postgres".to_string(),
            show_sql: true,
            url: None,
        };
        let result = execute_migrate(&args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_migrate_with_invalid_db_type() {
        let temp = tempfile::tempdir().unwrap();
        let args = MigrateArgs {
            rollback: false,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "invalid_db_type".to_string(),
            show_sql: false,
            url: None,
        };
        let result = execute_migrate(&args).await;
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[tokio::test]
    async fn test_execute_migrate_rollback_offline() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        create_test_migration(&path, "001", "create_users");
        create_test_migration(&path, "002", "add_index");

        let args = MigrateArgs {
            rollback: true,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "postgres".to_string(),
            show_sql: false,
            url: None,
        };
        let result = execute_migrate(&args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_migrate_rollback_with_show_sql_offline() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();

        let up_path = path.join("001_init_up.sql");
        let down_path = path.join("001_init_down.sql");
        fs::write(&up_path, "CREATE TABLE users (id INT);").unwrap();
        fs::write(&down_path, "DROP TABLE users;").unwrap();

        let args = MigrateArgs {
            rollback: true,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "postgres".to_string(),
            show_sql: true,
            url: None,
        };
        let result = execute_migrate(&args).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_sql_block_empty_sql() {
        // 空 SQL 不输出任何内容
        let out = print_sql_block("SQL UP", "");
        assert!(out.is_empty(), "空 SQL 不应产生输出，实际: {:?}", out);
    }

    #[test]
    fn test_print_sql_block_with_content() {
        let out = print_sql_block("SQL UP", "CREATE TABLE users (id INT);");
        assert!(out.contains("--- SQL UP ---"), "应包含标题, 实际: {out}");
        assert!(
            out.contains("CREATE TABLE users (id INT);"),
            "应包含 SQL 内容, 实际: {out}"
        );
        assert!(
            out.contains(&"-".repeat("SQL UP".len() + 8)),
            "应以分隔线结尾, 实际: {:?}",
            out
        );
    }

    #[tokio::test]
    async fn test_execute_status_full_offline_no_url() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        create_test_migration(&path, "001", "init");

        let path_str = temp.path().to_str().unwrap();
        let result = execute_status_full(path_str, "postgres", false, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_status_full_offline_with_show_sql() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let up_path = path.join("001_init_up.sql");
        let down_path = path.join("001_init_down.sql");
        fs::write(&up_path, "CREATE TABLE t (id INT);").unwrap();
        fs::write(&down_path, "DROP TABLE t;").unwrap();

        let path_str = temp.path().to_str().unwrap();
        let result = execute_status_full(path_str, "postgres", true, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_status_full_invalid_db_type() {
        let temp = tempfile::tempdir().unwrap();
        let path_str = temp.path().to_str().unwrap();
        let result = execute_status_full(path_str, "invalid_db", false, None).await;
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[tokio::test]
    async fn test_execute_migrate_online_with_invalid_url_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        create_test_migration(&path, "001", "init");

        let args = MigrateArgs {
            rollback: false,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "postgres".to_string(),
            show_sql: false,
            url: Some("postgres://invalid:invalid@127.0.0.1:1/invalid".to_string()),
        };
        let result = execute_migrate(&args).await;
        // 连接失败应返回错误（不 panic）
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_connection_oracle_dsn_attempts_connect() {
        let result = create_connection(
            "oracle://invalid:invalid@127.0.0.1:1/invalid",
            DbType::Oracle,
        )
        .await;
        match result {
            Err(e) => {
                let err = format!("{}", e);
                assert!(
                    !err.contains("not supported"),
                    "Oracle 应尝试连接而非拒绝: {err}"
                );
            }
            Ok(_) => panic!("Oracle 连接应失败"),
        }
    }

    #[tokio::test]
    async fn test_create_connection_mssql_dsn_attempts_connect() {
        let result = create_connection(
            "mssql://invalid:invalid@127.0.0.1:1/invalid",
            DbType::SqlServer,
        )
        .await;
        match result {
            Err(e) => {
                let err = format!("{}", e);
                assert!(
                    !err.contains("not supported"),
                    "MSSQL 应尝试连接而非拒绝: {err}"
                );
            }
            Ok(_) => panic!("MSSQL 连接应失败"),
        }
    }

    #[test]
    fn test_migrations_table_name_postgres() {
        assert_eq!(migrations_table_name(DbType::PostgreSQL), "__migrations");
    }

    #[test]
    fn test_migrations_table_name_mysql() {
        assert_eq!(migrations_table_name(DbType::MySQL), "__migrations");
    }

    #[test]
    fn test_migrations_table_name_sqlite() {
        assert_eq!(migrations_table_name(DbType::Sqlite), "__migrations");
    }

    #[test]
    fn test_migrations_table_name_oracle() {
        assert_eq!(migrations_table_name(DbType::Oracle), "\"__migrations\"");
    }

    #[test]
    fn test_migrations_table_name_mssql() {
        assert_eq!(migrations_table_name(DbType::SqlServer), "__migrations");
    }

    #[test]
    fn test_prepare_sql_for_db_postgres() {
        let sql = "CREATE TABLE users (id INT);";
        assert_eq!(prepare_sql_for_db(sql, DbType::PostgreSQL), sql);
    }

    #[test]
    fn test_prepare_sql_for_db_oracle_strips_semicolon() {
        let sql = "CREATE TABLE users (id INT);";
        assert_eq!(
            prepare_sql_for_db(sql, DbType::Oracle),
            "CREATE TABLE users (id INT)"
        );
    }

    #[test]
    fn test_prepare_sql_for_db_oracle_no_semicolon() {
        let sql = "CREATE TABLE users (id INT)";
        assert_eq!(prepare_sql_for_db(sql, DbType::Oracle), sql);
    }

    #[test]
    fn test_prepare_sql_for_db_oracle_trailing_whitespace() {
        let sql = "CREATE TABLE users (id INT);  \n";
        assert_eq!(
            prepare_sql_for_db(sql, DbType::Oracle),
            "CREATE TABLE users (id INT)"
        );
    }

    #[test]
    fn test_prepare_sql_for_db_mysql_no_change() {
        let sql = "CREATE TABLE users (id INT);";
        assert_eq!(prepare_sql_for_db(sql, DbType::MySQL), sql);
    }
}
