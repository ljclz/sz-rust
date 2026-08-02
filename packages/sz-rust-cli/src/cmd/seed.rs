//! `db:seed` 命令 — 数据填充（对齐 PHP `think db:seed`）
//!
//! ## PHP 对齐
//!
//! PHP `db:seed` 通过 `Seeder` 类执行数据填充：
//! ```text
//! php think db:seed              # 运行默认 DatabaseSeeder
//! php think db:seed -s UserSeeder # 运行指定填充器
//! ```
//!
//! Rust 端由于静态编译无法按类名动态加载，采用以下策略：
//! - **SQL 文件模式**（默认）：从 `seeds/` 目录加载 `.sql` 文件并执行
//! - **程序化模式**：业务实现 `sz_rust_core::seed::Seeder` trait，通过 `SeedRunner` 注册执行
//!
//! ## 文件命名约定
//!
//! `seeds/` 目录下的 `.sql` 文件按文件名升序执行：
//! ```text
//! seeds/
//! ├── 001_users_seed.sql
//! ├── 002_roles_seed.sql
//! └── 003_user_roles_seed.sql
//! ```
//!
//! ## 模式
//!
//! - **离线模式**（默认，未提供 `--url`）：仅列出待执行的 SQL 文件内容
//! - **在线模式**（提供 `--url`）：连接数据库执行真实填充

use std::path::{Path, PathBuf};

use sz_rust_core::orm::{Connection, ConnectionFactory, DbType};

use crate::error::CliError;

/// `db:seed` 命令参数
///
/// 对齐 PHP `php think db:seed` / `php think db:seed -s <class>`。
#[derive(Debug, Clone)]
pub struct SeedArgs {
    /// 填充目录（默认 `seeds`）
    pub path: String,

    /// 数据库类型（默认 `postgres`，对齐 sz-orm `DbType`）
    pub db_type: String,

    /// 仅打印 SQL 内容（dry-run 模式，便于审查）
    pub show_sql: bool,

    /// 数据库连接 URL（启用在线模式）
    ///
    /// 提供时连接数据库执行真实填充；省略时为离线模式（仅列出待执行的 SQL）。
    pub url: Option<String>,

    /// 指定填充器文件名（不含扩展名，如 `001_users_seed`）
    ///
    /// 省略时执行目录下所有 `.sql` 文件。
    pub class: Option<String>,
}

impl Default for SeedArgs {
    fn default() -> Self {
        Self {
            path: "seeds".to_string(),
            db_type: "postgres".to_string(),
            show_sql: false,
            url: None,
            class: None,
        }
    }
}

/// 执行 db:seed 命令
///
/// # 模式
///
/// - **离线模式**（默认，未提供 `--url`）：仅解析填充目录并打印待执行内容
/// - **在线模式**（提供 `--url`）：连接数据库执行真实填充
pub fn execute_seed(args: &SeedArgs) -> Result<(), CliError> {
    let path = PathBuf::from(&args.path);

    if !path.exists() {
        return Err(CliError::Generic(format!(
            "Seed directory not found: {}",
            path.display()
        )));
    }

    let db_type = DbType::from_str(&args.db_type)
        .ok_or_else(|| CliError::Generic(format!("Unknown database type: {}", args.db_type)))?;

    let seed_files = resolve_seed_files(&path, args.class.as_deref())?;

    if seed_files.is_empty() {
        println!("No seed files found in: {}", path.display());
        return Ok(());
    }

    match &args.url {
        None => execute_seed_offline(args, &seed_files),
        Some(url) => execute_seed_online(args, &seed_files, url, db_type),
    }
}

/// 离线模式执行 seed（仅打印，不连库）
fn execute_seed_offline(args: &SeedArgs, seed_files: &[SeedFile]) -> Result<(), CliError> {
    println!("Seed files in: {}", args.path);
    for sf in seed_files {
        println!("  Would execute: {}", sf.name);
        if args.show_sql {
            print_sql_block("SQL", &sf.content);
        }
    }
    println!(
        "Total: {} seed file(s). Note: Actual execution requires database connection (offline mode).",
        seed_files.len()
    );
    Ok(())
}

/// 在线模式执行 seed（连接数据库真实执行）
fn execute_seed_online(
    args: &SeedArgs,
    seed_files: &[SeedFile],
    url: &str,
    db_type: DbType,
) -> Result<(), CliError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Generic(format!("Failed to create tokio runtime: {}", e)))?;

    rt.block_on(async move {
        let mut conn = create_connection(url, db_type).await?;

        println!("Running {} seed file(s):", seed_files.len());
        for sf in seed_files {
            println!("  Seeding: {}", sf.name);
            if args.show_sql {
                print_sql_block("SQL", &sf.content);
            }
            conn.execute(&sf.content)
                .await
                .map_err(|e| CliError::Generic(format!("Seed failed ({}): {}", sf.name, e)))?;
            println!("  Completed: {}", sf.name);
        }
        println!("Seed completed: {} file(s) applied.", seed_files.len());

        Ok::<(), CliError>(())
    })
}

/// 填充文件
#[derive(Debug, Clone)]
struct SeedFile {
    /// 文件名（含扩展名）
    name: String,
    /// SQL 内容
    content: String,
}

/// 解析填充目录，返回按文件名排序的填充文件列表
///
/// # 参数
///
/// - `path`：填充目录
/// - `class_filter`：可选的文件名过滤（不含扩展名）
///
/// # 错误
///
/// - [`CliError::Generic`]：目录读取失败或文件读取失败
fn resolve_seed_files(path: &Path, class_filter: Option<&str>) -> Result<Vec<SeedFile>, CliError> {
    let entries = std::fs::read_dir(path).map_err(|e| {
        CliError::Generic(format!(
            "Failed to read seed directory {}: {}",
            path.display(),
            e
        ))
    })?;

    let mut files: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|e| CliError::Generic(format!("Failed to read directory entry: {}", e)))?;
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("sql") {
            if let Some(filter) = class_filter {
                // 按文件名（不含扩展名）匹配
                if p.file_stem().and_then(|s| s.to_str()) != Some(filter) {
                    continue;
                }
            }
            files.push(p);
        }
    }

    // 按文件名升序排序
    files.sort();

    let mut seed_files = Vec::with_capacity(files.len());
    for f in files {
        let name = f
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let content = std::fs::read_to_string(&f).map_err(|e| {
            CliError::Generic(format!("Failed to read seed file {}: {}", f.display(), e))
        })?;
        seed_files.push(SeedFile { name, content });
    }

    Ok(seed_files)
}

/// 创建数据库连接（按 DbType 选择驱动）
///
/// 复用 `migrate` 模块的连接创建逻辑，保持一致性。
async fn create_connection(url: &str, db_type: DbType) -> Result<Box<dyn Connection>, CliError> {
    use std::sync::Arc;
    use sz_orm_sqlx::{
        MySqlPoolHandle, PgPoolHandle, SqlitePoolHandle, SqlxMySqlConnectionFactory,
        SqlxPgConnectionFactory, SqlxSqliteConnectionFactory,
    };

    match db_type {
        DbType::PostgreSQL => {
            let pool = PgPoolHandle::connect(url)
                .await
                .map_err(|e| CliError::Generic(format!("PostgreSQL connect failed: {}", e)))?;
            let factory = SqlxPgConnectionFactory::new(Arc::new(pool));
            let conn = factory
                .create()
                .await
                .map_err(|e| CliError::Generic(format!("PostgreSQL acquire failed: {}", e)))?;
            Ok(conn)
        }
        DbType::MySQL => {
            let pool = MySqlPoolHandle::connect(url)
                .await
                .map_err(|e| CliError::Generic(format!("MySQL connect failed: {}", e)))?;
            let factory = SqlxMySqlConnectionFactory::new(Arc::new(pool));
            let conn = factory
                .create()
                .await
                .map_err(|e| CliError::Generic(format!("MySQL acquire failed: {}", e)))?;
            Ok(conn)
        }
        DbType::Sqlite => {
            let pool = SqlitePoolHandle::connect(url)
                .await
                .map_err(|e| CliError::Generic(format!("SQLite connect failed: {}", e)))?;
            let factory = SqlxSqliteConnectionFactory::new(Arc::new(pool));
            let conn = factory
                .create()
                .await
                .map_err(|e| CliError::Generic(format!("SQLite acquire failed: {}", e)))?;
            Ok(conn)
        }
        _ => Err(CliError::Generic(format!(
            "Online seed not supported for db_type {:?}. Supported: PostgreSQL, MySQL, SQLite.",
            db_type
        ))),
    }
}

/// 打印 SQL 代码块（带标题分隔符）
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

    /// 创建测试用填充文件
    fn create_test_seed_file(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        fs::write(&path, content).expect("Failed to write test seed file");
    }

    #[test]
    fn test_resolve_seed_files_sorted_by_name() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        create_test_seed_file(tmp.path(), "003_third.sql", "INSERT INTO t VALUES (3);");
        create_test_seed_file(tmp.path(), "001_first.sql", "INSERT INTO t VALUES (1);");
        create_test_seed_file(tmp.path(), "002_second.sql", "INSERT INTO t VALUES (2);");

        let files = resolve_seed_files(tmp.path(), None).expect("resolve failed");
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].name, "001_first.sql");
        assert_eq!(files[1].name, "002_second.sql");
        assert_eq!(files[2].name, "003_third.sql");
    }

    #[test]
    fn test_resolve_seed_files_ignores_non_sql() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        create_test_seed_file(tmp.path(), "001_first.sql", "INSERT 1;");
        // 非 SQL 文件应被忽略
        let txt_path = tmp.path().join("readme.txt");
        fs::write(&txt_path, "ignore me").expect("write txt");
        let md_path = tmp.path().join("notes.md");
        fs::write(&md_path, "ignore me").expect("write md");

        let files = resolve_seed_files(tmp.path(), None).expect("resolve failed");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "001_first.sql");
    }

    #[test]
    fn test_resolve_seed_files_class_filter() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        create_test_seed_file(tmp.path(), "001_first.sql", "INSERT 1;");
        create_test_seed_file(tmp.path(), "002_second.sql", "INSERT 2;");

        let files = resolve_seed_files(tmp.path(), Some("002_second")).expect("resolve failed");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "002_second.sql");
    }

    #[test]
    fn test_resolve_seed_files_empty_directory() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        let files = resolve_seed_files(tmp.path(), None).expect("resolve failed");
        assert!(files.is_empty());
    }

    #[test]
    fn test_resolve_seed_files_nonexistent_directory() {
        let result = resolve_seed_files(Path::new("/nonexistent/path/to/seeds"), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_seed_args_default() {
        let args = SeedArgs::default();
        assert_eq!(args.path, "seeds");
        assert_eq!(args.db_type, "postgres");
        assert!(!args.show_sql);
        assert!(args.url.is_none());
        assert!(args.class.is_none());
    }

    #[test]
    fn test_execute_seed_offline_with_show_sql() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        create_test_seed_file(
            tmp.path(),
            "001_users.sql",
            "INSERT INTO users (name) VALUES ('admin');",
        );

        let args = SeedArgs {
            path: tmp.path().to_string_lossy().to_string(),
            show_sql: true,
            ..SeedArgs::default()
        };

        let result = execute_seed(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_seed_offline_without_show_sql() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        create_test_seed_file(
            tmp.path(),
            "001_users.sql",
            "INSERT INTO users (name) VALUES ('admin');",
        );

        let args = SeedArgs {
            path: tmp.path().to_string_lossy().to_string(),
            ..SeedArgs::default()
        };

        let result = execute_seed(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_seed_directory_not_found() {
        let args = SeedArgs {
            path: "/nonexistent/path/to/seeds".to_string(),
            ..SeedArgs::default()
        };
        let result = execute_seed(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Seed directory not found"));
    }

    #[test]
    fn test_execute_seed_empty_directory() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        let args = SeedArgs {
            path: tmp.path().to_string_lossy().to_string(),
            ..SeedArgs::default()
        };
        let result = execute_seed(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_seed_invalid_db_type() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        create_test_seed_file(tmp.path(), "001.sql", "INSERT 1;");
        let args = SeedArgs {
            path: tmp.path().to_string_lossy().to_string(),
            db_type: "invalid_db".to_string(),
            ..SeedArgs::default()
        };
        let result = execute_seed(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown database type"));
    }

    #[test]
    fn test_execute_seed_class_filter_offline() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        create_test_seed_file(tmp.path(), "001_first.sql", "INSERT 1;");
        create_test_seed_file(tmp.path(), "002_second.sql", "INSERT 2;");

        let args = SeedArgs {
            path: tmp.path().to_string_lossy().to_string(),
            class: Some("002_second".to_string()),
            ..SeedArgs::default()
        };

        let result = execute_seed(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_seed_class_filter_not_found() {
        let tmp = tempfile::tempdir().expect("Failed to create temp dir");
        create_test_seed_file(tmp.path(), "001_first.sql", "INSERT 1;");

        let args = SeedArgs {
            path: tmp.path().to_string_lossy().to_string(),
            class: Some("nonexistent".to_string()),
            ..SeedArgs::default()
        };

        let result = execute_seed(&args);
        assert!(result.is_ok()); // 空列表不算错误
    }

    #[test]
    fn test_print_sql_block_empty() {
        // 空内容不应输出任何东西
        print_sql_block("TITLE", "");
    }

    #[test]
    fn test_print_sql_block_non_empty() {
        print_sql_block("TITLE", "SELECT 1;\nSELECT 2;");
    }
}
