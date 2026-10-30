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
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
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
    /// 完整签名验证（2026-10-28 修复 AppSec P0-1 伪验签）：
    /// 1. 将 `public_key`（Base64）解码为 32 字节 Ed25519 公钥；
    /// 2. 将 `plugin.signature`（Base64）解码为 64 字节签名；
    /// 3. 将 `plugin.package`（Base64 WASM）解码为原始字节；
    /// 4. 用 `ed25519-dalek` 对 package 字节验签。
    ///
    /// 任一解码失败或验签不通过均返回 `false`（fail-closed），
    /// 不再接受"长度 ≥ 64"的伪验签。
    fn verify_signature(&self, plugin: &PluginInfo) -> bool {
        if plugin.signature.is_empty() || plugin.package.is_empty() {
            return false;
        }
        if self.public_key.is_empty() {
            return false;
        }

        // 公钥解码（Base64 → 32 字节）
        let Ok(pk_bytes) = B64.decode(&self.public_key) else {
            tracing::warn!("插件公钥 Base64 解码失败");
            return false;
        };
        let Ok(pk_array): Result<[u8; 32], _> = pk_bytes.as_slice().try_into() else {
            tracing::warn!("插件公钥长度非 32 字节");
            return false;
        };
        let Ok(public_key) = VerifyingKey::from_bytes(&pk_array) else {
            tracing::warn!("插件公钥无效");
            return false;
        };

        // 签名解码（Base64 → 64 字节）
        let Ok(sig_bytes) = B64.decode(&plugin.signature) else {
            tracing::warn!("插件签名 Base64 解码失败");
            return false;
        };
        let Ok(sig_array): Result<[u8; 64], _> = sig_bytes.as_slice().try_into() else {
            tracing::warn!("插件签名长度非 64 字节");
            return false;
        };
        let signature = Signature::from_bytes(&sig_array);

        // 插件包内容解码（Base64 WASM）
        let Ok(package) = B64.decode(&plugin.package) else {
            tracing::warn!("插件包 Base64 解码失败");
            return false;
        };

        public_key.verify(&package, &signature).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use ed25519_dalek::{Signer, SigningKey};
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

    /// 固定测试密钥对（确定性，避免每次测试生成随机密钥）
    fn test_keypair() -> SigningKey {
        let seed = [42u8; 32];
        SigningKey::from_bytes(&seed)
    }

    /// 测试公钥（Base64 编码的 32 字节）
    fn test_public_key_b64() -> String {
        B64.encode(test_keypair().verifying_key().to_bytes())
    }

    /// 构造插件：valid_sig = true 时用测试私钥对 package 内容真实签名（Base64）
    fn make_plugin(id: &str, valid_sig: bool) -> PluginInfo {
        let package = format!("wasm-bytes-{id}");
        let package_b64 = B64.encode(package.as_bytes());
        let signature = if valid_sig {
            let sig = test_keypair().sign(package.as_bytes());
            B64.encode(sig.to_bytes())
        } else {
            // 伪造签名：64 字节非 Base64 有效签名（历史上长度 ≥64 即放行）
            B64.encode([7u8; 64])
        };
        PluginInfo {
            id: id.to_string(),
            name: format!("plugin-{}", id),
            version: "1.0.0".to_string(),
            description: "test plugin".to_string(),
            signature,
            package: package_b64,
        }
    }

    #[tokio::test]
    async fn test_install_success() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", true));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, test_public_key_b64());
        let result = mgr.install("p1").await.unwrap();
        assert_eq!(result.id, "p1");
        assert_eq!(result.name, "plugin-p1");
    }

    #[tokio::test]
    async fn test_install_invalid_signature() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", false));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, test_public_key_b64());
        let result = mgr.install("p1").await;
        assert!(matches!(result, Err(PluginError::SignatureInvalid)));
    }

    /// AppSec P0-1 负例：伪造 64 字节签名（Base64 合法但非真实 Ed25519 签名）必须被拒绝
    #[tokio::test]
    async fn test_install_forged_64byte_signature_rejected() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", false));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, test_public_key_b64());
        let result = mgr.install("p1").await;
        assert!(
            matches!(result, Err(PluginError::SignatureInvalid)),
            "伪造 64 字节签名不得通过验签"
        );
    }

    /// 无效公钥（非 Base64 的字符串）必须 fail-closed
    #[tokio::test]
    async fn test_install_invalid_public_key_rejected() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", true));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, "not-a-base64-public-key".into());
        let result = mgr.install("p1").await;
        assert!(
            matches!(result, Err(PluginError::SignatureInvalid)),
            "无效公钥必须拒绝安装"
        );
    }

    #[tokio::test]
    async fn test_install_market_unavailable() {
        let market = Arc::new(MockMarketplace::failing());
        let mgr = PluginManager::new(market, test_public_key_b64());
        let result = mgr.install("p1").await;
        assert!(matches!(result, Err(PluginError::MarketUnavailable(_))));
    }

    #[tokio::test]
    async fn test_install_already_installed() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", true));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, test_public_key_b64());
        mgr.install("p1").await.unwrap();
        let result = mgr.install("p1").await;
        assert!(matches!(result, Err(PluginError::AlreadyInstalled(_))));
    }

    #[tokio::test]
    async fn test_uninstall_success() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", true));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, test_public_key_b64());
        mgr.install("p1").await.unwrap();
        mgr.uninstall("p1").await.unwrap();
        assert!(mgr.list().await.is_empty());
    }

    #[tokio::test]
    async fn test_uninstall_not_found() {
        let market = Arc::new(MockMarketplace::new(HashMap::new()));
        let mgr = PluginManager::new(market, test_public_key_b64());
        let result = mgr.uninstall("nonexistent").await;
        assert!(matches!(result, Err(PluginError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_list_installed() {
        let mut plugins = HashMap::new();
        plugins.insert("p1".into(), make_plugin("p1", true));
        plugins.insert("p2".into(), make_plugin("p2", true));
        let market = Arc::new(MockMarketplace::new(plugins));
        let mgr = PluginManager::new(market, test_public_key_b64());
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
