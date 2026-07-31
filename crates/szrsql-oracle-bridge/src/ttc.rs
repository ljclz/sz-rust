//! Oracle TTC（Two-Task Common）协议层 — function code 解析与数据类型网络字节编码。
//!
//! TTC 是 Oracle Net 协议栈中的应用层协议，位于 TNS 之上。客户端通过 TTC function code
//! 标识请求类型（如 SQL 执行、游标管理、事务控制等），服务端按 function code 分派处理。
//!
//! # TTC 包结构
//!
//! TTC Data 包负载格式：
//! ```text
//! +-------------------+-------------------+--------------------+--------------------+
//! | Function Code (1B)| Sequence ID (1B)  | Flags (1B)         | Payload ...        |
//! +-------------------+-------------------+--------------------+--------------------+
//! ```
//!
//! # Function Code 列表
//!
//! 常见 function code（参考 Oracle TTC 协议规范）：
//! - `0x01` FAST_FETCH — 快速取数据
//! - `0x03` EXECUTE — 执行 SQL（OCI EXECUTE）
//! - `0x05` FETCH — 从游标取数据
//! - `0x0B` PARSE — 解析 SQL（OCI PARSE）
//! - `0x0E` DEFINE — 定义输出列
//! - `0x10` BIND — 绑定参数
//! - `0x17` SET_PROTOCOL — 设置协议版本
//! - `0x5E` AUTH — 认证
//! - `0x76` CLOSE — 关闭游标
//! - `0x91` BEGIN_TXN — 开始事务
//! - `0x92` COMMIT — 提交事务
//! - `0x93` ROLLBACK — 回滚事务
//!
//! # 数据类型网络字节编码
//!
//! Oracle NUMBER 类型使用 Oracle 内部定点数格式（base-100 尾数 + 指数）：
//! - 首字节为指数位（exponent），后跟 base-100 尾数字节
//! - 终止符 0x00 表示 NULL
//! - 最末字节 0x66（102）用于表示正数尾数结束
//! - 最末字节 0x66 取补表示负数
//!
//! DATE 类型使用 7 字节定点编码：
//! - century, year, month, day, hour, minute, second（每个 +100 偏移）
//! - 例：2024-06-15 10:30:45 → [0xc8, 0xd8, 0x06, 0x0f, 0x0b, 0x1f, 0x2d]
//!   （实际 Oracle 用 century+100=200=0xC8, year+100=216=0xD8, ... hour=10+1=11=0x0B）

use chrono::Datelike;
use szrsql_types::value::Value;
use thiserror::Error;

// =====================================================================
//  Function Code 常量
// =====================================================================

/// TTC function code 枚举。
///
/// 参考 Oracle TTC 协议规范，覆盖 SQL 执行相关的主要 function code。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TtcFunction {
    /// FAST_FETCH — 快速取数据（执行+取数据）
    FastFetch = 0x01,
    /// EXECUTE — 执行 SQL
    Execute = 0x03,
    /// FETCH — 从游标取数据
    Fetch = 0x05,
    /// PARSE — 解析 SQL
    Parse = 0x0B,
    /// DEFINE — 定义输出列
    Define = 0x0E,
    /// BIND — 绑定参数
    Bind = 0x10,
    /// SET_PROTOCOL — 设置协议版本
    SetProtocol = 0x17,
    /// AUTH — 认证
    Auth = 0x5E,
    /// CLOSE — 关闭游标
    Close = 0x76,
    /// BEGIN_TXN — 开始事务
    BeginTxn = 0x91,
    /// COMMIT — 提交事务
    Commit = 0x92,
    /// ROLLBACK — 回滚事务
    Rollback = 0x93,
    /// 未知 function code
    Unknown = 0xFF,
}

impl TtcFunction {
    /// 从字节构造 [`TtcFunction`]，未知值返回 [`TtcFunction::Unknown`]。
    pub fn from_u8(byte: u8) -> Self {
        match byte {
            0x01 => Self::FastFetch,
            0x03 => Self::Execute,
            0x05 => Self::Fetch,
            0x0B => Self::Parse,
            0x0E => Self::Define,
            0x10 => Self::Bind,
            0x17 => Self::SetProtocol,
            0x5E => Self::Auth,
            0x76 => Self::Close,
            0x91 => Self::BeginTxn,
            0x92 => Self::Commit,
            0x93 => Self::Rollback,
            _ => Self::Unknown,
        }
    }

    /// 是否为 SQL 执行类 function（Parse / Execute / FastFetch）。
    pub fn is_sql_exec(&self) -> bool {
        matches!(self, Self::Parse | Self::Execute | Self::FastFetch)
    }

    /// 是否为事务控制类 function（BeginTxn / Commit / Rollback）。
    pub fn is_txn_control(&self) -> bool {
        matches!(self, Self::BeginTxn | Self::Commit | Self::Rollback)
    }
}

// =====================================================================
//  TTC 包解析
// =====================================================================

/// TTC 解析错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TtcError {
    /// 负载过短（不足最小 TTC 头部）
    #[error("TTC payload too short: {got} bytes (need at least {min})")]
    PayloadTooShort {
        /// 实际长度
        got: usize,
        /// 最小需要长度
        min: usize,
    },
    /// 不支持的 function code
    #[error("unsupported TTC function code: 0x{0:02X}")]
    UnsupportedFunction(u8),
    /// NUMBER 编码格式错误
    #[error("invalid Oracle NUMBER encoding: {0}")]
    InvalidNumber(String),
    /// DATE 编码格式错误
    #[error("invalid Oracle DATE encoding: {0}")]
    InvalidDate(String),
}

/// TTC 包头部最小长度（function code + sequence + flags）。
pub const TTC_HEADER_LEN: usize = 3;

/// TTC 包解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtcPacket {
    /// function code
    pub function: TtcFunction,
    /// 序列号
    pub seq_id: u8,
    /// 标志位
    pub flags: u8,
    /// 负载（不含头部）
    pub payload: Vec<u8>,
}

impl TtcPacket {
    /// 从字节切片解析 TTC 包。
    pub fn parse(buf: &[u8]) -> Result<Self, TtcError> {
        if buf.len() < TTC_HEADER_LEN {
            return Err(TtcError::PayloadTooShort {
                got: buf.len(),
                min: TTC_HEADER_LEN,
            });
        }
        let function = TtcFunction::from_u8(buf[0]);
        let seq_id = buf[1];
        let flags = buf[2];
        let payload = buf[TTC_HEADER_LEN..].to_vec();
        Ok(Self {
            function,
            seq_id,
            flags,
            payload,
        })
    }

    /// 编码 TTC 包到字节。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(TTC_HEADER_LEN + self.payload.len());
        buf.push(self.function as u8);
        buf.push(self.seq_id);
        buf.push(self.flags);
        buf.extend_from_slice(&self.payload);
        buf
    }
}

// =====================================================================
//  Oracle NUMBER 网络字节编码
// =====================================================================

/// Oracle NUMBER 内部编码格式：
///
/// - 0x00（1 字节）= NULL
/// - 1 字节指数 + base-100 尾数字节（每字节表示两位十进制数字）
/// - 正数：尾数每字节 +1（避免 0x00）；末字节固定 0x66（102）作为终止符
/// - 负数：尾数每字节取补；末字节固定 0x66 取补 = 0x9A
///
/// # 示例
///
/// ```text
/// 123.45 → 指数=193（3位整数+2位小数），尾数=[124, 46, 102]
/// -123.45 → 指数=62（193取补），尾数=[61, 110, 154]
/// 0 → 单字节 0x80
/// ```
pub fn encode_number(value: i128, scale: i8) -> Vec<u8> {
    // 零特殊处理：Oracle 用 0x80 表示数值 0
    if value == 0 {
        return vec![0x80];
    }

    // 将定点数转换为字符串，分离整数部分和小数部分
    let s = if scale > 0 {
        // 正 scale：value 是 unscaled，需要除以 10^scale
        format_decimal_string(value, scale as u32)
    } else if scale < 0 {
        // 负 scale：value 需要乘以 10^|scale|
        let factor = 10i128.pow((-scale) as u32);
        format!("{}", value * factor)
    } else {
        format!("{}", value)
    };

    encode_number_from_str(&s)
}

/// 将十进制数字字符串编码为 Oracle NUMBER 内部格式。
fn encode_number_from_str(s: &str) -> Vec<u8> {
    let s = s.trim();
    if s.is_empty() || s == "0" {
        return vec![0x80];
    }

    let (negative, digits) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest.to_string())
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest.to_string())
    } else {
        (false, s.to_string())
    };

    // 分离整数部分和小数部分
    let (int_part, frac_part) = if let Some(dot_pos) = digits.find('.') {
        (
            digits[..dot_pos].to_string(),
            digits[dot_pos + 1..].to_string(),
        )
    } else {
        (digits, String::new())
    };

    // 移除前导零，保留至少一位
    let int_trimmed = int_part.trim_start_matches('0');
    let int_digits = if int_trimmed.is_empty() {
        "0"
    } else {
        int_trimmed
    };

    // 拼接所有数字（含整数部分前导零，用于指数计算）
    let mut all_digits = int_digits.to_string();
    all_digits.push_str(&frac_part);

    // 移除末尾零（小数尾部零无意义）
    while all_digits.ends_with('0') && all_digits.len() > 1 {
        all_digits.pop();
    }

    // 计算指数
    // - 整数部分非零：exponent = int_digit_count - 1
    //   例：123(3位) → exp=2；42(2位) → exp=1；1(1位) → exp=0
    // - 纯小数：exponent = -((first_nonzero_pos + 1) / 2)（base-100 指数）
    //   例：0.01 → first_nonzero=2 → exp=-((2+1)/2)=-1
    //       0.001 → first_nonzero=3 → exp=-((3+1)/2)=-2
    let int_digit_count = int_digits.len() as i8;
    let exponent = if int_digit_count > 0 && int_digits != "0" {
        int_digit_count - 1
    } else {
        // 纯小数：在 all_digits（含前导 "0"）中找第一个非零数字位置
        let first_nonzero = all_digits.chars().position(|c| c != '0').unwrap_or(0);
        -(((first_nonzero as i8) + 1) / 2)
    };

    // Oracle 指数字节：正数 = exponent + 193，负数 = 62 - exponent
    // 注：193 超出 i8 范围，需在 i32 上做运算再转 u8
    let exp_byte = if negative {
        (62 - exponent as i32) as u8
    } else {
        (exponent as i32 + 193) as u8
    };

    let mut result = Vec::new();
    result.push(exp_byte as u8);

    // 尾数仅包含有效数字（剥离前导零），用于 base-100 分组
    let mantissa_digits: String = all_digits.chars().skip_while(|c| *c == '0').collect();
    let mantissa_digits = if mantissa_digits.is_empty() {
        "0".to_string()
    } else {
        mantissa_digits
    };

    // Oracle base-100 尾数每两位数字组成一字节，从左侧分组。
    // 当有效数字总位数为奇数时，在左侧补 "0" 使分组对齐：
    //   "1"   → "01"   → [1]        (单字节尾数，对应数值 1)
    //   "123" → "0123" → [1, 23]    (两字节尾数，对应数值 1.23)
    //   "42"  → "42"   → [42]       (偶数位，无需补零)
    let mut grouped = mantissa_digits;
    if grouped.len() % 2 == 1 {
        grouped.insert(0, '0');
    }

    // 尾数字节：每两位数字组成一个 base-100 字节
    let mut digits_iter = grouped.chars().peekable();
    while digits_iter.peek().is_some() {
        let d1 = digits_iter.next().and_then(|c| c.to_digit(10)).unwrap_or(0);
        let d2 = digits_iter.next().and_then(|c| c.to_digit(10));
        let mantissa_byte = if let Some(d2v) = d2 {
            (d1 * 10 + d2v) as u8
        } else {
            (d1 * 10) as u8
        };

        // 正数 +1，负数取补（101 - byte）
        let encoded_byte = if negative {
            101u8.wrapping_sub(mantissa_byte)
        } else {
            mantissa_byte + 1
        };
        result.push(encoded_byte);
    }

    // 终止符：正数 0x66（102），负数 0x9A（102 取补）
    result.push(if negative { 0x9A } else { 0x66 });

    result
}

/// 格式化定点数为十进制字符串。
fn format_decimal_string(unscaled: i128, scale: u32) -> String {
    if scale == 0 {
        return unscaled.to_string();
    }
    let negative = unscaled < 0;
    let abs = unscaled.unsigned_abs();
    let abs_str = abs.to_string();
    let scale = scale as usize;

    let (int_part, frac_part) = if abs_str.len() > scale {
        let split = abs_str.len() - scale;
        (abs_str[..split].to_string(), abs_str[split..].to_string())
    } else {
        (
            "0".to_string(),
            format!("{:0>width$}", abs_str, width = scale),
        )
    };

    if negative {
        format!("-{}.{}", int_part, frac_part)
    } else {
        format!("{}.{}", int_part, frac_part)
    }
}

// =====================================================================
//  Oracle DATE 网络字节编码
// =====================================================================

/// Oracle DATE 编码：7 字节，每个字段加 100 偏移（century/year）或 1 偏移（month/day/hour/min/sec）。
///
/// # 编码格式
///
/// ```text
/// +-----------+--------+-------+-----+------+--------+--------+
/// | century+100 | year+100 | month+1 | day+1 | hour+1 | minute+1 | second+1 |
/// +-----------+--------+-------+-----+------+--------+--------+
/// ```
///
/// 例：2024-06-15 10:30:45
/// - century = 20, +100 = 120 = 0x78
/// - year = 24, +100 = 124 = 0x7C
/// - month = 6, +1 = 7 = 0x07
/// - day = 15, +1 = 16 = 0x10
/// - hour = 10, +1 = 11 = 0x0B
/// - minute = 30, +1 = 31 = 0x1F
/// - second = 45, +1 = 46 = 0x2E
///
/// 编码结果：[0x78, 0x7C, 0x07, 0x10, 0x0B, 0x1F, 0x2E]
pub fn encode_date(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Vec<u8> {
    let century = year.div_euclid(100);
    let year_in_century = year.rem_euclid(100);

    vec![
        (century + 100) as u8,
        (year_in_century + 100) as u8,
        (month + 1) as u8,
        (day + 1) as u8,
        (hour + 1) as u8,
        (minute + 1) as u8,
        (second + 1) as u8,
    ]
}

/// 解码 Oracle DATE 字节为 (year, month, day, hour, minute, second)。
pub fn decode_date(buf: &[u8]) -> Result<(i32, u32, u32, u32, u32, u32), TtcError> {
    if buf.len() < 7 {
        return Err(TtcError::InvalidDate(format!(
            "DATE encoding too short: {} bytes (need 7)",
            buf.len()
        )));
    }
    let century = buf[0] as i32 - 100;
    let year_in_century = buf[1] as i32 - 100;
    let month = buf[2] as u32 - 1;
    let day = buf[3] as u32 - 1;
    let hour = buf[4] as u32 - 1;
    let minute = buf[5] as u32 - 1;
    let second = buf[6] as u32 - 1;

    let year = century * 100 + year_in_century;
    Ok((year, month, day, hour, minute, second))
}

// =====================================================================
//  Value → Oracle 网络字节编码
// =====================================================================

/// 将 SzRSQL `Value` 编码为 Oracle 网络字节序列。
///
/// 返回 `(oracle_type_code, encoded_bytes)`：
/// - `oracle_type_code`：Oracle 内部类型编号（NUMBER=2, DATE=12, VARCHAR2=1, ...）
/// - `encoded_bytes`：Oracle 网络格式的字节流
pub fn encode_value(value: &Value) -> (u8, Vec<u8>) {
    match value {
        Value::Null => (0, vec![0x00]),
        Value::Bool(b) => {
            // Oracle 没有 BOOLEAN 类型，用 NUMBER(1) 表示
            (2, encode_number(if *b { 1 } else { 0 }, 0))
        }
        Value::Int64(n) => (2, encode_number(*n as i128, 0)),
        Value::Float64(f) => {
            // 浮点数转 NUMBER（使用字符串中间格式避免精度损失）
            let s = format!("{}", f);
            (2, encode_number_from_str(&s))
        }
        Value::Text(s) => {
            // VARCHAR2 类型码 = 1，后跟长度前缀 + UTF-8 字节
            let mut buf = Vec::with_capacity(1 + s.len());
            buf.push(s.len() as u8);
            buf.extend_from_slice(s.as_bytes());
            (1, buf)
        }
        Value::Blob(b) => {
            // RAW 类型码 = 23，后跟长度前缀 + 字节
            let mut buf = Vec::with_capacity(1 + b.len());
            buf.push(b.len() as u8);
            buf.extend_from_slice(b);
            (23, buf)
        }
        Value::Date(days) => {
            // SzRSQL Date = days since epoch，转 Oracle DATE 7字节
            let date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                .unwrap()
                .checked_add_signed(chrono::Duration::days(*days as i64))
                .unwrap_or_else(|| chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
                        (12, encode_date(date.year(), date.month(), date.day(), 0, 0, 0))
        }
        Value::Timestamp(micros) => {
            // SzRSQL Timestamp = microseconds since epoch，转 Oracle DATE 7字节（秒精度）
            let secs = micros / 1_000_000;
            let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
                .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap());
            let naive = dt.naive_utc();
            use chrono::{Datelike, Timelike};
            (12, encode_date(
                naive.year(),
                naive.month(),
                naive.day(),
                naive.hour(),
                naive.minute(),
                naive.second(),
            ))
        }
        Value::Decimal(unscaled, scale) => {
            (2, encode_number(*unscaled, *scale as i8))
        }
        Value::Json(v) => {
            // JSON 序列化为字符串，按 VARCHAR2 编码
            let s = serde_json::to_string(v).unwrap_or_default();
            let mut buf = Vec::with_capacity(1 + s.len());
            buf.push(s.len() as u8);
            buf.extend_from_slice(s.as_bytes());
            (1, buf)
        }
        Value::Array(arr) => {
            // 数组序列化为 JSON 字符串
            let json_arr: Vec<&Value> = arr.iter().collect();
            let s = serde_json::to_string(&json_arr).unwrap_or_default();
            let mut buf = Vec::with_capacity(1 + s.len());
            buf.push(s.len() as u8);
            buf.extend_from_slice(s.as_bytes());
            (1, buf)
        }
        Value::Enum(s) => {
            let mut buf = Vec::with_capacity(1 + s.len());
            buf.push(s.len() as u8);
            buf.extend_from_slice(s.as_bytes());
            (1, buf)
        }
        _ => {
            // 其他类型降级为字符串
            let s = format!("{:?}", value);
            let mut buf = Vec::with_capacity(1 + s.len());
            buf.push(s.len() as u8);
            buf.extend_from_slice(s.as_bytes());
            (1, buf)
        }
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------- TtcFunction --------------------

    #[test]
    fn ttc_function_from_u8_known_codes() {
        assert_eq!(TtcFunction::from_u8(0x01), TtcFunction::FastFetch);
        assert_eq!(TtcFunction::from_u8(0x03), TtcFunction::Execute);
        assert_eq!(TtcFunction::from_u8(0x05), TtcFunction::Fetch);
        assert_eq!(TtcFunction::from_u8(0x0B), TtcFunction::Parse);
        assert_eq!(TtcFunction::from_u8(0x92), TtcFunction::Commit);
        assert_eq!(TtcFunction::from_u8(0x93), TtcFunction::Rollback);
    }

    #[test]
    fn ttc_function_unknown_code_returns_unknown() {
        assert_eq!(TtcFunction::from_u8(0xAA), TtcFunction::Unknown);
        assert_eq!(TtcFunction::from_u8(0x00), TtcFunction::Unknown);
    }

    #[test]
    fn ttc_function_is_sql_exec() {
        assert!(TtcFunction::Parse.is_sql_exec());
        assert!(TtcFunction::Execute.is_sql_exec());
        assert!(TtcFunction::FastFetch.is_sql_exec());
        assert!(!TtcFunction::Commit.is_sql_exec());
    }

    #[test]
    fn ttc_function_is_txn_control() {
        assert!(TtcFunction::BeginTxn.is_txn_control());
        assert!(TtcFunction::Commit.is_txn_control());
        assert!(TtcFunction::Rollback.is_txn_control());
        assert!(!TtcFunction::Execute.is_txn_control());
    }

    // -------------------- TtcPacket parse/encode --------------------

    #[test]
    fn ttc_packet_parse_execute() {
        // function=Execute(0x03), seq=1, flags=0, payload="SELECT 1"
        let buf = [0x03, 0x01, 0x00, b'S', b'E', b'L', b'E', b'C', b'T'];
        let pkt = TtcPacket::parse(&buf).unwrap();
        assert_eq!(pkt.function, TtcFunction::Execute);
        assert_eq!(pkt.seq_id, 1);
        assert_eq!(pkt.flags, 0);
        assert_eq!(pkt.payload, b"SELECT");
    }

    #[test]
    fn ttc_packet_parse_too_short() {
        let buf = [0x01, 0x02];
        let result = TtcPacket::parse(&buf);
        assert!(matches!(result, Err(TtcError::PayloadTooShort { got: 2, min: 3 })));
    }

    #[test]
    fn ttc_packet_encode_roundtrip() {
        let pkt = TtcPacket {
            function: TtcFunction::Parse,
            seq_id: 5,
            flags: 0x01,
            payload: b"SELECT 1 FROM dual".to_vec(),
        };
        let encoded = pkt.encode();
        let decoded = TtcPacket::parse(&encoded).unwrap();
        assert_eq!(decoded, pkt);
    }

    // -------------------- Oracle NUMBER 编码 --------------------

    #[test]
    fn encode_number_zero() {
        // 零特殊编码：单字节 0x80
        assert_eq!(encode_number(0, 0), vec![0x80]);
    }

    #[test]
    fn encode_number_one() {
        // 1 → 指数 0+193=193=0xC1，尾数 1+1=2=0x02，终止符 0x66
        let encoded = encode_number(1, 0);
        assert_eq!(encoded[0], 0xC1); // 指数
        assert_eq!(encoded[1], 0x02); // 尾数（1+1=2）
        assert_eq!(encoded[2], 0x66); // 终止符
    }

    #[test]
    fn encode_number_positive_integer() {
        // 123 → 3位整数，指数 = 2+193 = 195 = 0xC3
        // 尾数 base-100: 1,23 → 字节 [2, 24] (1+1=2, 23+1=24)
        let encoded = encode_number(123, 0);
        assert_eq!(encoded[0], 0xC3);
        assert_eq!(encoded[1], 0x02); // 1+1
        assert_eq!(encoded[2], 0x18); // 23+1=24
        assert_eq!(encoded[3], 0x66); // 终止符
    }

    #[test]
    fn encode_number_negative() {
        // -123 → 指数 = 62-2 = 60 = 0x3C
        // 尾数取补：[101-1=100, 101-23=78]，终止符 0x9A
        let encoded = encode_number(-123, 0);
        assert_eq!(encoded[0], 0x3C);
        assert_eq!(encoded[1], 100); // 101-1
        assert_eq!(encoded[2], 78); // 101-23
        assert_eq!(encoded[3], 0x9A); // 终止符
    }

    // -------------------- Oracle DATE 编码 --------------------

    #[test]
    fn encode_date_2024_06_15() {
        // 2024-06-15 10:30:45
        let encoded = encode_date(2024, 6, 15, 10, 30, 45);
        assert_eq!(encoded.len(), 7);
        assert_eq!(encoded[0], 120); // century=20, +100=120
        assert_eq!(encoded[1], 124); // year=24, +100=124
        assert_eq!(encoded[2], 7);   // month=6, +1=7
        assert_eq!(encoded[3], 16);  // day=15, +1=16
        assert_eq!(encoded[4], 11);  // hour=10, +1=11
        assert_eq!(encoded[5], 31);  // minute=30, +1=31
        assert_eq!(encoded[6], 46);  // second=45, +1=46
    }

    #[test]
    fn decode_date_roundtrip() {
        let original = (2024, 6, 15, 10, 30, 45);
        let encoded = encode_date(original.0, original.1, original.2, original.3, original.4, original.5);
        let decoded = decode_date(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_date_too_short() {
        let buf = [0x78, 0x7C, 0x07];
        let result = decode_date(&buf);
        assert!(matches!(result, Err(TtcError::InvalidDate(_))));
    }

    // -------------------- Value → Oracle 编码 --------------------

    #[test]
    fn encode_value_null() {
        let (type_code, bytes) = encode_value(&Value::Null);
        assert_eq!(type_code, 0);
        assert_eq!(bytes, vec![0x00]);
    }

    #[test]
    fn encode_value_int64() {
        let (type_code, bytes) = encode_value(&Value::Int64(42));
        assert_eq!(type_code, 2); // NUMBER 类型码
        assert!(!bytes.is_empty());
        // 42 为 2 位整数 → 指数 = 2-1 = 1 → exp_byte = 1+193 = 194 = 0xC2
        assert_eq!(bytes[0], 0xC2); // 指数（1+193）
        // 尾数：base-100 字节 = 42，正数 +1 = 43 = 0x2B
        assert_eq!(bytes[1], 0x2B); // 42+1=43
    }

    #[test]
    fn encode_value_text() {
        let (type_code, bytes) = encode_value(&Value::Text("hello".to_string()));
        assert_eq!(type_code, 1); // VARCHAR2 类型码
        assert_eq!(bytes[0], 5); // 长度前缀
        assert_eq!(&bytes[1..], b"hello");
    }

    #[test]
    fn encode_value_bool() {
        let (type_code_true, bytes_true) = encode_value(&Value::Bool(true));
        assert_eq!(type_code_true, 2); // NUMBER
        assert_eq!(bytes_true[0], 0xC1); // 指数
        assert_eq!(bytes_true[1], 0x02); // 1+1=2

        let (type_code_false, bytes_false) = encode_value(&Value::Bool(false));
        assert_eq!(type_code_false, 2);
        // false = 0，编码为 0x80
        assert_eq!(bytes_false, vec![0x80]);
    }

    #[test]
    fn encode_value_date() {
                let days = chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .signed_duration_since(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
            .num_days() as i32;
        let (type_code, bytes) = encode_value(&Value::Date(days));
        assert_eq!(type_code, 12); // DATE 类型码
        assert_eq!(bytes.len(), 7);
        // century = 20+100 = 120
        assert_eq!(bytes[0], 120); // century=20, +100=120
        assert_eq!(bytes[1], 124); // year=24, +100=124
        assert_eq!(bytes[2], 2);   // month=1, +1=2
        assert_eq!(bytes[3], 2);   // day=1, +1=2
        assert_eq!(bytes[4], 1);   // hour=0, +1=1
        assert_eq!(bytes[5], 1);   // minute=0, +1=1
        assert_eq!(bytes[6], 1);   // second=0, +1=1
    }
}