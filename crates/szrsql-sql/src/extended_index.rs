//! 扩展索引 — Phase 6.17
//!
//! 提供四种扩展索引类型，超越 Phase 3.5 的 `InMemoryBTreeIndex`（仅支持 i64 等值/范围查询）：
//!
//! - **`GinIndex`**：GIN（Generalized Inverted Index）倒排索引，加速 `TsVector` 全文检索（`@@` 操作符）
//! - **`RTreeIndex`**：R-Tree 空间索引，支持 2D 点的空间范围查询与 k-最近邻（k-NN）搜索
//! - **`GistIndex`**：GiST（Generalized Search Tree）框架，支持 2D 点的 k-NN 搜索（欧几里得距离）
//! - **`Fts5Index`**：FTS5 风格全文检索索引，对 `Text` 列分词建倒排索引，支持 `MATCH` 查询
//!
//! # 设计
//!
//! - 所有索引类型为具体 struct + 固有方法（与 `InMemoryBTreeIndex` 模式一致）
//! - 空间点以 `(f64, f64)` 表示，从 `Value::Array(vec![Float64(x), Float64(y)])` 提取
//! - GIN 索引复用 `TsVector` 的词素（lexemes）构建倒排索引
//! - FTS5 索引自带分词器（空白 + 标点分割，小写化）
//! - R-Tree 采用批量构建（bulk-load）：按 x 排序 → 分组叶节点 → 递归构建内部节点
//!
//! # 与 PG 的关系
//!
//! - PG GIN 索引支持 `tsvector @@ tsquery`、`jsonb @> jsonb`、`array @> array`
//! - PG GiST 索引支持 k-NN（`<->` 距离操作符）、范围重叠（`&&`）
//! - PG R-Tree 已被 GiST 取代（pg_legacy），但概念相同
//! - SQLite FTS5 支持 `MATCH` 操作符与 `rank` 函数
//!
//! # 限制
//!
//! - **无 DML 自动维护**：索引构建后不会随 INSERT/UPDATE/DELETE 自动更新（需手动重建）
//! - **R-Tree 为批量构建**：不支持动态插入（需 rebuild_from_table 重建）
//! - **GiST k-NN 为线性扫描**：未实现 GiST 树结构，但提供正确的 k-NN 结果
//! - **FTS5 分词器为简化版**：仅按空白/标点分割 + 小写化，无词干提取/停用词

use crate::executor::ExecutionError;
use crate::executor::TableStorage;
use std::collections::HashMap;
use szrsql_types::value::{TsQuery, Value};

// =====================================================================
//  索引方法枚举
// =====================================================================

/// 索引访问方法（USING 子句）
///
/// 对应 PG 的 `CREATE INDEX ... USING <method>` 语法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexMethod {
    /// B-Tree（默认，Phase 3.5 已实现 `InMemoryBTreeIndex`）
    BTree,
    /// GiST — 广义搜索树（k-NN、范围重叠）
    GiST,
    /// GIN — 广义倒排索引（tsvector、jsonb、array）
    Gin,
    /// R-Tree — 空间索引（R*Tree 变体）
    RTree,
    /// FTS5 — SQLite 风格全文检索
    Fts5,
}

impl IndexMethod {
    /// 从字符串解析索引方法（大小写不敏感）
    pub fn from_str_ci(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "btree" => Some(Self::BTree),
            "gist" => Some(Self::GiST),
            "gin" => Some(Self::Gin),
            "rtree" => Some(Self::RTree),
            "fts5" => Some(Self::Fts5),
            _ => None,
        }
    }

    /// 返回字符串表示（大写）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BTree => "BTREE",
            Self::GiST => "GIST",
            Self::Gin => "GIN",
            Self::RTree => "RTREE",
            Self::Fts5 => "FTS5",
        }
    }
}

impl std::fmt::Display for IndexMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =====================================================================
//  2D 点辅助类型
// =====================================================================

/// 2D 点（f64, f64）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point2D {
    pub x: f64,
    pub y: f64,
}

impl Point2D {
    /// 创建新点
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// 计算到另一点的欧几里得距离
    pub fn distance_to(&self, other: &Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }

    /// 从 Value::Array 提取 2D 点
    ///
    /// 接受 `Value::Array(vec![Value::Float64(x), Value::Float64(y)])` 或
    /// `Value::Array(vec![Value::Int64(x), Value::Int64(y)])`。
    pub fn from_value(value: &Value) -> Result<Self, ExecutionError> {
        match value {
            Value::Array(elems) if elems.len() >= 2 => {
                let x = value_to_f64(&elems[0]).ok_or_else(|| {
                    ExecutionError::InvalidArgument(format!(
                        "point x coordinate must be numeric, got {:?}",
                        elems[0]
                    ))
                })?;
                let y = value_to_f64(&elems[1]).ok_or_else(|| {
                    ExecutionError::InvalidArgument(format!(
                        "point y coordinate must be numeric, got {:?}",
                        elems[1]
                    ))
                })?;
                Ok(Self::new(x, y))
            }
            other => Err(ExecutionError::InvalidArgument(format!(
                "expected Array([Float64, Float64]) for point, got {:?}",
                other
            ))),
        }
    }
}

/// 将 Value 转换为 f64（支持 Int64 和 Float64）
fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Float64(f) => Some(*f),
        Value::Int64(i) => Some(*i as f64),
        Value::Decimal(unscaled, scale) => {
            let scale_factor = 10f64.powi(*scale as i32);
            Some(*unscaled as f64 / scale_factor)
        }
        _ => None,
    }
}

// =====================================================================
//  边界框
// =====================================================================

/// 2D 边界框（AABB — Axis-Aligned Bounding Box）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl BoundingBox {
    /// 创建空边界框（无效：min > max）
    pub fn empty() -> Self {
        Self {
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        }
    }

    /// 从单个点创建
    pub fn from_point(p: &Point2D) -> Self {
        Self {
            min_x: p.x,
            min_y: p.y,
            max_x: p.x,
            max_y: p.y,
        }
    }

    /// 扩展以包含另一个边界框
    pub fn merge(&mut self, other: &Self) {
        self.min_x = self.min_x.min(other.min_x);
        self.min_y = self.min_y.min(other.min_y);
        self.max_x = self.max_x.max(other.max_x);
        self.max_y = self.max_y.max(other.max_y);
    }

    /// 扩展以包含一个点
    pub fn merge_point(&mut self, p: &Point2D) {
        self.min_x = self.min_x.min(p.x);
        self.min_y = self.min_y.min(p.y);
        self.max_x = self.max_x.max(p.x);
        self.max_y = self.max_y.max(p.y);
    }

    /// 检查是否包含一个点
    pub fn contains_point(&self, p: &Point2D) -> bool {
        p.x >= self.min_x && p.x <= self.max_x && p.y >= self.min_y && p.y <= self.max_y
    }

    /// 检查是否与另一个边界框相交
    pub fn intersects(&self, other: &Self) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_y <= other.max_y
            && self.max_y >= other.min_y
    }

    /// 计算面积
    pub fn area(&self) -> f64 {
        if self.max_x < self.min_x || self.max_y < self.min_y {
            return 0.0;
        }
        (self.max_x - self.min_x) * (self.max_y - self.min_y)
    }

    /// 计算到点的最小距离（用于 k-NN 剪枝）
    pub fn min_distance_to_point(&self, p: &Point2D) -> f64 {
        let dx = if p.x < self.min_x {
            self.min_x - p.x
        } else if p.x > self.max_x {
            p.x - self.max_x
        } else {
            0.0
        };
        let dy = if p.y < self.min_y {
            self.min_y - p.y
        } else if p.y > self.max_y {
            p.y - self.max_y
        } else {
            0.0
        };
        (dx * dx + dy * dy).sqrt()
    }

    /// 是否为空（无数据）
    pub fn is_empty(&self) -> bool {
        self.min_x > self.max_x || self.min_y > self.max_y
    }
}

// =====================================================================
//  GIN 倒排索引 — 用于 TsVector 全文检索
// =====================================================================

/// GIN 倒排索引 — 加速 `tsvector @@ tsquery` 查询
///
/// # 原理
///
/// GIN（Generalized Inverted Index）将每个词素（lexeme）映射到包含该词素的行 ID 列表。
/// 查询时，从 tsquery 提取词素，在倒排索引中查找，合并结果。
///
/// # 复杂度
///
/// - 构建：O(n × m)，n = 行数，m = 平均词素数/行
/// - 查询单词素：O(1) HashMap 查找 + O(k) 结果合并，k = 匹配行数
/// - 查询 AND：O(k1 + k2) 交集
/// - 查询 OR：O(k1 + k2) 并集
pub struct GinIndex {
    /// 索引名
    name: String,
    /// 所属表名
    table_name: String,
    /// 索引列名
    column: String,
    /// 倒排索引：词素 → 行 ID 列表（升序、去重）
    inverted: HashMap<String, Vec<usize>>,
}

impl GinIndex {
    /// 创建空 GIN 索引
    pub fn new(
        name: impl Into<String>,
        table_name: impl Into<String>,
        column: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            table_name: table_name.into(),
            column: column.into(),
            inverted: HashMap::new(),
        }
    }

    /// 索引名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 所属表名
    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    /// 索引列名
    pub fn column(&self) -> &str {
        &self.column
    }

    /// 插入一条索引项（词素 → 行 ID）
    pub fn insert(&mut self, term: &str, row_id: usize) {
        let entry = self.inverted.entry(term.to_lowercase()).or_default();
        if !entry.contains(&row_id) {
            entry.push(row_id);
            entry.sort_unstable();
        }
    }

    /// 批量构建：从表数据的 TsVector 列提取词素
    pub fn build_from_table(
        &mut self,
        table: &dyn TableStorage,
        column_idx: usize,
    ) -> Result<usize, ExecutionError> {
        let mut count = 0;
        for (row_id, row) in table.scan_with_ids() {
            let value = row.get(column_idx).ok_or_else(|| {
                ExecutionError::InvalidArgument(format!(
                    "column index {} out of bounds (row {} has {} columns)",
                    column_idx,
                    row_id,
                    row.len()
                ))
            })?;
            match value {
                Value::TsVector(tsvec) => {
                    for lexeme in &tsvec.lexemes {
                        self.insert(&lexeme.term, row_id);
                    }
                    count += 1;
                }
                Value::Null => {}
                other => {
                    return Err(ExecutionError::UnsupportedIndexKeyType(format!(
                        "GIN index column `{}` value {:?} is not TsVector",
                        self.column, other
                    )));
                }
            }
        }
        Ok(count)
    }

    /// 查询单个词素 → 匹配的行 ID 列表
    pub fn lookup_term(&self, term: &str) -> Vec<usize> {
        self.inverted
            .get(&term.to_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    /// 执行 tsquery 查询 → 匹配的行 ID 列表
    ///
    /// 支持 Lexeme / And / Or / Not / FollowedBy 语义。
    pub fn search_tsquery(&self, query: &TsQuery) -> Vec<usize> {
        match query {
            TsQuery::Empty => Vec::new(),
            TsQuery::Lexeme { term, .. } => self.lookup_term(term),
            TsQuery::And(left, right) => {
                let left_ids = self.search_tsquery(left);
                let right_ids = self.search_tsquery(right);
                intersect_sorted(&left_ids, &right_ids)
            }
            TsQuery::Or(left, right) => {
                let left_ids = self.search_tsquery(left);
                let right_ids = self.search_tsquery(right);
                union_sorted(&left_ids, &right_ids)
            }
            TsQuery::Not(inner) => {
                // NOT：返回所有不在 inner 中的行 ID
                let exclude_ids = self.search_tsquery(inner);
                let all_ids = self.all_row_ids();
                all_ids
                    .into_iter()
                    .filter(|id| !exclude_ids.binary_search(id).is_ok())
                    .collect()
            }
            TsQuery::FollowedBy { left, right, .. } => {
                // FOLLOWED BY：简化为 AND（不检查位置相邻性，因为 GIN 索引不存储位置）
                let left_ids = self.search_tsquery(left);
                let right_ids = self.search_tsquery(right);
                intersect_sorted(&left_ids, &right_ids)
            }
        }
    }

    /// 返回所有行 ID（去重、升序）
    pub fn all_row_ids(&self) -> Vec<usize> {
        let mut all: Vec<usize> = self
            .inverted
            .values()
            .flat_map(|ids| ids.iter().copied())
            .collect();
        all.sort_unstable();
        all.dedup();
        all
    }

    /// 索引中不同词素数
    pub fn len(&self) -> usize {
        self.inverted.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.inverted.is_empty()
    }
}

/// 两个升序向量的交集
fn intersect_sorted(a: &[usize], b: &[usize]) -> Vec<usize> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                result.push(a[i]);
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    result
}

/// 两个升序向量的并集
fn union_sorted(a: &[usize], b: &[usize]) -> Vec<usize> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                result.push(a[i]);
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => {
                result.push(a[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                result.push(b[j]);
                j += 1;
            }
        }
    }
    while i < a.len() {
        result.push(a[i]);
        i += 1;
    }
    while j < b.len() {
        result.push(b[j]);
        j += 1;
    }
    result
}

// =====================================================================
//  R-Tree 空间索引 — 批量构建
// =====================================================================

/// R-Tree 叶节点条目
#[derive(Debug, Clone)]
struct RTreeLeafEntry {
    point: Point2D,
    row_id: usize,
}

/// R-Tree 节点
#[derive(Debug, Clone)]
enum RTreeNode {
    /// 叶节点（包含点 + 行 ID）
    Leaf {
        bbox: BoundingBox,
        entries: Vec<RTreeLeafEntry>,
    },
    /// 内部节点（包含子节点）
    Internal {
        bbox: BoundingBox,
        children: Vec<RTreeNode>,
    },
}

impl RTreeNode {
    /// 获取节点的边界框
    fn bbox(&self) -> &BoundingBox {
        match self {
            Self::Leaf { bbox, .. } | Self::Internal { bbox, .. } => bbox,
        }
    }

    /// 是否为叶节点
    fn is_leaf(&self) -> bool {
        matches!(self, Self::Leaf { .. })
    }
}

/// R-Tree 空间索引 — 支持 2D 点的范围查询与 k-NN 搜索
///
/// # 构建
///
/// 批量构建（bulk-load）：按 x 坐标排序 → 分组为叶节点（每 LEAF_CAPACITY 个点）→
/// 递归构建内部节点（每 INTERNAL_FANOUT 个子节点）。
///
/// # 复杂度
///
/// - 构建：O(n log n)（排序 + 树构建）
/// - 范围查询：O(n) 最坏，实际为 O(结果数 + 访问节点数)（边界框剪枝）
/// - k-NN：O(n) 最坏，实际通过优先队列 + 距离剪枝优化
pub struct RTreeIndex {
    /// 索引名
    name: String,
    /// 所属表名
    table_name: String,
    /// 索引列名
    column: String,
    /// 根节点
    root: Option<RTreeNode>,
    /// 总点数
    point_count: usize,
}

/// R-Tree 叶节点容量
const RTREE_LEAF_CAPACITY: usize = 8;

/// R-Tree 内部节点扇出
const RTREE_INTERNAL_FANOUT: usize = 8;

impl RTreeIndex {
    /// 创建空 R-Tree 索引
    pub fn new(
        name: impl Into<String>,
        table_name: impl Into<String>,
        column: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            table_name: table_name.into(),
            column: column.into(),
            root: None,
            point_count: 0,
        }
    }

    /// 索引名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 所属表名
    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    /// 索引列名
    pub fn column(&self) -> &str {
        &self.column
    }

    /// 批量构建：从表数据的 2D 点列构建 R-Tree
    pub fn build_from_table(
        &mut self,
        table: &dyn TableStorage,
        column_idx: usize,
    ) -> Result<usize, ExecutionError> {
        let mut entries: Vec<RTreeLeafEntry> = Vec::new();
        for (row_id, row) in table.scan_with_ids() {
            let value = row.get(column_idx).ok_or_else(|| {
                ExecutionError::InvalidArgument(format!(
                    "column index {} out of bounds (row {} has {} columns)",
                    column_idx,
                    row_id,
                    row.len()
                ))
            })?;
            match value {
                Value::Null => {}
                _ => {
                    let point = Point2D::from_value(value)?;
                    entries.push(RTreeLeafEntry { point, row_id });
                }
            }
        }
        self.point_count = entries.len();
        if entries.is_empty() {
            self.root = None;
            return Ok(0);
        }
        // 按 x 坐标排序（STR — Sort-Tile-Recursive 批量加载）
        entries.sort_by(|a, b| {
            a.point
                .x
                .partial_cmp(&b.point.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.root = Some(Self::build_node(entries));
        Ok(self.point_count)
    }

    /// 递归构建 R-Tree 节点
    fn build_node(entries: Vec<RTreeLeafEntry>) -> RTreeNode {
        if entries.len() <= RTREE_LEAF_CAPACITY {
            // 创建叶节点
            let mut bbox = BoundingBox::empty();
            for e in &entries {
                bbox.merge_point(&e.point);
            }
            return RTreeNode::Leaf { bbox, entries };
        }

        // 分组为叶节点
        let chunk_size = RTREE_LEAF_CAPACITY;
        let mut leaf_nodes: Vec<RTreeNode> = entries
            .chunks(chunk_size)
            .map(|chunk| {
                let mut bbox = BoundingBox::empty();
                for e in chunk {
                    bbox.merge_point(&e.point);
                }
                RTreeNode::Leaf {
                    bbox,
                    entries: chunk.to_vec(),
                }
            })
            .collect();

        // 递归构建内部节点直到根
        while leaf_nodes.len() > RTREE_INTERNAL_FANOUT {
            let mut next_level: Vec<RTreeNode> = Vec::new();
            for chunk in leaf_nodes.chunks(RTREE_INTERNAL_FANOUT) {
                let mut bbox = BoundingBox::empty();
                for child in chunk {
                    bbox.merge(child.bbox());
                }
                next_level.push(RTreeNode::Internal {
                    bbox,
                    children: chunk.to_vec(),
                });
            }
            leaf_nodes = next_level;
        }

        // 创建根节点
        let mut bbox = BoundingBox::empty();
        for child in &leaf_nodes {
            bbox.merge(child.bbox());
        }
        RTreeNode::Internal {
            bbox,
            children: leaf_nodes,
        }
    }

    /// 范围查询：返回边界框内的所有行 ID
    pub fn range_query(&self, query_bbox: &BoundingBox) -> Vec<usize> {
        match &self.root {
            None => Vec::new(),
            Some(root) => {
                let mut result = Vec::new();
                Self::range_query_recursive(root, query_bbox, &mut result);
                result.sort_unstable();
                result
            }
        }
    }

    fn range_query_recursive(node: &RTreeNode, query_bbox: &BoundingBox, result: &mut Vec<usize>) {
        let node_bbox = node.bbox();
        if !node_bbox.intersects(query_bbox) {
            return; // 剪枝
        }
        match node {
            RTreeNode::Leaf { entries, .. } => {
                for e in entries {
                    if query_bbox.contains_point(&e.point) {
                        result.push(e.row_id);
                    }
                }
            }
            RTreeNode::Internal { children, .. } => {
                for child in children {
                    Self::range_query_recursive(child, query_bbox, result);
                }
            }
        }
    }

    /// k-最近邻查询：返回距离查询点最近的 k 个行 ID（按距离升序）
    pub fn knn_query(&self, query_point: &Point2D, k: usize) -> Vec<(usize, f64)> {
        match &self.root {
            None => Vec::new(),
            Some(root) => {
                let mut results: Vec<(usize, f64)> = Vec::new();
                Self::knn_recursive(root, query_point, k, &mut results);
                results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                results.truncate(k);
                results
            }
        }
    }

    fn knn_recursive(
        node: &RTreeNode,
        query_point: &Point2D,
        k: usize,
        results: &mut Vec<(usize, f64)>,
    ) {
        match node {
            RTreeNode::Leaf { entries, .. } => {
                for e in entries {
                    let dist = query_point.distance_to(&e.point);
                    results.push((e.row_id, dist));
                }
            }
            RTreeNode::Internal { children, .. } => {
                // 按边界框到查询点的最小距离排序子节点（best-first 遍历）
                let mut sorted_children: Vec<&RTreeNode> = children.iter().collect();
                sorted_children.sort_by(|a, b| {
                    let dist_a = a.bbox().min_distance_to_point(query_point);
                    let dist_b = b.bbox().min_distance_to_point(query_point);
                    dist_a
                        .partial_cmp(&dist_b)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

                for child in sorted_children {
                    // 剪枝：如果已有 k 个结果且当前节点最小距离 >= 第 k 近的距离
                    if results.len() >= k {
                        let kth_dist = {
                            let mut dists: Vec<f64> = results.iter().map(|(_, d)| *d).collect();
                            dists.sort_by(|a, b| {
                                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                            });
                            dists[k - 1]
                        };
                        if child.bbox().min_distance_to_point(query_point) >= kth_dist {
                            continue;
                        }
                    }
                    Self::knn_recursive(child, query_point, k, results);
                }
            }
        }
    }

    /// 总点数
    pub fn len(&self) -> usize {
        self.point_count
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.point_count == 0
    }

    /// 返回整个索引的边界框
    pub fn bounds(&self) -> Option<BoundingBox> {
        self.root.as_ref().map(|r| *r.bbox())
    }
}

// =====================================================================
//  GiST k-NN 索引 — 用于 2D 点的最近邻搜索
// =====================================================================

/// GiST 索引 — 广义搜索树框架实现
///
/// 本实现利用 R-Tree 结构进行 k-NN 搜索，模拟 GiST 的 `<->` 距离操作符。
/// 与 RTreeIndex 的区别：GiST 强调通用框架（可扩展距离函数），RTreeIndex 强调空间范围查询。
pub struct GistIndex {
    /// 索引名
    name: String,
    /// 所属表名
    table_name: String,
    /// 索引列名
    column: String,
    /// 内部使用 R-Tree 结构
    rtree: RTreeIndex,
}

impl GistIndex {
    /// 创建空 GiST 索引
    pub fn new(
        name: impl Into<String>,
        table_name: impl Into<String>,
        column: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            table_name: table_name.into(),
            column: column.into(),
            rtree: RTreeIndex::new("", "", ""),
        }
    }

    /// 索引名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 所属表名
    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    /// 索引列名
    pub fn column(&self) -> &str {
        &self.column
    }

    /// 批量构建：从表数据的 2D 点列构建 GiST 索引
    pub fn build_from_table(
        &mut self,
        table: &dyn TableStorage,
        column_idx: usize,
    ) -> Result<usize, ExecutionError> {
        self.rtree = RTreeIndex::new(&self.name, &self.table_name, &self.column);
        self.rtree.build_from_table(table, column_idx)
    }

    /// k-最近邻查询（`<->` 距离操作符）
    ///
    /// 返回 (row_id, distance) 元组列表，按距离升序。
    pub fn knn(&self, query_point: &Point2D, k: usize) -> Vec<(usize, f64)> {
        self.rtree.knn_query(query_point, k)
    }

    /// 范围查询（GiST 也支持范围查询）
    pub fn range_query(&self, query_bbox: &BoundingBox) -> Vec<usize> {
        self.rtree.range_query(query_bbox)
    }

    /// 总点数
    pub fn len(&self) -> usize {
        self.rtree.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.rtree.is_empty()
    }
}

// =====================================================================
//  FTS5 全文检索索引 — 用于 Text 列的 MATCH 查询
// =====================================================================

/// FTS5 倒排索引条目（词素 → 行 ID + 位置列表）
#[derive(Debug, Clone)]
struct Fts5Posting {
    /// 行 ID
    row_id: usize,
    /// 该词素在行中的位置列表（从 0 开始）
    positions: Vec<usize>,
}

/// FTS5 全文检索索引 — SQLite 风格
///
/// # 原理
///
/// 对 Text 列进行分词（空白/标点分割 + 小写化），构建倒排索引：
/// term → Vec<Fts5Posting>（每个 posting 含行 ID + 位置列表）
///
/// 支持：
/// - `MATCH 'word'`：单词素查询
/// - `MATCH 'word1 word2'`：多词素查询（AND 语义）
/// - `MATCH '"phrase"'`：短语查询（位置相邻）
///
/// # 复杂度
///
/// - 构建：O(n × m)，n = 行数，m = 平均词数/行
/// - 查询单词素：O(1) HashMap 查找 + O(k) 结果，k = 匹配行数
/// - 短语查询：O(k × p)，p = 平均位置数/行
pub struct Fts5Index {
    /// 索引名
    name: String,
    /// 所属表名
    table_name: String,
    /// 索引列名
    column: String,
    /// 倒排索引：词素 → posting 列表
    postings: HashMap<String, Vec<Fts5Posting>>,
}

impl Fts5Index {
    /// 创建空 FTS5 索引
    pub fn new(
        name: impl Into<String>,
        table_name: impl Into<String>,
        column: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            table_name: table_name.into(),
            column: column.into(),
            postings: HashMap::new(),
        }
    }

    /// 索引名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 所属表名
    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    /// 索引列名
    pub fn column(&self) -> &str {
        &self.column
    }

    /// 分词器：将文本分割为词素列表（小写化，去除标点）
    ///
    /// 规则：
    /// - 按空白字符（空格/制表符/换行）分割
    /// - 去除首尾标点
    /// - 小写化
    pub fn tokenize(text: &str) -> Vec<(String, usize)> {
        text.split_whitespace()
            .enumerate()
            .filter_map(|(pos, word)| {
                let cleaned: String = word
                    .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                    .to_lowercase();
                if cleaned.is_empty() {
                    None
                } else {
                    Some((cleaned, pos))
                }
            })
            .collect()
    }

    /// 插入一行文本
    pub fn insert_text(&mut self, text: &str, row_id: usize) {
        let tokens = Self::tokenize(text);
        for (term, position) in tokens {
            let posting_list = self.postings.entry(term).or_default();
            if let Some(existing) = posting_list.iter_mut().find(|p| p.row_id == row_id) {
                existing.positions.push(position);
            } else {
                posting_list.push(Fts5Posting {
                    row_id,
                    positions: vec![position],
                });
            }
        }
    }

    /// 批量构建：从表数据的 Text 列分词建索引
    pub fn build_from_table(
        &mut self,
        table: &dyn TableStorage,
        column_idx: usize,
    ) -> Result<usize, ExecutionError> {
        let mut count = 0;
        for (row_id, row) in table.scan_with_ids() {
            let value = row.get(column_idx).ok_or_else(|| {
                ExecutionError::InvalidArgument(format!(
                    "column index {} out of bounds (row {} has {} columns)",
                    column_idx,
                    row_id,
                    row.len()
                ))
            })?;
            match value {
                Value::Text(text) => {
                    self.insert_text(text, row_id);
                    count += 1;
                }
                Value::Null => {}
                other => {
                    return Err(ExecutionError::UnsupportedIndexKeyType(format!(
                        "FTS5 index column `{}` value {:?} is not Text",
                        self.column, other
                    )));
                }
            }
        }
        Ok(count)
    }

    /// MATCH 查询：返回匹配的行 ID 列表
    ///
    /// 查询语法：
    /// - `word`：单词素查询
    /// - `word1 word2`：多词素查询（AND 语义 — 所有词素都需出现）
    /// - `"phrase"`：短语查询（位置相邻）
    pub fn match_query(&self, query: &str) -> Vec<usize> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }

        // 短语查询（双引号包裹）
        if query.starts_with('"') && query.ends_with('"') && query.len() >= 2 {
            let phrase = &query[1..query.len() - 1];
            return self.phrase_query(phrase);
        }

        // 多词素查询（AND 语义）
        let terms = query.split_whitespace().collect::<Vec<_>>();
        if terms.is_empty() {
            return Vec::new();
        }

        let mut result_sets: Vec<Vec<usize>> = Vec::new();
        for term in &terms {
            let cleaned = term
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_lowercase();
            if cleaned.is_empty() {
                continue;
            }
            let row_ids: Vec<usize> = self
                .postings
                .get(&cleaned)
                .map(|plist| plist.iter().map(|p| p.row_id).collect())
                .unwrap_or_default();
            result_sets.push(row_ids);
        }

        if result_sets.is_empty() {
            return Vec::new();
        }

        // 交集（AND 语义）
        let mut result = result_sets.remove(0);
        for rs in result_sets {
            result = intersect_sorted(&result, &rs);
        }
        result.sort_unstable();
        result
    }

    /// 短语查询：词素序列在原文中位置相邻
    fn phrase_query(&self, phrase: &str) -> Vec<usize> {
        let tokens = Self::tokenize(phrase);
        if tokens.is_empty() {
            return Vec::new();
        }

        // 找到包含所有词素的行
        let mut candidate_sets: Vec<Vec<usize>> = Vec::new();
        for (term, _) in &tokens {
            let row_ids: Vec<usize> = self
                .postings
                .get(term)
                .map(|plist| plist.iter().map(|p| p.row_id).collect())
                .unwrap_or_default();
            candidate_sets.push(row_ids);
        }

        let mut candidates = candidate_sets.remove(0);
        for cs in candidate_sets {
            candidates = intersect_sorted(&candidates, &cs);
        }

        // 检查位置相邻性
        let mut result = Vec::new();
        for row_id in candidates {
            if self.check_phrase_positions(row_id, &tokens) {
                result.push(row_id);
            }
        }
        result.sort_unstable();
        result
    }

    /// 检查短语在指定行中位置是否相邻
    fn check_phrase_positions(&self, row_id: usize, tokens: &[(String, usize)]) -> bool {
        // 获取每个词素在该行的位置列表
        let mut position_lists: Vec<Vec<usize>> = Vec::new();
        for (term, _) in tokens {
            let positions = self
                .postings
                .get(term)
                .and_then(|plist| plist.iter().find(|p| p.row_id == row_id))
                .map(|p| p.positions.clone())
                .unwrap_or_default();
            if positions.is_empty() {
                return false;
            }
            position_lists.push(positions);
        }

        // 检查是否存在 i, i+1, i+2, ... 的位置序列
        for &start_pos in &position_lists[0] {
            let mut found = true;
            for (idx, positions) in position_lists.iter().enumerate().skip(1) {
                let expected = start_pos + idx;
                if !positions.contains(&expected) {
                    found = false;
                    break;
                }
            }
            if found {
                return true;
            }
        }
        false
    }

    /// 查询单个词素 → 匹配的行 ID 列表
    pub fn lookup_term(&self, term: &str) -> Vec<usize> {
        let cleaned = term
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .to_lowercase();
        self.postings
            .get(&cleaned)
            .map(|plist| {
                let mut ids: Vec<usize> = plist.iter().map(|p| p.row_id).collect();
                ids.sort_unstable();
                ids
            })
            .unwrap_or_default()
    }

    /// 索引中不同词素数
    pub fn len(&self) -> usize {
        self.postings.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.postings.is_empty()
    }

    /// 返回所有行 ID（去重、升序）
    pub fn all_row_ids(&self) -> Vec<usize> {
        let mut all: Vec<usize> = self
            .postings
            .values()
            .flat_map(|plist| plist.iter().map(|p| p.row_id))
            .collect();
        all.sort_unstable();
        all.dedup();
        all
    }
}
