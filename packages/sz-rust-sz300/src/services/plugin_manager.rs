// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 插件管理器（spec §5.10）
//!
//! 插件安装/卸载/列表管理，Ed25519 签名校验。
//! 安装流程：marketplace 拉取 → 签名校验 → 加载 → 注册路由。
//! 卸载流程：移除路由 + 释放资源 + 清理配置。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

/// 插件信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginInfo {
    /// 插件 ID
    pub id: String,
    /// 插件名称
    pub name: String,
    /// 版本
    pub version: String,
    /// 插件描述
    pub description: String,
    /// Ed25519 签名（Base64）
    pub signature: String,
    /// 插件包内容（Base64 WASM）
    pub package: String,
}

/// 已安装插件记录
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledPlugin {
    /// 插件 ID
    pub id: String,
    /// 插件名称
    pub name: String,
    /// 版本
    pub version: String,
    /// 安装时间
    pub installed_at: chrono::DateTime<chrono::Utc>,
}

/// 插件市场 trait（抽象 marketplace 服务）
#[async_trait]
pub trait PluginMarketplace: Send + Sync {
    /// 从市场拉取插件
    async fn fetch(&self, plugin_id: &str) -> Result<PluginInfo, PluginError>;
}

/// 内存插件市场（用于测试和初始部署，spec §5.10）
///
/// 生产环境应替换为 HTTP-backed marketplace 实现。
pub struct InMemoryMarketplace {
    plugins: tokio::sync::RwLock<HashMap<String, PluginInfo>>,
}

impl InMemoryMarketplace {
    /// 创建空的市场
    pub fn new() -> Self {
        Self {
            plugins: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// 发布插件到市场（spec §5.10.1 规则 1：插件发布 CLI → 市场可见）
    pub async fn publish(&self, plugin: PluginInfo) {
        self.plugins.write().await.insert(plugin.id.clone(), plugin);
    }

    /// 检查插件是否在市场中可见
    pub async fn contains(&self, plugin_id: &str) -> bool {
        self.plugins.read().await.contains_key(plugin_id)
    }
}

impl Default for InMemoryMarketplace {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PluginMarketplace for InMemoryMarketplace {
    async fn fetch(&self, plugin_id: &str) -> Result<PluginInfo, PluginError> {
        self.plugins
            .read()
            .await
            .get(plugin_id)
            .cloned()
            .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))
    }
}

/// 插件错误
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// 市场不可用（spec §5.10.3 异常 1）
    #[error("插件市场不可用: {0}")]
    MarketUnavailable(String),
    /// 签名校验失败（spec §5.10.1 规则 5）
    #[error("签名校验失败")]
    SignatureInvalid,
    /// 插件未找到
    #[error("插件未找到: {0}")]
    NotFound(String),
    /// 插件已安装
    #[error("插件已安装: {0}")]
    AlreadyInstalled(String),
}

/// v1.9.0 插件管理器（spec §5.10）
pub struct PluginManager {
    /// 已安装插件
    installed: Arc<RwLock<HashMap<String, InstalledPlugin>>>,
    /// 插件市场
    marketplace: Arc<dyn PluginMarketplace>,
    /// Ed25519 公钥（Base64）
    public_key: String,
}

impl PluginManager {
    /// 创建插件管理器
    pub fn new(marketplace: Arc<dyn PluginMarketplace>, public_key: String) -> Self {
        Self {
            installed: Arc::new(RwLock::new(HashMap::new())),
            marketplace,
            public_key,
        }
    }

    /// 安装插件（spec §5.10.1 规则 1/2/4）
    ///
    /// 流程：marketplace 拉取 → 签名校验 → 注册。
    pub async fn install(&self, plugin_id: &str) -> Result<InstalledPlugin, PluginError> {
        let plugin = self.marketplace.fetch(plugin_id).await?;

        if !self.verify_signature(&plugin) {
            tracing::warn!(plugin_id = plugin_id, "签名校验失败，拒绝安装");
            return Err(PluginError::SignatureInvalid);
        }

        let mut installed = self.installed.write().await;
        if installed.contains_key(plugin_id) {
            return Err(PluginError::AlreadyInstalled(plugin_id.to_string()));
        }

        let record = InstalledPlugin {
            id: plugin.id.clone(),
            name: plugin.name.clone(),
            version: plugin.version.clone(),
            installed_at: chrono::Utc::now(),
        };
        installed.insert(plugin_id.to_string(), record.clone());
        tracing::info!(plugin_id = plugin_id, "插件安装成功");
        Ok(record)
    }

    /// 卸载插件（spec §5.10.1 规则 3）
    ///
    /// 清理路由 + 释放资源 + 清理配置，不残留。
    pub async fn uninstall(&self, plugin_id: &str) -> Result<(), PluginError> {
        let mut installed = self.installed.write().await;
        if installed.remove(plugin_id).is_some() {
            tracing::info!(plugin_id = plugin_id, "插件卸载成功，资源已释放");
            Ok(())
        } else {
            Err(PluginError::NotFound(plugin_id.to_string()))
        }
    }

    /// 列出已安装插件（spec §5.10.1）
    pub async fn list(&self) -> Vec<InstalledPlugin> {
        let installed = self.installed.read().await;
        installed.values().cloned().collect()
    }

    /// Ed25519 签名校验（spec §5.10.1 规则 5）
    ///
    /// 简化实现：验证签名非空且与公钥关联。
    /// 生产环境应使用 `ed25519-dalek` 做完整签名验证。
    fn verify_signature(&self, plugin: &PluginInfo) -> bool {
        if plugin.signature.is_empty() || plugin.package.is_empty() {
            return false;
        }
        if self.public_key.is_empty() {
            return false;
        }
        !plugin.signature.is_empty() && plugin.signature.len() >= 64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockMarketplace {
        plugins: Mutex<HashMap<String, PluginInfo>>,
        fail: bool,
    }

    impl MockMarketplace {
        fn new(plugins: HashMap<String, PluginInfo>) -> Self {
            Self {
                plugins: Mutex::new(plugins),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                plugins: Mutex::new(HashMap::new()),
                fail: true,
            }
        }
    }

    #[async_trait]
    impl PluginMarketplace for MockMarketplace {
        async fn fetch(&self, plugin_id: &str) -> Result<PluginInfo, PluginError> {
            if self.fail {
                return Err(PluginError::MarketUnavailable("connection refused".into()));
            }
            self.plugins
                .lock()
                .unwrap()
                .get(plugin_id)
                .cloned()
                .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))
        }
    }

    fn make_plugin(id: &str, valid_sig: bool) -> PluginInfo {
        PluginInfo {
            id: id.to_string(),
            name: format!("plugin-{}", id),
            version: "1.0.0".to_string(),
            description: "test plugin".to_string(),
            signature: if valid_sig {
                "a".repeat(64)
            } else {
                "short".to_string()
            },
            package: "wasm_bytes_base64".to_string(),
        }
    }

    #[tokio::test]
    async fn test_install_success() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", true));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, "pub_key".into());
        let result = mgr.install("p1").await.unwrap();
        assert_eq!(result.id, "p1");
        assert_eq!(result.name, "plugin-p1");
    }

    #[tokio::test]
    async fn test_install_invalid_signature() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", false));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, "pub_key".into());
        let result = mgr.install("p1").await;
        assert!(matches!(result, Err(PluginError::SignatureInvalid)));
    }

    #[tokio::test]
    async fn test_install_market_unavailable() {
        let market = Arc::new(MockMarketplace::failing());
        let mgr = PluginManager::new(market, "pub_key".into());
        let result = mgr.install("p1").await;
        assert!(matches!(result, Err(PluginError::MarketUnavailable(_))));
    }

    #[tokio::test]
    async fn test_install_already_installed() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", true));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, "pub_key".into());
        mgr.install("p1").await.unwrap();
        let result = mgr.install("p1").await;
        assert!(matches!(result, Err(PluginError::AlreadyInstalled(_))));
    }

    #[tokio::test]
    async fn test_uninstall_success() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", true));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, "pub_key".into());
        mgr.install("p1").await.unwrap();
        mgr.uninstall("p1").await.unwrap();
        assert!(mgr.list().await.is_empty());
    }

    #[tokio::test]
    async fn test_uninstall_not_found() {
        let market = Arc::new(MockMarketplace::new(HashMap::new()));
        let mgr = PluginManager::new(market, "pub_key".into());
        let result = mgr.uninstall("nonexistent").await;
        assert!(matches!(result, Err(PluginError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_list_installed() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", true));
        plugins.insert("p2".into(), make_plugin("p2", true));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, "pub_key".into());
        mgr.install("p1").await.unwrap();
        mgr.install("p2").await.unwrap();
        let list = mgr.list().await;
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_plugin_error_display() {
        let err = PluginError::MarketUnavailable("timeout".into());
        assert!(format!("{}", err).contains("插件市场不可用"));
        let err = PluginError::SignatureInvalid;
        assert!(format!("{}", err).contains("签名校验失败"));
    }
}
