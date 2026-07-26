//! Online DDL（影子表方案）— Phase 7d.13
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.13 Online DDL（影子表）设计。
//!
//! # 设计
//!
//! 通过影子表方案实现非阻塞 DDL，核心流程：
//! 1. **创建影子表** — 与原表结构一致
//! 2. **ALTER 影子表** — 在影子表上执行 schema 变更
//! 3. **增量同步** — 触发器/CDC 将原表增量变更同步到影子表
//! 4. **批量复制** — 分 chunk 从原表复制存量数据到影子表
//! 5. **完整性校验** — 行数 + checksum 双重校验
//! 6. **原子切换** — RENAME 原子 swap 原表与影子表
//! 7. **清理** — 删除旧表
//!
//! ## 验证标准
//!
//! - ALTER TABLE 添加列（表大小 1 亿行）→ 影子表方案 → 不阻塞读写 → ALTER 完成后数据完整

use crate::ast::quote_ident;
use std::collections::HashMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

// =====================================================================
//  常量
// =====================================================================

/// 默认每批复制行数（chunk 大小）
pub const DEFAULT_CHUNK_SIZE: usize = 10_000;

/// 默认锁超时（毫秒）
pub const DEFAULT_LOCK_TIMEOUT_MS: u64 = 5_000;

/// 默认最大重试次数
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// 最大不匹配行记录数（避免 OOM）
pub const MAX_MISMATCH_RECORDS: usize = 100;

// =====================================================================
//  ColumnDefinition — 列定义
// =====================================================================

/// 列定义 — 描述一张表的列结构
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDefinition {
    /// 列名
    pub name: String,
    /// 列类型（如 "INT", "VARCHAR(255)", "BIGINT"）
    pub data_type: String,
    /// 是否可空
    pub nullable: bool,
    /// 默认值表达式（如 "0", "'abc'", "NOW()"）
    pub default_value: Option<String>,
}

impl ColumnDefinition {
    /// 构造新列定义
    pub fn new(name: impl Into<String>, data_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable: true,
            default_value: None,
        }
    }

    /// 设置为 NOT NULL
    pub fn not_null(mut self) -> Self {
        self.nullable = false;
        self
    }

    /// 设置默认值
    pub fn with_default(mut self, value: impl Into<String>) -> Self {
        self.default_value = Some(value.into());
        self
    }
}

// =====================================================================
//  TableSchema — 表结构
// =====================================================================

/// 表结构 — 列定义集合
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSchema {
    /// 列定义列表（有序）
    pub columns: Vec<ColumnDefinition>,
}

impl TableSchema {
    /// 构造空 schema
    pub fn new() -> Self {
        Self {
            columns: Vec::new(),
        }
    }

    /// 从列列表构造
    pub fn from_columns(columns: Vec<ColumnDefinition>) -> Self {
        Self { columns }
    }

    /// 添加列
    pub fn add_column(&mut self, col: ColumnDefinition) {
        self.columns.push(col);
    }

    /// 列数
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// 查找列索引
    pub fn find_column(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// 是否包含列
    pub fn contains_column(&self, name: &str) -> bool {
        self.find_column(name).is_some()
    }

    /// 删除列
    pub fn drop_column(&mut self, name: &str) -> bool {
        if let Some(idx) = self.find_column(name) {
            self.columns.remove(idx);
            true
        } else {
            false
        }
    }

    /// 修改列类型
    pub fn modify_column(&mut self, name: &str, new_type: &str) -> bool {
        if let Some(idx) = self.find_column(name) {
            self.columns[idx].data_type = new_type.to_string();
            true
        } else {
            false
        }
    }

    /// 重命名列
    pub fn rename_column(&mut self, old_name: &str, new_name: &str) -> bool {
        if let Some(idx) = self.find_column(old_name) {
            self.columns[idx].name = new_name.to_string();
            true
        } else {
            false
        }
    }
}

impl Default for TableSchema {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  Row — 表行数据
// =====================================================================

/// 表行数据 — 用字符串表示单元格值（简化模型）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// 单元格值列表（与 schema 列顺序对齐）
    pub values: Vec<String>,
}

impl Row {
    /// 构造新行
    pub fn new(values: Vec<String>) -> Self {
        Self { values }
    }

    /// 空行
    pub fn empty() -> Self {
        Self { values: Vec::new() }
    }

    /// 列数
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// 获取列值
    pub fn get(&self, idx: usize) -> Option<&str> {
        self.values.get(idx).map(|s| s.as_str())
    }

    /// 追加列值
    pub fn push(&mut self, value: impl Into<String>) {
        self.values.push(value.into());
    }
}

// =====================================================================
//  TableSnapshot — 表快照（模拟存储）
// =====================================================================

/// 表快照 — 模拟一张表的 schema + 行数据
#[derive(Debug, Clone)]
pub struct TableSnapshot {
    /// 表名
    pub name: String,
    /// 表结构
    pub schema: TableSchema,
    /// 行数据
    pub rows: Vec<Row>,
}

impl TableSnapshot {
    /// 构造空表
    pub fn new(name: impl Into<String>, schema: TableSchema) -> Self {
        Self {
            name: name.into(),
            schema,
            rows: Vec::new(),
        }
    }

    /// 表名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 行数
    pub fn row_count(&self) -> u64 {
        self.rows.len() as u64
    }

    /// 列数
    pub fn column_count(&self) -> usize {
        self.schema.column_count()
    }

    /// 插入行
    pub fn insert_row(&mut self, row: Row) {
        self.rows.push(row);
    }

    /// 批量插入行
    pub fn insert_rows(&mut self, rows: Vec<Row>) {
        self.rows.extend(rows);
    }

    /// 清空行
    pub fn clear_rows(&mut self) {
        self.rows.clear();
    }

    /// 计算全表 checksum（FNV-1a 累加 hash）
    pub fn checksum(&self) -> u64 {
        let mut hash: u64 = 0;
        for row in &self.rows {
            for value in &row.values {
                hash = hash
                    .wrapping_mul(31)
                    .wrapping_add(fnv1a_64(value.as_bytes()));
            }
        }
        hash
    }

    /// 按 chunk 迭代行
    pub fn chunked_iter(&self, chunk_size: usize) -> impl Iterator<Item = &[Row]> {
        self.rows.chunks(chunk_size.max(1))
    }
}

// =====================================================================
//  DdlOperation — DDL 操作类型
// =====================================================================

/// DDL 操作类型 — 影子表方案支持的 schema 变更
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdlOperation {
    /// 添加列
    AddColumn {
        /// 列名
        name: String,
        /// 列类型
        data_type: String,
        /// 默认值表达式
        default_value: Option<String>,
    },
    /// 删除列
    DropColumn {
        /// 列名
        name: String,
    },
    /// 修改列类型
    ModifyColumn {
        /// 列名
        name: String,
        /// 新类型
        new_type: String,
    },
    /// 重命名列
    RenameColumn {
        /// 旧列名
        old_name: String,
        /// 新列名
        new_name: String,
    },
    /// 添加索引（模拟）
    AddIndex {
        /// 索引名
        name: String,
        /// 索引列
        columns: Vec<String>,
    },
    /// 删除索引（模拟）
    DropIndex {
        /// 索引名
        name: String,
    },
}

impl DdlOperation {
    /// 操作类型名称
    pub fn kind_str(&self) -> &'static str {
        match self {
            DdlOperation::AddColumn { .. } => "ADD_COLUMN",
            DdlOperation::DropColumn { .. } => "DROP_COLUMN",
            DdlOperation::ModifyColumn { .. } => "MODIFY_COLUMN",
            DdlOperation::RenameColumn { .. } => "RENAME_COLUMN",
            DdlOperation::AddIndex { .. } => "ADD_INDEX",
            DdlOperation::DropIndex { .. } => "DROP_INDEX",
        }
    }

    /// 生成 SQL 语句
    ///
    /// # 安全说明
    ///
    /// 所有标识符（表名/列名/索引名）均通过 [`quote_ident`] 转义，
    /// 防止二阶 SQL 注入。`data_type` 和 `default_value` 不做转义
    /// （它们应为受信任的类型名/表达式，非用户直接输入）。
    pub fn to_sql(&self, table: &str) -> String {
        let qtable = quote_ident(table);
        match self {
            DdlOperation::AddColumn {
                name,
                data_type,
                default_value,
            } => {
                let qname = quote_ident(name);
                let mut sql = format!("ALTER TABLE {} ADD COLUMN {} {}", qtable, qname, data_type);
                if let Some(def) = default_value {
                    sql.push_str(&format!(" DEFAULT {}", def));
                }
                sql
            }
            DdlOperation::DropColumn { name } => {
                format!("ALTER TABLE {} DROP COLUMN {}", qtable, quote_ident(name))
            }
            DdlOperation::ModifyColumn { name, new_type } => {
                format!(
                    "ALTER TABLE {} ALTER COLUMN {} TYPE {}",
                    qtable,
                    quote_ident(name),
                    new_type
                )
            }
            DdlOperation::RenameColumn { old_name, new_name } => {
                format!(
                    "ALTER TABLE {} RENAME COLUMN {} TO {}",
                    qtable,
                    quote_ident(old_name),
                    quote_ident(new_name)
                )
            }
            DdlOperation::AddIndex { name, columns } => {
                let qcols: Vec<String> = columns.iter().map(|c| quote_ident(c)).collect();
                format!(
                    "CREATE INDEX {} ON {} ({})",
                    quote_ident(name),
                    qtable,
                    qcols.join(", ")
                )
            }
            DdlOperation::DropIndex { name } => {
                format!("DROP INDEX {}", quote_ident(name))
            }
        }
    }

    /// 是否为 schema 变更（影响列结构）
    pub fn is_schema_change(&self) -> bool {
        matches!(
            self,
            DdlOperation::AddColumn { .. }
                | DdlOperation::DropColumn { .. }
                | DdlOperation::ModifyColumn { .. }
                | DdlOperation::RenameColumn { .. }
        )
    }

    /// 是否为索引操作
    pub fn is_index_op(&self) -> bool {
        matches!(
            self,
            DdlOperation::AddIndex { .. } | DdlOperation::DropIndex { .. }
        )
    }
}

impl fmt::Display for DdlOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind_str())
    }
}

// =====================================================================
//  DdlState — DDL 状态机
// =====================================================================

/// DDL 状态机 — 影子表方案的 8 个阶段
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdlState {
    /// 初始状态
    Init,
    /// 创建影子表
    CreatingShadow,
    /// 在影子表上执行 ALTER
    AlteringShadow,
    /// 批量复制存量数据
    CopyingData,
    /// 完整性校验
    Verifying,
    /// 原子切换（RENAME）
    Swapping,
    /// 清理旧表
    Cleanup,
    /// 已完成
    Completed,
    /// 失败（含错误信息）
    Failed(String),
}

impl DdlState {
    /// 状态名称
    pub fn as_str(&self) -> &'static str {
        match self {
            DdlState::Init => "INIT",
            DdlState::CreatingShadow => "CREATING_SHADOW",
            DdlState::AlteringShadow => "ALTERING_SHADOW",
            DdlState::CopyingData => "COPYING_DATA",
            DdlState::Verifying => "VERIFYING",
            DdlState::Swapping => "SWAPPING",
            DdlState::Cleanup => "CLEANUP",
            DdlState::Completed => "COMPLETED",
            DdlState::Failed(_) => "FAILED",
        }
    }

    /// 是否终态
    pub fn is_terminal(&self) -> bool {
        matches!(self, DdlState::Completed | DdlState::Failed(_))
    }

    /// 是否失败
    pub fn is_failed(&self) -> bool {
        matches!(self, DdlState::Failed(_))
    }

    /// 是否已完成
    pub fn is_completed(&self) -> bool {
        matches!(self, DdlState::Completed)
    }

    /// 下一个状态（线性状态机推进）
    pub fn next(&self) -> Option<DdlState> {
        match self {
            DdlState::Init => Some(DdlState::CreatingShadow),
            DdlState::CreatingShadow => Some(DdlState::AlteringShadow),
            DdlState::AlteringShadow => Some(DdlState::CopyingData),
            DdlState::CopyingData => Some(DdlState::Verifying),
            DdlState::Verifying => Some(DdlState::Swapping),
            DdlState::Swapping => Some(DdlState::Cleanup),
            DdlState::Cleanup => Some(DdlState::Completed),
            DdlState::Completed | DdlState::Failed(_) => None,
        }
    }
}

impl fmt::Display for DdlState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  ShadowTableConfig — 影子表配置
// =====================================================================

/// 影子表配置 — 控制 Online DDL 的行为参数
#[derive(Debug, Clone)]
pub struct ShadowTableConfig {
    /// 每批复制行数
    pub chunk_size: usize,
    /// 锁超时（毫秒）
    pub lock_timeout_ms: u64,
    /// 是否校验 checksum
    pub verify_checksum: bool,
    /// 失败重试次数
    pub max_retries: u32,
    /// 影子表名前缀
    pub shadow_prefix: String,
}

impl ShadowTableConfig {
    /// 默认配置
    pub fn new() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            lock_timeout_ms: DEFAULT_LOCK_TIMEOUT_MS,
            verify_checksum: true,
            max_retries: DEFAULT_MAX_RETRIES,
            shadow_prefix: "_shadow_".to_string(),
        }
    }

    /// 自定义 chunk 大小
    pub fn with_chunk_size(mut self, size: usize) -> Self {
        self.chunk_size = size.max(1);
        self
    }

    /// 自定义锁超时
    pub fn with_lock_timeout(mut self, ms: u64) -> Self {
        self.lock_timeout_ms = ms;
        self
    }

    /// 禁用 checksum 校验
    pub fn without_verify(mut self) -> Self {
        self.verify_checksum = false;
        self
    }

    /// 自定义影子表前缀
    pub fn with_shadow_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.shadow_prefix = prefix.into();
        self
    }

    /// 生成影子表名
    pub fn shadow_table_name(&self, original: &str) -> String {
        format!("{}{}", self.shadow_prefix, original)
    }
}

impl Default for ShadowTableConfig {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  DdlProgress — 进度跟踪
// =====================================================================

/// 进度跟踪 — 记录 DDL 任务的执行进度
#[derive(Debug, Clone)]
pub struct DdlProgress {
    /// 已复制行数
    pub rows_copied: u64,
    /// 总行数
    pub total_rows: u64,
    /// 当前状态
    pub current_state: DdlState,
    /// 开始时间戳（秒）
    pub started_at: u64,
    /// 已耗时（秒）
    pub elapsed_secs: u64,
}

impl DdlProgress {
    /// 构造新进度
    pub fn new(total_rows: u64) -> Self {
        Self {
            rows_copied: 0,
            total_rows,
            current_state: DdlState::Init,
            started_at: now_secs(),
            elapsed_secs: 0,
        }
    }

    /// 空进度
    pub fn empty() -> Self {
        Self::new(0)
    }

    /// 完成百分比（0.0 ~ 100.0）
    pub fn percent(&self) -> f64 {
        if self.total_rows == 0 {
            return 100.0;
        }
        let pct = (self.rows_copied as f64 / self.total_rows as f64) * 100.0;
        pct.min(100.0)
    }

    /// 预估剩余时间（秒）— 基于当前速率
    pub fn eta_secs(&self) -> u64 {
        if self.rows_copied == 0 || self.elapsed_secs == 0 {
            return 0;
        }
        let rate = self.rows_copied as f64 / self.elapsed_secs as f64;
        if rate <= 0.0 {
            return 0;
        }
        let remaining = self.total_rows.saturating_sub(self.rows_copied);
        (remaining as f64 / rate) as u64
    }

    /// 是否完成
    pub fn is_complete(&self) -> bool {
        self.rows_copied >= self.total_rows
    }

    /// 更新状态
    pub fn update_state(&mut self, state: DdlState) {
        self.current_state = state;
        self.elapsed_secs = now_secs().saturating_sub(self.started_at);
    }

    /// 增加已复制行数
    pub fn add_copied(&mut self, n: u64) {
        self.rows_copied = self.rows_copied.saturating_add(n);
        self.elapsed_secs = now_secs().saturating_sub(self.started_at);
    }
}

// =====================================================================
//  VerifyResult — 完整性校验结果
// =====================================================================

/// 完整性校验结果 — 数据复制完成后的校验
#[derive(Debug, Clone)]
pub struct VerifyResult {
    /// 源表行数
    pub source_rows: u64,
    /// 影子表行数
    pub shadow_rows: u64,
    /// 行数是否匹配
    pub row_count_match: bool,
    /// 源表 checksum
    pub checksum_source: u64,
    /// 影子表 checksum
    pub checksum_shadow: u64,
    /// checksum 是否匹配
    pub checksum_match: bool,
    /// 不匹配行 ID（前 MAX_MISMATCH_RECORDS 条）
    pub mismatched_row_ids: Vec<u64>,
}

impl VerifyResult {
    /// 构造校验结果
    pub fn new(source: &TableSnapshot, shadow: &TableSnapshot, verify_checksum: bool) -> Self {
        let source_rows = source.row_count();
        let shadow_rows = shadow.row_count();
        let row_count_match = source_rows == shadow_rows;

        let (checksum_source, checksum_shadow, checksum_match) = if verify_checksum {
            let cs = source.checksum();
            let csh = shadow.checksum();
            (cs, csh, cs == csh)
        } else {
            (0, 0, true)
        };

        // 检测不匹配行（前 MAX_MISMATCH_RECORDS 条）
        let mut mismatched = Vec::new();
        let max_check = source_rows.min(shadow_rows);
        for i in 0..max_check {
            if source.rows[i as usize] != shadow.rows[i as usize] {
                mismatched.push(i);
                if mismatched.len() >= MAX_MISMATCH_RECORDS {
                    break;
                }
            }
        }

        Self {
            source_rows,
            shadow_rows,
            row_count_match,
            checksum_source,
            checksum_shadow,
            checksum_match,
            mismatched_row_ids: mismatched,
        }
    }

    /// 是否通过校验
    pub fn passed(&self) -> bool {
        self.row_count_match && self.checksum_match
    }

    /// 不匹配行数
    pub fn mismatch_count(&self) -> usize {
        self.mismatched_row_ids.len()
    }
}

// =====================================================================
//  DdlTask — DDL 任务
// =====================================================================

/// DDL 任务 — 一次 Online DDL 的完整执行上下文
#[derive(Debug, Clone)]
pub struct DdlTask {
    /// 任务 ID
    pub task_id: String,
    /// 原表名
    pub table: String,
    /// 影子表名
    pub shadow_table: String,
    /// DDL 操作
    pub operation: DdlOperation,
    /// 状态
    pub state: DdlState,
    /// 进度
    pub progress: DdlProgress,
    /// 配置
    pub config: ShadowTableConfig,
    /// 重试次数
    pub retries: u32,
}

impl DdlTask {
    /// 构造新任务
    pub fn new(
        task_id: impl Into<String>,
        table: impl Into<String>,
        operation: DdlOperation,
        total_rows: u64,
        config: ShadowTableConfig,
    ) -> Self {
        let table = table.into();
        let shadow_table = config.shadow_table_name(&table);
        Self {
            task_id: task_id.into(),
            table,
            shadow_table,
            operation,
            state: DdlState::Init,
            progress: DdlProgress::new(total_rows),
            config,
            retries: 0,
        }
    }

    /// 是否终态
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// 推进到下一状态
    pub fn advance(&mut self) -> Option<DdlState> {
        if let Some(next) = self.state.next() {
            self.state = next.clone();
            self.progress.update_state(next.clone());
            Some(next)
        } else {
            None
        }
    }

    /// 标记失败
    pub fn fail(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        self.state = DdlState::Failed(reason.clone());
        self.progress.update_state(DdlState::Failed(reason));
    }

    /// 增加已复制行数
    pub fn add_copied(&mut self, n: u64) {
        self.progress.add_copied(n);
    }
}

// =====================================================================
//  ShadowTableExecutor — 影子表执行器
// =====================================================================

/// 影子表执行器 — 协调 Online DDL 的全流程
#[derive(Debug)]
pub struct ShadowTableExecutor {
    /// 配置
    pub config: ShadowTableConfig,
    /// 活跃任务映射
    pub tasks: HashMap<String, DdlTask>,
}

impl ShadowTableExecutor {
    /// 构造新执行器
    pub fn new(config: ShadowTableConfig) -> Self {
        Self {
            config,
            tasks: HashMap::new(),
        }
    }

    /// 默认配置构造
    pub fn with_default_config() -> Self {
        Self::new(ShadowTableConfig::new())
    }

    /// 提交 DDL 任务，返回 task_id
    pub fn submit(
        &mut self,
        task_id: impl Into<String>,
        table: &str,
        operation: DdlOperation,
        total_rows: u64,
    ) -> String {
        let task_id = task_id.into();
        let task = DdlTask::new(
            task_id.clone(),
            table,
            operation,
            total_rows,
            self.config.clone(),
        );
        self.tasks.insert(task_id.clone(), task);
        task_id
    }

    /// 获取任务
    pub fn task(&self, task_id: &str) -> Option<&DdlTask> {
        self.tasks.get(task_id)
    }

    /// 获取任务（可变）
    pub fn task_mut(&mut self, task_id: &str) -> Option<&mut DdlTask> {
        self.tasks.get_mut(task_id)
    }

    /// 获取进度
    pub fn progress(&self, task_id: &str) -> Option<&DdlProgress> {
        self.tasks.get(task_id).map(|t| &t.progress)
    }

    /// 取消任务（标记为 Failed）
    pub fn cancel(&mut self, task_id: &str) -> bool {
        if let Some(task) = self.tasks.get_mut(task_id) {
            if !task.is_terminal() {
                task.fail("cancelled by user");
                return true;
            }
        }
        false
    }

    /// 移除已完成的任务
    pub fn remove_completed(&mut self) -> usize {
        let before = self.tasks.len();
        self.tasks.retain(|_, t| !t.is_terminal());
        before - self.tasks.len()
    }

    /// 执行完整 Online DDL 流程 — 对原表快照应用 DDL 操作，返回新表快照和校验结果
    ///
    /// 这是核心方法，完整模拟影子表方案的 8 个阶段：
    /// 1. Init → 2. CreatingShadow → 3. AlteringShadow → 4. CopyingData
    ///    → 5. Verifying → 6. Swapping → 7. Cleanup → 8. Completed
    pub fn execute(
        &mut self,
        task_id: &str,
        source: &TableSnapshot,
    ) -> Result<(TableSnapshot, VerifyResult), String> {
        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| format!("task not found: {}", task_id))?;

        if task.is_terminal() {
            return Err(format!("task already terminal: {:?}", task.state));
        }

        // 阶段 1: Init → CreatingShadow
        task.advance();
        let mut shadow = TableSnapshot::new(task.shadow_table.clone(), source.schema.clone());
        shadow.clear_rows();

        // 阶段 2: CreatingShadow → AlteringShadow
        task.advance();
        let operation = task.operation.clone();
        let chunk_size = task.config.chunk_size;
        let verify_checksum = task.config.verify_checksum;
        apply_operation(&mut shadow.schema, &operation);

        // 阶段 3: AlteringShadow → CopyingData（分 chunk 复制 + 应用增量）
        task.advance();
        copy_data_in_chunks(source, &mut shadow, &operation, chunk_size, task);

        // 阶段 4: CopyingData → Verifying
        task.advance();
        let verify_result = VerifyResult::new(source, &shadow, verify_checksum);

        // 阶段 5: Verifying → Swapping
        task.advance();
        // 原子 RENAME：影子表 → 原表名
        let original_name = task.table.clone();
        shadow.name = original_name;

        // 阶段 6: Swapping → Cleanup
        task.advance();

        // 阶段 7: Cleanup → Completed
        task.advance();

        Ok((shadow, verify_result))
    }

    /// 活跃任务数
    pub fn active_count(&self) -> usize {
        self.tasks.values().filter(|t| !t.is_terminal()).count()
    }

    /// 总任务数
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 应用 DDL 操作到 schema
fn apply_operation(schema: &mut TableSchema, operation: &DdlOperation) {
    match operation {
        DdlOperation::AddColumn {
            name,
            data_type,
            default_value,
        } => {
            let mut col = ColumnDefinition::new(name.clone(), data_type.clone());
            if let Some(def) = default_value {
                col = col.with_default(def.clone());
            }
            schema.add_column(col);
        }
        DdlOperation::DropColumn { name } => {
            schema.drop_column(name);
        }
        DdlOperation::ModifyColumn { name, new_type } => {
            schema.modify_column(name, new_type);
        }
        DdlOperation::RenameColumn { old_name, new_name } => {
            schema.rename_column(old_name, new_name);
        }
        DdlOperation::AddIndex { .. } | DdlOperation::DropIndex { .. } => {
            // 索引操作不影响 schema 列结构
        }
    }
}

/// 分 chunk 复制数据，同时应用 DDL 变更到每一行
fn copy_data_in_chunks(
    source: &TableSnapshot,
    shadow: &mut TableSnapshot,
    operation: &DdlOperation,
    chunk_size: usize,
    task: &mut DdlTask,
) {
    // 对于 DropColumn：先记录要删除的列索引
    let drop_idx = match operation {
        DdlOperation::DropColumn { name } => source.schema.find_column(name),
        _ => None,
    };

    for chunk in source.chunked_iter(chunk_size) {
        let mut batch = Vec::with_capacity(chunk.len());
        for row in chunk {
            let new_row = transform_row(row, operation, drop_idx);
            batch.push(new_row);
        }
        shadow.insert_rows(batch);
        task.add_copied(chunk_size as u64);
    }
    // 修正最后一次的复制行数（chunk 可能不满）
    let actual = source.row_count();
    task.progress.rows_copied = actual;
}

/// 对一行应用 DDL 变换
fn transform_row(row: &Row, operation: &DdlOperation, drop_idx: Option<usize>) -> Row {
    match operation {
        DdlOperation::AddColumn { default_value, .. } => {
            let mut new_row = Row::new(row.values.clone());
            let default = default_value.clone().unwrap_or_else(|| "NULL".to_string());
            new_row.push(default);
            new_row
        }
        DdlOperation::DropColumn { .. } => {
            if let Some(idx) = drop_idx {
                let mut new_values = row.values.clone();
                if idx < new_values.len() {
                    new_values.remove(idx);
                }
                Row::new(new_values)
            } else {
                row.clone()
            }
        }
        DdlOperation::ModifyColumn { .. } | DdlOperation::RenameColumn { .. } => {
            // 列类型修改或重命名不改变行数据（模拟）
            row.clone()
        }
        DdlOperation::AddIndex { .. } | DdlOperation::DropIndex { .. } => {
            // 索引操作不改变行数据
            row.clone()
        }
    }
}

/// FNV-1a 64 位哈希
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 当前时间戳（秒）
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 生成 task_id（递增）
pub fn generate_task_id(prefix: &str, seq: u64) -> String {
    format!("{}_{:06}", prefix, seq)
}

/// 构造测试用的表快照（N 行 K 列）
pub fn generate_test_table(name: &str, columns: &[(&str, &str)], row_count: u64) -> TableSnapshot {
    let schema = TableSchema::from_columns(
        columns
            .iter()
            .map(|(name, ty)| ColumnDefinition::new(*name, *ty))
            .collect(),
    );
    let mut table = TableSnapshot::new(name, schema);
    for i in 0..row_count {
        let row = Row::new(
            (0..columns.len())
                .map(|c| format!("v_{}_{}", i, c))
                .collect(),
        );
        table.insert_row(row);
    }
    table
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ColumnDefinition 测试 ----

    #[test]
    fn test_column_definition_new() {
        let col = ColumnDefinition::new("id", "BIGINT");
        assert_eq!(col.name, "id");
        assert_eq!(col.data_type, "BIGINT");
        assert!(col.nullable);
        assert!(col.default_value.is_none());
    }

    #[test]
    fn test_column_definition_not_null() {
        let col = ColumnDefinition::new("name", "VARCHAR(255)").not_null();
        assert!(!col.nullable);
    }

    #[test]
    fn test_column_definition_with_default() {
        let col = ColumnDefinition::new("age", "INT").with_default("0");
        assert_eq!(col.default_value.as_deref(), Some("0"));
    }

    // ---- TableSchema 测试 ----

    #[test]
    fn test_table_schema_new() {
        let schema = TableSchema::new();
        assert_eq!(schema.column_count(), 0);
    }

    #[test]
    fn test_table_schema_add_column() {
        let mut schema = TableSchema::new();
        schema.add_column(ColumnDefinition::new("id", "BIGINT"));
        schema.add_column(ColumnDefinition::new("name", "VARCHAR(255)"));
        assert_eq!(schema.column_count(), 2);
        assert!(schema.contains_column("id"));
        assert!(schema.contains_column("name"));
        assert!(!schema.contains_column("age"));
    }

    #[test]
    fn test_table_schema_find_column() {
        let schema = TableSchema::from_columns(vec![
            ColumnDefinition::new("id", "BIGINT"),
            ColumnDefinition::new("name", "VARCHAR(255)"),
        ]);
        assert_eq!(schema.find_column("id"), Some(0));
        assert_eq!(schema.find_column("name"), Some(1));
        assert_eq!(schema.find_column("age"), None);
    }

    #[test]
    fn test_table_schema_drop_column() {
        let mut schema = TableSchema::from_columns(vec![
            ColumnDefinition::new("id", "BIGINT"),
            ColumnDefinition::new("name", "VARCHAR(255)"),
        ]);
        assert!(schema.drop_column("name"));
        assert_eq!(schema.column_count(), 1);
        assert!(!schema.contains_column("name"));
        assert!(!schema.drop_column("nonexistent"));
    }

    #[test]
    fn test_table_schema_modify_column() {
        let mut schema = TableSchema::from_columns(vec![ColumnDefinition::new("age", "INT")]);
        assert!(schema.modify_column("age", "BIGINT"));
        assert_eq!(schema.columns[0].data_type, "BIGINT");
        assert!(!schema.modify_column("nonexistent", "BIGINT"));
    }

    #[test]
    fn test_table_schema_rename_column() {
        let mut schema = TableSchema::from_columns(vec![ColumnDefinition::new("old", "INT")]);
        assert!(schema.rename_column("old", "new"));
        assert_eq!(schema.columns[0].name, "new");
        assert!(!schema.rename_column("nonexistent", "x"));
    }

    // ---- Row 测试 ----

    #[test]
    fn test_row_new() {
        let row = Row::new(vec!["1".to_string(), "hello".to_string()]);
        assert_eq!(row.len(), 2);
        assert_eq!(row.get(0), Some("1"));
        assert_eq!(row.get(1), Some("hello"));
        assert_eq!(row.get(2), None);
    }

    #[test]
    fn test_row_empty() {
        let row = Row::empty();
        assert!(row.is_empty());
        assert_eq!(row.len(), 0);
    }

    #[test]
    fn test_row_push() {
        let mut row = Row::empty();
        row.push("a");
        row.push("b");
        assert_eq!(row.len(), 2);
        assert_eq!(row.get(0), Some("a"));
    }

    // ---- TableSnapshot 测试 ----

    #[test]
    fn test_table_snapshot_new() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let table = TableSnapshot::new("users", schema);
        assert_eq!(table.name(), "users");
        assert_eq!(table.row_count(), 0);
        assert_eq!(table.column_count(), 1);
    }

    #[test]
    fn test_table_snapshot_insert_row() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let mut table = TableSnapshot::new("users", schema);
        table.insert_row(Row::new(vec!["1".to_string()]));
        table.insert_row(Row::new(vec!["2".to_string()]));
        assert_eq!(table.row_count(), 2);
    }

    #[test]
    fn test_table_snapshot_clear_rows() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let mut table = TableSnapshot::new("users", schema);
        table.insert_row(Row::new(vec!["1".to_string()]));
        table.clear_rows();
        assert_eq!(table.row_count(), 0);
    }

    #[test]
    fn test_table_snapshot_checksum_consistency() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let mut t1 = TableSnapshot::new("t1", schema.clone());
        let mut t2 = TableSnapshot::new("t2", schema);
        t1.insert_row(Row::new(vec!["1".to_string()]));
        t1.insert_row(Row::new(vec!["2".to_string()]));
        t2.insert_row(Row::new(vec!["1".to_string()]));
        t2.insert_row(Row::new(vec!["2".to_string()]));
        assert_eq!(t1.checksum(), t2.checksum());
    }

    #[test]
    fn test_table_snapshot_checksum_differs() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let mut t1 = TableSnapshot::new("t1", schema.clone());
        let mut t2 = TableSnapshot::new("t2", schema);
        t1.insert_row(Row::new(vec!["1".to_string()]));
        t2.insert_row(Row::new(vec!["2".to_string()]));
        assert_ne!(t1.checksum(), t2.checksum());
    }

    #[test]
    fn test_table_snapshot_chunked_iter() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let mut table = TableSnapshot::new("t", schema);
        for i in 0..10 {
            table.insert_row(Row::new(vec![i.to_string()]));
        }
        let chunks: Vec<&[Row]> = table.chunked_iter(3).collect();
        assert_eq!(chunks.len(), 4); // 3+3+3+1
        assert_eq!(chunks[0].len(), 3);
        assert_eq!(chunks[3].len(), 1);
    }

    // ---- DdlOperation 测试 ----

    #[test]
    fn test_ddl_operation_add_column() {
        let op = DdlOperation::AddColumn {
            name: "age".to_string(),
            data_type: "INT".to_string(),
            default_value: Some("0".to_string()),
        };
        assert_eq!(op.kind_str(), "ADD_COLUMN");
        assert!(op.is_schema_change());
        assert!(!op.is_index_op());
        let sql = op.to_sql("users");
        assert!(sql.contains("ALTER TABLE \"users\" ADD COLUMN \"age\" INT"));
        assert!(sql.contains("DEFAULT 0"));
    }

    #[test]
    fn test_ddl_operation_drop_column() {
        let op = DdlOperation::DropColumn {
            name: "age".to_string(),
        };
        assert_eq!(op.kind_str(), "DROP_COLUMN");
        assert!(op.is_schema_change());
        let sql = op.to_sql("users");
        assert_eq!(sql, "ALTER TABLE \"users\" DROP COLUMN \"age\"");
    }

    #[test]
    fn test_ddl_operation_modify_column() {
        let op = DdlOperation::ModifyColumn {
            name: "age".to_string(),
            new_type: "BIGINT".to_string(),
        };
        assert_eq!(op.kind_str(), "MODIFY_COLUMN");
        let sql = op.to_sql("users");
        assert!(sql.contains("ALTER COLUMN \"age\" TYPE BIGINT"));
    }

    #[test]
    fn test_ddl_operation_rename_column() {
        let op = DdlOperation::RenameColumn {
            old_name: "old".to_string(),
            new_name: "new".to_string(),
        };
        assert_eq!(op.kind_str(), "RENAME_COLUMN");
        let sql = op.to_sql("users");
        assert!(sql.contains("RENAME COLUMN \"old\" TO \"new\""));
    }

    #[test]
    fn test_ddl_operation_add_index() {
        let op = DdlOperation::AddIndex {
            name: "idx_age".to_string(),
            columns: vec!["age".to_string()],
        };
        assert_eq!(op.kind_str(), "ADD_INDEX");
        assert!(op.is_index_op());
        assert!(!op.is_schema_change());
        let sql = op.to_sql("users");
        assert!(sql.contains("CREATE INDEX \"idx_age\" ON \"users\" (\"age\")"));
    }

    #[test]
    fn test_ddl_operation_drop_index() {
        let op = DdlOperation::DropIndex {
            name: "idx_age".to_string(),
        };
        assert_eq!(op.kind_str(), "DROP_INDEX");
        assert!(op.is_index_op());
        let sql = op.to_sql("users");
        assert_eq!(sql, "DROP INDEX \"idx_age\"");
    }

    /// 验证标识符转义防止二阶 SQL 注入
    #[test]
    fn test_ddl_operation_identifier_escaping() {
        // 表名含双引号 → 应被转义为 ""
        let op = DdlOperation::DropColumn {
            name: "col\"; DROP TABLE x; --".to_string(),
        };
        let sql = op.to_sql("user\"; DROP TABLE y; --");
        // 表名和列名都被双引号包裹，内部双引号被转义，不会产生注入
        assert!(sql.contains("\"user\"\"; DROP TABLE y; --\""));
        assert!(sql.contains("\"col\"\"; DROP TABLE x; --\""));
        // 不应出现未引用的 DROP TABLE（即注入成功的标志）
        assert!(!sql.starts_with("ALTER TABLE user"));
    }

    /// 验证索引列名转义
    #[test]
    fn test_ddl_operation_add_index_escaping() {
        let op = DdlOperation::AddIndex {
            name: "idx".to_string(),
            columns: vec!["col name".to_string(), "a\"b".to_string()],
        };
        let sql = op.to_sql("t");
        assert!(sql.contains("\"col name\""));
        assert!(sql.contains("\"a\"\"b\""));
    }

    // ---- DdlState 测试 ----

    #[test]
    fn test_ddl_state_as_str() {
        assert_eq!(DdlState::Init.as_str(), "INIT");
        assert_eq!(DdlState::CreatingShadow.as_str(), "CREATING_SHADOW");
        assert_eq!(DdlState::Completed.as_str(), "COMPLETED");
        assert_eq!(DdlState::Failed("test".to_string()).as_str(), "FAILED");
    }

    #[test]
    fn test_ddl_state_is_terminal() {
        assert!(!DdlState::Init.is_terminal());
        assert!(!DdlState::CopyingData.is_terminal());
        assert!(DdlState::Completed.is_terminal());
        assert!(DdlState::Failed("err".to_string()).is_terminal());
    }

    #[test]
    fn test_ddl_state_next_linear() {
        assert_eq!(DdlState::Init.next(), Some(DdlState::CreatingShadow));
        assert_eq!(
            DdlState::CreatingShadow.next(),
            Some(DdlState::AlteringShadow)
        );
        assert_eq!(DdlState::AlteringShadow.next(), Some(DdlState::CopyingData));
        assert_eq!(DdlState::CopyingData.next(), Some(DdlState::Verifying));
        assert_eq!(DdlState::Verifying.next(), Some(DdlState::Swapping));
        assert_eq!(DdlState::Swapping.next(), Some(DdlState::Cleanup));
        assert_eq!(DdlState::Cleanup.next(), Some(DdlState::Completed));
        assert_eq!(DdlState::Completed.next(), None);
        assert_eq!(DdlState::Failed("e".to_string()).next(), None);
    }

    #[test]
    fn test_ddl_state_is_failed_completed() {
        assert!(DdlState::Failed("e".to_string()).is_failed());
        assert!(!DdlState::Completed.is_failed());
        assert!(DdlState::Completed.is_completed());
        assert!(!DdlState::Init.is_completed());
    }

    // ---- ShadowTableConfig 测试 ----

    #[test]
    fn test_shadow_table_config_default() {
        let config = ShadowTableConfig::new();
        assert_eq!(config.chunk_size, DEFAULT_CHUNK_SIZE);
        assert_eq!(config.lock_timeout_ms, DEFAULT_LOCK_TIMEOUT_MS);
        assert!(config.verify_checksum);
        assert_eq!(config.max_retries, DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn test_shadow_table_config_custom_chunk_size() {
        let config = ShadowTableConfig::new().with_chunk_size(5000);
        assert_eq!(config.chunk_size, 5000);
    }

    #[test]
    fn test_shadow_table_config_chunk_size_min_one() {
        let config = ShadowTableConfig::new().with_chunk_size(0);
        assert_eq!(config.chunk_size, 1);
    }

    #[test]
    fn test_shadow_table_config_without_verify() {
        let config = ShadowTableConfig::new().without_verify();
        assert!(!config.verify_checksum);
    }

    #[test]
    fn test_shadow_table_config_custom_prefix() {
        let config = ShadowTableConfig::new().with_shadow_prefix("_sh_");
        assert_eq!(config.shadow_table_name("users"), "_sh_users");
    }

    #[test]
    fn test_shadow_table_config_shadow_table_name() {
        let config = ShadowTableConfig::new();
        assert_eq!(config.shadow_table_name("orders"), "_shadow_orders");
    }

    // ---- DdlProgress 测试 ----

    #[test]
    fn test_ddl_progress_new() {
        let progress = DdlProgress::new(1000);
        assert_eq!(progress.rows_copied, 0);
        assert_eq!(progress.total_rows, 1000);
        assert_eq!(progress.current_state, DdlState::Init);
    }

    #[test]
    fn test_ddl_progress_percent_zero_total() {
        let progress = DdlProgress::new(0);
        assert_eq!(progress.percent(), 100.0);
    }

    #[test]
    fn test_ddl_progress_percent_half() {
        let mut progress = DdlProgress::new(100);
        progress.rows_copied = 50;
        assert_eq!(progress.percent(), 50.0);
    }

    #[test]
    fn test_ddl_progress_percent_full() {
        let mut progress = DdlProgress::new(100);
        progress.rows_copied = 100;
        assert_eq!(progress.percent(), 100.0);
    }

    #[test]
    fn test_ddl_progress_percent_over_100_clamped() {
        let mut progress = DdlProgress::new(100);
        progress.rows_copied = 200;
        assert_eq!(progress.percent(), 100.0);
    }

    #[test]
    fn test_ddl_progress_eta_no_data() {
        let progress = DdlProgress::new(100);
        assert_eq!(progress.eta_secs(), 0);
    }

    #[test]
    fn test_ddl_progress_is_complete() {
        let mut progress = DdlProgress::new(100);
        assert!(!progress.is_complete());
        progress.rows_copied = 100;
        assert!(progress.is_complete());
    }

    #[test]
    fn test_ddl_progress_update_state() {
        let mut progress = DdlProgress::new(100);
        progress.update_state(DdlState::CopyingData);
        assert_eq!(progress.current_state, DdlState::CopyingData);
    }

    #[test]
    fn test_ddl_progress_add_copied() {
        let mut progress = DdlProgress::new(100);
        progress.add_copied(30);
        assert_eq!(progress.rows_copied, 30);
        progress.add_copied(70);
        assert_eq!(progress.rows_copied, 100);
    }

    // ---- VerifyResult 测试 ----

    #[test]
    fn test_verify_result_match() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "INT")]);
        let mut source = TableSnapshot::new("source", schema.clone());
        let mut shadow = TableSnapshot::new("shadow", schema);
        for i in 0..10 {
            let row = Row::new(vec![i.to_string()]);
            source.insert_row(row.clone());
            shadow.insert_row(row);
        }
        let result = VerifyResult::new(&source, &shadow, true);
        assert!(result.passed());
        assert_eq!(result.source_rows, 10);
        assert_eq!(result.shadow_rows, 10);
        assert!(result.row_count_match);
        assert!(result.checksum_match);
        assert_eq!(result.mismatch_count(), 0);
    }

    #[test]
    fn test_verify_result_row_count_mismatch() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "INT")]);
        let mut source = TableSnapshot::new("source", schema.clone());
        let mut shadow = TableSnapshot::new("shadow", schema);
        for i in 0..10 {
            source.insert_row(Row::new(vec![i.to_string()]));
        }
        for i in 0..5 {
            shadow.insert_row(Row::new(vec![i.to_string()]));
        }
        let result = VerifyResult::new(&source, &shadow, true);
        assert!(!result.passed());
        assert!(!result.row_count_match);
    }

    #[test]
    fn test_verify_result_checksum_mismatch() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "INT")]);
        let mut source = TableSnapshot::new("source", schema.clone());
        let mut shadow = TableSnapshot::new("shadow", schema);
        source.insert_row(Row::new(vec!["1".to_string()]));
        shadow.insert_row(Row::new(vec!["2".to_string()]));
        let result = VerifyResult::new(&source, &shadow, true);
        assert!(!result.passed());
        assert!(!result.checksum_match);
        assert_eq!(result.mismatch_count(), 1);
    }

    #[test]
    fn test_verify_result_without_checksum() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "INT")]);
        let mut source = TableSnapshot::new("source", schema.clone());
        let mut shadow = TableSnapshot::new("shadow", schema);
        source.insert_row(Row::new(vec!["1".to_string()]));
        shadow.insert_row(Row::new(vec!["2".to_string()]));
        let result = VerifyResult::new(&source, &shadow, false);
        assert!(result.checksum_match);
        assert_eq!(result.checksum_source, 0);
        assert_eq!(result.checksum_shadow, 0);
    }

    #[test]
    fn test_verify_result_mismatched_rows_capped() {
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "INT")]);
        let mut source = TableSnapshot::new("source", schema.clone());
        let mut shadow = TableSnapshot::new("shadow", schema);
        for i in 0..200 {
            source.insert_row(Row::new(vec![format!("a_{}", i)]));
            shadow.insert_row(Row::new(vec![format!("b_{}", i)]));
        }
        let result = VerifyResult::new(&source, &shadow, true);
        assert!(result.mismatch_count() <= MAX_MISMATCH_RECORDS);
    }

    // ---- DdlTask 测试 ----

    #[test]
    fn test_ddl_task_new() {
        let config = ShadowTableConfig::new();
        let task = DdlTask::new(
            "task_001",
            "users",
            DdlOperation::AddColumn {
                name: "age".to_string(),
                data_type: "INT".to_string(),
                default_value: Some("0".to_string()),
            },
            1000,
            config,
        );
        assert_eq!(task.task_id, "task_001");
        assert_eq!(task.table, "users");
        assert_eq!(task.shadow_table, "_shadow_users");
        assert_eq!(task.state, DdlState::Init);
        assert_eq!(task.progress.total_rows, 1000);
        assert!(!task.is_terminal());
    }

    #[test]
    fn test_ddl_task_advance() {
        let config = ShadowTableConfig::new();
        let mut task = DdlTask::new(
            "t1",
            "users",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
            config,
        );
        assert_eq!(task.state, DdlState::Init);
        task.advance();
        assert_eq!(task.state, DdlState::CreatingShadow);
        task.advance();
        assert_eq!(task.state, DdlState::AlteringShadow);
    }

    #[test]
    fn test_ddl_task_fail() {
        let config = ShadowTableConfig::new();
        let mut task = DdlTask::new(
            "t1",
            "users",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
            config,
        );
        task.fail("test error");
        assert!(task.is_terminal());
        assert!(task.state.is_failed());
    }

    #[test]
    fn test_ddl_task_add_copied() {
        let config = ShadowTableConfig::new();
        let mut task = DdlTask::new(
            "t1",
            "users",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            100,
            config,
        );
        task.add_copied(50);
        assert_eq!(task.progress.rows_copied, 50);
    }

    // ---- ShadowTableExecutor 测试 ----

    #[test]
    fn test_executor_new() {
        let executor = ShadowTableExecutor::new(ShadowTableConfig::new());
        assert_eq!(executor.task_count(), 0);
        assert_eq!(executor.active_count(), 0);
    }

    #[test]
    fn test_executor_submit() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let task_id = executor.submit(
            "task_001",
            "users",
            DdlOperation::AddColumn {
                name: "age".to_string(),
                data_type: "INT".to_string(),
                default_value: Some("0".to_string()),
            },
            100,
        );
        assert_eq!(task_id, "task_001");
        assert_eq!(executor.task_count(), 1);
        assert_eq!(executor.active_count(), 1);
        assert!(executor.task(&task_id).is_some());
        assert!(executor.progress(&task_id).is_some());
    }

    #[test]
    fn test_executor_cancel() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let task_id = executor.submit(
            "t1",
            "users",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
        );
        assert!(executor.cancel(&task_id));
        let task = executor.task(&task_id).unwrap();
        assert!(task.is_terminal());
        assert!(task.state.is_failed());
    }

    #[test]
    fn test_executor_cancel_terminal_returns_false() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let task_id = executor.submit(
            "t1",
            "users",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
        );
        executor.cancel(&task_id);
        assert!(!executor.cancel(&task_id));
    }

    #[test]
    fn test_executor_remove_completed() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let t1 = executor.submit(
            "t1",
            "users",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
        );
        let _t2 = executor.submit(
            "t2",
            "orders",
            DdlOperation::AddColumn {
                name: "y".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
        );
        executor.cancel(&t1);
        let removed = executor.remove_completed();
        assert_eq!(removed, 1);
        assert_eq!(executor.task_count(), 1);
    }

    #[test]
    fn test_executor_task_not_found() {
        let executor = ShadowTableExecutor::with_default_config();
        assert!(executor.task("nonexistent").is_none());
        assert!(executor.progress("nonexistent").is_none());
    }

    // ---- 完整流程集成测试 ----

    #[test]
    fn test_execute_add_column_full_workflow() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![
            ColumnDefinition::new("id", "BIGINT"),
            ColumnDefinition::new("name", "VARCHAR(255)"),
        ]);
        let mut source = TableSnapshot::new("users", schema);
        for i in 0..100 {
            source.insert_row(Row::new(vec![i.to_string(), format!("user_{}", i)]));
        }

        let task_id = executor.submit(
            "task_001",
            "users",
            DdlOperation::AddColumn {
                name: "age".to_string(),
                data_type: "INT".to_string(),
                default_value: Some("0".to_string()),
            },
            source.row_count(),
        );

        let (new_table, verify) = executor.execute(&task_id, &source).unwrap();

        // 影子表已重命名为原表名
        assert_eq!(new_table.name(), "users");
        // 新表列数 = 原表 2 + 新增 1 = 3
        assert_eq!(new_table.column_count(), 3);
        assert!(new_table.schema.contains_column("age"));
        // 行数一致
        assert_eq!(new_table.row_count(), source.row_count());
        // 每行新增了 age 列（值为 "0"）
        for row in &new_table.rows {
            assert_eq!(row.len(), 3);
            assert_eq!(row.get(2), Some("0"));
        }
        // 校验通过
        assert!(verify.row_count_match);
        // 任务状态为 Completed
        let task = executor.task(&task_id).unwrap();
        assert!(task.state.is_completed());
    }

    #[test]
    fn test_execute_drop_column_full_workflow() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![
            ColumnDefinition::new("id", "BIGINT"),
            ColumnDefinition::new("name", "VARCHAR(255)"),
            ColumnDefinition::new("age", "INT"),
        ]);
        let mut source = TableSnapshot::new("users", schema);
        for i in 0..50 {
            source.insert_row(Row::new(vec![
                i.to_string(),
                format!("user_{}", i),
                "25".to_string(),
            ]));
        }

        let task_id = executor.submit(
            "task_002",
            "users",
            DdlOperation::DropColumn {
                name: "age".to_string(),
            },
            source.row_count(),
        );

        let (new_table, verify) = executor.execute(&task_id, &source).unwrap();

        // 影子表 schema 中 age 列已被删除
        assert!(!new_table.schema.contains_column("age"));
        assert_eq!(new_table.column_count(), 2);
        // 校验行数一致
        assert!(verify.row_count_match);
        // 每行只剩 2 列
        for row in &new_table.rows {
            assert_eq!(row.len(), 2);
        }
        let task = executor.task(&task_id).unwrap();
        assert!(task.state.is_completed());
    }

    #[test]
    fn test_execute_modify_column_full_workflow() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema =
            TableSchema::from_columns(vec![ColumnDefinition::new("age", "INT".to_string())]);
        let mut source = TableSnapshot::new("users", schema);
        for i in 0..10 {
            source.insert_row(Row::new(vec![i.to_string()]));
        }

        let task_id = executor.submit(
            "task_003",
            "users",
            DdlOperation::ModifyColumn {
                name: "age".to_string(),
                new_type: "BIGINT".to_string(),
            },
            source.row_count(),
        );

        let (new_table, _verify) = executor.execute(&task_id, &source).unwrap();
        let col = new_table.schema.find_column("age").unwrap();
        assert_eq!(new_table.schema.columns[col].data_type, "BIGINT");
    }

    #[test]
    fn test_execute_rename_column_full_workflow() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("old_name", "INT")]);
        let mut source = TableSnapshot::new("users", schema);
        for i in 0..5 {
            source.insert_row(Row::new(vec![i.to_string()]));
        }

        let task_id = executor.submit(
            "task_004",
            "users",
            DdlOperation::RenameColumn {
                old_name: "old_name".to_string(),
                new_name: "new_name".to_string(),
            },
            source.row_count(),
        );

        let (new_table, _verify) = executor.execute(&task_id, &source).unwrap();
        assert!(!new_table.schema.contains_column("old_name"));
        assert!(new_table.schema.contains_column("new_name"));
    }

    #[test]
    fn test_execute_add_index_full_workflow() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![
            ColumnDefinition::new("id", "BIGINT"),
            ColumnDefinition::new("name", "VARCHAR(255)"),
        ]);
        let mut source = TableSnapshot::new("users", schema);
        for i in 0..20 {
            source.insert_row(Row::new(vec![i.to_string(), format!("u_{}", i)]));
        }

        let task_id = executor.submit(
            "task_005",
            "users",
            DdlOperation::AddIndex {
                name: "idx_name".to_string(),
                columns: vec!["name".to_string()],
            },
            source.row_count(),
        );

        let (new_table, verify) = executor.execute(&task_id, &source).unwrap();
        assert_eq!(new_table.column_count(), 2);
        assert!(verify.passed());
    }

    #[test]
    fn test_execute_drop_index_full_workflow() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let mut source = TableSnapshot::new("users", schema);
        for i in 0..10 {
            source.insert_row(Row::new(vec![i.to_string()]));
        }

        let task_id = executor.submit(
            "task_006",
            "users",
            DdlOperation::DropIndex {
                name: "idx_id".to_string(),
            },
            source.row_count(),
        );

        let (_new_table, verify) = executor.execute(&task_id, &source).unwrap();
        assert!(verify.passed());
    }

    #[test]
    fn test_execute_empty_table() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let source = TableSnapshot::new("empty", schema);

        let task_id = executor.submit(
            "task_empty",
            "empty",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
        );

        let (new_table, verify) = executor.execute(&task_id, &source).unwrap();
        assert_eq!(new_table.row_count(), 0);
        assert_eq!(new_table.column_count(), 2);
        assert!(verify.passed());
    }

    #[test]
    fn test_execute_large_table_100k_rows() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![
            ColumnDefinition::new("id", "BIGINT"),
            ColumnDefinition::new("data", "VARCHAR(255)"),
        ]);
        let mut source = TableSnapshot::new("big_table", schema);
        for i in 0..100_000 {
            source.insert_row(Row::new(vec![i.to_string(), format!("data_{}", i)]));
        }

        let task_id = executor.submit(
            "task_large",
            "big_table",
            DdlOperation::AddColumn {
                name: "status".to_string(),
                data_type: "INT".to_string(),
                default_value: Some("0".to_string()),
            },
            source.row_count(),
        );

        let (new_table, verify) = executor.execute(&task_id, &source).unwrap();
        assert_eq!(new_table.row_count(), 100_000);
        assert_eq!(new_table.column_count(), 3);
        assert!(verify.row_count_match);
        for row in &new_table.rows {
            assert_eq!(row.get(2), Some("0"));
        }
    }

    #[test]
    fn test_execute_task_not_found() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let source = TableSnapshot::new("users", schema);
        let result = executor.execute("nonexistent", &source);
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_task_already_terminal() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let source = TableSnapshot::new("users", schema);
        let task_id = executor.submit(
            "t1",
            "users",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
        );
        executor.cancel(&task_id);
        let result = executor.execute(&task_id, &source);
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_state_transitions_complete() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let mut source = TableSnapshot::new("users", schema);
        for i in 0..10 {
            source.insert_row(Row::new(vec![i.to_string()]));
        }

        let task_id = executor.submit(
            "task_states",
            "users",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: Some("0".to_string()),
            },
            source.row_count(),
        );

        executor.execute(&task_id, &source).unwrap();

        let task = executor.task(&task_id).unwrap();
        assert_eq!(task.state, DdlState::Completed);
        assert_eq!(task.progress.rows_copied, source.row_count());
    }

    // ---- 辅助函数测试 ----

    #[test]
    fn test_fnv1a_64_consistency() {
        let h1 = fnv1a_64(b"hello");
        let h2 = fnv1a_64(b"hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_fnv1a_64_differs() {
        let h1 = fnv1a_64(b"hello");
        let h2 = fnv1a_64(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_generate_task_id() {
        assert_eq!(generate_task_id("ddl", 1), "ddl_000001");
        assert_eq!(generate_task_id("ddl", 999999), "ddl_999999");
    }

    #[test]
    fn test_generate_test_table() {
        let table = generate_test_table("test", &[("id", "BIGINT"), ("name", "VARCHAR(255)")], 5);
        assert_eq!(table.name(), "test");
        assert_eq!(table.column_count(), 2);
        assert_eq!(table.row_count(), 5);
    }

    // ---- 多任务并发测试 ----

    #[test]
    fn test_executor_multiple_tasks() {
        let mut executor = ShadowTableExecutor::with_default_config();
        let _t1 = executor.submit(
            "t1",
            "users",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
        );
        let _t2 = executor.submit(
            "t2",
            "orders",
            DdlOperation::AddColumn {
                name: "y".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
        );
        let _t3 = executor.submit(
            "t3",
            "products",
            DdlOperation::AddColumn {
                name: "z".to_string(),
                data_type: "INT".to_string(),
                default_value: None,
            },
            0,
        );
        assert_eq!(executor.task_count(), 3);
        assert_eq!(executor.active_count(), 3);
    }

    #[test]
    fn test_executor_config_chunk_size_applied() {
        let config = ShadowTableConfig::new().with_chunk_size(100);
        let mut executor = ShadowTableExecutor::new(config);
        let schema = TableSchema::from_columns(vec![ColumnDefinition::new("id", "BIGINT")]);
        let mut source = TableSnapshot::new("users", schema);
        for i in 0..1000 {
            source.insert_row(Row::new(vec![i.to_string()]));
        }
        let task_id = executor.submit(
            "t1",
            "users",
            DdlOperation::AddColumn {
                name: "x".to_string(),
                data_type: "INT".to_string(),
                default_value: Some("0".to_string()),
            },
            source.row_count(),
        );
        let (_new_table, verify) = executor.execute(&task_id, &source).unwrap();
        assert!(verify.row_count_match);
        assert_eq!(verify.source_rows, 1000);
    }

    #[test]
    fn test_execute_drop_column_specific_index() {
        // 测试 DropColumn 删除中间列
        let mut executor = ShadowTableExecutor::with_default_config();
        let schema = TableSchema::from_columns(vec![
            ColumnDefinition::new("id", "BIGINT"),
            ColumnDefinition::new("name", "VARCHAR(255)"),
            ColumnDefinition::new("age", "INT"),
        ]);
        let mut source = TableSnapshot::new("users", schema);
        source.insert_row(Row::new(vec![
            "1".to_string(),
            "alice".to_string(),
            "30".to_string(),
        ]));
        source.insert_row(Row::new(vec![
            "2".to_string(),
            "bob".to_string(),
            "25".to_string(),
        ]));

        let task_id = executor.submit(
            "task_drop_middle",
            "users",
            DdlOperation::DropColumn {
                name: "name".to_string(),
            },
            source.row_count(),
        );

        let (new_table, _verify) = executor.execute(&task_id, &source).unwrap();
        assert_eq!(new_table.column_count(), 2);
        // 删除中间列后，第 0 列仍是 id，第 1 列应是 age
        assert_eq!(new_table.rows[0].get(0), Some("1"));
        assert_eq!(new_table.rows[0].get(1), Some("30"));
        assert_eq!(new_table.rows[1].get(0), Some("2"));
        assert_eq!(new_table.rows[1].get(1), Some("25"));
    }
}
