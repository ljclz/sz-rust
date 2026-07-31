//! MySQL 握手协议 — HandshakeV10 + HandshakeResponse41。
//!
//! 握手流程：
//! ```text
//! Server → Client: HandshakeV10（协议版本、服务器版本、salt 前半、能力标志）
//! Client → Server: HandshakeResponse41（用户名、auth_response、数据库、能力标志）
//! Server → Client: OK / ERR
//! ```
//!
//! 详见 MySQL 文档 "Protocol::HandshakeV10" 和 "Protocol::HandshakeResponse41"。

use crate::auth::SALT_LEN;
use crate::packet::{
    read_lenenc_string, read_nul_string, write_lenenc_int, write_nul_string,
};
use thiserror::Error;

/// 服务器能力标志位（部分）。
pub const CLIENT_LONG_PASSWORD: u32 = 1;
pub const CLIENT_FOUND_ROWS: u32 = 2;
pub const CLIENT_LONG_FLAG: u32 = 4;
pub const CLIENT_CONNECT_WITH_DB: u32 = 8;
pub const CLIENT_PROTOCOL_41: u32 = 1 << 9;
pub const CLIENT_SECURE_CONNECTION: u32 = 1 << 15;
pub const CLIENT_PLUGIN_AUTH: u32 = 1 << 19;
pub const CLIENT_DEPRECATE_EOF: u32 = 1 << 24;

/// 服务器默认能力（CLIENT_PROTOCOL_41 | CLIENT_SECURE_CONNECTION | CLIENT_PLUGIN_AUTH）。
pub const SERVER_DEFAULT_CAPABILITIES: u32 =
    CLIENT_PROTOCOL_41 | CLIENT_SECURE_CONNECTION | CLIENT_PLUGIN_AUTH | CLIENT_LONG_PASSWORD;

/// 字符集：utf8mb4 (45)。
pub const CHARSET_UTF8MB4: u8 = 45;

/// 状态标志位：SERVER_STATUS_AUTOCOMMIT。
pub const SERVER_STATUS_AUTOCOMMIT: u16 = 0x0002;

/// 状态标志位：SERVER_MORE_RESULTS_EXISTS（多语句查询中还有更多结果集）。
pub const SERVER_MORE_RESULTS_EXISTS: u16 = 0x0008;

/// 握手错误。
#[derive(Debug, Error)]
pub enum HandshakeError {
    /// IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// 协议格式错误
    #[error("protocol error: {0}")]
    Protocol(String),
    /// 包解析错误
    #[error("packet error: {0}")]
    Packet(#[from] crate::packet::PacketError),
}

/// HandshakeV10：服务器发送给客户端的握手包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeV10 {
    /// 协议版本（固定为 10）
    pub protocol_version: u8,
    /// 服务器版本字符串（如 "8.0.32-szrsql"）
    pub server_version: String,
    /// 连接 ID（4 字节）
    pub connection_id: u32,
    /// 认证 salt（前 8 字节）
    pub auth_plugin_data_part_1: [u8; 8],
    /// 服务器能力标志（低 16 位）
    pub capability_flags_1: u16,
    /// 字符集
    pub character_set: u8,
    /// 状态标志
    pub status_flags: u16,
    /// 服务器能力标志（高 16 位）
    pub capability_flags_2: u16,
    /// 认证插件数据长度（如果 CLIENT_PLUGIN_AUTH）
    pub auth_plugin_data_len: u8,
    /// 保留字段（10 字节，全 0）
    pub reserved: [u8; 10],
    /// 认证 salt（后 12 字节 + 1 字节 NUL）
    pub auth_plugin_data_part_2: Vec<u8>,
    /// 认证插件名（如 "mysql_native_password"）
    pub auth_plugin_name: String,
}

impl HandshakeV10 {
    /// 构造新的 HandshakeV10。
    pub fn new(
        server_version: impl Into<String>,
        connection_id: u32,
        salt: &[u8; SALT_LEN],
    ) -> Self {
        let server_version = server_version.into();
        // salt 前 8 字节
        let mut part_1 = [0u8; 8];
        part_1.copy_from_slice(&salt[..8]);
        // salt 后 12 字节
        let part_2 = salt[8..SALT_LEN].to_vec();

        Self {
            protocol_version: 10,
            server_version,
            connection_id,
            auth_plugin_data_part_1: part_1,
            capability_flags_1: (SERVER_DEFAULT_CAPABILITIES & 0xFFFF) as u16,
            character_set: CHARSET_UTF8MB4,
            status_flags: SERVER_STATUS_AUTOCOMMIT,
            capability_flags_2: ((SERVER_DEFAULT_CAPABILITIES >> 16) & 0xFFFF) as u16,
            auth_plugin_data_len: (SALT_LEN + 1) as u8,
            reserved: [0u8; 10],
            auth_plugin_data_part_2: part_2,
            auth_plugin_name: "mysql_native_password".to_string(),
        }
    }

    /// 编码为字节序列（payload 部分，不含包头）。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);
        // 1. 协议版本
        buf.push(self.protocol_version);
        // 2. 服务器版本（NUL 结尾）
        write_nul_string(&mut buf, &self.server_version);
        // 3. 连接 ID（4 字节小端）
        buf.extend_from_slice(&self.connection_id.to_le_bytes());
        // 4. auth_plugin_data_part_1（8 字节）
        buf.extend_from_slice(&self.auth_plugin_data_part_1);
        // 5. 填充字节（1 字节 0）
        buf.push(0);
        // 6. capability_flags_1（2 字节小端）
        buf.extend_from_slice(&self.capability_flags_1.to_le_bytes());
        // 7. 字符集（1 字节）
        buf.push(self.character_set);
        // 8. status_flags（2 字节小端）
        buf.extend_from_slice(&self.status_flags.to_le_bytes());
        // 9. capability_flags_2（2 字节小端）
        buf.extend_from_slice(&self.capability_flags_2.to_le_bytes());
        // 10. auth_plugin_data_len（1 字节，如果 CLIENT_PLUGIN_AUTH）
        buf.push(self.auth_plugin_data_len);
        // 11. 保留字段（10 字节）
        buf.extend_from_slice(&self.reserved);
        // 12. auth_plugin_data_part_2（12 字节 + 1 字节 NUL）
        buf.extend_from_slice(&self.auth_plugin_data_part_2);
        buf.push(0);
        // 13. auth_plugin_name（NUL 结尾，如果 CLIENT_PLUGIN_AUTH）
        write_nul_string(&mut buf, &self.auth_plugin_name);
        buf
    }

    /// 从 payload 字节序列解析。
    pub fn decode(payload: &[u8]) -> Result<Self, HandshakeError> {
        let mut buf = payload;
        if buf.len() < 4 {
            return Err(HandshakeError::Protocol("handshake too short".to_string()));
        }

        // 1. 协议版本
        let protocol_version = buf[0];
        buf = &buf[1..];
        if protocol_version != 10 {
            return Err(HandshakeError::Protocol(format!(
                "unsupported protocol version: {protocol_version}"
            )));
        }

        // 2. 服务器版本
        let server_version =
            read_nul_string(&mut buf).ok_or_else(|| HandshakeError::Protocol(
                "missing server version".to_string(),
            ))?;

        // 3. 连接 ID
        if buf.len() < 4 {
            return Err(HandshakeError::Protocol("missing connection id".to_string()));
        }
        let connection_id = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        buf = &buf[4..];

        // 4. auth_plugin_data_part_1
        if buf.len() < 8 {
            return Err(HandshakeError::Protocol(
                "missing auth_plugin_data_part_1".to_string(),
            ));
        }
        let mut auth_plugin_data_part_1 = [0u8; 8];
        auth_plugin_data_part_1.copy_from_slice(&buf[..8]);
        buf = &buf[8..];

        // 5. 填充字节
        if buf.is_empty() {
            return Err(HandshakeError::Protocol(
                "missing filler byte".to_string(),
            ));
        }
        buf = &buf[1..];

        // 6. capability_flags_1
        if buf.len() < 2 {
            return Err(HandshakeError::Protocol(
                "missing capability_flags_1".to_string(),
            ));
        }
        let capability_flags_1 = u16::from_le_bytes([buf[0], buf[1]]);
        buf = &buf[2..];

        // 7. 字符集
        if buf.is_empty() {
            return Err(HandshakeError::Protocol("missing character_set".to_string()));
        }
        let character_set = buf[0];
        buf = &buf[1..];

        // 8. status_flags
        if buf.len() < 2 {
            return Err(HandshakeError::Protocol("missing status_flags".to_string()));
        }
        let status_flags = u16::from_le_bytes([buf[0], buf[1]]);
        buf = &buf[2..];

        // 9. capability_flags_2
        if buf.len() < 2 {
            return Err(HandshakeError::Protocol(
                "missing capability_flags_2".to_string(),
            ));
        }
        let capability_flags_2 = u16::from_le_bytes([buf[0], buf[1]]);
        buf = &buf[2..];

        // 10. auth_plugin_data_len
        if buf.is_empty() {
            return Err(HandshakeError::Protocol(
                "missing auth_plugin_data_len".to_string(),
            ));
        }
        let auth_plugin_data_len = buf[0];
        buf = &buf[1..];

        // 11. 保留字段（10 字节）
        if buf.len() < 10 {
            return Err(HandshakeError::Protocol("missing reserved".to_string()));
        }
        let mut reserved = [0u8; 10];
        reserved.copy_from_slice(&buf[..10]);
        buf = &buf[10..];

        // 12. auth_plugin_data_part_2（max(13, auth_plugin_data_len - 8) 字节）
        let part_2_len = if auth_plugin_data_len > 8 {
            (auth_plugin_data_len - 8) as usize
        } else {
            13
        };
        let part_2_len = part_2_len.max(13);
        if buf.len() < part_2_len {
            return Err(HandshakeError::Protocol(
                "missing auth_plugin_data_part_2".to_string(),
            ));
        }
        // 移除末尾 NUL
        let mut auth_plugin_data_part_2 = buf[..part_2_len.saturating_sub(1)].to_vec();
        if auth_plugin_data_part_2.len() > 12 {
            auth_plugin_data_part_2.truncate(12);
        }
        buf = &buf[part_2_len..];

        // 13. auth_plugin_name
        let auth_plugin_name = read_nul_string(&mut buf).unwrap_or_default();

        Ok(Self {
            protocol_version,
            server_version,
            connection_id,
            auth_plugin_data_part_1,
            capability_flags_1,
            character_set,
            status_flags,
            capability_flags_2,
            auth_plugin_data_len,
            reserved,
            auth_plugin_data_part_2,
            auth_plugin_name,
        })
    }
}

/// HandshakeResponse41：客户端发送给服务器的握手响应包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeResponse41 {
    /// 客户端能力标志
    pub capability_flags: u32,
    /// 最大包大小
    pub max_packet_size: u32,
    /// 字符集
    pub character_set: u8,
    /// 保留字段（23 字节，全 0）
    pub reserved: [u8; 23],
    /// 用户名（NUL 结尾）
    pub username: String,
    /// 认证响应（长度编码）
    pub auth_response: Vec<u8>,
    /// 数据库名（可选，如果 CLIENT_CONNECT_WITH_DB）
    pub database: Option<String>,
    /// 认证插件名（可选，如果 CLIENT_PLUGIN_AUTH）
    pub auth_plugin_name: String,
}

impl HandshakeResponse41 {
    /// 编码为字节序列。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);
        // 1. capability_flags（4 字节小端）
        buf.extend_from_slice(&self.capability_flags.to_le_bytes());
        // 2. max_packet_size（4 字节小端）
        buf.extend_from_slice(&self.max_packet_size.to_le_bytes());
        // 3. character_set（1 字节）
        buf.push(self.character_set);
        // 4. 保留字段（23 字节）
        buf.extend_from_slice(&self.reserved);
        // 5. username（NUL 结尾）
        write_nul_string(&mut buf, &self.username);
        // 6. auth_response（长度编码，如果 CLIENT_PLUGIN_AUTH）
        if self.capability_flags & CLIENT_PLUGIN_AUTH != 0 {
            write_lenenc_string_inline(&mut buf, &self.auth_response);
        } else {
            buf.push(self.auth_response.len() as u8);
            buf.extend_from_slice(&self.auth_response);
        }
        // 7. database（可选）
        if self.capability_flags & CLIENT_CONNECT_WITH_DB != 0 {
            if let Some(db) = &self.database {
                write_nil_string(&mut buf, db);
            }
        }
        // 8. auth_plugin_name（可选）
        if self.capability_flags & CLIENT_PLUGIN_AUTH != 0 {
            write_nil_string(&mut buf, &self.auth_plugin_name);
        }
        buf
    }

    /// 从 payload 解析。
    pub fn decode(payload: &[u8]) -> Result<Self, HandshakeError> {
        let mut buf = payload;
        if buf.len() < 32 {
            return Err(HandshakeError::Protocol(
                "handshake response too short".to_string(),
            ));
        }

        // 1. capability_flags
        let capability_flags = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        buf = &buf[4..];

        // 2. max_packet_size
        let max_packet_size = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        buf = &buf[4..];

        // 3. character_set
        let character_set = buf[0];
        buf = &buf[1..];

        // 4. reserved
        let mut reserved = [0u8; 23];
        reserved.copy_from_slice(&buf[..23]);
        buf = &buf[23..];

        // 5. username
        let username =
            read_nul_string(&mut buf).ok_or_else(|| HandshakeError::Protocol(
                "missing username".to_string(),
            ))?;

        // 6. auth_response
        let auth_response = if capability_flags & CLIENT_PLUGIN_AUTH != 0 {
            read_lenenc_string(&mut buf)
                .ok_or_else(|| HandshakeError::Protocol("missing auth_response".to_string()))?
        } else {
            if buf.is_empty() {
                return Err(HandshakeError::Protocol(
                    "missing auth_response length".to_string(),
                ));
            }
            let len = buf[0] as usize;
            buf = &buf[1..];
            if buf.len() < len {
                return Err(HandshakeError::Protocol(
                    "auth_response truncated".to_string(),
                ));
            }
            let r = buf[..len].to_vec();
            buf = &buf[len..];
            r
        };

        // 7. database（可选）+ 8. auth_plugin_name（可选）
        //
        // **MySQL 协议兼容性修复**：
        // 部分客户端（如 aiomysql）设置了 CLIENT_CONNECT_WITH_DB 标志但实际未发送 database 字段，
        // 仅发送 auth_plugin_name。这导致服务器把 auth_plugin_name（如 "mysql_native_password"）
        // 误读为 database。
        //
        // 修复策略：
        // - 若 CONNECT_WITH_DB 和 PLUGIN_AUTH 都设置：先读 database，若读完无剩余数据，
        //   说明客户端实际未发 database，把刚才读到的值作为 auth_plugin_name。
        // - 若仅 CONNECT_WITH_DB：正常读 database。
        // - 若仅 PLUGIN_AUTH：正常读 auth_plugin_name。
        let (database, auth_plugin_name) =
            if capability_flags & CLIENT_CONNECT_WITH_DB != 0
                && capability_flags & CLIENT_PLUGIN_AUTH != 0
            {
                // 两者都设置：先读 database
                let db_candidate = read_nul_string(&mut buf);
                // 读完 database 后，buf 应该还有 auth_plugin_name（以 NUL 结尾）
                if buf.is_empty() {
                    // 没有剩余数据：客户端实际未发 database，db_candidate 就是 auth_plugin_name
                    let plugin = db_candidate.unwrap_or_default();
                    tracing::debug!(
                        target: "mysql_handshake",
                        plugin = %plugin,
                        "CONNECT_WITH_DB set but no database sent; treating value as auth_plugin_name"
                    );
                    (None, plugin)
                } else {
                    // 有剩余数据：db_candidate 是真 database，继续读 auth_plugin_name
                    let plugin = read_nul_string(&mut buf).unwrap_or_default();
                    (db_candidate, plugin)
                }
            } else if capability_flags & CLIENT_CONNECT_WITH_DB != 0 {
                // 仅 CONNECT_WITH_DB：读 database
                (read_nul_string(&mut buf), String::new())
            } else if capability_flags & CLIENT_PLUGIN_AUTH != 0 {
                // 仅 PLUGIN_AUTH：读 auth_plugin_name
                (None, read_nul_string(&mut buf).unwrap_or_default())
            } else {
                (None, String::new())
            };

        Ok(Self {
            capability_flags,
            max_packet_size,
            character_set,
            reserved,
            username,
            auth_response,
            database,
            auth_plugin_name,
        })
    }
}

/// OK 包（用于认证成功后回复）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkPacket {
    /// 受影响行数
    pub affected_rows: u64,
    /// 最后插入 ID
    pub last_insert_id: u64,
    /// 状态标志
    pub status_flags: u16,
    /// 警告数
    pub warnings: u16,
    /// 信息字符串
    pub info: String,
}

impl OkPacket {
    /// 编码为 payload（CLIENT_PROTOCOL_41 + CLIENT_SECURE_CONNECTION）。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32);
        // 0x00 标识 OK 包
        buf.push(0x00);
        // affected_rows（lenenc）
        write_lenenc_int(&mut buf, self.affected_rows);
        // last_insert_id（lenenc）
        write_lenenc_int(&mut buf, self.last_insert_id);
        // status_flags（2 字节）
        buf.extend_from_slice(&self.status_flags.to_le_bytes());
        // warnings（2 字节）
        buf.extend_from_slice(&self.warnings.to_le_bytes());
        // info
        buf.extend_from_slice(self.info.as_bytes());
        buf
    }

    /// 构造一个简单的 OK 包。
    pub fn simple() -> Self {
        Self {
            affected_rows: 0,
            last_insert_id: 0,
            status_flags: SERVER_STATUS_AUTOCOMMIT,
            warnings: 0,
            info: String::new(),
        }
    }
}

/// ERR 包（用于认证失败、SQL 错误等）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrPacket {
    /// 错误码
    pub error_code: u16,
    /// SQL 状态标记（'#'）
    pub sql_state_marker: u8,
    /// SQL 状态（5 字节）
    pub sql_state: [u8; 5],
    /// 错误消息
    pub error_message: String,
}

impl ErrPacket {
    /// 创建新的 ERR 包。
    pub fn new(error_code: u16, sql_state: [u8; 5], error_message: impl Into<String>) -> Self {
        Self {
            error_code,
            sql_state_marker: b'#',
            sql_state,
            error_message: error_message.into(),
        }
    }

    /// 编码为 payload。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32);
        // 0xFF 标识 ERR 包
        buf.push(0xFF);
        // error_code（2 字节小端）
        buf.extend_from_slice(&self.error_code.to_le_bytes());
        // sql_state_marker（'#'）
        buf.push(self.sql_state_marker);
        // sql_state（5 字节）
        buf.extend_from_slice(&self.sql_state);
        // error_message
        buf.extend_from_slice(self.error_message.as_bytes());
        buf
    }
}

/// 常用错误码。
pub mod error_codes {
    /// ER_ACCESS_DENIED_ERROR
    pub const ACCESS_DENIED: u16 = 1045;
    /// ER_BAD_DB_ERROR
    pub const BAD_DB: u16 = 1049;
    /// ER_PARSE_ERROR
    pub const PARSE_ERROR: u16 = 1064;
    /// ER_NO_SUCH_TABLE
    pub const NO_SUCH_TABLE: u16 = 1146;
    /// ER_INTERNAL_ERROR
    pub const INTERNAL_ERROR: u16 = 1815;
}

/// 常用 SQL 状态。
pub mod sql_states {
    /// 28000：认证失败
    pub const ACCESS_DENIED: [u8; 5] = *b"28000";
    /// 42000：语法错误
    pub const SYNTAX_ERROR: [u8; 5] = *b"42000";
    /// 42S02：表不存在
    pub const TABLE_NOT_FOUND: [u8; 5] = *b"42S02";
    /// HY000：通用错误
    pub const GENERAL: [u8; 5] = *b"HY000";
}

// 内部辅助函数
fn write_lenenc_string_inline(buf: &mut Vec<u8>, s: &[u8]) {
    write_lenenc_int(buf, s.len() as u64);
    buf.extend_from_slice(s);
}

fn write_nil_string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handshake_v10_roundtrip() {
        let salt = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
        ];
        let original = HandshakeV10::new("8.0.32-szrsql", 42, &salt);
        let encoded = original.encode();
        let decoded = HandshakeV10::decode(&encoded).unwrap();
        assert_eq!(decoded.protocol_version, 10);
        assert_eq!(decoded.server_version, "8.0.32-szrsql");
        assert_eq!(decoded.connection_id, 42);
        assert_eq!(decoded.character_set, CHARSET_UTF8MB4);
        assert_eq!(decoded.auth_plugin_name, "mysql_native_password");
    }

    #[test]
    fn test_handshake_v10_salt_split_correctly() {
        let salt = [
            10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160, 170, 180, 190,
            200,
        ];
        let handshake = HandshakeV10::new("test", 1, &salt);
        // 前 8 字节
        assert_eq!(handshake.auth_plugin_data_part_1, [10, 20, 30, 40, 50, 60, 70, 80]);
        // 后 12 字节
        assert_eq!(
            handshake.auth_plugin_data_part_2,
            vec![90, 100, 110, 120, 130, 140, 150, 160, 170, 180, 190, 200]
        );
    }

    #[test]
    fn test_handshake_response_roundtrip() {
        let original = HandshakeResponse41 {
            capability_flags: CLIENT_PROTOCOL_41 | CLIENT_PLUGIN_AUTH,
            max_packet_size: 16777216,
            character_set: CHARSET_UTF8MB4,
            reserved: [0u8; 23],
            username: "root".to_string(),
            auth_response: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20],
            database: None,
            auth_plugin_name: "mysql_native_password".to_string(),
        };
        let encoded = original.encode();
        let decoded = HandshakeResponse41::decode(&encoded).unwrap();
        assert_eq!(decoded.username, "root");
        assert_eq!(decoded.auth_response.len(), 20);
        assert_eq!(decoded.auth_plugin_name, "mysql_native_password");
    }

    #[test]
    fn test_handshake_response_with_database() {
        let original = HandshakeResponse41 {
            capability_flags: CLIENT_PROTOCOL_41 | CLIENT_PLUGIN_AUTH | CLIENT_CONNECT_WITH_DB,
            max_packet_size: 16777216,
            character_set: CHARSET_UTF8MB4,
            reserved: [0u8; 23],
            username: "admin".to_string(),
            auth_response: vec![0; 20],
            database: Some("testdb".to_string()),
            auth_plugin_name: "mysql_native_password".to_string(),
        };
        let encoded = original.encode();
        let decoded = HandshakeResponse41::decode(&encoded).unwrap();
        assert_eq!(decoded.database, Some("testdb".to_string()));
    }

    #[test]
    fn test_ok_packet_encode() {
        let ok = OkPacket::simple();
        let encoded = ok.encode();
        assert_eq!(encoded[0], 0x00); // OK 标识
    }

    #[test]
    fn test_err_packet_encode() {
        let err = ErrPacket::new(
            error_codes::ACCESS_DENIED,
            sql_states::ACCESS_DENIED,
            "Access denied for user 'root'",
        );
        let encoded = err.encode();
        assert_eq!(encoded[0], 0xFF); // ERR 标识
        assert_eq!(encoded[1..3], [error_codes::ACCESS_DENIED as u8, (error_codes::ACCESS_DENIED >> 8) as u8]);
        assert_eq!(encoded[3], b'#');
        assert_eq!(&encoded[4..9], b"28000");
    }

    #[test]
    fn test_handshake_v10_invalid_protocol_version() {
        // 构造一个协议版本不是 10 的包
        let mut payload = vec![9u8]; // 错误版本
        payload.extend_from_slice(b"test\0");
        payload.extend_from_slice(&[0u8; 50]);
        let result = HandshakeV10::decode(&payload);
        assert!(result.is_err());
    }

    #[test]
    fn test_handshake_response_too_short() {
        let short_payload = vec![0u8; 10];
        let result = HandshakeResponse41::decode(&short_payload);
        assert!(result.is_err());
    }
}
