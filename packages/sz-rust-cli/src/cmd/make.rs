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

    /// 生成填充文件（对齐 PHP `make:seeder`）
    ///
    /// 在 `seeds/` 目录下生成 `<name>.sql` 文件骨架。
    #[command(name = "seeder")]
    Seeder {
        /// 填充文件名称（如 `001_users_seed`）
        name: String,
        /// 填充目录（默认 `seeds`）
        #[arg(short = 'p', long, default_value = "seeds")]
        path: String,
    },

    /// 生成 Guard（sz-rust 自研，无 PHP 对应）
    #[command(name = "guard")]
    Guard {
        /// Guard 名称（如 `Admin`）
        name: String,
    },

    /// 生成验证器（对齐 PHP `make:validate`）
    ///
    /// 在 `app/validate/` 目录下生成 `<Name>.rs` 验证器骨架。
    #[command(name = "validate")]
    Validate {
        /// 验证器类名（如 `User` 或 `admin/User`）
        name: String,
    },

    /// 生成事件类（对齐 PHP `make:event`）
    ///
    /// 在 `app/event/` 目录下生成 `<Name>.rs` 事件骨架，包含事件名与负载构造方法。
    #[command(name = "event")]
    Event {
        /// 事件类名（如 `UserLogin` 或 `admin/UserLogin`）
        name: String,
    },

    /// 生成监听器（对齐 PHP `make:listener`）
    ///
    /// 在 `app/listener/` 目录下生成 `<Name>.rs` 监听器骨架，实现 `Listener` trait。
    #[command(name = "listener")]
    Listener {
        /// 监听器类名（如 `SendWelcomeEmail`）
        name: String,
        /// 监听的事件名（可选，默认使用类名）
        #[arg(long)]
        event: Option<String>,
    },

    /// 生成自定义命令（对齐 PHP `make:command`）
    ///
    /// 在 `app/command/` 目录下生成 `<Name>.rs` 命令骨架，实现 `Command` trait。
    #[command(name = "command")]
    Command {
        /// 命令类名（如 `SyncData`）
        name: String,
    },

    /// 生成服务类（对齐 PHP `make:service`）
    ///
    /// 在 `app/service/` 目录下生成 `<Name>.rs` 服务骨架，可注入容器。
    #[command(name = "service")]
    Service {
        /// 服务类名（如 `UserService`）
        name: String,
    },

    /// 生成中间件（sz-rust 自研，对齐 NestJS `make:middleware`）
    ///
    /// 在 `app/middleware/` 目录下生成 `<Name>.rs` 中间件骨架，
    /// 实现 `SzMiddleware` trait，可注册到中间件链。
    #[command(name = "middleware")]
    Middleware {
        /// 中间件类名（如 `CorsMiddleware` 或 `admin/RateLimit`）
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
        MakeCommand::Seeder { name, path } => execute_make_seeder(name, path),
        MakeCommand::Guard { name } => execute_make_guard(name),
        MakeCommand::Validate { name } => execute_make_validate(name),
        MakeCommand::Event { name } => execute_make_event(name),
        MakeCommand::Listener { name, event } => execute_make_listener(name, event.as_deref()),
        MakeCommand::Command { name } => execute_make_command(name),
        MakeCommand::Service { name } => execute_make_service(name),
        MakeCommand::Middleware { name } => execute_make_middleware(name),
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

/// 生成填充文件
///
/// 对齐 PHP `make:seeder`：在 `seeds/` 目录下生成 `<name>.sql` 文件骨架。
fn execute_make_seeder(name: &str, path: &str) -> Result<(), CliError> {
    let dir = Path::new(path);
    std::fs::create_dir_all(dir)?;

    let file_path = dir.join(format!("{}.sql", name));
    check_file_exists(&file_path)?;

    let timestamp = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();
    let content = render_template(
        stubs::SEED_STUB,
        &[("{%name%}", name), ("{%timestamp%}", &timestamp)],
    );

    write_file(&file_path, &content)?;
    println!("Seeder created: {}", file_path.display());
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

/// 生成验证器文件
///
/// 对齐 PHP `make:validate`：在 `app/validate/` 目录下生成 `<Name>.rs` 验证器骨架，
/// 包含 `Validate` 结构体初始化与常见规则示例。
fn execute_make_validate(name: &str) -> Result<(), CliError> {
    let (class_name, module_path, file_path) = resolve_target(name, "validate");
    check_file_exists(&file_path)?;

    let namespace = format!("app::{}", module_path);
    let content = render_template(
        stubs::VALIDATE_STUB,
        &[
            ("{%className%}", &class_name),
            ("{%namespace%}", &namespace),
        ],
    );

    write_file(&file_path, &content)?;
    println!("Validator created: {}", file_path.display());
    Ok(())
}

/// 生成事件类文件
///
/// 对齐 PHP `make:event`：读取 `event.stub`，替换占位符，写入 `app/event/{name}.rs`。
fn execute_make_event(name: &str) -> Result<(), CliError> {
    let (class_name, module_path, file_path) = resolve_target(name, "event");
    check_file_exists(&file_path)?;

    let namespace = format!("app::{}", module_path);
    // 事件名默认使用类名（对齐 PHP `make:event` 默认行为）
    let event_name = class_name.clone();
    let content = render_template(
        stubs::EVENT_STUB,
        &[
            ("{%className%}", &class_name),
            ("{%namespace%}", &namespace),
            ("{%event_name%}", &event_name),
        ],
    );

    write_file(&file_path, &content)?;
    println!("Event created: {}", file_path.display());
    Ok(())
}

/// 生成监听器文件
///
/// 对齐 PHP `make:listener`：读取 `listener.stub`，替换占位符，写入 `app/listener/{name}.rs`。
/// 若未指定 `--event`，事件名默认使用监听器类名。
fn execute_make_listener(name: &str, event: Option<&str>) -> Result<(), CliError> {
    let (class_name, module_path, file_path) = resolve_target(name, "listener");
    check_file_exists(&file_path)?;

    let namespace = format!("app::{}", module_path);
    // 事件名：优先使用 --event 参数，否则默认使用类名
    let event_name = event.unwrap_or(&class_name).to_string();
    let content = render_template(
        stubs::LISTENER_STUB,
        &[
            ("{%className%}", &class_name),
            ("{%namespace%}", &namespace),
            ("{%event_name%}", &event_name),
        ],
    );

    write_file(&file_path, &content)?;
    println!("Listener created: {}", file_path.display());
    Ok(())
}

/// 生成自定义命令文件
///
/// 对齐 PHP `make:command`：读取 `command.stub`，替换占位符，写入 `app/command/{name}.rs`。
/// 命令名默认为类名的 snake_case 形式（如 `SyncData` → `sync_data`）。
fn execute_make_command(name: &str) -> Result<(), CliError> {
    let (class_name, module_path, file_path) = resolve_target(name, "command");
    check_file_exists(&file_path)?;

    let namespace = format!("app::{}", module_path);
    // 命令名：snake_case(类名)（对齐 PHP `make:command` 默认行为）
    let command_name = class_to_snake(&class_name);
    let content = render_template(
        stubs::COMMAND_STUB,
        &[
            ("{%className%}", &class_name),
            ("{%namespace%}", &namespace),
            ("{%command_name%}", &command_name),
        ],
    );

    write_file(&file_path, &content)?;
    println!("Command created: {}", file_path.display());
    Ok(())
}

/// 生成服务类文件
///
/// 对齐 PHP `make:service`：读取 `service.stub`，替换占位符，写入 `app/service/{name}.rs`。
fn execute_make_service(name: &str) -> Result<(), CliError> {
    let (class_name, module_path, file_path) = resolve_target(name, "service");
    check_file_exists(&file_path)?;

    let namespace = format!("app::{}", module_path);
    let content = render_template(
        stubs::SERVICE_STUB,
        &[
            ("{%className%}", &class_name),
            ("{%namespace%}", &namespace),
        ],
    );

    write_file(&file_path, &content)?;
    println!("Service created: {}", file_path.display());
    Ok(())
}

/// 生成中间件文件（sz-rust 自研）
///
/// 在 `app/middleware/` 目录下生成 `<Name>.rs` 中间件骨架，
/// 实现 `SzMiddleware` trait，可注册到中间件链。
fn execute_make_middleware(name: &str) -> Result<(), CliError> {
    let (class_name, module_path, file_path) = resolve_target(name, "middleware");
    check_file_exists(&file_path)?;

    let namespace = format!("app::{}", module_path);
    let content = render_template(
        stubs::MIDDLEWARE_STUB,
        &[
            ("{%className%}", &class_name),
            ("{%namespace%}", &namespace),
        ],
    );

    write_file(&file_path, &content)?;
    println!("Middleware created: {}", file_path.display());
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

    /// RAII 守卫：在作用域结束时恢复原始工作目录。
    ///
    /// 即使测试 panic 也能保证恢复，避免污染后续测试。
    /// 配合 `super::super::test_support::acquire_global_lock()` 使用，
    /// 确保与 optimize 模块测试的 set_current_dir 互斥。
    struct CwdGuard {
        original: Option<PathBuf>,
    }

    impl CwdGuard {
        /// 切换到 `new_dir` 并返回守卫。守卫 drop 时恢复原目录。
        fn switch(new_dir: &Path) -> std::io::Result<Self> {
            let original = std::env::current_dir().ok();
            std::env::set_current_dir(new_dir)?;
            Ok(Self { original })
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            if let Some(ref orig) = self.original {
                let _ = std::env::set_current_dir(orig);
            }
        }
    }

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
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_model("TestUser").unwrap();

        let model_path = temp_dir.path().join("app/model/TestUser.rs");
        assert!(model_path.exists());

        let content = std::fs::read_to_string(&model_path).unwrap();
        assert!(content.contains("TestUser"));
        assert!(content.contains("test_user"));
    }

    #[test]
    fn test_execute_make_seeder_creates_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        execute_make_seeder("001_test_seed", path).unwrap();

        let seed_path = Path::new(path).join("001_test_seed.sql");
        assert!(seed_path.exists());

        let content = std::fs::read_to_string(&seed_path).unwrap();
        assert!(content.contains("001_test_seed"));
        assert!(content.contains("-- Seed:"));
        // 渲染后不应残留占位符
        assert!(!content.contains("{%"));
    }

    #[test]
    fn test_execute_make_seeder_file_already_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        // 第一次创建应成功
        execute_make_seeder("001_dup_seed", path).unwrap();
        // 第二次创建同名文件应失败
        let result = execute_make_seeder("001_dup_seed", path);
        assert!(matches!(result, Err(CliError::FileExists(_))));
    }

    #[test]
    fn test_execute_make_seeder_creates_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let nested = temp_dir.path().join("nested").join("seeds");
        let path = nested.to_str().unwrap();

        // 目录不存在时应自动创建
        execute_make_seeder("001_seed", path).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn test_make_validate_creates_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        // 通过顶层 execute 入口调用 make validate（验证命令分发正常）
        let cmd = MakeCommand::Validate {
            name: "Order".to_string(),
        };
        execute(&cmd).unwrap();

        let validate_path = temp_dir.path().join("app/validate/Order.rs");
        assert!(validate_path.exists());

        let content = std::fs::read_to_string(&validate_path).unwrap();
        // 验证器 struct 与 Validate 引用
        assert!(content.contains("pub struct OrderValidate;"));
        assert!(content.contains("use sz_rust_core::validate::Validate"));
        assert!(content.contains("app::validate"));
        // 渲染后不应残留占位符
        assert!(!content.contains("{%"));
    }

    #[test]
    fn test_validate_stub_contains_required_elements() {
        // 验证 stub 模板包含必要元素：struct 定义、Validate 引用、占位符
        assert!(stubs::VALIDATE_STUB.contains("pub struct {%className%}Validate;"));
        assert!(stubs::VALIDATE_STUB.contains("use sz_rust_core::validate::Validate"));
        assert!(stubs::VALIDATE_STUB.contains("impl {%className%}Validate"));
        assert!(stubs::VALIDATE_STUB.contains("pub fn new() -> Validate"));
        assert!(stubs::VALIDATE_STUB.contains("{%className%}"));
        assert!(stubs::VALIDATE_STUB.contains("{%namespace%}"));
    }

    #[test]
    fn test_execute_make_validate_creates_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_validate("User").unwrap();

        let validate_path = temp_dir.path().join("app/validate/User.rs");
        assert!(validate_path.exists());

        let content = std::fs::read_to_string(&validate_path).unwrap();
        assert!(content.contains("UserValidate"));
        assert!(content.contains("use sz_rust_core::validate::Validate"));
        assert!(content.contains("app::validate"));
        // 渲染后不应残留占位符
        assert!(!content.contains("{%"));
    }

    #[test]
    fn test_execute_make_validate_file_already_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        // 第一次创建应成功
        execute_make_validate("User").unwrap();
        // 第二次创建同名文件应失败
        let result = execute_make_validate("User");
        assert!(matches!(result, Err(CliError::FileExists(_))));
    }

    #[test]
    fn test_execute_make_validate_nested_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        // 嵌套路径：admin/User → app/validate/admin/User.rs
        execute_make_validate("admin/User").unwrap();

        let validate_path = temp_dir.path().join("app/validate/admin/User.rs");
        assert!(validate_path.exists());

        let content = std::fs::read_to_string(&validate_path).unwrap();
        assert!(content.contains("UserValidate"));
        assert!(content.contains("app::validate::admin"));
    }

    // ---------- make event/listener/command/service 测试 ----------

    #[test]
    fn test_execute_make_event_creates_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_event("UserLogin").unwrap();

        let event_path = temp_dir.path().join("app/event/UserLogin.rs");
        assert!(event_path.exists());

        let content = std::fs::read_to_string(&event_path).unwrap();
        assert!(content.contains("pub struct UserLogin;"));
        assert!(content.contains("app::event"));
        assert!(content.contains("UserLogin"));
        assert!(content.contains("use serde_json::Value"));
        // 渲染后不应残留占位符
        assert!(!content.contains("{%"));
    }

    #[test]
    fn test_execute_make_event_file_already_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_event("UserLogin").unwrap();
        let result = execute_make_event("UserLogin");
        assert!(matches!(result, Err(CliError::FileExists(_))));
    }

    #[test]
    fn test_execute_make_event_nested_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_event("admin/UserLogin").unwrap();

        let event_path = temp_dir.path().join("app/event/admin/UserLogin.rs");
        assert!(event_path.exists());

        let content = std::fs::read_to_string(&event_path).unwrap();
        assert!(content.contains("app::event::admin"));
    }

    #[test]
    fn test_execute_make_listener_default_event_name() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        // 未指定 --event，事件名默认使用类名
        execute_make_listener("SendWelcomeEmail", None).unwrap();

        let listener_path = temp_dir.path().join("app/listener/SendWelcomeEmail.rs");
        assert!(listener_path.exists());

        let content = std::fs::read_to_string(&listener_path).unwrap();
        assert!(content.contains("pub struct SendWelcomeEmail;"));
        assert!(content.contains("app::listener"));
        assert!(content.contains("use sz_rust_core::event::{EventError, Listener}"));
        assert!(content.contains("impl Listener for SendWelcomeEmail"));
        // 默认事件名 = 类名
        assert!(content.contains(r#""SendWelcomeEmail""#));
        assert!(!content.contains("{%"));
    }

    #[test]
    fn test_execute_make_listener_custom_event_name() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        // 指定 --event UserLogin
        execute_make_listener("SendWelcomeEmail", Some("UserLogin")).unwrap();

        let listener_path = temp_dir.path().join("app/listener/SendWelcomeEmail.rs");
        assert!(listener_path.exists());

        let content = std::fs::read_to_string(&listener_path).unwrap();
        assert!(content.contains(r#""UserLogin""#));
        assert!(!content.contains(r#""SendWelcomeEmail""#));
        assert!(!content.contains("{%"));
    }

    #[test]
    fn test_execute_make_listener_file_already_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_listener("SendWelcomeEmail", None).unwrap();
        let result = execute_make_listener("SendWelcomeEmail", None);
        assert!(matches!(result, Err(CliError::FileExists(_))));
    }

    #[test]
    fn test_execute_make_command_creates_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_command("SyncData").unwrap();

        let command_path = temp_dir.path().join("app/command/SyncData.rs");
        assert!(command_path.exists());

        let content = std::fs::read_to_string(&command_path).unwrap();
        assert!(content.contains("pub struct SyncData;"));
        assert!(content.contains("app::command"));
        assert!(content.contains("use sz_rust_cli::console::{Command, CommandSignature}"));
        assert!(content.contains("impl Command for SyncData"));
        // 命令名应为 snake_case：sync_data
        assert!(content.contains(r#""sync_data""#));
        assert!(!content.contains("{%"));
    }

    #[test]
    fn test_execute_make_command_file_already_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_command("SyncData").unwrap();
        let result = execute_make_command("SyncData");
        assert!(matches!(result, Err(CliError::FileExists(_))));
    }

    #[test]
    fn test_execute_make_command_nested_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_command("admin/SyncData").unwrap();

        let command_path = temp_dir.path().join("app/command/admin/SyncData.rs");
        assert!(command_path.exists());

        let content = std::fs::read_to_string(&command_path).unwrap();
        assert!(content.contains("app::command::admin"));
    }

    #[test]
    fn test_execute_make_service_creates_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_service("UserService").unwrap();

        let service_path = temp_dir.path().join("app/service/UserService.rs");
        assert!(service_path.exists());

        let content = std::fs::read_to_string(&service_path).unwrap();
        assert!(content.contains("pub struct UserService;"));
        assert!(content.contains("app::service"));
        assert!(content.contains("impl Default for UserService"));
        assert!(content.contains("pub fn new() -> Self"));
        assert!(!content.contains("{%"));
    }

    #[test]
    fn test_execute_make_service_file_already_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_service("UserService").unwrap();
        let result = execute_make_service("UserService");
        assert!(matches!(result, Err(CliError::FileExists(_))));
    }

    #[test]
    fn test_execute_make_service_nested_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _lock = super::super::test_support::acquire_global_lock();
        let _guard = CwdGuard::switch(temp_dir.path()).unwrap();

        execute_make_service("admin/UserService").unwrap();

        let service_path = temp_dir.path().join("app/service/admin/UserService.rs");
        assert!(service_path.exists());

        let content = std::fs::read_to_string(&service_path).unwrap();
        assert!(content.contains("app::service::admin"));
    }
}
