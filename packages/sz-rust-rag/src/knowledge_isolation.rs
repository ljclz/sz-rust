//! 知识库隔离（v1.5.0 P2-3）
//!
//! 按 `knowledge_base_id` + `tenant_id` 隔离过滤（spec 5.7.7 + 5.7.10）。
//! 知识库不存在 → 空列表 + 告警（spec 5.7.3 异常3）。
#![forbid(unsafe_code)]

use std::collections::HashSet;

/// 知识库隔离过滤器
pub struct KnowledgeIsolation {
    /// 已注册的知识库 ID 集合
    registered_kbases: HashSet<String>,
}

/// 隔离过滤结果
#[derive(Debug, Clone)]
pub struct IsolationResult<T> {
    /// 过滤后的结果
    pub items: Vec<T>,
    /// 知识库是否不存在
    pub kbase_not_found: bool,
}

impl KnowledgeIsolation {
    /// 创建隔离过滤器，注册已知知识库
    pub fn new(registered_kbases: Vec<String>) -> Self {
        Self {
            registered_kbases: registered_kbases.into_iter().collect(),
        }
    }

    /// 按知识库 + 租户过滤结果（spec 5.7.7 + 5.7.10）
    ///
    /// `knowledge_base_id` 不存在 → 返回空列表 + `kbase_not_found = true`
    pub fn filter<T>(
        &self,
        items: Vec<T>,
        knowledge_base_id: &str,
        tenant_id: &str,
        item_kbase: impl Fn(&T) -> &str,
        item_tenant: impl Fn(&T) -> &str,
    ) -> IsolationResult<T> {
        if !self.registered_kbases.contains(knowledge_base_id) {
            tracing::warn!(knowledge_base_id, tenant_id, "知识库不存在，返回空列表");
            return IsolationResult {
                items: Vec::new(),
                kbase_not_found: true,
            };
        }

        let filtered: Vec<T> = items
            .into_iter()
            .filter(|item| item_kbase(item) == knowledge_base_id && item_tenant(item) == tenant_id)
            .collect();

        IsolationResult {
            items: filtered,
            kbase_not_found: false,
        }
    }

    /// 注册新知识库
    pub fn register(&mut self, kbase_id: &str) {
        self.registered_kbases.insert(kbase_id.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct TestItem {
        kbase: String,
        tenant: String,
        content: String,
    }

    #[test]
    fn test_filter_by_kbase_and_tenant() {
        let isolation = KnowledgeIsolation::new(vec!["kb_a".to_string(), "kb_b".to_string()]);
        let items = vec![
            TestItem {
                kbase: "kb_a".into(),
                tenant: "t1".into(),
                content: "a1".into(),
            },
            TestItem {
                kbase: "kb_a".into(),
                tenant: "t2".into(),
                content: "a2".into(),
            },
            TestItem {
                kbase: "kb_b".into(),
                tenant: "t1".into(),
                content: "b1".into(),
            },
        ];

        let result = isolation.filter(items, "kb_a", "t1", |i| &i.kbase, |i| &i.tenant);

        assert!(!result.kbase_not_found);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].content, "a1");
    }

    #[test]
    fn test_kbase_not_found() {
        let isolation = KnowledgeIsolation::new(vec!["kb_a".to_string()]);
        let items = vec![TestItem {
            kbase: "kb_a".into(),
            tenant: "t1".into(),
            content: "a1".into(),
        }];

        let result = isolation.filter(items, "kb_unknown", "t1", |i| &i.kbase, |i| &i.tenant);

        assert!(result.kbase_not_found, "知识库不存在应标记");
        assert!(result.items.is_empty(), "应返回空列表");
    }

    #[test]
    fn test_register_new_kbase() {
        let mut isolation = KnowledgeIsolation::new(vec![]);
        assert!(!isolation.registered_kbases.contains("kb_new"));

        isolation.register("kb_new");
        let items = vec![TestItem {
            kbase: "kb_new".into(),
            tenant: "t1".into(),
            content: "x".into(),
        }];
        let result = isolation.filter(items, "kb_new", "t1", |i| &i.kbase, |i| &i.tenant);
        assert!(!result.kbase_not_found);
        assert_eq!(result.items.len(), 1);
    }
}
