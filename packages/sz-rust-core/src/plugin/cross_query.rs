//! 跨插件查询 — 通过 CapabilityRegistry 调用其他插件能力。
//!
//! 对应 design.md §2.2.2 接口 9。
//! 自动注入 `tenant_id` 过滤，拒绝跨租户查询。

use serde_json::Value;

/// 跨插件查询错误。
#[derive(Debug, thiserror::Error)]
pub enum CrossQueryError {
    #[error("权限不足：租户 {tenant_id} 无权查询")]
    PermissionDenied { tenant_id: i64 },
    #[error("能力未找到: {0}")]
    NotFound(String),
    #[error("查询失败: {0}")]
    QueryFailed(String),
}

/// 跨插件查询。
///
/// 通过 `CapabilityRegistry::call_with_tenant` 调用其他插件能力，
/// 自动注入 `tenant_id` 实现租户隔离。
pub struct CrossQuery {
    tenant_id: i64,
}

impl CrossQuery {
    /// 创建指定租户的跨插件查询实例。
    pub fn new(tenant_id: i64) -> Self {
        Self { tenant_id }
    }

    /// 返回当前租户 ID。
    pub fn tenant_id(&self) -> i64 {
        self.tenant_id
    }

    /// 构建带 tenant_id 的查询参数。
    ///
    /// 自动将 `tenant_id` 注入到查询参数中，确保租户隔离。
    pub fn inject_tenant_filter(&self, mut args: Value) -> Value {
        if let Some(obj) = args.as_object_mut() {
            obj.insert("tenant_id".to_string(), Value::from(self.tenant_id));
        } else if args.is_null() {
            args = serde_json::json!({ "tenant_id": self.tenant_id });
        }
        args
    }

    /// 验证目标租户与当前租户一致。
    ///
    /// 拒绝跨租户查询，返回 `PermissionDenied`。
    pub fn verify_tenant(&self, target_tenant_id: i64) -> Result<(), CrossQueryError> {
        if self.tenant_id != target_tenant_id {
            return Err(CrossQueryError::PermissionDenied {
                tenant_id: self.tenant_id,
            });
        }
        Ok(())
    }

    /// 构建聚合查询参数（批量查询多个能力）。
    ///
    /// `queries` 为 `(capability_name, args)` 元组列表，
    /// 返回批量查询的 JSON 参数。
    pub fn aggregate(&self, queries: &[(&str, Value)]) -> Value {
        let queries_json: Vec<Value> = queries
            .iter()
            .map(|(name, args)| {
                let injected = self.inject_tenant_filter(args.clone());
                serde_json::json!({
                    "capability": name,
                    "args": injected,
                })
            })
            .collect();
        serde_json::json!({
            "tenant_id": self.tenant_id,
            "queries": queries_json,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inject_tenant_filter() {
        let cq = CrossQuery::new(100);
        let args = serde_json::json!({"keyword": "test"});
        let result = cq.inject_tenant_filter(args);
        assert_eq!(result["tenant_id"], 100);
        assert_eq!(result["keyword"], "test");
    }

    #[test]
    fn test_inject_tenant_filter_null() {
        let cq = CrossQuery::new(200);
        let result = cq.inject_tenant_filter(Value::Null);
        assert_eq!(result["tenant_id"], 200);
    }

    #[test]
    fn test_verify_tenant_same() {
        let cq = CrossQuery::new(100);
        assert!(cq.verify_tenant(100).is_ok());
    }

    #[test]
    fn test_verify_tenant_different() {
        let cq = CrossQuery::new(100);
        assert!(cq.verify_tenant(200).is_err());
    }

    #[test]
    fn test_aggregate() {
        let cq = CrossQuery::new(100);
        let queries = vec![
            ("plugin_a.search", serde_json::json!({"q": "hello"})),
            ("plugin_b.list", serde_json::json!({})),
        ];
        let result = cq.aggregate(&queries);
        assert_eq!(result["tenant_id"], 100);
        assert_eq!(result["queries"].as_array().unwrap().len(), 2);
        assert_eq!(result["queries"][0]["args"]["tenant_id"], 100);
    }
}
