//! SzRSQL B-Tree 范围扫描游标 — 对应 `SzRSQL技术实现方案.md` 9.5 节 `BTreeCursor`。
//!
//! Phase 1.5: B-Tree 点查 + 范围扫描
//!
//! 设计要点：
//! - **懒加载迭代器**：`BTreeCursor` 仅在 `next()` 被调用时才读取下一个叶子节点，
//!   避免一次性物化全部结果（适合大范围扫描）。
//! - **边界语义**：使用 `std::ops::Bound` 表示 lower/upper，支持 Included/Excluded/Unbounded。
//! - **前向遍历**：从 lower_bound 所在叶子开始，沿 `next_sibling` 链表向右扫描，
//!   直到 key 超过 upper_bound 或到达链表末尾。
//! - **生命周期**：`BTreeCursor<'a>` 借用 `BTree`，无法超出 BTree 生命周期。
//! - **空树处理**：空树（仅有空根叶子）的 cursor 立即返回 None。
//!
//! 不变量：
//! - cursor 产生的 key 严格升序
//! - cursor 不跨越 lower/upper 边界
//! - cursor 不持有任何写锁（只读迭代器）

use crate::btree::{BTree, BTreeError};
use std::ops::Bound;

/// B-Tree 范围扫描游标
///
/// 通过 `BTree::cursor(lower, upper)` 创建，实现 `Iterator<Item = (Vec<u8>, Vec<u8>)>`。
pub struct BTreeCursor<'a> {
    /// 借用的 BTree（只读）
    btree: &'a BTree,
    /// 当前所在叶子节点 page_id
    current_page: u32,
    /// 当前叶子节点中下一个待返回的 key 索引
    current_idx: usize,
    /// 上界（扫描停止条件）
    upper: Bound<Vec<u8>>,
    /// 是否已耗尽（一旦遇到 > upper 的 key 或到达链表末尾，置为 true）
    exhausted: bool,
}

impl<'a> BTreeCursor<'a> {
    /// 创建游标
    ///
    /// 内部执行：
    /// 1. 从根下沉到 lower_bound 所在叶子（lower=Unbounded 时下沉到最左叶子）
    /// 2. 在叶子内定位第一个满足 lower 条件的 key 索引
    /// 3. 若该索引超出叶子 key 范围或 key 已超过 upper，则跳到 next_sibling
    pub(crate) fn new(
        btree: &'a BTree,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<Self, BTreeError> {
        // 1. 定位起始叶子节点和 key 索引
        let (start_page, start_idx) = btree.find_range_start(lower)?;

        // 2. 转换 upper 为 Vec<u8> 拥有所有权（避免生命周期问题）
        let upper_owned = match upper {
            Bound::Included(k) => Bound::Included(k.to_vec()),
            Bound::Excluded(k) => Bound::Excluded(k.to_vec()),
            Bound::Unbounded => Bound::Unbounded,
        };

        let mut cursor = Self {
            btree,
            current_page: start_page,
            current_idx: start_idx,
            upper: upper_owned,
            exhausted: false,
        };

        // 3. 若起始位置已超过 upper 或叶子为空，尝试前进到第一个有效 key
        cursor.advance_to_valid()?;
        Ok(cursor)
    }

    /// 前进到第一个满足 lower 和 upper 条件的 key（跳过空叶子）
    ///
    /// 若当前叶子已无 key 可读，沿 next_sibling 链前进。
    /// 若遇到超过 upper 的 key，标记 exhausted。
    fn advance_to_valid(&mut self) -> Result<(), BTreeError> {
        loop {
            if self.exhausted {
                return Ok(());
            }
            let node = self.btree.read_node_public(self.current_page)?;
            // 当前叶子的 key 范围检查
            if self.current_idx >= node.keys.len() {
                // 当前叶子已读完，跳到 next_sibling
                if node.next_sibling == 0 {
                    self.exhausted = true;
                    return Ok(());
                }
                self.current_page = node.next_sibling;
                self.current_idx = 0;
                continue;
            }
            // 检查 current_idx 处的 key 是否超过 upper
            let key = &node.keys[self.current_idx];
            if Self::key_exceeds_upper(key, &self.upper) {
                self.exhausted = true;
                return Ok(());
            }
            // 当前位置有效
            return Ok(());
        }
    }

    /// 判断 key 是否超过 upper 边界
    fn key_exceeds_upper(key: &[u8], upper: &Bound<Vec<u8>>) -> bool {
        match upper {
            Bound::Included(upper_key) => key > upper_key.as_slice(),
            Bound::Excluded(upper_key) => key >= upper_key.as_slice(),
            Bound::Unbounded => false,
        }
    }
}

impl<'a> Iterator for BTreeCursor<'a> {
    type Item = (Vec<u8>, Vec<u8>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }
        // 读取当前叶子（advance_to_valid 已保证 current_idx 有效）
        let node = self.btree.read_node_public(self.current_page).ok()?;
        if self.current_idx >= node.keys.len() {
            // 防御性：理论上 advance_to_valid 已处理，这里再次前进
            self.advance_to_valid().ok()?;
            if self.exhausted {
                return None;
            }
            return self.next();
        }
        let key = node.keys[self.current_idx].clone();
        let value = node.values[self.current_idx].clone();
        // 前进到下一个位置
        self.current_idx += 1;
        // 若超出当前叶子，下一次 next() 会触发 advance_to_valid 跳页
        if self.current_idx >= node.keys.len() {
            let next_sibling = node.next_sibling;
            if next_sibling == 0 {
                self.exhausted = true;
            } else {
                self.current_page = next_sibling;
                self.current_idx = 0;
                self.advance_to_valid().ok()?;
            }
        } else {
            // 检查下一个 key 是否超过 upper
            let next_key = &node.keys[self.current_idx];
            if Self::key_exceeds_upper(next_key, &self.upper) {
                self.exhausted = true;
            }
        }
        Some((key, value))
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use crate::btree::{encode_i64_key, BTree};
    use std::ops::Bound;

    fn make_key(v: i64) -> Vec<u8> {
        encode_i64_key(v)
    }

    #[test]
    fn cursor_empty_tree_returns_none() {
        let bt = BTree::new(4);
        let mut cursor = bt.cursor(Bound::Unbounded, Bound::Unbounded).unwrap();
        assert!(cursor.next().is_none());
    }

    #[test]
    fn cursor_single_key() {
        let bt = {
            let mut b = BTree::new(4);
            b.insert(make_key(42), vec![100u8]).unwrap();
            b
        };
        let mut cursor = bt.cursor(Bound::Unbounded, Bound::Unbounded).unwrap();
        assert_eq!(cursor.next(), Some((make_key(42), vec![100u8])));
        assert!(cursor.next().is_none());
    }

    #[test]
    fn cursor_collect_all_in_order() {
        let bt = {
            let mut b = BTree::new(4);
            for i in 0..50i64 {
                b.insert(make_key(i), vec![i as u8]).unwrap();
            }
            b
        };
        let cursor = bt.cursor(Bound::Unbounded, Bound::Unbounded).unwrap();
        let pairs: Vec<(Vec<u8>, Vec<u8>)> = cursor.collect();
        assert_eq!(pairs.len(), 50);
        for (i, (k, v)) in pairs.iter().enumerate() {
            assert_eq!(crate::btree::decode_i64_key(k).unwrap(), i as i64);
            assert_eq!(v[0], i as u8);
        }
    }

    #[test]
    fn cursor_with_lower_included() {
        let bt = {
            let mut b = BTree::new(4);
            for i in 0..20i64 {
                b.insert(make_key(i), vec![i as u8]).unwrap();
            }
            b
        };
        let cursor = bt
            .cursor(Bound::Included(&make_key(10)), Bound::Unbounded)
            .unwrap();
        let keys: Vec<i64> = cursor
            .map(|(k, _)| crate::btree::decode_i64_key(&k).unwrap())
            .collect();
        assert_eq!(keys, (10..20).collect::<Vec<_>>());
    }

    #[test]
    fn cursor_with_upper_excluded() {
        let bt = {
            let mut b = BTree::new(4);
            for i in 0..20i64 {
                b.insert(make_key(i), vec![i as u8]).unwrap();
            }
            b
        };
        let cursor = bt
            .cursor(Bound::Unbounded, Bound::Excluded(&make_key(10)))
            .unwrap();
        let keys: Vec<i64> = cursor
            .map(|(k, _)| crate::btree::decode_i64_key(&k).unwrap())
            .collect();
        assert_eq!(keys, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn cursor_multi_level_tree_correct() {
        let bt = {
            let mut b = BTree::new(4);
            for i in 0..500i64 {
                b.insert(make_key(i), vec![i as u8]).unwrap();
            }
            b
        };
        // 范围 [100, 200)
        let cursor = bt
            .cursor(
                Bound::Included(&make_key(100)),
                Bound::Excluded(&make_key(200)),
            )
            .unwrap();
        let keys: Vec<i64> = cursor
            .map(|(k, _)| crate::btree::decode_i64_key(&k).unwrap())
            .collect();
        assert_eq!(keys.len(), 100);
        assert_eq!(keys[0], 100);
        assert_eq!(keys[99], 199);
    }
}
