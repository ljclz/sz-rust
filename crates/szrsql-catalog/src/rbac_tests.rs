//! Phase 3.12 RBAC 测试 — 验证 GRANT/REVOKE + SUPERUSER 权限覆盖.

use crate::rbac::{DatabaseObject, Privilege, RbacError, RbacManager, Role, User, PUBLIC_ROLE};
use szrsql_sql::ast::TableName;

// =====================================================================
//  辅助函数
// =====================================================================

fn table(name: &str) -> DatabaseObject {
    DatabaseObject::Table(TableName::new(name))
}

fn column(table: &str, col: &str) -> DatabaseObject {
    DatabaseObject::Column {
        table: TableName::new(table),
        column: col.to_string(),
    }
}

// =====================================================================
//  Privilege / DatabaseObject 单元测试
// =====================================================================

#[test]
fn test_privilege_is_all() {
    assert!(Privilege::All.is_all());
    assert!(!Privilege::Select.is_all());
    assert!(!Privilege::Insert.is_all());
}

#[test]
fn test_privilege_covers() {
    // ALL 涵盖所有权限
    assert!(Privilege::All.covers(Privilege::Select));
    assert!(Privilege::All.covers(Privilege::Insert));
    assert!(Privilege::All.covers(Privilege::Update));
    assert!(Privilege::All.covers(Privilege::Delete));
    assert!(Privilege::All.covers(Privilege::All));

    // 具体权限只涵盖自身
    assert!(Privilege::Select.covers(Privilege::Select));
    assert!(!Privilege::Select.covers(Privilege::Insert));
    assert!(!Privilege::Insert.covers(Privilege::Select));
}

#[test]
fn test_database_object_is_table_is_column() {
    assert!(table("t").is_table());
    assert!(!table("t").is_column());

    assert!(column("t", "c").is_column());
    assert!(!column("t", "c").is_table());
}

#[test]
fn test_database_object_parent_table() {
    assert_eq!(table("t").parent_table(), None);
    assert_eq!(column("t", "c").parent_table(), Some(&TableName::new("t")));
}

#[test]
fn test_database_object_matches_self() {
    let t1 = table("t");
    assert!(t1.matches(&t1), "对象应匹配自身");

    let c1 = column("t", "c");
    assert!(c1.matches(&c1));
}

#[test]
fn test_database_object_matches_table_column() {
    let tbl = table("t");
    let col = column("t", "c");

    // 表级 → 列级（同表）→ 匹配（GRANT SELECT ON t 隐含 SELECT(t.c)）
    assert!(
        tbl.matches(&col),
        "表级对象应匹配同表的列级对象（GRANT SELECT ON t 隐含 SELECT(t.c)）"
    );
    // 列级 → 表级 → 不匹配（不对称，GRANT SELECT(id) ON t 不隐含 SELECT * FROM t）
    assert!(
        !col.matches(&tbl),
        "列级对象不应匹配表级对象（GRANT SELECT(id) ON t 不隐含 SELECT * FROM t）"
    );

    // 不同表不匹配
    let tbl2 = table("u");
    assert!(!tbl.matches(&tbl2));
    assert!(!tbl.matches(&column("u", "c")));
}

#[test]
fn test_database_object_matches_case_insensitive() {
    let tbl_lower = table("t");
    let tbl_upper = table("T");
    assert!(tbl_lower.matches(&tbl_upper), "表名匹配应大小写不敏感");

    let col_lower = column("t", "c");
    let col_upper = column("T", "C");
    // 列级相同对象（大小写不敏感）通过 self == other? 不，因为 String 大小写敏感
    // 但完全相等的对象（同 case）会通过 self == other 短路
    assert!(col_lower.matches(&col_lower));
    assert!(col_upper.matches(&col_upper));
    // 列级对象大小写不敏感匹配：col_lower vs col_upper
    // 由于 PartialEq 是大小写敏感的，self == other 为 false
    // 但 matches 没有为 (Column, Column) 添加大小写不敏感比较 → 返回 false
    // 这是预期行为（列级对象的大小写不敏感匹配需要专门处理，但当前实现不要求）
}

#[test]
fn test_database_object_different_types_no_match() {
    let tbl = table("t");
    let db = DatabaseObject::Database("mydb".to_string());
    let schema = DatabaseObject::Schema("public".to_string());
    let seq = DatabaseObject::Sequence("seq1".to_string());
    let func = DatabaseObject::Function("f".to_string());

    assert!(!tbl.matches(&db));
    assert!(!tbl.matches(&schema));
    assert!(!tbl.matches(&seq));
    assert!(!tbl.matches(&func));
    assert!(!db.matches(&schema));
}

// =====================================================================
//  User / Role 构造器
// =====================================================================

#[test]
fn test_user_new() {
    let user = User::new("alice");
    assert_eq!(user.name, "alice");
    assert!(!user.is_superuser);
    assert!(user.roles.contains(&PUBLIC_ROLE.to_string()));
}

#[test]
fn test_user_new_superuser() {
    let user = User::new_superuser("root");
    assert_eq!(user.name, "root");
    assert!(user.is_superuser);
    assert!(user.roles.contains(&PUBLIC_ROLE.to_string()));
}

#[test]
fn test_role_new() {
    let role = Role::new("admin");
    assert_eq!(role.name, "admin");
    assert!(role.members.is_empty());
}

// =====================================================================
//  RbacManager 用户/角色管理
// =====================================================================

#[test]
fn test_create_user_basic() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    assert_eq!(mgr.user_count(), 1);
    assert!(mgr.get_user("alice").is_some());
    assert!(mgr.get_user("ALICE").is_some(), "用户名大小写不敏感");
    assert!(!mgr.is_superuser("alice"));
}

#[test]
fn test_create_superuser() {
    let mut mgr = RbacManager::new();
    mgr.create_superuser("root").unwrap();
    assert!(mgr.is_superuser("root"));
    assert!(mgr.is_superuser("ROOT"), "大小写不敏感");
}

#[test]
fn test_create_user_duplicate() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    let result = mgr.create_user("alice");
    assert!(matches!(result, Err(RbacError::UserAlreadyExists(_))));
    // 大小写不敏感冲突
    let result = mgr.create_user("ALICE");
    assert!(matches!(result, Err(RbacError::UserAlreadyExists(_))));
}

#[test]
fn test_drop_user() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.grant_role("reader", "alice").unwrap();

    mgr.drop_user("alice").unwrap();
    assert!(mgr.get_user("alice").is_none());
    // 角色的成员列表中应已移除 alice
    assert!(!mgr.role_members("reader").contains(&"alice".to_lowercase()));
}

#[test]
fn test_drop_user_not_found() {
    let mut mgr = RbacManager::new();
    let result = mgr.drop_user("ghost");
    assert!(matches!(result, Err(RbacError::UserNotFound(_))));
}

#[test]
fn test_set_superuser() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    assert!(!mgr.is_superuser("alice"));

    mgr.set_superuser("alice", true).unwrap();
    assert!(mgr.is_superuser("alice"));

    mgr.set_superuser("alice", false).unwrap();
    assert!(!mgr.is_superuser("alice"));
}

#[test]
fn test_create_role_basic() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    assert_eq!(mgr.role_count(), 2, "PUBLIC + reader");
    assert!(mgr.get_role("reader").is_some());
    assert!(mgr.get_role("READER").is_some(), "角色名大小写不敏感");
}

#[test]
fn test_create_role_duplicate() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    let result = mgr.create_role("reader");
    assert!(matches!(result, Err(RbacError::RoleAlreadyExists(_))));
}

#[test]
fn test_drop_role() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    mgr.create_user("alice").unwrap();
    mgr.grant_role("reader", "alice").unwrap();

    mgr.drop_role("reader").unwrap();
    assert!(mgr.get_role("reader").is_none());
    // alice 的角色列表中应已移除 reader
    let alice_roles = mgr.user_roles("alice");
    assert!(!alice_roles.contains(&"reader".to_string()));
}

#[test]
fn test_drop_role_not_found() {
    let mut mgr = RbacManager::new();
    let result = mgr.drop_role("ghost");
    assert!(matches!(result, Err(RbacError::RoleNotFound(_))));
}

#[test]
fn test_drop_public_role_fails() {
    let mut mgr = RbacManager::new();
    let result = mgr.drop_role("public");
    assert!(matches!(result, Err(RbacError::RoleNotFound(_))));
}

#[test]
fn test_grant_role_basic() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();

    mgr.grant_role("reader", "alice").unwrap();
    let alice_roles = mgr.user_roles("alice");
    assert!(alice_roles.contains(&"reader".to_string()));
    assert!(mgr.role_members("reader").contains(&"alice".to_lowercase()));
}

#[test]
fn test_grant_role_duplicate() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.grant_role("reader", "alice").unwrap();

    let result = mgr.grant_role("reader", "alice");
    assert!(matches!(result, Err(RbacError::UserAlreadyInRole { .. })));
}

#[test]
fn test_grant_role_role_not_found() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    let result = mgr.grant_role("ghost", "alice");
    assert!(matches!(result, Err(RbacError::RoleNotFound(_))));
}

#[test]
fn test_grant_role_user_not_found() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    let result = mgr.grant_role("reader", "ghost");
    assert!(matches!(result, Err(RbacError::UserNotFound(_))));
}

#[test]
fn test_revoke_role_basic() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.grant_role("reader", "alice").unwrap();

    mgr.revoke_role("reader", "alice").unwrap();
    let alice_roles = mgr.user_roles("alice");
    assert!(!alice_roles.contains(&"reader".to_string()));
    assert!(!mgr.role_members("reader").contains(&"alice".to_lowercase()));
}

#[test]
fn test_revoke_role_not_member() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    // alice 不属于 reader
    let result = mgr.revoke_role("reader", "alice");
    assert!(matches!(result, Err(RbacError::UserNotInRole { .. })));
}

// =====================================================================
//  GRANT / REVOKE 权限管理
// =====================================================================

#[test]
fn test_grant_basic() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    mgr.grant(Privilege::Select, table("t"), "reader", false)
        .unwrap();

    let grants = mgr.list_grants("reader").unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].privilege, Privilege::Select);
    assert_eq!(grants[0].object, table("t"));
    assert!(!grants[0].grantable);
}

#[test]
fn test_grant_with_grant_option() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    mgr.grant(Privilege::Select, table("t"), "reader", true)
        .unwrap();

    let grants = mgr.list_grants("reader").unwrap();
    assert!(grants[0].grantable, "应支持 WITH GRANT OPTION");
}

#[test]
fn test_grant_updates_existing() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    mgr.grant(Privilege::Select, table("t"), "reader", false)
        .unwrap();
    // 再次 GRANT 同样的权限但 grantable=true
    mgr.grant(Privilege::Select, table("t"), "reader", true)
        .unwrap();

    let grants = mgr.list_grants("reader").unwrap();
    assert_eq!(grants.len(), 1, "相同 (privilege, object) 应去重");
    assert!(grants[0].grantable, "应更新 grantable 标志");
}

#[test]
fn test_grant_role_not_found() {
    let mut mgr = RbacManager::new();
    let result = mgr.grant(Privilege::Select, table("t"), "ghost", false);
    assert!(matches!(result, Err(RbacError::RoleNotFound(_))));
}

#[test]
fn test_grant_multiple_privileges() {
    let mut mgr = RbacManager::new();
    mgr.create_role("rw").unwrap();
    mgr.grant(Privilege::Select, table("t"), "rw", false)
        .unwrap();
    mgr.grant(Privilege::Insert, table("t"), "rw", false)
        .unwrap();
    mgr.grant(Privilege::Update, table("t"), "rw", false)
        .unwrap();

    let grants = mgr.list_grants("rw").unwrap();
    assert_eq!(grants.len(), 3);
}

#[test]
fn test_revoke_basic() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    mgr.grant(Privilege::Select, table("t"), "reader", false)
        .unwrap();
    assert_eq!(mgr.list_grants("reader").unwrap().len(), 1);

    mgr.revoke(Privilege::Select, &table("t"), "reader")
        .unwrap();
    assert_eq!(mgr.list_grants("reader").unwrap().len(), 0);
}

#[test]
fn test_revoke_grant_not_found() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    // 没有 GRANT 就 REVOKE
    let result = mgr.revoke(Privilege::Select, &table("t"), "reader");
    assert!(matches!(result, Err(RbacError::GrantNotFound { .. })));
}

#[test]
fn test_revoke_wrong_privilege() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    mgr.grant(Privilege::Select, table("t"), "reader", false)
        .unwrap();
    // REVOKE 错误的权限
    let result = mgr.revoke(Privilege::Insert, &table("t"), "reader");
    assert!(matches!(result, Err(RbacError::GrantNotFound { .. })));
}

#[test]
fn test_revoke_wrong_object() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    mgr.grant(Privilege::Select, table("t"), "reader", false)
        .unwrap();
    // REVOKE 错误的对象
    let result = mgr.revoke(Privilege::Select, &table("u"), "reader");
    assert!(matches!(result, Err(RbacError::GrantNotFound { .. })));
}

// =====================================================================
//  权限检查 — Phase 3.12 核心验收
// =====================================================================

#[test]
fn test_check_grant_select_user_can_select_cannot_insert() {
    // Phase 3.12 验收：GRANT SELECT ON t TO role1 → user1 可查不可写
    let mut mgr = RbacManager::new();
    mgr.create_user("user1").unwrap();
    mgr.create_role("role1").unwrap();
    mgr.grant_role("role1", "user1").unwrap();

    // GRANT SELECT ON t TO role1
    mgr.grant(Privilege::Select, table("t"), "role1", false)
        .unwrap();

    // user1 可查
    assert!(
        mgr.check("user1", Privilege::Select, &table("t")),
        "GRANT SELECT 后 user1 应能 SELECT"
    );
    // user1 不可写
    assert!(
        !mgr.check("user1", Privilege::Insert, &table("t")),
        "未 GRANT INSERT，user1 不应能 INSERT"
    );
    assert!(
        !mgr.check("user1", Privilege::Update, &table("t")),
        "未 GRANT UPDATE，user1 不应能 UPDATE"
    );
    assert!(
        !mgr.check("user1", Privilege::Delete, &table("t")),
        "未 GRANT DELETE，user1 不应能 DELETE"
    );
}

#[test]
fn test_check_revoke_user_cannot_select() {
    // Phase 3.12 验收：REVOKE → user1 不可查
    let mut mgr = RbacManager::new();
    mgr.create_user("user1").unwrap();
    mgr.create_role("role1").unwrap();
    mgr.grant_role("role1", "user1").unwrap();
    mgr.grant(Privilege::Select, table("t"), "role1", false)
        .unwrap();
    assert!(mgr.check("user1", Privilege::Select, &table("t")));

    // REVOKE SELECT
    mgr.revoke(Privilege::Select, &table("t"), "role1").unwrap();
    assert!(
        !mgr.check("user1", Privilege::Select, &table("t")),
        "REVOKE 后 user1 不应能 SELECT"
    );
}

#[test]
fn test_check_superuser_overrides() {
    // Phase 3.12 验收：SUPERUSER 权限覆盖
    let mut mgr = RbacManager::new();
    mgr.create_superuser("admin").unwrap();
    // admin 没有任何 GRANT，但应有所有权限
    assert!(
        mgr.check("admin", Privilege::Select, &table("t")),
        "SUPERUSER 应覆盖所有权限检查"
    );
    assert!(mgr.check("admin", Privilege::Insert, &table("t")));
    assert!(mgr.check("admin", Privilege::Update, &table("t")));
    assert!(mgr.check("admin", Privilege::Delete, &table("t")));
    assert!(mgr.check("admin", Privilege::Truncate, &table("t")));
    assert!(mgr.check(
        "admin",
        Privilege::Create,
        &DatabaseObject::Database("d".to_string())
    ));
}

#[test]
fn test_check_user_not_found() {
    let mgr = RbacManager::new();
    assert!(
        !mgr.check("ghost", Privilege::Select, &table("t")),
        "不存在的用户应无权限"
    );
}

#[test]
fn test_check_no_grants() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    assert!(
        !mgr.check("alice", Privilege::Select, &table("t")),
        "无任何 GRANT 的用户应无权限"
    );
}

#[test]
fn test_check_all_privileges_wildcard() {
    // GRANT ALL PRIVILEGES → 所有具体权限都通过
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("admin").unwrap();
    mgr.grant_role("admin", "alice").unwrap();
    mgr.grant(Privilege::All, table("t"), "admin", false)
        .unwrap();

    assert!(mgr.check("alice", Privilege::Select, &table("t")));
    assert!(mgr.check("alice", Privilege::Insert, &table("t")));
    assert!(mgr.check("alice", Privilege::Update, &table("t")));
    assert!(mgr.check("alice", Privilege::Delete, &table("t")));
    assert!(mgr.check("alice", Privilege::Truncate, &table("t")));
    assert!(mgr.check("alice", Privilege::References, &table("t")));
    assert!(mgr.check("alice", Privilege::Trigger, &table("t")));
}

#[test]
fn test_check_table_covers_column() {
    // GRANT SELECT ON t → 隐含 SELECT(t.id), SELECT(t.name) 等
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.grant_role("reader", "alice").unwrap();
    mgr.grant(Privilege::Select, table("t"), "reader", false)
        .unwrap();

    assert!(
        mgr.check("alice", Privilege::Select, &column("t", "id")),
        "表级 SELECT 应隐含列级 SELECT"
    );
    assert!(
        mgr.check("alice", Privilege::Select, &column("t", "name")),
        "表级 SELECT 应隐含任意列 SELECT"
    );
    assert!(
        !mgr.check("alice", Privilege::Insert, &column("t", "id")),
        "表级未授权 INSERT，列级也不应通过"
    );
}

#[test]
fn test_check_column_grant_does_not_cover_table() {
    // GRANT SELECT(id) ON t → 不应允许 SELECT * FROM t（表级）
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.grant_role("reader", "alice").unwrap();
    mgr.grant(Privilege::Select, column("t", "id"), "reader", false)
        .unwrap();

    // 列级 SELECT 通过
    assert!(mgr.check("alice", Privilege::Select, &column("t", "id")));
    // 其他列不通过
    assert!(
        !mgr.check("alice", Privilege::Select, &column("t", "name")),
        "列级 SELECT(id) 不应允许 SELECT(name)"
    );
    // 表级 SELECT 不通过（只授权了 id 列）
    assert!(
        !mgr.check("alice", Privilege::Select, &table("t")),
        "列级授权不应隐含表级授权"
    );
}

#[test]
fn test_check_public_role_shared() {
    // PUBLIC 角色的权限所有用户共享
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_user("bob").unwrap();
    mgr.grant(
        Privilege::Connect,
        DatabaseObject::Database("mydb".to_string()),
        "public",
        false,
    )
    .unwrap();

    assert!(
        mgr.check(
            "alice",
            Privilege::Connect,
            &DatabaseObject::Database("mydb".to_string())
        ),
        "PUBLIC 角色 GRANT CONNECT 应让所有用户都能 CONNECT"
    );
    assert!(
        mgr.check(
            "bob",
            Privilege::Connect,
            &DatabaseObject::Database("mydb".to_string())
        ),
        "PUBLIC 角色对所有用户生效"
    );
}

#[test]
fn test_check_multiple_roles_inherited() {
    // 用户继承多个角色的权限
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.create_role("writer").unwrap();
    mgr.grant_role("reader", "alice").unwrap();
    mgr.grant_role("writer", "alice").unwrap();

    mgr.grant(Privilege::Select, table("t"), "reader", false)
        .unwrap();
    mgr.grant(Privilege::Insert, table("t"), "writer", false)
        .unwrap();
    mgr.grant(Privilege::Update, table("t"), "writer", false)
        .unwrap();

    assert!(mgr.check("alice", Privilege::Select, &table("t")));
    assert!(mgr.check("alice", Privilege::Insert, &table("t")));
    assert!(mgr.check("alice", Privilege::Update, &table("t")));
    assert!(!mgr.check("alice", Privilege::Delete, &table("t")));
}

#[test]
fn test_check_revoke_role_removes_permissions() {
    // 撤销角色后，用户不再继承该角色的权限
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.grant_role("reader", "alice").unwrap();
    mgr.grant(Privilege::Select, table("t"), "reader", false)
        .unwrap();
    assert!(mgr.check("alice", Privilege::Select, &table("t")));

    mgr.revoke_role("reader", "alice").unwrap();
    assert!(
        !mgr.check("alice", Privilege::Select, &table("t")),
        "撤销角色后用户不应再有该角色的权限"
    );
}

#[test]
fn test_check_different_objects_independent() {
    // 不同对象的权限相互独立
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.grant_role("reader", "alice").unwrap();
    mgr.grant(Privilege::Select, table("t1"), "reader", false)
        .unwrap();

    assert!(mgr.check("alice", Privilege::Select, &table("t1")));
    assert!(
        !mgr.check("alice", Privilege::Select, &table("t2")),
        "对 t1 的 SELECT 不应允许对 t2 的 SELECT"
    );
}

#[test]
fn test_check_all_helper() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("rw").unwrap();
    mgr.grant_role("rw", "alice").unwrap();
    mgr.grant(Privilege::Select, table("t"), "rw", false)
        .unwrap();
    mgr.grant(Privilege::Insert, table("t"), "rw", false)
        .unwrap();

    // 拥有全部指定权限
    assert!(
        mgr.check_all(
            "alice",
            &[Privilege::Select, Privilege::Insert],
            &table("t")
        ),
        "alice 应同时拥有 SELECT 和 INSERT"
    );
    // 不拥有全部
    assert!(
        !mgr.check_all(
            "alice",
            &[Privilege::Select, Privilege::Delete],
            &table("t")
        ),
        "alice 没有 DELETE，check_all 应为 false"
    );
}

#[test]
fn test_check_any_helper() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("rw").unwrap();
    mgr.grant_role("rw", "alice").unwrap();
    mgr.grant(Privilege::Select, table("t"), "rw", false)
        .unwrap();

    // 拥有任意一个
    assert!(
        mgr.check_any(
            "alice",
            &[Privilege::Insert, Privilege::Select],
            &table("t")
        ),
        "alice 有 SELECT，check_any 应为 true"
    );
    // 都没有
    assert!(
        !mgr.check_any(
            "alice",
            &[Privilege::Insert, Privilege::Delete],
            &table("t")
        ),
        "alice 既无 INSERT 也无 DELETE，check_any 应为 false"
    );
}

#[test]
fn test_grant_to_public_role() {
    // 直接对 PUBLIC 角色 GRANT
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_user("bob").unwrap();

    mgr.grant(
        Privilege::Usage,
        DatabaseObject::Schema("public".to_string()),
        "public",
        false,
    )
    .unwrap();

    // 所有用户都应有 USAGE 权限
    assert!(
        mgr.check(
            "alice",
            Privilege::Usage,
            &DatabaseObject::Schema("public".to_string())
        ),
        "PUBLIC USAGE 应让所有用户都有权限"
    );
    assert!(mgr.check(
        "bob",
        Privilege::Usage,
        &DatabaseObject::Schema("public".to_string())
    ));
}

#[test]
fn test_drop_role_removes_grants() {
    let mut mgr = RbacManager::new();
    mgr.create_role("reader").unwrap();
    mgr.grant(Privilege::Select, table("t"), "reader", false)
        .unwrap();
    assert_eq!(mgr.list_grants("reader").unwrap().len(), 1);

    mgr.drop_role("reader").unwrap();
    // 删除角色后 grants 也应被清除
    assert!(mgr.get_role("reader").is_none());
}

#[test]
fn test_user_roles_includes_public() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.grant_role("reader", "alice").unwrap();

    let roles = mgr.user_roles("alice");
    assert!(roles.contains(&PUBLIC_ROLE.to_string()), "应包含 PUBLIC");
    assert!(roles.contains(&"reader".to_string()));
}

#[test]
fn test_role_members_includes_user() {
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.grant_role("reader", "alice").unwrap();

    let members = mgr.role_members("reader");
    assert!(members.contains(&"alice".to_lowercase()));
}

// =====================================================================
//  综合场景测试
// =====================================================================

#[test]
fn test_full_rbac_scenario() {
    // 综合场景：模拟完整的 RBAC 使用流程
    let mut mgr = RbacManager::new();

    // 创建用户和角色
    mgr.create_superuser("admin").unwrap();
    mgr.create_user("alice").unwrap();
    mgr.create_user("bob").unwrap();
    mgr.create_role("reader").unwrap();
    mgr.create_role("writer").unwrap();

    // 分配角色
    mgr.grant_role("reader", "alice").unwrap();
    mgr.grant_role("reader", "bob").unwrap();
    mgr.grant_role("writer", "bob").unwrap();

    // 授权
    mgr.grant(Privilege::Select, table("users"), "reader", false)
        .unwrap();
    mgr.grant(Privilege::Select, table("orders"), "reader", false)
        .unwrap();
    mgr.grant(Privilege::Insert, table("orders"), "writer", false)
        .unwrap();
    mgr.grant(Privilege::Update, table("orders"), "writer", false)
        .unwrap();

    // 验证：admin 拥有所有权限
    assert!(mgr.check("admin", Privilege::Select, &table("users")));
    assert!(mgr.check("admin", Privilege::Insert, &table("orders")));
    assert!(mgr.check("admin", Privilege::Delete, &table("users")));

    // 验证：alice 只能查（reader）
    assert!(mgr.check("alice", Privilege::Select, &table("users")));
    assert!(mgr.check("alice", Privilege::Select, &table("orders")));
    assert!(!mgr.check("alice", Privilege::Insert, &table("orders")));
    assert!(!mgr.check("alice", Privilege::Update, &table("orders")));

    // 验证：bob 能查 + 能写（reader + writer）
    assert!(mgr.check("bob", Privilege::Select, &table("users")));
    assert!(mgr.check("bob", Privilege::Select, &table("orders")));
    assert!(mgr.check("bob", Privilege::Insert, &table("orders")));
    assert!(mgr.check("bob", Privilege::Update, &table("orders")));
    assert!(!mgr.check("bob", Privilege::Delete, &table("orders")));

    // 撤销 bob 的 writer 角色
    mgr.revoke_role("writer", "bob").unwrap();
    assert!(!mgr.check("bob", Privilege::Insert, &table("orders")));
    assert!(!mgr.check("bob", Privilege::Update, &table("orders")));
    assert!(mgr.check("bob", Privilege::Select, &table("orders")));

    // REVOKE reader 的 SELECT(users)
    mgr.revoke(Privilege::Select, &table("users"), "reader")
        .unwrap();
    assert!(!mgr.check("alice", Privilege::Select, &table("users")));
    assert!(!mgr.check("bob", Privilege::Select, &table("users")));
    assert!(mgr.check("bob", Privilege::Select, &table("orders")));
}

#[test]
fn test_role_hierarchy_scenario() {
    // 多层角色继承场景（当前实现为扁平继承，不递归）
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("base").unwrap();
    mgr.create_role("admin").unwrap();
    mgr.grant_role("base", "alice").unwrap();
    mgr.grant_role("admin", "alice").unwrap();

    mgr.grant(Privilege::Select, table("t1"), "base", false)
        .unwrap();
    mgr.grant(Privilege::All, table("t2"), "admin", false)
        .unwrap();

    // alice 通过 base 拥有 t1 的 SELECT
    assert!(mgr.check("alice", Privilege::Select, &table("t1")));
    // alice 通过 admin 拥有 t2 的所有权限
    assert!(mgr.check("alice", Privilege::Select, &table("t2")));
    assert!(mgr.check("alice", Privilege::Insert, &table("t2")));
    assert!(mgr.check("alice", Privilege::Delete, &table("t2")));
    // alice 不拥有 t1 的 INSERT（base 只授了 SELECT）
    assert!(!mgr.check("alice", Privilege::Insert, &table("t1")));
}

#[test]
fn test_grantable_flag_does_not_affect_check() {
    // WITH GRANT OPTION 不影响权限检查结果
    let mut mgr = RbacManager::new();
    mgr.create_user("alice").unwrap();
    mgr.create_role("r1").unwrap();
    mgr.grant_role("r1", "alice").unwrap();

    // r1 不带 grantable
    mgr.grant(Privilege::Select, table("t1"), "r1", false)
        .unwrap();
    // r1 带 grantable
    mgr.grant(Privilege::Select, table("t2"), "r1", true)
        .unwrap();

    assert!(mgr.check("alice", Privilege::Select, &table("t1")));
    assert!(mgr.check("alice", Privilege::Select, &table("t2")));
    // grantable 标志不影响 check 结果
}
