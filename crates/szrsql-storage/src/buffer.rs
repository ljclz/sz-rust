//! SzRSQL 缓冲池 — 对应 `SzRSQL技术实现方案.md` 9.3 节。
//!
//! 分片 LRU 缓冲池，支持 Pin/Unpin 计数器、脏页标记、异步刷盘。
//! Phase 0.9: LRU 淘汰 + Pin/Unpin 核心逻辑
//! Phase 0.10: 脏页链表 + 同步/异步刷盘 + 崩溃恢复

use crate::page::{Page, PageError, PageType};
use tracing::{instrument, trace, warn};
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::Mutex;

// =====================================================================
//  BufferError
// =====================================================================

/// 缓冲池错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BufferError {
    #[error("buffer pool is full: no evictable pages (all pinned)")]
    NoEvictablePages,
    #[error("page {page_id} not found in buffer pool")]
    PageNotFound { page_id: u32 },
    #[error("page {page_id} is already pinned {pin_count} times")]
    AlreadyPinned { page_id: u32, pin_count: i32 },
    #[error("pin count underflow for page {page_id}")]
    PinCountUnderflow { page_id: u32 },
    #[error("capacity must be > 0, got {0}")]
    InvalidCapacity(usize),
    #[error("page loader error: {0}")]
    LoaderError(String),
    #[error("page writer error: {0}")]
    WriterError(String),
    #[error("doublewrite buffer error: {0}")]
    DoublewriteError(String),
    #[error("flush worker already running")]
    FlushWorkerRunning,
    #[error("page error: {0}")]
    PageError(#[from] PageError),
    /// P0-STORE-2：文件 I/O 错误（FilePageLoader/FilePageWriter 使用）
    #[error("io error: {0}")]
    IoError(String),
}

// =====================================================================
//  PageLoader / PageWriter — 磁盘 I/O 回调（解耦缓冲池与存储后端）
// =====================================================================

/// 页加载器：当缓冲池未命中时调用，返回 page_id 对应的 Page
///
/// 使用 trait object 允许不同后端（内存模拟 / 文件 / mmap）注入
pub trait PageLoader: Send + Sync {
    fn load_page(&self, page_id: u32) -> Result<Page, BufferError>;
}

/// 页写入器：刷盘时调用，将 Page 持久化到存储后端
///
/// 实现方需保证：
/// 1. write_page 完成后数据已持久化（fsync 或等价语义）
/// 2. write_page 是线程安全的
pub trait PageWriter: Send + Sync {
    fn write_page(&self, page: &Page) -> Result<(), BufferError>;
}

/// 内存页加载器 — 用于测试：从一个 HashMap<page_id, Page> 读取
pub struct InMemoryPageLoader {
    pages: Mutex<std::collections::HashMap<u32, Page>>,
}

impl InMemoryPageLoader {
    pub fn new() -> Self {
        Self {
            pages: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 插入一个预生成的页
    pub fn insert(&self, page_id: u32, page: Page) {
        self.pages.lock().insert(page_id, page);
    }

    /// 生成一个空白数据页并插入
    pub fn insert_blank(&self, page_id: u32) {
        let mut page = Page::new(page_id, PageType::Data);
        page.update_checksum();
        self.insert(page_id, page);
    }

    /// 获取某页（用于测试断言）
    pub fn get_persisted(&self, page_id: u32) -> Option<Page> {
        self.pages.lock().get(&page_id).cloned()
    }
}

impl Default for InMemoryPageLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl PageLoader for InMemoryPageLoader {
    fn load_page(&self, page_id: u32) -> Result<Page, BufferError> {
        let pages = self.pages.lock();
        pages
            .get(&page_id)
            .cloned()
            .ok_or(BufferError::PageNotFound { page_id })
    }
}

/// 内存页写入器 — 用于测试：写入到 HashMap，支持"崩溃"模拟
///
/// `crash_flag` 为 true 时，write_page 返回 WriterError 模拟崩溃
pub struct InMemoryPageWriter {
    /// 持久化存储（模拟磁盘文件）
    persisted: Mutex<std::collections::HashMap<u32, Page>>,
    /// 崩溃标志：true 时所有 write_page 失败
    crash_flag: std::sync::atomic::AtomicBool,
    /// 写入计数（用于测试）
    write_count: std::sync::atomic::AtomicU64,
}

impl InMemoryPageWriter {
    pub fn new() -> Self {
        Self {
            persisted: Mutex::new(std::collections::HashMap::new()),
            crash_flag: std::sync::atomic::AtomicBool::new(false),
            write_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// 触发崩溃（后续 write_page 失败）
    pub fn crash(&self) {
        self.crash_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 恢复（清除崩溃标志）
    pub fn recover(&self) {
        self.crash_flag
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// 写入计数
    pub fn write_count(&self) -> u64 {
        self.write_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 获取持久化的页（测试断言用）
    pub fn get_persisted(&self, page_id: u32) -> Option<Page> {
        self.persisted.lock().get(&page_id).cloned()
    }

    /// 持久化页数量
    pub fn len(&self) -> usize {
        self.persisted.lock().len()
    }

    /// 是否没有持久化页
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 是否已崩溃
    pub fn is_crashed(&self) -> bool {
        self.crash_flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 获取所有已持久化的 page_id 列表（无序）
    ///
    /// 用于崩溃恢复后的校验：扫描所有已恢复到 writer 的页
    pub fn persisted_page_ids(&self) -> Vec<u32> {
        self.persisted.lock().keys().copied().collect()
    }
}

impl Default for InMemoryPageWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl PageWriter for InMemoryPageWriter {
    fn write_page(&self, page: &Page) -> Result<(), BufferError> {
        if self.crash_flag.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(BufferError::WriterError("crash simulated".into()));
        }
        self.write_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.persisted
            .lock()
            .insert(page.header.page_id, page.clone());
        Ok(())
    }
}

// =====================================================================
//  BufferPool — 分片 LRU 缓冲池
// =====================================================================

/// 缓冲池分片数量（16 分片减少锁竞争）
pub const SHARD_COUNT: usize = 16;

/// 缓冲池分片 — 每片独立 LRU + 锁
struct BufferPoolShard {
    /// LRU 链表（最近使用在前，最久未使用在后）
    /// 存储 page_id，实际 Page 存在 lookup 中
    lru_list: std::collections::VecDeque<u32>,
    /// page_id → PageEntry 映射
    lookup: std::collections::HashMap<u32, PageEntry>,
    /// 该分片的容量（总容量 / SHARD_COUNT）
    capacity: usize,
}

/// 缓冲池条目：Page + Pin 计数 + 脏页标志
struct PageEntry {
    page: Page,
    pin_count: std::sync::atomic::AtomicI32,
    dirty: std::sync::atomic::AtomicBool,
}

impl PageEntry {
    fn new(page: Page) -> Self {
        Self {
            page,
            pin_count: std::sync::atomic::AtomicI32::new(0),
            dirty: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl BufferPoolShard {
    fn new(capacity: usize) -> Self {
        Self {
            lru_list: std::collections::VecDeque::with_capacity(capacity),
            lookup: std::collections::HashMap::with_capacity(capacity),
            capacity,
        }
    }
}

/// 缓冲池统计信息
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BufferPoolStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub pin_count: u64,
    pub unpin_count: u64,
    pub flush_count: u64,
    pub dirty_pages: u64,
}

/// 缓冲池 — 分片 LRU，支持 Pin/Unpin + 脏页刷盘
pub struct BufferPool {
    shards: Vec<Mutex<BufferPoolShard>>,
    /// 实际使用的分片数（<= SHARD_COUNT，小容量时自动缩减）
    shard_count: usize,
    loader: std::sync::Arc<dyn PageLoader>,
    /// 可选的页写入器（Phase 0.10+ 启用）
    writer: std::sync::Arc<dyn PageWriter>,
    /// 可选的 Doublewrite Buffer（Phase 0.11 启用）
    doublewrite: Mutex<Option<DoublewriteBuffer>>,
    stats: Mutex<BufferPoolStats>,
    /// 异步刷盘线程句柄
    flush_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 异步刷盘停止标志
    flush_stop: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for BufferPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferPool")
            .field("shard_count", &self.shard_count)
            .field("stats", &self.stats)
            .finish()
    }
}

impl BufferPool {
    /// 创建缓冲池（兼容 Phase 0.9 的旧 API，writer 为 Noop）
    pub fn new(
        capacity: usize,
        loader: std::sync::Arc<dyn PageLoader>,
    ) -> Result<Self, BufferError> {
        Self::with_writer(capacity, loader, std::sync::Arc::new(NoopPageWriter))
    }

    /// 创建带写入器的缓冲池（Phase 0.10+）
    pub fn with_writer(
        capacity: usize,
        loader: std::sync::Arc<dyn PageLoader>,
        writer: std::sync::Arc<dyn PageWriter>,
    ) -> Result<Self, BufferError> {
        if capacity == 0 {
            return Err(BufferError::InvalidCapacity(capacity));
        }
        let shard_count = capacity.min(SHARD_COUNT);
        let per_shard = capacity.div_ceil(shard_count);
        let mut shards = Vec::with_capacity(shard_count);
        for _ in 0..shard_count {
            shards.push(Mutex::new(BufferPoolShard::new(per_shard)));
        }
        Ok(Self {
            shards,
            shard_count,
            loader,
            writer,
            doublewrite: Mutex::new(None),
            stats: Mutex::new(BufferPoolStats::default()),
            flush_handle: Mutex::new(None),
            flush_stop: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// 创建带写入器 + Doublewrite Buffer 的缓冲池（Phase 0.11+）
    pub fn with_doublewrite(
        capacity: usize,
        loader: std::sync::Arc<dyn PageLoader>,
        writer: std::sync::Arc<dyn PageWriter>,
        dwb_capacity: usize,
    ) -> Result<Self, BufferError> {
        let pool = Self::with_writer(capacity, loader, writer)?;
        *pool.doublewrite.lock() = Some(DoublewriteBuffer::new(dwb_capacity));
        Ok(pool)
    }

    /// 计算 page_id 属于哪个分片
    fn shard_for(&self, page_id: u32) -> usize {
        (page_id as usize) % self.shard_count
    }

    /// 访问 page_id：返回 Page 的克隆（不 Pin）
    ///
    /// 内部：命中则移到 LRU 头部，未命中则加载并可能淘汰
    #[instrument(skip(self), fields(page_id, hit = tracing::field::Empty))]
    pub fn read_page(&self, page_id: u32) -> Result<Page, BufferError> {
        let shard_idx = self.shard_for(page_id);

        // ===== 第一次锁：检查命中 =====
        {
            let shard_guard = self.shards[shard_idx].lock();
            let shard = &*shard_guard;

            if let Some(entry) = shard.lookup.get(&page_id) {
                // 命中：先 clone page，再修改 lru_list（避免借用冲突）
                let page_clone = entry.page.clone();
                drop(shard_guard);

                let mut shard_guard = self.shards[shard_idx].lock();
                let shard = &mut *shard_guard;
                shard.lru_list.retain(|&p| p != page_id);
                shard.lru_list.push_front(page_id);

                self.stats.lock().hits += 1;
                tracing::Span::current().record("hit", true);
                trace!(page_id, hit = true, "buffer pool hit");
                return Ok(page_clone);
            }
        }

        // 未命中
        self.stats.lock().misses += 1;
        tracing::Span::current().record("hit", false);

        // ===== 加载页（不持锁）=====
        let page = self.loader.load_page(page_id)?;
        trace!(
            page_id,
            hit = false,
            "buffer pool miss, loaded from storage"
        );

        // ===== 第二次锁：插入 =====
        let mut shard_guard = self.shards[shard_idx].lock();
        let shard = &mut *shard_guard;

        // 再次检查（可能在 drop 锁期间其他线程已加载）
        if let Some(entry) = shard.lookup.get(&page_id) {
            let page_clone = entry.page.clone();
            shard.lru_list.retain(|&p| p != page_id);
            shard.lru_list.push_front(page_id);
            return Ok(page_clone);
        }

        // 容量检查 — 需要淘汰？
        if shard.lookup.len() >= shard.capacity {
            warn!(page_id, "buffer pool full, evicting a page");
            self.evict_one_locked(shard)?;
        }

        // 插入
        shard.lookup.insert(page_id, PageEntry::new(page.clone()));
        shard.lru_list.push_front(page_id);

        Ok(page)
    }

    /// Pin 一个页（pin_count += 1，防止被淘汰）
    ///
    /// 返回当前 pin_count
    #[instrument(skip(self), fields(page_id))]
    pub fn pin_page(&self, page_id: u32) -> Result<i32, BufferError> {
        let shard_idx = self.shard_for(page_id);
        let mut shard_guard = self.shards[shard_idx].lock();
        let shard = &mut *shard_guard;

        // 先检查存在性
        if !shard.lookup.contains_key(&page_id) {
            return Err(BufferError::PageNotFound { page_id });
        }

        // 移到 LRU 头部
        shard.lru_list.retain(|&p| p != page_id);
        shard.lru_list.push_front(page_id);

        // 再操作 pin_count（此时 lru_list 借用已结束）
        let entry = shard.lookup.get(&page_id).unwrap();
        let new_count = entry
            .pin_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        self.stats.lock().pin_count += 1;
        Ok(new_count)
    }

    /// Unpin 一个页（pin_count -= 1）
    ///
    /// 返回当前 pin_count。若 pin_count 减到 0，该页可被淘汰
    pub fn unpin_page(&self, page_id: u32) -> Result<i32, BufferError> {
        let shard_idx = self.shard_for(page_id);
        let shard_guard = self.shards[shard_idx].lock();
        let shard = &*shard_guard;

        let entry = shard
            .lookup
            .get(&page_id)
            .ok_or(BufferError::PageNotFound { page_id })?;

        let current = entry.pin_count.load(std::sync::atomic::Ordering::SeqCst);
        if current <= 0 {
            return Err(BufferError::PinCountUnderflow { page_id });
        }

        let new_count = entry
            .pin_count
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
            - 1;
        self.stats.lock().unpin_count += 1;
        Ok(new_count)
    }

    /// 获取某页的 pin_count
    pub fn pin_count(&self, page_id: u32) -> Result<i32, BufferError> {
        let shard_idx = self.shard_for(page_id);
        let shard_guard = self.shards[shard_idx].lock();
        let shard = &*shard_guard;
        let entry = shard
            .lookup
            .get(&page_id)
            .ok_or(BufferError::PageNotFound { page_id })?;
        Ok(entry.pin_count.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// 标记某页为脏页（修改后需刷盘）
    pub fn mark_dirty(&self, page_id: u32) -> Result<(), BufferError> {
        let shard_idx = self.shard_for(page_id);
        let shard_guard = self.shards[shard_idx].lock();
        let shard = &*shard_guard;
        let entry = shard
            .lookup
            .get(&page_id)
            .ok_or(BufferError::PageNotFound { page_id })?;
        let was_dirty = entry.dirty.swap(true, std::sync::atomic::Ordering::SeqCst);
        if !was_dirty {
            self.stats.lock().dirty_pages += 1;
        }
        Ok(())
    }

    /// 检查某页是否为脏页
    pub fn is_dirty(&self, page_id: u32) -> Result<bool, BufferError> {
        let shard_idx = self.shard_for(page_id);
        let shard_guard = self.shards[shard_idx].lock();
        let shard = &*shard_guard;
        let entry = shard
            .lookup
            .get(&page_id)
            .ok_or(BufferError::PageNotFound { page_id })?;
        Ok(entry.dirty.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// 更新缓冲池中某页的内容（修改后自动 mark_dirty）
    ///
    /// 注意：调用方需保证已 Pin 该页，避免并发淘汰
    #[instrument(skip(self, new_page), fields(page_id))]
    pub fn write_page(&self, page_id: u32, new_page: Page) -> Result<(), BufferError> {
        let shard_idx = self.shard_for(page_id);
        let mut shard_guard = self.shards[shard_idx].lock();
        let shard = &mut *shard_guard;
        let entry = shard
            .lookup
            .get_mut(&page_id)
            .ok_or(BufferError::PageNotFound { page_id })?;
        entry.page = new_page;
        let was_dirty = entry.dirty.swap(true, std::sync::atomic::Ordering::SeqCst);
        if !was_dirty {
            self.stats.lock().dirty_pages += 1;
        }
        Ok(())
    }

    /// P0-STORE-2：upsert 语义写入页 — 若 page_id 已存在则更新，否则创建新 entry
    ///
    /// 与 `write_page` 的区别：`write_page` 要求 page_id 已缓存（否则 PageNotFound），
    /// `put_page` 支持首次写入新页（自动创建 PageEntry 插入 lookup + LRU）。
    ///
    /// **适用场景**：BufferPool 接入运行时持久化路径时，flush_to_disk 需要写入
    /// 首次创建的页（page 0 header + page 1..N data），这些页不在 loader 中。
    ///
    /// **淘汰策略**：若插入新页导致超出容量，按 LRU 淘汰最久未使用且 pin_count=0 的页。
    /// 若无可淘汰页（全部 pinned），返回 NoEvictablePages。
    pub fn put_page(&self, page_id: u32, new_page: Page) -> Result<(), BufferError> {
        let shard_idx = self.shard_for(page_id);
        let mut shard_guard = self.shards[shard_idx].lock();
        // 已存在：更新内容 + mark dirty
        if let Some(entry) = shard_guard.lookup.get_mut(&page_id) {
            entry.page = new_page;
            let was_dirty = entry.dirty.swap(true, std::sync::atomic::Ordering::SeqCst);
            if !was_dirty {
                self.stats.lock().dirty_pages += 1;
            }
            // 移到 LRU 前部
            if let Some(pos) = shard_guard.lru_list.iter().position(|&id| id == page_id) {
                shard_guard.lru_list.remove(pos);
                shard_guard.lru_list.push_front(page_id);
            }
            return Ok(());
        }
        // 不存在：需创建新 entry，先检查容量
        if shard_guard.lookup.len() >= shard_guard.capacity {
            // 尝试淘汰最久未使用且 pin_count=0 的页
            let evict_candidate = shard_guard
                .lru_list
                .iter()
                .rev()
                .find(|&&id| {
                    shard_guard
                        .lookup
                        .get(&id)
                        .map(|e| e.pin_count.load(std::sync::atomic::Ordering::SeqCst) == 0)
                        .unwrap_or(false)
                })
                .copied();
            let evict_id = match evict_candidate {
                Some(id) => id,
                None => return Err(BufferError::NoEvictablePages),
            };
            // 先在短作用域内取出 is_dirty + page_copy，避免长生命周期借用 shard_guard
            // 若 evict_id 不在 lookup 中（异常情况），跳过淘汰逻辑
            let is_dirty = shard_guard
                .lookup
                .get(&evict_id)
                .map(|e| e.dirty.load(std::sync::atomic::Ordering::SeqCst))
                .unwrap_or(false);
            let page_copy = shard_guard.lookup.get(&evict_id).map(|e| e.page.clone());
            if is_dirty {
                if let Some(page_copy) = page_copy {
                    drop(shard_guard);
                    if let Err(e) = self.writer.write_page(&page_copy) {
                        tracing::warn!(error = ?e, page_id = evict_id, "evict flush failed");
                    }
                    let mut sg = self.shards[shard_idx].lock();
                    if let Some(e) = sg.lookup.get_mut(&evict_id) {
                        e.dirty.store(false, std::sync::atomic::Ordering::SeqCst);
                    }
                    sg.lookup.remove(&evict_id);
                    if let Some(pos) = sg.lru_list.iter().position(|&id| id == evict_id) {
                        sg.lru_list.remove(pos);
                    }
                    // 重新获取锁以插入新 entry
                    shard_guard = self.shards[shard_idx].lock();
                }
            } else {
                shard_guard.lookup.remove(&evict_id);
                if let Some(pos) = shard_guard.lru_list.iter().position(|&id| id == evict_id) {
                    shard_guard.lru_list.remove(pos);
                }
            }
        }
        // 插入新 entry
        let entry = PageEntry::new(new_page);
        entry.dirty.store(true, std::sync::atomic::Ordering::SeqCst);
        shard_guard.lookup.insert(page_id, entry);
        shard_guard.lru_list.push_front(page_id);
        self.stats.lock().dirty_pages += 1;
        Ok(())
    }

    /// 同步刷盘：将所有脏页写入磁盘，清除 dirty 标志
    ///
    /// 如果启用了 Doublewrite Buffer，先写入 DWB 再写入实际存储
    #[instrument(skip(self), fields(flushed_count))]
    pub fn flush_all(&self) -> Result<usize, BufferError> {
        let mut flushed = 0usize;
        let mut pages_to_flush: Vec<Page> = Vec::new();
        let mut page_ids_to_clear: Vec<(usize, u32)> = Vec::new();

        // 1. 收集所有脏页
        for (shard_idx, shard_mutex) in self.shards.iter().enumerate() {
            let shard_guard = shard_mutex.lock();
            let shard = &*shard_guard;
            for (&page_id, entry) in shard.lookup.iter() {
                if entry.dirty.load(std::sync::atomic::Ordering::SeqCst) {
                    pages_to_flush.push(entry.page.clone());
                    page_ids_to_clear.push((shard_idx, page_id));
                }
            }
        }

        // 2. 如果启用了 Doublewrite Buffer，先写入 DWB
        {
            let dwb_guard = self.doublewrite.lock();
            if let Some(dwb) = dwb_guard.as_ref() {
                if !pages_to_flush.is_empty() {
                    dwb.write_pages(&pages_to_flush)?;
                }
            }
        }

        // 3. 写入实际存储
        for page in &pages_to_flush {
            self.writer.write_page(page)?;
            flushed += 1;
        }

        // 4. 清除 dirty 标志
        for (shard_idx, page_id) in &page_ids_to_clear {
            let shard_guard = self.shards[*shard_idx].lock();
            let shard = &*shard_guard;
            if let Some(entry) = shard.lookup.get(page_id) {
                entry
                    .dirty
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }

        // 5. 更新统计
        self.stats.lock().flush_count += flushed as u64;

        Ok(flushed)
    }

    /// 刷盘单页（同步）
    #[instrument(skip(self), fields(page_id))]
    pub fn flush_page(&self, page_id: u32) -> Result<(), BufferError> {
        let shard_idx = self.shard_for(page_id);
        let page_clone = {
            let shard_guard = self.shards[shard_idx].lock();
            let shard = &*shard_guard;
            let entry = shard
                .lookup
                .get(&page_id)
                .ok_or(BufferError::PageNotFound { page_id })?;
            if !entry.dirty.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(()); // 非脏页，无需刷盘
            }
            entry.page.clone()
        };

        // 写入 DWB（如果启用）
        {
            let dwb_guard = self.doublewrite.lock();
            if let Some(dwb) = dwb_guard.as_ref() {
                dwb.write_pages(std::slice::from_ref(&page_clone))?;
            }
        }

        // 写入实际存储
        self.writer.write_page(&page_clone)?;

        // 清除 dirty 标志
        let shard_guard = self.shards[shard_idx].lock();
        let shard = &*shard_guard;
        if let Some(entry) = shard.lookup.get(&page_id) {
            entry
                .dirty
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }

        self.stats.lock().flush_count += 1;
        Ok(())
    }

    /// 启动异步刷盘线程
    ///
    /// 每 `interval_ms` 毫秒自动 flush_all
    pub fn start_flush_worker(
        self: &std::sync::Arc<Self>,
        interval_ms: u64,
    ) -> Result<(), BufferError> {
        let mut handle_guard = self.flush_handle.lock();
        if handle_guard.is_some() {
            return Err(BufferError::FlushWorkerRunning);
        }
        self.flush_stop
            .store(false, std::sync::atomic::Ordering::SeqCst);

        let pool = self.clone();
        let handle = std::thread::spawn(move || {
            while !pool.flush_stop.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(interval_ms));
                if pool.flush_stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                // 忽略刷盘错误（崩溃模拟时 write_page 会失败）
                let _ = pool.flush_all();
            }
        });
        *handle_guard = Some(handle);
        Ok(())
    }

    /// 停止异步刷盘线程
    pub fn stop_flush_worker(&self) -> Result<(), BufferError> {
        self.flush_stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut handle_guard = self.flush_handle.lock();
        if let Some(handle) = handle_guard.take() {
            // 释放锁再 join 避免死锁
            drop(handle_guard);
            let _ = handle.join();
        }
        Ok(())
    }

    /// 检查某页是否在缓冲池中
    pub fn contains(&self, page_id: u32) -> bool {
        let shard_idx = self.shard_for(page_id);
        let shard_guard = self.shards[shard_idx].lock();
        let shard = shard_guard;
        shard.lookup.contains_key(&page_id)
    }

    /// 获取 Doublewrite Buffer 的锁（用于崩溃恢复扫描）
    ///
    /// 返回 `MutexGuard<Option<DoublewriteBuffer>>`，调用方通过 `.as_ref()`
    /// 判断是否启用 DWB。未启用 DWB 时（`with_writer` 构造）返回 `None`
    pub fn lock_doublewrite(&self) -> parking_lot::MutexGuard<'_, Option<DoublewriteBuffer>> {
        self.doublewrite.lock()
    }

    /// 获取某分片当前缓存的页数
    pub fn len(&self, shard_idx: usize) -> usize {
        let shard_guard = self.shards[shard_idx].lock();
        shard_guard.lookup.len()
    }

    /// 获取缓冲池总缓存页数
    pub fn total_len(&self) -> usize {
        let mut total = 0;
        for shard in &self.shards {
            total += shard.lock().lookup.len();
        }
        total
    }

    /// 获取统计信息快照
    pub fn stats(&self) -> BufferPoolStats {
        *self.stats.lock()
    }

    /// 淘汰 LRU 尾部第一个 pin_count == 0 的页
    ///
    /// 必须已持有 shard 锁。脏页在淘汰前会先刷盘（避免数据丢失）
    #[instrument(skip(self, shard), fields(evicted_page_id, was_dirty), level = "trace")]
    fn evict_one_locked(&self, shard: &mut BufferPoolShard) -> Result<(), BufferError> {
        // 从 LRU 尾部向前扫描，找第一个 pin_count == 0 的页
        let mut evict_idx = None;
        for (idx, &page_id) in shard.lru_list.iter().enumerate().rev() {
            if let Some(entry) = shard.lookup.get(&page_id) {
                if entry.pin_count.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                    evict_idx = Some(idx);
                    break;
                }
            }
        }

        let idx = match evict_idx {
            Some(i) => i,
            None => {
                warn!("evict_one_locked: no evictable pages (all pinned)");
                return Err(BufferError::NoEvictablePages);
            }
        };
        let page_id = shard.lru_list.remove(idx).unwrap();
        tracing::Span::current().record("evicted_page_id", page_id);
        // 取出被淘汰的 entry，若脏则刷盘
        if let Some(entry) = shard.lookup.remove(&page_id) {
            let was_dirty = entry.dirty.load(std::sync::atomic::Ordering::SeqCst);
            tracing::Span::current().record("was_dirty", was_dirty);
            if was_dirty {
                trace!(page_id, "evicting dirty page, flushing before eviction");
                // 脏页淘汰前先刷盘（避免数据丢失）
                // 注意：这里不使用 DWB（淘汰是低频操作，直接写入即可）
                if let Err(e) = self.writer.write_page(&entry.page) {
                    warn!(page_id, error = %e, "evict_one_locked: flush before eviction failed");
                    return Err(e);
                }
                // 同样计入 flush_count 统计（淘汰时的刷盘也是一次 flush）
                self.stats.lock().flush_count += 1;
            } else {
                trace!(page_id, "evicting clean page");
            }
        }
        self.stats.lock().evictions += 1;
        Ok(())
    }
}

// =====================================================================
//  NoopPageWriter — 默认空写入器（向后兼容 Phase 0.9）
// =====================================================================

/// 空写入器：write_page 不做任何操作（Phase 0.9 兼容模式）
///
/// 用于不需要持久化的场景（纯内存测试）
pub struct NoopPageWriter;

impl PageWriter for NoopPageWriter {
    fn write_page(&self, _page: &Page) -> Result<(), BufferError> {
        Ok(())
    }
}

// =====================================================================
//  P0-STORE-2：FilePageLoader / FilePageWriter — 文件后端
//  （将 BufferPool 接入运行时持久化路径）
// =====================================================================

use crate::page::PAGE_SIZE;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// 文件页加载器 — 从单个文件按 `page_id * PAGE_SIZE` 偏移读取页
///
/// **设计**：所有页连续存储在一个文件中，page_id 直接映射到文件偏移。
/// 启动时打开已有文件；若 page_id 对应偏移超出文件末尾或读取到全零字节，
/// 返回 `PageNotFound`（视为该页从未写入）。
///
/// **线程安全**：内部用 `Mutex<File>` 保护，所有 read+seek 串行化。
pub struct FilePageLoader {
    file: Mutex<File>,
}

impl FilePageLoader {
    /// 打开已有文件（只读模式）
    ///
    /// 文件不存在则返回 IoError
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, BufferError> {
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|e| BufferError::IoError(format!("open loader failed: {e}")))?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }
}

impl PageLoader for FilePageLoader {
    fn load_page(&self, page_id: u32) -> Result<Page, BufferError> {
        let mut file = self.file.lock();
        let offset = (page_id as u64) * (PAGE_SIZE as u64);
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| BufferError::IoError(format!("seek failed: {e}")))?;
        let mut buf = [0u8; PAGE_SIZE];
        let n = file
            .read(&mut buf)
            .map_err(|e| BufferError::IoError(format!("read failed: {e}")))?;
        // 读取 0 字节表示文件末尾，该页从未写入
        if n == 0 {
            return Err(BufferError::PageNotFound { page_id });
        }
        // 读取不足一页：若是全零，视为未写入；否则尝试解码（容忍尾部零填充）
        if n < PAGE_SIZE {
            // 检查读取到的部分是否全零
            if buf[..n].iter().all(|&b| b == 0) {
                return Err(BufferError::PageNotFound { page_id });
            }
            // 非全零但不完整：报错（数据损坏）
            return Err(BufferError::IoError(format!(
                "short read: page_id={page_id} expected {PAGE_SIZE} got {n}"
            )));
        }
        // 全零页视为未写入
        if buf.iter().all(|&b| b == 0) {
            return Err(BufferError::PageNotFound { page_id });
        }
        Page::decode(&buf).map_err(BufferError::PageError)
    }
}

/// 文件页写入器 — 按 `page_id * PAGE_SIZE` 偏移写入页到单个文件
///
/// **设计**：write_page 完成后立即 flush+sync 到磁盘，保证崩溃一致性。
/// 文件不存在时自动创建（create(true)）。
///
/// **线程安全**：内部用 `Mutex<File>` 保护，所有 write+seek+sync 串行化。
pub struct FilePageWriter {
    file: Mutex<File>,
}

impl FilePageWriter {
    /// 打开或创建文件（读写模式，create=true，不截断已有文件）
    ///
    /// 注意：`enable_persistence` 对已存在的表文件会复用本函数打开（load 场景），
    /// 因此**不能** truncate —— 截断会导致重启后旧数据全部丢失。
    #[allow(clippy::suspicious_open_options)]
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, BufferError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .map_err(|e| BufferError::IoError(format!("open writer failed: {e}")))?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }
}

impl PageWriter for FilePageWriter {
    fn write_page(&self, page: &Page) -> Result<(), BufferError> {
        let mut file = self.file.lock();
        let offset = (page.header.page_id as u64) * (PAGE_SIZE as u64);
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| BufferError::IoError(format!("seek failed: {e}")))?;
        let buf = page.encode();
        file.write_all(&buf)
            .map_err(|e| BufferError::IoError(format!("write failed: {e}")))?;
        file.sync_data()
            .map_err(|e| BufferError::IoError(format!("sync failed: {e}")))?;
        Ok(())
    }
}

// =====================================================================
//  DoublewriteBuffer — 双写缓冲区（Phase 0.11 完整实现）
// =====================================================================

/// 双写缓冲区内部条目：Page + 插入序号（用于 FIFO 淘汰）
struct DwbEntry {
    page: Page,
    /// 插入时的全局序号，FIFO 淘汰时移除序号最小的
    seq: u64,
}

/// 双写缓冲区：先将脏页写入 DWB，再写入实际存储
///
/// 崩溃恢复原理：
/// 1. flush_all 时，先将脏页批量写入 DWB（内存中）
/// 2. 再将脏页逐个写入实际存储（writer）
/// 3. 若步骤 2 中途崩溃，DWB 中仍有完整副本
/// 4. 重启后，从 DWB 恢复所有页到 writer
///
/// Phase 0.11 升级：
/// - FIFO 淘汰（基于序号，而非随机）
/// - 原子批量写入（一批要么全部写入，要么不写）
/// - 恢复扫描按 page_id 排序，保证确定性
pub struct DoublewriteBuffer {
    /// DWB 容量（最多缓存多少页）
    capacity: usize,
    /// 双写缓冲区：page_id → DwbEntry
    pages: Mutex<std::collections::HashMap<u32, DwbEntry>>,
    /// 全局序号计数器（FIFO 淘汰用）
    seq_counter: std::sync::atomic::AtomicU64,
    /// 写入计数（累计写入的页数）
    write_count: std::sync::atomic::AtomicU64,
    /// 淘汰计数（累计被 FIFO 淘汰的页数）
    evict_count: std::sync::atomic::AtomicU64,
}

impl DoublewriteBuffer {
    /// 创建 DWB
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            pages: Mutex::new(std::collections::HashMap::new()),
            seq_counter: std::sync::atomic::AtomicU64::new(0),
            write_count: std::sync::atomic::AtomicU64::new(0),
            evict_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// 写入一组页到 DWB（原子批量写入）
    ///
    /// 同一批次的所有页要么全部写入成功，要么全部不写（模拟原子性）
    /// 超过容量时按 FIFO 淘汰最早插入的页
    pub fn write_pages(&self, pages: &[Page]) -> Result<(), BufferError> {
        if pages.is_empty() {
            return Ok(());
        }
        let mut guard = self.pages.lock();

        // 1. 先分配所有序号（保证同批次序号连续）
        let mut seqs = Vec::with_capacity(pages.len());
        for _ in pages {
            let seq = self
                .seq_counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            seqs.push(seq);
        }

        // 2. 批量插入（覆盖已存在的 page_id）
        for (page, seq) in pages.iter().zip(seqs.iter()) {
            guard.insert(
                page.header.page_id,
                DwbEntry {
                    page: page.clone(),
                    seq: *seq,
                },
            );
            self.write_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        // 3. FIFO 淘汰：如果超过容量，移除 seq 最小的条目
        while guard.len() > self.capacity {
            // 找到 seq 最小的条目
            let min_key = guard.iter().min_by_key(|(_, e)| e.seq).map(|(k, _)| *k);
            if let Some(k) = min_key {
                guard.remove(&k);
                self.evict_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            } else {
                break;
            }
        }
        Ok(())
    }

    /// 从 DWB 读取一页（崩溃恢复时使用）
    pub fn get_page(&self, page_id: u32) -> Option<Page> {
        self.pages.lock().get(&page_id).map(|e| e.page.clone())
    }

    /// DWB 当前页数
    pub fn len(&self) -> usize {
        self.pages.lock().len()
    }

    /// DWB 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// DWB 容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 写入计数（累计）
    pub fn write_count(&self) -> u64 {
        self.write_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 淘汰计数（累计）
    pub fn evict_count(&self) -> u64 {
        self.evict_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 清空 DWB
    pub fn clear(&self) {
        self.pages.lock().clear();
    }

    /// 获取所有 page_id（按 page_id 升序，崩溃恢复扫描用）
    pub fn page_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.pages.lock().keys().copied().collect();
        ids.sort();
        ids
    }

    /// 崩溃恢复：将 DWB 中所有页写入 writer（按 page_id 升序，保证确定性）
    ///
    /// 返回恢复的页数
    pub fn recover_to_writer(&self, writer: &dyn PageWriter) -> Result<usize, BufferError> {
        let guard = self.pages.lock();
        // 按 page_id 升序排序，保证恢复顺序确定
        let mut entries: Vec<(u32, &DwbEntry)> = guard.iter().map(|(k, v)| (*k, v)).collect();
        entries.sort_by_key(|(k, _)| *k);
        let mut recovered = 0usize;
        for (_page_id, entry) in entries {
            writer.write_page(&entry.page)?;
            recovered += 1;
        }
        Ok(recovered)
    }

    /// 崩溃恢复并校验：将 DWB 中所有页写入 writer，并校验每页 checksum
    ///
    /// 返回 (恢复页数, checksum 校验失败的页数)
    pub fn recover_to_writer_with_checksum(
        &self,
        writer: &dyn PageWriter,
    ) -> Result<(usize, usize), BufferError> {
        let guard = self.pages.lock();
        let mut entries: Vec<(u32, &DwbEntry)> = guard.iter().map(|(k, v)| (*k, v)).collect();
        entries.sort_by_key(|(k, _)| *k);
        let mut recovered = 0usize;
        let mut checksum_failures = 0usize;
        for (_page_id, entry) in entries {
            // 校验 checksum（如果页设置了 checksum）
            if entry.page.header.checksum != 0 {
                let computed = entry.page.compute_checksum();
                if computed != entry.page.header.checksum {
                    checksum_failures += 1;
                    // 仍然写入（恢复优先于校验）
                }
            }
            writer.write_page(&entry.page)?;
            recovered += 1;
        }
        Ok((recovered, checksum_failures))
    }
}

// =====================================================================
//  PageGuard — RAII Pin 守卫
// =====================================================================

/// 页守卫：Drop 时自动 Unpin
///
/// 持有 BufferPool 的 Arc 引用，确保 Drop 时 BufferPool 仍存活
pub struct PageGuard {
    pool: std::sync::Arc<BufferPool>,
    page_id: u32,
    page: Page,
}

impl PageGuard {
    /// 获取页引用（只读）
    pub fn page(&self) -> &Page {
        &self.page
    }

    /// 获取页的可变引用（需要独占访问，调用方需保证无并发修改）
    pub fn page_mut(&mut self) -> &mut Page {
        &mut self.page
    }

    /// 获取 page_id
    pub fn page_id(&self) -> u32 {
        self.page_id
    }
}

impl Drop for PageGuard {
    fn drop(&mut self) {
        let _ = self.pool.unpin_page(self.page_id);
    }
}

impl BufferPool {
    /// 读取并 Pin 一个页，返回 PageGuard
    ///
    /// Drop PageGuard 时自动 Unpin
    pub fn read_page_pinned(
        self: &std::sync::Arc<Self>,
        page_id: u32,
    ) -> Result<PageGuard, BufferError> {
        let page = self.read_page(page_id)?;
        self.pin_page(page_id)?;
        Ok(PageGuard {
            pool: self.clone(),
            page_id,
            page,
        })
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::PageType;
    use std::sync::Arc;

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    fn make_loader_with_pages(page_ids: &[u32]) -> Arc<InMemoryPageLoader> {
        let loader = InMemoryPageLoader::new();
        for &pid in page_ids {
            loader.insert_blank(pid);
        }
        Arc::new(loader)
    }

    // -----------------------------------------------------------------
    //  BufferError 测试
    // -----------------------------------------------------------------

    #[test]
    fn buffer_error_display_messages() {
        assert_eq!(
            BufferError::NoEvictablePages.to_string(),
            "buffer pool is full: no evictable pages (all pinned)"
        );
        assert_eq!(
            BufferError::PageNotFound { page_id: 42 }.to_string(),
            "page 42 not found in buffer pool"
        );
        assert_eq!(
            BufferError::InvalidCapacity(0).to_string(),
            "capacity must be > 0, got 0"
        );
    }

    // -----------------------------------------------------------------
    //  BufferPool 创建测试
    // -----------------------------------------------------------------

    #[test]
    fn buffer_pool_new_zero_capacity_fails() {
        let loader = make_loader_with_pages(&[]);
        let result = BufferPool::new(0, loader);
        assert!(matches!(result, Err(BufferError::InvalidCapacity(0))));
    }

    #[test]
    fn buffer_pool_new_valid_capacity_succeeds() {
        let loader = make_loader_with_pages(&[]);
        let pool = BufferPool::new(100, loader).unwrap();
        assert_eq!(pool.total_len(), 0);
    }

    #[test]
    fn buffer_pool_capacity_distributes_across_shards() {
        let loader = make_loader_with_pages(&[]);
        // capacity=16 → 16 个分片，每分片 1 页
        let pool = BufferPool::new(16, loader).unwrap();
        for i in 0..SHARD_COUNT {
            assert_eq!(pool.len(i), 0, "shard {i} should be empty");
        }
    }

    #[test]
    fn buffer_pool_small_capacity_uses_fewer_shards() {
        let loader = make_loader_with_pages(&[0, 1, 2]);
        // capacity=2 → 2 个分片，每分片 1 页
        let pool = BufferPool::new(2, loader).unwrap();
        // 访问 0, 1, 2 — 应该触发淘汰
        // 0 % 2 = 0, 1 % 2 = 1, 2 % 2 = 0
        // 分片 0 容量 1，访问 0 后再访问 2 应淘汰 0
        pool.read_page(0).unwrap();
        pool.read_page(1).unwrap();
        assert!(pool.contains(0));
        assert!(pool.contains(1));
        pool.read_page(2).unwrap(); // 2 % 2 = 0，淘汰 0
        assert!(!pool.contains(0), "page 0 should be evicted");
        assert!(pool.contains(1));
        assert!(pool.contains(2));
    }

    // -----------------------------------------------------------------
    //  read_page 基本测试
    // -----------------------------------------------------------------

    #[test]
    fn buffer_pool_read_page_miss_loads_from_loader() {
        let loader = make_loader_with_pages(&[1, 2, 3]);
        let pool = BufferPool::new(10, loader).unwrap();

        let page = pool.read_page(1).unwrap();
        assert_eq!(page.header.page_id, 1);
        assert_eq!(page.header.page_type, PageType::Data);
        assert!(pool.contains(1));

        let stats = pool.stats();
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 0);
    }

    #[test]
    fn buffer_pool_read_page_hit_increments_hits() {
        let loader = make_loader_with_pages(&[1]);
        let pool = BufferPool::new(10, loader).unwrap();

        // 第一次：miss
        pool.read_page(1).unwrap();
        // 第二次：hit
        pool.read_page(1).unwrap();

        let stats = pool.stats();
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 1);
    }

    #[test]
    fn buffer_pool_read_page_not_in_loader_returns_error() {
        let loader = make_loader_with_pages(&[1, 2]);
        let pool = BufferPool::new(10, loader).unwrap();

        let result = pool.read_page(999);
        assert!(matches!(
            result,
            Err(BufferError::PageNotFound { page_id: 999 })
        ));
    }

    #[test]
    fn buffer_pool_read_multiple_pages() {
        let loader = make_loader_with_pages(&[0, 1, 2, 3, 4]);
        let pool = BufferPool::new(100, loader).unwrap();

        for pid in 0..5u32 {
            let page = pool.read_page(pid).unwrap();
            assert_eq!(page.header.page_id, pid);
        }

        assert_eq!(pool.total_len(), 5);
        let stats = pool.stats();
        assert_eq!(stats.misses, 5);
    }

    // -----------------------------------------------------------------
    //  LRU 淘汰测试
    // -----------------------------------------------------------------

    #[test]
    fn buffer_pool_evicts_lru_when_full() {
        // capacity=1 → 1 分片 1 页，访问 0 后访问 1 应淘汰 0
        let loader = make_loader_with_pages(&[0, 1]);
        let pool = BufferPool::new(1, loader).unwrap();

        pool.read_page(0).unwrap();
        pool.read_page(1).unwrap(); // 1 % 1 = 0，同分片，淘汰 0

        assert!(!pool.contains(0), "page 0 should have been evicted");
        assert!(pool.contains(1));

        let stats = pool.stats();
        assert_eq!(stats.evictions, 1);
    }

    #[test]
    fn buffer_pool_lru_order_updated_on_access() {
        // capacity=2 → 2 分片每分片 1 页
        // 0 % 2 = 0, 1 % 2 = 1, 2 % 2 = 0
        // 访问 0（分片0），访问 1（分片1），重新访问 0（命中），访问 2（分片0，淘汰 0）
        let loader = make_loader_with_pages(&[0, 1, 2]);
        let pool = BufferPool::new(2, loader).unwrap();

        pool.read_page(0).unwrap();
        pool.read_page(1).unwrap();

        // 重新访问 0，使其成为最近使用（分片 0 的 LRU 头部）
        pool.read_page(0).unwrap();

        // 访问 2（分片 0），应该淘汰 0（分片 0 唯一的页）
        pool.read_page(2).unwrap();

        assert!(!pool.contains(0), "page 0 should have been evicted");
        assert!(pool.contains(1), "page 1 should remain (different shard)");
        assert!(pool.contains(2));
    }

    #[test]
    fn buffer_pool_no_eviction_when_under_capacity() {
        let loader = make_loader_with_pages(&[0, 1, 2, 3]);
        let pool = BufferPool::new(10, loader).unwrap();

        for pid in 0..4u32 {
            pool.read_page(pid).unwrap();
        }

        assert_eq!(pool.total_len(), 4);
        let stats = pool.stats();
        assert_eq!(stats.evictions, 0);
    }

    #[test]
    fn buffer_pool_eviction_creates_space_for_new_page() {
        // capacity=3 → 3 分片每分片 1 页
        // 0%3=0, 1%3=1, 2%3=2, 3%3=0, 4%3=1, 5%3=2
        // 每个 pid 都会落入已满的分片，触发淘汰
        let loader = make_loader_with_pages(&[0, 1, 2, 3, 4, 5]);
        let pool = BufferPool::new(3, loader).unwrap();

        for pid in 0..6u32 {
            pool.read_page(pid).unwrap();
        }

        // 最终保留最后访问的 3, 4, 5
        assert_eq!(pool.total_len(), 3);
        assert!(pool.contains(3));
        assert!(pool.contains(4));
        assert!(pool.contains(5));
        let stats = pool.stats();
        assert_eq!(stats.evictions, 3);
    }

    // -----------------------------------------------------------------
    //  Pin/Unpin 测试
    // -----------------------------------------------------------------

    #[test]
    fn buffer_pool_pin_increments_count() {
        let loader = make_loader_with_pages(&[1]);
        let pool = BufferPool::new(10, loader).unwrap();

        pool.read_page(1).unwrap();
        assert_eq!(pool.pin_count(1).unwrap(), 0);

        let c1 = pool.pin_page(1).unwrap();
        assert_eq!(c1, 1);
        assert_eq!(pool.pin_count(1).unwrap(), 1);

        let c2 = pool.pin_page(1).unwrap();
        assert_eq!(c2, 2);
        assert_eq!(pool.pin_count(1).unwrap(), 2);
    }

    #[test]
    fn buffer_pool_unpin_decrements_count() {
        let loader = make_loader_with_pages(&[1]);
        let pool = BufferPool::new(10, loader).unwrap();

        pool.read_page(1).unwrap();
        pool.pin_page(1).unwrap();
        pool.pin_page(1).unwrap();
        assert_eq!(pool.pin_count(1).unwrap(), 2);

        let c1 = pool.unpin_page(1).unwrap();
        assert_eq!(c1, 1);

        let c2 = pool.unpin_page(1).unwrap();
        assert_eq!(c2, 0);
    }

    #[test]
    fn buffer_pool_unpin_underflow_returns_error() {
        let loader = make_loader_with_pages(&[1]);
        let pool = BufferPool::new(10, loader).unwrap();

        pool.read_page(1).unwrap();
        // pin_count = 0，直接 unpin 应该失败
        let result = pool.unpin_page(1);
        assert!(matches!(
            result,
            Err(BufferError::PinCountUnderflow { page_id: 1 })
        ));
    }

    #[test]
    fn buffer_pool_pin_not_in_pool_returns_error() {
        let loader = make_loader_with_pages(&[]);
        let pool = BufferPool::new(10, loader).unwrap();

        let result = pool.pin_page(999);
        assert!(matches!(
            result,
            Err(BufferError::PageNotFound { page_id: 999 })
        ));
    }

    #[test]
    fn buffer_pool_pin_prevents_eviction() {
        // capacity=1 → 1 分片 1 页
        let loader = make_loader_with_pages(&[0, 1]);
        let pool = BufferPool::new(1, loader).unwrap();

        pool.read_page(0).unwrap();
        pool.pin_page(0).unwrap(); // pin 0

        // 现在访问 1（同分片），应该无法淘汰 0（pinned），返回 NoEvictablePages
        let result = pool.read_page(1);
        assert!(
            matches!(result, Err(BufferError::NoEvictablePages)),
            "expected NoEvictablePages, got {result:?}"
        );

        // 0 仍在缓冲池
        assert!(pool.contains(0));
        assert_eq!(pool.pin_count(0).unwrap(), 1);
    }

    #[test]
    fn buffer_pool_unpin_allows_eviction() {
        // capacity=1 → 1 分片 1 页
        let loader = make_loader_with_pages(&[0, 1]);
        let pool = BufferPool::new(1, loader).unwrap();

        pool.read_page(0).unwrap();
        pool.pin_page(0).unwrap();

        // 此时无法淘汰
        assert!(pool.read_page(1).is_err());

        // unpin 后可以淘汰
        pool.unpin_page(0).unwrap();
        assert_eq!(pool.pin_count(0).unwrap(), 0);

        // 现在访问 1，应该成功淘汰 0
        let result = pool.read_page(1);
        assert!(result.is_ok(), "read_page(1) should succeed: {result:?}");
        assert!(!pool.contains(0));
        assert!(pool.contains(1));
    }

    #[test]
    fn buffer_pool_pin_multiple_then_unpin_all() {
        let loader = make_loader_with_pages(&[1]);
        let pool = BufferPool::new(10, loader).unwrap();

        pool.read_page(1).unwrap();
        pool.pin_page(1).unwrap();
        pool.pin_page(1).unwrap();
        pool.pin_page(1).unwrap();
        assert_eq!(pool.pin_count(1).unwrap(), 3);

        pool.unpin_page(1).unwrap();
        pool.unpin_page(1).unwrap();
        assert_eq!(pool.pin_count(1).unwrap(), 1);
        // 仍 pinned
        assert_eq!(pool.pin_count(1).unwrap(), 1);

        pool.unpin_page(1).unwrap();
        assert_eq!(pool.pin_count(1).unwrap(), 0);
    }

    // -----------------------------------------------------------------
    //  PageGuard 测试
    // -----------------------------------------------------------------

    #[test]
    fn page_guard_auto_unpin_on_drop() {
        let loader = make_loader_with_pages(&[1]);
        let pool = Arc::new(BufferPool::new(10, loader).unwrap());

        {
            let _guard = pool.read_page_pinned(1).unwrap();
            assert_eq!(pool.pin_count(1).unwrap(), 1);
        } // guard drop

        assert_eq!(pool.pin_count(1).unwrap(), 0);
    }

    #[test]
    fn page_guard_provides_page_access() {
        let loader = make_loader_with_pages(&[1]);
        let pool = Arc::new(BufferPool::new(10, loader).unwrap());

        let guard = pool.read_page_pinned(1).unwrap();
        assert_eq!(guard.page_id(), 1);
        assert_eq!(guard.page().header.page_id, 1);
        assert_eq!(guard.page().header.page_type, PageType::Data);
    }

    #[test]
    fn page_guard_multiple_concurrent() {
        let loader = make_loader_with_pages(&[1, 2, 3]);
        let pool = Arc::new(BufferPool::new(10, loader).unwrap());

        let g1 = pool.read_page_pinned(1).unwrap();
        let g2 = pool.read_page_pinned(2).unwrap();
        let g3 = pool.read_page_pinned(3).unwrap();

        assert_eq!(pool.pin_count(1).unwrap(), 1);
        assert_eq!(pool.pin_count(2).unwrap(), 1);
        assert_eq!(pool.pin_count(3).unwrap(), 1);

        drop(g1);
        assert_eq!(pool.pin_count(1).unwrap(), 0);
        assert_eq!(pool.pin_count(2).unwrap(), 1);

        drop(g2);
        drop(g3);
        assert_eq!(pool.pin_count(2).unwrap(), 0);
        assert_eq!(pool.pin_count(3).unwrap(), 0);
    }

    #[test]
    fn page_guard_pin_same_page_twice() {
        let loader = make_loader_with_pages(&[1]);
        let pool = Arc::new(BufferPool::new(10, loader).unwrap());

        let g1 = pool.read_page_pinned(1).unwrap();
        let g2 = pool.read_page_pinned(1).unwrap();
        assert_eq!(pool.pin_count(1).unwrap(), 2);

        drop(g1);
        assert_eq!(pool.pin_count(1).unwrap(), 1);

        drop(g2);
        assert_eq!(pool.pin_count(1).unwrap(), 0);
    }

    // -----------------------------------------------------------------
    //  命中率测试（Phase 0.9 验证标准：> 90%）
    // -----------------------------------------------------------------

    #[test]
    fn buffer_pool_hit_rate_above_90_percent() {
        // 容量 50，访问模式：先访问 50 个页（全部 miss），然后重复访问这 50 个页 10 次（全部 hit）
        // 总访问 550 次，50 miss + 500 hit = 91% 命中率 > 90%
        let page_ids: Vec<u32> = (0..50).collect();
        let loader = make_loader_with_pages(&page_ids);
        let pool = BufferPool::new(50, loader).unwrap();

        // 11 轮：第 1 轮全 miss，后 10 轮全 hit
        for _ in 0..11 {
            for &pid in &page_ids {
                pool.read_page(pid).unwrap();
            }
        }

        let stats = pool.stats();
        let total = stats.hits + stats.misses;
        let hit_rate = stats.hits as f64 / total as f64;
        assert!(
            hit_rate > 0.90,
            "hit rate {hit_rate:.4} should be > 0.90 (hits={}, misses={})",
            stats.hits,
            stats.misses
        );
    }

    #[test]
    fn buffer_pool_lru_hit_rate_with_locality() {
        // 局部性访问模式：80% 访问热页（10 个），20% 访问冷页（90 个）
        // 容量 20，热页应该常驻
        let page_ids: Vec<u32> = (0..100).collect();
        let loader = make_loader_with_pages(&page_ids);
        let pool = BufferPool::new(20, loader).unwrap();

        let mut rng_state = 0x1234_5678u64;
        let mut next_rand = || {
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            rng_state
        };

        for _ in 0..1000 {
            let pid = if next_rand() % 10 < 8 {
                // 80% 热页 0..10
                next_rand() as u32 % 10
            } else {
                // 20% 冷页 10..100
                10 + (next_rand() as u32 % 90)
            };
            pool.read_page(pid).unwrap();
        }

        let stats = pool.stats();
        let total = stats.hits + stats.misses;
        let hit_rate = stats.hits as f64 / total as f64;
        // 局部性访问应该有较高命中率（热页常驻）
        assert!(
            hit_rate > 0.50,
            "hit rate {hit_rate:.4} should be > 0.50 with 80/20 locality"
        );
    }

    // -----------------------------------------------------------------
    //  并发测试（Phase 0.9 验证标准：多个 shard 并发读）
    // -----------------------------------------------------------------

    #[test]
    fn buffer_pool_concurrent_reads_no_panic() {
        let page_ids: Vec<u32> = (0..100).collect();
        let loader = make_loader_with_pages(&page_ids);
        let pool = Arc::new(BufferPool::new(100, loader).unwrap());

        let mut handles = Vec::new();
        for tid in 0..8u32 {
            let pool_clone = pool.clone();
            let handle = std::thread::spawn(move || {
                for i in 0..1000u32 {
                    let pid = (tid * 100 + i) % 100;
                    let _ = pool_clone.read_page(pid).unwrap();
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        // 没有 panic 即通过
        let stats = pool.stats();
        assert!(stats.hits + stats.misses > 0);
    }

    #[test]
    fn buffer_pool_concurrent_pin_unpin_no_panic() {
        let loader = make_loader_with_pages(&[0, 1, 2, 3, 4]);
        let pool = Arc::new(BufferPool::new(10, loader).unwrap());

        // 预加载所有页
        for pid in 0..5u32 {
            pool.read_page(pid).unwrap();
        }

        let mut handles = Vec::new();
        for tid in 0..16u32 {
            let pool_clone = pool.clone();
            let handle = std::thread::spawn(move || {
                for i in 0..100u32 {
                    let pid = (tid + i) % 5;
                    let _ = pool_clone.pin_page(pid);
                    let _ = pool_clone.unpin_page(pid);
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        // 所有 pin 都应该被 unpin
        for pid in 0..5u32 {
            assert_eq!(
                pool.pin_count(pid).unwrap(),
                0,
                "page {pid} pin_count should be 0 after all threads done"
            );
        }
    }

    #[test]
    fn buffer_pool_concurrent_read_and_pin() {
        let page_ids: Vec<u32> = (0..50).collect();
        let loader = make_loader_with_pages(&page_ids);
        let pool = Arc::new(BufferPool::new(50, loader).unwrap());

        let mut handles = Vec::new();

        // 一半线程只读
        for tid in 0..4u32 {
            let pool_clone = pool.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..500u32 {
                    let pid = (tid * 13 + i) % 50;
                    let _ = pool_clone.read_page(pid).unwrap();
                }
            }));
        }

        // 一半线程 read + pin + unpin
        for tid in 0..4u32 {
            let pool_clone = pool.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..500u32 {
                    let pid = (tid * 17 + i) % 50;
                    let _g = pool_clone.read_page_pinned(pid);
                    // guard drop 时自动 unpin
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // 所有 pin 应该已清零
        for pid in 0..50u32 {
            if pool.contains(pid) {
                assert!(
                    pool.pin_count(pid).unwrap() <= 0,
                    "page {pid} should have 0 pin_count"
                );
            }
        }
    }

    // -----------------------------------------------------------------
    //  边界值测试
    // -----------------------------------------------------------------

    #[test]
    fn buffer_pool_capacity_one() {
        let loader = make_loader_with_pages(&[0, 1, 2]);
        // capacity=1 → 1 个分片，每分片 1 页
        let pool = BufferPool::new(1, loader).unwrap();

        pool.read_page(0).unwrap();
        assert_eq!(pool.total_len(), 1);

        // 访问 1，应该淘汰 0（同分片，容量 1）
        pool.read_page(1).unwrap();
        assert!(!pool.contains(0), "page 0 should be evicted");
        assert!(pool.contains(1));

        // 访问 2，淘汰 1
        pool.read_page(2).unwrap();
        assert!(!pool.contains(1));
        assert!(pool.contains(2));
    }

    #[test]
    fn buffer_pool_large_page_id() {
        let loader = make_loader_with_pages(&[u32::MAX, 1_000_000]);
        let pool = BufferPool::new(10, loader).unwrap();

        let p1 = pool.read_page(u32::MAX).unwrap();
        assert_eq!(p1.header.page_id, u32::MAX);

        let p2 = pool.read_page(1_000_000).unwrap();
        assert_eq!(p2.header.page_id, 1_000_000);
    }

    #[test]
    fn buffer_pool_repeated_eviction() {
        // capacity=1 → 1 分片 1 页，访问 10 个页应触发 9 次淘汰
        let loader = make_loader_with_pages(&(0..10).collect::<Vec<u32>>());
        let pool = BufferPool::new(1, loader).unwrap();

        for pid in 0..10u32 {
            pool.read_page(pid).unwrap();
        }

        let stats = pool.stats();
        assert_eq!(stats.misses, 10);
        assert_eq!(stats.evictions, 9, "should have 9 evictions");
    }

    #[test]
    fn buffer_pool_stats_track_all_operations() {
        let loader = make_loader_with_pages(&[0, 1, 2]);
        let pool = BufferPool::new(10, loader).unwrap();

        pool.read_page(0).unwrap(); // miss
        pool.read_page(0).unwrap(); // hit
        pool.read_page(1).unwrap(); // miss
        pool.pin_page(0).unwrap(); // pin
        pool.unpin_page(0).unwrap(); // unpin

        let stats = pool.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.pin_count, 1);
        assert_eq!(stats.unpin_count, 1);
    }

    // =================================================================
    //  Phase 0.10 测试：脏页跟踪 + 同步/异步刷盘 + 崩溃恢复
    // =================================================================

    // --- 脏页跟踪基础测试 ---

    #[test]
    fn phase_010_mark_dirty_sets_dirty_flag() {
        let loader = make_loader_with_pages(&[1, 2]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer).unwrap();

        pool.read_page(1).unwrap();
        assert!(!pool.is_dirty(1).unwrap());

        pool.mark_dirty(1).unwrap();
        assert!(pool.is_dirty(1).unwrap());

        // 多次 mark_dirty 应保持 dirty=true
        pool.mark_dirty(1).unwrap();
        assert!(pool.is_dirty(1).unwrap());
    }

    #[test]
    fn phase_010_mark_dirty_unknown_page_returns_error() {
        let loader = make_loader_with_pages(&[]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer).unwrap();

        let result = pool.mark_dirty(999);
        assert!(matches!(
            result,
            Err(BufferError::PageNotFound { page_id: 999 })
        ));
    }

    #[test]
    fn phase_010_is_dirty_unknown_page_returns_error() {
        let loader = make_loader_with_pages(&[]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer).unwrap();

        let result = pool.is_dirty(999);
        assert!(matches!(
            result,
            Err(BufferError::PageNotFound { page_id: 999 })
        ));
    }

    #[test]
    fn phase_010_write_page_marks_dirty() {
        let loader = make_loader_with_pages(&[1]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer).unwrap();

        pool.read_page(1).unwrap();
        assert!(!pool.is_dirty(1).unwrap());

        // 修改页内容
        let mut new_page = pool.read_page(1).unwrap();
        new_page.header.tuple_count = 42;
        pool.write_page(1, new_page).unwrap();

        // 应自动标记为脏
        assert!(pool.is_dirty(1).unwrap());
    }

    #[test]
    fn phase_010_write_page_unknown_returns_error() {
        let loader = make_loader_with_pages(&[]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer).unwrap();

        let page = Page::new(999, PageType::Data);
        let result = pool.write_page(999, page);
        assert!(matches!(
            result,
            Err(BufferError::PageNotFound { page_id: 999 })
        ));
    }

    // --- flush_page / flush_all 同步刷盘测试 ---

    #[test]
    fn phase_010_flush_page_clears_dirty_flag() {
        let loader = make_loader_with_pages(&[1]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer.clone()).unwrap();

        pool.read_page(1).unwrap();
        pool.mark_dirty(1).unwrap();
        assert!(pool.is_dirty(1).unwrap());

        pool.flush_page(1).unwrap();
        assert!(!pool.is_dirty(1).unwrap());

        // writer 应该收到一页
        assert_eq!(writer.write_count(), 1);
        assert!(writer.get_persisted(1).is_some());
    }

    #[test]
    fn phase_010_flush_page_not_dirty_is_noop() {
        let loader = make_loader_with_pages(&[1]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer.clone()).unwrap();

        pool.read_page(1).unwrap();
        // 未标记脏页，flush 应该是 no-op
        pool.flush_page(1).unwrap();
        assert_eq!(writer.write_count(), 0);
    }

    #[test]
    fn phase_010_flush_all_clears_all_dirty_flags() {
        let loader = make_loader_with_pages(&[0, 1, 2, 3, 4]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer.clone()).unwrap();

        // 加载并标记 5 个脏页
        for pid in 0..5u32 {
            pool.read_page(pid).unwrap();
            pool.mark_dirty(pid).unwrap();
            assert!(pool.is_dirty(pid).unwrap());
        }

        let flushed = pool.flush_all().unwrap();
        assert_eq!(flushed, 5, "should flush 5 dirty pages");
        assert_eq!(writer.write_count(), 5);

        // 所有 dirty 标志应该被清除
        for pid in 0..5u32 {
            assert!(
                !pool.is_dirty(pid).unwrap(),
                "page {pid} should not be dirty after flush_all"
            );
        }

        let stats = pool.stats();
        assert_eq!(stats.flush_count, 5);
    }

    #[test]
    fn phase_010_flush_all_no_dirty_returns_zero() {
        let loader = make_loader_with_pages(&[1, 2]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer.clone()).unwrap();

        pool.read_page(1).unwrap();
        pool.read_page(2).unwrap();
        // 无脏页
        let flushed = pool.flush_all().unwrap();
        assert_eq!(flushed, 0);
        assert_eq!(writer.write_count(), 0);
    }

    #[test]
    fn phase_010_flush_all_persists_page_content() {
        // 验证刷盘后实际存储中的页内容与缓冲池中一致
        let loader = make_loader_with_pages(&[1]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer.clone()).unwrap();

        pool.read_page(1).unwrap();
        // 修改页内容
        let mut modified = pool.read_page(1).unwrap();
        modified.header.tuple_count = 123;
        pool.write_page(1, modified).unwrap();

        pool.flush_all().unwrap();

        let persisted = writer.get_persisted(1).expect("page 1 should be persisted");
        assert_eq!(persisted.header.tuple_count, 123);
        assert_eq!(persisted.header.page_id, 1);
    }

    #[test]
    fn phase_010_flush_all_with_dwb_writes_to_both() {
        // 启用 Doublewrite Buffer：flush 时应同时写入 DWB 和实际存储
        let loader = make_loader_with_pages(&[1, 2, 3]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_doublewrite(10, loader, writer.clone(), 100).unwrap();

        for pid in 1..=3u32 {
            pool.read_page(pid).unwrap();
            pool.mark_dirty(pid).unwrap();
        }

        let flushed = pool.flush_all().unwrap();
        assert_eq!(flushed, 3);
        assert_eq!(writer.write_count(), 3, "should write 3 to writer");

        // DWB 应该也包含这 3 页
        let dwb = pool.doublewrite.lock();
        let dwb = dwb.as_ref().unwrap();
        assert_eq!(dwb.len(), 3);
        assert_eq!(dwb.write_count(), 3);
        assert!(dwb.get_page(1).is_some());
        assert!(dwb.get_page(2).is_some());
        assert!(dwb.get_page(3).is_some());
    }

    // --- 异步刷盘线程测试 ---

    #[test]
    fn phase_010_start_flush_worker_runs_periodically() {
        let loader = make_loader_with_pages(&[1, 2]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = Arc::new(BufferPool::with_writer(10, loader, writer.clone()).unwrap());

        // 启动异步刷盘线程，每 10ms 触发一次
        pool.start_flush_worker(10).unwrap();

        // 标记脏页
        pool.read_page(1).unwrap();
        pool.read_page(2).unwrap();
        pool.mark_dirty(1).unwrap();
        pool.mark_dirty(2).unwrap();

        // 等待异步线程触发几次刷盘
        std::thread::sleep(std::time::Duration::from_millis(100));

        // 停止线程
        pool.stop_flush_worker().unwrap();

        // writer 应该收到至少一次刷盘
        assert!(
            writer.write_count() >= 2,
            "async flush should have written at least 2 pages, got {}",
            writer.write_count()
        );

        // dirty 标志应被清除
        assert!(!pool.is_dirty(1).unwrap());
        assert!(!pool.is_dirty(2).unwrap());
    }

    #[test]
    fn phase_010_start_flush_worker_twice_returns_error() {
        let loader = make_loader_with_pages(&[]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = Arc::new(BufferPool::with_writer(10, loader, writer).unwrap());

        pool.start_flush_worker(100).unwrap();
        let result = pool.start_flush_worker(100);
        assert!(matches!(result, Err(BufferError::FlushWorkerRunning)));

        pool.stop_flush_worker().unwrap();
    }

    #[test]
    fn phase_010_stop_flush_worker_idempotent() {
        let loader = make_loader_with_pages(&[]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = Arc::new(BufferPool::with_writer(10, loader, writer).unwrap());

        // 未启动就停止 — 应该是 no-op
        pool.stop_flush_worker().unwrap();

        pool.start_flush_worker(100).unwrap();
        pool.stop_flush_worker().unwrap();
        // 再次停止 — 应该是 no-op
        pool.stop_flush_worker().unwrap();
    }

    // --- NoopPageWriter 测试 ---

    #[test]
    fn phase_010_noop_page_writer_does_nothing() {
        let loader = make_loader_with_pages(&[1]);
        // BufferPool::new 使用 NoopPageWriter
        let pool = BufferPool::new(10, loader).unwrap();

        pool.read_page(1).unwrap();
        pool.mark_dirty(1).unwrap();
        // flush 应该成功但不会真正写入（Noop）
        let flushed = pool.flush_all().unwrap();
        assert_eq!(flushed, 1);
        assert!(!pool.is_dirty(1).unwrap());
    }

    // --- Dirty 页淘汰测试 ---

    #[test]
    fn phase_010_dirty_page_eviction_flushes_before_evict() {
        // 脏页被淘汰前应该先刷盘
        let loader = make_loader_with_pages(&[0, 1]);
        let writer = Arc::new(InMemoryPageWriter::new());
        // capacity=1 → 1 分片 1 页
        let pool = BufferPool::with_writer(1, loader, writer.clone()).unwrap();

        pool.read_page(0).unwrap();
        pool.mark_dirty(0).unwrap();
        assert!(pool.is_dirty(0).unwrap());

        // 访问 1，应该淘汰 0，且 0 在淘汰前被刷盘
        pool.read_page(1).unwrap();

        // writer 应该收到 1 次写入（淘汰时的脏页刷盘）
        assert_eq!(
            writer.write_count(),
            1,
            "dirty page should be flushed before eviction"
        );
        assert!(
            writer.get_persisted(0).is_some(),
            "page 0 should be persisted before eviction"
        );
    }

    // --- DoublewriteBuffer 单元测试 ---

    #[test]
    fn phase_010_dwb_basic_write_and_get() {
        let dwb = DoublewriteBuffer::new(100);
        assert_eq!(dwb.capacity(), 100);
        assert!(dwb.is_empty());

        let page1 = Page::new(1, PageType::Data);
        let page2 = Page::new(2, PageType::Data);
        dwb.write_pages(&[page1, page2]).unwrap();

        assert_eq!(dwb.len(), 2);
        assert!(!dwb.is_empty());
        assert_eq!(dwb.write_count(), 2);
        assert!(dwb.get_page(1).is_some());
        assert!(dwb.get_page(2).is_some());
        assert!(dwb.get_page(999).is_none());
    }

    #[test]
    fn phase_010_dwb_capacity_eviction() {
        let dwb = DoublewriteBuffer::new(2);
        let p1 = Page::new(1, PageType::Data);
        let p2 = Page::new(2, PageType::Data);
        let p3 = Page::new(3, PageType::Data);

        dwb.write_pages(&[p1, p2]).unwrap();
        assert_eq!(dwb.len(), 2);

        // 写入 p3 应该触发淘汰（容量 2）
        dwb.write_pages(&[p3]).unwrap();
        assert_eq!(dwb.len(), 2, "DWB should maintain capacity 2");
        assert_eq!(dwb.write_count(), 3);
    }

    #[test]
    fn phase_010_dwb_clear() {
        let dwb = DoublewriteBuffer::new(100);
        dwb.write_pages(&[Page::new(1, PageType::Data)]).unwrap();
        assert_eq!(dwb.len(), 1);

        dwb.clear();
        assert!(dwb.is_empty());
    }

    #[test]
    fn phase_010_dwb_page_ids() {
        let dwb = DoublewriteBuffer::new(100);
        dwb.write_pages(&[Page::new(10, PageType::Data), Page::new(20, PageType::Data)])
            .unwrap();

        let mut ids = dwb.page_ids();
        ids.sort();
        assert_eq!(ids, vec![10, 20]);
    }

    #[test]
    fn phase_010_dwb_recover_to_writer() {
        // DWB 恢复：将 DWB 中所有页写入 writer
        let dwb = DoublewriteBuffer::new(100);
        let p1 = Page::new(1, PageType::Data);
        let p2 = Page::new(2, PageType::Data);
        dwb.write_pages(&[p1, p2]).unwrap();

        let writer = InMemoryPageWriter::new();
        let recovered = dwb.recover_to_writer(&writer).unwrap();
        assert_eq!(recovered, 2);
        assert_eq!(writer.write_count(), 2);
        assert!(writer.get_persisted(1).is_some());
        assert!(writer.get_persisted(2).is_some());
    }

    // =================================================================
    //  Phase 0.10 集成测试：10000 脏页刷盘 + 5000 页崩溃恢复
    // =================================================================

    #[test]
    fn phase_010_integration_10000_dirty_pages_flush() {
        // 验证标准：标记 10000 页为脏 → 触发刷盘 → 验证磁盘文件数据一致
        const NUM_PAGES: u32 = 10_000;

        let loader = InMemoryPageLoader::new();
        // 预生成 10000 个空白数据页
        for pid in 0..NUM_PAGES {
            loader.insert_blank(pid);
        }
        let loader = Arc::new(loader);
        let writer = Arc::new(InMemoryPageWriter::new());
        // 缓冲池容量 200，远小于 10000，会触发淘汰
        let pool = BufferPool::with_writer(200, loader, writer.clone()).unwrap();

        // 1. 逐个读取并修改 10000 页（每个页设置不同的 tuple_count）
        for pid in 0..NUM_PAGES {
            pool.read_page(pid).unwrap();
            let mut modified = pool.read_page(pid).unwrap();
            // 用 page_id 作为 tuple_count，便于后续验证
            modified.header.tuple_count = (pid & 0xFFFF) as u16;
            pool.write_page(pid, modified).unwrap();

            // 每 1000 页触发一次 flush，避免淘汰时频繁刷盘
            if (pid + 1) % 1000 == 0 {
                let _ = pool.flush_all().unwrap();
            }
        }

        // 2. 最终 flush_all
        let final_flushed = pool.flush_all().unwrap();
        // 由于中途已 flush，最终 flush 的脏页数应该为 0 或很少
        assert!(
            final_flushed < 1000,
            "final flush should be small (most pages already flushed), got {final_flushed}"
        );

        // 3. 验证 writer 中所有 10000 页内容正确
        assert_eq!(
            writer.len(),
            NUM_PAGES as usize,
            "all 10000 pages should be persisted"
        );

        for pid in 0..NUM_PAGES {
            let persisted = writer
                .get_persisted(pid)
                .unwrap_or_else(|| panic!("page {pid} should be persisted"));
            assert_eq!(
                persisted.header.page_id, pid,
                "page_id mismatch for page {pid}"
            );
            assert_eq!(
                persisted.header.tuple_count,
                (pid & 0xFFFF) as u16,
                "tuple_count mismatch for page {pid}"
            );
        }

        // 4. 验证统计
        let stats = pool.stats();
        assert!(
            stats.flush_count >= NUM_PAGES as u64,
            "flush_count {} should >= {NUM_PAGES}",
            stats.flush_count
        );
        assert!(
            stats.evictions > 0,
            "should have evictions due to small capacity"
        );
    }

    #[test]
    fn phase_010_integration_5000_pages_crash_recovery_with_dwb() {
        // 验证标准：写入 5000 页后模拟崩溃 → 重启 → doublewrite buffer 恢复
        // 流程：
        // 1. 启用 DWB 的缓冲池写入 5000 页
        // 2. 在写入到一半时（约 2500 页）触发崩溃（writer.crash()）
        // 3. flush_all 会失败（write_page 返回 WriterError）
        // 4. 此时 DWB 中应该有所有已尝试刷盘的页
        // 5. 重启：新 writer + 从 DWB recover_to_writer
        // 6. 验证 DWB 中所有页都能正确恢复到 writer

        const NUM_PAGES: u32 = 5000;
        const CRASH_AT: u32 = 2500;

        let loader = InMemoryPageLoader::new();
        for pid in 0..NUM_PAGES {
            loader.insert_blank(pid);
        }
        let loader = Arc::new(loader);
        let writer = Arc::new(InMemoryPageWriter::new());
        // 启用 DWB，容量足够容纳全部 5000 页
        let pool = BufferPool::with_doublewrite(5000, loader, writer.clone(), 10000).unwrap();

        // 1. 加载并修改所有 5000 页
        for pid in 0..NUM_PAGES {
            pool.read_page(pid).unwrap();
            let mut modified = pool.read_page(pid).unwrap();
            modified.header.tuple_count = (pid & 0xFFFF) as u16;
            modified.header.lsn = pid as u64;
            pool.write_page(pid, modified).unwrap();
        }

        // 2. 触发崩溃（在 writer 写入到约一半时崩溃）
        //    先让 writer 处理一部分写入，然后 crash
        //    方案：手动调用 flush_all，让它在写入过程中崩溃
        //    但是 InMemoryPageWriter 是全或无 — 一旦 crash，所有 write_page 失败
        //    所以我们在 flush 之前先 crash，flush 应该返回 WriterError
        //    但 DWB 已经先写入了，所以 DWB 中应该有所有页

        writer.crash();
        assert!(writer.is_crashed());

        // 3. flush_all 应该失败（因为 writer 已崩溃）
        //    但 DWB 应该已经被写入（DWB 在 writer 之前写入）
        let flush_result = pool.flush_all();
        assert!(
            flush_result.is_err(),
            "flush_all should fail when writer is crashed"
        );

        // 4. 验证 DWB 中包含了所有 5000 页
        {
            let dwb_guard = pool.doublewrite.lock();
            let dwb = dwb_guard.as_ref().expect("DWB should be enabled");
            assert_eq!(
                dwb.len(),
                NUM_PAGES as usize,
                "DWB should contain all {} pages, got {}",
                NUM_PAGES,
                dwb.len()
            );
            assert_eq!(dwb.write_count(), NUM_PAGES as u64);
        }

        // 5. 模拟崩溃恢复：创建新的 writer，从 DWB 恢复
        let new_writer = InMemoryPageWriter::new();
        let recovered_count = {
            let dwb_guard = pool.doublewrite.lock();
            let dwb = dwb_guard.as_ref().unwrap();
            dwb.recover_to_writer(&new_writer).unwrap()
        };
        assert_eq!(
            recovered_count, NUM_PAGES as usize,
            "should recover all {NUM_PAGES} pages from DWB"
        );

        // 6. 验证恢复后的数据完整性
        for pid in 0..NUM_PAGES {
            let persisted = new_writer
                .get_persisted(pid)
                .unwrap_or_else(|| panic!("page {pid} should be recovered"));
            assert_eq!(persisted.header.page_id, pid);
            assert_eq!(persisted.header.tuple_count, (pid & 0xFFFF) as u16);
            assert_eq!(persisted.header.lsn, pid as u64);
        }

        // CRASH_AT 仅作为文档说明，实际崩溃发生在 flush_all 调用 writer 之前
        let _ = CRASH_AT;
    }

    #[test]
    fn phase_010_integration_crash_recovery_partial_flush() {
        // 边界场景：部分页已刷盘成功，部分未刷盘时崩溃
        // 验证 DWB 中包含所有脏页，恢复时覆盖已刷盘的部分
        const NUM_PAGES: u32 = 100;

        let loader = InMemoryPageLoader::new();
        for pid in 0..NUM_PAGES {
            loader.insert_blank(pid);
        }
        let loader = Arc::new(loader);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_doublewrite(100, loader, writer.clone(), 1000).unwrap();

        // 加载并修改 100 页
        for pid in 0..NUM_PAGES {
            pool.read_page(pid).unwrap();
            let mut modified = pool.read_page(pid).unwrap();
            modified.header.tuple_count = (pid + 1) as u16;
            pool.write_page(pid, modified).unwrap();
        }

        // 第一次 flush 成功（无崩溃）
        let flushed1 = pool.flush_all().unwrap();
        assert_eq!(flushed1, NUM_PAGES as usize);
        assert_eq!(writer.write_count(), NUM_PAGES as u64);

        // 修改其中 50 页
        for pid in 0..50u32 {
            let mut modified = pool.read_page(pid).unwrap();
            modified.header.tuple_count = (pid + 1000) as u16;
            pool.write_page(pid, modified).unwrap();
        }

        // 崩溃前 flush_all — DWB 先写入 50 页（覆盖之前的版本），然后 writer 崩溃
        writer.crash();
        let _ = pool.flush_all(); // 失败

        // DWB 是 HashMap，第二次写入 50 页会覆盖第一次的对应条目
        // DWB.len() = 100（50 个被覆盖 + 50 个未修改的保留）
        {
            let dwb_guard = pool.doublewrite.lock();
            let dwb = dwb_guard.as_ref().unwrap();
            assert_eq!(dwb.len(), NUM_PAGES as usize);

            // 恢复：DWB 中所有 100 页都会被写入 new_writer
            let new_writer = InMemoryPageWriter::new();
            let recovered = dwb.recover_to_writer(&new_writer).unwrap();
            assert_eq!(recovered, NUM_PAGES as usize);

            // 验证前 50 页是最新版本（tuple_count = pid + 1000）
            for pid in 0..50u32 {
                let p = new_writer.get_persisted(pid).unwrap();
                assert_eq!(
                    p.header.tuple_count,
                    (pid + 1000) as u16,
                    "page {pid} should be latest version"
                );
            }
            // 后 50 页是第一次 flush 时的版本（tuple_count = pid + 1）
            for pid in 50..NUM_PAGES {
                let p = new_writer.get_persisted(pid).unwrap();
                assert_eq!(
                    p.header.tuple_count,
                    (pid + 1) as u16,
                    "page {pid} should be first version"
                );
            }
        }

        // 旧 writer 中所有 100 页都是第一次 flush 的版本
        // （第二次 flush 因为崩溃没有写入 writer）
        for pid in 0..NUM_PAGES {
            let p = writer.get_persisted(pid).unwrap();
            assert_eq!(p.header.tuple_count, (pid + 1) as u16);
        }
    }

    #[test]
    fn phase_010_integration_concurrent_flush_no_data_loss() {
        // 并发场景：多线程修改页 + 单线程异步刷盘
        // 验证不丢数据（每个 page_id 至少被刷盘一次）
        const NUM_THREADS: u32 = 8;
        const PAGES_PER_THREAD: u32 = 100;

        let loader = InMemoryPageLoader::new();
        for pid in 0..NUM_THREADS * PAGES_PER_THREAD {
            loader.insert_blank(pid);
        }
        let loader = Arc::new(loader);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = Arc::new(BufferPool::with_writer(200, loader, writer.clone()).unwrap());

        // 启动异步刷盘
        pool.start_flush_worker(5).unwrap();

        let mut handles = Vec::new();
        for tid in 0..NUM_THREADS {
            let pool_clone = pool.clone();
            handles.push(std::thread::spawn(move || {
                let base = tid * PAGES_PER_THREAD;
                for i in 0..PAGES_PER_THREAD {
                    let pid = base + i;
                    pool_clone.read_page(pid).unwrap();
                    let mut p = pool_clone.read_page(pid).unwrap();
                    p.header.tuple_count = (pid & 0xFFFF) as u16;
                    pool_clone.write_page(pid, p).unwrap();
                    pool_clone.mark_dirty(pid).unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // 等待异步刷盘完成
        std::thread::sleep(std::time::Duration::from_millis(200));
        pool.stop_flush_worker().unwrap();

        // 最终 flush_all 确保所有脏页刷盘
        let _ = pool.flush_all().unwrap();

        // 验证：所有页都应该已持久化
        let total_pages = (NUM_THREADS * PAGES_PER_THREAD) as usize;
        assert_eq!(
            writer.len(),
            total_pages,
            "all {} pages should be persisted, got {}",
            total_pages,
            writer.len()
        );

        for pid in 0..(NUM_THREADS * PAGES_PER_THREAD) {
            let p = writer
                .get_persisted(pid)
                .unwrap_or_else(|| panic!("page {pid} should be persisted"));
            assert_eq!(p.header.page_id, pid);
        }
    }

    #[test]
    fn phase_010_integration_writer_error_propagates() {
        // writer 返回错误时，flush_all 应该返回错误
        let loader = make_loader_with_pages(&[1]);
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(10, loader, writer.clone()).unwrap();

        pool.read_page(1).unwrap();
        pool.mark_dirty(1).unwrap();

        // 触发崩溃
        writer.crash();
        let result = pool.flush_all();
        assert!(matches!(result, Err(BufferError::WriterError(_))));

        // 恢复后应该能正常刷盘
        writer.recover();
        let flushed = pool.flush_all().unwrap();
        assert_eq!(flushed, 1);
    }
}
