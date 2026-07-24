//! `make:*` 代码生成命令 — 对齐 PHP `think\console\command\make\*`
//!
//! ## PHP 对齐
//!
//! PHP `Make::execute()` 流程：
//! 1. `getArgument('name')` 获取类名
//! 2. `getClassName(name)` 处理 `@` 分隔应用名 + `/` 转 `\`
//! 3. `getPathName(className)` 剥离 `app\` 前缀 + `/` 替换 + `.php` 后缀
//! 4. 检查文件存在 → `mkdir` → `file_put_contents(buildClass())`
//! 5. `buildClass(name)` 读取 stub，替换占位符
//!
//! Rust 端对齐上述流程，但生成 `.rs` 文件而非 `.php` 文件。

use std::path::{Path, PathBuf};

use clap::Subcommand;

use crate::error::CliError;
use crate::stubs::{self, render_template};

/// `make` 子命令枚举
///
/// 对齐 PHP `think make:*` 命令组。
#[derive(Subcommand, Debug)]
pub enum MakeCommand {
    /// 生成 Model（对齐 `php think make:model User`）
    #[command(name = "model")]
    Model {
        /// 类名（如 `User` 或 `admin/User`）
        name: String,
    },

    /// 生成 Controller（对齐 `php think make:controller User`）
    #[command(name = "controller")]
    Controller {
        /// 类名
        name: String,
        /// 生成 API 风格控制器（5 方法，无 create/edit）
        #[arg(long)]
        api: bool,
        /// 生成空控制器
        #[arg(long)]
        plain: bool,
    },

    /// 生成迁移文件（对齐 Phinx `make:migration`）
    #[command(name = "migration")]
    Migration {
        /// 迁移名称（如 `create_users`）
        name: String,
        /// 迁移目录（默认 `migrations`）
        #[arg(short = 'p', long, default_value = "migrations")]
        path: String,
    },

    /// 生成 Guard（sz-rust 自研，无 PHP 对应）
    #[command(name = "guard")]
    Guard {
        /// Guard 名称（如 `Admin`）
        name: String,
    },

    /// 生成脚手架（Model + Controller + Migration）
    #[command(name = "scaffold")]
    Scaffold {
        /// 资源名称（如 `User`）
        name: String,
    },
}

/// 执行 make 子命令
pub fn execute(cmd: &MakeCommand) -> Result<(), CliError> {
    match cmd {
        MakeCommand::Model { name } => execute_make_model(name),
        MakeCommand::Controller { name, api, plain } => execute_make_controller(name, *api, *plain),
        MakeCommand::Migration { name, path } => execute_make_migration(name, path),
        MakeCommand::Guard { name } => execute_make_guard(name),
        MakeCommand::Scaffold { name } => execute_make_scaffold(name),
    }
}

/// 生成 Model 文件
///
/// 对齐 PHP `make:model`：读取 `model.stub`，替换占位符，写入 `app/model/{name}.rs`。
fn execute_make_model(name: &str) -> Result<(), CliError> {
    let (class_name, module_path, file_path) = resolve_target(name, "model");
    check_file_exists(&file_path)?;

    let namespace = format!("app::{}", module_path);
    let table_name = class_to_snake(&class_name);
    let content = render_template(
        stubs::MODEL_STUB,
        &[
            ("{%className%}", &class_name),
            ("{%namespace%}", &namespace),
            ("{%table_name%}", &table_name),
        ],
    );

    write_file(&file_path, &content)?;
    println!("Model created: {}", file_path.display());
    Ok(())
}

/// 生成 Controller 文件
///
/// 对齐 PHP `make:controller`：根据 `--api` / `--plain` 选择不同 stub。
fn execute_make_controller(name: &str, api: bool, plain: bool) -> Result<(), CliError> {
    let (class_name, module_path, file_path) = resolve_target(name, "controller");
    check_file_exists(&file_path)?;

    let namespace = format!("app::{}", module_path);
    let route = class_to_snake(&class_name);

    let template = if plain {
        stubs::CONTROLLER_PLAIN_STUB
    } else if api {
        stubs::CONTROLLER_API_STUB
    } else {
        stubs::CONTROLLER_STUB
    };

    let content = render_template(
        template,
        &[
            ("{%className%}", &class_name),
            ("{%namespace%}", &namespace),
            ("{%route%}", &route),
        ],
    );

    write_file(&file_path, &content)?;
    println!("Controller created: {}", file_path.display());
    Ok(())
}

/// 生成迁移文件
///
/// 对齐 Phinx 风格：生成 `{timestamp}_{name}_up.sql` 和 `{timestamp}_{name}_down.sql`。
fn execute_make_migration(name: &str, path: &str) -> Result<(), CliError> {
    let dir = Path::new(path);
    std::fs::create_dir_all(dir)?;

    let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let table_name = name_to_table(name);

    let up_file = dir.join(format!("{}_{}_up.sql", timestamp, name));
    let down_file = dir.join(format!("{}_{}_down.sql", timestamp, name));

    check_file_exists(&up_file)?;
    check_file_exists(&down_file)?;

    let up_content = render_template(
        stubs::MIGRATION_UP_STUB,
        &[
            ("{%name%}", name),
            ("{%timestamp%}", &timestamp),
            ("{%table_name%}", &table_name),
        ],
    );
    let down_content = render_template(
        stubs::MIGRATION_DOWN_STUB,
        &[
            ("{%name%}", name),
            ("{%timestamp%}", &timestamp),
            ("{%table_name%}", &table_name),
        ],
    );

    write_file(&up_file, &up_content)?;
    write_file(&down_file, &down_content)?;
    println!(
        "Migration created: {} & {}",
        up_file.display(),
        down_file.display()
    );
    Ok(())
}

/// 生成 Guard 文件（sz-rust 自研）
fn execute_make_guard(name: &str) -> Result<(), CliError> {
    let (class_name, _module_path, file_path) = resolve_target(name, "guard");
    check_file_exists(&file_path)?;

    let content = format!(
        "//! Guard: {class_name}\n//!\n//! 由 `sz-rust make:guard` 生成。\n//!\n//! 对齐 NestJS Guard + Spring Security 模式。\n\nuse sz_rust_core::guard::Guard;\nuse sz_rust_core::request::Request;\n\n/// {class_name} Guard\npub struct {class_name};\n\nimpl Guard for {class_name} {{\n    async fn can_activate(&self, _req: &Request) -> bool {{\n        // 在此实现鉴权逻辑\n        true\n    }}\n}}\n"
    );

    write_file(&file_path, &content)?;
    println!("Guard created: {}", file_path.display());
    Ok(())
}

/// 生成脚手架（Model + Controller + Migration）
fn execute_make_scaffold(name: &str) -> Result<(), CliError> {
    println!("Scaffolding for: {}", name);
    execute_make_model(name)?;
    execute_make_controller(name, false, false)?;
    execute_make_migration(&class_to_snake(name), "migrations")?;
    println!("Scaffold complete.");
    Ok(())
}

// ============================================================================
// 辅助函数（对齐 PHP Make 基类方法）
// ============================================================================

/// 解析目标（类名 + 模块路径 + 文件路径）
///
/// 对齐 PHP `Make::getClassName()` + `getPathName()`：
///
/// - `User` → (`User`, `model`, `app/model/User.rs`)
/// - `admin/User` → (`User`, `controller::admin`, `app/controller/admin/User.rs`)
/// - `admin@User` → (`User`, `admin::model`, `app/admin/model/User.rs`)
fn resolve_target(name: &str, layer: &str) -> (String, String, PathBuf) {
    // 处理 @ 分隔应用名（对齐 PHP getClassName）
    let (app, class_part) = if let Some(idx) = name.find('@') {
        (&name[..idx], &name[idx + 1..])
    } else {
        ("", name)
    };

    // 处理 / 分隔子目录（对齐 PHP / → \）
    let segments: Vec<&str> = class_part.split('/').collect();
    let class_name = segments.last().unwrap_or(&"").to_string();

    // 构建模块路径
    let parent_segments: Vec<&str> = if segments.len() > 1 {
        segments[..segments.len() - 1].to_vec()
    } else {
        Vec::new()
    };

    let module_path = if app.is_empty() {
        if parent_segments.is_empty() {
            layer.to_string()
        } else {
            format!("{}::{}", layer, parent_segments.join("::"))
        }
    } else if parent_segments.is_empty() {
        format!("{}::{}", app, layer)
    } else {
        format!("{}::{}::{}", app, layer, parent_segments.join("::"))
    };

    // 构建文件路径（对齐 PHP getPathName：app\ → app/ + / 替换）
    let mut path = PathBuf::from("app");
    if !app.is_empty() {
        path.push(app);
    }
    // 添加层目录
    path.push(layer);
    // 添加子目录
    for seg in &parent_segments {
        path.push(seg);
    }
    // 添加文件名
    path.push(format!("{}.rs", class_name));

    (class_name, module_path, path)
}

/// 检查文件是否已存在
///
/// 对齐 PHP `Make::execute()` 中 `already exists!` 提示。
fn check_file_exists(path: &Path) -> Result<(), CliError> {
    if path.exists() {
        return Err(CliError::FileExists(path.display().to_string()));
    }
    Ok(())
}

/// 写入文件（自动创建父目录）
///
/// 对齐 PHP `mkdir` + `file_put_contents`。
fn write_file(path: &Path, content: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// 类名转 snake_case 表名
///
/// `User` → `user`，`OrderItem` → `order_item`
fn class_to_snake(s: &str) -> String {
    let mut result = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(ch.to_lowercase().next().unwrap_or(ch));
    }
    result
}

/// 迁移名转表名
///
/// `create_users` → `users`，`add_index_to_orders` → `orders`
fn name_to_table(name: &str) -> String {
    // 简单提取：create_xxx → xxx
    if let Some(rest) = name.strip_prefix("create_") {
        return rest.to_string();
    }
    if let Some(rest) = name.strip_prefix("add_") {
        // add_index_to_orders → orders
        if let Some(to_pos) = rest.find("_to_") {
            return rest[to_pos + 4..].to_string();
        }
        return rest.to_string();
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_target_simple_model() {
        let (class, module, path) = resolve_target("User", "model");
        assert_eq!(class, "User");
        assert_eq!(module, "model");
        assert_eq!(path, PathBuf::from("app/model/User.rs"));
    }

    #[test]
    fn test_resolve_target_nested_controller() {
        let (class, module, path) = resolve_target("admin/User", "controller");
        assert_eq!(class, "User");
        assert_eq!(module, "controller::admin");
        assert_eq!(path, PathBuf::from("app/controller/admin/User.rs"));
    }

    #[test]
    fn test_resolve_target_with_app() {
        let (class, module, _path) = resolve_target("admin@User", "model");
        assert_eq!(class, "User");
        assert_eq!(module, "admin::model");
    }

    #[test]
    fn test_class_to_snake() {
        assert_eq!(class_to_snake("User"), "user");
        assert_eq!(class_to_snake("OrderItem"), "order_item");
        assert_eq!(class_to_snake("API"), "a_p_i");
    }

    #[test]
    fn test_name_to_table_create() {
        assert_eq!(name_to_table("create_users"), "users");
        assert_eq!(name_to_table("create_orders"), "orders");
    }

    #[test]
    fn test_name_to_table_add() {
        assert_eq!(name_to_table("add_index_to_orders"), "orders");
        assert_eq!(name_to_table("add_status"), "status");
    }

    #[test]
    fn test_name_to_table_other() {
        assert_eq!(name_to_table("custom_migration"), "custom_migration");
    }

    #[test]
    fn test_check_file_exists_nonexistent() {
        let result = check_file_exists(Path::new("/nonexistent/path/file.txt"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_file_exists_existing() {
        // 使用临时文件确保文件确实存在
        let temp = tempfile::NamedTempFile::new().unwrap();
        let result = check_file_exists(temp.path());
        assert!(matches!(result, Err(CliError::FileExists(_))));
    }

    #[test]
    fn test_write_and_read_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test_file.txt");

        write_file(&file_path, "test content").unwrap();
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "test content");
    }

    #[test]
    fn test_execute_make_migration_creates_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        execute_make_migration("create_test_table", path).unwrap();

        let entries: Vec<_> = std::fs::read_dir(path).unwrap().collect();
        assert_eq!(entries.len(), 2); // up + down

        let mut has_up = false;
        let mut has_down = false;
        for entry in entries {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            if name.ends_with("_up.sql") {
                has_up = true;
            }
            if name.ends_with("_down.sql") {
                has_down = true;
            }
        }
        assert!(has_up);
        assert!(has_down);
    }

    #[test]
    fn test_execute_make_model_in_temp() {
        // 模拟生成（在临时目录验证逻辑）
        let temp_dir = tempfile::tempdir().unwrap();
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        execute_make_model("TestUser").unwrap();

        let model_path = Path::new("app/model/TestUser.rs");
        assert!(model_path.exists());

        let content = std::fs::read_to_string(model_path).unwrap();
        assert!(content.contains("TestUser"));
        assert!(content.contains("test_user"));

        std::env::set_current_dir(old).unwrap();
    }
}
