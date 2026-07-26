//! Phase 4.7 元数据查询 — pg_catalog + information_schema 子集。
//!
//! # 设计目标
//!
//! 支持 DBeaver / DataGrip 等数据库工具连接后自动列出数据库/表/列信息。
//! 通过拦截 SELECT 语句中的系统表引用，直接计算结果集返回，无需注册虚拟表。
//!
//! # 支持的系统表
//!
//! - `pg_catalog.pg_tables` / `pg_tables` — 表清单
//! - `pg_catalog.pg_indexes` / `pg_indexes` — 索引清单
//! - `information_schema.tables` — 表清单（ANSI SQL 标准）
//! - `information_schema.columns` — 列清单（ANSI SQL 标准）
//! - `information_schema.table_constraints` — 约束清单
//! - `information_schema.referential_constraints` — 外键约束详情
//!
//! # 查询支持范围
//!
//! - `SELECT *` / `SELECT <cols>` FROM <system_table>
//! - 可选 `WHERE <col> = <literal> [AND ...]`（简单等值过滤）
//! - 可选 `ORDER BY <col> [ASC|DESC]`（单列排序）
//! - 可选 `LIMIT <n> [OFFSET <n>]`
//!
//! # 设计决策
//!
//! 直接在 `ExecutorService::execute_statement` 中拦截系统表查询，而不通过
//! Planner + Executor 路径。原因：
//! 1. 系统表数据由 `szrsql-catalog` 模块计算，需要 `MutableCatalog` 参数
//! 2. 会话级 `InMemoryCatalog` 不实现 `MutableCatalog`，需要 adapter
//! 3. 避免在 `InMemoryCatalog` 中注册虚拟表 Schema（污染用户表空间）
//! 4. 保持系统表只读语义，防止 DML 误操作

use crate::pgwire::session::{QueryResult, ResultColumn, SessionError};
use szrsql_catalog::{information_schema, system_tables, CatalogError, IndexInfo, MutableCatalog};
use szrsql_sql::ast::{Expr, OrderByExpr, Select, SelectItem, Statement, TableFactor, TableName};
use szrsql_sql::plan::{Catalog, ForeignKeyConstraint, ReferencingKey, TableSchema};
use szrsql_types::value::Value;

// =====================================================================
//  CatalogAdapter — 只读 MutableCatalog 适配器
// =====================================================================

/// 只读 Catalog 适配器 — 包装 `InMemoryCatalog` 以实现 `MutableCatalog`。
///
/// 由于会话级 `InMemoryCatalog`（来自 `szrsql-sql`）不跟踪索引元数据，
/// 索引相关方法返回空结果。写方法（create/drop）始终返回错误。
pub struct CatalogAdapter<'a> {
    catalog: &'a szrsql_sql::plan::InMemoryCatalog,
}

impl<'a> CatalogAdapter<'a> {
    pub fn new(catalog: &'a szrsql_sql::plan::InMemoryCatalog) -> Self {
        Self { catalog }
    }
}

impl<'a> Catalog for CatalogAdapter<'a> {
    fn table_exists(&self, name: &TableName) -> bool {
        self.catalog.table_exists(name)
    }

    fn get_table(&self, name: &TableName) -> Option<TableSchema> {
        self.catalog.get_table(name)
    }

    fn list_tables(&self) -> Vec<TableName> {
        self.catalog.list_tables()
    }

    fn sequence_exists(&self, name: &TableName) -> bool {
        self.catalog.sequence_exists(name)
    }

    fn get_sequence(&self, name: &TableName) -> Option<szrsql_sql::plan::SequenceDefinition> {
        self.catalog.get_sequence(name)
    }

    fn list_sequences(&self) -> Vec<TableName> {
        self.catalog.list_sequences()
    }

    fn get_foreign_keys(&self, name: &TableName) -> Vec<ForeignKeyConstraint> {
        self.catalog.get_foreign_keys(name)
    }

    fn get_referencing_keys(&self, name: &TableName) -> Vec<ReferencingKey> {
        self.catalog.get_referencing_keys(name)
    }

    fn get_check_constraints(&self, name: &TableName) -> Vec<szrsql_sql::plan::CheckConstraint> {
        self.catalog.get_check_constraints(name)
    }

    fn enum_type_exists(&self, name: &TableName) -> bool {
        self.catalog.enum_type_exists(name)
    }

    fn get_enum_type(&self, name: &TableName) -> Option<szrsql_sql::plan::EnumTypeDefinition> {
        self.catalog.get_enum_type(name)
    }

    fn list_enum_types(&self) -> Vec<TableName> {
        self.catalog.list_enum_types()
    }
}

impl<'a> MutableCatalog for CatalogAdapter<'a> {
    fn create_table(
        &mut self,
        _schema: TableSchema,
        _if_not_exists: bool,
    ) -> Result<(), CatalogError> {
        Err(CatalogError::InvalidArgument(
            "CatalogAdapter is read-only".into(),
        ))
    }

    fn drop_table(
        &mut self,
        _name: &TableName,
        _if_exists: bool,
        _cascade: bool,
    ) -> Result<(), CatalogError> {
        Err(CatalogError::InvalidArgument(
            "CatalogAdapter is read-only".into(),
        ))
    }

    fn create_index(
        &mut self,
        _index: IndexInfo,
        _if_not_exists: bool,
    ) -> Result<(), CatalogError> {
        Err(CatalogError::InvalidArgument(
            "CatalogAdapter is read-only".into(),
        ))
    }

    fn drop_index(&mut self, _name: &str, _if_exists: bool) -> Result<(), CatalogError> {
        Err(CatalogError::InvalidArgument(
            "CatalogAdapter is read-only".into(),
        ))
    }

    fn list_indexes(&self) -> Vec<IndexInfo> {
        Vec::new()
    }

    fn list_indexes_for_table(&self, _table: &TableName) -> Vec<IndexInfo> {
        Vec::new()
    }

    fn get_index(&self, _name: &str) -> Option<IndexInfo> {
        None
    }
}

// =====================================================================
//  系统表标识
// =====================================================================

/// 系统表类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemTableKind {
    /// `pg_tables`
    PgTables,
    /// `pg_indexes`
    PgIndexes,
    /// `information_schema.tables`
    InfoSchemaTables,
    /// `information_schema.columns`
    InfoSchemaColumns,
    /// `information_schema.table_constraints`
    InfoSchemaTableConstraints,
    /// `information_schema.referential_constraints`
    InfoSchemaReferentialConstraints,
}

impl SystemTableKind {
    /// 根据表名识别系统表类型（大小写不敏感）
    ///
    /// 匹配规则：
    /// - `pg_tables` / `pg_catalog.pg_tables` → PgTables
    /// - `pg_indexes` / `pg_catalog.pg_indexes` → PgIndexes
    /// - `information_schema.tables` → InfoSchemaTables
    /// - `information_schema.columns` → InfoSchemaColumns
    /// - `information_schema.table_constraints` → InfoSchemaTableConstraints
    /// - `information_schema.referential_constraints` → InfoSchemaReferentialConstraints
    pub fn from_name(name: &TableName) -> Option<Self> {
        let lower_name = name.name.to_lowercase();
        let lower_schema = name.schema.as_ref().map(|s| s.to_lowercase());
        match (lower_schema.as_deref(), lower_name.as_str()) {
            (Some("pg_catalog"), "pg_tables") | (None, "pg_tables") => Some(Self::PgTables),
            (Some("pg_catalog"), "pg_indexes") | (None, "pg_indexes") => Some(Self::PgIndexes),
            (Some("information_schema"), "tables") => Some(Self::InfoSchemaTables),
            (Some("information_schema"), "columns") => Some(Self::InfoSchemaColumns),
            (Some("information_schema"), "table_constraints") => {
                Some(Self::InfoSchemaTableConstraints)
            }
            (Some("information_schema"), "referential_constraints") => {
                Some(Self::InfoSchemaReferentialConstraints)
            }
            _ => None,
        }
    }

    /// 返回该系统表的列 Schema
    pub fn schema(self) -> TableSchema {
        match self {
            Self::PgTables => system_tables::pg_tables_schema(),
            Self::PgIndexes => system_tables::pg_indexes_schema(),
            Self::InfoSchemaTables => information_schema::tables_schema(),
            Self::InfoSchemaColumns => information_schema::columns_schema(),
            Self::InfoSchemaTableConstraints => information_schema::table_constraints_schema(),
            Self::InfoSchemaReferentialConstraints => {
                information_schema::referential_constraints_schema()
            }
        }
    }

    /// 返回该系统表的列名列表
    pub fn column_names(self) -> Vec<String> {
        self.schema().columns.into_iter().map(|c| c.name).collect()
    }

    /// 计算该系统表的所有行
    pub fn compute_rows(self, catalog: &dyn MutableCatalog) -> Vec<Vec<Value>> {
        match self {
            Self::PgTables => system_tables::pg_tables(catalog),
            Self::PgIndexes => system_tables::pg_indexes(catalog),
            Self::InfoSchemaTables => information_schema::tables(catalog),
            Self::InfoSchemaColumns => information_schema::columns(catalog),
            Self::InfoSchemaTableConstraints => information_schema::table_constraints(catalog),
            Self::InfoSchemaReferentialConstraints => {
                information_schema::referential_constraints(catalog)
            }
        }
    }
}

// =====================================================================
//  SELECT 拦截入口
// =====================================================================

/// 尝试将 SELECT 语句作为系统表查询执行。
///
/// 若语句是 `SELECT ... FROM <single_system_table>`（无 JOIN、无集合操作），
/// 则计算系统表数据并应用 WHERE/ORDER BY/LIMIT，返回 `Some(Ok(result))`。
///
/// 若不是系统表查询，返回 `None`（交由正常 Planner 路径处理）。
///
/// 限制（简化实现，覆盖 DBeaver 等工具的基本元数据浏览场景）：
/// - 仅支持单表查询（无 JOIN）
/// - WHERE 仅支持 `col = literal` 的 AND 组合
/// - ORDER BY 仅支持单列
/// - 不支持 GROUP BY / HAVING / 集合操作
pub fn try_execute_system_table_query(
    stmt: &Statement,
    catalog: &szrsql_sql::plan::InMemoryCatalog,
) -> Option<Result<QueryResult, SessionError>> {
    let select = match stmt {
        Statement::Select(s) if s.set_op.is_none() => s.as_ref(),
        _ => return None,
    };

    // 仅支持单表 SELECT（无 JOIN）
    if select.from.len() != 1 || !select.from[0].joins.is_empty() {
        return None;
    }
    let table_name = match &select.from[0].relation {
        TableFactor::Table { name, .. } => name,
        _ => return None,
    };

    let kind = SystemTableKind::from_name(table_name)?;

    // 不支持 GROUP BY / HAVING / DISTINCT（系统表查询一般不需要）
    if !select.group_by.is_empty() || select.having.is_some() || select.distinct {
        return None;
    }

    Some(execute_system_table_select(select, kind, catalog))
}

/// 执行系统表 SELECT（已通过前置检查）
fn execute_system_table_select(
    select: &Select,
    kind: SystemTableKind,
    catalog: &szrsql_sql::plan::InMemoryCatalog,
) -> Result<QueryResult, SessionError> {
    let adapter = CatalogAdapter::new(catalog);
    let schema = kind.schema();
    let column_names = kind.column_names();
    let mut rows = kind.compute_rows(&adapter);

    // 1. WHERE 过滤
    if let Some(where_expr) = &select.where_clause {
        rows.retain(|row| eval_where_predicate(where_expr, &column_names, row));
    }

    // 2. ORDER BY 排序（单列）
    if select.order_by.len() == 1 {
        let order_expr = &select.order_by[0];
        apply_order_by(&mut rows, &column_names, order_expr);
    } else if select.order_by.len() > 1 {
        return Err(SessionError::Protocol(
            "system table query supports only single-column ORDER BY".into(),
        ));
    }

    // 3. OFFSET 跳过
    if let Some(offset_expr) = &select.offset {
        let offset = eval_literal_int(offset_expr)? as usize;
        if offset >= rows.len() {
            rows.clear();
        } else {
            rows.drain(..offset);
        }
    }

    // 4. LIMIT 截断
    if let Some(limit_expr) = &select.limit {
        let limit = eval_literal_int(limit_expr)? as usize;
        if limit < rows.len() {
            rows.truncate(limit);
        }
    }

    // 5. 投影列
    let (columns, projected_rows) = project_columns(&select.projection, &schema, &rows)?;

    let tag = format!("SELECT {}", projected_rows.len());
    Ok(QueryResult::ResultSet {
        columns,
        rows: projected_rows,
        tag,
    })
}

// =====================================================================
//  WHERE 求值（简单等值过滤）
// =====================================================================

/// 评估 WHERE 谓词是否匹配行。
///
/// 支持的形式：
/// - `col = literal`（单条件）
/// - `cond1 AND cond2 AND ...`（多条件组合）
/// - 其他形式返回 true（不过滤，避免误删数据）
fn eval_where_predicate(expr: &Expr, column_names: &[String], row: &[Value]) -> bool {
    match expr {
        Expr::BinaryOp {
            left,
            op: szrsql_sql::ast::BinaryOp::Eq,
            right,
        } => eval_eq_condition(left, right, column_names, row),
        Expr::BinaryOp {
            left,
            op: szrsql_sql::ast::BinaryOp::And,
            right,
        } => {
            eval_where_predicate(left, column_names, row)
                && eval_where_predicate(right, column_names, row)
        }
        // 其他形式：保守返回 true（不过滤）
        _ => true,
    }
}

/// 评估 `col = literal` 条件
fn eval_eq_condition(left: &Expr, right: &Expr, column_names: &[String], row: &[Value]) -> bool {
    let (col_idx, literal) = match (
        extract_column_index(left, column_names),
        extract_literal(right),
    ) {
        (Some(idx), Some(val)) => (idx, val),
        (Some(idx), None) => {
            // 左边是列，右边不是字面量；尝试反向
            match extract_literal(right) {
                Some(val) => (idx, val),
                None => return true,
            }
        }
        (None, _) => {
            // 左边不是列；尝试反向（right 是列，left 是字面量）
            match (
                extract_column_index(right, column_names),
                extract_literal(left),
            ) {
                (Some(idx), Some(val)) => (idx, val),
                _ => return true,
            }
        }
    };

    col_idx < row.len() && values_equal(&row[col_idx], &literal)
}

/// 从表达式中提取列索引
fn extract_column_index(expr: &Expr, column_names: &[String]) -> Option<usize> {
    match expr {
        Expr::Identifier(idents) => {
            if idents.len() == 1 {
                let name = idents[0].to_lowercase();
                column_names.iter().position(|c| c.to_lowercase() == name)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 从表达式中提取字面量值
fn extract_literal(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Literal(v) => Some(v.clone()),
        _ => None,
    }
}

/// 值相等比较（大小写不敏感比较 Text）
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Text(s1), Value::Text(s2)) => s1.eq_ignore_ascii_case(s2),
        _ => a == b,
    }
}

// =====================================================================
//  ORDER BY 排序
// =====================================================================

/// 应用单列 ORDER BY 排序
fn apply_order_by(rows: &mut [Vec<Value>], column_names: &[String], order: &OrderByExpr) {
    let col_idx = match extract_column_index(&order.expr, column_names) {
        Some(idx) => idx,
        None => return,
    };
    let ascending = order.asc;
    rows.sort_by(|a, b| {
        let cmp = compare_values(a.get(col_idx), b.get(col_idx));
        if ascending {
            cmp
        } else {
            cmp.reverse()
        }
    });
}

/// 值比较（用于排序）
fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(v1), Some(v2)) => match (v1, v2) {
            (Value::Int64(n1), Value::Int64(n2)) => n1.cmp(n2),
            (Value::Float64(n1), Value::Float64(n2)) => {
                n1.partial_cmp(n2).unwrap_or(Ordering::Equal)
            }
            (Value::Text(s1), Value::Text(s2)) => s1.cmp(s2),
            (Value::Bool(b1), Value::Bool(b2)) => b1.cmp(b2),
            _ => Ordering::Equal,
        },
    }
}

// =====================================================================
//  LIMIT / OFFSET 字面量求值
// =====================================================================

/// 从表达式求值为 i64 整数（仅支持字面量）
fn eval_literal_int(expr: &Expr) -> Result<i64, SessionError> {
    match expr {
        Expr::Literal(Value::Int64(n)) => Ok(*n),
        Expr::Literal(Value::Float64(n)) => Ok(*n as i64),
        _ => Err(SessionError::Protocol(format!(
            "system table LIMIT/OFFSET requires integer literal, got {:?}",
            expr
        ))),
    }
}

// =====================================================================
//  投影列
// =====================================================================

/// 根据 SELECT 投影列表生成结果列与行
fn project_columns(
    projection: &[SelectItem],
    schema: &TableSchema,
    rows: &[Vec<Value>],
) -> Result<(Vec<ResultColumn>, Vec<Vec<Value>>), SessionError> {
    let all_columns: Vec<ResultColumn> = schema
        .columns
        .iter()
        .map(|c| ResultColumn {
            name: c.name.clone(),
            column_type: c.data_type.clone(),
        })
        .collect();

    // 处理通配符
    let has_wildcard = projection
        .iter()
        .any(|p| matches!(p, SelectItem::Wildcard | SelectItem::QualifiedWildcard(_)));

    if has_wildcard {
        if projection.len() > 1 {
            return Err(SessionError::Protocol(
                "system table query does not support mixing * with other projections".into(),
            ));
        }
        let mut projected_rows = Vec::with_capacity(rows.len());
        for row in rows {
            projected_rows.push(row.clone());
        }
        return Ok((all_columns, projected_rows));
    }

    // 处理指定列
    let mut col_indices: Vec<usize> = Vec::with_capacity(projection.len());
    let mut result_columns: Vec<ResultColumn> = Vec::with_capacity(projection.len());
    for item in projection {
        let (expr, alias) = match item {
            SelectItem::UnnamedExpr(e) => (e, None),
            SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.clone())),
            _ => {
                return Err(SessionError::Protocol(
                    "unsupported projection in system table query".into(),
                ))
            }
        };
        let col_name = match expr {
            Expr::Identifier(idents) if idents.len() == 1 => idents[0].clone(),
            _ => {
                return Err(SessionError::Protocol(format!(
                    "unsupported projection expression in system table query: {:?}",
                    expr
                )))
            }
        };
        let idx = schema
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(&col_name))
            .ok_or_else(|| {
                SessionError::Protocol(format!("column \"{col_name}\" not found in system table"))
            })?;
        col_indices.push(idx);
        let output_name = alias.unwrap_or_else(|| schema.columns[idx].name.clone());
        result_columns.push(ResultColumn {
            name: output_name,
            column_type: schema.columns[idx].data_type.clone(),
        });
    }

    let mut projected_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let new_row: Vec<Value> = col_indices.iter().map(|&i| row[i].clone()).collect();
        projected_rows.push(new_row);
    }
    Ok((result_columns, projected_rows))
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_sql::plan::InMemoryCatalog;
    use szrsql_types::value::ColumnType;

    fn make_catalog_with_tables() -> InMemoryCatalog {
        let mut cat = InMemoryCatalog::new();
        cat.add_simple_table(
            "users",
            vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
        );
        cat.add_simple_table(
            "orders",
            vec![("id", ColumnType::Int64), ("user_id", ColumnType::Int64)],
        );
        cat
    }

    #[test]
    fn test_catalog_adapter_delegates_list_tables() {
        let cat = make_catalog_with_tables();
        let adapter = CatalogAdapter::new(&cat);
        let mut tables = adapter.list_tables();
        tables.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].name, "orders");
        assert_eq!(tables[1].name, "users");
    }

    #[test]
    fn test_catalog_adapter_stubs_index_methods() {
        let cat = make_catalog_with_tables();
        let adapter = CatalogAdapter::new(&cat);
        assert!(MutableCatalog::list_indexes(&adapter).is_empty());
        assert!(adapter
            .list_indexes_for_table(&TableName::new("users"))
            .is_empty());
        assert!(adapter.get_index("idx_anything").is_none());
    }

    #[test]
    fn test_catalog_adapter_rejects_writes() {
        let cat = make_catalog_with_tables();
        let mut adapter = CatalogAdapter::new(&cat);
        let schema = TableSchema {
            name: TableName::new("new_table"),
            columns: vec![],
        };
        let result = adapter.create_table(schema, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_system_table_kind_from_name_pg_tables() {
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_tables")),
            Some(SystemTableKind::PgTables)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::with_schema("pg_catalog", "pg_tables")),
            Some(SystemTableKind::PgTables)
        );
    }

    #[test]
    fn test_system_table_kind_from_name_information_schema() {
        assert_eq!(
            SystemTableKind::from_name(&TableName::with_schema("information_schema", "tables")),
            Some(SystemTableKind::InfoSchemaTables)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::with_schema("information_schema", "columns")),
            Some(SystemTableKind::InfoSchemaColumns)
        );
    }

    #[test]
    fn test_system_table_kind_from_name_case_insensitive() {
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("PG_TABLES")),
            Some(SystemTableKind::PgTables)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("Pg_Tables")),
            Some(SystemTableKind::PgTables)
        );
    }

    #[test]
    fn test_system_table_kind_from_name_unknown_returns_none() {
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_class")),
            None
        );
        assert_eq!(SystemTableKind::from_name(&TableName::new("users")), None);
    }

    #[test]
    fn test_pg_tables_compute_rows() {
        let cat = make_catalog_with_tables();
        let adapter = CatalogAdapter::new(&cat);
        let rows = SystemTableKind::PgTables.compute_rows(&adapter);
        assert_eq!(rows.len(), 2);
        // 每行：schemaname, tablename, tableowner, hasindexes
        let names: Vec<String> = rows
            .iter()
            .map(|r| match &r[1] {
                Value::Text(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(names.contains(&"users".into()));
        assert!(names.contains(&"orders".into()));
    }

    #[test]
    fn test_info_schema_tables_compute_rows() {
        let cat = make_catalog_with_tables();
        let adapter = CatalogAdapter::new(&cat);
        let rows = SystemTableKind::InfoSchemaTables.compute_rows(&adapter);
        assert_eq!(rows.len(), 2);
        // 每行：TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE
        for row in &rows {
            assert!(matches!(&row[3], Value::Text(s) if s == "BASE TABLE"));
        }
    }

    #[test]
    fn test_info_schema_columns_compute_rows() {
        let cat = make_catalog_with_tables();
        let adapter = CatalogAdapter::new(&cat);
        let rows = SystemTableKind::InfoSchemaColumns.compute_rows(&adapter);
        // users: 2 cols + orders: 2 cols = 4 rows
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn test_try_execute_returns_none_for_user_table() {
        let cat = make_catalog_with_tables();
        let stmts =
            szrsql_sql::parser::parse_sql("SELECT * FROM users").expect("parse should succeed");
        assert_eq!(stmts.len(), 1);
        let result = try_execute_system_table_query(&stmts[0], &cat);
        assert!(result.is_none());
    }

    #[test]
    fn test_try_execute_returns_some_for_pg_tables() {
        let cat = make_catalog_with_tables();
        let stmts =
            szrsql_sql::parser::parse_sql("SELECT * FROM pg_tables").expect("parse should succeed");
        assert_eq!(stmts.len(), 1);
        let result = try_execute_system_table_query(&stmts[0], &cat);
        assert!(result.is_some());
        let inner = result.unwrap().expect("should be Ok");
        match inner {
            QueryResult::ResultSet { columns, rows, tag } => {
                assert_eq!(columns.len(), 4);
                assert_eq!(rows.len(), 2);
                assert!(tag.starts_with("SELECT"));
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_with_where_filter() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM pg_tables WHERE tablename = 'users'";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { rows, .. } => {
                assert_eq!(rows.len(), 1);
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_with_order_by() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM pg_tables ORDER BY tablename DESC";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { rows, .. } => {
                assert_eq!(rows.len(), 2);
                // DESC 排序：users 在前，orders 在后
                let first_name = match &rows[0][1] {
                    Value::Text(s) => s.clone(),
                    _ => String::new(),
                };
                assert_eq!(first_name, "users");
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_with_limit() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM pg_tables LIMIT 1";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { rows, .. } => {
                assert_eq!(rows.len(), 1);
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_information_schema_tables() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM information_schema.tables";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { columns, rows, .. } => {
                assert_eq!(columns.len(), 4);
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_information_schema_columns() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM information_schema.columns";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { columns, rows, .. } => {
                assert_eq!(columns.len(), 11);
                assert_eq!(rows.len(), 4);
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_specific_columns() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT tablename, tableowner FROM pg_tables";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { columns, rows, .. } => {
                assert_eq!(columns.len(), 2);
                assert_eq!(columns[0].name, "tablename");
                assert_eq!(columns[1].name, "tableowner");
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_column_alias() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT tablename AS name FROM pg_tables";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { columns, .. } => {
                assert_eq!(columns.len(), 1);
                assert_eq!(columns[0].name, "name");
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_offset() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM pg_tables OFFSET 1";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { rows, .. } => {
                assert_eq!(rows.len(), 1);
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_limit_offset() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM pg_tables LIMIT 1 OFFSET 1";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { rows, .. } => {
                assert_eq!(rows.len(), 1);
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_rejects_join() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM pg_tables t1 JOIN pg_indexes t2 ON t1.tablename = t2.tablename";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat);
        assert!(result.is_none());
    }

    #[test]
    fn test_try_execute_rejects_distinct() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT DISTINCT tablename FROM pg_tables";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat);
        assert!(result.is_none());
    }

    #[test]
    fn test_try_execute_rejects_group_by() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT tablename FROM pg_tables GROUP BY tablename";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat);
        assert!(result.is_none());
    }

    #[test]
    fn test_try_execute_with_pg_catalog_schema_prefix() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM pg_catalog.pg_tables";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat);
        assert!(result.is_some());
    }

    #[test]
    fn test_try_execute_where_case_insensitive() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM pg_tables WHERE TABLENAME = 'USERS'";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { rows, .. } => {
                assert_eq!(rows.len(), 1);
            }
            _ => panic!("expected ResultSet"),
        }
    }
}
