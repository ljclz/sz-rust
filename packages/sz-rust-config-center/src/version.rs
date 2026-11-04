// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 版本历史存储与回滚（T018）

use std::collections::HashMap;

use parking_lot::RwLock;

use crate::source::ConfigEntry;

/// 版本历史管理器
pub struct VersionHistory {
    /// version → 配置快照
    snapshots: RwLock<HashMap<u64, HashMap<String, ConfigEntry>>>,
    /// 当前版本号
    current: RwLock<u64>,
}

impl VersionHistory {
    /// 创建版本历史管理器
    pub fn new() -> Self {
        Self {
            snapshots: RwLock::new(HashMap::new()),
            current: RwLock::new(0),
        }
    }

    /// 保存当前配置为新版本
    pub fn save(&self, config: HashMap<String, ConfigEntry>) -> u64 {
        let mut current = self.current.write();
        *current += 1;
        let version = *current;
        drop(current);

        let mut snapshots = self.snapshots.write();
        snapshots.insert(version, config);
        version
    }

    /// 回滚到指定版本
    pub fn rollback(&self, version: u64) -> Option<HashMap<String, ConfigEntry>> {
        let snapshots = self.snapshots.read();
        if let Some(config) = snapshots.get(&version) {
            let config = config.clone();
            drop(snapshots);
            *self.current.write() = version;
            Some(config)
        } else {
            None
        }
    }

    /// 获取当前版本号
    pub fn current_version(&self) -> u64 {
        *self.current.read()
    }

    /// 获取所有历史版本号
    pub fn versions(&self) -> Vec<u64> {
        let snapshots = self.snapshots.read();
        let mut versions: Vec<u64> = snapshots.keys().copied().collect();
        versions.sort();
        versions
    }
}

impl Default for VersionHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_config(key: &str, val: &str) -> HashMap<String, ConfigEntry> {
        let mut map = HashMap::new();
        map.insert(
            key.to_string(),
            ConfigEntry {
                key: key.to_string(),
                value: json!(val),
                is_sensitive: false,
                version: 1,
                gray_rule: None,
                updated_at: 0,
            },
        );
        map
    }

    #[test]
    fn test_save_and_rollback() {
        let history = VersionHistory::new();

        let v1 = history.save(make_config("db.host", "localhost"));
        let v2 = history.save(make_config("db.host", "remote"));

        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
        assert_eq!(history.current_version(), 2);

        let rolled_back = history.rollback(v1).unwrap();
        assert_eq!(
            rolled_back.get("db.host").unwrap().value,
            json!("localhost")
        );
        assert_eq!(history.current_version(), 1);
    }

    #[test]
    fn test_rollback_nonexistent() {
        let history = VersionHistory::new();
        assert!(history.rollback(99).is_none());
    }

    #[test]
    fn test_versions_list() {
        let history = VersionHistory::new();
        history.save(make_config("a", "1"));
        history.save(make_config("a", "2"));
        history.save(make_config("a", "3"));

        assert_eq!(history.versions(), vec![1, 2, 3]);
    }
}
