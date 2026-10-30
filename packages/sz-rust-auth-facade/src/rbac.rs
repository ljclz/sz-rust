// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! RBAC 细粒度权限 — 角色继承 + 资源粒度 + 数据范围（spec §5.15）

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 角色标识
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoleId(pub String);

impl RoleId {
    /// 创建角色 ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// 资源标识（资源类型 + 资源 ID + 操作，spec §6.15 规则 4）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Resource {
    /// 资源类型
    pub resource_type: String,
    /// 资源 ID
    pub resource_id: String,
    /// 操作
    pub operation: String,
}

impl Resource {
    /// 创建资源标识
    pub fn new(
        resource_type: impl Into<String>,
        resource_id: impl Into<String>,
        operation: impl Into<String>,
    ) -> Self {
        Self {
            resource_type: resource_type.into(),
            resource_id: resource_id.into(),
            operation: operation.into(),
        }
    }
}

/// 数据范围（spec §6.15 规则 2）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataScope {
    /// 全部数据
    All,
    /// 本部门数据
    Department,
    /// 个人数据
    Personal,
}

/// 权限决策结果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PermissionDecision {
    /// 允许
    Allow,
    /// 拒绝（spec §5.15 规则 5：默认拒绝）
    Deny,
}

/// RBAC 错误
#[derive(Debug, Clone, thiserror::Error)]
pub enum RbacError {
    /// 角色继承循环（spec §5.15 异常 2）
    #[error("角色继承循环: {0}")]
    InheritanceCycle(String),
    /// 继承深度超限
    #[error("继承深度超限: {0}")]
    DepthExceeded(usize),
}

/// 权限缓存 trait
#[async_trait]
pub trait PermissionCache: Send + Sync {
    /// 查询缓存
    async fn get(&self, user_id: &str, resource: &Resource) -> Option<PermissionDecision>;
    /// 写入缓存
    async fn set(&self, user_id: &str, resource: &Resource, decision: PermissionDecision);
    /// 刷新缓存（权限变更时，spec §5.15 规则 4）
    async fn invalidate(&self, user_id: &str);
}

/// 无缓存实现（每次都查实际权限）
pub struct NoCache;

#[async_trait]
impl PermissionCache for NoCache {
    async fn get(&self, _user_id: &str, _resource: &Resource) -> Option<PermissionDecision> {
        None
    }
    async fn set(&self, _user_id: &str, _resource: &Resource, _decision: PermissionDecision) {}
    async fn invalidate(&self, _user_id: &str) {}
}

/// RBAC 引擎
pub struct RbacEngine {
    /// 角色继承关系（子角色 → 父角色集合）
    role_inheritance: RwLock<HashMap<RoleId, HashSet<RoleId>>>,
    /// 角色 → 权限
    role_permissions: RwLock<HashMap<RoleId, HashSet<Resource>>>,
    /// 用户 → 角色
    user_roles: RwLock<HashMap<String, HashSet<RoleId>>>,
    /// 继承深度上限（spec §6.15 规则 1）
    max_depth: usize,
}

impl RbacEngine {
    /// 创建 RBAC 引擎
    pub fn new(max_depth: usize) -> Self {
        Self {
            role_inheritance: RwLock::new(HashMap::new()),
            role_permissions: RwLock::new(HashMap::new()),
            user_roles: RwLock::new(HashMap::new()),
            max_depth,
        }
    }

    /// 添加角色继承（子角色继承父角色权限）
    pub fn add_inheritance(&self, child: RoleId, parent: RoleId) {
        self.role_inheritance
            .write()
            .entry(child)
            .or_default()
            .insert(parent);
    }

    /// 授予角色权限
    pub fn grant_permission(&self, role: RoleId, resource: Resource) {
        self.role_permissions
            .write()
            .entry(role)
            .or_default()
            .insert(resource);
    }

    /// 撤销角色权限
    pub fn revoke_permission(&self, role: &RoleId, resource: &Resource) {
        if let Some(perms) = self.role_permissions.write().get_mut(role) {
            perms.remove(resource);
        }
    }

    /// 分配用户角色
    pub fn assign_role(&self, user_id: impl Into<String>, role: RoleId) {
        self.user_roles
            .write()
            .entry(user_id.into())
            .or_default()
            .insert(role);
    }

    /// 解析角色继承树（BFS + 环检测 + 深度限制）
    pub fn resolve_inheritance(&self, role: &RoleId) -> Result<HashSet<RoleId>, RbacError> {
        let inheritance = self.role_inheritance.read();
        let mut result = HashSet::new();
        result.insert(role.clone());

        let mut queue = vec![role.clone()];
        let mut depth = 0;

        while !queue.is_empty() {
            if depth > self.max_depth {
                return Err(RbacError::DepthExceeded(self.max_depth));
            }
            let mut next_queue = Vec::new();
            for current in &queue {
                if let Some(parents) = inheritance.get(current) {
                    for parent in parents {
                        if result.contains(parent) {
                            return Err(RbacError::InheritanceCycle(parent.0.clone()));
                        }
                        result.insert(parent.clone());
                        next_queue.push(parent.clone());
                    }
                }
            }
            queue = next_queue;
            depth += 1;
        }

        Ok(result)
    }

    /// 校验权限（spec §5.15 规则 5：默认拒绝）
    pub async fn check(
        &self,
        user_id: &str,
        resource: &Resource,
    ) -> Result<PermissionDecision, RbacError> {
        let user_roles = self.user_roles.read();
        let roles = user_roles.get(user_id).cloned().unwrap_or_default();
        drop(user_roles);

        let role_perms = self.role_permissions.read();

        for role in &roles {
            let resolved = self.resolve_inheritance(role)?;
            for r in &resolved {
                if let Some(perms) = role_perms.get(r) {
                    if perms.contains(resource) {
                        return Ok(PermissionDecision::Allow);
                    }
                }
            }
        }

        Ok(PermissionDecision::Deny)
    }

    /// 获取用户所有角色
    pub fn get_user_roles(&self, user_id: &str) -> HashSet<RoleId> {
        self.user_roles
            .read()
            .get(user_id)
            .cloned()
            .unwrap_or_default()
    }

    /// 获取角色所有权限（含继承）
    pub fn get_effective_permissions(&self, role: &RoleId) -> Result<HashSet<Resource>, RbacError> {
        let resolved = self.resolve_inheritance(role)?;
        let role_perms = self.role_permissions.read();
        let mut result = HashSet::new();
        for r in &resolved {
            if let Some(perms) = role_perms.get(r) {
                result.extend(perms.iter().cloned());
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_resource(op: &str) -> Resource {
        Resource::new("article", "1", op)
    }

    #[tokio::test]
    async fn test_check_allow() {
        let engine = RbacEngine::new(10);
        let role = RoleId::new("editor");
        engine.grant_permission(role.clone(), make_resource("edit"));
        engine.assign_role("user1", role);
        let decision = engine.check("user1", &make_resource("edit")).await.unwrap();
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[tokio::test]
    async fn test_check_deny_no_role() {
        let engine = RbacEngine::new(10);
        let decision = engine.check("user1", &make_resource("edit")).await.unwrap();
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[tokio::test]
    async fn test_check_deny_no_permission() {
        let engine = RbacEngine::new(10);
        let role = RoleId::new("viewer");
        engine.grant_permission(role.clone(), make_resource("read"));
        engine.assign_role("user1", role);
        let decision = engine.check("user1", &make_resource("edit")).await.unwrap();
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[tokio::test]
    async fn test_inheritance() {
        let engine = RbacEngine::new(10);
        let admin = RoleId::new("admin");
        let editor = RoleId::new("editor");
        engine.grant_permission(admin.clone(), make_resource("delete"));
        engine.add_inheritance(editor.clone(), admin);
        engine.assign_role("user1", editor);
        let decision = engine
            .check("user1", &make_resource("delete"))
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[tokio::test]
    async fn test_inheritance_cycle() {
        let engine = RbacEngine::new(10);
        let a = RoleId::new("a");
        let b = RoleId::new("b");
        engine.add_inheritance(a.clone(), b.clone());
        engine.add_inheritance(b, a.clone());
        let result = engine.resolve_inheritance(&a);
        assert!(matches!(result, Err(RbacError::InheritanceCycle(_))));
    }

    #[tokio::test]
    async fn test_inheritance_depth_exceeded() {
        let engine = RbacEngine::new(2);
        let r1 = RoleId::new("r1");
        let r2 = RoleId::new("r2");
        let r3 = RoleId::new("r3");
        let r4 = RoleId::new("r4");
        engine.add_inheritance(r1.clone(), r2.clone());
        engine.add_inheritance(r2, r3.clone());
        engine.add_inheritance(r3, r4);
        let result = engine.resolve_inheritance(&r1);
        assert!(matches!(result, Err(RbacError::DepthExceeded(2))));
    }

    #[tokio::test]
    async fn test_revoke_permission() {
        let engine = RbacEngine::new(10);
        let role = RoleId::new("editor");
        let resource = make_resource("edit");
        engine.grant_permission(role.clone(), resource.clone());
        engine.assign_role("user1", role.clone());
        assert_eq!(
            engine.check("user1", &resource).await.unwrap(),
            PermissionDecision::Allow
        );
        engine.revoke_permission(&role, &resource);
        assert_eq!(
            engine.check("user1", &resource).await.unwrap(),
            PermissionDecision::Deny
        );
    }

    #[tokio::test]
    async fn test_multi_role() {
        let engine = RbacEngine::new(10);
        let reader = RoleId::new("reader");
        let writer = RoleId::new("writer");
        engine.grant_permission(reader, make_resource("read"));
        engine.grant_permission(writer, make_resource("write"));
        engine.assign_role("user1", RoleId::new("reader"));
        engine.assign_role("user1", RoleId::new("writer"));
        assert_eq!(
            engine.check("user1", &make_resource("read")).await.unwrap(),
            PermissionDecision::Allow
        );
        assert_eq!(
            engine
                .check("user1", &make_resource("write"))
                .await
                .unwrap(),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn test_get_effective_permissions() {
        let engine = RbacEngine::new(10);
        let admin = RoleId::new("admin");
        let editor = RoleId::new("editor");
        engine.grant_permission(admin.clone(), make_resource("delete"));
        engine.grant_permission(editor.clone(), make_resource("edit"));
        engine.add_inheritance(editor.clone(), admin);
        let perms = engine.get_effective_permissions(&editor).unwrap();
        assert_eq!(perms.len(), 2);
    }
}
