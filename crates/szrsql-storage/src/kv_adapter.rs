//! 多级 KV 适配器框架 — 统一 B-Tree / LSM-tree 存储引擎接口。
//!
//! 对应 `SzRSQL实施进度.md` Phase 1.9。
//!
//! 设计目标：
//! - 上层 SQL 执行器无需感知底层存储引擎类型，通过 `IndexAdapter` trait 统一访问
//! - 支持 BTree（点查/范围扫描友好）和 LSM（写入友好）两种引擎
//! - 提供基于工作负载特征的引擎选择策略
//! - 工厂模式创建适配器实例，支持运行时切换引擎
//!
//! 当前实现：
//! - `BTreeAdapter`：包装 `crate::btree::BTree`，所有操作委派给底层 BTree
//! - `LsmAdapter`：占位实现，使用 `std::collections::BTreeMap` 模拟 memtable
//!   （真实 LSM-tree 待 Phase 4 实现，当前仅验证 trait 接口一致性）

use crate::btree::BTree;
use std::collections::{BTreeMap, HashSet};
use std::ops::Bound;

// =====================================================================
//  错误类型
// =====================================================================

/// KV 适配器错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KvAdapterError {
    #[error("key not found")]
    KeyNotFound,
    #[error("engine error: {0}")]
    EngineError(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
}

// =====================================================================
//  IndexAdapter trait
// =====================================================================

/// KV 适配器统一接口
///
/// 所有存储引擎（BTree / LSM / 后续可能的扩展）实现此 trait，
/// 上层通过 `Box<dyn IndexAdapter>` 动态分发。
pub trait IndexAdapter: Send + Sync {
    /// 插入或更新 key-value（upsert 语义）
    ///
    /// 若 key 已存在则更新 value，否则插入新条目。
    fn insert(&mut self, key: &[u8], value: u16) -> Result<(), KvAdapterError>;

    /// 删除 key
    ///
    /// 返回 `true` 表示找到并删除，`false` 表示 key 不存在。
    fn delete(&mut self, key: &[u8]) -> Result<bool, KvAdapterError>;

    /// 点查
    ///
    /// 返回 `Some(value)` 表示命中，`None` 表示未命中。
    fn get(&self, key: &[u8]) -> Result<Option<u16>, KvAdapterError>;

    /// 范围扫描 [lower, upper]
    ///
    /// `Bound::Unbounded` 表示该侧无限制。
    /// 返回的 Vec 按 key 升序排列。
    fn range_scan(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<Vec<(Vec<u8>, u16)>, KvAdapterError>;

    /// 范围扫描（带 LIMIT 截断）
    ///
    /// `limit = 0` 返回空 Vec；`limit >= 结果数` 返回全部。
    fn range_scan_with_limit(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, u16)>, KvAdapterError>;

    /// 已存储 key 数量
    fn len(&self) -> Result<usize, KvAdapterError>;

    /// 是否为空
    fn is_empty(&self) -> Result<bool, KvAdapterError> {
        Ok(self.len()? == 0)
    }

    /// 引擎名称（用于日志/监控）
    fn engine_name(&self) -> &'static str;

    /// 引擎统计信息
    fn stats(&self) -> EngineStats;
}

// =====================================================================
//  EngineStats / EngineType / EngineConfig
// =====================================================================

/// 引擎统计信息
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineStats {
    /// 引擎名称
    pub engine_name: String,
    /// 已存储 key 数量
    pub key_count: usize,
    /// 节点数量（BTree: pages.len()，LSM: SSTable 数 + 1 memtable）
    pub node_count: usize,
    /// 树高度 / LSM 层数
    pub height: usize,
    /// 估计占用字节数（当前实现为 0，待后续 Phase 跟踪）
    pub bytes_used: usize,
}

/// 引擎类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineType {
    /// B+Tree 引擎（点查/范围扫描友好）
    BTree,
    /// LSM-Tree 引擎（写入友好，占位实现）
    Lsm,
}

/// 引擎配置
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// 引擎类型
    pub engine_type: EngineType,
    /// BTree 阶数（仅 EngineType::BTree 时生效）
    pub order: usize,
    /// LSM memtable 大小阈值（仅 EngineType::Lsm 时生效，占位未使用）
    pub write_buffer_size: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            engine_type: EngineType::BTree,
            order: 256,
            write_buffer_size: 64 * 1024 * 1024, // 64MB
        }
    }
}

// =====================================================================
//  工作负载特征 & 引擎选择策略
// =====================================================================

/// 工作负载特征（用于引擎选择策略）
#[derive(Debug, Clone)]
pub struct WorkloadCharacteristics {
    /// 读操作比例 [0.0, 1.0]
    pub read_ratio: f64,
    /// 写操作比例 [0.0, 1.0]（read_ratio + write_ratio 应 = 1.0）
    pub write_ratio: f64,
    /// 数据量预估（字节数）
    pub data_size: usize,
    /// 点查比例 [0.0, 1.0]
    pub point_query_ratio: f64,
    /// 范围扫描比例 [0.0, 1.0]
    pub range_scan_ratio: f64,
}

impl Default for WorkloadCharacteristics {
    fn default() -> Self {
        Self {
            read_ratio: 0.7,
            write_ratio: 0.3,
            data_size: 0,
            point_query_ratio: 0.8,
            range_scan_ratio: 0.2,
        }
    }
}

/// 引擎选择策略
///
/// 决策规则：
/// 1. 写比例 > 70% → LSM（写入友好）
/// 2. 否则 → BTree（点查/范围扫描友好）
///
/// **注**：当前 LSM 为占位实现，生产环境选择 LSM 时实际仍走 BTreeMap 模拟。
/// 待 Phase 4 实现 LSM-tree 后，此策略才真正生效。
pub fn select_engine(workload: &WorkloadCharacteristics) -> EngineType {
    if workload.write_ratio > 0.7 {
        EngineType::Lsm
    } else {
        EngineType::BTree
    }
}

// =====================================================================
//  BTreeAdapter
// =====================================================================

/// BTree 适配器
///
/// 包装 `crate::btree::BTree`，实现 `IndexAdapter` trait。
pub struct BTreeAdapter {
    btree: BTree,
}

impl BTreeAdapter {
    /// 创建指定阶数的 BTreeAdapter
    pub fn new(order: usize) -> Self {
        Self {
            btree: BTree::new(order),
        }
    }

    /// 创建默认阶数（256）的 BTreeAdapter
    pub fn with_default_order() -> Self {
        Self {
            btree: BTree::with_default_order(),
        }
    }

    /// 获取底层 BTree 的不可变引用（供高级用法/调试使用）
    pub fn inner(&self) -> &BTree {
        &self.btree
    }
}

impl IndexAdapter for BTreeAdapter {
    fn insert(&mut self, key: &[u8], value: u16) -> Result<(), KvAdapterError> {
        self.btree
            .insert(key.to_vec(), value)
            .map_err(|e| KvAdapterError::EngineError(e.to_string()))
    }

    fn delete(&mut self, key: &[u8]) -> Result<bool, KvAdapterError> {
        self.btree
            .delete(key)
            .map_err(|e| KvAdapterError::EngineError(e.to_string()))
    }

    fn get(&self, key: &[u8]) -> Result<Option<u16>, KvAdapterError> {
        self.btree
            .search(key)
            .map_err(|e| KvAdapterError::EngineError(e.to_string()))
    }

    fn range_scan(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<Vec<(Vec<u8>, u16)>, KvAdapterError> {
        self.btree
            .range_scan(lower, upper)
            .map_err(|e| KvAdapterError::EngineError(e.to_string()))
    }

    fn range_scan_with_limit(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, u16)>, KvAdapterError> {
        self.btree
            .range_scan_with_limit(lower, upper, Some(limit))
            .map_err(|e| KvAdapterError::EngineError(e.to_string()))
    }

    fn len(&self) -> Result<usize, KvAdapterError> {
        self.btree
            .in_order_leaf_traverse()
            .map(|v| v.len())
            .map_err(|e| KvAdapterError::EngineError(e.to_string()))
    }

    fn engine_name(&self) -> &'static str {
        "BTree"
    }

    fn stats(&self) -> EngineStats {
        EngineStats {
            engine_name: "BTree".to_string(),
            key_count: self
                .btree
                .in_order_leaf_traverse()
                .map(|v| v.len())
                .unwrap_or(0),
            node_count: self.btree.node_count(),
            height: self.btree.height(),
            bytes_used: 0,
        }
    }
}

// =====================================================================
//  LsmAdapter（占位实现）
// =====================================================================

/// LSM 适配器（占位实现）
///
/// **注**：当前使用 `std::collections::BTreeMap` 模拟 memtable，
/// 不含真实的 SSTable / compaction / bloom filter 等 LSM 组件。
/// 待 Phase 4 实现真实 LSM-tree 后替换内部实现，trait 接口保持不变。
pub struct LsmAdapter {
    /// memtable（内存表）
    memtable: BTreeMap<Vec<u8>, u16>,
    /// tombstone 集合（记录已删除的 key，用于 LSM 语义模拟）
    tombstones: HashSet<Vec<u8>>,
}

impl LsmAdapter {
    /// 创建空 LsmAdapter
    pub fn new() -> Self {
        Self {
            memtable: BTreeMap::new(),
            tombstones: HashSet::new(),
        }
    }

    /// 获取 tombstone 数量（供测试/调试使用）
    pub fn tombstone_count(&self) -> usize {
        self.tombstones.len()
    }
}

impl Default for LsmAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexAdapter for LsmAdapter {
    fn insert(&mut self, key: &[u8], value: u16) -> Result<(), KvAdapterError> {
        self.memtable.insert(key.to_vec(), value);
        self.tombstones.remove(key);
        Ok(())
    }

    fn delete(&mut self, key: &[u8]) -> Result<bool, KvAdapterError> {
        let existed = self.memtable.contains_key(key);
        self.memtable.remove(key);
        self.tombstones.insert(key.to_vec());
        Ok(existed)
    }

    fn get(&self, key: &[u8]) -> Result<Option<u16>, KvAdapterError> {
        Ok(self.memtable.get(key).copied())
    }

    fn range_scan(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<Vec<(Vec<u8>, u16)>, KvAdapterError> {
        // BTreeMap::range 在 lower > upper 时会 panic，用 catch_unwind 兜底为空 Vec
        // （与 BTreeAdapter 的空结果语义一致）
        let result: Vec<(Vec<u8>, u16)> = std::panic::catch_unwind(|| {
            self.memtable
                .range::<[u8], _>((lower, upper))
                .map(|(k, v)| (k.clone(), *v))
                .collect()
        })
        .unwrap_or_default();
        Ok(result)
    }

    fn range_scan_with_limit(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, u16)>, KvAdapterError> {
        let full = self.range_scan(lower, upper)?;
        Ok(full.into_iter().take(limit).collect())
    }

    fn len(&self) -> Result<usize, KvAdapterError> {
        Ok(self.memtable.len())
    }

    fn engine_name(&self) -> &'static str {
        "LSM"
    }

    fn stats(&self) -> EngineStats {
        EngineStats {
            engine_name: "LSM".to_string(),
            key_count: self.memtable.len(),
            node_count: 1, // 单 memtable（占位实现）
            height: 1,
            bytes_used: 0,
        }
    }
}

// =====================================================================
//  适配器工厂
// =====================================================================

/// 适配器工厂：根据配置创建对应的适配器实例
pub fn create_adapter(config: &EngineConfig) -> Box<dyn IndexAdapter> {
    match config.engine_type {
        EngineType::BTree => Box::new(BTreeAdapter::new(config.order)),
        EngineType::Lsm => Box::new(LsmAdapter::new()),
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  测试辅助函数
    // -----------------------------------------------------------------

    fn make_key(v: i64) -> Vec<u8> {
        // 大端 i64 编码（与 btree.rs 中的 encode_i64_key 等价）
        let flipped = (v as u64) ^ (1u64 << 63);
        flipped.to_be_bytes().to_vec()
    }

    /// 在适配器中插入 N 个连续 key，返回 (keys, values) 元组
    fn insert_sequential_keys(
        adapter: &mut dyn IndexAdapter,
        start: i64,
        count: usize,
    ) -> Vec<(Vec<u8>, u16)> {
        let mut pairs = Vec::with_capacity(count);
        for i in 0..count {
            let key_i64 = start + i as i64;
            let key = make_key(key_i64);
            let value = (i % 65536) as u16;
            adapter.insert(&key, value).unwrap();
            pairs.push((key, value));
        }
        pairs
    }

    // -----------------------------------------------------------------
    //  Phase 1.9 — BTreeAdapter 测试
    // -----------------------------------------------------------------

    /// BTreeAdapter 基本 CRUD：insert → get → delete → get
    #[test]
    fn phase_019_btree_adapter_insert_get_delete_roundtrip() {
        let mut adapter = BTreeAdapter::with_default_order();
        assert_eq!(adapter.engine_name(), "BTree");
        assert!(adapter.is_empty().unwrap());

        let key = make_key(42);
        // 初始未命中
        assert_eq!(adapter.get(&key).unwrap(), None);

        // 插入
        adapter.insert(&key, 100).unwrap();
        assert_eq!(adapter.len().unwrap(), 1);
        assert_eq!(adapter.get(&key).unwrap(), Some(100));

        // upsert 更新
        adapter.insert(&key, 200).unwrap();
        assert_eq!(adapter.len().unwrap(), 1); // 数量不变
        assert_eq!(adapter.get(&key).unwrap(), Some(200));

        // 删除
        let deleted = adapter.delete(&key).unwrap();
        assert!(deleted);
        assert_eq!(adapter.len().unwrap(), 0);
        assert_eq!(adapter.get(&key).unwrap(), None);

        // 重复删除返回 false
        let deleted_again = adapter.delete(&key).unwrap();
        assert!(!deleted_again);
    }

    /// BTreeAdapter 范围扫描基本功能
    #[test]
    fn phase_019_btree_adapter_range_scan_basic() {
        let mut adapter = BTreeAdapter::with_default_order();
        let pairs = insert_sequential_keys(&mut adapter, 0, 100);

        // 全表扫描
        let all = adapter
            .range_scan(Bound::Unbounded, Bound::Unbounded)
            .unwrap();
        assert_eq!(all.len(), 100);
        // 严格递增
        for i in 1..all.len() {
            assert!(all[i - 1].0 < all[i].0, "not strictly increasing at {}", i);
        }
        // 数值匹配
        for (i, (expected_key, expected_val)) in pairs.iter().enumerate() {
            assert_eq!(all[i].0, *expected_key, "key mismatch at {}", i);
            assert_eq!(all[i].1, *expected_val, "value mismatch at {}", i);
        }

        // Included 边界
        let lower = make_key(10);
        let upper = make_key(20);
        let partial = adapter
            .range_scan(Bound::Included(&lower), Bound::Included(&upper))
            .unwrap();
        assert_eq!(partial.len(), 11); // [10, 20] 含两端共 11 个

        // Excluded 边界
        let partial_excl = adapter
            .range_scan(Bound::Excluded(&lower), Bound::Excluded(&upper))
            .unwrap();
        assert_eq!(partial_excl.len(), 9); // (10, 20) 不含两端共 9 个
    }

    /// BTreeAdapter 范围扫描 LIMIT 截断
    #[test]
    fn phase_019_btree_adapter_range_scan_with_limit() {
        let mut adapter = BTreeAdapter::with_default_order();
        insert_sequential_keys(&mut adapter, 0, 100);

        // limit = 0 → 空
        let empty = adapter
            .range_scan_with_limit(Bound::Unbounded, Bound::Unbounded, 0)
            .unwrap();
        assert_eq!(empty.len(), 0);

        // limit = 5
        let limited = adapter
            .range_scan_with_limit(Bound::Unbounded, Bound::Unbounded, 5)
            .unwrap();
        assert_eq!(limited.len(), 5);

        // limit >= 结果数 → 全部
        let all = adapter
            .range_scan_with_limit(Bound::Unbounded, Bound::Unbounded, 200)
            .unwrap();
        assert_eq!(all.len(), 100);
    }

    /// BTreeAdapter 与 BTree 直接调用结果一致
    #[test]
    fn phase_019_btree_adapter_matches_btree_direct() {
        let mut adapter = BTreeAdapter::with_default_order();
        let mut bt = BTree::with_default_order();

        // 同步插入 1000 个 key
        for i in 0..1000i64 {
            let key = make_key(i);
            let val = i as u16;
            adapter.insert(&key, val).unwrap();
            bt.insert(key.clone(), val).unwrap();
        }

        // 点查一致
        for i in 0..1000i64 {
            let key = make_key(i);
            assert_eq!(
                adapter.get(&key).unwrap(),
                bt.search(&key).unwrap(),
                "point lookup mismatch at i={}",
                i
            );
        }

        // 范围扫描一致
        let lower = make_key(100);
        let upper = make_key(200);
        let adapter_result = adapter
            .range_scan(Bound::Included(&lower), Bound::Included(&upper))
            .unwrap();
        let bt_result = bt
            .range_scan(
                Bound::Included(lower.as_slice()),
                Bound::Included(upper.as_slice()),
            )
            .unwrap();
        assert_eq!(adapter_result, bt_result);

        // 长度一致
        assert_eq!(
            adapter.len().unwrap(),
            bt.in_order_leaf_traverse().unwrap().len()
        );

        // 删除一致
        for i in (0..1000i64).step_by(2) {
            let key = make_key(i);
            assert_eq!(
                adapter.delete(&key).unwrap(),
                bt.delete(&key).unwrap(),
                "delete return value mismatch at i={}",
                i
            );
        }

        // 删除后点查一致
        for i in 0..1000i64 {
            let key = make_key(i);
            assert_eq!(
                adapter.get(&key).unwrap(),
                bt.search(&key).unwrap(),
                "post-delete point lookup mismatch at i={}",
                i
            );
        }

        // 删除后范围扫描一致
        let adapter_after = adapter
            .range_scan(Bound::Unbounded, Bound::Unbounded)
            .unwrap();
        let bt_after = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(adapter_after, bt_after);
    }

    /// BTreeAdapter upsert 语义验证
    #[test]
    fn phase_019_btree_adapter_upsert_semantics() {
        let mut adapter = BTreeAdapter::new(16);
        let key = make_key(123);

        // 反复 upsert
        for v in 0..10u16 {
            adapter.insert(&key, v).unwrap();
        }
        assert_eq!(adapter.len().unwrap(), 1); // 仍为 1 个 key
        assert_eq!(adapter.get(&key).unwrap(), Some(9)); // 最后一次值

        // upsert 不增加 key 数量
        for i in 0..10 {
            let k = make_key(i);
            adapter.insert(&k, i as u16).unwrap();
        }
        assert_eq!(adapter.len().unwrap(), 11); // 1 (key=123) + 10 (key=0..9)
    }

    /// BTreeAdapter 空树操作
    #[test]
    fn phase_019_btree_adapter_empty_operations() {
        let mut adapter = BTreeAdapter::with_default_order();
        assert!(adapter.is_empty().unwrap());
        assert_eq!(adapter.len().unwrap(), 0);

        // 空树点查
        assert_eq!(adapter.get(&make_key(42)).unwrap(), None);

        // 空树范围扫描
        let result = adapter
            .range_scan(Bound::Unbounded, Bound::Unbounded)
            .unwrap();
        assert_eq!(result.len(), 0);

        // 空树删除返回 false
        assert!(!adapter.delete(&make_key(42)).unwrap());

        // 统计信息正确
        let stats = adapter.stats();
        assert_eq!(stats.engine_name, "BTree");
        assert_eq!(stats.key_count, 0);
        assert_eq!(stats.node_count, 1); // 仅根叶子
        assert_eq!(stats.height, 1);
    }

    /// BTreeAdapter 统计信息正确
    #[test]
    fn phase_019_btree_adapter_stats_returns_correct_info() {
        let mut adapter = BTreeAdapter::new(8); // 小 order 强制分裂
        insert_sequential_keys(&mut adapter, 0, 1000);

        let stats = adapter.stats();
        assert_eq!(stats.engine_name, "BTree");
        assert_eq!(stats.key_count, 1000);
        assert!(
            stats.node_count > 1,
            "expected > 1 nodes, got {}",
            stats.node_count
        );
        assert!(
            stats.height >= 2,
            "expected height >= 2, got {}",
            stats.height
        );
    }

    // -----------------------------------------------------------------
    //  Phase 1.9 — LsmAdapter 测试
    // -----------------------------------------------------------------

    /// LsmAdapter 基本 CRUD：insert → get → delete → get
    #[test]
    fn phase_019_lsm_adapter_insert_get_delete_roundtrip() {
        let mut adapter = LsmAdapter::new();
        assert_eq!(adapter.engine_name(), "LSM");
        assert!(adapter.is_empty().unwrap());

        let key = make_key(42);
        // 初始未命中
        assert_eq!(adapter.get(&key).unwrap(), None);

        // 插入
        adapter.insert(&key, 100).unwrap();
        assert_eq!(adapter.len().unwrap(), 1);
        assert_eq!(adapter.get(&key).unwrap(), Some(100));

        // upsert 更新
        adapter.insert(&key, 200).unwrap();
        assert_eq!(adapter.len().unwrap(), 1);
        assert_eq!(adapter.get(&key).unwrap(), Some(200));

        // 删除
        let deleted = adapter.delete(&key).unwrap();
        assert!(deleted);
        assert_eq!(adapter.len().unwrap(), 0);
        assert_eq!(adapter.get(&key).unwrap(), None);

        // 重复删除返回 false（key 已不在 memtable）
        let deleted_again = adapter.delete(&key).unwrap();
        assert!(!deleted_again);
    }

    /// LsmAdapter 范围扫描基本功能
    #[test]
    fn phase_019_lsm_adapter_range_scan_basic() {
        let mut adapter = LsmAdapter::new();
        let pairs = insert_sequential_keys(&mut adapter, 0, 100);

        // 全表扫描
        let all = adapter
            .range_scan(Bound::Unbounded, Bound::Unbounded)
            .unwrap();
        assert_eq!(all.len(), 100);
        for i in 1..all.len() {
            assert!(all[i - 1].0 < all[i].0);
        }
        for (i, (expected_key, expected_val)) in pairs.iter().enumerate() {
            assert_eq!(all[i].0, *expected_key);
            assert_eq!(all[i].1, *expected_val);
        }

        // Included 边界
        let lower = make_key(10);
        let upper = make_key(20);
        let partial = adapter
            .range_scan(Bound::Included(&lower), Bound::Included(&upper))
            .unwrap();
        assert_eq!(partial.len(), 11);

        // Excluded 边界
        let partial_excl = adapter
            .range_scan(Bound::Excluded(&lower), Bound::Excluded(&upper))
            .unwrap();
        assert_eq!(partial_excl.len(), 9);
    }

    /// LsmAdapter 范围扫描 LIMIT 截断
    #[test]
    fn phase_019_lsm_adapter_range_scan_with_limit() {
        let mut adapter = LsmAdapter::new();
        insert_sequential_keys(&mut adapter, 0, 100);

        let empty = adapter
            .range_scan_with_limit(Bound::Unbounded, Bound::Unbounded, 0)
            .unwrap();
        assert_eq!(empty.len(), 0);

        let limited = adapter
            .range_scan_with_limit(Bound::Unbounded, Bound::Unbounded, 5)
            .unwrap();
        assert_eq!(limited.len(), 5);

        let all = adapter
            .range_scan_with_limit(Bound::Unbounded, Bound::Unbounded, 200)
            .unwrap();
        assert_eq!(all.len(), 100);
    }

    /// LsmAdapter tombstone 语义验证
    ///
    /// 验证删除后 key 进入 tombstone 集合，且再次插入会清除 tombstone。
    #[test]
    fn phase_019_lsm_adapter_tombstone_semantics() {
        let mut adapter = LsmAdapter::new();
        let key = make_key(42);

        // 插入
        adapter.insert(&key, 100).unwrap();
        assert_eq!(adapter.tombstone_count(), 0);

        // 删除 → tombstone 数量 +1
        adapter.delete(&key).unwrap();
        assert_eq!(adapter.tombstone_count(), 1);
        assert_eq!(adapter.get(&key).unwrap(), None);

        // 再次删除（key 不存在）→ tombstone 数量不变（已存在）
        adapter.delete(&key).unwrap();
        assert_eq!(adapter.tombstone_count(), 1);

        // 重新插入 → tombstone 被清除
        adapter.insert(&key, 200).unwrap();
        assert_eq!(adapter.tombstone_count(), 0);
        assert_eq!(adapter.get(&key).unwrap(), Some(200));
    }

    /// LsmAdapter 统计信息正确
    #[test]
    fn phase_019_lsm_adapter_stats_returns_correct_info() {
        let mut adapter = LsmAdapter::new();
        insert_sequential_keys(&mut adapter, 0, 100);

        let stats = adapter.stats();
        assert_eq!(stats.engine_name, "LSM");
        assert_eq!(stats.key_count, 100);
        assert_eq!(stats.node_count, 1); // 占位实现：单 memtable
        assert_eq!(stats.height, 1);
    }

    // -----------------------------------------------------------------
    //  Phase 1.9 — 引擎选择策略测试
    // -----------------------------------------------------------------

    /// 写密集（write_ratio > 0.7）→ LSM
    #[test]
    fn phase_019_select_engine_write_heavy_returns_lsm() {
        let workload = WorkloadCharacteristics {
            read_ratio: 0.2,
            write_ratio: 0.8,
            data_size: 0,
            point_query_ratio: 0.5,
            range_scan_ratio: 0.5,
        };
        assert_eq!(select_engine(&workload), EngineType::Lsm);
    }

    /// 读密集（write_ratio <= 0.7）→ BTree
    #[test]
    fn phase_019_select_engine_read_heavy_returns_btree() {
        let workload = WorkloadCharacteristics {
            read_ratio: 0.8,
            write_ratio: 0.2,
            data_size: 0,
            point_query_ratio: 0.5,
            range_scan_ratio: 0.5,
        };
        assert_eq!(select_engine(&workload), EngineType::BTree);
    }

    /// 边界：write_ratio = 0.7（恰好等于阈值）→ BTree（> 0.7 才选 LSM）
    #[test]
    fn phase_019_select_engine_boundary_70_percent_returns_btree() {
        let workload = WorkloadCharacteristics {
            read_ratio: 0.3,
            write_ratio: 0.7,
            data_size: 0,
            point_query_ratio: 0.5,
            range_scan_ratio: 0.5,
        };
        assert_eq!(select_engine(&workload), EngineType::BTree);
    }

    /// 边界：write_ratio = 0.7 + epsilon → LSM
    #[test]
    fn phase_019_select_engine_just_above_boundary_returns_lsm() {
        let workload = WorkloadCharacteristics {
            read_ratio: 0.2999,
            write_ratio: 0.7001,
            data_size: 0,
            point_query_ratio: 0.5,
            range_scan_ratio: 0.5,
        };
        assert_eq!(select_engine(&workload), EngineType::Lsm);
    }

    /// 默认 WorkloadCharacteristics → BTree
    #[test]
    fn phase_019_select_engine_default_workload_returns_btree() {
        let workload = WorkloadCharacteristics::default();
        assert_eq!(workload.write_ratio, 0.3);
        assert_eq!(select_engine(&workload), EngineType::BTree);
    }

    // -----------------------------------------------------------------
    //  Phase 1.9 — 工厂 & 配置测试
    // -----------------------------------------------------------------

    /// 默认 EngineConfig → BTree
    #[test]
    fn phase_019_engine_config_defaults_to_btree() {
        let config = EngineConfig::default();
        assert_eq!(config.engine_type, EngineType::BTree);
        assert_eq!(config.order, 256);
        assert_eq!(config.write_buffer_size, 64 * 1024 * 1024);
    }

    /// 工厂：BTree config → BTreeAdapter
    #[test]
    fn phase_019_factory_creates_btree_adapter() {
        let config = EngineConfig {
            engine_type: EngineType::BTree,
            order: 32,
            write_buffer_size: 0,
        };
        let mut adapter = create_adapter(&config);
        assert_eq!(adapter.engine_name(), "BTree");
        assert!(adapter.is_empty().unwrap());

        // 基本 CRUD 验证
        let key = make_key(42);
        adapter.insert(&key, 100).unwrap();
        assert_eq!(adapter.get(&key).unwrap(), Some(100));
        assert_eq!(adapter.len().unwrap(), 1);
    }

    /// 工厂：LSM config → LsmAdapter
    #[test]
    fn phase_019_factory_creates_lsm_adapter() {
        let config = EngineConfig {
            engine_type: EngineType::Lsm,
            order: 0, // LSM 不使用 order
            write_buffer_size: 1024,
        };
        let mut adapter = create_adapter(&config);
        assert_eq!(adapter.engine_name(), "LSM");
        assert!(adapter.is_empty().unwrap());

        // 基本 CRUD 验证
        let key = make_key(42);
        adapter.insert(&key, 100).unwrap();
        assert_eq!(adapter.get(&key).unwrap(), Some(100));
        assert_eq!(adapter.len().unwrap(), 1);
    }

    // -----------------------------------------------------------------
    //  Phase 1.9 — trait object 动态分发测试
    // -----------------------------------------------------------------

    /// 通过 Box<dyn IndexAdapter> 动态分发，BTree 和 LSM 行为一致
    ///
    /// 这是 IndexAdapter trait 的核心价值：上层不感知底层引擎，
    /// 切换引擎只需替换 create_adapter 的配置。
    #[test]
    fn phase_019_trait_object_dynamic_dispatch_btree_and_lsm_equivalent() {
        let btree_config = EngineConfig {
            engine_type: EngineType::BTree,
            order: 16,
            write_buffer_size: 0,
        };
        let lsm_config = EngineConfig {
            engine_type: EngineType::Lsm,
            order: 0,
            write_buffer_size: 0,
        };

        let mut btree_adapter: Box<dyn IndexAdapter> = create_adapter(&btree_config);
        let mut lsm_adapter: Box<dyn IndexAdapter> = create_adapter(&lsm_config);

        // 同步插入 500 个 key
        for i in 0..500i64 {
            let key = make_key(i);
            let val = i as u16;
            btree_adapter.insert(&key, val).unwrap();
            lsm_adapter.insert(&key, val).unwrap();
        }

        // 长度一致
        assert_eq!(btree_adapter.len().unwrap(), 500);
        assert_eq!(lsm_adapter.len().unwrap(), 500);

        // 点查一致
        for i in 0..500i64 {
            let key = make_key(i);
            assert_eq!(
                btree_adapter.get(&key).unwrap(),
                lsm_adapter.get(&key).unwrap(),
                "point lookup mismatch at i={}",
                i
            );
            assert_eq!(btree_adapter.get(&key).unwrap(), Some(i as u16));
        }

        // 范围扫描一致
        let lower = make_key(100);
        let upper = make_key(200);
        let btree_scan = btree_adapter
            .range_scan(Bound::Included(&lower), Bound::Included(&upper))
            .unwrap();
        let lsm_scan = lsm_adapter
            .range_scan(Bound::Included(&lower), Bound::Included(&upper))
            .unwrap();
        assert_eq!(btree_scan.len(), lsm_scan.len());
        assert_eq!(btree_scan.len(), 101); // [100, 200] 含两端
        for (i, (bt, lsm)) in btree_scan.iter().zip(lsm_scan.iter()).enumerate() {
            assert_eq!(bt.0, lsm.0, "key mismatch at {}", i);
            assert_eq!(bt.1, lsm.1, "value mismatch at {}", i);
        }

        // 删除一致
        for i in (0..500i64).step_by(3) {
            let key = make_key(i);
            assert_eq!(
                btree_adapter.delete(&key).unwrap(),
                lsm_adapter.delete(&key).unwrap(),
                "delete return value mismatch at i={}",
                i
            );
        }

        // 删除后状态一致
        assert_eq!(btree_adapter.len().unwrap(), lsm_adapter.len().unwrap());

        // 删除后点查一致
        for i in 0..500i64 {
            let key = make_key(i);
            assert_eq!(
                btree_adapter.get(&key).unwrap(),
                lsm_adapter.get(&key).unwrap(),
                "post-delete point lookup mismatch at i={}",
                i
            );
        }
    }

    /// 通过 trait object 在 Vec 中混合使用多种适配器
    #[test]
    fn phase_019_trait_object_vec_of_mixed_adapters() {
        let configs = [
            EngineConfig {
                engine_type: EngineType::BTree,
                order: 8,
                write_buffer_size: 0,
            },
            EngineConfig {
                engine_type: EngineType::Lsm,
                order: 0,
                write_buffer_size: 0,
            },
            EngineConfig {
                engine_type: EngineType::BTree,
                order: 256,
                write_buffer_size: 0,
            },
        ];

        let mut adapters: Vec<Box<dyn IndexAdapter>> =
            configs.iter().map(|c| create_adapter(c)).collect();

        // 在所有适配器中插入相同 key
        let key = make_key(42);
        for adapter in adapters.iter_mut() {
            adapter.insert(&key, 100).unwrap();
        }

        // 验证每个适配器都能查到
        for (i, adapter) in adapters.iter().enumerate() {
            assert_eq!(
                adapter.get(&key).unwrap(),
                Some(100),
                "adapter {} ({}) should find key",
                i,
                adapter.engine_name()
            );
            assert_eq!(adapter.len().unwrap(), 1);
        }

        // 引擎名称各不相同
        assert_eq!(adapters[0].engine_name(), "BTree");
        assert_eq!(adapters[1].engine_name(), "LSM");
        assert_eq!(adapters[2].engine_name(), "BTree");
    }

    /// 通过 select_engine + create_adapter 端到端：根据工作负载自动选择引擎
    #[test]
    fn phase_019_end_to_end_select_and_create_based_on_workload() {
        // 写密集 → LSM
        let write_heavy = WorkloadCharacteristics {
            read_ratio: 0.2,
            write_ratio: 0.8,
            data_size: 0,
            point_query_ratio: 0.5,
            range_scan_ratio: 0.5,
        };
        let engine_type = select_engine(&write_heavy);
        assert_eq!(engine_type, EngineType::Lsm);
        let config = EngineConfig {
            engine_type,
            order: 256,
            write_buffer_size: 64 * 1024 * 1024,
        };
        let adapter = create_adapter(&config);
        assert_eq!(adapter.engine_name(), "LSM");

        // 读密集 → BTree
        let read_heavy = WorkloadCharacteristics {
            read_ratio: 0.9,
            write_ratio: 0.1,
            data_size: 0,
            point_query_ratio: 0.8,
            range_scan_ratio: 0.2,
        };
        let engine_type = select_engine(&read_heavy);
        assert_eq!(engine_type, EngineType::BTree);
        let config = EngineConfig {
            engine_type,
            order: 256,
            write_buffer_size: 0,
        };
        let adapter = create_adapter(&config);
        assert_eq!(adapter.engine_name(), "BTree");
    }

    // -----------------------------------------------------------------
    //  Phase 1.9 — 错误处理测试
    // -----------------------------------------------------------------

    /// BTreeAdapter 在无效范围（lower > upper）下返回空 Vec 而非 panic
    #[test]
    fn phase_019_btree_adapter_invalid_range_returns_empty_not_panic() {
        let mut adapter = BTreeAdapter::with_default_order();
        insert_sequential_keys(&mut adapter, 0, 100);

        // lower > upper → 空 Vec（与 BTree 的语义一致）
        let lower = make_key(80);
        let upper = make_key(20);
        let result = adapter
            .range_scan(Bound::Included(&lower), Bound::Included(&upper))
            .unwrap();
        assert_eq!(result.len(), 0);
    }

    /// LsmAdapter 在无效范围（lower > upper）下返回空 Vec 而非 panic
    ///
    /// **关键**：底层 BTreeMap::range 在 lower > upper 时会 panic，
    /// LsmAdapter 用 catch_unwind 兜底为空 Vec，与 BTreeAdapter 语义一致。
    #[test]
    fn phase_019_lsm_adapter_invalid_range_returns_empty_not_panic() {
        let mut adapter = LsmAdapter::new();
        insert_sequential_keys(&mut adapter, 0, 100);

        // lower > upper → 空 Vec（不 panic）
        let lower = make_key(80);
        let upper = make_key(20);
        let result = adapter
            .range_scan(Bound::Included(&lower), Bound::Included(&upper))
            .unwrap();
        assert_eq!(result.len(), 0);
    }

    /// BTreeAdapter 与 LsmAdapter 在无效范围下行为一致
    #[test]
    fn phase_019_both_adapters_invalid_range_consistent() {
        let mut btree = BTreeAdapter::with_default_order();
        let mut lsm = LsmAdapter::new();

        // 同步插入
        for i in 0..100i64 {
            let key = make_key(i);
            btree.insert(&key, i as u16).unwrap();
            lsm.insert(&key, i as u16).unwrap();
        }

        // 多种无效范围组合
        let invalid_ranges = vec![
            (make_key(50), make_key(10)),
            (make_key(99), make_key(0)),
            (make_key(100), make_key(50)), // lower 超出所有 key
        ];

        for (lower, upper) in invalid_ranges {
            let bt_result = btree
                .range_scan(Bound::Included(&lower), Bound::Included(&upper))
                .unwrap();
            let lsm_result = lsm
                .range_scan(Bound::Included(&lower), Bound::Included(&upper))
                .unwrap();
            assert_eq!(
                bt_result.len(),
                lsm_result.len(),
                "invalid range [{:?}, {:?}] results inconsistent: BTree={}, LSM={}",
                lower,
                upper,
                bt_result.len(),
                lsm_result.len()
            );
            assert_eq!(bt_result.len(), 0);
        }
    }
}
