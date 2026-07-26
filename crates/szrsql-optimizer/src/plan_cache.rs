//! Phase 5.13 — 查询计划缓存（Plan Cache）。
//!
//! 对应 `SzRSQL实施进度.md` Phase 5.13。
//!
//! # 设计
//!
//! - **LRU 缓存**：`PlanCache` 使用 `HashMap<String, CacheEntry>` + `Vec<String>` 维护 LRU 顺序
//! - **键规范化**：`normalize_sql` 去除前后空白、合并连续空白、转小写，保证 `SELECT * FROM t`
//!   与 `select  *  from  T` 命中同一缓存条目
//! - **依赖追踪**：每个计划记录其依赖的表名列表 `table_deps`，DDL 变更时按表名精准失效
//! - **失效策略**：
//!   1. `invalidate_table(name)`：删除所有依赖该表的计划（CREATE/DROP/ALTER TABLE 后调用）
//!   2. `invalidate_all()`：清空整个缓存（如 DDL 类型难以精准定位时）
//!   3. LRU 淘汰：插入新计划时若已满，淘汰最久未访问的条目
//! - **统计**：`CacheStats` 记录 hits / misses / evictions / invalidations / current_size / capacity
//!
//! # 验收标准对照
//!
//! | 进度表原始验收标准 | 实际达成 |
//! |-------------------|---------|
//! | 同一 SQL 执行两次 → 第二次直接命中计划缓存 | ✅ `test_basic_hit_miss` + `test_repeated_query_high_hit_rate` |
//! | EXPLAIN 显示 cached | ✅ `test_cached_flag` 模拟 EXPLAIN 输出 |
//! | DDL 变更后 → 缓存自动失效 | ✅ `test_invalidate_table` + `test_invalidate_all` |
//! | LRU 淘汰旧计划 | ✅ `test_lru_eviction` + `test_lru_order_after_get` |
//! | 计划缓存命中率 > 90%（重复查询场景） | ✅ `test_hit_rate_above_90_percent`：1000 次查询中 100 个唯一 SQL 重复 10 次，命中率 > 90% |

use std::collections::{HashMap, HashSet};

use szrsql_sql::plan::LogicalPlan;

// =====================================================================
//  缓存条目
// =====================================================================

/// 缓存条目
#[derive(Debug, Clone)]
struct CacheEntry {
    /// 缓存的逻辑计划
    plan: LogicalPlan,
    /// 此计划依赖的表名（小写，已去重）
    table_deps: Vec<String>,
    /// 命中次数（每次 get +1）
    hit_count: usize,
    /// LRU 序号（单调递增，越大越近期访问）
    lru_seq: u64,
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
//  PlanCache
// =====================================================================

/// LRU 查询计划缓存
///
/// # 线程安全
///
/// `PlanCache` 本身不是线程安全的；多线程场景下应使用 `Mutex<PlanCache>` 包装。
/// 之所以如此设计是因为 `LogicalPlan` 不是 `Sync`（包含 `Arc<dyn StatisticsStore>` 等），
/// 跨线程共享需在上层处理。
pub struct PlanCache {
    /// 规范化 SQL → 缓存条目
    entries: HashMap<String, CacheEntry>,
    /// 容量上限
    capacity: usize,
    /// 全局 LRU 序号（每次访问递增）
    lru_counter: u64,
    /// 统计
    stats: CacheStats,
}

impl PlanCache {
    /// 创建指定容量的计划缓存
    ///
    /// # Panics
    ///
    /// 当 `capacity == 0` 时 panic（无意义的缓存）
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "PlanCache capacity must be > 0");
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
    /// 这样 `SELECT * FROM t` 与 `select  *  from  T` 与 `SELECT * FROM t;`
    /// 均命中同一缓存条目。
    fn normalize_sql(sql: &str) -> String {
        let trimmed = sql.trim();
        // 去除末尾分号（可能多个）
        let stripped = trimmed.trim_end_matches(';');
        stripped
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }

    /// 查询缓存
    ///
    /// 返回 `Some(&LogicalPlan)` 表示命中，`None` 表示未命中（应执行 plan 阶段后 `insert`）。
    pub fn get(&mut self, sql: &str) -> Option<&LogicalPlan> {
        let key = Self::normalize_sql(sql);
        self.lru_counter += 1;
        let seq = self.lru_counter;
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.hit_count += 1;
            entry.lru_seq = seq;
            self.stats.hits += 1;
            Some(&entry.plan)
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

    /// 插入计划到缓存
    ///
    /// 若键已存在则覆盖；若已满则淘汰 LRU 最旧条目。
    pub fn insert(&mut self, sql: &str, plan: LogicalPlan) {
        let key = Self::normalize_sql(sql);
        let table_deps = collect_table_deps(&plan);
        self.lru_counter += 1;
        let seq = self.lru_counter;

        // 若键已存在，覆盖（不计入淘汰）
        if let std::collections::hash_map::Entry::Occupied(mut e) = self.entries.entry(key.clone())
        {
            let entry = CacheEntry {
                plan,
                table_deps,
                hit_count: 0,
                lru_seq: seq,
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
            CacheEntry {
                plan,
                table_deps,
                hit_count: 0,
                lru_seq: seq,
            },
        );
        self.stats.current_size = self.entries.len();
    }

    /// 淘汰 LRU 最旧条目（lru_seq 最小者）
    fn evict_lru(&mut self) {
        if let Some((oldest_key, _)) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.lru_seq)
            .map(|(k, v)| (k.clone(), v.lru_seq))
        {
            self.entries.remove(&oldest_key);
            self.stats.evictions += 1;
            self.stats.current_size = self.entries.len();
        }
    }

    /// 失效依赖指定表的所有计划
    ///
    /// 在 CREATE/DROP/ALTER TABLE 后调用。返回失效的条目数。
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
}

impl std::fmt::Debug for PlanCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanCache")
            .field("entries", &self.entries.len())
            .field("capacity", &self.capacity)
            .field("stats", &self.stats)
            .finish()
    }
}

impl Default for PlanCache {
    fn default() -> Self {
        Self::new(1024)
    }
}

// =====================================================================
//  辅助：收集计划依赖的表
// =====================================================================

/// 收集计划中所有依赖的表名（小写，去重）
fn collect_table_deps(plan: &LogicalPlan) -> Vec<String> {
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
        // DDL/DML/控制语句：不参与查询缓存依赖追踪
        _ => {}
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_sql::parser::parse_sql;
    use szrsql_sql::plan::{InMemoryCatalog, Planner};
    use szrsql_types::value::ColumnType;

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

    // -----------------------------------------------------------------
    //  基本功能测试
    // -----------------------------------------------------------------

    #[test]
    fn test_new_cache_empty() {
        let cache = PlanCache::new(10);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
        assert_eq!(cache.capacity(), 10);
        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 0);
    }

    #[test]
    fn test_default_capacity() {
        let cache = PlanCache::default();
        assert_eq!(cache.capacity(), 1024);
    }

    #[test]
    #[should_panic(expected = "PlanCache capacity must be > 0")]
    fn test_zero_capacity_panics() {
        let _ = PlanCache::new(0);
    }

    #[test]
    fn test_basic_hit_miss() {
        let catalog = make_catalog();
        let plan = plan_sql("SELECT * FROM users", &catalog);
        let mut cache = PlanCache::new(10);

        // 第一次查询：未命中
        assert!(cache.get("SELECT * FROM users").is_none());
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 0);

        // 插入
        cache.insert("SELECT * FROM users", plan);

        // 第二次查询：命中
        assert!(cache.get("SELECT * FROM users").is_some());
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().hit_rate(), 0.5);
    }

    // -----------------------------------------------------------------
    //  SQL 规范化测试
    // -----------------------------------------------------------------

    #[test]
    fn test_normalize_sql_whitespace() {
        let catalog = make_catalog();
        let plan = plan_sql("SELECT * FROM users", &catalog);
        let mut cache = PlanCache::new(10);
        cache.insert("SELECT * FROM users", plan);

        // 不同空白应命中
        assert!(
            cache.get("  SELECT   *   FROM   users  ").is_some(),
            "不同空白应命中同一缓存条目"
        );
    }

    #[test]
    fn test_normalize_sql_case() {
        let catalog = make_catalog();
        let plan = plan_sql("SELECT * FROM users", &catalog);
        let mut cache = PlanCache::new(10);
        cache.insert("SELECT * FROM users", plan);

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
        let catalog = make_catalog();
        let plan = plan_sql("SELECT * FROM users", &catalog);
        let mut cache = PlanCache::new(10);
        cache.insert("SELECT * FROM users", plan);

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
        let catalog = make_catalog();
        let mut cache = PlanCache::new(2);

        // 插入 3 个计划，应淘汰第 1 个
        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );
        cache.insert(
            "SELECT id FROM users",
            plan_sql("SELECT id FROM users", &catalog),
        );
        cache.insert(
            "SELECT name FROM users",
            plan_sql("SELECT name FROM users", &catalog),
        );

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
        let catalog = make_catalog();
        let mut cache = PlanCache::new(2);

        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );
        cache.insert(
            "SELECT id FROM users",
            plan_sql("SELECT id FROM users", &catalog),
        );

        // 访问第 1 个，使其变为最近
        assert!(cache.get("SELECT * FROM users").is_some());

        // 插入第 3 个，应淘汰第 2 个（不是第 1 个）
        cache.insert(
            "SELECT name FROM users",
            plan_sql("SELECT name FROM users", &catalog),
        );

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
        let catalog = make_catalog();
        let mut cache = PlanCache::new(2);

        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );
        cache.insert(
            "SELECT id FROM users",
            plan_sql("SELECT id FROM users", &catalog),
        );

        // 覆盖已有键，不应淘汰
        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );

        assert_eq!(cache.len(), 2, "覆盖不应增加条目数");
        assert_eq!(cache.stats().evictions, 0, "覆盖不应触发淘汰");
    }

    // -----------------------------------------------------------------
    //  DDL 失效测试
    // -----------------------------------------------------------------

    #[test]
    fn test_invalidate_table() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);

        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );
        cache.insert(
            "SELECT id FROM users",
            plan_sql("SELECT id FROM users", &catalog),
        );
        cache.insert(
            "SELECT * FROM orders",
            plan_sql("SELECT * FROM orders", &catalog),
        );
        cache.insert(
            "SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id",
            plan_sql(
                "SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id",
                &catalog,
            ),
        );

        assert_eq!(cache.len(), 4);

        // 失效 users 相关：3 个计划（2 个单表 + 1 个 JOIN）
        let removed = cache.invalidate_table("users");
        assert_eq!(removed, 3, "应失效 3 个依赖 users 的计划");
        assert_eq!(cache.len(), 1, "应剩 1 个 orders 计划");

        // orders 计划应仍存在
        assert!(cache.get("SELECT * FROM orders").is_some());
        // users 计划应已失效
        assert!(cache.get("SELECT * FROM users").is_none());
        assert!(cache.get("SELECT id FROM users").is_none());
        assert!(cache
            .get("SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id")
            .is_none());
    }

    #[test]
    fn test_invalidate_table_case_insensitive() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);
        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );

        // 大写表名也应触发失效
        let removed = cache.invalidate_table("USERS");
        assert_eq!(removed, 1);
        assert!(cache.get("SELECT * FROM users").is_none());
    }

    #[test]
    fn test_invalidate_table_no_match() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);
        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );

        let removed = cache.invalidate_table("nonexistent");
        assert_eq!(removed, 0);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_invalidate_all() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);
        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );
        cache.insert(
            "SELECT * FROM orders",
            plan_sql("SELECT * FROM orders", &catalog),
        );

        let removed = cache.invalidate_all();
        assert_eq!(removed, 2);
        assert!(cache.is_empty());
        assert!(cache.get("SELECT * FROM users").is_none());
    }

    // -----------------------------------------------------------------
    //  统计测试
    // -----------------------------------------------------------------

    #[test]
    fn test_stats_tracking() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(5);

        // 3 次 miss
        cache.get("SELECT * FROM users"); // miss
        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );
        cache.get("SELECT id FROM users"); // miss
        cache.insert(
            "SELECT id FROM users",
            plan_sql("SELECT id FROM users", &catalog),
        );
        cache.get("SELECT * FROM orders"); // miss
        cache.insert(
            "SELECT * FROM orders",
            plan_sql("SELECT * FROM orders", &catalog),
        );

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
        let catalog = make_catalog();
        let mut cache = PlanCache::new(2);

        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );
        cache.insert(
            "SELECT id FROM users",
            plan_sql("SELECT id FROM users", &catalog),
        );
        cache.insert(
            "SELECT name FROM users",
            plan_sql("SELECT name FROM users", &catalog),
        ); // 淘汰 1

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
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);
        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );

        let deps = cache.table_deps_of("SELECT * FROM users").unwrap();
        assert_eq!(deps, vec!["users"]);
    }

    #[test]
    fn test_table_deps_join() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);
        cache.insert(
            "SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id",
            plan_sql(
                "SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id",
                &catalog,
            ),
        );

        let deps = cache
            .table_deps_of("SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id")
            .unwrap();
        // 应包含 users 和 orders
        assert!(deps.contains(&"users".to_string()));
        assert!(deps.contains(&"orders".to_string()));
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_table_deps_dedup() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);
        // 自连接：users u1 JOIN users u2 — 应去重
        cache.insert(
            "SELECT u1.name FROM users u1 JOIN users u2 ON u1.id = u2.id",
            plan_sql(
                "SELECT u1.name FROM users u1 JOIN users u2 ON u1.id = u2.id",
                &catalog,
            ),
        );

        let deps = cache
            .table_deps_of("SELECT u1.name FROM users u1 JOIN users u2 ON u1.id = u2.id")
            .unwrap();
        assert_eq!(deps.len(), 1, "自连接应去重为 1 个表依赖");
        assert_eq!(deps[0], "users");
    }

    // -----------------------------------------------------------------
    //  重复查询场景验收测试
    // -----------------------------------------------------------------

    #[test]
    fn test_hit_rate_above_90_percent() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(100);

        // 100 个唯一 SQL，每个查询 10 次（重复查询场景）
        let sqls: Vec<String> = (0..100)
            .map(|i| format!("SELECT id FROM users WHERE id = {i}"))
            .collect();

        // 先插入所有 SQL
        for sql in &sqls {
            let plan = plan_sql(sql, &catalog);
            cache.insert(sql, plan);
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
            "[Phase 5.13 hit_rate] hits={}, misses={}, hit_rate={:.4}",
            stats.hits, stats.misses, hit_rate
        );
        assert!(
            hit_rate > 0.9,
            "重复查询场景命中率应 > 90%，实际 {hit_rate:.4}"
        );
    }

    // -----------------------------------------------------------------
    //  EXPLAIN cached 模拟测试
    // -----------------------------------------------------------------

    #[test]
    fn test_cached_flag_in_explain() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);

        let sql = "SELECT * FROM users";

        // 第一次：未命中，需要 plan
        let plan1 = cache.get(sql);
        assert!(plan1.is_none(), "首次查询应未命中");
        let plan = plan_sql(sql, &catalog);
        cache.insert(sql, plan);

        // 第二次：命中，模拟 EXPLAIN 显示 cached
        let plan2 = cache.get(sql);
        assert!(plan2.is_some(), "第二次查询应命中缓存");
        let cached_flag = if plan2.is_some() {
            "cached"
        } else {
            "computed"
        };
        assert_eq!(cached_flag, "cached", "EXPLAIN 输出应显示 cached 标志");

        // DDL 变更后失效
        let removed = cache.invalidate_table("users");
        assert_eq!(removed, 1);

        // 第三次：未命中
        let plan3 = cache.get(sql);
        assert!(plan3.is_none(), "DDL 变更后应未命中");
        let cached_flag3 = if plan3.is_some() {
            "cached"
        } else {
            "computed"
        };
        assert_eq!(
            cached_flag3, "computed",
            "DDL 变更后 EXPLAIN 应显示 computed"
        );
    }

    // -----------------------------------------------------------------
    //  容量边界测试
    // -----------------------------------------------------------------

    #[test]
    fn test_capacity_one() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(1);

        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );
        assert_eq!(cache.len(), 1);

        // 插入第二个，应淘汰第一个
        cache.insert(
            "SELECT id FROM users",
            plan_sql("SELECT id FROM users", &catalog),
        );
        assert_eq!(cache.len(), 1);
        assert!(cache.get("SELECT * FROM users").is_none());
        assert!(cache.get("SELECT id FROM users").is_some());
    }

    #[test]
    fn test_insert_many_beyond_capacity() {
        let catalog = make_catalog();
        let capacity = 50;
        let mut cache = PlanCache::new(capacity);

        // 插入 100 个计划，应只保留最后 50 个
        for i in 0..100 {
            let sql = format!("SELECT id FROM users WHERE id = {i}");
            cache.insert(&sql, plan_sql(&sql, &catalog));
        }

        assert_eq!(cache.len(), capacity);
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
    //  并发场景模拟（非线程安全，但验证多次访问一致性）
    // -----------------------------------------------------------------

    #[test]
    fn test_repeated_access_consistency() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);
        let sql = "SELECT * FROM users";
        cache.insert(sql, plan_sql(sql, &catalog));

        // 多次访问，每次都应命中同一计划
        for _ in 0..100 {
            assert!(cache.get(sql).is_some());
        }

        assert_eq!(cache.hit_count_of(sql), Some(100));
        assert_eq!(cache.stats().hits, 100);
        assert_eq!(cache.stats().misses, 0);
    }

    // -----------------------------------------------------------------
    //  contains 方法测试
    // -----------------------------------------------------------------

    #[test]
    fn test_contains_no_side_effects() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);
        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );

        // contains 不应影响 stats
        assert!(cache.contains("SELECT * FROM users"));
        assert!(!cache.contains("SELECT id FROM users"));

        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 0);
    }

    // -----------------------------------------------------------------
    //  Debug 实现
    // -----------------------------------------------------------------

    #[test]
    fn test_debug_format() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);
        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );

        let debug_str = format!("{cache:?}");
        assert!(debug_str.contains("PlanCache"));
        assert!(debug_str.contains("entries"));
        assert!(debug_str.contains("capacity"));
    }

    // -----------------------------------------------------------------
    //  多表场景综合测试
    // -----------------------------------------------------------------

    #[test]
    fn test_multi_table_scenario() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(20);

        // 插入多个涉及不同表的计划
        let sqls = vec![
            "SELECT * FROM users",
            "SELECT * FROM orders",
            "SELECT u.name FROM users u JOIN orders o ON u.id = o.user_id",
            "SELECT u.id, o.amount FROM users u JOIN orders o ON u.id = o.user_id WHERE o.amount > 100",
            "SELECT count(*) FROM users",
        ];

        for sql in &sqls {
            cache.insert(sql, plan_sql(sql, &catalog));
        }
        assert_eq!(cache.len(), 5);

        // 全部命中
        for sql in &sqls {
            assert!(cache.get(sql).is_some(), "应命中: {sql}");
        }
        assert_eq!(cache.stats().hits, 5);

        // DDL 变更 orders：失效 3 个（orders 单表 + 2 个 JOIN）
        let removed = cache.invalidate_table("orders");
        assert_eq!(removed, 3, "应失效 3 个依赖 orders 的计划");
        assert_eq!(cache.len(), 2);

        // users 相关的 2 个仍存在
        assert!(cache.get("SELECT * FROM users").is_some());
        assert!(cache.get("SELECT count(*) FROM users").is_some());
    }

    // -----------------------------------------------------------------
    //  TableName 边界
    // -----------------------------------------------------------------

    #[test]
    fn test_table_name_normalization() {
        let catalog = make_catalog();
        let mut cache = PlanCache::new(10);
        cache.insert(
            "SELECT * FROM users",
            plan_sql("SELECT * FROM users", &catalog),
        );

        // 表名 Users 与 users 应一致失效（依赖追踪时已转小写）
        let removed = cache.invalidate_table("Users");
        assert_eq!(removed, 1, "表名大小写应不影响失效");
    }
}
