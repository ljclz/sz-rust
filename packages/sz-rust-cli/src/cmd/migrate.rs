//! `migrate` / `migrate:status` 命令 — 对齐 PHP `think migrate*`
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
//! Rust 端使用 `sz-orm-core::migration::{FileMigrationResolver, MigrationResolver}` 读取迁移目录。

use std::path::{Path, PathBuf};

use clap::Args;

use crate::error::CliError;

/// `migrate` 命令参数
#[derive(Args, Debug)]
pub struct MigrateArgs {
    /// 回滚最后一批迁移（对齐 PHP `migrate:rollback`）
    #[arg(long)]
    pub rollback: bool,

    /// 迁移目录（默认 `migrations`）
    #[arg(short = 'p', long, default_value = "migrations")]
    pub path: String,
}

/// 执行 migrate 命令
///
/// - 无 `--rollback`：执行所有待迁移（对齐 `php think migrate`）
/// - 有 `--rollback`：回滚最后一批（对齐 `php think migrate:rollback`）
pub fn execute_migrate(args: &MigrateArgs) -> Result<(), CliError> {
    let path = PathBuf::from(&args.path);

    if !path.exists() {
        return Err(CliError::Migration(format!(
            "Migration directory not found: {}",
            path.display()
        )));
    }

    if args.rollback {
        println!("Rolling back last batch in: {}", path.display());
        // 离线模式：仅列出回滚将影响的迁移
        let migrations = list_migrations(&path)?;
        if migrations.is_empty() {
            println!("No migrations to rollback.");
            return Ok(());
        }
        // 模拟回滚：显示最后一个迁移
        if let Some(last) = migrations.last() {
            println!("Would rollback: {} ({})", last.0, last.1);
        }
        println!("Note: Actual rollback requires database connection.");
    } else {
        println!("Running migrations in: {}", path.display());
        let migrations = list_migrations(&path)?;
        if migrations.is_empty() {
            println!("No migrations found.");
            return Ok(());
        }
        for (version, name) in &migrations {
            println!("  Would apply: {} ({})", version, name);
        }
        println!(
            "Total: {} migration(s). Note: Actual execution requires database connection.",
            migrations.len()
        );
    }

    Ok(())
}

/// 执行 migrate:status 命令
///
/// 对齐 PHP `php think migrate:status`，输出表格格式的迁移状态。
pub fn execute_status(path: &str) -> Result<(), CliError> {
    let path_buf = PathBuf::from(path);

    if !path_buf.exists() {
        return Err(CliError::Migration(format!(
            "Migration directory not found: {}",
            path_buf.display()
        )));
    }

    let migrations = list_migrations(&path_buf)?;

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

    for (version, name) in &migrations {
        // 离线模式：无法确定是否已执行，统一显示 "Pending*"
        println!("{:<15} {:<30} {:<20}", version, name, "Pending*");
    }

    println!();
    println!("* Status cannot be determined without database connection (offline mode).");

    Ok(())
}

/// 列出迁移目录中的所有迁移
///
/// 对齐 `FileMigrationResolver::resolve()` 的文件名解析逻辑：
/// `<version>_<name>_up.sql` / `<version>_<name>_down.sql`
fn list_migrations(path: &Path) -> Result<Vec<(String, String)>, CliError> {
    let mut versions: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    let entries = std::fs::read_dir(path)?;
    for entry in entries {
        let entry = entry?;
        let file_path = entry.path();
        let filename = match file_path.file_stem().and_then(|s| s.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        // 解析文件名：<version>_<name>_up 或 <version>_<name>_down
        let base = if let Some(rest) = filename.strip_suffix("_up") {
            rest
        } else if let Some(rest) = filename.strip_suffix("_down") {
            rest
        } else {
            &filename
        };

        if let Some(underscore_pos) = base.find('_') {
            let version = base[..underscore_pos].to_string();
            let name = base[underscore_pos + 1..].to_string();
            versions.entry(version).or_insert(name);
        } else {
            versions
                .entry(base.to_string())
                .or_insert_with(|| base.to_string());
        }
    }

    Ok(versions.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

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
    fn test_list_migrations_empty() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let result = list_migrations(&path).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_list_migrations_with_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();

        create_test_migration(&path, "001", "create_users");
        create_test_migration(&path, "002", "add_index");

        let result = list_migrations(&path).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], ("001".to_string(), "create_users".to_string()));
        assert_eq!(result[1], ("002".to_string(), "add_index".to_string()));
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
    fn test_execute_migrate_nonexistent_dir() {
        let args = MigrateArgs {
            rollback: false,
            path: "/nonexistent/migrations".to_string(),
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
        };
        let result = execute_migrate(&args);
        assert!(result.is_ok());
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
        };
        let result = execute_migrate(&args);
        assert!(result.is_ok());
    }
}
