//! Phase 5.14 — 结果缓存（Result Cache）。
//!
//! 对应 `SzRSQL实施进度.md` Phase 5.14。
//!
//! # 设计
//!
//! - **LRU 缓存**：`ResultCache` 使用 `HashMap<String, ResultEntry>` + 单调递增 `lru_counter` 维护 LRU 顺序
//! - **键规范化**：`normalize_sql` 去除前后空白、合并连续空白、转小写、去除末尾分号，保证
//!   `SELECT * FROM t` 与 `select  *  from  T` 命中同一缓存条目
//! - **依赖追踪**：每个缓存结果记录其依赖的表名列表 `table_deps`，CDC 变更时按表名精准失效
//! - **失效策略**：
//!   1. `invalidate_table(name)`：删除所有依赖该表的结果（INSERT/UPDATE/DELETE 后调用，模拟 CDC）
//!   2. `invalidate_all()`：清空整个缓存（如 DDL 类型难以精准定位时）
//!   3. LRU 淘汰：插入新结果时若已满，淘汰最久未访问的条目
//! - **统计**：`CacheStats` 记录 hits / misses / evictions / invalidations / current_size / capacity
//!
//! # 与 PlanCache 的差异
//!
//! - `PlanCache` 缓存 `LogicalPlan`（结构化计划），用于跳过 plan 阶段
//! - `ResultCache` 缓存 `Vec<Row>`（查询结果），用于跳过 execute 阶段
//! - `ResultCache` 的 CDC 失效语义更严格：任何 DML（INSERT/UPDATE/DELETE）都应失效相关表的结果
//!
//! # 验收标准对照
//!
//! | 进度表原始验收标准 | 实际达成 |
//! |-------------------|---------|
//! | 相同参数查询两次 → 第二次命中结果缓存 | ✅ `test_basic_hit_miss` + `test_repeated_query_hit` |
//! | 响应时间 < 1ms | ✅ `test_cache_hit_response_time_under_1ms`：1000 次命中平均 < 1ms |
//! | CDC 变更后 → 关联表的结果缓存自动失效 | ✅ `test_cdc_invalidate_table` + `test_cdc_invalidate_join` |
//! | CDC 失效时间 < 10ms | ✅ `test_cdc_invalidation_time_under_10ms`：1000 条目失效 < 10ms |
//! | 缓存命中率 > 70% | ✅ `test_hit_rate_above_70_percent`：1000 次查询中 100 唯一 SQL 重复 10 次 |

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use szrsql_sql::executor::Row;
use szrsql_sql::plan::LogicalPlan;

// =====================================================================
//  缓存条目
// =====================================================================

/// 结果缓存条目
#[derive(Debug, Clone)]
struct ResultEntry {
    /// 缓存的查询结果行
    rows: Vec<Row>,
    /// 此结果依赖的表名（小写，已去重）
    table_deps: Vec<String>,
    /// 命中次数（每次 get +1）
    hit_count: usize,
    /// LRU 序号（单调递增，越大越近期访问）
    lru_seq: u64,
    /// 创建时间（用于 TTL 扩展）
    created_at: Instant,
}

// =====================================================================
//  缓存统计
// =====================================================================

/// 缓存统计信息
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// 命中次数
    pub hits: usize,
    /// 未命中次数
    pub misses: usize,
    /// LRU 淘汰次数
    pub evictions: usize,
    /// 失效条目数（含 invalidate_table 和 invalidate_all）
    pub invalidations: usize,
    /// 当前缓存大小
    pub current_size: usize,
    /// 容量上限
    pub capacity: usize,
}

impl CacheStats {
    /// 计算命中率（0.0 ~ 1.0）
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

// =====================================================================
//  ResultCache
// =====================================================================

/// LRU 查询结果缓存
///
/// # 线程安全
///
/// `ResultCache` 本身不是线程安全的；多线程场景下应使用 `Mutex<ResultCache>` 包装。
///
/// # 语义
///
/// - **命中**：返回缓存的 `Vec<Row>` 引用，跳过 parse + plan + execute 全流程
/// - **未命中**：调用方执行查询后调用 `insert` 或 `insert_with_plan` 写入缓存
/// - **失效**：DML（INSERT/UPDATE/DELETE）后调用 `invalidate_table` 模拟 CDC 失效
///
/// # 示例
///
/// ```ignore
/// use szrsql_optimizer::result_cache::ResultCache;
///
/// let mut cache = ResultCache::new(1024);
///
/// // 首次查询：未命中
/// let sql = "SELECT id FROM users WHERE id = 1";
/// if cache.get(sql).is_none() {
///     let rows = execute_query(sql);  // 用户自定义执行
///     cache.insert_with_plan(sql, rows, &plan);
/// }
///
/// // 第二次查询：命中
/// let cached = cache.get(sql).unwrap();
/// ```
pub struct ResultCache {
    /// 规范化 SQL → 缓存条目
    entries: HashMap<String, ResultEntry>,
    /// 容量上限
    capacity: usize,
    /// 全局 LRU 序号（每次访问递增）
    lru_counter: u64,
    /// 统计
    stats: CacheStats,
}

impl ResultCache {
    /// 创建指定容量的结果缓存
    ///
    /// # Panics
    ///
    /// 当 `capacity == 0` 时 panic（无意义的缓存）
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "ResultCache capacity must be > 0");
        Self {
            entries: HashMap::new(),
            capacity,
            lru_counter: 0,
            stats: CacheStats {
                capacity,
                ..Default::default()
            },
        }
    }

    /// 规范化 SQL：去除前后空白、合并连续空白、转小写、去除末尾分号
    ///
    /// 与 `PlanCache::normalize_sql` 保持一致，保证两个缓存可以共用同一规范化规则。
    fn normalize_sql(sql: &str) -> String {
        let trimmed = sql.trim();
        let stripped = trimmed.trim_end_matches(';');
        stripped
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }

    /// 查询缓存
    ///
    /// 返回 `Some(&Vec<Row>)` 表示命中，`None` 表示未命中（应执行查询后 `insert`）。
    /// 命中时更新 LRU 序号与 hit_count。
    pub fn get(&mut self, sql: &str) -> Option<&Vec<Row>> {
        let key = Self::normalize_sql(sql);
        self.lru_counter += 1;
        let seq = self.lru_counter;
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.hit_count += 1;
            entry.lru_seq = seq;
            self.stats.hits += 1;
            Some(&entry.rows)
        } else {
            self.stats.misses += 1;
            None
        }
    }

    /// 查询缓存并返回是否命中（不更新 LRU，用于检查）
    pub fn contains(&self, sql: &str) -> bool {
        let key = Self::normalize_sql(sql);
        self.entries.contains_key(&key)
    }

    /// 插入查询结果到缓存（显式指定表依赖）
    ///
    /// 若键已存在则覆盖；若已满则淘汰 LRU 最旧条目。
    pub fn insert(&mut self, sql: &str, rows: Vec<Row>, table_deps: Vec<String>) {
        let key = Self::normalize_sql(sql);
        self.lru_counter += 1;
        let seq = self.lru_counter;
        let now = Instant::now();

        // 若键已存在，覆盖（不计入淘汰）
        if let std::collections::hash_map::Entry::Occupied(mut e) = self.entries.entry(key.clone())
        {
            let entry = ResultEntry {
                rows,
                table_deps,
                hit_count: 0,
                lru_seq: seq,
                created_at: now,
            };
            e.insert(entry);
            return;
        }

        // 容量已满，LRU 淘汰
        if self.entries.len() >= self.capacity {
            self.evict_lru();
        }

        self.entries.insert(
            key,
            ResultEntry {
                rows,
                table_deps,
                hit_count: 0,
                lru_seq: seq,
                created_at: now,
            },
        );
        self.stats.current_size = self.entries.len();
    }

    /// 插入查询结果到缓存（从 LogicalPlan 提取表依赖）
    ///
    /// 等价于 `insert(sql, rows, collect_table_deps(&plan))`。
    pub fn insert_with_plan(&mut self, sql: &str, rows: Vec<Row>, plan: &LogicalPlan) {
        let deps = collect_table_deps(plan);
        self.insert(sql, rows, deps);
    }

    /// 淘汰 LRU 最旧条目（lru_seq 最小者）
    fn evict_lru(&mut self) {
        if let Some(oldest_key) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.lru_seq)
            .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest_key);
            self.stats.evictions += 1;
            self.stats.current_size = self.entries.len();
        }
    }

    /// CDC 失效：删除依赖指定表的所有结果
    ///
    /// 在 INSERT/UPDATE/DELETE 后调用，模拟 CDC（Change Data Capture）触发的失效。
    /// 返回失效的条目数。
    pub fn invalidate_table(&mut self, table_name: &str) -> usize {
        let table_lower = table_name.to_lowercase();
        let keys_to_remove: Vec<String> = self
            .entries
            .iter()
            .filter_map(|(k, e)| {
                if e.table_deps.contains(&table_lower) {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        let count = keys_to_remove.len();
        for key in keys_to_remove {
            self.entries.remove(&key);
        }
        self.stats.invalidations += count;
        self.stats.current_size = self.entries.len();
        count
    }

    /// 清空所有缓存
    ///
    /// 返回被清空的条目数。
    pub fn invalidate_all(&mut self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        self.stats.invalidations += count;
        self.stats.current_size = 0;
        count
    }

    /// 获取缓存统计
    #[must_use]
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    /// 获取当前缓存大小
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 获取容量上限
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 获取某个 SQL 的命中次数（用于测试）
    #[must_use]
    pub fn hit_count_of(&self, sql: &str) -> Option<usize> {
        let key = Self::normalize_sql(sql);
        self.entries.get(&key).map(|e| e.hit_count)
    }

    /// 获取某个 SQL 依赖的表列表（用于测试）
    #[must_use]
    pub fn table_deps_of(&self, sql: &str) -> Option<Vec<String>> {
        let key = Self::normalize_sql(sql);
        self.entries.get(&key).map(|e| e.table_deps.clone())
    }

    /// 获取某个 SQL 的缓存行数（用于测试）
    #[must_use]
    pub fn row_count_of(&self, sql: &str) -> Option<usize> {
        let key = Self::normalize_sql(sql);
        self.entries.get(&key).map(|e| e.rows.len())
    }
}

impl std::fmt::Debug for ResultCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultCache")
            .field("entries", &self.entries.len())
            .field("capacity", &self.capacity)
            .field("stats", &self.stats)
            .finish()
    }
}

impl Default for ResultCache {
    fn default() -> Self {
        Self::new(1024)
    }
}

// =====================================================================
//  辅助：收集计划依赖的表
// =====================================================================

/// 收集计划中所有依赖的表名（小写，去重）
///
/// 此函数与 `plan_cache::collect_table_deps` 实现一致，保持模块独立性。
pub fn collect_table_deps(plan: &LogicalPlan) -> Vec<String> {
    let mut tables = HashSet::new();
    collect_table_deps_recursive(plan, &mut tables);
    tables.into_iter().collect()
}

fn collect_table_deps_recursive(plan: &LogicalPlan, out: &mut HashSet<String>) {
    match plan {
        LogicalPlan::Scan { table, .. } | LogicalPlan::IndexScan { table, .. } => {
            out.insert(table.name.to_lowercase());
        }
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Projection { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Distinct { input }
        | LogicalPlan::Aggregate { input, .. } => {
            collect_table_deps_recursive(input, out);
        }
        LogicalPlan::Join { left, right, .. } | LogicalPlan::SetOp { left, right, .. } => {
            collect_table_deps_recursive(left, out);
            collect_table_deps_recursive(right, out);
        }
        LogicalPlan::Shared { plan, .. } => collect_table_deps_recursive(plan, out),
        // DDL/DML/控制语句：不参与查询结果缓存依赖追踪
        _ => {}
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use szrsql_sql::executor::InMemoryTable;
    use szrsql_sql::parser::parse_sql;
    use szrsql_sql::plan::{InMemoryCatalog, Planner};
    use szrsql_types::value::{ColumnType, Value};

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    /// 构建 catalog + 简单测试表
    fn make_catalog() -> InMemoryCatalog {
        let mut catalog = InMemoryCatalog::new();
        catalog.add_simple_table(
            "users",
            vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
        );
        catalog.add_simple_table(
            "orders",
            vec![
                ("id", ColumnType::Int64),
                ("user_id", ColumnType::Int64),
                ("amount", ColumnType::Float64),
            ],
        );
        catalog
    }

    /// 规划 SQL，返回 LogicalPlan
    fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
        let stmts = parse_sql(sql).expect("parse failed");
        assert_eq!(stmts.len(), 1);
        let planner = Planner::new(catalog);
        planner
            .plan_statement(stmts.into_iter().next().unwrap())
            .expect("plan failed")
    }

    /// 构造 N 行测试结果
    fn make_rows(n: usize) -> Vec<Row> {
        (0..n)
            .map(|i| vec![Value::Int64(i as i64), Value::Text(format!("user{i}"))])
            .collect()
    }

    // -----------------------------------------------------------------
    //  基本功能测试
    // -----------------------------------------------------------------

    #[test]
    fn test_new_cache_empty() {
        let cache = ResultCache::new(10);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
        assert_eq!(cache.capacity(), 10);
        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 0);
    }

    #[test]
    fn test_default_capacity() {
        let cache = ResultCache::default();
        assert_eq!(cache.capacity(), 1024);
    }

    #[test]
    #[should_panic(expected = "ResultCache capacity must be > 0")]
    fn test_zero_capacity_panics() {
        let _ = ResultCache::new(0);
    }

    #[test]
    fn test_basic_hit_miss() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(3);

        // 第一次查询：未命中
        assert!(cache.get("SELECT * FROM users").is_none());
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 0);

        // 插入
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        // 第二次查询：命中
        let cached = cache.get("SELECT * FROM users");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().len(), 3);
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 1);
        assert!((cache.stats().hit_rate() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_repeated_query_hit() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(5);
        cache.insert("SELECT id FROM users", rows.clone(), vec!["users".into()]);

        // 连续 10 次查询都应命中
        for _ in 0..10 {
            let cached = cache.get("SELECT id FROM users");
            assert!(cached.is_some());
            assert_eq!(cached.unwrap().len(), 5);
        }
        assert_eq!(cache.stats().hits, 10);
        assert_eq!(cache.stats().misses, 0);
    }

    // -----------------------------------------------------------------
    //  SQL 规范化测试
    // -----------------------------------------------------------------

    #[test]
    fn test_normalize_sql_whitespace() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        // 不同空白应命中
        assert!(
            cache.get("  SELECT   *   FROM   users  ").is_some(),
            "不同空白应命中同一缓存条目"
        );
    }

    #[test]
    fn test_normalize_sql_case() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        // 不同大小写应命中
        assert!(
            cache.get("select * from Users").is_some(),
            "不同大小写应命中同一缓存条目"
        );
        assert!(
            cache.get("SELECT * FROM USERS").is_some(),
            "全大写应命中同一缓存条目"
        );
    }

    #[test]
    fn test_normalize_sql_trailing_semicolon() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        // 末尾分号被 trim 处理
        assert!(
            cache.get("SELECT * FROM users;").is_some(),
            "末尾分号应命中同一缓存条目"
        );
    }

    // -----------------------------------------------------------------
    //  LRU 淘汰测试
    // -----------------------------------------------------------------

    #[test]
    fn test_lru_eviction() {
        let mut cache = ResultCache::new(2);
        let rows = make_rows(1);

        // 插入 3 个结果，应淘汰第 1 个
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);
        cache.insert("SELECT id FROM users", rows.clone(), vec!["users".into()]);
        cache.insert("SELECT name FROM users", rows.clone(), vec!["users".into()]);

        // 容量应为 2
        assert_eq!(cache.len(), 2, "容量应为 2");
        assert_eq!(cache.stats().evictions, 1, "应淘汰 1 个条目");

        // 第 1 个应已失效
        assert!(
            cache.get("SELECT * FROM users").is_none(),
            "LRU 最旧条目应被淘汰"
        );
        // 第 2、3 个应命中
        assert!(cache.get("SELECT id FROM users").is_some());
        assert!(cache.get("SELECT name FROM users").is_some());
    }

    #[test]
    fn test_lru_order_after_get() {
        let mut cache = ResultCache::new(2);
        let rows = make_rows(1);

        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);
        cache.insert("SELECT id FROM users", rows.clone(), vec!["users".into()]);

        // 访问第 1 个，使其变为最近
        assert!(cache.get("SELECT * FROM users").is_some());

        // 插入第 3 个，应淘汰第 2 个（不是第 1 个）
        cache.insert("SELECT name FROM users", rows.clone(), vec!["users".into()]);

        assert!(
            cache.get("SELECT * FROM users").is_some(),
            "最近访问的应保留"
        );
        assert!(
            cache.get("SELECT id FROM users").is_none(),
            "最久未访问的应被淘汰"
        );
        assert!(cache.get("SELECT name FROM users").is_some());
    }

    #[test]
    fn test_lru_overwrite_no_eviction() {
        let mut cache = ResultCache::new(2);
        let rows = make_rows(1);

        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);
        cache.insert("SELECT id FROM users", rows.clone(), vec!["users".into()]);

        // 覆盖已有键，不应淘汰
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        assert_eq!(cache.len(), 2, "覆盖不应增加条目数");
        assert_eq!(cache.stats().evictions, 0, "覆盖不应触发淘汰");
    }

    // -----------------------------------------------------------------
    //  CDC 失效测试
    // -----------------------------------------------------------------

    #[test]
    fn test_cdc_invalidate_table() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);

        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);
        cache.insert("SELECT id FROM users", rows.clone(), vec!["users".into()]);
        cache.insert("SELECT * FROM orders", rows.clone(), vec!["orders".into()]);

        assert_eq!(cache.len(), 3);

        // 模拟 CDC：users 表发生 INSERT/UPDATE/DELETE
        let removed = cache.invalidate_table("users");
        assert_eq!(removed, 2, "应失效 2 个依赖 users 的结果");
        assert_eq!(cache.len(), 1, "应剩 1 个 orders 结果");

        // orders 结果应仍存在
        assert!(cache.get("SELECT * FROM orders").is_some());
        // users 结果应已失效
        assert!(cache.get("SELECT * FROM users").is_none());
        assert!(cache.get("SELECT id FROM users").is_none());
    }

    #[test]
    fn test_cdc_invalidate_join() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);

        // JOIN 查询依赖 users 和 orders 两张表
        cache.insert(
            "SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id",
            rows.clone(),
            vec!["users".into(), "orders".into()],
        );
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);
        cache.insert("SELECT * FROM orders", rows.clone(), vec!["orders".into()]);

        // CDC 失效 orders：应失效 JOIN + orders 单表
        let removed = cache.invalidate_table("orders");
        assert_eq!(removed, 2, "应失效 2 个依赖 orders 的结果（JOIN + 单表）");
        assert_eq!(cache.len(), 1);
        assert!(cache.get("SELECT * FROM users").is_some());
    }

    #[test]
    fn test_cdc_invalidate_table_case_insensitive() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        // 大写表名也应触发失效
        let removed = cache.invalidate_table("USERS");
        assert_eq!(removed, 1);
        assert!(cache.get("SELECT * FROM users").is_none());
    }

    #[test]
    fn test_cdc_invalidate_table_no_match() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        let removed = cache.invalidate_table("nonexistent");
        assert_eq!(removed, 0);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_invalidate_all() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);
        cache.insert("SELECT * FROM orders", rows.clone(), vec!["orders".into()]);

        let removed = cache.invalidate_all();
        assert_eq!(removed, 2);
        assert!(cache.is_empty());
        assert!(cache.get("SELECT * FROM users").is_none());
    }

    // -----------------------------------------------------------------
    //  响应时间验收测试（< 1ms）
    // -----------------------------------------------------------------

    #[test]
    fn test_cache_hit_response_time_under_1ms() {
        let mut cache = ResultCache::new(1024);
        // 插入 100 行结果
        let rows = make_rows(100);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        // 预热（首次 get 可能涉及 HashMap 内部调整）
        let _ = cache.get("SELECT * FROM users");

        // 1000 次命中，每次应 < 1ms
        let iterations = 1000;
        let mut total_duration = Duration::ZERO;
        for _ in 0..iterations {
            let start = Instant::now();
            let cached = cache.get("SELECT * FROM users");
            let elapsed = start.elapsed();
            assert!(cached.is_some());
            assert_eq!(cached.unwrap().len(), 100);
            total_duration += elapsed;
        }

        let avg_us = total_duration.as_micros() / iterations as u128;
        println!(
            "[Phase 5.14 响应时间] 1000 次命中平均: {}μs ({}ms)",
            avg_us,
            avg_us / 1000
        );
        // 平均应 < 1ms（1000μs）；放宽到 1000μs 以适应 CI 慢机器
        assert!(
            avg_us < 1000,
            "缓存命中平均响应时间应 < 1ms，实际 {avg_us}μs"
        );
    }

    // -----------------------------------------------------------------
    //  CDC 失效时间验收测试（< 10ms）
    // -----------------------------------------------------------------

    #[test]
    fn test_cdc_invalidation_time_under_10ms() {
        let mut cache = ResultCache::new(2048);
        let rows = make_rows(1);

        // 插入 1000 个结果，全部依赖 users 表
        for i in 0..1000 {
            let sql = format!("SELECT id FROM users WHERE id = {i}");
            cache.insert(&sql, rows.clone(), vec!["users".into()]);
        }
        assert_eq!(cache.len(), 1000);

        // CDC 失效 users 表，应在 10ms 内完成
        let start = Instant::now();
        let removed = cache.invalidate_table("users");
        let elapsed = start.elapsed();

        assert_eq!(removed, 1000);
        let elapsed_us = elapsed.as_micros();
        println!(
            "[Phase 5.14 CDC 失效] 1000 个条目失效耗时: {}μs ({}ms)",
            elapsed_us,
            elapsed_us / 1000
        );
        assert!(
            elapsed.as_millis() < 10,
            "CDC 失效 1000 个条目应 < 10ms，实际 {}ms",
            elapsed.as_millis()
        );
    }

    // -----------------------------------------------------------------
    //  缓存命中率验收测试（> 70%）
    // -----------------------------------------------------------------

    #[test]
    fn test_hit_rate_above_70_percent() {
        let mut cache = ResultCache::new(200);
        let rows = make_rows(1);

        // 100 个唯一 SQL，每个查询 10 次（重复查询场景）
        let sqls: Vec<String> = (0..100)
            .map(|i| format!("SELECT id FROM users WHERE id = {i}"))
            .collect();

        // 先插入所有 SQL
        for sql in &sqls {
            cache.insert(sql, rows.clone(), vec!["users".into()]);
        }

        // 重复查询 10 轮
        for _ in 0..10 {
            for sql in &sqls {
                let _ = cache.get(sql);
            }
        }

        let stats = cache.stats();
        let hit_rate = stats.hit_rate();
        println!(
            "[Phase 5.14 命中率] hits={}, misses={}, hit_rate={:.4}",
            stats.hits, stats.misses, hit_rate
        );
        assert!(
            hit_rate > 0.7,
            "重复查询场景命中率应 > 70%，实际 {hit_rate:.4}"
        );
    }

    // -----------------------------------------------------------------
    //  统计测试
    // -----------------------------------------------------------------

    #[test]
    fn test_stats_tracking() {
        let mut cache = ResultCache::new(5);
        let rows = make_rows(1);

        // 3 次 miss
        cache.get("SELECT * FROM users"); // miss
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);
        cache.get("SELECT id FROM users"); // miss
        cache.insert("SELECT id FROM users", rows.clone(), vec!["users".into()]);
        cache.get("SELECT * FROM orders"); // miss
        cache.insert("SELECT * FROM orders", rows.clone(), vec!["orders".into()]);

        // 2 次 hit
        cache.get("SELECT * FROM users"); // hit
        cache.get("SELECT id FROM users"); // hit

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 3);
        assert_eq!(stats.evictions, 0);
        assert_eq!(stats.invalidations, 0);
        assert_eq!(stats.current_size, 3);
        assert_eq!(stats.capacity, 5);
        assert!((stats.hit_rate() - 0.4).abs() < 0.001);
    }

    #[test]
    fn test_stats_after_eviction_and_invalidation() {
        let mut cache = ResultCache::new(2);
        let rows = make_rows(1);

        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);
        cache.insert("SELECT id FROM users", rows.clone(), vec!["users".into()]);
        cache.insert("SELECT name FROM users", rows.clone(), vec!["users".into()]); // 淘汰 1

        cache.invalidate_table("users"); // 失效剩余 2

        let stats = cache.stats();
        assert_eq!(stats.evictions, 1);
        assert_eq!(stats.invalidations, 2);
        assert_eq!(stats.current_size, 0);
    }

    // -----------------------------------------------------------------
    //  依赖追踪测试
    // -----------------------------------------------------------------

    #[test]
    fn test_table_deps_single_table() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        let deps = cache.table_deps_of("SELECT * FROM users").unwrap();
        assert_eq!(deps, vec!["users"]);
    }

    #[test]
    fn test_table_deps_join() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert(
            "SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id",
            rows.clone(),
            vec!["users".into(), "orders".into()],
        );

        let deps = cache
            .table_deps_of("SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id")
            .unwrap();
        // 应包含 users 和 orders
        assert!(deps.contains(&"users".to_string()));
        assert!(deps.contains(&"orders".to_string()));
        assert_eq!(deps.len(), 2);
    }

    // -----------------------------------------------------------------
    //  insert_with_plan 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_insert_with_plan_single_table() {
        let catalog = make_catalog();
        let plan = plan_sql("SELECT * FROM users", &catalog);
        let mut cache = ResultCache::new(10);
        let rows = make_rows(3);

        cache.insert_with_plan("SELECT * FROM users", rows.clone(), &plan);

        // 验证依赖自动提取
        let deps = cache.table_deps_of("SELECT * FROM users").unwrap();
        assert_eq!(deps, vec!["users"]);

        // 验证结果可命中
        let cached = cache.get("SELECT * FROM users");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().len(), 3);
    }

    #[test]
    fn test_insert_with_plan_join() {
        let catalog = make_catalog();
        let plan = plan_sql(
            "SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id",
            &catalog,
        );
        let mut cache = ResultCache::new(10);
        let rows = make_rows(2);

        cache.insert_with_plan(
            "SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id",
            rows.clone(),
            &plan,
        );

        // 验证 JOIN 依赖自动提取
        let deps = cache
            .table_deps_of("SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id")
            .unwrap();
        assert!(deps.contains(&"users".to_string()));
        assert!(deps.contains(&"orders".to_string()));
        assert_eq!(deps.len(), 2);

        // CDC 失效 orders 应命中此条目
        let removed = cache.invalidate_table("orders");
        assert_eq!(removed, 1);
    }

    // -----------------------------------------------------------------
    //  端到端集成测试
    // -----------------------------------------------------------------

    #[test]
    fn test_end_to_end_with_executor() {
        use szrsql_sql::executor::Executor;

        // 准备 catalog + 表数据
        let mut catalog = make_catalog();
        let _ = &mut catalog;
        let mut users_tbl = InMemoryTable::with_columns(
            "users",
            vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
        );
        users_tbl.insert(vec![Value::Int64(1), Value::Text("alice".into())]);
        users_tbl.insert(vec![Value::Int64(2), Value::Text("bob".into())]);
        users_tbl.insert(vec![Value::Int64(3), Value::Text("carol".into())]);

        let mut exec = Executor::new();
        exec.register_table(&users_tbl);

        let mut cache = ResultCache::new(10);
        let sql = "SELECT * FROM users";

        // 第一次查询：未命中缓存 → 执行查询 → 写入缓存
        let rows_first = if let Some(cached) = cache.get(sql) {
            cached.clone()
        } else {
            let plan = plan_sql(sql, &catalog);
            let rows = exec.execute(&plan).expect("execute failed");
            cache.insert_with_plan(sql, rows.clone(), &plan);
            rows
        };
        assert_eq!(rows_first.len(), 3);
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 0);

        // 第二次查询：命中缓存 → 跳过执行
        let rows_second = if let Some(cached) = cache.get(sql) {
            cached.clone()
        } else {
            let plan = plan_sql(sql, &catalog);
            let rows = exec.execute(&plan).expect("execute failed");
            cache.insert_with_plan(sql, rows.clone(), &plan);
            rows
        };
        assert_eq!(rows_second.len(), 3);
        assert_eq!(cache.stats().hits, 1);

        // 验证两次结果一致
        assert_eq!(rows_first, rows_second);

        // CDC 失效后应重新查询
        let removed = cache.invalidate_table("users");
        assert_eq!(removed, 1);
        assert!(cache.get(sql).is_none());
    }

    // -----------------------------------------------------------------
    //  容量边界测试
    // -----------------------------------------------------------------

    #[test]
    fn test_capacity_one() {
        let mut cache = ResultCache::new(1);
        let rows = make_rows(1);

        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);
        assert_eq!(cache.len(), 1);

        // 插入第二个，应淘汰第一个
        cache.insert("SELECT id FROM users", rows.clone(), vec!["users".into()]);
        assert_eq!(cache.len(), 1);
        assert!(cache.get("SELECT * FROM users").is_none());
        assert!(cache.get("SELECT id FROM users").is_some());
    }

    #[test]
    fn test_insert_many_beyond_capacity() {
        let mut cache = ResultCache::new(50);
        let rows = make_rows(1);

        // 插入 100 个结果，应只保留最后 50 个
        for i in 0..100 {
            let sql = format!("SELECT id FROM users WHERE id = {i}");
            cache.insert(&sql, rows.clone(), vec!["users".into()]);
        }

        assert_eq!(cache.len(), 50);
        assert_eq!(cache.stats().evictions, 50);

        // 前 50 个应被淘汰
        for i in 0..50 {
            let sql = format!("SELECT id FROM users WHERE id = {i}");
            assert!(cache.get(&sql).is_none(), "SQL #{i} 应被淘汰");
        }
        // 后 50 个应存在
        for i in 50..100 {
            let sql = format!("SELECT id FROM users WHERE id = {i}");
            assert!(cache.get(&sql).is_some(), "SQL #{i} 应保留");
        }
    }

    // -----------------------------------------------------------------
    //  一致性测试
    // -----------------------------------------------------------------

    #[test]
    fn test_repeated_access_consistency() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(5);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        // 多次访问，每次都应命中同一结果
        for _ in 0..100 {
            let cached = cache.get("SELECT * FROM users");
            assert!(cached.is_some());
            assert_eq!(cached.unwrap().len(), 5);
        }

        assert_eq!(cache.hit_count_of("SELECT * FROM users"), Some(100));
        assert_eq!(cache.stats().hits, 100);
        assert_eq!(cache.stats().misses, 0);
    }

    // -----------------------------------------------------------------
    //  contains 方法测试
    // -----------------------------------------------------------------

    #[test]
    fn test_contains_no_side_effects() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        // contains 不应影响 stats
        assert!(cache.contains("SELECT * FROM users"));
        assert!(!cache.contains("SELECT id FROM users"));

        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 0);
    }

    // -----------------------------------------------------------------
    //  row_count_of 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_row_count_of() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(42);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        assert_eq!(cache.row_count_of("SELECT * FROM users"), Some(42));
        assert_eq!(cache.row_count_of("SELECT id FROM users"), None);
    }

    // -----------------------------------------------------------------
    //  Debug 实现
    // -----------------------------------------------------------------

    #[test]
    fn test_debug_format() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        let debug_str = format!("{cache:?}");
        assert!(debug_str.contains("ResultCache"));
        assert!(debug_str.contains("entries"));
        assert!(debug_str.contains("capacity"));
    }

    // -----------------------------------------------------------------
    //  多表场景综合测试
    // -----------------------------------------------------------------

    #[test]
    fn test_multi_table_scenario() {
        let mut cache = ResultCache::new(20);
        let rows = make_rows(1);

        // 插入多个涉及不同表的结果
        let sqls = vec![
            ("SELECT * FROM users", vec!["users".to_string()]),
            ("SELECT * FROM orders", vec!["orders".to_string()]),
            (
                "SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id",
                vec!["users".to_string(), "orders".to_string()],
            ),
            (
                "SELECT u.id, o.amount FROM users u JOIN orders o ON u.id = o.user_id WHERE o.amount > 100",
                vec!["users".to_string(), "orders".to_string()],
            ),
            ("SELECT count(*) FROM users", vec!["users".to_string()]),
        ];

        for (sql, deps) in &sqls {
            cache.insert(sql, rows.clone(), deps.clone());
        }
        assert_eq!(cache.len(), 5);

        // 全部命中
        for (sql, _) in &sqls {
            assert!(cache.get(sql).is_some(), "应命中: {sql}");
        }
        assert_eq!(cache.stats().hits, 5);

        // CDC 变更 orders：失效 3 个（orders 单表 + 2 个 JOIN）
        let removed = cache.invalidate_table("orders");
        assert_eq!(removed, 3, "应失效 3 个依赖 orders 的结果");
        assert_eq!(cache.len(), 2);

        // users 相关的 2 个仍存在
        assert!(cache.get("SELECT * FROM users").is_some());
        assert!(cache.get("SELECT count(*) FROM users").is_some());
    }

    // -----------------------------------------------------------------
    //  表名规范化测试
    // -----------------------------------------------------------------

    #[test]
    fn test_table_name_normalization() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(1);
        cache.insert("SELECT * FROM users", rows.clone(), vec!["users".into()]);

        // 表名 Users 与 users 应一致失效
        let removed = cache.invalidate_table("Users");
        assert_eq!(removed, 1, "表名大小写应不影响失效");
    }

    // -----------------------------------------------------------------
    //  空结果缓存测试
    // -----------------------------------------------------------------

    #[test]
    fn test_empty_result_cached() {
        let mut cache = ResultCache::new(10);
        // 空结果也应被缓存（避免反复查询空表）
        cache.insert(
            "SELECT * FROM users WHERE id = 999",
            vec![],
            vec!["users".into()],
        );

        assert_eq!(
            cache.row_count_of("SELECT * FROM users WHERE id = 999"),
            Some(0)
        );
        let cached = cache.get("SELECT * FROM users WHERE id = 999");
        assert!(cached.is_some());
        assert!(cached.unwrap().is_empty());
    }

    // -----------------------------------------------------------------
    //  大结果集缓存测试
    // -----------------------------------------------------------------

    #[test]
    fn test_large_result_cached() {
        let mut cache = ResultCache::new(10);
        let rows = make_rows(10_000);
        cache.insert("SELECT * FROM big_table", rows, vec!["big_table".into()]);

        let cached = cache.get("SELECT * FROM big_table");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().len(), 10_000);
    }
}
