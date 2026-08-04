//! Phase 4.8 COPY FROM/TO — CSV/TEXT 解析与序列化。
//!
//! # 职责
//!
//! - 提供 CSV/TEXT 格式的行解析（文件字符串 → `Vec<String>`）
//! - 提供 CSV/TEXT 格式的行序列化（`Vec<String>` → 文件字符串）
//! - 提供 `Value` ↔ 字符串转换（结合列类型与 NULL 字符串）
//!
//! # 设计
//!
//! - 纯函数模块，不涉及文件 I/O 与会话状态
//! - CSV 解析遵循 RFC 4180 子集：支持引号引用、转义，不支持嵌入换行符
//! - TEXT 格式遵循 PG 默认：TAB 分隔，`\N` 表示 NULL，无引号引用
//! - 会话层（`session.rs::execute_copy_plan`）负责文件 I/O、表锁、批量 INSERT

use szrsql_types::value::{CastError, ColumnType, Value};
use thiserror::Error;

// =====================================================================
//  错误类型
// =====================================================================

/// COPY 操作错误
#[derive(Debug, Clone, Error)]
pub enum CopyError {
    /// 文件 I/O 错误
    #[error("file I/O error: {0}")]
    Io(String),
    /// CSV/TEXT 解析错误
    #[error("parse error at line {line}: {reason}")]
    Parse {
        /// 行号（1-based）
        line: usize,
        /// 错误原因
        reason: String,
    },
    /// 类型转换错误
    #[error("type conversion error at line {line}, column {column}: {reason}")]
    TypeConversion {
        /// 行号（1-based）
        line: usize,
        /// 列号（1-based）
        column: usize,
        /// 错误原因
        reason: String,
    },
    /// 列数不匹配
    #[error("column count mismatch at line {line}: expected {expected}, got {actual}")]
    ColumnCount {
        /// 行号（1-based）
        line: usize,
        /// 期望列数
        expected: usize,
        /// 实际列数
        actual: usize,
    },
    /// 不支持的操作
    #[error("unsupported: {0}")]
    Unsupported(String),
}

// =====================================================================
//  CSV 解析与序列化
// =====================================================================

/// 解析单行 CSV（RFC 4180 子集）
///
/// 规则：
/// - 字段以 `delimiter` 分隔（默认 `,`）
/// - 含 `delimiter`、`quote`、换行符的字段必须用 `quote` 引用
/// - 引用字段内的 `quote` 通过 `escape`（或双写 `quote`）转义
/// - 空字段（非引用）为空字符串
///
/// 不支持：嵌入换行符（PG 支持，但本实现简化为单行解析）
pub fn parse_csv_line(
    line: &str,
    delimiter: char,
    quote: char,
    escape: char,
) -> Result<Vec<String>, CopyError> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quote {
            if ch == escape {
                // 转义字符：下一个字符原样加入
                if let Some(&next) = chars.peek() {
                    // PG 中 escape 与 quote 相同时，双写 quote 表示一个 quote
                    // escape 与 quote 不同时，escape + 任意字符 = 任意字符
                    if next == quote && escape == quote {
                        chars.next();
                        current.push(quote);
                    } else if escape != quote {
                        chars.next();
                        current.push(next);
                    } else {
                        // escape == quote 但 next != quote → 结束引用
                        in_quote = false;
                    }
                } else {
                    // 行末尾的 escape 字符
                    if escape == quote {
                        // escape == quote 且行末尾 → 实际是引用结束（如 "" 表示空字符串字段）
                        in_quote = false;
                    } else {
                        // escape != quote 的行末尾 escape（非法，但容忍）
                        current.push(escape);
                    }
                }
            } else if ch == quote {
                // 引用结束
                in_quote = false;
                // 检查是否是双写 quote（RFC 4180 转义）
                if let Some(&next) = chars.peek() {
                    if next == quote && escape == quote {
                        chars.next();
                        current.push(quote);
                        in_quote = true;
                    }
                }
            } else {
                current.push(ch);
            }
        } else if ch == quote {
            // 字段开始引用（仅在字段开头有效）
            if current.is_empty() {
                in_quote = true;
            } else {
                // 引号出现在非引用字段中间 → 视为字面量
                current.push(ch);
            }
        } else if ch == delimiter {
            fields.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }

    if in_quote {
        return Err(CopyError::Parse {
            line: 0,
            reason: "unterminated quoted field".into(),
        });
    }

    fields.push(current);
    Ok(fields)
}

/// 序列化单行 CSV
///
/// 规则：
/// - 含 `delimiter`、`quote`、换行符的字段用 `quote` 引用
/// - 引用字段内的 `quote` 通过 `escape` + `quote` 转义（若 escape == quote，则双写 quote）
/// - 空字段不引用
pub fn format_csv_field(s: &str, delimiter: char, quote: char, escape: char) -> String {
    let needs_quote = s.is_empty()
        || s.contains(delimiter)
        || s.contains(quote)
        || s.contains('\n')
        || s.contains('\r');

    if !needs_quote {
        return s.to_string();
    }

    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for ch in s.chars() {
        if ch == quote {
            if escape == quote {
                // RFC 4180：双写 quote
                out.push(quote);
                out.push(quote);
            } else {
                out.push(escape);
                out.push(quote);
            }
        } else if ch == escape && escape != quote {
            // escape 字符本身需要转义（仅当 escape != quote 时）
            out.push(escape);
            out.push(escape);
        } else {
            out.push(ch);
        }
    }
    out.push(quote);
    out
}

// =====================================================================
//  TEXT 解析与序列化
// =====================================================================

/// 解析单行 TEXT（PG 默认格式）
///
/// 规则：
/// - 字段以 `delimiter` 分隔（默认 `\t`）
/// - 无引号引用
/// - `\N` 表示 NULL（由上层根据 null_string 判断）
/// - 反斜杠转义：`\\` → `\`，`\t` → TAB，`\n` → 换行，`\r` → 回车
///
/// 简化实现：不处理反斜杠转义（PG TEXT 格式中转义较复杂，本实现按字面解析）
pub fn parse_text_line(line: &str, delimiter: char) -> Vec<String> {
    line.split(delimiter).map(|s| s.to_string()).collect()
}

/// 序列化单行 TEXT
///
/// 规则：
/// - 字段以 `delimiter` 分隔
/// - 无引号引用
/// - NULL 输出为 `\N`（由上层根据 null_string 处理）
pub fn format_text_field(s: &str) -> &str {
    s
}

// =====================================================================
//  Value ↔ String 转换
// =====================================================================

/// 将 `Value` 序列化为字符串（用于 COPY TO）
///
/// - `Value::Null` → `null_string`
/// - `Value::Text(s)` → `s`
/// - `Value::Int64(n)` → `n.to_string()`
/// - `Value::Float64(f)` → `f.to_string()`
/// - `Value::Bool(b)` → `"t"` / `"f"`（PG 风格）
/// - 其他类型 → 使用 `format!("{value:?}")`（简化实现）
pub fn value_to_string(v: &Value, null_string: &str) -> String {
    match v {
        Value::Null => null_string.to_string(),
        Value::Text(s) => s.clone(),
        Value::Int64(n) => n.to_string(),
        Value::Float64(f) => f.to_string(),
        Value::Bool(b) => {
            if *b {
                "t".to_string()
            } else {
                "f".to_string()
            }
        }
        Value::Date(days) => {
            // 自 epoch 的天数 → YYYY-MM-DD
            let secs = i64::from(*days) * 86400;
            format_date_from_epoch_secs(secs)
        }
        Value::Timestamp(us) => {
            // 微秒 → YYYY-MM-DD HH:MM:SS.ffffff
            format_timestamp_from_epoch_us(*us)
        }
        Value::Decimal(unscaled, scale) => format_decimal(*unscaled, *scale),
        Value::Blob(b) => {
            // PG BYTEA hex 格式：`\x` + hex
            format!("\\x{}", hex_encode(b))
        }
        Value::Enum(s) => s.clone(),
        Value::Array(items) => {
            // PG 数组格式：`{elem1,elem2,...}`
            let inner: Vec<String> = items
                .iter()
                .map(|v| value_to_string(v, null_string))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Json(v) => v.to_string(),
        Value::Range(r) => format!("{r:?}"),
        Value::TsVector(tv) => tv.to_pg_string(),
        Value::TsQuery(tq) => tq.to_pg_string(),
        // P4-5: 向量以文本格式输出
        Value::Vector(v) => v.to_string(),

        // SQL/XML: XML 以文本格式输出
        Value::Xml(x) => x.clone(),
    }
}

/// 将字符串转换为 `Value`（用于 COPY FROM）
///
/// - 若 `s == null_string`，返回 `Value::Null`
/// - 否则根据 `target_type` 转换：
///   - `Text` → `Value::Text(s)`
///   - `Int64` → 解析为 i64
///   - `Float64` → 解析为 f64
///   - `Bool` → `"t"/"true"/"1"` → true，`"f"/"false"/"0"` → false
///   - 其他类型 → 先 `Value::Text(s)`，再 `cast_explicit`
pub fn string_to_value(
    s: &str,
    target_type: &ColumnType,
    null_string: &str,
) -> Result<Value, CastError> {
    // NULL 判断
    // 注意：当 null_string 为空字符串（CSV 默认）时，空字段也视为 NULL。
    // 这是 PG COPY CSV 的标准行为（除非使用 FORCE_NOT_NULL，本实现暂不支持）。
    if s == null_string {
        return Ok(Value::Null);
    }

    match target_type {
        ColumnType::Text => Ok(Value::Text(s.to_string())),
        ColumnType::Int64 => {
            s.parse::<i64>()
                .map(Value::Int64)
                .map_err(|_| CastError::Impossible {
                    reason: format!("cannot parse '{s}' as Int64"),
                })
        }
        ColumnType::Float64 => {
            s.parse::<f64>()
                .map(Value::Float64)
                .map_err(|_| CastError::Impossible {
                    reason: format!("cannot parse '{s}' as Float64"),
                })
        }
        ColumnType::Bool => match s.to_lowercase().as_str() {
            "t" | "true" | "1" => Ok(Value::Bool(true)),
            "f" | "false" | "0" => Ok(Value::Bool(false)),
            _ => Err(CastError::Impossible {
                reason: format!("cannot parse '{s}' as Bool"),
            }),
        },
        _ => {
            // 其他类型：先 Text，再 cast_explicit
            Value::Text(s.to_string()).cast_explicit(target_type)
        }
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 将 epoch 秒数格式化为 YYYY-MM-DD
fn format_date_from_epoch_secs(secs: i64) -> String {
    // 简化实现：使用 chrono 风格的手算
    // 注意：PG DATE 范围远超 i32，但本实现用 i32 days 足够覆盖 ±588 万年
    let days = secs.div_euclid(86400);
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// 将 epoch 微秒格式化为 YYYY-MM-DD HH:MM:SS.ffffff
fn format_timestamp_from_epoch_us(us: i64) -> String {
    let secs = us.div_euclid(1_000_000);
    let frac = us.rem_euclid(1_000_000);
    let days = secs.div_euclid(86400);
    let day_secs = secs.rem_euclid(86400);
    let (year, month, day) = days_to_ymd(days);
    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{frac:06}")
}

/// 将自 epoch 起的天数转换为 (year, month, day)
///
/// 使用 Howard Hinnant 的算法：https://howardhinnant.github.io/date_algorithms.html
fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 {
        z
    } else {
        z - 146096
    } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 {
        mp + 3
    } else {
        mp - 9
    } as u32; // [1, 12]
    (
        if m <= 2 {
            y + 1
        } else {
            y
        },
        m,
        d,
    )
}

/// 格式化 DECIMAL(unscaled, scale) 为字符串
///
/// 例如：DECIMAL(12345, 2) → "123.45"，DECIMAL(-12345, 2) → "-123.45"
///
/// 使用 `unsigned_abs()` 同时取整数部分与小数部分的绝对值，避免截断除法 (`/`)
/// 与欧几里得余数 (`rem_euclid`) 混用导致的负数符号不一致问题。
fn format_decimal(unscaled: i128, scale: u8) -> String {
    if scale == 0 {
        return unscaled.to_string();
    }
    let scale_u32 = u32::from(scale);
    let divisor = 10_u128.pow(scale_u32);
    let sign = if unscaled < 0 {
        "-"
    } else {
        ""
    };
    let abs_unscaled = unscaled.unsigned_abs();
    let int_part = abs_unscaled / divisor;
    let frac_part = abs_unscaled % divisor;
    format!(
        "{sign}{}.{frac_part:0>scale$}",
        int_part,
        scale = scale_u32 as usize
    )
}

/// 简单 hex 编码
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- CSV 解析 ---

    #[test]
    fn test_parse_csv_simple() {
        let fields = parse_csv_line("a,b,c", ',', '"', '"').unwrap();
        assert_eq!(fields, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_csv_quoted() {
        let fields = parse_csv_line("\"a,b\",c", ',', '"', '"').unwrap();
        assert_eq!(fields, vec!["a,b", "c"]);
    }

    #[test]
    fn test_parse_csv_escaped_quote_rfc4180() {
        // RFC 4180：双写 quote 表示一个 quote
        let fields = parse_csv_line("\"a\"\"b\",c", ',', '"', '"').unwrap();
        assert_eq!(fields, vec!["a\"b", "c"]);
    }

    #[test]
    fn test_parse_csv_empty_field() {
        let fields = parse_csv_line("a,,c", ',', '"', '"').unwrap();
        assert_eq!(fields, vec!["a", "", "c"]);
    }

    #[test]
    fn test_parse_csv_custom_delimiter() {
        let fields = parse_csv_line("a;b;c", ';', '"', '"').unwrap();
        assert_eq!(fields, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_csv_unterminated_quote() {
        let result = parse_csv_line("\"abc", ',', '"', '"');
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_csv_escape_different_from_quote() {
        // escape = '\\'，quote = '"'
        let fields = parse_csv_line("\"a\\\"b\",c", ',', '"', '\\').unwrap();
        assert_eq!(fields, vec!["a\"b", "c"]);
    }

    // --- CSV 序列化 ---

    #[test]
    fn test_format_csv_simple() {
        let s = format_csv_field("abc", ',', '"', '"');
        assert_eq!(s, "abc");
    }

    #[test]
    fn test_format_csv_with_delimiter() {
        let s = format_csv_field("a,b", ',', '"', '"');
        assert_eq!(s, "\"a,b\"");
    }

    #[test]
    fn test_format_csv_with_quote_rfc4180() {
        let s = format_csv_field("a\"b", ',', '"', '"');
        assert_eq!(s, "\"a\"\"b\"");
    }

    #[test]
    fn test_format_csv_with_newline() {
        let s = format_csv_field("a\nb", ',', '"', '"');
        assert_eq!(s, "\"a\nb\"");
    }

    #[test]
    fn test_format_csv_empty() {
        let s = format_csv_field("", ',', '"', '"');
        assert_eq!(s, "\"\"");
    }

    // --- TEXT 解析与序列化 ---

    #[test]
    fn test_parse_text_default_delimiter() {
        let fields = parse_text_line("a\tb\tc", '\t');
        assert_eq!(fields, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_text_null_string() {
        // TEXT 格式中 \N 是 NULL 字符串，但解析层不处理 NULL，由上层判断
        let fields = parse_text_line("a\t\\N\tc", '\t');
        assert_eq!(fields, vec!["a", "\\N", "c"]);
    }

    #[test]
    fn test_format_text_simple() {
        let s = format_text_field("abc");
        assert_eq!(s, "abc");
    }

    // --- Value ↔ String ---

    #[test]
    fn test_value_to_string_null_text() {
        assert_eq!(value_to_string(&Value::Null, "\\N"), "\\N");
        assert_eq!(value_to_string(&Value::Null, ""), "");
    }

    #[test]
    fn test_value_to_string_int64() {
        assert_eq!(value_to_string(&Value::Int64(42), "\\N"), "42");
        assert_eq!(value_to_string(&Value::Int64(-7), "\\N"), "-7");
    }

    #[test]
    fn test_value_to_string_float64() {
        // 使用 2.71 而非 3.14 以避免 clippy::approx_constant 警告
        assert_eq!(value_to_string(&Value::Float64(2.71), "\\N"), "2.71");
    }

    #[test]
    fn test_value_to_string_bool() {
        assert_eq!(value_to_string(&Value::Bool(true), "\\N"), "t");
        assert_eq!(value_to_string(&Value::Bool(false), "\\N"), "f");
    }

    #[test]
    fn test_value_to_string_text() {
        assert_eq!(
            value_to_string(&Value::Text("hello".into()), "\\N"),
            "hello"
        );
    }

    #[test]
    fn test_value_to_string_decimal() {
        assert_eq!(value_to_string(&Value::Decimal(12345, 2), "\\N"), "123.45");
        assert_eq!(
            value_to_string(&Value::Decimal(-12345, 2), "\\N"),
            "-123.45"
        );
        assert_eq!(value_to_string(&Value::Decimal(100, 0), "\\N"), "100");
    }

    #[test]
    fn test_value_to_string_date() {
        // 1970-01-01 = epoch 0 days
        assert_eq!(value_to_string(&Value::Date(0), "\\N"), "1970-01-01");
        // 1970-01-02 = 1 day
        assert_eq!(value_to_string(&Value::Date(1), "\\N"), "1970-01-02");
        // 1969-12-31 = -1 day
        assert_eq!(value_to_string(&Value::Date(-1), "\\N"), "1969-12-31");
    }

    #[test]
    fn test_value_to_string_timestamp() {
        // epoch 0 us = 1970-01-01 00:00:00.000000
        assert_eq!(
            value_to_string(&Value::Timestamp(0), "\\N"),
            "1970-01-01 00:00:00.000000"
        );
    }

    #[test]
    fn test_string_to_value_null() {
        // TEXT 格式 NULL
        assert_eq!(
            string_to_value("\\N", &ColumnType::Text, "\\N").unwrap(),
            Value::Null
        );
        // CSV 格式 NULL（空字符串）
        assert_eq!(
            string_to_value("", &ColumnType::Text, "").unwrap(),
            Value::Null
        );
    }

    #[test]
    fn test_string_to_value_int64() {
        assert_eq!(
            string_to_value("42", &ColumnType::Int64, "\\N").unwrap(),
            Value::Int64(42)
        );
        assert!(string_to_value("abc", &ColumnType::Int64, "\\N").is_err());
    }

    #[test]
    fn test_string_to_value_float64() {
        // 使用 2.71 而非 3.14 以避免 clippy::approx_constant 警告
        assert_eq!(
            string_to_value("2.71", &ColumnType::Float64, "\\N").unwrap(),
            Value::Float64(2.71)
        );
    }

    #[test]
    fn test_string_to_value_bool() {
        assert_eq!(
            string_to_value("t", &ColumnType::Bool, "\\N").unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            string_to_value("f", &ColumnType::Bool, "\\N").unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            string_to_value("true", &ColumnType::Bool, "\\N").unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_string_to_value_text() {
        assert_eq!(
            string_to_value("hello", &ColumnType::Text, "\\N").unwrap(),
            Value::Text("hello".into())
        );
    }

    #[test]
    fn test_string_to_value_decimal() {
        let result = string_to_value(
            "123.45",
            &ColumnType::Decimal {
                precision: 10,
                scale: 2,
            },
            "\\N",
        )
        .unwrap();
        assert_eq!(result, Value::Decimal(12345, 2));
    }

    // --- days_to_ymd ---

    #[test]
    fn test_days_to_ymd_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn test_days_to_ymd_negative() {
        assert_eq!(days_to_ymd(-1), (1969, 12, 31));
    }

    #[test]
    fn test_days_to_ymd_year_2000() {
        // 2000-01-01 = 10957 days since epoch
        assert_eq!(days_to_ymd(10957), (2000, 1, 1));
    }

    // --- 往返测试 ---

    #[test]
    fn test_csv_roundtrip() {
        let original = vec![
            "hello".to_string(),
            "world".to_string(),
            "with,comma".to_string(),
            "with\"quote".to_string(),
            "with\nnewline".to_string(),
            "".to_string(),
        ];
        let line: Vec<String> = original
            .iter()
            .map(|s| format_csv_field(s, ',', '"', '"'))
            .collect();
        let line_str = line.join(",");
        let parsed = parse_csv_line(&line_str, ',', '"', '"').unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_value_int64_roundtrip() {
        let values = vec![Value::Int64(0), Value::Int64(42), Value::Int64(-7)];
        for v in values {
            let s = value_to_string(&v, "\\N");
            let v2 = string_to_value(&s, &ColumnType::Int64, "\\N").unwrap();
            assert_eq!(v, v2);
        }
    }

    #[test]
    fn test_value_bool_roundtrip() {
        let s_true = value_to_string(&Value::Bool(true), "\\N");
        let s_false = value_to_string(&Value::Bool(false), "\\N");
        assert_eq!(
            string_to_value(&s_true, &ColumnType::Bool, "\\N").unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            string_to_value(&s_false, &ColumnType::Bool, "\\N").unwrap(),
            Value::Bool(false)
        );
    }
}
