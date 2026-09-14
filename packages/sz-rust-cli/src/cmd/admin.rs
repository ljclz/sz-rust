// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

//! `admin` 命令组 — Admin 后台管理插件 CLI 工具
//!
//! 提供数据库迁移、路由/Capability 查看、初始化等命令，
//! 对接 `sz-rust-addons-admin` 插件。

use clap::{Args, Subcommand};
use sz_rust_core::orm::Connection;
use tabled::{Table, Tabled};

use crate::error::CliError;

/// Admin 后台管理插件命令组
#[derive(Subcommand, Debug)]
pub enum AdminCommand {
    /// 执行 admin 插件数据库迁移（创建 8 张表 + 3 个索引）
    Migrate(MigrateArgs),
    /// 列出 admin 插件 21 个 API 端点
    ListRoutes,
    /// 列出 admin 插件 17 个 Capability
    ListCapabilities,
    /// 初始化 admin 插件（创建内置角色、权限项、超级管理员账户）
    Init(InitArgs),
}

/// `admin migrate` 命令参数
#[derive(Args, Debug)]
pub struct MigrateArgs {
    /// 数据库类型（默认 postgres）
    #[arg(long, default_value = "postgres")]
    db_type: String,
    /// 数据库连接 URL（省略时为离线模式）
    #[arg(long)]
    url: Option<String>,
    /// 打印迁移 SQL 内容
    #[arg(long)]
    show_sql: bool,
}

/// `admin init` 命令参数
#[derive(Args, Debug)]
pub struct InitArgs {
    /// 数据库类型（默认 postgres）
    #[arg(long, default_value = "postgres")]
    db_type: String,
    /// 数据库连接 URL（省略时为离线模式）
    #[arg(long)]
    url: Option<String>,
    /// 超级管理员用户名（默认 admin）
    #[arg(long, default_value = "admin")]
    username: String,
    /// 超级管理员密码（默认 admin123）
    #[arg(long, default_value = "admin123")]
    password: String,
}

#[derive(Tabled)]
struct RouteRow {
    method: &'static str,
    path: &'static str,
    description: &'static str,
}

#[derive(Tabled)]
struct CapabilityRow {
    name: &'static str,
    description: &'static str,
    tags: &'static str,
}

const ROUTES: &[(&str, &str, &str)] = &[
    ("GET", "/api/admin/users", "用户列表"),
    ("POST", "/api/admin/users", "创建用户"),
    ("PUT", "/api/admin/users/{user_id}", "更新用户"),
    ("DELETE", "/api/admin/users/{user_id}", "删除用户"),
    ("PUT", "/api/admin/users/{user_id}/status", "更新用户状态"),
    ("PUT", "/api/admin/users/{user_id}/roles", "分配用户角色"),
    ("GET", "/api/admin/roles", "角色列表"),
    ("POST", "/api/admin/roles", "创建角色"),
    ("PUT", "/api/admin/roles/{role_id}", "更新角色"),
    ("DELETE", "/api/admin/roles/{role_id}", "删除角色"),
    (
        "PUT",
        "/api/admin/roles/{role_id}/permissions",
        "分配角色权限",
    ),
    ("GET", "/api/admin/permissions/tree", "权限树"),
    ("GET", "/api/admin/menus/tree", "菜单树"),
    ("POST", "/api/admin/menus", "创建菜单"),
    ("PUT", "/api/admin/menus/{menu_id}", "更新菜单"),
    ("DELETE", "/api/admin/menus/{menu_id}", "删除菜单"),
    ("GET", "/api/admin/configs", "配置列表"),
    ("PUT", "/api/admin/configs/{config_key}", "更新配置"),
    ("DELETE", "/api/admin/configs/{config_key}", "删除配置"),
    ("GET", "/api/admin/operation-logs", "操作日志列表"),
    ("GET", "/api/admin/dashboard", "仪表盘统计"),
];

const CAPABILITIES: &[(&str, &str, &str)] = &[
    ("admin.user_list", "用户列表查询", "user,query"),
    ("admin.user_create", "创建用户", "user,create"),
    ("admin.user_update", "更新用户", "user,update"),
    ("admin.user_delete", "删除用户", "user,delete"),
    ("admin.role_list", "角色列表查询", "role,query"),
    ("admin.role_create", "创建角色", "role,create"),
    ("admin.role_update", "更新角色", "role,update"),
    ("admin.role_delete", "删除角色", "role,delete"),
    ("admin.permission_tree", "权限树查询", "permission,query"),
    ("admin.menu_tree", "菜单树查询", "menu,query"),
    ("admin.menu_create", "创建菜单", "menu,create"),
    ("admin.menu_update", "更新菜单", "menu,update"),
    ("admin.menu_delete", "删除菜单", "menu,delete"),
    ("admin.config_list", "配置列表查询", "config,query"),
    ("admin.config_update", "更新配置", "config,update"),
    ("admin.log_list", "操作日志查询", "log,query"),
    ("admin.dashboard", "仪表盘统计", "dashboard,query"),
];

const MIGRATION_SQL: &str =
    include_str!("../../../sz-rust-addons-admin/src/migrations/001_init_admin.sql");

/// 执行 admin 命令
pub async fn execute(cmd: &AdminCommand) -> Result<i32, CliError> {
    match cmd {
        AdminCommand::Migrate(args) => execute_migrate(args).await,
        AdminCommand::ListRoutes => execute_list_routes(),
        AdminCommand::ListCapabilities => execute_list_capabilities(),
        AdminCommand::Init(args) => execute_init(args).await,
    }
}

async fn create_connection(url: &str, db_type: &str) -> Result<Box<dyn Connection>, CliError> {
    use sz_orm_sqlx::any_driver::AnyPool;

    let pool = AnyPool::connect(url)
        .await
        .map_err(|e| CliError::Migration(format!("{db_type} 连接失败: {e}")))?;
    let conn = pool
        .create()
        .await
        .map_err(|e| CliError::Migration(format!("获取连接失败: {e}")))?;
    Ok(Box::new(conn))
}

async fn execute_migrate(args: &MigrateArgs) -> Result<i32, CliError> {
    if args.show_sql || args.url.is_none() {
        println!("=== sz-rust-addons-admin 迁移 SQL ===\n");
        println!("{}", MIGRATION_SQL);
        println!("=== 共 8 张表 + 3 个索引 ===");
    }

    if let Some(url) = &args.url {
        println!("\n连接数据库 {} ...", args.db_type);
        let mut conn = create_connection(url, &args.db_type).await?;

        let sql_statements: Vec<&str> = MIGRATION_SQL
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("--"))
            .collect();
        let joined = sql_statements.join("\n");
        let statements: Vec<&str> = joined.split(';').filter(|s| !s.trim().is_empty()).collect();
        println!("待执行 {} 条 SQL 语句", statements.len());

        for (i, stmt) in statements.iter().enumerate() {
            conn.execute(stmt)
                .await
                .map_err(|e| CliError::Migration(format!("第 {} 条 SQL 执行失败: {e}", i + 1)))?;
            println!("  [{}] OK", i + 1);
        }
        println!("\n迁移完成：8 张表 + 3 个索引已创建");
    } else if !args.show_sql {
        println!("（离线模式，使用 --url <DATABASE_URL> 执行在线迁移）");
    }

    Ok(0)
}

fn execute_list_routes() -> Result<i32, CliError> {
    let rows: Vec<RouteRow> = ROUTES
        .iter()
        .map(|(method, path, desc)| RouteRow {
            method,
            path,
            description: desc,
        })
        .collect();
    let table = Table::new(rows);
    println!("=== Admin 插件 API 端点（共 {} 个）===\n", ROUTES.len());
    println!("{table}");
    Ok(0)
}

fn execute_list_capabilities() -> Result<i32, CliError> {
    let rows: Vec<CapabilityRow> = CAPABILITIES
        .iter()
        .map(|(name, desc, tags)| CapabilityRow {
            name,
            description: desc,
            tags,
        })
        .collect();
    let table = Table::new(rows);
    println!(
        "=== Admin 插件 Capability（共 {} 个）===\n",
        CAPABILITIES.len()
    );
    println!("{table}");
    Ok(0)
}

async fn execute_init(args: &InitArgs) -> Result<i32, CliError> {
    if args.url.is_none() {
        println!("=== Admin 插件初始化（离线模式）===\n");
        println!("将创建以下初始数据：");
        println!("  1. 内置角色: super_admin（超级管理员）、tenant_admin（租户管理员）");
        println!("  2. 内置权限项: admin:* 系列（{} 个）", CAPABILITIES.len());
        println!("  3. 超级管理员账户: {} (密码: ***)", args.username);
        println!("\n使用 --url <DATABASE_URL> 执行在线初始化");
        return Ok(0);
    }

    let url = args
        .url
        .as_deref()
        .ok_or_else(|| CliError::Clap("admin init 在线模式必须提供 --url".to_string()))?;
    println!("=== Admin 插件初始化 ===\n");
    println!("连接数据库...");
    let mut conn = create_connection(url, &args.db_type).await?;

    println!("创建内置角色...");
    for (code, name, desc) in [
        ("super_admin", "超级管理员", "拥有所有权限，不可删除"),
        ("tenant_admin", "租户管理员", "租户内管理权限，不可删除"),
    ] {
        let sql = format!(
            "INSERT INTO roles (name, code, description, is_builtin, tenant_id) \
             VALUES ('{name}', '{code}', '{desc}', TRUE, 0) \
             ON CONFLICT (code, tenant_id) DO NOTHING"
        );
        conn.execute(&sql)
            .await
            .map_err(|e| CliError::Migration(format!("创建角色 {code} 失败: {e}")))?;
        println!("  角色 {} ({}) 已创建", code, name);
    }

    println!("创建内置权限项...");
    for (cap_name, cap_desc, _tags) in CAPABILITIES {
        let module = cap_name.split('.').nth(1).unwrap_or("admin");
        let sql = format!(
            "INSERT INTO permissions (code, name, module, description, tenant_id) \
             VALUES ('{cap_name}', '{cap_desc}', '{module}', '{cap_desc}', 0) \
             ON CONFLICT (code, tenant_id) DO NOTHING"
        );
        conn.execute(&sql)
            .await
            .map_err(|e| CliError::Migration(format!("创建权限 {cap_name} 失败: {e}")))?;
    }
    println!("  {} 个权限项已创建", CAPABILITIES.len());

    println!("为 super_admin 分配所有权限...");
    let sql = "INSERT INTO role_permissions (role_id, permission_code, tenant_id) \
         SELECT r.id, p.code, 0 FROM roles r, permissions p \
         WHERE r.code = 'super_admin' AND r.tenant_id = 0 \
         AND p.tenant_id = 0 \
         ON CONFLICT (role_id, permission_code, tenant_id) DO NOTHING";
    conn.execute(sql)
        .await
        .map_err(|e| CliError::Migration(format!("分配权限失败: {e}")))?;
    println!("  super_admin 已分配全部权限");

    println!("创建超级管理员账户...");
    let hashed = bcrypt::hash(&args.password, 10)
        .map_err(|e| CliError::Migration(format!("密码哈希失败: {e}")))?;
    let sql = format!(
        "INSERT INTO users (username, password, status, tenant_id) \
         VALUES ('{}', '{}', 'active', 0) \
         ON CONFLICT (username, tenant_id) DO NOTHING",
        args.username, hashed
    );
    conn.execute(&sql)
        .await
        .map_err(|e| CliError::Migration(format!("创建管理员账户失败: {e}")))?;

    let sql = format!(
        "INSERT INTO user_roles (user_id, role_id, tenant_id) \
         SELECT u.id, r.id, 0 FROM users u, roles r \
         WHERE u.username = '{}' AND u.tenant_id = 0 \
         AND r.code = 'super_admin' AND r.tenant_id = 0 \
         ON CONFLICT (user_id, role_id, tenant_id) DO NOTHING",
        args.username
    );
    conn.execute(&sql)
        .await
        .map_err(|e| CliError::Migration(format!("分配管理员角色失败: {e}")))?;
    println!("  账户 {} 已创建并分配 super_admin 角色", args.username);

    println!("\n初始化完成！");
    println!("  超级管理员: {} (密码: {})", args.username, args.password);
    println!("  请及时修改默认密码！");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        admin: AdminCommand,
    }

    #[test]
    fn test_parse_migrate() {
        let cli = TestCli::parse_from(["sz-rust", "migrate", "--db-type", "postgres"]);
        assert!(matches!(cli.admin, AdminCommand::Migrate { .. }));
    }

    #[test]
    fn test_parse_migrate_with_url() {
        let cli = TestCli::parse_from([
            "sz-rust",
            "migrate",
            "--url",
            "postgres://localhost/db",
            "--show-sql",
        ]);
        match cli.admin {
            AdminCommand::Migrate(args) => {
                assert_eq!(args.url.as_deref(), Some("postgres://localhost/db"));
                assert!(args.show_sql);
            }
            _ => panic!("expected Migrate"),
        }
    }

    #[test]
    fn test_parse_list_routes() {
        let cli = TestCli::parse_from(["sz-rust", "list-routes"]);
        assert!(matches!(cli.admin, AdminCommand::ListRoutes));
    }

    #[test]
    fn test_parse_list_capabilities() {
        let cli = TestCli::parse_from(["sz-rust", "list-capabilities"]);
        assert!(matches!(cli.admin, AdminCommand::ListCapabilities));
    }

    #[test]
    fn test_parse_init() {
        let cli = TestCli::parse_from([
            "sz-rust",
            "init",
            "--username",
            "myadmin",
            "--password",
            "mypass",
        ]);
        match cli.admin {
            AdminCommand::Init(args) => {
                assert_eq!(args.username, "myadmin");
                assert_eq!(args.password, "mypass");
            }
            _ => panic!("expected Init"),
        }
    }

    #[test]
    fn test_routes_count_21() {
        assert_eq!(ROUTES.len(), 21);
    }

    #[test]
    fn test_capabilities_count_17() {
        assert_eq!(CAPABILITIES.len(), 17);
    }

    #[test]
    fn test_all_capabilities_prefixed_admin() {
        for (name, _, _) in CAPABILITIES {
            assert!(
                name.starts_with("admin."),
                "capability {name} must start with admin."
            );
        }
    }

    #[tokio::test]
    async fn test_execute_list_routes() {
        let result = execute(&AdminCommand::ListRoutes).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_execute_list_capabilities() {
        let result = execute(&AdminCommand::ListCapabilities).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_execute_migrate_offline() {
        let args = MigrateArgs {
            db_type: "postgres".to_string(),
            url: None,
            show_sql: true,
        };
        let result = execute(&AdminCommand::Migrate(args)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_init_offline() {
        let args = InitArgs {
            db_type: "postgres".to_string(),
            url: None,
            username: "admin".to_string(),
            password: "admin123".to_string(),
        };
        let result = execute(&AdminCommand::Init(args)).await;
        assert!(result.is_ok());
    }
}
