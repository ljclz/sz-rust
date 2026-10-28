// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 模板缓存 — LRU 淘汰策略（spec 5.7.4 + 6.4.2）

#![forbid(unsafe_code)]

use std::collections::HashMap;

/// 已编译模板（简化表示）
#[derive(Debug, Clone)]
pub struct CompiledTemplate {
    /// 模板名称
    pub name: String,
    /// 编译后的内容（Askama 为 Rust 代码，Tera 为 AST）
    pub compiled: String,
    /// 编译时间戳
    pub compiled_at: u64,
}

/// LRU 模板缓存（spec 5.7.4 + 6.4.2）
pub struct TemplateCache {
    /// 缓存容量
    capacity: usize,
    /// 缓存条目
    entries: HashMap<String, CompiledTemplate>,
    /// 访问顺序（LRU 淘汰用，最近访问的在末尾）
    access_order: Vec<String>,
}

impl TemplateCache {
    /// 创建指定容量的缓存
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            access_order: Vec::new(),
        }
    }

    /// 获取缓存中的模板
    pub fn get(&mut self, name: &str) -> Option<&CompiledTemplate> {
        if self.entries.contains_key(name) {
            // 更新访问顺序
            self.access_order.retain(|n| n != name);
            self.access_order.push(name.to_string());
            self.entries.get(name)
        } else {
            None
        }
    }

    /// 插入模板到缓存
    pub fn insert(&mut self, template: CompiledTemplate) {
        let name = template.name.clone();

        // 如果已存在，先移除旧条目
        if self.entries.contains_key(&name) {
            self.access_order.retain(|n| n != &name);
        } else if self.entries.len() >= self.capacity {
            // LRU 淘汰：移除最久未访问的
            if let Some(oldest) = self.access_order.first().cloned() {
                self.entries.remove(&oldest);
                self.access_order.remove(0);
            }
        }

        self.entries.insert(name.clone(), template);
        self.access_order.push(name);
    }

    /// 缓存命中数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 缓存是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 清空缓存
    pub fn clear(&mut self) {
        self.entries.clear();
        self.access_order.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_template(name: &str) -> CompiledTemplate {
        CompiledTemplate {
            name: name.to_string(),
            compiled: format!("compiled_{}", name),
            compiled_at: 0,
        }
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = TemplateCache::new(3);
        cache.insert(make_template("a"));
        cache.insert(make_template("b"));

        assert_eq!(cache.len(), 2);
        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_some());
        assert!(cache.get("c").is_none());
    }

    #[test]
    fn test_cache_lru_eviction() {
        let mut cache = TemplateCache::new(2);
        cache.insert(make_template("a"));
        cache.insert(make_template("b"));

        // 访问 a，使 b 成为最久未访问
        cache.get("a");

        // 插入 c，应淘汰 b
        cache.insert(make_template("c"));

        assert!(cache.get("a").is_some(), "a 应存在（最近访问过）");
        assert!(cache.get("b").is_none(), "b 应被 LRU 淘汰");
        assert!(cache.get("c").is_some(), "c 应存在");
    }

    #[test]
    fn test_cache_update_existing() {
        let mut cache = TemplateCache::new(2);
        cache.insert(make_template("a"));
        cache.insert(CompiledTemplate {
            name: "a".to_string(),
            compiled: "updated".to_string(),
            compiled_at: 1,
        });

        let entry = cache.get("a").unwrap();
        assert_eq!(entry.compiled, "updated");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_cache_clear() {
        let mut cache = TemplateCache::new(3);
        cache.insert(make_template("a"));
        cache.insert(make_template("b"));
        cache.clear();

        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }
}
