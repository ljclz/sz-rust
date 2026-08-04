//! Oracle 类型系统映射。
//!
//! 本模块定义 Oracle 的内置数据类型枚举 [`OracleType`]，并提供
//! 与 SzRSQL [`Value`] 之间的双向转换能力。
//!
//! # Oracle 类型系统
//!
//! Oracle 的核心内置类型与 SzRSQL 的对应关系如下：
//!
//! | Oracle 类型 | 数字精度 | SzRSQL 类型 |
//! |-------------|----------|-------------|
//! | NUMBER(p, s) | p=1..=38, s=-84..=127 | Decimal / Int64 / Float64 |
//! | VARCHAR2(n [BYTE\|CHAR]) | n=1..=32767 | Text |
//! | CHAR(n [BYTE\|CHAR]) | n=1..=2000 | Text |
//! | DATE | 含日期+时间（秒精度） | Timestamp（秒精度） |
//! | TIMESTAMP(p) | p=0..=9（小数秒精度） | Timestamp |
//! | CLOB | 大文本 | Text |
//! | BLOB | 大二进制 | Blob |
//! | RAW(n) | n=1..=2000 | Blob |
//!
//! # 设计要点
//!
//! - **NUMBER 精度范围**：Oracle NUMBER 类型精度 1..=38，刻度 -84..=127，
//!   构造时严格校验
//! - **字符语义**：VARCHAR2/CHAR 支持 BYTE/CHAR 两种长度语义，仅作记录，
//!   不影响 SzRSQL 内部表示（SzRSQL 统一按 UTF-8 字符处理）
//! - **DATE 含时间**：Oracle DATE 含日期+时间（秒精度），与 PG DATE 不同；
//!   映射到 SzRSQL Timestamp 以保留时间信息
//! - **TIMESTAMP 精度**：小数秒精度 0..=9，SzRSQL Timestamp 为微秒精度（6 位）

use szrsql_types::value::Value;

// =====================================================================
//  错误类型
// =====================================================================

/// Oracle 类型转换错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OracleTypeError {
    /// NUMBER 精度超出范围（合法范围 1..=38）。
    #[error("NUMBER precision out of range: {precision} (must be 1..=38)")]
    NumberPrecisionOutOfRange {
        /// 实际精度
        precision: u32,
    },
    /// NUMBER 刻度超出范围（合法范围 -84..=127）。
    #[error("NUMBER scale out of range: {scale} (must be -84..=127)")]
    NumberScaleOutOfRange {
        /// 实际刻度
        scale: i32,
    },
    /// TIMESTAMP 精度超出范围（合法范围 0..=9）。
    #[error("TIMESTAMP precision out of range: {precision} (must be 0..=9)")]
    TimestampPrecisionOutOfRange {
        /// 实际精度
        precision: u32,
    },
    /// VARCHAR2/CHAR 长度为零。
    #[error("character length must be positive: {size}")]
    ZeroLength {
        /// 实际长度
        size: u32,
    },
    /// RAW 长度为零。
    #[error("RAW length must be positive: {size}")]
    RawZeroLength {
        /// 实际长度
        size: u32,
    },
    /// 文本解析为数值失败。
    #[error("parse number failed: {input}")]
    ParseNumberFailed {
        /// 输入文本
        input: String,
    },
    /// 文本解析为日期失败。
    #[error("parse date failed: {input} (expected YYYY-MM-DD or YYYY-MM-DD HH:MM:SS)")]
    ParseDateFailed {
        /// 输入文本
        input: String,
    },
    /// 十六进制解析失败。
    #[error("parse hex failed: {input}")]
    ParseHexFailed {
        /// 输入文本
        input: String,
    },
    /// 不支持的 Oracle 类型与 Value 组合。
    #[error("unsupported conversion: oracle_type={oracle_type:?}, value={value:?}")]
    UnsupportedConversion {
        /// Oracle 类型
        oracle_type: String,
        /// SzRSQL 值
        value: String,
    },
}

// =====================================================================
//  Oracle 类型枚举
// =====================================================================

/// Oracle 内置数据类型枚举。
///
/// 覆盖 Oracle 最常用的 8 种内置类型，与 [`Value`] 的映射遵循
/// "最小损失"原则：NUMBER → Decimal/Int64/Float64，DATE/TIMESTAMP → Timestamp，
/// VARCHAR2/CHAR/CLOB → Text，BLOB/RAW → Blob。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OracleType {
    /// NUMBER(precision, scale) — Oracle 定点数类型
    ///
    /// - precision: 1..=38（总位数）
    /// - scale: -84..=127（小数位数；正数=小数点右位数，负数=向左舍入位）
    Number {
        /// 总位数（1..=38）
        precision: u8,
        /// 刻度（-84..=127）
        scale: i8,
    },
    /// VARCHAR2(n [BYTE|CHAR]) — 变长字符串
    Varchar2 {
        /// 最大长度
        size: u32,
        /// true=CHAR 语义，false=BYTE 语义
        char_semantics: bool,
    },
    /// CHAR(n [BYTE|CHAR]) — 定长字符串
    Char {
        /// 最大长度
        size: u32,
        /// true=CHAR 语义，false=BYTE 语义
        char_semantics: bool,
    },
    /// DATE — 日期+时间（秒精度，无时区）
    Date,
    /// TIMESTAMP(precision) — 日期+时间+小数秒
    Timestamp {
        /// 小数秒精度（0..=9）
        precision: u8,
    },
    /// CLOB — 大文本（字符大对象）
    Clob,
    /// BLOB — 大二进制（二进制大对象）
    Blob,
    /// RAW(size) — 定长二进制
    Raw {
        /// 最大长度（1..=2000）
        size: u32,
    },
}

impl OracleType {
    // -----------------------------------------------------------------
    //  常用构造函数
    // -----------------------------------------------------------------

    /// 构造默认 NUMBER 类型（precision=38, scale=0）。
    pub fn number_default() -> Self {
        Self::Number {
            precision: 38,
            scale: 0,
        }
    }

    /// 构造指定精度与刻度的 NUMBER 类型，校验范围。
    ///
    /// 注意：scale 上界 127 由 i8 类型本身保证，无需运行时校验；
    /// 仅需校验下界 -84（Oracle NUMBER 最小刻度）。
    pub fn number(precision: u8, scale: i8) -> Result<Self, OracleTypeError> {
        if !(1..=38).contains(&precision) {
            return Err(OracleTypeError::NumberPrecisionOutOfRange {
                precision: precision as u32,
            });
        }
        // scale 上界 127 由 i8 类型保证；仅需校验下界
        if scale < -84 {
            return Err(OracleTypeError::NumberScaleOutOfRange {
                scale: scale as i32,
            });
        }
        Ok(Self::Number { precision, scale })
    }

    /// 构造 VARCHAR2(size, char_semantics)。
    pub fn varchar2(size: u32, char_semantics: bool) -> Result<Self, OracleTypeError> {
        if size == 0 {
            return Err(OracleTypeError::ZeroLength { size });
        }
        Ok(Self::Varchar2 {
            size,
            char_semantics,
        })
    }

    /// 构造 TIMESTAMP(precision)，校验范围。
    pub fn timestamp(precision: u8) -> Result<Self, OracleTypeError> {
        if precision > 9 {
            return Err(OracleTypeError::TimestampPrecisionOutOfRange {
                precision: precision as u32,
            });
        }
        Ok(Self::Timestamp { precision })
    }

    // -----------------------------------------------------------------
    //  类型元信息
    // -----------------------------------------------------------------

    /// 返回该类型的 Oracle 标准名称字符串。
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Number { .. } => "NUMBER",
            Self::Varchar2 { .. } => "VARCHAR2",
            Self::Char { .. } => "CHAR",
            Self::Date => "DATE",
            Self::Timestamp { .. } => "TIMESTAMP",
            Self::Clob => "CLOB",
            Self::Blob => "BLOB",
            Self::Raw { .. } => "RAW",
        }
    }

    /// 返回带参数的类型声明字符串（用于 DDL 生成）。
    pub fn to_ddl(&self) -> String {
        match self {
            Self::Number { precision, scale } => {
                if *scale == 0 {
                    format!("NUMBER({precision})")
                } else {
                    format!("NUMBER({precision},{scale})")
                }
            }
            Self::Varchar2 {
                size,
                char_semantics,
            } => {
                let unit = if *char_semantics {
                    "CHAR"
                } else {
                    "BYTE"
                };
                format!("VARCHAR2({size} {unit})")
            }
            Self::Char {
                size,
                char_semantics,
            } => {
                let unit = if *char_semantics {
                    "CHAR"
                } else {
                    "BYTE"
                };
                format!("CHAR({size} {unit})")
            }
            Self::Date => "DATE".to_string(),
            Self::Timestamp { precision } => format!("TIMESTAMP({precision})"),
            Self::Clob => "CLOB".to_string(),
            Self::Blob => "BLOB".to_string(),
            Self::Raw { size } => format!("RAW({size})"),
        }
    }

    // -----------------------------------------------------------------
    //  从 SzRSQL Value 推导 Oracle 类型
    // -----------------------------------------------------------------

    /// 从 SzRSQL [`Value`] 推导对应的 Oracle 类型。
    ///
    /// 映射规则：
    /// - Null → NUMBER（默认精度，Oracle 中 NULL 无类型）
    /// - Int64 → NUMBER(38, 0)（Oracle 整数最大精度）
    /// - Float64 → NUMBER(38, 127)（最大刻度以保留精度）
    /// - Decimal(_, scale) → NUMBER(38, scale)
    /// - Bool → NUMBER(1, 0)（Oracle 无原生 BOOLEAN，按 0/1 存储）
    /// - Date → TIMESTAMP(0)（Oracle DATE 仅秒精度，但 SzRSQL Date 含日期，映射为 TIMESTAMP 更安全）
    /// - Timestamp → TIMESTAMP(6)（微秒精度）
    /// - Text / Enum → VARCHAR2(4000 CHAR)（默认最大 4000，CHAR 语义）
    /// - Blob → BLOB
    /// - Array / Range / Json / TsVector / TsQuery → CLOB（序列化为文本存储）
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => Self::number_default(),
            Value::Int64(_) => Self::Number {
                precision: 38,
                scale: 0,
            },
            Value::Float64(_) => Self::Number {
                precision: 38,
                scale: 127,
            },
            Value::Decimal(_, scale) => Self::Number {
                precision: 38,
                scale: *scale as i8,
            },
            Value::Bool(_) => Self::Number {
                precision: 1,
                scale: 0,
            },
            // SzRSQL Date 是天数偏移；Oracle DATE 含日期+时间（秒精度）。
            // 为保留语义映射为 TIMESTAMP(0)，避免时间分量丢失
            Value::Date(_) => Self::Timestamp { precision: 0 },
            Value::Timestamp(_) => Self::Timestamp { precision: 6 },
            Value::Text(_) | Value::Enum(_) => Self::Varchar2 {
                size: 4000,
                char_semantics: true,
            },
            Value::Blob(_) => Self::Blob,
            // 复合类型序列化为文本存入 CLOB
            Value::Array(_)
            | Value::Range(_)
            | Value::Json(_)
            | Value::TsVector(_)
            | Value::TsQuery(_)
            | Value::Vector(_) => Self::Clob,
        }
    }

    // -----------------------------------------------------------------
    //  Oracle 文本表示 → SzRSQL Value
    // -----------------------------------------------------------------

    /// 将 Oracle 文本表示解析为 SzRSQL [`Value`]。
    ///
    /// # 参数
    /// - `raw`：Oracle 类型对应的文本表示（如 NUMBER 字面量 "123.45"、
    ///   DATE 字面量 "2024-01-01 12:00:00"、BLOB 十六进制 "DEADBEEF"）
    ///
    /// # 返回
    /// - `Ok(Value)`：解析成功
    /// - `Err(OracleTypeError)`：解析失败（格式不合法或类型不匹配）
    ///
    /// # 解析规则
    /// - NUMBER: scale=0 → Int64（溢出则 Decimal），scale>0 → Decimal，scale<0 → Int64（已舍入）
    /// - VARCHAR2/CHAR/CLOB: 直接 `Value::Text(raw)`
    /// - DATE: "YYYY-MM-DD" 或 "YYYY-MM-DD HH:MM:SS" → Value::Timestamp（微秒）
    /// - TIMESTAMP: 同 DATE
    /// - BLOB/RAW: 十六进制字符串 → Value::Blob
    pub fn to_value(self, raw: &str) -> Result<Value, OracleTypeError> {
        match self {
            Self::Number { scale, .. } => parse_number_to_value(raw, scale),
            Self::Varchar2 { .. } | Self::Char { .. } | Self::Clob => {
                Ok(Value::Text(raw.to_string()))
            }
            Self::Date | Self::Timestamp { .. } => parse_datetime_to_value(raw),
            Self::Blob | Self::Raw { .. } => {
                let bytes = hex_decode(raw)?;
                Ok(Value::Blob(bytes))
            }
        }
    }

    // -----------------------------------------------------------------
    //  SzRSQL Value → Oracle 字面量字符串
    // -----------------------------------------------------------------

    /// 将 SzRSQL [`Value`] 转换为 Oracle 兼容的字面量字符串。
    ///
    /// 用于 `export_to_oracle` 生成 INSERT 语句的 VALUES 子句。
    pub fn value_to_oracle_literal(value: &Value) -> String {
        match value {
            Value::Null => "NULL".to_string(),
            Value::Int64(n) => n.to_string(),
            Value::Float64(f) => format_oracle_float(*f),
            Value::Decimal(unscaled, scale) => format_oracle_decimal(*unscaled, *scale),
            Value::Bool(b) => {
                // Oracle 无原生 BOOLEAN，按 1/0 存储
                if *b {
                    "1".to_string()
                } else {
                    "0".to_string()
                }
            }
            Value::Text(s) | Value::Enum(s) => format_oracle_string(s),
            Value::Blob(b) => {
                // Oracle 十六进制字面量：HEXTORAW('DEADBEEF')
                let hex: String = b.iter().map(|byte| format!("{byte:02X}")).collect();
                format!("HEXTORAW('{hex}')")
            }
            Value::Date(days) => {
                let date_str = days_to_date_string(*days);
                format!("TO_DATE('{date_str}', 'YYYY-MM-DD')")
            }
            Value::Timestamp(us) => {
                let ts_str = micros_to_timestamp_string(*us);
                format!("TO_TIMESTAMP('{ts_str}', 'YYYY-MM-DD HH24:MI:SS.FF6')")
            }
            // 复合类型序列化为 JSON 文本，作为 CLOB 字面量
            Value::Array(_)
            | Value::Range(_)
            | Value::Json(_)
            | Value::TsVector(_)
            | Value::TsQuery(_)
            | Value::Vector(_) => {
                let json = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
                format_oracle_string(&json)
            }
        }
    }
}

impl From<&Value> for OracleType {
    fn from(value: &Value) -> Self {
        Self::from_value(value)
    }
}

// =====================================================================
//  辅助函数：数值/日期/字符串/十六进制
// =====================================================================

/// 将 NUMBER 文本解析为 Value。
///
/// - scale=0 → Int64（若溢出则回退到 Decimal(unscaled, 0)）
/// - scale>0 → Decimal(unscaled, scale)
/// - scale<0 → Int64（按 10^|scale| 舍入）
fn parse_number_to_value(raw: &str, scale: i8) -> Result<Value, OracleTypeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(OracleTypeError::ParseNumberFailed {
            input: raw.to_string(),
        });
    }

    // 处理科学计数法：1.23e4 → 12300
    let normalized = normalize_number_literal(trimmed)?;

    // 解析为 Decimal：将字符串拆分为整数与小数部分
    let (negative, int_part, frac_part) = split_decimal_parts(&normalized)?;

    // 应用 scale：scale<0 时向左舍入，scale>0 时按 scale 截断/补零
    let (unscaled_i128, final_scale_u8) = apply_scale(int_part, frac_part, negative, scale)?;

    if final_scale_u8 == 0 {
        // 整数：尝试用 Int64 表示，溢出则 Decimal
        if let Ok(n) = i64::try_from(unscaled_i128) {
            Ok(Value::Int64(n))
        } else {
            Ok(Value::Decimal(unscaled_i128, 0))
        }
    } else {
        Ok(Value::Decimal(unscaled_i128, final_scale_u8))
    }
}

/// 规范化数字字面量：去除前导 +、处理科学计数法。
fn normalize_number_literal(s: &str) -> Result<String, OracleTypeError> {
    let s = s.trim_start_matches('+');
    if s.is_empty() {
        return Err(OracleTypeError::ParseNumberFailed {
            input: s.to_string(),
        });
    }

    // 处理科学计数法 e/E
    let lower = s.to_lowercase();
    if let Some(e_pos) = lower.find('e') {
        let mantissa = &s[..e_pos];
        let exp_str = &s[e_pos + 1..];
        let exp: i32 = exp_str
            .parse()
            .map_err(|_| OracleTypeError::ParseNumberFailed {
                input: s.to_string(),
            })?;
        // 移除 mantissa 中的小数点，根据 exp 调整
        let (neg, mantissa_abs) = if let Some(rest) = mantissa.strip_prefix('-') {
            (true, rest)
        } else {
            (false, mantissa.trim_start_matches('+'))
        };
        if let Some(dot_pos) = mantissa_abs.find('.') {
            let int_part = &mantissa_abs[..dot_pos];
            let frac_part = &mantissa_abs[dot_pos + 1..];
            let digits: String = format!("{int_part}{frac_part}");
            // 新小数点位置 = 原小数点位置（int_part.len()）+ 指数移动位数（exp）
            // 注意：不应减去 frac_part 长度，因为 digits 已合并整数与小数部分，
            // 小数点相对于 digits 起始的偏移即为 int_part.len() + exp
            let new_dot_pos = int_part.len() as i32 + exp;
            let mut result = String::new();
            if neg {
                result.push('-');
            }
            if new_dot_pos <= 0 {
                result.push_str("0.");
                for _ in 0..(-new_dot_pos) {
                    result.push('0');
                }
                result.push_str(&digits);
            } else if (new_dot_pos as usize) >= digits.len() {
                result.push_str(&digits);
                for _ in 0..(new_dot_pos as usize - digits.len()) {
                    result.push('0');
                }
            } else {
                let pos = new_dot_pos as usize;
                result.push_str(&digits[..pos]);
                result.push('.');
                result.push_str(&digits[pos..]);
            }
            Ok(result)
        } else {
            let mut result = String::new();
            if neg {
                result.push('-');
            }
            result.push_str(mantissa_abs);
            if exp > 0 {
                for _ in 0..exp {
                    result.push('0');
                }
            }
            Ok(result)
        }
    } else {
        Ok(s.to_string())
    }
}

/// 将数字字符串拆分为（负数标志，整数部分数字，小数部分数字）。
fn split_decimal_parts(s: &str) -> Result<(bool, String, String), OracleTypeError> {
    let (negative, abs) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s.trim_start_matches('+'))
    };

    if abs.is_empty() {
        return Err(OracleTypeError::ParseNumberFailed {
            input: s.to_string(),
        });
    }

    if let Some(dot_pos) = abs.find('.') {
        let int_part = &abs[..dot_pos];
        let frac_part = &abs[dot_pos + 1..];
        // 校验：仅含数字
        if !int_part.chars().all(|c| c.is_ascii_digit())
            || !frac_part.chars().all(|c| c.is_ascii_digit())
        {
            return Err(OracleTypeError::ParseNumberFailed {
                input: s.to_string(),
            });
        }
        let int_part = if int_part.is_empty() {
            "0".to_string()
        } else {
            int_part.to_string()
        };
        let frac_part = if frac_part.is_empty() {
            String::new()
        } else {
            frac_part.to_string()
        };
        Ok((negative, int_part, frac_part))
    } else {
        if !abs.chars().all(|c| c.is_ascii_digit()) {
            return Err(OracleTypeError::ParseNumberFailed {
                input: s.to_string(),
            });
        }
        Ok((negative, abs.to_string(), String::new()))
    }
}

/// 应用 Oracle NUMBER 的 scale，返回 (i128 未缩放值, u8 最终刻度)。
///
/// - scale >= 0：保留 scale 位小数；frac_part 不足补零，多余截断
/// - scale < 0：整数向左舍入 |scale| 位
fn apply_scale(
    int_part: String,
    frac_part: String,
    negative: bool,
    scale: i8,
) -> Result<(i128, u8), OracleTypeError> {
    if scale < 0 {
        // 负 scale：向左舍入
        let zeros = (-scale) as usize;
        let mut digits: String = format!("{int_part}{frac_part}");
        // 在 digits 末尾补足 zeros 个零，然后从末尾移除 zeros 个零
        // 实际上：scale=-2 表示数值 = unscaled * 10^2，故 unscaled = digits / 10^2
        // 如果 digits 长度 <= zeros，则 unscaled=0
        if digits.len() <= zeros {
            return Ok((0, 0));
        }
        let cut = digits.len() - zeros;
        let kept: String = digits.drain(..cut).collect();
        let unscaled = parse_i128(&kept, negative)?;
        Ok((unscaled, 0))
    } else {
        let scale_u8 = scale as u8;
        let mut frac = frac_part;
        // 截断或补零到 scale 位
        if frac.len() > scale_u8 as usize {
            frac.truncate(scale_u8 as usize);
        } else {
            while frac.len() < scale_u8 as usize {
                frac.push('0');
            }
        }
        let digits = format!("{int_part}{frac}");
        let unscaled = parse_i128(&digits, negative)?;
        Ok((unscaled, scale_u8))
    }
}

/// 将数字字符串解析为 i128，支持负数。
fn parse_i128(digits: &str, negative: bool) -> Result<i128, OracleTypeError> {
    if digits.is_empty() {
        return Ok(0);
    }
    let abs: i128 = digits
        .parse()
        .map_err(|_| OracleTypeError::ParseNumberFailed {
            input: digits.to_string(),
        })?;
    Ok(if negative {
        -abs
    } else {
        abs
    })
}

/// 解析 Oracle 日期/时间字符串为 Value::Timestamp（微秒精度，UTC）。
///
/// 支持两种格式：
/// - "YYYY-MM-DD"
/// - "YYYY-MM-DD HH:MM:SS" 或 "YYYY-MM-DD HH:MM:SS.FFFFFF"
fn parse_datetime_to_value(raw: &str) -> Result<Value, OracleTypeError> {
    let trimmed = raw.trim();
    let mut parts = trimmed.splitn(2, ' ');

    let date_part = parts
        .next()
        .ok_or_else(|| OracleTypeError::ParseDateFailed {
            input: raw.to_string(),
        })?;
    let time_part = parts.next().unwrap_or("00:00:00");

    // 解析日期 YYYY-MM-DD
    let date_components: Vec<&str> = date_part.split('-').collect();
    if date_components.len() != 3 {
        return Err(OracleTypeError::ParseDateFailed {
            input: raw.to_string(),
        });
    }
    let year: i32 = date_components[0]
        .parse()
        .map_err(|_| OracleTypeError::ParseDateFailed {
            input: raw.to_string(),
        })?;
    let month: u32 = date_components[1]
        .parse()
        .map_err(|_| OracleTypeError::ParseDateFailed {
            input: raw.to_string(),
        })?;
    let day: u32 = date_components[2]
        .parse()
        .map_err(|_| OracleTypeError::ParseDateFailed {
            input: raw.to_string(),
        })?;

    // 解析时间 HH:MM:SS[.FFFFFF]
    let (hms, frac_us) = if let Some(dot_pos) = time_part.find('.') {
        (&time_part[..dot_pos], &time_part[dot_pos + 1..])
    } else {
        (time_part, "")
    };
    let time_components: Vec<&str> = hms.split(':').collect();
    if time_components.len() != 3 {
        return Err(OracleTypeError::ParseDateFailed {
            input: raw.to_string(),
        });
    }
    let hour: u32 = time_components[0]
        .parse()
        .map_err(|_| OracleTypeError::ParseDateFailed {
            input: raw.to_string(),
        })?;
    let minute: u32 = time_components[1]
        .parse()
        .map_err(|_| OracleTypeError::ParseDateFailed {
            input: raw.to_string(),
        })?;
    let second: u32 = time_components[2]
        .parse()
        .map_err(|_| OracleTypeError::ParseDateFailed {
            input: raw.to_string(),
        })?;

    // 计算自 1970-01-01 00:00:00 UTC 起的微秒数
    let days_since_epoch = days_from_civil(year, month, day);
    let mut total_us: i64 = (days_since_epoch as i64) * 86_400_000_000;
    total_us += (hour as i64) * 3_600_000_000;
    total_us += (minute as i64) * 60_000_000;
    total_us += (second as i64) * 1_000_000;
    if !frac_us.is_empty() {
        // 解析小数秒为微秒（截断到 6 位）
        let mut frac_str = frac_us.to_string();
        if frac_str.len() > 6 {
            frac_str.truncate(6);
        }
        while frac_str.len() < 6 {
            frac_str.push('0');
        }
        let us: i64 = frac_str
            .parse()
            .map_err(|_| OracleTypeError::ParseDateFailed {
                input: raw.to_string(),
            })?;
        total_us += us;
    }

    Ok(Value::Timestamp(total_us))
}

/// Howard Hinnant 算法：从公历日期计算自 1970-01-01 起的天数。
///
/// 输入：year (任意整数), month (1..=12), day (1..=31)
/// 输出：days since 1970-01-01（可负）
fn days_from_civil(year: i32, month: u32, day: u32) -> i32 {
    let y = if month <= 2 {
        year - 1
    } else {
        year
    };
    let era = if y >= 0 {
        y
    } else {
        y - 399
    } / 400;
    let yoe = (y - era * 400) as u32; // [0, 399]
    let doy =
        (153 * (if month > 2 {
            month - 3
        } else {
            month + 9
        }) + 2)
            / 5
            + day
            - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    (era as i64 * 146_097 + doe as i64 - 719_468) as i32
}

/// 反向：自 1970-01-01 起的天数 → 公历日期字符串 "YYYY-MM-DD"。
fn days_to_date_string(days: i32) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant 算法：自 1970-01-01 起的天数 → 公历 (year, month, day)。
fn civil_from_days(days: i32) -> (i32, u32, u32) {
    let z = days as i64 + 719_468;
    let era = if z >= 0 {
        z
    } else {
        z - 146_096
    } / 146_097;
    let doe = (z - era * 146_097) as u32; // [0, 146096]
                                          // 注意：必须包含 - doe / 146_096 项，否则 doe=146096（400 年周期最后一天）
                                          // 会得到 yoe=400（超出 [0, 399] 范围），导致 2000-02-29 等日期往返失败
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 {
        mp + 3
    } else {
        mp - 9
    }; // [1, 12]
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

/// 微秒时间戳 → "YYYY-MM-DD HH:MM:SS.FFFFFF"。
fn micros_to_timestamp_string(us: i64) -> String {
    let total_seconds = us.div_euclid(1_000_000);
    let frac_us = us.rem_euclid(1_000_000);
    let days = total_seconds.div_euclid(86_400);
    let secs_in_day = total_seconds.rem_euclid(86_400);
    let hour = (secs_in_day / 3_600) as u32;
    let minute = ((secs_in_day % 3_600) / 60) as u32;
    let second = (secs_in_day % 60) as u32;
    let (year, month, day) = civil_from_days(days as i32);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{frac_us:06}")
}

/// 格式化 Oracle 字符串字面量：单引号转义为两个单引号。
fn format_oracle_string(s: &str) -> String {
    let escaped = s.replace('\'', "''");
    format!("'{escaped}'")
}

/// 格式化 f64 为 Oracle NUMBER 字面量。
///
/// 处理特殊值：NaN → "NULL"（Oracle 不支持 NaN NUMBER），Infinity → 抛错由上层处理。
fn format_oracle_float(f: f64) -> String {
    if f.is_nan() {
        // Oracle NUMBER 不支持 NaN，回退为 NULL
        "NULL".to_string()
    } else if f.is_infinite() {
        // Oracle NUMBER 不支持 Infinity，回退为 NULL
        "NULL".to_string()
    } else {
        // 用 {:?} 保留完整精度
        format!("{f:?}")
    }
}

/// 格式化 Decimal 为 Oracle NUMBER 字面量。
fn format_oracle_decimal(unscaled: i128, scale: u8) -> String {
    if scale == 0 {
        return unscaled.to_string();
    }
    let negative = unscaled < 0;
    let abs = unscaled.unsigned_abs();
    let digits = abs.to_string();
    let scale_us = scale as usize;
    let mut result = String::new();
    if negative {
        result.push('-');
    }
    if digits.len() <= scale_us {
        // 0.00...digits
        result.push_str("0.");
        for _ in 0..(scale_us - digits.len()) {
            result.push('0');
        }
        result.push_str(&digits);
    } else {
        let cut = digits.len() - scale_us;
        result.push_str(&digits[..cut]);
        result.push('.');
        result.push_str(&digits[cut..]);
    }
    result
}

/// 解析十六进制字符串为字节数组。
fn hex_decode(s: &str) -> Result<Vec<u8>, OracleTypeError> {
    let trimmed = s.trim();
    let cleaned = trimmed.trim_start_matches("0x").trim_start_matches("0X");
    if !cleaned.len().is_multiple_of(2) {
        return Err(OracleTypeError::ParseHexFailed {
            input: s.to_string(),
        });
    }
    let mut bytes = Vec::with_capacity(cleaned.len() / 2);
    let chars: Vec<char> = cleaned.chars().collect();
    for i in (0..chars.len()).step_by(2) {
        let high = chars[i]
            .to_digit(16)
            .ok_or_else(|| OracleTypeError::ParseHexFailed {
                input: s.to_string(),
            })?;
        let low = chars[i + 1]
            .to_digit(16)
            .ok_or_else(|| OracleTypeError::ParseHexFailed {
                input: s.to_string(),
            })?;
        bytes.push(((high << 4) | low) as u8);
    }
    Ok(bytes)
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  构造与校验测试
    // -----------------------------------------------------------------

    #[test]
    fn number_constructor_validates_precision_range() {
        // precision 边界：1..=38
        assert!(OracleType::number(1, 0).is_ok());
        assert!(OracleType::number(38, 0).is_ok());
        assert!(OracleType::number(0, 0).is_err());
        assert!(OracleType::number(39, 0).is_err());
    }

    #[test]
    fn number_constructor_validates_scale_range() {
        // scale 边界：-84..=127
        // 上界 127 由 i8 类型本身保证（i8::MAX = 127），无法在运行时传入 > 127 的值；
        // 仅需校验下界 -84
        assert!(OracleType::number(10, -84).is_ok());
        assert!(OracleType::number(10, 127).is_ok());
        assert!(OracleType::number(10, -85).is_err());
    }

    #[test]
    fn timestamp_constructor_validates_precision_range() {
        // precision 边界：0..=9
        assert!(OracleType::timestamp(0).is_ok());
        assert!(OracleType::timestamp(9).is_ok());
        assert!(OracleType::timestamp(10).is_err());
    }

    #[test]
    fn varchar2_rejects_zero_length() {
        assert!(OracleType::varchar2(0, true).is_err());
        assert!(OracleType::varchar2(1, true).is_ok());
        assert!(OracleType::varchar2(4000, false).is_ok());
    }

    // -----------------------------------------------------------------
    //  from_value 测试
    // -----------------------------------------------------------------

    #[test]
    fn from_value_maps_int64_to_number_38_0() {
        let v = Value::Int64(42);
        let t = OracleType::from_value(&v);
        assert_eq!(
            t,
            OracleType::Number {
                precision: 38,
                scale: 0
            }
        );
    }

    #[test]
    fn from_value_maps_text_to_varchar2_4000_char() {
        let v = Value::Text("hello".to_string());
        let t = OracleType::from_value(&v);
        assert_eq!(
            t,
            OracleType::Varchar2 {
                size: 4000,
                char_semantics: true
            }
        );
    }

    #[test]
    fn from_value_maps_blob_to_blob() {
        let v = Value::Blob(vec![0xDE, 0xAD]);
        let t = OracleType::from_value(&v);
        assert_eq!(t, OracleType::Blob);
    }

    #[test]
    fn from_value_maps_timestamp_to_timestamp_6() {
        let v = Value::Timestamp(1_700_000_000_000_000);
        let t = OracleType::from_value(&v);
        assert_eq!(t, OracleType::Timestamp { precision: 6 });
    }

    #[test]
    fn from_value_maps_decimal_with_scale() {
        let v = Value::Decimal(12345, 2);
        let t = OracleType::from_value(&v);
        assert_eq!(
            t,
            OracleType::Number {
                precision: 38,
                scale: 2
            }
        );
    }

    #[test]
    fn from_value_maps_compound_types_to_clob() {
        // Array / Json → CLOB
        let arr = Value::Array(vec![Value::Int64(1), Value::Int64(2)]);
        assert_eq!(OracleType::from_value(&arr), OracleType::Clob);

        let json = Value::Json(serde_json::json!({"key": "value"}));
        assert_eq!(OracleType::from_value(&json), OracleType::Clob);
    }

    // -----------------------------------------------------------------
    //  to_value 测试（Oracle 文本 → SzRSQL Value）
    // -----------------------------------------------------------------

    #[test]
    fn to_value_number_integer_scale_zero() {
        let t = OracleType::number(10, 0).unwrap();
        let v = t.to_value("12345").unwrap();
        assert_eq!(v, Value::Int64(12345));
    }

    #[test]
    fn to_value_number_with_positive_scale() {
        let t = OracleType::number(10, 2).unwrap();
        let v = t.to_value("123.45").unwrap();
        assert_eq!(v, Value::Decimal(12345, 2));
    }

    #[test]
    fn to_value_number_with_negative_scale() {
        // scale=-2：12345 → 12300 的 unscaled=123
        let t = OracleType::number(10, -2).unwrap();
        let v = t.to_value("12345").unwrap();
        assert_eq!(v, Value::Int64(123));
    }

    #[test]
    fn to_value_varchar2_returns_text() {
        let t = OracleType::varchar2(100, true).unwrap();
        let v = t.to_value("hello world").unwrap();
        assert_eq!(v, Value::Text("hello world".to_string()));
    }

    #[test]
    fn to_value_blob_parses_hex() {
        let t = OracleType::Blob;
        let v = t.to_value("DEADBEEF").unwrap();
        assert_eq!(v, Value::Blob(vec![0xDE, 0xAD, 0xBE, 0xEF]));
    }

    #[test]
    fn to_value_date_parses_yyyy_mm_dd() {
        let t = OracleType::Date;
        let v = t.to_value("2024-01-01").unwrap();
        // 2024-01-01 00:00:00 UTC 的微秒数
        // 1970-01-01 至 2024-01-01 = 19723 + 366 闰年调整（实际 19723 天）
        // 用 days_from_civil 验证
        let expected_days = days_from_civil(2024, 1, 1);
        let expected_us = (expected_days as i64) * 86_400_000_000;
        assert_eq!(v, Value::Timestamp(expected_us));
    }

    #[test]
    fn to_value_timestamp_parses_datetime_with_fraction() {
        let t = OracleType::timestamp(6).unwrap();
        let v = t.to_value("2024-01-01 12:30:45.123456").unwrap();
        let expected_days = days_from_civil(2024, 1, 1);
        let mut expected_us = (expected_days as i64) * 86_400_000_000;
        expected_us += 12 * 3_600_000_000;
        expected_us += 30 * 60_000_000;
        expected_us += 45 * 1_000_000;
        expected_us += 123_456;
        assert_eq!(v, Value::Timestamp(expected_us));
    }

    #[test]
    fn to_value_invalid_number_returns_error() {
        let t = OracleType::number(10, 2).unwrap();
        assert!(t.to_value("not-a-number").is_err());
    }

    #[test]
    fn to_value_invalid_hex_returns_error() {
        let t = OracleType::Blob;
        // 奇数长度 hex 应失败
        assert!(t.clone().to_value("ABC").is_err());
        // 非法字符应失败
        assert!(t.to_value("XYZW").is_err());
    }

    // -----------------------------------------------------------------
    //  value_to_oracle_literal 测试
    // -----------------------------------------------------------------

    #[test]
    fn literal_null_is_null_keyword() {
        assert_eq!(OracleType::value_to_oracle_literal(&Value::Null), "NULL");
    }

    #[test]
    fn literal_int64_is_plain_digits() {
        assert_eq!(
            OracleType::value_to_oracle_literal(&Value::Int64(-42)),
            "-42"
        );
    }

    #[test]
    fn literal_text_escapes_single_quotes() {
        let v = Value::Text("it's a test".to_string());
        assert_eq!(OracleType::value_to_oracle_literal(&v), "'it''s a test'");
    }

    #[test]
    fn literal_blob_uses_hextoraw() {
        let v = Value::Blob(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(
            OracleType::value_to_oracle_literal(&v),
            "HEXTORAW('DEADBEEF')"
        );
    }

    #[test]
    fn literal_bool_uses_zero_one() {
        assert_eq!(OracleType::value_to_oracle_literal(&Value::Bool(true)), "1");
        assert_eq!(
            OracleType::value_to_oracle_literal(&Value::Bool(false)),
            "0"
        );
    }

    #[test]
    fn literal_date_uses_to_date() {
        let v = Value::Date(days_from_civil(2024, 1, 1));
        let s = OracleType::value_to_oracle_literal(&v);
        assert!(s.starts_with("TO_DATE('2024-01-01'"));
        assert!(s.contains("YYYY-MM-DD"));
    }

    #[test]
    fn literal_timestamp_uses_to_timestamp() {
        let days = days_from_civil(2024, 1, 1);
        let us = (days as i64) * 86_400_000_000 + 123_456;
        let v = Value::Timestamp(us);
        let s = OracleType::value_to_oracle_literal(&v);
        assert!(s.starts_with("TO_TIMESTAMP('"));
        assert!(s.contains("2024-01-01"));
        assert!(s.contains("YYYY-MM-DD HH24:MI:SS.FF6"));
    }

    #[test]
    fn literal_decimal_preserves_scale() {
        let v = Value::Decimal(12345, 2);
        let s = OracleType::value_to_oracle_literal(&v);
        assert_eq!(s, "123.45");
    }

    // -----------------------------------------------------------------
    //  type_name / to_ddl 测试
    // -----------------------------------------------------------------

    #[test]
    fn type_name_returns_canonical_strings() {
        assert_eq!(
            OracleType::Number {
                precision: 10,
                scale: 2
            }
            .type_name(),
            "NUMBER"
        );
        assert_eq!(OracleType::Date.type_name(), "DATE");
        assert_eq!(OracleType::Blob.type_name(), "BLOB");
        assert_eq!(OracleType::Clob.type_name(), "CLOB");
    }

    #[test]
    fn to_ddl_generates_correct_syntax() {
        assert_eq!(
            OracleType::Number {
                precision: 10,
                scale: 2
            }
            .to_ddl(),
            "NUMBER(10,2)"
        );
        assert_eq!(
            OracleType::Number {
                precision: 10,
                scale: 0
            }
            .to_ddl(),
            "NUMBER(10)"
        );
        assert_eq!(
            OracleType::Varchar2 {
                size: 100,
                char_semantics: true
            }
            .to_ddl(),
            "VARCHAR2(100 CHAR)"
        );
        assert_eq!(
            OracleType::Varchar2 {
                size: 100,
                char_semantics: false
            }
            .to_ddl(),
            "VARCHAR2(100 BYTE)"
        );
        assert_eq!(OracleType::Date.to_ddl(), "DATE");
        assert_eq!(
            OracleType::Timestamp { precision: 6 }.to_ddl(),
            "TIMESTAMP(6)"
        );
        assert_eq!(OracleType::Raw { size: 200 }.to_ddl(), "RAW(200)");
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn days_from_civil_known_dates() {
        // 1970-01-01 → 0
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        // 1970-01-02 → 1
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        // 1969-12-31 → -1
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        // 2024-01-01 → 应为 19723（与 chrono 一致）
        assert_eq!(days_from_civil(2024, 1, 1), 19723);
        // 2000-02-29（闰年）→ 11016
        // 验证：1970-01-01 至 2000-01-01 = 30*365 + 7 个闰年(1972..=1996) = 10957
        // 2000-01-01 至 2000-02-29 = 31(1月) + 28 = 59
        // 合计 = 10957 + 59 = 11016
        assert_eq!(days_from_civil(2000, 2, 29), 11016);
    }

    #[test]
    fn civil_from_days_roundtrip() {
        // 往返测试
        for (y, m, d) in [
            (1970, 1, 1),
            (2024, 1, 1),
            (2000, 2, 29),
            (1969, 12, 31),
            (1999, 12, 31),
        ] {
            let days = days_from_civil(y, m, d);
            let (ry, rm, rd) = civil_from_days(days);
            assert_eq!((ry, rm, rd), (y, m, d), "roundtrip failed for {y}-{m}-{d}");
        }
    }

    #[test]
    fn hex_decode_uppercase_lowercase() {
        assert_eq!(
            hex_decode("DEADBEEF").unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF]
        );
        assert_eq!(
            hex_decode("deadbeef").unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF]
        );
        assert_eq!(
            hex_decode("0xDEADBEEF").unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF]
        );
        assert_eq!(hex_decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn format_oracle_decimal_examples() {
        assert_eq!(format_oracle_decimal(12345, 2), "123.45");
        assert_eq!(format_oracle_decimal(12345, 0), "12345");
        assert_eq!(format_oracle_decimal(-12345, 2), "-123.45");
        assert_eq!(format_oracle_decimal(5, 3), "0.005");
        assert_eq!(format_oracle_decimal(0, 2), "0.00");
    }

    #[test]
    fn normalize_number_literal_scientific_notation() {
        assert_eq!(normalize_number_literal("1.23e4").unwrap(), "12300");
        assert_eq!(normalize_number_literal("1.23e2").unwrap(), "123");
        assert_eq!(normalize_number_literal("1e3").unwrap(), "1000");
        assert_eq!(normalize_number_literal("-1.5e1").unwrap(), "-15");
    }

    #[test]
    fn parse_number_to_value_overflow_falls_back_to_decimal() {
        // i64::MAX + 1 应回退到 Decimal
        let big = format!("{}", (i64::MAX as i128) + 1);
        let t = OracleType::number(38, 0).unwrap();
        let v = t.to_value(&big).unwrap();
        match v {
            Value::Decimal(unscaled, scale) => {
                assert_eq!(scale, 0);
                assert_eq!(unscaled, (i64::MAX as i128) + 1);
            }
            _ => panic!("expected Decimal for overflow, got {v:?}"),
        }
    }
}
