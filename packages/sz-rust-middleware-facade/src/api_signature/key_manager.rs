// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 密钥管理器（spec 5.12.7）
//!
//! 多密钥查找 + 轮换窗口内旧密钥有效。

#![forbid(unsafe_code)]

use std::collections::HashMap;

use super::ApiKey;

/// 密钥管理器
pub struct KeyManager {
    keys: HashMap<String, ApiKey>,
}

impl KeyManager {
    /// 创建密钥管理器
    pub fn new(keys: Vec<ApiKey>) -> Self {
        let map = keys.into_iter().map(|k| (k.key_id.clone(), k)).collect();
        Self { keys: map }
    }

    /// 按 key_id 查找密钥
    ///
    /// 轮换窗口内旧密钥仍有效（spec 5.12.7）。
    pub fn find_key(&self, key_id: &str) -> Option<&ApiKey> {
        self.keys.get(key_id)
    }

    /// 添加密钥（轮换时添加新密钥，旧密钥保留）
    pub fn add_key(&mut self, key: ApiKey) {
        self.keys.insert(key.key_id.clone(), key);
    }

    /// 移除密钥（轮换完成后移除旧密钥）
    pub fn remove_key(&mut self, key_id: &str) {
        self.keys.remove(key_id);
    }

    /// 密钥数量
    pub fn count(&self) -> usize {
        self.keys.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_existing_key() {
        let mgr = KeyManager::new(vec![ApiKey::new("k1", "s1")]);
        assert!(mgr.find_key("k1").is_some());
    }

    #[test]
    fn test_find_nonexistent_key() {
        let mgr = KeyManager::new(vec![ApiKey::new("k1", "s1")]);
        assert!(mgr.find_key("k2").is_none());
    }

    #[test]
    fn test_key_rotation() {
        let mut mgr = KeyManager::new(vec![ApiKey::new("old_key", "old_secret")]);

        // 添加新密钥（轮换）
        mgr.add_key(ApiKey::new("new_key", "new_secret"));

        // 旧密钥仍有效
        assert!(mgr.find_key("old_key").is_some(), "旧密钥应仍有效");
        assert!(mgr.find_key("new_key").is_some(), "新密钥应有效");

        // 移除旧密钥
        mgr.remove_key("old_key");
        assert!(mgr.find_key("old_key").is_none(), "旧密钥移除后应无效");
    }
}
