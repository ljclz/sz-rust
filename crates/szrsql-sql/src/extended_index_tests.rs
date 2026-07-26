//! Phase 6.17 集成测试 — 扩展索引（GiST/GIN/R*Tree/FTS5）。
//!
//! 覆盖类别：
//! - IndexMethod 枚举（3 条）：from_str_ci / as_str / Display
//! - Point2D + BoundingBox（6 条）：距离计算 / from_value / 边界框包含/相交/合并/距离
//! - GIN 倒排索引（8 条）：构建 / 单词素查询 / AND / OR / NOT / FollowedBy / 空索引 / all_row_ids
//! - R-Tree 空间索引（8 条）：构建 / 范围查询 / k-NN / 空索引 / 边界框 / 大量数据 / 剪枝 / NULL 跳过
//! - GiST k-NN 索引（5 条）：构建 / k-NN / 范围查询 / 空索引 / 距离排序
//! - FTS5 全文检索（8 条）：分词器 / 构建 / MATCH 单词 / MATCH 多词 / 短语查询 / 空查询 / lookup_term / all_row_ids
//! - build_from_table 集成（4 条）：GIN from TsVector table / R-Tree from Point table / FTS5 from Text table / 类型错误
//!
//! 共 42 个测试用例。

use super::executor::InMemoryTable;
use super::extended_index::{
    BoundingBox, Fts5Index, GinIndex, GistIndex, IndexMethod, Point2D, RTreeIndex,
};
use szrsql_types::value::{ColumnType, TsQuery, TsVector, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建带 tsvector 列的表并填充数据
fn make_tsvector_table() -> InMemoryTable {
    let mut table = InMemoryTable::with_columns(
        "docs",
        vec![("id", ColumnType::Int64), ("content", ColumnType::TsVector)],
    );
    table.insert(vec![
        Value::Int64(1),
        Value::TsVector(TsVector::from_lexemes(vec!["hello", "world"])),
    ]);
    table.insert(vec![
        Value::Int64(2),
        Value::TsVector(TsVector::from_lexemes(vec!["hello", "rust"])),
    ]);
    table.insert(vec![
        Value::Int64(3),
        Value::TsVector(TsVector::from_lexemes(vec!["world", "peace"])),
    ]);
    table.insert(vec![
        Value::Int64(4),
        Value::TsVector(TsVector::from_lexemes(vec!["foo", "bar"])),
    ]);
    table
}

/// 创建带 2D 点列的表并填充数据
fn make_points_table() -> InMemoryTable {
    let mut table = InMemoryTable::with_columns(
        "locations",
        vec![
            ("id", ColumnType::Int64),
            ("pos", ColumnType::Array(Box::new(ColumnType::Float64))),
        ],
    );
    // 10 个点：网格布局
    let points: Vec<(f64, f64)> = vec![
        (0.0, 0.0),
        (1.0, 0.0),
        (2.0, 0.0),
        (0.0, 1.0),
        (1.0, 1.0),
        (2.0, 1.0),
        (0.0, 2.0),
        (1.0, 2.0),
        (2.0, 2.0),
        (5.0, 5.0),
    ];
    for (i, (x, y)) in points.iter().enumerate() {
        table.insert(vec![
            Value::Int64(i as i64),
            Value::Array(vec![Value::Float64(*x), Value::Float64(*y)]),
        ]);
    }
    table
}

/// 创建带 Text 列的表并填充数据
fn make_text_table() -> InMemoryTable {
    let mut table = InMemoryTable::with_columns(
        "articles",
        vec![("id", ColumnType::Int64), ("body", ColumnType::Text)],
    );
    table.insert(vec![
        Value::Int64(1),
        Value::Text("The quick brown fox jumps over the lazy dog".into()),
    ]);
    table.insert(vec![
        Value::Int64(2),
        Value::Text("A quick brown dog runs in the park".into()),
    ]);
    table.insert(vec![
        Value::Int64(3),
        Value::Text("The lazy cat sleeps on the brown mat".into()),
    ]);
    table.insert(vec![
        Value::Int64(4),
        Value::Text("Hello world from Rust programming language".into()),
    ]);
    table
}

// =====================================================================
//  IndexMethod 枚举测试（3 条）
// =====================================================================

#[test]
fn index_method_from_str_ci() {
    assert_eq!(IndexMethod::from_str_ci("btree"), Some(IndexMethod::BTree));
    assert_eq!(IndexMethod::from_str_ci("BTREE"), Some(IndexMethod::BTree));
    assert_eq!(IndexMethod::from_str_ci("gist"), Some(IndexMethod::GiST));
    assert_eq!(IndexMethod::from_str_ci("GIST"), Some(IndexMethod::GiST));
    assert_eq!(IndexMethod::from_str_ci("gin"), Some(IndexMethod::Gin));
    assert_eq!(IndexMethod::from_str_ci("GIN"), Some(IndexMethod::Gin));
    assert_eq!(IndexMethod::from_str_ci("rtree"), Some(IndexMethod::RTree));
    assert_eq!(IndexMethod::from_str_ci("RTREE"), Some(IndexMethod::RTree));
    assert_eq!(IndexMethod::from_str_ci("fts5"), Some(IndexMethod::Fts5));
    assert_eq!(IndexMethod::from_str_ci("FTS5"), Some(IndexMethod::Fts5));
    assert_eq!(IndexMethod::from_str_ci("unknown"), None);
}

#[test]
fn index_method_as_str() {
    assert_eq!(IndexMethod::BTree.as_str(), "BTREE");
    assert_eq!(IndexMethod::GiST.as_str(), "GIST");
    assert_eq!(IndexMethod::Gin.as_str(), "GIN");
    assert_eq!(IndexMethod::RTree.as_str(), "RTREE");
    assert_eq!(IndexMethod::Fts5.as_str(), "FTS5");
}

#[test]
fn index_method_display() {
    assert_eq!(format!("{}", IndexMethod::BTree), "BTREE");
    assert_eq!(format!("{}", IndexMethod::GiST), "GIST");
    assert_eq!(format!("{}", IndexMethod::Gin), "GIN");
    assert_eq!(format!("{}", IndexMethod::RTree), "RTREE");
    assert_eq!(format!("{}", IndexMethod::Fts5), "FTS5");
}

// =====================================================================
//  Point2D + BoundingBox 测试（6 条）
// =====================================================================

#[test]
fn point2d_distance_to() {
    let p1 = Point2D::new(0.0, 0.0);
    let p2 = Point2D::new(3.0, 4.0);
    assert!((p1.distance_to(&p2) - 5.0).abs() < 1e-10);
    assert!((p2.distance_to(&p1) - 5.0).abs() < 1e-10);
    assert!((p1.distance_to(&p1) - 0.0).abs() < 1e-10);
}

#[test]
fn point2d_from_value_float64_array() {
    let val = Value::Array(vec![Value::Float64(1.5), Value::Float64(2.5)]);
    let p = Point2D::from_value(&val).unwrap();
    assert!((p.x - 1.5).abs() < 1e-10);
    assert!((p.y - 2.5).abs() < 1e-10);
}

#[test]
fn point2d_from_value_int64_array() {
    let val = Value::Array(vec![Value::Int64(3), Value::Int64(4)]);
    let p = Point2D::from_value(&val).unwrap();
    assert!((p.x - 3.0).abs() < 1e-10);
    assert!((p.y - 4.0).abs() < 1e-10);
}

#[test]
fn point2d_from_value_invalid_type() {
    let val = Value::Text("not a point".into());
    assert!(Point2D::from_value(&val).is_err());

    let val = Value::Array(vec![Value::Text("x".into()), Value::Float64(1.0)]);
    assert!(Point2D::from_value(&val).is_err());
}

#[test]
fn bbox_contains_and_intersects() {
    let bbox = BoundingBox {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 10.0,
        max_y: 10.0,
    };
    // 包含
    assert!(bbox.contains_point(&Point2D::new(5.0, 5.0)));
    assert!(bbox.contains_point(&Point2D::new(0.0, 0.0))); // 边界
    assert!(bbox.contains_point(&Point2D::new(10.0, 10.0))); // 边界
    assert!(!bbox.contains_point(&Point2D::new(11.0, 5.0)));
    assert!(!bbox.contains_point(&Point2D::new(5.0, -1.0)));

    // 相交
    let other = BoundingBox {
        min_x: 5.0,
        min_y: 5.0,
        max_x: 15.0,
        max_y: 15.0,
    };
    assert!(bbox.intersects(&other));

    let disjoint = BoundingBox {
        min_x: 20.0,
        min_y: 20.0,
        max_x: 30.0,
        max_y: 30.0,
    };
    assert!(!bbox.intersects(&disjoint));
}

#[test]
fn bbox_merge_and_area() {
    let mut bbox1 = BoundingBox {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 5.0,
        max_y: 5.0,
    };
    let bbox2 = BoundingBox {
        min_x: 3.0,
        min_y: 3.0,
        max_x: 10.0,
        max_y: 10.0,
    };
    bbox1.merge(&bbox2);
    assert_eq!(bbox1.min_x, 0.0);
    assert_eq!(bbox1.min_y, 0.0);
    assert_eq!(bbox1.max_x, 10.0);
    assert_eq!(bbox1.max_y, 10.0);
    assert!((bbox1.area() - 100.0).abs() < 1e-10);

    // 空边界框
    let empty = BoundingBox::empty();
    assert!(empty.is_empty());
    assert!((empty.area() - 0.0).abs() < 1e-10);
}

// =====================================================================
//  GIN 倒排索引测试（8 条）
// =====================================================================

#[test]
fn gin_build_and_lookup_single_term() {
    let table = make_tsvector_table();
    let mut index = GinIndex::new("gin_idx", "docs", "content");
    let count = index.build_from_table(&table, 1).unwrap();
    assert_eq!(count, 4); // 4 行被索引

    // "hello" 出现在行 0 和 1
    let hello_ids = index.lookup_term("hello");
    assert_eq!(hello_ids, vec![0, 1]);

    // "world" 出现在行 0 和 2
    let world_ids = index.lookup_term("world");
    assert_eq!(world_ids, vec![0, 2]);

    // "foo" 只出现在行 3
    let foo_ids = index.lookup_term("foo");
    assert_eq!(foo_ids, vec![3]);

    // "nonexistent" 不存在
    let none_ids = index.lookup_term("nonexistent");
    assert!(none_ids.is_empty());
}

#[test]
fn gin_search_tsquery_lexeme() {
    let table = make_tsvector_table();
    let mut index = GinIndex::new("gin_idx", "docs", "content");
    index.build_from_table(&table, 1).unwrap();

    let result = index.search_tsquery(&TsQuery::lexeme("hello"));
    assert_eq!(result, vec![0, 1]);

    let result = index.search_tsquery(&TsQuery::lexeme("world"));
    assert_eq!(result, vec![0, 2]);
}

#[test]
fn gin_search_tsquery_and() {
    let table = make_tsvector_table();
    let mut index = GinIndex::new("gin_idx", "docs", "content");
    index.build_from_table(&table, 1).unwrap();

    // hello AND world → 只有行 0
    let query = TsQuery::lexeme("hello").and(TsQuery::lexeme("world"));
    let result = index.search_tsquery(&query);
    assert_eq!(result, vec![0]);

    // hello AND rust → 只有行 1
    let query = TsQuery::lexeme("hello").and(TsQuery::lexeme("rust"));
    let result = index.search_tsquery(&query);
    assert_eq!(result, vec![1]);
}

#[test]
fn gin_search_tsquery_or() {
    let table = make_tsvector_table();
    let mut index = GinIndex::new("gin_idx", "docs", "content");
    index.build_from_table(&table, 1).unwrap();

    // hello OR foo → 行 0, 1, 3
    let query = TsQuery::lexeme("hello").or(TsQuery::lexeme("foo"));
    let result = index.search_tsquery(&query);
    assert_eq!(result, vec![0, 1, 3]);
}

#[test]
fn gin_search_tsquery_not() {
    let table = make_tsvector_table();
    let mut index = GinIndex::new("gin_idx", "docs", "content");
    index.build_from_table(&table, 1).unwrap();

    // NOT hello → 行 2, 3（不包含 hello 的行）
    let query = TsQuery::lexeme("hello").not_query();
    let result = index.search_tsquery(&query);
    assert_eq!(result, vec![2, 3]);
}

#[test]
fn gin_search_tsquery_followed_by() {
    let table = make_tsvector_table();
    let mut index = GinIndex::new("gin_idx", "docs", "content");
    index.build_from_table(&table, 1).unwrap();

    // hello FOLLOWED BY world → 简化为 AND → 行 0
    let query = TsQuery::FollowedBy {
        distance: 1,
        left: Box::new(TsQuery::lexeme("hello")),
        right: Box::new(TsQuery::lexeme("world")),
    };
    let result = index.search_tsquery(&query);
    assert_eq!(result, vec![0]);
}

#[test]
fn gin_empty_index() {
    let index = GinIndex::new("gin_idx", "docs", "content");
    assert!(index.is_empty());
    assert_eq!(index.len(), 0);
    assert!(index.lookup_term("anything").is_empty());
    assert!(index.all_row_ids().is_empty());
}

#[test]
fn gin_all_row_ids() {
    let table = make_tsvector_table();
    let mut index = GinIndex::new("gin_idx", "docs", "content");
    index.build_from_table(&table, 1).unwrap();

    let all_ids = index.all_row_ids();
    assert_eq!(all_ids, vec![0, 1, 2, 3]);
}

// =====================================================================
//  R-Tree 空间索引测试（8 条）
// =====================================================================

#[test]
fn rtree_build_from_table() {
    let table = make_points_table();
    let mut index = RTreeIndex::new("rtree_idx", "locations", "pos");
    let count = index.build_from_table(&table, 1).unwrap();
    assert_eq!(count, 10);
    assert_eq!(index.len(), 10);
    assert!(!index.is_empty());

    // 边界框应覆盖所有点
    let bounds = index.bounds().unwrap();
    assert!((bounds.min_x - 0.0).abs() < 1e-10);
    assert!((bounds.min_y - 0.0).abs() < 1e-10);
    assert!((bounds.max_x - 5.0).abs() < 1e-10);
    assert!((bounds.max_y - 5.0).abs() < 1e-10);
}

#[test]
fn rtree_range_query() {
    let table = make_points_table();
    let mut index = RTreeIndex::new("rtree_idx", "locations", "pos");
    index.build_from_table(&table, 1).unwrap();

    // 查询 [0, 2] × [0, 2] 范围内的点 → 行 0-8（不含 (5,5)）
    let bbox = BoundingBox {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 2.0,
        max_y: 2.0,
    };
    let result = index.range_query(&bbox);
    assert_eq!(result.len(), 9); // 9 个点在范围内

    // 查询仅包含 (5,5) 的范围
    let bbox = BoundingBox {
        min_x: 4.0,
        min_y: 4.0,
        max_x: 6.0,
        max_y: 6.0,
    };
    let result = index.range_query(&bbox);
    assert_eq!(result, vec![9]); // 行 9 = (5,5)
}

#[test]
fn rtree_knn_query() {
    let table = make_points_table();
    let mut index = RTreeIndex::new("rtree_idx", "locations", "pos");
    index.build_from_table(&table, 1).unwrap();

    // 查询距离 (1.0, 1.0) 最近的 3 个点
    let query = Point2D::new(1.0, 1.0);
    let result = index.knn_query(&query, 3);
    assert_eq!(result.len(), 3);

    // 最近的应该是 (1,1) 行 4，距离 0
    assert_eq!(result[0].0, 4); // row_id 4 = (1.0, 1.0)
    assert!((result[0].1 - 0.0).abs() < 1e-10);

    // 第二近的应该是 (0,1) 或 (2,1) 或 (1,0) 或 (1,2)，距离都是 1.0
    assert!((result[1].1 - 1.0).abs() < 1e-10);
}

#[test]
fn rtree_empty_index() {
    let index = RTreeIndex::new("rtree_idx", "locations", "pos");
    assert!(index.is_empty());
    assert_eq!(index.len(), 0);
    assert!(index.bounds().is_none());

    let bbox = BoundingBox {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 10.0,
        max_y: 10.0,
    };
    assert!(index.range_query(&bbox).is_empty());

    let result = index.knn_query(&Point2D::new(0.0, 0.0), 5);
    assert!(result.is_empty());
}

#[test]
fn rtree_range_query_no_match() {
    let table = make_points_table();
    let mut index = RTreeIndex::new("rtree_idx", "locations", "pos");
    index.build_from_table(&table, 1).unwrap();

    // 查询远离所有点的范围
    let bbox = BoundingBox {
        min_x: 100.0,
        min_y: 100.0,
        max_x: 200.0,
        max_y: 200.0,
    };
    let result = index.range_query(&bbox);
    assert!(result.is_empty());
}

#[test]
fn rtree_large_dataset() {
    // 创建 1000 个点
    let mut table = InMemoryTable::with_columns(
        "large",
        vec![
            ("id", ColumnType::Int64),
            ("pos", ColumnType::Array(Box::new(ColumnType::Float64))),
        ],
    );
    for i in 0..1000i64 {
        let x = (i % 100) as f64;
        let y = (i / 100) as f64;
        table.insert(vec![
            Value::Int64(i),
            Value::Array(vec![Value::Float64(x), Value::Float64(y)]),
        ]);
    }

    let mut index = RTreeIndex::new("rtree_idx", "large", "pos");
    let count = index.build_from_table(&table, 1).unwrap();
    assert_eq!(count, 1000);

    // 范围查询 [0, 10] × [0, 5]
    let bbox = BoundingBox {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 10.0,
        max_y: 5.0,
    };
    let result = index.range_query(&bbox);
    // x: 0..=10 (11 values), y: 0..=5 (6 values) → 11 × 6 = 66
    assert_eq!(result.len(), 66);

    // k-NN 查询
    let knn_result = index.knn_query(&Point2D::new(50.0, 5.0), 5);
    assert_eq!(knn_result.len(), 5);
    // 距离应按升序排列
    for i in 1..knn_result.len() {
        assert!(knn_result[i - 1].1 <= knn_result[i].1);
    }
}

#[test]
fn rtree_knn_returns_all_when_k_exceeds() {
    let table = make_points_table();
    let mut index = RTreeIndex::new("rtree_idx", "locations", "pos");
    index.build_from_table(&table, 1).unwrap();

    // k > 总点数 → 返回所有点
    let result = index.knn_query(&Point2D::new(0.0, 0.0), 100);
    assert_eq!(result.len(), 10); // 只有 10 个点
}

#[test]
fn rtree_skips_null_values() {
    let mut table = InMemoryTable::with_columns(
        "nullable",
        vec![
            ("id", ColumnType::Int64),
            ("pos", ColumnType::Array(Box::new(ColumnType::Float64))),
        ],
    );
    table.insert(vec![
        Value::Int64(1),
        Value::Array(vec![Value::Float64(1.0), Value::Float64(2.0)]),
    ]);
    table.insert(vec![Value::Int64(2), Value::Null]);
    table.insert(vec![
        Value::Int64(3),
        Value::Array(vec![Value::Float64(3.0), Value::Float64(4.0)]),
    ]);

    let mut index = RTreeIndex::new("rtree_idx", "nullable", "pos");
    let count = index.build_from_table(&table, 1).unwrap();
    assert_eq!(count, 2); // NULL 被跳过
    assert_eq!(index.len(), 2);
}

// =====================================================================
//  GiST k-NN 索引测试（5 条）
// =====================================================================

#[test]
fn gist_build_from_table() {
    let table = make_points_table();
    let mut index = GistIndex::new("gist_idx", "locations", "pos");
    let count = index.build_from_table(&table, 1).unwrap();
    assert_eq!(count, 10);
    assert_eq!(index.len(), 10);
    assert!(!index.is_empty());
}

#[test]
fn gist_knn_query() {
    let table = make_points_table();
    let mut index = GistIndex::new("gist_idx", "locations", "pos");
    index.build_from_table(&table, 1).unwrap();

    // 查询距离 (0.0, 0.0) 最近的 3 个点
    let result = index.knn(&Point2D::new(0.0, 0.0), 3);
    assert_eq!(result.len(), 3);

    // 最近的是 (0,0) 行 0，距离 0
    assert_eq!(result[0].0, 0);
    assert!((result[0].1 - 0.0).abs() < 1e-10);

    // 距离按升序排列
    for i in 1..result.len() {
        assert!(result[i - 1].1 <= result[i].1);
    }
}

#[test]
fn gist_range_query() {
    let table = make_points_table();
    let mut index = GistIndex::new("gist_idx", "locations", "pos");
    index.build_from_table(&table, 1).unwrap();

    let bbox = BoundingBox {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 1.0,
        max_y: 1.0,
    };
    let result = index.range_query(&bbox);
    // (0,0), (1,0), (0,1), (1,1) → 4 个点
    assert_eq!(result.len(), 4);
}

#[test]
fn gist_empty_index() {
    let index = GistIndex::new("gist_idx", "locations", "pos");
    assert!(index.is_empty());
    assert_eq!(index.len(), 0);

    let result = index.knn(&Point2D::new(0.0, 0.0), 5);
    assert!(result.is_empty());
}

#[test]
fn gist_knn_distance_ordering() {
    let table = make_points_table();
    let mut index = GistIndex::new("gist_idx", "locations", "pos");
    index.build_from_table(&table, 1).unwrap();

    // 查询距离 (5.0, 5.0) 最近的 5 个点
    let result = index.knn(&Point2D::new(5.0, 5.0), 5);
    assert_eq!(result.len(), 5);

    // 第一个应该是 (5,5) 行 9，距离 0
    assert_eq!(result[0].0, 9);
    assert!((result[0].1 - 0.0).abs() < 1e-10);

    // 距离严格升序（允许相等）
    for i in 1..result.len() {
        assert!(result[i - 1].1 <= result[i].1 + 1e-10);
    }
}

// =====================================================================
//  FTS5 全文检索测试（8 条）
// =====================================================================

#[test]
fn fts5_tokenize() {
    let tokens = Fts5Index::tokenize("Hello, World! foo_bar");
    assert_eq!(tokens.len(), 3);
    assert_eq!(tokens[0].0, "hello");
    assert_eq!(tokens[0].1, 0);
    assert_eq!(tokens[1].0, "world");
    assert_eq!(tokens[1].1, 1);
    assert_eq!(tokens[2].0, "foo_bar");
    assert_eq!(tokens[2].1, 2);

    // 空字符串
    let empty_tokens = Fts5Index::tokenize("");
    assert!(empty_tokens.is_empty());

    // 仅标点
    let punct_tokens = Fts5Index::tokenize("!!! ??? ...");
    assert!(punct_tokens.is_empty());
}

#[test]
fn fts5_build_from_table() {
    let table = make_text_table();
    let mut index = Fts5Index::new("fts5_idx", "articles", "body");
    let count = index.build_from_table(&table, 1).unwrap();
    assert_eq!(count, 4);
    assert!(!index.is_empty());
}

#[test]
fn fts5_match_single_word() {
    let table = make_text_table();
    let mut index = Fts5Index::new("fts5_idx", "articles", "body");
    index.build_from_table(&table, 1).unwrap();

    // "quick" 出现在行 0 和 1
    let result = index.match_query("quick");
    assert_eq!(result, vec![0, 1]);

    // "rust" 只出现在行 3
    let result = index.match_query("rust");
    assert_eq!(result, vec![3]);

    // "nonexistent" 不存在
    let result = index.match_query("nonexistent");
    assert!(result.is_empty());
}

#[test]
fn fts5_match_multiple_words() {
    let table = make_text_table();
    let mut index = Fts5Index::new("fts5_idx", "articles", "body");
    index.build_from_table(&table, 1).unwrap();

    // "quick brown" → AND 语义 → 行 0 和 1 都包含 quick 和 brown
    let result = index.match_query("quick brown");
    assert_eq!(result, vec![0, 1]);

    // "quick hello" → 行 1 不含 hello，只有行 0 同时包含（但行 0 不含 hello）
    // 实际上 hello 在行 3，quick 在行 0 和 1 → 无交集
    let result = index.match_query("quick hello");
    assert!(result.is_empty());
}

#[test]
fn fts5_phrase_query() {
    let table = make_text_table();
    let mut index = Fts5Index::new("fts5_idx", "articles", "body");
    index.build_from_table(&table, 1).unwrap();

    // "quick brown" 作为短语 → 位置相邻
    // 行 0: "The quick(1) brown(2) fox(3)..." → quick 和 brown 相邻
    // 行 1: "A quick(1) brown(2) dog(3)..." → quick 和 brown 相邻
    let result = index.match_query("\"quick brown\"");
    assert_eq!(result, vec![0, 1]);

    // "brown quick" 作为短语 → 位置不相邻（顺序反了）
    let result = index.match_query("\"brown quick\"");
    assert!(result.is_empty());

    // "the lazy" 作为短语
    // 行 0: "...over the(6) lazy(7) dog(8)" → 相邻
    // 行 2: "The(1) lazy(2) cat(3)..." → 相邻
    let result = index.match_query("\"the lazy\"");
    assert_eq!(result, vec![0, 2]);
}

#[test]
fn fts5_empty_query() {
    let table = make_text_table();
    let mut index = Fts5Index::new("fts5_idx", "articles", "body");
    index.build_from_table(&table, 1).unwrap();

    let result = index.match_query("");
    assert!(result.is_empty());

    let result = index.match_query("   ");
    assert!(result.is_empty());
}

#[test]
fn fts5_lookup_term() {
    let table = make_text_table();
    let mut index = Fts5Index::new("fts5_idx", "articles", "body");
    index.build_from_table(&table, 1).unwrap();

    // "the" 出现在行 0, 1, 2（行 3 无 "the"）
    let result = index.lookup_term("the");
    assert!(result.contains(&0));
    assert!(result.contains(&1));
    assert!(result.contains(&2));
    assert!(!result.contains(&3));

    // "fox" 只出现在行 0
    let result = index.lookup_term("fox");
    assert_eq!(result, vec![0]);
}

#[test]
fn fts5_all_row_ids() {
    let table = make_text_table();
    let mut index = Fts5Index::new("fts5_idx", "articles", "body");
    index.build_from_table(&table, 1).unwrap();

    let all_ids = index.all_row_ids();
    assert_eq!(all_ids, vec![0, 1, 2, 3]);
}

// =====================================================================
//  build_from_table 类型错误测试（4 条）
// =====================================================================

#[test]
fn gin_build_from_table_wrong_type() {
    // GIN 索引要求 TsVector 列，传 Text 列应报错
    let table = make_text_table();
    let mut index = GinIndex::new("gin_idx", "articles", "body");
    let result = index.build_from_table(&table, 1);
    assert!(result.is_err());
}

#[test]
fn rtree_build_from_table_wrong_type() {
    // R-Tree 索引要求 Array 列，传 Text 列应报错
    let table = make_text_table();
    let mut index = RTreeIndex::new("rtree_idx", "articles", "body");
    let result = index.build_from_table(&table, 1);
    assert!(result.is_err());
}

#[test]
fn fts5_build_from_table_wrong_type() {
    // FTS5 索引要求 Text 列，传 TsVector 列应报错
    let table = make_tsvector_table();
    let mut index = Fts5Index::new("fts5_idx", "docs", "content");
    let result = index.build_from_table(&table, 1);
    assert!(result.is_err());
}

#[test]
fn gist_build_from_wrong_type() {
    // GiST 索引要求 Array 列，传 Text 列应报错
    let table = make_text_table();
    let mut index = GistIndex::new("gist_idx", "articles", "body");
    let result = index.build_from_table(&table, 1);
    assert!(result.is_err());
}
