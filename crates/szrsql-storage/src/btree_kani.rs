//! B-Tree Kani 形式化验证 (Phase 1.12)
//!
//! 本文件包含两部分：
//! 1. **Kani 证明 harness**（`#[cfg(kani)]` 门控）— 使用 `kani::any()` 符号执行 +
//!    `kani::assert!` / `kani::cover!` 验证关键路径的无 panic + 不变性。
//!    仅在 Linux/macOS + Kani Rust Verifier 环境下编译运行。
//! 2. **等价 property-based 测试**（`#[cfg(test)]` 门控）— 使用 `proptest` 随机输入
//!    验证相同性质，作为 Windows 环境下的可运行替代验证。
//!
//! 验证目标（对应 SzRSQL实施进度.md Phase 1.12 验证标准）：
//! - **节点分裂** (`BTreeNode::split`)：分裂后左右节点结构合法、key 全保留、promoted_key 正确
//! - **节点合并** (`BTreeNode::merge`)：合并后节点结构合法、key 全保留、merge 是 split 的逆操作
//! - **key 比较** (`compare_keys` / `encode_i64_key` / `decode_i64_key`)：编码/解码 round-trip、
//!   比较结果与 i64 数值顺序一致
//! - **插入路径** (`BTree::insert` + `split_upwards`)：插入后 validate_all_nodes 通过、
//!   search 能找到新 key
//! - **删除路径** (`BTree::delete` + `rebalance_upwards`)：删除后 validate_all_nodes 通过、
//!   search 找不到已删 key
//!
//! 设计参考：`SzRSQL技术实现方案.md` 9.5 节 (B-Tree 存储引擎)
//! 验证方法：Kani Rust Verifier (CBMC 后端) — bounded model checking

use crate::btree::{compare_keys, decode_i64_key, encode_i64_key, BTree, BTreeNode, NodeType};

// =====================================================================
//  Kani 证明 harness（仅在 Kani 环境下编译）
// =====================================================================
#[cfg(kani)]
mod kani_proofs {
    use super::*;
    use kani::cover;

    /// 验证 `encode_i64_key` → `decode_i64_key` round-trip 对任意 i64 成立
    ///
    /// 性质：∀ v ∈ i64. decode_i64_key(encode_i64_key(v)) == Ok(v)
    /// 覆盖：负数、零、正数、i64::MIN、i64::MAX
    #[kani::proof]
    fn verify_encode_decode_i64_roundtrip() {
        let v: i64 = kani::any();
        let encoded = encode_i64_key(v);
        let decoded = decode_i64_key(&encoded).expect("decode should succeed for 8-byte key");
        kani::assert!(decoded == v, "encode/decode round-trip preserves value");
    }

    /// 验证 `encode_i64_key` 输出长度恒为 8
    #[kani::proof]
    fn verify_encode_i64_key_length() {
        let v: i64 = kani::any();
        let encoded = encode_i64_key(v);
        kani::assert!(encoded.len() == 8, "encoded i64 key must be 8 bytes");
    }

    /// 验证 `compare_keys` 与 i64 数值顺序一致
    ///
    /// 性质：∀ a, b ∈ i64. compare_keys(encode(a), encode(b)) == a.cmp(&b)
    #[kani::proof]
    fn verify_compare_keys_matches_i64_order() {
        let a: i64 = kani::any();
        let b: i64 = kani::any();
        let ka = encode_i64_key(a);
        let kb = encode_i64_key(b);
        let cmp_bytes = compare_keys(&ka, &kb);
        let cmp_i64 = a.cmp(&b);
        kani::assert!(
            cmp_bytes == cmp_i64,
            "compare_keys must match i64 numeric ordering"
        );
    }

    /// 验证 `decode_i64_key` 对错误长度输入返回 Err（无 panic）
    ///
    /// 性质：∀ len ∈ 0..=16, len ≠ 8. decode_i64_key(任意 len 字节) == Err
    #[kani::proof]
    fn verify_decode_i64_key_rejects_invalid_length() {
        let len: usize = kani::any();
        kani::assume(len <= 16);
        kani::assume(len != 8);
        let buf: Vec<u8> = (0..len).map(|_| kani::any()).collect();
        let result = decode_i64_key(&buf);
        kani::assert!(result.is_err(), "decode must reject non-8-byte input");
    }

    /// 验证 `BTreeNode::search_key` 在合法升序 keys 上无 panic 且返回正确结果
    ///
    /// 性质：对于升序 keys 序列，search_key 要么找到 (Some(idx), idx)，
    /// 要么返回插入位置 (None, idx) ∈ [0, keys.len()]
    #[kani::proof]
    fn verify_search_key_no_panic_on_sorted_keys() {
        // 构造一个小的升序 keys 序列（最多 4 个 key，每个 8 字节）
        let n: usize = kani::any();
        kani::assume(n <= 4);
        let mut keys: Vec<Vec<u8>> = Vec::with_capacity(n);
        let mut prev: i64 = i64::MIN;
        for _ in 0..n {
            let v: i64 = kani::any();
            // 保证严格升序（防止 binary_search_by panic）
            kani::assume(v > prev);
            keys.push(encode_i64_key(v));
            prev = v;
        }
        let node = BTreeNode {
            page_id: 1,
            node_type: NodeType::Leaf,
            keys,
            children: Vec::new(),
            tuple_ids: Vec::new(),
            next_sibling: 0,
            prev_sibling: 0,
            parent: 0,
        };
        // 任意查询 key
        let q: i64 = kani::any();
        let qk = encode_i64_key(q);
        let (found, insert_pos) = node.search_key(&qk);
        // 插入位置必须在合法范围
        kani::assert!(insert_pos <= node.keys.len(), "insert_pos within bounds");
        // 如果找到，索引必须有效
        if let Some(idx) = found {
            kani::assert!(idx < node.keys.len(), "found index within bounds");
            kani::assert!(node.keys[idx] == qk, "found key matches query");
        }
        // 覆盖：找到与未找到两条路径
        cover!(found.is_some());
        cover!(found.is_none());
    }

    /// 验证 `BTreeNode::validate` 对合法 Leaf 节点返回 Ok
    ///
    /// 性质：合法 Leaf（keys 升序、tuple_ids.len()==keys.len()、children 空）→ validate == Ok
    #[kani::proof]
    fn verify_validate_leaf_ok() {
        let n: usize = kani::any();
        kani::assume(n >= 1 && n <= 4);
        let mut keys: Vec<Vec<u8>> = Vec::with_capacity(n);
        let mut tuple_ids: Vec<u16> = Vec::with_capacity(n);
        let mut prev: i64 = i64::MIN;
        for i in 0..n {
            let v: i64 = kani::any();
            kani::assume(v > prev);
            keys.push(encode_i64_key(v));
            tuple_ids.push(i as u16);
            prev = v;
        }
        let node = BTreeNode {
            page_id: 1,
            node_type: NodeType::Leaf,
            keys,
            children: Vec::new(),
            tuple_ids,
            next_sibling: 0,
            prev_sibling: 0,
            parent: 0,
        };
        let result = node.validate();
        kani::assert!(result.is_ok(), "valid leaf must pass validate");
    }

    /// 验证 `BTreeNode::split` 对 Leaf 节点：key 全保留 + 左右节点合法
    ///
    /// 性质：split 后 left.keys ∪ right.keys == 原 keys（多重集等价），
    ///       且 left.next_sibling == right.page_id，right.prev_sibling == left.page_id
    #[kani::proof]
    fn verify_split_leaf_preserves_keys() {
        // 构造 2~4 个升序 key 的叶子
        let n: usize = kani::any();
        kani::assume(n >= 2 && n <= 4);
        let mut keys: Vec<Vec<u8>> = Vec::with_capacity(n);
        let mut tuple_ids: Vec<u16> = Vec::with_capacity(n);
        let mut prev: i64 = i64::MIN;
        for i in 0..n {
            let v: i64 = kani::any();
            kani::assume(v > prev);
            keys.push(encode_i64_key(v));
            tuple_ids.push(i as u16);
            prev = v;
        }
        let mut node = BTreeNode {
            page_id: 10,
            node_type: NodeType::Leaf,
            keys: keys.clone(),
            children: Vec::new(),
            tuple_ids: tuple_ids.clone(),
            next_sibling: 0,
            prev_sibling: 0,
            parent: 0,
        };
        let (left, right, promoted) = node.split(1, 2).expect("split should succeed for n>=2");
        // 1. 左右节点类型正确
        kani::assert!(left.is_leaf(), "left is leaf");
        kani::assert!(right.is_leaf(), "right is leaf");
        // 2. left.next_sibling == right.page_id, right.prev_sibling == left.page_id
        kani::assert!(left.next_sibling == right.page_id, "sibling chain forward");
        kani::assert!(right.prev_sibling == left.page_id, "sibling chain backward");
        // 3. promoted key 等于 right.keys[0]（Leaf 分裂：mid key 保留在 right）
        kani::assert!(!right.keys.is_empty(), "right has at least 1 key");
        kani::assert!(
            right.keys[0] == promoted,
            "promoted key equals right's first key"
        );
        // 4. key 全保留：left.keys.len() + right.keys.len() == n
        kani::assert!(
            left.keys.len() + right.keys.len() == n,
            "key count preserved after split"
        );
        // 5. 左右节点各自 validate 通过
        kani::assert!(left.validate().is_ok(), "left validates");
        kani::assert!(right.validate().is_ok(), "right validates");
        // 6. 左右节点 key 严格升序（由 validate 保证）
        // 7. 左右节点 tuple_ids.len() == keys.len()
        kani::assert!(
            left.tuple_ids.len() == left.keys.len(),
            "left tuple_ids count matches keys"
        );
        kani::assert!(
            right.tuple_ids.len() == right.keys.len(),
            "right tuple_ids count matches keys"
        );
    }

    /// 验证 `BTreeNode::split` 对 Internal 节点：children 数量守恒
    ///
    /// 性质：split 后 left.children.len() + right.children.len() == 原 children.len()
    ///       且 left.keys.len() + right.keys.len() + 1 == 原 keys.len()（mid key 提升）
    #[kani::proof]
    fn verify_split_internal_preserves_structure() {
        let n: usize = kani::any();
        kani::assume(n >= 2 && n <= 4);
        let mut keys: Vec<Vec<u8>> = Vec::with_capacity(n);
        let mut prev: i64 = i64::MIN;
        for _ in 0..n {
            let v: i64 = kani::any();
            kani::assume(v > prev);
            keys.push(encode_i64_key(v));
            prev = v;
        }
        // Internal 节点 children.len() == keys.len() + 1
        let children: Vec<u32> = (0..=n as u32).collect();
        let mut node = BTreeNode {
            page_id: 10,
            node_type: NodeType::Internal,
            keys: keys.clone(),
            children: children.clone(),
            tuple_ids: Vec::new(),
            next_sibling: 0,
            prev_sibling: 0,
            parent: 0,
        };
        let (left, right, promoted) = node.split(1, 2).expect("split internal should succeed");
        kani::assert!(left.is_internal(), "left is internal");
        kani::assert!(right.is_internal(), "right is internal");
        // children 守恒
        kani::assert!(
            left.children.len() + right.children.len() == children.len(),
            "children count preserved"
        );
        // keys 守恒（mid key 提升到父节点，不在 left/right 中）
        kani::assert!(
            left.keys.len() + right.keys.len() + 1 == n,
            "keys count: left + right + 1 (promoted) == original"
        );
        // promoted key 不在 left 或 right 的 keys 中
        kani::assert!(
            !left.keys.iter().any(|k| k == &promoted),
            "promoted not in left"
        );
        kani::assert!(
            !right.keys.iter().any(|k| k == &promoted),
            "promoted not in right"
        );
        // 左右节点 validate 通过
        kani::assert!(left.validate().is_ok(), "left validates");
        kani::assert!(right.validate().is_ok(), "right validates");
        // Internal 结构：children.len() == keys.len() + 1
        kani::assert!(
            left.children.len() == left.keys.len() + 1,
            "left internal structure"
        );
        kani::assert!(
            right.children.len() == right.keys.len() + 1,
            "right internal structure"
        );
    }

    /// 验证 `BTreeNode::split` 对 keys.len() < 2 返回 Err（无 panic）
    #[kani::proof]
    fn verify_split_rejects_too_few_keys() {
        let node = BTreeNode {
            page_id: 1,
            node_type: NodeType::Leaf,
            keys: vec![encode_i64_key(42)], // 仅 1 个 key
            children: Vec::new(),
            tuple_ids: vec![0],
            next_sibling: 0,
            prev_sibling: 0,
            parent: 0,
        };
        let mut n = node;
        let result = n.split(1, 2);
        kani::assert!(result.is_err(), "split must reject keys.len() < 2");
    }

    /// 验证 `BTreeNode::merge` 是 `split` 的逆操作（Leaf round-trip）
    ///
    /// 性质：merge(splitted_left, splitted_right, None) 的 keys 与原节点 keys 等价
    #[kani::proof]
    fn verify_merge_is_inverse_of_split_leaf() {
        let n: usize = kani::any();
        kani::assume(n >= 2 && n <= 4);
        let mut keys: Vec<Vec<u8>> = Vec::with_capacity(n);
        let mut tuple_ids: Vec<u16> = Vec::with_capacity(n);
        let mut prev: i64 = i64::MIN;
        for i in 0..n {
            let v: i64 = kani::any();
            kani::assume(v > prev);
            keys.push(encode_i64_key(v));
            tuple_ids.push(i as u16);
            prev = v;
        }
        let mut node = BTreeNode {
            page_id: 10,
            node_type: NodeType::Leaf,
            keys: keys.clone(),
            children: Vec::new(),
            tuple_ids: tuple_ids.clone(),
            next_sibling: 0,
            prev_sibling: 0,
            parent: 0,
        };
        let (left, right, _promoted) = node.split(10, 20).expect("split");
        // merge(left, right) 应恢复原 keys（顺序保持）
        let merged = left.merge(right, None).expect("merge");
        kani::assert!(merged.is_leaf(), "merged is leaf");
        kani::assert!(merged.keys.len() == n, "merged key count matches original");
        kani::assert!(merged.validate().is_ok(), "merged validates");
        // keys 顺序与原节点一致
        for i in 0..n {
            kani::assert!(merged.keys[i] == keys[i], "merged key order preserved");
        }
    }

    /// 验证 `BTreeNode::merge` 拒绝非相邻节点（无 panic）
    #[kani::proof]
    fn verify_merge_rejects_non_adjacent() {
        let left = BTreeNode {
            page_id: 1,
            node_type: NodeType::Leaf,
            keys: vec![encode_i64_key(1)],
            children: Vec::new(),
            tuple_ids: vec![0],
            next_sibling: 99, // 不等于 right.page_id
            prev_sibling: 0,
            parent: 0,
        };
        let right = BTreeNode {
            page_id: 2,
            node_type: NodeType::Leaf,
            keys: vec![encode_i64_key(2)],
            children: Vec::new(),
            tuple_ids: vec![1],
            next_sibling: 0,
            prev_sibling: 1,
            parent: 0,
        };
        let result = left.merge(right, None);
        kani::assert!(result.is_err(), "merge must reject non-adjacent nodes");
    }

    /// 验证 `BTreeNode::merge` 拒绝不同类型节点
    #[kani::proof]
    fn verify_merge_rejects_different_types() {
        let left = BTreeNode {
            page_id: 1,
            node_type: NodeType::Leaf,
            keys: vec![encode_i64_key(1)],
            children: Vec::new(),
            tuple_ids: vec![0],
            next_sibling: 2,
            prev_sibling: 0,
            parent: 0,
        };
        let right = BTreeNode {
            page_id: 2,
            node_type: NodeType::Internal, // 不同类型
            keys: vec![encode_i64_key(2)],
            children: vec![3, 4],
            tuple_ids: Vec::new(),
            next_sibling: 0,
            prev_sibling: 1,
            parent: 0,
        };
        let result = left.merge(right, None);
        kani::assert!(result.is_err(), "merge must reject different types");
    }

    /// 验证 `BTree::insert` 后 `validate_all_nodes` 通过（小规模符号执行）
    ///
    /// 性质：对空树执行有限次 insert 后，validate_all_nodes() == Ok(())
    #[kani::proof]
    #[kani::unwind(8)] // 限制递归深度，防止路径爆炸
    fn verify_insert_preserves_invariants() {
        let mut bt = BTree::with_default_order();
        // 插入 3 个符号 key（避免大规模状态爆炸）
        for _ in 0..3 {
            let v: i64 = kani::any();
            let key = encode_i64_key(v);
            // insert 可能返回 Err（重复 key），忽略错误
            let _ = bt.insert(key, 1);
        }
        // 不变量：所有节点结构合法
        let validate_result = bt.validate_all_nodes();
        kani::assert!(validate_result.is_ok(), "validate_all_nodes after inserts");
    }

    /// 验证 `BTree::search` 对已插入 key 一定找到
    ///
    /// 性质：insert(k, tid) 成功后，search(k) == Ok(Some(tid))
    #[kani::proof]
    #[kani::unwind(8)]
    fn verify_insert_then_search_finds_key() {
        let mut bt = BTree::with_default_order();
        let v: i64 = kani::any();
        let key = encode_i64_key(v);
        let insert_result = bt.insert(key.clone(), 42);
        if insert_result.is_ok() {
            let search_result = bt.search(&key).expect("search should not error");
            kani::assert!(
                search_result == Some(42),
                "search finds inserted key with correct tid"
            );
        }
        kani::assert!(bt.validate_all_nodes().is_ok(), "tree still valid");
    }

    /// 验证 `BTree::delete` 后 key 不再被 search 找到
    ///
    /// 性质：delete(k) 成功后，search(k) == Ok(None)
    #[kani::proof]
    #[kani::unwind(8)]
    fn verify_delete_removes_key() {
        let mut bt = BTree::with_default_order();
        let v: i64 = kani::any();
        let key = encode_i64_key(v);
        // 先插入
        if bt.insert(key.clone(), 7).is_ok() {
            // 再删除
            let delete_result = bt.delete(&key).expect("delete should not error");
            kani::assert!(delete_result, "delete returns true for existing key");
            // search 应返回 None
            let search_result = bt.search(&key).expect("search should not error");
            kani::assert!(search_result.is_none(), "deleted key not found");
            // 树仍合法
            kani::assert!(bt.validate_all_nodes().is_ok(), "tree valid after delete");
        }
    }

    /// 验证 `BTree::insert` + `BTree::delete` round-trip 恢复空树状态
    ///
    /// 性质：insert(k) 后 delete(k) 应使树回到插入前状态（节点数可能不变但 key 不在）
    #[kani::proof]
    #[kani::unwind(8)]
    fn verify_insert_delete_roundtrip() {
        let mut bt = BTree::with_default_order();
        let v: i64 = kani::any();
        let key = encode_i64_key(v);
        let _ = bt.insert(key.clone(), 1);
        let _ = bt.delete(&key);
        // 树仍合法
        kani::assert!(
            bt.validate_all_nodes().is_ok(),
            "tree valid after round-trip"
        );
        // search 找不到
        let sr = bt.search(&key).expect("search");
        kani::assert!(sr.is_none(), "key not found after round-trip");
    }

    /// 覆盖验证：BTree 在多次操作后仍保持合法
    ///
    /// 性质：任意交错 insert/delete 序列后，validate_all_nodes() == Ok(())
    #[kani::proof]
    #[kani::unwind(8)]
    fn verify_mixed_operations_preserve_invariants() {
        let mut bt = BTree::with_default_order();
        // 3 次 insert + 2 次 delete，全部符号化
        for i in 0..3 {
            let v: i64 = kani::any();
            let _ = bt.insert(encode_i64_key(v), i as u16);
        }
        for _ in 0..2 {
            let v: i64 = kani::any();
            let _ = bt.delete(&encode_i64_key(v));
        }
        kani::assert!(
            bt.validate_all_nodes().is_ok(),
            "tree valid after mixed ops"
        );
    }
}

// =====================================================================
//  等价 property-based 测试（proptest，全平台可运行）
//  作为 Kani 证明的 Windows 环境可运行替代验证：
//  - proptest 用随机输入覆盖相同性质
//  - 不是穷尽证明，但能以高概率发现违反
//  - 与 Kani 证明目标一一对应（见每个测试的注释）
// =====================================================================
#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::collection::vec as prop_vec;
    use proptest::prelude::*;

    /// 对应 Kani proof: `verify_encode_decode_i64_roundtrip`
    /// 性质：∀ v ∈ i64. decode_i64_key(encode_i64_key(v)) == Ok(v)
    #[test]
    fn proptest_encode_decode_i64_roundtrip() {
        proptest!(|(v in any::<i64>())| {
            let encoded = encode_i64_key(v);
            prop_assert_eq!(encoded.len(), 8);
            let decoded = decode_i64_key(&encoded).expect("decode 8-byte key");
            prop_assert_eq!(decoded, v);
        });
    }

    /// 对应 Kani proof: `verify_compare_keys_matches_i64_order`
    /// 性质：∀ a, b ∈ i64. compare_keys(encode(a), encode(b)) == a.cmp(&b)
    #[test]
    fn proptest_compare_keys_matches_i64_order() {
        proptest!(|(a in any::<i64>(), b in any::<i64>())| {
            let ka = encode_i64_key(a);
            let kb = encode_i64_key(b);
            let cmp_bytes = compare_keys(&ka, &kb);
            let cmp_i64 = a.cmp(&b);
            prop_assert_eq!(cmp_bytes, cmp_i64);
        });
    }

    /// 对应 Kani proof: `verify_decode_i64_key_rejects_invalid_length`
    /// 性质：∀ len ∈ 0..=16, len ≠ 8. decode_i64_key(任意 len 字节) == Err
    #[test]
    fn proptest_decode_i64_key_rejects_invalid_length() {
        proptest!(|(bytes in prop_vec(any::<u8>(), 0..=16usize))| {
            if bytes.len() != 8 {
                let result = decode_i64_key(&bytes);
                prop_assert!(result.is_err(), "decode must reject non-8-byte input");
            }
        });
    }

    /// 对应 Kani proof: `verify_search_key_no_panic_on_sorted_keys`
    /// 性质：search_key 在升序 keys 上无 panic 且返回合法结果
    #[test]
    fn proptest_search_key_no_panic_sorted() {
        proptest!(|(vals in prop_vec(any::<i64>(), 0..=8))| {
            let mut sorted: Vec<i64> = vals;
            sorted.sort();
            sorted.dedup();
            let keys: Vec<Vec<u8>> = sorted.iter().map(|v| encode_i64_key(*v)).collect();
            let node = BTreeNode {
                page_id: 1,
                node_type: NodeType::Leaf,
                keys,
                children: Vec::new(),
                tuple_ids: Vec::new(),
                next_sibling: 0,
                prev_sibling: 0,
                parent: 0,
            };
            for v in &sorted {
                let k = encode_i64_key(*v);
                let (found, pos) = node.search_key(&k);
                prop_assert!(found.is_some(), "existing key must be found");
                prop_assert!(pos < node.keys.len());
            }
            // 查询不存在的 key
            let q = encode_i64_key(i64::MAX);
            let (found, pos) = node.search_key(&q);
            prop_assert!(pos <= node.keys.len());
            if !node.keys.is_empty() && q > node.keys[node.keys.len()-1] {
                prop_assert!(found.is_none());
                prop_assert_eq!(pos, node.keys.len());
            }
        });
    }

    /// 对应 Kani proof: `verify_validate_leaf_ok`
    /// 性质：合法 Leaf 节点 validate == Ok
    #[test]
    fn proptest_validate_leaf_ok() {
        proptest!(|(vals in prop_vec(any::<i64>(), 1..=8))| {
            let mut sorted: Vec<i64> = vals;
            sorted.sort();
            sorted.dedup();
            let keys: Vec<Vec<u8>> = sorted.iter().map(|v| encode_i64_key(*v)).collect();
            let tuple_ids: Vec<u16> = (0..keys.len() as u16).collect();
            let node = BTreeNode {
                page_id: 1,
                node_type: NodeType::Leaf,
                keys,
                children: Vec::new(),
                tuple_ids,
                next_sibling: 0,
                prev_sibling: 0,
                parent: 0,
            };
            prop_assert!(node.validate().is_ok());
        });
    }

    /// 对应 Kani proof: `verify_split_leaf_preserves_keys`
    /// 性质：split 后 key 全保留 + 左右节点合法 + sibling 链正确
    #[test]
    fn proptest_split_leaf_preserves_keys() {
        proptest!(|(vals in prop_vec(any::<i64>(), 2..=8))| {
            let mut sorted: Vec<i64> = vals;
            sorted.sort();
            sorted.dedup();
            // 需要 >= 2 个 key 才能分裂
            if sorted.len() < 2 {
                return Ok(());
            }
            let keys: Vec<Vec<u8>> = sorted.iter().map(|v| encode_i64_key(*v)).collect();
            let tuple_ids: Vec<u16> = (0..keys.len() as u16).collect();
            let n = keys.len();
            let mut node = BTreeNode {
                page_id: 10,
                node_type: NodeType::Leaf,
                keys: keys.clone(),
                children: Vec::new(),
                tuple_ids: tuple_ids.clone(),
                next_sibling: 0,
                prev_sibling: 0,
                parent: 0,
            };
            let (left, right, promoted) = node.split(1, 2).expect("split");
            prop_assert!(left.is_leaf());
            prop_assert!(right.is_leaf());
            prop_assert_eq!(left.next_sibling, right.page_id);
            prop_assert_eq!(right.prev_sibling, left.page_id);
            prop_assert!(!right.keys.is_empty());
            prop_assert_eq!(&right.keys[0], &promoted);
            prop_assert_eq!(left.keys.len() + right.keys.len(), n);
            prop_assert!(left.validate().is_ok());
            prop_assert!(right.validate().is_ok());
            prop_assert_eq!(left.tuple_ids.len(), left.keys.len());
            prop_assert_eq!(right.tuple_ids.len(), right.keys.len());
        });
    }

    /// 对应 Kani proof: `verify_split_internal_preserves_structure`
    /// 性质：split Internal 后 children 守恒 + keys 守恒（+1 promoted）
    #[test]
    fn proptest_split_internal_preserves_structure() {
        proptest!(|(vals in prop_vec(any::<i64>(), 2..=8))| {
            let mut sorted: Vec<i64> = vals;
            sorted.sort();
            sorted.dedup();
            if sorted.len() < 2 {
                return Ok(());
            }
            let n = sorted.len();
            let keys: Vec<Vec<u8>> = sorted.iter().map(|v| encode_i64_key(*v)).collect();
            let children: Vec<u32> = (0..=n as u32).collect();
            let mut node = BTreeNode {
                page_id: 10,
                node_type: NodeType::Internal,
                keys: keys.clone(),
                children: children.clone(),
                tuple_ids: Vec::new(),
                next_sibling: 0,
                prev_sibling: 0,
                parent: 0,
            };
            let (left, right, promoted) = node.split(1, 2).expect("split internal");
            prop_assert!(left.is_internal());
            prop_assert!(right.is_internal());
            prop_assert_eq!(left.children.len() + right.children.len(), children.len());
            prop_assert_eq!(left.keys.len() + right.keys.len() + 1, n);
            prop_assert!(!left.keys.iter().any(|k| k == &promoted));
            prop_assert!(!right.keys.iter().any(|k| k == &promoted));
            prop_assert!(left.validate().is_ok());
            prop_assert!(right.validate().is_ok());
            prop_assert_eq!(left.children.len(), left.keys.len() + 1);
            prop_assert_eq!(right.children.len(), right.keys.len() + 1);
        });
    }

    /// 对应 Kani proof: `verify_split_rejects_too_few_keys`
    #[test]
    fn proptest_split_rejects_too_few_keys() {
        let node = BTreeNode {
            page_id: 1,
            node_type: NodeType::Leaf,
            keys: vec![encode_i64_key(42)],
            children: Vec::new(),
            tuple_ids: vec![0],
            next_sibling: 0,
            prev_sibling: 0,
            parent: 0,
        };
        let mut n = node;
        assert!(n.split(1, 2).is_err());
    }

    /// 对应 Kani proof: `verify_merge_is_inverse_of_split_leaf`
    /// 性质：merge(split(node)) 恢复原 keys
    #[test]
    fn proptest_merge_is_inverse_of_split_leaf() {
        proptest!(|(vals in prop_vec(any::<i64>(), 2..=8))| {
            let mut sorted: Vec<i64> = vals;
            sorted.sort();
            sorted.dedup();
            if sorted.len() < 2 {
                return Ok(());
            }
            let keys: Vec<Vec<u8>> = sorted.iter().map(|v| encode_i64_key(*v)).collect();
            let tuple_ids: Vec<u16> = (0..keys.len() as u16).collect();
            let n = keys.len();
            let mut node = BTreeNode {
                page_id: 10,
                node_type: NodeType::Leaf,
                keys: keys.clone(),
                children: Vec::new(),
                tuple_ids: tuple_ids.clone(),
                next_sibling: 0,
                prev_sibling: 0,
                parent: 0,
            };
            let (left, right, _) = node.split(10, 20).expect("split");
            let merged = left.merge(right, None).expect("merge");
            prop_assert!(merged.is_leaf());
            prop_assert_eq!(merged.keys.len(), n);
            prop_assert!(merged.validate().is_ok());
            for (merged_key, orig_key) in merged.keys.iter().zip(keys.iter()) {
                prop_assert_eq!(merged_key, orig_key);
            }
        });
    }

    /// 对应 Kani proof: `verify_merge_rejects_non_adjacent`
    #[test]
    fn proptest_merge_rejects_non_adjacent() {
        let left = BTreeNode {
            page_id: 1,
            node_type: NodeType::Leaf,
            keys: vec![encode_i64_key(1)],
            children: Vec::new(),
            tuple_ids: vec![0],
            next_sibling: 99,
            prev_sibling: 0,
            parent: 0,
        };
        let right = BTreeNode {
            page_id: 2,
            node_type: NodeType::Leaf,
            keys: vec![encode_i64_key(2)],
            children: Vec::new(),
            tuple_ids: vec![1],
            next_sibling: 0,
            prev_sibling: 1,
            parent: 0,
        };
        assert!(left.merge(right, None).is_err());
    }

    /// 对应 Kani proof: `verify_merge_rejects_different_types`
    #[test]
    fn proptest_merge_rejects_different_types() {
        let left = BTreeNode {
            page_id: 1,
            node_type: NodeType::Leaf,
            keys: vec![encode_i64_key(1)],
            children: Vec::new(),
            tuple_ids: vec![0],
            next_sibling: 2,
            prev_sibling: 0,
            parent: 0,
        };
        let right = BTreeNode {
            page_id: 2,
            node_type: NodeType::Internal,
            keys: vec![encode_i64_key(2)],
            children: vec![3, 4],
            tuple_ids: Vec::new(),
            next_sibling: 0,
            prev_sibling: 1,
            parent: 0,
        };
        assert!(left.merge(right, None).is_err());
    }

    /// 对应 Kani proof: `verify_insert_preserves_invariants`
    /// 性质：多次 insert 后 validate_all_nodes 通过
    #[test]
    fn proptest_insert_preserves_invariants() {
        proptest!(|(vals in prop_vec(any::<i64>(), 1..=50))| {
            let mut bt = BTree::with_default_order();
            for v in &vals {
                let _ = bt.insert(encode_i64_key(*v), 1);
            }
            prop_assert!(bt.validate_all_nodes().is_ok());
        });
    }

    /// 对应 Kani proof: `verify_insert_then_search_finds_key`
    /// 性质：insert(k, tid) 成功后 search(k) == Ok(Some(tid))
    #[test]
    fn proptest_insert_then_search_finds_key() {
        proptest!(|(v in any::<i64>())| {
            let mut bt = BTree::with_default_order();
            let key = encode_i64_key(v);
            if bt.insert(key.clone(), 42).is_ok() {
                let sr = bt.search(&key).expect("search");
                prop_assert_eq!(sr, Some(42));
            }
            prop_assert!(bt.validate_all_nodes().is_ok());
        });
    }

    /// 对应 Kani proof: `verify_delete_removes_key`
    /// 性质：delete(k) 成功后 search(k) == None
    #[test]
    fn proptest_delete_removes_key() {
        proptest!(|(v in any::<i64>())| {
            let mut bt = BTree::with_default_order();
            let key = encode_i64_key(v);
            if bt.insert(key.clone(), 7).is_ok() {
                let dr = bt.delete(&key).expect("delete");
                prop_assert!(dr);
                let sr = bt.search(&key).expect("search");
                prop_assert!(sr.is_none());
                prop_assert!(bt.validate_all_nodes().is_ok());
            }
        });
    }

    /// 对应 Kani proof: `verify_insert_delete_roundtrip`
    /// 性质：insert + delete round-trip 后 key 不在树中
    #[test]
    fn proptest_insert_delete_roundtrip() {
        proptest!(|(v in any::<i64>())| {
            let mut bt = BTree::with_default_order();
            let key = encode_i64_key(v);
            let _ = bt.insert(key.clone(), 1);
            let _ = bt.delete(&key);
            prop_assert!(bt.validate_all_nodes().is_ok());
            let sr = bt.search(&key).expect("search");
            prop_assert!(sr.is_none());
        });
    }

    /// 对应 Kani proof: `verify_mixed_operations_preserve_invariants`
    /// 性质：交错 insert/delete 序列后 validate_all_nodes 通过
    #[test]
    fn proptest_mixed_operations_preserve_invariants() {
        proptest!(|(ops in prop_vec(any::<i64>(), 1..=50))| {
            let mut bt = BTree::with_default_order();
            // 交替 insert / delete
            for (i, v) in ops.iter().enumerate() {
                let key = encode_i64_key(*v);
                if i % 2 == 0 {
                    let _ = bt.insert(key, i as u16);
                } else {
                    let _ = bt.delete(&key);
                }
            }
            prop_assert!(bt.validate_all_nodes().is_ok());
        });
    }

    /// 综合随机化测试：大规模随机 insert/delete 序列保持不变量
    /// 这是 Kani 证明的强力补充 — Kani 限深度，proptest 限规模
    /// 生成 (is_insert, value) 操作序列，模拟交错 insert/delete
    #[test]
    fn proptest_large_random_sequence_invariants() {
        proptest!(|(ops in prop_vec((any::<bool>(), any::<i64>()), 50..=500))| {
            let mut bt = BTree::with_default_order();
            let mut inserted: std::collections::HashSet<i64> = std::collections::HashSet::new();
            for (is_insert, v) in &ops {
                let key = encode_i64_key(*v);
                if *is_insert {
                    if bt.insert(key, 1).is_ok() {
                        inserted.insert(*v);
                    }
                } else if inserted.contains(v) {
                    let _ = bt.delete(&key);
                    inserted.remove(v);
                }
            }
            // 最终树必须合法
            prop_assert!(bt.validate_all_nodes().is_ok());
            // 所有已插入 key 必须可查
            for v in &inserted {
                let key = encode_i64_key(*v);
                let sr = bt.search(&key).expect("search");
                prop_assert!(sr.is_some(), "inserted key must be found");
            }
        });
    }
}
