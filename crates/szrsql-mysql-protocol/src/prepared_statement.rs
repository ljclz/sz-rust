//! MySQL Prepared Statement 实现 — COM_STMT_PREPARE / EXECUTE / CLOSE / RESET / SEND_LONG_DATA。
//!
//! 本模块实现 MySQL 二进制协议（Binary Protocol）的完整状态机：
//!
//! - `COM_STMT_PREPARE`：解析 SQL 中的 `?` 占位符，分配 stmt_id，返回参数与列元数据
//! - `COM_STMT_EXECUTE`：解码二进制参数，替换占位符，执行 SQL，以二进制行格式返回结果
//! - `COM_STMT_CLOSE`：释放 prepared statement
//! - `COM_STMT_RESET`：清空 long data 缓冲，重置参数
//! - `COM_STMT_SEND_LONG_DATA`：流式发送 BLOB/TEXT 参数（不立即绑定）
//!
//! ## Binary Protocol Row 格式
//!
//! ```text
//! +----------+------------+--------+--------+---+----------+
//! | header   | NULL bitmap| val_1  | val_2  |...| val_n    |
//! | (1 byte) | (ceil(n/8))|        |        |   |          |
//! +----------+------------+--------+--------+---+----------+
//! ```
//!
//! - header：固定 0x00
//! - NULL bitmap：每位列对应一列（bit 0 = 第 1 列），置位表示该列为 NULL
//! - 非 NULL 列按列类型编码（见 `encode_binary_value`）
//!
//! ## COM_STMT_EXECUTE 参数解码
//!
//! ```text
//! +------------+----------+--------+-------------------+---------------+-----+
//! | stmt_id(4) | flags(1) | iter(4)| NULL bitmap       | new_params(1) | ... |
//! +------------+----------+--------+-------------------+---------------+-----+
//! ```
//!
//! 若 `new_params_bound_flag == 1`，后续跟随每个参数的类型描述与值。

use crate::packet::{read_lenenc_string, write_lenenc_string};
use crate::types::MysqlType;
use chrono::{Datelike, Timelike};
use szrsql_types::value::Value;
use std::collections::HashMap;

/// Prepared Statement ID 类型。
pub type StmtId = u32;

/// 单个 Prepared Statement 的运行时状态。
#[derive(Debug, Clone)]
pub struct PreparedStatement {
    /// 服务端分配的 stmt_id
    pub stmt_id: StmtId,
    /// 原始 SQL（含 `?` 占位符）
    pub sql: String,
    /// 参数数量（`?` 占位符个数）
    pub num_params: u16,
    /// 参数类型绑定（客户端在 COM_STMT_EXECUTE 中可重新指定）
    pub param_types: Vec<(MysqlType, bool)>,
    /// 缓存的 long data（按参数索引），由 COM_STMT_SEND_LONG_DATA 写入
    pub long_data: HashMap<u16, Vec<u8>>,
}

impl PreparedStatement {
    /// 创建新的 prepared statement。
    pub fn new(stmt_id: StmtId, sql: String) -> Self {
        let num_params = count_placeholders(&sql);
        Self {
            stmt_id,
            sql,
            num_params,
            param_types: Vec::new(),
            long_data: HashMap::new(),
        }
    }

    /// 设置参数类型绑定。
    pub fn set_param_types(&mut self, types: Vec<(MysqlType, bool)>) {
        self.param_types = types;
    }

    /// 记录某个参数的 long data（BLOB/TEXT 流式发送）。
    pub fn append_long_data(&mut self, param_idx: u16, data: &[u8]) {
        self.long_data.entry(param_idx).or_default().extend_from_slice(data);
    }

    /// 清空所有 long data（COM_STMT_RESET 调用）。
    pub fn reset(&mut self) {
        self.long_data.clear();
    }
}

/// Prepared Statement 全局存储（按 stmt_id 索引）。
#[derive(Debug, Default)]
pub struct PreparedStatementStore {
    statements: HashMap<StmtId, PreparedStatement>,
    next_stmt_id: StmtId,
}

impl PreparedStatementStore {
    /// 创建空存储。
    pub fn new() -> Self {
        Self {
            statements: HashMap::new(),
            next_stmt_id: 1,
        }
    }

    /// 注册新的 prepared statement，返回分配的 stmt_id。
    pub fn prepare(&mut self, sql: String) -> StmtId {
        let stmt_id = self.next_stmt_id;
        self.next_stmt_id = self.next_stmt_id.wrapping_add(1);
        let stmt = PreparedStatement::new(stmt_id, sql);
        self.statements.insert(stmt_id, stmt);
        stmt_id
    }

    /// 获取 prepared statement。
    pub fn get(&self, stmt_id: StmtId) -> Option<&PreparedStatement> {
        self.statements.get(&stmt_id)
    }

    /// 获取 prepared statement（可变）。
    pub fn get_mut(&mut self, stmt_id: StmtId) -> Option<&mut PreparedStatement> {
        self.statements.get_mut(&stmt_id)
    }

    /// 关闭 prepared statement。
    pub fn close(&mut self, stmt_id: StmtId) -> bool {
        self.statements.remove(&stmt_id).is_some()
    }

    /// 当前活跃 prepared statement 数量。
    pub fn len(&self) -> usize {
        self.statements.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }
}

/// 统计 SQL 中的 `?` 占位符数量。
///
/// 正确处理字符串字面量内的 `?`（不计数）和转义序列。
fn count_placeholders(sql: &str) -> u16 {
    let mut count: u32 = 0;
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\\' if !in_backtick => {
                // 反斜杠转义：跳过下一字节
                i += 2;
                continue;
            }
            b'\'' if !in_double && !in_backtick => {
                in_single = !in_single;
            }
            b'"' if !in_single && !in_backtick => {
                in_double = !in_double;
            }
            b'`' if !in_single && !in_double => {
                in_backtick = !in_backtick;
            }
            b'?' if !in_single && !in_double && !in_backtick => {
                count += 1;
            }
            _ => {}
        }
        i += 1;
    }
    // 防御性截断：实际 SQL 不可能超过 u16::MAX 个参数
    count.min(u16::MAX as u32) as u16
}

/// 将 SQL 中的 `?` 占位符依次替换为参数值（转义为 SQL 字面量）。
///
/// # 参数
/// - `sql`：原始 SQL
/// - `params`：参数值（按 `?` 出现顺序）
///
/// # 返回
/// 替换后的 SQL。若参数数量与占位符数量不符，返回 None。
pub fn substitute_placeholders(sql: &str, params: &[Value]) -> Option<String> {
    let mut result = String::with_capacity(sql.len() + 64);
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut param_idx: usize = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\\' if !in_backtick => {
                result.push(c as char);
                if i + 1 < bytes.len() {
                    result.push(bytes[i + 1] as char);
                    i += 2;
                    continue;
                }
            }
            b'\'' if !in_double && !in_backtick => {
                in_single = !in_single;
                result.push('\'');
            }
            b'"' if !in_single && !in_backtick => {
                in_double = !in_double;
                result.push('"');
            }
            b'`' if !in_single && !in_double => {
                in_backtick = !in_backtick;
                result.push('`');
            }
            b'?' if !in_single && !in_double && !in_backtick => {
                if param_idx >= params.len() {
                    return None;
                }
                result.push_str(&value_to_sql_literal(&params[param_idx]));
                param_idx += 1;
            }
            _ => {
                result.push(c as char);
            }
        }
        i += 1;
    }
    if param_idx != params.len() {
        return None;
    }
    Some(result)
}

/// 将 `Value` 转换为 SQL 字面量（用于占位符替换）。
fn value_to_sql_literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => {
            if *b {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        Value::Int64(n) => n.to_string(),
        Value::Float64(f) => {
            if f.is_nan() || f.is_infinite() {
                "NULL".to_string()
            } else {
                format!("{:.6}", f)
            }
        }
        Value::Text(s) => {
            let escaped = s.replace('\'', "''");
            format!("'{}'", escaped)
        }
        Value::Blob(b) => {
            // 转换为十六进制字面量（MySQL x'...' 语法）
            let hex: String = b.iter().map(|byte| format!("{:02X}", byte)).collect();
            format!("x'{}'", hex)
        }
        Value::Date(days) => {
            let date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                .unwrap()
                .checked_add_signed(chrono::Duration::days(*days as i64))
                .unwrap_or_else(|| chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
            format!("'{}'", date.format("%Y-%m-%d"))
        }
        Value::Timestamp(micros) => {
            let secs = micros / 1_000_000;
            let nano = (micros % 1_000_000) * 1000;
            let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nano as u32)
                .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap());
            format!("'{}'", dt.format("%Y-%m-%d %H:%M:%S"))
        }
        Value::Decimal(unscaled, scale) => format_decimal_literal(*unscaled, *scale),
        Value::Json(v) => {
            let s = serde_json::to_string(v).unwrap_or_else(|_| "null".to_string());
            let escaped = s.replace('\'', "''");
            format!("'{}'", escaped)
        }
        Value::Array(arr) => {
            let json_arr: Vec<&Value> = arr.iter().collect();
            let s = serde_json::to_string(&json_arr).unwrap_or_else(|_| "null".to_string());
            let escaped = s.replace('\'', "''");
            format!("'{}'", escaped)
        }
        Value::Enum(s) => {
            let escaped = s.replace('\'', "''");
            format!("'{}'", escaped)
        }
        Value::Range(_) => "NULL".to_string(),
        Value::TsVector(tv) => {
            let escaped = tv.to_pg_string().replace('\'', "''");
            format!("'{}'", escaped)
        }
        Value::TsQuery(_) => "NULL".to_string(),
    }
}

fn format_decimal_literal(unscaled: i128, scale: u8) -> String {
    if scale == 0 {
        return unscaled.to_string();
    }
    let scale = scale as u32;
    let abs = unscaled.unsigned_abs();
    let abs_str = abs.to_string();
    let int_part_len = abs_str.len().saturating_sub(scale as usize);
    let int_part = &abs_str[..int_part_len];
    let frac_part = &abs_str[int_part_len..];
    let mut result = String::new();
    if unscaled < 0 {
        result.push('-');
    }
    if int_part.is_empty() {
        result.push('0');
    } else {
        result.push_str(int_part);
    }
    result.push('.');
    let frac_padded = format!("{:0>width$}", frac_part, width = scale as usize);
    result.push_str(&frac_padded);
    result
}

// =====================================================================
//  COM_STMT_* 命令解析
// =====================================================================

/// COM_STMT_PREPARE 命令（payload 已去除命令字节）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StmtPrepareCommand {
    pub sql: String,
}

impl StmtPrepareCommand {
    pub fn parse(payload: &[u8]) -> Self {
        let sql = String::from_utf8_lossy(payload).to_string();
        Self {
            sql: sql.trim_end_matches('\0').to_string(),
        }
    }
}

/// COM_STMT_EXECUTE 命令（payload 已去除命令字节）。
#[derive(Debug, Clone)]
pub struct StmtExecuteCommand {
    pub stmt_id: StmtId,
    pub flags: u8,
    pub iteration_count: u32,
    /// 解码后的参数值（按 `?` 出现顺序）
    pub params: Vec<Value>,
    /// 是否绑定了新参数类型
    pub new_params_bound: bool,
}

/// COM_STMT_EXECUTE 的 flags 字段（cursor 类型）。
pub mod cursor_types {
    pub const CURSOR_TYPE_NO_CURSOR: u8 = 0x00;
    pub const CURSOR_TYPE_READ_ONLY: u8 = 0x01;
    pub const CURSOR_TYPE_FOR_UPDATE: u8 = 0x02;
    pub const CURSOR_TYPE_PARAM_INPUT: u8 = 0x04;
    pub const CURSOR_TYPE_PARAM_OUTPUT: u8 = 0x08;
    pub const CURSOR_TYPE_PARAM_INPUT_OUTPUT: u8 = 0x10;
}

impl StmtExecuteCommand {
    /// 解析 COM_STMT_EXECUTE payload。
    ///
    /// 需要传入 prepared statement 的参数数量，用于解析 NULL bitmap。
    pub fn parse(payload: &[u8], num_params: u16, long_data: &HashMap<u16, Vec<u8>>) -> Result<Self, StmtExecError> {
        if payload.len() < 9 {
            return Err(StmtExecError::PayloadTooShort);
        }
        let stmt_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let flags = payload[4];
        let iteration_count = u32::from_le_bytes([payload[5], payload[6], payload[7], payload[8]]);
        let mut buf = &payload[9..];

        if num_params == 0 {
            return Ok(Self {
                stmt_id,
                flags,
                iteration_count,
                params: Vec::new(),
                new_params_bound: false,
            });
        }

        // NULL bitmap：(num_params + 7) / 8 字节
        let bitmap_len = ((num_params as usize) + 7) / 8;
        if buf.len() < bitmap_len {
            return Err(StmtExecError::BitmapTruncated);
        }
        let bitmap = &buf[..bitmap_len];
        buf = &buf[bitmap_len..];

        // new_params_bound_flag
        if buf.is_empty() {
            return Err(StmtExecError::MissingNewParamsFlag);
        }
        let new_params_bound_flag = buf[0];
        buf = &buf[1..];

        let mut params: Vec<Value> = Vec::with_capacity(num_params as usize);
        let mut param_types: Vec<(MysqlType, bool)> = Vec::with_capacity(num_params as usize);

        if new_params_bound_flag == 0x01 {
            // 读取每个参数的类型描述（2 字节：type + unsigned flag）
            for _ in 0..num_params {
                if buf.len() < 2 {
                    return Err(StmtExecError::TypeDescTruncated);
                }
                let type_byte = buf[0];
                let unsigned = buf[1] & 0x80 != 0;
                buf = &buf[2..];
                let mysql_type = mysql_type_from_byte(type_byte, unsigned);
                param_types.push((mysql_type, unsigned));
            }
            // 按类型解码每个参数值
            for idx in 0..num_params {
                let bit = (idx as usize) & 7;
                let byte_idx = (idx as usize) >> 3;
                let is_null = (bitmap[byte_idx] >> bit) & 1 == 1;
                if is_null {
                    params.push(Value::Null);
                    continue;
                }
                // 若有 long data，则用其作为参数值
                if let Some(data) = long_data.get(&idx) {
                    if !data.is_empty() {
                        // 根据参数类型决定是 BLOB 还是 TEXT
                        let (expected_type, _) = param_types.get(idx as usize).copied().unwrap_or((MysqlType::Blob, false));
                        let value = match expected_type {
                            MysqlType::TinyBlob | MysqlType::MediumBlob | MysqlType::LongBlob | MysqlType::Blob => Value::Blob(data.clone()),
                            _ => Value::Text(String::from_utf8_lossy(data).to_string()),
                        };
                        params.push(value);
                        continue;
                    }
                }
                let (mysql_type, unsigned) = param_types[idx as usize];
                let (value, rest) = decode_binary_value(mysql_type, unsigned, buf)?;
                params.push(value);
                buf = rest;
            }
            Ok(Self {
                stmt_id,
                flags,
                iteration_count,
                params,
                new_params_bound: true,
            })
        } else {
            // new_params_bound_flag == 0：使用先前绑定的类型（这里我们无法知道，按 NULL 处理）
            // 实际客户端通常会绑定新参数，此分支主要为了协议完整性。
            for idx in 0..num_params {
                let bit = (idx as usize) & 7;
                let byte_idx = (idx as usize) >> 3;
                let is_null = (bitmap[byte_idx] >> bit) & 1 == 1;
                if is_null {
                    params.push(Value::Null);
                } else if let Some(data) = long_data.get(&idx) {
                    if !data.is_empty() {
                        params.push(Value::Blob(data.clone()));
                    } else {
                        params.push(Value::Null);
                    }
                } else {
                    // 无类型信息，无法解码
                    return Err(StmtExecError::NoTypeBinding);
                }
            }
            Ok(Self {
                stmt_id,
                flags,
                iteration_count,
                params,
                new_params_bound: false,
            })
        }
    }
}

/// COM_STMT_CLOSE 命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StmtCloseCommand {
    pub stmt_id: StmtId,
}

impl StmtCloseCommand {
    pub fn parse(payload: &[u8]) -> Result<Self, StmtExecError> {
        if payload.len() < 4 {
            return Err(StmtExecError::PayloadTooShort);
        }
        Ok(Self {
            stmt_id: u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]),
        })
    }
}

/// COM_STMT_RESET 命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StmtResetCommand {
    pub stmt_id: StmtId,
}

impl StmtResetCommand {
    pub fn parse(payload: &[u8]) -> Result<Self, StmtExecError> {
        if payload.len() < 4 {
            return Err(StmtExecError::PayloadTooShort);
        }
        Ok(Self {
            stmt_id: u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]),
        })
    }
}

/// COM_STMT_SEND_LONG_DATA 命令。
#[derive(Debug, Clone)]
pub struct StmtSendLongDataCommand {
    pub stmt_id: StmtId,
    pub param_id: u16,
    pub data: Vec<u8>,
}

impl StmtSendLongDataCommand {
    pub fn parse(payload: &[u8]) -> Result<Self, StmtExecError> {
        if payload.len() < 6 {
            return Err(StmtExecError::PayloadTooShort);
        }
        let stmt_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let param_id = u16::from_le_bytes([payload[4], payload[5]]);
        let data = payload[6..].to_vec();
        Ok(Self {
            stmt_id,
            param_id,
            data,
        })
    }
}

// =====================================================================
//  二进制协议值编解码
// =====================================================================

/// MySQL 二进制协议值解码错误。
#[derive(Debug, thiserror::Error)]
pub enum StmtExecError {
    #[error("payload too short")]
    PayloadTooShort,
    #[error("NULL bitmap truncated")]
    BitmapTruncated,
    #[error("missing new_params_bound flag")]
    MissingNewParamsFlag,
    #[error("type descriptor truncated")]
    TypeDescTruncated,
    #[error("value truncated for type {0:?}")]
    ValueTruncated(MysqlType),
    #[error("no type binding for parameter")]
    NoTypeBinding,
    #[error("invalid type byte: 0x{0:02X}")]
    InvalidTypeByte(u8),
}

/// 从字节解码 MySQL 类型（参考 mysql_com.h 的 enum_field_types）。
fn mysql_type_from_byte(byte: u8, _unsigned: bool) -> MysqlType {
    match byte {
        0x00 => MysqlType::Decimal,
        0x01 => MysqlType::Tiny,
        0x02 => MysqlType::Short,
        0x03 => MysqlType::Long,
        0x04 => MysqlType::Float,
        0x05 => MysqlType::Double,
        0x06 => MysqlType::Null,
        0x07 => MysqlType::Timestamp,
        0x08 => MysqlType::LongLong,
        0x09 => MysqlType::Int24,
        0x0A => MysqlType::Date,
        0x0B => MysqlType::Time,
        0x0C => MysqlType::DateTime,
        0x0D => MysqlType::Year,
        0x0F => MysqlType::VarString,
        0x10 => MysqlType::Bit,
        0xF5 => MysqlType::Json,
        0xF6 => MysqlType::NewDecimal,
        0xF7 => MysqlType::Enum,
        0xF8 => MysqlType::Set,
        0xF9 => MysqlType::TinyBlob,
        0xFA => MysqlType::MediumBlob,
        0xFB => MysqlType::LongBlob,
        0xFC => MysqlType::Blob,
        0xFD => MysqlType::VarString,
        0xFE => MysqlType::String,
        0xFF => MysqlType::Geometry,
        _ => MysqlType::VarString, // 未知类型降级为字符串
    }
}

/// 解码二进制协议参数值。
///
/// 返回 (Value, 剩余字节切片)。
fn decode_binary_value(
    mysql_type: MysqlType,
    unsigned: bool,
    buf: &[u8],
) -> Result<(Value, &[u8]), StmtExecError> {
    use StmtExecError::*;
    match mysql_type {
        MysqlType::Tiny => {
            if buf.is_empty() {
                return Err(ValueTruncated(mysql_type));
            }
            let v = if unsigned {
                Value::Int64(buf[0] as i64)
            } else {
                Value::Int64(buf[0] as i8 as i64)
            };
            Ok((v, &buf[1..]))
        }
        MysqlType::Short | MysqlType::Year => {
            if buf.len() < 2 {
                return Err(ValueTruncated(mysql_type));
            }
            let v = u16::from_le_bytes([buf[0], buf[1]]);
            let val = if unsigned {
                v as i64
            } else {
                v as i16 as i64
            };
            Ok((Value::Int64(val), &buf[2..]))
        }
        MysqlType::Long | MysqlType::Int24 => {
            if buf.len() < 4 {
                return Err(ValueTruncated(mysql_type));
            }
            let v = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            let val = if unsigned {
                v as i64
            } else {
                v as i32 as i64
            };
            Ok((Value::Int64(val), &buf[4..]))
        }
        MysqlType::LongLong => {
            if buf.len() < 8 {
                return Err(ValueTruncated(mysql_type));
            }
            let v = u64::from_le_bytes([
                buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
            ]);
            let val = if unsigned {
                // u64 超出 i64 范围时降级为 i64::MAX（信息有损但避免 panic）
                if v > i64::MAX as u64 {
                    i64::MAX
                } else {
                    v as i64
                }
            } else {
                v as i64
            };
            Ok((Value::Int64(val), &buf[8..]))
        }
        MysqlType::Float => {
            if buf.len() < 4 {
                return Err(ValueTruncated(mysql_type));
            }
            let v = f32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            Ok((Value::Float64(v as f64), &buf[4..]))
        }
        MysqlType::Double => {
            if buf.len() < 8 {
                return Err(ValueTruncated(mysql_type));
            }
            let v = f64::from_le_bytes([
                buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
            ]);
            Ok((Value::Float64(v), &buf[8..]))
        }
        MysqlType::Date => {
            // 1 字节长度 + 数据（4 字节：年/月/日）
            if buf.is_empty() {
                return Err(ValueTruncated(mysql_type));
            }
            let len = buf[0] as usize;
            if buf.len() < 1 + len {
                return Err(ValueTruncated(mysql_type));
            }
            let data = &buf[1..1 + len];
            if len >= 3 {
                let year = u16::from_le_bytes([data[0], data[1]]);
                let month = data[2];
                let day = data[3.min(len - 1)];
                let date = chrono::NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32)
                    .unwrap_or_else(|| chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
                let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
                let days = date.signed_duration_since(epoch).num_days() as i32;
                Ok((Value::Date(days), &buf[1 + len..]))
            } else {
                Ok((Value::Date(0), &buf[1 + len..]))
            }
        }
        MysqlType::DateTime | MysqlType::Timestamp => {
            // 1 字节长度 + 数据（7 字节：年/月/日/时/分/秒/微秒(可选 4 字节)）
            if buf.is_empty() {
                return Err(ValueTruncated(mysql_type));
            }
            let len = buf[0] as usize;
            if buf.len() < 1 + len {
                return Err(ValueTruncated(mysql_type));
            }
            let data = &buf[1..1 + len];
            if len >= 7 {
                let year = u16::from_le_bytes([data[0], data[1]]);
                let month = data[2] as u32;
                let day = data[3] as u32;
                let hour = data[4] as u32;
                let min = data[5] as u32;
                let sec = data[6] as u32;
                let micros = if len >= 11 {
                    u32::from_le_bytes([data[7], data[8], data[9], data[10]])
                } else {
                    0
                };
                let dt = chrono::NaiveDate::from_ymd_opt(year as i32, month, day)
                    .and_then(|d| d.and_hms_micro_opt(hour, min, sec, micros))
                    .unwrap_or_else(|| {
                        chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                            .unwrap()
                            .and_hms_micro_opt(0, 0, 0, 0)
                            .unwrap()
                    });
                let utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc);
                let micros = utc.timestamp_micros();
                Ok((Value::Timestamp(micros), &buf[1 + len..]))
            } else {
                Ok((Value::Timestamp(0), &buf[1 + len..]))
            }
        }
        MysqlType::Time => {
            // 1 字节长度 + 数据（0/8/12 字节）
            if buf.is_empty() {
                return Err(ValueTruncated(mysql_type));
            }
            let len = buf[0] as usize;
            if buf.len() < 1 + len {
                return Err(ValueTruncated(mysql_type));
            }
            // TIME 类型当前不直接映射到 SzRSQL Value，降级为文本
            let data = &buf[1..1 + len];
            let text = format!("TIME_RAW(len={})", data.len());
            Ok((Value::Text(text), &buf[1 + len..]))
        }
        MysqlType::Null => Ok((Value::Null, buf)),
        MysqlType::VarString
        | MysqlType::String
        | MysqlType::Decimal
        | MysqlType::NewDecimal
        | MysqlType::Enum
        | MysqlType::Set
        | MysqlType::Json
        | MysqlType::Bit => {
            // 长度编码字符串
            let mut slice = buf;
            let data = read_lenenc_string(&mut slice).ok_or(ValueTruncated(mysql_type))?;
            let s = String::from_utf8_lossy(&data).to_string();
            Ok((Value::Text(s), slice))
        }
        MysqlType::TinyBlob
        | MysqlType::MediumBlob
        | MysqlType::LongBlob
        | MysqlType::Blob
        | MysqlType::Geometry => {
            let mut slice = buf;
            let data = read_lenenc_string(&mut slice).ok_or(ValueTruncated(mysql_type))?;
            Ok((Value::Blob(data), slice))
        }
    }
}

/// 编码二进制协议行（用于 COM_STMT_EXECUTE 响应）。
///
/// 行格式：header(0x00) + NULL bitmap + 非 NULL 列值
pub fn encode_binary_row(values: &[Value]) -> Vec<u8> {
    let num_cols = values.len();
    let bitmap_len = (num_cols + 7) / 8;
    let mut buf = Vec::with_capacity(1 + bitmap_len + num_cols * 8);
    // header
    buf.push(0x00);
    // NULL bitmap
    let mut bitmap = vec![0u8; bitmap_len];
    for (idx, v) in values.iter().enumerate() {
        if matches!(v, Value::Null) {
            bitmap[idx >> 3] |= 1 << (idx & 7);
        }
    }
    buf.extend_from_slice(&bitmap);
    // 非 NULL 值
    for v in values.iter() {
        if !matches!(v, Value::Null) {
            encode_binary_value(v, &mut buf);
        }
    }
    buf
}

/// 按列类型编码单个值为二进制协议格式。
fn encode_binary_value(value: &Value, buf: &mut Vec<u8>) {
    match value {
        Value::Null => {} // NULL 已在 bitmap 中标记
        Value::Bool(b) => {
            buf.push(if *b { 1 } else { 0 });
        }
        Value::Int64(n) => {
            // 按值范围选择最小整数类型
            if *n >= i8::MIN as i64 && *n <= i8::MAX as i64 {
                buf.push(*n as u8);
            } else if *n >= i16::MIN as i64 && *n <= i16::MAX as i64 {
                buf.extend_from_slice(&(*n as i16).to_le_bytes());
            } else if *n >= i32::MIN as i64 && *n <= i32::MAX as i64 {
                buf.extend_from_slice(&(*n as i32).to_le_bytes());
            } else {
                buf.extend_from_slice(&n.to_le_bytes());
            }
        }
        Value::Float64(f) => {
            buf.extend_from_slice(&f.to_le_bytes());
        }
        Value::Text(s) => {
            write_lenenc_string(buf, s.as_bytes());
        }
        Value::Blob(b) => {
            write_lenenc_string(buf, b);
        }
        Value::Date(days) => {
            let date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                .unwrap()
                .checked_add_signed(chrono::Duration::days(*days as i64))
                .unwrap_or_else(|| chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
            let year = date.year() as u16;
            let month = date.month() as u8;
            let day = date.day() as u8;
            buf.push(4);
            buf.extend_from_slice(&year.to_le_bytes());
            buf.push(month);
            buf.push(day);
        }
        Value::Timestamp(micros) => {
            let secs = micros / 1_000_000;
            let nano = (micros % 1_000_000) * 1000;
            let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nano as u32)
                .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap());
            let naive = dt.naive_utc();
            let year = naive.year() as u16;
            buf.push(11);
            buf.extend_from_slice(&year.to_le_bytes());
            buf.push(naive.month() as u8);
            buf.push(naive.day() as u8);
            buf.push(naive.hour() as u8);
            buf.push(naive.minute() as u8);
            buf.push(naive.second() as u8);
            let micros = naive.and_utc().timestamp_subsec_micros();
            buf.extend_from_slice(&micros.to_le_bytes());
        }
        Value::Decimal(unscaled, scale) => {
            let s = format_decimal_literal(*unscaled, *scale);
            write_lenenc_string(buf, s.as_bytes());
        }
        Value::Json(v) => {
            let s = serde_json::to_string(v).unwrap_or_default();
            write_lenenc_string(buf, s.as_bytes());
        }
        Value::Array(arr) => {
            let json_arr: Vec<&Value> = arr.iter().collect();
            let s = serde_json::to_string(&json_arr).unwrap_or_default();
            write_lenenc_string(buf, s.as_bytes());
        }
        Value::Enum(s) => {
            write_lenenc_string(buf, s.as_bytes());
        }
        Value::Range(_) => {
            // 降级为 NULL（SzRSQL 不支持 RANGE 类型在二进制协议中传输）
            write_lenenc_string(buf, b"");
        }
        Value::TsVector(tv) => {
            write_lenenc_string(buf, tv.to_pg_string().as_bytes());
        }
        Value::TsQuery(_) => {
            write_lenenc_string(buf, b"");
        }
    }
}

/// COM_STMT_PREPARE 响应包（PREPARE_OK）。
#[derive(Debug, Clone)]
pub struct PrepareOkPacket {
    pub status: u8, // 固定 0x00
    pub stmt_id: u32,
    pub num_columns: u16,
    pub num_params: u16,
    pub reserved: u8,
    pub warning_count: u16,
}

impl PrepareOkPacket {
    /// 构造新的 PREPARE_OK 包。
    pub fn new(stmt_id: u32, num_columns: u16, num_params: u16) -> Self {
        Self {
            status: 0x00,
            stmt_id,
            num_columns,
            num_params,
            reserved: 0x00,
            warning_count: 0,
        }
    }

    /// 编码为 payload。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12);
        buf.push(self.status);
        buf.extend_from_slice(&self.stmt_id.to_le_bytes());
        buf.extend_from_slice(&self.num_columns.to_le_bytes());
        buf.extend_from_slice(&self.num_params.to_le_bytes());
        buf.push(self.reserved);
        buf.extend_from_slice(&self.warning_count.to_le_bytes());
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_placeholders_simple() {
        assert_eq!(count_placeholders("SELECT ? + ?"), 2);
        assert_eq!(count_placeholders("SELECT 1"), 0);
        assert_eq!(count_placeholders("INSERT INTO t VALUES (?, ?, ?)"), 3);
    }

    #[test]
    fn test_count_placeholders_ignores_string_literals() {
        assert_eq!(count_placeholders("SELECT '?' FROM t WHERE x = ?"), 1);
        assert_eq!(count_placeholders("SELECT \"?\""), 0);
        assert_eq!(count_placeholders("SELECT `col?` FROM t"), 0);
    }

    #[test]
    fn test_count_placeholders_handles_escape() {
        assert_eq!(count_placeholders("SELECT '\\\\?' FROM t WHERE x = ?"), 1);
    }

    #[test]
    fn test_substitute_placeholders_basic() {
        let sql = "SELECT ? + ?";
        let params = vec![Value::Int64(10), Value::Int64(20)];
        let result = substitute_placeholders(sql, &params).unwrap();
        assert_eq!(result, "SELECT 10 + 20");
    }

    #[test]
    fn test_substitute_placeholders_string_escape() {
        let sql = "INSERT INTO t VALUES (?)";
        let params = vec![Value::Text("it's a test".to_string())];
        let result = substitute_placeholders(sql, &params).unwrap();
        assert_eq!(result, "INSERT INTO t VALUES ('it''s a test')");
    }

    #[test]
    fn test_substitute_placeholders_null() {
        let sql = "INSERT INTO t VALUES (?)";
        let params = vec![Value::Null];
        let result = substitute_placeholders(sql, &params).unwrap();
        assert_eq!(result, "INSERT INTO t VALUES (NULL)");
    }

    #[test]
    fn test_substitute_placeholders_count_mismatch() {
        let sql = "SELECT ? + ?";
        let params = vec![Value::Int64(10)];
        assert!(substitute_placeholders(sql, &params).is_none());
    }

    #[test]
    fn test_substitute_placeholders_in_string_literal() {
        let sql = "SELECT '?' AS q, ? AS p";
        let params = vec![Value::Int64(42)];
        let result = substitute_placeholders(sql, &params).unwrap();
        assert_eq!(result, "SELECT '?' AS q, 42 AS p");
    }

    #[test]
    fn test_prepared_statement_store_lifecycle() {
        let mut store = PreparedStatementStore::new();
        let id1 = store.prepare("SELECT ?".to_string());
        let id2 = store.prepare("INSERT INTO t VALUES (?)".to_string());
        assert_ne!(id1, id2);
        assert_eq!(store.len(), 2);
        assert!(store.get(id1).is_some());
        assert!(store.close(id1));
        assert_eq!(store.len(), 1);
        assert!(store.get(id1).is_none());
    }

    #[test]
    fn test_prepared_statement_long_data() {
        let mut stmt = PreparedStatement::new(1, "INSERT INTO t VALUES (?)".to_string());
        stmt.append_long_data(0, b"hello ");
        stmt.append_long_data(0, b"world");
        assert_eq!(stmt.long_data.get(&0).unwrap(), b"hello world");
        stmt.reset();
        assert!(stmt.long_data.is_empty());
    }

    #[test]
    fn test_stmt_prepare_command_parse() {
        let cmd = StmtPrepareCommand::parse(b"SELECT ?\0");
        assert_eq!(cmd.sql, "SELECT ?");
    }

    #[test]
    fn test_stmt_close_command_parse() {
        let payload = 42u32.to_le_bytes();
        let cmd = StmtCloseCommand::parse(&payload).unwrap();
        assert_eq!(cmd.stmt_id, 42);
    }

    #[test]
    fn test_stmt_reset_command_parse() {
        let payload = 7u32.to_le_bytes();
        let cmd = StmtResetCommand::parse(&payload).unwrap();
        assert_eq!(cmd.stmt_id, 7);
    }

    #[test]
    fn test_stmt_send_long_data_command_parse() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_le_bytes()); // stmt_id
        payload.extend_from_slice(&0u16.to_le_bytes()); // param_id
        payload.extend_from_slice(b"binary data");
        let cmd = StmtSendLongDataCommand::parse(&payload).unwrap();
        assert_eq!(cmd.stmt_id, 1);
        assert_eq!(cmd.param_id, 0);
        assert_eq!(cmd.data, b"binary data");
    }

    #[test]
    fn test_stmt_execute_parse_no_params() {
        // stmt_id=1, flags=0, iteration_count=1
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.push(0);
        payload.extend_from_slice(&1u32.to_le_bytes());
        let cmd = StmtExecuteCommand::parse(&payload, 0, &HashMap::new()).unwrap();
        assert_eq!(cmd.stmt_id, 1);
        assert_eq!(cmd.iteration_count, 1);
        assert!(cmd.params.is_empty());
    }

    #[test]
    fn test_stmt_execute_parse_with_int_params() {
        // stmt_id=1, flags=0, iteration_count=1, 2 个参数
        // NULL bitmap: 0x00（无 NULL）
        // new_params_bound_flag: 0x01
        // param types: LongLong (0x08, signed), LongLong (0x08, signed)
        // param values: 100 (8 bytes LE), 200 (8 bytes LE)
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_le_bytes()); // stmt_id
        payload.push(0); // flags
        payload.extend_from_slice(&1u32.to_le_bytes()); // iteration_count
        payload.push(0x00); // NULL bitmap (1 byte, no nulls)
        payload.push(0x01); // new_params_bound_flag
        payload.push(0x08); // type: LongLong
        payload.push(0x00); // unsigned=0
        payload.push(0x08); // type: LongLong
        payload.push(0x00); // unsigned=0
        payload.extend_from_slice(&100i64.to_le_bytes());
        payload.extend_from_slice(&200i64.to_le_bytes());
        let cmd = StmtExecuteCommand::parse(&payload, 2, &HashMap::new()).unwrap();
        assert_eq!(cmd.params.len(), 2);
        assert_eq!(cmd.params[0], Value::Int64(100));
        assert_eq!(cmd.params[1], Value::Int64(200));
    }

    #[test]
    fn test_stmt_execute_parse_with_null_param() {
        // stmt_id=1, flags=0, iteration_count=1, 1 个参数，参数为 NULL
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.push(0);
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.push(0x01); // NULL bitmap: bit 0 set (param 0 is NULL)
        payload.push(0x01); // new_params_bound_flag
        payload.push(0x06); // type: NULL
        payload.push(0x00);
        let cmd = StmtExecuteCommand::parse(&payload, 1, &HashMap::new()).unwrap();
        assert_eq!(cmd.params.len(), 1);
        assert_eq!(cmd.params[0], Value::Null);
    }

    #[test]
    fn test_stmt_execute_parse_with_string_param() {
        // stmt_id=1, 1 个 VarString 参数，值 "hello"
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.push(0);
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.push(0x00); // NULL bitmap
        payload.push(0x01); // new_params_bound_flag
        payload.push(0x0F); // type: VarString
        payload.push(0x00);
        // lenenc string: 5 + "hello"
        payload.push(5);
        payload.extend_from_slice(b"hello");
        let cmd = StmtExecuteCommand::parse(&payload, 1, &HashMap::new()).unwrap();
        assert_eq!(cmd.params.len(), 1);
        assert_eq!(cmd.params[0], Value::Text("hello".to_string()));
    }

    #[test]
    fn test_encode_binary_row_integers() {
        let row = vec![Value::Int64(42), Value::Int64(100)];
        let encoded = encode_binary_row(&row);
        assert_eq!(encoded[0], 0x00); // header
        assert_eq!(encoded[1], 0x00); // NULL bitmap (no nulls)
        // 42 fits in i8, so 1 byte
        assert_eq!(encoded[2], 42);
        // 100 fits in i8, so 1 byte
        assert_eq!(encoded[3], 100);
    }

    #[test]
    fn test_encode_binary_row_with_null() {
        let row = vec![Value::Int64(1), Value::Null, Value::Int64(3)];
        let encoded = encode_binary_row(&row);
        assert_eq!(encoded[0], 0x00); // header
        assert_eq!(encoded[1], 0x02); // bit 1 set (param 1 is NULL)
        assert_eq!(encoded[2], 1); // value 1
        // NULL skipped
        assert_eq!(encoded[3], 3); // value 3
    }

    #[test]
    fn test_encode_binary_row_string() {
        let row = vec![Value::Text("hi".to_string())];
        let encoded = encode_binary_row(&row);
        assert_eq!(encoded[0], 0x00);
        assert_eq!(encoded[1], 0x00);
        assert_eq!(encoded[2], 2); // lenenc length
        assert_eq!(&encoded[3..5], b"hi");
    }

    #[test]
    fn test_encode_binary_row_blob() {
        let row = vec![Value::Blob(vec![0xDE, 0xAD, 0xBE, 0xEF])];
        let encoded = encode_binary_row(&row);
        assert_eq!(encoded[0], 0x00);
        assert_eq!(encoded[1], 0x00);
        assert_eq!(encoded[2], 4);
        assert_eq!(&encoded[3..7], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_encode_binary_row_float() {
        let row = vec![Value::Float64(3.5)];
        let encoded = encode_binary_row(&row);
        assert_eq!(encoded[0], 0x00);
        assert_eq!(encoded[1], 0x00);
        assert_eq!(encoded.len(), 1 + 1 + 8);
        let f = f64::from_le_bytes(encoded[2..10].try_into().unwrap());
        assert!((f - 3.5).abs() < 1e-9);
    }

    #[test]
    fn test_prepare_ok_packet_encode() {
        let pkt = PrepareOkPacket::new(42, 3, 2);
        let encoded = pkt.encode();
        assert_eq!(encoded[0], 0x00);
        assert_eq!(u32::from_le_bytes(encoded[1..5].try_into().unwrap()), 42);
        assert_eq!(u16::from_le_bytes(encoded[5..7].try_into().unwrap()), 3);
        assert_eq!(u16::from_le_bytes(encoded[7..9].try_into().unwrap()), 2);
    }

    #[test]
    fn test_mysql_type_from_byte_known() {
        assert_eq!(mysql_type_from_byte(0x01, false), MysqlType::Tiny);
        assert_eq!(mysql_type_from_byte(0x03, false), MysqlType::Long);
        assert_eq!(mysql_type_from_byte(0x08, false), MysqlType::LongLong);
        assert_eq!(mysql_type_from_byte(0x0F, false), MysqlType::VarString);
        assert_eq!(mysql_type_from_byte(0xFC, false), MysqlType::Blob);
    }

    #[test]
    fn test_mysql_type_from_byte_unknown_fallback() {
        assert_eq!(mysql_type_from_byte(0xAA, false), MysqlType::VarString);
    }

    #[test]
    fn test_decode_binary_value_unsigned_tiny() {
        let buf = vec![200u8];
        let (value, rest) = decode_binary_value(MysqlType::Tiny, true, &buf).unwrap();
        assert_eq!(value, Value::Int64(200));
        assert!(rest.is_empty());
    }

    #[test]
    fn test_decode_binary_value_signed_short() {
        let buf = (-1i16).to_le_bytes();
        let (value, rest) = decode_binary_value(MysqlType::Short, false, &buf).unwrap();
        assert_eq!(value, Value::Int64(-1));
        assert!(rest.is_empty());
    }

    #[test]
    fn test_decode_binary_value_unsigned_long_long_overflow() {
        // u64::MAX 会被截断为 i64::MAX
        let buf = u64::MAX.to_le_bytes();
        let (value, _) = decode_binary_value(MysqlType::LongLong, true, &buf).unwrap();
        assert_eq!(value, Value::Int64(i64::MAX));
    }

    #[test]
    fn test_value_to_sql_literal_text() {
        assert_eq!(
            value_to_sql_literal(&Value::Text("hello".to_string())),
            "'hello'"
        );
        assert_eq!(
            value_to_sql_literal(&Value::Text("it's me".to_string())),
            "'it''s me'"
        );
    }

    #[test]
    fn test_value_to_sql_literal_blob_hex() {
        let v = value_to_sql_literal(&Value::Blob(vec![0xDE, 0xAD, 0xBE, 0xEF]));
        assert_eq!(v, "x'DEADBEEF'");
    }

    #[test]
    fn test_value_to_sql_literal_int_float() {
        assert_eq!(value_to_sql_literal(&Value::Int64(42)), "42");
        let f = value_to_sql_literal(&Value::Float64(3.5));
        assert_eq!(f, "3.500000");
    }

    #[test]
    fn test_format_decimal_literal_negative() {
        assert_eq!(format_decimal_literal(-12345, 2), "-123.45");
        assert_eq!(format_decimal_literal(0, 4), "0.0000");
    }
}
