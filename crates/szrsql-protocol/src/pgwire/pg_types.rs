//! PostgreSQL 类型 OID 与文本格式化。
//!
//! Phase 4.2 — 将 SzRSQL `ColumnType` / `Value` 转换为 pgwire 协议所需的：
//! - 类型 OID（用于 RowDescription）
//! - 类型大小（用于 RowDescription，-1 表示变长）
//! - 文本表示（用于 DataRow 的 text 格式编码）
//!
//! 参考文档：
//! - <https://www.postgresql.org/docs/current/protocol-message-formats.html>
//! - <https://github.com/postgres/postgres/blob/master/src/include/catalog/pg_type.dat>

use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  PG OID 常量
// =====================================================================

/// PostgreSQL 内置类型 OID（来自 pg_type.dat）。
pub mod oid {
    /// BOOL (bool)
    pub const BOOL: u32 = 16;
    /// BYTEA (byte[])
    pub const BYTEA: u32 = 17;
    /// INT8 / BIGINT (i64)
    pub const INT8: u32 = 20;
    /// INT2 / SMALLINT (i16)
    pub const INT2: u32 = 21;
    /// INT4 / INTEGER (i32)
    pub const INT4: u32 = 23;
    /// TEXT (变长字符串)
    pub const TEXT: u32 = 25;
    /// FLOAT8 / DOUBLE PRECISION (f64)
    pub const FLOAT8: u32 = 701;
    /// VARCHAR (变长字符串，等同于 TEXT)
    pub const VARCHAR: u32 = 1043;
    /// DATE (i32 天数偏移)
    pub const DATE: u32 = 1082;
    /// TIMESTAMP (i64 微秒)
    pub const TIMESTAMP: u32 = 1114;
    /// NUMERIC (变长十进制)
    pub const NUMERIC: u32 = 1700;
    /// JSON
    pub const JSON: u32 = 114;
    /// JSONB
    pub const JSONB: u32 = 3802;
    ///任何数组类型的基类占位（实际数组有独立 OID）
    pub const ANY_ARRAY: u32 = 2277;
    /// TSVECTOR
    pub const TSVECTOR: u32 = 3614;
    /// TSQUERY
    pub const TSQUERY: u32 = 3615;
    /// UNKNOWN（未知类型，pgwire 中常用作 fallback）
    pub const UNKNOWN: u32 = 705;
}

// =====================================================================
//  ColumnType → PG OID / Type Size
// =====================================================================

/// 将 `ColumnType` 映射为 PG 类型 OID。
///
/// 对于未知/复杂类型（Range / Array / Enum / Custom），返回 UNKNOWN (705)，
/// 与 PostgreSQL 在缺省类型信息时的行为一致。
pub fn column_type_oid(ct: &ColumnType) -> u32 {
    match ct {
        ColumnType::Null => oid::UNKNOWN,
        ColumnType::Int64 => oid::INT8,
        ColumnType::Float64 => oid::FLOAT8,
        ColumnType::Text => oid::TEXT,
        ColumnType::Blob => oid::BYTEA,
        ColumnType::Bool => oid::BOOL,
        ColumnType::Date => oid::DATE,
        ColumnType::Timestamp => oid::TIMESTAMP,
        // PG 的 NUMERIC OID 用于所有 DECIMAL(p,s)
        ColumnType::Decimal { .. } => oid::NUMERIC,
        ColumnType::Json => oid::JSONB,
        ColumnType::TsVector => oid::TSVECTOR,
        ColumnType::TsQuery => oid::TSQUERY,
        ColumnType::Enum(_) => oid::TEXT,
        ColumnType::Array(_) => oid::ANY_ARRAY,
        ColumnType::Range(_) => oid::UNKNOWN,
    }
}

/// 返回 PG RowDescription 中的 type_size 字段。
///
/// - 固定长度类型返回字节数（如 INT8 = 8）
/// - 变长类型返回 -1（PG 协议约定）
pub fn column_type_size(ct: &ColumnType) -> i16 {
    match ct {
        ColumnType::Int64 => 8,
        ColumnType::Float64 => 8,
        ColumnType::Bool => 1,
        ColumnType::Date => 4,
        ColumnType::Timestamp => 8,
        // 变长类型
        ColumnType::Null
        | ColumnType::Text
        | ColumnType::Blob
        | ColumnType::Decimal { .. }
        | ColumnType::Array(_)
        | ColumnType::Enum(_)
        | ColumnType::Range(_)
        | ColumnType::Json
        | ColumnType::TsVector
        | ColumnType::TsQuery => -1,
    }
}

// =====================================================================
//  Value → 文本表示
// =====================================================================

/// 将 `Value` 转换为 pgwire 文本格式字符串。
///
/// - `Null` → `None`（协议层在 DataRow 中以 length=-1 表示 NULL）
/// - 其他 → `Some(text)`
///
/// 文本格式遵循 PostgreSQL 默认 text 格式（非 binary）。
pub fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,

        Value::Int64(i) => Some(i.to_string()),

        Value::Float64(f) => Some(format_float(*f)),

        Value::Text(s) => Some(s.clone()),

        Value::Bool(b) => Some(if *b {
            "t".into()
        } else {
            "f".into()
        }),

        Value::Date(days) => Some(format_date(*days)),

        Value::Timestamp(us) => Some(format_timestamp(*us)),

        Value::Decimal(unscaled, scale) => Some(format_decimal(*unscaled, *scale)),

        Value::Blob(bytes) => Some(format!("\\x{}", hex(bytes))),

        Value::Enum(s) => Some(s.clone()),

        Value::Array(items) => Some(format_array(items)),

        Value::Json(json) => Some(json.to_string()),

        Value::TsVector(tv) => Some(tv.to_pg_string()),

        Value::TsQuery(tq) => Some(tq.to_pg_string()),

        Value::Range(r) => Some(format_range(r)),
    }
}

// =====================================================================
//  Value → 二进制表示（Phase 4.9 协议兼容性）
// =====================================================================

/// PG 基准日期：2000-01-01 距 1970-01-01 的天数。
///
/// PG 的 DATE / TIMESTAMP 二进制格式以 2000-01-01 为基准，
/// SzRSQL 内部以 1970-01-01 为基准，需要转换。
const PG_EPOCH_DAYS_FROM_UNIX: i32 = 10_957;

/// PG 基准时间戳：2000-01-01 00:00:00 UTC 距 1970-01-01 的微秒数。
const PG_EPOCH_MICROS_FROM_UNIX: i64 = 946_684_800_000_000;

/// 将 `Value` 转换为 pgwire 二进制格式字节。
///
/// - `Null` → `None`（协议层在 DataRow 中以 length=-1 表示 NULL）
/// - 简单类型（Int64/Float64/Bool/Text/Blob/Enum/Date/Timestamp/Json）→ `Some(bytes)`
/// - 复杂类型（Decimal/Array/TsVector/TsQuery/Range）→ `None`（不支持二进制，调用方应回退到文本）
///
/// 二进制格式遵循 PostgreSQL 协议规范：
/// - <https://www.postgresql.org/docs/current/protocol-binary-format.html>
pub fn value_to_binary(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Null => None,

        Value::Int64(i) => Some(i.to_be_bytes().to_vec()),

        Value::Float64(f) => Some(f.to_be_bytes().to_vec()),

        Value::Bool(b) => Some(vec![if *b {
            1
        } else {
            0
        }]),

        Value::Text(s) => Some(s.as_bytes().to_vec()),

        Value::Blob(bytes) => Some(bytes.clone()),

        Value::Enum(s) => Some(s.as_bytes().to_vec()),

        Value::Date(days) => {
            // PG DATE: i32 天数（自 2000-01-01）
            let pg_days = days - PG_EPOCH_DAYS_FROM_UNIX;
            Some(pg_days.to_be_bytes().to_vec())
        }

        Value::Timestamp(us) => {
            // PG TIMESTAMP: i64 微秒（自 2000-01-01）
            let pg_us = us - PG_EPOCH_MICROS_FROM_UNIX;
            Some(pg_us.to_be_bytes().to_vec())
        }

        Value::Json(json) => Some(json.to_string().into_bytes()),

        // 复杂类型暂不支持二进制（调用方应回退到文本格式）
        Value::Decimal(_, _)
        | Value::Array(_)
        | Value::TsVector(_)
        | Value::TsQuery(_)
        | Value::Range(_) => None,
    }
}

/// 判断 `ColumnType` 是否支持二进制格式编码。
///
/// 用于 RowDescription 的 format_code 决策：
/// - 客户端请求二进制且本类型支持 → format_code = 1
/// - 否则 → format_code = 0
pub fn column_type_supports_binary(ct: &ColumnType) -> bool {
    matches!(
        ct,
        ColumnType::Int64
            | ColumnType::Float64
            | ColumnType::Bool
            | ColumnType::Text
            | ColumnType::Blob
            | ColumnType::Date
            | ColumnType::Timestamp
            | ColumnType::Enum(_)
            | ColumnType::Json
    )
}

/// 格式化 f64，遵循 PG 风格（无尾随零，特殊值用 PG 表示）。
fn format_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        }
    } else {
        // PG 默认使用 %g 风格输出，但简化为 to_string
        // 注意：Rust 的 f64::to_string 与 PG 略有差异（如 1.0 vs 1）
        // Phase 4.2 暂用 Rust 默认格式化，后续可考虑用 ryu crate 精确对齐
        let s = f.to_string();
        // 处理整数值（1.0 → "1"）
        if s.ends_with(".0") {
            s.trim_end_matches(".0").to_string()
        } else {
            s
        }
    }
}

/// 格式化 DATE：将自 1970-01-01 起的天数转为 YYYY-MM-DD。
fn format_date(days: i32) -> String {
    // 简化实现：直接计算日期，不依赖 chrono
    // PG 的 DATE 也是基于 2000-01-01 的 Julian 计算，但 SzRSQL 用 1970-01-01
    // 这里用 chrono 风格的手算，覆盖 ±588 万年
    let date = days_from_unix_epoch(days);
    format!("{:04}-{:02}-{:02}", date.year, date.month, date.day)
}

/// 格式化 TIMESTAMP：将微秒 UTC 时间戳转为 YYYY-MM-DD HH:MM:SS.ffffff。
fn format_timestamp(us: i64) -> String {
    let secs = us.div_euclid(1_000_000);
    let sub_us = us.rem_euclid(1_000_000);
    let days = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400);
    let hour = (day_secs / 3600) as u32;
    let minute = ((day_secs % 3600) / 60) as u32;
    let second = (day_secs % 60) as u32;
    let date = days_from_unix_epoch(days as i32);
    if sub_us == 0 {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            date.year, date.month, date.day, hour, minute, second
        )
    } else {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
            date.year, date.month, date.day, hour, minute, second, sub_us
        )
    }
}

/// 格式化 DECIMAL：将 (unscaled, scale) 转为 "123.45" 形式。
fn format_decimal(unscaled: i128, scale: u8) -> String {
    if scale == 0 {
        return unscaled.to_string();
    }
    let sign = if unscaled < 0 {
        "-"
    } else {
        ""
    };
    let abs = unscaled.unsigned_abs();
    let abs_str = abs.to_string();
    let scale = scale as usize;
    if abs_str.len() <= scale {
        // 数字位数小于 scale，需要前导零
        let zeros = "0".repeat(scale - abs_str.len());
        format!("{sign}0.{zeros}{abs_str}")
    } else {
        // 在合适位置插入小数点
        let split = abs_str.len() - scale;
        let (int_part, frac_part) = abs_str.split_at(split);
        format!("{sign}{int_part}.{frac_part}")
    }
}

/// 格式化数组：`{1,2,3}` / `{"a","b"}`。
fn format_array(items: &[Value]) -> String {
    let parts: Vec<String> = items
        .iter()
        .map(|v| match v {
            Value::Null => "NULL".into(),
            Value::Text(s) => format!("\"{}\"", s.replace('"', "\\\"")),
            _ => value_to_text(v).unwrap_or_else(|| "NULL".into()),
        })
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// 格式化 range：`[1,10)` / `(1,+∞)`。
fn format_range(r: &szrsql_types::value::RangeValue) -> String {
    let lb = match (&r.lower, r.lower_inc) {
        (Some(v), true) => format!("[{}", value_to_text(v).unwrap_or_default()),
        (Some(v), false) => format!("({}", value_to_text(v).unwrap_or_default()),
        (None, _) => "(".into(),
    };
    let rb = match (&r.upper, r.upper_inc) {
        (Some(v), true) => format!("{}]", value_to_text(v).unwrap_or_default()),
        (Some(v), false) => format!("{})", value_to_text(v).unwrap_or_default()),
        (None, _) => ")".into(),
    };
    format!("{lb},{rb}")
}

/// 十六进制编码（小写）。
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 简单日期结构（无时区）。
struct SimpleDate {
    year: i32,
    month: u32,
    day: u32,
}

/// 将自 1970-01-01 起的天数转换为 (year, month, day)。
///
/// 使用 Howard Hinnant 的算法（<http://howardhinnant.github.io/date_algorithms.html>）。
fn days_from_unix_epoch(days: i32) -> SimpleDate {
    // 将天数转换为 civil calendar
    let z = days as i64 + 719468; // 1970-01-01 是 era 的第 719468 天
    let era = if z >= 0 {
        z
    } else {
        z - 146096
    } / 146097;
    let doe = (z - era * 146097) as u32; // [0, 146097)
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 {
        mp + 3
    } else {
        mp - 9
    }; // [1, 12]
    let year = if m <= 2 {
        y + 1
    } else {
        y
    };
    SimpleDate {
        year,
        month: m,
        day: d,
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_column_type_oid_mapping() {
        assert_eq!(column_type_oid(&ColumnType::Int64), oid::INT8);
        assert_eq!(column_type_oid(&ColumnType::Text), oid::TEXT);
        assert_eq!(column_type_oid(&ColumnType::Bool), oid::BOOL);
        assert_eq!(column_type_oid(&ColumnType::Date), oid::DATE);
        assert_eq!(column_type_oid(&ColumnType::Timestamp), oid::TIMESTAMP);
        assert_eq!(
            column_type_oid(&ColumnType::Decimal {
                precision: 10,
                scale: 2
            }),
            oid::NUMERIC
        );
        assert_eq!(column_type_oid(&ColumnType::Json), oid::JSONB);
        assert_eq!(column_type_oid(&ColumnType::Null), oid::UNKNOWN);
    }

    #[test]
    fn test_column_type_size() {
        assert_eq!(column_type_size(&ColumnType::Int64), 8);
        assert_eq!(column_type_size(&ColumnType::Bool), 1);
        assert_eq!(column_type_size(&ColumnType::Date), 4);
        assert_eq!(column_type_size(&ColumnType::Text), -1);
        assert_eq!(
            column_type_size(&ColumnType::Decimal {
                precision: 10,
                scale: 2
            }),
            -1
        );
    }

    #[test]
    fn test_value_to_text_int64() {
        assert_eq!(value_to_text(&Value::Int64(42)), Some("42".into()));
        assert_eq!(value_to_text(&Value::Int64(-100)), Some("-100".into()));
    }

    #[test]
    fn test_value_to_text_bool() {
        assert_eq!(value_to_text(&Value::Bool(true)), Some("t".into()));
        assert_eq!(value_to_text(&Value::Bool(false)), Some("f".into()));
    }

    #[test]
    fn test_value_to_text_null() {
        assert_eq!(value_to_text(&Value::Null), None);
    }

    #[test]
    fn test_value_to_text_float() {
        assert_eq!(value_to_text(&Value::Float64(2.71)), Some("2.71".into()));
        assert_eq!(value_to_text(&Value::Float64(1.0)), Some("1".into()));
    }

    #[test]
    fn test_value_to_text_text() {
        assert_eq!(
            value_to_text(&Value::Text("hello".into())),
            Some("hello".into())
        );
    }

    #[test]
    fn test_value_to_text_decimal() {
        assert_eq!(
            value_to_text(&Value::Decimal(12345, 2)),
            Some("123.45".into())
        );
        assert_eq!(value_to_text(&Value::Decimal(100, 2)), Some("1.00".into()));
        assert_eq!(value_to_text(&Value::Decimal(42, 0)), Some("42".into()));
        assert_eq!(
            value_to_text(&Value::Decimal(-12345, 2)),
            Some("-123.45".into())
        );
    }

    #[test]
    fn test_value_to_text_date() {
        // 1970-01-01 是 days=0
        assert_eq!(value_to_text(&Value::Date(0)), Some("1970-01-01".into()));
        // 1970-01-02 是 days=1
        assert_eq!(value_to_text(&Value::Date(1)), Some("1970-01-02".into()));
        // 2024-01-01 ≈ 19723 天
        let days = (2024 - 1970) * 365 + 17; // 粗略估计，含闰年
        let text = value_to_text(&Value::Date(days)).unwrap();
        assert!(text.starts_with("202"));
    }

    #[test]
    fn test_value_to_text_blob() {
        let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        assert_eq!(
            value_to_text(&Value::Blob(bytes)),
            Some("\\xdeadbeef".into())
        );
    }

    #[test]
    fn test_value_to_text_array() {
        let arr = vec![Value::Int64(1), Value::Int64(2), Value::Int64(3)];
        assert_eq!(value_to_text(&Value::Array(arr)), Some("{1,2,3}".into()));
    }

    #[test]
    fn test_format_date_known_values() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(31), "1970-02-01");
        assert_eq!(format_date(365), "1971-01-01");
    }

    #[test]
    fn test_format_decimal_scale_zero() {
        assert_eq!(format_decimal(42, 0), "42");
        assert_eq!(format_decimal(-42, 0), "-42");
    }

    #[test]
    fn test_format_decimal_leading_zeros() {
        assert_eq!(format_decimal(5, 3), "0.005");
        assert_eq!(format_decimal(50, 3), "0.050");
        assert_eq!(format_decimal(-5, 3), "-0.005");
    }

    // --- Phase 4.9 二进制格式测试 ---

    #[test]
    fn test_value_to_binary_null() {
        assert_eq!(value_to_binary(&Value::Null), None);
    }

    #[test]
    fn test_value_to_binary_int64() {
        let bytes = value_to_binary(&Value::Int64(42)).unwrap();
        assert_eq!(bytes, vec![0, 0, 0, 0, 0, 0, 0, 42]);
        let bytes = value_to_binary(&Value::Int64(-1)).unwrap();
        assert_eq!(bytes, vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn test_value_to_binary_float64() {
        let bytes = value_to_binary(&Value::Float64(1.0)).unwrap();
        assert_eq!(bytes.len(), 8);
        // 1.0 的 IEEE 754 BE: 0x3FF0000000000000
        assert_eq!(bytes, vec![0x3F, 0xF0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_value_to_binary_bool() {
        assert_eq!(value_to_binary(&Value::Bool(true)), Some(vec![1]));
        assert_eq!(value_to_binary(&Value::Bool(false)), Some(vec![0]));
    }

    #[test]
    fn test_value_to_binary_text() {
        assert_eq!(
            value_to_binary(&Value::Text("hello".into())),
            Some(b"hello".to_vec())
        );
    }

    #[test]
    fn test_value_to_binary_blob() {
        assert_eq!(
            value_to_binary(&Value::Blob(vec![0xDE, 0xAD])),
            Some(vec![0xDE, 0xAD])
        );
    }

    #[test]
    fn test_value_to_binary_date() {
        // 1970-01-01 (days=0) → PG days = -10957
        let bytes = value_to_binary(&Value::Date(0)).unwrap();
        assert_eq!(bytes.len(), 4);
        let pg_days = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(pg_days, -10_957);
        // 2000-01-01 (days=10957) → PG days = 0
        let bytes = value_to_binary(&Value::Date(10_957)).unwrap();
        let pg_days = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(pg_days, 0);
    }

    #[test]
    fn test_value_to_binary_timestamp() {
        // 1970-01-01 (us=0) → PG us = -946684800000000
        let bytes = value_to_binary(&Value::Timestamp(0)).unwrap();
        assert_eq!(bytes.len(), 8);
        let pg_us = i64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        assert_eq!(pg_us, -946_684_800_000_000);
    }

    #[test]
    fn test_value_to_binary_decimal_unsupported() {
        // Decimal 不支持二进制，返回 None（调用方回退到文本）
        assert_eq!(value_to_binary(&Value::Decimal(12345, 2)), None);
    }

    #[test]
    fn test_column_type_supports_binary() {
        assert!(column_type_supports_binary(&ColumnType::Int64));
        assert!(column_type_supports_binary(&ColumnType::Float64));
        assert!(column_type_supports_binary(&ColumnType::Bool));
        assert!(column_type_supports_binary(&ColumnType::Text));
        assert!(column_type_supports_binary(&ColumnType::Blob));
        assert!(column_type_supports_binary(&ColumnType::Date));
        assert!(column_type_supports_binary(&ColumnType::Timestamp));
        assert!(column_type_supports_binary(&ColumnType::Json));
        assert!(!column_type_supports_binary(&ColumnType::Decimal {
            precision: 10,
            scale: 2
        }));
        assert!(!column_type_supports_binary(&ColumnType::Array(Box::new(
            ColumnType::Int64
        ))));
    }
}
