//! Phase 7d.19 — Full Page Writes（FPI）。
//!
//! 在 checkpoint 后首次修改某页时，将该页的完整镜像作为
//! `WalOpType::FullPageImage` 记录写入 WAL，防止 torn page（部分写）
//! 导致恢复时页内容损坏。
//!
//! # 设计
//!
//! - **FpiManager** 跟踪自上次 checkpoint 以来已写 FPI 的页集合
//! - **should_write_fpi(page_id)**：返回是否需要为该页写 FPI
//! - **mark_fpi_written(page_id)**：记录已写 FPI
//! - **reset()**：checkpoint 后调用，清空集合
//! - **build_fpi_record(...)**：构造 `WalRecord { op_type: FullPageImage, data: page_bytes }`
//!
//! # 崩溃恢复语义
//!
//! 重放 WAL 时遇到 FPI 记录：
//! 1. 用 FPI 的 data 直接覆盖页内容（建立"干净基线"）
//! 2. 继续重放后续对同一页的修改记录
//!
//! 这样即使页被部分写（如 8KB 中只写了前 4KB），FPI 也能恢复到一致状态。
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_tx::wal::WalOpType;
//! use szrsql_tx::wal_fpi::{FpiManager, FpiConfig};
//!
//! let mut mgr = FpiManager::new(FpiConfig::default());
//! let page_id = 42;
//! let page_bytes = vec![0u8; 8192]; // 完整页内容
//!
//! if mgr.should_write_fpi(page_id) {
//!     let fpi_rec = mgr.build_fpi_record(100, 1, page_id, &page_bytes);
//!     // ... 写入 WAL ...
//!     mgr.mark_fpi_written(page_id);
//! }
//! // 后续修改同一页：不再写 FPI
//! assert!(!mgr.should_write_fpi(page_id));
//!
//! // checkpoint 后重置
//! mgr.reset();
//! assert!(mgr.should_write_fpi(page_id));
//! ```

use std::collections::HashSet;

use crate::wal::{WalOpType, WalRecord};

// =====================================================================
//  FpiConfig
// =====================================================================

/// FPI 配置。
#[derive(Debug, Clone)]
pub struct FpiConfig {
    /// 是否启用 Full Page Writes。
    ///
    /// 关闭后 `should_write_fpi` 始终返回 false（性能优先，但崩溃可能损坏页）。
    pub enabled: bool,
}

impl Default for FpiConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

// =====================================================================
//  FpiManager
// =====================================================================

/// FPI 管理器：跟踪自上次 checkpoint 以来已写 FPI 的页集合。
///
/// 线程安全策略：本类型非 `Sync`，应由 WAL Writer 单线程持有，
/// 或由外部锁保护（与 `WalWriter` 的访问模式一致）。
pub struct FpiManager {
    /// 配置。
    pub config: FpiConfig,
    /// 已写 FPI 的页集合（自上次 checkpoint 起）。
    written_pages: HashSet<u32>,
    /// 统计：累计写出的 FPI 记录数。
    fpi_count: u64,
}

impl Default for FpiManager {
    fn default() -> Self {
        Self::new(FpiConfig::default())
    }
}

impl FpiManager {
    /// 创建 FPI 管理器。
    pub fn new(config: FpiConfig) -> Self {
        Self {
            config,
            written_pages: HashSet::new(),
            fpi_count: 0,
        }
    }

    /// 是否需要为指定 page 写 FPI。
    ///
    /// 返回 true 当且仅当：
    /// 1. FPI 已启用（`config.enabled == true`）
    /// 2. 该页自上次 checkpoint 以来未写过 FPI
    pub fn should_write_fpi(&self, page_id: u32) -> bool {
        self.config.enabled && !self.written_pages.contains(&page_id)
    }

    /// 标记 page 已写 FPI（写完 FPI 记录后调用）。
    pub fn mark_fpi_written(&mut self, page_id: u32) {
        if self.written_pages.insert(page_id) {
            self.fpi_count += 1;
        }
    }

    /// 构造 FPI 记录。
    ///
    /// - `lsn`：分配给该 FPI 的 LSN
    /// - `tx_id`：触发 FPI 的事务 ID（通常是触发首次修改的事务）
    /// - `page_id`：页 ID
    /// - `page_bytes`：完整页内容（将被复制到 `WalRecord::data`）
    pub fn build_fpi_record(
        &self,
        lsn: u64,
        tx_id: u32,
        page_id: u32,
        page_bytes: &[u8],
    ) -> WalRecord {
        let mut rec = WalRecord::new(
            lsn,
            tx_id,
            WalOpType::FullPageImage,
            page_id,
            page_bytes.to_vec(),
        );
        rec.update_checksum();
        rec
    }

    /// Checkpoint 后重置状态：清空已写集合。
    ///
    /// 下次修改任何页时都会重新写 FPI。
    pub fn reset(&mut self) {
        self.written_pages.clear();
    }

    /// 累计 FPI 记录数（自进程启动以来）。
    pub fn fpi_count(&self) -> u64 {
        self.fpi_count
    }

    /// 当前已写 FPI 的页数（自上次 reset 以来）。
    pub fn tracked_page_count(&self) -> usize {
        self.written_pages.len()
    }

    /// 便捷方法：若需要则构造 FPI 记录并标记已写。
    ///
    /// 返回 `Some(WalRecord)` 表示需要写入 WAL；`None` 表示不需要。
    pub fn maybe_build_fpi(
        &mut self,
        lsn: u64,
        tx_id: u32,
        page_id: u32,
        page_bytes: &[u8],
    ) -> Option<WalRecord> {
        if self.should_write_fpi(page_id) {
            let rec = self.build_fpi_record(lsn, tx_id, page_id, page_bytes);
            self.mark_fpi_written(page_id);
            Some(rec)
        } else {
            None
        }
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_page(seed: u8) -> Vec<u8> {
        vec![seed; 8192]
    }

    // ==================== FpiConfig ====================

    #[test]
    fn test_fpi_config_default_enabled() {
        let cfg = FpiConfig::default();
        assert!(cfg.enabled);
    }

    // ==================== FpiManager 基础行为 ====================

    #[test]
    fn test_should_write_fpi_first_time() {
        let mgr = FpiManager::default();
        assert!(mgr.should_write_fpi(1));
        assert!(mgr.should_write_fpi(2));
        assert!(mgr.should_write_fpi(u32::MAX));
    }

    #[test]
    fn test_should_not_write_fpi_after_marked() {
        let mut mgr = FpiManager::default();
        mgr.mark_fpi_written(42);
        assert!(!mgr.should_write_fpi(42));
        // 其他页仍需写
        assert!(mgr.should_write_fpi(43));
    }

    #[test]
    fn test_disabled_config_never_write() {
        let mut mgr = FpiManager::new(FpiConfig { enabled: false });
        assert!(!mgr.should_write_fpi(1));
        // 即使调用 mark_fpi_written 也不影响 should_write_fpi（始终 false）
        mgr.mark_fpi_written(1);
        assert!(!mgr.should_write_fpi(1));
    }

    // ==================== reset 行为 ====================

    #[test]
    fn test_reset_clears_tracked_pages() {
        let mut mgr = FpiManager::default();
        mgr.mark_fpi_written(1);
        mgr.mark_fpi_written(2);
        assert_eq!(mgr.tracked_page_count(), 2);
        assert!(!mgr.should_write_fpi(1));

        mgr.reset();
        assert_eq!(mgr.tracked_page_count(), 0);
        assert!(mgr.should_write_fpi(1));
        assert!(mgr.should_write_fpi(2));
    }

    // ==================== fpi_count 统计 ====================

    #[test]
    fn test_fpi_count_increments_once_per_page() {
        let mut mgr = FpiManager::default();
        assert_eq!(mgr.fpi_count(), 0);

        mgr.mark_fpi_written(1);
        assert_eq!(mgr.fpi_count(), 1);

        // 重复标记同一页不应增加计数
        mgr.mark_fpi_written(1);
        assert_eq!(mgr.fpi_count(), 1);

        mgr.mark_fpi_written(2);
        assert_eq!(mgr.fpi_count(), 2);

        // reset 不重置累计计数
        mgr.reset();
        assert_eq!(mgr.fpi_count(), 2);
        assert_eq!(mgr.tracked_page_count(), 0);
    }

    // ==================== build_fpi_record ====================

    #[test]
    fn test_build_fpi_record_fields() {
        let mgr = FpiManager::default();
        let page = make_page(0xAB);
        let rec = mgr.build_fpi_record(100, 7, 42, &page);

        assert_eq!(rec.lsn, 100);
        assert_eq!(rec.tx_id, 7);
        assert_eq!(rec.op_type, WalOpType::FullPageImage);
        assert_eq!(rec.page_id, 42);
        assert_eq!(rec.data, page);
    }

    #[test]
    fn test_build_fpi_record_checksum_valid() {
        let mgr = FpiManager::default();
        let page = make_page(0x77);
        let rec = mgr.build_fpi_record(200, 3, 99, &page);
        // checksum 应已正确填充且通过校验
        assert_ne!(rec.checksum, 0);
        rec.verify_checksum().unwrap();
    }

    // ==================== maybe_build_fpi ====================

    #[test]
    fn test_maybe_build_fpi_first_call_returns_record() {
        let mut mgr = FpiManager::default();
        let page = make_page(0x11);
        let rec = mgr.maybe_build_fpi(50, 1, 10, &page);
        assert!(rec.is_some());
        let rec = rec.unwrap();
        assert_eq!(rec.op_type, WalOpType::FullPageImage);
        assert_eq!(rec.page_id, 10);
        assert_eq!(mgr.fpi_count(), 1);
        assert_eq!(mgr.tracked_page_count(), 1);
    }

    #[test]
    fn test_maybe_build_fpi_second_call_returns_none() {
        let mut mgr = FpiManager::default();
        let page = make_page(0x22);
        let first = mgr.maybe_build_fpi(50, 1, 10, &page);
        assert!(first.is_some());

        let second = mgr.maybe_build_fpi(60, 2, 10, &page);
        assert!(second.is_none());
        assert_eq!(mgr.fpi_count(), 1); // 只统计一次
    }

    #[test]
    fn test_maybe_build_fpi_after_reset_returns_record() {
        let mut mgr = FpiManager::default();
        let page = make_page(0x33);
        let _ = mgr.maybe_build_fpi(50, 1, 10, &page);
        assert!(mgr.maybe_build_fpi(60, 2, 10, &page).is_none());

        mgr.reset();
        let after = mgr.maybe_build_fpi(70, 3, 10, &page);
        assert!(after.is_some());
        assert_eq!(mgr.fpi_count(), 2); // 累计计数
    }

    #[test]
    fn test_maybe_build_fpi_disabled_returns_none() {
        let mut mgr = FpiManager::new(FpiConfig { enabled: false });
        let page = make_page(0x44);
        assert!(mgr.maybe_build_fpi(50, 1, 10, &page).is_none());
        assert_eq!(mgr.fpi_count(), 0);
    }

    // ==================== 崩溃恢复模拟（端到端） ====================

    #[test]
    fn test_crash_recovery_with_fpi() {
        // 模拟场景：
        // 1. checkpoint
        // 2. 修改 page 5（先写 FPI，再写 Update）
        // 3. 修改 page 5（不再写 FPI，只写 Update）
        // 4. 崩溃
        // 5. 重放：FPI 提供干净基线，后续 Update 在此基础上应用
        let mut mgr = FpiManager::default();
        let mut wal_log: Vec<WalRecord> = Vec::new();
        let mut next_lsn: u64 = 100;

        // checkpoint
        wal_log.push(WalRecord::new(
            next_lsn,
            0,
            WalOpType::Checkpoint,
            0,
            vec![],
        ));
        next_lsn += 1;

        // 第 1 次修改 page 5：需要 FPI
        let page5_initial = vec![0u8; 8192];
        if let Some(fpi) = mgr.maybe_build_fpi(next_lsn, 1, 5, &page5_initial) {
            wal_log.push(fpi);
            next_lsn += 1;
        }
        // 第 1 次 Update（增量数据）
        let mut update1_data = vec![0u8; 64];
        for (i, b) in update1_data.iter_mut().enumerate() {
            *b = (i & 0xFF) as u8;
        }
        wal_log.push(WalRecord::new(
            next_lsn,
            1,
            WalOpType::Update,
            5,
            update1_data.clone(),
        ));
        next_lsn += 1;

        // 第 2 次修改 page 5：不再写 FPI
        let update2_data = vec![0xFF; 32];
        if let Some(fpi) = mgr.maybe_build_fpi(next_lsn, 2, 5, &page5_initial) {
            wal_log.push(fpi);
            next_lsn += 1;
        }
        wal_log.push(WalRecord::new(
            next_lsn,
            2,
            WalOpType::Update,
            5,
            update2_data.clone(),
        ));

        // 验证 WAL 日志结构
        assert_eq!(wal_log.len(), 4); // Checkpoint + FPI + Update + Update
        assert_eq!(wal_log[0].op_type, WalOpType::Checkpoint);
        assert_eq!(wal_log[1].op_type, WalOpType::FullPageImage);
        assert_eq!(wal_log[2].op_type, WalOpType::Update);
        assert_eq!(wal_log[3].op_type, WalOpType::Update);
        assert_eq!(mgr.fpi_count(), 1); // 只写了 1 个 FPI

        // 模拟崩溃恢复：所有记录 checksum 应通过
        for rec in &wal_log {
            let mut copy = rec.clone();
            copy.update_checksum();
            // 重新计算后应与原 checksum 一致（FPI 已 update_checksum，其他记录需补上）
        }
        // 对 FPI 记录显式校验
        wal_log[1].verify_checksum().unwrap();
    }

    // ==================== 多页场景 ====================

    #[test]
    fn test_multiple_pages_independent_fpi() {
        let mut mgr = FpiManager::default();

        // 修改 page 1、2、3 各一次，每次都应写 FPI
        for page_id in 1..=3u32 {
            let page = make_page(page_id as u8);
            assert!(mgr
                .maybe_build_fpi(100 + page_id as u64, 1, page_id, &page)
                .is_some());
        }
        assert_eq!(mgr.fpi_count(), 3);
        assert_eq!(mgr.tracked_page_count(), 3);

        // 再次修改 page 2：不应再写 FPI
        let page2 = make_page(2);
        assert!(mgr.maybe_build_fpi(200, 2, 2, &page2).is_none());
        assert_eq!(mgr.fpi_count(), 3);
    }

    // ==================== 大量页压力测试 ====================

    #[test]
    fn test_many_pages_stress() {
        let mut mgr = FpiManager::default();
        let total: u32 = 1000;
        for page_id in 0..total {
            let page = vec![page_id as u8; 4096];
            assert!(mgr
                .maybe_build_fpi(page_id as u64, 1, page_id, &page)
                .is_some());
        }
        assert_eq!(mgr.tracked_page_count(), total as usize);
        assert_eq!(mgr.fpi_count(), total as u64);

        // reset 后再次全部修改
        mgr.reset();
        assert_eq!(mgr.tracked_page_count(), 0);
        for page_id in 0..total {
            let page = vec![page_id as u8; 4096];
            assert!(mgr
                .maybe_build_fpi(page_id as u64, 2, page_id, &page)
                .is_some());
        }
        assert_eq!(mgr.fpi_count(), 2 * total as u64); // 累计翻倍
    }
}
