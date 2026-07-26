//! RBAC 权限系统 — Phase 3.12.
//!
//! # 设计
//!
//! - **`Privilege`** — SQL 权限类型（SELECT / INSERT / UPDATE / DELETE / TRUNCATE /
//!   REFERENCES / TRIGGER / CREATE / CONNECT / TEMPORARY / EXECUTE / USAGE / ALL）
//! - **`DatabaseObject`** — 权限作用对象（TABLE / DATABASE / SCHEMA / SEQUENCE /
//!   FUNCTION / COLUMN）
//! - **`User`** — 用户（含 `is_superuser` 标志 + 所属角色列表）
//! - **`Role`** — 角色（含成员列表）
//! - **`Grant`** — 单条授权（privilege + object + grantable）
//! - **`RbacManager`** — 权限管理器（用户/角色/授权 CRUD + 权限检查）
//!
//! # 权限检查规则
//!
//! 1. **SUPERUSER 直接放行**：`is_superuser=true` 的用户通过所有权限检查（覆盖所有规则）
//! 2. **角色继承**：用户继承其所有角色的权限（含 PUBLIC 角色，所有用户自动拥有）
//! 3. **ALL PRIVILEGES 通配**：授予 `Privilege::All` 时，后续任何具体权限检查都通过
//! 4. **TABLE 覆盖 COLUMN**：表级权限覆盖列级权限（如 GRANT SELECT ON t 隐含 SELECT(t, any_col)）
//! 5. **大小写不敏感**：用户名/角色名 lowercase 存储，大小写不敏感比较
//!
//! # 对应 `SzRSQL实施进度.md` Phase 3.12

use std::collections::HashMap;

use szrsql_sql::ast::TableName;
use thiserror::Error;

// =====================================================================
//  权限类型
// =====================================================================

/// SQL 权限类型 — 对应 PG GRANT/REVOKE 语法
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Privilege {
    /// SELECT — 查询权限
    Select,
    /// INSERT — 插入权限
    Insert,
    /// UPDATE — 更新权限
    Update,
    /// DELETE — 删除权限
    Delete,
    /// TRUNCATE — 清空表权限
    Truncate,
    /// REFERENCES — 外键引用权限
    References,
    /// TRIGGER — 创建触发器权限
    Trigger,
    /// CREATE — 创建对象权限（数据库/Schema 内）
    Create,
    /// CONNECT — 连接数据库权限
    Connect,
    /// TEMPORARY — 创建临时表权限
    Temporary,
    /// EXECUTE — 执行函数权限
    Execute,
    /// USAGE — 使用权限（Schema/Sequence/语言等）
    Usage,
    /// ALL PRIVILEGES — 所有权限（通配）
    All,
}

impl Privilege {
    /// 是否为通配权限（ALL）
    pub fn is_all(&self) -> bool {
        matches!(self, Privilege::All)
    }

    /// ALL 是否涵盖指定具体权限
    pub fn covers(&self, other: Privilege) -> bool {
        self.is_all() || *self == other
    }
}

// =====================================================================
//  数据库对象
// =====================================================================

/// 权限作用对象 — PG GRANT/REVOKE 的 ON 子句
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DatabaseObject {
    /// 表（含视图、物化视图）
    Table(TableName),
    /// 数据库
    Database(String),
    /// Schema
    Schema(String),
    /// 序列
    Sequence(String),
    /// 函数/存储过程
    Function(String),
    /// 列级对象（GRANT SELECT (col) ON t）
    Column { table: TableName, column: String },
}

impl DatabaseObject {
    /// 是否为表级对象
    pub fn is_table(&self) -> bool {
        matches!(self, DatabaseObject::Table(_))
    }

    /// 是否为列级对象
    pub fn is_column(&self) -> bool {
        matches!(self, DatabaseObject::Column { .. })
    }

    /// 列级对象所属的表名（None 表示非列级对象）
    pub fn parent_table(&self) -> Option<&TableName> {
        match self {
            DatabaseObject::Column { table, .. } => Some(table),
            _ => None,
        }
    }

    /// 判断两个对象在权限检查时是否匹配（asymmetric）
    ///
    /// - 完全相等 → true
    /// - 表级对象匹配列级对象（同表）→ true（GRANT SELECT ON t 隐含 SELECT(t.col)）
    /// - 表级对象匹配表级对象（同表，大小写不敏感）→ true
    /// - **列级对象不匹配表级对象**（GRANT SELECT(id) ON t 不隐含 SELECT * FROM t）→ false
    /// - 其他 → false
    pub fn matches(&self, other: &DatabaseObject) -> bool {
        if self == other {
            return true;
        }
        match (self, other) {
            // 表级 → 列级：GRANT SELECT ON t 隐含 SELECT(t.col)
            (DatabaseObject::Table(t1), DatabaseObject::Column { table: t2, .. }) => {
                table_eq(t1, t2)
            }
            // 表级 → 表级：大小写不敏感比较（GRANT SELECT ON t 应匹配 SELECT ON T）
            (DatabaseObject::Table(t1), DatabaseObject::Table(t2)) => table_eq(t1, t2),
            // 注意：列级 → 表级 不匹配（不对称），避免 GRANT(id) ON t 隐含 SELECT * FROM t
            _ => false,
        }
    }
}

/// 表名大小写不敏感比较
fn table_eq(a: &TableName, b: &TableName) -> bool {
    a.name.eq_ignore_ascii_case(&b.name)
        && a.schema.as_deref().map(|s| s.to_lowercase())
            == b.schema.as_deref().map(|s| s.to_lowercase())
}

// =====================================================================
//  User / Role / Grant
// =====================================================================

/// 用户
#[derive(Debug, Clone)]
pub struct User {
    /// 用户名（原始大小写）
    pub name: String,
    /// 是否为超级用户（绕过所有权限检查）
    pub is_superuser: bool,
    /// 所属角色名列表（lowercase）
    pub roles: Vec<String>,
}

impl User {
    /// 创建普通用户
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_superuser: false,
            roles: vec![PUBLIC_ROLE.to_string()],
        }
    }

    /// 创建超级用户
    pub fn new_superuser(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_superuser: true,
            roles: vec![PUBLIC_ROLE.to_string()],
        }
    }
}

/// 角色
#[derive(Debug, Clone)]
pub struct Role {
    /// 角色名（原始大小写）
    pub name: String,
    /// 成员用户名列表（lowercase）
    pub members: Vec<String>,
}

impl Role {
    /// 创建角色
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            members: Vec::new(),
        }
    }
}

/// 单条授权
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// 授权的权限
    pub privilege: Privilege,
    /// 作用对象
    pub object: DatabaseObject,
    /// 是否带 WITH GRANT OPTION（允许被授权者再授权给他人）
    pub grantable: bool,
}

/// PUBLIC 角色名 — 所有用户自动拥有
pub const PUBLIC_ROLE: &str = "public";

// =====================================================================
//  错误类型
// =====================================================================

/// RBAC 操作错误
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RbacError {
    /// 用户不存在
    #[error("user not found: {0}")]
    UserNotFound(String),
    /// 角色不存在
    #[error("role not found: {0}")]
    RoleNotFound(String),
    /// 用户已存在
    #[error("user already exists: {0}")]
    UserAlreadyExists(String),
    /// 角色已存在
    #[error("role already exists: {0}")]
    RoleAlreadyExists(String),
    /// 授权不存在（REVOKE 时未找到对应 GRANT）
    #[error("grant not found: {privilege:?} on {object:?} for role {role}")]
    GrantNotFound {
        privilege: Privilege,
        object: DatabaseObject,
        role: String,
    },
    /// 用户已属于该角色
    #[error("user {user} already belongs to role {role}")]
    UserAlreadyInRole { user: String, role: String },
    /// 用户不属于该角色
    #[error("user {user} does not belong to role {role}")]
    UserNotInRole { user: String, role: String },
}

// =====================================================================
//  RbacManager
// =====================================================================

/// RBAC 权限管理器 — 用户/角色/授权 CRUD + 权限检查
///
/// # 内部存储
///
/// - `users: HashMap<String, User>` — 用户名（lowercase）→ User
/// - `roles: HashMap<String, Role>` — 角色名（lowercase）→ Role
/// - `grants: HashMap<String, Vec<Grant>>` — 角色名（lowercase）→ 该角色的所有授权
///
/// # 默认角色
///
/// - `public` — 所有用户自动拥有此角色，用于授予所有用户共享的权限
#[derive(Debug, Default, Clone)]
pub struct RbacManager {
    users: HashMap<String, User>,
    roles: HashMap<String, Role>,
    grants: HashMap<String, Vec<Grant>>,
}

impl RbacManager {
    /// 创建空 RBAC 管理器（自动注册 PUBLIC 角色）
    pub fn new() -> Self {
        let mut mgr = Self::default();
        mgr.roles
            .insert(PUBLIC_ROLE.to_string(), Role::new(PUBLIC_ROLE));
        mgr.grants.insert(PUBLIC_ROLE.to_string(), Vec::new());
        mgr
    }

    // -----------------------------------------------------------------
    //  用户管理
    // -----------------------------------------------------------------

    /// 创建普通用户
    pub fn create_user(&mut self, name: &str) -> Result<(), RbacError> {
        self.create_user_inner(name, false)
    }

    /// 创建超级用户
    pub fn create_superuser(&mut self, name: &str) -> Result<(), RbacError> {
        self.create_user_inner(name, true)
    }

    fn create_user_inner(&mut self, name: &str, is_superuser: bool) -> Result<(), RbacError> {
        let key = name.to_lowercase();
        if self.users.contains_key(&key) {
            return Err(RbacError::UserAlreadyExists(name.to_string()));
        }
        let mut user = if is_superuser {
            User::new_superuser(name)
        } else {
            User::new(name)
        };
        // 确保拥有 PUBLIC 角色
        if !user.roles.iter().any(|r| r == PUBLIC_ROLE) {
            user.roles.push(PUBLIC_ROLE.to_string());
        }
        self.users.insert(key.clone(), user);
        // 将用户加入 PUBLIC 角色的成员列表
        if let Some(public_role) = self.roles.get_mut(PUBLIC_ROLE) {
            if !public_role.members.contains(&key) {
                public_role.members.push(key);
            }
        }
        Ok(())
    }

    /// 删除用户
    pub fn drop_user(&mut self, name: &str) -> Result<(), RbacError> {
        let key = name.to_lowercase();
        let user = self
            .users
            .remove(&key)
            .ok_or_else(|| RbacError::UserNotFound(name.to_string()))?;
        // 从所有角色的成员列表中移除
        for role_name in &user.roles {
            if let Some(role) = self.roles.get_mut(role_name) {
                role.members.retain(|m| m != &key);
            }
        }
        Ok(())
    }

    /// 查询用户
    pub fn get_user(&self, name: &str) -> Option<&User> {
        self.users.get(&name.to_lowercase())
    }

    /// 设置/取消超级用户
    pub fn set_superuser(&mut self, name: &str, is_superuser: bool) -> Result<(), RbacError> {
        let key = name.to_lowercase();
        let user = self
            .users
            .get_mut(&key)
            .ok_or_else(|| RbacError::UserNotFound(name.to_string()))?;
        user.is_superuser = is_superuser;
        Ok(())
    }

    /// 是否为超级用户
    pub fn is_superuser(&self, name: &str) -> bool {
        self.users
            .get(&name.to_lowercase())
            .map(|u| u.is_superuser)
            .unwrap_or(false)
    }

    /// 用户数量
    pub fn user_count(&self) -> usize {
        self.users.len()
    }

    // -----------------------------------------------------------------
    //  角色管理
    // -----------------------------------------------------------------

    /// 创建角色
    pub fn create_role(&mut self, name: &str) -> Result<(), RbacError> {
        let key = name.to_lowercase();
        if self.roles.contains_key(&key) {
            return Err(RbacError::RoleAlreadyExists(name.to_string()));
        }
        self.roles.insert(key.clone(), Role::new(name));
        self.grants.insert(key, Vec::new());
        Ok(())
    }

    /// 删除角色
    pub fn drop_role(&mut self, name: &str) -> Result<(), RbacError> {
        let key = name.to_lowercase();
        if key == PUBLIC_ROLE {
            return Err(RbacError::RoleNotFound(format!(
                "cannot drop built-in role: {name}"
            )));
        }
        let role = self
            .roles
            .remove(&key)
            .ok_or_else(|| RbacError::RoleNotFound(name.to_string()))?;
        self.grants.remove(&key);
        // 从所有用户的角色列表中移除
        for member in &role.members {
            if let Some(user) = self.users.get_mut(member) {
                user.roles.retain(|r| r != &key);
            }
        }
        Ok(())
    }

    /// 查询角色
    pub fn get_role(&self, name: &str) -> Option<&Role> {
        self.roles.get(&name.to_lowercase())
    }

    /// 角色数量
    pub fn role_count(&self) -> usize {
        self.roles.len()
    }

    /// 将角色授予用户（用户加入角色）
    pub fn grant_role(&mut self, role: &str, user: &str) -> Result<(), RbacError> {
        let role_key = role.to_lowercase();
        let user_key = user.to_lowercase();
        // 验证角色存在
        if !self.roles.contains_key(&role_key) {
            return Err(RbacError::RoleNotFound(role.to_string()));
        }
        // 验证用户存在
        let user_obj = self
            .users
            .get_mut(&user_key)
            .ok_or_else(|| RbacError::UserNotFound(user.to_string()))?;
        // 检查是否已属于该角色
        if user_obj.roles.contains(&role_key) {
            return Err(RbacError::UserAlreadyInRole {
                user: user.to_string(),
                role: role.to_string(),
            });
        }
        user_obj.roles.push(role_key.clone());
        // 添加到角色成员列表
        if let Some(role_obj) = self.roles.get_mut(&role_key) {
            if !role_obj.members.contains(&user_key) {
                role_obj.members.push(user_key);
            }
        }
        Ok(())
    }

    /// 撤销用户的角色
    pub fn revoke_role(&mut self, role: &str, user: &str) -> Result<(), RbacError> {
        let role_key = role.to_lowercase();
        let user_key = user.to_lowercase();
        if !self.roles.contains_key(&role_key) {
            return Err(RbacError::RoleNotFound(role.to_string()));
        }
        let user_obj = self
            .users
            .get_mut(&user_key)
            .ok_or_else(|| RbacError::UserNotFound(user.to_string()))?;
        if !user_obj.roles.contains(&role_key) {
            return Err(RbacError::UserNotInRole {
                user: user.to_string(),
                role: role.to_string(),
            });
        }
        user_obj.roles.retain(|r| r != &role_key);
        if let Some(role_obj) = self.roles.get_mut(&role_key) {
            role_obj.members.retain(|m| m != &user_key);
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    //  授权管理（GRANT / REVOKE）
    // -----------------------------------------------------------------

    /// 授予角色对对象的权限
    ///
    /// - `grantable=true` 表示带 WITH GRANT OPTION（被授权者可再授权）
    /// - 如果已存在相同的 (privilege, object) 授权，更新 grantable 标志
    pub fn grant(
        &mut self,
        privilege: Privilege,
        object: DatabaseObject,
        role: &str,
        grantable: bool,
    ) -> Result<(), RbacError> {
        let role_key = role.to_lowercase();
        if !self.roles.contains_key(&role_key) {
            return Err(RbacError::RoleNotFound(role.to_string()));
        }
        let grants = self.grants.entry(role_key).or_default();
        // 查找已有授权
        if let Some(existing) = grants
            .iter_mut()
            .find(|g| g.privilege == privilege && g.object == object)
        {
            existing.grantable = grantable;
        } else {
            grants.push(Grant {
                privilege,
                object,
                grantable,
            });
        }
        Ok(())
    }

    /// 撤销角色对对象的权限
    pub fn revoke(
        &mut self,
        privilege: Privilege,
        object: &DatabaseObject,
        role: &str,
    ) -> Result<(), RbacError> {
        let role_key = role.to_lowercase();
        if !self.roles.contains_key(&role_key) {
            return Err(RbacError::RoleNotFound(role.to_string()));
        }
        let grants = self
            .grants
            .get_mut(&role_key)
            .expect("grants entry must exist when role exists");
        let before = grants.len();
        grants.retain(|g| !(g.privilege == privilege && &g.object == object));
        if grants.len() == before {
            return Err(RbacError::GrantNotFound {
                privilege,
                object: object.clone(),
                role: role.to_string(),
            });
        }
        Ok(())
    }

    /// 列出角色的所有授权
    pub fn list_grants(&self, role: &str) -> Result<&[Grant], RbacError> {
        let role_key = role.to_lowercase();
        if !self.roles.contains_key(&role_key) {
            return Err(RbacError::RoleNotFound(role.to_string()));
        }
        Ok(self
            .grants
            .get(&role_key)
            .map(|v| v.as_slice())
            .unwrap_or(&[]))
    }

    // -----------------------------------------------------------------
    //  权限检查
    // -----------------------------------------------------------------

    /// 检查用户是否拥有对对象的具体权限
    ///
    /// # 规则
    ///
    /// 1. 用户不存在 → false
    /// 2. 用户是超级用户 → true
    /// 3. 遍历用户的所有角色（含 PUBLIC）：
    ///    - 对每个角色的每条 grant：
    ///      - grant.object.matches(object) 且 (grant.privilege.covers(privilege) 或 grant.privilege.is_all())
    ///      - → 返回 true
    /// 4. 所有角色所有 grant 都不匹配 → false
    pub fn check(&self, user: &str, privilege: Privilege, object: &DatabaseObject) -> bool {
        let user_key = user.to_lowercase();
        let user_obj = match self.users.get(&user_key) {
            Some(u) => u,
            None => return false,
        };
        // 规则 1: 超级用户
        if user_obj.is_superuser {
            return true;
        }
        // 规则 2-3: 遍历角色
        for role_name in &user_obj.roles {
            if let Some(grants) = self.grants.get(role_name) {
                for grant in grants {
                    if grant.object.matches(object)
                        && (grant.privilege.is_all() || grant.privilege == privilege)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// 检查用户是否拥有对表的所有指定权限
    pub fn check_all(&self, user: &str, privileges: &[Privilege], object: &DatabaseObject) -> bool {
        privileges.iter().all(|p| self.check(user, *p, object))
    }

    /// 检查用户是否拥有对表的任意一个权限
    pub fn check_any(&self, user: &str, privileges: &[Privilege], object: &DatabaseObject) -> bool {
        privileges.iter().any(|p| self.check(user, *p, object))
    }

    /// 列出用户拥有的所有角色（含 PUBLIC）
    pub fn user_roles(&self, user: &str) -> Vec<String> {
        self.users
            .get(&user.to_lowercase())
            .map(|u| u.roles.clone())
            .unwrap_or_default()
    }

    /// 列出角色的所有成员
    pub fn role_members(&self, role: &str) -> Vec<String> {
        self.roles
            .get(&role.to_lowercase())
            .map(|r| r.members.clone())
            .unwrap_or_default()
    }
}
