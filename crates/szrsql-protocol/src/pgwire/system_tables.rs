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
use szrsql_sql::ast::{
    BinaryOp, Expr, JoinCondition, JoinType, OrderByExpr, Select, SelectItem, Statement,
    TableFactor, TableName, UnaryOp,
};
use szrsql_sql::plan::{Catalog, ForeignKeyConstraint, ReferencingKey, TableSchema};
use szrsql_types::value::{ColumnType, Value};

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

    /// 列出所有视图名 — 委托给内部 InMemoryCatalog
    fn list_views(&self) -> Vec<TableName> {
        self.catalog.list_views()
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

    /// 替换表 Schema — Phase F-10（CatalogAdapter 只读，直接报错）
    fn replace_table_schema(&mut self, _schema: TableSchema) -> Result<(), CatalogError> {
        Err(CatalogError::InvalidArgument(
            "CatalogAdapter is read-only".into(),
        ))
    }

    /// 重命名表 — Phase F-10（CatalogAdapter 只读，直接报错）
    fn rename_table(
        &mut self,
        _old_name: &TableName,
        _new_name: &TableName,
    ) -> Result<(), CatalogError> {
        Err(CatalogError::InvalidArgument(
            "CatalogAdapter is read-only".into(),
        ))
    }

    /// 设置表注释 — Phase TDengine-P2（CatalogAdapter 只读，直接报错）
    fn set_table_comment(
        &mut self,
        _name: &TableName,
        _comment: Option<String>,
    ) -> Result<(), CatalogError> {
        Err(CatalogError::InvalidArgument(
            "CatalogAdapter is read-only".into(),
        ))
    }

    /// 设置列注释 — Phase TDengine-P2（CatalogAdapter 只读，直接报错）
    fn set_column_comment(
        &mut self,
        _table: &TableName,
        _column: &str,
        _comment: Option<String>,
    ) -> Result<(), CatalogError> {
        Err(CatalogError::InvalidArgument(
            "CatalogAdapter is read-only".into(),
        ))
    }

    /// 获取表注释 — Phase TDengine-P2（委托给内部 InMemoryCatalog）
    fn get_table_comment(&self, name: &TableName) -> Option<String> {
        self.catalog.get_table_comment(name)
    }

    /// 获取列注释 — Phase TDengine-P2（委托给内部 InMemoryCatalog）
    fn get_column_comment(&self, table: &TableName, column: &str) -> Option<String> {
        self.catalog.get_column_comment(table, column)
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
    /// `pg_database` — 数据库列表（Navicat 连接后立即查询）
    PgDatabase,
    /// `pg_namespace` — schema 列表
    PgNamespace,
    /// `pg_class` — 表/索引对象（relkind 区分）
    PgClass,
    /// `pg_attribute` — 表的列定义
    PgAttribute,
    /// `pg_type` — 类型定义
    PgType,
    /// `pg_index` — 索引详情
    PgIndex,
    /// `pg_constraint` — 约束
    PgConstraint,
    /// `pg_description` — 注释（占位）
    PgDescription,
    /// `pg_views` — 视图列表（占位）
    PgViews,
    /// `pg_roles` — 角色列表（Navicat JOIN 兼容，单行 postgres）
    PgRoles,
    /// `pg_shadow` — 用户密码视图（Navicat JOIN 兼容，单行 postgres）
    PgShadow,
    /// `pg_user` — 用户视图（Navicat JOIN 兼容，单行 postgres）
    PgUser,
    /// `pg_settings` — 服务器配置参数（Navicat 启动时查询）
    PgSettings,
    /// `pg_tablespace` — 表空间（Navicat 列数据库时 JOIN）
    PgTablespace,
    /// `pg_stat_activity` — 会话活动统计（已实现）
    PgStatActivity,
    /// `pg_locks` — 锁信息（占位空）
    PgLocks,
    /// `pg_matviews` — 物化视图（占位空）
    PgMatviews,
    /// `pg_rewrite` — 重写规则（占位空）
    PgRewrite,
    /// `pg_trigger` — 触发器（占位空）
    PgTrigger,
    /// `pg_authid` — 认证信息（已实现，硬编码单用户）
    PgAuthid,
    /// `pg_proc` — 存储过程（已实现）
    PgProc,
    /// `pg_db_role_setting` — 数据库角色设置（占位空）
    PgDbRoleSetting,
    /// `pg_default_acl` — 默认 ACL（占位空）
    PgDefaultAcl,
    /// `pg_shdescription` — 共享描述（占位空）
    PgShdescription,
    /// `pg_event_trigger` — 事件触发器（占位空）
    PgEventTrigger,
    /// `pg_extension` — 扩展（占位空）
    PgExtension,
    /// `pg_collation` — 排序规则（占位空）
    PgCollation,
    /// `pg_am` — 访问方法（占位空）
    PgAm,
    /// `pg_opclass` — 操作符类（占位空）
    PgOpclass,
    /// `pg_opfamily` — 操作符族（占位空）
    PgOpfamily,
    /// `pg_cast` — 类型转换（占位空）
    PgCast,
    /// `pg_conversion` — 编码转换（占位空）
    PgConversion,
    /// `pg_depend` — 依赖（占位空）
    PgDepend,
    /// `pg_shdepend` — 共享依赖（占位空）
    PgShdepend,
    /// `pg_stat_user_tables` — 用户表统计（占位空）
    PgStatUserTables,
    /// `pg_statio_user_tables` — 用户表 IO 统计（占位空）
    PgStatioUserTables,
    /// `pg_attrdef` — 列默认值定义（占位空）
    PgAttrdef,
    /// `pg_auth_members` — 角色成员（占位空）
    PgAuthMembers,
    /// `pg_policy` — 策略（占位空）
    PgPolicy,
    /// `pg_inherits` — 继承关系（占位空）
    PgInherits,
    /// `pg_init_privs` — 初始权限（占位空）
    PgInitPrivs,
    /// `pg_language` — 过程语言（占位空）
    PgLanguage,
    /// `pg_largeobject` — 大对象（占位空）
    PgLargeobject,
    /// `pg_largeobject_metadata` — 大对象元数据（占位空）
    PgLargeobjectMetadata,
    /// `pg_seclabel` — 安全标签（占位空）
    PgSeclabel,
    /// `pg_shseclabel` — 共享安全标签（占位空）
    PgShseclabel,
    /// `pg_stat_database` — 数据库统计（占位空）
    PgStatDatabase,
    /// `pg_stat_database_conflicts` — 数据库冲突统计（占位空）
    PgStatDatabaseConflicts,
    /// `pg_stat_bgwriter` — bgwriter 统计（占位空）
    PgStatBgwriter,
    /// `pg_stats` — 统计信息（占位空）
    PgStats,
    /// `pg_statistic` — 列级统计信息（P2-1.3：从 ANALYZE 收集的统计信息填充）
    PgStatistic,
    /// `pg_operator` — 操作符目录（已实现）
    PgOperator,
    /// `pg_foreign_table` — 外部表目录（占位空）
    PgForeignTable,
    /// `information_schema.routines` — 存储过程/函数元数据（占位空）
    InfoSchemaRoutines,
    /// `information_schema.parameters` — 参数元数据（占位空）
    InfoSchemaParameters,
    /// `pg_sequence` — 序列定义（已实现，从 catalog 查询）
    PgSequence,
    /// `pg_foreign_server` — 外部服务器（占位空）
    PgForeignServer,
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
    /// - `pg_database` / `pg_catalog.pg_database` → PgDatabase
    /// - `pg_namespace` / `pg_catalog.pg_namespace` → PgNamespace
    /// - `pg_class` / `pg_catalog.pg_class` → PgClass
    /// - `pg_attribute` / `pg_catalog.pg_attribute` → PgAttribute
    /// - `pg_type` / `pg_catalog.pg_type` → PgType
    /// - `pg_index` / `pg_catalog.pg_index` → PgIndex
    /// - `pg_constraint` / `pg_catalog.pg_constraint` → PgConstraint
    /// - `pg_description` / `pg_catalog.pg_description` → PgDescription
    /// - `pg_views` / `pg_catalog.pg_views` → PgViews
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
            // Navicat 兼容：pg_catalog 系统目录表（Phase 3.18）
            (Some("pg_catalog"), "pg_database") | (None, "pg_database") => Some(Self::PgDatabase),
            (Some("pg_catalog"), "pg_namespace") | (None, "pg_namespace") => {
                Some(Self::PgNamespace)
            }
            (Some("pg_catalog"), "pg_class") | (None, "pg_class") => Some(Self::PgClass),
            (Some("pg_catalog"), "pg_attribute") | (None, "pg_attribute") => {
                Some(Self::PgAttribute)
            }
            (Some("pg_catalog"), "pg_type") | (None, "pg_type") => Some(Self::PgType),
            (Some("pg_catalog"), "pg_index") | (None, "pg_index") => Some(Self::PgIndex),
            (Some("pg_catalog"), "pg_constraint") | (None, "pg_constraint") => {
                Some(Self::PgConstraint)
            }
            (Some("pg_catalog"), "pg_description") | (None, "pg_description") => {
                Some(Self::PgDescription)
            }
            (Some("pg_catalog"), "pg_views") | (None, "pg_views") => Some(Self::PgViews),
            // Navicat JOIN 兼容：用户/角色系统表（Phase 3.18 扩展）
            (Some("pg_catalog"), "pg_roles") | (None, "pg_roles") => Some(Self::PgRoles),
            (Some("pg_catalog"), "pg_shadow") | (None, "pg_shadow") => Some(Self::PgShadow),
            (Some("pg_catalog"), "pg_user") | (None, "pg_user") => Some(Self::PgUser),
            (Some("pg_catalog"), "pg_settings") | (None, "pg_settings") => Some(Self::PgSettings),
            // Navicat 兼容：表空间与统计视图（占位实现）
            (Some("pg_catalog"), "pg_tablespace") | (None, "pg_tablespace") => {
                Some(Self::PgTablespace)
            }
            (Some("pg_catalog"), "pg_stat_activity") | (None, "pg_stat_activity") => {
                Some(Self::PgStatActivity)
            }
            (Some("pg_catalog"), "pg_locks") | (None, "pg_locks") => Some(Self::PgLocks),
            (Some("pg_catalog"), "pg_matviews") | (None, "pg_matviews") => Some(Self::PgMatviews),
            (Some("pg_catalog"), "pg_rewrite") | (None, "pg_rewrite") => Some(Self::PgRewrite),
            (Some("pg_catalog"), "pg_trigger") | (None, "pg_trigger") => Some(Self::PgTrigger),
            (Some("pg_catalog"), "pg_authid") | (None, "pg_authid") => Some(Self::PgAuthid),
            (Some("pg_catalog"), "pg_proc") | (None, "pg_proc") => Some(Self::PgProc),
            (Some("pg_catalog"), "pg_db_role_setting") | (None, "pg_db_role_setting") => {
                Some(Self::PgDbRoleSetting)
            }
            (Some("pg_catalog"), "pg_default_acl") | (None, "pg_default_acl") => {
                Some(Self::PgDefaultAcl)
            }
            (Some("pg_catalog"), "pg_shdescription") | (None, "pg_shdescription") => {
                Some(Self::PgShdescription)
            }
            (Some("pg_catalog"), "pg_event_trigger") | (None, "pg_event_trigger") => {
                Some(Self::PgEventTrigger)
            }
            (Some("pg_catalog"), "pg_extension") | (None, "pg_extension") => {
                Some(Self::PgExtension)
            }
            (Some("pg_catalog"), "pg_collation") | (None, "pg_collation") => {
                Some(Self::PgCollation)
            }
            (Some("pg_catalog"), "pg_am") | (None, "pg_am") => Some(Self::PgAm),
            (Some("pg_catalog"), "pg_opclass") | (None, "pg_opclass") => Some(Self::PgOpclass),
            (Some("pg_catalog"), "pg_opfamily") | (None, "pg_opfamily") => Some(Self::PgOpfamily),
            (Some("pg_catalog"), "pg_cast") | (None, "pg_cast") => Some(Self::PgCast),
            (Some("pg_catalog"), "pg_conversion") | (None, "pg_conversion") => {
                Some(Self::PgConversion)
            }
            (Some("pg_catalog"), "pg_depend") | (None, "pg_depend") => Some(Self::PgDepend),
            (Some("pg_catalog"), "pg_shdepend") | (None, "pg_shdepend") => Some(Self::PgShdepend),
            (Some("pg_catalog"), "pg_stat_user_tables") | (None, "pg_stat_user_tables") => {
                Some(Self::PgStatUserTables)
            }
            (Some("pg_catalog"), "pg_statio_user_tables") | (None, "pg_statio_user_tables") => {
                Some(Self::PgStatioUserTables)
            }
            (Some("pg_catalog"), "pg_attrdef") | (None, "pg_attrdef") => Some(Self::PgAttrdef),
            (Some("pg_catalog"), "pg_auth_members") | (None, "pg_auth_members") => {
                Some(Self::PgAuthMembers)
            }
            (Some("pg_catalog"), "pg_policy") | (None, "pg_policy") => Some(Self::PgPolicy),
            (Some("pg_catalog"), "pg_inherits") | (None, "pg_inherits") => Some(Self::PgInherits),
            (Some("pg_catalog"), "pg_init_privs") | (None, "pg_init_privs") => {
                Some(Self::PgInitPrivs)
            }
            (Some("pg_catalog"), "pg_language") | (None, "pg_language") => Some(Self::PgLanguage),
            (Some("pg_catalog"), "pg_largeobject") | (None, "pg_largeobject") => {
                Some(Self::PgLargeobject)
            }
            (Some("pg_catalog"), "pg_largeobject_metadata") | (None, "pg_largeobject_metadata") => {
                Some(Self::PgLargeobjectMetadata)
            }
            (Some("pg_catalog"), "pg_seclabel") | (None, "pg_seclabel") => Some(Self::PgSeclabel),
            (Some("pg_catalog"), "pg_shseclabel") | (None, "pg_shseclabel") => {
                Some(Self::PgShseclabel)
            }
            (Some("pg_catalog"), "pg_stat_database") | (None, "pg_stat_database") => {
                Some(Self::PgStatDatabase)
            }
            (Some("pg_catalog"), "pg_stat_database_conflicts")
            | (None, "pg_stat_database_conflicts") => Some(Self::PgStatDatabaseConflicts),
            (Some("pg_catalog"), "pg_stat_bgwriter") | (None, "pg_stat_bgwriter") => {
                Some(Self::PgStatBgwriter)
            }
            (Some("pg_catalog"), "pg_stats") | (None, "pg_stats") => Some(Self::PgStats),
            // P2-1.3：pg_statistic 系统表（列级统计信息，从 ANALYZE 收集）
            (Some("pg_catalog"), "pg_statistic") | (None, "pg_statistic") => {
                Some(Self::PgStatistic)
            }
            // Navicat 兼容：pg_operator / pg_foreign_table（占位空）
            (Some("pg_catalog"), "pg_operator") | (None, "pg_operator") => Some(Self::PgOperator),
            (Some("pg_catalog"), "pg_foreign_table") | (None, "pg_foreign_table") => {
                Some(Self::PgForeignTable)
            }
            // Navicat 兼容：information_schema.routines / parameters（占位空）
            (Some("information_schema"), "routines") => Some(Self::InfoSchemaRoutines),
            (Some("information_schema"), "parameters") => Some(Self::InfoSchemaParameters),
            // Navicat 兼容：系统函数被解析为 Table 时也需识别（如 pg_available_extension_versions()）
            // sqlparser 会将无参数的表函数解析为 Table，这里统一处理
            (None, "pg_available_extension_versions")
            | (Some("pg_catalog"), "pg_available_extension_versions") => Some(Self::PgExtension),
            (None, "pg_available_extensions") | (Some("pg_catalog"), "pg_available_extensions") => {
                Some(Self::PgExtension)
            }
            (None, "pg_foreign_server") | (Some("pg_catalog"), "pg_foreign_server") => {
                Some(Self::PgForeignServer)
            }
            (None, "pg_sequence") | (Some("pg_catalog"), "pg_sequence") => Some(Self::PgSequence),
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
            Self::PgDatabase => szrsql_catalog::navicat::pg_database_schema(),
            Self::PgNamespace => szrsql_catalog::navicat::pg_namespace_schema(),
            Self::PgClass => szrsql_catalog::navicat::pg_class_schema(),
            Self::PgAttribute => szrsql_catalog::navicat::pg_attribute_schema(),
            Self::PgType => szrsql_catalog::navicat::pg_type_schema(),
            Self::PgIndex => szrsql_catalog::navicat::pg_index_schema(),
            Self::PgConstraint => szrsql_catalog::navicat::pg_constraint_schema(),
            Self::PgDescription => szrsql_catalog::navicat::pg_description_schema(),
            Self::PgViews => szrsql_catalog::navicat::pg_views_schema(),
            Self::PgRoles => szrsql_catalog::navicat::pg_roles_schema(),
            Self::PgShadow => szrsql_catalog::navicat::pg_shadow_schema(),
            Self::PgUser => szrsql_catalog::navicat::pg_user_schema(),
            Self::PgSettings => szrsql_catalog::navicat::pg_settings_schema(),
            Self::PgTablespace => szrsql_catalog::navicat::pg_tablespace_schema(),
            Self::PgStatActivity => szrsql_catalog::navicat::pg_stat_activity_schema(),
            Self::PgLocks => szrsql_catalog::navicat::pg_locks_schema(),
            Self::PgMatviews => szrsql_catalog::navicat::pg_matviews_schema(),
            Self::PgRewrite => szrsql_catalog::navicat::pg_rewrite_schema(),
            Self::PgTrigger => szrsql_catalog::navicat::pg_trigger_schema(),
            Self::PgAuthid => szrsql_catalog::navicat::pg_authid_schema(),
            Self::PgProc => szrsql_catalog::navicat::pg_proc_schema(),
            Self::PgDbRoleSetting => szrsql_catalog::navicat::pg_db_role_setting_schema(),
            Self::PgDefaultAcl => szrsql_catalog::navicat::pg_default_acl_schema(),
            Self::PgShdescription => szrsql_catalog::navicat::pg_shdescription_schema(),
            Self::PgEventTrigger => szrsql_catalog::navicat::pg_event_trigger_schema(),
            Self::PgExtension => szrsql_catalog::navicat::pg_extension_schema(),
            Self::PgCollation => szrsql_catalog::navicat::pg_collation_schema(),
            Self::PgAm => szrsql_catalog::navicat::pg_am_schema(),
            Self::PgOpclass => szrsql_catalog::navicat::pg_opclass_schema(),
            Self::PgOpfamily => szrsql_catalog::navicat::pg_opfamily_schema(),
            Self::PgCast => szrsql_catalog::navicat::pg_cast_schema(),
            Self::PgConversion => szrsql_catalog::navicat::pg_conversion_schema(),
            Self::PgDepend => szrsql_catalog::navicat::pg_depend_schema(),
            Self::PgShdepend => szrsql_catalog::navicat::pg_shdepend_schema(),
            Self::PgStatUserTables => szrsql_catalog::navicat::pg_stat_user_tables_schema(),
            Self::PgStatioUserTables => szrsql_catalog::navicat::pg_statio_user_tables_schema(),
            Self::PgAttrdef => szrsql_catalog::navicat::pg_attrdef_schema(),
            Self::PgAuthMembers => szrsql_catalog::navicat::pg_auth_members_schema(),
            Self::PgPolicy => szrsql_catalog::navicat::pg_policy_schema(),
            Self::PgInherits => szrsql_catalog::navicat::pg_inherits_schema(),
            Self::PgInitPrivs => szrsql_catalog::navicat::pg_init_privs_schema(),
            Self::PgLanguage => szrsql_catalog::navicat::pg_language_schema(),
            Self::PgLargeobject => szrsql_catalog::navicat::pg_largeobject_schema(),
            Self::PgLargeobjectMetadata => {
                szrsql_catalog::navicat::pg_largeobject_metadata_schema()
            }
            Self::PgSeclabel => szrsql_catalog::navicat::pg_seclabel_schema(),
            Self::PgShseclabel => szrsql_catalog::navicat::pg_shseclabel_schema(),
            Self::PgStatDatabase => szrsql_catalog::navicat::pg_stat_database_schema(),
            Self::PgStatDatabaseConflicts => {
                szrsql_catalog::navicat::pg_stat_database_conflicts_schema()
            }
            Self::PgStatBgwriter => szrsql_catalog::navicat::pg_stat_bgwriter_schema(),
            Self::PgStats => szrsql_catalog::navicat::pg_stats_schema(),
            Self::PgStatistic => szrsql_catalog::navicat::pg_statistic_schema(),
            Self::PgOperator => szrsql_catalog::navicat::pg_operator_schema(),
            Self::PgForeignTable => szrsql_catalog::navicat::pg_foreign_table_schema(),
            Self::InfoSchemaRoutines => {
                szrsql_catalog::navicat::information_schema_routines_schema()
            }
            Self::InfoSchemaParameters => {
                szrsql_catalog::navicat::information_schema_parameters_schema()
            }
            Self::PgSequence => szrsql_catalog::navicat::pg_sequence_schema(),
            Self::PgForeignServer => szrsql_catalog::navicat::pg_foreign_server_schema(),
        }
    }

    /// 返回该系统表的列名列表
    pub fn column_names(self) -> Vec<String> {
        self.schema().columns.into_iter().map(|c| c.name).collect()
    }

    /// 计算该系统表的所有行
    ///
    /// `current_db` 仅 `PgDatabase` 使用（返回当前连接的数据库名）；其他系统表忽略此参数。
    /// `stats` 为统计信息存储（P2-1.3：PgClass/PgStatistic/PgStats 使用，其他系统表忽略）。
    pub fn compute_rows(
        self,
        catalog: &dyn MutableCatalog,
        current_db: &str,
        stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
    ) -> Vec<Vec<Value>> {
        match self {
            Self::PgTables => system_tables::pg_tables(catalog),
            Self::PgIndexes => system_tables::pg_indexes(catalog),
            Self::InfoSchemaTables => information_schema::tables(catalog),
            Self::InfoSchemaColumns => information_schema::columns(catalog),
            Self::InfoSchemaTableConstraints => information_schema::table_constraints(catalog),
            Self::InfoSchemaReferentialConstraints => {
                information_schema::referential_constraints(catalog)
            }
            Self::PgDatabase => szrsql_catalog::navicat::pg_database(current_db),
            Self::PgNamespace => szrsql_catalog::navicat::pg_namespace(catalog),
            Self::PgClass => pg_class_with_stats(catalog, stats),
            Self::PgAttribute => szrsql_catalog::navicat::pg_attribute(catalog),
            Self::PgType => szrsql_catalog::navicat::pg_type(),
            Self::PgIndex => szrsql_catalog::navicat::pg_index(catalog),
            Self::PgConstraint => szrsql_catalog::navicat::pg_constraint(catalog),
            Self::PgDescription => szrsql_catalog::navicat::pg_description(catalog),
            Self::PgViews => szrsql_catalog::navicat::pg_views(catalog),
            Self::PgRoles => szrsql_catalog::navicat::pg_roles(&[]),
            Self::PgShadow => szrsql_catalog::navicat::pg_shadow(&[]),
            Self::PgUser => szrsql_catalog::navicat::pg_user(&[]),
            Self::PgSettings => szrsql_catalog::navicat::pg_settings("15.0-szrsql", &[]),
            Self::PgTablespace => szrsql_catalog::navicat::pg_tablespace(),
            Self::PgStatActivity => szrsql_catalog::navicat::pg_stat_activity(current_db),
            Self::PgLocks => szrsql_catalog::navicat::empty_rows(),
            Self::PgMatviews => szrsql_catalog::navicat::empty_rows(),
            Self::PgRewrite => szrsql_catalog::navicat::empty_rows(),
            Self::PgTrigger => szrsql_catalog::navicat::empty_rows(),
            Self::PgAuthid => szrsql_catalog::navicat::pg_authid(&[]),
            Self::PgProc => szrsql_catalog::navicat::pg_proc(),
            Self::PgDbRoleSetting => szrsql_catalog::navicat::empty_rows(),
            Self::PgDefaultAcl => szrsql_catalog::navicat::empty_rows(),
            Self::PgShdescription => szrsql_catalog::navicat::empty_rows(),
            Self::PgEventTrigger => szrsql_catalog::navicat::empty_rows(),
            Self::PgExtension => szrsql_catalog::navicat::empty_rows(),
            Self::PgCollation => szrsql_catalog::navicat::pg_collation(),
            Self::PgAm => szrsql_catalog::navicat::empty_rows(),
            Self::PgOpclass => szrsql_catalog::navicat::empty_rows(),
            Self::PgOpfamily => szrsql_catalog::navicat::empty_rows(),
            Self::PgCast => szrsql_catalog::navicat::pg_cast(),
            Self::PgConversion => szrsql_catalog::navicat::empty_rows(),
            Self::PgDepend => szrsql_catalog::navicat::empty_rows(),
            Self::PgShdepend => szrsql_catalog::navicat::empty_rows(),
            Self::PgStatUserTables => szrsql_catalog::navicat::empty_rows(),
            Self::PgStatioUserTables => szrsql_catalog::navicat::empty_rows(),
            Self::PgAttrdef => szrsql_catalog::navicat::empty_rows(),
            Self::PgAuthMembers => szrsql_catalog::navicat::empty_rows(),
            Self::PgPolicy => szrsql_catalog::navicat::empty_rows(),
            Self::PgInherits => szrsql_catalog::navicat::empty_rows(),
            Self::PgInitPrivs => szrsql_catalog::navicat::empty_rows(),
            Self::PgLanguage => szrsql_catalog::navicat::empty_rows(),
            Self::PgLargeobject => szrsql_catalog::navicat::empty_rows(),
            Self::PgLargeobjectMetadata => szrsql_catalog::navicat::empty_rows(),
            Self::PgSeclabel => szrsql_catalog::navicat::empty_rows(),
            Self::PgShseclabel => szrsql_catalog::navicat::empty_rows(),
            Self::PgStatDatabase => szrsql_catalog::navicat::empty_rows(),
            Self::PgStatDatabaseConflicts => szrsql_catalog::navicat::empty_rows(),
            Self::PgStatBgwriter => szrsql_catalog::navicat::empty_rows(),
            Self::PgStats => pg_stats_view(catalog, stats),
            Self::PgStatistic => pg_statistic_with_stats(catalog, stats),
            Self::PgOperator => szrsql_catalog::navicat::pg_operator(),
            Self::PgForeignTable => szrsql_catalog::navicat::empty_rows(),
            Self::InfoSchemaRoutines => szrsql_catalog::navicat::empty_rows(),
            Self::InfoSchemaParameters => szrsql_catalog::navicat::empty_rows(),
            Self::PgSequence => szrsql_catalog::navicat::pg_sequence(catalog),
            Self::PgForeignServer => szrsql_catalog::navicat::empty_rows(),
        }
    }
}

// =====================================================================
//  P2-1.3：统计信息感知的系统表行计算
// =====================================================================

/// 从统计信息存储构建 pg_class 行，填充 reltuples 和 relpages
///
/// # 行为
///
/// - 无 stats：返回原始 pg_class 行（reltuples=0.0, relpages=0）
/// - 有 stats：遍历 catalog 中的表，对每个有统计信息的表：
///   - `reltuples`（index 10）= row_count as f64
///   - `relpages`（index 9）= ceil(row_count / 80.0)（估算：每页 8KB，每行约 100 字节）
///
/// # 列索引
///
/// pg_class 行结构（31 列）：
/// - index 1：relname（表名）
/// - index 9：relpages
/// - index 10：reltuples
/// - index 16：relkind（"r"=表，"i"=索引）
fn pg_class_with_stats(
    catalog: &dyn MutableCatalog,
    stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
) -> Vec<Vec<Value>> {
    let mut rows = szrsql_catalog::navicat::pg_class(catalog);
    let Some(stats) = stats else {
        return rows;
    };

    // 构建 relname → TableStatistics 的查找表
    // stats 以 qualified_name().to_lowercase() 为键，但 pg_class 行中只有 relname（不含 schema）
    // 对无 schema 的表（常见情况），relname == qualified_name
    use std::sync::Arc;
    let mut stats_by_relname: std::collections::HashMap<
        String,
        Arc<szrsql_optimizer::statistics::TableStatistics>,
    > = std::collections::HashMap::new();
    for table_name in catalog.list_tables() {
        let qualified = table_name.qualified_name().to_lowercase();
        if let Some(table_stats) = stats.get_table_stats(&qualified) {
            stats_by_relname.insert(table_name.name.to_lowercase(), table_stats);
        }
    }

    // 遍历 pg_class 行，填充表行（relkind="r"）的 reltuples 和 relpages
    for row in &mut rows {
        // relname at index 1, relkind at index 16
        if let (Value::Text(name), Value::Text(kind)) = (&row[1], &row[16]) {
            if kind == "r" {
                if let Some(table_stats) = stats_by_relname.get(&name.to_lowercase()) {
                    // reltuples at index 10
                    row[10] = Value::Float64(table_stats.row_count as f64);
                    // relpages at index 9：估算每页约 80 行（8KB 页 / 100 字节每行）
                    let relpages = ((table_stats.row_count as f64) / 80.0).ceil() as i64;
                    row[9] = Value::Int64(relpages);
                }
            }
        }
    }
    rows
}

/// 从统计信息存储构建 pg_statistic 行
///
/// # 行为
///
/// - 无 stats：返回空 Vec（pg_statistic 表无数据）
/// - 有 stats：遍历 catalog 中的表，对每个有统计信息的表，
///   遍历其所有列，构建 pg_statistic 行（每列一行）
///
/// # 每列的统计信息
///
/// - `stanullfrac` = null_count / row_count（NULL 比例）
/// - `stadistinct` = distinct_count（正数=绝对值）
/// - `stavalues1` = [min_value, max_value]（文本表示，stakind1=2 histogram_bounds）
fn pg_statistic_with_stats(
    catalog: &dyn MutableCatalog,
    stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
) -> Vec<Vec<Value>> {
    let mut rows = Vec::new();
    let Some(stats) = stats else {
        return rows;
    };

    for table_name in catalog.list_tables() {
        let qualified = table_name.qualified_name().to_lowercase();
        let Some(table_stats) = stats.get_table_stats(&qualified) else {
            continue;
        };
        let table_oid = szrsql_catalog::navicat::oid_class_table(&table_name);
        let Some(table_schema) = catalog.get_table(&table_name) else {
            continue;
        };

        for (attnum, col_def) in table_schema.columns.iter().enumerate() {
            let Some(col_stats) = table_stats.column(&col_def.name) else {
                continue;
            };
            // stanullfrac = null_count / row_count（避免除零）
            let row_count = table_stats.row_count.max(1);
            let stanullfrac = col_stats.null_count as f64 / row_count as f64;
            // stadistinct：正数表示绝对值（与 PG 一致）
            let stadistinct = col_stats.distinct_count as f64;
            let row = szrsql_catalog::navicat::build_pg_statistic_row(
                table_oid,
                (attnum + 1) as i64,
                stanullfrac,
                stadistinct,
                &col_stats.min_value,
                &col_stats.max_value,
            );
            rows.push(row);
        }
    }
    rows
}

/// pg_stats 视图（pg_statistic 的可读视图）
///
/// pg_stats 列结构与 pg_statistic 不同（tablename/attname/null_frac/n_distinct 等），
/// 需要单独的 schema 和行构建逻辑。
///
/// **简化实现**：暂时返回空行（保持现状），pg_statistic 表已提供完整统计信息。
/// 后续可通过映射 pg_statistic 行到 pg_stats 视图列来实现。
fn pg_stats_view(
    _catalog: &dyn MutableCatalog,
    _stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
) -> Vec<Vec<Value>> {
    Vec::new()
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
    current_db: &str,
    stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
) -> Option<Result<QueryResult, SessionError>> {
    let select = match stmt {
        Statement::Select(s) if s.set_op.is_none() => s.as_ref(),
        _ => {
            // Navicat 兼容：UNION 查询中包含系统表（如 pg_class UNION pg_attribute）
            // 简化处理：返回空结果集，避免 table not found 错误
            if let Statement::Select(s) = stmt {
                if select_contains_system_table(s) {
                    return Some(Ok(QueryResult::ResultSet {
                        columns: vec![ResultColumn {
                            name: "count".to_string(),
                            column_type: ColumnType::Int64,
                        }],
                        rows: Vec::new(),
                        tag: "SELECT 0".to_string(),
                    }));
                }
            }
            return None;
        }
    };

    // Navicat 兼容：无 FROM 的函数查询（SELECT version() / SELECT current_database() 等）
    if select.from.is_empty() && select.projection.len() == 1 {
        if let Some(result) = try_execute_navicat_function_query(select, current_db) {
            return Some(result);
        }
    }

    // Navicat 兼容：CTE 查询 `WITH d AS (SELECT * FROM pg_database) SELECT * FROM d`
    // 如果 CTE 体本身是系统表查询，且外层 FROM 引用 CTE 名，则展开为对系统表的直接查询。
    if let Some(with) = &select.with {
        return try_execute_cte_system_table(select, with, catalog, current_db, stats);
    }

    // Navicat 兼容：含 JOIN 的系统目录查询（如 pg_class JOIN pg_namespace）。
    // 普通 Planner 不认识 pg_catalog 系统表，会报 table not found，
    // 这里拦截后用内存 hash join 执行。
    if select.from.len() == 1 && !select.from[0].joins.is_empty() {
        if contains_system_table_factor(&select.from[0].relation)
            || select.from[0]
                .joins
                .iter()
                .any(|j| contains_system_table_factor(&j.relation))
        {
            return Some(execute_system_catalog_join(
                select, catalog, current_db, stats,
            ));
        }
        return None;
    }

    // Navicat 兼容：逗号分隔的 CROSS JOIN（FROM pg_opclass opc, pg_namespace nsp WHERE ...）
    // 多个 from 项被解析为 select.from.len() > 1，需拦截处理
    if select.from.len() > 1 {
        let any_system = select
            .from
            .iter()
            .any(|t| contains_system_table_factor(&t.relation));
        if any_system {
            return Some(execute_system_catalog_cross_join(
                select, catalog, current_db, stats,
            ));
        }
        return None;
    }

    // 仅支持单表 SELECT（无 JOIN）
    if select.from.len() != 1 || !select.from[0].joins.is_empty() {
        return None;
    }
    let table_name = match &select.from[0].relation {
        TableFactor::Table { name, .. } => name,
        _ => return None,
    };

    let kind = SystemTableKind::from_name(table_name)?;

    // Navicat 兼容：GROUP BY + count(*) 聚合查询（简化支持）
    // HAVING 在分组后作为行级过滤应用（聚合表达式暂用 NULL 降级，仅 count 求值）
    if !select.group_by.is_empty() {
        return Some(execute_system_table_group_by(
            select, kind, catalog, current_db, stats,
        ));
    }
    if select.having.is_some() {
        return None;
    }

    Some(execute_system_table_select(
        select, kind, catalog, current_db, stats,
    ))
}

/// Describe 阶段推导系统表查询的结果列（不执行实际查询）。
///
/// 用于修复扩展查询协议 Describe 消息的 RowDescription 列数与实际 DataRow 列数不匹配的 bug。
///
/// # 背景
///
/// `try_describe_select_columns` 用 `Planner::plan_statement` 推导 SELECT 结果列，
/// 但 Planner 不认识系统表（pg_namespace/pg_class/information_schema.tables 等），
/// 会返回 "table not found" 错误，导致 Describe 响应发送 NoData（0 列）。
/// 而实际执行时 `try_execute_system_table_query` 拦截系统表查询并返回 N 列数据，
/// 造成 "RowDescription 列数=0 但 DataRow 列数=N" 的协议错误。
///
/// # 返回
///
/// - `Some(cols)` — 是系统表查询，返回与执行时一致的结果列描述
/// - `None` — 不是系统表查询，或查询形式过于复杂（JOIN/CTE），交由现有路径处理
pub fn try_describe_system_table_columns(
    stmt: &Statement,
    catalog: &szrsql_sql::plan::InMemoryCatalog,
    current_db: &str,
) -> Option<Vec<ResultColumn>> {
    let select = match stmt {
        Statement::Select(s) if s.set_op.is_none() => s.as_ref(),
        _ => return None,
    };

    // Navicat 兼容：无 FROM 的函数查询（SELECT version() / SELECT current_database() 等）
    // 调用 try_execute_navicat_function_query 获取完整结果，只取 columns
    if select.from.is_empty() && select.projection.len() == 1 {
        if let Some(Ok(QueryResult::ResultSet { columns, .. })) =
            try_execute_navicat_function_query(select, current_db)
        {
            return Some(columns);
        }
        return None;
    }

    // CTE 查询：列推导复杂，降级为 None（Describe 会发 NoData，但 CTE 查询通常不依赖 Describe）
    if select.with.is_some() {
        return None;
    }

    // JOIN 查询：仅支持系统表 JOIN 的列推导。
    //
    // Navicat 打开数据库时会发送 `pg_class JOIN pg_namespace` 查询表列表，
    // 执行时走 execute_system_catalog_join 返回 N 列，但 Describe 若返回 None
    // 会导致 RowDescription 列数=0 而 DataRow 列数=N 的协议错误。
    // 系统表查询是只读的，这里直接执行查询取 columns（无副作用）。
    if select.from.len() != 1 || !select.from[0].joins.is_empty() {
        let is_system_join = select.from.len() == 1
            && !select.from[0].joins.is_empty()
            && (contains_system_table_factor(&select.from[0].relation)
                || select.from[0]
                    .joins
                    .iter()
                    .any(|j| contains_system_table_factor(&j.relation)));

        if !is_system_join {
            return None;
        }

        return match execute_system_catalog_join(select, catalog, current_db, None) {
            Ok(QueryResult::ResultSet { columns, .. }) => Some(columns),
            _ => None,
        };
    }

    let table_name = match &select.from[0].relation {
        TableFactor::Table { name, .. } => name,
        _ => return None,
    };

    let kind = SystemTableKind::from_name(table_name)?;

    // GROUP BY 聚合查询：列推导复杂，降级为 None
    if !select.group_by.is_empty() || select.having.is_some() {
        return None;
    }

    // 用真实 catalog 计算 rows（project_columns 需要行数据来处理通配符展开）
    // 对 pg_namespace 等固定系统表，rows 为预定义行；对 pg_tables 等，rows 反映 catalog 内容
    // Describe 阶段不需要统计信息（仅推导列结构），传 None
    let adapter = CatalogAdapter::new(catalog);
    let schema = kind.schema();
    let rows = kind.compute_rows(&adapter, current_db, None);

    // 调用 project_columns 推导列（与执行时一致的列描述）
    let (columns, _projected_rows) = project_columns(&select.projection, &schema, &rows).ok()?;
    Some(columns)
}

/// 执行系统表的 GROUP BY 聚合查询（简化支持：count(*) + 单列分组）
///
/// 支持 `SELECT col, count(*) FROM sys_table GROUP BY col` 形式。
/// 其他聚合函数（sum/avg/max/min）暂不支持的会降级为 NULL。
fn execute_system_table_group_by(
    select: &Select,
    kind: SystemTableKind,
    catalog: &szrsql_sql::plan::InMemoryCatalog,
    current_db: &str,
    stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
) -> Result<QueryResult, SessionError> {
    let adapter = CatalogAdapter::new(catalog);
    let schema = kind.schema();
    let mut rows = kind.compute_rows(&adapter, current_db, stats);

    // 提取 GROUP BY 列索引
    let column_names: Vec<String> = schema
        .columns
        .iter()
        .map(|c| c.name.to_lowercase())
        .collect();
    let mut group_indices: Vec<usize> = Vec::new();
    for g in &select.group_by {
        let idx = extract_column_index(g, &column_names).ok_or_else(|| {
            SessionError::Protocol(format!(
                "GROUP BY column not found in system table: {:?}",
                g
            ))
        })?;
        group_indices.push(idx);
    }

    // 应用 WHERE 过滤
    if let Some(where_expr) = &select.where_clause {
        let col_names: Vec<String> = schema.columns.iter().map(|c| c.name.clone()).collect();
        rows.retain(|row| eval_where_predicate(where_expr, &col_names, row));
    }

    // 分组：用 group_indices 作为 key
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<Vec<String>, Vec<Vec<Value>>> = BTreeMap::new();
    for row in rows {
        let key: Vec<String> = group_indices
            .iter()
            .map(|&i| format!("{:?}", row.get(i).cloned().unwrap_or(Value::Null)))
            .collect();
        groups.entry(key).or_default().push(row);
    }

    // 生成结果：每个投影项
    let mut result_columns: Vec<ResultColumn> = Vec::new();
    let mut projected_rows: Vec<Vec<Value>> = Vec::new();

    for (key, group_rows) in &groups {
        let mut new_row = Vec::new();
        for item in &select.projection {
            let (expr, alias) = match item {
                SelectItem::UnnamedExpr(e) => (e, None),
                SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.clone())),
                _ => continue,
            };
            let val = match expr {
                Expr::Function { name, .. } if name.eq_ignore_ascii_case("count") => {
                    Value::Int64(group_rows.len() as i64)
                }
                Expr::Identifier(idents) => {
                    let name = idents.last().cloned().unwrap_or_default();
                    let name_lower = name.to_lowercase();
                    if let Some(idx) = column_names.iter().position(|c| *c == name_lower) {
                        group_rows
                            .first()
                            .and_then(|r| r.get(idx))
                            .cloned()
                            .unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    }
                }
                _ => Value::Null,
            };
            new_row.push(val);
            if projected_rows.is_empty() {
                let name = alias.unwrap_or_else(|| expr_display_name(expr));
                result_columns.push(ResultColumn {
                    name,
                    column_type: match expr {
                        Expr::Function { name, .. } if name.eq_ignore_ascii_case("count") => {
                            ColumnType::Int64
                        }
                        _ => ColumnType::Text,
                    },
                });
            }
        }
        let _ = key;
        projected_rows.push(new_row);
    }

    // Navicat 兼容：应用 HAVING 过滤
    // 简化策略：HAVING 中的 count(*) 替换为分组行数后求值布尔结果
    if let Some(having_expr) = &select.having {
        projected_rows.retain(|row| {
            // 构造一个虚拟行：用投影结果列替换聚合表达式
            // 简化：直接用 eval_having_predicate 求值
            eval_having_predicate(having_expr, &result_columns, row)
        });
    }

    let tag = format!("SELECT {}", projected_rows.len());
    Ok(QueryResult::ResultSet {
        columns: result_columns,
        rows: projected_rows,
        tag,
    })
}

/// 求值 HAVING 谓词（简化：支持 `count(*) > N`、`count(*) = N` 等形式）
///
/// 对 HAVING 中的 `count(*)` / `count(col)` 表达式替换为结果行中对应的 count 列值，
/// 然后用通用求值器求值布尔结果。
fn eval_having_predicate(expr: &Expr, result_columns: &[ResultColumn], row: &[Value]) -> bool {
    // 找到 count 列的索引
    let count_idx: Option<usize> = result_columns
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case("count") || c.column_type == ColumnType::Int64);

    // 将 HAVING 中的 count(*) 替换为对应列值
    fn replace_count(expr: &Expr, count_idx: Option<usize>, row: &[Value]) -> Expr {
        match expr {
            Expr::Function { name, .. } if name.eq_ignore_ascii_case("count") => {
                if let Some(idx) = count_idx {
                    if let Some(v) = row.get(idx) {
                        return Expr::Literal(v.clone());
                    }
                }
                Expr::Literal(Value::Int64(0))
            }
            Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
                left: Box::new(replace_count(left, count_idx, row)),
                op: *op,
                right: Box::new(replace_count(right, count_idx, row)),
            },
            Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
                op: *op,
                expr: Box::new(replace_count(inner, count_idx, row)),
            },
            other => other.clone(),
        }
    }

    let replaced = replace_count(expr, count_idx, row);
    let dummy_cols: Vec<String> = result_columns
        .iter()
        .map(|c| c.name.to_lowercase())
        .collect();
    let val = eval_projection_expr(&replaced, &dummy_cols, row);
    match val {
        Value::Bool(b) => b,
        Value::Null => false,
        _ => true, // 非布尔非NULL值视为真（容错）
    }
}

/// 执行 Navicat 常用的无 FROM 函数查询
///
/// Navicat 连接时会发送以下查询：
/// - `SELECT version()` — 服务器版本信息
/// - `SELECT current_database()` — 当前数据库名
/// - `SELECT current_schema()` — 当前 schema 名
/// - `SELECT current_user` — 当前用户名
/// - `SELECT current_setting('name')` — 配置参数
fn try_execute_navicat_function_query(
    select: &Select,
    current_db: &str,
) -> Option<Result<QueryResult, SessionError>> {
    use szrsql_sql::ast::SelectItem;

    let item = &select.projection[0];
    let (expr, alias) = match item {
        SelectItem::UnnamedExpr(e) => (e, None),
        SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.clone())),
        _ => return None,
    };

    // 版本号（与 main.rs 中的 server_version 一致）
    const SZRSQL_VERSION: &str = "14.0-szrsql (SzRSQL 1.0.0-rc.2)";

    let (col_name, value) = match expr {
        // SELECT version()
        Expr::Function { name, .. } if name.eq_ignore_ascii_case("version") => (
            "version".to_string(),
            Value::Text(SZRSQL_VERSION.to_string()),
        ),
        // SELECT current_database()
        Expr::Function { name, .. } if name.eq_ignore_ascii_case("current_database") => (
            "current_database".to_string(),
            Value::Text(current_db.to_string()),
        ),
        // SELECT current_schema()
        Expr::Function { name, .. } if name.eq_ignore_ascii_case("current_schema") => (
            "current_schema".to_string(),
            Value::Text("public".to_string()),
        ),
        // SELECT current_user() / SELECT current_user
        Expr::Function { name, .. } if name.eq_ignore_ascii_case("current_user") => (
            "current_user".to_string(),
            Value::Text("szrsql".to_string()),
        ),
        // SELECT current_user（标识符形式）
        Expr::Identifier(idents)
            if idents.len() == 1 && idents[0].eq_ignore_ascii_case("current_user") =>
        {
            (
                "current_user".to_string(),
                Value::Text("szrsql".to_string()),
            )
        }
        // SELECT current_setting('name')
        Expr::Function { name, args, .. }
            if name.eq_ignore_ascii_case("current_setting") && args.len() == 1 =>
        {
            let setting_name = match extract_literal(&args[0]) {
                Some(Value::Text(s)) => s.to_lowercase(),
                _ => return None,
            };
            let setting_val = match setting_name.as_str() {
                "server_version" => SZRSQL_VERSION.to_string(),
                "client_encoding" => "UTF8".to_string(),
                "server_encoding" => "UTF8".to_string(),
                "integer_datetimes" => "on".to_string(),
                "standard_conforming_strings" => "on".to_string(),
                "TimeZone" | "timezone" => "UTC".to_string(),
                "extra_float_digits" => "3".to_string(),
                _ => "".to_string(),
            };
            ("current_setting".to_string(), Value::Text(setting_val))
        }
        _ => return None,
    };

    let output_name = alias.unwrap_or(col_name);
    let column = ResultColumn {
        name: output_name,
        column_type: ColumnType::Text,
    };
    let rows = vec![vec![value]];
    let tag = "SELECT 1".to_string();
    Some(Ok(QueryResult::ResultSet {
        columns: vec![column],
        rows,
        tag,
    }))
}

/// 执行 CTE 包装的系统表查询
///
/// Navicat 会发 `WITH d AS (SELECT * FROM pg_database) SELECT * FROM d` 形式查询。
/// 这里把 CTE 体当作子查询展开：如果 CTE 体是系统表查询，外层 SELECT * FROM cte_name
/// 等价于直接执行 CTE 体内的系统表查询。
fn try_execute_cte_system_table(
    outer_select: &Select,
    with: &szrsql_sql::ast::WithClause,
    catalog: &szrsql_sql::plan::InMemoryCatalog,
    current_db: &str,
    stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
) -> Option<Result<QueryResult, SessionError>> {
    // 仅处理单 CTE、非递归
    if with.ctes.len() != 1 || with.recursive {
        return None;
    }
    let cte = &with.ctes[0];
    // CTE 体必须是 SELECT 语句，且不能嵌套 CTE
    let cte_select = cte.query.as_ref();
    if cte_select.with.is_some() {
        return None;
    }
    // CTE 体必须是系统表查询（单表，无 JOIN）
    if cte_select.from.len() != 1 || !cte_select.from[0].joins.is_empty() {
        return None;
    }
    let cte_table_name = match &cte_select.from[0].relation {
        TableFactor::Table { name, .. } => name,
        _ => return None,
    };
    let kind = SystemTableKind::from_name(cte_table_name)?;

    // Navicat 兼容：外层带 JOIN 的情况
    // 策略：把 CTE 体包装为 TableFactor::Derived，替换外层 FROM 中的 CTE 名引用，
    // 然后调用 execute_system_catalog_join 处理。
    if outer_select.from.len() == 1 && !outer_select.from[0].joins.is_empty() {
        // 仅当外层 FROM 主表或 JOIN 表引用了 CTE 名时才处理
        let cte_name = &cte.name;
        let outer_uses_cte = match &outer_select.from[0].relation {
            TableFactor::Table { name, .. } => name.name.eq_ignore_ascii_case(cte_name),
            _ => false,
        } || outer_select.from[0]
            .joins
            .iter()
            .any(|j| match &j.relation {
                TableFactor::Table { name, .. } => name.name.eq_ignore_ascii_case(cte_name),
                _ => false,
            });
        if !outer_uses_cte {
            return None;
        }
        // 构造替换后的外层 SELECT（去除 WITH 子句，FROM 中 CTE 引用替换为 Derived）
        let mut new_select = outer_select.clone();
        new_select.with = None;
        let cte_alias_name = cte_name.clone();
        let cte_subquery = cte.query.clone();
        let derived = TableFactor::Derived {
            subquery: cte_subquery,
            alias: szrsql_sql::ast::TableAlias {
                name: cte_alias_name.clone(),
                column_aliases: None,
            },
            lateral: false,
        };
        // 替换主表
        if let TableFactor::Table { name, .. } = &new_select.from[0].relation {
            if name.name.eq_ignore_ascii_case(&cte_alias_name) {
                new_select.from[0].relation = derived.clone();
            }
        }
        // 替换 JOIN 表
        for join in &mut new_select.from[0].joins {
            if let TableFactor::Table { name, .. } = &join.relation {
                if name.name.eq_ignore_ascii_case(&cte_alias_name) {
                    join.relation = derived.clone();
                }
            }
        }
        return Some(execute_system_catalog_join(
            &new_select,
            catalog,
            current_db,
            stats,
        ));
    }

    // 外层 FROM 必须是简单的 Table 引用，且名字匹配 CTE 名
    if outer_select.from.len() != 1 || !outer_select.from[0].joins.is_empty() {
        return None;
    }
    let outer_table_name = match &outer_select.from[0].relation {
        TableFactor::Table { name, .. } => &name.name,
        _ => return None,
    };
    if !outer_table_name.eq_ignore_ascii_case(&cte.name) {
        return None;
    }

    // 执行 CTE 体的系统表查询，得到原始行
    let adapter = CatalogAdapter::new(catalog);
    let schema = kind.schema();
    let column_names = kind.column_names();
    let mut rows = kind.compute_rows(&adapter, current_db, stats);

    // 应用 CTE 体的 WHERE
    if let Some(where_expr) = &cte_select.where_clause {
        rows.retain(|row| eval_where_predicate(where_expr, &column_names, row));
    }

    // 应用外层 SELECT 的投影（通常是 SELECT *）
    let (columns, mut projected_rows) =
        match project_columns(&outer_select.projection, &schema, &rows) {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };

    // 应用外层 DISTINCT
    if outer_select.distinct {
        dedup_rows(&mut projected_rows);
    }

    let tag = format!("SELECT {}", projected_rows.len());
    Some(Ok(QueryResult::ResultSet {
        columns,
        rows: projected_rows,
        tag,
    }))
}

/// 对结果行做去重（DISTINCT）
fn dedup_rows(rows: &mut Vec<Vec<Value>>) {
    let mut seen = std::collections::HashSet::new();
    rows.retain(|row| {
        // 用 Value 的 Debug 表示作为去重键（简单可靠）
        let key: String = row
            .iter()
            .map(|v| format!("{:?}", v))
            .collect::<Vec<_>>()
            .join("|");
        seen.insert(key)
    });
}

/// 执行系统表 SELECT（已通过前置检查）
fn execute_system_table_select(
    select: &Select,
    kind: SystemTableKind,
    catalog: &szrsql_sql::plan::InMemoryCatalog,
    current_db: &str,
    stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
) -> Result<QueryResult, SessionError> {
    let adapter = CatalogAdapter::new(catalog);
    let schema = kind.schema();
    let column_names = kind.column_names();
    let mut rows = kind.compute_rows(&adapter, current_db, stats);

    // 1. WHERE 过滤
    if let Some(where_expr) = &select.where_clause {
        rows.retain(|row| eval_where_predicate(where_expr, &column_names, row));
    }

    // 2. ORDER BY 排序（支持多列）
    if !select.order_by.is_empty() {
        apply_order_by_multi(&mut rows, &column_names, &select.order_by);
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
    let (columns, mut projected_rows) = project_columns(&select.projection, &schema, &rows)?;

    // 6. DISTINCT 去重
    if select.distinct {
        dedup_rows(&mut projected_rows);
    }

    let tag = format!("SELECT {}", projected_rows.len());
    Ok(QueryResult::ResultSet {
        columns,
        rows: projected_rows,
        tag,
    })
}

// =====================================================================
//  Navicat 兼容：系统目录表 JOIN 查询
// =====================================================================

/// 判断 TableFactor 是否引用系统表（pg_database/pg_namespace/pg_class 等）。
///
/// 用于拦截 Navicat 发送的 `pg_class JOIN pg_namespace ON ...` 这类元数据查询。
/// 子查询中只要 FROM 子句包含系统表即判定为系统表查询。
fn contains_system_table_factor(factor: &TableFactor) -> bool {
    match factor {
        TableFactor::Table { name, .. } => SystemTableKind::from_name(name).is_some(),
        TableFactor::Derived { subquery, .. } => {
            subquery
                .from
                .iter()
                .any(|t| contains_system_table_factor(&t.relation))
                || subquery.from.iter().any(|t| {
                    t.joins
                        .iter()
                        .any(|j| contains_system_table_factor(&j.relation))
                })
        }
        // Navicat 兼容：表函数（如 pg_available_extension_versions()）
        // 视为系统表引用，避免 table not found 错误
        TableFactor::TableFunction { name, .. } => is_system_function(name),
    }
}

/// 判断函数名是否为 PG 系统函数（返回结果集的系统函数）。
///
/// Navicat 会 JOIN 这些函数获取元数据，简化处理为空结果集。
fn is_system_function(name: &str) -> bool {
    let lower = name.to_lowercase();
    matches!(
        lower.as_str(),
        "pg_available_extension_versions"
            | "pg_available_extensions"
            | "pg_get_userbyid"
            | "pg_get_keywords"
            | "pg_settings"
            | "pg_stat_get_activity"
            | "pg_stat_get_backend_subxact"
            | "current_schemas"
            | "pg_table_is_visible"
            | "pg_total_relation_size"
            | "pg_relation_size"
            | "pg_size_pretty"
            | "pg_database_size"
            | "pg_tablespace_size"
            | "pg_relation_filenode"
            | "pg_get_expr"
            | "pg_get_viewdef"
            | "pg_get_triggerdef"
            | "pg_get_indexdef"
            | "pg_get_constraintdef"
            | "pg_get_serial_sequence"
            | "pg_get_partkeydef"
            | "col_description"
            | "obj_description"
            | "shobj_description"
            | "format_type"
            | "pg_typeof"
            | "pg_encoding_to_char"
            | "pg_char_to_encoding"
            | "pg_function_is_visible"
            | "pg_operator_is_visible"
            | "pg_opclass_is_visible"
            | "pg_collation_is_visible"
            | "pg_conversion_is_visible"
            | "pg_ts_parser_is_visible"
            | "pg_ts_dict_is_visible"
            | "pg_ts_template_is_visible"
            | "pg_ts_config_is_visible"
            | "pg_my_temp_schema"
            | "pg_is_other_temp_schema"
            | "pg_get_backend_subxact"
    )
}

/// 判断一个 SELECT 语句（可能含 UNION）是否引用了系统表。
///
/// Navicat 会发送 `SELECT COUNT(*) FROM pg_class ... UNION SELECT COUNT(*) FROM pg_attribute ...`，
/// 此类查询普通 Planner 无法处理（table not found），需在系统表拦截层返回空结果。
fn select_contains_system_table(select: &Select) -> bool {
    // 检查主 SELECT 的 FROM 子句
    for table_with_joins in &select.from {
        if contains_system_table_factor(&table_with_joins.relation) {
            return true;
        }
        for join in &table_with_joins.joins {
            if contains_system_table_factor(&join.relation) {
                return true;
            }
        }
    }
    false
}

/// 物化一个 TableFactor 的所有行（系统表或系统表子查询）。
///
/// 返回 (列名列表, 行数据)；非系统表返回 None。
fn materialize_system_table_factor(
    factor: &TableFactor,
    catalog: &szrsql_sql::plan::InMemoryCatalog,
    current_db: &str,
    stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
) -> Option<(Vec<String>, Vec<Vec<Value>>)> {
    match factor {
        TableFactor::Table { name, .. } => {
            let kind = SystemTableKind::from_name(name)?;
            let adapter = CatalogAdapter::new(catalog);
            let column_names = kind.column_names();
            let rows = kind.compute_rows(&adapter, current_db, stats);
            Some((column_names, rows))
        }
        TableFactor::Derived {
            subquery, alias, ..
        } => {
            // Navicat 兼容：子查询作为 JOIN 的左表或右表
            // 递归执行子查询（必须是系统表查询），再应用子查询自身的投影
            let sub_stmt = Statement::Select(subquery.clone());
            match try_execute_system_table_query(&sub_stmt, catalog, current_db, stats)? {
                Ok(QueryResult::ResultSet { columns, rows, .. }) => {
                    // 应用列别名（如果指定）
                    let col_names: Vec<String> = match &alias.column_aliases {
                        Some(aliases) => aliases.to_vec(),
                        None => columns.iter().map(|c| c.name.clone()).collect(),
                    };
                    Some((col_names, rows))
                }
                _ => None,
            }
        }
        // Navicat 兼容：系统表函数（如 pg_available_extension_versions()）
        // 返回空结果集，避免 JOIN 失败
        TableFactor::TableFunction { name, alias, .. } => {
            if !is_system_function(name) {
                return None;
            }
            // 为系统函数构造空结果集
            // 列名简化为 "col1", "col2", ... 或使用别名列
            let col_names: Vec<String> = match &alias {
                Some(a) if a.column_aliases.is_some() => a.column_aliases.clone().unwrap(),
                _ => vec!["col1".to_string()],
            };
            Some((col_names, Vec::new()))
        }
    }
}

/// 解析 JOIN 后的列引用（带表别名前缀）。
///
/// Navicat 查询形如 `SELECT n.nspname, c.relname FROM pg_class c JOIN pg_namespace n ON ...`，
/// 投影列和 ON 条件都用 `alias.col` 形式引用。
/// 返回 (table_alias_or_name, col_name)。
fn resolve_qualified_ident(idents: &[String]) -> Option<(&str, &str)> {
    match idents.len() {
        2 => Some((idents[0].as_str(), idents[1].as_str())),
        _ => None,
    }
}

/// 执行 Navicat 风格的系统目录 JOIN 查询。
///
/// 支持范围：
/// - INNER / LEFT JOIN（最多 3 个系统表 JOIN）
/// - ON 条件：`alias.col = alias.col`（等值连接）
/// - WHERE：`alias.col = literal`（等值过滤，AND 组合）
/// - 投影：`alias.col` 或 `alias.col AS name` 或 `*`
/// - ORDER BY 单列
///
/// 不支持：RIGHT/FULL/CROSS JOIN、子查询、聚合、GROUP BY。
/// 执行逗号分隔的 CROSS JOIN 系统目录查询。
///
/// Navicat 发送 `FROM pg_opclass opc, pg_namespace nsp WHERE opc.opcnamespace = nsp.oid`，
/// 这类逗号分隔的多表查询被解析为 select.from.len() > 1。
///
/// 简化实现：
/// 1. 物化所有 from 项（系统表）
/// 2. 笛卡尔积
/// 3. 应用 WHERE 过滤
/// 4. 应用投影
fn execute_system_catalog_cross_join(
    select: &Select,
    catalog: &szrsql_sql::plan::InMemoryCatalog,
    current_db: &str,
    stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
) -> Result<QueryResult, SessionError> {
    if select.distinct {
        return Err(SessionError::Protocol(
            "system catalog CROSS JOIN query does not support DISTINCT".into(),
        ));
    }

    // 物化所有 from 项并累积列信息
    let mut all_cols: Vec<(String, String)> = Vec::new(); // (alias, col_name)
    let mut all_rows: Vec<Vec<Value>> = vec![Vec::new()]; // 初始为 1 行 0 列

    for table_with_joins in &select.from {
        let (cols, rows) = match materialize_system_table_factor(
            &table_with_joins.relation,
            catalog,
            current_db,
            stats,
        ) {
            Some(v) => v,
            None => {
                // Navicat 兼容：无法物化的表降级为空表
                (vec!["placeholder".to_string()], Vec::new())
            }
        };
        let alias = table_factor_alias(&table_with_joins.relation);

        // 累积列
        for c in &cols {
            all_cols.push((alias.clone(), c.clone()));
        }

        // 笛卡尔积
        let mut new_rows: Vec<Vec<Value>> = Vec::new();
        for left_row in &all_rows {
            for right_row in &rows {
                let mut combined = left_row.clone();
                combined.extend(right_row.iter().cloned());
                new_rows.push(combined);
            }
        }
        all_rows = new_rows;
    }

    // 应用 WHERE 过滤（复用 JOIN 的 WHERE 求值器）
    let all_col_names: Vec<String> = all_cols
        .iter()
        .map(|(a, c)| format!("{}.{}", a, c))
        .collect();
    let mut filtered_rows = all_rows;
    if let Some(where_expr) = &select.where_clause {
        filtered_rows.retain(|row| eval_where_predicate_join(where_expr, &all_col_names, row));
    }

    // ORDER BY 多列
    if !select.order_by.is_empty() {
        apply_order_by_multi_join(&mut filtered_rows, &all_col_names, &select.order_by);
    }

    // OFFSET
    if let Some(offset_expr) = &select.offset {
        let offset = eval_literal_int(offset_expr)? as usize;
        if offset >= filtered_rows.len() {
            filtered_rows.clear();
        } else {
            filtered_rows.drain(..offset);
        }
    }
    // LIMIT
    if let Some(limit_expr) = &select.limit {
        let limit = eval_literal_int(limit_expr)? as usize;
        if limit < filtered_rows.len() {
            filtered_rows.truncate(limit);
        }
    }

    // 投影（复用 JOIN 的投影函数）
    let (result_columns, projected_rows) =
        project_join_columns(&select.projection, &all_cols, &filtered_rows)?;

    let tag = format!("SELECT {}", projected_rows.len());
    Ok(QueryResult::ResultSet {
        columns: result_columns,
        rows: projected_rows,
        tag,
    })
}

fn execute_system_catalog_join(
    select: &Select,
    catalog: &szrsql_sql::plan::InMemoryCatalog,
    current_db: &str,
    stats: Option<&dyn szrsql_optimizer::statistics::StatisticsStore>,
) -> Result<QueryResult, SessionError> {
    // Navicat 兼容：DISTINCT 暂不支持（需去重，复杂度高）
    if select.distinct {
        return Err(SessionError::Protocol(
            "system catalog JOIN query does not support DISTINCT".into(),
        ));
    }

    let from = &select.from[0];

    // 物化主表
    let (left_cols, left_rows) =
        match materialize_system_table_factor(&from.relation, catalog, current_db, stats) {
            Some(v) => v,
            None => {
                return Err(SessionError::Protocol(format!(
                    "system catalog JOIN: left relation is not a system table: {:?}",
                    from.relation
                )));
            }
        };
    // 主表别名（用于 ON/WHERE 中的别名解析）
    let left_alias = table_factor_alias(&from.relation);

    // 累积结果：(列名列表, 行数据, 表别名)
    // 初始为主表
    let mut joined_cols: Vec<(String, String)> = left_cols
        .iter()
        .map(|c| (left_alias.clone(), c.clone()))
        .collect();
    let mut joined_rows: Vec<Vec<Value>> = left_rows;

    // 依次处理每个 JOIN
    for join in &from.joins {
        // Navicat 兼容：支持 INNER/LEFT/CROSS JOIN
        if !matches!(
            join.join_type,
            JoinType::Inner | JoinType::LeftOuter | JoinType::Cross
        ) {
            return Err(SessionError::Protocol(format!(
                "system catalog JOIN only supports INNER/LEFT/CROSS JOIN, got {:?}",
                join.join_type
            )));
        }
        let (right_cols, right_rows) =
            match materialize_system_table_factor(&join.relation, catalog, current_db, stats) {
                Some(v) => v,
                None => {
                    // Navicat 兼容：无法物化的 JOIN 右表（如复杂子查询、未知系统表）
                    // 降级为空表，避免整个查询失败。LEFT JOIN 保留左表行（右表列填 NULL），
                    // INNER JOIN 结果为空。
                    let placeholder_cols = vec!["placeholder".to_string()];
                    let right_alias_tmp = table_factor_alias(&join.relation);
                    for c in &placeholder_cols {
                        joined_cols.push((right_alias_tmp.clone(), c.clone()));
                    }
                    if matches!(join.join_type, JoinType::LeftOuter) {
                        for row in &joined_rows {
                            let mut new_row = row.clone();
                            for _ in &placeholder_cols {
                                new_row.push(Value::Null);
                            }
                            // 注意：这里不能直接修改 joined_rows，需要通过 new_rows
                            // 但由于右表为空，LEFT JOIN 的结果就是左表行 + NULL
                            // 简化处理：直接跳过此 JOIN 的笛卡尔积，保留左表行
                            let _ = new_row; // 抑制未使用警告
                        }
                        // LEFT JOIN 空右表：保留左表行，右表列填 NULL
                        for row in joined_rows.iter_mut() {
                            for _ in &placeholder_cols {
                                row.push(Value::Null);
                            }
                        }
                    } else {
                        // INNER JOIN 空右表：结果为空
                        joined_rows.clear();
                    }
                    continue;
                }
            };
        let right_alias = table_factor_alias(&join.relation);

        // 解析 ON 条件：alias.col = alias.col
        let on_pairs =
            extract_join_on_pairs(&join.condition, &joined_cols, &right_cols, &right_alias)?;

        // Navicat 兼容：当 ON 条件全部降级为 extra_filters（无有效等值连接对）时，
        // 对左表每行与右表做笛卡尔积，应用 extra_filters 过滤。
        // LEFT JOIN 时若该左行无任何匹配，则填 NULL（PG 语义）。
        let mut new_rows: Vec<Vec<Value>> = Vec::new();
        if on_pairs.left_indices.is_empty() && on_pairs.right_indices.is_empty() {
            // 构造临时合并列名（用于 extra_filters 求值）
            let temp_joined_cols: Vec<(String, String)> = {
                let mut cols = joined_cols.clone();
                for c in &right_cols {
                    cols.push((right_alias.clone(), c.clone()));
                }
                cols
            };
            let temp_col_names: Vec<String> = temp_joined_cols
                .iter()
                .map(|(a, c)| format!("{}.{}", a, c))
                .collect();
            for lrow in &joined_rows {
                let mut matched = false;
                for rrow in &right_rows {
                    let mut combined = lrow.clone();
                    combined.extend(rrow.clone());
                    let passes: bool = on_pairs
                        .extra_filters
                        .iter()
                        .all(|f| eval_where_predicate_join(f, &temp_col_names, &combined));
                    if passes {
                        new_rows.push(combined);
                        matched = true;
                    }
                }
                if !matched && matches!(join.join_type, JoinType::LeftOuter | JoinType::Cross) {
                    let mut combined = lrow.clone();
                    for _ in &right_cols {
                        combined.push(Value::Null);
                    }
                    new_rows.push(combined);
                }
            }
        } else {
            // 执行 hash join：右表构建 hash，左表探测
            // hash key = 右表 on 列索引元组的 Value 序列化
            let mut right_hash: std::collections::HashMap<Vec<String>, Vec<Vec<Value>>> =
                std::collections::HashMap::new();
            for rrow in &right_rows {
                let key: Vec<String> = on_pairs
                    .right_indices
                    .iter()
                    .map(|&i| value_to_key(&rrow[i]))
                    .collect();
                right_hash.entry(key).or_default().push(rrow.clone());
            }

            for lrow in &joined_rows {
                let key: Vec<String> = on_pairs
                    .left_indices
                    .iter()
                    .map(|&i| value_to_key(&lrow[i]))
                    .collect();
                let matches = right_hash.get(&key);
                match matches {
                    Some(rs) => {
                        for rrow in rs {
                            let mut combined = lrow.clone();
                            combined.extend(rrow.clone());
                            new_rows.push(combined);
                        }
                    }
                    None => {
                        if matches!(join.join_type, JoinType::LeftOuter) {
                            let mut combined = lrow.clone();
                            // 右表列填 NULL
                            for _ in &right_cols {
                                combined.push(Value::Null);
                            }
                            new_rows.push(combined);
                        }
                    }
                }
            }
        }

        // 累加右表列
        for c in &right_cols {
            joined_cols.push((right_alias.clone(), c.clone()));
        }
        joined_rows = new_rows;

        // Navicat 兼容：应用 ON 中的非等值过滤条件（如 `t.oid > 0`）
        // 这些条件在等值连接后作为行级过滤应用。
        if !on_pairs.extra_filters.is_empty() {
            let filter_col_names: Vec<String> = joined_cols
                .iter()
                .map(|(a, c)| format!("{}.{}", a, c))
                .collect();
            for filter_expr in &on_pairs.extra_filters {
                joined_rows
                    .retain(|row| eval_where_predicate_join(filter_expr, &filter_col_names, row));
            }
        }
    }

    // 全局列名列表（带表别名前缀）
    let all_col_names: Vec<String> = joined_cols
        .iter()
        .map(|(a, c)| format!("{}.{}", a, c))
        .collect();

    // WHERE 过滤（支持 alias.col = literal 的 AND 组合）
    if let Some(where_expr) = &select.where_clause {
        joined_rows.retain(|row| eval_where_predicate_join(where_expr, &all_col_names, row));
    }

    // Navicat 兼容：JOIN + GROUP BY 聚合
    // 策略：按 GROUP BY 列分组，对每组应用聚合函数（count/sum/avg/max/min）
    // 非聚合列取组内第一行值。
    if !select.group_by.is_empty() {
        let group_indices: Vec<usize> = select
            .group_by
            .iter()
            .filter_map(|g| {
                // 解析 alias.col 形式
                if let Expr::Identifier(idents) = g {
                    if idents.len() == 2 {
                        let full =
                            format!("{}.{}", idents[0].to_lowercase(), idents[1].to_lowercase());
                        return all_col_names.iter().position(|c| c.to_lowercase() == full);
                    } else if idents.len() == 1 {
                        return all_col_names
                            .iter()
                            .position(|c| c.to_lowercase() == idents[0].to_lowercase());
                    }
                }
                None
            })
            .collect();

        if group_indices.len() != select.group_by.len() {
            return Err(SessionError::Protocol(
                "system catalog JOIN GROUP BY: 无法解析所有分组列".into(),
            ));
        }

        // 分组
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<Vec<String>, Vec<Vec<Value>>> = BTreeMap::new();
        for row in joined_rows {
            let key: Vec<String> = group_indices
                .iter()
                .map(|&i| format!("{:?}", row.get(i).cloned().unwrap_or(Value::Null)))
                .collect();
            groups.entry(key).or_default().push(row);
        }

        // 生成聚合行
        let mut agg_rows: Vec<Vec<Value>> = Vec::with_capacity(groups.len());
        for group_rows in groups.values() {
            let first_row = group_rows.first().cloned().unwrap_or_default();
            let mut new_row: Vec<Value> = Vec::new();
            for item in &select.projection {
                let expr = match item {
                    SelectItem::UnnamedExpr(e) => e,
                    SelectItem::ExprWithAlias { expr, .. } => expr,
                    _ => continue,
                };
                let val = eval_join_agg_expr(expr, &all_col_names, &first_row, group_rows);
                new_row.push(val);
            }
            agg_rows.push(new_row);
        }

        // 构造聚合后的列结构（用于 HAVING/ORDER 求值）
        let agg_cols: Vec<(String, String)> = select
            .projection
            .iter()
            .filter_map(|item| {
                let (expr, alias) = match item {
                    SelectItem::UnnamedExpr(e) => (e, None),
                    SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.clone())),
                    _ => return None,
                };
                let name = alias.unwrap_or_else(|| expr_display_name(expr));
                Some(("_agg".to_string(), name))
            })
            .collect();

        // HAVING 过滤
        if let Some(having_expr) = &select.having {
            let agg_col_names: Vec<String> =
                agg_cols.iter().map(|(_, c)| c.to_lowercase()).collect();
            agg_rows.retain(|row| eval_having_predicate_join(having_expr, &agg_col_names, row));
        }

        // 替换 joined_rows 和 joined_cols
        joined_rows = agg_rows;
        joined_cols = agg_cols;

        // 跳过普通投影，直接构造结果
        let result_columns: Vec<ResultColumn> = joined_cols
            .iter()
            .map(|(_, name)| ResultColumn {
                name: name.clone(),
                column_type: ColumnType::Text,
            })
            .collect();

        // ORDER BY 多列（聚合后）— Navicat 兼容
        if !select.order_by.is_empty() {
            let agg_col_names: Vec<String> =
                joined_cols.iter().map(|(_, c)| c.to_lowercase()).collect();
            apply_order_by_multi_join(&mut joined_rows, &agg_col_names, &select.order_by);
        }

        // OFFSET
        if let Some(offset_expr) = &select.offset {
            let offset = eval_literal_int(offset_expr)? as usize;
            if offset >= joined_rows.len() {
                joined_rows.clear();
            } else {
                joined_rows.drain(..offset);
            }
        }
        // LIMIT
        if let Some(limit_expr) = &select.limit {
            let limit = eval_literal_int(limit_expr)? as usize;
            if limit < joined_rows.len() {
                joined_rows.truncate(limit);
            }
        }

        let tag = format!("SELECT {}", joined_rows.len());
        return Ok(QueryResult::ResultSet {
            columns: result_columns,
            rows: joined_rows,
            tag,
        });
    }

    // HAVING（无 GROUP BY 时）暂不支持
    if select.having.is_some() {
        return Err(SessionError::Protocol(
            "system catalog JOIN without GROUP BY does not support HAVING".into(),
        ));
    }

    // ORDER BY 多列 — Navicat 兼容
    if !select.order_by.is_empty() {
        apply_order_by_multi_join(&mut joined_rows, &all_col_names, &select.order_by);
    }

    // OFFSET
    if let Some(offset_expr) = &select.offset {
        let offset = eval_literal_int(offset_expr)? as usize;
        if offset >= joined_rows.len() {
            joined_rows.clear();
        } else {
            joined_rows.drain(..offset);
        }
    }
    // LIMIT
    if let Some(limit_expr) = &select.limit {
        let limit = eval_literal_int(limit_expr)? as usize;
        if limit < joined_rows.len() {
            joined_rows.truncate(limit);
        }
    }

    // 投影
    let (result_columns, projected_rows) =
        project_join_columns(&select.projection, &joined_cols, &joined_rows)?;

    let tag = format!("SELECT {}", projected_rows.len());
    Ok(QueryResult::ResultSet {
        columns: result_columns,
        rows: projected_rows,
        tag,
    })
}

/// 获取 TableFactor 的别名（若无则用表名）。
fn table_factor_alias(factor: &TableFactor) -> String {
    match factor {
        TableFactor::Table {
            name,
            alias,
            system_time_as_of: _,
        } => {
            if let Some(a) = alias {
                a.name.clone()
            } else {
                name.name.clone()
            }
        }
        TableFactor::Derived { alias, .. } => alias.name.clone(),
        TableFactor::TableFunction { name, alias, .. } => {
            if let Some(a) = alias {
                a.name.clone()
            } else {
                name.clone()
            }
        }
    }
}

/// JOIN ON 条件解析结果。
struct JoinOnPairs {
    /// 左表（已 join 的累积结果）中的列索引
    left_indices: Vec<usize>,
    /// 右表中的列索引
    right_indices: Vec<usize>,
    /// ON 条件中的非等值过滤表达式（JOIN 后应用到合并行）
    extra_filters: Vec<Expr>,
}

/// 从 ON 条件中提取等值连接列对。
///
/// 支持 `a.col = b.col` 和 `a.col = b.col AND a.col2 = b.col2`（多列连接）。
fn extract_join_on_pairs(
    condition: &JoinCondition,
    left_cols: &[(String, String)],
    right_cols: &[String],
    right_alias: &str,
) -> Result<JoinOnPairs, SessionError> {
    let on_expr = match condition {
        JoinCondition::On(e) => e,
        JoinCondition::Using(cols) => {
            // USING(col): 双方都有同名列
            let mut left_indices = Vec::new();
            let mut right_indices = Vec::new();
            for c in cols {
                let li = left_cols
                    .iter()
                    .position(|(_, name)| name.eq_ignore_ascii_case(c))
                    .ok_or_else(|| {
                        SessionError::Protocol(format!(
                            "USING column '{}' not found in left side of system catalog JOIN",
                            c
                        ))
                    })?;
                let ri = right_cols
                    .iter()
                    .position(|name| name.eq_ignore_ascii_case(c))
                    .ok_or_else(|| {
                        SessionError::Protocol(format!(
                            "USING column '{}' not found in right side of system catalog JOIN",
                            c
                        ))
                    })?;
                left_indices.push(li);
                right_indices.push(ri);
            }
            return Ok(JoinOnPairs {
                left_indices,
                right_indices,
                extra_filters: Vec::new(),
            });
        }
        JoinCondition::Natural => {
            // NATURAL JOIN: 双方所有同名列
            let mut left_indices = Vec::new();
            let mut right_indices = Vec::new();
            for (li, (_, lname)) in left_cols.iter().enumerate() {
                if let Some(ri) = right_cols
                    .iter()
                    .position(|r| r.eq_ignore_ascii_case(lname))
                {
                    left_indices.push(li);
                    right_indices.push(ri);
                }
            }
            return Ok(JoinOnPairs {
                left_indices,
                right_indices,
                extra_filters: Vec::new(),
            });
        }
        JoinCondition::None => {
            // CROSS JOIN 无条件 — 返回空列对，触发笛卡尔积
            return Ok(JoinOnPairs {
                left_indices: Vec::new(),
                right_indices: Vec::new(),
                extra_filters: Vec::new(),
            });
        }
    };

    let mut left_indices = Vec::new();
    let mut right_indices = Vec::new();

    fn collect_eq(
        expr: &Expr,
        left_cols: &[(String, String)],
        right_cols: &[String],
        right_alias: &str,
        left_indices: &mut Vec<usize>,
        right_indices: &mut Vec<usize>,
        extra_filters: &mut Vec<Expr>,
    ) -> Result<(), SessionError> {
        // 支持 ON true / ON 1（CROSS JOIN 语义，笛卡尔积）
        match expr {
            Expr::Literal(Value::Bool(true)) | Expr::Literal(Value::Int64(1)) => {
                return Ok(());
            }
            _ => {}
        }
        match expr {
            Expr::BinaryOp {
                left,
                op: BinaryOp::Eq,
                right,
            } => {
                // Navicat 兼容：支持 `col = literal` 形式（如 `n.nspname = 'public'`）
                // 当一边是字面量时，整个条件作为 JOIN 后过滤，不参与等值连接。
                let left_is_literal = matches!(&**left, Expr::Literal(_));
                let right_is_literal = matches!(&**right, Expr::Literal(_));
                if left_is_literal || right_is_literal {
                    extra_filters.push(expr.clone());
                    return Ok(());
                }
                // 两边都必须是 alias.col 形式，否则降级为过滤条件
                let Some((l_alias, l_col)) = resolve_qualified_ident_col(left) else {
                    extra_filters.push(expr.clone());
                    return Ok(());
                };
                let Some((r_alias, r_col)) = resolve_qualified_ident_col(right) else {
                    extra_filters.push(expr.clone());
                    return Ok(());
                };
                // Navicat 兼容：ON 中引用的列在左右表找不到时（如 v.tableoid），
                // 不直接报错，而是将整个等值条件降级为 JOIN 后过滤条件。
                // 这样 LEFT JOIN 时左行保留并填 NULL，再由 extra_filters 行级过滤。
                let (li, ri) = if l_alias == right_alias && r_alias != right_alias {
                    // left 是右表，right 是左表
                    let Some(ri) = right_cols
                        .iter()
                        .position(|c| c.eq_ignore_ascii_case(&l_col))
                    else {
                        extra_filters.push(expr.clone());
                        return Ok(());
                    };
                    let Some(li) = find_col_in_joined(left_cols, &r_alias, &r_col) else {
                        extra_filters.push(expr.clone());
                        return Ok(());
                    };
                    (li, ri)
                } else if r_alias == right_alias && l_alias != right_alias {
                    // 常规：left 是左表，right 是右表
                    let Some(li) = find_col_in_joined(left_cols, &l_alias, &l_col) else {
                        extra_filters.push(expr.clone());
                        return Ok(());
                    };
                    let Some(ri) = right_cols
                        .iter()
                        .position(|c| c.eq_ignore_ascii_case(&r_col))
                    else {
                        extra_filters.push(expr.clone());
                        return Ok(());
                    };
                    (li, ri)
                } else {
                    // 无法判定左右表归属，降级为过滤条件
                    extra_filters.push(expr.clone());
                    return Ok(());
                };
                left_indices.push(li);
                right_indices.push(ri);
                Ok(())
            }
            Expr::BinaryOp {
                left,
                op: BinaryOp::And,
                right,
            } => {
                collect_eq(
                    left,
                    left_cols,
                    right_cols,
                    right_alias,
                    left_indices,
                    right_indices,
                    extra_filters,
                )?;
                collect_eq(
                    right,
                    left_cols,
                    right_cols,
                    right_alias,
                    left_indices,
                    right_indices,
                    extra_filters,
                )?;
                Ok(())
            }
            // Navicat 兼容：ON 中的非等值条件（如 `t.oid > 0`）降级为 JOIN 后过滤
            // 而不是直接报错——确保 Navicat 浏览功能不被阻塞。
            _ => {
                extra_filters.push(expr.clone());
                Ok(())
            }
        }
    }

    let mut extra_filters: Vec<Expr> = Vec::new();
    collect_eq(
        on_expr,
        left_cols,
        right_cols,
        right_alias,
        &mut left_indices,
        &mut right_indices,
        &mut extra_filters,
    )?;
    Ok(JoinOnPairs {
        left_indices,
        right_indices,
        extra_filters,
    })
}

/// 从表达式中解析出 (alias, col) 形式。
fn resolve_qualified_ident_col(expr: &Expr) -> Option<(String, String)> {
    match expr {
        Expr::Identifier(idents) if idents.len() == 2 => {
            Some((idents[0].clone(), idents[1].clone()))
        }
        _ => None,
    }
}

/// 在已 JOIN 的累积列中查找 `(alias, col)` 对应的索引。
fn find_col_in_joined(joined_cols: &[(String, String)], alias: &str, col: &str) -> Option<usize> {
    joined_cols
        .iter()
        .position(|(a, c)| a.eq_ignore_ascii_case(alias) && c.eq_ignore_ascii_case(col))
}

/// Value → 用于 hash join 的字符串键。
fn value_to_key(v: &Value) -> String {
    match v {
        Value::Int64(i) => format!("i:{}", i),
        Value::Text(s) => format!("t:{}", s),
        Value::Bool(b) => format!("b:{}", b),
        Value::Float64(f) => format!("f:{}", f),
        Value::Null => "n".to_string(),
        Value::Date(d) => format!("d:{}", d),
        Value::Timestamp(t) => format!("ts:{}", t),
        // 系统表不会出现这些类型，但需穷尽匹配
        _ => format!("x:{:?}", v),
    }
}

/// 评估 WHERE 谓词（JOIN 版本，支持 alias.col 形式）。
fn eval_where_predicate_join(expr: &Expr, col_names: &[String], row: &[Value]) -> bool {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOp::Eq,
            right,
        } => eval_eq_condition_join(left, right, col_names, row),
        Expr::BinaryOp {
            left,
            op: BinaryOp::And,
            right,
        } => {
            eval_where_predicate_join(left, col_names, row)
                && eval_where_predicate_join(right, col_names, row)
        }
        // 其他形式：保守返回 true
        _ => true,
    }
}

/// 评估 `alias.col = literal` 条件（JOIN 版本）。
fn eval_eq_condition_join(left: &Expr, right: &Expr, col_names: &[String], row: &[Value]) -> bool {
    let (col_idx, literal) = match (
        extract_column_index_join(left, col_names),
        extract_literal(right),
    ) {
        (Some(idx), Some(val)) => (idx, val),
        (Some(idx), None) => match extract_literal(right) {
            Some(val) => (idx, val),
            None => return true,
        },
        (None, _) => {
            match (
                extract_column_index_join(right, col_names),
                extract_literal(left),
            ) {
                (Some(idx), Some(val)) => (idx, val),
                _ => return true,
            }
        }
    };
    col_idx < row.len() && values_equal(&row[col_idx], &literal)
}

/// 从表达式中提取 JOIN 后的列索引（支持 alias.col 形式）。
fn extract_column_index_join(expr: &Expr, col_names: &[String]) -> Option<usize> {
    match expr {
        Expr::Identifier(idents) if idents.len() == 2 => {
            let qual = format!("{}.{}", idents[0].to_lowercase(), idents[1].to_lowercase());
            col_names.iter().position(|c| c.to_lowercase() == qual)
        }
        Expr::Identifier(idents) if idents.len() == 1 => {
            // 不带前缀：匹配列名的尾段
            let name = idents[0].to_lowercase();
            col_names
                .iter()
                .position(|c| c.to_lowercase().ends_with(&format!(".{}", name)))
        }
        _ => None,
    }
}

/// 应用单列 ORDER BY（JOIN 版本）。
fn apply_order_by_join(rows: &mut [Vec<Value>], col_names: &[String], order: &OrderByExpr) {
    let col_idx = match extract_column_index_join(&order.expr, col_names) {
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

/// 多列 ORDER BY（JOIN 版本）— Navicat 兼容
///
/// 按 ORDER BY 子句中的列顺序依次排序，先按第一列排序，
/// 第一列相等时按第二列排序，以此类推。支持 alias.col 形式。
fn apply_order_by_multi_join(
    rows: &mut [Vec<Value>],
    col_names: &[String],
    orders: &[OrderByExpr],
) {
    // 预解析每列的索引和升降序
    let sort_keys: Vec<(usize, bool)> = orders
        .iter()
        .filter_map(|order| {
            let idx = match &order.expr {
                Expr::Literal(Value::Int64(n)) => {
                    if *n < 1 || (*n as usize) > col_names.len() {
                        return None;
                    }
                    (*n - 1) as usize
                }
                _ => extract_column_index_join(&order.expr, col_names)?,
            };
            Some((idx, order.asc))
        })
        .collect();

    rows.sort_by(|a, b| {
        for &(idx, ascending) in &sort_keys {
            let cmp = compare_values(a.get(idx), b.get(idx));
            if cmp != std::cmp::Ordering::Equal {
                return if ascending {
                    cmp
                } else {
                    cmp.reverse()
                };
            }
        }
        std::cmp::Ordering::Equal
    });
}

/// 投影 JOIN 后的列。
fn project_join_columns(
    projection: &[SelectItem],
    joined_cols: &[(String, String)],
    rows: &[Vec<Value>],
) -> Result<(Vec<ResultColumn>, Vec<Vec<Value>>), SessionError> {
    // 构造 schema 用于投影（虚拟）
    let all_columns: Vec<ResultColumn> = joined_cols
        .iter()
        .map(|(_, name)| ResultColumn {
            name: name.clone(),
            column_type: ColumnType::Text, // 系统表大多是文本/整型，Text 兼容
        })
        .collect();

    let has_wildcard = projection
        .iter()
        .any(|p| matches!(p, SelectItem::Wildcard | SelectItem::QualifiedWildcard(_)));

    // 纯通配符单项 —— 返回对应列
    if has_wildcard && projection.len() == 1 {
        // 处理 table.* 形式
        if let Some(SelectItem::QualifiedWildcard(alias)) = projection.first() {
            let alias_lower = alias.to_lowercase();
            let mut col_indices: Vec<usize> = Vec::new();
            let mut result_columns: Vec<ResultColumn> = Vec::new();
            for (i, (a, name)) in joined_cols.iter().enumerate() {
                if a.eq_ignore_ascii_case(&alias_lower) {
                    col_indices.push(i);
                    result_columns.push(ResultColumn {
                        name: name.clone(),
                        column_type: ColumnType::Text,
                    });
                }
            }
            let projected_rows: Vec<Vec<Value>> = rows
                .iter()
                .map(|row| col_indices.iter().map(|&i| row[i].clone()).collect())
                .collect();
            return Ok((result_columns, projected_rows));
        }
        // 纯 * —— 返回所有列
        let projected_rows: Vec<Vec<Value>> = rows.to_vec();
        return Ok((all_columns, projected_rows));
    }

    // Navicat 兼容：混合通配符与表达式 —— 展开通配符后追加表达式列
    if has_wildcard {
        let column_names: Vec<String> = joined_cols.iter().map(|(_, c)| c.to_lowercase()).collect();
        let mut result_columns: Vec<ResultColumn> = Vec::new();
        // (None=通配符列索引, Some(expr)=表达式)
        let mut expr_list: Vec<Option<usize>> = Vec::new();
        let mut wildcard_indices: Vec<usize> = Vec::new();

        // 先收集所有通配符展开的列索引
        for item in projection {
            match item {
                SelectItem::Wildcard => {
                    for (i, (_, name)) in joined_cols.iter().enumerate() {
                        result_columns.push(ResultColumn {
                            name: name.clone(),
                            column_type: ColumnType::Text,
                        });
                        expr_list.push(Some(i));
                        wildcard_indices.push(i);
                    }
                }
                SelectItem::QualifiedWildcard(alias) => {
                    let alias_lower = alias.to_lowercase();
                    for (i, (a, name)) in joined_cols.iter().enumerate() {
                        if a.eq_ignore_ascii_case(&alias_lower) {
                            result_columns.push(ResultColumn {
                                name: name.clone(),
                                column_type: ColumnType::Text,
                            });
                            expr_list.push(Some(i));
                            wildcard_indices.push(i);
                        }
                    }
                }
                SelectItem::UnnamedExpr(e) => {
                    result_columns.push(ResultColumn {
                        name: expr_display_name(e),
                        column_type: ColumnType::Text,
                    });
                    expr_list.push(None);
                }
                SelectItem::ExprWithAlias { expr, alias } => {
                    result_columns.push(ResultColumn {
                        name: alias.clone(),
                        column_type: ColumnType::Text,
                    });
                    expr_list.push(None);
                    // 需要保存 expr 引用以便后续求值
                    // 使用一个 trick：把 alias 当作 key，在求值时重新解析
                    // 但更简单的方式是直接在此处内联求值
                    let _ = expr; // expr 在下方处理
                }
            }
        }

        // 对于混合模式，我们需要分别处理通配符列和表达式列
        // 重新构建：收集所有 (is_wildcard, col_idx_or_expr) 对
        let mut mixed_plan: Vec<(bool, usize, Option<&Expr>)> = Vec::new();
        for item in projection {
            match item {
                SelectItem::Wildcard => {
                    for (i, _) in joined_cols.iter().enumerate() {
                        mixed_plan.push((true, i, None));
                    }
                }
                SelectItem::QualifiedWildcard(alias) => {
                    let alias_lower = alias.to_lowercase();
                    for (i, (a, _)) in joined_cols.iter().enumerate() {
                        if a.eq_ignore_ascii_case(&alias_lower) {
                            mixed_plan.push((true, i, None));
                        }
                    }
                }
                SelectItem::UnnamedExpr(e) => {
                    mixed_plan.push((false, 0, Some(e)));
                }
                SelectItem::ExprWithAlias { expr, .. } => {
                    mixed_plan.push((false, 0, Some(expr)));
                }
            }
        }

        let mut projected_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let mut new_row = Vec::with_capacity(mixed_plan.len());
            for (is_wildcard, col_idx, opt_expr) in &mixed_plan {
                if *is_wildcard {
                    new_row.push(row.get(*col_idx).cloned().unwrap_or(Value::Null));
                } else if let Some(expr) = opt_expr {
                    new_row.push(eval_projection_expr(expr, &column_names, row));
                } else {
                    new_row.push(Value::Null);
                }
            }
            projected_rows.push(new_row);
        }
        return Ok((result_columns, projected_rows));
    }

    // JOIN 上下文的列名列表（仅列名，用于通用求值器）
    let column_names: Vec<String> = joined_cols.iter().map(|(_, c)| c.to_lowercase()).collect();

    // 预先检查 count(*) 聚合
    if projection.len() == 1 {
        if let SelectItem::UnnamedExpr(Expr::Function { name, .. }) = &projection[0] {
            if name.eq_ignore_ascii_case("count") {
                return Ok((
                    vec![ResultColumn {
                        name: "count".to_string(),
                        column_type: ColumnType::Int64,
                    }],
                    vec![vec![Value::Int64(rows.len() as i64)]],
                ));
            }
        }
    }

    let mut result_columns: Vec<ResultColumn> = Vec::with_capacity(projection.len());
    let mut expr_list: Vec<&Expr> = Vec::with_capacity(projection.len());

    for item in projection {
        let (expr, alias) = match item {
            SelectItem::UnnamedExpr(e) => (e, None),
            SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.clone())),
            _ => {
                return Err(SessionError::Protocol(
                    "unsupported projection in system catalog JOIN query".into(),
                ))
            }
        };
        let output_name = alias.unwrap_or_else(|| expr_display_name(expr));
        result_columns.push(ResultColumn {
            name: output_name,
            column_type: ColumnType::Text,
        });
        expr_list.push(expr);
    }

    let mut projected_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let mut new_row = Vec::with_capacity(expr_list.len());
        for expr in &expr_list {
            new_row.push(eval_projection_expr(expr, &column_names, row));
        }
        projected_rows.push(new_row);
    }
    Ok((result_columns, projected_rows))
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
        // col = literal
        Expr::BinaryOp {
            left,
            op: szrsql_sql::ast::BinaryOp::Eq,
            right,
        } => eval_eq_condition(left, right, column_names, row),
        // col <> literal
        Expr::BinaryOp {
            left,
            op: szrsql_sql::ast::BinaryOp::NotEq,
            right,
        } => eval_not_eq_condition(left, right, column_names, row),
        // col < / <= / > / >= literal
        Expr::BinaryOp {
            left,
            op: szrsql_sql::ast::BinaryOp::Lt,
            right,
        } => eval_cmp_condition(left, right, column_names, row, |o| {
            o == std::cmp::Ordering::Less
        }),
        Expr::BinaryOp {
            left,
            op: szrsql_sql::ast::BinaryOp::LtEq,
            right,
        } => eval_cmp_condition(left, right, column_names, row, |o| {
            o == std::cmp::Ordering::Less || o == std::cmp::Ordering::Equal
        }),
        Expr::BinaryOp {
            left,
            op: szrsql_sql::ast::BinaryOp::Gt,
            right,
        } => eval_cmp_condition(left, right, column_names, row, |o| {
            o == std::cmp::Ordering::Greater
        }),
        Expr::BinaryOp {
            left,
            op: szrsql_sql::ast::BinaryOp::GtEq,
            right,
        } => eval_cmp_condition(left, right, column_names, row, |o| {
            o == std::cmp::Ordering::Greater || o == std::cmp::Ordering::Equal
        }),
        // AND 组合
        Expr::BinaryOp {
            left,
            op: szrsql_sql::ast::BinaryOp::And,
            right,
        } => {
            eval_where_predicate(left, column_names, row)
                && eval_where_predicate(right, column_names, row)
        }
        // OR 组合
        Expr::BinaryOp {
            left,
            op: szrsql_sql::ast::BinaryOp::Or,
            right,
        } => {
            eval_where_predicate(left, column_names, row)
                || eval_where_predicate(right, column_names, row)
        }
        // [NOT] LIKE / [NOT] ILIKE pattern
        Expr::Like {
            expr,
            pattern,
            negated,
            case_insensitive,
        } => eval_like_condition(
            expr,
            pattern,
            *negated,
            *case_insensitive,
            column_names,
            row,
        ),
        // IS [NOT] NULL
        Expr::IsNull { expr, negated } => {
            let idx = extract_column_index(expr, column_names);
            match idx {
                Some(i) => {
                    let is_null = i >= row.len() || matches!(row[i], Value::Null);
                    if *negated {
                        !is_null
                    } else {
                        is_null
                    }
                }
                None => true,
            }
        }
        // expr IN (val1, val2, ...)
        Expr::InList {
            expr,
            list,
            negated,
        } => eval_in_list_condition(expr, list, *negated, column_names, row),
        // 其他形式：保守返回 true（不过滤，保证不误杀 Navicat 查询）
        _ => true,
    }
}

/// 评估 `col <> literal` 条件
fn eval_not_eq_condition(
    left: &Expr,
    right: &Expr,
    column_names: &[String],
    row: &[Value],
) -> bool {
    let (col_idx, literal) = match (
        extract_column_index(left, column_names),
        extract_literal(right),
    ) {
        (Some(idx), Some(val)) => (idx, val),
        _ => return true,
    };
    col_idx < row.len() && !values_equal(&row[col_idx], &literal)
}

/// 评估 `col < / <= / > / >= literal` 条件
///
/// `expected` 闭包接收 `compare_values` 的 Ordering 结果，返回是否匹配。
fn eval_cmp_condition<F: Fn(std::cmp::Ordering) -> bool>(
    left: &Expr,
    right: &Expr,
    column_names: &[String],
    row: &[Value],
    expected: F,
) -> bool {
    let (col_idx, literal) = match (
        extract_column_index(left, column_names),
        extract_literal(right),
    ) {
        (Some(idx), Some(val)) => (idx, val),
        _ => {
            // 尝试反向（right 是列，left 是字面量），反向时 Ordering 取反
            match (
                extract_column_index(right, column_names),
                extract_literal(left),
            ) {
                (Some(idx), Some(val)) => (idx, val),
                _ => return true,
            }
        }
    };
    if col_idx >= row.len() {
        return true;
    }
    let ord = compare_values(Some(&row[col_idx]), Some(&literal));
    // 如果是反向（right 是列），需要反转 Ordering
    // 这里简单处理：如果 left 不是列但 right 是列，反转比较结果
    let left_is_col = extract_column_index(left, column_names).is_some();
    let final_ord = if left_is_col {
        ord
    } else {
        ord.reverse()
    };
    expected(final_ord)
}

/// 评估 `[NOT] LIKE / [NOT] ILIKE pattern` 条件
///
/// Navicat 常用 `n.nspname NOT LIKE 'pg\_toast%'` 过滤系统 schema。
fn eval_like_condition(
    expr: &Expr,
    pattern: &Expr,
    negated: bool,
    case_insensitive: bool,
    column_names: &[String],
    row: &[Value],
) -> bool {
    let col_idx = match extract_column_index(expr, column_names) {
        Some(i) => i,
        None => return true,
    };
    if col_idx >= row.len() {
        return true;
    }
    let text = match &row[col_idx] {
        Value::Text(s) => s.clone(),
        Value::Int64(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return true,
    };
    let pattern_str = match extract_literal(pattern) {
        Some(Value::Text(s)) => s,
        _ => return true,
    };
    // 简单 LIKE 模式匹配：% → 任意字符序列，_ → 单个字符，\X → 字面 X
    let matched = like_match(&text, &pattern_str, case_insensitive);
    if negated {
        !matched
    } else {
        matched
    }
}

/// 简单 LIKE 模式匹配（不依赖 regex crate）
///
/// 规则：
/// - `%` 匹配任意长度字符序列（含空）
/// - `_` 匹配单个字符
/// - `\X` 匹配字面字符 X（转义）
/// - 其他字符字面匹配
fn like_match(text: &str, pattern: &str, case_insensitive: bool) -> bool {
    let text_chars: Vec<char> = if case_insensitive {
        text.to_lowercase().chars().collect()
    } else {
        text.chars().collect()
    };
    let pattern_chars: Vec<char> = if case_insensitive {
        pattern.to_lowercase().chars().collect()
    } else {
        pattern.chars().collect()
    };
    like_match_helper(&text_chars, 0, &pattern_chars, 0)
}

/// 递归匹配 LIKE 模式
fn like_match_helper(text: &[char], ti: usize, pattern: &[char], pi: usize) -> bool {
    if pi == pattern.len() {
        return ti == text.len();
    }
    match pattern[pi] {
        '\\' => {
            // 转义下一个字符（字面匹配）
            if pi + 1 >= pattern.len() {
                return ti == text.len();
            }
            if ti < text.len() && text[ti] == pattern[pi + 1] {
                like_match_helper(text, ti + 1, pattern, pi + 2)
            } else {
                false
            }
        }
        '%' => {
            // % 匹配任意长度字符序列（含空）
            // 跳过连续的 %
            let mut next_pi = pi + 1;
            while next_pi < pattern.len() && pattern[next_pi] == '%' {
                next_pi += 1;
            }
            if next_pi == pattern.len() {
                return true;
            }
            // 尝试匹配剩余模式
            for i in ti..=text.len() {
                if like_match_helper(text, i, pattern, next_pi) {
                    return true;
                }
            }
            false
        }
        '_' => {
            // _ 匹配单个字符
            if ti < text.len() {
                like_match_helper(text, ti + 1, pattern, pi + 1)
            } else {
                false
            }
        }
        c => {
            // 字面匹配
            if ti < text.len() && text[ti] == c {
                like_match_helper(text, ti + 1, pattern, pi + 1)
            } else {
                false
            }
        }
    }
}

/// 评估 `expr [NOT] IN (val1, val2, ...)` 条件
fn eval_in_list_condition(
    expr: &Expr,
    list: &[Expr],
    negated: bool,
    column_names: &[String],
    row: &[Value],
) -> bool {
    let col_idx = match extract_column_index(expr, column_names) {
        Some(i) => i,
        None => return true,
    };
    if col_idx >= row.len() {
        return true;
    }
    let cell = &row[col_idx];
    let in_list = list.iter().any(|e| {
        if let Some(v) = extract_literal(e) {
            values_equal(cell, &v)
        } else {
            false
        }
    });
    if negated {
        !in_list
    } else {
        in_list
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
///
/// 支持两种形式：
/// - 单段列名：`datname`
/// - 限定列名：`d.datname`（Navicat 兼容，取最后一段作为列名）
fn extract_column_index(expr: &Expr, column_names: &[String]) -> Option<usize> {
    match expr {
        Expr::Identifier(idents) => {
            // Navicat 兼容：单段或限定列名，均取最后一段
            if idents.len() == 1 || idents.len() == 2 {
                let name = idents[idents.len() - 1].to_lowercase();
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
    // Navicat 兼容：支持 ORDER BY <number>（按列序号排序）
    let col_idx = match &order.expr {
        Expr::Literal(Value::Int64(n)) => {
            // 列序号从 1 开始
            if *n < 1 || (*n as usize) > column_names.len() {
                return;
            }
            (*n - 1) as usize
        }
        _ => match extract_column_index(&order.expr, column_names) {
            Some(idx) => idx,
            None => return,
        },
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

/// 多列 ORDER BY 排序（Navicat 兼容）
///
/// 按 ORDER BY 子句中的列顺序依次排序，先按第一列排序，
/// 第一列相等时按第二列排序，以此类推。
fn apply_order_by_multi(rows: &mut [Vec<Value>], column_names: &[String], orders: &[OrderByExpr]) {
    // 预解析每列的索引和升降序
    let sort_keys: Vec<(usize, bool)> = orders
        .iter()
        .filter_map(|order| {
            let idx = match &order.expr {
                Expr::Literal(Value::Int64(n)) => {
                    if *n < 1 || (*n as usize) > column_names.len() {
                        return None;
                    }
                    (*n - 1) as usize
                }
                _ => extract_column_index(&order.expr, column_names)?,
            };
            Some((idx, order.asc))
        })
        .collect();

    rows.sort_by(|a, b| {
        for &(idx, ascending) in &sort_keys {
            let cmp = compare_values(a.get(idx), b.get(idx));
            if cmp != std::cmp::Ordering::Equal {
                return if ascending {
                    cmp
                } else {
                    cmp.reverse()
                };
            }
        }
        std::cmp::Ordering::Equal
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

    // 处理通配符（Navicat 兼容：支持 * 与表达式混合）
    let has_wildcard = projection
        .iter()
        .any(|p| matches!(p, SelectItem::Wildcard | SelectItem::QualifiedWildcard(_)));

    if has_wildcard && projection.len() == 1 {
        // 纯通配符 —— 返回所有列
        let projected_rows: Vec<Vec<Value>> = rows.to_vec();
        return Ok((all_columns, projected_rows));
    }

    if has_wildcard {
        // 混合通配符与表达式 —— 展开通配符后追加表达式列
        let column_names: Vec<String> = schema
            .columns
            .iter()
            .map(|c| c.name.to_lowercase())
            .collect();
        let mut result_columns: Vec<ResultColumn> = Vec::new();
        let mut expr_list: Vec<Option<&Expr>> = Vec::new(); // None = 通配符列

        for item in projection {
            match item {
                SelectItem::Wildcard | SelectItem::QualifiedWildcard(_) => {
                    for c in &schema.columns {
                        result_columns.push(ResultColumn {
                            name: c.name.clone(),
                            column_type: c.data_type.clone(),
                        });
                        expr_list.push(None);
                    }
                }
                SelectItem::UnnamedExpr(e) => {
                    result_columns.push(ResultColumn {
                        name: expr_display_name(e),
                        column_type: expr_result_type(e, schema, &column_names),
                    });
                    expr_list.push(Some(e));
                }
                SelectItem::ExprWithAlias { expr, alias } => {
                    result_columns.push(ResultColumn {
                        name: alias.clone(),
                        column_type: expr_result_type(expr, schema, &column_names),
                    });
                    expr_list.push(Some(expr));
                }
            }
        }

        let mut projected_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let mut new_row = Vec::with_capacity(expr_list.len());
            let mut col_idx = 0;
            for opt_expr in &expr_list {
                match opt_expr {
                    None => {
                        new_row.push(row.get(col_idx).cloned().unwrap_or(Value::Null));
                        col_idx += 1;
                    }
                    Some(expr) => {
                        new_row.push(eval_projection_expr(expr, &column_names, row));
                    }
                }
            }
            projected_rows.push(new_row);
        }
        return Ok((result_columns, projected_rows));
    }

    let column_names: Vec<String> = schema
        .columns
        .iter()
        .map(|c| c.name.to_lowercase())
        .collect();

    // 预先检查是否是 count(*) 聚合（整行 count）
    if projection.len() == 1 {
        if let SelectItem::UnnamedExpr(Expr::Function { name, .. }) = &projection[0] {
            if name.eq_ignore_ascii_case("count") {
                let count_val = Value::Int64(rows.len() as i64);
                return Ok((
                    vec![ResultColumn {
                        name: "count".to_string(),
                        column_type: ColumnType::Int64,
                    }],
                    vec![vec![count_val]],
                ));
            }
        }
    }

    let mut result_columns: Vec<ResultColumn> = Vec::with_capacity(projection.len());
    let mut expr_list: Vec<&Expr> = Vec::with_capacity(projection.len());
    let mut name_list: Vec<String> = Vec::with_capacity(projection.len());

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
        let output_name = alias.unwrap_or_else(|| expr_display_name(expr));
        result_columns.push(ResultColumn {
            name: output_name.clone(),
            column_type: expr_result_type(expr, schema, &column_names),
        });
        expr_list.push(expr);
        name_list.push(output_name);
    }

    let mut projected_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let mut new_row = Vec::with_capacity(expr_list.len());
        for expr in &expr_list {
            new_row.push(eval_projection_expr(expr, &column_names, row));
        }
        projected_rows.push(new_row);
    }
    Ok((result_columns, projected_rows))
}

/// 生成投影表达式的显示名称（当无 alias 时使用）
fn expr_display_name(expr: &Expr) -> String {
    match expr {
        Expr::Identifier(idents) => idents.last().cloned().unwrap_or_default(),
        Expr::Function { name, .. } => name.clone(),
        Expr::Literal(_) => "?column?".to_string(),
        _ => "?column?".to_string(),
    }
}

/// 推断投影表达式的结果类型
fn expr_result_type(expr: &Expr, schema: &TableSchema, column_names: &[String]) -> ColumnType {
    match expr {
        Expr::Identifier(idents) => {
            let name = idents.last().cloned().unwrap_or_default();
            let name_lower = name.to_lowercase();
            if let Some(idx) = column_names.iter().position(|c| *c == name_lower) {
                schema.columns[idx].data_type.clone()
            } else {
                ColumnType::Text
            }
        }
        Expr::Literal(v) => match v {
            Value::Int64(_) => ColumnType::Int64,
            Value::Float64(_) => ColumnType::Float64,
            Value::Text(_) => ColumnType::Text,
            Value::Bool(_) => ColumnType::Bool,
            Value::Null => ColumnType::Text,
            _ => ColumnType::Text,
        },
        Expr::Function { name, .. } => {
            if name.eq_ignore_ascii_case("count") {
                ColumnType::Int64
            } else {
                ColumnType::Text
            }
        }
        _ => ColumnType::Text,
    }
}

/// 通用投影表达式求值器 — 在行上下文中求值，无法处理时返回 Value::Null（永不报错）
///
/// 支持：
/// - Literal / Identifier（单段/多段）
/// - Function（count/coalesce/nullif/greatest/least/current_user/current_database/
///   version/pg_get_userbyid/format_type/array_to_string 等）
/// - Case / Cast / UnaryOp / BinaryOp（含字符串连接 ||）
/// - IsNull / InList / Like / Between（返回 Bool）
fn eval_projection_expr(expr: &Expr, column_names: &[String], row: &[Value]) -> Value {
    match expr {
        Expr::Literal(v) => v.clone(),
        Expr::Identifier(idents) => {
            let name = match idents.len() {
                1 => &idents[0],
                2 => &idents[1],
                n if n >= 3 => idents.last().unwrap(),
                _ => return Value::Null,
            };
            let name_lower = name.to_lowercase();
            column_names
                .iter()
                .position(|c| *c == name_lower)
                .map(|i| row.get(i).cloned().unwrap_or(Value::Null))
                .unwrap_or(Value::Null)
        }
        Expr::Function { name, args, .. } => {
            eval_projection_function(name, args, column_names, row)
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            for (when_expr, then_expr) in when_then {
                let matched = if let Some(op) = operand {
                    let op_val = eval_projection_expr(op, column_names, row);
                    let when_val = eval_projection_expr(when_expr, column_names, row);
                    values_equal_case(&op_val, &when_val)
                } else {
                    let v = eval_projection_expr(when_expr, column_names, row);
                    is_truthy(&v)
                };
                if matched {
                    return eval_projection_expr(then_expr, column_names, row);
                }
            }
            else_expr
                .as_ref()
                .map(|e| eval_projection_expr(e, column_names, row))
                .unwrap_or(Value::Null)
        }
        Expr::Cast { expr, .. } => eval_projection_expr(expr, column_names, row),
        Expr::UnaryOp { op, expr } => {
            let v = eval_projection_expr(expr, column_names, row);
            match (op, &v) {
                (UnaryOp::Minus, Value::Int64(n)) => Value::Int64(-n),
                (UnaryOp::Minus, Value::Float64(n)) => Value::Float64(-n),
                (UnaryOp::Plus, _) => v,
                (UnaryOp::Not, Value::Bool(b)) => Value::Bool(!b),
                (UnaryOp::BitNot, Value::Int64(n)) => Value::Int64(!n),
                _ => Value::Null,
            }
        }
        Expr::BinaryOp { left, op, right } => eval_binary_op(left, *op, right, column_names, row),
        Expr::IsNull { expr, negated } => {
            let v = eval_projection_expr(expr, column_names, row);
            let is_null = matches!(v, Value::Null);
            Value::Bool(if *negated {
                !is_null
            } else {
                is_null
            })
        }
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let v = eval_projection_expr(expr, column_names, row);
            let mut found = false;
            for item in list {
                let iv = eval_projection_expr(item, column_names, row);
                if values_equal_case(&v, &iv) {
                    found = true;
                    break;
                }
            }
            Value::Bool(if *negated {
                !found
            } else {
                found
            })
        }
        _ => Value::Null,
    }
}

/// 投影函数调用求值（支持 Navicat 常用函数）
fn eval_projection_function(
    name: &str,
    args: &[Expr],
    column_names: &[String],
    row: &[Value],
) -> Value {
    let name_lower = name.to_lowercase();
    let arg_vals: Vec<Value> = args
        .iter()
        .map(|a| eval_projection_expr(a, column_names, row))
        .collect();
    match name_lower.as_str() {
        "coalesce" => arg_vals
            .iter()
            .find(|v| !matches!(v, Value::Null))
            .cloned()
            .unwrap_or(Value::Null),
        "nullif" => {
            if arg_vals.len() == 2 && values_equal_case(&arg_vals[0], &arg_vals[1]) {
                Value::Null
            } else {
                arg_vals.first().cloned().unwrap_or(Value::Null)
            }
        }
        "greatest" => arg_vals
            .iter()
            .filter(|v| !matches!(v, Value::Null))
            .max_by(|a, b| compare_values(Some(a), Some(b)))
            .cloned()
            .unwrap_or(Value::Null),
        "least" => arg_vals
            .iter()
            .filter(|v| !matches!(v, Value::Null))
            .min_by(|a, b| compare_values(Some(a), Some(b)))
            .cloned()
            .unwrap_or(Value::Null),
        "current_user" | "user" | "session_user" | "current_role" => Value::Text("postgres".into()),
        "current_database" => Value::Text("szrsql".into()),
        "version" => Value::Text("14.0-szrsql (SzRSQL 1.0.0-rc.2)".into()),
        "pg_get_userbyid" => Value::Text("postgres".into()),
        "format_type" => arg_vals.first().cloned().unwrap_or(Value::Null),
        "array_to_string" => {
            if arg_vals.len() >= 2 {
                match (&arg_vals[0], &arg_vals[1]) {
                    (Value::Array(arr), Value::Text(sep)) => Value::Text(
                        arr.iter()
                            .filter_map(|v| match v {
                                Value::Text(s) => Some(s.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(sep),
                    ),
                    _ => Value::Text(String::new()),
                }
            } else {
                Value::Text(String::new())
            }
        }
        "pg_catalog.pg_get_userbyid" => Value::Text("postgres".into()),
        "pg_get_expr" => Value::Text(String::new()),
        "pg_get_constraintdef" => {
            // P0 Navicat 兼容修复：从同一行的 condef 列取值
            if let Some(pos) = column_names
                .iter()
                .position(|n| n.eq_ignore_ascii_case("condef"))
            {
                row.get(pos).cloned().unwrap_or(Value::Text(String::new()))
            } else {
                Value::Text(String::new())
            }
        }
        "pg_get_viewdef" | "pg_get_indexdef" | "pg_get_triggerdef" => Value::Text(String::new()),
        "to_char" | "to_number" | "to_date" | "to_timestamp" => {
            arg_vals.first().cloned().unwrap_or(Value::Null)
        }
        "length" | "char_length" | "character_length" => match arg_vals.first() {
            Some(Value::Text(s)) => Value::Int64(s.chars().count() as i64),
            _ => Value::Null,
        },
        "upper" => match arg_vals.first() {
            Some(Value::Text(s)) => Value::Text(s.to_uppercase()),
            _ => Value::Null,
        },
        "lower" => match arg_vals.first() {
            Some(Value::Text(s)) => Value::Text(s.to_lowercase()),
            _ => Value::Null,
        },
        "substring" | "substr" => match (arg_vals.first(), arg_vals.get(1), arg_vals.get(2)) {
            (Some(Value::Text(s)), Some(Value::Int64(start)), Some(Value::Int64(len))) => {
                let start = (*start - 1).max(0) as usize;
                let len = *len as usize;
                Value::Text(s.chars().skip(start).take(len).collect())
            }
            (Some(Value::Text(s)), Some(Value::Int64(start)), None) => {
                let start = (*start - 1).max(0) as usize;
                Value::Text(s.chars().skip(start).collect())
            }
            _ => Value::Null,
        },
        "replace" => {
            if arg_vals.len() == 3 {
                match (&arg_vals[0], &arg_vals[1], &arg_vals[2]) {
                    (Value::Text(s), Value::Text(from), Value::Text(to)) => {
                        Value::Text(s.replace(from, to))
                    }
                    _ => Value::Null,
                }
            } else {
                Value::Null
            }
        }
        "current_setting" => match arg_vals.first() {
            Some(Value::Text(s)) => {
                let lower = s.to_lowercase();
                pg_default_setting(&lower)
                    .map(Value::Text)
                    .unwrap_or(Value::Text(String::new()))
            }
            _ => Value::Text(String::new()),
        },
        _ => Value::Null,
    }
}

/// 获取 PG 默认设置值（与 SessionState::new() 一致）
fn pg_default_setting(name: &str) -> Option<String> {
    match name {
        "server_version" => Some("14.0-szrsql (SzRSQL 1.0.0-rc.2)".into()),
        "server_encoding" => Some("UTF8".into()),
        "client_encoding" => Some("UTF8".into()),
        "transaction_isolation" => Some("read committed".into()),
        "standard_conforming_strings" => Some("on".into()),
        "integer_datetimes" => Some("on".into()),
        "timezone" => Some("UTC".into()),
        "extra_float_digits" => Some("3".into()),
        "search_path" => Some("public".into()),
        "max_connections" => Some("100".into()),
        "application_name" => Some(String::new()),
        "datestyle" => Some("ISO, MDY".into()),
        "intervalstyle" => Some("postgres".into()),
        "lc_collate" => Some("C".into()),
        "lc_ctype" => Some("C".into()),
        "listen_addresses" => Some("*".into()),
        "wal_level" => Some("replica".into()),
        "max_wal_senders" => Some("0".into()),
        "hot_standby" => Some("off".into()),
        // 规则2：PG autocommit 返回 "on"，不是 "1" 或 "true"
        "autocommit" => Some("on".into()),
        // 规则2：PG 标准布尔系统变量返回 "on"/"off"
        "is_superuser" => Some("on".into()),
        "session_authorization" => Some("postgres".into()),
        "default_transaction_isolation" => Some("read committed".into()),
        "default_transaction_read_only" => Some("off".into()),
        "default_transaction_deferrable" => Some("off".into()),
        "tcp_keepalives_idle" => Some("0".into()),
        "tcp_keepalives_interval" => Some("0".into()),
        "tcp_keepalives_count" => Some("0".into()),
        _ => None,
    }
}

/// 二元运算求值（投影上下文）
fn eval_binary_op(
    left: &Expr,
    op: BinaryOp,
    right: &Expr,
    column_names: &[String],
    row: &[Value],
) -> Value {
    let l = eval_projection_expr(left, column_names, row);
    let r = eval_projection_expr(right, column_names, row);
    match op {
        BinaryOp::StringConcat => {
            let ls = value_to_text(&l);
            let rs = value_to_text(&r);
            Value::Text(format!("{ls}{rs}"))
        }
        BinaryOp::Plus => numeric_op(&l, &r, |a, b| a + b, |a, b| a + b),
        BinaryOp::Minus => numeric_op(&l, &r, |a, b| a - b, |a, b| a - b),
        BinaryOp::Multiply => numeric_op(&l, &r, |a, b| a * b, |a, b| a * b),
        BinaryOp::Divide => numeric_op(
            &l,
            &r,
            |a, b| {
                if b == 0 {
                    0
                } else {
                    a / b
                }
            },
            |a, b| a / b,
        ),
        BinaryOp::Modulo => numeric_op(
            &l,
            &r,
            |a, b| {
                if b == 0 {
                    0
                } else {
                    a % b
                }
            },
            |a, b| a % b,
        ),
        BinaryOp::Eq => Value::Bool(values_equal_case(&l, &r)),
        BinaryOp::NotEq => Value::Bool(!values_equal_case(&l, &r)),
        BinaryOp::Lt => Value::Bool(compare_values(Some(&l), Some(&r)) == std::cmp::Ordering::Less),
        BinaryOp::LtEq => Value::Bool(matches!(
            compare_values(Some(&l), Some(&r)),
            std::cmp::Ordering::Less | std::cmp::Ordering::Equal
        )),
        BinaryOp::Gt => {
            Value::Bool(compare_values(Some(&l), Some(&r)) == std::cmp::Ordering::Greater)
        }
        BinaryOp::GtEq => Value::Bool(matches!(
            compare_values(Some(&l), Some(&r)),
            std::cmp::Ordering::Greater | std::cmp::Ordering::Equal
        )),
        BinaryOp::And => Value::Bool(is_truthy(&l) && is_truthy(&r)),
        BinaryOp::Or => Value::Bool(is_truthy(&l) || is_truthy(&r)),
        _ => Value::Null,
    }
}

/// 数值运算辅助
fn numeric_op<F: Fn(i64, i64) -> i64, G: Fn(f64, f64) -> f64>(
    l: &Value,
    r: &Value,
    int_fn: F,
    float_fn: G,
) -> Value {
    match (l, r) {
        (Value::Int64(a), Value::Int64(b)) => Value::Int64(int_fn(*a, *b)),
        (Value::Float64(a), Value::Float64(b)) => Value::Float64(float_fn(*a, *b)),
        (Value::Int64(a), Value::Float64(b)) => Value::Float64(float_fn(*a as f64, *b)),
        (Value::Float64(a), Value::Int64(b)) => Value::Float64(float_fn(*a, *b as f64)),
        _ => Value::Null,
    }
}

/// 值转文本
fn value_to_text(v: &Value) -> String {
    match v {
        Value::Text(s) => s.clone(),
        Value::Int64(n) => n.to_string(),
        Value::Float64(n) => n.to_string(),
        Value::Bool(b) => {
            if *b {
                "t".into()
            } else {
                "f".into()
            }
        }
        Value::Null => String::new(),
        _ => format!("{:?}", v),
    }
}

/// 判断值是否为真（SQL 语义）
fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Int64(n) => *n != 0,
        Value::Float64(n) => *n != 0.0,
        Value::Text(s) => !s.is_empty() && s != "f" && s != "false",
        Value::Null => false,
        _ => false,
    }
}

/// 大小写不敏感的值相等比较（用于 CASE WHEN）
fn values_equal_case(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Int64(a), Value::Int64(b)) => a == b,
        (Value::Float64(a), Value::Float64(b)) => (a - b).abs() < f64::EPSILON,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Text(a), Value::Text(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

// =====================================================================
//  JOIN 聚合求值器
// =====================================================================

/// 求值 JOIN 后的聚合表达式（用于 JOIN + GROUP BY）
///
/// 支持的聚合函数：count(*) / count(col) / sum(col) / avg(col) / max(col) / min(col)
/// 非聚合表达式取组内第一行值（用 eval_projection_expr 求值）。
fn eval_join_agg_expr(
    expr: &Expr,
    column_names: &[String],
    first_row: &[Value],
    group_rows: &[Vec<Value>],
) -> Value {
    match expr {
        Expr::Function { name, args, .. } => {
            let lower = name.to_lowercase();
            match lower.as_str() {
                "count" => {
                    // count(*) 或 count(col) 都返回行数
                    Value::Int64(group_rows.len() as i64)
                }
                "sum" => {
                    if let Some(arg) = args.first() {
                        let mut sum: f64 = 0.0;
                        let mut is_int = true;
                        for row in group_rows {
                            let v = eval_projection_expr(arg, column_names, row);
                            match v {
                                Value::Int64(n) => sum += n as f64,
                                Value::Float64(n) => {
                                    is_int = false;
                                    sum += n;
                                }
                                Value::Null => {}
                                _ => {}
                            }
                        }
                        if is_int {
                            Value::Int64(sum as i64)
                        } else {
                            Value::Float64(sum)
                        }
                    } else {
                        Value::Null
                    }
                }
                "avg" => {
                    if let Some(arg) = args.first() {
                        let mut sum: f64 = 0.0;
                        let mut count = 0;
                        for row in group_rows {
                            let v = eval_projection_expr(arg, column_names, row);
                            match v {
                                Value::Int64(n) => {
                                    sum += n as f64;
                                    count += 1;
                                }
                                Value::Float64(n) => {
                                    sum += n;
                                    count += 1;
                                }
                                Value::Null => {}
                                _ => {}
                            }
                        }
                        if count > 0 {
                            Value::Float64(sum / count as f64)
                        } else {
                            Value::Null
                        }
                    } else {
                        Value::Null
                    }
                }
                "max" | "min" => {
                    if let Some(arg) = args.first() {
                        let mut best: Option<Value> = None;
                        for row in group_rows {
                            let v = eval_projection_expr(arg, column_names, row);
                            if matches!(v, Value::Null) {
                                continue;
                            }
                            best = Some(match best {
                                None => v,
                                Some(cur) => {
                                    let is_max = lower == "max";
                                    let replace = if is_max {
                                        compare_values_agg(&v, &cur) > 0
                                    } else {
                                        compare_values_agg(&v, &cur) < 0
                                    };
                                    if replace {
                                        v
                                    } else {
                                        cur
                                    }
                                }
                            });
                        }
                        best.unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    }
                }
                _ => eval_projection_expr(expr, column_names, first_row),
            }
        }
        _ => eval_projection_expr(expr, column_names, first_row),
    }
}

/// 比较两个 Value（用于 max/min 聚合）
fn compare_values_agg(a: &Value, b: &Value) -> i32 {
    match (a, b) {
        (Value::Int64(x), Value::Int64(y)) => (x - y).signum() as i32,
        (Value::Float64(x), Value::Float64(y)) => {
            if x > y {
                1
            } else if x < y {
                -1
            } else {
                0
            }
        }
        (Value::Int64(x), Value::Float64(y)) => {
            let xf = *x as f64;
            if xf > *y {
                1
            } else if xf < *y {
                -1
            } else {
                0
            }
        }
        (Value::Float64(x), Value::Int64(y)) => {
            let yf = *y as f64;
            if *x > yf {
                1
            } else if *x < yf {
                -1
            } else {
                0
            }
        }
        (Value::Text(x), Value::Text(y)) => x.cmp(y) as i32,
        (Value::Bool(x), Value::Bool(y)) => (*x as i32) - (*y as i32),
        _ => 0,
    }
}

/// 求值 JOIN + GROUP BY 后的 HAVING 谓词
///
/// 将 HAVING 中的聚合函数替换为聚合结果后求值布尔结果。
fn eval_having_predicate_join(expr: &Expr, column_names: &[String], row: &[Value]) -> bool {
    let val = eval_projection_expr(expr, column_names, row);
    match val {
        Value::Bool(b) => b,
        Value::Null => false,
        _ => true,
    }
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
        // pg_class 现已支持（Navicat 兼容），改用其他未知名测试
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_unknown_table")),
            None
        );
        assert_eq!(SystemTableKind::from_name(&TableName::new("users")), None);
    }

    #[test]
    fn test_system_table_kind_from_name_navicat_catalog_tables() {
        // Phase 3.18 Navicat 兼容：pg_catalog 系统目录表
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_database")),
            Some(SystemTableKind::PgDatabase)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_namespace")),
            Some(SystemTableKind::PgNamespace)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_class")),
            Some(SystemTableKind::PgClass)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_attribute")),
            Some(SystemTableKind::PgAttribute)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_type")),
            Some(SystemTableKind::PgType)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_index")),
            Some(SystemTableKind::PgIndex)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_constraint")),
            Some(SystemTableKind::PgConstraint)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_description")),
            Some(SystemTableKind::PgDescription)
        );
        assert_eq!(
            SystemTableKind::from_name(&TableName::new("pg_views")),
            Some(SystemTableKind::PgViews)
        );
        // 带 pg_catalog schema 前缀
        assert_eq!(
            SystemTableKind::from_name(&TableName::with_schema("pg_catalog", "pg_database")),
            Some(SystemTableKind::PgDatabase)
        );
    }

    #[test]
    fn test_pg_tables_compute_rows() {
        let cat = make_catalog_with_tables();
        let adapter = CatalogAdapter::new(&cat);
        let rows = SystemTableKind::PgTables.compute_rows(&adapter, "szrsql", None);
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
        let rows = SystemTableKind::InfoSchemaTables.compute_rows(&adapter, "szrsql", None);
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
        let rows = SystemTableKind::InfoSchemaColumns.compute_rows(&adapter, "szrsql", None);
        // users: 2 cols + orders: 2 cols = 4 rows
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn test_try_execute_returns_none_for_user_table() {
        let cat = make_catalog_with_tables();
        let stmts =
            szrsql_sql::parser::parse_sql("SELECT * FROM users").expect("parse should succeed");
        assert_eq!(stmts.len(), 1);
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None);
        assert!(result.is_none());
    }

    #[test]
    fn test_try_execute_returns_some_for_pg_tables() {
        let cat = make_catalog_with_tables();
        let stmts =
            szrsql_sql::parser::parse_sql("SELECT * FROM pg_tables").expect("parse should succeed");
        assert_eq!(stmts.len(), 1);
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None);
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
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
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
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
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
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
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
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
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
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { columns, rows, .. } => {
                // 12 列：ANSI SQL 标准 11 列 + szrsql 扩展 COMMENT 列
                assert_eq!(columns.len(), 12);
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
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
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
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
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
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
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
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
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
        // Navicat 兼容：系统表 JOIN 现在已被 execute_system_catalog_join 支持，
        // 不再返回 None。改为验证非系统表 JOIN 仍返回 None。
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM employees t1 JOIN employees t2 ON t1.id = t2.id";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None);
        assert!(result.is_none());
    }

    #[test]
    fn test_try_execute_system_catalog_join_basic() {
        // Navicat 兼容：pg_class JOIN pg_namespace 应成功返回结果
        let cat = make_catalog_with_tables();
        let sql = "SELECT n.nspname, c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None);
        assert!(result.is_some(), "system catalog JOIN should be handled");
        let query_result = result.unwrap().expect("JOIN should succeed");
        match query_result {
            QueryResult::ResultSet { rows, .. } => {
                assert!(
                    !rows.is_empty(),
                    "should have at least one row for user tables"
                );
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_describe_system_catalog_join_columns() {
        // Navicat 兼容：pg_class JOIN pg_namespace 的 Describe 应返回与执行时一致的列数。
        //
        // 修复前的 bug：try_describe_system_table_columns 对 JOIN 查询返回 None，
        // 导致 Describe 阶段发送 NoData（0 列），而实际执行返回 N 列，
        // asyncpg 报 "the number of columns in the result row (N) is different from what was described (0)"。
        let cat = make_catalog_with_tables();
        let sql = "SELECT n.nspname, c.relname, c.relkind FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");

        // Describe 应返回 3 列（nspname, relname, relkind）
        let cols = try_describe_system_table_columns(&stmts[0], &cat, "szrsql")
            .expect("describe should return columns for system catalog JOIN");
        assert_eq!(cols.len(), 3, "should describe 3 columns for JOIN query");

        // 执行也应返回 3 列（列数必须一致，否则协议错误）
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
            .expect("should be handled")
            .expect("should succeed");
        match result {
            QueryResult::ResultSet { columns, .. } => {
                assert_eq!(
                    columns.len(),
                    cols.len(),
                    "describe columns {} must match execute columns {}",
                    cols.len(),
                    columns.len()
                );
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_try_execute_supports_distinct() {
        // Navicat 兼容：DISTINCT 已支持（投影后去重）
        let cat = make_catalog_with_tables();
        let sql = "SELECT DISTINCT tablename FROM pg_tables";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None);
        assert!(
            result.is_some(),
            "DISTINCT should be supported for system table queries"
        );
        match result.unwrap() {
            Ok(QueryResult::ResultSet { rows, .. }) => {
                // users/orders 两张表 → DISTINCT 后 2 行
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("expected ResultSet for DISTINCT system table query"),
        }
    }

    #[test]
    fn test_try_execute_rejects_group_by() {
        // Navicat 兼容：GROUP BY 现已支持（Phase 3.18 扩展）
        let cat = make_catalog_with_tables();
        let sql = "SELECT tablename FROM pg_tables GROUP BY tablename";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None);
        assert!(result.is_some(), "GROUP BY should now be supported");
    }

    #[test]
    fn test_try_execute_with_pg_catalog_schema_prefix() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM pg_catalog.pg_tables";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None);
        assert!(result.is_some());
    }

    #[test]
    fn test_try_execute_where_case_insensitive() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT * FROM pg_tables WHERE TABLENAME = 'USERS'";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { rows, .. } => {
                assert_eq!(rows.len(), 1);
            }
            _ => panic!("expected ResultSet"),
        }
    }

    /// 复现 Navicat 报告的 Unicode 字符串字面量崩溃问题。
    ///
    /// 故障表现：当字符串字面量以非 ASCII 字符开头时（如 `'你'`、`'你好'`、`'数据库'`），
    /// 服务器在处理查询时异常关闭连接；而 ASCII 字符开头的字符串正常。
    #[test]
    fn test_unicode_literal_in_projection_chinese_first() {
        let cat = make_catalog_with_tables();
        // 单个中文字符
        let sql = "SELECT '你' FROM pg_database d";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { rows, columns, .. } => {
                assert_eq!(rows.len(), 2, "pg_database 默认有 2 行");
                assert_eq!(columns.len(), 1);
                assert_eq!(rows[0][0], Value::Text("你".into()));
                assert_eq!(rows[1][0], Value::Text("你".into()));
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_unicode_literal_in_projection_mixed_chinese_first() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT '你a' FROM pg_database d";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None)
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { rows, .. } => {
                assert_eq!(rows[0][0], Value::Text("你a".into()));
            }
            _ => panic!("expected ResultSet"),
        }
    }

    #[test]
    fn test_unicode_literal_no_from_chinese() {
        let cat = make_catalog_with_tables();
        let sql = "SELECT '数据库'";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        // 无 FROM 的字面量投影，try_execute_system_table_query 应返回 None
        // 走主路径执行器
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", None);
        // 这种情况应该返回 None（不是系统表查询）
        assert!(result.is_none(), "纯字面量查询不应由系统表路径处理");
    }

    // =================================================================
    //  P2-1.3：pg_class.reltuples + pg_statistic 系统表测试
    // =================================================================

    /// 构建 users 表的统计信息（row_count = 100，2 列）
    fn make_stats_store_for_users() -> szrsql_optimizer::statistics::InMemoryStatisticsStore {
        use std::collections::HashMap;
        use std::time::SystemTime;
        use szrsql_optimizer::statistics::{
            ColumnStatistics, InMemoryStatisticsStore, StatisticsStore, TableStatistics,
        };

        let mut column_stats = HashMap::new();
        column_stats.insert(
            "id".to_string(),
            ColumnStatistics {
                null_count: 0,
                distinct_count: 100,
                min_value: Some(Value::Int64(1)),
                max_value: Some(Value::Int64(100)),
                histogram: None,
            },
        );
        column_stats.insert(
            "name".to_string(),
            ColumnStatistics {
                null_count: 5,
                distinct_count: 95,
                min_value: Some(Value::Text("alice".into())),
                max_value: Some(Value::Text("zoe".into())),
                histogram: None,
            },
        );
        let table_stats = TableStatistics {
            table_name: "users".to_string(),
            row_count: 100,
            column_stats,
            collected_at: SystemTime::now(),
        };

        let mut store = InMemoryStatisticsStore::new();
        store.update_table_stats("users", table_stats);
        store
    }

    /// P2-1.3：ANALYZE 后查询 pg_class，验证 reltuples 从统计信息填充真实行数
    #[test]
    fn test_pg_class_reltuples_filled_from_stats() {
        let cat = make_catalog_with_tables();
        let adapter = CatalogAdapter::new(&cat);
        let store = make_stats_store_for_users();

        // 查询 pg_class，传入统计信息
        let rows = SystemTableKind::PgClass.compute_rows(&adapter, "szrsql", Some(&store));

        // 找到 users 表的行（relname="users", relkind="r"）
        let users_row = rows
            .iter()
            .find(|r| {
                matches!(&r[1], Value::Text(name) if name == "users")
                    && matches!(&r[16], Value::Text(kind) if kind == "r")
            })
            .expect("users table row should exist in pg_class");

        // reltuples at index 10 — 应为 100.0（从统计信息填充）
        match &users_row[10] {
            Value::Float64(n) => {
                assert_eq!(*n, 100.0, "reltuples should be 100.0 after ANALYZE")
            }
            other => panic!("expected Float64 for reltuples, got {:?}", other),
        }

        // relpages at index 9 — 应为 ceil(100/80) = 2
        match &users_row[9] {
            Value::Int64(n) => assert_eq!(*n, 2, "relpages should be 2 (ceil(100/80))"),
            other => panic!("expected Int64 for relpages, got {:?}", other),
        }
    }

    /// P2-1.3：无统计信息时 pg_class.reltuples 保持 0.0（兼容旧行为）
    #[test]
    fn test_pg_class_reltuples_zero_without_stats() {
        let cat = make_catalog_with_tables();
        let adapter = CatalogAdapter::new(&cat);

        // 无统计信息
        let rows = SystemTableKind::PgClass.compute_rows(&adapter, "szrsql", None);

        let users_row = rows
            .iter()
            .find(|r| {
                matches!(&r[1], Value::Text(name) if name == "users")
                    && matches!(&r[16], Value::Text(kind) if kind == "r")
            })
            .expect("users table row should exist");

        // reltuples 应为 0.0（无统计信息时保持默认）
        match &users_row[10] {
            Value::Float64(n) => assert_eq!(*n, 0.0, "reltuples should be 0.0 without stats"),
            other => panic!("expected Float64 for reltuples, got {:?}", other),
        }
    }

    /// P2-1.3：ANALYZE 后查询 pg_statistic，验证返回行包含列统计信息
    #[test]
    fn test_pg_statistic_returns_column_stats() {
        let cat = make_catalog_with_tables();
        let adapter = CatalogAdapter::new(&cat);
        let store = make_stats_store_for_users();

        // 查询 pg_statistic
        let rows = SystemTableKind::PgStatistic.compute_rows(&adapter, "szrsql", Some(&store));

        // users 表有 2 列（id, name），所以应返回 2 行
        assert_eq!(
            rows.len(),
            2,
            "pg_statistic should have 2 rows for users table (2 columns)"
        );

        // 验证第一行（id 列，staattnum=1）
        let id_row = &rows[0];
        // staattnum at index 1
        match &id_row[1] {
            Value::Int64(n) => assert_eq!(*n, 1, "staattnum should be 1 for id column"),
            other => panic!("expected Int64 for staattnum, got {:?}", other),
        }
        // stanullfrac at index 3 (0/100 = 0.0)
        match &id_row[3] {
            Value::Float64(n) => {
                assert!(
                    (n - 0.0).abs() < f64::EPSILON,
                    "stanullfrac should be 0.0 for id column"
                )
            }
            other => panic!("expected Float64 for stanullfrac, got {:?}", other),
        }
        // stadistinct at index 4 (100.0)
        match &id_row[4] {
            Value::Float64(n) => {
                assert!(
                    (n - 100.0).abs() < f64::EPSILON,
                    "stadistinct should be 100.0 for id column"
                )
            }
            other => panic!("expected Float64 for stadistinct, got {:?}", other),
        }

        // 验证第二行（name 列，staattnum=2）
        let name_row = &rows[1];
        match &name_row[1] {
            Value::Int64(n) => assert_eq!(*n, 2, "staattnum should be 2 for name column"),
            other => panic!("expected Int64 for staattnum, got {:?}", other),
        }
        // stanullfrac at index 3 (5/100 = 0.05)
        match &name_row[3] {
            Value::Float64(n) => {
                assert!(
                    (n - 0.05).abs() < 1e-9,
                    "stanullfrac should be 0.05 for name column (5 nulls / 100 rows)"
                )
            }
            other => panic!("expected Float64 for stanullfrac, got {:?}", other),
        }
    }

    /// P2-1.3：未 ANALYZE 时查询 pg_statistic 返回空
    #[test]
    fn test_pg_statistic_empty_without_analyze() {
        let cat = make_catalog_with_tables();
        let adapter = CatalogAdapter::new(&cat);

        // 无统计信息时，pg_statistic 应返回空
        let rows = SystemTableKind::PgStatistic.compute_rows(&adapter, "szrsql", None);
        assert!(
            rows.is_empty(),
            "pg_statistic should be empty without ANALYZE"
        );
    }

    /// P2-1.3：通过 SQL 查询 pg_statistic 验证完整流程
    #[test]
    fn test_try_execute_pg_statistic_query() {
        let cat = make_catalog_with_tables();
        let store = make_stats_store_for_users();
        let sql = "SELECT starelid, staattnum, stanullfrac, stadistinct FROM pg_statistic";
        let stmts = szrsql_sql::parser::parse_sql(sql).expect("parse should succeed");
        let result = try_execute_system_table_query(&stmts[0], &cat, "szrsql", Some(&store))
            .expect("should be Some")
            .expect("should be Ok");
        match result {
            QueryResult::ResultSet { columns, rows, .. } => {
                assert_eq!(columns.len(), 4);
                assert_eq!(rows.len(), 2, "users table has 2 columns");
            }
            _ => panic!("expected ResultSet"),
        }
    }
}
