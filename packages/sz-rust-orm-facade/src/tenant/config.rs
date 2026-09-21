// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户配置注册表 — (tenant_id, config_key) 命名空间 + 全局继承 + 内存缓存

use chrono::Utc;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use super::error::TenantError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantConfigType {
    String,
    Number,
    Boolean,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    Tenant,
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantConfig {
    pub tenant_id: i64,
    pub config_key: String,
    pub config_value: String,
    pub config_type: TenantConfigType,
    pub is_global: bool,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TenantConfig {
    pub fn new(
        tenant_id: i64,
        key: impl Into<String>,
        value: impl Into<String>,
        config_type: TenantConfigType,
    ) -> Self {
        Self {
            tenant_id,
            config_key: key.into(),
            config_value: value.into(),
            config_type,
            is_global: tenant_id == 0,
            updated_at: Utc::now(),
        }
    }

    pub fn parse_value(&self) -> Result<serde_json::Value, TenantError> {
        match self.config_type {
            TenantConfigType::String => Ok(serde_json::Value::String(self.config_value.clone())),
            TenantConfigType::Number => self
                .config_value
                .parse::<f64>()
                .map(serde_json::Value::from)
                .map_err(|_| {
                    TenantError::TenantConfigReloadFailed(format!(
                        "invalid number value: {}",
                        self.config_value
                    ))
                }),
            TenantConfigType::Boolean => self
                .config_value
                .parse::<bool>()
                .map(serde_json::Value::from)
                .map_err(|_| {
                    TenantError::TenantConfigReloadFailed(format!(
                        "invalid boolean value: {}",
                        self.config_value
                    ))
                }),
            TenantConfigType::Json => serde_json::from_str(&self.config_value).map_err(|e| {
                TenantError::TenantConfigReloadFailed(format!("invalid json value: {}", e))
            }),
        }
    }
}

pub struct TenantConfigRegistry {
    configs: DashMap<(i64, String), TenantConfig>,
    cache: DashMap<(i64, String), serde_json::Value>,
}

impl TenantConfigRegistry {
    pub fn new() -> Self {
        Self {
            configs: DashMap::new(),
            cache: DashMap::new(),
        }
    }

    pub fn get(&self, tenant_id: i64, key: &str) -> Option<(serde_json::Value, ConfigSource)> {
        let cache_key = (tenant_id, key.to_string());
        if let Some(cached) = self.cache.get(&cache_key) {
            let source = self
                .configs
                .get(&(tenant_id, key.to_string()))
                .map(|c| {
                    if c.is_global {
                        ConfigSource::Global
                    } else {
                        ConfigSource::Tenant
                    }
                })
                .unwrap_or(ConfigSource::Global);
            return Some((cached.clone(), source));
        }

        if let Some(config) = self.configs.get(&(tenant_id, key.to_string())) {
            if !config.is_global {
                let value = config.parse_value().ok()?;
                self.cache.insert(cache_key, value.clone());
                return Some((value, ConfigSource::Tenant));
            }
        }

        if let Some(global_config) = self.configs.get(&(0, key.to_string())) {
            if global_config.is_global {
                let value = global_config.parse_value().ok()?;
                self.cache.insert(cache_key, value.clone());
                return Some((value, ConfigSource::Global));
            }
        }

        None
    }

    pub fn set_tenant_config(
        &self,
        tenant_id: i64,
        key: impl Into<String>,
        value: impl Into<String>,
        config_type: TenantConfigType,
    ) -> Result<TenantConfig, TenantError> {
        let key = key.into();
        self.validate_key(&key)?;
        let config = TenantConfig::new(tenant_id, key.clone(), value, config_type);
        self.configs
            .insert((tenant_id, key.clone()), config.clone());
        self.invalidate_cache(tenant_id, &key);
        Ok(config)
    }

    pub fn set_global_config(
        &self,
        key: impl Into<String>,
        value: impl Into<String>,
        config_type: TenantConfigType,
    ) -> Result<TenantConfig, TenantError> {
        let key = key.into();
        self.validate_key(&key)?;
        let config = TenantConfig::new(0, key.clone(), value, config_type);
        self.configs.insert((0, key.clone()), config.clone());
        self.invalidate_cache(0, &key);
        Ok(config)
    }

    pub fn delete_tenant_config(&self, tenant_id: i64, key: &str) -> Option<TenantConfig> {
        let removed = self
            .configs
            .remove(&(tenant_id, key.to_string()))
            .map(|(_, v)| v);
        self.invalidate_cache(tenant_id, key);
        removed
    }

    pub fn list_tenant_configs(&self, tenant_id: i64) -> Vec<(TenantConfig, ConfigSource)> {
        let mut result = Vec::new();

        for entry in self.configs.iter() {
            if entry.key().0 == tenant_id && !entry.is_global {
                result.push((entry.clone(), ConfigSource::Tenant));
            }
        }

        for entry in self.configs.iter() {
            if entry.key().0 == 0 && entry.is_global {
                let key = entry.key().1.clone();
                if !self.configs.contains_key(&(tenant_id, key.clone())) {
                    result.push((entry.clone(), ConfigSource::Global));
                }
            }
        }

        result
    }

    pub fn list_global_configs(&self) -> Vec<TenantConfig> {
        self.configs
            .iter()
            .filter(|c| c.is_global)
            .map(|c| c.clone())
            .collect()
    }

    pub fn invalidate_cache(&self, tenant_id: i64, key: &str) {
        self.cache.remove(&(tenant_id, key.to_string()));
        if tenant_id == 0 {
            let keys_to_remove: Vec<(i64, String)> = self
                .cache
                .iter()
                .filter(|e| e.key().1 == key)
                .map(|e| e.key().clone())
                .collect();
            for k in keys_to_remove {
                self.cache.remove(&k);
            }
        }
    }

    fn validate_key(&self, key: &str) -> Result<(), TenantError> {
        if key.is_empty() || key.len() > 128 {
            return Err(TenantError::TenantContextRequired);
        }
        for ch in key.chars() {
            if !ch.is_alphanumeric() && ch != '_' && ch != '.' && ch != '-' {
                return Err(TenantError::TenantContextRequired);
            }
        }
        Ok(())
    }
}

impl Default for TenantConfigRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_value_string() {
        let config = TenantConfig::new(1, "name", "acme", TenantConfigType::String);
        let value = config.parse_value().unwrap();
        assert_eq!(value, serde_json::Value::String("acme".to_string()));
    }

    #[test]
    fn test_parse_value_number() {
        let config = TenantConfig::new(1, "rate", "2.5", TenantConfigType::Number);
        let value = config.parse_value().unwrap();
        assert_eq!(value, serde_json::json!(2.5));
    }

    #[test]
    fn test_parse_value_boolean() {
        let config = TenantConfig::new(1, "enabled", "true", TenantConfigType::Boolean);
        let value = config.parse_value().unwrap();
        assert_eq!(value, serde_json::Value::Bool(true));
    }

    #[test]
    fn test_parse_value_json() {
        let config = TenantConfig::new(1, "settings", r#"{"key":"val"}"#, TenantConfigType::Json);
        let value = config.parse_value().unwrap();
        assert_eq!(value["key"], "val");
    }

    #[test]
    fn test_get_tenant_specific_overrides_global() {
        let registry = TenantConfigRegistry::new();
        registry
            .set_global_config("theme", "dark", TenantConfigType::String)
            .unwrap();
        registry
            .set_tenant_config(1, "theme", "light", TenantConfigType::String)
            .unwrap();

        let (value, source) = registry.get(1, "theme").unwrap();
        assert_eq!(value, serde_json::Value::String("light".to_string()));
        assert_eq!(source, ConfigSource::Tenant);
    }

    #[test]
    fn test_get_global_inheritance() {
        let registry = TenantConfigRegistry::new();
        registry
            .set_global_config("theme", "dark", TenantConfigType::String)
            .unwrap();

        let (value, source) = registry.get(1, "theme").unwrap();
        assert_eq!(value, serde_json::Value::String("dark".to_string()));
        assert_eq!(source, ConfigSource::Global);
    }

    #[test]
    fn test_tenant_isolation() {
        let registry = TenantConfigRegistry::new();
        registry
            .set_tenant_config(1, "secret", "tenant1-value", TenantConfigType::String)
            .unwrap();

        assert!(registry.get(2, "secret").is_none());
    }

    #[test]
    fn test_cache_invalidation_on_update() {
        let registry = TenantConfigRegistry::new();
        registry
            .set_tenant_config(1, "theme", "dark", TenantConfigType::String)
            .unwrap();
        let (value, _) = registry.get(1, "theme").unwrap();
        assert_eq!(value, serde_json::Value::String("dark".to_string()));

        registry
            .set_tenant_config(1, "theme", "light", TenantConfigType::String)
            .unwrap();
        let (value, _) = registry.get(1, "theme").unwrap();
        assert_eq!(value, serde_json::Value::String("light".to_string()));
    }

    #[test]
    fn test_delete_falls_back_to_global() {
        let registry = TenantConfigRegistry::new();
        registry
            .set_global_config("theme", "dark", TenantConfigType::String)
            .unwrap();
        registry
            .set_tenant_config(1, "theme", "light", TenantConfigType::String)
            .unwrap();

        let (_, source) = registry.get(1, "theme").unwrap();
        assert_eq!(source, ConfigSource::Tenant);

        registry.delete_tenant_config(1, "theme");

        let (value, source) = registry.get(1, "theme").unwrap();
        assert_eq!(value, serde_json::Value::String("dark".to_string()));
        assert_eq!(source, ConfigSource::Global);
    }

    #[test]
    fn test_list_tenant_configs_with_source() {
        let registry = TenantConfigRegistry::new();
        registry
            .set_global_config("theme", "dark", TenantConfigType::String)
            .unwrap();
        registry
            .set_tenant_config(1, "theme", "light", TenantConfigType::String)
            .unwrap();
        registry
            .set_tenant_config(1, "logo", "custom.png", TenantConfigType::String)
            .unwrap();

        let configs = registry.list_tenant_configs(1);
        assert_eq!(configs.len(), 2);

        let tenant_configs: Vec<_> = configs
            .iter()
            .filter(|(_, source)| *source == ConfigSource::Tenant)
            .collect();
        assert_eq!(tenant_configs.len(), 2);
    }
}
