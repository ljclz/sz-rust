// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Admin services 层集成测试（覆盖率基线缺口补测，见
//! docs/audit/2026-09-14-覆盖率基线与行动项执行报告.md 第二节）
//!
//! 通过 SQL 内容路由的 MockConnection 驱动 UserService/RoleService 的
//! 验证分支、查重分支、成功路径与事务回滚分支，不依赖真实数据库。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sz_rust_addons_admin::error::AdminError;
use sz_rust_addons_admin::models::config::ConfigType;
use sz_rust_addons_admin::models::operation_log::OperationType;
use sz_rust_addons_admin::models::user::UserStatus;
use sz_rust_addons_admin::services::config_service::{ConfigService, UpsertConfigRequest};
use sz_rust_addons_admin::services::dashboard_service::DashboardService;
use sz_rust_addons_admin::services::log_service::{LogFilter, OperationLogService};
use sz_rust_addons_admin::services::menu_service::{
    CreateMenuRequest, MenuService, UpdateMenuRequest,
};
use sz_rust_addons_admin::services::permission_service::PermissionService;
use sz_rust_addons_admin::services::role_service::{
    CreateRoleRequest, RoleService, UpdateRoleRequest,
};
use sz_rust_addons_admin::services::user_service::{
    CreateUserRequest, UpdateUserRequest, UserListFilter, UserService,
};
use sz_rust_orm_facade::{Connection, ConnectionFactory, DbError, Pool, PoolConfig, Value};

type QueryRows = Vec<HashMap<String, Value>>;

/// 可按 SQL 内容路由的 mock 行为（跨连接共享：service 方法内会多次 acquire）
#[derive(Default, Clone)]
struct MockBehavior {
    /// create/update 的重名检查命中（SELECT id FROM users WHERE username）
    duplicate_username: bool,
    /// update/delete 前置的 id 存在性检查命中（SELECT id FROM users WHERE id）
    user_exists: bool,
    /// get_by_id / create 后查询返回完整用户行（SELECT id, username）
    return_user_row: bool,
    /// COUNT(*) 返回值
    total_cnt: u64,
    /// DELETE affected 行数（0 → 触发 UserNotFound 回滚分支）
    delete_affected: u64,

    // —— 扩展字段（menu/config/log/role/permission）——
    /// UPDATE affected 行数（update_status 等）
    update_affected: u64,
    /// 通用重复检查命中（SELECT id FROM menus/roles WHERE code、configs WHERE `key`）
    duplicate: bool,
    /// 通用存在性命中（SELECT id FROM X WHERE id、assign_permissions 的 role 存在性）
    entity_exists: bool,
    /// 权限存在性命中（SELECT code FROM permissions WHERE code）
    permission_exists: bool,
    /// super_admin 角色命中（SELECT r.code FROM user_roles JOIN roles）
    is_super_admin: bool,
    /// 角色 is_builtin 字段值（SELECT id, is_builtin FROM roles WHERE id）
    role_is_builtin: bool,
    /// 子菜单存在命中（SELECT id FROM menus WHERE parent_id）
    menu_has_child: bool,
    /// menu list_all / get_by_id 返回行集
    menu_rows: Vec<HashMap<String, Value>>,
    /// config list / get_by_key 返回行集
    config_rows: Vec<HashMap<String, Value>>,
    /// log list 返回行集
    log_rows: Vec<HashMap<String, Value>>,
    /// role list / get_by_id 返回行集
    role_rows: Vec<HashMap<String, Value>>,
    /// permission list_all / tree 返回行集
    permission_rows: Vec<HashMap<String, Value>>,
}

fn user_row() -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::I64(1));
    row.insert("username".into(), Value::String("alice".into()));
    row.insert("status".into(), Value::String("active".into()));
    row.insert("tenant_id".into(), Value::I64(0));
    row.insert(
        "created_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row.insert(
        "updated_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row
}

struct MockConnection {
    behavior: Arc<Mutex<MockBehavior>>,
}

fn cnt_row(n: u64) -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("cnt".into(), Value::I64(n as i64));
    row
}

impl MockConnection {
    fn query(sql: &str, behavior: &MockBehavior) -> QueryRows {
        let sql = sql.to_ascii_lowercase();

        // 1. COUNT(*) → 计数
        if sql.contains("count(*)") {
            return vec![cnt_row(behavior.total_cnt)];
        }

        // 2. 现有：SELECT id, username ... FROM users → return_user_row
        if sql.contains("select id, username") {
            return if behavior.return_user_row {
                vec![user_row()]
            } else {
                vec![]
            };
        }

        // 3. 现有：SELECT id FROM users WHERE username → duplicate_username
        if sql.contains("select id from users where username") {
            return if behavior.duplicate_username {
                vec![HashMap::new()]
            } else {
                vec![]
            };
        }

        // 4. 现有：SELECT id FROM users WHERE id → user_exists
        if sql.contains("select id from users where id") {
            return if behavior.user_exists {
                vec![HashMap::new()]
            } else {
                vec![]
            };
        }

        // 5. SELECT id, is_builtin FROM roles WHERE id → 角色存在性 + builtin
        if sql.contains("select id, is_builtin from roles") {
            if !behavior.entity_exists {
                return vec![];
            }
            let mut row = HashMap::new();
            row.insert("is_builtin".into(), Value::Bool(behavior.role_is_builtin));
            return vec![row];
        }

        // 6. SELECT id FROM X WHERE ... → 存在性/重复/子菜单检查
        if sql.contains("select id from") {
            // 重复检查（WHERE code/key）
            if (sql.contains("where code")
                && (sql.contains("from menus") || sql.contains("from roles")))
                || sql.contains("where `key`")
            {
                return if behavior.duplicate {
                    vec![HashMap::new()]
                } else {
                    vec![]
                };
            }
            // 子菜单检查
            if sql.contains("where parent_id") {
                return if behavior.menu_has_child {
                    vec![HashMap::new()]
                } else {
                    vec![]
                };
            }
            // 通用存在性（WHERE id）
            return if behavior.entity_exists {
                vec![HashMap::new()]
            } else {
                vec![]
            };
        }

        // 7. SELECT code FROM permissions WHERE code = ? → 权限存在性
        if sql.contains("select code from permissions where code") {
            return if behavior.permission_exists {
                vec![HashMap::new()]
            } else {
                vec![]
            };
        }

        // 8. super_admin 检查（SELECT r.code FROM user_roles JOIN roles）
        if sql.contains("from user_roles") && sql.contains("join roles") {
            return if behavior.is_super_admin {
                vec![HashMap::new()]
            } else {
                vec![]
            };
        }

        // 9. guard 权限检查（SELECT rp.permission_code FROM user_roles JOIN role_permissions）
        if sql.contains("from user_roles") && sql.contains("join role_permissions") {
            return if behavior.entity_exists {
                vec![HashMap::new()]
            } else {
                vec![]
            };
        }

        // 10. 实体行返回：根据表名选择行集
        if sql.contains("from menus") {
            return behavior.menu_rows.clone();
        }
        if sql.contains("from configs") {
            return behavior.config_rows.clone();
        }
        if sql.contains("from operation_logs") {
            return behavior.log_rows.clone();
        }
        if sql.contains("from roles") {
            return behavior.role_rows.clone();
        }
        if sql.contains("from permissions") {
            return behavior.permission_rows.clone();
        }
        if sql.contains("from users") {
            return if behavior.return_user_row {
                vec![user_row()]
            } else {
                vec![]
            };
        }

        vec![]
    }
}

impl Connection for MockConnection {
    fn execute<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(1) })
    }
    fn query<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
        let rows = Self::query(_sql, &self.behavior.lock().unwrap().clone());
        Box::pin(async { Ok(rows) })
    }
    fn execute_with_params<'a>(
        &'a mut self,
        sql: &'a str,
        _params: &'a [Value],
    ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
        let lower = sql.to_ascii_lowercase();
        let b = self.behavior.lock().unwrap().clone();
        let affected = if lower.starts_with("delete from") {
            b.delete_affected
        } else if lower.starts_with("update users set status") {
            b.update_affected
        } else {
            1
        };
        Box::pin(async move { Ok(affected) })
    }
    fn query_with_params<'a>(
        &'a mut self,
        sql: &'a str,
        _params: &'a [Value],
    ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
        let rows = Self::query(sql, &self.behavior.lock().unwrap().clone());
        Box::pin(async { Ok(rows) })
    }
    fn begin_transaction<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn commit<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn rollback<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn is_connected(&self) -> bool {
        true
    }
    fn ping<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async { true })
    }
    fn close<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

struct MockConnectionFactory {
    behavior: Arc<Mutex<MockBehavior>>,
}

#[async_trait]
impl ConnectionFactory for MockConnectionFactory {
    async fn create(&self) -> Result<Box<dyn Connection>, DbError> {
        Ok(Box::new(MockConnection {
            behavior: self.behavior.clone(),
        }))
    }
}

fn make_service(behavior: MockBehavior) -> UserService {
    let config = PoolConfig::default();
    let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory {
        behavior: Arc::new(Mutex::new(behavior)),
    });
    UserService::new(Arc::new(Pool::new(config, factory).expect("mock pool")))
}

fn make_role_service(behavior: MockBehavior) -> RoleService {
    let config = PoolConfig::default();
    let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory {
        behavior: Arc::new(Mutex::new(behavior)),
    });
    RoleService::new(Arc::new(Pool::new(config, factory).expect("mock pool")))
}

fn make_menu_service(behavior: MockBehavior) -> MenuService {
    let config = PoolConfig::default();
    let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory {
        behavior: Arc::new(Mutex::new(behavior)),
    });
    MenuService::new(Arc::new(Pool::new(config, factory).expect("mock pool")))
}

fn make_config_service(behavior: MockBehavior) -> ConfigService {
    let config = PoolConfig::default();
    let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory {
        behavior: Arc::new(Mutex::new(behavior)),
    });
    ConfigService::new(Arc::new(Pool::new(config, factory).expect("mock pool")))
}

fn make_log_service(behavior: MockBehavior) -> OperationLogService {
    let config = PoolConfig::default();
    let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory {
        behavior: Arc::new(Mutex::new(behavior)),
    });
    OperationLogService::new(Arc::new(Pool::new(config, factory).expect("mock pool")))
}

fn make_permission_service(behavior: MockBehavior) -> PermissionService {
    let config = PoolConfig::default();
    let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory {
        behavior: Arc::new(Mutex::new(behavior)),
    });
    PermissionService::new(Arc::new(Pool::new(config, factory).expect("mock pool")))
}

fn make_dashboard_service(behavior: MockBehavior) -> DashboardService {
    let config = PoolConfig::default();
    let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory {
        behavior: Arc::new(Mutex::new(behavior)),
    });
    DashboardService::new(Arc::new(Pool::new(config, factory).expect("mock pool")))
}

/// 构造 menu 行（id/name/code/path/parent_id/sort/is_visible/tenant_id/created_at/updated_at）
fn menu_row(id: i64, code: &str, parent_id: i64, sort: i32) -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::I64(id));
    row.insert("name".into(), Value::String(code.into()));
    row.insert("code".into(), Value::String(code.into()));
    row.insert("path".into(), Value::String(format!("/{code}")));
    row.insert("icon".into(), Value::Null);
    row.insert("parent_id".into(), Value::I64(parent_id));
    row.insert("sort".into(), Value::I32(sort));
    row.insert("is_visible".into(), Value::Bool(true));
    row.insert("permission_code".into(), Value::Null);
    row.insert("tenant_id".into(), Value::I64(0));
    row.insert(
        "created_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row.insert(
        "updated_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row
}

/// 构造 config 行
fn config_row(id: i64, key: &str, value: &str, value_type: &str) -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::I64(id));
    row.insert("key".into(), Value::String(key.into()));
    row.insert("value".into(), Value::String(value.into()));
    row.insert("value_type".into(), Value::String(value_type.into()));
    row.insert("group".into(), Value::Null);
    row.insert("description".into(), Value::Null);
    row.insert("tenant_id".into(), Value::I64(0));
    row.insert(
        "created_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row.insert(
        "updated_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row
}

/// 构造 operation_log 行
fn log_row(id: i64, op_type: &str, target_type: &str) -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::I64(id));
    row.insert("operator_id".into(), Value::I64(1));
    row.insert("operator_name".into(), Value::String("admin".into()));
    row.insert("operation_type".into(), Value::String(op_type.into()));
    row.insert("target_type".into(), Value::String(target_type.into()));
    row.insert("target_id".into(), Value::I64(5));
    row.insert("detail".into(), Value::Null);
    row.insert("ip".into(), Value::String("127.0.0.1".into()));
    row.insert("tenant_id".into(), Value::I64(0));
    row.insert(
        "created_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row
}

/// 构造 role 行
fn role_row(id: i64, name: &str, code: &str, is_builtin: bool) -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::I64(id));
    row.insert("name".into(), Value::String(name.into()));
    row.insert("code".into(), Value::String(code.into()));
    row.insert("description".into(), Value::Null);
    row.insert("is_builtin".into(), Value::Bool(is_builtin));
    row.insert("tenant_id".into(), Value::I64(0));
    row.insert(
        "created_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row.insert(
        "updated_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row
}

/// 构造 permission 行（仅 code 列，用于 list_all）
fn permission_code_row(code: &str) -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("code".into(), Value::String(code.into()));
    row
}

/// 构造 permission 行（code + name 列，用于 tree）
fn permission_tree_row(code: &str, name: &str) -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("code".into(), Value::String(code.into()));
    row.insert("name".into(), Value::String(name.into()));
    row
}

fn create_req(username: &str, password: &str) -> CreateUserRequest {
    CreateUserRequest {
        username: username.into(),
        password: password.into(),
        email: Some("alice@example.com".into()),
        phone: None,
    }
}

// ---------- create：验证矩阵 ----------

#[tokio::test]
async fn test_create_empty_username_rejected() {
    let result = make_service(MockBehavior::default())
        .create(create_req("   ", "password123"), 0)
        .await;
    assert!(matches!(result, Err(AdminError::UsernameRequired)));
}

#[tokio::test]
async fn test_create_short_username_rejected() {
    let result = make_service(MockBehavior::default())
        .create(create_req("ab", "password123"), 0)
        .await;
    assert!(matches!(result, Err(AdminError::UsernameRequired)));
}

#[tokio::test]
async fn test_create_overlong_username_rejected() {
    let username = "a".repeat(51);
    let result = make_service(MockBehavior::default())
        .create(create_req(&username, "password123"), 0)
        .await;
    assert!(matches!(result, Err(AdminError::UsernameRequired)));
}

#[tokio::test]
async fn test_create_short_password_rejected() {
    let result = make_service(MockBehavior::default())
        .create(create_req("alice", "Aa1!"), 0)
        .await;
    assert!(matches!(result, Err(AdminError::PasswordTooShort)));
}

#[tokio::test]
async fn test_create_duplicate_username_rejected() {
    let result = make_service(MockBehavior {
        duplicate_username: true,
        ..Default::default()
    })
    .create(create_req("alice", "password123"), 0)
    .await;
    assert!(matches!(result, Err(AdminError::UserDuplicate)));
}

#[tokio::test]
async fn test_create_success_returns_user_row() {
    let result = make_service(MockBehavior {
        return_user_row: true,
        ..Default::default()
    })
    .create(create_req("alice", "password123"), 0)
    .await
    .expect("create should succeed");
    assert_eq!(result.username, "alice");
    assert_eq!(result.id, 1);
    assert_eq!(result.status, UserStatus::Active);
}

#[tokio::test]
async fn test_create_row_lost_returns_internal() {
    // INSERT 成功但后续 SELECT 未命中（返回空行）→ Internal("创建后查询失败")
    let result = make_service(MockBehavior::default())
        .create(create_req("alice", "password123"), 0)
        .await;
    assert!(matches!(result, Err(AdminError::Internal(_))));
}

// ---------- list：分页钳制 ----------

#[tokio::test]
async fn test_list_page_size_clamped() {
    let page = make_service(MockBehavior {
        total_cnt: 7,
        ..Default::default()
    })
    .list(UserListFilter::default(), 0, 0, 500)
    .await
    .expect("list should succeed");
    assert_eq!(page.page, 1, "page=0 应钳制为 1");
    assert_eq!(page.page_size, 100, "size=500 应钳制为 100");
    assert_eq!(page.total, 7);
    assert!(page.items.is_empty(), "mock 空行集 → items 为空");
}

#[tokio::test]
async fn test_list_empty_result() {
    let page = make_service(MockBehavior::default())
        .list(UserListFilter::default(), 0, 1, 20)
        .await
        .expect("list should succeed");
    assert_eq!(page.total, 0);
    assert!(page.items.is_empty());
}

// ---------- update：存在性 / 验证 / 空请求回退 ----------

#[tokio::test]
async fn test_update_user_not_found() {
    let result = make_service(MockBehavior::default())
        .update(
            42,
            UpdateUserRequest {
                password: None,
                email: Some("new@example.com".into()),
                phone: None,
            },
            0,
        )
        .await;
    assert!(matches!(result, Err(AdminError::UserNotFound)));
}

#[tokio::test]
async fn test_update_short_password_rejected() {
    let result = make_service(MockBehavior {
        user_exists: true,
        ..Default::default()
    })
    .update(
        1,
        UpdateUserRequest {
            password: Some("short".into()),
            email: None,
            phone: None,
        },
        0,
    )
    .await;
    assert!(matches!(result, Err(AdminError::PasswordTooShort)));
}

#[tokio::test]
async fn test_update_empty_request_falls_back_to_get() {
    // 全空请求不产生 UPDATE，直接回退 get_by_id
    let result = make_service(MockBehavior {
        user_exists: true,
        return_user_row: true,
        ..Default::default()
    })
    .update(
        1,
        UpdateUserRequest {
            password: None,
            email: None,
            phone: None,
        },
        0,
    )
    .await
    .expect("empty update should fall back to get_by_id");
    assert_eq!(result.username, "alice");
}

// ---------- delete：超级管理员保护 / 回滚 / 成功 ----------

#[tokio::test]
async fn test_delete_success() {
    let result = make_service(MockBehavior {
        delete_affected: 1,
        ..Default::default()
    })
    .delete(1, 0)
    .await;
    assert!(result.is_ok(), "普通用户删除应成功: {result:?}");
}

#[tokio::test]
async fn test_delete_affected_zero_rolls_back_to_not_found() {
    let result = make_service(MockBehavior {
        delete_affected: 0,
        ..Default::default()
    })
    .delete(1, 0)
    .await;
    assert!(matches!(result, Err(AdminError::UserNotFound)));
}

#[tokio::test]
async fn test_is_super_admin_empty_rows_false() {
    let result = make_service(MockBehavior::default())
        .is_super_admin(1, 0)
        .await
        .expect("is_super_admin should succeed");
    assert!(!result);
}

// ---------- role_service：验证矩阵 ----------

#[tokio::test]
async fn test_role_create_empty_name_rejected() {
    let result = make_role_service(MockBehavior::default())
        .create(
            CreateRoleRequest {
                name: "  ".into(),
                code: "viewer".into(),
                description: None,
            },
            0,
        )
        .await;
    assert!(matches!(result, Err(AdminError::RoleNameRequired)));
}

#[tokio::test]
async fn test_role_create_invalid_code_rejected() {
    // code 规则：2-50 字节且仅 ascii_lowercase / '_'
    let result = make_role_service(MockBehavior::default())
        .create(
            CreateRoleRequest {
                name: "Viewer".into(),
                code: "BAD-CODE".into(),
                description: None,
            },
            0,
        )
        .await;
    assert!(matches!(result, Err(AdminError::RoleNameRequired)));
}
// =========================================================================
// MenuService 测试
// =========================================================================

fn menu_create_req(code: &str, parent_id: i64) -> CreateMenuRequest {
    CreateMenuRequest {
        name: code.into(),
        code: code.into(),
        path: format!("/{code}"),
        icon: None,
        parent_id,
        sort: 0,
        is_visible: true,
        permission_code: None,
    }
}

#[tokio::test]
async fn test_menu_list_all_returns_rows() {
    let menus = make_menu_service(MockBehavior {
        menu_rows: vec![menu_row(1, "root", 0, 1), menu_row(2, "child", 1, 0)],
        ..Default::default()
    })
    .list_all(0)
    .await
    .expect("list_all should succeed");
    assert_eq!(menus.len(), 2);
    assert_eq!(menus[0].code, "root");
    assert_eq!(menus[1].parent_id, 1);
}

#[tokio::test]
async fn test_menu_list_all_empty() {
    let menus = make_menu_service(MockBehavior::default())
        .list_all(0)
        .await
        .expect("list_all empty should succeed");
    assert!(menus.is_empty());
}

#[tokio::test]
async fn test_menu_tree_builds_hierarchy() {
    let tree = make_menu_service(MockBehavior {
        menu_rows: vec![
            menu_row(1, "root", 0, 2),
            menu_row(2, "a", 0, 1),
            menu_row(3, "a_child", 2, 0),
        ],
        ..Default::default()
    })
    .tree(&[], 0)
    .await
    .expect("tree should succeed");
    assert_eq!(tree.len(), 2, "两个根菜单");
    assert_eq!(tree[0].menu.code, "a", "sort 升序");
    assert_eq!(tree[1].menu.code, "root");
    assert_eq!(tree[0].children.len(), 1, "a 有一个子菜单");
    assert_eq!(tree[0].children[0].menu.code, "a_child");
}

#[tokio::test]
async fn test_menu_create_success() {
    let menu = make_menu_service(MockBehavior {
        menu_rows: vec![menu_row(1, "new_menu", 0, 0)],
        ..Default::default()
    })
    .create(menu_create_req("new_menu", 0), 0)
    .await
    .expect("create should succeed");
    assert_eq!(menu.code, "new_menu");
    assert_eq!(menu.id, 1);
}

#[tokio::test]
async fn test_menu_create_duplicate_code_rejected() {
    let result = make_menu_service(MockBehavior {
        duplicate: true,
        ..Default::default()
    })
    .create(menu_create_req("dup", 0), 0)
    .await;
    assert!(matches!(result, Err(AdminError::MenuCodeDuplicate)));
}

#[tokio::test]
async fn test_menu_create_parent_not_found() {
    let result = make_menu_service(MockBehavior {
        entity_exists: false,
        ..Default::default()
    })
    .create(menu_create_req("child", 99), 0)
    .await;
    assert!(matches!(result, Err(AdminError::ParentMenuNotFound)));
}

#[tokio::test]
async fn test_menu_create_with_valid_parent() {
    let menu = make_menu_service(MockBehavior {
        entity_exists: true,
        menu_rows: vec![menu_row(2, "child", 1, 0)],
        ..Default::default()
    })
    .create(menu_create_req("child", 1), 0)
    .await
    .expect("create with parent should succeed");
    assert_eq!(menu.code, "child");
}

#[tokio::test]
async fn test_menu_create_row_lost_returns_internal() {
    let result = make_menu_service(MockBehavior::default())
        .create(menu_create_req("ghost", 0), 0)
        .await;
    assert!(matches!(result, Err(AdminError::Internal(_))));
}

#[tokio::test]
async fn test_menu_update_success() {
    let menu = make_menu_service(MockBehavior {
        menu_rows: vec![menu_row(1, "target", 0, 0)],
        ..Default::default()
    })
    .update(
        1,
        UpdateMenuRequest {
            name: Some("renamed".into()),
            path: None,
            icon: None,
            parent_id: None,
            sort: Some(5),
            is_visible: None,
            permission_code: None,
        },
        0,
    )
    .await
    .expect("update should succeed");
    assert_eq!(menu.id, 1);
}

#[tokio::test]
async fn test_menu_update_not_found() {
    let result = make_menu_service(MockBehavior::default())
        .update(
            42,
            UpdateMenuRequest {
                name: Some("x".into()),
                path: None,
                icon: None,
                parent_id: None,
                sort: None,
                is_visible: None,
                permission_code: None,
            },
            0,
        )
        .await;
    assert!(matches!(result, Err(AdminError::MenuNotFound)));
}

#[tokio::test]
async fn test_menu_update_empty_request_returns_existing() {
    let menu = make_menu_service(MockBehavior {
        menu_rows: vec![menu_row(1, "keep", 0, 0)],
        ..Default::default()
    })
    .update(
        1,
        UpdateMenuRequest {
            name: None,
            path: None,
            icon: None,
            parent_id: None,
            sort: None,
            is_visible: None,
            permission_code: None,
        },
        0,
    )
    .await
    .expect("empty update should return existing");
    assert_eq!(menu.code, "keep");
}

#[tokio::test]
async fn test_menu_update_circular_reference_rejected() {
    // menu 1 parent=0，menu 2 parent=1；将 menu 1 的 parent 改为 2 → 循环
    let result = make_menu_service(MockBehavior {
        menu_rows: vec![menu_row(1, "a", 0, 0), menu_row(2, "b", 1, 0)],
        ..Default::default()
    })
    .update(
        1,
        UpdateMenuRequest {
            name: None,
            path: None,
            icon: None,
            parent_id: Some(2),
            sort: None,
            is_visible: None,
            permission_code: None,
        },
        0,
    )
    .await;
    assert!(matches!(result, Err(AdminError::MenuCircularReference)));
}

#[tokio::test]
async fn test_menu_update_parent_not_found_in_list() {
    let result = make_menu_service(MockBehavior {
        menu_rows: vec![menu_row(1, "only", 0, 0)],
        ..Default::default()
    })
    .update(
        1,
        UpdateMenuRequest {
            name: None,
            path: None,
            icon: None,
            parent_id: Some(999),
            sort: None,
            is_visible: None,
            permission_code: None,
        },
        0,
    )
    .await;
    assert!(matches!(result, Err(AdminError::ParentMenuNotFound)));
}

#[tokio::test]
async fn test_menu_delete_success() {
    let result = make_menu_service(MockBehavior {
        delete_affected: 1,
        ..Default::default()
    })
    .delete(1, 0)
    .await;
    assert!(result.is_ok(), "delete should succeed: {result:?}");
}

#[tokio::test]
async fn test_menu_delete_has_child_rejected() {
    let result = make_menu_service(MockBehavior {
        menu_has_child: true,
        ..Default::default()
    })
    .delete(1, 0)
    .await;
    assert!(matches!(result, Err(AdminError::HasChildMenu)));
}

#[tokio::test]
async fn test_menu_delete_not_found() {
    let result = make_menu_service(MockBehavior {
        delete_affected: 0,
        ..Default::default()
    })
    .delete(42, 0)
    .await;
    assert!(matches!(result, Err(AdminError::MenuNotFound)));
}

// =========================================================================
// ConfigService 测试
// =========================================================================

fn upsert_req(value: serde_json::Value, value_type: ConfigType) -> UpsertConfigRequest {
    UpsertConfigRequest {
        value,
        value_type,
        group: Some("system".into()),
        description: None,
    }
}

#[tokio::test]
async fn test_config_list_no_group() {
    let configs = make_config_service(MockBehavior {
        config_rows: vec![
            config_row(1, "key1", "\"v1\"", "string"),
            config_row(2, "key2", "42", "number"),
        ],
        ..Default::default()
    })
    .list(None, 0)
    .await
    .expect("list should succeed");
    assert_eq!(configs.len(), 2);
    assert_eq!(configs[0].key, "key1");
}

#[tokio::test]
async fn test_config_list_with_group() {
    let configs = make_config_service(MockBehavior {
        config_rows: vec![config_row(1, "g_key", "true", "boolean")],
        ..Default::default()
    })
    .list(Some("system".into()), 0)
    .await
    .expect("list with group should succeed");
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].key, "g_key");
}

#[tokio::test]
async fn test_config_upsert_insert_new() {
    let config = make_config_service(MockBehavior {
        config_rows: vec![config_row(1, "new_key", "\"hello\"", "string")],
        ..Default::default()
    })
    .upsert(
        "new_key".into(),
        upsert_req(serde_json::json!("hello"), ConfigType::String),
        0,
    )
    .await
    .expect("upsert insert should succeed");
    assert_eq!(config.key, "new_key");
    assert_eq!(config.id, 1);
}

#[tokio::test]
async fn test_config_upsert_update_existing() {
    let config = make_config_service(MockBehavior {
        duplicate: true,
        config_rows: vec![config_row(1, "exist_key", "true", "boolean")],
        ..Default::default()
    })
    .upsert(
        "exist_key".into(),
        upsert_req(serde_json::json!(true), ConfigType::Boolean),
        0,
    )
    .await
    .expect("upsert update should succeed");
    assert_eq!(config.key, "exist_key");
}

#[tokio::test]
async fn test_config_upsert_type_mismatch_rejected() {
    let result = make_config_service(MockBehavior::default())
        .upsert(
            "bad".into(),
            upsert_req(serde_json::json!("not_bool"), ConfigType::Boolean),
            0,
        )
        .await;
    assert!(matches!(result, Err(AdminError::ConfigTypeMismatch)));
}

#[tokio::test]
async fn test_config_upsert_row_lost_returns_not_found() {
    let result = make_config_service(MockBehavior::default())
        .upsert(
            "ghost".into(),
            upsert_req(serde_json::json!("x"), ConfigType::String),
            0,
        )
        .await;
    assert!(matches!(result, Err(AdminError::ConfigNotFound)));
}

#[tokio::test]
async fn test_config_delete_success() {
    let result = make_config_service(MockBehavior {
        delete_affected: 1,
        ..Default::default()
    })
    .delete("key", 0)
    .await;
    assert!(result.is_ok(), "delete should succeed: {result:?}");
}

#[tokio::test]
async fn test_config_delete_not_found() {
    let result = make_config_service(MockBehavior {
        delete_affected: 0,
        ..Default::default()
    })
    .delete("missing", 0)
    .await;
    assert!(matches!(result, Err(AdminError::ConfigNotFound)));
}

#[tokio::test]
async fn test_config_get_with_inheritance_hit() {
    let config = make_config_service(MockBehavior {
        config_rows: vec![config_row(1, "inh", "42", "number")],
        ..Default::default()
    })
    .get_with_inheritance("inh", 0)
    .await
    .expect("get_with_inheritance should succeed");
    assert!(config.is_some());
    assert_eq!(config.unwrap().key, "inh");
}

#[tokio::test]
async fn test_config_get_with_inheritance_miss() {
    let config = make_config_service(MockBehavior::default())
        .get_with_inheritance("none", 0)
        .await
        .expect("miss should succeed");
    assert!(config.is_none());
}

// =========================================================================
// OperationLogService 测试
// =========================================================================

#[tokio::test]
async fn test_log_list_normal() {
    let page = make_log_service(MockBehavior {
        total_cnt: 3,
        log_rows: vec![log_row(1, "create", "user"), log_row(2, "update", "role")],
        ..Default::default()
    })
    .list(LogFilter::default(), 0, 1, 20)
    .await
    .expect("list should succeed");
    assert_eq!(page.total, 3);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].operator_name, "admin");
}

#[tokio::test]
async fn test_log_list_invalid_time_range() {
    let end = chrono::Utc::now();
    let start = end + chrono::Duration::hours(1);
    let result = make_log_service(MockBehavior::default())
        .list(
            LogFilter {
                start_time: Some(start),
                end_time: Some(end),
                ..Default::default()
            },
            0,
            1,
            20,
        )
        .await;
    assert!(matches!(result, Err(AdminError::InvalidTimeRange)));
}

#[tokio::test]
async fn test_log_list_page_size_clamped() {
    let page = make_log_service(MockBehavior {
        total_cnt: 5,
        ..Default::default()
    })
    .list(LogFilter::default(), 0, 0, 500)
    .await
    .expect("clamped list should succeed");
    assert_eq!(page.page, 1);
    assert_eq!(page.page_size, 100);
}

#[tokio::test]
async fn test_log_list_with_filter() {
    let page = make_log_service(MockBehavior {
        total_cnt: 1,
        log_rows: vec![log_row(1, "delete", "menu")],
        ..Default::default()
    })
    .list(
        LogFilter {
            operator_id: Some(1),
            operation_type: Some(OperationType::Delete),
            ..Default::default()
        },
        0,
        1,
        10,
    )
    .await
    .expect("filtered list should succeed");
    assert_eq!(page.total, 1);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].operation_type, OperationType::Delete);
}

#[tokio::test]
async fn test_log_cleanup_expired() {
    let affected = make_log_service(MockBehavior {
        delete_affected: 7,
        ..Default::default()
    })
    .cleanup_expired(30)
    .await
    .expect("cleanup should succeed");
    assert_eq!(affected, 7);
}

// =========================================================================
// RoleService 扩展测试
// =========================================================================

#[tokio::test]
async fn test_role_list_returns_rows() {
    let roles = make_role_service(MockBehavior {
        role_rows: vec![
            role_row(1, "管理员", "admin", true),
            role_row(2, "访客", "viewer", false),
        ],
        ..Default::default()
    })
    .list(0)
    .await
    .expect("list should succeed");
    assert_eq!(roles.len(), 2);
    assert_eq!(roles[0].code, "admin");
    assert!(roles[0].is_builtin);
    assert!(!roles[1].is_builtin);
}

#[tokio::test]
async fn test_role_create_success() {
    let role = make_role_service(MockBehavior {
        role_rows: vec![role_row(1, "编辑者", "editor", false)],
        ..Default::default()
    })
    .create(
        CreateRoleRequest {
            name: "编辑者".into(),
            code: "editor".into(),
            description: None,
        },
        0,
    )
    .await
    .expect("create should succeed");
    assert_eq!(role.code, "editor");
    assert_eq!(role.id, 1);
}

#[tokio::test]
async fn test_role_create_duplicate_code_rejected() {
    let result = make_role_service(MockBehavior {
        duplicate: true,
        ..Default::default()
    })
    .create(
        CreateRoleRequest {
            name: "重复".into(),
            code: "dup".into(),
            description: None,
        },
        0,
    )
    .await;
    assert!(matches!(result, Err(AdminError::RoleCodeDuplicate)));
}

#[tokio::test]
async fn test_role_create_row_lost_returns_internal() {
    let result = make_role_service(MockBehavior::default())
        .create(
            CreateRoleRequest {
                name: "幽灵".into(),
                code: "ghost".into(),
                description: None,
            },
            0,
        )
        .await;
    assert!(matches!(result, Err(AdminError::Internal(_))));
}

#[tokio::test]
async fn test_role_update_success() {
    let role = make_role_service(MockBehavior {
        entity_exists: true,
        role_rows: vec![role_row(1, "改名后", "r1", false)],
        ..Default::default()
    })
    .update(
        1,
        UpdateRoleRequest {
            name: Some("改名后".into()),
            description: Some("新描述".into()),
        },
        0,
    )
    .await
    .expect("update should succeed");
    assert_eq!(role.name, "改名后");
}

#[tokio::test]
async fn test_role_update_not_found() {
    let result = make_role_service(MockBehavior {
        entity_exists: false,
        ..Default::default()
    })
    .update(
        42,
        UpdateRoleRequest {
            name: Some("x".into()),
            description: None,
        },
        0,
    )
    .await;
    assert!(matches!(result, Err(AdminError::RoleNotFound)));
}

#[tokio::test]
async fn test_role_update_empty_request_falls_back_to_get() {
    let role = make_role_service(MockBehavior {
        entity_exists: true,
        role_rows: vec![role_row(1, "保持", "keep", false)],
        ..Default::default()
    })
    .update(
        1,
        UpdateRoleRequest {
            name: None,
            description: None,
        },
        0,
    )
    .await
    .expect("empty update should fall back");
    assert_eq!(role.code, "keep");
}

#[tokio::test]
async fn test_role_delete_success() {
    let result = make_role_service(MockBehavior {
        entity_exists: true,
        role_is_builtin: false,
        delete_affected: 1,
        ..Default::default()
    })
    .delete(1, 0)
    .await;
    assert!(result.is_ok(), "delete should succeed: {result:?}");
}

#[tokio::test]
async fn test_role_delete_not_found() {
    let result = make_role_service(MockBehavior {
        entity_exists: false,
        ..Default::default()
    })
    .delete(42, 0)
    .await;
    assert!(matches!(result, Err(AdminError::RoleNotFound)));
}

#[tokio::test]
async fn test_role_delete_builtin_protected() {
    let result = make_role_service(MockBehavior {
        entity_exists: true,
        role_is_builtin: true,
        ..Default::default()
    })
    .delete(1, 0)
    .await;
    assert!(matches!(result, Err(AdminError::BuiltinRoleProtected)));
}

#[tokio::test]
async fn test_role_delete_affected_zero_rolls_back() {
    let result = make_role_service(MockBehavior {
        entity_exists: true,
        role_is_builtin: false,
        delete_affected: 0,
        ..Default::default()
    })
    .delete(1, 0)
    .await;
    assert!(matches!(result, Err(AdminError::RoleNotFound)));
}

#[tokio::test]
async fn test_role_assign_permissions_success() {
    let result = make_role_service(MockBehavior {
        entity_exists: true,
        permission_exists: true,
        ..Default::default()
    })
    .assign_permissions(
        1,
        vec!["admin:user:list".into(), "admin:role:list".into()],
        0,
    )
    .await;
    assert!(result.is_ok(), "assign should succeed: {result:?}");
}

#[tokio::test]
async fn test_role_assign_permissions_role_not_found() {
    let result = make_role_service(MockBehavior {
        entity_exists: false,
        ..Default::default()
    })
    .assign_permissions(42, vec!["admin:user:list".into()], 0)
    .await;
    assert!(matches!(result, Err(AdminError::RoleNotFound)));
}

#[tokio::test]
async fn test_role_assign_permissions_permission_not_found() {
    let result = make_role_service(MockBehavior {
        entity_exists: true,
        permission_exists: false,
        ..Default::default()
    })
    .assign_permissions(1, vec!["unknown:perm".into()], 0)
    .await;
    assert!(matches!(result, Err(AdminError::PermissionNotFound)));
}

// =========================================================================
// PermissionService 测试
// =========================================================================

#[tokio::test]
async fn test_permission_list_all_returns_triplets() {
    let perms = make_permission_service(MockBehavior {
        permission_rows: vec![
            permission_code_row("admin:user:list"),
            permission_code_row("admin:role:create"),
        ],
        ..Default::default()
    })
    .list_all(0)
    .await
    .expect("list_all should succeed");
    assert_eq!(perms.len(), 2);
    assert_eq!(perms[0].0, "admin");
    assert_eq!(perms[0].1, "user");
    assert_eq!(perms[0].2, "list");
}

#[tokio::test]
async fn test_permission_list_all_filters_non_three_part() {
    let perms = make_permission_service(MockBehavior {
        permission_rows: vec![
            permission_code_row("admin:user:list"),
            permission_code_row("invalid_code"),
            permission_code_row("a:b"),
        ],
        ..Default::default()
    })
    .list_all(0)
    .await
    .expect("filtered list_all should succeed");
    assert_eq!(perms.len(), 1, "仅三段式 code 应保留");
}

#[tokio::test]
async fn test_permission_tree_builds_structure() {
    let tree = make_permission_service(MockBehavior {
        permission_rows: vec![
            permission_tree_row("admin:user:list", "用户列表"),
            permission_tree_row("admin:user:create", "创建用户"),
            permission_tree_row("admin:role:list", "角色列表"),
        ],
        ..Default::default()
    })
    .tree(0)
    .await
    .expect("tree should succeed");
    assert_eq!(tree.modules.len(), 1, "一个 module: admin");
    assert_eq!(tree.modules[0].name, "admin");
    assert_eq!(tree.modules[0].resources.len(), 2, "user + role");
    // BTreeMap 按字母序：role < user
    assert_eq!(tree.modules[0].resources[0].name, "role");
    assert_eq!(tree.modules[0].resources[1].name, "user");
    assert_eq!(tree.modules[0].resources[1].actions.len(), 2);
}

#[tokio::test]
async fn test_permission_tree_empty() {
    let tree = make_permission_service(MockBehavior::default())
        .tree(0)
        .await
        .expect("empty tree should succeed");
    assert!(tree.modules.is_empty());
}

#[tokio::test]
async fn test_permission_tree_filters_non_three_part() {
    let tree = make_permission_service(MockBehavior {
        permission_rows: vec![
            permission_tree_row("admin:user:list", "列表"),
            permission_tree_row("bad", "坏"),
        ],
        ..Default::default()
    })
    .tree(0)
    .await
    .expect("filtered tree should succeed");
    assert_eq!(tree.modules.len(), 1);
    assert_eq!(tree.modules[0].resources[0].actions.len(), 1);
}

// =========================================================================
// UserService 扩展测试（update_status / assign_roles / update 成功 / delete 保护）
// =========================================================================

#[tokio::test]
async fn test_user_update_status_success() {
    let result = make_service(MockBehavior {
        update_affected: 1,
        ..Default::default()
    })
    .update_status(1, UserStatus::Disabled, 0)
    .await;
    assert!(result.is_ok(), "update_status should succeed: {result:?}");
}

#[tokio::test]
async fn test_user_update_status_not_found() {
    let result = make_service(MockBehavior {
        update_affected: 0,
        ..Default::default()
    })
    .update_status(42, UserStatus::Locked, 0)
    .await;
    assert!(matches!(result, Err(AdminError::UserNotFound)));
}

#[tokio::test]
async fn test_user_assign_roles_success() {
    let result = make_service(MockBehavior::default())
        .assign_roles(1, vec![1, 2, 3], 0)
        .await;
    assert!(result.is_ok(), "assign_roles should succeed: {result:?}");
}

#[tokio::test]
async fn test_user_assign_roles_empty_list() {
    let result = make_service(MockBehavior::default())
        .assign_roles(1, vec![], 0)
        .await;
    assert!(result.is_ok(), "empty assign should succeed: {result:?}");
}

#[tokio::test]
async fn test_user_update_with_email_success() {
    let user = make_service(MockBehavior {
        user_exists: true,
        return_user_row: true,
        ..Default::default()
    })
    .update(
        1,
        UpdateUserRequest {
            password: None,
            email: Some("new@example.com".into()),
            phone: Some("13800000000".into()),
        },
        0,
    )
    .await
    .expect("update with email should succeed");
    assert_eq!(user.username, "alice");
}

#[tokio::test]
async fn test_user_delete_super_admin_protected() {
    let result = make_service(MockBehavior {
        is_super_admin: true,
        ..Default::default()
    })
    .delete(1, 0)
    .await;
    assert!(matches!(result, Err(AdminError::SuperAdminProtected)));
}

#[tokio::test]
async fn test_user_is_super_admin_true() {
    let result = make_service(MockBehavior {
        is_super_admin: true,
        ..Default::default()
    })
    .is_super_admin(1, 0)
    .await
    .expect("is_super_admin true should succeed");
    assert!(result);
}

// =========================================================================
// DashboardService 测试
// =========================================================================

#[tokio::test]
async fn test_dashboard_stats_returns_counts() {
    let stats = make_dashboard_service(MockBehavior {
        total_cnt: 10,
        ..Default::default()
    })
    .stats(0)
    .await
    .expect("stats should succeed");
    assert_eq!(stats.user_count, 10);
    assert_eq!(stats.role_count, 10);
    assert_eq!(stats.permission_count, 10);
    assert_eq!(stats.menu_count, 10);
    assert_eq!(stats.today_log_count, 10);
    assert!(stats.system_status.is_some(), "应采集系统状态");
}

#[tokio::test]
async fn test_dashboard_stats_zero_counts() {
    let stats = make_dashboard_service(MockBehavior::default())
        .stats(0)
        .await
        .expect("zero stats should succeed");
    assert_eq!(stats.user_count, 0);
    assert_eq!(stats.role_count, 0);
}
