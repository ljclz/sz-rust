//! SzRSQL Schema 版本注册表 — 对应 `SzRSQL实施进度.md` Phase 2.5.6。
//!
//! 跟踪每个表的 Schema 历史版本，支持按 LSN 查询历史 Schema，用于 CDC 场景下
//! 解码 WAL 记录时还原正确的列结构。
//!
//! # 核心概念
//!
//! - **SchemaVersion**：一个表在某段时间内有效的 Schema 快照，含 `start_lsn` 和 `end_lsn`
//! - **SchemaVersionRegistry**：按表名组织的版本注册表，支持按 LSN 查询历史版本
//! - **SchemaDiff**：两个 Schema 之间的差异（新增列/删除列/修改列）
//! - **CompatibilityLevel**：Schema 兼容性级别（Backward/Forward/Full/None）
//!
//! # 设计要点
//!
//! 1. **按 LSN 索引**：每个版本绑定 `start_lsn`，前一个版本的 `end_lsn` 在新版本插入时被填充
//!    - 查询 `get_schema_at(table, lsn)` 返回 `start_lsn <= lsn < end_lsn` 的版本
//!    - 当前版本（最新）的 `end_lsn` 为 `None`，表示仍然有效
//! 2. **全局 version_id**：所有表共享一个递增计数器，确保版本 ID 在整个注册表中唯一
//! 3. **兼容性检查**：基于列名集合和列约束的差异判断（参考 Phase 2.5.5 AVRO Schema Registry）
//! 4. **线程安全**：内部使用 `RwLock<HashMap>` + `AtomicU32`，支持并发读、互斥写
//! 5. **与 Phase 2.5.5 SchemaRegistry 的区别**：
//!    - Phase 2.5.5 `SchemaRegistry`（szrsql-cdc）：管理 AVRO schema 字符串，按 subject+version 索引
//!    - Phase 2.5.6 `SchemaVersionRegistry`（szrsql-tx）：管理表结构 Schema，按 table+LSN 索引

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, RwLock};
use szrsql_types::schema::{ColumnDef, Schema};

// =====================================================================
// 兼容性级别
// =====================================================================

/// Schema 兼容性级别 — 控制 DDL 变更时是否允许破坏性变更
///
/// - **Backward**（向后兼容）：新 schema 能读取旧 schema 写的数据
///   - 即：新 schema 可以新增有 default 的列，但不能新增无 default 的必需列（NOT NULL）
///   - 旧消费者读取新数据时，新增列被忽略
/// - **Forward**（向前兼容）：旧 schema 能读取新 schema 写的数据
///   - 即：新 schema 可以删除列，但不能新增无 default 的必需列
///   - 新消费者读取旧数据时，被删除的列返回 NULL（或 default）
/// - **Full**（双向兼容）：同时满足 Backward 和 Forward
/// - **None**：不检查兼容性，允许任意 DDL 变更
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompatibilityLevel {
    /// 向后兼容：新 schema 能读取旧数据
    Backward,
    /// 向前兼容：旧 schema 能读取新数据
    Forward,
    /// 双向兼容
    Full,
    /// 不检查兼容性
    None,
}

// =====================================================================
// Schema 差异
// =====================================================================

/// 两个 Schema 之间的差异
///
/// 用于兼容性检查和审计日志，记录 DDL 变更的具体内容。
///
/// **注**：不 derive `Eq`，因为 `ColumnDef` 包含 `Option<Value>` 字段，
/// `Value` 有 `Float64` 变体不实现 `Eq`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaDiff {
    /// 新增列（在新 schema 中存在，旧 schema 中不存在）
    pub added_columns: Vec<ColumnDef>,
    /// 删除列的名称（在旧 schema 中存在，新 schema 中不存在）
    pub removed_columns: Vec<String>,
    /// 修改的列（列名相同，但类型或约束不同）
    /// 元组：(列名, 旧列定义, 新列定义)
    pub modified_columns: Vec<(String, ColumnDef, ColumnDef)>,
}

impl SchemaDiff {
    /// 创建一个空的 SchemaDiff（无差异）
    pub fn empty() -> Self {
        Self {
            added_columns: Vec::new(),
            removed_columns: Vec::new(),
            modified_columns: Vec::new(),
        }
    }

    /// 是否无差异
    pub fn is_empty(&self) -> bool {
        self.added_columns.is_empty()
            && self.removed_columns.is_empty()
            && self.modified_columns.is_empty()
    }

    /// 差异总数
    pub fn change_count(&self) -> usize {
        self.added_columns.len() + self.removed_columns.len() + self.modified_columns.len()
    }
}

/// 计算两个 Schema 之间的差异
///
/// **比较规则**：
/// - 按列名匹配（不依赖列顺序）
/// - 列名相同但 col_type/not_null/default 等属性不同 → 修改
/// - 列名仅在新 schema 中 → 新增
/// - 列名仅在旧 schema 中 → 删除
pub fn diff_schemas(old: &Schema, new: &Schema) -> SchemaDiff {
    let mut added_columns = Vec::new();
    let mut removed_columns = Vec::new();
    let mut modified_columns = Vec::new();

    // 找出新增和修改的列
    for new_col in &new.columns {
        match old.columns.iter().find(|c| c.name == new_col.name) {
            None => added_columns.push(new_col.clone()),
            Some(old_col) => {
                if old_col != new_col {
                    modified_columns.push((new_col.name.clone(), old_col.clone(), new_col.clone()));
                }
            }
        }
    }

    // 找出删除的列
    for old_col in &old.columns {
        if !new.columns.iter().any(|c| c.name == old_col.name) {
            removed_columns.push(old_col.name.clone());
        }
    }

    SchemaDiff {
        added_columns,
        removed_columns,
        modified_columns,
    }
}

// =====================================================================
// Schema 版本
// =====================================================================

/// 表 Schema 的一个历史版本
///
/// **生命周期**：
/// - `start_lsn`：本版本生效的起始 LSN（含），即 DDL 变更所在 WAL 记录的 LSN
/// - `end_lsn`：本版本失效的 LSN（不含），即下一个 DDL 变更的 LSN
///   - `None` 表示本版本仍为当前版本（最新）
///   - `Some(x)` 表示本版本在 LSN `x` 被下一个版本取代
///
/// **查询规则**：`get_schema_at(table, lsn)` 返回满足 `start_lsn <= lsn < end_lsn` 的版本
///
/// **注**：不 derive `Eq`，因为 `Schema` 包含 `ColumnDef`，`ColumnDef` 包含 `Option<Value>`，
/// `Value` 有 `Float64` 变体不实现 `Eq`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaVersion {
    /// 全局唯一的版本 ID（递增分配）
    pub version_id: u32,
    /// 表名
    pub table_name: String,
    /// 本版本生效的起始 LSN（含）
    pub start_lsn: u64,
    /// 本版本失效的 LSN（不含）；None 表示当前版本
    pub end_lsn: Option<u64>,
    /// Schema 定义
    pub schema: Schema,
    /// 创建时间戳（Unix 毫秒，便于审计）
    pub created_at: u64,
}

impl SchemaVersion {
    /// 判断本版本在指定 LSN 时是否有效
    ///
    /// 有效条件：`start_lsn <= lsn` 且 (`end_lsn` 为 None 或 `lsn < end_lsn`)
    pub fn is_valid_at(&self, lsn: u64) -> bool {
        if lsn < self.start_lsn {
            return false;
        }
        match self.end_lsn {
            None => true,
            Some(end) => lsn < end,
        }
    }

    /// 本版本是否为当前版本（未被取代）
    pub fn is_current(&self) -> bool {
        self.end_lsn.is_none()
    }

    /// 本版本的有效 LSN 区间长度（None 表示无上限）
    pub fn lsn_span(&self) -> Option<u64> {
        self.end_lsn.map(|end| end - self.start_lsn)
    }
}

// =====================================================================
// Schema 版本注册表
// =====================================================================

/// Schema 版本注册表错误
#[derive(Debug, thiserror::Error)]
pub enum SchemaRegistryError {
    /// 表未找到
    #[error("table not found: {0}")]
    TableNotFound(String),
    /// 指定 LSN 处无有效版本
    #[error("no schema version for table {table} at lsn {lsn}")]
    VersionNotFound { table: String, lsn: u64 },
    /// 兼容性检查失败
    #[error("schema is not compatible: {0}")]
    Incompatible(String),
    /// LSN 倒退（新 DDL 的 LSN 小于上一个版本的 start_lsn）
    #[error("lsn regression: new lsn {new_lsn} < last version start_lsn {last_start_lsn}")]
    LsnRegression { new_lsn: u64, last_start_lsn: u64 },
}

/// Schema 版本注册表 — 按 LSN 索引的表 Schema 历史版本
///
/// **设计**：
/// - `versions: RwLock<HashMap<String, Vec<SchemaVersion>>>`：按表名组织，每个表的版本按 `start_lsn` 升序
/// - `next_version_id: AtomicU32`：全局递增的版本 ID 计数器，确保所有表的版本 ID 唯一
/// - `compatibility: Mutex<CompatibilityLevel>`：默认兼容性级别，`bump_version` 时使用
///
/// **API**：
/// - `bump_version(table, schema, lsn)`：DDL 变更时调用，创建新版本，标记前一版本 end_lsn
/// - `get_schema_at(table, lsn)`：按 LSN 查询历史 Schema（CDC 解码时使用）
/// - `get_version_at(table, lsn)`：按 LSN 查询版本元信息
/// - `latest_version(table)`：获取当前版本
/// - `list_versions(table)`：列出所有版本（按 start_lsn 升序）
/// - `check_compatibility(old, new, level)`：静态兼容性检查
pub struct SchemaVersionRegistry {
    /// 按表名组织的版本列表（每个表的版本按 start_lsn 升序）
    versions: RwLock<HashMap<String, Vec<SchemaVersion>>>,
    /// 全局递增的版本 ID 计数器
    next_version_id: AtomicU32,
    /// 默认兼容性级别（bump_version 时使用）
    compatibility: Mutex<CompatibilityLevel>,
}

impl Default for SchemaVersionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaVersionRegistry {
    /// 创建一个空的 Schema 版本注册表，默认兼容性级别为 Backward
    pub fn new() -> Self {
        Self {
            versions: RwLock::new(HashMap::new()),
            next_version_id: AtomicU32::new(1),
            compatibility: Mutex::new(CompatibilityLevel::Backward),
        }
    }

    /// 创建注册表并指定默认兼容性级别
    pub fn with_compatibility(level: CompatibilityLevel) -> Self {
        Self {
            versions: RwLock::new(HashMap::new()),
            next_version_id: AtomicU32::new(1),
            compatibility: Mutex::new(level),
        }
    }

    /// 设置默认兼容性级别
    pub fn set_compatibility(&self, level: CompatibilityLevel) {
        *self.compatibility.lock().unwrap() = level;
    }

    /// 获取当前默认兼容性级别
    pub fn compatibility(&self) -> CompatibilityLevel {
        *self.compatibility.lock().unwrap()
    }

    /// DDL 变更时调用：创建新版本，标记前一版本 end_lsn
    ///
    /// **流程**：
    /// 1. 检查 LSN 不倒退（新 LSN 必须大于上一版本的 start_lsn）
    /// 2. 若表中已有版本，对新旧 Schema 执行兼容性检查（使用 `compatibility` 级别）
    /// 3. 分配新 version_id
    /// 4. 将前一版本的 `end_lsn` 设为新 LSN（标记失效点）
    /// 5. 插入新版本（end_lsn = None）
    ///
    /// **参数**：
    /// - `table_name`：表名
    /// - `schema`：新 Schema 定义
    /// - `lsn`：DDL 变更所在 WAL 记录的 LSN
    ///
    /// **返回**：新分配的 version_id
    ///
    /// **错误**：
    /// - `LsnRegression`：新 LSN 小于等于上一版本的 start_lsn
    /// - `Incompatible`：兼容性检查失败
    pub fn bump_version(
        &self,
        table_name: &str,
        schema: Schema,
        lsn: u64,
    ) -> Result<u32, SchemaRegistryError> {
        let mut versions = self.versions.write().unwrap();
        let entry = versions.entry(table_name.to_string()).or_default();

        if let Some(last) = entry.last() {
            // 检查 LSN 不倒退
            if lsn <= last.start_lsn {
                return Err(SchemaRegistryError::LsnRegression {
                    new_lsn: lsn,
                    last_start_lsn: last.start_lsn,
                });
            }

            // 兼容性检查（使用默认级别）
            let level = self.compatibility();
            if level != CompatibilityLevel::None {
                Self::check_compatibility(&last.schema, &schema, level)?;
            }

            // 标记前一版本失效
            let last_idx = entry.len() - 1;
            entry[last_idx].end_lsn = Some(lsn);
        }

        let version_id = self.next_version_id.fetch_add(1, Ordering::SeqCst);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let version = SchemaVersion {
            version_id,
            table_name: table_name.to_string(),
            start_lsn: lsn,
            end_lsn: None,
            schema,
            created_at: timestamp,
        };
        entry.push(version);

        Ok(version_id)
    }

    /// 按 LSN 查询历史 Schema（返回克隆）
    ///
    /// **查询规则**：返回满足 `start_lsn <= lsn < end_lsn` 的版本
    pub fn get_schema_at(&self, table_name: &str, lsn: u64) -> Option<Schema> {
        let versions = self.versions.read().unwrap();
        versions
            .get(table_name)
            .and_then(|v| v.iter().find(|sv| sv.is_valid_at(lsn)))
            .map(|sv| sv.schema.clone())
    }

    /// 按 LSN 查询版本元信息（返回克隆）
    pub fn get_version_at(&self, table_name: &str, lsn: u64) -> Option<SchemaVersion> {
        let versions = self.versions.read().unwrap();
        versions
            .get(table_name)
            .and_then(|v| v.iter().find(|sv| sv.is_valid_at(lsn)))
            .cloned()
    }

    /// 获取当前（最新）版本
    pub fn latest_version(&self, table_name: &str) -> Option<SchemaVersion> {
        let versions = self.versions.read().unwrap();
        versions.get(table_name).and_then(|v| v.last().cloned())
    }

    /// 获取当前（最新）版本的 Schema
    pub fn latest_schema(&self, table_name: &str) -> Option<Schema> {
        self.latest_version(table_name).map(|v| v.schema)
    }

    /// 列出某表的所有版本（按 start_lsn 升序）
    pub fn list_versions(&self, table_name: &str) -> Vec<SchemaVersion> {
        let versions = self.versions.read().unwrap();
        versions.get(table_name).cloned().unwrap_or_default()
    }

    /// 列出所有已注册的表名
    pub fn list_tables(&self) -> Vec<String> {
        self.versions.read().unwrap().keys().cloned().collect()
    }

    /// 获取已注册表的数量
    pub fn table_count(&self) -> usize {
        self.versions.read().unwrap().len()
    }

    /// 获取指定表的版本数量
    pub fn version_count(&self, table_name: &str) -> usize {
        self.versions
            .read()
            .unwrap()
            .get(table_name)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// 获取所有表所有版本的总数
    pub fn total_version_count(&self) -> usize {
        self.versions
            .read()
            .unwrap()
            .values()
            .map(|v| v.len())
            .sum()
    }

    /// 计算两个 Schema 之间的差异（便捷封装）
    pub fn diff(&self, old: &Schema, new: &Schema) -> SchemaDiff {
        diff_schemas(old, new)
    }

    /// 检查两个 Schema 的兼容性（静态方法）
    ///
    /// **规则**（基于列名和约束）：
    /// - **Backward**：新 schema 新增的必需列（NOT NULL 无 default）不允许
    /// - **Forward**：旧 schema 的必需列被删除不允许
    /// - **Full**：同时满足 Backward 和 Forward
    /// - **None**：不检查
    ///
    /// **注**：本实现使用列名和必需性比较，不深入类型兼容性。
    /// 实际生产可扩展为类型兼容性矩阵检查。
    pub fn check_compatibility(
        old: &Schema,
        new: &Schema,
        level: CompatibilityLevel,
    ) -> Result<(), SchemaRegistryError> {
        if level == CompatibilityLevel::None {
            return Ok(());
        }

        let diff = diff_schemas(old, new);

        // Backward：新 schema 新增的必需列（NOT NULL 且无 default）不允许
        if matches!(
            level,
            CompatibilityLevel::Backward | CompatibilityLevel::Full
        ) {
            let incompatible_added: Vec<_> = diff
                .added_columns
                .iter()
                .filter(|c| c.not_null && c.default.is_none())
                .map(|c| c.name.clone())
                .collect();
            if !incompatible_added.is_empty() {
                return Err(SchemaRegistryError::Incompatible(format!(
                    "Backward incompatible: new required (NOT NULL) columns without default: {}",
                    incompatible_added.join(", ")
                )));
            }

            // 修改列：类型变化也算不兼容（简化规则）
            let incompatible_modified: Vec<_> = diff
                .modified_columns
                .iter()
                .filter(|(_, old_col, new_col)| old_col.col_type != new_col.col_type)
                .map(|(name, _, _)| name.clone())
                .collect();
            if !incompatible_modified.is_empty() {
                return Err(SchemaRegistryError::Incompatible(format!(
                    "Backward incompatible: column type changes: {}",
                    incompatible_modified.join(", ")
                )));
            }
        }

        // Forward：旧 schema 的必需列被删除不允许
        if matches!(
            level,
            CompatibilityLevel::Forward | CompatibilityLevel::Full
        ) {
            let removed_required: Vec<String> = diff
                .removed_columns
                .iter()
                .filter(|name| {
                    old.columns
                        .iter()
                        .find(|c| &c.name == *name)
                        .map(|c| c.not_null && c.default.is_none())
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
            if !removed_required.is_empty() {
                return Err(SchemaRegistryError::Incompatible(format!(
                    "Forward incompatible: removed required (NOT NULL) columns: {}",
                    removed_required.join(", ")
                )));
            }
        }

        Ok(())
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_types::schema::ColumnDef;
    use szrsql_types::value::ColumnType;

    // -----------------------------------------------------------------
    // 测试辅助函数
    // -----------------------------------------------------------------

    /// 创建一个简单的 users 表 Schema（id + name）
    fn make_users_schema_v1() -> Schema {
        let mut schema = Schema::new("users");
        schema.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(ColumnDef::new("name", ColumnType::Text).not_null(true));
        schema.finalize_primary_key();
        schema
    }

    /// 创建 users 表 Schema v2（新增 email 列，有 default）
    fn make_users_schema_v2_with_email() -> Schema {
        let mut schema = Schema::new("users");
        schema.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(ColumnDef::new("name", ColumnType::Text).not_null(true));
        schema.add_column(
            ColumnDef::new("email", ColumnType::Text)
                .not_null(false)
                .default(szrsql_types::value::Value::Text("unknown".to_string())),
        );
        schema.finalize_primary_key();
        schema
    }

    /// 创建 users 表 Schema v3（新增 age 列，NOT NULL 无 default — 破坏 Backward）
    fn make_users_schema_v3_with_required_age() -> Schema {
        let mut schema = Schema::new("users");
        schema.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(ColumnDef::new("name", ColumnType::Text).not_null(true));
        schema.add_column(
            ColumnDef::new("email", ColumnType::Text)
                .not_null(false)
                .default(szrsql_types::value::Value::Text("unknown".to_string())),
        );
        schema.add_column(ColumnDef::new("age", ColumnType::Int64).not_null(true));
        schema.finalize_primary_key();
        schema
    }

    /// 创建 users 表 Schema v4（删除 name 列 — 破坏 Forward）
    fn make_users_schema_v4_without_name() -> Schema {
        let mut schema = Schema::new("users");
        schema.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(
            ColumnDef::new("email", ColumnType::Text)
                .not_null(false)
                .default(szrsql_types::value::Value::Text("unknown".to_string())),
        );
        schema.finalize_primary_key();
        schema
    }

    /// 创建 products 表 Schema
    fn make_products_schema() -> Schema {
        let mut schema = Schema::new("products");
        schema.add_column(
            ColumnDef::new("product_id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        schema.add_column(ColumnDef::new("price", ColumnType::Float64).not_null(true));
        schema.finalize_primary_key();
        schema
    }

    // -----------------------------------------------------------------
    // Part 1: SchemaDiff 基础
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_schema_diff_empty_when_identical() {
        let schema = make_users_schema_v1();
        let diff = diff_schemas(&schema, &schema);
        assert!(diff.is_empty());
        assert_eq!(diff.change_count(), 0);
    }

    #[test]
    fn phase_2_5_6_schema_diff_empty_constructor() {
        let diff = SchemaDiff::empty();
        assert!(diff.is_empty());
        assert_eq!(diff.change_count(), 0);
    }

    #[test]
    fn phase_2_5_6_schema_diff_added_column() {
        let old = make_users_schema_v1();
        let new = make_users_schema_v2_with_email();
        let diff = diff_schemas(&old, &new);
        assert_eq!(diff.added_columns.len(), 1);
        assert_eq!(diff.added_columns[0].name, "email");
        assert!(diff.removed_columns.is_empty());
        assert!(diff.modified_columns.is_empty());
        assert!(!diff.is_empty());
        assert_eq!(diff.change_count(), 1);
    }

    #[test]
    fn phase_2_5_6_schema_diff_removed_column() {
        let old = make_users_schema_v2_with_email();
        let new = make_users_schema_v1();
        let diff = diff_schemas(&old, &new);
        assert_eq!(diff.removed_columns.len(), 1);
        assert_eq!(diff.removed_columns[0], "email");
        assert!(diff.added_columns.is_empty());
        assert!(diff.modified_columns.is_empty());
    }

    #[test]
    fn phase_2_5_6_schema_diff_modified_column_type() {
        let old = make_users_schema_v1();
        let mut new = make_users_schema_v1();
        // 修改 name 列的类型（Text → Int64）
        new.columns[1].col_type = ColumnType::Int64;
        let diff = diff_schemas(&old, &new);
        assert_eq!(diff.modified_columns.len(), 1);
        assert_eq!(diff.modified_columns[0].0, "name");
        assert!(diff.added_columns.is_empty());
        assert!(diff.removed_columns.is_empty());
    }

    #[test]
    fn phase_2_5_6_schema_diff_modified_column_not_null() {
        let old = make_users_schema_v1();
        let mut new = make_users_schema_v1();
        // 修改 name 列的 NOT NULL 约束
        new.columns[1].not_null = false;
        let diff = diff_schemas(&old, &new);
        assert_eq!(diff.modified_columns.len(), 1);
        assert_eq!(diff.modified_columns[0].0, "name");
    }

    #[test]
    fn phase_2_5_6_schema_diff_mixed_changes() {
        let old = make_users_schema_v1();
        let mut new = Schema::new("users");
        new.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        // 修改 name 列类型
        new.add_column(ColumnDef::new("name", ColumnType::Int64).not_null(true));
        // 新增 email 列
        new.add_column(ColumnDef::new("email", ColumnType::Text));
        new.finalize_primary_key();
        let diff = diff_schemas(&old, &new);
        assert_eq!(diff.added_columns.len(), 1);
        assert_eq!(diff.modified_columns.len(), 1);
        assert_eq!(diff.removed_columns.len(), 0);
        assert_eq!(diff.change_count(), 2);
    }

    // -----------------------------------------------------------------
    // Part 2: SchemaVersion 基础
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_schema_version_is_valid_at_current() {
        let schema = make_users_schema_v1();
        let version = SchemaVersion {
            version_id: 1,
            table_name: "users".to_string(),
            start_lsn: 100,
            end_lsn: None,
            schema,
            created_at: 0,
        };
        // 当前版本，所有 lsn >= 100 都有效
        assert!(version.is_valid_at(100));
        assert!(version.is_valid_at(101));
        assert!(version.is_valid_at(u64::MAX));
        // lsn < start_lsn 无效
        assert!(!version.is_valid_at(99));
        assert!(!version.is_valid_at(0));
    }

    #[test]
    fn phase_2_5_6_schema_version_is_valid_at_historical() {
        let schema = make_users_schema_v1();
        let version = SchemaVersion {
            version_id: 1,
            table_name: "users".to_string(),
            start_lsn: 100,
            end_lsn: Some(200),
            schema,
            created_at: 0,
        };
        // [100, 200) 有效
        assert!(version.is_valid_at(100));
        assert!(version.is_valid_at(150));
        assert!(version.is_valid_at(199));
        // lsn >= end_lsn 无效
        assert!(!version.is_valid_at(200));
        assert!(!version.is_valid_at(201));
        // lsn < start_lsn 无效
        assert!(!version.is_valid_at(99));
    }

    #[test]
    fn phase_2_5_6_schema_version_is_current() {
        let schema = make_users_schema_v1();
        let current = SchemaVersion {
            version_id: 1,
            table_name: "users".to_string(),
            start_lsn: 100,
            end_lsn: None,
            schema: schema.clone(),
            created_at: 0,
        };
        let historical = SchemaVersion {
            version_id: 2,
            table_name: "users".to_string(),
            start_lsn: 100,
            end_lsn: Some(200),
            schema,
            created_at: 0,
        };
        assert!(current.is_current());
        assert!(!historical.is_current());
    }

    #[test]
    fn phase_2_5_6_schema_version_lsn_span() {
        let schema = make_users_schema_v1();
        let current = SchemaVersion {
            version_id: 1,
            table_name: "users".to_string(),
            start_lsn: 100,
            end_lsn: None,
            schema: schema.clone(),
            created_at: 0,
        };
        let historical = SchemaVersion {
            version_id: 2,
            table_name: "users".to_string(),
            start_lsn: 100,
            end_lsn: Some(200),
            schema,
            created_at: 0,
        };
        assert_eq!(current.lsn_span(), None);
        assert_eq!(historical.lsn_span(), Some(100));
    }

    // -----------------------------------------------------------------
    // Part 3: SchemaVersionRegistry 基础
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_registry_new_is_empty() {
        let registry = SchemaVersionRegistry::new();
        assert_eq!(registry.table_count(), 0);
        assert_eq!(registry.total_version_count(), 0);
        assert!(registry.list_tables().is_empty());
    }

    #[test]
    fn phase_2_5_6_registry_default_compatibility_is_backward() {
        let registry = SchemaVersionRegistry::new();
        assert_eq!(registry.compatibility(), CompatibilityLevel::Backward);
    }

    #[test]
    fn phase_2_5_6_registry_with_compatibility() {
        let registry = SchemaVersionRegistry::with_compatibility(CompatibilityLevel::Full);
        assert_eq!(registry.compatibility(), CompatibilityLevel::Full);
    }

    #[test]
    fn phase_2_5_6_registry_set_compatibility() {
        let registry = SchemaVersionRegistry::new();
        registry.set_compatibility(CompatibilityLevel::None);
        assert_eq!(registry.compatibility(), CompatibilityLevel::None);
    }

    // -----------------------------------------------------------------
    // Part 4: bump_version — DDL 变更 → Schema 版本递增
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_bump_version_first_version() {
        let registry = SchemaVersionRegistry::new();
        let schema = make_users_schema_v1();
        let version_id = registry.bump_version("users", schema, 100).unwrap();
        assert_eq!(version_id, 1);
        assert_eq!(registry.table_count(), 1);
        assert_eq!(registry.version_count("users"), 1);
        assert_eq!(registry.total_version_count(), 1);
    }

    #[test]
    fn phase_2_5_6_bump_version_increments_version_id() {
        let registry = SchemaVersionRegistry::new();
        let v1 = registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        let v2 = registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();
        let v3 = registry
            .bump_version("products", make_products_schema(), 300)
            .unwrap();
        // version_id 在所有表间全局递增
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
        assert_eq!(v3, 3);
    }

    #[test]
    fn phase_2_5_6_bump_version_marks_previous_end_lsn() {
        let registry = SchemaVersionRegistry::new();
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();

        let versions = registry.list_versions("users");
        assert_eq!(versions.len(), 2);
        // 第一版本的 end_lsn 应为 200
        assert_eq!(versions[0].start_lsn, 100);
        assert_eq!(versions[0].end_lsn, Some(200));
        assert!(!versions[0].is_current());
        // 第二版本（当前）的 end_lsn 应为 None
        assert_eq!(versions[1].start_lsn, 200);
        assert_eq!(versions[1].end_lsn, None);
        assert!(versions[1].is_current());
    }

    #[test]
    fn phase_2_5_6_bump_version_rejects_lsn_regression() {
        let registry = SchemaVersionRegistry::new();
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        // LSN 倒退（新 LSN <= 上一版本 start_lsn）
        let result = registry.bump_version("users", make_users_schema_v1(), 100);
        assert!(matches!(
            result,
            Err(SchemaRegistryError::LsnRegression {
                new_lsn: 100,
                last_start_lsn: 100
            })
        ));
        let result = registry.bump_version("users", make_users_schema_v1(), 50);
        assert!(matches!(
            result,
            Err(SchemaRegistryError::LsnRegression {
                new_lsn: 50,
                last_start_lsn: 100
            })
        ));
    }

    #[test]
    fn phase_2_5_6_bump_version_multiple_tables_independent() {
        let registry = SchemaVersionRegistry::new();
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        registry
            .bump_version("products", make_products_schema(), 150)
            .unwrap();
        registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();

        assert_eq!(registry.table_count(), 2);
        assert_eq!(registry.version_count("users"), 2);
        assert_eq!(registry.version_count("products"), 1);
        assert_eq!(registry.total_version_count(), 3);
        let mut tables = registry.list_tables();
        tables.sort();
        assert_eq!(tables, vec!["products".to_string(), "users".to_string()]);
    }

    // -----------------------------------------------------------------
    // Part 5: 按 LSN 查询历史 Schema
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_get_schema_at_returns_correct_version() {
        let registry = SchemaVersionRegistry::new();
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();

        // LSN 100-199: v1（无 email 列）
        let schema_v1 = registry.get_schema_at("users", 100).unwrap();
        assert_eq!(schema_v1.columns.len(), 2);
        assert!(schema_v1.column_index("email").is_none());

        let schema_v1_at_199 = registry.get_schema_at("users", 199).unwrap();
        assert_eq!(schema_v1_at_199.columns.len(), 2);

        // LSN 200+: v2（有 email 列）
        let schema_v2 = registry.get_schema_at("users", 200).unwrap();
        assert_eq!(schema_v2.columns.len(), 3);
        assert!(schema_v2.column_index("email").is_some());

        let schema_v2_at_300 = registry.get_schema_at("users", 300).unwrap();
        assert_eq!(schema_v2_at_300.columns.len(), 3);
    }

    #[test]
    fn phase_2_5_6_get_schema_at_before_first_version_returns_none() {
        let registry = SchemaVersionRegistry::new();
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        // LSN < 100 无版本
        assert!(registry.get_schema_at("users", 99).is_none());
        assert!(registry.get_schema_at("users", 0).is_none());
    }

    #[test]
    fn phase_2_5_6_get_schema_at_unknown_table_returns_none() {
        let registry = SchemaVersionRegistry::new();
        assert!(registry.get_schema_at("unknown", 100).is_none());
    }

    #[test]
    fn phase_2_5_6_get_version_at_returns_metadata() {
        let registry = SchemaVersionRegistry::new();
        let v1_id = registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        let v2_id = registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();

        let v1_at_150 = registry.get_version_at("users", 150).unwrap();
        assert_eq!(v1_at_150.version_id, v1_id);
        assert_eq!(v1_at_150.start_lsn, 100);
        assert_eq!(v1_at_150.end_lsn, Some(200));
        assert!(!v1_at_150.is_current());

        let v2_at_250 = registry.get_version_at("users", 250).unwrap();
        assert_eq!(v2_at_250.version_id, v2_id);
        assert_eq!(v2_at_250.start_lsn, 200);
        assert_eq!(v2_at_250.end_lsn, None);
        assert!(v2_at_250.is_current());
    }

    #[test]
    fn phase_2_5_6_get_schema_at_boundary_lsn() {
        let registry = SchemaVersionRegistry::new();
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();

        // 边界 LSN = start_lsn：属于新版本（is_valid_at 包含 start_lsn）
        let schema = registry.get_schema_at("users", 200).unwrap();
        assert_eq!(schema.columns.len(), 3); // v2

        // 边界 LSN = end_lsn：不属于旧版本（is_valid_at 不包含 end_lsn）
        let schema = registry.get_schema_at("users", 199).unwrap();
        assert_eq!(schema.columns.len(), 2); // v1
    }

    #[test]
    fn phase_2_5_6_get_schema_at_three_versions() {
        let registry = SchemaVersionRegistry::new();
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();
        // 删除 name 列（兼容性 None 才能通过）
        registry.set_compatibility(CompatibilityLevel::None);
        registry
            .bump_version("users", make_users_schema_v4_without_name(), 300)
            .unwrap();

        // LSN 100-199: v1（2 列）
        assert_eq!(
            registry.get_schema_at("users", 100).unwrap().columns.len(),
            2
        );
        // LSN 200-299: v2（3 列）
        assert_eq!(
            registry.get_schema_at("users", 200).unwrap().columns.len(),
            3
        );
        assert_eq!(
            registry.get_schema_at("users", 299).unwrap().columns.len(),
            3
        );
        // LSN 300+: v3（2 列，删除了 name）
        assert_eq!(
            registry.get_schema_at("users", 300).unwrap().columns.len(),
            2
        );
        assert_eq!(
            registry.get_schema_at("users", 9999).unwrap().columns.len(),
            2
        );
    }

    // -----------------------------------------------------------------
    // Part 6: latest_version / latest_schema
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_latest_version_returns_current() {
        let registry = SchemaVersionRegistry::new();
        let v1_id = registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        let v2_id = registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();

        let latest = registry.latest_version("users").unwrap();
        assert_eq!(latest.version_id, v2_id);
        assert_eq!(latest.start_lsn, 200);
        assert_eq!(latest.end_lsn, None);
        assert!(latest.is_current());
        assert_ne!(latest.version_id, v1_id);
    }

    #[test]
    fn phase_2_5_6_latest_version_unknown_table_returns_none() {
        let registry = SchemaVersionRegistry::new();
        assert!(registry.latest_version("unknown").is_none());
    }

    #[test]
    fn phase_2_5_6_latest_schema_returns_current_schema() {
        let registry = SchemaVersionRegistry::new();
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();

        let latest_schema = registry.latest_schema("users").unwrap();
        assert_eq!(latest_schema.columns.len(), 3);
        assert!(latest_schema.column_index("email").is_some());
    }

    // -----------------------------------------------------------------
    // Part 7: list_versions
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_list_versions_sorted_by_start_lsn() {
        let registry = SchemaVersionRegistry::new();
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();
        registry.set_compatibility(CompatibilityLevel::None);
        registry
            .bump_version("users", make_users_schema_v4_without_name(), 300)
            .unwrap();

        let versions = registry.list_versions("users");
        assert_eq!(versions.len(), 3);
        // 按 start_lsn 升序
        assert_eq!(versions[0].start_lsn, 100);
        assert_eq!(versions[1].start_lsn, 200);
        assert_eq!(versions[2].start_lsn, 300);
    }

    #[test]
    fn phase_2_5_6_list_versions_unknown_table_returns_empty() {
        let registry = SchemaVersionRegistry::new();
        assert!(registry.list_versions("unknown").is_empty());
    }

    // -----------------------------------------------------------------
    // Part 8: 兼容性检查 — Backward
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_compatibility_backward_adding_optional_column_ok() {
        let old = make_users_schema_v1();
        let new = make_users_schema_v2_with_email(); // 新增 email（nullable + default）
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::Backward);
        assert!(result.is_ok());
    }

    #[test]
    fn phase_2_5_6_compatibility_backward_adding_required_column_fails() {
        let old = make_users_schema_v2_with_email();
        let new = make_users_schema_v3_with_required_age(); // 新增 age（NOT NULL 无 default）
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::Backward);
        assert!(matches!(result, Err(SchemaRegistryError::Incompatible(_))));
    }

    #[test]
    fn phase_2_5_6_compatibility_backward_type_change_fails() {
        let old = make_users_schema_v1();
        let mut new = make_users_schema_v1();
        new.columns[1].col_type = ColumnType::Int64; // name: Text → Int64
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::Backward);
        assert!(matches!(result, Err(SchemaRegistryError::Incompatible(_))));
    }

    #[test]
    fn phase_2_5_6_compatibility_backward_removing_column_ok() {
        // Backward 不关心删除列（新 schema 读取旧数据时，被删的列被忽略）
        let old = make_users_schema_v2_with_email();
        let new = make_users_schema_v1(); // 删除 email
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::Backward);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------
    // Part 9: 兼容性检查 — Forward
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_compatibility_forward_removing_optional_column_ok() {
        // Forward：旧 schema 能读新数据 → 删除有 default 的列 OK
        let old = make_users_schema_v2_with_email();
        let new = make_users_schema_v1(); // 删除 email（有 default）
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::Forward);
        assert!(result.is_ok());
    }

    #[test]
    fn phase_2_5_6_compatibility_forward_removing_required_column_fails() {
        // Forward：旧 schema 需要 name 列，新 schema 删除 name → 失败
        let old = make_users_schema_v1(); // name 是 NOT NULL 无 default
        let mut new = Schema::new("users");
        new.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        new.finalize_primary_key();

        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::Forward);
        assert!(matches!(result, Err(SchemaRegistryError::Incompatible(_))));
    }

    #[test]
    fn phase_2_5_6_compatibility_forward_adding_column_ok() {
        // Forward：新 schema 新增列不影响旧 schema 读取新数据
        let old = make_users_schema_v1();
        let new = make_users_schema_v2_with_email();
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::Forward);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------
    // Part 10: 兼容性检查 — Full / None
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_compatibility_full_adding_optional_column_ok() {
        let old = make_users_schema_v1();
        let new = make_users_schema_v2_with_email();
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::Full);
        assert!(result.is_ok());
    }

    #[test]
    fn phase_2_5_6_compatibility_full_adding_required_column_fails() {
        let old = make_users_schema_v2_with_email();
        let new = make_users_schema_v3_with_required_age();
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::Full);
        assert!(matches!(result, Err(SchemaRegistryError::Incompatible(_))));
    }

    #[test]
    fn phase_2_5_6_compatibility_full_removing_required_column_fails() {
        let old = make_users_schema_v1();
        let mut new = Schema::new("users");
        new.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        new.finalize_primary_key();
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::Full);
        assert!(matches!(result, Err(SchemaRegistryError::Incompatible(_))));
    }

    #[test]
    fn phase_2_5_6_compatibility_none_always_passes() {
        // None：任何变更都通过
        let old = make_users_schema_v1();
        let new = make_users_schema_v3_with_required_age();
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::None);
        assert!(result.is_ok());
    }

    #[test]
    fn phase_2_5_6_compatibility_none_allows_column_removal() {
        let old = make_users_schema_v1();
        let mut new = Schema::new("users");
        new.add_column(
            ColumnDef::new("id", ColumnType::Int64)
                .not_null(true)
                .primary_key(true),
        );
        new.finalize_primary_key();
        let result =
            SchemaVersionRegistry::check_compatibility(&old, &new, CompatibilityLevel::None);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------
    // Part 11: bump_version 集成兼容性检查
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_bump_version_backward_rejects_incompatible() {
        let registry = SchemaVersionRegistry::new(); // 默认 Backward
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        // 新增 NOT NULL 无 default 列 → Backward 不兼容
        let result = registry.bump_version("users", make_users_schema_v3_with_required_age(), 200);
        assert!(matches!(result, Err(SchemaRegistryError::Incompatible(_))));
        // 版本未增加
        assert_eq!(registry.version_count("users"), 1);
    }

    #[test]
    fn phase_2_5_6_bump_version_none_allows_incompatible() {
        let registry = SchemaVersionRegistry::with_compatibility(CompatibilityLevel::None);
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        // None 不检查，允许任何变更
        let result = registry.bump_version("users", make_users_schema_v3_with_required_age(), 200);
        assert!(result.is_ok());
        assert_eq!(registry.version_count("users"), 2);
    }

    #[test]
    fn phase_2_5_6_bump_version_backward_allows_compatible() {
        let registry = SchemaVersionRegistry::new(); // 默认 Backward
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        // 新增 nullable + default 列 → Backward 兼容
        let result = registry.bump_version("users", make_users_schema_v2_with_email(), 200);
        assert!(result.is_ok());
        assert_eq!(registry.version_count("users"), 2);
    }

    #[test]
    fn phase_2_5_6_bump_version_full_rejects_incompatible() {
        let registry = SchemaVersionRegistry::with_compatibility(CompatibilityLevel::Full);
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();
        // Full 模式下新增 NOT NULL 无 default 列 → 拒绝
        let result = registry.bump_version("users", make_users_schema_v3_with_required_age(), 200);
        assert!(matches!(result, Err(SchemaRegistryError::Incompatible(_))));
    }

    // -----------------------------------------------------------------
    // Part 12: diff 便捷方法
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_registry_diff_returns_same_as_diff_schemas() {
        let registry = SchemaVersionRegistry::new();
        let old = make_users_schema_v1();
        let new = make_users_schema_v2_with_email();
        let diff1 = diff_schemas(&old, &new);
        let diff2 = registry.diff(&old, &new);
        assert_eq!(diff1, diff2);
    }

    // -----------------------------------------------------------------
    // Part 13: 端到端 — DDL 变更序列
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_end_to_end_ddl_sequence() {
        let registry = SchemaVersionRegistry::new();

        // LSN 100: CREATE TABLE users (id, name)
        registry
            .bump_version("users", make_users_schema_v1(), 100)
            .unwrap();

        // LSN 200: ALTER TABLE users ADD COLUMN email TEXT DEFAULT 'unknown'
        registry
            .bump_version("users", make_users_schema_v2_with_email(), 200)
            .unwrap();

        // LSN 300: CREATE TABLE products
        registry
            .bump_version("products", make_products_schema(), 300)
            .unwrap();

        // 验证：查询 LSN 150 的 users schema → v1
        let users_at_150 = registry.get_schema_at("users", 150).unwrap();
        assert_eq!(users_at_150.columns.len(), 2);
        assert!(users_at_150.column_index("id").is_some());
        assert!(users_at_150.column_index("name").is_some());
        assert!(users_at_150.column_index("email").is_none());

        // 验证：查询 LSN 250 的 users schema → v2
        let users_at_250 = registry.get_schema_at("users", 250).unwrap();
        assert_eq!(users_at_250.columns.len(), 3);
        assert!(users_at_250.column_index("email").is_some());

        // 验证：查询 LSN 350 的 products schema → v1
        let products_at_350 = registry.get_schema_at("products", 350).unwrap();
        assert_eq!(products_at_350.columns.len(), 2);
        assert!(products_at_350.column_index("product_id").is_some());
        assert!(products_at_350.column_index("price").is_some());

        // 验证：当前最新版本
        let users_latest = registry.latest_version("users").unwrap();
        assert_eq!(users_latest.start_lsn, 200);
        assert!(users_latest.is_current());

        let products_latest = registry.latest_version("products").unwrap();
        assert_eq!(products_latest.start_lsn, 300);
        assert!(products_latest.is_current());

        // 验证：注册表统计
        assert_eq!(registry.table_count(), 2);
        assert_eq!(registry.version_count("users"), 2);
        assert_eq!(registry.version_count("products"), 1);
        assert_eq!(registry.total_version_count(), 3);
    }

    #[test]
    fn phase_2_5_6_end_to_end_cdc_scenario() {
        // 模拟 CDC 场景：消费 WAL 记录时按 LSN 还原 Schema
        let registry = SchemaVersionRegistry::new();

        // DDL 变更历史
        registry
            .bump_version("orders", make_users_schema_v1(), 1000)
            .unwrap();
        registry
            .bump_version("orders", make_users_schema_v2_with_email(), 2000)
            .unwrap();

        // 模拟 WAL 记录 LSN 序列
        let wal_lsns = vec![1001, 1500, 1999, 2000, 2500, 3000];

        for lsn in wal_lsns {
            let schema = registry.get_schema_at("orders", lsn);
            assert!(schema.is_some(), "should have schema at lsn {}", lsn);
            let schema = schema.unwrap();
            if lsn < 2000 {
                // v1: 2 列
                assert_eq!(schema.columns.len(), 2, "lsn {} should be v1", lsn);
            } else {
                // v2: 3 列
                assert_eq!(schema.columns.len(), 3, "lsn {} should be v2", lsn);
            }
        }
    }

    // -----------------------------------------------------------------
    // Part 14: 不变量验证
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_invariant_version_id_globally_unique() {
        let registry = SchemaVersionRegistry::new();
        let mut all_ids = std::collections::HashSet::new();

        for table in &["users", "products", "orders"] {
            for lsn in (100..500).step_by(100) {
                let schema = make_users_schema_v1();
                let mut schema = schema;
                schema.table_name = table.to_string();
                let version_id = registry.bump_version(table, schema, lsn).unwrap();
                assert!(
                    all_ids.insert(version_id),
                    "version_id {} not unique",
                    version_id
                );
            }
        }
    }

    #[test]
    fn phase_2_5_6_invariant_versions_sorted_by_start_lsn() {
        let registry = SchemaVersionRegistry::new();
        for lsn in [100, 200, 300, 400, 500] {
            registry
                .bump_version("users", make_users_schema_v1(), lsn)
                .unwrap();
        }
        let versions = registry.list_versions("users");
        for w in versions.windows(2) {
            assert!(
                w[0].start_lsn < w[1].start_lsn,
                "versions not sorted: {} >= {}",
                w[0].start_lsn,
                w[1].start_lsn
            );
        }
    }

    #[test]
    fn phase_2_5_6_invariant_end_lsn_equals_next_start_lsn() {
        let registry = SchemaVersionRegistry::new();
        for lsn in [100, 200, 300, 400, 500] {
            registry
                .bump_version("users", make_users_schema_v1(), lsn)
                .unwrap();
        }
        let versions = registry.list_versions("users");
        for w in versions.windows(2) {
            assert_eq!(
                w[0].end_lsn,
                Some(w[1].start_lsn),
                "end_lsn of version {} should equal start_lsn of version {}",
                w[0].version_id,
                w[1].version_id
            );
        }
        // 最后一个版本的 end_lsn 为 None
        assert_eq!(versions.last().unwrap().end_lsn, None);
    }

    #[test]
    fn phase_2_5_6_invariant_only_one_current_version_per_table() {
        let registry = SchemaVersionRegistry::new();
        for lsn in [100, 200, 300, 400, 500] {
            registry
                .bump_version("users", make_users_schema_v1(), lsn)
                .unwrap();
        }
        let versions = registry.list_versions("users");
        let current_count = versions.iter().filter(|v| v.is_current()).count();
        assert_eq!(current_count, 1, "exactly one current version expected");
    }

    #[test]
    fn phase_2_5_6_invariant_get_schema_at_returns_correct_version_for_all_lsns() {
        let registry = SchemaVersionRegistry::new();
        let lsns = vec![100, 200, 300];
        for &lsn in &lsns {
            registry
                .bump_version("users", make_users_schema_v1(), lsn)
                .unwrap();
        }

        // 对每个 LSN 都能查到版本
        for &lsn in &lsns {
            assert!(registry.get_schema_at("users", lsn).is_some());
        }

        // 在 [100, 300] 区间内的所有 LSN 都能查到
        for lsn in 100..=300 {
            assert!(
                registry.get_schema_at("users", lsn).is_some(),
                "should have schema at lsn {}",
                lsn
            );
        }

        // LSN < 100 查不到
        assert!(registry.get_schema_at("users", 99).is_none());
        assert!(registry.get_schema_at("users", 0).is_none());
    }

    // -----------------------------------------------------------------
    // Part 15: SchemaDiff 序列化（serde）
    // -----------------------------------------------------------------

    #[test]
    fn phase_2_5_6_schema_diff_serializes_to_json() {
        let old = make_users_schema_v1();
        let new = make_users_schema_v2_with_email();
        let diff = diff_schemas(&old, &new);
        let json = serde_json::to_string(&diff).unwrap();
        assert!(json.contains("added_columns"));
        assert!(json.contains("email"));
    }

    #[test]
    fn phase_2_5_6_schema_diff_roundtrip_json() {
        let old = make_users_schema_v1();
        let new = make_users_schema_v2_with_email();
        let diff = diff_schemas(&old, &new);
        let json = serde_json::to_string(&diff).unwrap();
        let restored: SchemaDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(diff, restored);
    }

    #[test]
    fn phase_2_5_6_schema_version_serializes_to_json() {
        let schema = make_users_schema_v1();
        let version = SchemaVersion {
            version_id: 1,
            table_name: "users".to_string(),
            start_lsn: 100,
            end_lsn: Some(200),
            schema,
            created_at: 1234567890,
        };
        let json = serde_json::to_string(&version).unwrap();
        assert!(json.contains("version_id"));
        assert!(json.contains("start_lsn"));
        assert!(json.contains("end_lsn"));
        assert!(json.contains("users"));
    }

    #[test]
    fn phase_2_5_6_schema_version_roundtrip_json() {
        let schema = make_users_schema_v1();
        let version = SchemaVersion {
            version_id: 42,
            table_name: "users".to_string(),
            start_lsn: 100,
            end_lsn: Some(200),
            schema,
            created_at: 1234567890,
        };
        let json = serde_json::to_string(&version).unwrap();
        let restored: SchemaVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(version, restored);
    }

    #[test]
    fn phase_2_5_6_compatibility_level_serializes() {
        let levels = vec![
            CompatibilityLevel::Backward,
            CompatibilityLevel::Forward,
            CompatibilityLevel::Full,
            CompatibilityLevel::None,
        ];
        for level in levels {
            let json = serde_json::to_string(&level).unwrap();
            let restored: CompatibilityLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(level, restored);
        }
    }
}
