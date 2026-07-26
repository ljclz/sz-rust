//! LLM 缓存层 — Phase 7b.4
//!
//! 对应 `SzRSQL技术实现方案.md` 9.9 节。
//!
//! # 设计
//!
//! 缓存 NL 查询 → SQL 结果，避免重复调用 NL2SQL 引擎。
//!
//! 1. **LRU 淘汰** — 容量满时淘汰最久未访问的条目
//! 2. **CDC 失效** — 表数据变更时失效所有依赖该表的缓存条目
//! 3. **命中率统计** — 记录 hit/miss 计数，供监控使用
//!
//! # 验证标准
//!
//! - 模拟 100000 条重复 NL 查询 → 缓存命中率 >= 60%
//! - CDC 事件触发缓存失效 → 下次查询重新生成
//!
//! 对应 `SzRSQL实施进度.md` Phase 7b.4。

use std::collections::HashMap;

// =====================================================================
//  错误类型
// =====================================================================

/// LLM 缓存错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmCacheError {
    /// 容量为 0
    #[error("cache capacity must be > 0")]
    ZeroCapacity,
}

// =====================================================================
//  缓存条目
// =====================================================================

/// 缓存条目
#[derive(Debug, Clone)]
struct CacheEntry {
    /// 生成的 SQL
    sql: String,
    /// 依赖的表列表（用于 CDC 失效）
    table_deps: Vec<String>,
    /// 创建时的逻辑时钟（用于 LRU 淘汰）
    created_at: u64,
    /// 最后访问时的逻辑时钟
    last_accessed: u64,
    /// 访问次数
    access_count: u64,
}

// =====================================================================
//  缓存统计
// =====================================================================

/// 缓存统计信息
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// 当前缓存条目数
    pub entry_count: usize,
    /// 缓存容量
    pub capacity: usize,
    /// 总查询次数（hit + miss）
    pub total_queries: u64,
    /// 缓存命中次数
    pub hits: u64,
    /// 缓存未命中次数
    pub misses: u64,
    /// CDC 失效次数
    pub invalidations: u64,
    /// LRU 淘汰次数
    pub evictions: u64,
}

impl CacheStats {
    /// 缓存命中率（0.0 ~ 1.0）
    pub fn hit_rate(&self) -> f64 {
        if self.total_queries == 0 {
            return 0.0;
        }
        self.hits as f64 / self.total_queries as f64
    }
}

// =====================================================================
//  LlmCache — LLM 缓存层
// =====================================================================

/// LLM 缓存层 — LRU 淘汰 + CDC 表级失效
///
/// # 工作流程
///
/// 1. `get(query)` — 查询缓存，命中返回 SQL，未命中返回 None
/// 2. `put(query, sql, table_deps)` — 插入缓存条目（容量满时 LRU 淘汰）
/// 3. `invalidate_table(table)` — CDC 事件触发，失效所有依赖该表的条目
/// 4. `invalidate_query(query)` — 失效特定查询
///
/// # LRU 淘汰策略
///
/// 每次访问更新 `last_accessed` 逻辑时钟。容量满时淘汰 `last_accessed` 最小的条目。
#[derive(Debug)]
pub struct LlmCache {
    /// 缓存映射：normalized_query → CacheEntry
    entries: HashMap<String, CacheEntry>,
    /// 最大容量
    capacity: usize,
    /// 逻辑时钟（每次 get/put 递增）
    clock: u64,
    /// 统计信息
    stats: CacheStats,
}

impl LlmCache {
    /// 创建指定容量的缓存
    pub fn new(capacity: usize) -> Result<Self, LlmCacheError> {
        if capacity == 0 {
            return Err(LlmCacheError::ZeroCapacity);
        }
        Ok(Self {
            entries: HashMap::with_capacity(capacity),
            capacity,
            clock: 0,
            stats: CacheStats {
                capacity,
                ..Default::default()
            },
        })
    }

    /// 当前缓存条目数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 缓存容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 获取统计信息（包含当前 entry_count 快照）
    pub fn stats(&self) -> CacheStats {
        let mut s = self.stats.clone();
        s.entry_count = self.entries.len();
        s
    }

    /// 查询缓存
    ///
    /// 命中返回 SQL 的克隆，未命中返回 None。
    /// 每次调用递增 total_queries；命中递增 hits，未命中递增 misses。
    pub fn get(&mut self, query: &str) -> Option<String> {
        self.clock += 1;
        self.stats.total_queries += 1;

        let key = normalize_query_key(query);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_accessed = self.clock;
            entry.access_count += 1;
            self.stats.hits += 1;
            Some(entry.sql.clone())
        } else {
            self.stats.misses += 1;
            None
        }
    }

    /// 插入缓存条目
    ///
    /// 如果容量满，先 LRU 淘汰最久未访问的条目，再插入。
    pub fn put(&mut self, query: &str, sql: String, table_deps: Vec<String>) {
        self.clock += 1;
        let key = normalize_query_key(query);

        // 已存在则更新
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.sql = sql;
            entry.table_deps = table_deps;
            entry.created_at = self.clock;
            entry.last_accessed = self.clock;
            entry.access_count = 1;
            return;
        }

        // 容量满则 LRU 淘汰
        if self.entries.len() >= self.capacity {
            self.evict_lru();
        }

        self.entries.insert(
            key,
            CacheEntry {
                sql,
                table_deps,
                created_at: self.clock,
                last_accessed: self.clock,
                access_count: 1,
            },
        );
    }

    /// CDC 事件触发：失效所有依赖指定表的缓存条目
    ///
    /// 返回失效的条目数。
    pub fn invalidate_table(&mut self, table_name: &str) -> usize {
        let table_lower = table_name.to_lowercase();
        let before = self.entries.len();

        self.entries.retain(|_, entry| {
            !entry
                .table_deps
                .iter()
                .any(|t| t.to_lowercase() == table_lower)
        });

        let invalidated = before - self.entries.len();
        self.stats.invalidations += invalidated as u64;
        invalidated
    }

    /// 失效特定查询的缓存
    ///
    /// 返回是否成功失效（true = 原本存在并已移除）。
    pub fn invalidate_query(&mut self, query: &str) -> bool {
        let key = normalize_query_key(query);
        if self.entries.remove(&key).is_some() {
            self.stats.invalidations += 1;
            true
        } else {
            false
        }
    }

    /// 清空所有缓存
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// LRU 淘汰：移除 last_accessed 最小的条目
    fn evict_lru(&mut self) {
        // 找到 last_accessed 最小的 key
        let evict_key = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_accessed)
            .map(|(key, _)| key.clone());

        if let Some(key) = evict_key {
            self.entries.remove(&key);
            self.stats.evictions += 1;
        }
    }
}

/// 归一化查询作为缓存 key：trim + 小写化
///
/// 注意：与 nl2sql 的 normalize_query 不同，缓存 key 需要小写化
/// 以确保 "Find Students" 和 "find students" 命中同一缓存条目。
fn normalize_query_key(query: &str) -> String {
    // 合并多余空白 + 小写化（split_whitespace 已自动处理首尾空白）
    query
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .to_lowercase()
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b4_cache_creation() {
        let cache = LlmCache::new(100).unwrap();
        assert_eq!(cache.capacity(), 100);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_7b4_zero_capacity_errors() {
        let result = LlmCache::new(0);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LlmCacheError::ZeroCapacity);
    }

    #[test]
    fn test_7b4_put_and_get() {
        let mut cache = LlmCache::new(10).unwrap();
        cache.put(
            "find students where age > 20",
            "SELECT * FROM students WHERE age > 20".to_string(),
            vec!["students".to_string()],
        );
        assert_eq!(cache.len(), 1);

        let result = cache.get("find students where age > 20");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "SELECT * FROM students WHERE age > 20");
    }

    #[test]
    fn test_7b4_get_miss() {
        let mut cache = LlmCache::new(10).unwrap();
        let result = cache.get("nonexistent query");
        assert!(result.is_none());
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn test_7b4_case_insensitive_key() {
        let mut cache = LlmCache::new(10).unwrap();
        cache.put(
            "Find Students",
            "SELECT * FROM students".to_string(),
            vec!["students".to_string()],
        );

        // 不同大小写应命中
        assert!(cache.get("find students").is_some());
        assert!(cache.get("FIND STUDENTS").is_some());
        assert_eq!(cache.stats().hits, 2);
    }

    #[test]
    fn test_7b4_whitespace_normalization() {
        let mut cache = LlmCache::new(10).unwrap();
        cache.put(
            "find students",
            "SELECT * FROM students".to_string(),
            vec!["students".to_string()],
        );

        // 多余空白应命中
        assert!(cache.get("  find   students  ").is_some());
        assert_eq!(cache.stats().hits, 1);
    }

    // -----------------------------------------------------------------
    //  LRU 淘汰测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b4_lru_eviction() {
        let mut cache = LlmCache::new(3).unwrap();

        // 插入 3 个条目
        cache.put("q1", "sql1".to_string(), vec!["t1".to_string()]);
        cache.put("q2", "sql2".to_string(), vec!["t1".to_string()]);
        cache.put("q3", "sql3".to_string(), vec!["t1".to_string()]);
        assert_eq!(cache.len(), 3);

        // 访问 q1 使其成为最近访问
        cache.get("q1");

        // 插入 q4，应淘汰 q2（最久未访问）
        cache.put("q4", "sql4".to_string(), vec!["t1".to_string()]);
        assert_eq!(cache.len(), 3);
        assert!(cache.get("q2").is_none(), "q2 should have been evicted");
        assert!(cache.get("q1").is_some(), "q1 should still be cached");
        assert!(cache.get("q3").is_some(), "q3 should still be cached");
        assert!(cache.get("q4").is_some(), "q4 should be cached");
        assert!(cache.stats().evictions >= 1);
    }

    #[test]
    fn test_7b4_put_update_existing() {
        let mut cache = LlmCache::new(10).unwrap();
        cache.put("query", "old_sql".to_string(), vec!["t1".to_string()]);
        cache.put("query", "new_sql".to_string(), vec!["t2".to_string()]);
        assert_eq!(cache.len(), 1); // 不应新增条目
        assert_eq!(cache.get("query").unwrap(), "new_sql");
    }

    // -----------------------------------------------------------------
    //  CDC 失效测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b4_invalidate_table() {
        let mut cache = LlmCache::new(10).unwrap();
        cache.put(
            "find students",
            "SELECT * FROM students".to_string(),
            vec!["students".to_string()],
        );
        cache.put(
            "find courses",
            "SELECT * FROM courses".to_string(),
            vec!["courses".to_string()],
        );
        cache.put(
            "join students courses",
            "SELECT * FROM students JOIN courses".to_string(),
            vec!["students".to_string(), "courses".to_string()],
        );
        assert_eq!(cache.len(), 3);

        // CDC 事件：students 表变更
        let invalidated = cache.invalidate_table("students");
        assert_eq!(invalidated, 2); // "find students" 和 "join students courses"
        assert_eq!(cache.len(), 1);
        assert!(cache.get("find courses").is_some());
        assert!(cache.stats().invalidations >= 2);
    }

    #[test]
    fn test_7b4_invalidate_table_case_insensitive() {
        let mut cache = LlmCache::new(10).unwrap();
        cache.put(
            "find students",
            "SELECT * FROM students".to_string(),
            vec!["Students".to_string()],
        );

        let invalidated = cache.invalidate_table("STUDENTS");
        assert_eq!(invalidated, 1);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_7b4_invalidate_table_no_match() {
        let mut cache = LlmCache::new(10).unwrap();
        cache.put(
            "find students",
            "SELECT * FROM students".to_string(),
            vec!["students".to_string()],
        );

        let invalidated = cache.invalidate_table("nonexistent_table");
        assert_eq!(invalidated, 0);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_7b4_invalidate_query() {
        let mut cache = LlmCache::new(10).unwrap();
        cache.put(
            "find students",
            "SELECT * FROM students".to_string(),
            vec!["students".to_string()],
        );

        assert!(cache.invalidate_query("find students"));
        assert_eq!(cache.len(), 0);
        assert!(!cache.invalidate_query("find students")); // 再次失效返回 false
    }

    #[test]
    fn test_7b4_clear() {
        let mut cache = LlmCache::new(10).unwrap();
        cache.put("q1", "s1".to_string(), vec![]);
        cache.put("q2", "s2".to_string(), vec![]);
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    // -----------------------------------------------------------------
    //  命中率统计测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b4_hit_rate() {
        let mut cache = LlmCache::new(100).unwrap();

        // 5 次 miss
        cache.get("q1");
        cache.get("q2");
        cache.get("q3");
        cache.get("q4");
        cache.get("q5");

        // 填充缓存
        cache.put("q1", "s1".to_string(), vec![]);
        cache.put("q2", "s2".to_string(), vec![]);
        cache.put("q3", "s3".to_string(), vec![]);

        // 3 次 hit
        cache.get("q1");
        cache.get("q2");
        cache.get("q3");

        // 2 次 miss
        cache.get("q6");
        cache.get("q7");

        let stats = cache.stats();
        assert_eq!(stats.total_queries, 10);
        assert_eq!(stats.hits, 3);
        assert_eq!(stats.misses, 7);
        assert!((stats.hit_rate() - 0.3).abs() < 0.001);
    }

    // -----------------------------------------------------------------
    //  Stress 测试：100000 条重复查询 → 命中率 >= 60%
    // -----------------------------------------------------------------

    #[test]
    fn test_7b4_stress_100000_queries_hit_rate() {
        let mut cache = LlmCache::new(1000).unwrap();

        // 模拟 100000 条查询，其中 70% 是重复查询（来自 100 个模板）
        let templates: Vec<String> = (0..100)
            .map(|i| format!("find data from table_{i} where id = {i}"))
            .collect();
        let sqls: Vec<String> = templates
            .iter()
            .map(|t| format!("SELECT * FROM table WHERE q = '{t}'"))
            .collect();

        // 先填充缓存（100 个模板）
        for (tmpl, sql) in templates.iter().zip(sqls.iter()) {
            cache.put(tmpl, sql.clone(), vec!["table".to_string()]);
        }

        // 模拟 100000 次查询：70% 重复（命中），30% 新查询（未命中）
        let total = 100_000usize;
        for i in 0..total {
            if i % 10 < 7 {
                // 70% 重复查询
                let idx = i % templates.len();
                cache.get(&templates[idx]);
            } else {
                // 30% 新查询
                cache.get(&format!("unique query {i}"));
            }
        }

        let stats = cache.stats();
        let hit_rate = stats.hit_rate();
        assert!(
            hit_rate >= 0.6,
            "cache hit rate should be >= 60%, got {:.1}% (hits={}, total={})",
            hit_rate * 100.0,
            stats.hits,
            stats.total_queries
        );
    }

    // -----------------------------------------------------------------
    //  CDC 失效 → 重新生成测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b4_cdc_invalidation_then_regenerate() {
        let mut cache = LlmCache::new(100).unwrap();

        // 第一次查询：miss → 生成 → 缓存
        let query = "find students where age > 20";
        assert!(cache.get(query).is_none()); // miss

        let sql_v1 = "SELECT * FROM students WHERE age > 20".to_string();
        cache.put(query, sql_v1.clone(), vec!["students".to_string()]);

        // 第二次查询：hit（返回缓存的 SQL）
        let result = cache.get(query);
        assert_eq!(result, Some(sql_v1));

        // CDC 事件：students 表数据变更 → 失效缓存
        let invalidated = cache.invalidate_table("students");
        assert_eq!(invalidated, 1);

        // 第三次查询：miss（缓存已失效，需重新生成）
        assert!(cache.get(query).is_none());

        // 重新生成 SQL（可能因数据变更而不同）
        let sql_v2 = "SELECT * FROM students WHERE age > 20 AND status = 'active'".to_string();
        cache.put(query, sql_v2.clone(), vec!["students".to_string()]);

        // 第四次查询：hit（返回新缓存的 SQL）
        let result = cache.get(query);
        assert_eq!(result, Some(sql_v2));
    }

    // -----------------------------------------------------------------
    //  多表 CDC 级联失效测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b4_cdc_multi_table_cascade() {
        let mut cache = LlmCache::new(100).unwrap();

        // 缓存多个查询，依赖不同表
        cache.put("q1", "s1".to_string(), vec!["students".to_string()]);
        cache.put("q2", "s2".to_string(), vec!["courses".to_string()]);
        cache.put(
            "q3",
            "s3".to_string(),
            vec!["students".to_string(), "courses".to_string()],
        );
        cache.put("q4", "s4".to_string(), vec!["enrollments".to_string()]);

        assert_eq!(cache.len(), 4);

        // CDC: students 表变更 → 失效 q1, q3
        let inv1 = cache.invalidate_table("students");
        assert_eq!(inv1, 2);
        assert_eq!(cache.len(), 2);

        // CDC: courses 表变更 → 失效 q2, q3（q3 已被失效，只剩 q2）
        let inv2 = cache.invalidate_table("courses");
        assert_eq!(inv2, 1);
        assert_eq!(cache.len(), 1);

        // q4 仍存在
        assert!(cache.get("q4").is_some());
    }

    // -----------------------------------------------------------------
    //  容量边界测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b4_capacity_one() {
        let mut cache = LlmCache::new(1).unwrap();
        cache.put("q1", "s1".to_string(), vec![]);
        assert_eq!(cache.len(), 1);

        cache.put("q2", "s2".to_string(), vec![]);
        assert_eq!(cache.len(), 1);
        assert!(cache.get("q1").is_none(), "q1 should be evicted");
        assert!(cache.get("q2").is_some());
    }

    #[test]
    fn test_7b4_stats_tracking() {
        let mut cache = LlmCache::new(5).unwrap();

        cache.put("q1", "s1".to_string(), vec!["t1".to_string()]);
        cache.put("q2", "s2".to_string(), vec!["t1".to_string()]);
        cache.put("q3", "s3".to_string(), vec!["t1".to_string()]);
        cache.put("q4", "s4".to_string(), vec!["t1".to_string()]);
        cache.put("q5", "s5".to_string(), vec!["t1".to_string()]);
        cache.put("q6", "s6".to_string(), vec!["t1".to_string()]); // 触发淘汰

        let stats = cache.stats();
        assert_eq!(stats.entry_count, 5); // 容量 5，插入 6 个后淘汰 1 个
        assert_eq!(stats.capacity, 5);
        assert!(stats.evictions >= 1);
    }
}
