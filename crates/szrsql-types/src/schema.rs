//! SzRSQL 表结构定义 — ColumnDef / Schema / ForeignKey。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.1 节。
//!
//! 设计要点：
//! - `ColumnDef` 用 builder 模式累积约束（NOT NULL / UNIQUE / DEFAULT / CHECK / FK）
//! - `Schema` 用 `Vec<usize>` 索引主键与唯一约束列，避免名称依赖
//! - `Schema::validate()` 在 DDL 写入前做完整性校验，拒绝越界索引与重复列名
//! - `SchemaError` 使用 thiserror 提供稳定的错误类型匹配

use crate::value::{ColumnType, Value};
use serde::{Deserialize, Serialize};

// =====================================================================
//  外键约束
// =====================================================================

/// 外键动作 — 对应 SQL 标准 ON DELETE / ON UPDATE 子句
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ForeignKeyAction {
    /// NO ACTION — 默认，违反引用完整性时拒绝操作
    NoAction,
    /// RESTRICT — 与 NO ACTION 类似，但检查立即执行
    Restrict,
    /// CASCADE — 级联删除/更新
    Cascade,
    /// SET NULL — 引用行被删/改时置 NULL
    SetNull,
    /// SET DEFAULT — 引用行被删/改时置为默认值
    SetDefault,
}

/// 外键定义
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ForeignKeyDef {
    /// 引用的表名
    pub ref_table: String,
    /// 引用的列名列表（支持复合外键）
    pub ref_columns: Vec<String>,
    /// ON DELETE 动作
    pub on_delete: ForeignKeyAction,
    /// ON UPDATE 动作
    pub on_update: ForeignKeyAction,
}

// =====================================================================
//  列定义
// =====================================================================

/// 列定义 — 描述表中的一列
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnDef {
    /// 列名
    pub name: String,
    /// 列类型
    pub col_type: ColumnType,
    /// NOT NULL 约束
    pub not_null: bool,
    /// 是否为主键的一部分
    pub is_primary_key: bool,
    /// UNIQUE 约束（单列）
    pub is_unique: bool,
    /// DEFAULT 表达式（存储为已求值的 Value）
    pub default: Option<Value>,
    /// 列注释（COMMENT ON COLUMN）
    pub comment: Option<String>,
    /// CHECK 约束表达式（存储为字符串，由 SQL 执行器求值）
    pub check_expr: Option<String>,
    /// ENUM 类型的可选值列表（仅 col_type 为 Enum 时有意义）
    pub enum_values: Option<Vec<String>>,
    /// 列级外键约束
    pub foreign_key: Option<ForeignKeyDef>,
}

impl ColumnDef {
    /// 创建一个新的列定义，所有约束默认关闭
    pub fn new(name: impl Into<String>, col_type: ColumnType) -> Self {
        Self {
            name: name.into(),
            col_type,
            not_null: false,
            is_primary_key: false,
            is_unique: false,
            default: None,
            comment: None,
            check_expr: None,
            enum_values: None,
            foreign_key: None,
        }
    }

    /// 设置 NOT NULL 约束
    pub fn not_null(mut self, v: bool) -> Self {
        self.not_null = v;
        self
    }

    /// 设置是否为主键
    pub fn primary_key(mut self, v: bool) -> Self {
        self.is_primary_key = v;
        self
    }

    /// 设置 UNIQUE 约束
    pub fn unique(mut self, v: bool) -> Self {
        self.is_unique = v;
        self
    }

    /// 设置 DEFAULT 值
    pub fn default(mut self, v: Value) -> Self {
        self.default = Some(v);
        self
    }

    /// 设置列注释
    pub fn comment(mut self, s: impl Into<String>) -> Self {
        self.comment = Some(s.into());
        self
    }

    /// 设置 CHECK 约束表达式
    pub fn check_expr(mut self, s: impl Into<String>) -> Self {
        self.check_expr = Some(s.into());
        self
    }

    /// 设置 ENUM 可选值列表
    pub fn enum_values(mut self, vs: Vec<String>) -> Self {
        self.enum_values = Some(vs);
        self
    }

    /// 设置列级外键
    pub fn foreign_key(mut self, fk: ForeignKeyDef) -> Self {
        self.foreign_key = Some(fk);
        self
    }

    /// 应用默认值：若传入 `Value::Null` 且定义了 DEFAULT，则返回 DEFAULT 值；
    /// 否则原样返回。该函数不进行类型转换，调用方需保证 DEFAULT 与列类型匹配。
    pub fn apply_default(&self, v: &Value) -> Value {
        if matches!(v, Value::Null) {
            if let Some(d) = &self.default {
                return d.clone();
            }
        }
        v.clone()
    }
}

// =====================================================================
//  Schema 错误
// =====================================================================

/// Schema 校验错误
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
pub enum SchemaError {
    /// 列名重复
    #[error("duplicate column name: {name}")]
    DuplicateColumnName {
        /// 重复的列名
        name: String,
    },
    /// 主键索引越界
    #[error("primary key index {index} out of bounds (column count: {column_count})")]
    PrimaryKeyIndexOutOfBounds {
        /// 越界的索引
        index: usize,
        /// 当前列数
        column_count: usize,
    },
    /// 显式设置的主键为空
    #[error("primary key is explicitly set to empty")]
    EmptyPrimaryKey,
    /// 唯一约束索引越界
    #[error("unique constraint index {index} out of bounds (column count: {column_count})")]
    UniqueConstraintIndexOutOfBounds {
        /// 越界的索引
        index: usize,
        /// 当前列数
        column_count: usize,
    },
    /// 主键列允许 NULL（主键列必须 NOT NULL）
    #[error("primary key column at index {index} must be NOT NULL")]
    PrimaryKeyColumnNullable {
        /// 违规列索引
        index: usize,
    },
}

// =====================================================================
//  Schema
// =====================================================================

/// Schema — 表结构定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    /// 表名
    pub table_name: String,
    /// 列定义（按声明顺序）
    pub columns: Vec<ColumnDef>,
    /// 主键列的索引列表（支持复合主键）
    pub primary_key: Vec<usize>,
    /// 唯一约束列表（每个内层 Vec 是一组列索引）
    pub unique_constraints: Vec<Vec<usize>>,
    /// 表级外键约束
    pub foreign_keys: Vec<ForeignKeyDef>,
    /// 表级 CHECK 约束表达式
    pub check_constraints: Vec<String>,
    /// 排序规则（如 "zh_CN", "en_US"）
    pub collation: Option<String>,
    /// 主键是否被显式设置过（用于区分"未设置"与"显式设为空"）
    #[serde(skip)]
    primary_key_explicit: bool,
}

impl PartialEq for Schema {
    /// 比较时忽略 `primary_key_explicit` 跟踪标志
    ///
    /// 原因：该字段仅用于 `validate()` 区分"未设置主键"与"显式设为空主键"，
    /// 不影响 Schema 实际语义；序列化时被 `#[serde(skip)]` 跳过，
    /// 因此反序列化后默认为 `false`，需要手动 `PartialEq` 保证 roundtrip 相等。
    fn eq(&self, other: &Self) -> bool {
        self.table_name == other.table_name
            && self.columns == other.columns
            && self.primary_key == other.primary_key
            && self.unique_constraints == other.unique_constraints
            && self.foreign_keys == other.foreign_keys
            && self.check_constraints == other.check_constraints
            && self.collation == other.collation
    }
}

impl Schema {
    /// 创建一个空的 schema
    pub fn new(table_name: impl Into<String>) -> Self {
        Self {
            table_name: table_name.into(),
            columns: Vec::new(),
            primary_key: Vec::new(),
            unique_constraints: Vec::new(),
            foreign_keys: Vec::new(),
            check_constraints: Vec::new(),
            collation: None,
            primary_key_explicit: false,
        }
    }

    /// 添加一列（builder 风格，返回 `&mut Self` 支持链式调用）
    pub fn add_column(&mut self, col: ColumnDef) -> &mut Self {
        self.columns.push(col);
        self
    }

    /// 从 `columns` 中收集 `is_primary_key == true` 的列索引，作为主键
    ///
    /// 调用此方法会覆盖之前通过 `set_primary_key` 设置的主键。
    pub fn finalize_primary_key(&mut self) -> &mut Self {
        self.primary_key = self
            .columns
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                if c.is_primary_key {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        self.primary_key_explicit = true;
        self
    }

    /// 显式设置主键列索引
    pub fn set_primary_key(&mut self, indices: Vec<usize>) -> &mut Self {
        self.primary_key = indices;
        self.primary_key_explicit = true;
        self
    }

    /// 添加一个唯一约束（列索引列表）
    pub fn add_unique_constraint(&mut self, cols: Vec<usize>) -> &mut Self {
        self.unique_constraints.push(cols);
        self
    }

    /// 添加表级外键
    pub fn add_foreign_key(&mut self, fk: ForeignKeyDef) -> &mut Self {
        self.foreign_keys.push(fk);
        self
    }

    /// 添加表级 CHECK 约束
    pub fn add_check_constraint(&mut self, expr: impl Into<String>) -> &mut Self {
        self.check_constraints.push(expr.into());
        self
    }

    /// 设置排序规则
    pub fn set_collation(&mut self, collation: impl Into<String>) -> &mut Self {
        self.collation = Some(collation.into());
        self
    }

    /// 按列名查找列索引
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// 校验 Schema 完整性
    ///
    /// 检查项：
    /// 1. 列名不重复
    /// 2. 主键索引不越界
    /// 3. 若主键被显式设置，不能为空
    /// 4. 唯一约束索引不越界
    /// 5. 主键列必须 NOT NULL
    pub fn validate(&self) -> Result<(), SchemaError> {
        let column_count = self.columns.len();

        // 1. 检查列名重复
        let mut seen = std::collections::HashSet::with_capacity(column_count);
        for col in &self.columns {
            if !seen.insert(&col.name) {
                return Err(SchemaError::DuplicateColumnName {
                    name: col.name.clone(),
                });
            }
        }

        // 2. 检查主键索引越界
        for &idx in &self.primary_key {
            if idx >= column_count {
                return Err(SchemaError::PrimaryKeyIndexOutOfBounds {
                    index: idx,
                    column_count,
                });
            }
        }

        // 3. 显式设置的主键不能为空
        if self.primary_key_explicit && self.primary_key.is_empty() {
            return Err(SchemaError::EmptyPrimaryKey);
        }

        // 4. 检查唯一约束索引越界
        for constraint in &self.unique_constraints {
            for &idx in constraint {
                if idx >= column_count {
                    return Err(SchemaError::UniqueConstraintIndexOutOfBounds {
                        index: idx,
                        column_count,
                    });
                }
            }
        }

        // 5. 主键列必须 NOT NULL
        for &idx in &self.primary_key {
            if !self.columns[idx].not_null {
                return Err(SchemaError::PrimaryKeyColumnNullable { index: idx });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{ColumnType, RangeType, Value};
    use serde_json;

    // -----------------------------------------------------------------
    //  ColumnDef 构造与序列化
    // -----------------------------------------------------------------

    #[test]
    fn column_def_basic_construction() {
        let col = ColumnDef::new("id", ColumnType::Int64)
            .not_null(true)
            .primary_key(true);
        assert_eq!(col.name, "id");
        assert_eq!(col.col_type, ColumnType::Int64);
        assert!(col.not_null);
        assert!(col.is_primary_key);
        assert!(!col.is_unique);
        assert!(col.default.is_none());
        assert!(col.comment.is_none());
        assert!(col.check_expr.is_none());
        assert!(col.enum_values.is_none());
        assert!(col.foreign_key.is_none());
    }

    #[test]
    fn column_def_with_all_attributes() {
        let col = ColumnDef::new(
            "price",
            ColumnType::Decimal {
                precision: 10,
                scale: 2,
            },
        )
        .not_null(true)
        .unique(true)
        .default(Value::Decimal(0, 2))
        .comment("商品价格")
        .check_expr("price >= 0");
        assert_eq!(col.name, "price");
        assert!(col.not_null);
        assert!(col.is_unique);
        assert_eq!(col.default, Some(Value::Decimal(0, 2)));
        assert_eq!(col.comment.as_deref(), Some("商品价格"));
        assert_eq!(col.check_expr.as_deref(), Some("price >= 0"));
    }

    #[test]
    fn column_def_with_enum_values() {
        let col = ColumnDef::new("status", ColumnType::Enum(vec![]))
            .enum_values(vec!["active".to_string(), "inactive".to_string()]);
        assert_eq!(
            col.enum_values.as_deref(),
            Some(&["active".to_string(), "inactive".to_string()][..])
        );
    }

    #[test]
    fn column_def_serde_roundtrip() {
        let col = ColumnDef::new("ts", ColumnType::Timestamp)
            .not_null(true)
            .default(Value::Timestamp(0))
            .comment("创建时间");
        let json = serde_json::to_string(&col).expect("serialize ColumnDef");
        let back: ColumnDef = serde_json::from_str(&json).expect("deserialize ColumnDef");
        assert_eq!(col, back);
    }

    #[test]
    fn column_default_default_all_none() {
        let col = ColumnDef::new("name", ColumnType::Text);
        assert!(!col.not_null);
        assert!(!col.is_primary_key);
        assert!(!col.is_unique);
        assert!(col.default.is_none());
        assert!(col.comment.is_none());
        assert!(col.check_expr.is_none());
        assert!(col.enum_values.is_none());
        assert!(col.foreign_key.is_none());
    }

    // -----------------------------------------------------------------
    //  ForeignKeyDef / ForeignKeyAction
    // -----------------------------------------------------------------

    #[test]
    fn foreign_key_action_variants() {
        assert_eq!(ForeignKeyAction::NoAction, ForeignKeyAction::NoAction);
        assert_eq!(ForeignKeyAction::Restrict, ForeignKeyAction::Restrict);
        assert_eq!(ForeignKeyAction::Cascade, ForeignKeyAction::Cascade);
        assert_eq!(ForeignKeyAction::SetNull, ForeignKeyAction::SetNull);
        assert_eq!(ForeignKeyAction::SetDefault, ForeignKeyAction::SetDefault);
        assert_ne!(ForeignKeyAction::NoAction, ForeignKeyAction::Restrict);
    }

    #[test]
    fn foreign_key_def_construction() {
        let fk = ForeignKeyDef {
            ref_table: "users".to_string(),
            ref_columns: vec!["id".to_string()],
            on_delete: ForeignKeyAction::Cascade,
            on_update: ForeignKeyAction::Restrict,
        };
        assert_eq!(fk.ref_table, "users");
        assert_eq!(fk.ref_columns, vec!["id".to_string()]);
        assert_eq!(fk.on_delete, ForeignKeyAction::Cascade);
        assert_eq!(fk.on_update, ForeignKeyAction::Restrict);
    }

    #[test]
    fn foreign_key_def_serde_roundtrip() {
        let fk = ForeignKeyDef {
            ref_table: "orders".to_string(),
            ref_columns: vec!["order_id".to_string(), "line".to_string()],
            on_delete: ForeignKeyAction::SetNull,
            on_update: ForeignKeyAction::NoAction,
        };
        let json = serde_json::to_string(&fk).expect("serialize ForeignKeyDef");
        let back: ForeignKeyDef = serde_json::from_str(&json).expect("deserialize ForeignKeyDef");
        assert_eq!(fk, back);
    }

    #[test]
    fn column_def_with_foreign_key() {
        let col = ColumnDef::new("user_id", ColumnType::Int64)
            .not_null(true)
            .foreign_key(ForeignKeyDef {
                ref_table: "users".to_string(),
                ref_columns: vec!["id".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Cascade,
            });
        assert!(col.foreign_key.is_some());
        let fk = col.foreign_key.as_ref().unwrap();
        assert_eq!(fk.ref_table, "users");
        assert_eq!(fk.on_delete, ForeignKeyAction::Cascade);
    }

    // -----------------------------------------------------------------
    //  Schema 构造与序列化
    // -----------------------------------------------------------------

    #[test]
    fn schema_empty_construction() {
        let schema = Schema::new("empty_table");
        assert_eq!(schema.table_name, "empty_table");
        assert!(schema.columns.is_empty());
        assert!(schema.primary_key.is_empty());
        assert!(schema.unique_constraints.is_empty());
        assert!(schema.foreign_keys.is_empty());
        assert!(schema.check_constraints.is_empty());
        assert!(schema.collation.is_none());
    }

    #[test]
    fn schema_with_single_primary_key() {
        let mut schema = Schema::new("users");
        schema.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(ColumnDef::new("name", ColumnType::Text).not_null(true));
        schema.finalize_primary_key();
        assert_eq!(schema.primary_key, vec![0]);
    }

    #[test]
    fn schema_with_composite_primary_key() {
        let mut schema = Schema::new("order_lines");
        schema.add_column(
            ColumnDef::new("order_id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(
            ColumnDef::new("line_no", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(ColumnDef::new("product", ColumnType::Text));
        schema.finalize_primary_key();
        assert_eq!(schema.primary_key, vec![0, 1]);
    }

    #[test]
    fn schema_with_explicit_primary_key_indices() {
        let mut schema = Schema::new("t");
        schema.add_column(ColumnDef::new("a", ColumnType::Int64));
        schema.add_column(ColumnDef::new("b", ColumnType::Int64));
        schema.add_column(ColumnDef::new("c", ColumnType::Int64));
        schema.set_primary_key(vec![1, 2]);
        assert_eq!(schema.primary_key, vec![1, 2]);
    }

    #[test]
    fn schema_serde_roundtrip() {
        let mut schema = Schema::new("products");
        schema.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(
            ColumnDef::new("name", ColumnType::Text)
                .not_null(true)
                .default(Value::Text("unnamed".to_string())),
        );
        schema.add_column(
            ColumnDef::new(
                "price",
                ColumnType::Decimal {
                    precision: 10,
                    scale: 2,
                },
            )
            .check_expr("price >= 0"),
        );
        schema.finalize_primary_key();
        schema.add_unique_constraint(vec![1]);
        schema.add_check_constraint("name <> ''");
        schema.set_collation("zh_CN");

        let json = serde_json::to_string(&schema).expect("serialize Schema");
        let back: Schema = serde_json::from_str(&json).expect("deserialize Schema");
        assert_eq!(schema, back);
    }

    // -----------------------------------------------------------------
    //  Schema 约束辅助方法
    // -----------------------------------------------------------------

    #[test]
    fn schema_column_lookup_by_name() {
        let mut schema = Schema::new("t");
        schema.add_column(ColumnDef::new("id", ColumnType::Int64));
        schema.add_column(ColumnDef::new("name", ColumnType::Text));
        assert_eq!(schema.column_index("id"), Some(0));
        assert_eq!(schema.column_index("name"), Some(1));
        assert_eq!(schema.column_index("missing"), None);
    }

    #[test]
    fn schema_unique_constraints() {
        let mut schema = Schema::new("t");
        schema.add_column(ColumnDef::new("id", ColumnType::Int64));
        schema.add_column(ColumnDef::new("email", ColumnType::Text));
        schema.add_column(ColumnDef::new("phone", ColumnType::Text));
        schema.add_unique_constraint(vec![1]);
        schema.add_unique_constraint(vec![2, 1]);
        assert_eq!(schema.unique_constraints.len(), 2);
        assert_eq!(schema.unique_constraints[0], vec![1]);
        assert_eq!(schema.unique_constraints[1], vec![2, 1]);
    }

    #[test]
    fn schema_table_level_foreign_keys() {
        let mut schema = Schema::new("orders");
        schema.add_column(ColumnDef::new("id", ColumnType::Int64).primary_key(true));
        schema.add_column(ColumnDef::new("user_id", ColumnType::Int64));
        schema.add_foreign_key(ForeignKeyDef {
            ref_table: "users".to_string(),
            ref_columns: vec!["id".to_string()],
            on_delete: ForeignKeyAction::Cascade,
            on_update: ForeignKeyAction::Restrict,
        });
        assert_eq!(schema.foreign_keys.len(), 1);
        assert_eq!(schema.foreign_keys[0].ref_table, "users");
    }

    #[test]
    fn schema_check_constraints() {
        let mut schema = Schema::new("t");
        schema.add_check_constraint("a > 0");
        schema.add_check_constraint("b < 100");
        assert_eq!(schema.check_constraints.len(), 2);
        assert_eq!(schema.check_constraints[0], "a > 0");
        assert_eq!(schema.check_constraints[1], "b < 100");
    }

    #[test]
    fn schema_collation() {
        let mut schema = Schema::new("t");
        schema.set_collation("en_US");
        assert_eq!(schema.collation.as_deref(), Some("en_US"));
    }

    // -----------------------------------------------------------------
    //  Schema 校验
    // -----------------------------------------------------------------

    #[test]
    fn schema_validate_rejects_duplicate_column_names() {
        let mut schema = Schema::new("t");
        schema.add_column(ColumnDef::new("id", ColumnType::Int64));
        schema.add_column(ColumnDef::new("id", ColumnType::Text));
        let err = schema
            .validate()
            .expect_err("duplicate column names should fail");
        assert!(matches!(err, SchemaError::DuplicateColumnName { ref name } if name == "id"));
    }

    #[test]
    fn schema_validate_rejects_primary_key_out_of_bounds() {
        let mut schema = Schema::new("t");
        schema.add_column(ColumnDef::new("a", ColumnType::Int64));
        schema.set_primary_key(vec![5]);
        let err = schema.validate().expect_err("PK out of bounds should fail");
        assert!(
            matches!(err, SchemaError::PrimaryKeyIndexOutOfBounds { index, column_count } if index == 5 && column_count == 1)
        );
    }

    #[test]
    fn schema_validate_rejects_empty_primary_key_index() {
        let mut schema = Schema::new("t");
        schema.add_column(ColumnDef::new("a", ColumnType::Int64));
        schema.set_primary_key(vec![]);
        let err = schema.validate().expect_err("empty PK should fail");
        assert!(matches!(err, SchemaError::EmptyPrimaryKey));
    }

    #[test]
    fn schema_validate_rejects_unique_constraint_out_of_bounds() {
        let mut schema = Schema::new("t");
        schema.add_column(ColumnDef::new("a", ColumnType::Int64));
        schema.add_unique_constraint(vec![3]);
        let err = schema.validate().expect_err("unique OOB should fail");
        assert!(
            matches!(err, SchemaError::UniqueConstraintIndexOutOfBounds { index, column_count } if index == 3 && column_count == 1)
        );
    }

    #[test]
    fn schema_validate_rejects_primary_key_column_nullable() {
        let mut schema = Schema::new("t");
        schema.add_column(ColumnDef::new("id", ColumnType::Int64).primary_key(true));
        // 没有设置 not_null
        schema.finalize_primary_key();
        let err = schema.validate().expect_err("nullable PK should fail");
        assert!(matches!(err, SchemaError::PrimaryKeyColumnNullable { index } if index == 0));
    }

    #[test]
    fn schema_validate_accepts_valid_schema() {
        let mut schema = Schema::new("users");
        schema.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(
            ColumnDef::new("email", ColumnType::Text)
                .not_null(true)
                .unique(true),
        );
        schema.finalize_primary_key();
        schema.add_unique_constraint(vec![1]);
        schema.validate().expect("valid schema should pass");
    }

    #[test]
    fn schema_error_display() {
        let err = SchemaError::DuplicateColumnName {
            name: "id".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("id"));
        assert!(s.contains("duplicate"));
    }

    // -----------------------------------------------------------------
    //  Schema Builder 风格（链式 add_column 返回 &mut Self）
    // -----------------------------------------------------------------

    #[test]
    fn schema_builder_chaining() {
        let mut schema = Schema::new("t");
        schema
            .add_column(
                ColumnDef::new("a", ColumnType::Int64)
                    .primary_key(true)
                    .not_null(true),
            )
            .add_column(ColumnDef::new("b", ColumnType::Text));
        schema.finalize_primary_key();
        assert_eq!(schema.columns.len(), 2);
        assert_eq!(schema.primary_key, vec![0]);
    }

    // -----------------------------------------------------------------
    //  ColumnDef default value 应用
    // -----------------------------------------------------------------

    #[test]
    fn column_def_apply_default_when_value_null() {
        let col =
            ColumnDef::new("status", ColumnType::Text).default(Value::Text("active".to_string()));
        let applied = col.apply_default(&Value::Null);
        assert_eq!(applied, Value::Text("active".to_string()));
    }

    #[test]
    fn column_def_apply_default_passthrough_when_value_not_null() {
        let col =
            ColumnDef::new("status", ColumnType::Text).default(Value::Text("active".to_string()));
        let applied = col.apply_default(&Value::Text("custom".to_string()));
        assert_eq!(applied, Value::Text("custom".to_string()));
    }

    #[test]
    fn column_def_apply_default_no_default_returns_null() {
        let col = ColumnDef::new("status", ColumnType::Text);
        let applied = col.apply_default(&Value::Null);
        assert_eq!(applied, Value::Null);
    }

    // -----------------------------------------------------------------
    //  边界值与跨类型组合
    // -----------------------------------------------------------------

    #[test]
    fn schema_with_all_column_types() {
        let mut schema = Schema::new("all_types");
        schema.add_column(ColumnDef::new("c_null", ColumnType::Null));
        schema.add_column(ColumnDef::new("c_int", ColumnType::Int64));
        schema.add_column(ColumnDef::new("c_float", ColumnType::Float64));
        schema.add_column(ColumnDef::new("c_text", ColumnType::Text));
        schema.add_column(ColumnDef::new("c_blob", ColumnType::Blob));
        schema.add_column(ColumnDef::new("c_bool", ColumnType::Bool));
        schema.add_column(ColumnDef::new("c_date", ColumnType::Date));
        schema.add_column(ColumnDef::new("c_ts", ColumnType::Timestamp));
        schema.add_column(ColumnDef::new(
            "c_dec",
            ColumnType::Decimal {
                precision: 38,
                scale: 10,
            },
        ));
        schema.add_column(ColumnDef::new(
            "c_arr",
            ColumnType::Array(Box::new(ColumnType::Int64)),
        ));
        schema.add_column(ColumnDef::new(
            "c_enum",
            ColumnType::Enum(vec!["a".to_string()]),
        ));
        schema.add_column(ColumnDef::new(
            "c_range",
            ColumnType::Range(RangeType::Int4Range),
        ));
        schema.add_column(ColumnDef::new("c_json", ColumnType::Json));
        schema.validate().expect("all column types should be valid");
        assert_eq!(schema.columns.len(), 13);
    }

    #[test]
    fn schema_with_many_columns_stress() {
        let mut schema = Schema::new("wide");
        for i in 0..1000 {
            schema.add_column(ColumnDef::new(format!("col_{i}"), ColumnType::Int64));
        }
        schema.validate().expect("1000 columns should be valid");
        assert_eq!(schema.columns.len(), 1000);
        assert_eq!(schema.column_index("col_999"), Some(999));
    }

    // -----------------------------------------------------------------
    //  Schema PartialEq 字段差异测试
    // -----------------------------------------------------------------
    //
    // Schema 手动实现了 PartialEq（忽略 primary_key_explicit 跟踪标志），
    // 以下测试确保每个字段的差异都会返回 false，杀死 `&& -> ||` 和
    // `eq -> true` 类型的变异体。

    /// 构造基准 Schema：包含所有字段以覆盖每条 && 链
    fn make_baseline_schema() -> Schema {
        let mut schema = Schema::new("products");
        schema.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(
            ColumnDef::new("name", ColumnType::Text).not_null(true),
        );
        schema.add_column(
            ColumnDef::new(
                "price",
                ColumnType::Decimal {
                    precision: 10,
                    scale: 2,
                },
            )
            .check_expr("price >= 0"),
        );
        schema.finalize_primary_key();
        schema.add_unique_constraint(vec![1]);
        schema.add_foreign_key(ForeignKeyDef {
            ref_table: "suppliers".to_string(),
            ref_columns: vec!["id".to_string()],
            on_delete: ForeignKeyAction::Cascade,
            on_update: ForeignKeyAction::Restrict,
        });
        schema.add_check_constraint("name <> ''");
        schema.set_collation("zh_CN");
        schema
    }

    #[test]
    fn schema_eq_same_instance() {
        let s1 = make_baseline_schema();
        let s2 = make_baseline_schema();
        assert_eq!(s1, s2);
    }

    #[test]
    fn schema_neq_different_table_name() {
        let mut s1 = make_baseline_schema();
        s1.table_name = "different".to_string();
        let s2 = make_baseline_schema();
        assert_ne!(s1, s2);
    }

    #[test]
    fn schema_neq_different_columns() {
        let s1 = make_baseline_schema();
        let mut s2 = make_baseline_schema();
        s2.columns[1].name = "different_name".to_string();
        assert_ne!(s1, s2);
    }

    #[test]
    fn schema_neq_different_primary_key() {
        let s1 = make_baseline_schema();
        let mut s2 = make_baseline_schema();
        s2.primary_key = vec![1];
        assert_ne!(s1, s2);
    }

    #[test]
    fn schema_neq_different_unique_constraints() {
        let s1 = make_baseline_schema();
        let mut s2 = make_baseline_schema();
        s2.unique_constraints = vec![vec![0]];
        assert_ne!(s1, s2);
    }

    #[test]
    fn schema_neq_different_foreign_keys() {
        let s1 = make_baseline_schema();
        let mut s2 = make_baseline_schema();
        s2.foreign_keys[0].ref_table = "different".to_string();
        assert_ne!(s1, s2);
    }

    #[test]
    fn schema_neq_different_check_constraints() {
        let s1 = make_baseline_schema();
        let mut s2 = make_baseline_schema();
        s2.check_constraints[0] = "a > 0".to_string();
        assert_ne!(s1, s2);
    }

    #[test]
    fn schema_neq_different_collation() {
        let s1 = make_baseline_schema();
        let mut s2 = make_baseline_schema();
        s2.set_collation("en_US");
        assert_ne!(s1, s2);
    }

    #[test]
    fn schema_neq_collation_none_vs_some() {
        let s1 = make_baseline_schema();
        let mut s2 = make_baseline_schema();
        s2.collation = None;
        assert_ne!(s1, s2);
    }

    #[test]
    fn schema_eq_ignores_primary_key_explicit() {
        // primary_key_explicit 不参与比较
        let s1 = make_baseline_schema();
        let mut s2 = make_baseline_schema();
        // 通过反射设置不可行（无反射），改为序列化 → 反序列化（serde skip 会丢失该字段）
        let json = serde_json::to_string(&s1).expect("serialize");
        s2 = serde_json::from_str(&json).expect("deserialize");
        // primary_key_explicit 默认为 false，但 s1 可能是 true（finalize_primary_key 调用过）
        assert_eq!(s1, s2);
    }

    #[test]
    fn schema_eq_after_serde_roundtrip() {
        let s1 = make_baseline_schema();
        let json = serde_json::to_string(&s1).expect("serialize");
        let s2: Schema = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s1, s2);
    }
}
