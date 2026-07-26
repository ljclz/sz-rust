//! 触发器引擎 — Phase 6.4
//!
//! 提供触发器函数注册与触发执行能力。设计目标：
//!
//! - **Rust 内建触发器函数**：通过 [`TriggerRegistry`] 注册 `Arc<dyn TriggerFunction>`
//! - **触发时机**：BEFORE / AFTER / INSTEAD OF × ROW / STATEMENT
//! - **触发事件**：INSERT / UPDATE / DELETE / TRUNCATE
//! - **行级触发器可访问 NEW/OLD 行**；BEFORE 行级触发器可修改 NEW 行或跳过该行
//! - **语句级触发器**只在语句执行前后触发一次，无法访问行数据
//!
//! # 触发器执行顺序（与 PG 一致）
//!
//! 1. `INSTEAD OF` 触发器（仅视图，当前 Phase 6.4 记录但暂按 BEFORE 语义执行）
//! 2. `BEFORE` 语句级触发器
//! 3. 对每行：
//!    - `BEFORE` 行级触发器（可修改 NEW 或跳过）
//!    - DML 实际操作
//!    - `AFTER` 行级触发器
//! 4. `AFTER` 语句级触发器
//!
//! # PL/pgSQL 触发器函数
//!
//! Phase 6.4 仅支持 Rust 注册的触发器函数；PL/pgSQL 函数体执行留待 Phase 6.5/6.6 实现。

use crate::ast::{TriggerDefinition, TriggerEvent, TriggerLevel, TriggerTiming};
use crate::executor::{ExecutionError, Row};
use crate::plan::TableSchema;
use std::collections::HashMap;
use std::sync::Arc;

// =====================================================================
//  触发器执行上下文
// =====================================================================

/// 触发器执行上下文
///
/// 在触发器函数被调用时构造，提供触发事件与行数据访问。
///
/// # 字段语义
/// - INSERT：`new_row` = 待插入行；`old_row` = None
/// - UPDATE：`new_row` = 更新后行；`old_row` = 更新前旧行
/// - DELETE：`new_row` = None；`old_row` = 被删除行
/// - TRUNCATE：`new_row` = `old_row` = None（仅语句级触发器可触发）
/// - 语句级触发器：`new_row` = `old_row` = None
#[derive(Debug, Clone)]
pub struct TriggerContext<'a> {
    /// 触发器所属表名（来自 `TriggerDefinition.table`）
    pub table_name: &'a str,
    /// 触发器名（来自 `TriggerDefinition.name`）
    pub trigger_name: &'a str,
    /// 触发时机（BEFORE / AFTER / INSTEAD OF）
    pub timing: TriggerTiming,
    /// 触发事件（INSERT / UPDATE(cols) / DELETE / TRUNCATE）
    pub event: &'a TriggerEvent,
    /// 触发级别（ROW / STATEMENT）
    pub level: TriggerLevel,
    /// 表 Schema
    pub schema: &'a TableSchema,
    /// NEW 行（INSERT/UPDATE 行级触发器有效；其他情况为 None）
    pub new_row: Option<&'a Row>,
    /// OLD 行（UPDATE/DELETE 行级触发器有效；其他情况为 None）
    pub old_row: Option<&'a Row>,
}

impl<'a> TriggerContext<'a> {
    /// 创建行级触发器上下文
    pub fn for_row(
        table_name: &'a str,
        trigger_name: &'a str,
        timing: TriggerTiming,
        event: &'a TriggerEvent,
        schema: &'a TableSchema,
        new_row: Option<&'a Row>,
        old_row: Option<&'a Row>,
    ) -> Self {
        Self {
            table_name,
            trigger_name,
            timing,
            event,
            level: TriggerLevel::Row,
            schema,
            new_row,
            old_row,
        }
    }

    /// 创建语句级触发器上下文
    pub fn for_statement(
        table_name: &'a str,
        trigger_name: &'a str,
        timing: TriggerTiming,
        event: &'a TriggerEvent,
        schema: &'a TableSchema,
    ) -> Self {
        Self {
            table_name,
            trigger_name,
            timing,
            event,
            level: TriggerLevel::Statement,
            schema,
            new_row: None,
            old_row: None,
        }
    }
}

// =====================================================================
//  触发器执行结果
// =====================================================================

/// 触发器函数返回值
///
/// 仅 BEFORE 行级触发器可以返回 `Modify` 或 `SkipRow`；其他情况返回 `Continue`。
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerOutcome {
    /// 继续执行（无修改）
    ///
    /// - BEFORE 行级触发器：使用原 NEW 行继续 DML
    /// - AFTER 触发器：忽略返回值
    Continue,
    /// 修改 NEW 行（仅 BEFORE INSERT/UPDATE 行级触发器有效）
    ///
    /// 执行器将使用修改后的 NEW 行执行 DML。
    /// 返回的 Row 长度必须与 schema.columns.len() 一致。
    Modify(Row),
    /// 跳过当前行（仅 BEFORE 行级触发器有效）
    ///
    /// - INSERT：该行不插入
    /// - UPDATE：该行不更新
    /// - DELETE：该行不删除
    SkipRow,
}

// =====================================================================
//  触发器函数 trait
// =====================================================================

/// 触发器函数 trait
///
/// 由 Rust 调用方实现并通过 [`TriggerRegistry::register`] 注册。
/// 触发器函数按 `func_name` 在 `TriggerDefinition.func_name` 中引用。
///
/// # 实现要求
/// - 必须 `Send + Sync`（触发器注册表可跨线程共享）
/// - 应避免长耗时操作（会阻塞 DML）
/// - AFTER 触发器返回值被忽略（强制为 `Continue`）
pub trait TriggerFunction: Send + Sync {
    /// 触发器函数入口
    ///
    /// # 参数
    /// - `ctx`：触发上下文，提供 NEW/OLD 行与事件信息
    ///
    /// # 返回
    /// - `Ok(TriggerOutcome::Continue)` — 继续
    /// - `Ok(TriggerOutcome::Modify(row))` — 修改 NEW 行（仅 BEFORE 行级）
    /// - `Ok(TriggerOutcome::SkipRow)` — 跳过该行（仅 BEFORE 行级）
    /// - `Err(_)` — 触发器执行错误，将中止 DML
    fn call(&self, ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError>;
}

// =====================================================================
//  触发器注册表
// =====================================================================

/// 触发器函数注册表
///
/// 维护 `func_name → Arc<dyn TriggerFunction>` 映射，由执行器持有引用。
///
/// # 用法
/// ```
/// use szrsql_sql::trigger::{TriggerRegistry, TriggerOutcome, TriggerContext};
/// use szrsql_sql::executor::ExecutionError;
/// use std::sync::Arc;
///
/// struct MyTrigger;
/// impl szrsql_sql::trigger::TriggerFunction for MyTrigger {
///     fn call(&self, _ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
///         Ok(TriggerOutcome::Continue)
///     }
/// }
///
/// let mut registry = TriggerRegistry::new();
/// registry.register("audit_insert", Arc::new(MyTrigger));
/// assert!(registry.get("audit_insert").is_some());
/// ```
#[derive(Default)]
pub struct TriggerRegistry {
    /// func_name（小写）→ 触发器函数
    functions: HashMap<String, Arc<dyn TriggerFunction>>,
}

impl std::fmt::Debug for TriggerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TriggerRegistry")
            .field("registered_count", &self.functions.len())
            .field("names", &self.functions.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl TriggerRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
        }
    }

    /// 注册触发器函数（若同名已存在则覆盖）
    pub fn register(&mut self, name: &str, func: Arc<dyn TriggerFunction>) {
        self.functions.insert(name.to_lowercase(), func);
    }

    /// 按名查找触发器函数
    pub fn get(&self, name: &str) -> Option<&Arc<dyn TriggerFunction>> {
        self.functions.get(&name.to_lowercase())
    }

    /// 已注册的触发器函数数量
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// 列出所有已注册函数名（小写）
    pub fn names(&self) -> Vec<String> {
        self.functions.keys().cloned().collect()
    }
}

// =====================================================================
//  触发器筛选与执行
// =====================================================================

/// 触发器匹配查询条件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmlKind {
    /// INSERT 操作
    Insert,
    /// UPDATE 操作
    Update,
    /// DELETE 操作
    Delete,
    /// TRUNCATE 操作
    Truncate,
}

impl DmlKind {
    /// 将 DML 类型转换为对应的 TriggerEvent（无列列表的 UPDATE）
    pub fn to_trigger_event(self) -> TriggerEvent {
        match self {
            DmlKind::Insert => TriggerEvent::Insert,
            DmlKind::Update => TriggerEvent::Update(Vec::new()),
            DmlKind::Delete => TriggerEvent::Delete,
            DmlKind::Truncate => TriggerEvent::Truncate,
        }
    }

    /// 判断触发器事件是否匹配当前 DML 类型
    ///
    /// - INSERT ↔ TriggerEvent::Insert
    /// - DELETE ↔ TriggerEvent::Delete
    /// - TRUNCATE ↔ TriggerEvent::Truncate
    /// - UPDATE ↔ TriggerEvent::Update(_)（任意列或全部列）
    pub fn matches_event(self, event: &TriggerEvent) -> bool {
        matches!(
            (self, event),
            (DmlKind::Insert, TriggerEvent::Insert)
                | (DmlKind::Update, TriggerEvent::Update(_))
                | (DmlKind::Delete, TriggerEvent::Delete)
                | (DmlKind::Truncate, TriggerEvent::Truncate)
        )
    }
}

/// 判断触发器是否应被当前 DML 触发
///
/// # 规则
/// 1. DML 类型必须匹配触发器事件之一（INSERT/UPDATE/DELETE/TRUNCATE）
/// 2. UPDATE 触发器带列列表时，当前 UPDATE 必须涉及其中至少一列（当前 Phase 6.4 简化：
///    若无法获取实际更新列集合，则匹配任意列 UPDATE 触发器；带列列表触发器仅在传入
///    `updated_columns` 时才精确过滤）
/// 3. 触发器必须启用（`enabled == true`）
pub fn should_fire(
    trigger: &TriggerDefinition,
    kind: DmlKind,
    updated_columns: Option<&[String]>,
) -> bool {
    if !trigger.enabled {
        return false;
    }
    // 任一事件匹配即触发（支持 INSERT OR UPDATE OR DELETE）
    let mut matched = false;
    for ev in &trigger.events {
        if kind.matches_event(ev) {
            // UPDATE 带列列表时，进行精确过滤
            if let TriggerEvent::Update(cols) = ev {
                if !cols.is_empty() {
                    if let Some(updated) = updated_columns {
                        // 触发器列与实际更新列有交集才触发
                        let has_intersection = cols
                            .iter()
                            .any(|c| updated.iter().any(|u| u.eq_ignore_ascii_case(c)));
                        if !has_intersection {
                            continue; // 此事件不触发，但其他事件可能触发
                        }
                    }
                    // 未提供 updated_columns 时，按 PG 语义：仅当实际更新列与触发器列有交集
                    // 才触发；为简化 Phase 6.4，无信息时默认触发（保守策略）
                }
            }
            matched = true;
            break;
        }
    }
    matched
}

/// 触发器执行结果（内部使用）
#[derive(Debug, Clone)]
pub enum FireResult {
    /// 继续执行 DML（可能使用修改后的 NEW 行）
    ContinueWith(Option<Row>),
    /// 跳过当前行
    SkipRow,
}

/// 触发单个触发器
///
/// 用于执行器在 DML 流程中调用。返回 [`FireResult`] 指导后续执行。
///
/// # 参数
/// - `func`：触发器函数
/// - `ctx`：触发上下文
/// - `respect_outcome`：是否尊重 `Modify`/`SkipRow` 返回值
///   - true：BEFORE 行级触发器
///   - false：AFTER/语句级触发器（强制 `Continue`）
fn fire_one(
    func: &dyn TriggerFunction,
    ctx: &TriggerContext,
    respect_outcome: bool,
) -> Result<FireResult, ExecutionError> {
    let outcome = func.call(ctx)?;
    if !respect_outcome {
        // AFTER / 语句级触发器：忽略返回值
        return Ok(FireResult::ContinueWith(None));
    }
    match outcome {
        TriggerOutcome::Continue => Ok(FireResult::ContinueWith(None)),
        TriggerOutcome::Modify(row) => {
            // 校验行长度
            if row.len() != ctx.schema.columns.len() {
                return Err(ExecutionError::InvalidArgument(format!(
                    "trigger {} returned row with {} columns, expected {}",
                    ctx.trigger_name,
                    row.len(),
                    ctx.schema.columns.len()
                )));
            }
            Ok(FireResult::ContinueWith(Some(row)))
        }
        TriggerOutcome::SkipRow => Ok(FireResult::SkipRow),
    }
}

/// 在 DML 行操作前后触发匹配的触发器
///
/// # BEFORE 行级触发器
/// - 按定义顺序（catalog 中的存储顺序）依次触发
/// - 任一触发器返回 `SkipRow`：跳过该行（不再触发后续 BEFORE 触发器）
/// - 任一触发器返回 `Modify(row)`：后续触发器看到修改后的 NEW 行
/// - 任一触发器返回 `Err`：中止 DML
///
/// # AFTER 行级触发器
/// - 在行实际写入存储后触发
/// - 按定义顺序触发
/// - 返回值被忽略
///
/// # 参数
/// - `registry`：触发器函数注册表
/// - `triggers`：从 `catalog.list_triggers(table)` 获取的触发器定义列表
/// - `kind`：当前 DML 类型
/// - `timing`：要触发的时机（BEFORE 或 AFTER）
/// - `table_name`：表名
/// - `schema`：表 Schema
/// - `new_row` / `old_row`：行数据
/// - `updated_columns`：UPDATE 操作的更新列列表（仅 UPDATE 有效）
///
/// # 返回
/// - `Ok(FireResult::ContinueWith(modified))`：继续；`modified` 为修改后的 NEW 行（仅 BEFORE 触发器）
/// - `Ok(FireResult::SkipRow)`：BEFORE 触发器要求跳过
/// - `Err(_)`：触发器执行错误
#[allow(clippy::too_many_arguments)]
pub fn fire_row_triggers(
    registry: &TriggerRegistry,
    triggers: &[TriggerDefinition],
    kind: DmlKind,
    timing: TriggerTiming,
    table_name: &str,
    schema: &TableSchema,
    new_row: Option<&Row>,
    old_row: Option<&Row>,
    updated_columns: Option<&[String]>,
) -> Result<FireResult, ExecutionError> {
    let respect_outcome = timing == TriggerTiming::Before;
    // 当前可变的 NEW 行（BEFORE 触发器可能修改）
    // 初始化为 None：表示"无修改"，调用方应使用原 new_row；
    // 触发器返回 Modify 时设为 Some(modified)，调用方应使用修改后的行。
    let mut current_new: Option<Row> = None;
    let dummy_event = kind.to_trigger_event();

    for trig in triggers {
        // 仅处理行级、指定时机、事件匹配的触发器
        if trig.level != TriggerLevel::Row || trig.timing != timing {
            continue;
        }
        if !should_fire(trig, kind, updated_columns) {
            continue;
        }
        // WHEN 条件评估（仅当存在时；当前 Phase 6.4 简化：WHEN 留待 Phase 6.5 实现，
        // 因为 WHEN 需要 ExprEvaluator + 行上下文，而此处避免引入循环依赖）
        // TODO Phase 6.5: 评估 trig.when_clause
        if trig.when_clause.is_some() {
            // 暂时跳过 WHEN 条件的触发器（避免错误触发）
            // 注：这是 Phase 6.4 已知限制，将在 Phase 6.5 实现
            continue;
        }
        let func = match registry.get(&trig.func_name) {
            Some(f) => f,
            None => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "trigger function not registered: {} (referenced by trigger {})",
                    trig.func_name, trig.name
                )));
            }
        };
        // 构造上下文：使用当前（可能已修改的）NEW 行；
        // 若 current_new 为 None（未被修改），回退到原 new_row
        let new_ref = current_new.as_ref().or(new_row);
        let ctx = TriggerContext::for_row(
            table_name,
            &trig.name,
            timing,
            &dummy_event,
            schema,
            new_ref,
            old_row,
        );
        match fire_one(func.as_ref(), &ctx, respect_outcome)? {
            FireResult::ContinueWith(None) => {
                // 无修改，继续下一个触发器
            }
            FireResult::ContinueWith(Some(modified)) => {
                // BEFORE 触发器修改了 NEW 行
                current_new = Some(modified);
            }
            FireResult::SkipRow => {
                // 仅 BEFORE 触发器可返回 SkipRow
                return Ok(FireResult::SkipRow);
            }
        }
    }
    Ok(FireResult::ContinueWith(current_new))
}

/// 在 DML 语句前后触发语句级触发器
///
/// # 参数
/// - `registry`：触发器函数注册表
/// - `triggers`：触发器定义列表
/// - `kind`：DML 类型
/// - `timing`：BEFORE 或 AFTER
/// - `table_name`：表名
/// - `schema`：表 Schema
pub fn fire_statement_triggers(
    registry: &TriggerRegistry,
    triggers: &[TriggerDefinition],
    kind: DmlKind,
    timing: TriggerTiming,
    table_name: &str,
    schema: &TableSchema,
) -> Result<(), ExecutionError> {
    let dummy_event = kind.to_trigger_event();
    for trig in triggers {
        // 仅处理语句级、指定时机、事件匹配的触发器
        if trig.level != TriggerLevel::Statement || trig.timing != timing {
            continue;
        }
        if !should_fire(trig, kind, None) {
            continue;
        }
        // WHEN 条件在语句级触发器上无效（PG 语义），跳过评估
        if trig.when_clause.is_some() {
            continue;
        }
        let func = match registry.get(&trig.func_name) {
            Some(f) => f,
            None => {
                return Err(ExecutionError::InvalidArgument(format!(
                    "trigger function not registered: {} (referenced by trigger {})",
                    trig.func_name, trig.name
                )));
            }
        };
        let ctx =
            TriggerContext::for_statement(table_name, &trig.name, timing, &dummy_event, schema);
        fire_one(func.as_ref(), &ctx, false)?;
    }
    Ok(())
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{TableName, TriggerDefinition, TriggerEvent, TriggerLevel, TriggerTiming};
    use crate::executor::ExecutionError;
    use crate::plan::TableSchema;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use szrsql_types::value::{ColumnType, Value};

    /// 构造简单 schema：id INT, name TEXT
    fn make_schema() -> TableSchema {
        use crate::ast::ColumnDefinition;
        TableSchema {
            name: TableName::new("t"),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("name", ColumnType::Text),
            ],
        }
    }

    /// 计数触发器：统计被调用次数
    struct CountingTrigger {
        counter: Arc<AtomicUsize>,
    }

    impl TriggerFunction for CountingTrigger {
        fn call(&self, _ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(TriggerOutcome::Continue)
        }
    }

    /// 修改 NEW 行的触发器：把 name 列改为固定值
    struct ModifyNameTrigger;

    impl TriggerFunction for ModifyNameTrigger {
        fn call(&self, ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
            if let Some(row) = ctx.new_row {
                let mut modified = row.clone();
                modified[1] = Value::Text("modified".to_string());
                Ok(TriggerOutcome::Modify(modified))
            } else {
                Ok(TriggerOutcome::Continue)
            }
        }
    }

    /// 跳过行的触发器
    struct SkipRowTrigger;

    impl TriggerFunction for SkipRowTrigger {
        fn call(&self, _ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
            Ok(TriggerOutcome::SkipRow)
        }
    }

    /// 出错触发器
    struct ErrorTrigger;

    impl TriggerFunction for ErrorTrigger {
        fn call(&self, _ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
            Err(ExecutionError::InvalidArgument("trigger error".to_string()))
        }
    }

    fn make_trigger(
        name: &str,
        func_name: &str,
        timing: TriggerTiming,
        level: TriggerLevel,
        events: Vec<TriggerEvent>,
    ) -> TriggerDefinition {
        TriggerDefinition {
            name: name.to_string(),
            table: TableName::new("t"),
            timing,
            level,
            events,
            when_clause: None,
            func_name: func_name.to_string(),
            func_args: Vec::new(),
            enabled: true,
            is_constraint: false,
        }
    }

    #[test]
    fn registry_register_and_get() {
        let mut reg = TriggerRegistry::new();
        assert!(reg.is_empty());
        reg.register(
            "audit",
            Arc::new(CountingTrigger {
                counter: Arc::new(AtomicUsize::new(0)),
            }),
        );
        assert_eq!(reg.len(), 1);
        assert!(reg.get("audit").is_some());
        assert!(reg.get("AUDIT").is_some()); // 大小写不敏感
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn registry_names() {
        let mut reg = TriggerRegistry::new();
        reg.register(
            "foo",
            Arc::new(CountingTrigger {
                counter: Arc::new(AtomicUsize::new(0)),
            }),
        );
        reg.register(
            "Bar",
            Arc::new(CountingTrigger {
                counter: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let mut names = reg.names();
        names.sort();
        assert_eq!(names, vec!["bar".to_string(), "foo".to_string()]);
    }

    #[test]
    fn dml_kind_matches_event() {
        assert!(DmlKind::Insert.matches_event(&TriggerEvent::Insert));
        assert!(!DmlKind::Insert.matches_event(&TriggerEvent::Delete));
        assert!(DmlKind::Update.matches_event(&TriggerEvent::Update(vec![])));
        assert!(DmlKind::Update.matches_event(&TriggerEvent::Update(vec!["col".to_string()])));
        assert!(!DmlKind::Update.matches_event(&TriggerEvent::Insert));
        assert!(DmlKind::Delete.matches_event(&TriggerEvent::Delete));
        assert!(DmlKind::Truncate.matches_event(&TriggerEvent::Truncate));
    }

    #[test]
    fn should_fire_basic() {
        let trig = make_trigger(
            "t1",
            "f",
            TriggerTiming::Before,
            TriggerLevel::Row,
            vec![TriggerEvent::Insert],
        );
        assert!(should_fire(&trig, DmlKind::Insert, None));
        assert!(!should_fire(&trig, DmlKind::Update, None));
        assert!(!should_fire(&trig, DmlKind::Delete, None));
    }

    #[test]
    fn should_fire_disabled() {
        let mut trig = make_trigger(
            "t1",
            "f",
            TriggerTiming::Before,
            TriggerLevel::Row,
            vec![TriggerEvent::Insert],
        );
        trig.enabled = false;
        assert!(!should_fire(&trig, DmlKind::Insert, None));
    }

    #[test]
    fn should_fire_multi_event() {
        let trig = make_trigger(
            "t1",
            "f",
            TriggerTiming::Before,
            TriggerLevel::Row,
            vec![TriggerEvent::Insert, TriggerEvent::Update(vec![])],
        );
        assert!(should_fire(&trig, DmlKind::Insert, None));
        assert!(should_fire(&trig, DmlKind::Update, None));
        assert!(!should_fire(&trig, DmlKind::Delete, None));
    }

    #[test]
    fn should_fire_update_with_columns() {
        let trig = make_trigger(
            "t1",
            "f",
            TriggerTiming::Before,
            TriggerLevel::Row,
            vec![TriggerEvent::Update(vec!["name".to_string()])],
        );
        // 实际更新了 name 列 → 触发
        assert!(should_fire(
            &trig,
            DmlKind::Update,
            Some(&["name".to_string()])
        ));
        // 实际更新了 id 列 → 不触发（name 不在更新列中）
        assert!(!should_fire(
            &trig,
            DmlKind::Update,
            Some(&["id".to_string()])
        ));
        // 未提供更新列信息 → 保守触发（PG 语义要求交集，但 Phase 6.4 简化）
        assert!(should_fire(&trig, DmlKind::Update, None));
    }

    #[test]
    fn fire_row_triggers_before_continue() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut reg = TriggerRegistry::new();
        reg.register(
            "f",
            Arc::new(CountingTrigger {
                counter: counter.clone(),
            }),
        );
        let schema = make_schema();
        let trig = make_trigger(
            "t1",
            "f",
            TriggerTiming::Before,
            TriggerLevel::Row,
            vec![TriggerEvent::Insert],
        );
        let new_row = vec![Value::Int64(1), Value::Text("a".to_string())];
        let result = fire_row_triggers(
            &reg,
            &[trig],
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        )
        .unwrap();
        match result {
            FireResult::ContinueWith(None) => assert_eq!(counter.load(Ordering::SeqCst), 1),
            _ => panic!("expected ContinueWith(None)"),
        }
    }

    #[test]
    fn fire_row_triggers_before_modify() {
        let mut reg = TriggerRegistry::new();
        reg.register("f", Arc::new(ModifyNameTrigger));
        let schema = make_schema();
        let trig = make_trigger(
            "t1",
            "f",
            TriggerTiming::Before,
            TriggerLevel::Row,
            vec![TriggerEvent::Insert],
        );
        let new_row = vec![Value::Int64(1), Value::Text("original".to_string())];
        let result = fire_row_triggers(
            &reg,
            &[trig],
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        )
        .unwrap();
        match result {
            FireResult::ContinueWith(Some(modified)) => {
                assert_eq!(modified[1], Value::Text("modified".to_string()));
            }
            _ => panic!("expected ContinueWith(Some)"),
        }
    }

    #[test]
    fn fire_row_triggers_before_skip() {
        let mut reg = TriggerRegistry::new();
        reg.register("f", Arc::new(SkipRowTrigger));
        let schema = make_schema();
        let trig = make_trigger(
            "t1",
            "f",
            TriggerTiming::Before,
            TriggerLevel::Row,
            vec![TriggerEvent::Insert],
        );
        let new_row = vec![Value::Int64(1), Value::Text("a".to_string())];
        let result = fire_row_triggers(
            &reg,
            &[trig],
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        )
        .unwrap();
        assert!(matches!(result, FireResult::SkipRow));
    }

    #[test]
    fn fire_row_triggers_before_error() {
        let mut reg = TriggerRegistry::new();
        reg.register("f", Arc::new(ErrorTrigger));
        let schema = make_schema();
        let trig = make_trigger(
            "t1",
            "f",
            TriggerTiming::Before,
            TriggerLevel::Row,
            vec![TriggerEvent::Insert],
        );
        let new_row = vec![Value::Int64(1), Value::Text("a".to_string())];
        let result = fire_row_triggers(
            &reg,
            &[trig],
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn fire_row_triggers_after_ignores_outcome() {
        let mut reg = TriggerRegistry::new();
        // AFTER 触发器即使返回 SkipRow 也应被忽略
        reg.register("f", Arc::new(SkipRowTrigger));
        let schema = make_schema();
        let trig = make_trigger(
            "t1",
            "f",
            TriggerTiming::After,
            TriggerLevel::Row,
            vec![TriggerEvent::Insert],
        );
        let new_row = vec![Value::Int64(1), Value::Text("a".to_string())];
        let result = fire_row_triggers(
            &reg,
            &[trig],
            DmlKind::Insert,
            TriggerTiming::After,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        )
        .unwrap();
        // AFTER 触发器返回值被忽略
        match result {
            FireResult::ContinueWith(None) => {}
            _ => panic!("expected ContinueWith(None) for AFTER trigger"),
        }
    }

    #[test]
    fn fire_row_triggers_missing_function() {
        let reg = TriggerRegistry::new();
        let schema = make_schema();
        let trig = make_trigger(
            "t1",
            "nonexistent",
            TriggerTiming::Before,
            TriggerLevel::Row,
            vec![TriggerEvent::Insert],
        );
        let new_row = vec![Value::Int64(1), Value::Text("a".to_string())];
        let result = fire_row_triggers(
            &reg,
            &[trig],
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn fire_row_triggers_filters_by_timing_and_level() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut reg = TriggerRegistry::new();
        reg.register(
            "f",
            Arc::new(CountingTrigger {
                counter: counter.clone(),
            }),
        );
        let schema = make_schema();
        // 仅 AFTER Row 触发器
        let triggers = vec![
            make_trigger(
                "after_row",
                "f",
                TriggerTiming::After,
                TriggerLevel::Row,
                vec![TriggerEvent::Insert],
            ),
            make_trigger(
                "before_stmt",
                "f",
                TriggerTiming::Before,
                TriggerLevel::Statement,
                vec![TriggerEvent::Insert],
            ),
        ];
        let new_row = vec![Value::Int64(1), Value::Text("a".to_string())];
        // 触发 BEFORE Row：应只匹配 before_stmt？不对，level=Statement 不匹配 Row
        // 预期：0 次调用（after_row 时机不匹配，before_stmt 级别不匹配）
        let _ = fire_row_triggers(
            &reg,
            &triggers,
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        )
        .unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        // 触发 AFTER Row：应只匹配 after_row
        let _ = fire_row_triggers(
            &reg,
            &triggers,
            DmlKind::Insert,
            TriggerTiming::After,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        )
        .unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fire_row_triggers_chained_modify() {
        // 第一个触发器修改 name="modified"
        // 第二个触发器应看到修改后的 NEW 行
        struct VerifyModifiedTrigger;
        impl TriggerFunction for VerifyModifiedTrigger {
            fn call(&self, ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
                if let Some(row) = ctx.new_row {
                    assert_eq!(row[1], Value::Text("modified".to_string()));
                }
                Ok(TriggerOutcome::Continue)
            }
        }
        let mut reg = TriggerRegistry::new();
        reg.register("modify", Arc::new(ModifyNameTrigger));
        reg.register("verify", Arc::new(VerifyModifiedTrigger));
        let schema = make_schema();
        let triggers = vec![
            make_trigger(
                "t1",
                "modify",
                TriggerTiming::Before,
                TriggerLevel::Row,
                vec![TriggerEvent::Insert],
            ),
            make_trigger(
                "t2",
                "verify",
                TriggerTiming::Before,
                TriggerLevel::Row,
                vec![TriggerEvent::Insert],
            ),
        ];
        let new_row = vec![Value::Int64(1), Value::Text("original".to_string())];
        let result = fire_row_triggers(
            &reg,
            &triggers,
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        )
        .unwrap();
        match result {
            FireResult::ContinueWith(Some(modified)) => {
                assert_eq!(modified[1], Value::Text("modified".to_string()));
            }
            _ => panic!("expected ContinueWith(Some)"),
        }
    }

    #[test]
    fn fire_statement_triggers_basic() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut reg = TriggerRegistry::new();
        reg.register(
            "f",
            Arc::new(CountingTrigger {
                counter: counter.clone(),
            }),
        );
        let schema = make_schema();
        let triggers = vec![
            make_trigger(
                "before_stmt",
                "f",
                TriggerTiming::Before,
                TriggerLevel::Statement,
                vec![TriggerEvent::Insert],
            ),
            make_trigger(
                "after_stmt",
                "f",
                TriggerTiming::After,
                TriggerLevel::Statement,
                vec![TriggerEvent::Insert],
            ),
            make_trigger(
                "row_only",
                "f",
                TriggerTiming::Before,
                TriggerLevel::Row,
                vec![TriggerEvent::Insert],
            ),
        ];
        // BEFORE STATEMENT：只触发 before_stmt
        fire_statement_triggers(
            &reg,
            &triggers,
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
        )
        .unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // AFTER STATEMENT：只触发 after_stmt
        fire_statement_triggers(
            &reg,
            &triggers,
            DmlKind::Insert,
            TriggerTiming::After,
            "t",
            &schema,
        )
        .unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn fire_statement_triggers_missing_function() {
        let reg = TriggerRegistry::new();
        let schema = make_schema();
        let triggers = vec![make_trigger(
            "t1",
            "nonexistent",
            TriggerTiming::Before,
            TriggerLevel::Statement,
            vec![TriggerEvent::Insert],
        )];
        let result = fire_statement_triggers(
            &reg,
            &triggers,
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
        );
        assert!(result.is_err());
    }

    #[test]
    fn fire_row_triggers_skip_aborts_subsequent() {
        // SkipRow 应中止后续 BEFORE 触发器
        let counter = Arc::new(AtomicUsize::new(0));
        let mut reg = TriggerRegistry::new();
        reg.register("skip", Arc::new(SkipRowTrigger));
        reg.register(
            "count",
            Arc::new(CountingTrigger {
                counter: counter.clone(),
            }),
        );
        let schema = make_schema();
        let triggers = vec![
            make_trigger(
                "t1",
                "skip",
                TriggerTiming::Before,
                TriggerLevel::Row,
                vec![TriggerEvent::Insert],
            ),
            make_trigger(
                "t2",
                "count",
                TriggerTiming::Before,
                TriggerLevel::Row,
                vec![TriggerEvent::Insert],
            ),
        ];
        let new_row = vec![Value::Int64(1), Value::Text("a".to_string())];
        let result = fire_row_triggers(
            &reg,
            &triggers,
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        )
        .unwrap();
        assert!(matches!(result, FireResult::SkipRow));
        // 第二个触发器不应被调用
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn fire_row_triggers_modify_row_length_validation() {
        // Modify 返回的行长度与 schema 不匹配应报错
        struct BadModifyTrigger;
        impl TriggerFunction for BadModifyTrigger {
            fn call(&self, _ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
                // 只返回 1 列，schema 要求 2 列
                Ok(TriggerOutcome::Modify(vec![Value::Int64(1)]))
            }
        }
        let mut reg = TriggerRegistry::new();
        reg.register("f", Arc::new(BadModifyTrigger));
        let schema = make_schema();
        let trig = make_trigger(
            "t1",
            "f",
            TriggerTiming::Before,
            TriggerLevel::Row,
            vec![TriggerEvent::Insert],
        );
        let new_row = vec![Value::Int64(1), Value::Text("a".to_string())];
        let result = fire_row_triggers(
            &reg,
            &[trig],
            DmlKind::Insert,
            TriggerTiming::Before,
            "t",
            &schema,
            Some(&new_row),
            None,
            None,
        );
        assert!(result.is_err());
    }
}
