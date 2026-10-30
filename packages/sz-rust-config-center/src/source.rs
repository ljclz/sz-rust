// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! ConfigSource trait + ConfigEntry + GrayRule

use std::collections::HashMap;

use async_trait::async_trait;
use ipnet::IpNet;
use serde_json::Value;

use crate::ConfigCenterError;

/// 配置条目
#[derive(Debug, Clone)]
pub struct ConfigEntry {
    /// 配置键
    pub key: String,
    /// 配置值
    pub value: Value,
    /// 是否敏感字段（加密存储）
    pub is_sensitive: bool,
    /// 配置版本号
    pub version: u64,
    /// 灰度规则（可选）
    pub gray_rule: Option<GrayRule>,
    /// 更新时间戳（Unix 秒）
    pub updated_at: i64,
}

/// 灰度发布规则
#[derive(Debug, Clone)]
pub enum GrayRule {
    /// 按 IP 网段灰度
    ByIp(Vec<IpNet>),
    /// 按租户灰度
    ByTenant(Vec<String>),
    /// 按百分比灰度（0.0 ~ 1.0）
    ByPercent(f64),
}

/// 配置变更事件
#[derive(Debug, Clone)]
pub struct ConfigChange {
    /// 变更的键
    pub key: String,
    /// 新值
    pub new_value: Value,
    /// 新版本号
    pub version: u64,
}

/// 配置源 trait
///
/// 后端实现此 trait 以提供配置拉取、监听、回滚和健康检查能力。
#[async_trait]
pub trait ConfigSource: Send + Sync + 'static {
    /// 拉取全部配置
    async fn fetch_all(&self) -> Result<HashMap<String, ConfigEntry>, ConfigCenterError>;

    /// 监听配置变更（长轮询）
    ///
    /// 返回一个 channel receiver，配置变更时推送 `ConfigChange`。
    async fn watch(
        &self,
        key_prefix: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<ConfigChange>, ConfigCenterError>;

    /// 回滚到指定版本
    async fn rollback(
        &self,
        version: u64,
    ) -> Result<HashMap<String, ConfigEntry>, ConfigCenterError>;

    /// 健康检查
    async fn health_check(&self) -> Result<(), ConfigCenterError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_entry_construction() {
        let entry = ConfigEntry {
            key: "db.host".to_string(),
            value: Value::String("localhost".to_string()),
            is_sensitive: false,
            version: 1,
            gray_rule: None,
            updated_at: 1700000000,
        };
        assert_eq!(entry.key, "db.host");
        assert!(!entry.is_sensitive);
    }

    #[test]
    fn test_gray_rule_by_percent() {
        let rule = GrayRule::ByPercent(0.1);
        assert!(matches!(rule, GrayRule::ByPercent(p) if p == 0.1));
    }

    #[test]
    fn test_gray_rule_by_tenant() {
        let rule = GrayRule::ByTenant(vec!["tenant_a".to_string()]);
        assert!(matches!(rule, GrayRule::ByTenant(_)));
    }
}
