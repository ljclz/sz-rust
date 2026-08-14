use async_trait::async_trait;
use serde_json::Value;

use crate::error::{CapError, CapResult};

/// 权限检查器 trait，在能力调用前执行授权决策。
///
/// 实现方根据 `cap_name`、`args`、`tenant_id` 判断是否放行。
/// 所有实现必须 `Send + Sync + 'static` 以支持并发调用。
#[async_trait]
pub trait PermissionChecker: Send + Sync + 'static {
    /// 检查指定租户是否有权调用该能力。
    ///
    /// 返回 `Ok(())` 表示放行，返回 `Err(CapError::PermissionDenied(_))` 表示拒绝。
    async fn check(&self, cap_name: &str, args: &Value, tenant_id: i64) -> CapResult<()>;
}

/// 默认放行检查器，用于测试和未配置权限的场景。
pub struct AllowAll;

#[async_trait]
impl PermissionChecker for AllowAll {
    async fn check(&self, _cap_name: &str, _args: &Value, _tenant_id: i64) -> CapResult<()> {
        Ok(())
    }
}

/// 基于租户范围的权限检查器。
///
/// 维护一张"能力名 → 允许的租户 ID 集合"映射，
/// 仅当调用方 `tenant_id` 在允许集合中时放行。
pub struct TenantScopeChecker {
    allowed: parking_lot::RwLock<std::collections::HashMap<String, std::collections::HashSet<i64>>>,
}

impl TenantScopeChecker {
    pub fn new() -> Self {
        Self {
            allowed: parking_lot::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// 授权指定租户调用指定能力。
    pub fn grant(&self, cap_name: &str, tenant_id: i64) {
        let mut map = self.allowed.write();
        map.entry(cap_name.to_string())
            .or_default()
            .insert(tenant_id);
    }

    /// 撤销指定租户对指定能力的调用权限。
    pub fn revoke(&self, cap_name: &str, tenant_id: i64) {
        let mut map = self.allowed.write();
        if let Some(set) = map.get_mut(cap_name) {
            set.remove(&tenant_id);
        }
    }
}

impl Default for TenantScopeChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PermissionChecker for TenantScopeChecker {
    async fn check(&self, cap_name: &str, _args: &Value, tenant_id: i64) -> CapResult<()> {
        let map = self.allowed.read();
        match map.get(cap_name) {
            Some(set) if set.contains(&tenant_id) => Ok(()),
            _ => Err(CapError::PermissionDenied(format!(
                "租户 {tenant_id} 无权调用能力 {cap_name}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_allow_all() {
        let checker = AllowAll;
        assert!(checker.check("any.cap", &Value::Null, 1).await.is_ok());
    }

    #[tokio::test]
    async fn test_tenant_scope_grant_revoke() {
        let checker = TenantScopeChecker::new();
        checker.grant("cap.a", 100);
        assert!(checker.check("cap.a", &Value::Null, 100).await.is_ok());
        assert!(checker.check("cap.a", &Value::Null, 200).await.is_err());
        checker.revoke("cap.a", 100);
        assert!(checker.check("cap.a", &Value::Null, 100).await.is_err());
    }
}
