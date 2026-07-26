//! 自动清理（AutoVacuum）— Phase 7d.21
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.21 自动清理 (AutoVacuum) 设计。
//!
//! # 设计
//!
//! 借鉴 PostgreSQL AutoVacuum 守护进程：
//! - **自动 VACUUM 触发** — 当表的死元组数超过阈值时自动触发：
//!   `dead_tuples > vacuum_threshold + vacuum_scale_factor * total_tuples`
//! - **自动 ANALYZE 触发** — 当表自上次 ANALYZE 以来修改行数超过阈值时自动更新统计：
//!   `mod_since_analyze > analyze_threshold + analyze_scale_factor * total_tuples`
//! - **事务 ID 回卷预防** — 当 `xid_age(current, oldest) > freeze_max_age` 时强制 VACUUM，
//!   推进 `oldest_xid`，防止 32-bit XID 回卷导致数据可见性问题（对应 PostgreSQL
//!   `autovacuum_freeze_max_age`，默认 2 亿）。
//! - **调度器** — 周期性扫描所有表（`naptime_secs` 间隔），对需要清理的表执行
//!   VACUUM/ANALYZE；VACUUM 调用全局 `MvccManager::vacuum()` 回收 MVCC 状态，
//!   同时重置每表 `dead_tuples` 计数器；ANALYZE 生成/更新表统计信息。
//!
//! ## 验证标准
//!
//! - 插入+删除 100000 行 → autovacuum 触发 → 死元组回收
//! - ANALYZE 统计更新（`mod_since_analyze` 重置 + `TableStatistics` 生成）
//! - 事务 ID 回卷预防（`xid_age > freeze_max_age` 时强制 VACUUM）

use crate::mvcc::{MvccManager, VacuumStats};
use std::collections::{HashMap, HashSet};

// =====================================================================
//  常量 — 对应 PostgreSQL autovacuum_* 配置参数默认值
// =====================================================================

/// autovacuum 默认开启（PostgreSQL: on）
pub const DEFAULT_AUTOVACUUM_ENABLED: bool = true;

/// autovacuum_vacuum_threshold 默认值（PostgreSQL: 50）
pub const DEFAULT_VACUUM_THRESHOLD: u64 = 50;

/// autovacuum_vacuum_scale_factor 默认值（PostgreSQL: 0.2）
pub const DEFAULT_VACUUM_SCALE_FACTOR: f64 = 0.2;

/// autovacuum_analyze_threshold 默认值（PostgreSQL: 50）
pub const DEFAULT_ANALYZE_THRESHOLD: u64 = 50;

/// autovacuum_analyze_scale_factor 默认值（PostgreSQL: 0.1）
pub const DEFAULT_ANALYZE_SCALE_FACTOR: f64 = 0.1;

/// autovacuum_naptime 默认值（秒，PostgreSQL: 60s）
pub const DEFAULT_NAPTIME_SECS: u64 = 60;

/// 事务 ID 周期上限（2^31 - 1，PostgreSQL 使用 32-bit 有符号 XID）
pub const XID_MAX: u32 = i32::MAX as u32;

/// 事务 ID 回卷预防阈值（对应 PostgreSQL `autovacuum_freeze_max_age`）
pub const DEFAULT_FREEZE_MAX_AGE: u32 = 1_000_000;

// =====================================================================
//  AutoVacuumConfig — 自动清理配置
// =====================================================================

/// 自动清理配置 — 对应 PostgreSQL `autovacuum_*` GUC 参数
#[derive(Debug, Clone, PartialEq)]
pub struct AutoVacuumConfig {
    /// 是否启用 autovacuum（PostgreSQL: `autovacuum`）
    pub enabled: bool,
    /// VACUUM 触发阈值（最小死元组数，PostgreSQL: `autovacuum_vacuum_threshold`）
    pub vacuum_threshold: u64,
    /// VACUUM 触发比例因子（0.0 - 1.0，PostgreSQL: `autovacuum_vacuum_scale_factor`）
    pub vacuum_scale_factor: f64,
    /// ANALYZE 触发阈值（最小修改行数，PostgreSQL: `autovacuum_analyze_threshold`）
    pub analyze_threshold: u64,
    /// ANALYZE 触发比例因子（0.0 - 1.0，PostgreSQL: `autovacuum_analyze_scale_factor`）
    pub analyze_scale_factor: f64,
    /// 调度器检查间隔（秒，PostgreSQL: `autovacuum_naptime`）
    pub naptime_secs: u64,
    /// 事务 ID 回卷预防阈值（PostgreSQL: `autovacuum_freeze_max_age`）
    pub freeze_max_age: u32,
}

impl Default for AutoVacuumConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_AUTOVACUUM_ENABLED,
            vacuum_threshold: DEFAULT_VACUUM_THRESHOLD,
            vacuum_scale_factor: DEFAULT_VACUUM_SCALE_FACTOR,
            analyze_threshold: DEFAULT_ANALYZE_THRESHOLD,
            analyze_scale_factor: DEFAULT_ANALYZE_SCALE_FACTOR,
            naptime_secs: DEFAULT_NAPTIME_SECS,
            freeze_max_age: DEFAULT_FREEZE_MAX_AGE,
        }
    }
}

impl AutoVacuumConfig {
    /// 构造默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 计算表的 VACUUM 触发阈值（绝对死元组数）
    pub fn vacuum_threshold_for(&self, total_tuples: u64) -> u64 {
        self.vacuum_threshold + (self.vacuum_scale_factor * total_tuples as f64) as u64
    }

    /// 计算表的 ANALYZE 触发阈值（绝对修改行数）
    pub fn analyze_threshold_for(&self, total_tuples: u64) -> u64 {
        self.analyze_threshold + (self.analyze_scale_factor * total_tuples as f64) as u64
    }

    /// 是否应该对表执行 VACUUM
    pub fn should_vacuum(&self, stats: &TableStats) -> bool {
        self.enabled && stats.dead_tuples > self.vacuum_threshold_for(stats.total_tuples())
    }

    /// 是否应该对表执行 ANALYZE
    pub fn should_analyze(&self, stats: &TableStats) -> bool {
        self.enabled && stats.mod_since_analyze > self.analyze_threshold_for(stats.total_tuples())
    }

    /// 是否需要因事务 ID 回卷而强制 VACUUM
    pub fn needs_force_vacuum_for_wraparound(&self, current_xid: u32, oldest_xid: u32) -> bool {
        xid_age(current_xid, oldest_xid) > self.freeze_max_age
    }
}

// =====================================================================
//  XID 回卷辅助函数
// =====================================================================

/// 计算事务 ID "年龄"（age = current_xid - oldest_xid，使用 wrapping_sub 模拟回卷）
pub fn xid_age(current_xid: u32, oldest_xid: u32) -> u32 {
    current_xid.wrapping_sub(oldest_xid)
}

// =====================================================================
//  TableStats — 表统计信息（对应 pg_stat_user_tables 视图）
// =====================================================================

/// 表 ID
pub type TableId = u32;

/// 表统计信息 — 对应 PostgreSQL `pg_stat_user_tables` 视图
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableStats {
    pub table_id: TableId,
    pub live_tuples: u64,
    pub dead_tuples: u64,
    pub mod_since_analyze: u64,
    pub last_vacuum: u64,
    pub last_autovacuum: u64,
    pub last_analyze: u64,
    pub last_autoanalyze: u64,
    pub vacuum_count: u64,
    pub autovacuum_count: u64,
    pub analyze_count: u64,
    pub autoanalyze_count: u64,
}

impl TableStats {
    pub fn new(table_id: TableId) -> Self {
        Self {
            table_id,
            live_tuples: 0,
            dead_tuples: 0,
            mod_since_analyze: 0,
            last_vacuum: 0,
            last_autovacuum: 0,
            last_analyze: 0,
            last_autoanalyze: 0,
            vacuum_count: 0,
            autovacuum_count: 0,
            analyze_count: 0,
            autoanalyze_count: 0,
        }
    }

    pub fn total_tuples(&self) -> u64 {
        self.live_tuples + self.dead_tuples
    }

    pub fn record_insert(&mut self, count: u64) {
        self.live_tuples += count;
        self.mod_since_analyze += count;
    }

    pub fn record_delete(&mut self, count: u64) {
        self.live_tuples = self.live_tuples.saturating_sub(count);
        self.dead_tuples += count;
        self.mod_since_analyze += count;
    }

    pub fn record_update(&mut self, count: u64) {
        self.dead_tuples += count;
        self.live_tuples += count;
        self.mod_since_analyze += count;
    }

    pub fn vacuum_dead_tuples(&mut self) -> u64 {
        let reclaimed = self.dead_tuples;
        self.dead_tuples = 0;
        reclaimed
    }

    pub fn reset_mod_since_analyze(&mut self) {
        self.mod_since_analyze = 0;
    }

    pub fn mark_autovacuum(&mut self, timestamp: u64) {
        self.last_autovacuum = timestamp;
        self.autovacuum_count += 1;
    }

    pub fn mark_autoanalyze(&mut self, timestamp: u64) {
        self.last_autoanalyze = timestamp;
        self.autoanalyze_count += 1;
    }

    pub fn mark_vacuum(&mut self, timestamp: u64) {
        self.last_vacuum = timestamp;
        self.vacuum_count += 1;
    }

    pub fn mark_analyze(&mut self, timestamp: u64) {
        self.last_analyze = timestamp;
        self.analyze_count += 1;
    }
}

// =====================================================================
//  ColumnStats / TableStatistics — ANALYZE 生成的统计
// =====================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnStats {
    pub column_name: String,
    pub null_count: u64,
    pub distinct_count: u64,
}

impl ColumnStats {
    pub fn new(column_name: impl Into<String>, null_count: u64, distinct_count: u64) -> Self {
        Self {
            column_name: column_name.into(),
            null_count,
            distinct_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableStatistics {
    pub table_id: TableId,
    pub row_count_estimate: u64,
    pub column_stats: Vec<ColumnStats>,
    pub analyzed_at: u64,
}

impl TableStatistics {
    pub fn new(table_id: TableId, row_count_estimate: u64, analyzed_at: u64) -> Self {
        Self {
            table_id,
            row_count_estimate,
            column_stats: Vec::new(),
            analyzed_at,
        }
    }

    pub fn with_column(mut self, col: ColumnStats) -> Self {
        self.column_stats.push(col);
        self
    }

    pub fn column_count(&self) -> usize {
        self.column_stats.len()
    }
}

// =====================================================================
//  AutoVacuumReport — 自动清理执行报告
// =====================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VacuumedTable {
    pub table_id: TableId,
    pub dead_tuples_before: u64,
    pub dead_tuples_after: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzedTable {
    pub table_id: TableId,
    pub mod_since_analyze_before: u64,
    pub live_tuples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoVacuumReport {
    pub vacuumed_tables: Vec<VacuumedTable>,
    pub analyzed_tables: Vec<AnalyzedTable>,
    pub skipped_tables: usize,
    pub forced_vacuum: bool,
    pub global_vacuum_stats: VacuumStats,
    pub elapsed_ms: u64,
}

impl Default for AutoVacuumReport {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoVacuumReport {
    pub fn new() -> Self {
        Self {
            vacuumed_tables: Vec::new(),
            analyzed_tables: Vec::new(),
            skipped_tables: 0,
            forced_vacuum: false,
            global_vacuum_stats: VacuumStats {
                safe_xid: 0,
                vacuumed_committed: 0,
                vacuumed_aborted: 0,
                vacuumed_writes: 0,
                retained_active: 0,
                retained_committed: 0,
                retained_aborted: 0,
                retained_writes: 0,
            },
            elapsed_ms: 0,
        }
    }

    pub fn vacuumed_table_count(&self) -> usize {
        self.vacuumed_tables.len()
    }

    pub fn analyzed_table_count(&self) -> usize {
        self.analyzed_tables.len()
    }

    pub fn total_dead_tuples_reclaimed(&self) -> u64 {
        self.vacuumed_tables
            .iter()
            .map(|t| t.dead_tuples_before)
            .sum()
    }
}

// =====================================================================
//  AutoVacuumScheduler — 自动清理调度器
// =====================================================================

/// 自动清理调度器 — 对应 PostgreSQL autovacuum launcher + worker
///
/// 周期性扫描所有注册表，对满足触发条件的表执行 VACUUM/ANALYZE。
/// VACUUM 会调用全局 `MvccManager::vacuum()` 回收 MVCC 状态并重置每表死元组计数器；
/// ANALYZE 生成/更新表统计信息（`TableStatistics`）。
pub struct AutoVacuumScheduler {
    /// 自动清理配置
    pub config: AutoVacuumConfig,
    tables: HashMap<TableId, TableStats>,
    statistics: HashMap<TableId, TableStatistics>,
    last_run: u64,
    oldest_xid: u32,
    run_count: u64,
}

impl AutoVacuumScheduler {
    /// 构造自定义配置的调度器
    pub fn new(config: AutoVacuumConfig) -> Self {
        Self {
            config,
            tables: HashMap::new(),
            statistics: HashMap::new(),
            last_run: 0,
            oldest_xid: 0,
            run_count: 0,
        }
    }

    /// 构造默认配置的调度器
    pub fn with_default_config() -> Self {
        Self::new(AutoVacuumConfig::default())
    }

    /// 注册新表（若已存在则 noop）
    pub fn register_table(&mut self, table_id: TableId) {
        self.tables
            .entry(table_id)
            .or_insert_with(|| TableStats::new(table_id));
    }

    pub fn get_table_stats(&self, table_id: TableId) -> Option<&TableStats> {
        self.tables.get(&table_id)
    }

    pub fn get_table_stats_mut(&mut self, table_id: TableId) -> Option<&mut TableStats> {
        self.tables.get_mut(&table_id)
    }

    pub fn get_table_statistics(&self, table_id: TableId) -> Option<&TableStatistics> {
        self.statistics.get(&table_id)
    }

    pub fn all_tables(&self) -> &HashMap<TableId, TableStats> {
        &self.tables
    }

    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    pub fn last_run(&self) -> u64 {
        self.last_run
    }

    pub fn run_count(&self) -> u64 {
        self.run_count
    }

    pub fn oldest_xid(&self) -> u32 {
        self.oldest_xid
    }

    pub fn update_oldest_xid(&mut self, xid: u32) {
        self.oldest_xid = xid;
    }

    /// 记录插入（自动注册表）
    pub fn record_insert(&mut self, table_id: TableId, count: u64) {
        self.register_table(table_id);
        if let Some(stats) = self.tables.get_mut(&table_id) {
            stats.record_insert(count);
        }
    }

    /// 记录删除（自动注册表）
    pub fn record_delete(&mut self, table_id: TableId, count: u64) {
        self.register_table(table_id);
        if let Some(stats) = self.tables.get_mut(&table_id) {
            stats.record_delete(count);
        }
    }

    /// 记录更新（自动注册表）
    pub fn record_update(&mut self, table_id: TableId, count: u64) {
        self.register_table(table_id);
        if let Some(stats) = self.tables.get_mut(&table_id) {
            stats.record_update(count);
        }
    }

    /// 是否到达调度时间（`now - last_run >= naptime_secs`）
    pub fn should_run(&self, now: u64) -> bool {
        now.saturating_sub(self.last_run) >= self.config.naptime_secs
    }

    /// 是否需要因事务 ID 回卷而强制 VACUUM
    pub fn needs_force_vacuum_for_wraparound(&self, current_xid: u32) -> bool {
        self.config
            .needs_force_vacuum_for_wraparound(current_xid, self.oldest_xid)
    }

    /// 执行一轮 AutoVacuum
    ///
    /// - `now`: 当前时间戳（秒）
    /// - `current_xid`: 当前事务 ID（用于回卷检测）
    /// - `mgr`: MVCC 管理器（VACUUM 时调用 `mgr.vacuum()` 回收全局 MVCC 状态）
    pub fn run(&mut self, now: u64, current_xid: u32, mgr: &MvccManager) -> AutoVacuumReport {
        let mut report = AutoVacuumReport::new();

        let force = self.needs_force_vacuum_for_wraparound(current_xid);
        if force {
            report.forced_vacuum = true;
        }

        let mut need_vacuum: Vec<TableId> = Vec::new();
        let mut need_analyze: Vec<TableId> = Vec::new();
        for (table_id, stats) in &self.tables {
            if force || self.config.should_vacuum(stats) {
                need_vacuum.push(*table_id);
            }
            if self.config.should_analyze(stats) {
                need_analyze.push(*table_id);
            }
        }

        if !need_vacuum.is_empty() {
            report.global_vacuum_stats = mgr.vacuum();
        }

        for table_id in &need_vacuum {
            if let Some(stats) = self.tables.get_mut(table_id) {
                let dead_before = stats.dead_tuples;
                stats.vacuum_dead_tuples();
                stats.mark_autovacuum(now);
                report.vacuumed_tables.push(VacuumedTable {
                    table_id: *table_id,
                    dead_tuples_before: dead_before,
                    dead_tuples_after: stats.dead_tuples,
                });
            }
        }

        for table_id in &need_analyze {
            if let Some(stats) = self.tables.get_mut(table_id) {
                let mod_before = stats.mod_since_analyze;
                let live = stats.live_tuples;
                stats.reset_mod_since_analyze();
                stats.mark_autoanalyze(now);
                self.statistics
                    .insert(*table_id, TableStatistics::new(*table_id, live, now));
                report.analyzed_tables.push(AnalyzedTable {
                    table_id: *table_id,
                    mod_since_analyze_before: mod_before,
                    live_tuples: live,
                });
            }
        }

        let mut processed: HashSet<TableId> = HashSet::new();
        for t in &need_vacuum {
            processed.insert(*t);
        }
        for t in &need_analyze {
            processed.insert(*t);
        }
        report.skipped_tables = self.tables.len().saturating_sub(processed.len());

        let safe_xid = mgr.vacuum_safe_xid();
        if safe_xid > self.oldest_xid {
            self.oldest_xid = safe_xid;
        }

        self.last_run = now;
        self.run_count += 1;
        report
    }

    /// 强制对所有表执行 VACUUM（用于回卷预防或手动触发）
    pub fn force_vacuum_all(&mut self, now: u64, mgr: &MvccManager) -> AutoVacuumReport {
        let mut report = AutoVacuumReport::new();
        report.forced_vacuum = true;

        report.global_vacuum_stats = mgr.vacuum();

        let table_ids: Vec<TableId> = self.tables.keys().copied().collect();
        for table_id in table_ids {
            if let Some(stats) = self.tables.get_mut(&table_id) {
                let dead_before = stats.dead_tuples;
                stats.vacuum_dead_tuples();
                stats.mark_autovacuum(now);
                report.vacuumed_tables.push(VacuumedTable {
                    table_id,
                    dead_tuples_before: dead_before,
                    dead_tuples_after: stats.dead_tuples,
                });
            }
        }

        let safe_xid = mgr.vacuum_safe_xid();
        if safe_xid > self.oldest_xid {
            self.oldest_xid = safe_xid;
        }
        self.last_run = now;
        self.run_count += 1;
        report
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // AutoVacuumConfig
    // -----------------------------------------------------------------

    #[test]
    fn test_config_default_matches_postgres() {
        let cfg = AutoVacuumConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.vacuum_threshold, 50);
        assert!((cfg.vacuum_scale_factor - 0.2).abs() < 1e-9);
        assert_eq!(cfg.analyze_threshold, 50);
        assert!((cfg.analyze_scale_factor - 0.1).abs() < 1e-9);
        assert_eq!(cfg.naptime_secs, 60);
        assert_eq!(cfg.freeze_max_age, DEFAULT_FREEZE_MAX_AGE);
    }

    #[test]
    fn test_config_vacuum_threshold_for() {
        let cfg = AutoVacuumConfig::default();
        assert_eq!(cfg.vacuum_threshold_for(0), 50);
        assert_eq!(cfg.vacuum_threshold_for(1000), 250);
        assert_eq!(cfg.vacuum_threshold_for(100_000), 20_050);
    }

    #[test]
    fn test_config_analyze_threshold_for() {
        let cfg = AutoVacuumConfig::default();
        assert_eq!(cfg.analyze_threshold_for(0), 50);
        assert_eq!(cfg.analyze_threshold_for(1000), 150);
        assert_eq!(cfg.analyze_threshold_for(100_000), 10_050);
    }

    #[test]
    fn test_config_should_vacuum_below_threshold() {
        let cfg = AutoVacuumConfig::default();
        let mut stats = TableStats::new(1);
        stats.record_insert(1000);
        stats.record_delete(100); // dead=100 < 250
        assert!(!cfg.should_vacuum(&stats));
    }

    #[test]
    fn test_config_should_vacuum_above_threshold() {
        let cfg = AutoVacuumConfig::default();
        let mut stats = TableStats::new(1);
        stats.record_insert(1000);
        stats.record_delete(300); // dead=300 > 250
        assert!(cfg.should_vacuum(&stats));
    }

    #[test]
    fn test_config_should_analyze_below_threshold() {
        // total=50, analyze_threshold = 50 + 0.1*50 = 55; mod_since_analyze=50 < 55 → below
        let cfg = AutoVacuumConfig::default();
        let mut stats = TableStats::new(1);
        stats.record_insert(50);
        assert!(!cfg.should_analyze(&stats));
    }

    #[test]
    fn test_config_should_analyze_above_threshold() {
        let cfg = AutoVacuumConfig::default();
        let mut stats = TableStats::new(1);
        stats.record_insert(1000);
        stats.record_insert(200); // mod=1200 > 150
        assert!(cfg.should_analyze(&stats));
    }

    #[test]
    fn test_config_disabled_never_triggers() {
        let cfg = AutoVacuumConfig {
            enabled: false,
            ..Default::default()
        };
        let mut stats = TableStats::new(1);
        stats.record_insert(100_000);
        stats.record_delete(100_000);
        assert!(!cfg.should_vacuum(&stats));
        assert!(!cfg.should_analyze(&stats));
    }

    // -----------------------------------------------------------------
    // TableStats
    // -----------------------------------------------------------------

    #[test]
    fn test_table_stats_new() {
        let stats = TableStats::new(42);
        assert_eq!(stats.table_id, 42);
        assert_eq!(stats.live_tuples, 0);
        assert_eq!(stats.dead_tuples, 0);
        assert_eq!(stats.mod_since_analyze, 0);
        assert_eq!(stats.total_tuples(), 0);
    }

    #[test]
    fn test_table_stats_record_insert_delete_update() {
        let mut stats = TableStats::new(1);
        stats.record_insert(100);
        assert_eq!(stats.live_tuples, 100);
        assert_eq!(stats.dead_tuples, 0);
        assert_eq!(stats.mod_since_analyze, 100);

        stats.record_delete(30);
        assert_eq!(stats.live_tuples, 70);
        assert_eq!(stats.dead_tuples, 30);
        assert_eq!(stats.mod_since_analyze, 130);

        stats.record_update(20);
        assert_eq!(stats.live_tuples, 90);
        assert_eq!(stats.dead_tuples, 50);
        assert_eq!(stats.mod_since_analyze, 150);
        assert_eq!(stats.total_tuples(), 140);
    }

    #[test]
    fn test_table_stats_delete_saturating() {
        let mut stats = TableStats::new(1);
        stats.record_delete(100);
        assert_eq!(stats.live_tuples, 0);
        assert_eq!(stats.dead_tuples, 100);
    }

    #[test]
    fn test_table_stats_vacuum_dead_tuples() {
        let mut stats = TableStats::new(1);
        stats.record_insert(100);
        stats.record_delete(40);
        let reclaimed = stats.vacuum_dead_tuples();
        assert_eq!(reclaimed, 40);
        assert_eq!(stats.dead_tuples, 0);
        assert_eq!(stats.live_tuples, 60);
    }

    #[test]
    fn test_table_stats_reset_mod_since_analyze() {
        let mut stats = TableStats::new(1);
        stats.record_insert(100);
        stats.reset_mod_since_analyze();
        assert_eq!(stats.mod_since_analyze, 0);
        assert_eq!(stats.live_tuples, 100);
    }

    #[test]
    fn test_table_stats_mark_autovacuum_autoanalyze() {
        let mut stats = TableStats::new(1);
        stats.mark_autovacuum(1000);
        assert_eq!(stats.last_autovacuum, 1000);
        assert_eq!(stats.autovacuum_count, 1);
        stats.mark_autovacuum(2000);
        assert_eq!(stats.last_autovacuum, 2000);
        assert_eq!(stats.autovacuum_count, 2);
        stats.mark_autoanalyze(1500);
        assert_eq!(stats.last_autoanalyze, 1500);
        assert_eq!(stats.autoanalyze_count, 1);
    }

    // -----------------------------------------------------------------
    // xid_age / 回卷检测
    // -----------------------------------------------------------------

    #[test]
    fn test_xid_age_basic() {
        // 普通情况：current > oldest
        assert_eq!(xid_age(100, 10), 90);
        assert_eq!(xid_age(10, 10), 0);
        assert_eq!(xid_age(1_000_000, 0), 1_000_000);
    }

    #[test]
    fn test_xid_age_wraparound() {
        // 32-bit 回卷：oldest_xid 接近 u32::MAX，current_xid 已回卷到小值
        // oldest = u32::MAX - 100 = 4294967195, current = 200
        // age = 200u32.wrapping_sub(4294967195) = 200 + 101 = 301 (回卷后)
        let oldest = u32::MAX - 100;
        let current = 200u32;
        assert_eq!(xid_age(current, oldest), 301);

        // 完全回卷：oldest = u32::MAX, current = 0 → age = 1
        assert_eq!(xid_age(0, u32::MAX), 1);
        // oldest = u32::MAX, current = 1 → age = 2
        assert_eq!(xid_age(1, u32::MAX), 2);
    }

    #[test]
    fn test_needs_force_vacuum_below_threshold() {
        let cfg = AutoVacuumConfig::default();
        // freeze_max_age = 1_000_000
        // age = 999_999 < 1_000_000 → 不需要强制
        let oldest = 0u32;
        let current = 999_999u32;
        assert!(!cfg.needs_force_vacuum_for_wraparound(current, oldest));
    }

    #[test]
    fn test_needs_force_vacuum_above_threshold() {
        let cfg = AutoVacuumConfig::default();
        // age = 1_000_001 > 1_000_000 → 需要强制
        let oldest = 0u32;
        let current = 1_000_001u32;
        assert!(cfg.needs_force_vacuum_for_wraparound(current, oldest));
    }

    #[test]
    fn test_needs_force_vacuum_with_oldest_xid_nonzero() {
        let cfg = AutoVacuumConfig::default();
        // oldest = 100, current = 1_000_102 → age = 1_000_002 > 1_000_000 → 强制
        let oldest = 100u32;
        let current = 1_000_102u32;
        assert!(cfg.needs_force_vacuum_for_wraparound(current, oldest));

        // oldest = 100, current = 1_000_100 → age = 1_000_000 == freeze_max_age → 不强制（严格 >）
        let current2 = 1_000_100u32;
        assert!(!cfg.needs_force_vacuum_for_wraparound(current2, oldest));
    }

    // -----------------------------------------------------------------
    // ColumnStats / TableStatistics
    // -----------------------------------------------------------------

    #[test]
    fn test_column_stats_new() {
        let col = ColumnStats::new("user_id", 5, 1000);
        assert_eq!(col.column_name, "user_id");
        assert_eq!(col.null_count, 5);
        assert_eq!(col.distinct_count, 1000);
    }

    #[test]
    fn test_table_statistics_new() {
        let stats = TableStatistics::new(42, 5000, 1234);
        assert_eq!(stats.table_id, 42);
        assert_eq!(stats.row_count_estimate, 5000);
        assert_eq!(stats.analyzed_at, 1234);
        assert_eq!(stats.column_count(), 0);
        assert!(stats.column_stats.is_empty());
    }

    #[test]
    fn test_table_statistics_with_column_builder() {
        let stats = TableStatistics::new(1, 1000, 100)
            .with_column(ColumnStats::new("id", 0, 1000))
            .with_column(ColumnStats::new("name", 10, 990))
            .with_column(ColumnStats::new("email", 50, 950));
        assert_eq!(stats.column_count(), 3);
        assert_eq!(stats.column_stats[0].column_name, "id");
        assert_eq!(stats.column_stats[1].null_count, 10);
        assert_eq!(stats.column_stats[2].distinct_count, 950);
    }

    // -----------------------------------------------------------------
    // AutoVacuumReport
    // -----------------------------------------------------------------

    #[test]
    fn test_report_new_empty() {
        let report = AutoVacuumReport::new();
        assert_eq!(report.vacuumed_table_count(), 0);
        assert_eq!(report.analyzed_table_count(), 0);
        assert_eq!(report.skipped_tables, 0);
        assert!(!report.forced_vacuum);
        assert_eq!(report.elapsed_ms, 0);
        assert_eq!(report.total_dead_tuples_reclaimed(), 0);
        assert_eq!(report.global_vacuum_stats.total_vacuumed(), 0);
    }

    #[test]
    fn test_report_total_dead_tuples_reclaimed() {
        let mut report = AutoVacuumReport::new();
        report.vacuumed_tables.push(VacuumedTable {
            table_id: 1,
            dead_tuples_before: 100,
            dead_tuples_after: 0,
        });
        report.vacuumed_tables.push(VacuumedTable {
            table_id: 2,
            dead_tuples_before: 250,
            dead_tuples_after: 0,
        });
        assert_eq!(report.total_dead_tuples_reclaimed(), 350);
        assert_eq!(report.vacuumed_table_count(), 2);
    }

    #[test]
    fn test_report_default_is_empty() {
        let report = AutoVacuumReport::default();
        assert_eq!(report.vacuumed_table_count(), 0);
        assert_eq!(report.analyzed_table_count(), 0);
    }

    // -----------------------------------------------------------------
    // AutoVacuumScheduler — 基础操作
    // -----------------------------------------------------------------

    #[test]
    fn test_scheduler_new_with_config() {
        let cfg = AutoVacuumConfig {
            enabled: true,
            vacuum_threshold: 100,
            vacuum_scale_factor: 0.15,
            analyze_threshold: 80,
            analyze_scale_factor: 0.05,
            naptime_secs: 30,
            freeze_max_age: 500_000,
        };
        let sched = AutoVacuumScheduler::new(cfg);
        assert_eq!(sched.config.vacuum_threshold, 100);
        assert_eq!(sched.table_count(), 0);
        assert_eq!(sched.last_run(), 0);
        assert_eq!(sched.run_count(), 0);
        assert_eq!(sched.oldest_xid(), 0);
    }

    #[test]
    fn test_scheduler_with_default_config() {
        let sched = AutoVacuumScheduler::with_default_config();
        assert!(sched.config.enabled);
        assert_eq!(sched.config.vacuum_threshold, 50);
        assert_eq!(sched.table_count(), 0);
    }

    #[test]
    fn test_scheduler_register_table_idempotent() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        sched.register_table(1);
        sched.register_table(1); // 重复注册 noop
        sched.register_table(2);
        assert_eq!(sched.table_count(), 2);
        assert!(sched.get_table_stats(1).is_some());
        assert!(sched.get_table_stats(2).is_some());
        assert!(sched.get_table_stats(3).is_none());
    }

    #[test]
    fn test_scheduler_record_insert_delete_update_autoregisters() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        // record_insert 应自动注册表
        sched.record_insert(10, 100);
        assert_eq!(sched.table_count(), 1);
        let stats = sched.get_table_stats(10).unwrap();
        assert_eq!(stats.live_tuples, 100);
        assert_eq!(stats.mod_since_analyze, 100);

        // record_delete
        sched.record_delete(10, 30);
        let stats = sched.get_table_stats(10).unwrap();
        assert_eq!(stats.live_tuples, 70);
        assert_eq!(stats.dead_tuples, 30);
        assert_eq!(stats.mod_since_analyze, 130);

        // record_update
        sched.record_update(10, 20);
        let stats = sched.get_table_stats(10).unwrap();
        assert_eq!(stats.live_tuples, 90);
        assert_eq!(stats.dead_tuples, 50);
        assert_eq!(stats.mod_since_analyze, 150);

        // record_insert 新表
        sched.record_insert(20, 50);
        assert_eq!(sched.table_count(), 2);
    }

    #[test]
    fn test_scheduler_should_run_naptime() {
        let cfg = AutoVacuumConfig {
            naptime_secs: 60,
            ..Default::default()
        };
        let mut sched = AutoVacuumScheduler::new(cfg);

        // last_run=0, now=59 → 不应触发
        assert!(!sched.should_run(59));
        // now=60 → 应触发
        assert!(sched.should_run(60));
        // now=100 → 应触发
        assert!(sched.should_run(100));

        // 模拟一次 run 后，last_run 更新
        sched.last_run = 100;
        assert!(!sched.should_run(159));
        assert!(sched.should_run(160));
    }

    #[test]
    fn test_scheduler_update_oldest_xid() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        assert_eq!(sched.oldest_xid(), 0);
        sched.update_oldest_xid(100);
        assert_eq!(sched.oldest_xid(), 100);
        sched.update_oldest_xid(50); // 可以回退（测试用途）
        assert_eq!(sched.oldest_xid(), 50);
    }

    #[test]
    fn test_scheduler_needs_force_vacuum_for_wraparound() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        sched.update_oldest_xid(0);
        // current = 1_000_001 > freeze_max_age = 1_000_000 → 强制
        assert!(sched.needs_force_vacuum_for_wraparound(1_000_001));
        // current = 999_999 → 不强制
        assert!(!sched.needs_force_vacuum_for_wraparound(999_999));
    }

    #[test]
    fn test_scheduler_get_table_stats_mut() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        sched.register_table(1);
        if let Some(stats) = sched.get_table_stats_mut(1) {
            stats.record_insert(100);
        }
        assert_eq!(sched.get_table_stats(1).unwrap().live_tuples, 100);
    }

    #[test]
    fn test_scheduler_get_table_statistics_none_initially() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        sched.register_table(1); // register_table 不在 statistics 中创建
        assert!(sched.get_table_statistics(1).is_none());
    }

    #[test]
    fn test_scheduler_all_tables() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        sched.register_table(1);
        sched.register_table(2);
        sched.register_table(3);
        assert_eq!(sched.all_tables().len(), 3);
    }

    // -----------------------------------------------------------------
    // AutoVacuumScheduler.run() — 场景测试
    // -----------------------------------------------------------------

    #[test]
    fn test_run_no_work_when_below_thresholds() {
        // 表数据未达阈值，run 应 noop（skipped = 表数）
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        sched.record_insert(1, 50); // total=50, dead=0, mod=50
                                    // vacuum_threshold = 50 + 0.2*50 = 60; dead=0 < 60 → 不 VACUUM
                                    // analyze_threshold = 50 + 0.1*50 = 55; mod=50 < 55 → 不 ANALYZE

        let report = sched.run(60, 1, &mgr); // now=60 达到 naptime
        assert_eq!(report.vacuumed_table_count(), 0);
        assert_eq!(report.analyzed_table_count(), 0);
        assert_eq!(report.skipped_tables, 1);
        assert!(!report.forced_vacuum);
        assert_eq!(sched.run_count(), 1);
        assert_eq!(sched.last_run(), 60);
    }

    #[test]
    fn test_run_triggers_vacuum_when_dead_tuples_exceed() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // total=1000, vacuum_threshold = 50 + 0.2*1000 = 250
        // 死元组 = 300 > 250 → 触发 VACUUM
        sched.record_insert(1, 1000);
        sched.record_delete(1, 300);

        // 在 MVCC 中提交一些事务让 mgr.vacuum() 有事可做
        for i in 0..10 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        let report = sched.run(60, 1, &mgr);
        assert_eq!(report.vacuumed_table_count(), 1);
        assert_eq!(report.vacuumed_tables[0].table_id, 1);
        assert_eq!(report.vacuumed_tables[0].dead_tuples_before, 300);
        assert_eq!(report.vacuumed_tables[0].dead_tuples_after, 0);

        // 表统计应被更新
        let stats = sched.get_table_stats(1).unwrap();
        assert_eq!(stats.dead_tuples, 0);
        assert_eq!(stats.live_tuples, 700); // 1000 - 300
        assert_eq!(stats.autovacuum_count, 1);
        assert_eq!(stats.last_autovacuum, 60);

        // global_vacuum_stats 应反映 mgr.vacuum() 的结果
        assert_eq!(report.global_vacuum_stats.vacuumed_committed, 10);
    }

    #[test]
    fn test_run_triggers_analyze_when_mod_exceeds() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // total=1000, analyze_threshold = 50 + 0.1*1000 = 150
        // mod_since_analyze = 1200 > 150 → 触发 ANALYZE
        sched.record_insert(1, 1000);
        sched.record_insert(1, 200); // mod = 1200 > 150
                                     // dead = 0 < 250 → 不触发 VACUUM

        let report = sched.run(60, 1, &mgr);
        assert_eq!(report.vacuumed_table_count(), 0);
        assert_eq!(report.analyzed_table_count(), 1);
        assert_eq!(report.analyzed_tables[0].table_id, 1);
        assert_eq!(report.analyzed_tables[0].live_tuples, 1200);
        assert_eq!(report.analyzed_tables[0].mod_since_analyze_before, 1200);

        // 表统计应被更新
        let stats = sched.get_table_stats(1).unwrap();
        assert_eq!(stats.mod_since_analyze, 0); // 已重置
        assert_eq!(stats.autoanalyze_count, 1);
        assert_eq!(stats.last_autoanalyze, 60);

        // TableStatistics 应被生成
        let table_stats = sched.get_table_statistics(1).unwrap();
        assert_eq!(table_stats.table_id, 1);
        assert_eq!(table_stats.row_count_estimate, 1200);
        assert_eq!(table_stats.analyzed_at, 60);
    }

    #[test]
    fn test_run_triggers_both_vacuum_and_analyze() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // 同时满足 VACUUM 和 ANALYZE 条件
        sched.record_insert(1, 1000);
        sched.record_delete(1, 300); // dead=300 > 250 (VACUUM)
                                     // mod = 1300 > 150 (ANALYZE)

        let report = sched.run(60, 1, &mgr);
        assert_eq!(report.vacuumed_table_count(), 1);
        assert_eq!(report.analyzed_table_count(), 1);
        assert_eq!(report.skipped_tables, 0);

        // ANALYZE 时 live_tuples = 700（VACUUM 后未变 live，只清 dead）
        let stats = sched.get_table_stats(1).unwrap();
        assert_eq!(stats.live_tuples, 700);
        assert_eq!(stats.dead_tuples, 0);
        assert_eq!(stats.mod_since_analyze, 0);
        assert_eq!(stats.autovacuum_count, 1);
        assert_eq!(stats.autoanalyze_count, 1);
    }

    #[test]
    fn test_run_skipped_tables_count() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // 表 1 触发 VACUUM（dead=300 > 250）和 ANALYZE（mod=1300 > 150）
        sched.record_insert(1, 1000);
        sched.record_delete(1, 300);
        // 表 2 不触发任何操作
        sched.record_insert(2, 50);
        // 表 3 不触发任何操作
        sched.record_insert(3, 50);

        let report = sched.run(60, 1, &mgr);
        assert_eq!(report.vacuumed_table_count(), 1);
        assert_eq!(report.analyzed_table_count(), 1); // 表 1 mod=1300 > 150 也触发 ANALYZE
        assert_eq!(report.skipped_tables, 2);
    }

    #[test]
    fn test_run_idempotent_when_no_work() {
        // 连续两次 run，第二次应 noop
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        sched.record_insert(1, 1000);
        sched.record_delete(1, 300);

        let report1 = sched.run(60, 1, &mgr);
        assert_eq!(report1.vacuumed_table_count(), 1);

        // 第二次 run（naptime 后）应无工作（dead 已清零）
        let report2 = sched.run(120, 2, &mgr);
        assert_eq!(report2.vacuumed_table_count(), 0);
        assert_eq!(report2.analyzed_table_count(), 0);
        assert_eq!(report2.skipped_tables, 1);
    }

    #[test]
    fn test_run_disabled_config_never_triggers() {
        let cfg = AutoVacuumConfig {
            enabled: false,
            ..Default::default()
        };
        let mut sched = AutoVacuumScheduler::new(cfg);
        let mgr = MvccManager::new();

        // 即使数据远超阈值，因 enabled=false，不应触发
        sched.record_insert(1, 100_000);
        sched.record_delete(1, 100_000);

        let report = sched.run(60, 1, &mgr);
        assert_eq!(report.vacuumed_table_count(), 0);
        assert_eq!(report.analyzed_table_count(), 0);
        // skipped_tables 在 enabled=false 时仍计数（所有表都"被跳过"）
        assert_eq!(report.skipped_tables, 1);
    }

    // -----------------------------------------------------------------
    // AutoVacuumScheduler — 回卷预防（强制 VACUUM）
    // -----------------------------------------------------------------

    #[test]
    fn test_run_force_vacuum_on_wraparound_risk() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // 表 1 数据量小，正常情况下不会触发 VACUUM
        sched.record_insert(1, 50);
        sched.record_delete(1, 5); // dead=5 < 60 (vacuum_threshold_for(50)=60)

        // 但 oldest_xid=0, current_xid=1_000_001 > freeze_max_age=1_000_000 → 强制
        let report = sched.run(60, 1_000_001, &mgr);
        assert!(report.forced_vacuum);
        assert_eq!(report.vacuumed_table_count(), 1); // 强制模式下所有表都被 VACUUM
        assert_eq!(report.vacuumed_tables[0].table_id, 1);
        assert_eq!(report.vacuumed_tables[0].dead_tuples_before, 5);

        // oldest_xid 应被推进（mgr 为空时 vacuum_safe_xid 返回下一个待分配 txn_id = 1）
        // 由于 1 > 0（原 oldest_xid），oldest_xid 被更新为 1
        assert_eq!(sched.oldest_xid(), 1);
    }

    #[test]
    fn test_force_vacuum_all_processes_all_tables() {
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // 注册 3 个表，都不满足触发条件
        sched.record_insert(1, 10);
        sched.record_insert(2, 20);
        sched.record_insert(3, 30);

        // 在 mgr 中提交一些事务
        for i in 0..5 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        let report = sched.force_vacuum_all(100, &mgr);
        assert!(report.forced_vacuum);
        assert_eq!(report.vacuumed_table_count(), 3);
        assert_eq!(report.analyzed_table_count(), 0); // force_vacuum_all 不做 ANALYZE
        assert_eq!(report.skipped_tables, 0);
        assert_eq!(report.global_vacuum_stats.vacuumed_committed, 5);

        // 所有表的 autovacuum_count 应为 1
        for tid in [1, 2, 3] {
            let stats = sched.get_table_stats(tid).unwrap();
            assert_eq!(stats.autovacuum_count, 1);
            assert_eq!(stats.last_autovacuum, 100);
        }
    }

    #[test]
    fn test_force_vacuum_all_empty_scheduler() {
        // 无表时 force_vacuum_all 应返回空报告（但仍调用 mgr.vacuum）
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        let report = sched.force_vacuum_all(100, &mgr);
        assert!(report.forced_vacuum);
        assert_eq!(report.vacuumed_table_count(), 0);
        assert_eq!(report.skipped_tables, 0);
        assert_eq!(sched.run_count(), 1);
        assert_eq!(sched.last_run(), 100);
    }

    // -----------------------------------------------------------------
    // 端到端测试 — 验证 Phase 7d.21 验证标准
    // -----------------------------------------------------------------

    #[test]
    fn test_end_to_end_100k_rows_autovacuum() {
        // 验证标准：插入+删除 100000 行 → autovacuum 触发 → 死元组回收
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // 插入 100000 行
        sched.record_insert(1, 100_000);
        // 删除 50000 行（产生 50000 死元组）
        sched.record_delete(1, 50_000);

        // 验证触发条件：vacuum_threshold_for(100000) = 50 + 0.2*100000 = 20050
        // dead=50000 > 20050 → 应触发
        assert!(sched
            .config
            .should_vacuum(sched.get_table_stats(1).unwrap()));

        // 在 MVCC 中创建对应事务（让 mgr.vacuum 有事可做）
        for i in 0..100 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        let report = sched.run(60, 1, &mgr);

        // 验证死元组被回收
        assert_eq!(report.vacuumed_table_count(), 1);
        assert_eq!(report.vacuumed_tables[0].dead_tuples_before, 50_000);
        assert_eq!(report.vacuumed_tables[0].dead_tuples_after, 0);
        assert_eq!(report.total_dead_tuples_reclaimed(), 50_000);

        // 验证表统计
        let stats = sched.get_table_stats(1).unwrap();
        assert_eq!(stats.dead_tuples, 0);
        assert_eq!(stats.live_tuples, 50_000);
        assert_eq!(stats.autovacuum_count, 1);

        // 验证全局 MVCC VACUUM 也执行了
        assert_eq!(report.global_vacuum_stats.vacuumed_committed, 100);

        // mod_since_analyze = 150000 > analyze_threshold_for(100000)=10050 → 也应触发 ANALYZE
        // 注：run 会在同一轮中既 VACUUM 又 ANALYZE
        assert_eq!(report.analyzed_table_count(), 1);
        assert_eq!(report.analyzed_tables[0].live_tuples, 50_000);
    }

    #[test]
    fn test_end_to_end_wraparound_prevention() {
        // 验证标准：事务 ID 回卷预防
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // 模拟 XID 接近回卷：oldest_xid=0, current_xid=1_000_001 > freeze_max_age=1_000_000
        sched.record_insert(1, 100);
        sched.record_delete(1, 1); // dead=1, 远低于阈值（vacuum_threshold_for(100)=70）

        // 正常情况下不会触发 VACUUM
        assert!(!sched
            .config
            .should_vacuum(sched.get_table_stats(1).unwrap()));

        // 但因回卷风险，应强制 VACUUM
        assert!(sched.needs_force_vacuum_for_wraparound(1_000_001));

        // 在 mgr 中提交一些事务
        for i in 0..20 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }

        let report = sched.run(60, 1_000_001, &mgr);
        assert!(report.forced_vacuum);
        assert_eq!(report.vacuumed_table_count(), 1);
        assert_eq!(report.global_vacuum_stats.vacuumed_committed, 20);

        // 验证 oldest_xid 被推进
        // mgr.vacuum_safe_xid() 在无活跃事务时返回当前最大已分配 txn_id+1
        // 20 个事务后，txn_id_alloc = 21，safe_xid 应为 21
        assert_eq!(sched.oldest_xid(), 21);
    }

    #[test]
    fn test_end_to_end_repeated_cycles() {
        // 验证多轮 AutoVacuum 循环：插入→删除→autovacuum→再插入→再删除→autovacuum
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // 第一轮：插入 1000，删除 300
        // live: 0 + 1000 - 300 = 700; dead: 300; mod: 1300
        sched.record_insert(1, 1000);
        sched.record_delete(1, 300);
        let report1 = sched.run(60, 1, &mgr);
        assert_eq!(report1.vacuumed_table_count(), 1);
        assert_eq!(sched.get_table_stats(1).unwrap().autovacuum_count, 1);
        assert_eq!(sched.get_table_stats(1).unwrap().live_tuples, 700);
        assert_eq!(sched.get_table_stats(1).unwrap().dead_tuples, 0);

        // 第二轮：再插入 500，删除 200
        // live: 700 + 500 - 200 = 1000; dead: 200; mod: 700
        // vacuum_threshold_for(1200) = 50 + 240 = 290; dead=200 < 290 → 不触发 VACUUM
        sched.record_insert(1, 500);
        sched.record_delete(1, 200);
        let report2 = sched.run(120, 2, &mgr);
        assert_eq!(report2.vacuumed_table_count(), 0);

        // 第三轮：再删除 200（dead 累积到 400 > 290）
        // live: 1000 - 200 = 800; dead: 200 + 200 = 400; mod: 0 + 200 = 200
        sched.record_delete(1, 200);
        let report3 = sched.run(180, 3, &mgr);
        assert_eq!(report3.vacuumed_table_count(), 1);
        assert_eq!(sched.get_table_stats(1).unwrap().autovacuum_count, 2);
        assert_eq!(sched.get_table_stats(1).unwrap().dead_tuples, 0);
        assert_eq!(sched.get_table_stats(1).unwrap().live_tuples, 800);

        // 验证 run_count
        assert_eq!(sched.run_count(), 3);
        assert_eq!(sched.last_run(), 180);
    }

    #[test]
    fn test_end_to_end_multiple_tables_independent() {
        // 多表独立触发：表 1 触发 VACUUM，表 2 触发 ANALYZE，表 3 跳过
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // 表 1：触发 VACUUM（dead=300 > threshold_for(1000)=250）
        sched.record_insert(1, 1000);
        sched.record_delete(1, 300);
        // mod_since_analyze=1300 > 150 → 也触发 ANALYZE

        // 表 2：只触发 ANALYZE（mod 大但 dead 小）
        sched.record_insert(2, 1000);
        sched.record_insert(2, 200); // mod=1200 > 150, dead=0 < 250

        // 表 3：都不触发
        sched.record_insert(3, 50); // mod=50 < 55, dead=0 < 60

        let report = sched.run(60, 1, &mgr);

        assert_eq!(report.vacuumed_table_count(), 1);
        assert_eq!(report.vacuumed_tables[0].table_id, 1);

        assert_eq!(report.analyzed_table_count(), 2);
        let analyzed_ids: Vec<_> = report.analyzed_tables.iter().map(|t| t.table_id).collect();
        assert!(analyzed_ids.contains(&1));
        assert!(analyzed_ids.contains(&2));

        assert_eq!(report.skipped_tables, 1); // 表 3
    }

    #[test]
    fn test_end_to_end_analyze_generates_statistics() {
        // 验证 ANALYZE 后 TableStatistics 被正确生成并可查询
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        sched.record_insert(1, 1000);
        sched.record_insert(1, 200); // mod=1200 > 150

        let report = sched.run(60, 1, &mgr);
        assert_eq!(report.analyzed_table_count(), 1);

        // 验证 statistics 中有对应条目
        let stats = sched.get_table_statistics(1);
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert_eq!(stats.table_id, 1);
        assert_eq!(stats.row_count_estimate, 1200);
        assert_eq!(stats.analyzed_at, 60);
        assert_eq!(stats.column_count(), 0); // 默认无列统计
    }

    #[test]
    fn test_end_to_end_vacuum_advances_oldest_xid() {
        // 验证 VACUUM 后 oldest_xid 被推进（基于 mgr.vacuum_safe_xid）
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        // 在 mgr 中创建并提交事务
        for i in 0..50 {
            let txn = mgr.begin();
            let _ = mgr.register_write(txn.txn_id, format!("k{i}"));
            mgr.commit(txn.txn_id, 0).unwrap();
        }
        // 50 个事务后，txn_id_alloc = 51，无活跃事务时 vacuum_safe_xid = 51

        sched.record_insert(1, 1000);
        sched.record_delete(1, 300);

        assert_eq!(sched.oldest_xid(), 0);

        let _report = sched.run(60, 1, &mgr);

        // vacuum_safe_xid 应为 51（无活跃事务，safe_xid = next_txn_id = 51）
        assert_eq!(sched.oldest_xid(), 51);
    }

    #[test]
    fn test_end_to_end_force_vacuum_does_not_analyze() {
        // 验证 force_vacuum_all 只做 VACUUM 不做 ANALYZE
        let mut sched = AutoVacuumScheduler::with_default_config();
        let mgr = MvccManager::new();

        sched.record_insert(1, 1000);
        sched.record_insert(1, 200); // mod=1200 > 150，但 force_vacuum_all 不 ANALYZE

        let report = sched.force_vacuum_all(100, &mgr);
        assert!(report.forced_vacuum);
        assert_eq!(report.vacuumed_table_count(), 1);
        assert_eq!(report.analyzed_table_count(), 0);

        // mod_since_analyze 应保持不变（未被重置）
        let stats = sched.get_table_stats(1).unwrap();
        assert_eq!(stats.mod_since_analyze, 1200);
        assert_eq!(stats.autoanalyze_count, 0);
    }

    #[test]
    fn test_end_to_end_wraparound_with_32bit_overflow() {
        // 验证 32-bit XID 回卷场景下的强制 VACUUM
        let cfg = AutoVacuumConfig {
            freeze_max_age: 100, // 降低阈值便于测试
            ..Default::default()
        };
        let mut sched = AutoVacuumScheduler::new(cfg);
        let mgr = MvccManager::new();

        // 模拟回卷：oldest_xid 接近 u32::MAX，current_xid 已回卷到小值
        let oldest = u32::MAX - 50; // 4294967245
        sched.update_oldest_xid(oldest);
        let current = 100u32; // 回卷后 age = 100 + 51 = 151 > 100

        assert!(sched.needs_force_vacuum_for_wraparound(current));

        sched.record_insert(1, 10);
        sched.record_delete(1, 1);

        let report = sched.run(60, current, &mgr);
        assert!(report.forced_vacuum);
        assert_eq!(report.vacuumed_table_count(), 1);
    }
}
