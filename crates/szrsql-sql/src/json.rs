//! SQL/JSON 类型支持 — Phase 6.21
//!
//! 提供 PG 风格的 JSON/JSONB 类型支持：
//!
//! - **操作符**：`->` / `->>` / `#>` / `#>>` / `@>` / `<@` / `?` / `?|` / `?&` / `||` / `-` / `#-`
//! - **路径查询**：`@>`（包含）/ `?`（键存在）等
//! - **JSONB 索引扫描**：基于倒排索引（GIN-like）支持 `@>` 包含查询
//!
//! # 设计
//!
//! - **类型统一**：`Value::Json(serde_json::Value)` 同时承载 JSON 和 JSONB
//!   （PG 中 JSONB 是二进制存储的 JSON；本实现内存存储，无需区分二进制格式）
//! - **操作符**：以纯函数实现，接受 `&Value` 参数返回 `Result<Value, JsonError>`
//! - **JSONB 索引**：`JsonbIndex` 维护"路径/键 → 行 ID 列表"倒排表，
//!   `@>` 查询时分解查询 JSON 的所有路径，对候选集取交集
//!
//! # 与 PG 的关系
//!
//! - PG 9.3+ 支持 JSON 类型，9.4+ 支持 JSONB
//! - PG 的 `->` 返回 JSON/JSONB，`->>` 返回 text
//! - PG 的 `@>` 仅支持 JSONB（JSON 不支持）
//! - PG 的 `?`/`?|`/`?&` 也仅支持 JSONB
//! - PG 的 JSONB 索引：GIN（默认）+ Hash（仅等值）
//! - 本实现统一支持 JSON/JSONB 上的所有操作符（运行时不区分）
//!
//! # 限制
//!
//! - **无 DDL 集成**：未集成到 SQL 解析路径（sqlparser 0.53 不识别 `->`/`@>` 等 JSON 操作符）
//! - **无路径字面量语法**：PG 的 `'{"a":1}'::jsonb @> '{"a":1}'::jsonb` 需调用方先 CAST
//! - **JSONB 索引仅支持 `@>`**：不支持 `?` 键存在查询走索引（PG 的 GIN 支持，本实现简化）
//! - **无 JSONPath**：PG 12+ 的 `jsonb_path_query` 不在本阶段范围
//! - **类型不严格**：PG 区分 JSON/JSONB 操作符可用性；本实现统一支持（运行时不区分）

use crate::executor::{ExecutionError, TableStorage};
use crate::plan::TableSchema;
use std::collections::HashMap;
use szrsql_types::value::Value;

// =====================================================================
//  错误类型
// =====================================================================

/// JSON 操作错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JsonError {
    /// 操作数不是 JSON 类型
    #[error("expected JSON value, got {0}")]
    NotJson(&'static str),
    /// 键/索引类型错误（期望 Text 或 Int64）
    #[error("expected Text or Int64 key, got {0}")]
    InvalidKeyType(&'static str),
    /// 数组索引越界
    #[error("array index out of bounds: {0}")]
    IndexOutOfBounds(i64),
    /// 负索引（PG 允许 -1 表示末元素，本实现支持）
    #[error("negative array index too large: {0}")]
    NegativeIndexTooLarge(i64),
    /// 路径元素类型错误
    #[error("invalid path element: expected string or integer")]
    InvalidPathElement,
    /// JSON 解析错误
    #[error("JSON parse error: {0}")]
    ParseError(String),
}

impl From<JsonError> for ExecutionError {
    fn from(e: JsonError) -> Self {
        ExecutionError::EvalError(format!("JSON error: {e}"))
    }
}

// =====================================================================
//  JSON 操作符枚举
// =====================================================================

/// JSON 操作符（对应 PG 的操作符）
///
/// 用于 `apply_json_operator` 统一分派。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JsonOperator {
    /// `->` — 获取对象字段 / 数组元素（返回 JSON）
    Get,
    /// `->>` — 获取对象字段 / 数组元素（返回 TEXT）
    GetAsText,
    /// `#>` — 按 JSON 路径获取（返回 JSON）
    PathGet,
    /// `#>>` — 按 JSON 路径获取（返回 TEXT）
    PathGetAsText,
    /// `@>` — 包含（左侧包含右侧）→ 返回 BOOL
    Contains,
    /// `<@` — 被包含（左侧被右侧包含）→ 返回 BOOL
    ContainedBy,
    /// `?` — 键存在 → 返回 BOOL
    KeyExists,
    /// `?|` — 任一键存在 → 返回 BOOL
    AnyKeyExists,
    /// `?&` — 所有键存在 → 返回 BOOL
    AllKeysExist,
    /// `||` — 拼接 → 返回 JSON
    Concat,
    /// `-` — 删除键/元素 → 返回 JSON
    Delete,
    /// `#-` — 按路径删除 → 返回 JSON
    PathDelete,
}

impl JsonOperator {
    /// 操作符符号表示
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "->",
            Self::GetAsText => "->>",
            Self::PathGet => "#>",
            Self::PathGetAsText => "#>>",
            Self::Contains => "@>",
            Self::ContainedBy => "<@",
            Self::KeyExists => "?",
            Self::AnyKeyExists => "?|",
            Self::AllKeysExist => "?&",
            Self::Concat => "||",
            Self::Delete => "-",
            Self::PathDelete => "#-",
        }
    }
}

impl std::fmt::Display for JsonOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =====================================================================
//  JSON 路径类型
// =====================================================================

/// JSON 路径元素 — 键名或数组索引
///
/// 用于 `#>` / `#>>` / `#-` 操作符的路径参数。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathElement {
    /// 对象字段名
    Key(String),
    /// 数组索引（可为负数，-1 表示末元素）
    Index(i64),
}

impl PathElement {
    /// 从字符串创建键路径元素
    pub fn key(s: impl Into<String>) -> Self {
        Self::Key(s.into())
    }

    /// 从整数创建索引路径元素
    pub fn index(i: i64) -> Self {
        Self::Index(i)
    }

    /// 从 Value 创建路径元素
    ///
    /// - `Value::Text(s)` → `Key(s)`
    /// - `Value::Int64(n)` → `Index(n)`
    /// - 其他 → `Err`
    pub fn from_value(v: &Value) -> Result<Self, JsonError> {
        match v {
            Value::Text(s) => Ok(Self::Key(s.clone())),
            Value::Int64(n) => Ok(Self::Index(*n)),
            Value::Null => Err(JsonError::InvalidPathElement),
            Value::Float64(_) => Err(JsonError::InvalidPathElement),
            Value::Bool(_) => Err(JsonError::InvalidPathElement),
            _ => Err(JsonError::InvalidPathElement),
        }
    }
}

/// JSON 路径 — 一系列路径元素
pub type JsonPath = Vec<PathElement>;

/// 从 `Value::Array` 构造 JSON 路径
///
/// 数组元素依次转换为 `PathElement`。
pub fn path_from_array(arr: &Value) -> Result<JsonPath, JsonError> {
    match arr {
        Value::Array(items) => items.iter().map(PathElement::from_value).collect(),
        _ => Err(JsonError::InvalidPathElement),
    }
}

// =====================================================================
//  核心操作符函数
// =====================================================================

/// `->` 操作符：获取对象字段或数组元素，返回 JSON
///
/// - 左操作数为 `Value::Json`，右操作数为 `Value::Text`（键名）或 `Value::Int64`（索引）
/// - 对象：返回对应字段的 JSON 值；字段不存在返回 NULL
/// - 数组：返回对应索引的 JSON 值；索引越界返回 NULL；支持负索引（-1 = 末元素）
/// - 标量 JSON：返回 NULL
pub fn json_get(left: &Value, right: &Value) -> Result<Value, JsonError> {
    let json = as_json(left)?;
    match right {
        Value::Text(key) => Ok(get_field(json, key)),
        Value::Int64(idx) => Ok(get_index(json, *idx)?),
        Value::Null => Ok(Value::Null),
        _ => Err(JsonError::InvalidKeyType(value_type_name(right))),
    }
}

/// `->>` 操作符：获取对象字段或数组元素，返回 TEXT
///
/// 与 `json_get` 类似，但结果转为文本：
/// - JSON 字符串：返回去引号的字符串内容
/// - JSON 数字/布尔/null：返回 JSON 序列化文本
/// - JSON 对象/数组：返回 JSON 序列化文本
/// - 字段不存在或索引越界：返回 NULL
pub fn json_get_as_text(left: &Value, right: &Value) -> Result<Value, JsonError> {
    let v = json_get(left, right)?;
    match v {
        Value::Null => Ok(Value::Null),
        Value::Json(j) => Ok(json_to_text(&j)),
        _ => Ok(Value::Null),
    }
}

/// `#>` 操作符：按 JSON 路径获取，返回 JSON
///
/// 路径由 `Value::Array` 表示，元素为 Text（键名）或 Int64（索引）。
/// 任一路径元素不存在则返回 NULL。
pub fn json_path_get(left: &Value, path: &Value) -> Result<Value, JsonError> {
    let path = path_from_array(path)?;
    let mut current = as_json(left)?.clone();
    for elem in path {
        current = match navigate(&current, &elem)? {
            Some(v) => v,
            None => return Ok(Value::Null),
        };
    }
    Ok(Value::Json(current))
}

/// `#>>` 操作符：按 JSON 路径获取，返回 TEXT
pub fn json_path_get_as_text(left: &Value, path: &Value) -> Result<Value, JsonError> {
    let v = json_path_get(left, path)?;
    match v {
        Value::Null => Ok(Value::Null),
        Value::Json(j) => Ok(json_to_text(&j)),
        _ => Ok(Value::Null),
    }
}

/// `@>` 操作符：左侧 JSON 包含右侧 JSON → 返回 BOOL
///
/// PG 语义：
/// - 对象包含：左有右的所有键值对（递归）
/// - 数组包含：左有右的所有元素（递归，顺序无关）
/// - 标量包含：数组包含该标量
pub fn json_contains(left: &Value, right: &Value) -> Result<Value, JsonError> {
    let l = as_json(left)?;
    let r = as_json(right)?;
    Ok(Value::Bool(json_contains_impl(l, r)))
}

/// `<@` 操作符：左侧 JSON 被右侧包含 → 返回 BOOL
///
/// 等价于 `r @> l`。
pub fn json_contained_by(left: &Value, right: &Value) -> Result<Value, JsonError> {
    json_contains(right, left)
}

/// `?` 操作符：JSON 对象是否包含指定键 → 返回 BOOL
///
/// - 对象：检查键是否存在
/// - 数组：检查是否存在元素（字符串匹配）
/// - 标量：返回 false
pub fn json_key_exists(left: &Value, key: &Value) -> Result<Value, JsonError> {
    let json = as_json(left)?;
    let key_str = as_key_string(key)?;
    Ok(Value::Bool(key_exists_impl(json, &key_str)))
}

/// `?|` 操作符：JSON 是否包含任一键 → 返回 BOOL
pub fn json_any_key_exists(left: &Value, keys: &Value) -> Result<Value, JsonError> {
    let json = as_json(left)?;
    let key_strs = as_key_string_array(keys)?;
    Ok(Value::Bool(
        key_strs.iter().any(|k| key_exists_impl(json, k)),
    ))
}

/// `?&` 操作符：JSON 是否包含所有键 → 返回 BOOL
pub fn json_all_keys_exist(left: &Value, keys: &Value) -> Result<Value, JsonError> {
    let json = as_json(left)?;
    let key_strs = as_key_string_array(keys)?;
    Ok(Value::Bool(
        key_strs.iter().all(|k| key_exists_impl(json, k)),
    ))
}

/// `||` 操作符：拼接两个 JSON 值 → 返回 JSON
///
/// PG 语义：
/// - 两个对象：合并（右覆盖左）
/// - 两个数组：连接
/// - 其他：返回右侧值
pub fn json_concat(left: &Value, right: &Value) -> Result<Value, JsonError> {
    let l = as_json(left)?;
    let r = as_json(right)?;
    Ok(Value::Json(json_concat_impl(l, r)))
}

/// `-` 操作符：删除键/元素 → 返回 JSON
///
/// - 对象 + Text：删除指定键
/// - 数组 + Int64：删除指定索引元素
/// - 数组 + Text：删除匹配字符串的元素
pub fn json_delete(left: &Value, key: &Value) -> Result<Value, JsonError> {
    let json = as_json(left)?;
    match key {
        Value::Text(k) => Ok(Value::Json(json_delete_key_impl(json, k))),
        Value::Int64(idx) => Ok(Value::Json(json_delete_index_impl(json, *idx)?)),
        Value::Null => Ok(Value::Json(json.clone())),
        _ => Err(JsonError::InvalidKeyType(value_type_name(key))),
    }
}

/// `#-` 操作符：按路径删除 → 返回 JSON
pub fn json_path_delete(left: &Value, path: &Value) -> Result<Value, JsonError> {
    let path = path_from_array(path)?;
    let json = as_json(left)?;
    let result = json_path_delete_impl(json, &path)?;
    Ok(Value::Json(result))
}

// =====================================================================
//  统一操作符分派
// =====================================================================

/// 按 `JsonOperator` 分派执行 JSON 操作
///
/// 便于执行器统一调用。
pub fn apply_json_operator(
    op: JsonOperator,
    left: &Value,
    right: &Value,
) -> Result<Value, JsonError> {
    match op {
        JsonOperator::Get => json_get(left, right),
        JsonOperator::GetAsText => json_get_as_text(left, right),
        JsonOperator::PathGet => json_path_get(left, right),
        JsonOperator::PathGetAsText => json_path_get_as_text(left, right),
        JsonOperator::Contains => json_contains(left, right),
        JsonOperator::ContainedBy => json_contained_by(left, right),
        JsonOperator::KeyExists => json_key_exists(left, right),
        JsonOperator::AnyKeyExists => json_any_key_exists(left, right),
        JsonOperator::AllKeysExist => json_all_keys_exist(left, right),
        JsonOperator::Concat => json_concat(left, right),
        JsonOperator::Delete => json_delete(left, right),
        JsonOperator::PathDelete => json_path_delete(left, right),
    }
}

// =====================================================================
//  内部辅助函数
// =====================================================================

/// 将 `Value` 转为 `&serde_json::Value`，非 JSON 返回错误
fn as_json(v: &Value) -> Result<&serde_json::Value, JsonError> {
    match v {
        Value::Json(j) => Ok(j),
        Value::Null => Err(JsonError::NotJson("Null")),
        Value::Int64(_) => Err(JsonError::NotJson("Int64")),
        Value::Float64(_) => Err(JsonError::NotJson("Float64")),
        Value::Text(_) => Err(JsonError::NotJson("Text")),
        Value::Bool(_) => Err(JsonError::NotJson("Bool")),
        _ => Err(JsonError::NotJson("other")),
    }
}

/// 获取 `Value` 类型名（用于错误信息）
fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "Null",
        Value::Int64(_) => "Int64",
        Value::Float64(_) => "Float64",
        Value::Text(_) => "Text",
        Value::Blob(_) => "Blob",
        Value::Bool(_) => "Bool",
        Value::Date(_) => "Date",
        Value::Timestamp(_) => "Timestamp",
        Value::Decimal(_, _) => "Decimal",
        Value::Array(_) => "Array",
        Value::Enum(_) => "Enum",
        Value::Range(_) => "Range",
        Value::Json(_) => "Json",
        Value::TsVector(_) => "TsVector",
        Value::TsQuery(_) => "TsQuery",
        Value::Vector(_) => "Vector",
        Value::Xml(_) => "Xml",
    }
}

/// 从对象获取字段
fn get_field(json: &serde_json::Value, key: &str) -> Value {
    match json {
        serde_json::Value::Object(map) => match map.get(key) {
            Some(v) => Value::Json(v.clone()),
            None => Value::Null,
        },
        _ => Value::Null,
    }
}

/// 从数组获取索引元素（支持负索引）
fn get_index(json: &serde_json::Value, idx: i64) -> Result<Value, JsonError> {
    match json {
        serde_json::Value::Array(arr) => {
            let len = arr.len() as i64;
            let actual = if idx < 0 {
                len + idx
            } else {
                idx
            };
            if actual < 0 {
                return Ok(Value::Null);
            }
            if actual >= len {
                return Ok(Value::Null);
            }
            Ok(Value::Json(arr[actual as usize].clone()))
        }
        _ => Ok(Value::Null),
    }
}

/// 按路径元素导航 JSON
fn navigate(
    json: &serde_json::Value,
    elem: &PathElement,
) -> Result<Option<serde_json::Value>, JsonError> {
    match elem {
        PathElement::Key(k) => match json {
            serde_json::Value::Object(map) => Ok(map.get(k).cloned()),
            _ => Ok(None),
        },
        PathElement::Index(i) => match json {
            serde_json::Value::Array(arr) => {
                let len = arr.len() as i64;
                let actual = if *i < 0 {
                    len + i
                } else {
                    *i
                };
                if actual < 0 || actual >= len {
                    Ok(None)
                } else {
                    Ok(Some(arr[actual as usize].clone()))
                }
            }
            _ => Ok(None),
        },
    }
}

/// 将 `serde_json::Value` 转为 `Value::Text`
///
/// PG 语义：
/// - 字符串：去引号，返回纯字符串
/// - 其他：JSON 序列化
fn json_to_text(j: &serde_json::Value) -> Value {
    match j {
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Null => Value::Null,
        _ => Value::Text(j.to_string()),
    }
}

/// 将 `Value` 转为键字符串（用于 `?` 系列操作符）
fn as_key_string(v: &Value) -> Result<String, JsonError> {
    match v {
        Value::Text(s) => Ok(s.clone()),
        Value::Int64(n) => Ok(n.to_string()),
        Value::Float64(f) => Ok(f.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Err(JsonError::InvalidKeyType("Null")),
        _ => Err(JsonError::InvalidKeyType(value_type_name(v))),
    }
}

/// 将 `Value::Array` 转为键字符串数组（用于 `?|` / `?&`）
fn as_key_string_array(v: &Value) -> Result<Vec<String>, JsonError> {
    match v {
        Value::Array(items) => items.iter().map(as_key_string).collect(),
        Value::Text(s) => Ok(vec![s.clone()]),
        Value::Int64(n) => Ok(vec![n.to_string()]),
        _ => Err(JsonError::InvalidKeyType(value_type_name(v))),
    }
}

/// `@>` 包含判断的实现
fn json_contains_impl(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    match (left, right) {
        (serde_json::Value::Object(lm), serde_json::Value::Object(rm)) => rm
            .iter()
            .all(|(k, v)| lm.get(k).is_some_and(|lv| json_contains_impl(lv, v))),
        (serde_json::Value::Array(la), serde_json::Value::Array(ra)) => ra
            .iter()
            .all(|rv| la.iter().any(|lv| json_value_equals(lv, rv))),
        (serde_json::Value::Array(la), r) if !r.is_array() => {
            la.iter().any(|lv| json_value_equals(lv, r))
        }
        (l, r) => json_value_equals(l, r),
    }
}

/// JSON 值相等比较（递归）
fn json_value_equals(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    a == b
}

/// `?` 键存在判断的实现
///
/// - 对象：检查键
/// - 数组：检查字符串元素
/// - 标量：false
fn key_exists_impl(json: &serde_json::Value, key: &str) -> bool {
    match json {
        serde_json::Value::Object(map) => map.contains_key(key),
        serde_json::Value::Array(arr) => arr
            .iter()
            .any(|v| matches!(v, serde_json::Value::String(s) if s == key)),
        _ => false,
    }
}

/// `||` 拼接的实现
fn json_concat_impl(left: &serde_json::Value, right: &serde_json::Value) -> serde_json::Value {
    match (left, right) {
        (serde_json::Value::Object(lm), serde_json::Value::Object(rm)) => {
            let mut merged = lm.clone();
            for (k, v) in rm {
                merged.insert(k.clone(), v.clone());
            }
            serde_json::Value::Object(merged)
        }
        (serde_json::Value::Array(la), serde_json::Value::Array(ra)) => {
            let mut merged = la.clone();
            merged.extend(ra.iter().cloned());
            serde_json::Value::Array(merged)
        }
        (_, r) => r.clone(),
    }
}

/// `-` 删除键的实现（对象）
fn json_delete_key_impl(json: &serde_json::Value, key: &str) -> serde_json::Value {
    match json {
        serde_json::Value::Object(map) => {
            let mut m = map.clone();
            m.remove(key);
            serde_json::Value::Object(m)
        }
        serde_json::Value::Array(arr) => {
            let filtered: Vec<_> = arr
                .iter()
                .filter(|v| !matches!(v, serde_json::Value::String(s) if s == key))
                .cloned()
                .collect();
            serde_json::Value::Array(filtered)
        }
        other => other.clone(),
    }
}

/// `-` 删除索引的实现（数组）
fn json_delete_index_impl(
    json: &serde_json::Value,
    idx: i64,
) -> Result<serde_json::Value, JsonError> {
    match json {
        serde_json::Value::Array(arr) => {
            let len = arr.len() as i64;
            let actual = if idx < 0 {
                len + idx
            } else {
                idx
            };
            if actual < 0 {
                return Err(JsonError::NegativeIndexTooLarge(idx));
            }
            if actual >= len {
                return Err(JsonError::IndexOutOfBounds(idx));
            }
            let mut v = arr.clone();
            v.remove(actual as usize);
            Ok(serde_json::Value::Array(v))
        }
        other => Ok(other.clone()),
    }
}

/// `#-` 按路径删除的实现（递归）
fn json_path_delete_impl(
    json: &serde_json::Value,
    path: &[PathElement],
) -> Result<serde_json::Value, JsonError> {
    if path.is_empty() {
        return Ok(json.clone());
    }
    let (first, rest) = path.split_first().unwrap();
    match json {
        serde_json::Value::Object(map) => match first {
            PathElement::Key(k) => {
                if let Some(child) = map.get(k) {
                    if rest.is_empty() {
                        let mut m = map.clone();
                        m.remove(k);
                        Ok(serde_json::Value::Object(m))
                    } else {
                        let new_child = json_path_delete_impl(child, rest)?;
                        let mut m = map.clone();
                        m.insert(k.clone(), new_child);
                        Ok(serde_json::Value::Object(m))
                    }
                } else {
                    Ok(json.clone())
                }
            }
            PathElement::Index(_) => Ok(json.clone()),
        },
        serde_json::Value::Array(arr) => match first {
            PathElement::Index(i) => {
                let len = arr.len() as i64;
                let actual = if *i < 0 {
                    len + i
                } else {
                    *i
                };
                if actual < 0 || actual >= len {
                    return Ok(json.clone());
                }
                let idx_usize = actual as usize;
                if rest.is_empty() {
                    let mut v = arr.clone();
                    v.remove(idx_usize);
                    Ok(serde_json::Value::Array(v))
                } else {
                    let new_child = json_path_delete_impl(&arr[idx_usize], rest)?;
                    let mut v = arr.clone();
                    v[idx_usize] = new_child;
                    Ok(serde_json::Value::Array(v))
                }
            }
            PathElement::Key(_) => Ok(json.clone()),
        },
        other => Ok(other.clone()),
    }
}

// =====================================================================
//  JSONB 索引
// =====================================================================

/// JSONB 倒排索引（GIN-like）
///
/// 用于加速 `@>` 包含查询。索引将每个 JSONB 值的所有"路径-值"对提取出来，
/// 维护"路径-值 → 行 ID 列表"倒排表。
///
/// 查询 `col @> '{"key": val}'` 时：
/// 1. 分解查询 JSON 的所有路径-值对
/// 2. 对每个路径-值取候选行 ID 集
/// 3. 取交集得到最终匹配行
///
/// # 设计
///
/// - 仅支持 `@>` 查询（`?`/`?|`/`?&` 不走索引，回退到顺序扫描）
/// - 单列索引
/// - 内存索引，无并发控制
pub struct JsonbIndex {
    /// 索引名
    name: String,
    /// 所属表名
    table_name: String,
    /// 索引列名
    column_name: String,
    /// 倒排表：路径-值 → 行 ID 列表（按行 ID 升序）
    postings: HashMap<String, Vec<usize>>,
    /// 已索引行数
    indexed_count: usize,
}

impl JsonbIndex {
    /// 创建新 JSONB 索引
    pub fn new(
        name: impl Into<String>,
        table_name: impl Into<String>,
        column_name: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            table_name: table_name.into(),
            column_name: column_name.into(),
            postings: HashMap::new(),
            indexed_count: 0,
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
    pub fn column_name(&self) -> &str {
        &self.column_name
    }

    /// 已索引行数
    pub fn indexed_count(&self) -> usize {
        self.indexed_count
    }

    /// 是否为空索引
    pub fn is_empty(&self) -> bool {
        self.indexed_count == 0
    }

    /// 清空索引
    pub fn clear(&mut self) {
        self.postings.clear();
        self.indexed_count = 0;
    }

    /// 从表构建索引
    ///
    /// 扫描指定列的所有 JSON 值，建立倒排表。
    /// 非 JSON 值（包括 NULL）跳过。
    pub fn build_from_table(&mut self, table: &dyn TableStorage) -> Result<(), JsonError> {
        self.clear();
        let col_idx = self.find_column_index(table.schema())?;
        for (row_id, row) in table.scan_with_ids() {
            if let Some(Value::Json(j)) = row.get(col_idx) {
                self.index_json(row_id, j);
                self.indexed_count += 1;
            }
        }
        Ok(())
    }

    /// 插入一行
    ///
    /// 若列值为 JSON，提取路径-值对并加入倒排表。
    pub fn insert(&mut self, row_id: usize, value: &Value) {
        if let Value::Json(j) = value {
            self.index_json(row_id, j);
            self.indexed_count += 1;
        }
    }

    /// 删除一行
    ///
    /// 从倒排表中移除该行 ID 的所有条目。
    pub fn remove(&mut self, row_id: usize) {
        for posting in self.postings.values_mut() {
            posting.retain(|&id| id != row_id);
        }
        self.postings.retain(|_, v| !v.is_empty());
        if self.indexed_count > 0 {
            self.indexed_count -= 1;
        }
    }

    /// `@>` 包含查询：返回所有 JSON 包含 `query` 的行 ID
    ///
    /// 算法：
    /// 1. 分解 `query` 的所有路径-值对
    /// 2. 对每个路径-值查倒排表得到候选集
    /// 3. 取所有候选集的交集
    pub fn contains_query(&self, query: &serde_json::Value) -> Vec<usize> {
        let path_values = extract_path_values(query);
        if path_values.is_empty() {
            return Vec::new();
        }
        let mut candidates: Option<Vec<usize>> = None;
        for (pv, val) in &path_values {
            let key = make_posting_key(pv, val);
            let posting = self.postings.get(&key).cloned().unwrap_or_default();
            candidates = Some(match candidates {
                None => posting,
                Some(curr) => intersect_sorted(&curr, &posting),
            });
        }
        candidates.unwrap_or_default()
    }

    /// `@>` 包含查询（Value 版本）
    pub fn contains_query_value(&self, query: &Value) -> Result<Vec<usize>, JsonError> {
        match query {
            Value::Json(j) => Ok(self.contains_query(j)),
            _ => Err(JsonError::NotJson(value_type_name(query))),
        }
    }

    /// 查找列索引
    fn find_column_index(&self, schema: &TableSchema) -> Result<usize, JsonError> {
        schema
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(&self.column_name))
            .ok_or_else(|| JsonError::ParseError(format!("column not found: {}", self.column_name)))
    }

    /// 索引单个 JSON 值
    fn index_json(&mut self, row_id: usize, json: &serde_json::Value) {
        let path_values = extract_path_values(json);
        for (pv, val) in path_values {
            let key = make_posting_key(&pv, &val);
            self.postings.entry(key).or_default().push(row_id);
        }
    }
}

/// 提取 JSON 的所有"路径-值"对
///
/// 路径表示为 `$.key1.key2[0].key3` 形式（PG JSONPath 简化）。
/// 标量值直接作为路径终点。
///
/// 返回 `(路径-值字符串, 原始值)` 对列表。
fn extract_path_values(json: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    let mut result = Vec::new();
    extract_path_values_recursive(json, "$", &mut result);
    result
}

/// 构造倒排表键 — `path=serialized_value`
///
/// 使用 `serde_json::to_string` 序列化值，保证索引/查询键一致。
fn make_posting_key(path: &str, value: &serde_json::Value) -> String {
    format!("{path}={value}")
}

/// 递归提取路径-值对
///
/// 对象的键名加入路径（`$.key`）；数组的元素不加索引（`$.path`），
/// 使得 `@>` 数组包含查询与顺序无关（与 PG JSONB GIN 语义一致）。
fn extract_path_values_recursive(
    json: &serde_json::Value,
    path: &str,
    out: &mut Vec<(String, serde_json::Value)>,
) {
    match json {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let child_path = format!("{path}.{k}");
                extract_path_values_recursive(v, &child_path, out);
            }
        }
        serde_json::Value::Array(arr) => {
            // 数组元素不加索引 — 所有元素共享数组路径，使 @> 查询与顺序无关
            for v in arr {
                extract_path_values_recursive(v, path, out);
            }
        }
        scalar => {
            out.push((path.to_string(), scalar.clone()));
        }
    }
}

/// 有序数组交集
fn intersect_sorted(a: &[usize], b: &[usize]) -> Vec<usize> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            result.push(a[i]);
            i += 1;
            j += 1;
        } else if a[i] < b[j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    result
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::ColumnDefinition;
    use crate::executor::{InMemoryTable, MutableTable};
    use szrsql_types::value::ColumnType;

    /// 辅助：构造 JSON Value
    fn j(s: &str) -> Value {
        Value::Json(serde_json::from_str(s).unwrap())
    }

    /// 辅助：构造 JSON serde 值
    fn sj(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap()
    }

    /// 辅助：构造 Text Value
    fn t(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    /// 辅助：构造 Int64 Value
    fn n(i: i64) -> Value {
        Value::Int64(i)
    }

    // =====================================================================
    //  -> 操作符测试
    // =====================================================================

    #[test]
    fn test_get_object_field() {
        let json = j(r#"{"a": 1, "b": "hello"}"#);
        assert_eq!(json_get(&json, &t("a")).unwrap(), j("1"));
        assert_eq!(json_get(&json, &t("b")).unwrap(), j(r#""hello""#));
    }

    #[test]
    fn test_get_object_field_not_exists() {
        let json = j(r#"{"a": 1}"#);
        assert_eq!(json_get(&json, &t("missing")), Ok(Value::Null));
    }

    #[test]
    fn test_get_array_index() {
        let json = j(r#"[10, 20, 30]"#);
        assert_eq!(json_get(&json, &n(0)).unwrap(), j("10"));
        assert_eq!(json_get(&json, &n(1)).unwrap(), j("20"));
        assert_eq!(json_get(&json, &n(2)).unwrap(), j("30"));
    }

    #[test]
    fn test_get_array_index_negative() {
        let json = j(r#"[10, 20, 30]"#);
        assert_eq!(json_get(&json, &n(-1)).unwrap(), j("30"));
        assert_eq!(json_get(&json, &n(-3)).unwrap(), j("10"));
    }

    #[test]
    fn test_get_array_index_out_of_bounds() {
        let json = j(r#"[10, 20]"#);
        assert_eq!(json_get(&json, &n(5)), Ok(Value::Null));
        assert_eq!(json_get(&json, &n(-5)), Ok(Value::Null));
    }

    #[test]
    fn test_get_nested() {
        let json = j(r#"{"a": {"b": [1, 2, 3]}}"#);
        let step1 = json_get(&json, &t("a")).unwrap();
        let step2 = json_get(&step1, &t("b")).unwrap();
        let step3 = json_get(&step2, &n(1)).unwrap();
        assert_eq!(step3, j("2"));
    }

    #[test]
    fn test_get_on_scalar_returns_null() {
        let json = j("42");
        assert_eq!(json_get(&json, &t("a")), Ok(Value::Null));
        assert_eq!(json_get(&json, &n(0)), Ok(Value::Null));
    }

    #[test]
    fn test_get_invalid_key_type() {
        let json = j(r#"{"a": 1}"#);
        assert!(matches!(
            json_get(&json, &Value::Bool(true)),
            Err(JsonError::InvalidKeyType(_))
        ));
    }

    #[test]
    fn test_get_non_json_left() {
        let result = json_get(&Value::Int64(42), &t("a"));
        assert!(matches!(result, Err(JsonError::NotJson(_))));
    }

    #[test]
    fn test_get_with_null_key() {
        let json = j(r#"{"a": 1}"#);
        assert_eq!(json_get(&json, &Value::Null).unwrap(), Value::Null);
    }

    // =====================================================================
    //  ->> 操作符测试
    // =====================================================================

    #[test]
    fn test_get_as_text_string() {
        let json = j(r#"{"a": "hello"}"#);
        assert_eq!(json_get_as_text(&json, &t("a")).unwrap(), t("hello"));
    }

    #[test]
    fn test_get_as_text_number() {
        let json = j(r#"{"a": 42}"#);
        assert_eq!(json_get_as_text(&json, &t("a")).unwrap(), t("42"));
    }

    #[test]
    fn test_get_as_text_bool() {
        let json = j(r#"{"a": true}"#);
        assert_eq!(json_get_as_text(&json, &t("a")).unwrap(), t("true"));
    }

    #[test]
    fn test_get_as_text_array() {
        let json = j(r#"{"a": [1, 2, 3]}"#);
        assert_eq!(json_get_as_text(&json, &t("a")).unwrap(), t("[1,2,3]"));
    }

    #[test]
    fn test_get_as_text_object() {
        let json = j(r#"{"a": {"x": 1}}"#);
        assert_eq!(json_get_as_text(&json, &t("a")).unwrap(), t(r#"{"x":1}"#));
    }

    #[test]
    fn test_get_as_text_null_field() {
        let json = j(r#"{"a": null}"#);
        assert_eq!(json_get_as_text(&json, &t("a")).unwrap(), Value::Null);
    }

    #[test]
    fn test_get_as_text_missing_field() {
        let json = j(r#"{"a": 1}"#);
        assert_eq!(json_get_as_text(&json, &t("missing")), Ok(Value::Null));
    }

    // =====================================================================
    //  #> / #>> 路径操作符测试
    // =====================================================================

    #[test]
    fn test_path_get_object() {
        let json = j(r#"{"a": {"b": {"c": 42}}}"#);
        let path = Value::Array(vec![t("a"), t("b"), t("c")]);
        assert_eq!(json_path_get(&json, &path).unwrap(), j("42"));
    }

    #[test]
    fn test_path_get_array() {
        let json = j(r#"[{"x": 1}, {"x": 2}]"#);
        let path = Value::Array(vec![n(1), t("x")]);
        assert_eq!(json_path_get(&json, &path).unwrap(), j("2"));
    }

    #[test]
    fn test_path_get_mixed() {
        let json = j(r#"{"arr": [{"k": "v"}, {"k": "w"}]}"#);
        let path = Value::Array(vec![t("arr"), n(1), t("k")]);
        assert_eq!(json_path_get(&json, &path).unwrap(), j(r#""w""#));
    }

    #[test]
    fn test_path_get_missing() {
        let json = j(r#"{"a": 1}"#);
        let path = Value::Array(vec![t("a"), t("b")]);
        assert_eq!(json_path_get(&json, &path).unwrap(), Value::Null);
    }

    #[test]
    fn test_path_get_empty_path() {
        let json = j(r#"{"a": 1}"#);
        let path = Value::Array(vec![]);
        assert_eq!(json_path_get(&json, &path).unwrap(), json);
    }

    #[test]
    fn test_path_get_as_text() {
        let json = j(r#"{"a": {"b": "hello"}}"#);
        let path = Value::Array(vec![t("a"), t("b")]);
        assert_eq!(json_path_get_as_text(&json, &path).unwrap(), t("hello"));
    }

    #[test]
    fn test_path_get_as_text_number() {
        let json = j(r#"{"a": {"b": 42}}"#);
        let path = Value::Array(vec![t("a"), t("b")]);
        assert_eq!(json_path_get_as_text(&json, &path).unwrap(), t("42"));
    }

    #[test]
    fn test_path_get_with_negative_index() {
        let json = j(r#"{"arr": [10, 20, 30]}"#);
        let path = Value::Array(vec![t("arr"), n(-1)]);
        assert_eq!(json_path_get(&json, &path).unwrap(), j("30"));
    }

    // =====================================================================
    //  @> / <@ 包含操作符测试
    // =====================================================================

    #[test]
    fn test_contains_object_subset() {
        let left = j(r#"{"a": 1, "b": 2, "c": 3}"#);
        let right = j(r#"{"a": 1, "b": 2}"#);
        assert_eq!(json_contains(&left, &right).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_contains_object_not_subset() {
        let left = j(r#"{"a": 1}"#);
        let right = j(r#"{"a": 1, "b": 2}"#);
        assert_eq!(json_contains(&left, &right).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_contains_object_nested() {
        let left = j(r#"{"a": {"x": 1, "y": 2}, "b": 5}"#);
        let right = j(r#"{"a": {"x": 1}}"#);
        assert_eq!(json_contains(&left, &right).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_contains_object_value_mismatch() {
        let left = j(r#"{"a": 1}"#);
        let right = j(r#"{"a": 2}"#);
        assert_eq!(json_contains(&left, &right).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_contains_array_elements() {
        let left = j(r#"[1, 2, 3, 4]"#);
        let right = j(r#"[2, 3]"#);
        assert_eq!(json_contains(&left, &right).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_contains_array_not_subset() {
        let left = j(r#"[1, 2, 3]"#);
        let right = j(r#"[1, 5]"#);
        assert_eq!(json_contains(&left, &right).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_contains_array_with_objects() {
        let left = j(r#"[{"x": 1}, {"y": 2}]"#);
        let right = j(r#"[{"x": 1}]"#);
        assert_eq!(json_contains(&left, &right).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_contains_array_scalar() {
        let left = j(r#"[1, 2, 3]"#);
        let right = j("2");
        assert_eq!(json_contains(&left, &right).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_contains_scalar_equal() {
        let left = j("42");
        let right = j("42");
        assert_eq!(json_contains(&left, &right).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_contains_scalar_not_equal() {
        let left = j("42");
        let right = j("43");
        assert_eq!(json_contains(&left, &right).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_contained_by() {
        let left = j(r#"{"a": 1}"#);
        let right = j(r#"{"a": 1, "b": 2}"#);
        assert_eq!(json_contained_by(&left, &right).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_contained_by_symmetry() {
        let a = j(r#"{"a": 1, "b": 2}"#);
        let b = j(r#"{"a": 1}"#);
        assert_eq!(
            json_contains(&a, &b).unwrap(),
            json_contained_by(&b, &a).unwrap()
        );
    }

    // =====================================================================
    //  ? / ?| / ?& 键存在操作符测试
    // =====================================================================

    #[test]
    fn test_key_exists_object_yes() {
        let json = j(r#"{"a": 1, "b": 2}"#);
        assert_eq!(json_key_exists(&json, &t("a")).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_key_exists_object_no() {
        let json = j(r#"{"a": 1}"#);
        assert_eq!(
            json_key_exists(&json, &t("missing")).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_key_exists_array_string() {
        let json = j(r#"["a", "b", "c"]"#);
        assert_eq!(json_key_exists(&json, &t("b")).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_key_exists_array_not_string() {
        let json = j(r#"["a", "b"]"#);
        assert_eq!(json_key_exists(&json, &t("c")).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_key_exists_scalar() {
        let json = j("42");
        assert_eq!(json_key_exists(&json, &t("a")).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_key_exists_integer_key() {
        let json = j(r#"{"42": "value"}"#);
        assert_eq!(json_key_exists(&json, &n(42)).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_any_key_exists() {
        let json = j(r#"{"a": 1, "b": 2}"#);
        let keys = Value::Array(vec![t("a"), t("c")]);
        assert_eq!(
            json_any_key_exists(&json, &keys).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_any_key_exists_none() {
        let json = j(r#"{"a": 1}"#);
        let keys = Value::Array(vec![t("b"), t("c")]);
        assert_eq!(
            json_any_key_exists(&json, &keys).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_all_keys_exist_yes() {
        let json = j(r#"{"a": 1, "b": 2, "c": 3}"#);
        let keys = Value::Array(vec![t("a"), t("b")]);
        assert_eq!(
            json_all_keys_exist(&json, &keys).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_all_keys_exist_no() {
        let json = j(r#"{"a": 1, "b": 2}"#);
        let keys = Value::Array(vec![t("a"), t("c")]);
        assert_eq!(
            json_all_keys_exist(&json, &keys).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_all_keys_exist_empty() {
        let json = j(r#"{"a": 1}"#);
        let keys = Value::Array(vec![]);
        assert_eq!(
            json_all_keys_exist(&json, &keys).unwrap(),
            Value::Bool(true)
        );
    }

    // =====================================================================
    //  || 拼接操作符测试
    // =====================================================================

    #[test]
    fn test_concat_objects() {
        let left = j(r#"{"a": 1}"#);
        let right = j(r#"{"b": 2}"#);
        assert_eq!(
            json_concat(&left, &right).unwrap(),
            j(r#"{"a": 1, "b": 2}"#)
        );
    }

    #[test]
    fn test_concat_objects_overwrite() {
        let left = j(r#"{"a": 1, "b": 2}"#);
        let right = j(r#"{"b": 99}"#);
        assert_eq!(
            json_concat(&left, &right).unwrap(),
            j(r#"{"a": 1, "b": 99}"#)
        );
    }

    #[test]
    fn test_concat_arrays() {
        let left = j(r#"[1, 2]"#);
        let right = j(r#"[3, 4]"#);
        assert_eq!(json_concat(&left, &right).unwrap(), j(r#"[1, 2, 3, 4]"#));
    }

    #[test]
    fn test_concat_mixed_returns_right() {
        let left = j(r#"{"a": 1}"#);
        let right = j(r#"[1, 2]"#);
        assert_eq!(json_concat(&left, &right).unwrap(), j(r#"[1, 2]"#));
    }

    #[test]
    fn test_concat_with_scalar() {
        let left = j(r#"[1, 2]"#);
        let right = j("99");
        assert_eq!(json_concat(&left, &right).unwrap(), j("99"));
    }

    // =====================================================================
    //  - 删除操作符测试
    // =====================================================================

    #[test]
    fn test_delete_object_key() {
        let json = j(r#"{"a": 1, "b": 2}"#);
        assert_eq!(json_delete(&json, &t("a")).unwrap(), j(r#"{"b": 2}"#));
    }

    #[test]
    fn test_delete_object_missing_key() {
        let json = j(r#"{"a": 1}"#);
        assert_eq!(json_delete(&json, &t("missing")).unwrap(), j(r#"{"a": 1}"#));
    }

    #[test]
    fn test_delete_array_index() {
        let json = j(r#"[1, 2, 3]"#);
        assert_eq!(json_delete(&json, &n(1)).unwrap(), j(r#"[1, 3]"#));
    }

    #[test]
    fn test_delete_array_negative_index() {
        let json = j(r#"[1, 2, 3]"#);
        assert_eq!(json_delete(&json, &n(-1)).unwrap(), j(r#"[1, 2]"#));
    }

    #[test]
    fn test_delete_array_out_of_bounds() {
        let json = j(r#"[1, 2]"#);
        assert!(matches!(
            json_delete(&json, &n(5)),
            Err(JsonError::IndexOutOfBounds(_))
        ));
    }

    #[test]
    fn test_delete_array_by_string() {
        let json = j(r#"["a", "b", "c", "b"]"#);
        assert_eq!(json_delete(&json, &t("b")).unwrap(), j(r#"["a", "c"]"#));
    }

    #[test]
    fn test_delete_scalar() {
        let json = j("42");
        assert_eq!(json_delete(&json, &t("a")).unwrap(), j("42"));
    }

    // =====================================================================
    //  #- 路径删除操作符测试
    // =====================================================================

    #[test]
    fn test_path_delete_object() {
        let json = j(r#"{"a": 1, "b": 2}"#);
        let path = Value::Array(vec![t("a")]);
        assert_eq!(json_path_delete(&json, &path).unwrap(), j(r#"{"b": 2}"#));
    }

    #[test]
    fn test_path_delete_nested() {
        let json = j(r#"{"a": {"b": 1, "c": 2}}"#);
        let path = Value::Array(vec![t("a"), t("b")]);
        assert_eq!(
            json_path_delete(&json, &path).unwrap(),
            j(r#"{"a": {"c": 2}}"#)
        );
    }

    #[test]
    fn test_path_delete_array_index() {
        let json = j(r#"[1, 2, 3]"#);
        let path = Value::Array(vec![n(1)]);
        assert_eq!(json_path_delete(&json, &path).unwrap(), j(r#"[1, 3]"#));
    }

    #[test]
    fn test_path_delete_mixed() {
        let json = j(r#"{"arr": [1, 2, 3]}"#);
        let path = Value::Array(vec![t("arr"), n(1)]);
        assert_eq!(
            json_path_delete(&json, &path).unwrap(),
            j(r#"{"arr": [1, 3]}"#)
        );
    }

    #[test]
    fn test_path_delete_empty_path() {
        let json = j(r#"{"a": 1}"#);
        let path = Value::Array(vec![]);
        assert_eq!(json_path_delete(&json, &path).unwrap(), json);
    }

    #[test]
    fn test_path_delete_missing_key() {
        let json = j(r#"{"a": 1}"#);
        let path = Value::Array(vec![t("missing")]);
        assert_eq!(json_path_delete(&json, &path).unwrap(), j(r#"{"a": 1}"#));
    }

    // =====================================================================
    //  统一分派测试
    // =====================================================================

    #[test]
    fn test_apply_get_operator() {
        let json = j(r#"{"a": 1}"#);
        let result = apply_json_operator(JsonOperator::Get, &json, &t("a")).unwrap();
        assert_eq!(result, j("1"));
    }

    #[test]
    fn test_apply_contains_operator() {
        let left = j(r#"{"a": 1, "b": 2}"#);
        let right = j(r#"{"a": 1}"#);
        let result = apply_json_operator(JsonOperator::Contains, &left, &right).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_apply_concat_operator() {
        let left = j(r#"{"a": 1}"#);
        let right = j(r#"{"b": 2}"#);
        let result = apply_json_operator(JsonOperator::Concat, &left, &right).unwrap();
        assert_eq!(result, j(r#"{"a": 1, "b": 2}"#));
    }

    #[test]
    fn test_operator_as_str() {
        assert_eq!(JsonOperator::Get.as_str(), "->");
        assert_eq!(JsonOperator::GetAsText.as_str(), "->>");
        assert_eq!(JsonOperator::PathGet.as_str(), "#>");
        assert_eq!(JsonOperator::PathGetAsText.as_str(), "#>>");
        assert_eq!(JsonOperator::Contains.as_str(), "@>");
        assert_eq!(JsonOperator::ContainedBy.as_str(), "<@");
        assert_eq!(JsonOperator::KeyExists.as_str(), "?");
        assert_eq!(JsonOperator::AnyKeyExists.as_str(), "?|");
        assert_eq!(JsonOperator::AllKeysExist.as_str(), "?&");
        assert_eq!(JsonOperator::Concat.as_str(), "||");
        assert_eq!(JsonOperator::Delete.as_str(), "-");
        assert_eq!(JsonOperator::PathDelete.as_str(), "#-");
    }

    // =====================================================================
    //  PathElement / JsonPath 测试
    // =====================================================================

    #[test]
    fn test_path_element_from_value_text() {
        let v = t("key");
        let pe = PathElement::from_value(&v).unwrap();
        assert_eq!(pe, PathElement::Key("key".to_string()));
    }

    #[test]
    fn test_path_element_from_value_int() {
        let v = n(5);
        let pe = PathElement::from_value(&v).unwrap();
        assert_eq!(pe, PathElement::Index(5));
    }

    #[test]
    fn test_path_element_from_value_invalid() {
        assert!(PathElement::from_value(&Value::Bool(true)).is_err());
        assert!(PathElement::from_value(&Value::Null).is_err());
    }

    #[test]
    fn test_path_from_array() {
        let arr = Value::Array(vec![t("a"), n(1), t("b")]);
        let path = path_from_array(&arr).unwrap();
        assert_eq!(path.len(), 3);
        assert_eq!(path[0], PathElement::Key("a".to_string()));
        assert_eq!(path[1], PathElement::Index(1));
        assert_eq!(path[2], PathElement::Key("b".to_string()));
    }

    #[test]
    fn test_path_from_non_array() {
        let v = t("not_array");
        assert!(path_from_array(&v).is_err());
    }

    // =====================================================================
    //  extract_path_values 测试
    // =====================================================================

    #[test]
    fn test_extract_path_values_scalar() {
        let json = sj("42");
        let pvs = extract_path_values(&json);
        assert_eq!(pvs.len(), 1);
        assert_eq!(pvs[0].0, "$");
        assert_eq!(pvs[0].1, sj("42"));
    }

    #[test]
    fn test_extract_path_values_object() {
        let json = sj(r#"{"a": 1, "b": 2}"#);
        let pvs = extract_path_values(&json);
        assert_eq!(pvs.len(), 2);
        let paths: Vec<_> = pvs.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"$.a"));
        assert!(paths.contains(&"$.b"));
    }

    #[test]
    fn test_extract_path_values_nested() {
        // 数组元素共享数组路径（不加索引后缀）— 使 @> 查询与顺序无关（PG JSONB GIN 语义）
        let json = sj(r#"{"a": {"b": 1}, "c": [10, 20]}"#);
        let pvs = extract_path_values(&json);
        assert_eq!(pvs.len(), 3);
        let paths: Vec<_> = pvs.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"$.a.b"));
        assert!(paths.contains(&"$.c")); // 两个数组元素都映射到 $.c
        let c_entries: Vec<_> = pvs
            .iter()
            .filter(|(p, _)| p == "$.c")
            .map(|(_, v)| v.clone())
            .collect();
        assert_eq!(c_entries.len(), 2);
        assert!(c_entries.contains(&sj("10")));
        assert!(c_entries.contains(&sj("20")));
    }

    #[test]
    fn test_extract_path_values_empty() {
        let json = sj("{}");
        let pvs = extract_path_values(&json);
        assert!(pvs.is_empty());
    }

    #[test]
    fn test_intersect_sorted_basic() {
        let a = vec![1, 3, 5, 7, 9];
        let b = vec![3, 4, 5, 6, 7];
        assert_eq!(intersect_sorted(&a, &b), vec![3, 5, 7]);
    }

    #[test]
    fn test_intersect_sorted_disjoint() {
        let a = vec![1, 3, 5];
        let b = vec![2, 4, 6];
        assert!(intersect_sorted(&a, &b).is_empty());
    }

    #[test]
    fn test_intersect_sorted_empty() {
        let a: Vec<usize> = vec![];
        let b = vec![1, 2, 3];
        assert!(intersect_sorted(&a, &b).is_empty());
    }

    // =====================================================================
    //  JsonbIndex 测试
    // =====================================================================

    #[test]
    fn test_jsonb_index_basic_query() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &j(r#"{"name": "Alice", "age": 30}"#));
        idx.insert(1, &j(r#"{"name": "Bob", "age": 25}"#));
        idx.insert(2, &j(r#"{"name": "Alice", "age": 40}"#));

        let results = idx.contains_query(&sj(r#"{"name": "Alice"}"#));
        assert_eq!(results, vec![0, 2]);
    }

    #[test]
    fn test_jsonb_index_query_with_nested() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &j(r#"{"profile": {"city": "NYC"}}"#));
        idx.insert(1, &j(r#"{"profile": {"city": "LA"}}"#));
        idx.insert(2, &j(r#"{"profile": {"city": "NYC", "zip": "10001"}}"#));

        let results = idx.contains_query(&sj(r#"{"profile": {"city": "NYC"}}"#));
        assert_eq!(results, vec![0, 2]);
    }

    #[test]
    fn test_jsonb_index_query_no_match() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &j(r#"{"a": 1}"#));

        let results = idx.contains_query(&sj(r#"{"a": 2}"#));
        assert!(results.is_empty());
    }

    #[test]
    fn test_jsonb_index_query_multiple_conditions() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &j(r#"{"a": 1, "b": 2}"#));
        idx.insert(1, &j(r#"{"a": 1, "b": 3}"#));
        idx.insert(2, &j(r#"{"a": 2, "b": 2}"#));

        let results = idx.contains_query(&sj(r#"{"a": 1, "b": 2}"#));
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn test_jsonb_index_remove() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &j(r#"{"a": 1}"#));
        idx.insert(1, &j(r#"{"a": 1}"#));

        let before = idx.contains_query(&sj(r#"{"a": 1}"#));
        assert_eq!(before, vec![0, 1]);

        idx.remove(0);
        let after = idx.contains_query(&sj(r#"{"a": 1}"#));
        assert_eq!(after, vec![1]);
    }

    #[test]
    fn test_jsonb_index_clear() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &j(r#"{"a": 1}"#));
        assert!(!idx.is_empty());

        idx.clear();
        assert!(idx.is_empty());
        assert_eq!(idx.indexed_count(), 0);
    }

    #[test]
    fn test_jsonb_index_metadata() {
        let idx = JsonbIndex::new("idx_test", "users", "data");
        assert_eq!(idx.name(), "idx_test");
        assert_eq!(idx.table_name(), "users");
        assert_eq!(idx.column_name(), "data");
        assert_eq!(idx.indexed_count(), 0);
        assert!(idx.is_empty());
    }

    #[test]
    fn test_jsonb_index_insert_non_json_skipped() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &Value::Int64(42));
        idx.insert(1, &Value::Null);
        idx.insert(2, &j(r#"{"a": 1}"#));
        assert_eq!(idx.indexed_count(), 1);
    }

    #[test]
    fn test_jsonb_index_empty_query() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &j(r#"{"a": 1}"#));

        let results = idx.contains_query(&sj("{}"));
        assert!(results.is_empty());
    }

    #[test]
    fn test_jsonb_index_query_array() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &j(r#"[1, 2, 3]"#));
        idx.insert(1, &j(r#"[4, 5, 6]"#));
        idx.insert(2, &j(r#"[1, 2, 3, 4]"#));

        let results = idx.contains_query(&sj("[1, 2]"));
        assert_eq!(results, vec![0, 2]);
    }

    #[test]
    fn test_jsonb_index_query_scalar() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &j("42"));
        idx.insert(1, &j("43"));
        idx.insert(2, &j(r#"{"a": 42}"#));

        let results = idx.contains_query(&sj("42"));
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn test_jsonb_index_build_from_table() {
        let schema = TableSchema {
            name: crate::ast::TableName::new("t"),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("data", ColumnType::Json),
            ],
        };
        let mut table = InMemoryTable::new(schema);
        table.insert_row(vec![n(1), j(r#"{"name": "Alice"}"#)]);
        table.insert_row(vec![n(2), j(r#"{"name": "Bob"}"#)]);
        table.insert_row(vec![n(3), j(r#"{"name": "Alice"}"#)]);

        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.build_from_table(&table).unwrap();
        assert_eq!(idx.indexed_count(), 3);

        let results = idx.contains_query(&sj(r#"{"name": "Alice"}"#));
        assert_eq!(results, vec![0, 2]);
    }

    #[test]
    fn test_jsonb_index_build_skips_non_json() {
        let schema = TableSchema {
            name: crate::ast::TableName::new("t"),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("data", ColumnType::Json),
            ],
        };
        let mut table = InMemoryTable::new(schema);
        table.insert_row(vec![n(1), j(r#"{"a": 1}"#)]);
        table.insert_row(vec![n(2), Value::Null]);
        table.insert_row(vec![n(3), j(r#"{"a": 1}"#)]);

        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.build_from_table(&table).unwrap();
        assert_eq!(idx.indexed_count(), 2);
    }

    #[test]
    fn test_jsonb_index_contains_query_value() {
        let mut idx = JsonbIndex::new("idx_j", "t", "data");
        idx.insert(0, &j(r#"{"a": 1}"#));

        let query = j(r#"{"a": 1}"#);
        let results = idx.contains_query_value(&query).unwrap();
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn test_jsonb_index_contains_query_value_non_json() {
        let idx = JsonbIndex::new("idx_j", "t", "data");
        let result = idx.contains_query_value(&Value::Int64(42));
        assert!(matches!(result, Err(JsonError::NotJson(_))));
    }

    #[test]
    fn test_jsonb_index_column_not_found() {
        let schema = TableSchema {
            name: crate::ast::TableName::new("t"),
            columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
        };
        let table = InMemoryTable::new(schema);

        let mut idx = JsonbIndex::new("idx_j", "t", "missing");
        assert!(idx.build_from_table(&table).is_err());
    }

    // =====================================================================
    //  INSERT 场景模拟测试
    // =====================================================================

    #[test]
    fn test_simulate_insert_and_query() {
        // 模拟 CREATE TABLE t (id INT, data JSON)
        // INSERT INTO t VALUES (1, '{"name": "Alice", "tags": ["a", "b"]}')
        // INSERT INTO t VALUES (2, '{"name": "Bob", "tags": ["b", "c"]}')
        // SELECT * FROM t WHERE data @> '{"name": "Alice"}'

        let schema = TableSchema {
            name: crate::ast::TableName::new("t"),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("data", ColumnType::Json),
            ],
        };
        let mut table = InMemoryTable::new(schema);
        table.insert_row(vec![n(1), j(r#"{"name": "Alice", "tags": ["a", "b"]}"#)]);
        table.insert_row(vec![n(2), j(r#"{"name": "Bob", "tags": ["b", "c"]}"#)]);

        // 构建索引
        let mut idx = JsonbIndex::new("idx_data", "t", "data");
        idx.build_from_table(&table).unwrap();

        // 查询 data @> '{"name": "Alice"}'
        let row_ids = idx.contains_query(&sj(r#"{"name": "Alice"}"#));
        assert_eq!(row_ids, vec![0]);

        // 验证行内容
        let rows: Vec<_> = table.scan_with_ids().collect();
        let matched_row = &rows[row_ids[0]].1;
        assert_eq!(matched_row[0], n(1));
    }

    #[test]
    fn test_simulate_get_operator_query() {
        // 模拟 SELECT data->'name' FROM t
        let json = j(r#"{"name": "Alice", "age": 30}"#);
        let name = json_get(&json, &t("name")).unwrap();
        assert_eq!(name, j(r#""Alice""#));

        let name_text = json_get_as_text(&json, &t("name")).unwrap();
        assert_eq!(name_text, t("Alice"));
    }

    #[test]
    fn test_simulate_contains_query_with_index() {
        // 模拟 SELECT * FROM t WHERE data @> '{"tags": ["a"]}'
        let schema = TableSchema {
            name: crate::ast::TableName::new("t"),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("data", ColumnType::Json),
            ],
        };
        let mut table = InMemoryTable::new(schema);
        table.insert_row(vec![n(1), j(r#"{"tags": ["a", "b"]}"#)]);
        table.insert_row(vec![n(2), j(r#"{"tags": ["b", "c"]}"#)]);
        table.insert_row(vec![n(3), j(r#"{"tags": ["a", "c"]}"#)]);

        let mut idx = JsonbIndex::new("idx_tags", "t", "data");
        idx.build_from_table(&table).unwrap();

        // 查询 data @> '{"tags": ["a"]}'
        let row_ids = idx.contains_query(&sj(r#"{"tags": ["a"]}"#));
        assert_eq!(row_ids, vec![0, 2]);
    }
}
