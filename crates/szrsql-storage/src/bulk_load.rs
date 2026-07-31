//! SzRSQL 批量导入（Bottom-Up Bulk Load）— 对应 `SzRSQL实施进度.md` Phase 1.10。
//!
//! 验证标准：
//! - **已排序 1 亿行批量构建 B-Tree**（不逐条插入）
//! - **内存受限分批构建**
//! - **批量导入比逐条插入快 10x**
//! - **结果树完全平衡**（所有叶子处于同一深度）
//!
//! 实现位于 `crate::btree::BTree::bulk_load` / `bulk_load_batched`。
//! 本模块仅包含测试与辅助工具。

use crate::btree::{BTree, BTreeError, BTREE_DEFAULT_ORDER};

// =====================================================================
//  辅助工具
// =====================================================================

/// 生成 n 个升序的 (i64-encoded key, tuple_id) 数据
fn make_sorted_items(n: usize) -> Vec<(Vec<u8>, u32)> {
    (0..n)
        .map(|i| {
            let key = (i as i64).to_be_bytes().to_vec(); // i64 升序 → 大端字节序升序
            let tuple_id = (i % 65536) as u32;
            (key, tuple_id)
        })
        .collect()
}

/// 生成 n 个升序的 (i64-encoded key, tuple_id) 数据，带起始偏移
fn make_sorted_items_offset(n: usize, offset: i64) -> Vec<(Vec<u8>, u32)> {
    (0..n)
        .map(|i| {
            let key = (offset + i as i64).to_be_bytes().to_vec();
            let tuple_id = (i % 65536) as u32;
            (key, tuple_id)
        })
        .collect()
}

/// 计算所有叶子节点是否处于同一深度（结果树完全平衡）
fn all_leaves_at_same_depth(bt: &BTree) -> bool {
    let root = bt.root_page_id();
    let mut depths: std::collections::HashSet<usize> = std::collections::HashSet::new();
    // DFS 遍历，记录每个叶子节点的深度
    fn dfs(bt: &BTree, page_id: u32, depth: usize, depths: &mut std::collections::HashSet<usize>) {
        let node = match bt.read_node_public(page_id) {
            Ok(n) => n,
            Err(_) => return,
        };
        if node.is_leaf() {
            depths.insert(depth);
            return;
        }
        for &child in &node.children {
            dfs(bt, child, depth + 1, depths);
        }
    }
    dfs(bt, root, 1, &mut depths);
    depths.len() == 1
}

// =====================================================================
//  Phase 1.10 测试 — 基础正确性
// =====================================================================

/// 基础：100 个已排序 item 批量构建，验证全部可查、严格升序
#[test]
fn phase_0110_bulk_load_basic_100_items() {
    let items = make_sorted_items(100);
    let mut bt = BTree::with_default_order();
    bt.bulk_load(items.clone())
        .expect("bulk_load should succeed");

    // 校验所有节点不变量
    bt.validate_all_nodes().expect("validate should pass");

    // 全部可查
    for (i, (k, _)) in items.iter().enumerate() {
        let found = bt.search(k).expect("search should not error");
        assert_eq!(
            found,
            Some((i % 65536) as u32),
            "key at index {} not found or tuple_id mismatch",
            i
        );
    }

    // 中序遍历严格升序
    let pairs = bt
        .in_order_leaf_traverse()
        .expect("traverse should succeed");
    assert_eq!(pairs.len(), 100);
    for i in 1..pairs.len() {
        assert!(
            pairs[i - 1].0 < pairs[i].0,
            "not strictly increasing at {}",
            i
        );
    }
}

/// 空输入：返回 BulkLoadEmpty 错误
#[test]
fn phase_0110_bulk_load_empty_input_errors() {
    let items: Vec<(Vec<u8>, u32)> = Vec::new();
    let mut bt = BTree::with_default_order();
    let result = bt.bulk_load(items);
    assert!(matches!(result, Err(BTreeError::BulkLoadEmpty)));
}

/// 未排序输入：返回 BulkLoadNotSorted 错误
#[test]
fn phase_0110_bulk_load_unsorted_input_errors() {
    let mut items = make_sorted_items(10);
    // 交换第 3 和第 4 个元素，破坏升序
    items.swap(3, 4);
    let mut bt = BTree::with_default_order();
    let result = bt.bulk_load(items);
    assert!(matches!(result, Err(BTreeError::BulkLoadNotSorted { .. })));
}

/// 重复 key 输入：返回 BulkLoadNotSorted 错误（>= 比较）
#[test]
fn phase_0110_bulk_load_duplicate_key_errors() {
    let mut items = make_sorted_items(10);
    // 复制第 5 个 key，插入到第 6 个位置
    let dup = items[5].clone();
    items.insert(6, dup);
    let mut bt = BTree::with_default_order();
    let result = bt.bulk_load(items);
    assert!(matches!(result, Err(BTreeError::BulkLoadNotSorted { .. })));
}

/// 单元素：构建高度=1 的单节点树
#[test]
fn phase_0110_bulk_load_single_item() {
    let items = make_sorted_items(1);
    let mut bt = BTree::with_default_order();
    bt.bulk_load(items.clone())
        .expect("bulk_load should succeed");
    assert_eq!(bt.height(), 1, "single item should have height 1");
    assert_eq!(bt.node_count(), 1, "single item should have 1 node");
    assert_eq!(
        bt.search(&items[0].0).unwrap(),
        Some(items[0].1),
        "single item should be found"
    );
}

/// 边界：恰好 order 个 item（1 个叶子，无 internal）
#[test]
fn phase_0110_bulk_load_exact_one_leaf() {
    let order = 16;
    let items = make_sorted_items(order);
    let mut bt = BTree::new(order);
    bt.bulk_load(items.clone())
        .expect("bulk_load should succeed");
    assert_eq!(bt.height(), 1, "should be single-leaf tree");
    assert_eq!(bt.node_count(), 1, "should have 1 node");
    bt.validate_all_nodes().unwrap();
    for (k, v) in &items {
        assert_eq!(bt.search(k).unwrap(), Some(*v));
    }
}

/// 边界：order + 1 个 item（2 个叶子 + 1 个 internal 根）
#[test]
fn phase_0110_bulk_load_two_leaves_one_root() {
    let order = 16;
    let items = make_sorted_items(order + 1);
    let mut bt = BTree::new(order);
    bt.bulk_load(items.clone())
        .expect("bulk_load should succeed");
    assert_eq!(bt.height(), 2, "should have height 2");
    assert_eq!(
        bt.node_count(),
        3,
        "should have 2 leaves + 1 root = 3 nodes"
    );
    bt.validate_all_nodes().unwrap();
    for (k, v) in &items {
        assert_eq!(bt.search(k).unwrap(), Some(*v));
    }
}

// =====================================================================
//  Phase 1.10 测试 — 完全平衡
// =====================================================================

/// 完全平衡：10000 个 item，验证所有叶子处于同一深度
#[test]
fn phase_0110_bulk_load_tree_fully_balanced_10k() {
    let items = make_sorted_items(10_000);
    let mut bt = BTree::new(32);
    bt.bulk_load(items.clone())
        .expect("bulk_load should succeed");
    bt.validate_all_nodes().unwrap();
    assert!(
        all_leaves_at_same_depth(&bt),
        "tree should be fully balanced (all leaves at same depth)"
    );
    // 全部可查
    for (k, v) in &items {
        assert_eq!(bt.search(k).unwrap(), Some(*v));
    }
}

/// 完全平衡：100000 个 item（默认 order=256），验证平衡
#[test]
fn phase_0110_bulk_load_tree_fully_balanced_100k() {
    let items = make_sorted_items(100_000);
    let mut bt = BTree::with_default_order();
    bt.bulk_load(items.clone())
        .expect("bulk_load should succeed");
    bt.validate_all_nodes().unwrap();
    assert!(
        all_leaves_at_same_depth(&bt),
        "tree should be fully balanced"
    );
    assert!(bt.height() >= 2, "100k items should have height >= 2");
    // 全部可查
    for (k, v) in &items {
        assert_eq!(bt.search(k).unwrap(), Some(*v));
    }
}

/// 完全平衡：1M item（验证大规模式仍平衡）
#[test]
fn phase_0110_bulk_load_tree_fully_balanced_1m() {
    let items = make_sorted_items(1_000_000);
    let mut bt = BTree::with_default_order();
    let start = std::time::Instant::now();
    bt.bulk_load(items.clone())
        .expect("bulk_load should succeed");
    let elapsed = start.elapsed();
    println!("bulk_load 1M items: {:?}", elapsed);
    bt.validate_all_nodes().unwrap();
    assert!(
        all_leaves_at_same_depth(&bt),
        "tree should be fully balanced"
    );
    // 抽样验证（验证全 1M 太慢，抽 1000 个）
    for i in (0..1_000_000).step_by(1000) {
        assert_eq!(bt.search(&items[i].0).unwrap(), Some(items[i].1));
    }
}

// =====================================================================
//  Phase 1.10 测试 — 10x 性能提升
// =====================================================================

/// 性能：批量导入 vs 逐条插入，验证至少 10x 加速（10k item, order=32）
#[test]
fn phase_0110_bulk_load_at_least_10x_faster_than_sequential() {
    let n = 10_000;
    let items = make_sorted_items(n);

    // 1. 逐条插入计时
    let mut bt_seq = BTree::new(32);
    let seq_start = std::time::Instant::now();
    for (k, v) in items.iter() {
        bt_seq.insert(k.clone(), *v).expect("insert should succeed");
    }
    let seq_elapsed = seq_start.elapsed();

    // 2. 批量导入计时
    let mut bt_bulk = BTree::new(32);
    let bulk_start = std::time::Instant::now();
    bt_bulk
        .bulk_load(items.clone())
        .expect("bulk_load should succeed");
    let bulk_elapsed = bulk_start.elapsed();

    println!(
        "sequential: {:?}, bulk: {:?}, speedup: {:.2}x",
        seq_elapsed,
        bulk_elapsed,
        seq_elapsed.as_nanos() as f64 / bulk_elapsed.as_nanos() as f64
    );

    // 验证两棵树内容一致（中序遍历相等）
    let seq_pairs = bt_seq.in_order_leaf_traverse().unwrap();
    let bulk_pairs = bt_bulk.in_order_leaf_traverse().unwrap();
    assert_eq!(seq_pairs, bulk_pairs, "trees should have same content");

    // 验证至少 10x 加速
    // 注：10x 是规格要求，实测在小数据量下可能波动，但 order=32 + 10k item 应能稳定达到
    let speedup = seq_elapsed.as_nanos() as f64 / bulk_elapsed.as_nanos() as f64;
    assert!(
        speedup >= 10.0,
        "bulk_load should be at least 10x faster than sequential insert, got {:.2}x",
        speedup
    );
}

/// 性能：100k item 批量 vs 逐条（验证大规模式加速更显著）
#[test]
fn phase_0110_bulk_load_100k_speedup() {
    let n = 100_000;
    let items = make_sorted_items(n);

    let mut bt_seq = BTree::with_default_order();
    let seq_start = std::time::Instant::now();
    for (k, v) in items.iter() {
        bt_seq.insert(k.clone(), *v).expect("insert should succeed");
    }
    let seq_elapsed = seq_start.elapsed();

    let mut bt_bulk = BTree::with_default_order();
    let bulk_start = std::time::Instant::now();
    bt_bulk
        .bulk_load(items.clone())
        .expect("bulk_load should succeed");
    let bulk_elapsed = bulk_start.elapsed();

    let speedup = seq_elapsed.as_nanos() as f64 / bulk_elapsed.as_nanos() as f64;
    println!(
        "100k: sequential: {:?}, bulk: {:?}, speedup: {:.2}x",
        seq_elapsed, bulk_elapsed, speedup
    );

    // 内容一致
    let seq_pairs = bt_seq.in_order_leaf_traverse().unwrap();
    let bulk_pairs = bt_bulk.in_order_leaf_traverse().unwrap();
    assert_eq!(seq_pairs, bulk_pairs);

    assert!(
        speedup >= 10.0,
        "100k items: bulk should be >= 10x faster, got {:.2}x",
        speedup
    );
}

// =====================================================================
//  Phase 1.10 测试 — 内存受限分批构建
// =====================================================================

/// 分批构建：1000 个 item，batch_size=100，验证结果与普通 bulk_load 一致
#[test]
fn phase_0110_bulk_load_batched_matches_bulk_load() {
    let items = make_sorted_items(1000);

    let mut bt_bulk = BTree::new(32);
    bt_bulk
        .bulk_load(items.clone())
        .expect("bulk_load should succeed");

    let mut bt_batched = BTree::new(32);
    bt_batched
        .bulk_load_batched(items.clone(), 100)
        .expect("bulk_load_batched should succeed");

    bt_batched.validate_all_nodes().unwrap();

    let bulk_pairs = bt_bulk.in_order_leaf_traverse().unwrap();
    let batched_pairs = bt_batched.in_order_leaf_traverse().unwrap();
    assert_eq!(bulk_pairs, batched_pairs, "batched should match bulk_load");
    assert_eq!(bulk_pairs.len(), 1000);
}

/// 分批构建：1M item，batch_size=10000，验证平衡 + 全部可查
#[test]
fn phase_0110_bulk_load_batched_1m_items() {
    let items = make_sorted_items(1_000_000);
    let mut bt = BTree::with_default_order();
    let start = std::time::Instant::now();
    bt.bulk_load_batched(items.clone(), 10_000)
        .expect("bulk_load_batched should succeed");
    let elapsed = start.elapsed();
    println!("bulk_load_batched 1M items, batch=10k: {:?}", elapsed);

    bt.validate_all_nodes().unwrap();
    assert!(all_leaves_at_same_depth(&bt), "should be fully balanced");

    // 抽样验证
    for i in (0..1_000_000).step_by(1000) {
        assert_eq!(bt.search(&items[i].0).unwrap(), Some(items[i].1));
    }
}

/// 分批构建：batch_size 太小（<2）返回错误
#[test]
fn phase_0110_bulk_load_batched_too_small_batch() {
    let items = make_sorted_items(10);
    let mut bt = BTree::with_default_order();
    let result = bt.bulk_load_batched(items, 1);
    assert!(matches!(result, Err(BTreeError::BulkLoadBatchTooSmall(1))));
}

/// 分批构建：未排序输入返回错误
#[test]
fn phase_0110_bulk_load_batched_unsorted_errors() {
    let mut items = make_sorted_items(100);
    items.swap(50, 51); // 破坏升序
    let mut bt = BTree::with_default_order();
    let result = bt.bulk_load_batched(items, 10);
    assert!(matches!(result, Err(BTreeError::BulkLoadNotSorted { .. })));
}

/// 分批构建：batch_size 不是 order 的倍数，验证跨批打包正确
#[test]
fn phase_0110_bulk_load_batched_cross_boundary() {
    // order=32, batch_size=50（不是 32 的倍数），验证跨边界打包正确
    let items = make_sorted_items(500);
    let mut bt = BTree::new(32);
    bt.bulk_load_batched(items.clone(), 50)
        .expect("bulk_load_batched should succeed");
    bt.validate_all_nodes().unwrap();
    for (k, v) in &items {
        assert_eq!(bt.search(k).unwrap(), Some(*v));
    }
    assert!(all_leaves_at_same_depth(&bt));
}

/// 分批构建：空输入返回 BulkLoadEmpty
#[test]
fn phase_0110_bulk_load_batched_empty_errors() {
    let items: Vec<(Vec<u8>, u32)> = Vec::new();
    let mut bt = BTree::with_default_order();
    let result = bt.bulk_load_batched(items, 100);
    assert!(matches!(result, Err(BTreeError::BulkLoadEmpty)));
}

// =====================================================================
//  Phase 1.10 测试 — from_sorted_iter 便捷构造
// =====================================================================

/// from_sorted_iter：便捷构造函数，等价于 new + bulk_load
#[test]
fn phase_0110_from_sorted_iter_basic() {
    let items = make_sorted_items(500);
    let bt = BTree::from_sorted_iter(32, items.clone()).expect("from_sorted_iter should succeed");
    bt.validate_all_nodes().unwrap();
    for (k, v) in &items {
        assert_eq!(bt.search(k).unwrap(), Some(*v));
    }
}

// =====================================================================
//  Phase 1.10 测试 — 与逐条插入的内容等价性（对照测试）
// =====================================================================

/// 对照：bulk_load 与逐条插入的结果完全一致（1000 item, order=16）
#[test]
fn phase_0110_bulk_load_equivalent_to_sequential_insert() {
    let items = make_sorted_items(1000);

    // 逐条插入
    let mut bt_seq = BTree::new(16);
    for (k, v) in items.iter() {
        bt_seq.insert(k.clone(), *v).unwrap();
    }

    // 批量导入
    let mut bt_bulk = BTree::new(16);
    bt_bulk.bulk_load(items.clone()).unwrap();

    // 内容一致
    let seq_pairs = bt_seq.in_order_leaf_traverse().unwrap();
    let bulk_pairs = bt_bulk.in_order_leaf_traverse().unwrap();
    assert_eq!(seq_pairs.len(), bulk_pairs.len());
    for (i, (s, b)) in seq_pairs.iter().zip(bulk_pairs.iter()).enumerate() {
        assert_eq!(s, b, "mismatch at index {}", i);
    }

    // 全部可查（两棵树）
    for (k, v) in &items {
        assert_eq!(bt_seq.search(k).unwrap(), Some(*v));
        assert_eq!(bt_bulk.search(k).unwrap(), Some(*v));
    }

    // 批量树应完全平衡
    assert!(all_leaves_at_same_depth(&bt_bulk));
}

/// 对照：随机乱序数据排序后 bulk_load 与 BTreeMap 等价
#[test]
fn phase_0110_bulk_load_matches_btreemap_reference() {
    // 生成 5000 个随机 u64 key，去重排序
    let mut keys: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut rng_state: u64 = 0x1234_5678_9ABC_DEF0;
    while keys.len() < 5000 {
        // XorShift64
        let mut x = rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        rng_state = x;
        keys.insert(x);
    }
    let mut sorted_keys: Vec<u64> = keys.into_iter().collect();
    sorted_keys.sort_unstable();

    // 构造 sorted items
    let items: Vec<(Vec<u8>, u32)> = sorted_keys
        .iter()
        .enumerate()
        .map(|(i, &k)| (k.to_be_bytes().to_vec(), (i % 65536) as u32))
        .collect();

    // BTreeMap 参考
    let mut ref_map: std::collections::BTreeMap<Vec<u8>, u32> = std::collections::BTreeMap::new();
    for (k, v) in &items {
        ref_map.insert(k.clone(), *v);
    }

    // bulk_load
    let mut bt = BTree::with_default_order();
    bt.bulk_load(items.clone()).unwrap();
    bt.validate_all_nodes().unwrap();

    // 中序遍历 == BTreeMap.iter()
    let bulk_pairs = bt.in_order_leaf_traverse().unwrap();
    let ref_pairs: Vec<(Vec<u8>, u32)> = ref_map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    assert_eq!(bulk_pairs, ref_pairs);

    // 全部可查
    for (k, v) in &items {
        assert_eq!(bt.search(k).unwrap(), Some(*v));
    }
}

// =====================================================================
//  Phase 1.10 测试 — 范围扫描兼容性
// =====================================================================

/// bulk_load 后的范围扫描与逐条插入的范围扫描结果一致
#[test]
fn phase_0110_bulk_load_range_scan_compatible() {
    use std::ops::Bound;
    let items = make_sorted_items_offset(1000, 0);

    let mut bt_seq = BTree::new(16);
    for (k, v) in items.iter() {
        bt_seq.insert(k.clone(), *v).unwrap();
    }

    let mut bt_bulk = BTree::new(16);
    bt_bulk.bulk_load(items.clone()).unwrap();

    // 范围 [100, 200)
    let lower = Bound::Included(100i64.to_be_bytes().to_vec());
    let upper = Bound::Excluded(200i64.to_be_bytes().to_vec());
    let seq_result = bt_seq
        .range_scan(
            lower.as_ref().map(|k| k.as_slice()),
            upper.as_ref().map(|k| k.as_slice()),
        )
        .unwrap();
    let bulk_result = bt_bulk
        .range_scan(
            lower.as_ref().map(|k| k.as_slice()),
            upper.as_ref().map(|k| k.as_slice()),
        )
        .unwrap();
    assert_eq!(seq_result, bulk_result);
    assert_eq!(seq_result.len(), 100); // 100..199 = 100 个
}

// =====================================================================
//  Phase 1.10 测试 — 借键补足最后一个叶子
// =====================================================================

/// 借键：构造最后一个叶子 < order/2 的场景，验证 bulk_load 借键后所有叶子 >= order/2
#[test]
fn phase_0110_bulk_load_last_leaf_borrow_keys() {
    // order=16，min_keys=8
    // 25 个 item：1 个满叶子(16) + 1 个 9 个 key 的叶子 = 2 个叶子
    // 17 个 item：1 个满叶子(16) + 1 个 1 个 key 的叶子（<8，需借键）
    let items = make_sorted_items(17);
    let mut bt = BTree::new(16);
    bt.bulk_load(items).expect("bulk_load should succeed");
    bt.validate_all_nodes().unwrap();

    // 遍历所有叶子，验证 keys.len() >= 8（min_keys）
    fn check_leaves_min_keys(bt: &BTree, min_keys: usize) {
        let root = bt.root_page_id();
        fn dfs(bt: &BTree, page_id: u32, min_keys: usize, violations: &mut Vec<usize>) {
            let node = bt.read_node_public(page_id).unwrap();
            if node.is_leaf() {
                if node.keys.len() < min_keys {
                    violations.push(node.keys.len());
                }
                return;
            }
            for &child in &node.children {
                dfs(bt, child, min_keys, violations);
            }
        }
        let mut violations = Vec::new();
        dfs(bt, root, min_keys, &mut violations);
        assert!(
            violations.is_empty(),
            "leaves with < min_keys: {:?}",
            violations
        );
    }
    check_leaves_min_keys(&bt, 8);
}

/// 借键：大规模场景下所有叶子至少半满
#[test]
fn phase_0110_bulk_load_all_leaves_at_least_half_full_10k() {
    let order = 32;
    let min_keys = order / 2; // 16
    let items = make_sorted_items(10_000);
    let mut bt = BTree::new(order);
    bt.bulk_load(items).expect("bulk_load should succeed");
    bt.validate_all_nodes().unwrap();

    let root = bt.root_page_id();
    fn dfs(bt: &BTree, page_id: u32, min_keys: usize, violations: &mut Vec<(u32, usize)>) {
        let node = bt.read_node_public(page_id).unwrap();
        if node.is_leaf() {
            if node.keys.len() < min_keys {
                violations.push((page_id, node.keys.len()));
            }
            return;
        }
        for &child in &node.children {
            dfs(bt, child, min_keys, violations);
        }
    }
    let mut violations = Vec::new();
    dfs(&bt, root, min_keys, &mut violations);
    assert!(
        violations.is_empty(),
        "leaves with < min_keys={}: {:?}",
        min_keys,
        violations
    );
}

// =====================================================================
//  Phase 1.10 测试 — 幂等性 / 重复调用
// =====================================================================

/// 重复调用 bulk_load：第二次调用完全替换第一次的结果
#[test]
fn phase_0110_bulk_load_idempotent_replace() {
    let items1 = make_sorted_items(100);
    let items2 = make_sorted_items_offset(200, 1000); // 不同的 key 范围

    let mut bt = BTree::with_default_order();
    bt.bulk_load(items1.clone()).unwrap();
    let count1 = bt.node_count();
    assert_eq!(bt.in_order_leaf_traverse().unwrap().len(), 100);

    // 第二次 bulk_load 应清空旧的 pages
    bt.bulk_load(items2.clone()).unwrap();
    let count2 = bt.node_count();
    assert_eq!(bt.in_order_leaf_traverse().unwrap().len(), 200);

    // 第二次的 key 应全部可查
    for (k, v) in &items2 {
        assert_eq!(bt.search(k).unwrap(), Some(*v));
    }

    // 第一次的 key 应全部找不到（已被替换）
    for (k, _) in &items1 {
        assert_eq!(bt.search(k).unwrap(), None);
    }

    // node_count 不应叠加（pages 已被 clear）
    assert!(
        count2 < count1 * 3,
        "pages should be cleared, count2={}, count1={}",
        count2,
        count1
    );
}

// =====================================================================
//  Phase 1.10 测试 — 自定义 order 兼容性
// =====================================================================

/// 多种 order（3, 4, 8, 16, 32, 64, 128, 256）下 bulk_load 都能正确构建
#[test]
fn phase_0110_bulk_load_various_orders() {
    for &order in &[3usize, 4, 8, 16, 32, 64, 128, 256] {
        let items = make_sorted_items(500);
        let mut bt = BTree::new(order);
        bt.bulk_load(items.clone())
            .unwrap_or_else(|e| panic!("bulk_load order={} failed: {:?}", order, e));
        bt.validate_all_nodes()
            .unwrap_or_else(|e| panic!("validate order={} failed: {:?}", order, e));
        for (k, v) in &items {
            assert_eq!(
                bt.search(k).unwrap(),
                Some(*v),
                "order={} key={:?} not found",
                order,
                k
            );
        }
        assert!(
            all_leaves_at_same_depth(&bt),
            "order={} not balanced",
            order
        );
    }
}

/// 显式 BTREE_DEFAULT_ORDER 常量校验
#[test]
fn phase_0110_default_order_constant() {
    assert_eq!(
        BTREE_DEFAULT_ORDER, 256,
        "BTREE_DEFAULT_ORDER should be 256"
    );
}
