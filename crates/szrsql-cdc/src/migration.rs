//! 结构迁移 — Schema diff + 方言 DDL 生成
//!
//! # 设计要点
//!
//! 1. **Schema 比对**：对比源端和目标端的 TableSchema，找出差异
//! 2. **DDL 生成**：根据差异生成目标端方言的 DDL 语句（CREATE/ALTER/DROP）
//! 3. **方言适配**：支持 PostgreSQL / MySQL / SQLite / Oracle / SQL Server 方言
//! 4. **安全性**：生成的 DDL 使用 IF NOT EXISTS / IF EXISTS，避免幂等性问题
//! 5. **类型映射**：源端 DataType → 目标端方言类型（如 Int64 → BIGINT / NUMBER(19)）
//!
//! # 流程
//!
//! ```text
//! 1. 提取源端 schema（通过 SchemaRegistry）
//! 2. 提取目标端 schema（通过 information_schema 或 SHOW CREATE TABLE）
//! 3. 比对两端 schema，生成 SchemaDiff
//! 4. 根据 SchemaDiff + 目标端方言，生成 DDL 列表
//! 5. 在目标端执行 DDL（通过 TargetWriter.ensure_table 或直接执行）
//! ```

use crate::schema::{ColumnDef, DataType, TableSchema};
use std::collections::{HashMap, HashSet};

// =====================================================================
// SchemaDiff — Schema 差异
// =====================================================================

/// 表是否存在
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableExistence {
    /// 源端有，目标端无（需要 CREATE）
    SourceOnly,
    /// 两端都有（可能需要 ALTER）
    Both,
    /// 目标端有，源端无（可 DROP 或忽略）
    TargetOnly,
}

/// 单列差异
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnDiff {
    /// 列在源端存在，目标端不存在（需要 ADD COLUMN）
    AddColumn(ColumnDef),
    /// 列在两端都存在但类型/可空性不同（需要 ALTER COLUMN TYPE）
    TypeMismatch {
        /// 源端列定义
        source: ColumnDef,
        /// 目标端列定义
        target: ColumnDef,
    },
    /// 列在两端都存在且一致（无差异）
    NoDiff,
}

/// 表级 Schema 差异
#[derive(Debug, Clone)]
pub struct TableDiff {
    /// 表名
    pub table_name: String,
    /// 表存在性
    pub existence: TableExistence,
    /// 列差异列表（仅列出有差异的列）
    pub column_diffs: Vec<ColumnDiff>,
}

/// Schema 级差异（多张表）
#[derive(Debug, Clone, Default)]
pub struct SchemaDiff {
    /// 所有表的差异
    pub tables: Vec<TableDiff>,
}

impl SchemaDiff {
    /// 是否无差异
    pub fn is_empty(&self) -> bool {
        self.tables
            .iter()
            .all(|t| t.existence == TableExistence::Both && t.column_diffs.is_empty())
    }

    /// 差异表数量
    pub fn diff_table_count(&self) -> usize {
        self.tables
            .iter()
            .filter(|t| !(t.existence == TableExistence::Both && t.column_diffs.is_empty()))
            .count()
    }
}

// =====================================================================
// Dialect — 目标端方言
// =====================================================================

/// 数据库方言
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// PostgreSQL
    Postgres,
    /// MySQL
    MySQL,
    /// SQLite
    SQLite,
    /// Oracle
    Oracle,
    /// SQL Server
    SqlServer,
}

impl Dialect {
    /// 转字符串
    pub fn as_str(self) -> &'static str {
        match self {
            Dialect::Postgres => "postgres",
            Dialect::MySQL => "mysql",
            Dialect::SQLite => "sqlite",
            Dialect::Oracle => "oracle",
            Dialect::SqlServer => "sqlserver",
        }
    }

    /// 从字符串解析
    ///
    /// 注：未实现 `std::str::FromStr` trait 是因为返回 `Option<Self>` 而非 `Result<Self, E>`，
    /// 便于调用方用 `?` 短路。如需统一 trait 实现，可在未来版本中迁移。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" => Some(Dialect::Postgres),
            "mysql" => Some(Dialect::MySQL),
            "sqlite" => Some(Dialect::SQLite),
            "oracle" => Some(Dialect::Oracle),
            "sqlserver" | "mssql" => Some(Dialect::SqlServer),
            _ => None,
        }
    }
}

impl std::fmt::Display for Dialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
// DdlStatement — DDL 语句
// =====================================================================

/// DDL 语句类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdlKind {
    /// CREATE TABLE
    CreateTable,
    /// ALTER TABLE ADD COLUMN
    AddColumn,
    /// ALTER TABLE ALTER COLUMN TYPE
    AlterColumnType,
    /// DROP TABLE
    DropTable,
}

/// DDL 语句
#[derive(Debug, Clone)]
pub struct DdlStatement {
    /// DDL 类型
    pub kind: DdlKind,
    /// 表名
    pub table_name: String,
    /// SQL 语句
    pub sql: String,
}

// =====================================================================
// SchemaComparer — Schema 比对器
// =====================================================================

/// Schema 比对器
pub struct SchemaComparer;

impl SchemaComparer {
    /// 比对源端和目标端的 Schema
    ///
    /// # 参数
    /// - `source`：源端表 schema 列表
    /// - `target`：目标端表 schema 列表
    ///
    /// # 返回
    /// - `SchemaDiff`：差异描述
    pub fn compare(source: &[TableSchema], target: &[TableSchema]) -> SchemaDiff {
        let source_map: HashMap<&str, &TableSchema> =
            source.iter().map(|s| (s.table_name.as_str(), s)).collect();
        let target_map: HashMap<&str, &TableSchema> =
            target.iter().map(|s| (s.table_name.as_str(), s)).collect();

        let all_names: HashSet<&str> = source_map
            .keys()
            .chain(target_map.keys())
            .copied()
            .collect();

        let mut tables = Vec::with_capacity(all_names.len());

        for name in all_names {
            let existence = match (source_map.get(name), target_map.get(name)) {
                (Some(_), None) => TableExistence::SourceOnly,
                (Some(_), Some(_)) => TableExistence::Both,
                (None, Some(_)) => TableExistence::TargetOnly,
                (None, None) => continue,
            };

            let column_diffs = match (&source_map.get(name), &target_map.get(name)) {
                (Some(src), Some(tgt)) => Self::compare_columns(src, tgt),
                _ => Vec::new(),
            };

            tables.push(TableDiff {
                table_name: name.to_string(),
                existence,
                column_diffs,
            });
        }

        SchemaDiff { tables }
    }

    /// 比对两张表的列差异
    fn compare_columns(source: &TableSchema, target: &TableSchema) -> Vec<ColumnDiff> {
        let source_cols: HashMap<&str, &ColumnDef> = source
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();
        let target_cols: HashMap<&str, &ColumnDef> = target
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();

        let mut diffs = Vec::new();

        // 检查源端有但目标端没有的列（ADD COLUMN）
        for (name, src_col) in &source_cols {
            if !target_cols.contains_key(*name) {
                diffs.push(ColumnDiff::AddColumn((*src_col).clone()));
            }
        }

        // 检查类型/可空性差异
        for (name, src_col) in &source_cols {
            if let Some(tgt_col) = target_cols.get(*name) {
                if src_col.data_type != tgt_col.data_type || src_col.nullable != tgt_col.nullable {
                    diffs.push(ColumnDiff::TypeMismatch {
                        source: (*src_col).clone(),
                        target: (*tgt_col).clone(),
                    });
                }
            }
        }

        diffs
    }
}

// =====================================================================
// DdlGenerator — DDL 生成器
// =====================================================================

/// DDL 生成器 — 根据 SchemaDiff + 方言生成 DDL 语句
pub struct DdlGenerator {
    /// 目标端方言
    dialect: Dialect,
}

impl DdlGenerator {
    /// 创建 DDL 生成器
    pub fn new(dialect: Dialect) -> Self {
        Self { dialect }
    }

    /// 根据 SchemaDiff 生成 DDL 语句列表
    ///
    /// # 参数
    /// - `diff`：Schema 差异
    /// - `source_schemas`：源端 schema（用于 CREATE TABLE 时获取完整列定义）
    ///
    /// # 返回
    /// - `Vec<DdlStatement>`：按依赖顺序排列的 DDL 列表
    pub fn generate(&self, diff: &SchemaDiff, source_schemas: &[TableSchema]) -> Vec<DdlStatement> {
        let source_map: HashMap<&str, &TableSchema> = source_schemas
            .iter()
            .map(|s| (s.table_name.as_str(), s))
            .collect();

        let mut ddls = Vec::new();

        for table_diff in &diff.tables {
            match table_diff.existence {
                TableExistence::SourceOnly => {
                    // CREATE TABLE
                    if let Some(schema) = source_map.get(table_diff.table_name.as_str()) {
                        let sql = self.generate_create_table(schema);
                        ddls.push(DdlStatement {
                            kind: DdlKind::CreateTable,
                            table_name: table_diff.table_name.clone(),
                            sql,
                        });
                    }
                }
                TableExistence::Both => {
                    // ALTER TABLE（ADD COLUMN / ALTER COLUMN TYPE）
                    for col_diff in &table_diff.column_diffs {
                        match col_diff {
                            ColumnDiff::AddColumn(col) => {
                                let sql = self.generate_add_column(&table_diff.table_name, col);
                                ddls.push(DdlStatement {
                                    kind: DdlKind::AddColumn,
                                    table_name: table_diff.table_name.clone(),
                                    sql,
                                });
                            }
                            ColumnDiff::TypeMismatch { source, .. } => {
                                let sql =
                                    self.generate_alter_column_type(&table_diff.table_name, source);
                                ddls.push(DdlStatement {
                                    kind: DdlKind::AlterColumnType,
                                    table_name: table_diff.table_name.clone(),
                                    sql,
                                });
                            }
                            ColumnDiff::NoDiff => {}
                        }
                    }
                }
                TableExistence::TargetOnly => {
                    // 默认不删除目标端多余的表（保守策略）
                    // 如需删除，调用方可在配置中启用
                }
            }
        }

        ddls
    }

    /// 生成 CREATE TABLE 语句
    pub fn generate_create_table(&self, schema: &TableSchema) -> String {
        match self.dialect {
            Dialect::Postgres | Dialect::SQLite => {
                self.generate_create_table_standard(schema, "IF NOT EXISTS")
            }
            Dialect::MySQL => self.generate_create_table_standard(schema, "IF NOT EXISTS"),
            Dialect::Oracle => {
                // Oracle 不支持 IF NOT EXISTS，用 PL/SQL 块或先检查
                // 简化：直接 CREATE TABLE，依赖调用方捕获错误
                self.generate_create_table_standard(schema, "")
            }
            Dialect::SqlServer => {
                // SQL Server 也不支持 IF NOT EXISTS，需要 IF NOT EXISTS 包装
                format!(
                    "IF NOT EXISTS (SELECT * FROM sys.tables WHERE name = '{}') {}\n",
                    schema.table_name,
                    self.generate_create_table_standard(schema, "")
                )
            }
        }
    }

    /// 生成标准 CREATE TABLE（按方言引用标识符）
    fn generate_create_table_standard(&self, schema: &TableSchema, if_not_exists: &str) -> String {
        let table_name = self.quote_ident(&schema.table_name);

        let prefix = if if_not_exists.is_empty() {
            "CREATE TABLE".to_string()
        } else {
            format!("CREATE TABLE {if_not_exists}")
        };

        let cols: Vec<String> = schema
            .columns
            .iter()
            .map(|c| {
                let col_name = self.quote_ident(&c.name);
                let pg_type = self.map_type(c.data_type);
                let nullability = if c.nullable {
                    ""
                } else {
                    " NOT NULL"
                };
                format!("{col_name} {pg_type}{nullability}")
            })
            .collect();

        format!("{prefix} {table_name} ({});", cols.join(", "))
    }

    /// 生成 ADD COLUMN 语句
    pub fn generate_add_column(&self, table_name: &str, col: &ColumnDef) -> String {
        let table = self.quote_ident(table_name);
        let column = self.quote_ident(&col.name);
        let col_type = self.map_type(col.data_type);
        let nullability = if col.nullable {
            ""
        } else {
            " NOT NULL"
        };

        match self.dialect {
            Dialect::Postgres | Dialect::SQLite => {
                format!("ALTER TABLE {table} ADD COLUMN {column} {col_type}{nullability};")
            }
            Dialect::MySQL => {
                format!("ALTER TABLE {table} ADD COLUMN {column} {col_type}{nullability};")
            }
            Dialect::Oracle => {
                format!("ALTER TABLE {table} ADD ({column} {col_type}{nullability});")
            }
            Dialect::SqlServer => {
                format!("ALTER TABLE {table} ADD {column} {col_type}{nullability};")
            }
        }
    }

    /// 生成 ALTER COLUMN TYPE 语句
    pub fn generate_alter_column_type(&self, table_name: &str, col: &ColumnDef) -> String {
        let table = self.quote_ident(table_name);
        let column = self.quote_ident(&col.name);
        let col_type = self.map_type(col.data_type);

        match self.dialect {
            Dialect::Postgres => {
                format!("ALTER TABLE {table} ALTER COLUMN {column} TYPE {col_type};")
            }
            Dialect::MySQL => {
                // MySQL 用 MODIFY COLUMN
                let nullability = if col.nullable {
                    ""
                } else {
                    " NOT NULL"
                };
                format!("ALTER TABLE {table} MODIFY COLUMN {column} {col_type}{nullability};")
            }
            Dialect::SQLite => {
                // SQLite 不支持 ALTER COLUMN TYPE，需要重建表
                format!(
                    "-- SQLite 不支持 ALTER COLUMN TYPE，需重建表: {table}.{column} -> {col_type}"
                )
            }
            Dialect::Oracle => {
                format!("ALTER TABLE {table} MODIFY ({column} {col_type});")
            }
            Dialect::SqlServer => {
                format!("ALTER TABLE {table} ALTER COLUMN {column} {col_type};")
            }
        }
    }

    /// 生成 DROP TABLE 语句
    pub fn generate_drop_table(&self, table_name: &str) -> String {
        let table = self.quote_ident(table_name);
        match self.dialect {
            Dialect::Postgres | Dialect::SQLite => {
                format!("DROP TABLE IF EXISTS {table};")
            }
            Dialect::MySQL => {
                format!("DROP TABLE IF EXISTS {table};")
            }
            Dialect::Oracle => {
                // Oracle 不支持 IF EXISTS
                format!("DROP TABLE {table};")
            }
            Dialect::SqlServer => {
                format!("IF EXISTS (SELECT * FROM sys.tables WHERE name = '{table_name}') DROP TABLE {table};")
            }
        }
    }

    /// 按方言引用标识符（MySQL 用反引号，其他用双引号）
    fn quote_ident(&self, name: &str) -> String {
        match self.dialect {
            Dialect::MySQL => format!("`{}`", name.replace('`', "``")),
            _ => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    /// 类型映射：DataType → 目标方言类型
    fn map_type(&self, dt: DataType) -> String {
        match self.dialect {
            Dialect::Postgres => pg_type_name(dt).to_string(),
            Dialect::MySQL => mysql_type_name(dt).to_string(),
            Dialect::SQLite => sqlite_type_name(dt).to_string(),
            Dialect::Oracle => oracle_type_name(dt).to_string(),
            Dialect::SqlServer => sqlserver_type_name(dt).to_string(),
        }
    }

    /// 获取方言
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }
}

// =====================================================================
// StructureMigration — 结构迁移器
// =====================================================================

/// 结构迁移器 — 协调 Schema 比对、DDL 生成、目标端执行
pub struct StructureMigration {
    /// DDL 生成器
    generator: DdlGenerator,
}

/// 结构迁移结果
#[derive(Debug, Clone, Default)]
pub struct MigrationResult {
    /// 执行的 DDL 数量
    pub ddl_count: usize,
    /// 各 DDL 执行结果（DDL SQL → 是否成功 + 错误消息）
    pub results: Vec<(DdlStatement, Result<(), String>)>,
    /// 成功数量
    pub success_count: usize,
    /// 失败数量
    pub failure_count: usize,
}

impl StructureMigration {
    /// 创建结构迁移器
    pub fn new(dialect: Dialect) -> Self {
        Self {
            generator: DdlGenerator::new(dialect),
        }
    }

    /// 比对源端和目标端 Schema，生成 DDL 列表（不执行）
    ///
    /// # 参数
    /// - `source`：源端 schema
    /// - `target`：目标端 schema
    ///
    /// # 返回
    /// - `(SchemaDiff, Vec<DdlStatement>)`：差异 + 待执行的 DDL
    pub fn plan(
        &self,
        source: &[TableSchema],
        target: &[TableSchema],
    ) -> (SchemaDiff, Vec<DdlStatement>) {
        let diff = SchemaComparer::compare(source, target);
        let ddls = self.generator.generate(&diff, source);
        (diff, ddls)
    }

    /// 执行迁移（在目标端执行 DDL）
    ///
    /// # 参数
    /// - `ddls`：待执行的 DDL 列表
    /// - `executor`：DDL 执行回调
    pub fn execute<F>(&self, ddls: Vec<DdlStatement>, mut executor: F) -> MigrationResult
    where
        F: FnMut(&DdlStatement) -> Result<(), String>,
    {
        let mut results = Vec::with_capacity(ddls.len());
        let mut success_count = 0;
        let mut failure_count = 0;

        for ddl in ddls {
            let result = executor(&ddl);
            match &result {
                Ok(()) => success_count += 1,
                Err(_) => failure_count += 1,
            }
            results.push((ddl, result));
        }

        MigrationResult {
            ddl_count: results.len(),
            results,
            success_count,
            failure_count,
        }
    }

    /// 一站式迁移：比对 + 生成 + 执行
    pub fn migrate<F>(
        &self,
        source: &[TableSchema],
        target: &[TableSchema],
        executor: F,
    ) -> MigrationResult
    where
        F: FnMut(&DdlStatement) -> Result<(), String>,
    {
        let (_, ddls) = self.plan(source, target);
        self.execute(ddls, executor)
    }

    /// 获取 DDL 生成器
    pub fn generator(&self) -> &DdlGenerator {
        &self.generator
    }
}

// =====================================================================
// 辅助函数 — 类型映射
// =====================================================================

/// PG 类型映射
fn pg_type_name(dt: DataType) -> &'static str {
    match dt {
        DataType::Int32 => "INTEGER",
        DataType::Int64 => "BIGINT",
        DataType::Text => "TEXT",
        DataType::Blob => "BYTEA",
        DataType::Real => "DOUBLE PRECISION",
        DataType::Bool => "BOOLEAN",
        DataType::Date => "DATE",
        DataType::Timestamp => "TIMESTAMP",
        DataType::Json => "JSONB",
        DataType::Uuid => "UUID",
    }
}

/// MySQL 类型映射
fn mysql_type_name(dt: DataType) -> &'static str {
    match dt {
        DataType::Int32 => "INT",
        DataType::Int64 => "BIGINT",
        DataType::Text => "TEXT",
        DataType::Blob => "BLOB",
        DataType::Real => "DOUBLE",
        DataType::Bool => "TINYINT(1)",
        DataType::Date => "DATE",
        DataType::Timestamp => "DATETIME",
        DataType::Json => "JSON",
        DataType::Uuid => "CHAR(36)",
    }
}

/// SQLite 类型映射
fn sqlite_type_name(dt: DataType) -> &'static str {
    match dt {
        DataType::Int32 => "INTEGER",
        DataType::Int64 => "INTEGER",
        DataType::Text => "TEXT",
        DataType::Blob => "BLOB",
        DataType::Real => "REAL",
        DataType::Bool => "INTEGER",
        DataType::Date => "TEXT",
        DataType::Timestamp => "TEXT",
        DataType::Json => "TEXT",
        DataType::Uuid => "TEXT",
    }
}

/// Oracle 类型映射
fn oracle_type_name(dt: DataType) -> &'static str {
    match dt {
        DataType::Int32 => "NUMBER(10)",
        DataType::Int64 => "NUMBER(19)",
        DataType::Text => "CLOB",
        DataType::Blob => "BLOB",
        DataType::Real => "BINARY_DOUBLE",
        DataType::Bool => "NUMBER(1)",
        DataType::Date => "DATE",
        DataType::Timestamp => "TIMESTAMP",
        DataType::Json => "CLOB",
        DataType::Uuid => "CHAR(36)",
    }
}

/// SQL Server 类型映射
fn sqlserver_type_name(dt: DataType) -> &'static str {
    match dt {
        DataType::Int32 => "INT",
        DataType::Int64 => "BIGINT",
        DataType::Text => "NVARCHAR(MAX)",
        DataType::Blob => "VARBINARY(MAX)",
        DataType::Real => "FLOAT(53)",
        DataType::Bool => "BIT",
        DataType::Date => "DATE",
        DataType::Timestamp => "DATETIME2",
        DataType::Json => "NVARCHAR(MAX)",
        DataType::Uuid => "UNIQUEIDENTIFIER",
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_schema(table_id: u32, name: &str, columns: Vec<ColumnDef>) -> TableSchema {
        TableSchema {
            table_id,
            table_name: name.to_string(),
            columns,
            version: 1,
        }
    }

    fn make_users_schema() -> TableSchema {
        make_schema(
            1,
            "users",
            vec![
                ColumnDef::not_null("id", DataType::Int64),
                ColumnDef::nullable("name", DataType::Text),
                ColumnDef::nullable("age", DataType::Int32),
            ],
        )
    }

    #[test]
    fn dialect_from_str() {
        assert_eq!(Dialect::from_str("postgres"), Some(Dialect::Postgres));
        assert_eq!(Dialect::from_str("postgresql"), Some(Dialect::Postgres));
        assert_eq!(Dialect::from_str("pg"), Some(Dialect::Postgres));
        assert_eq!(Dialect::from_str("mysql"), Some(Dialect::MySQL));
        assert_eq!(Dialect::from_str("oracle"), Some(Dialect::Oracle));
        assert_eq!(Dialect::from_str("mssql"), Some(Dialect::SqlServer));
        assert_eq!(Dialect::from_str("sqlite"), Some(Dialect::SQLite));
        assert_eq!(Dialect::from_str("unknown"), None);
    }

    #[test]
    fn schema_diff_empty_when_identical() {
        let source = vec![make_users_schema()];
        let target = vec![make_users_schema()];
        let diff = SchemaComparer::compare(&source, &target);
        assert!(diff.is_empty());
    }

    #[test]
    fn schema_diff_source_only() {
        let source = vec![make_users_schema()];
        let target = vec![];
        let diff = SchemaComparer::compare(&source, &target);
        assert_eq!(diff.tables.len(), 1);
        assert_eq!(diff.tables[0].existence, TableExistence::SourceOnly);
    }

    #[test]
    fn schema_diff_target_only() {
        let source = vec![];
        let target = vec![make_users_schema()];
        let diff = SchemaComparer::compare(&source, &target);
        assert_eq!(diff.tables.len(), 1);
        assert_eq!(diff.tables[0].existence, TableExistence::TargetOnly);
    }

    #[test]
    fn schema_diff_add_column() {
        let source = vec![make_users_schema()];
        let target = vec![make_schema(
            1,
            "users",
            vec![
                ColumnDef::not_null("id", DataType::Int64),
                ColumnDef::nullable("name", DataType::Text),
            ],
        )];
        let diff = SchemaComparer::compare(&source, &target);
        assert_eq!(diff.tables.len(), 1);
        assert_eq!(diff.tables[0].existence, TableExistence::Both);
        assert!(diff.tables[0]
            .column_diffs
            .iter()
            .any(|d| matches!(d, ColumnDiff::AddColumn(c) if c.name == "age")));
    }

    #[test]
    fn schema_diff_type_mismatch() {
        let source = vec![make_users_schema()];
        let target = vec![make_schema(
            1,
            "users",
            vec![
                ColumnDef::not_null("id", DataType::Int32), // 类型不匹配
                ColumnDef::nullable("name", DataType::Text),
                ColumnDef::nullable("age", DataType::Int32),
            ],
        )];
        let diff = SchemaComparer::compare(&source, &target);
        assert!(diff.tables[0]
            .column_diffs
            .iter()
            .any(|d| matches!(d, ColumnDiff::TypeMismatch { .. })));
    }

    #[test]
    fn ddl_generator_create_table_postgres() {
        let gen = DdlGenerator::new(Dialect::Postgres);
        let sql = gen.generate_create_table(&make_users_schema());
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"users\""));
        assert!(sql.contains("\"id\" BIGINT NOT NULL"));
        assert!(sql.contains("\"name\" TEXT"));
        assert!(sql.contains("\"age\" INTEGER"));
    }

    #[test]
    fn ddl_generator_create_table_mysql() {
        let gen = DdlGenerator::new(Dialect::MySQL);
        let sql = gen.generate_create_table(&make_users_schema());
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS `users`"));
        assert!(sql.contains("`id` BIGINT NOT NULL"));
        assert!(sql.contains("`age` INT"));
        assert!(sql.contains("`name` TEXT"));
    }

    #[test]
    fn ddl_generator_create_table_oracle() {
        let gen = DdlGenerator::new(Dialect::Oracle);
        let sql = gen.generate_create_table(&make_users_schema());
        assert!(sql.starts_with("CREATE TABLE \"users\""));
        assert!(sql.contains("\"id\" NUMBER(19) NOT NULL"));
        assert!(sql.contains("\"name\" CLOB"));
    }

    #[test]
    fn ddl_generator_create_table_sqlserver() {
        let gen = DdlGenerator::new(Dialect::SqlServer);
        let sql = gen.generate_create_table(&make_users_schema());
        assert!(sql.contains("IF NOT EXISTS"));
        assert!(sql.contains("CREATE TABLE \"users\""));
        assert!(sql.contains("\"id\" BIGINT NOT NULL"));
        assert!(sql.contains("\"name\" NVARCHAR(MAX)"));
    }

    #[test]
    fn ddl_generator_add_column_postgres() {
        let gen = DdlGenerator::new(Dialect::Postgres);
        let col = ColumnDef::nullable("email", DataType::Text);
        let sql = gen.generate_add_column("users", &col);
        assert!(sql.contains("ALTER TABLE \"users\" ADD COLUMN \"email\" TEXT"));
    }

    #[test]
    fn ddl_generator_add_column_oracle() {
        let gen = DdlGenerator::new(Dialect::Oracle);
        let col = ColumnDef::not_null("email", DataType::Text);
        let sql = gen.generate_add_column("users", &col);
        // Oracle 用 ADD (col type) 语法
        assert!(sql.contains("ALTER TABLE \"users\" ADD"));
        assert!(sql.contains("\"email\" CLOB NOT NULL"));
    }

    #[test]
    fn ddl_generator_alter_column_type_postgres() {
        let gen = DdlGenerator::new(Dialect::Postgres);
        let col = ColumnDef::not_null("id", DataType::Int64);
        let sql = gen.generate_alter_column_type("users", &col);
        assert!(sql.contains("ALTER TABLE \"users\" ALTER COLUMN \"id\" TYPE BIGINT"));
    }

    #[test]
    fn ddl_generator_alter_column_type_mysql() {
        let gen = DdlGenerator::new(Dialect::MySQL);
        let col = ColumnDef::not_null("id", DataType::Int64);
        let sql = gen.generate_alter_column_type("users", &col);
        assert!(sql.contains("MODIFY COLUMN"));
    }

    #[test]
    fn ddl_generator_alter_column_type_sqlite_returns_comment() {
        let gen = DdlGenerator::new(Dialect::SQLite);
        let col = ColumnDef::not_null("id", DataType::Int64);
        let sql = gen.generate_alter_column_type("users", &col);
        assert!(sql.starts_with("--"));
    }

    #[test]
    fn ddl_generator_drop_table_postgres() {
        let gen = DdlGenerator::new(Dialect::Postgres);
        let sql = gen.generate_drop_table("users");
        assert!(sql.contains("DROP TABLE IF EXISTS \"users\""));
    }

    #[test]
    fn ddl_generator_drop_table_oracle() {
        let gen = DdlGenerator::new(Dialect::Oracle);
        let sql = gen.generate_drop_table("users");
        assert!(sql.contains("DROP TABLE \"users\""));
        assert!(!sql.contains("IF EXISTS"));
    }

    #[test]
    fn structure_migration_plan() {
        let migration = StructureMigration::new(Dialect::Postgres);
        let source = vec![make_users_schema()];
        let target = vec![];

        let (diff, ddls) = migration.plan(&source, &target);

        assert!(!diff.is_empty());
        assert_eq!(ddls.len(), 1);
        assert_eq!(ddls[0].kind, DdlKind::CreateTable);
        assert!(ddls[0].sql.contains("CREATE TABLE"));
    }

    #[test]
    fn structure_migration_plan_no_diff() {
        let migration = StructureMigration::new(Dialect::Postgres);
        let schema = make_users_schema();
        let source = vec![schema.clone()];
        let target = vec![schema];

        let (diff, ddls) = migration.plan(&source, &target);

        assert!(diff.is_empty());
        assert!(ddls.is_empty());
    }

    #[test]
    fn structure_migration_plan_with_alter() {
        let migration = StructureMigration::new(Dialect::Postgres);
        let source = vec![make_users_schema()];
        let target = vec![make_schema(
            1,
            "users",
            vec![
                ColumnDef::not_null("id", DataType::Int64),
                ColumnDef::nullable("name", DataType::Text),
            ],
        )];

        let (diff, ddls) = migration.plan(&source, &target);

        assert!(!diff.is_empty());
        // 应该有一个 ADD COLUMN
        assert!(ddls.iter().any(|d| d.kind == DdlKind::AddColumn));
    }

    #[test]
    fn structure_migration_execute_success() {
        let migration = StructureMigration::new(Dialect::Postgres);
        let source = vec![make_users_schema()];
        let target = vec![];

        let (_, ddls) = migration.plan(&source, &target);
        let result = migration.execute(ddls, |_ddl| Ok(()));

        assert_eq!(result.success_count, 1);
        assert_eq!(result.failure_count, 0);
    }

    #[test]
    fn structure_migration_execute_failure() {
        let migration = StructureMigration::new(Dialect::Postgres);
        let source = vec![make_users_schema()];
        let target = vec![];

        let (_, ddls) = migration.plan(&source, &target);
        let result = migration.execute(ddls, |_ddl| Err("connection refused".to_string()));

        assert_eq!(result.success_count, 0);
        assert_eq!(result.failure_count, 1);
    }

    #[test]
    fn structure_migration_migrate_one_shot() {
        let migration = StructureMigration::new(Dialect::Postgres);
        let source = vec![make_users_schema()];
        let target = vec![];

        let mut executed = Vec::new();
        let result = migration.migrate(&source, &target, |ddl| {
            executed.push(ddl.sql.clone());
            Ok(())
        });

        assert_eq!(result.success_count, 1);
        assert_eq!(executed.len(), 1);
        assert!(executed[0].contains("CREATE TABLE"));
    }

    #[test]
    fn schema_diff_diff_table_count() {
        let source = vec![make_users_schema()];
        let target = vec![];
        let diff = SchemaComparer::compare(&source, &target);
        assert_eq!(diff.diff_table_count(), 1);
    }

    #[test]
    fn type_mapping_all_dialects() {
        // 验证所有类型在所有方言下都有映射
        let dialects = [
            Dialect::Postgres,
            Dialect::MySQL,
            Dialect::SQLite,
            Dialect::Oracle,
            Dialect::SqlServer,
        ];
        let types = [
            DataType::Int32,
            DataType::Int64,
            DataType::Text,
            DataType::Blob,
            DataType::Real,
            DataType::Bool,
            DataType::Date,
            DataType::Timestamp,
            DataType::Json,
            DataType::Uuid,
        ];

        for dialect in &dialects {
            let gen = DdlGenerator::new(*dialect);
            for dt in &types {
                let col = ColumnDef::not_null("test_col", *dt);
                let sql = gen.generate_add_column("test_table", &col);
                // 至少包含列名和某些类型字符串
                assert!(sql.contains("test_col"), "dialect={dialect} dt={dt:?}");
            }
        }
    }

    #[test]
    fn structure_migration_target_only_no_drop_by_default() {
        let migration = StructureMigration::new(Dialect::Postgres);
        let source = vec![];
        let target = vec![make_users_schema()];

        let (_, ddls) = migration.plan(&source, &target);

        // 默认不删除目标端多余的表
        assert!(ddls.is_empty());
    }

    #[test]
    fn column_diff_no_diff_variant() {
        let diff = ColumnDiff::NoDiff;
        assert_eq!(diff, ColumnDiff::NoDiff);
    }
}
