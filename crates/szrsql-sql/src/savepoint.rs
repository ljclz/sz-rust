//! SzRSQL Savepoint 保存点 — Phase 3.23
//!
//! # 设计
//!
//! - **`SavepointStack`** — 命名保存点栈，栈底是事务起始保存点（BEGIN）
//! - **`NamedSavepoint`** — 单个保存点（name + 表快照集合）
//! - 调用方负责对每个表调用 `snapshot()` / `restore()`，本模块只管理命名栈
//! - 与 PG 行为一致：
//!   - SAVEPOINT 嵌套同名允许（新创建的覆盖 ROLLBACK TO 的目标）
//!   - ROLLBACK TO 不存在的 savepoint → `SavepointError::NotFound`
//!   - RELEASE 不存在的 savepoint → `SavepointError::NotFound`
//!   - RELEASE 事务起始（BEGIN）→ `SavepointError::CannotReleaseTransaction`
//!   - COMMIT/ROLLBACK 无活动事务 → 静默（PG 警告但不报错）
//!   - BEGIN 嵌套 → 静默忽略（PG 警告但保持当前事务）
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.23。

use crate::executor::TableSnapshot;
use std::collections::HashMap;
use thiserror::Error;

// =====================================================================
//  错误类型
// =====================================================================

/// Savepoint 操作错误
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SavepointError {
    /// 保存点不存在
    #[error("savepoint \"{0}\" does not exist")]
    NotFound(String),
    /// 不能 RELEASE 事务本身（BEGIN 创建的保存点）
    #[error("cannot release savepoint \"{0}\" because it is the transaction start")]
    CannotReleaseTransaction(String),
    /// 无活动事务时尝试 SAVEPOINT / ROLLBACK TO / RELEASE
    #[error("no active transaction")]
    NoActiveTransaction,
    /// 保存点名重复（PG 允许，但本实现为简化报错；测试验证 PG 兼容性时需放开）
    #[error("duplicate savepoint name \"{0}\"")]
    DuplicateName(String),
}

// =====================================================================
//  NamedSavepoint — 单个保存点
// =====================================================================

/// 一个命名的保存点 — 持有当前事务中所有相关表的快照
///
/// `name` 为空字符串表示事务起始保存点（由 BEGIN 创建）。
#[derive(Debug, Clone)]
pub struct NamedSavepoint {
    /// 保存点名（空字符串表示事务起始）
    pub name: String,
    /// 表名（小写）→ 表快照
    pub snapshots: HashMap<String, TableSnapshot>,
}

impl NamedSavepoint {
    /// 创建命名保存点
    pub fn new(name: impl Into<String>, snapshots: HashMap<String, TableSnapshot>) -> Self {
        Self {
            name: name.into(),
            snapshots,
        }
    }

    /// 是否为事务起始保存点（BEGIN 创建）
    pub fn is_transaction_start(&self) -> bool {
        self.name.is_empty()
    }
}

// =====================================================================
//  SavepointStack — 保存点栈
// =====================================================================

/// 保存点栈 — 管理事务中的多个命名保存点
///
/// # 栈结构
///
/// ```text
/// 栈底 → [事务起始(BEGIN), sp1, sp2, sp3] ← 栈顶
/// ```
///
/// - `BEGIN` → 推入 `name=""` 的事务起始保存点
/// - `SAVEPOINT sp` → 推入 `name="sp"` 的命名保存点
/// - `ROLLBACK TO sp` → 截断到 sp（保留 sp），返回 sp 的快照用于恢复
/// - `RELEASE sp` → 从栈中删除 sp（保留前面的）
/// - `COMMIT` → 清空栈
/// - `ROLLBACK`（无参数）→ 返回事务起始快照，清空栈
///
/// # 用法
///
/// ```ignore
/// use szrsql_sql::executor::{InMemoryTable, MutableTable};
/// use szrsql_sql::savepoint::{SavepointStack, SavepointError};
///
/// let mut stack = SavepointStack::new();
/// let mut table = InMemoryTable::new("t", schema);
///
/// // BEGIN
/// let mut snaps = HashMap::new();
/// snaps.insert("t".to_string(), table.snapshot());
/// stack.begin(snaps);
///
/// // INSERT ...
/// table.insert_row(row);
///
/// // SAVEPOINT sp1
/// let mut snaps = HashMap::new();
/// snaps.insert("t".to_string(), table.snapshot());
/// stack.savepoint("sp1", snaps).unwrap();
///
/// // INSERT ...
/// table.insert_row(row2);
///
/// // ROLLBACK TO sp1
/// let snaps = stack.rollback_to("sp1").unwrap();
/// for (name, s) in snaps {
///     if name == "t" { table.restore(s); }
/// }
///
/// // COMMIT
/// stack.commit();
/// ```
#[derive(Debug, Default, Clone)]
pub struct SavepointStack {
    /// 保存点栈（栈底是事务起始）
    savepoints: Vec<NamedSavepoint>,
}

impl SavepointStack {
    /// 创建空栈
    pub fn new() -> Self {
        Self {
            savepoints: Vec::new(),
        }
    }

    /// 是否有活动事务
    pub fn is_active(&self) -> bool {
        !self.savepoints.is_empty()
    }

    /// 当前栈深度（活动事务时 >=1）
    pub fn depth(&self) -> usize {
        self.savepoints.len()
    }

    /// 列出所有保存点名（栈底到栈顶顺序）
    pub fn list_names(&self) -> Vec<&str> {
        self.savepoints.iter().map(|sp| sp.name.as_str()).collect()
    }

    /// BEGIN — 推入事务起始保存点（name=""）
    ///
    /// 若已有活动事务，按 PG 行为静默忽略（保留当前事务）。
    pub fn begin(&mut self, snapshots: HashMap<String, TableSnapshot>) {
        // PG 行为：嵌套 BEGIN 仅警告，不创建新事务
        if self.is_active() {
            return;
        }
        self.savepoints.push(NamedSavepoint::new("", snapshots));
    }

    /// SAVEPOINT name — 推入命名保存点
    ///
    /// # 错误
    /// - `NoActiveTransaction` — 无活动事务
    /// - `DuplicateName` — 同名保存点已存在（PG 允许同名，本实现为简化报错）
    pub fn savepoint(
        &mut self,
        name: &str,
        snapshots: HashMap<String, TableSnapshot>,
    ) -> Result<(), SavepointError> {
        if !self.is_active() {
            return Err(SavepointError::NoActiveTransaction);
        }
        if self.savepoints.iter().any(|sp| sp.name == name) {
            return Err(SavepointError::DuplicateName(name.into()));
        }
        self.savepoints.push(NamedSavepoint::new(name, snapshots));
        Ok(())
    }

    /// ROLLBACK TO name — 截断到指定保存点（保留该保存点），返回其快照副本
    ///
    /// 调用方应使用返回的快照对每个表调用 `restore()`。
    ///
    /// # 错误
    /// - `NoActiveTransaction` — 无活动事务
    /// - `NotFound` — 指定保存点不存在
    pub fn rollback_to(
        &mut self,
        name: &str,
    ) -> Result<HashMap<String, TableSnapshot>, SavepointError> {
        if !self.is_active() {
            return Err(SavepointError::NoActiveTransaction);
        }
        let idx = self
            .savepoints
            .iter()
            .rposition(|sp| sp.name == name)
            .ok_or_else(|| SavepointError::NotFound(name.into()))?;
        let snapshots = self.savepoints[idx].snapshots.clone();
        // 截断到 idx（保留 idx 处的保存点，删除其后的）
        self.savepoints.truncate(idx + 1);
        Ok(snapshots)
    }

    /// RELEASE name — 删除指定保存点（保留其前面的保存点）
    ///
    /// # 错误
    /// - `NoActiveTransaction` — 无活动事务
    /// - `NotFound` — 指定保存点不存在
    /// - `CannotReleaseTransaction` — 尝试 RELEASE 事务起始（name=""）
    pub fn release(&mut self, name: &str) -> Result<(), SavepointError> {
        if !self.is_active() {
            return Err(SavepointError::NoActiveTransaction);
        }
        let idx = self
            .savepoints
            .iter()
            .rposition(|sp| sp.name == name)
            .ok_or_else(|| SavepointError::NotFound(name.into()))?;
        // 不能 RELEASE 事务起始（idx==0 且 name 为空）
        if idx == 0 && name.is_empty() {
            return Err(SavepointError::CannotReleaseTransaction(name.into()));
        }
        // 若 name 非空但匹配到 idx==0（理论上不会发生，因为 idx==0 的 name 必为空）
        self.savepoints.remove(idx);
        Ok(())
    }

    /// ROLLBACK（无参数）— 返回事务起始保存点的快照副本，并清空栈
    ///
    /// 调用方应使用返回的快照对每个表调用 `restore()`，然后事务结束。
    ///
    /// 无活动事务时返回 None。
    pub fn rollback_all(&mut self) -> Option<HashMap<String, TableSnapshot>> {
        if let Some(sp) = self.savepoints.first() {
            let snapshots = sp.snapshots.clone();
            self.savepoints.clear();
            Some(snapshots)
        } else {
            None
        }
    }

    /// COMMIT — 清空栈
    ///
    /// 无活动事务时静默忽略（PG 警告但不报错）。
    pub fn commit(&mut self) {
        self.savepoints.clear();
    }

    /// 获取指定保存点的快照（只读，用于测试）
    pub fn get_snapshots(&self, name: &str) -> Option<&HashMap<String, TableSnapshot>> {
        self.savepoints
            .iter()
            .rposition(|sp| sp.name == name)
            .map(|idx| &self.savepoints[idx].snapshots)
    }
}

// =====================================================================
//  便捷函数 — 从一组 MutableTable 创建快照
// =====================================================================

/// 从一组 MutableTable 创建快照集合（表名小写 → 快照）
///
/// # 用法
///
/// ```ignore
/// let snapshots = collect_snapshots(&[("t", &table_t), ("s", &table_s)]);
/// stack.savepoint("sp1", snapshots);
/// ```
#[allow(dead_code)]
pub fn collect_snapshots<'a, I>(tables: I) -> HashMap<String, TableSnapshot>
where
    I: IntoIterator<Item = (&'a str, &'a dyn crate::executor::MutableTable)>,
{
    tables
        .into_iter()
        .map(|(name, t)| (name.to_lowercase(), t.snapshot()))
        .collect()
}

/// 将快照集合应用到一组 MutableTable（恢复表状态）
///
/// # 用法
///
/// ```ignore
/// let snapshots = stack.rollback_to("sp1")?;
/// apply_snapshots(&mut [("t", &mut table_t), ("s", &mut table_s)], snapshots);
/// ```
#[allow(dead_code)]
pub fn apply_snapshots<'a, I>(tables: I, mut snapshots: HashMap<String, TableSnapshot>)
where
    I: IntoIterator<Item = (&'a str, &'a mut dyn crate::executor::MutableTable)>,
{
    for (name, t) in tables {
        if let Some(s) = snapshots.remove(&name.to_lowercase()) {
            t.restore(s);
        }
    }
}
