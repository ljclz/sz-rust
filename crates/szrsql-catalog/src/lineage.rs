//! 数据血缘追踪（最小可行版）— Phase TDengine-P5。
//!
//! # 设计目标
//!
//! 受 TDengine IDMP 启发，记录字段级血缘关系：每条 `LineageEdge` 描述
//! "目标列 ← 源列 + 转换描述"的有向边。本模块为**最小可行版**，仅使用
//! 内存 `HashMap` + `Vec`，不引入图数据库依赖。
//!
//! # 数据模型
//!
//! ```text
//! LineageEdge {
//!     source: ColumnRef,      // 上游：表名 + 列名
//!     target: ColumnRef,      // 下游：表名 + 列名
//!     transform: String,      // 转换描述（如 "SUM(price)" / "CAST AS BIGINT"）
//!     source_type: EdgeSource,// 来源：CTAS / VIEW / CDC / MANUAL
//! }
//! ```
//!
//! # 查询能力
//!
//! - `upstream_of(table)` — 该表所有列的上游来源
//! - `downstream_of(table)` — 该表被哪些表引用
//! - `column_lineage(table, column)` — 字段级血缘（target = table.column 的所有边）
//! - `all_edges()` — 全量血缘（用于 MCP `get_lineage` 暴露给 LLM）
//!
//! # 非目标（最小可行版不涵盖）
//!
//! - 不解析 SQL AST 自动推断血缘（需 CTAS/VIEW 执行器手动调用 `record_*`）
//! - 不持久化（重启即丢失，Phase 5+ 配合 WAL 持久化）
//! - 不做图遍历（仅 1 跳查询，多跳留待后续）
//!
//! 对应 `docs/TDengine启发技术方案.md` P5。

use std::collections::HashMap;

// =====================================================================
//  核心数据结构
// =====================================================================

/// 列引用 — 表名 + 列名（字段级血缘最小单元）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnRef {
    /// 表名（schema.table 或裸表名）
    pub table: String,
    /// 列名
    pub column: String,
}

impl ColumnRef {
    /// 构造列引用
    pub fn new(table: impl Into<String>, column: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            column: column.into(),
        }
    }
}

/// 血缘边来源 — 标记血缘是如何产生的
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeSource {
    /// CTAS（CREATE TABLE AS SELECT）
    Ctas,
    /// 视图（CREATE VIEW AS SELECT）
    View,
    /// CDC 演化（ALTER TABLE 重命名列时迁移血缘）
    Cdc,
    /// 手动标注（通过 MCP 或 API 显式声明）
    Manual,
}

impl EdgeSource {
    /// 转字符串（用于序列化与展示）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ctas => "ctas",
            Self::View => "view",
            Self::Cdc => "cdc",
            Self::Manual => "manual",
        }
    }
}

/// 血缘边 — 一条"target ← source + transform"的有向边
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineageEdge {
    /// 上游来源（字段级）
    pub source: ColumnRef,
    /// 下游目标（字段级）
    pub target: ColumnRef,
    /// 转换描述（如 "SUM(price)" / "direct" / "CAST AS BIGINT"）
    pub transform: String,
    /// 血缘来源类型
    pub source_type: EdgeSource,
}

impl LineageEdge {
    /// 构造直接血缘（无转换，常用于视图直接选列）
    pub fn direct(
        source: ColumnRef,
        target: ColumnRef,
        source_type: EdgeSource,
    ) -> Self {
        Self {
            source,
            target,
            transform: "direct".to_string(),
            source_type,
        }
    }

    /// 构造带转换的血缘
    pub fn with_transform(
        source: ColumnRef,
        target: ColumnRef,
        transform: impl Into<String>,
        source_type: EdgeSource,
    ) -> Self {
        Self {
            source,
            target,
            transform: transform.into(),
            source_type,
        }
    }
}

// =====================================================================
//  LineageStore — 血缘存储（内存最小可行版）
// =====================================================================

/// 数据血缘存储 — 内存实现，按 target 索引以加速 upstream 查询。
///
/// 设计权衡：
/// - **索引结构**：`HashMap<target_table, Vec<LineageEdge>>` + 同步维护
///   `HashMap<source_table, Vec<LineageEdge>>`，双向 O(1) 查找。
/// - **去重策略**：相同 (source, target, source_type) 视为同一条边，重复 add 视为幂等。
/// - **线程安全**：本结构非 `Sync`，调用方需在外层加锁（与 `ManagedCatalog` 一致）。
pub struct LineageStore {
    /// 按 target_table 索引（用于 upstream_of / column_lineage 查询）
    by_target: HashMap<String, Vec<LineageEdge>>,
    /// 按 source_table 索引（用于 downstream_of 查询）
    by_source: HashMap<String, Vec<LineageEdge>>,
}

impl Default for LineageStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LineageStore {
    /// 创建空血缘存储
    pub fn new() -> Self {
        Self {
            by_target: HashMap::new(),
            by_source: HashMap::new(),
        }
    }

    /// 添加一条血缘边（幂等：相同 (source, target, source_type) 不重复记录）
    ///
    /// 返回 `true` 表示新增，`false` 表示已存在（幂等）。
    pub fn add_edge(&mut self, edge: LineageEdge) -> bool {
        let target_key = edge.target.table.clone();
        let source_key = edge.source.table.clone();

        // 去重检查：同 (source, target, source_type) 已存在则跳过
        let target_vec = self.by_target.entry(target_key).or_default();
        let exists = target_vec.iter().any(|e| {
            e.source == edge.source && e.target == edge.target && e.source_type == edge.source_type
        });
        if exists {
            return false;
        }
        target_vec.push(edge.clone());

        self.by_source
            .entry(source_key)
            .or_default()
            .push(edge);
        true
    }

    /// 查询某表的**上游**血缘（target.table = table 的所有边）
    ///
    /// 返回该表所有列的来源信息。若该表无血缘，返回空 Vec。
    pub fn upstream_of(&self, table: &str) -> Vec<LineageEdge> {
        self.by_target
            .get(table)
            .cloned()
            .unwrap_or_default()
    }

    /// 查询某表的**下游**血缘（source.table = table 的所有边）
    ///
    /// 返回该表被哪些下游表引用。若该表无下游，返回空 Vec。
    pub fn downstream_of(&self, table: &str) -> Vec<LineageEdge> {
        self.by_source
            .get(table)
            .cloned()
            .unwrap_or_default()
    }

    /// 查询字段级血缘（target = table.column 的所有边）
    ///
    /// 用于回答"这一列是怎么算出来的"。
    pub fn column_lineage(&self, table: &str, column: &str) -> Vec<LineageEdge> {
        self.upstream_of(table)
            .into_iter()
            .filter(|e| e.target.column == column)
            .collect()
    }

    /// 返回全量血缘边（用于 MCP `get_lineage` 暴露给 LLM）
    pub fn all_edges(&self) -> Vec<LineageEdge> {
        self.by_target
            .values()
            .flat_map(|v| v.iter().cloned())
            .collect()
    }

    /// 血缘边总数
    pub fn len(&self) -> usize {
        self.by_target.values().map(|v| v.len()).sum()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 已记录血缘的所有表名（target ∪ source 去重）
    pub fn tables(&self) -> Vec<String> {
        let mut set = std::collections::HashSet::new();
        for k in self.by_target.keys() {
            set.insert(k.clone());
        }
        for k in self.by_source.keys() {
            set.insert(k.clone());
        }
        let mut v: Vec<String> = set.into_iter().collect();
        v.sort();
        v
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn col(table: &str, column: &str) -> ColumnRef {
        ColumnRef::new(table, column)
    }

    #[test]
    fn test_lineage_add_and_upstream() {
        let mut store = LineageStore::new();
        assert!(store.is_empty());

        // products.price ← orders.total_price (CTAS: SUM)
        let added = store.add_edge(LineageEdge::with_transform(
            col("products", "price"),
            col("orders", "total_price"),
            "SUM(price)",
            EdgeSource::Ctas,
        ));
        assert!(added);
        assert_eq!(store.len(), 1);

        let upstream = store.upstream_of("orders");
        assert_eq!(upstream.len(), 1);
        assert_eq!(upstream[0].source.table, "products");
        assert_eq!(upstream[0].source.column, "price");
        assert_eq!(upstream[0].target.column, "total_price");
        assert_eq!(upstream[0].source_type, EdgeSource::Ctas);
    }

    #[test]
    fn test_lineage_idempotent_add() {
        let mut store = LineageStore::new();
        let edge = LineageEdge::direct(
            col("a", "x"),
            col("b", "y"),
            EdgeSource::View,
        );
        assert!(store.add_edge(edge.clone()));
        // 重复添加 — 幂等，返回 false
        assert!(!store.add_edge(edge.clone()));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_lineage_different_transform_creates_new_edge() {
        let mut store = LineageStore::new();
        // 相同 (source, target, source_type) 但 transform 不同 —
        // 按当前去重策略仍视为同一条边（transform 不参与去重键）
        let e1 = LineageEdge::with_transform(
            col("a", "x"), col("b", "y"), "SUM", EdgeSource::Ctas,
        );
        let e2 = LineageEdge::with_transform(
            col("a", "x"), col("b", "y"), "AVG", EdgeSource::Ctas,
        );
        assert!(store.add_edge(e1));
        // 同 source/target/source_type — 幂等
        assert!(!store.add_edge(e2));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_lineage_different_source_type_distinct() {
        let mut store = LineageStore::new();
        // 同 (source, target) 但 source_type 不同 — 视为两条边
        assert!(store.add_edge(LineageEdge::direct(
            col("a", "x"), col("b", "y"), EdgeSource::Ctas,
        )));
        assert!(store.add_edge(LineageEdge::direct(
            col("a", "x"), col("b", "y"), EdgeSource::View,
        )));
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_lineage_downstream() {
        let mut store = LineageStore::new();
        store.add_edge(LineageEdge::direct(
            col("products", "id"), col("orders", "product_id"), EdgeSource::Ctas,
        ));
        store.add_edge(LineageEdge::direct(
            col("products", "name"), col("order_items", "product_name"), EdgeSource::View,
        ));

        let downstream = store.downstream_of("products");
        assert_eq!(downstream.len(), 2);
        let target_tables: Vec<&str> =
            downstream.iter().map(|e| e.target.table.as_str()).collect();
        assert!(target_tables.contains(&"orders"));
        assert!(target_tables.contains(&"order_items"));
    }

    #[test]
    fn test_lineage_column_level() {
        let mut store = LineageStore::new();
        store.add_edge(LineageEdge::with_transform(
            col("products", "price"), col("orders", "total_price"),
            "SUM(price)", EdgeSource::Ctas,
        ));
        store.add_edge(LineageEdge::with_transform(
            col("products", "tax"), col("orders", "total_tax"),
            "SUM(tax)", EdgeSource::Ctas,
        ));

        let lineage = store.column_lineage("orders", "total_price");
        assert_eq!(lineage.len(), 1);
        assert_eq!(lineage[0].source.column, "price");
        assert_eq!(lineage[0].transform, "SUM(price)");

        // 不存在的列 — 空结果
        assert!(store.column_lineage("orders", "nonexistent").is_empty());
    }

    #[test]
    fn test_lineage_upstream_empty_for_unknown_table() {
        let store = LineageStore::new();
        assert!(store.upstream_of("nonexistent").is_empty());
        assert!(store.downstream_of("nonexistent").is_empty());
    }

    #[test]
    fn test_lineage_all_edges_and_tables() {
        let mut store = LineageStore::new();
        store.add_edge(LineageEdge::direct(
            col("a", "x"), col("b", "y"), EdgeSource::Ctas,
        ));
        store.add_edge(LineageEdge::direct(
            col("b", "y"), col("c", "z"), EdgeSource::View,
        ));

        let all = store.all_edges();
        assert_eq!(all.len(), 2);

        let tables = store.tables();
        assert_eq!(tables, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn test_lineage_edge_source_as_str() {
        assert_eq!(EdgeSource::Ctas.as_str(), "ctas");
        assert_eq!(EdgeSource::View.as_str(), "view");
        assert_eq!(EdgeSource::Cdc.as_str(), "cdc");
        assert_eq!(EdgeSource::Manual.as_str(), "manual");
    }

    #[test]
    fn test_lineage_edge_direct_constructor() {
        let edge = LineageEdge::direct(
            col("a", "x"), col("b", "y"), EdgeSource::Manual,
        );
        assert_eq!(edge.transform, "direct");
        assert_eq!(edge.source_type, EdgeSource::Manual);
    }

    #[test]
    fn test_lineage_len_and_is_empty() {
        let mut store = LineageStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        store.add_edge(LineageEdge::direct(
            col("a", "x"), col("b", "y"), EdgeSource::Ctas,
        ));
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);
    }
}
