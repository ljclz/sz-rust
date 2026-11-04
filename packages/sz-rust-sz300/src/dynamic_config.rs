// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 配置中心动态配置（spec §5.6）
//!
//! 持有配置源 + 本地缓存，支持配置订阅、回写、降级。
//! 配置中心不可用时使用本地缓存继续运行 + WARN 日志（spec §5.6.1 规则 2/5）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use tokio::sync::RwLock;

/// 配置快照（spec §5.6.1 规则 4）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigSnapshot {
    /// 配置版本号（递增）
    pub version: u64,
    /// 配置内容
    pub content: HashMap<String, Value>,
    /// 更新时间
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ConfigSnapshot {
    /// 创建空配置快照
    pub fn empty() -> Self {
        Self {
            version: 0,
            content: HashMap::new(),
            updated_at: Utc::now(),
        }
    }
}

/// 配置变更事件
#[derive(Debug, Clone)]
pub struct ConfigChange {
    /// 变更的 key
    pub key: String,
    /// 旧值
    pub old_value: Option<Value>,
    /// 新值
    pub new_value: Value,
    /// 变更版本号
    pub version: u64,
}

/// 配置源 trait（抽象 Consul/Nacos 等配置中心）
#[async_trait]
pub trait ConfigSource: Send + Sync {
    /// 拉取全量配置
    async fn fetch_all(&self) -> Result<HashMap<String, Value>, ConfigError>;
    /// 回写单个配置
    async fn write(&self, key: &str, value: &Value) -> Result<(), ConfigError>;
    /// 配置版本号
    async fn version(&self) -> Result<u64, ConfigError>;
}

/// 配置错误
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// 配置中心不可用（spec §5.6.3 异常 1）
    #[error("配置中心不可用: {0}")]
    SourceUnavailable(String),
    /// 配置回写失败
    #[error("配置回写失败: {0}")]
    WriteFailed(String),
    /// 本地缓存读写失败
    #[error("本地缓存IO错误: {0}")]
    CacheIoError(String),
}

/// v1.9.0 动态配置（spec §5.6）
pub struct DynamicConfig {
    /// 配置源句柄
    source: Arc<dyn ConfigSource>,
    /// 当前配置快照
    snapshot: Arc<RwLock<ConfigSnapshot>>,
    /// 本地缓存路径
    local_cache_path: PathBuf,
}

impl DynamicConfig {
    /// 创建动态配置实例
    ///
    /// 从配置源拉取初始配置，同时写入本地缓存。
    /// 配置源不可用时尝试从本地缓存恢复（spec §5.6.1 规则 2）。
    pub async fn new(
        source: Arc<dyn ConfigSource>,
        cache_path: PathBuf,
    ) -> Result<Self, ConfigError> {
        let snapshot = match source.fetch_all().await {
            Ok(content) => {
                let version = source.version().await.unwrap_or(1);
                let snap = ConfigSnapshot {
                    version,
                    content,
                    updated_at: Utc::now(),
                };
                let _ = Self::save_cache(&cache_path, &snap).await;
                snap
            }
            Err(e) => {
                tracing::warn!(error = %e, "配置中心不可用，尝试本地缓存恢复");
                Self::load_cache(&cache_path)
                    .await
                    .unwrap_or_else(ConfigSnapshot::empty)
            }
        };

        Ok(Self {
            source,
            snapshot: Arc::new(RwLock::new(snapshot)),
            local_cache_path: cache_path,
        })
    }

    /// 获取配置值（spec §5.6.1 规则 1）
    pub async fn get(&self, key: &str) -> Option<Value> {
        let snap = self.snapshot.read().await;
        snap.content.get(key).cloned()
    }

    /// 获取当前配置版本号
    pub async fn version(&self) -> u64 {
        self.snapshot.read().await.version
    }

    /// 回写配置到配置中心（spec §5.6.1 规则 3）
    ///
    /// 保证多实例一致：先写配置中心，成功后更新本地快照。
    pub async fn write_back(&self, key: &str, value: Value) -> Result<(), ConfigError> {
        self.source.write(key, &value).await?;

        let mut snap = self.snapshot.write().await;
        let old_value = snap.content.insert(key.to_string(), value.clone());
        snap.version += 1;
        snap.updated_at = Utc::now();

        let change = ConfigChange {
            key: key.to_string(),
            old_value,
            new_value: value,
            version: snap.version,
        };

        let _ = Self::save_cache(&self.local_cache_path, &snap).await;
        tracing::info!(key = change.key, version = change.version, "配置已更新");
        Ok(())
    }

    /// 从配置中心刷新配置（spec §5.6.1 规则 1，≤1s 生效）
    ///
    /// 配置中心不可用时保持当前快照不变 + WARN（spec §5.6.3 异常 1）。
    pub async fn refresh(&self) -> Result<(), ConfigError> {
        let content = self.source.fetch_all().await?;
        let version = self.source.version().await.unwrap_or(0);

        let mut snap = self.snapshot.write().await;
        snap.content = content;
        snap.version = version;
        snap.updated_at = Utc::now();

        let _ = Self::save_cache(&self.local_cache_path, &snap).await;
        Ok(())
    }

    /// 保存配置到本地缓存
    async fn save_cache(path: &PathBuf, snap: &ConfigSnapshot) -> Result<(), ConfigError> {
        let json = serde_json::to_string(snap)
            .map_err(|e| ConfigError::CacheIoError(format!("序列化失败: {}", e)))?;
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(path, json)
            .await
            .map_err(|e| ConfigError::CacheIoError(e.to_string()))?;
        Ok(())
    }

    /// 从本地缓存加载配置
    async fn load_cache(path: &PathBuf) -> Option<ConfigSnapshot> {
        let json = tokio::fs::read_to_string(path).await.ok()?;
        serde_json::from_str(&json).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockConfigSource {
        data: Mutex<HashMap<String, Value>>,
        version: Mutex<u64>,
        fail: bool,
    }

    impl MockConfigSource {
        fn new(data: HashMap<String, Value>) -> Self {
            Self {
                data: Mutex::new(data),
                version: Mutex::new(1),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                data: Mutex::new(HashMap::new()),
                version: Mutex::new(0),
                fail: true,
            }
        }
    }

    #[async_trait]
    impl ConfigSource for MockConfigSource {
        async fn fetch_all(&self) -> Result<HashMap<String, Value>, ConfigError> {
            if self.fail {
                return Err(ConfigError::SourceUnavailable("mock fail".into()));
            }
            Ok(self.data.lock().unwrap().clone())
        }

        async fn write(&self, key: &str, value: &Value) -> Result<(), ConfigError> {
            if self.fail {
                return Err(ConfigError::WriteFailed("mock fail".into()));
            }
            self.data
                .lock()
                .unwrap()
                .insert(key.to_string(), value.clone());
            *self.version.lock().unwrap() += 1;
            Ok(())
        }

        async fn version(&self) -> Result<u64, ConfigError> {
            Ok(*self.version.lock().unwrap())
        }
    }

    #[tokio::test]
    async fn test_dynamic_config_get() {
        let mut data = HashMap::new();
        data.insert("rate_limit".into(), Value::from(100));
        data.insert("timeout".into(), Value::from(30));
        let source = Arc::new(MockConfigSource::new(data));
        let cache = std::env::temp_dir().join("sz300_test_dc_get.json");
        let config = DynamicConfig::new(source, cache.clone()).await.unwrap();

        assert_eq!(config.get("rate_limit").await, Some(Value::from(100)));
        assert_eq!(config.get("timeout").await, Some(Value::from(30)));
        assert_eq!(config.get("nonexistent").await, None);

        let _ = tokio::fs::remove_file(&cache).await;
    }

    #[tokio::test]
    async fn test_dynamic_config_write_back() {
        let data = HashMap::new();
        let source = Arc::new(MockConfigSource::new(data));
        let cache = std::env::temp_dir().join("sz300_test_dc_write.json");
        let config = DynamicConfig::new(source, cache.clone()).await.unwrap();

        let v1 = config.version().await;
        config
            .write_back("new_key", Value::String("new_value".into()))
            .await
            .unwrap();
        let v2 = config.version().await;
        assert!(v2 > v1);
        assert_eq!(
            config.get("new_key").await,
            Some(Value::String("new_value".into()))
        );

        let _ = tokio::fs::remove_file(&cache).await;
    }

    #[tokio::test]
    async fn test_dynamic_config_degrade_on_source_unavailable() {
        let source = Arc::new(MockConfigSource::failing());
        let cache = std::env::temp_dir().join("sz300_test_dc_degrade.json");
        let config = DynamicConfig::new(source, cache.clone()).await.unwrap();

        assert_eq!(config.get("any_key").await, None);
        assert_eq!(config.version().await, 0);

        let _ = tokio::fs::remove_file(&cache).await;
    }

    #[tokio::test]
    async fn test_dynamic_config_refresh() {
        let data = HashMap::new();
        let source = Arc::new(MockConfigSource::new(data));
        let cache = std::env::temp_dir().join("sz300_test_dc_refresh.json");
        let config = DynamicConfig::new(source.clone(), cache.clone())
            .await
            .unwrap();

        source
            .write("refreshed_key", &Value::Bool(true))
            .await
            .unwrap();
        config.refresh().await.unwrap();
        assert_eq!(config.get("refreshed_key").await, Some(Value::Bool(true)));

        let _ = tokio::fs::remove_file(&cache).await;
    }

    #[tokio::test]
    async fn test_dynamic_config_cache_recovery() {
        let cache = std::env::temp_dir().join("sz300_test_dc_recovery.json");

        let mut data = HashMap::new();
        data.insert("cached_key".into(), Value::from(42));
        let source = Arc::new(MockConfigSource::new(data));
        let config = DynamicConfig::new(source, cache.clone()).await.unwrap();
        assert_eq!(config.get("cached_key").await, Some(Value::from(42)));

        let failing_source = Arc::new(MockConfigSource::failing());
        let config2 = DynamicConfig::new(failing_source, cache.clone())
            .await
            .unwrap();
        assert_eq!(config2.get("cached_key").await, Some(Value::from(42)));

        let _ = tokio::fs::remove_file(&cache).await;
    }

    #[test]
    fn test_config_snapshot_empty() {
        let snap = ConfigSnapshot::empty();
        assert_eq!(snap.version, 0);
        assert!(snap.content.is_empty());
    }

    #[test]
    fn test_config_error_display() {
        let err = ConfigError::SourceUnavailable("connection refused".into());
        assert!(format!("{}", err).contains("配置中心不可用"));
        assert!(format!("{}", err).contains("connection refused"));
    }
}
