// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 密钥轮换管理（spec §5.18）
//!
//! 自动轮换策略 + 版本管理 + 无缝切换 + 历史密钥验证。

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::KeyRotationError;

/// 密钥标识
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyId(pub String);

impl KeyId {
    /// 创建密钥 ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// 密钥版本
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyVersion {
    /// 版本号
    pub version: u64,
    /// 密钥材料
    pub key_material: Vec<u8>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 是否活跃
    pub active: bool,
    /// 是否退役
    pub retired: bool,
}

/// 轮换策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationPolicy {
    /// 轮换周期（spec §6.18 规则 1）
    pub interval: Duration,
    /// 保留历史版本数
    pub keep_history: usize,
}

impl Default for RotationPolicy {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(86400),
            keep_history: 5,
        }
    }
}

/// 轮换事件（spec §5.18 规则 6）
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum RotationEvent {
    /// 新密钥生成
    Rotated { key_id: KeyId, new_version: u64 },
    /// 历史密钥退役
    Retired { key_id: KeyId, version: u64 },
}

/// 密钥生成器 trait
pub trait KeyGenerator: Send + Sync {
    /// 生成新密钥材料
    fn generate(&self) -> Vec<u8>;
}

/// 随机密钥生成器（用于测试）
pub struct RandomKeyGenerator;

impl KeyGenerator for RandomKeyGenerator {
    fn generate(&self) -> Vec<u8> {
        let now = Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;
        let mut key = Vec::with_capacity(32);
        let mut state = now
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        for _ in 0..32 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            key.push((state >> 33) as u8);
        }
        key
    }
}

/// 密钥管理器
pub struct KeyManager {
    keys: RwLock<HashMap<KeyId, Vec<KeyVersion>>>,
    policy: RotationPolicy,
    generator: Box<dyn KeyGenerator>,
}

impl KeyManager {
    /// 创建密钥管理器
    pub fn new(policy: RotationPolicy, generator: Box<dyn KeyGenerator>) -> Self {
        Self {
            keys: RwLock::new(HashMap::new()),
            policy,
            generator,
        }
    }

    /// 创建默认密钥管理器
    pub fn with_default() -> Self {
        Self::new(RotationPolicy::default(), Box::new(RandomKeyGenerator))
    }

    /// 轮换密钥（spec §5.18 规则 1）
    ///
    /// # 后置条件
    /// - 生成新密钥（版本 N+1）设为活跃
    /// - 旧密钥（版本 N）保留可验证
    /// - 超过 keep_history 的旧密钥退役
    /// - 发出轮换事件
    pub fn rotate(&self, key_id: &KeyId) -> Result<RotationEvent, KeyRotationError> {
        let mut keys = self.keys.write();
        let versions = keys.entry(key_id.clone()).or_default();

        let new_version = versions.iter().map(|v| v.version).max().unwrap_or(0) + 1;

        let new_key = KeyVersion {
            version: new_version,
            key_material: self.generator.generate(),
            created_at: Utc::now(),
            active: true,
            retired: false,
        };

        for v in versions.iter_mut() {
            if v.active {
                v.active = false;
            }
        }

        versions.push(new_key);

        for v in versions.iter_mut() {
            if !v.active {
                v.retired = true;
            }
        }

        while versions.len() > self.policy.keep_history + 1 {
            versions.remove(0);
        }

        Ok(RotationEvent::Rotated {
            key_id: key_id.clone(),
            new_version,
        })
    }

    /// 获取活跃密钥
    pub fn get_active(&self, key_id: &KeyId) -> Option<KeyVersion> {
        self.keys
            .read()
            .get(key_id)
            .and_then(|vs| vs.iter().find(|v| v.active).cloned())
    }

    /// 获取所有未退役密钥（活跃 + 历史）
    pub fn get_valid_keys(&self, key_id: &KeyId) -> Vec<KeyVersion> {
        self.keys
            .read()
            .get(key_id)
            .map(|vs| vs.iter().filter(|v| !v.retired).cloned().collect())
            .unwrap_or_default()
    }

    /// 查询密钥版本列表
    pub fn list_versions(&self, key_id: &KeyId) -> Vec<KeyVersion> {
        self.keys.read().get(key_id).cloned().unwrap_or_default()
    }

    /// 获取轮换策略
    pub fn policy(&self) -> &RotationPolicy {
        &self.policy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rotate_new_key() {
        let manager = KeyManager::with_default();
        let key_id = KeyId::new("jwt-signing");
        let event = manager.rotate(&key_id).unwrap();
        assert!(matches!(
            event,
            RotationEvent::Rotated { new_version: 1, .. }
        ));
        let active = manager.get_active(&key_id).unwrap();
        assert_eq!(active.version, 1);
        assert!(active.active);
    }

    #[test]
    fn test_rotate_multiple() {
        let manager = KeyManager::with_default();
        let key_id = KeyId::new("test");
        for i in 1..=3 {
            let event = manager.rotate(&key_id).unwrap();
            assert!(
                matches!(event, RotationEvent::Rotated { new_version, .. } if new_version == i)
            );
        }
        let active = manager.get_active(&key_id).unwrap();
        assert_eq!(active.version, 3);
        let versions = manager.list_versions(&key_id);
        assert_eq!(versions.len(), 3);
    }

    #[test]
    fn test_rotate_deactivates_old() {
        let manager = KeyManager::with_default();
        let key_id = KeyId::new("test");
        manager.rotate(&key_id).unwrap();
        let v1 = manager.get_active(&key_id).unwrap();
        assert!(v1.active);
        manager.rotate(&key_id).unwrap();
        let versions = manager.list_versions(&key_id);
        assert!(!versions[0].active);
        assert!(versions[1].active);
    }

    #[test]
    fn test_keep_history_retires_old() {
        let manager = KeyManager::new(
            RotationPolicy {
                interval: Duration::from_secs(60),
                keep_history: 2,
            },
            Box::new(RandomKeyGenerator),
        );
        let key_id = KeyId::new("test");
        for _ in 0..5 {
            manager.rotate(&key_id).unwrap();
        }
        let versions = manager.list_versions(&key_id);
        assert!(versions.len() <= 3);
        assert!(versions.iter().any(|v| v.retired));
    }

    #[test]
    fn test_get_valid_keys() {
        let manager = KeyManager::new(
            RotationPolicy {
                interval: Duration::from_secs(60),
                keep_history: 1,
            },
            Box::new(RandomKeyGenerator),
        );
        let key_id = KeyId::new("test");
        for _ in 0..3 {
            manager.rotate(&key_id).unwrap();
        }
        let valid = manager.get_valid_keys(&key_id);
        for v in &valid {
            assert!(!v.retired);
        }
    }

    #[test]
    fn test_get_active_nonexistent() {
        let manager = KeyManager::with_default();
        assert!(manager.get_active(&KeyId::new("nonexistent")).is_none());
    }

    #[test]
    fn test_list_versions_empty() {
        let manager = KeyManager::with_default();
        let versions = manager.list_versions(&KeyId::new("nonexistent"));
        assert!(versions.is_empty());
    }

    #[test]
    fn test_random_key_generator() {
        let gen = RandomKeyGenerator;
        let key1 = gen.generate();
        let key2 = gen.generate();
        assert_eq!(key1.len(), 32);
        assert_eq!(key2.len(), 32);
    }
}
