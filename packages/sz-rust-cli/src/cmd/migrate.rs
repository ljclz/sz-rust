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
//! 本模块使用 [`sz_orm_core::migration::FileMigrationResolver`] 解析迁移目录，
//! 对齐 sz-orm 的迁移文件命名约定（`<version>_<name>_up.sql` / `<version>_<name>_down.sql`）。
//!
//! ### 离线模式（默认）
//!
//! 不连接数据库，仅解析并列出迁移文件。`migrate` 命令输出"将执行的 SQL"，
//! `migrate:status` 输出迁移列表（状态统一显示 `Pending*`，因离线无法确定执行历史）。
//!
//! ### 在线模式（未来扩展）
//!
//! 通过 `Migrator::migrate()` 执行真实迁移，需要注入 `MigrationContext::connection`。
//! 当前 CLI 不直接依赖具体数据库驱动（如 `sz-orm-sqlx`），保持包体积精简。
//! 用户可基于本模块的解析结果，自行调用 `Migrator` API 执行迁移。

use std::path::{Path, PathBuf};

use clap::Args;
use sz_orm_core::migration::{FileMigrationResolver, Migration, MigrationResolver};
use sz_orm_core::DbType;

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
}

/// 执行 migrate 命令
///
/// - 无 `--rollback`：列出所有待迁移，可选打印 SQL（对齐 `php think migrate`）
/// - 有 `--rollback`：列出最后一个迁移作为回滚目标（对齐 `php think migrate:rollback`）
///
/// # 离线模式说明
///
/// 当前为离线模式：仅解析迁移目录并打印待执行内容，不连接数据库。
/// 真正执行迁移需要在线模式（未来扩展，通过 `Migrator::migrate` 注入连接）。
pub fn execute_migrate(args: &MigrateArgs) -> Result<(), CliError> {
    let path = PathBuf::from(&args.path);

    if !path.exists() {
        return Err(CliError::Migration(format!(
            "Migration directory not found: {}",
            path.display()
        )));
    }

    let db_type = DbType::from_str(&args.db_type).ok_or_else(|| {
        CliError::Migration(format!("Unknown database type: {}", args.db_type))
    })?;

    let migrations = resolve_migrations(&path, db_type)?;

    if migrations.is_empty() {
        println!("No migrations found in: {}", path.display());
        return Ok(());
    }

    if args.rollback {
        println!("Rolling back last batch in: {}", path.display());
        // 离线模式：回滚目标为列表中最后一个迁移
        if let Some(last) = migrations.last() {
            println!("  Would rollback: {} ({})", last.version, last.name);
            if args.show_sql {
                print_sql_block("SQL DOWN", &last.sql_down);
            }
        }
        println!("Note: Actual rollback requires database connection (offline mode).");
    } else {
        println!("Running migrations in: {}", path.display());
        for m in &migrations {
            println!("  Would apply: {} ({})", m.version, m.name);
            if args.show_sql {
                print_sql_block("SQL UP", &m.sql_up);
            }
        }
        println!(
            "Total: {} migration(s). Note: Actual execution requires database connection (offline mode).",
            migrations.len()
        );
    }

    Ok(())
}

/// 执行 migrate:status 命令（兼容入口，使用默认 `postgres` 方言）
///
/// 对齐 PHP `php think migrate:status`，输出表格格式的迁移状态。
///
/// 等价于 [`execute_status_with`] 传入 `db_type="postgres"`、`show_sql=false`。
pub fn execute_status(path: &str) -> Result<(), CliError> {
    execute_status_with(path, "postgres", false)
}

/// 执行 migrate:status 命令（完整参数）
///
/// # 参数
///
/// - `path`：迁移目录
/// - `db_type_str`：数据库类型字符串（由 `DbType::from_str` 解析）
/// - `show_sql`：是否打印每个迁移的 SQL 内容
pub fn execute_status_with(path: &str, db_type_str: &str, show_sql: bool) -> Result<(), CliError> {
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

    // 表格输出（对齐 PHP migrate:status 格式）
    println!(
        "{:<15} {:<30} {:<20}",
        "Version", "Migration Name", "Status"
    );
    println!("{}", "-".repeat(65));

    for m in &migrations {
        // 离线模式：无法确定是否已执行，统一显示 "Pending*"
        println!("{:<15} {:<30} {:<20}", m.version, m.name, "Pending*");
        if show_sql {
            print_sql_block("SQL UP", &m.sql_up);
            print_sql_block("SQL DOWN", &m.sql_down);
        }
    }

    println!();
    println!("* Status cannot be determined without database connection (offline mode).");

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
fn print_sql_block(title: &str, sql: &str) {
    if sql.is_empty() {
        return;
    }
    println!("  --- {} ---", title);
    for line in sql.lines() {
        println!("  {}", line);
    }
    println!("  {}", "-".repeat(title.len() + 8));
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
        // 验证整合 sz-orm 后能正确读取 SQL 内容（不再是空 stub）
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
        // 验证 DbType 参数能正确传入（当前 FileMigrationResolver 不区分方言，但 API 兼容）
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
        };
        let result = execute_migrate(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_migrate_with_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        create_test_migration(&path, "001", "create_users");

        let args = MigrateArgs {
            rollback: false,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "postgres".to_string(),
            show_sql: false,
        };
        let result = execute_migrate(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_migrate_with_show_sql() {
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
        };
        let result = execute_migrate(&args);
        assert!(matches!(result, Err(CliError::Migration(_))));
    }

    #[test]
    fn test_execute_migrate_rollback() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        create_test_migration(&path, "001", "create_users");
        create_test_migration(&path, "002", "add_index");

        let args = MigrateArgs {
            rollback: true,
            path: temp.path().to_str().unwrap().to_string(),
            db_type: "postgres".to_string(),
            show_sql: false,
        };
        let result = execute_migrate(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_migrate_rollback_with_show_sql() {
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
        };
        let result = execute_migrate(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_sql_block_empty_sql() {
        // 空 SQL 不应输出任何内容（不 panic）
        print_sql_block("SQL UP", "");
    }

    #[test]
    fn test_print_sql_block_with_content() {
        // 非空 SQL 应正常输出（不 panic）
        print_sql_block("SQL UP", "CREATE TABLE users (id INT);");
    }
}
