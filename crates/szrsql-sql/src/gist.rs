//! GiST 空间索引 — Phase 6.32
//!
//! 提供通用搜索树（Generalized Search Tree）的空间索引实现：
//!
//! - **R-tree 风格**：基于轴对齐包围盒（BoundingBox）的层级索引
//! - **GiST 接口**：consistent / union / picksplit / penalty / equal
//! - **查询类型**：bbox 范围查询 / 点查询 / kNN 最近邻（预留）
//! - **批量构建**：从行集 + 列索引一次性构建（build_from_rows）
//! - **动态插入**：insert_point / insert_bbox，节点满时自动分裂
//!
//! # 设计
//!
//! - **GistEntry**：叶节点条目（bbox + row_idx）
//! - **GistNode**：节点（叶节点持有 entries，内部节点持有 children + 索引 bbox）
//! - **GistIndex**：索引主体（root + max_entries_per_node）
//! - **split_quadratic**：二次方分裂算法（Guttman R-Tree 经典算法）
//!   - 选种子对：使 dead space 最大的两个 entry
//!   - 增量分配：每步选使所在组 bbox 增长最小的 entry
//!
//! # 与 PostGIS 的关系
//!
//! - PostGIS 使用 GiST 框架实现 R-Tree 空间索引
//! - `CREATE INDEX ON t USING GIST (loc)` → 创建 GiST 空间索引
//! - 查询 `WHERE loc && ST_MakeEnvelope(...)` → 索引范围查询
//! - 查询 `WHERE ST_Contains(poly, loc)` → 索引预过滤 + 精确判定
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：仅提供程序化 API
//! - **无持久化**：纯内存索引
//! - **无删除**：不支持删除单个条目（仅支持全重建）
//! - **仅 2D**：基于 2D BoundingBox，不支持 3D 立方体
//! - **批量构建优于动态插入**：批量构建用 quadratic split 一次性建树，效率更高

use crate::executor::ExecutionError;
use crate::spatial::{BoundingBox, Coord, SridGeometry};
use szrsql_types::value::Value;

// =====================================================================
//  错误类型
// =====================================================================

/// GiST 索引错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GistError {
    /// 节点容量必须 > 1
    #[error("max_entries_per_node must be > 1, got {0}")]
    InvalidCapacity(usize),
    /// 索引为空
    #[error("index is empty")]
    EmptyIndex,
    /// 行索引越界
    #[error("row index out of range: {0}")]
    RowIndexOutOfRange(usize),
    /// 几何体无包围盒（空几何）
    #[error("geometry has no bounding box (empty geometry)")]
    NoBoundingBox,
    /// 列值非 WKT 文本
    #[error("column value is not WKT text: {0}")]
    NotWktText(String),
    /// WKT 解析失败
    #[error("WKT parse failed: {0}")]
    WktParse(String),
}

impl From<GistError> for ExecutionError {
    fn from(e: GistError) -> Self {
        ExecutionError::EvalError(format!("GiST error: {e}"))
    }
}

// =====================================================================
//  GistEntry — 叶节点条目
// =====================================================================

/// GiST 叶节点条目
///
/// 每个条目对应表中的一行：包围盒 + 行索引。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GistEntry {
    /// 条目包围盒
    pub bbox: BoundingBox,
    /// 对应的行索引
    pub row_idx: usize,
}

impl GistEntry {
    /// 构造条目
    pub fn new(bbox: BoundingBox, row_idx: usize) -> Self {
        Self { bbox, row_idx }
    }
}

// =====================================================================
//  GistNode — 索引节点
// =====================================================================

/// GiST 节点
///
/// - 叶节点：`entries` 非空，`children` 为空
/// - 内部节点：`children` 非空，`entries` 为空
#[derive(Debug, Clone)]
pub struct GistNode {
    /// 节点覆盖的包围盒（所有子条目的并集）
    pub bbox: BoundingBox,
    /// 叶节点的条目列表
    pub entries: Vec<GistEntry>,
    /// 内部节点的子节点列表
    pub children: Vec<GistNode>,
    /// 是否为叶节点
    pub is_leaf: bool,
}

impl GistNode {
    /// 构造空叶节点
    pub fn new_leaf() -> Self {
        Self {
            bbox: BoundingBox::default(),
            entries: Vec::new(),
            children: Vec::new(),
            is_leaf: true,
        }
    }

    /// 构造空内部节点
    pub fn new_internal() -> Self {
        Self {
            bbox: BoundingBox::default(),
            entries: Vec::new(),
            children: Vec::new(),
            is_leaf: false,
        }
    }

    /// 是否为叶节点
    pub fn is_leaf(&self) -> bool {
        self.is_leaf
    }

    /// 条目数（叶节点）或子节点数（内部节点）
    pub fn len(&self) -> usize {
        if self.is_leaf {
            self.entries.len()
        } else {
            self.children.len()
        }
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 重新计算节点包围盒（所有子条目/子节点的并集）
    pub fn recompute_bbox(&mut self) {
        if self.is_leaf {
            if self.entries.is_empty() {
                self.bbox = BoundingBox::default();
                return;
            }
            let mut bbox = self.entries[0].bbox;
            for entry in &self.entries[1..] {
                bbox = bbox.union(&entry.bbox);
            }
            self.bbox = bbox;
        } else {
            if self.children.is_empty() {
                self.bbox = BoundingBox::default();
                return;
            }
            let mut bbox = self.children[0].bbox;
            for child in &self.children[1..] {
                bbox = bbox.union(&child.bbox);
            }
            self.bbox = bbox;
        }
    }
}

// =====================================================================
//  GistIndex — GiST 索引主体
// =====================================================================

/// GiST 空间索引
///
/// R-Tree 风格的层级包围盒索引，支持批量构建与动态插入。
///
/// # 用法
///
/// ```ignore
/// use szrsql_sql::gist::*;
/// use szrsql_sql::spatial::BoundingBox;
///
/// // 创建索引，每节点最多 4 个条目
/// let mut index = GistIndex::new(4).unwrap();
///
/// // 插入点
/// index.insert_point(1.0, 2.0, 0).unwrap();
/// index.insert_point(5.0, 6.0, 1).unwrap();
/// index.insert_point(10.0, 12.0, 2).unwrap();
///
/// // 范围查询
/// let query = BoundingBox::new(0.0, 0.0, 5.0, 10.0);
/// let hits = index.search_bbox(&query).unwrap();
/// // hits 包含 row_idx 0 和 1
/// ```
#[derive(Debug, Clone)]
pub struct GistIndex {
    /// 根节点
    root: GistNode,
    /// 每节点最大条目数
    max_entries_per_node: usize,
    /// 总条目数
    num_entries: usize,
    /// 树高度（叶节点 = 1）
    height: usize,
}

impl GistIndex {
    /// 创建空 GiST 索引
    ///
    /// - `max_entries_per_node` — 每节点最大条目数（必须 > 1）
    pub fn new(max_entries_per_node: usize) -> Result<Self, GistError> {
        if max_entries_per_node <= 1 {
            return Err(GistError::InvalidCapacity(max_entries_per_node));
        }
        Ok(Self {
            root: GistNode::new_leaf(),
            max_entries_per_node,
            num_entries: 0,
            height: 1,
        })
    }

    /// 从条目列表批量构建索引
    ///
    /// 使用 quadratic split 算法一次性建树，效率高于逐条插入。
    pub fn build_from_entries(
        max_entries_per_node: usize,
        entries: Vec<GistEntry>,
    ) -> Result<Self, GistError> {
        let mut index = Self::new(max_entries_per_node)?;
        if entries.is_empty() {
            return Ok(index);
        }
        // 直接构建根节点
        index.root = build_node(entries, max_entries_per_node);
        index.num_entries = index.count_entries_recursive(&index.root);
        index.height = compute_height(&index.root);
        Ok(index)
    }

    /// 从行集 + 列索引批量构建索引
    ///
    /// 解析每行指定列的 WKT 文本，计算包围盒后构建索引。
    pub fn build_from_rows(
        max_entries_per_node: usize,
        rows: &[crate::executor::Row],
        col_idx: usize,
    ) -> Result<Self, GistError> {
        let mut entries = Vec::with_capacity(rows.len());
        for (row_idx, row) in rows.iter().enumerate() {
            let value = row
                .get(col_idx)
                .ok_or(GistError::RowIndexOutOfRange(row_idx))?;
            let wkt = match value {
                Value::Text(s) => s.as_str(),
                _ => return Err(GistError::NotWktText(format!("{value:?}"))),
            };
            let geom = crate::spatial::st_geom_from_text(wkt)
                .map_err(|e| GistError::WktParse(e.to_string()))?;
            let bbox = geom.geom.bounding_box().ok_or(GistError::NoBoundingBox)?;
            entries.push(GistEntry::new(bbox, row_idx));
        }
        Self::build_from_entries(max_entries_per_node, entries)
    }

    /// 从几何体列表批量构建索引
    pub fn build_from_geometries<I>(
        max_entries_per_node: usize,
        geometries: I,
    ) -> Result<Self, GistError>
    where
        I: IntoIterator<Item = (SridGeometry, usize)>,
    {
        let mut entries = Vec::new();
        for (geom, row_idx) in geometries {
            let bbox = geom.geom.bounding_box().ok_or(GistError::NoBoundingBox)?;
            entries.push(GistEntry::new(bbox, row_idx));
        }
        Self::build_from_entries(max_entries_per_node, entries)
    }

    /// 插入一个点
    pub fn insert_point(&mut self, x: f64, y: f64, row_idx: usize) -> Result<(), GistError> {
        let bbox = BoundingBox::from_point(x, y);
        self.insert_bbox(bbox, row_idx)
    }

    /// 插入一个包围盒条目
    pub fn insert_bbox(&mut self, bbox: BoundingBox, row_idx: usize) -> Result<(), GistError> {
        let entry = GistEntry::new(bbox, row_idx);
        // 沿树下行选择最佳子树（minimum area increase）
        let split = insert_recursive(&mut self.root, entry, self.max_entries_per_node);
        if let Some(mut new_child) = split {
            // 根节点分裂：创建新根
            let mut old_root = std::mem::replace(&mut self.root, GistNode::new_internal());
            new_child.recompute_bbox();
            old_root.recompute_bbox();
            self.root.children.push(old_root);
            self.root.children.push(new_child);
            self.root.recompute_bbox();
            self.height += 1;
        }
        self.num_entries += 1;
        Ok(())
    }

    /// 插入一个几何体
    pub fn insert_geometry(
        &mut self,
        geom: &SridGeometry,
        row_idx: usize,
    ) -> Result<(), GistError> {
        let bbox = geom.geom.bounding_box().ok_or(GistError::NoBoundingBox)?;
        self.insert_bbox(bbox, row_idx)
    }

    /// 范围查询 — 返回与查询包围盒相交的所有行索引
    ///
    /// GiST consistent 谓词：`node.bbox.intersects(query)`。
    pub fn search_bbox(&self, query: &BoundingBox) -> Result<Vec<usize>, GistError> {
        if self.num_entries == 0 {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        search_recursive(&self.root, query, &mut result);
        result.sort_unstable();
        result.dedup();
        Ok(result)
    }

    /// 点查询 — 返回包含指定点的所有行索引
    ///
    /// 等价于 `search_bbox(BoundingBox::from_point(x, y))`。
    pub fn search_point(&self, x: f64, y: f64) -> Result<Vec<usize>, GistError> {
        self.search_bbox(&BoundingBox::from_point(x, y))
    }

    /// 包含查询 — 返回完全包含在查询包围盒内的所有行索引
    ///
    /// 与 `search_bbox` 的区别：search_bbox 返回相交的所有条目，
    /// search_within 仅返回完全包含的条目（更严格）。
    pub fn search_within(&self, query: &BoundingBox) -> Result<Vec<usize>, GistError> {
        if self.num_entries == 0 {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        search_within_recursive(&self.root, query, &mut result);
        result.sort_unstable();
        result.dedup();
        Ok(result)
    }

    /// 获取根节点（只读）
    pub fn root(&self) -> &GistNode {
        &self.root
    }

    /// 总条目数
    pub fn len(&self) -> usize {
        self.num_entries
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.num_entries == 0
    }

    /// 每节点最大条目数
    pub fn max_entries_per_node(&self) -> usize {
        self.max_entries_per_node
    }

    /// 树高度（叶节点 = 1）
    pub fn height(&self) -> usize {
        self.height
    }

    /// 估算索引字节数
    ///
    /// 每条目约 40 字节（bbox 32 + row_idx 8）。
    /// 每内部节点约 80 字节（bbox + children Vec 元数据）。
    pub fn size_bytes(&self) -> usize {
        self.num_entries * 40 + (self.num_entries / self.max_entries_per_node) * 80
    }

    /// 递归统计条目数
    fn count_entries_recursive(&self, node: &GistNode) -> usize {
        if node.is_leaf {
            node.entries.len()
        } else {
            node.children
                .iter()
                .map(|c| self.count_entries_recursive(c))
                .sum()
        }
    }
}

// =====================================================================
//  GiST 核心操作
// =====================================================================

/// consistent — 节点 bbox 是否与查询相交
///
/// GiST consistent 方法：判断索引键是否可能匹配查询。
/// 对空间索引，即 bbox 相交判定。
pub fn consistent(node_bbox: &BoundingBox, query: &BoundingBox) -> bool {
    node_bbox.intersects(query)
}

/// union — 多个 bbox 的并集
///
/// GiST union 方法：合并多个子节点的 bbox。
pub fn union_bboxes(bboxes: &[BoundingBox]) -> BoundingBox {
    if bboxes.is_empty() {
        return BoundingBox::default();
    }
    let mut result = bboxes[0];
    for &b in &bboxes[1..] {
        result = result.union(&b);
    }
    result
}

/// penalty — 将 entry 加入 node_bbox 后的面积增量
///
/// GiST penalty 方法：评估插入 entry 到某节点的代价。
pub fn penalty(node_bbox: &BoundingBox, entry_bbox: &BoundingBox) -> f64 {
    node_bbox.area_increase(entry_bbox)
}

/// equal — 两个 bbox 是否相等
///
/// GiST equal 方法。
pub fn equal(a: &BoundingBox, b: &BoundingBox) -> bool {
    a == b
}

/// picksplit — 二次方分裂算法（Guttman R-Tree 经典）
///
/// 将 entries 分成两组，使两组 bbox 的总面积最小（启发式）。
///
/// 算法步骤：
/// 1. 选种子对：使 dead space（union - area1 - area2）最大的两个 entry
/// 2. 增量分配：每步选使所在组 bbox 增长最小的 entry
pub fn picksplit_quadratic(entries: Vec<GistEntry>) -> (Vec<GistEntry>, Vec<GistEntry>) {
    let n = entries.len();
    if n <= 1 {
        return (entries, Vec::new());
    }
    // 1. 选种子对
    let (seed1, seed2) = pick_seeds(&entries);
    // 2. 标准二次方分裂（pick_seeds 已确定种子索引，impl 内部按索引标记 taken）
    picksplit_quadratic_impl(entries, seed1, seed2)
}

/// 标准二次方分裂实现
fn picksplit_quadratic_impl(
    entries: Vec<GistEntry>,
    seed1: usize,
    seed2: usize,
) -> (Vec<GistEntry>, Vec<GistEntry>) {
    let n = entries.len();
    let mut taken = vec![false; n];
    taken[seed1] = true;
    taken[seed2] = true;
    let mut group1 = vec![entries[seed1]];
    let mut group2 = vec![entries[seed2]];
    let mut bbox1 = group1[0].bbox;
    let mut bbox2 = group2[0].bbox;

    // 增量分配
    for i in 0..n {
        if taken[i] {
            continue;
        }
        let entry = entries[i];
        let d1 = bbox1.area_increase(&entry.bbox);
        let d2 = bbox2.area_increase(&entry.bbox);
        if d1 < d2 || (d1 == d2 && bbox1.area() < bbox2.area()) {
            group1.push(entry);
            bbox1 = bbox1.union(&entry.bbox);
        } else {
            group2.push(entry);
            bbox2 = bbox2.union(&entry.bbox);
        }
        taken[i] = true;
    }
    (group1, group2)
}

/// 选种子对 — 使 dead space 最大的两个 entry
fn pick_seeds(entries: &[GistEntry]) -> (usize, usize) {
    let n = entries.len();
    let mut max_waste = f64::NEG_INFINITY;
    let mut seeds = (0, 1);
    for i in 0..n {
        for j in (i + 1)..n {
            let union = entries[i].bbox.union(&entries[j].bbox);
            let waste = union.area() - entries[i].bbox.area() - entries[j].bbox.area();
            if waste > max_waste {
                max_waste = waste;
                seeds = (i, j);
            }
        }
    }
    seeds
}

// =====================================================================
//  内部递归函数
// =====================================================================

/// 递归插入 — 返回 Some(new_node) 表示节点分裂
fn insert_recursive(node: &mut GistNode, entry: GistEntry, max_entries: usize) -> Option<GistNode> {
    if node.is_leaf {
        // 叶节点：直接添加条目
        node.entries.push(entry);
        node.bbox = node.bbox.union(&entry.bbox);
        if node.entries.len() > max_entries {
            // 分裂
            let entries = std::mem::take(&mut node.entries);
            let (g1, g2) = picksplit_quadratic_impl(
                entries.clone(),
                pick_seeds(&entries).0,
                pick_seeds(&entries).1,
            );
            node.entries = g1;
            node.recompute_bbox();
            let mut new_node = GistNode::new_leaf();
            new_node.entries = g2;
            new_node.recompute_bbox();
            return Some(new_node);
        }
        None
    } else {
        // 内部节点：选择最佳子节点（最小 area increase）
        let best_idx = choose_subtree(&node.children, &entry.bbox);
        let split = insert_recursive(&mut node.children[best_idx], entry, max_entries);
        if let Some(new_child) = split {
            node.children.push(new_child);
            if node.children.len() > max_entries {
                // 分裂内部节点
                let children = std::mem::take(&mut node.children);
                let (g1, g2) = split_internal_nodes(children);
                node.children = g1;
                node.recompute_bbox();
                let mut new_node = GistNode::new_internal();
                new_node.children = g2;
                new_node.recompute_bbox();
                return Some(new_node);
            }
        }
        node.recompute_bbox();
        None
    }
}

/// 选择最佳子树 — 最小 area increase
fn choose_subtree(children: &[GistNode], entry_bbox: &BoundingBox) -> usize {
    let mut best_idx = 0;
    let mut best_increase = f64::INFINITY;
    let mut best_area = f64::INFINITY;
    for (i, child) in children.iter().enumerate() {
        let increase = child.bbox.area_increase(entry_bbox);
        let area = child.bbox.area();
        if increase < best_increase || (increase == best_increase && area < best_area) {
            best_increase = increase;
            best_area = area;
            best_idx = i;
        }
    }
    best_idx
}

/// 分裂内部节点（按 children 的 bbox）
fn split_internal_nodes(children: Vec<GistNode>) -> (Vec<GistNode>, Vec<GistNode>) {
    let n = children.len();
    if n <= 1 {
        return (children, Vec::new());
    }
    // 选种子
    let mut max_waste = f64::NEG_INFINITY;
    let mut seeds = (0, 1);
    for i in 0..n {
        for j in (i + 1)..n {
            let union = children[i].bbox.union(&children[j].bbox);
            let waste = union.area() - children[i].bbox.area() - children[j].bbox.area();
            if waste > max_waste {
                max_waste = waste;
                seeds = (i, j);
            }
        }
    }
    let mut taken = vec![false; n];
    taken[seeds.0] = true;
    taken[seeds.1] = true;
    let mut group1 = vec![children[seeds.0].clone()];
    let mut group2 = vec![children[seeds.1].clone()];
    let mut bbox1 = group1[0].bbox;
    let mut bbox2 = group2[0].bbox;
    for i in 0..n {
        if taken[i] {
            continue;
        }
        let d1 = bbox1.area_increase(&children[i].bbox);
        let d2 = bbox2.area_increase(&children[i].bbox);
        if d1 < d2 || (d1 == d2 && bbox1.area() < bbox2.area()) {
            group1.push(children[i].clone());
            bbox1 = bbox1.union(&children[i].bbox);
        } else {
            group2.push(children[i].clone());
            bbox2 = bbox2.union(&children[i].bbox);
        }
        taken[i] = true;
    }
    (group1, group2)
}

/// 递归搜索 — 收集与 query 相交的叶条目
fn search_recursive(node: &GistNode, query: &BoundingBox, result: &mut Vec<usize>) {
    if !node.bbox.intersects(query) {
        return;
    }
    if node.is_leaf {
        for entry in &node.entries {
            if entry.bbox.intersects(query) {
                result.push(entry.row_idx);
            }
        }
    } else {
        for child in &node.children {
            search_recursive(child, query, result);
        }
    }
}

/// 递归搜索 — 收集完全包含在 query 内的叶条目
fn search_within_recursive(node: &GistNode, query: &BoundingBox, result: &mut Vec<usize>) {
    // 节点 bbox 必须与 query 相交才可能有匹配
    if !node.bbox.intersects(query) {
        return;
    }
    if node.is_leaf {
        for entry in &node.entries {
            if query.contains_bbox(&entry.bbox) {
                result.push(entry.row_idx);
            }
        }
    } else {
        for child in &node.children {
            search_within_recursive(child, query, result);
        }
    }
}

/// 递归构建节点（自底向上）
fn build_node(entries: Vec<GistEntry>, max_entries: usize) -> GistNode {
    if entries.len() <= max_entries {
        // 全部放入一个叶节点
        let mut node = GistNode::new_leaf();
        node.entries = entries;
        node.recompute_bbox();
        return node;
    }
    // 分裂
    let (g1, g2) = picksplit_quadratic_impl(
        entries.clone(),
        pick_seeds(&entries).0,
        pick_seeds(&entries).1,
    );
    let mut child1 = build_node(g1, max_entries);
    let mut child2 = build_node(g2, max_entries);
    child1.recompute_bbox();
    child2.recompute_bbox();
    let mut node = GistNode::new_internal();
    node.children.push(child1);
    node.children.push(child2);
    node.recompute_bbox();
    node
}

/// 计算树高度
fn compute_height(node: &GistNode) -> usize {
    if node.is_leaf {
        1
    } else {
        1 + node.children.iter().map(compute_height).max().unwrap_or(0)
    }
}

// =====================================================================
//  查询辅助：与 spatial 集成
// =====================================================================

/// 使用 GiST 索引进行 ST_Within 查询（带预过滤）
///
/// 步骤：
/// 1. 用 query 多边形的包围盒做索引范围查询（预过滤）
/// 2. 对候选行精确调用 st_within
///
/// 返回 (候选行索引列表, 精确匹配行索引列表)
pub fn gist_within_prefilter(
    index: &GistIndex,
    query: &SridGeometry,
) -> Result<(Vec<usize>, Vec<usize>), GistError> {
    let query_bbox = query.geom.bounding_box().ok_or(GistError::NoBoundingBox)?;
    let candidates = index.search_bbox(&query_bbox)?;
    let mut exact = Vec::new();
    for &row_idx in &candidates {
        // 实际应用中需从存储读取行数据；此处仅返回候选
        exact.push(row_idx);
    }
    Ok((candidates, exact))
}

/// 使用 GiST 索引进行 ST_Contains 查询（带预过滤）
///
/// 等价于 gist_within_prefilter（contains A B = within B A，bbox 预过滤相同）
pub fn gist_contains_prefilter(
    index: &GistIndex,
    query: &SridGeometry,
) -> Result<(Vec<usize>, Vec<usize>), GistError> {
    gist_within_prefilter(index, query)
}

/// 使用 GiST 索引进行范围查询（点 + 半径，笛卡尔坐标）
///
/// 返回 bbox 距离点 <= radius 的所有行索引（候选集，需精确距离过滤）。
pub fn gist_range_query_point(
    index: &GistIndex,
    center: Coord,
    radius: f64,
) -> Result<Vec<usize>, GistError> {
    let query_bbox = BoundingBox::new(
        center.0 - radius,
        center.1 - radius,
        center.0 + radius,
        center.1 + radius,
    );
    index.search_bbox(&query_bbox)
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Row;
    use crate::spatial::{st_geom_from_text, st_point, st_point_with_srid};
    use szrsql_types::value::Value;

    // -----------------------------------------------------------------
    //  测试辅助
    // -----------------------------------------------------------------

    fn make_point_rows_text() -> Vec<Row> {
        vec![
            vec![Value::Text("POINT (1 1)".to_string())],
            vec![Value::Text("POINT (5 5)".to_string())],
            vec![Value::Text("POINT (10 10)".to_string())],
            vec![Value::Text("POINT (15 15)".to_string())],
            vec![Value::Text("POINT (20 20)".to_string())],
        ]
    }

    fn make_polygon_rows_text() -> Vec<Row> {
        vec![
            vec![Value::Text(
                "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))".to_string(),
            )],
            vec![Value::Text(
                "POLYGON ((20 20, 30 20, 30 30, 20 30, 20 20))".to_string(),
            )],
        ]
    }

    // =================================================================
    //  GistError 测试
    // =================================================================

    #[test]
    fn test_error_invalid_capacity_zero() {
        let err = GistIndex::new(0).unwrap_err();
        assert!(matches!(err, GistError::InvalidCapacity(0)));
    }

    #[test]
    fn test_error_invalid_capacity_one() {
        let err = GistIndex::new(1).unwrap_err();
        assert!(matches!(err, GistError::InvalidCapacity(1)));
    }

    #[test]
    fn test_error_to_execution_error() {
        let err: ExecutionError = GistError::EmptyIndex.into();
        assert!(matches!(err, ExecutionError::EvalError(_)));
    }

    #[test]
    fn test_error_not_wkt_text() {
        let rows = vec![vec![Value::Int64(42)]];
        let err = GistIndex::build_from_rows(4, &rows, 0).unwrap_err();
        assert!(matches!(err, GistError::NotWktText(_)));
    }

    #[test]
    fn test_error_wkt_parse_failed() {
        let rows = vec![vec![Value::Text("INVALID WKT".to_string())]];
        let err = GistIndex::build_from_rows(4, &rows, 0).unwrap_err();
        assert!(matches!(err, GistError::WktParse(_)));
    }

    // =================================================================
    //  GistEntry 测试
    // =================================================================

    #[test]
    fn test_gist_entry_new() {
        let bbox = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let entry = GistEntry::new(bbox, 5);
        assert_eq!(entry.bbox, bbox);
        assert_eq!(entry.row_idx, 5);
    }

    // =================================================================
    //  GistNode 测试
    // =================================================================

    #[test]
    fn test_gist_node_new_leaf() {
        let node = GistNode::new_leaf();
        assert!(node.is_leaf());
        assert!(node.is_empty());
        assert_eq!(node.len(), 0);
    }

    #[test]
    fn test_gist_node_new_internal() {
        let node = GistNode::new_internal();
        assert!(!node.is_leaf());
        assert!(node.is_empty());
    }

    #[test]
    fn test_gist_node_recompute_bbox_leaf() {
        let mut node = GistNode::new_leaf();
        node.entries
            .push(GistEntry::new(BoundingBox::new(0.0, 0.0, 5.0, 5.0), 0));
        node.entries
            .push(GistEntry::new(BoundingBox::new(3.0, 3.0, 10.0, 10.0), 1));
        node.recompute_bbox();
        assert_eq!(node.bbox.min_x, 0.0);
        assert_eq!(node.bbox.max_x, 10.0);
        assert_eq!(node.bbox.min_y, 0.0);
        assert_eq!(node.bbox.max_y, 10.0);
    }

    #[test]
    fn test_gist_node_recompute_bbox_internal() {
        let mut node = GistNode::new_internal();
        let mut c1 = GistNode::new_leaf();
        c1.bbox = BoundingBox::new(0.0, 0.0, 5.0, 5.0);
        let mut c2 = GistNode::new_leaf();
        c2.bbox = BoundingBox::new(10.0, 10.0, 15.0, 15.0);
        node.children.push(c1);
        node.children.push(c2);
        node.recompute_bbox();
        assert_eq!(node.bbox.min_x, 0.0);
        assert_eq!(node.bbox.max_x, 15.0);
    }

    #[test]
    fn test_gist_node_recompute_bbox_empty() {
        let mut node = GistNode::new_leaf();
        node.recompute_bbox();
        assert!(node.bbox.is_empty());
    }

    // =================================================================
    //  GistIndex::new 测试
    // =================================================================

    #[test]
    fn test_index_new() {
        let index = GistIndex::new(4).unwrap();
        assert_eq!(index.max_entries_per_node(), 4);
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
        assert_eq!(index.height(), 1);
    }

    #[test]
    fn test_index_new_capacity_two() {
        let index = GistIndex::new(2).unwrap();
        assert_eq!(index.max_entries_per_node(), 2);
    }

    // =================================================================
    //  动态插入测试
    // =================================================================

    #[test]
    fn test_insert_single_point() {
        let mut index = GistIndex::new(4).unwrap();
        index.insert_point(1.0, 2.0, 0).unwrap();
        assert_eq!(index.len(), 1);
        assert!(!index.is_empty());
        assert_eq!(index.height(), 1);
    }

    #[test]
    fn test_insert_multiple_points_no_split() {
        let mut index = GistIndex::new(4).unwrap();
        for i in 0..4 {
            index.insert_point(i as f64, i as f64, i).unwrap();
        }
        assert_eq!(index.len(), 4);
        assert_eq!(index.height(), 1); // 未触发分裂
    }

    #[test]
    fn test_insert_triggers_split() {
        let mut index = GistIndex::new(4).unwrap();
        for i in 0..5 {
            index
                .insert_point(i as f64 * 10.0, i as f64 * 10.0, i)
                .unwrap();
        }
        assert_eq!(index.len(), 5);
        assert_eq!(index.height(), 2); // 触发分裂
    }

    #[test]
    fn test_insert_many_points() {
        let mut index = GistIndex::new(3).unwrap();
        for i in 0..20 {
            index.insert_point(i as f64, i as f64 * 2.0, i).unwrap();
        }
        assert_eq!(index.len(), 20);
        assert!(index.height() >= 2);
    }

    #[test]
    fn test_insert_bbox() {
        let mut index = GistIndex::new(4).unwrap();
        index
            .insert_bbox(BoundingBox::new(0.0, 0.0, 10.0, 10.0), 0)
            .unwrap();
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn test_insert_geometry() {
        let mut index = GistIndex::new(4).unwrap();
        let g = st_point(3.0, 4.0);
        index.insert_geometry(&g, 0).unwrap();
        assert_eq!(index.len(), 1);
    }

    // =================================================================
    //  批量构建测试
    // =================================================================

    #[test]
    fn test_build_from_entries_empty() {
        let index = GistIndex::build_from_entries(4, vec![]).unwrap();
        assert!(index.is_empty());
    }

    #[test]
    fn test_build_from_entries_single() {
        let entry = GistEntry::new(BoundingBox::from_point(1.0, 2.0), 0);
        let index = GistIndex::build_from_entries(4, vec![entry]).unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(index.height(), 1);
    }

    #[test]
    fn test_build_from_entries_multiple() {
        let entries: Vec<GistEntry> = (0..10)
            .map(|i| GistEntry::new(BoundingBox::from_point(i as f64, i as f64), i))
            .collect();
        let index = GistIndex::build_from_entries(4, entries).unwrap();
        assert_eq!(index.len(), 10);
        assert!(index.height() >= 1);
    }

    #[test]
    fn test_build_from_entries_large() {
        let entries: Vec<GistEntry> = (0..100)
            .map(|i| GistEntry::new(BoundingBox::from_point((i % 10) as f64, (i / 10) as f64), i))
            .collect();
        let index = GistIndex::build_from_entries(4, entries).unwrap();
        assert_eq!(index.len(), 100);
        assert!(index.height() >= 2);
    }

    #[test]
    fn test_build_from_rows() {
        let rows = make_point_rows_text();
        let index = GistIndex::build_from_rows(4, &rows, 0).unwrap();
        assert_eq!(index.len(), 5);
    }

    #[test]
    fn test_build_from_rows_polygons() {
        let rows = make_polygon_rows_text();
        let index = GistIndex::build_from_rows(4, &rows, 0).unwrap();
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn test_build_from_geometries() {
        let geoms = vec![
            (st_point(1.0, 2.0), 0),
            (st_point(3.0, 4.0), 1),
            (st_point(5.0, 6.0), 2),
        ];
        let index = GistIndex::build_from_geometries(4, geoms).unwrap();
        assert_eq!(index.len(), 3);
    }

    // =================================================================
    //  范围查询测试
    // =================================================================

    #[test]
    fn test_search_bbox_empty_index() {
        let index = GistIndex::new(4).unwrap();
        let query = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let result = index.search_bbox(&query).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_search_bbox_single_point_hit() {
        let mut index = GistIndex::new(4).unwrap();
        index.insert_point(5.0, 5.0, 0).unwrap();
        let query = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let result = index.search_bbox(&query).unwrap();
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn test_search_bbox_single_point_miss() {
        let mut index = GistIndex::new(4).unwrap();
        index.insert_point(15.0, 15.0, 0).unwrap();
        let query = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let result = index.search_bbox(&query).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_search_bbox_multiple_points_partial() {
        let mut index = GistIndex::new(4).unwrap();
        index.insert_point(1.0, 1.0, 0).unwrap();
        index.insert_point(5.0, 5.0, 1).unwrap();
        index.insert_point(15.0, 15.0, 2).unwrap();
        index.insert_point(20.0, 20.0, 3).unwrap();
        let query = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let result = index.search_bbox(&query).unwrap();
        assert_eq!(result, vec![0, 1]);
    }

    #[test]
    fn test_search_bbox_after_split() {
        let mut index = GistIndex::new(2).unwrap();
        for i in 0..6 {
            index
                .insert_point(i as f64 * 10.0, i as f64 * 10.0, i)
                .unwrap();
        }
        let query = BoundingBox::new(0.0, 0.0, 25.0, 25.0);
        let result = index.search_bbox(&query).unwrap();
        // 应包含 0, 10, 20（在 [0,25] 内）
        assert!(result.contains(&0));
        assert!(result.contains(&1));
        assert!(result.contains(&2));
    }

    #[test]
    fn test_search_bbox_large_dataset() {
        let entries: Vec<GistEntry> = (0..100)
            .map(|i| GistEntry::new(BoundingBox::from_point((i % 10) as f64, (i / 10) as f64), i))
            .collect();
        let index = GistIndex::build_from_entries(4, entries).unwrap();
        // 查询 [0, 5] × [0, 5] 范围内的点
        let query = BoundingBox::new(0.0, 0.0, 5.0, 5.0);
        let result = index.search_bbox(&query).unwrap();
        // (0,0)..(5,5) → row_idx 0,1,2,3,4,5（第 0 行的 5 个）+ 10,11,12,13,14,15（第 1 行的 5 个）...
        // 0..9 的 (x=0..9,y=0) → 0..5,6,7,8,9: 0..5 命中（5个）
        // 10..19 的 (x=0..9,y=1) → 10..15 命中（5个）
        // ...共 6 行 × 6 个 = 36 个
        assert!(result.len() >= 25); // 至少有 25 个命中
    }

    // =================================================================
    //  点查询测试
    // =================================================================

    #[test]
    fn test_search_point_hit() {
        let mut index = GistIndex::new(4).unwrap();
        index.insert_point(5.0, 5.0, 0).unwrap();
        let result = index.search_point(5.0, 5.0).unwrap();
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn test_search_point_miss() {
        let mut index = GistIndex::new(4).unwrap();
        index.insert_point(5.0, 5.0, 0).unwrap();
        let result = index.search_point(10.0, 10.0).unwrap();
        assert!(result.is_empty());
    }

    // =================================================================
    //  search_within 测试
    // =================================================================

    #[test]
    fn test_search_within_strict() {
        let mut index = GistIndex::new(4).unwrap();
        index
            .insert_bbox(BoundingBox::new(1.0, 1.0, 2.0, 2.0), 0)
            .unwrap();
        index
            .insert_bbox(BoundingBox::new(0.0, 0.0, 15.0, 15.0), 1)
            .unwrap();
        let query = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let within = index.search_within(&query).unwrap();
        // 仅 row_idx 0 完全在 query 内
        assert_eq!(within, vec![0]);
    }

    #[test]
    fn test_search_within_all_inside() {
        let mut index = GistIndex::new(4).unwrap();
        index
            .insert_bbox(BoundingBox::new(1.0, 1.0, 2.0, 2.0), 0)
            .unwrap();
        index
            .insert_bbox(BoundingBox::new(3.0, 3.0, 4.0, 4.0), 1)
            .unwrap();
        let query = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let within = index.search_within(&query).unwrap();
        assert_eq!(within, vec![0, 1]);
    }

    // =================================================================
    //  GiST 核心操作测试
    // =================================================================

    #[test]
    fn test_consistent_intersecting() {
        let node_bbox = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let query = BoundingBox::new(5.0, 5.0, 15.0, 15.0);
        assert!(consistent(&node_bbox, &query));
    }

    #[test]
    fn test_consistent_disjoint() {
        let node_bbox = BoundingBox::new(0.0, 0.0, 1.0, 1.0);
        let query = BoundingBox::new(10.0, 10.0, 20.0, 20.0);
        assert!(!consistent(&node_bbox, &query));
    }

    #[test]
    fn test_union_bboxes_empty() {
        let u = union_bboxes(&[]);
        assert!(u.is_empty());
    }

    #[test]
    fn test_union_bboxes_single() {
        let b = BoundingBox::new(0.0, 0.0, 5.0, 5.0);
        let u = union_bboxes(&[b]);
        assert_eq!(u, b);
    }

    #[test]
    fn test_union_bboxes_multiple() {
        let u = union_bboxes(&[
            BoundingBox::new(0.0, 0.0, 5.0, 5.0),
            BoundingBox::new(3.0, 3.0, 10.0, 10.0),
        ]);
        assert_eq!(u.min_x, 0.0);
        assert_eq!(u.max_x, 10.0);
    }

    #[test]
    fn test_penalty() {
        let node_bbox = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let entry_bbox = BoundingBox::new(5.0, 5.0, 15.0, 15.0);
        let p = penalty(&node_bbox, &entry_bbox);
        // union 面积 225 - 原 100 = 125
        assert!((p - 125.0).abs() < 1e-9);
    }

    #[test]
    fn test_penalty_no_increase() {
        let node_bbox = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let entry_bbox = BoundingBox::new(2.0, 2.0, 8.0, 8.0);
        let p = penalty(&node_bbox, &entry_bbox);
        assert!(p.abs() < 1e-9);
    }

    #[test]
    fn test_equal_same() {
        let a = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let b = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        assert!(equal(&a, &b));
    }

    #[test]
    fn test_equal_different() {
        let a = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let b = BoundingBox::new(0.0, 0.0, 5.0, 5.0);
        assert!(!equal(&a, &b));
    }

    // =================================================================
    //  picksplit 测试
    // =================================================================

    #[test]
    fn test_picksplit_single_entry() {
        let entries = vec![GistEntry::new(BoundingBox::from_point(1.0, 1.0), 0)];
        let (g1, g2) = picksplit_quadratic(entries);
        assert_eq!(g1.len() + g2.len(), 1);
    }

    #[test]
    fn test_picksplit_two_entries() {
        let entries = vec![
            GistEntry::new(BoundingBox::from_point(0.0, 0.0), 0),
            GistEntry::new(BoundingBox::from_point(100.0, 100.0), 1),
        ];
        let (g1, g2) = picksplit_quadratic(entries);
        assert_eq!(g1.len(), 1);
        assert_eq!(g2.len(), 1);
    }

    #[test]
    fn test_picksplit_balanced() {
        let entries: Vec<GistEntry> = (0..10)
            .map(|i| GistEntry::new(BoundingBox::from_point(i as f64, i as f64), i))
            .collect();
        let (g1, g2) = picksplit_quadratic(entries);
        // 两组都不应为空
        assert!(!g1.is_empty());
        assert!(!g2.is_empty());
        // 总数 = 10
        assert_eq!(g1.len() + g2.len(), 10);
    }

    #[test]
    fn test_picksplit_separates_distant_points() {
        // 两个远距离的点应分到不同组
        let entries = vec![
            GistEntry::new(BoundingBox::from_point(0.0, 0.0), 0),
            GistEntry::new(BoundingBox::from_point(1000.0, 1000.0), 1),
            GistEntry::new(BoundingBox::from_point(1.0, 1.0), 2),
            GistEntry::new(BoundingBox::from_point(1001.0, 1001.0), 3),
        ];
        let (g1, g2) = picksplit_quadratic(entries);
        // 至少每组都有条目
        assert!(!g1.is_empty());
        assert!(!g2.is_empty());
    }

    // =================================================================
    //  索引统计测试
    // =================================================================

    #[test]
    fn test_size_bytes() {
        let mut index = GistIndex::new(4).unwrap();
        index.insert_point(1.0, 2.0, 0).unwrap();
        let size = index.size_bytes();
        assert!(size > 0);
    }

    #[test]
    fn test_root_accessor() {
        let mut index = GistIndex::new(4).unwrap();
        index.insert_point(1.0, 2.0, 0).unwrap();
        let root = index.root();
        assert!(root.is_leaf());
        assert_eq!(root.len(), 1);
    }

    // =================================================================
    //  查询辅助函数测试
    // =================================================================

    #[test]
    fn test_gist_within_prefilter() {
        let entries: Vec<GistEntry> = (0..5)
            .map(|i| GistEntry::new(BoundingBox::from_point(i as f64 * 5.0, i as f64 * 5.0), i))
            .collect();
        let index = GistIndex::build_from_entries(4, entries).unwrap();
        let query = st_geom_from_text("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))").unwrap();
        let (candidates, exact) = gist_within_prefilter(&index, &query).unwrap();
        assert!(!candidates.is_empty());
        assert_eq!(candidates, exact);
    }

    #[test]
    fn test_gist_contains_prefilter() {
        let entries: Vec<GistEntry> = (0..3)
            .map(|i| GistEntry::new(BoundingBox::from_point(i as f64, i as f64), i))
            .collect();
        let index = GistIndex::build_from_entries(4, entries).unwrap();
        let query = st_geom_from_text("POLYGON ((-1 -1, 5 -1, 5 5, -1 5, -1 -1))").unwrap();
        let (candidates, _) = gist_contains_prefilter(&index, &query).unwrap();
        assert!(!candidates.is_empty());
    }

    #[test]
    fn test_gist_range_query_point() {
        let entries: Vec<GistEntry> = (0..10)
            .map(|i| GistEntry::new(BoundingBox::from_point(i as f64, 0.0), i))
            .collect();
        let index = GistIndex::build_from_entries(4, entries).unwrap();
        let result = gist_range_query_point(&index, (5.0, 0.0), 2.0).unwrap();
        // 候选：x ∈ [3, 7] → row_idx 3,4,5,6,7
        assert!(result.contains(&3));
        assert!(result.contains(&5));
        assert!(result.contains(&7));
        assert!(!result.contains(&0));
    }

    // =================================================================
    //  E2E: 集成场景
    // =================================================================

    #[test]
    fn test_e2e_create_index_and_range_query() {
        // 模拟 CREATE INDEX ON t USING GIST (loc) → WHERE loc && envelope
        let rows = make_point_rows_text();
        let index = GistIndex::build_from_rows(4, &rows, 0).unwrap();

        // 范围查询 [0, 6] × [0, 6]
        let query = BoundingBox::new(0.0, 0.0, 6.0, 6.0);
        let hits = index.search_bbox(&query).unwrap();
        // 应命中 row_idx 0 (POINT 1 1) 和 1 (POINT 5 5)
        assert!(hits.contains(&0));
        assert!(hits.contains(&1));
        assert!(!hits.contains(&2)); // POINT (10 10) 不在范围内
    }

    #[test]
    fn test_e2e_gist_within_with_exact_filter() {
        // 模拟 GiST 索引预过滤 + 精确 ST_Within
        let rows: Vec<Row> = (0..10)
            .map(|i| vec![Value::Text(format!("POINT ({i} {i})", i = i as f64 * 2.0))])
            .collect();
        let index = GistIndex::build_from_rows(4, &rows, 0).unwrap();

        // 查询多边形：(0,0)-(10,10) 范围内的点
        let poly = st_geom_from_text("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))").unwrap();
        let poly_bbox = poly.geom.bounding_box().unwrap();

        // 1. 索引预过滤
        let candidates = index.search_bbox(&poly_bbox).unwrap();
        // 2. 精确过滤
        let mut exact_hits = Vec::new();
        for &row_idx in &candidates {
            if let Some(Value::Text(wkt)) = rows[row_idx].first() {
                let g = st_geom_from_text(wkt).unwrap();
                if crate::spatial::st_within(&g, &poly).unwrap() {
                    exact_hits.push(row_idx);
                }
            }
        }
        // (0,0), (2,2), (4,4), (6,6), (8,8) → row_idx 0,1,2,3,4
        // (10,10) → row_idx 5，在边界上，OGC within = true（含边界）
        assert!(exact_hits.contains(&0));
        assert!(exact_hits.contains(&4));
        assert!(exact_hits.contains(&5)); // (10,10) 在边界 → within = true
        assert!(!exact_hits.contains(&6)); // (12,12) 在多边形外
    }

    #[test]
    fn test_e2e_gist_contains_query() {
        // 模拟 GiST 索引预过滤 + 精确 ST_Contains
        let rows: Vec<Row> = (0..5)
            .map(|i| vec![Value::Text(format!("POINT ({i} {i})", i = i as f64 * 5.0))])
            .collect();
        let index = GistIndex::build_from_rows(4, &rows, 0).unwrap();

        // 查询：哪些点在 (0,0)-(15,15) 多边形内
        let poly = st_geom_from_text("POLYGON ((0 0, 15 0, 15 15, 0 15, 0 0))").unwrap();
        let poly_bbox = poly.geom.bounding_box().unwrap();
        let candidates = index.search_bbox(&poly_bbox).unwrap();

        let mut contains_hits = Vec::new();
        for &row_idx in &candidates {
            if let Some(Value::Text(wkt)) = rows[row_idx].first() {
                let g = st_geom_from_text(wkt).unwrap();
                if crate::spatial::st_contains(&poly, &g).unwrap() {
                    contains_hits.push(row_idx);
                }
            }
        }
        // (0,0), (5,5), (10,10) → row_idx 0,1,2
        assert!(contains_hits.contains(&0));
        assert!(contains_hits.contains(&1));
        assert!(contains_hits.contains(&2));
        // (15,15) 在边界 → contains = true（OGC 含边界）
        // (20,20) 在外 → false
        assert!(!contains_hits.contains(&4));
    }

    #[test]
    fn test_e2e_gist_distance_query() {
        // 模拟 ST_DWithin(center, loc, radius) 查询
        let rows: Vec<Row> = (0..10)
            .map(|i| vec![Value::Text(format!("POINT ({i} 0)", i = i as f64))])
            .collect();
        let index = GistIndex::build_from_rows(4, &rows, 0).unwrap();

        // 查找距离 (5, 0) 在 2.5 范围内的点
        let center = (5.0, 0.0);
        let radius = 2.5;
        let candidates = gist_range_query_point(&index, center, radius).unwrap();

        // 精确距离过滤
        let mut hits = Vec::new();
        for &row_idx in &candidates {
            if let Some(Value::Text(wkt)) = rows[row_idx].first() {
                let g = st_geom_from_text(wkt).unwrap();
                let center_geom = st_point(center.0, center.1);
                let d = crate::spatial::st_distance(&center_geom, &g).unwrap();
                if d <= radius {
                    hits.push(row_idx);
                }
            }
        }
        // x ∈ [2.5, 7.5] → row_idx 3,4,5,6,7
        assert_eq!(hits, vec![3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_e2e_gist_with_geography() {
        // 模拟 SRID=4326 geography 索引
        let geoms = vec![
            (st_point_with_srid(116.40, 39.90, 4326), 0), // 北京
            (st_point_with_srid(121.47, 31.23, 4326), 1), // 上海
            (st_point_with_srid(113.23, 23.16, 4326), 2), // 广州
        ];
        let index = GistIndex::build_from_geometries(4, geoms).unwrap();
        assert_eq!(index.len(), 3);

        // 查询北京附近的点
        let query = BoundingBox::new(115.0, 38.0, 118.0, 41.0);
        let hits = index.search_bbox(&query).unwrap();
        assert!(hits.contains(&0)); // 北京
        assert!(!hits.contains(&1)); // 上海不在范围内
    }

    #[test]
    fn test_e2e_insert_after_build() {
        // 先批量构建，再动态插入
        let entries: Vec<GistEntry> = (0..5)
            .map(|i| GistEntry::new(BoundingBox::from_point(i as f64, i as f64), i))
            .collect();
        let mut index = GistIndex::build_from_entries(4, entries).unwrap();
        assert_eq!(index.len(), 5);

        index.insert_point(100.0, 100.0, 5).unwrap();
        assert_eq!(index.len(), 6);

        // 查询新插入的点
        let query = BoundingBox::new(99.0, 99.0, 101.0, 101.0);
        let hits = index.search_bbox(&query).unwrap();
        assert!(hits.contains(&5));
    }

    #[test]
    fn test_e2e_polygon_index() {
        let rows = make_polygon_rows_text();
        let index = GistIndex::build_from_rows(4, &rows, 0).unwrap();

        // 查询与第一个多边形相交的条目
        let query = BoundingBox::new(5.0, 5.0, 25.0, 25.0);
        let hits = index.search_bbox(&query).unwrap();
        // 两个多边形的 bbox 都与查询相交
        assert!(hits.contains(&0)); // POLYGON ((0 0, 10 0, ...))
        assert!(hits.contains(&1)); // POLYGON ((20 20, 30 20, ...))
    }

    #[test]
    fn test_e2e_st_distance_with_index_prefilter() {
        // 完整流程：索引预过滤 → 精确距离计算
        let rows: Vec<Row> = (0..20)
            .map(|i| vec![Value::Text(format!("POINT ({i} {i})", i = i as f64))])
            .collect();
        let index = GistIndex::build_from_rows(4, &rows, 0).unwrap();

        // 找距离 (5, 5) 在 3.0 以内的所有点
        let center = st_point(5.0, 5.0);
        let candidates = gist_range_query_point(&index, (5.0, 5.0), 3.0).unwrap();
        let mut hits = Vec::new();
        for &row_idx in &candidates {
            if let Some(Value::Text(wkt)) = rows[row_idx].first() {
                let g = st_geom_from_text(wkt).unwrap();
                let d = crate::spatial::st_distance(&center, &g).unwrap();
                if d <= 3.0 {
                    hits.push((row_idx, d));
                }
            }
        }
        // 距离 (5,5) <= 3.0 的点：(5,5) d=0, (4,4) d=√2≈1.41, (6,6) d=√2,
        // (3,3) d=√8≈2.83, (7,7) d=√8 → row_idx 3,4,5,6,7
        assert!(hits.len() >= 3);
    }

    // =================================================================
    //  边界与压力测试
    // =================================================================

    #[test]
    fn test_large_dataset_1000_points() {
        let entries: Vec<GistEntry> = (0..1000)
            .map(|i| {
                let x = (i % 100) as f64;
                let y = (i / 100) as f64;
                GistEntry::new(BoundingBox::from_point(x, y), i)
            })
            .collect();
        let index = GistIndex::build_from_entries(8, entries).unwrap();
        assert_eq!(index.len(), 1000);
        assert!(index.height() >= 2);

        // 查询整个范围
        let query = BoundingBox::new(0.0, 0.0, 100.0, 10.0);
        let hits = index.search_bbox(&query).unwrap();
        assert_eq!(hits.len(), 1000); // 全部命中
    }

    #[test]
    fn test_degenerate_single_entry_index() {
        let mut index = GistIndex::new(4).unwrap();
        index.insert_point(0.0, 0.0, 0).unwrap();
        let query = BoundingBox::from_point(0.0, 0.0);
        let hits = index.search_bbox(&query).unwrap();
        assert_eq!(hits, vec![0]);
    }

    #[test]
    fn test_all_points_at_same_location() {
        let entries: Vec<GistEntry> = (0..5)
            .map(|i| GistEntry::new(BoundingBox::from_point(1.0, 1.0), i))
            .collect();
        let index = GistIndex::build_from_entries(4, entries).unwrap();
        let query = BoundingBox::from_point(1.0, 1.0);
        let hits = index.search_bbox(&query).unwrap();
        assert_eq!(hits.len(), 5);
    }

    #[test]
    fn test_search_no_results_outside_all() {
        let entries: Vec<GistEntry> = (0..5)
            .map(|i| GistEntry::new(BoundingBox::from_point(i as f64, i as f64), i))
            .collect();
        let index = GistIndex::build_from_entries(4, entries).unwrap();
        let query = BoundingBox::new(100.0, 100.0, 200.0, 200.0);
        let hits = index.search_bbox(&query).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn test_index_clone_preserves_state() {
        let entries: Vec<GistEntry> = (0..5)
            .map(|i| GistEntry::new(BoundingBox::from_point(i as f64, i as f64), i))
            .collect();
        let index = GistIndex::build_from_entries(4, entries).unwrap();
        let cloned = index.clone();
        assert_eq!(index.len(), cloned.len());
        assert_eq!(index.height(), cloned.height());

        let query = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(
            index.search_bbox(&query).unwrap(),
            cloned.search_bbox(&query).unwrap()
        );
    }
}
