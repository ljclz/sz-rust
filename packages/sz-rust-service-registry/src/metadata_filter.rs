// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 元数据过滤路由（P3-2）
//!
//! 按版本/区域/标签过滤服务实例。

use std::collections::HashMap;

use crate::registry::{InstanceStatus, ServiceInstance};

/// 元数据过滤器
///
/// 按指定标签键值对过滤实例，仅返回健康且匹配所有标签的实例。
pub struct MetadataFilter;

impl MetadataFilter {
    /// 创建元数据过滤器
    pub fn new() -> Self {
        Self
    }

    /// 过滤实例
    ///
    /// - 仅返回健康实例（`status == Healthy`）
    /// - 实例的 `metadata` 必须包含 `tags` 中所有键值对
    /// - `tags` 为空时返回所有健康实例
    pub fn filter(
        &self,
        instances: &[ServiceInstance],
        tags: &HashMap<String, String>,
    ) -> Vec<ServiceInstance> {
        instances
            .iter()
            .filter(|i| i.status == InstanceStatus::Healthy)
            .filter(|i| tags.iter().all(|(k, v)| i.metadata.get(k) == Some(v)))
            .cloned()
            .collect()
    }
}

impl Default for MetadataFilter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_instance(id: &str, metadata: Vec<(&str, &str)>) -> ServiceInstance {
        let mut inst = ServiceInstance::new("svc", "127.0.0.1", 8080);
        inst.instance_id = id.to_string();
        for (k, v) in metadata {
            inst.metadata.insert(k.to_string(), v.to_string());
        }
        inst
    }

    #[test]
    fn test_filter_empty_tags_returns_all_healthy() {
        let instances = vec![
            make_instance("a", vec![("version", "1")]),
            make_instance("b", vec![("version", "2")]),
        ];
        let filter = MetadataFilter::new();
        let result = filter.filter(&instances, &HashMap::new());
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_filter_by_version() {
        let instances = vec![
            make_instance("a", vec![("version", "1")]),
            make_instance("b", vec![("version", "2")]),
            make_instance("c", vec![("version", "1")]),
        ];
        let mut tags = HashMap::new();
        tags.insert("version".to_string(), "1".to_string());

        let filter = MetadataFilter::new();
        let result = filter.filter(&instances, &tags);
        assert_eq!(result.len(), 2);
        assert!(result
            .iter()
            .all(|i| i.metadata.get("version") == Some(&"1".to_string())));
    }

    #[test]
    fn test_filter_by_multiple_tags() {
        let instances = vec![
            make_instance("a", vec![("version", "1"), ("region", "us")]),
            make_instance("b", vec![("version", "1"), ("region", "eu")]),
            make_instance("c", vec![("version", "2"), ("region", "us")]),
        ];
        let mut tags = HashMap::new();
        tags.insert("version".to_string(), "1".to_string());
        tags.insert("region".to_string(), "us".to_string());

        let filter = MetadataFilter::new();
        let result = filter.filter(&instances, &tags);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].instance_id, "a");
    }

    #[test]
    fn test_filter_excludes_unhealthy() {
        let mut inst = make_instance("a", vec![("version", "1")]);
        inst.status = InstanceStatus::Unhealthy;
        let instances = vec![inst];
        let mut tags = HashMap::new();
        tags.insert("version".to_string(), "1".to_string());

        let filter = MetadataFilter::new();
        let result = filter.filter(&instances, &tags);
        assert!(result.is_empty(), "unhealthy instance should be excluded");
    }

    #[test]
    fn test_filter_no_match() {
        let instances = vec![make_instance("a", vec![("version", "1")])];
        let mut tags = HashMap::new();
        tags.insert("version".to_string(), "99".to_string());

        let filter = MetadataFilter::new();
        let result = filter.filter(&instances, &tags);
        assert!(result.is_empty());
    }

    #[test]
    fn test_filter_instance_without_metadata() {
        let instances = vec![ServiceInstance::new("svc", "127.0.0.1", 8080)];
        let mut tags = HashMap::new();
        tags.insert("version".to_string(), "1".to_string());

        let filter = MetadataFilter::new();
        let result = filter.filter(&instances, &tags);
        assert!(
            result.is_empty(),
            "instance without matching metadata excluded"
        );
    }
}
