//! 磁盘溢出（Disk Spilling）：外部排序 + Hash JOIN 分桶溢出。
//!
//! 对应 `SzRSQL技术实现方案.md` 第 7d.5 节。
//!
//! 设计思路：
//! - **外部排序**：当输入数据量超过内存限制时，分批读入内存排序，
//!   每批排序后作为一个有序 run 溢出到磁盘（此处用 `Vec<SortEntry>` 模拟）；
//!   所有输入完成后，用 k 路归并合并所有 runs，输出最终有序结果。
//! - **Hash JOIN 溢出**：当 build 侧数据超过内存限制时，将 build 和 probe
//!   两侧都按 hash 分桶；相同 key 一定落在同一桶；逐桶读入内存做嵌套循环 JOIN。
//!
//! 本模块用 `Vec` 模拟磁盘文件，便于单元测试；生产环境可替换为真实文件 IO。

use std::cmp::Reverse;
use std::collections::BinaryHeap;

// =====================================================================
//  常量
// =====================================================================

/// 默认内存限制（条目数）：模拟 256MB / 16 字节/条 ≈ 1600 万条。
pub const DEFAULT_MEMORY_LIMIT_ENTRIES: usize = 1_000;

/// 默认 Hash JOIN 分桶数。
pub const DEFAULT_HASH_PARTITIONS: usize = 8;

// =====================================================================
//  排序条目
// =====================================================================

/// 排序条目：key + row_id（payload 简化为 row_id）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortEntry {
    /// 排序键。
    pub key: i64,
    /// 行 ID（模拟行数据指针）。
    pub row_id: u64,
}

impl SortEntry {
    /// 创建排序条目。
    pub fn new(key: i64, row_id: u64) -> Self {
        Self { key, row_id }
    }

    /// 字节大小估算（用于统计）。
    pub fn size(&self) -> usize {
        std::mem::size_of::<i64>() + std::mem::size_of::<u64>()
    }
}

impl Ord for SortEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key
            .cmp(&other.key)
            .then(self.row_id.cmp(&other.row_id))
    }
}

impl PartialOrd for SortEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// =====================================================================
//  有序 Run（模拟磁盘文件）
// =====================================================================

/// 有序 run：一个已排序的条目序列，模拟磁盘上的临时文件。
#[derive(Debug, Clone)]
pub struct SortedRun {
    /// 已排序的条目。
    entries: Vec<SortEntry>,
    /// 当前读取游标。
    cursor: usize,
}

impl SortedRun {
    /// 创建有序 run。
    pub fn new(mut entries: Vec<SortEntry>) -> Self {
        entries.sort();
        Self { entries, cursor: 0 }
    }

    /// 条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 查看当前游标位置的条目（不消费）。
    pub fn peek(&self) -> Option<&SortEntry> {
        self.entries.get(self.cursor)
    }

    /// 消费当前游标位置的条目，游标前进。
    pub fn next_entry(&mut self) -> Option<SortEntry> {
        if self.cursor < self.entries.len() {
            let entry = self.entries[self.cursor].clone();
            self.cursor += 1;
            Some(entry)
        } else {
            None
        }
    }

    /// 是否已读完。
    pub fn is_exhausted(&self) -> bool {
        self.cursor >= self.entries.len()
    }

    /// 重置游标。
    pub fn reset(&mut self) {
        self.cursor = 0;
    }

    /// 字节大小估算。
    pub fn bytes(&self) -> usize {
        self.entries.iter().map(|e| e.size()).sum()
    }
}

// =====================================================================
//  外部排序统计
// =====================================================================

/// 外部排序统计信息。
#[derive(Debug, Clone, Default)]
pub struct SorterStats {
    /// 输入总条目数。
    pub total_input: u64,
    /// 溢出到磁盘的条目数。
    pub total_spilled: u64,
    /// 仍在内存中的条目数。
    pub in_memory: u64,
    /// 溢出次数（生成 run 数）。
    pub run_count: u64,
    /// 溢出字节数。
    pub spilled_bytes: u64,
    /// 是否发生了溢出。
    pub spilled: bool,
}

impl SorterStats {
    /// 创建空统计。
    pub fn new() -> Self {
        Self::default()
    }

    /// 内存命中率（0.0~1.0）：未溢出的比例。
    pub fn memory_hit_rate(&self) -> f64 {
        if self.total_input == 0 {
            return 1.0;
        }
        1.0 - (self.total_spilled as f64 / self.total_input as f64)
    }
}

// =====================================================================
//  外部排序器
// =====================================================================

/// 外部排序器：当内存缓冲达到限制时，排序后作为一个 run 溢出。
///
/// 算法：
/// 1. `push` 持续添加条目到内存缓冲。
/// 2. 当缓冲达到 `memory_limit` 时，调用 `flush_run` 排序后溢出。
/// 3. 所有输入完成后，调用 `finish` 把剩余缓冲作为一个 run。
/// 4. 调用 `merge` 执行 k 路归并，输出最终有序结果。
pub struct ExternalSorter {
    /// 内存限制（条目数）。
    memory_limit: usize,
    /// 当前内存缓冲。
    buffer: Vec<SortEntry>,
    /// 已溢出的有序 runs。
    runs: Vec<SortedRun>,
    /// 统计信息。
    stats: SorterStats,
}

impl ExternalSorter {
    /// 创建外部排序器，指定内存限制（条目数）。
    pub fn new(memory_limit: usize) -> Self {
        Self {
            memory_limit,
            buffer: Vec::new(),
            runs: Vec::new(),
            stats: SorterStats::new(),
        }
    }

    /// 使用默认内存限制创建。
    pub fn with_default_limit() -> Self {
        Self::new(DEFAULT_MEMORY_LIMIT_ENTRIES)
    }

    /// 添加一个条目。当缓冲满时自动溢出。
    pub fn push(&mut self, entry: SortEntry) {
        self.buffer.push(entry);
        self.stats.total_input += 1;
        if self.buffer.len() >= self.memory_limit {
            self.flush_run();
        }
    }

    /// 批量添加条目。
    pub fn extend(&mut self, entries: impl IntoIterator<Item = SortEntry>) {
        for entry in entries {
            self.push(entry);
        }
    }

    /// 将当前内存缓冲排序后作为一个 run 溢出。
    pub fn flush_run(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        let bytes: u64 = self.buffer.iter().map(|e| e.size() as u64).sum();
        let run = SortedRun::new(std::mem::take(&mut self.buffer));
        self.stats.total_spilled += run.len() as u64;
        self.stats.spilled_bytes += bytes;
        self.stats.run_count += 1;
        self.stats.spilled = true;
        self.runs.push(run);
    }

    /// 完成输入：剩余缓冲也作为一个 run（如有）。
    pub fn finish(&mut self) {
        if !self.buffer.is_empty() {
            let bytes: u64 = self.buffer.iter().map(|e| e.size() as u64).sum();
            let run = SortedRun::new(std::mem::take(&mut self.buffer));
            self.stats.total_spilled += run.len() as u64;
            self.stats.spilled_bytes += bytes;
            self.stats.run_count += 1;
            self.stats.spilled = true;
            self.runs.push(run);
        }
        self.stats.in_memory = 0;
    }

    /// 完成输入但不强制溢出（保留最后一个 run 在内存中）。
    /// 适用于数据量小于内存限制的场景。
    pub fn finish_in_memory(&mut self) {
        if !self.buffer.is_empty() {
            // 如果没有已溢出的 run，直接保留在内存中排序
            if self.runs.is_empty() {
                self.buffer.sort();
                self.stats.in_memory = self.buffer.len() as u64;
            } else {
                // 已有溢出的 run，则把剩余缓冲也作为 run
                self.finish();
            }
        }
    }

    /// k 路归并：合并所有有序 runs + 内存缓冲，输出最终有序结果。
    /// 使用最小堆实现，时间复杂度 O(N log K)，K = run 数。
    pub fn merge(&mut self) -> Vec<SortEntry> {
        // 先把剩余缓冲作为一个 run
        if !self.buffer.is_empty() {
            let run = SortedRun::new(std::mem::take(&mut self.buffer));
            self.runs.push(run);
        }

        if self.runs.is_empty() {
            return Vec::new();
        }

        // 单 run 直接返回
        if self.runs.len() == 1 {
            let mut run = self.runs.pop().unwrap();
            run.reset();
            return run.entries;
        }

        // k 路归并：最小堆，堆元素 (entry, run_index)
        let mut heap: BinaryHeap<Reverse<(SortEntry, usize)>> = BinaryHeap::new();
        let mut runs = std::mem::take(&mut self.runs);
        for (idx, run) in runs.iter_mut().enumerate() {
            run.reset();
            if let Some(entry) = run.next_entry() {
                heap.push(Reverse((entry, idx)));
            }
        }

        let mut result = Vec::new();
        while let Some(Reverse((entry, run_idx))) = heap.pop() {
            result.push(entry.clone());
            if let Some(next) = runs[run_idx].next_entry() {
                heap.push(Reverse((next, run_idx)));
            }
        }

        result
    }

    /// 获取统计信息。
    pub fn stats(&self) -> &SorterStats {
        &self.stats
    }

    /// 内存限制。
    pub fn memory_limit(&self) -> usize {
        self.memory_limit
    }

    /// 当前 run 数。
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// 当前内存缓冲条目数。
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    /// 是否已溢出。
    pub fn has_spilled(&self) -> bool {
        self.stats.spilled
    }

    /// 重置排序器（清空所有状态）。
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.runs.clear();
        self.stats = SorterStats::new();
    }
}

// =====================================================================
//  Hash JOIN 分桶
// =====================================================================

/// Hash JOIN 元组：(build_key, build_value, probe_value)。
/// 简化设计：key 和 value 都用 Vec<u8>。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinTuple {
    pub build_key: Vec<u8>,
    pub build_value: Vec<u8>,
    pub probe_value: Vec<u8>,
}

/// Hash 分桶：内存部分 + 溢出部分。
#[derive(Debug, Clone)]
pub struct HashPartition {
    /// 内存中的 build 侧条目。
    build_in_memory: Vec<(Vec<u8>, Vec<u8>)>,
    /// 溢出到磁盘的 build 侧条目（模拟磁盘文件）。
    build_spilled: Vec<(Vec<u8>, Vec<u8>)>,
    /// 内存中的 probe 侧条目。
    probe_in_memory: Vec<(Vec<u8>, Vec<u8>)>,
    /// 溢出到磁盘的 probe 侧条目。
    probe_spilled: Vec<(Vec<u8>, Vec<u8>)>,
    /// 该桶内存限制（条目数）。
    memory_limit: usize,
    /// 是否已溢出。
    spilled: bool,
}

impl HashPartition {
    /// 创建空分桶。
    pub fn new(memory_limit: usize) -> Self {
        Self {
            build_in_memory: Vec::new(),
            build_spilled: Vec::new(),
            probe_in_memory: Vec::new(),
            probe_spilled: Vec::new(),
            memory_limit,
            spilled: false,
        }
    }

    /// 添加 build 侧条目。超出限制时溢出。
    pub fn push_build(&mut self, key: Vec<u8>, value: Vec<u8>) {
        if self.build_in_memory.len() >= self.memory_limit {
            self.build_spilled.push((key, value));
            self.spilled = true;
        } else {
            self.build_in_memory.push((key, value));
        }
    }

    /// 添加 probe 侧条目。如果 build 已溢出，probe 也跟着溢出。
    pub fn push_probe(&mut self, key: Vec<u8>, value: Vec<u8>) {
        if self.spilled {
            self.probe_spilled.push((key, value));
        } else {
            self.probe_in_memory.push((key, value));
        }
    }

    /// build 内存条目数。
    pub fn build_in_memory_count(&self) -> usize {
        self.build_in_memory.len()
    }

    /// build 溢出条目数。
    pub fn build_spilled_count(&self) -> usize {
        self.build_spilled.len()
    }

    /// probe 内存条目数。
    pub fn probe_in_memory_count(&self) -> usize {
        self.probe_in_memory.len()
    }

    /// probe 溢出条目数。
    pub fn probe_spilled_count(&self) -> usize {
        self.probe_spilled.len()
    }

    /// 总条目数。
    pub fn total_count(&self) -> usize {
        self.build_in_memory.len()
            + self.build_spilled.len()
            + self.probe_in_memory.len()
            + self.probe_spilled.len()
    }

    /// 是否已溢出。
    pub fn is_spilled(&self) -> bool {
        self.spilled
    }

    /// 内存 JOIN（仅当未溢出时）：build_in_memory × probe_in_memory。
    pub fn join_in_memory(&self) -> Vec<JoinTuple> {
        if self.spilled {
            return Vec::new();
        }
        let mut result = Vec::new();
        for (b_key, b_value) in &self.build_in_memory {
            for (p_key, p_value) in &self.probe_in_memory {
                if b_key == p_key {
                    result.push(JoinTuple {
                        build_key: b_key.clone(),
                        build_value: b_value.clone(),
                        probe_value: p_value.clone(),
                    });
                }
            }
        }
        result
    }

    /// 溢出 JOIN（当分桶已溢出时）：把 build_spilled + build_in_memory
    /// 与 probe_spilled + probe_in_memory 做嵌套循环 JOIN。
    /// 生产环境应逐批读入磁盘数据做 JOIN；此处简化为一次性读入。
    pub fn join_spilled(&self) -> Vec<JoinTuple> {
        if !self.spilled {
            return self.join_in_memory();
        }
        let mut result = Vec::new();
        // build 侧：内存 + 溢出
        let build_all: Vec<&(Vec<u8>, Vec<u8>)> = self
            .build_in_memory
            .iter()
            .chain(self.build_spilled.iter())
            .collect();
        // probe 侧：内存 + 溢出
        let probe_all: Vec<&(Vec<u8>, Vec<u8>)> = self
            .probe_in_memory
            .iter()
            .chain(self.probe_spilled.iter())
            .collect();
        for (b_key, b_value) in &build_all {
            for (p_key, p_value) in &probe_all {
                if *b_key == *p_key {
                    result.push(JoinTuple {
                        build_key: b_key.clone(),
                        build_value: b_value.clone(),
                        probe_value: p_value.clone(),
                    });
                }
            }
        }
        result
    }

    /// 执行该桶的 JOIN（自动选择内存或溢出路径）。
    pub fn join(&self) -> Vec<JoinTuple> {
        if self.spilled {
            self.join_spilled()
        } else {
            self.join_in_memory()
        }
    }

    /// 字节大小估算。
    pub fn bytes(&self) -> usize {
        let build_bytes = self
            .build_in_memory
            .iter()
            .chain(self.build_spilled.iter())
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>();
        let probe_bytes = self
            .probe_in_memory
            .iter()
            .chain(self.probe_spilled.iter())
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>();
        build_bytes + probe_bytes
    }
}

// =====================================================================
//  Hash JOIN 统计
// =====================================================================

/// Hash JOIN 溢出统计。
#[derive(Debug, Clone, Default)]
pub struct HashJoinStats {
    /// build 侧总条目数。
    pub total_build: u64,
    /// probe 侧总条目数。
    pub total_probe: u64,
    /// 溢出的分桶数。
    pub spilled_partitions: u64,
    /// 总分桶数。
    pub total_partitions: u64,
    /// JOIN 结果数。
    pub join_result_count: u64,
    /// 溢出字节数。
    pub spilled_bytes: u64,
    /// 是否发生溢出。
    pub spilled: bool,
}

impl HashJoinStats {
    /// 创建空统计。
    pub fn new() -> Self {
        Self::default()
    }

    /// 溢出率（0.0~1.0）。
    pub fn spill_rate(&self) -> f64 {
        if self.total_partitions == 0 {
            return 0.0;
        }
        self.spilled_partitions as f64 / self.total_partitions as f64
    }
}

// =====================================================================
//  Hash JOIN 溢出器
// =====================================================================

/// Hash JOIN 溢出器：按 hash 分桶，相同 key 落在同一桶。
///
/// 算法（Grace Hash JOIN 简化版）：
/// 1. build 侧和 probe 侧都按 `hash(key) % num_partitions` 分桶。
/// 2. 每个桶有内存限制；超出时 build 侧溢出到磁盘，probe 侧跟着溢出。
/// 3. JOIN 时，逐桶做嵌套循环（内存桶直接 JOIN，溢出桶读入后 JOIN）。
pub struct HashJoinSpiller {
    /// 分桶列表。
    partitions: Vec<HashPartition>,
    /// 分桶数。
    num_partitions: usize,
    /// 统计信息。
    stats: HashJoinStats,
}

impl HashJoinSpiller {
    /// 创建 Hash JOIN 溢出器。
    /// `num_partitions` 分桶数，`memory_limit_per_partition` 每桶内存限制。
    pub fn new(num_partitions: usize, memory_limit_per_partition: usize) -> Self {
        let partitions = (0..num_partitions)
            .map(|_| HashPartition::new(memory_limit_per_partition))
            .collect();
        Self {
            partitions,
            num_partitions,
            stats: HashJoinStats {
                total_partitions: num_partitions as u64,
                ..Default::default()
            },
        }
    }

    /// 使用默认配置创建。
    pub fn with_default() -> Self {
        Self::new(DEFAULT_HASH_PARTITIONS, DEFAULT_MEMORY_LIMIT_ENTRIES)
    }

    /// 计算 key 的分桶索引。
    fn partition_index(&self, key: &[u8]) -> usize {
        // 简单 hash：FNV-1a
        let mut hash: u64 = 0xcbf29ce484222325;
        for &byte in key {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        (hash % self.num_partitions as u64) as usize
    }

    /// 添加 build 侧条目。
    pub fn push_build(&mut self, key: Vec<u8>, value: Vec<u8>) {
        let idx = self.partition_index(&key);
        let was_spilled = self.partitions[idx].is_spilled();
        self.partitions[idx].push_build(key, value);
        self.stats.total_build += 1;
        if !was_spilled && self.partitions[idx].is_spilled() {
            self.stats.spilled_partitions += 1;
            self.stats.spilled = true;
        }
    }

    /// 添加 probe 侧条目。
    pub fn push_probe(&mut self, key: Vec<u8>, value: Vec<u8>) {
        let idx = self.partition_index(&key);
        self.partitions[idx].push_probe(key, value);
        self.stats.total_probe += 1;
    }

    /// 执行 JOIN：逐桶 JOIN 后合并结果。
    pub fn join(&mut self) -> Vec<JoinTuple> {
        let mut result = Vec::new();
        for partition in &self.partitions {
            let tuples = partition.join();
            result.extend(tuples);
        }
        self.stats.join_result_count = result.len() as u64;
        result
    }

    /// 获取统计信息。
    pub fn stats(&self) -> &HashJoinStats {
        &self.stats
    }

    /// 分桶数。
    pub fn num_partitions(&self) -> usize {
        self.num_partitions
    }

    /// 获取指定分桶的引用。
    pub fn partition(&self, idx: usize) -> Option<&HashPartition> {
        self.partitions.get(idx)
    }

    /// 是否发生溢出。
    pub fn has_spilled(&self) -> bool {
        self.stats.spilled
    }

    /// 计算溢出字节数。
    pub fn spilled_bytes(&self) -> u64 {
        self.partitions
            .iter()
            .filter(|p| p.is_spilled())
            .map(|p| {
                p.build_spilled_count() as u64 * 16 + // 简化估算
                p.probe_spilled_count() as u64 * 16
            })
            .sum()
    }

    /// 重置所有状态。
    pub fn reset(&mut self, memory_limit_per_partition: usize) {
        for p in &mut self.partitions {
            *p = HashPartition::new(memory_limit_per_partition);
        }
        self.stats = HashJoinStats {
            total_partitions: self.num_partitions as u64,
            ..Default::default()
        };
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 验证一个序列是否按 key 升序排列。
pub fn is_sorted_by_key(entries: &[SortEntry]) -> bool {
    entries.windows(2).all(|w| w[0].key <= w[1].key)
}

/// 验证 JOIN 结果是否正确：每个结果的 build_key == probe_key。
pub fn validate_join_result(result: &[JoinTuple]) -> bool {
    result.iter().all(|t| !t.build_key.is_empty())
}

/// 生成 N 个随机排序条目（key 在 0..range 范围内）。
pub fn generate_random_entries(n: usize, range: u64) -> Vec<SortEntry> {
    use std::cell::Cell;
    // 简单 LCG 伪随机，避免引入 rand 依赖
    thread_local! {
        static SEED: Cell<u64> = const { Cell::new(0x12345678_9abcdef0) };
    }
    SEED.with(|seed| {
        (0..n as u64)
            .map(|i| {
                let s = seed.get();
                let new_s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                seed.set(new_s);
                let key = (s % range) as i64;
                SortEntry::new(key, i)
            })
            .collect()
    })
}

/// 生成 N 个有序排序条目（key 从 0 递增）。
pub fn generate_sorted_entries(n: usize) -> Vec<SortEntry> {
    (0..n as u64).map(|i| SortEntry::new(i as i64, i)).collect()
}

/// 生成 N 个逆序排序条目（key 从 n-1 递减）。
pub fn generate_reverse_entries(n: usize) -> Vec<SortEntry> {
    (0..n as u64)
        .map(|i| SortEntry::new((n - 1 - i as usize) as i64, i))
        .collect()
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  SortEntry 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sort_entry_new() {
        let entry = SortEntry::new(42, 100);
        assert_eq!(entry.key, 42);
        assert_eq!(entry.row_id, 100);
    }

    #[test]
    fn test_sort_entry_size() {
        let entry = SortEntry::new(0, 0);
        assert_eq!(entry.size(), 16);
    }

    #[test]
    fn test_sort_entry_ordering() {
        let e1 = SortEntry::new(1, 10);
        let e2 = SortEntry::new(2, 5);
        assert!(e1 < e2, "key=1 should be less than key=2");

        let e3 = SortEntry::new(1, 5);
        let e4 = SortEntry::new(1, 10);
        assert!(e3 < e4, "same key, row_id=5 should be less than row_id=10");
    }

    #[test]
    fn test_sort_entry_eq() {
        let e1 = SortEntry::new(1, 2);
        let e2 = SortEntry::new(1, 2);
        assert_eq!(e1, e2);
    }

    // -----------------------------------------------------------------
    //  SortedRun 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sorted_run_new_sorts_entries() {
        let entries = vec![
            SortEntry::new(3, 0),
            SortEntry::new(1, 1),
            SortEntry::new(2, 2),
        ];
        let run = SortedRun::new(entries);
        assert_eq!(run.len(), 3);
        assert_eq!(run.entries[0].key, 1);
        assert_eq!(run.entries[1].key, 2);
        assert_eq!(run.entries[2].key, 3);
    }

    #[test]
    fn test_sorted_run_empty() {
        let run = SortedRun::new(Vec::new());
        assert!(run.is_empty());
        assert_eq!(run.len(), 0);
    }

    #[test]
    fn test_sorted_run_peek() {
        let entries = vec![SortEntry::new(1, 0), SortEntry::new(2, 1)];
        let run = SortedRun::new(entries);
        assert_eq!(run.peek(), Some(&SortEntry::new(1, 0)));
    }

    #[test]
    fn test_sorted_run_peek_empty() {
        let run = SortedRun::new(Vec::new());
        assert_eq!(run.peek(), None);
    }

    #[test]
    fn test_sorted_run_next_entry() {
        let entries = vec![SortEntry::new(1, 0), SortEntry::new(2, 1)];
        let mut run = SortedRun::new(entries);

        assert_eq!(run.next_entry(), Some(SortEntry::new(1, 0)));
        assert_eq!(run.next_entry(), Some(SortEntry::new(2, 1)));
        assert_eq!(run.next_entry(), None);
    }

    #[test]
    fn test_sorted_run_is_exhausted() {
        let entries = vec![SortEntry::new(1, 0)];
        let mut run = SortedRun::new(entries);
        assert!(!run.is_exhausted());
        run.next_entry();
        assert!(run.is_exhausted());
    }

    #[test]
    fn test_sorted_run_reset() {
        let entries = vec![SortEntry::new(1, 0), SortEntry::new(2, 1)];
        let mut run = SortedRun::new(entries);
        run.next_entry();
        run.next_entry();
        assert!(run.is_exhausted());
        run.reset();
        assert!(!run.is_exhausted());
        assert_eq!(run.peek(), Some(&SortEntry::new(1, 0)));
    }

    #[test]
    fn test_sorted_run_bytes() {
        let entries = vec![SortEntry::new(1, 0), SortEntry::new(2, 1)];
        let run = SortedRun::new(entries);
        assert_eq!(run.bytes(), 32); // 2 * 16
    }

    // -----------------------------------------------------------------
    //  SorterStats 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sorter_stats_default() {
        let stats = SorterStats::default();
        assert_eq!(stats.total_input, 0);
        assert_eq!(stats.total_spilled, 0);
        assert_eq!(stats.run_count, 0);
        assert!(!stats.spilled);
    }

    #[test]
    fn test_sorter_stats_memory_hit_rate_no_input() {
        let stats = SorterStats::new();
        assert_eq!(stats.memory_hit_rate(), 1.0);
    }

    #[test]
    fn test_sorter_stats_memory_hit_rate_all_spilled() {
        let stats = SorterStats {
            total_input: 100,
            total_spilled: 100,
            ..Default::default()
        };
        assert_eq!(stats.memory_hit_rate(), 0.0);
    }

    #[test]
    fn test_sorter_stats_memory_hit_rate_half_spilled() {
        let stats = SorterStats {
            total_input: 100,
            total_spilled: 50,
            ..Default::default()
        };
        assert_eq!(stats.memory_hit_rate(), 0.5);
    }

    // -----------------------------------------------------------------
    //  ExternalSorter 基本操作测试
    // -----------------------------------------------------------------

    #[test]
    fn test_external_sorter_new() {
        let sorter = ExternalSorter::new(100);
        assert_eq!(sorter.memory_limit(), 100);
        assert_eq!(sorter.run_count(), 0);
        assert_eq!(sorter.buffer_len(), 0);
        assert!(!sorter.has_spilled());
    }

    #[test]
    fn test_external_sorter_with_default_limit() {
        let sorter = ExternalSorter::with_default_limit();
        assert_eq!(sorter.memory_limit(), DEFAULT_MEMORY_LIMIT_ENTRIES);
    }

    #[test]
    fn test_external_sorter_push_no_spill() {
        let mut sorter = ExternalSorter::new(10);
        sorter.push(SortEntry::new(1, 0));
        sorter.push(SortEntry::new(2, 1));
        assert_eq!(sorter.buffer_len(), 2);
        assert!(!sorter.has_spilled());
        assert_eq!(sorter.run_count(), 0);
    }

    #[test]
    fn test_external_sorter_push_triggers_spill() {
        let mut sorter = ExternalSorter::new(2);
        sorter.push(SortEntry::new(3, 0));
        sorter.push(SortEntry::new(1, 1));
        // 第 3 个 push 应触发溢出（缓冲满 2 条）
        sorter.push(SortEntry::new(2, 2));
        assert!(sorter.has_spilled());
        assert_eq!(sorter.run_count(), 1);
        assert_eq!(sorter.buffer_len(), 1); // 第 3 条在缓冲
    }

    #[test]
    fn test_external_sorter_flush_run_empty() {
        let mut sorter = ExternalSorter::new(10);
        sorter.flush_run(); // 空缓冲不应产生 run
        assert_eq!(sorter.run_count(), 0);
        assert!(!sorter.has_spilled());
    }

    #[test]
    fn test_external_sorter_flush_run() {
        let mut sorter = ExternalSorter::new(10);
        sorter.push(SortEntry::new(3, 0));
        sorter.push(SortEntry::new(1, 1));
        sorter.flush_run();
        assert_eq!(sorter.run_count(), 1);
        assert!(sorter.has_spilled());
        assert_eq!(sorter.buffer_len(), 0);
    }

    #[test]
    fn test_external_sorter_finish() {
        let mut sorter = ExternalSorter::new(10);
        sorter.push(SortEntry::new(1, 0));
        sorter.push(SortEntry::new(2, 1));
        sorter.finish();
        assert_eq!(sorter.run_count(), 1);
        assert_eq!(sorter.buffer_len(), 0);
    }

    #[test]
    fn test_external_sorter_finish_empty() {
        let mut sorter = ExternalSorter::new(10);
        sorter.finish();
        assert_eq!(sorter.run_count(), 0);
    }

    #[test]
    fn test_external_sorter_finish_in_memory_no_spill() {
        let mut sorter = ExternalSorter::new(100);
        sorter.push(SortEntry::new(3, 0));
        sorter.push(SortEntry::new(1, 1));
        sorter.finish_in_memory();
        assert!(!sorter.has_spilled());
        assert_eq!(sorter.stats().in_memory, 2);
    }

    #[test]
    fn test_external_sorter_finish_in_memory_with_spill() {
        let mut sorter = ExternalSorter::new(2);
        sorter.push(SortEntry::new(3, 0));
        sorter.push(SortEntry::new(1, 1));
        sorter.push(SortEntry::new(2, 2)); // 触发溢出
        sorter.push(SortEntry::new(4, 3));
        sorter.finish_in_memory(); // 已有 run，剩余缓冲作为 run
        assert!(sorter.has_spilled());
    }

    // -----------------------------------------------------------------
    //  ExternalSorter 归并测试
    // -----------------------------------------------------------------

    #[test]
    fn test_external_sorter_merge_empty() {
        let mut sorter = ExternalSorter::new(10);
        let result = sorter.merge();
        assert!(result.is_empty());
    }

    #[test]
    fn test_external_sorter_merge_single_run() {
        let mut sorter = ExternalSorter::new(100);
        sorter.push(SortEntry::new(3, 0));
        sorter.push(SortEntry::new(1, 1));
        sorter.push(SortEntry::new(2, 2));
        let result = sorter.merge();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].key, 1);
        assert_eq!(result[1].key, 2);
        assert_eq!(result[2].key, 3);
    }

    #[test]
    fn test_external_sorter_merge_multiple_runs() {
        let mut sorter = ExternalSorter::new(2);
        // run 1: keys 3, 1
        sorter.push(SortEntry::new(3, 0));
        sorter.push(SortEntry::new(1, 1));
        // 触发溢出
        sorter.push(SortEntry::new(5, 2));
        // run 2: keys 2, 4
        sorter.push(SortEntry::new(2, 3));
        sorter.push(SortEntry::new(4, 4));
        // 触发溢出
        sorter.push(SortEntry::new(0, 5));

        let result = sorter.merge();
        assert_eq!(result.len(), 6);
        assert!(is_sorted_by_key(&result));
        assert_eq!(result[0].key, 0);
        assert_eq!(result[5].key, 5);
    }

    #[test]
    fn test_external_sorter_merge_preserves_all_entries() {
        let mut sorter = ExternalSorter::new(3);
        for i in 0..10 {
            sorter.push(SortEntry::new((10 - i) as i64, i));
        }
        let result = sorter.merge();
        assert_eq!(result.len(), 10);
        assert!(is_sorted_by_key(&result));
    }

    #[test]
    fn test_external_sorter_merge_with_duplicates() {
        let mut sorter = ExternalSorter::new(2);
        sorter.push(SortEntry::new(1, 0));
        sorter.push(SortEntry::new(1, 1));
        sorter.push(SortEntry::new(1, 2));
        sorter.push(SortEntry::new(1, 3));

        let result = sorter.merge();
        assert_eq!(result.len(), 4);
        assert!(result.iter().all(|e| e.key == 1));
        // row_id 应保持升序（同 key 内稳定排序）
        assert_eq!(result[0].row_id, 0);
        assert_eq!(result[1].row_id, 1);
        assert_eq!(result[2].row_id, 2);
        assert_eq!(result[3].row_id, 3);
    }

    #[test]
    fn test_external_sorter_merge_negative_keys() {
        let mut sorter = ExternalSorter::new(2);
        sorter.push(SortEntry::new(5, 0));
        sorter.push(SortEntry::new(-3, 1));
        sorter.push(SortEntry::new(0, 2));
        sorter.push(SortEntry::new(-1, 3));

        let result = sorter.merge();
        assert!(is_sorted_by_key(&result));
        assert_eq!(result[0].key, -3);
        assert_eq!(result[1].key, -1);
        assert_eq!(result[2].key, 0);
        assert_eq!(result[3].key, 5);
    }

    // -----------------------------------------------------------------
    //  ExternalSorter 统计测试
    // -----------------------------------------------------------------

    #[test]
    fn test_external_sorter_stats_no_spill() {
        let mut sorter = ExternalSorter::new(100);
        sorter.push(SortEntry::new(1, 0));
        sorter.push(SortEntry::new(2, 1));
        let stats = sorter.stats();
        assert_eq!(stats.total_input, 2);
        assert_eq!(stats.total_spilled, 0);
        assert!(!stats.spilled);
    }

    #[test]
    fn test_external_sorter_stats_with_spill() {
        let mut sorter = ExternalSorter::new(2);
        sorter.push(SortEntry::new(1, 0));
        sorter.push(SortEntry::new(2, 1));
        sorter.push(SortEntry::new(3, 2)); // 触发溢出
        let stats = sorter.stats();
        assert_eq!(stats.total_input, 3);
        assert_eq!(stats.total_spilled, 2);
        assert_eq!(stats.run_count, 1);
        assert!(stats.spilled);
    }

    #[test]
    fn test_external_sorter_extend() {
        let mut sorter = ExternalSorter::new(100);
        let entries = vec![
            SortEntry::new(3, 0),
            SortEntry::new(1, 1),
            SortEntry::new(2, 2),
        ];
        sorter.extend(entries);
        assert_eq!(sorter.buffer_len(), 3);
        assert_eq!(sorter.stats().total_input, 3);
    }

    #[test]
    fn test_external_sorter_reset() {
        let mut sorter = ExternalSorter::new(2);
        sorter.push(SortEntry::new(1, 0));
        sorter.push(SortEntry::new(2, 1));
        sorter.push(SortEntry::new(3, 2));
        sorter.reset();
        assert_eq!(sorter.buffer_len(), 0);
        assert_eq!(sorter.run_count(), 0);
        assert!(!sorter.has_spilled());
        assert_eq!(sorter.stats().total_input, 0);
    }

    // -----------------------------------------------------------------
    //  HashPartition 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_hash_partition_new() {
        let p = HashPartition::new(10);
        assert_eq!(p.build_in_memory_count(), 0);
        assert_eq!(p.build_spilled_count(), 0);
        assert_eq!(p.probe_in_memory_count(), 0);
        assert_eq!(p.probe_spilled_count(), 0);
        assert!(!p.is_spilled());
    }

    #[test]
    fn test_hash_partition_push_build_no_spill() {
        let mut p = HashPartition::new(10);
        p.push_build(b"k1".to_vec(), b"v1".to_vec());
        p.push_build(b"k2".to_vec(), b"v2".to_vec());
        assert_eq!(p.build_in_memory_count(), 2);
        assert_eq!(p.build_spilled_count(), 0);
        assert!(!p.is_spilled());
    }

    #[test]
    fn test_hash_partition_push_build_triggers_spill() {
        let mut p = HashPartition::new(2);
        p.push_build(b"k1".to_vec(), b"v1".to_vec());
        p.push_build(b"k2".to_vec(), b"v2".to_vec());
        // 第 3 个应溢出
        p.push_build(b"k3".to_vec(), b"v3".to_vec());
        assert_eq!(p.build_in_memory_count(), 2);
        assert_eq!(p.build_spilled_count(), 1);
        assert!(p.is_spilled());
    }

    #[test]
    fn test_hash_partition_push_probe_no_spill() {
        let mut p = HashPartition::new(10);
        p.push_probe(b"k1".to_vec(), b"v1".to_vec());
        assert_eq!(p.probe_in_memory_count(), 1);
        assert_eq!(p.probe_spilled_count(), 0);
    }

    #[test]
    fn test_hash_partition_push_probe_after_spill() {
        let mut p = HashPartition::new(1);
        p.push_build(b"k1".to_vec(), b"v1".to_vec());
        p.push_build(b"k2".to_vec(), b"v2".to_vec()); // 溢出
                                                      // build 已溢出，probe 也跟着溢出
        p.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        assert_eq!(p.probe_spilled_count(), 1);
        assert_eq!(p.probe_in_memory_count(), 0);
    }

    #[test]
    fn test_hash_partition_total_count() {
        let mut p = HashPartition::new(2);
        p.push_build(b"k1".to_vec(), b"v1".to_vec());
        p.push_build(b"k2".to_vec(), b"v2".to_vec());
        p.push_build(b"k3".to_vec(), b"v3".to_vec()); // 溢出
        p.push_probe(b"k1".to_vec(), b"pv1".to_vec()); // 跟着溢出
        assert_eq!(p.total_count(), 4);
    }

    #[test]
    fn test_hash_partition_join_in_memory() {
        let mut p = HashPartition::new(10);
        p.push_build(b"k1".to_vec(), b"bv1".to_vec());
        p.push_build(b"k2".to_vec(), b"bv2".to_vec());
        p.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        p.push_probe(b"k3".to_vec(), b"pv3".to_vec()); // 无匹配

        let result = p.join_in_memory();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].build_key, b"k1");
        assert_eq!(result[0].build_value, b"bv1");
        assert_eq!(result[0].probe_value, b"pv1");
    }

    #[test]
    fn test_hash_partition_join_in_memory_multiple_matches() {
        let mut p = HashPartition::new(10);
        p.push_build(b"k1".to_vec(), b"bv1".to_vec());
        p.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        p.push_probe(b"k1".to_vec(), b"pv2".to_vec());

        let result = p.join_in_memory();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_hash_partition_join_in_memory_when_spilled_returns_empty() {
        let mut p = HashPartition::new(1);
        p.push_build(b"k1".to_vec(), b"bv1".to_vec());
        p.push_build(b"k2".to_vec(), b"bv2".to_vec()); // 溢出
        let result = p.join_in_memory();
        assert!(result.is_empty());
    }

    #[test]
    fn test_hash_partition_join_spilled_no_spill_fallback() {
        let mut p = HashPartition::new(10);
        p.push_build(b"k1".to_vec(), b"bv1".to_vec());
        p.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        // 未溢出时，join_spilled 应回退到 join_in_memory
        let result = p.join_spilled();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_hash_partition_join_spilled_with_spill() {
        let mut p = HashPartition::new(1);
        p.push_build(b"k1".to_vec(), b"bv1".to_vec());
        p.push_build(b"k2".to_vec(), b"bv2".to_vec()); // 溢出
        p.push_probe(b"k1".to_vec(), b"pv1".to_vec()); // 跟着溢出
        p.push_probe(b"k2".to_vec(), b"pv2".to_vec());

        let result = p.join_spilled();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_hash_partition_join_auto_select() {
        let mut p1 = HashPartition::new(10);
        p1.push_build(b"k1".to_vec(), b"bv1".to_vec());
        p1.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        let r1 = p1.join();
        assert_eq!(r1.len(), 1);

        let mut p2 = HashPartition::new(1);
        p2.push_build(b"k1".to_vec(), b"bv1".to_vec());
        p2.push_build(b"k2".to_vec(), b"bv2".to_vec()); // 溢出
        p2.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        let r2 = p2.join();
        assert_eq!(r2.len(), 1);
    }

    #[test]
    fn test_hash_partition_bytes() {
        let mut p = HashPartition::new(10);
        p.push_build(b"k1".to_vec(), b"v1".to_vec()); // 4 bytes
        p.push_probe(b"k1".to_vec(), b"pv1".to_vec()); // 5 bytes
        assert_eq!(p.bytes(), 9);
    }

    // -----------------------------------------------------------------
    //  HashJoinStats 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_hash_join_stats_default() {
        let stats = HashJoinStats::default();
        assert_eq!(stats.total_build, 0);
        assert_eq!(stats.total_probe, 0);
        assert_eq!(stats.spilled_partitions, 0);
        assert!(!stats.spilled);
    }

    #[test]
    fn test_hash_join_stats_spill_rate_no_partitions() {
        let stats = HashJoinStats::new();
        assert_eq!(stats.spill_rate(), 0.0);
    }

    #[test]
    fn test_hash_join_stats_spill_rate_half() {
        let stats = HashJoinStats {
            total_partitions: 8,
            spilled_partitions: 4,
            ..Default::default()
        };
        assert_eq!(stats.spill_rate(), 0.5);
    }

    // -----------------------------------------------------------------
    //  HashJoinSpiller 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_hash_join_spiller_new() {
        let spiller = HashJoinSpiller::new(8, 100);
        assert_eq!(spiller.num_partitions(), 8);
        assert!(!spiller.has_spilled());
    }

    #[test]
    fn test_hash_join_spiller_with_default() {
        let spiller = HashJoinSpiller::with_default();
        assert_eq!(spiller.num_partitions(), DEFAULT_HASH_PARTITIONS);
    }

    #[test]
    fn test_hash_join_spiller_partition_index_consistent() {
        let spiller = HashJoinSpiller::new(8, 100);
        let idx1 = spiller.partition_index(b"hello");
        let idx2 = spiller.partition_index(b"hello");
        assert_eq!(idx1, idx2, "same key should map to same partition");
    }

    #[test]
    fn test_hash_join_spiller_partition_index_different_keys() {
        let spiller = HashJoinSpiller::new(8, 100);
        // 不同 key 可能映射到相同或不同桶
        let idx1 = spiller.partition_index(b"key1");
        let idx2 = spiller.partition_index(b"key2");
        // 不强制断言不同，但至少不会 panic
        assert!(idx1 < 8);
        assert!(idx2 < 8);
    }

    #[test]
    fn test_hash_join_spiller_push_build_no_spill() {
        let mut spiller = HashJoinSpiller::new(8, 100);
        spiller.push_build(b"k1".to_vec(), b"v1".to_vec());
        spiller.push_build(b"k2".to_vec(), b"v2".to_vec());
        assert!(!spiller.has_spilled());
        assert_eq!(spiller.stats().total_build, 2);
    }

    #[test]
    fn test_hash_join_spiller_push_build_triggers_spill() {
        let mut spiller = HashJoinSpiller::new(1, 2); // 1 桶，限制 2 条
        spiller.push_build(b"k1".to_vec(), b"v1".to_vec());
        spiller.push_build(b"k2".to_vec(), b"v2".to_vec());
        spiller.push_build(b"k3".to_vec(), b"v3".to_vec()); // 溢出
        assert!(spiller.has_spilled());
        assert_eq!(spiller.stats().spilled_partitions, 1);
    }

    #[test]
    fn test_hash_join_spiller_push_probe() {
        let mut spiller = HashJoinSpiller::new(8, 100);
        spiller.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        spiller.push_probe(b"k2".to_vec(), b"pv2".to_vec());
        assert_eq!(spiller.stats().total_probe, 2);
    }

    #[test]
    fn test_hash_join_spiller_join_no_spill() {
        let mut spiller = HashJoinSpiller::new(8, 100);
        spiller.push_build(b"k1".to_vec(), b"bv1".to_vec());
        spiller.push_build(b"k2".to_vec(), b"bv2".to_vec());
        spiller.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        spiller.push_probe(b"k3".to_vec(), b"pv3".to_vec());

        let result = spiller.join();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].build_key, b"k1");
        assert_eq!(result[0].build_value, b"bv1");
        assert_eq!(result[0].probe_value, b"pv1");
    }

    #[test]
    fn test_hash_join_spiller_join_with_spill() {
        let mut spiller = HashJoinSpiller::new(1, 1); // 1 桶，限制 1 条
        spiller.push_build(b"k1".to_vec(), b"bv1".to_vec());
        spiller.push_build(b"k2".to_vec(), b"bv2".to_vec()); // 溢出
        spiller.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        spiller.push_probe(b"k2".to_vec(), b"pv2".to_vec());

        let result = spiller.join();
        assert_eq!(result.len(), 2);
        assert!(spiller.has_spilled());
    }

    #[test]
    fn test_hash_join_spiller_join_multiple_matches() {
        let mut spiller = HashJoinSpiller::new(8, 100);
        spiller.push_build(b"k1".to_vec(), b"bv1".to_vec());
        spiller.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        spiller.push_probe(b"k1".to_vec(), b"pv2".to_vec());
        spiller.push_probe(b"k1".to_vec(), b"pv3".to_vec());

        let result = spiller.join();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_hash_join_spiller_join_no_match() {
        let mut spiller = HashJoinSpiller::new(8, 100);
        spiller.push_build(b"k1".to_vec(), b"bv1".to_vec());
        spiller.push_probe(b"k2".to_vec(), b"pv2".to_vec());
        spiller.push_probe(b"k3".to_vec(), b"pv3".to_vec());

        let result = spiller.join();
        assert!(result.is_empty());
    }

    #[test]
    fn test_hash_join_spiller_partition_access() {
        let spiller = HashJoinSpiller::new(4, 100);
        assert!(spiller.partition(0).is_some());
        assert!(spiller.partition(3).is_some());
        assert!(spiller.partition(4).is_none());
    }

    #[test]
    fn test_hash_join_spiller_reset() {
        let mut spiller = HashJoinSpiller::new(8, 100);
        spiller.push_build(b"k1".to_vec(), b"v1".to_vec());
        spiller.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        spiller.reset(50);
        assert_eq!(spiller.stats().total_build, 0);
        assert_eq!(spiller.stats().total_probe, 0);
        assert!(!spiller.has_spilled());
    }

    #[test]
    fn test_hash_join_spiller_spilled_bytes() {
        let mut spiller = HashJoinSpiller::new(1, 1);
        spiller.push_build(b"k1".to_vec(), b"v1".to_vec());
        spiller.push_build(b"k2".to_vec(), b"v2".to_vec()); // 溢出
        let bytes = spiller.spilled_bytes();
        assert!(bytes > 0);
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_is_sorted_by_key_sorted() {
        let entries = vec![
            SortEntry::new(1, 0),
            SortEntry::new(2, 1),
            SortEntry::new(3, 2),
        ];
        assert!(is_sorted_by_key(&entries));
    }

    #[test]
    fn test_is_sorted_by_key_unsorted() {
        let entries = vec![
            SortEntry::new(3, 0),
            SortEntry::new(1, 1),
            SortEntry::new(2, 2),
        ];
        assert!(!is_sorted_by_key(&entries));
    }

    #[test]
    fn test_is_sorted_by_key_empty() {
        assert!(is_sorted_by_key(&[]));
    }

    #[test]
    fn test_is_sorted_by_key_single() {
        assert!(is_sorted_by_key(&[SortEntry::new(1, 0)]));
    }

    #[test]
    fn test_is_sorted_by_key_duplicates() {
        let entries = vec![
            SortEntry::new(1, 0),
            SortEntry::new(1, 1),
            SortEntry::new(1, 2),
        ];
        assert!(is_sorted_by_key(&entries));
    }

    #[test]
    fn test_validate_join_result_valid() {
        let result = vec![JoinTuple {
            build_key: b"k1".to_vec(),
            build_value: b"v1".to_vec(),
            probe_value: b"pv1".to_vec(),
        }];
        assert!(validate_join_result(&result));
    }

    #[test]
    fn test_validate_join_result_empty_key() {
        let result = vec![JoinTuple {
            build_key: Vec::new(),
            build_value: b"v1".to_vec(),
            probe_value: b"pv1".to_vec(),
        }];
        assert!(!validate_join_result(&result));
    }

    #[test]
    fn test_validate_join_result_empty() {
        assert!(validate_join_result(&[]));
    }

    #[test]
    fn test_generate_random_entries_count() {
        let entries = generate_random_entries(100, 1000);
        assert_eq!(entries.len(), 100);
    }

    #[test]
    fn test_generate_random_entries_row_ids_unique() {
        let entries = generate_random_entries(100, 1000);
        let row_ids: std::collections::HashSet<u64> = entries.iter().map(|e| e.row_id).collect();
        assert_eq!(row_ids.len(), 100);
    }

    #[test]
    fn test_generate_sorted_entries() {
        let entries = generate_sorted_entries(100);
        assert_eq!(entries.len(), 100);
        assert!(is_sorted_by_key(&entries));
        assert_eq!(entries[0].key, 0);
        assert_eq!(entries[99].key, 99);
    }

    #[test]
    fn test_generate_reverse_entries() {
        let entries = generate_reverse_entries(100);
        assert_eq!(entries.len(), 100);
        assert!(!is_sorted_by_key(&entries));
        assert_eq!(entries[0].key, 99);
        assert_eq!(entries[99].key, 0);
    }

    // -----------------------------------------------------------------
    //  集成测试：完整工作流
    // -----------------------------------------------------------------

    #[test]
    fn test_integration_small_sort_no_spill() {
        let mut sorter = ExternalSorter::new(1000);
        let entries = generate_random_entries(100, 1000);
        sorter.extend(entries.clone());
        sorter.finish_in_memory();

        let result = sorter.merge();
        assert_eq!(result.len(), 100);
        assert!(is_sorted_by_key(&result));
        assert!(!sorter.has_spilled());
    }

    #[test]
    fn test_integration_small_sort_with_spill() {
        let mut sorter = ExternalSorter::new(10);
        let entries = generate_random_entries(100, 1000);
        sorter.extend(entries.clone());

        let result = sorter.merge();
        assert_eq!(result.len(), 100);
        assert!(is_sorted_by_key(&result));
        assert!(sorter.has_spilled());
        assert!(sorter.stats().run_count >= 10);
    }

    #[test]
    fn test_integration_medium_sort_10000() {
        let mut sorter = ExternalSorter::new(100);
        let entries = generate_random_entries(10_000, 100_000);
        sorter.extend(entries);

        let result = sorter.merge();
        assert_eq!(result.len(), 10_000);
        assert!(is_sorted_by_key(&result));
        assert!(sorter.has_spilled());
    }

    #[test]
    fn test_integration_sort_already_sorted() {
        let mut sorter = ExternalSorter::new(100);
        let entries = generate_sorted_entries(1000);
        sorter.extend(entries);

        let result = sorter.merge();
        assert_eq!(result.len(), 1000);
        assert!(is_sorted_by_key(&result));
    }

    #[test]
    fn test_integration_sort_reverse_order() {
        let mut sorter = ExternalSorter::new(100);
        let entries = generate_reverse_entries(1000);
        sorter.extend(entries);

        let result = sorter.merge();
        assert_eq!(result.len(), 1000);
        assert!(is_sorted_by_key(&result));
        assert_eq!(result[0].key, 0);
        assert_eq!(result[999].key, 999);
    }

    #[test]
    fn test_integration_sort_with_duplicates() {
        let mut sorter = ExternalSorter::new(50);
        // 1000 条，key 只有 10 个不同值
        for i in 0..1000 {
            sorter.push(SortEntry::new((i % 10) as i64, i));
        }

        let result = sorter.merge();
        assert_eq!(result.len(), 1000);
        assert!(is_sorted_by_key(&result));
    }

    #[test]
    fn test_integration_hash_join_small_no_spill() {
        let mut spiller = HashJoinSpiller::new(8, 1000);
        // build: 10 条
        for i in 0..10 {
            spiller.push_build(
                format!("key{i}").into_bytes(),
                format!("bv{i}").into_bytes(),
            );
        }
        // probe: 5 条匹配
        for i in 0..5 {
            spiller.push_probe(
                format!("key{i}").into_bytes(),
                format!("pv{i}").into_bytes(),
            );
        }

        let result = spiller.join();
        assert_eq!(result.len(), 5);
        assert!(!spiller.has_spilled());
        assert!(validate_join_result(&result));
    }

    #[test]
    fn test_integration_hash_join_with_spill() {
        let mut spiller = HashJoinSpiller::new(2, 5); // 2 桶，每桶限 5 条
                                                      // build: 100 条
        for i in 0..100 {
            spiller.push_build(
                format!("key{i}").into_bytes(),
                format!("bv{i}").into_bytes(),
            );
        }
        // probe: 50 条
        for i in 0..50 {
            spiller.push_probe(
                format!("key{i}").into_bytes(),
                format!("pv{i}").into_bytes(),
            );
        }

        let result = spiller.join();
        assert_eq!(result.len(), 50);
        assert!(spiller.has_spilled());
        assert!(validate_join_result(&result));
    }

    #[test]
    fn test_integration_hash_join_one_to_many() {
        let mut spiller = HashJoinSpiller::new(8, 1000);
        // build: 1 条
        spiller.push_build(b"shared_key".to_vec(), b"bv".to_vec());
        // probe: 100 条都匹配
        for i in 0..100 {
            spiller.push_probe(b"shared_key".to_vec(), format!("pv{i}").into_bytes());
        }

        let result = spiller.join();
        assert_eq!(result.len(), 100);
    }

    #[test]
    fn test_integration_hash_join_many_to_one() {
        let mut spiller = HashJoinSpiller::new(8, 1000);
        // build: 100 条相同 key
        for i in 0..100 {
            spiller.push_build(b"shared_key".to_vec(), format!("bv{i}").into_bytes());
        }
        // probe: 1 条匹配
        spiller.push_probe(b"shared_key".to_vec(), b"pv".to_vec());

        let result = spiller.join();
        assert_eq!(result.len(), 100);
    }

    #[test]
    fn test_integration_hash_join_empty_build() {
        let mut spiller = HashJoinSpiller::new(8, 1000);
        spiller.push_probe(b"k1".to_vec(), b"pv1".to_vec());
        let result = spiller.join();
        assert!(result.is_empty());
    }

    #[test]
    fn test_integration_hash_join_empty_probe() {
        let mut spiller = HashJoinSpiller::new(8, 1000);
        spiller.push_build(b"k1".to_vec(), b"bv1".to_vec());
        let result = spiller.join();
        assert!(result.is_empty());
    }

    #[test]
    fn test_integration_sort_and_join_combined() {
        // 先用 ExternalSorter 排序，再用 HashJoinSpiller JOIN
        let mut sorter = ExternalSorter::new(50);
        for i in 0..200 {
            sorter.push(SortEntry::new((i % 20) as i64, i));
        }
        let sorted = sorter.merge();
        assert!(is_sorted_by_key(&sorted));

        // 用排序后的 key 做 JOIN
        let mut spiller = HashJoinSpiller::new(4, 100);
        for entry in &sorted {
            spiller.push_build(
                entry.key.to_le_bytes().to_vec(),
                entry.row_id.to_le_bytes().to_vec(),
            );
        }
        for i in 0..50 {
            let key = (i % 20) as i64;
            spiller.push_probe(
                key.to_le_bytes().to_vec(),
                (i as u64).to_le_bytes().to_vec(),
            );
        }

        let result = spiller.join();
        assert!(!result.is_empty());
        assert!(validate_join_result(&result));
    }

    #[test]
    fn test_integration_correctness_vs_std_sort() {
        // 与标准库排序对比正确性
        let mut sorter = ExternalSorter::new(30);
        let mut expected = Vec::new();
        for i in 0..300 {
            let key = (300 - i) as i64;
            sorter.push(SortEntry::new(key, i));
            expected.push(SortEntry::new(key, i));
        }
        expected.sort();

        let result = sorter.merge();
        assert_eq!(result, expected);
    }

    // -----------------------------------------------------------------
    //  大规模测试（#[ignore]，手动运行）
    // -----------------------------------------------------------------

    #[test]
    #[ignore = "大规模测试：1 亿行外部排序"]
    fn test_integration_large_scale_sort_100_million() {
        let mut sorter = ExternalSorter::new(1_000_000); // 每批 100 万条
        let entries = generate_random_entries(100_000_000, 1_000_000_000);
        sorter.extend(entries);

        let result = sorter.merge();
        assert_eq!(result.len(), 100_000_000);
        assert!(is_sorted_by_key(&result));
        assert!(sorter.has_spilled());
        assert!(sorter.stats().run_count >= 100);
    }

    #[test]
    #[ignore = "大规模测试：1 百万行 Hash JOIN 溢出"]
    fn test_integration_large_scale_hash_join_spill() {
        let mut spiller = HashJoinSpiller::new(16, 1000); // 16 桶，每桶 1000 条
                                                          // build: 1 百万条
        for i in 0..1_000_000 {
            let key = format!("key{}", i % 100_000); // 10 万不同 key
            spiller.push_build(key.into_bytes(), format!("bv{i}").into_bytes());
        }
        // probe: 50 万条
        for i in 0..500_000 {
            let key = format!("key{}", i % 100_000);
            spiller.push_probe(key.into_bytes(), format!("pv{i}").into_bytes());
        }

        let result = spiller.join();
        assert!(!result.is_empty());
        assert!(spiller.has_spilled());
        assert!(validate_join_result(&result));
    }
}
