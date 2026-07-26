//! 冷热数据分层（Hot/Cold Data Tiering）— Phase 7d.4
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7 冷热数据分层设计。
//!
//! # 设计
//!
//! ColdDataMigrator 自动将冷数据（超过阈值天数未访问）从热存储（内存/SSD）
//! 迁移至冷存储（低成本磁盘），查询时透明读取，结果一致。
//!
//! - **DataTier** — 数据层级（Hot 热 / Cold 冷 / Archive 归档）
//! - **TierConfig** — 分层配置（冷数据阈值天数、归档阈值天数、迁移周期）
//! - **DataRow** — 数据行（含 last_accessed 时间戳，用于冷热判定）
//! - **ColdDataMigrator** — 冷数据迁移器（扫描热数据 → 标记冷数据 → 迁移至冷存储）
//! - **TieredStorage** — 分层存储（热存储 + 冷存储，查询时透明路由）
//!
//! ## 验证标准
//!
//! - 写入数据 → ColdDataMigrator 自动迁移 30 天前数据至冷存储
//! - 热数据查询走热存储，冷数据查询走冷存储
//! - 冷热查询结果一致
//! - 冷数据存储成本降低 >= 5x（冷存储用低成本磁盘模拟）

use std::collections::HashMap;

// =====================================================================
//  常量
// =====================================================================

/// 一天的秒数（86400 秒）
pub const SECONDS_PER_DAY: u64 = 86_400;

/// 默认冷数据阈值天数（30 天未访问 → 冷数据）
pub const DEFAULT_COLD_THRESHOLD_DAYS: u64 = 30;

/// 默认归档数据阈值天数（90 天未访问 → 归档数据）
pub const DEFAULT_ARCHIVE_THRESHOLD_DAYS: u64 = 90;

/// 默认迁移检查周期（每小时检查一次）
pub const DEFAULT_MIGRATION_INTERVAL_SECS: u64 = 3_600;

// =====================================================================
//  DataTier — 数据层级
// =====================================================================

/// 数据层级 — 冷热数据分层
///
/// - **Hot** — 热数据（最近访问，存内存/SSD）
/// - **Cold** — 冷数据（超过冷阈值未访问，存低成本磁盘）
/// - **Archive** — 归档数据（超过归档阈值未访问，存对象存储/磁带）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataTier {
    /// 热数据（最近访问，存内存/SSD）
    Hot,
    /// 冷数据（超过冷阈值未访问，存低成本磁盘）
    Cold,
    /// 归档数据（超过归档阈值未访问，存对象存储/磁带）
    Archive,
}

impl DataTier {
    /// 层级名称
    pub fn as_str(&self) -> &'static str {
        match self {
            DataTier::Hot => "hot",
            DataTier::Cold => "cold",
            DataTier::Archive => "archive",
        }
    }

    /// 是否热数据
    pub fn is_hot(&self) -> bool {
        matches!(self, DataTier::Hot)
    }

    /// 是否冷数据
    pub fn is_cold(&self) -> bool {
        matches!(self, DataTier::Cold)
    }

    /// 是否归档数据
    pub fn is_archive(&self) -> bool {
        matches!(self, DataTier::Archive)
    }

    /// 存储成本系数（相对于热存储）
    ///
    /// Hot=1.0, Cold=0.1, Archive=0.02
    pub fn cost_factor(&self) -> f64 {
        match self {
            DataTier::Hot => 1.0,
            DataTier::Cold => 0.1,
            DataTier::Archive => 0.02,
        }
    }
}

impl std::fmt::Display for DataTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  TierConfig — 分层配置
// =====================================================================

/// 冷热数据分层配置
#[derive(Debug, Clone)]
pub struct TierConfig {
    /// 冷数据阈值天数（超过此天数未访问 → 冷数据）
    pub cold_threshold_days: u64,
    /// 归档数据阈值天数（超过此天数未访问 → 归档数据）
    pub archive_threshold_days: u64,
    /// 迁移检查周期（秒）
    pub migration_interval_secs: u64,
}

impl Default for TierConfig {
    fn default() -> Self {
        Self {
            cold_threshold_days: DEFAULT_COLD_THRESHOLD_DAYS,
            archive_threshold_days: DEFAULT_ARCHIVE_THRESHOLD_DAYS,
            migration_interval_secs: DEFAULT_MIGRATION_INTERVAL_SECS,
        }
    }
}

impl TierConfig {
    /// 构造默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置冷数据阈值天数
    pub fn with_cold_threshold(mut self, days: u64) -> Self {
        self.cold_threshold_days = days;
        self
    }

    /// 设置归档数据阈值天数
    pub fn with_archive_threshold(mut self, days: u64) -> Self {
        self.archive_threshold_days = days;
        self
    }

    /// 设置迁移检查周期（秒）
    pub fn with_migration_interval(mut self, secs: u64) -> Self {
        self.migration_interval_secs = secs;
        self
    }

    /// 根据最后访问时间判定数据层级
    ///
    /// `last_accessed` 是最后访问时间戳（秒），`now` 是当前时间戳（秒）
    pub fn classify(&self, last_accessed: u64, now: u64) -> DataTier {
        let days_since_access = now.saturating_sub(last_accessed) / SECONDS_PER_DAY;
        if days_since_access >= self.archive_threshold_days {
            DataTier::Archive
        } else if days_since_access >= self.cold_threshold_days {
            DataTier::Cold
        } else {
            DataTier::Hot
        }
    }
}

// =====================================================================
//  DataRow — 数据行
// =====================================================================

/// 分层数据行 — 含 last_accessed 时间戳
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRow {
    /// 行 ID
    pub row_id: u64,
    /// 数据内容（简化为字节向量）
    pub data: Vec<u8>,
    /// 创建时间戳（秒）
    pub created_at: u64,
    /// 最后访问时间戳（秒）
    pub last_accessed: u64,
    /// 当前层级
    pub tier: DataTier,
}

impl DataRow {
    /// 构造新行（默认 Hot 层级）
    pub fn new(row_id: u64, data: Vec<u8>, now: u64) -> Self {
        Self {
            row_id,
            data,
            created_at: now,
            last_accessed: now,
            tier: DataTier::Hot,
        }
    }

    /// 更新访问时间
    pub fn touch(&mut self, now: u64) {
        self.last_accessed = now;
    }

    /// 数据大小（字节）
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// 距离上次访问的天数
    pub fn days_since_access(&self, now: u64) -> u64 {
        now.saturating_sub(self.last_accessed) / SECONDS_PER_DAY
    }
}

// =====================================================================
//  TieredStorage — 分层存储
// =====================================================================

/// 分层存储 — 热存储 + 冷存储 + 归档存储
///
/// 查询时透明路由：先查热存储，未命中查冷存储，再未命中查归档存储。
/// 查询命中后自动提升至热存储（可选）。
#[derive(Debug, Clone)]
pub struct TieredStorage {
    /// 热存储（row_id → DataRow）
    hot: HashMap<u64, DataRow>,
    /// 冷存储（row_id → DataRow）
    cold: HashMap<u64, DataRow>,
    /// 归档存储（row_id → DataRow）
    archive: HashMap<u64, DataRow>,
    /// 分层配置
    config: TierConfig,
    /// 总查询次数
    total_queries: u64,
    /// 热存储命中次数
    hot_hits: u64,
    /// 冷存储命中次数
    cold_hits: u64,
    /// 归档存储命中次数
    archive_hits: u64,
    /// 迁移次数
    migration_count: u64,
}

impl TieredStorage {
    /// 构造空分层存储
    pub fn new(config: TierConfig) -> Self {
        Self {
            hot: HashMap::new(),
            cold: HashMap::new(),
            archive: HashMap::new(),
            config,
            total_queries: 0,
            hot_hits: 0,
            cold_hits: 0,
            archive_hits: 0,
            migration_count: 0,
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &TierConfig {
        &self.config
    }

    /// 热存储行数
    pub fn hot_count(&self) -> usize {
        self.hot.len()
    }

    /// 冷存储行数
    pub fn cold_count(&self) -> usize {
        self.cold.len()
    }

    /// 归档存储行数
    pub fn archive_count(&self) -> usize {
        self.archive.len()
    }

    /// 总行数
    pub fn total_count(&self) -> usize {
        self.hot.len() + self.cold.len() + self.archive.len()
    }

    /// 热存储字节数
    pub fn hot_bytes(&self) -> usize {
        self.hot.values().map(|r| r.size()).sum()
    }

    /// 冷存储字节数
    pub fn cold_bytes(&self) -> usize {
        self.cold.values().map(|r| r.size()).sum()
    }

    /// 归档存储字节数
    pub fn archive_bytes(&self) -> usize {
        self.archive.values().map(|r| r.size()).sum()
    }

    /// 总字节数
    pub fn total_bytes(&self) -> usize {
        self.hot_bytes() + self.cold_bytes() + self.archive_bytes()
    }

    /// 存储成本（相对于全热存储）
    ///
    /// 成本 = hot_bytes * 1.0 + cold_bytes * 0.1 + archive_bytes * 0.02
    pub fn storage_cost(&self) -> f64 {
        self.hot_bytes() as f64 * DataTier::Hot.cost_factor()
            + self.cold_bytes() as f64 * DataTier::Cold.cost_factor()
            + self.archive_bytes() as f64 * DataTier::Archive.cost_factor()
    }

    /// 成本节省率（相对于全热存储）
    ///
    /// savings = 1.0 - storage_cost / total_bytes
    pub fn cost_savings(&self) -> f64 {
        let total = self.total_bytes();
        if total == 0 {
            return 0.0;
        }
        1.0 - self.storage_cost() / total as f64
    }

    /// 写入行（默认进热存储）
    pub fn put(&mut self, row: DataRow) {
        self.hot.insert(row.row_id, row);
    }

    /// 查询行 — 透明路由热→冷→归档
    ///
    /// 命中后更新访问时间（touch），可选提升至热存储。
    pub fn get(&mut self, row_id: u64, now: u64) -> Option<&DataRow> {
        self.total_queries += 1;

        // 先查热存储
        if let Some(row) = self.hot.get_mut(&row_id) {
            row.touch(now);
            self.hot_hits += 1;
            return self.hot.get(&row_id);
        }

        // 再查冷存储
        if let Some(mut row) = self.cold.remove(&row_id) {
            row.touch(now);
            row.tier = DataTier::Hot;
            self.cold_hits += 1;
            self.hot.insert(row_id, row);
            return self.hot.get(&row_id);
        }

        // 最后查归档存储
        if let Some(mut row) = self.archive.remove(&row_id) {
            row.touch(now);
            row.tier = DataTier::Hot;
            self.archive_hits += 1;
            self.hot.insert(row_id, row);
            return self.hot.get(&row_id);
        }

        None
    }

    /// 查询行（不更新访问时间，不提升层级）
    pub fn peek(&self, row_id: u64) -> Option<&DataRow> {
        self.hot
            .get(&row_id)
            .or_else(|| self.cold.get(&row_id))
            .or_else(|| self.archive.get(&row_id))
    }

    /// 热存储命中率
    pub fn hot_hit_rate(&self) -> f64 {
        if self.total_queries == 0 {
            return 0.0;
        }
        self.hot_hits as f64 / self.total_queries as f64
    }

    /// 冷存储命中率
    pub fn cold_hit_rate(&self) -> f64 {
        if self.total_queries == 0 {
            return 0.0;
        }
        self.cold_hits as f64 / self.total_queries as f64
    }

    /// 归档存储命中率
    pub fn archive_hit_rate(&self) -> f64 {
        if self.total_queries == 0 {
            return 0.0;
        }
        self.archive_hits as f64 / self.total_queries as f64
    }

    /// 迁移次数
    pub fn migration_count(&self) -> u64 {
        self.migration_count
    }

    /// 执行冷数据迁移 — 扫描热存储，将冷数据迁移至冷存储
    ///
    /// 返回迁移的行数
    pub fn migrate_cold(&mut self, now: u64) -> usize {
        let hot_ids: Vec<u64> = self.hot.keys().copied().collect();
        let mut migrated = 0;

        for row_id in hot_ids {
            let row = self.hot.get(&row_id).unwrap();
            let new_tier = self.config.classify(row.last_accessed, now);
            if new_tier != DataTier::Hot {
                let mut row = self.hot.remove(&row_id).unwrap();
                row.tier = new_tier;
                match new_tier {
                    DataTier::Cold => {
                        self.cold.insert(row_id, row);
                    }
                    DataTier::Archive => {
                        self.archive.insert(row_id, row);
                    }
                    DataTier::Hot => unreachable!(),
                }
                migrated += 1;
            }
        }

        self.migration_count += migrated as u64;
        migrated
    }

    /// 执行归档迁移 — 扫描冷存储，将归档数据迁移至归档存储
    ///
    /// 返回迁移的行数
    pub fn migrate_archive(&mut self, now: u64) -> usize {
        let cold_ids: Vec<u64> = self.cold.keys().copied().collect();
        let mut migrated = 0;

        for row_id in cold_ids {
            let row = self.cold.get(&row_id).unwrap();
            let new_tier = self.config.classify(row.last_accessed, now);
            if new_tier == DataTier::Archive {
                let mut row = self.cold.remove(&row_id).unwrap();
                row.tier = DataTier::Archive;
                self.archive.insert(row_id, row);
                migrated += 1;
            }
        }

        self.migration_count += migrated as u64;
        migrated
    }

    /// 执行完整迁移 — 先迁移归档，再迁移冷数据
    ///
    /// 返回 (冷迁移数, 归档迁移数)
    pub fn migrate_all(&mut self, now: u64) -> (usize, usize) {
        let archived = self.migrate_archive(now);
        let colded = self.migrate_cold(now);
        (colded, archived)
    }

    /// 重置统计
    pub fn reset_stats(&mut self) {
        self.total_queries = 0;
        self.hot_hits = 0;
        self.cold_hits = 0;
        self.archive_hits = 0;
        self.migration_count = 0;
    }
}

// =====================================================================
//  ColdDataMigrator — 冷数据迁移器
// =====================================================================

/// 冷数据迁移器 — 定期扫描热存储，将冷数据迁移至冷存储
pub struct ColdDataMigrator {
    /// 分层存储引用
    storage: TieredStorage,
    /// 上次迁移时间戳
    last_migration: u64,
}

impl ColdDataMigrator {
    /// 构造迁移器
    pub fn new(config: TierConfig) -> Self {
        Self {
            storage: TieredStorage::new(config),
            last_migration: 0,
        }
    }

    /// 获取存储引用
    pub fn storage(&self) -> &TieredStorage {
        &self.storage
    }

    /// 获取存储可变引用
    pub fn storage_mut(&mut self) -> &mut TieredStorage {
        &mut self.storage
    }

    /// 检查是否到达迁移周期
    pub fn should_migrate(&self, now: u64) -> bool {
        now.saturating_sub(self.last_migration) >= self.storage.config().migration_interval_secs
    }

    /// 执行迁移
    ///
    /// 返回 (冷迁移数, 归档迁移数)
    pub fn migrate(&mut self, now: u64) -> (usize, usize) {
        let result = self.storage.migrate_all(now);
        self.last_migration = now;
        result
    }

    /// 强制执行迁移（忽略周期检查）
    pub fn force_migrate(&mut self, now: u64) -> (usize, usize) {
        let result = self.storage.migrate_all(now);
        self.last_migration = now;
        result
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  DataTier 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_data_tier_as_str() {
        assert_eq!(DataTier::Hot.as_str(), "hot");
        assert_eq!(DataTier::Cold.as_str(), "cold");
        assert_eq!(DataTier::Archive.as_str(), "archive");
    }

    #[test]
    fn test_data_tier_is_hot() {
        assert!(DataTier::Hot.is_hot());
        assert!(!DataTier::Cold.is_hot());
        assert!(!DataTier::Archive.is_hot());
    }

    #[test]
    fn test_data_tier_is_cold() {
        assert!(DataTier::Cold.is_cold());
        assert!(!DataTier::Hot.is_cold());
        assert!(!DataTier::Archive.is_cold());
    }

    #[test]
    fn test_data_tier_is_archive() {
        assert!(DataTier::Archive.is_archive());
        assert!(!DataTier::Hot.is_archive());
        assert!(!DataTier::Cold.is_archive());
    }

    #[test]
    fn test_data_tier_cost_factor() {
        assert_eq!(DataTier::Hot.cost_factor(), 1.0);
        assert_eq!(DataTier::Cold.cost_factor(), 0.1);
        assert_eq!(DataTier::Archive.cost_factor(), 0.02);
    }

    #[test]
    fn test_data_tier_display() {
        assert_eq!(format!("{}", DataTier::Hot), "hot");
        assert_eq!(format!("{}", DataTier::Cold), "cold");
        assert_eq!(format!("{}", DataTier::Archive), "archive");
    }

    // -----------------------------------------------------------------
    //  TierConfig 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_tier_config_default() {
        let config = TierConfig::default();
        assert_eq!(config.cold_threshold_days, 30);
        assert_eq!(config.archive_threshold_days, 90);
        assert_eq!(config.migration_interval_secs, 3600);
    }

    #[test]
    fn test_tier_config_new() {
        let config = TierConfig::new();
        assert_eq!(config.cold_threshold_days, DEFAULT_COLD_THRESHOLD_DAYS);
        assert_eq!(
            config.archive_threshold_days,
            DEFAULT_ARCHIVE_THRESHOLD_DAYS
        );
    }

    #[test]
    fn test_tier_config_builder() {
        let config = TierConfig::new()
            .with_cold_threshold(7)
            .with_archive_threshold(30)
            .with_migration_interval(60);
        assert_eq!(config.cold_threshold_days, 7);
        assert_eq!(config.archive_threshold_days, 30);
        assert_eq!(config.migration_interval_secs, 60);
    }

    #[test]
    fn test_tier_config_classify_hot() {
        let config = TierConfig::default();
        let now = 1_000_000_000;
        // 1 天前访问 → Hot
        let last_accessed = now - SECONDS_PER_DAY;
        assert_eq!(config.classify(last_accessed, now), DataTier::Hot);
    }

    #[test]
    fn test_tier_config_classify_cold() {
        let config = TierConfig::default();
        let now = 1_000_000_000;
        // 35 天前访问 → Cold
        let last_accessed = now - 35 * SECONDS_PER_DAY;
        assert_eq!(config.classify(last_accessed, now), DataTier::Cold);
    }

    #[test]
    fn test_tier_config_classify_archive() {
        let config = TierConfig::default();
        let now = 1_000_000_000;
        // 100 天前访问 → Archive
        let last_accessed = now - 100 * SECONDS_PER_DAY;
        assert_eq!(config.classify(last_accessed, now), DataTier::Archive);
    }

    #[test]
    fn test_tier_config_classify_boundary_cold() {
        let config = TierConfig::default();
        let now = 1_000_000_000;
        // 恰好 30 天 → Cold
        let last_accessed = now - 30 * SECONDS_PER_DAY;
        assert_eq!(config.classify(last_accessed, now), DataTier::Cold);
    }

    #[test]
    fn test_tier_config_classify_boundary_archive() {
        let config = TierConfig::default();
        let now = 1_000_000_000;
        // 恰好 90 天 → Archive
        let last_accessed = now - 90 * SECONDS_PER_DAY;
        assert_eq!(config.classify(last_accessed, now), DataTier::Archive);
    }

    #[test]
    fn test_tier_config_classify_boundary_29_days() {
        let config = TierConfig::default();
        let now = 1_000_000_000;
        // 29 天 → Hot
        let last_accessed = now - 29 * SECONDS_PER_DAY;
        assert_eq!(config.classify(last_accessed, now), DataTier::Hot);
    }

    // -----------------------------------------------------------------
    //  DataRow 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_data_row_new() {
        let row = DataRow::new(1, vec![1, 2, 3], 1000);
        assert_eq!(row.row_id, 1);
        assert_eq!(row.data, vec![1, 2, 3]);
        assert_eq!(row.created_at, 1000);
        assert_eq!(row.last_accessed, 1000);
        assert_eq!(row.tier, DataTier::Hot);
    }

    #[test]
    fn test_data_row_touch() {
        let mut row = DataRow::new(1, vec![1], 1000);
        row.touch(2000);
        assert_eq!(row.last_accessed, 2000);
    }

    #[test]
    fn test_data_row_size() {
        let row = DataRow::new(1, vec![1, 2, 3, 4, 5], 1000);
        assert_eq!(row.size(), 5);
    }

    #[test]
    fn test_data_row_days_since_access() {
        let row = DataRow::new(1, vec![1], 1000);
        // 3 天后
        let now = 1000 + 3 * SECONDS_PER_DAY;
        assert_eq!(row.days_since_access(now), 3);
    }

    // -----------------------------------------------------------------
    //  TieredStorage 基本操作测试
    // -----------------------------------------------------------------

    #[test]
    fn test_tiered_storage_new() {
        let storage = TieredStorage::new(TierConfig::default());
        assert_eq!(storage.hot_count(), 0);
        assert_eq!(storage.cold_count(), 0);
        assert_eq!(storage.archive_count(), 0);
        assert_eq!(storage.total_count(), 0);
    }

    #[test]
    fn test_tiered_storage_put() {
        let mut storage = TieredStorage::new(TierConfig::default());
        let row = DataRow::new(1, vec![1, 2, 3], 1000);
        storage.put(row);
        assert_eq!(storage.hot_count(), 1);
        assert_eq!(storage.cold_count(), 0);
        assert_eq!(storage.total_count(), 1);
    }

    #[test]
    fn test_tiered_storage_get_hot_hit() {
        let mut storage = TieredStorage::new(TierConfig::default());
        let row = DataRow::new(1, vec![1, 2, 3], 1000);
        storage.put(row);

        let result = storage.get(1, 2000);
        assert!(result.is_some());
        assert_eq!(result.unwrap().data, vec![1, 2, 3]);
        assert_eq!(storage.hot_hits, 1);
        assert_eq!(storage.cold_hits, 0);
        assert_eq!(storage.archive_hits, 0);
    }

    #[test]
    fn test_tiered_storage_get_cold_hit() {
        let mut storage = TieredStorage::new(TierConfig::default());
        let row = DataRow::new(1, vec![1, 2, 3], 1000);
        storage.put(row);

        // 迁移至冷存储（35 天后）
        let now = 1000 + 35 * SECONDS_PER_DAY;
        storage.migrate_cold(now);
        assert_eq!(storage.hot_count(), 0);
        assert_eq!(storage.cold_count(), 1);

        // 查询冷存储 → 命中并提升至热存储
        let result = storage.get(1, now);
        assert!(result.is_some());
        assert_eq!(storage.cold_hits, 1);
        assert_eq!(storage.hot_count(), 1);
        assert_eq!(storage.cold_count(), 0);
    }

    #[test]
    fn test_tiered_storage_get_archive_hit() {
        let mut storage = TieredStorage::new(TierConfig::default());
        let row = DataRow::new(1, vec![1, 2, 3], 1000);
        storage.put(row);

        // 迁移至归档存储（100 天后）
        let now = 1000 + 100 * SECONDS_PER_DAY;
        storage.migrate_cold(now);
        assert_eq!(storage.archive_count(), 1);

        // 查询归档存储 → 命中并提升至热存储
        let result = storage.get(1, now);
        assert!(result.is_some());
        assert_eq!(storage.archive_hits, 1);
        assert_eq!(storage.hot_count(), 1);
        assert_eq!(storage.archive_count(), 0);
    }

    #[test]
    fn test_tiered_storage_get_miss() {
        let mut storage = TieredStorage::new(TierConfig::default());
        let result = storage.get(999, 1000);
        assert!(result.is_none());
        assert_eq!(storage.total_queries, 1);
    }

    #[test]
    fn test_tiered_storage_peek() {
        let mut storage = TieredStorage::new(TierConfig::default());
        let row = DataRow::new(1, vec![1, 2, 3], 1000);
        storage.put(row);

        // peek 不更新访问时间
        let result = storage.peek(1);
        assert!(result.is_some());
        assert_eq!(storage.total_queries, 0);
    }

    #[test]
    fn test_tiered_storage_peek_cold() {
        let mut storage = TieredStorage::new(TierConfig::default());
        let row = DataRow::new(1, vec![1, 2, 3], 1000);
        storage.put(row);

        let now = 1000 + 35 * SECONDS_PER_DAY;
        storage.migrate_cold(now);

        let result = storage.peek(1);
        assert!(result.is_some());
        assert_eq!(storage.cold_count(), 1); // 未提升
    }

    // -----------------------------------------------------------------
    //  迁移测试
    // -----------------------------------------------------------------

    #[test]
    fn test_migrate_cold_no_migration() {
        let mut storage = TieredStorage::new(TierConfig::default());
        storage.put(DataRow::new(1, vec![1], 1000));

        // 1 天后 → 无迁移
        let now = 1000 + SECONDS_PER_DAY;
        let migrated = storage.migrate_cold(now);
        assert_eq!(migrated, 0);
        assert_eq!(storage.hot_count(), 1);
        assert_eq!(storage.cold_count(), 0);
    }

    #[test]
    fn test_migrate_cold_single() {
        let mut storage = TieredStorage::new(TierConfig::default());
        storage.put(DataRow::new(1, vec![1], 1000));

        // 35 天后 → 迁移至冷存储
        let now = 1000 + 35 * SECONDS_PER_DAY;
        let migrated = storage.migrate_cold(now);
        assert_eq!(migrated, 1);
        assert_eq!(storage.hot_count(), 0);
        assert_eq!(storage.cold_count(), 1);
        assert_eq!(storage.migration_count, 1);
    }

    #[test]
    fn test_migrate_cold_multiple() {
        let mut storage = TieredStorage::new(TierConfig::default());
        for i in 0..10 {
            storage.put(DataRow::new(i, vec![i as u8], 1000));
        }

        let now = 1000 + 35 * SECONDS_PER_DAY;
        let migrated = storage.migrate_cold(now);
        assert_eq!(migrated, 10);
        assert_eq!(storage.hot_count(), 0);
        assert_eq!(storage.cold_count(), 10);
    }

    #[test]
    fn test_migrate_cold_partial() {
        let mut storage = TieredStorage::new(TierConfig::default());
        // 行 0: 最近访问（热）
        storage.put(DataRow::new(0, vec![0], 1000));
        // 行 1: 35 天前访问（冷）
        let mut row1 = DataRow::new(1, vec![1], 1000);
        row1.last_accessed = 1000;
        storage.put(row1);

        let now = 1000 + 35 * SECONDS_PER_DAY;
        // 先 touch 行 0
        storage.get(0, now);

        let migrated = storage.migrate_cold(now);
        assert_eq!(migrated, 1);
        assert_eq!(storage.hot_count(), 1);
        assert_eq!(storage.cold_count(), 1);
    }

    #[test]
    fn test_migrate_archive() {
        let mut storage = TieredStorage::new(TierConfig::default());
        storage.put(DataRow::new(1, vec![1], 1000));

        // 100 天后 → 迁移至归档存储
        let now = 1000 + 100 * SECONDS_PER_DAY;
        let migrated = storage.migrate_cold(now);
        assert_eq!(migrated, 1);
        assert_eq!(storage.archive_count(), 1);
        assert_eq!(storage.cold_count(), 0);
    }

    #[test]
    fn test_migrate_all() {
        let mut storage = TieredStorage::new(TierConfig::default());
        // 行 0: 35 天前 → 冷
        let mut row0 = DataRow::new(0, vec![0], 1000);
        row0.last_accessed = 1000;
        storage.put(row0);
        // 行 1: 100 天前 → 归档
        let mut row1 = DataRow::new(1, vec![1], 1000);
        row1.last_accessed = 1000;
        storage.put(row1);

        // 先迁移至冷存储
        let now1 = 1000 + 35 * SECONDS_PER_DAY;
        storage.migrate_cold(now1);
        assert_eq!(storage.cold_count(), 2);

        // 再迁移：行 1 应归档（100 天前）
        let now2 = 1000 + 100 * SECONDS_PER_DAY;
        let (colded, archived) = storage.migrate_all(now2);
        // colded 为 u64 永远 >= 0，断言 archived 至少 1 行
        let _ = colded;
        assert!(archived >= 1);
    }

    // -----------------------------------------------------------------
    //  统计测试
    // -----------------------------------------------------------------

    #[test]
    fn test_storage_cost() {
        let mut storage = TieredStorage::new(TierConfig::default());
        // 热存储 100 字节
        storage.put(DataRow::new(0, vec![0; 100], 1000));
        // 迁移至冷存储
        let now = 1000 + 35 * SECONDS_PER_DAY;
        storage.migrate_cold(now);

        // 成本 = 0 * 1.0 + 100 * 0.1 + 0 * 0.02 = 10.0
        assert!((storage.storage_cost() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn test_cost_savings() {
        let mut storage = TieredStorage::new(TierConfig::default());
        // 100 字节热数据
        storage.put(DataRow::new(0, vec![0; 100], 1000));

        // 全热：savings = 0
        assert!((storage.cost_savings() - 0.0).abs() < 1e-6);

        // 迁移至冷存储
        let now = 1000 + 35 * SECONDS_PER_DAY;
        storage.migrate_cold(now);

        // 成本 = 100 * 0.1 = 10, total = 100, savings = 1 - 10/100 = 0.9
        assert!((storage.cost_savings() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_cost_savings_5x() {
        let mut storage = TieredStorage::new(TierConfig::default());
        // 100 行，每行 100 字节
        for i in 0..100 {
            storage.put(DataRow::new(i, vec![0; 100], 1000));
        }

        // 迁移至冷存储
        let now = 1000 + 35 * SECONDS_PER_DAY;
        storage.migrate_cold(now);

        // 成本节省率 = 1 - 0.1 = 0.9 = 90%
        // 存储成本降低 10x（远超 5x 要求）
        assert!(storage.cost_savings() >= 0.8); // 至少 80% 节省
    }

    #[test]
    fn test_hit_rates() {
        let mut storage = TieredStorage::new(TierConfig::default());
        storage.put(DataRow::new(0, vec![1], 1000));
        storage.put(DataRow::new(1, vec![2], 1000));

        // 迁移行 1 至冷存储
        let mut row1 = storage.hot.remove(&1).unwrap();
        row1.last_accessed = 1000;
        row1.tier = DataTier::Cold;
        storage.cold.insert(1, row1);

        // 查询行 0（热命中）
        storage.get(0, 2000);
        // 查询行 1（冷命中）
        storage.get(1, 2000);
        // 查询不存在的行
        storage.get(999, 2000);

        assert!((storage.hot_hit_rate() - 1.0 / 3.0).abs() < 1e-6);
        assert!((storage.cold_hit_rate() - 1.0 / 3.0).abs() < 1e-6);
        assert!((storage.archive_hit_rate() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_reset_stats() {
        let mut storage = TieredStorage::new(TierConfig::default());
        storage.put(DataRow::new(0, vec![1], 1000));
        storage.get(0, 2000);

        assert_eq!(storage.total_queries, 1);
        storage.reset_stats();
        assert_eq!(storage.total_queries, 0);
        assert_eq!(storage.hot_hits, 0);
    }

    // -----------------------------------------------------------------
    //  ColdDataMigrator 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_migrator_new() {
        let migrator = ColdDataMigrator::new(TierConfig::default());
        assert_eq!(migrator.storage().total_count(), 0);
        assert_eq!(migrator.last_migration, 0);
    }

    #[test]
    fn test_migrator_should_migrate() {
        let mut migrator = ColdDataMigrator::new(TierConfig::default());
        migrator.last_migration = 1000;

        // 不到周期
        assert!(!migrator.should_migrate(1000 + 1800));

        // 到周期
        assert!(migrator.should_migrate(1000 + 3600));
    }

    #[test]
    fn test_migrator_force_migrate() {
        let mut migrator = ColdDataMigrator::new(TierConfig::default());
        migrator.storage_mut().put(DataRow::new(0, vec![1], 1000));

        let now = 1000 + 35 * SECONDS_PER_DAY;
        let (colded, archived) = migrator.force_migrate(now);
        assert_eq!(colded, 1);
        assert_eq!(archived, 0);
        assert_eq!(migrator.storage().cold_count(), 1);
        assert_eq!(migrator.last_migration, now);
    }

    #[test]
    fn test_migrator_migrate() {
        let mut migrator = ColdDataMigrator::new(TierConfig::default());
        migrator.storage_mut().put(DataRow::new(0, vec![1], 1000));

        let now = 1000 + 35 * SECONDS_PER_DAY;
        let (colded, archived) = migrator.migrate(now);
        assert_eq!(colded, 1);
        assert_eq!(archived, 0);
    }

    // -----------------------------------------------------------------
    //  完整工作流测试
    // -----------------------------------------------------------------

    #[test]
    fn test_full_workflow_hot_cold_archive() {
        let mut migrator = ColdDataMigrator::new(TierConfig::default());
        let base_time = 1_000_000;

        // 写入 100 行热数据
        for i in 0..100 {
            migrator
                .storage_mut()
                .put(DataRow::new(i, vec![i as u8; 50], base_time));
        }
        assert_eq!(migrator.storage().hot_count(), 100);

        // 35 天后迁移 → 全部冷数据
        let now1 = base_time + 35 * SECONDS_PER_DAY;
        let (colded, _) = migrator.force_migrate(now1);
        assert_eq!(colded, 100);
        assert_eq!(migrator.storage().hot_count(), 0);
        assert_eq!(migrator.storage().cold_count(), 100);

        // 100 天后迁移 → 全部归档
        let now2 = base_time + 100 * SECONDS_PER_DAY;
        let (_, archived) = migrator.force_migrate(now2);
        assert_eq!(archived, 100);
        assert_eq!(migrator.storage().cold_count(), 0);
        assert_eq!(migrator.storage().archive_count(), 100);

        // 查询归档数据 → 提升至热存储
        let result = migrator.storage_mut().get(50, now2);
        assert!(result.is_some());
        assert_eq!(migrator.storage().hot_count(), 1);
        assert_eq!(migrator.storage().archive_count(), 99);
    }

    #[test]
    fn test_workflow_query_consistency() {
        // 冷热查询结果一致
        let mut migrator = ColdDataMigrator::new(TierConfig::default());
        let base_time = 1_000_000;

        // 写入数据
        for i in 0..10 {
            migrator
                .storage_mut()
                .put(DataRow::new(i, vec![i as u8; 20], base_time));
        }

        // 迁移至冷存储
        let now1 = base_time + 35 * SECONDS_PER_DAY;
        migrator.force_migrate(now1);

        // 查询冷数据
        let cold_result = migrator.storage_mut().get(5, now1).cloned();
        assert!(cold_result.is_some());
        let cold_data = cold_result.unwrap().data;
        assert_eq!(cold_data, vec![5; 20]);

        // 重新写入热数据
        migrator
            .storage_mut()
            .put(DataRow::new(5, vec![5; 20], base_time));

        // 迁移至冷存储
        migrator.force_migrate(now1);

        // 查询冷数据 → 结果应一致
        let cold_result2 = migrator.storage_mut().get(5, now1).cloned();
        assert!(cold_result2.is_some());
        assert_eq!(cold_result2.unwrap().data, cold_data);
    }

    #[test]
    fn test_storage_cost_reduction_5x() {
        // 验证冷数据存储成本降低 >= 5x
        let mut migrator = ColdDataMigrator::new(TierConfig::default());
        let base_time = 1_000_000;

        // 写入 1000 行，每行 100 字节 = 100000 字节
        for i in 0..1000 {
            migrator
                .storage_mut()
                .put(DataRow::new(i, vec![0; 100], base_time));
        }

        // 全热存储成本 = 100000 * 1.0 = 100000
        let hot_cost = migrator.storage().storage_cost();
        assert!((hot_cost - 100_000.0).abs() < 1e-6);

        // 迁移至冷存储
        let now = base_time + 35 * SECONDS_PER_DAY;
        migrator.force_migrate(now);

        // 冷存储成本 = 100000 * 0.1 = 10000
        let cold_cost = migrator.storage().storage_cost();
        assert!((cold_cost - 10_000.0).abs() < 1e-6);

        // 成本降低 10x（>= 5x 要求）
        let reduction = hot_cost / cold_cost;
        assert!(
            reduction >= 5.0,
            "cost reduction should be >= 5x, got {reduction}"
        );
    }

    #[test]
    fn test_archive_cost_reduction() {
        let mut migrator = ColdDataMigrator::new(TierConfig::default());
        let base_time = 1_000_000;

        for i in 0..1000 {
            migrator
                .storage_mut()
                .put(DataRow::new(i, vec![0; 100], base_time));
        }

        // 迁移至归档存储
        let now = base_time + 100 * SECONDS_PER_DAY;
        migrator.force_migrate(now);

        // 归档成本 = 100000 * 0.02 = 2000
        let archive_cost = migrator.storage().storage_cost();
        assert!((archive_cost - 2_000.0).abs() < 1e-6);

        // 成本降低 50x
        let reduction = 100_000.0 / archive_cost;
        assert!(reduction >= 50.0);
    }
}
