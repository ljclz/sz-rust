//! SzRSQL 逻辑计划（Phase 3.2）— AST → LogicalPlan 转换器。
//!
//! # 设计
//!
//! - **入口**：`Planner::new(catalog).plan_statement(stmt) -> Result<LogicalPlan, PlanError>`
//! - **Catalog 抽象**：`Catalog` trait 提供 `table_exists` / `get_table` 接口，
//!   让 Planner 在生成计划时验证表/列存在性
//! - **逻辑算子**：Scan / Projection / Filter / Join / Aggregate / Sort / Limit / Distinct
//!   以及 DML 节点（Insert / Update / Delete）和 DDL 节点（CreateTable / DropTable / ...）
//! - **表达式**：直接复用 `ast::Expr`，避免类型重复
//!
//! # 关系代数转换规则
//!
//! - `SELECT DISTINCT cols FROM t1 JOIN t2 ON c WHERE w GROUP BY g HAVING h ORDER BY o LIMIT l OFFSET f`
//!   → `Limit(l, f, Sort(o, Distinct(Project(cols, Filter(w, Join(t1, t2, c))))))`
//!   - GROUP BY/HAVING：`Aggregate(g, h, Project(cols, Filter(w, Join(t1, t2, c))))`
//! - `INSERT INTO t(c) VALUES(v)` → `Insert { table: t, columns: c, source: Values(v) }`
//! - `UPDATE t SET c=v WHERE w` → `Update { table: t, assignments: [(c, v)], source: Filter(w, Scan(t)) }`
//! - `DELETE FROM t WHERE w` → `Delete { table: t, source: Filter(w, Scan(t)) }`
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.2。

use crate::ast::*;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use szrsql_types::value::{ColumnType, Value};
use thiserror::Error;

// =====================================================================
//  错误类型
// =====================================================================

/// 逻辑计划错误
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanError {
    /// 表不存在
    #[error("table not found: {0}")]
    TableNotFound(String),
    /// 列不存在
    #[error("column not found: {0}")]
    ColumnNotFound(String),
    /// 列歧义（多个表都有同名列）
    #[error("ambiguous column: {0}")]
    AmbiguousColumn(String),
    /// 列数不匹配（INSERT 列数 vs VALUES 数）
    #[error("column count mismatch: expected {expected}, got {actual}")]
    ColumnCountMismatch {
        /// 期望列数
        expected: usize,
        /// 实际列数
        actual: usize,
    },
    /// 不支持的 SQL 特性
    #[error("unsupported SQL feature: {0}")]
    Unsupported(String),
    /// 无效表达式
    #[error("invalid expression: {0}")]
    InvalidExpression(String),
    /// 表已存在（CREATE TABLE IF NOT EXISTS 未指定时）
    #[error("table already exists: {0}")]
    TableAlreadyExists(String),
}

// =====================================================================
//  Catalog 抽象
// =====================================================================

/// 表 Schema（列名 + 类型）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableSchema {
    /// 表名
    pub name: TableName,
    /// 列定义
    pub columns: Vec<ColumnDefinition>,
}

impl TableSchema {
    /// 按列名查找列定义
    pub fn find_column(&self, name: &str) -> Option<&ColumnDefinition> {
        self.columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// 列名列表（按定义顺序）
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.name.as_str()).collect()
    }
}

// =====================================================================
//  Sequence 定义 — Phase 3.22
// =====================================================================

/// 序列定义（CREATE SEQUENCE）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceDefinition {
    /// 序列名
    pub name: TableName,
    /// 起始值
    pub start: i64,
    /// 步长
    pub increment: i64,
    /// 最小值
    pub min_value: Option<i64>,
    /// 最大值
    pub max_value: Option<i64>,
    /// 是否循环
    pub cycle: bool,
}

impl SequenceDefinition {
    /// 默认序列（start=1, increment=1, no min/max, no cycle）
    pub fn new(name: TableName) -> Self {
        Self {
            name,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cycle: false,
        }
    }
}

// =====================================================================
//  ENUM 类型定义 — Phase 3.31
// =====================================================================

/// ENUM 类型定义（CREATE TYPE name AS ENUM (...)）
///
/// 存储在 Catalog 的 `enum_types` 中，用于：
/// - `CREATE TABLE t (c mood)` 时解析 `mood` 为 `ColumnType::Enum(values)`
/// - `INSERT/UPDATE` 时校验 enum 值是否在 labels 中
/// - `ALTER TYPE ... ADD VALUE` 追加 label
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumTypeDefinition {
    /// 类型名
    pub name: TableName,
    /// ENUM 标签值列表（按声明顺序）
    pub labels: Vec<String>,
}

impl EnumTypeDefinition {
    /// 创建新的 ENUM 类型定义
    pub fn new(name: TableName, labels: Vec<String>) -> Self {
        Self { name, labels }
    }

    /// 检查 label 是否在此 ENUM 类型中
    pub fn contains(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }
}

// =====================================================================
//  外键约束 — Phase 3.29
// =====================================================================

/// 外键约束（统一表示列级与表级 FK）— Phase 3.29
///
/// 存储在 Catalog 中，用于运行时引用完整性校验。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForeignKeyConstraint {
    /// 约束名（可选）
    pub name: Option<String>,
    /// 本表（子表）列名
    pub columns: Vec<String>,
    /// 引用信息
    pub reference: ForeignKeyReference,
}

/// 引用方信息（反向索引条目）— Phase 3.29
///
/// 表示"哪个表的哪个 FK 引用了本表"，用于 ON DELETE / ON UPDATE 级联。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReferencingKey {
    /// 引用方（子表）表名
    pub child_table: TableName,
    /// 子表中的 FK 列名
    pub child_columns: Vec<String>,
    /// 父表中被引用的列名
    pub parent_columns: Vec<String>,
    /// ON DELETE 动作
    pub on_delete: ReferenceAction,
    /// ON UPDATE 动作
    pub on_update: ReferenceAction,
    /// 约束名（可选）
    pub name: Option<String>,
}

/// Catalog 抽象接口 — 提供 Schema 查询能力
pub trait Catalog {
    /// 表是否存在
    fn table_exists(&self, name: &TableName) -> bool;

    /// 获取表 Schema
    fn get_table(&self, name: &TableName) -> Option<TableSchema>;

    /// 列出所有表
    fn list_tables(&self) -> Vec<TableName>;

    /// 序列是否存在（Phase 3.22）
    fn sequence_exists(&self, _name: &TableName) -> bool {
        false // 默认实现：不支持序列
    }

    /// 获取序列定义（Phase 3.22）
    fn get_sequence(&self, _name: &TableName) -> Option<SequenceDefinition> {
        None // 默认实现：不支持序列
    }

    /// 列出所有序列（Phase 3.22）
    fn list_sequences(&self) -> Vec<TableName> {
        Vec::new()
    }

    /// 获取表的 outgoing 外键约束（本表引用其他表）— Phase 3.29
    ///
    /// 默认实现返回空 Vec（不支持 FK）。
    fn get_foreign_keys(&self, _name: &TableName) -> Vec<ForeignKeyConstraint> {
        Vec::new()
    }

    /// 获取表的 incoming 引用（其他表引用本表）— Phase 3.29
    ///
    /// 用于 ON DELETE / ON UPDATE 级联。默认实现返回空 Vec。
    fn get_referencing_keys(&self, _name: &TableName) -> Vec<ReferencingKey> {
        Vec::new()
    }

    /// 获取表的 CHECK 约束列表 — Phase 3.30
    ///
    /// 包含列级 CHECK（`col INT CHECK (...)`)和表级 CHECK（`CHECK (...)`)。
    /// 默认实现返回空 Vec（不支持 CHECK）。
    fn get_check_constraints(&self, _name: &TableName) -> Vec<CheckConstraint> {
        Vec::new()
    }

    /// ENUM 类型是否存在 — Phase 3.31
    ///
    /// 默认实现返回 false（不支持 ENUM 类型）。
    fn enum_type_exists(&self, _name: &TableName) -> bool {
        false
    }

    /// 获取 ENUM 类型定义 — Phase 3.31
    ///
    /// 默认实现返回 None（不支持 ENUM 类型）。
    fn get_enum_type(&self, _name: &TableName) -> Option<EnumTypeDefinition> {
        None
    }

    /// 列出所有 ENUM 类型 — Phase 3.31
    ///
    /// 默认实现返回空 Vec。
    fn list_enum_types(&self) -> Vec<TableName> {
        Vec::new()
    }

    /// 列出表上的所有索引 — Phase 5.7
    ///
    /// 默认实现返回空 Vec（不支持索引）。
    /// 索引选择规则使用此接口获取候选索引列表。
    fn list_indexes(&self, _table: &TableName) -> Vec<IndexDefinition> {
        Vec::new()
    }

    /// 列出表上的所有触发器 — Phase 6.4
    ///
    /// 默认实现返回空 Vec（不支持触发器）。
    /// 执行器在执行 DML 时通过此接口获取候选触发器列表。
    fn list_triggers(&self, _table: &TableName) -> Vec<TriggerDefinition> {
        Vec::new()
    }

    /// 获取指定表上的指定触发器 — Phase 6.4
    ///
    /// 默认实现返回 None（不支持触发器）。
    /// 用于 CREATE TRIGGER / DROP TRIGGER 时的存在性检查。
    fn get_trigger(&self, _table: &TableName, _name: &str) -> Option<TriggerDefinition> {
        None
    }

    /// 视图是否存在 — Phase 6.15
    ///
    /// 默认实现返回 false（不支持视图）。
    /// 用于查询重写时判断表名是否为视图。
    fn view_exists(&self, _name: &TableName) -> bool {
        false
    }

    /// 获取视图定义 — Phase 6.15
    ///
    /// 默认实现返回 None（不支持视图）。
    /// 用于查询重写时获取视图查询体（普通视图展开 / 物化视图路由）。
    fn get_view(&self, _name: &TableName) -> Option<crate::materialized_view::ViewDefinition> {
        None
    }

    /// 列出所有视图名 — Phase 6.15
    ///
    /// 默认实现返回空 Vec（不支持视图）。
    /// 用于 pg_views 系统表查询，返回 catalog 中所有已注册的视图。
    fn list_views(&self) -> Vec<TableName> {
        Vec::new()
    }
}

/// 索引定义 — Phase 5.7
///
/// 表示一个已创建的索引。存储在 Catalog 中，供索引选择规则查询。
/// 由 `CREATE INDEX` 语句注册到 `InMemoryCatalog.indexes`。
///
/// 注：不派生 `Eq`，因 `IndexColumn` 含 `Option<Expr>`，而 `Expr` 含浮点字面量无法实现 `Eq`。
#[derive(Debug, Clone, PartialEq)]
pub struct IndexDefinition {
    /// 索引名
    pub name: String,
    /// 所属表名
    pub table: TableName,
    /// 索引列（按声明顺序；复合索引按最左前缀匹配）
    pub columns: Vec<IndexColumn>,
    /// 是否为 UNIQUE 索引
    pub unique: bool,
}

impl IndexDefinition {
    /// 创建普通索引
    pub fn new(name: impl Into<String>, table: TableName, columns: Vec<IndexColumn>) -> Self {
        Self {
            name: name.into(),
            table,
            columns,
            unique: false,
        }
    }

    /// 创建 UNIQUE 索引
    pub fn new_unique(
        name: impl Into<String>,
        table: TableName,
        columns: Vec<IndexColumn>,
    ) -> Self {
        Self {
            name: name.into(),
            table,
            columns,
            unique: true,
        }
    }

    /// 索引列名列表（按声明顺序）
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.column.as_str()).collect()
    }
}

/// CHECK 约束定义 — Phase 3.30
///
/// 表示一个 CHECK 约束（列级或表级统一存储）。
/// 校验时对行求值 `expr`，结果必须为 true 或 NULL（PG 语义：NULL 视为通过）。
#[derive(Debug, Clone, PartialEq)]
pub struct CheckConstraint {
    /// 约束名（可选；未指定时为 None）
    pub name: Option<String>,
    /// CHECK 表达式（基于表行的布尔表达式）
    pub expr: Expr,
}

impl CheckConstraint {
    /// 创建无名 CHECK 约束
    pub fn new(expr: Expr) -> Self {
        Self { name: None, expr }
    }

    /// 创建具名 CHECK 约束
    pub fn with_name(name: impl Into<String>, expr: Expr) -> Self {
        Self {
            name: Some(name.into()),
            expr,
        }
    }
}

/// 函数定义 — Phase 6.5（P0-5 修复）
///
/// 存储 SQL 函数的元数据，支持 PL/pgSQL、SQL、C 等多种语言。
/// 函数体执行由表达式求值器在调用时按需触发（PL/pgSQL 解释器或 UDF 注册表）。
///
/// # 字段语义
/// - `name`：函数名（保留原始大小写，catalog 内部以小写为键）
/// - `parameters`：参数列表（按声明顺序）
/// - `return_type`：返回类型原文（如 `integer`、`void`、`TABLE(...)`）
/// - `language`：函数语言（`plpgsql` / `sql` / `c`）
/// - `body`：函数体原文（已剥离 `$$` / `'` 等定界符）
/// - `volatility`：IMMUTABLE / STABLE / VOLATILE（None=VOLATILE）
/// - `strict`：RETURNS NULL ON NULL INPUT
/// - `security_definer`：以定义者权限执行
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// 函数名（含可选 schema 前缀）
    pub name: String,
    /// 参数列表
    pub parameters: Vec<crate::ast::FunctionParameter>,
    /// 返回类型原文
    pub return_type: String,
    /// 函数语言
    pub language: String,
    /// 函数体原文
    pub body: String,
    /// 函数波动性
    pub volatility: Option<crate::ast::FunctionVolatility>,
    /// STRICT
    pub strict: bool,
    /// SECURITY DEFINER
    pub security_definer: bool,
}

/// 内存 Catalog — 用于单元测试和示例
#[derive(Debug, Default, Clone)]
pub struct InMemoryCatalog {
    tables: HashMap<String, TableSchema>,
    sequences: HashMap<String, SequenceDefinition>,
    /// 外键约束（表名小写 → outgoing FK 列表）— Phase 3.29
    foreign_keys: HashMap<String, Vec<ForeignKeyConstraint>>,
    /// 反向索引（被引用表名小写 → incoming 引用列表）— Phase 3.29
    referencing_keys: HashMap<String, Vec<ReferencingKey>>,
    /// CHECK 约束（表名小写 → CHECK 列表）— Phase 3.30
    check_constraints: HashMap<String, Vec<CheckConstraint>>,
    /// ENUM 类型（类型名小写 → ENUM 定义）— Phase 3.31
    enum_types: HashMap<String, EnumTypeDefinition>,
    /// 索引（表名小写 → 索引列表）— Phase 5.7
    indexes: HashMap<String, Vec<IndexDefinition>>,
    /// 触发器（表名小写 → 触发器列表）— Phase 6.4
    triggers: HashMap<String, Vec<TriggerDefinition>>,
    /// 视图（视图名小写 → 视图定义）— Phase 6.10
    views: HashMap<String, crate::materialized_view::ViewDefinition>,
    /// 注释存储（key = "table_name" 或 "table_name.column_name"）— Phase TDengine-P2
    comments: HashMap<String, String>,
    /// 函数定义（函数名小写 → 函数定义列表，支持重载）— Phase 6.5（P0-5 修复）
    functions: HashMap<String, Vec<FunctionDefinition>>,
}

impl InMemoryCatalog {
    /// 创建空 catalog
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            sequences: HashMap::new(),
            foreign_keys: HashMap::new(),
            referencing_keys: HashMap::new(),
            check_constraints: HashMap::new(),
            enum_types: HashMap::new(),
            indexes: HashMap::new(),
            triggers: HashMap::new(),
            views: HashMap::new(),
            comments: HashMap::new(),
            functions: HashMap::new(),
        }
    }

    /// 添加表 Schema
    pub fn add_table(&mut self, schema: TableSchema) {
        let key = self.key(&schema.name);
        self.tables.insert(key, schema);
    }

    /// 移除表 Schema — Phase 3.28
    ///
    /// 用于临时表 `ON COMMIT DROP` 或显式 `DROP TEMPORARY TABLE`。
    /// 返回被移除的 Schema（若存在）。
    pub fn remove_table(&mut self, name: &TableName) -> Option<TableSchema> {
        self.tables.remove(&self.key(name))
    }

    /// 添加表（简化方式：表名 + 列定义）
    pub fn add_simple_table(&mut self, name: &str, columns: Vec<(&str, ColumnType)>) {
        let table_name = TableName::new(name);
        let cols = columns
            .into_iter()
            .map(|(n, t)| ColumnDefinition::new(n, t))
            .collect();
        self.add_table(TableSchema {
            name: table_name,
            columns: cols,
        });
    }

    /// 添加序列（Phase 3.22）
    pub fn add_sequence(&mut self, def: SequenceDefinition) {
        let key = self.key(&def.name);
        self.sequences.insert(key, def);
    }

    /// 注册外键约束 — Phase 3.29
    ///
    /// 同时更新 `foreign_keys`（子表 → FK 列表）和 `referencing_keys`（父表 → 引用方列表）。
    /// 调用方需保证父表已存在（用于解析被引用列名）。
    pub fn add_foreign_key(
        &mut self,
        child_table: &TableName,
        fk: ForeignKeyConstraint,
    ) -> Result<(), PlanError> {
        let child_key = self.key(child_table);
        let parent_key = self.key(&fk.reference.table);

        // 解析被引用列名：None 表示引用父表主键
        let parent_columns: Vec<String> = match &fk.reference.columns {
            Some(cols) => cols.clone(),
            None => {
                // 查找父表主键列
                let parent_schema = self
                    .tables
                    .get(&parent_key)
                    .ok_or_else(|| PlanError::TableNotFound(fk.reference.table.qualified_name()))?;
                parent_schema
                    .columns
                    .iter()
                    .filter(|c| c.primary_key)
                    .map(|c| c.name.clone())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .next()
                    .map(|c| vec![c])
                    .ok_or_else(|| {
                        PlanError::Unsupported(format!(
                            "cannot resolve PK of referenced table {} for FK",
                            fk.reference.table.qualified_name()
                        ))
                    })?
            }
        };

        // 记录 outgoing FK（子表 → 父表）
        self.foreign_keys
            .entry(child_key)
            .or_default()
            .push(fk.clone());

        // 记录 incoming 引用（父表 → 子表）
        let referencing = ReferencingKey {
            child_table: child_table.clone(),
            child_columns: fk.columns.clone(),
            parent_columns,
            on_delete: fk.reference.on_delete.unwrap_or(ReferenceAction::NoAction),
            on_update: fk.reference.on_update.unwrap_or(ReferenceAction::NoAction),
            name: fk.name.clone(),
        };
        self.referencing_keys
            .entry(parent_key)
            .or_default()
            .push(referencing);

        Ok(())
    }

    /// 表名 → lowercase qualified key（大小写不敏感）
    ///
    /// 当 schema 为 None 时默认使用 "public"，与 ManagedCatalog::table_key 保持一致，
    /// 确保 `CREATE TABLE t` 和 `SELECT * FROM "public"."t"` 能匹配同一张表。
    fn key(&self, name: &TableName) -> String {
        match &name.schema {
            Some(s) => format!("{}.{}", s.to_lowercase(), name.name.to_lowercase()),
            None => format!("public.{}", name.name.to_lowercase()),
        }
    }

    /// 设置表注释 — Phase TDengine-P2
    ///
    /// `comment=None` 时删除已有注释。
    pub fn set_table_comment(
        &mut self,
        name: &TableName,
        comment: Option<String>,
    ) -> Result<(), PlanError> {
        let key = self.key(name);
        match comment {
            Some(c) => {
                self.comments.insert(key, c);
            }
            None => {
                self.comments.remove(&key);
            }
        }
        Ok(())
    }

    /// 设置列注释 — Phase TDengine-P2
    ///
    /// `comment=None` 时删除已有注释。
    pub fn set_column_comment(
        &mut self,
        table: &TableName,
        column: &str,
        comment: Option<String>,
    ) -> Result<(), PlanError> {
        let key = format!("{}.{}", self.key(table), column.to_lowercase());
        match comment {
            Some(c) => {
                self.comments.insert(key, c);
            }
            None => {
                self.comments.remove(&key);
            }
        }
        Ok(())
    }

    /// 获取表注释 — Phase TDengine-P2
    pub fn get_table_comment(&self, name: &TableName) -> Option<String> {
        self.comments.get(&self.key(name)).cloned()
    }

    /// 获取列注释 — Phase TDengine-P2
    pub fn get_column_comment(&self, table: &TableName, column: &str) -> Option<String> {
        let key = format!("{}.{}", self.key(table), column.to_lowercase());
        self.comments.get(&key).cloned()
    }

    /// 从 CREATE TABLE 计划注册表 Schema 和外键 — Phase 3.29
    ///
    /// 解析列级 `REFERENCES` 和表级 `FOREIGN KEY` 约束，统一注册到 catalog。
    /// 调用方需保证父表已注册（用于解析 `REFERENCES` 省略列名时引用父表 PK）。
    pub fn register_from_create_plan(&mut self, plan: &LogicalPlan) -> Result<(), PlanError> {
        let (name, columns, constraints) = match plan {
            LogicalPlan::CreateTable {
                name,
                columns,
                constraints,
                ..
            } => (name, columns, constraints),
            _ => return Ok(()),
        };

        // 1. 注册表 Schema
        self.add_table(TableSchema {
            name: name.clone(),
            columns: columns.clone(),
        });

        // 2. 提取并注册表级 FK（FOREIGN KEY (cols) REFERENCES ...）
        for constraint in constraints {
            if let TableConstraint::ForeignKey {
                name: fk_name,
                columns: fk_cols,
                reference,
            } = constraint
            {
                self.add_foreign_key(
                    name,
                    ForeignKeyConstraint {
                        name: fk_name.clone(),
                        columns: fk_cols.clone(),
                        reference: reference.clone(),
                    },
                )?;
            }
        }

        // 3. 提取并注册列级 FK（col REFERENCES ...）
        for col in columns {
            if let Some(ref reference) = col.references {
                self.add_foreign_key(
                    name,
                    ForeignKeyConstraint {
                        name: None,
                        columns: vec![col.name.clone()],
                        reference: reference.clone(),
                    },
                )?;
            }
        }

        // 4. 提取并注册表级 CHECK 约束 — Phase 3.30
        for constraint in constraints {
            if let TableConstraint::Check {
                name: ck_name,
                expr,
            } = constraint
            {
                self.add_check_constraint(
                    name,
                    CheckConstraint {
                        name: ck_name.clone(),
                        expr: expr.clone(),
                    },
                );
            }
        }

        // 5. 提取并注册列级 CHECK 约束 — Phase 3.30
        for col in columns {
            if let Some(ref expr) = col.check {
                self.add_check_constraint(
                    name,
                    CheckConstraint {
                        name: None,
                        expr: expr.clone(),
                    },
                );
            }
        }

        Ok(())
    }

    /// 注册 CHECK 约束 — Phase 3.30
    ///
    /// 将一个 CHECK 约束（列级或表级）添加到指定表的约束列表。
    pub fn add_check_constraint(&mut self, table: &TableName, check: CheckConstraint) {
        let key = self.key(table);
        self.check_constraints.entry(key).or_default().push(check);
    }

    /// 注册 ENUM 类型 — Phase 3.31
    ///
    /// 将一个 ENUM 类型定义添加到 catalog 中。若同名类型已存在则覆盖。
    pub fn add_enum_type(&mut self, def: EnumTypeDefinition) {
        let key = self.key(&def.name);
        self.enum_types.insert(key, def);
    }

    /// 移除 ENUM 类型 — Phase 3.31
    ///
    /// 用于 `DROP TYPE`。返回被移除的定义（若存在）。
    pub fn remove_enum_type(&mut self, name: &TableName) -> Option<EnumTypeDefinition> {
        self.enum_types.remove(&self.key(name))
    }

    /// 可变获取 ENUM 类型定义 — Phase 3.31
    ///
    /// 用于 `ALTER TYPE` 修改 labels。
    pub fn get_enum_type_mut(&mut self, name: &TableName) -> Option<&mut EnumTypeDefinition> {
        self.enum_types.get_mut(&self.key(name))
    }

    /// 注册索引 — Phase 5.7
    ///
    /// 将一个索引定义添加到指定表的索引列表。
    /// 同名索引会被覆盖（与 `CREATE INDEX IF NOT EXISTS` 语义不同——
    /// `IF NOT EXISTS` 检查在 parser/planner 层完成，catalog 仅存储最终结果）。
    pub fn add_index(&mut self, index: IndexDefinition) {
        let key = self.key(&index.table);
        let list = self.indexes.entry(key).or_default();
        // 若同名索引已存在则替换
        if let Some(pos) = list
            .iter()
            .position(|i| i.name.eq_ignore_ascii_case(&index.name))
        {
            list[pos] = index;
        } else {
            list.push(index);
        }
    }

    /// 移除指定名称的索引 — Phase 5.7
    ///
    /// 返回被移除的索引定义；若不存在返回 None。
    /// 用于 `DROP INDEX` 语句。
    pub fn remove_index(&mut self, name: &str) -> Option<IndexDefinition> {
        let name_lower = name.to_lowercase();
        for list in self.indexes.values_mut() {
            if let Some(pos) = list
                .iter()
                .position(|i| i.name.eq_ignore_ascii_case(&name_lower))
            {
                return Some(list.remove(pos));
            }
        }
        None
    }

    /// 注册触发器 — Phase 6.4
    ///
    /// 将一个触发器定义添加到指定表的触发器列表。
    /// 同名触发器会被覆盖（用于 `CREATE OR REPLACE TRIGGER` 语义）。
    /// `CREATE TRIGGER IF NOT EXISTS` 的存在性检查在 planner 层完成；
    /// executor 在 `or_replace=true` 时调用本方法替换旧定义。
    pub fn add_trigger(&mut self, trigger: TriggerDefinition) {
        let key = self.key(&trigger.table);
        let list = self.triggers.entry(key).or_default();
        if let Some(pos) = list
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(&trigger.name))
        {
            list[pos] = trigger;
        } else {
            list.push(trigger);
        }
    }

    /// 移除指定表上的指定触发器 — Phase 6.4
    ///
    /// 返回被移除的触发器定义；若不存在返回 None。
    /// 用于 `DROP TRIGGER` 语句。
    pub fn remove_trigger(&mut self, table: &TableName, name: &str) -> Option<TriggerDefinition> {
        let key = self.key(table);
        if let Some(list) = self.triggers.get_mut(&key) {
            if let Some(pos) = list.iter().position(|t| t.name.eq_ignore_ascii_case(name)) {
                return Some(list.remove(pos));
            }
        }
        None
    }

    /// 表删除时清理关联的触发器 — Phase 6.4
    ///
    /// 用于 `DROP TABLE` 时级联清理触发器。
    pub fn drop_triggers_for_table(&mut self, table: &TableName) {
        let key = self.key(table);
        self.triggers.remove(&key);
    }

    // -----------------------------------------------------------------
    //  视图管理 — Phase 6.10
    // -----------------------------------------------------------------

    /// 注册视图定义 — Phase 6.10
    ///
    /// 若同名视图已存在，**不会**自动替换（与 PG `CREATE VIEW` 一致）。
    /// 调用方应在 `or_replace=true` 时先调用 `remove_view`。
    pub fn add_view(&mut self, view: crate::materialized_view::ViewDefinition) {
        let key = self.key(&view.name);
        self.views.insert(key, view);
    }

    /// 移除视图定义 — Phase 6.10
    ///
    /// 返回被移除的视图定义；若不存在返回 None。
    /// 用于 `DROP VIEW` / `DROP MATERIALIZED VIEW` 语句。
    pub fn remove_view(
        &mut self,
        name: &TableName,
    ) -> Option<crate::materialized_view::ViewDefinition> {
        self.views.remove(&self.key(name))
    }

    /// 查询视图定义 — Phase 6.10
    pub fn get_view(&self, name: &TableName) -> Option<&crate::materialized_view::ViewDefinition> {
        self.views.get(&self.key(name))
    }

    /// 视图是否存在 — Phase 6.10
    pub fn view_exists(&self, name: &TableName) -> bool {
        self.views.contains_key(&self.key(name))
    }

    /// 列出所有视图名 — Phase 6.10
    pub fn list_views(&self) -> Vec<TableName> {
        self.views.values().map(|v| v.name.clone()).collect()
    }

    // =================================================================
    //  函数定义管理 — Phase 6.5（P0-5 修复）
    // =================================================================

    /// 添加函数定义 — Phase 6.5
    ///
    /// 支持函数重载：同名但参数签名不同的函数可共存。
    /// 若 `or_replace=true` 且存在相同签名的函数，则替换；否则报错。
    /// 若 `or_replace=false` 且存在相同签名的函数，则报错。
    pub fn add_function(
        &mut self,
        def: FunctionDefinition,
        or_replace: bool,
    ) -> Result<(), PlanError> {
        let key = def.name.to_lowercase();
        let signatures = self.functions.entry(key.clone()).or_default();
        // 检查签名冲突（参数类型 + 参数数量）
        let conflict_idx = signatures.iter().position(|existing| {
            existing.parameters.len() == def.parameters.len()
                && existing
                    .parameters
                    .iter()
                    .zip(def.parameters.iter())
                    .all(|(a, b)| {
                        a.data_type.trim().eq_ignore_ascii_case(b.data_type.trim())
                    })
        });
        if let Some(idx) = conflict_idx {
            if or_replace {
                signatures[idx] = def;
                Ok(())
            } else {
                Err(PlanError::Unsupported(format!(
                    "function {} already exists with same parameter types",
                    def.name
                )))
            }
        } else {
            signatures.push(def);
            Ok(())
        }
    }

    /// 删除函数定义 — Phase 6.5
    ///
    /// 按 `parameter_types` 精确匹配签名删除。
    /// 若 `parameter_types` 为空且该函数名只有一个定义，则删除之；
    /// 若有多个重载则报错（PG 语义：必须指定参数类型）。
    ///
    /// 返回是否实际删除了函数。
    pub fn drop_function(
        &mut self,
        name: &str,
        parameter_types: &[String],
        if_exists: bool,
    ) -> Result<bool, PlanError> {
        let key = name.to_lowercase();
        let signatures = match self.functions.get_mut(&key) {
            Some(s) if !s.is_empty() => s,
            _ => {
                if if_exists {
                    return Ok(false);
                }
                return Err(PlanError::Unsupported(format!(
                    "function {} does not exist",
                    name
                )));
            }
        };
        if parameter_types.is_empty() {
            if signatures.len() == 1 {
                signatures.clear();
                Ok(true)
            } else {
                Err(PlanError::Unsupported(format!(
                    "function {} is overloaded ({} variants), must specify parameter types",
                    name,
                    signatures.len()
                )))
            }
        } else {
            let idx = signatures.iter().position(|existing| {
                existing.parameters.len() == parameter_types.len()
                    && existing
                        .parameters
                        .iter()
                        .zip(parameter_types.iter())
                        .all(|(p, t)| p.data_type.trim().eq_ignore_ascii_case(t.trim()))
            });
            match idx {
                Some(i) => {
                    signatures.remove(i);
                    Ok(true)
                }
                None => {
                    if if_exists {
                        Ok(false)
                    } else {
                        Err(PlanError::Unsupported(format!(
                            "function {} with specified parameter types does not exist",
                            name
                        )))
                    }
                }
            }
        }
    }

    /// 按名查询函数定义（取第一个匹配，用于无重载场景）— Phase 6.5
    pub fn get_function(&self, name: &str) -> Option<&FunctionDefinition> {
        self.functions
            .get(&name.to_lowercase())
            .and_then(|v| v.first())
    }

    /// 按名+参数数量查询函数定义（用于调用时解析）— Phase 6.5
    pub fn find_function(&self, name: &str, arg_count: usize) -> Option<&FunctionDefinition> {
        self.functions
            .get(&name.to_lowercase())
            .and_then(|v| v.iter().find(|f| f.parameters.len() == arg_count))
    }

    /// 列出所有函数名 — Phase 6.5
    pub fn list_functions(&self) -> Vec<String> {
        self.functions.keys().cloned().collect()
    }

    /// 列出指定函数名的所有重载定义 — Phase 6.5
    pub fn list_function_overloads(&self, name: &str) -> Vec<&FunctionDefinition> {
        self.functions
            .get(&name.to_lowercase())
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// 替换表 Schema — Phase F-10
    ///
    /// 用于 `ALTER TABLE` 系列操作：执行器先 `get_table` 取得现有 Schema，
    /// 在克隆上修改（增删列、改类型、改约束、改默认值、改 NOT NULL 等），
    /// 再调用此方法整体替换。
    ///
    /// - 若表不存在，返回 `PlanError::TableNotFound`
    /// - 表名必须与现有表名一致（不可用于 RENAME，RENAME 走 `rename_table`）
    /// - 不会影响关联索引（索引元数据保持不变；若 DROP COLUMN 删除了被索引引用的列，
    ///   执行器应先调用 `remove_index` 再调用此方法）
    pub fn replace_table_schema(&mut self, schema: TableSchema) -> Result<(), PlanError> {
        let key = self.key(&schema.name);
        if !self.tables.contains_key(&key) {
            return Err(PlanError::TableNotFound(schema.name.qualified_name()));
        }
        self.tables.insert(key, schema);
        Ok(())
    }

    /// 重命名表 — Phase F-10
    ///
    /// 用于 `ALTER TABLE ... RENAME TO new_name`。
    /// - 若旧表不存在，返回 `PlanError::TableNotFound`
    /// - 若新表名已存在，返回 `PlanError::TableAlreadyExists`
    /// - 同时更新关联索引、外键、CHECK 约束、触发器中的表名字段
    pub fn rename_table(
        &mut self,
        old_name: &TableName,
        new_name: &TableName,
    ) -> Result<(), PlanError> {
        let old_key = self.key(old_name);
        let new_key = self.key(new_name);

        if !self.tables.contains_key(&old_key) {
            return Err(PlanError::TableNotFound(old_name.qualified_name()));
        }
        if self.tables.contains_key(&new_key) {
            return Err(PlanError::Unsupported(format!(
                "table already exists: {}",
                new_name.qualified_name()
            )));
        }

        // 1. 移除旧 schema，修改表名后插入新 key
        let mut schema = self.tables.remove(&old_key).expect("checked above");
        schema.name = new_name.clone();
        self.tables.insert(new_key.clone(), schema);

        // 2. 更新关联索引的 table 字段
        if let Some(indexes) = self.indexes.remove(&old_key) {
            let mut new_indexes = indexes;
            for idx in new_indexes.iter_mut() {
                idx.table = new_name.clone();
            }
            self.indexes.insert(new_key.clone(), new_indexes);
        }

        // 3. 更新触发器的 table 字段
        if let Some(triggers) = self.triggers.remove(&old_key) {
            let mut new_triggers = triggers;
            for trg in new_triggers.iter_mut() {
                trg.table = new_name.clone();
            }
            self.triggers.insert(new_key.clone(), new_triggers);
        }

        // 4. 迁移外键约束 / CHECK 约束的 key
        if let Some(fks) = self.foreign_keys.remove(&old_key) {
            self.foreign_keys.insert(new_key.clone(), fks);
        }
        if let Some(checks) = self.check_constraints.remove(&old_key) {
            self.check_constraints.insert(new_key.clone(), checks);
        }

        Ok(())
    }
}

impl Catalog for InMemoryCatalog {
    fn table_exists(&self, name: &TableName) -> bool {
        let key = self.key(name);
        if self.tables.contains_key(&key) {
            return true;
        }
        // MySQL 兼容回退：schema.table → public.schema_table
        // Navicat 发送 SELECT * FROM `njszjt`.`soci_article`，
        // 但 szrsql 表存储为 public.njszjt_soci_article
        if let Some(schema) = &name.schema {
            let fallback = format!("public.{}_{}", schema.to_lowercase(), name.name.to_lowercase());
            if self.tables.contains_key(&fallback) {
                return true;
            }
        }
        // MySQL 兼容回退：table（无 schema）→ 遍历找 _table 后缀
        // Navicat 在 USE njszjt 后发送 SELECT * FROM soci_article，
        // 但 szrsql 表存储为 public.njszjt_soci_article
        if name.schema.is_none() {
            let suffix = format!("_{}", name.name.to_lowercase());
            for k in self.tables.keys() {
                if k.ends_with(&suffix) {
                    return true;
                }
            }
        }
        false
    }

    fn get_table(&self, name: &TableName) -> Option<TableSchema> {
        let key = self.key(name);
        if let Some(t) = self.tables.get(&key) {
            return Some(t.clone());
        }
        // MySQL 兼容回退：schema.table → public.schema_table
        if let Some(schema) = &name.schema {
            let fallback = format!("public.{}_{}", schema.to_lowercase(), name.name.to_lowercase());
            if let Some(t) = self.tables.get(&fallback) {
                return Some(t.clone());
            }
        }
        // MySQL 兼容回退：table（无 schema）→ 遍历找 _table 后缀
        if name.schema.is_none() {
            let suffix = format!("_{}", name.name.to_lowercase());
            for k in self.tables.keys() {
                if k.ends_with(&suffix) {
                    return self.tables.get(k).cloned();
                }
            }
        }
        None
    }

    fn list_tables(&self) -> Vec<TableName> {
        self.tables.values().map(|t| t.name.clone()).collect()
    }

    fn sequence_exists(&self, name: &TableName) -> bool {
        self.sequences.contains_key(&self.key(name))
    }

    fn get_sequence(&self, name: &TableName) -> Option<SequenceDefinition> {
        self.sequences.get(&self.key(name)).cloned()
    }

    fn list_sequences(&self) -> Vec<TableName> {
        self.sequences.values().map(|s| s.name.clone()).collect()
    }

    fn get_foreign_keys(&self, name: &TableName) -> Vec<ForeignKeyConstraint> {
        self.foreign_keys
            .get(&self.key(name))
            .cloned()
            .unwrap_or_default()
    }

    fn get_referencing_keys(&self, name: &TableName) -> Vec<ReferencingKey> {
        self.referencing_keys
            .get(&self.key(name))
            .cloned()
            .unwrap_or_default()
    }

    fn get_check_constraints(&self, name: &TableName) -> Vec<CheckConstraint> {
        self.check_constraints
            .get(&self.key(name))
            .cloned()
            .unwrap_or_default()
    }

    fn enum_type_exists(&self, name: &TableName) -> bool {
        self.enum_types.contains_key(&self.key(name))
    }

    fn get_enum_type(&self, name: &TableName) -> Option<EnumTypeDefinition> {
        self.enum_types.get(&self.key(name)).cloned()
    }

    fn list_enum_types(&self) -> Vec<TableName> {
        self.enum_types.values().map(|t| t.name.clone()).collect()
    }

    fn list_indexes(&self, table: &TableName) -> Vec<IndexDefinition> {
        self.indexes
            .get(&self.key(table))
            .cloned()
            .unwrap_or_default()
    }

    fn list_triggers(&self, table: &TableName) -> Vec<TriggerDefinition> {
        self.triggers
            .get(&self.key(table))
            .cloned()
            .unwrap_or_default()
    }

    fn get_trigger(&self, table: &TableName, name: &str) -> Option<TriggerDefinition> {
        self.triggers.get(&self.key(table)).and_then(|list| {
            list.iter()
                .find(|t| t.name.eq_ignore_ascii_case(name))
                .cloned()
        })
    }

    // Phase 6.15: 视图查询重写支持
    fn view_exists(&self, name: &TableName) -> bool {
        self.views.contains_key(&self.key(name))
    }

    fn get_view(&self, name: &TableName) -> Option<crate::materialized_view::ViewDefinition> {
        self.views.get(&self.key(name)).cloned()
    }

    /// 列出所有视图名 — 用于 pg_views 系统表
    fn list_views(&self) -> Vec<TableName> {
        self.views.values().map(|v| v.name.clone()).collect()
    }
}

// =====================================================================
//  LogicalPlan 数据结构
// =====================================================================

/// 逻辑计划节点 — 树形结构
#[derive(Debug, Clone, PartialEq)]
pub enum LogicalPlan {
    /// 表扫描（含可选列投影，None 表示全表）
    Scan {
        /// 表名
        table: TableName,
        /// 表别名
        alias: Option<String>,
        /// 表 Schema（执行器/优化器使用）
        schema: TableSchema,
    },
    /// 索引扫描 — Phase 5.7
    ///
    /// 通过索引访问表数据，避免全表扫描。由 `IndexSelection` 规则在
    /// `Filter { predicate, input: Scan }` 模式下选择产生。
    ///
    /// `predicate` 包含完整过滤条件（含索引列谓词 + 残余谓词），
    /// 执行器先按索引列谓词做点查/范围查询，再对结果应用完整谓词过滤。
    IndexScan {
        /// 表名
        table: TableName,
        /// 表别名
        alias: Option<String>,
        /// 表 Schema
        schema: TableSchema,
        /// 索引名
        index_name: String,
        /// 索引列名（按声明顺序）
        index_columns: Vec<String>,
        /// 过滤谓词（含索引列条件 + 残余条件）
        predicate: Expr,
    },
    /// 投影（SELECT cols）
    Projection {
        /// 投影表达式列表
        exprs: Vec<(Expr, Option<String>)>,
        /// 输出列名（与 exprs 一一对应）
        output_names: Vec<String>,
        /// 子计划
        input: Box<LogicalPlan>,
    },
    /// 过滤（WHERE）
    Filter {
        /// 过滤条件
        predicate: Expr,
        /// 子计划
        input: Box<LogicalPlan>,
    },
    /// JOIN
    Join {
        /// JOIN 类型
        join_type: JoinType,
        /// JOIN 条件
        condition: JoinCondition,
        /// 左子计划
        left: Box<LogicalPlan>,
        /// 右子计划
        right: Box<LogicalPlan>,
    },
    /// 聚合（GROUP BY + 聚合函数）
    Aggregate {
        /// GROUP BY 表达式
        group_exprs: Vec<Expr>,
        /// 聚合函数 + 别名（提取自 SELECT 投影）
        aggregates: Vec<AggregateExpr>,
        /// HAVING 条件
        having: Option<Expr>,
        /// 子计划
        input: Box<LogicalPlan>,
    },
    /// 窗口函数（Phase 6.2）
    ///
    /// 在 Aggregate 之后、Projection 之前求值窗口函数。
    /// 输出 = 输入列 ++ 窗口函数结果列，按 window_funcs 顺序追加。
    Window {
        /// 窗口函数表达式列表
        window_funcs: Vec<WindowFunctionExpr>,
        /// 子计划
        input: Box<LogicalPlan>,
    },
    /// 排序（ORDER BY）
    Sort {
        /// 排序键
        order_by: Vec<OrderByExpr>,
        /// 子计划
        input: Box<LogicalPlan>,
    },
    /// LIMIT + OFFSET
    Limit {
        /// LIMIT 表达式
        limit: Option<Expr>,
        /// OFFSET 表达式
        offset: Option<Expr>,
        /// 子计划
        input: Box<LogicalPlan>,
    },
    /// DISTINCT
    Distinct {
        /// 子计划
        input: Box<LogicalPlan>,
    },
    /// INSERT
    Insert {
        /// 目标表
        table: TableName,
        /// 目标表 Schema
        schema: TableSchema,
        /// 显式列名（None 表示全部列）
        columns: Option<Vec<String>>,
        /// 数据源
        source: InsertSourcePlan,
        /// ON CONFLICT 处理（None 表示普通 INSERT）
        on_conflict: Option<OnConflict>,
        /// RETURNING 子句（None 表示无 RETURNING）
        returning: Option<Vec<SelectItem>>,
    },
    /// REPLACE INTO — MySQL 扩展（Phase 3.25）
    ///
    /// 主键/UNIQUE 冲突时 DELETE 旧行 + INSERT 新行；无冲突时直接 INSERT。
    Replace {
        /// 目标表
        table: TableName,
        /// 目标表 Schema
        schema: TableSchema,
        /// 显式列名（None 表示全部列）
        columns: Option<Vec<String>>,
        /// 数据源（与 INSERT 共用）
        source: InsertSourcePlan,
    },
    /// UPDATE
    Update {
        /// 目标表
        table: TableName,
        /// 目标表 Schema
        schema: TableSchema,
        /// SET 赋值
        assignments: Vec<Assignment>,
        /// WHERE 子计划（None 表示无 WHERE，更新所有行）
        source: Option<Box<LogicalPlan>>,
        /// RETURNING 子句（None 表示无 RETURNING）
        returning: Option<Vec<SelectItem>>,
    },
    /// DELETE
    Delete {
        /// 目标表
        table: TableName,
        /// 目标表 Schema
        schema: TableSchema,
        /// WHERE 子计划（None 表示无 WHERE，删除所有行）
        source: Option<Box<LogicalPlan>>,
        /// RETURNING 子句（None 表示无 RETURNING）
        returning: Option<Vec<SelectItem>>,
    },
    /// CREATE TABLE / CREATE TEMPORARY TABLE
    CreateTable {
        /// 表名
        name: TableName,
        /// 列定义
        columns: Vec<ColumnDefinition>,
        /// 表级约束
        constraints: Vec<TableConstraint>,
        /// IF NOT EXISTS
        if_not_exists: bool,
        /// 是否为临时表 — Phase 3.28
        temporary: bool,
        /// ON COMMIT 行为 — Phase 3.28
        on_commit: Option<OnCommitAction>,
    },
    /// DROP TABLE
    DropTable {
        /// 表名列表
        names: Vec<TableName>,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT
        cascade: bool,
    },
    /// CREATE INDEX
    CreateIndex {
        /// 索引名
        name: Option<String>,
        /// 表名
        table: TableName,
        /// 索引列
        columns: Vec<IndexColumn>,
        /// UNIQUE
        unique: bool,
        /// IF NOT EXISTS
        if_not_exists: bool,
    },
    /// DROP INDEX
    DropIndex {
        /// 索引名列表
        names: Vec<String>,
        /// IF EXISTS
        if_exists: bool,
    },
    /// CREATE SEQUENCE — Phase 3.22
    CreateSequence {
        /// 序列定义
        definition: SequenceDefinition,
        /// IF NOT EXISTS
        if_not_exists: bool,
    },
    /// DROP SEQUENCE — Phase 3.22
    DropSequence {
        /// 序列名列表
        names: Vec<TableName>,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT（当前仅记录）
        cascade: bool,
    },
    /// CREATE TYPE name AS ENUM (...) — Phase 3.31
    CreateType {
        /// ENUM 类型定义
        definition: EnumTypeDefinition,
        /// IF NOT EXISTS（当前 sqlparser 不解析此子句，始终为 false）
        if_not_exists: bool,
    },
    /// DROP TYPE — Phase 3.31
    DropType {
        /// 类型名列表
        names: Vec<TableName>,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT（当前仅记录）
        cascade: bool,
    },
    /// ALTER TYPE name action — Phase 3.31
    AlterType {
        /// 类型名
        name: TableName,
        /// 操作
        action: AlterTypeAction,
    },
    /// ALTER TABLE — Phase F-10
    ///
    /// 计划层仅做表存在性校验与基本语义校验，实际 catalog 修改由 executor 执行。
    /// 操作列表按顺序执行：ADD COLUMN / DROP COLUMN / RENAME COLUMN / RENAME TABLE /
    /// ALTER COLUMN TYPE / SET DEFAULT / DROP DEFAULT / SET NOT NULL / DROP NOT NULL /
    /// ADD CONSTRAINT / DROP CONSTRAINT。
    AlterTable {
        /// 目标表名
        name: TableName,
        /// IF EXISTS（表不存在时是否跳过）
        if_exists: bool,
        /// 仅作用于表本身（PG `ALTER TABLE ONLY`，不影响子表）
        only: bool,
        /// 操作列表（按顺序执行）
        operations: Vec<AlterTableOperation>,
    },
    /// TRUNCATE TABLE — 清空表数据（保留表结构）
    ///
    /// 计划层校验所有目标表存在，执行器实际清空数据文件并重置自增序列。
    /// 各方言均支持：PG/MySQL/Oracle/SQL Server/SQLite。
    Truncate {
        /// 待清空的表名列表
        names: Vec<TableName>,
        /// IF EXISTS（PG/MySQL 支持）
        if_exists: bool,
        /// CASCADE / RESTRICT（PG/Oracle 支持，当前仅记录）
        cascade: bool,
    },
    /// 空计划（BEGIN / COMMIT / ROLLBACK / SAVEPOINT 等事务控制语句）
    Empty,
    /// 虚拟单行表（SELECT without FROM，类似 PG dual）— Phase 3.22
    Dual,
    /// MERGE INTO ... USING ... ON ... WHEN ... THEN ... — Phase 3.24
    Merge {
        /// 目标表名
        target: TableName,
        /// 目标表别名
        target_alias: Option<String>,
        /// 目标表 Schema
        target_schema: TableSchema,
        /// 源表因子
        source: TableFactor,
        /// 源表 Schema（仅当源是物理表时有效）
        source_schema: Option<TableSchema>,
        /// ON 条件
        on: Expr,
        /// WHEN 子句列表
        clauses: Vec<MergeClause>,
    },
    /// PREPARE name [ (types...) ] AS statement — Phase 3.26
    ///
    /// 存储 AST（不立即 plan），EXECUTE 时再 plan 内部语句。
    Prepare {
        /// 预处理语句名
        name: String,
        /// 参数类型声明（仅记录，当前不强制校验）
        parameter_types: Vec<ColumnType>,
        /// 被预处理的 SQL 语句（AST）
        statement: Box<Statement>,
    },
    /// EXECUTE name (params...) — Phase 3.26
    Execute {
        /// 预处理语句名
        name: String,
        /// 实际参数值表达式列表
        parameters: Vec<Expr>,
    },
    /// DEALLOCATE [PREPARE] { name | ALL } — Phase 3.26
    Deallocate {
        /// 待删除的预处理语句名（None 表示 DEALLOCATE ALL）
        name: Option<String>,
    },
    /// 集合操作（INTERSECT / EXCEPT / UNION）— Phase 3.27
    ///
    /// 执行 left 与 right 的集合运算；ORDER BY / LIMIT / OFFSET 由外层包装处理。
    SetOp {
        /// 操作类型
        op: SetOperator,
        /// 量词（ALL / DISTINCT / None）
        quantifier: SetQuantifier,
        /// 左侧子计划
        left: Box<LogicalPlan>,
        /// 右侧子计划
        right: Box<LogicalPlan>,
    },
    /// SHOW TABLES — Phase 3.34
    ///
    /// 执行器枚举 catalog 中所有表，返回单列结果集。
    ShowTables,
    /// SHOW CREATE TABLE name — Phase 3.34
    ///
    /// 执行器根据 catalog 中的 Schema 重建 DDL 文本。
    ShowCreateTable {
        /// 目标表名
        name: TableName,
    },
    /// SET NAMES 'charset' [COLLATE 'collation'] — Phase 3.34
    ///
    /// 执行器将 charset/collation 写入 SessionState。
    SetNames {
        /// 字符集名称
        charset: String,
        /// 可选 collation
        collation: Option<String>,
    },
    /// SET variable = value — Phase 3.34
    ///
    /// 执行器求值 value 表达式，将 (variable, value) 写入 SessionState。
    SetVariable {
        /// 参数名
        variable: String,
        /// 参数值表达式
        value: Expr,
    },
    /// SHOW variable — Phase 3.34
    ///
    /// 执行器从 SessionState 读取参数值，返回单行单列结果集。
    ShowVariable {
        /// 参数名
        variable: String,
    },
    /// FLASHBACK TRANSACTION <txn_id> — Phase 3.35
    ///
    /// 执行器从 TransactionHistory 取出该事务的快照，对每个受影响表调用 restore。
    FlashbackTransaction {
        /// 事务 ID
        txn_id: u64,
    },
    /// FLASHBACK TABLE <name> TO TIMESTAMP '<ts>' — Phase 3.35
    ///
    /// 执行器从 TransactionHistory 查找 commit_ts <= ts 的最近事务，
    /// 返回该事务"事务前"的表快照内容作为查询结果。
    FlashbackTable {
        /// 目标表名
        table: TableName,
        /// 时间戳字符串（ISO 8601 或可解析格式）
        timestamp: String,
    },
    /// LISTEN <channel> — Phase 4.6
    ///
    /// 执行器注册当前会话监听指定频道。无结果集，CommandComplete 标签 "LISTEN"。
    Listen {
        /// 频道名
        channel: String,
    },
    /// UNLISTEN <channel> / UNLISTEN * — Phase 4.6
    ///
    /// 执行器取消当前会话监听指定频道（或所有频道）。CommandComplete 标签 "UNLISTEN"。
    Unlisten {
        /// 频道名；`*` 表示取消所有
        channel: String,
    },
    /// NOTIFY <channel> [, <payload>] — Phase 4.6
    ///
    /// 执行器向指定频道发送通知。CommandComplete 标签 "NOTIFY"。
    /// 由会话层检查监听集合，决定是否向当前会话发送 NotificationResponse。
    Notify {
        /// 频道名
        channel: String,
        /// 负载字符串
        payload: String,
    },
    /// COPY FROM / COPY TO — Phase 4.8
    ///
    /// 执行器在会话层处理：
    /// - COPY FROM：读取文件 → CSV/TEXT 解析 → 批量 INSERT
    /// - COPY TO：执行 SELECT（或表扫描）→ 序列化为 CSV/TEXT → 写入文件
    Copy {
        /// 目标：表名或 SELECT 查询
        target: CopyTarget,
        /// 列列表（可选）
        columns: Option<Vec<String>>,
        /// 方向：FROM 或 TO
        direction: CopyDirection,
        /// 文件路径
        file_path: String,
        /// 格式选项
        options: CopyOptions,
    },
    /// 共享子计划 — Phase 5.8
    ///
    /// 由 `CommonSubexpressionElimination` 规则在检测到重复子树时包装产生。
    /// 执行器首次执行后将结果缓存到 `memo_cache`，后续相同 `id` 的 `MemoRef` 直接读缓存。
    ///
    /// 仅对纯查询节点（Scan/IndexScan/Filter/Projection/Join/Aggregate/Sort/Limit/
    /// Distinct/SetOp/Empty/Dual）做 CSE；DML/DDL 节点不参与。
    Shared {
        /// 共享 ID（CSE 规则分配，全局唯一）
        id: u64,
        /// 被共享的子计划
        plan: Box<LogicalPlan>,
    },
    /// 共享子计划引用 — Phase 5.8
    ///
    /// 替换计划树中第二次及之后出现的相同子树。执行器直接从 `memo_cache` 读取结果。
    /// `schema` 与对应 `Shared` 节点的输出 schema 一致（由 CSE 规则捕获）。
    MemoRef {
        /// 引用的 Shared ID
        id: u64,
        /// 输出 Schema（与原 Shared 节点一致）
        schema: TableSchema,
    },
    /// WITH 子句（CTE）— Phase 6.1
    ///
    /// 表示 `WITH [RECURSIVE] cte1 AS (...), cte2 AS (...) <body>`。
    /// 执行时先依次物化每个 CTE（普通 CTE 一次性物化，递归 CTE 迭代至不动点），
    /// 将结果存入执行器 CTE 缓存，然后执行 `body`（body 中通过 `CteRef` 引用 CTE）。
    With {
        /// CTE 条目列表（按声明顺序）
        ctes: Vec<CteEntry>,
        /// 主查询体
        input: Box<LogicalPlan>,
    },
    /// CTE 引用 — Phase 6.1
    ///
    /// 在 FROM 中引用 CTE 名时产生。执行器从 CTE 缓存读取物化结果。
    /// `schema` 为 CTE 输出 schema（含列别名重命名）。
    CteRef {
        /// CTE 名称（小写）
        name: String,
        /// CTE 输出 Schema
        schema: TableSchema,
    },
    /// CREATE TRIGGER — Phase 6.4
    CreateTrigger {
        /// 触发器定义
        definition: TriggerDefinition,
        /// OR REPLACE
        or_replace: bool,
        /// IF NOT EXISTS
        if_not_exists: bool,
    },
    /// DROP TRIGGER — Phase 6.4
    DropTrigger {
        /// 触发器名
        name: String,
        /// 所属表名
        table: TableName,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT（当前仅记录）
        cascade: bool,
    },
    /// CREATE VIEW / CREATE MATERIALIZED VIEW — Phase 6.10
    CreateView {
        /// 视图名
        name: TableName,
        /// 显式列别名（空 Vec 表示未指定）
        columns: Vec<String>,
        /// 视图查询体
        query: Box<Select>,
        /// 是否为物化视图
        materialized: bool,
        /// IF NOT EXISTS
        if_not_exists: bool,
        /// OR REPLACE
        or_replace: bool,
    },
    /// DROP VIEW / DROP MATERIALIZED VIEW — Phase 6.10
    DropView {
        /// 视图名列表
        names: Vec<TableName>,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT（当前仅记录）
        cascade: bool,
        /// 是否为物化视图
        materialized: bool,
    },
    /// REFRESH MATERIALIZED VIEW — Phase 6.10
    RefreshMaterializedView {
        /// 物化视图名
        name: TableName,
        /// WITH DATA / WITH NO DATA（当前统一按 WITH DATA 处理）
        with_data: bool,
    },
    /// 物化视图扫描 — Phase 6.15
    ///
    /// 当查询引用物化视图名时，路由到物化视图存储表（而非展开视图查询）。
    /// 执行器通过 `materialized_view_stores` 注册表按名查找存储。
    MaterializedViewScan {
        /// 物化视图名
        name: TableName,
        /// 表别名
        alias: Option<String>,
        /// 物化视图 Schema（列名 + 类型，与存储表一致）
        schema: TableSchema,
    },
    /// CREATE FUNCTION — Phase 6.5（P0-5 修复：注册函数定义到 catalog）
    ///
    /// 将函数元数据（名称、参数、返回类型、language、body、波动性、strict 等）
    /// 注册到 catalog，使后续函数调用可路由到 PL/pgSQL 解释器或 UDF 注册表。
    ///
    /// 注意：函数体执行（PL/pgSQL 解释器调用）在表达式求值时按需触发，
    /// 此计划节点仅负责元数据注册，不执行函数体。
    CreateFunction {
        /// 函数名（含可选 schema 前缀）
        name: String,
        /// 参数列表
        parameters: Vec<crate::ast::FunctionParameter>,
        /// 返回类型原文
        return_type: String,
        /// 函数语言（plpgsql / sql / c 等）
        language: String,
        /// 函数体原文
        body: String,
        /// OR REPLACE
        or_replace: bool,
        /// 函数波动性
        volatility: Option<crate::ast::FunctionVolatility>,
        /// STRICT
        strict: bool,
        /// SECURITY DEFINER
        security_definer: bool,
    },
    /// DROP FUNCTION — Phase 6.5（P0-5 修复）
    DropFunction {
        /// 函数名
        name: String,
        /// 参数类型列表（用于重载解析）
        parameter_types: Vec<String>,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT
        cascade: bool,
    },
}

impl LogicalPlan {
    /// OPT-6：收集当前计划树中引用的所有物理表名（小写，去重）。
    ///
    /// 用于 SELECT / COPY EXPORT 路径仅锁定查询实际引用的表，避免对会话中
    /// 所有表加锁造成不必要的并发阻塞。
    ///
    /// 收集范围：
    /// - 直接引用：`Scan` / `IndexScan` / `Insert` / `Replace` / `Update` /
    ///   `Delete` / `Merge` / `Truncate` / `CreateTable` / `DropTable` /
    ///   `CreateIndex` / `AlterTable` / `ShowCreateTable` / `FlashbackTable`
    ///   / `MaterializedViewScan` / `DropTrigger` 中出现的表名
    /// - 递归子计划：`Projection` / `Filter` / `Join` / `Aggregate` / `Window`
    ///   / `Sort` / `Limit` / `Distinct` / `SetOp` / `Shared` / `With`
    /// - DML 的 `source` 子计划（UPDATE / DELETE 的 WHERE 子查询）
    ///
    /// 不收集：CTE 名称（`CteRef`）、临时表名（执行器内部处理）。
    /// 返回空集合表示计划不引用任何物理表（如 `SELECT 1`、`Empty`、`Dual`）。
    pub fn collect_referenced_table_names(&self) -> std::collections::HashSet<String> {
        let mut names = std::collections::HashSet::new();
        self.collect_table_names_into(&mut names);
        names
    }

    fn collect_table_names_into(&self, names: &mut std::collections::HashSet<String>) {
        use std::collections::HashSet;
        // 递归处理子计划
        let push_name = |names: &mut HashSet<String>, n: &TableName| {
            names.insert(n.name.to_lowercase());
        };
        match self {
            LogicalPlan::Scan { table, .. } | LogicalPlan::IndexScan { table, .. } => {
                push_name(names, table);
            }
            LogicalPlan::Insert { table, .. } | LogicalPlan::Replace { table, .. } => {
                push_name(names, table);
            }
            LogicalPlan::Update { table, source, .. } => {
                push_name(names, table);
                if let Some(src) = source {
                    src.collect_table_names_into(names);
                }
            }
            LogicalPlan::Delete { table, source, .. } => {
                push_name(names, table);
                if let Some(src) = source {
                    src.collect_table_names_into(names);
                }
            }
            LogicalPlan::Merge { target, .. } => {
                push_name(names, target);
            }
            LogicalPlan::Truncate { names: ns, .. } => {
                for n in ns {
                    push_name(names, n);
                }
            }
            LogicalPlan::CreateTable { name, .. }
            | LogicalPlan::ShowCreateTable { name, .. }
            | LogicalPlan::AlterTable { name, .. }
            | LogicalPlan::FlashbackTable { table: name, .. } => {
                push_name(names, name);
            }
            LogicalPlan::DropTable { names: ns, .. } => {
                for n in ns {
                    push_name(names, n);
                }
            }
            LogicalPlan::CreateIndex { table, .. } => {
                push_name(names, table);
            }
            LogicalPlan::DropTrigger { table, .. } => {
                push_name(names, table);
            }
            LogicalPlan::MaterializedViewScan { name, .. } => {
                push_name(names, name);
            }
            // 递归子计划
            LogicalPlan::Projection { input, .. }
            | LogicalPlan::Filter { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::Window { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Distinct { input, .. }
            | LogicalPlan::Shared { plan: input, .. } => {
                input.collect_table_names_into(names);
            }
            LogicalPlan::Join { left, right, .. }
            | LogicalPlan::SetOp { left, right, .. } => {
                left.collect_table_names_into(names);
                right.collect_table_names_into(names);
            }
            LogicalPlan::With { ctes, input } => {
                for cte in ctes {
                    match cte {
                        CteEntry::Simple { plan, .. } => plan.collect_table_names_into(names),
                        CteEntry::Recursive {
                            anchor, recursive, ..
                        } => {
                            anchor.collect_table_names_into(names);
                            recursive.collect_table_names_into(names);
                        }
                    }
                }
                input.collect_table_names_into(names);
            }
            // 不引用物理表的节点
            LogicalPlan::Empty
            | LogicalPlan::Dual
            | LogicalPlan::ShowTables
            | LogicalPlan::SetNames { .. }
            | LogicalPlan::SetVariable { .. }
            | LogicalPlan::ShowVariable { .. }
            | LogicalPlan::FlashbackTransaction { .. }
            | LogicalPlan::Listen { .. }
            | LogicalPlan::Unlisten { .. }
            | LogicalPlan::Notify { .. }
            | LogicalPlan::Copy { .. }
            | LogicalPlan::MemoRef { .. }
            | LogicalPlan::CteRef { .. }
            | LogicalPlan::CreateSequence { .. }
            | LogicalPlan::DropSequence { .. }
            | LogicalPlan::CreateType { .. }
            | LogicalPlan::DropType { .. }
            | LogicalPlan::AlterType { .. }
            | LogicalPlan::DropIndex { .. }
            | LogicalPlan::CreateView { .. }
            | LogicalPlan::DropView { .. }
            | LogicalPlan::RefreshMaterializedView { .. }
            | LogicalPlan::CreateFunction { .. }
            | LogicalPlan::DropFunction { .. }
            | LogicalPlan::Prepare { .. }
            | LogicalPlan::Execute { .. }
            | LogicalPlan::Deallocate { .. }
            | LogicalPlan::CreateTrigger { .. } => {}
        }
    }
}

/// CTE 条目（普通或递归）— Phase 6.1
#[derive(Debug, Clone, PartialEq)]
pub enum CteEntry {
    /// 普通非递归 CTE：一次性物化 `plan` 的结果
    Simple {
        /// CTE 名称（小写）
        name: String,
        /// CTE 查询计划
        plan: Box<LogicalPlan>,
        /// CTE 输出 Schema
        schema: TableSchema,
    },
    /// 递归 CTE：`anchor UNION [ALL] recursive_part`
    ///
    /// 执行流程：
    /// 1. 物化 anchor → R₀
    /// 2. 用 R_i 执行 recursive_part → R_{i+1}（仅新增行）
    /// 3. R_{i+1} 为空则停止；否则 R_{i+1} 累加到 R₀（UNION ALL 直接拼接，UNION 去重后拼接）
    /// 4. 最终 R₀ 即为 CTE 物化结果
    Recursive {
        /// CTE 名称（小写）
        name: String,
        /// anchor 计划（非递归部分）
        anchor: Box<LogicalPlan>,
        /// recursive 计划（引用 CTE 自身的部分）
        recursive: Box<LogicalPlan>,
        /// 集合量词：true=UNION ALL（保留重复），false=UNION [DISTINCT]（去重）
        all: bool,
        /// CTE 输出 Schema
        schema: TableSchema,
    },
}

/// 聚合函数表达式（从 SELECT 投影中提取的聚合函数）
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateExpr {
    /// 函数名（lowercase，如 count / sum / avg / min / max）
    pub func_name: String,
    /// DISTINCT 标志
    pub distinct: bool,
    /// 参数列表（COUNT(*) 时为空 Vec）
    pub args: Vec<Expr>,
    /// 输出别名
    pub alias: Option<String>,
}

/// 窗口函数表达式 — Phase 6.2
///
/// 表示 SELECT 投影中的 `func(args) OVER (window_spec)` 调用。
/// 在 `LogicalPlan::Window` 节点中按声明顺序求值，结果追加到行尾。
#[derive(Debug, Clone, PartialEq)]
pub struct WindowFunctionExpr {
    /// 函数名（lowercase，如 row_number / rank / sum / lag ...）
    pub func_name: String,
    /// DISTINCT 标志（仅 COUNT 等少数窗口函数支持，多数忽略）
    pub distinct: bool,
    /// 参数列表（无参函数如 ROW_NUMBER() 为空 Vec）
    pub args: Vec<Expr>,
    /// 窗口规格（PARTITION BY / ORDER BY / 帧）
    pub window: WindowSpec,
    /// 输出别名（若 SELECT 中以 `AS alias` 指定）
    pub alias: Option<String>,
}

/// INSERT 数据源计划
#[derive(Debug, Clone, PartialEq)]
pub enum InsertSourcePlan {
    /// VALUES (...) 列表
    Values(Vec<Vec<Expr>>),
    /// INSERT INTO ... SELECT ...
    Select(Box<LogicalPlan>),
    /// DEFAULT VALUES
    DefaultValues,
}

// =====================================================================
//  Planner
// =====================================================================

/// 逻辑计划生成器
pub struct Planner<'a> {
    catalog: &'a dyn Catalog,
    /// CTE 作用域栈 — Phase 6.1
    ///
    /// 每层作用域是 `CTE 名（小写） → (LogicalPlan, TableSchema)` 的映射。
    /// - 进入 `WITH` 子句时压入新作用域
    /// - 解析 `TableFactor::Table` 时从栈顶向下查找 CTE 名
    /// - 退出 `plan_select` 时弹出当前作用域（用 RAII 模式）
    ///
    /// 使用 `RefCell` 以便在 `&self` 方法中修改。
    cte_scopes: RefCell<Vec<HashMap<String, (LogicalPlan, TableSchema)>>>,
}

impl<'a> Planner<'a> {
    /// 创建 Planner
    pub fn new(catalog: &'a dyn Catalog) -> Self {
        Self {
            catalog,
            cte_scopes: RefCell::new(Vec::new()),
        }
    }

    /// 将 AST Statement 转换为 LogicalPlan
    pub fn plan_statement(&self, stmt: Statement) -> Result<LogicalPlan, PlanError> {
        match stmt {
            Statement::Select(select) => self.plan_select(*select),
            Statement::Insert {
                table,
                columns,
                source,
                on_conflict,
                returning,
            } => self.plan_insert(table, columns, source, on_conflict, returning),
            Statement::Replace {
                table,
                columns,
                source,
            } => self.plan_replace(table, columns, source),
            Statement::Update {
                table,
                alias,
                assignments,
                from,
                where_clause,
                returning,
            } => {
                if !from.is_empty() {
                    return Err(PlanError::Unsupported(
                        "UPDATE FROM not supported in planner".into(),
                    ));
                }
                self.plan_update(table, alias, assignments, where_clause, returning)
            }
            Statement::Delete {
                table,
                alias,
                using,
                where_clause,
                returning,
            } => {
                if !using.is_empty() {
                    return Err(PlanError::Unsupported(
                        "DELETE USING not supported in planner".into(),
                    ));
                }
                self.plan_delete(table, alias, where_clause, returning)
            }
            Statement::CreateTable {
                name,
                columns,
                constraints,
                if_not_exists,
                temporary,
                on_commit,
            } => {
                // 临时表可遮蔽普通表 — Phase 3.28
                // 临时表存在性冲突由执行阶段 TempTableStore 检查，规划阶段不检查 catalog。
                // 仅对普通表（temporary=false）在规划阶段检查同名冲突。
                if !temporary && !if_not_exists && self.catalog.table_exists(&name) {
                    return Err(PlanError::TableAlreadyExists(name.qualified_name()));
                }
                // Phase 3.31: 解析 custom_type_name — 若该名是已注册的 ENUM 类型，
                // 将 data_type 从 Text（parser 降级）改写为 ColumnType::Enum(labels)
                let columns = columns
                    .into_iter()
                    .map(|mut col| {
                        if let Some(type_name) = &col.custom_type_name {
                            let lookup_name = TableName::new(type_name.clone());
                            if let Some(enum_def) = self.catalog.get_enum_type(&lookup_name) {
                                col.data_type = ColumnType::Enum(enum_def.labels.clone());
                            }
                            // 若不是已知 ENUM 类型，保持 data_type 不变（Text 降级）
                        }
                        col
                    })
                    .collect::<Vec<_>>();
                Ok(LogicalPlan::CreateTable {
                    name,
                    columns,
                    constraints,
                    if_not_exists,
                    temporary,
                    on_commit,
                })
            }
            Statement::DropTable {
                names,
                if_exists,
                cascade,
            } => {
                if !if_exists {
                    for n in &names {
                        if !self.catalog.table_exists(n) {
                            return Err(PlanError::TableNotFound(n.qualified_name()));
                        }
                    }
                }
                Ok(LogicalPlan::DropTable {
                    names,
                    if_exists,
                    cascade,
                })
            }
            Statement::CreateIndex {
                name,
                table,
                columns,
                unique,
                if_not_exists,
            } => {
                if !self.catalog.table_exists(&table) {
                    return Err(PlanError::TableNotFound(table.qualified_name()));
                }
                Ok(LogicalPlan::CreateIndex {
                    name,
                    table,
                    columns,
                    unique,
                    if_not_exists,
                })
            }
            Statement::DropIndex { names, if_exists } => {
                Ok(LogicalPlan::DropIndex { names, if_exists })
            }
            Statement::CreateSequence {
                name,
                if_not_exists,
                start,
                increment,
                min_value,
                max_value,
                cycle,
            } => {
                if !if_not_exists && self.catalog.sequence_exists(&name) {
                    return Err(PlanError::Unsupported(format!(
                        "sequence already exists: {}",
                        name.qualified_name()
                    )));
                }
                let definition = SequenceDefinition {
                    name,
                    start,
                    increment,
                    min_value,
                    max_value,
                    cycle,
                };
                Ok(LogicalPlan::CreateSequence {
                    definition,
                    if_not_exists,
                })
            }
            Statement::DropSequence {
                names,
                if_exists,
                cascade,
            } => {
                // 序列存在性由 executor 在 SequenceStore 上检查（catalog 不持有序列状态）
                Ok(LogicalPlan::DropSequence {
                    names,
                    if_exists,
                    cascade,
                })
            }
            Statement::Begin { .. }
            | Statement::Commit
            | Statement::Rollback { .. }
            | Statement::Savepoint(_)
            | Statement::ReleaseSavepoint(_)
            | Statement::SetTransaction { .. } => Ok(LogicalPlan::Empty),
            Statement::Explain { statement, .. } => {
                // EXPLAIN 透传内部计划（执行器负责打印）
                self.plan_statement(*statement)
            }
            Statement::Merge {
                target,
                target_alias,
                source,
                on,
                clauses,
            } => {
                // 验证目标表存在
                if !self.catalog.table_exists(&target) {
                    return Err(PlanError::TableNotFound(target.qualified_name()));
                }
                let target_schema = self
                    .catalog
                    .get_table(&target)
                    .ok_or_else(|| PlanError::TableNotFound(target.qualified_name()))?
                    .clone();
                // 验证源表存在并获取 schema（仅当源是物理表时）
                let source_schema = match &source {
                    TableFactor::Table { name, .. } => {
                        if !self.catalog.table_exists(name) {
                            return Err(PlanError::TableNotFound(name.qualified_name()));
                        }
                        Some(
                            self.catalog
                                .get_table(name)
                                .ok_or_else(|| PlanError::TableNotFound(name.qualified_name()))?
                                .clone(),
                        )
                    }
                    _ => None, // 子查询源留待后续支持
                };
                Ok(LogicalPlan::Merge {
                    target,
                    target_alias,
                    target_schema,
                    source,
                    source_schema,
                    on,
                    clauses,
                })
            }
            // Phase 3.26: PREPARE / EXECUTE / DEALLOCATE
            // 预处理语句不立即 plan，仅存储 AST（EXECUTE 时再 plan）
            Statement::Prepare {
                name,
                parameter_types,
                statement,
            } => Ok(LogicalPlan::Prepare {
                name,
                parameter_types,
                statement,
            }),
            Statement::Execute { name, parameters } => {
                Ok(LogicalPlan::Execute { name, parameters })
            }
            Statement::Deallocate { name } => Ok(LogicalPlan::Deallocate { name }),
            // Phase 3.31: CREATE TYPE / DROP TYPE / ALTER TYPE
            Statement::CreateType {
                name,
                as_enum,
                if_not_exists,
            } => {
                if !if_not_exists && self.catalog.enum_type_exists(&name) {
                    return Err(PlanError::Unsupported(format!(
                        "type already exists: {}",
                        name.qualified_name()
                    )));
                }
                let definition = EnumTypeDefinition::new(name, as_enum);
                Ok(LogicalPlan::CreateType {
                    definition,
                    if_not_exists,
                })
            }
            Statement::DropType {
                names,
                if_exists,
                cascade,
            } => {
                // 类型存在性由 executor 在 catalog 上检查（catalog 是不可变引用，
                // 无法在此移除）。这里仅记录计划。
                if !if_exists {
                    for n in &names {
                        if !self.catalog.enum_type_exists(n) {
                            return Err(PlanError::Unsupported(format!(
                                "type not found: {}",
                                n.qualified_name()
                            )));
                        }
                    }
                }
                Ok(LogicalPlan::DropType {
                    names,
                    if_exists,
                    cascade,
                })
            }
            Statement::AlterType { name, action } => {
                if !self.catalog.enum_type_exists(&name) {
                    return Err(PlanError::Unsupported(format!(
                        "type not found: {}",
                        name.qualified_name()
                    )));
                }
                // ADD VALUE 的 if_not_exists=true 时，若值已存在，executor 需静默跳过
                // 这里仅传递计划，由 executor 实际修改 catalog
                Ok(LogicalPlan::AlterType { name, action })
            }
            // Phase 3.34: SHOW / SET 命令
            Statement::ShowTables => Ok(LogicalPlan::ShowTables),
            Statement::ShowCreateTable { name } => {
                if !self.catalog.table_exists(&name) {
                    return Err(PlanError::TableNotFound(name.qualified_name()));
                }
                Ok(LogicalPlan::ShowCreateTable { name })
            }
            Statement::SetNames { charset, collation } => {
                Ok(LogicalPlan::SetNames { charset, collation })
            }
            Statement::SetVariable { variable, value } => {
                Ok(LogicalPlan::SetVariable { variable, value })
            }
            Statement::ShowVariable { variable } => Ok(LogicalPlan::ShowVariable { variable }),
            // Phase 3.35: FLASHBACK 语句（无需 catalog 校验，由 executor 配合 TransactionHistory 执行）
            Statement::FlashbackTransaction { txn_id } => {
                Ok(LogicalPlan::FlashbackTransaction { txn_id })
            }
            Statement::FlashbackTable { table, timestamp } => {
                Ok(LogicalPlan::FlashbackTable { table, timestamp })
            }
            // Phase 4.6: LISTEN/UNLISTEN/NOTIFY — 无需 catalog 校验，直接透传到执行器
            Statement::Listen { channel } => Ok(LogicalPlan::Listen { channel }),
            Statement::Unlisten { channel } => Ok(LogicalPlan::Unlisten { channel }),
            Statement::Notify { channel, payload } => Ok(LogicalPlan::Notify { channel, payload }),
            // Phase 4.8: COPY FROM/TO — 直接透传到执行器（会话层处理文件 I/O 与批量 INSERT）
            // COPY FROM 时校验目标表存在；COPY TO 时表存在性由执行器在扫描时校验
            Statement::Copy {
                target,
                columns,
                direction,
                file_path,
                options,
            } => {
                if let CopyTarget::Table(ref name) = target {
                    if !self.catalog.table_exists(name) {
                        return Err(PlanError::TableNotFound(name.qualified_name()));
                    }
                }
                Ok(LogicalPlan::Copy {
                    target,
                    columns,
                    direction,
                    file_path,
                    options,
                })
            }
            // Phase 6.4: CREATE TRIGGER / DROP TRIGGER
            Statement::CreateTrigger {
                definition,
                or_replace,
                if_not_exists,
            } => {
                // 校验目标表存在
                if !self.catalog.table_exists(&definition.table) {
                    return Err(PlanError::TableNotFound(definition.table.qualified_name()));
                }
                // 存在性检查：catalog 已存在同名触发器时的语义
                // - OR REPLACE：替换（executor 处理）
                // - IF NOT EXISTS：静默跳过（executor 处理）
                // - 都未指定：报错
                let exists = self
                    .catalog
                    .get_trigger(&definition.table, &definition.name)
                    .is_some();
                if exists && !or_replace && !if_not_exists {
                    return Err(PlanError::Unsupported(format!(
                        "trigger already exists: {} on table {}",
                        definition.name,
                        definition.table.qualified_name()
                    )));
                }
                Ok(LogicalPlan::CreateTrigger {
                    definition,
                    or_replace,
                    if_not_exists,
                })
            }
            Statement::DropTrigger {
                name,
                table,
                if_exists,
                cascade,
            } => {
                // 校验目标表存在（PG 语义：DROP TRIGGER 时表不存在报错）
                if !self.catalog.table_exists(&table) {
                    return Err(PlanError::TableNotFound(table.qualified_name()));
                }
                // 触发器存在性检查留待 executor 执行时进行：
                // - 若调用方使用的 catalog 与 planner 不同（如先 CREATE 再 DROP 的测试场景），
                //   planner 无法感知已注册的触发器；
                // - executor 在 execute_drop_trigger 中根据 if_exists 决定报错或静默跳过。
                Ok(LogicalPlan::DropTrigger {
                    name,
                    table,
                    if_exists,
                    cascade,
                })
            }
            // Phase 6.5: CREATE/DROP FUNCTION — P0-5 修复：注册函数定义到 catalog
            Statement::CreateFunction {
                name,
                parameters,
                return_type,
                language,
                body,
                or_replace,
                volatility,
                strict,
                security_definer,
            } => Ok(LogicalPlan::CreateFunction {
                name,
                parameters,
                return_type,
                language,
                body,
                or_replace,
                volatility,
                strict,
                security_definer,
            }),
            Statement::DropFunction {
                name,
                parameter_types,
                if_exists,
                cascade,
            } => Ok(LogicalPlan::DropFunction {
                name,
                parameter_types,
                if_exists,
                cascade,
            }),
            // Phase 6.10: CREATE VIEW / CREATE MATERIALIZED VIEW
            Statement::CreateView {
                name,
                columns,
                query,
                materialized,
                if_not_exists,
                or_replace,
            } => Ok(LogicalPlan::CreateView {
                name,
                columns,
                query,
                materialized,
                if_not_exists,
                or_replace,
            }),
            // Phase 6.10: DROP VIEW / DROP MATERIALIZED VIEW
            Statement::DropView {
                names,
                if_exists,
                cascade,
                materialized,
            } => Ok(LogicalPlan::DropView {
                names,
                if_exists,
                cascade,
                materialized,
            }),
            // Phase 6.10: REFRESH MATERIALIZED VIEW
            Statement::RefreshMaterializedView { name, with_data } => {
                Ok(LogicalPlan::RefreshMaterializedView { name, with_data })
            }
            // Phase F-10: ALTER TABLE — 计划层仅做表存在性校验
            // 操作语义校验（如列存在性、约束冲突、类型兼容性等）由 executor 在 catalog 上执行
            Statement::AlterTable {
                name,
                if_exists,
                only,
                operations,
            } => {
                // 校验目标表存在
                if !self.catalog.table_exists(&name) {
                    if if_exists {
                        // IF EXISTS 时表不存在则静默跳过（返回 Empty 计划）
                        return Ok(LogicalPlan::Empty);
                    }
                    return Err(PlanError::TableNotFound(name.qualified_name()));
                }
                // `only` 字段仅影响 PG 子表行为，SzRSQL 无子表继承，记录但不强制
                Ok(LogicalPlan::AlterTable {
                    name,
                    if_exists,
                    only,
                    operations,
                })
            }
            // TRUNCATE TABLE — 计划层仅做表存在性校验
            Statement::Truncate {
                names,
                if_exists,
                cascade,
            } => {
                // 校验所有目标表存在
                for name in &names {
                    if !self.catalog.table_exists(name) {
                        if if_exists {
                            continue;
                        }
                        return Err(PlanError::TableNotFound(name.qualified_name()));
                    }
                }
                Ok(LogicalPlan::Truncate {
                    names,
                    if_exists,
                    cascade,
                })
            }
            // Phase TDengine-P2: COMMENT ON 由 session 层直接操作 catalog，
            // 不经过 Planner，不产生逻辑计划。若到达此处说明调用路径异常。
            Statement::Comment { .. } => Err(PlanError::Unsupported(
                "COMMENT ON statements are handled directly in session and do not generate a logical plan".into(),
            )),
            // P2-1.1: ANALYZE 由 session 层直接扫描表数据收集统计信息，
            // 不经过 Planner，不产生逻辑计划。若到达此处说明调用路径异常。
            Statement::Analyze { .. } => Err(PlanError::Unsupported(
                "ANALYZE statements are handled directly in session and do not generate a logical plan".into(),
            )),
        }
    }

    // -----------------------------------------------------------------
    //  SELECT 计划生成
    // -----------------------------------------------------------------

    fn plan_select(&self, select: Select) -> Result<LogicalPlan, PlanError> {
        // Phase 6.1: WITH 子句（CTE）— 先处理 CTE，再 plan 主体
        // 语义（与 PG 一致）：
        // - 非 RECURSIVE：CTE 仅可见于同 WITH 内的后续 CTE 和主体（不可前向引用）
        // - RECURSIVE：所有 CTE 在自身定义时可引用自身（递归）和前序 CTE
        // - 执行：CTE 物化后挂为 LogicalPlan::With 节点，主体在 With 节点的 input 中
        let with_clause = select.with.clone();
        let has_with = with_clause.is_some();
        let cte_entries = self.plan_with_clause(&with_clause)?;

        let result = self.plan_select_inner(select, cte_entries);

        // 弹出 WITH 子句压入的作用域
        if has_with {
            self.pop_cte_scope();
        }

        result
    }

    /// 实际的 SELECT plan 生成（不含 WITH 作用域管理）— Phase 6.1
    fn plan_select_inner(
        &self,
        select: Select,
        cte_entries: Vec<CteEntry>,
    ) -> Result<LogicalPlan, PlanError> {
        // 1. FROM → Scan + JOIN 链
        let mut plan = self.plan_from(&select.from)?;

        // 2. WHERE
        if let Some(predicate) = select.where_clause {
            plan = LogicalPlan::Filter {
                predicate,
                input: Box::new(plan),
            };
        }

        // 3. 检测是否含聚合（SELECT 中有聚合函数 或 有 GROUP BY）
        let has_aggregates = !select.group_by.is_empty()
            || select
                .projection
                .iter()
                .any(|item| select_item_expr(item).is_some_and(expr_contains_aggregate));

        if has_aggregates {
            // 提取聚合表达式
            let mut aggregates = Vec::new();
            let group_exprs = select.group_by.clone();

            for item in &select.projection {
                if let Some(expr) = select_item_expr(item) {
                    extract_aggregates(expr, &mut aggregates);
                }
            }

            let having = select.having;

            plan = LogicalPlan::Aggregate {
                group_exprs,
                aggregates,
                having,
                input: Box::new(plan),
            };
        }

        // 3.5 Phase 6.2: 提取窗口函数，构造 Window 节点
        //     位置在 Aggregate 之后（窗口函数可引用 GROUP BY 列与聚合结果）、
        //     Projection 之前（让 Projection 能引用窗口函数输出列）。
        let mut window_funcs: Vec<WindowFunctionExpr> = Vec::new();
        for item in &select.projection {
            if let Some(expr) = select_item_expr(item) {
                extract_window_functions(expr, &mut window_funcs);
            }
        }
        if !window_funcs.is_empty() {
            plan = LogicalPlan::Window {
                window_funcs,
                input: Box::new(plan),
            };
        }

        // 4. 投影
        let mut output_names = Vec::with_capacity(select.projection.len());
        let mut proj_exprs = Vec::with_capacity(select.projection.len());
        for (idx, item) in select.projection.into_iter().enumerate() {
            match item {
                SelectItem::UnnamedExpr(expr) => {
                    let name = derive_output_name(&expr, idx);
                    output_names.push(name.clone());
                    proj_exprs.push((expr, Some(name)));
                }
                SelectItem::ExprWithAlias { expr, alias } => {
                    output_names.push(alias.clone());
                    proj_exprs.push((expr, Some(alias)));
                }
                SelectItem::Wildcard => {
                    // 通配符展开为所有列
                    let schema = plan_schema(&plan);
                    for col in &schema.columns {
                        output_names.push(col.name.clone());
                        proj_exprs.push((
                            Expr::Identifier(vec![col.name.clone()]),
                            Some(col.name.clone()),
                        ));
                    }
                }
                SelectItem::QualifiedWildcard(table_alias) => {
                    // 限定表通配符：只展开该表的列
                    let schema = plan_schema(&plan);
                    for col in &schema.columns {
                        output_names.push(col.name.clone());
                        proj_exprs.push((
                            Expr::Identifier(vec![table_alias.clone(), col.name.clone()]),
                            Some(col.name.clone()),
                        ));
                    }
                }
            }
        }

        if !proj_exprs.is_empty() {
            plan = LogicalPlan::Projection {
                exprs: proj_exprs,
                output_names,
                input: Box::new(plan),
            };
        }

        // 5. DISTINCT
        if select.distinct {
            plan = LogicalPlan::Distinct {
                input: Box::new(plan),
            };
        }

        // 5.5 集合操作（INTERSECT / EXCEPT / UNION）— Phase 3.27
        // 递归处理嵌套集合操作：从最内层开始包装 SetOp 节点
        if let Some(set_op) = select.set_op {
            // left 是嵌套 Select（可能含自己的 set_op），递归 plan
            let left_plan = self.plan_select(*set_op.left)?;
            let right_plan = self.plan_select(*set_op.right)?;
            plan = LogicalPlan::SetOp {
                op: set_op.op,
                quantifier: set_op.quantifier,
                left: Box::new(left_plan),
                right: Box::new(right_plan),
            };
        }

        // 6. ORDER BY
        if !select.order_by.is_empty() {
            plan = LogicalPlan::Sort {
                order_by: select.order_by,
                input: Box::new(plan),
            };
        }

        // 7. LIMIT + OFFSET
        if select.limit.is_some() || select.offset.is_some() {
            plan = LogicalPlan::Limit {
                limit: select.limit,
                offset: select.offset,
                input: Box::new(plan),
            };
        }

        // Phase 6.1: 若存在 WITH 子句，将主体包装为 With 节点
        if !cte_entries.is_empty() {
            plan = LogicalPlan::With {
                ctes: cte_entries,
                input: Box::new(plan),
            };
        }

        Ok(plan)
    }

    /// 处理 WITH 子句 — Phase 6.1
    ///
    /// 对每个 CTE：
    /// 1. 进入新作用域（压栈）
    /// 2. 若 `recursive=true` 且 CTE body 是 `anchor UNION [ALL] recursive_part` 形式，
    ///    且 recursive_part 引用了 CTE 自身，则构造 `CteEntry::Recursive`
    /// 3. 否则构造 `CteEntry::Simple`（非递归）
    /// 4. 推导 CTE 输出 Schema（应用显式列别名重命名）
    /// 5. 注册到作用域供后续 CTE 和主体引用
    fn plan_with_clause(&self, with: &Option<WithClause>) -> Result<Vec<CteEntry>, PlanError> {
        let Some(with_clause) = with else {
            return Ok(Vec::new());
        };

        // 进入新作用域
        self.cte_scopes.borrow_mut().push(HashMap::new());
        let scope_depth = self.cte_scopes.borrow().len();

        let mut entries = Vec::with_capacity(with_clause.ctes.len());
        for cte in &with_clause.ctes {
            let name_lower = cte.name.to_lowercase();

            // 检测是否为递归 CTE：body 是 set_op 且右侧引用 CTE 自身
            // 且 with_clause.recursive = true
            let recursive_info = if with_clause.recursive {
                detect_recursive_cte(&cte.query, &name_lower)
            } else {
                None
            };

            let entry = if let Some((all, anchor_select, recursive_select)) = recursive_info {
                // 递归 CTE
                // 先 plan anchor（不引用自身，使用当前作用域）
                let anchor_plan = self.plan_select((*anchor_select).clone())?;
                let anchor_schema = plan_schema(&anchor_plan);

                // 验证 anchor schema 列数与显式列别名一致
                if !cte.columns.is_empty() && cte.columns.len() != anchor_schema.columns.len() {
                    return Err(PlanError::Unsupported(format!(
                        "recursive CTE '{}' has {} column aliases but query produces {} columns",
                        cte.name,
                        cte.columns.len(),
                        anchor_schema.columns.len()
                    )));
                }

                let cte_schema = apply_column_aliases(&anchor_schema, &cte.columns, &cte.name);

                // 注册 CTE 自身引用（使用 anchor_schema 作为引用 schema）
                // 注意：recursive part 中引用 CTE 时，看到的是 anchor 的列结构
                self.cte_scopes.borrow_mut().last_mut().unwrap().insert(
                    name_lower.clone(),
                    (anchor_plan.clone(), cte_schema.clone()),
                );

                // plan recursive part（可引用 CTE 自身）
                let recursive_plan = self.plan_select((*recursive_select).clone())?;

                CteEntry::Recursive {
                    name: name_lower,
                    anchor: Box::new(anchor_plan),
                    recursive: Box::new(recursive_plan),
                    all,
                    schema: cte_schema,
                }
            } else {
                // 非递归 CTE
                let cte_plan = self.plan_select((*cte.query).clone())?;
                let plan_output_schema = plan_schema(&cte_plan);

                // 验证列别名数与查询列数一致
                if !cte.columns.is_empty() && cte.columns.len() != plan_output_schema.columns.len()
                {
                    return Err(PlanError::Unsupported(format!(
                        "CTE '{}' has {} column aliases but query produces {} columns",
                        cte.name,
                        cte.columns.len(),
                        plan_output_schema.columns.len()
                    )));
                }

                let cte_schema = apply_column_aliases(&plan_output_schema, &cte.columns, &cte.name);

                // 注册到当前作用域
                self.cte_scopes
                    .borrow_mut()
                    .last_mut()
                    .unwrap()
                    .insert(name_lower.clone(), (cte_plan.clone(), cte_schema.clone()));

                CteEntry::Simple {
                    name: name_lower,
                    plan: Box::new(cte_plan),
                    schema: cte_schema,
                }
            };

            entries.push(entry);
        }

        // 作用域在 plan_select 退出时由 wrap_with_scope 弹出；此处仅确保深度对齐
        debug_assert_eq!(self.cte_scopes.borrow().len(), scope_depth);

        Ok(entries)
    }

    /// 弹出当前 CTE 作用域（由 plan_select 的调用方在结束时调用）— Phase 6.1
    fn pop_cte_scope(&self) {
        self.cte_scopes.borrow_mut().pop();
    }

    /// 在 CTE 作用域栈中查找名称 — Phase 6.1
    fn lookup_cte(&self, name: &str) -> Option<(LogicalPlan, TableSchema)> {
        let key = name.to_lowercase();
        let scopes = self.cte_scopes.borrow();
        // 从栈顶向下查找（内层作用域优先）
        for scope in scopes.iter().rev() {
            if let Some(entry) = scope.get(&key) {
                return Some(entry.clone());
            }
        }
        None
    }

    /// FROM 子句 → Scan + JOIN 链
    fn plan_from(&self, from: &[TableWithJoins]) -> Result<LogicalPlan, PlanError> {
        if from.is_empty() {
            // SELECT without FROM → 虚拟单行表（类似 PG dual）
            return Ok(LogicalPlan::Dual);
        }

        let mut plan = self.plan_table_factor(&from[0].relation)?;

        // 第一个表的 JOINs
        for join in from[0].joins.iter() {
            plan = self.apply_join(plan, join)?;
        }

        // 多表 FROM（逗号分隔）：SELECT * FROM t1, t2 等价于 CROSS JOIN
        for twj in from.iter().skip(1) {
            let right = self.plan_table_factor(&twj.relation)?;
            plan = LogicalPlan::Join {
                join_type: JoinType::Cross,
                condition: JoinCondition::None,
                left: Box::new(plan),
                right: Box::new(right),
            };
            for join in twj.joins.iter() {
                plan = self.apply_join(plan, join)?;
            }
        }

        Ok(plan)
    }

    fn plan_table_factor(&self, tf: &TableFactor) -> Result<LogicalPlan, PlanError> {
        match tf {
            TableFactor::Table { name, alias } => {
                // Phase 6.1: 优先检查 CTE 作用域
                if let Some((_cte_plan, cte_schema)) = self.lookup_cte(&name.name) {
                    return Ok(LogicalPlan::CteRef {
                        name: name.name.to_lowercase(),
                        schema: cte_schema,
                    });
                }
                // Phase 6.15: 检查是否为视图（物化视图路由 / 普通视图展开）
                if let Some(view_def) = self.catalog.get_view(name) {
                    if view_def.materialized {
                        // 物化视图：路由到存储表（执行器按名查找 MV 存储引用）
                        // Schema 由视图查询输出推导
                        let inner_plan = self.plan_select((*view_def.query).clone())?;
                        let mv_schema = plan_schema(&inner_plan);
                        return Ok(LogicalPlan::MaterializedViewScan {
                            name: name.clone(),
                            alias: alias.as_ref().map(|a| a.name.clone()),
                            schema: mv_schema,
                        });
                    } else {
                        // 普通视图：展开为子查询（与 PG 行为一致）
                        let inner_plan = self.plan_select((*view_def.query).clone())?;
                        let view_schema = plan_schema(&inner_plan);
                        return Ok(LogicalPlan::Projection {
                            exprs: view_schema
                                .columns
                                .iter()
                                .map(|c| {
                                    (Expr::Identifier(vec![c.name.clone()]), Some(c.name.clone()))
                                })
                                .collect(),
                            output_names: view_schema
                                .columns
                                .iter()
                                .map(|c| c.name.clone())
                                .collect(),
                            input: Box::new(inner_plan),
                        });
                    }
                }
                let schema = self
                    .catalog
                    .get_table(name)
                    .ok_or_else(|| PlanError::TableNotFound(name.qualified_name()))?;
                Ok(LogicalPlan::Scan {
                    table: name.clone(),
                    alias: alias.as_ref().map(|a| a.name.clone()),
                    schema,
                })
            }
            TableFactor::Derived { subquery, alias } => {
                // 子查询作为派生表
                let inner = self.plan_select(subquery.as_ref().clone())?;
                let derived_schema = plan_schema(&inner);
                let _ = alias; // 别名暂未在 Scan 中保存（执行器阶段补全）
                Ok(LogicalPlan::Projection {
                    exprs: derived_schema
                        .columns
                        .iter()
                        .map(|c| (Expr::Identifier(vec![c.name.clone()]), Some(c.name.clone())))
                        .collect(),
                    output_names: derived_schema
                        .columns
                        .iter()
                        .map(|c| c.name.clone())
                        .collect(),
                    input: Box::new(inner),
                })
            }
            TableFactor::TableFunction { .. } => Err(PlanError::Unsupported(
                "table function not yet supported in planner".into(),
            )),
        }
    }

    fn apply_join(&self, left: LogicalPlan, join: &Join) -> Result<LogicalPlan, PlanError> {
        let right = self.plan_table_factor(&join.relation)?;
        Ok(LogicalPlan::Join {
            join_type: join.join_type,
            condition: join.condition.clone(),
            left: Box::new(left),
            right: Box::new(right),
        })
    }

    // -----------------------------------------------------------------
    //  INSERT 计划生成
    // -----------------------------------------------------------------

    fn plan_insert(
        &self,
        table: TableName,
        columns: Option<Vec<String>>,
        source: InsertSource,
        on_conflict: Option<OnConflict>,
        returning: Option<Vec<SelectItem>>,
    ) -> Result<LogicalPlan, PlanError> {
        let schema = self
            .catalog
            .get_table(&table)
            .ok_or_else(|| PlanError::TableNotFound(table.qualified_name()))?;

        // 列名校验
        if let Some(cols) = &columns {
            for c in cols {
                if schema.find_column(c).is_none() {
                    return Err(PlanError::ColumnNotFound(c.clone()));
                }
            }
        }

        // Phase 6.18: 生成列校验 — 不允许显式插入生成列
        let has_generated = schema.columns.iter().any(|c| c.generated.is_some());
        if has_generated {
            if let Some(cols) = &columns {
                // 显式列模式：检查是否有生成列
                for c in cols {
                    if let Some(col) = schema.find_column(c) {
                        if col.generated.is_some() {
                            return Err(PlanError::Unsupported(format!(
                                "cannot insert into generated column \"{c}\""
                            )));
                        }
                    }
                }
            } else {
                // 无显式列模式：若表含生成列，不允许 INSERT VALUES/SELECT（DEFAULT VALUES 除外）
                if !matches!(source, InsertSource::DefaultValues) {
                    return Err(PlanError::Unsupported(
                        "table has generated columns; must specify explicit column list excluding generated columns"
                            .into(),
                    ));
                }
            }
        }

        // ON CONFLICT 冲突列校验
        let conflict_cols: Option<&Vec<String>> = match &on_conflict {
            Some(OnConflict::DoNothing { conflict_columns }) => conflict_columns.as_ref(),
            Some(OnConflict::DoUpdate {
                conflict_columns, ..
            }) => conflict_columns.as_ref(),
            None => None,
        };
        if let Some(cols) = conflict_cols {
            for c in cols {
                if schema.find_column(c).is_none() {
                    return Err(PlanError::ColumnNotFound(c.clone()));
                }
            }
        }

        let source_plan = match source {
            InsertSource::Values(rows) => {
                // 列数校验
                let expected = columns
                    .as_ref()
                    .map(|c| c.len())
                    .unwrap_or(schema.columns.len());
                for (idx, row) in rows.iter().enumerate() {
                    if row.len() != expected {
                        return Err(PlanError::ColumnCountMismatch {
                            expected,
                            actual: row.len(),
                        }
                        .into_error_with_row(idx));
                    }
                }
                InsertSourcePlan::Values(rows)
            }
            InsertSource::Select(select) => {
                let inner = self.plan_select(*select)?;
                InsertSourcePlan::Select(Box::new(inner))
            }
            InsertSource::DefaultValues => InsertSourcePlan::DefaultValues,
        };

        Ok(LogicalPlan::Insert {
            table,
            schema,
            columns,
            source: source_plan,
            on_conflict,
            returning,
        })
    }

    // -----------------------------------------------------------------
    //  REPLACE 计划生成 — Phase 3.25
    // -----------------------------------------------------------------

    fn plan_replace(
        &self,
        table: TableName,
        columns: Option<Vec<String>>,
        source: InsertSource,
    ) -> Result<LogicalPlan, PlanError> {
        let schema = self
            .catalog
            .get_table(&table)
            .ok_or_else(|| PlanError::TableNotFound(table.qualified_name()))?;

        // 列名校验
        if let Some(cols) = &columns {
            for c in cols {
                if schema.find_column(c).is_none() {
                    return Err(PlanError::ColumnNotFound(c.clone()));
                }
            }
        }

        let source_plan = match source {
            InsertSource::Values(rows) => {
                let expected = columns
                    .as_ref()
                    .map(|c| c.len())
                    .unwrap_or(schema.columns.len());
                for (idx, row) in rows.iter().enumerate() {
                    if row.len() != expected {
                        return Err(PlanError::ColumnCountMismatch {
                            expected,
                            actual: row.len(),
                        }
                        .into_error_with_row(idx));
                    }
                }
                InsertSourcePlan::Values(rows)
            }
            InsertSource::Select(select) => {
                let inner = self.plan_select(*select)?;
                InsertSourcePlan::Select(Box::new(inner))
            }
            InsertSource::DefaultValues => InsertSourcePlan::DefaultValues,
        };

        Ok(LogicalPlan::Replace {
            table,
            schema,
            columns,
            source: source_plan,
        })
    }

    // -----------------------------------------------------------------
    //  UPDATE 计划生成
    // -----------------------------------------------------------------

    fn plan_update(
        &self,
        table: TableName,
        alias: Option<String>,
        assignments: Vec<Assignment>,
        where_clause: Option<Expr>,
        returning: Option<Vec<SelectItem>>,
    ) -> Result<LogicalPlan, PlanError> {
        let schema = self
            .catalog
            .get_table(&table)
            .ok_or_else(|| PlanError::TableNotFound(table.qualified_name()))?;

        // 赋值列名校验
        for a in &assignments {
            if schema.find_column(&a.column).is_none() {
                return Err(PlanError::ColumnNotFound(a.column.clone()));
            }
            // Phase 6.18: 不允许 UPDATE 生成列
            if let Some(col) = schema.find_column(&a.column) {
                if col.generated.is_some() {
                    return Err(PlanError::Unsupported(format!(
                        "cannot update generated column \"{}\"",
                        a.column
                    )));
                }
            }
        }

        // WHERE 子计划
        let source = where_clause.map(|predicate| {
            Box::new(LogicalPlan::Filter {
                predicate,
                input: Box::new(LogicalPlan::Scan {
                    table: table.clone(),
                    alias: alias.clone(),
                    schema: schema.clone(),
                }),
            })
        });

        Ok(LogicalPlan::Update {
            table,
            schema,
            assignments,
            source,
            returning,
        })
    }

    // -----------------------------------------------------------------
    //  DELETE 计划生成
    // -----------------------------------------------------------------

    fn plan_delete(
        &self,
        table: TableName,
        alias: Option<String>,
        where_clause: Option<Expr>,
        returning: Option<Vec<SelectItem>>,
    ) -> Result<LogicalPlan, PlanError> {
        let schema = self
            .catalog
            .get_table(&table)
            .ok_or_else(|| PlanError::TableNotFound(table.qualified_name()))?;

        let source = where_clause.map(|predicate| {
            Box::new(LogicalPlan::Filter {
                predicate,
                input: Box::new(LogicalPlan::Scan {
                    table: table.clone(),
                    alias: alias.clone(),
                    schema: schema.clone(),
                }),
            })
        });

        Ok(LogicalPlan::Delete {
            table,
            schema,
            source,
            returning,
        })
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 取出 SelectItem 中的表达式引用
fn select_item_expr(item: &SelectItem) -> Option<&Expr> {
    match item {
        SelectItem::UnnamedExpr(expr) => Some(expr),
        SelectItem::ExprWithAlias { expr, .. } => Some(expr),
        SelectItem::Wildcard | SelectItem::QualifiedWildcard(_) => None,
    }
}

/// 判断表达式是否含聚合函数
fn expr_contains_aggregate(expr: &Expr) -> bool {
    match expr {
        Expr::Function { name, .. } => is_aggregate_function(name),
        Expr::BinaryOp { left, right, .. } => {
            expr_contains_aggregate(left) || expr_contains_aggregate(right)
        }
        Expr::UnaryOp { expr, .. } => expr_contains_aggregate(expr),
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            operand.as_ref().is_some_and(|e| expr_contains_aggregate(e))
                || when_then
                    .iter()
                    .any(|(w, t)| expr_contains_aggregate(w) || expr_contains_aggregate(t))
                || else_expr
                    .as_ref()
                    .is_some_and(|e| expr_contains_aggregate(e))
        }
        Expr::Cast { expr, .. } => expr_contains_aggregate(expr),
        Expr::InList { expr, list, .. } => {
            expr_contains_aggregate(expr) || list.iter().any(expr_contains_aggregate)
        }
        Expr::InSubquery { expr, .. } => expr_contains_aggregate(expr),
        Expr::Between {
            expr, low, high, ..
        } => {
            expr_contains_aggregate(expr)
                || expr_contains_aggregate(low)
                || expr_contains_aggregate(high)
        }
        Expr::Like { expr, pattern, .. } => {
            expr_contains_aggregate(expr) || expr_contains_aggregate(pattern)
        }
        Expr::IsNull { expr, .. } => expr_contains_aggregate(expr),
        Expr::Exists { .. } | Expr::Subquery(_) => false,
        Expr::Tuple(exprs) => exprs.iter().any(expr_contains_aggregate),
        Expr::Literal(_) | Expr::Identifier(_) | Expr::Wildcard | Expr::Parameter(_) => false,
        // Phase 3.32: 数组字面量与 ANY/ALL 子表达式可能含聚合
        Expr::Array(exprs) => exprs.iter().any(expr_contains_aggregate),
        Expr::AnyOp { left, right, .. } | Expr::AllOp { left, right, .. } => {
            expr_contains_aggregate(left) || expr_contains_aggregate(right)
        }
        // Phase 6.2: 窗口函数不视为聚合（由 Window 节点单独处理）
        Expr::WindowFunction { .. } => false,
        // Phase F-9: PG 兼容表达式 — 递归判断子表达式
        Expr::IsDistinctFrom { left, right, .. } => {
            expr_contains_aggregate(left) || expr_contains_aggregate(right)
        }
        Expr::SimilarTo { expr, pattern, .. } => {
            expr_contains_aggregate(expr) || expr_contains_aggregate(pattern)
        }
        Expr::Substring {
            expr, from, for_len, ..
        } => {
            expr_contains_aggregate(expr)
                || from.as_ref().is_some_and(|e| expr_contains_aggregate(e))
                || for_len.as_ref().is_some_and(|e| expr_contains_aggregate(e))
        }
    }
}

/// 判断函数名是否为聚合函数
fn is_aggregate_function(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "count" | "sum" | "avg" | "min" | "max" | "array_agg" | "string_agg"
    )
}

/// 从表达式中递归提取聚合函数
fn extract_aggregates(expr: &Expr, out: &mut Vec<AggregateExpr>) {
    match expr {
        Expr::Function {
            name,
            args,
            distinct,
        } => {
            if is_aggregate_function(name) {
                out.push(AggregateExpr {
                    func_name: name.clone(),
                    distinct: *distinct,
                    args: args.clone(),
                    alias: None,
                });
            } else {
                for a in args {
                    extract_aggregates(a, out);
                }
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            extract_aggregates(left, out);
            extract_aggregates(right, out);
        }
        Expr::UnaryOp { expr, .. } => extract_aggregates(expr, out),
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            if let Some(op) = operand {
                extract_aggregates(op, out);
            }
            for (w, t) in when_then {
                extract_aggregates(w, out);
                extract_aggregates(t, out);
            }
            if let Some(e) = else_expr {
                extract_aggregates(e, out);
            }
        }
        Expr::Cast { expr, .. } => extract_aggregates(expr, out),
        Expr::InList { expr, list, .. } => {
            extract_aggregates(expr, out);
            for l in list {
                extract_aggregates(l, out);
            }
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            extract_aggregates(expr, out);
            extract_aggregates(low, out);
            extract_aggregates(high, out);
        }
        Expr::Like { expr, pattern, .. } => {
            extract_aggregates(expr, out);
            extract_aggregates(pattern, out);
        }
        Expr::IsNull { expr, .. } => extract_aggregates(expr, out),
        Expr::Tuple(exprs) => {
            for e in exprs {
                extract_aggregates(e, out);
            }
        }
        Expr::InSubquery { .. } | Expr::Exists { .. } | Expr::Subquery(_) => {}
        Expr::Literal(_) | Expr::Identifier(_) | Expr::Wildcard | Expr::Parameter(_) => {}
        // Phase 3.32: 数组字面量与 ANY/ALL 内部子表达式可包含聚合
        Expr::Array(exprs) => {
            for e in exprs {
                extract_aggregates(e, out);
            }
        }
        Expr::AnyOp { left, right, .. } | Expr::AllOp { left, right, .. } => {
            extract_aggregates(left, out);
            extract_aggregates(right, out);
        }
        // Phase 6.2: 窗口函数由 Window 节点单独处理，不视为聚合
        Expr::WindowFunction { .. } => {}
        // Phase F-9: PG 兼容表达式 — 递归提取子表达式中的聚合
        Expr::IsDistinctFrom { left, right, .. } => {
            extract_aggregates(left, out);
            extract_aggregates(right, out);
        }
        Expr::SimilarTo { expr, pattern, .. } => {
            extract_aggregates(expr, out);
            extract_aggregates(pattern, out);
        }
        Expr::Substring {
            expr, from, for_len, ..
        } => {
            extract_aggregates(expr, out);
            if let Some(e) = from {
                extract_aggregates(e, out);
            }
            if let Some(e) = for_len {
                extract_aggregates(e, out);
            }
        }
    }
}

/// 从表达式中递归提取窗口函数 — Phase 6.2
///
/// 与 `extract_aggregates` 类似，但提取的是 `Expr::WindowFunction` 节点。
/// 用于在 `plan_select_inner` 中构造 `LogicalPlan::Window` 节点。
fn extract_window_functions(expr: &Expr, out: &mut Vec<WindowFunctionExpr>) {
    match expr {
        Expr::WindowFunction {
            name,
            args,
            distinct,
            window,
        } => {
            out.push(WindowFunctionExpr {
                func_name: name.clone(),
                distinct: *distinct,
                args: args.clone(),
                window: window.clone(),
                alias: None,
            });
            // 注：窗口函数参数本身不再递归提取嵌套窗口函数
            // （PG 不支持嵌套窗口函数）
        }
        Expr::BinaryOp { left, right, .. } => {
            extract_window_functions(left, out);
            extract_window_functions(right, out);
        }
        Expr::UnaryOp { expr, .. } => extract_window_functions(expr, out),
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            if let Some(op) = operand {
                extract_window_functions(op, out);
            }
            for (w, t) in when_then {
                extract_window_functions(w, out);
                extract_window_functions(t, out);
            }
            if let Some(e) = else_expr {
                extract_window_functions(e, out);
            }
        }
        Expr::Cast { expr, .. } => extract_window_functions(expr, out),
        Expr::InList { expr, list, .. } => {
            extract_window_functions(expr, out);
            for l in list {
                extract_window_functions(l, out);
            }
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            extract_window_functions(expr, out);
            extract_window_functions(low, out);
            extract_window_functions(high, out);
        }
        Expr::Like { expr, pattern, .. } => {
            extract_window_functions(expr, out);
            extract_window_functions(pattern, out);
        }
        Expr::IsNull { expr, .. } => extract_window_functions(expr, out),
        Expr::Tuple(exprs) => {
            for e in exprs {
                extract_window_functions(e, out);
            }
        }
        Expr::Array(exprs) => {
            for e in exprs {
                extract_window_functions(e, out);
            }
        }
        Expr::AnyOp { left, right, .. } | Expr::AllOp { left, right, .. } => {
            extract_window_functions(left, out);
            extract_window_functions(right, out);
        }
        // 不递归的叶子或非窗口节点
        Expr::Function { args, .. } => {
            for a in args {
                extract_window_functions(a, out);
            }
        }
        Expr::InSubquery { .. } | Expr::Exists { .. } | Expr::Subquery(_) => {}
        Expr::Literal(_) | Expr::Identifier(_) | Expr::Wildcard | Expr::Parameter(_) => {}
        // Phase F-9: PG 兼容表达式 — 递归提取子表达式中的窗口函数
        Expr::IsDistinctFrom { left, right, .. } => {
            extract_window_functions(left, out);
            extract_window_functions(right, out);
        }
        Expr::SimilarTo { expr, pattern, .. } => {
            extract_window_functions(expr, out);
            extract_window_functions(pattern, out);
        }
        Expr::Substring {
            expr, from, for_len, ..
        } => {
            extract_window_functions(expr, out);
            if let Some(e) = from {
                extract_window_functions(e, out);
            }
            if let Some(e) = for_len {
                extract_window_functions(e, out);
            }
        }
    }
}

/// 从表达式推导输出列名
fn derive_output_name(expr: &Expr, idx: usize) -> String {
    match expr {
        Expr::Identifier(parts) => parts.last().cloned().unwrap_or_else(|| format!("col{idx}")),
        Expr::Function { name, .. } => name.clone(),
        _ => format!("col{idx}"),
    }
}

/// 从投影表达式推导列类型 — P0-VIEW 修复
///
/// 推导规则：
/// - `Expr::Literal(v)` → `v.column_type()`
/// - `Expr::Identifier(names)` → 在 input_schema 中按列名查找（大小写不敏感）
/// - `Expr::Cast { data_type, .. }` → `data_type.clone()`
/// - 其他表达式 → `ColumnType::Null`（兜底，由协议层进一步推导）
fn derive_expr_column_type(expr: &Expr, input_schema: &TableSchema) -> ColumnType {
    match expr {
        Expr::Literal(value) => value.column_type(),
        Expr::Identifier(names) => {
            let col_name = names.last().map(|s| s.to_lowercase()).unwrap_or_default();
            input_schema
                .columns
                .iter()
                .find(|c| c.name.to_lowercase() == col_name)
                .map(|c| c.data_type.clone())
                .unwrap_or(ColumnType::Null)
        }
        Expr::Cast { data_type, .. } => data_type.clone(),
        _ => ColumnType::Null,
    }
}

/// 从 LogicalPlan 推导 Schema（用于通配符展开和派生表）
pub fn plan_schema(plan: &LogicalPlan) -> TableSchema {
    match plan {
        LogicalPlan::Scan { schema, .. }
        | LogicalPlan::IndexScan { schema, .. }
        | LogicalPlan::MaterializedViewScan { schema, .. } => schema.clone(),
        LogicalPlan::Projection {
            exprs,
            output_names,
            input,
            ..
        } => {
            // P0-VIEW 修复：从投影表达式和 input schema 推导列类型（类型保真），
            // 不再将所有列设为 Null。这确保视图展开时 view_schema 携带正确类型。
            let inner = plan_schema(input);
            let columns = output_names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let ct = exprs
                        .get(i)
                        .map(|(e, _)| derive_expr_column_type(e, &inner))
                        .unwrap_or(ColumnType::Null);
                    ColumnDefinition::new(name.clone(), ct)
                })
                .collect();
            TableSchema {
                name: TableName::new("__derived__"),
                columns,
            }
        }
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Distinct { input, .. } => plan_schema(input),
        LogicalPlan::Join { left, right, .. } => {
            let mut l = plan_schema(left);
            let r = plan_schema(right);
            l.columns.extend(r.columns);
            l
        }
        LogicalPlan::Aggregate {
            aggregates,
            group_exprs,
            ..
        } => {
            let mut cols = Vec::new();
            for g in group_exprs {
                if let Expr::Identifier(parts) = g {
                    cols.push(ColumnDefinition::new(
                        parts.last().cloned().unwrap_or_default(),
                        ColumnType::Null,
                    ));
                }
            }
            for a in aggregates {
                let name = a.alias.clone().unwrap_or_else(|| a.func_name.clone());
                cols.push(ColumnDefinition::new(name, ColumnType::Null));
            }
            TableSchema {
                name: TableName::new("__aggregate__"),
                columns: cols,
            }
        }
        // Phase 6.2: Window 节点输出 = input schema 列 ++ 每个窗口函数一列
        LogicalPlan::Window {
            window_funcs,
            input,
        } => {
            let mut inner = plan_schema(input);
            for w in window_funcs {
                let name = w.alias.clone().unwrap_or_else(|| w.func_name.clone());
                inner
                    .columns
                    .push(ColumnDefinition::new(name, ColumnType::Null));
            }
            inner
        }
        LogicalPlan::Insert { schema, .. }
        | LogicalPlan::Update { schema, .. }
        | LogicalPlan::Delete { schema, .. } => schema.clone(),
        LogicalPlan::CreateTable { name, columns, .. } => TableSchema {
            name: name.clone(),
            columns: columns.clone(),
        },
        LogicalPlan::DropTable { .. }
        | LogicalPlan::CreateIndex { .. }
        | LogicalPlan::DropIndex { .. }
        | LogicalPlan::CreateSequence { .. }
        | LogicalPlan::DropSequence { .. }
        | LogicalPlan::CreateType { .. }
        | LogicalPlan::DropType { .. }
        | LogicalPlan::AlterType { .. }
        | LogicalPlan::Empty
        | LogicalPlan::Dual
        | LogicalPlan::Merge { .. }
        | LogicalPlan::Replace { .. }
        | LogicalPlan::Prepare { .. }
        | LogicalPlan::Execute { .. }
        | LogicalPlan::Deallocate { .. }
        | LogicalPlan::SetNames { .. }
        | LogicalPlan::SetVariable { .. }
        // Phase 6.4: CREATE/DROP TRIGGER 为 DDL，不返回结果集
        | LogicalPlan::CreateTrigger { .. }
        | LogicalPlan::DropTrigger { .. }
        // Phase 6.10: CREATE/DROP VIEW / REFRESH MATERIALIZED VIEW 为 DDL，不返回结果集
        | LogicalPlan::CreateView { .. }
        | LogicalPlan::DropView { .. }
        | LogicalPlan::RefreshMaterializedView { .. }
        // Phase 6.5: CREATE/DROP FUNCTION 为 DDL，不返回结果集
        | LogicalPlan::CreateFunction { .. }
        | LogicalPlan::DropFunction { .. } => TableSchema {
            name: TableName::new("__empty__"),
            columns: Vec::new(),
        },
        // Phase 3.34: SHOW TABLES → 单列 `Tables_in_<db>`
        LogicalPlan::ShowTables => TableSchema {
            name: TableName::new("__show_tables__"),
            columns: vec![ColumnDefinition::new("Tables_in_szrsql", ColumnType::Text)],
        },
        // Phase 3.34: SHOW CREATE TABLE → 两列 `Table` + `Create Table`
        LogicalPlan::ShowCreateTable { name: _ } => TableSchema {
            name: TableName::new("__show_create_table__"),
            columns: vec![
                ColumnDefinition::new("Table", ColumnType::Text),
                ColumnDefinition::new("Create Table", ColumnType::Text),
            ],
        },
        // Phase 3.34: SHOW variable → 单列 `setting`
        LogicalPlan::ShowVariable { .. } => TableSchema {
            name: TableName::new("__show_variable__"),
            columns: vec![ColumnDefinition::new("setting", ColumnType::Text)],
        },
        LogicalPlan::SetOp { left, .. } => plan_schema(left),
        // Phase 5.8: Shared/MemoRef
        LogicalPlan::Shared { plan, .. } => plan_schema(plan),
        LogicalPlan::MemoRef { schema, .. } => schema.clone(),
        // Phase 3.35: FLASHBACK 命令无常规 Schema
        // - FlashbackTransaction 返回 Vec<(表名, TableSnapshot)>（非 Vec<Row>），无 Schema
        // - FlashbackTable 返回 Vec<Row>，但其 Schema 取决于历史快照内容，
        //   通配符展开等场景不会进入 FLASHBACK 计划，此处返回空 Schema 即可。
        LogicalPlan::FlashbackTransaction { .. } | LogicalPlan::FlashbackTable { .. } => {
            TableSchema {
                name: TableName::new("__flashback__"),
                columns: Vec::new(),
            }
        }
        // Phase 4.6: LISTEN/UNLISTEN/NOTIFY 不返回结果集，返回空 Schema
        LogicalPlan::Listen { .. } | LogicalPlan::Unlisten { .. } | LogicalPlan::Notify { .. } => {
            TableSchema {
                name: TableName::new("__notify__"),
                columns: Vec::new(),
            }
        }
        // Phase 4.8: COPY FROM/TO 不返回常规结果集（COPY TO 写文件，COPY FROM 返回 AffectedRows）
        LogicalPlan::Copy { .. } => TableSchema {
            name: TableName::new("__copy__"),
            columns: Vec::new(),
        },
        // Phase 6.1: WITH 节点的 schema 等于主体的 schema
        LogicalPlan::With { input, .. } => plan_schema(input),
        // Phase 6.1: CteRef 节点的 schema 已在 CTE 注册时确定
        LogicalPlan::CteRef { schema, .. } => schema.clone(),
        // Phase F-10: ALTER TABLE 不返回结果集（DDL，返回空 Schema）
        LogicalPlan::AlterTable { .. } => TableSchema {
            name: TableName::new("__alter_table__"),
            columns: Vec::new(),
        },
        // TRUNCATE TABLE 不返回结果集（DDL，返回空 Schema）
        LogicalPlan::Truncate { .. } => TableSchema {
            name: TableName::new("__truncate__"),
            columns: Vec::new(),
        },
    }
}

// =====================================================================
//  Phase 6.15: 计划格式化（EXPLAIN 输出）
// =====================================================================

/// 格式化逻辑计划为可读文本树（用于 EXPLAIN 输出）— Phase 6.15
///
/// 输出格式为缩进树，每个节点一行，格式为 `节点类型: 详情`。
/// 物化视图扫描节点显示 `MaterializedViewScan: <name>`，
/// 普通表扫描显示 `SeqScan: <name>`，索引扫描显示 `IndexScan: <name> on <index>`。
///
/// # 示例
///
/// ```ignore
/// let text = format_plan(&plan);
/// // 输出：
/// // MaterializedViewScan: mv
/// ```
pub fn format_plan(plan: &LogicalPlan) -> String {
    let mut buf = String::new();
    format_plan_impl(plan, 0, &mut buf);
    // 去掉末尾换行
    if buf.ends_with('\n') {
        buf.pop();
    }
    buf
}

fn format_plan_impl(plan: &LogicalPlan, indent: usize, buf: &mut String) {
    let pad = "  ".repeat(indent);
    match plan {
        LogicalPlan::Scan { table, .. } => {
            buf.push_str(&format!("{pad}SeqScan: {}\n", table.name));
        }
        LogicalPlan::MaterializedViewScan { name, .. } => {
            buf.push_str(&format!("{pad}MaterializedViewScan: {}\n", name.name));
        }
        LogicalPlan::IndexScan {
            table, index_name, ..
        } => {
            buf.push_str(&format!(
                "{pad}IndexScan: {} on {}\n",
                table.name, index_name
            ));
        }
        LogicalPlan::Projection { input, .. } => {
            buf.push_str(&format!("{pad}Projection\n"));
            format_plan_impl(input, indent + 1, buf);
        }
        LogicalPlan::Filter { predicate, input } => {
            buf.push_str(&format!("{pad}Filter: {predicate:?}\n"));
            format_plan_impl(input, indent + 1, buf);
        }
        LogicalPlan::Join {
            join_type,
            left,
            right,
            ..
        } => {
            buf.push_str(&format!("{pad}Join: {join_type:?}\n"));
            format_plan_impl(left, indent + 1, buf);
            format_plan_impl(right, indent + 1, buf);
        }
        LogicalPlan::Aggregate {
            group_exprs,
            aggregates,
            input,
            ..
        } => {
            buf.push_str(&format!(
                "{pad}Aggregate: groups={} aggs={}\n",
                group_exprs.len(),
                aggregates.len()
            ));
            format_plan_impl(input, indent + 1, buf);
        }
        LogicalPlan::Window {
            window_funcs,
            input,
        } => {
            buf.push_str(&format!("{pad}Window: {} funcs\n", window_funcs.len()));
            format_plan_impl(input, indent + 1, buf);
        }
        LogicalPlan::Sort { order_by, input } => {
            buf.push_str(&format!("{pad}Sort: {} keys\n", order_by.len()));
            format_plan_impl(input, indent + 1, buf);
        }
        LogicalPlan::Limit { input, .. } => {
            buf.push_str(&format!("{pad}Limit\n"));
            format_plan_impl(input, indent + 1, buf);
        }
        LogicalPlan::Distinct { input } => {
            buf.push_str(&format!("{pad}Distinct\n"));
            format_plan_impl(input, indent + 1, buf);
        }
        LogicalPlan::SetOp {
            op,
            quantifier,
            left,
            right,
        } => {
            buf.push_str(&format!("{pad}SetOp: {op:?} {quantifier:?}\n"));
            format_plan_impl(left, indent + 1, buf);
            format_plan_impl(right, indent + 1, buf);
        }
        LogicalPlan::Empty => {
            buf.push_str(&format!("{pad}Empty\n"));
        }
        LogicalPlan::Dual => {
            buf.push_str(&format!("{pad}Dual\n"));
        }
        LogicalPlan::Shared { id, plan } => {
            buf.push_str(&format!("{pad}Shared: id={id}\n"));
            format_plan_impl(plan, indent + 1, buf);
        }
        LogicalPlan::MemoRef { id, .. } => {
            buf.push_str(&format!("{pad}MemoRef: id={id}\n"));
        }
        LogicalPlan::With { ctes, input } => {
            buf.push_str(&format!("{pad}With: {} CTEs\n", ctes.len()));
            format_plan_impl(input, indent + 1, buf);
        }
        LogicalPlan::CteRef { name, .. } => {
            buf.push_str(&format!("{pad}CteRef: {name}\n"));
        }
        LogicalPlan::CreateView {
            name, materialized, ..
        } => {
            buf.push_str(&format!(
                "{pad}CreateView: {} (materialized={materialized})\n",
                name.name
            ));
        }
        LogicalPlan::DropView {
            names,
            materialized,
            ..
        } => {
            buf.push_str(&format!(
                "{pad}DropView: {} (materialized={materialized})\n",
                names
                    .iter()
                    .map(|n| n.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        LogicalPlan::RefreshMaterializedView { name, .. } => {
            buf.push_str(&format!("{pad}RefreshMaterializedView: {}\n", name.name));
        }
        _ => {
            buf.push_str(&format!("{pad}{:?}\n", std::mem::discriminant(plan)));
        }
    }
}

// =====================================================================
//  Phase 6.1: CTE 辅助函数
// =====================================================================

/// 检测 SELECT 是否为递归 CTE 形式：`anchor UNION [ALL] recursive_part`
///
/// 返回 `Some((all, anchor_select, recursive_select))` 当：
/// 1. `select.set_op` 为 Some 且 op 为 Union
/// 2. `recursive_part`（即 right）的 FROM 中引用了 `cte_name`（自身）
///
/// 否则返回 None（视为普通 CTE）。
///
/// 注意：此函数不验证 anchor 不引用自身（PG 语义要求 anchor 不可自引用，
/// 否则视为非法递归 CTE；此处简化为信任用户输入）。
fn detect_recursive_cte(
    select: &Select,
    cte_name: &str,
) -> Option<(bool, Box<Select>, Box<Select>)> {
    let set_op = select.set_op.as_ref()?;
    if !matches!(set_op.op, SetOperator::Union) {
        return None;
    }
    let all = matches!(set_op.quantifier, SetQuantifier::All);
    // 检查 right 是否引用 CTE 自身
    if select_references_table(&set_op.right, cte_name) {
        Some((all, set_op.left.clone(), set_op.right.clone()))
    } else {
        None
    }
}

/// 递归检查 SELECT 是否在 FROM 中引用了指定表名（不区分大小写）
fn select_references_table(select: &Select, table_name: &str) -> bool {
    let target = table_name.to_lowercase();
    for twj in &select.from {
        if table_with_joins_references_table(twj, &target) {
            return true;
        }
    }
    // 检查 set_op 两侧
    if let Some(set_op) = &select.set_op {
        return select_references_table(&set_op.left, &target)
            || select_references_table(&set_op.right, &target);
    }
    false
}

fn table_with_joins_references_table(twj: &TableWithJoins, target: &str) -> bool {
    if table_factor_references_table(&twj.relation, target) {
        return true;
    }
    for join in &twj.joins {
        if table_factor_references_table(&join.relation, target) {
            return true;
        }
    }
    false
}

fn table_factor_references_table(tf: &TableFactor, target: &str) -> bool {
    match tf {
        TableFactor::Table { name, .. } => name.name.to_lowercase() == target,
        TableFactor::Derived { subquery, .. } => select_references_table(subquery, target),
        TableFactor::TableFunction { .. } => false,
    }
}

/// 应用显式列别名到 CTE 输出 schema
///
/// - `base_schema`：CTE 查询体的输出 schema
/// - `columns`：显式列别名列表（空 Vec 表示无重命名）
/// - `cte_name`：CTE 名称（用于命名新 schema）
fn apply_column_aliases(
    base_schema: &TableSchema,
    columns: &[String],
    cte_name: &str,
) -> TableSchema {
    if columns.is_empty() {
        // 无列别名：保留原列名，但改 schema 名为 CTE 名
        return TableSchema {
            name: TableName::new(cte_name),
            columns: base_schema.columns.clone(),
        };
    }
    let renamed: Vec<ColumnDefinition> = base_schema
        .columns
        .iter()
        .zip(columns.iter())
        .map(|(col, alias)| ColumnDefinition {
            name: alias.clone(),
            data_type: col.data_type.clone(),
            not_null: col.not_null,
            primary_key: col.primary_key,
            unique: col.unique,
            default: col.default.clone(),
            check: col.check.clone(),
            references: col.references.clone(),
            enum_values: col.enum_values.clone(),
            custom_type_name: col.custom_type_name.clone(),
            generated: col.generated.clone(),
            comment: col.comment.clone(),
        })
        .collect();
    TableSchema {
        name: TableName::new(cte_name),
        columns: renamed,
    }
}

// 兼容性辅助：把 PlanError + 行号包装成 PlanError（保持错误类型一致）
trait WithRow {
    fn into_error_with_row(self, row: usize) -> PlanError;
}

impl WithRow for PlanError {
    fn into_error_with_row(self, row: usize) -> PlanError {
        match self {
            PlanError::ColumnCountMismatch { expected, actual } => PlanError::InvalidExpression(
                format!("row {row}: column count mismatch (expected {expected}, got {actual})"),
            ),
            other => other,
        }
    }
}
