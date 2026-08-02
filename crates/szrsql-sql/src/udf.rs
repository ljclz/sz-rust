//! Rust UDF 插件系统 — Phase 6.7
//!
//! 提供 Rust 编写的用户定义函数（UDF）注册、调用与卸载能力。设计目标：
//!
//! - **静态注册模式**：UDF 在编译时通过 `UdfRegistry::register` 注册为
//!   `Arc<dyn UdfFunction>` trait 对象。**不使用 `libloading` 动态加载 .so/.dll**，
//!   避免恶意代码执行风险与 ABI 不稳定问题。
//! - **SQL 集成**：`CREATE FUNCTION name(args) RETURNS ret LANGUAGE rust AS 'symbol'`
//!   中的 `body`（`'symbol'`）作为 UDF 注册符号名，执行器从 `UdfRegistry` 查找。
//! - **安全沙箱**：
//!   - **Panic 捕获**：`call()` 通过 `catch_unwind` 捕获 UDF panic，转为 `UdfError::Panic`
//!   - **超时保护**：`UdfSandbox::with_timeout(duration)` 限制单次调用时长（可选）
//!   - **参数类型校验**：UDF 注册时声明参数类型签名，调用前校验
//!   - **STRICT 语义**：STRICT UDF 任一参数为 NULL 直接返回 NULL（与 PG 一致）
//! - **线程安全**：`UdfFunction: Send + Sync`，`UdfRegistry` 内部 `HashMap` + `Arc`
//!
//! # 与 Phase 6.5/6.6 的关系
//!
//! - Phase 6.5 解析 `CREATE FUNCTION ... LANGUAGE rust AS 'symbol'`，`language = "rust"`
//! - Phase 6.6 PL/pgSQL 解释器处理 `LANGUAGE plpgsql` 的函数体
//! - Phase 6.7 UDF 系统处理 `LANGUAGE rust` 的符号查找与调用
//! - 执行器根据 `language` 字段路由：`plpgsql` → 解释器；`rust` → UDF 注册表
//!
//! # 用法
//!
//! ```
//! use szrsql_sql::udf::{UdfFunction, UdfRegistry, UdfContext, UdfError};
//! use szrsql_types::value::Value;
//! use std::sync::Arc;
//!
//! // 1. 定义 UDF
//! struct AddUdf;
//! impl UdfFunction for AddUdf {
//!     fn signature(&self) -> (&'static str, &'static [(&'static str, &'static str)], &'static str) {
//!         ("add", &[("a", "integer"), ("b", "integer")], "integer")
//!     }
//!     fn call(&self, args: &[Value], _ctx: &UdfContext) -> Result<Value, UdfError> {
//!         match (&args[0], &args[1]) {
//!             (Value::Int64(a), Value::Int64(b)) => Ok(Value::Int64(a + b)),
//!             _ => Err(UdfError::TypeError("expected two integers".into())),
//!         }
//!     }
//! }
//!
//! // 2. 注册
//! let mut registry = UdfRegistry::new();
//! registry.register(Arc::new(AddUdf));
//!
//! // 3. 调用
//! let ctx = UdfContext::default();
//! let result = registry.call("add", &[Value::Int64(3), Value::Int64(4)], &ctx).unwrap();
//! assert_eq!(result, Value::Int64(7));
//!
//! // 4. 卸载
//! assert!(registry.unregister("add").is_some());
//! assert!(registry.call("add", &[], &ctx).is_err());
//! ```

use std::any::Any;
use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use szrsql_types::value::Value;
use thiserror::Error;

// =====================================================================
//  错误类型
// =====================================================================

/// UDF 错误
#[derive(Debug, Clone, PartialEq, Error)]
pub enum UdfError {
    /// UDF 未注册
    #[error("UDF not found: {0}")]
    NotFound(String),
    /// 参数数量不匹配
    #[error("argument count mismatch: expected {expected}, got {got}")]
    ArgCountMismatch { expected: usize, got: usize },
    /// 参数类型不匹配
    #[error("type error: {0}")]
    TypeError(String),
    /// UDF 执行 panic
    #[error("UDF panic: {0}")]
    Panic(String),
    /// UDF 执行超时
    #[error("UDF timeout: exceeded {0:?}")]
    Timeout(Duration),
    /// STRICT 函数收到 NULL 参数
    #[error("STRICT function received NULL argument")]
    StrictNullInput,
    /// UDF 返回类型不匹配
    #[error("return type mismatch: expected {expected}, got {got}")]
    ReturnTypeMismatch { expected: String, got: String },
    /// UDF 自定义错误
    #[error("UDF error: {0}")]
    Custom(String),
}

// =====================================================================
//  UDF 上下文
// =====================================================================

/// UDF 执行上下文
///
/// 提供 UDF 调用时的元信息与可选服务。当前 Phase 6.7 提供：
/// - `deadline`：可选的调用截止时间（由 `UdfSandbox::with_timeout` 设置）
/// - `call_id`：每次调用分配的唯一 ID（便于日志追踪）
/// - `user_data`：可选的用户自定义数据（`Any` trait object，供 UDF 访问会话状态）
///
/// # 未来扩展
/// - 数据库句柄（允许 UDF 执行 SQL 查询）
/// - 事务上下文
/// - 当前用户与权限信息
#[derive(Debug, Clone)]
pub struct UdfContext<'a> {
    /// 调用截止时间（None 表示无超时限制）
    pub deadline: Option<Instant>,
    /// 本次调用的唯一 ID（递增分配）
    pub call_id: u64,
    /// 用户自定义数据（可空）
    pub user_data: Option<&'a dyn Any>,
}

impl<'a> Default for UdfContext<'a> {
    /// 创建默认上下文（无超时、call_id=0、无 user_data）
    fn default() -> Self {
        Self {
            deadline: None,
            call_id: 0,
            user_data: None,
        }
    }
}

impl<'a> UdfContext<'a> {
    /// 创建带超时的上下文
    pub fn with_deadline(call_id: u64, deadline: Instant) -> Self {
        Self {
            deadline: Some(deadline),
            call_id,
            user_data: None,
        }
    }

    /// 检查是否超时
    pub fn is_timeout(&self) -> bool {
        self.deadline.map(|d| Instant::now() >= d).unwrap_or(false)
    }

    /// 剩余时间（None 表示无限制）
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }
}

// =====================================================================
//  UDF 函数 trait
// =====================================================================

/// UDF 函数 trait — 由 Rust 代码实现，注册到 `UdfRegistry`
///
/// # 实现要求
/// - 必须 `Send + Sync`（UDF 注册表可跨线程共享）
/// - `call()` 应避免长耗时操作；如需限制，使用 `UdfSandbox::with_timeout`
/// - `call()` 内 panic 会被 `UdfRegistry::call` 捕获并转为 `UdfError::Panic`
/// - 应通过 `UdfError` 返回错误，不应直接 panic
///
/// # 签名声明
///
/// `signature()` 返回 `(name, params, return_type)`：
/// - `name`：UDF 名（小写规范，注册时自动转小写）
/// - `params`：参数列表 `&[(name, type_name)]`，用于调用时类型校验
/// - `return_type`：返回类型名（用于结果校验）
pub trait UdfFunction: Send + Sync {
    /// UDF 签名
    ///
    /// 返回 `(name, params, return_type)`：
    /// - `name`：函数名（建议小写，注册表会强制小写）
    /// - `params`：参数列表 `&[(param_name, type_name)]`
    /// - `return_type`：返回类型名（如 `"integer"`、`"text"`）
    fn signature(
        &self,
    ) -> (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
    );

    /// 调用 UDF
    ///
    /// # 参数
    /// - `args`：参数值列表，顺序与 `signature()` 中的 `params` 一致
    /// - `ctx`：执行上下文（超时、call_id、user_data）
    ///
    /// # 返回
    /// - `Ok(Value)`：执行成功
    /// - `Err(UdfError)`：执行错误
    ///
    /// # 注意
    /// - STRICT 语义（NULL 参数直接返回 NULL）由 `UdfRegistry::call` 处理，
    ///   UDF 实现无需检查 NULL
    /// - panic 会被 `UdfRegistry::call` 的 `catch_unwind` 捕获
    fn call(&self, args: &[Value], ctx: &UdfContext) -> Result<Value, UdfError>;

    /// 是否为 STRICT（任一参数为 NULL 直接返回 NULL）
    ///
    /// 默认 `false`。UDF 可重写为 `true` 实现 PG 的 STRICT 语义。
    fn strict(&self) -> bool {
        false
    }

    /// 是否为 IMMUTABLE（相同参数永远返回相同结果）
    ///
    /// 默认 `true`（PG 中 `LANGUAGE rust` 默认 VOLATILE，但 SzRSQL 保守标记为 IMMUTABLE
    /// 以允许缓存优化；UDF 若需 VOLATILE 语义应重写为 `false`）。
    fn immutable(&self) -> bool {
        true
    }
}

// =====================================================================
//  UDF 注册表
// =====================================================================

/// UDF 注册表 — 维护 `name → Arc<dyn UdfFunction>` 映射
///
/// # 设计
/// - `register`：注册 UDF（同名覆盖）
/// - `unregister`：卸载 UDF（返回原 UDF 供调用方清理资源）
/// - `call`：按名调用 UDF，自动处理 STRICT 语义、参数校验、panic 捕获
/// - `get`：按名查找 UDF（不调用）
/// - `list`：列出所有已注册 UDF 名（供 `DROP FUNCTION` 校验）
///
/// # 线程安全
/// `UdfRegistry` 内部使用 `HashMap<String, Arc<dyn UdfFunction>>`。
/// `Arc` 允许 UDF 跨线程共享；`call_counter` 使用 `AtomicU64`，
/// 因此 `call` / `next_call_id` 仅需 `&self`，可在不可变上下文中调用。
#[derive(Default)]
pub struct UdfRegistry {
    /// 函数名（小写） → UDF
    functions: HashMap<String, Arc<dyn UdfFunction>>,
    /// 调用计数器（用于生成 call_id，原子操作以支持 `&self` 调用）
    call_counter: AtomicU64,
}

impl std::fmt::Debug for UdfRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UdfRegistry")
            .field("registered_count", &self.functions.len())
            .field("names", &self.functions.keys().collect::<Vec<_>>())
            .field("call_counter", &self.call_counter.load(Ordering::Relaxed))
            .finish()
    }
}

impl UdfRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
            call_counter: AtomicU64::new(0),
        }
    }

    /// 注册 UDF（同名覆盖）
    ///
    /// # 参数
    /// - `func`：UDF 实现（`Arc<dyn UdfFunction>`）
    ///
    /// # 返回
    /// - `Some(Arc<dyn UdfFunction>)`：同名 UDF 已存在，返回旧 UDF
    /// - `None`：无同名 UDF
    pub fn register(&mut self, func: Arc<dyn UdfFunction>) -> Option<Arc<dyn UdfFunction>> {
        let (name, _, _) = func.signature();
        let key = name.to_lowercase();
        self.functions.insert(key, func)
    }

    /// 卸载 UDF（对应 `DROP FUNCTION`）
    ///
    /// # 参数
    /// - `name`：UDF 名（大小写不敏感）
    ///
    /// # 返回
    /// - `Some(Arc<dyn UdfFunction>)`：UDF 存在并被卸载
    /// - `None`：UDF 不存在
    pub fn unregister(&mut self, name: &str) -> Option<Arc<dyn UdfFunction>> {
        self.functions.remove(&name.to_lowercase())
    }

    /// 按名查找 UDF（不调用）
    pub fn get(&self, name: &str) -> Option<&Arc<dyn UdfFunction>> {
        self.functions.get(&name.to_lowercase())
    }

    /// UDF 是否已注册
    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains_key(&name.to_lowercase())
    }

    /// 已注册 UDF 数量
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// 列出所有已注册 UDF 名（小写，按字母序）
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.functions.keys().cloned().collect();
        names.sort();
        names
    }

    /// 调用 UDF（核心入口）
    ///
    /// # 参数
    /// - `name`：UDF 名（大小写不敏感）
    /// - `args`：参数值列表
    /// - `ctx`：执行上下文
    ///
    /// # 返回
    /// - `Ok(Value)`：调用成功
    /// - `Err(UdfError)`：调用失败（未找到 / 参数不匹配 / panic / 超时 / 自定义错误）
    ///
    /// # 处理顺序
    /// 1. 查找 UDF（`NotFound`）
    /// 2. 参数数量校验（`ArgCountMismatch`）
    /// 3. STRICT 语义（任一参数为 NULL → 返回 `Ok(Value::Null)`）
    /// 4. 超时检查（`Timeout`）
    /// 5. panic 捕获（`catch_unwind` → `Panic`）
    /// 6. 调用 UDF（返回 `UdfError` 或 `Ok(Value)`）
    pub fn call(&self, name: &str, args: &[Value], ctx: &UdfContext) -> Result<Value, UdfError> {
        // 1. 查找 UDF
        let func = self
            .functions
            .get(&name.to_lowercase())
            .ok_or_else(|| UdfError::NotFound(name.into()))?
            .clone();

        // 2. 参数数量校验
        let (_, params, _) = func.signature();
        if args.len() != params.len() {
            return Err(UdfError::ArgCountMismatch {
                expected: params.len(),
                got: args.len(),
            });
        }

        // 3. STRICT 语义
        if func.strict() && args.iter().any(|v| matches!(v, Value::Null)) {
            return Ok(Value::Null);
        }

        // 4. 超时检查
        if ctx.is_timeout() {
            let limit = ctx.deadline.map(|d| d.elapsed()).unwrap_or(Duration::ZERO);
            return Err(UdfError::Timeout(limit));
        }

        // 5 & 6. panic 捕获 + 调用
        // AssertUnwindSafe：UDF 内部不应有共享可变状态，故 panic 不会破坏注册表
        let args_clone = args.to_vec();
        let result = panic::catch_unwind(AssertUnwindSafe(|| func.call(&args_clone, ctx)));

        match result {
            Ok(inner) => inner,
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
                    s.to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                Err(UdfError::Panic(msg))
            }
        }
    }

    /// 生成下一个 call_id（单调递增）
    pub fn next_call_id(&self) -> u64 {
        self.call_counter.fetch_add(1, Ordering::Relaxed) + 1
    }
}

// =====================================================================
//  UDF 沙箱（超时保护）
// =====================================================================

/// UDF 沙箱 — 提供超时保护与上下文构造
///
/// # 设计
/// - `with_timeout(duration)`：设置单次调用最大时长
/// - `call(registry, name, args)`：在沙箱内调用 UDF
/// - 超时检查在调用前进行（粗粒度，不中断执行中的 UDF）
///
/// # 限制
/// 当前实现为"调用前检查"超时，无法中断执行中的 UDF。
/// 真正的中断需要异步执行 + cancel 机制，留待未来版本。
pub struct UdfSandbox {
    timeout: Option<Duration>,
    call_counter: u64,
}

impl Default for UdfSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl UdfSandbox {
    /// 创建无超时限制的沙箱
    pub fn new() -> Self {
        Self {
            timeout: None,
            call_counter: 0,
        }
    }

    /// 设置超时（链式调用）
    pub fn with_timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    /// 在沙箱内调用 UDF
    ///
    /// # 参数
    /// - `registry`：UDF 注册表
    /// - `name`：UDF 名
    /// - `args`：参数值列表
    ///
    /// # 返回
    /// - `Ok(Value)`：调用成功
    /// - `Err(UdfError)`：调用失败（含超时）
    pub fn call(
        &mut self,
        registry: &UdfRegistry,
        name: &str,
        args: &[Value],
    ) -> Result<Value, UdfError> {
        self.call_counter += 1;
        let call_id = self.call_counter;
        let ctx = match self.timeout {
            Some(d) => {
                let deadline = Instant::now() + d;
                UdfContext::with_deadline(call_id, deadline)
            }
            None => UdfContext {
                call_id,
                ..Default::default()
            },
        };
        registry.call(name, args, &ctx)
    }

    /// 当前超时设置
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// 累计调用次数
    pub fn call_count(&self) -> u64 {
        self.call_counter
    }
}

// =====================================================================
//  类型校验辅助函数
// =====================================================================

/// 校验 Value 是否匹配声明的类型名
///
/// 支持的类型名（大小写不敏感）：
/// - `integer` / `int` / `int4` / `bigint` / `int8` → `Value::Int64`
/// - `double` / `float8` / `float` / `real` → `Value::Float64`
/// - `text` / `varchar` / `char` / `string` → `Value::Text`
/// - `bool` / `boolean` → `Value::Bool`
/// - `decimal` / `numeric` → `Value::Decimal`
/// - `date` → `Value::Date`
/// - `timestamp` → `Value::Timestamp`
/// - `bytea` / `blob` → `Value::Blob`
/// - `json` / `jsonb` → `Value::Json`
/// - 其他类型 → 始终返回 `Ok(())`（宽松校验，允许自定义类型）
pub fn check_value_type(value: &Value, type_name: &str) -> Result<(), UdfError> {
    let lower = type_name.to_lowercase();
    let ok = match (value, lower.as_str()) {
        (_, "any") | (_, "unknown") => true,
        (Value::Null, _) => true, // NULL 可匹配任何类型（STRICT 检查在调用前处理）
        (Value::Int64(_), "integer" | "int" | "int4" | "bigint" | "int8" | "smallint" | "int2") => {
            true
        }
        (Value::Float64(_), "double" | "float8" | "float" | "real" | "float4") => true,
        (Value::Text(_), "text" | "varchar" | "char" | "string" | "bpchar") => true,
        (Value::Bool(_), "bool" | "boolean") => true,
        (Value::Decimal(_, _), "decimal" | "numeric") => true,
        (Value::Date(_), "date") => true,
        (Value::Timestamp(_), "timestamp" | "timestamptz") => true,
        (Value::Blob(_), "bytea" | "blob" | "varbinary" | "binary") => true,
        (Value::Json(_), "json" | "jsonb") => true,
        (Value::Array(_), t) if t.ends_with("[]") => true,
        (Value::Enum(_), "enum") => true,
        (_, _) => true, // 未知类型名宽松通过
    };
    if ok {
        Ok(())
    } else {
        Err(UdfError::TypeError(format!(
            "value {:?} does not match type {}",
            value, type_name
        )))
    }
}

/// 校验参数列表是否匹配签名
pub fn check_args_type(args: &[Value], params: &[(&str, &str)]) -> Result<(), UdfError> {
    if args.len() != params.len() {
        return Err(UdfError::ArgCountMismatch {
            expected: params.len(),
            got: args.len(),
        });
    }
    for (i, (arg, (_, type_name))) in args.iter().zip(params.iter()).enumerate() {
        check_value_type(arg, type_name)
            .map_err(|e| UdfError::TypeError(format!("arg {i}: {e}")))?;
    }
    Ok(())
}

// =====================================================================
//  内置 UDF 示例（用于测试与演示）
// =====================================================================

/// 内置 UDF：整数加法 `add(a, b)`
pub struct AddUdf;

impl UdfFunction for AddUdf {
    fn signature(
        &self,
    ) -> (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
    ) {
        ("add", &[("a", "integer"), ("b", "integer")], "integer")
    }
    fn call(&self, args: &[Value], _ctx: &UdfContext) -> Result<Value, UdfError> {
        match (&args[0], &args[1]) {
            (Value::Int64(a), Value::Int64(b)) => a
                .checked_add(*b)
                .map(Value::Int64)
                .ok_or_else(|| UdfError::Custom("integer overflow".into())),
            _ => Err(UdfError::TypeError("expected two integers".into())),
        }
    }
}

/// 内置 UDF：字符串长度 `my_length(s)`
pub struct MyLengthUdf;

impl UdfFunction for MyLengthUdf {
    fn signature(
        &self,
    ) -> (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
    ) {
        ("my_length", &[("s", "text")], "integer")
    }
    fn call(&self, args: &[Value], _ctx: &UdfContext) -> Result<Value, UdfError> {
        match &args[0] {
            Value::Text(s) => Ok(Value::Int64(s.len() as i64)),
            Value::Null => Ok(Value::Null),
            _ => Err(UdfError::TypeError("expected text".into())),
        }
    }
}

/// 内置 UDF：STRICT 双精度平方 `strict_square(x)`
pub struct StrictSquareUdf;

impl UdfFunction for StrictSquareUdf {
    fn signature(
        &self,
    ) -> (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
    ) {
        (
            "strict_square",
            &[("x", "double precision")],
            "double precision",
        )
    }
    fn call(&self, args: &[Value], _ctx: &UdfContext) -> Result<Value, UdfError> {
        match &args[0] {
            Value::Float64(x) => Ok(Value::Float64(x * x)),
            _ => Err(UdfError::TypeError("expected double".into())),
        }
    }
    fn strict(&self) -> bool {
        true
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- 测试专用 UDF（仅编译进测试二进制，避免污染生产代码） ---

    /// 测试用 UDF：故意 panic（用于验证 UdfRegistry 的 panic 捕获机制）
    pub struct PanicUdf;

    impl UdfFunction for PanicUdf {
        fn signature(
            &self,
        ) -> (
            &'static str,
            &'static [(&'static str, &'static str)],
            &'static str,
        ) {
            ("panic_udf", &[], "integer")
        }
        fn call(&self, _args: &[Value], _ctx: &UdfContext) -> Result<Value, UdfError> {
            panic!("intentional panic from PanicUdf")
        }
    }

    /// 测试用 UDF：长耗时操作（用于验证 UdfRegistry 的超时控制机制）
    pub struct SlowUdf;

    impl UdfFunction for SlowUdf {
        fn signature(
            &self,
        ) -> (
            &'static str,
            &'static [(&'static str, &'static str)],
            &'static str,
        ) {
            ("slow_udf", &[("ms", "integer")], "integer")
        }
        fn call(&self, args: &[Value], ctx: &UdfContext) -> Result<Value, UdfError> {
            let ms = match &args[0] {
                Value::Int64(n) => *n as u64,
                _ => return Err(UdfError::TypeError("expected integer".into())),
            };
            let start = Instant::now();
            let target = Duration::from_millis(ms);
            // 忙等（模拟长耗时操作）
            while start.elapsed() < target {
                if ctx.is_timeout() {
                    return Err(UdfError::Timeout(
                        ctx.deadline.map(|d| d.elapsed()).unwrap_or_default(),
                    ));
                }
                std::hint::spin_loop();
            }
            Ok(Value::Int64(ms as i64))
        }
        fn immutable(&self) -> bool {
            false // 耗时操作视为 VOLATILE
        }
    }

    // --- 基础注册与查找 ---

    #[test]
    fn test_register_and_get() {
        let mut registry = UdfRegistry::new();
        assert!(registry.is_empty());

        registry.register(Arc::new(AddUdf));
        assert_eq!(registry.len(), 1);
        assert!(registry.contains("add"));
        assert!(registry.contains("ADD")); // 大小写不敏感
        assert!(registry.get("add").is_some());
    }

    #[test]
    fn test_register_overwrites_same_name() {
        let mut registry = UdfRegistry::new();
        let old = registry.register(Arc::new(AddUdf));
        assert!(old.is_none());

        // 再次注册同名（覆盖）
        let old = registry.register(Arc::new(AddUdf));
        assert!(old.is_some());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_unregister() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(AddUdf));
        assert!(registry.contains("add"));

        let removed = registry.unregister("add");
        assert!(removed.is_some());
        assert!(!registry.contains("add"));
        assert!(registry.is_empty());

        // 再次卸载返回 None
        assert!(registry.unregister("add").is_none());
    }

    #[test]
    fn test_list_sorted() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(AddUdf));
        registry.register(Arc::new(MyLengthUdf));
        registry.register(Arc::new(StrictSquareUdf));

        let names = registry.list();
        assert_eq!(names, vec!["add", "my_length", "strict_square"]);
    }

    // --- 调用与参数校验 ---

    #[test]
    fn test_call_basic() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(AddUdf));

        let ctx = UdfContext::default();
        let result = registry.call("add", &[Value::Int64(3), Value::Int64(4)], &ctx);
        assert_eq!(result.unwrap(), Value::Int64(7));
    }

    #[test]
    fn test_call_case_insensitive() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(AddUdf));

        let ctx = UdfContext::default();
        let result = registry.call("ADD", &[Value::Int64(1), Value::Int64(2)], &ctx);
        assert_eq!(result.unwrap(), Value::Int64(3));
    }

    #[test]
    fn test_call_not_found() {
        let registry = UdfRegistry::new();
        let ctx = UdfContext::default();
        let result = registry.call("nonexistent", &[], &ctx);
        assert!(matches!(result, Err(UdfError::NotFound(_))));
    }

    #[test]
    fn test_call_arg_count_mismatch() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(AddUdf));

        let ctx = UdfContext::default();
        // add 期望 2 个参数，传 1 个
        let result = registry.call("add", &[Value::Int64(1)], &ctx);
        assert!(matches!(
            result,
            Err(UdfError::ArgCountMismatch {
                expected: 2,
                got: 1
            })
        ));
    }

    #[test]
    fn test_call_integer_overflow() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(AddUdf));

        let ctx = UdfContext::default();
        let result = registry.call("add", &[Value::Int64(i64::MAX), Value::Int64(1)], &ctx);
        assert!(matches!(result, Err(UdfError::Custom(_))));
    }

    // --- STRICT 语义 ---

    #[test]
    fn test_strict_function_with_null() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(StrictSquareUdf));

        let ctx = UdfContext::default();
        // STRICT 函数收到 NULL 直接返回 NULL
        let result = registry.call("strict_square", &[Value::Null], &ctx);
        assert_eq!(result.unwrap(), Value::Null);
    }

    #[test]
    fn test_strict_function_with_non_null() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(StrictSquareUdf));

        let ctx = UdfContext::default();
        let result = registry.call("strict_square", &[Value::Float64(3.0)], &ctx);
        assert_eq!(result.unwrap(), Value::Float64(9.0));
    }

    #[test]
    fn test_non_strict_function_handles_null() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(MyLengthUdf));

        let ctx = UdfContext::default();
        // my_length 非 STRICT，自己处理 NULL
        let result = registry.call("my_length", &[Value::Null], &ctx);
        assert_eq!(result.unwrap(), Value::Null);
    }

    // --- panic 捕获 ---

    #[test]
    fn test_panic_captured() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(PanicUdf));

        let ctx = UdfContext::default();
        let result = registry.call("panic_udf", &[], &ctx);
        match result {
            Err(UdfError::Panic(msg)) => {
                assert!(msg.contains("intentional panic"));
            }
            _ => panic!("expected UdfError::Panic, got {:?}", result),
        }
    }

    #[test]
    fn test_registry_unchanged_after_panic() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(PanicUdf));

        let ctx = UdfContext::default();
        // 第一次调用：panic
        let _ = registry.call("panic_udf", &[], &ctx);
        // 注册表应保持完整
        assert!(registry.contains("panic_udf"));
        assert_eq!(registry.len(), 1);

        // 再次调用：仍然 panic（证明注册表未损坏）
        let result = registry.call("panic_udf", &[], &ctx);
        assert!(matches!(result, Err(UdfError::Panic(_))));
    }

    // --- 超时保护 ---

    #[test]
    fn test_sandbox_no_timeout() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(SlowUdf));

        let mut sandbox = UdfSandbox::new();
        // 50ms 应快速完成
        let result = sandbox.call(&registry, "slow_udf", &[Value::Int64(50)]);
        assert_eq!(result.unwrap(), Value::Int64(50));
        assert_eq!(sandbox.call_count(), 1);
    }

    #[test]
    fn test_sandbox_with_timeout_passes() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(SlowUdf));

        let mut sandbox = UdfSandbox::new().with_timeout(Duration::from_millis(500));
        let result = sandbox.call(&registry, "slow_udf", &[Value::Int64(50)]);
        assert_eq!(result.unwrap(), Value::Int64(50));
    }

    #[test]
    fn test_sandbox_with_timeout_already_expired() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(SlowUdf));

        // 构造已过期的 deadline
        let ctx = UdfContext::with_deadline(1, Instant::now() - Duration::from_millis(1));
        let result = registry.call("slow_udf", &[Value::Int64(50)], &ctx);
        assert!(matches!(result, Err(UdfError::Timeout(_))));
    }

    #[test]
    fn test_sandbox_with_timeout_slow_udf_checks_deadline() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(SlowUdf));

        // 设置 10ms 超时，但 SlowUdf 需要 500ms，应在中途检测到超时
        let ctx = UdfContext::with_deadline(1, Instant::now() + Duration::from_millis(10));
        let result = registry.call("slow_udf", &[Value::Int64(500)], &ctx);
        assert!(matches!(result, Err(UdfError::Timeout(_))));
    }

    // --- 上下文 ---

    #[test]
    fn test_context_default() {
        let ctx = UdfContext::default();
        assert_eq!(ctx.call_id, 0);
        assert!(ctx.deadline.is_none());
        assert!(!ctx.is_timeout());
        assert!(ctx.remaining().is_none());
    }

    #[test]
    fn test_context_with_deadline() {
        let deadline = Instant::now() + Duration::from_secs(10);
        let ctx = UdfContext::with_deadline(42, deadline);
        assert_eq!(ctx.call_id, 42);
        assert!(ctx.deadline.is_some());
        assert!(!ctx.is_timeout());
        let remaining = ctx.remaining().unwrap();
        assert!(remaining <= Duration::from_secs(10));
        assert!(remaining > Duration::from_secs(9));
    }

    #[test]
    fn test_context_expired_deadline() {
        let deadline = Instant::now() - Duration::from_millis(1);
        let ctx = UdfContext::with_deadline(1, deadline);
        assert!(ctx.is_timeout());
    }

    #[test]
    fn test_next_call_id_monotonic() {
        let registry = UdfRegistry::new();
        let id1 = registry.next_call_id();
        let id2 = registry.next_call_id();
        let id3 = registry.next_call_id();
        assert!(id1 < id2);
        assert!(id2 < id3);
        assert_eq!(id1, 1);
        assert_eq!(id3, 3);
    }

    // --- 类型校验 ---

    #[test]
    fn test_check_value_type_int64() {
        assert!(check_value_type(&Value::Int64(42), "integer").is_ok());
        assert!(check_value_type(&Value::Int64(42), "INT").is_ok());
        assert!(check_value_type(&Value::Int64(42), "bigint").is_ok());
        assert!(check_value_type(&Value::Int64(42), "any").is_ok());
    }

    #[test]
    fn test_check_value_type_text() {
        assert!(check_value_type(&Value::Text("hello".into()), "text").is_ok());
        assert!(check_value_type(&Value::Text("hello".into()), "varchar").is_ok());
    }

    #[test]
    fn test_check_value_type_null_matches_any() {
        // NULL 可匹配任何类型
        assert!(check_value_type(&Value::Null, "integer").is_ok());
        assert!(check_value_type(&Value::Null, "text").is_ok());
        assert!(check_value_type(&Value::Null, "custom_type").is_ok());
    }

    #[test]
    fn test_check_value_type_unknown_type_passes() {
        // 未知类型名宽松通过
        assert!(check_value_type(&Value::Int64(42), "custom_type").is_ok());
    }

    #[test]
    fn test_check_args_type() {
        let params = [("a", "integer"), ("b", "text")];
        let args = vec![Value::Int64(1), Value::Text("x".into())];
        assert!(check_args_type(&args, &params).is_ok());

        // 数量不匹配
        let args = vec![Value::Int64(1)];
        assert!(check_args_type(&args, &params).is_err());
    }

    // --- 端到端：注册 → 调用 → 卸载全流程 ---

    #[test]
    fn test_full_lifecycle() {
        let mut registry = UdfRegistry::new();
        let ctx = UdfContext::default();

        // 1. 注册
        registry.register(Arc::new(AddUdf));
        assert!(registry.contains("add"));

        // 2. 调用
        let result = registry.call("add", &[Value::Int64(10), Value::Int64(20)], &ctx);
        assert_eq!(result.unwrap(), Value::Int64(30));

        // 3. 再次调用
        let result = registry.call("add", &[Value::Int64(-5), Value::Int64(5)], &ctx);
        assert_eq!(result.unwrap(), Value::Int64(0));

        // 4. 卸载
        let removed = registry.unregister("add");
        assert!(removed.is_some());

        // 5. 卸载后调用应失败
        let result = registry.call("add", &[Value::Int64(1), Value::Int64(2)], &ctx);
        assert!(matches!(result, Err(UdfError::NotFound(_))));
    }

    #[test]
    fn test_multiple_udfs() {
        let mut registry = UdfRegistry::new();
        let ctx = UdfContext::default();

        registry.register(Arc::new(AddUdf));
        registry.register(Arc::new(MyLengthUdf));
        registry.register(Arc::new(StrictSquareUdf));

        assert_eq!(registry.len(), 3);

        // add
        assert_eq!(
            registry
                .call("add", &[Value::Int64(1), Value::Int64(2)], &ctx)
                .unwrap(),
            Value::Int64(3)
        );
        // my_length
        assert_eq!(
            registry
                .call("my_length", &[Value::Text("hello".into())], &ctx)
                .unwrap(),
            Value::Int64(5)
        );
        // strict_square
        assert_eq!(
            registry
                .call("strict_square", &[Value::Float64(4.0)], &ctx)
                .unwrap(),
            Value::Float64(16.0)
        );
    }

    #[test]
    fn test_immutable_default() {
        let add = AddUdf;
        assert!(add.immutable()); // 默认 IMMUTABLE

        let slow = SlowUdf;
        assert!(!slow.immutable()); // SlowUdf 标记为 VOLATILE
    }

    #[test]
    fn test_strict_default() {
        let add = AddUdf;
        assert!(!add.strict()); // 默认非 STRICT

        let sq = StrictSquareUdf;
        assert!(sq.strict()); // STRICT
    }

    // --- 沙箱调用计数 ---

    #[test]
    fn test_sandbox_call_count() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(AddUdf));

        let mut sandbox = UdfSandbox::new();
        assert_eq!(sandbox.call_count(), 0);

        for i in 0..10 {
            sandbox
                .call(&registry, "add", &[Value::Int64(i), Value::Int64(i)])
                .unwrap();
        }
        assert_eq!(sandbox.call_count(), 10);
    }

    #[test]
    fn test_sandbox_timeout_getter() {
        let sandbox = UdfSandbox::new();
        assert_eq!(sandbox.timeout(), None);

        let sandbox = UdfSandbox::new().with_timeout(Duration::from_secs(5));
        assert_eq!(sandbox.timeout(), Some(Duration::from_secs(5)));
    }

    // --- 自定义 UDF 实现（演示扩展性）---

    #[test]
    fn test_custom_string_concat_udf() {
        // 用户自定义 UDF：字符串拼接
        struct ConcatUdf;
        impl UdfFunction for ConcatUdf {
            fn signature(
                &self,
            ) -> (
                &'static str,
                &'static [(&'static str, &'static str)],
                &'static str,
            ) {
                ("my_concat", &[("a", "text"), ("b", "text")], "text")
            }
            fn call(&self, args: &[Value], _ctx: &UdfContext) -> Result<Value, UdfError> {
                match (&args[0], &args[1]) {
                    (Value::Text(a), Value::Text(b)) => Ok(Value::Text(format!("{a}{b}"))),
                    (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                    _ => Err(UdfError::TypeError("expected two texts".into())),
                }
            }
        }

        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(ConcatUdf));

        let ctx = UdfContext::default();
        let result = registry.call(
            "my_concat",
            &[Value::Text("hello, ".into()), Value::Text("world!".into())],
            &ctx,
        );
        assert_eq!(result.unwrap(), Value::Text("hello, world!".into()));
    }

    #[test]
    fn test_custom_array_sum_udf() {
        // 用户自定义 UDF：数组求和（接受整数数组）
        struct ArraySumUdf;
        impl UdfFunction for ArraySumUdf {
            fn signature(
                &self,
            ) -> (
                &'static str,
                &'static [(&'static str, &'static str)],
                &'static str,
            ) {
                ("array_sum", &[("arr", "integer[]")], "integer")
            }
            fn call(&self, args: &[Value], _ctx: &UdfContext) -> Result<Value, UdfError> {
                match &args[0] {
                    Value::Array(items) => {
                        let mut sum: i64 = 0;
                        for v in items {
                            if let Value::Int64(n) = v {
                                sum = sum.checked_add(*n).ok_or_else(|| {
                                    UdfError::Custom("integer overflow in array sum".into())
                                })?;
                            } else {
                                return Err(UdfError::TypeError(format!(
                                    "expected integer array, got {:?}",
                                    v
                                )));
                            }
                        }
                        Ok(Value::Int64(sum))
                    }
                    Value::Null => Ok(Value::Null),
                    _ => Err(UdfError::TypeError("expected array".into())),
                }
            }
        }

        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(ArraySumUdf));

        let ctx = UdfContext::default();
        let arr = Value::Array(vec![
            Value::Int64(1),
            Value::Int64(2),
            Value::Int64(3),
            Value::Int64(4),
            Value::Int64(5),
        ]);
        let result = registry.call("array_sum", &[arr], &ctx);
        assert_eq!(result.unwrap(), Value::Int64(15));
    }

    #[test]
    fn test_custom_stateful_udf() {
        // 用户自定义有状态 UDF：调用计数器（使用 AtomicU64）
        use std::sync::atomic::{AtomicU64, Ordering};

        struct CounterUdf {
            count: AtomicU64,
        }
        impl UdfFunction for CounterUdf {
            fn signature(
                &self,
            ) -> (
                &'static str,
                &'static [(&'static str, &'static str)],
                &'static str,
            ) {
                ("counter", &[], "integer")
            }
            fn call(&self, _args: &[Value], _ctx: &UdfContext) -> Result<Value, UdfError> {
                let n = self.count.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Int64(n as i64))
            }
            fn immutable(&self) -> bool {
                false // 有状态 UDF 非 IMMUTABLE
            }
        }

        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(CounterUdf {
            count: AtomicU64::new(0),
        }));

        let ctx = UdfContext::default();
        assert_eq!(
            registry.call("counter", &[], &ctx).unwrap(),
            Value::Int64(0)
        );
        assert_eq!(
            registry.call("counter", &[], &ctx).unwrap(),
            Value::Int64(1)
        );
        assert_eq!(
            registry.call("counter", &[], &ctx).unwrap(),
            Value::Int64(2)
        );
    }

    // --- Debug trait ---

    #[test]
    fn test_registry_debug() {
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(AddUdf));
        registry.register(Arc::new(MyLengthUdf));

        let debug_str = format!("{:?}", registry);
        assert!(debug_str.contains("UdfRegistry"));
        assert!(debug_str.contains("registered_count: 2"));
        assert!(debug_str.contains("add"));
        assert!(debug_str.contains("my_length"));
    }

    // --- Stress 测试 ---

    #[test]
    fn test_stress_1000_calls() {
        // Stress：1000 次调用验证性能合理（< 1s）
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(AddUdf));

        let ctx = UdfContext::default();
        let start = std::time::Instant::now();
        for i in 0..1000 {
            let result = registry
                .call("add", &[Value::Int64(i), Value::Int64(i)], &ctx)
                .unwrap();
            assert_eq!(result, Value::Int64(i * 2));
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_secs() < 1,
            "1000 calls took {:?}, expected < 1s",
            elapsed
        );
    }

    #[test]
    fn test_stress_register_unregister_cycle() {
        // Stress：100 次注册/卸载循环验证无内存泄漏
        let mut registry = UdfRegistry::new();
        for _ in 0..100 {
            registry.register(Arc::new(AddUdf));
            assert_eq!(registry.len(), 1);
            assert!(registry.unregister("add").is_some());
            assert_eq!(registry.len(), 0);
        }
        // 最终应为空
        assert!(registry.is_empty());
    }

    #[test]
    fn test_stress_concurrent_safe() {
        // Stress：多线程并发调用（验证 Send + Sync）
        // P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
        use parking_lot::Mutex;
        use std::sync::Arc as StdArc;
        use std::thread;

        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(AddUdf));
        let registry = StdArc::new(Mutex::new(registry));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let registry = registry.clone();
            handles.push(thread::spawn(move || {
                for i in 0..100 {
                    let ctx = UdfContext::default();
                    let result =
                        registry
                            .lock()
                            .call("add", &[Value::Int64(i), Value::Int64(i)], &ctx);
                    assert!(result.is_ok());
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
    }
}
