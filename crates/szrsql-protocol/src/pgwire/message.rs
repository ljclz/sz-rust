//! pgwire 协议消息定义与编解码。
//!
//! # 协议格式
//!
//! ## 前端 → 后端（普通消息）
//! ```text
//! +-----+--------+---------+
//! | Type| Length | Payload |
//! | 1B  |  4B    | N bytes |
//! +-----+--------+---------+
//! ```
//! Length 包含自身 4 字节，不包含 Type。
//!
//! ## 后端 → 前端
//! 同上格式，但 Type 是 ASCII 字符（如 'R'/'S'/'K'/'Z'/'T'/'D'/'C'/'E'/'N'）。
//!
//! ## 启动阶段（前端 → 后端）
//! StartupMessage 没有 Type 字节，只有 Length + ProtocolVersion + 参数对。
//!
//! 参考文档：<https://www.postgresql.org/docs/current/protocol-message-format.html>

use bytes::{Buf, BufMut, BytesMut};
use std::io;

// =====================================================================
//  常量
// =====================================================================

/// 后端消息类型字节（Backend → Frontend）。
pub const MSG_AUTHENTICATION: u8 = b'R';
pub const MSG_PARAMETER_STATUS: u8 = b'S';
pub const MSG_BACKEND_KEY_DATA: u8 = b'K';
pub const MSG_READY_FOR_QUERY: u8 = b'Z';
pub const MSG_ROW_DESCRIPTION: u8 = b'T';
pub const MSG_DATA_ROW: u8 = b'D';
pub const MSG_COMMAND_COMPLETE: u8 = b'C';
pub const MSG_EMPTY_QUERY_RESPONSE: u8 = b'I';
pub const MSG_ERROR_RESPONSE: u8 = b'E';
pub const MSG_NOTICE_RESPONSE: u8 = b'N';
/// Phase 4.6：NotificationResponse（'A'）— LISTEN/NOTIFY 异步通知。
pub const MSG_NOTIFICATION_RESPONSE: u8 = b'A';

/// 后端扩展查询消息类型字节（Phase 4.3）。
pub const MSG_PARSE_COMPLETE: u8 = b'1';
pub const MSG_BIND_COMPLETE: u8 = b'2';
pub const MSG_CLOSE_COMPLETE: u8 = b'3';
pub const MSG_PORTAL_SUSPENDED: u8 = b's';
pub const MSG_PARAMETER_DESCRIPTION: u8 = b't';
pub const MSG_NO_DATA: u8 = b'n';

/// 前端消息类型字节（Frontend → Backend）。
pub const MSG_QUERY: u8 = b'Q';
pub const MSG_TERMINATE: u8 = b'X';
pub const MSG_PARSE: u8 = b'P';
pub const MSG_BIND: u8 = b'B';
pub const MSG_EXECUTE: u8 = b'E';
pub const MSG_DESCRIBE: u8 = b'D';
pub const MSG_SYNC: u8 = b'S';
pub const MSG_CLOSE: u8 = b'C';
pub const MSG_FLUSH: u8 = b'H';

/// Describe/Close 消息的变体字节。
pub const DESCRIBE_STATEMENT: u8 = b'S';
pub const DESCRIBE_PORTAL: u8 = b'P';

/// Close 消息的变体字节（与 Describe 共用）。
pub const CLOSE_STATEMENT: u8 = b'S';
pub const CLOSE_PORTAL: u8 = b'P';

/// 参数/结果格式码：0 = 文本，1 = 二进制。
pub const FORMAT_TEXT: i16 = 0;
pub const FORMAT_BINARY: i16 = 1;

/// `ReadyForQuery` 状态字节。
pub const STATUS_IDLE: u8 = b'I';
pub const STATUS_IN_TRANSACTION: u8 = b'T';
pub const STATUS_IN_FAILED_TRANSACTION: u8 = b'E';

/// AuthenticationOk 子类型码。
pub const AUTH_OK: u32 = 0;
pub const AUTH_CLEARTEXT_PASSWORD: u32 = 3;
pub const AUTH_MD5_PASSWORD: u32 = 5;
pub const AUTH_SASL: u32 = 10;
/// Phase 4.4：SASL 认证继续（服务器发送 server-first 消息）。
pub const AUTH_SASL_CONTINUE: u32 = 11;
/// Phase 4.4：SASL 认证完成（服务器发送 server-final 消息）。
pub const AUTH_SASL_FINAL: u32 = 12;

/// Phase 4.4：SASL 消息类型字节（'p'，与 PasswordMessage 共用）。
pub const MSG_PASSWORD_OR_SASL: u8 = b'p';

/// Phase 4.4：SCRAM-SHA-256 机制名称。
pub const SASL_MECHANISM_SCRAM_SHA_256: &str = "SCRAM-SHA-256";

// =====================================================================
//  SqlState / Severity
// =====================================================================

/// PostgreSQL SQLSTATE 错误码（5 字符）。
///
/// 完整列表见 <https://www.postgresql.org/docs/current/errcodes-appendix.html>。
/// Phase 4.1 仅实现协议层常用码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlState(pub &'static str);

impl SqlState {
    pub const SUCCESSFUL_COMPLETION: Self = Self("00000");
    pub const SYNTAX_ERROR: Self = Self("42601");
    pub const INVALID_AUTHORIZATION_SPECIFICATION: Self = Self("28000");
    pub const PROTOCOL_VIOLATION: Self = Self("08P01");
    pub const CONNECTION_EXCEPTION: Self = Self("08000");
    pub const FEATURE_NOT_SUPPORTED: Self = Self("0A000");
    pub const INTERNAL_ERROR: Self = Self("XX000");

    // ---- Phase 4.2 新增：SQL 执行相关 SQLSTATE ----

    /// 未定义表（42P01）
    pub const UNDEFINED_TABLE: Self = Self("42P01");
    /// 未定义列（42703）
    pub const UNDEFINED_COLUMN: Self = Self("42703");
    /// 重复表（42P07）
    pub const DUPLICATE_TABLE: Self = Self("42P07");
    /// 外键约束违反（23503）
    pub const FOREIGN_KEY_VIOLATION: Self = Self("23503");
    /// CHECK 约束违反（23514）
    pub const CHECK_VIOLATION: Self = Self("23514");
    /// 无效的文本表示（22P02）
    pub const INVALID_TEXT_REPRESENTATION: Self = Self("22P02");
    /// 未定义对象（42704）
    pub const UNDEFINED_OBJECT: Self = Self("42704");
    /// 重复对象（42710）
    pub const DUPLICATE_OBJECT: Self = Self("42710");
    /// 无效事务状态（25000）
    pub const INVALID_TRANSACTION_STATE: Self = Self("25000");

    /// 返回 5 字符 SQLSTATE 字符串。
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

/// 错误/通知严重性级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Fatal,
    Panic,
    Warning,
    Notice,
    Debug,
    Info,
    Log,
}

impl Severity {
    /// 返回 PG 兼容的严重性字符串（首字母大写）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
            Self::Panic => "PANIC",
            Self::Warning => "WARNING",
            Self::Notice => "NOTICE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Log => "LOG",
        }
    }
}

// =====================================================================
//  ErrorResponse
// =====================================================================

/// pgwire 错误响应消息字段（ErrorResponse / NoticeResponse）。
///
/// 字段以类型字节标识，常用：'S' 严重性、'C' SQLSTATE、'M' 消息、'F' 文件、
/// 'L' 行号、'R' 函数名。以 '\0' 结束。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorResponse {
    pub severity: Severity,
    pub sqlstate: SqlState,
    pub message: String,
}

impl ErrorResponse {
    /// 构造一个新的 ERROR 级别错误响应。
    pub fn error(sqlstate: SqlState, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            sqlstate,
            message: message.into(),
        }
    }

    /// 构造一个新的 FATAL 级别错误响应。
    pub fn fatal(sqlstate: SqlState, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Fatal,
            sqlstate,
            message: message.into(),
        }
    }

    /// 编码为后端消息字节流（不含 Type + Length 头）。
    fn encode_payload(&self, dst: &mut BytesMut) {
        dst.put_u8(b'S');
        dst.put_slice(self.severity.as_str().as_bytes());
        dst.put_u8(0);
        dst.put_u8(b'V'); // 9.6+ 区分 ERROR/NOTICE 的次要严重性
        dst.put_slice(self.severity.as_str().as_bytes());
        dst.put_u8(0);
        dst.put_u8(b'C');
        dst.put_slice(self.sqlstate.as_str().as_bytes());
        dst.put_u8(0);
        dst.put_u8(b'M');
        dst.put_slice(self.message.as_bytes());
        dst.put_u8(0);
        dst.put_u8(0); // 字段结束
    }
}

// =====================================================================
//  BackendMessage（后端 → 前端）
// =====================================================================

/// 后端消息枚举。仅实现 Phase 4.1 验收所需的最小子集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendMessage {
    /// AuthenticationOk（'R'，子类型 0）。
    AuthenticationOk,
    /// AuthenticationSASL（'R'，子类型 10）— Phase 4.4：列出服务器支持的 SASL 机制。
    AuthenticationSASL { mechanisms: Vec<String> },
    /// AuthenticationSASLContinue（'R'，子类型 11）— Phase 4.4：发送 server-first 消息。
    AuthenticationSASLContinue { data: Vec<u8> },
    /// AuthenticationSASLFinal（'R'，子类型 12）— Phase 4.4：发送 server-final 消息。
    AuthenticationSASLFinal { data: Vec<u8> },
    /// ParameterStatus（'S'）— 服务器参数（如 server_version）。
    ParameterStatus { name: String, value: String },
    /// BackendKeyData（'K'）— pid + secret_key，用于取消查询。
    BackendKeyData { pid: i32, secret_key: i32 },
    /// ReadyForQuery（'Z'）。
    ReadyForQuery { status: u8 },
    /// ErrorResponse（'E'）。
    ErrorResponse(ErrorResponse),
    /// NoticeResponse（'N'）。
    NoticeResponse(ErrorResponse),
    /// CommandComplete（'C'）— 命令完成标签。
    CommandComplete { tag: String },
    /// EmptyQueryResponse（'I'）。
    EmptyQueryResponse,
    /// ParseComplete（'1'）— Phase 4.3 扩展查询。
    ParseComplete,
    /// BindComplete（'2'）— Phase 4.3 扩展查询。
    BindComplete,
    /// CloseComplete（'3'）— Phase 4.3 扩展查询。
    CloseComplete,
    /// PortalSuspended（'s'）— Execute 时达到 max_rows 但仍有剩余行。
    PortalSuspended,
    /// ParameterDescription（'t'）— Describe statement 时返回参数类型 OID 列表。
    ParameterDescription { parameter_oids: Vec<u32> },
    /// NoData（'n'）— Describe 时该语句/portal 不返回结果集（如 DML/DDL）。
    NoData,
    /// NotificationResponse（'A'）— Phase 4.6：LISTEN/NOTIFY 异步通知。
    ///
    /// 当某个会话执行 `NOTIFY <channel>` 时，所有监听该频道的会话将收到此消息。
    /// payload 为通知负载字符串（PG 中默认为空字符串）。
    NotificationResponse {
        /// 发送方会话的 pid（即执行 NOTIFY 的 backend pid）。
        pid: i32,
        /// 频道名。
        channel: String,
        /// 负载字符串。
        payload: String,
    },
}

impl BackendMessage {
    /// 将消息完整编码为字节流（Type + Length + Payload）。
    pub fn encode(&self, dst: &mut BytesMut) {
        match self {
            Self::AuthenticationOk => {
                dst.put_u8(MSG_AUTHENTICATION);
                dst.put_i32(8); // length = 4 (self) + 4 (auth code)
                dst.put_u32(AUTH_OK);
            }
            Self::AuthenticationSASL { mechanisms } => {
                // payload: auth_code(4) + 每个机制 cstring + 终止 \0
                let mut payload_len = 4; // AUTH_SASL
                for m in mechanisms {
                    payload_len += m.len() + 1;
                }
                payload_len += 1; // 终止 \0
                dst.put_u8(MSG_AUTHENTICATION);
                dst.put_i32((payload_len + 4) as i32);
                dst.put_u32(AUTH_SASL);
                for m in mechanisms {
                    put_cstring(dst, m);
                }
                dst.put_u8(0); // 终止 \0
            }
            Self::AuthenticationSASLContinue { data } => {
                dst.put_u8(MSG_AUTHENTICATION);
                dst.put_i32((data.len() + 4 + 4) as i32);
                dst.put_u32(AUTH_SASL_CONTINUE);
                dst.put_slice(data);
            }
            Self::AuthenticationSASLFinal { data } => {
                dst.put_u8(MSG_AUTHENTICATION);
                dst.put_i32((data.len() + 4 + 4) as i32);
                dst.put_u32(AUTH_SASL_FINAL);
                dst.put_slice(data);
            }
            Self::ParameterStatus { name, value } => {
                let payload_len = name.len() + 1 + value.len() + 1;
                dst.put_u8(MSG_PARAMETER_STATUS);
                dst.put_i32((payload_len + 4) as i32);
                dst.put_slice(name.as_bytes());
                dst.put_u8(0);
                dst.put_slice(value.as_bytes());
                dst.put_u8(0);
            }
            Self::BackendKeyData { pid, secret_key } => {
                dst.put_u8(MSG_BACKEND_KEY_DATA);
                dst.put_i32(12); // length = 4 + 4 + 4
                dst.put_i32(*pid);
                dst.put_i32(*secret_key);
            }
            Self::ReadyForQuery { status } => {
                dst.put_u8(MSG_READY_FOR_QUERY);
                dst.put_i32(5); // length = 4 + 1
                dst.put_u8(*status);
            }
            Self::ErrorResponse(err) => {
                // 先写占位 Type + Length=0，待 payload 编码完成后回填长度
                dst.put_u8(MSG_ERROR_RESPONSE);
                let length_pos = dst.len();
                dst.put_i32(0);
                let payload_start = dst.len();
                err.encode_payload(dst);
                let payload_len = dst.len() - payload_start;
                let total_len = (payload_len + 4) as i32;
                dst[length_pos..length_pos + 4].copy_from_slice(&total_len.to_be_bytes());
            }
            Self::NoticeResponse(err) => {
                dst.put_u8(MSG_NOTICE_RESPONSE);
                let length_pos = dst.len();
                dst.put_i32(0);
                let payload_start = dst.len();
                err.encode_payload(dst);
                let payload_len = dst.len() - payload_start;
                let total_len = (payload_len + 4) as i32;
                dst[length_pos..length_pos + 4].copy_from_slice(&total_len.to_be_bytes());
            }
            Self::CommandComplete { tag } => {
                let payload_len = tag.len() + 1;
                dst.put_u8(MSG_COMMAND_COMPLETE);
                dst.put_i32((payload_len + 4) as i32);
                dst.put_slice(tag.as_bytes());
                dst.put_u8(0);
            }
            Self::EmptyQueryResponse => {
                dst.put_u8(MSG_EMPTY_QUERY_RESPONSE);
                dst.put_i32(4);
            }
            Self::ParseComplete => {
                dst.put_u8(MSG_PARSE_COMPLETE);
                dst.put_i32(4);
            }
            Self::BindComplete => {
                dst.put_u8(MSG_BIND_COMPLETE);
                dst.put_i32(4);
            }
            Self::CloseComplete => {
                dst.put_u8(MSG_CLOSE_COMPLETE);
                dst.put_i32(4);
            }
            Self::PortalSuspended => {
                dst.put_u8(MSG_PORTAL_SUSPENDED);
                dst.put_i32(4);
            }
            Self::ParameterDescription { parameter_oids } => {
                // payload: i16 count + count * i32 OID
                let payload_len = 2 + parameter_oids.len() * 4;
                dst.put_u8(MSG_PARAMETER_DESCRIPTION);
                dst.put_i32((payload_len + 4) as i32);
                dst.put_i16(parameter_oids.len() as i16);
                for oid in parameter_oids {
                    dst.put_u32(*oid);
                }
            }
            Self::NoData => {
                dst.put_u8(MSG_NO_DATA);
                dst.put_i32(4);
            }
            Self::NotificationResponse {
                pid,
                channel,
                payload,
            } => {
                // payload: i32 pid + cstring channel + cstring payload
                let payload_len = 4 + (channel.len() + 1) + (payload.len() + 1);
                dst.put_u8(MSG_NOTIFICATION_RESPONSE);
                dst.put_i32((payload_len + 4) as i32);
                dst.put_i32(*pid);
                put_cstring(dst, channel);
                put_cstring(dst, payload);
            }
        }
    }

    /// 便捷方法：编码并返回独立的 `BytesMut`。
    pub fn to_bytes(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        self.encode(&mut buf);
        buf
    }
}

// =====================================================================
//  FrontendMessage（前端 → 后端）
// =====================================================================

/// 前端消息枚举（不含启动阶段消息）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendMessage {
    /// Query（'Q'）— 简单查询。
    Query { sql: String },
    /// Terminate（'X'）— 关闭连接。
    Terminate,
    /// SASLInitialResponse（'p'）— Phase 4.4：客户端选择的 SASL 机制与初始响应。
    SASLInitialResponse {
        mechanism: String,
        initial_response: Option<Vec<u8>>,
    },
    /// SASLResponse（'p'）— Phase 4.4：SASL 后续响应数据。
    SASLResponse { data: Vec<u8> },
    /// Parse（'P'）— Phase 4.3 扩展查询：将 SQL 解析为命名预处理语句。
    Parse {
        statement_name: String,
        sql: String,
        parameter_oids: Vec<u32>,
    },
    /// Bind（'B'）— 将参数绑定到预处理语句，生成命名 portal。
    Bind {
        portal_name: String,
        statement_name: String,
        parameter_format_codes: Vec<i16>,
        parameters: Vec<Option<Vec<u8>>>,
        result_format_codes: Vec<i16>,
    },
    /// Execute（'E'）— 执行已绑定的 portal，限制返回行数（0 表示全部）。
    Execute { portal_name: String, max_rows: i32 },
    /// Describe（'D'）— 描述语句或 portal 的参数/结果。
    Describe { variant: u8, name: String },
    /// Close（'C'）— 关闭语句或 portal。
    Close { variant: u8, name: String },
    /// Sync（'S'）— 触发 ReadyForQuery 响应（无 payload）。
    Sync,
    /// Flush（'H'）— 强制发送已缓冲的响应（不触发 ReadyForQuery）。
    Flush,
}

impl FrontendMessage {
    /// 从已读入的 `BytesMut` 解码下一条前端消息。
    ///
    /// 返回 `Ok(Some(msg))` 表示成功解码一条消息；`Ok(None)` 表示缓冲区数据不足；
    /// `Err` 表示协议错误。
    pub fn decode(src: &mut BytesMut) -> io::Result<Option<FrontendMessage>> {
        if src.len() < 5 {
            return Ok(None);
        }
        let msg_type = src[0];
        // BUG-001 修复：使用 u32 解析长度，避免 i32 负值经 as usize 符号扩展为 usize::MAX 导致溢出 panic
        let length = u32::from_be_bytes([src[1], src[2], src[3], src[4]]) as usize;
        if length < 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid message length: {length}"),
            ));
        }
        if src.len() < 1 + length {
            return Ok(None);
        }
        // 已确认有完整消息，advance 过 Type + Length
        src.advance(5);
        let payload_len = length - 4;
        let payload = src.split_to(payload_len);

        match msg_type {
            MSG_QUERY => {
                // Query payload 以 \0 结束
                if payload.last() != Some(&0) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "query message missing NUL terminator",
                    ));
                }
                let sql = String::from_utf8_lossy(&payload[..payload.len() - 1]).into_owned();
                Ok(Some(FrontendMessage::Query { sql }))
            }
            MSG_TERMINATE => Ok(Some(FrontendMessage::Terminate)),
            MSG_PASSWORD_OR_SASL => {
                // 'p' 消息在 Phase 4.4 用于 SASL 认证：
                // - SASLInitialResponse: cstring mechanism + i32 length + bytes
                // - SASLResponse: 整个 payload 为响应数据
                // 通过结构启发式区分：若 payload 以 cstring + 有效 i32 长度开始，则解析为
                // SASLInitialResponse；否则解析为 SASLResponse。
                if let Some((mechanism, initial_response)) = try_decode_sasl_initial(&payload)? {
                    Ok(Some(FrontendMessage::SASLInitialResponse {
                        mechanism,
                        initial_response,
                    }))
                } else {
                    Ok(Some(FrontendMessage::SASLResponse {
                        data: payload.to_vec(),
                    }))
                }
            }
            MSG_PARSE => {
                let mut cur = &payload[..];
                let statement_name = read_cstring_from_slice(&mut cur)?;
                let sql = read_cstring_from_slice(&mut cur)?;
                if cur.len() < 2 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "parse message: truncated parameter count",
                    ));
                }
                // BUG-002 修复：Parse 消息同样使用 u16 避免 i16 负值符号扩展
                let param_count = u16::from_be_bytes([cur[0], cur[1]]) as usize;
                cur = &cur[2..];
                if cur.len() < param_count * 4 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "parse message: truncated parameter OID list",
                    ));
                }
                let mut parameter_oids = Vec::with_capacity(param_count);
                for i in 0..param_count {
                    let off = i * 4;
                    parameter_oids.push(u32::from_be_bytes([
                        cur[off],
                        cur[off + 1],
                        cur[off + 2],
                        cur[off + 3],
                    ]));
                }
                Ok(Some(FrontendMessage::Parse {
                    statement_name,
                    sql,
                    parameter_oids,
                }))
            }
            MSG_BIND => {
                let mut cur = &payload[..];
                let portal_name = read_cstring_from_slice(&mut cur)?;
                let statement_name = read_cstring_from_slice(&mut cur)?;

                // parameter format codes
                if cur.len() < 2 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bind message: truncated parameter format code count",
                    ));
                }
                // BUG-006 修复：使用 u16 解析 pfc_count，避免 i16 负值经 as usize 符号扩展
                // 为 usize::MAX 导致 Vec::with_capacity panic（远程 DoS，与 BUG-002 同类）。
                let pfc_count = u16::from_be_bytes([cur[0], cur[1]]) as usize;
                const MAX_BIND_FORMAT_CODES: usize = 65535;
                if pfc_count > MAX_BIND_FORMAT_CODES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("bind message: format code count too large: {pfc_count}"),
                    ));
                }
                cur = &cur[2..];
                if cur.len() < pfc_count * 2 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bind message: truncated parameter format code list",
                    ));
                }
                let mut parameter_format_codes = Vec::with_capacity(pfc_count);
                for i in 0..pfc_count {
                    let off = i * 2;
                    parameter_format_codes.push(i16::from_be_bytes([cur[off], cur[off + 1]]));
                }
                cur = &cur[pfc_count * 2..];

                // parameters
                if cur.len() < 2 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bind message: truncated parameter count",
                    ));
                }
                // BUG-002 修复：使用 u16 解析 param_count，避免 i16 负值经 as usize 符号扩展
                // 为 usize::MAX 导致 Vec::with_capacity panic（远程 DoS）。
                let param_count = u16::from_be_bytes([cur[0], cur[1]]) as usize;
                const MAX_BIND_PARAMS: usize = 65535;
                if param_count > MAX_BIND_PARAMS {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("bind message: parameter count too large: {param_count}"),
                    ));
                }
                cur = &cur[2..];
                let mut parameters = Vec::with_capacity(param_count);
                for _ in 0..param_count {
                    if cur.len() < 4 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "bind message: truncated parameter length",
                        ));
                    }
                    let plen = i32::from_be_bytes([cur[0], cur[1], cur[2], cur[3]]);
                    cur = &cur[4..];
                    if plen < 0 {
                        // NULL 参数
                        parameters.push(None);
                    } else {
                        let plen = plen as usize;
                        if cur.len() < plen {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "bind message: truncated parameter value",
                            ));
                        }
                        parameters.push(Some(cur[..plen].to_vec()));
                        cur = &cur[plen..];
                    }
                }

                // result format codes
                if cur.len() < 2 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bind message: truncated result format code count",
                    ));
                }
                // BUG-006 修复：使用 u16 解析 rfc_count，避免 i16 负值经 as usize 符号扩展
                // 为 usize::MAX 导致 Vec::with_capacity panic（远程 DoS，与 pfc_count 同类）。
                let rfc_count = u16::from_be_bytes([cur[0], cur[1]]) as usize;
                const MAX_RESULT_FORMAT_CODES: usize = 65535;
                if rfc_count > MAX_RESULT_FORMAT_CODES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("bind message: result format code count too large: {rfc_count}"),
                    ));
                }
                cur = &cur[2..];
                if cur.len() < rfc_count * 2 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bind message: truncated result format code list",
                    ));
                }
                let mut result_format_codes = Vec::with_capacity(rfc_count);
                for i in 0..rfc_count {
                    let off = i * 2;
                    result_format_codes.push(i16::from_be_bytes([cur[off], cur[off + 1]]));
                }

                Ok(Some(FrontendMessage::Bind {
                    portal_name,
                    statement_name,
                    parameter_format_codes,
                    parameters,
                    result_format_codes,
                }))
            }
            MSG_EXECUTE => {
                let mut cur = &payload[..];
                let portal_name = read_cstring_from_slice(&mut cur)?;
                if cur.len() < 4 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "execute message: truncated max_rows",
                    ));
                }
                let max_rows = i32::from_be_bytes([cur[0], cur[1], cur[2], cur[3]]);
                Ok(Some(FrontendMessage::Execute {
                    portal_name,
                    max_rows,
                }))
            }
            MSG_DESCRIBE => {
                let mut cur = &payload[..];
                if cur.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "describe message: missing variant byte",
                    ));
                }
                let variant = cur[0];
                cur = &cur[1..];
                let name = read_cstring_from_slice(&mut cur)?;
                Ok(Some(FrontendMessage::Describe { variant, name }))
            }
            MSG_CLOSE => {
                let mut cur = &payload[..];
                if cur.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "close message: missing variant byte",
                    ));
                }
                let variant = cur[0];
                cur = &cur[1..];
                let name = read_cstring_from_slice(&mut cur)?;
                Ok(Some(FrontendMessage::Close { variant, name }))
            }
            MSG_SYNC => Ok(Some(FrontendMessage::Sync)),
            MSG_FLUSH => Ok(Some(FrontendMessage::Flush)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported frontend message type: 0x{:02X}", other),
            )),
        }
    }
}

/// 从字节切片读取以 `\0` 结束的 C 字符串，返回字符串并 advance 切片。
///
/// 用于在已切出的 payload 中解析 cstring 字段，避免 `BytesMut` 上的 split_to 副作用。
fn read_cstring_from_slice(cur: &mut &[u8]) -> io::Result<String> {
    let nul_pos = cur
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing NUL terminator"))?;
    let s = String::from_utf8_lossy(&cur[..nul_pos]).into_owned();
    *cur = &cur[nul_pos + 1..];
    Ok(s)
}

/// Phase 4.4：尝试将 'p' 消息 payload 解析为 SASLInitialResponse。
///
/// SASLInitialResponse 结构：
/// - mechanism: cstring
/// - length: i32（-1 表示无初始响应）
/// - initial_response: length 字节（仅在 length >= 0 时存在）
///
/// 返回 `Ok(Some((mechanism, initial_response)))` 当 payload 严格符合该结构；
/// 返回 `Ok(None)` 当 payload 不符合（应为 SASLResponse）。
fn try_decode_sasl_initial(payload: &[u8]) -> io::Result<Option<(String, Option<Vec<u8>>)>> {
    // 找到第一个 \0 作为 mechanism 的终止符
    let nul_pos = match payload.iter().position(|&b| b == 0) {
        Some(p) => p,
        None => return Ok(None),
    };
    // mechanism 名长度限制（避免误判 SASLResponse 中偶然出现的 \0）
    if nul_pos == 0 || nul_pos > 256 {
        return Ok(None);
    }
    // mechanism 必须为可打印 ASCII（A-Z, a-z, 0-9, -, _）
    let mechanism_bytes = &payload[..nul_pos];
    if !mechanism_bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Ok(None);
    }
    // 检查 length 字段
    if payload.len() < nul_pos + 1 + 4 {
        return Ok(None);
    }
    let length_bytes = &payload[nul_pos + 1..nul_pos + 1 + 4];
    let length = i32::from_be_bytes([
        length_bytes[0],
        length_bytes[1],
        length_bytes[2],
        length_bytes[3],
    ]);
    let mechanism = String::from_utf8_lossy(mechanism_bytes).into_owned();
    if length < 0 {
        // -1 表示无初始响应，payload 应恰好到 length 字段结束
        if payload.len() != nul_pos + 1 + 4 {
            return Ok(None);
        }
        Ok(Some((mechanism, None)))
    } else {
        let length = length as usize;
        let remaining = &payload[nul_pos + 1 + 4..];
        if remaining.len() != length {
            return Ok(None);
        }
        Ok(Some((mechanism, Some(remaining.to_vec()))))
    }
}

// =====================================================================
//  辅助函数：从 BytesMut 读取 C 字符串
// =====================================================================

/// 从 `src` 读取以 `\0` 结束的 C 字符串，返回字符串并 advance 缓冲区。
///
/// 如果未找到 `\0`，返回 `Err`。
pub(crate) fn read_cstring(src: &mut BytesMut) -> io::Result<String> {
    let nul_pos = src
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing NUL terminator"))?;
    let bytes = src.split_to(nul_pos + 1);
    let s = String::from_utf8_lossy(&bytes[..nul_pos]).into_owned();
    Ok(s)
}

/// 将 `s` 以 `\0` 结束的 C 字符串形式写入 `dst`。
pub(crate) fn put_cstring(dst: &mut BytesMut, s: &str) {
    dst.put_slice(s.as_bytes());
    dst.put_u8(0);
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_to_vec(msg: &BackendMessage) -> Vec<u8> {
        msg.to_bytes().to_vec()
    }

    // ---- BackendMessage 编码测试 ----

    #[test]
    fn test_encode_authentication_ok() {
        let bytes = encode_to_vec(&BackendMessage::AuthenticationOk);
        // Type='R'(0x52) + Length=8 + AuthCode=0
        assert_eq!(bytes, vec![b'R', 0, 0, 0, 8, 0, 0, 0, 0]);
    }

    #[test]
    fn test_encode_authentication_sasl() {
        let bytes = encode_to_vec(&BackendMessage::AuthenticationSASL {
            mechanisms: vec!["SCRAM-SHA-256".into()],
        });
        // Type='R' + Length + AuthCode=10 + "SCRAM-SHA-256\0" + "\0"
        // "SCRAM-SHA-256" = 13 字节，payload = 4 (auth_code) + 13 + 1 (NUL) + 1 (终止 NUL) = 19
        // total_len = 19 + 4 = 23
        let mut expected = vec![b'R', 0, 0, 0, 23];
        expected.extend_from_slice(&10u32.to_be_bytes());
        expected.extend_from_slice(b"SCRAM-SHA-256\0");
        expected.push(0);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_encode_authentication_sasl_continue() {
        let data = b"r=abc,s=xyz,i=4096".to_vec();
        let bytes =
            encode_to_vec(&BackendMessage::AuthenticationSASLContinue { data: data.clone() });
        // Type='R' + Length + AuthCode=11 + data
        let total_len = 4 + 4 + data.len();
        let mut expected = vec![b'R'];
        expected.extend_from_slice(&(total_len as i32).to_be_bytes());
        expected.extend_from_slice(&11u32.to_be_bytes());
        expected.extend_from_slice(&data);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_encode_authentication_sasl_final() {
        let data = b"v=base64sig".to_vec();
        let bytes = encode_to_vec(&BackendMessage::AuthenticationSASLFinal { data: data.clone() });
        let total_len = 4 + 4 + data.len();
        let mut expected = vec![b'R'];
        expected.extend_from_slice(&(total_len as i32).to_be_bytes());
        expected.extend_from_slice(&12u32.to_be_bytes());
        expected.extend_from_slice(&data);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_decode_sasl_initial_response_with_data() {
        // 构造 SASLInitialResponse payload
        let mechanism = "SCRAM-SHA-256";
        let initial_response = b"n,,n=user,r=nonce123";
        let mut payload = Vec::new();
        payload.extend_from_slice(mechanism.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&(initial_response.len() as i32).to_be_bytes());
        payload.extend_from_slice(initial_response);

        let (m, ir) = try_decode_sasl_initial(&payload)
            .unwrap()
            .expect("should decode");
        assert_eq!(m, "SCRAM-SHA-256");
        assert_eq!(ir, Some(initial_response.to_vec()));
    }

    #[test]
    fn test_decode_sasl_initial_response_no_data() {
        let mechanism = "SCRAM-SHA-256";
        let mut payload = Vec::new();
        payload.extend_from_slice(mechanism.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&(-1i32).to_be_bytes());

        let (m, ir) = try_decode_sasl_initial(&payload)
            .unwrap()
            .expect("should decode");
        assert_eq!(m, "SCRAM-SHA-256");
        assert_eq!(ir, None);
    }

    #[test]
    fn test_decode_sasl_initial_response_rejects_random_data() {
        // 随机数据不应被解析为 SASLInitialResponse
        let payload = b"random response data without structure";
        assert!(try_decode_sasl_initial(payload).unwrap().is_none());

        // 空数据
        assert!(try_decode_sasl_initial(b"").unwrap().is_none());

        // 只有 \0
        assert!(try_decode_sasl_initial(b"\0").unwrap().is_none());
    }

    #[test]
    fn test_decode_sasl_initial_response_rejects_invalid_mechanism() {
        // mechanism 含非法字符（非 ASCII alphanumeric/-/_）
        let mut payload = Vec::new();
        payload.extend_from_slice(b"BAD MECH");
        payload.push(0);
        payload.extend_from_slice(&0i32.to_be_bytes());
        assert!(try_decode_sasl_initial(&payload).unwrap().is_none());
    }

    #[test]
    fn test_decode_sasl_initial_response_rejects_length_mismatch() {
        let mechanism = "SCRAM-SHA-256";
        let mut payload = Vec::new();
        payload.extend_from_slice(mechanism.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&100i32.to_be_bytes()); // 声明 100 字节
        payload.extend_from_slice(b"only_5"); // 实际只有 6 字节
        assert!(try_decode_sasl_initial(&payload).unwrap().is_none());
    }

    #[test]
    fn test_decode_frontend_sasl_initial_response_message() {
        // 构造完整 'p' 消息：Type + Length + payload
        let mechanism = "SCRAM-SHA-256";
        let initial_response_bytes: &[u8] = b"n,,n=alice,r=abc123";
        let mut payload = Vec::new();
        payload.extend_from_slice(mechanism.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&(initial_response_bytes.len() as i32).to_be_bytes());
        payload.extend_from_slice(initial_response_bytes);

        let mut buf = BytesMut::new();
        buf.put_u8(MSG_PASSWORD_OR_SASL);
        buf.put_i32((payload.len() + 4) as i32);
        buf.extend_from_slice(&payload);

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::SASLInitialResponse {
                mechanism,
                initial_response,
            } => {
                assert_eq!(mechanism, "SCRAM-SHA-256");
                assert_eq!(initial_response, Some(initial_response_bytes.to_vec()));
            }
            other => panic!("expected SASLInitialResponse, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_frontend_sasl_response_message() {
        // 随机字节作为 SASLResponse
        let response_data = b"c=biws,r=nonce,p=proof";
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_PASSWORD_OR_SASL);
        buf.put_i32((response_data.len() + 4) as i32);
        buf.extend_from_slice(response_data);

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::SASLResponse { data } => {
                assert_eq!(data, response_data.to_vec());
            }
            other => panic!("expected SASLResponse, got {other:?}"),
        }
    }

    #[test]
    fn test_encode_parameter_status() {
        let bytes = encode_to_vec(&BackendMessage::ParameterStatus {
            name: "server_version".into(),
            value: "14.0".into(),
        });
        // Type='S' + Length + name\0 + value\0
        // name.len=14 + 1 + value.len=4 + 1 = 20 ; +4 length = 24
        let mut expected = vec![b'S', 0, 0, 0, 24];
        expected.extend_from_slice(b"server_version\0");
        expected.extend_from_slice(b"14.0\0");
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_encode_backend_key_data() {
        let bytes = encode_to_vec(&BackendMessage::BackendKeyData {
            pid: 1234,
            secret_key: -5678,
        });
        // Type='K' + Length=12 + pid(i32) + secret(i32)
        let mut expected = vec![b'K', 0, 0, 0, 12];
        expected.extend_from_slice(&1234i32.to_be_bytes());
        expected.extend_from_slice(&(-5678i32).to_be_bytes());
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_encode_ready_for_query_idle() {
        let bytes = encode_to_vec(&BackendMessage::ReadyForQuery {
            status: STATUS_IDLE,
        });
        assert_eq!(bytes, vec![b'Z', 0, 0, 0, 5, b'I']);
    }

    #[test]
    fn test_encode_ready_for_query_in_transaction() {
        let bytes = encode_to_vec(&BackendMessage::ReadyForQuery {
            status: STATUS_IN_TRANSACTION,
        });
        assert_eq!(bytes, vec![b'Z', 0, 0, 0, 5, b'T']);
    }

    #[test]
    fn test_encode_command_complete() {
        let bytes = encode_to_vec(&BackendMessage::CommandComplete {
            tag: "SELECT 1".into(),
        });
        // Type='C' + Length + tag\0
        // tag.len=8 + 1 = 9 ; +4 = 13
        let mut expected = vec![b'C', 0, 0, 0, 13];
        expected.extend_from_slice(b"SELECT 1\0");
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_encode_empty_query_response() {
        let bytes = encode_to_vec(&BackendMessage::EmptyQueryResponse);
        assert_eq!(bytes, vec![b'I', 0, 0, 0, 4]);
    }

    #[test]
    fn test_encode_error_response_length_correct() {
        let err = ErrorResponse::error(SqlState::SYNTAX_ERROR, "syntax error at or near \"FOO\"");
        let bytes = encode_to_vec(&BackendMessage::ErrorResponse(err));
        assert_eq!(bytes[0], MSG_ERROR_RESPONSE);
        let length = i32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        assert_eq!(bytes.len() as i32, 1 + length);
        // 验证 payload 包含必要字段
        let payload = &bytes[5..];
        // payload 以 'S' + 'ERROR\0' 开始
        assert_eq!(&payload[..6], b"SERROR");
    }

    #[test]
    fn test_encode_error_response_contains_sqlstate() {
        let err = ErrorResponse::fatal(SqlState::PROTOCOL_VIOLATION, "bad proto");
        let bytes = encode_to_vec(&BackendMessage::ErrorResponse(err));
        // 找 'C' 字段
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("08P01"), "should contain SQLSTATE 08P01: {s}");
        assert!(s.contains("bad proto"));
        assert!(s.contains("FATAL"));
    }

    // ---- FrontendMessage 解码测试 ----

    #[test]
    fn test_decode_query_message() {
        // 构造 Query 消息：'Q' + Length + "SELECT 1\0"
        let sql = "SELECT 1";
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_QUERY);
        buf.put_i32((sql.len() + 1 + 4) as i32);
        buf.put_slice(sql.as_bytes());
        buf.put_u8(0);

        let mut src = buf.clone();
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::Query { sql: decoded } => assert_eq!(decoded, "SELECT 1"),
            other => panic!("expected Query, got {other:?}"),
        }
        // 缓冲区应被消费
        assert!(src.is_empty(), "src should be consumed, remaining: {src:?}");
    }

    #[test]
    fn test_decode_terminate_message() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_TERMINATE);
        buf.put_i32(4);
        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        assert_eq!(msg, FrontendMessage::Terminate);
        assert!(src.is_empty());
    }

    #[test]
    fn test_decode_returns_none_when_incomplete() {
        // 只有 Type 字节
        let mut src = BytesMut::from(&b"Q"[..]);
        assert!(FrontendMessage::decode(&mut src).unwrap().is_none());

        // Type + Length 但 payload 不足
        let mut src = BytesMut::new();
        src.put_u8(MSG_QUERY);
        src.put_i32(100); // 声明 100 字节 payload
        src.put_slice(b"SEL"); // 只有 3 字节
        assert!(FrontendMessage::decode(&mut src).unwrap().is_none());
    }

    #[test]
    fn test_decode_rejects_invalid_length() {
        let mut src = BytesMut::new();
        src.put_u8(MSG_QUERY);
        src.put_i32(2); // length < 4 非法
        let err = FrontendMessage::decode(&mut src).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_decode_rejects_unknown_message_type() {
        let mut src = BytesMut::new();
        src.put_u8(b'?'); // 未知类型
        src.put_i32(4);
        let err = FrontendMessage::decode(&mut src).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_decode_rejects_query_without_nul() {
        let mut src = BytesMut::new();
        src.put_u8(MSG_QUERY);
        src.put_i32(8); // length=8 表示 4 字节 payload
        src.put_slice(b"abcd"); // 无 NUL
        let err = FrontendMessage::decode(&mut src).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    // ---- Severity / SqlState ----

    #[test]
    fn test_severity_as_str() {
        assert_eq!(Severity::Error.as_str(), "ERROR");
        assert_eq!(Severity::Fatal.as_str(), "FATAL");
        assert_eq!(Severity::Panic.as_str(), "PANIC");
        assert_eq!(Severity::Warning.as_str(), "WARNING");
        assert_eq!(Severity::Notice.as_str(), "NOTICE");
    }

    #[test]
    fn test_sqlstate_as_str() {
        assert_eq!(SqlState::SYNTAX_ERROR.as_str(), "42601");
        assert_eq!(SqlState::PROTOCOL_VIOLATION.as_str(), "08P01");
        assert_eq!(SqlState::SUCCESSFUL_COMPLETION.as_str(), "00000");
    }

    // ---- 辅助函数测试 ----

    #[test]
    fn test_read_cstring() {
        let mut src = BytesMut::from(&b"hello\0world"[..]);
        let s = read_cstring(&mut src).unwrap();
        assert_eq!(s, "hello");
        assert_eq!(src.as_ref(), b"world");
    }

    #[test]
    fn test_read_cstring_missing_nul() {
        let mut src = BytesMut::from(&b"hello"[..]);
        let err = read_cstring(&mut src).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_put_cstring() {
        let mut dst = BytesMut::new();
        put_cstring(&mut dst, "hi");
        assert_eq!(dst.as_ref(), b"hi\0");
    }

    // ---- 多消息连续编码 ----

    #[test]
    fn test_multiple_messages_concatenated() {
        let mut buf = BytesMut::new();
        BackendMessage::AuthenticationOk.encode(&mut buf);
        BackendMessage::ParameterStatus {
            name: "client_encoding".into(),
            value: "UTF8".into(),
        }
        .encode(&mut buf);
        BackendMessage::ReadyForQuery {
            status: STATUS_IDLE,
        }
        .encode(&mut buf);

        // 第一条：R + 8 字节
        assert_eq!(&buf[..9], &[b'R', 0, 0, 0, 8, 0, 0, 0, 0]);
        // 第二条起始位置 = 9
        assert_eq!(buf[9], b'S');
        // 最后一条消息的 Type='Z' 在末尾倒数 6 字节（Z + length(4) + status(1)）
        assert_eq!(buf[buf.len() - 6], b'Z');
        // 最末字节是 status='I'
        assert_eq!(buf.last(), Some(&b'I'));
    }

    // ---- 错误响应 round-trip ----

    #[test]
    fn test_error_response_fields_complete() {
        let err = ErrorResponse::error(SqlState::INTERNAL_ERROR, "boom");
        let bytes = encode_to_vec(&BackendMessage::ErrorResponse(err));
        // 验证 payload 包含 S/V/C/M 四个字段，并以 \0\0 结束
        let payload = &bytes[5..];
        assert!(payload.starts_with(b"SERROR\0"));
        assert!(payload.ends_with(b"\0\0"));
        // 找 'C' 字段
        let payload_str: Vec<u8> = payload.to_vec();
        assert!(payload_str.windows(7).any(|w| *w == *b"CXX000\0"));
        // 找 'M' 字段
        assert!(payload_str.windows(6).any(|w| *w == *b"Mboom\0"));
    }

    // ---- Phase 4.3 扩展查询：BackendMessage 编码测试 ----

    #[test]
    fn test_encode_parse_complete() {
        let bytes = encode_to_vec(&BackendMessage::ParseComplete);
        // Type='1' + Length=4
        assert_eq!(bytes, vec![b'1', 0, 0, 0, 4]);
    }

    #[test]
    fn test_encode_bind_complete() {
        let bytes = encode_to_vec(&BackendMessage::BindComplete);
        assert_eq!(bytes, vec![b'2', 0, 0, 0, 4]);
    }

    #[test]
    fn test_encode_close_complete() {
        let bytes = encode_to_vec(&BackendMessage::CloseComplete);
        assert_eq!(bytes, vec![b'3', 0, 0, 0, 4]);
    }

    #[test]
    fn test_encode_portal_suspended() {
        let bytes = encode_to_vec(&BackendMessage::PortalSuspended);
        assert_eq!(bytes, vec![b's', 0, 0, 0, 4]);
    }

    #[test]
    fn test_encode_no_data() {
        let bytes = encode_to_vec(&BackendMessage::NoData);
        assert_eq!(bytes, vec![b'n', 0, 0, 0, 4]);
    }

    #[test]
    fn test_encode_parameter_description_empty() {
        let bytes = encode_to_vec(&BackendMessage::ParameterDescription {
            parameter_oids: vec![],
        });
        // Type='t' + Length=6 (4 + 2 for count=0)
        assert_eq!(bytes, vec![b't', 0, 0, 0, 6, 0, 0]);
    }

    #[test]
    fn test_encode_parameter_description_with_oids() {
        let bytes = encode_to_vec(&BackendMessage::ParameterDescription {
            parameter_oids: vec![20, 25], // INT8, TEXT
        });
        // Type='t' + Length=14 (4 + 2 count + 2*4 OIDs) + count=2 + OID1 + OID2
        assert_eq!(bytes[0], b't');
        let length = i32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        assert_eq!(length, 14);
        let count = i16::from_be_bytes([bytes[5], bytes[6]]);
        assert_eq!(count, 2);
        let oid1 = u32::from_be_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]);
        assert_eq!(oid1, 20);
        let oid2 = u32::from_be_bytes([bytes[11], bytes[12], bytes[13], bytes[14]]);
        assert_eq!(oid2, 25);
    }

    // ---- Phase 4.3 扩展查询：FrontendMessage 解码测试 ----

    #[test]
    fn test_decode_parse_message_no_params() {
        // Parse: statement_name="" + sql="SELECT $1" + param_count=0
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_PARSE);
        // payload: "" \0 (1) + "SELECT $1" \0 (10) + count(2) = 13
        let payload_len = 1 + 10 + 2;
        buf.put_i32(payload_len + 4);
        buf.put_u8(0); // statement_name=""
        buf.put_slice(b"SELECT $1");
        buf.put_u8(0); // sql
        buf.put_i16(0); // param count

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::Parse {
                statement_name,
                sql,
                parameter_oids,
            } => {
                assert_eq!(statement_name, "");
                assert_eq!(sql, "SELECT $1");
                assert!(parameter_oids.is_empty());
            }
            other => panic!("expected Parse, got {other:?}"),
        }
        assert!(src.is_empty());
    }

    #[test]
    fn test_decode_parse_message_with_params() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_PARSE);
        // payload: "stmt1\0" (6) + "SELECT $1, $2\0" (14) + count(2) + OID1(4) + OID2(4) = 30
        let payload_len = 6 + 14 + 2 + 4 + 4;
        buf.put_i32(payload_len + 4);
        buf.put_slice(b"stmt1\0");
        buf.put_slice(b"SELECT $1, $2\0");
        buf.put_i16(2);
        buf.put_u32(20); // INT8
        buf.put_u32(25); // TEXT

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::Parse {
                statement_name,
                sql,
                parameter_oids,
            } => {
                assert_eq!(statement_name, "stmt1");
                assert_eq!(sql, "SELECT $1, $2");
                assert_eq!(parameter_oids, vec![20, 25]);
            }
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_bind_message_text_params() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_BIND);
        // portal="" \0 + stmt="" \0 + pfc_count(2)=0 + param_count(2)=2
        //   param1: len(4)=5 + "hello"
        //   param2: len(4)=-1 (NULL)
        // rfc_count(2)=0
        let payload_len = 1 + 1 + 2 + 2 + (4 + 5) + 4 + 2;
        buf.put_i32(payload_len + 4);
        buf.put_u8(0); // portal_name=""
        buf.put_u8(0); // statement_name=""
        buf.put_i16(0); // parameter_format_codes count=0 (default text)
        buf.put_i16(2); // parameter count=2
        buf.put_i32(5); // param1 length
        buf.put_slice(b"hello");
        buf.put_i32(-1); // param2 NULL
        buf.put_i16(0); // result_format_codes count=0 (default text)

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::Bind {
                portal_name,
                statement_name,
                parameter_format_codes,
                parameters,
                result_format_codes,
            } => {
                assert_eq!(portal_name, "");
                assert_eq!(statement_name, "");
                assert!(parameter_format_codes.is_empty());
                assert_eq!(parameters.len(), 2);
                assert_eq!(parameters[0], Some(b"hello".to_vec()));
                assert_eq!(parameters[1], None);
                assert!(result_format_codes.is_empty());
            }
            other => panic!("expected Bind, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_execute_message() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_EXECUTE);
        // payload: "portal\0"(7) + max_rows(4) = 11，length = 11 + 4 = 15
        buf.put_i32(7 + 4 + 4);
        buf.put_slice(b"portal\0");
        buf.put_i32(100);

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::Execute {
                portal_name,
                max_rows,
            } => {
                assert_eq!(portal_name, "portal");
                assert_eq!(max_rows, 100);
            }
            other => panic!("expected Execute, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_describe_statement() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_DESCRIBE);
        buf.put_i32(1 + 6 + 4); // variant(1) + "stmt1\0" (6) + 4
        buf.put_u8(DESCRIBE_STATEMENT);
        buf.put_slice(b"stmt1\0");

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::Describe { variant, name } => {
                assert_eq!(variant, DESCRIBE_STATEMENT);
                assert_eq!(name, "stmt1");
            }
            other => panic!("expected Describe, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_describe_portal() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_DESCRIBE);
        buf.put_i32(1 + 7 + 4); // variant(1) + "portal\0" (7) + 4
        buf.put_u8(DESCRIBE_PORTAL);
        buf.put_slice(b"portal\0");

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::Describe { variant, name } => {
                assert_eq!(variant, DESCRIBE_PORTAL);
                assert_eq!(name, "portal");
            }
            other => panic!("expected Describe, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_close_statement() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_CLOSE);
        buf.put_i32(1 + 6 + 4);
        buf.put_u8(CLOSE_STATEMENT);
        buf.put_slice(b"stmt1\0");

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::Close { variant, name } => {
                assert_eq!(variant, CLOSE_STATEMENT);
                assert_eq!(name, "stmt1");
            }
            other => panic!("expected Close, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_sync_message() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_SYNC);
        buf.put_i32(4);

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        assert_eq!(msg, FrontendMessage::Sync);
        assert!(src.is_empty());
    }

    #[test]
    fn test_decode_flush_message() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_FLUSH);
        buf.put_i32(4);

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        assert_eq!(msg, FrontendMessage::Flush);
    }

    #[test]
    fn test_decode_bind_rejects_truncated_parameter() {
        // 构造一个声明长度与实际一致、但 payload 内部声称 param_count=1
        // 却没有提供参数长度字节的 Bind 消息
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_BIND);
        // payload: portal\0(1) + stmt\0(1) + pfc_count(2) + param_count(2) = 6 字节
        let payload_len = 1 + 1 + 2 + 2;
        buf.put_i32(payload_len + 4);
        buf.put_u8(0); // portal
        buf.put_u8(0); // statement
        buf.put_i16(0); // pfc count
        buf.put_i16(1); // param count=1（但后续没有参数长度字节）

        let mut src = buf;
        let err = FrontendMessage::decode(&mut src).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("truncated parameter length"));
    }

    #[test]
    fn test_decode_parse_rejects_truncated_oid_list() {
        // 构造一个声明长度与实际一致、但 payload 内部声称 param_count=2
        // 却只提供 1 个 OID 的 Parse 消息
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_PARSE);
        // payload: "s\0"(2) + "SELECT 1\0"(9) + count(2) + 1*OID(4) = 17 字节
        let payload_len = 2 + 9 + 2 + 4;
        buf.put_i32(payload_len + 4);
        buf.put_slice(b"s\0");
        buf.put_slice(b"SELECT 1\0");
        buf.put_i16(2); // 声明 2 个 OID
        buf.put_u32(20); // 只提供 1 个

        let mut src = buf;
        let err = FrontendMessage::decode(&mut src).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("truncated parameter OID list"));
    }
}
