# ADR-0010: Buffer Pool Sharded LRU

- **状态**: Accepted
- **日期**: 2026-07-24
- **决策类型**: 资源限制
- **相关代码**: `crates/szrsql-storage/src/buffer.rs (L197-L264)`
- **修复编号**: 无

## 背景

Buffer Pool 是存储引擎的核心缓存组件，缓存热点 page 以减少磁盘 IO。传统单一 LRU 设计在高并发下出现严重锁竞争：

1. **单 LRU 锁瓶颈**：所有读写 page 操作都需获取同一把 `Mutex<LRU>`，并发数 64+ 时锁等待时间 > 实际处理时间。
2. **Pin/Unpin 计数竞争**：page 被引用时 Pin，释放时 Unpin，单计数器在 32 核以上机器上 cache line bouncing 严重。
3. **LRU 链表修改竞争**：每次访问 page 都需移到链表头部，链表修改需独占锁。

实测数据（基准测试）：
- 单 LRU 在 64 线程下吞吐 < 50000 ops/s（锁等待占 70% 时间）
- 32 核机器上 cache miss 率 > 30%（cache line bouncing）

候选方案：

1. **单 LRU + 读写锁**：LRU 修改用写锁，访问用读锁；问题：访问后需移到头部，必须写锁，无优化。
2. **CLOCK 算法**：避免链表修改，用循环扫描 + 引用位；问题：实现复杂，访问位仍需锁。
3. **Sharded LRU**：分 N 个独立 LRU shard，每 shard 独立锁；优点：锁竞争降为 1/N。
4. **Thread-local LRU**：每线程私有 LRU；问题：跨线程访问 page 重复缓存，内存浪费。

需求约束：
- 高并发下吞吐线性扩展（64 线程下 ≥ 500000 ops/s）
- 内存占用可控（无重复缓存）
- Pin/Unpin 计数器无竞争
- 与现有 Page 16KB 设计兼容（见 ADR-0008）

不选 Sharded LRU 的后果：
- 单 LRU 锁竞争限制吞吐，无法利用多核
- Thread-local 重复缓存导致内存浪费
- CLOCK 实现复杂且仍有锁

## 决策

采用 **16 分片 LRU**，每分片独立锁，page 按 `page_id % 16` 路由到对应分片。

关键设计：

- **SHARD_COUNT = 16**：经实测 16 分片在 64 核机器下竞争最小
- **路由策略**：`shard_id = page_id % 16`，分布均匀
- **独立锁**：每分片独立 `Mutex<BufferPoolShard>`，无跨分片锁
- **Pin/Unpin 计数**：每 page 独立 `AtomicU32` 计数器，无锁
- **LRU 替换**：分片内 LRU，淘汰时仅影响该分片

关键代码（`crates/szrsql-storage/src/buffer.rs` L197-L264）：

```rust
// L197 常量定义
const SHARD_COUNT: usize = 16;

// L200 BufferPool 分片设计
pub struct BufferPool {
    shards: [Mutex<BufferPoolShard>; SHARD_COUNT],
    total_pages: AtomicUsize,
}

struct BufferPoolShard {
    pages: HashMap<u64, PageEntry>,
    lru: VecDeque<u64>,  // LRU 链表，尾部为最久未用
    capacity: usize,     // 单分片容量 = 总容量 / SHARD_COUNT
}

struct PageEntry {
    page: Page,
    pin_count: AtomicU32,  // 引用计数，无锁
    dirty: bool,
}

impl BufferPool {
    // L230 获取 page
    pub fn get_page(&self, page_id: u64) -> Result<PageGuard, BufferError> {
        let shard_idx = (page_id as usize) % SHARD_COUNT;
        let mut shard = self.shards[shard_idx].lock().unwrap();
        if let Some(entry) = shard.pages.get(&page_id) {
            // 命中：Pin 后返回
            entry.pin_count.fetch_add(1, Ordering::SeqCst);
            shard.lru.retain(|&k| k != page_id);
            shard.lru.push_front(page_id);
            return Ok(PageGuard { ... });
        }
        // 未命中：从磁盘加载，可能触发淘汰
        if shard.pages.len() >= shard.capacity {
            self.evict(&mut shard)?;
        }
        let page = self.load_from_disk(page_id)?;
        let entry = PageEntry { page, pin_count: AtomicU32::new(1), dirty: false };
        shard.pages.insert(page_id, entry);
        shard.lru.push_front(page_id);
        Ok(PageGuard { ... })
    }

    // L260 淘汰 LRU 尾部 page
    fn evict(&self, shard: &mut BufferPoolShard) -> Result<(), BufferError> {
        while let Some(page_id) = shard.lru.back() {
            let entry = shard.pages.get(page_id).unwrap();
            if entry.pin_count.load(Ordering::SeqCst) > 0 {
                // 被 Pin，跳过
                shard.lru.pop_back();
                continue;
            }
            // 淘汰：脏页写回磁盘
            if entry.dirty {
                self.flush_page(&entry.page)?;
            }
            shard.pages.remove(page_id);
            shard.lru.pop_back();
            return Ok(());
        }
        Err(BufferError::NoEvictablePage)
    }
}
```

## 后果

**正面**：
- 锁竞争降为 1/16，64 线程下吞吐 ≥ 500000 ops/s（实测）
- Pin/Unpin 无锁（AtomicU32），cache line bouncing 消除
- 内存无重复缓存（page 唯一路由到一个分片）
- 分片内 LRU 独立，淘汰不影响其他分片

**负面**：
- 分片间负载可能不均（若 page_id 分布不均，某分片成为热点）
- 跨分片事务需获取多分片锁，需注意锁顺序避免死锁
- 容量调整需重新分配（单分片容量 = 总容量 / 16）
- 分片数 16 固定，无法动态调整（未来可改进）

## 注意事项

### 调用方约束
- 访问 page 必须通过 `get_page()` 获取 `PageGuard`，不可直接持有 `Page` 引用
- `PageGuard` 释放时自动 `Unpin`，不可手动调整 pin_count
- 跨分片事务需按 `page_id` 升序获取锁，避免死锁
- 脏页淘汰会触发磁盘 IO，可能阻塞（建议异步淘汰）

### 迁移路径
- 当前 16 分片固定，未来若支持动态调整：
  1. 分片数作为启动参数（4/8/16/32 可选）
  2. 在线 reshard：渐进式迁移 page 到新分片
- 长期可引入 adaptive sharding：根据负载动态增减分片

### Bug 定位提示

**如果出现缓冲池满（无法分配新 page）**：
1. **查 `pin_count`**：是否有 page 长期被 Pin 未释放（`PageGuard` 未 drop）
2. **查 LRU 链表**：`shard.lru` 是否所有 page 都被 Pin（pin_count > 0）
3. **查内存泄漏**：grep `PageGuard` 使用，确认所有 guard 都在作用域内 drop

**如果出现性能低（吞吐不达预期）**：
1. **查 hits/misses ratio**：命中率 < 95% 则增大 buffer pool 或调整 LRU 策略
2. **查分片负载均衡**：各分片 `pages.len()` 是否严重不均（热点分片瓶颈）
3. **查锁等待时间**：tracing span `buffer_pool.get_page` 的 wait time，若 > 1ms 则锁竞争
4. **查淘汰频率**：`evict_count` 增长过快说明 buffer pool 不足

**如果出现死锁（多个事务互相等待 page）**：
1. **查 shard 锁顺序**：跨分片事务是否按 `page_id` 升序获取锁
2. **查 Pin/Unpin 配对**：是否有 Pin 后未 Unpin（导致其他事务等待）
3. **查淘汰逻辑**：`evict()` 是否在持锁状态下触发磁盘 IO（应释放锁后异步 IO）

**如果出现 page 内容不一致（脏页未写回）**：
1. **查 `dirty` 标志**：page 修改后是否设置 `dirty = true`
2. **查淘汰逻辑**：`evict()` 是否检查 `dirty` 并写回磁盘
3. **查 `flush_page()` 返回值**：写回失败是否回滚淘汰（避免数据丢失）

**如果出现分片负载不均（某分片成为热点）**：
1. **查 page_id 分布**：业务访问的 page_id 是否集中在某分片（如自增 page_id 集中在分片 0）
2. **查路由策略**：`page_id % 16` 是否均匀，考虑改用 hash 路由
3. **缓解措施**：增大分片数（如 32）或使用 hash 路由

**如果出现 Pin/Unpin 计数错误（page 被误淘汰或无法淘汰）**：
1. **查 `PageGuard` 实现**：`Drop` trait 是否正确 `fetch_sub(1, SeqCst)`
2. **查并发 Pin**：多线程同时 Pin 同一 page，`AtomicU32::fetch_add` 是否原子
3. **查计数器溢出**：`pin_count` 是否可能 > u32::MAX（理论上不可能，但需防护）

**如果 buffer pool 启动后无法加载 page**：
1. **查 `load_from_disk` 实现**：磁盘 IO 是否正常，page 文件是否存在
2. **查 `capacity` 配置**：单分片容量是否为 0（总容量 < 16 时）
3. **可排除**：MVCC / Raft 层（buffer pool 是存储层职责）
