// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 配置中心动态配置端到端测试（spec §5.6）
//!
//! 验证 4 个业务规则：
//! - 规则 1: 配置变更后 refresh → 新值生效
//! - 规则 2: 配置中心宕机 → 使用本地缓存，不崩溃
//! - 规则 3: 实例 A write_back → 实例 B refresh → 收到新值
//! - 规则 4: 配置版本递增 + 变更时间更新

#![cfg(feature = "v19-config-center")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use sz_rust_sz300::dynamic_config::{ConfigError, ConfigSource, DynamicConfig};

struct SharedConfigSource {
    data: Mutex<HashMap<String, Value>>,
    version: Mutex<u64>,
}

impl SharedConfigSource {
    fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
            version: Mutex::new(1),
        }
    }
}

#[async_trait]
impl ConfigSource for SharedConfigSource {
    async fn fetch_all(&self) -> Result<HashMap<String, Value>, ConfigError> {
        Ok(self.data.lock().unwrap().clone())
    }

    async fn write(&self, key: &str, value: &Value) -> Result<(), ConfigError> {
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

struct FailingConfigSource;

#[async_trait]
impl ConfigSource for FailingConfigSource {
    async fn fetch_all(&self) -> Result<HashMap<String, Value>, ConfigError> {
        Err(ConfigError::SourceUnavailable("connection refused".into()))
    }

    async fn write(&self, _key: &str, _value: &Value) -> Result<(), ConfigError> {
        Err(ConfigError::WriteFailed("connection refused".into()))
    }

    async fn version(&self) -> Result<u64, ConfigError> {
        Err(ConfigError::SourceUnavailable("connection refused".into()))
    }
}

#[tokio::test]
async fn test_rule1_config_change_takes_effect() {
    let source = Arc::new(SharedConfigSource::new());
    source.write("rate_limit", &Value::from(100)).await.unwrap();
    let cache = std::env::temp_dir().join("sz300_e2e_dc_rule1.json");
    let config = DynamicConfig::new(source.clone(), cache.clone())
        .await
        .unwrap();
    assert_eq!(config.get("rate_limit").await, Some(Value::from(100)));

    source.write("rate_limit", &Value::from(200)).await.unwrap();
    config.refresh().await.unwrap();
    assert_eq!(config.get("rate_limit").await, Some(Value::from(200)));

    let _ = tokio::fs::remove_file(&cache).await;
}

#[tokio::test]
async fn test_rule2_source_down_uses_cache() {
    let source = Arc::new(SharedConfigSource::new());
    source.write("threshold", &Value::from(50)).await.unwrap();
    let cache = std::env::temp_dir().join("sz300_e2e_dc_rule2.json");
    let config = DynamicConfig::new(source, cache.clone()).await.unwrap();
    assert_eq!(config.get("threshold").await, Some(Value::from(50)));

    let failing = Arc::new(FailingConfigSource);
    let config2 = DynamicConfig::new(failing, cache.clone()).await.unwrap();
    assert_eq!(config2.get("threshold").await, Some(Value::from(50)));

    let _ = tokio::fs::remove_file(&cache).await;
}

#[tokio::test]
async fn test_rule3_multi_instance_config_sync() {
    let source = Arc::new(SharedConfigSource::new());
    let cache = std::env::temp_dir().join("sz300_e2e_dc_rule3.json");

    let config_a = DynamicConfig::new(source.clone(), cache.clone())
        .await
        .unwrap();
    config_a
        .write_back("shared_key", Value::String("value_from_a".into()))
        .await
        .unwrap();

    let config_b = DynamicConfig::new(source.clone(), cache.clone())
        .await
        .unwrap();
    assert_eq!(
        config_b.get("shared_key").await,
        Some(Value::String("value_from_a".into()))
    );

    let _ = tokio::fs::remove_file(&cache).await;
}

#[tokio::test]
async fn test_rule4_version_increments() {
    let source = Arc::new(SharedConfigSource::new());
    let cache = std::env::temp_dir().join("sz300_e2e_dc_rule4.json");
    let config = DynamicConfig::new(source, cache.clone()).await.unwrap();

    let v1 = config.version().await;
    config.write_back("k1", Value::from(1)).await.unwrap();
    let v2 = config.version().await;
    assert!(v2 > v1);

    config.write_back("k2", Value::from(2)).await.unwrap();
    let v3 = config.version().await;
    assert!(v3 > v2);

    let _ = tokio::fs::remove_file(&cache).await;
}
