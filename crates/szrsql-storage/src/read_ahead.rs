//! 预读（Read-Ahead）：顺序扫描时提前加载后续页面，减少 I/O 等待。
//!
//! 对应 `SzRSQL技术实现方案.md` 第 7d.6 节。
//!
//! 设计思路：
//! - **顺序访问检测**：连续访问 page_id 递增时，判定为顺序扫描。
//! - **预读触发**：检测到连续 N 次顺序访问后，预读后续 M 页（默认 16）。
//! - **预读缓冲**：提前加载的页存入预读缓冲区，下次访问时直接命中。
//! - **I/O 节省**：命中预读缓冲区时不发生实际 I/O，减少 I/O 等待时间。

use std::collections::HashMap;

// =====================================================================
//  常量
// =====================================================================

/// 默认预读页数。
pub const DEFAULT_READ_AHEAD_PAGES: usize = 16;

/// 默认触发预读的连续顺序访问次数。
pub const DEFAULT_SEQUENTIAL_THRESHOLD: usize = 3;

/// 默认预读缓冲区容量（页数）。
pub const DEFAULT_BUFFER_CAPACITY: usize = 64;

/// 模拟单页 I/O 等待时间（微秒）。
pub const IO_WAIT_US: u64 = 1_000;

// =====================================================================
//  Page 模拟
// =====================================================================

/// 模拟磁盘页：page_id + 数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// 页 ID。
    pub page_id: u64,
    /// 页数据。
    pub data: Vec<u8>,
}

impl Page {
    /// 创建页。
    pub fn new(page_id: u64, data: Vec<u8>) -> Self {
        Self { page_id, data }
    }

    /// 创建空页（指定 page_id）。
    pub fn empty(page_id: u64) -> Self {
        Self {
            page_id,
            data: Vec::new(),
        }
    }

    /// 页大小（字节）。
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

// =====================================================================
//  预读统计
// =====================================================================

/// 预读统计信息。
#[derive(Debug, Clone, Default)]
pub struct ReadAheadStats {
    /// 总访问次数。
    pub total_accesses: u64,
    /// 预读缓冲命中次数。
    pub buffer_hits: u64,
    /// 实际 I/O 次数（未命中预读缓冲）。
    pub io_reads: u64,
    /// 预读触发次数。
    pub read_ahead_triggers: u64,
    /// 预读的页总数。
    pub pages_read_ahead: u64,
    /// 顺序访问次数。
    pub sequential_accesses: u64,
    /// 随机访问次数。
    pub random_accesses: u64,
    /// 节省的 I/O 等待时间（微秒）。
    pub io_time_saved_us: u64,
    /// 实际 I/O 等待时间（微秒）。
    pub io_time_spent_us: u64,
}

impl ReadAheadStats {
    /// 创建空统计。
    pub fn new() -> Self {
        Self::default()
    }

    /// 预读命中率（0.0~1.0）。
    pub fn hit_rate(&self) -> f64 {
        if self.total_accesses == 0 {
            return 0.0;
        }
        self.buffer_hits as f64 / self.total_accesses as f64
    }

    /// 顺序访问比例（0.0~1.0）。
    pub fn sequential_rate(&self) -> f64 {
        let total = self.sequential_accesses + self.random_accesses;
        if total == 0 {
            return 0.0;
        }
        self.sequential_accesses as f64 / total as f64
    }

    /// I/O 等待时间减少比例（0.0~1.0）。
    /// 减少 = 节省 / (节省 + 实际)。
    pub fn io_wait_reduction(&self) -> f64 {
        let total = self.io_time_saved_us + self.io_time_spent_us;
        if total == 0 {
            return 0.0;
        }
        self.io_time_saved_us as f64 / total as f64
    }

    /// 是否达到 I/O 等待减少 >= 30% 的目标。
    pub fn meets_30_percent_reduction(&self) -> bool {
        self.io_wait_reduction() >= 0.3
    }
}

// =====================================================================
//  预读管理器
// =====================================================================

/// 预读管理器：检测顺序访问模式，提前加载后续页面。
///
/// 算法：
/// 1. 每次 `access` 一个 page_id 时，先检查预读缓冲区。
/// 2. 命中：直接返回，统计 buffer_hit，记录节省的 I/O 时间。
/// 3. 未命中：发生实际 I/O，统计 io_read。
/// 4. 检测顺序访问：如果 page_id == last_page_id + 1，sequential_count++；
///    否则 sequential_count 重置为 0。
/// 5. 当 sequential_count >= threshold 时，触发预读后续 read_ahead_pages 页。
pub struct ReadAheadManager {
    /// 预读缓冲区：page_id -> Page。
    buffer: HashMap<u64, Page>,
    /// 预读缓冲区容量。
    buffer_capacity: usize,
    /// 预读页数。
    read_ahead_pages: usize,
    /// 触发预读的连续顺序访问阈值。
    sequential_threshold: usize,
    /// 上次访问的 page_id。
    last_page_id: Option<u64>,
    /// 连续顺序访问计数。
    sequential_count: usize,
    /// 统计信息。
    stats: ReadAheadStats,
    /// 模拟磁盘：page_id -> Page（用于预读加载）。
    disk: HashMap<u64, Page>,
    /// 预读队列（按 LRU 顺序淘汰）。
    lru_order: Vec<u64>,
}

impl ReadAheadManager {
    /// 创建预读管理器。
    pub fn new(
        buffer_capacity: usize,
        read_ahead_pages: usize,
        sequential_threshold: usize,
    ) -> Self {
        Self {
            buffer: HashMap::new(),
            buffer_capacity,
            read_ahead_pages,
            sequential_threshold,
            last_page_id: None,
            sequential_count: 0,
            stats: ReadAheadStats::new(),
            disk: HashMap::new(),
            lru_order: Vec::new(),
        }
    }

    /// 使用默认配置创建。
    pub fn with_default() -> Self {
        Self::new(
            DEFAULT_BUFFER_CAPACITY,
            DEFAULT_READ_AHEAD_PAGES,
            DEFAULT_SEQUENTIAL_THRESHOLD,
        )
    }

    /// 加载磁盘数据（模拟磁盘内容）。
    pub fn load_disk(&mut self, pages: Vec<Page>) {
        for page in pages {
            self.disk.insert(page.page_id, page);
        }
    }

    /// 访问一个页面。返回页面数据。
    /// 命中预读缓冲区时不发生 I/O；未命中时发生 I/O。
    pub fn access(&mut self, page_id: u64) -> Option<Page> {
        self.stats.total_accesses += 1;

        // 检测顺序访问
        let is_sequential = matches!(self.last_page_id, Some(last) if last + 1 == page_id);

        if is_sequential {
            self.stats.sequential_accesses += 1;
            self.sequential_count += 1;
        } else {
            self.stats.random_accesses += 1;
            self.sequential_count = 0;
        }
        self.last_page_id = Some(page_id);

        // 检查预读缓冲区
        if let Some(page) = self.buffer.get(&page_id) {
            // 缓冲命中
            let page = page.clone();
            self.stats.buffer_hits += 1;
            self.stats.io_time_saved_us += IO_WAIT_US;
            // 更新 LRU
            self.touch_lru(page_id);
            return Some(page);
        }

        // 未命中：发生实际 I/O
        self.stats.io_reads += 1;
        self.stats.io_time_spent_us += IO_WAIT_US;

        let page = self.disk.get(&page_id).cloned()?;
        // 放入缓冲区
        self.put_buffer(page.clone());

        // 检查是否触发预读
        if is_sequential && self.sequential_count >= self.sequential_threshold {
            self.trigger_read_ahead(page_id);
        }

        Some(page)
    }

    /// 触发预读：提前加载后续 read_ahead_pages 页。
    fn trigger_read_ahead(&mut self, current_page_id: u64) {
        self.stats.read_ahead_triggers += 1;
        let start = current_page_id + 1;
        let end = start + self.read_ahead_pages as u64;

        for pid in start..end {
            if let Some(page) = self.disk.get(&pid) {
                // 只加载不在缓冲区中的页
                if !self.buffer.contains_key(&pid) {
                    self.put_buffer(page.clone());
                    self.stats.pages_read_ahead += 1;
                }
            } else {
                // 磁盘上没有该页，停止预读
                break;
            }
        }
    }

    /// 放入缓冲区，必要时淘汰最旧的页。
    fn put_buffer(&mut self, page: Page) {
        let pid = page.page_id;
        if !self.buffer.contains_key(&pid) {
            // 缓冲区满则淘汰
            while self.buffer.len() >= self.buffer_capacity && !self.lru_order.is_empty() {
                let evicted = self.lru_order.remove(0);
                self.buffer.remove(&evicted);
            }
        }
        self.buffer.insert(pid, page);
        self.touch_lru(pid);
    }

    /// 更新 LRU 顺序。
    fn touch_lru(&mut self, page_id: u64) {
        self.lru_order.retain(|&pid| pid != page_id);
        self.lru_order.push(page_id);
    }

    /// 获取统计信息。
    pub fn stats(&self) -> &ReadAheadStats {
        &self.stats
    }

    /// 预读缓冲区当前页数。
    pub fn buffer_size(&self) -> usize {
        self.buffer.len()
    }

    /// 预读缓冲区容量。
    pub fn buffer_capacity(&self) -> usize {
        self.buffer_capacity
    }

    /// 预读页数。
    pub fn read_ahead_pages(&self) -> usize {
        self.read_ahead_pages
    }

    /// 顺序访问阈值。
    pub fn sequential_threshold(&self) -> usize {
        self.sequential_threshold
    }

    /// 缓冲区是否包含指定页。
    pub fn contains(&self, page_id: u64) -> bool {
        self.buffer.contains_key(&page_id)
    }

    /// 上次访问的 page_id。
    pub fn last_page_id(&self) -> Option<u64> {
        self.last_page_id
    }

    /// 当前连续顺序访问计数。
    pub fn sequential_count(&self) -> usize {
        self.sequential_count
    }

    /// 重置所有状态。
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.lru_order.clear();
        self.last_page_id = None;
        self.sequential_count = 0;
        self.stats = ReadAheadStats::new();
    }

    /// 清空预读缓冲区（保留统计）。
    pub fn clear_buffer(&mut self) {
        self.buffer.clear();
        self.lru_order.clear();
    }

    /// 重置顺序访问检测状态（不清空缓冲区）。
    pub fn reset_sequential(&mut self) {
        self.last_page_id = None;
        self.sequential_count = 0;
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 生成 N 个连续页（page_id 从 0 开始，每页 size 字节）。
pub fn generate_sequential_pages(n: usize, page_size: usize) -> Vec<Page> {
    (0..n as u64)
        .map(|i| Page::new(i, vec![i as u8; page_size]))
        .collect()
}

/// 生成随机访问序列（page_id 在 0..range 范围内）。
pub fn generate_random_access_sequence(n: usize, range: u64) -> Vec<u64> {
    use std::cell::Cell;
    thread_local! {
        static SEED: Cell<u64> = const { Cell::new(0xdeadbeef_cafebabe) };
    }
    SEED.with(|seed| {
        (0..n as u64)
            .map(|_| {
                let s = seed.get();
                let new_s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                seed.set(new_s);
                s % range
            })
            .collect()
    })
}

/// 生成顺序访问序列（page_id 从 0 递增）。
pub fn generate_sequential_access_sequence(n: usize) -> Vec<u64> {
    (0..n as u64).collect()
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  Page 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_page_new() {
        let page = Page::new(1, vec![1, 2, 3]);
        assert_eq!(page.page_id, 1);
        assert_eq!(page.data, vec![1, 2, 3]);
    }

    #[test]
    fn test_page_empty() {
        let page = Page::empty(5);
        assert_eq!(page.page_id, 5);
        assert!(page.data.is_empty());
    }

    #[test]
    fn test_page_size() {
        let page = Page::new(1, vec![0; 4096]);
        assert_eq!(page.size(), 4096);
    }

    #[test]
    fn test_page_eq() {
        let p1 = Page::new(1, vec![1]);
        let p2 = Page::new(1, vec![1]);
        assert_eq!(p1, p2);
    }

    // -----------------------------------------------------------------
    //  ReadAheadStats 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_stats_default() {
        let stats = ReadAheadStats::default();
        assert_eq!(stats.total_accesses, 0);
        assert_eq!(stats.buffer_hits, 0);
        assert_eq!(stats.io_reads, 0);
    }

    #[test]
    fn test_stats_hit_rate_no_access() {
        let stats = ReadAheadStats::new();
        assert_eq!(stats.hit_rate(), 0.0);
    }

    #[test]
    fn test_stats_hit_rate_all_hits() {
        let stats = ReadAheadStats {
            total_accesses: 100,
            buffer_hits: 100,
            ..Default::default()
        };
        assert_eq!(stats.hit_rate(), 1.0);
    }

    #[test]
    fn test_stats_hit_rate_half() {
        let stats = ReadAheadStats {
            total_accesses: 100,
            buffer_hits: 50,
            ..Default::default()
        };
        assert_eq!(stats.hit_rate(), 0.5);
    }

    #[test]
    fn test_stats_sequential_rate_no_access() {
        let stats = ReadAheadStats::new();
        assert_eq!(stats.sequential_rate(), 0.0);
    }

    #[test]
    fn test_stats_sequential_rate_all_sequential() {
        let stats = ReadAheadStats {
            sequential_accesses: 100,
            random_accesses: 0,
            ..Default::default()
        };
        assert_eq!(stats.sequential_rate(), 1.0);
    }

    #[test]
    fn test_stats_sequential_rate_mixed() {
        let stats = ReadAheadStats {
            sequential_accesses: 70,
            random_accesses: 30,
            ..Default::default()
        };
        assert!((stats.sequential_rate() - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_stats_io_wait_reduction_no_io() {
        let stats = ReadAheadStats::new();
        assert_eq!(stats.io_wait_reduction(), 0.0);
    }

    #[test]
    fn test_stats_io_wait_reduction_50_percent() {
        let stats = ReadAheadStats {
            io_time_saved_us: 500,
            io_time_spent_us: 500,
            ..Default::default()
        };
        assert!((stats.io_wait_reduction() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_stats_io_wait_reduction_60_percent() {
        let stats = ReadAheadStats {
            io_time_saved_us: 600,
            io_time_spent_us: 400,
            ..Default::default()
        };
        assert!((stats.io_wait_reduction() - 0.6).abs() < 0.001);
    }

    #[test]
    fn test_stats_meets_30_percent_reduction_yes() {
        let stats = ReadAheadStats {
            io_time_saved_us: 600,
            io_time_spent_us: 400,
            ..Default::default()
        };
        assert!(stats.meets_30_percent_reduction());
    }

    #[test]
    fn test_stats_meets_30_percent_reduction_no() {
        let stats = ReadAheadStats {
            io_time_saved_us: 200,
            io_time_spent_us: 800,
            ..Default::default()
        };
        assert!(!stats.meets_30_percent_reduction());
    }

    // -----------------------------------------------------------------
    //  ReadAheadManager 基本操作测试
    // -----------------------------------------------------------------

    #[test]
    fn test_manager_new() {
        let manager = ReadAheadManager::new(64, 16, 3);
        assert_eq!(manager.buffer_capacity(), 64);
        assert_eq!(manager.read_ahead_pages(), 16);
        assert_eq!(manager.sequential_threshold(), 3);
        assert_eq!(manager.buffer_size(), 0);
        assert_eq!(manager.last_page_id(), None);
        assert_eq!(manager.sequential_count(), 0);
    }

    #[test]
    fn test_manager_with_default() {
        let manager = ReadAheadManager::with_default();
        assert_eq!(manager.buffer_capacity(), DEFAULT_BUFFER_CAPACITY);
        assert_eq!(manager.read_ahead_pages(), DEFAULT_READ_AHEAD_PAGES);
        assert_eq!(manager.sequential_threshold(), DEFAULT_SEQUENTIAL_THRESHOLD);
    }

    #[test]
    fn test_manager_load_disk() {
        let mut manager = ReadAheadManager::with_default();
        let pages = generate_sequential_pages(100, 64);
        manager.load_disk(pages);
        // 磁盘已加载（通过 access 验证）
        let page = manager.access(0);
        assert!(page.is_some());
        assert_eq!(page.unwrap().page_id, 0);
    }

    #[test]
    fn test_manager_access_first_page() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(100, 64));

        let page = manager.access(0);
        assert!(page.is_some());
        assert_eq!(manager.stats().total_accesses, 1);
        assert_eq!(manager.stats().io_reads, 1);
        assert_eq!(manager.stats().buffer_hits, 0);
        // 第一次访问不是顺序访问
        assert_eq!(manager.stats().sequential_accesses, 0);
        assert_eq!(manager.stats().random_accesses, 1);
    }

    #[test]
    fn test_manager_access_missing_page() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(10, 64));

        let page = manager.access(100); // 磁盘上没有
        assert!(page.is_none());
        assert_eq!(manager.stats().io_reads, 1);
    }

    #[test]
    fn test_manager_access_cached_page() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(100, 64));

        // 第一次访问：I/O
        manager.access(0);
        // 第二次访问：缓冲命中
        manager.reset_sequential();
        let page = manager.access(0);
        assert!(page.is_some());
        assert_eq!(manager.stats().buffer_hits, 1);
        assert_eq!(manager.stats().io_reads, 1);
    }

    #[test]
    fn test_manager_sequential_detection() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(100, 64));

        manager.access(0); // 随机
        assert_eq!(manager.sequential_count(), 0);

        manager.access(1); // 顺序
        assert_eq!(manager.sequential_count(), 1);

        manager.access(2); // 顺序
        assert_eq!(manager.sequential_count(), 2);

        manager.access(5); // 随机（不连续）
        assert_eq!(manager.sequential_count(), 0);
    }

    #[test]
    fn test_manager_read_ahead_trigger() {
        let mut manager = ReadAheadManager::new(64, 4, 2); // 阈值 2，预读 4 页
        manager.load_disk(generate_sequential_pages(100, 64));

        manager.access(0); // 随机
        manager.access(1); // 顺序，count=1
        manager.access(2); // 顺序，count=2 >= 阈值，触发预读 page 3,4,5,6

        assert!(manager.stats().read_ahead_triggers >= 1);
        assert!(manager.stats().pages_read_ahead >= 4);
        // 预读的页应在缓冲区中
        assert!(manager.contains(3));
        assert!(manager.contains(4));
        assert!(manager.contains(5));
        assert!(manager.contains(6));
    }

    #[test]
    fn test_manager_read_ahead_no_trigger_below_threshold() {
        let mut manager = ReadAheadManager::new(64, 4, 5); // 阈值 5
        manager.load_disk(generate_sequential_pages(100, 64));

        manager.access(0);
        manager.access(1); // count=1
        manager.access(2); // count=2

        assert_eq!(manager.stats().read_ahead_triggers, 0);
    }

    #[test]
    fn test_manager_read_ahead_buffer_hit_after_trigger() {
        let mut manager = ReadAheadManager::new(64, 4, 2);
        manager.load_disk(generate_sequential_pages(100, 64));

        manager.access(0);
        manager.access(1); // count=1
        manager.access(2); // count=2，触发预读 3,4,5,6

        // 访问预读的页：应缓冲命中
        manager.reset_sequential();
        let page = manager.access(3);
        assert!(page.is_some());
        assert!(manager.stats().buffer_hits >= 1);
    }

    #[test]
    fn test_manager_buffer_capacity_eviction() {
        let mut manager = ReadAheadManager::new(3, 0, 100); // 容量 3，不预读
        manager.load_disk(generate_sequential_pages(100, 64));

        manager.access(0);
        manager.access(1);
        manager.access(2);
        assert_eq!(manager.buffer_size(), 3);

        // 访问第 4 页，应淘汰最旧的
        manager.reset_sequential();
        manager.access(3);
        assert_eq!(manager.buffer_size(), 3);
        // page 0 应被淘汰
        assert!(!manager.contains(0));
        assert!(manager.contains(3));
    }

    #[test]
    fn test_manager_lru_update_on_access() {
        let mut manager = ReadAheadManager::new(2, 0, 100);
        manager.load_disk(generate_sequential_pages(100, 64));

        manager.access(0);
        manager.access(1);
        assert_eq!(manager.buffer_size(), 2);

        // 访问 page 0，更新 LRU
        manager.reset_sequential();
        manager.access(0); // 命中

        // 访问 page 2，应淘汰 page 1（最久未用）
        manager.reset_sequential();
        manager.access(2);
        assert!(manager.contains(0));
        assert!(!manager.contains(1));
        assert!(manager.contains(2));
    }

    // -----------------------------------------------------------------
    //  ReadAheadManager 统计测试
    // -----------------------------------------------------------------

    #[test]
    fn test_manager_stats_after_sequential_scan() {
        let mut manager = ReadAheadManager::new(64, 8, 2);
        manager.load_disk(generate_sequential_pages(100, 64));

        // 顺序扫描 50 页
        for i in 0..50u64 {
            manager.access(i);
        }

        let stats = manager.stats();
        assert_eq!(stats.total_accesses, 50);
        // 应有缓冲命中（预读的页）
        assert!(stats.buffer_hits > 0);
        // 应触发了预读
        assert!(stats.read_ahead_triggers > 0);
        // 顺序访问应占大多数
        assert!(stats.sequential_accesses > stats.random_accesses);
    }

    #[test]
    fn test_manager_stats_after_random_access() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(100, 64));

        let sequence = generate_random_access_sequence(50, 100);
        for pid in sequence {
            manager.access(pid);
        }

        let stats = manager.stats();
        assert_eq!(stats.total_accesses, 50);
        // 随机访问不应触发预读
        assert_eq!(stats.read_ahead_triggers, 0);
        assert_eq!(stats.pages_read_ahead, 0);
    }

    #[test]
    fn test_manager_io_wait_reduction_sequential() {
        let mut manager = ReadAheadManager::new(64, 16, 2);
        manager.load_disk(generate_sequential_pages(1000, 64));

        // 顺序扫描 100 页
        for i in 0..100u64 {
            manager.access(i);
        }

        let stats = manager.stats();
        // I/O 等待减少应 >= 30%
        assert!(
            stats.meets_30_percent_reduction(),
            "I/O wait reduction {:.2}% should be >= 30%, hits={}, io={}, saved={}us, spent={}us",
            stats.io_wait_reduction() * 100.0,
            stats.buffer_hits,
            stats.io_reads,
            stats.io_time_saved_us,
            stats.io_time_spent_us
        );
    }

    #[test]
    fn test_manager_io_wait_reduction_random_low() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(1000, 64));

        let sequence = generate_random_access_sequence(100, 1000);
        for pid in sequence {
            manager.access(pid);
        }

        let stats = manager.stats();
        // 随机访问的 I/O 等待减少应较低
        assert!(stats.io_wait_reduction() < 0.5);
    }

    // -----------------------------------------------------------------
    //  ReadAheadManager 其他方法测试
    // -----------------------------------------------------------------

    #[test]
    fn test_manager_contains() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(10, 64));

        assert!(!manager.contains(0));
        manager.access(0);
        assert!(manager.contains(0));
    }

    #[test]
    fn test_manager_reset() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(100, 64));
        manager.access(0);
        manager.access(1);

        manager.reset();
        assert_eq!(manager.buffer_size(), 0);
        assert_eq!(manager.last_page_id(), None);
        assert_eq!(manager.sequential_count(), 0);
        assert_eq!(manager.stats().total_accesses, 0);
    }

    #[test]
    fn test_manager_clear_buffer() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(100, 64));
        manager.access(0);
        manager.access(1);

        let total_before = manager.stats().total_accesses;
        manager.clear_buffer();
        assert_eq!(manager.buffer_size(), 0);
        // 统计应保留
        assert_eq!(manager.stats().total_accesses, total_before);
    }

    #[test]
    fn test_manager_reset_sequential() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(100, 64));
        manager.access(0);
        manager.access(1);
        assert_eq!(manager.sequential_count(), 1);

        manager.reset_sequential();
        assert_eq!(manager.last_page_id(), None);
        assert_eq!(manager.sequential_count(), 0);
        // 缓冲区应保留
        assert!(manager.buffer_size() > 0);
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_generate_sequential_pages() {
        let pages = generate_sequential_pages(10, 64);
        assert_eq!(pages.len(), 10);
        assert_eq!(pages[0].page_id, 0);
        assert_eq!(pages[9].page_id, 9);
        assert_eq!(pages[0].size(), 64);
    }

    #[test]
    fn test_generate_sequential_pages_empty() {
        let pages = generate_sequential_pages(0, 64);
        assert!(pages.is_empty());
    }

    #[test]
    fn test_generate_random_access_sequence_count() {
        let seq = generate_random_access_sequence(100, 50);
        assert_eq!(seq.len(), 100);
        // 所有 page_id 应在 0..50 范围内
        assert!(seq.iter().all(|&pid| pid < 50));
    }

    #[test]
    fn test_generate_random_access_sequence_empty() {
        let seq = generate_random_access_sequence(0, 50);
        assert!(seq.is_empty());
    }

    #[test]
    fn test_generate_sequential_access_sequence() {
        let seq = generate_sequential_access_sequence(10);
        assert_eq!(seq, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn test_generate_sequential_access_sequence_empty() {
        let seq = generate_sequential_access_sequence(0);
        assert!(seq.is_empty());
    }

    // -----------------------------------------------------------------
    //  集成测试：完整工作流
    // -----------------------------------------------------------------

    #[test]
    fn test_integration_full_sequential_scan() {
        let mut manager = ReadAheadManager::new(64, 16, 2);
        manager.load_disk(generate_sequential_pages(500, 64));

        // 顺序扫描 500 页
        for i in 0..500u64 {
            let page = manager.access(i);
            assert!(page.is_some(), "page {} should exist", i);
            assert_eq!(page.unwrap().page_id, i);
        }

        let stats = manager.stats();
        assert_eq!(stats.total_accesses, 500);
        assert!(stats.buffer_hits > 0);
        assert!(stats.read_ahead_triggers > 0);
        assert!(stats.meets_30_percent_reduction());
    }

    #[test]
    fn test_integration_mixed_access_pattern() {
        let mut manager = ReadAheadManager::with_default();
        manager.load_disk(generate_sequential_pages(1000, 64));

        // 先随机访问 20 次
        let random_seq = generate_random_access_sequence(20, 1000);
        for pid in random_seq {
            manager.access(pid);
        }

        // 再顺序扫描 100 页
        for i in 0..100u64 {
            manager.access(i);
        }

        let stats = manager.stats();
        assert_eq!(stats.total_accesses, 120);
        assert!(stats.buffer_hits > 0);
    }

    #[test]
    fn test_integration_read_ahead_skips_cached_pages() {
        let mut manager = ReadAheadManager::new(64, 8, 2);
        manager.load_disk(generate_sequential_pages(100, 64));

        // 先访问 page 5，放入缓冲区
        manager.access(5);

        // 重置顺序检测
        manager.reset_sequential();

        // 顺序访问 0,1,2 触发预读 3,4,5,6,7,8,9,10
        manager.access(0);
        manager.access(1);
        manager.access(2); // 触发预读

        // page 5 已在缓冲区，不应重复预读
        let stats = manager.stats();
        // pages_read_ahead 应 <= 8（因为 page 5 已缓存，不重复计数）
        assert!(stats.pages_read_ahead <= 8);
    }

    #[test]
    fn test_integration_read_ahead_stops_at_disk_end() {
        let mut manager = ReadAheadManager::new(64, 16, 2);
        manager.load_disk(generate_sequential_pages(10, 64)); // 只有 10 页

        manager.access(0);
        manager.access(1);
        manager.access(2); // 触发预读，但磁盘只有 10 页

        // 预读应在 page 9 后停止
        assert!(manager.contains(9));
        assert!(!manager.contains(10)); // 磁盘上没有
    }

    #[test]
    fn test_integration_repeated_sequential_scans() {
        let mut manager = ReadAheadManager::new(32, 8, 2);
        manager.load_disk(generate_sequential_pages(200, 64));

        // 第一次顺序扫描
        for i in 0..100u64 {
            manager.access(i);
        }
        let stats1 = manager.stats().clone();

        // 重置顺序检测，第二次扫描
        manager.reset_sequential();
        for i in 0..100u64 {
            manager.access(i);
        }
        let stats2 = manager.stats();

        // 第二次扫描应有更多缓冲命中
        assert!(stats2.buffer_hits > stats1.buffer_hits);
    }

    #[test]
    fn test_integration_correctness_data_integrity() {
        let mut manager = ReadAheadManager::with_default();
        let pages = generate_sequential_pages(100, 64);
        let original_data = pages[50].data.clone();
        manager.load_disk(pages);

        // 顺序扫描触发预读
        for i in 0..60u64 {
            manager.access(i);
        }

        // 访问预读的 page 50，验证数据完整性
        manager.reset_sequential();
        let page = manager.access(50);
        assert!(page.is_some());
        assert_eq!(page.unwrap().data, original_data);
    }

    #[test]
    fn test_integration_io_wait_reduction_30_percent_target() {
        // 验证 30% I/O 等待减少目标
        let mut manager = ReadAheadManager::new(128, 16, 2);
        manager.load_disk(generate_sequential_pages(1000, 64));

        // 顺序扫描 500 页
        for i in 0..500u64 {
            manager.access(i);
        }

        let stats = manager.stats();
        let reduction = stats.io_wait_reduction();
        assert!(
            reduction >= 0.3,
            "I/O wait reduction should be >= 30%, got {:.2}% (saved={}us, spent={}us, hits={}, io={})",
            reduction * 100.0,
            stats.io_time_saved_us,
            stats.io_time_spent_us,
            stats.buffer_hits,
            stats.io_reads
        );
    }

    // -----------------------------------------------------------------
    //  大规模测试（#[ignore]，手动运行）
    // -----------------------------------------------------------------

    #[test]
    #[ignore = "大规模测试：1 亿行顺序扫描预读"]
    fn test_integration_large_scale_sequential_scan_100_million() {
        let mut manager = ReadAheadManager::new(256, 16, 2);
        // 模拟 1 亿页（每页 64 字节 = 6.4GB），但只生成 100 万页用于测试
        // 真实 1 亿页需要太多内存，此处用 100 万页验证算法
        manager.load_disk(generate_sequential_pages(1_000_000, 64));

        for i in 0..1_000_000u64 {
            manager.access(i);
        }

        let stats = manager.stats();
        assert_eq!(stats.total_accesses, 1_000_000);
        assert!(stats.meets_30_percent_reduction());
        // 命中率应较高
        assert!(stats.hit_rate() > 0.5);
    }
}
