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
//! - **`pg_description`** — 注释（L9：从 catalog.comments 真实查询，非占位）
//! - **`pg_views`** — 视图列表（L9：从 catalog.views 真实查询，definition 字段为元数据摘要）
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
//! - `pg_description` 从 catalog.comments 真实查询（L9：原占位已修复）
//! - `pg_views` 从 catalog.views 真实查询（L9：原占位已修复）
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

/// pg_namespace OID — 内置 schema 用 PG 硬编码 OID，自定义 schema 用 hash
///
/// **重要**：必须与 `pg_namespace()` 函数返回的 OID 一致，否则 JOIN 失败。
/// - `pg_catalog` → 11（PG 内置）
/// - `public` → 2200（PG 默认用户 schema）
/// - `information_schema` → 13078（SQL 标准 schema）
/// - 其他 → 10000 + hash(schema) & 0xFFFF
pub fn oid_namespace(schema: &str) -> i64 {
    match schema {
        "pg_catalog" => 11,
        "public" => 2200,
        "information_schema" => 13078,
        _ => 10000 + (fnv1a_64(schema) & 0xFFFF) as i64,
    }
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

/// pg_type 行 — (oid, typname, typnamespace, typlen, typtype)
///
/// - `typnamespace`：类型所属的 namespace OID（PG 2200 = public）
/// - `typlen`：固定长度类型为正数（如 int8=8），变长类型为 -1
/// - `typtype`：'b' = base type
fn make_pg_type_row(oid: i64, name: &str, typlen: i64) -> SysRow {
    vec![
        Value::Int64(oid),
        Value::Text(name.into()),
        Value::Int64(PG_NAMESPACE_PUBLIC_OID),
        Value::Int64(typlen),
        Value::Text("b".into()),
    ]
}

/// public namespace 的 OID（与 PostgreSQL 14 默认一致）
pub const PG_NAMESPACE_PUBLIC_OID: i64 = 2200;

/// `pg_type` 系统表的列名
pub const PG_TYPE_COLUMNS: &[&str] = &["oid", "typname", "typnamespace", "typlen", "typtype"];

/// `pg_type` 系统表的 Schema
pub fn pg_type_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_type"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("typname", ColumnType::Text),
            ColumnDefinition::new("typnamespace", ColumnType::Int64),
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
/// 列顺序与 PostgreSQL 14 的 pg_database 完全一致：
/// (oid, datname, datdba, encoding, datcollate, datctype, datistemplate,
///  datallowconn, datconnlimit, datlastsysoid, datfrozenxid, datminmxid,
///  dattablespace, datacl)
pub const PG_DATABASE_COLUMNS: &[&str] = &[
    "oid",
    "datname",
    "datdba",
    "encoding",
    "datcollate",
    "datctype",
    "datistemplate",
    "datallowconn",
    "datconnlimit",
    "datlastsysoid",
    "datfrozenxid",
    "datminmxid",
    "dattablespace",
    "datacl",
];

/// `pg_database` 系统表的 Schema
///
/// 与 PostgreSQL 14 的 pg_database 表结构完全一致（14 列）。
pub fn pg_database_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_database"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("datname", ColumnType::Text),
            ColumnDefinition::new("datdba", ColumnType::Int64),
            ColumnDefinition::new("encoding", ColumnType::Int64),
            ColumnDefinition::new("datcollate", ColumnType::Text),
            ColumnDefinition::new("datctype", ColumnType::Text),
            ColumnDefinition::new("datistemplate", ColumnType::Bool),
            ColumnDefinition::new("datallowconn", ColumnType::Bool),
            ColumnDefinition::new("datconnlimit", ColumnType::Int64),
            ColumnDefinition::new("datlastsysoid", ColumnType::Int64),
            ColumnDefinition::new("datfrozenxid", ColumnType::Int64),
            ColumnDefinition::new("datminmxid", ColumnType::Int64),
            ColumnDefinition::new("dattablespace", ColumnType::Int64),
            ColumnDefinition::new("datacl", ColumnType::Array(Box::new(ColumnType::Text))),
        ],
    }
}

/// 查询 `pg_database` — 返回当前数据库
///
/// SzRSQL 当前为单数据库实例，返回固定的 `szrsql` 数据库。
/// Phase 4 pgwire 集成时可通过参数注入实际数据库名。
///
/// 返回行与 PG_DATABASE_COLUMNS 列顺序一致（14 列）。
pub fn pg_database(current_db: &str) -> Vec<SysRow> {
    // 模板数据库：template1（PG 兼容，Navicat 期望存在）
    vec![
        vec![
            Value::Int64(1),                      // oid
            Value::Text("template1".into()),      // datname
            Value::Int64(10),                     // datdba
            Value::Int64(6),                      // encoding (UTF8)
            Value::Text("C".into()),              // datcollate
            Value::Text("C".into()),              // datctype
            Value::Bool(true),                    // datistemplate
            Value::Bool(false),                   // datallowconn
            Value::Int64(-1),                     // datconnlimit
            Value::Int64(1255),                   // datlastsysoid
            Value::Int64(722),                    // datfrozenxid
            Value::Int64(722),                    // datminmxid
            Value::Int64(1663),                   // dattablespace
            Value::Array(vec![]),                 // datacl
        ],
        vec![
            Value::Int64(16384),                  // oid
            Value::Text(current_db.into()),       // datname
            Value::Int64(10),                     // datdba
            Value::Int64(6),                      // encoding (UTF8)
            Value::Text("C".into()),              // datcollate
            Value::Text("C".into()),              // datctype
            Value::Bool(false),                   // datistemplate
            Value::Bool(true),                    // datallowconn
            Value::Int64(-1),                     // datconnlimit
            Value::Int64(1255),                   // datlastsysoid
            Value::Int64(722),                    // datfrozenxid
            Value::Int64(722),                    // datminmxid
            Value::Int64(1663),                   // dattablespace
            Value::Array(vec![]),                 // datacl
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
/// 列顺序与 PostgreSQL 14 的 pg_class 一致（35 列）：
/// (oid, relname, relnamespace, reltype, reloftype, relowner, relam,
///  relfilenode, reltablespace, relpages, reltuples, relallvisible,
///  reltoastrelid, relhasindex, relisshared, relpersistence, relkind,
///  relnatts, relchecks, relhasrules, relhastriggers, relhassubclass,
///  relrowsecurity, relforcerowsecurity, relispartition, relrewrite,
///  relfrozenxid, relminmxid, relacl, reloptions, relpartbound)
pub const PG_CLASS_COLUMNS: &[&str] = &[
    "oid",
    "relname",
    "relnamespace",
    "reltype",
    "reloftype",
    "relowner",
    "relam",
    "relfilenode",
    "reltablespace",
    "relpages",
    "reltuples",
    "relallvisible",
    "reltoastrelid",
    "relhasindex",
    "relisshared",
    "relpersistence",
    "relkind",
    "relnatts",
    "relchecks",
    "relhasrules",
    "relhastriggers",
    "relhassubclass",
    "relrowsecurity",
    "relforcerowsecurity",
    "relispartition",
    "relrewrite",
    "relfrozenxid",
    "relminmxid",
    "relacl",
    "reloptions",
    "relpartbound",
];

/// `pg_class` 系统表的 Schema
///
/// 与 PostgreSQL 14 的 pg_class 表结构完全一致（31 列）。
pub fn pg_class_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_class"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("relname", ColumnType::Text),
            ColumnDefinition::new("relnamespace", ColumnType::Int64),
            ColumnDefinition::new("reltype", ColumnType::Int64),
            ColumnDefinition::new("reloftype", ColumnType::Int64),
            ColumnDefinition::new("relowner", ColumnType::Int64),
            ColumnDefinition::new("relam", ColumnType::Int64),
            ColumnDefinition::new("relfilenode", ColumnType::Int64),
            ColumnDefinition::new("reltablespace", ColumnType::Int64),
            ColumnDefinition::new("relpages", ColumnType::Int64),
            ColumnDefinition::new("reltuples", ColumnType::Float64),
            ColumnDefinition::new("relallvisible", ColumnType::Int64),
            ColumnDefinition::new("reltoastrelid", ColumnType::Int64),
            ColumnDefinition::new("relhasindex", ColumnType::Bool),
            ColumnDefinition::new("relisshared", ColumnType::Bool),
            ColumnDefinition::new("relpersistence", ColumnType::Text),
            ColumnDefinition::new("relkind", ColumnType::Text),
            ColumnDefinition::new("relnatts", ColumnType::Int64),
            ColumnDefinition::new("relchecks", ColumnType::Int64),
            ColumnDefinition::new("relhasrules", ColumnType::Bool),
            ColumnDefinition::new("relhastriggers", ColumnType::Bool),
            ColumnDefinition::new("relhassubclass", ColumnType::Bool),
            ColumnDefinition::new("relrowsecurity", ColumnType::Bool),
            ColumnDefinition::new("relforcerowsecurity", ColumnType::Bool),
            ColumnDefinition::new("relispartition", ColumnType::Bool),
            ColumnDefinition::new("relrewrite", ColumnType::Int64),
            ColumnDefinition::new("relfrozenxid", ColumnType::Int64),
            ColumnDefinition::new("relminmxid", ColumnType::Int64),
            ColumnDefinition::new("relacl", ColumnType::Array(Box::new(ColumnType::Text))),
            ColumnDefinition::new("reloptions", ColumnType::Array(Box::new(ColumnType::Text))),
            ColumnDefinition::new("relpartbound", ColumnType::Blob),
        ],
    }
}

/// 查询 `pg_class` — 返回所有表 + 索引对象
///
/// 返回行与 PG_CLASS_COLUMNS 列顺序一致（31 列）。
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
        rows.push(build_pg_class_row(
            class_oid,
            &name.name,
            ns_oid,
            relkind::RELATION,
            relnatts,
        ));
    }

    // 索引对象
    for idx in MutableCatalog::list_indexes(catalog) {
        let schema = schema_name(&idx.table);
        let ns_oid = oid_namespace(&schema);
        let class_oid = oid_class_index(&idx.name);
        rows.push(build_pg_class_row(
            class_oid,
            &idx.name,
            ns_oid,
            relkind::INDEX,
            idx.columns.len() as i64,
        ));
    }

    rows
}

/// 构建 pg_class 行（31 列）
fn build_pg_class_row(oid: i64, name: &str, ns_oid: i64, kind: &str, natts: i64) -> SysRow {
    vec![
        Value::Int64(oid),                       // oid
        Value::Text(name.into()),                // relname
        Value::Int64(ns_oid),                    // relnamespace
        Value::Int64(0),                         // reltype
        Value::Int64(0),                         // reloftype
        Value::Int64(10),                        // relowner
        Value::Int64(0),                         // relam
        Value::Int64(0),                         // relfilenode
        Value::Int64(0),                         // reltablespace
        Value::Int64(0),                         // relpages
        Value::Float64(0.0),                     // reltuples
        Value::Int64(0),                         // relallvisible
        Value::Int64(0),                         // reltoastrelid
        Value::Bool(false),                      // relhasindex
        Value::Bool(false),                      // relisshared
        Value::Text("p".into()),                 // relpersistence (p=permanent)
        Value::Text(kind.into()),                // relkind
        Value::Int64(natts),                     // relnatts
        Value::Int64(0),                         // relchecks
        Value::Bool(false),                      // relhasrules
        Value::Bool(false),                      // relhastriggers
        Value::Bool(false),                      // relhassubclass
        Value::Bool(false),                      // relrowsecurity
        Value::Bool(false),                      // relforcerowsecurity
        Value::Bool(false),                      // relispartition
        Value::Int64(0),                         // relrewrite
        Value::Int64(722),                       // relfrozenxid
        Value::Int64(722),                       // relminmxid
        Value::Array(vec![]),                    // relacl
        Value::Array(vec![]),                    // reloptions
        Value::Blob(vec![]),                     // relpartbound
    ]
}

// =====================================================================
//  pg_attribute — 表的列定义
// =====================================================================

/// `pg_attribute` 系统表的列名
///
/// 列顺序与 PostgreSQL 14 的 pg_attribute 一致（22 列）：
/// (oid, attrelid, attname, atttypid, attlen, attnotnull, atthasdef,
///  attnum, atttypmod, attbyval, attidentity, attgenerated, attisdropped,
///  attlocal, attinhcount, attcollation, attacl, attoptions, attfdwoptions,
///  attmissingval)
pub const PG_ATTRIBUTE_COLUMNS: &[&str] = &[
    "oid",
    "attrelid",
    "attname",
    "atttypid",
    "attlen",
    "attnotnull",
    "atthasdef",
    "attnum",
    "atttypmod",
    "attbyval",
    "attidentity",
    "attgenerated",
    "attisdropped",
    "attlocal",
    "attinhcount",
    "attcollation",
    "attacl",
    "attoptions",
    "attfdwoptions",
    "attmissingval",
];

/// `pg_attribute` 系统表的 Schema
///
/// 与 PostgreSQL 14 的 pg_attribute 表结构完全一致（20 列）。
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
            ColumnDefinition::new("atttypmod", ColumnType::Int64),
            ColumnDefinition::new("attbyval", ColumnType::Bool),
            ColumnDefinition::new("attidentity", ColumnType::Text),
            ColumnDefinition::new("attgenerated", ColumnType::Text),
            ColumnDefinition::new("attisdropped", ColumnType::Bool),
            ColumnDefinition::new("attlocal", ColumnType::Bool),
            ColumnDefinition::new("attinhcount", ColumnType::Int64),
            ColumnDefinition::new("attcollation", ColumnType::Int64),
            ColumnDefinition::new("attacl", ColumnType::Array(Box::new(ColumnType::Text))),
            ColumnDefinition::new("attoptions", ColumnType::Array(Box::new(ColumnType::Text))),
            ColumnDefinition::new("attfdwoptions", ColumnType::Array(Box::new(ColumnType::Text))),
            ColumnDefinition::new("attmissingval", ColumnType::Array(Box::new(ColumnType::Text))),
        ],
    }
}

/// 查询 `pg_attribute` — 返回所有表的所有列
///
/// 返回行与 PG_ATTRIBUTE_COLUMNS 列顺序一致（20 列）。
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
                Value::Int64(att_oid),                  // oid
                Value::Int64(table_oid),                // attrelid
                Value::Text(col.name.clone()),          // attname
                Value::Int64(typoid),                   // atttypid
                Value::Int64(typlen),                   // attlen
                Value::Bool(col.not_null || col.primary_key), // attnotnull
                Value::Bool(col.default.is_some()),     // atthasdef
                Value::Int64(attnum),                   // attnum
                Value::Int64(-1),                       // atttypmod
                Value::Bool(false),                     // attbyval
                Value::Text("".into()),                 // attidentity
                Value::Text("".into()),                 // attgenerated
                Value::Bool(false),                     // attisdropped
                Value::Bool(true),                      // attlocal
                Value::Int64(0),                        // attinhcount
                Value::Int64(0),                        // attcollation
                Value::Array(vec![]),                   // attacl
                Value::Array(vec![]),                   // attoptions
                Value::Array(vec![]),                   // attfdwoptions
                Value::Array(vec![]),                   // attmissingval
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
            // P0 Navicat 兼容修复：预计算的约束定义字符串
            // Navicat 通过 pg_get_constraintdef(oid) 获取主键定义以判断可编辑列
            ColumnDefinition::new("condef", ColumnType::Text),
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
            // P0 Navicat 兼容：生成 PRIMARY KEY (col1, col2) 形式的约束定义
            let pk_cols = col_pks
                .iter()
                .map(|c| format!("\"{}\"", c))
                .collect::<Vec<_>>()
                .join(", ");
            let condef = format!("PRIMARY KEY ({})", pk_cols);
            rows.push(make_constraint_row(
                &conname,
                table_oid,
                contype::PRIMARY_KEY,
                &conkey,
                &condef,
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
            let condef = format!("UNIQUE (\"{}\")", col);
            rows.push(make_constraint_row(
                &conname,
                table_oid,
                contype::UNIQUE,
                &conkey,
                &condef,
            ));
        }

        // 列级 CHECK
        for (idx, col) in schema.columns.iter().enumerate() {
            if col.check.is_some() {
                let conname = format!("{}_{}_check", name.name, col.name);
                let conkey = (idx + 1).to_string();
                let condef = format!("CHECK ({:?})", col.check.as_ref().unwrap());
                rows.push(make_constraint_row(
                    &conname,
                    table_oid,
                    contype::CHECK,
                    &conkey,
                    &condef,
                ));
            }
        }

        // 列级 FOREIGN KEY
        for (idx, col) in schema.columns.iter().enumerate() {
            if col.references.is_some() {
                let conname = format!("{}_{}_fkey", name.name, col.name);
                let conkey = (idx + 1).to_string();
                let fk_ref = col.references.as_ref().unwrap();
                let ref_cols = fk_ref.columns.as_ref()
                    .map(|cs| cs.iter().map(|c| format!("\"{}\"", c)).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default();
                let ref_part = if ref_cols.is_empty() {
                    fk_ref.table.qualified_name()
                } else {
                    format!("{}({})", fk_ref.table.qualified_name(), ref_cols)
                };
                let condef = format!("FOREIGN KEY (\"{}\") REFERENCES {}", col.name, ref_part);
                rows.push(make_constraint_row(
                    &conname,
                    table_oid,
                    contype::FOREIGN_KEY,
                    &conkey,
                    &condef,
                ));
            }
        }
    }
    rows
}

/// 构造 pg_constraint 单行
fn make_constraint_row(conname: &str, table_oid: i64, contype: &str, conkey: &str, condef: &str) -> SysRow {
    let oid = oid_constraint(conname, table_oid);
    vec![
        Value::Int64(oid),
        Value::Text(conname.into()),
        Value::Int64(table_oid),
        Value::Text(contype.into()),
        Value::Text(conkey.into()),
        Value::Text(condef.into()),
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
//  pg_description — 注释（从 catalog comments 字段实时查询）
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

/// pg_class 的 OID（PG 内置 = 1259）— 用于 pg_description.classoid
const PG_CLASS_OID: i64 = 1259;

/// 查询 `pg_description` — 遍历 catalog 中所有表和列，返回真实注释
///
/// - 表注释：objoid=表OID, classoid=1259(pg_class), objsubid=0
/// - 列注释：objoid=表OID, classoid=1259, objsubid=列序号(1-indexed)
pub fn pg_description(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    let mut rows = Vec::new();
    for name in catalog.list_tables() {
        let table_oid = oid_class_table(&name);

        // 表注释（objsubid=0 表示表级注释）
        if let Some(comment) = catalog.get_table_comment(&name) {
            rows.push(vec![
                Value::Int64(table_oid),           // objoid
                Value::Int64(PG_CLASS_OID),        // classoid
                Value::Int64(0),                   // objsubid（0=表级）
                Value::Text(comment),              // description
            ]);
        }

        // 列注释（objsubid=列序号，1-indexed）
        if let Some(schema) = catalog.get_table(&name) {
            for (idx, col) in schema.columns.iter().enumerate() {
                let attnum = (idx as i64) + 1;
                if let Some(comment) = catalog.get_column_comment(&name, &col.name) {
                    rows.push(vec![
                        Value::Int64(table_oid),           // objoid
                        Value::Int64(PG_CLASS_OID),        // classoid
                        Value::Int64(attnum),              // objsubid（列序号）
                        Value::Text(comment),              // description
                    ]);
                }
            }
        }
    }
    rows
}

// =====================================================================
//  pg_views — 视图列表（从 catalog 实时查询）
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

/// 查询 `pg_views` — 从 catalog 获取所有视图定义
///
/// - `schemaname`：视图所属 schema（None 时为 "public"）
/// - `viewname`：视图名
/// - `viewowner`：视图所有者（固定 "postgres"，SzRSQL 单用户模式）
/// - `definition`：视图查询定义（L9 修复：原为 "SELECT ..." 占位文本，
///   现根据 ViewDefinition.columns 和 materialized 标志生成更准确的表达）
pub fn pg_views(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    let mut rows = Vec::new();
    for name in catalog.list_views() {
        let schema = schema_name(&name);
        // L9 修复：原为硬编码 "SELECT ..." 占位，现根据视图元数据生成更准确文本
        // 注：完整的 Select AST → SQL 反序列化需要 fmt::Display for Select，
        // 当前仅根据 columns/materialized 标志构造表示性 SQL，确保非空且有区分度
        let definition = catalog
            .get_view(&name)
            .map(|v| {
                let view_type = if v.materialized { "MATERIALIZED VIEW" } else { "VIEW" };
                if v.columns.is_empty() {
                    format!("/* {} {} (no explicit columns) */", view_type, name.name)
                } else {
                    format!(
                        "/* {} {} columns={} */",
                        view_type,
                        name.name,
                        v.columns.join(", ")
                    )
                }
            })
            .unwrap_or_default();
        rows.push(vec![
            Value::Text(schema),                        // schemaname
            Value::Text(name.name.clone()),             // viewname
            Value::Text("postgres".into()),             // viewowner
            Value::Text(definition),                    // definition
        ]);
    }
    rows
}

// =====================================================================
//  pg_roles / pg_shadow / pg_user — 用户/角色信息（Navicat JOIN 兼容）
//
//  Navicat 列数据库时会 JOIN pg_roles 来显示数据库所有者：
//  `SELECT d.datname, r.rolname FROM pg_database d JOIN pg_roles r ON d.datdba = r.oid`
//
//  接收 `allowed_users` 参数，为每个用户生成一行。
//  第一个用户使用 OID=10（与 pg_database.datdba=10 对应，确保 JOIN 命中）。
// =====================================================================

/// `pg_roles` 系统表的列名（与 PG 14 一致，10 列）
///
/// 列顺序：(oid, rolname, rolsuper, rolinherit, rolcreaterole, rolcreatedb,
///          rolcanlogin, rolconnlimit, rolvaliduntil, rolbypassrls)
pub const PG_ROLES_COLUMNS: &[&str] = &[
    "oid",
    "rolname",
    "rolsuper",
    "rolinherit",
    "rolcreaterole",
    "rolcreatedb",
    "rolcanlogin",
    "rolconnlimit",
    "rolvaliduntil",
    "rolbypassrls",
];

/// `pg_roles` 系统表的 Schema
pub fn pg_roles_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_roles"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("rolname", ColumnType::Text),
            ColumnDefinition::new("rolsuper", ColumnType::Bool),
            ColumnDefinition::new("rolinherit", ColumnType::Bool),
            ColumnDefinition::new("rolcreaterole", ColumnType::Bool),
            ColumnDefinition::new("rolcreatedb", ColumnType::Bool),
            ColumnDefinition::new("rolcanlogin", ColumnType::Bool),
            ColumnDefinition::new("rolconnlimit", ColumnType::Int64),
            ColumnDefinition::new("rolvaliduntil", ColumnType::Timestamp),
            ColumnDefinition::new("rolbypassrls", ColumnType::Bool),
        ],
    }
}

/// 查询 `pg_roles` — 为每个允许的用户生成一行
///
/// - 第一个用户 OID=10（与 pg_database.datdba=10 对应，确保 JOIN 命中）
/// - 后续用户 OID 从 11 开始递增
/// - 默认属性：rolsuper=true, rolcreatelb=true, rolcreatedb=true, rolcanlogin=true
pub fn pg_roles(allowed_users: &[String]) -> Vec<SysRow> {
    // 无用户列表时默认返回 postgres
    let users: Vec<&str> = if allowed_users.is_empty() {
        vec!["postgres"]
    } else {
        allowed_users.iter().map(|s| s.as_str()).collect()
    };

    users
        .iter()
        .enumerate()
        .map(|(idx, user)| {
            vec![
                Value::Int64(10 + idx as i64),       // oid（postgres=10，后续递增）
                Value::Text((*user).into()),         // rolname
                Value::Bool(true),                   // rolsuper
                Value::Bool(true),                   // rolinherit
                Value::Bool(true),                   // rolcreaterole
                Value::Bool(true),                   // rolcreatedb
                Value::Bool(true),                   // rolcanlogin
                Value::Int64(-1),                    // rolconnlimit
                Value::Null,                         // rolvaliduntil
                Value::Bool(true),                   // rolbypassrls
            ]
        })
        .collect()
}

/// `pg_shadow` 系统表的列名（与 PG 14 一致，9 列）
///
/// 列顺序：(usename, usesysid, usecreatedb, usesuper, userepl,
///          usebypassrls, passwd, valuntil, useconfig)
pub const PG_SHADOW_COLUMNS: &[&str] = &[
    "usename",
    "usesysid",
    "usecreatedb",
    "usesuper",
    "userepl",
    "usebypassrls",
    "passwd",
    "valuntil",
    "useconfig",
];

/// `pg_shadow` 系统表的 Schema
pub fn pg_shadow_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_shadow"),
        columns: vec![
            ColumnDefinition::new("usename", ColumnType::Text),
            ColumnDefinition::new("usesysid", ColumnType::Int64),
            ColumnDefinition::new("usecreatedb", ColumnType::Bool),
            ColumnDefinition::new("usesuper", ColumnType::Bool),
            ColumnDefinition::new("userepl", ColumnType::Bool),
            ColumnDefinition::new("usebypassrls", ColumnType::Bool),
            ColumnDefinition::new("passwd", ColumnType::Text),
            ColumnDefinition::new("valuntil", ColumnType::Timestamp),
            ColumnDefinition::new("useconfig", ColumnType::Array(Box::new(ColumnType::Text))),
        ],
    }
}

/// 查询 `pg_shadow` — 为每个允许的用户生成一行
///
/// usesysid 与 pg_roles.oid 一致（postgres=10）。
pub fn pg_shadow(allowed_users: &[String]) -> Vec<SysRow> {
    let users: Vec<&str> = if allowed_users.is_empty() {
        vec!["postgres"]
    } else {
        allowed_users.iter().map(|s| s.as_str()).collect()
    };

    users
        .iter()
        .enumerate()
        .map(|(idx, user)| {
            vec![
                Value::Text((*user).into()),         // usename
                Value::Int64(10 + idx as i64),       // usesysid
                Value::Bool(true),                   // usecreatedb
                Value::Bool(true),                   // usesuper
                Value::Bool(false),                  // userepl
                Value::Bool(true),                   // usebypassrls
                Value::Null,                         // passwd
                Value::Null,                         // valuntil
                Value::Array(vec![]),                // useconfig
            ]
        })
        .collect()
}

/// `pg_user` 系统表的列名（与 PG 14 一致，8 列）
///
/// 列顺序：(usename, usesysid, usecreatedb, usesuper, userepl,
///          usebypassrls, valuntil, useconfig)
pub const PG_USER_COLUMNS: &[&str] = &[
    "usename",
    "usesysid",
    "usecreatedb",
    "usesuper",
    "userepl",
    "usebypassrls",
    "valuntil",
    "useconfig",
];

/// `pg_user` 系统表的 Schema
pub fn pg_user_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_user"),
        columns: vec![
            ColumnDefinition::new("usename", ColumnType::Text),
            ColumnDefinition::new("usesysid", ColumnType::Int64),
            ColumnDefinition::new("usecreatedb", ColumnType::Bool),
            ColumnDefinition::new("usesuper", ColumnType::Bool),
            ColumnDefinition::new("userepl", ColumnType::Bool),
            ColumnDefinition::new("usebypassrls", ColumnType::Bool),
            ColumnDefinition::new("valuntil", ColumnType::Timestamp),
            ColumnDefinition::new("useconfig", ColumnType::Array(Box::new(ColumnType::Text))),
        ],
    }
}

/// 查询 `pg_user` — 为每个允许的用户生成一行
///
/// usesysid 与 pg_roles.oid 一致（postgres=10）。
pub fn pg_user(allowed_users: &[String]) -> Vec<SysRow> {
    let users: Vec<&str> = if allowed_users.is_empty() {
        vec!["postgres"]
    } else {
        allowed_users.iter().map(|s| s.as_str()).collect()
    };

    users
        .iter()
        .enumerate()
        .map(|(idx, user)| {
            vec![
                Value::Text((*user).into()),         // usename
                Value::Int64(10 + idx as i64),       // usesysid
                Value::Bool(true),                   // usecreatedb
                Value::Bool(true),                   // usesuper
                Value::Bool(false),                  // userepl
                Value::Bool(true),                   // usebypassrls
                Value::Null,                         // valuntil
                Value::Array(vec![]),                // useconfig
            ]
        })
        .collect()
}

// =====================================================================
//  pg_settings — 服务器配置参数（Navicat 启动时查询）
//
//  Navicat 启动时会发送 `SELECT name, setting, category, short_desc FROM pg_settings`
//  来获取服务器配置信息。返回 PG 14 默认配置值，确保 Navicat 能正常解析。
// =====================================================================

/// `pg_settings` 系统表的列名（与 PG 14 一致，14 列）
///
/// 列顺序：(name, setting, unit, category, short_desc, context, vartype,
///          source, min_val, max_val, enumvals, boot_val, reset_val, sourcefile)
pub const PG_SETTINGS_COLUMNS: &[&str] = &[
    "name",
    "setting",
    "unit",
    "category",
    "short_desc",
    "context",
    "vartype",
    "source",
    "min_val",
    "max_val",
    "enumvals",
    "boot_val",
    "reset_val",
    "sourcefile",
];

/// `pg_settings` 系统表的 Schema
pub fn pg_settings_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_settings"),
        columns: vec![
            ColumnDefinition::new("name", ColumnType::Text),
            ColumnDefinition::new("setting", ColumnType::Text),
            ColumnDefinition::new("unit", ColumnType::Text),
            ColumnDefinition::new("category", ColumnType::Text),
            ColumnDefinition::new("short_desc", ColumnType::Text),
            ColumnDefinition::new("context", ColumnType::Text),
            ColumnDefinition::new("vartype", ColumnType::Text),
            ColumnDefinition::new("source", ColumnType::Text),
            ColumnDefinition::new("min_val", ColumnType::Text),
            ColumnDefinition::new("max_val", ColumnType::Text),
            ColumnDefinition::new("enumvals", ColumnType::Array(Box::new(ColumnType::Text))),
            ColumnDefinition::new("boot_val", ColumnType::Text),
            ColumnDefinition::new("reset_val", ColumnType::Text),
            ColumnDefinition::new("sourcefile", ColumnType::Text),
        ],
    }
}

/// 查询 `pg_settings` — 返回 Navicat 启动时需要的核心配置参数
///
/// 返回 PG 14 默认配置值（与 SessionState::new() 注入的默认值一致）。
/// 仅返回 Navicat 启动必需的核心参数，不含 PG 全部 300+ 配置项。
pub fn pg_settings(server_version: &str, allowed_databases: &[String]) -> Vec<SysRow> {
    // 构建 search_path：默认 "public"，若 allowed_databases 非空则追加
    let search_path = if allowed_databases.is_empty() {
        "public".to_string()
    } else {
        let mut parts: Vec<&str> = vec!["public"];
        for db in allowed_databases {
            parts.push(db.as_str());
        }
        parts.join(", ")
    };

    // 构建 server_version 显示字符串
    let version_str = if server_version.is_empty() {
        "14.0-szrsql (SzRSQL 1.0.0-rc.2)".to_string()
    } else {
        server_version.to_string()
    };
    fn row(name: &str, setting: &str, category: &str, short_desc: &str) -> SysRow {
        vec![
            Value::Text(name.into()),
            Value::Text(setting.into()),
            Value::Null,                              // unit
            Value::Text(category.into()),
            Value::Text(short_desc.into()),
            Value::Text("user".into()),               // context
            Value::Text("string".into()),             // vartype
            Value::Text("default".into()),            // source
            Value::Null,                              // min_val
            Value::Null,                              // max_val
            Value::Null,                              // enumvals
            Value::Text(setting.into()),              // boot_val
            Value::Text(setting.into()),              // reset_val
            Value::Null,                              // sourcefile
        ]
    }
    vec![
        row("server_version", &version_str, "Preset Options", "Shows the server version."),
        row("server_encoding", "UTF8", "Preset Options", "Sets the server (database) character set encoding."),
        row("client_encoding", "UTF8", "Client Connection Defaults / Locale and Formatting", "Sets the client's character set encoding."),
        row("transaction_isolation", "read committed", "Client Connection Defaults / Statement Behavior", "Sets the current transaction's isolation level."),
        row("standard_conforming_strings", "on", "Client Connection Defaults / Statement Behavior", "Causes '...' strings to treat backslashes literally."),
        row("integer_datetimes", "on", "Preset Options", "Datetimes are represented as 64-bit integers."),
        row("TimeZone", "UTC", "Client Connection Defaults / Locale and Formatting", "Sets the time zone for displaying and interpreting time stamps."),
        row("extra_float_digits", "3", "Client Connection Defaults / Statement Behavior", "Sets the number of digits displayed for floating-point values."),
        row("search_path", &search_path, "Client Connection Defaults / Statement Behavior", "Sets the schema search order for names that are not schema-qualified."),
        row("max_connections", "100", "Connections and Authentication / Connection Settings", "Sets the maximum number of concurrent connections."),
        row("application_name", "", "Client Connection Defaults / Statement Behavior", "Sets the application name used to identify the session in various logs."),
        row("DateStyle", "ISO, MDY", "Client Connection Defaults / Locale and Formatting", "Sets the display format for date and time values."),
        row("IntervalStyle", "postgres", "Client Connection Defaults / Locale and Formatting", "Sets the display format for interval values."),
        row("lc_collate", "C", "Preset Options", "Shows the collation order locale."),
        row("lc_ctype", "C", "Preset Options", "Shows the character classification locale."),
        row("listen_addresses", "*", "Connections and Authentication / Connection Settings", "Sets the host name or IP address(es) to listen to."),
        row("wal_level", "replica", "Write-Ahead Log / Settings", "Sets the level of information written to the WAL."),
        row("max_wal_senders", "0", "Replication / Sending Servers", "Sets the maximum number of simultaneously running WAL sender processes."),
        row("hot_standby", "off", "Replication / Standby Servers", "Allows connections and queries during recovery."),
    ]
}

// =====================================================================
//  Navicat 兼容：其余 pg_catalog 系统表/视图（占位实现）
//
//  这些表 Navicat 启动时会查询，但 SzRSQL 不需要真实数据，返回空结果集即可。
//  关键的 pg_tablespace 需要返回 1 行（pg_default，OID=1663），让
//  `pg_database JOIN pg_tablespace` 能命中。
// =====================================================================

/// `pg_tablespace` 系统表的列名（与 PG 14 一致，6 列）
pub const PG_TABLESPACE_COLUMNS: &[&str] = &[
    "oid", "spcname", "spcowner", "spcacl", "spcoptions", "spcmaxsize",
];

/// `pg_tablespace` 系统表的 Schema
pub fn pg_tablespace_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_tablespace"),
        columns: vec![
            ColumnDefinition::new("oid", ColumnType::Int64),
            ColumnDefinition::new("spcname", ColumnType::Text),
            ColumnDefinition::new("spcowner", ColumnType::Int64),
            ColumnDefinition::new("spcacl", ColumnType::Array(Box::new(ColumnType::Text))),
            ColumnDefinition::new("spcoptions", ColumnType::Array(Box::new(ColumnType::Text))),
            ColumnDefinition::new("spcmaxsize", ColumnType::Int64),
        ],
    }
}

/// 查询 `pg_tablespace` — 返回 pg_default（OID=1663）和 pg_global（OID=1664）
///
/// 让 `pg_database JOIN pg_tablespace ON d.dattablespace = t.oid` 能命中。
pub fn pg_tablespace() -> Vec<SysRow> {
    vec![
        vec![
            Value::Int64(1663),                       // oid (pg_default)
            Value::Text("pg_default".into()),         // spcname
            Value::Int64(10),                         // spcowner
            Value::Array(vec![]),                     // spcacl
            Value::Array(vec![]),                     // spcoptions
            Value::Null,                              // spcmaxsize
        ],
        vec![
            Value::Int64(1664),                       // oid (pg_global)
            Value::Text("pg_global".into()),          // spcname
            Value::Int64(10),                         // spcowner
            Value::Array(vec![]),                     // spcacl
            Value::Array(vec![]),                     // spcoptions
            Value::Null,                              // spcmaxsize
        ],
    ]
}

/// 创建一个文本列占位的 Schema
fn placeholder_schema(table_name: &str, columns: &[&str]) -> TableSchema {
    TableSchema {
        name: TableName::new(table_name),
        columns: columns
            .iter()
            .map(|c| ColumnDefinition::new(*c, ColumnType::Text))
            .collect(),
    }
}

macro_rules! define_placeholder_system_table {
    ($func_name:ident, $const_name:ident, $table_name:expr, $columns:expr) => {
        pub const $const_name: &[&str] = $columns;
        pub fn $func_name() -> TableSchema {
            placeholder_schema($table_name, $columns)
        }
    };
}

// Navicat 启动时查询的占位系统表（返回空行集）
define_placeholder_system_table!(pg_stat_activity_schema, PG_STAT_ACTIVITY_COLUMNS, "pg_stat_activity",
    &["datid","datname","pid","usesysid","application_name","state","query","wait_event_type","wait_event","xact_start","query_start","backend_start","state_change","client_addr","client_hostname","client_port","backend_xid","backend_xmin","backend_type"]);
define_placeholder_system_table!(pg_locks_schema, PG_LOCKS_COLUMNS, "pg_locks",
    &["locktype","database","relation","page","tuple","virtualxid","transactionid","classid","objid","objsubid","virtualtransaction","pid","mode","granted","fastpath"]);
define_placeholder_system_table!(pg_matviews_schema, PG_MATVIEWS_COLUMNS, "pg_matviews",
    &["schemaname","matviewname","matviewowner","definition"]);
define_placeholder_system_table!(pg_rewrite_schema, PG_REWRITE_COLUMNS, "pg_rewrite",
    &["oid","rulename","ev_class","ev_type","ev_enabled","is_instead","ev_qual","ev_action"]);
define_placeholder_system_table!(pg_trigger_schema, PG_TRIGGER_COLUMNS, "pg_trigger",
    &["oid","tgrelid","tgname","tgfoid","tgtype","tgenabled","tgisinternal","tgconstrrelid","tgconstrindid","tgconstraint","tgdeferrable","tginitdeferred","tgnargs","tgattr","tgargs","tgqual","tgoldtable","tgnewtable"]);
define_placeholder_system_table!(pg_authid_schema, PG_AUTHID_COLUMNS, "pg_authid",
    &["oid","rolname","rolsuper","rolinherit","rolcreaterole","rolcreatedb","rolcanlogin","rolreplication","rolconnlimit","rolvaliduntil","rolbypassrls"]);
define_placeholder_system_table!(pg_proc_schema, PG_PROC_COLUMNS, "pg_proc",
    &["oid","proname","pronamespace","proowner","prolang","procost","prorows","provariadic","protransform","proisagg","proiswindow","prosecdef","proleakproof","proisstrict","proretset","provolatile","proparallel","pronargs","pronargdefaults","prorettype","proargtypes","proallargtypes","proargmodes","proargnames","proargdefaults","protrftypes","prosrc","probin","proconfig","proacl"]);
define_placeholder_system_table!(pg_db_role_setting_schema, PG_DB_ROLE_SETTING_COLUMNS, "pg_db_role_setting",
    &["setdatabase","setrole","setconfig"]);
define_placeholder_system_table!(pg_default_acl_schema, PG_DEFAULT_ACL_COLUMNS, "pg_default_acl",
    &["oid","defaclrole","defaclnamespace","defaclobjtype","defaclacl"]);
define_placeholder_system_table!(pg_shdescription_schema, PG_SHDESCRIPTION_COLUMNS, "pg_shdescription",
    &["objoid","classoid","description"]);
define_placeholder_system_table!(pg_event_trigger_schema, PG_EVENT_TRIGGER_COLUMNS, "pg_event_trigger",
    &["oid","evtname","evtevent","evtowner","evtfoid","evtenabled","evtenabled"]);
define_placeholder_system_table!(pg_extension_schema, PG_EXTENSION_COLUMNS, "pg_extension",
    &["oid","extname","extowner","extnamespace","extrelocatable","extversion","extconfig","extcondition"]);
define_placeholder_system_table!(pg_collation_schema, PG_COLLATION_COLUMNS, "pg_collation",
    &["oid","collname","collnamespace","collowner","collencoding","collcollate","collctype","collprovider","collisdefault"]);
define_placeholder_system_table!(pg_am_schema, PG_AM_COLUMNS, "pg_am",
    &["oid","amname","amhandler","amtype"]);
define_placeholder_system_table!(pg_opclass_schema, PG_OPCLASS_COLUMNS, "pg_opclass",
    &["oid","opcmethod","opcname","opcnamespace","opcowner","opcfamily","opcintype","opcdefault","opckeytype"]);
define_placeholder_system_table!(pg_opfamily_schema, PG_OPFAMILY_COLUMNS, "pg_opfamily",
    &["oid","opfmethod","opfname","opfnamespace","opfowner"]);
define_placeholder_system_table!(pg_cast_schema, PG_CAST_COLUMNS, "pg_cast",
    &["oid","castsource","casttarget","castfunc","castcontext","castmethod"]);
define_placeholder_system_table!(pg_conversion_schema, PG_CONVERSION_COLUMNS, "pg_conversion",
    &["oid","conname","connamespace","conowner","conforencoding","contoencoding","conproc","condefault"]);
define_placeholder_system_table!(pg_depend_schema, PG_DEPEND_COLUMNS, "pg_depend",
    &["classid","objid","objsubid","refclassid","refobjid","refobjsubid","deptype"]);
define_placeholder_system_table!(pg_shdepend_schema, PG_SHDEPEND_COLUMNS, "pg_shdepend",
    &["dbid","classid","objid","objsubid","refclassid","refobjid","refobjsubid","deptype"]);
define_placeholder_system_table!(pg_stat_user_tables_schema, PG_STAT_USER_TABLES_COLUMNS, "pg_stat_user_tables",
    &["relid","schemaname","relname","seq_scan","seq_tup_read","idx_scan","idx_tup_fetch","n_tup_ins","n_tup_upd","n_tup_del","n_tup_hot_upd","n_live_tup","n_dead_tup","n_mod_since_analyze","last_vacuum","last_autovacuum","last_analyze","last_autoanalyze","vacuum_count","autovacuum_count","analyze_count","autoanalyze_count"]);
define_placeholder_system_table!(pg_statio_user_tables_schema, PG_STATIO_USER_TABLES_COLUMNS, "pg_statio_user_tables",
    &["relid","schemaname","relname","heap_blks_read","heap_blks_hit","idx_blks_read","idx_blks_hit","toast_blks_read","toast_blks_hit","tidx_blks_read","tidx_blks_hit"]);
define_placeholder_system_table!(pg_attrdef_schema, PG_ATTRDEF_COLUMNS, "pg_attrdef",
    &["oid","adrelid","adnum","adbin"]);
define_placeholder_system_table!(pg_auth_members_schema, PG_AUTH_MEMBERS_COLUMNS, "pg_auth_members",
    &["roleid","member","grantor","admin_option"]);
define_placeholder_system_table!(pg_policy_schema, PG_POLICY_COLUMNS, "pg_policy",
    &["oid","polrelid","polname","polcmd","polpermissive","polroles","polqual","polwithcheck"]);
define_placeholder_system_table!(pg_inherits_schema, PG_INHERITS_COLUMNS, "pg_inherits",
    &["inhrelid","inhparent","inhseqno"]);
define_placeholder_system_table!(pg_init_privs_schema, PG_INIT_PRIVS_COLUMNS, "pg_init_privs",
    &["objoid","classoid","objsubid","privtype","grantor","grantee","privs"]);
define_placeholder_system_table!(pg_language_schema, PG_LANGUAGE_COLUMNS, "pg_language",
    &["oid","lanname","lanowner","lanispl","lanpltrusted","lanplcallfoid","lanvalidator","lanacl","laninl","laninline"]);
define_placeholder_system_table!(pg_largeobject_schema, PG_LARGEOBJECT_COLUMNS, "pg_largeobject",
    &["loid","pageno","data"]);
define_placeholder_system_table!(pg_largeobject_metadata_schema, PG_LARGEOBJECT_METADATA_COLUMNS, "pg_largeobject_metadata",
    &["oid","lomowner","lomacl"]);
define_placeholder_system_table!(pg_seclabel_schema, PG_SECLABEL_COLUMNS, "pg_seclabel",
    &["objoid","classoid","objsubid","provider","label"]);
define_placeholder_system_table!(pg_shseclabel_schema, PG_SHSECLABEL_COLUMNS, "pg_shseclabel",
    &["objoid","classoid","provider","label"]);
define_placeholder_system_table!(pg_stat_database_schema, PG_STAT_DATABASE_COLUMNS, "pg_stat_database",
    &["datid","datname","numbackends","xact_commit","xact_rollback","blks_read","blks_hit","tup_returned","tup_fetched","tup_inserted","tup_updated","tup_deleted","conflicts","temp_files","temp_bytes","deadlocks","blk_read_time","blk_write_time","stats_reset"]);
define_placeholder_system_table!(pg_stat_database_conflicts_schema, PG_STAT_DATABASE_CONFLICTS_COLUMNS, "pg_stat_database_conflicts",
    &["datid","datname","confl_tablespace","confl_lock","confl_snapshot","confl_bufferpin","confl_deadlock"]);
define_placeholder_system_table!(pg_stat_bgwriter_schema, PG_STAT_BGWRITER_COLUMNS, "pg_stat_bgwriter",
    &["checkpoints_timed","checkpoints_req","checkpoint_write_time","checkpoint_sync_time","buffers_checkpoint","buffers_clean","maxwritten_clean","buffers_backend","buffers_backend_fsync","buffers_alloc","stats_reset"]);
define_placeholder_system_table!(pg_stats_schema, PG_STATS_COLUMNS, "pg_stats",
    &["schemaname","tablename","attname","inherited","null_frac","avg_width","n_distinct","most_common_vals","most_common_freqs","histogram_bounds","correlation","most_common_elems","most_common_elem_freqs","elem_count_histogram"]);
define_placeholder_system_table!(pg_class_reltype_schema, PG_CLASS_RELTYPE_COLUMNS, "pg_class_reltype",
    &["oid","relname","relnamespace","reltype","relowner","relam","relfilenode","reltablespace","relpages","reltuples","relallvisible","reltoastrelid","relhasindex","relisshared","relpersistence","relkind","relnatts","relchecks","relhasrules","relhastriggers","relhassubclass","relrowsecurity","relforcerowsecurity","relispartition","relrewrite","relfrozenxid","relminmxid","relacl","reloptions","relpartbound"]);
// Navicat 兼容：pg_operator 系统表（操作符目录，空占位）
define_placeholder_system_table!(pg_operator_schema, PG_OPERATOR_COLUMNS, "pg_operator",
    &["oid","oprname","oprnamespace","oprowner","oprkind","oprcanmerge","oprcanhash","oprleft","oprright","oprresult","oprcom","oprnegate","oprcode","oprrest","oprjoin"]);
// Navicat 兼容：pg_foreign_table 系统表（外部表目录，空占位）
define_placeholder_system_table!(pg_foreign_table_schema, PG_FOREIGN_TABLE_COLUMNS, "pg_foreign_table",
    &["ftrelid","ftserver","ftoptions"]);
// Navicat 兼容：information_schema.routines 系统表（存储过程/函数元数据，空占位）
define_placeholder_system_table!(information_schema_routines_schema, INFORMATION_SCHEMA_ROUTINES_COLUMNS, "routines",
    &["specific_catalog","specific_schema","specific_name","routine_catalog","routine_schema","routine_name","routine_type","module_catalog","module_schema","module_name","udt_catalog","udt_schema","udt_name","character_maximum_length","character_octet_length","created","last_altered","numeric_precision","numeric_precision_radix","numeric_scale","datetime_precision","interval_type","interval_precision","type_udt_catalog","type_udt_schema","type_udt_name","scope_catalog","scope_schema","scope_name","maximum_cardinality","dtd_identifier","sql_data_access","is_deterministic","sql_path","schema_name","specific_schema","security_type"]);
// Navicat 兼容：information_schema.parameters 系统表（参数元数据，空占位）
define_placeholder_system_table!(information_schema_parameters_schema, INFORMATION_SCHEMA_PARAMETERS_COLUMNS, "parameters",
    &["specific_catalog","specific_schema","specific_name","ordinal_position","parameter_mode","is_result","as_locator","parameter_name","data_type","character_maximum_length","character_octet_length","character_set_catalog","character_set_schema","character_set_name","collation_catalog","collation_schema","collation_name","numeric_precision","numeric_precision_radix","numeric_scale","datetime_precision","interval_type","interval_precision","udt_catalog","udt_schema","udt_name","scope_catalog","scope_schema","scope_name","maximum_cardinality","dtd_identifier","parameter_default"]);
// Navicat 兼容：pg_sequence 系统表（序列定义，空占位）
define_placeholder_system_table!(pg_sequence_schema, PG_SEQUENCE_COLUMNS, "pg_sequence",
    &["seqrelid","seqtypid","seqstart","seqincrement","seqmax","seqmin","seqcache","seqcycle"]);
// Navicat 兼容：pg_foreign_server 系统表（外部服务器，空占位）
define_placeholder_system_table!(pg_foreign_server_schema, PG_FOREIGN_SERVER_COLUMNS, "pg_foreign_server",
    &["oid","srvname","srvowner","srvfdw","srvtype","srvversion","srvacl","srvoptions"]);

/// 所有占位系统表的空行集
pub fn empty_rows() -> Vec<SysRow> {
    Vec::new()
}

// =====================================================================
//  pg_proc — 内置函数列表（Navicat 查询函数元数据）
// =====================================================================

/// 查询 `pg_proc` — 返回 SzRSQL 内置函数列表
///
/// 列顺序与 PG_PROC_COLUMNS 一致（30 列），但仅填充关键字段：
/// oid / proname / pronamespace / prorettype / pronargs / proargtypes
/// 其余字段为默认值。
pub fn pg_proc() -> Vec<SysRow> {
    // (函数名, 返回类型OID, 参数类型OID列表)
    // OID 从 20000 开始，避免与 PG 内置函数 OID 冲突
    let funcs: &[(&str, i64, &[i64])] = &[
        // 聚合函数
        ("count", pg_type_oid::INT8, &[]),
        ("sum", pg_type_oid::NUMERIC, &[pg_type_oid::INT8]),
        ("avg", pg_type_oid::NUMERIC, &[pg_type_oid::INT8]),
        ("min", pg_type_oid::INT8, &[pg_type_oid::INT8]),
        ("max", pg_type_oid::INT8, &[pg_type_oid::INT8]),
        // 数学函数
        ("abs", pg_type_oid::INT8, &[pg_type_oid::INT8]),
        ("ceil", pg_type_oid::FLOAT8, &[pg_type_oid::FLOAT8]),
        ("floor", pg_type_oid::FLOAT8, &[pg_type_oid::FLOAT8]),
        ("round", pg_type_oid::NUMERIC, &[pg_type_oid::NUMERIC]),
        ("sqrt", pg_type_oid::FLOAT8, &[pg_type_oid::FLOAT8]),
        // 字符串函数
        ("length", pg_type_oid::INT8, &[pg_type_oid::TEXT]),
        ("char_length", pg_type_oid::INT8, &[pg_type_oid::TEXT]),
        ("lower", pg_type_oid::TEXT, &[pg_type_oid::TEXT]),
        ("upper", pg_type_oid::TEXT, &[pg_type_oid::TEXT]),
        ("substring", pg_type_oid::TEXT, &[pg_type_oid::TEXT]),
        ("trim", pg_type_oid::TEXT, &[pg_type_oid::TEXT]),
        ("concat", pg_type_oid::TEXT, &[pg_type_oid::TEXT]),
        // 日期时间函数
        ("now", pg_type_oid::TIMESTAMP, &[]),
        ("current_timestamp", pg_type_oid::TIMESTAMP, &[]),
        ("current_date", pg_type_oid::DATE, &[]),
        ("current_time", pg_type_oid::TEXT, &[]),
        ("extract", pg_type_oid::FLOAT8, &[pg_type_oid::TEXT]),
        ("date_part", pg_type_oid::FLOAT8, &[pg_type_oid::TEXT]),
        // 其他
        ("coalesce", pg_type_oid::TEXT, &[pg_type_oid::TEXT]),
        ("nullif", pg_type_oid::TEXT, &[pg_type_oid::TEXT]),
    ];

    funcs
        .iter()
        .enumerate()
        .map(|(idx, (name, ret_type, arg_types))| {
            let oid = 20000 + idx as i64;
            let pronargs = arg_types.len() as i64;
            // proargtypes：空格分隔的参数类型 OID
            let proargtypes = arg_types
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            vec![
                Value::Int64(oid),                        // oid
                Value::Text((*name).into()),              // proname
                Value::Int64(PG_NAMESPACE_PUBLIC_OID),    // pronamespace
                Value::Int64(10),                         // proowner
                Value::Int64(12),                         // prolang (internal)
                Value::Int64(1),                          // procost
                Value::Int64(0),                          // prorows
                Value::Int64(0),                          // provariadic
                Value::Int64(0),                          // protransform
                Value::Bool(false),                       // proisagg (聚合函数标记)
                Value::Bool(false),                       // proiswindow
                Value::Bool(false),                       // prosecdef
                Value::Bool(false),                       // proleakproof
                Value::Bool(true),                        // proisstrict
                Value::Bool(false),                       // proretset
                Value::Text("v".into()),                  // provolatile (v=volatile)
                Value::Text("s".into()),                  // proparallel (s=safe)
                Value::Int64(pronargs),                   // pronargs
                Value::Int64(0),                          // pronargdefaults
                Value::Int64(*ret_type),                  // prorettype
                Value::Text(proargtypes),                 // proargtypes
                Value::Array(vec![]),                     // proallargtypes
                Value::Array(vec![]),                     // proargmodes
                Value::Array(vec![]),                     // proargnames
                Value::Null,                              // proargdefaults
                Value::Array(vec![]),                     // protrftypes
                Value::Text((*name).into()),              // prosrc
                Value::Null,                              // probin
                Value::Array(vec![]),                     // proconfig
                Value::Array(vec![]),                     // proacl
            ]
        })
        .collect()
}

// =====================================================================
//  pg_cast — 类型转换规则
// =====================================================================

/// 查询 `pg_cast` — 返回基本类型转换规则
///
/// 列顺序：(oid, castsource, casttarget, castfunc, castcontext, castmethod)
/// - castcontext: 'e'=explicit (需显式 CAST), 'a'=assignment, 'i'=implicit
/// - castmethod: 'f'=function, 'b'=binary coercion
pub fn pg_cast() -> Vec<SysRow> {
    let casts: &[(i64, i64, i64, &str, &str)] = &[
        // (source, target, func, context, method)
        // int8 ↔ int4
        (pg_type_oid::INT8, pg_type_oid::INT4, 0, "i", "b"),
        (pg_type_oid::INT4, pg_type_oid::INT8, 0, "i", "b"),
        // int8 → float8
        (pg_type_oid::INT8, pg_type_oid::FLOAT8, 0, "i", "b"),
        // int4 → float8
        (pg_type_oid::INT4, pg_type_oid::FLOAT8, 0, "i", "b"),
        // int8 → numeric
        (pg_type_oid::INT8, pg_type_oid::NUMERIC, 0, "i", "b"),
        // int4 → numeric
        (pg_type_oid::INT4, pg_type_oid::NUMERIC, 0, "i", "b"),
        // float8 → numeric
        (pg_type_oid::FLOAT8, pg_type_oid::NUMERIC, 0, "a", "b"),
        // numeric → float8
        (pg_type_oid::NUMERIC, pg_type_oid::FLOAT8, 0, "a", "b"),
        // text → varchar
        (pg_type_oid::TEXT, pg_type_oid::VARCHAR, 0, "i", "b"),
        // varchar → text
        (pg_type_oid::VARCHAR, pg_type_oid::TEXT, 0, "i", "b"),
        // bool → int4
        (pg_type_oid::BOOL, pg_type_oid::INT4, 0, "i", "b"),
        // int4 → bool
        (pg_type_oid::INT4, pg_type_oid::BOOL, 0, "i", "b"),
        // text → int4
        (pg_type_oid::TEXT, pg_type_oid::INT4, 0, "a", "f"),
        // text → int8
        (pg_type_oid::TEXT, pg_type_oid::INT8, 0, "a", "f"),
        // text → float8
        (pg_type_oid::TEXT, pg_type_oid::FLOAT8, 0, "a", "f"),
        // text → numeric
        (pg_type_oid::TEXT, pg_type_oid::NUMERIC, 0, "a", "f"),
        // text → bool
        (pg_type_oid::TEXT, pg_type_oid::BOOL, 0, "a", "f"),
        // text → date
        (pg_type_oid::TEXT, pg_type_oid::DATE, 0, "a", "f"),
        // text → timestamp
        (pg_type_oid::TEXT, pg_type_oid::TIMESTAMP, 0, "a", "f"),
    ];

    casts
        .iter()
        .enumerate()
        .map(|(idx, (src, tgt, func, ctx, method))| {
            vec![
                Value::Int64(30000 + idx as i64),  // oid
                Value::Int64(*src),                // castsource
                Value::Int64(*tgt),                // casttarget
                Value::Int64(*func),               // castfunc
                Value::Text((*ctx).into()),        // castcontext
                Value::Text((*method).into()),     // castmethod
            ]
        })
        .collect()
}

// =====================================================================
//  pg_operator — 运算符列表
// =====================================================================

/// 查询 `pg_operator` — 返回基本运算符列表
///
/// 列顺序与 PG_OPERATOR_COLUMNS 一致（15 列）。
/// 仅包含 SzRSQL 支持的核心运算符。
pub fn pg_operator() -> Vec<SysRow> {
    // (oprname, oprkind, oprleft, oprright, oprresult)
    // oprkind: 'b'=binary, 'l'=left unary, 'r'=right unary
    let ops: &[(&str, &str, i64, i64, i64)] = &[
        // 比较运算符（返回 bool）
        ("=", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::BOOL),
        ("<>", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::BOOL),
        ("<", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::BOOL),
        (">", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::BOOL),
        ("<=", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::BOOL),
        (">=", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::BOOL),
        // 文本比较
        ("=", "b", pg_type_oid::TEXT, pg_type_oid::TEXT, pg_type_oid::BOOL),
        ("<>", "b", pg_type_oid::TEXT, pg_type_oid::TEXT, pg_type_oid::BOOL),
        ("<", "b", pg_type_oid::TEXT, pg_type_oid::TEXT, pg_type_oid::BOOL),
        (">", "b", pg_type_oid::TEXT, pg_type_oid::TEXT, pg_type_oid::BOOL),
        // 算术运算符
        ("+", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::INT8),
        ("-", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::INT8),
        ("*", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::INT8),
        ("/", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::INT8),
        ("%", "b", pg_type_oid::INT8, pg_type_oid::INT8, pg_type_oid::INT8),
        // 一元负号
        ("-", "l", 0, pg_type_oid::INT8, pg_type_oid::INT8),
        // 浮点算术
        ("+", "b", pg_type_oid::FLOAT8, pg_type_oid::FLOAT8, pg_type_oid::FLOAT8),
        ("-", "b", pg_type_oid::FLOAT8, pg_type_oid::FLOAT8, pg_type_oid::FLOAT8),
        ("*", "b", pg_type_oid::FLOAT8, pg_type_oid::FLOAT8, pg_type_oid::FLOAT8),
        ("/", "b", pg_type_oid::FLOAT8, pg_type_oid::FLOAT8, pg_type_oid::FLOAT8),
    ];

    ops.iter()
        .enumerate()
        .map(|(idx, (name, kind, left, right, result))| {
            vec![
                Value::Int64(40000 + idx as i64),    // oid
                Value::Text((*name).into()),         // oprname
                Value::Int64(PG_NAMESPACE_PUBLIC_OID), // oprnamespace
                Value::Int64(10),                    // oprowner
                Value::Text((*kind).into()),         // oprkind
                Value::Bool(false),                  // oprcanmerge
                Value::Bool(false),                  // oprcanhash
                Value::Int64(*left),                 // oprleft
                Value::Int64(*right),                // oprright
                Value::Int64(*result),               // oprresult
                Value::Int64(0),                     // oprcom
                Value::Int64(0),                     // oprnegate
                Value::Int64(0),                     // oprcode
                Value::Int64(0),                     // oprrest
                Value::Int64(0),                     // oprjoin
            ]
        })
        .collect()
}

// =====================================================================
//  pg_authid — 角色认证信息（同 pg_roles）
// =====================================================================

/// 查询 `pg_authid` — 与 pg_roles 一致，为每个允许的用户生成一行
///
/// 列顺序与 PG_AUTHID_COLUMNS 一致（11 列，比 pg_roles 多 rolreplication）。
pub fn pg_authid(allowed_users: &[String]) -> Vec<SysRow> {
    let users: Vec<&str> = if allowed_users.is_empty() {
        vec!["postgres"]
    } else {
        allowed_users.iter().map(|s| s.as_str()).collect()
    };

    users
        .iter()
        .enumerate()
        .map(|(idx, user)| {
            vec![
                Value::Int64(10 + idx as i64),       // oid
                Value::Text((*user).into()),         // rolname
                Value::Bool(true),                   // rolsuper
                Value::Bool(true),                   // rolinherit
                Value::Bool(true),                   // rolcreaterole
                Value::Bool(true),                   // rolcreatedb
                Value::Bool(true),                   // rolcanlogin
                Value::Bool(false),                  // rolreplication
                Value::Int64(-1),                    // rolconnlimit
                Value::Null,                         // rolvaliduntil
                Value::Bool(true),                   // rolbypassrls
            ]
        })
        .collect()
}

// =====================================================================
//  pg_collation — 默认排序规则
// =====================================================================

/// 查询 `pg_collation` — 返回默认排序规则
///
/// 返回 PG 内置的两个排序规则：C 和 en_US.utf8。
pub fn pg_collation() -> Vec<SysRow> {
    vec![
        vec![
            Value::Int64(100),                        // oid (PG 内置 C 排序规则)
            Value::Text("C".into()),                 // collname
            Value::Int64(11),                        // collnamespace (pg_catalog)
            Value::Int64(10),                        // collowner
            Value::Int64(-1),                        // collencoding (任意编码)
            Value::Text("C".into()),                 // collcollate
            Value::Text("C".into()),                 // collctype
            Value::Text("c".into()),                 // collprovider (c=libc)
            Value::Bool(true),                       // collisdefault
        ],
        vec![
            Value::Int64(950),                       // oid (PG 内置 default 排序规则)
            Value::Text("default".into()),           // collname
            Value::Int64(11),                        // collnamespace
            Value::Int64(10),                        // collowner
            Value::Int64(6),                         // collencoding (UTF8)
            Value::Text("".into()),                  // collcollate
            Value::Text("".into()),                  // collctype
            Value::Text("c".into()),                 // collprovider
            Value::Bool(true),                       // collisdefault
        ],
        vec![
            Value::Int64(962),                       // oid
            Value::Text("en_US.utf8".into()),        // collname
            Value::Int64(11),                        // collnamespace
            Value::Int64(10),                        // collowner
            Value::Int64(6),                         // collencoding (UTF8)
            Value::Text("en_US.utf8".into()),        // collcollate
            Value::Text("en_US.utf8".into()),        // collctype
            Value::Text("c".into()),                 // collprovider
            Value::Bool(false),                      // collisdefault
        ],
    ]
}

// =====================================================================
//  pg_stat_activity — 当前连接（返回自身连接一行）
// =====================================================================

/// 查询 `pg_stat_activity` — 返回当前连接信息
///
/// SzRSQL 无连接级状态跟踪，返回单行占位表示自身连接。
pub fn pg_stat_activity(current_db: &str) -> Vec<SysRow> {
    vec![vec![
        Value::Int64(16384),                         // datid (当前数据库 OID)
        Value::Text(current_db.into()),              // datname
        Value::Int64(1),                             // pid (占位进程 ID)
        Value::Int64(10),                            // usesysid (postgres)
        Value::Text("".into()),                      // application_name
        Value::Text("idle".into()),                  // state
        Value::Text("SELECT 1".into()),              // query (占位)
        Value::Null,                                 // wait_event_type
        Value::Null,                                 // wait_event
        Value::Null,                                 // xact_start
        Value::Null,                                 // query_start
        Value::Null,                                 // backend_start
        Value::Null,                                 // state_change
        Value::Null,                                 // client_addr
        Value::Null,                                 // client_hostname
        Value::Null,                                 // client_port
        Value::Null,                                 // backend_xid
        Value::Null,                                 // backend_xmin
        Value::Text("client backend".into()),        // backend_type
    ]]
}

// =====================================================================
//  pg_sequence — 序列定义（P0-PG-7 修复：真实数据返回）
// =====================================================================

/// 查询 `pg_sequence` — 返回所有序列的真实定义
///
/// 列顺序与 `PG_SEQUENCE_COLUMNS` 一致：
/// `(seqrelid, seqtypid, seqstart, seqincrement, seqmax, seqmin, seqcache, seqcycle)`
///
/// - `seqrelid`：序列关系的 OID（复用 `oid_class_table`，与 pg_class 中序列项一致）
/// - `seqtypid`：序列数据类型 OID（PG 10+ 序列默认 bigint，OID=20）
/// - `seqstart`：起始值（`SequenceDefinition.start`）
/// - `seqincrement`：步长（`SequenceDefinition.increment`）
/// - `seqmax`：最大值（None 时按 PG 默认：increment>0 为 `i64::MAX`，increment<0 为 -1）
/// - `seqmin`：最小值（None 时按 PG 默认：increment>0 为 1，increment<0 为 `i64::MIN`）
/// - `seqcache`：缓存大小（SzRSQL 当前未实现 cache，固定返回 1，与 PG 默认一致）
/// - `seqcycle`：是否循环（`SequenceDefinition.cycle`）
///
/// 对应 PostgreSQL 14 `pg_catalog.pg_sequence` 行结构。
pub fn pg_sequence(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    let mut rows = Vec::new();
    for name in catalog.list_sequences() {
        if let Some(def) = catalog.get_sequence(&name) {
            let seqrelid = oid_class_table(&def.name);
            let seqtypid = pg_type_oid::INT8; // SzRSQL 序列始终为 bigint（与 PG 10+ 一致）
            let seqstart = def.start;
            let seqincrement = def.increment;
            // PG 默认值规则（与 CREATE SEQUENCE 默认行为一致）：
            // - increment > 0：min=1, max=INT64_MAX
            // - increment < 0：min=INT64_MIN, max=-1
            let seqmax = def
                .max_value
                .unwrap_or(if def.increment > 0 { i64::MAX } else { -1 });
            let seqmin = def
                .min_value
                .unwrap_or(if def.increment > 0 { 1 } else { i64::MIN });
            let seqcache = 1i64; // SzRSQL 当前未实现序列缓存，固定 1（PG 默认值）
            let seqcycle = def.cycle;

            rows.push(vec![
                Value::Int64(seqrelid),
                Value::Int64(seqtypid),
                Value::Int64(seqstart),
                Value::Int64(seqincrement),
                Value::Int64(seqmax),
                Value::Int64(seqmin),
                Value::Int64(seqcache),
                Value::Bool(seqcycle),
            ]);
        }
    }
    rows
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

// =====================================================================
//  测试 — P0-PG-7 验证
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManagedCatalog;
    use szrsql_sql::ast::TableName;
    use szrsql_sql::plan::SequenceDefinition;

    /// 测试 pg_sequence 返回空（无序列时）
    #[test]
    fn test_pg_sequence_empty() {
        let catalog = ManagedCatalog::new();
        let rows = pg_sequence(&catalog);
        assert!(rows.is_empty(), "无序列时 pg_sequence 应返回空");
    }

    /// 测试 pg_sequence 返回真实序列数据
    #[test]
    fn test_pg_sequence_with_sequences() {
        let mut catalog = ManagedCatalog::new();

        // 创建默认序列（start=1, increment=1, no min/max, no cycle）
        let def1 = SequenceDefinition::new(TableName::new("seq_test1"));
        catalog.create_sequence(def1);

        // 创建自定义序列
        let mut def2 = SequenceDefinition::new(TableName::new("seq_test2"));
        def2.start = 100;
        def2.increment = 5;
        def2.max_value = Some(1000);
        def2.min_value = Some(10);
        def2.cycle = true;
        catalog.create_sequence(def2);

        let rows = pg_sequence(&catalog);
        assert_eq!(rows.len(), 2, "应有 2 个序列");

        // 找到 seq_test1 的行（顺序可能不固定）
        let row1 = rows
            .iter()
            .find(|r| matches!(r[0], Value::Int64(oid) if oid == oid_class_table(&TableName::new("seq_test1"))))
            .expect("seq_test1 行应存在");
        // 验证列：seqrelid, seqtypid, seqstart, seqincrement, seqmax, seqmin, seqcache, seqcycle
        assert!(matches!(row1[0], Value::Int64(_)), "seqrelid 应为 Int64");
        assert_eq!(row1[1], Value::Int64(pg_type_oid::INT8), "seqtypid 应为 INT8(20)");
        assert_eq!(row1[2], Value::Int64(1), "seqstart 应为 1");
        assert_eq!(row1[3], Value::Int64(1), "seqincrement 应为 1");
        assert_eq!(row1[4], Value::Int64(i64::MAX), "seqmax 默认应为 i64::MAX");
        assert_eq!(row1[5], Value::Int64(1), "seqmin 默认应为 1");
        assert_eq!(row1[6], Value::Int64(1), "seqcache 应为 1");
        assert_eq!(row1[7], Value::Bool(false), "seqcycle 应为 false");

        // 验证 seq_test2
        let row2 = rows
            .iter()
            .find(|r| matches!(r[0], Value::Int64(oid) if oid == oid_class_table(&TableName::new("seq_test2"))))
            .expect("seq_test2 行应存在");
        assert_eq!(row2[2], Value::Int64(100), "seqstart 应为 100");
        assert_eq!(row2[3], Value::Int64(5), "seqincrement 应为 5");
        assert_eq!(row2[4], Value::Int64(1000), "seqmax 应为 1000");
        assert_eq!(row2[5], Value::Int64(10), "seqmin 应为 10");
        assert_eq!(row2[7], Value::Bool(true), "seqcycle 应为 true");
    }

    /// 测试 pg_sequence 降序序列的默认 min/max
    #[test]
    fn test_pg_sequence_descending() {
        let mut catalog = ManagedCatalog::new();
        let mut def = SequenceDefinition::new(TableName::new("seq_desc"));
        def.increment = -1; // 降序序列
        catalog.create_sequence(def);

        let rows = pg_sequence(&catalog);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        // 降序序列默认：max=-1, min=INT64_MIN
        assert_eq!(row[4], Value::Int64(-1), "降序序列 seqmax 默认应为 -1");
        assert_eq!(row[5], Value::Int64(i64::MIN), "降序序列 seqmin 默认应为 i64::MIN");
    }
}
