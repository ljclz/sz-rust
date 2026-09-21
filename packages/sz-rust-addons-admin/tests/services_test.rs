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
use sz_rust_addons_admin::models::user::UserStatus;
use sz_rust_addons_admin::services::role_service::{CreateRoleRequest, RoleService};
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

impl MockConnection {
    fn query(sql: &str, behavior: &MockBehavior) -> QueryRows {
        let sql = sql.to_ascii_lowercase();
        if sql.contains("count(*)") {
            let mut row = HashMap::new();
            row.insert("cnt".into(), Value::I64(behavior.total_cnt as i64));
            return vec![row];
        }
        if sql.contains("select id, username") {
            return if behavior.return_user_row {
                vec![user_row()]
            } else {
                vec![]
            };
        }
        if sql.contains("select id from users where username") {
            return if behavior.duplicate_username {
                vec![HashMap::new()]
            } else {
                vec![]
            };
        }
        if sql.contains("select id from users where id") {
            return if behavior.user_exists {
                vec![HashMap::new()]
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
        let affected = if lower.starts_with("delete from users") {
            self.behavior.lock().unwrap().delete_affected
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
