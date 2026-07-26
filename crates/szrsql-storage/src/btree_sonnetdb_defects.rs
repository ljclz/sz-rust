//! SzRSQL B-Tree SonnetDB 缺陷覆盖测试 — 对应 `SzRSQL实施进度.md` M1 里程碑。
//!
//! ## 缺陷来源说明（诚实披露）
//!
//! `SzRSQL技术实现方案.md` 6.1 节列出"SonnetDB 54 条生产级缺陷清单"作为 SzRSQL
//! 的"已知风险清单"。经独立核查 SonnetDB 仓库（`E:\vue\test\C数据库\参考项目\SonnetDB`），
//! SonnetDB 实际使用 LSM-Tree + HNSW + FTS 索引栈，**不存在 B-Tree 数据结构**，
//! 因此 54 条缺陷中**无任何 B-Tree 缺陷**。
//!
//! `SzRSQL技术实现方案.md` 第 476 行列出 SzRSQL 自身需警示的 3 项 B-Tree 缺陷：
//! > "B-Tree 并发分裂数据丢失、空值索引不一致、唯一索引重复"
//!
//! M1 里程碑要求"B-Tree 相关 6 条缺陷"覆盖。本文件覆盖这 3 项具名缺陷，并补
//! 3 项来自 SzRSQL 自身代码注释/规格的 B-Tree 正确性风险点（共 6 条）：
//!
//! | # | 缺陷类别 | 来源 | 测试函数 |
//! |---|---------|------|---------|
//! | 1 | B-Tree 并发分裂数据丢失 | 技术方案 476 行 | `defect_01_concurrent_split_no_data_loss_*` |
//! | 2 | 空值索引不一致 | 技术方案 476 行 | `defect_02_null_key_index_consistency_*` |
//! | 3 | 唯一索引重复 | 技术方案 476 行 | `defect_03_unique_index_duplication_*` |
//! | 4 | Stale separator 导致搜索错误 | btree.rs 861-863 行注释 | `defect_04_stale_separator_search_*` |
//! | 5 | 并发混合操作不变量破坏 | 技术方案 3906 行"200+ 并发测试用例" | `defect_05_concurrent_mixed_ops_*` |
//! | 6 | 批量加载与增量插入不等价 | 技术方案 9.5 节 bulk_load 设计 | `defect_06_bulk_load_equivalence_*` |
//!
//! 每条缺陷至少 2 个测试用例（基本 + 边界），合计 14+ 测试。

use crate::btree::{BTree, BTreeError, BTREE_DEFAULT_ORDER};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::thread;

// =====================================================================
//  辅助函数
// =====================================================================

/// i64 → 8 字节大端 key（字典序 == 数值序）
fn encode_i64_key(v: i64) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

/// u64 → 8 字节大端 key
fn encode_u64_key(v: u64) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

/// XorShift64 PRNG（固定种子，可重现）
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xDEADBEEFCAFEBABE
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_u64_below(&mut self, max: u64) -> u64 {
        if max == 0 {
            return 0;
        }
        self.next_u64() % max
    }
}

// =====================================================================
//  缺陷 #1: B-Tree 并发分裂数据丢失
//  来源: SzRSQL技术实现方案.md 476 行
//  场景: 多线程并发插入触发节点分裂，分裂过程中其他线程的插入不应丢失
//  当前实现: Arc<Mutex<BTree>> 串行化保证原子性，但需验证分裂逻辑无遗漏
// =====================================================================

/// 缺陷 #1 基本: 8 线程并发插入 100K key（小 order 强制频繁分裂），无数据丢失
#[test]
fn defect_01_concurrent_split_no_data_loss_basic() {
    let order = 4; // 小 order 强制频繁分裂
    let threads = 8usize;
    let per_thread = 12_500usize; // 总计 100,000
    let bt = Arc::new(Mutex::new(BTree::new(order)));

    let mut handles = Vec::with_capacity(threads);
    for tid in 0..threads {
        let bt = Arc::clone(&bt);
        handles.push(thread::spawn(move || {
            let mut rng = XorShift64::new(0xA1B2_C3D4 + tid as u64);
            let base = (tid as u64) * (per_thread as u64);
            let range = per_thread as u64;
            let mut local_keys: HashSet<u64> = HashSet::with_capacity(per_thread);
            while local_keys.len() < per_thread {
                local_keys.insert(base + rng.next_u64_below(range));
            }
            assert_eq!(local_keys.len(), per_thread);

            let keys_vec: Vec<u64> = local_keys.into_iter().collect();
            for (idx, &k) in keys_vec.iter().enumerate() {
                let mut guard = bt.lock().unwrap();
                guard
                    .insert(encode_u64_key(k), (idx % 65536) as u16)
                    .expect("insert should not fail");
            }
            keys_vec
        }));
    }

    // 收集所有 key
    let mut all_keys: HashSet<u64> = HashSet::with_capacity(threads * per_thread);
    for h in handles {
        let keys = h.join().expect("thread should not panic");
        for k in keys {
            assert!(all_keys.insert(k), "duplicate key across threads");
        }
    }
    assert_eq!(all_keys.len(), threads * per_thread);

    // 验证：所有 key 都能被 search 到（无数据丢失）
    let bt = bt.lock().unwrap();
    let pairs = bt.in_order_leaf_traverse().expect("traverse");
    assert_eq!(pairs.len(), threads * per_thread, "data loss detected");

    // 严格递增
    for i in 1..pairs.len() {
        assert!(
            pairs[i - 1].0 < pairs[i].0,
            "not strictly increasing at {}",
            i
        );
    }

    // 全命中
    for &k in &all_keys {
        let found = bt.search(&encode_u64_key(k)).expect("search");
        assert!(found.is_some(), "key {} lost after concurrent split", k);
    }

    // 结构不变量
    assert!(bt.validate_all_nodes().is_ok(), "tree structure invalid");
}

/// 缺陷 #1 边界: 高并发 + 极小 order（order=3，BTree 允许的最小值）触发最频繁分裂
#[test]
fn defect_01_concurrent_split_no_data_loss_minimal_order() {
    let order = 3; // BTree::new 要求 order >= 3，否则 panic
    let threads = 4usize;
    let per_thread = 2_500usize; // 总计 10,000
    let bt = Arc::new(Mutex::new(BTree::new(order)));

    let mut handles = Vec::with_capacity(threads);
    for tid in 0..threads {
        let bt = Arc::clone(&bt);
        handles.push(thread::spawn(move || {
            let mut rng = XorShift64::new(0xF1E2_D3C4 + tid as u64);
            let base = (tid as u64) * (per_thread as u64);
            let range = per_thread as u64;
            let mut local_keys: HashSet<u64> = HashSet::with_capacity(per_thread);
            while local_keys.len() < per_thread {
                local_keys.insert(base + rng.next_u64_below(range));
            }
            let keys_vec: Vec<u64> = local_keys.into_iter().collect();
            for (idx, &k) in keys_vec.iter().enumerate() {
                let mut guard = bt.lock().unwrap();
                guard
                    .insert(encode_u64_key(k), (idx % 65536) as u16)
                    .expect("insert");
            }
            keys_vec
        }));
    }

    let mut all_keys: HashSet<u64> = HashSet::new();
    for h in handles {
        for k in h.join().expect("thread") {
            all_keys.insert(k);
        }
    }
    assert_eq!(all_keys.len(), threads * per_thread);

    let bt = bt.lock().unwrap();
    let pairs = bt.in_order_leaf_traverse().expect("traverse");
    assert_eq!(pairs.len(), threads * per_thread, "data loss at order=3");
    assert!(bt.validate_all_nodes().is_ok(), "invalid at order=3");

    for &k in &all_keys {
        assert!(
            bt.search(&encode_u64_key(k)).expect("search").is_some(),
            "key {} lost at order=3",
            k
        );
    }
}

// =====================================================================
//  缺陷 #2: 空值索引不一致
//  来源: SzRSQL技术实现方案.md 476 行
//  场景: NULL/空 key 在 insert/search/delete/range_scan 各路径应一致处理
//  当前实现: key 类型为 Vec<u8>，空 Vec<u8> 是合法 key，无需特殊编码
//  风险点: 不同代码路径对空 key 的处理可能不一致（如 binary_search 边界）
// =====================================================================

/// 缺陷 #2 基本: 空 key 插入/搜索/删除一致
#[test]
fn defect_02_null_key_index_consistency_basic() {
    let mut bt = BTree::with_default_order();

    // 插入空 key
    bt.insert(Vec::new(), 42).expect("insert empty key");
    // 搜索空 key 应找到 tuple_id=42
    assert_eq!(
        bt.search(&[]).expect("search"),
        Some(42),
        "empty key not found"
    );

    // 插入非空 key 混合
    for v in 1..=100i64 {
        bt.insert(encode_i64_key(v), v as u16).expect("insert");
    }

    // 空 key 仍应可搜索
    assert_eq!(
        bt.search(&[]).expect("search"),
        Some(42),
        "empty key lost after mixed insert"
    );

    // 中序遍历应包含空 key 作为第一个元素（空字节 < 任何非空字节）
    let pairs = bt.in_order_leaf_traverse().expect("traverse");
    assert_eq!(
        pairs.len(),
        101,
        "expected 101 keys (1 empty + 100 non-empty)"
    );
    assert!(
        pairs[0].0.is_empty(),
        "first key should be empty (lexicographically smallest)"
    );
    assert_eq!(pairs[0].1, 42, "empty key tuple_id mismatch");

    // 严格递增（空 key < 任何非空 key）
    for i in 1..pairs.len() {
        assert!(
            pairs[i - 1].0 < pairs[i].0,
            "not strictly increasing at {}",
            i
        );
    }

    // 删除空 key
    let deleted = bt.delete(&[]).expect("delete");
    assert!(deleted, "empty key should be deleted");
    assert_eq!(
        bt.search(&[]).expect("search"),
        None,
        "empty key still found after delete"
    );

    // 删除后剩余 key 仍可搜索
    for v in 1..=100i64 {
        assert!(
            bt.search(&encode_i64_key(v)).expect("search").is_some(),
            "key {} lost after empty key delete",
            v
        );
    }

    assert!(bt.validate_all_nodes().is_ok());
}

/// 缺陷 #2 边界: 多个空 key upsert（应只保留一个）+ 空 key 触发分裂
#[test]
fn defect_02_null_key_index_consistency_upsert_and_split() {
    let order = 4; // 小 order 强制分裂
    let mut bt = BTree::new(order);

    // 多次插入空 key（upsert 语义：应只保留最后一个 tuple_id）
    for tid in 1..=10u16 {
        bt.insert(Vec::new(), tid).expect("insert empty key");
    }
    assert_eq!(
        bt.search(&[]).expect("search"),
        Some(10),
        "empty key upsert failed"
    );

    // 插入大量非空 key 触发分裂
    for v in 1..=200i64 {
        bt.insert(encode_i64_key(v), v as u16).expect("insert");
    }

    // 空 key 仍应是 tuple_id=10
    assert_eq!(
        bt.search(&[]).expect("search"),
        Some(10),
        "empty key tuple_id changed after split"
    );

    // 中序遍历验证：空 key 应为第一个，且总 key 数 = 201（1 空 + 200 非空）
    let pairs = bt.in_order_leaf_traverse().expect("traverse");
    assert_eq!(pairs.len(), 201, "expected 201 keys");
    assert!(pairs[0].0.is_empty(), "first key should be empty");
    assert_eq!(pairs[0].1, 10, "empty key tuple_id mismatch after split");

    // 严格递增
    for i in 1..pairs.len() {
        assert!(
            pairs[i - 1].0 < pairs[i].0,
            "not strictly increasing at {}",
            i
        );
    }

    assert!(bt.validate_all_nodes().is_ok());
}

// =====================================================================
//  缺陷 #3: 唯一索引重复
//  来源: SzRSQL技术实现方案.md 476 行
//  场景: 同一 key 多次插入应只保留一个条目（upsert 语义），并发场景下也不应产生重复
//  当前实现: insert() 内部 search_key 检查存在性，存在则更新 tuple_id
//  风险点: 并发 upsert 可能因竞态产生重复；delete + reinsert 路径可能产生重复
// =====================================================================

/// 缺陷 #3 基本: 串行多次插入同一 key 应只保留一个条目
#[test]
fn defect_03_unique_index_duplication_basic() {
    let mut bt = BTree::with_default_order();
    let key = encode_i64_key(12345);

    // 串行插入同一 key 100 次，每次不同 tuple_id
    for tid in 1..=100u16 {
        bt.insert(key.clone(), tid).expect("insert");
    }

    // search 应返回最后一个 tuple_id
    assert_eq!(
        bt.search(&key).expect("search"),
        Some(100),
        "tuple_id not updated"
    );

    // 中序遍历应只有 1 个条目
    let pairs = bt.in_order_leaf_traverse().expect("traverse");
    assert_eq!(pairs.len(), 1, "duplicate key detected: {}", pairs.len());
    assert_eq!(pairs[0].0, key);
    assert_eq!(pairs[0].1, 100);

    // 混合插入其他 key 后再 upsert 原 key
    for v in 1..=50i64 {
        bt.insert(encode_i64_key(v), v as u16).expect("insert");
    }
    bt.insert(key.clone(), 999).expect("upsert");
    assert_eq!(
        bt.search(&key).expect("search"),
        Some(999),
        "upsert after mixed insert failed"
    );

    // 总 key 数应为 51（12345 + 1..=50）
    let pairs = bt.in_order_leaf_traverse().expect("traverse");
    assert_eq!(pairs.len(), 51, "duplicate after mixed upsert");

    assert!(bt.validate_all_nodes().is_ok());
}

/// 缺陷 #3 边界: 并发 upsert 同一 key，最终应只保留一个条目
#[test]
fn defect_03_unique_index_duplication_concurrent_upsert() {
    let key = encode_i64_key(99999);
    let bt = Arc::new(Mutex::new(BTree::with_default_order()));
    let threads = 16usize;
    let per_thread = 100usize;

    // 16 线程，每线程 100 次插入同一 key（不同 tuple_id）
    let mut handles = Vec::with_capacity(threads);
    for tid in 0..threads {
        let bt = Arc::clone(&bt);
        let key_clone = key.clone();
        handles.push(thread::spawn(move || {
            for i in 0..per_thread {
                let tuple_id = ((tid * per_thread + i) % 65536) as u16;
                let mut guard = bt.lock().unwrap();
                guard.insert(key_clone.clone(), tuple_id).expect("insert");
            }
        }));
    }
    for h in handles {
        h.join().expect("thread");
    }

    // 验证：中序遍历应只有 1 个条目（key, 最后写入的 tuple_id）
    let bt = bt.lock().unwrap();
    let pairs = bt.in_order_leaf_traverse().expect("traverse");
    assert_eq!(
        pairs.len(),
        1,
        "concurrent upsert produced duplicates: {}",
        pairs.len()
    );
    assert_eq!(pairs[0].0, key, "key mismatch");

    // search 应返回某个 tuple_id（具体值取决于调度，但应为 0..65536 内某值）
    let found = bt.search(&key).expect("search");
    assert!(found.is_some(), "key not found after concurrent upsert");

    assert!(bt.validate_all_nodes().is_ok());
}

// =====================================================================
//  缺陷 #4: Stale separator 导致搜索错误
//  来源: btree.rs 861-863 行注释（B+Tree 允许 internal 节点保留 stale separator）
//  场景: 删除 key 后，internal 节点可能仍保留该 key 作为分隔键；
//        search 用 >= 导航到右子树，右子树叶子中找不到该 key 应返回 None
//  风险点: 若 search 逻辑错误地匹配 internal 节点的 stale separator，会返回错误结果
// =====================================================================

/// 缺陷 #4 基本: 删除 key 后再搜索该 key 应返回 None（即使 internal 节点仍保留 stale separator）
#[test]
fn defect_04_stale_separator_search_basic() {
    let order = 4; // 小 order 强制产生多层 internal 节点
    let mut bt = BTree::new(order);

    // 插入 1..=200，触发多次分裂，产生多层 internal 节点
    for v in 1..=200i64 {
        bt.insert(encode_i64_key(v), v as u16).expect("insert");
    }
    assert!(
        bt.height() >= 2,
        "expected height >= 2, got {}",
        bt.height()
    );

    // 删除中间的 key（这些 key 可能作为 separator 出现在 internal 节点中）
    let deleted_keys: Vec<i64> = (50..=150).collect();
    for &v in &deleted_keys {
        let deleted = bt.delete(&encode_i64_key(v)).expect("delete");
        assert!(deleted, "key {} should be deleted", v);
    }

    // 验证：删除的 key 都应返回 None（即使可能仍是 stale separator）
    for &v in &deleted_keys {
        assert_eq!(
            bt.search(&encode_i64_key(v)).expect("search"),
            None,
            "deleted key {} still found (stale separator bug)",
            v
        );
    }

    // 验证：未删除的 key 都应能找到
    for v in 1i64..=200 {
        if deleted_keys.contains(&v) {
            continue;
        }
        assert!(
            bt.search(&encode_i64_key(v)).expect("search").is_some(),
            "non-deleted key {} lost",
            v
        );
    }

    assert!(bt.validate_all_nodes().is_ok());
}

/// 缺陷 #4 边界: 删除全部 key 后再搜索任意 key 应返回 None
#[test]
fn defect_04_stale_separator_search_all_deleted() {
    let order = 4;
    let mut bt = BTree::new(order);

    // 插入 1..=100
    for v in 1..=100i64 {
        bt.insert(encode_i64_key(v), v as u16).expect("insert");
    }

    // 删除全部
    for v in 1..=100i64 {
        assert!(
            bt.delete(&encode_i64_key(v)).expect("delete"),
            "delete {}",
            v
        );
    }

    // 搜索任意已删除 key 都应返回 None
    for v in [1, 25, 50, 75, 100].iter().copied() {
        assert_eq!(
            bt.search(&encode_i64_key(v)).expect("search"),
            None,
            "deleted key {} still found",
            v
        );
    }

    // 重新插入应正常工作
    for v in 1..=50i64 {
        bt.insert(encode_i64_key(v), v as u16).expect("reinsert");
    }
    for v in 1..=50i64 {
        assert_eq!(
            bt.search(&encode_i64_key(v)).expect("search"),
            Some(v as u16),
            "reinserted key {} not found",
            v
        );
    }

    assert!(bt.validate_all_nodes().is_ok());
}

// =====================================================================
//  缺陷 #5: 并发混合操作不变量破坏
//  来源: SzRSQL技术实现方案.md 3906 行"200+ 并发测试用例"
//  场景: 多线程同时执行 insert/delete/search 混合操作，最终树结构应保持不变量
//  当前实现: Arc<Mutex<BTree>> 串行化保证原子性，但需验证混合操作下不变量始终成立
//  风险点: 长时间混合操作可能累积状态污染（如 delete 后 insert 路径错误）
// =====================================================================

/// 缺陷 #5 基本: 8 线程混合 insert/delete/search，最终结构不变量成立
#[test]
fn defect_05_concurrent_mixed_ops_invariants() {
    let order = 8;
    let threads = 8usize;
    let ops_per_thread = 5_000usize;
    let bt = Arc::new(Mutex::new(BTree::new(order)));

    // 预填充 1000 个 key
    {
        let mut guard = bt.lock().unwrap();
        for v in 0..1000i64 {
            guard.insert(encode_i64_key(v), v as u16).expect("prefill");
        }
    }

    let mut handles = Vec::with_capacity(threads);
    for tid in 0..threads {
        let bt = Arc::clone(&bt);
        handles.push(thread::spawn(move || {
            let mut rng = XorShift64::new(0x5A5A_5A5A + tid as u64);
            let mut local_inserted: HashSet<i64> = HashSet::new();
            for _ in 0..ops_per_thread {
                let v = (rng.next_u64() % 100_000) as i64;
                let op = rng.next_u64_below(3);
                let mut guard = bt.lock().unwrap();
                match op {
                    0 => {
                        // insert
                        guard
                            .insert(encode_i64_key(v), (v % 65536) as u16)
                            .expect("insert");
                        local_inserted.insert(v);
                    }
                    1 => {
                        // delete
                        let _ = guard.delete(&encode_i64_key(v));
                        local_inserted.remove(&v);
                    }
                    _ => {
                        // search
                        let _ = guard.search(&encode_i64_key(v));
                    }
                }
            }
            local_inserted
        }));
    }

    // 收集每线程最后持有的 key 集合（注意：由于并发，这些集合可能已过时）
    let mut _union: HashSet<i64> = HashSet::new();
    for h in handles {
        let local = h.join().expect("thread");
        for k in local {
            _union.insert(k);
        }
    }

    // 关键验证：树结构不变量始终成立（即使有并发混合操作）
    let bt = bt.lock().unwrap();
    assert!(
        bt.validate_all_nodes().is_ok(),
        "invariants broken by concurrent mixed ops"
    );

    // 中序遍历应严格递增
    let pairs = bt.in_order_leaf_traverse().expect("traverse");
    for i in 1..pairs.len() {
        assert!(
            pairs[i - 1].0 < pairs[i].0,
            "not strictly increasing at {}",
            i
        );
    }
}

/// 缺陷 #5 边界: 并发 insert + delete 同一 key 集合，最终状态自洽
#[test]
fn defect_05_concurrent_mixed_ops_insert_delete_same_keys() {
    let order = 8;
    let bt = Arc::new(Mutex::new(BTree::new(order)));
    let key_count = 2_000i64;

    // 线程 A: 持续 insert key 0..key_count
    // 线程 B: 持续 delete key 0..key_count
    // 线程 C: 持续 search key 0..key_count
    let bt_a = Arc::clone(&bt);
    let bt_b = Arc::clone(&bt);
    let bt_c = Arc::clone(&bt);

    let h_a = thread::spawn(move || {
        for round in 0..3 {
            for v in 0..key_count {
                bt_a.lock()
                    .unwrap()
                    .insert(encode_i64_key(v), ((round * key_count + v) % 65536) as u16)
                    .expect("insert");
            }
        }
    });

    let h_b = thread::spawn(move || {
        for _ in 0..3 {
            for v in 0..key_count {
                let _ = bt_b.lock().unwrap().delete(&encode_i64_key(v));
            }
        }
    });

    let h_c = thread::spawn(move || {
        for _ in 0..3 {
            for v in 0..key_count {
                let _ = bt_c.lock().unwrap().search(&encode_i64_key(v));
            }
        }
    });

    h_a.join().expect("thread A");
    h_b.join().expect("thread B");
    h_c.join().expect("thread C");

    // 最终状态自洽：validate 通过 + 中序遍历严格递增
    let bt = bt.lock().unwrap();
    assert!(bt.validate_all_nodes().is_ok(), "invariants broken");
    let pairs = bt.in_order_leaf_traverse().expect("traverse");
    for i in 1..pairs.len() {
        assert!(
            pairs[i - 1].0 < pairs[i].0,
            "not strictly increasing at {}",
            i
        );
    }
    // search 任意 key 应返回 Some 或 None，不应 panic
    for v in 0..key_count {
        let _ = bt.search(&encode_i64_key(v)).expect("search");
    }
}

// =====================================================================
//  缺陷 #6: 批量加载与增量插入不等价
//  来源: SzRSQL技术实现方案.md 9.5 节 bulk_load 设计
//  场景: bulk_load 与逐条 insert 应产生等价的 B-Tree（相同 key 集合 + 不变量成立）
//  风险点: bulk_load 走不同代码路径，可能产生与 insert 不同的树结构（如错误的兄弟链表）
// =====================================================================

/// 缺陷 #6 基本: bulk_load 与逐条 insert 产生等价的 key 集合
#[test]
fn defect_06_bulk_load_equivalence_basic() {
    // 生成 5000 个已排序的 i64 key
    let keys: Vec<i64> = (1..=5_000).collect();
    let items: Vec<(Vec<u8>, u16)> = keys
        .iter()
        .map(|&v| (encode_i64_key(v), v as u16))
        .collect();

    // 树 A: 逐条 insert
    let mut bt_a = BTree::with_default_order();
    for (k, tid) in &items {
        bt_a.insert(k.clone(), *tid).expect("insert");
    }

    // 树 B: bulk_load
    let mut bt_b = BTree::with_default_order();
    bt_b.bulk_load(items.iter().cloned()).expect("bulk_load");

    // 验证：两棵树的中序遍历结果相同
    let pairs_a = bt_a.in_order_leaf_traverse().expect("traverse A");
    let pairs_b = bt_b.in_order_leaf_traverse().expect("traverse B");
    assert_eq!(pairs_a.len(), pairs_b.len(), "length mismatch");
    for (i, (a, b)) in pairs_a.iter().zip(pairs_b.iter()).enumerate() {
        assert_eq!(a.0, b.0, "key mismatch at {}", i);
        assert_eq!(a.1, b.1, "tuple_id mismatch at {}", i);
    }

    // 验证：两棵树都通过 validate
    assert!(bt_a.validate_all_nodes().is_ok(), "tree A invalid");
    assert!(bt_b.validate_all_nodes().is_ok(), "tree B invalid");

    // 验证：两棵树对任意 key 的 search 结果相同
    for &v in &keys {
        let ra = bt_a.search(&encode_i64_key(v)).expect("search A");
        let rb = bt_b.search(&encode_i64_key(v)).expect("search B");
        assert_eq!(ra, rb, "search mismatch for key {}", v);
    }
}

/// 缺陷 #6 边界: bulk_load 空输入 + bulk_load 单元素 + bulk_load 后再 insert
#[test]
fn defect_06_bulk_load_equivalence_edge_cases() {
    // 边界 1: bulk_load 空输入应返回 Err(BulkLoadEmpty)
    let mut bt_empty = BTree::with_default_order();
    let empty: Vec<(Vec<u8>, u16)> = Vec::new();
    let result = bt_empty.bulk_load(empty);
    assert!(
        matches!(result, Err(BTreeError::BulkLoadEmpty)),
        "expected BulkLoadEmpty"
    );

    // 边界 2: bulk_load 单元素
    let mut bt_one = BTree::with_default_order();
    let single = vec![(encode_i64_key(42), 42u16)];
    bt_one.bulk_load(single).expect("bulk_load single");
    assert_eq!(
        bt_one.search(&encode_i64_key(42)).expect("search"),
        Some(42)
    );
    assert!(bt_one.validate_all_nodes().is_ok());

    // 边界 3: bulk_load 后再 insert 应正常工作
    let mut bt_mixed = BTree::with_default_order();
    let initial: Vec<(Vec<u8>, u16)> = (1..=100).map(|v| (encode_i64_key(v), v as u16)).collect();
    bt_mixed.bulk_load(initial).expect("bulk_load");

    // 再 insert 101..=200
    for v in 101..=200i64 {
        bt_mixed
            .insert(encode_i64_key(v), v as u16)
            .expect("insert after bulk_load");
    }

    // 验证全部 key
    for v in 1..=200i64 {
        assert_eq!(
            bt_mixed.search(&encode_i64_key(v)).expect("search"),
            Some(v as u16),
            "key {} not found after bulk_load + insert",
            v
        );
    }
    assert!(bt_mixed.validate_all_nodes().is_ok());

    // 边界 4: bulk_load 未排序输入应返回 Err(BulkLoadNotSorted)
    let mut bt_unsorted = BTree::with_default_order();
    let unsorted = vec![
        (encode_i64_key(10), 10u16),
        (encode_i64_key(5), 5u16), // 乱序
    ];
    let result = bt_unsorted.bulk_load(unsorted);
    assert!(
        matches!(result, Err(BTreeError::BulkLoadNotSorted { .. })),
        "expected BulkLoadNotSorted"
    );
}

// =====================================================================
//  M1 里程碑汇总验证
// =====================================================================

/// M1 汇总: 6 类缺陷全部覆盖 + 默认 order 下大数量级插入无丢失
#[test]
fn m1_milestone_all_defects_summary_large_scale() {
    let mut bt = BTree::with_default_order();
    let total = 50_000usize;

    // 大量随机 key 插入
    let mut rng = XorShift64::new(0xCAFE_BABE_2024_0718);
    let mut keys: HashSet<i64> = HashSet::with_capacity(total);
    while keys.len() < total {
        keys.insert((rng.next_u64() as i64) / 2); // 转为非负 i64
    }
    assert_eq!(keys.len(), total);

    for (idx, &v) in keys.iter().enumerate() {
        bt.insert(encode_i64_key(v), (idx % 65536) as u16)
            .expect("insert");
    }

    // 全命中
    for &v in &keys {
        assert!(
            bt.search(&encode_i64_key(v)).expect("search").is_some(),
            "key {} lost",
            v
        );
    }

    // 中序遍历严格递增
    let pairs = bt.in_order_leaf_traverse().expect("traverse");
    assert_eq!(pairs.len(), total);
    for i in 1..pairs.len() {
        assert!(
            pairs[i - 1].0 < pairs[i].0,
            "not strictly increasing at {}",
            i
        );
    }

    // 不变量
    assert!(bt.validate_all_nodes().is_ok());

    // 默认 order 验证
    assert_eq!(bt.order(), BTREE_DEFAULT_ORDER);
}
