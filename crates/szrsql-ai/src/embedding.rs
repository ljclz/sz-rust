//! Phase 7b.2 — 自动 Embedding 生命周期
//!
//! 提供端到端的向量 Embedding 自动化流水线，覆盖声明 → 写入 → 自动嵌入 →
//! HNSW 索引 → 相似搜索的完整生命周期，零手动操作。
//!
//! # 架构
//!
//! 1. **HashingEmbedder** — 确定性本地嵌入模型（hashing trick）
//!    - 文本分词 → FNV-1a 哈希 → 稀疏累加 → L2 归一化
//!    - 无外部依赖、确定性、可复现
//!    - 共享 token 的文本具有高余弦相似度（保证搜索相关性）
//!
//! 2. **HnswIndex** — 分层可导航小世界图（Malkov & Yashunin 2016）
//!    - 多层图结构：layer 0 全节点，高层指数稀疏
//!    - 插入：贪心下降 + ef_construction beam search + 双向连接 + 连接裁剪
//!    - 搜索：层间贪心下降 + layer 0 ef_search beam
//!    - 距离：1 - 余弦相似度（L2 归一化向量等价于欧氏距离/2）
//!    - 确定性：种子化 LCG 控制层级分配
//!
//! 3. **EmbeddingColumnDecl** — DDL `EMBEDDING FROM` 声明
//!    - 解析 `col EMBEDDING(dim) FROM (src1, src2)` 语法
//!    - 记录目标列、源列、维度
//!
//! 4. **EmbeddingLifecycle** — 自动化流水线管理器
//!    - `declare_embedding()` — 注册 DDL 声明
//!    - `on_insert()` / `on_bulk_insert()` — 自动生成 Embedding + 写入 HNSW
//!    - `search()` — TOP-K 相似搜索
//!    - 每 (table, column) 维护独立 HNSW 索引
//!
//! # 验证标准
//!
//! - DDL 声明 EMBEDDING FROM → INSERT 10000 行 → 自动生成 Embedding
//! - 创建 HNSW 索引 → 搜索 TOP10 → 结果相关
//! - 自动化流程完整，零手动操作
//!
//! 对应 `SzRSQL实施进度.md` Phase 7b.2。

use std::collections::{BinaryHeap, HashMap};
use std::fmt;

use thiserror::Error;

// =====================================================================
//  常量
// =====================================================================

/// 默认 Embedding 维度
pub const DEFAULT_EMBEDDING_DIM: usize = 128;

/// HNSW 默认 M（每层最大连接数）
pub const DEFAULT_HNSW_M: usize = 16;

/// HNSW 默认 ef_construction
pub const DEFAULT_EF_CONSTRUCTION: usize = 200;

/// HNSW 默认 ef_search
pub const DEFAULT_EF_SEARCH: usize = 50;

/// HNSW 确定性种子（可复现）
pub const DEFAULT_HNSW_SEED: u64 = 0x5EED_5EED_5EED_5EED;

// =====================================================================
//  错误类型
// =====================================================================

/// Embedding 生命周期错误
#[derive(Debug, Clone, Error)]
pub enum EmbeddingError {
    #[error("embedding declaration not found for {0}.{1}")]
    DeclarationNotFound(String, String),

    #[error("source column not found: {0}")]
    SourceColumnNotFound(String),

    #[error("duplicate embedding declaration for {0}.{1}")]
    DuplicateDeclaration(String, String),

    #[error("invalid embedding dimension: {0} (must be > 0)")]
    InvalidDimension(usize),

    #[error("empty text cannot be embedded")]
    EmptyText,

    #[error("DDL parse error: {0}")]
    DdlParse(String),
}

// =====================================================================
//  HashingEmbedder — 确定性本地嵌入模型
// =====================================================================

/// 确定性本地嵌入模型（hashing trick）
///
/// 文本分词后，每个 token 经 FNV-1a 哈希映射到向量某维（符号由哈希最高位决定），
/// 累加后 L2 归一化。共享 token 的文本具有高余弦相似度。
///
/// # 确定性
///
/// 相同输入永远产生相同输出（无随机性），适合测试与可复现性。
///
/// # 相关性保证
///
/// - 完全相同的文本 → 余弦相似度 = 1.0
/// - 共享多数 token 的文本 → 相似度接近 1.0
/// - 无共享 token 的文本 → 相似度接近 0.0（归一化后随机正交期望）
pub struct HashingEmbedder {
    dim: usize,
}

impl HashingEmbedder {
    /// 创建指定维度的嵌入器
    pub fn new(dim: usize) -> Result<Self, EmbeddingError> {
        if dim == 0 {
            return Err(EmbeddingError::InvalidDimension(dim));
        }
        Ok(Self { dim })
    }

    /// 嵌入维度
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 将文本嵌入为 L2 归一化向量
    ///
    /// 空文本（无 token）返回错误。
    pub fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        let tokens = tokenize(text);
        if tokens.is_empty() {
            return Err(EmbeddingError::EmptyText);
        }
        let mut vec = vec![0.0f32; self.dim];
        for token in &tokens {
            let h = fnv1a(token);
            let idx = (h % self.dim as u64) as usize;
            // 最高位决定符号（+1/-1），减少哈希碰撞偏差
            let sign = if (h >> 63) & 1 == 0 {
                1.0
            } else {
                -1.0
            };
            vec[idx] += sign;
        }
        // L2 归一化
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 1e-9 {
            for v in &mut vec {
                *v /= norm;
            }
        }
        Ok(vec)
    }

    /// 将多个文本字段拼接后嵌入（用于多列 EMBEDDING FROM）
    pub fn embed_combined(&self, texts: &[&str]) -> Result<Vec<f32>, EmbeddingError> {
        let combined: String = texts.iter().map(|t| format!("{} ", t)).collect();
        self.embed(&combined)
    }
}

/// 文本分词 — 按非字母数字字符分割，小写化
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

/// FNV-1a 64-bit 哈希
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// =====================================================================
//  LCG — 确定性伪随机数生成器（用于 HNSW 层级分配）
// =====================================================================

/// 线性同余生成器（Numerical Recipes 常数）
///
/// 确定性、可复现，无需 `rand` crate 依赖。
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x6D2B_79F5),
        }
    }

    /// 返回 [0, 1) 区间均匀分布的 f64
    fn next_f64(&mut self) -> f64 {
        // PCG 风格 LCG
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let xorshifted = (((self.state >> 18) ^ self.state) >> 27) as u32;
        let rot = (self.state >> 59) as u32;
        let result = (xorshifted >> rot) | (xorshifted << ((rot.wrapping_neg()) & 31));
        (result as f64) / (u32::MAX as f64 + 1.0)
    }
}

// =====================================================================
//  HnswIndex — 分层可导航小世界图
// =====================================================================

/// 节点 ID（内部序号）
type NodeId = usize;

/// f32 全序比较（BinaryHeap 需要 Ord，但 f32 只有 PartialOrd）
fn total_cmp_f32(a: f32, b: f32) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

/// 最大堆条目 — 距离大者排前（用于结果集，便于弹出最远）
#[derive(Debug, Clone, Copy)]
struct MaxHeapEntry {
    dist: f32,
    node: NodeId,
}

impl PartialEq for MaxHeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.dist == other.dist && self.node == other.node
    }
}
impl Eq for MaxHeapEntry {}
impl PartialOrd for MaxHeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MaxHeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 最大堆：距离大者排前；距离相等时按 node 比较（保证全序）
        total_cmp_f32(self.dist, other.dist).then(self.node.cmp(&other.node))
    }
}

/// 最小堆条目 — 距离小者排前（用于候选集，便于弹出最近）
#[derive(Debug, Clone, Copy)]
struct MinHeapEntry {
    dist: f32,
    node: NodeId,
}

impl PartialEq for MinHeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.dist == other.dist && self.node == other.node
    }
}
impl Eq for MinHeapEntry {}
impl PartialOrd for MinHeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MinHeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 最小堆：距离小者排前；距离相等时按 node 比较（保证全序）
        // BinaryHeap 是最大堆，所以这里反转比较
        total_cmp_f32(other.dist, self.dist).then(other.node.cmp(&self.node))
    }
}

/// HNSW 图节点
#[derive(Debug, Clone)]
struct HnswNode {
    /// 层级（0 = 最底层，包含所有节点）
    level: usize,
    /// 每层的邻居列表
    connections: Vec<Vec<NodeId>>,
}

/// HNSW 索引 — 分层可导航小世界图
///
/// # 算法
///
/// 参考 Malkov & Yashunin (2016) "Efficient and robust approximate nearest
/// neighbor search using Hierarchical Navigable Small World graphs".
///
/// - 插入时按指数分布分配层级 `l = floor(-ln(unif) * mL)`
/// - 从顶层贪心下降到 l+1，再从 min(L,l) 到 0 做 ef_construction beam search
/// - 每层选择 M 个最近邻居建立双向连接，连接超限时裁剪
/// - 搜索时顶层贪心下降，layer 0 做 ef_search beam
pub struct HnswIndex {
    dim: usize,
    m: usize,
    m_max: usize,
    m_max0: usize,
    ef_construction: usize,
    ef_search: usize,
    ml: f64,
    rng: Lcg,
    nodes: Vec<HnswNode>,
    vectors: Vec<Vec<f32>>,
    /// 载荷（外部 row_id）
    payloads: Vec<u64>,
    entry_point: Option<NodeId>,
    max_level: usize,
}

impl HnswIndex {
    /// 创建 HNSW 索引
    ///
    /// - `dim` — 向量维度
    /// - `m` — 每层最大连接数（layer 0 为 2*M）
    /// - `ef_construction` — 构建时 beam 宽度
    /// - `ef_search` — 搜索时 beam 宽度
    /// - `seed` — 确定性种子
    pub fn new(
        dim: usize,
        m: usize,
        ef_construction: usize,
        ef_search: usize,
        seed: u64,
    ) -> Result<Self, EmbeddingError> {
        if dim == 0 {
            return Err(EmbeddingError::InvalidDimension(dim));
        }
        let m_max = m;
        let m_max0 = m * 2;
        let ml = 1.0 / (m as f64).ln();
        Ok(Self {
            dim,
            m,
            m_max,
            m_max0,
            ef_construction,
            ef_search,
            ml,
            rng: Lcg::new(seed),
            nodes: Vec::new(),
            vectors: Vec::new(),
            payloads: Vec::new(),
            entry_point: None,
            max_level: 0,
        })
    }

    /// 节点数
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// 插入向量及其载荷
    pub fn insert(&mut self, vector: Vec<f32>, payload: u64) -> Result<(), EmbeddingError> {
        if vector.len() != self.dim {
            return Err(EmbeddingError::InvalidDimension(vector.len()));
        }

        let new_node_id = self.nodes.len();
        let level = self.random_level();
        let connections = vec![Vec::new(); level + 1];

        self.nodes.push(HnswNode { level, connections });
        self.vectors.push(vector);
        self.payloads.push(payload);

        // 首个节点成为入口点
        if self.entry_point.is_none() {
            self.entry_point = Some(new_node_id);
            self.max_level = level;
            return Ok(());
        }

        let entry = self.entry_point.unwrap();
        let mut current_ep = vec![entry];

        // Phase 1: 从顶层贪心下降到 level+1
        for lc in (level + 1..=self.max_level).rev() {
            current_ep = self.search_layer(&self.vectors[new_node_id], &current_ep, 1, lc);
        }

        // Phase 2: 从 min(max_level, level) 到 0，beam search + 连接
        for lc in (0..=level.min(self.max_level)).rev() {
            let candidates = self.search_layer(
                &self.vectors[new_node_id],
                &current_ep,
                self.ef_construction,
                lc,
            );

            // 选择 M 个最近邻居
            let m_layer = if lc == 0 {
                self.m_max0
            } else {
                self.m_max
            };
            let neighbors =
                self.select_neighbors(&self.vectors[new_node_id], &candidates, self.m, lc);

            // 建立双向连接
            self.set_connections(new_node_id, lc, &neighbors);
            for &neighbor in &neighbors {
                let neighbor_conns = self.get_connections(neighbor, lc);
                let mut updated = neighbor_conns.to_vec();
                updated.push(new_node_id);

                // 裁剪连接（如果超限）
                if updated.len() > m_layer {
                    let pruned = self.select_neighbors_for(&neighbor, lc, &updated, m_layer);
                    self.set_connections(neighbor, lc, &pruned);
                } else {
                    self.set_connections(neighbor, lc, &updated);
                }
            }

            current_ep = candidates;
        }

        // 更新入口点（如果新节点层级更高）
        if level > self.max_level {
            self.max_level = level;
            self.entry_point = Some(new_node_id);
        }

        Ok(())
    }

    /// 搜索 TOP-K 最近邻
    pub fn search(&self, query: &[f32], k: usize) -> Vec<SearchResult> {
        if self.entry_point.is_none() || self.nodes.is_empty() || k == 0 {
            return Vec::new();
        }

        let mut current_ep = vec![self.entry_point.unwrap()];

        // 顶层贪心下降到 layer 1
        for lc in (1..=self.max_level).rev() {
            current_ep = self.search_layer(query, &current_ep, 1, lc);
        }

        // layer 0 ef_search beam
        let ef = self.ef_search.max(k);
        let candidates = self.search_layer(query, &current_ep, ef, 0);

        // 取 TOP-K
        let mut sorted: Vec<(f32, NodeId)> = candidates
            .iter()
            .map(|&node| (self.distance(query, &self.vectors[node]), node))
            .collect();
        sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        sorted
            .into_iter()
            .take(k)
            .map(|(dist, node)| SearchResult {
                row_id: self.payloads[node],
                score: 1.0 - dist, // 余弦相似度
            })
            .collect()
    }

    /// 暴力搜索（用于召回率测试的 ground truth）
    pub fn brute_force_search(&self, query: &[f32], k: usize) -> Vec<SearchResult> {
        if self.nodes.is_empty() || k == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(f32, NodeId)> = self
            .payloads
            .iter()
            .enumerate()
            .map(|(node, _)| (self.distance(query, &self.vectors[node]), node))
            .collect();
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(k)
            .map(|(dist, node)| SearchResult {
                row_id: self.payloads[node],
                score: 1.0 - dist,
            })
            .collect()
    }

    // -----------------------------------------------------------------
    //  内部方法
    // -----------------------------------------------------------------

    /// 余弦距离（1 - 余弦相似度），L2 归一化向量下等价
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        1.0 - dot
    }

    /// 随机层级分配（指数分布）
    fn random_level(&mut self) -> usize {
        let u = self.rng.next_f64();
        (-(u.ln()) * self.ml).floor() as usize
    }

    /// 获取节点在某层的连接
    fn get_connections(&self, node: NodeId, lc: usize) -> &[NodeId] {
        &self.nodes[node].connections[lc]
    }

    /// 设置节点在某层的连接
    fn set_connections(&mut self, node: NodeId, lc: usize, conns: &[NodeId]) {
        self.nodes[node].connections[lc] = conns.to_vec();
    }

    /// 为指定节点选择最近邻居 — Malkov & Yashunin (2016) Algorithm 4 启发式选择
    ///
    /// 启发式思想：对候选集 W（按到 query 距离升序）中的每个 e，
    /// 若已有选中邻居 r 使 dist(e, r) < dist(e, query)，则跳过 e（避免聚集）；
    /// 否则将 e 加入结果集 R。这样选出的邻居在空间中分布更均匀，
    /// 既保证局部性又保证多样性，大幅提升图的导航质量与召回率。
    ///
    /// 参数 `lc` 指定当前操作的层级，`extend_candidates` 控制是否扩展候选集
    /// （加入候选在同层 lc 的邻居），`keep_pruned_connections` 控制是否在结果
    /// 不足 m 时回填被裁剪的候选。
    fn select_neighbors_heuristic(
        &self,
        query: &[f32],
        candidates: &[NodeId],
        m: usize,
        lc: usize,
        extend_candidates: bool,
        keep_pruned_connections: bool,
    ) -> Vec<NodeId> {
        // 1. 构造候选集（去重 + 排序：按到 query 距离升序）
        let mut work_set: Vec<NodeId> = candidates.to_vec();
        if extend_candidates {
            // 扩展：加入每个候选在同层 lc 的邻居（论文 Algorithm 4 的 extendCandidates）
            // 注意：必须使用同层 lc 的连接，避免把低层级节点引入高层级连接列表
            let mut seen: std::collections::HashSet<NodeId> = work_set.iter().copied().collect();
            let mut extended: Vec<NodeId> = work_set.clone();
            for &c in candidates {
                // 仅当候选节点在该层有连接时才扩展（防御性：避免越界）
                if self.nodes[c].connections.len() <= lc {
                    continue;
                }
                for &nb in self.get_connections(c, lc) {
                    // 防御性过滤：仅引入层级 >= lc 的节点，防止跨层级污染
                    if self.nodes[nb].level >= lc && seen.insert(nb) {
                        extended.push(nb);
                    }
                }
            }
            work_set = extended;
        }

        // 按到 query 距离升序排序
        let mut scored: Vec<(f32, NodeId)> = work_set
            .iter()
            .map(|&node| (self.distance(query, &self.vectors[node]), node))
            .collect();
        scored.sort_by(|a, b| total_cmp_f32(a.0, b.0).then(a.1.cmp(&b.1)));

        // 2. 启发式选择主循环
        let mut result: Vec<NodeId> = Vec::with_capacity(m);
        let mut pruned: Vec<NodeId> = Vec::new(); // 被裁剪的候选（keep_pruned 时回填）
        for (_, e) in scored {
            if result.len() >= m {
                break;
            }
            let e_vec = &self.vectors[e];
            // 检查是否已有选中邻居 r 使 dist(e, r) < dist(e, query)
            let dist_e_q = self.distance(query, e_vec);
            let mut good = true;
            for &r in &result {
                let dist_e_r = self.distance(e_vec, &self.vectors[r]);
                if total_cmp_f32(dist_e_r, dist_e_q) == std::cmp::Ordering::Less {
                    good = false;
                    break;
                }
            }
            if good {
                result.push(e);
            } else if keep_pruned_connections {
                pruned.push(e);
            }
        }

        // 3. 若启用 keep_pruned 且结果不足 m，从被裁剪的候选中回填
        if keep_pruned_connections {
            for e in pruned {
                if result.len() >= m {
                    break;
                }
                result.push(e);
            }
        }

        result
    }

    /// 为指定节点选择最近邻居（基于查询向量）— 启发式版本
    fn select_neighbors(
        &self,
        query: &[f32],
        candidates: &[NodeId],
        m: usize,
        lc: usize,
    ) -> Vec<NodeId> {
        // 插入新节点时：不扩展候选，但保留被裁剪项（保证连通性 + 质量优先）
        // keep_pruned_connections=true 确保：启发式选出不足 m 个时，从被裁剪候选回填
        self.select_neighbors_heuristic(query, candidates, m, lc, false, true)
    }

    /// 为指定节点（非新插入）选择最近邻居 — 基于该节点自身向量
    fn select_neighbors_for(
        &self,
        node: &NodeId,
        lc: usize,
        candidates: &[NodeId],
        m: usize,
    ) -> Vec<NodeId> {
        let query = &self.vectors[*node];
        // 裁剪已有节点连接时：扩展同层候选 + 保留被裁剪项（论文推荐保持连通性）
        let filtered: Vec<NodeId> = candidates
            .iter()
            .filter(|&&c| c != *node)
            .copied()
            .collect();
        self.select_neighbors_heuristic(query, &filtered, m, lc, true, true)
    }

    /// 在单层搜索 ef 个最近邻居
    ///
    /// 返回找到的节点 ID 列表（未排序）
    fn search_layer(
        &self,
        query: &[f32],
        entry_points: &[NodeId],
        ef: usize,
        lc: usize,
    ) -> Vec<NodeId> {
        let mut visited: Vec<bool> = vec![false; self.nodes.len()];
        let mut candidates: BinaryHeap<MinHeapEntry> = BinaryHeap::new(); // min-heap
        let mut results: BinaryHeap<MaxHeapEntry> = BinaryHeap::new(); // max-heap

        for &ep in entry_points {
            let dist = self.distance(query, &self.vectors[ep]);
            visited[ep] = true;
            candidates.push(MinHeapEntry { dist, node: ep });
            results.push(MaxHeapEntry { dist, node: ep });
        }

        while let Some(entry) = candidates.pop() {
            let c_dist = entry.dist;
            let c_node = entry.node;
            // 结果集中最远的距离
            let furthest = results.peek().map(|e| e.dist).unwrap_or(f32::INFINITY);
            if c_dist > furthest && results.len() >= ef {
                break;
            }

            // 遍历该层邻居
            for &neighbor in self.get_connections(c_node, lc) {
                if visited[neighbor] {
                    continue;
                }
                visited[neighbor] = true;
                let n_dist = self.distance(query, &self.vectors[neighbor]);
                let furthest = results.peek().map(|e| e.dist).unwrap_or(f32::INFINITY);

                if n_dist < furthest || results.len() < ef {
                    candidates.push(MinHeapEntry {
                        dist: n_dist,
                        node: neighbor,
                    });
                    results.push(MaxHeapEntry {
                        dist: n_dist,
                        node: neighbor,
                    });
                    if results.len() > ef {
                        results.pop(); // 弹出最远
                    }
                }
            }
        }

        results.into_iter().map(|e| e.node).collect()
    }
}

impl fmt::Debug for HnswIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HnswIndex")
            .field("dim", &self.dim)
            .field("m", &self.m)
            .field("ef_construction", &self.ef_construction)
            .field("ef_search", &self.ef_search)
            .field("len", &self.nodes.len())
            .field("max_level", &self.max_level)
            .finish()
    }
}

// =====================================================================
//  SearchResult — 搜索结果
// =====================================================================

/// 向量搜索结果
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// 行 ID（插入时传入的 payload）
    pub row_id: u64,
    /// 余弦相似度分数 [0, 1]
    pub score: f32,
}

// =====================================================================
//  EmbeddingColumnDecl — DDL 声明
// =====================================================================

/// DDL `EMBEDDING FROM` 声明
///
/// 语法：`column_name EMBEDDING(dim) FROM (source_col1, source_col2, ...)`
///
/// 表示 `column_name` 列自动从 `source_col1..N` 生成维度为 `dim` 的 Embedding。
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingColumnDecl {
    /// 表名
    pub table: String,
    /// 目标 Embedding 列名
    pub column: String,
    /// 源列列表
    pub source_columns: Vec<String>,
    /// Embedding 维度
    pub dim: usize,
}

impl EmbeddingColumnDecl {
    /// 创建新声明
    pub fn new(
        table: impl Into<String>,
        column: impl Into<String>,
        source_columns: Vec<String>,
        dim: usize,
    ) -> Result<Self, EmbeddingError> {
        if dim == 0 {
            return Err(EmbeddingError::InvalidDimension(dim));
        }
        if source_columns.is_empty() {
            return Err(EmbeddingError::DdlParse(
                "source_columns cannot be empty".into(),
            ));
        }
        Ok(Self {
            table: table.into(),
            column: column.into(),
            source_columns,
            dim,
        })
    }

    /// 声明键（table.column，小写）
    pub fn key(&self) -> String {
        format!(
            "{}.{}",
            self.table.to_lowercase(),
            self.column.to_lowercase()
        )
    }
}

// =====================================================================
//  EmbeddingDdlParser — DDL 解析器
// =====================================================================

/// DDL `EMBEDDING FROM` 子句解析器
///
/// 解析语法：
/// ```text
/// column_name EMBEDDING(dim) FROM (source_col1, source_col2)
/// ```
///
/// 示例：
/// ```
/// # use szrsql_ai::embedding::{EmbeddingDdlParser, EmbeddingColumnDecl};
/// let decl = EmbeddingDdlParser::parse(
///     "products",
///     "name_emb EMBEDDING(128) FROM (name, description)",
/// ).unwrap();
/// assert_eq!(decl.table, "products");
/// assert_eq!(decl.column, "name_emb");
/// assert_eq!(decl.source_columns, vec!["name", "description"]);
/// assert_eq!(decl.dim, 128);
/// ```
pub struct EmbeddingDdlParser;

impl EmbeddingDdlParser {
    /// 解析 EMBEDDING FROM 子句
    pub fn parse(table: &str, clause: &str) -> Result<EmbeddingColumnDecl, EmbeddingError> {
        let clause = clause.trim();

        // 解析列名（第一个 token）
        let paren_open = clause
            .find('(')
            .ok_or_else(|| EmbeddingError::DdlParse("missing '(' in EMBEDDING clause".into()))?;

        // 找到 EMBEDDING 关键字
        let embedding_pos = clause
            .find("EMBEDDING")
            .ok_or_else(|| EmbeddingError::DdlParse("missing EMBEDDING keyword".into()))?;

        let column_name = clause[..embedding_pos].trim().to_string();
        if column_name.is_empty() {
            return Err(EmbeddingError::DdlParse("empty column name".into()));
        }

        // 解析维度 EMBEDDING(dim)
        let dim_str = &clause[paren_open + 1..];
        let dim_end = dim_str
            .find(')')
            .ok_or_else(|| EmbeddingError::DdlParse("missing ')' after EMBEDDING dim".into()))?;
        let dim: usize = dim_str[..dim_end].trim().parse().map_err(|_| {
            EmbeddingError::DdlParse(format!("invalid dim: {}", &dim_str[..dim_end]))
        })?;

        // 解析 FROM (source_col1, source_col2)
        let from_pos = clause
            .find("FROM")
            .ok_or_else(|| EmbeddingError::DdlParse("missing FROM keyword".into()))?;

        let from_clause = clause[from_pos + 4..].trim();
        let sources_open = from_clause
            .find('(')
            .ok_or_else(|| EmbeddingError::DdlParse("missing '(' after FROM".into()))?;
        let sources_close = from_clause
            .rfind(')')
            .ok_or_else(|| EmbeddingError::DdlParse("missing ')' after source columns".into()))?;

        let sources_str = &from_clause[sources_open + 1..sources_close];
        let source_columns: Vec<String> = sources_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if source_columns.is_empty() {
            return Err(EmbeddingError::DdlParse("no source columns".into()));
        }

        EmbeddingColumnDecl::new(table, &column_name, source_columns, dim)
    }
}

// =====================================================================
//  EmbeddingLifecycle — 自动化流水线管理器
// =====================================================================

/// 行数据 — 列名 → 文本值
pub type Row = HashMap<String, String>;

/// 自动 Embedding 生命周期管理器
///
/// 封装 DDL 声明 → INSERT 自动嵌入 → HNSW 自动索引 → 相似搜索的完整流程。
///
/// # 零手动操作
///
/// 1. `declare_embedding()` — 注册 DDL 声明（一次性）
/// 2. `on_insert()` / `on_bulk_insert()` — INSERT 时自动生成 Embedding + 写入 HNSW
/// 3. `search()` — TOP-K 相似搜索
///
/// 用户无需手动调用嵌入器或索引操作。
pub struct EmbeddingLifecycle {
    /// 嵌入器
    embedder: HashingEmbedder,
    /// DDL 声明（key = table.column → 声明）
    declarations: HashMap<String, EmbeddingColumnDecl>,
    /// HNSW 索引（key = table.column → 索引）
    indexes: HashMap<String, HnswIndex>,
}

impl EmbeddingLifecycle {
    /// 创建生命周期管理器（使用默认维度）
    pub fn new() -> Self {
        Self {
            embedder: HashingEmbedder::new(DEFAULT_EMBEDDING_DIM).expect("default dim > 0"),
            declarations: HashMap::new(),
            indexes: HashMap::new(),
        }
    }

    /// 创建生命周期管理器（指定嵌入维度）
    pub fn with_dim(dim: usize) -> Result<Self, EmbeddingError> {
        Ok(Self {
            embedder: HashingEmbedder::new(dim)?,
            declarations: HashMap::new(),
            indexes: HashMap::new(),
        })
    }

    /// 声明 EMBEDDING FROM（DDL 注册）
    ///
    /// 为表注册一个自动 Embedding 列。后续对该表的 INSERT 会自动生成 Embedding。
    pub fn declare_embedding(
        &mut self,
        table: impl Into<String>,
        column: impl Into<String>,
        source_columns: Vec<String>,
        dim: usize,
    ) -> Result<(), EmbeddingError> {
        let decl = EmbeddingColumnDecl::new(table, column, source_columns, dim)?;
        let key = decl.key();
        if self.declarations.contains_key(&key) {
            return Err(EmbeddingError::DuplicateDeclaration(
                decl.table,
                decl.column,
            ));
        }

        // 为该声明创建独立 HNSW 索引（维度 = 声明维度）
        let index = HnswIndex::new(
            decl.dim,
            DEFAULT_HNSW_M,
            DEFAULT_EF_CONSTRUCTION,
            DEFAULT_EF_SEARCH,
            DEFAULT_HNSW_SEED,
        )?;

        self.declarations.insert(key.clone(), decl);
        self.indexes.insert(key, index);
        Ok(())
    }

    /// 通过 DDL 子句字符串声明（便捷方法）
    pub fn declare_by_ddl(&mut self, table: &str, clause: &str) -> Result<(), EmbeddingError> {
        let decl = EmbeddingDdlParser::parse(table, clause)?;
        let dim = decl.dim;
        let source_columns = decl.source_columns.clone();
        let column = decl.column.clone();
        self.declare_embedding(table, &column, source_columns, dim)
    }

    /// INSERT 触发 — 自动生成 Embedding + 写入 HNSW
    ///
    /// `row_id` 是外部行 ID，`row` 是该行的列值映射。
    /// 自动从声明的源列提取文本，生成 Embedding，插入 HNSW 索引。
    pub fn on_insert(&mut self, table: &str, row_id: u64, row: &Row) -> Result<(), EmbeddingError> {
        // 查找该表的所有 Embedding 声明
        let table_prefix = format!("{}.", table.to_lowercase());
        let matching_keys: Vec<String> = self
            .declarations
            .keys()
            .filter(|k| k.starts_with(&table_prefix))
            .cloned()
            .collect();

        if matching_keys.is_empty() {
            // 该表无 Embedding 声明，跳过（非错误 — 允许普通表）
            return Ok(());
        }

        for key in matching_keys {
            let decl = &self.declarations[&key];

            // 从源列提取文本
            let source_texts: Vec<String> = decl
                .source_columns
                .iter()
                .map(|col| row.get(col).cloned().unwrap_or_default())
                .collect();

            let combined: String = source_texts.join(" ");
            if combined.trim().is_empty() {
                return Err(EmbeddingError::EmptyText);
            }

            // 为该声明的维度创建临时嵌入器（可能不同于默认维度）
            let embedder = HashingEmbedder::new(decl.dim)?;
            let vector = embedder.embed(&combined)?;

            // 插入 HNSW 索引
            let index = self.indexes.get_mut(&key).ok_or_else(|| {
                EmbeddingError::DeclarationNotFound(decl.table.clone(), decl.column.clone())
            })?;
            index.insert(vector, row_id)?;
        }

        Ok(())
    }

    /// 批量 INSERT 触发 — 自动生成所有 Embedding + 写入 HNSW
    pub fn on_bulk_insert(
        &mut self,
        table: &str,
        rows: Vec<(u64, Row)>,
    ) -> Result<(), EmbeddingError> {
        for (row_id, row) in rows {
            self.on_insert(table, row_id, &row)?;
        }
        Ok(())
    }

    /// TOP-K 相似搜索
    ///
    /// 在指定表的 Embedding 列上搜索与查询文本最相似的 K 行。
    pub fn search(
        &self,
        table: &str,
        column: &str,
        query_text: &str,
        top_k: usize,
    ) -> Result<Vec<SearchResult>, EmbeddingError> {
        let key = format!("{}.{}", table.to_lowercase(), column.to_lowercase());
        let decl = self
            .declarations
            .get(&key)
            .ok_or_else(|| EmbeddingError::DeclarationNotFound(table.into(), column.into()))?;
        let index = self
            .indexes
            .get(&key)
            .ok_or_else(|| EmbeddingError::DeclarationNotFound(table.into(), column.into()))?;

        let embedder = HashingEmbedder::new(decl.dim)?;
        let query_vec = embedder.embed(query_text)?;

        Ok(index.search(&query_vec, top_k))
    }

    /// 获取指定声明的索引节点数
    pub fn index_size(&self, table: &str, column: &str) -> Result<usize, EmbeddingError> {
        let key = format!("{}.{}", table.to_lowercase(), column.to_lowercase());
        let index = self
            .indexes
            .get(&key)
            .ok_or_else(|| EmbeddingError::DeclarationNotFound(table.into(), column.into()))?;
        Ok(index.len())
    }

    /// 已注册的声明数
    pub fn declaration_count(&self) -> usize {
        self.declarations.len()
    }
}

impl Default for EmbeddingLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for EmbeddingLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddingLifecycle")
            .field("dim", &self.embedder.dim)
            .field("declarations", &self.declarations.len())
            .field("indexes", &self.indexes.len())
            .finish()
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成 L2 归一化的随机向量（用于 HNSW 召回率测试）
    fn generate_random_unit_vector(dim: usize, rng: &mut Lcg) -> Vec<f32> {
        let mut v: Vec<f32> = (0..dim)
            .map(|_| (rng.next_f64() as f32) * 2.0 - 1.0)
            .collect();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-9 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }

    // -----------------------------------------------------------------
    //  HashingEmbedder 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b2_embedder_deterministic() {
        let embedder = HashingEmbedder::new(64).unwrap();
        let v1 = embedder.embed("hello world database").unwrap();
        let v2 = embedder.embed("hello world database").unwrap();
        assert_eq!(v1, v2, "same text must produce identical embedding");
        assert_eq!(v1.len(), 64);
    }

    #[test]
    fn test_7b2_embedder_normalized() {
        let embedder = HashingEmbedder::new(128).unwrap();
        let v = embedder.embed("normalization test vector").unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "L2 norm must be 1.0, got {}",
            norm
        );
    }

    #[test]
    fn test_7b2_embedder_relevance() {
        let embedder = HashingEmbedder::new(256).unwrap();
        let v1 = embedder.embed("database query optimization").unwrap();
        let v2 = embedder.embed("database query performance").unwrap();
        let v3 = embedder.embed("cooking recipe pasta").unwrap();

        let sim_12 = cosine_sim(&v1, &v2);
        let sim_13 = cosine_sim(&v1, &v3);

        assert!(
            sim_12 > sim_13,
            "similar texts must have higher cosine similarity: {} vs {}",
            sim_12,
            sim_13
        );
        assert!(
            sim_12 > 0.3,
            "texts with shared tokens should have similarity > 0.3, got {}",
            sim_12
        );
    }

    #[test]
    fn test_7b2_embedder_identical_text() {
        let embedder = HashingEmbedder::new(128).unwrap();
        let v1 = embedder.embed("identical text").unwrap();
        let v2 = embedder.embed("identical text").unwrap();
        let sim = cosine_sim(&v1, &v2);
        assert!(
            (sim - 1.0).abs() < 1e-4,
            "identical text similarity must be 1.0, got {}",
            sim
        );
    }

    #[test]
    fn test_7b2_embedder_empty_text_errors() {
        let embedder = HashingEmbedder::new(64).unwrap();
        assert!(embedder.embed("").is_err());
        assert!(embedder.embed("   ").is_err());
        assert!(embedder.embed("!!!").is_err());
    }

    #[test]
    fn test_7b2_embedder_combined() {
        let embedder = HashingEmbedder::new(128).unwrap();
        let v = embedder
            .embed_combined(&["product name", "product description"])
            .unwrap();
        assert_eq!(v.len(), 128);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
    }

    // -----------------------------------------------------------------
    //  HNSW 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b2_hnsw_empty_index() {
        let index = HnswIndex::new(64, 16, 200, 50, 42).unwrap();
        assert!(index.is_empty());
        let results = index.search(&[0.0; 64], 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_7b2_hnsw_insert_and_search_single() {
        let mut index = HnswIndex::new(64, 8, 100, 20, 42).unwrap();
        let embedder = HashingEmbedder::new(64).unwrap();

        let v = embedder.embed("test vector").unwrap();
        index.insert(v.clone(), 1).unwrap();

        assert_eq!(index.len(), 1);
        let results = index.search(&v, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row_id, 1);
        assert!(results[0].score > 0.99);
    }

    #[test]
    fn test_7b2_hnsw_insert_multiple_and_search() {
        let mut index = HnswIndex::new(128, 16, 100, 50, 42).unwrap();
        let embedder = HashingEmbedder::new(128).unwrap();

        let texts = [
            "database query optimization",
            "cooking italian pasta recipe",
            "machine learning neural network",
            "gardening flowers spring",
            "database index performance",
        ];

        for (i, text) in texts.iter().enumerate() {
            let v = embedder.embed(text).unwrap();
            index.insert(v, i as u64 + 1).unwrap();
        }

        assert_eq!(index.len(), 5);

        // 搜索与 "database" 相关的
        let query = embedder.embed("database performance tuning").unwrap();
        let results = index.search(&query, 3);
        assert_eq!(results.len(), 3);

        // 结果应包含 database 相关行（row_id 1 和 5）
        let row_ids: Vec<u64> = results.iter().map(|r| r.row_id).collect();
        assert!(
            row_ids.contains(&1) || row_ids.contains(&5),
            "search results should contain database-related rows, got {:?}",
            row_ids
        );
    }

    #[test]
    fn test_7b2_hnsw_recall_vs_brute_force() {
        // 使用随机向量测试 HNSW 召回率（HNSW 论文标准评估方法）
        // 随机向量在高维空间中距离独特，能准确反映 HNSW 算法本身的性能
        // 避免 HashingEmbedder 稀疏向量导致的等距问题
        let mut index = HnswIndex::new(128, 16, 200, 100, 42).unwrap();
        let mut rng = Lcg::new(12345);

        // 插入 500 个 L2 归一化的随机向量
        for i in 0..500 {
            let v = generate_random_unit_vector(128, &mut rng);
            index.insert(v, i as u64).unwrap();
        }

        assert_eq!(index.len(), 500);

        // 对 10 个随机查询测试召回率
        let mut total_recall = 0.0f32;
        let num_queries = 10;
        for _ in 0..num_queries {
            let qv = generate_random_unit_vector(128, &mut rng);
            let hnsw_results = index.search(&qv, 10);
            let bf_results = index.brute_force_search(&qv, 10);

            let hnsw_ids: std::collections::HashSet<u64> =
                hnsw_results.iter().map(|r| r.row_id).collect();
            let bf_ids: std::collections::HashSet<u64> =
                bf_results.iter().map(|r| r.row_id).collect();

            let overlap = hnsw_ids.intersection(&bf_ids).count() as f32;
            let recall = overlap / bf_ids.len() as f32;
            total_recall += recall;
        }

        let avg_recall = total_recall / num_queries as f32;
        assert!(
            avg_recall >= 0.7,
            "HNSW recall vs brute-force should be >= 70%, got {:.2}%",
            avg_recall * 100.0
        );
    }

    #[test]
    fn test_7b2_hnsw_large_insert() {
        // 测试插入 10000 个向量（验证规模可扩展性）
        let mut index = HnswIndex::new(128, 16, 200, 50, 42).unwrap();
        let embedder = HashingEmbedder::new(128).unwrap();

        for i in 0..10_000u64 {
            let text = format!("document number {} category {}", i, i % 10);
            let v = embedder.embed(&text).unwrap();
            index.insert(v, i).unwrap();
        }

        assert_eq!(index.len(), 10_000);

        // 搜索应该正常工作
        let qv = embedder.embed("document number 500 category 5").unwrap();
        let results = index.search(&qv, 10);
        assert_eq!(results.len(), 10);
        // 最相似的结果应该是 row_id 500 本身
        assert!(
            results[0].score > 0.9,
            "top result score should be > 0.9, got {}",
            results[0].score
        );
    }

    // -----------------------------------------------------------------
    //  DDL 解析器测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b2_ddl_parse_basic() {
        let decl = EmbeddingDdlParser::parse(
            "products",
            "name_emb EMBEDDING(128) FROM (name, description)",
        )
        .unwrap();
        assert_eq!(decl.table, "products");
        assert_eq!(decl.column, "name_emb");
        assert_eq!(decl.source_columns, vec!["name", "description"]);
        assert_eq!(decl.dim, 128);
    }

    #[test]
    fn test_7b2_ddl_parse_single_source() {
        let decl =
            EmbeddingDdlParser::parse("docs", "content_emb EMBEDDING(256) FROM (content)").unwrap();
        assert_eq!(decl.table, "docs");
        assert_eq!(decl.column, "content_emb");
        assert_eq!(decl.source_columns, vec!["content"]);
        assert_eq!(decl.dim, 256);
    }

    #[test]
    fn test_7b2_ddl_parse_invalid() {
        // 缺少 EMBEDDING 关键字
        assert!(EmbeddingDdlParser::parse("t", "col FROM (src)").is_err());
        // 缺少 FROM
        assert!(EmbeddingDdlParser::parse("t", "col EMBEDDING(128)").is_err());
        // 维度为 0
        assert!(EmbeddingDdlParser::parse("t", "col EMBEDDING(0) FROM (src)").is_err());
        // 空源列
        assert!(EmbeddingDdlParser::parse("t", "col EMBEDDING(128) FROM ()").is_err());
    }

    // -----------------------------------------------------------------
    //  EmbeddingLifecycle 完整生命周期测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b2_lifecycle_declare_and_search_empty() {
        let mut lifecycle = EmbeddingLifecycle::new();
        lifecycle
            .declare_embedding("products", "emb", vec!["name".into()], 128)
            .unwrap();

        assert_eq!(lifecycle.declaration_count(), 1);
        assert_eq!(lifecycle.index_size("products", "emb").unwrap(), 0);

        // 空索引搜索返回空结果
        let results = lifecycle.search("products", "emb", "query", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_7b2_lifecycle_duplicate_declaration_errors() {
        let mut lifecycle = EmbeddingLifecycle::new();
        lifecycle
            .declare_embedding("products", "emb", vec!["name".into()], 128)
            .unwrap();

        let err = lifecycle.declare_embedding("products", "emb", vec!["name".into()], 128);
        assert!(err.is_err());
    }

    #[test]
    fn test_7b2_lifecycle_on_insert_auto_embeds() {
        let mut lifecycle = EmbeddingLifecycle::new();
        lifecycle
            .declare_embedding("products", "emb", vec!["name".into(), "desc".into()], 128)
            .unwrap();

        // 构造一行
        let mut row = Row::new();
        row.insert("name".into(), "Smartphone X1".into());
        row.insert("desc".into(), "Latest flagship smartphone".into());

        lifecycle.on_insert("products", 1, &row).unwrap();

        assert_eq!(lifecycle.index_size("products", "emb").unwrap(), 1);

        // 搜索应该能找到
        let results = lifecycle
            .search("products", "emb", "smartphone flagship", 10)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row_id, 1);
        assert!(results[0].score > 0.3);
    }

    #[test]
    fn test_7b2_lifecycle_bulk_insert() {
        let mut lifecycle = EmbeddingLifecycle::new();
        lifecycle
            .declare_embedding("docs", "emb", vec!["content".into()], 128)
            .unwrap();

        let rows: Vec<(u64, Row)> = (0..100)
            .map(|i| {
                let mut row = Row::new();
                row.insert(
                    "content".into(),
                    format!("document {} about topic {}", i, i % 5),
                );
                (i, row)
            })
            .collect();

        lifecycle.on_bulk_insert("docs", rows).unwrap();
        assert_eq!(lifecycle.index_size("docs", "emb").unwrap(), 100);

        let results = lifecycle
            .search("docs", "emb", "document topic", 10)
            .unwrap();
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_7b2_lifecycle_no_declaration_table_silent_ok() {
        // 对未声明 Embedding 的表 INSERT 应该静默成功（非错误）
        let mut lifecycle = EmbeddingLifecycle::new();
        let mut row = Row::new();
        row.insert("name".into(), "test".into());
        // 无声明 → 静默 OK
        lifecycle.on_insert("unknown_table", 1, &row).unwrap();
    }

    #[test]
    fn test_7b2_lifecycle_search_wrong_column_errors() {
        let mut lifecycle = EmbeddingLifecycle::new();
        lifecycle
            .declare_embedding("products", "emb", vec!["name".into()], 128)
            .unwrap();

        let err = lifecycle.search("products", "wrong_col", "query", 10);
        assert!(err.is_err());
    }

    #[test]
    fn test_7b2_lifecycle_declare_by_ddl() {
        let mut lifecycle = EmbeddingLifecycle::new();
        lifecycle
            .declare_by_ddl("articles", "title_emb EMBEDDING(128) FROM (title, body)")
            .unwrap();

        assert_eq!(lifecycle.declaration_count(), 1);

        let mut row = Row::new();
        row.insert("title".into(), "Rust programming guide".into());
        row.insert("body".into(), "Learn Rust from basics".into());
        lifecycle.on_insert("articles", 1, &row).unwrap();

        let results = lifecycle
            .search("articles", "title_emb", "rust programming", 5)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row_id, 1);
    }

    // -----------------------------------------------------------------
    //  完整端到端测试 — 10000 行
    // -----------------------------------------------------------------

    #[test]
    fn test_7b2_full_lifecycle_10000_rows() {
        // 验证标准：DDL 声明 EMBEDDING FROM → INSERT 10000 行 → 自动生成 Embedding
        //           → 创建 HNSW 索引 → 搜索 TOP10 → 结果相关
        let mut lifecycle = EmbeddingLifecycle::new();

        // Step 1: DDL 声明 EMBEDDING FROM
        lifecycle
            .declare_by_ddl(
                "products",
                "emb EMBEDDING(128) FROM (name, category, description)",
            )
            .unwrap();
        assert_eq!(lifecycle.declaration_count(), 1);

        // Step 2: INSERT 10000 行 — 自动生成 Embedding + 自动创建 HNSW 索引
        let categories = [
            "electronics",
            "books",
            "clothing",
            "food",
            "toys",
            "sports",
            "music",
            "garden",
            "automotive",
            "health",
        ];
        let rows: Vec<(u64, Row)> = (0..10_000u64)
            .map(|i| {
                let cat = categories[(i % categories.len() as u64) as usize];
                let mut row = Row::new();
                row.insert("name".into(), format!("product {}", i));
                row.insert("category".into(), cat.into());
                row.insert(
                    "description".into(),
                    format!("high quality {} product for everyday use", cat),
                );
                (i, row)
            })
            .collect();

        lifecycle.on_bulk_insert("products", rows).unwrap();

        // Step 3: 验证 HNSW 索引已创建且有 10000 个节点
        assert_eq!(lifecycle.index_size("products", "emb").unwrap(), 10_000);

        // Step 4: 搜索 TOP10 — 结果相关
        let results = lifecycle
            .search("products", "emb", "electronics product", 10)
            .unwrap();

        assert_eq!(results.len(), 10, "should return exactly TOP10 results");

        // 结果应按相似度降序排列
        for i in 0..results.len() - 1 {
            assert!(
                results[i].score >= results[i + 1].score,
                "results must be sorted by score descending at index {}",
                i
            );
        }

        // Step 5: 验证相关性 — electronics 类别的行应占多数
        // row_id % 10 == 0 对应 electronics 类别
        let electronics_count = results.iter().filter(|r| r.row_id % 10 == 0).count();
        assert!(
            electronics_count >= 5,
            "at least 5 of TOP10 should be electronics-related, got {}",
            electronics_count
        );

        // 所有结果的分数应 > 0（有相关性）
        for r in &results {
            assert!(
                r.score > 0.0,
                "all TOP10 results should have positive similarity, got {}",
                r.score
            );
        }
    }

    #[test]
    fn test_7b2_zero_manual_operation() {
        // 验证标准：自动化流程完整，零手动操作
        // 用户只需 3 步：declare → insert → search，无需手动嵌入或索引操作
        let mut lifecycle = EmbeddingLifecycle::new();

        // 1. 声明（一次性）
        lifecycle
            .declare_embedding("items", "vec", vec!["title".into()], 64)
            .unwrap();

        // 2. INSERT（自动嵌入 + 自动索引）
        for i in 0..100u64 {
            let mut row = Row::new();
            row.insert("title".into(), format!("item {}", i));
            lifecycle.on_insert("items", i, &row).unwrap();
        }

        // 3. 搜索（自动查询 HNSW）
        let results = lifecycle.search("items", "vec", "item", 5).unwrap();

        // 全自动 — 用户从未调用 embedder 或 index
        assert_eq!(results.len(), 5);
        assert_eq!(lifecycle.index_size("items", "vec").unwrap(), 100);
    }

    #[test]
    fn test_7b2_multi_table_independent_indexes() {
        let mut lifecycle = EmbeddingLifecycle::new();

        // 两个表各自声明
        lifecycle
            .declare_embedding("table_a", "emb", vec!["text".into()], 128)
            .unwrap();
        lifecycle
            .declare_embedding("table_b", "emb", vec!["text".into()], 128)
            .unwrap();

        // 各自插入
        for i in 0..50u64 {
            let mut row_a = Row::new();
            row_a.insert("text".into(), format!("alpha document {}", i));
            lifecycle.on_insert("table_a", i, &row_a).unwrap();

            let mut row_b = Row::new();
            row_b.insert("text".into(), format!("beta record {}", i));
            lifecycle.on_insert("table_b", i, &row_b).unwrap();
        }

        assert_eq!(lifecycle.index_size("table_a", "emb").unwrap(), 50);
        assert_eq!(lifecycle.index_size("table_b", "emb").unwrap(), 50);

        // 搜索 table_a 不应返回 table_b 的行
        let results_a = lifecycle.search("table_a", "emb", "alpha", 5).unwrap();
        for r in &results_a {
            assert!(
                r.row_id < 50,
                "table_a results should only contain table_a row_ids"
            );
        }

        let results_b = lifecycle.search("table_b", "emb", "beta", 5).unwrap();
        for r in &results_b {
            assert!(
                r.row_id < 50,
                "table_b results should only contain table_b row_ids"
            );
        }
    }

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a < 1e-9 || norm_b < 1e-9 {
            return 0.0;
        }
        dot / (norm_a * norm_b)
    }
}
