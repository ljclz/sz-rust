//! Multi-tenant SQL rewriter — Phase 3.9.
//!
//! # Design
//!
//! - **TenantContext** — Holds `tenant_id` + `prefix` + list of exempt (system) table names.
//!   `prefix` is prepended to physical table names (e.g. `tenant_001_`).
//! - **SqlRewriter** — Walks the AST and rewrites all `TableName` references
//!   to `prefix + name`, **unless** the table is in the exempt list (system tables
//!   like `pg_tables`, `pg_indexes`, `information_schema.*`).
//! - **Strategy: AST-level rewrite (not string substitution)** — Avoids substring
//!   mismatches (e.g. column name `users` inside a string literal would not be
//!   incorrectly prefixed). Walks all variants of `Statement` that contain
//!   `TableName`: CreateTable / DropTable / CreateIndex / DropIndex / Insert /
//!   Update / Delete / Select (TableFactor::Table).
//! - **System tables exemption** — `pg_tables`, `pg_indexes`, `pg_catalog.*`,
//!   `information_schema.*` are never prefixed. Configurable via `TenantContext::exempt`.
//!
//! # Rewrite rules
//!
//! - `FROM users` → `FROM tenant_001_users`
//! - `FROM public.users` → `FROM tenant_001.users` (schema = tenant_id, table stays)
//!   — SchemaPrefix mode (Phase 3.11 will exercise this). For Phase 3.9 we use
//!   `TableNamePrefix` mode (prepend to `name`, leave `schema` untouched).
//! - Subqueries (`Subquery`, `InSubquery`, `Exists`) — recursively rewritten
//! - JOIN relations — both sides rewritten
//! - DML (Insert/Update/Delete) — target table rewritten
//! - DDL (CreateTable/DropTable/CreateIndex/DropIndex) — target table/index rewritten
//!
//! Corresponds to `SzRSQL实施进度.md` Phase 3.9.

use szrsql_sql::ast::{
    Assignment, Expr, ForeignKeyReference, InsertSource, MatchRecognizeClause, OnConflict,
    OrderByExpr, Select, SelectItem, Statement, TableConstraint, TableFactor, TableName,
    TableWithJoins, WindowSpec, WithClause,
};

// =====================================================================
//  Tenant context
// =====================================================================

/// Multi-tenant context — carries tenant identity + prefix + exempt list
#[derive(Debug, Clone)]
pub struct TenantContext {
    /// Tenant identifier (e.g. "tenant_001")
    pub tenant_id: String,
    /// Prefix prepended to physical table names (typically `tenant_id + "_"`)
    pub prefix: String,
    /// System tables / schemas exempt from prefixing
    pub exempt: Vec<String>,
}

impl TenantContext {
    /// Create a new tenant context with default exempt list
    pub fn new(tenant_id: impl Into<String>) -> Self {
        let tenant_id = tenant_id.into();
        let prefix = format!("{tenant_id}_");
        Self {
            tenant_id,
            prefix,
            exempt: default_exempt_tables(),
        }
    }

    /// Create with a custom prefix
    pub fn with_prefix(tenant_id: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            prefix: prefix.into(),
            exempt: default_exempt_tables(),
        }
    }

    /// Add an exempt table name (case-insensitive)
    pub fn with_exempt(mut self, name: impl Into<String>) -> Self {
        self.exempt.push(name.into().to_lowercase());
        self
    }

    /// Check if a table name is exempt (case-insensitive)
    pub fn is_exempt(&self, name: &TableName) -> bool {
        // Check schema first (e.g. information_schema.* / pg_catalog.*)
        if let Some(schema) = &name.schema {
            let schema_lower = schema.to_lowercase();
            if schema_lower == "information_schema" || schema_lower == "pg_catalog" {
                return true;
            }
        }
        let name_lower = name.name.to_lowercase();
        self.exempt.contains(&name_lower)
    }
}

/// Default system tables exempt from tenant prefixing
fn default_exempt_tables() -> Vec<String> {
    [
        "pg_tables",
        "pg_indexes",
        "pg_class",
        "pg_namespace",
        "pg_attribute",
        "pg_index",
        "pg_constraint",
        "pg_type",
        "pg_proc",
        "pg_authid",
        "pg_database",
        "pg_views",
        "pg_settings",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

// =====================================================================
//  Rewrite error
// =====================================================================

/// Rewriter error
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RewriteError {
    /// Unsupported statement type for rewriting
    #[error("unsupported statement for tenant rewrite: {0}")]
    Unsupported(String),
}

// =====================================================================
//  SqlRewriter
// =====================================================================

/// SQL rewriter — walks AST and applies tenant prefix to all table references
#[derive(Debug, Clone)]
pub struct SqlRewriter {
    tenant: TenantContext,
}

impl SqlRewriter {
    /// Create a new rewriter for the given tenant
    pub fn new(tenant: TenantContext) -> Self {
        Self { tenant }
    }

    /// Borrow the tenant context
    pub fn tenant(&self) -> &TenantContext {
        &self.tenant
    }

    /// Rewrite a top-level statement
    pub fn rewrite_statement(&self, stmt: Statement) -> Result<Statement, RewriteError> {
        Ok(match stmt {
            Statement::CreateTable {
                name,
                columns,
                constraints,
                if_not_exists,
                temporary,
                on_commit,
            } => Statement::CreateTable {
                name: self.rewrite_table_name(name),
                columns,
                constraints: self.rewrite_constraints(constraints),
                if_not_exists,
                temporary,
                on_commit,
            },
            Statement::DropTable {
                names,
                if_exists,
                cascade,
            } => Statement::DropTable {
                names: names
                    .into_iter()
                    .map(|n| self.rewrite_table_name(n))
                    .collect(),
                if_exists,
                cascade,
            },
            Statement::CreateIndex {
                name,
                table,
                columns,
                unique,
                if_not_exists,
            } => Statement::CreateIndex {
                name,
                table: self.rewrite_table_name(table),
                columns,
                unique,
                if_not_exists,
            },
            Statement::DropIndex { names, if_exists } => Statement::DropIndex { names, if_exists },
            Statement::Insert {
                table,
                columns,
                source,
                on_conflict,
                returning,
            } => Statement::Insert {
                table: self.rewrite_table_name(table),
                columns,
                source: self.rewrite_insert_source(source),
                on_conflict: self.rewrite_on_conflict(on_conflict),
                returning,
            },
            Statement::Update {
                table,
                alias,
                assignments,
                where_clause,
                from,
                returning,
            } => Statement::Update {
                table: self.rewrite_table_name(table),
                alias,
                assignments: self.rewrite_assignments(assignments),
                where_clause: where_clause.map(|e| self.rewrite_expr(e)),
                from: self.rewrite_table_factors(from),
                returning,
            },
            Statement::Delete {
                table,
                alias,
                using,
                where_clause,
                returning,
            } => Statement::Delete {
                table: self.rewrite_table_name(table),
                alias,
                using: self.rewrite_table_factors(using),
                where_clause: where_clause.map(|e| self.rewrite_expr(e)),
                returning,
            },
            Statement::Select(s) => Statement::Select(Box::new(self.rewrite_select(*s))),
            other => {
                return Err(RewriteError::Unsupported(format!(
                    "{:?}",
                    std::mem::discriminant(&other)
                )))
            }
        })
    }

    // -----------------------------------------------------------------
    //  TableName rewriting
    // -----------------------------------------------------------------

    /// Rewrite a single TableName — prepend tenant prefix if not exempt
    pub fn rewrite_table_name(&self, name: TableName) -> TableName {
        if self.tenant.is_exempt(&name) {
            return name;
        }
        TableName {
            schema: name.schema,
            name: format!("{}{}", self.tenant.prefix, name.name),
        }
    }

    // -----------------------------------------------------------------
    //  Select / FROM / JOIN
    // -----------------------------------------------------------------

    /// Rewrite a SELECT statement (recursive)
    pub fn rewrite_select(&self, mut select: Select) -> Select {
        // Phase 6.1: WITH 子句（CTE）— 递归重写 CTE 查询体
        if let Some(with) = select.with.take() {
            let ctes = with
                .ctes
                .into_iter()
                .map(|mut cte| {
                    cte.query = Box::new(self.rewrite_select(*cte.query));
                    cte
                })
                .collect();
            select.with = Some(WithClause {
                recursive: with.recursive,
                ctes,
            });
        }
        select.from = select
            .from
            .into_iter()
            .map(|t| self.rewrite_table_with_joins(t))
            .collect();
        select.projection = select
            .projection
            .into_iter()
            .map(|p| self.rewrite_select_item(p))
            .collect();
        select.where_clause = select.where_clause.map(|e| self.rewrite_expr(e));
        select.group_by = select
            .group_by
            .into_iter()
            .map(|e| self.rewrite_expr(e))
            .collect();
        select.having = select.having.map(|e| self.rewrite_expr(e));
        select.order_by = select
            .order_by
            .into_iter()
            .map(|o| self.rewrite_order_by_expr(o))
            .collect();
        // limit / offset do not contain TableName references
        select
    }

    /// Rewrite a SelectItem — its expression may contain subqueries
    fn rewrite_select_item(&self, item: SelectItem) -> SelectItem {
        match item {
            SelectItem::UnnamedExpr(e) => SelectItem::UnnamedExpr(self.rewrite_expr(e)),
            SelectItem::ExprWithAlias { expr, alias } => SelectItem::ExprWithAlias {
                expr: self.rewrite_expr(expr),
                alias,
            },
            other => other,
        }
    }

    /// Rewrite a TableWithJoins (main table + JOINs)
    fn rewrite_table_with_joins(&self, mut twj: TableWithJoins) -> TableWithJoins {
        twj.relation = self.rewrite_table_factor(twj.relation);
        twj.joins = twj
            .joins
            .into_iter()
            .map(|j| self.rewrite_join(j))
            .collect();
        twj
    }

    /// Rewrite a TableFactor (Table / Derived / TableFunction)
    fn rewrite_table_factor(&self, factor: TableFactor) -> TableFactor {
        match factor {
            TableFactor::Table {
                name,
                alias,
                system_time_as_of: _,
            } => TableFactor::Table {
                name: self.rewrite_table_name(name),
                alias,
                system_time_as_of: None,
            },
            TableFactor::Derived {
                subquery,
                alias,
                lateral,
            } => TableFactor::Derived {
                subquery: Box::new(self.rewrite_select(*subquery)),
                alias,
                lateral,
            },
            TableFactor::TableFunction { name, args, alias } => TableFactor::TableFunction {
                name,
                args: args.into_iter().map(|a| self.rewrite_expr(a)).collect(),
                alias,
            },
            // P4-1: 递归重写 MATCH_RECOGNIZE 内层表与子句表达式
            TableFactor::MatchRecognize {
                table,
                clause,
                alias,
            } => TableFactor::MatchRecognize {
                table: Box::new(self.rewrite_table_factor(*table)),
                clause: MatchRecognizeClause {
                    partition_by: clause
                        .partition_by
                        .iter()
                        .map(|e| self.rewrite_expr(e.clone()))
                        .collect(),
                    order_by: clause
                        .order_by
                        .iter()
                        .map(|o| OrderByExpr {
                            expr: self.rewrite_expr(o.expr.clone()),
                            ..o.clone()
                        })
                        .collect(),
                    measures: clause
                        .measures
                        .iter()
                        .map(|(e, a)| (self.rewrite_expr(e.clone()), a.clone()))
                        .collect(),
                    rows_per_match: clause.rows_per_match.clone(),
                    after_match_skip: clause.after_match_skip.clone(),
                    pattern: clause.pattern.clone(),
                    symbols: clause
                        .symbols
                        .iter()
                        .map(|(s, e)| (s.clone(), self.rewrite_expr(e.clone())))
                        .collect(),
                },
                alias,
            },
        }
    }

    /// Rewrite a JOIN (right side + condition expr)
    fn rewrite_join(&self, mut join: szrsql_sql::ast::Join) -> szrsql_sql::ast::Join {
        join.relation = self.rewrite_table_factor(join.relation);
        // JOIN condition may reference columns; expressions don't contain TableName,
        // but ON expr may contain subqueries that do
        join.condition = self.rewrite_join_condition(join.condition);
        join
    }

    /// Rewrite JoinCondition — ON expr may contain subqueries
    fn rewrite_join_condition(
        &self,
        cond: szrsql_sql::ast::JoinCondition,
    ) -> szrsql_sql::ast::JoinCondition {
        use szrsql_sql::ast::JoinCondition;
        match cond {
            JoinCondition::On(expr) => JoinCondition::On(self.rewrite_expr(expr)),
            other => other,
        }
    }

    // -----------------------------------------------------------------
    //  Expression rewriting (subqueries only — TableName doesn't appear in Expr)
    // -----------------------------------------------------------------

    /// Rewrite an expression — only subquery variants need table rewriting
    pub fn rewrite_expr(&self, expr: Expr) -> Expr {
        match expr {
            Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
                left: Box::new(self.rewrite_expr(*left)),
                op,
                right: Box::new(self.rewrite_expr(*right)),
            },
            Expr::UnaryOp { op, expr } => Expr::UnaryOp {
                op,
                expr: Box::new(self.rewrite_expr(*expr)),
            },
            Expr::Function {
                name,
                args,
                distinct,
            } => Expr::Function {
                name,
                args: args.into_iter().map(|a| self.rewrite_expr(a)).collect(),
                distinct,
            },
            // Phase 6.2: 窗口函数 — 递归改写 args / partition_by / order_by 内的列引用
            Expr::WindowFunction {
                name,
                args,
                distinct,
                window,
            } => {
                let WindowSpec {
                    partition_by,
                    order_by,
                    window_frame,
                } = window;
                let new_partition_by = partition_by
                    .into_iter()
                    .map(|e| self.rewrite_expr(e))
                    .collect();
                let new_order_by = order_by
                    .into_iter()
                    .map(|mut obe| {
                        obe.expr = self.rewrite_expr(obe.expr);
                        obe
                    })
                    .collect();
                Expr::WindowFunction {
                    name,
                    args: args.into_iter().map(|a| self.rewrite_expr(a)).collect(),
                    distinct,
                    window: WindowSpec {
                        partition_by: new_partition_by,
                        order_by: new_order_by,
                        window_frame,
                    },
                }
            }
            Expr::Case {
                operand,
                when_then,
                else_expr,
            } => Expr::Case {
                operand: operand.map(|o| Box::new(self.rewrite_expr(*o))),
                when_then: when_then
                    .into_iter()
                    .map(|(w, t)| (self.rewrite_expr(w), self.rewrite_expr(t)))
                    .collect(),
                else_expr: else_expr.map(|e| Box::new(self.rewrite_expr(*e))),
            },
            Expr::Cast { expr, data_type } => Expr::Cast {
                expr: Box::new(self.rewrite_expr(*expr)),
                data_type,
            },
            Expr::InList {
                expr,
                list,
                negated,
            } => Expr::InList {
                expr: Box::new(self.rewrite_expr(*expr)),
                list: list.into_iter().map(|e| self.rewrite_expr(e)).collect(),
                negated,
            },
            Expr::InSubquery {
                expr,
                subquery,
                negated,
            } => Expr::InSubquery {
                expr: Box::new(self.rewrite_expr(*expr)),
                subquery: Box::new(self.rewrite_select(*subquery)),
                negated,
            },
            Expr::Between {
                expr,
                low,
                high,
                negated,
            } => Expr::Between {
                expr: Box::new(self.rewrite_expr(*expr)),
                low: Box::new(self.rewrite_expr(*low)),
                high: Box::new(self.rewrite_expr(*high)),
                negated,
            },
            Expr::Like {
                expr,
                pattern,
                negated,
                case_insensitive,
            } => Expr::Like {
                expr: Box::new(self.rewrite_expr(*expr)),
                pattern: Box::new(self.rewrite_expr(*pattern)),
                negated,
                case_insensitive,
            },
            Expr::IsNull { expr, negated } => Expr::IsNull {
                expr: Box::new(self.rewrite_expr(*expr)),
                negated,
            },
            // Phase F-9: PG 兼容表达式 — 递归重写
            Expr::IsDistinctFrom { left, right, not } => Expr::IsDistinctFrom {
                left: Box::new(self.rewrite_expr(*left)),
                right: Box::new(self.rewrite_expr(*right)),
                not,
            },
            Expr::SimilarTo {
                expr,
                pattern,
                negated,
            } => Expr::SimilarTo {
                expr: Box::new(self.rewrite_expr(*expr)),
                pattern: Box::new(self.rewrite_expr(*pattern)),
                negated,
            },
            Expr::Substring {
                expr,
                from,
                for_len,
            } => Expr::Substring {
                expr: Box::new(self.rewrite_expr(*expr)),
                from: from.map(|e| Box::new(self.rewrite_expr(*e))),
                for_len: for_len.map(|e| Box::new(self.rewrite_expr(*e))),
            },
            Expr::Subquery(s) => Expr::Subquery(Box::new(self.rewrite_select(*s))),
            Expr::Exists { subquery, negated } => Expr::Exists {
                subquery: Box::new(self.rewrite_select(*subquery)),
                negated,
            },
            Expr::Tuple(items) => {
                Expr::Tuple(items.into_iter().map(|e| self.rewrite_expr(e)).collect())
            }
            // Phase 3.32: Array / AnyOp / AllOp — 无 TableName，仅递归子表达式
            Expr::Array(items) => {
                Expr::Array(items.into_iter().map(|e| self.rewrite_expr(e)).collect())
            }
            Expr::AnyOp { left, op, right } => Expr::AnyOp {
                left: Box::new(self.rewrite_expr(*left)),
                op,
                right: Box::new(self.rewrite_expr(*right)),
            },
            Expr::AllOp { left, op, right } => Expr::AllOp {
                left: Box::new(self.rewrite_expr(*left)),
                op,
                right: Box::new(self.rewrite_expr(*right)),
            },
            // Leaf nodes — no rewriting needed
            leaf @ (Expr::Literal(_)
            | Expr::Identifier(_)
            | Expr::Wildcard
            | Expr::Parameter(_)
            // P3-1: GROUP BY constructs — no subqueries, pass through
            | Expr::GroupingSets(_)
            | Expr::Cube(_)
            | Expr::Rollup(_)) => leaf,
        }
    }

    /// Rewrite an OrderByExpr (the underlying expr may contain subqueries)
    fn rewrite_order_by_expr(&self, mut obe: OrderByExpr) -> OrderByExpr {
        obe.expr = self.rewrite_expr(obe.expr);
        obe
    }

    // -----------------------------------------------------------------
    //  DML helpers
    // -----------------------------------------------------------------

    /// Rewrite INSERT source (Values / Select / DefaultValues)
    fn rewrite_insert_source(&self, source: InsertSource) -> InsertSource {
        match source {
            InsertSource::Values(rows) => InsertSource::Values(
                rows.into_iter()
                    .map(|row| row.into_iter().map(|e| self.rewrite_expr(e)).collect())
                    .collect(),
            ),
            InsertSource::Select(s) => InsertSource::Select(Box::new(self.rewrite_select(*s))),
            InsertSource::DefaultValues => InsertSource::DefaultValues,
        }
    }

    /// Rewrite ON CONFLICT clause (target columns + DO UPDATE assignments + WHERE)
    fn rewrite_on_conflict(&self, on_conflict: Option<OnConflict>) -> Option<OnConflict> {
        on_conflict.map(|oc| match oc {
            OnConflict::DoNothing { conflict_columns } => {
                OnConflict::DoNothing { conflict_columns }
            }
            OnConflict::DoUpdate {
                conflict_columns,
                assignments,
                where_clause,
            } => OnConflict::DoUpdate {
                conflict_columns,
                assignments: self.rewrite_assignments(assignments),
                where_clause: where_clause.map(|e| self.rewrite_expr(e)),
            },
        })
    }

    /// Rewrite UPDATE assignments (RHS expressions may contain subqueries)
    fn rewrite_assignments(&self, assignments: Vec<Assignment>) -> Vec<Assignment> {
        assignments
            .into_iter()
            .map(|mut a| {
                a.value = self.rewrite_expr(a.value);
                a
            })
            .collect()
    }

    /// Rewrite a list of TableFactors (UPDATE FROM / DELETE USING)
    fn rewrite_table_factors(&self, factors: Vec<TableFactor>) -> Vec<TableFactor> {
        factors
            .into_iter()
            .map(|f| self.rewrite_table_factor(f))
            .collect()
    }

    // -----------------------------------------------------------------
    //  DDL helpers
    // -----------------------------------------------------------------

    /// Rewrite table-level constraints (foreign key references)
    fn rewrite_constraints(&self, constraints: Vec<TableConstraint>) -> Vec<TableConstraint> {
        constraints
            .into_iter()
            .map(|c| match c {
                TableConstraint::ForeignKey {
                    name,
                    columns,
                    reference,
                } => TableConstraint::ForeignKey {
                    name,
                    columns,
                    reference: self.rewrite_foreign_key_reference(reference),
                },
                other => other,
            })
            .collect()
    }

    /// Rewrite foreign key reference target table
    fn rewrite_foreign_key_reference(&self, mut ref_: ForeignKeyReference) -> ForeignKeyReference {
        ref_.table = self.rewrite_table_name(ref_.table);
        ref_
    }
}

// =====================================================================
//  SelectItem helper (unused variants ignored — kept for future expansion)
// =====================================================================

#[allow(dead_code)]
fn select_item_uses_table(_item: &SelectItem) -> bool {
    // SelectItem expressions may contain subqueries (rare in projections, but possible)
    // We always recurse into them via rewrite_select if needed.
    false
}
