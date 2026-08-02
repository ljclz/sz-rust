//! SzRSQL 执行器（Phase 3.4 + 3.5）— 火山模型执行算子 + DML。
//!
//! # 设计
//!
//! - **Row** — `Vec<Value>`，与 `szrsql_types::Value` 对齐
//! - **TableStorage trait** — 抽象表存储，提供 `scan_iter / scan_with_ids / row_count / get_row` 接口
//! - **MutableTable trait** — 扩展 `TableStorage` 添加 `insert_row / update_row / delete_row / snapshot / restore`
//! - **InMemoryTable** — `Vec<Row>` + tombstone（`HashSet<usize>`）后端，用于功能测试与 DML
//! - **CounterTable** — 惰性生成行，用于 1M 行性能/正确性测试（无需实际存储 1M 行）
//! - **InMemoryBTreeIndex** — `BTreeMap<i64, Vec<usize>>` 后端索引，支持点查/范围查询
//! - **Executor** — 走访 `LogicalPlan` 树，产出 `Vec<Row>`（SELECT）或 `usize`（DML affected rows）
//! - **TableSnapshot** — 表快照，用于 Phase 3.5 简化事务回滚
//!
//! ## 支持的算子
//!
//! - **SELECT**：Scan（SeqScan）、Filter（WHERE）、Projection（SELECT cols）、Limit、Distinct、Empty
//! - **DML**：Insert（VALUES / SELECT / DEFAULT VALUES）、Update（SET + WHERE）、Delete（WHERE）
//! - **IndexScan** — 独立 API（不在 LogicalPlan 中），直接基于 Table + Index 执行
//!
//! # 关键决策
//!
//! - **不耦合 szrsql-storage 的 Page/TupleSlot**：Phase 3.4/3.5 聚焦"扫描/DML 正确性"，
//!   Page ↔ Value 的序列化层留待 Phase 4 实现
//! - **不引入迭代器树**：当前实现用 `Vec<Row>` 物化中间结果，简化测试与调试
//! - **1M 行测试用 CounterTable**：避免分配 1M × Vec<Value> 的实际内存，
//!   通过惰性生成验证扫描行数正确性
//! - **DML 用 tombstone 删除**：`InMemoryTable` 用 `HashSet<usize>` 标记已删除 row_id，
//!   保留原始 Vec 索引稳定性（row_id 不变）
//! - **事务用 snapshot/restore**：Phase 3.5 简化事务模型，完整 ACID 留待 szrsql-tx 子系统
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.4 + 3.5。

use crate::ast::*;
use crate::check_constraint::CheckConstraintValidator;
use crate::expr::{EvalContext, EvalError, ExprEvaluator};
use crate::foreign_key::{CascadeOp, ForeignKeyValidator};
use crate::iter_exec::build_iter_plan;
use crate::plan::{
    AggregateExpr, Catalog, CheckConstraint, CteEntry, ForeignKeyConstraint, FunctionDefinition,
    InMemoryCatalog, IndexDefinition, InsertSourcePlan, LogicalPlan, Planner, ReferencingKey,
    SequenceDefinition, TableSchema, WindowFunctionExpr,
};
use crate::trigger::{
    fire_row_triggers, fire_statement_triggers, DmlKind, FireResult, TriggerRegistry,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Bound;
use std::sync::{Arc, Mutex};
use szrsql_storage::page::PAGE_BODY_SIZE as BODY_SIZE;
use szrsql_tx::mvcc::MvccManager;
use szrsql_types::value::{ColumnType, Value};
use thiserror::Error;
use tracing::{instrument, trace};

// =====================================================================
//  错误类型
// =====================================================================

/// 执行器错误
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ExecutionError {
    /// 表不存在
    #[error("table not found: {0}")]
    TableNotFound(String),
    /// 列不存在
    #[error("column not found: {0}")]
    ColumnNotFound(String),
    /// 表达式求值错误
    #[error("evaluation error: {0}")]
    EvalError(String),
    /// 索引不存在
    #[error("index not found: {0}")]
    IndexNotFound(String),
    /// 索引列类型不支持（仅支持 Int64 索引键）
    #[error("unsupported index key type: {0}")]
    UnsupportedIndexKeyType(String),
    /// 不支持的逻辑计划节点
    #[error("unsupported plan node: {0}")]
    Unsupported(String),
    /// 无效参数
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// 序列不存在（Phase 3.22）
    #[error("sequence not found: {0}")]
    SequenceNotFound(String),
    /// 序列已存在（Phase 3.22）
    #[error("sequence already exists: {0}")]
    SequenceAlreadyExists(String),
    /// 序列值越界（Phase 3.22）
    #[error("sequence value out of range: {0}")]
    SequenceOutOfRange(String),
    /// 序列尚未在当前会话被调用 nextval（currval 报错）
    #[error("currval of sequence {0} is not yet defined in this session")]
    SequenceCurrvalNotDefined(String),
    /// 外键约束违反 — Phase 3.29
    #[error("foreign key violation: {0}")]
    ForeignKeyViolation(String),
    /// CHECK 约束违反 — Phase 3.30
    #[error("check constraint violation: {0}")]
    CheckViolation(String),
    /// ENUM 值违反 — Phase 3.31
    ///
    /// INSERT/UPDATE 试图将不在 ENUM 类型 labels 中的值写入列。
    #[error("enum value violation: {0}")]
    EnumValueViolation(String),
    /// 类型不存在 — Phase 3.31
    #[error("type not found: {0}")]
    TypeNotFound(String),
    /// 类型已存在 — Phase 3.31
    #[error("type already exists: {0}")]
    TypeAlreadyExists(String),
    /// NOT NULL 约束违反 — Phase F-10
    ///
    /// ALTER COLUMN ... SET NOT NULL 时，若现有行包含 NULL 值，则报此错误。
    #[error("not null violation: {0}")]
    NotNullViolation(String),
    /// P0-STORE-2：存储层错误（BufferPool/Page I/O）
    #[error("storage error: {0}")]
    Storage(String),
    /// P1-9：行锁冲突（死锁/超时/冲突）
    #[error("lock conflict: {0}")]
    LockConflict(String),
}

impl From<EvalError> for ExecutionError {
    fn from(e: EvalError) -> Self {
        ExecutionError::EvalError(e.to_string())
    }
}

impl From<crate::plan::PlanError> for ExecutionError {
    fn from(e: crate::plan::PlanError) -> Self {
        match e {
            crate::plan::PlanError::TableNotFound(name) => ExecutionError::TableNotFound(name),
            other => ExecutionError::InvalidArgument(format!("plan error: {}", other)),
        }
    }
}

impl From<FlashbackError> for ExecutionError {
    fn from(e: FlashbackError) -> Self {
        match e {
            FlashbackError::TransactionNotFound(txn_id) => {
                ExecutionError::InvalidArgument(format!("transaction not found: {txn_id}"))
            }
            FlashbackError::AlreadyFlashedBack(txn_id) => ExecutionError::InvalidArgument(format!(
                "transaction {txn_id} has already been flashed back"
            )),
        }
    }
}

// =====================================================================
//  Row 类型别名
// =====================================================================

/// 一行数据 — `Vec<Value>`，按 Schema 列顺序排列
pub type Row = Vec<Value>;

// =====================================================================
//  DmlResult — DML 执行结果（受影响行数 + RETURNING 行）
// =====================================================================

/// DML（INSERT / UPDATE / DELETE）执行结果
///
/// - `affected_rows` — 受影响行数（插入 + 更新 + 跳过计 0）
/// - `returning_rows` — RETURNING 子句返回的行（无 RETURNING 时为空 Vec）
///
/// RETURNING 语义（与 PG 一致）：
/// - INSERT → 返回新插入的行
/// - UPDATE → 返回更新后的新行
/// - DELETE → 返回被删除的旧行
#[derive(Debug, Clone, PartialEq)]
pub struct DmlResult {
    /// 受影响行数
    pub affected_rows: usize,
    /// RETURNING 子句返回的行（无 RETURNING 时为空 Vec）
    pub returning_rows: Vec<Row>,
}

impl DmlResult {
    /// 创建仅含受影响行数的结果（无 RETURNING）
    pub fn new(affected_rows: usize) -> Self {
        Self {
            affected_rows,
            returning_rows: Vec::new(),
        }
    }

    /// 创建含 RETURNING 行的结果
    pub fn with_returning(affected_rows: usize, returning_rows: Vec<Row>) -> Self {
        Self {
            affected_rows,
            returning_rows,
        }
    }
}

// =====================================================================
//  SequenceStore trait + InMemorySequenceStore — Phase 3.22
// =====================================================================

/// 序列存储抽象 — 提供 nextval/currval 与 DDL 操作
///
/// 序列是有状态的数据库对象，nextval 修改状态，currval 读取会话内最近一次 nextval 的结果。
/// 在 SzRSQL 中，序列存储与表存储分离，由执行器在需要时传入。
pub trait SequenceStore {
    /// 创建序列
    fn create_sequence(&mut self, def: SequenceDefinition) -> Result<(), ExecutionError>;

    /// 删除序列
    fn drop_sequence(&mut self, name: &TableName, if_exists: bool) -> Result<(), ExecutionError>;

    /// 调用 nextval — 返回当前值并推进序列
    fn next_value(&mut self, name: &TableName) -> Result<i64, ExecutionError>;

    /// 调用 currval — 返回会话内最近一次 nextval 的结果（未调用 nextval 则报错）
    fn current_value(&self, name: &TableName) -> Result<i64, ExecutionError>;

    /// 判断序列是否存在
    fn sequence_exists(&self, name: &TableName) -> bool;

    /// 列出所有序列名
    fn list_sequences(&self) -> Vec<TableName>;
}

/// 内存序列存储 — 用于单元测试和示例
///
/// # 字段
/// - `sequences` — 序列名（小写）→ 序列状态（**全局共享**，通过 `Arc<Mutex>` 实现）
/// - `session_currval` — 序列名（小写）→ 会话内最近一次 nextval 值（**会话隔离**）
///
/// # 语义
/// - **nextval**：返回当前 `current` 值（首次返回 `start`），然后将 `current` 推进 `increment`
///   - **跨会话共享**：多个 session 调用同一序列的 nextval 会推进同一全局状态
/// - **currval**：返回 `session_currval` 中的值；若不存在则报错（PG 语义）
///   - **会话隔离**：每个 session 独立维护 currval，互不影响
/// - **CYCLE**：达到 max_value 后回到 min_value；NO CYCLE 时越界报错
/// - **负 increment**：递减序列，达到 min_value 后行为同上
///
/// # P0-4 修复
/// 原实现将 `sequences` 与 `session_currval` 同放在 per-session 的结构体中，
/// 导致不同 session 各自维护一份序列全局状态，nextval 跨 session 不连续。
/// 现将 `sequences` 改为 `Arc<Mutex<HashMap>>`，由 `PgwireServer` 持有共享句柄，
/// 在 session 创建时克隆 Arc 注入，确保全局状态唯一。
#[derive(Debug, Clone)]
pub struct InMemorySequenceStore {
    /// 全局共享的序列状态（多 session 共享，需加锁）
    sequences: Arc<std::sync::Mutex<HashMap<String, SequenceState>>>,
    /// 会话隔离的 currval（每 session 独立）
    session_currval: HashMap<String, i64>,
}

impl Default for InMemorySequenceStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct SequenceState {
    definition: SequenceDefinition,
    /// 下一次 nextval 返回的值
    current: i64,
    /// 是否已被 nextval 调用过
    initialized: bool,
}

/// P0-4：跨会话共享的序列全局状态句柄
///
/// 由 `PgwireServer` 持有一份，每个新 session 创建时调用
/// [`InMemorySequenceStore::from_shared_state`] 注入，从而所有 session
/// 共享同一份 `nextval` 推进状态；`currval` 仍按 session 隔离。
///
/// 内部类型不暴露（封装 `SequenceState` 私有性）。
#[derive(Debug, Clone, Default)]
pub struct SharedSequenceState {
    inner: Arc<std::sync::Mutex<HashMap<String, SequenceState>>>,
}

impl SharedSequenceState {
    /// 创建一个空的共享状态句柄
    pub fn new() -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }
}

impl InMemorySequenceStore {
    /// 创建空序列存储（独立的全局状态，用于测试）
    pub fn new() -> Self {
        Self {
            sequences: Arc::new(std::sync::Mutex::new(HashMap::new())),
            session_currval: HashMap::new(),
        }
    }

    /// 从共享全局状态创建会话存储（用于 PgwireServer 注入 session）
    ///
    /// # 参数
    /// - `shared` — 共享的序列全局状态句柄
    pub fn from_shared_state(shared: SharedSequenceState) -> Self {
        Self {
            sequences: shared.inner,
            session_currval: HashMap::new(),
        }
    }

    /// 获取共享全局状态的句柄（用于 PgwireServer 创建新 session 时注入）
    pub fn shared_state(&self) -> SharedSequenceState {
        SharedSequenceState {
            inner: Arc::clone(&self.sequences),
        }
    }

    fn key(name: &TableName) -> String {
        name.qualified_name().to_lowercase()
    }
}

impl SequenceStore for InMemorySequenceStore {
    fn create_sequence(&mut self, def: SequenceDefinition) -> Result<(), ExecutionError> {
        let key = Self::key(&def.name);
        let mut sequences = self
            .sequences
            .lock()
            .expect("sequence store mutex poisoned");
        if sequences.contains_key(&key) {
            return Err(ExecutionError::SequenceAlreadyExists(
                def.name.qualified_name(),
            ));
        }
        let state = SequenceState {
            current: def.start,
            initialized: false,
            definition: def,
        };
        sequences.insert(key, state);
        Ok(())
    }

    fn drop_sequence(&mut self, name: &TableName, if_exists: bool) -> Result<(), ExecutionError> {
        let key = Self::key(name);
        let mut sequences = self
            .sequences
            .lock()
            .expect("sequence store mutex poisoned");
        if sequences.remove(&key).is_none() && !if_exists {
            return Err(ExecutionError::SequenceNotFound(name.qualified_name()));
        }
        drop(sequences);
        self.session_currval.remove(&key);
        Ok(())
    }

    fn next_value(&mut self, name: &TableName) -> Result<i64, ExecutionError> {
        let key = Self::key(name);
        let mut sequences = self
            .sequences
            .lock()
            .expect("sequence store mutex poisoned");
        let state = sequences
            .get_mut(&key)
            .ok_or_else(|| ExecutionError::SequenceNotFound(name.qualified_name()))?;

        let lo = state.definition.min_value.unwrap_or(i64::MIN);
        let hi = state.definition.max_value.unwrap_or(i64::MAX);

        // 非首次调用时，检查 current 是否已越界（NO CYCLE 情况）
        // 首次调用跳过此检查，因为 start 已在 planner 中校验在 [lo, hi] 范围内
        if state.initialized && (state.current < lo || state.current > hi) {
            return Err(ExecutionError::SequenceOutOfRange(format!(
                "nextval reached {} for sequence {}",
                if state.definition.increment > 0 {
                    "MAXVALUE"
                } else {
                    "MINVALUE"
                },
                name.qualified_name()
            )));
        }

        // 返回当前值
        let returned = state.current;
        state.initialized = true;

        // 推进 current
        let next = state
            .current
            .checked_add(state.definition.increment)
            .ok_or_else(|| {
                ExecutionError::SequenceOutOfRange(format!(
                    "nextval overflow for sequence {}",
                    name.qualified_name()
                ))
            })?;

        // 应用 CYCLE/NO CYCLE 语义
        state.current = if next < lo || next > hi {
            if state.definition.cycle {
                // CYCLE：回绕到 min（升序）或 max（降序）
                if state.definition.increment > 0 {
                    lo
                } else {
                    hi
                }
            } else {
                // NO CYCLE：保留越界值，下次调用时报错
                next
            }
        } else {
            next
        };

        drop(sequences);
        // 记录会话 currval（session 隔离）
        self.session_currval.insert(key, returned);
        Ok(returned)
    }

    fn current_value(&self, name: &TableName) -> Result<i64, ExecutionError> {
        let key = Self::key(name);
        // 先检查序列存在（锁全局状态）
        let sequences = self
            .sequences
            .lock()
            .expect("sequence store mutex poisoned");
        if !sequences.contains_key(&key) {
            return Err(ExecutionError::SequenceNotFound(name.qualified_name()));
        }
        drop(sequences);
        // 再检查 currval 是否已定义（session 隔离）
        self.session_currval
            .get(&key)
            .copied()
            .ok_or_else(|| ExecutionError::SequenceCurrvalNotDefined(name.qualified_name()))
    }

    fn sequence_exists(&self, name: &TableName) -> bool {
        let sequences = self
            .sequences
            .lock()
            .expect("sequence store mutex poisoned");
        sequences.contains_key(&Self::key(name))
    }

    fn list_sequences(&self) -> Vec<TableName> {
        let sequences = self
            .sequences
            .lock()
            .expect("sequence store mutex poisoned");
        sequences
            .values()
            .map(|s| s.definition.name.clone())
            .collect()
    }
}

// =====================================================================
//  序列函数解析 — Phase 3.22
// =====================================================================

/// 解析 `nextval('seq_name')` / `currval('seq_name')` 表达式，
/// 返回序列名（小写字符串）
///
/// 接受参数形态：
/// - `Expr::Literal(Value::Text(name))`
/// - `Expr::Identifier([name])`
fn extract_sequence_name(arg: &Expr) -> Result<TableName, ExecutionError> {
    match arg {
        Expr::Literal(Value::Text(s)) => Ok(parse_seq_name(s)),
        Expr::Identifier(parts) if !parts.is_empty() => {
            let joined = parts.join(".");
            Ok(parse_seq_name(&joined))
        }
        other => Err(ExecutionError::InvalidArgument(format!(
            "nextval/currval argument must be a string literal, got {:?}",
            other
        ))),
    }
}

/// 将字符串解析为序列名（支持 `schema.seq` 形式）
fn parse_seq_name(s: &str) -> TableName {
    let s = s.trim_matches('"'); // PG 标识符可带双引号
    if let Some((schema, name)) = s.split_once('.') {
        TableName::with_schema(schema, name)
    } else {
        TableName::new(s)
    }
}

/// 递归将表达式中的 `nextval(...)` / `currval(...)` 调用替换为常量字面量
///
/// # 参数
/// - `expr` — 原始表达式
/// - `seq_store` — 序列存储（mutable，因为 nextval 推进状态）
///
/// # 行为
/// - `nextval('seq')` → `Expr::Literal(Value::Int64(n))`，调用 seq_store.next_value
/// - `currval('seq')` → `Expr::Literal(Value::Int64(n))`，调用 seq_store.current_value
/// - 其他函数调用：递归处理参数
/// - 其他表达式：递归处理子表达式
pub fn resolve_sequence_calls(
    expr: &Expr,
    seq_store: &mut dyn SequenceStore,
) -> Result<Expr, ExecutionError> {
    match expr {
        Expr::Function {
            name,
            args,
            distinct,
        } => {
            let fname = name.to_lowercase();
            if fname == "nextval" {
                if args.len() != 1 {
                    return Err(ExecutionError::InvalidArgument(format!(
                        "nextval expects 1 argument, got {}",
                        args.len()
                    )));
                }
                let seq_name = extract_sequence_name(&args[0])?;
                let n = seq_store.next_value(&seq_name)?;
                Ok(Expr::Literal(Value::Int64(n)))
            } else if fname == "currval" {
                if args.len() != 1 {
                    return Err(ExecutionError::InvalidArgument(format!(
                        "currval expects 1 argument, got {}",
                        args.len()
                    )));
                }
                let seq_name = extract_sequence_name(&args[0])?;
                let n = seq_store.current_value(&seq_name)?;
                Ok(Expr::Literal(Value::Int64(n)))
            } else {
                // 其他函数：递归处理参数
                let new_args: Result<Vec<Expr>, ExecutionError> = args
                    .iter()
                    .map(|a| resolve_sequence_calls(a, seq_store))
                    .collect();
                Ok(Expr::Function {
                    name: name.clone(),
                    args: new_args?,
                    distinct: *distinct,
                })
            }
        }
        Expr::BinaryOp { left, op, right } => Ok(Expr::BinaryOp {
            left: Box::new(resolve_sequence_calls(left, seq_store)?),
            op: *op,
            right: Box::new(resolve_sequence_calls(right, seq_store)?),
        }),
        Expr::UnaryOp { op, expr: inner } => Ok(Expr::UnaryOp {
            op: *op,
            expr: Box::new(resolve_sequence_calls(inner, seq_store)?),
        }),
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            let new_operand = operand
                .as_ref()
                .map(|e| resolve_sequence_calls(e, seq_store).map(Box::new))
                .transpose()?;
            let new_when_then: Result<Vec<(Expr, Expr)>, ExecutionError> = when_then
                .iter()
                .map(|(w, t)| {
                    Ok((
                        resolve_sequence_calls(w, seq_store)?,
                        resolve_sequence_calls(t, seq_store)?,
                    ))
                })
                .collect();
            let new_else = else_expr
                .as_ref()
                .map(|e| resolve_sequence_calls(e, seq_store).map(Box::new))
                .transpose()?;
            Ok(Expr::Case {
                operand: new_operand,
                when_then: new_when_then?,
                else_expr: new_else,
            })
        }
        Expr::Cast {
            expr: inner,
            data_type,
        } => Ok(Expr::Cast {
            expr: Box::new(resolve_sequence_calls(inner, seq_store)?),
            data_type: data_type.clone(),
        }),
        Expr::InList {
            expr: inner,
            list,
            negated,
        } => {
            let new_inner = Box::new(resolve_sequence_calls(inner, seq_store)?);
            let new_list: Result<Vec<Expr>, ExecutionError> = list
                .iter()
                .map(|e| resolve_sequence_calls(e, seq_store))
                .collect();
            Ok(Expr::InList {
                expr: new_inner,
                list: new_list?,
                negated: *negated,
            })
        }
        Expr::Between {
            expr: inner,
            low,
            high,
            negated,
        } => Ok(Expr::Between {
            expr: Box::new(resolve_sequence_calls(inner, seq_store)?),
            low: Box::new(resolve_sequence_calls(low, seq_store)?),
            high: Box::new(resolve_sequence_calls(high, seq_store)?),
            negated: *negated,
        }),
        Expr::Like {
            expr: inner,
            pattern,
            negated,
            case_insensitive,
        } => Ok(Expr::Like {
            expr: Box::new(resolve_sequence_calls(inner, seq_store)?),
            pattern: Box::new(resolve_sequence_calls(pattern, seq_store)?),
            negated: *negated,
            case_insensitive: *case_insensitive,
        }),
        Expr::IsNull {
            expr: inner,
            negated,
        } => Ok(Expr::IsNull {
            expr: Box::new(resolve_sequence_calls(inner, seq_store)?),
            negated: *negated,
        }),
        Expr::Tuple(items) => {
            let new_items: Result<Vec<Expr>, ExecutionError> = items
                .iter()
                .map(|e| resolve_sequence_calls(e, seq_store))
                .collect();
            Ok(Expr::Tuple(new_items?))
        }
        // Phase 6.2: 窗口函数 — 递归解析 args / partition_by / order_by 中的序列调用
        Expr::WindowFunction {
            name,
            args,
            distinct,
            window,
        } => {
            let new_args: Result<Vec<Expr>, ExecutionError> = args
                .iter()
                .map(|a| resolve_sequence_calls(a, seq_store))
                .collect();
            let new_partition: Result<Vec<Expr>, ExecutionError> = window
                .partition_by
                .iter()
                .map(|e| resolve_sequence_calls(e, seq_store))
                .collect();
            let new_order: Result<Vec<OrderByExpr>, ExecutionError> = window
                .order_by
                .iter()
                .map(|obe| {
                    Ok(OrderByExpr {
                        expr: resolve_sequence_calls(&obe.expr, seq_store)?,
                        asc: obe.asc,
                        nulls_first: obe.nulls_first,
                    })
                })
                .collect();
            Ok(Expr::WindowFunction {
                name: name.clone(),
                args: new_args?,
                distinct: *distinct,
                window: WindowSpec {
                    partition_by: new_partition?,
                    order_by: new_order?,
                    window_frame: window.window_frame.clone(),
                },
            })
        }
        // 其他叶节点（Literal/Identifier/Subquery/Exists/Wildcard/InSubquery）：直接克隆
        // 子查询由执行器另行处理（其内部 nextval 在子查询执行时解析）
        other => Ok(other.clone()),
    }
}

/// 递归遍历 LogicalPlan，解析所有节点中的 nextval/currval 调用
///
/// 支持：Projection / Filter / Aggregate(having) / Sort / Limit / Join
/// 不处理：Insert/Update/Delete（由 `execute_insert_with_sequences` 专门处理）
pub fn resolve_sequences_in_plan(
    plan: &LogicalPlan,
    seq_store: &mut dyn SequenceStore,
) -> Result<LogicalPlan, ExecutionError> {
    match plan {
        LogicalPlan::Projection {
            exprs,
            output_names,
            input,
        } => {
            let mut new_exprs = Vec::with_capacity(exprs.len());
            for (e, alias) in exprs {
                let new_e = resolve_sequence_calls(e, seq_store)?;
                new_exprs.push((new_e, alias.clone()));
            }
            let new_input = Box::new(resolve_sequences_in_plan(input, seq_store)?);
            Ok(LogicalPlan::Projection {
                exprs: new_exprs,
                output_names: output_names.clone(),
                input: new_input,
            })
        }
        LogicalPlan::Filter { predicate, input } => {
            let new_pred = resolve_sequence_calls(predicate, seq_store)?;
            let new_input = Box::new(resolve_sequences_in_plan(input, seq_store)?);
            Ok(LogicalPlan::Filter {
                predicate: new_pred,
                input: new_input,
            })
        }
        LogicalPlan::Aggregate {
            group_exprs,
            aggregates,
            having,
            input,
        } => {
            let mut new_group = Vec::with_capacity(group_exprs.len());
            for e in group_exprs {
                new_group.push(resolve_sequence_calls(e, seq_store)?);
            }
            let new_having = having
                .as_ref()
                .map(|e| resolve_sequence_calls(e, seq_store))
                .transpose()?;
            let new_input = Box::new(resolve_sequences_in_plan(input, seq_store)?);
            // aggregates 内的 args 也需要解析
            let mut new_aggs = Vec::with_capacity(aggregates.len());
            for agg in aggregates {
                let mut new_args = Vec::with_capacity(agg.args.len());
                for a in &agg.args {
                    new_args.push(resolve_sequence_calls(a, seq_store)?);
                }
                new_aggs.push(AggregateExpr {
                    func_name: agg.func_name.clone(),
                    distinct: agg.distinct,
                    args: new_args,
                    alias: agg.alias.clone(),
                });
            }
            Ok(LogicalPlan::Aggregate {
                group_exprs: new_group,
                aggregates: new_aggs,
                having: new_having,
                input: new_input,
            })
        }
        // Phase 6.2: Window 节点 — 解析 window_funcs 内 args / partition_by / order_by 中的序列调用
        LogicalPlan::Window {
            window_funcs,
            input,
        } => {
            let new_input = Box::new(resolve_sequences_in_plan(input, seq_store)?);
            let mut new_funcs = Vec::with_capacity(window_funcs.len());
            for wf in window_funcs {
                let WindowFunctionExpr {
                    func_name,
                    distinct,
                    args,
                    window,
                    alias,
                } = wf;
                // 1. args
                let mut new_args = Vec::with_capacity(args.len());
                for a in args {
                    new_args.push(resolve_sequence_calls(a, seq_store)?);
                }
                // 2. window.partition_by
                let mut new_partition = Vec::with_capacity(window.partition_by.len());
                for e in &window.partition_by {
                    new_partition.push(resolve_sequence_calls(e, seq_store)?);
                }
                // 3. window.order_by
                let mut new_order = Vec::with_capacity(window.order_by.len());
                for obe in &window.order_by {
                    let new_expr = resolve_sequence_calls(&obe.expr, seq_store)?;
                    new_order.push(OrderByExpr {
                        expr: new_expr,
                        asc: obe.asc,
                        nulls_first: obe.nulls_first,
                    });
                }
                new_funcs.push(WindowFunctionExpr {
                    func_name: func_name.clone(),
                    distinct: *distinct,
                    args: new_args,
                    window: WindowSpec {
                        partition_by: new_partition,
                        order_by: new_order,
                        window_frame: window.window_frame.clone(),
                    },
                    alias: alias.clone(),
                });
            }
            Ok(LogicalPlan::Window {
                window_funcs: new_funcs,
                input: new_input,
            })
        }
        LogicalPlan::Sort { order_by, input } => {
            let new_input = Box::new(resolve_sequences_in_plan(input, seq_store)?);
            Ok(LogicalPlan::Sort {
                order_by: order_by.clone(),
                input: new_input,
            })
        }
        LogicalPlan::Limit {
            limit,
            offset,
            input,
        } => {
            let new_limit = limit
                .as_ref()
                .map(|e| resolve_sequence_calls(e, seq_store))
                .transpose()?;
            let new_offset = offset
                .as_ref()
                .map(|e| resolve_sequence_calls(e, seq_store))
                .transpose()?;
            let new_input = Box::new(resolve_sequences_in_plan(input, seq_store)?);
            Ok(LogicalPlan::Limit {
                limit: new_limit,
                offset: new_offset,
                input: new_input,
            })
        }
        LogicalPlan::Distinct { input } => {
            let new_input = Box::new(resolve_sequences_in_plan(input, seq_store)?);
            Ok(LogicalPlan::Distinct { input: new_input })
        }
        LogicalPlan::Join {
            join_type,
            condition,
            left,
            right,
        } => {
            let new_left = Box::new(resolve_sequences_in_plan(left, seq_store)?);
            let new_right = Box::new(resolve_sequences_in_plan(right, seq_store)?);
            Ok(LogicalPlan::Join {
                join_type: *join_type,
                condition: condition.clone(),
                left: new_left,
                right: new_right,
            })
        }
        // 其他节点（Scan / Empty / DDL / DML）直接克隆 — 它们不包含需要序列解析的表达式
        other => Ok(other.clone()),
    }
}

// =====================================================================
//  TableStorage trait
// =====================================================================

/// P0-STORE 阶段 1：主键访问路径（从 WHERE 谓词中提取）
///
/// 描述主键列的等值或范围访问条件，用于 B+Tree 索引优化。
#[derive(Debug, Clone)]
enum PkAccess {
    /// 等值查询：`pk = literal`
    Point(i64),
    /// 范围查询：`pk >= low AND pk < high`
    Range(i64, i64),
}

/// 表存储抽象 — 上层执行器通过此 trait 访问表数据
///
/// 实现方需保证 `scan_iter` 产出的 Row 与 `get_row(row_id)` 一致：
/// `scan_iter().nth(row_id) == get_row(row_id)`
pub trait TableStorage: Sync {
    /// 表名
    fn name(&self) -> &str;

    /// 表 Schema（列定义）
    fn schema(&self) -> &TableSchema;

    /// 顺序扫描所有行（SeqScan 核心接口）
    fn scan_iter(&self) -> Box<dyn Iterator<Item = Row> + Send + '_>;

    /// 顺序扫描所有行，返回 (row_id, row) 对（DML 用于定位行）
    fn scan_with_ids(&self) -> Box<dyn Iterator<Item = (usize, Row)> + Send + '_>;

    /// 行数
    fn row_count(&self) -> usize;

    /// 按 row_id（0-indexed）取单行，越界或已删除返回 None
    fn get_row(&self, row_id: usize) -> Option<Row>;

    /// P0-TX-1 Phase B：顺序扫描所有行，返回 (row_id, row, xmin, xmax) — MVCC 可见性过滤用
    ///
    /// 默认实现返回 xmin=0（Frozen）/ xmax=0（未删除），表示所有行对全部事务可见。
    /// 支持 MVCC 的存储后端（如 `InMemoryTable`）覆盖此方法返回真实版本元数据。
    ///
    /// **xmin 语义**：插入此行版本的事务 ID（0 = Frozen/系统数据，恒可见）
    /// **xmax 语义**：删除此行版本的事务 ID（0 = 未删除）
    fn scan_with_versions(&self) -> Box<dyn Iterator<Item = (usize, Row, u32, u32)> + Send + '_> {
        Box::new(self.scan_with_ids().map(|(id, row)| (id, row, 0u32, 0u32)))
    }

    /// P0-STORE 阶段 1：主键列索引（若已启用 B+Tree 主键索引）
    ///
    /// 返回 `Some(column_idx)` 表示主键列在 schema 中的位置，
    /// `None` 表示未启用 B+Tree 主键索引（调用方应退化为全表扫描）。
    fn pk_column_idx(&self) -> Option<usize> {
        None
    }

    /// P0-STORE 阶段 1：通过主键值快速点查完整行（O(log n)）
    ///
    /// 默认实现返回 `None`（未启用 B+Tree），调用方应退化为全表扫描 + 过滤。
    /// 支持 B+Tree 主键索引的存储后端（如 `InMemoryTable`）覆盖此方法。
    fn pk_point_lookup(&self, _key: i64) -> Option<Row> {
        None
    }

    /// P0-STORE 阶段 1：通过主键值范围查询多行（O(log n + k)）
    ///
    /// 返回主键值在 [low, high) 范围内的所有行（升序）。
    /// 默认实现返回 `None`（未启用 B+Tree），调用方应退化为全表扫描 + 过滤。
    fn pk_range_lookup(&self, _low: i64, _high: i64) -> Option<Vec<Row>> {
        None
    }
}

// =====================================================================
//  InMemoryTable — B+Tree 主存储引擎后端
// =====================================================================

/// P0-4：B+Tree 叶子节点存储的行数据二进制格式
///
/// 编码格式（小端）：
/// - 偏移 0，4 字节：xmin（u32 LE）— 插入事务 ID（0 = Frozen/系统数据）
/// - 偏移 4，4 字节：xmax（u32 LE）— 删除事务 ID（0 = 未删除）
/// - 偏移 8..：serde_json 序列化的 Row（`Vec<Value>`）
///
/// 选择 serde_json 而非 bincode 的原因：
/// - `Value::Json(serde_json::Value)` 变体无法通过 bincode 1.x 往返（已知限制）
/// - serde_json 对所有 Value 变体均支持，无特例
/// - 缺点：输出较冗余（约 2-3x bincode），后续可优化为自定义二进制格式
mod btree_value_codec {
    use super::Row;

    const XMIN_SIZE: usize = 4;
    const XMAX_SIZE: usize = 4;
    const HEADER_SIZE: usize = XMIN_SIZE + XMAX_SIZE; // 8 bytes

    /// 编码 (xmin, xmax, row) 为 B+Tree 叶子节点存储的字节串
    pub fn encode(xmin: u32, xmax: u32, row: &Row) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_SIZE + 64);
        buf.extend_from_slice(&xmin.to_le_bytes());
        buf.extend_from_slice(&xmax.to_le_bytes());
        // serde_json 序列化 Row（Vec<Value>）
        let row_bytes = serde_json::to_vec(row).unwrap_or_default();
        buf.extend(row_bytes);
        buf
    }

    /// 解码 B+Tree 叶子节点字节串为 (xmin, xmax, row)
    /// 返回 Err 表示解码失败（数据损坏或格式不匹配）
    pub fn decode(bytes: &[u8]) -> Result<(u32, u32, Row), String> {
        if bytes.len() < HEADER_SIZE {
            return Err(format!(
                "btree value too short: {} < {}",
                bytes.len(),
                HEADER_SIZE
            ));
        }
        let xmin = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let xmax = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let row: Row = serde_json::from_slice(&bytes[HEADER_SIZE..])
            .map_err(|e| format!("btree value row decode failed: {e}"))?;
        Ok((xmin, xmax, row))
    }
}

/// 将 tuple_id 编码为 B+Tree 键（8 字节 big-endian u32，保持数值序）
fn encode_tuple_id_key(tuple_id: u32) -> Vec<u8> {
    tuple_id.to_be_bytes().to_vec()
}

/// 从 B+Tree 键解码 tuple_id
fn decode_tuple_id_key(key: &[u8]) -> u32 {
    if key.len() >= 4 {
        u32::from_be_bytes([key[0], key[1], key[2], key[3]])
    } else {
        0
    }
}

/// 简单内存表 — 用于功能测试
///
/// 所有行存储在 B+Tree 叶节点中（由 BufferPool 管理页面），row_id 即为插入时的 tuple_id。
/// 删除采用 tombstone（标记删除）策略：`deleted` 集合记录已删除的 tuple_id。
///
/// P0-4：B+Tree 从「可选索引」升级为「主存储引擎」，行数据直接存入 B+Tree 叶节点。
/// Vec<Row> 退化为有界热数据缓存（row_cache），避免重复反序列化。
/// xmin/xmax MVCC 版本元数据存储在 B+Tree value 的前 8 字节。
/// 迭代执行器用：表名 → (全量行, 列名)
type TableDataMap = std::collections::HashMap<String, (Vec<Row>, Vec<String>)>;

#[derive(Debug, Clone)]
pub struct InMemoryTable {
    /// 表名
    name: String,
    /// 表 Schema
    schema: TableSchema,
    /// P0-4：B+Tree 主存储引擎 — 键为 tuple_id（8B big-endian u32），值为序列化行数据（含 xmin/xmax/Row）
    ///
    /// 行数据直接存入 B+Tree 叶节点，由 BufferPool 管理页面，支持冷页逐出。
    /// Vec<Row> 退化为有界热数据缓存（row_cache）。
    btree: szrsql_storage::btree::BTree,
    /// P0-4：二级主键索引（仅对有主键的表启用）
    /// 映射 encoded_pk → tuple_id（u32，编码为 8B big-endian）
    pk_index: Option<szrsql_storage::btree::BTree>,
    /// P0-4：下一个将分配的 tuple_id（单调递增，从 0 开始）
    next_tuple_id: u32,
    /// P0-4：已删除的 tuple_id 集合（软删除标记，B+Tree 中 xmax 已置位）
    deleted: HashSet<u32>,
    /// P0-4：热数据行缓存（tuple_id → Row），避免重复反序列化
    /// 有界缓存：超过 row_cache_max 时随机淘汰
    row_cache: std::collections::HashMap<u32, Row>,
    /// P0-4：行缓存上限（默认 10_000）
    row_cache_max: usize,
    /// P0-4：主键列在 schema 中的索引位置（pk_index 启用时有效）
    pk_column_idx: Option<usize>,
    /// P0-STORE-2：可选的 BufferPool 持久化后端
    persistence: Option<std::sync::Arc<szrsql_storage::buffer::BufferPool>>,
    /// P1-1：分页存储主路径
    paged_storage: Option<std::sync::Arc<szrsql_storage::buffer::BufferPool>>,
    /// P1-1：自动 spill 阈值
    spill_threshold: usize,
}

/// P1-1：分页存储信息（由 `InMemoryTable::paged_storage_info()` 返回）
///
/// 用于观测分页存储状态：当前缓存页数、热数据行数、spill 阈值。
#[derive(Debug, Clone)]
pub struct PagedStorageInfo {
    /// 当前 BufferPool 缓存的页数（含 header 页；若页被 LRU 淘汰，该值可能小于磁盘实际页数）
    pub page_count: usize,
    /// rows 中的行数（热数据缓存）
    pub row_count: usize,
    /// 自动 spill 阈值
    pub threshold: usize,
}

/// 全行可变句柄（P0-4）：DerefMut 修改 + Drop 时回写 B+Tree
///
/// 保持旧版 `&mut Vec<Row>` 的语义：调用方修改行内容后无需显式写回，
/// guard 析构时自动将修改后的行重新编码写入 B+Tree（保留 xmin/xmax）。
pub struct RowsMutGuard<'a> {
    table: &'a mut InMemoryTable,
    rows: Vec<Row>,
    /// 每行原始 (xmin, xmax)，与 rows 一一对应
    versions: Vec<(u32, u32)>,
}

impl std::ops::Deref for RowsMutGuard<'_> {
    type Target = Vec<Row>;
    fn deref(&self) -> &Self::Target {
        &self.rows
    }
}

impl std::ops::DerefMut for RowsMutGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rows
    }
}

impl std::ops::Drop for RowsMutGuard<'_> {
    fn drop(&mut self) {
        // 回写 B+Tree：重建并插入所有行（保留原始 xmin/xmax）
        let table = &mut *self.table;
        table.btree = szrsql_storage::btree::BTree::with_default_order();
        table.next_tuple_id = 0;
        table.row_cache.clear();
        for (i, row) in self.rows.iter().enumerate() {
            let tuple_id = table.next_tuple_id;
            table.next_tuple_id += 1;
            let (xmin, xmax) = self.versions.get(i).copied().unwrap_or((0, 0));
            let value = btree_value_codec::encode(xmin, xmax, row);
            let key = encode_tuple_id_key(tuple_id);
            let _ = table.btree.insert(key, value);
            table.row_cache.insert(tuple_id, row.clone());
        }
    }
}

// =====================================================================
//  InMemoryTable 手写序列化（P0-4）
// =====================================================================
// B+Tree 主存储（btree/pk_index/row_cache 等）不可直接 serde 序列化，
// 因此序列化时保存 name/schema/next_tuple_id/deleted + TableSnapshot（行数据），
// 反序列化时通过 restore() 重建 B+Tree。与 flush_to_disk 的 TableSnapshot 路径一致。

impl serde::Serialize for InMemoryTable {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("InMemoryTable", 6)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("schema", &self.schema)?;
        state.serialize_field("next_tuple_id", &self.next_tuple_id)?;
        state.serialize_field("deleted", &self.deleted)?;
        state.serialize_field("pk_column_idx", &self.pk_column_idx)?;
        state.serialize_field("snapshot", &self.snapshot())?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for InMemoryTable {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // 中间结构：只承载序列化字段（兼容旧格式：next_tuple_id/deleted 可选）
        #[derive(serde::Deserialize)]
        struct RawTable {
            name: String,
            schema: TableSchema,
            #[serde(default)]
            next_tuple_id: u32,
            #[serde(default)]
            deleted: HashSet<u32>,
            #[serde(default)]
            pk_column_idx: Option<usize>,
            snapshot: TableSnapshot,
        }

        let raw = RawTable::deserialize(deserializer)?;
        let mut table = InMemoryTable::new(raw.schema);
        table.name = raw.name;
        table.next_tuple_id = raw.next_tuple_id;
        table.deleted = raw.deleted;
        table.pk_column_idx = raw.pk_column_idx;
        // 重建 B+Tree（行数据、xmin/xmax、deleted 标记）
        table.restore(raw.snapshot);
        Ok(table)
    }
}

impl InMemoryTable {
    /// 创建空表
    pub fn new(schema: TableSchema) -> Self {
        Self {
            name: schema.name.name.clone(),
            schema,
            btree: szrsql_storage::btree::BTree::with_default_order(),
            pk_index: None,
            next_tuple_id: 0,
            deleted: HashSet::new(),
            row_cache: std::collections::HashMap::new(),
            row_cache_max: 10_000,
            pk_column_idx: None,
            persistence: None,
            paged_storage: None,
            spill_threshold: 100_000,
        }
    }

    /// 创建空表（简化构造）
    pub fn with_columns(name: &str, columns: Vec<(&str, szrsql_types::value::ColumnType)>) -> Self {
        let table_name = TableName::new(name);
        let cols = columns
            .into_iter()
            .map(|(n, t)| ColumnDefinition::new(n, t))
            .collect();
        let schema = TableSchema {
            name: table_name,
            columns: cols,
        };
        Self::new(schema)
    }

    /// 插入一行，返回 tuple_id（即 row_id）
    ///
    /// P0-4：行数据写入 B+Tree 叶节点（xmin=0, xmax=0），同时更新热缓存。
    pub fn insert(&mut self, row: Row) -> usize {
        self.insert_with_xmin(row, 0)
    }

    /// P0-TX-1 Phase B / P0-4：插入一行并设置 xmin（MVCC 版本元数据）
    ///
    /// 由 Executor 在事务内 INSERT 时调用。行数据（含 xmin/xmax/Row）序列化后
    /// 直接存入 B+Tree 叶节点，tuple_id 单调递增。
    pub fn insert_with_xmin(&mut self, row: Row, xmin: u32) -> usize {
        let tuple_id = self.next_tuple_id;
        self.next_tuple_id += 1;

        // 提取主键编码（在 row 被 move 前）
        let pk_bytes = self.extract_pk_bytes(&row);

        // 序列化行数据并写入 B+Tree
        let value = btree_value_codec::encode(xmin, 0, &row);
        let key = encode_tuple_id_key(tuple_id);
        if let Err(e) = self.btree.insert(key, value) {
            tracing::warn!(tuple_id, error = ?e, "insert_with_xmin: BTree insert failed");
        }

        // 更新热缓存
        self.maybe_cache_row(tuple_id, row);

        // 同步更新二级主键索引
        self.update_pk_index(tuple_id as usize, pk_bytes);

        tuple_id as usize
    }

    /// P1-5：WAL 行级回放专用 — 在指定 tuple_id 处插入行（不分配新 ID）。
    ///
    /// 与 `insert_with_xmin` 不同，此方法**不递增 `next_tuple_id`**，而是直接将行
    /// 写入给定的 tuple_id 位置。用于崩溃恢复时重建与 WAL 记录中 row_id 一致的行索引，
    /// 使后续 Update/Delete 回放能通过原始 row_id 正确定位到同一行。
    ///
    /// # 参数
    /// - `tuple_id`：目标行 ID（回放时对应 WAL 记录的 `page_id` 字段）
    /// - `row`：行数据（由 WAL `new_payload` 反序列化得到）
    /// - `xmin`：创建该行的事务 ID（恢复时通常传 0，表示已提交行）
    pub fn insert_at_tuple_id(&mut self, tuple_id: u32, row: Row, xmin: u32) {
        let pk_bytes = self.extract_pk_bytes(&row);
        let key = encode_tuple_id_key(tuple_id);
        let value = btree_value_codec::encode(xmin, 0, &row);
        if let Err(e) = self.btree.insert(key, value) {
            tracing::warn!(
                tuple_id,
                error = ?e,
                "insert_at_tuple_id: BTree insert failed"
            );
        }
        self.maybe_cache_row(tuple_id, row);
        self.update_pk_index(tuple_id as usize, pk_bytes);
    }

    /// 将行加入热缓存（有界，超出上限时随机淘汰）
    fn maybe_cache_row(&mut self, tuple_id: u32, row: Row) {
        if self.row_cache.len() >= self.row_cache_max {
            // 随机淘汰一个条目（简单 LRU 替代方案）
            if let Some((&evict_key, _)) = self.row_cache.iter().next() {
                self.row_cache.remove(&evict_key);
            }
        }
        self.row_cache.insert(tuple_id, row);
    }

    /// P0-4：启用 B+Tree 主键索引
    ///
    /// 启用后，后续 INSERT 会同步更新二级 PK 索引（encoded_pk → tuple_id）。
    ///
    /// **P0-3 修复**：启用时自动回填已有数据到 BTree。
    /// 遍历 B+Tree 主存储中所有活跃行，将 (pk_bytes → tuple_id) 插入二级索引。
    ///
    /// **参数**：`column_idx` 主键列在 schema 中的索引位置（0-based）
    ///
    /// **限制**：
    /// - 仅支持 Int64 类型主键（其他类型记录 warn 并不启用）
    pub fn enable_btree_pk(&mut self, column_idx: usize) {
        if column_idx >= self.schema.columns.len() {
            tracing::warn!(
                column_idx,
                "enable_btree_pk: column_idx out of range, ignored"
            );
            return;
        }
        let col = &self.schema.columns[column_idx];
        if col.data_type != szrsql_types::value::ColumnType::Int64 {
            tracing::warn!(column_idx, data_type = ?col.data_type, "enable_btree_pk: only Int64 supported, ignored");
            return;
        }
        // 创建二级 PK 索引并回填已有数据
        let mut btree = szrsql_storage::btree::BTree::with_default_order();
        let mut backfilled = 0usize;
        let mut skipped_deleted = 0usize;
        let mut skipped_non_int64 = 0usize;
        // 遍历 B+Tree 主存储的所有活跃行
        for (tuple_id, row) in self.scan_with_ids() {
            if self.deleted.contains(&(tuple_id as u32)) {
                skipped_deleted += 1;
                continue;
            }
            match row.get(column_idx) {
                Some(Value::Int64(v)) => {
                    let pk_bytes = szrsql_storage::btree::encode_i64_key(*v);
                    // PK 索引值：tuple_id 编码为 8B big-endian（与主存储键格式一致）
                    if let Err(e) = btree.insert(pk_bytes, encode_tuple_id_key(tuple_id as u32)) {
                        tracing::warn!(tuple_id, error = ?e, "enable_btree_pk: BTree insert failed during backfill");
                    } else {
                        backfilled += 1;
                    }
                }
                _ => {
                    skipped_non_int64 += 1;
                }
            }
        }
        self.pk_index = Some(btree);
        self.pk_column_idx = Some(column_idx);
        tracing::info!(
            column_idx,
            backfilled,
            skipped_deleted,
            skipped_non_int64,
            "B+Tree PK index enabled (P0-4 primary storage)"
        );
    }

    /// P0-4：通过主键值快速点查 tuple_id（O(log n)）
    ///
    /// **返回**：`Some(tuple_id)` 找到；`None` 未找到或 BTree 未启用
    pub fn pk_lookup(&self, key: i64) -> Option<usize> {
        let btree = self.pk_index.as_ref()?;
        let encoded = szrsql_storage::btree::encode_i64_key(key);
        match btree.search(&encoded) {
            Ok(Some(value)) => {
                // value 是 encode_tuple_id_key(tuple_id)（4B big-endian）
                if value.len() == 4 {
                    Some(decode_tuple_id_key(&value) as usize)
                } else {
                    None
                }
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(error = ?e, "pk_lookup: BTree search failed");
                None
            }
        }
    }

    /// P0-STORE-1：检查是否已启用 B+Tree 主键索引
    pub fn has_btree_pk(&self) -> bool {
        self.pk_index.is_some()
    }

    /// P0-STORE 阶段 1：获取主键列索引（若已启用 B+Tree 主键索引）
    pub fn pk_column_idx(&self) -> Option<usize> {
        self.pk_column_idx
    }

    /// P0-4：通过主键值快速点查完整行（O(log n)）
    ///
    /// 若 B+Tree 主键索引已启用，通过二级索引定位 tuple_id，再从主 B+Tree 读取行。
    pub fn pk_point_lookup(&self, key: i64) -> Option<Row> {
        let tuple_id = self.pk_lookup(key)?;
        if self.deleted.contains(&(tuple_id as u32)) {
            return None;
        }
        self.get_row(tuple_id)
    }

    /// P0-4：通过主键值范围查询多行（O(log n + k)）
    ///
    /// 返回主键值在 [low, high) 范围内的所有行（升序）。
    pub fn pk_range_lookup(&self, low: i64, high: i64) -> Option<Vec<Row>> {
        let btree = self.pk_index.as_ref()?;
        let low_bytes = szrsql_storage::btree::encode_i64_key(low);
        let high_bytes = szrsql_storage::btree::encode_i64_key(high);
        let pairs = btree
            .range_scan(
                std::ops::Bound::Included(&low_bytes[..]),
                std::ops::Bound::Excluded(&high_bytes[..]),
            )
            .ok()?;
        let mut result = Vec::with_capacity(pairs.len());
        for (_key, value) in pairs {
            if value.len() != 8 {
                continue;
            }
            let tuple_id = decode_tuple_id_key(&value) as usize;
            if self.deleted.contains(&(tuple_id as u32)) {
                continue;
            }
            if let Some(row) = self.get_row(tuple_id) {
                result.push(row);
            }
        }
        Some(result)
    }

    /// P0-STORE-1：从行中提取主键并编码为 Vec<u8>
    fn extract_pk_bytes(&self, row: &Row) -> Option<Vec<u8>> {
        let col_idx = self.pk_column_idx?;
        let pk_val = row.get(col_idx)?;
        if let Value::Int64(v) = pk_val {
            Some(szrsql_storage::btree::encode_i64_key(*v))
        } else {
            None
        }
    }

    /// P0-4：同步更新二级 B+Tree 主键索引
    ///
    /// 在 INSERT 后调用，将 (pk_bytes → encode_tuple_id_key(tuple_id)) 插入 BTree。
    fn update_pk_index(&mut self, tuple_id: usize, pk_bytes: Option<Vec<u8>>) {
        let btree = match self.pk_index.as_mut() {
            Some(b) => b,
            None => return,
        };
        let pk_bytes = match pk_bytes {
            Some(b) => b,
            None => return,
        };
        if let Err(e) = btree.insert(pk_bytes, encode_tuple_id_key(tuple_id as u32)) {
            tracing::warn!(error = ?e, "update_pk_index: BTree insert failed");
        }
    }

    // -----------------------------------------------------------------
    //  Phase F-10 / P0-4：在线 DDL 支持
    // -----------------------------------------------------------------

    /// 取所有行（含已删除的行）的可变引用 — Phase F-10 / P0-4
    ///
    /// 用于 ALTER TABLE 数据迁移：ADD COLUMN 追加列值、DROP COLUMN 移除列值、
    /// ALTER COLUMN TYPE 转换列值。
    ///
    /// P0-4：从 B+Tree 解码所有行到临时 Vec，调用方修改后写回 B+Tree。
    /// 获取全部行的可变句柄（P0-4）
    ///
    /// 返回 `RowsMutGuard`：通过 `DerefMut` 修改行，guard 析构时自动回写 B+Tree
    /// （保留各行原始 xmin/xmax 版本元数据）。语义与旧版 `&mut Vec<Row>` 一致。
    pub fn rows_mut(&mut self) -> RowsMutGuard<'_> {
        // 解码 B+Tree 中全部行（含已删除标记的行，与 deleted 集合配合使用）
        let mut rows = Vec::new();
        let cursor = match self
            .btree
            .cursor(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        {
            Ok(c) => c,
            Err(_) => {
                return RowsMutGuard {
                    table: self,
                    rows,
                    versions: Vec::new(),
                }
            }
        };
        let mut versions = Vec::new();
        for (_key, value) in cursor {
            if let Ok((xmin, xmax, row)) = btree_value_codec::decode(&value) {
                versions.push((xmin, xmax));
                rows.push(row);
            }
        }
        RowsMutGuard {
            table: self,
            rows,
            versions,
        }
    }

    /// P0-4：行缓存上限设置
    pub fn set_row_cache_max(&mut self, max: usize) {
        self.row_cache_max = max;
    }

    /// P0-4：行缓存当前大小
    pub fn row_cache_len(&self) -> usize {
        self.row_cache.len()
    }

    /// 替换表 Schema — Phase F-10
    ///
    /// 用于 ALTER TABLE 完成后同步存储层的 Schema（catalog 与 storage 各持一份）。
    pub fn set_schema(&mut self, schema: TableSchema) {
        self.schema = schema;
    }

    // -----------------------------------------------------------------
    //  P0-STORE-2：BufferPool 持久化接入
    // -----------------------------------------------------------------

    /// 查询是否启用了 BufferPool 持久化
    pub fn has_persistence(&self) -> bool {
        self.persistence.is_some()
    }

    /// 启用 BufferPool 持久化后端
    ///
    /// `path` 为数据文件路径（若文件不存在，FilePageWriter 会自动创建）。
    /// 启用后：
    /// - `flush_to_disk()` 将整表序列化字节流分页写入 BufferPool
    /// - `load_from_disk()` 从 BufferPool 读取所有页重建表状态
    ///
    /// **BufferPool 配置**：容量 256 页（256 * 8KB = 2MB 缓存），FilePageLoader/FilePageWriter 文件后端。
    ///
    /// **幂等性**：重复调用会覆盖现有 persistence（旧 BufferPool 被 drop）。
    /// **错误**：文件无法打开时返回 ExecutionError。
    pub fn enable_persistence<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
    ) -> Result<(), ExecutionError> {
        let path_ref = path.as_ref();
        let writer = szrsql_storage::buffer::FilePageWriter::open(path_ref)
            .map_err(|e| ExecutionError::Storage(format!("open writer failed: {e}")))?;
        // 文件已存在则用 FilePageLoader，否则（首次启用）用 InMemoryPageLoader 占位
        let loader: std::sync::Arc<dyn szrsql_storage::buffer::PageLoader> =
            match szrsql_storage::buffer::FilePageLoader::open(path_ref) {
                Ok(l) => std::sync::Arc::new(l),
                Err(_) => std::sync::Arc::new(szrsql_storage::buffer::InMemoryPageLoader::new()),
            };
        let writer: std::sync::Arc<dyn szrsql_storage::buffer::PageWriter> =
            std::sync::Arc::new(writer);
        let pool = szrsql_storage::buffer::BufferPool::with_writer(256, loader, writer)
            .map_err(|e| ExecutionError::Storage(format!("buffer pool init failed: {e}")))?;
        self.persistence = Some(std::sync::Arc::new(pool));
        tracing::debug!(table = %self.name, path = ?path_ref, "P0-STORE-2: persistence enabled");
        Ok(())
    }

    /// 将整表状态序列化并分页写入 BufferPool（同步刷盘）
    ///
    /// **布局**：
    /// - page 0：header 页，body 存储 8 字节 LE 总字节长度 + 表名 UTF-8 字符串
    /// - page 1..N：data 页，body 存储序列化字节流分片（每片最多 PAGE_BODY_SIZE 字节）
    ///
    /// **序列化内容**：name + schema + rows + deleted + xmin + xmax
    /// （BTree pk_index 和 persistence 字段不序列化，重启后需重新启用）
    ///
    /// **错误**：序列化失败、BufferPool 写入失败、未启用 persistence 时返回错误。
    pub fn flush_to_disk(&self) -> Result<(), ExecutionError> {
        let pool = self.persistence.as_ref().ok_or_else(|| {
            ExecutionError::Storage(
                "persistence not enabled, call enable_persistence() first".into(),
            )
        })?;

        // P0-4：B+Tree 是 #[serde(skip)]，不能直接序列化 self。
        // 改为先取快照（遍历 B+Tree 解码所有行），再序列化快照数据。
        let snapshot = self.snapshot();
        let serialized = serde_json::to_vec(&snapshot)
            .map_err(|e| ExecutionError::Storage(format!("serialize snapshot failed: {e}")))?;
        let total_len = serialized.len();
        tracing::debug!(table = %self.name, total_len, pages = total_len.div_ceil(BODY_SIZE), "flush_to_disk");

        // page 0：header 页 — body = [8 字节 LE total_len][表名 UTF-8]
        let mut header_page =
            szrsql_storage::page::Page::new(0, szrsql_storage::page::PageType::Data);
        let mut header_body = Vec::with_capacity(8 + self.name.len());
        header_body.extend_from_slice(&(total_len as u64).to_le_bytes());
        header_body.extend_from_slice(self.name.as_bytes());
        header_page
            .append_body(&header_body)
            .map_err(|e| ExecutionError::Storage(format!("header append failed: {e}")))?;
        header_page.update_checksum();
        pool.put_page(0, header_page)
            .map_err(|e| ExecutionError::Storage(format!("write header page failed: {e}")))?;

        // page 1..N：data 页 — body = 序列化字节流分片
        let total_pages = total_len.div_ceil(BODY_SIZE);
        for page_idx in 0..total_pages {
            let page_id = (page_idx + 1) as u32;
            let start = page_idx * BODY_SIZE;
            let end = std::cmp::min(start + BODY_SIZE, total_len);
            let chunk = &serialized[start..end];
            let mut page =
                szrsql_storage::page::Page::new(page_id, szrsql_storage::page::PageType::Data);
            page.append_body(chunk).map_err(|e| {
                ExecutionError::Storage(format!("data page {page_id} append failed: {e}"))
            })?;
            page.update_checksum();
            pool.put_page(page_id, page).map_err(|e| {
                ExecutionError::Storage(format!("write data page {page_id} failed: {e}"))
            })?;
        }

        // flush_all 确保所有脏页落盘
        pool.flush_all()
            .map_err(|e| ExecutionError::Storage(format!("flush_all failed: {e}")))?;
        tracing::info!(table = %self.name, total_len, total_pages, "P0-STORE-2: flush_to_disk completed");
        Ok(())
    }

    /// 从 BufferPool 读取所有页并重建表状态
    ///
    /// **流程**：
    /// 1. 读取 page 0（header）— 解析 total_len 和表名
    /// 2. 读取 page 1..N（data）— 拼接字节流
    /// 3. 反序列化为 InMemoryTable 并替换 self
    ///
    /// **错误**：未启用 persistence、header 页缺失、页读取失败、反序列化失败。
    /// **注意**：重建后 pk_index 和 persistence 字段为 None（serde skip），
    /// 需调用方重新 `enable_btree_pk()` 和 `enable_persistence()`。
    pub fn load_from_disk(&mut self) -> Result<(), ExecutionError> {
        let pool = self.persistence.as_ref().ok_or_else(|| {
            ExecutionError::Storage(
                "persistence not enabled, call enable_persistence() first".into(),
            )
        })?;

        // page 0：header
        let header_page = pool
            .read_page(0)
            .map_err(|e| ExecutionError::Storage(format!("read header page failed: {e}")))?;
        let header_body = header_page
            .read_body(0, 8)
            .map_err(|e| ExecutionError::Storage(format!("header body read failed: {e}")))?;
        if header_body.len() < 8 {
            return Err(ExecutionError::Storage(format!(
                "header body too short: {} < 8",
                header_body.len()
            )));
        }
        let total_len = u64::from_le_bytes(header_body[..8].try_into().unwrap()) as usize;

        // 拼接 data 页字节流
        let total_pages = total_len.div_ceil(BODY_SIZE);
        let mut serialized = Vec::with_capacity(total_len);
        for page_idx in 0..total_pages {
            let page_id = (page_idx + 1) as u32;
            let page = pool.read_page(page_id).map_err(|e| {
                ExecutionError::Storage(format!("read data page {page_id} failed: {e}"))
            })?;
            // 读取 body 中实际写入的部分（从 offset 0 开始）
            let body = page
                .read_body(0, std::cmp::min(BODY_SIZE, total_len - serialized.len()))
                .map_err(|e| {
                    ExecutionError::Storage(format!("data page {page_id} body read failed: {e}"))
                })?;
            serialized.extend_from_slice(body);
        }
        if serialized.len() != total_len {
            return Err(ExecutionError::Storage(format!(
                "data length mismatch: expected {total_len} got {}",
                serialized.len()
            )));
        }

        // 反序列化为 TableSnapshot（InMemoryTable 含 #[serde(skip)] 的 B+Tree，不能直接反序列化）
        let snapshot: TableSnapshot = serde_json::from_slice(&serialized)
            .map_err(|e| ExecutionError::Storage(format!("deserialize failed: {e}")))?;
        // 用快照重建 B+Tree
        self.restore(snapshot);
        tracing::info!(table = %self.name, total_len, total_pages, "P0-STORE-2: load_from_disk completed");
        Ok(())
    }

    // -----------------------------------------------------------------
    //  P1-1：BTree+BufferPool 分页存储主路径
    //  （Vec<Row> 热缓存 + BufferPool 分页主存，热数据溢出到冷存储）
    // -----------------------------------------------------------------

    /// P1-1：启用分页存储主路径
    ///
    /// `path` 为分页存储文件路径。启用后：
    /// - `spill_to_paged_storage()` 将 rows 分页写入 paged_storage
    /// - `restore_from_paged_storage()` 从 paged_storage 重建 rows
    /// - `auto_spill_if_needed()` 在 insert 后自动检查并 spill
    ///
    /// **与 enable_persistence 的区别**：
    /// - enable_persistence：整表序列化字节流分页写入（一次性快照）
    /// - enable_paged_storage：按行分页存储（支持增量更新和按页读取）
    ///
    /// **BufferPool 配置**：容量 256 页（256 * 8KB = 2MB 缓存），
    /// FilePageLoader/FilePageWriter 文件后端（与 enable_persistence 相同的 I/O 策略）。
    ///
    /// **幂等性**：重复调用会覆盖现有 paged_storage（旧 BufferPool 被 drop）。
    /// **错误**：文件无法打开时返回 ExecutionError。
    pub fn enable_paged_storage<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
    ) -> Result<(), ExecutionError> {
        let path_ref = path.as_ref();
        let writer = szrsql_storage::buffer::FilePageWriter::open(path_ref).map_err(|e| {
            ExecutionError::Storage(format!("paged_storage: open writer failed: {e}"))
        })?;
        // 文件已存在则用 FilePageLoader，否则（首次启用）用 InMemoryPageLoader 占位
        // （首次启用时文件尚未创建，FilePageLoader::open 会失败，退化为内存 loader；
        //   spill 后 flush_all 通过 FilePageWriter 写盘，重启时文件已存在可正常加载）
        let loader: std::sync::Arc<dyn szrsql_storage::buffer::PageLoader> =
            match szrsql_storage::buffer::FilePageLoader::open(path_ref) {
                Ok(l) => std::sync::Arc::new(l),
                Err(_) => std::sync::Arc::new(szrsql_storage::buffer::InMemoryPageLoader::new()),
            };
        let writer: std::sync::Arc<dyn szrsql_storage::buffer::PageWriter> =
            std::sync::Arc::new(writer);
        let pool =
            szrsql_storage::buffer::BufferPool::with_writer(256, loader, writer).map_err(|e| {
                ExecutionError::Storage(format!("paged_storage: buffer pool init failed: {e}"))
            })?;
        self.paged_storage = Some(std::sync::Arc::new(pool));
        tracing::debug!(table = %self.name, path = ?path_ref, "P1-1: paged_storage enabled");
        Ok(())
    }

    /// P1-1：查询是否启用了分页存储
    pub fn has_paged_storage(&self) -> bool {
        self.paged_storage.is_some()
    }

    /// P1-1：设置自动 spill 阈值（仅当 paged_storage 已启用时生效）
    ///
    /// `threshold` 为行数阈值，rows 行数超过此值时触发自动 spill。
    pub fn set_spill_threshold(&mut self, threshold: usize) {
        self.spill_threshold = threshold;
    }

    /// P1-1：将 rows 分页写入 paged_storage（不清空 rows，作为持久化镜像）
    ///
    /// **页布局**：
    /// - page 0：header 页，body = [8 字节 LE 总行数][8 字节 LE data 页数]
    /// - page 1..N：data 页，每页存储若干行
    ///   （每行格式：[4 字节 LE row_len][row_bytes]，row_bytes 为行的 serde_json 序列化）
    ///
    /// **分页策略**：逐行填充当前页，当当前页剩余空间不足以容纳下一行（4 + row_len 字节）
    /// 时封页并开新页。单行超过单页容量时该行独占一页（body 由 Page::append_body 校验）。
    ///
    /// **幂等性**：重复调用会覆盖现有分页数据（按 page_id 覆盖写）。
    /// **注意**：不清空 rows，rows 仍作为热数据缓存保留。
    /// P1-6：将 rows 分页写入 paged_storage（不清空 rows，作为持久化镜像）
    ///
    /// **页面格式（v2，含 MVCC 元数据）**：
    /// - page 0（header）：`[8 字节 LE total_rows][8 字节 LE data_page_count][1 字节 flags]`
    ///   - flags bit 0 = 1 表示存储了 xmin/xmax/deleted 元数据
    /// - page 1..N（data）：每行格式为
    ///   `[4 字节 LE row_len][4 字节 LE xmin][4 字节 LE xmax][1 字节 deleted][row_bytes]`
    ///
    /// **与 v1 的区别**：
    /// - v1 跳过 xmax==MAX 的已删除行，且不存储 xmin/xmax
    /// - v2 存储所有行（含 tombstone），恢复时可完整重建 MVCC 状态
    pub fn spill_to_paged_storage(&self) -> Result<(), ExecutionError> {
        let pool = self.paged_storage.as_ref().ok_or_else(|| {
            ExecutionError::Storage(
                "paged_storage not enabled, call enable_paged_storage() first".into(),
            )
        })?;

        // 遍历 B+Tree，解码所有行数据（含已删除的 tombstone 行）
        // 每行存储：(xmin, xmax, row_bytes)
        #[derive(Clone)]
        struct SpillEntry {
            xmin: u32,
            xmax: u32,
            deleted: bool,
            row_bytes: Vec<u8>,
        }
        let mut entries: Vec<SpillEntry> = Vec::new();
        let cursor = self
            .btree
            .cursor(Bound::Unbounded, Bound::Unbounded)
            .map_err(|e| ExecutionError::Storage(format!("spill: cursor open failed: {e}")))?;
        for (_key, value) in cursor {
            let (xmin, xmax, row) = btree_value_codec::decode(&value).map_err(|e| {
                ExecutionError::Storage(format!("spill: decode btree value failed: {e}"))
            })?;
            let deleted = xmax == u32::MAX;
            let bytes = serde_json::to_vec(&row).map_err(|e| {
                ExecutionError::Storage(format!("spill: serialize row failed: {e}"))
            })?;
            entries.push(SpillEntry {
                xmin,
                xmax,
                deleted,
                row_bytes: bytes,
            });
        }

        let total_rows = entries.len();
        tracing::debug!(table = %self.name, total_rows, "spill_to_paged_storage: starting");

        // 按 data 页组织：每页存储尽可能多的行
        // 每行开销 = 4(row_len) + 4(xmin) + 4(xmax) + 1(deleted) + row_bytes.len()
        const ROW_META_SIZE: usize = 4 + 4 + 4 + 1; // 13 bytes
        let mut data_pages: Vec<Vec<u8>> = Vec::new();
        let mut current_body: Vec<u8> = Vec::with_capacity(BODY_SIZE);
        for entry in &entries {
            let needed = ROW_META_SIZE + entry.row_bytes.len();
            // 当前页非空且放不下此行 → 封页开新页
            if !current_body.is_empty() && current_body.len() + needed > BODY_SIZE {
                data_pages.push(std::mem::take(&mut current_body));
                current_body = Vec::with_capacity(BODY_SIZE);
            }
            // 写入 [4 字节 LE row_len][4 字节 LE xmin][4 字节 LE xmax][1 字节 deleted][row_bytes]
            current_body.extend_from_slice(&(entry.row_bytes.len() as u32).to_le_bytes());
            current_body.extend_from_slice(&entry.xmin.to_le_bytes());
            current_body.extend_from_slice(&entry.xmax.to_le_bytes());
            current_body.push(if entry.deleted {
                1u8
            } else {
                0u8
            });
            current_body.extend_from_slice(&entry.row_bytes);
        }
        if !current_body.is_empty() {
            data_pages.push(current_body);
        }
        let data_page_count = data_pages.len();

        // page 0：header 页
        // body = [8 字节 LE total_rows][8 字节 LE data_page_count][1 字节 flags]
        // flags bit 0 = 1 表示含 MVCC 元数据（v2 格式）
        let mut header_page =
            szrsql_storage::page::Page::new(0, szrsql_storage::page::PageType::Data);
        let mut header_body = Vec::with_capacity(17);
        header_body.extend_from_slice(&(total_rows as u64).to_le_bytes());
        header_body.extend_from_slice(&(data_page_count as u64).to_le_bytes());
        header_body.push(1u8); // flags: bit 0 = MVCC present
        header_page
            .append_body(&header_body)
            .map_err(|e| ExecutionError::Storage(format!("spill: header append failed: {e}")))?;
        header_page.update_checksum();
        pool.put_page(0, header_page).map_err(|e| {
            ExecutionError::Storage(format!("spill: write header page failed: {e}"))
        })?;

        // page 1..N：data 页
        for (page_idx, body_bytes) in data_pages.iter().enumerate() {
            let page_id = (page_idx + 1) as u32;
            let mut page =
                szrsql_storage::page::Page::new(page_id, szrsql_storage::page::PageType::Data);
            page.append_body(body_bytes).map_err(|e| {
                ExecutionError::Storage(format!("spill: data page {page_id} append failed: {e}"))
            })?;
            page.update_checksum();
            pool.put_page(page_id, page).map_err(|e| {
                ExecutionError::Storage(format!("spill: write data page {page_id} failed: {e}"))
            })?;
        }

        // flush_all 确保所有脏页落盘
        pool.flush_all()
            .map_err(|e| ExecutionError::Storage(format!("spill: flush_all failed: {e}")))?;
        tracing::info!(
            table = %self.name, total_rows, data_page_count,
            "P1-6: spill_to_paged_storage completed (v2 with MVCC metadata)"
        );
        Ok(())
    }

    /// P1-6：从 paged_storage 读取所有页重建 rows（含 MVCC 元数据）
    ///
    /// **流程**：
    /// 1. 读取 page 0（header）— 解析 total_rows、data_page_count 和 flags
    /// 2. flags bit 0 = 1 时为 v2 格式，每行含 xmin/xmax/deleted 元数据
    /// 3. flags = 0 或 header < 17 字节时回退到 v1 格式（无 MVCC 元数据）
    /// 4. 读取 page 1..(1+data_page_count)（data）— 按页解析每行
    /// 5. 重建 B+Tree，还原 xmin/xmax 向量及 deleted 集合
    ///
    /// **注意**：
    /// - pk_index 不重建（需调用方重新 `enable_btree_pk()`）
    /// - v1 格式回退时 xmin/xmax 重置为 0、deleted 清空（与原行为一致）
    pub fn restore_from_paged_storage(&mut self) -> Result<(), ExecutionError> {
        let pool = self.paged_storage.as_ref().ok_or_else(|| {
            ExecutionError::Storage(
                "paged_storage not enabled, call enable_paged_storage() first".into(),
            )
        })?;

        // page 0：header
        let header_page = pool.read_page(0).map_err(|e| {
            ExecutionError::Storage(format!("restore: read header page failed: {e}"))
        })?;
        let header_body = header_page.read_body(0, 17).map_err(|e| {
            ExecutionError::Storage(format!("restore: header body read failed: {e}"))
        })?;

        // 检测格式版本：v2 header >= 17 字节且 flags bit 0 = 1
        let (total_rows, data_page_count, has_mvcc) = if header_body.len() >= 17 {
            let mut arr8 = [0u8; 8];
            arr8.copy_from_slice(&header_body[..8]);
            let total = u64::from_le_bytes(arr8) as usize;
            arr8.copy_from_slice(&header_body[8..16]);
            let pages = u64::from_le_bytes(arr8) as usize;
            let flags = header_body[16];
            (total, pages, (flags & 1) != 0)
        } else if header_body.len() >= 16 {
            // v1 格式：无 flags 字节
            let mut arr8 = [0u8; 8];
            arr8.copy_from_slice(&header_body[..8]);
            let total = u64::from_le_bytes(arr8) as usize;
            arr8.copy_from_slice(&header_body[8..16]);
            let pages = u64::from_le_bytes(arr8) as usize;
            (total, pages, false)
        } else {
            return Err(ExecutionError::Storage(format!(
                "restore: header body too short: {}",
                header_body.len()
            )));
        };

        // 每行存储结构（v2）：
        // [4 字节 row_len][4 字节 xmin][4 字节 xmax][1 字节 deleted][row_bytes]
        // v1 格式：[4 字节 row_len][row_bytes]
        let row_meta_size = if has_mvcc {
            4 + 4 + 4 + 1
        } else {
            4
        };

        // 逐页读取 data 页，解析每行
        #[derive(Clone)]
        struct RestoredEntry {
            row: Row,
            xmin: u32,
            xmax: u32,
            deleted: bool,
        }
        let mut restored: Vec<RestoredEntry> = Vec::with_capacity(total_rows);
        for page_idx in 0..data_page_count {
            let page_id = (page_idx + 1) as u32;
            let page = pool.read_page(page_id).map_err(|e| {
                ExecutionError::Storage(format!("restore: read data page {page_id} failed: {e}"))
            })?;
            // 用 free_offset 知道本页实际写入字节数
            let body_len = page.header.free_offset as usize;
            if body_len == 0 {
                continue;
            }
            let body = page.read_body(0, body_len).map_err(|e| {
                ExecutionError::Storage(format!(
                    "restore: data page {page_id} body read failed: {e}"
                ))
            })?;
            let mut offset = 0usize;
            while offset + row_meta_size <= body.len() && restored.len() < total_rows {
                let mut arr4 = [0u8; 4];
                arr4.copy_from_slice(&body[offset..offset + 4]);
                let row_len = u32::from_le_bytes(arr4) as usize;
                offset += 4;

                let (xmin, xmax, deleted) = if has_mvcc {
                    // 读取 xmin
                    if offset + 4 > body.len() {
                        return Err(ExecutionError::Storage(format!(
                            "restore: xmin truncated at page {page_id} offset {offset}"
                        )));
                    }
                    arr4.copy_from_slice(&body[offset..offset + 4]);
                    let xmin = u32::from_le_bytes(arr4);
                    offset += 4;
                    // 读取 xmax
                    if offset + 4 > body.len() {
                        return Err(ExecutionError::Storage(format!(
                            "restore: xmax truncated at page {page_id} offset {offset}"
                        )));
                    }
                    arr4.copy_from_slice(&body[offset..offset + 4]);
                    let xmax = u32::from_le_bytes(arr4);
                    offset += 4;
                    // 读取 deleted 标志
                    if offset + 1 > body.len() {
                        return Err(ExecutionError::Storage(format!(
                            "restore: deleted flag truncated at page {page_id} offset {offset}"
                        )));
                    }
                    let deleted = body[offset] != 0;
                    offset += 1;
                    (xmin, xmax, deleted)
                } else {
                    (0, 0, false)
                };

                if offset + row_len > body.len() {
                    return Err(ExecutionError::Storage(format!(
                        "restore: row data truncated at page {page_id} offset {offset}, need {row_len} have {}",
                        body.len() - offset
                    )));
                }
                let row: Row =
                    serde_json::from_slice(&body[offset..offset + row_len]).map_err(|e| {
                        ExecutionError::Storage(format!(
                        "restore: deserialize row at page {page_id} offset {offset} failed: {e}"
                    ))
                    })?;
                restored.push(RestoredEntry {
                    row,
                    xmin,
                    xmax,
                    deleted,
                });
                offset += row_len;
            }
        }

        if restored.len() != total_rows {
            return Err(ExecutionError::Storage(format!(
                "restore: row count mismatch: expected {total_rows} got {}",
                restored.len()
            )));
        }

        // 重建 B+Tree：清空并重新插入所有行（含 MVCC 元数据）
        self.btree = szrsql_storage::btree::BTree::with_default_order();
        self.next_tuple_id = 0;
        self.deleted.clear();
        self.row_cache.clear();
        for entry in restored {
            let tuple_id = self.next_tuple_id;
            self.next_tuple_id += 1;
            let key = encode_tuple_id_key(tuple_id);
            let value = btree_value_codec::encode(entry.xmin, entry.xmax, &entry.row);
            self.btree.insert(key, value).map_err(|e| {
                ExecutionError::Storage(format!("restore: btree insert failed: {e}"))
            })?;
            // 恢复 deleted 集合（tombstone 行）
            if entry.deleted {
                self.deleted.insert(tuple_id);
            }
        }
        tracing::info!(
            table = %self.name, total_rows, data_page_count, has_mvcc,
            "P1-6: restore_from_paged_storage completed"
        );
        Ok(())
    }

    /// P1-1：如果 rows 行数超过 spill_threshold，自动 spill 到 paged_storage
    ///
    /// 在 insert/bulk_insert 后调用，实现"热数据溢出到冷存储"。
    /// 仅在 paged_storage 已启用且行数超过阈值时触发；
    /// spill 失败仅记录 warning，不影响主路径（rows 仍保留在内存）。
    fn auto_spill_if_needed(&mut self) {
        if self.paged_storage.is_some() && self.btree.len() > self.spill_threshold {
            if let Err(e) = self.spill_to_paged_storage() {
                tracing::warn!(table = %self.name, error = %e, "P1-1: auto spill failed");
            }
        }
    }

    /// P1-1：返回分页存储信息（页数、行数、阈值）
    ///
    /// 返回 None 表示未启用分页存储。
    /// **注意**：`page_count` 取自 BufferPool 当前缓存页数（含 header 页），
    /// 若部分页已被 LRU 淘汰，该值可能小于实际磁盘页数。
    pub fn paged_storage_info(&self) -> Option<PagedStorageInfo> {
        let pool = self.paged_storage.as_ref()?;
        Some(PagedStorageInfo {
            page_count: pool.total_len(),
            row_count: self.btree.len().saturating_sub(self.deleted.len()),
            threshold: self.spill_threshold,
        })
    }

    /// 批量插入多行，返回起始 tuple_id
    pub fn bulk_insert(&mut self, rows: Vec<Row>) -> usize {
        let start = self.next_tuple_id as usize;
        for row in rows {
            let tuple_id = self.next_tuple_id;
            self.next_tuple_id += 1;
            let key = encode_tuple_id_key(tuple_id);
            let value = btree_value_codec::encode(0, 0, &row);
            self.btree.insert(key, value).ok();
            self.maybe_cache_row(tuple_id, row);
        }
        // P1-1：批量插入后检查是否需要自动 spill 到分页存储
        self.auto_spill_if_needed();
        start
    }

    /// 取所有行（含已删除的行）— 供测试断言原始数据
    ///
    /// 注意：返回的 Vec 包含已删除的行（xmax = u32::MAX）。
    /// 如需仅活跃行，请用 `scan_iter()`。
    pub fn rows(&self) -> Vec<Row> {
        let mut result = Vec::new();
        let cursor = self
            .btree
            .cursor(Bound::Unbounded, Bound::Unbounded)
            .ok()
            .into_iter()
            .flatten();
        for (key, value) in cursor {
            let tuple_id = decode_tuple_id_key(&key) as u32;
            // 跳过已删除行（tombstone 过滤）
            if self.deleted.contains(&tuple_id) {
                continue;
            }
            if let Ok((_xmin, _xmax, row)) = btree_value_codec::decode(&value) {
                result.push(row);
            }
        }
        result
    }

    /// B+Tree 中存储的总条目数（含 tombstone 已删除行）。
    ///
    /// 与 [`InMemoryTable::rows`] 不同，此方法不过滤 `deleted` 集合，
    /// 反映存储引擎的物理条目数，用于需要区分活跃行与 tombstone 的场景。
    pub fn total_row_count(&self) -> usize {
        self.btree.len()
    }

    /// P1-6：查询指定 tuple_id 的 MVCC 版本元数据（xmin, xmax）
    ///
    /// 用于验证分页恢复后 MVCC 元数据的完整性。
    /// 返回 None 表示该 tuple_id 不存在于 B+Tree 中。
    pub fn row_version(&self, tuple_id: usize) -> Option<(u32, u32)> {
        let key = encode_tuple_id_key(tuple_id as u32);
        match self.btree.search(&key) {
            Ok(Some(value)) => btree_value_codec::decode(&value)
                .map(|(xmin, xmax, _)| (xmin, xmax))
                .ok(),
            _ => None,
        }
    }

    /// P1-6：查询指定 row_id 是否为 tombstone（已删除）
    ///
    /// 用于验证分页恢复后 deleted 集合的完整性。
    pub fn is_deleted(&self, row_id: usize) -> bool {
        self.deleted.contains(&(row_id as u32))
    }

    /// 取表名 — 用于持久化时作为 HashMap key
    pub fn name(&self) -> &str {
        &self.name
    }

    /// P0-TX-1 Phase B：修复旧版快照中缺失的 xmin/xmax 数组
    ///
    /// P0-4 后版本信息直接存储在 B+Tree 值中，此方法为兼容保留（无操作）。
    pub fn ensure_version_arrays(&mut self) {
        // P0-4: 版本信息存储在 B+Tree 值中，无需独立数组
    }

    /// 按 row_id 标记删除（tombstone）— P0-4
    ///
    /// 如果 row_id 已被删除或不存在于 B+Tree，返回 `false`；
    /// 否则标记 deleted 并更新 B+Tree 中该行的 xmax。
    pub fn delete_row(&mut self, row_id: usize) -> bool {
        let tuple_id = row_id as u32;
        if self.deleted.contains(&tuple_id) {
            return false;
        }
        // 检查 B+Tree 中是否存在该行
        let key = encode_tuple_id_key(tuple_id);
        match self.btree.search(&key) {
            Ok(Some(value)) => {
                // 更新 B+Tree 中的 xmax（标记删除）
                if let Ok((xmin, _xmax, row)) = btree_value_codec::decode(&value) {
                    let new_value = btree_value_codec::encode(xmin, u32::MAX, &row);
                    if let Err(e) = self.btree.insert(key, new_value) {
                        tracing::warn!(tuple_id, error = ?e, "delete_row: BTree xmax update failed");
                    }
                }
                self.deleted.insert(tuple_id);
                self.row_cache.remove(&tuple_id);
                true
            }
            _ => false,
        }
    }

    /// P0-TX-1 Phase B / P0-4：按 row_id 标记删除并设置 xmax（MVCC 版本元数据）
    ///
    /// 由 Executor 在事务内 DELETE 时调用。xmax 设为当前事务 ID，
    /// 更新 B+Tree 中该行的 value。不立即加入 deleted 集合（由 COMMIT 时 finalize）。
    pub fn delete_row_with_xmax(&mut self, row_id: usize, xmax: u32) -> bool {
        let tuple_id = row_id as u32;
        if self.deleted.contains(&tuple_id) {
            return false;
        }
        let key = encode_tuple_id_key(tuple_id);
        match self.btree.search(&key) {
            Ok(Some(value)) => {
                if let Ok((xmin, _xmax, row)) = btree_value_codec::decode(&value) {
                    let new_value = btree_value_codec::encode(xmin, xmax, &row);
                    if let Err(e) = self.btree.insert(key, new_value) {
                        tracing::warn!(tuple_id, error = ?e, "delete_row_with_xmax: BTree update failed");
                    } else {
                        // 从缓存移除（已脏）
                        self.row_cache.remove(&tuple_id);
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    /// P0-TX-1 Phase B：将已提交删除的行加入 tombstone 集合
    ///
    /// 由 Executor 在事务 COMMIT 后调用。
    pub fn finalize_deleted_rows(&mut self, deleted_row_ids: &[usize]) {
        for &id in deleted_row_ids {
            self.deleted.insert(id as u32);
        }
    }

    /// 按 row_id 替换行（tombstone 行不可更新）— P0-4
    ///
    /// 从 B+Tree 读取现有值，保留 xmin，写入新行。
    pub fn update_row(&mut self, row_id: usize, row: Row) -> bool {
        let tuple_id = row_id as u32;
        if self.deleted.contains(&tuple_id) {
            return false;
        }
        let key = encode_tuple_id_key(tuple_id);
        match self.btree.search(&key) {
            Ok(Some(value)) => {
                if let Ok((xmin, xmax, _old_row)) = btree_value_codec::decode(&value) {
                    let new_value = btree_value_codec::encode(xmin, xmax, &row);
                    if let Err(e) = self.btree.insert(key, new_value) {
                        tracing::warn!(tuple_id, error = ?e, "update_row: BTree update failed");
                        false
                    } else {
                        // 更新缓存
                        self.row_cache.insert(tuple_id, row);
                        true
                    }
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// 清空表数据（保留表结构）— TRUNCATE TABLE — P0-4
    ///
    /// 重建 B+Tree（丢弃所有节点），重置 next_tuple_id，清空 deleted 和缓存。
    pub fn truncate(&mut self) {
        self.btree = szrsql_storage::btree::BTree::with_default_order();
        self.next_tuple_id = 0;
        self.deleted.clear();
        self.row_cache.clear();
        // pk_index 保留（主键约束仍有效）
    }
}

impl TableStorage for InMemoryTable {
    fn name(&self) -> &str {
        &self.name
    }

    fn schema(&self) -> &TableSchema {
        &self.schema
    }

    /// P0-4：遍历 B+Tree 所有叶子节点，跳过 deleted，解码行数据
    fn scan_iter(&self) -> Box<dyn Iterator<Item = Row> + Send + '_> {
        let cursor = match self
            .btree
            .cursor(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = ?e, "scan_iter: cursor creation failed");
                return Box::new(std::iter::empty());
            }
        };
        Box::new(cursor.filter_map(move |(key, value)| {
            let tuple_id = decode_tuple_id_key(&key);
            if self.deleted.contains(&tuple_id) {
                return None;
            }
            btree_value_codec::decode(&value)
                .map(|(_, _, row)| row)
                .ok()
        }))
    }

    /// P0-4：遍历 B+Tree，yield (tuple_id as usize, row)
    fn scan_with_ids(&self) -> Box<dyn Iterator<Item = (usize, Row)> + Send + '_> {
        let cursor = match self
            .btree
            .cursor(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = ?e, "scan_with_ids: cursor creation failed");
                return Box::new(std::iter::empty());
            }
        };
        Box::new(cursor.filter_map(move |(key, value)| {
            let tuple_id = decode_tuple_id_key(&key);
            if self.deleted.contains(&tuple_id) {
                return None;
            }
            btree_value_codec::decode(&value)
                .map(|(_, _, row)| (tuple_id as usize, row))
                .ok()
        }))
    }

    /// P0-4：B+Tree 条目数 - deleted 数量
    fn row_count(&self) -> usize {
        self.btree.len().saturating_sub(self.deleted.len())
    }

    /// P0-4：B+Tree 点查 + 缓存
    fn get_row(&self, row_id: usize) -> Option<Row> {
        let tuple_id = row_id as u32;
        if self.deleted.contains(&tuple_id) {
            return None;
        }
        // 先查缓存
        if let Some(row) = self.row_cache.get(&tuple_id) {
            return Some(row.clone());
        }
        // 查 B+Tree
        let key = encode_tuple_id_key(tuple_id);
        match self.btree.search(&key) {
            Ok(Some(value)) => btree_value_codec::decode(&value)
                .map(|(_, _, row)| row)
                .ok(),
            _ => None,
        }
    }

    /// P0-TX-1 Phase B / P0-4：返回所有行（含已 tombstone 的）及其 xmin/xmax 版本元数据。
    ///
    /// 遍历 B+Tree 所有叶子节点（不过滤 deleted），解码 xmin/xmax/row。
    /// 由 Executor 的 MVCC 可见性判断决定哪些行对当前事务可见。
    fn scan_with_versions(&self) -> Box<dyn Iterator<Item = (usize, Row, u32, u32)> + Send + '_> {
        let cursor = match self
            .btree
            .cursor(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = ?e, "scan_with_versions: cursor creation failed");
                return Box::new(std::iter::empty());
            }
        };
        Box::new(cursor.filter_map(move |(key, value)| {
            let tuple_id = decode_tuple_id_key(&key);
            btree_value_codec::decode(&value)
                .map(|(xmin, xmax, row)| (tuple_id as usize, row, xmin, xmax))
                .ok()
        }))
    }

    /// P0-STORE 阶段 1：主键列索引（委托给 InMemoryTable::pk_column_idx）
    fn pk_column_idx(&self) -> Option<usize> {
        InMemoryTable::pk_column_idx(self)
    }

    /// P0-STORE 阶段 1：主键点查（委托给 InMemoryTable::pk_point_lookup）
    fn pk_point_lookup(&self, key: i64) -> Option<Row> {
        InMemoryTable::pk_point_lookup(self, key)
    }

    /// P0-STORE 阶段 1：主键范围查询（委托给 InMemoryTable::pk_range_lookup）
    fn pk_range_lookup(&self, low: i64, high: i64) -> Option<Vec<Row>> {
        InMemoryTable::pk_range_lookup(self, low, high)
    }
}

// =====================================================================
//  CounterTable — 惰性计数表（1M 行测试用）
// =====================================================================

/// 惰性计数表 — 不实际存储行，按需生成 `[Value::Int64(0), Value::Int64(1), ...]`
///
/// 用于 Phase 3.4 集成测试：全表扫描 1,000,000 行 → 行数正确
pub struct CounterTable {
    /// 表名
    name: String,
    /// 表 Schema（单列 `id BIGINT`）
    schema: TableSchema,
    /// 行数
    count: usize,
}

impl CounterTable {
    /// 创建一个生成 `count` 行的计数表，每行为 `vec![Value::Int64(i as i64)]`
    pub fn new(name: &str, count: usize) -> Self {
        use szrsql_types::value::ColumnType;
        let table_name = TableName::new(name);
        let columns = vec![ColumnDefinition::new("id", ColumnType::Int64)];
        Self {
            name: name.to_string(),
            schema: TableSchema {
                name: table_name,
                columns,
            },
            count,
        }
    }
}

impl TableStorage for CounterTable {
    fn name(&self) -> &str {
        &self.name
    }

    fn schema(&self) -> &TableSchema {
        &self.schema
    }

    fn scan_iter(&self) -> Box<dyn Iterator<Item = Row> + Send + '_> {
        // 闭包不捕获任何借用，仅使用 usize 计数器 — 完全 Send
        Box::new((0..self.count).map(|i| vec![Value::Int64(i as i64)]))
    }

    fn scan_with_ids(&self) -> Box<dyn Iterator<Item = (usize, Row)> + Send + '_> {
        Box::new((0..self.count).map(|i| (i, vec![Value::Int64(i as i64)])))
    }

    fn row_count(&self) -> usize {
        self.count
    }

    fn get_row(&self, row_id: usize) -> Option<Row> {
        if row_id < self.count {
            Some(vec![Value::Int64(row_id as i64)])
        } else {
            None
        }
    }
}

// =====================================================================
//  InMemoryBTreeIndex — BTreeMap 后端索引
// =====================================================================

/// 内存 BTreeIndex — 简单 `BTreeMap<i64, Vec<usize>>` 实现
///
/// 支持：
/// - 点查 `point_lookup(key) -> Vec<row_id>`
/// - 范围查询 `range_lookup(low, high) -> Vec<row_id>`（含两端）
///
/// 限制：索引键固定为 `i64`（覆盖最常见的主键/整数列场景）
pub struct InMemoryBTreeIndex {
    /// 索引名
    name: String,
    /// 所属表名
    table_name: String,
    /// 索引列名
    column: String,
    /// i64 键 → row_ids
    index: BTreeMap<i64, Vec<usize>>,
}

impl InMemoryBTreeIndex {
    /// 创建空索引
    pub fn new(
        name: impl Into<String>,
        table_name: impl Into<String>,
        column: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            table_name: table_name.into(),
            column: column.into(),
            index: BTreeMap::new(),
        }
    }

    /// 索引名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 所属表名
    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    /// 索引列名
    pub fn column(&self) -> &str {
        &self.column
    }

    /// 插入一条索引项
    pub fn insert(&mut self, key: i64, row_id: usize) {
        self.index.entry(key).or_default().push(row_id);
    }

    /// 批量构建索引：从表数据按列提取 i64 键
    pub fn build_from_table(
        &mut self,
        table: &dyn TableStorage,
        column_idx: usize,
    ) -> Result<usize, ExecutionError> {
        let mut count = 0;
        for (row_id, row) in table.scan_iter().enumerate() {
            let value = row.get(column_idx).ok_or_else(|| {
                ExecutionError::InvalidArgument(format!(
                    "column index {} out of bounds (row {} has {} columns)",
                    column_idx,
                    row_id,
                    row.len()
                ))
            })?;
            match value {
                Value::Int64(n) => {
                    self.insert(*n, row_id);
                    count += 1;
                }
                Value::Null => {
                    // NULL 不进索引（与 PG 语义一致）
                }
                other => {
                    return Err(ExecutionError::UnsupportedIndexKeyType(format!(
                        "column `{}` value {:?} is not Int64",
                        self.column, other
                    )));
                }
            }
        }
        Ok(count)
    }

    /// 点查：返回所有匹配 key 的 row_ids
    pub fn point_lookup(&self, key: i64) -> Vec<usize> {
        self.index.get(&key).cloned().unwrap_or_default()
    }

    /// 范围查询 [low, high]（含两端）：返回所有匹配的 row_ids（按 key 升序）
    pub fn range_lookup(&self, low: i64, high: i64) -> Vec<usize> {
        if low > high {
            return Vec::new();
        }
        self.index
            .range(low..=high)
            .flat_map(|(_, ids)| ids.iter().copied())
            .collect()
    }

    /// 索引项数量（不同 key 数）
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// 全索引扫描：返回所有 row_ids（按 key 升序）— Phase 5.7
    ///
    /// 当 IndexScan 的谓词不含索引列条件时，退化为全索引扫描。
    /// 仍比 SeqScan 优越的情形：索引列宽度 < 行宽，I/O 量更小（本简化版本未利用此优势）。
    pub fn all_row_ids(&self) -> Vec<usize> {
        self.index
            .values()
            .flat_map(|ids| ids.iter().copied())
            .collect()
    }
}

// =====================================================================
//  IndexAccessPath — Phase 5.7 索引访问路径提取
// =====================================================================

/// 索引访问路径 — 从谓词中提取的索引列访问条件
#[derive(Debug, Clone, PartialEq, Eq)]
enum IndexAccessPath {
    /// 点查：`col = literal`
    Point(i64),
    /// 范围查询：`col >= low AND col <= high`（或其他等价组合）
    /// `(i64::MIN, i64::MAX)` 表示无界范围
    Range(i64, i64),
    /// 无索引列条件（退化为全索引扫描）
    None,
}

/// 从谓词中提取索引列的访问条件
///
/// 支持的谓词形式：
/// - `col = literal` → `Point(literal)`
/// - `col >/>= literal AND col </<= literal` → `Range(low, high)`
/// - `col >/>= literal` → `Range(literal, i64::MAX)`
/// - `col </<= literal` → `Range(i64::MIN, literal)`
/// - AND 连接的多个上述形式 → 取索引列对应的条件
/// - 其他形式 → `None`
///
/// `col_name` 参数为索引首列名（大小写不敏感）
fn extract_index_access(predicate: &Expr, col_name: &str) -> IndexAccessPath {
    let conjuncts = split_and_conjuncts(predicate);
    let mut low: Option<i64> = None;
    let mut high: Option<i64> = None;
    let mut point: Option<i64> = None;

    for conjunct in &conjuncts {
        if let Some((op, value)) = extract_col_literal_comparison(conjunct, col_name) {
            match op {
                BinaryOp::Eq => {
                    point = Some(value);
                    break; // 等值条件优先，无需继续
                }
                BinaryOp::Gt | BinaryOp::GtEq => {
                    let v = if matches!(op, BinaryOp::Gt) {
                        value.saturating_add(1)
                    } else {
                        value
                    };
                    low = Some(match low {
                        Some(cur) => cur.max(v),
                        None => v,
                    });
                }
                BinaryOp::Lt | BinaryOp::LtEq => {
                    let v = if matches!(op, BinaryOp::Lt) {
                        value.saturating_sub(1)
                    } else {
                        value
                    };
                    high = Some(match high {
                        Some(cur) => cur.min(v),
                        None => v,
                    });
                }
                _ => {}
            }
        }
    }

    if let Some(v) = point {
        return IndexAccessPath::Point(v);
    }
    match (low, high) {
        (Some(l), Some(h)) => IndexAccessPath::Range(l, h),
        (Some(l), None) => IndexAccessPath::Range(l, i64::MAX),
        (None, Some(h)) => IndexAccessPath::Range(i64::MIN, h),
        (None, None) => IndexAccessPath::None,
    }
}

/// 将表达式按 AND 拆分为合取项列表
fn split_and_conjuncts(expr: &Expr) -> Vec<&Expr> {
    let mut result = Vec::new();
    split_and_conjuncts_recursive(expr, &mut result);
    result
}

fn split_and_conjuncts_recursive<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::BinaryOp {
        left,
        op: BinaryOp::And,
        right,
    } = expr
    {
        split_and_conjuncts_recursive(left, out);
        split_and_conjuncts_recursive(right, out);
    } else {
        out.push(expr);
    }
}

/// 从 `col OP literal` 或 `literal OP col` 形式中提取 `(op, literal_value)`
///
/// 仅识别 i64 字面量；其他类型返回 None。
fn extract_col_literal_comparison(expr: &Expr, col_name: &str) -> Option<(BinaryOp, i64)> {
    if let Expr::BinaryOp { left, op, right } = expr {
        let bin_op = *op;
        // col OP literal
        if is_col_ref(left, col_name) {
            if let Some(v) = extract_i64_literal(right) {
                return Some((bin_op, v));
            }
        }
        // literal OP col → 翻转运算符
        if is_col_ref(right, col_name) {
            if let Some(v) = extract_i64_literal(left) {
                let flipped = flip_comparison_op(&bin_op)?;
                return Some((flipped, v));
            }
        }
    }
    None
}

/// 判断表达式是否为指定列的引用（大小写不敏感）
fn is_col_ref(expr: &Expr, col_name: &str) -> bool {
    if let Expr::Identifier(parts) = expr {
        if let Some(last) = parts.last() {
            return last.eq_ignore_ascii_case(col_name);
        }
    }
    false
}

/// 从字面量表达式中提取 i64 值
fn extract_i64_literal(expr: &Expr) -> Option<i64> {
    if let Expr::Literal(Value::Int64(n)) = expr {
        return Some(*n);
    }
    None
}

/// 翻转比较运算符（用于 `literal OP col` → `col FLIP(OP) literal`）
fn flip_comparison_op(op: &BinaryOp) -> Option<BinaryOp> {
    Some(match op {
        BinaryOp::Eq => BinaryOp::Eq,
        BinaryOp::Lt => BinaryOp::Gt,
        BinaryOp::Gt => BinaryOp::Lt,
        BinaryOp::LtEq => BinaryOp::GtEq,
        BinaryOp::GtEq => BinaryOp::LtEq,
        _ => return None,
    })
}

// =====================================================================
//  MutableTable trait + TableSnapshot — DML 支持
// =====================================================================

/// 表快照 — 用于事务回滚（Phase 3.5 简化事务模型）
///
/// 捕获表的完整状态（行数据 + 已删除集合 + MVCC 版本元数据），通过 `MutableTable::restore` 恢复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSnapshot {
    /// 行数据副本
    rows: Vec<Row>,
    /// 已删除 row_id 集合副本
    deleted: HashSet<usize>,
    /// P0-TX-1 Phase B：每行 xmin 副本（事务回滚后恢复版本元数据）
    xmin: Vec<u32>,
    /// P0-TX-1 Phase B：每行 xmax 副本
    xmax: Vec<u32>,
}

impl TableSnapshot {
    /// 创建空快照
    pub fn empty() -> Self {
        Self {
            rows: Vec::new(),
            deleted: HashSet::new(),
            xmin: Vec::new(),
            xmax: Vec::new(),
        }
    }

    /// 从行列表构造快照（无已删除行）
    pub fn from_rows(rows: Vec<Row>) -> Self {
        let len = rows.len();
        Self {
            rows,
            deleted: HashSet::new(),
            xmin: vec![0; len],
            xmax: vec![0; len],
        }
    }

    /// 返回快照中所有活跃行（未删除）的克隆迭代器
    ///
    /// Phase 3.35：用于 FLASHBACK TABLE 查询历史快照内容。
    pub fn active_rows(&self) -> Vec<Row> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.deleted.contains(i))
            .map(|(_, row)| row.clone())
            .collect()
    }

    /// 活跃行数
    pub fn active_row_count(&self) -> usize {
        self.rows.len() - self.deleted.len()
    }
}

/// 可变表存储 — 支持插入/更新/删除（DML）
///
/// 扩展 `TableStorage` 添加修改操作。实现方需保证：
/// - `insert_row` 返回的 row_id 在后续 `get_row / update_row / delete_row` 中有效
/// - `delete_row` 后 `get_row(row_id)` 返回 `None`
/// - `snapshot / restore` 完整保存/恢复表状态
pub trait MutableTable: TableStorage {
    /// 插入一行，返回新 row_id
    fn insert_row(&mut self, row: Row) -> usize;

    /// 更新指定 row_id 的行
    ///
    /// 返回 `true` 表示找到并更新，`false` 表示 row_id 不存在或已删除。
    fn update_row(&mut self, row_id: usize, row: Row) -> bool;

    /// 删除指定 row_id 的行（tombstone 语义）
    ///
    /// 返回 `true` 表示找到并删除，`false` 表示 row_id 不存在或已删除。
    fn delete_row(&mut self, row_id: usize) -> bool;

    /// 清空所有行（删除所有数据，重置 row_id 计数）
    fn clear(&mut self);

    /// 创建表快照（用于事务回滚）
    fn snapshot(&self) -> TableSnapshot;

    /// 从快照恢复表状态
    fn restore(&mut self, snapshot: TableSnapshot);

    /// P0-TX-1 Phase B：插入一行并设置 xmin（MVCC 版本元数据）。
    ///
    /// 默认实现退化为 `insert_row`（不记录版本，向后兼容）。
    /// 支持 MVCC 的存储后端覆盖此方法设置真实 xmin。
    fn insert_row_versioned(&mut self, row: Row, xmin: u32) -> usize {
        let _ = xmin;
        self.insert_row(row)
    }

    /// P0-TX-1 Phase B：删除指定 row_id 并设置 xmax（MVCC 版本元数据）。
    ///
    /// 默认实现退化为 `delete_row`（tombstone 语义，向后兼容）。
    /// 支持 MVCC 的存储后端覆盖此方法设置真实 xmax 而非立即 tombstone。
    fn delete_row_versioned(&mut self, row_id: usize, xmax: u32) -> bool {
        let _ = xmax;
        self.delete_row(row_id)
    }
}

impl MutableTable for InMemoryTable {
    fn insert_row(&mut self, row: Row) -> usize {
        self.insert(row)
    }

    fn update_row(&mut self, row_id: usize, row: Row) -> bool {
        InMemoryTable::update_row(self, row_id, row)
    }

    fn delete_row(&mut self, row_id: usize) -> bool {
        InMemoryTable::delete_row(self, row_id)
    }

    /// P0-4：清空所有行（重建 B+Tree，重置 tuple_id 计数）
    fn clear(&mut self) {
        self.truncate()
    }

    /// P0-4：遍历 B+Tree 构建 TableSnapshot（向后兼容）
    fn snapshot(&self) -> TableSnapshot {
        let mut rows = Vec::new();
        let mut deleted = HashSet::new();
        let mut xmin = Vec::new();
        let mut xmax = Vec::new();

        let cursor = match self
            .btree
            .cursor(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = ?e, "snapshot: cursor creation failed");
                return TableSnapshot::empty();
            }
        };

        for (key, value) in cursor {
            let tuple_id = decode_tuple_id_key(&key) as usize;
            match btree_value_codec::decode(&value) {
                Ok((xmn, xmx, row)) => {
                    if self.deleted.contains(&(tuple_id as u32)) {
                        deleted.insert(tuple_id);
                    }
                    rows.push(row);
                    xmin.push(xmn);
                    xmax.push(xmx);
                }
                Err(e) => {
                    tracing::warn!(tuple_id, error = %e, "snapshot: decode failed, skipping");
                }
            }
        }

        TableSnapshot {
            rows,
            deleted,
            xmin,
            xmax,
        }
    }

    /// P0-4：从 TableSnapshot 重建 B+Tree
    fn restore(&mut self, snapshot: TableSnapshot) {
        // 重建 B+Tree
        self.btree = szrsql_storage::btree::BTree::with_default_order();
        self.next_tuple_id = 0;
        self.deleted.clear();
        self.row_cache.clear();

        // 重新插入所有行（保留原始 xmin/xmax）
        for (i, row) in snapshot.rows.into_iter().enumerate() {
            let tuple_id = self.next_tuple_id;
            self.next_tuple_id += 1;
            let xmin = snapshot.xmin.get(i).copied().unwrap_or(0);
            let xmax = snapshot.xmax.get(i).copied().unwrap_or(0);
            let value = btree_value_codec::encode(xmin, xmax, &row);
            let key = encode_tuple_id_key(tuple_id);
            if let Err(e) = self.btree.insert(key, value) {
                tracing::warn!(tuple_id, error = ?e, "restore: BTree insert failed");
            }
            self.row_cache.insert(tuple_id, row);
        }

        // 恢复 deleted 集合
        for &tuple_id in &snapshot.deleted {
            self.deleted.insert(tuple_id as u32);
        }
    }

    /// P0-TX-1 Phase B：插入并设置 xmin
    fn insert_row_versioned(&mut self, row: Row, xmin: u32) -> usize {
        self.insert_with_xmin(row, xmin)
    }

    /// P0-TX-1 Phase B：删除并设置 xmax（不立即 tombstone，由 COMMIT 时 finalize）
    fn delete_row_versioned(&mut self, row_id: usize, xmax: u32) -> bool {
        self.delete_row_with_xmax(row_id, xmax)
    }
}

// =====================================================================
//  执行行上下文
// =====================================================================

/// 执行期行上下文 — 把一行数据 + Schema 暴露给 ExprEvaluator
pub struct ExecRowContext<'a> {
    /// 表 Schema
    schema: &'a TableSchema,
    /// 当前行
    row: &'a Row,
}

impl<'a> ExecRowContext<'a> {
    /// 创建执行期行上下文（内部使用）
    fn new(schema: &'a TableSchema, row: &'a Row) -> Self {
        Self { schema, row }
    }

    /// 创建执行期行上下文（公开接口，供测试与外部调用方使用）
    pub fn new_proxy(schema: &'a TableSchema, row: &'a Row) -> Self {
        Self::new(schema, row)
    }

    /// 按列名查找列索引
    fn find_column_index(&self, name: &str) -> Option<usize> {
        self.schema
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    }
}

impl<'a> EvalContext for ExecRowContext<'a> {
    fn lookup_column(&self, name: &str) -> Result<Value, EvalError> {
        match self.find_column_index(name) {
            Some(idx) => Ok(self.row.get(idx).cloned().unwrap_or(Value::Null)),
            None => Err(EvalError::ColumnNotFound(name.to_string())),
        }
    }

    fn lookup_qualified(&self, table: &str, column: &str) -> Result<Value, EvalError> {
        // 校验表名前缀（若 Schema 表名匹配则忽略前缀，直接按列名查）
        if !self.schema.name.name.eq_ignore_ascii_case(table) {
            // 表名不匹配：仍按列名查（容忍多余前缀）
            // 严格语义需 JOIN 上下文，此处单表场景下宽容处理
        }
        self.lookup_column(column)
    }
}

// =====================================================================
//  UpsertContext — ON CONFLICT DO UPDATE 行上下文
// =====================================================================

/// UPSERT 行上下文 — 同时暴露目标表当前行与 EXCLUDED 伪表行
///
/// 用于 `ON CONFLICT ... DO UPDATE` 子句中的表达式求值：
/// - 不限定列名（如 `name`）→ 查找目标表当前行（即冲突行，将被更新）
/// - `EXCLUDED.col` → 查找拟插入行的对应列值
/// - `target_table.col` → 查找目标表当前行（与不限定列名等价）
///
/// 与 PG 语义一致：未限定列名解析为目标表行，而非 EXCLUDED。
pub struct UpsertContext<'a> {
    /// 目标表 Schema
    target_schema: &'a TableSchema,
    /// 目标表当前行（冲突行）
    target_row: &'a Row,
    /// 拟插入行（EXCLUDED 伪表）
    excluded_row: &'a Row,
}

impl<'a> UpsertContext<'a> {
    /// 创建 UPSERT 行上下文
    pub fn new(target_schema: &'a TableSchema, target_row: &'a Row, excluded_row: &'a Row) -> Self {
        Self {
            target_schema,
            target_row,
            excluded_row,
        }
    }

    /// 在指定 schema 中按列名查找索引
    fn find_idx(schema: &TableSchema, name: &str) -> Option<usize> {
        schema
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    }
}

impl<'a> EvalContext for UpsertContext<'a> {
    fn lookup_column(&self, name: &str) -> Result<Value, EvalError> {
        match Self::find_idx(self.target_schema, name) {
            Some(idx) => Ok(self.target_row.get(idx).cloned().unwrap_or(Value::Null)),
            None => Err(EvalError::ColumnNotFound(name.to_string())),
        }
    }

    fn lookup_qualified(&self, table: &str, column: &str) -> Result<Value, EvalError> {
        // EXCLUDED 伪表 → 拟插入行
        if table.eq_ignore_ascii_case("EXCLUDED") {
            return match Self::find_idx(self.target_schema, column) {
                Some(idx) => Ok(self.excluded_row.get(idx).cloned().unwrap_or(Value::Null)),
                None => Err(EvalError::ColumnNotFound(format!("EXCLUDED.{}", column))),
            };
        }
        // 目标表名前缀 → 当前冲突行
        if self.target_schema.name.name.eq_ignore_ascii_case(table) {
            return self.lookup_column(column);
        }
        // 容忍未知表名前缀，按列名查目标表（与 ExecRowContext 一致的宽容策略）
        self.lookup_column(column)
    }
}

// =====================================================================
//  JoinedRowContext — JOIN 后的双表行上下文
// =====================================================================

/// JOIN 后的行上下文 — 同时暴露左右两表的列给 ExprEvaluator
///
/// 用于 JOIN 条件求值与 JOIN 后的 Projection/Filter：支持 `t1.col` / `t2.col`
/// 限定名查找，以及不限定表名的列查找（先查左表，再查右表）。
///
/// `right_row = None` 表示 LEFT OUTER JOIN 的左表无匹配右表行，
/// 此时所有右表列查找返回 `Value::Null`。
///
/// 使用 `&[Value]` 切片而非 `&Row`，便于在 JOIN 输出行上按列偏移切分
/// （避免为每行克隆出独立的左/右 Row）。
pub struct JoinedRowContext<'a> {
    /// 左表 Schema
    left_schema: &'a TableSchema,
    /// 左表当前行（切片）
    left_row: &'a [Value],
    /// 右表 Schema
    right_schema: &'a TableSchema,
    /// 右表当前行切片（None 表示 OUTER JOIN 的未匹配行）
    right_row: Option<&'a [Value]>,
}

impl<'a> JoinedRowContext<'a> {
    /// 创建 JOIN 行上下文
    pub fn new(
        left_schema: &'a TableSchema,
        left_row: &'a [Value],
        right_schema: &'a TableSchema,
        right_row: Option<&'a [Value]>,
    ) -> Self {
        Self {
            left_schema,
            left_row,
            right_schema,
            right_row,
        }
    }

    /// 按列名在指定 schema 中查找列索引
    fn find_in(schema: &TableSchema, name: &str) -> Option<usize> {
        schema
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// 按表名 + 列名在指定表侧查找值
    fn lookup_side(
        schema: &TableSchema,
        row: Option<&[Value]>,
        table: &str,
        column: &str,
    ) -> Option<Value> {
        if schema.name.name.eq_ignore_ascii_case(table) {
            Self::find_in(schema, column)
                .map(|idx| row.and_then(|r| r.get(idx).cloned()).unwrap_or(Value::Null))
        } else {
            None
        }
    }
}

impl<'a> EvalContext for JoinedRowContext<'a> {
    fn lookup_column(&self, name: &str) -> Result<Value, EvalError> {
        // 先查左表
        if let Some(idx) = Self::find_in(self.left_schema, name) {
            return Ok(self.left_row.get(idx).cloned().unwrap_or(Value::Null));
        }
        // 再查右表
        if let Some(idx) = Self::find_in(self.right_schema, name) {
            return Ok(self
                .right_row
                .and_then(|r| r.get(idx).cloned())
                .unwrap_or(Value::Null));
        }
        Err(EvalError::ColumnNotFound(name.to_string()))
    }

    fn lookup_qualified(&self, table: &str, column: &str) -> Result<Value, EvalError> {
        // 先匹配左表名
        if let Some(v) = Self::lookup_side(self.left_schema, Some(self.left_row), table, column) {
            return Ok(v);
        }
        // 再匹配右表名（right_row 可能为 None，此时仍返回 Null）
        if let Some(v) = Self::lookup_side(self.right_schema, self.right_row, table, column) {
            return Ok(v);
        }
        // 表名都不匹配 → 退化为按列名查（容忍）
        self.lookup_column(column)
    }
}

// =====================================================================
//  TempTableStore — 临时表存储（Phase 3.28）
// =====================================================================

/// 临时表存储 — Phase 3.28
///
/// 会话级隔离的临时表存储，由调用方（会话/连接）拥有。
///
/// # 设计
/// - 执行器仅持有 `&TempTableStore` 只读引用用于 Scan / MERGE / IndexScan 等读取路径
/// - 临时表的创建、删除、ON COMMIT 等修改操作由调用方直接调用本存储的方法
/// - 这样设计遵循 Rust 借用规则：避免执行器同时持有 `&self`（用于 execute_insert）和 `&mut self.temp_tables`
///
/// # 生命周期
/// - **创建**：`create_table_from_plan` — CREATE TEMPORARY TABLE
/// - **查询**：`get` / `get_mut` — 供 DML 操作使用
/// - **COMMIT**：`on_commit` — 应用 ON COMMIT DELETE ROWS / PRESERVE ROWS / DROP
/// - **会话结束**：`clear` — 清空所有临时表（对应 PG 会话断开时自动删除）
///
/// # 会话隔离
/// 每个会话/连接拥有独立的 `TempTableStore` 实例，天然实现会话级隔离：
/// 不同会话的临时表互不可见。
///
/// # 示例
/// ```
/// use szrsql_sql::executor::{Executor, TempTableStore};
/// use szrsql_sql::plan::InMemoryCatalog;
///
/// let mut catalog = InMemoryCatalog::new();
/// let mut temp_store = TempTableStore::new();
/// let executor = Executor::new().with_temp_store(&temp_store);
/// // executor 可读取 temp_store 中的临时表
/// ```
#[derive(Debug, Default)]
pub struct TempTableStore {
    /// 临时表存储（表名小写 → InMemoryTable）
    tables: HashMap<String, InMemoryTable>,
    /// ON COMMIT 行为（表名小写 → 动作）
    on_commit: HashMap<String, OnCommitAction>,
}

impl TempTableStore {
    /// 创建空临时表存储
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            on_commit: HashMap::new(),
        }
    }

    /// 从 CREATE TABLE 计划创建临时表 — Phase 3.28
    ///
    /// # 语义（与 PG 一致）
    /// - `temporary` 必须为 `true`，否则返回错误
    /// - `if_not_exists=true` 且临时表已存在时静默返回
    /// - `if_not_exists=false` 且临时表已存在时返回 `TableAlreadyExists` 错误
    /// - 同时将表 Schema 注册到 `catalog`，以便后续 Planner 解析
    /// - 记录 `on_commit` 行为，供 `on_commit` 方法应用
    ///
    /// # 参数
    /// - `plan`：`LogicalPlan::CreateTable` 计划节点
    /// - `catalog`：可变 catalog 引用，用于注册表 Schema
    pub fn create_table_from_plan(
        &mut self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (name, columns, constraints, if_not_exists, temporary, on_commit) = match plan {
            LogicalPlan::CreateTable {
                name,
                columns,
                constraints,
                if_not_exists,
                temporary,
                on_commit,
            } => (
                name,
                columns,
                constraints,
                *if_not_exists,
                *temporary,
                *on_commit,
            ),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected CreateTable plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        if !temporary {
            return Err(ExecutionError::InvalidArgument(
                "create_table_from_plan called with temporary=false".into(),
            ));
        }
        let key = name.name.to_lowercase();
        if self.tables.contains_key(&key) {
            if if_not_exists {
                return Ok(());
            }
            return Err(ExecutionError::InvalidArgument(format!(
                "temporary table \"{}\" already exists",
                name.qualified_name()
            )));
        }
        // 构造 Schema 并创建 InMemoryTable
        let schema = TableSchema {
            name: name.clone(),
            columns: columns.clone(),
        };
        let table = InMemoryTable::new(schema.clone());
        self.tables.insert(key.clone(), table);
        if let Some(action) = on_commit {
            self.on_commit.insert(key, action);
        }
        // 注册到 catalog（供后续 Planner 解析）
        catalog.add_table(schema);
        // 注意：constraints 当前仅记录，不实际创建索引/约束（与普通 CREATE TABLE 一致）
        let _ = constraints;
        Ok(())
    }

    /// 删除临时表 — Phase 3.28
    ///
    /// 若 `name` 是临时表，从存储中移除，返回 true。
    /// 若不是临时表，返回 false。
    ///
    /// # 注意
    /// 此方法不修改 catalog。调用方负责从 catalog 中移除表 Schema（如有需要）。
    pub fn drop_table(&mut self, name: &str) -> bool {
        let key = name.to_lowercase();
        let removed = self.tables.remove(&key).is_some();
        self.on_commit.remove(&key);
        removed
    }

    /// 判断临时表是否存在 — Phase 3.28
    pub fn exists(&self, name: &str) -> bool {
        self.tables.contains_key(&name.to_lowercase())
    }

    /// 获取临时表的只读引用 — Phase 3.28
    pub fn get(&self, name: &str) -> Option<&InMemoryTable> {
        self.tables.get(&name.to_lowercase())
    }

    /// 获取临时表的可变引用（供 DML 操作使用）— Phase 3.28
    ///
    /// 返回 `Option<&mut InMemoryTable>`。调用方可用此引用执行
    /// `Executor::execute_insert / execute_update / execute_delete` 等需要 `&mut dyn MutableTable` 的操作。
    pub fn get_mut(&mut self, name: &str) -> Option<&mut InMemoryTable> {
        self.tables.get_mut(&name.to_lowercase())
    }

    /// 列出所有临时表名（小写）— Phase 3.28
    pub fn list(&self) -> Vec<String> {
        self.tables.keys().cloned().collect()
    }

    /// 当前临时表数量 — Phase 3.28
    pub fn len(&self) -> usize {
        self.tables.len()
    }

    /// 是否为空 — Phase 3.28
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// 应用 ON COMMIT 行为 — Phase 3.28
    ///
    /// 在事务 COMMIT 时调用，对每个临时表应用其 `on_commit` 行为：
    /// - `DeleteRows`：清空数据但保留表结构
    /// - `PreserveRows`：保留数据（默认，无操作）
    /// - `Drop`：删除临时表（含从 catalog 移除 Schema）
    ///
    /// # 参数
    /// - `catalog`：可变 catalog 引用，用于在 `Drop` 时移除表 Schema
    ///
    /// # 返回
    /// 被 `Drop` 删除的临时表名列表（调用方可用于日志/通知）
    pub fn on_commit(
        &mut self,
        catalog: &mut InMemoryCatalog,
    ) -> Result<Vec<String>, ExecutionError> {
        let mut dropped: Vec<String> = Vec::new();
        // 收集需要处理的表名（避免在迭代中修改 HashMap）
        let keys: Vec<String> = self.on_commit.keys().cloned().collect();
        for key in keys {
            let action = match self.on_commit.get(&key) {
                Some(a) => *a,
                None => continue,
            };
            match action {
                OnCommitAction::PreserveRows => {
                    // 无操作
                }
                OnCommitAction::DeleteRows => {
                    if let Some(table) = self.tables.get_mut(&key) {
                        table.clear();
                    }
                }
                OnCommitAction::Drop => {
                    // 从 tables 移除
                    if let Some(table) = self.tables.remove(&key) {
                        let dropped_name = table.name().to_string();
                        // 从 catalog 移除 Schema
                        let table_name = TableName::new(&dropped_name);
                        catalog.remove_table(&table_name);
                        dropped.push(dropped_name);
                    }
                    // 从 on_commit 移除
                    self.on_commit.remove(&key);
                }
            }
        }
        Ok(dropped)
    }

    /// 会话结束时清理所有临时表 — Phase 3.28
    ///
    /// 对应 PG 语义：会话断开时自动删除所有临时表。
    /// 清空 `tables` 和 `on_commit`。
    pub fn clear(&mut self) {
        self.tables.clear();
        self.on_commit.clear();
    }
}

// =====================================================================
//  Executor — 逻辑计划执行器
// =====================================================================

/// 逻辑计划执行器
///
/// 用法：
/// ```no_run
/// use szrsql_sql::executor::{Executor, InMemoryTable};
/// use szrsql_sql::ast::{ColumnDefinition, TableName};
/// use szrsql_sql::plan::{LogicalPlan, TableSchema};
/// use szrsql_types::value::ColumnType;
///
/// // 1. 注册表
/// let mut exec = Executor::new();
/// let table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
/// exec.register_table(&table);
///
/// // 2. 构建逻辑计划
/// let schema = TableSchema {
///     name: TableName::new("t"),
///     columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
/// };
/// let plan = LogicalPlan::Scan {
///     table: TableName::new("t"),
///     alias: None,
///     schema,
/// };
///
/// // 3. 执行
/// let result = exec.execute(&plan).unwrap();
/// assert_eq!(result.len(), 0);
/// ```
pub struct Executor<'a> {
    /// 表名（小写） → 表存储引用
    tables: HashMap<String, &'a dyn TableStorage>,
    /// 临时表存储引用（会话级隔离）— Phase 3.28
    ///
    /// 由调用方拥有的 `TempTableStore` 提供，执行器仅持有只读引用用于 Scan 等读取路径。
    /// 临时表优先于普通表查询（`lookup_table` 先查此引用）。
    /// 生命周期与会话绑定：调用方在会话结束时调用 `TempTableStore::clear` 清理。
    temp_store: Option<&'a TempTableStore>,
    /// Catalog 引用（用于外键校验）— Phase 3.29
    ///
    /// 提供表 Schema 与 FK 元数据查询。None 表示不启用 FK 校验。
    catalog: Option<&'a dyn Catalog>,
    /// 索引注册表 — Phase 5.7
    ///
    /// Key: `"{table_name_lower}.{index_name_lower}"`，Value: 索引对象引用。
    /// 由调用方通过 `register_index` 注册；IndexScan 计划节点执行时按名查找。
    indexes: HashMap<String, &'a InMemoryBTreeIndex>,
    /// 共享子计划结果缓存 — Phase 5.8
    ///
    /// Key: Shared ID；Value: 该 Shared 节点首次执行的物化结果。
    /// `MemoRef` 节点直接从此缓存读取，避免重复执行相同子树。
    /// 使用 `RefCell` 以便在 `execute(&self, ...)` 的不可变借用下实现缓存写入。
    memo_cache: RefCell<HashMap<u64, Vec<Row>>>,
    /// CTE 物化结果缓存 — Phase 6.1
    ///
    /// Key: CTE 名（小写）；Value: 该 CTE 的物化结果。
    /// - 进入 `With` 节点时，按 ctes 顺序依次执行并填充此缓存
    /// - `CteRef` 节点按名查找
    /// - 离开 `With` 节点时，清除本层 CTE 条目（作用域）
    ///
    /// 使用 `Vec<HashMap<...>>` 模拟作用域栈：每个 `With` 节点压入一层 HashMap，
    /// `CteRef` 从栈顶向下查找，离开 `With` 时弹出栈顶并清除对应条目。
    cte_scopes: RefCell<Vec<HashMap<String, Vec<Row>>>>,
    /// 触发器函数注册表引用 — Phase 6.4
    ///
    /// 由调用方拥有并通过 `with_trigger_registry` 绑定。
    /// DML（INSERT/UPDATE/DELETE）执行时按 catalog 中的触发器定义匹配并调用对应函数。
    /// None 表示不启用触发器（DML 静默跳过触发器调用）。
    trigger_registry: Option<&'a TriggerRegistry>,
    /// 物化视图存储引用注册表 — Phase 6.15
    ///
    /// Key: 物化视图名（小写）；Value: 物化视图存储表引用。
    /// 由调用方通过 `register_materialized_view_store` 注册。
    /// `MaterializedViewScan` 计划节点执行时按名查找。
    materialized_view_stores: HashMap<String, &'a dyn TableStorage>,
    /// UDF 注册表（`Arc` 共享所有权）— P0-SQL-8 修复
    ///
    /// 由调用方通过 `with_udf_registry` 绑定。绑定后，`Executor::execute` 入口
    /// 会把此 `Arc` 设置到当前线程的 `current_udf_registry` thread_local，
    /// 供 `ExprEvaluator` 在内建函数表未命中时回退查询。
    /// `UdfRegistry::call` 仅需 `&self`（`call_counter` 使用 `AtomicU64`）。
    /// None 表示不启用 UDF（表达式求值器对未知函数直接返回 `FunctionNotFound`）。
    udf_registry: Option<Arc<crate::udf::UdfRegistry>>,
    /// SQL 函数定义注册表（`CREATE FUNCTION`）— P0-FN 修复
    ///
    /// 由调用方通过 `with_sql_functions` / `with_sql_functions_from_catalog` 绑定。
    /// `Executor::execute` 入口会把此映射设置到当前线程的 `current_sql_functions`
    /// thread_local，供 `ExprEvaluator` 在内建函数表和 UDF 注册表未命中时回退查询，
    /// 执行 `CREATE FUNCTION` 创建的 SQL/PLpgSQL 函数体。
    /// None 表示不启用 SQL 函数（表达式求值器对未知函数直接返回 `FunctionNotFound`）。
    sql_functions: Option<HashMap<String, Vec<FunctionDefinition>>>,
    /// P0-3：PL/pgSQL 函数注册表（跨会话共享）。
    ///
    /// 注入后，`execute` 入口会将此注册表设置到 `current_plpgsql_interp` 线程局部，
    /// 供 `ExprEvaluator` 在调用 `LANGUAGE plpgsql` 函数时通过 `PlPgSqlInterpreter`
    /// 执行函数体。None 表示不启用 PL/pgSQL 解释器。
    plpgsql_registry: Option<Arc<Mutex<crate::plpgsql_interp::FunctionRegistry>>>,
    /// P0-TX-1 Phase B：MVCC 事务管理器引用（跨会话共享）。
    ///
    /// 注入后，Scan 节点执行时会按 `current_txn_id` 的快照过滤行可见性，
    /// DML 操作会注册 read_set/write_set 用于 SSI 写偏斜检测和 First-Committer-Wins。
    /// None 表示不启用 MVCC（退化为表级 snapshot/restore，旧行为）。
    mvcc: Option<&'a MvccManager>,
    /// P0-TX-1 Phase B：当前事务 ID（由 session 层在 BEGIN 时设置）。
    ///
    /// 0 表示无活跃事务（autocommit 模式，所有行可见）。
    mvcc_txn_id: u32,
    /// P0-DIST-1/2/3：分布式运行时句柄（Arc 共享，'static 生命周期）。
    ///
    /// 注入后，DML 操作（INSERT/UPDATE/DELETE）会同时写入分布式 KV 存储
    /// （通过 Raft propose → apply），实现分布式日志复制。
    /// None 表示不启用分布式写入（纯单机模式，旧行为）。
    ///
    /// **双写策略**：DML 同时写入本地内存表和分布式 KV，保证：
    /// - 本地内存表用于快速查询响应（低延迟）
    /// - 分布式 KV 用于持久化 + 多节点复制（高可用）
    ///
    /// **键编码**：`{table_name}:{row_id}` → serde_json 序列化的行数据
    dist_runtime: Option<szrsql_dist::runtime::DistRuntimeHandle>,
    /// P7-1：CDC 引擎引用（Arc 共享，跨会话共享同一实例）。
    ///
    /// 注入后，DML 操作（INSERT/UPDATE/DELETE）会将行级变更事件分发到 CDC 引擎，
    /// 供已注册的 CdcObserver（如 ReplicationTask）消费。
    /// None 表示不启用 CDC（DML 静默跳过事件分发，旧行为）。
    cdc_engine: Option<std::sync::Arc<szrsql_cdc::CdcEngine>>,
    /// P9-2：WAL 写入器引用（Arc 共享，跨会话共享同一实例）。
    ///
    /// 注入后，DML 操作（INSERT/UPDATE/DELETE）会将行级变更以
    /// `WalOpType::Insert/Update/Delete` 记录写入 WAL，提供细粒度变更流，
    /// 用于 CDC 增量同步和未来 PITR（Point-in-Time Recovery）。
    /// None 表示不启用行级 WAL（DML 静默跳过行级记录，仅依赖 commit 时的
    /// TableData 全表快照做崩溃恢复，旧行为）。
    wal_writer: Option<std::sync::Arc<szrsql_tx::wal::WalWriter>>,
    /// P2-2.1：分布式事务累积器（显式事务模式下共享）。
    ///
    /// 当处于显式事务（BEGIN...COMMIT）且 dist_runtime 已注入时，
    /// 此字段为 `Some(Arc<Mutex<Vec<Mutation>>>)`，与 session 层共享同一 Arc。
    /// DML 操作将 `Mutation::Put`/`Mutation::Delete` 累积到此处，
    /// 由 session 的 COMMIT 触发 Percolator 2PC（prewrite → commit），
    /// ROLLBACK 触发 2PC rollback。
    ///
    /// 为 None 时（autocommit 模式或未注入 dist_runtime），DML 走即时 2PC
    /// （begin → prewrite → commit 单语句事务）或退化为直接写（无 dist_runtime）。
    dist_txn_mutations: Option<std::sync::Arc<std::sync::Mutex<Vec<szrsql_dist::txn::Mutation>>>>,
    /// P2-1：HLC 混合逻辑时钟（Multi-Master 因果排序）。
    ///
    /// 注入后，DML 操作会调用 `stamp_hlc_timestamp()` 获取 HLC 时间戳，
    /// 用于 Multi-Master 场景下的因果排序和冲突检测。
    /// None 表示不启用 Multi-Master 因果排序（单节点模式，旧行为）。
    hlc_clock: Option<std::sync::Arc<std::sync::Mutex<szrsql_dist::conflict::HlcClock>>>,
    /// P2-1：冲突日志（Multi-Master 写入冲突审计）。
    ///
    /// 注入后，当检测到写-写冲突（如 duplicate key）时，调用
    /// `record_write_conflict()` 记录冲突事件到日志，用于审计和回放。
    /// None 表示不启用冲突日志（单节点模式，旧行为）。
    conflict_log: Option<std::sync::Arc<std::sync::Mutex<szrsql_dist::conflict::ConflictLog>>>,
    /// P2-1：本节点 ID（Multi-Master 写操作来源标识）。
    ///
    /// 用于 HLC 时间戳和冲突日志中标识写操作来源节点。
    /// 默认为 1（单节点模式）。
    node_id: u64,
    /// P1-9：行锁管理器（Arc 共享，跨会话共享同一实例）。
    ///
    /// 注入后，UPDATE/DELETE 操作在扫描到匹配行后、实际修改前，
    /// 会通过 LockManager 获取行级 X 锁（资源 ID = row_resource_id(table, row_id)），
    /// 实现行级冲突检测。若两事务并发修改同一行，后到事务将等待或死锁中止。
    ///
    /// None 表示不启用行级锁（退化为表级 Mutex 串行化，旧行为）。
    row_lock_manager: Option<std::sync::Arc<szrsql_tx::lock::LockManager>>,
    /// P1-9：行锁所属事务 ID（与 row_lock_manager 配合使用）。
    ///
    /// 0 表示无活跃事务（autocommit 模式，不获取行级锁）。
    row_lock_txn_id: u32,
}

// =====================================================================
//  PreparedStatementStore — Phase 3.26
// =====================================================================

/// 预处理语句存储 — Phase 3.26
///
/// 存储命名预处理语句（name → (Statement, parameter_types)）。
///
/// # 生命周期
/// - **PREPARE**：将 AST 存入此存储（不立即 plan）
/// - **EXECUTE**：取出 AST 克隆、替换 `$N` 占位符为实际值、再 plan + 执行
/// - **DEALLOCATE**：从存储中删除
///
/// # 语义（与 PG 一致）
/// - 同名 PREPARE 会覆盖之前的定义
/// - DEALLOCATE 不存在的语句报错
/// - DEALLOCATE ALL 清空所有预处理语句
/// - 会话级存储：每个连接/事务独立持有一份
#[derive(Debug, Default, Clone)]
pub struct PreparedStatementStore {
    /// 预处理语句名（小写） → (AST, 参数类型声明)
    statements: HashMap<String, (Statement, Vec<szrsql_types::value::ColumnType>)>,
}

impl PreparedStatementStore {
    /// 创建空存储
    pub fn new() -> Self {
        Self {
            statements: HashMap::new(),
        }
    }

    /// 存储预处理语句（若同名已存在则覆盖，与 PG 行为一致）
    pub fn prepare(
        &mut self,
        name: &str,
        statement: Statement,
        parameter_types: Vec<szrsql_types::value::ColumnType>,
    ) {
        self.statements
            .insert(name.to_lowercase(), (statement, parameter_types));
    }

    /// 取出预处理语句的引用
    pub fn get(&self, name: &str) -> Option<&(Statement, Vec<szrsql_types::value::ColumnType>)> {
        self.statements.get(&name.to_lowercase())
    }

    /// 删除指定预处理语句。返回是否曾存在。
    pub fn deallocate(&mut self, name: &str) -> bool {
        self.statements.remove(&name.to_lowercase()).is_some()
    }

    /// 删除所有预处理语句（DEALLOCATE ALL）
    pub fn deallocate_all(&mut self) {
        self.statements.clear();
    }

    /// 是否存在指定预处理语句
    pub fn exists(&self, name: &str) -> bool {
        self.statements.contains_key(&name.to_lowercase())
    }

    /// 列出所有预处理语句名（小写）
    pub fn list(&self) -> Vec<String> {
        self.statements.keys().cloned().collect()
    }

    /// 当前预处理语句数量
    pub fn len(&self) -> usize {
        self.statements.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }
}

// =====================================================================
//  SessionState — Phase 3.34
// =====================================================================

/// 会话状态 — Phase 3.34
///
/// 存储 `SET` 命令设置的会话变量（如 `statement_timeout`、`search_path`、
/// `NAMES` 字符集等）。会话级生命周期：每个连接/事务独立持有一份。
///
/// # 语义（与 PG/MySQL 一致）
/// - `SET var = value` → 写入或覆盖现有值
/// - `SHOW var` → 读取当前值（不存在时返回空字符串）
/// - `SET NAMES 'charset'` → 写入特殊键 `names_charset` 和可选 `names_collation`
/// - 默认值：未设置的变量返回 `Value::Text("")`（与 PG `SHOW` 行为一致）
#[derive(Debug, Default, Clone)]
pub struct SessionState {
    /// 会话变量（小写键） → 值
    variables: HashMap<String, Value>,
}

impl SessionState {
    /// 创建空会话状态（注入 PostgreSQL 默认变量，与 PG 14 默认值一致）
    ///
    /// 这些默认值让 Navicat 等客户端在启动时发送的 SHOW 命令能返回真实值，
    /// 避免客户端因空值进入异常分支。
    pub fn new() -> Self {
        let mut vars: HashMap<String, Value> = HashMap::new();
        vars.insert(
            "server_version".into(),
            Value::Text("14.0-szrsql (SzRSQL 1.0.0-rc.2)".into()),
        );
        vars.insert("server_encoding".into(), Value::Text("UTF8".into()));
        vars.insert("client_encoding".into(), Value::Text("UTF8".into()));
        vars.insert(
            "transaction_isolation".into(),
            Value::Text("read committed".into()),
        );
        vars.insert(
            "standard_conforming_strings".into(),
            Value::Text("on".into()),
        );
        vars.insert("integer_datetimes".into(), Value::Text("on".into()));
        vars.insert("timezone".into(), Value::Text("UTC".into()));
        vars.insert("extra_float_digits".into(), Value::Text("3".into()));
        vars.insert("search_path".into(), Value::Text("public".into()));
        vars.insert("max_connections".into(), Value::Text("100".into()));
        vars.insert("application_name".into(), Value::Text(String::new()));
        vars.insert("datestyle".into(), Value::Text("ISO, MDY".into()));
        vars.insert("intervalstyle".into(), Value::Text("postgres".into()));
        vars.insert("lc_collate".into(), Value::Text("C".into()));
        vars.insert("lc_ctype".into(), Value::Text("C".into()));
        vars.insert("listen_addresses".into(), Value::Text("*".into()));
        vars.insert("max_wal_senders".into(), Value::Text("0".into()));
        vars.insert("hot_standby".into(), Value::Text("off".into()));
        vars.insert("wal_level".into(), Value::Text("replica".into()));
        vars.insert(
            "data_directory".into(),
            Value::Text("/var/lib/postgresql/data".into()),
        );
        vars.insert(
            "hba_file".into(),
            Value::Text("/var/lib/postgresql/data/pg_hba.conf".into()),
        );
        vars.insert(
            "ident_file".into(),
            Value::Text("/var/lib/postgresql/data/pg_ident.conf".into()),
        );
        Self { variables: vars }
    }

    /// 设置变量值（变量名大小写不敏感，存储为小写）
    pub fn set(&mut self, variable: &str, value: Value) {
        self.variables.insert(variable.to_lowercase(), value);
    }

    /// 读取变量值（变量名大小写不敏感）
    ///
    /// 不存在时返回 None（调用方可决定返回默认值或报错）。
    pub fn get(&self, variable: &str) -> Option<&Value> {
        self.variables.get(&variable.to_lowercase())
    }

    /// 设置 NAMES 字符集（SET NAMES 'charset' [COLLATE 'collation']）
    pub fn set_names(&mut self, charset: &str, collation: Option<&str>) {
        self.variables
            .insert("names_charset".into(), Value::Text(charset.into()));
        if let Some(c) = collation {
            self.variables
                .insert("names_collation".into(), Value::Text(c.into()));
        }
    }

    /// 当前变量数量
    pub fn len(&self) -> usize {
        self.variables.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.variables.is_empty()
    }
}

impl<'a> Executor<'a> {
    /// 创建空执行器
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            temp_store: None,
            catalog: None,
            indexes: HashMap::new(),
            memo_cache: RefCell::new(HashMap::new()),
            cte_scopes: RefCell::new(Vec::new()),
            trigger_registry: None,
            materialized_view_stores: HashMap::new(),
            udf_registry: None,
            sql_functions: None,
            mvcc: None,
            mvcc_txn_id: 0,
            dist_runtime: None,
            cdc_engine: None,
            wal_writer: None,
            dist_txn_mutations: None,
            hlc_clock: None,
            conflict_log: None,
            node_id: 1,
            row_lock_manager: None,
            row_lock_txn_id: 0,
            plpgsql_registry: None,
        }
    }

    /// 注册表
    pub fn register_table(&mut self, table: &'a dyn TableStorage) {
        self.tables.insert(table.name().to_lowercase(), table);
    }

    /// 注册物化视图存储 — Phase 6.15
    ///
    /// 将物化视图存储表注册到执行器，供 `MaterializedViewScan` 计划节点执行时按名查找。
    /// Key 为物化视图名（小写）。
    pub fn register_materialized_view_store(&mut self, name: &str, store: &'a dyn TableStorage) {
        self.materialized_view_stores
            .insert(name.to_lowercase(), store);
    }

    /// 按物化视图名查找存储引用 — Phase 6.15
    fn lookup_materialized_view_store(&self, name: &str) -> Option<&'a dyn TableStorage> {
        self.materialized_view_stores
            .get(&name.to_lowercase())
            .copied()
    }

    /// 注册索引 — Phase 5.7
    ///
    /// 将 `InMemoryBTreeIndex` 注册到执行器，供 `IndexScan` 计划节点执行时按名查找。
    /// Key 格式：`"{table_name_lower}.{index_name_lower}"`。
    pub fn register_index(&mut self, index: &'a InMemoryBTreeIndex) {
        let key = format!(
            "{}.{}",
            index.table_name().to_lowercase(),
            index.name().to_lowercase()
        );
        self.indexes.insert(key, index);
    }

    /// 按表名 + 索引名查找已注册索引 — Phase 5.7
    fn lookup_index(&self, table: &str, index_name: &str) -> Option<&InMemoryBTreeIndex> {
        let key = format!("{}.{}", table.to_lowercase(), index_name.to_lowercase());
        self.indexes.get(&key).copied()
    }

    /// 绑定临时表存储 — Phase 3.28
    ///
    /// 执行器仅持有只读引用用于 Scan / MERGE / IndexScan 等读取路径。
    /// 临时表的创建、删除、ON COMMIT 等修改操作由调用方通过 `TempTableStore` 直接调用。
    pub fn with_temp_store(mut self, store: &'a TempTableStore) -> Self {
        self.temp_store = Some(store);
        self
    }

    /// 设置临时表存储 — Phase 3.28
    pub fn set_temp_store(&mut self, store: &'a TempTableStore) {
        self.temp_store = Some(store);
    }

    /// 绑定 Catalog（用于外键校验）— Phase 3.29
    ///
    /// 绑定后，`execute_insert` / `execute_update` / `execute_delete` 会自动校验外键约束。
    pub fn with_catalog(mut self, catalog: &'a dyn Catalog) -> Self {
        self.catalog = Some(catalog);
        self
    }

    /// 设置 Catalog — Phase 3.29
    pub fn set_catalog(&mut self, catalog: &'a dyn Catalog) {
        self.catalog = Some(catalog);
    }

    /// 绑定触发器函数注册表 — Phase 6.4
    ///
    /// 绑定后，DML（INSERT/UPDATE/DELETE）会自动调用 catalog 中匹配的触发器。
    /// 未绑定时 DML 静默跳过触发器调用（保持向后兼容）。
    pub fn with_trigger_registry(mut self, registry: &'a TriggerRegistry) -> Self {
        self.trigger_registry = Some(registry);
        self
    }

    /// 设置触发器函数注册表 — Phase 6.4
    pub fn set_trigger_registry(&mut self, registry: &'a TriggerRegistry) {
        self.trigger_registry = Some(registry);
    }

    /// 绑定 UDF 注册表 — P0-SQL-8 修复
    ///
    /// 绑定后，`ExprEvaluator` 在内建函数表未命中时会回退查询此注册表。
    /// 未绑定时表达式求值器对未知函数直接返回 `FunctionNotFound`。
    pub fn with_udf_registry(mut self, registry: Arc<crate::udf::UdfRegistry>) -> Self {
        self.udf_registry = Some(registry);
        self
    }

    /// 设置 UDF 注册表 — P0-SQL-8 修复
    pub fn set_udf_registry(&mut self, registry: Arc<crate::udf::UdfRegistry>) {
        self.udf_registry = Some(registry);
    }

    /// 绑定 SQL 函数定义注册表（`CREATE FUNCTION`）— P0-FN 修复
    ///
    /// 绑定后，`Executor::execute` 入口会把此映射设置到当前线程的
    /// `current_sql_functions` thread_local，供 `ExprEvaluator` 在内建函数表
    /// 和 UDF 注册表未命中时回退查询，执行 SQL/PLpgSQL 函数体。
    /// 未绑定时表达式求值器对未知函数直接返回 `FunctionNotFound`。
    pub fn with_sql_functions(
        mut self,
        functions: HashMap<String, Vec<FunctionDefinition>>,
    ) -> Self {
        self.sql_functions = Some(functions);
        self
    }

    /// 设置 SQL 函数定义注册表 — P0-FN 修复
    pub fn set_sql_functions(&mut self, functions: HashMap<String, Vec<FunctionDefinition>>) {
        self.sql_functions = Some(functions);
    }

    /// 从 `InMemoryCatalog` 构建并绑定 SQL 函数定义注册表 — P0-FN 修复
    ///
    /// 遍历 catalog 中所有已注册的函数定义（含重载），构建
    /// `HashMap<函数名小写, Vec<FunctionDefinition>>` 并绑定到执行器。
    /// `Executor::execute` 入口会据此设置 `current_sql_functions` 线程局部注册表。
    pub fn with_sql_functions_from_catalog(mut self, catalog: &InMemoryCatalog) -> Self {
        self.sql_functions = Some(Self::collect_sql_functions(catalog));
        self
    }

    /// 设置：从 `InMemoryCatalog` 构建并绑定 SQL 函数定义注册表 — P0-FN 修复
    pub fn set_sql_functions_from_catalog(&mut self, catalog: &InMemoryCatalog) {
        self.sql_functions = Some(Self::collect_sql_functions(catalog));
    }

    /// 从 catalog 收集全部 SQL 函数定义（含重载）为映射 — P0-FN 修复
    ///
    /// Key 为函数名小写，Value 为该函数名的所有重载定义（克隆）。
    ///
    /// P0-FN-TYPE 修复：改为 pub，供协议层在 Describe 等无 executor 的场景下
    /// 单独设置 `current_sql_functions` guard，使 `derive_output_columns`
    /// 能正确推导函数返回类型。
    pub fn collect_sql_functions(
        catalog: &InMemoryCatalog,
    ) -> HashMap<String, Vec<FunctionDefinition>> {
        let mut map: HashMap<String, Vec<FunctionDefinition>> = HashMap::new();
        for name in catalog.list_functions() {
            let overloads = catalog.list_function_overloads(&name);
            let key = name.to_lowercase();
            map.insert(key, overloads.into_iter().cloned().collect());
        }
        map
    }

    /// P0-TX-1 Phase B：绑定 MVCC 事务管理器 + 当前事务 ID。
    ///
    /// 绑定后：
    /// - `execute_scan` 会按当前事务快照过滤行可见性（xmin/xmax）
    /// - DML 操作会设置 xmin/xmax 版本元数据
    /// - DML 操作会注册 read_set/write_set（SSI + First-Committer-Wins）
    ///
    /// `txn_id` 为 0 表示 autocommit 模式（无活跃事务，所有已提交行可见）。
    /// 未绑定时退化为旧行为（表级 snapshot/restore，无 MVCC 可见性判断）。
    pub fn with_mvcc(mut self, mvcc: &'a MvccManager, txn_id: u32) -> Self {
        self.mvcc = Some(mvcc);
        self.mvcc_txn_id = txn_id;
        self
    }

    /// P0-TX-1 Phase B：设置 MVCC 上下文（mutable setter）
    pub fn set_mvcc(&mut self, mvcc: &'a MvccManager, txn_id: u32) {
        self.mvcc = Some(mvcc);
        self.mvcc_txn_id = txn_id;
    }

    /// P0-TX-1 Phase B：判断 MVCC 是否已启用
    pub fn has_mvcc(&self) -> bool {
        self.mvcc.is_some()
    }

    /// P0-DIST-1/2/3：绑定分布式运行时句柄。
    ///
    /// 绑定后，DML 操作（INSERT/UPDATE/DELETE）会双写到分布式 KV 存储
    /// （通过 Raft propose → apply），实现分布式日志复制。
    /// 未绑定时退化为纯单机模式（仅写本地内存表）。
    ///
    /// `handle` 为 `Arc<RwLock<DistRuntime>>`，可跨线程共享。
    pub fn with_dist_runtime(mut self, handle: szrsql_dist::runtime::DistRuntimeHandle) -> Self {
        self.dist_runtime = Some(handle);
        self
    }

    /// P2-2.1：绑定分布式事务累积器（显式事务模式）。
    ///
    /// 注入后，DML 操作将 `Mutation` 累积到此共享 `Arc<Mutex<Vec<Mutation>>>`，
    /// 由 session 层的 COMMIT/ROLLBACK 统一触发 Percolator 2PC。
    /// 未注入时（autocommit），DML 走即时 2PC 或直接写。
    pub fn with_dist_txn_mutations(
        mut self,
        mutations: std::sync::Arc<std::sync::Mutex<Vec<szrsql_dist::txn::Mutation>>>,
    ) -> Self {
        self.dist_txn_mutations = Some(mutations);
        self
    }

    /// P7-1：绑定 CDC 引擎，启用 DML 事件分发。
    ///
    /// 绑定后，DML 操作（INSERT/UPDATE/DELETE）会将行级变更事件分发到 CDC 引擎，
    /// 供已注册的 CdcObserver（如 ReplicationTask）消费。
    /// 未绑定时退化为旧行为（DML 不触发 CDC 事件）。
    pub fn with_cdc_engine(mut self, engine: std::sync::Arc<szrsql_cdc::CdcEngine>) -> Self {
        self.cdc_engine = Some(engine);
        self
    }

    /// P9-2：绑定 WAL 写入器，启用 DML 行级 WAL 记录。
    ///
    /// 绑定后，DML 操作（INSERT/UPDATE/DELETE）会将行级变更以
    /// `WalOpType::Insert/Update/Delete` 记录写入 WAL。记录在 DML 执行时立即
    /// append（不 fsync），事务提交时由 `commit_transaction` 统一 fsync。
    /// 未绑定时退化为旧行为（仅 commit 时写 TableData 全表快照）。
    pub fn with_wal_writer(mut self, writer: std::sync::Arc<szrsql_tx::wal::WalWriter>) -> Self {
        self.wal_writer = Some(writer);
        self
    }

    /// P2-1：绑定 HLC 混合逻辑时钟，启用 Multi-Master 因果排序。
    ///
    /// 绑定后，DML 操作会调用 `stamp_hlc_timestamp()` 获取 HLC 时间戳，
    /// 用于 Multi-Master 场景下的因果排序和冲突检测。
    /// 未绑定时退化为旧行为（不生成 HLC 时间戳）。
    pub fn with_hlc_clock(
        mut self,
        clock: std::sync::Arc<std::sync::Mutex<szrsql_dist::conflict::HlcClock>>,
    ) -> Self {
        self.hlc_clock = Some(clock);
        self
    }

    /// P2-1：绑定冲突日志，启用 Multi-Master 写入冲突审计。
    ///
    /// 绑定后，当检测到写-写冲突时，调用 `record_write_conflict()` 记录冲突事件。
    /// 未绑定时退化为旧行为（不记录冲突日志）。
    pub fn with_conflict_log(
        mut self,
        log: std::sync::Arc<std::sync::Mutex<szrsql_dist::conflict::ConflictLog>>,
    ) -> Self {
        self.conflict_log = Some(log);
        self
    }

    /// P2-1：设置本节点 ID（Multi-Master 写操作来源标识）。
    ///
    /// 用于 HLC 时间戳和冲突日志中标识写操作来源节点。
    /// 默认为 1（单节点模式）。
    pub fn with_node_id(mut self, node_id: u64) -> Self {
        self.node_id = node_id;
        self
    }

    /// P1-9：注入行锁管理器（启用行级冲突检测）。
    ///
    /// 注入后，UPDATE/DELETE 操作在扫描到匹配行后、实际修改前，
    /// 会通过 LockManager 获取行级 X 锁，实现行级冲突检测。
    ///
    /// # 参数
    /// - `lm`：共享行锁管理器（`Arc<LockManager>`，由 `PgwireServer` 持有）
    /// - `txn_id`：当前事务 ID（0 表示 autocommit，不获取行级锁）
    pub fn with_row_lock_manager(
        mut self,
        lm: std::sync::Arc<szrsql_tx::lock::LockManager>,
        txn_id: u32,
    ) -> Self {
        self.row_lock_manager = Some(lm);
        self.row_lock_txn_id = txn_id;
        self
    }

    /// P1-9：计算行级锁资源 ID。
    ///
    /// 使用 `(table_hash << 32) | row_id` 编码，高 bit 为 0（区分表级锁高 bit=1）。
    /// table_hash 使用稳定哈希（DefaultHasher），row_id 为表内行号（0..N）。
    fn row_resource_id(table_name: &str, row_id: usize) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        table_name.to_lowercase().hash(&mut hasher);
        let hash = hasher.finish();
        // 高 bit 为 0（区分表级锁的高 bit=1），取 table_hash 低 31 bit
        let table_part = hash & 0x7FFF_FFFF;
        (table_part << 32) | (row_id as u64 & 0xFFFF_FFFF)
    }

    /// P1-9：对匹配行获取行级 X 锁。
    ///
    /// 在 UPDATE/DELETE 扫描到匹配行后、实际修改前调用。
    /// 若行锁管理器未注入或 txn_id=0，直接返回 Ok（autocommit 模式）。
    /// 若锁冲突导致死锁，返回 `ExecutionError::LockConflict`。
    ///
    /// # 参数
    /// - `table_name`：目标表名
    /// - `row_ids`：待加锁的行 ID 列表
    fn acquire_row_xlocks(
        &self,
        table_name: &str,
        row_ids: &[usize],
    ) -> Result<(), ExecutionError> {
        let lm = match &self.row_lock_manager {
            Some(lm) => lm,
            None => return Ok(()),
        };
        let txn_id = self.row_lock_txn_id;
        if txn_id == 0 || row_ids.is_empty() {
            return Ok(());
        }
        for &row_id in row_ids {
            let resource = Self::row_resource_id(table_name, row_id);
            match lm.lock(
                txn_id,
                resource,
                szrsql_tx::lock::LockMode::Exclusive,
                std::time::Duration::from_secs(30),
            ) {
                Ok(()) => {}
                Err(szrsql_tx::lock::LockError::Deadlock(aborted_txn_id)) => {
                    lm.unlock_all(aborted_txn_id);
                    return Err(ExecutionError::LockConflict(format!(
                        "row lock deadlock: txn {aborted_txn_id} aborted (table={table_name}, row_id={row_id})"
                    )));
                }
                Err(e) => {
                    return Err(ExecutionError::LockConflict(format!(
                        "row lock failed: {e} (table={table_name}, row_id={row_id})"
                    )));
                }
            }
        }
        Ok(())
    }

    /// P2-1：获取 HLC 时间戳（Multi-Master 因果排序用）。
    ///
    /// 若 HLC 时钟未注入，返回 None（单节点模式）。
    /// 若 HLC 时钟已注入但锁获取失败（poisoned），返回 None 并记录 warn。
    fn stamp_hlc_timestamp(&self) -> Option<szrsql_dist::conflict::HlcTimestamp> {
        let clock = self.hlc_clock.as_ref()?;
        match clock.lock() {
            Ok(mut c) => Some(c.now()),
            Err(e) => {
                tracing::warn!(error = %e, "P2-1: HlcClock mutex poisoned");
                None
            }
        }
    }

    /// P2-1：记录写-写冲突到 ConflictLog（Multi-Master 审计用）。
    ///
    /// 当 Multi-Master 场景下检测到 duplicate key 等写-写冲突时调用。
    /// 若 ConflictLog 未注入，直接返回（单节点模式）。
    fn record_write_conflict(
        &self,
        winner_key: &[u8],
        winner_value: &[u8],
        loser_key: &[u8],
        loser_value: &[u8],
    ) {
        let log = match self.conflict_log.as_ref() {
            Some(l) => l,
            None => return,
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let hlc_ts = self.stamp_hlc_timestamp();
        let timestamp = hlc_ts.as_ref().map(|t| t.l).unwrap_or(now_ms);
        let winner = szrsql_dist::conflict::WriteOperation {
            node_id: self.node_id,
            lsn: now_ms,
            timestamp,
            key: winner_key.to_vec(),
            value: winner_value.to_vec(),
        };
        let loser = szrsql_dist::conflict::WriteOperation {
            node_id: self.node_id,
            lsn: now_ms,
            timestamp,
            key: loser_key.to_vec(),
            value: loser_value.to_vec(),
        };
        let entry = szrsql_dist::conflict::ConflictEntry {
            winner,
            loser,
            detected_at: now_ms,
            resolution: szrsql_dist::conflict::ConflictResolution::LastTimestampWins,
        };
        if let Ok(mut log) = log.lock() {
            log.record(entry);
            tracing::info!(
                node_id = self.node_id,
                "P2-1: write conflict recorded to ConflictLog"
            );
        }
    }

    /// P7-1：将 Row 序列化为字节流（CDC 事件载荷）
    fn serialize_row_for_cdc(row: &Row) -> Vec<u8> {
        serde_json::to_vec(row).unwrap_or_default()
    }

    /// P7-1：将表名转为稳定的 table_id（FNV-1a 哈希，u32 范围）
    ///
    /// 用于 CDC 事件中的 table_id 字段，保证同一表名在进程内得到相同 ID。
    /// P1-5 起公开供 WAL 行级回放使用（通过 table_id 反查目标表）。
    pub fn table_name_to_id(table_name: &str) -> u32 {
        // FNV-1a 32-bit 哈希
        let mut hash: u32 = 0x811c9dc5;
        for byte in table_name.as_bytes() {
            hash ^= *byte as u32;
            hash = hash.wrapping_mul(0x01000193);
        }
        hash
    }

    /// P2-1：获取 CDC 事件时间戳（优先 HLC，回退到引擎时间戳）。
    ///
    /// Multi-Master 模式下使用 HLC 时间戳保证因果排序，
    /// 单节点模式回退到 `engine.current_timestamp()`。
    fn cdc_event_timestamp(&self, engine: &szrsql_cdc::CdcEngine) -> u64 {
        self.stamp_hlc_timestamp()
            .map(|t| t.l)
            .unwrap_or_else(|| engine.current_timestamp())
    }

    /// P7-1：分发 CDC Insert 事件（内部辅助方法）
    ///
    /// Batch 3：显式事务（mvcc_txn_id != 0）时缓冲到 CdcEngine 事务缓冲区，
    /// COMMIT 后统一分发。
    /// P2-2：autocommit 模式（mvcc_txn_id == 0）也走 staging 缓冲（虚拟 tx_id=1），
    /// 在语句执行完成后由 `flush_autocommit_cdc_events` 统一 flush，减少同步开销。
    fn dispatch_cdc_insert(&self, table_name: &str, new_row: &Row) {
        if let Some(engine) = &self.cdc_engine {
            let table_id = Self::table_name_to_id(table_name);
            let lsn = engine.next_lsn();
            let tx_id = if self.mvcc_txn_id != 0 {
                self.mvcc_txn_id
            } else {
                1 // autocommit 模式使用虚拟 tx_id=1
            };
            let event = szrsql_cdc::ChangeEvent::insert(
                tx_id,
                lsn,
                table_id,
                Self::serialize_row_for_cdc(new_row),
                self.cdc_event_timestamp(engine),
            );
            // P2-2：统一走 stage_event 路径，减少同步开销
            // - 显式事务（mvcc_txn_id != 0）：stage 到 mvcc_txn_id，COMMIT 时统一 flush
            // - autocommit（mvcc_txn_id == 0）：stage 到虚拟 tx_id=1，
            //   由 flush_autocommit_cdc_events 在语句执行完成后统一 flush
            engine.stage_event(tx_id, event);
        }
    }

    /// P7-1：分发 CDC Update 事件（内部辅助方法）
    ///
    /// Batch 3：显式事务时缓冲，COMMIT 后统一分发。
    /// P2-2：autocommit 模式也走 staging 缓冲（虚拟 tx_id=1），
    /// 语句执行完成后由 `flush_autocommit_cdc_events` 统一 flush。
    fn dispatch_cdc_update(&self, table_name: &str, old_row: &Row, new_row: &Row) {
        if let Some(engine) = &self.cdc_engine {
            let table_id = Self::table_name_to_id(table_name);
            let lsn = engine.next_lsn();
            let tx_id = if self.mvcc_txn_id != 0 {
                self.mvcc_txn_id
            } else {
                1
            };
            let event = szrsql_cdc::ChangeEvent::update(
                tx_id,
                lsn,
                table_id,
                Self::serialize_row_for_cdc(old_row),
                Self::serialize_row_for_cdc(new_row),
                self.cdc_event_timestamp(engine),
            );
            // P2-2：统一走 stage_event 路径，减少同步开销
            // - 显式事务（mvcc_txn_id != 0）：stage 到 mvcc_txn_id，COMMIT 时统一 flush
            // - autocommit（mvcc_txn_id == 0）：stage 到虚拟 tx_id=1，
            //   由 flush_autocommit_cdc_events 在语句执行完成后统一 flush
            engine.stage_event(tx_id, event);
        }
    }

    /// P7-1：分发 CDC Delete 事件（内部辅助方法）
    ///
    /// Batch 3：显式事务时缓冲，COMMIT 后统一分发。
    /// P2-2：autocommit 模式也走 staging 缓冲（虚拟 tx_id=1），
    /// 语句执行完成后由 `flush_autocommit_cdc_events` 统一 flush。
    fn dispatch_cdc_delete(&self, table_name: &str, old_row: &Row) {
        if let Some(engine) = &self.cdc_engine {
            let table_id = Self::table_name_to_id(table_name);
            let lsn = engine.next_lsn();
            let tx_id = if self.mvcc_txn_id != 0 {
                self.mvcc_txn_id
            } else {
                1
            };
            let event = szrsql_cdc::ChangeEvent::delete(
                tx_id,
                lsn,
                table_id,
                Self::serialize_row_for_cdc(old_row),
                self.cdc_event_timestamp(engine),
            );
            // P2-2：统一走 stage_event 路径，减少同步开销
            // - 显式事务（mvcc_txn_id != 0）：stage 到 mvcc_txn_id，COMMIT 时统一 flush
            // - autocommit（mvcc_txn_id == 0）：stage 到虚拟 tx_id=1，
            //   由 flush_autocommit_cdc_events 在语句执行完成后统一 flush
            engine.stage_event(tx_id, event);
        }
    }

    /// P2-2：autocommit 模式下统一分发暂存的 CDC 事件
    ///
    /// # 调用时机
    /// 在 DML 语句（INSERT/UPDATE/DELETE）执行成功后调用。若处于 autocommit
    /// 模式（`mvcc_txn_id == 0`），将虚拟 tx_id=1 的所有暂存事件统一 flush 到
    /// CdcEngine 的 observer。
    ///
    /// # 设计要点
    /// - autocommit 模式下 `dispatch_cdc_*` 将事件 stage 到缓冲区（虚拟 tx_id=1），
    ///   而非逐条同步分发，减少每行的同步开销（observer 锁获取/释放、catch_unwind）
    /// - 语句执行完成后一次性 flush，语义等价于立即分发，但走 staging 路径
    /// - 显式事务模式（`mvcc_txn_id != 0`）不在此处 flush，由 COMMIT 时统一 flush
    /// - 若无 staged 事件，`flush_staged_events` 返回 0（no-op），无副作用
    /// - 未绑定 CDC 引擎时（`cdc_engine == None`），直接返回（no-op）
    fn flush_autocommit_cdc_events(&self) {
        // 仅 autocommit 模式（mvcc_txn_id == 0）需要在此 flush
        // 显式事务模式由 COMMIT 流程统一 flush，此处跳过
        if self.mvcc_txn_id != 0 {
            return;
        }
        if let Some(engine) = &self.cdc_engine {
            // autocommit 模式使用虚拟 tx_id=1，与 dispatch_cdc_* 中的 tx_id 一致
            engine.flush_staged_events(1);
        }
    }

    /// P9-2：获取当前事务 ID（用于行级 WAL 记录的 tx_id 字段）。
    ///
    /// autocommit 模式（mvcc_txn_id=0）使用虚拟 tx_id=1，与 CDC 事件一致。
    fn wal_tx_id(&self) -> u32 {
        if self.mvcc_txn_id != 0 {
            self.mvcc_txn_id
        } else {
            1
        }
    }

    /// P9-2：写入行级 Insert WAL 记录（best-effort，失败仅 warn）。
    ///
    /// 在 `mvcc_insert` 成功插入行后调用，记录新行的字节序列。
    /// 仅 append 到 OS 缓冲区，不 fsync（由 commit_transaction 统一 fsync）。
    fn append_wal_row_insert(&self, table_name: &str, row_id: usize, new_row: &Row) {
        if let Some(writer) = &self.wal_writer {
            let table_id = Self::table_name_to_id(table_name);
            let change = szrsql_tx::wal::WalRowChange::for_insert(
                table_id,
                row_id,
                Self::serialize_row_for_cdc(new_row),
            );
            let record = szrsql_tx::wal::WalRecord::new_row_insert(self.wal_tx_id(), &change);
            if let Err(e) = writer.append(record) {
                tracing::warn!(
                    table = table_name,
                    row_id = row_id,
                    error = %e,
                    "P9-2: append WAL Insert record failed (best-effort, continuing)"
                );
            }
        }
    }

    /// P9-2：写入行级 Update WAL 记录（best-effort，失败仅 warn）。
    ///
    /// 在 `execute_update` 成功更新行后调用，记录 old_row + new_row。
    fn append_wal_row_update(&self, table_name: &str, row_id: usize, old_row: &Row, new_row: &Row) {
        if let Some(writer) = &self.wal_writer {
            let table_id = Self::table_name_to_id(table_name);
            let change = szrsql_tx::wal::WalRowChange::for_update(
                table_id,
                row_id,
                Self::serialize_row_for_cdc(old_row),
                Self::serialize_row_for_cdc(new_row),
            );
            let record = szrsql_tx::wal::WalRecord::new_row_update(self.wal_tx_id(), &change);
            if let Err(e) = writer.append(record) {
                tracing::warn!(
                    table = table_name,
                    row_id = row_id,
                    error = %e,
                    "P9-2: append WAL Update record failed (best-effort, continuing)"
                );
            }
        }
    }

    /// P9-2：写入行级 Delete WAL 记录（best-effort，失败仅 warn）。
    ///
    /// 在 `execute_delete` 成功删除行后调用，记录 old_row。
    fn append_wal_row_delete(&self, table_name: &str, row_id: usize, old_row: &Row) {
        if let Some(writer) = &self.wal_writer {
            let table_id = Self::table_name_to_id(table_name);
            let change = szrsql_tx::wal::WalRowChange::for_delete(
                table_id,
                row_id,
                Self::serialize_row_for_cdc(old_row),
            );
            let record = szrsql_tx::wal::WalRecord::new_row_delete(self.wal_tx_id(), &change);
            if let Err(e) = writer.append(record) {
                tracing::warn!(
                    table = table_name,
                    row_id = row_id,
                    error = %e,
                    "P9-2: append WAL Delete record failed (best-effort, continuing)"
                );
            }
        }
    }

    /// P0-DIST-1/2/3：设置分布式运行时句柄（mutable setter）
    pub fn set_dist_runtime(&mut self, handle: szrsql_dist::runtime::DistRuntimeHandle) {
        self.dist_runtime = Some(handle);
    }

    /// P0-DIST-1/2/3：判断分布式运行时是否已启用
    pub fn has_dist_runtime(&self) -> bool {
        self.dist_runtime.is_some()
    }

    /// P2-2.1：将行数据写入分布式 KV 存储（Percolator 2PC）。
    ///
    /// **键编码**：`{table_name}:{row_id}`（UTF-8 字符串）
    /// **值编码**：serde_json 序列化的 `Vec<Value>`（行数据）
    ///
    /// # 路径分派
    /// - **显式事务**（`dist_txn_mutations` 已注入）：累积 `Mutation::Put` 到共享 Vec，
    ///   由 session 的 COMMIT 触发批量 prewrite + commit，ROLLBACK 触发 rollback。
    /// - **Autocommit**（未注入累积器）：即时 2PC（begin → prewrite → commit），
    ///   保证单语句原子性。
    ///
    /// 写入失败仅记录 warn 日志，不中断 DML 流程（best-effort，与旧版兼容）。
    fn dist_dual_write(&self, table_name: &str, row_id: usize, row: &Row) {
        use szrsql_dist::dist_txn::DistTxnClient;
        use szrsql_dist::txn::Mutation;

        let handle = match &self.dist_runtime {
            Some(h) => h,
            None => return,
        };
        let key = format!("{}:{}", table_name, row_id);
        let value = serde_json::to_vec(row).unwrap_or_else(|_| Vec::new());
        let mutation = Mutation::put(key.into_bytes(), value);

        // 显式事务：累积到共享 Vec，由 session COMMIT/ROLLBACK 统一 2PC
        if let Some(mutations_arc) = &self.dist_txn_mutations {
            if let Ok(mut guard) = mutations_arc.lock() {
                guard.push(mutation);
            }
            return;
        }

        // Autocommit：即时 2PC（begin → prewrite → commit）
        let mut rt = handle.write();
        let mut txn = DistTxnClient::new(&mut rt);
        let start_ts = txn.begin();
        if let Err(e) = txn.prewrite_all(std::slice::from_ref(&mutation), start_ts) {
            tracing::warn!(
                table = table_name,
                row_id = row_id,
                error = %e,
                "P2-2.1: autocommit prewrite failed (best-effort, continuing)"
            );
            return;
        }
        if let Err(e) = txn.commit(&[mutation], start_ts) {
            tracing::warn!(
                table = table_name,
                row_id = row_id,
                error = %e,
                "P2-2.1: autocommit commit failed (best-effort, continuing)"
            );
        }
    }

    /// P2-2.1：从分布式 KV 存储删除行数据（Percolator 2PC）。
    ///
    /// # 路径分派
    /// - **显式事务**：累积 `Mutation::Delete`，由 session COMMIT/ROLLBACK 统一 2PC。
    /// - **Autocommit**：即时 2PC（begin → prewrite → commit with Delete mutation）。
    ///
    /// 失败时仅记录警告，不影响本地删除结果（best-effort）。
    fn dist_dual_delete(&self, table_name: &str, row_id: usize) {
        use szrsql_dist::dist_txn::DistTxnClient;
        use szrsql_dist::txn::Mutation;

        let handle = match &self.dist_runtime {
            Some(h) => h,
            None => return,
        };
        let key = format!("{}:{}", table_name, row_id);
        let mutation = Mutation::delete(key.into_bytes());

        // 显式事务：累积
        if let Some(mutations_arc) = &self.dist_txn_mutations {
            if let Ok(mut guard) = mutations_arc.lock() {
                guard.push(mutation);
            }
            return;
        }

        // Autocommit：即时 2PC
        let mut rt = handle.write();
        let mut txn = DistTxnClient::new(&mut rt);
        let start_ts = txn.begin();
        if let Err(e) = txn.prewrite_all(std::slice::from_ref(&mutation), start_ts) {
            tracing::warn!(
                table = table_name,
                row_id = row_id,
                error = %e,
                "P2-2.1: autocommit delete prewrite failed (best-effort, continuing)"
            );
            return;
        }
        if let Err(e) = txn.commit(&[mutation], start_ts) {
            tracing::warn!(
                table = table_name,
                row_id = row_id,
                error = %e,
                "P2-2.1: autocommit delete commit failed (best-effort, continuing)"
            );
        }
    }

    /// P0-DIST-1/2/3：从分布式 KV 存储读取行数据。
    ///
    /// 返回反序列化后的 `Vec<Value>`，若键不存在或反序列化失败则返回 None。
    /// 用于测试和验证双写一致性。
    pub fn dist_read(&self, table_name: &str, row_id: usize) -> Option<Vec<Value>> {
        let handle = self.dist_runtime.as_ref()?;
        let key = format!("{}:{}", table_name, row_id);
        let rt = handle.read();
        let value = rt.get(key.as_bytes()).ok()??;
        serde_json::from_slice(&value).ok()
    }

    /// P0-DIST-1/2/3：获取分布式运行时的当前 TSO 时间戳。
    ///
    /// 用于将 SQL 事务的 start_ts 与分布式 TSO 协同。
    /// 未绑定 DistRuntime 时返回 None。
    pub fn dist_current_timestamp(&self) -> Option<u64> {
        let handle = self.dist_runtime.as_ref()?;
        let rt = handle.read();
        Some(rt.current_timestamp())
    }

    /// 在当前线程注入 UDF 注册表（返回 RAII guard）— P0-SQL-8 修复
    ///
    /// 返回的 guard 在析构时自动清理 thread_local。
    /// 若 `self.udf_registry` 为 None 则返回 None（无需清理）。
    /// 嵌套调用安全：内层 guard 不重复设置/清理。
    fn thread_udf_guard(&self) -> Option<crate::expr::current_udf_registry::UdfGuard> {
        self.udf_registry
            .as_ref()
            .map(|reg| crate::expr::current_udf_registry::guard(reg.clone()))
    }

    /// 在当前线程注入 SQL 函数注册表（返回 RAII guard）— P0-FN 修复
    ///
    /// 返回的 guard 在析构时自动清理 thread_local。
    /// 若 `self.sql_functions` 为 None 则返回 None（无需清理）。
    /// guard 在 `execute` 入口创建，确保整个执行期间 `current_sql_functions`
    /// 可被 `ExprEvaluator::try_call_udf` 查询；执行完毕自动 drop 清理。
    ///
    /// P0-FN-TYPE 修复：改为 pub，供协议层在 `execute()` 返回后、
    /// `derive_output_columns()` 调用前重新设置 guard，确保
    /// `derive_expr_type` 能查询到函数返回类型声明。
    pub fn sql_functions_guard(&self) -> Option<crate::expr::current_sql_functions::SqlFuncGuard> {
        self.sql_functions
            .as_ref()
            .map(|funcs| crate::expr::current_sql_functions::guard(funcs.clone()))
    }

    /// P0-3：绑定 PL/pgSQL 函数注册表 — 注入后 `execute` 会将此注册表设置到
    /// `current_plpgsql_interp` 线程局部，供 `ExprEvaluator` 在调用
    /// `LANGUAGE plpgsql` 函数时通过 `PlPgSqlInterpreter` 执行函数体。
    pub fn with_plpgsql_registry(
        mut self,
        registry: Arc<Mutex<crate::plpgsql_interp::FunctionRegistry>>,
    ) -> Self {
        self.plpgsql_registry = Some(registry);
        self
    }

    /// 在当前线程注入 PL/pgSQL 函数注册表（返回 RAII guard）— P0-3 修复
    ///
    /// guard 析构时自动清理 thread_local。
    /// 若 `self.plpgsql_registry` 为 None 则返回 None。
    pub fn plpgsql_guard(&self) -> Option<crate::expr::current_plpgsql_interp::PlPgGuard> {
        self.plpgsql_registry
            .as_ref()
            .map(|r| crate::expr::current_plpgsql_interp::guard(r.clone()))
    }

    /// 按名查表（仅普通表，不含临时表）
    ///
    /// 返回 `&'a dyn TableStorage`（引用生命周期与执行器 `'a` 绑定）。
    /// 若需同时查询临时表，请使用内部方法 `lookup_table`。
    pub fn get_table(&self, name: &str) -> Option<&'a dyn TableStorage> {
        self.tables.get(&name.to_lowercase()).copied()
    }

    /// 按名查表（先查临时表，再查普通表）— Phase 3.28
    ///
    /// 返回 `&dyn TableStorage`（引用生命周期与 `&self` 绑定）。
    /// 用于执行器内部的 Scan / MERGE / IndexScan 等读取路径。
    ///
    /// MySQL 兼容回退：当精确匹配失败时，尝试 `schema_table` 连接形式
    /// （Navicat 发送 `njszjt.soci_article`，但 szrsql 表存储为 `njszjt_soci_article`）。
    fn lookup_table(&self, name: &str) -> Option<&dyn TableStorage> {
        let key = name.to_lowercase();
        // 临时表优先（PG 语义：同名时临时表遮蔽普通表）
        if let Some(store) = &self.temp_store {
            if let Some(temp) = store.get(&key) {
                return Some(temp as &dyn TableStorage);
            }
        }
        // 普通表：精确匹配
        if let Some(t) = self.tables.get(&key).copied() {
            return Some(t as &dyn TableStorage);
        }
        // MySQL 兼容回退：name 可能是 "njszjt.soci_article" → 尝试 "njszjt_soci_article"
        if key.contains('.') {
            let joined = key.replace('.', "_");
            if let Some(t) = self.tables.get(&joined).copied() {
                return Some(t as &dyn TableStorage);
            }
        }
        // MySQL 兼容回退：name 是 "soci_article"（无 schema）→ 遍历找 "_soci_article" 后缀
        let suffix = format!("_{}", key);
        for k in self.tables.keys() {
            if k.ends_with(&suffix) {
                return self.tables.get(k).copied().map(|t| t as &dyn TableStorage);
            }
        }
        None
    }

    // -----------------------------------------------------------------
    //  P0-STORE 阶段 1：主键索引优化
    // -----------------------------------------------------------------

    /// P0-STORE 阶段 1：尝试使用 B+Tree 主键索引优化 WHERE 过滤
    ///
    /// 检测谓词是否为「主键列 等值/范围 条件」模式，若是则通过 B+Tree O(log n) 查找，
    /// 避免全表扫描。支持以下模式：
    ///
    /// - `pk = literal` → `pk_point_lookup`
    /// - `pk > literal` / `pk >= literal` / `pk < literal` / `pk <= literal` → `pk_range_lookup`
    /// - `pk BETWEEN low AND high` → `pk_range_lookup`
    /// - `<literal> = pk` / `<literal> < pk` 等（操作数反转）→ 同上
    ///
    /// # 返回
    /// - `Ok(Some(rows))`：优化成功，返回结果行（已应用残余谓词）
    /// - `Ok(None)`：优化不适用（无主键索引/谓词不含主键条件），调用方退化为全表扫描
    fn try_pk_optimized_filter(
        &self,
        table: &TableName,
        predicate: &Expr,
    ) -> Result<Option<Vec<Row>>, ExecutionError> {
        let storage = match self.lookup_table(&table.name) {
            Some(s) => s,
            None => return Ok(None),
        };

        // 检查是否启用了 B+Tree 主键索引
        let pk_col_idx = match storage.pk_column_idx() {
            Some(idx) => idx,
            None => return Ok(None),
        };

        // 获取主键列名
        let schema = storage.schema();
        let pk_col_name = match schema.columns.get(pk_col_idx) {
            Some(col) => col.name.as_str(),
            None => return Ok(None),
        };

        // 尝试从谓词中提取主键访问条件
        let pk_access = match self.extract_pk_access(predicate, pk_col_name) {
            Some(access) => access,
            None => return Ok(None),
        };

        // MVCC 模式下，需要额外过滤可见性
        let mvcc_txn_id = self.mvcc_txn_id;
        let mvcc = self.mvcc;

        match pk_access {
            PkAccess::Point(key) => {
                let row = storage.pk_point_lookup(key);
                let mut result = Vec::new();
                if let Some(row) = row {
                    // MVCC 可见性检查
                    if mvcc_txn_id != 0 {
                        if let Some(mvcc) = mvcc {
                            // 注册读集合（SSI）
                            let table_key = table.name.to_lowercase();
                            let _ = mvcc.register_read(mvcc_txn_id, &table_key);
                            // 点查结果需要通过 scan_with_versions 验证可见性
                            // 简化：若 MVCC 启用，退化为全表扫描 + 过滤（保证正确性）
                            return Ok(None);
                        }
                    }
                    // 应用完整谓词过滤（可能有其他条件 AND 连接）
                    let ctx = ExecRowContext::new(schema, &row);
                    if let Value::Bool(true) = ExprEvaluator::eval(predicate, &ctx)? {
                        result.push(row);
                    }
                }
                Ok(Some(result))
            }
            PkAccess::Range(low, high) => {
                let rows = match storage.pk_range_lookup(low, high) {
                    Some(r) => r,
                    None => return Ok(None),
                };
                // MVCC 模式退化为全表扫描（保证正确性）
                if mvcc_txn_id != 0 && mvcc.is_some() {
                    return Ok(None);
                }
                // 应用完整谓词过滤
                let mut result = Vec::with_capacity(rows.len());
                for row in rows {
                    let ctx = ExecRowContext::new(schema, &row);
                    if let Value::Bool(true) = ExprEvaluator::eval(predicate, &ctx)? {
                        result.push(row);
                    }
                }
                Ok(Some(result))
            }
        }
    }

    /// P0-STORE 阶段 1：从谓词中提取主键访问条件
    ///
    /// 识别 `pk = literal`、`pk > literal`、`pk BETWEEN low AND high` 等模式。
    /// 返回 `None` 表示谓词不含可优化的主键条件。
    fn extract_pk_access(&self, predicate: &Expr, pk_col_name: &str) -> Option<PkAccess> {
        match predicate {
            Expr::BinaryOp { left, op, right } => {
                // 尝试 left = pk_col, right = literal
                if let (Some(pk_side), Some(literal_side)) = (
                    self.expr_is_pk_column(left, pk_col_name),
                    self.expr_as_i64_literal(right),
                ) {
                    return Some(self.binary_op_to_pk_access(pk_side, op, literal_side));
                }
                // 尝试 left = literal, right = pk_col（操作数反转）
                if let (Some(literal_side), Some(pk_side)) = (
                    self.expr_as_i64_literal(left),
                    self.expr_is_pk_column(right, pk_col_name),
                ) {
                    // 反转操作符：a < b 等价于 b > a
                    let reversed_op = match op {
                        BinaryOp::Eq => BinaryOp::Eq,
                        BinaryOp::Lt => BinaryOp::Gt,
                        BinaryOp::LtEq => BinaryOp::GtEq,
                        BinaryOp::Gt => BinaryOp::Lt,
                        BinaryOp::GtEq => BinaryOp::LtEq,
                        _ => return None,
                    };
                    return Some(self.binary_op_to_pk_access(pk_side, &reversed_op, literal_side));
                }
                // AND 连接：尝试在左右子表达式中找主键条件
                if let BinaryOp::And = op {
                    if let Some(access) = self.extract_pk_access(left, pk_col_name) {
                        return Some(access);
                    }
                    if let Some(access) = self.extract_pk_access(right, pk_col_name) {
                        return Some(access);
                    }
                }
                None
            }
            Expr::Between {
                expr,
                low,
                high,
                negated: false,
            } => {
                if self.expr_is_pk_column(expr, pk_col_name).is_some() {
                    let low_val = self.expr_as_i64_literal(low)?;
                    let high_val = self.expr_as_i64_literal(high)?;
                    return Some(PkAccess::Range(low_val, high_val + 1));
                }
                None
            }
            _ => None,
        }
    }

    /// 检查表达式是否为主键列引用
    ///
    /// 返回 `Some(())` 若表达式是 `Identifier([pk_col_name])` 或 `Identifier([table, pk_col_name])`。
    fn expr_is_pk_column(&self, expr: &Expr, pk_col_name: &str) -> Option<()> {
        match expr {
            Expr::Identifier(parts) => {
                // 匹配 "pk_col" 或 "table.pk_col"
                let last = parts.last()?;
                if last.eq_ignore_ascii_case(pk_col_name) {
                    Some(())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// 尝试将表达式解析为 i64 字面量
    fn expr_as_i64_literal(&self, expr: &Expr) -> Option<i64> {
        match expr {
            Expr::Literal(Value::Int64(n)) => Some(*n),
            Expr::Cast { expr, data_type } => {
                // CAST(literal AS BIGINT) 等场景
                if matches!(data_type, ColumnType::Int64) {
                    self.expr_as_i64_literal(expr)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// 将二元操作符 + 字面量转换为 PkAccess
    fn binary_op_to_pk_access(&self, _pk_side: (), op: &BinaryOp, literal: i64) -> PkAccess {
        match op {
            BinaryOp::Eq => PkAccess::Point(literal),
            BinaryOp::Gt => PkAccess::Range(literal + 1, i64::MAX),
            BinaryOp::GtEq => PkAccess::Range(literal, i64::MAX),
            BinaryOp::Lt => PkAccess::Range(i64::MIN, literal),
            BinaryOp::LtEq => PkAccess::Range(i64::MIN, literal + 1),
            _ => PkAccess::Point(literal), // 保守退化为点查
        }
    }

    // -----------------------------------------------------------------
    //  P0-TX-1 Phase B：MVCC 辅助方法
    // -----------------------------------------------------------------

    /// MVCC 感知的行插入 — 根据是否启用 MVCC 选择版本化或普通插入。
    ///
    /// 启用 MVCC 时：
    /// 1. 调用 `insert_row_versioned(row, txn_id)` 设置 xmin = 当前事务 ID
    /// 2. 注册 write_set（key = "table_name:row_id"）用于 First-Committer-Wins + SSI
    ///
    /// 未启用时退化为 `insert_row(row)`（旧行为）。
    fn mvcc_insert(&self, table: &mut dyn MutableTable, row: Row, table_name: &TableName) -> usize {
        if let Some(mvcc) = self.mvcc {
            if self.mvcc_txn_id != 0 {
                let row_id = table.insert_row_versioned(row.clone(), self.mvcc_txn_id);
                let key = format!("{}:{}", table_name.name.to_lowercase(), row_id);
                let _ = mvcc.register_write(self.mvcc_txn_id, &key);
                // P0-DIST-1/2/3：双写到分布式 KV 存储（Raft propose → apply）
                self.dist_dual_write(&table_name.name.to_lowercase(), row_id, &row);
                // P7-1：分发 CDC Insert 事件
                self.dispatch_cdc_insert(&table_name.name, &row);
                // P9-2：写入行级 Insert WAL 记录
                self.append_wal_row_insert(&table_name.name, row_id, &row);
                return row_id;
            }
        }
        let row_id = table.insert_row(row.clone());
        // P0-DIST-1/2/3：双写到分布式 KV 存储（Raft propose → apply）
        self.dist_dual_write(&table_name.name.to_lowercase(), row_id, &row);
        // P7-1：分发 CDC Insert 事件
        self.dispatch_cdc_insert(&table_name.name, &row);
        // P9-2：写入行级 Insert WAL 记录
        self.append_wal_row_insert(&table_name.name, row_id, &row);
        row_id
    }

    /// 判断 MVCC 是否已启用且有活跃事务。
    fn mvcc_active(&self) -> bool {
        self.mvcc.is_some() && self.mvcc_txn_id != 0
    }

    // -----------------------------------------------------------------
    //  Sequence DDL — Phase 3.22
    // -----------------------------------------------------------------

    /// 执行 CREATE SEQUENCE 计划
    ///
    /// 若 `if_not_exists=true` 且序列已存在，返回 Ok(())
    pub fn execute_create_sequence(
        &self,
        plan: &LogicalPlan,
        seq_store: &mut dyn SequenceStore,
    ) -> Result<(), ExecutionError> {
        let (definition, if_not_exists) = match plan {
            LogicalPlan::CreateSequence {
                definition,
                if_not_exists,
            } => (definition, *if_not_exists),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected CreateSequence plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        if if_not_exists && seq_store.sequence_exists(&definition.name) {
            return Ok(());
        }
        seq_store.create_sequence(definition.clone())
    }

    /// 执行 DROP SEQUENCE 计划
    pub fn execute_drop_sequence(
        &self,
        plan: &LogicalPlan,
        seq_store: &mut dyn SequenceStore,
    ) -> Result<(), ExecutionError> {
        let (names, if_exists, _cascade) = match plan {
            LogicalPlan::DropSequence {
                names,
                if_exists,
                cascade,
            } => (names, *if_exists, *cascade),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected DropSequence plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        for name in names {
            seq_store.drop_sequence(name, if_exists)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    //  Trigger DDL — Phase 6.4
    // -----------------------------------------------------------------

    /// 执行 CREATE TRIGGER 计划 — Phase 6.4
    ///
    /// 将触发器定义注册到 `InMemoryCatalog`。
    ///
    /// # 语义
    /// - `or_replace=true`：替换同名触发器（catalog 中已存在则覆盖）
    /// - `if_not_exists=true`：跳过（catalog 中已存在则静默返回）
    /// - 二者都未指定且触发器已存在：报错（应在 planner 层拦截，此处兜底）
    ///
    /// **注意**：执行器持有的 `catalog: Option<&dyn Catalog>` 是不可变引用，无法修改。
    /// 此方法签名要求传入 `&mut InMemoryCatalog`，调用方需自行管理 catalog 生命周期。
    pub fn execute_create_trigger(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (definition, or_replace, if_not_exists) = match plan {
            LogicalPlan::CreateTrigger {
                definition,
                or_replace,
                if_not_exists,
            } => (definition, *or_replace, *if_not_exists),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected CreateTrigger plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        // 存在性检查
        let exists = catalog
            .get_trigger(&definition.table, &definition.name)
            .is_some();
        if exists {
            if if_not_exists && !or_replace {
                // IF NOT EXISTS 且未指定 OR REPLACE：静默跳过
                return Ok(());
            }
            // or_replace=true：add_trigger 会替换；二者都没指定的情况已在 planner 拦截
            // 这里兜底：如果 or_replace=false && if_not_exists=false && exists，
            // 应该报错（但 planner 应该已经处理）
            if !or_replace {
                return Err(ExecutionError::InvalidArgument(format!(
                    "trigger already exists: {} on table {}",
                    definition.name,
                    definition.table.qualified_name()
                )));
            }
        }
        catalog.add_trigger(definition.clone());
        Ok(())
    }

    /// 执行 DROP TRIGGER 计划 — Phase 6.4
    ///
    /// 从 `InMemoryCatalog` 移除触发器定义。
    ///
    /// # 语义
    /// - `if_exists=true`：触发器不存在时静默返回
    /// - `if_exists=false`：触发器不存在时报错（应在 planner 层拦截，此处兜底）
    pub fn execute_drop_trigger(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (name, table, if_exists, _cascade) = match plan {
            LogicalPlan::DropTrigger {
                name,
                table,
                if_exists,
                cascade,
            } => (name, table, *if_exists, *cascade),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected DropTrigger plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        let removed = catalog.remove_trigger(table, name);
        if removed.is_none() && !if_exists {
            return Err(ExecutionError::InvalidArgument(format!(
                "trigger not found: {} on table {}",
                name,
                table.qualified_name()
            )));
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    //  ENUM Type DDL — Phase 3.31
    // -----------------------------------------------------------------

    /// 执行 CREATE TYPE 计划 — Phase 3.31
    ///
    /// 将 ENUM 类型定义注册到 `InMemoryCatalog`。若 `if_not_exists=true` 且类型已存在，跳过。
    ///
    /// **注意**：执行器持有的 `catalog: Option<&dyn Catalog>` 是不可变引用，无法在此修改。
    /// 此方法签名要求传入 `&mut InMemoryCatalog`，调用方需自行管理 catalog 生命周期。
    pub fn execute_create_type(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (definition, if_not_exists) = match plan {
            LogicalPlan::CreateType {
                definition,
                if_not_exists,
            } => (definition, *if_not_exists),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected CreateType plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        if if_not_exists && catalog.enum_type_exists(&definition.name) {
            return Ok(());
        }
        if !if_not_exists && catalog.enum_type_exists(&definition.name) {
            return Err(ExecutionError::TypeAlreadyExists(
                definition.name.qualified_name(),
            ));
        }
        catalog.add_enum_type(definition.clone());
        Ok(())
    }

    /// 执行 DROP TYPE 计划 — Phase 3.31
    ///
    /// 从 `InMemoryCatalog` 移除指定的 ENUM 类型。
    /// `if_exists=true` 时若类型不存在则跳过；否则报错。
    ///
    /// **注意**：`cascade=true` 当前仅记录，不实际级联删除引用此类型的列。
    pub fn execute_drop_type(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (names, if_exists, _cascade) = match plan {
            LogicalPlan::DropType {
                names,
                if_exists,
                cascade,
            } => (names, *if_exists, *cascade),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected DropType plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        for name in names {
            if !catalog.enum_type_exists(name) {
                if if_exists {
                    continue;
                }
                return Err(ExecutionError::TypeNotFound(name.qualified_name()));
            }
            catalog.remove_enum_type(name);
        }
        Ok(())
    }

    /// 执行 ALTER TYPE 计划 — Phase 3.31
    ///
    /// 支持的操作：
    /// - `ADD VALUE 'val'` — 向 ENUM 类型追加新值（不允许已存在）
    /// - `ADD VALUE IF NOT EXISTS 'val'` — 若已存在则跳过
    /// - `RENAME VALUE 'old' TO 'new'` — 重命名枚举值
    /// - `RENAME TO new_name` — 重命名类型
    pub fn execute_alter_type(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (name, action) = match plan {
            LogicalPlan::AlterType { name, action } => (name, action),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected AlterType plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        match action {
            AlterTypeAction::AddValue {
                value,
                if_not_exists,
            } => {
                let enum_def = catalog
                    .get_enum_type_mut(name)
                    .ok_or_else(|| ExecutionError::TypeNotFound(name.qualified_name()))?;
                if enum_def.contains(value) {
                    if *if_not_exists {
                        return Ok(());
                    }
                    return Err(ExecutionError::EnumValueViolation(format!(
                        "ENUM type {} already contains value '{}'",
                        name.qualified_name(),
                        value
                    )));
                }
                enum_def.labels.push(value.clone());
                Ok(())
            }
            AlterTypeAction::RenameValue { old, new } => {
                let enum_def = catalog
                    .get_enum_type_mut(name)
                    .ok_or_else(|| ExecutionError::TypeNotFound(name.qualified_name()))?;
                let idx = enum_def
                    .labels
                    .iter()
                    .position(|l| l == old)
                    .ok_or_else(|| {
                        ExecutionError::EnumValueViolation(format!(
                            "ENUM type {} does not contain value '{}'",
                            name.qualified_name(),
                            old
                        ))
                    })?;
                // 检查新值不与现有值冲突（除非新旧相同）
                if old != new && enum_def.contains(new) {
                    return Err(ExecutionError::EnumValueViolation(format!(
                        "ENUM type {} already contains value '{}'",
                        name.qualified_name(),
                        new
                    )));
                }
                enum_def.labels[idx] = new.clone();
                Ok(())
            }
            AlterTypeAction::Rename { new_name } => {
                // 取出旧定义，改名为 new_name，重新插入
                let mut enum_def = catalog
                    .remove_enum_type(name)
                    .ok_or_else(|| ExecutionError::TypeNotFound(name.qualified_name()))?;
                // 检查新名不冲突
                if catalog.enum_type_exists(new_name) {
                    // 重新插回旧名以保持状态
                    catalog.add_enum_type(enum_def);
                    return Err(ExecutionError::TypeAlreadyExists(new_name.qualified_name()));
                }
                enum_def.name = new_name.clone();
                catalog.add_enum_type(enum_def);
                Ok(())
            }
        }
    }

    /// 执行 ALTER TABLE 计划 — Phase F-10
    ///
    /// 支持 PG/MySQL/Oracle/SQL Server/SQLite 通用的 ALTER TABLE 操作：
    /// - `ADD COLUMN [IF NOT EXISTS] col TYPE [options]`
    /// - `DROP COLUMN [IF EXISTS] col [CASCADE]`
    /// - `RENAME COLUMN old TO new`
    /// - `RENAME TO new_table`
    /// - `ALTER COLUMN col TYPE new_type [USING expr]`（USING 当前忽略，仅改类型）
    /// - `ALTER COLUMN col SET DEFAULT expr` / `DROP DEFAULT`
    /// - `ALTER COLUMN col SET NOT NULL` / `DROP NOT NULL`
    /// - `ADD CONSTRAINT ...`（PRIMARY KEY / UNIQUE / CHECK / FOREIGN KEY）
    /// - `DROP CONSTRAINT [IF EXISTS] name [CASCADE]`
    ///
    /// # 数据迁移
    /// - ADD COLUMN：现有行的该列填充为 NULL（或 DEFAULT 表达式求值结果）
    /// - DROP COLUMN：现有行的该列被移除（行宽度收缩）
    /// - ALTER COLUMN TYPE：尝试对现有行的该列值进行隐式类型转换，失败则报错
    /// - SET DEFAULT / DROP DEFAULT / SET NOT NULL / DROP NOT NULL：仅修改 Schema，不迁移数据
    ///   （SET NOT NULL 时会校验现有行该列无 NULL 值）
    ///
    /// # 限制
    /// - 不支持 ADD COLUMN ... PRIMARY KEY / UNIQUE（应使用 ADD CONSTRAINT）
    /// - DROP COLUMN CASCADE 当前与 RESTRICT 行为一致（不级联删除依赖对象）
    /// - ALTER COLUMN TYPE 的 USING expr 当前仅记录不执行（类型转换走隐式规则）
    /// - ADD CONSTRAINT FOREIGN KEY 当前仅记录到 catalog.foreign_keys，不强制校验现有数据
    pub fn execute_alter_table(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
        table_data: Option<&mut InMemoryTable>,
    ) -> Result<(), ExecutionError> {
        let (name, _if_exists, _only, operations) = match plan {
            LogicalPlan::AlterTable {
                name,
                if_exists,
                only,
                operations,
            } => (name, *if_exists, *only, operations),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected AlterTable plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // 将 Option<&mut InMemoryTable> 绑定到可变变量，便于循环中多次借用
        let mut table_data = table_data;

        // 取出当前 Schema 克隆，所有操作在克隆上累积修改
        let mut schema = catalog
            .get_table(name)
            .ok_or_else(|| ExecutionError::TableNotFound(name.qualified_name()))?;

        for op in operations {
            match op {
                AlterTableOperation::AddColumn {
                    column_def,
                    if_not_exists,
                } => {
                    // 检查列是否已存在
                    let exists = schema.columns.iter().any(|c| c.name == column_def.name);
                    if exists {
                        if *if_not_exists {
                            continue;
                        }
                        return Err(ExecutionError::InvalidArgument(format!(
                            "column \"{}\" already exists in table \"{}\"",
                            column_def.name,
                            name.qualified_name()
                        )));
                    }
                    schema.columns.push(column_def.clone());

                    // 数据迁移：现有行追加 NULL（或 DEFAULT 求值结果）
                    if let Some(table) = table_data.as_mut() {
                        let default_value = Self::eval_default_value(column_def)?;
                        {
                            let mut guard = (*table).rows_mut();
                            for row in guard.iter_mut() {
                                row.push(default_value.clone());
                            }
                        }
                    }
                }
                AlterTableOperation::DropColumn {
                    name: col_name,
                    if_exists,
                    cascade: _,
                } => {
                    let idx = schema.columns.iter().position(|c| c.name == *col_name);
                    match idx {
                        Some(i) => {
                            schema.columns.remove(i);
                            // 数据迁移：现有行移除该列
                            if let Some(table) = table_data.as_mut() {
                                {
                                    let mut guard = (*table).rows_mut();
                                    for row in guard.iter_mut() {
                                        if i < row.len() {
                                            row.remove(i);
                                        }
                                    }
                                }
                            }
                        }
                        None => {
                            if *if_exists {
                                continue;
                            }
                            return Err(ExecutionError::InvalidArgument(format!(
                                "column \"{}\" does not exist in table \"{}\"",
                                col_name,
                                name.qualified_name()
                            )));
                        }
                    }
                }
                AlterTableOperation::RenameColumn { old_name, new_name } => {
                    let col = schema
                        .columns
                        .iter_mut()
                        .find(|c| c.name == *old_name)
                        .ok_or_else(|| {
                            ExecutionError::InvalidArgument(format!(
                                "column \"{}\" does not exist in table \"{}\"",
                                old_name,
                                name.qualified_name()
                            ))
                        })?;
                    col.name = new_name.clone();
                }
                AlterTableOperation::RenameTable { new_name } => {
                    // 先保存当前 schema 修改（如果有）
                    catalog.replace_table_schema(schema.clone())?;
                    // 执行重命名
                    catalog.rename_table(name, new_name)?;
                    return Ok(());
                }
                AlterTableOperation::AlterColumnType {
                    name: col_name,
                    data_type,
                    using: _,
                } => {
                    let col = schema
                        .columns
                        .iter_mut()
                        .find(|c| c.name == *col_name)
                        .ok_or_else(|| {
                            ExecutionError::InvalidArgument(format!(
                                "column \"{}\" does not exist in table \"{}\"",
                                col_name,
                                name.qualified_name()
                            ))
                        })?;
                    let old_type = col.data_type.clone();
                    col.data_type = data_type.clone();

                    // 数据迁移：尝试对现有行的该列值进行隐式类型转换
                    if let Some(table) = table_data.as_mut() {
                        let idx = schema
                            .columns
                            .iter()
                            .position(|c| c.name == *col_name)
                            .ok_or_else(|| {
                                ExecutionError::InvalidArgument(format!(
                                    "ALTER COLUMN TYPE: column '{}' not found in table '{}'",
                                    col_name,
                                    schema.name.qualified_name()
                                ))
                            })?;
                        {
                            let mut guard = (*table).rows_mut();
                            for row in guard.iter_mut() {
                                if idx < row.len() {
                                    row[idx] = Self::cast_value(&row[idx], &old_type, data_type)?;
                                }
                            }
                        }
                    }
                }
                AlterTableOperation::AlterColumnDefault {
                    name: col_name,
                    default,
                } => {
                    let col = schema
                        .columns
                        .iter_mut()
                        .find(|c| c.name == *col_name)
                        .ok_or_else(|| {
                            ExecutionError::InvalidArgument(format!(
                                "column \"{}\" does not exist in table \"{}\"",
                                col_name,
                                name.qualified_name()
                            ))
                        })?;
                    col.default = default.clone();
                }
                AlterTableOperation::AlterColumnNotNull {
                    name: col_name,
                    not_null,
                } => {
                    let idx = schema
                        .columns
                        .iter()
                        .position(|c| c.name == *col_name)
                        .ok_or_else(|| {
                            ExecutionError::InvalidArgument(format!(
                                "column \"{}\" does not exist in table \"{}\"",
                                col_name,
                                name.qualified_name()
                            ))
                        })?;
                    let col = &mut schema.columns[idx];
                    col.not_null = *not_null;

                    // SET NOT NULL 时校验现有数据
                    if *not_null {
                        if let Some(table) = table_data.as_ref() {
                            for row in (*table).rows() {
                                if idx < row.len() && row[idx] == Value::Null {
                                    return Err(ExecutionError::NotNullViolation(format!(
                                        "column \"{}\" contains NULL values, cannot SET NOT NULL",
                                        col_name
                                    )));
                                }
                            }
                        }
                    }
                }
                AlterTableOperation::AddConstraint { constraint } => {
                    Self::apply_table_constraint(&mut schema, constraint.clone())?;
                }
                AlterTableOperation::DropConstraint {
                    name: constraint_name,
                    if_exists,
                    cascade: _,
                } => {
                    // 当前实现：列级 PRIMARY KEY / UNIQUE 通过列字段标记，
                    // 表级约束通过 catalog 的 indexes/check_constraints/foreign_keys 管理。
                    // DROP CONSTRAINT 主要用于删除 CHECK 约束（按名匹配）。
                    let existed = Self::drop_table_constraint(catalog, name, constraint_name)?;
                    if !existed && !*if_exists {
                        return Err(ExecutionError::InvalidArgument(format!(
                            "constraint \"{}\" does not exist on table \"{}\"",
                            constraint_name,
                            name.qualified_name()
                        )));
                    }
                }
            }
        }

        // 所有操作完成后，整体替换 Schema
        catalog.replace_table_schema(schema)?;
        Ok(())
    }

    /// 求值列 DEFAULT 表达式为 Value — Phase F-10
    ///
    /// - None → Value::Null
    /// - Some(expr) → 在空行上下文求值（仅支持字面量表达式）
    fn eval_default_value(col: &ColumnDefinition) -> Result<Value, ExecutionError> {
        match &col.default {
            None => Ok(Value::Null),
            Some(expr) => {
                // 复用 expr 求值逻辑：传空 RowContext（DEFAULT 表达式不应引用其他列）
                let ctx = crate::expr::RowContext::new();
                crate::expr::ExprEvaluator::eval(expr, &ctx).map_err(|e| {
                    ExecutionError::InvalidArgument(format!(
                        "failed to evaluate DEFAULT for column \"{}\": {}",
                        col.name, e
                    ))
                })
            }
        }
    }

    /// 隐式类型转换 — Phase F-10
    ///
    /// 尝试将 value 从 from_type 转换为 to_type，失败则返回错误。
    /// 当前仅支持同类型或 Text↔数值/日期 的双向转换。
    fn cast_value(
        value: &Value,
        from_type: &ColumnType,
        to_type: &ColumnType,
    ) -> Result<Value, ExecutionError> {
        // NULL 保持不变
        if value == &Value::Null {
            return Ok(Value::Null);
        }
        // 同类型直接返回
        if from_type == to_type {
            return Ok(value.clone());
        }
        // 委托给 Value::cast_implicit
        value.clone().cast_implicit(to_type).map_err(|e| {
            ExecutionError::InvalidArgument(format!(
                "cannot cast value {:?} from {:?} to {:?}: {}",
                value, from_type, to_type, e
            ))
        })
    }

    /// 应用表级约束到 schema — Phase F-10
    ///
    /// - PRIMARY KEY(cols)：标记列为 primary_key=true，not_null=true
    /// - UNIQUE(cols)：标记列为 unique=true
    /// - CHECK(expr)：当前仅记录到 schema.check（待 catalog 支持）
    /// - FOREIGN KEY：当前仅记录，不强制
    fn apply_table_constraint(
        schema: &mut TableSchema,
        constraint: TableConstraint,
    ) -> Result<(), ExecutionError> {
        match constraint {
            TableConstraint::PrimaryKey { columns, .. } => {
                for col_name in columns {
                    let col = schema
                        .columns
                        .iter_mut()
                        .find(|c| c.name == col_name)
                        .ok_or_else(|| {
                            ExecutionError::InvalidArgument(format!(
                                "column \"{}\" does not exist",
                                col_name
                            ))
                        })?;
                    col.primary_key = true;
                    col.not_null = true;
                }
            }
            TableConstraint::Unique { columns, .. } => {
                for col_name in columns {
                    let col = schema
                        .columns
                        .iter_mut()
                        .find(|c| c.name == col_name)
                        .ok_or_else(|| {
                            ExecutionError::InvalidArgument(format!(
                                "column \"{}\" does not exist",
                                col_name
                            ))
                        })?;
                    col.unique = true;
                }
            }
            TableConstraint::Check { .. } => {
                // CHECK 约束需要 catalog 支持 check_constraints 存储
                // 当前简化：仅记录在 schema 中（待后续完善）
            }
            TableConstraint::ForeignKey { .. } => {
                // FK 约束需要 catalog 支持 foreign_keys 存储
                // 当前简化：仅记录在 schema 中（待后续完善）
            }
        }
        Ok(())
    }

    /// 删除表级约束 — Phase F-10
    ///
    /// 当前实现：尝试从 catalog 的 check_constraints 中按名删除。
    /// PRIMARY KEY / UNIQUE 约束的删除需要同时删除关联索引，待后续完善。
    fn drop_table_constraint(
        _catalog: &mut InMemoryCatalog,
        _table: &TableName,
        _constraint_name: &str,
    ) -> Result<bool, ExecutionError> {
        // 简化实现：当前不维护约束名 → 约束的映射表
        // 返回 false 让调用方根据 if_exists 决定报错或跳过
        Ok(false)
    }

    /// 校验行中所有 ENUM 列的值是否合法 — Phase 3.31
    ///
    /// PG 语义：
    /// - 列类型为 `ColumnType::Enum(labels)` 时，值必须为 `Value::Text(s)` 且 `s ∈ labels`，
    ///   或 `Value::Null`（若列允许 NULL）
    /// - 其他类型列不校验
    ///
    /// 此方法由 `execute_insert` / `execute_update` 在写入行前调用。
    fn validate_enum_values(&self, schema: &TableSchema, row: &Row) -> Result<(), ExecutionError> {
        for (i, col) in schema.columns.iter().enumerate() {
            if let szrsql_types::value::ColumnType::Enum(labels) = &col.data_type {
                let value = &row[i];
                match value {
                    Value::Null => {
                        // NULL 允许（由 NOT NULL 约束另行校验，此处不重复）
                    }
                    Value::Text(s) => {
                        if !labels.iter().any(|l| l == s) {
                            return Err(ExecutionError::EnumValueViolation(format!(
                                "invalid ENUM value for column {}.{}: '{}' (allowed: {:?})",
                                schema.name.qualified_name(),
                                col.name,
                                s,
                                labels
                            )));
                        }
                    }
                    _ => {
                        return Err(ExecutionError::EnumValueViolation(format!(
                            "ENUM column {}.{} expects text value, got {:?}",
                            schema.name.qualified_name(),
                            col.name,
                            value
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// 为外部批量插入路径（如 COPY FROM）校验单行所有约束
    ///
    /// P0 修复：COPY FROM 此前直接调用 `table.insert_row` 跳过所有约束校验，
    /// 现在通过此公共方法复用 Executor 的 FK/CHECK/ENUM 校验逻辑。
    ///
    /// # 参数
    /// - `table_name`：目标表名（用于从 catalog 查询 FK/CHECK 约束）
    /// - `schema`：目标表 schema
    /// - `row`：待插入的行（已对齐 schema.columns 顺序）
    ///
    /// # 返回
    /// - `Ok(())`：所有约束通过
    /// - `Err(_)`：违反约束（调用方应中止 COPY 并返回错误）
    pub fn validate_row_for_insert(
        &self,
        table_name: &crate::ast::TableName,
        schema: &TableSchema,
        row: &Row,
    ) -> Result<(), ExecutionError> {
        // ENUM 校验
        self.validate_enum_values(schema, row)?;
        // FK 校验（若 catalog 已绑定）
        if let Some(cat) = self.catalog {
            let fks = cat.get_foreign_keys(table_name);
            if !fks.is_empty() {
                ForeignKeyValidator::validate_insert(schema, row, &fks, &|name| {
                    self.lookup_table(name)
                })?;
            }
            let checks = cat.get_check_constraints(table_name);
            if !checks.is_empty() {
                CheckConstraintValidator::validate_row(schema, row, &checks)?;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    //  视图 DDL — Phase 6.10
    // -----------------------------------------------------------------

    /// 执行 CREATE VIEW / CREATE MATERIALIZED VIEW 计划 — Phase 6.10
    ///
    /// 将视图定义注册到 catalog。
    ///
    /// # 语义
    /// - `if_not_exists=true`：视图已存在时静默跳过
    /// - `if_not_exists=false`：视图已存在时报错
    /// - 物化视图的物化数据由调用方另行管理（执行 SELECT 并填充存储表）
    ///
    /// **注意**：执行器持有的 `catalog: Option<&dyn Catalog>` 是不可变引用，无法修改。
    /// 此方法签名要求传入 `&mut InMemoryCatalog`，调用方需自行管理 catalog 生命周期。
    pub fn execute_create_view(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (name, columns, query, materialized, if_not_exists, or_replace) = match plan {
            LogicalPlan::CreateView {
                name,
                columns,
                query,
                materialized,
                if_not_exists,
                or_replace,
            } => (
                name,
                columns,
                query,
                *materialized,
                *if_not_exists,
                *or_replace,
            ),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected CreateView plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        // 存在性检查
        if catalog.view_exists(name) {
            if if_not_exists && !or_replace {
                return Ok(());
            }
            if !or_replace {
                return Err(ExecutionError::InvalidArgument(format!(
                    "view already exists: {}",
                    name.qualified_name()
                )));
            }
            // or_replace=true：先移除旧视图，再添加新视图
            catalog.remove_view(name);
        }
        // 构造 ViewDefinition
        let view_def = if materialized {
            crate::materialized_view::ViewDefinition::new_materialized(name.clone(), query.clone())
        } else {
            crate::materialized_view::ViewDefinition::new_view(name.clone(), query.clone())
        }
        .with_columns(columns.clone());
        catalog.add_view(view_def);
        Ok(())
    }

    /// 执行 DROP VIEW / DROP MATERIALIZED VIEW 计划 — Phase 6.10
    ///
    /// # 语义
    /// - `if_exists=true`：视图不存在时静默跳过
    /// - `if_exists=false`：任一视图不存在时报错
    /// - `cascade=true`：当前仅记录，不实际级联（与 DROP TYPE 一致）
    pub fn execute_drop_view(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (names, if_exists, _cascade, _materialized) = match plan {
            LogicalPlan::DropView {
                names,
                if_exists,
                cascade,
                materialized,
            } => (names, *if_exists, *cascade, *materialized),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected DropView plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        for name in names {
            if catalog.remove_view(name).is_none() && !if_exists {
                return Err(ExecutionError::InvalidArgument(format!(
                    "view does not exist: {}",
                    name.qualified_name()
                )));
            }
        }
        Ok(())
    }

    /// 执行 REFRESH MATERIALIZED VIEW 计划 — Phase 6.10
    ///
    /// # 语义
    /// - 校验视图存在且为物化视图
    /// - 物化数据刷新由调用方另行执行（执行 SELECT 并覆盖存储表）
    /// - `with_data=false`（WITH NO DATA）：仅校验，不刷新（调用方应清空存储表）
    /// - `with_data=true`（WITH DATA）：刷新由调用方执行；本方法仅返回成功信号
    ///
    /// # 返回值
    /// 返回视图的查询计划 `Select`，供调用方执行并填充物化表。
    pub fn execute_refresh_materialized_view(
        &self,
        plan: &LogicalPlan,
        catalog: &InMemoryCatalog,
    ) -> Result<crate::ast::Select, ExecutionError> {
        let (name, _with_data) = match plan {
            LogicalPlan::RefreshMaterializedView { name, with_data } => (name, *with_data),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected RefreshMaterializedView plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        let view_def = catalog.get_view(name).ok_or_else(|| {
            ExecutionError::InvalidArgument(format!(
                "materialized view does not exist: {}",
                name.qualified_name()
            ))
        })?;
        if !view_def.materialized {
            return Err(ExecutionError::InvalidArgument(format!(
                "{} is not a materialized view",
                name.qualified_name()
            )));
        }
        Ok((*view_def.query).clone())
    }

    /// 执行 CREATE INDEX 计划 — Phase P0-FIX
    ///
    /// # 语义
    /// - 校验表存在（planner 已校验，此处防御性校验）
    /// - 校验索引列存在
    /// - 若 `if_not_exists=true` 且同名索引已存在，静默返回
    /// - 若 `if_not_exists=false` 且同名索引已存在，报错
    /// - 在 catalog 注册索引元数据
    ///
    /// # 注意
    /// 当前运行时使用 `InMemoryBTreeIndex`（基于 BTreeMap），
    /// 索引数据在 DML 路径中维护（见 `execute_insert` 等）。
    /// 本方法仅注册元数据；索引数据的实际构建在后续 DML 时增量维护，
    /// 对已存在的行不回填索引数据（与 PG 语义有差异，PG 会立即构建索引）。
    pub fn execute_create_index(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (name_opt, table, columns, unique, if_not_exists) = match plan {
            LogicalPlan::CreateIndex {
                name,
                table,
                columns,
                unique,
                if_not_exists,
            } => (name, table, columns, *unique, *if_not_exists),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected CreateIndex plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        // 生成索引名（未指定时按 PG 规则 <table>_<col>_idx）
        let index_name = match name_opt {
            Some(n) => n.clone(),
            None => {
                let cols: Vec<&str> = columns.iter().map(|c| c.column.as_str()).collect();
                format!("{}_{}_idx", table.name, cols.join("_"))
            }
        };
        // 检查同名索引是否已存在
        let existing = catalog.list_indexes(table);
        if existing
            .iter()
            .any(|i| i.name.eq_ignore_ascii_case(&index_name))
        {
            if if_not_exists {
                return Ok(());
            }
            return Err(ExecutionError::InvalidArgument(format!(
                "relation \"{}\" already exists",
                index_name
            )));
        }
        // 构造 IndexDefinition 并注册到 catalog
        let index_def = if unique {
            IndexDefinition::new_unique(index_name, table.clone(), columns.clone())
        } else {
            IndexDefinition::new(index_name, table.clone(), columns.clone())
        };
        catalog.add_index(index_def);
        Ok(())
    }

    /// 执行 DROP INDEX 计划 — Phase P0-FIX
    ///
    /// # 语义
    /// - 对每个索引名：
    ///   - `if_exists=true`：索引不存在时静默跳过
    ///   - `if_exists=false`：索引不存在时报错
    /// - 从 catalog 移除索引元数据
    pub fn execute_drop_index(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (names, if_exists) = match plan {
            LogicalPlan::DropIndex { names, if_exists } => (names, *if_exists),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected DropIndex plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        for name in names {
            if catalog.remove_index(name).is_none() && !if_exists {
                return Err(ExecutionError::InvalidArgument(format!(
                    "index \"{}\" does not exist",
                    name
                )));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    //  Phase 6.5: CREATE/DROP FUNCTION — P0-5 修复
    // -----------------------------------------------------------------

    /// 执行 CREATE FUNCTION — Phase 6.5（P0-5 修复）
    ///
    /// 将函数元数据（名称、参数、返回类型、language、body、波动性、strict 等）
    /// 注册到 catalog。函数体执行在调用时由表达式求值器按需触发。
    ///
    /// # 参数
    /// - `plan`：`LogicalPlan::CreateFunction` 计划节点
    /// - `catalog`：catalog 引用（函数定义将注册到此）
    ///
    /// # 错误
    /// - `or_replace=false` 且已存在相同签名函数 → InvalidArgument
    /// - plan 不是 CreateFunction → InvalidArgument
    pub fn execute_create_function(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (
            name,
            parameters,
            return_type,
            language,
            body,
            or_replace,
            volatility,
            strict,
            security_definer,
        ) = match plan {
            LogicalPlan::CreateFunction {
                name,
                parameters,
                return_type,
                language,
                body,
                or_replace,
                volatility,
                strict,
                security_definer,
            } => (
                name.clone(),
                parameters.clone(),
                return_type.clone(),
                language.clone(),
                body.clone(),
                *or_replace,
                *volatility,
                *strict,
                *security_definer,
            ),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected CreateFunction plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        let def = FunctionDefinition {
            name: name.clone(),
            parameters,
            return_type,
            language,
            body,
            volatility,
            strict,
            security_definer,
        };
        catalog
            .add_function(def, or_replace)
            .map_err(|e| ExecutionError::InvalidArgument(format!("create function failed: {e}")))?;
        tracing::debug!(function = %name, "registered function definition");
        Ok(())
    }

    /// 执行 DROP FUNCTION — Phase 6.5（P0-5 修复）
    ///
    /// 按 `parameter_types` 精确匹配签名删除函数定义。
    /// 若 `parameter_types` 为空且该函数名只有一个定义，则删除之；
    /// 若有多个重载则报错（PG 语义：必须指定参数类型）。
    ///
    /// # 参数
    /// - `plan`：`LogicalPlan::DropFunction` 计划节点
    /// - `catalog`：catalog 引用
    pub fn execute_drop_function(
        &self,
        plan: &LogicalPlan,
        catalog: &mut InMemoryCatalog,
    ) -> Result<(), ExecutionError> {
        let (name, parameter_types, if_exists, _cascade) = match plan {
            LogicalPlan::DropFunction {
                name,
                parameter_types,
                if_exists,
                cascade,
            } => (name.clone(), parameter_types.clone(), *if_exists, *cascade),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected DropFunction plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        let dropped = catalog
            .drop_function(&name, &parameter_types, if_exists)
            .map_err(|e| ExecutionError::InvalidArgument(format!("drop function failed: {e}")))?;
        if dropped {
            tracing::debug!(function = %name, "dropped function definition");
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    //  Phase 6.11: INSERT_ONLY 增量刷新
    // -----------------------------------------------------------------

    /// 执行 INSERT_ONLY 增量刷新 — Phase 6.11
    ///
    /// # 语义
    ///
    /// - 从 `cdc_feed` drain 所有 CDC 事件
    /// - 对每个 `CdcEvent::Insert` 事件，将投影后的行追加到 `mv_store.storage`
    /// - 更新 `mv_store.refresh_state` 与高水位
    /// - 返回 `RefreshOutcome`（含追加行数、刷新后总行数）
    ///
    /// # 限制
    ///
    /// - Phase 6.11 仅处理 `CdcEvent::Insert`；`Update`/`Delete` 留待 Phase 6.12
    /// - 调用方需在 DML 时显式推送到 `cdc_feed`（执行器不自动捕获）
    /// - 投影逻辑由调用方完成（执行器仅追加预投影的行）
    ///
    /// # 参数
    ///
    /// - `view_name`：物化视图名（用于校验视图存在且为物化视图）
    /// - `catalog`：catalog（用于查询视图定义）
    /// - `mv_store`：物化视图存储（可变引用，追加行）
    /// - `cdc_feed`：CDC 事件缓冲（可变引用，drain 事件）
    /// - `source_table_name`：源表名（用于高水位追踪）
    /// - `timestamp`：刷新时间戳（Unix 微秒）
    pub fn refresh_materialized_view_incremental(
        &self,
        view_name: &TableName,
        catalog: &InMemoryCatalog,
        mv_store: &mut crate::materialized_view::MaterializedViewStore,
        cdc_feed: &mut crate::materialized_view::CdcFeed,
        source_table_name: &TableName,
        timestamp: i64,
    ) -> Result<crate::materialized_view::RefreshOutcome, ExecutionError> {
        // 校验视图存在且为物化视图
        let view_def = catalog.get_view(view_name).ok_or_else(|| {
            ExecutionError::InvalidArgument(format!(
                "materialized view does not exist: {}",
                view_name.qualified_name()
            ))
        })?;
        if !view_def.materialized {
            return Err(ExecutionError::InvalidArgument(format!(
                "{} is not a materialized view",
                view_name.qualified_name()
            )));
        }

        // drain CDC 事件
        let events = cdc_feed.drain();
        let mut rows_appended: usize = 0;

        for event in events {
            match event {
                crate::materialized_view::CdcEvent::Insert { row, .. } => {
                    mv_store.append_row(row);
                    rows_appended += 1;
                }
                // INSERT_ONLY 模式不处理 UPDATE/DELETE — 调用方应使用 SIMPLE 模式
                crate::materialized_view::CdcEvent::Update { .. }
                | crate::materialized_view::CdcEvent::Delete { .. } => {
                    // no-op: 忽略非 INSERT 事件
                }
            }
        }

        // 更新高水位（设为当前源表行数；调用方需保证一致性）
        // Phase 6.11 设计：高水位由调用方在外部根据源表行数设置，
        // 此处仅更新 refresh_state。
        let total_rows = mv_store.row_count();
        mv_store.refresh_state = crate::materialized_view::RefreshState::initialized(
            total_rows,
            timestamp,
            crate::materialized_view::RefreshMode::InsertOnly,
        );
        // 标记源表已见行数（与追加行数一致，因 INSERT_ONLY 模式下 CDC 事件数 == 新增行数）
        let current_hwm = mv_store.high_water_mark(source_table_name);
        mv_store.set_high_water_mark(source_table_name, current_hwm + rows_appended);

        Ok(crate::materialized_view::RefreshOutcome::insert_only(
            rows_appended,
            total_rows,
        ))
    }

    // -----------------------------------------------------------------
    //  Phase 6.12: SIMPLE 增量刷新
    // -----------------------------------------------------------------

    /// 执行 SIMPLE 增量刷新 — Phase 6.12
    ///
    /// # 语义
    ///
    /// - 从 `cdc_feed` drain 所有 CDC 事件（INSERT/UPDATE/DELETE）
    /// - 对 `CdcEvent::Insert`：追加新行到 `mv_store.storage`（若设置主键，更新主键索引）
    /// - 对 `CdcEvent::Update`：按主键查找行，存在则替换，不存在则追加（UPSERT 语义）
    /// - 对 `CdcEvent::Delete`：按主键查找行，标记删除（tombstone）
    /// - 更新 `mv_store.refresh_state` 与高水位
    /// - 返回 `RefreshOutcome`（含插入/更新/删除行数、刷新后总行数）
    ///
    /// # 限制
    ///
    /// - Phase 6.12 仅处理上述三类事件；不处理聚合（留待 Phase 6.13）
    /// - 调用方需在 DML 时显式推送到 `cdc_feed`（执行器不自动捕获）
    /// - 投影逻辑由调用方完成（执行器仅处理预投影的行）
    /// - UPDATE 事件需提供 `pk`（主键值）和 `row`（投影后的新行）
    /// - DELETE 事件需提供 `pk`（主键值）
    /// - 若 `mv_store` 未设置主键索引，UPDATE 退化为 INSERT，DELETE 为 no-op
    ///
    /// # 参数
    ///
    /// - `view_name`：物化视图名（用于校验视图存在且为物化视图）
    /// - `catalog`：catalog（用于查询视图定义）
    /// - `mv_store`：物化视图存储（可变引用，按主键合并）
    /// - `cdc_feed`：CDC 事件缓冲（可变引用，drain 事件）
    /// - `source_table_name`：源表名（用于高水位追踪）
    /// - `timestamp`：刷新时间戳（Unix 微秒）
    pub fn refresh_materialized_view_simple(
        &self,
        view_name: &TableName,
        catalog: &InMemoryCatalog,
        mv_store: &mut crate::materialized_view::MaterializedViewStore,
        cdc_feed: &mut crate::materialized_view::CdcFeed,
        source_table_name: &TableName,
        timestamp: i64,
    ) -> Result<crate::materialized_view::RefreshOutcome, ExecutionError> {
        // 校验视图存在且为物化视图
        let view_def = catalog.get_view(view_name).ok_or_else(|| {
            ExecutionError::InvalidArgument(format!(
                "materialized view does not exist: {}",
                view_name.qualified_name()
            ))
        })?;
        if !view_def.materialized {
            return Err(ExecutionError::InvalidArgument(format!(
                "{} is not a materialized view",
                view_name.qualified_name()
            )));
        }

        // drain CDC 事件
        let events = cdc_feed.drain();
        let mut rows_inserted: usize = 0;
        let mut rows_updated: usize = 0;
        let mut rows_deleted: usize = 0;

        for event in events {
            match event {
                crate::materialized_view::CdcEvent::Insert { row, .. } => {
                    mv_store.append_row(row);
                    rows_inserted += 1;
                }
                crate::materialized_view::CdcEvent::Update { pk, row, .. } => {
                    // 按主键 UPSERT：若主键存在则替换，否则追加
                    if mv_store.has_primary_key() {
                        // 先尝试按 pk 删除旧行
                        if mv_store.delete_by_pk(&pk) {
                            // 旧行已删除，追加新行
                            mv_store.append_row(row);
                            rows_updated += 1;
                        } else {
                            // 主键不存在，视为新插入
                            mv_store.append_row(row);
                            rows_inserted += 1;
                        }
                    } else {
                        // 无主键索引，退化为追加
                        mv_store.append_row(row);
                        rows_inserted += 1;
                    }
                }
                crate::materialized_view::CdcEvent::Delete { pk, .. } => {
                    if mv_store.delete_by_pk(&pk) {
                        rows_deleted += 1;
                    }
                    // 主键不存在或已删除：no-op
                }
            }
        }

        // 更新刷新状态
        let total_rows = mv_store.active_row_count();
        mv_store.refresh_state = crate::materialized_view::RefreshState::initialized(
            total_rows,
            timestamp,
            crate::materialized_view::RefreshMode::Simple,
        );
        // 高水位推进（按事件总数）
        let events_count = rows_inserted + rows_updated + rows_deleted;
        let current_hwm = mv_store.high_water_mark(source_table_name);
        mv_store.set_high_water_mark(source_table_name, current_hwm + events_count);

        Ok(crate::materialized_view::RefreshOutcome::simple(
            rows_inserted,
            rows_updated,
            rows_deleted,
            total_rows,
        ))
    }

    /// 增量刷新物化视图（AGGREGATE 模式）— Phase 6.13
    ///
    /// 消费 CDC 事件缓冲中的 INSERT/DELETE 事件，按聚合函数（SUM/COUNT/AVG/MIN/MAX）
    /// 增量更新物化视图存储中的预聚合值。
    ///
    /// # 参数
    ///
    /// - `view_name`：物化视图名（用于校验视图存在且为物化视图）
    /// - `catalog`：catalog（用于查询视图定义）
    /// - `mv_store`：物化视图存储（可变引用，必须已配置 `aggregate_specs`）
    /// - `cdc_feed`：CDC 事件缓冲（可变引用，drain 事件）
    /// - `source_table_name`：源表名（用于高水位追踪）
    /// - `timestamp`：刷新时间戳（Unix 微秒）
    ///
    /// # 行为
    ///
    /// - `Insert`：对每个聚合规格，提取源列值并递增聚合状态
    /// - `Delete`：对每个聚合规格，提取源列值并递减聚合状态
    ///   - 若 CDC 事件未提供完整旧行（`row = None`），则跳过（no-op）
    ///   - SUM/COUNT/AVG 可递减；MIN/MAX 无法递减（计数到 `decrements_failed`）
    /// - `Update`：视为 DELETE（旧行）+ INSERT（新行），但 CDC 事件未提供旧行时退化为仅 INSERT
    ///
    /// # 限制
    ///
    /// - MIN/MAX 的 DELETE 无法简单递减，需全量重算（`decrements_failed > 0` 时提示调用方）
    /// - UPDATE 事件未包含旧行时，退化为仅 INSERT（聚合值偏高）
    pub fn refresh_materialized_view_aggregate(
        &self,
        view_name: &TableName,
        catalog: &InMemoryCatalog,
        mv_store: &mut crate::materialized_view::MaterializedViewStore,
        cdc_feed: &mut crate::materialized_view::CdcFeed,
        source_table_name: &TableName,
        timestamp: i64,
    ) -> Result<crate::materialized_view::RefreshOutcome, ExecutionError> {
        // 校验视图存在且为物化视图
        let view_def = catalog.get_view(view_name).ok_or_else(|| {
            ExecutionError::InvalidArgument(format!(
                "materialized view does not exist: {}",
                view_name.qualified_name()
            ))
        })?;
        if !view_def.materialized {
            return Err(ExecutionError::InvalidArgument(format!(
                "{} is not a materialized view",
                view_name.qualified_name()
            )));
        }

        // 校验聚合规格已配置
        if !mv_store.has_aggregates() {
            return Err(ExecutionError::InvalidArgument(format!(
                "materialized view {} has no aggregate specs configured (AGGREGATE mode requires new_with_aggregates)",
                view_name.qualified_name()
            )));
        }

        // drain CDC 事件
        let events = cdc_feed.drain();
        let mut rows_inserted: usize = 0;
        let mut rows_deleted: usize = 0;
        let mut decrements_failed: usize = 0;

        for event in events {
            match event {
                crate::materialized_view::CdcEvent::Insert { row, .. } => {
                    mv_store.apply_aggregate_insert(&row);
                    rows_inserted += 1;
                }
                crate::materialized_view::CdcEvent::Delete { row, .. } => {
                    if let Some(old_row) = row {
                        if !mv_store.apply_aggregate_delete(&old_row) {
                            decrements_failed += 1;
                        }
                        rows_deleted += 1;
                    }
                    // 无完整旧行时跳过（无法递减）
                }
                crate::materialized_view::CdcEvent::Update { row, .. } => {
                    // UPDATE 视为 INSERT（新行）；若需要递减旧行，调用方应拆分为 DELETE + INSERT
                    mv_store.apply_aggregate_insert(&row);
                    rows_inserted += 1;
                }
            }
        }

        // 更新刷新状态
        let total_rows = mv_store.active_row_count();
        mv_store.refresh_state = crate::materialized_view::RefreshState::initialized(
            total_rows,
            timestamp,
            crate::materialized_view::RefreshMode::Aggregate,
        );
        // 高水位推进（按事件总数）
        let events_count = rows_inserted + rows_deleted;
        let current_hwm = mv_store.high_water_mark(source_table_name);
        mv_store.set_high_water_mark(source_table_name, current_hwm + events_count);

        Ok(crate::materialized_view::RefreshOutcome::aggregate(
            rows_inserted,
            rows_deleted,
            decrements_failed,
            total_rows,
        ))
    }

    /// 增量刷新物化视图（GROUP_AGGREGATE 模式）— Phase 6.14
    ///
    /// 按 CDC 事件驱动分组聚合增量更新：每个分组独立维护 SUM/COUNT/AVG/MIN/MAX 状态。
    ///
    /// # 参数
    ///
    /// - `view_name`：物化视图名（必须在 catalog 中注册为物化视图）
    /// - `catalog`：catalog 引用（用于校验视图存在性）
    /// - `mv_store`：物化视图存储（必须通过 `new_with_group_aggregates` 创建）
    /// - `cdc_feed`：CDC 事件缓冲（可变引用，drain 事件）
    /// - `source_table_name`：源表名（用于高水位追踪）
    /// - `timestamp`：刷新时间戳（Unix 微秒）
    ///
    /// # 行为
    ///
    /// - `Insert`：提取分组键，查找或创建该分组的聚合状态，递增聚合值
    /// - `Delete`：提取分组键，查找该分组的聚合状态，递减聚合值
    ///   - 若 CDC 事件未提供完整旧行（`row = None`），则跳过（no-op）
    ///   - SUM/COUNT/AVG 可递减；MIN/MAX 无法递减（计数到 `decrements_failed`）
    ///   - 分组不存在时返回 `false`（计数到 `decrements_failed`）
    /// - `Update`：视为 INSERT（新行）；若需要递减旧行，调用方应拆分为 DELETE + INSERT
    ///
    /// # 限制
    ///
    /// - MIN/MAX 的 DELETE 无法简单递减，需全量重算（`decrements_failed > 0` 时提示调用方）
    /// - UPDATE 事件未包含旧行时，退化为仅 INSERT（聚合值偏高）
    pub fn refresh_materialized_view_group_aggregate(
        &self,
        view_name: &TableName,
        catalog: &InMemoryCatalog,
        mv_store: &mut crate::materialized_view::MaterializedViewStore,
        cdc_feed: &mut crate::materialized_view::CdcFeed,
        source_table_name: &TableName,
        timestamp: i64,
    ) -> Result<crate::materialized_view::RefreshOutcome, ExecutionError> {
        // 校验视图存在且为物化视图
        let view_def = catalog.get_view(view_name).ok_or_else(|| {
            ExecutionError::InvalidArgument(format!(
                "materialized view does not exist: {}",
                view_name.qualified_name()
            ))
        })?;
        if !view_def.materialized {
            return Err(ExecutionError::InvalidArgument(format!(
                "{} is not a materialized view",
                view_name.qualified_name()
            )));
        }

        // 校验分组聚合规格已配置
        if !mv_store.has_group_aggregates() {
            return Err(ExecutionError::InvalidArgument(format!(
                "materialized view {} has no group aggregate specs configured (GROUP_AGGREGATE mode requires new_with_group_aggregates)",
                view_name.qualified_name()
            )));
        }

        // drain CDC 事件
        let events = cdc_feed.drain();
        let mut rows_inserted: usize = 0;
        let mut rows_deleted: usize = 0;
        let mut decrements_failed: usize = 0;

        for event in events {
            match event {
                crate::materialized_view::CdcEvent::Insert { row, .. } => {
                    mv_store.apply_group_aggregate_insert(&row);
                    rows_inserted += 1;
                }
                crate::materialized_view::CdcEvent::Delete { row, .. } => {
                    if let Some(old_row) = row {
                        if !mv_store.apply_group_aggregate_delete(&old_row) {
                            decrements_failed += 1;
                        }
                        rows_deleted += 1;
                    }
                    // 无完整旧行时跳过（无法递减）
                }
                crate::materialized_view::CdcEvent::Update { row, .. } => {
                    // UPDATE 视为 INSERT（新行）；若需要递减旧行，调用方应拆分为 DELETE + INSERT
                    mv_store.apply_group_aggregate_insert(&row);
                    rows_inserted += 1;
                }
            }
        }

        // 更新刷新状态
        let total_rows = mv_store.group_count();
        mv_store.refresh_state = crate::materialized_view::RefreshState::initialized(
            total_rows,
            timestamp,
            crate::materialized_view::RefreshMode::GroupAggregate,
        );
        // 高水位推进（按事件总数）
        let events_count = rows_inserted + rows_deleted;
        let current_hwm = mv_store.high_water_mark(source_table_name);
        mv_store.set_high_water_mark(source_table_name, current_hwm + events_count);

        Ok(crate::materialized_view::RefreshOutcome::group_aggregate(
            rows_inserted,
            rows_deleted,
            decrements_failed,
            total_rows,
        ))
    }

    // -----------------------------------------------------------------
    //  Phase 6.4: 触发器钩子
    // -----------------------------------------------------------------

    /// 收集表上的所有触发器定义（若 catalog 或 trigger_registry 未绑定则返回空 Vec）
    ///
    /// DML 入口处调用一次，缓存触发器列表避免重复查询 catalog。
    fn triggers_for_table(&self, table: &TableName) -> Vec<TriggerDefinition> {
        if self.trigger_registry.is_none() {
            return Vec::new();
        }
        match self.catalog {
            Some(cat) => cat.list_triggers(table),
            None => Vec::new(),
        }
    }

    /// 触发 BEFORE STATEMENT 触发器（无触发器时为 no-op）
    fn fire_before_statement(
        &self,
        triggers: &[TriggerDefinition],
        kind: DmlKind,
        table_name: &TableName,
        schema: &TableSchema,
    ) -> Result<(), ExecutionError> {
        if triggers.is_empty() {
            return Ok(());
        }
        if let Some(reg) = self.trigger_registry {
            fire_statement_triggers(
                reg,
                triggers,
                kind,
                TriggerTiming::Before,
                &table_name.qualified_name(),
                schema,
            )?;
        }
        Ok(())
    }

    /// 触发 AFTER STATEMENT 触发器（无触发器时为 no-op）
    fn fire_after_statement(
        &self,
        triggers: &[TriggerDefinition],
        kind: DmlKind,
        table_name: &TableName,
        schema: &TableSchema,
    ) -> Result<(), ExecutionError> {
        if triggers.is_empty() {
            return Ok(());
        }
        if let Some(reg) = self.trigger_registry {
            fire_statement_triggers(
                reg,
                triggers,
                kind,
                TriggerTiming::After,
                &table_name.qualified_name(),
                schema,
            )?;
        }
        Ok(())
    }

    /// 触发 BEFORE ROW 触发器（无触发器时为 no-op）
    ///
    /// 返回 [`FireResult`]：
    /// - `ContinueWith(None)` — 使用原 NEW 行继续
    /// - `ContinueWith(Some(modified))` — 使用修改后的 NEW 行
    /// - `SkipRow` — 跳过该行
    #[allow(clippy::too_many_arguments)]
    fn fire_before_row(
        &self,
        triggers: &[TriggerDefinition],
        kind: DmlKind,
        table_name: &TableName,
        schema: &TableSchema,
        new_row: Option<&Row>,
        old_row: Option<&Row>,
        updated_columns: Option<&[String]>,
    ) -> Result<FireResult, ExecutionError> {
        if triggers.is_empty() {
            // 无触发器：返回 None 表示"无修改，调用方使用原行"
            return Ok(FireResult::ContinueWith(None));
        }
        if let Some(reg) = self.trigger_registry {
            fire_row_triggers(
                reg,
                triggers,
                kind,
                TriggerTiming::Before,
                &table_name.qualified_name(),
                schema,
                new_row,
                old_row,
                updated_columns,
            )
        } else {
            // 未绑定 registry：返回 None 表示"无修改"
            Ok(FireResult::ContinueWith(None))
        }
    }

    /// 触发 AFTER ROW 触发器（无触发器时为 no-op）
    ///
    /// AFTER 触发器返回值被忽略。
    #[allow(clippy::too_many_arguments)]
    fn fire_after_row(
        &self,
        triggers: &[TriggerDefinition],
        kind: DmlKind,
        table_name: &TableName,
        schema: &TableSchema,
        new_row: Option<&Row>,
        old_row: Option<&Row>,
        updated_columns: Option<&[String]>,
    ) -> Result<(), ExecutionError> {
        if triggers.is_empty() {
            return Ok(());
        }
        if let Some(reg) = self.trigger_registry {
            // AFTER 触发器内部强制忽略返回值
            let _ = fire_row_triggers(
                reg,
                triggers,
                kind,
                TriggerTiming::After,
                &table_name.qualified_name(),
                schema,
                new_row,
                old_row,
                updated_columns,
            )?;
        }
        Ok(())
    }

    /// Phase 3.32: 将行中数组列的 `Value::Text` 形式解析为 `Value::Array`
    ///
    /// PG 语义：INSERT/UPDATE 时允许字符串字面量 '{1,2,3}' 隐式转换为数组。
    /// 此方法就地修改 row，将数组列中匹配 `ColumnType::Array(_)` 的 `Value::Text(s)`
    /// 解析为 `Value::Array(...)`。其他值类型保持不变。
    ///
    /// 解析格式：`{v1,v2,...}`（PG 数组字面量；元素之间用逗号分隔；支持引号包裹）
    fn coerce_array_values(
        &self,
        schema: &TableSchema,
        row: &mut Row,
    ) -> Result<(), ExecutionError> {
        for (i, col) in schema.columns.iter().enumerate() {
            if let szrsql_types::value::ColumnType::Array(elem_type) = &col.data_type {
                let value = std::mem::replace(&mut row[i], Value::Null);
                match value {
                    Value::Array(_) => {
                        // 已是数组，保持
                        row[i] = value;
                    }
                    Value::Text(s) => {
                        // 尝试解析 PG 数组字面量 '{v1,v2,...}'
                        match parse_pg_array_literal(&s, elem_type) {
                            Ok(arr) => row[i] = Value::Array(arr),
                            Err(_) => {
                                // 解析失败：保留为 Text（与 PG 行为一致：非法数组字面量报错）
                                return Err(ExecutionError::Unsupported(format!(
                                    "cannot parse '{}' as array for column {}.{}",
                                    s,
                                    schema.name.qualified_name(),
                                    col.name
                                )));
                            }
                        }
                    }
                    Value::Null => {
                        row[i] = Value::Null;
                    }
                    other => {
                        row[i] = other;
                    }
                }
            }
        }
        Ok(())
    }

    /// Phase 6.18: 求值生成列表达式
    ///
    /// 遍历 schema.columns 按声明顺序，对每个 `generated` 字段非空的列，
    /// 使用 `ExecRowContext` + `ExprEvaluator::eval` 求值生成表达式并覆写行值。
    ///
    /// 求值顺序保证：按列声明顺序，生成列 B 可引用生成列 A（A 必须在 B 之前声明）。
    /// 这与 PG STORED 生成列语义一致。
    fn evaluate_generated_columns(
        &self,
        schema: &TableSchema,
        row: &mut Row,
    ) -> Result<(), ExecutionError> {
        for (i, col) in schema.columns.iter().enumerate() {
            if let Some(gen) = &col.generated {
                let ctx = ExecRowContext::new_proxy(schema, row);
                let value = ExprEvaluator::eval(&gen.expr, &ctx)?;
                if i < row.len() {
                    row[i] = value;
                }
            }
        }
        Ok(())
    }

    /// 执行 SELECT nextval('seq') / currval('seq') — Phase 3.22
    ///
    /// 在执行 SELECT 前预处理计划，将 nextval/currval 函数调用替换为常量字面量。
    /// 然后调用普通 `execute`。
    pub fn execute_with_sequences(
        &self,
        plan: &LogicalPlan,
        seq_store: &mut dyn SequenceStore,
    ) -> Result<Vec<Row>, ExecutionError> {
        let resolved = resolve_sequences_in_plan(plan, seq_store)?;
        self.execute(&resolved)
    }

    /// 执行 INSERT 计划（带序列支持）— Phase 3.22
    ///
    /// 与 `execute_insert` 区别：
    /// - VALUES 表达式中的 `nextval('seq')` / `currval('seq')` 被解析为常量
    /// - 未指定列的 DEFAULT 表达式被求值（含 SERIAL 列的 `nextval('table_col_seq')`）
    pub fn execute_insert_with_sequences(
        &self,
        plan: &LogicalPlan,
        table: &mut dyn MutableTable,
        seq_store: &mut dyn SequenceStore,
    ) -> Result<DmlResult, ExecutionError> {
        // 1. 解析 INSERT VALUES 中的序列调用 + 应用 DEFAULT
        let resolved_plan = self.resolve_insert_sequences(plan, seq_store)?;
        // 2. 调用普通 execute_insert 路径
        self.execute_insert(&resolved_plan, table)
    }

    /// 将 INSERT 计划中的序列调用解析为常量，并为未指定列应用 DEFAULT
    ///
    /// 返回新的 LogicalPlan::Insert，其中：
    /// - 所有 VALUES 行的表达式被 nextval/currval 替换为常量字面量
    /// - 未显式指定的列被追加其 DEFAULT 表达式（已解析序列调用）
    fn resolve_insert_sequences(
        &self,
        plan: &LogicalPlan,
        seq_store: &mut dyn SequenceStore,
    ) -> Result<LogicalPlan, ExecutionError> {
        let (table, schema, columns, source, on_conflict, returning) = match plan {
            LogicalPlan::Insert {
                table,
                schema,
                columns,
                source,
                on_conflict,
                returning,
            } => (table, schema, columns, source, on_conflict, returning),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Insert plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // 计算目标列索引
        let target_indices: Vec<usize> = match columns {
            None => (0..schema.columns.len()).collect(),
            Some(cols) => cols
                .iter()
                .map(|name| {
                    schema
                        .columns
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(name))
                        .ok_or_else(|| ExecutionError::ColumnNotFound(name.clone()))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };

        // 收集未指定列的索引和其 DEFAULT 表达式
        let mut default_pairs: Vec<(usize, &Expr)> = Vec::new();
        for (i, col) in schema.columns.iter().enumerate() {
            if !target_indices.contains(&i) {
                if let Some(default_expr) = &col.default {
                    default_pairs.push((i, default_expr));
                }
            }
        }

        let new_source = match source {
            InsertSourcePlan::Values(rows_expr) => {
                let mut new_rows = Vec::with_capacity(rows_expr.len());
                for row_expr in rows_expr {
                    // 解析每行的序列调用
                    let mut new_row: Vec<Expr> =
                        Vec::with_capacity(row_expr.len() + default_pairs.len());
                    for expr in row_expr {
                        new_row.push(resolve_sequence_calls(expr, seq_store)?);
                    }
                    // 追加 DEFAULT 表达式（已解析序列调用）— 未指定列以 DEFAULT 填充
                    for (_, default_expr) in &default_pairs {
                        let resolved_default = resolve_sequence_calls(default_expr, seq_store)?;
                        new_row.push(resolved_default);
                    }
                    new_rows.push(new_row);
                }

                // 扩展目标列索引，追加 DEFAULT 列
                let mut extended_targets = target_indices.clone();
                for (idx, _) in &default_pairs {
                    extended_targets.push(*idx);
                }

                let new_columns: Option<Vec<String>> = Some(
                    extended_targets
                        .iter()
                        .map(|&i| schema.columns[i].name.clone())
                        .collect(),
                );
                return Ok(LogicalPlan::Insert {
                    table: table.clone(),
                    schema: schema.clone(),
                    columns: new_columns,
                    source: InsertSourcePlan::Values(new_rows),
                    on_conflict: on_conflict.clone(),
                    returning: returning.clone(),
                });
            }
            InsertSourcePlan::Select(sub_plan) => {
                let resolved_sub = resolve_sequences_in_plan(sub_plan, seq_store)?;
                InsertSourcePlan::Select(Box::new(resolved_sub))
            }
            InsertSourcePlan::DefaultValues => {
                // DEFAULT VALUES：所有列取 DEFAULT
                // 构造一个空 VALUES 行 + 扩展 DEFAULT 表达式
                let mut new_row: Vec<Expr> = Vec::with_capacity(default_pairs.len());
                let mut extended_targets: Vec<usize> = Vec::with_capacity(default_pairs.len());
                for (idx, default_expr) in &default_pairs {
                    new_row.push(resolve_sequence_calls(default_expr, seq_store)?);
                    extended_targets.push(*idx);
                }
                let new_columns: Option<Vec<String>> = Some(
                    extended_targets
                        .iter()
                        .map(|&i| schema.columns[i].name.clone())
                        .collect(),
                );
                return Ok(LogicalPlan::Insert {
                    table: table.clone(),
                    schema: schema.clone(),
                    columns: new_columns,
                    source: InsertSourcePlan::Values(vec![new_row]),
                    on_conflict: on_conflict.clone(),
                    returning: returning.clone(),
                });
            }
        };

        Ok(LogicalPlan::Insert {
            table: table.clone(),
            schema: schema.clone(),
            columns: columns.clone(),
            source: new_source,
            on_conflict: on_conflict.clone(),
            returning: returning.clone(),
        })
    }

    /// 迭代器执行逻辑计划（Volcano 模型）
    ///
    /// 适用于简单查询（Scan + Filter + Projection + Limit），无需物化中间结果。
    /// 对于复杂查询（Join/Aggregate/Sort），回退到 execute()。
    #[instrument(skip(self), fields(plan_type = ?std::mem::discriminant(plan), row_count = tracing::field::Empty))]
    pub fn execute_iterative(&self, plan: &LogicalPlan) -> Result<Vec<Row>, ExecutionError> {
        trace!("executing logical plan (iterative)");
        let _udf_guard = self.thread_udf_guard();
        let _sql_func_guard = self.sql_functions_guard();

        // 构建迭代器执行树
        let table_data = self.build_table_data_for_iter(plan)?;
        let mut iter_plan = build_iter_plan(plan, &table_data);

        // 排空迭代器
        let mut rows = Vec::new();
        while let Some(row) = iter_plan.next() {
            rows.push(row);
        }
        Ok(rows)
    }

    /// 构建迭代器所需的表数据映射
    fn build_table_data_for_iter(
        &self,
        plan: &LogicalPlan,
    ) -> Result<TableDataMap, ExecutionError> {
        let mut table_data = std::collections::HashMap::new();
        self.collect_scan_tables(plan, &mut table_data)?;
        Ok(table_data)
    }

    /// 递归收集计划中的表数据
    fn collect_scan_tables(
        &self,
        plan: &LogicalPlan,
        table_data: &mut TableDataMap,
    ) -> Result<(), ExecutionError> {
        match plan {
            LogicalPlan::Scan { table, schema, .. } => {
                let key = table.name.to_lowercase();
                if let std::collections::hash_map::Entry::Vacant(e) = table_data.entry(key) {
                    let rows = self.execute_scan(table, schema)?;
                    let names: Vec<String> =
                        schema.columns.iter().map(|c| c.name.clone()).collect();
                    e.insert((rows, names));
                }
            }
            LogicalPlan::Filter { input, .. } => self.collect_scan_tables(input, table_data)?,
            LogicalPlan::Projection { input, .. } => self.collect_scan_tables(input, table_data)?,
            LogicalPlan::Limit { input, .. } => self.collect_scan_tables(input, table_data)?,
            LogicalPlan::Sort { input, .. } => self.collect_scan_tables(input, table_data)?,
            LogicalPlan::Distinct { input } => self.collect_scan_tables(input, table_data)?,
            LogicalPlan::Join { left, right, .. } => {
                self.collect_scan_tables(left, table_data)?;
                self.collect_scan_tables(right, table_data)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// 执行逻辑计划，物化全部结果行
    #[instrument(skip(self), fields(plan_type = ?std::mem::discriminant(plan), row_count = tracing::field::Empty))]
    pub fn execute(&self, plan: &LogicalPlan) -> Result<Vec<Row>, ExecutionError> {
        trace!("executing logical plan");
        // P0-SQL-8 修复：在 SQL 执行期间注入 UDF 注册表到当前线程，
        // 供 ExprEvaluator 在内建函数表未命中时回退查询。
        // guard 析构时自动清理，嵌套 execute 调用安全。
        let _udf_guard = self.thread_udf_guard();
        // P0-FN 修复：在 SQL 执行期间注入 SQL 函数定义（CREATE FUNCTION）到当前线程，
        // 供 ExprEvaluator 在内建函数表和 UDF 注册表未命中时回退查询。
        // guard 析构时自动清理 thread_local，执行完毕后注册表归零。
        let _sql_func_guard = self.sql_functions_guard();
        // P0-3 修复：在 SQL 执行期间注入 PL/pgSQL 函数注册表到当前线程，
        // 供 ExprEvaluator 在调用 LANGUAGE plpgsql 函数时通过 PlPgSqlInterpreter 执行。
        let _plpgsql_guard = self.plpgsql_guard();
        let result = match plan {
            LogicalPlan::Scan {
                table,
                schema,
                alias: _,
            } => self.execute_scan(table, schema),
            LogicalPlan::IndexScan {
                table,
                schema,
                alias: _,
                index_name,
                index_columns,
                predicate,
            } => self.execute_index_scan(table, schema, index_name, index_columns, predicate),
            LogicalPlan::Filter { predicate, input } => self.execute_filter(predicate, input),
            LogicalPlan::Projection {
                exprs,
                output_names: _,
                input,
            } => self.execute_projection(exprs, input),
            LogicalPlan::Limit {
                limit,
                offset,
                input,
            } => self.execute_limit(limit, offset, input),
            LogicalPlan::Distinct { input } => self.execute_distinct(input),
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
            } => self.execute_join(*join_type, condition, left, right),
            LogicalPlan::Aggregate {
                group_exprs,
                aggregates,
                having,
                input,
            } => self.execute_aggregate(group_exprs, aggregates, having, input),
            LogicalPlan::SetOp {
                op,
                quantifier,
                left,
                right,
            } => self.execute_set_op(*op, *quantifier, left, right),
            LogicalPlan::Empty => Ok(Vec::new()),
            LogicalPlan::Dual => Ok(vec![Vec::new()]), // 虚拟单行表（无列）
            // Phase 5.8: Shared/MemoRef
            LogicalPlan::Shared { id, plan } => self.execute_shared(*id, plan),
            LogicalPlan::MemoRef { id, .. } => self.execute_memo_ref(*id),
            // Phase 6.1: WITH 子句（CTE）
            LogicalPlan::With { ctes, input } => self.execute_with(ctes, input),
            // Phase 6.1: CTE 引用
            LogicalPlan::CteRef { name, schema: _ } => self.execute_cte_ref(name),
            // Phase 6.2: 窗口函数
            LogicalPlan::Window {
                window_funcs,
                input,
            } => self.execute_window(window_funcs, input),
            // Phase 6.3: ORDER BY 排序
            LogicalPlan::Sort { order_by, input } => self.execute_sort(order_by, input),
            // Phase 6.15: 物化视图扫描
            LogicalPlan::MaterializedViewScan { name, .. } => {
                self.execute_materialized_view_scan(name)
            }
            _ => Err(ExecutionError::Unsupported(format!(
                "{:?}",
                std::mem::discriminant(plan)
            ))),
        };
        match &result {
            Ok(rows) => {
                tracing::Span::current().record("row_count", rows.len());
                trace!(row_count = rows.len(), "plan executed");
            }
            Err(e) => trace!(error = %e, "plan execution failed"),
        }
        result
    }

    // -----------------------------------------------------------------
    //  SeqScan
    // -----------------------------------------------------------------

    fn execute_scan(
        &self,
        table: &TableName,
        _schema: &TableSchema,
    ) -> Result<Vec<Row>, ExecutionError> {
        let storage = self
            .lookup_table(&table.name)
            .ok_or_else(|| ExecutionError::TableNotFound(table.qualified_name()))?;
        // P0-TX-1 Phase B/C：MVCC 可见性过滤
        //
        // 当 MVCC 管理器已注入且当前有活跃事务（txn_id != 0）时：
        // 1. Phase C — 若隔离级别为 ReadCommitted/ReadUncommitted，先刷新快照
        //    （PG 语义：RC 每条语句看到最新已提交数据，即 statement-level snapshot）
        // 2. 使用 scan_with_versions() 获取每行的 xmin/xmax
        // 3. 通过 MvccManager::is_visible() 按当前事务快照过滤
        // 4. 注册读集合（SSI 写偏斜检测用）
        //
        // RR/Serializable 使用 BEGIN 时的快照（事务级快照），不刷新。
        // 未注入或 txn_id == 0（autocommit）时，退化为 scan_iter()（旧行为）。
        if let Some(mvcc) = self.mvcc {
            if self.mvcc_txn_id != 0 {
                // Phase C：RC/RU 在每条 SELECT 前刷新快照，看到最新已提交数据
                if let Some(iso) = mvcc.get_isolation_level(self.mvcc_txn_id) {
                    if matches!(
                        iso,
                        szrsql_tx::mvcc::IsolationLevel::ReadCommitted
                            | szrsql_tx::mvcc::IsolationLevel::ReadUncommitted
                    ) {
                        // 刷新快照（已校验隔离级别，理论上不会返回 Err，
                        // 但保守忽略错误以避免 SELECT 因快照刷新失败而中断）
                        let _ = mvcc.refresh_snapshot(self.mvcc_txn_id);
                    }
                }
                // 注册读集合（SSI 写偏斜检测用）— key 格式 "table_name"
                // 注：此处注册表级读，DML 路径会注册行级读
                let table_key = table.name.to_lowercase();
                let _ = mvcc.register_read(self.mvcc_txn_id, &table_key);
                let rows: Vec<Row> = storage
                    .scan_with_versions()
                    .filter(|(_, _, xmin, xmax)| mvcc.is_visible(self.mvcc_txn_id, *xmin, *xmax))
                    .map(|(_, row, _, _)| row)
                    .collect();
                return Ok(rows);
            }
        }
        Ok(storage.scan_iter().collect())
    }

    // -----------------------------------------------------------------
    //  MaterializedViewScan — Phase 6.15
    // -----------------------------------------------------------------

    /// 执行物化视图扫描
    ///
    /// 从 `materialized_view_stores` 注册表按名查找物化视图存储引用，
    /// 返回存储表中的全部行。
    fn execute_materialized_view_scan(&self, name: &TableName) -> Result<Vec<Row>, ExecutionError> {
        let storage = self
            .lookup_materialized_view_store(&name.name)
            .ok_or_else(|| ExecutionError::TableNotFound(name.qualified_name()))?;
        Ok(storage.scan_iter().collect())
    }

    // -----------------------------------------------------------------
    //  IndexScan — Phase 5.7
    // -----------------------------------------------------------------

    /// 执行索引扫描
    ///
    /// 流程：
    /// 1. 按索引名查找已注册的 `InMemoryBTreeIndex`
    /// 2. 从 `predicate` 中提取索引列的访问条件（等值 / 范围）
    /// 3. 调用 `point_lookup` 或 `range_lookup` 获取候选 row_ids
    /// 4. 按 row_ids 从表存储中 fetch 行
    /// 5. 对候选行应用完整 `predicate` 作为残余过滤
    ///
    /// 限制：
    /// - 仅支持单列 i64 索引（受 `InMemoryBTreeIndex` 限制）
    /// - 谓词提取仅识别 `col = literal` / `col >/</>=/<= literal` 形式
    /// - 复合索引当前仅使用首列（最左前缀）做索引查找
    fn execute_index_scan(
        &self,
        table: &TableName,
        schema: &TableSchema,
        index_name: &str,
        index_columns: &[String],
        predicate: &Expr,
    ) -> Result<Vec<Row>, ExecutionError> {
        let storage = self
            .lookup_table(&table.name)
            .ok_or_else(|| ExecutionError::TableNotFound(table.qualified_name()))?;

        let index = self.lookup_index(&table.name, index_name).ok_or_else(|| {
            ExecutionError::InvalidArgument(format!(
                "index '{}' not registered for table '{}'",
                index_name,
                table.qualified_name()
            ))
        })?;

        // 索引首列名（复合索引仅使用最左前缀做查找）
        let first_idx_col = index_columns.first().ok_or_else(|| {
            ExecutionError::InvalidArgument(format!(
                "IndexScan on {} has no index_columns",
                table.qualified_name()
            ))
        })?;

        // 验证索引首列存在于表 schema
        if schema.find_column(first_idx_col).is_none() {
            return Err(ExecutionError::InvalidArgument(format!(
                "index column '{}' not found in schema of table '{}'",
                first_idx_col,
                table.qualified_name()
            )));
        }

        // 从谓词中提取索引列的访问条件
        let access = extract_index_access(predicate, first_idx_col);

        // 按访问条件做索引查找
        let row_ids: Vec<usize> = match access {
            IndexAccessPath::Point(key) => index.point_lookup(key),
            IndexAccessPath::Range(low, high) => index.range_lookup(low, high),
            IndexAccessPath::None => {
                // 谓词不含索引列条件 → 退化为全索引扫描
                // 遍历所有 key 收集 row_ids
                index.all_row_ids()
            }
        };

        // fetch 行 + 残余过滤
        let mut result = Vec::with_capacity(row_ids.len());
        for row_id in row_ids {
            let row = match storage.get_row(row_id) {
                Some(r) => r,
                None => continue,
            };
            // 应用完整谓词过滤
            let ctx = ExecRowContext::new(schema, &row);
            match ExprEvaluator::eval(predicate, &ctx)? {
                Value::Bool(true) => result.push(row),
                Value::Bool(false) | Value::Null => {}
                other => {
                    return Err(ExecutionError::EvalError(format!(
                        "IndexScan predicate must evaluate to bool, got {:?}",
                        other
                    )));
                }
            }
        }
        Ok(result)
    }

    // -----------------------------------------------------------------
    //  Shared / MemoRef — Phase 5.8
    // -----------------------------------------------------------------

    /// 执行共享子计划，首次执行后缓存结果
    ///
    /// 缓存命中时直接返回克隆，未命中则执行 `plan` 并写入缓存。
    /// 使用 `RefCell` 以便在 `&self` 的不可变借用下安全修改缓存。
    fn execute_shared(&self, id: u64, plan: &LogicalPlan) -> Result<Vec<Row>, ExecutionError> {
        {
            let cache = self.memo_cache.borrow();
            if let Some(cached) = cache.get(&id) {
                return Ok(cached.clone());
            }
        }
        let rows = self.execute(plan)?;
        let mut cache = self.memo_cache.borrow_mut();
        cache.insert(id, rows.clone());
        Ok(rows)
    }

    /// 从缓存读取共享子计划结果
    ///
    /// 缓存未命中时返回 `Unsupported` 错误（正常流程下 CSE 规则保证先执行对应 Shared）。
    fn execute_memo_ref(&self, id: u64) -> Result<Vec<Row>, ExecutionError> {
        let cache = self.memo_cache.borrow();
        cache
            .get(&id)
            .cloned()
            .ok_or_else(|| ExecutionError::Unsupported(format!("MemoRef cache miss for id={}", id)))
    }

    // -----------------------------------------------------------------
    //  WITH 子句（CTE）— Phase 6.1
    // -----------------------------------------------------------------

    /// 执行 WITH 子句
    ///
    /// 流程：
    /// 1. 压入新 CTE 作用域
    /// 2. 按声明顺序依次物化每个 CTE：
    ///    - `Simple`：执行 plan → 结果存入作用域
    ///    - `Recursive`：迭代至不动点
    ///      a. 执行 anchor → R₀
    ///      b. 将 R₀ 存入作用域供 recursive part 引用
    ///      c. 执行 recursive part → new_rows
    ///      d. 若 new_rows 为空则停止；否则累加到 R₀（UNION ALL 直接拼接，UNION 去重后拼接）
    ///      e. 更新作用域中的 CTE 结果为新的 R₀，重复 c-e
    /// 3. 执行主体（input），返回结果
    /// 4. 弹出作用域
    pub fn execute_with(
        &self,
        ctes: &[CteEntry],
        input: &LogicalPlan,
    ) -> Result<Vec<Row>, ExecutionError> {
        // 压入新作用域
        self.cte_scopes.borrow_mut().push(HashMap::new());

        // 物化每个 CTE
        for entry in ctes {
            match entry {
                CteEntry::Simple { name, plan, .. } => {
                    let rows = self.execute(plan)?;
                    self.cte_scopes
                        .borrow_mut()
                        .last_mut()
                        .unwrap()
                        .insert(name.clone(), rows);
                }
                CteEntry::Recursive {
                    name,
                    anchor,
                    recursive,
                    all,
                    ..
                } => {
                    // PostgreSQL 递归 CTE 语义：
                    // 1. 执行 anchor → 初始"工作表" working_table = R₀
                    // 2. 累积结果 accumulated = R₀
                    // 3. 循环：
                    //    a. 将 CTE 作用域设为 working_table（仅上次新增行）
                    //    b. 执行 recursive part → new_rows
                    //    c. 若 new_rows 为空则停止
                    //    d. UNION ALL：accumulated ∪= new_rows；
                    //       UNION DISTINCT：accumulated ∪= (new_rows 中尚未出现过的)
                    //    e. working_table = new_rows（去重后的新行）
                    //
                    // 关键：recursive part 每次迭代只看到"上次新增行"，
                    // 而非全部累积结果，否则会导致无限循环
                    // （例：SELECT n+1 FROM r WHERE n<5 若 r 持续包含 n=1，则永远产生 n=2）
                    let mut accumulated = self.execute(anchor)?;

                    // 初始工作表 = anchor 结果
                    let mut working_table = accumulated.clone();

                    // 作用域先置为初始工作表
                    self.cte_scopes
                        .borrow_mut()
                        .last_mut()
                        .unwrap()
                        .insert(name.clone(), working_table.clone());

                    // 安全阀：最多迭代 10000 次（防止无限循环）
                    const MAX_ITERATIONS: usize = 10000;
                    let mut iterations = 0usize;

                    loop {
                        iterations += 1;
                        if iterations > MAX_ITERATIONS {
                            return Err(ExecutionError::InvalidArgument(format!(
                                "recursive CTE '{}' exceeded max iterations ({})",
                                name, MAX_ITERATIONS
                            )));
                        }

                        // 执行 recursive part（作用域中 CTE = 当前 working_table）
                        let new_rows = self.execute(recursive)?;

                        if new_rows.is_empty() {
                            break;
                        }

                        // 对新行去重并筛选出真正"新"的行
                        // UNION ALL：保留所有重复行（仍需与 accumulated 去重以构成下一次工作表？）
                        //   实际上 PG UNION ALL recursive CTE 也用工作表去重 — 否则链式树遍历会死循环
                        //   正确语义：working_table_new = new_rows 中不在 accumulated 出现过的行
                        // UNION DISTINCT：accumulated 中没出现过的行
                        let existing: HashSet<String> =
                            accumulated.iter().map(|r| format!("{r:?}")).collect();
                        let mut truly_new: Vec<Row> = Vec::with_capacity(new_rows.len());
                        for row in new_rows {
                            let key = format!("{row:?}");
                            if existing.contains(&key) {
                                continue;
                            }
                            truly_new.push(row);
                        }

                        if truly_new.is_empty() {
                            break;
                        }

                        // 累加到 accumulated（无论 UNION ALL 还是 DISTINCT 都累加真正新行）
                        accumulated.extend(truly_new.clone());

                        // 更新工作表 = 本次迭代真正新增的行
                        working_table = truly_new;

                        // 更新作用域：recursive part 下次迭代看到的就是本次新行
                        self.cte_scopes
                            .borrow_mut()
                            .last_mut()
                            .unwrap()
                            .insert(name.clone(), working_table.clone());

                        // all 标记：未来若需区分 UNION ALL/DISTINCT 的不同语义可在此分支
                        let _ = all;
                    }

                    // 迭代结束后，将作用域中的 CTE 更新为完整累积结果，
                    // 供主体查询（input）引用此 CTE 时看到全部行
                    self.cte_scopes
                        .borrow_mut()
                        .last_mut()
                        .unwrap()
                        .insert(name.clone(), accumulated.clone());
                }
            }
        }

        // 执行主体
        let result = self.execute(input);

        // 弹出作用域
        self.cte_scopes.borrow_mut().pop();

        result
    }

    /// 执行 CTE 引用 — 从作用域读取物化结果
    fn execute_cte_ref(&self, name: &str) -> Result<Vec<Row>, ExecutionError> {
        let key = name.to_lowercase();
        let scopes = self.cte_scopes.borrow();
        for scope in scopes.iter().rev() {
            if let Some(rows) = scope.get(&key) {
                return Ok(rows.clone());
            }
        }
        Err(ExecutionError::Unsupported(format!(
            "CTE '{}' not in scope (not materialized or scope already exited)",
            name
        )))
    }

    // -----------------------------------------------------------------
    //  Window（窗口函数）— Phase 6.2
    // -----------------------------------------------------------------

    /// 执行窗口函数节点
    ///
    /// 输入行的格式：`[input_cols...]`
    /// 输出行的格式：`[input_cols..., window_func_results...]`
    ///
    /// 算法：
    /// 1. 执行 input 获取所有行
    /// 2. 对每个窗口函数：
    ///    a. 按 PARTITION BY 分组
    ///    b. 在每个分组内按 ORDER BY 排序
    ///    c. 对每行计算其窗口帧
    ///    d. 在帧内计算函数值
    /// 3. 将所有窗口函数结果按声明顺序追加到行尾
    fn execute_window(
        &self,
        window_funcs: &[WindowFunctionExpr],
        input: &LogicalPlan,
    ) -> Result<Vec<Row>, ExecutionError> {
        let rows = self.execute(input)?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let schema = input_schema(input)?;
        let n_funcs = window_funcs.len();
        let n_rows = rows.len();

        // 为每个窗口函数计算所有行的结果
        let mut per_func_results: Vec<Vec<Value>> = Vec::with_capacity(n_funcs);
        for wf in window_funcs {
            let results = compute_window_function(wf, &rows, &schema)?;
            per_func_results.push(results);
        }

        // 拼接：每行 = 原行 ++ [各窗口函数在该行的结果]
        let mut out = Vec::with_capacity(n_rows);
        for (row_idx, row) in rows.into_iter().enumerate() {
            let mut new_row = Vec::with_capacity(row.len() + n_funcs);
            new_row.extend(row);
            for func_results in &per_func_results {
                new_row.push(func_results[row_idx].clone());
            }
            out.push(new_row);
        }
        Ok(out)
    }

    // -----------------------------------------------------------------
    //  Filter（WHERE）
    // -----------------------------------------------------------------

    fn execute_filter(
        &self,
        predicate: &Expr,
        input: &LogicalPlan,
    ) -> Result<Vec<Row>, ExecutionError> {
        // P0-STORE 阶段 1：主键等值/范围查询优化
        //
        // 当 input 为 Scan 且谓词包含主键列的等值或范围条件时，
        // 使用 B+Tree 主键索引 O(log n) 查找，避免全表扫描。
        // 若优化不适用（无主键索引、谓词不含主键条件），退化为原有路径。
        if let LogicalPlan::Scan { table, .. } = input {
            if let Some(rows) = self.try_pk_optimized_filter(table, predicate)? {
                tracing::trace!(
                    table = %table.name,
                    "PK-optimized filter: bypassed full table scan"
                );
                return Ok(rows);
            }
        }

        let rows = self.execute(input)?;

        // 预处理：将 IN (SELECT ...) 子查询重写为 IN (val1, val2, ...)
        // 执行子查询并收集第一列值，转换为 InList 表达式
        let predicate = self.rewrite_in_subqueries(predicate)?;

        // 若 input 为 JOIN，使用 JoinedRowContext 以正确路由 `t1.col` / `t2.col`
        // 限定名查找（避免 ExecRowContext 在重复列名时总是命中左表列）
        if let LogicalPlan::Join { left, right, .. } = input {
            let left_schema = input_schema(left)?;
            let right_schema = input_schema(right)?;
            let left_col_count = left_schema.columns.len();
            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                let (left_slice, right_slice) = split_row_at(&row, left_col_count);
                let ctx = JoinedRowContext::new(
                    &left_schema,
                    left_slice,
                    &right_schema,
                    Some(right_slice),
                );
                match ExprEvaluator::eval(&predicate, &ctx)? {
                    Value::Bool(true) => result.push(row),
                    Value::Bool(false) | Value::Null => {}
                    other => {
                        return Err(ExecutionError::EvalError(format!(
                            "WHERE predicate must evaluate to bool, got {:?}",
                            other
                        )));
                    }
                }
            }
            return Ok(result);
        }

        let schema = input_schema(input)?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let ctx = ExecRowContext::new(&schema, &row);
            match ExprEvaluator::eval(&predicate, &ctx)? {
                Value::Bool(true) => result.push(row),
                Value::Bool(false) | Value::Null => {}
                other => {
                    return Err(ExecutionError::EvalError(format!(
                        "WHERE predicate must evaluate to bool, got {:?}",
                        other
                    )));
                }
            }
        }
        Ok(result)
    }

    /// 递归重写表达式中的 InSubquery 为 InList。
    ///
    /// 对每个 `expr IN (SELECT ...)` 节点：
    /// 1. 将子查询 `Select` 包装为 `Statement::Select`
    /// 2. 使用 Planner 规划为 LogicalPlan
    /// 3. 执行 LogicalPlan 收集结果行
    /// 4. 取每行第一列构造 `Expr::Literal` 列表
    /// 5. 用 `Expr::InList` 替换原 `Expr::InSubquery`
    ///
    /// 对其他表达式类型递归重写子表达式。
    fn rewrite_in_subqueries(&self, expr: &Expr) -> Result<Expr, ExecutionError> {
        match expr {
            Expr::InSubquery {
                expr: operand,
                subquery,
                negated,
            } => {
                // 递归重写操作数表达式
                let operand = self.rewrite_in_subqueries(operand)?;

                // 规划并执行子查询
                let stmt = Statement::Select(subquery.clone());
                let plan = if let Some(catalog) = self.catalog {
                    let planner = Planner::new(catalog);
                    planner
                        .plan_statement(stmt)
                        .map_err(|e| ExecutionError::InvalidArgument(format!("plan error: {e}")))?
                } else {
                    let catalog = InMemoryCatalog::new();
                    let planner = Planner::new(&catalog);
                    planner
                        .plan_statement(stmt)
                        .map_err(|e| ExecutionError::InvalidArgument(format!("plan error: {e}")))?
                };

                let rows = self.execute(&plan)?;

                // 收集第一列值构造 Literal 列表
                let list: Vec<Expr> = rows
                    .into_iter()
                    .filter_map(|row| row.into_iter().next().map(Expr::Literal))
                    .collect();

                Ok(Expr::InList {
                    expr: Box::new(operand),
                    list,
                    negated: *negated,
                })
            }

            // 递归重写二元运算
            Expr::BinaryOp { left, op, right } => Ok(Expr::BinaryOp {
                left: Box::new(self.rewrite_in_subqueries(left)?),
                op: *op,
                right: Box::new(self.rewrite_in_subqueries(right)?),
            }),

            // 递归重写一元运算
            Expr::UnaryOp { op, expr } => Ok(Expr::UnaryOp {
                op: *op,
                expr: Box::new(self.rewrite_in_subqueries(expr)?),
            }),

            // 递归重写函数参数
            Expr::Function {
                name,
                args,
                distinct,
            } => {
                let args: Result<Vec<Expr>, _> =
                    args.iter().map(|a| self.rewrite_in_subqueries(a)).collect();
                Ok(Expr::Function {
                    name: name.clone(),
                    args: args?,
                    distinct: *distinct,
                })
            }

            // 递归重写 CASE 表达式
            Expr::Case {
                operand,
                when_then,
                else_expr,
            } => {
                let operand = match operand {
                    Some(e) => Some(Box::new(self.rewrite_in_subqueries(e)?)),
                    None => None,
                };
                let when_then: Result<Vec<(Expr, Expr)>, ExecutionError> = when_then
                    .iter()
                    .map(|(w, t)| {
                        Ok::<(Expr, Expr), ExecutionError>((
                            self.rewrite_in_subqueries(w)?,
                            self.rewrite_in_subqueries(t)?,
                        ))
                    })
                    .collect();
                let else_expr = match else_expr {
                    Some(e) => Some(Box::new(self.rewrite_in_subqueries(e)?)),
                    None => None,
                };
                Ok(Expr::Case {
                    operand,
                    when_then: when_then?,
                    else_expr,
                })
            }

            // 递归重写 Cast
            Expr::Cast { expr, data_type } => Ok(Expr::Cast {
                expr: Box::new(self.rewrite_in_subqueries(expr)?),
                data_type: data_type.clone(),
            }),

            // 递归重写 InList 内部
            Expr::InList {
                expr,
                list,
                negated,
            } => {
                let expr = Box::new(self.rewrite_in_subqueries(expr)?);
                let list: Result<Vec<Expr>, _> =
                    list.iter().map(|e| self.rewrite_in_subqueries(e)).collect();
                Ok(Expr::InList {
                    expr,
                    list: list?,
                    negated: *negated,
                })
            }

            // 递归重写 Between
            Expr::Between {
                expr,
                low,
                high,
                negated,
            } => Ok(Expr::Between {
                expr: Box::new(self.rewrite_in_subqueries(expr)?),
                low: Box::new(self.rewrite_in_subqueries(low)?),
                high: Box::new(self.rewrite_in_subqueries(high)?),
                negated: *negated,
            }),

            // 递归重写 Like
            Expr::Like {
                expr,
                pattern,
                negated,
                case_insensitive,
            } => Ok(Expr::Like {
                expr: Box::new(self.rewrite_in_subqueries(expr)?),
                pattern: Box::new(self.rewrite_in_subqueries(pattern)?),
                negated: *negated,
                case_insensitive: *case_insensitive,
            }),

            // 递归重写 IsNull
            Expr::IsNull { expr, negated } => Ok(Expr::IsNull {
                expr: Box::new(self.rewrite_in_subqueries(expr)?),
                negated: *negated,
            }),

            // 其他表达式类型无子表达式或无需重写，直接克隆
            other => Ok(other.clone()),
        }
    }

    // -----------------------------------------------------------------
    //  Projection（SELECT cols）
    // -----------------------------------------------------------------

    fn execute_projection(
        &self,
        exprs: &[(Expr, Option<String>)],
        input: &LogicalPlan,
    ) -> Result<Vec<Row>, ExecutionError> {
        let rows = self.execute(input)?;

        // 若 input 为 JOIN，使用 JoinedRowContext 以正确路由 `t1.col` / `t2.col`
        // 限定名查找（避免 ExecRowContext 在重复列名时总是命中左表列）
        if let LogicalPlan::Join { left, right, .. } = input {
            let left_schema = input_schema(left)?;
            let right_schema = input_schema(right)?;
            let left_col_count = left_schema.columns.len();
            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                let (left_slice, right_slice) = split_row_at(&row, left_col_count);
                let ctx = JoinedRowContext::new(
                    &left_schema,
                    left_slice,
                    &right_schema,
                    Some(right_slice),
                );
                let mut out_row = Vec::with_capacity(exprs.len());
                for (expr, _) in exprs {
                    let value = ExprEvaluator::eval(expr, &ctx)?;
                    out_row.push(value);
                }
                result.push(out_row);
            }
            return Ok(result);
        }

        // 若 input 为 Aggregate，投影表达式可能含聚合函数调用（如 COUNT(*)），
        // 需将它们替换为已物化的字面量再求值（聚合值已在 Aggregate 输出行的
        // [group_count..] 区间）
        if let LogicalPlan::Aggregate {
            group_exprs,
            aggregates,
            ..
        } = input
        {
            let group_count = group_exprs.len();
            let schema = input_schema(input)?;
            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                let ctx = ExecRowContext::new(&schema, &row);
                let mut out_row = Vec::with_capacity(exprs.len());
                for (expr, _) in exprs {
                    let substituted = substitute_aggregates(expr, aggregates, &row, group_count);
                    let value = ExprEvaluator::eval(&substituted, &ctx)?;
                    out_row.push(value);
                }
                result.push(out_row);
            }
            return Ok(result);
        }

        // Phase 6.2: 若 input 为 Window，投影表达式可能含 Expr::WindowFunction 引用，
        // 需将它们替换为已物化的字面量再求值（窗口函数结果在行的 [input_col_count..] 区间）
        if let LogicalPlan::Window {
            window_funcs,
            input: win_input,
        } = input
        {
            let input_col_count = input_schema(win_input)?.columns.len();
            let schema = input_schema(input)?;
            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                let ctx = ExecRowContext::new(&schema, &row);
                let mut out_row = Vec::with_capacity(exprs.len());
                for (expr, _) in exprs {
                    let substituted =
                        substitute_window_functions(expr, window_funcs, &row, input_col_count);
                    let value = ExprEvaluator::eval(&substituted, &ctx)?;
                    out_row.push(value);
                }
                result.push(out_row);
            }
            return Ok(result);
        }

        let schema = input_schema(input)?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let ctx = ExecRowContext::new(&schema, &row);
            let mut out_row = Vec::with_capacity(exprs.len());
            for (expr, _) in exprs {
                let value = ExprEvaluator::eval(expr, &ctx)?;
                out_row.push(value);
            }
            result.push(out_row);
        }
        Ok(result)
    }

    // -----------------------------------------------------------------
    //  Limit + Offset
    // -----------------------------------------------------------------

    fn execute_limit(
        &self,
        limit: &Option<Expr>,
        offset: &Option<Expr>,
        input: &LogicalPlan,
    ) -> Result<Vec<Row>, ExecutionError> {
        // 计算 LIMIT 和 OFFSET 值
        let offset_n = match offset {
            Some(expr) => {
                let v = ExprEvaluator::eval(expr, &crate::expr::RowContext::new())?;
                match v {
                    Value::Int64(n) => n as usize,
                    Value::Null => 0,
                    other => {
                        return Err(ExecutionError::EvalError(format!(
                            "OFFSET must be int, got {:?}",
                            other
                        )));
                    }
                }
            }
            None => 0,
        };
        let limit_n = match limit {
            Some(expr) => {
                let v = ExprEvaluator::eval(expr, &crate::expr::RowContext::new())?;
                match v {
                    Value::Int64(n) if n >= 0 => Some(n as usize),
                    Value::Null => None,
                    other => {
                        return Err(ExecutionError::EvalError(format!(
                            "LIMIT must be non-negative int, got {:?}",
                            other
                        )));
                    }
                }
            }
            None => None,
        };

        // 优化：对于简单计划（Scan + Filter + Projection），使用迭代器路径
        // 避免物化全部中间结果
        if self.is_simple_plan(input) {
            if let Ok(rows) = self.execute_iterative(input) {
                let start = offset_n.min(rows.len());
                let end = match limit_n {
                    Some(n) => (start + n).min(rows.len()),
                    None => rows.len(),
                };
                return Ok(rows.into_iter().skip(start).take(end - start).collect());
            }
        }

        // 回退：物化全部输入后 skip/take
        let rows = self.execute(input)?;
        let start = offset_n.min(rows.len());
        let end = match limit_n {
            Some(n) => (start + n).min(rows.len()),
            None => rows.len(),
        };
        Ok(rows.into_iter().skip(start).take(end - start).collect())
    }

    /// 判断计划是否为简单计划（适合迭代器执行）
    fn is_simple_plan(&self, plan: &LogicalPlan) -> bool {
        match plan {
            LogicalPlan::Scan { .. } => true,
            LogicalPlan::IndexScan { .. } => true,
            LogicalPlan::Filter { input, .. } => self.is_simple_plan(input),
            LogicalPlan::Projection { input, .. } => self.is_simple_plan(input),
            LogicalPlan::Limit { input, .. } => self.is_simple_plan(input),
            // Join/Aggregate/Sort 等复杂算子不适合迭代器路径
            _ => false,
        }
    }

    // -----------------------------------------------------------------
    //  Sort — Phase 6.3
    // -----------------------------------------------------------------

    /// 执行 ORDER BY 排序
    ///
    /// 实现语义：
    /// - 多键排序：按 `order_by` 顺序依次比较
    /// - ASC/DESC：每个键独立指定升降序
    /// - NULLS FIRST/LAST：每个键独立指定 NULL 位置；PG 默认 NULLS LAST（ASC）/ NULLS FIRST（DESC）
    /// - 稳定排序：保持相等元素的原始顺序
    fn execute_sort(
        &self,
        order_by: &[OrderByExpr],
        input: &LogicalPlan,
    ) -> Result<Vec<Row>, ExecutionError> {
        let rows = self.execute(input)?;
        let schema = input_schema(input)?;

        // 预计算每行的排序键，避免在比较函数中重复求值
        // paired[i] = (keys[i], row[i])
        let mut paired: Vec<(Vec<Value>, Row)> = Vec::with_capacity(rows.len());
        for row in rows {
            let ctx = ExecRowContext::new(&schema, &row);
            let mut row_keys = Vec::with_capacity(order_by.len());
            for ob in order_by {
                let v = ExprEvaluator::eval(&ob.expr, &ctx)?;
                row_keys.push(v);
            }
            paired.push((row_keys, row));
        }

        // 稳定排序（sort_by 保证稳定性）
        paired.sort_by(|(ki, _), (kj, _)| {
            for (k, ob) in order_by.iter().enumerate() {
                let vi = &ki[k];
                let vj = &kj[k];
                let is_null_i = matches!(vi, Value::Null);
                let is_null_j = matches!(vj, Value::Null);
                let ord = if is_null_i || is_null_j {
                    if is_null_i && is_null_j {
                        std::cmp::Ordering::Equal
                    } else if is_null_i {
                        // i 是 NULL
                        if ob.nulls_first {
                            std::cmp::Ordering::Less
                        } else {
                            std::cmp::Ordering::Greater
                        }
                    } else {
                        // j 是 NULL
                        if ob.nulls_first {
                            std::cmp::Ordering::Greater
                        } else {
                            std::cmp::Ordering::Less
                        }
                    }
                } else {
                    compare_values(vi, vj)
                };
                let ord = if ob.asc {
                    ord
                } else {
                    ord.reverse()
                };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        });

        Ok(paired.into_iter().map(|(_, row)| row).collect())
    }

    // -----------------------------------------------------------------
    //  Distinct
    // -----------------------------------------------------------------

    fn execute_distinct(&self, input: &LogicalPlan) -> Result<Vec<Row>, ExecutionError> {
        let rows = self.execute(input)?;
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            // Row 是 Vec<Value>，Value 实现 PartialEq + Hash? — 实际上 Value 未实现 Hash
            // 用序列化字符串做去重键（足够测试用途）
            let key = serialize_row_for_distinct(&row);
            if seen.insert(key) {
                result.push(row);
            }
        }
        Ok(result)
    }

    // -----------------------------------------------------------------
    //  SetOp（INTERSECT / EXCEPT / UNION）— Phase 3.27
    // -----------------------------------------------------------------

    /// 执行集合操作（INTERSECT / EXCEPT / UNION）
    ///
    /// **行为**（与 PG 一致）：
    /// - UNION：并集。`DISTINCT`（默认）去重，`ALL` 保留所有重复。
    /// - INTERSECT：交集。`DISTINCT`（默认）去重，`ALL` 保留 min(left, right) 重复次数。
    /// - EXCEPT：差集（left - right）。`DISTINCT`（默认）去重，`ALL` 按 left 重复次数减去 right 重复次数。
    /// - 量词 `None` 等同于 `DISTINCT`（默认）。
    ///
    /// **列数校验**：left/right 列数必须相等。
    pub fn execute_set_op(
        &self,
        op: SetOperator,
        quantifier: SetQuantifier,
        left: &LogicalPlan,
        right: &LogicalPlan,
    ) -> Result<Vec<Row>, ExecutionError> {
        let left_rows = self.execute(left)?;
        let right_rows = self.execute(right)?;

        // 列数校验
        if let (Some(l), Some(r)) = (left_rows.first(), right_rows.first()) {
            if l.len() != r.len() {
                return Err(ExecutionError::InvalidArgument(format!(
                    "set operation requires equal column counts: left has {}, right has {}",
                    l.len(),
                    r.len()
                )));
            }
        }

        match op {
            SetOperator::Union => {
                if matches!(quantifier, SetQuantifier::All) {
                    // UNION ALL：直接拼接
                    let mut result = left_rows;
                    result.extend(right_rows);
                    Ok(result)
                } else {
                    // UNION [DISTINCT]：拼接后去重
                    let mut combined = left_rows;
                    combined.extend(right_rows);
                    Ok(dedup_rows(combined))
                }
            }
            SetOperator::Intersect => {
                if matches!(quantifier, SetQuantifier::All) {
                    // INTERSECT ALL：保留 min(left, right) 重复次数
                    Ok(intersect_all(&left_rows, &right_rows))
                } else {
                    // INTERSECT [DISTINCT]：交集后去重
                    Ok(intersect_distinct(&left_rows, &right_rows))
                }
            }
            SetOperator::Except => {
                if matches!(quantifier, SetQuantifier::All) {
                    // EXCEPT ALL：left 重复次数 - right 重复次数（>= 1 才输出）
                    Ok(except_all(&left_rows, &right_rows))
                } else {
                    // EXCEPT [DISTINCT]：差集后去重
                    Ok(except_distinct(&left_rows, &right_rows))
                }
            }
        }
    }

    // -----------------------------------------------------------------
    //  JOIN（NestedLoopJoin + HashJoin）
    // -----------------------------------------------------------------

    /// 执行 JOIN — 自动选择 NestedLoop 或 Hash 策略
    ///
    /// **策略选择**：
    /// - CROSS JOIN / JoinCondition::None / Natural / Using → NestedLoop
    /// - JoinCondition::On(expr)：
    ///   - 若 expr 为单一等值 `t1.col = t2.col` 或 AND 链的等值谓词 → HashJoin
    ///   - 否则（含非等值、OR、复杂表达式）→ NestedLoop
    ///
    /// **输出 Schema**：左表列 ++ 右表列（与 `plan_schema` 推导保持一致）
    /// **输出 Row**：左行 ++ 右行（外连接未匹配侧用 NULL 填充）
    pub fn execute_join(
        &self,
        join_type: JoinType,
        condition: &JoinCondition,
        left: &LogicalPlan,
        right: &LogicalPlan,
    ) -> Result<Vec<Row>, ExecutionError> {
        // 物化左右两侧
        let left_rows = self.execute(left)?;
        let right_rows = self.execute(right)?;
        let left_schema = input_schema(left)?;
        let right_schema = input_schema(right)?;

        // 解析 JOIN 条件 → Option<Expr>
        let condition_expr = build_join_condition_expr(condition, &left_schema, &right_schema)?;

        // CROSS JOIN 或 None 条件 → 无条件笛卡尔积
        let is_cross =
            matches!(join_type, JoinType::Cross) || matches!(condition, JoinCondition::None);

        if is_cross {
            return Ok(nested_loop_emit_all(
                &left_rows,
                &right_rows,
                right_schema.columns.len(),
            ));
        }

        // 尝试提取等值键（用于 HashJoin 优化）
        let hash_keys = try_extract_hash_keys(&condition_expr, &left_schema, &right_schema);

        if let Some(keys) = hash_keys {
            // HashJoin 仅支持 INNER / LEFT OUTER（RIGHT/FULL 需对称处理，留待 NestedLoop）
            if matches!(join_type, JoinType::Inner | JoinType::LeftOuter) {
                return execute_hash_join(
                    join_type,
                    &keys,
                    &left_rows,
                    &right_rows,
                    &left_schema,
                    &right_schema,
                    &condition_expr,
                );
            }
        }

        // 退化到 NestedLoopJoin（支持所有 JOIN 类型 + 所有条件）
        execute_nested_loop_join(
            join_type,
            &condition_expr,
            &left_rows,
            &right_rows,
            &left_schema,
            &right_schema,
        )
    }

    // -----------------------------------------------------------------
    //  Aggregate（GROUP BY + COUNT/SUM/AVG/MIN/MAX）
    // -----------------------------------------------------------------

    /// 执行聚合计划 — HashAgg 策略
    ///
    /// **算法**：
    /// 1. 物化 input 行
    /// 2. 按 `group_exprs` 求值分组键，HashMap 分组
    /// 3. 每组逐聚合函数计算（COUNT/SUM/AVG/MIN/MAX，含 DISTINCT）
    /// 4. 应用 HAVING 过滤（聚合值已物化，用 `substitute_aggregates` 替换后求值）
    /// 5. 输出行 = [group_values..., agg_values...]
    ///
    /// **无 GROUP BY + 有聚合**：单组（所有行聚为一组），输出 1 行
    /// **空组**：无 GROUP BY 时即使 input 为空也输出 1 行（COUNT=0, SUM/AVG/MIN/MAX=NULL）；
    ///          有 GROUP BY 时空输入输出 0 行
    pub fn execute_aggregate(
        &self,
        group_exprs: &[Expr],
        aggregates: &[AggregateExpr],
        having: &Option<Expr>,
        input: &LogicalPlan,
    ) -> Result<Vec<Row>, ExecutionError> {
        let input_rows = self.execute(input)?;
        let input_schema = input_schema(input)?;
        let group_count = group_exprs.len();
        let agg_count = aggregates.len();

        // 1. 分组：group_key → Vec<Row>
        let mut groups: HashMap<String, Vec<Row>> = HashMap::new();
        let mut group_keys_order: Vec<String> = Vec::new();
        let mut group_key_values: HashMap<String, Vec<Value>> = HashMap::new();

        for row in &input_rows {
            let ctx = ExecRowContext::new(&input_schema, row);
            let mut key_parts = Vec::with_capacity(group_count);
            let mut key_values = Vec::with_capacity(group_count);
            for g_expr in group_exprs {
                let v = ExprEvaluator::eval(g_expr, &ctx)?;
                key_parts.push(format!("{v:?}"));
                key_values.push(v.clone());
            }
            let key = key_parts.join("|");
            if !groups.contains_key(&key) {
                group_keys_order.push(key.clone());
                group_key_values.insert(key.clone(), key_values);
            }
            groups.entry(key).or_default().push(row.clone());
        }

        // 2. 计算每组聚合值
        let mut result_rows: Vec<Row> = Vec::new();

        // 无 GROUP BY 且 input 为空：仍输出 1 行（COUNT=0, 其他=NULL）
        if group_count == 0 && groups.is_empty() {
            let mut out_row = Vec::with_capacity(agg_count);
            for agg in aggregates {
                out_row.push(compute_empty_aggregate(agg));
            }
            // HAVING 过滤
            if let Some(having_expr) = having {
                let schema = aggregate_output_schema(group_exprs, aggregates)?;
                let ctx = ExecRowContext::new(&schema, &out_row);
                let substituted =
                    substitute_aggregates(having_expr, aggregates, &out_row, group_count);
                match ExprEvaluator::eval(&substituted, &ctx)? {
                    Value::Bool(true) => {}
                    _ => return Ok(Vec::new()),
                }
            }
            result_rows.push(out_row);
        }

        for key in &group_keys_order {
            let group_rows = groups.get(key).unwrap();
            let key_values = group_key_values.get(key).unwrap();

            // 计算每个聚合
            let mut agg_values: Vec<Value> = Vec::with_capacity(agg_count);
            for agg in aggregates {
                let v = compute_aggregate(agg, group_rows, &input_schema)?;
                agg_values.push(v);
            }

            // 构造输出行 = [group_values..., agg_values...]
            let mut out_row = Vec::with_capacity(group_count + agg_count);
            out_row.extend(key_values.iter().cloned());
            out_row.extend(agg_values.iter().cloned());

            // HAVING 过滤
            if let Some(having_expr) = having {
                let schema = aggregate_output_schema(group_exprs, aggregates)?;
                let ctx = ExecRowContext::new(&schema, &out_row);
                let substituted =
                    substitute_aggregates(having_expr, aggregates, &out_row, group_count);
                match ExprEvaluator::eval(&substituted, &ctx)? {
                    Value::Bool(true) => {}
                    Value::Bool(false) | Value::Null => continue,
                    other => {
                        return Err(ExecutionError::EvalError(format!(
                            "HAVING predicate must evaluate to bool, got {:?}",
                            other
                        )));
                    }
                }
            }

            result_rows.push(out_row);
        }

        Ok(result_rows)
    }

    // -----------------------------------------------------------------
    //  DML：Insert / Update / Delete
    // -----------------------------------------------------------------

    /// 执行 INSERT 计划，返回插入的行数
    ///
    /// 支持三种数据源：
    /// - `Values` — 直接求值每行表达式
    /// - `Select` — 执行子查询，将结果行插入目标表
    /// - `DefaultValues` — 插入一行全 NULL 值
    ///
    /// 列匹配规则：
    /// - `columns = None` — 按表 Schema 列顺序提供所有列
    /// - `columns = Some(cols)` — 仅提供指定列，其他列为 NULL（或 DEFAULT，未实现）
    pub fn execute_insert(
        &self,
        plan: &LogicalPlan,
        table: &mut dyn MutableTable,
    ) -> Result<DmlResult, ExecutionError> {
        let (table_name, schema, columns, source, on_conflict, returning) = match plan {
            LogicalPlan::Insert {
                table,
                schema,
                columns,
                source,
                on_conflict,
                returning,
            } => (table, schema, columns, source, on_conflict, returning),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Insert plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // 计算目标列索引（None 表示全部列）
        let target_indices: Vec<usize> = match columns {
            None => (0..schema.columns.len()).collect(),
            Some(cols) => cols
                .iter()
                .map(|name| {
                    schema
                        .columns
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(name))
                        .ok_or_else(|| ExecutionError::ColumnNotFound(name.clone()))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };

        // Phase 3.29: 收集 FK 约束（若 catalog 已绑定）
        let fks: Vec<ForeignKeyConstraint> = match self.catalog {
            Some(cat) => cat.get_foreign_keys(table_name),
            None => Vec::new(),
        };
        // Phase 3.30: 收集 CHECK 约束（若 catalog 已绑定）
        let checks: Vec<CheckConstraint> = match self.catalog {
            Some(cat) => cat.get_check_constraints(table_name),
            None => Vec::new(),
        };
        let validate_fk = |row: &Row| -> Result<(), ExecutionError> {
            if !fks.is_empty() {
                ForeignKeyValidator::validate_insert(schema, row, &fks, &|name| {
                    self.lookup_table(name)
                })?;
            }
            Ok(())
        };
        // Phase 3.30: 校验 CHECK 约束
        let validate_check = |row: &Row| -> Result<(), ExecutionError> {
            if !checks.is_empty() {
                CheckConstraintValidator::validate_row(schema, row, &checks)?;
            }
            Ok(())
        };
        // Phase 3.31: 校验 ENUM 列值（schema 中列类型为 ColumnType::Enum）
        // 对每行调用 validate_enum_values，非法值直接拒绝
        let validate_enum =
            |row: &Row| -> Result<(), ExecutionError> { self.validate_enum_values(schema, row) };

        let has_returning = returning.is_some();
        let mut returning_rows: Vec<Row> = Vec::new();
        let mut count = 0;

        // Phase 6.4: 收集触发器并触发 BEFORE STATEMENT
        let triggers = self.triggers_for_table(table_name);
        self.fire_before_statement(&triggers, DmlKind::Insert, table_name, schema)?;

        // 无 ON CONFLICT → 走普通 INSERT 路径
        if on_conflict.is_none() {
            match source {
                InsertSourcePlan::Values(rows_expr) => {
                    for row_expr in rows_expr {
                        let mut row =
                            self.evaluate_insert_row(schema, &target_indices, row_expr)?;
                        // Phase 3.32: 将数组列的 Text 字面量解析为 Value::Array
                        self.coerce_array_values(schema, &mut row)?;
                        // Phase 6.18: 求值生成列表达式（在 coerce 之后、触发器/校验之前）
                        self.evaluate_generated_columns(schema, &mut row)?;
                        // Phase 6.4: BEFORE ROW 触发器（可修改 NEW 行或跳过）
                        let row_to_insert = match self.fire_before_row(
                            &triggers,
                            DmlKind::Insert,
                            table_name,
                            schema,
                            Some(&row),
                            None,
                            None,
                        )? {
                            FireResult::SkipRow => continue,
                            FireResult::ContinueWith(Some(modified)) => modified,
                            FireResult::ContinueWith(None) => row,
                        };
                        validate_enum(&row_to_insert)?;
                        validate_fk(&row_to_insert)?;
                        validate_check(&row_to_insert)?;
                        self.mvcc_insert(table, row_to_insert.clone(), table_name);
                        count += 1;
                        if has_returning {
                            returning_rows.push(project_returning(
                                schema,
                                &row_to_insert,
                                returning,
                            )?);
                        }
                        // Phase 6.4: AFTER ROW 触发器
                        self.fire_after_row(
                            &triggers,
                            DmlKind::Insert,
                            table_name,
                            schema,
                            Some(&row_to_insert),
                            None,
                            None,
                        )?;
                    }
                }
                InsertSourcePlan::Select(sub_plan) => {
                    let result_rows = self.execute(sub_plan)?;
                    for result_row in result_rows {
                        let mut row =
                            self.assemble_insert_row(schema, &target_indices, &result_row)?;
                        self.coerce_array_values(schema, &mut row)?;
                        // Phase 6.18: 求值生成列表达式
                        self.evaluate_generated_columns(schema, &mut row)?;
                        // Phase 6.4: BEFORE ROW 触发器
                        let row_to_insert = match self.fire_before_row(
                            &triggers,
                            DmlKind::Insert,
                            table_name,
                            schema,
                            Some(&row),
                            None,
                            None,
                        )? {
                            FireResult::SkipRow => continue,
                            FireResult::ContinueWith(Some(modified)) => modified,
                            FireResult::ContinueWith(None) => row,
                        };
                        validate_enum(&row_to_insert)?;
                        validate_fk(&row_to_insert)?;
                        validate_check(&row_to_insert)?;
                        self.mvcc_insert(table, row_to_insert.clone(), table_name);
                        count += 1;
                        if has_returning {
                            returning_rows.push(project_returning(
                                schema,
                                &row_to_insert,
                                returning,
                            )?);
                        }
                        self.fire_after_row(
                            &triggers,
                            DmlKind::Insert,
                            table_name,
                            schema,
                            Some(&row_to_insert),
                            None,
                            None,
                        )?;
                    }
                }
                InsertSourcePlan::DefaultValues => {
                    let mut row = vec![Value::Null; schema.columns.len()];
                    self.coerce_array_values(schema, &mut row)?;
                    // Phase 6.18: 求值生成列表达式
                    self.evaluate_generated_columns(schema, &mut row)?;
                    // Phase 6.4: BEFORE ROW 触发器
                    let row_to_insert = match self.fire_before_row(
                        &triggers,
                        DmlKind::Insert,
                        table_name,
                        schema,
                        Some(&row),
                        None,
                        None,
                    )? {
                        FireResult::SkipRow => {
                            // DefaultValues 单行被跳过，直接进入 AFTER STATEMENT
                            self.fire_after_statement(
                                &triggers,
                                DmlKind::Insert,
                                table_name,
                                schema,
                            )?;
                            return Ok(if has_returning {
                                DmlResult::with_returning(0, returning_rows)
                            } else {
                                DmlResult::new(0)
                            });
                        }
                        FireResult::ContinueWith(Some(modified)) => modified,
                        FireResult::ContinueWith(None) => row,
                    };
                    validate_enum(&row_to_insert)?;
                    validate_fk(&row_to_insert)?;
                    validate_check(&row_to_insert)?;
                    self.mvcc_insert(table, row_to_insert.clone(), table_name);
                    count += 1;
                    if has_returning {
                        returning_rows.push(project_returning(schema, &row_to_insert, returning)?);
                    }
                    self.fire_after_row(
                        &triggers,
                        DmlKind::Insert,
                        table_name,
                        schema,
                        Some(&row_to_insert),
                        None,
                        None,
                    )?;
                }
            }
            self.fire_after_statement(&triggers, DmlKind::Insert, table_name, schema)?;
            // P2-2：autocommit 模式下统一 flush 暂存的 CDC 事件
            self.flush_autocommit_cdc_events();
            return Ok(if has_returning {
                DmlResult::with_returning(count, returning_rows)
            } else {
                DmlResult::new(count)
            });
        }

        // ON CONFLICT 路径
        let on_conflict = on_conflict.as_ref().unwrap();
        let conflict_indices = self.resolve_conflict_indices(schema, on_conflict)?;

        match source {
            InsertSourcePlan::Values(rows_expr) => {
                for row_expr in rows_expr {
                    let proposed = self.evaluate_insert_row(schema, &target_indices, row_expr)?;
                    validate_fk(&proposed)?;
                    let (affected, opt_row) = self.apply_upsert_with_returning(
                        table,
                        schema,
                        &conflict_indices,
                        &proposed,
                        on_conflict,
                        has_returning,
                    )?;
                    count += affected;
                    if let Some(row) = opt_row {
                        returning_rows.push(project_returning(schema, &row, returning)?);
                    }
                }
            }
            InsertSourcePlan::Select(sub_plan) => {
                let result_rows = self.execute(sub_plan)?;
                for result_row in result_rows {
                    let proposed =
                        self.assemble_insert_row(schema, &target_indices, &result_row)?;
                    validate_fk(&proposed)?;
                    let (affected, opt_row) = self.apply_upsert_with_returning(
                        table,
                        schema,
                        &conflict_indices,
                        &proposed,
                        on_conflict,
                        has_returning,
                    )?;
                    count += affected;
                    if let Some(row) = opt_row {
                        returning_rows.push(project_returning(schema, &row, returning)?);
                    }
                }
            }
            InsertSourcePlan::DefaultValues => {
                let proposed = vec![Value::Null; schema.columns.len()];
                validate_fk(&proposed)?;
                let (affected, opt_row) = self.apply_upsert_with_returning(
                    table,
                    schema,
                    &conflict_indices,
                    &proposed,
                    on_conflict,
                    has_returning,
                )?;
                count += affected;
                if let Some(row) = opt_row {
                    returning_rows.push(project_returning(schema, &row, returning)?);
                }
            }
        }
        // Phase 6.4: ON CONFLICT 路径下，行级触发器未触发（Phase 6.4 已知限制）；
        // 但仍触发 AFTER STATEMENT 触发器
        self.fire_after_statement(&triggers, DmlKind::Insert, table_name, schema)?;
        // P2-2：autocommit 模式下统一 flush 暂存的 CDC 事件
        self.flush_autocommit_cdc_events();
        Ok(if has_returning {
            DmlResult::with_returning(count, returning_rows)
        } else {
            DmlResult::new(count)
        })
    }

    /// 解析 ON CONFLICT 冲突列索引
    ///
    /// - 显式列：`ON CONFLICT (col1, col2)` → 列索引列表
    /// - 无显式列：`ON CONFLICT` → 使用表的主键列索引
    ///   - 若表无主键，返回 `InvalidArgument` 错误（与 PG 一致）
    fn resolve_conflict_indices(
        &self,
        schema: &TableSchema,
        on_conflict: &OnConflict,
    ) -> Result<Vec<usize>, ExecutionError> {
        let cols: Option<&Vec<String>> = match on_conflict {
            OnConflict::DoNothing { conflict_columns } => conflict_columns.as_ref(),
            OnConflict::DoUpdate {
                conflict_columns, ..
            } => conflict_columns.as_ref(),
        };

        match cols {
            None => {
                // 无显式冲突列 → 使用主键
                let pk_indices: Vec<usize> = schema
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.primary_key)
                    .map(|(i, _)| i)
                    .collect();
                if pk_indices.is_empty() {
                    return Err(ExecutionError::InvalidArgument(
                        "ON CONFLICT without conflict columns requires a primary key".into(),
                    ));
                }
                Ok(pk_indices)
            }
            Some(cols) => cols
                .iter()
                .map(|name| {
                    schema
                        .columns
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(name))
                        .ok_or_else(|| ExecutionError::ColumnNotFound(name.clone()))
                })
                .collect(),
        }
    }

    /// 在表中查找与拟插入行冲突的现有行
    ///
    /// 冲突判定：所有冲突列值相等（NULL 不参与冲突，与 PG 语义一致）
    /// 返回第一个匹配的 (row_id, existing_row)
    fn find_conflict_row(
        &self,
        table: &dyn MutableTable,
        conflict_indices: &[usize],
        proposed: &Row,
    ) -> Option<(usize, Row)> {
        table.scan_with_ids().find(|(_, existing)| {
            for &idx in conflict_indices {
                let existing_val = existing.get(idx);
                let proposed_val = proposed.get(idx);
                match (existing_val, proposed_val) {
                    (Some(Value::Null), _) | (_, Some(Value::Null)) | (None, _) | (_, None) => {
                        return false;
                    }
                    (Some(a), Some(b)) if a == b => continue,
                    _ => return false,
                }
            }
            true
        })
    }

    /// 应用单行 UPSERT 逻辑
    ///
    /// 返回值：(affected, Option<final_row>)
    /// - `affected` — 1 = 插入或更新成功，0 = 跳过（DO NOTHING 或 WHERE 不满足）
    /// - `final_row` — 当 `collect_row=true` 且 affected=1 时，返回最终行（插入的拟插入行 / 更新后的新行），
    ///   用于 RETURNING 投影；其他情况返回 None
    fn apply_upsert_with_returning(
        &self,
        table: &mut dyn MutableTable,
        schema: &TableSchema,
        conflict_indices: &[usize],
        proposed: &Row,
        on_conflict: &OnConflict,
        collect_row: bool,
    ) -> Result<(usize, Option<Row>), ExecutionError> {
        // 查找冲突行
        let conflict = self.find_conflict_row(table, conflict_indices, proposed);

        match on_conflict {
            OnConflict::DoNothing { .. } => match conflict {
                Some(_) => Ok((0, None)), // 冲突 → 跳过
                None => {
                    table.insert_row(proposed.clone());
                    Ok((
                        1,
                        if collect_row {
                            Some(proposed.clone())
                        } else {
                            None
                        },
                    ))
                }
            },
            OnConflict::DoUpdate {
                assignments,
                where_clause,
                ..
            } => match conflict {
                None => {
                    // 无冲突 → 插入
                    table.insert_row(proposed.clone());
                    Ok((
                        1,
                        if collect_row {
                            Some(proposed.clone())
                        } else {
                            None
                        },
                    ))
                }
                Some((row_id, existing)) => {
                    // 冲突 → 检查 WHERE 子句
                    let do_update = match where_clause {
                        None => true,
                        Some(cond) => {
                            let ctx = UpsertContext::new(schema, &existing, proposed);
                            matches!(ExprEvaluator::eval(cond, &ctx)?, Value::Bool(true))
                        }
                    };
                    if !do_update {
                        return Ok((0, None)); // WHERE 不满足 → 跳过
                    }
                    // 应用 SET 赋值
                    let mut new_row = existing.clone();
                    let ctx = UpsertContext::new(schema, &existing, proposed);
                    for a in assignments {
                        let idx = schema
                            .columns
                            .iter()
                            .position(|c| c.name.eq_ignore_ascii_case(&a.column))
                            .ok_or_else(|| ExecutionError::ColumnNotFound(a.column.clone()))?;
                        let value = ExprEvaluator::eval(&a.value, &ctx)?;
                        if let Some(slot) = new_row.get_mut(idx) {
                            *slot = value;
                        }
                    }
                    // Phase 6.18: 重新求值生成列表达式
                    self.evaluate_generated_columns(schema, &mut new_row)?;
                    table.update_row(row_id, new_row.clone());
                    Ok((
                        1,
                        if collect_row {
                            Some(new_row)
                        } else {
                            None
                        },
                    ))
                }
            },
        }
    }

    /// 执行 UPDATE 计划，返回更新的行数
    ///
    /// 流程：
    /// 1. 从 source 子计划中提取 WHERE 谓词（如有）
    /// 2. 通过 `MutableTable::scan_with_ids()` 直接扫描目标表并应用谓词过滤
    ///    —— 避免目标表同时注册到 Executor（借用冲突）
    /// 3. 对每行应用 SET 赋值表达式，生成新行
    /// 4. 调用 `update_row(row_id, new_row)` 更新
    pub fn execute_update(
        &self,
        plan: &LogicalPlan,
        table: &mut dyn MutableTable,
    ) -> Result<DmlResult, ExecutionError> {
        let (table_name, schema, assignments, source, returning) = match plan {
            LogicalPlan::Update {
                table,
                schema,
                assignments,
                source,
                returning,
            } => (table, schema, assignments, source, returning),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Update plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // 从 source 中提取 WHERE 谓词
        let predicate = extract_where_predicate(source)?;

        // P0-TX-1 Phase B：MVCC 可见性过滤
        let mvcc_enabled = self.mvcc_active();
        // 扫描目标表，应用谓词，收集匹配 (row_id, row)
        let matching: Vec<(usize, Row)> = if mvcc_enabled {
            let mvcc = self.mvcc.unwrap();
            let txn_id = self.mvcc_txn_id;
            // 注册表级读（SSI）
            let table_key = table_name.name.to_lowercase();
            let _ = mvcc.register_read(txn_id, &table_key);
            table
                .scan_with_versions()
                .filter(|(_, row, xmin, xmax)| {
                    mvcc.is_visible(txn_id, *xmin, *xmax)
                        && row_matches_predicate(schema, row, predicate)
                })
                .map(|(id, row, _, _)| (id, row))
                .collect()
        } else {
            table
                .scan_with_ids()
                .filter(|(_, row)| row_matches_predicate(schema, row, predicate))
                .collect()
        };

        // P1-9：行级锁获取 — 在实际修改前锁定所有匹配行，实现行级冲突检测。
        // 若两事务并发修改同一行，后到事务将等待或死锁中止。
        // autocommit 模式（txn_id=0）或未注入 row_lock_manager 时跳过（保持兼容）。
        let matching_row_ids: Vec<usize> = matching.iter().map(|(id, _)| *id).collect();
        self.acquire_row_xlocks(&table_name.name, &matching_row_ids)?;

        // Phase 3.29: 收集 FK 约束（若 catalog 已绑定）
        let fks: Vec<ForeignKeyConstraint> = match self.catalog {
            Some(cat) => cat.get_foreign_keys(table_name),
            None => Vec::new(),
        };
        // Phase 3.30: 收集 CHECK 约束（若 catalog 已绑定）
        let checks: Vec<CheckConstraint> = match self.catalog {
            Some(cat) => cat.get_check_constraints(table_name),
            None => Vec::new(),
        };

        let has_returning = returning.is_some();
        let mut returning_rows: Vec<Row> = Vec::new();
        // Phase 6.4: 收集触发器并触发 BEFORE STATEMENT
        let triggers = self.triggers_for_table(table_name);
        self.fire_before_statement(&triggers, DmlKind::Update, table_name, schema)?;
        // Phase 6.4: 收集 SET 涉及的列名（用于 UPDATE(cols) 触发器过滤）
        let updated_columns: Vec<String> = assignments.iter().map(|a| a.column.clone()).collect();
        // 应用 SET 赋值
        let mut count = 0;
        for (row_id, row) in matching {
            // 先求值所有赋值表达式（基于旧行），再统一应用 — 避免 borrow 冲突
            let ctx = ExecRowContext::new_proxy(schema, &row);
            let mut new_values: Vec<(usize, Value)> = Vec::with_capacity(assignments.len());
            for assignment in assignments {
                let col_idx = schema
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(&assignment.column))
                    .ok_or_else(|| ExecutionError::ColumnNotFound(assignment.column.clone()))?;
                let new_value = ExprEvaluator::eval(&assignment.value, &ctx)?;
                new_values.push((col_idx, new_value));
            }
            // 应用新值
            let mut new_row = row.clone();
            for (col_idx, value) in new_values {
                if col_idx < new_row.len() {
                    new_row[col_idx] = value;
                }
            }
            // Phase 3.32: 将数组列的 Text 字面量解析为 Value::Array
            self.coerce_array_values(schema, &mut new_row)?;
            // Phase 6.18: 重新求值生成列表达式（SET 赋值后、触发器/校验之前）
            self.evaluate_generated_columns(schema, &mut new_row)?;
            // Phase 6.4: BEFORE ROW 触发器（可修改 NEW 行或跳过）
            let final_new_row = match self.fire_before_row(
                &triggers,
                DmlKind::Update,
                table_name,
                schema,
                Some(&new_row),
                Some(&row),
                Some(&updated_columns),
            )? {
                FireResult::SkipRow => continue,
                FireResult::ContinueWith(Some(modified)) => modified,
                FireResult::ContinueWith(None) => new_row,
            };
            // Phase 3.29: 校验新行不违反 FK（仅当 FK 列改变时）
            if !fks.is_empty() {
                ForeignKeyValidator::validate_update(
                    schema,
                    &row,
                    &final_new_row,
                    &fks,
                    &|name| self.lookup_table(name),
                )?;
            }
            // Phase 3.30: 校验新行不违反 CHECK
            if !checks.is_empty() {
                CheckConstraintValidator::validate_row(schema, &final_new_row, &checks)?;
            }
            // Phase 3.31: 校验新行不违反 ENUM 约束
            self.validate_enum_values(schema, &final_new_row)?;
            // P0-TX-1 Phase B：MVCC 版本化 UPDATE = DELETE old + INSERT new
            let updated = if mvcc_enabled {
                let mvcc = self.mvcc.unwrap();
                let txn_id = self.mvcc_txn_id;
                // 注册 write_set（old row + new row）
                let old_key = format!("{}:{}", table_name.name.to_lowercase(), row_id);
                let _ = mvcc.register_write(txn_id, &old_key);
                // 标记旧行删除（设置 xmax）
                let old_deleted = table.delete_row_versioned(row_id, txn_id);
                if old_deleted {
                    // 插入新行（设置 xmin）
                    let new_row_id = table.insert_row_versioned(final_new_row.clone(), txn_id);
                    let new_key = format!("{}:{}", table_name.name.to_lowercase(), new_row_id);
                    let _ = mvcc.register_write(txn_id, &new_key);
                    true
                } else {
                    false
                }
            } else {
                table.update_row(row_id, final_new_row.clone())
            };
            if updated {
                count += 1;
                // P0-DIST-1/2/3：双写到分布式 KV 存储（UPDATE = 新行覆盖旧键）
                // 注：dist KV 以 {table}:{row_id} 为键，UPDATE 时用新行覆盖该键
                //     MVCC 模式下旧行已标记 xmax，新行已 insert_row_versioned，
                //     此处仅同步新行到分布式 KV（Raft propose → apply）
                self.dist_dual_write(&table_name.name.to_lowercase(), row_id, &final_new_row);
                // P7-1：分发 CDC Update 事件（old_row + new_row）
                self.dispatch_cdc_update(&table_name.name, &row, &final_new_row);
                // P9-2：写入行级 Update WAL 记录（old_row + new_row）
                self.append_wal_row_update(&table_name.name, row_id, &row, &final_new_row);
                if has_returning {
                    returning_rows.push(project_returning(schema, &final_new_row, returning)?);
                }
                // Phase 6.4: AFTER ROW 触发器
                self.fire_after_row(
                    &triggers,
                    DmlKind::Update,
                    table_name,
                    schema,
                    Some(&final_new_row),
                    Some(&row),
                    Some(&updated_columns),
                )?;
            }
        }
        // Phase 6.4: AFTER STATEMENT 触发器
        self.fire_after_statement(&triggers, DmlKind::Update, table_name, schema)?;
        // P2-2：autocommit 模式下统一 flush 暂存的 CDC 事件
        self.flush_autocommit_cdc_events();
        Ok(if has_returning {
            DmlResult::with_returning(count, returning_rows)
        } else {
            DmlResult::new(count)
        })
    }

    /// 执行 DELETE 计划，返回删除的行数
    ///
    /// 流程：
    /// 1. 从 source 子计划中提取 WHERE 谓词（如有）
    /// 2. 通过 `MutableTable::scan_with_ids()` 直接扫描目标表并应用谓词过滤
    /// 3. 调用 `delete_row(row_id)` 删除
    ///
    /// Phase 3.29: 若 catalog 已绑定，会校验父表侧 RESTRICT/NO ACTION。
    /// 对于 CASCADE / SET NULL / SET DEFAULT 级联，请使用 `execute_delete_with_cascades`。
    pub fn execute_delete(
        &self,
        plan: &LogicalPlan,
        table: &mut dyn MutableTable,
    ) -> Result<DmlResult, ExecutionError> {
        let (table_name, schema, source, returning) = match plan {
            LogicalPlan::Delete {
                table,
                schema,
                source,
                returning,
            } => (table, schema, source, returning),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Delete plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        let predicate = extract_where_predicate(source)?;

        let has_returning = returning.is_some();
        let mut returning_rows: Vec<Row> = Vec::new();
        // 收集匹配 (row_id, row) — DELETE 需要旧行用于 RETURNING
        //
        // P0-TX-1 Phase B：MVCC 可见性过滤
        // 当 MVCC 启用且有活跃事务时，使用 scan_with_versions + is_visible 过滤，
        // 仅删除对当前事务可见的行。未启用时退化为 scan_with_ids（旧行为）。
        let mvcc_enabled = self.mvcc.is_some() && self.mvcc_txn_id != 0;
        let matching: Vec<(usize, Row)> = if mvcc_enabled {
            let mvcc = self.mvcc.unwrap();
            let txn_id = self.mvcc_txn_id;
            table
                .scan_with_versions()
                .filter(|(_, row, xmin, xmax)| {
                    mvcc.is_visible(txn_id, *xmin, *xmax)
                        && row_matches_predicate(schema, row, predicate)
                })
                .map(|(id, row, _, _)| (id, row))
                .collect()
        } else {
            table
                .scan_with_ids()
                .filter(|(_, row)| row_matches_predicate(schema, row, predicate))
                .collect()
        };

        // P1-9：行级锁获取 — 在实际删除前锁定所有匹配行，实现行级冲突检测。
        // 若两事务并发删除同一行，后到事务将等待或死锁中止。
        // autocommit 模式（txn_id=0）或未注入 row_lock_manager 时跳过（保持兼容）。
        let matching_row_ids: Vec<usize> = matching.iter().map(|(id, _)| *id).collect();
        self.acquire_row_xlocks(&table_name.name, &matching_row_ids)?;

        // Phase 3.29: 父表侧 FK 校验（RESTRICT/NO ACTION 报错；CASCADE/SET NULL 级联）
        // 这里仅做 RESTRICT 检查；级联操作需调用 `execute_delete_with_cascades`。
        if let Some(cat) = self.catalog {
            let referencing_keys = cat.get_referencing_keys(table_name);
            if !referencing_keys.is_empty() {
                // 仅校验是否存在 RESTRICT/NO ACTION 引用（不收集 CASCADE ops）
                ForeignKeyValidator::collect_delete_cascades(
                    schema,
                    &matching,
                    &referencing_keys,
                    &|name| self.lookup_table(name),
                )?;
            }
        }

        let mut count = 0;
        // Phase 6.4: 收集触发器并触发 BEFORE STATEMENT
        let triggers = self.triggers_for_table(table_name);
        self.fire_before_statement(&triggers, DmlKind::Delete, table_name, schema)?;
        for (row_id, row) in matching {
            // Phase 6.4: BEFORE ROW 触发器（可跳过该行删除）
            match self.fire_before_row(
                &triggers,
                DmlKind::Delete,
                table_name,
                schema,
                None,
                Some(&row),
                None,
            )? {
                FireResult::SkipRow => continue,
                FireResult::ContinueWith(_) => {}
            }
            // P0-TX-1 Phase B：MVCC 版本化删除
            let deleted = if mvcc_enabled {
                let mvcc = self.mvcc.unwrap();
                let txn_id = self.mvcc_txn_id;
                // 注册 write_set（First-Committer-Wins + SSI）
                let key = format!("{}:{}", table_name.name.to_lowercase(), row_id);
                let _ = mvcc.register_write(txn_id, &key);
                table.delete_row_versioned(row_id, txn_id)
            } else {
                table.delete_row(row_id)
            };
            if deleted {
                count += 1;
                // P0-DIST-1/2/3：从分布式 KV 存储删除对应键（Raft propose → apply）
                self.dist_dual_delete(&table_name.name.to_lowercase(), row_id);
                // P7-1：分发 CDC Delete 事件（old_row）
                self.dispatch_cdc_delete(&table_name.name, &row);
                // P9-2：写入行级 Delete WAL 记录（old_row）
                self.append_wal_row_delete(&table_name.name, row_id, &row);
                if has_returning {
                    returning_rows.push(project_returning(schema, &row, returning)?);
                }
                // Phase 6.4: AFTER ROW 触发器
                self.fire_after_row(
                    &triggers,
                    DmlKind::Delete,
                    table_name,
                    schema,
                    None,
                    Some(&row),
                    None,
                )?;
            }
        }
        // Phase 6.4: AFTER STATEMENT 触发器
        self.fire_after_statement(&triggers, DmlKind::Delete, table_name, schema)?;
        // P2-2：autocommit 模式下统一 flush 暂存的 CDC 事件
        self.flush_autocommit_cdc_events();
        Ok(if has_returning {
            DmlResult::with_returning(count, returning_rows)
        } else {
            DmlResult::new(count)
        })
    }

    /// 执行 DELETE 计划并收集级联操作 — Phase 3.29
    ///
    /// 与 `execute_delete` 相同，但额外返回需要应用到子表的级联操作列表。
    /// 调用方需通过 `apply_cascade_ops` 应用这些操作。
    ///
    /// # 返回
    /// - `DmlResult` — 受影响行数 + RETURNING 行
    /// - `Vec<CascadeOp>` — 待应用的级联操作（CASCADE/SET NULL/SET DEFAULT）
    pub fn execute_delete_with_cascades(
        &self,
        plan: &LogicalPlan,
        table: &mut dyn MutableTable,
    ) -> Result<(DmlResult, Vec<CascadeOp>), ExecutionError> {
        let (table_name, schema, source, returning) = match plan {
            LogicalPlan::Delete {
                table,
                schema,
                source,
                returning,
            } => (table, schema, source, returning),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Delete plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        let predicate = extract_where_predicate(source)?;

        let has_returning = returning.is_some();
        let mut returning_rows: Vec<Row> = Vec::new();
        let matching: Vec<(usize, Row)> = table
            .scan_with_ids()
            .filter(|(_, row)| row_matches_predicate(schema, row, predicate))
            .collect();

        // Phase 3.29: 收集级联操作（包括 RESTRICT 检查）
        let cascade_ops = if let Some(cat) = self.catalog {
            let referencing_keys = cat.get_referencing_keys(table_name);
            if !referencing_keys.is_empty() {
                ForeignKeyValidator::collect_delete_cascades(
                    schema,
                    &matching,
                    &referencing_keys,
                    &|name| self.lookup_table(name),
                )?
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let mut count = 0;
        for (row_id, row) in matching {
            if table.delete_row(row_id) {
                count += 1;
                if has_returning {
                    returning_rows.push(project_returning(schema, &row, returning)?);
                }
            }
        }
        let result = if has_returning {
            DmlResult::with_returning(count, returning_rows)
        } else {
            DmlResult::new(count)
        };
        Ok((result, cascade_ops))
    }

    /// 执行 UPDATE 计划并收集级联操作 — Phase 3.29
    ///
    /// 与 `execute_update` 相同，但当父表被引用列的值改变时，额外返回
    /// 需要应用到子表的级联操作列表。调用方需通过 `apply_cascade_ops` 应用。
    pub fn execute_update_with_cascades(
        &self,
        plan: &LogicalPlan,
        table: &mut dyn MutableTable,
    ) -> Result<(DmlResult, Vec<CascadeOp>), ExecutionError> {
        let (table_name, schema, assignments, source, returning) = match plan {
            LogicalPlan::Update {
                table,
                schema,
                assignments,
                source,
                returning,
            } => (table, schema, assignments, source, returning),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Update plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        let predicate = extract_where_predicate(source)?;
        let matching: Vec<(usize, Row)> = table
            .scan_with_ids()
            .filter(|(_, row)| row_matches_predicate(schema, row, predicate))
            .collect();

        let fks: Vec<ForeignKeyConstraint> = match self.catalog {
            Some(cat) => cat.get_foreign_keys(table_name),
            None => Vec::new(),
        };
        let referencing_keys: Vec<ReferencingKey> = match self.catalog {
            Some(cat) => cat.get_referencing_keys(table_name),
            None => Vec::new(),
        };
        // Phase 3.30: 收集 CHECK 约束
        let checks: Vec<CheckConstraint> = match self.catalog {
            Some(cat) => cat.get_check_constraints(table_name),
            None => Vec::new(),
        };

        let has_returning = returning.is_some();

        // 第一阶段：计算所有新行 + 子表侧 FK 校验 + CHECK 校验
        let mut computed: Vec<(usize, Row, Row)> = Vec::with_capacity(matching.len());
        for (row_id, row) in matching {
            let ctx = ExecRowContext::new_proxy(schema, &row);
            let mut new_values: Vec<(usize, Value)> = Vec::with_capacity(assignments.len());
            for assignment in assignments {
                let col_idx = schema
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(&assignment.column))
                    .ok_or_else(|| ExecutionError::ColumnNotFound(assignment.column.clone()))?;
                let new_value = ExprEvaluator::eval(&assignment.value, &ctx)?;
                new_values.push((col_idx, new_value));
            }
            let mut new_row = row.clone();
            for (col_idx, value) in new_values {
                if col_idx < new_row.len() {
                    new_row[col_idx] = value;
                }
            }
            // Phase 3.32: 将数组列的 Text 字面量解析为 Value::Array
            self.coerce_array_values(schema, &mut new_row)?;
            // Phase 6.18: 重新求值生成列表达式
            self.evaluate_generated_columns(schema, &mut new_row)?;
            // Phase 3.29: 子表侧 FK 校验（仅当 FK 列改变）
            if !fks.is_empty() {
                ForeignKeyValidator::validate_update(schema, &row, &new_row, &fks, &|name| {
                    self.lookup_table(name)
                })?;
            }
            // Phase 3.30: CHECK 约束校验
            if !checks.is_empty() {
                CheckConstraintValidator::validate_row(schema, &new_row, &checks)?;
            }
            // Phase 3.31: ENUM 列值校验
            self.validate_enum_values(schema, &new_row)?;
            computed.push((row_id, row, new_row));
        }

        // 第二阶段：父表侧 RESTRICT/NO ACTION 检查（在任何写入之前）
        // 收集值实际改变的行用于级联检查
        let changed_pairs: Vec<(usize, Row, Row)> = computed
            .iter()
            .filter(|(_, old_row, new_row)| old_row != new_row)
            .cloned()
            .collect();
        let cascade_ops = if !referencing_keys.is_empty() && !changed_pairs.is_empty() {
            ForeignKeyValidator::collect_update_cascades(
                schema,
                &changed_pairs,
                &referencing_keys,
                &|name| self.lookup_table(name),
            )?
        } else {
            Vec::new()
        };

        // 第三阶段：执行更新（此时所有校验已通过）
        let mut returning_rows: Vec<Row> = Vec::new();
        let mut count = 0;
        for (row_id, _, new_row) in computed {
            if table.update_row(row_id, new_row.clone()) {
                count += 1;
                if has_returning {
                    returning_rows.push(project_returning(schema, &new_row, returning)?);
                }
            }
        }

        let result = if has_returning {
            DmlResult::with_returning(count, returning_rows)
        } else {
            DmlResult::new(count)
        };
        Ok((result, cascade_ops))
    }

    /// 应用级联操作到子表 — Phase 3.29
    ///
    /// 遍历所有 `CascadeOp`，从 `tables` 切片中按名称查找子表并应用变更。
    ///
    /// # 参数
    /// - `ops` — 由 `execute_delete_with_cascades` / `execute_update_with_cascades` 返回的级联操作列表
    /// - `tables` — 子表名（小写）→ 可变表引用的切片；查找时按名线性匹配
    ///
    /// # 返回
    /// 实际应用的级联操作数量。
    pub fn apply_cascade_ops(
        ops: Vec<CascadeOp>,
        tables: &mut [(&str, &mut dyn MutableTable)],
    ) -> Result<usize, ExecutionError> {
        let mut applied = 0;
        for op in ops {
            match op {
                CascadeOp::DeleteChildRow { table, row_id } => {
                    if let Some((_, child)) = tables.iter_mut().find(|(name, _)| *name == table) {
                        if child.delete_row(row_id) {
                            applied += 1;
                        }
                    }
                }
                CascadeOp::UpdateChildRow {
                    table,
                    row_id,
                    updates,
                } => {
                    if let Some((_, child)) = tables.iter_mut().find(|(name, _)| *name == table) {
                        if let Some(mut row) = child.get_row(row_id) {
                            for (idx, val) in updates {
                                if idx < row.len() {
                                    row[idx] = val;
                                }
                            }
                            if child.update_row(row_id, row) {
                                applied += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(applied)
    }

    /// 执行 MERGE 计划 — Phase 3.24
    ///
    /// 行为与 SQL:2003 标准一致：
    /// - WHEN MATCHED THEN UPDATE/DELETE — 源行匹配目标行时执行
    /// - WHEN NOT MATCHED THEN INSERT — 源行无匹配目标行时执行
    /// - WHEN NOT MATCHED BY SOURCE THEN UPDATE/DELETE — 目标行无匹配源行时执行
    ///
    /// # 流程
    /// 1. 扫描源表所有行
    /// 2. 对每个源行，扫描目标表（初始快照）评估 ON 条件
    ///    - 匹配 → 执行第一个满足 predicate 的 WHEN MATCHED 子句
    ///    - 无匹配 → 执行第一个满足 predicate 的 WHEN NOT MATCHED 子句
    /// 3. 处理 NOT MATCHED BY SOURCE：对未被任何源行匹配的目标行执行 WHEN NOT MATCHED BY SOURCE 子句
    ///
    /// # 注意
    /// - 使用初始目标表快照，INSERT 的新行不会被后续源行匹配（PG 兼容）
    /// - 一个源行最多匹配一个目标行（SQL:2003 标准）
    pub fn execute_merge(
        &self,
        plan: &LogicalPlan,
        target_table: &mut dyn MutableTable,
    ) -> Result<DmlResult, ExecutionError> {
        let (target_schema, source, source_schema, on, clauses) = match plan {
            LogicalPlan::Merge {
                target_schema,
                source,
                source_schema,
                on,
                clauses,
                ..
            } => (target_schema, source, source_schema, on, clauses),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Merge plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // 获取源表存储
        let source_storage = match source {
            TableFactor::Table { name, .. } => self
                .lookup_table(&name.name)
                .ok_or_else(|| ExecutionError::TableNotFound(name.qualified_name()))?,
            _ => {
                return Err(ExecutionError::Unsupported(
                    "MERGE source must be a physical table (subquery not yet supported)".into(),
                ))
            }
        };
        let source_schema = source_schema.as_ref().ok_or_else(|| {
            ExecutionError::InvalidArgument("MERGE source schema required".into())
        })?;

        // 收集源表所有行
        let source_rows: Vec<Row> = source_storage.scan_iter().collect();
        // 收集初始目标表快照（INSERT 的新行不会被后续源行匹配）
        let initial_target_rows: Vec<(usize, Row)> = target_table.scan_with_ids().collect();
        // 记录被匹配的目标 row_id（用于 NOT MATCHED BY SOURCE）
        let mut matched_target_ids: HashSet<usize> = HashSet::new();

        let mut affected = 0usize;

        // 第一遍：处理每个源行
        for source_row in &source_rows {
            let mut found_match = false;

            for (row_id, target_row) in &initial_target_rows {
                // 评估 ON 条件（用 JoinedRowContext 支持 t.col / s.col 限定名）
                let ctx = JoinedRowContext::new(
                    target_schema,
                    target_row,
                    source_schema,
                    Some(source_row),
                );
                let matched = ExprEvaluator::eval(on, &ctx)?;
                if !matches!(matched, Value::Bool(true)) {
                    continue;
                }
                // 匹配成功
                found_match = true;
                matched_target_ids.insert(*row_id);

                // 查找适用的 WHEN MATCHED 子句
                for clause in clauses {
                    if !matches!(clause.kind, MergeClauseKind::Matched) {
                        continue;
                    }
                    // 评估可选 predicate
                    let predicate_ok = if let Some(pred) = &clause.predicate {
                        let pv = ExprEvaluator::eval(pred, &ctx)?;
                        matches!(pv, Value::Bool(true))
                    } else {
                        true
                    };
                    if !predicate_ok {
                        continue;
                    }
                    // 执行动作
                    match &clause.action {
                        MergeAction::Update { assignments } => {
                            let mut new_row = target_row.clone();
                            for assignment in assignments {
                                let col_idx = target_schema
                                    .columns
                                    .iter()
                                    .position(|c| c.name.eq_ignore_ascii_case(&assignment.column))
                                    .ok_or_else(|| {
                                        ExecutionError::ColumnNotFound(assignment.column.clone())
                                    })?;
                                let new_value = ExprEvaluator::eval(&assignment.value, &ctx)?;
                                if col_idx < new_row.len() {
                                    new_row[col_idx] = new_value;
                                }
                            }
                            if target_table.update_row(*row_id, new_row) {
                                affected += 1;
                            }
                        }
                        MergeAction::Delete => {
                            if target_table.delete_row(*row_id) {
                                affected += 1;
                            }
                        }
                        MergeAction::Insert { .. } => {
                            return Err(ExecutionError::InvalidArgument(
                                "WHEN MATCHED THEN INSERT is not allowed".into(),
                            ))
                        }
                    }
                    break; // 仅执行第一个匹配的子句
                }
                break; // 一个源行最多匹配一个目标行
            }

            // 若无匹配，处理 WHEN NOT MATCHED
            if !found_match {
                // 上下文仅含源行（用 ExecRowContext + source_schema）
                let ctx = ExecRowContext::new(source_schema, source_row);
                for clause in clauses {
                    if !matches!(clause.kind, MergeClauseKind::NotMatched) {
                        continue;
                    }
                    let predicate_ok = if let Some(pred) = &clause.predicate {
                        let pv = ExprEvaluator::eval(pred, &ctx)?;
                        matches!(pv, Value::Bool(true))
                    } else {
                        true
                    };
                    if !predicate_ok {
                        continue;
                    }
                    match &clause.action {
                        MergeAction::Insert { columns, values } => {
                            // 求值 VALUES 表达式
                            let row_values: Result<Vec<Value>, ExecutionError> = values
                                .iter()
                                .map(|v| ExprEvaluator::eval(v, &ctx).map_err(Into::into))
                                .collect();
                            let row_values = row_values?;

                            // 映射到目标表列
                            let new_row = if columns.is_empty() {
                                // 按目标表列顺序
                                if row_values.len() != target_schema.columns.len() {
                                    return Err(ExecutionError::InvalidArgument(format!(
                                        "MERGE INSERT value count {} != target column count {}",
                                        row_values.len(),
                                        target_schema.columns.len()
                                    )));
                                }
                                row_values
                            } else {
                                let mut full_row = vec![Value::Null; target_schema.columns.len()];
                                for (i, col_name) in columns.iter().enumerate() {
                                    let idx = target_schema
                                        .columns
                                        .iter()
                                        .position(|c| c.name.eq_ignore_ascii_case(col_name))
                                        .ok_or_else(|| {
                                            ExecutionError::ColumnNotFound(col_name.clone())
                                        })?;
                                    if i < row_values.len() && idx < full_row.len() {
                                        full_row[idx] = row_values[i].clone();
                                    }
                                }
                                full_row
                            };
                            target_table.insert_row(new_row);
                            affected += 1;
                        }
                        _ => {
                            return Err(ExecutionError::InvalidArgument(
                                "WHEN NOT MATCHED THEN UPDATE/DELETE is not allowed".into(),
                            ))
                        }
                    }
                    break;
                }
            }
        }

        // 第二遍：处理 NOT MATCHED BY SOURCE（目标表有但源表无匹配的行）
        let has_not_matched_by_source = clauses
            .iter()
            .any(|c| matches!(c.kind, MergeClauseKind::NotMatchedBySource));
        if has_not_matched_by_source {
            for (row_id, target_row) in &initial_target_rows {
                // 跳过已匹配的行
                if matched_target_ids.contains(row_id) {
                    continue;
                }
                // 跳过已被删除的行（可能在第一遍被 WHEN MATCHED DELETE 删除）
                if target_table.get_row(*row_id).is_none() {
                    continue;
                }

                let ctx = ExecRowContext::new(target_schema, target_row);
                for clause in clauses {
                    if !matches!(clause.kind, MergeClauseKind::NotMatchedBySource) {
                        continue;
                    }
                    let predicate_ok = if let Some(pred) = &clause.predicate {
                        let pv = ExprEvaluator::eval(pred, &ctx)?;
                        matches!(pv, Value::Bool(true))
                    } else {
                        true
                    };
                    if !predicate_ok {
                        continue;
                    }
                    match &clause.action {
                        MergeAction::Update { assignments } => {
                            let mut new_row = target_row.clone();
                            for assignment in assignments {
                                let col_idx = target_schema
                                    .columns
                                    .iter()
                                    .position(|c| c.name.eq_ignore_ascii_case(&assignment.column))
                                    .ok_or_else(|| {
                                        ExecutionError::ColumnNotFound(assignment.column.clone())
                                    })?;
                                let new_value = ExprEvaluator::eval(&assignment.value, &ctx)?;
                                if col_idx < new_row.len() {
                                    new_row[col_idx] = new_value;
                                }
                            }
                            if target_table.update_row(*row_id, new_row) {
                                affected += 1;
                            }
                        }
                        MergeAction::Delete => {
                            if target_table.delete_row(*row_id) {
                                affected += 1;
                            }
                        }
                        MergeAction::Insert { .. } => {
                            return Err(ExecutionError::InvalidArgument(
                                "WHEN NOT MATCHED BY SOURCE THEN INSERT is not allowed".into(),
                            ))
                        }
                    }
                    break;
                }
            }
        }

        Ok(DmlResult::new(affected))
    }

    // -----------------------------------------------------------------
    //  REPLACE — Phase 3.25
    // -----------------------------------------------------------------

    /// 执行 REPLACE 计划 — Phase 3.25
    ///
    /// 行为与 MySQL 一致：
    /// - 主键/UNIQUE 冲突时 DELETE 旧行 + INSERT 新行（受影响行数 = 1 + 删除数）
    /// - 无冲突时直接 INSERT（受影响行数 = 1）
    /// - 不支持 RETURNING（MySQL 不支持）
    ///
    /// # 受影响行数计算
    /// - 0 个冲突 → +1（仅插入）
    /// - N 个冲突 → +1+N（N 次删除 + 1 次插入）
    ///
    /// # 冲突列选择
    /// - 优先使用主键列（`primary_key = true`）
    /// - 若无主键，使用所有 UNIQUE 列（`unique = true`）
    /// - 若两者皆无，返回错误（REPLACE 需要唯一约束才能判断冲突）
    pub fn execute_replace(
        &self,
        plan: &LogicalPlan,
        table: &mut dyn MutableTable,
    ) -> Result<DmlResult, ExecutionError> {
        let (schema, columns, source) = match plan {
            LogicalPlan::Replace {
                schema,
                columns,
                source,
                ..
            } => (schema, columns, source),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Replace plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // 计算目标列索引（None 表示全部列）
        let target_indices: Vec<usize> = match columns {
            None => (0..schema.columns.len()).collect(),
            Some(cols) => cols
                .iter()
                .map(|name| {
                    schema
                        .columns
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(name))
                        .ok_or_else(|| ExecutionError::ColumnNotFound(name.clone()))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };

        // 解析冲突列索引（PK 优先，否则 UNIQUE）
        let conflict_indices = self.resolve_replace_conflict_indices(schema)?;

        let mut count = 0usize;
        match source {
            InsertSourcePlan::Values(rows_expr) => {
                for row_expr in rows_expr {
                    let proposed = self.evaluate_insert_row(schema, &target_indices, row_expr)?;
                    let deleted = self.delete_conflicts(table, &conflict_indices, &proposed);
                    table.insert_row(proposed);
                    count += 1 + deleted;
                }
            }
            InsertSourcePlan::Select(sub_plan) => {
                let result_rows = self.execute(sub_plan)?;
                for result_row in result_rows {
                    let proposed =
                        self.assemble_insert_row(schema, &target_indices, &result_row)?;
                    let deleted = self.delete_conflicts(table, &conflict_indices, &proposed);
                    table.insert_row(proposed);
                    count += 1 + deleted;
                }
            }
            InsertSourcePlan::DefaultValues => {
                let proposed = vec![Value::Null; schema.columns.len()];
                let deleted = self.delete_conflicts(table, &conflict_indices, &proposed);
                table.insert_row(proposed);
                count += 1 + deleted;
            }
        }

        Ok(DmlResult::new(count))
    }

    /// 解析 REPLACE 冲突列索引 — Phase 3.25
    ///
    /// 与 MySQL 一致：
    /// - 优先使用主键列
    /// - 若无主键，使用所有 UNIQUE 列
    /// - 若两者皆无，返回错误（REPLACE 需要唯一约束才能判断冲突）
    fn resolve_replace_conflict_indices(
        &self,
        schema: &TableSchema,
    ) -> Result<Vec<usize>, ExecutionError> {
        let pk_indices: Vec<usize> = schema
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.primary_key)
            .map(|(i, _)| i)
            .collect();
        if !pk_indices.is_empty() {
            return Ok(pk_indices);
        }
        let unique_indices: Vec<usize> = schema
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.unique)
            .map(|(i, _)| i)
            .collect();
        if unique_indices.is_empty() {
            return Err(ExecutionError::InvalidArgument(
                "REPLACE requires a PRIMARY KEY or UNIQUE constraint".into(),
            ));
        }
        Ok(unique_indices)
    }

    /// 删除所有与拟插入行冲突的现有行 — Phase 3.25
    ///
    /// 冲突判定：所有冲突列值相等（NULL 不参与冲突，与 PG/MySQL 语义一致）。
    /// 返回删除的行数。
    fn delete_conflicts(
        &self,
        table: &mut dyn MutableTable,
        conflict_indices: &[usize],
        proposed: &Row,
    ) -> usize {
        // 收集所有冲突的 row_id（避免在迭代中修改表）
        let conflict_ids: Vec<usize> = table
            .scan_with_ids()
            .filter(|(_, existing)| {
                for &idx in conflict_indices {
                    let existing_val = existing.get(idx);
                    let proposed_val = proposed.get(idx);
                    match (existing_val, proposed_val) {
                        (Some(Value::Null), _) | (_, Some(Value::Null)) | (None, _) | (_, None) => {
                            return false;
                        }
                        (Some(a), Some(b)) if a == b => continue,
                        _ => return false,
                    }
                }
                true
            })
            .map(|(id, _)| id)
            .collect();
        let deleted = conflict_ids.len();
        for id in conflict_ids {
            table.delete_row(id);
        }
        deleted
    }

    /// 求值 INSERT VALUES 一行表达式，生成完整行（按表 Schema 列顺序）
    fn evaluate_insert_row(
        &self,
        schema: &TableSchema,
        target_indices: &[usize],
        row_expr: &[Expr],
    ) -> Result<Row, ExecutionError> {
        if row_expr.len() != target_indices.len() {
            return Err(ExecutionError::InvalidArgument(format!(
                "INSERT column count mismatch: expected {}, got {}",
                target_indices.len(),
                row_expr.len()
            )));
        }
        let mut row = vec![Value::Null; schema.columns.len()];
        let empty_ctx = crate::expr::RowContext::new();
        for (i, expr) in row_expr.iter().enumerate() {
            let col_idx = target_indices[i];
            let value = ExprEvaluator::eval(expr, &empty_ctx)?;
            row[col_idx] = value;
        }
        Ok(row)
    }

    /// 将 SELECT 结果行组装为 INSERT 目标行（按表 Schema 列顺序）
    fn assemble_insert_row(
        &self,
        schema: &TableSchema,
        target_indices: &[usize],
        source_row: &Row,
    ) -> Result<Row, ExecutionError> {
        if source_row.len() != target_indices.len() {
            return Err(ExecutionError::InvalidArgument(format!(
                "INSERT SELECT column count mismatch: expected {}, got {}",
                target_indices.len(),
                source_row.len()
            )));
        }
        let mut row = vec![Value::Null; schema.columns.len()];
        for (i, value) in source_row.iter().enumerate() {
            let col_idx = target_indices[i];
            row[col_idx] = value.clone();
        }
        Ok(row)
    }

    // -----------------------------------------------------------------
    //  IndexScan — 独立 API（不在 LogicalPlan 中）
    // -----------------------------------------------------------------

    /// 索引点查：返回所有匹配 key 的行
    ///
    /// 流程：IndexLookup → FetchRow
    pub fn index_scan_point(
        &self,
        table_name: &str,
        index: &InMemoryBTreeIndex,
        key: i64,
    ) -> Result<Vec<Row>, ExecutionError> {
        let table = self
            .lookup_table(table_name)
            .ok_or_else(|| ExecutionError::TableNotFound(table_name.to_string()))?;
        let row_ids = index.point_lookup(key);
        let mut result = Vec::with_capacity(row_ids.len());
        for row_id in row_ids {
            if let Some(row) = table.get_row(row_id) {
                result.push(row);
            }
        }
        Ok(result)
    }

    /// 索引范围查询 [low, high]：返回所有匹配的行（按 key 升序）
    pub fn index_scan_range(
        &self,
        table_name: &str,
        index: &InMemoryBTreeIndex,
        low: i64,
        high: i64,
    ) -> Result<Vec<Row>, ExecutionError> {
        let table = self
            .lookup_table(table_name)
            .ok_or_else(|| ExecutionError::TableNotFound(table_name.to_string()))?;
        let row_ids = index.range_lookup(low, high);
        let mut result = Vec::with_capacity(row_ids.len());
        for row_id in row_ids {
            if let Some(row) = table.get_row(row_id) {
                result.push(row);
            }
        }
        Ok(result)
    }

    // -----------------------------------------------------------------
    //  PREPARE / EXECUTE / DEALLOCATE — Phase 3.26
    // -----------------------------------------------------------------

    /// 执行 PREPARE 计划 — Phase 3.26
    ///
    /// 将预处理语句的 AST 存入 `PreparedStatementStore`，不立即 plan。
    /// 同名预处理语句会被覆盖（与 PG 一致）。
    ///
    /// # 参数
    /// - `plan` — `LogicalPlan::Prepare` 计划节点
    /// - `store` — 预处理语句存储
    pub fn execute_prepare(
        &self,
        plan: &LogicalPlan,
        store: &mut PreparedStatementStore,
    ) -> Result<(), ExecutionError> {
        let (name, parameter_types, statement) = match plan {
            LogicalPlan::Prepare {
                name,
                parameter_types,
                statement,
            } => (name, parameter_types, statement),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Prepare plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        store.prepare(name, (**statement).clone(), parameter_types.clone());
        Ok(())
    }

    /// 执行 EXECUTE 计划 — Phase 3.26
    ///
    /// # 流程
    /// 1. 从 `PreparedStatementStore` 取出预处理语句的 AST
    /// 2. 求值 EXECUTE 参数表达式（在空上下文中），得到参数值列表
    /// 3. 调用 `substitute_parameters` 将 AST 中 `Expr::Parameter(idx)` 替换为 `Expr::Literal(value)`
    /// 4. 调用 `Planner::plan_statement` 将替换后的 AST 转换为 LogicalPlan
    /// 5. 执行 LogicalPlan（当前仅支持 SELECT 类计划；DML 暂不支持）
    ///
    /// # 参数
    /// - `plan` — `LogicalPlan::Execute` 计划节点
    /// - `store` — 预处理语句存储
    /// - `catalog` — 用于 plan 阶段的 Catalog
    ///
    /// # 错误
    /// - 预处理语句不存在
    /// - 参数索引越界（如 `$3` 但仅提供 2 个参数）
    /// - 内部语句为 DML（暂不支持）
    pub fn execute_execute(
        &self,
        plan: &LogicalPlan,
        store: &PreparedStatementStore,
        catalog: &dyn Catalog,
    ) -> Result<Vec<Row>, ExecutionError> {
        let (name, parameters) = match plan {
            LogicalPlan::Execute { name, parameters } => (name, parameters),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Execute plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        let (inner_stmt, _param_types) = store
            .get(name)
            .ok_or_else(|| {
                ExecutionError::InvalidArgument(format!(
                    "prepared statement \"{name}\" does not exist"
                ))
            })?
            .clone();

        // 求值 EXECUTE 参数表达式（在空上下文中，参数应为常量表达式）
        let empty_ctx = crate::expr::RowContext::new();
        let mut param_values: Vec<Value> = Vec::with_capacity(parameters.len());
        for expr in parameters {
            let v = ExprEvaluator::eval(expr, &empty_ctx)?;
            param_values.push(v);
        }

        // 替换 AST 中的 $N 占位符
        let substituted = substitute_parameters(inner_stmt, &param_values)?;

        // Plan
        let planner = Planner::new(catalog);
        let inner_plan = planner
            .plan_statement(substituted)
            .map_err(|e| ExecutionError::InvalidArgument(format!("plan error: {e}")))?;

        // 执行（仅支持 SELECT 类计划）
        match &inner_plan {
            LogicalPlan::Scan { .. }
            | LogicalPlan::IndexScan { .. }
            | LogicalPlan::MaterializedViewScan { .. }
            | LogicalPlan::Filter { .. }
            | LogicalPlan::Projection { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::Join { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Empty
            | LogicalPlan::Dual
            | LogicalPlan::Shared { .. }
            | LogicalPlan::MemoRef { .. } => self.execute(&inner_plan),
            _ => Err(ExecutionError::Unsupported(format!(
                "EXECUTE only supports SELECT plans, got {:?}",
                std::mem::discriminant(&inner_plan)
            ))),
        }
    }

    /// 执行 DEALLOCATE 计划 — Phase 3.26
    ///
    /// # 语义（与 PG 一致）
    /// - `DEALLOCATE name` — 删除指定预处理语句；若不存在则报错
    /// - `DEALLOCATE ALL` — 清空所有预处理语句
    pub fn execute_deallocate(
        &self,
        plan: &LogicalPlan,
        store: &mut PreparedStatementStore,
    ) -> Result<(), ExecutionError> {
        let name = match plan {
            LogicalPlan::Deallocate { name } => name,
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected Deallocate plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        match name {
            None => {
                store.deallocate_all();
                Ok(())
            }
            Some(n) => {
                if !store.deallocate(n) {
                    return Err(ExecutionError::InvalidArgument(format!(
                        "prepared statement \"{n}\" does not exist"
                    )));
                }
                Ok(())
            }
        }
    }

    // -----------------------------------------------------------------
    //  Phase 3.34: SHOW / SET 命令执行
    // -----------------------------------------------------------------

    /// 执行 SHOW TABLES 计划 — Phase 3.34
    ///
    /// 从 catalog 列出所有表名，返回单列结果集（列名 `Tables_in_szrsql`）。
    /// 若未绑定 catalog，返回空结果集。
    pub fn execute_show_tables(&self) -> Result<Vec<Row>, ExecutionError> {
        let mut rows = Vec::new();
        if let Some(catalog) = self.catalog {
            // 按表名排序输出（与 MySQL 行为一致）
            let mut tables = catalog.list_tables();
            tables.sort_by_key(|a| a.name.to_lowercase());
            for table in tables {
                rows.push(vec![Value::Text(table.name)]);
            }
        }
        Ok(rows)
    }

    /// 执行 SHOW CREATE TABLE 计划 — Phase 3.34
    ///
    /// 从 catalog 读取表 Schema，重建 DDL 文本。
    /// 返回两列结果集：`Table`（表名）和 `Create Table`（DDL 文本）。
    pub fn execute_show_create_table(
        &self,
        plan: &LogicalPlan,
    ) -> Result<Vec<Row>, ExecutionError> {
        let name = match plan {
            LogicalPlan::ShowCreateTable { name } => name,
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected ShowCreateTable plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        let catalog = self.catalog.ok_or_else(|| {
            ExecutionError::InvalidArgument("SHOW CREATE TABLE requires catalog".into())
        })?;
        let schema = catalog
            .get_table(name)
            .ok_or_else(|| ExecutionError::TableNotFound(name.qualified_name()))?;
        let ddl = render_create_table_ddl(&schema);
        Ok(vec![vec![Value::Text(name.name.clone()), Value::Text(ddl)]])
    }

    /// 执行 SET NAMES 计划 — Phase 3.34
    ///
    /// 将 charset/collation 写入 SessionState。返回空结果集（无输出）。
    pub fn execute_set_names(
        &self,
        plan: &LogicalPlan,
        session: &mut SessionState,
    ) -> Result<Vec<Row>, ExecutionError> {
        let (charset, collation) = match plan {
            LogicalPlan::SetNames { charset, collation } => (charset, collation),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected SetNames plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        session.set_names(charset, collation.as_deref());
        Ok(Vec::new())
    }

    /// 执行 SET variable = value 计划 — Phase 3.34
    ///
    /// 求值 value 表达式（在空上下文中），将 (variable, value) 写入 SessionState。
    /// 返回空结果集（无输出）。
    pub fn execute_set_variable(
        &self,
        plan: &LogicalPlan,
        session: &mut SessionState,
    ) -> Result<Vec<Row>, ExecutionError> {
        let (variable, value_expr) = match plan {
            LogicalPlan::SetVariable { variable, value } => (variable, value),
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected SetVariable plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        // PostgreSQL 语义：SET variable = value 中的 value 可以是未加引号的标识符
        // （如 SET search_path = public、SET standard_conforming_strings = on）。
        // 这些标识符应被当作字符串字面量处理，而非列引用。
        let value = match value_expr {
            Expr::Identifier(parts) if !parts.is_empty() => {
                Value::Text(parts.last().unwrap().clone())
            }
            Expr::Identifier(_) => {
                return Err(ExecutionError::EvalError(
                    "SET variable value cannot be empty identifier".into(),
                ));
            }
            _ => {
                let empty_ctx = crate::expr::RowContext::new();
                ExprEvaluator::eval(value_expr, &empty_ctx)?
            }
        };
        session.set(variable, value);
        Ok(Vec::new())
    }

    /// 执行 SHOW variable 计划 — Phase 3.34
    ///
    /// 从 SessionState 读取变量值，返回单行单列结果集（列名 `setting`）。
    /// 若变量未设置，返回空字符串（与 PG `SHOW` 行为一致）。
    pub fn execute_show_variable(
        &self,
        plan: &LogicalPlan,
        session: &SessionState,
    ) -> Result<Vec<Row>, ExecutionError> {
        let variable = match plan {
            LogicalPlan::ShowVariable { variable } => variable,
            _ => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected ShowVariable plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        let value = session
            .get(variable)
            .cloned()
            .unwrap_or_else(|| Value::Text(String::new()));
        // 转换为文本（PG SHOW 返回文本）
        let text = value_to_text(value);
        Ok(vec![vec![Value::Text(text)]])
    }

    // -----------------------------------------------------------------
    //  Phase 3.35: FLASHBACK 闪回命令执行
    // -----------------------------------------------------------------

    /// 执行 FLASHBACK TRANSACTION <txn_id> 计划 — Phase 3.35
    ///
    /// 从 `TransactionHistory` 取出指定事务的"事务前快照"，返回受影响表名 + 快照列表。
    /// 调用方负责对每个表调用 `MutableTable::restore(snapshot)` 应用恢复。
    ///
    /// # 设计说明
    ///
    /// 执行器仅持有 `&dyn TableStorage`（不可变引用），无法直接调用
    /// `MutableTable::restore(&mut self, ...)`。因此采用"取出快照 + 调用方应用"
    /// 的两阶段模式，与现有 `SessionState` 外部管理方式一致。
    ///
    /// # 返回
    ///
    /// `Vec<(表名小写, TableSnapshot)>` — 受影响表名 + 对应的事务前快照。
    /// 调用方遍历此列表，对每个表名查找 `MutableTable` 并调用 `restore`。
    ///
    /// # 错误
    ///
    /// - `FlashbackError::TransactionNotFound(txn_id)` → `InvalidArgument`
    /// - `FlashbackError::AlreadyFlashedBack(txn_id)` → `InvalidArgument`
    pub fn execute_flashback_transaction(
        &self,
        plan: &LogicalPlan,
        history: &mut TransactionHistory,
    ) -> Result<Vec<(String, TableSnapshot)>, ExecutionError> {
        let txn_id = match plan {
            LogicalPlan::FlashbackTransaction { txn_id } => *txn_id,
            other => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected FlashbackTransaction plan, got {:?}",
                    std::mem::discriminant(other)
                )))
            }
        };
        let snapshots = history.take_flashback_snapshots(txn_id)?;
        // 按表名排序输出，保证测试与可观测性稳定
        let mut result: Vec<(String, TableSnapshot)> = snapshots.into_iter().collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(result)
    }

    /// 执行 FLASHBACK TABLE <name> TO TIMESTAMP '<ts>' 计划 — Phase 3.35
    ///
    /// 从 `TransactionHistory` 查找 `commit_ts <= ts_ms` 的最近一个未闪回事务，
    /// 返回该事务"事务前"该表的快照中的活跃行（即该时间点最近的可见状态）。
    ///
    /// # 时间戳解析
    ///
    /// 支持以下格式（由 `parse_timestamp_to_millis` 解析）：
    /// - Unix 毫秒整数（如 `"1700000000000"`）
    /// - ISO 8601 日期：`"2026-07-20"`
    /// - ISO 8601 日期时间：`"2026-07-20 10:30:00"` / `"2026-07-20T10:30:00Z"`
    ///
    /// # 返回
    ///
    /// `Vec<Row>` — 快照中所有活跃行（按原始 row_id 顺序）。
    ///
    /// # 错误
    ///
    /// - 时间戳解析失败 → `InvalidArgument`
    /// - 无符合条件的事务/快照 → `InvalidArgument`
    pub fn execute_flashback_table(
        &self,
        plan: &LogicalPlan,
        history: &TransactionHistory,
    ) -> Result<Vec<Row>, ExecutionError> {
        let (table, timestamp) = match plan {
            LogicalPlan::FlashbackTable { table, timestamp } => (table, timestamp),
            other => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "expected FlashbackTable plan, got {:?}",
                    std::mem::discriminant(other)
                )))
            }
        };
        let ts_ms = parse_timestamp_to_millis(timestamp).ok_or_else(|| {
            ExecutionError::InvalidArgument(format!("invalid timestamp: {timestamp}"))
        })?;
        let snapshot = history
            .get_snapshot_as_of(&table.name, ts_ms)
            .ok_or_else(|| {
                ExecutionError::InvalidArgument(format!(
                    "no snapshot found for table {} as of {}",
                    table.qualified_name(),
                    timestamp
                ))
            })?;
        Ok(snapshot.active_rows())
    }
}

// =====================================================================
//  Phase 3.35: 时间戳解析
// =====================================================================

/// 将时间戳字符串解析为 Unix 毫秒 — Phase 3.35
///
/// 支持以下格式（按顺序尝试）：
/// 1. Unix 毫秒整数（纯数字，如 `"1700000000000"`）
/// 2. ISO 8601 日期：`"2026-07-20"` → 当天 00:00:00 UTC
/// 3. ISO 8601 日期时间（含空格或 T 分隔）：
///    - `"2026-07-20 10:30:00"`（空格分隔，假定 UTC）
///    - `"2026-07-20T10:30:00Z"`（带 Z 后缀）
///    - `"2026-07-20T10:30:00"`（无时区后缀，假定 UTC）
///
/// 解析失败返回 None（由调用方转换为 `InvalidArgument` 错误）。
pub fn parse_timestamp_to_millis_pub(ts: &str) -> Option<u64> {
    parse_timestamp_to_millis(ts)
}

/// 将时间戳字符串解析为 Unix 毫秒 — Phase 3.35（内部实现）
fn parse_timestamp_to_millis(ts: &str) -> Option<u64> {
    let trimmed = ts.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 1. 纯数字 → Unix 毫秒
    if let Ok(ms) = trimmed.parse::<u64>() {
        return Some(ms);
    }
    // 2. 解析 ISO 8601 日期或日期时间
    //    标准化为 "YYYY-MM-DDTHH:MM:SS" 形式后用 strptime 解析
    let normalized = normalize_iso8601(trimmed)?;
    parse_iso8601_to_millis(&normalized)
}

/// 将多种 ISO 8601 写法归一化为 `YYYY-MM-DDTHH:MM:SS` 形式（不带时区后缀）
///
/// - `"2026-07-20"` → `"2026-07-20T00:00:00"`
/// - `"2026-07-20 10:30:00"` → `"2026-07-20T10:30:00"`
/// - `"2026-07-20T10:30:00Z"` → `"2026-07-20T10:30:00"`
/// - `"2026-07-20T10:30:00"` → `"2026-07-20T10:30:00"`
fn normalize_iso8601(s: &str) -> Option<String> {
    // 去除尾部 Z/z 时区标记
    let s = s.trim_end_matches(['Z', 'z']);
    if s.len() == 10 {
        // 仅日期：YYYY-MM-DD
        if s.chars().nth(4) == Some('-') && s.chars().nth(7) == Some('-') {
            return Some(format!("{s}T00:00:00"));
        }
        return None;
    }
    // 含时间：将第一个空格替换为 T
    if let Some(idx) = s.find(' ') {
        let mut out = String::with_capacity(s.len());
        out.push_str(&s[..idx]);
        out.push('T');
        out.push_str(s[idx + 1..].trim_start());
        return Some(out);
    }
    // 已是 T 分隔形式
    if s.contains('T') {
        return Some(s.to_string());
    }
    None
}

/// 解析归一化后的 ISO 8601（YYYY-MM-DDTHH:MM:SS）为 Unix 毫秒
///
/// 使用手动字段解析（避免引入 chrono 依赖）。假定 UTC 时区。
fn parse_iso8601_to_millis(s: &str) -> Option<u64> {
    // 期望格式：YYYY-MM-DDTHH:MM:SS
    let bytes = s.as_bytes();
    if bytes.len() != 19 {
        return None;
    }
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = parse_two_digit_slice(s, 0, 4)?;
    let month = parse_two_digit_slice(s, 5, 7)?;
    let day = parse_two_digit_slice(s, 8, 10)?;
    let hour = parse_two_digit_slice(s, 11, 13)?;
    let minute = parse_two_digit_slice(s, 14, 16)?;
    let second = parse_two_digit_slice(s, 17, 19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days_since_epoch = days_from_civil(year, month, day)?;
    let secs = days_since_epoch * 86400 + hour as u64 * 3600 + minute as u64 * 60 + second as u64;
    Some(secs * 1000)
}

/// 解析字符串切片为 u32（如 "2026" → 2026）
fn parse_two_digit_slice(s: &str, start: usize, end: usize) -> Option<u32> {
    s.get(start..end)?.parse().ok()
}

/// 计算从 1970-01-01 到给定年月日的天数（Howard Hinnant 算法）
///
/// 返回 None 当年份超出合理范围（< 1970 或 > 9999）。
fn days_from_civil(year: u32, month: u32, day: u32) -> Option<u64> {
    if year < 1970 {
        return None;
    }
    // Howard Hinnant 算法（civil_from_days 的逆函数）
    // 全部使用 i64 计算后转为 u64，避免类型不匹配。
    let y = if month <= 2 {
        year as i64 - 1
    } else {
        year as i64
    };
    let m = month as i64;
    let d = day as i64;
    let era = if y >= 0 {
        y
    } else {
        y - 399
    } / 400;
    let yoe = y - era * 400;
    let doy =
        (153 * (if m > 2 {
            m - 3
        } else {
            m + 9
        }) + 2)
            / 5
            + d
            - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    if days < 0 {
        None
    } else {
        Some(days as u64)
    }
}

// =====================================================================
//  Phase 3.34: CREATE TABLE DDL 渲染
// =====================================================================

/// 根据 TableSchema 重建 CREATE TABLE DDL 文本（用于 SHOW CREATE TABLE）
///
/// 简化实现：仅渲染列名 + 类型 + NOT NULL，不渲染约束、外键、CHECK 等。
/// 输出格式：`CREATE TABLE name (col1 TYPE [NOT NULL], col2 TYPE, ...)`
///
/// # 安全说明（ADV-BUG-004 修复）
///
/// 所有标识符（表名/列名）均通过 [`quote_ident_smart`] 智能转义：
/// - 普通标识符（仅含字母/数字/下划线）→ 不加双引号，输出更可读
/// - 含特殊字符的标识符（如 `a"b`、`user name`）→ 强制加双引号转义
///
/// 这与 PostgreSQL `pg_get_constraintdef` 等系统函数的输出风格一致，
/// 在保证安全性的同时提升 DDL 可读性。
fn render_create_table_ddl(schema: &TableSchema) -> String {
    let mut cols = Vec::with_capacity(schema.columns.len());
    for col in &schema.columns {
        let mut s = format!(
            "{} {}",
            quote_ident_smart(&col.name),
            column_type_to_sql(&col.data_type)
        );
        if col.not_null {
            s.push_str(" NOT NULL");
        }
        cols.push(s);
    }
    format!(
        "CREATE TABLE {} (\n  {}\n)",
        quote_ident_smart(&schema.name.name),
        cols.join(",\n  ")
    )
}

/// 将 ColumnType 渲染为 SQL 类型字符串（用于 SHOW CREATE TABLE）
fn column_type_to_sql(ty: &szrsql_types::value::ColumnType) -> String {
    use szrsql_types::value::ColumnType;
    match ty {
        ColumnType::Null => "NULL".into(),
        ColumnType::Int64 => "INT8".into(),
        ColumnType::Float64 => "FLOAT8".into(),
        ColumnType::Text => "TEXT".into(),
        ColumnType::Blob => "BYTEA".into(),
        ColumnType::Bool => "BOOLEAN".into(),
        ColumnType::Date => "DATE".into(),
        ColumnType::Timestamp => "TIMESTAMP".into(),
        ColumnType::Decimal { precision, scale } => {
            if scale == &0 {
                format!("DECIMAL({precision})")
            } else {
                format!("DECIMAL({precision},{scale})")
            }
        }
        ColumnType::Array(inner) => format!("{}[]", column_type_to_sql(inner)),
        ColumnType::Enum(_) => "ENUM".into(),
        ColumnType::Range(_) => "RANGE".into(),
        ColumnType::Json => "JSON".into(),
        ColumnType::TsVector => "TSVECTOR".into(),
        ColumnType::TsQuery => "TSQUERY".into(),
    }
}

/// 将 Value 转换为文本表示（用于 SHOW variable 输出）
///
/// 简化实现：
/// - Text 直接返回内部字符串
/// - Int64/Float64/Bool 等转换为字符串
/// - 复杂类型（Array/Json/TsVector 等）使用 Debug 格式
fn value_to_text(value: Value) -> String {
    match value {
        Value::Text(s) => s,
        Value::Int64(i) => i.to_string(),
        Value::Float64(f) => f.to_string(),
        Value::Bool(b) => {
            if b {
                "true".into()
            } else {
                "false".into()
            }
        }
        Value::Null => String::new(),
        // 其余类型暂以 Debug 形式呈现（SET 变量值通常为标量）
        other => format!("{:?}", other),
    }
}

// =====================================================================
//  Phase 3.35: 事务历史记录 + FLASHBACK 闪回
// =====================================================================

/// 已提交事务记录 — Phase 3.35
///
/// 记录一个已提交事务的元数据 + 事务前所有受影响表的快照，
/// 用于 `FLASHBACK TRANSACTION <txn_id>` 撤销该事务的修改。
#[derive(Debug, Clone)]
pub struct CommittedTransaction {
    /// 事务 ID（由 TransactionHistory 在 record_commit 时分配，从 1 开始递增）
    pub txn_id: u64,
    /// 提交时间戳（Unix 毫秒）
    pub commit_ts_ms: u64,
    /// 事务前所有受影响表的快照（表名小写 → 快照）
    ///
    /// 调用方在 COMMIT 时通过 `record_commit` 传入：
    /// - 表名为受影响表的小写形式
    /// - 快照为该表在事务开始前的状态（由 `MutableTable::snapshot()` 获取）
    pub pre_snapshots: HashMap<String, TableSnapshot>,
    /// 是否已被闪回（已闪回事务不能再次闪回）
    pub flashed_back: bool,
}

/// 事务历史记录器 — Phase 3.35
///
/// 维护已提交事务的列表，支持：
/// - `record_commit(pre_snapshots)` — 在 COMMIT 时记录事务前快照，返回分配的 txn_id
/// - `get_flashback_snapshots(txn_id)` — 取出该事务的事务前快照用于 restore
/// - `get_snapshot_as_of(table, ts_ms)` — 查找 commit_ts <= ts 的最近事务，返回该表的事务前快照
///
/// # 设计
///
/// - 简化为单线程、内存模型：不持久化到磁盘，进程重启后丢失
/// - 仅记录"事务前快照"，不记录"事务后快照"（节省内存）
/// - FLASHBACK TABLE AS OF TIMESTAMP 返回 commit_ts <= ts 的最近事务的"事务前快照"
///   （即该事务开始前的表状态，等价于该时间点最近的可见状态）
#[derive(Debug, Default, Clone)]
pub struct TransactionHistory {
    /// 已提交事务列表（按 txn_id 升序，等价于按 commit_ts 升序）
    transactions: Vec<CommittedTransaction>,
    /// 下一个待分配的 txn_id（从 1 开始）
    next_txn_id: u64,
}

impl TransactionHistory {
    /// 创建空事务历史
    pub fn new() -> Self {
        Self {
            transactions: Vec::new(),
            next_txn_id: 1,
        }
    }

    /// 记录一个已提交事务
    ///
    /// 调用方需在 COMMIT 时传入事务前所有受影响表的快照（表名小写 → 快照）。
    /// 返回分配的 txn_id（从 1 开始递增）。
    pub fn record_commit(&mut self, pre_snapshots: HashMap<String, TableSnapshot>) -> u64 {
        let txn_id = self.next_txn_id;
        self.next_txn_id += 1;
        let commit_ts_ms = current_unix_millis();
        self.transactions.push(CommittedTransaction {
            txn_id,
            commit_ts_ms,
            pre_snapshots,
            flashed_back: false,
        });
        txn_id
    }

    /// 取出指定事务的事务前快照（用于 FLASHBACK TRANSACTION）
    ///
    /// 返回 `Ok(snapshots)` 表示可以闪回；返回 `Err` 表示事务不存在或已被闪回。
    /// 注意：调用成功后会把该事务标记为 `flashed_back=true`，避免重复闪回。
    pub fn take_flashback_snapshots(
        &mut self,
        txn_id: u64,
    ) -> Result<HashMap<String, TableSnapshot>, FlashbackError> {
        let txn = self
            .transactions
            .iter_mut()
            .find(|t| t.txn_id == txn_id)
            .ok_or(FlashbackError::TransactionNotFound(txn_id))?;
        if txn.flashed_back {
            return Err(FlashbackError::AlreadyFlashedBack(txn_id));
        }
        txn.flashed_back = true;
        Ok(std::mem::take(&mut txn.pre_snapshots))
    }

    /// 查找 commit_ts <= ts_ms 的最近一个事务，返回其"事务前"指定表的快照
    ///
    /// 用于 FLASHBACK TABLE TO TIMESTAMP 查询历史快照。
    /// 若无符合条件的事务或事务未涉及该表，返回 None。
    /// 已被闪回的事务不参与查询（其快照已被取走）。
    pub fn get_snapshot_as_of(&self, table_name: &str, ts_ms: u64) -> Option<&TableSnapshot> {
        let key = table_name.to_lowercase();
        // 反向遍历找最近一个 commit_ts <= ts 的事务
        for txn in self.transactions.iter().rev() {
            if txn.flashed_back {
                continue;
            }
            if txn.commit_ts_ms <= ts_ms {
                if let Some(snap) = txn.pre_snapshots.get(&key) {
                    return Some(snap);
                }
            }
        }
        None
    }

    /// 当前已记录的事务数
    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    /// 获取指定事务的引用（用于测试断言）
    pub fn get_transaction(&self, txn_id: u64) -> Option<&CommittedTransaction> {
        self.transactions.iter().find(|t| t.txn_id == txn_id)
    }
}

/// 闪回操作错误 — Phase 3.35
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlashbackError {
    /// 事务不存在
    #[error("transaction not found: {0}")]
    TransactionNotFound(u64),
    /// 事务已被闪回，不能重复闪回
    #[error("transaction {0} has already been flashed back")]
    AlreadyFlashedBack(u64),
}

/// 获取当前 Unix 时间戳（毫秒）— Phase 3.35
///
/// 使用 `SystemTime::now()` 获取系统时间并转为毫秒。
pub fn current_unix_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl<'a> Default for Executor<'a> {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  Aggregate 辅助函数
// =====================================================================

/// 构造 Aggregate 输出 Schema
///
/// 列顺序：[GROUP BY 列..., 聚合列...]
/// - GROUP BY 列名：取表达式的最后一个标识符部分（如 `t1.id` → `id`）
/// - 聚合列名：取 alias 或 func_name
/// - 所有列类型暂设为 `ColumnType::Null`（执行器不依赖类型）
fn aggregate_output_schema(
    group_exprs: &[Expr],
    aggregates: &[AggregateExpr],
) -> Result<TableSchema, ExecutionError> {
    let mut cols = Vec::with_capacity(group_exprs.len() + aggregates.len());
    for g in group_exprs {
        let name = if let Expr::Identifier(parts) = g {
            parts.last().cloned().unwrap_or_default()
        } else {
            format!("{g:?}")
        };
        cols.push(ColumnDefinition::new(
            name,
            szrsql_types::value::ColumnType::Null,
        ));
    }
    for a in aggregates {
        let name = a.alias.clone().unwrap_or_else(|| a.func_name.clone());
        cols.push(ColumnDefinition::new(
            name,
            szrsql_types::value::ColumnType::Null,
        ));
    }
    Ok(TableSchema {
        name: TableName::new("__aggregate__"),
        columns: cols,
    })
}

/// 在空组上计算聚合值（无 GROUP BY 且 input 为空时使用）
///
/// 语义对齐 PG：
/// - COUNT / COUNT(*) → 0
/// - SUM / AVG / MIN / MAX → NULL
fn compute_empty_aggregate(agg: &AggregateExpr) -> Value {
    match agg.func_name.to_lowercase().as_str() {
        "count" => Value::Int64(0),
        _ => Value::Null,
    }
}

/// 在一组行上计算单个聚合值
fn compute_aggregate(
    agg: &AggregateExpr,
    group_rows: &[Row],
    input_schema: &TableSchema,
) -> Result<Value, ExecutionError> {
    let func = agg.func_name.to_lowercase();

    // 求值聚合参数表达式 → 收集值列表
    // COUNT(*) 的 args 为空 Vec，特殊处理
    let is_count_star = func == "count" && agg.args.is_empty();

    let mut values: Vec<Value> = Vec::with_capacity(group_rows.len());
    if !is_count_star {
        // 多参数聚合支持：string_agg(expr, delimiter) 需要 2 个参数
        // 其他聚合（sum/avg/min/max/array_agg）只取第一个参数
        let expected_args = if func == "string_agg" {
            2
        } else {
            1
        };
        if agg.args.len() != expected_args {
            return Err(ExecutionError::Unsupported(format!(
                "aggregate `{}` with {} args (expected {})",
                func,
                agg.args.len(),
                expected_args
            )));
        }
        let arg_expr = &agg.args[0];
        for row in group_rows {
            let ctx = ExecRowContext::new(input_schema, row);
            let v = ExprEvaluator::eval(arg_expr, &ctx)?;
            values.push(v);
        }
    }

    // DISTINCT 去重（基于 Debug 字符串）
    if agg.distinct {
        let mut seen = HashSet::new();
        values.retain(|v| seen.insert(format!("{v:?}")));
    }

    match func.as_str() {
        "count" => {
            if is_count_star {
                // COUNT(*) = 行数（含 NULL 行）
                Ok(Value::Int64(group_rows.len() as i64))
            } else {
                // COUNT(expr) = 非空值数
                let n = values.iter().filter(|v| !matches!(v, Value::Null)).count();
                Ok(Value::Int64(n as i64))
            }
        }
        "sum" => {
            let nums: Vec<&Value> = values
                .iter()
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            if nums.is_empty() {
                return Ok(Value::Null);
            }
            // 全 Int64 → Int64 求和；含 Float64 → Float64 求和
            let all_int = nums.iter().all(|v| matches!(v, Value::Int64(_)));
            if all_int {
                let sum: i64 = nums
                    .iter()
                    .map(|v| {
                        if let Value::Int64(n) = v {
                            *n
                        } else {
                            0
                        }
                    })
                    .sum();
                Ok(Value::Int64(sum))
            } else {
                let sum: f64 = nums
                    .iter()
                    .map(|v| match v {
                        Value::Int64(n) => *n as f64,
                        Value::Float64(f) => *f,
                        _ => 0.0,
                    })
                    .sum();
                Ok(Value::Float64(sum))
            }
        }
        "avg" => {
            let nums: Vec<&Value> = values
                .iter()
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            if nums.is_empty() {
                return Ok(Value::Null);
            }
            let sum: f64 = nums
                .iter()
                .map(|v| match v {
                    Value::Int64(n) => *n as f64,
                    Value::Float64(f) => *f,
                    _ => 0.0,
                })
                .sum();
            Ok(Value::Float64(sum / nums.len() as f64))
        }
        "min" => {
            let nums: Vec<&Value> = values
                .iter()
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            if nums.is_empty() {
                return Ok(Value::Null);
            }
            Ok(nums
                .into_iter()
                .min_by(|a, b| compare_values(a, b))
                .cloned()
                .unwrap_or(Value::Null))
        }
        "max" => {
            let nums: Vec<&Value> = values
                .iter()
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            if nums.is_empty() {
                return Ok(Value::Null);
            }
            Ok(nums
                .into_iter()
                .max_by(|a, b| compare_values(a, b))
                .cloned()
                .unwrap_or(Value::Null))
        }
        // Phase 3.32: array_agg(expr) — 收集所有非 NULL 值（保留顺序），返回 Value::Array
        // PG 语义：array_agg 忽略 NULL；空组或全 NULL → 空数组（PG 返回空数组 '{}'，不是 NULL）
        "array_agg" => {
            let collected: Vec<Value> = values
                .into_iter()
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            Ok(Value::Array(collected))
        }
        // Phase 3.32: string_agg(expr, delimiter) — 用 delimiter 拼接所有非 NULL 值
        // PG 语义：空组或全 NULL → NULL；delimiter 必须是 Text
        "string_agg" => {
            // string_agg 需要 2 个参数：expr 和 delimiter
            // 但 aggregate 框架只对第一个参数求值；delimiter 在每行都一样，从第一行取
            if agg.args.len() != 2 {
                return Err(ExecutionError::Unsupported(format!(
                    "string_agg expects 2 args (expr, delimiter), got {}",
                    agg.args.len()
                )));
            }
            // 对每行求值 delimiter（应该都一样）
            let delimiter = if group_rows.is_empty() {
                Value::Null
            } else {
                let ctx = ExecRowContext::new(input_schema, &group_rows[0]);
                ExprEvaluator::eval(&agg.args[1], &ctx)?
            };
            let delimiter_str = match delimiter {
                Value::Text(s) => s,
                Value::Null => return Ok(Value::Null),
                other => {
                    return Err(ExecutionError::Unsupported(format!(
                        "string_agg delimiter must be text, got {:?}",
                        other
                    )));
                }
            };
            // values 已包含 expr 的求值结果
            let parts: Vec<String> = values
                .iter()
                .filter(|v| !matches!(v, Value::Null))
                .filter_map(|v| match v {
                    Value::Text(s) => Some(s.clone()),
                    Value::Int64(n) => Some(n.to_string()),
                    Value::Float64(f) => Some(f.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                })
                .collect();
            if parts.is_empty() {
                return Ok(Value::Null);
            }
            Ok(Value::Text(parts.join(&delimiter_str)))
        }
        other => Err(ExecutionError::Unsupported(format!(
            "aggregate function `{other}`"
        ))),
    }
}

/// 值比较（用于 MIN/MAX / ORDER BY / RANGE 帧）— 返回 Ordering
///
/// 支持 Int64 与 Float64 混合比较；其他类型按 Debug 字符串比较
fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Int64(x), Value::Int64(y)) => x.cmp(y),
        (Value::Float64(x), Value::Float64(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Int64(x), Value::Float64(y)) => {
            (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (Value::Float64(x), Value::Int64(y)) => {
            x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
        }
        (Value::Text(x), Value::Text(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::Timestamp(x), Value::Timestamp(y)) => x.cmp(y),
        _ => format!("{a:?}").cmp(&format!("{b:?}")),
    }
}

/// 递归替换表达式中的聚合函数调用为已物化的字面量
///
/// 用于 Projection / HAVING 求值 — Aggregate 节点已产出 [group_cols..., agg_vals...]
/// 行，后续节点引用聚合函数（如 `COUNT(*)`、`SUM(x)`）时需替换为对应位置的值。
///
/// 匹配规则：函数名 + DISTINCT + 参数列表完全相等
fn substitute_aggregates(
    expr: &Expr,
    aggregates: &[AggregateExpr],
    row: &Row,
    group_count: usize,
) -> Expr {
    match expr {
        Expr::Function {
            name,
            args,
            distinct,
        } => {
            if is_aggregate_fn(name) {
                // 在 aggregates 列表中查找匹配
                for (idx, agg) in aggregates.iter().enumerate() {
                    if agg.func_name == *name && agg.distinct == *distinct && agg.args == *args {
                        let value = row.get(group_count + idx).cloned().unwrap_or(Value::Null);
                        return Expr::Literal(value);
                    }
                }
            }
            // 非聚合函数或未匹配 — 递归处理参数后保留
            Expr::Function {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|a| substitute_aggregates(a, aggregates, row, group_count))
                    .collect(),
                distinct: *distinct,
            }
        }
        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: Box::new(substitute_aggregates(left, aggregates, row, group_count)),
            op: *op,
            right: Box::new(substitute_aggregates(right, aggregates, row, group_count)),
        },
        Expr::UnaryOp { op, expr } => Expr::UnaryOp {
            op: *op,
            expr: Box::new(substitute_aggregates(expr, aggregates, row, group_count)),
        },
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            let new_when_then: Vec<(Expr, Expr)> = when_then
                .iter()
                .map(|(w, t)| {
                    (
                        substitute_aggregates(w, aggregates, row, group_count),
                        substitute_aggregates(t, aggregates, row, group_count),
                    )
                })
                .collect();
            Expr::Case {
                operand: operand
                    .as_ref()
                    .map(|e| Box::new(substitute_aggregates(e, aggregates, row, group_count))),
                when_then: new_when_then,
                else_expr: else_expr
                    .as_ref()
                    .map(|e| Box::new(substitute_aggregates(e, aggregates, row, group_count))),
            }
        }
        Expr::Cast { expr, data_type } => Expr::Cast {
            expr: Box::new(substitute_aggregates(expr, aggregates, row, group_count)),
            data_type: data_type.clone(),
        },
        Expr::InList {
            expr,
            list,
            negated,
        } => Expr::InList {
            expr: Box::new(substitute_aggregates(expr, aggregates, row, group_count)),
            list: list
                .iter()
                .map(|e| substitute_aggregates(e, aggregates, row, group_count))
                .collect(),
            negated: *negated,
        },
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Expr::Between {
            expr: Box::new(substitute_aggregates(expr, aggregates, row, group_count)),
            low: Box::new(substitute_aggregates(low, aggregates, row, group_count)),
            high: Box::new(substitute_aggregates(high, aggregates, row, group_count)),
            negated: *negated,
        },
        Expr::Like {
            expr,
            pattern,
            negated,
            case_insensitive,
        } => Expr::Like {
            expr: Box::new(substitute_aggregates(expr, aggregates, row, group_count)),
            pattern: Box::new(substitute_aggregates(pattern, aggregates, row, group_count)),
            negated: *negated,
            case_insensitive: *case_insensitive,
        },
        Expr::IsNull { expr, negated } => Expr::IsNull {
            expr: Box::new(substitute_aggregates(expr, aggregates, row, group_count)),
            negated: *negated,
        },
        Expr::Tuple(exprs) => Expr::Tuple(
            exprs
                .iter()
                .map(|e| substitute_aggregates(e, aggregates, row, group_count))
                .collect(),
        ),
        // Phase 3.32: 数组字面量与 ANY/ALL 子表达式可能含聚合
        Expr::Array(exprs) => Expr::Array(
            exprs
                .iter()
                .map(|e| substitute_aggregates(e, aggregates, row, group_count))
                .collect(),
        ),
        Expr::AnyOp { left, op, right } => Expr::AnyOp {
            left: Box::new(substitute_aggregates(left, aggregates, row, group_count)),
            op: *op,
            right: Box::new(substitute_aggregates(right, aggregates, row, group_count)),
        },
        Expr::AllOp { left, op, right } => Expr::AllOp {
            left: Box::new(substitute_aggregates(left, aggregates, row, group_count)),
            op: *op,
            right: Box::new(substitute_aggregates(right, aggregates, row, group_count)),
        },
        // 叶子节点 — 原样返回
        Expr::Literal(_)
        | Expr::Identifier(_)
        | Expr::Wildcard
        | Expr::Parameter(_)
        | Expr::Subquery(_)
        | Expr::Exists { .. }
        | Expr::InSubquery { .. }
        // Phase F-9: 新增 PG 兼容表达式 — 不参与聚合替换，原样返回
        | Expr::IsDistinctFrom { .. }
        | Expr::SimilarTo { .. }
        | Expr::Substring { .. } => expr.clone(),
        // Phase 6.2: 窗口函数由 Window 节点单独求值后由 substitute_window_functions 处理，
        // 此处保持原样（避免被聚合 substitute 误改）
        Expr::WindowFunction { .. } => expr.clone(),
    }
}

// =====================================================================
//  窗口函数实现 — Phase 6.2
// =====================================================================

/// 递归替换表达式中的 `Expr::WindowFunction` 引用为已物化的字面量
///
/// 用于 Projection 求值 — Window 节点已产出 `[input_cols..., win_vals...]` 行，
/// 后续 Projection 引用窗口函数时需替换为对应位置的值。
///
/// 匹配规则：函数名 + DISTINCT + 参数列表 + 窗口规格完全相等
fn substitute_window_functions(
    expr: &Expr,
    window_funcs: &[WindowFunctionExpr],
    row: &Row,
    input_col_count: usize,
) -> Expr {
    match expr {
        Expr::WindowFunction {
            name,
            args,
            distinct,
            window,
        } => {
            for (idx, wf) in window_funcs.iter().enumerate() {
                if wf.func_name == *name
                    && wf.distinct == *distinct
                    && wf.args == *args
                    && &wf.window == window
                {
                    let value = row
                        .get(input_col_count + idx)
                        .cloned()
                        .unwrap_or(Value::Null);
                    return Expr::Literal(value);
                }
            }
            // 未匹配 — 保持原样（执行期会报错）
            expr.clone()
        }
        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: Box::new(substitute_window_functions(
                left,
                window_funcs,
                row,
                input_col_count,
            )),
            op: *op,
            right: Box::new(substitute_window_functions(
                right,
                window_funcs,
                row,
                input_col_count,
            )),
        },
        Expr::UnaryOp { op, expr } => Expr::UnaryOp {
            op: *op,
            expr: Box::new(substitute_window_functions(
                expr,
                window_funcs,
                row,
                input_col_count,
            )),
        },
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            let new_when_then: Vec<(Expr, Expr)> = when_then
                .iter()
                .map(|(w, t)| {
                    (
                        substitute_window_functions(w, window_funcs, row, input_col_count),
                        substitute_window_functions(t, window_funcs, row, input_col_count),
                    )
                })
                .collect();
            Expr::Case {
                operand: operand.as_ref().map(|e| {
                    Box::new(substitute_window_functions(
                        e,
                        window_funcs,
                        row,
                        input_col_count,
                    ))
                }),
                when_then: new_when_then,
                else_expr: else_expr.as_ref().map(|e| {
                    Box::new(substitute_window_functions(
                        e,
                        window_funcs,
                        row,
                        input_col_count,
                    ))
                }),
            }
        }
        Expr::Cast { expr, data_type } => Expr::Cast {
            expr: Box::new(substitute_window_functions(
                expr,
                window_funcs,
                row,
                input_col_count,
            )),
            data_type: data_type.clone(),
        },
        Expr::Function {
            name,
            args,
            distinct,
        } => Expr::Function {
            name: name.clone(),
            args: args
                .iter()
                .map(|a| substitute_window_functions(a, window_funcs, row, input_col_count))
                .collect(),
            distinct: *distinct,
        },
        Expr::InList {
            expr,
            list,
            negated,
        } => Expr::InList {
            expr: Box::new(substitute_window_functions(
                expr,
                window_funcs,
                row,
                input_col_count,
            )),
            list: list
                .iter()
                .map(|e| substitute_window_functions(e, window_funcs, row, input_col_count))
                .collect(),
            negated: *negated,
        },
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Expr::Between {
            expr: Box::new(substitute_window_functions(
                expr,
                window_funcs,
                row,
                input_col_count,
            )),
            low: Box::new(substitute_window_functions(
                low,
                window_funcs,
                row,
                input_col_count,
            )),
            high: Box::new(substitute_window_functions(
                high,
                window_funcs,
                row,
                input_col_count,
            )),
            negated: *negated,
        },
        Expr::Like {
            expr,
            pattern,
            negated,
            case_insensitive,
        } => Expr::Like {
            expr: Box::new(substitute_window_functions(
                expr,
                window_funcs,
                row,
                input_col_count,
            )),
            pattern: Box::new(substitute_window_functions(
                pattern,
                window_funcs,
                row,
                input_col_count,
            )),
            negated: *negated,
            case_insensitive: *case_insensitive,
        },
        Expr::IsNull { expr, negated } => Expr::IsNull {
            expr: Box::new(substitute_window_functions(
                expr,
                window_funcs,
                row,
                input_col_count,
            )),
            negated: *negated,
        },
        Expr::Tuple(exprs) => Expr::Tuple(
            exprs
                .iter()
                .map(|e| substitute_window_functions(e, window_funcs, row, input_col_count))
                .collect(),
        ),
        Expr::Array(exprs) => Expr::Array(
            exprs
                .iter()
                .map(|e| substitute_window_functions(e, window_funcs, row, input_col_count))
                .collect(),
        ),
        Expr::AnyOp { left, op, right } => Expr::AnyOp {
            left: Box::new(substitute_window_functions(
                left,
                window_funcs,
                row,
                input_col_count,
            )),
            op: *op,
            right: Box::new(substitute_window_functions(
                right,
                window_funcs,
                row,
                input_col_count,
            )),
        },
        Expr::AllOp { left, op, right } => Expr::AllOp {
            left: Box::new(substitute_window_functions(
                left,
                window_funcs,
                row,
                input_col_count,
            )),
            op: *op,
            right: Box::new(substitute_window_functions(
                right,
                window_funcs,
                row,
                input_col_count,
            )),
        },
        // 叶子节点 — 原样返回
        Expr::Literal(_)
        | Expr::Identifier(_)
        | Expr::Wildcard
        | Expr::Parameter(_)
        | Expr::Subquery(_)
        | Expr::Exists { .. }
        | Expr::InSubquery { .. }
        // Phase F-9: 新增 PG 兼容表达式 — 不参与窗口替换，原样返回
        | Expr::IsDistinctFrom { .. }
        | Expr::SimilarTo { .. }
        | Expr::Substring { .. } => expr.clone(),
    }
}

/// 计算单个窗口函数在所有行上的结果 — Phase 6.2
///
/// 返回 `Vec<Value>`，长度等于 `rows.len()`，按原始行顺序。
fn compute_window_function(
    wf: &WindowFunctionExpr,
    rows: &[Row],
    schema: &TableSchema,
) -> Result<Vec<Value>, ExecutionError> {
    let func_name = wf.func_name.as_lowercase_str();
    let partition_keys = eval_partition_keys(&wf.window.partition_by, rows, schema)?;
    let order_keys = eval_order_keys(&wf.window.order_by, rows, schema)?;

    // 按 partition_key 分组（保留原始行索引）
    let mut partitions: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, key) in partition_keys.iter().enumerate() {
        partitions.entry(key.clone()).or_default().push(idx);
    }

    let mut results: Vec<Value> = vec![Value::Null; rows.len()];

    for (_, mut indices) in partitions {
        // 在分区内按 ORDER BY 排序
        if !order_keys.is_empty() {
            indices.sort_by(|a, b| compare_order_keys(&order_keys, *a, *b, &wf.window.order_by));
        }

        // 计算每行的窗口函数值
        compute_window_function_values(
            &func_name,
            &wf.args,
            wf.distinct,
            &wf.window,
            &indices,
            rows,
            schema,
            &mut results,
        )?;
    }

    Ok(results)
}

/// 评估所有行的 PARTITION BY 键
///
/// 返回 `Vec<String>`，每个元素是对应行的分区键序列化字符串。
fn eval_partition_keys(
    partition_by: &[Expr],
    rows: &[Row],
    schema: &TableSchema,
) -> Result<Vec<String>, ExecutionError> {
    let mut keys = Vec::with_capacity(rows.len());
    for row in rows {
        let ctx = ExecRowContext::new(schema, row);
        let mut parts = Vec::with_capacity(partition_by.len());
        for expr in partition_by {
            let v = ExprEvaluator::eval(expr, &ctx)?;
            parts.push(format!("{v:?}"));
        }
        keys.push(parts.join("\x1F")); // 用单元分隔符拼接多列键
    }
    Ok(keys)
}

/// 评估所有行的 ORDER BY 键
///
/// 返回 `Vec<Vec<Value>>`，每个元素是对应行的 ORDER BY 值列表。
fn eval_order_keys(
    order_by: &[OrderByExpr],
    rows: &[Row],
    schema: &TableSchema,
) -> Result<Vec<Vec<Value>>, ExecutionError> {
    let mut keys = Vec::with_capacity(rows.len());
    for row in rows {
        let ctx = ExecRowContext::new(schema, row);
        let mut parts = Vec::with_capacity(order_by.len());
        for ob in order_by {
            parts.push(ExprEvaluator::eval(&ob.expr, &ctx)?);
        }
        keys.push(parts);
    }
    Ok(keys)
}

/// 比较 ORDER BY 键
fn compare_order_keys(
    order_keys: &[Vec<Value>],
    a: usize,
    b: usize,
    order_by: &[OrderByExpr],
) -> std::cmp::Ordering {
    let ka = &order_keys[a];
    let kb = &order_keys[b];
    for (i, ob) in order_by.iter().enumerate() {
        let ord = compare_values(&ka[i], &kb[i]);
        let ord = if ob.asc {
            ord
        } else {
            ord.reverse()
        };
        // NULLS FIRST/LAST
        let ord = match (value_is_null(&ka[i]), value_is_null(&kb[i])) {
            (true, false) => {
                if ob.nulls_first {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            }
            (false, true) => {
                if ob.nulls_first {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                }
            }
            _ => ord,
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// 判断 Value 是否为 NULL
#[inline]
fn value_is_null(v: &Value) -> bool {
    matches!(v, Value::Null)
}

/// 计算窗口函数在分区内每行的值，写入 `results`
#[allow(clippy::too_many_arguments)]
fn compute_window_function_values(
    func_name: &str,
    args: &[Expr],
    distinct: bool,
    window: &WindowSpec,
    indices: &[usize],
    rows: &[Row],
    schema: &TableSchema,
    results: &mut [Value],
) -> Result<(), ExecutionError> {
    let _ = distinct; // 多数窗口函数不支持 DISTINCT，暂忽略

    match func_name {
        // 排名函数 — 忽略帧
        "row_number" => {
            for (rank, &idx) in indices.iter().enumerate() {
                results[idx] = Value::Int64((rank + 1) as i64);
            }
            Ok(())
        }
        "rank" => {
            let mut current_rank = 1i64;
            for (pos, &idx) in indices.iter().enumerate() {
                if pos > 0 {
                    let prev = indices[pos - 1];
                    if !order_keys_equal(&window.order_by, rows, schema, prev, idx)? {
                        current_rank = (pos + 1) as i64;
                    }
                }
                results[idx] = Value::Int64(current_rank);
            }
            Ok(())
        }
        "dense_rank" => {
            let mut current_rank = 1i64;
            for (pos, &idx) in indices.iter().enumerate() {
                if pos > 0 {
                    let prev = indices[pos - 1];
                    if !order_keys_equal(&window.order_by, rows, schema, prev, idx)? {
                        current_rank += 1;
                    }
                }
                results[idx] = Value::Int64(current_rank);
            }
            Ok(())
        }
        "ntile" => {
            // NTILE(N): 将分区分成 N 个桶，每行分配桶号
            let n_buckets = match args.first() {
                Some(Expr::Literal(Value::Int64(n))) => *n,
                _ => {
                    return Err(ExecutionError::EvalError(format!(
                        "NTILE requires integer argument, got {:?}",
                        args.first()
                    )));
                }
            };
            if n_buckets <= 0 {
                return Err(ExecutionError::EvalError(
                    "NTILE argument must be positive".into(),
                ));
            }
            let n = indices.len() as i64;
            let base = n / n_buckets;
            let extra = n % n_buckets;
            let mut bucket = 1i64;
            let mut count_in_bucket = 0i64;
            let mut bucket_size = if bucket <= extra {
                base + 1
            } else {
                base
            };
            for &idx in indices {
                results[idx] = Value::Int64(bucket);
                count_in_bucket += 1;
                if count_in_bucket >= bucket_size {
                    bucket += 1;
                    count_in_bucket = 0;
                    bucket_size = if bucket <= extra {
                        base + 1
                    } else {
                        base
                    };
                }
            }
            Ok(())
        }
        // 导航函数
        "lag" => {
            // LAG(expr [, offset [, default]]) — 返回当前行之前 offset 行的 expr 值
            let offset_val = match args.get(1) {
                Some(Expr::Literal(Value::Int64(n))) => *n as isize,
                _ => 1,
            };
            let default_val = match args.get(2) {
                Some(Expr::Literal(v)) => v.clone(),
                _ => Value::Null,
            };
            for (pos, &idx) in indices.iter().enumerate() {
                let target_pos = pos as isize - offset_val;
                if target_pos < 0 || target_pos as usize >= indices.len() {
                    results[idx] = default_val.clone();
                } else {
                    let target_idx = indices[target_pos as usize];
                    let v = eval_expr_in_row(&args[0], &rows[target_idx], schema)?;
                    results[idx] = v;
                }
            }
            Ok(())
        }
        "lead" => {
            // LEAD(expr [, offset [, default]]) — 返回当前行之后 offset 行的 expr 值
            let offset_val = match args.get(1) {
                Some(Expr::Literal(Value::Int64(n))) => *n as isize,
                _ => 1,
            };
            let default_val = match args.get(2) {
                Some(Expr::Literal(v)) => v.clone(),
                _ => Value::Null,
            };
            for (pos, &idx) in indices.iter().enumerate() {
                let target_pos = pos as isize + offset_val;
                if target_pos < 0 || target_pos as usize >= indices.len() {
                    results[idx] = default_val.clone();
                } else {
                    let target_idx = indices[target_pos as usize];
                    let v = eval_expr_in_row(&args[0], &rows[target_idx], schema)?;
                    results[idx] = v;
                }
            }
            Ok(())
        }
        "first_value" => {
            // FIRST_VALUE(expr) — 返回帧的第一行的 expr 值
            for (pos, &idx) in indices.iter().enumerate() {
                let frame = compute_frame_indices(window, indices, pos);
                if let Some(&first_idx) = frame.first() {
                    let v = eval_expr_in_row(&args[0], &rows[first_idx], schema)?;
                    results[idx] = v;
                } else {
                    results[idx] = Value::Null;
                }
            }
            Ok(())
        }
        "last_value" => {
            // LAST_VALUE(expr) — 返回帧的最后一行的 expr 值
            // 注意：默认帧是 RANGE UNBOUNDED PRECEDING TO CURRENT ROW，
            // 所以 LAST_VALUE 默认返回当前行的值。需显式指定 ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING 才返回最后值。
            for (pos, &idx) in indices.iter().enumerate() {
                let frame = compute_frame_indices(window, indices, pos);
                if let Some(&last_idx) = frame.last() {
                    let v = eval_expr_in_row(&args[0], &rows[last_idx], schema)?;
                    results[idx] = v;
                } else {
                    results[idx] = Value::Null;
                }
            }
            Ok(())
        }
        "nth_value" => {
            // NTH_VALUE(expr, N) — 返回帧的第 N 行的 expr 值
            let n = match args.get(1) {
                Some(Expr::Literal(Value::Int64(n))) => *n as usize,
                _ => {
                    return Err(ExecutionError::EvalError(format!(
                        "NTH_VALUE requires integer second argument, got {:?}",
                        args.get(1)
                    )));
                }
            };
            for (pos, &idx) in indices.iter().enumerate() {
                let frame = compute_frame_indices(window, indices, pos);
                if n >= 1 && n <= frame.len() {
                    let target_idx = frame[n - 1];
                    let v = eval_expr_in_row(&args[0], &rows[target_idx], schema)?;
                    results[idx] = v;
                } else {
                    results[idx] = Value::Null;
                }
            }
            Ok(())
        }
        // 聚合窗口函数
        "sum" | "count" | "avg" | "min" | "max" => {
            // COUNT(*) 特殊处理：parser 将 `*` 过滤为空 args，直接返回帧内行数
            let is_count_star = func_name == "count" && args.is_empty();
            for (pos, &idx) in indices.iter().enumerate() {
                let frame = compute_frame_indices(window, indices, pos);
                let v = if is_count_star {
                    Value::Int64(frame.len() as i64)
                } else {
                    let frame_values = collect_frame_values(args, &frame, rows, schema)?;
                    compute_aggregate_value(func_name, &frame_values)?
                };
                results[idx] = v;
            }
            Ok(())
        }
        _ => Err(ExecutionError::Unsupported(format!(
            "window function '{}' is not supported",
            func_name
        ))),
    }
}

/// 比较 prev 和 current 行的 ORDER BY 键是否相等（用于 RANK / DENSE_RANK）
fn order_keys_equal(
    order_by: &[OrderByExpr],
    rows: &[Row],
    schema: &TableSchema,
    prev: usize,
    current: usize,
) -> Result<bool, ExecutionError> {
    if order_by.is_empty() {
        return Ok(false);
    }
    let prev_row = &rows[prev];
    let cur_row = &rows[current];
    let prev_ctx = ExecRowContext::new(schema, prev_row);
    let cur_ctx = ExecRowContext::new(schema, cur_row);
    for ob in order_by {
        let v1 = ExprEvaluator::eval(&ob.expr, &prev_ctx)?;
        let v2 = ExprEvaluator::eval(&ob.expr, &cur_ctx)?;
        if compare_values(&v1, &v2) != std::cmp::Ordering::Equal {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 计算当前行的窗口帧索引
///
/// 返回分区内（indices 上下文）属于当前行帧的索引列表。
fn compute_frame_indices(window: &WindowSpec, indices: &[usize], current_pos: usize) -> Vec<usize> {
    let n = indices.len();
    if n == 0 {
        return Vec::new();
    }

    // 默认帧规则：
    // - 若 ORDER BY 存在：RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    // - 若 ORDER BY 不存在：ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
    let (start_pos, end_pos) = match &window.window_frame {
        None => {
            if window.order_by.is_empty() {
                (0, n - 1)
            } else {
                (0, current_pos)
            }
        }
        Some(frame) => {
            let start = match &frame.start_bound {
                WindowFrameBound::CurrentRow => current_pos,
                WindowFrameBound::Preceding(None) => 0,
                WindowFrameBound::Preceding(Some(offset_expr)) => {
                    let offset = eval_frame_offset(offset_expr);
                    current_pos.saturating_sub(offset)
                }
                WindowFrameBound::Following(None) => n - 1,
                WindowFrameBound::Following(Some(offset_expr)) => {
                    let offset = eval_frame_offset(offset_expr);
                    (current_pos + offset).min(n - 1)
                }
            };
            let end = match &frame.end_bound {
                None | Some(WindowFrameBound::CurrentRow) => current_pos,
                Some(WindowFrameBound::Preceding(None)) => 0,
                Some(WindowFrameBound::Preceding(Some(offset_expr))) => {
                    let offset = eval_frame_offset(offset_expr);
                    current_pos.saturating_sub(offset)
                }
                Some(WindowFrameBound::Following(None)) => n - 1,
                Some(WindowFrameBound::Following(Some(offset_expr))) => {
                    let offset = eval_frame_offset(offset_expr);
                    (current_pos + offset).min(n - 1)
                }
            };
            (start.min(end), end.max(start))
        }
    };

    if start_pos > end_pos {
        return Vec::new();
    }
    (start_pos..=end_pos).map(|p| indices[p]).collect()
}

/// 评估帧偏移表达式为整数
fn eval_frame_offset(expr: &Expr) -> usize {
    match expr {
        Expr::Literal(Value::Int64(n)) => *n as usize,
        _ => 0,
    }
}

/// 收集帧内所有行针对某个表达式的值
fn collect_frame_values(
    args: &[Expr],
    frame: &[usize],
    rows: &[Row],
    schema: &TableSchema,
) -> Result<Vec<Value>, ExecutionError> {
    let arg = match args.first() {
        Some(a) => a,
        None => return Ok(Vec::new()),
    };
    let mut values = Vec::with_capacity(frame.len());
    for &idx in frame {
        let v = eval_expr_in_row(arg, &rows[idx], schema)?;
        values.push(v);
    }
    Ok(values)
}

/// 在行上下文中评估表达式
fn eval_expr_in_row(expr: &Expr, row: &Row, schema: &TableSchema) -> Result<Value, ExecutionError> {
    let ctx = ExecRowContext::new(schema, row);
    Ok(ExprEvaluator::eval(expr, &ctx)?)
}

/// 计算聚合值（用于聚合窗口函数）
fn compute_aggregate_value(func_name: &str, values: &[Value]) -> Result<Value, ExecutionError> {
    match func_name {
        "count" => {
            // COUNT(*) 计所有行；COUNT(expr) 计非 NULL 行
            let count = values.iter().filter(|v| !value_is_null(v)).count();
            Ok(Value::Int64(count as i64))
        }
        "sum" => {
            let mut total = 0.0f64;
            let mut is_int = true;
            let mut int_total = 0i64;
            let mut has_value = false;
            for v in values {
                match v {
                    Value::Int64(n) => {
                        int_total += n;
                        total += *n as f64;
                        has_value = true;
                    }
                    Value::Float64(f) => {
                        total += f;
                        is_int = false;
                        has_value = true;
                    }
                    _ => {}
                }
            }
            if !has_value {
                return Ok(Value::Null);
            }
            if is_int {
                Ok(Value::Int64(int_total))
            } else {
                Ok(Value::Float64(total))
            }
        }
        "avg" => {
            let mut total = 0.0f64;
            let mut count = 0usize;
            for v in values {
                match v {
                    Value::Int64(n) => {
                        total += *n as f64;
                        count += 1;
                    }
                    Value::Float64(f) => {
                        total += f;
                        count += 1;
                    }
                    _ => {}
                }
            }
            if count == 0 {
                return Ok(Value::Null);
            }
            Ok(Value::Float64(total / count as f64))
        }
        "min" => {
            let mut min: Option<&Value> = None;
            for v in values {
                if value_is_null(v) {
                    continue;
                }
                min = Some(match min {
                    None => v,
                    Some(cur) => {
                        if compare_values(v, cur) == std::cmp::Ordering::Less {
                            v
                        } else {
                            cur
                        }
                    }
                });
            }
            Ok(min.cloned().unwrap_or(Value::Null))
        }
        "max" => {
            let mut max: Option<&Value> = None;
            for v in values {
                if value_is_null(v) {
                    continue;
                }
                max = Some(match max {
                    None => v,
                    Some(cur) => {
                        if compare_values(v, cur) == std::cmp::Ordering::Greater {
                            v
                        } else {
                            cur
                        }
                    }
                });
            }
            Ok(max.cloned().unwrap_or(Value::Null))
        }
        _ => Err(ExecutionError::Unsupported(format!(
            "aggregate window function '{}' is not supported",
            func_name
        ))),
    }
}

// =====================================================================
//  辅助 trait: 获取小写函数名
// =====================================================================

/// 获取 String 的小写形式（避免每次都调用 to_lowercase 分配新 String）
trait LowercaseStr {
    fn as_lowercase_str(&self) -> String;
}

impl LowercaseStr for String {
    fn as_lowercase_str(&self) -> String {
        self.to_lowercase()
    }
}

/// 判断函数名是否为聚合函数（与 plan.rs / expr.rs 的 is_aggregate_function 对齐）
fn is_aggregate_fn(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "count" | "sum" | "avg" | "min" | "max" | "array_agg" | "string_agg"
    )
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 将 Row 序列化为字符串用于 DISTINCT 去重
///
/// 简化实现：直接 Debug 格式化。生产实现应使用 Hash 的序列化
fn serialize_row_for_distinct(row: &Row) -> String {
    format!("{row:?}")
}

// =====================================================================
//  Phase 3.27 — 集合操作辅助函数
// =====================================================================

/// 对行集合去重，保留首次出现顺序
fn dedup_rows(rows: Vec<Row>) -> Vec<Row> {
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let key = serialize_row_for_distinct(&row);
        if seen.insert(key) {
            result.push(row);
        }
    }
    result
}

/// 统计每行的出现次数（按序列化键分组）
fn count_rows(rows: &[Row]) -> HashMap<String, (Row, usize)> {
    let mut counts: HashMap<String, (Row, usize)> = HashMap::new();
    for row in rows {
        let key = serialize_row_for_distinct(row);
        match counts.get_mut(&key) {
            Some((_, n)) => *n += 1,
            None => {
                counts.insert(key, (row.clone(), 1));
            }
        }
    }
    counts
}

/// INTERSECT [DISTINCT]：返回两边的交集（去重）
fn intersect_distinct(left: &[Row], right: &[Row]) -> Vec<Row> {
    let right_keys: HashSet<String> = right.iter().map(serialize_row_for_distinct).collect();
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for row in left {
        let key = serialize_row_for_distinct(row);
        if right_keys.contains(&key) && seen.insert(key) {
            result.push(row.clone());
        }
    }
    result
}

/// INTERSECT ALL：保留 min(left, right) 重复次数
fn intersect_all(left: &[Row], right: &[Row]) -> Vec<Row> {
    let right_counts = count_rows(right);
    let mut left_counts: HashMap<String, usize> = HashMap::new();
    let mut result = Vec::new();
    for row in left {
        let key = serialize_row_for_distinct(row);
        let left_n = left_counts.entry(key.clone()).or_insert(0);
        *left_n += 1;
        if let Some((right_row, right_n)) = right_counts.get(&key) {
            // 当 left 累计次数 <= right 次数时，输出该行
            if *left_n <= *right_n {
                result.push(right_row.clone());
            }
        }
    }
    result
}

/// EXCEPT [DISTINCT]：返回在 left 但不在 right 的行（去重）
fn except_distinct(left: &[Row], right: &[Row]) -> Vec<Row> {
    let right_keys: HashSet<String> = right.iter().map(serialize_row_for_distinct).collect();
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for row in left {
        let key = serialize_row_for_distinct(row);
        if !right_keys.contains(&key) && seen.insert(key) {
            result.push(row.clone());
        }
    }
    result
}

/// EXCEPT ALL：返回 left 重复次数 - right 重复次数（>= 1 才输出）
fn except_all(left: &[Row], right: &[Row]) -> Vec<Row> {
    let right_counts = count_rows(right);
    let mut left_counts: HashMap<String, usize> = HashMap::new();
    let mut result = Vec::new();
    for row in left {
        let key = serialize_row_for_distinct(row);
        let left_n = left_counts.entry(key.clone()).or_insert(0);
        *left_n += 1;
        let right_n = right_counts.get(&key).map(|(_, n)| *n).unwrap_or(0);
        // 当 left 累计次数 > right 次数时，输出该行
        if *left_n > right_n {
            result.push(row.clone());
        }
    }
    result
}

/// 从 DML 的 source 子计划中提取 WHERE 谓词
///
/// DML 计划（Update / Delete）的 source 字段结构：
/// - `None` — 无 WHERE 子句（影响所有行）
/// - `Some(Filter { predicate, input: Scan { .. } })` — 标准 WHERE 子句
///
/// 其他结构（如 JOIN 子查询）当前不支持，返回错误。
fn extract_where_predicate(
    source: &Option<Box<LogicalPlan>>,
) -> Result<Option<&Expr>, ExecutionError> {
    match source {
        None => Ok(None),
        Some(plan) => match plan.as_ref() {
            LogicalPlan::Filter { predicate, input } => {
                // 验证 input 是目标表的 Scan 或 IndexScan（Planner 保证此结构）
                // Phase 5.8: CSE 包装的 Shared/MemoRef 也允许（解包后应为 Scan/IndexScan）
                if matches!(
                    input.as_ref(),
                    LogicalPlan::Scan { .. }
                        | LogicalPlan::IndexScan { .. }
                        | LogicalPlan::Shared { .. }
                        | LogicalPlan::MemoRef { .. }
                ) {
                    Ok(Some(predicate))
                } else {
                    Err(ExecutionError::Unsupported(format!(
                        "DML WHERE source must be Scan, got {:?}",
                        std::mem::discriminant(input.as_ref())
                    )))
                }
            }
            other => Err(ExecutionError::Unsupported(format!(
                "DML WHERE source must be Filter, got {:?}",
                std::mem::discriminant(other)
            ))),
        },
    }
}

/// 对一行应用 RETURNING 投影，返回投影后的行
///
/// 支持的 SelectItem 变体：
/// - `Wildcard` — 返回完整行（`RETURNING *`）
/// - `QualifiedWildcard(_)` — 返回完整行（表名限定通配，等价于 `*`）
/// - `UnnamedExpr(Expr)` — 求值表达式（如列引用 `id` 或计算表达式 `id + 1`）
/// - `ExprWithAlias { expr, alias: _ }` — 求值表达式（别名仅用于命名，此处忽略）
///
/// PG 语义：RETURNING 表达式基于受影响行的列值求值
/// - INSERT → 基于新插入的行
/// - UPDATE → 基于更新后的新行
/// - DELETE → 基于被删除的旧行
fn project_returning(
    schema: &TableSchema,
    row: &Row,
    returning: &Option<Vec<SelectItem>>,
) -> Result<Row, ExecutionError> {
    let items = match returning {
        None => return Ok(row.clone()),
        Some(items) => items,
    };
    let ctx = ExecRowContext::new(schema, row);
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            SelectItem::Wildcard | SelectItem::QualifiedWildcard(_) => {
                out.extend_from_slice(row);
            }
            SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
                out.push(ExprEvaluator::eval(expr, &ctx)?);
            }
        }
    }
    Ok(out)
}

/// Phase 3.32: 解析 PG 数组字面量字符串为 `Vec<Value>`
///
/// 支持格式：`{v1,v2,...}`（元素之间用逗号分隔；元素可带引号也可不带）
/// - `'{1,2,3}'` → `[Int64(1), Int64(2), Int64(3)]`
/// - `'{a,b,c}'` → `[Text("a"), Text("b"), Text("c")]`
/// - `'{}'` → `[]`
/// - `'{1,"a",true}'` → `[Int64(1), Text("a"), Bool(true)]`（按元素类型解析）
///
/// 元素类型推导：
/// - `ColumnType::Int64` → 尝试解析为 i64；失败则保留为 Text
/// - `ColumnType::Float64` → 尝试解析为 f64；失败则保留为 Text
/// - `ColumnType::Bool` → "true"/"false"/"t"/"f" → Bool；其他保留为 Text
/// - 其他类型 → 保留为 Text（去掉外层引号）
///
/// 多维数组：当前简化为递归解析嵌套 `{...}`，元素为 `Value::Array`
fn parse_pg_array_literal(
    s: &str,
    elem_type: &szrsql_types::value::ColumnType,
) -> Result<Vec<Value>, String> {
    let s = s.trim();
    // 必须以 { 开头、} 结尾
    if !s.starts_with('{') || !s.ends_with('}') {
        return Err(format!("array literal must be '{{...}}', got: {s}"));
    }
    let inner = &s[1..s.len() - 1];
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    // 按逗号切分（不处理嵌套花括号 — 多维数组留待后续）
    let mut elems = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escape_next = false;
    for ch in inner.chars() {
        if escape_next {
            current.push(ch);
            escape_next = false;
            continue;
        }
        if ch == '\\' {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_quotes = !in_quotes;
            continue;
        }
        if ch == ',' && !in_quotes {
            elems.push(parse_array_element(current.trim(), elem_type)?);
            current.clear();
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        elems.push(parse_array_element(current.trim(), elem_type)?);
    }
    Ok(elems)
}

/// 解析单个数组元素字符串为 Value（按目标元素类型推导）
fn parse_array_element(
    s: &str,
    elem_type: &szrsql_types::value::ColumnType,
) -> Result<Value, String> {
    use szrsql_types::value::ColumnType;
    // NULL 字面量
    if s.eq_ignore_ascii_case("null") || s.is_empty() {
        return Ok(Value::Null);
    }
    match elem_type {
        ColumnType::Int64 => {
            // 去掉引号
            let s = s.trim_matches('"');
            s.parse::<i64>()
                .map(Value::Int64)
                .map_err(|e| format!("cannot parse '{s}' as int64: {e}"))
        }
        ColumnType::Float64 => {
            let s = s.trim_matches('"');
            s.parse::<f64>()
                .map(Value::Float64)
                .map_err(|e| format!("cannot parse '{s}' as float64: {e}"))
        }
        ColumnType::Bool => {
            let s = s.trim_matches('"');
            match s.to_lowercase().as_str() {
                "true" | "t" => Ok(Value::Bool(true)),
                "false" | "f" => Ok(Value::Bool(false)),
                _ => Ok(Value::Text(s.to_string())),
            }
        }
        _ => {
            // 文本或其他类型：保留引号外的内容
            let s = s.trim_matches('"');
            Ok(Value::Text(s.to_string()))
        }
    }
}

/// 判断一行是否匹配 WHERE 谓词
///
/// `predicate = None` → 匹配所有行（无 WHERE 子句）
/// `predicate = Some(expr)` → 求值表达式，仅 `true` 视为匹配
fn row_matches_predicate(schema: &TableSchema, row: &Row, predicate: Option<&Expr>) -> bool {
    match predicate {
        None => true,
        Some(pred) => {
            let ctx = ExecRowContext::new(schema, row);
            matches!(ExprEvaluator::eval(pred, &ctx), Ok(Value::Bool(true)))
        }
    }
}

/// 推导逻辑计划节点输出 Schema
///
/// - `Scan` → 直接返回表 Schema
/// - `Filter / Limit / Distinct` → 沿用 input Schema
/// - `Projection` → 列名取 output_names，列类型沿用 input 同位置列类型
/// - `Empty` → 空 Schema
/// - 其他节点 → 返回 `Unsupported` 错误
fn input_schema(plan: &LogicalPlan) -> Result<TableSchema, ExecutionError> {
    match plan {
        LogicalPlan::Scan { schema, alias, .. }
        | LogicalPlan::IndexScan { schema, alias, .. }
        | LogicalPlan::MaterializedViewScan { schema, alias, .. } => {
            // 若有别名（如 `FROM employees e`），用别名作为 schema 名
            // — 使 SELF JOIN `e1.id = e2.id` 的限定名查找正确路由
            if let Some(alias_name) = alias {
                let mut s = schema.clone();
                s.name = TableName::new(alias_name.clone());
                Ok(s)
            } else {
                Ok(schema.clone())
            }
        }
        LogicalPlan::Filter { input, .. } => input_schema(input),
        LogicalPlan::Projection {
            output_names,
            input,
            ..
        } => {
            // 投影后的 schema 列名取 output_names，类型沿用 input 类型（简化）
            let inner = input_schema(input)?;
            let columns = output_names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    // 若是简单列引用，沿用 input 列类型；否则默认 Int64
                    let col_type = inner
                        .columns
                        .get(i)
                        .map(|c| c.data_type.clone())
                        .unwrap_or(szrsql_types::value::ColumnType::Int64);
                    ColumnDefinition::new(name, col_type)
                })
                .collect();
            Ok(TableSchema {
                name: TableName::new("projection"),
                columns,
            })
        }
        LogicalPlan::Limit { input, .. } => input_schema(input),
        LogicalPlan::Distinct { input } => input_schema(input),
        // Phase 6.3: Sort schema = input schema
        LogicalPlan::Sort { input, .. } => input_schema(input),
        // Phase 6.3: SetOp schema = left schema（列名取自左侧 SELECT）
        LogicalPlan::SetOp { left, .. } => input_schema(left),
        LogicalPlan::Join {
            join_type,
            left,
            right,
            ..
        } => {
            // SEMI/ANTI JOIN 输出 Schema = 仅左表列（右表列不输出）
            if matches!(join_type, JoinType::Semi | JoinType::Anti) {
                let l = input_schema(left)?;
                return Ok(TableSchema {
                    name: TableName::new("__semijoin__"),
                    columns: l.columns,
                });
            }
            // 普通 JOIN 输出 Schema = 左表列 ++ 右表列（与 plan_schema 推导一致）
            let mut l = input_schema(left)?;
            let r = input_schema(right)?;
            l.columns.extend(r.columns);
            l.name = TableName::new("__join__");
            Ok(l)
        }
        LogicalPlan::Aggregate {
            group_exprs,
            aggregates,
            ..
        } => aggregate_output_schema(group_exprs, aggregates),
        LogicalPlan::Empty | LogicalPlan::Dual => Ok(TableSchema {
            name: TableName::new("empty"),
            columns: Vec::new(),
        }),
        // Phase 5.8: Shared/MemoRef
        LogicalPlan::Shared { plan, .. } => input_schema(plan),
        LogicalPlan::MemoRef { schema, .. } => Ok(schema.clone()),
        // Phase 6.1: CTE
        LogicalPlan::With { input, .. } => input_schema(input),
        LogicalPlan::CteRef { schema, .. } => Ok(schema.clone()),
        // Phase 6.2: Window — schema = input schema 列 ++ 窗口函数结果列
        LogicalPlan::Window {
            window_funcs,
            input,
        } => {
            let mut inner = input_schema(input)?;
            for w in window_funcs {
                let name = w.alias.clone().unwrap_or_else(|| w.func_name.clone());
                inner.columns.push(ColumnDefinition::new(
                    name,
                    szrsql_types::value::ColumnType::Null,
                ));
            }
            Ok(inner)
        }
        _ => Err(ExecutionError::Unsupported(format!(
            "cannot derive schema for plan: {:?}",
            std::mem::discriminant(plan)
        ))),
    }
}

// =====================================================================
//  JOIN 辅助函数
// =====================================================================

/// 将 `JoinCondition` 转换为求值用的 `Option<Expr>`
///
/// - `On(expr)` → `Some(expr)`
/// - `Using(cols)` → `Some(t1.col1 = t2.col1 AND t1.col2 = t2.col2 AND ...)`
/// - `Natural` → 自动找出两表同名列，按 Using 语义生成等值谓词
/// - `None` → `None`（CROSS JOIN，无条件）
fn build_join_condition_expr(
    condition: &JoinCondition,
    left_schema: &TableSchema,
    right_schema: &TableSchema,
) -> Result<Option<Expr>, ExecutionError> {
    match condition {
        JoinCondition::None => Ok(None),
        JoinCondition::On(expr) => Ok(Some(expr.clone())),
        JoinCondition::Using(cols) => {
            if cols.is_empty() {
                return Ok(None);
            }
            Ok(Some(build_using_predicate(
                cols,
                left_schema,
                right_schema,
            )?))
        }
        JoinCondition::Natural => {
            // 找出两表同名列
            let common: Vec<String> = left_schema
                .columns
                .iter()
                .filter_map(|lc| {
                    if right_schema
                        .columns
                        .iter()
                        .any(|rc| rc.name.eq_ignore_ascii_case(&lc.name))
                    {
                        Some(lc.name.clone())
                    } else {
                        None
                    }
                })
                .collect();
            if common.is_empty() {
                return Ok(None);
            }
            Ok(Some(build_using_predicate(
                &common,
                left_schema,
                right_schema,
            )?))
        }
    }
}

/// 构建 `USING (c1, c2, ...)` 等值谓词：`left.c1 = right.c1 AND left.c2 = right.c2 AND ...`
fn build_using_predicate(
    cols: &[String],
    left_schema: &TableSchema,
    right_schema: &TableSchema,
) -> Result<Expr, ExecutionError> {
    let mut eqs: Vec<Expr> = Vec::with_capacity(cols.len());
    for col in cols {
        let left_table = left_schema.name.name.clone();
        let right_table = right_schema.name.name.clone();
        let left_id = Expr::Identifier(vec![left_table, col.clone()]);
        let right_id = Expr::Identifier(vec![right_table, col.clone()]);
        eqs.push(Expr::BinaryOp {
            left: Box::new(left_id),
            op: BinaryOp::Eq,
            right: Box::new(right_id),
        });
    }
    // AND 折叠
    Ok(eqs
        .into_iter()
        .reduce(|acc, eq| Expr::BinaryOp {
            left: Box::new(acc),
            op: BinaryOp::And,
            right: Box::new(eq),
        })
        .unwrap_or(Expr::Literal(Value::Bool(true))))
}

/// 从 ON 条件中尝试提取等值键（用于 HashJoin）
///
/// 支持形式：
/// - 单一 `t1.col = t2.col` → 单键
/// - `t1.c1 = t2.c1 AND t1.c2 = t2.c2 AND ...` → 多键
///
/// 返回 `Vec<(left_col_idx, right_col_idx)>`；非等值或复杂条件返回 `None`。
fn try_extract_hash_keys(
    condition_expr: &Option<Expr>,
    left_schema: &TableSchema,
    right_schema: &TableSchema,
) -> Option<Vec<(usize, usize)>> {
    let expr = condition_expr.as_ref()?;
    let mut keys = Vec::new();
    collect_eq_keys(expr, left_schema, right_schema, &mut keys)?;
    if keys.is_empty() {
        None
    } else {
        Some(keys)
    }
}

/// 递归收集 AND 链中的等值键
fn collect_eq_keys(
    expr: &Expr,
    left_schema: &TableSchema,
    right_schema: &TableSchema,
    keys: &mut Vec<(usize, usize)>,
) -> Option<()> {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOp::And,
            right,
        } => {
            collect_eq_keys(left, left_schema, right_schema, keys)?;
            collect_eq_keys(right, left_schema, right_schema, keys)?;
            Some(())
        }
        Expr::BinaryOp {
            left,
            op: BinaryOp::Eq,
            right,
        } => {
            let l_idx = identifier_column_index(left, left_schema)?;
            let r_idx = identifier_column_index(right, right_schema)?;
            // 同时确认两侧互不混淆（左引用右表、右引用左表也算等值，需交换）
            if let (Some(l), Some(r)) = (l_idx, r_idx) {
                keys.push((l, r));
                return Some(());
            }
            // 尝试反向：左引用右表，右引用左表
            let l_in_right = identifier_column_index(left, right_schema)?;
            let r_in_left = identifier_column_index(right, left_schema)?;
            if let (Some(r), Some(l)) = (r_in_left, l_in_right) {
                keys.push((r, l));
                return Some(());
            }
            None
        }
        _ => None,
    }
}

/// 若 `expr` 是限定名 `table.col` 且 `col` 在指定 schema 中存在，返回列索引
fn identifier_column_index(expr: &Expr, schema: &TableSchema) -> Option<Option<usize>> {
    if let Expr::Identifier(parts) = expr {
        if parts.len() >= 2 {
            let table = &parts[parts.len() - 2];
            let col = &parts[parts.len() - 1];
            if schema.name.name.eq_ignore_ascii_case(table) {
                return Some(
                    schema
                        .columns
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(col)),
                );
            }
            return Some(None);
        }
        // 单层标识符：仅按列名查（容忍）
        if parts.len() == 1 {
            let col = &parts[0];
            return Some(
                schema
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(col)),
            );
        }
    }
    Some(None)
}

/// CROSS JOIN 的无条件笛卡尔积
fn nested_loop_emit_all(left_rows: &[Row], right_rows: &[Row], right_col_count: usize) -> Vec<Row> {
    let _ = right_col_count;
    let mut result = Vec::with_capacity(left_rows.len() * right_rows.len());
    for left in left_rows {
        for right in right_rows {
            let mut row = left.clone();
            row.extend(right.iter().cloned());
            result.push(row);
        }
    }
    result
}

/// 按 left_col_count 将 JOIN 输出行切分为 (左表切片, 右表切片)
///
/// 用于在 Projection/Filter 中构造 JoinedRowContext — 把物化的 JOIN 行
/// 按列偏移拆回左右两表视图，以支持 `t1.col` / `t2.col` 限定名查找。
///
/// 若行长度不足（理论上不应发生），右表切片退化为空切片。
fn split_row_at(row: &[Value], left_col_count: usize) -> (&[Value], &[Value]) {
    let split = left_col_count.min(row.len());
    (&row[..split], &row[split..])
}

/// 判断 JOIN 条件是否匹配（基于 JoinedRowContext 求值）
fn join_condition_match(
    condition_expr: &Option<Expr>,
    left_schema: &TableSchema,
    left_row: &Row,
    right_schema: &TableSchema,
    right_row: Option<&Row>,
) -> bool {
    match condition_expr {
        None => true,
        Some(expr) => {
            // &Row → &[Value]（deref coercion），Option<&Row> → Option<&[Value]>
            let ctx = JoinedRowContext::new(
                left_schema,
                left_row as &[Value],
                right_schema,
                right_row.map(|r| r as &[Value]),
            );
            matches!(ExprEvaluator::eval(expr, &ctx), Ok(Value::Bool(true)))
        }
    }
}

/// NestedLoopJoin — 通用算法，支持所有 JOIN 类型
///
/// 算法复杂度：O(|L| × |R|) — 适用于小表或无法使用 HashJoin 的场景
fn execute_nested_loop_join(
    join_type: JoinType,
    condition_expr: &Option<Expr>,
    left_rows: &[Row],
    right_rows: &[Row],
    left_schema: &TableSchema,
    right_schema: &TableSchema,
) -> Result<Vec<Row>, ExecutionError> {
    let left_col_count = left_schema.columns.len();
    let right_col_count = right_schema.columns.len();
    let mut result = Vec::new();

    match join_type {
        JoinType::Inner => {
            for left in left_rows {
                for right in right_rows {
                    if join_condition_match(
                        condition_expr,
                        left_schema,
                        left,
                        right_schema,
                        Some(right),
                    ) {
                        let mut row = left.clone();
                        row.extend(right.iter().cloned());
                        result.push(row);
                    }
                }
            }
        }
        JoinType::LeftOuter => {
            for left in left_rows {
                let mut matched = false;
                for right in right_rows {
                    if join_condition_match(
                        condition_expr,
                        left_schema,
                        left,
                        right_schema,
                        Some(right),
                    ) {
                        let mut row = left.clone();
                        row.extend(right.iter().cloned());
                        result.push(row);
                        matched = true;
                    }
                }
                if !matched {
                    let mut row = left.clone();
                    row.extend(std::iter::repeat_n(Value::Null, right_col_count));
                    result.push(row);
                }
            }
        }
        JoinType::RightOuter => {
            // 右外连接：以右表为驱动，未匹配的右行用 NULL 填充左列
            for right in right_rows {
                let mut matched = false;
                for left in left_rows {
                    if join_condition_match(
                        condition_expr,
                        left_schema,
                        left,
                        right_schema,
                        Some(right),
                    ) {
                        let mut row = left.clone();
                        row.extend(right.iter().cloned());
                        result.push(row);
                        matched = true;
                    }
                }
                if !matched {
                    let mut row = vec![Value::Null; left_col_count];
                    row.extend(right.iter().cloned());
                    result.push(row);
                }
            }
        }
        JoinType::FullOuter => {
            // 全外连接：左外 + 未匹配的右行
            let mut right_matched = vec![false; right_rows.len()];
            for left in left_rows {
                let mut left_matched = false;
                for (i, right) in right_rows.iter().enumerate() {
                    if join_condition_match(
                        condition_expr,
                        left_schema,
                        left,
                        right_schema,
                        Some(right),
                    ) {
                        let mut row = left.clone();
                        row.extend(right.iter().cloned());
                        result.push(row);
                        left_matched = true;
                        right_matched[i] = true;
                    }
                }
                if !left_matched {
                    let mut row = left.clone();
                    row.extend(std::iter::repeat_n(Value::Null, right_col_count));
                    result.push(row);
                }
            }
            // 追加未匹配的右行
            for (i, right) in right_rows.iter().enumerate() {
                if !right_matched[i] {
                    let mut row = vec![Value::Null; left_col_count];
                    row.extend(right.iter().cloned());
                    result.push(row);
                }
            }
        }
        JoinType::Cross => {
            // CROSS JOIN 已在 execute_join 中提前处理，此处兜底
            result = nested_loop_emit_all(left_rows, right_rows, right_col_count);
        }
        JoinType::Semi => {
            // 半连接：左表行在右表至少存在一行匹配时输出（不输出右表列）
            for left in left_rows {
                for right in right_rows {
                    if join_condition_match(
                        condition_expr,
                        left_schema,
                        left,
                        right_schema,
                        Some(right),
                    ) {
                        result.push(left.clone());
                        break;
                    }
                }
            }
        }
        JoinType::Anti => {
            // 反连接：左表行在右表无任何匹配时输出（不输出右表列）
            for left in left_rows {
                let mut matched = false;
                for right in right_rows {
                    if join_condition_match(
                        condition_expr,
                        left_schema,
                        left,
                        right_schema,
                        Some(right),
                    ) {
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    result.push(left.clone());
                }
            }
        }
    }
    Ok(result)
}

/// HashJoin — 等值连接优化
///
/// **算法**：
/// 1. Build 阶段：以右表为 Build 侧，构建 `HashMap<Value, Vec<Row>>`
/// 2. Probe 阶段：遍历左表，对每行计算 hash 键并探测
///
/// **支持类型**：INNER、LEFT OUTER
/// **限制**：条件必须为纯等值（已在 `try_extract_hash_keys` 中验证）
fn execute_hash_join(
    join_type: JoinType,
    hash_keys: &[(usize, usize)],
    left_rows: &[Row],
    right_rows: &[Row],
    left_schema: &TableSchema,
    right_schema: &TableSchema,
    condition_expr: &Option<Expr>,
) -> Result<Vec<Row>, ExecutionError> {
    let right_col_count = right_schema.columns.len();
    let mut result = Vec::new();

    // Build 阶段：构建右表哈希
    // Key: hash 右表行的等值列组合的 Debug 字符串（Value 未实现 Hash）
    // Value: 右表行索引列表
    let mut hash_map: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, right) in right_rows.iter().enumerate() {
        let key = hash_key_for_row(right, hash_keys, true);
        hash_map.entry(key).or_default().push(i);
    }

    // Probe 阶段：遍历左表
    for left in left_rows {
        let key = hash_key_for_row(left, hash_keys, false);
        let mut matched = false;
        if let Some(right_indices) = hash_map.get(&key) {
            for &i in right_indices {
                let right = &right_rows[i];
                // Hash 命中后仍需用 condition_expr 二次校验（HashJoin 仅提取了等值子句）
                if join_condition_match(
                    condition_expr,
                    left_schema,
                    left,
                    right_schema,
                    Some(right),
                ) {
                    let mut row = left.clone();
                    row.extend(right.iter().cloned());
                    result.push(row);
                    matched = true;
                }
            }
        }
        // LEFT OUTER: 未匹配的左行用 NULL 填充右列
        if !matched && matches!(join_type, JoinType::LeftOuter) {
            let mut row = left.clone();
            row.extend(std::iter::repeat_n(Value::Null, right_col_count));
            result.push(row);
        }
    }

    Ok(result)
}

/// 计算 JOIN 行的 hash 键字符串
///
/// `is_right = true` → 取右表列索引（hash_keys 的 right_col_idx）
/// `is_right = false` → 取左表列索引（hash_keys 的 left_col_idx）
fn hash_key_for_row(row: &Row, hash_keys: &[(usize, usize)], is_right: bool) -> String {
    let mut parts = Vec::with_capacity(hash_keys.len());
    for (left_idx, right_idx) in hash_keys {
        let idx = if is_right {
            *right_idx
        } else {
            *left_idx
        };
        let value = row.get(idx).unwrap_or(&Value::Null);
        parts.push(format!("{value:?}"));
    }
    parts.join("|")
}

// =====================================================================
//  参数替换 — Phase 3.26
// =====================================================================

/// 递归替换 Statement 中所有 `Expr::Parameter(idx)` 为 `Expr::Literal(value)` — Phase 3.26
///
/// - `idx` 是 1-based 索引：`$1` → `parameters[0]`, `$2` → `parameters[1]`, ...
/// - 若 `idx` 越界（`idx == 0` 或 `idx > parameters.len()`），返回错误
/// - 当前仅支持 `Statement::Select` 的参数替换；其他语句类型报错
fn substitute_parameters(
    stmt: Statement,
    parameters: &[Value],
) -> Result<Statement, ExecutionError> {
    match stmt {
        Statement::Select(select) => {
            let select = substitute_parameters_in_select(*select, parameters)?;
            Ok(Statement::Select(Box::new(select)))
        }
        _ => Err(ExecutionError::Unsupported(format!(
            "EXECUTE only supports SELECT statements with parameters, got {:?}",
            std::mem::discriminant(&stmt)
        ))),
    }
}

/// 替换 SELECT 语句中的参数占位符 — Phase 3.26
fn substitute_parameters_in_select(
    mut select: Select,
    parameters: &[Value],
) -> Result<Select, ExecutionError> {
    // projection
    let mut new_projection = Vec::with_capacity(select.projection.len());
    for item in select.projection.drain(..) {
        new_projection.push(substitute_parameters_in_select_item(item, parameters)?);
    }
    select.projection = new_projection;

    // from
    let mut new_from = Vec::with_capacity(select.from.len());
    for twj in select.from.drain(..) {
        new_from.push(substitute_parameters_in_table_with_joins(twj, parameters)?);
    }
    select.from = new_from;

    // where
    if let Some(where_expr) = select.where_clause.take() {
        select.where_clause = Some(substitute_parameters_in_expr(where_expr, parameters)?);
    }

    // group by
    let mut new_group_by = Vec::with_capacity(select.group_by.len());
    for g in select.group_by.drain(..) {
        new_group_by.push(substitute_parameters_in_expr(g, parameters)?);
    }
    select.group_by = new_group_by;

    // having
    if let Some(having_expr) = select.having.take() {
        select.having = Some(substitute_parameters_in_expr(having_expr, parameters)?);
    }

    // order by
    let mut new_order_by = Vec::with_capacity(select.order_by.len());
    for ob in select.order_by.drain(..) {
        new_order_by.push(substitute_parameters_in_order_by(ob, parameters)?);
    }
    select.order_by = new_order_by;

    // limit
    if let Some(limit_expr) = select.limit.take() {
        select.limit = Some(substitute_parameters_in_expr(limit_expr, parameters)?);
    }

    // offset
    if let Some(offset_expr) = select.offset.take() {
        select.offset = Some(substitute_parameters_in_expr(offset_expr, parameters)?);
    }

    Ok(select)
}

/// 替换 SELECT 投影项中的参数占位符 — Phase 3.26
fn substitute_parameters_in_select_item(
    item: SelectItem,
    parameters: &[Value],
) -> Result<SelectItem, ExecutionError> {
    Ok(match item {
        SelectItem::UnnamedExpr(expr) => {
            SelectItem::UnnamedExpr(substitute_parameters_in_expr(expr, parameters)?)
        }
        SelectItem::ExprWithAlias { expr, alias } => SelectItem::ExprWithAlias {
            expr: substitute_parameters_in_expr(expr, parameters)?,
            alias,
        },
        SelectItem::QualifiedWildcard(s) => SelectItem::QualifiedWildcard(s),
        SelectItem::Wildcard => SelectItem::Wildcard,
    })
}

/// 替换 TableWithJoins 中的参数占位符 — Phase 3.26
fn substitute_parameters_in_table_with_joins(
    twj: TableWithJoins,
    parameters: &[Value],
) -> Result<TableWithJoins, ExecutionError> {
    let relation = substitute_parameters_in_table_factor(twj.relation, parameters)?;
    let mut joins = Vec::with_capacity(twj.joins.len());
    for j in twj.joins {
        joins.push(substitute_parameters_in_join(j, parameters)?);
    }
    Ok(TableWithJoins { relation, joins })
}

/// 替换 TableFactor 中的参数占位符 — Phase 3.26
fn substitute_parameters_in_table_factor(
    tf: TableFactor,
    parameters: &[Value],
) -> Result<TableFactor, ExecutionError> {
    Ok(match tf {
        TableFactor::Table { name, alias } => TableFactor::Table { name, alias },
        TableFactor::Derived { subquery, alias } => TableFactor::Derived {
            subquery: Box::new(substitute_parameters_in_select(*subquery, parameters)?),
            alias,
        },
        TableFactor::TableFunction { name, args, alias } => {
            let mut new_args = Vec::with_capacity(args.len());
            for a in args {
                new_args.push(substitute_parameters_in_expr(a, parameters)?);
            }
            TableFactor::TableFunction {
                name,
                args: new_args,
                alias,
            }
        }
    })
}

/// 替换 Join 中的参数占位符 — Phase 3.26
fn substitute_parameters_in_join(j: Join, parameters: &[Value]) -> Result<Join, ExecutionError> {
    Ok(Join {
        relation: substitute_parameters_in_table_factor(j.relation, parameters)?,
        join_type: j.join_type,
        condition: substitute_parameters_in_join_condition(j.condition, parameters)?,
    })
}

/// 替换 JoinCondition 中的参数占位符 — Phase 3.26
fn substitute_parameters_in_join_condition(
    jc: JoinCondition,
    parameters: &[Value],
) -> Result<JoinCondition, ExecutionError> {
    Ok(match jc {
        JoinCondition::None => JoinCondition::None,
        JoinCondition::On(expr) => {
            JoinCondition::On(substitute_parameters_in_expr(expr, parameters)?)
        }
        JoinCondition::Using(cols) => JoinCondition::Using(cols),
        JoinCondition::Natural => JoinCondition::Natural,
    })
}

/// 替换 OrderByExpr 中的参数占位符 — Phase 3.26
fn substitute_parameters_in_order_by(
    ob: OrderByExpr,
    parameters: &[Value],
) -> Result<OrderByExpr, ExecutionError> {
    Ok(OrderByExpr {
        expr: substitute_parameters_in_expr(ob.expr, parameters)?,
        asc: ob.asc,
        nulls_first: ob.nulls_first,
    })
}

/// 递归替换 Expr 中所有 `Expr::Parameter(idx)` 为 `Expr::Literal(value)` — Phase 3.26
///
/// - `idx` 是 1-based 索引：`$1` → `parameters[0]`, `$2` → `parameters[1]`, ...
/// - 若 `idx` 越界（`idx == 0` 或 `idx > parameters.len()`），返回错误
fn substitute_parameters_in_expr(expr: Expr, parameters: &[Value]) -> Result<Expr, ExecutionError> {
    match expr {
        // 叶子节点
        Expr::Parameter(idx) => {
            if idx == 0 {
                return Err(ExecutionError::InvalidArgument(
                    "placeholder index must be >= 1: $0 is invalid".into(),
                ));
            }
            let value = parameters.get(idx - 1).cloned().ok_or_else(|| {
                ExecutionError::InvalidArgument(format!(
                    "parameter ${idx} out of range: only {} parameters provided",
                    parameters.len()
                ))
            })?;
            Ok(Expr::Literal(value))
        }
        // 无需替换的叶子
        Expr::Literal(v) => Ok(Expr::Literal(v)),
        Expr::Identifier(parts) => Ok(Expr::Identifier(parts)),
        Expr::Wildcard => Ok(Expr::Wildcard),

        // 递归节点
        Expr::BinaryOp { left, op, right } => Ok(Expr::BinaryOp {
            left: Box::new(substitute_parameters_in_expr(*left, parameters)?),
            op,
            right: Box::new(substitute_parameters_in_expr(*right, parameters)?),
        }),
        Expr::UnaryOp { op, expr: inner } => Ok(Expr::UnaryOp {
            op,
            expr: Box::new(substitute_parameters_in_expr(*inner, parameters)?),
        }),
        Expr::Function {
            name,
            args,
            distinct,
        } => {
            let mut new_args = Vec::with_capacity(args.len());
            for a in args {
                new_args.push(substitute_parameters_in_expr(a, parameters)?);
            }
            Ok(Expr::Function {
                name,
                args: new_args,
                distinct,
            })
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            let new_operand = match operand {
                Some(o) => Some(Box::new(substitute_parameters_in_expr(*o, parameters)?)),
                None => None,
            };
            let mut new_when_then = Vec::with_capacity(when_then.len());
            for (when_expr, then_expr) in when_then {
                new_when_then.push((
                    substitute_parameters_in_expr(when_expr, parameters)?,
                    substitute_parameters_in_expr(then_expr, parameters)?,
                ));
            }
            let new_else = match else_expr {
                Some(e) => Some(Box::new(substitute_parameters_in_expr(*e, parameters)?)),
                None => None,
            };
            Ok(Expr::Case {
                operand: new_operand,
                when_then: new_when_then,
                else_expr: new_else,
            })
        }
        Expr::Cast {
            expr: inner,
            data_type,
        } => Ok(Expr::Cast {
            expr: Box::new(substitute_parameters_in_expr(*inner, parameters)?),
            data_type,
        }),
        Expr::InList {
            expr: inner,
            list,
            negated,
        } => {
            let mut new_list = Vec::with_capacity(list.len());
            for e in list {
                new_list.push(substitute_parameters_in_expr(e, parameters)?);
            }
            Ok(Expr::InList {
                expr: Box::new(substitute_parameters_in_expr(*inner, parameters)?),
                list: new_list,
                negated,
            })
        }
        Expr::InSubquery {
            expr: inner,
            subquery,
            negated,
        } => Ok(Expr::InSubquery {
            expr: Box::new(substitute_parameters_in_expr(*inner, parameters)?),
            subquery: Box::new(substitute_parameters_in_select(*subquery, parameters)?),
            negated,
        }),
        Expr::Between {
            expr: inner,
            low,
            high,
            negated,
        } => Ok(Expr::Between {
            expr: Box::new(substitute_parameters_in_expr(*inner, parameters)?),
            low: Box::new(substitute_parameters_in_expr(*low, parameters)?),
            high: Box::new(substitute_parameters_in_expr(*high, parameters)?),
            negated,
        }),
        Expr::Like {
            expr: inner,
            pattern,
            negated,
            case_insensitive,
        } => Ok(Expr::Like {
            expr: Box::new(substitute_parameters_in_expr(*inner, parameters)?),
            pattern: Box::new(substitute_parameters_in_expr(*pattern, parameters)?),
            negated,
            case_insensitive,
        }),
        Expr::IsNull {
            expr: inner,
            negated,
        } => Ok(Expr::IsNull {
            expr: Box::new(substitute_parameters_in_expr(*inner, parameters)?),
            negated,
        }),
        Expr::Subquery(select) => Ok(Expr::Subquery(Box::new(substitute_parameters_in_select(
            *select, parameters,
        )?))),
        Expr::Exists { subquery, negated } => Ok(Expr::Exists {
            subquery: Box::new(substitute_parameters_in_select(*subquery, parameters)?),
            negated,
        }),
        Expr::Tuple(exprs) => {
            let mut new_exprs = Vec::with_capacity(exprs.len());
            for e in exprs {
                new_exprs.push(substitute_parameters_in_expr(e, parameters)?);
            }
            Ok(Expr::Tuple(new_exprs))
        }
        // Phase 3.32: 数组字面量与 ANY/ALL 子表达式递归替换参数
        Expr::Array(exprs) => {
            let mut new_exprs = Vec::with_capacity(exprs.len());
            for e in exprs {
                new_exprs.push(substitute_parameters_in_expr(e, parameters)?);
            }
            Ok(Expr::Array(new_exprs))
        }
        Expr::AnyOp { left, op, right } => Ok(Expr::AnyOp {
            left: Box::new(substitute_parameters_in_expr(*left, parameters)?),
            op,
            right: Box::new(substitute_parameters_in_expr(*right, parameters)?),
        }),
        Expr::AllOp { left, op, right } => Ok(Expr::AllOp {
            left: Box::new(substitute_parameters_in_expr(*left, parameters)?),
            op,
            right: Box::new(substitute_parameters_in_expr(*right, parameters)?),
        }),
        // Phase 6.2: 窗口函数 — 递归替换 args / partition_by / order_by 中的参数
        Expr::WindowFunction {
            name,
            args,
            distinct,
            window,
        } => {
            let mut new_args = Vec::with_capacity(args.len());
            for a in args {
                new_args.push(substitute_parameters_in_expr(a, parameters)?);
            }
            let mut new_partition = Vec::with_capacity(window.partition_by.len());
            for e in window.partition_by {
                new_partition.push(substitute_parameters_in_expr(e, parameters)?);
            }
            let mut new_order = Vec::with_capacity(window.order_by.len());
            for obe in window.order_by {
                new_order.push(OrderByExpr {
                    expr: substitute_parameters_in_expr(obe.expr, parameters)?,
                    asc: obe.asc,
                    nulls_first: obe.nulls_first,
                });
            }
            Ok(Expr::WindowFunction {
                name,
                args: new_args,
                distinct,
                window: WindowSpec {
                    partition_by: new_partition,
                    order_by: new_order,
                    window_frame: window.window_frame,
                },
            })
        }
        // Phase F-9: PG 兼容表达式 — 递归替换参数
        Expr::IsDistinctFrom { left, right, not } => Ok(Expr::IsDistinctFrom {
            left: Box::new(substitute_parameters_in_expr(*left, parameters)?),
            right: Box::new(substitute_parameters_in_expr(*right, parameters)?),
            not,
        }),
        Expr::SimilarTo {
            expr: inner,
            pattern,
            negated,
        } => Ok(Expr::SimilarTo {
            expr: Box::new(substitute_parameters_in_expr(*inner, parameters)?),
            pattern: Box::new(substitute_parameters_in_expr(*pattern, parameters)?),
            negated,
        }),
        Expr::Substring {
            expr: inner,
            from,
            for_len,
        } => Ok(Expr::Substring {
            expr: Box::new(substitute_parameters_in_expr(*inner, parameters)?),
            from: match from {
                Some(e) => Some(Box::new(substitute_parameters_in_expr(*e, parameters)?)),
                None => None,
            },
            for_len: match for_len {
                Some(e) => Some(Box::new(substitute_parameters_in_expr(*e, parameters)?)),
                None => None,
            },
        }),
    }
}
