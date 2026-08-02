//! TDS 命令处理 — SQLBatch / RPC / Logout / Attention。

use thiserror::Error;

/// TDS 命令类型枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Command {
    /// SQL Batch（0x01）：执行 SQL 文本
    SqlBatch = 0x01,
    /// RPC（0x03）：远程过程调用
    Rpc = 0x03,
    /// Logout：客户端关闭连接（逻辑标记，无独立字节码）
    Logout = 0x00,
    /// Attention（0x06）：取消正在执行的查询
    Attention = 0x06,
}

impl Command {
    /// 从 TDS 包类型字节解析命令类型。
    pub fn from_packet_type(byte: u8) -> Option<Self> {
        Some(match byte {
            0x01 => Command::SqlBatch,
            0x03 => Command::Rpc,
            0x06 => Command::Attention,
            _ => return None,
        })
    }

    /// 是否需要关闭连接。
    pub fn is_logout(self) -> bool {
        matches!(self, Command::Logout)
    }

    /// 是否为 SQL 查询类命令。
    pub fn is_query(self) -> bool {
        matches!(self, Command::SqlBatch | Command::Rpc)
    }

    /// 是否为 Attention（取消信号）。
    pub fn is_attention(self) -> bool {
        matches!(self, Command::Attention)
    }
}

/// 命令解析错误。
#[derive(Debug, Error)]
pub enum CommandError {
    /// 命令包为空
    #[error("empty command packet")]
    Empty,
    /// 未知命令类型
    #[error("unknown command type: 0x{0:02X}")]
    UnknownCommand(u8),
    /// SQL 解析错误
    #[error("sql parse error: {0}")]
    SqlParse(String),
    /// 执行错误
    #[error("execution error: {0}")]
    Execution(String),
}

/// SQLBatch 命令解析结果（SQL 文本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlBatchCommand {
    /// SQL 语句（UTF-8 解码后的文本）
    pub sql: String,
    /// 是否以 NUL 字符结尾（部分客户端会附加 0x0000）
    pub trailing_nul: bool,
}

impl SqlBatchCommand {
    /// 从 payload 解析 SQLBatch（payload 即 UTF-16LE 字节序列）。
    pub fn parse(payload: &[u8]) -> Self {
        // 检测并去除末尾的 UTF-16LE NUL（0x00 0x00）
        let (data, trailing_nul) = if payload.len() >= 2
            && payload[payload.len() - 2] == 0
            && payload[payload.len() - 1] == 0
        {
            (&payload[..payload.len() - 2], true)
        } else {
            (payload, false)
        };

        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let sql = String::from_utf16_lossy(&units);
        Self {
            sql: sql.trim().to_string(),
            trailing_nul,
        }
    }

    /// 将 SQL 文本编码为 UTF-16LE 字节序列（payload）。
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.sql.len() * 2);
        for unit in self.sql.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        if self.trailing_nul {
            bytes.extend_from_slice(&[0, 0]);
        }
        bytes
    }
}

/// RPC 单个参数（已解析）。
///
/// TDS RPC 参数格式（MS-TDS 2.2.6.5）：
/// ```text
/// 名称长度(1B, UCS-2 字符数) + 名称(UTF-16LE) + 状态(1B) + 类型信息 + 值长度(varint) + 值
/// ```
/// 其中类型信息与值长度前缀由类型字节决定：
/// - BIT(0x68)：无类型特定字节，1B 值长度前缀
/// - INTN(0x26)/FLOATN(0x6E)/DATETIMEN(0x6D)/TIME(0x29)：1B max_length + 1B 值长度前缀
/// - NUMERICN(0x6C)：1B precision + 1B scale + 1B 值长度前缀
/// - BIGVARBIN(0xA5)：2B max_length + 2B 值长度前缀
/// - BIGVARCHAR(0xA7)/NCHAR(0xE6)/NVARCHAR(0xE7)：2B max_length + 4B collation + 2B 值长度前缀
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcParam {
    /// 参数名（含前导 @，如 "@P1"）
    pub name: String,
    /// 状态标志（0x01=ByRef, 0x02=Default）
    pub status: u8,
    /// TDS 类型字节（与 `TdsType` 的 u8 值一致）
    pub type_byte: u8,
    /// 类型信息特定字节（如 INTN 的 1B max_length、NVARCHAR 的 max_length+collation）
    pub type_info: Vec<u8>,
    /// 参数值（NULL 时为 None；空字符串为 Some(vec![])）
    pub value: Option<Vec<u8>>,
}

impl RpcParam {
    /// 从 payload 当前位置解析单个 RPC 参数。
    ///
    /// 解析失败时返回 `CommandError`，`*pos` 的位置不确定（由调用方处理）。
    fn parse(payload: &[u8], pos: &mut usize) -> Result<Self, CommandError> {
        // 1) 名称长度（1B，UCS-2 字符数）
        if *pos + 1 > payload.len() {
            return Err(CommandError::SqlParse(
                "RPC param name length truncated".to_string(),
            ));
        }
        let name_char_len = payload[*pos] as usize;
        *pos += 1;
        let name_byte_len = name_char_len * 2;
        if *pos + name_byte_len > payload.len() {
            return Err(CommandError::SqlParse(
                "RPC param name out of bounds".to_string(),
            ));
        }
        let name = if name_byte_len > 0 {
            let units: Vec<u16> = payload[*pos..*pos + name_byte_len]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            String::new()
        };
        *pos += name_byte_len;

        // 2) 状态（1B）
        if *pos + 1 > payload.len() {
            return Err(CommandError::SqlParse(
                "RPC param status truncated".to_string(),
            ));
        }
        let status = payload[*pos];
        *pos += 1;

        // 3) 类型字节
        if *pos + 1 > payload.len() {
            return Err(CommandError::SqlParse(
                "RPC param type byte truncated".to_string(),
            ));
        }
        let type_byte = payload[*pos];
        *pos += 1;

        // 4) 类型特定字节 + 值长度前缀大小
        let (type_info, value_len_size) = Self::parse_type_info(type_byte, payload, pos)?;

        // 5) 值
        let value = Self::parse_value(value_len_size, payload, pos)?;

        Ok(Self {
            name,
            status,
            type_byte,
            type_info,
            value,
        })
    }

    /// 解析类型信息特定字节，返回 (type_info_bytes, value_len_size)。
    ///
    /// `value_len_size`：0=定长无前缀（不应出现于 RPC），1=1B 前缀，2=2B 前缀。
    fn parse_type_info(
        type_byte: u8,
        payload: &[u8],
        pos: &mut usize,
    ) -> Result<(Vec<u8>, usize), CommandError> {
        match type_byte {
            // BIT(0x68)：无类型特定字节；RPC 中使用 1B 值长度前缀
            0x68 => Ok((Vec::new(), 1)),
            // DATE(0x28)：无类型特定字节；RPC 中使用 1B 值长度前缀
            0x28 => Ok((Vec::new(), 1)),
            // INTN(0x26)/FLOATN(0x6E)/DATETIMEN(0x6D)/TIME(0x29)：1B max_length/scale
            0x26 | 0x6E | 0x6D | 0x29 => {
                if *pos + 1 > payload.len() {
                    return Err(CommandError::SqlParse(
                        "RPC param type_info truncated (1B)".to_string(),
                    ));
                }
                let bytes = payload[*pos..*pos + 1].to_vec();
                *pos += 1;
                Ok((bytes, 1))
            }
            // NUMERICN(0x6C)：1B precision + 1B scale
            0x6C => {
                if *pos + 2 > payload.len() {
                    return Err(CommandError::SqlParse(
                        "RPC param type_info truncated (NUMERICN 2B)".to_string(),
                    ));
                }
                let bytes = payload[*pos..*pos + 2].to_vec();
                *pos += 2;
                Ok((bytes, 1))
            }
            // BIGVARBIN(0xA5)：2B max_length
            0xA5 => {
                if *pos + 2 > payload.len() {
                    return Err(CommandError::SqlParse(
                        "RPC param type_info truncated (BIGVARBIN 2B)".to_string(),
                    ));
                }
                let bytes = payload[*pos..*pos + 2].to_vec();
                *pos += 2;
                Ok((bytes, 2))
            }
            // BIGVARCHAR(0xA7)/NCHAR(0xE6)/NVARCHAR(0xE7)：2B max_length + 4B collation
            0xA7 | 0xE6 | 0xE7 => {
                if *pos + 6 > payload.len() {
                    return Err(CommandError::SqlParse(
                        "RPC param type_info truncated (varchar/nchar 6B)".to_string(),
                    ));
                }
                let bytes = payload[*pos..*pos + 6].to_vec();
                *pos += 6;
                Ok((bytes, 2))
            }
            _ => Err(CommandError::SqlParse(format!(
                "unknown RPC param type: 0x{type_byte:02X}"
            ))),
        }
    }

    /// 解析值字节（含长度前缀）。
    ///
    /// - 1B 前缀：0x00 = NULL，其他 = 实际字节数
    /// - 2B 前缀：0xFFFF = NULL，0x0000 = 空字符串，其他 = 实际字节数
    fn parse_value(
        value_len_size: usize,
        payload: &[u8],
        pos: &mut usize,
    ) -> Result<Option<Vec<u8>>, CommandError> {
        if value_len_size == 0 {
            // RPC 中不应出现定长无前缀类型，按 NULL 处理
            return Ok(None);
        }
        // 读取长度前缀
        let (len, is_null_marker) = if value_len_size == 1 {
            if *pos + 1 > payload.len() {
                return Err(CommandError::SqlParse(
                    "RPC param value length truncated (1B)".to_string(),
                ));
            }
            let l = payload[*pos] as usize;
            *pos += 1;
            (l, l == 0)
        } else {
            if *pos + 2 > payload.len() {
                return Err(CommandError::SqlParse(
                    "RPC param value length truncated (2B)".to_string(),
                ));
            }
            let l = u16::from_le_bytes([payload[*pos], payload[*pos + 1]]) as usize;
            *pos += 2;
            // 0xFFFF 表示 NULL/DEFAULT（仅 BIGVARCHAR/BIGVARBIN/NVARCHAR/NCHAR）
            if l == 0xFFFF {
                return Ok(None);
            }
            (l, false)
        };
        if is_null_marker && value_len_size == 1 {
            return Ok(None);
        }
        if *pos + len > payload.len() {
            return Err(CommandError::SqlParse(format!(
                "RPC param value out of bounds: len={len}, remaining={}",
                payload.len() - *pos
            )));
        }
        let value = payload[*pos..*pos + len].to_vec();
        *pos += len;
        Ok(Some(value))
    }

    /// 是否为 NULL 值。
    pub fn is_null(&self) -> bool {
        self.value.is_none()
    }

    /// 解码为 UTF-16LE 字符串（适用于 NVARCHAR/NCHAR）。
    ///
    /// NULL 返回 None；空字符串返回 Some("")。
    pub fn as_string(&self) -> Option<String> {
        let bytes = self.value.as_ref()?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    /// 解码为 ANSI 字符串（适用于 BIGVARCHAR）。
    pub fn as_ansi_string(&self) -> Option<String> {
        let bytes = self.value.as_ref()?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    /// 解析为 i64 整数（适用于 INTN）。
    ///
    /// 根据 INTN 的 max_length 决定解析字节数（1/2/4/8）。
    pub fn as_int(&self) -> Option<i64> {
        let bytes = self.value.as_ref()?;
        let byte_len = self.type_info.first().copied().unwrap_or(8);
        if bytes.len() != byte_len as usize {
            // 实际值长度与声明的 max_length 不一致，按实际长度解析
            return parse_int_bytes(bytes);
        }
        parse_int_bytes(bytes)
    }
}

/// 按字节数解析有符号整数（1/2/4/8 字节 LE）。
fn parse_int_bytes(bytes: &[u8]) -> Option<i64> {
    match bytes.len() {
        1 => Some(i8::from_le_bytes([bytes[0]]) as i64),
        2 => Some(i16::from_le_bytes([bytes[0], bytes[1]]) as i64),
        4 => Some(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64),
        8 => Some(i64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])),
        _ => None,
    }
}

/// RPC 命令解析结果（过程名 + 参数列表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcCommand {
    /// 过程名（如 "sp_executesql"）
    pub proc_name: String,
    /// RPC 原始参数字节（含 Options(2B) + 参数字节，向后兼容字段）
    pub raw_params: Vec<u8>,
    /// 已解析的参数列表（不含 Options）
    pub params: Vec<RpcParam>,
}

impl RpcCommand {
    /// 从 payload 解析 RPC。
    ///
    /// RPC payload 格式（MS-TDS 2.2.6.5）：
    /// - ProcName（2 字节 LE 字节长度前缀 + UTF-16LE 字节）
    /// - Options（2 字节 LE）
    /// - 参数列表（每个参数格式见 [`RpcParam`]）
    ///
    /// 参数解析采用容错策略：遇到无法解析的字节时停止，
    /// 保留已成功解析的参数。`raw_params` 始终包含 Options + 全部参数原始字节。
    pub fn parse(payload: &[u8]) -> Result<Self, CommandError> {
        if payload.len() < 2 {
            return Err(CommandError::Empty);
        }
        let proc_name_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
        if 2 + proc_name_len > payload.len() {
            return Err(CommandError::SqlParse(
                "RPC proc name out of bounds".to_string(),
            ));
        }
        let name_units: Vec<u16> = payload[2..2 + proc_name_len]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let proc_name = String::from_utf16_lossy(&name_units);
        // raw_params 保留 Options + 参数字节（向后兼容）
        let raw_params = payload[2 + proc_name_len..].to_vec();

        // 解析参数列表：跳过 Options(2B)，逐个解析参数
        let mut params = Vec::new();
        let mut pos = 2 + proc_name_len + 2; // 跳过 proc_name + Options
        while pos < payload.len() {
            match RpcParam::parse(payload, &mut pos) {
                Ok(p) => params.push(p),
                Err(_) => {
                    // 解析失败：停止（保留已解析的参数）
                    break;
                }
            }
        }

        Ok(Self {
            proc_name,
            raw_params,
            params,
        })
    }

    /// 解析 sp_executesql 参数。
    ///
    /// sp_executesql 参数顺序（MS-TDS）：
    /// - 参数1：SQL 语句（NVARCHAR，UTF-16LE）
    /// - 参数2：参数定义（NVARCHAR，如 "@p1 int, @p2 varchar(10)"）
    /// - 参数3+：参数值（按参数定义顺序）
    ///
    /// 返回 (SQL 语句, 参数定义字符串, 参数值列表引用)。
    /// 如果参数不足，返回错误。
    pub fn parse_sp_executesql(&self) -> Result<SpExecutesqlParams<'_>, CommandError> {
        if self.params.is_empty() {
            return Err(CommandError::SqlParse(
                "sp_executesql requires at least 1 parameter (sql)".to_string(),
            ));
        }
        let sql = self.params[0]
            .as_string()
            .ok_or_else(|| CommandError::SqlParse("sp_executesql sql param is null".to_string()))?;
        // 参数2：参数定义（可选，无参数时省略）
        let param_def = if self.params.len() >= 2 {
            self.params[1].as_string().unwrap_or_default()
        } else {
            String::new()
        };
        // 参数3+：参数值
        let values: Vec<&RpcParam> = if self.params.len() > 2 {
            self.params[2..].iter().collect()
        } else {
            Vec::new()
        };
        Ok(SpExecutesqlParams {
            sql,
            param_def,
            values,
        })
    }

    /// 解析 sp_prepare 参数。
    ///
    /// sp_prepare 参数顺序（MS-TDS）：
    /// - 参数1：@handle（INTN，OUTPUT，由服务器分配）
    /// - 参数2：@params（NVARCHAR，参数定义）
    /// - 参数3：@stmt（NVARCHAR，SQL 语句）
    /// - 参数4：@options（INTN，可选）
    ///
    /// 兼容任务约定：若参数数不足 3，则取第一个 NVARCHAR 参数作为 SQL。
    /// 返回 SQL 语句字符串。
    pub fn parse_sp_prepare(&self) -> Result<String, CommandError> {
        // 标准 SQL Server 规范：第三个参数为 stmt
        if self.params.len() >= 3 {
            if let Some(stmt) = self.params[2].as_string() {
                return Ok(stmt);
            }
        }
        // 兼容形式：第一个 NVARCHAR 参数作为 SQL
        for p in &self.params {
            if p.type_byte == 0xE7 || p.type_byte == 0xA7 || p.type_byte == 0xE6 {
                if let Some(s) = p.as_string() {
                    return Ok(s);
                }
            }
        }
        Err(CommandError::SqlParse(
            "sp_prepare: cannot extract SQL statement".to_string(),
        ))
    }

    /// 解析 sp_execute 参数。
    ///
    /// sp_execute 参数顺序（MS-TDS）：
    /// - 参数1：@handle（INTN，预处理语句句柄）
    /// - 参数2+：参数值（按 sp_prepare 时的参数定义顺序）
    ///
    /// 返回 (handle, 参数值列表引用)。
    pub fn parse_sp_execute(&self) -> Result<SpExecuteParams<'_>, CommandError> {
        if self.params.is_empty() {
            return Err(CommandError::SqlParse(
                "sp_execute requires at least 1 parameter (handle)".to_string(),
            ));
        }
        let handle = self.params[0].as_int().ok_or_else(|| {
            CommandError::SqlParse("sp_execute handle is not an integer".to_string())
        })?;
        let values: Vec<&RpcParam> = if self.params.len() > 1 {
            self.params[1..].iter().collect()
        } else {
            Vec::new()
        };
        Ok(SpExecuteParams { handle, values })
    }
}

/// sp_executesql 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpExecutesqlParams<'a> {
    /// SQL 语句（UTF-16LE 解码后）
    pub sql: String,
    /// 参数定义字符串（如 "@p1 int, @p2 varchar(10)"）
    pub param_def: String,
    /// 参数值列表（引用原始 RpcParam）
    pub values: Vec<&'a RpcParam>,
}

/// sp_execute 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpExecuteParams<'a> {
    /// 预处理语句句柄
    pub handle: i64,
    /// 参数值列表（引用原始 RpcParam）
    pub values: Vec<&'a RpcParam>,
}

/// 从 TDS 包类型与 payload 解析命令。
///
/// 返回 (命令类型, payload)。
pub fn parse_command(
    packet_type_byte: u8,
    payload: &[u8],
) -> Result<(Command, &[u8]), CommandError> {
    let cmd = Command::from_packet_type(packet_type_byte)
        .ok_or(CommandError::UnknownCommand(packet_type_byte))?;
    Ok((cmd, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_from_packet_type_known() {
        assert_eq!(Command::from_packet_type(0x01), Some(Command::SqlBatch));
        assert_eq!(Command::from_packet_type(0x03), Some(Command::Rpc));
        assert_eq!(Command::from_packet_type(0x06), Some(Command::Attention));
    }

    #[test]
    fn test_command_from_packet_type_unknown() {
        assert_eq!(Command::from_packet_type(0xFF), None);
        assert_eq!(Command::from_packet_type(0x99), None);
        assert_eq!(Command::from_packet_type(0x00), None);
    }

    #[test]
    fn test_command_is_logout_and_attention() {
        assert!(Command::Logout.is_logout());
        assert!(!Command::SqlBatch.is_logout());
        assert!(Command::Attention.is_attention());
        assert!(!Command::SqlBatch.is_attention());
    }

    #[test]
    fn test_command_is_query() {
        assert!(Command::SqlBatch.is_query());
        assert!(Command::Rpc.is_query());
        assert!(!Command::Logout.is_query());
        assert!(!Command::Attention.is_query());
    }

    #[test]
    fn test_sql_batch_parse_basic() {
        let sql = "SELECT 1";
        let mut bytes = Vec::new();
        for unit in sql.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let cmd = SqlBatchCommand::parse(&bytes);
        assert_eq!(cmd.sql, "SELECT 1");
        assert!(!cmd.trailing_nul);
    }

    #[test]
    fn test_sql_batch_parse_with_trailing_nul() {
        let sql = "SELECT 1";
        let mut bytes = Vec::new();
        for unit in sql.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        let cmd = SqlBatchCommand::parse(&bytes);
        assert_eq!(cmd.sql, "SELECT 1");
        assert!(cmd.trailing_nul);
    }

    #[test]
    fn test_sql_batch_encode_roundtrip() {
        let original = SqlBatchCommand {
            sql: "SELECT * FROM users".to_string(),
            trailing_nul: false,
        };
        let bytes = original.encode();
        let decoded = SqlBatchCommand::parse(&bytes);
        assert_eq!(decoded.sql, original.sql);
    }

    #[test]
    fn test_sql_batch_parse_chinese() {
        let sql = "SELECT '中文测试'";
        let mut bytes = Vec::new();
        for unit in sql.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let cmd = SqlBatchCommand::parse(&bytes);
        assert_eq!(cmd.sql, "SELECT '中文测试'");
    }

    #[test]
    fn test_sql_batch_parse_empty() {
        let bytes = Vec::new();
        let cmd = SqlBatchCommand::parse(&bytes);
        assert_eq!(cmd.sql, "");
        assert!(!cmd.trailing_nul);
    }

    #[test]
    fn test_sql_batch_parse_single_nul_byte() {
        // 奇数字节长度：UTF-16LE 应为偶数，测试边界情况
        let bytes = vec![0u8];
        let cmd = SqlBatchCommand::parse(&bytes);
        assert_eq!(cmd.sql, "");
    }

    #[test]
    fn test_rpc_parse_basic() {
        let name: Vec<u16> = "sp_executesql".encode_utf16().collect();
        let name_byte_len = name.len() * 2;
        let mut payload = Vec::new();
        payload.extend_from_slice(&(name_byte_len as u16).to_le_bytes());
        for unit in name {
            payload.extend_from_slice(&unit.to_le_bytes());
        }
        payload.extend_from_slice(&[1, 2, 3]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        assert_eq!(cmd.proc_name, "sp_executesql");
        assert_eq!(cmd.raw_params, vec![1, 2, 3]);
    }

    #[test]
    fn test_rpc_parse_too_short() {
        let payload = [0u8; 1];
        let result = RpcCommand::parse(&payload);
        assert!(matches!(result, Err(CommandError::Empty)));
    }

    #[test]
    fn test_rpc_parse_proc_name_out_of_bounds() {
        // 声明长度 100 但 payload 不足
        let payload = [100u8, 0, b's', b'p'];
        let result = RpcCommand::parse(&payload);
        assert!(matches!(result, Err(CommandError::SqlParse(_))));
    }

    #[test]
    fn test_parse_command_known() {
        let payload = [0u8; 4];
        let (cmd, _) = parse_command(0x01, &payload).unwrap();
        assert_eq!(cmd, Command::SqlBatch);
    }

    #[test]
    fn test_parse_command_unknown() {
        let payload = [0u8; 4];
        let result = parse_command(0xAB, &payload);
        assert!(matches!(result, Err(CommandError::UnknownCommand(0xAB))));
    }

    #[test]
    fn test_sql_batch_with_whitespace_trimmed() {
        let sql = "   SELECT 1   ";
        let mut bytes = Vec::new();
        for unit in sql.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let cmd = SqlBatchCommand::parse(&bytes);
        assert_eq!(cmd.sql, "SELECT 1");
    }

    // =====================================================================
    //  RPC 参数解析测试
    // =====================================================================

    /// 构造 NVARCHAR RPC 参数字节序列。
    fn build_nvarchar_param(name: &str, value: &str) -> Vec<u8> {
        let name_units: Vec<u16> = name.encode_utf16().collect();
        let value_units: Vec<u16> = value.encode_utf16().collect();
        let value_bytes: Vec<u8> = value_units.iter().flat_map(|u| u.to_le_bytes()).collect();
        let mut buf = Vec::new();
        // 名称长度（1B，字符数）
        buf.push(name_units.len() as u8);
        // 名称（UTF-16LE）
        for u in name_units {
            buf.extend_from_slice(&u.to_le_bytes());
        }
        // 状态（1B）
        buf.push(0x00);
        // 类型字节 NVARCHAR
        buf.push(0xE7);
        // max_length（2B LE）+ collation（4B）
        buf.extend_from_slice(&8000u16.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        // 值长度（2B LE）+ 值
        buf.extend_from_slice(&(value_bytes.len() as u16).to_le_bytes());
        buf.extend_from_slice(&value_bytes);
        buf
    }

    /// 构造 INTN RPC 参数字节序列。
    fn build_int_param(name: &str, max_len: u8, value: Option<i64>) -> Vec<u8> {
        let name_units: Vec<u16> = name.encode_utf16().collect();
        let mut buf = Vec::new();
        buf.push(name_units.len() as u8);
        for u in name_units {
            buf.extend_from_slice(&u.to_le_bytes());
        }
        buf.push(0x00); // 状态
        buf.push(0x26); // INTN
        buf.push(max_len); // max_length
        match value {
            None => buf.push(0), // NULL
            Some(v) => match max_len {
                1 => {
                    buf.push(1);
                    buf.push(v as i8 as u8);
                }
                2 => {
                    buf.push(2);
                    buf.extend_from_slice(&(v as i16).to_le_bytes());
                }
                4 => {
                    buf.push(4);
                    buf.extend_from_slice(&(v as i32).to_le_bytes());
                }
                _ => {
                    buf.push(8);
                    buf.extend_from_slice(&v.to_le_bytes());
                }
            },
        }
        buf
    }

    /// 构造完整 RPC payload（proc_name + options + params）。
    fn build_rpc_payload(proc_name: &str, params: &[Vec<u8>]) -> Vec<u8> {
        let name_units: Vec<u16> = proc_name.encode_utf16().collect();
        let name_byte_len = name_units.len() * 2;
        let mut payload = Vec::new();
        // proc_name 长度（2B LE，字节长度）
        payload.extend_from_slice(&(name_byte_len as u16).to_le_bytes());
        for u in name_units {
            payload.extend_from_slice(&u.to_le_bytes());
        }
        // Options（2B LE）
        payload.extend_from_slice(&0u16.to_le_bytes());
        // 参数
        for p in params {
            payload.extend_from_slice(p);
        }
        payload
    }

    #[test]
    fn test_rpc_parse_with_nvarchar_param() {
        let param = build_nvarchar_param("@P1", "SELECT 1");
        let payload = build_rpc_payload("sp_executesql", &[param]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        assert_eq!(cmd.proc_name, "sp_executesql");
        assert_eq!(cmd.params.len(), 1);
        assert_eq!(cmd.params[0].name, "@P1");
        assert_eq!(cmd.params[0].type_byte, 0xE7);
        assert_eq!(cmd.params[0].as_string().unwrap(), "SELECT 1");
    }

    #[test]
    fn test_rpc_parse_with_int_param() {
        let param = build_int_param("@handle", 8, Some(42));
        let payload = build_rpc_payload("sp_execute", &[param]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        assert_eq!(cmd.proc_name, "sp_execute");
        assert_eq!(cmd.params.len(), 1);
        assert_eq!(cmd.params[0].name, "@handle");
        assert_eq!(cmd.params[0].type_byte, 0x26);
        assert_eq!(cmd.params[0].as_int().unwrap(), 42);
    }

    #[test]
    fn test_rpc_parse_with_null_int_param() {
        let param = build_int_param("@p1", 4, None);
        let payload = build_rpc_payload("sp_executesql", &[param]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        assert_eq!(cmd.params.len(), 1);
        assert!(cmd.params[0].is_null());
        assert_eq!(cmd.params[0].as_int(), None);
    }

    #[test]
    fn test_rpc_parse_multiple_params() {
        let sql_param = build_nvarchar_param("@P1", "SELECT @p1");
        let def_param = build_nvarchar_param("@P2", "@p1 int");
        let val_param = build_int_param("@P3", 4, Some(100));
        let payload = build_rpc_payload("sp_executesql", &[sql_param, def_param, val_param]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        assert_eq!(cmd.params.len(), 3);
        assert_eq!(cmd.params[0].as_string().unwrap(), "SELECT @p1");
        assert_eq!(cmd.params[1].as_string().unwrap(), "@p1 int");
        assert_eq!(cmd.params[2].as_int().unwrap(), 100);
    }

    #[test]
    fn test_rpc_parse_sp_executesql() {
        let sql_param = build_nvarchar_param("@P1", "SELECT @p1 + @p2");
        let def_param = build_nvarchar_param("@P2", "@p1 int, @p2 int");
        let val1 = build_int_param("@P3", 4, Some(1));
        let val2 = build_int_param("@P4", 4, Some(2));
        let payload = build_rpc_payload("sp_executesql", &[sql_param, def_param, val1, val2]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        let parsed = cmd.parse_sp_executesql().unwrap();
        assert_eq!(parsed.sql, "SELECT @p1 + @p2");
        assert_eq!(parsed.param_def, "@p1 int, @p2 int");
        assert_eq!(parsed.values.len(), 2);
        assert_eq!(parsed.values[0].as_int().unwrap(), 1);
        assert_eq!(parsed.values[1].as_int().unwrap(), 2);
    }

    #[test]
    fn test_rpc_parse_sp_executesql_no_params() {
        // 无参数定义：sp_executesql 'SELECT 1'
        let sql_param = build_nvarchar_param("@P1", "SELECT 1");
        let payload = build_rpc_payload("sp_executesql", &[sql_param]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        let parsed = cmd.parse_sp_executesql().unwrap();
        assert_eq!(parsed.sql, "SELECT 1");
        assert_eq!(parsed.param_def, "");
        assert!(parsed.values.is_empty());
    }

    #[test]
    fn test_rpc_parse_sp_executesql_missing_sql() {
        // 无参数：sp_executesql 无参数时应返回错误
        let payload = build_rpc_payload("sp_executesql", &[]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        let result = cmd.parse_sp_executesql();
        assert!(matches!(result, Err(CommandError::SqlParse(_))));
    }

    #[test]
    fn test_rpc_parse_sp_prepare_standard() {
        // SQL Server 规范：sp_prepare(@handle, @params, @stmt, @options)
        let handle = build_int_param("@P1", 4, Some(0)); // OUTPUT handle
        let params_def = build_nvarchar_param("@P2", "@p1 int");
        let stmt = build_nvarchar_param("@P3", "SELECT @p1");
        let options = build_int_param("@P4", 4, Some(1));
        let payload = build_rpc_payload("sp_prepare", &[handle, params_def, stmt, options]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        let sql = cmd.parse_sp_prepare().unwrap();
        assert_eq!(sql, "SELECT @p1");
    }

    #[test]
    fn test_rpc_parse_sp_prepare_compat() {
        // 兼容形式：sp_prepare(@stmt) 仅一个 NVARCHAR 参数
        let stmt = build_nvarchar_param("@P1", "SELECT 1");
        let payload = build_rpc_payload("sp_prepare", &[stmt]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        let sql = cmd.parse_sp_prepare().unwrap();
        assert_eq!(sql, "SELECT 1");
    }

    #[test]
    fn test_rpc_parse_sp_execute() {
        let handle = build_int_param("@P1", 8, Some(1));
        let val1 = build_int_param("@P2", 4, Some(42));
        let val2 = build_int_param("@P3", 4, Some(99));
        let payload = build_rpc_payload("sp_execute", &[handle, val1, val2]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        let parsed = cmd.parse_sp_execute().unwrap();
        assert_eq!(parsed.handle, 1);
        assert_eq!(parsed.values.len(), 2);
        assert_eq!(parsed.values[0].as_int().unwrap(), 42);
        assert_eq!(parsed.values[1].as_int().unwrap(), 99);
    }

    #[test]
    fn test_rpc_parse_sp_execute_missing_handle() {
        let payload = build_rpc_payload("sp_execute", &[]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        let result = cmd.parse_sp_execute();
        assert!(matches!(result, Err(CommandError::SqlParse(_))));
    }

    #[test]
    fn test_rpc_param_as_string_chinese() {
        let param = build_nvarchar_param("@P1", "SELECT '中文测试'");
        let payload = build_rpc_payload("sp_executesql", &[param]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        assert_eq!(cmd.params[0].as_string().unwrap(), "SELECT '中文测试'");
    }

    #[test]
    fn test_rpc_parse_int_byte_lengths() {
        // 1 字节
        let p1 = build_int_param("@p1", 1, Some(127));
        // 2 字节
        let p2 = build_int_param("@p2", 2, Some(32767));
        // 4 字节
        let p4 = build_int_param("@p4", 4, Some(-1));
        // 8 字节
        let p8 = build_int_param("@p8", 8, Some(i64::MAX));
        let payload = build_rpc_payload("sp_executesql", &[p1, p2, p4, p8]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        assert_eq!(cmd.params.len(), 4);
        assert_eq!(cmd.params[0].as_int().unwrap(), 127);
        assert_eq!(cmd.params[1].as_int().unwrap(), 32767);
        assert_eq!(cmd.params[2].as_int().unwrap(), -1);
        assert_eq!(cmd.params[3].as_int().unwrap(), i64::MAX);
    }

    #[test]
    fn test_rpc_parse_unknown_param_type_stops_parsing() {
        // 构造一个未知类型字节，参数解析应停止
        let name_units: Vec<u16> = "@P1".encode_utf16().collect();
        let mut bad_param = Vec::new();
        bad_param.push(name_units.len() as u8);
        for u in name_units {
            bad_param.extend_from_slice(&u.to_le_bytes());
        }
        bad_param.push(0x00); // 状态
        bad_param.push(0xFF); // 未知类型字节
        let payload = build_rpc_payload("sp_executesql", &[bad_param]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        // 参数解析失败，params 为空，但 raw_params 仍保留
        assert_eq!(cmd.proc_name, "sp_executesql");
        assert!(cmd.params.is_empty());
        assert!(!cmd.raw_params.is_empty());
    }

    #[test]
    fn test_rpc_parse_empty_nvarchar_value() {
        // 空字符串值：2B 长度前缀 = 0x0000
        let name_units: Vec<u16> = "@P1".encode_utf16().collect();
        let mut param = Vec::new();
        param.push(name_units.len() as u8);
        for u in name_units {
            param.extend_from_slice(&u.to_le_bytes());
        }
        param.push(0x00); // 状态
        param.push(0xE7); // NVARCHAR
        param.extend_from_slice(&8000u16.to_le_bytes()); // max_length
        param.extend_from_slice(&0u32.to_le_bytes()); // collation
        param.extend_from_slice(&0u16.to_le_bytes()); // 值长度 = 0（空字符串）
        let payload = build_rpc_payload("sp_executesql", &[param]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        assert_eq!(cmd.params.len(), 1);
        assert!(!cmd.params[0].is_null()); // 空字符串不是 NULL
        assert_eq!(cmd.params[0].as_string().unwrap(), "");
    }

    #[test]
    fn test_rpc_parse_nvarchar_null_value() {
        // NULL 值：2B 长度前缀 = 0xFFFF
        let name_units: Vec<u16> = "@P1".encode_utf16().collect();
        let mut param = Vec::new();
        param.push(name_units.len() as u8);
        for u in name_units {
            param.extend_from_slice(&u.to_le_bytes());
        }
        param.push(0x00); // 状态
        param.push(0xE7); // NVARCHAR
        param.extend_from_slice(&8000u16.to_le_bytes());
        param.extend_from_slice(&0u32.to_le_bytes());
        param.extend_from_slice(&0xFFFFu16.to_le_bytes()); // NULL marker
        let payload = build_rpc_payload("sp_executesql", &[param]);
        let cmd = RpcCommand::parse(&payload).unwrap();
        assert_eq!(cmd.params.len(), 1);
        assert!(cmd.params[0].is_null());
        assert_eq!(cmd.params[0].as_string(), None);
    }

    #[test]
    fn test_parse_int_bytes_helper() {
        assert_eq!(parse_int_bytes(&[127]), Some(127));
        assert_eq!(parse_int_bytes(&[0xFF]), Some(-1));
        assert_eq!(parse_int_bytes(&[0xFF, 0x7F]), Some(32767));
        assert_eq!(parse_int_bytes(&[0x00, 0x80]), Some(-32768));
        assert_eq!(
            parse_int_bytes(&[0xFF, 0xFF, 0xFF, 0x7F]),
            Some(i32::MAX as i64)
        );
        assert_eq!(parse_int_bytes(&[0, 0, 0, 0]), Some(0));
        assert_eq!(
            parse_int_bytes(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]),
            Some(i64::MAX)
        );
        assert_eq!(parse_int_bytes(&[]), None);
        assert_eq!(parse_int_bytes(&[1, 2, 3]), None);
    }
}
