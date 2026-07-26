//! FOR UPDATE / SKIP LOCKED / NOWAIT — Phase 6.26
//!
//! 提供 PG 风格的行级锁定（Row-Level Locking）功能：
//!
//! - **FOR UPDATE**：获取行级排他锁，阻塞其他会话对同一行的修改
//! - **SKIP LOCKED**：跳过已被其他会话锁定的行（不阻塞）
//! - **NOWAIT**：遇到锁定行立即报错（不等待）
//!
//! # 设计
//!
//! - **LockManager**：行锁管理器，使用 `RefCell<HashMap<(table, row_id), session_id>>` 存储
//! - **LockMode**：3 种锁定模式（ForUpdate/SkipLocked/Nowait）
//! - **select_for_update()**：对输入行应用锁定策略，返回选中的行
//!
//! # 与 PG 的关系
//!
//! - PG 6.5+ 支持 `SELECT ... FOR UPDATE`
//! - PG 9.5+ 支持 `SKIP LOCKED` 和 `NOWAIT`
//! - `FOR UPDATE` 获取行级排他锁；其他事务的 UPDATE/DELETE/FOR UPDATE 会等待
//! - `SKIP LOCKED` 跳过已锁定的行（不报错，不等待）
//! - `NOWAIT` 遇到锁定行立即报错（不等待）
//! - PG 的行锁在事务结束时自动释放；本实现需手动调用 `unlock` / `unlock_all`
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：未集成到 SQL 解析路径，仅提供程序化 API
//! - **无等待语义**：ForUpdate 遇到锁定行立即报错（无法模拟阻塞等待）
//! - **无事务集成**：锁不会在事务结束时自动释放，需手动解锁
//! - **无死锁检测**：不检测死锁
//! - **无锁升级**：不支持行锁→表锁升级
//! - **单进程**：使用 RefCell，非线程安全（多线程需换 Mutex）
//! - **主键限定 i64**：行 ID 假设为 Int64 类型

use crate::executor::{ExecutionError, Row};
use std::cell::RefCell;
use std::collections::HashMap;
use szrsql_types::value::Value;

// =====================================================================
//  错误类型
// =====================================================================

/// 行锁错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ForUpdateError {
    /// 行被其他会话锁定（FOR UPDATE 模式，无法等待）
    #[error("row is locked by another session (table={table}, row_id={row_id})")]
    RowLocked { table: String, row_id: i64 },
    /// NOWAIT：行被其他会话锁定
    #[error("NOWAIT: row is locked by another session (table={table}, row_id={row_id})")]
    NowaitLocked { table: String, row_id: i64 },
    /// 无效主键值（非 Int64 或越界）
    #[error("invalid primary key value at column {0}")]
    InvalidPrimaryKey(usize),
    /// 行已被当前会话锁定（重复锁定）
    #[error("row already locked by this session (table={table}, row_id={row_id})")]
    AlreadyLockedBySelf { table: String, row_id: i64 },
}

impl From<ForUpdateError> for ExecutionError {
    fn from(e: ForUpdateError) -> Self {
        ExecutionError::EvalError(format!("FOR UPDATE error: {e}"))
    }
}

// =====================================================================
//  LockMode — 锁定模式
// =====================================================================

/// 锁定模式
///
/// 对应 PG 的 `FOR UPDATE` 子句选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// `FOR UPDATE` — 获取排他锁；遇锁定行报错（无法等待）
    ///
    /// PG 中会阻塞等待，本实现因同步模型无法等待，故报错。
    ForUpdate,
    /// `FOR UPDATE SKIP LOCKED` — 跳过已锁定的行
    SkipLocked,
    /// `FOR UPDATE NOWAIT` — 遇锁定行立即报错
    Nowait,
}

// =====================================================================
//  LockManager — 行锁管理器
// =====================================================================

/// 行锁管理器
///
/// 管理 `(table_name, row_id) → session_id` 的行级排他锁映射。
///
/// # 并发模型
///
/// 使用 `RefCell` 实现内部可变性（单线程）。多线程场景需替换为 `Mutex<HashMap<...>>`。
pub struct LockManager {
    /// 锁映射：(table_name, row_id) → 持有锁的 session_id
    locks: RefCell<HashMap<(String, i64), u64>>,
}

impl Default for LockManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LockManager {
    /// 创建空的锁管理器
    pub fn new() -> Self {
        Self {
            locks: RefCell::new(HashMap::new()),
        }
    }

    /// 尝试获取行锁
    ///
    /// 返回 `true` 表示成功获取（行未被锁定或已被当前会话锁定），
    /// `false` 表示行被其他会话锁定。
    pub fn try_lock(&self, table: &str, row_id: i64, session_id: u64) -> bool {
        let mut locks = self.locks.borrow_mut();
        let key = (table.to_string(), row_id);
        if let Some(&holder) = locks.get(&key) {
            if holder == session_id {
                return true; // 已被自己锁定
            }
            return false; // 被其他会话锁定
        }
        locks.insert(key, session_id);
        true
    }

    /// 释放行锁
    ///
    /// 仅当行被指定会话锁定时才释放。返回 `true` 表示成功释放。
    pub fn unlock(&self, table: &str, row_id: i64, session_id: u64) -> bool {
        let mut locks = self.locks.borrow_mut();
        let key = (table.to_string(), row_id);
        if let Some(&holder) = locks.get(&key) {
            if holder == session_id {
                locks.remove(&key);
                return true;
            }
        }
        false
    }

    /// 检查行是否被其他会话锁定
    pub fn is_locked_by_other(&self, table: &str, row_id: i64, session_id: u64) -> bool {
        let locks = self.locks.borrow();
        matches!(locks.get(&(table.to_string(), row_id)), Some(&h) if h != session_id)
    }

    /// 检查行是否被任何会话锁定
    pub fn is_locked(&self, table: &str, row_id: i64) -> bool {
        self.locks
            .borrow()
            .contains_key(&(table.to_string(), row_id))
    }

    /// 检查行是否被指定会话锁定
    pub fn is_locked_by(&self, table: &str, row_id: i64, session_id: u64) -> bool {
        matches!(
            self.locks.borrow().get(&(table.to_string(), row_id)),
            Some(&h) if h == session_id
        )
    }

    /// 释放指定会话的所有行锁
    ///
    /// 返回释放的锁数量。
    pub fn unlock_all(&self, session_id: u64) -> usize {
        let mut locks = self.locks.borrow_mut();
        let to_remove: Vec<(String, i64)> = locks
            .iter()
            .filter(|(_, &h)| h == session_id)
            .map(|(k, _)| k.clone())
            .collect();
        let count = to_remove.len();
        for key in to_remove {
            locks.remove(&key);
        }
        count
    }

    /// 当前锁总数
    pub fn lock_count(&self) -> usize {
        self.locks.borrow().len()
    }

    /// 获取指定表的锁数量
    pub fn lock_count_for_table(&self, table: &str) -> usize {
        self.locks
            .borrow()
            .keys()
            .filter(|(t, _)| t == table)
            .count()
    }
}

// =====================================================================
//  select_for_update — 应用锁定策略
// =====================================================================

/// 对输入行应用锁定策略
///
/// # 参数
///
/// - `rows` — 输入行（已过滤的候选行）
/// - `pk_col_idx` — 主键列在行中的索引（必须为 Int64 类型）
/// - `lock_manager` — 行锁管理器
/// - `table` — 表名
/// - `session_id` — 当前会话 ID
/// - `mode` — 锁定模式
///
/// # 返回
///
/// 成功锁定（或跳过）的行列表。
///
/// # PG 语义
///
/// - `ForUpdate`：尝试锁定所有行；遇其他会话锁定的行报错（无法等待）
/// - `SkipLocked`：跳过其他会话锁定的行，返回剩余行
/// - `Nowait`：遇任何其他会话锁定的行立即报错
///
/// # 错误
///
/// - `InvalidPrimaryKey` — 主键列非 Int64 或索引越界
/// - `RowLocked` — ForUpdate 模式遇锁定行
/// - `NowaitLocked` — Nowait 模式遇锁定行
pub fn select_for_update(
    rows: &[Row],
    pk_col_idx: usize,
    lock_manager: &LockManager,
    table: &str,
    session_id: u64,
    mode: LockMode,
) -> Result<Vec<Row>, ForUpdateError> {
    let mut result: Vec<Row> = Vec::new();

    for row in rows {
        let row_id = extract_row_id(row, pk_col_idx)?;

        match mode {
            LockMode::ForUpdate => {
                // 尝试锁定；遇其他会话锁定的行报错
                if lock_manager.is_locked_by_other(table, row_id, session_id) {
                    return Err(ForUpdateError::RowLocked {
                        table: table.to_string(),
                        row_id,
                    });
                }
                if !lock_manager.try_lock(table, row_id, session_id) {
                    return Err(ForUpdateError::RowLocked {
                        table: table.to_string(),
                        row_id,
                    });
                }
                result.push(row.clone());
            }
            LockMode::SkipLocked => {
                // 跳过其他会话锁定的行
                if lock_manager.is_locked_by_other(table, row_id, session_id) {
                    continue; // 跳过
                }
                // 尝试锁定（应该总是成功，因为已检查 is_locked_by_other）
                lock_manager.try_lock(table, row_id, session_id);
                result.push(row.clone());
            }
            LockMode::Nowait => {
                // 遇锁定行立即报错
                if lock_manager.is_locked_by_other(table, row_id, session_id) {
                    return Err(ForUpdateError::NowaitLocked {
                        table: table.to_string(),
                        row_id,
                    });
                }
                if !lock_manager.try_lock(table, row_id, session_id) {
                    return Err(ForUpdateError::NowaitLocked {
                        table: table.to_string(),
                        row_id,
                    });
                }
                result.push(row.clone());
            }
        }
    }

    Ok(result)
}

/// 从行中提取主键值（Int64）
fn extract_row_id(row: &Row, pk_col_idx: usize) -> Result<i64, ForUpdateError> {
    match row.get(pk_col_idx) {
        Some(Value::Int64(id)) => Ok(*id),
        Some(_) => Err(ForUpdateError::InvalidPrimaryKey(pk_col_idx)),
        None => Err(ForUpdateError::InvalidPrimaryKey(pk_col_idx)),
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    fn make_row(id: i64, name: &str) -> Row {
        vec![Value::Int64(id), Value::Text(name.to_string())]
    }

    fn make_row_int(id: i64, val: i64) -> Row {
        vec![Value::Int64(id), Value::Int64(val)]
    }

    // -----------------------------------------------------------------
    //  LockManager 基本操作
    // -----------------------------------------------------------------

    #[test]
    fn test_lock_manager_new_empty() {
        let lm = LockManager::new();
        assert_eq!(lm.lock_count(), 0);
    }

    #[test]
    fn test_lock_manager_try_lock_success() {
        let lm = LockManager::new();
        assert!(lm.try_lock("t1", 1, 100));
        assert_eq!(lm.lock_count(), 1);
        assert!(lm.is_locked("t1", 1));
        assert!(lm.is_locked_by("t1", 1, 100));
    }

    #[test]
    fn test_lock_manager_try_lock_already_self() {
        let lm = LockManager::new();
        assert!(lm.try_lock("t1", 1, 100));
        // 同一会话重复锁定 → 返回 true
        assert!(lm.try_lock("t1", 1, 100));
        assert_eq!(lm.lock_count(), 1); // 不会增加
    }

    #[test]
    fn test_lock_manager_try_lock_by_other() {
        let lm = LockManager::new();
        assert!(lm.try_lock("t1", 1, 100));
        // 其他会话尝试锁定 → false
        assert!(!lm.try_lock("t1", 1, 200));
        assert_eq!(lm.lock_count(), 1); // 不变
        assert!(lm.is_locked_by("t1", 1, 100)); // 仍是 session 100
    }

    #[test]
    fn test_lock_manager_unlock_success() {
        let lm = LockManager::new();
        lm.try_lock("t1", 1, 100);
        assert!(lm.unlock("t1", 1, 100));
        assert!(!lm.is_locked("t1", 1));
        assert_eq!(lm.lock_count(), 0);
    }

    #[test]
    fn test_lock_manager_unlock_wrong_session() {
        let lm = LockManager::new();
        lm.try_lock("t1", 1, 100);
        // 其他会话无法解锁
        assert!(!lm.unlock("t1", 1, 200));
        assert!(lm.is_locked("t1", 1));
    }

    #[test]
    fn test_lock_manager_unlock_not_locked() {
        let lm = LockManager::new();
        assert!(!lm.unlock("t1", 1, 100));
    }

    #[test]
    fn test_lock_manager_is_locked_by_other() {
        let lm = LockManager::new();
        lm.try_lock("t1", 1, 100);
        assert!(lm.is_locked_by_other("t1", 1, 200)); // 200 视角：被其他锁定
        assert!(!lm.is_locked_by_other("t1", 1, 100)); // 100 视角：自己锁定
        assert!(!lm.is_locked_by_other("t1", 2, 200)); // 未锁定的行
    }

    #[test]
    fn test_lock_manager_unlock_all() {
        let lm = LockManager::new();
        lm.try_lock("t1", 1, 100);
        lm.try_lock("t1", 2, 100);
        lm.try_lock("t1", 3, 200); // 不同会话
        assert_eq!(lm.lock_count(), 3);

        let released = lm.unlock_all(100);
        assert_eq!(released, 2);
        assert_eq!(lm.lock_count(), 1);
        assert!(!lm.is_locked("t1", 1));
        assert!(!lm.is_locked("t1", 2));
        assert!(lm.is_locked("t1", 3)); // session 200 的锁不受影响
    }

    #[test]
    fn test_lock_manager_unlock_all_no_locks() {
        let lm = LockManager::new();
        assert_eq!(lm.unlock_all(100), 0);
    }

    #[test]
    fn test_lock_manager_lock_count_for_table() {
        let lm = LockManager::new();
        lm.try_lock("t1", 1, 100);
        lm.try_lock("t1", 2, 100);
        lm.try_lock("t2", 1, 100);
        assert_eq!(lm.lock_count_for_table("t1"), 2);
        assert_eq!(lm.lock_count_for_table("t2"), 1);
        assert_eq!(lm.lock_count_for_table("t3"), 0);
    }

    #[test]
    fn test_lock_manager_multiple_tables() {
        let lm = LockManager::new();
        // 同一 row_id 在不同表中是不同的锁
        assert!(lm.try_lock("t1", 1, 100));
        assert!(lm.try_lock("t2", 1, 100));
        assert_eq!(lm.lock_count(), 2);
    }

    // -----------------------------------------------------------------
    //  select_for_update — ForUpdate 模式
    // -----------------------------------------------------------------

    #[test]
    fn test_for_update_basic() {
        let lm = LockManager::new();
        let rows = vec![make_row(1, "Alice"), make_row(2, "Bob")];

        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate).unwrap();

        assert_eq!(result.len(), 2);
        assert!(lm.is_locked_by("t1", 1, 100));
        assert!(lm.is_locked_by("t1", 2, 100));
    }

    #[test]
    fn test_for_update_empty_rows() {
        let lm = LockManager::new();
        let rows: Vec<Row> = vec![];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate).unwrap();
        assert!(result.is_empty());
        assert_eq!(lm.lock_count(), 0);
    }

    #[test]
    fn test_for_update_locked_by_other() {
        let lm = LockManager::new();
        // session 200 已锁定 row 1
        lm.try_lock("t1", 1, 200);

        let rows = vec![make_row(1, "Alice"), make_row(2, "Bob")];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate);

        assert_eq!(
            result,
            Err(ForUpdateError::RowLocked {
                table: "t1".to_string(),
                row_id: 1
            })
        );
        // row 2 未被锁定（因为遇到 row 1 就报错了）
        assert!(!lm.is_locked("t1", 2));
    }

    #[test]
    fn test_for_update_already_locked_by_self() {
        let lm = LockManager::new();
        // session 100 已锁定 row 1
        lm.try_lock("t1", 1, 100);

        let rows = vec![make_row(1, "Alice")];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate).unwrap();

        assert_eq!(result.len(), 1);
        assert!(lm.is_locked_by("t1", 1, 100));
    }

    // -----------------------------------------------------------------
    //  select_for_update — SkipLocked 模式
    // -----------------------------------------------------------------

    #[test]
    fn test_skip_locked_basic() {
        let lm = LockManager::new();
        let rows = vec![
            make_row(1, "Alice"),
            make_row(2, "Bob"),
            make_row(3, "Carol"),
        ];

        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::SkipLocked).unwrap();

        assert_eq!(result.len(), 3); // 无锁，全部返回
        assert!(lm.is_locked_by("t1", 1, 100));
        assert!(lm.is_locked_by("t1", 2, 100));
        assert!(lm.is_locked_by("t1", 3, 100));
    }

    #[test]
    fn test_skip_locked_skips_locked_rows() {
        let lm = LockManager::new();
        // session 200 锁定 row 2
        lm.try_lock("t1", 2, 200);

        let rows = vec![
            make_row(1, "Alice"),
            make_row(2, "Bob"),
            make_row(3, "Carol"),
        ];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::SkipLocked).unwrap();

        assert_eq!(result.len(), 2); // 跳过 row 2
        assert_eq!(result[0], make_row(1, "Alice"));
        assert_eq!(result[1], make_row(3, "Carol"));
        assert!(lm.is_locked_by("t1", 1, 100));
        assert!(!lm.is_locked_by("t1", 2, 100)); // 未锁定（被 200 持有）
        assert!(lm.is_locked_by("t1", 3, 100));
    }

    #[test]
    fn test_skip_locked_all_locked() {
        let lm = LockManager::new();
        lm.try_lock("t1", 1, 200);
        lm.try_lock("t1", 2, 200);

        let rows = vec![make_row(1, "Alice"), make_row(2, "Bob")];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::SkipLocked).unwrap();

        assert!(result.is_empty()); // 全部跳过
    }

    #[test]
    fn test_skip_locked_empty_rows() {
        let lm = LockManager::new();
        let rows: Vec<Row> = vec![];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::SkipLocked).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_skip_locked_self_locked_not_skipped() {
        let lm = LockManager::new();
        // session 100 自己锁定的行不跳过
        lm.try_lock("t1", 1, 100);

        let rows = vec![make_row(1, "Alice")];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::SkipLocked).unwrap();

        assert_eq!(result.len(), 1);
    }

    // -----------------------------------------------------------------
    //  select_for_update — Nowait 模式
    // -----------------------------------------------------------------

    #[test]
    fn test_nowait_basic() {
        let lm = LockManager::new();
        let rows = vec![make_row(1, "Alice"), make_row(2, "Bob")];

        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::Nowait).unwrap();

        assert_eq!(result.len(), 2);
        assert!(lm.is_locked_by("t1", 1, 100));
        assert!(lm.is_locked_by("t1", 2, 100));
    }

    #[test]
    fn test_nowait_locked_by_other() {
        let lm = LockManager::new();
        lm.try_lock("t1", 1, 200);

        let rows = vec![make_row(1, "Alice")];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::Nowait);

        assert_eq!(
            result,
            Err(ForUpdateError::NowaitLocked {
                table: "t1".to_string(),
                row_id: 1
            })
        );
    }

    #[test]
    fn test_nowait_locked_second_row() {
        let lm = LockManager::new();
        lm.try_lock("t1", 2, 200);

        let rows = vec![make_row(1, "Alice"), make_row(2, "Bob")];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::Nowait);

        // row 1 成功锁定，row 2 报错
        assert_eq!(
            result,
            Err(ForUpdateError::NowaitLocked {
                table: "t1".to_string(),
                row_id: 2
            })
        );
        // row 1 已被锁定（在遇到 row 2 之前）
        assert!(lm.is_locked_by("t1", 1, 100));
    }

    #[test]
    fn test_nowait_empty_rows() {
        let lm = LockManager::new();
        let rows: Vec<Row> = vec![];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::Nowait).unwrap();
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------
    //  错误处理
    // -----------------------------------------------------------------

    #[test]
    fn test_error_invalid_primary_key_text() {
        let lm = LockManager::new();
        // 主键列是 Text，不是 Int64
        let rows = vec![vec![Value::Text("not_int".to_string())]];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate);
        assert_eq!(result, Err(ForUpdateError::InvalidPrimaryKey(0)));
    }

    #[test]
    fn test_error_invalid_primary_key_null() {
        let lm = LockManager::new();
        let rows = vec![vec![Value::Null]];
        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate);
        assert_eq!(result, Err(ForUpdateError::InvalidPrimaryKey(0)));
    }

    #[test]
    fn test_error_index_out_of_range() {
        let lm = LockManager::new();
        let rows = vec![make_row(1, "Alice")]; // 宽度 2
        let result = select_for_update(&rows, 5, &lm, "t1", 100, LockMode::ForUpdate);
        assert_eq!(result, Err(ForUpdateError::InvalidPrimaryKey(5)));
    }

    #[test]
    fn test_error_to_execution_error() {
        let e: ExecutionError = ForUpdateError::InvalidPrimaryKey(0).into();
        match e {
            ExecutionError::EvalError(msg) => {
                assert!(msg.contains("FOR UPDATE error"));
                assert!(msg.contains("invalid primary key"));
            }
            _ => panic!("expected EvalError"),
        }
    }

    // -----------------------------------------------------------------
    //  E2E 场景
    // -----------------------------------------------------------------

    #[test]
    fn test_e2e_two_sessions_for_update_conflict() {
        // 模拟：session A 锁定 row 1，session B 尝试 FOR UPDATE 同一行
        let lm = LockManager::new();
        let rows_a = vec![make_row(1, "Alice"), make_row(2, "Bob")];

        // session A 锁定
        let result_a = select_for_update(&rows_a, 0, &lm, "t1", 100, LockMode::ForUpdate).unwrap();
        assert_eq!(result_a.len(), 2);

        // session B 尝试 FOR UPDATE → 报错（row 1 被锁定）
        let result_b = select_for_update(&rows_a, 0, &lm, "t1", 200, LockMode::ForUpdate);
        assert!(result_b.is_err());
    }

    #[test]
    fn test_e2e_two_sessions_skip_locked() {
        // 模拟：session A 锁定部分行，session B 用 SKIP LOCKED 获取剩余行
        let lm = LockManager::new();
        let rows = vec![
            make_row(1, "Alice"),
            make_row(2, "Bob"),
            make_row(3, "Carol"),
            make_row(4, "Dave"),
        ];

        // session A 锁定 row 1, 2
        let rows_a = &rows[0..2];
        select_for_update(rows_a, 0, &lm, "t1", 100, LockMode::ForUpdate).unwrap();

        // session B 用 SKIP LOCKED → 跳过 row 1, 2，获取 row 3, 4
        let result_b = select_for_update(&rows, 0, &lm, "t1", 200, LockMode::SkipLocked).unwrap();
        assert_eq!(result_b.len(), 2);
        assert_eq!(result_b[0], make_row(3, "Carol"));
        assert_eq!(result_b[1], make_row(4, "Dave"));
    }

    #[test]
    fn test_e2e_two_sessions_nowait() {
        // 模拟：session A 锁定行，session B 用 NOWAIT 立即报错
        let lm = LockManager::new();
        let rows = vec![make_row(1, "Alice")];

        select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate).unwrap();

        let result_b = select_for_update(&rows, 0, &lm, "t1", 200, LockMode::Nowait);
        assert_eq!(
            result_b,
            Err(ForUpdateError::NowaitLocked {
                table: "t1".to_string(),
                row_id: 1
            })
        );
    }

    #[test]
    fn test_e2e_unlock_and_relock() {
        // 模拟：session A 锁定后解锁，session B 再锁定
        let lm = LockManager::new();
        let rows = vec![make_row(1, "Alice")];

        // session A 锁定
        select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate).unwrap();
        assert!(lm.is_locked_by("t1", 1, 100));

        // session A 解锁
        lm.unlock_all(100);
        assert!(!lm.is_locked("t1", 1));

        // session B 现在可以锁定
        let result_b = select_for_update(&rows, 0, &lm, "t1", 200, LockMode::ForUpdate).unwrap();
        assert_eq!(result_b.len(), 1);
        assert!(lm.is_locked_by("t1", 1, 200));
    }

    #[test]
    fn test_e2e_multiple_tables_independent() {
        // 不同表的锁互不影响
        let lm = LockManager::new();
        let rows = vec![make_row(1, "Alice")];

        // session A 锁定 t1 row 1
        select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate).unwrap();

        // session B 可以锁定 t2 row 1（不同表）
        let result_b = select_for_update(&rows, 0, &lm, "t2", 200, LockMode::ForUpdate).unwrap();
        assert_eq!(result_b.len(), 1);
        assert!(lm.is_locked_by("t1", 1, 100));
        assert!(lm.is_locked_by("t2", 1, 200));
    }

    #[test]
    fn test_e2e_transaction_commit_unlock() {
        // 模拟事务提交：unlock_all 释放所有锁
        let lm = LockManager::new();
        let rows = vec![
            make_row(1, "Alice"),
            make_row(2, "Bob"),
            make_row(3, "Carol"),
        ];

        select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate).unwrap();
        assert_eq!(lm.lock_count(), 3);

        let released = lm.unlock_all(100);
        assert_eq!(released, 3);
        assert_eq!(lm.lock_count(), 0);

        // 其他会话现在可以锁定
        let result_b = select_for_update(&rows, 0, &lm, "t1", 200, LockMode::ForUpdate).unwrap();
        assert_eq!(result_b.len(), 3);
    }

    #[test]
    fn test_e2e_mixed_int_data() {
        // 使用纯 Int 行测试
        let lm = LockManager::new();
        let rows = vec![
            make_row_int(1, 100),
            make_row_int(2, 200),
            make_row_int(3, 300),
        ];

        let result = select_for_update(&rows, 0, &lm, "t1", 100, LockMode::ForUpdate).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(lm.lock_count(), 3);

        // 第二列不影响锁定（只看主键列）
        assert!(lm.is_locked_by("t1", 1, 100));
        assert!(lm.is_locked_by("t1", 2, 100));
        assert!(lm.is_locked_by("t1", 3, 100));
    }

    #[test]
    fn test_e2e_skip_locked_partial_conflict() {
        // 部分冲突：session A 锁定 row 2，session B SKIP LOCKED 获取 row 1, 3
        let lm = LockManager::new();
        let rows = vec![
            make_row(1, "Alice"),
            make_row(2, "Bob"),
            make_row(3, "Carol"),
        ];

        // session A 只锁定 row 2
        lm.try_lock("t1", 2, 100);

        let result_b = select_for_update(&rows, 0, &lm, "t1", 200, LockMode::SkipLocked).unwrap();
        assert_eq!(result_b.len(), 2);
        assert_eq!(result_b[0], make_row(1, "Alice"));
        assert_eq!(result_b[1], make_row(3, "Carol"));
        // row 2 仍被 session A 持有
        assert!(lm.is_locked_by("t1", 2, 100));
        // row 1, 3 被 session B 持有
        assert!(lm.is_locked_by("t1", 1, 200));
        assert!(lm.is_locked_by("t1", 3, 200));
    }
}
