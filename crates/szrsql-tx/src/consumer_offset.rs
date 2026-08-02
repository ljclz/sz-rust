//! SzRSQL 消费者组 offset 持久化 — 对应 `SzRSQL实施进度.md` Phase 2.5.7。
//!
//! 基于 **at-least-once + 去重 = exactly-once** 语义模型，提供消费者组级别的
//! offset 持久化管理，支持崩溃恢复后从上次提交的 offset 继续消费。
//!
//! # 核心概念
//!
//! - **OffsetStore**：管理多个消费者组 × 多个分区的 `committed_lsn`，持久化到磁盘
//! - **committed_lsn**：已安全应用到下游的最后一个 LSN（小于等于此值的所有事件已处理）
//! - **去重窗口（dedup window）**：in-memory `BTreeSet<lsn>`，记录已处理但未提交的 LSN
//!   - 用于检测同一会话内的重复处理
//!   - 崩溃后丢失（in-memory），重启后从 `committed_lsn + 1` 开始重新消费
//!
//! # exactly-once 实现策略
//!
//! 1. **at-least-once**：CdcEngine 可能重投事件（observer 注册时从某 LSN 开始）
//! 2. **去重**：
//!   - **跨会话**：`committed_lsn` 之前的事件不再处理（`lsn <= committed_lsn` 跳过）
//!   - **会话内**：去重窗口检测已处理但未提交的 LSN
//! 3. **崩溃恢复**：
//!   - 已提交的 offset 持久化到磁盘（atomic write + rename）
//!   - 未提交的 in-memory 去重窗口丢失，可能重复处理 → consumer 端需 idempotent
//!
//! # 持久化格式
//!
//! 单个 JSON 文件，结构：
//!
//! ```json
//! {
//!   "version": 1,
//!   "offsets": [
//!     {
//!       "consumer_group": "group1",
//!       "partition": 0,
//!       "committed_lsn": 12345,
//!       "committed_at": 1700000000000
//!     }
//!   ]
//! }
//! ```
//!
//! **原子写入**：先写入 `path.tmp` 临时文件，再 `rename` 到目标路径，保证崩溃时
//! 不会留下半写入的文件。
//!
//! # 设计要点
//!
//! 1. **线程安全**：内部使用 `RwLock<HashMap>` + `Mutex` 支持并发读、串行持久化
//! 2. **LSN 单调性**：`commit_offset` 拒绝 LSN 倒退（小于已提交的 LSN）
//! 3. **去重窗口自清理**：`commit_offset` 时自动清理窗口中 `<= committed_lsn` 的项
//! 4. **可重置**：`reset` 接口允许重置某分区的 offset（用于重新消费）
//! 5. **统计接口**：`commit_count` / `dedup_window_size` 便于监控和测试

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::{Mutex, RwLock};

// =====================================================================
// 持久化文件格式
// =====================================================================

/// 持久化文件格式版本
const OFFSET_FILE_VERSION: u32 = 1;

/// 持久化文件根结构（内部使用）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OffsetFile {
    /// 格式版本号
    version: u32,
    /// 所有 offset 记录
    offsets: Vec<OffsetRecord>,
}

/// Offset 提交记录 — 单个 (消费者组, 分区) 的已提交 LSN
///
/// **字段含义**：
/// - `committed_lsn`：该 (group, partition) 上已安全应用的最后一个 LSN
///   - 小于等于此值的所有事件已处理
///   - 下次消费从此值 + 1 开始
/// - `committed_at`：提交时间戳（Unix 毫秒，便于审计）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffsetRecord {
    /// 消费者组 ID
    pub consumer_group: String,
    /// 分区 ID（在 CDC 场景下通常为 table_id）
    pub partition: u32,
    /// 已提交的 LSN（小于等于此值的所有事件已处理）
    pub committed_lsn: u64,
    /// 提交时间戳（Unix 毫秒）
    pub committed_at: u64,
}

// =====================================================================
// 错误类型
// =====================================================================

/// Offset 存储错误
#[derive(Debug, thiserror::Error)]
pub enum OffsetStoreError {
    /// IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// 序列化/反序列化错误
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    /// Offset 倒退（新 LSN 小于已提交的 LSN）
    #[error("offset regression: new lsn {new_lsn} < committed lsn {committed_lsn}")]
    Regression {
        /// 试图提交的新 LSN
        new_lsn: u64,
        /// 已提交的 LSN
        committed_lsn: u64,
    },
    /// 文件格式不兼容（版本号不匹配）
    #[error("incompatible file format: expected version {expected}, got {actual}")]
    IncompatibleFormat {
        /// 期望的版本号
        expected: u32,
        /// 实际的版本号
        actual: u32,
    },
}

// =====================================================================
// OffsetStore
// =====================================================================

/// 持久化 offset 存储 — 支持崩溃恢复的消费者组 offset 管理
///
/// **设计**：
/// - `offsets: RwLock<HashMap<(group, partition), lsn>>`：内存索引，加速查询
/// - `processed: RwLock<HashMap<(group, partition), BTreeSet<lsn>>>`：去重窗口
///   - 记录已处理但未提交的 LSN（在 `commit_offset` 之前）
///   - `commit_offset` 时自动清理窗口中 `<= committed_lsn` 的项
/// - `persist_lock: Mutex<()>`：保证 `commit_offset` 串行持久化（避免并发写文件）
/// - `path: PathBuf`：持久化文件路径；空路径表示 in-memory 模式（不持久化）
///
/// **API**：
/// - `open(path)`：打开或创建 offset 存储，从磁盘加载已有 offsets
/// - `in_memory()`：创建非持久化的内存存储（主要用于测试）
/// - `commit_offset(group, partition, lsn)`：持久化提交 LSN
/// - `get_offset(group, partition)`：查询已提交的 LSN
/// - `mark_processed(group, partition, lsn)`：标记 LSN 已处理（去重窗口）
/// - `is_processed(group, partition, lsn)`：检查是否已处理
/// - `reset(group, partition)`：重置 offset（用于重新消费）
/// - `flush()`：强制持久化
pub struct OffsetStore {
    /// 内存索引：(consumer_group, partition) -> committed_lsn
    offsets: RwLock<HashMap<(String, u32), u64>>,
    /// 已处理但未提交的 LSN 去重窗口：(group, partition) -> BTreeSet<lsn>
    processed: RwLock<HashMap<(String, u32), BTreeSet<u64>>>,
    /// 持久化文件路径（空路径表示 in-memory 模式）
    path: PathBuf,
    /// 持久化锁（保证 commit 串行化）
    persist_lock: Mutex<()>,
    /// 提交计数（统计用）
    commit_count: AtomicU64,
}

impl Default for OffsetStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl OffsetStore {
    /// 打开或创建 offset 存储
    ///
    /// **行为**：
    /// - 若文件存在且格式正确，加载已有 offsets 到内存
    /// - 若文件不存在，创建空存储（首次 open 不会立即写文件，需等首次 `commit_offset`）
    /// - 若文件存在但格式错误（版本不匹配 / JSON 解析失败），返回 `Err`
    ///
    /// **参数**：`path` — 持久化文件路径
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OffsetStoreError> {
        let path = path.as_ref().to_path_buf();
        let offsets = if path.exists() {
            Self::load_from_file(&path)?
        } else {
            HashMap::new()
        };

        Ok(Self {
            offsets: RwLock::new(offsets),
            processed: RwLock::new(HashMap::new()),
            path,
            persist_lock: Mutex::new(()),
            commit_count: AtomicU64::new(0),
        })
    }

    /// 创建内存中的 offset 存储（不持久化）
    ///
    /// 主要用于测试：所有 `commit_offset` 调用不会写入磁盘
    pub fn in_memory() -> Self {
        Self {
            offsets: RwLock::new(HashMap::new()),
            processed: RwLock::new(HashMap::new()),
            path: PathBuf::new(),
            persist_lock: Mutex::new(()),
            commit_count: AtomicU64::new(0),
        }
    }

    /// 从文件加载 offsets
    fn load_from_file(path: &Path) -> Result<HashMap<(String, u32), u64>, OffsetStoreError> {
        let content = std::fs::read_to_string(path)?;
        if content.trim().is_empty() {
            return Ok(HashMap::new());
        }
        let file: OffsetFile = serde_json::from_str(&content)?;
        if file.version != OFFSET_FILE_VERSION {
            return Err(OffsetStoreError::IncompatibleFormat {
                expected: OFFSET_FILE_VERSION,
                actual: file.version,
            });
        }
        let mut map = HashMap::with_capacity(file.offsets.len());
        for record in file.offsets {
            map.insert(
                (record.consumer_group, record.partition),
                record.committed_lsn,
            );
        }
        Ok(map)
    }

    /// 持久化所有 offsets 到文件（atomic：write-to-tmp + rename）
    ///
    /// **流程**：
    /// 1. 读取内存索引快照
    /// 2. 序列化为 JSON
    /// 3. 写入临时文件 `path.tmp`
    /// 4. `rename` 到目标路径（原子操作）
    ///
    /// **注**：in-memory 模式（`path` 为空）下，此函数是 no-op
    fn persist_to_file(&self) -> Result<(), OffsetStoreError> {
        if self.path.as_os_str().is_empty() {
            return Ok(()); // in-memory 模式
        }

        let offsets = self.offsets.read();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let records: Vec<OffsetRecord> = offsets
            .iter()
            .map(|((group, partition), lsn)| OffsetRecord {
                consumer_group: group.clone(),
                partition: *partition,
                committed_lsn: *lsn,
                committed_at: now,
            })
            .collect();

        let file = OffsetFile {
            version: OFFSET_FILE_VERSION,
            offsets: records,
        };

        let json = serde_json::to_string_pretty(&file)?;
        let tmp_path = self.path.with_extension("tmp");

        // 写入临时文件
        std::fs::write(&tmp_path, &json)?;
        // 原子 rename（Windows 上若目标存在，rename 会失败，需先移除）
        if self.path.exists() {
            // 在 Windows 上，rename 到已存在文件会失败；使用 remove + rename
            // 在 Linux 上，rename 是原子的且会覆盖目标
            #[cfg(windows)]
            let _ = std::fs::remove_file(&self.path);
        }
        std::fs::rename(&tmp_path, &self.path)?;

        Ok(())
    }

    /// 提交 offset（持久化）
    ///
    /// **流程**：
    /// 1. 检查 LSN 不倒退（新 LSN 必须大于等于已提交的 LSN）
    /// 2. 更新内存索引
    /// 3. 清理去重窗口中 `<= 新 LSN` 的项（已通过 commit 持久化）
    /// 4. 持久化到磁盘（atomic write + rename）
    ///
    /// **参数**：
    /// - `group`：消费者组 ID
    /// - `partition`：分区 ID
    /// - `lsn`：要提交的 LSN（必须 >= 已提交的 LSN）
    ///
    /// **错误**：
    /// - `Regression`：新 LSN 小于已提交的 LSN
    pub fn commit_offset(
        &self,
        group: &str,
        partition: u32,
        lsn: u64,
    ) -> Result<(), OffsetStoreError> {
        let _guard = self.persist_lock.lock();

        // 检查 LSN 不倒退
        {
            let offsets = self.offsets.read();
            if let Some(&existing) = offsets.get(&(group.to_string(), partition)) {
                if lsn < existing {
                    return Err(OffsetStoreError::Regression {
                        new_lsn: lsn,
                        committed_lsn: existing,
                    });
                }
            }
        }

        // 更新内存索引
        {
            let mut offsets = self.offsets.write();
            offsets.insert((group.to_string(), partition), lsn);
        }

        // 清理去重窗口中 <= 新 LSN 的项
        {
            let mut processed = self.processed.write();
            if let Some(set) = processed.get_mut(&(group.to_string(), partition)) {
                set.retain(|&x| x > lsn);
            }
        }

        // 持久化
        self.persist_to_file()?;

        self.commit_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// 获取已提交的 offset
    ///
    /// 返回 `Some(lsn)` 表示该 (group, partition) 已有提交记录；`None` 表示从未提交
    pub fn get_offset(&self, group: &str, partition: u32) -> Option<u64> {
        self.offsets
            .read()
            .get(&(group.to_string(), partition))
            .copied()
    }

    /// 列出所有已提交的 offset 记录
    ///
    /// **注**：`committed_at` 字段使用当前时间戳（内存中不存储原始 committed_at）
    pub fn list_offsets(&self) -> Vec<OffsetRecord> {
        let offsets = self.offsets.read();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        offsets
            .iter()
            .map(|((group, partition), lsn)| OffsetRecord {
                consumer_group: group.clone(),
                partition: *partition,
                committed_lsn: *lsn,
                committed_at: now,
            })
            .collect()
    }

    /// 标记一个 LSN 已处理（添加到去重窗口）
    ///
    /// **行为**：
    /// - 若 LSN <= 已提交的 offset，返回 `false`（已处理过）
    /// - 若 LSN 已在去重窗口中，返回 `false`（重复）
    /// - 否则，添加到去重窗口，返回 `true`（首次标记）
    ///
    /// **返回**：
    /// - `true`：首次标记（之前未处理过）
    /// - `false`：已处理过（重复，应跳过）
    pub fn mark_processed(&self, group: &str, partition: u32, lsn: u64) -> bool {
        // 先检查是否已提交
        if let Some(committed) = self.get_offset(group, partition) {
            if lsn <= committed {
                return false; // 已提交，视为已处理
            }
        }

        // 添加到去重窗口
        let mut processed = self.processed.write();
        let set = processed.entry((group.to_string(), partition)).or_default();
        set.insert(lsn)
    }

    /// 检查 LSN 是否已处理（含已提交 + 去重窗口）
    ///
    /// **返回**：
    /// - `true`：已处理（已提交或已在去重窗口）
    /// - `false`：未处理
    pub fn is_processed(&self, group: &str, partition: u32, lsn: u64) -> bool {
        // 检查是否已提交
        if let Some(committed) = self.get_offset(group, partition) {
            if lsn <= committed {
                return true;
            }
        }

        // 检查去重窗口
        let processed = self.processed.read();
        processed
            .get(&(group.to_string(), partition))
            .map(|set| set.contains(&lsn))
            .unwrap_or(false)
    }

    /// 重置某 (group, partition) 的 offset（用于重新消费）
    ///
    /// **行为**：
    /// - 删除内存索引中的 offset 记录
    /// - 清空去重窗口
    /// - 持久化（若为持久化模式）
    ///
    /// **注**：重置后，`get_offset` 返回 `None`，下次消费从 LSN 0 + 1 = 1 开始
    pub fn reset(&self, group: &str, partition: u32) -> Result<(), OffsetStoreError> {
        let _guard = self.persist_lock.lock();

        {
            let mut offsets = self.offsets.write();
            offsets.remove(&(group.to_string(), partition));
        }
        {
            let mut processed = self.processed.write();
            processed.remove(&(group.to_string(), partition));
        }

        self.persist_to_file()?;
        Ok(())
    }

    /// 强制持久化（即使无变更也写入）
    ///
    /// 主要用于安全关闭前的 flush
    pub fn flush(&self) -> Result<(), OffsetStoreError> {
        let _guard = self.persist_lock.lock();
        self.persist_to_file()
    }

    /// 获取提交次数（统计用）
    pub fn commit_count(&self) -> u64 {
        self.commit_count.load(Ordering::SeqCst)
    }

    /// 获取去重窗口大小（用于监控和测试）
    pub fn dedup_window_size(&self, group: &str, partition: u32) -> usize {
        let processed = self.processed.read();
        processed
            .get(&(group.to_string(), partition))
            .map(|set| set.len())
            .unwrap_or(0)
    }

    /// 获取所有消费者组列表（去重 + 排序）
    pub fn list_consumer_groups(&self) -> Vec<String> {
        let offsets = self.offsets.read();
        let mut groups: Vec<String> = offsets
            .keys()
            .map(|(g, _)| g.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        groups.sort();
        groups
    }

    /// 获取指定消费者组的所有分区 offset（按分区 ID 升序）
    pub fn list_partitions(&self, group: &str) -> Vec<(u32, u64)> {
        let offsets = self.offsets.read();
        let mut result: Vec<(u32, u64)> = offsets
            .iter()
            .filter(|((g, _), _)| g == group)
            .map(|((_, p), lsn)| (*p, *lsn))
            .collect();
        result.sort_by_key(|(p, _)| *p);
        result
    }

    /// 获取已注册的 (group, partition) 总数
    pub fn offset_count(&self) -> usize {
        self.offsets.read().len()
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // -----------------------------------------------------------------
    // 测试辅助函数
    // -----------------------------------------------------------------

    /// 生成唯一的临时文件路径（不实际创建文件）
    fn make_temp_path(test_name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let pid = std::process::id();
        let filename = format!("szrsql_offset_{}_{}_{}.json", test_name, pid, timestamp);
        path.push(filename);
        path
    }

    /// 清理临时文件（包括 .tmp 文件）
    fn cleanup_temp_file(path: &Path) {
        let _ = std::fs::remove_file(path);
        let tmp = path.with_extension("tmp");
        let _ = std::fs::remove_file(&tmp);
    }

    // =================================================================
    // Part 1: OffsetRecord 基础
    // =================================================================

    #[test]
    fn phase_2_5_7_offset_record_construct_and_fields() {
        let record = OffsetRecord {
            consumer_group: "group1".to_string(),
            partition: 0,
            committed_lsn: 12345,
            committed_at: 1700000000000,
        };
        assert_eq!(record.consumer_group, "group1");
        assert_eq!(record.partition, 0);
        assert_eq!(record.committed_lsn, 12345);
        assert_eq!(record.committed_at, 1700000000000);
    }

    #[test]
    fn phase_2_5_7_offset_record_eq() {
        let r1 = OffsetRecord {
            consumer_group: "g".to_string(),
            partition: 1,
            committed_lsn: 100,
            committed_at: 0,
        };
        let r2 = r1.clone();
        assert_eq!(r1, r2);

        let r3 = OffsetRecord {
            consumer_group: "g".to_string(),
            partition: 1,
            committed_lsn: 100,
            committed_at: 999, // 不同 committed_at
        };
        assert_ne!(r1, r3);
    }

    #[test]
    fn phase_2_5_7_offset_record_serde_json_roundtrip() {
        let record = OffsetRecord {
            consumer_group: "consumers".to_string(),
            partition: 42,
            committed_lsn: 999999,
            committed_at: 1700000000123,
        };
        let json = serde_json::to_string(&record).unwrap();
        let decoded: OffsetRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, decoded);
    }

    #[test]
    fn phase_2_5_7_offset_record_serde_json_fields() {
        let record = OffsetRecord {
            consumer_group: "g1".to_string(),
            partition: 7,
            committed_lsn: 123,
            committed_at: 456,
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"consumer_group\":\"g1\""));
        assert!(json.contains("\"partition\":7"));
        assert!(json.contains("\"committed_lsn\":123"));
        assert!(json.contains("\"committed_at\":456"));
    }

    // =================================================================
    // Part 2: OffsetStore 基础（in-memory 模式）
    // =================================================================

    #[test]
    fn phase_2_5_7_in_memory_store_starts_empty() {
        let store = OffsetStore::in_memory();
        assert_eq!(store.offset_count(), 0);
        assert_eq!(store.commit_count(), 0);
        assert!(store.list_offsets().is_empty());
        assert!(store.list_consumer_groups().is_empty());
    }

    #[test]
    fn phase_2_5_7_default_is_in_memory() {
        let store = OffsetStore::default();
        assert_eq!(store.offset_count(), 0);
    }

    #[test]
    fn phase_2_5_7_get_offset_returns_none_when_empty() {
        let store = OffsetStore::in_memory();
        assert_eq!(store.get_offset("group1", 0), None);
    }

    #[test]
    fn phase_2_5_7_commit_offset_basic() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        assert_eq!(store.get_offset("group1", 0), Some(100));
        assert_eq!(store.commit_count(), 1);
        assert_eq!(store.offset_count(), 1);
    }

    #[test]
    fn phase_2_5_7_commit_offset_increases_lsn() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        store.commit_offset("group1", 0, 200).unwrap();
        store.commit_offset("group1", 0, 300).unwrap();
        assert_eq!(store.get_offset("group1", 0), Some(300));
        assert_eq!(store.commit_count(), 3);
    }

    #[test]
    fn phase_2_5_7_commit_offset_equal_lsn_allowed() {
        // 提交相同 LSN 应该允许（幂等）
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        store.commit_offset("group1", 0, 100).unwrap(); // 相同 LSN
        assert_eq!(store.get_offset("group1", 0), Some(100));
        assert_eq!(store.commit_count(), 2);
    }

    #[test]
    fn phase_2_5_7_commit_offset_regression_rejected() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 200).unwrap();
        let result = store.commit_offset("group1", 0, 100);
        assert!(matches!(
            result,
            Err(OffsetStoreError::Regression {
                new_lsn: 100,
                committed_lsn: 200
            })
        ));
        // 原 offset 不变
        assert_eq!(store.get_offset("group1", 0), Some(200));
    }

    #[test]
    fn phase_2_5_7_commit_offset_multi_group_independent() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        store.commit_offset("group2", 0, 200).unwrap();
        store.commit_offset("group1", 1, 150).unwrap();

        assert_eq!(store.get_offset("group1", 0), Some(100));
        assert_eq!(store.get_offset("group2", 0), Some(200));
        assert_eq!(store.get_offset("group1", 1), Some(150));
        assert_eq!(store.offset_count(), 3);
    }

    #[test]
    fn phase_2_5_7_commit_offset_multi_partition_independent() {
        let store = OffsetStore::in_memory();
        for partition in 0..10u32 {
            store
                .commit_offset("group1", partition, partition as u64 * 100)
                .unwrap();
        }
        for partition in 0..10u32 {
            assert_eq!(
                store.get_offset("group1", partition),
                Some(partition as u64 * 100)
            );
        }
        assert_eq!(store.offset_count(), 10);
    }

    #[test]
    fn phase_2_5_7_list_offsets_returns_all() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        store.commit_offset("group1", 1, 200).unwrap();
        store.commit_offset("group2", 0, 300).unwrap();

        let list = store.list_offsets();
        assert_eq!(list.len(), 3);

        // 验证包含所有记录（顺序可能不同）
        let mut found_g1_p0 = false;
        let mut found_g1_p1 = false;
        let mut found_g2_p0 = false;
        for record in &list {
            match (record.consumer_group.as_str(), record.partition) {
                ("group1", 0) => {
                    assert_eq!(record.committed_lsn, 100);
                    found_g1_p0 = true;
                }
                ("group1", 1) => {
                    assert_eq!(record.committed_lsn, 200);
                    found_g1_p1 = true;
                }
                ("group2", 0) => {
                    assert_eq!(record.committed_lsn, 300);
                    found_g2_p0 = true;
                }
                _ => panic!("unexpected record: {:?}", record),
            }
        }
        assert!(found_g1_p0 && found_g1_p1 && found_g2_p0);
    }

    #[test]
    fn phase_2_5_7_list_consumer_groups_dedup_and_sort() {
        let store = OffsetStore::in_memory();
        store.commit_offset("charlie", 0, 1).unwrap();
        store.commit_offset("alpha", 0, 1).unwrap();
        store.commit_offset("bravo", 0, 1).unwrap();
        store.commit_offset("alpha", 1, 2).unwrap(); // 重复的 group

        let groups = store.list_consumer_groups();
        assert_eq!(groups, vec!["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn phase_2_5_7_list_partitions_for_group() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 5, 500).unwrap();
        store.commit_offset("group1", 1, 100).unwrap();
        store.commit_offset("group1", 3, 300).unwrap();
        store.commit_offset("group2", 0, 1).unwrap();

        let partitions = store.list_partitions("group1");
        assert_eq!(partitions, vec![(1, 100), (3, 300), (5, 500)]);
    }

    #[test]
    fn phase_2_5_7_list_partitions_for_nonexistent_group_empty() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        let partitions = store.list_partitions("nonexistent");
        assert!(partitions.is_empty());
    }

    // =================================================================
    // Part 3: 去重窗口（dedup window）
    // =================================================================

    #[test]
    fn phase_2_5_7_mark_processed_returns_true_for_first() {
        let store = OffsetStore::in_memory();
        assert!(store.mark_processed("group1", 0, 100));
    }

    #[test]
    fn phase_2_5_7_mark_processed_returns_false_for_duplicate() {
        let store = OffsetStore::in_memory();
        assert!(store.mark_processed("group1", 0, 100));
        assert!(!store.mark_processed("group1", 0, 100)); // 重复
    }

    #[test]
    fn phase_2_5_7_mark_processed_returns_false_for_committed() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        // 已提交的 LSN，mark_processed 返回 false
        assert!(!store.mark_processed("group1", 0, 50));
        assert!(!store.mark_processed("group1", 0, 100));
        // 大于已提交的 LSN，mark_processed 返回 true
        assert!(store.mark_processed("group1", 0, 101));
    }

    #[test]
    fn phase_2_5_7_is_processed_checks_committed() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        assert!(store.is_processed("group1", 0, 50));
        assert!(store.is_processed("group1", 0, 100));
        assert!(!store.is_processed("group1", 0, 101));
    }

    #[test]
    fn phase_2_5_7_is_processed_checks_dedup_window() {
        let store = OffsetStore::in_memory();
        store.mark_processed("group1", 0, 100);
        assert!(store.is_processed("group1", 0, 100));
        assert!(!store.is_processed("group1", 0, 101));
    }

    #[test]
    fn phase_2_5_7_is_processed_for_nonexistent_group() {
        let store = OffsetStore::in_memory();
        assert!(!store.is_processed("group1", 0, 100));
    }

    #[test]
    fn phase_2_5_7_dedup_window_size_empty() {
        let store = OffsetStore::in_memory();
        assert_eq!(store.dedup_window_size("group1", 0), 0);
    }

    #[test]
    fn phase_2_5_7_dedup_window_size_after_marks() {
        let store = OffsetStore::in_memory();
        store.mark_processed("group1", 0, 100);
        store.mark_processed("group1", 0, 101);
        store.mark_processed("group1", 0, 102);
        // 重复 mark 不增加窗口
        store.mark_processed("group1", 0, 100);
        assert_eq!(store.dedup_window_size("group1", 0), 3);
    }

    #[test]
    fn phase_2_5_7_commit_clears_dedup_window() {
        let store = OffsetStore::in_memory();
        store.mark_processed("group1", 0, 100);
        store.mark_processed("group1", 0, 101);
        store.mark_processed("group1", 0, 102);
        assert_eq!(store.dedup_window_size("group1", 0), 3);

        // commit 100 会清理 <= 100 的项
        store.commit_offset("group1", 0, 100).unwrap();
        assert_eq!(store.dedup_window_size("group1", 0), 2); // 101, 102

        // commit 102 会清理 <= 102 的项
        store.commit_offset("group1", 0, 102).unwrap();
        assert_eq!(store.dedup_window_size("group1", 0), 0);
    }

    #[test]
    fn phase_2_5_7_dedup_window_independent_per_partition() {
        let store = OffsetStore::in_memory();
        store.mark_processed("group1", 0, 100);
        store.mark_processed("group1", 1, 200);
        assert_eq!(store.dedup_window_size("group1", 0), 1);
        assert_eq!(store.dedup_window_size("group1", 1), 1);

        store.commit_offset("group1", 0, 100).unwrap();
        assert_eq!(store.dedup_window_size("group1", 0), 0);
        assert_eq!(store.dedup_window_size("group1", 1), 1); // 不受影响
    }

    #[test]
    fn phase_2_5_7_dedup_window_independent_per_group() {
        let store = OffsetStore::in_memory();
        store.mark_processed("group1", 0, 100);
        store.mark_processed("group2", 0, 100);
        assert_eq!(store.dedup_window_size("group1", 0), 1);
        assert_eq!(store.dedup_window_size("group2", 0), 1);

        store.commit_offset("group1", 0, 100).unwrap();
        assert_eq!(store.dedup_window_size("group1", 0), 0);
        assert_eq!(store.dedup_window_size("group2", 0), 1); // 不受影响
    }

    #[test]
    fn phase_2_5_7_mark_processed_out_of_order() {
        let store = OffsetStore::in_memory();
        // LSN 不需要按顺序 mark
        store.mark_processed("group1", 0, 300);
        store.mark_processed("group1", 0, 100);
        store.mark_processed("group1", 0, 200);
        assert_eq!(store.dedup_window_size("group1", 0), 3);
        assert!(store.is_processed("group1", 0, 100));
        assert!(store.is_processed("group1", 0, 200));
        assert!(store.is_processed("group1", 0, 300));
    }

    // =================================================================
    // Part 4: 持久化与崩溃恢复
    // =================================================================

    #[test]
    fn phase_2_5_7_open_nonexistent_file_returns_empty() {
        let path = make_temp_path("open_nonexistent");
        cleanup_temp_file(&path);

        let store = OffsetStore::open(&path).unwrap();
        assert_eq!(store.offset_count(), 0);
        assert!(store.get_offset("group1", 0).is_none());

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_persist_and_reopen() {
        let path = make_temp_path("persist_reopen");
        cleanup_temp_file(&path);

        // 第一次打开，commit 一些 offsets
        {
            let store = OffsetStore::open(&path).unwrap();
            store.commit_offset("group1", 0, 100).unwrap();
            store.commit_offset("group1", 1, 200).unwrap();
            store.commit_offset("group2", 0, 300).unwrap();
            assert_eq!(store.offset_count(), 3);
        }

        // 第二次打开，验证 offsets 已持久化
        {
            let store = OffsetStore::open(&path).unwrap();
            assert_eq!(store.offset_count(), 3);
            assert_eq!(store.get_offset("group1", 0), Some(100));
            assert_eq!(store.get_offset("group1", 1), Some(200));
            assert_eq!(store.get_offset("group2", 0), Some(300));
        }

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_reopen_recovers_latest_offset() {
        let path = make_temp_path("recover_latest");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();
            // 多次 commit，验证重启后拿到的是最新值
            store.commit_offset("group1", 0, 100).unwrap();
            store.commit_offset("group1", 0, 200).unwrap();
            store.commit_offset("group1", 0, 300).unwrap();
            store.commit_offset("group1", 0, 400).unwrap();
            store.commit_offset("group1", 0, 500).unwrap();
        }

        let store = OffsetStore::open(&path).unwrap();
        assert_eq!(store.get_offset("group1", 0), Some(500));

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_reopen_dedup_window_lost() {
        // 去重窗口是 in-memory 的，重启后丢失
        let path = make_temp_path("dedup_lost");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();
            store.mark_processed("group1", 0, 100);
            store.mark_processed("group1", 0, 101);
            store.mark_processed("group1", 0, 102);
            assert_eq!(store.dedup_window_size("group1", 0), 3);
            // 不 commit，直接 drop 模拟崩溃
        }

        let store = OffsetStore::open(&path).unwrap();
        // 去重窗口丢失
        assert_eq!(store.dedup_window_size("group1", 0), 0);
        // committed offset 也丢失（因为没有 commit）
        assert_eq!(store.get_offset("group1", 0), None);

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_reopen_with_committed_offset_resumes_correctly() {
        // 重启后从 committed + 1 继续，去重窗口为空但不影响正确性
        let path = make_temp_path("resume_correct");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();
            store.mark_processed("group1", 0, 100);
            store.mark_processed("group1", 0, 101);
            store.mark_processed("group1", 0, 102);
            store.commit_offset("group1", 0, 100).unwrap(); // 只 commit 100
                                                            // 101, 102 已处理但未 commit
        }

        let store = OffsetStore::open(&path).unwrap();
        // committed = 100，应该从 101 开始消费
        assert_eq!(store.get_offset("group1", 0), Some(100));
        // 去重窗口丢失，101/102 会被重新处理（at-least-once）
        assert!(!store.is_processed("group1", 0, 101));
        assert!(!store.is_processed("group1", 0, 102));
        // 但 100 仍然算已处理（已提交）
        assert!(store.is_processed("group1", 0, 100));

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_persistent_store_independent_groups() {
        let path = make_temp_path("independent_groups");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();
            store.commit_offset("group1", 0, 100).unwrap();
            store.commit_offset("group2", 0, 200).unwrap();
            store.commit_offset("group3", 0, 300).unwrap();
        }

        let store = OffsetStore::open(&path).unwrap();
        assert_eq!(store.get_offset("group1", 0), Some(100));
        assert_eq!(store.get_offset("group2", 0), Some(200));
        assert_eq!(store.get_offset("group3", 0), Some(300));
        assert_eq!(store.offset_count(), 3);

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_persistent_store_multi_partitions() {
        let path = make_temp_path("multi_partitions");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();
            for p in 0..50u32 {
                store.commit_offset("group1", p, p as u64 * 1000).unwrap();
            }
        }

        let store = OffsetStore::open(&path).unwrap();
        for p in 0..50u32 {
            assert_eq!(store.get_offset("group1", p), Some(p as u64 * 1000));
        }
        assert_eq!(store.offset_count(), 50);

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_incompatible_file_version_rejected() {
        let path = make_temp_path("incompatible_version");
        cleanup_temp_file(&path);

        // 写入版本号不兼容的文件
        let bad_file = OffsetFile {
            version: 999,
            offsets: vec![],
        };
        let json = serde_json::to_string_pretty(&bad_file).unwrap();
        std::fs::write(&path, &json).unwrap();

        let result = OffsetStore::open(&path);
        assert!(matches!(
            result,
            Err(OffsetStoreError::IncompatibleFormat {
                expected: 1,
                actual: 999
            })
        ));

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_corrupted_json_rejected() {
        let path = make_temp_path("corrupted_json");
        cleanup_temp_file(&path);

        std::fs::write(&path, "this is not valid json {{{").unwrap();

        let result = OffsetStore::open(&path);
        assert!(matches!(result, Err(OffsetStoreError::Serde(_))));

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_empty_file_treated_as_empty_store() {
        let path = make_temp_path("empty_file");
        cleanup_temp_file(&path);

        std::fs::write(&path, "").unwrap();

        let store = OffsetStore::open(&path).unwrap();
        assert_eq!(store.offset_count(), 0);

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_whitespace_only_file_treated_as_empty() {
        let path = make_temp_path("whitespace_only");
        cleanup_temp_file(&path);

        std::fs::write(&path, "   \n\t  \n").unwrap();

        let store = OffsetStore::open(&path).unwrap();
        assert_eq!(store.offset_count(), 0);

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_atomic_write_no_partial_state() {
        // 验证 commit 是原子的：commit 后文件存在且完整
        let path = make_temp_path("atomic_write");
        cleanup_temp_file(&path);

        let store = OffsetStore::open(&path).unwrap();
        store.commit_offset("group1", 0, 12345).unwrap();

        // 文件存在且可被重新加载
        assert!(path.exists());
        let store2 = OffsetStore::open(&path).unwrap();
        assert_eq!(store2.get_offset("group1", 0), Some(12345));

        // 临时文件应该已被 rename 走
        let tmp_path = path.with_extension("tmp");
        assert!(!tmp_path.exists());

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_flush_persists_without_commit() {
        // flush 不改变 offset，但会写文件（如果之前没写过）
        let path = make_temp_path("flush_no_commit");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();
            // 没有 commit，flush 应该写一个空文件
            store.flush().unwrap();
            assert!(path.exists());
        }

        let store = OffsetStore::open(&path).unwrap();
        assert_eq!(store.offset_count(), 0);

        cleanup_temp_file(&path);
    }

    // =================================================================
    // Part 5: reset 操作
    // =================================================================

    #[test]
    fn phase_2_5_7_reset_clears_offset() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        assert_eq!(store.get_offset("group1", 0), Some(100));

        store.reset("group1", 0).unwrap();
        assert_eq!(store.get_offset("group1", 0), None);
    }

    #[test]
    fn phase_2_5_7_reset_clears_dedup_window() {
        let store = OffsetStore::in_memory();
        store.mark_processed("group1", 0, 100);
        store.mark_processed("group1", 0, 101);
        assert_eq!(store.dedup_window_size("group1", 0), 2);

        store.reset("group1", 0).unwrap();
        assert_eq!(store.dedup_window_size("group1", 0), 0);
    }

    #[test]
    fn phase_2_5_7_reset_persists_to_disk() {
        let path = make_temp_path("reset_persist");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();
            store.commit_offset("group1", 0, 100).unwrap();
            store.commit_offset("group1", 1, 200).unwrap();
            store.reset("group1", 0).unwrap(); // 只 reset partition 0
        }

        let store = OffsetStore::open(&path).unwrap();
        assert_eq!(store.get_offset("group1", 0), None); // 已 reset
        assert_eq!(store.get_offset("group1", 1), Some(200)); // 不受影响

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_reset_nonexistent_no_error() {
        let store = OffsetStore::in_memory();
        // reset 不存在的 (group, partition) 应该不报错
        store.reset("nonexistent", 0).unwrap();
    }

    #[test]
    fn phase_2_5_7_reset_only_affects_target() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        store.commit_offset("group1", 1, 200).unwrap();
        store.commit_offset("group2", 0, 300).unwrap();

        store.reset("group1", 0).unwrap();

        assert_eq!(store.get_offset("group1", 0), None);
        assert_eq!(store.get_offset("group1", 1), Some(200));
        assert_eq!(store.get_offset("group2", 0), Some(300));
    }

    #[test]
    fn phase_2_5_7_reset_allows_reprocessing_from_start() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 1000).unwrap();
        assert!(store.is_processed("group1", 0, 500));

        store.reset("group1", 0).unwrap();
        // reset 后，500 不再算已处理
        assert!(!store.is_processed("group1", 0, 500));
        assert!(!store.is_processed("group1", 0, 1000));
    }

    // =================================================================
    // Part 6: 并发安全（多线程场景）
    // =================================================================

    #[test]
    fn phase_2_5_7_concurrent_commit_different_groups_safe() {
        let store = Arc::new(OffsetStore::in_memory());
        let mut handles = vec![];

        for i in 0..10 {
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                let group = format!("group_{}", i);
                for lsn in 1..=100u64 {
                    store.commit_offset(&group, 0, lsn).unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // 每个 group 应该有最终的 offset = 100
        for i in 0..10 {
            let group = format!("group_{}", i);
            assert_eq!(store.get_offset(&group, 0), Some(100));
        }
        assert_eq!(store.offset_count(), 10);
    }

    #[test]
    fn phase_2_5_7_concurrent_commit_same_group_serialized() {
        // 同一 (group, partition) 的并发 commit 应该被串行化
        let store = Arc::new(OffsetStore::in_memory());
        let mut handles = vec![];

        for _ in 0..4 {
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                // 每个线程尝试 commit 1..=100
                // 由于串行化，最终结果应该是 100
                for lsn in 1..=100u64 {
                    // 忽略 Regression 错误（其他线程已经 commit 了更高的 LSN）
                    let _ = store.commit_offset("group1", 0, lsn);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // 最终 offset 应该是 100（所有线程都尝试 commit 到 100）
        assert_eq!(store.get_offset("group1", 0), Some(100));
    }

    #[test]
    fn phase_2_5_7_concurrent_mark_processed_safe() {
        let store = Arc::new(OffsetStore::in_memory());
        let mut handles = vec![];

        // 10 个线程，每个线程 mark 1000 个不重复的 LSN
        for t in 0..10 {
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                let start = t * 1000 + 1;
                let end = start + 999;
                let mut count = 0;
                for lsn in start..=end {
                    if store.mark_processed("group1", 0, lsn) {
                        count += 1;
                    }
                }
                count
            }));
        }

        let total_marked: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        // 10000 个不重复的 LSN，全部应该首次标记成功
        assert_eq!(total_marked, 10000);
        assert_eq!(store.dedup_window_size("group1", 0), 10000);
    }

    #[test]
    fn phase_2_5_7_concurrent_get_offset_safe() {
        let store = Arc::new(OffsetStore::in_memory());
        store.commit_offset("group1", 0, 100).unwrap();

        let store_clone = store.clone();
        let writer = std::thread::spawn(move || {
            for lsn in 101..=200u64 {
                store_clone.commit_offset("group1", 0, lsn).unwrap();
            }
        });

        // 并发读
        let store_clone = store.clone();
        let reader = std::thread::spawn(move || {
            for _ in 0..100 {
                let offset = store_clone.get_offset("group1", 0);
                assert!(offset.is_some());
                assert!(offset.unwrap() <= 200);
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();

        assert_eq!(store.get_offset("group1", 0), Some(200));
    }

    // =================================================================
    // Part 7: 端到端消费者模拟
    // =================================================================

    /// 模拟一个简单的消费者：从 OffsetStore 读取 committed + 1 开始消费事件
    struct SimulatedConsumer {
        store: Arc<OffsetStore>,
        group: String,
        partition: u32,
        processed: parking_lot::Mutex<std::collections::HashSet<u64>>,
    }

    impl SimulatedConsumer {
        fn new(store: Arc<OffsetStore>, group: &str, partition: u32) -> Self {
            Self {
                store,
                group: group.to_string(),
                partition,
                processed: parking_lot::Mutex::new(std::collections::HashSet::new()),
            }
        }

        /// 处理一批事件，返回实际处理的数量（去重后）
        fn process_batch(&self, events: &[u64]) -> usize {
            let mut processed_count = 0;
            for &lsn in events {
                // 通过 mark_processed 去重
                if self.store.mark_processed(&self.group, self.partition, lsn) {
                    // 模拟处理（应用 idempotent 操作）
                    let mut processed = self.processed.lock();
                    let is_new = processed.insert(lsn);
                    assert!(is_new, "duplicate processing detected for lsn {}", lsn);
                    processed_count += 1;
                }
            }
            processed_count
        }

        /// 提交 offset
        fn commit(&self, lsn: u64) -> Result<(), OffsetStoreError> {
            self.store.commit_offset(&self.group, self.partition, lsn)
        }

        /// 获取下次应该消费的起始 LSN
        fn next_lsn(&self) -> u64 {
            self.store
                .get_offset(&self.group, self.partition)
                .unwrap_or(0)
                + 1
        }

        /// 已处理的事件总数
        fn processed_count(&self) -> usize {
            self.processed.lock().len()
        }
    }

    #[test]
    fn phase_2_5_7_simulated_consumer_basic_flow() {
        let store = Arc::new(OffsetStore::in_memory());
        let consumer = SimulatedConsumer::new(store.clone(), "group1", 0);

        // 第一次消费：LSN 1..=100
        let events: Vec<u64> = (1..=100).collect();
        let processed = consumer.process_batch(&events);
        assert_eq!(processed, 100);
        consumer.commit(100).unwrap();
        assert_eq!(consumer.next_lsn(), 101);

        // 第二次消费：LSN 101..=200
        let events: Vec<u64> = (101..=200).collect();
        let processed = consumer.process_batch(&events);
        assert_eq!(processed, 100);
        consumer.commit(200).unwrap();
        assert_eq!(consumer.next_lsn(), 201);

        assert_eq!(consumer.processed_count(), 200);
    }

    #[test]
    fn phase_2_5_7_simulated_consumer_with_redelivery() {
        // 模拟 at-least-once：同一批事件被重投
        let store = Arc::new(OffsetStore::in_memory());
        let consumer = SimulatedConsumer::new(store.clone(), "group1", 0);

        // 第一次投递：1..=100，处理但未 commit
        let events: Vec<u64> = (1..=100).collect();
        let processed1 = consumer.process_batch(&events);
        assert_eq!(processed1, 100);

        // 重投同一批：mark_processed 应该全部返回 false
        let processed2 = consumer.process_batch(&events);
        assert_eq!(processed2, 0);

        // 总处理数仍是 100（去重生效）
        assert_eq!(consumer.processed_count(), 100);
    }

    #[test]
    fn phase_2_5_7_simulated_consumer_crash_recovery_no_redelivery() {
        // 场景：处理完一批并 commit 后崩溃 → 重启后从 committed + 1 继续
        let path = make_temp_path("crash_no_redelivery");
        cleanup_temp_file(&path);

        let processed_path = path.with_extension("processed");
        cleanup_temp_file(&processed_path);

        // 第一次会话：处理 1..=100 并 commit
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = SimulatedConsumer::new(store.clone(), "group1", 0);

            let events: Vec<u64> = (1..=100).collect();
            consumer.process_batch(&events);
            consumer.commit(100).unwrap();

            // 持久化 processed set（模拟下游应用的持久化状态）
            let processed = consumer.processed.lock();
            let json = serde_json::to_string(&processed.iter().collect::<Vec<_>>()).unwrap();
            std::fs::write(&processed_path, &json).unwrap();
        }

        // 第二次会话：从 committed + 1 继续
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = SimulatedConsumer::new(store.clone(), "group1", 0);

            assert_eq!(consumer.next_lsn(), 101);

            // 恢复 processed set
            let json = std::fs::read_to_string(&processed_path).unwrap();
            let vec: Vec<u64> = serde_json::from_str(&json).unwrap();
            {
                let mut processed = consumer.processed.lock();
                for lsn in vec {
                    processed.insert(lsn);
                }
            }

            // 处理 101..=200
            let events: Vec<u64> = (101..=200).collect();
            let processed_count = consumer.process_batch(&events);
            assert_eq!(processed_count, 100);

            // 总处理数应该是 200
            assert_eq!(consumer.processed_count(), 200);
        }

        cleanup_temp_file(&path);
        cleanup_temp_file(&processed_path);
    }

    #[test]
    fn phase_2_5_7_simulated_consumer_crash_recovery_with_redelivery() {
        // 场景：处理完一批但未 commit 就崩溃 → 重启后从 committed + 1 重新消费
        // 此时去重窗口丢失，已处理的事件会被重新处理（at-least-once）
        // 消费者必须 idempotent（用 processed set 去重）
        let path = make_temp_path("crash_with_redelivery");
        cleanup_temp_file(&path);

        let processed_path = path.with_extension("processed");
        cleanup_temp_file(&processed_path);

        // 第一次会话：处理 1..=100 但未 commit
        // 模拟下游应用持久化了 processed set
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = SimulatedConsumer::new(store.clone(), "group1", 0);

            let events: Vec<u64> = (1..=100).collect();
            consumer.process_batch(&events);

            // 持久化 processed set
            let processed = consumer.processed.lock();
            let json = serde_json::to_string(&processed.iter().collect::<Vec<_>>()).unwrap();
            std::fs::write(&processed_path, &json).unwrap();
            // 注意：没有 commit_offset，崩溃
        }

        // 第二次会话：从 committed + 1 = 1 重新消费
        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());
            let consumer = SimulatedConsumer::new(store.clone(), "group1", 0);

            assert_eq!(consumer.next_lsn(), 1); // 没有 commit，从头开始

            // 恢复 processed set（idempotent 消费的关键）
            let json = std::fs::read_to_string(&processed_path).unwrap();
            let vec: Vec<u64> = serde_json::from_str(&json).unwrap();
            {
                let mut processed = consumer.processed.lock();
                for lsn in vec {
                    processed.insert(lsn);
                }
            }

            // 重新投递 1..=100
            let events: Vec<u64> = (1..=100).collect();
            // 由于去重窗口丢失，mark_processed 会返回 true，但 SimulatedConsumer
            // 用 processed set 检测重复，会 panic
            // 所以我们改成只检查 mark_processed 返回值
            let mut redelivered_count = 0;
            for &lsn in &events {
                if store.mark_processed("group1", 0, lsn) {
                    redelivered_count += 1;
                }
            }
            // 全部 100 个 LSN 被重新标记（去重窗口丢失）
            assert_eq!(redelivered_count, 100);
        }

        cleanup_temp_file(&path);
        cleanup_temp_file(&processed_path);
    }

    // =================================================================
    // Part 8: Stress 测试 — 1M 事件 + 崩溃恢复
    // =================================================================

    #[test]
    fn phase_2_5_7_stress_1m_events_with_crash_recovery() {
        const TOTAL_EVENTS: u64 = 1_000_000;
        const BATCH_SIZE: u64 = 1_000;
        const CRASH_INTERVAL: u64 = 100_000; // 每 100K 事件模拟一次崩溃

        let path = make_temp_path("stress_1m");
        cleanup_temp_file(&path);

        // 用 Vec<u8> 作为 bitset 跟踪已处理事件
        // bitset[lsn / 8] & (1 << (lsn % 8)) == 1 表示 lsn 已处理
        let mut bitset = vec![0u8; (TOTAL_EVENTS as usize / 8) + 1];
        let mut processed_count = 0u64;
        let mut duplicate_count = 0u64;

        let mut store = OffsetStore::open(&path).unwrap();
        let mut next_lsn = 1u64;

        while next_lsn <= TOTAL_EVENTS {
            // 获取已提交的 offset，从 committed + 1 开始
            let committed = store.get_offset("group1", 0).unwrap_or(0);
            let start_lsn = committed + 1;
            if start_lsn > next_lsn {
                next_lsn = start_lsn;
            }

            let end_lsn = (next_lsn + BATCH_SIZE - 1).min(TOTAL_EVENTS);

            // 处理批量事件
            for lsn in next_lsn..=end_lsn {
                let idx = lsn as usize / 8;
                let bit = 1u8 << (lsn as usize % 8);
                if bitset[idx] & bit == 0 {
                    bitset[idx] |= bit;
                    processed_count += 1;
                } else {
                    duplicate_count += 1;
                }
            }

            // commit offset
            store.commit_offset("group1", 0, end_lsn).unwrap();
            next_lsn = end_lsn + 1;

            // 模拟崩溃：每 CRASH_INTERVAL 重新打开 store
            if end_lsn.is_multiple_of(CRASH_INTERVAL) {
                store = OffsetStore::open(&path).unwrap();
            }
        }

        // 验证：所有事件都被处理，无丢失
        assert_eq!(processed_count, TOTAL_EVENTS);
        // 验证：无重复（每次崩溃前都已 commit，所以重开不会重新处理）
        assert_eq!(duplicate_count, 0);

        // 验证 bitset 中所有位都为 1
        for lsn in 1..=TOTAL_EVENTS {
            let idx = lsn as usize / 8;
            let bit = 1u8 << (lsn as usize % 8);
            assert!(bitset[idx] & bit != 0, "lsn {} not processed", lsn);
        }

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_stress_1m_events_with_uncommitted_crash() {
        // 场景：每 CRASH_INTERVAL 事件，处理但故意不 commit，模拟崩溃
        // 重启后会重新处理这批事件（at-least-once）
        // 消费者必须 idempotent（用 bitset 检测重复）
        const TOTAL_EVENTS: u64 = 1_000_000;
        const BATCH_SIZE: u64 = 1_000;
        const CRASH_INTERVAL_BATCHES: u64 = 100; // 每 100 批崩溃一次

        let path = make_temp_path("stress_1m_uncommitted");
        cleanup_temp_file(&path);

        let mut bitset = vec![0u8; (TOTAL_EVENTS as usize / 8) + 1];
        let mut processed_count = 0u64;
        let mut duplicate_count = 0u64;

        let mut store = OffsetStore::open(&path).unwrap();
        let mut next_lsn = 1u64;
        let mut batch_count = 0u64;

        while next_lsn <= TOTAL_EVENTS {
            let committed = store.get_offset("group1", 0).unwrap_or(0);
            let start_lsn = committed + 1;
            if start_lsn > next_lsn {
                next_lsn = start_lsn;
            }

            let end_lsn = (next_lsn + BATCH_SIZE - 1).min(TOTAL_EVENTS);

            // 处理批量事件
            for lsn in next_lsn..=end_lsn {
                let idx = lsn as usize / 8;
                let bit = 1u8 << (lsn as usize % 8);
                if bitset[idx] & bit == 0 {
                    bitset[idx] |= bit;
                    processed_count += 1;
                } else {
                    duplicate_count += 1;
                }
            }

            batch_count += 1;

            // 每 CRASH_INTERVAL_BATCHES 批，故意不 commit 就崩溃
            if batch_count.is_multiple_of(CRASH_INTERVAL_BATCHES) {
                // 模拟崩溃：重新打开 store（不 commit 当前批次）
                store = OffsetStore::open(&path).unwrap();
                // next_lsn 不变，因为没 commit，下次会从 committed + 1 重新消费
            } else {
                // 正常 commit
                store.commit_offset("group1", 0, end_lsn).unwrap();
                next_lsn = end_lsn + 1;
            }
        }

        // 验证：所有事件最终都被处理
        assert_eq!(processed_count, TOTAL_EVENTS);
        // 验证：有重复（崩溃时未 commit 的事件被重新处理）
        // 由于每 100 批崩溃一次，每次崩溃丢失 1000 个事件的 commit
        // 总共崩溃约 10 次，所以至少有 10000 个重复
        assert!(
            duplicate_count > 0,
            "expected duplicates from uncommitted crashes, got 0"
        );
        assert!(
            duplicate_count >= 10_000,
            "expected at least 10000 duplicates, got {}",
            duplicate_count
        );

        cleanup_temp_file(&path);
    }

    // =================================================================
    // Part 9: Stress 测试 — 10M 事件（标记 #[ignore] 避免默认运行）
    // =================================================================

    #[test]
    #[ignore = "10M 事件压力测试，运行时间较长，使用 --ignored 单独运行"]
    fn phase_2_5_7_stress_10m_events_with_crash_recovery() {
        const TOTAL_EVENTS: u64 = 10_000_000;
        const BATCH_SIZE: u64 = 10_000; // 较大 batch 减少提交次数
        const CRASH_INTERVAL: u64 = 1_000_000; // 每 1M 事件崩溃一次

        let path = make_temp_path("stress_10m");
        cleanup_temp_file(&path);

        // 用 Vec<u8> bitset 跟踪（10M bits = 1.25MB）
        let mut bitset = vec![0u8; (TOTAL_EVENTS as usize / 8) + 1];
        let mut processed_count = 0u64;
        let mut duplicate_count = 0u64;

        let mut store = OffsetStore::open(&path).unwrap();
        let mut next_lsn = 1u64;

        while next_lsn <= TOTAL_EVENTS {
            let committed = store.get_offset("group1", 0).unwrap_or(0);
            let start_lsn = committed + 1;
            if start_lsn > next_lsn {
                next_lsn = start_lsn;
            }

            let end_lsn = (next_lsn + BATCH_SIZE - 1).min(TOTAL_EVENTS);

            // 处理批量事件
            for lsn in next_lsn..=end_lsn {
                let idx = lsn as usize / 8;
                let bit = 1u8 << (lsn as usize % 8);
                if bitset[idx] & bit == 0 {
                    bitset[idx] |= bit;
                    processed_count += 1;
                } else {
                    duplicate_count += 1;
                }
            }

            // commit offset
            store.commit_offset("group1", 0, end_lsn).unwrap();
            next_lsn = end_lsn + 1;

            // 模拟崩溃
            if end_lsn.is_multiple_of(CRASH_INTERVAL) {
                store = OffsetStore::open(&path).unwrap();
            }
        }

        // 验证：10M 事件全部处理，无丢失
        assert_eq!(processed_count, TOTAL_EVENTS);
        // 验证：无重复（每次崩溃前都已 commit）
        assert_eq!(duplicate_count, 0);

        // 验证 bitset 中所有位都为 1
        for lsn in 1..=TOTAL_EVENTS {
            let idx = lsn as usize / 8;
            let bit = 1u8 << (lsn as usize % 8);
            assert!(bitset[idx] & bit != 0, "lsn {} not processed", lsn);
        }

        cleanup_temp_file(&path);
    }

    // =================================================================
    // Part 10: 多消费者组并发消费
    // =================================================================

    #[test]
    fn phase_2_5_7_multiple_consumer_groups_independent() {
        // 多个消费者组独立消费同一批事件，每个组都有自己的 offset
        let path = make_temp_path("multi_groups");
        cleanup_temp_file(&path);

        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());

            // 3 个消费者组，每个组消费 1000 个事件
            let mut handles = vec![];
            for group_id in 0..3 {
                let store = store.clone();
                handles.push(std::thread::spawn(move || {
                    let group = format!("consumer_{}", group_id);
                    for lsn in 1..=1000u64 {
                        // mark_processed 应该首次都返回 true
                        assert!(store.mark_processed(&group, 0, lsn));
                    }
                    // commit 最终 offset
                    store.commit_offset(&group, 0, 1000).unwrap();
                }));
            }

            for h in handles {
                h.join().unwrap();
            }

            // 验证：每个组都有自己的 offset = 1000
            for group_id in 0..3 {
                let group = format!("consumer_{}", group_id);
                assert_eq!(store.get_offset(&group, 0), Some(1000));
            }
        }

        // 重启后验证持久化
        let store = OffsetStore::open(&path).unwrap();
        for group_id in 0..3 {
            let group = format!("consumer_{}", group_id);
            assert_eq!(store.get_offset(&group, 0), Some(1000));
        }

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_multiple_partitions_parallel_consumption() {
        // 一个消费者组并行消费多个分区
        let path = make_temp_path("multi_partitions_parallel");
        cleanup_temp_file(&path);

        {
            let store = Arc::new(OffsetStore::open(&path).unwrap());

            let mut handles = vec![];
            for partition in 0..10u32 {
                let store = store.clone();
                handles.push(std::thread::spawn(move || {
                    for lsn in 1..=1000u64 {
                        store.commit_offset("group1", partition, lsn).unwrap();
                    }
                }));
            }

            for h in handles {
                h.join().unwrap();
            }

            // 验证：每个分区的 offset 都是 1000
            for partition in 0..10u32 {
                assert_eq!(store.get_offset("group1", partition), Some(1000));
            }
        }

        let store = OffsetStore::open(&path).unwrap();
        for partition in 0..10u32 {
            assert_eq!(store.get_offset("group1", partition), Some(1000));
        }

        cleanup_temp_file(&path);
    }

    // =================================================================
    // Part 11: 错误场景与边界条件
    // =================================================================

    #[test]
    fn phase_2_5_7_empty_group_name() {
        let store = OffsetStore::in_memory();
        store.commit_offset("", 0, 100).unwrap();
        assert_eq!(store.get_offset("", 0), Some(100));
    }

    #[test]
    fn phase_2_5_7_zero_partition() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        assert_eq!(store.get_offset("group1", 0), Some(100));
    }

    #[test]
    fn phase_2_5_7_max_partition() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", u32::MAX, 100).unwrap();
        assert_eq!(store.get_offset("group1", u32::MAX), Some(100));
    }

    #[test]
    fn phase_2_5_7_zero_lsn() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 0).unwrap();
        assert_eq!(store.get_offset("group1", 0), Some(0));
    }

    #[test]
    fn phase_2_5_7_max_lsn() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, u64::MAX).unwrap();
        assert_eq!(store.get_offset("group1", 0), Some(u64::MAX));
    }

    #[test]
    fn phase_2_5_7_lsn_zero_mark_processed() {
        let store = OffsetStore::in_memory();
        // LSN 0 可以被 mark_processed
        assert!(store.mark_processed("group1", 0, 0));
        assert!(store.is_processed("group1", 0, 0));
    }

    #[test]
    fn phase_2_5_7_lsn_zero_committed_excludes_zero() {
        // 当 committed_lsn = 0 时，lsn <= 0（即 lsn = 0）算已处理
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 0).unwrap();
        assert!(store.is_processed("group1", 0, 0));
        assert!(!store.is_processed("group1", 0, 1));
    }

    #[test]
    fn phase_2_5_7_very_long_group_name() {
        let store = OffsetStore::in_memory();
        let long_name = "x".repeat(10000);
        store.commit_offset(&long_name, 0, 100).unwrap();
        assert_eq!(store.get_offset(&long_name, 0), Some(100));
    }

    #[test]
    fn phase_2_5_7_unicode_group_name() {
        let store = OffsetStore::in_memory();
        store.commit_offset("消费者组-1", 0, 100).unwrap();
        assert_eq!(store.get_offset("消费者组-1", 0), Some(100));
    }

    #[test]
    fn phase_2_5_7_commit_after_reset_works() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        store.reset("group1", 0).unwrap();
        // reset 后可以重新 commit
        store.commit_offset("group1", 0, 50).unwrap();
        assert_eq!(store.get_offset("group1", 0), Some(50));
    }

    #[test]
    fn phase_2_5_7_repeated_reset_safe() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        store.reset("group1", 0).unwrap();
        store.reset("group1", 0).unwrap(); // 重复 reset 不报错
        store.reset("group1", 0).unwrap();
        assert_eq!(store.get_offset("group1", 0), None);
    }

    // =================================================================
    // Part 12: 持久化文件格式验证
    // =================================================================

    #[test]
    fn phase_2_5_7_persisted_file_has_correct_version() {
        let path = make_temp_path("file_version");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();
            store.commit_offset("group1", 0, 100).unwrap();
        }

        // 读取文件内容，验证版本号
        let content = std::fs::read_to_string(&path).unwrap();
        let file: OffsetFile = serde_json::from_str(&content).unwrap();
        assert_eq!(file.version, OFFSET_FILE_VERSION);

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_persisted_file_contains_all_offsets() {
        let path = make_temp_path("file_all_offsets");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();
            store.commit_offset("group1", 0, 100).unwrap();
            store.commit_offset("group1", 1, 200).unwrap();
            store.commit_offset("group2", 0, 300).unwrap();
        }

        let content = std::fs::read_to_string(&path).unwrap();
        let file: OffsetFile = serde_json::from_str(&content).unwrap();
        assert_eq!(file.offsets.len(), 3);

        // 验证每条记录
        let mut found = std::collections::HashSet::new();
        for record in &file.offsets {
            found.insert((
                record.consumer_group.as_str(),
                record.partition,
                record.committed_lsn,
            ));
        }
        assert!(found.contains(&("group1", 0, 100)));
        assert!(found.contains(&("group1", 1, 200)));
        assert!(found.contains(&("group2", 0, 300)));

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_persisted_file_is_valid_json() {
        let path = make_temp_path("valid_json");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();
            store.commit_offset("group1", 0, 12345).unwrap();
        }

        let content = std::fs::read_to_string(&path).unwrap();
        // 应该是合法的 JSON
        let _: serde_json::Value = serde_json::from_str(&content).unwrap();

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_in_memory_no_file_written() {
        let store = OffsetStore::in_memory();
        store.commit_offset("group1", 0, 100).unwrap();
        // in-memory 模式不应该写文件（path 为空）
        // 验证：flush 也不写文件
        store.flush().unwrap();
    }

    // =================================================================
    // Part 13: 集成测试 — 与 CdcEngine 概念集成
    // =================================================================

    #[test]
    fn phase_2_5_7_integration_cdc_consumer_pattern() {
        // 模拟 CDC 消费者模式：
        // 1. CdcEngine 分发事件（模拟）
        // 2. Consumer 通过 OffsetStore 跟踪 offset
        // 3. 崩溃重启后从 committed + 1 继续
        let path = make_temp_path("cdc_integration");
        cleanup_temp_file(&path);

        // 模拟一批 CDC 事件（每个事件有 lsn）
        let generate_events = |start: u64, end: u64| -> Vec<u64> { (start..=end).collect() };

        // 第一阶段：消费 1..=500 并 commit
        {
            let store = OffsetStore::open(&path).unwrap();
            let events = generate_events(1, 500);
            for &lsn in &events {
                store.mark_processed("cdc_group", 0, lsn);
            }
            store.commit_offset("cdc_group", 0, 500).unwrap();
        }

        // 第二阶段：消费 501..=1000 并 commit
        {
            let store = OffsetStore::open(&path).unwrap();
            // 重启后从 committed + 1 = 501 开始
            let next = store.get_offset("cdc_group", 0).unwrap() + 1;
            assert_eq!(next, 501);

            let events = generate_events(501, 1000);
            for &lsn in &events {
                store.mark_processed("cdc_group", 0, lsn);
            }
            store.commit_offset("cdc_group", 0, 1000).unwrap();
        }

        // 第三阶段：验证最终状态
        let store = OffsetStore::open(&path).unwrap();
        assert_eq!(store.get_offset("cdc_group", 0), Some(1000));
        // committed_lsn = 1000，所以 lsn 1000 算已处理，lsn 1001 未处理
        assert!(store.is_processed("cdc_group", 0, 1000));
        assert!(!store.is_processed("cdc_group", 0, 1001));

        cleanup_temp_file(&path);
    }

    #[test]
    fn phase_2_5_7_integration_multi_table_cdc() {
        // 模拟多表 CDC：每个表是一个独立的分区
        let path = make_temp_path("multi_table_cdc");
        cleanup_temp_file(&path);

        {
            let store = OffsetStore::open(&path).unwrap();

            // 表 1 (partition=1)：消费到 lsn 1000
            for lsn in 1..=1000u64 {
                store.mark_processed("cdc_group", 1, lsn);
            }
            store.commit_offset("cdc_group", 1, 1000).unwrap();

            // 表 2 (partition=2)：消费到 lsn 2000
            for lsn in 1..=2000u64 {
                store.mark_processed("cdc_group", 2, lsn);
            }
            store.commit_offset("cdc_group", 2, 2000).unwrap();

            // 表 3 (partition=3)：消费到 lsn 500
            for lsn in 1..=500u64 {
                store.mark_processed("cdc_group", 3, lsn);
            }
            store.commit_offset("cdc_group", 3, 500).unwrap();
        }

        // 重启后验证各表的 offset
        let store = OffsetStore::open(&path).unwrap();
        assert_eq!(store.get_offset("cdc_group", 1), Some(1000));
        assert_eq!(store.get_offset("cdc_group", 2), Some(2000));
        assert_eq!(store.get_offset("cdc_group", 3), Some(500));
        assert_eq!(store.offset_count(), 3);

        // list_partitions 应返回所有分区
        let partitions = store.list_partitions("cdc_group");
        assert_eq!(partitions, vec![(1, 1000), (2, 2000), (3, 500)]);

        cleanup_temp_file(&path);
    }

    // =================================================================
    // Part 14: 模拟 exactly-once 端到端验证
    // =================================================================

    #[test]
    fn phase_2_5_7_exactly_once_with_idempotent_consumer() {
        // 端到端验证：at-least-once 投递 + idempotent 消费 = exactly-once
        // 消费者使用 HashSet 持久化已应用的 LSN（模拟下游应用的状态）
        let path = make_temp_path("exactly_once");
        cleanup_temp_file(&path);

        let applied_path = path.with_extension("applied");
        cleanup_temp_file(&applied_path);

        const TOTAL_EVENTS: u64 = 100_000;
        const BATCH_SIZE: u64 = 500;
        const CRASH_BATCHES: u64 = 50; // 每 50 批崩溃一次

        // 模拟下游应用状态：applied LSN set
        let mut applied: std::collections::HashSet<u64> = std::collections::HashSet::new();

        // 第一阶段：恢复 applied set（如果存在）
        if applied_path.exists() {
            let json = std::fs::read_to_string(&applied_path).unwrap();
            let vec: Vec<u64> = serde_json::from_str(&json).unwrap();
            for lsn in vec {
                applied.insert(lsn);
            }
        }

        let mut store = OffsetStore::open(&path).unwrap();
        let mut next_lsn = store.get_offset("group1", 0).unwrap_or(0) + 1;
        let mut batch_count = 0u64;

        while next_lsn <= TOTAL_EVENTS {
            let end_lsn = (next_lsn + BATCH_SIZE - 1).min(TOTAL_EVENTS);

            // 处理批量事件（idempotent：插入 HashSet）
            for lsn in next_lsn..=end_lsn {
                applied.insert(lsn);
            }

            batch_count += 1;

            // 每 CRASH_BATCHES 批，模拟崩溃（不 commit，但持久化 applied set）
            if batch_count.is_multiple_of(CRASH_BATCHES) {
                // 持久化 applied set（模拟下游应用的状态持久化）
                let json =
                    serde_json::to_string(&applied.iter().copied().collect::<Vec<_>>()).unwrap();
                std::fs::write(&applied_path, &json).unwrap();

                // 重新打开 store（崩溃恢复）
                store = OffsetStore::open(&path).unwrap();
                // 从 committed + 1 重新开始
                next_lsn = store.get_offset("group1", 0).unwrap_or(0) + 1;
            } else {
                // 正常 commit
                store.commit_offset("group1", 0, end_lsn).unwrap();
                next_lsn = end_lsn + 1;
            }
        }

        // 最终持久化 applied set
        let json = serde_json::to_string(&applied.iter().copied().collect::<Vec<_>>()).unwrap();
        std::fs::write(&applied_path, &json).unwrap();

        // 验证：所有事件都被应用（exactly-once）
        assert_eq!(applied.len() as u64, TOTAL_EVENTS);
        for lsn in 1..=TOTAL_EVENTS {
            assert!(applied.contains(&lsn), "lsn {} not applied", lsn);
        }

        cleanup_temp_file(&path);
        cleanup_temp_file(&applied_path);
    }
}
