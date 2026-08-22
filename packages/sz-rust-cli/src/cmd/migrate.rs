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
use sz_rust_core::orm::migration::{
    FileMigrationResolver, Migration, MigrationContext, MigrationResolver, Migrator,
};
use sz_rust_core::orm::{Connection, ConnectionFactory, DbType};

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
pub fn execute_migrate(args: &MigrateArgs) -> Result<(), CliError> {
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
        Some(url) => execute_migrate_online(args, &migrations, url, db_type),
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
fn execute_migrate_online(
    args: &MigrateArgs,
    migrations: &[Migration],
    url: &str,
    db_type: DbType,
) -> Result<(), CliError> {
    // 阻塞执行异步逻辑
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Migration(format!("Failed to create tokio runtime: {}", e)))?;

    rt.block_on(async move {
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
                conn.execute(&last.sql_down)
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

            // 构建 Migrator 并执行
            let mut context = MigrationContext::default().with_db_type(db_type);
            context.connection = Some(conn);

            let mut migrator = Migrator::new(context);
            for m in migrations {
                // Migration 未实现 Clone，按字段重建实例
                let rebuilt = Migration::new(&m.version, &m.name, &m.sql_up, &m.sql_down);
                if applied.contains(&m.version) {
                    // 标记为已执行（batch>0），避免 Migrator 重复执行
                    migrator = migrator.add_migration(rebuilt.with_batch(1));
                } else {
                    migrator = migrator.add_migration(rebuilt);
                }
            }

            let applied_versions = migrator
                .migrate()
                .await
                .map_err(|e| CliError::Migration(format!("Migration failed: {}", e)))?;

            for v in &applied_versions {
                println!("  Applied: {}", v);
            }
            println!("Migration completed: {} applied.", applied_versions.len());
        }

        Ok::<(), CliError>(())
    })
}

/// 执行 migrate:status 命令（兼容入口，使用默认 `postgres` 方言）
///
/// 对齐 PHP `php think migrate:status`，输出表格格式的迁移状态。
///
/// 等价于 [`execute_status_with`] 传入 `db_type="postgres"`、`show_sql=false`、`url=None`。
pub fn execute_status(path: &str) -> Result<(), CliError> {
    execute_status_full(path, "postgres", false, None)
}

/// 执行 migrate:status 命令（完整参数）
///
/// # 参数
///
/// - `path`：迁移目录
/// - `db_type_str`：数据库类型字符串（由 `DbType::from_str` 解析）
/// - `show_sql`：是否打印每个迁移的 SQL 内容
pub fn execute_status_with(path: &str, db_type_str: &str, show_sql: bool) -> Result<(), CliError> {
    execute_status_full(path, db_type_str, show_sql, None)
}

/// 执行 migrate:status 命令（完整参数，含在线模式）
pub fn execute_status_full(
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
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| CliError::Migration(format!("Failed to create tokio runtime: {}", e)))?;
        rt.block_on(async move {
            let mut conn = create_connection(url, db_type).await?;
            ensure_migrations_table(&mut conn, db_type).await?;
            fetch_applied_versions(&mut conn, db_type).await
        })?
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

/// 创建数据库连接（按 DbType 选择驱动）
async fn create_connection(url: &str, db_type: DbType) -> Result<Box<dyn Connection>, CliError> {
    use std::sync::Arc;
    use sz_orm_sqlx::{
        MySqlPoolHandle, PgPoolHandle, SqlitePoolHandle, SqlxMySqlConnectionFactory,
        SqlxPgConnectionFactory, SqlxSqliteConnectionFactory,
    };

    match db_type {
        DbType::PostgreSQL => {
            let pool = PgPoolHandle::connect(url).await.map_err(|e| {
                CliError::Migration(format!("PostgreSQL connect failed: {}", e))
            })?;
            let factory = SqlxPgConnectionFactory::new(Arc::new(pool));
            let conn = factory.create().await.map_err(|e| {
                CliError::Migration(format!("PostgreSQL acquire failed: {}", e))
            })?;
            Ok(conn)
        }
        DbType::MySQL => {
            let pool = MySqlPoolHandle::connect(url).await.map_err(|e| {
                CliError::Migration(format!("MySQL connect failed: {}", e))
            })?;
            let factory = SqlxMySqlConnectionFactory::new(Arc::new(pool));
            let conn = factory.create().await.map_err(|e| {
                CliError::Migration(format!("MySQL acquire failed: {}", e))
            })?;
            Ok(conn)
        }
        DbType::Sqlite => {
            let pool = SqlitePoolHandle::connect(url).await.map_err(|e| {
                CliError::Migration(format!("SQLite connect failed: {}", e))
            })?;
            let factory = SqlxSqliteConnectionFactory::new(Arc::new(pool));
            let conn = factory.create().await.map_err(|e| {
                CliError::Migration(format!("SQLite acquire failed: {}", e))
            })?;
            Ok(conn)
        }
        _ => Err(CliError::Migration(format!(
            "Online migration not supported for db_type {:?}. Supported: PostgreSQL, MySQL, SQLite.",
            db_type
        ))),
    }
}

/// 确保 __migrations 表存在
async fn ensure_migrations_table(
    conn: &mut Box<dyn Connection>,
    db_type: DbType,
) -> Result<(), CliError> {
    let sql = match db_type {
        DbType::PostgreSQL | DbType::Sqlite => {
            "CREATE TABLE IF NOT EXISTS __migrations (
                version VARCHAR(255) PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                batch INTEGER NOT NULL,
                run_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            )"
        }
        DbType::MySQL => {
            "CREATE TABLE IF NOT EXISTS __migrations (
                version VARCHAR(255) PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                batch INT NOT NULL,
                run_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            )"
        }
        _ => {
            return Err(CliError::Migration(format!(
                "Cannot ensure __migrations table for db_type {:?}",
                db_type
            )))
        }
    };
    conn.execute(sql)
        .await
        .map_err(|e| CliError::Migration(format!("Failed to create __migrations table: {}", e)))?;
    Ok(())
}

/// 查询已应用的迁移版本
///
/// 执行 `SELECT version FROM __migrations` 查询已应用的迁移版本集合。
/// 用于 `migrate:status` 在线模式区分已应用/未应用迁移。
async fn fetch_applied_versions(
    conn: &mut Box<dyn Connection>,
    _db_type: DbType,
) -> Result<std::collections::HashSet<String>, CliError> {
    // 查询 __migrations 表中所有已记录的迁移版本
    // QueryRows = Vec<HashMap<String, Value>>
    let rows = conn
        .query("SELECT version FROM __migrations")
        .await
        .map_err(|e| CliError::Migration(format!("Failed to query __migrations: {}", e)))?;

    let mut versions = std::collections::HashSet::new();
    for row in &rows {
        // version 列为字符串类型，尝试从 HashMap 提取
        if let Some(val) = row.get("version") {
            use sz_rust_core::orm::Value;
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
async fn delete_migration_record(
    conn: &mut Box<dyn Connection>,
    version: &str,
    db_type: DbType,
) -> Result<(), CliError> {
    if !matches!(db_type, DbType::PostgreSQL | DbType::Sqlite | DbType::MySQL) {
        return Ok(());
    }
    let params = [sz_rust_core::orm::Value::String(version.to_string())];
    conn.execute_with_params("DELETE FROM __migrations WHERE version = ?", &params)
        .await
        .map_err(|e| CliError::Migration(format!("Failed to delete migration record: {}", e)))?;
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

    #[test]
    fn test_execute_status_nonexistent_dir() {
        let result = execute_status("/nonexistent/path/migrations");
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[test]
    fn test_execute_status_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_str().unwrap();
        let result = execute_status(path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_status_with_migrations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        create_test_migration(&path, "001", "create_users");

        let path_str = temp.path().to_str().unwrap();
        let result = execute_status(path_str);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_status_with_invalid_db_type() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_str().unwrap();
        let result = execute_status_with(path, "invalid_db_type", false);
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[test]
    fn test_execute_status_with_show_sql() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();

        let up_path = path.join("001_init_up.sql");
        let down_path = path.join("001_init_down.sql");
        fs::write(&up_path, "CREATE TABLE users (id INT);").unwrap();
        fs::write(&down_path, "DROP TABLE users;").unwrap();

        let path_str = temp.path().to_str().unwrap();
        let result = execute_status_with(path_str, "postgres", true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_migrate_nonexistent_dir() {
        let args = MigrateArgs {
            rollback: false,
            path: "/nonexistent/migrations".to_string(),
            db_type: "postgres".to_string(),
            show_sql: false,
            url: None,
        };
        let result = execute_migrate(&args);
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[test]
    fn test_execute_migrate_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        let args = MigrateArgs {
            rollback: false,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "postgres".to_string(),
            show_sql: false,
            url: None,
        };
        let result = execute_migrate(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_migrate_with_files_offline() {
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
        let result = execute_migrate(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_migrate_with_show_sql_offline() {
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
        let result = execute_migrate(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_migrate_with_invalid_db_type() {
        let temp = tempfile::tempdir().unwrap();
        let args = MigrateArgs {
            rollback: false,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "invalid_db_type".to_string(),
            show_sql: false,
            url: None,
        };
        let result = execute_migrate(&args);
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[test]
    fn test_execute_migrate_rollback_offline() {
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
        let result = execute_migrate(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_migrate_rollback_with_show_sql_offline() {
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
        let result = execute_migrate(&args);
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

    #[test]
    fn test_execute_status_full_offline_no_url() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        create_test_migration(&path, "001", "init");

        let path_str = temp.path().to_str().unwrap();
        let result = execute_status_full(path_str, "postgres", false, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_status_full_offline_with_show_sql() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let up_path = path.join("001_init_up.sql");
        let down_path = path.join("001_init_down.sql");
        fs::write(&up_path, "CREATE TABLE t (id INT);").unwrap();
        fs::write(&down_path, "DROP TABLE t;").unwrap();

        let path_str = temp.path().to_str().unwrap();
        let result = execute_status_full(path_str, "postgres", true, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_status_full_invalid_db_type() {
        let temp = tempfile::tempdir().unwrap();
        let path_str = temp.path().to_str().unwrap();
        let result = execute_status_full(path_str, "invalid_db", false, None);
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[test]
    fn test_execute_migrate_online_with_invalid_url_returns_error() {
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
        let result = execute_migrate(&args);
        // 连接失败应返回错误（不 panic）
        assert!(result.is_err());
    }
}
