// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Mock 注入：复用 facade Mock 替换机制。

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Mock 注入器：管理 Mock 替换与恢复。
pub struct MockInjector {
    mocks: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl Default for MockInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl MockInjector {
    /// 创建空注入器。
    pub fn new() -> Self {
        Self {
            mocks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 注入 Mock 值。
    pub fn inject(&self, key: &str, value: impl serde::Serialize) {
        let mut mocks = self.mocks.write();
        mocks.insert(
            key.to_string(),
            serde_json::to_value(value).expect("testkit mock 值必须可序列化为 JSON"),
        );
    }

    /// 获取 Mock 值。
    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.mocks.read().get(key).cloned()
    }

    /// 移除 Mock。
    pub fn remove(&self, key: &str) {
        self.mocks.write().remove(key);
    }

    /// 清空所有 Mock。
    pub fn clear(&self) {
        self.mocks.write().clear();
    }

    /// 检查 Mock 是否存在。
    pub fn has(&self, key: &str) -> bool {
        self.mocks.read().contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_and_get() {
        let injector = MockInjector::new();
        injector.inject("cache", "mock_value");
        assert_eq!(
            injector.get("cache").unwrap(),
            serde_json::Value::String("mock_value".into())
        );
    }

    #[test]
    fn remove_mock() {
        let injector = MockInjector::new();
        injector.inject("key", 42);
        assert!(injector.has("key"));
        injector.remove("key");
        assert!(!injector.has("key"));
    }

    #[test]
    fn clear_all() {
        let injector = MockInjector::new();
        injector.inject("a", 1);
        injector.inject("b", 2);
        injector.clear();
        assert!(!injector.has("a"));
        assert!(!injector.has("b"));
    }

    #[test]
    fn get_nonexistent() {
        let injector = MockInjector::new();
        assert!(injector.get("nope").is_none());
    }

    #[test]
    fn inject_complex_value() {
        let injector = MockInjector::new();
        injector.inject("config", serde_json::json!({"timeout": 30, "retries": 3}));
        let val = injector.get("config").unwrap();
        assert_eq!(val["timeout"], 30);
        assert_eq!(val["retries"], 3);
    }
}
