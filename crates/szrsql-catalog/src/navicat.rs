//! Phase 3.18 Navicat 兼容 — pg_catalog 核心视图。
//!
//! # 设计目标
//!
//! 实现 Navicat 等数据库工具浏览/编辑所需的最小 pg_catalog 视图集合：
//! - **`pg_database`** — 数据库列表（Navicat 连接后自动列出）
//! - **`pg_namespace`** — schema 列表（public 等命名空间）
//! - **`pg_class`** — 表/索引对象（relkind 区分类型）
//! - **`pg_attribute`** — 表的列定义（attname/atttypid/attlen/attnotnull/atthasdef）
//! - **`pg_type`** — 类型定义（Navicat 显示列类型 OID 反查）
//! - **`pg_index`** — 索引详情（indrelid/indkey/indisunique/indisprimary）
//! - **`pg_constraint`** — 约束（PRIMARY KEY/UNIQUE/FOREIGN KEY/CHECK）
//! - **`pg_description`** — 注释（占位，暂返回空）
//! - **`pg_views`** — 视图列表（占位，暂返回空）
//!
//! # OID 分配策略
//!
//! 使用稳定的 OID 分配算法，确保同一对象在多次调用间 OID 一致：
//! - **`pg_namespace`**：10000 + hash(schema_name) & 0xFFFF
//! - **`pg_class`**：20000 + hash(qualified_table_name) & 0xFFFFF（表）
//! - **`pg_class`**：30000 + hash(index_name) & 0xFFFFF（索引）
//! - **`pg_type`**：固定 OID 表（见 `pg_type_oid`）
//! - **`pg_attribute`**：40000 + hash(table_oid || attnum)
//! - **`pg_constraint`**：50000 + hash(constraint_name || table_oid)
//!
//! OID 高位段隔离避免冲突，hash 采用 FNV-1a 64 位变体保证散列均匀。
//!
//! # 与 PG 的差异
//!
//! - 无持久化 OID：每次从 Catalog 实时计算，重启后 OID 不变（hash 稳定）
//! - `pg_type` 仅包含 SzRSQL 支持的 9 种核心类型，不含 PG 数百种扩展类型
//! - `pg_description` 暂返回空（SzRSQL 当前不支持 COMMENT ON）
//! - `pg_views` 暂返回空（SzRSQL 当前不支持 CREATE VIEW）
//! - 不包含 `pg_authid`/`pg_proc`/`pg_trigger` 等 Navicat 非必需的视图
//! - 约束仅覆盖列级（PRIMARY KEY/UNIQUE/CHECK/REFERENCES），
//!   表级约束在 `ManagedCatalog` 中未存储到 `TableSchema`，故不可见
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.18。

use crate::{IndexInfo, MutableCatalog};
use szrsql_sql::ast::{ColumnDefinition, ForeignKeyReference, TableName};
use szrsql_sql::plan::TableSchema;
use szrsql_types::value::{ColumnType, Value};

/// 系统表行 — 与 `system_tables::SysRow` 同类型
pub type SysRow = Vec<Value>;

// =====================================================================
//  FNV-1a 64 位 hash — 稳定 OID 生成
// =====================================================================

/// FNV-1a 64 位 hash — 用于稳定 OID 生成
///
/// 选择 FNV-1a 而非 SipHash：FNV-1a 实现简单、无需 std::DefaultHasher、
/// 跨进程/跨版本结果稳定（SipHash 受 RandomState 影响，不适合 OID 生成）
fn fnv1a_64(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 计算 schema 名（None 时返回 "public"）
fn schema_name(name: &TableName) -> String {
    name.schema.clone().unwrap_or_else(|| "public".into())
}

// =====================================================================
//  OID 分配 — 稳定 hash 算法
// =====================================================================

/// pg_namespace OID — 10000 + hash(schema) & 0xFFFF
pub fn oid_namespace(schema: &str) -> i64 {
    10000 + (fnv1a_64(schema) & 0xFFFF) as i64
}

/// pg_class OID（表）— 20000 + hash(qualified) & 0xFFFFF
pub fn oid_class_table(name: &TableName) -> i64 {
    let qualified = name.qualified_name();
    20000 + (fnv1a_64(&qualified.to_lowercase()) & 0xFFFFF) as i64
}

/// pg_class OID（索引）— 30000 + hash(index_name) & 0xFFFFF
pub fn oid_class_index(index_name: &str) -> i64 {
    30000 + (fnv1a_64(&index_name.to_lowercase()) & 0xFFFFF) as i64
}

/// pg_attribute OID — 40000 + hash(table_oid || attnum) & 0xFFFFF
pub fn oid_attribute(table_oid: i64, attnum: i64) -> i64 {
    let key = format!("{table_oid}#{attnum}");
    40000 + (fnv1a_64(&key) & 0xFFFFF) as i64
}

/// pg_constraint OID — 50000 + hash(name || table_oid) & 0xFFFFF
pub fn oid_constraint(constraint_name: &str, table_oid: i64) -> i64 {
    let key = format!("{constraint_name}#{table_oid}");
    50000 + (fnv1a_64(&key.to_lowercase()) & 0xFFFFF) as i64
}

// =====================================================================
//  pg_type — 固定类型表
// =====================================================================

/// PG 内置类型 OID（与 PG 14+ 一致）
///
/// 参考：https://www.postgresql.org/docs/current/catalog-pg-type.html
pub mod pg_type_oid {
    pub const BOOL: i64 = 16;
    pub const INT8: i64 = 20; // BIGINT
    pub const INT4: i64 = 23; // INTEGER
    pub const TEXT: i64 = 25;
    pub const FLOAT8: i64 = 701;
    pub const VARCHAR: i64 = 1043;
    pub const DATE: i64 = 1082;
    pub const TIMESTAMP: i64 = 1114;
    pub const NUMERIC: i64 = 1700;
}

/// SzRSQL ColumnType → PG type OID
pub fn column_type_to_oid(ct: &ColumnType) -> i64 {
    match ct {
        ColumnType::Int64 => pg_type_oid::INT8,
        ColumnType::Float64 => pg_type_oid::FLOAT8,
        ColumnType::Text => pg_type_oid::TEXT,
        ColumnType::Bool => pg_type_oid::BOOL,
        ColumnType::Date => pg_type_oid::DATE,
        ColumnType::Timestamp => pg_type_oid::TIMESTAMP,
        ColumnType::Decimal { .. } => pg_type_oid::NUMERIC,
        ColumnType::Enum(_) => pg_type_oid::TEXT, // Enum 暂映射 TEXT
        ColumnType::Null => pg_type_oid::TEXT,    // Null 暂映射 TEXT
        ColumnType::Blob => pg_type_oid::TEXT,    // Blob 暂映射 TEXT（PG bytea 暂未支持）
        ColumnType::Array(_) => pg_type_oid::TEXT, // Array 暂映射 TEXT
        ColumnType::Range(_) => pg_type_oid::TEXT, // Range 暂映射 TEXT
        ColumnType::Json => pg_type_oid::TEXT,    // Json 暂映射 TEXT（PG json OID 114 暂未暴露）
        ColumnType::TsVector => pg_type_oid::TEXT, // Phase 3.33: tsvector 暂映射 TEXT
        ColumnType::TsQuery => pg_type_oid::TEXT, // Phase 3.33: tsquery 暂映射 TEXT
    }
}

/// SzRSQL ColumnType → PG type name
pub fn column_type_to_name(ct: &ColumnType) -> &'static str {
    match ct {
        ColumnType::Int64 => "int8",
        ColumnType::Float64 => "float8",
        ColumnType::Text => "text",
        ColumnType::Bool => "bool",
        ColumnType::Date => "date",
        ColumnType::Timestamp => "timestamp",
        ColumnType::Decimal { .. } => "numeric",
        ColumnType::Enum(_) => "text",
        ColumnType::Null => "text",
        ColumnType::Blob => "bytea",
        ColumnType::Array(_) => "text",
        ColumnType::Range(_) => "text",
        ColumnType::Json => "json",
        ColumnType::TsVector => "tsvector",
        ColumnType::TsQuery => "tsquery",
    }
}

/// pg_type 行 — (oid, typname, typlen, typtype)
///
/// - `typlen`：固定长度类型为正数（如 int8=8），变长类型为 -1
/// - `typtype`：'b' = base type
fn make_pg_type_row(oid: i64, name: &str, typlen: i64) -> SysRow {
    vec![
        Value::Int64(oid),
        Value::Text(name.into()),
        Value::Int64(typlen),
        Value::Text("b".into()),
    ]
}

/// `pg_type` 系统表的列名
pub const PG_TYPE_COLUMNS: &[&str] = &["oid", "typname", "typlen", "typtype"];

/// `pg_type` 系统表的 Schema
pub fn pg_type_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_type"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("typname", ColumnType::Text),
            ColumnDefinition::new("typlen", ColumnType::Int64),
            ColumnDefinition::new("typtype", ColumnType::Text),
        ],
    }
}

/// 查询 `pg_type` — 返回 SzRSQL 支持的所有内置类型
pub fn pg_type() -> Vec<SysRow> {
    vec![
        make_pg_type_row(pg_type_oid::BOOL, "bool", 1),
        make_pg_type_row(pg_type_oid::INT8, "int8", 8),
        make_pg_type_row(pg_type_oid::INT4, "int4", 4),
        make_pg_type_row(pg_type_oid::TEXT, "text", -1),
        make_pg_type_row(pg_type_oid::FLOAT8, "float8", 8),
        make_pg_type_row(pg_type_oid::VARCHAR, "varchar", -1),
        make_pg_type_row(pg_type_oid::DATE, "date", 4),
        make_pg_type_row(pg_type_oid::TIMESTAMP, "timestamp", 8),
        make_pg_type_row(pg_type_oid::NUMERIC, "numeric", -1),
    ]
}

// =====================================================================
//  pg_database — 数据库列表（Navicat 连接后列出）
// =====================================================================

/// `pg_database` 系统表的列名
///
/// 列顺序：(oid, datname, datallowconn, datistemplate)
pub const PG_DATABASE_COLUMNS: &[&str] = &["oid", "datname", "datallowconn", "datistemplate"];

/// `pg_database` 系统表的 Schema
pub fn pg_database_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_database"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("datname", ColumnType::Text),
            ColumnDefinition::new("datallowconn", ColumnType::Bool),
            ColumnDefinition::new("datistemplate", ColumnType::Bool),
        ],
    }
}

/// 查询 `pg_database` — 返回当前数据库
///
/// SzRSQL 当前为单数据库实例，返回固定的 `szrsql` 数据库。
/// Phase 4 pgwire 集成时可通过参数注入实际数据库名。
pub fn pg_database(current_db: &str) -> Vec<SysRow> {
    // 模板数据库：template1（PG 兼容，Navicat 期望存在）
    vec![
        vec![
            Value::Int64(1),
            Value::Text("template1".into()),
            Value::Bool(false),
            Value::Bool(true),
        ],
        vec![
            Value::Int64(16384),
            Value::Text(current_db.into()),
            Value::Bool(true),
            Value::Bool(false),
        ],
    ]
}

// =====================================================================
//  pg_namespace — schema 列表
// =====================================================================

/// `pg_namespace` 系统表的列名
///
/// 列顺序：(oid, nspname, nspowner)
pub const PG_NAMESPACE_COLUMNS: &[&str] = &["oid", "nspname", "nspowner"];

/// `pg_namespace` 系统表的 Schema
pub fn pg_namespace_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_namespace"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("nspname", ColumnType::Text),
            ColumnDefinition::new("nspowner", ColumnType::Int64),
        ],
    }
}

/// 查询 `pg_namespace` — 返回所有 schema
///
/// 默认包含：
/// - `pg_catalog`（OID 11，PG 系统目录 schema）
/// - `public`（OID 2200，PG 默认用户 schema）
/// - `information_schema`（OID 13078，SQL 标准 schema）
/// - 用户自定义 schema（从 Catalog 中表的 schema 字段提取）
pub fn pg_namespace(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    let mut rows = vec![
        vec![
            Value::Int64(11),
            Value::Text("pg_catalog".into()),
            Value::Int64(10),
        ],
        vec![
            Value::Int64(2200),
            Value::Text("public".into()),
            Value::Int64(10),
        ],
        vec![
            Value::Int64(13078),
            Value::Text("information_schema".into()),
            Value::Int64(10),
        ],
    ];

    // 从 catalog 中提取所有用户 schema（去重）
    let mut user_schemas: Vec<String> = Vec::new();
    for name in catalog.list_tables() {
        let s = schema_name(&name);
        if s != "public"
            && s != "pg_catalog"
            && s != "information_schema"
            && !user_schemas.contains(&s)
        {
            user_schemas.push(s);
        }
    }

    for s in user_schemas {
        let oid = oid_namespace(&s);
        rows.push(vec![Value::Int64(oid), Value::Text(s), Value::Int64(10)]);
    }

    rows
}

// =====================================================================
//  pg_class — 表/索引对象
// =====================================================================

/// pg_class relkind 枚举（与 PG 一致）
pub mod relkind {
    /// 普通表
    pub const RELATION: &str = "r";
    /// 索引
    pub const INDEX: &str = "i";
    /// 视图（占位）
    pub const VIEW: &str = "v";
    /// 序列（占位）
    pub const SEQUENCE: &str = "S";
}

/// `pg_class` 系统表的列名
///
/// 列顺序：(oid, relname, relnamespace, relkind, relnatts, relpages, reltuples)
pub const PG_CLASS_COLUMNS: &[&str] = &[
    "oid",
    "relname",
    "relnamespace",
    "relkind",
    "relnatts",
    "relpages",
    "reltuples",
];

/// `pg_class` 系统表的 Schema
pub fn pg_class_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_class"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("relname", ColumnType::Text),
            ColumnDefinition::new("relnamespace", ColumnType::Int64),
            ColumnDefinition::new("relkind", ColumnType::Text),
            ColumnDefinition::new("relnatts", ColumnType::Int64),
            ColumnDefinition::new("relpages", ColumnType::Int64),
            ColumnDefinition::new("reltuples", ColumnType::Float64),
        ],
    }
}

/// 查询 `pg_class` — 返回所有表 + 索引对象
pub fn pg_class(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    let mut rows = Vec::new();

    // 表对象
    for name in catalog.list_tables() {
        let schema = schema_name(&name);
        let ns_oid = oid_namespace(&schema);
        let class_oid = oid_class_table(&name);
        let relnatts = catalog
            .get_table(&name)
            .map(|s| s.columns.len() as i64)
            .unwrap_or(0);
        rows.push(vec![
            Value::Int64(class_oid),
            Value::Text(name.name.clone()),
            Value::Int64(ns_oid),
            Value::Text(relkind::RELATION.into()),
            Value::Int64(relnatts),
            Value::Int64(0),     // relpages：实际页数未知
            Value::Float64(0.0), // reltuples：实际行数未知
        ]);
    }

    // 索引对象
    for idx in MutableCatalog::list_indexes(catalog) {
        let schema = schema_name(&idx.table);
        let ns_oid = oid_namespace(&schema);
        let class_oid = oid_class_index(&idx.name);
        rows.push(vec![
            Value::Int64(class_oid),
            Value::Text(idx.name.clone()),
            Value::Int64(ns_oid),
            Value::Text(relkind::INDEX.into()),
            Value::Int64(idx.columns.len() as i64),
            Value::Int64(0),
            Value::Float64(0.0),
        ]);
    }

    rows
}

// =====================================================================
//  pg_attribute — 表的列定义
// =====================================================================

/// `pg_attribute` 系统表的列名
///
/// 列顺序：(oid, attrelid, attname, atttypid, attlen, attnotnull, atthasdef, attnum)
pub const PG_ATTRIBUTE_COLUMNS: &[&str] = &[
    "oid",
    "attrelid",
    "attname",
    "atttypid",
    "attlen",
    "attnotnull",
    "atthasdef",
    "attnum",
];

/// `pg_attribute` 系统表的 Schema
pub fn pg_attribute_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_attribute"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("attrelid", ColumnType::Int64),
            ColumnDefinition::new("attname", ColumnType::Text),
            ColumnDefinition::new("atttypid", ColumnType::Int64),
            ColumnDefinition::new("attlen", ColumnType::Int64),
            ColumnDefinition::new("attnotnull", ColumnType::Bool),
            ColumnDefinition::new("atthasdef", ColumnType::Bool),
            ColumnDefinition::new("attnum", ColumnType::Int64),
        ],
    }
}

/// 查询 `pg_attribute` — 返回所有表的所有列
pub fn pg_attribute(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    let mut rows = Vec::new();
    for name in catalog.list_tables() {
        let table_oid = oid_class_table(&name);
        let schema = match catalog.get_table(&name) {
            Some(s) => s,
            None => continue,
        };
        for (idx, col) in schema.columns.iter().enumerate() {
            let typoid = column_type_to_oid(&col.data_type);
            let typlen = pg_type_len(typoid);
            let attnum = (idx as i64) + 1; // PG attnum 从 1 开始
            let att_oid = oid_attribute(table_oid, attnum);
            rows.push(vec![
                Value::Int64(att_oid),
                Value::Int64(table_oid),
                Value::Text(col.name.clone()),
                Value::Int64(typoid),
                Value::Int64(typlen),
                Value::Bool(col.not_null || col.primary_key),
                Value::Bool(col.default.is_some()),
                Value::Int64(attnum),
            ]);
        }
    }
    rows
}

/// PG type OID → typlen（固定长度为正，变长为 -1）
fn pg_type_len(oid: i64) -> i64 {
    match oid {
        pg_type_oid::BOOL => 1,
        pg_type_oid::INT8 => 8,
        pg_type_oid::INT4 => 4,
        pg_type_oid::TEXT | pg_type_oid::VARCHAR => -1,
        pg_type_oid::FLOAT8 => 8,
        pg_type_oid::DATE => 4,
        pg_type_oid::TIMESTAMP => 8,
        pg_type_oid::NUMERIC => -1,
        _ => -1,
    }
}

// =====================================================================
//  pg_index — 索引详情
// =====================================================================

/// `pg_index` 系统表的列名
///
/// 列顺序：(indexrelid, indrelid, indkey, indisunique, indisprimary, indnatts)
pub const PG_INDEX_COLUMNS: &[&str] = &[
    "indexrelid",
    "indrelid",
    "indkey",
    "indisunique",
    "indisprimary",
    "indnatts",
];

/// `pg_index` 系统表的 Schema
pub fn pg_index_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_index"),
        columns: vec![
            ColumnDefinition::new("indexrelid", ColumnType::Int64),
            ColumnDefinition::new("indrelid", ColumnType::Int64),
            ColumnDefinition::new("indkey", ColumnType::Text), // PG int2vector，简化为文本
            ColumnDefinition::new("indisunique", ColumnType::Bool),
            ColumnDefinition::new("indisprimary", ColumnType::Bool),
            ColumnDefinition::new("indnatts", ColumnType::Int64),
        ],
    }
}

/// 查询 `pg_index` — 返回所有索引详情
///
/// `indkey` 格式：列号空格分隔（PG int2vector 文本表示），如 "1 2" 表示第 1、2 列
/// `indisprimary`：UNIQUE 索引且列为主键时为 true
pub fn pg_index(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    let mut rows = Vec::new();
    for idx in MutableCatalog::list_indexes(catalog) {
        let index_oid = oid_class_index(&idx.name);
        let table_oid = oid_class_table(&idx.table);

        // 计算 indkey：列在表中的位置（1-indexed）
        let indkey = if let Some(schema) = catalog.get_table(&idx.table) {
            let keys: Vec<String> = idx
                .columns
                .iter()
                .map(|c| {
                    schema
                        .columns
                        .iter()
                        .position(|col| col.name.eq_ignore_ascii_case(&c.column))
                        .map(|p| (p + 1).to_string())
                        .unwrap_or_else(|| "0".into())
                })
                .collect();
            keys.join(" ")
        } else {
            String::new()
        };

        let is_primary = is_primary_key_index(catalog, &idx);

        rows.push(vec![
            Value::Int64(index_oid),
            Value::Int64(table_oid),
            Value::Text(indkey),
            Value::Bool(idx.unique),
            Value::Bool(is_primary),
            Value::Int64(idx.columns.len() as i64),
        ]);
    }
    rows
}

/// 判断索引是否为主键索引
///
/// 规则：UNIQUE 索引 + 索引列与表级 PRIMARY KEY 约束列完全一致。
/// SzRSQL 的 `TableSchema` 不存储表级约束，故仅检查列级 PRIMARY KEY。
fn is_primary_key_index(catalog: &dyn MutableCatalog, idx: &IndexInfo) -> bool {
    if !idx.unique {
        return false;
    }
    let schema = match catalog.get_table(&idx.table) {
        Some(s) => s,
        None => return false,
    };
    // 列级 PRIMARY KEY
    let col_pk: Vec<&str> = schema
        .columns
        .iter()
        .filter(|c| c.primary_key)
        .map(|c| c.name.as_str())
        .collect();
    if col_pk.is_empty() {
        return false;
    }
    let idx_cols: Vec<&str> = idx.columns.iter().map(|c| c.column.as_str()).collect();
    col_pk == idx_cols
}

// =====================================================================
//  pg_constraint — 约束
// =====================================================================

/// pg_constraint contype 枚举（与 PG 一致）
pub mod contype {
    /// CHECK
    pub const CHECK: &str = "c";
    /// FOREIGN KEY
    pub const FOREIGN_KEY: &str = "f";
    /// PRIMARY KEY
    pub const PRIMARY_KEY: &str = "p";
    /// UNIQUE
    pub const UNIQUE: &str = "u";
}

/// `pg_constraint` 系统表的列名
///
/// 列顺序：(oid, conname, conrelid, contype, conkey)
pub const PG_CONSTRAINT_COLUMNS: &[&str] = &["oid", "conname", "conrelid", "contype", "conkey"];

/// `pg_constraint` 系统表的 Schema
pub fn pg_constraint_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_constraint"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("conname", ColumnType::Text),
            ColumnDefinition::new("conrelid", ColumnType::Int64),
            ColumnDefinition::new("contype", ColumnType::Text),
            ColumnDefinition::new("conkey", ColumnType::Text), // int2[] 简化为文本
        ],
    }
}

/// 查询 `pg_constraint` — 返回所有约束
///
/// 来源（仅列级约束 — 表级约束未存储到 `TableSchema`）：
/// - PRIMARY KEY：列级 `primary_key=true` 合并为单条约束
/// - UNIQUE：列级 `unique=true`
/// - CHECK：列级 `check=Some`
/// - FOREIGN KEY：列级 `references=Some`
pub fn pg_constraint(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    let mut rows = Vec::new();
    for name in catalog.list_tables() {
        let table_oid = oid_class_table(&name);
        let schema = match catalog.get_table(&name) {
            Some(s) => s,
            None => continue,
        };

        // 列级 PRIMARY KEY：合并为单条约束（多列 PK 时按 PG 风格命名）
        let col_pks: Vec<&str> = schema
            .columns
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| c.name.as_str())
            .collect();
        if !col_pks.is_empty() {
            let conname = format!("{}_pkey", name.name);
            let conkey = col_pks
                .iter()
                .map(|c| column_attnum(&schema, c).to_string())
                .collect::<Vec<_>>()
                .join(" ");
            rows.push(make_constraint_row(
                &conname,
                table_oid,
                contype::PRIMARY_KEY,
                &conkey,
            ));
        }

        // 列级 UNIQUE（排除已为 PK 的列）
        let col_uniques: Vec<&str> = schema
            .columns
            .iter()
            .filter(|c| c.unique && !c.primary_key)
            .map(|c| c.name.as_str())
            .collect();
        for col in col_uniques {
            let conname = format!("{}_{}_key", name.name, col);
            let conkey = column_attnum(&schema, col).to_string();
            rows.push(make_constraint_row(
                &conname,
                table_oid,
                contype::UNIQUE,
                &conkey,
            ));
        }

        // 列级 CHECK
        for (idx, col) in schema.columns.iter().enumerate() {
            if col.check.is_some() {
                let conname = format!("{}_{}_check", name.name, col.name);
                let conkey = (idx + 1).to_string();
                rows.push(make_constraint_row(
                    &conname,
                    table_oid,
                    contype::CHECK,
                    &conkey,
                ));
            }
        }

        // 列级 FOREIGN KEY
        for (idx, col) in schema.columns.iter().enumerate() {
            if col.references.is_some() {
                let conname = format!("{}_{}_fkey", name.name, col.name);
                let conkey = (idx + 1).to_string();
                rows.push(make_constraint_row(
                    &conname,
                    table_oid,
                    contype::FOREIGN_KEY,
                    &conkey,
                ));
            }
        }
    }
    rows
}

/// 构造 pg_constraint 单行
fn make_constraint_row(conname: &str, table_oid: i64, contype: &str, conkey: &str) -> SysRow {
    let oid = oid_constraint(conname, table_oid);
    vec![
        Value::Int64(oid),
        Value::Text(conname.into()),
        Value::Int64(table_oid),
        Value::Text(contype.into()),
        Value::Text(conkey.into()),
    ]
}

/// 查找列在表中的 attnum（1-indexed，未找到返回 0）
fn column_attnum(schema: &TableSchema, col_name: &str) -> i64 {
    schema
        .columns
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case(col_name))
        .map(|p| (p + 1) as i64)
        .unwrap_or(0)
}

// =====================================================================
//  pg_description — 注释（占位，返回空）
// =====================================================================

/// `pg_description` 系统表的列名
///
/// 列顺序：(objoid, classoid, objsubid, description)
pub const PG_DESCRIPTION_COLUMNS: &[&str] = &["objoid", "classoid", "objsubid", "description"];

/// `pg_description` 系统表的 Schema
pub fn pg_description_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_description"),
        columns: vec![
            ColumnDefinition::new("objoid", ColumnType::Int64),
            ColumnDefinition::new("classoid", ColumnType::Int64),
            ColumnDefinition::new("objsubid", ColumnType::Int64),
            ColumnDefinition::new("description", ColumnType::Text),
        ],
    }
}

/// 查询 `pg_description` — SzRSQL 当前不支持 COMMENT ON，返回空
pub fn pg_description() -> Vec<SysRow> {
    Vec::new()
}

// =====================================================================
//  pg_views — 视图列表（占位，返回空）
// =====================================================================

/// `pg_views` 系统表的列名
///
/// 列顺序：(schemaname, viewname, viewowner, definition)
pub const PG_VIEWS_COLUMNS: &[&str] = &["schemaname", "viewname", "viewowner", "definition"];

/// `pg_views` 系统表的 Schema
pub fn pg_views_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_views"),
        columns: vec![
            ColumnDefinition::new("schemaname", ColumnType::Text),
            ColumnDefinition::new("viewname", ColumnType::Text),
            ColumnDefinition::new("viewowner", ColumnType::Text),
            ColumnDefinition::new("definition", ColumnType::Text),
        ],
    }
}

/// 查询 `pg_views` — SzRSQL 当前不支持 CREATE VIEW，返回空
pub fn pg_views() -> Vec<SysRow> {
    Vec::new()
}

// =====================================================================
//  Navicat 兼容性辅助 — 列类型显示格式
// =====================================================================

/// 返回 Navicat 风格的列类型显示字符串
///
/// 示例：
/// - `Int64` → "bigint"
/// - `Text` → "text"
/// - `Decimal { precision: 10, scale: 2 }` → "numeric(10,2)"
/// - `Enum(["a","b"])` → "text"（Enum 暂映射 text）
pub fn column_type_display(ct: &ColumnType) -> String {
    match ct {
        ColumnType::Int64 => "bigint".into(),
        ColumnType::Float64 => "double precision".into(),
        ColumnType::Text => "text".into(),
        ColumnType::Bool => "boolean".into(),
        ColumnType::Date => "date".into(),
        ColumnType::Timestamp => "timestamp without time zone".into(),
        ColumnType::Decimal { precision, scale } => {
            format!("numeric({precision},{scale})")
        }
        ColumnType::Enum(_) => "text".into(),
        ColumnType::Null => "text".into(),
        ColumnType::Blob => "bytea".into(),
        ColumnType::Array(_) => "text".into(),
        ColumnType::Range(_) => "text".into(),
        ColumnType::Json => "json".into(),
        ColumnType::TsVector => "tsvector".into(),
        ColumnType::TsQuery => "tsquery".into(),
    }
}

/// 返回 Navicat 风格的列 DDL 片段
///
/// 示例：`id bigint NOT NULL PRIMARY KEY` / `name text` / `price numeric(10,2) DEFAULT 0`
pub fn column_ddl_fragment(col: &ColumnDefinition) -> String {
    let mut s = format!("{} {}", col.name, column_type_display(&col.data_type));
    if col.not_null {
        s.push_str(" NOT NULL");
    }
    if col.primary_key {
        s.push_str(" PRIMARY KEY");
    }
    if col.unique && !col.primary_key {
        s.push_str(" UNIQUE");
    }
    // DEFAULT 表达式暂不输出（需 Expr → SQL 反序列化，留待未来实现）
    s
}

/// 反向引用 ForeignKeyReference（用于 DDL 输出）
pub fn foreign_key_reference_ddl(fk: &ForeignKeyReference) -> String {
    let cols = fk
        .columns
        .as_ref()
        .map(|cs| cs.join(","))
        .unwrap_or_else(|| "id".into());
    let mut s = format!("REFERENCES {} ({})", fk.table.qualified_name(), cols);
    if let Some(action) = &fk.on_delete {
        s.push_str(&format!(" ON DELETE {}", action_str(action)));
    }
    if let Some(action) = &fk.on_update {
        s.push_str(&format!(" ON UPDATE {}", action_str(action)));
    }
    s
}

/// ReferenceAction → 字符串
fn action_str(action: &szrsql_sql::ast::ReferenceAction) -> &'static str {
    use szrsql_sql::ast::ReferenceAction;
    match action {
        ReferenceAction::Cascade => "CASCADE",
        ReferenceAction::Restrict => "RESTRICT",
        ReferenceAction::SetNull => "SET NULL",
        ReferenceAction::SetDefault => "SET DEFAULT",
        ReferenceAction::NoAction => "NO ACTION",
    }
}
