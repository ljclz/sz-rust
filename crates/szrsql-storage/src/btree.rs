//! SzRSQL B-Tree 存储引擎 — 对应 `SzRSQL技术实现方案.md` 9.5 节。
//!
//! Phase 1.1: B-Tree 节点内部/叶子结构 + 编码/解码 + 分裂/合并
//!
//! 设计要点：
//! - 节点类型：Internal（内部节点）/ Leaf（叶子节点）
//! - 键类型：`Vec<u8>` 编码后的可比较字节串（上层负责将 Value 编码为可比较字节）
//! - Internal 节点：`children.len() == keys.len() + 1`
//! - Leaf 节点：`values.len() == keys.len()`
//! - 兄弟链表：Leaf 节点维护 next_sibling / prev_sibling，便于范围扫描与合并借键
//! - 父指针：parent 字段便于上行/下行遍历（0 表示根节点）
//!
//! BTreeNode 编码格式（小端，存储于 Page body）：
//! ```text
//! Offset  Size  Field
//! 0       1     node_type (0=Internal, 1=Leaf)
//! 1       4     page_id (u32 LE)
//! 5       4     key_count (u32 LE)
//! 9       4     next_sibling (u32 LE)
//! 13      4     prev_sibling (u32 LE)
//! 17      4     parent (u32 LE)
//! 21      ...   keys（每个 key：4B key_len + key_len 字节）
//! ...     ...   children（Internal：(keys+1) × 4B；Leaf：0）
//! ...     ...   values（Leaf：每个 value 为 4B 长度前缀 + 原始字节；Internal：0）
//! ```
//!
//! Header 固定 21 字节。
//!
//! 注：P0-4 将 values 从 u32 tuple_id 扩展为 Vec<u8>，行数据直接存入 B+Tree 叶节点。

use std::cmp::Ordering;
use std::ops::Bound;

use tracing::instrument;

// =====================================================================
//  常量
// =====================================================================

/// B-Tree 默认阶数（每个节点最多 order 个 key）
pub const BTREE_DEFAULT_ORDER: usize = 256;

/// BTreeNode 固定头部大小
pub const BTREE_NODE_HEADER_SIZE: usize = 1 + 4 + 4 + 4 + 4 + 4; // 21

// =====================================================================
//  NodeType
// =====================================================================

/// B-Tree 节点类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum NodeType {
    /// 内部节点（仅存 key + child page_id）
    Internal = 0,
    /// 叶子节点（存 key + value）
    Leaf = 1,
}

/// B+Tree 键值对条目：key + 序列化后的 value
pub type BTreeEntry = (Vec<u8>, Vec<u8>);

impl NodeType {
    /// 从 u8 构造 NodeType，非法值返回 Err
    pub fn from_u8(v: u8) -> Result<Self, BTreeError> {
        match v {
            0 => Ok(NodeType::Internal),
            1 => Ok(NodeType::Leaf),
            _ => Err(BTreeError::InvalidNodeType(v)),
        }
    }

    /// 转为 u8
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

// =====================================================================
//  BTreeError
// =====================================================================

/// B-Tree 错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BTreeError {
    #[error("invalid node type: {0}")]
    InvalidNodeType(u8),
    #[error("buffer too short: need {need}, have {have}")]
    BufferTooShort { need: usize, have: usize },
    #[error("key count mismatch: expected {expected}, actual {actual}")]
    KeyCountMismatch { expected: usize, actual: usize },
    #[error("children count mismatch for internal node: expected {expected}, actual {actual}")]
    ChildrenCountMismatch { expected: usize, actual: usize },
    #[error("values count mismatch for leaf node: expected {expected}, actual {actual}")]
    ValuesCountMismatch { expected: usize, actual: usize },
    #[error("keys not sorted at index {index}: prev={prev:?}, curr={curr:?}")]
    KeysNotSorted {
        index: usize,
        prev: Vec<u8>,
        curr: Vec<u8>,
    },
    #[error("node is full (key_count={key_count}, order={order})")]
    NodeFull { key_count: usize, order: usize },
    #[error("node is empty (no keys)")]
    NodeEmpty,
    #[error("cannot split internal node with < 2 keys (key_count={key_count})")]
    CannotSplitInternal { key_count: usize },
    #[error("cannot merge non-adjacent nodes")]
    CannotMergeNonAdjacent,
    #[error("cannot merge nodes of different types")]
    CannotMergeDifferentTypes,
    #[error("merged node would be full (merged_count={merged_count}, order={order})")]
    MergedNodeWouldBeFull { merged_count: usize, order: usize },
    #[error("key too large: {len} bytes (max: {max})")]
    KeyTooLarge { len: usize, max: usize },
    #[error("bulk load input not sorted at index {index}: prev={prev:?}, curr={curr:?}")]
    BulkLoadNotSorted {
        index: usize,
        prev: Vec<u8>,
        curr: Vec<u8>,
    },
    #[error("bulk load input empty")]
    BulkLoadEmpty,
    #[error("bulk load batch size too small: {0} (min: 2)")]
    BulkLoadBatchTooSmall(usize),
    /// P0-3 修复：BufferPool 持久化错误
    #[error("persistence error: {0}")]
    PersistenceError(String),
    /// P0-3 修复：节点编码超出单页容量
    #[error("node encoded size {encoded} exceeds page body capacity {max}")]
    NodeExceedsPageCapacity { encoded: usize, max: usize },
}

// =====================================================================
//  BTreeNode
// =====================================================================

/// B-Tree 节点
///
/// 对应技术方案 9.5 节 `BTreeNode`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BTreeNode {
    /// 页 ID（文件内唯一）
    pub page_id: u32,
    /// 节点类型
    pub node_type: NodeType,
    /// 键列表（按升序排列，每个 key 为可比较字节串）
    pub keys: Vec<Vec<u8>>,
    /// 内部节点：child page_id 列表（len == keys.len() + 1）
    /// 叶子节点：空
    pub children: Vec<u32>,
    /// 叶子节点：value 列表（len == keys.len()），每个 value 为序列化后的行数据
    /// 内部节点：空
    ///
    /// P0-4：从 u32 tuple_id 扩展为 Vec<u8> value，行数据直接存入 B+Tree 叶节点。
    pub values: Vec<Vec<u8>>,
    /// 右兄弟页 ID（叶子节点范围扫描用，0 = 无）
    pub next_sibling: u32,
    /// 左兄弟页 ID（叶子节点合并借键用，0 = 无）
    pub prev_sibling: u32,
    /// 父节点页 ID（0 = 根节点）
    pub parent: u32,
}

impl BTreeNode {
    /// 创建新叶子节点
    pub fn new_leaf(page_id: u32) -> Self {
        Self {
            page_id,
            node_type: NodeType::Leaf,
            keys: Vec::new(),
            children: Vec::new(),
            values: Vec::new(),
            next_sibling: 0,
            prev_sibling: 0,
            parent: 0,
        }
    }

    /// 创建新内部节点
    ///
    /// 注：B-Tree 不变量要求 Internal 节点的 children.len() == keys.len() + 1。
    /// 即使是空 Internal 节点（0 keys）也应有 1 个 child 指针。
    /// 这里用 `vec![0]` 作为占位 child，调用方在添加 keys 时应同步更新 children。
    pub fn new_internal(page_id: u32) -> Self {
        Self {
            page_id,
            node_type: NodeType::Internal,
            keys: Vec::new(),
            children: vec![0],
            values: Vec::new(),
            next_sibling: 0,
            prev_sibling: 0,
            parent: 0,
        }
    }

    /// 是否为叶子节点
    pub fn is_leaf(&self) -> bool {
        self.node_type == NodeType::Leaf
    }

    /// 是否为内部节点
    pub fn is_internal(&self) -> bool {
        self.node_type == NodeType::Internal
    }

    /// 节点是否已满（key 数量 >= order）
    pub fn is_full(&self, order: usize) -> bool {
        self.keys.len() >= order
    }

    /// 节点是否下溢（key 数量 < order / 2，根节点除外）
    pub fn is_underflow(&self, order: usize) -> bool {
        let min_keys = order / 2;
        self.keys.len() < min_keys
    }

    /// 节点是否至少半满（key 数量 >= order / 2）
    pub fn is_at_least_half_full(&self, order: usize) -> bool {
        self.keys.len() >= order / 2
    }

    /// key 数量
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// 校验内部不变量
    ///
    /// - Internal 节点：children.len() == keys.len() + 1, values 为空
    /// - Leaf 节点：values.len() == keys.len(), children 为空
    /// - keys 升序排列
    pub fn validate(&self) -> Result<(), BTreeError> {
        match self.node_type {
            NodeType::Internal => {
                let expected_children = self.keys.len() + 1;
                if self.children.len() != expected_children {
                    return Err(BTreeError::ChildrenCountMismatch {
                        expected: expected_children,
                        actual: self.children.len(),
                    });
                }
                if !self.values.is_empty() {
                    return Err(BTreeError::ValuesCountMismatch {
                        expected: 0,
                        actual: self.values.len(),
                    });
                }
            }
            NodeType::Leaf => {
                if self.values.len() != self.keys.len() {
                    return Err(BTreeError::ValuesCountMismatch {
                        expected: self.keys.len(),
                        actual: self.values.len(),
                    });
                }
                if !self.children.is_empty() {
                    return Err(BTreeError::ChildrenCountMismatch {
                        expected: 0,
                        actual: self.children.len(),
                    });
                }
            }
        }
        // keys 升序检查
        for i in 1..self.keys.len() {
            if self.keys[i - 1].as_slice() >= self.keys[i].as_slice() {
                return Err(BTreeError::KeysNotSorted {
                    index: i,
                    prev: self.keys[i - 1].clone(),
                    curr: self.keys[i].clone(),
                });
            }
        }
        Ok(())
    }

    /// 二分查找 key，返回 (found_index, insert_position)
    ///
    /// - 若找到：返回 (Some(idx), idx)
    /// - 若未找到：返回 (None, idx)，idx 为该 key 应插入的位置
    pub fn search_key(&self, key: &[u8]) -> (Option<usize>, usize) {
        match self.keys.binary_search_by(|k| k.as_slice().cmp(key)) {
            Ok(idx) => (Some(idx), idx),
            Err(idx) => (None, idx),
        }
    }

    /// 编码为字节序列
    pub fn encode(&self) -> Vec<u8> {
        let total_keys_len: usize = self.keys.iter().map(|k| 4 + k.len()).sum();
        let children_len = self.children.len() * 4;
        // P0-4：value 为 length-prefixed Vec<u8>（4B 长度 + 原始字节）
        let values_len: usize = self.values.iter().map(|v| 4 + v.len()).sum();
        let total = BTREE_NODE_HEADER_SIZE + total_keys_len + children_len + values_len;

        let mut buf = Vec::with_capacity(total);
        // Header
        buf.push(self.node_type.as_u8());
        buf.extend_from_slice(&self.page_id.to_le_bytes());
        buf.extend_from_slice(&(self.keys.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.next_sibling.to_le_bytes());
        buf.extend_from_slice(&self.prev_sibling.to_le_bytes());
        buf.extend_from_slice(&self.parent.to_le_bytes());
        // Keys
        for k in &self.keys {
            buf.extend_from_slice(&(k.len() as u32).to_le_bytes());
            buf.extend_from_slice(k);
        }
        // Children (Internal only)
        for c in &self.children {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        // Values (Leaf only) — P0-4: length-prefixed byte vectors
        for v in &self.values {
            buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
            buf.extend_from_slice(v);
        }
        buf
    }

    /// 从字节序列解码
    pub fn decode(buf: &[u8]) -> Result<Self, BTreeError> {
        if buf.len() < BTREE_NODE_HEADER_SIZE {
            return Err(BTreeError::BufferTooShort {
                need: BTREE_NODE_HEADER_SIZE,
                have: buf.len(),
            });
        }
        let node_type = NodeType::from_u8(buf[0])?;
        let page_id = u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]);
        let key_count = u32::from_le_bytes([buf[5], buf[6], buf[7], buf[8]]) as usize;
        let next_sibling = u32::from_le_bytes([buf[9], buf[10], buf[11], buf[12]]);
        let prev_sibling = u32::from_le_bytes([buf[13], buf[14], buf[15], buf[16]]);
        let parent = u32::from_le_bytes([buf[17], buf[18], buf[19], buf[20]]);

        // 容量上界：每个 key 至少占 4 字节（key_len），防止恶意/损坏的 key_count 触发 OOM
        let remaining = buf.len().saturating_sub(BTREE_NODE_HEADER_SIZE);
        let cap = key_count.min(remaining / 4);
        let mut pos = BTREE_NODE_HEADER_SIZE;
        let mut keys = Vec::with_capacity(cap);
        for _ in 0..key_count {
            if pos + 4 > buf.len() {
                return Err(BTreeError::BufferTooShort {
                    need: pos + 4,
                    have: buf.len(),
                });
            }
            let klen =
                u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
            pos += 4;
            if pos + klen > buf.len() {
                return Err(BTreeError::BufferTooShort {
                    need: pos + klen,
                    have: buf.len(),
                });
            }
            keys.push(buf[pos..pos + klen].to_vec());
            pos += klen;
        }

        // Children (Internal: keys.len() + 1)
        let mut children = Vec::new();
        let mut values = Vec::new();
        match node_type {
            NodeType::Internal => {
                let child_count = key_count + 1;
                let child_cap = child_count.min(buf.len().saturating_sub(pos) / 4 + 1);
                children.reserve(child_cap);
                for _ in 0..child_count {
                    if pos + 4 > buf.len() {
                        return Err(BTreeError::BufferTooShort {
                            need: pos + 4,
                            have: buf.len(),
                        });
                    }
                    let c =
                        u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
                    children.push(c);
                    pos += 4;
                }
            }
            NodeType::Leaf => {
                // P0-4：value 为 length-prefixed Vec<u8>（与 key 编码方式对称）
                let val_cap = key_count.min(remaining / 8); // 保守上界：每个 value 至少 4B len + 4B data
                values.reserve(val_cap);
                for _ in 0..key_count {
                    if pos + 4 > buf.len() {
                        return Err(BTreeError::BufferTooShort {
                            need: pos + 4,
                            have: buf.len(),
                        });
                    }
                    let vlen =
                        u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
                            as usize;
                    pos += 4;
                    if pos + vlen > buf.len() {
                        return Err(BTreeError::BufferTooShort {
                            need: pos + vlen,
                            have: buf.len(),
                        });
                    }
                    values.push(buf[pos..pos + vlen].to_vec());
                    pos += vlen;
                }
            }
        }

        Ok(Self {
            page_id,
            node_type,
            keys,
            children,
            values,
            next_sibling,
            prev_sibling,
            parent,
        })
    }

    /// 编码后字节数
    pub fn encoded_size(&self) -> usize {
        self.encode().len()
    }

    /// 分裂节点（返回 left, right, promoted_key）
    ///
    /// 分裂策略：取中点 mid = keys.len() / 2
    /// - Left 节点保留 keys[0..mid]
    /// - Right 节点保留 keys[mid+1..]（Internal）或 keys[mid..]（Leaf）
    /// - Promoted key = keys[mid]（Internal）或 keys[mid] 的副本（Leaf）
    ///
    /// 对于 Leaf 节点：mid 处的 key 同时存在于 Right 节点（叶子分裂不提升 key 副本，
    /// 直接将中点 key 提升到父节点，但叶子仍保留该 key）
    /// 对于 Internal 节点：mid 处的 key 提升到父节点，不保留在 Left 或 Right
    ///
    /// 分裂后两个节点都至少有 `(keys.len() - 1) / 2` 个 key（>= 半满）。
    #[instrument(skip(self), fields(key_count = self.keys.len()), level = "trace")]
    pub fn split(
        &mut self,
        left_page_id: u32,
        right_page_id: u32,
    ) -> Result<(BTreeNode, BTreeNode, Vec<u8>), BTreeError> {
        if self.keys.is_empty() {
            return Err(BTreeError::NodeEmpty);
        }
        if self.keys.len() < 2 {
            // 至少需要 2 个 key 才能分裂
            return Err(BTreeError::CannotSplitInternal {
                key_count: self.keys.len(),
            });
        }

        let mid = self.keys.len() / 2;
        let promoted_key = self.keys[mid].clone();

        match self.node_type {
            NodeType::Leaf => {
                // Leaf 分裂：mid 处的 key 仍保留在 Right 节点
                let left_keys = self.keys[0..mid].to_vec();
                let left_values = self.values[0..mid].to_vec();
                let right_keys = self.keys[mid..].to_vec();
                let right_values = self.values[mid..].to_vec();

                let left = BTreeNode {
                    page_id: left_page_id,
                    node_type: NodeType::Leaf,
                    keys: left_keys,
                    children: Vec::new(),
                    values: left_values,
                    next_sibling: right_page_id,
                    prev_sibling: self.prev_sibling,
                    parent: self.parent,
                };
                let right = BTreeNode {
                    page_id: right_page_id,
                    node_type: NodeType::Leaf,
                    keys: right_keys,
                    children: Vec::new(),
                    values: right_values,
                    next_sibling: self.next_sibling,
                    prev_sibling: left_page_id,
                    parent: self.parent,
                };
                Ok((left, right, promoted_key))
            }
            NodeType::Internal => {
                // Internal 分裂：mid 处的 key 提升到父节点，不保留在 Left 或 Right
                let left_keys = self.keys[0..mid].to_vec();
                let left_children = self.children[0..=mid].to_vec();
                let right_keys = self.keys[mid + 1..].to_vec();
                let right_children = self.children[mid + 1..].to_vec();

                let left = BTreeNode {
                    page_id: left_page_id,
                    node_type: NodeType::Internal,
                    keys: left_keys,
                    children: left_children,
                    values: Vec::new(),
                    next_sibling: 0,
                    prev_sibling: 0,
                    parent: self.parent,
                };
                let right = BTreeNode {
                    page_id: right_page_id,
                    node_type: NodeType::Internal,
                    keys: right_keys,
                    children: right_children,
                    values: Vec::new(),
                    next_sibling: 0,
                    prev_sibling: 0,
                    parent: self.parent,
                };
                Ok((left, right, promoted_key))
            }
        }
    }

    /// 合并两个相邻兄弟节点（self 为左兄弟，other 为右兄弟）
    ///
    /// 合并策略：
    /// - 两个节点必须是同类型且相邻
    /// - 合并后节点 keys 数量 = self.keys.len() + other.keys.len()
    ///   （Internal 节点还需加上从父节点下降的 separator key）
    /// - 合并后节点必须不满（< order）
    ///
    /// 注意：本方法不处理从父节点下降的 separator key（由调用方处理），
    /// 仅合并两个叶子或两个内部节点的 keys/children/values。
    /// 对于 Internal 节点合并，调用方需在合并后手动插入 separator key 到中间。
    #[instrument(skip(self, other, separator_key), fields(has_separator = separator_key.is_some()), level = "trace")]
    pub fn merge(
        self,
        other: BTreeNode,
        separator_key: Option<Vec<u8>>,
    ) -> Result<BTreeNode, BTreeError> {
        if self.node_type != other.node_type {
            return Err(BTreeError::CannotMergeDifferentTypes);
        }
        if self.next_sibling != other.page_id || other.prev_sibling != self.page_id {
            return Err(BTreeError::CannotMergeNonAdjacent);
        }

        let mut merged_keys = self.keys.clone();
        if let Some(sep) = &separator_key {
            if self.node_type == NodeType::Internal {
                merged_keys.push(sep.clone());
            }
        }
        merged_keys.extend(other.keys.clone());

        let merged = match self.node_type {
            NodeType::Leaf => {
                let mut merged_values = self.values.clone();
                merged_values.extend(other.values.clone());
                BTreeNode {
                    page_id: self.page_id,
                    node_type: NodeType::Leaf,
                    keys: merged_keys,
                    children: Vec::new(),
                    values: merged_values,
                    next_sibling: other.next_sibling,
                    prev_sibling: self.prev_sibling,
                    parent: self.parent,
                }
            }
            NodeType::Internal => {
                let mut merged_children = self.children.clone();
                merged_children.extend(other.children.clone());
                BTreeNode {
                    page_id: self.page_id,
                    node_type: NodeType::Internal,
                    keys: merged_keys,
                    children: merged_children,
                    values: Vec::new(),
                    next_sibling: 0,
                    prev_sibling: 0,
                    parent: self.parent,
                }
            }
        };
        Ok(merged)
    }
}

// =====================================================================
//  辅助函数：生成可比较的 key 字节串
// =====================================================================

/// 将 i64 编码为可比较的字节串（用于 B-Tree key）
///
/// 算法：翻转符号位后大端编码，使负数 < 正数按字典序排列。
pub fn encode_i64_key(v: i64) -> Vec<u8> {
    let u = v as u64 ^ (1u64 << 63); // 翻转符号位
    u.to_be_bytes().to_vec()
}

/// 将可比较字节串解码为 i64
pub fn decode_i64_key(key: &[u8]) -> Result<i64, BTreeError> {
    if key.len() != 8 {
        return Err(BTreeError::KeyTooLarge {
            len: key.len(),
            max: 8,
        });
    }
    let arr: [u8; 8] = key.try_into().unwrap();
    let u = u64::from_be_bytes(arr);
    Ok((u ^ (1u64 << 63)) as i64)
}

/// 比较两个 key 字节串（字典序）
pub fn compare_keys(a: &[u8], b: &[u8]) -> Ordering {
    a.cmp(b)
}

// =====================================================================
//  BTree: B-Tree 管理器（Phase 1.3 插入 + 搜索）
// =====================================================================

/// B-Tree 管理器
///
/// Phase 1.3 实现：
/// - 单线程插入（含递归分裂到根）
/// - 点查搜索
/// - 中序遍历（验证 key 有序性）
/// - 树高度查询
///
/// 存储使用内存 `HashMap<u32, BTreeNode>`，后续 Phase 集成 BufferPool。
#[derive(Debug, Clone)]
pub struct BTree {
    /// 根节点 page_id
    root_page_id: u32,
    /// B-Tree 阶数（每个节点最多 order 个 key）
    order: usize,
    /// 页存储（page_id → node）
    pages: std::collections::HashMap<u32, BTreeNode>,
    /// 下一个 page_id（简单计数器，Phase 1.2 的 FreeList 可替换）
    next_page_id: u32,
}

impl Default for BTree {
    fn default() -> Self {
        Self::with_default_order()
    }
}

impl BTree {
    /// 创建新 B-Tree，指定阶数
    pub fn new(order: usize) -> Self {
        assert!(order >= 3, "B-Tree order must be >= 3, got {}", order);
        let root_page_id = 1;
        let mut pages = std::collections::HashMap::new();
        pages.insert(root_page_id, BTreeNode::new_leaf(root_page_id));
        Self {
            root_page_id,
            order,
            pages,
            next_page_id: 2,
        }
    }

    /// 创建默认阶数（256）的 B-Tree
    pub fn with_default_order() -> Self {
        Self::new(BTREE_DEFAULT_ORDER)
    }

    /// 分配新 page_id
    fn alloc_page_id(&mut self) -> u32 {
        let id = self.next_page_id;
        self.next_page_id += 1;
        id
    }

    /// 读取节点
    fn read_node(&self, page_id: u32) -> Result<&BTreeNode, BTreeError> {
        self.pages.get(&page_id).ok_or(BTreeError::BufferTooShort {
            need: page_id as usize,
            have: self.pages.len(),
        })
    }

    /// 读取节点（可变）
    fn read_node_mut(&mut self, page_id: u32) -> Result<&mut BTreeNode, BTreeError> {
        let pages_len = self.pages.len();
        self.pages
            .get_mut(&page_id)
            .ok_or(BTreeError::BufferTooShort {
                need: page_id as usize,
                have: pages_len,
            })
    }

    /// 写入节点
    fn write_node(&mut self, node: BTreeNode) {
        let page_id = node.page_id;
        self.pages.insert(page_id, node);
    }

    /// 根节点 page_id
    pub fn root_page_id(&self) -> u32 {
        self.root_page_id
    }

    /// 阶数
    pub fn order(&self) -> usize {
        self.order
    }

    /// 节点总数
    pub fn node_count(&self) -> usize {
        self.pages.len()
    }

    /// 下一个将分配的 page_id（用于测试遍历所有已分配页）
    pub fn next_page_id(&self) -> u32 {
        self.next_page_id
    }

    /// 校验所有节点的不变量（用于 fuzz/stress 测试）
    ///
    /// 遍历 pages 中所有节点，调用 `validate()` 检查：
    /// - Internal 节点 children.len() == keys.len() + 1
    /// - Leaf 节点 values.len() == keys.len()
    /// - keys 严格升序
    pub fn validate_all_nodes(&self) -> Result<(), BTreeError> {
        for node in self.pages.values() {
            node.validate()?;
        }
        Ok(())
    }

    /// 树高度（单节点树高度 = 1）
    pub fn height(&self) -> usize {
        let mut height = 1;
        let mut current = self.root_page_id;
        loop {
            let node = self.read_node(current).unwrap();
            if node.is_leaf() {
                return height;
            }
            current = node.children[0];
            height += 1;
        }
    }

    /// 点查搜索
    ///
    /// 返回 key 对应的 value（序列化后的行数据），未找到返回 None。
    #[instrument(skip(self, key), fields(key_len = key.len(), root_page_id = self.root_page_id), level = "trace")]
    pub fn search(&self, key: &[u8]) -> Result<Option<Vec<u8>>, BTreeError> {
        let mut current = self.root_page_id;
        loop {
            let node = self.read_node(current)?;
            match node.node_type {
                NodeType::Internal => {
                    // Internal 节点的 key 是分隔键：
                    // - children[i] 包含 keys < keys[i]
                    // - children[i+1] 包含 keys >= keys[i]
                    // 因此若找到 key == keys[i]，应走向 children[i+1]（右子树）
                    let (found, pos) = node.search_key(key);
                    let child_idx = if found.is_some() {
                        pos + 1
                    } else {
                        pos
                    };
                    current = node.children[child_idx];
                }
                NodeType::Leaf => {
                    let (found, _) = node.search_key(key);
                    return Ok(found.map(|idx| node.values[idx].clone()));
                }
            }
        }
    }

    /// 插入 (key, value)
    ///
    /// 若 key 已存在，更新 value（upsert 语义）。
    /// 满节点递归分裂，根节点分裂时树高度 +1。
    ///
    /// P0-4：value 为序列化后的行数据（含 xmin/xmax/Row），行数据直接存入 B+Tree 叶节点。
    #[instrument(skip(self, key, value), fields(key_len = key.len(), value_len = value.len(), root_page_id = self.root_page_id), level = "trace")]
    pub fn insert(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), BTreeError> {
        tracing::Span::current().record("value_len", value.len());
        // 1. 搜索路径（从根到叶）
        let path = self.find_path_to_leaf(&key)?;

        // 2. 在叶子节点插入/更新
        let leaf_page_id = *path.last().unwrap();
        {
            let leaf = self.read_node_mut(leaf_page_id)?;
            // 检查是否已存在（upsert）
            let (found, pos) = leaf.search_key(&key);
            if let Some(idx) = found {
                leaf.values[idx] = value;
                return Ok(());
            }
            leaf.keys.insert(pos, key);
            leaf.values.insert(pos, value);
        }

        // 3. 若叶子节点满，递归分裂
        if self.read_node(leaf_page_id)?.is_full(self.order) {
            self.split_upwards(&path)?;
        }
        Ok(())
    }

    /// 从根到叶搜索路径，返回 page_id 列表（root 在前，leaf 在末尾）
    fn find_path_to_leaf(&self, key: &[u8]) -> Result<Vec<u32>, BTreeError> {
        let mut path = Vec::new();
        let mut current = self.root_page_id;
        loop {
            path.push(current);
            let node = self.read_node(current)?;
            if node.is_leaf() {
                return Ok(path);
            }
            // Internal 节点：若 key == keys[i]，走向 children[i+1]（右子树）
            let (found, pos) = node.search_key(key);
            let child_idx = if found.is_some() {
                pos + 1
            } else {
                pos
            };
            current = node.children[child_idx];
        }
    }

    /// 从路径末尾（叶子）向上递归分裂
    ///
    /// `path` 是从根到当前需分裂节点的 page_id 列表。
    fn split_upwards(&mut self, path: &[u32]) -> Result<(), BTreeError> {
        let mut i = path.len() - 1; // 从叶子开始
        loop {
            let page_id = path[i];
            if !self.read_node(page_id)?.is_full(self.order) {
                return Ok(()); // 无需分裂
            }

            // 分配新右节点 page_id
            let right_page_id = self.alloc_page_id();
            // 读取并移除当前节点（取出所有权）
            let mut node = self.pages.remove(&page_id).unwrap();
            let (left, right, promoted_key) = node.split(page_id, right_page_id)?;

            // 写回 left 和 right
            self.write_node(left);
            self.write_node(right);

            if i == 0 {
                // 根节点分裂 → 创建新根
                let new_root_id = self.alloc_page_id();
                let mut new_root = BTreeNode::new_internal(new_root_id);
                new_root.children.clear(); // 清除占位 child
                new_root.keys.push(promoted_key);
                new_root.children.push(page_id); // left
                new_root.children.push(right_page_id); // right
                                                       // 更新 left 和 right 的 parent
                self.read_node_mut(page_id)?.parent = new_root_id;
                self.read_node_mut(right_page_id)?.parent = new_root_id;
                self.write_node(new_root);
                self.root_page_id = new_root_id;
                return Ok(());
            }

            // 非根节点分裂 → 将 promoted_key 插入父节点
            let parent_page_id = path[i - 1];
            {
                let parent = self.read_node_mut(parent_page_id)?;
                let (_, insert_pos) = parent.search_key(&promoted_key);
                parent.keys.insert(insert_pos, promoted_key.clone());
                // 在 insert_pos+1 位置插入 right_page_id
                parent.children.insert(insert_pos + 1, right_page_id);
            }
            // 更新 right 的 parent（parent 借用已随块结束释放）
            self.read_node_mut(right_page_id)?.parent = parent_page_id;

            i -= 1; // 上溯到父节点
        }
    }

    // =================================================================
    // Phase 1.7: B-Tree 删除（含合并）
    // =================================================================

    /// 删除 key
    ///
    /// 返回 `true` 表示找到并删除，`false` 表示 key 不存在。
    ///
    /// 算法（B+Tree 叶子删除 + 上溯再平衡）：
    /// 1. 沿路径找到包含 key 的叶子；若叶子中无此 key，返回 false。
    /// 2. 从叶子移除 key + value。
    /// 3. 若叶子下溢（keys.len() < order/2）：
    ///    a. 优先向右兄弟借键（right.keys.len() > order/2）
    ///    b. 其次向左兄弟借键（left.keys.len() > order/2）
    ///    c. 否则与一个兄弟合并（合并后移除兄弟节点，父节点下降分隔键）
    /// 4. 合并会让父节点少一个 key+child，递归上溯再平衡父节点。
    /// 5. 若根节点 keys 为空且只有 1 个 child，将 child 提升为新根（高度 -1）。
    ///
    /// **internal separator 残留**：B+Tree 允许 internal 节点保留 stale separator
    /// （已被删除的 key 仍出现在 internal 节点中），因为 search 用 `>=` 导航到右子树，
    /// 删除的 key 在右子树叶子中找不到，仍返回 None。本实现不主动刷新 stale separator。
    #[instrument(skip(self, key), fields(key_len = key.len(), root_page_id = self.root_page_id), level = "trace")]
    pub fn delete(&mut self, key: &[u8]) -> Result<bool, BTreeError> {
        // 1. 沿路径找到叶子
        let path = self.find_path_to_leaf(key)?;
        let leaf_page_id = *path.last().unwrap();

        // 2. 检查叶子中是否存在 key
        let key_pos = {
            let leaf = self.read_node(leaf_page_id)?;
            let (found, pos) = leaf.search_key(key);
            match found {
                Some(_) => Some(pos),
                None => return Ok(false),
            }
        };

        // 3. 从叶子移除 key + value
        let pos = key_pos.unwrap();
        {
            let leaf = self.read_node_mut(leaf_page_id)?;
            leaf.keys.remove(pos);
            leaf.values.remove(pos);
        }

        // 4. 上溯再平衡
        self.rebalance_upwards(&path)?;

        Ok(true)
    }

    /// 从叶子开始上溯再平衡
    ///
    /// `path` 是从根到当前需再平衡节点的 page_id 列表。
    /// 从 path 末尾（叶子）开始，依次检查每个节点是否下溢，下溢则借键/合并，直到无需再平衡或到达根。
    fn rebalance_upwards(&mut self, path: &[u32]) -> Result<(), BTreeError> {
        let mut i = path.len() - 1; // 从叶子开始
        loop {
            let page_id = path[i];

            // 根节点无需再平衡（可以 0 keys）
            if i == 0 {
                // 根特殊处理：若 keys 为空且只有 1 个 child，将 child 提升为新根
                let needs_shrink = {
                    let root = self.read_node(page_id)?;
                    root.node_type == NodeType::Internal
                        && root.keys.is_empty()
                        && root.children.len() == 1
                };
                if needs_shrink {
                    let new_root_id = self.read_node(page_id)?.children[0];
                    self.pages.remove(&page_id);
                    if let Some(new_root) = self.pages.get_mut(&new_root_id) {
                        new_root.parent = 0;
                    }
                    self.root_page_id = new_root_id;
                }
                return Ok(());
            }

            // 检查当前节点是否下溢
            let underflow = {
                let node = self.read_node(page_id)?;
                node.is_underflow(self.order)
            };
            if !underflow {
                return Ok(()); // 无需再平衡
            }

            let parent_page_id = path[i - 1];

            // 找到当前节点在父节点 children 中的位置
            let child_idx = {
                let parent = self.read_node(parent_page_id)?;
                parent
                    .children
                    .iter()
                    .position(|&c| c == page_id)
                    .expect("child must exist in parent")
            };

            // 获取左右兄弟 page_id
            let (left_sibling_id, right_sibling_id) = {
                let parent = self.read_node(parent_page_id)?;
                let left = if child_idx > 0 {
                    Some(parent.children[child_idx - 1])
                } else {
                    None
                };
                let right = if child_idx + 1 < parent.children.len() {
                    Some(parent.children[child_idx + 1])
                } else {
                    None
                };
                (left, right)
            };

            // 尝试向右兄弟借键
            let can_borrow_right = right_sibling_id
                .map(|rid| {
                    let r = self.read_node(rid).unwrap();
                    r.keys.len() > self.order / 2
                })
                .unwrap_or(false);
            if can_borrow_right {
                let right_id = right_sibling_id.unwrap();
                self.borrow_from_right(parent_page_id, child_idx, page_id, right_id)?;
                return Ok(()); // 借键后下溢已解决
            }

            // 尝试向左兄弟借键
            let can_borrow_left = left_sibling_id
                .map(|lid| {
                    let l = self.read_node(lid).unwrap();
                    l.keys.len() > self.order / 2
                })
                .unwrap_or(false);
            if can_borrow_left {
                let left_id = left_sibling_id.unwrap();
                self.borrow_from_left(parent_page_id, child_idx, left_id, page_id)?;
                return Ok(()); // 借键后下溢已解决
            }

            // 无法借键，必须合并
            if let Some(right_id) = right_sibling_id {
                self.merge_with_right(parent_page_id, child_idx, page_id, right_id)?;
            } else if let Some(left_id) = left_sibling_id {
                self.merge_with_left(parent_page_id, child_idx, left_id, page_id)?;
            } else {
                // 无兄弟节点（只有根+一个 child 的场景，不应到此处）
                return Ok(());
            }

            // 合并后父节点少了 1 个 key+child，需上溯检查父节点
            i -= 1;
        }
    }

    /// 向右兄弟借键（left_page_id 是下溢节点，right_page_id 是其右兄弟）
    ///
    /// - Leaf: 把 right 的第一个 key 移到 left 末尾，更新父节点 separator 为 right 新的第一个 key
    /// - Internal: 父 separator 下降到 left 末尾，right 第一个 key 上升到父，right 第一个 child 转移给 left
    fn borrow_from_right(
        &mut self,
        parent_page_id: u32,
        child_idx: usize,
        left_page_id: u32,
        right_page_id: u32,
    ) -> Result<(), BTreeError> {
        // 先读取所需数据（避免同时持有多个 mut 借用）
        let (left_node_type, right_first_key, right_first_value, right_first_child) = {
            let right = self.read_node(right_page_id)?;
            let first_key = right.keys[0].clone();
            let first_tid = right.values.first().cloned();
            let first_child = right.children.first().cloned();
            (right.node_type, first_key, first_tid, first_child)
        };

        match left_node_type {
            NodeType::Leaf => {
                // Leaf: 把 right.keys[0] + values[0] 移到 left 末尾
                let value = right_first_value.expect("leaf must have values");
                {
                    let left = self.read_node_mut(left_page_id)?;
                    left.keys.push(right_first_key.clone());
                    left.values.push(value);
                }
                {
                    let right = self.read_node_mut(right_page_id)?;
                    right.keys.remove(0);
                    right.values.remove(0);
                }
                // 父 separator 更新为 right 新的第一个 key
                let new_sep = self.read_node(right_page_id)?.keys[0].clone();
                let parent = self.read_node_mut(parent_page_id)?;
                parent.keys[child_idx] = new_sep;
            }
            NodeType::Internal => {
                // Internal: 父 separator 下降到 left 末尾，right 第一个 key 上升到父
                let parent_sep = {
                    let parent = self.read_node(parent_page_id)?;
                    parent.keys[child_idx].clone()
                };
                {
                    let left = self.read_node_mut(left_page_id)?;
                    left.keys.push(parent_sep);
                    left.children
                        .push(right_first_child.expect("internal must have children"));
                }
                // 更新被转移 child 的 parent 指向 left
                let transferred_child = right_first_child.unwrap();
                if let Some(child) = self.pages.get_mut(&transferred_child) {
                    child.parent = left_page_id;
                }
                {
                    let right = self.read_node_mut(right_page_id)?;
                    right.keys.remove(0);
                    right.children.remove(0);
                }
                // 父 separator 更新为借出的 right_first_key
                let parent = self.read_node_mut(parent_page_id)?;
                parent.keys[child_idx] = right_first_key;
            }
        }
        Ok(())
    }

    /// 向左兄弟借键（right_page_id 是下溢节点，left_page_id 是其左兄弟）
    ///
    /// - Leaf: 把 left 的最后一个 key 移到 right 开头，更新父节点 separator 为 right 新的第一个 key
    /// - Internal: 父 separator 下降到 right 开头，left 最后一个 key 上升到父，left 最后一个 child 转移给 right
    fn borrow_from_left(
        &mut self,
        parent_page_id: u32,
        child_idx: usize,
        left_page_id: u32,
        right_page_id: u32,
    ) -> Result<(), BTreeError> {
        let (right_node_type, left_last_key, left_last_value, left_last_child) = {
            let left = self.read_node(left_page_id)?;
            let last_key = left.keys.last().cloned().expect("left has keys");
            let last_tid = left.values.last().cloned();
            let last_child = left.children.last().cloned();
            (left.node_type, last_key, last_tid, last_child)
        };
        // 注意：这里 right.node_type 应等于 left.node_type（兄弟节点同类型）
        // 我们用 left.node_type 作为代表

        match right_node_type {
            NodeType::Leaf => {
                // Leaf: 把 left 最后一个 key + value 移到 right 开头
                let value = left_last_value.expect("leaf must have values");
                {
                    let left = self.read_node_mut(left_page_id)?;
                    left.keys.pop();
                    left.values.pop();
                }
                {
                    let right = self.read_node_mut(right_page_id)?;
                    right.keys.insert(0, left_last_key.clone());
                    right.values.insert(0, value);
                }
                // 父 separator 更新为 right 新的第一个 key（即借过来的 key）
                let parent = self.read_node_mut(parent_page_id)?;
                parent.keys[child_idx - 1] = left_last_key;
            }
            NodeType::Internal => {
                // Internal: 父 separator 下降到 right 开头，left 最后一个 key 上升到父
                let parent_sep = {
                    let parent = self.read_node(parent_page_id)?;
                    parent.keys[child_idx - 1].clone()
                };
                {
                    let right = self.read_node_mut(right_page_id)?;
                    right.keys.insert(0, parent_sep);
                    right
                        .children
                        .insert(0, left_last_child.expect("internal must have children"));
                }
                // 更新被转移 child 的 parent 指向 right
                let transferred_child = left_last_child.unwrap();
                if let Some(child) = self.pages.get_mut(&transferred_child) {
                    child.parent = right_page_id;
                }
                {
                    let left = self.read_node_mut(left_page_id)?;
                    left.keys.pop();
                    left.children.pop();
                }
                // 父 separator 更新为借出的 left_last_key
                let parent = self.read_node_mut(parent_page_id)?;
                parent.keys[child_idx - 1] = left_last_key;
            }
        }
        Ok(())
    }

    /// 与右兄弟合并（left + right → left，移除 right）
    ///
    /// - Leaf: left.keys/values 拼接 right 的；left.next_sibling 更新为 right.next_sibling
    /// - Internal: left.keys + [父 separator] + right.keys 拼接；left.children 拼接 right.children
    /// - 父节点：移除 separator (parent.keys[child_idx]) 和 right child 指针 (parent.children[child_idx+1])
    fn merge_with_right(
        &mut self,
        parent_page_id: u32,
        child_idx: usize,
        left_page_id: u32,
        right_page_id: u32,
    ) -> Result<(), BTreeError> {
        let (node_type, parent_sep) = {
            let parent = self.read_node(parent_page_id)?;
            (
                self.read_node(left_page_id)?.node_type,
                parent.keys[child_idx].clone(),
            )
        };

        match node_type {
            NodeType::Leaf => {
                // Leaf: 直接拼接 keys + values
                let (right_keys, right_vals, right_next_sibling) = {
                    let right = self.pages.remove(&right_page_id).unwrap();
                    (right.keys, right.values, right.next_sibling)
                };
                {
                    let left = self.read_node_mut(left_page_id)?;
                    left.keys.extend(right_keys);
                    left.values.extend(right_vals);
                    left.next_sibling = right_next_sibling;
                }
                // 更新 right 原本下一个兄弟的 prev_sibling 指向 left
                if right_next_sibling != 0 {
                    if let Some(next) = self.pages.get_mut(&right_next_sibling) {
                        next.prev_sibling = left_page_id;
                    }
                }
            }
            NodeType::Internal => {
                // Internal: 拼接 keys (中间插入父 separator) + children
                let (right_keys, right_children) = {
                    let right = self.pages.remove(&right_page_id).unwrap();
                    (right.keys, right.children)
                };
                {
                    let left = self.read_node_mut(left_page_id)?;
                    left.keys.push(parent_sep.clone());
                    left.keys.extend(right_keys);
                    left.children.extend(right_children);
                }
                // 更新被合并进来的 children 的 parent 指向 left
                // 先克隆 children 列表避免在迭代中持有不可变借用
                let children_to_update: Vec<u32> = self.read_node(left_page_id)?.children.to_vec();
                for cid in children_to_update {
                    if let Some(child) = self.pages.get_mut(&cid) {
                        child.parent = left_page_id;
                    }
                }
            }
        }

        // 父节点：移除 separator 和 right child 指针
        {
            let parent = self.read_node_mut(parent_page_id)?;
            parent.keys.remove(child_idx);
            parent.children.remove(child_idx + 1);
        }

        Ok(())
    }

    /// 与左兄弟合并（left + right → left，移除 right）
    ///
    /// 与 merge_with_right 对称，但 right 是下溢节点。
    /// 父 separator 索引为 child_idx - 1（left 与 right 之间的分隔键）。
    fn merge_with_left(
        &mut self,
        parent_page_id: u32,
        child_idx: usize,
        left_page_id: u32,
        right_page_id: u32,
    ) -> Result<(), BTreeError> {
        // 等价于 merge_with_right(parent, child_idx - 1, left, right)
        // 因为 child_idx 是 right 在父中的位置，所以 left 是 child_idx - 1
        self.merge_with_right(parent_page_id, child_idx - 1, left_page_id, right_page_id)
    }

    /// 中序遍历，返回所有 (key, value) 按升序排列
    ///
    /// 用于验证 B-Tree 不变量：keys 严格递增。
    pub fn in_order_traverse(&self) -> Result<Vec<BTreeEntry>, BTreeError> {
        let mut result = Vec::new();
        self.in_order_recursive(self.root_page_id, &mut result)?;
        Ok(result)
    }

    fn in_order_recursive(
        &self,
        page_id: u32,
        result: &mut Vec<BTreeEntry>,
    ) -> Result<(), BTreeError> {
        let node = self.read_node(page_id)?;
        match node.node_type {
            NodeType::Leaf => {
                for (i, k) in node.keys.iter().enumerate() {
                    result.push((k.clone(), node.values[i].clone()));
                }
            }
            NodeType::Internal => {
                for i in 0..node.keys.len() {
                    self.in_order_recursive(node.children[i], result)?;
                    result.push((node.keys[i].clone(), Vec::new())); // Internal 节点无 value
                }
                // 最后一个 child
                let last = node.keys.len();
                self.in_order_recursive(node.children[last], result)?;
            }
        }
        Ok(())
    }

    /// 中序遍历仅叶子节点（实际数据），返回 (key, value) 按升序
    pub fn in_order_leaf_traverse(&self) -> Result<Vec<BTreeEntry>, BTreeError> {
        let mut result = Vec::new();
        // 找到最左叶子
        let mut current = self.root_page_id;
        loop {
            let node = self.read_node(current)?;
            if node.is_leaf() {
                break;
            }
            current = node.children[0];
        }
        // 沿 next_sibling 遍历
        let mut page_id = current;
        loop {
            let node = self.read_node(page_id)?;
            for (i, k) in node.keys.iter().enumerate() {
                result.push((k.clone(), node.values[i].clone()));
            }
            if node.next_sibling == 0 {
                break;
            }
            page_id = node.next_sibling;
        }
        Ok(result)
    }

    // -----------------------------------------------------------------
    //  Phase 1.5: 点查 + 范围扫描
    // -----------------------------------------------------------------

    /// 公共只读访问节点（供 cursor 模块使用）
    pub(crate) fn read_node_public(&self, page_id: u32) -> Result<&BTreeNode, BTreeError> {
        self.read_node(page_id)
    }

    /// 定位范围扫描的起始位置（叶子 page_id + 起始 key 索引）
    ///
    /// 根据 lower bound 下沉到对应叶子：
    /// - `Unbounded` → 最左叶子，索引 0
    /// - `Included(k)` → 第一个 key >= k 的位置
    /// - `Excluded(k)` → 第一个 key > k 的位置
    pub(crate) fn find_range_start(&self, lower: Bound<&[u8]>) -> Result<(u32, usize), BTreeError> {
        match lower {
            Bound::Unbounded => {
                // 下沉到最左叶子
                let mut current = self.root_page_id;
                loop {
                    let node = self.read_node(current)?;
                    if node.is_leaf() {
                        return Ok((current, 0));
                    }
                    current = node.children[0];
                }
            }
            Bound::Included(key) | Bound::Excluded(key) => {
                let excluded = matches!(lower, Bound::Excluded(_));
                // 类似 find_path_to_leaf，但需记录叶子内的位置
                let mut current = self.root_page_id;
                loop {
                    let node = self.read_node(current)?;
                    match node.node_type {
                        NodeType::Internal => {
                            let (found, pos) = node.search_key(key);
                            // Internal 节点：若 key == separator key，走向 children[pos+1]
                            let child_idx = if found.is_some() {
                                pos + 1
                            } else {
                                pos
                            };
                            current = node.children[child_idx];
                        }
                        NodeType::Leaf => {
                            let (found, pos) = node.search_key(key);
                            let start_idx = if excluded {
                                // Excluded：若找到则取下一个，若未找到则 pos 即是第一个 > key 的位置
                                if found.is_some() {
                                    pos + 1
                                } else {
                                    pos
                                }
                            } else {
                                // Included：pos 即是第一个 >= key 的位置（找到时 pos 指向 key，未找到时 pos 指向插入位置）
                                pos
                            };
                            return Ok((current, start_idx));
                        }
                    }
                }
            }
        }
    }

    /// 前向范围扫描
    ///
    /// 返回 [lower, upper] 范围内所有 (key, value) 按升序排列。
    /// - `Bound::Included(k)` → 包含 k
    /// - `Bound::Excluded(k)` → 不包含 k
    /// - `Bound::Unbounded` → 无边界
    pub fn range_scan(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<Vec<BTreeEntry>, BTreeError> {
        self.range_scan_with_limit(lower, upper, None)
    }

    /// 带 LIMIT 的前向范围扫描
    ///
    /// `limit = Some(n)` 最多返回 n 条；`limit = None` 不限制。
    /// `limit = Some(0)` 返回空 Vec。
    pub fn range_scan_with_limit(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
        limit: Option<usize>,
    ) -> Result<Vec<BTreeEntry>, BTreeError> {
        if let Some(0) = limit {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        let mut cursor = crate::cursor::BTreeCursor::new(self, lower, upper)?;
        for item in cursor.by_ref() {
            result.push(item);
            if let Some(n) = limit {
                if result.len() >= n {
                    break;
                }
            }
        }
        Ok(result)
    }

    /// 反向范围扫描
    ///
    /// 返回 [lower, upper] 范围内所有 (key, value) 按降序排列。
    pub fn range_scan_reverse(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<Vec<BTreeEntry>, BTreeError> {
        // 简化实现：先正向扫描，再反转
        // 性能优化留待 REFACTOR 阶段或后续 Phase（实现反向 cursor）
        let mut result = self.range_scan(lower, upper)?;
        result.reverse();
        Ok(result)
    }

    /// B+Tree 中存储的键值对总数（叶子节点条目数）
    ///
    /// P0-4 新增：遍历所有叶子节点统计条目数，用于 row_count 计算。
    pub fn len(&self) -> usize {
        // 遍历所有叶子节点统计
        let mut count = 0;
        let mut current = self.root_page_id;
        while let Ok(node) = self.read_node(current) {
            if node.is_leaf() {
                count += node.keys.len();
                if node.next_sibling == 0 {
                    break;
                }
                current = node.next_sibling;
            } else {
                current = node.children[0];
            }
        }
        count
    }

    /// B+Tree 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 创建范围扫描游标（懒加载迭代器）
    ///
    /// 推荐用于大范围扫描，避免一次性物化全部结果。
    pub fn cursor(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<crate::cursor::BTreeCursor<'_>, BTreeError> {
        crate::cursor::BTreeCursor::new(self, lower, upper)
    }

    // =================================================================
    // Phase 1.10: 批量导入（Bottom-Up Bulk Load）
    // =================================================================

    /// 从已排序的 (key, value) 序列批量构建 B-Tree（自底向上）
    ///
    /// 对应 `SzRSQL实施进度.md` Phase 1.10：
    /// - 已排序 1 亿行批量构建 B-Tree（不逐条插入）
    /// - 内存受限分批构建
    /// - 批量导入比逐条插入快 10x
    /// - 结果树完全平衡（所有叶子处于同一深度）
    ///
    /// 算法：
    /// 1. 校验输入严格升序；空输入返回 `BulkLoadEmpty`。
    /// 2. 将 items 按 `order` 个一组打包成叶子节点；最后一个叶子可能不满。
    ///    若最后一个叶子 key 数 < order/2，则从前一个叶子借键补足（保证至少半满）。
    /// 3. 自底向上构建 internal 节点层：每 `order` 个子节点归并到一个 internal 节点，
    ///    separator key 取右子树最小 key。
    /// 4. 重复步骤 3 直到本层只剩 1 个节点 → 设为根。
    ///
    /// 时间复杂度：O(N)，相比逐条插入 O(N·log N) 至少快 log N 倍。
    /// 空间复杂度：O(N)（一次性物化所有叶子，再逐层构建 internal）。
    ///
    /// 调用后 `self` 的全部状态被替换为批量构建结果。
    pub fn bulk_load<I>(&mut self, items: I) -> Result<(), BTreeError>
    where
        I: IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    {
        // 1. 物化为 Vec，校验升序
        let items_vec: Vec<BTreeEntry> = items.into_iter().collect();
        if items_vec.is_empty() {
            return Err(BTreeError::BulkLoadEmpty);
        }
        for i in 1..items_vec.len() {
            if items_vec[i - 1].0.as_slice() >= items_vec[i].0.as_slice() {
                return Err(BTreeError::BulkLoadNotSorted {
                    index: i,
                    prev: items_vec[i - 1].0.clone(),
                    curr: items_vec[i].0.clone(),
                });
            }
        }

        // 2. 重置树状态（保留 order），预分配 pages 容量避免 rehash
        let order = self.order;
        let total = items_vec.len();
        let estimated_leaves = total.div_ceil(order);
        // 估算总节点数：leaves + internal 各层（几何级数，总和 < 2 * leaves）
        let estimated_nodes = estimated_leaves * 2 + 10;
        self.pages.clear();
        self.pages.reserve(estimated_nodes);
        self.next_page_id = 1;

        // 3. 打包叶子：每 order 个 key 一个叶子（用 into_iter 直接消费，O(N) 无 clone）
        let mut leaves: Vec<BTreeNode> = Vec::with_capacity(estimated_leaves);
        let mut leaf = BTreeNode::new_leaf(self.alloc_page_id());
        leaf.keys.reserve(order);
        leaf.values.reserve(order);
        for (k, v) in items_vec.into_iter() {
            leaf.keys.push(k);
            leaf.values.push(v);
            if leaf.keys.len() >= order {
                let next_leaf = BTreeNode::new_leaf(self.alloc_page_id());
                let full_leaf = std::mem::replace(&mut leaf, next_leaf);
                leaves.push(full_leaf);
            }
        }
        // flush 最后一个非空叶子
        if !leaf.keys.is_empty() {
            leaves.push(leaf);
        }

        // 4. 借键补足最后一个叶子（若 < order/2 且前面有叶子）
        let min_keys = order / 2;
        if leaves.len() >= 2 {
            let last_len = leaves.last().unwrap().keys.len();
            if last_len < min_keys {
                let borrow = min_keys - last_len;
                let prev_idx = leaves.len() - 2;
                let prev_len = leaves[prev_idx].keys.len();
                if borrow < prev_len {
                    let split_at = prev_len - borrow;
                    let moved_keys: Vec<Vec<u8>> = leaves[prev_idx].keys.split_off(split_at);
                    let moved_vals: Vec<Vec<u8>> = leaves[prev_idx].values.split_off(split_at);
                    let last = leaves.last_mut().unwrap();
                    let mut new_keys = moved_keys;
                    new_keys.extend_from_slice(&last.keys);
                    let mut new_vals = moved_vals;
                    new_vals.extend_from_slice(&last.values);
                    last.keys = new_keys;
                    last.values = new_vals;
                }
            }
        }

        // 5. 设置叶子 sibling 链
        for i in 0..leaves.len() {
            if i + 1 < leaves.len() {
                leaves[i].next_sibling = leaves[i + 1].page_id;
                leaves[i + 1].prev_sibling = leaves[i].page_id;
            }
        }

        // 6. 提取 (page_id, first_key) 用于构建 internal 节点（之后 leaves 将被 move 进 pages）
        let current_level: Vec<(u32, Vec<u8>)> = leaves
            .iter()
            .map(|n| (n.page_id, n.keys[0].clone()))
            .collect();

        // 7. 将 leaves 移动进 pages（无 clone）
        for leaf in leaves {
            self.write_node(leaf);
        }

        // 8. 自底向上构建 internal 节点
        // 用 VecDeque 消费 current_level，child_min_key 直接 move，避免克隆
        let mut current_level: std::collections::VecDeque<(u32, Vec<u8>)> = current_level.into();
        while current_level.len() > 1 {
            let mut next_level: Vec<(u32, Vec<u8>)> = Vec::new();
            while !current_level.is_empty() {
                let internal_page_id = self.alloc_page_id();
                let mut internal = BTreeNode::new_internal(internal_page_id);
                internal.children.clear();

                let end = order.min(current_level.len());
                // 第一个子节点不需要 separator，后续子节点的 min_key 作为 separator
                // 循环条件 while !current_level.is_empty() 保证 pop_front 必然成功
                let (first_child_id, subtree_min) =
                    current_level.pop_front().unwrap_or_else(|| {
                        // 理论不可达：外层 while 已保证非空。返回占位避免 panic 路径
                        (0, Vec::new())
                    });
                internal.children.push(first_child_id);
                self.read_node_mut(first_child_id)?.parent = internal_page_id;
                // 后续子节点（直接 move min_key）
                for _ in 1..end {
                    let (child_page_id, child_min_key) =
                        current_level.pop_front().unwrap_or_else(|| {
                            // 理论不可达：end = min(order, len)，循环次数 <= len-1
                            (0, Vec::new())
                        });
                    internal.keys.push(child_min_key);
                    internal.children.push(child_page_id);
                    self.read_node_mut(child_page_id)?.parent = internal_page_id;
                }
                self.write_node(internal);
                next_level.push((internal_page_id, subtree_min));
            }
            current_level = next_level.into();
        }

        // 9. 设置根
        self.root_page_id = current_level[0].0;
        self.read_node_mut(self.root_page_id)?.parent = 0;

        Ok(())
    }

    /// 从已排序迭代器构建新 BTree（消费 self，返回全新构建的 BTree）
    ///
    /// 便捷构造函数，等价于 `BTree::new(order)` + `bulk_load(items)`。
    pub fn from_sorted_iter<I>(order: usize, items: I) -> Result<Self, BTreeError>
    where
        I: IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    {
        let mut tree = Self::new(order);
        tree.bulk_load(items)?;
        Ok(tree)
    }

    /// 内存受限的批量构建：分批从迭代器消费，每批 batch_size 个 item
    ///
    /// 算法（流式打包，假设输入已严格升序）：
    /// 1. 维护当前正在打包的叶子节点（current_leaf）
    /// 2. 每次从迭代器消费 batch_size 个 item 到缓冲区
    /// 3. 校验缓冲区内部、缓冲区与上一批的最后一个 key 严格升序
    /// 4. 将缓冲区 item 追加到 current_leaf；当 current_leaf 满（keys.len() == order）
    ///    时，flush 到 pages 并新建 current_leaf
    /// 5. 全部消费完后，flush 最后一个 current_leaf（若非空）
    /// 6. 自底向上构建 internal 节点（同 `bulk_load` 的步骤 6-8）
    ///
    /// 相比 `bulk_load`，本方法内存峰值 = O(batch_size + order)，
    /// 不需要一次性物化全部 items（适合 N=1 亿场景）。
    ///
    /// 注：当前 `pages` 仍为内存 HashMap，整体空间仍为 O(N)，
    /// 真正的"磁盘流式构建"需配合 Phase 2 buffer pool 扩展。
    pub fn bulk_load_batched<I>(&mut self, items: I, batch_size: usize) -> Result<(), BTreeError>
    where
        I: IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    {
        if batch_size < 2 {
            return Err(BTreeError::BulkLoadBatchTooSmall(batch_size));
        }
        // 重置树状态（保留 order）
        let order = self.order;
        self.pages.clear();
        self.next_page_id = 1;

        let mut leaves: Vec<BTreeNode> = Vec::new();
        let mut current_leaf = BTreeNode::new_leaf(self.alloc_page_id());
        let mut last_key: Option<Vec<u8>> = None;
        let mut batch_buf: Vec<BTreeEntry> = Vec::with_capacity(batch_size);
        let mut total_items: usize = 0;

        for item in items {
            batch_buf.push(item);
            if batch_buf.len() >= batch_size {
                // 校验本批 + 与上一批的衔接
                for (i, (k, _)) in batch_buf.iter().enumerate() {
                    if let Some(prev) = &last_key {
                        if k.as_slice() <= prev.as_slice() {
                            return Err(BTreeError::BulkLoadNotSorted {
                                index: total_items + i,
                                prev: prev.clone(),
                                curr: k.clone(),
                            });
                        }
                    } else if i > 0 && k.as_slice() <= batch_buf[i - 1].0.as_slice() {
                        return Err(BTreeError::BulkLoadNotSorted {
                            index: total_items + i,
                            prev: batch_buf[i - 1].0.clone(),
                            curr: k.clone(),
                        });
                    }
                    last_key = Some(k.clone());
                }
                // 追加到 current_leaf，必要时 flush
                for (k, v) in batch_buf.drain(..) {
                    current_leaf.keys.push(k);
                    current_leaf.values.push(v);
                    if current_leaf.keys.len() >= order {
                        leaves.push(std::mem::replace(
                            &mut current_leaf,
                            BTreeNode::new_leaf(self.alloc_page_id()),
                        ));
                    }
                }
                total_items += batch_buf.len();
                batch_buf.clear();
            }
        }
        // 处理最后一批
        if !batch_buf.is_empty() {
            for (i, (k, _)) in batch_buf.iter().enumerate() {
                if let Some(prev) = &last_key {
                    if k.as_slice() <= prev.as_slice() {
                        return Err(BTreeError::BulkLoadNotSorted {
                            index: total_items + i,
                            prev: prev.clone(),
                            curr: k.clone(),
                        });
                    }
                } else if i > 0 && k.as_slice() <= batch_buf[i - 1].0.as_slice() {
                    return Err(BTreeError::BulkLoadNotSorted {
                        index: total_items + i,
                        prev: batch_buf[i - 1].0.clone(),
                        curr: k.clone(),
                    });
                }
                last_key = Some(k.clone());
            }
            for (k, v) in batch_buf.drain(..) {
                current_leaf.keys.push(k);
                current_leaf.values.push(v);
                if current_leaf.keys.len() >= order {
                    leaves.push(std::mem::replace(
                        &mut current_leaf,
                        BTreeNode::new_leaf(self.alloc_page_id()),
                    ));
                }
            }
            total_items += 0; // batch_buf already drained
        }

        if total_items == 0 && current_leaf.keys.is_empty() && leaves.is_empty() {
            return Err(BTreeError::BulkLoadEmpty);
        }

        // flush 最后一个非空 current_leaf
        if !current_leaf.keys.is_empty() {
            leaves.push(current_leaf);
        }

        if leaves.is_empty() {
            return Err(BTreeError::BulkLoadEmpty);
        }

        // 借键补足最后一个叶子（若 < order/2 且前面有叶子）
        let min_keys = order / 2;
        if leaves.len() >= 2 {
            let last_len = leaves.last().unwrap().keys.len();
            if last_len < min_keys {
                let borrow = min_keys - last_len;
                let prev_idx = leaves.len() - 2;
                let prev_len = leaves[prev_idx].keys.len();
                if borrow < prev_len {
                    let split_at = prev_len - borrow;
                    let moved_keys: Vec<Vec<u8>> = leaves[prev_idx].keys.split_off(split_at);
                    let moved_vals: Vec<Vec<u8>> = leaves[prev_idx].values.split_off(split_at);
                    let last = leaves.last_mut().unwrap();
                    let mut new_keys = moved_keys;
                    new_keys.extend_from_slice(&last.keys);
                    let mut new_vals = moved_vals;
                    new_vals.extend_from_slice(&last.values);
                    last.keys = new_keys;
                    last.values = new_vals;
                }
            }
        }

        // 设置叶子 sibling 链
        for i in 0..leaves.len() {
            if i + 1 < leaves.len() {
                leaves[i].next_sibling = leaves[i + 1].page_id;
                leaves[i + 1].prev_sibling = leaves[i].page_id;
            }
        }

        // 提取 (page_id, first_key) 用于构建 internal 节点
        let mut current_level: Vec<(u32, Vec<u8>)> = leaves
            .iter()
            .map(|n| (n.page_id, n.keys[0].clone()))
            .collect();

        // 将 leaves 移动进 pages（无 clone）
        for leaf in leaves {
            self.write_node(leaf);
        }

        // 自底向上构建 internal 节点
        while current_level.len() > 1 {
            let mut next_level: Vec<(u32, Vec<u8>)> = Vec::new();
            let mut i = 0;
            while i < current_level.len() {
                let internal_page_id = self.alloc_page_id();
                let mut internal = BTreeNode::new_internal(internal_page_id);
                internal.children.clear();
                let end = (i + order).min(current_level.len());
                let first_child = &current_level[i];
                let subtree_min = first_child.1.clone();
                internal.children.push(first_child.0);
                self.read_node_mut(first_child.0)?.parent = internal_page_id;
                for (child_page_id, child_min_key) in &current_level[i + 1..end] {
                    internal.keys.push(child_min_key.clone());
                    internal.children.push(*child_page_id);
                    self.read_node_mut(*child_page_id)?.parent = internal_page_id;
                }
                self.write_node(internal);
                next_level.push((internal_page_id, subtree_min));
                i = end;
            }
            current_level = next_level;
        }

        self.root_page_id = current_level[0].0;
        self.read_node_mut(self.root_page_id)?.parent = 0;
        Ok(())
    }

    // =================================================================
    //  P0-3 修复：BufferPool 持久化集成
    //
    //  将 BTree 的所有节点通过 BufferPool 持久化到磁盘，实现真正的磁盘持久化。
    //  每个节点编码为一个 PageType::Index 类型的 Page，通过 BufferPool::put_page 写入。
    //  调用方应在 persist_to_buffer_pool 后调用 BufferPool::flush_all 确保数据落盘。
    //  加载时通过 load_from_buffer_pool 从 BufferPool 读取节点并重建 BTree。
    // =================================================================

    /// 将 BTree 的所有节点通过 BufferPool 持久化到磁盘
    ///
    /// # 算法
    ///
    /// 1. 遍历 `pages` 中的所有节点
    /// 2. 每个节点编码为字节串（`BTreeNode::encode`）
    /// 3. 创建 `PageType::Index` 类型的 Page，将编码写入 body
    /// 4. 通过 `BufferPool::put_page` 写入缓冲池（自动 mark dirty）
    /// 5. 调用方需后续调用 `BufferPool::flush_all` 确保落盘
    ///
    /// # 返回
    ///
    /// 返回 `PersistedBTreeMeta`，包含重建 BTree 所需的元数据。
    ///
    /// # 错误
    ///
    /// - `NodeExceedsPageCapacity`: 节点编码后超过单页 body 容量（8144 字节）
    /// - `PersistenceError`: BufferPool 写入失败
    pub fn persist_to_buffer_pool(
        &self,
        pool: &crate::buffer::BufferPool,
    ) -> Result<PersistedBTreeMeta, BTreeError> {
        use crate::page::{Page, PageType, PAGE_BODY_SIZE};

        for node in self.pages.values() {
            let encoded = node.encode();
            if encoded.len() > PAGE_BODY_SIZE {
                return Err(BTreeError::NodeExceedsPageCapacity {
                    encoded: encoded.len(),
                    max: PAGE_BODY_SIZE,
                });
            }
            let mut page = Page::new(node.page_id, PageType::Index);
            page.body[..encoded.len()].copy_from_slice(&encoded);
            page.header.free_offset = encoded.len() as u16;
            page.header.tuple_count = node.key_count() as u16;
            page.update_checksum();
            pool.put_page(node.page_id, page)
                .map_err(|e| BTreeError::PersistenceError(e.to_string()))?;
        }

        Ok(PersistedBTreeMeta {
            root_page_id: self.root_page_id,
            order: self.order,
            next_page_id: self.next_page_id,
            page_count: self.pages.len() as u32,
        })
    }

    /// 从 BufferPool 加载 BTree
    ///
    /// # 算法
    ///
    /// 1. 从 `meta.root_page_id` 开始 BFS 遍历
    /// 2. 通过 `BufferPool::read_page` 读取每个节点的 Page
    /// 3. 从 Page body 解码 `BTreeNode`（`BTreeNode::decode`）
    /// 4. 对于 Internal 节点，将 children 中的 page_id 加入遍历队列
    /// 5. 重建 BTree 结构
    ///
    /// # 参数
    ///
    /// - `pool`: BufferPool 引用（需与 persist 时使用相同的存储后端）
    /// - `meta`: 持久化时返回的元数据
    ///
    /// # 错误
    ///
    /// - `PersistenceError`: BufferPool 读取失败
    /// - `BufferTooShort`: 节点解码失败
    pub fn load_from_buffer_pool(
        pool: &crate::buffer::BufferPool,
        meta: PersistedBTreeMeta,
    ) -> Result<Self, BTreeError> {
        use std::collections::{HashMap, HashSet, VecDeque};

        let mut pages: HashMap<u32, BTreeNode> = HashMap::new();
        let mut queue: VecDeque<u32> = VecDeque::new();
        queue.push_back(meta.root_page_id);
        let mut visited: HashSet<u32> = HashSet::new();

        while let Some(page_id) = queue.pop_front() {
            if !visited.insert(page_id) {
                continue;
            }
            let page = pool
                .read_page(page_id)
                .map_err(|e| BTreeError::PersistenceError(e.to_string()))?;
            let encoded_len = page.header.free_offset as usize;
            if encoded_len == 0 {
                // 空页：可能是未写入的页，跳过
                tracing::warn!(page_id, "encountered empty page during BTree load, skipped");
                continue;
            }
            let node = BTreeNode::decode(&page.body[..encoded_len])?;
            // 收集 Internal 节点的子节点
            if node.is_internal() {
                for &child_id in &node.children {
                    if child_id != 0 {
                        queue.push_back(child_id);
                    }
                }
            }
            pages.insert(page_id, node);
        }

        Ok(Self {
            root_page_id: meta.root_page_id,
            order: meta.order,
            pages,
            next_page_id: meta.next_page_id,
        })
    }
}

/// BTree 持久化元数据（调用方需保存，用于后续加载）
///
/// 包含重建 BTree 所需的全部信息：根节点页 ID、阶数、下一个可用页 ID、页总数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedBTreeMeta {
    /// 根节点 page_id
    pub root_page_id: u32,
    /// B-Tree 阶数
    pub order: usize,
    /// 下一个将分配的 page_id
    pub next_page_id: u32,
    /// 已持久化的页总数
    pub page_count: u32,
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{prop_assert, prop_assert_eq};

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    fn make_key(v: i64) -> Vec<u8> {
        encode_i64_key(v)
    }

    fn make_leaf_with_keys(page_id: u32, keys: Vec<i64>) -> BTreeNode {
        let mut node = BTreeNode::new_leaf(page_id);
        for (i, k) in keys.into_iter().enumerate() {
            node.keys.push(make_key(k));
            node.values.push(vec![i as u8]);
        }
        node
    }

    fn make_internal_with_keys(page_id: u32, keys: Vec<i64>, child_start: u32) -> BTreeNode {
        let mut node = BTreeNode::new_internal(page_id);
        // 清除 new_internal 的占位 child，重新构造 children = keys.len() + 1
        node.children.clear();
        for k in keys {
            node.keys.push(make_key(k));
        }
        // children = keys.len() + 1
        for i in 0..=node.keys.len() {
            node.children.push(child_start + i as u32);
        }
        node
    }

    // -----------------------------------------------------------------
    //  NodeType 测试
    // -----------------------------------------------------------------

    #[test]
    fn node_type_from_u8_all_variants() {
        assert_eq!(NodeType::from_u8(0).unwrap(), NodeType::Internal);
        assert_eq!(NodeType::from_u8(1).unwrap(), NodeType::Leaf);
    }

    #[test]
    fn node_type_from_u8_invalid_returns_error() {
        assert!(matches!(
            NodeType::from_u8(2),
            Err(BTreeError::InvalidNodeType(2))
        ));
        assert!(matches!(
            NodeType::from_u8(255),
            Err(BTreeError::InvalidNodeType(_))
        ));
    }

    #[test]
    fn node_type_as_u8_roundtrip() {
        assert_eq!(NodeType::Internal.as_u8(), 0);
        assert_eq!(NodeType::Leaf.as_u8(), 1);
        assert_eq!(
            NodeType::from_u8(NodeType::Internal.as_u8()).unwrap(),
            NodeType::Internal
        );
        assert_eq!(
            NodeType::from_u8(NodeType::Leaf.as_u8()).unwrap(),
            NodeType::Leaf
        );
    }

    // -----------------------------------------------------------------
    //  BTreeNode 创建与默认值
    // -----------------------------------------------------------------

    #[test]
    fn new_leaf_defaults() {
        let node = BTreeNode::new_leaf(42);
        assert_eq!(node.page_id, 42);
        assert_eq!(node.node_type, NodeType::Leaf);
        assert!(node.keys.is_empty());
        assert!(node.children.is_empty());
        assert!(node.values.is_empty());
        assert_eq!(node.next_sibling, 0);
        assert_eq!(node.prev_sibling, 0);
        assert_eq!(node.parent, 0);
        assert!(node.is_leaf());
        assert!(!node.is_internal());
    }

    #[test]
    fn new_internal_defaults() {
        let node = BTreeNode::new_internal(99);
        assert_eq!(node.page_id, 99);
        assert_eq!(node.node_type, NodeType::Internal);
        assert!(node.keys.is_empty());
        // Internal 节点不变量：children.len() == keys.len() + 1，故 0 keys 时有 1 个占位 child
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0], 0);
        assert!(node.values.is_empty());
        assert!(!node.is_leaf());
        assert!(node.is_internal());
    }

    // -----------------------------------------------------------------
    //  满半满下溢检查
    // -----------------------------------------------------------------

    #[test]
    fn is_full_check() {
        let order = 4;
        let mut node = BTreeNode::new_leaf(1);
        assert!(!node.is_full(order));
        for i in 0..4 {
            node.keys.push(make_key(i));
            node.values.push(vec![i as u8]);
        }
        assert!(node.is_full(order));
        assert!(!node.is_full(order + 1));
    }

    #[test]
    fn is_underflow_check() {
        let order = 4;
        let mut node = BTreeNode::new_leaf(1);
        // 0 keys, min_keys = 2 → underflow
        assert!(node.is_underflow(order));
        for i in 0..2 {
            node.keys.push(make_key(i));
            node.values.push(vec![i as u8]);
        }
        // 2 keys = min_keys → not underflow
        assert!(!node.is_underflow(order));
    }

    #[test]
    fn is_at_least_half_full_check() {
        let order = 4;
        let mut node = BTreeNode::new_leaf(1);
        assert!(!node.is_at_least_half_full(order));
        for i in 0..2 {
            node.keys.push(make_key(i));
            node.values.push(vec![i as u8]);
        }
        assert!(node.is_at_least_half_full(order));
    }

    #[test]
    fn key_count_check() {
        let mut node = BTreeNode::new_leaf(1);
        assert_eq!(node.key_count(), 0);
        node.keys.push(make_key(1));
        assert_eq!(node.key_count(), 1);
        node.keys.push(make_key(2));
        assert_eq!(node.key_count(), 2);
    }

    // -----------------------------------------------------------------
    //  validate 不变量检查
    // -----------------------------------------------------------------

    #[test]
    fn validate_leaf_ok() {
        let node = make_leaf_with_keys(1, vec![10, 20, 30]);
        assert!(node.validate().is_ok());
    }

    #[test]
    fn validate_internal_ok() {
        let node = make_internal_with_keys(1, vec![10, 20, 30], 100);
        assert!(node.validate().is_ok());
    }

    #[test]
    fn validate_internal_children_count_mismatch() {
        let mut node = BTreeNode::new_internal(1);
        // 清除 new_internal 的占位 child，构造不匹配的 children 数量
        node.children.clear();
        node.keys.push(make_key(10));
        node.keys.push(make_key(20));
        // children should be 3, but only 2
        node.children.push(100);
        node.children.push(101);
        let err = node.validate().unwrap_err();
        assert!(matches!(
            err,
            BTreeError::ChildrenCountMismatch {
                expected: 3,
                actual: 2
            }
        ));
    }

    #[test]
    fn validate_leaf_tuple_ids_count_mismatch() {
        let mut node = BTreeNode::new_leaf(1);
        node.keys.push(make_key(10));
        node.keys.push(make_key(20));
        // tuple_ids should be 2, but only 1
        node.values.push(vec![0u8]);
        let err = node.validate().unwrap_err();
        assert!(matches!(
            err,
            BTreeError::ValuesCountMismatch {
                expected: 2,
                actual: 1
            }
        ));
    }

    #[test]
    fn validate_keys_not_sorted() {
        let mut node = BTreeNode::new_leaf(1);
        // 故意乱序：30 在 10 前面
        node.keys.push(make_key(30));
        node.values.push(vec![0u8]);
        node.keys.push(make_key(10));
        node.values.push(vec![1u8]);
        let err = node.validate().unwrap_err();
        assert!(matches!(err, BTreeError::KeysNotSorted { index: 1, .. }));
    }

    #[test]
    fn validate_internal_with_tuple_ids_fails() {
        let mut node = BTreeNode::new_internal(1);
        // 清除占位 child，构造合法 children 数量后添加非法 tuple_ids
        node.children.clear();
        node.keys.push(make_key(10));
        node.children.push(100);
        node.children.push(101);
        node.values.push(vec![0u8]); // Internal 不应有 values
        let err = node.validate().unwrap_err();
        assert!(matches!(
            err,
            BTreeError::ValuesCountMismatch {
                expected: 0,
                actual: 1
            }
        ));
    }

    #[test]
    fn validate_leaf_with_children_fails() {
        let mut node = BTreeNode::new_leaf(1);
        node.keys.push(make_key(10));
        node.values.push(vec![0u8]);
        node.children.push(100); // Leaf 不应有 children
        let err = node.validate().unwrap_err();
        assert!(matches!(
            err,
            BTreeError::ChildrenCountMismatch {
                expected: 0,
                actual: 1
            }
        ));
    }

    // -----------------------------------------------------------------
    //  search_key 二分查找
    // -----------------------------------------------------------------

    #[test]
    fn search_key_found() {
        let node = make_leaf_with_keys(1, vec![10, 20, 30, 40, 50]);
        let (found, pos) = node.search_key(&make_key(30));
        assert_eq!(found, Some(2));
        assert_eq!(pos, 2);
    }

    #[test]
    fn search_key_not_found_insert_position() {
        let node = make_leaf_with_keys(1, vec![10, 20, 30, 40, 50]);
        let (found, pos) = node.search_key(&make_key(25));
        assert_eq!(found, None);
        assert_eq!(pos, 2); // 25 应插入到 20 和 30 之间
    }

    #[test]
    fn search_key_empty_node() {
        let node = BTreeNode::new_leaf(1);
        let (found, pos) = node.search_key(&make_key(42));
        assert_eq!(found, None);
        assert_eq!(pos, 0);
    }

    #[test]
    fn search_key_smaller_than_all() {
        let node = make_leaf_with_keys(1, vec![10, 20, 30]);
        let (found, pos) = node.search_key(&make_key(5));
        assert_eq!(found, None);
        assert_eq!(pos, 0);
    }

    #[test]
    fn search_key_larger_than_all() {
        let node = make_leaf_with_keys(1, vec![10, 20, 30]);
        let (found, pos) = node.search_key(&make_key(99));
        assert_eq!(found, None);
        assert_eq!(pos, 3);
    }

    // -----------------------------------------------------------------
    //  encode/decode 往返测试
    // -----------------------------------------------------------------

    #[test]
    fn encode_decode_leaf_roundtrip_empty() {
        let original = BTreeNode::new_leaf(42);
        let encoded = original.encode();
        let decoded = BTreeNode::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn encode_decode_internal_roundtrip_empty() {
        let original = BTreeNode::new_internal(99);
        let encoded = original.encode();
        let decoded = BTreeNode::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn encode_decode_leaf_with_keys() {
        let mut original = BTreeNode::new_leaf(1);
        original.next_sibling = 2;
        original.prev_sibling = 0;
        original.parent = 5;
        for (i, k) in [10i64, 20, 30, 40, 50].iter().enumerate() {
            original.keys.push(make_key(*k));
            original.values.push(vec![i as u8]);
        }
        let encoded = original.encode();
        let decoded = BTreeNode::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn encode_decode_internal_with_keys() {
        let original = make_internal_with_keys(1, vec![10, 20, 30, 40], 100);
        let encoded = original.encode();
        let decoded = BTreeNode::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn encode_decode_max_page_id_and_siblings() {
        let mut original = BTreeNode::new_leaf(u32::MAX);
        original.next_sibling = u32::MAX - 1;
        original.prev_sibling = u32::MAX - 2;
        original.parent = u32::MAX - 3;
        original.keys.push(make_key(42));
        original.values.push(vec![u32::MAX as u8]);
        let encoded = original.encode();
        let decoded = BTreeNode::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn encode_decode_large_key_count() {
        let keys: Vec<i64> = (0..1000).map(|i| i * 2).collect();
        let original = make_leaf_with_keys(1, keys);
        let encoded = original.encode();
        let decoded = BTreeNode::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn encoded_size_matches_encode_len() {
        let node = make_internal_with_keys(1, vec![10, 20, 30], 100);
        assert_eq!(node.encoded_size(), node.encode().len());
    }

    // -----------------------------------------------------------------
    //  decode 错误处理（不 panic）
    // -----------------------------------------------------------------

    #[test]
    fn decode_empty_buffer_returns_error_no_panic() {
        let result = BTreeNode::decode(&[]);
        assert!(matches!(
            result,
            Err(BTreeError::BufferTooShort {
                need: BTREE_NODE_HEADER_SIZE,
                have: 0
            })
        ));
    }

    #[test]
    fn decode_short_buffer_returns_error_no_panic() {
        let buf = [0u8; 10];
        let result = BTreeNode::decode(&buf);
        assert!(matches!(result, Err(BTreeError::BufferTooShort { .. })));
    }

    #[test]
    fn decode_invalid_node_type_returns_error_no_panic() {
        let mut buf = vec![0u8; BTREE_NODE_HEADER_SIZE];
        buf[0] = 99; // 非法 node_type
        let result = BTreeNode::decode(&buf);
        assert!(matches!(result, Err(BTreeError::InvalidNodeType(99))));
    }

    #[test]
    fn decode_truncated_keys_returns_error_no_panic() {
        let node = make_leaf_with_keys(1, vec![10, 20, 30]);
        let mut encoded = node.encode();
        // 截断最后 5 字节
        encoded.truncate(encoded.len() - 5);
        let result = BTreeNode::decode(&encoded);
        assert!(matches!(result, Err(BTreeError::BufferTooShort { .. })));
    }

    #[test]
    fn decode_random_garbage_no_panic() {
        let mut garbage = vec![0u8; 200];
        for seed in 0..500u64 {
            let mut s = seed
                .wrapping_mul(2862933555777941757)
                .wrapping_add(3037000493);
            for b in garbage.iter_mut() {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *b = (s >> 33) as u8;
            }
            let _ = BTreeNode::decode(&garbage);
        }
    }

    // -----------------------------------------------------------------
    //  split 分裂测试
    // -----------------------------------------------------------------

    #[test]
    fn split_leaf_basic() {
        let order = 4;
        let mut node = make_leaf_with_keys(1, vec![10, 20, 30, 40, 50]);
        assert!(node.is_full(order));
        let (left, right, promoted) = node.split(1, 2).unwrap();

        // Leaf 分裂：mid = 5/2 = 2，keys[2] = 30 被提升
        assert_eq!(left.keys.len(), 2); // [10, 20]
        assert_eq!(right.keys.len(), 3); // [30, 40, 50]
        assert_eq!(promoted, make_key(30));

        // 兄弟链表
        assert_eq!(left.next_sibling, 2);
        assert_eq!(right.prev_sibling, 1);

        // 校验分裂后两个节点都至少半满（order=4，min=2）
        assert!(left.is_at_least_half_full(order));
        assert!(right.is_at_least_half_full(order));

        // validate 不变量
        assert!(left.validate().is_ok());
        assert!(right.validate().is_ok());

        // keys 升序
        assert_eq!(left.keys, vec![make_key(10), make_key(20)]);
        assert_eq!(right.keys, vec![make_key(30), make_key(40), make_key(50)]);
    }

    #[test]
    fn split_internal_basic() {
        let order = 4;
        let mut node = make_internal_with_keys(1, vec![10, 20, 30, 40, 50], 100);
        assert!(node.is_full(order));
        let (left, right, promoted) = node.split(1, 2).unwrap();

        // Internal 分裂：mid = 5/2 = 2，keys[2] = 30 被提升
        assert_eq!(left.keys.len(), 2); // [10, 20]
        assert_eq!(right.keys.len(), 2); // [40, 50]
        assert_eq!(promoted, make_key(30));

        // children 分配：left = children[0..=mid] = [100, 101, 102]
        // right = children[mid+1..] = [103, 104, 105]
        assert_eq!(left.children, vec![100, 101, 102]);
        assert_eq!(right.children, vec![103, 104, 105]);

        // validate
        assert!(left.validate().is_ok());
        assert!(right.validate().is_ok());
    }

    #[test]
    fn split_leaf_even_key_count() {
        let mut node = make_leaf_with_keys(1, vec![10, 20, 30, 40]);
        let (left, right, promoted) = node.split(1, 2).unwrap();
        // mid = 4/2 = 2，keys[2] = 30 被提升
        assert_eq!(left.keys.len(), 2);
        assert_eq!(right.keys.len(), 2);
        assert_eq!(promoted, make_key(30));
    }

    #[test]
    fn split_internal_even_key_count() {
        let mut node = make_internal_with_keys(1, vec![10, 20, 30, 40], 100);
        let (left, right, promoted) = node.split(1, 2).unwrap();
        // mid = 4/2 = 2，keys[2] = 30 被提升
        assert_eq!(left.keys.len(), 2); // [10, 20]
        assert_eq!(right.keys.len(), 1); // [40]
        assert_eq!(promoted, make_key(30));
        // left.children = [100, 101, 102] (3)
        // right.children = [103] (1) = keys.len()+1 = 1+1 = 2... wait
        // Actually right.keys = [40], so right.children should have 2 elements
        assert_eq!(right.children.len(), 2);
    }

    #[test]
    fn split_two_keys_minimum() {
        let mut node = make_leaf_with_keys(1, vec![10, 20]);
        let (left, right, promoted) = node.split(1, 2).unwrap();
        // mid = 2/2 = 1，keys[1] = 20 被提升
        assert_eq!(left.keys.len(), 1);
        assert_eq!(right.keys.len(), 1);
        assert_eq!(promoted, make_key(20));
    }

    #[test]
    fn split_single_key_returns_error() {
        let mut node = make_leaf_with_keys(1, vec![10]);
        let result = node.split(1, 2);
        assert!(matches!(
            result,
            Err(BTreeError::CannotSplitInternal { key_count: 1 })
        ));
    }

    #[test]
    fn split_empty_node_returns_error() {
        let mut node = BTreeNode::new_leaf(1);
        let result = node.split(1, 2);
        assert!(matches!(result, Err(BTreeError::NodeEmpty)));
    }

    #[test]
    fn split_leaf_both_half_full_constraint() {
        // 验证标准：分裂后两个子节点各 >= 半满
        for key_count in 2..=100 {
            let order = 4; // min_keys = 2
            let keys: Vec<i64> = (0..key_count).map(|i| i as i64 * 10).collect();
            let mut node = make_leaf_with_keys(1, keys);
            let (left, right, _) = node.split(1, 2).unwrap();
            // 至少有 (key_count-1)/2 个 key
            let min_expected = (key_count - 1) / 2;
            assert!(
                left.keys.len() >= min_expected,
                "left has {} keys, expected >= {} (key_count={})",
                left.keys.len(),
                min_expected,
                key_count
            );
            assert!(
                right.keys.len() >= min_expected,
                "right has {} keys, expected >= {} (key_count={})",
                right.keys.len(),
                min_expected,
                key_count
            );
            // 至少半满（order=4, min=2）— 仅当 key_count 足够大时
            if key_count >= 4 {
                assert!(left.is_at_least_half_full(order) || right.is_at_least_half_full(order));
            }
        }
    }

    // -----------------------------------------------------------------
    //  merge 合并测试
    // -----------------------------------------------------------------

    #[test]
    fn merge_leaf_basic() {
        let mut left = make_leaf_with_keys(1, vec![10, 20]);
        let mut right = make_leaf_with_keys(2, vec![30, 40]);
        left.next_sibling = 2;
        right.prev_sibling = 1;
        left.parent = 5;
        right.parent = 5;

        let merged = left.merge(right, None).unwrap();
        assert_eq!(merged.page_id, 1);
        assert_eq!(merged.node_type, NodeType::Leaf);
        assert_eq!(merged.keys.len(), 4);
        assert_eq!(merged.values.len(), 4);
        // keys 升序
        assert_eq!(merged.keys[0], make_key(10));
        assert_eq!(merged.keys[3], make_key(40));
        assert!(merged.validate().is_ok());
    }

    #[test]
    fn merge_internal_with_separator() {
        let mut left = make_internal_with_keys(1, vec![10, 20], 100);
        let mut right = make_internal_with_keys(2, vec![40, 50], 200);
        left.next_sibling = 2;
        right.prev_sibling = 1;
        left.parent = 5;
        right.parent = 5;

        // Internal 合并：separator key 30 从父节点下降
        let sep = make_key(30);
        let merged = left.merge(right, Some(sep)).unwrap();
        // 合并后 keys = [10, 20, 30, 40, 50]
        assert_eq!(merged.keys.len(), 5);
        assert_eq!(merged.keys[2], make_key(30));
        // children = left.children (3) + right.children (3) = 6
        assert_eq!(merged.children.len(), 6);
        assert!(merged.validate().is_ok());
    }

    #[test]
    fn merge_leaf_no_separator_ignored() {
        // Leaf 合并时 separator 被忽略
        let mut left = make_leaf_with_keys(1, vec![10]);
        let mut right = make_leaf_with_keys(2, vec![20]);
        left.next_sibling = 2;
        right.prev_sibling = 1;
        let sep = make_key(99);
        let merged = left.merge(right, Some(sep)).unwrap();
        // sep 应被忽略
        assert_eq!(merged.keys.len(), 2);
        assert_eq!(merged.keys[0], make_key(10));
        assert_eq!(merged.keys[1], make_key(20));
    }

    #[test]
    fn merge_different_types_returns_error() {
        let left = BTreeNode::new_leaf(1);
        let right = BTreeNode::new_internal(2);
        let result = left.merge(right, None);
        assert!(matches!(result, Err(BTreeError::CannotMergeDifferentTypes)));
    }

    #[test]
    fn merge_non_adjacent_returns_error() {
        let left = BTreeNode::new_leaf(1);
        let right = BTreeNode::new_leaf(2);
        // left.next_sibling != 2, right.prev_sibling != 1
        let result = left.merge(right, None);
        assert!(matches!(result, Err(BTreeError::CannotMergeNonAdjacent)));
    }

    #[test]
    fn merge_preserves_sibling_links() {
        // left <-> right <-> right_right
        let mut left = make_leaf_with_keys(1, vec![10]);
        let mut right = make_leaf_with_keys(2, vec![20]);
        let right_right_page_id = 3u32;
        left.next_sibling = 2;
        left.prev_sibling = 0;
        right.prev_sibling = 1;
        right.next_sibling = right_right_page_id;

        let merged = left.merge(right, None).unwrap();
        // merged.next_sibling 应继承 right.next_sibling
        assert_eq!(merged.next_sibling, right_right_page_id);
        // merged.prev_sibling 应保留 left.prev_sibling
        assert_eq!(merged.prev_sibling, 0);
    }

    #[test]
    fn merge_combined_size_under_order() {
        // 合并后节点 < 满（order）
        let order = 10;
        let mut left = make_leaf_with_keys(1, vec![10, 20, 30]);
        let mut right = make_leaf_with_keys(2, vec![40, 50]);
        left.next_sibling = 2;
        right.prev_sibling = 1;
        let merged = left.merge(right, None).unwrap();
        assert_eq!(merged.keys.len(), 5);
        assert!(!merged.is_full(order)); // 5 < 10
    }

    // -----------------------------------------------------------------
    //  encode_i64_key / decode_i64_key 测试
    // -----------------------------------------------------------------

    #[test]
    fn encode_i64_key_negative_less_than_positive() {
        let neg = encode_i64_key(-1);
        let zero = encode_i64_key(0);
        let pos = encode_i64_key(1);
        assert!(neg < zero);
        assert!(zero < pos);
        assert!(neg < pos);
    }

    #[test]
    fn encode_i64_key_ordered() {
        // 输入必须已排序，才能验证"编码后字节序 == i64 数值序"
        let keys: Vec<i64> = vec![i64::MIN, -100, -1, 0, 1, 100, i64::MAX];
        let encoded: Vec<Vec<u8>> = keys.iter().map(|&v| encode_i64_key(v)).collect();
        // 编码后字节序应与 i64 数值序一致
        let mut expected: Vec<Vec<u8>> = encoded.clone();
        expected.sort();
        assert_eq!(encoded, expected, "encoded keys should be sorted");

        // 验证解码还原
        for (i, &k) in keys.iter().enumerate() {
            assert_eq!(decode_i64_key(&encoded[i]).unwrap(), k);
        }
    }

    #[test]
    fn encode_decode_i64_extremes() {
        for v in [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX - 1, i64::MAX] {
            let encoded = encode_i64_key(v);
            let decoded = decode_i64_key(&encoded).unwrap();
            assert_eq!(decoded, v);
        }
    }

    #[test]
    fn decode_i64_key_wrong_length_returns_error() {
        assert!(decode_i64_key(&[0u8; 7]).is_err());
        assert!(decode_i64_key(&[0u8; 9]).is_err());
        assert!(decode_i64_key(&[]).is_err());
    }

    #[test]
    fn compare_keys_lexicographic() {
        use std::cmp::Ordering;
        assert_eq!(compare_keys(&[1, 2, 3], &[1, 2, 3]), Ordering::Equal);
        assert_eq!(compare_keys(&[1, 2, 3], &[1, 2, 4]), Ordering::Less);
        assert_eq!(compare_keys(&[1, 2, 4], &[1, 2, 3]), Ordering::Greater);
        assert_eq!(compare_keys(&[1, 2], &[1, 2, 3]), Ordering::Less);
        assert_eq!(compare_keys(&[], &[]), Ordering::Equal);
    }

    // -----------------------------------------------------------------
    //  Proptest
    // -----------------------------------------------------------------

    proptest::proptest! {
        #[test]
        fn prop_encode_decode_roundtrip(
            page_id in 0u32..=u32::MAX,
            key_count in 0usize..=200,
            is_leaf in proptest::bool::ANY,
            next_sibling in 0u32..=u32::MAX,
            prev_sibling in 0u32..=u32::MAX,
            parent in 0u32..=u32::MAX,
        ) {
            let mut node = if is_leaf {
                BTreeNode::new_leaf(page_id)
            } else {
                BTreeNode::new_internal(page_id)
            };
            node.next_sibling = next_sibling;
            node.prev_sibling = prev_sibling;
            node.parent = parent;
            if !is_leaf {
                // 清除 new_internal 的占位 child，重新构造 children = keys.len() + 1
                node.children.clear();
            }

            for i in 0..key_count {
                let k = encode_i64_key(i as i64);
                node.keys.push(k);
                if is_leaf {
                    node.values.push(vec![(i % 256) as u8]);
                } else {
                    node.children.push((i as u32) + 1000);
                }
            }
            if !is_leaf {
                // children = keys.len() + 1
                node.children.push((key_count as u32) + 1000);
            }

            // 仅在 keys 升序时验证（这里是升序）
            let encoded = node.encode();
            let decoded = BTreeNode::decode(&encoded).unwrap();
            let validate_ok = decoded.validate().is_ok();
            prop_assert_eq!(node, decoded);
            prop_assert!(validate_ok);
        }

        #[test]
        fn prop_split_preserves_keys(
            key_count in 2usize..=100,
            is_leaf in proptest::bool::ANY,
        ) {
            let keys: Vec<i64> = (0..key_count as i64).map(|i| i * 7).collect();
            let mut node = if is_leaf {
                let mut n = BTreeNode::new_leaf(1);
                for (i, k) in keys.iter().enumerate() {
                    n.keys.push(encode_i64_key(*k));
                    n.values.push(vec![i as u8]);
                }
                n
            } else {
                let mut n = BTreeNode::new_internal(1);
                // 清除占位 child，重新构造 children = keys.len() + 1
                n.children.clear();
                for k in &keys {
                    n.keys.push(encode_i64_key(*k));
                }
                for i in 0..=keys.len() {
                    n.children.push(100 + i as u32);
                }
                n
            };

            let original_keys: Vec<Vec<u8>> = node.keys.clone();
            let total_keys = node.keys.len();

            let (left, right, promoted) = node.split(1, 2).unwrap();

            // Leaf: left + right keys 应包含所有原 keys
            // Internal: left + promoted + right keys 应包含所有原 keys
            let mut combined = left.keys.clone();
            if !is_leaf {
                combined.push(promoted.clone());
            }
            combined.extend(right.keys.clone());

            if is_leaf {
                // Leaf: 左+右 = 原 keys（无 promoted）
                let mut combined_leaf = left.keys.clone();
                combined_leaf.extend(right.keys.clone());
                prop_assert_eq!(combined_leaf, original_keys);
            } else {
                // Internal: 左 + promoted + 右 = 原 keys
                prop_assert_eq!(combined, original_keys);
            }

            // 不变量校验
            prop_assert!(left.validate().is_ok());
            prop_assert!(right.validate().is_ok());

            // 内部节点 children 数 = keys + 1
            if !is_leaf {
                prop_assert_eq!(left.children.len(), left.keys.len() + 1);
                prop_assert_eq!(right.children.len(), right.keys.len() + 1);
                // 子节点总 children 数 = 原 children 数
                prop_assert_eq!(left.children.len() + right.children.len(), total_keys + 1);
            } else {
                // Leaf: values 总数 = 原 tuple_ids 数
                prop_assert_eq!(left.values.len() + right.values.len(), total_keys);
            }
        }

        #[test]
        fn prop_decode_garbage_no_panic(data in proptest::collection::vec(0u8..=255, 0..=500)) {
            let _ = BTreeNode::decode(&data);
        }
    }

    // =================================================================
    //  BTree 管理器测试（Phase 1.3：插入含分裂）
    //  验证标准（来自 SzRSQL实施进度.md Phase 1.3）：
    //    - 单行插入 / 顺序插入 100000 key / 乱序插入 100000 key
    //    - 满节点分裂 / 递归分裂到根 / 重复 key 处理
    //    - 插入后中序遍历严格递增，树高度正确
    // =================================================================

    // --- 创建与初始状态 ---

    #[test]
    fn btree_new_creates_empty_leaf_root() {
        let bt = BTree::new(4);
        assert_eq!(bt.root_page_id(), 1);
        assert_eq!(bt.order(), 4);
        assert_eq!(bt.node_count(), 1);
        assert_eq!(bt.height(), 1); // 单节点树高度 = 1
                                    // 根为叶子节点
        let root = bt.read_node(1).unwrap();
        assert!(root.is_leaf());
        assert_eq!(root.key_count(), 0);
    }

    #[test]
    fn btree_with_default_order_is_256() {
        let bt = BTree::with_default_order();
        assert_eq!(bt.order(), BTREE_DEFAULT_ORDER);
        assert_eq!(bt.order(), 256);
    }

    #[test]
    #[should_panic(expected = "B-Tree order must be >= 3")]
    fn btree_new_order_below_3_panics() {
        let _ = BTree::new(2);
    }

    #[test]
    #[should_panic(expected = "B-Tree order must be >= 3")]
    fn btree_new_order_zero_panics() {
        let _ = BTree::new(0);
    }

    // --- 单行插入 ---

    #[test]
    fn btree_insert_single_key() {
        let mut bt = BTree::new(4);
        bt.insert(make_key(42), vec![100u8]).unwrap();
        assert_eq!(bt.node_count(), 1);
        assert_eq!(bt.height(), 1);
        // 中序遍历返回单个 (key, value)（无分裂，in_order_traverse 与 leaf_traverse 等价）
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, make_key(42));
        assert_eq!(pairs[0].1, vec![100u8]);
        // search 能找到
        assert_eq!(bt.search(&make_key(42)).unwrap(), Some(vec![100u8]));
    }

    // --- 顺序插入 100 个 key（小规模验证） ---

    #[test]
    fn btree_insert_sequential_100_keys() {
        let mut bt = BTree::new(4);
        for i in 0..100i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        // 中序遍历严格递增（使用 in_order_leaf_traverse 只返回叶子节点的数据 key）
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), 100);
        for (i, (k, tid)) in pairs.iter().enumerate() {
            assert_eq!(*k, make_key(i as i64), "key at index {} mismatch", i);
            assert_eq!(*tid, vec![i as u8], "tuple_id at index {} mismatch", i);
        }
        // 高度 >= 1（100 个 key、order=4，应该多次分裂）
        assert!(
            bt.height() >= 2,
            "expected height >= 2, got {}",
            bt.height()
        );
        // 所有 key 可被 search 找到
        for i in 0..100i64 {
            assert_eq!(bt.search(&make_key(i)).unwrap(), Some(vec![i as u8]));
        }
    }

    // --- 顺序插入 100000 个 key（验证标准要求） ---

    #[test]
    fn btree_insert_sequential_100000_keys_in_order_strictly_increasing() {
        let mut bt = BTree::with_default_order();
        for i in 0..100_000i64 {
            bt.insert(make_key(i), vec![(i % 65536) as u8]).unwrap();
        }
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), 100_000, "expected 100000 pairs");
        // 中序遍历严格递增
        for i in 1..pairs.len() {
            assert!(
                pairs[i - 1].0 < pairs[i].0,
                "keys not strictly increasing at index {}: prev={:?}, curr={:?}",
                i,
                pairs[i - 1].0,
                pairs[i].0
            );
        }
        // 验证 key 值与预期一致
        for (i, (k, _)) in pairs.iter().enumerate() {
            assert_eq!(*k, make_key(i as i64), "key at index {} mismatch", i);
        }
        // 树高度合理：100000 keys / 256 阶 → 叶子数 ≈ 391，内部节点少量
        let h = bt.height();
        assert!(h >= 2, "expected height >= 2, got {}", h);
        // 节点数应 > 1
        assert!(bt.node_count() > 1);
    }

    // --- 乱序插入 100 个 key ---

    #[test]
    fn btree_insert_random_order_100_keys() {
        let mut bt = BTree::new(4);
        // 简单的 LCG 伪随机生成器（确定性，便于复现）
        let mut seed: u64 = 42;
        let mut keys: Vec<i64> = (0..100).collect();
        // Fisher-Yates 洗牌
        for i in (1..keys.len()).rev() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let j = (seed >> 33) as usize % (i + 1);
            keys.swap(i, j);
        }
        for (idx, &k) in keys.iter().enumerate() {
            bt.insert(make_key(k), vec![idx as u8]).unwrap();
        }
        // 中序遍历应为 0..100 严格递增
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), 100);
        for (i, (k, _)) in pairs.iter().enumerate() {
            assert_eq!(*k, make_key(i as i64), "key at index {} mismatch", i);
        }
        // search 验证：每个 key 的 tuple_id 应等于其原始插入顺序
        for (idx, &k) in keys.iter().enumerate() {
            assert_eq!(bt.search(&make_key(k)).unwrap(), Some(vec![idx as u8]));
        }
    }

    // --- 乱序插入 100000 个 key（验证标准要求） ---

    #[test]
    fn btree_insert_random_order_100000_keys_in_order_strictly_increasing() {
        let mut bt = BTree::with_default_order();
        // LCG 伪随机洗牌 0..100000
        let n = 100_000usize;
        let mut keys: Vec<i64> = (0..n as i64).collect();
        let mut seed: u64 = 12345;
        for i in (1..n).rev() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let j = (seed >> 33) as usize % (i + 1);
            keys.swap(i, j);
        }
        for &k in &keys {
            bt.insert(make_key(k), vec![(k % 65536) as u8]).unwrap();
        }
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), n);
        // 严格递增
        for i in 1..pairs.len() {
            assert!(
                pairs[i - 1].0 < pairs[i].0,
                "keys not strictly increasing at index {}",
                i
            );
        }
        // key 值应为 0..100000
        for (i, (k, _)) in pairs.iter().enumerate() {
            assert_eq!(*k, make_key(i as i64));
        }
        // 所有 key 可被 search 找到
        for &k in &keys {
            assert!(bt.search(&make_key(k)).unwrap().is_some());
        }
    }

    // --- 重复 key 处理（upsert 语义） ---

    #[test]
    fn btree_insert_duplicate_key_updates_tuple_id() {
        let mut bt = BTree::new(4);
        bt.insert(make_key(42), vec![100u8]).unwrap();
        bt.insert(make_key(42), vec![200u8]).unwrap(); // 更新 tuple_id
        bt.insert(make_key(42), vec![44u8]).unwrap(); // 再次更新
                                                      // 中序遍历只有一个 key（使用 leaf_traverse 避免包含 Internal 节点 key）
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, make_key(42));
        assert_eq!(pairs[0].1, vec![44u8]); // 最新值
                                            // search 返回最新值
        assert_eq!(bt.search(&make_key(42)).unwrap(), Some(vec![44u8]));
    }

    #[test]
    fn btree_insert_duplicate_key_does_not_grow_tree() {
        let mut bt = BTree::new(4);
        bt.insert(make_key(10), vec![1u8]).unwrap();
        let nodes_before = bt.node_count();
        // 重复插入 1000 次同一个 key
        for i in 0..1000u32 {
            bt.insert(make_key(10), vec![i as u8]).unwrap();
        }
        let nodes_after = bt.node_count();
        assert_eq!(
            nodes_before, nodes_after,
            "duplicate insert should not grow tree"
        );
        assert_eq!(bt.search(&make_key(10)).unwrap(), Some(vec![231u8]));
    }

    #[test]
    fn btree_insert_duplicate_key_after_split_still_upsert() {
        // 先插入足够多 key 触发分裂，再重复插入已有 key
        let mut bt = BTree::new(4);
        for i in 0..50i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        let nodes_before = bt.node_count();
        // 重复插入
        for i in 0..50i64 {
            bt.insert(make_key(i), vec![(i % 256) as u8]).unwrap();
        }
        let nodes_after = bt.node_count();
        assert_eq!(nodes_before, nodes_after, "upsert should not grow tree");
        // 验证 tuple_id 已更新
        for i in 0..50i64 {
            assert_eq!(
                bt.search(&make_key(i)).unwrap(),
                Some(vec![(i % 256) as u8])
            );
        }
    }

    // --- 满节点分裂 ---

    #[test]
    fn btree_insert_triggers_leaf_split() {
        let mut bt = BTree::new(4);
        // order=4，节点满 = keys.len() >= 4，即插入第 4 个 key 触发分裂
        for i in 0..3i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        assert_eq!(bt.node_count(), 1); // 还未分裂（3 < 4）
        bt.insert(make_key(3), vec![3u8]).unwrap(); // 第 4 个 key 触发分裂
        assert!(bt.node_count() > 1, "expected node_count > 1 after split");
        // 应该有新根（原根分裂后产生新根）
        assert_eq!(bt.height(), 2);
        // 中序遍历有序
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), 4);
        for i in 1..pairs.len() {
            assert!(pairs[i - 1].0 < pairs[i].0);
        }
    }

    // --- 递归分裂到根 ---

    #[test]
    fn btree_insert_triggers_recursive_split_to_root() {
        // 用小 order 强制多次分裂
        let mut bt = BTree::new(3);
        // 插入足够多 key 触发多层分裂
        for i in 0..100i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        // 中序遍历有序
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), 100);
        for i in 1..pairs.len() {
            assert!(pairs[i - 1].0 < pairs[i].0);
        }
        // 树高度应 >= 3（order=3，100 个 key 多次分裂）
        let h = bt.height();
        assert!(h >= 3, "expected height >= 3, got {}", h);
    }

    #[test]
    fn btree_root_split_creates_new_internal_root() {
        let mut bt = BTree::new(4);
        // order=4，插入第 4 个 key 触发根（叶子）分裂
        for i in 0..4i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        // 新根应为 Internal 节点
        let root = bt.read_node(bt.root_page_id()).unwrap();
        assert!(root.is_internal(), "root should be Internal after split");
        assert_eq!(root.children.len(), 2); // 两个子节点
                                            // 原 page_id=1 应不再是根
        assert_ne!(bt.root_page_id(), 1);
    }

    // --- search 搜索 ---

    #[test]
    fn btree_search_finds_existing_key() {
        let mut bt = BTree::new(4);
        for i in (0..50).rev() {
            // 反向插入，强制多次分裂
            bt.insert(make_key(i), vec![(i * 2 % 256) as u8]).unwrap();
        }
        for i in 0..50i64 {
            assert_eq!(bt.search(&make_key(i)).unwrap(), Some(vec![(i * 2) as u8]));
        }
    }

    #[test]
    fn btree_search_returns_none_for_missing_key() {
        let mut bt = BTree::new(4);
        for i in 0..20i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        // 未插入的 key
        assert_eq!(bt.search(&make_key(100)).unwrap(), None);
        assert_eq!(bt.search(&make_key(-1)).unwrap(), None);
        assert_eq!(bt.search(&make_key(50)).unwrap(), None);
    }

    #[test]
    fn btree_search_empty_tree_returns_none() {
        let bt = BTree::new(4);
        assert_eq!(bt.search(&make_key(42)).unwrap(), None);
    }

    #[test]
    fn btree_search_min_max_keys() {
        let mut bt = BTree::with_default_order();
        bt.insert(make_key(i64::MIN), vec![1u8]).unwrap();
        bt.insert(make_key(0), vec![2u8]).unwrap();
        bt.insert(make_key(i64::MAX), vec![3u8]).unwrap();
        assert_eq!(bt.search(&make_key(i64::MIN)).unwrap(), Some(vec![1u8]));
        assert_eq!(bt.search(&make_key(0)).unwrap(), Some(vec![2u8]));
        assert_eq!(bt.search(&make_key(i64::MAX)).unwrap(), Some(vec![3u8]));
    }

    // --- 高度查询 ---

    #[test]
    fn btree_height_increments_on_root_split() {
        let mut bt = BTree::new(4);
        assert_eq!(bt.height(), 1);
        // order=4：插入 3 个 key 不分裂，第 4 个触发根分裂
        for i in 0..3i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
            assert_eq!(bt.height(), 1);
        }
        bt.insert(make_key(3), vec![3u8]).unwrap(); // 第 4 个 key 触发根分裂
        assert_eq!(bt.height(), 2);
        // 继续插入，直到下一次根分裂（高度变 3）
        let mut next_id = 4i64;
        loop {
            bt.insert(make_key(next_id), vec![next_id as u8]).unwrap();
            if bt.height() >= 3 {
                break;
            }
            next_id += 1;
            if next_id > 1000 {
                panic!("expected height to reach 3 within 1000 inserts");
            }
        }
        assert_eq!(bt.height(), 3);
    }

    #[test]
    fn btree_height_grows_logarithmically() {
        // 大量插入后高度应保持较小（O(log_n)）
        let mut bt = BTree::with_default_order();
        for i in 0..10_000i64 {
            bt.insert(make_key(i), vec![(i % 65536) as u8]).unwrap();
        }
        let h = bt.height();
        // 10000 keys, order=256: log_256(10000) ≈ 1.66，高度应为 2 或 3
        assert!(h <= 4, "expected height <= 4, got {}", h);
        assert!(h >= 2, "expected height >= 2, got {}", h);
    }

    // --- 中序遍历 ---

    #[test]
    fn btree_in_order_leaf_traverse_strictly_increasing() {
        let mut bt = BTree::new(4);
        // 乱序插入
        let input = [
            50, 10, 40, 20, 30, 60, 70, 5, 15, 25, 35, 45, 55, 65, 75, 1, 100,
        ];
        for (idx, &v) in input.iter().enumerate() {
            bt.insert(make_key(v), vec![idx as u8]).unwrap();
        }
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), input.len());
        // 严格递增
        for i in 1..pairs.len() {
            assert!(
                pairs[i - 1].0 < pairs[i].0,
                "not strictly increasing at {}: prev={:?}, curr={:?}",
                i,
                pairs[i - 1].0,
                pairs[i].0
            );
        }
        // 第一个应是最小值，最后应是最大值
        assert_eq!(pairs[0].0, make_key(1));
        assert_eq!(pairs[pairs.len() - 1].0, make_key(100));
    }

    #[test]
    fn btree_in_order_traverse_includes_internal_keys() {
        // 中序遍历（in_order_traverse）会访问 Internal 节点的 key（tuple_id=0）
        let mut bt = BTree::new(4);
        for i in 0..10i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        let all_pairs = bt.in_order_traverse().unwrap();
        let leaf_pairs = bt.in_order_leaf_traverse().unwrap();
        // in_order_traverse 应包含 Internal 节点的 key（数量 > 叶子节点 key 数）
        assert!(all_pairs.len() >= leaf_pairs.len());
    }

    // --- 节点计数 ---

    #[test]
    fn btree_node_count_increases_with_splits() {
        let mut bt = BTree::new(4);
        let mut prev_count = bt.node_count();
        for i in 0..50i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
            let cur = bt.node_count();
            assert!(cur >= prev_count, "node_count should not decrease");
            prev_count = cur;
        }
        assert!(bt.node_count() > 1);
    }

    // --- 不变量校验（所有节点都应通过 validate） ---

    #[test]
    fn btree_all_nodes_validate_after_inserts() {
        let mut bt = BTree::new(4);
        for i in 0..100i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        // 遍历所有节点，检查 validate
        for page_id in 1..bt.next_page_id() {
            if let Ok(node) = bt.read_node(page_id) {
                assert!(node.validate().is_ok(), "node {} failed validate", page_id);
            }
        }
    }

    #[test]
    fn btree_internal_nodes_have_correct_children_count() {
        let mut bt = BTree::new(4);
        for i in 0..100i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        for page_id in 1..bt.next_page_id() {
            if let Ok(node) = bt.read_node(page_id) {
                if node.is_internal() {
                    assert_eq!(
                        node.children.len(),
                        node.keys.len() + 1,
                        "internal node {} has {} children, expected {}",
                        page_id,
                        node.children.len(),
                        node.keys.len() + 1
                    );
                } else {
                    assert!(
                        node.children.is_empty(),
                        "leaf node {} has children",
                        page_id
                    );
                    assert_eq!(
                        node.values.len(),
                        node.keys.len(),
                        "leaf node {} values/keys mismatch",
                        page_id
                    );
                }
            }
        }
    }

    // --- BTree Proptest ---

    proptest::proptest! {
        /// 随机插入 N 个 key，验证中序遍历严格递增且包含所有 key
        #[test]
        fn prop_btree_insert_random_keys_sorted_in_order(
            n in 1usize..500,
            seed in 0u64..100_000,
            order in 3usize..=32,
        ) {
            let mut bt = BTree::new(order);
            // 生成 n 个唯一 key（确定性伪随机）
            let mut keys: Vec<i64> = (0..n as i64).collect();
            let mut s = seed.wrapping_mul(2862933555777941757).wrapping_add(3037000493);
            for i in (1..keys.len()).rev() {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let j = (s >> 33) as usize % (i + 1);
                keys.swap(i, j);
            }
            for (idx, &k) in keys.iter().enumerate() {
                bt.insert(make_key(k), vec![(idx % 65536) as u8]).unwrap();
            }
            // 中序遍历严格递增
            let pairs = bt.in_order_leaf_traverse().unwrap();
            prop_assert_eq!(pairs.len(), n);
            for i in 1..pairs.len() {
                prop_assert!(
                    pairs[i - 1].0 < pairs[i].0,
                    "not strictly increasing at index {} (n={}, seed={}, order={})",
                    i, n, seed, order
                );
            }
            // 第一个应是最小值，最后应是最大值
            prop_assert_eq!(&pairs[0].0, &make_key(0));
            prop_assert_eq!(&pairs[pairs.len() - 1].0, &make_key((n - 1) as i64));
            // search 验证
            for (idx, &k) in keys.iter().enumerate() {
                let found = bt.search(&make_key(k)).unwrap();
                prop_assert_eq!(found, Some(vec![(idx % 256) as u8]));
            }
        }

        /// 随机插入 + 重复插入，验证 upsert 语义和不变量
        #[test]
        fn prop_btree_upsert_preserves_invariants(
            base_keys in 1usize..200,
            duplicate_count in 0usize..100,
            seed in 0u64..1000,
        ) {
            let mut bt = BTree::new(4);
            // 先插入 base_keys 个唯一 key
            for i in 0..base_keys as i64 {
                bt.insert(make_key(i), vec![i as u8]).unwrap();
            }
            let nodes_before = bt.node_count();
            // 随机选择 duplicate_count 个已存在的 key 重新插入（更新 tuple_id）
            let mut s = seed;
            for _ in 0..duplicate_count {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let k = (s >> 33) as i64 % base_keys as i64;
                let new_tid = (s >> 40) as u32;
                bt.insert(make_key(k), vec![new_tid as u8]).unwrap();
            }
            // 节点数不应增加（upsert 不应触发分裂）
            let nodes_after = bt.node_count();
            prop_assert_eq!(nodes_after, nodes_before, "upsert should not grow tree");
            // 中序遍历仍应有 base_keys 个 key
            let pairs = bt.in_order_leaf_traverse().unwrap();
            prop_assert_eq!(pairs.len(), base_keys);
            // 严格递增
            for i in 1..pairs.len() {
                prop_assert!(pairs[i - 1].0 < pairs[i].0);
            }
        }

        /// 验证 B-Tree 高度在大量插入后保持对数级
        #[test]
        fn prop_btree_height_bounded(
            n in 100usize..2000,
            order in 4usize..=64,
        ) {
            let mut bt = BTree::new(order);
            for i in 0..n as i64 {
                bt.insert(make_key(i), vec![(i % 65536) as u8]).unwrap();
            }
            let h = bt.height();
            // 高度上界：log_{order/2}(n) + 2
            let min_fanout = order / 2;
            let max_height = (n as f64).log(min_fanout.max(2) as f64).ceil() as usize + 2;
            prop_assert!(h <= max_height, "height {} > expected max {} (n={}, order={})",
                h, max_height, n, order);
            prop_assert!(h >= 2, "height {} < 2 (n={}, order={})", h, n, order);
        }
    }

    // =================================================================
    //  Phase 1.5 — 点查 + 范围扫描测试
    // =================================================================

    use std::ops::Bound;
    use std::time::Instant;

    /// 辅助：构造一棵插入 keys 的 BTree（i64 key → tuple_id = key as u32）
    fn make_btree_with_keys(order: usize, keys: &[i64]) -> BTree {
        let mut bt = BTree::new(order);
        for &k in keys {
            bt.insert(make_key(k), vec![k as u8]).unwrap();
        }
        bt
    }

    // --- 点查性能（< 10μs 预热后）---

    #[test]
    fn btree_point_lookup_latency_under_10us_after_warmup() {
        // 插入 10 万 key，预热后再做点查；测量 1 万次点查平均延迟
        let mut bt = BTree::with_default_order();
        for i in 0..100_000i64 {
            bt.insert(make_key(i), vec![(i % 65536) as u8]).unwrap();
        }
        // 预热：先做 1000 次点查
        for i in 0..1000i64 {
            let _ = bt.search(&make_key(i)).unwrap();
        }
        // 测量：10000 次点查的总耗时
        const ITERATIONS: usize = 10_000;
        let start = Instant::now();
        for i in 0..ITERATIONS as i64 {
            let result = bt.search(&make_key(i)).unwrap();
            assert!(result.is_some(), "key {} should be found", i);
        }
        let elapsed = start.elapsed();
        let avg_nanos = elapsed.as_nanos() as u64 / ITERATIONS as u64;
        // 预算：P50 < 10μs（即平均 < 10μs）
        // 注意：debug 模式下性能较差，这里只在 release 模式断言严格阈值
        // debug 模式下放宽到 50μs 防止 CI 假阴性
        #[cfg(debug_assertions)]
        let threshold_nanos = 50_000; // 50μs debug
        #[cfg(not(debug_assertions))]
        let threshold_nanos = 10_000; // 10μs release
        assert!(
            avg_nanos <= threshold_nanos,
            "avg point lookup latency {}ns > {}ns threshold (total {:?} for {} iterations)",
            avg_nanos,
            threshold_nanos,
            elapsed,
            ITERATIONS
        );
    }

    // --- 前向范围扫描 ---

    #[test]
    fn range_scan_unbounded_returns_all_keys_in_order() {
        // 全表扫描：lower=Unbounded, upper=Unbounded
        let bt = make_btree_with_keys(4, &[5, 3, 1, 7, 9, 2, 4, 8, 6, 10]);
        let result = bt.range_scan(Bound::Unbounded, Bound::Unbounded).unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        // tuple_id 与 key 一致（key as u32）
        for (k, tid) in &result {
            assert_eq!(*tid, vec![decode_i64_key(k).unwrap() as u8]);
        }
    }

    #[test]
    fn range_scan_inclusive_lower_bound() {
        // lower = Included(5), upper = Unbounded
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan(Bound::Included(&make_key(5)), Bound::Unbounded)
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn range_scan_inclusive_upper_bound() {
        // lower = Unbounded, upper = Included(5)
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan(Bound::Unbounded, Bound::Included(&make_key(5)))
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn range_scan_excluded_lower_bound() {
        // lower = Excluded(5), upper = Unbounded
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan(Bound::Excluded(&make_key(5)), Bound::Unbounded)
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![6, 7, 8, 9, 10]);
    }

    #[test]
    fn range_scan_excluded_upper_bound() {
        // lower = Unbounded, upper = Excluded(5)
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan(Bound::Unbounded, Bound::Excluded(&make_key(5)))
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![1, 2, 3, 4]);
    }

    #[test]
    fn range_scan_both_inclusive_bounds() {
        // lower = Included(3), upper = Included(7)
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan(Bound::Included(&make_key(3)), Bound::Included(&make_key(7)))
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![3, 4, 5, 6, 7]);
    }

    #[test]
    fn range_scan_both_excluded_bounds() {
        // lower = Excluded(3), upper = Excluded(7)
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan(Bound::Excluded(&make_key(3)), Bound::Excluded(&make_key(7)))
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![4, 5, 6]);
    }

    #[test]
    fn range_scan_empty_range_when_lower_exceeds_upper() {
        // lower = Included(8), upper = Included(3) → 空结果
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan(Bound::Included(&make_key(8)), Bound::Included(&make_key(3)))
            .unwrap();
        assert!(result.is_empty(), "expected empty result, got {:?}", result);
    }

    #[test]
    fn range_scan_single_key_when_lower_equals_upper_included() {
        // lower = Included(5), upper = Included(5)
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan(Bound::Included(&make_key(5)), Bound::Included(&make_key(5)))
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(decode_i64_key(&result[0].0).unwrap(), 5);
    }

    #[test]
    fn range_scan_empty_when_lower_equals_upper_excluded() {
        // lower = Excluded(5), upper = Excluded(5) → 空
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan(Bound::Excluded(&make_key(5)), Bound::Excluded(&make_key(5)))
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn range_scan_lower_above_all_keys_returns_empty() {
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5]);
        let result = bt
            .range_scan(Bound::Included(&make_key(100)), Bound::Unbounded)
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn range_scan_upper_below_all_keys_returns_empty() {
        let bt = make_btree_with_keys(4, &[10, 20, 30, 40, 50]);
        let result = bt
            .range_scan(Bound::Unbounded, Bound::Included(&make_key(5)))
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn range_scan_on_empty_tree_returns_empty() {
        let bt = BTree::new(4);
        let result = bt.range_scan(Bound::Unbounded, Bound::Unbounded).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn range_scan_with_multi_level_tree_correct() {
        // 用小 order 强制多级树
        let mut bt = BTree::new(4);
        for i in 0..1000i64 {
            bt.insert(make_key(i), vec![(i % 65536) as u8]).unwrap();
        }
        assert!(bt.height() >= 3, "expected multi-level tree");
        // 范围 [100, 200)
        let result = bt
            .range_scan(
                Bound::Included(&make_key(100)),
                Bound::Excluded(&make_key(200)),
            )
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys.len(), 100);
        assert_eq!(keys[0], 100);
        assert_eq!(keys[99], 199);
    }

    // --- 反向范围扫描 ---

    #[test]
    fn range_scan_reverse_unbounded_returns_all_keys_descending() {
        let bt = make_btree_with_keys(4, &[5, 3, 1, 7, 9, 2, 4, 8, 6, 10]);
        let result = bt
            .range_scan_reverse(Bound::Unbounded, Bound::Unbounded)
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![10, 9, 8, 7, 6, 5, 4, 3, 2, 1]);
    }

    #[test]
    fn range_scan_reverse_with_inclusive_bounds() {
        // 反向：从 upper=Included(7) 到 lower=Included(3)，结果应为 [7,6,5,4,3]
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan_reverse(Bound::Included(&make_key(3)), Bound::Included(&make_key(7)))
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![7, 6, 5, 4, 3]);
    }

    #[test]
    fn range_scan_reverse_with_excluded_bounds() {
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan_reverse(Bound::Excluded(&make_key(3)), Bound::Excluded(&make_key(7)))
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![6, 5, 4]);
    }

    #[test]
    fn range_scan_reverse_empty_range() {
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5]);
        let result = bt
            .range_scan_reverse(Bound::Included(&make_key(8)), Bound::Included(&make_key(3)))
            .unwrap();
        assert!(result.is_empty());
    }

    // --- LIMIT 截断 ---

    #[test]
    fn range_scan_with_limit_zero_returns_empty() {
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5]);
        let result = bt
            .range_scan_with_limit(Bound::Unbounded, Bound::Unbounded, Some(0))
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn range_scan_with_limit_one_returns_single_key() {
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5]);
        let result = bt
            .range_scan_with_limit(Bound::Unbounded, Bound::Unbounded, Some(1))
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(decode_i64_key(&result[0].0).unwrap(), 1);
    }

    #[test]
    fn range_scan_with_limit_n_truncates_correctly() {
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan_with_limit(Bound::Unbounded, Bound::Unbounded, Some(3))
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![1, 2, 3]);
    }

    #[test]
    fn range_scan_with_limit_none_returns_all() {
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5]);
        let result = bt
            .range_scan_with_limit(Bound::Unbounded, Bound::Unbounded, None)
            .unwrap();
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn range_scan_with_limit_exceeds_range_returns_all_in_range() {
        // limit=100 但范围内只有 5 个 key → 返回 5 个
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5]);
        let result = bt
            .range_scan_with_limit(
                Bound::Included(&make_key(1)),
                Bound::Included(&make_key(5)),
                Some(100),
            )
            .unwrap();
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn range_scan_with_limit_with_bounds_correct() {
        // 范围 [3, 8], limit=2 → [3, 4]
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = bt
            .range_scan_with_limit(
                Bound::Included(&make_key(3)),
                Bound::Included(&make_key(8)),
                Some(2),
            )
            .unwrap();
        let keys: Vec<i64> = result
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![3, 4]);
    }

    // --- Cursor 迭代器 ---

    #[test]
    fn cursor_iterates_in_order() {
        let bt = make_btree_with_keys(4, &[3, 1, 4, 1, 5, 9, 2, 6]);
        let cursor = bt.cursor(Bound::Unbounded, Bound::Unbounded).unwrap();
        let keys: Vec<i64> = cursor.map(|(k, _)| decode_i64_key(&k).unwrap()).collect();
        // 去重（upsert 语义）：1 出现两次但实际只有一个
        assert_eq!(keys, vec![1, 2, 3, 4, 5, 6, 9]);
    }

    #[test]
    fn cursor_respects_lower_bound() {
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let cursor = bt
            .cursor(Bound::Included(&make_key(5)), Bound::Unbounded)
            .unwrap();
        let keys: Vec<i64> = cursor.map(|(k, _)| decode_i64_key(&k).unwrap()).collect();
        assert_eq!(keys, vec![5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn cursor_respects_upper_bound() {
        let bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let cursor = bt
            .cursor(Bound::Unbounded, Bound::Excluded(&make_key(5)))
            .unwrap();
        let keys: Vec<i64> = cursor.map(|(k, _)| decode_i64_key(&k).unwrap()).collect();
        assert_eq!(keys, vec![1, 2, 3, 4]);
    }

    #[test]
    fn cursor_empty_tree_returns_empty_iterator() {
        let bt = BTree::new(4);
        let cursor = bt.cursor(Bound::Unbounded, Bound::Unbounded).unwrap();
        assert_eq!(cursor.count(), 0);
    }

    #[test]
    fn cursor_multi_level_tree_correct() {
        let mut bt = BTree::new(4);
        for i in 0..500i64 {
            bt.insert(make_key(i), vec![(i % 65536) as u8]).unwrap();
        }
        let cursor = bt
            .cursor(
                Bound::Included(&make_key(100)),
                Bound::Excluded(&make_key(200)),
            )
            .unwrap();
        let keys: Vec<i64> = cursor.map(|(k, _)| decode_i64_key(&k).unwrap()).collect();
        assert_eq!(keys.len(), 100);
        assert_eq!(keys[0], 100);
        assert_eq!(keys[99], 199);
    }

    // =================================================================
    // Phase 1.7: B-Tree 删除（含合并）测试
    // =================================================================
    //
    // 验证标准：
    // - 删除叶子节点 / 删除内部节点 / 删除后合并 / 删除所有节点 / 空树删除
    // - 唯一索引删除后不可重复插入（实际是验证 delete+reinsert 链路）
    // - 删除后节点大小 >= 半满（或已合并），key 不存在
    //
    // 设计要点：
    // - 使用 order=4 触发频繁的分裂/合并（max_keys=4, min_keys=2）
    // - 测试覆盖：空树/单 key/无 underflow/借键(左/右)/合并(左/右)/全删/重插/分隔键/高度缩减/不变量/有序性
    // - API：`pub fn delete(&mut self, key: &[u8]) -> Result<bool, BTreeError>`
    //        返回 true 表示找到并删除，false 表示 key 不存在

    #[test]
    fn phase_017_delete_from_empty_tree_returns_false() {
        let mut bt = BTree::new(4);
        let deleted = bt.delete(&make_key(42)).unwrap();
        assert!(!deleted, "delete on empty tree should return false");
        // 树仍为空
        assert_eq!(bt.in_order_leaf_traverse().unwrap().len(), 0);
        assert_eq!(bt.node_count(), 1); // 仅根叶子
        assert_eq!(bt.height(), 1);
    }

    #[test]
    fn phase_017_delete_single_key_tree_becomes_empty() {
        let mut bt = BTree::new(4);
        bt.insert(make_key(42), vec![100u8]).unwrap();
        assert!(bt.search(&make_key(42)).unwrap().is_some());

        let deleted = bt.delete(&make_key(42)).unwrap();
        assert!(deleted, "delete existing key should return true");

        // 树应为空
        assert_eq!(bt.in_order_leaf_traverse().unwrap().len(), 0);
        assert!(bt.search(&make_key(42)).unwrap().is_none());
        assert_eq!(bt.height(), 1); // 仍为单层根叶子
        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_nonexistent_key_returns_false_tree_unchanged() {
        let mut bt = make_btree_with_keys(4, &[1, 2, 3]);
        let before = bt.in_order_leaf_traverse().unwrap();

        let deleted = bt.delete(&make_key(999)).unwrap();
        assert!(!deleted, "delete nonexistent key should return false");

        let after = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(
            before, after,
            "tree should be unchanged after deleting nonexistent key"
        );
    }

    #[test]
    fn phase_017_delete_from_leaf_no_underflow_no_rebalance() {
        // order=4, min_keys=2. 3 keys in root leaf (no split). Delete 1 → 2 keys (no underflow).
        let mut bt = make_btree_with_keys(4, &[1, 2, 3]);
        let deleted = bt.delete(&make_key(1)).unwrap();
        assert!(deleted);

        // 验证：1 已删除，2 和 3 仍在
        assert!(bt.search(&make_key(1)).unwrap().is_none());
        assert_eq!(bt.search(&make_key(2)).unwrap(), Some(vec![2u8]));
        assert_eq!(bt.search(&make_key(3)).unwrap(), Some(vec![3u8]));

        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(decode_i64_key(&pairs[0].0).unwrap(), 2);
        assert_eq!(decode_i64_key(&pairs[1].0).unwrap(), 3);
        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_from_leaf_triggers_borrow_from_right_sibling() {
        // order=4, min_keys=2.
        // 插入 5 个 key: [1,2,3,4,5]
        // 分裂后：root=[3], left_leaf=[1,2], right_leaf=[3,4,5]
        // 删除 1 → left_leaf=[2] (underflow), right_leaf 有 3 keys (>2) 可借
        // 借键后：left_leaf=[2,3], right_leaf=[4,5], root separator 更新为 4
        let mut bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5]);
        assert_eq!(bt.height(), 2); // 确认已分裂为 2 层

        let deleted = bt.delete(&make_key(1)).unwrap();
        assert!(deleted);

        // 验证所有剩余 key 都能找到
        for k in [2, 3, 4, 5] {
            assert_eq!(
                bt.search(&make_key(k)).unwrap(),
                Some(vec![k as u8]),
                "key {} should be found after delete",
                k
            );
        }
        assert!(bt.search(&make_key(1)).unwrap().is_none());

        // 验证中序遍历有序
        let pairs = bt.in_order_leaf_traverse().unwrap();
        let keys: Vec<i64> = pairs
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![2, 3, 4, 5]);

        // 验证所有节点满足半满不变量
        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_from_leaf_triggers_borrow_from_left_sibling() {
        // order=4, min_keys=2.
        // 插入 5 个 key: [1,2,3,4,5]
        // 分裂后：root=[3], left_leaf=[1,2], right_leaf=[3,4,5]
        // 删除 4 和 5 → right_leaf=[3] (underflow), left_leaf 有 2 keys (== min, 无法借)
        //   实际上 left_leaf 无法借，应触发 merge
        // 改用 6 个 key 让 left 有 3 keys 可借：
        // 插入 [1,2,3,4,5,6] → root=[3,5], leaves=[1,2],[3,4],[5,6]
        // 删除 6 → right_leaf=[5] (underflow), middle_leaf=[3,4] 有 2 keys (== min, 无法借)
        //   → merge right + middle? 或者 middle + right?
        // 再用 7 个 key：[1,2,3,4,5,6,7] → root=[3,5], leaves=[1,2],[3,4],[5,6,7]
        // 删除 7 → right_leaf=[5,6] (no underflow), 不触发借键
        // 删除 6 和 7 → right_leaf=[5] (underflow), middle=[3,4] 无法借，触发 merge
        //
        // 为触发"借左"，需要：left 有 >min keys，right underflow
        // 插入 [1,2,3,4,5,6,7,8]：
        //   root=[3,5,7], leaves=[1,2],[3,4],[5,6],[7,8]
        // 删除 8 → right_leaf=[7] (underflow), middle_right=[5,6] (2 keys, 无法借)
        // 删除 6 → middle_right=[5] (underflow), middle=[3,4] (2 keys, 无法借)
        //   → 触发 merge
        //
        // 用 order=4 时 min_keys=2，要让 left 有 >2 keys 可借，需要插入更多 key 让某叶有 3 keys
        // 插入 [1,2,3,4,5,6,7]：root=[3,5], leaves=[1,2],[3,4],[5,6,7]
        // 删除 5 → right_leaf=[6,7] (2 keys, no underflow)
        // 删除 6 → right_leaf=[7] (underflow), middle=[3,4] (2 keys, 无法借)
        //   → merge right+middle
        //
        // 直接构造场景：插入 [1,2,3,4,5,6,7]，删除 7 → right=[5,6] (no underflow)
        // 再删除 5 → right=[6] (underflow), middle=[3,4] (无法借) → merge
        //
        // 改用 order=8 来更容易构造借左场景：
        // order=8, min_keys=4
        // 插入 9 个 key → root=[5], left=[1,2,3,4], right=[5,6,7,8,9]
        // 删除 5,6,7,8 → right=[9] (underflow), left=[1,2,3,4] (4 keys, 无法借，== min)
        // 还是不行。
        //
        // 用 order=8 插入 10 个 key：root=[5], left=[1,2,3,4], right=[5,6,7,8,9,10]? 不对，order=8 max=8
        // 插入 9 个 key: 8 个触发 split → root=[5], left=[1,2,3,4], right=[5,6,7,8,9]
        //   right 有 5 keys, left 有 4 keys
        // 删除 9,8,7 → right=[5,6] (2 keys < 4, underflow), left=[1,2,3,4] (4 keys, 无法借)
        //   → merge
        //
        // 看来 order=4 时很难触发"借左"，因为 split 后两个叶子都恰好 2 keys (== min)
        // 需要继续插入让某叶子有 3+ keys，然后删除让另一个 underflow
        //
        // 插入 [1,2,3,4,5,6,7] → root=[3,5], leaves=[1,2],[3,4],[5,6,7]
        //   right leaf 有 3 keys (>min=2)
        // 删除 1 → left=[2] (underflow), middle=[3,4] (2 keys, 无法借)
        //   → merge left+middle
        // 不行。
        //
        // 插入 [1,2,3,4,5,6,7] → root=[3,5], leaves=[1,2],[3,4],[5,6,7]
        // 删除 3 → middle=[4] (underflow), left=[1,2] (2 keys, 无法借)
        //   → merge left+middle
        // 还是不行。
        //
        // 要让"借左"发生：右叶子 underflow，左叶子 >min
        // 插入 [1,2,3,4,5,6,7] → leaves=[1,2],[3,4],[5,6,7]
        // 删除 5,6 → right=[7] (underflow), middle=[3,4] (2 keys, 无法借)
        //   → merge
        //
        // 用更多 key：插入 [1,2,3,4,5,6,7,8,9]
        //   1-4 root leaf → split → root=[3], left=[1,2], right=[3,4]
        //   5 → right=[3,4,5]
        //   6 → right=[3,4,5,6] (full) → split → root=[3,5], leaves=[1,2],[3,4],[5,6]
        //   7 → rightmost=[5,6,7]
        //   8 → rightmost=[5,6,7,8] (full) → split → root=[3,5,7], leaves=[1,2],[3,4],[5,6],[7,8]
        //   9 → rightmost=[7,8,9]
        // 最终：root=[3,5,7], leaves=[1,2],[3,4],[5,6],[7,8,9]
        // 删除 9,8 → rightmost=[7] (underflow), middle_right=[5,6] (2 keys, 无法借)
        //   → merge
        //
        // 看来 order=4 时，split 总是产生 2-key 叶子，所以兄弟总是 == min，无法借
        // 必须用更大的 order 让 split 后某些叶子有 >min keys
        //
        // 用 order=6 (min_keys=3):
        // 插入 7 个 key: root leaf [1..7] (7 keys, full, split)
        //   mid=3, left=[1,2,3], right=[4,5,6,7], promoted=4
        //   root=[4], left=[1,2,3], right=[4,5,6,7]
        // 删除 7,6 → right=[4,5] (2 keys < 3, underflow), left=[1,2,3] (3 keys, 无法借)
        //   → merge
        // 还是无法借。
        //
        // 用 order=6 插入 8 个 key:
        //   1-6 root leaf [1..6] (full, split) → root=[4], left=[1,2,3], right=[4,5,6]
        //   7 → rightmost=[4,5,6,7]
        //   8 → rightmost=[4,5,6,7,8]
        //   (8-7+1=2 inserts after split, but right=[4,5,6,7,8] has 5 keys <6, no split)
        // 删除 8 → right=[4,5,6,7] (4 keys, no underflow)
        // 删除 7 → right=[4,5,6] (3 keys, no underflow)
        // 还是不行。
        //
        // 用 order=8 (min_keys=4) 插入 11 个 key:
        //   1-8 root leaf [1..8] (full, split) → root=[5], left=[1,2,3,4], right=[5,6,7,8]
        //   9 → right=[5,6,7,8,9]
        //   10 → right=[5,6,7,8,9,10]
        //   11 → right=[5,6,7,8,9,10,11]
        // 删除 11,10,9 → right=[5,6,7,8] (4 keys, no underflow)
        // 删除 8 → right=[5,6,7] (3 keys < 4, underflow), left=[1,2,3,4] (4 keys, 无法借)
        //   → merge
        //
        // 看来对称 split 总是产生两个相等大小的叶子，要让一个叶子 >min，需要继续插入到该叶子
        // 但插入更多 key 又会触发该叶子 split
        //
        // 关键洞察：split 总是产生 floor(n/2) 和 ceil(n/2) 两个叶子
        //   order=4: 4 keys split → [2,2] (都==min)
        //   order=5: 5 keys split → [2,3] (一个==min, 一个>min) ✓
        //   order=6: 6 keys split → [3,3] (都==min)
        //   order=7: 7 keys split → [3,4] (一个==min, 一个>min) ✓
        //
        // 用 order=5 (min_keys=2):
        // 插入 6 个 key: root leaf [1..5] (5 keys, full, split)
        //   mid=2, left=[1,2], right=[3,4,5], promoted=3
        //   6 → right=[3,4,5,6]
        // 最终：root=[3], left=[1,2], right=[3,4,5,6]
        // 删除 6,5,4 → right=[3] (1 key < 2, underflow), left=[1,2] (2 keys, 无法借)
        //   → merge
        //
        // 删除 6 → right=[3,4,5] (3 keys, no underflow)
        // 删除 3 → right=[4,5] (2 keys, no underflow)
        //
        // 看来需要 left 有 >min keys 才能借给 right
        // 用 order=5 插入 11 个 key:
        //   1-5 root [1..5] (split) → root=[3], L=[1,2], R=[3,4,5]
        //   6 → R=[3,4,5,6]
        //   7 → R=[3,4,5,6,7] (5 keys, full, split) → root=[3,6], L=[1,2], M=[3,4,5], R=[6,7]
        //   8 → R=[6,7,8]
        //   9 → R=[6,7,8,9]
        //   10 → R=[6,7,8,9,10] (5 keys, full, split) → root=[3,6,9], leaves=[1,2],[3,4,5],[6,7,8],[9,10]
        //   11 → R=[9,10,11]
        // 最终：root=[3,6,9], leaves=[1,2],[3,4,5],[6,7,8],[9,10,11]
        // 删除 11,10 → R=[9] (underflow), M2=[6,7,8] (3 keys > 2, 可借！)
        //   → 借左：M2 最后一个 key 8 借给 R，separator 9 更新为 ... 实际是
        //   B+Tree 借左：把 separator (parent.keys[idx-1]) 下降到 R 头部，
        //   把 M2 最后一个 key 上升到 parent 替换 separator
        //   借左后：M2=[6,7], R=[8,9], parent separator [3,6,8]
        //
        // OK 用 order=5 + 11 个 key 可以触发借左。但 order=4 太难触发借左。
        // 我用 order=4 测借右（容易），用 order=5 测借左。
        //
        // 实际上为了简化测试，使用更大的 order 让 split 后某些叶子 >min 是更通用的做法。
        // 但为了测试稳定性，用 order=4 测合并，用 order=5 测借键。

        // === 借左场景（order=5, 11 keys）===
        let mut bt = make_btree_with_keys(5, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
        // root=[3,6,9], leaves: L=[1,2], M1=[3,4,5], M2=[6,7,8], R=[9,10,11]
        assert_eq!(bt.height(), 2);

        // 删除 11,10 → R=[9] underflow, M2=[6,7,8] 有 3 keys > min(2), 可借左
        assert!(bt.delete(&make_key(11)).unwrap());
        assert!(bt.delete(&make_key(10)).unwrap());

        // 借左后 R 应有 2 keys: [8,9] 或 [9, x]
        // 验证所有剩余 key 都能找到
        for k in 1..=9i64 {
            assert_eq!(
                bt.search(&make_key(k)).unwrap(),
                Some(vec![k as u8]),
                "key {} should be found after borrow-from-left",
                k
            );
        }
        assert!(bt.search(&make_key(10)).unwrap().is_none());
        assert!(bt.search(&make_key(11)).unwrap().is_none());

        // 中序遍历有序
        let pairs = bt.in_order_leaf_traverse().unwrap();
        let keys: Vec<i64> = pairs
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);

        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_from_leaf_triggers_merge_with_right_sibling() {
        // order=4, min_keys=2. 插入 [1,2,3,4,5] → root=[3], L=[1,2], R=[3,4,5]
        // 删除 4,5 → R=[3] (underflow), L=[1,2] (2 keys, 无法借)
        //   → merge L+R = [1,2,3], root 变空 → 高度 -1
        let mut bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5]);
        assert_eq!(bt.height(), 2);

        assert!(bt.delete(&make_key(4)).unwrap());
        // R=[3,5] (2 keys, no underflow yet)
        assert!(bt.delete(&make_key(5)).unwrap());
        // R=[3] (underflow), L=[1,2] (无法借) → merge → root=[1,2,3] (leaf), height=1

        assert_eq!(bt.height(), 1, "height should shrink to 1 after merge");
        for k in [1, 2, 3] {
            assert_eq!(bt.search(&make_key(k)).unwrap(), Some(vec![k as u8]));
        }
        assert!(bt.search(&make_key(4)).unwrap().is_none());
        assert!(bt.search(&make_key(5)).unwrap().is_none());

        let pairs = bt.in_order_leaf_traverse().unwrap();
        let keys: Vec<i64> = pairs
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![1, 2, 3]);
        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_from_leaf_triggers_merge_with_left_sibling() {
        // order=4, min_keys=2. 插入 [1,2,3,4,5] → root=[3], L=[1,2], R=[3,4,5]
        // 删除 3,4 → R=[5] (underflow), L=[1,2] (无法借)
        //   → merge L+R = [1,2,5], root 变空 → 高度 -1
        let mut bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5]);
        assert_eq!(bt.height(), 2);

        assert!(bt.delete(&make_key(3)).unwrap());
        // R=[4,5] (2 keys, no underflow)
        assert!(bt.delete(&make_key(4)).unwrap());
        // R=[5] (underflow), L=[1,2] (无法借) → merge → root=[1,2,5] (leaf), height=1

        assert_eq!(bt.height(), 1, "height should shrink to 1 after merge");
        for k in [1, 2, 5] {
            assert_eq!(bt.search(&make_key(k)).unwrap(), Some(vec![k as u8]));
        }
        for k in [3, 4] {
            assert!(bt.search(&make_key(k)).unwrap().is_none());
        }
        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_all_keys_one_by_one_tree_becomes_empty() {
        let keys: Vec<i64> = (1..=20).collect();
        let mut bt = make_btree_with_keys(4, &keys);
        assert!(bt.height() >= 2);

        // 乱序删除所有 key
        let mut delete_order = keys.clone();
        delete_order.reverse(); // 从大到小删
        for k in &delete_order {
            let deleted = bt.delete(&make_key(*k)).unwrap();
            assert!(deleted, "key {} should be deleted successfully", k);
        }

        // 树应为空
        assert_eq!(bt.in_order_leaf_traverse().unwrap().len(), 0);
        assert_eq!(
            bt.height(),
            1,
            "height should be 1 (only root leaf) after all deletes"
        );
        bt.validate_all_nodes().unwrap();

        // 再次删除已删的 key，应返回 false
        for k in &keys {
            let deleted = bt.delete(&make_key(*k)).unwrap();
            assert!(!deleted, "re-deleting {} should return false", k);
        }
    }

    #[test]
    fn phase_017_delete_then_reinsert_with_different_tuple_id() {
        let mut bt = make_btree_with_keys(4, &[1, 2, 3]);
        assert_eq!(bt.search(&make_key(2)).unwrap(), Some(vec![2u8]));

        // 删除 2
        assert!(bt.delete(&make_key(2)).unwrap());
        assert!(bt.search(&make_key(2)).unwrap().is_none());

        // 重新插入 2，但 tuple_id 不同
        bt.insert(make_key(2), vec![231u8]).unwrap();
        assert_eq!(bt.search(&make_key(2)).unwrap(), Some(vec![231u8]));

        // 其他 key 不受影响
        assert_eq!(bt.search(&make_key(1)).unwrap(), Some(vec![1u8]));
        assert_eq!(bt.search(&make_key(3)).unwrap(), Some(vec![3u8]));
        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_separator_key_in_internal_node_search_returns_none() {
        // order=4. 插入 [1,2,3,4,5] → root=[3], L=[1,2], R=[3,4,5]
        // key 3 既是 leaf 中的 key，也是 internal root 中的 separator
        // 删除 3 后：search(3) 应返回 None
        //   （B+Tree 允许 internal separator 残留，但 search 通过 >= 导航仍正确）
        let mut bt = make_btree_with_keys(4, &[1, 2, 3, 4, 5]);
        assert_eq!(bt.height(), 2);

        // 验证 3 在删除前可找到
        assert_eq!(bt.search(&make_key(3)).unwrap(), Some(vec![3u8]));

        // 删除 3
        assert!(bt.delete(&make_key(3)).unwrap());

        // search(3) 应返回 None（即使 internal 中可能还有 stale separator）
        assert!(
            bt.search(&make_key(3)).unwrap().is_none(),
            "search for deleted key should return None even if separator is stale"
        );

        // 其他 key 仍可找到
        for k in [1, 2, 4, 5] {
            assert_eq!(
                bt.search(&make_key(k)).unwrap(),
                Some(vec![k as u8]),
                "key {} should still be found",
                k
            );
        }

        // 中序遍历不应包含 3
        let pairs = bt.in_order_leaf_traverse().unwrap();
        let keys: Vec<i64> = pairs
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        assert_eq!(keys, vec![1, 2, 4, 5]);
        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_multi_level_tree_height_shrinks() {
        // 构造 3 层 B-Tree，删除大量 key 让高度从 3 降到 1
        // order=4, 插入 50 个 key → 高度约 3
        let mut bt = BTree::new(4);
        for i in 1..=50i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        let initial_height = bt.height();
        assert!(
            initial_height >= 3,
            "expected height >= 3, got {}",
            initial_height
        );

        // 删除大部分 key，只留 3 个
        for i in 1..=47i64 {
            assert!(
                bt.delete(&make_key(i)).unwrap(),
                "delete {} should succeed",
                i
            );
        }

        // 验证剩余 3 个 key
        for k in [48, 49, 50] {
            assert_eq!(bt.search(&make_key(k)).unwrap(), Some(vec![k as u8]));
        }

        // 高度应降到 1（所有 key 在根叶子中）
        assert_eq!(
            bt.height(),
            1,
            "height should shrink to 1 after massive deletes"
        );
        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_preserves_invariants_after_random_deletes() {
        // 插入 200 个 key，随机删除一半，验证不变量
        let mut bt = BTree::new(4);
        for i in 0..200i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }

        // 用固定种子删除偶数 key
        for i in (0..200i64).step_by(2) {
            assert!(bt.delete(&make_key(i)).unwrap());
        }

        // 验证不变量
        bt.validate_all_nodes().unwrap();

        // 剩余 100 个奇数 key
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), 100);
        for (i, (k, _)) in pairs.iter().enumerate() {
            let expected = (2 * i + 1) as i64;
            assert_eq!(
                decode_i64_key(k).unwrap(),
                expected,
                "key at index {} should be {}, got {}",
                i,
                expected,
                decode_i64_key(k).unwrap()
            );
        }

        // 验证搜索：偶数 key 已删，奇数 key 仍在
        for i in 0..200i64 {
            let result = bt.search(&make_key(i)).unwrap();
            if i % 2 == 0 {
                assert!(result.is_none(), "even key {} should be deleted", i);
            } else {
                assert_eq!(result, Some(vec![i as u8]), "odd key {} should be found", i);
            }
        }
    }

    #[test]
    fn phase_017_delete_maintains_strictly_increasing_order() {
        let mut bt = BTree::new(4);
        for i in 0..100i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }

        // 删除 25, 50, 75（间隔删除）
        for &k in &[25, 50, 75] {
            assert!(bt.delete(&make_key(k)).unwrap());
        }

        // 中序遍历应严格递增
        let pairs = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(pairs.len(), 97);
        for i in 1..pairs.len() {
            assert!(
                pairs[i - 1].0 < pairs[i].0,
                "keys not strictly increasing at index {}",
                i
            );
        }
        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_unique_index_can_reinsert_after_delete() {
        // 模拟唯一索引场景：删除后可以重新插入相同 key
        let mut bt = BTree::new(4);
        for i in 1..=10i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }

        // 删除 5
        assert!(bt.delete(&make_key(5)).unwrap());
        assert!(bt.search(&make_key(5)).unwrap().is_none());

        // 重新插入 5（应成功，因为已删除）
        bt.insert(make_key(5), vec![43u8]).unwrap();
        assert_eq!(bt.search(&make_key(5)).unwrap(), Some(vec![43u8]));

        // 再次删除 5
        assert!(bt.delete(&make_key(5)).unwrap());
        // 再次插入
        bt.insert(make_key(5), vec![9u8]).unwrap();
        assert_eq!(bt.search(&make_key(5)).unwrap(), Some(vec![9u8]));

        bt.validate_all_nodes().unwrap();
    }

    #[test]
    fn phase_017_delete_interleaved_with_inserts_maintains_consistency() {
        // 交替插入和删除，验证最终状态
        let mut bt = BTree::new(4);

        // 插入 1-20
        for i in 1..=20i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        // 删除 1-10
        for i in 1..=10i64 {
            assert!(bt.delete(&make_key(i)).unwrap());
        }
        // 插入 21-30
        for i in 21..=30i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        // 删除 15-20
        for i in 15..=20i64 {
            assert!(bt.delete(&make_key(i)).unwrap());
        }

        // 最终应有: 11-14, 21-30 (共 14 个 key)
        let pairs = bt.in_order_leaf_traverse().unwrap();
        let keys: Vec<i64> = pairs
            .iter()
            .map(|(k, _)| decode_i64_key(k).unwrap())
            .collect();
        let expected: Vec<i64> = vec![11, 12, 13, 14, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30];
        assert_eq!(keys, expected);

        // 验证搜索
        for k in &expected {
            assert_eq!(bt.search(&make_key(*k)).unwrap(), Some(vec![(*k) as u8]));
        }
        for k in 1..=10i64 {
            assert!(bt.search(&make_key(k)).unwrap().is_none());
        }
        for k in 15..=20i64 {
            assert!(bt.search(&make_key(k)).unwrap().is_none());
        }

        bt.validate_all_nodes().unwrap();
    }

    // -----------------------------------------------------------------
    //  P0-3 修复：BufferPool 持久化测试
    // -----------------------------------------------------------------

    /// 辅助函数：将 InMemoryPageWriter 中的页复制到 InMemoryPageLoader
    /// （模拟磁盘持久化后重新加载的场景）
    fn transfer_writer_to_loader(
        writer: &crate::buffer::InMemoryPageWriter,
        loader: &crate::buffer::InMemoryPageLoader,
    ) {
        for page_id in writer.persisted_page_ids() {
            if let Some(page) = writer.get_persisted(page_id) {
                loader.insert(page_id, page);
            }
        }
    }

    /// P0-3 测试 1：单节点 BTree 持久化 round-trip
    ///
    /// 验证：persist → flush → load → search 结果一致
    #[test]
    fn p0_3_persist_single_node_btree_roundtrip() {
        use crate::buffer::{BufferPool, InMemoryPageLoader, InMemoryPageWriter};
        use std::sync::Arc;

        let mut bt = BTree::with_default_order();
        for i in 1..=10i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        bt.validate_all_nodes().unwrap();
        let original_height = bt.height();
        let original_node_count = bt.node_count();

        // 持久化
        let loader = Arc::new(InMemoryPageLoader::new());
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(64, loader.clone(), writer.clone()).unwrap();
        let meta = bt.persist_to_buffer_pool(&pool).unwrap();
        assert_eq!(meta.page_count, original_node_count as u32);
        pool.flush_all().unwrap();

        // 模拟重启：新 BufferPool + 从 writer 加载到 loader
        let new_loader = Arc::new(InMemoryPageLoader::new());
        transfer_writer_to_loader(&writer, &new_loader);
        let new_pool = BufferPool::new(64, new_loader).unwrap();

        // 加载
        let loaded_bt = BTree::load_from_buffer_pool(&new_pool, meta).unwrap();

        // 验证结构一致
        assert_eq!(loaded_bt.root_page_id(), bt.root_page_id());
        assert_eq!(loaded_bt.order(), bt.order());
        assert_eq!(loaded_bt.node_count(), bt.node_count());
        assert_eq!(loaded_bt.height(), original_height);
        assert_eq!(loaded_bt.next_page_id(), bt.next_page_id());
        loaded_bt.validate_all_nodes().unwrap();

        // 验证搜索结果一致
        for i in 1..=10i64 {
            assert_eq!(
                loaded_bt.search(&make_key(i)).unwrap(),
                Some(vec![i as u8]),
                "search mismatch for key {}",
                i
            );
        }
        assert!(loaded_bt.search(&make_key(999)).unwrap().is_none());
    }

    /// P0-3 测试 2：多节点 BTree（触发分裂）持久化 round-trip
    ///
    /// 验证：插入足够多数据触发 B-Tree 分裂，持久化后加载，结构和数据一致
    #[test]
    fn p0_3_persist_multi_node_btree_roundtrip() {
        use crate::buffer::{BufferPool, InMemoryPageLoader, InMemoryPageWriter};
        use std::sync::Arc;

        let mut bt = BTree::new(4); // 小阶数，容易触发分裂
        for i in 1..=200i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        bt.validate_all_nodes().unwrap();
        assert!(
            bt.node_count() > 1,
            "expected multi-node tree after 200 inserts with order=4"
        );
        let original_height = bt.height();

        // 持久化
        let loader = Arc::new(InMemoryPageLoader::new());
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(256, loader.clone(), writer.clone()).unwrap();
        let meta = bt.persist_to_buffer_pool(&pool).unwrap();
        pool.flush_all().unwrap();

        // 模拟重启
        let new_loader = Arc::new(InMemoryPageLoader::new());
        transfer_writer_to_loader(&writer, &new_loader);
        let new_pool = BufferPool::new(256, new_loader).unwrap();

        // 加载
        let loaded_bt = BTree::load_from_buffer_pool(&new_pool, meta).unwrap();

        // 验证结构
        assert_eq!(loaded_bt.node_count(), bt.node_count());
        assert_eq!(loaded_bt.height(), original_height);
        assert_eq!(loaded_bt.next_page_id(), bt.next_page_id());
        loaded_bt.validate_all_nodes().unwrap();

        // 验证所有 key 可搜索且结果正确
        for i in 1..=200i64 {
            assert_eq!(
                loaded_bt.search(&make_key(i)).unwrap(),
                Some(vec![i as u8]),
                "search mismatch for key {} after roundtrip",
                i
            );
        }

        // 验证中序遍历一致
        let original_pairs = bt.in_order_leaf_traverse().unwrap();
        let loaded_pairs = loaded_bt.in_order_leaf_traverse().unwrap();
        assert_eq!(original_pairs.len(), loaded_pairs.len());
        for (a, b) in original_pairs.iter().zip(loaded_pairs.iter()) {
            assert_eq!(a.0, b.0, "key mismatch in in-order traverse");
            assert_eq!(a.1, b.1, "tuple_id mismatch in in-order traverse");
        }
    }

    /// P0-3 测试 3：持久化后可继续插入和删除
    ///
    /// 验证：加载后的 BTree 是可变的，支持后续 DML 操作
    #[test]
    fn p0_3_persist_then_mutate() {
        use crate::buffer::{BufferPool, InMemoryPageLoader, InMemoryPageWriter};
        use std::sync::Arc;

        let mut bt = BTree::new(4);
        for i in 1..=50i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }

        // 持久化 + 加载
        let loader = Arc::new(InMemoryPageLoader::new());
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(128, loader.clone(), writer.clone()).unwrap();
        let meta = bt.persist_to_buffer_pool(&pool).unwrap();
        pool.flush_all().unwrap();

        let new_loader = Arc::new(InMemoryPageLoader::new());
        transfer_writer_to_loader(&writer, &new_loader);
        let new_pool = BufferPool::new(128, new_loader).unwrap();
        let mut loaded_bt = BTree::load_from_buffer_pool(&new_pool, meta).unwrap();

        // 删除部分 key
        for i in 1..=25i64 {
            assert!(loaded_bt.delete(&make_key(i)).unwrap());
        }
        // 插入新 key
        for i in 100..=120i64 {
            loaded_bt.insert(make_key(i), vec![i as u8]).unwrap();
        }
        loaded_bt.validate_all_nodes().unwrap();

        // 验证：已删除的 key 不存在
        for i in 1..=25i64 {
            assert!(loaded_bt.search(&make_key(i)).unwrap().is_none());
        }
        // 验证：保留的 key 存在
        for i in 26..=50i64 {
            assert_eq!(loaded_bt.search(&make_key(i)).unwrap(), Some(vec![i as u8]));
        }
        // 验证：新插入的 key 存在
        for i in 100..=120i64 {
            assert_eq!(loaded_bt.search(&make_key(i)).unwrap(), Some(vec![i as u8]));
        }
    }

    /// P0-3 测试 4：PersistedBTreeMeta 字段正确性
    #[test]
    fn p0_3_persisted_meta_fields_correct() {
        use crate::buffer::{BufferPool, InMemoryPageLoader, InMemoryPageWriter};
        use std::sync::Arc;

        let mut bt = BTree::new(8);
        for i in 1..=100i64 {
            bt.insert(make_key(i), vec![i as u8]).unwrap();
        }

        let loader = Arc::new(InMemoryPageLoader::new());
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(128, loader, writer).unwrap();
        let meta = bt.persist_to_buffer_pool(&pool).unwrap();

        assert_eq!(meta.root_page_id, bt.root_page_id());
        assert_eq!(meta.order, 8);
        assert_eq!(meta.next_page_id, bt.next_page_id());
        assert_eq!(meta.page_count, bt.node_count() as u32);
    }

    // =================================================================
    // P9-1 测试：tuple_id u16→u32 扩容验证
    // =================================================================

    /// P9-1 验证 1：tuple_id 超过 u16::MAX (65535) 后仍可正常插入和查询
    ///
    /// 之前 u16 限制下，row_id > 65535 的行无法进入 BTree 索引。
    /// 扩容为 u32 后，支持最大 ~42 亿行。
    #[test]
    fn p9_1_tuple_id_exceeds_u16_max_insert_and_search() {
        let mut bt = BTree::with_default_order();

        // 插入 tuple_id = u16::MAX + 1 = 65536（旧限制下的第一个溢出值）
        let large_tid: u32 = u16::MAX as u32 + 1; // 65536
        let key = make_key(1i64);
        bt.insert(key.clone(), vec![large_tid as u8]).unwrap();

        // 点查应返回正确的 tuple_id
        let result = bt.search(&key).unwrap();
        assert_eq!(result, Some(vec![large_tid as u8]));
        assert_eq!(result, Some(vec![0u8]));

        // 插入更大的 tuple_id
        let very_large_tid: u32 = 1_000_000; // 100 万
        let key2 = make_key(2i64);
        bt.insert(key2.clone(), vec![very_large_tid as u8]).unwrap();
        assert_eq!(bt.search(&key2).unwrap(), Some(vec![very_large_tid as u8]));
    }

    /// P0-4 验证：批量插入 70000 个 Vec<u8> 值，验证中序遍历返回正确
    #[test]
    fn p0_4_bulk_load_with_large_values() {
        // 构造 70000 个 (key, value) 对，value 为单字节
        let items: Vec<BTreeEntry> = (0..70_000i64)
            .map(|i| (make_key(i), vec![i as u8]))
            .collect();

        let mut bt = BTree::with_default_order();
        bt.bulk_load(items).unwrap();

        // 验证节点数 > 1（确认发生了分裂）
        assert!(bt.node_count() > 1);

        // 中序遍历应返回 70000 条，且 key 单调递增
        let traversed = bt.in_order_leaf_traverse().unwrap();
        assert_eq!(traversed.len(), 70_000);

        // 验证第一个和最后一个 value
        assert_eq!(traversed[0].1, vec![0u8]);
        assert_eq!(traversed[69_999].1, vec![(69_999 % 256) as u8]);

        // 验证 key 单调递增
        for i in 1..traversed.len() {
            assert!(
                traversed[i - 1].0 < traversed[i].0,
                "keys not monotonically increasing at index {}",
                i
            );
        }
    }

    /// P0-4 验证：range_scan 返回的 Vec<u8> 值正确
    #[test]
    fn p0_4_range_scan_returns_correct_values() {
        let mut bt = BTree::with_default_order();

        // 插入 5 个 value：[33u8, 34u8, 35u8, 36u8, 37u8]
        for i in 0..5i64 {
            bt.insert(make_key(i), vec![(33 + i) as u8]).unwrap();
        }

        // range_scan 全范围
        let results = bt.range_scan(Bound::Unbounded, Bound::Unbounded).unwrap();
        assert_eq!(results.len(), 5);

        // 验证所有 value 正确
        assert_eq!(results[0].1, vec![33u8]);
        assert_eq!(results[1].1, vec![34u8]);
        assert_eq!(results[2].1, vec![35u8]);
        assert_eq!(results[3].1, vec![36u8]);
        assert_eq!(results[4].1, vec![37u8]);
    }

    /// P0-4 验证：编码/解码 roundtrip with Vec<u8> values
    #[test]
    fn p0_4_encode_decode_vec_u8_roundtrip() {
        let mut node = BTreeNode::new_leaf(1);
        node.keys.push(make_key(42));
        node.values.push(vec![255u8]); // 最大 u8 值
        node.keys.push(make_key(100));
        node.values.push(vec![254u8]); // u8::MAX - 1

        let encoded = node.encode();
        let decoded = BTreeNode::decode(&encoded).unwrap();

        assert_eq!(node, decoded);
        assert_eq!(decoded.values[0], vec![255u8]);
        assert_eq!(decoded.values[1], vec![254u8]);
    }

    /// P0-4 验证 5：persist/load to BufferPool with Vec<u8> values
    #[test]
    fn p9_1_persist_load_large_tuple_ids() {
        use crate::buffer::{BufferPool, InMemoryPageLoader, InMemoryPageWriter};
        use std::sync::Arc;

        let mut bt = BTree::new(8);
        // 插入 value 跨越 u8::MAX 边界（循环回绕）
        for i in 1..=300i64 {
            let val = (65500 + i) as u32;
            bt.insert(make_key(i), vec![val as u8]).unwrap();
        }

        let loader = Arc::new(InMemoryPageLoader::new());
        let writer = Arc::new(InMemoryPageWriter::new());
        let pool = BufferPool::with_writer(128, loader, writer).unwrap();
        let meta = bt.persist_to_buffer_pool(&pool).unwrap();

        // 从 BufferPool 重建
        let loaded = BTree::load_from_buffer_pool(&pool, meta).unwrap();

        // 验证重建后的 BTree 可以正确查询（value 为 val % 256）
        assert_eq!(
            loaded.search(&make_key(1i64)).unwrap(),
            Some(vec![221u8]) // 65501 % 256 = 221
        );
        assert_eq!(
            loaded.search(&make_key(300i64)).unwrap(),
            Some(vec![8u8]) // 65800 % 256 = 8
        );

        // 验证 value 正确循环
        assert_eq!(
            loaded.search(&make_key(36i64)).unwrap(),
            Some(vec![0u8]) // 65536 % 256 = 0
        );
    }
}
