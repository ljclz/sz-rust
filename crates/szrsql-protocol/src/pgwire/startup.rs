//! pgwire 启动消息握手。
//!
//! # 启动阶段流程
//!
//! 1. 客户端连接后发送以下之一：
//!    - `StartupMessage`：开始正常启动握手
//!    - `SSLRequest`：请求 SSL 加密（Phase 4.5 已实现 TLS，根据配置返回 'S' 或 'N'）
//!    - `GSSENCRequest`：请求 GSSAPI 加密（同样拒绝）
//!    - `CancelRequest`：请求取消正在执行的查询
//!
//! 2. 服务端收到 `StartupMessage` 后：
//!    - 检查协议版本（必须为 3.0）
//!    - 读取参数对（user/database 等键值对）
//!    - 根据认证模式发送 `AuthenticationOk`（trust 模式）或 `AuthenticationCleartextPassword` 等
//!    - 发送若干 `ParameterStatus`（server_version / client_encoding / DateStyle 等）
//!    - 发送 `BackendKeyData`（pid + secret_key）
//!    - 发送 `ReadyForQuery`（status='I'）
//!
//! 参考文档：<https://www.postgresql.org/docs/current/protocol-flow.html#PROTOCOL-STARTUP>

use crate::pgwire::message::{put_cstring, read_cstring, BackendMessage, ErrorResponse, SqlState};
use bytes::{Buf, BufMut, BytesMut};
use std::collections::HashMap;
use std::io;
use thiserror::Error;

// =====================================================================
//  常量
// =====================================================================

/// pgwire 协议版本 3.0（major=3, minor=0）。
///
/// 编码为 Int32 = (major << 16) | minor = 0x0003_0000 = 196608。
pub const PROTOCOL_VERSION_3_0: i32 = 196_608;

/// SSLRequest 特殊协议版本号。
pub const PROTOCOL_SSL_REQUEST: i32 = 80_877_103;

/// GSSAPI 加密请求特殊协议版本号。
pub const PROTOCOL_GSSNC_REQUEST: i32 = 80_877_104;

/// CancelRequest 特殊协议版本号。
pub const PROTOCOL_CANCEL_REQUEST: i32 = 80_877_102;

/// 启动消息中必须包含 `user` 参数。
pub const PARAM_USER: &str = "user";

/// 启动消息中可选 `database` 参数（缺省时使用 user 名作为 database）。
pub const PARAM_DATABASE: &str = "database";

// =====================================================================
//  StartupError
// =====================================================================

/// 启动握手阶段错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StartupError {
    /// 缺少 user 参数（必须）。
    #[error("missing required parameter: {0}")]
    MissingParam(String),

    /// 不支持的协议版本。
    #[error("unsupported protocol version: {0}")]
    UnsupportedProtocol(i32),

    /// 启动消息格式错误。
    #[error("invalid startup message: {0}")]
    InvalidMessage(String),

    /// IO 错误。
    #[error("io error: {0}")]
    Io(String),
}

impl From<io::Error> for StartupError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

// =====================================================================
//  StartupMessage
// =====================================================================

/// 前端启动消息（startup phase）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupMessage {
    /// 正常启动消息（含参数对）。
    Startup(StartupParams),

    /// SSL 请求（应回复 'N' 表示不支持）。
    SslRequest,

    /// GSSAPI 请求（应回复 'N' 表示不支持）。
    GssencRequest,

    /// 取消正在执行的查询请求。
    CancelRequest { pid: i32, secret_key: i32 },
}

/// 启动参数对（k\0v\0 序列）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupParams {
    pub params: HashMap<String, String>,
}

impl StartupParams {
    /// 构造空参数对。
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加参数（builder 模式）。
    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }

    /// 获取 `user` 参数。
    pub fn user(&self) -> Option<&str> {
        self.params.get(PARAM_USER).map(|s| s.as_str())
    }

    /// 获取 `database` 参数（缺省回退到 `user`）。
    pub fn database(&self) -> Option<&str> {
        self.params
            .get(PARAM_DATABASE)
            .or_else(|| self.params.get(PARAM_USER))
            .map(|s| s.as_str())
    }
}

impl StartupMessage {
    /// 从 `src` 解码启动消息。
    ///
    /// 返回 `Ok(Some(msg))` 表示成功解码；`Ok(None)` 表示缓冲区数据不足；
    /// `Err` 表示协议错误。
    ///
    /// **注意**：调用方应在调用此方法前确保 `src` 仅包含启动阶段数据，
    /// 而非普通前端消息（普通消息有 Type 字节）。
    pub fn decode(src: &mut BytesMut) -> Result<Option<StartupMessage>, StartupError> {
        if src.len() < 8 {
            return Ok(None);
        }
        // 启动消息：Length(Int32) + ProtocolVersion(Int32) + Payload
        let length = i32::from_be_bytes([src[0], src[1], src[2], src[3]]);
        if length < 8 || length as usize > 1_000_000 {
            return Err(StartupError::InvalidMessage(format!(
                "invalid startup message length: {length}"
            )));
        }
        if src.len() < length as usize {
            return Ok(None);
        }
        // 取出整条消息（包含 length）
        let total = src.split_to(length as usize);
        // 跳过 Length，读取 ProtocolVersion
        let mut cursor = total.clone();
        cursor.advance(4); // length
        let protocol_version = cursor.get_i32();

        match protocol_version {
            PROTOCOL_SSL_REQUEST => Ok(Some(StartupMessage::SslRequest)),
            PROTOCOL_GSSNC_REQUEST => Ok(Some(StartupMessage::GssencRequest)),
            PROTOCOL_CANCEL_REQUEST => {
                // CancelRequest: pid(i32) + secret_key(i32)
                if cursor.remaining() < 8 {
                    return Err(StartupError::InvalidMessage(
                        "cancel request missing pid/secret".into(),
                    ));
                }
                let pid = cursor.get_i32();
                let secret_key = cursor.get_i32();
                Ok(Some(StartupMessage::CancelRequest { pid, secret_key }))
            }
            PROTOCOL_VERSION_3_0 => {
                // 正常 Startup：参数对 k\0v\0... 以 \0 结束
                let mut params = HashMap::new();
                // cursor 现在从 protocol_version 之后开始
                let mut buf = cursor;
                loop {
                    if buf.remaining() == 0 {
                        return Err(StartupError::InvalidMessage(
                            "startup message missing terminating NUL".into(),
                        ));
                    }
                    // 检查终止 \0
                    if buf.as_ref()[0] == 0 {
                        break;
                    }
                    let key = read_cstring(&mut buf)?;
                    let value = read_cstring(&mut buf)?;
                    params.insert(key, value);
                }
                let startup_params = StartupParams { params };
                if startup_params.user().is_none() {
                    return Err(StartupError::MissingParam(PARAM_USER.to_string()));
                }
                Ok(Some(StartupMessage::Startup(startup_params)))
            }
            other => Err(StartupError::UnsupportedProtocol(other)),
        }
    }
}

// =====================================================================
//  握手响应构造
// =====================================================================

/// 构造启动握手响应消息序列（trust 认证）。
///
/// 输出顺序遵循 pgwire 协议：
/// 1. `AuthenticationOk`
/// 2. 一组 `ParameterStatus`（server_version / client_encoding / DateStyle / integer_datetimes / server_encoding / standard_conforming_strings / application_name / TimeZone）
/// 3. `BackendKeyData`（pid + secret_key）
/// 4. `ReadyForQuery`（status='I'）
pub fn build_startup_response(
    pid: i32,
    secret_key: i32,
    server_version: &str,
    application_name: Option<&str>,
    dst: &mut BytesMut,
) {
    BackendMessage::AuthenticationOk.encode(dst);

    // 标准 ParameterStatus 集合（PG 默认会发送这些）
    BackendMessage::ParameterStatus {
        name: "server_version".into(),
        value: server_version.into(),
    }
    .encode(dst);
    BackendMessage::ParameterStatus {
        name: "server_encoding".into(),
        value: "UTF8".into(),
    }
    .encode(dst);
    BackendMessage::ParameterStatus {
        name: "client_encoding".into(),
        value: "UTF8".into(),
    }
    .encode(dst);
    BackendMessage::ParameterStatus {
        name: "DateStyle".into(),
        value: "ISO, MDY".into(),
    }
    .encode(dst);
    BackendMessage::ParameterStatus {
        name: "TimeZone".into(),
        value: "UTC".into(),
    }
    .encode(dst);
    BackendMessage::ParameterStatus {
        name: "integer_datetimes".into(),
        value: "on".into(),
    }
    .encode(dst);
    BackendMessage::ParameterStatus {
        name: "standard_conforming_strings".into(),
        value: "on".into(),
    }
    .encode(dst);
    if let Some(app) = application_name {
        BackendMessage::ParameterStatus {
            name: "application_name".into(),
            value: app.into(),
        }
        .encode(dst);
    }

    BackendMessage::BackendKeyData { pid, secret_key }.encode(dst);
    BackendMessage::ReadyForQuery {
        status: crate::pgwire::message::STATUS_IDLE,
    }
    .encode(dst);
}

/// 构造认证失败错误响应（FATAL 级别）。
pub fn build_auth_error_response(message: &str, dst: &mut BytesMut) {
    let err = ErrorResponse::fatal(SqlState::INVALID_AUTHORIZATION_SPECIFICATION, message);
    BackendMessage::ErrorResponse(err).encode(dst);
}

/// 构造协议错误响应（FATAL 级别）。
pub fn build_protocol_error_response(message: &str, dst: &mut BytesMut) {
    let err = ErrorResponse::fatal(SqlState::PROTOCOL_VIOLATION, message);
    BackendMessage::ErrorResponse(err).encode(dst);
}

/// 构造功能未支持错误响应（ERROR 级别）。
pub fn build_feature_not_supported_response(message: &str, dst: &mut BytesMut) {
    let err = ErrorResponse::error(SqlState::FEATURE_NOT_SUPPORTED, message);
    BackendMessage::ErrorResponse(err).encode(dst);
}

// =====================================================================
//  编码 StartupMessage（用于测试/客户端模拟）
// =====================================================================

/// 将 `StartupParams` 编码为完整的 StartupMessage 字节流（含 Length + ProtocolVersion + Payload + \0）。
///
/// 主要用于测试，让单元测试可以构造启动消息并验证解码逻辑。
pub fn encode_startup_message(params: &StartupParams) -> BytesMut {
    let mut payload = BytesMut::new();
    payload.put_i32(PROTOCOL_VERSION_3_0);
    for (k, v) in &params.params {
        put_cstring(&mut payload, k);
        put_cstring(&mut payload, v);
    }
    payload.put_u8(0); // 终止 \0

    let total_len = (payload.len() + 4) as i32;
    let mut dst = BytesMut::with_capacity(payload.len() + 4);
    dst.put_i32(total_len);
    dst.extend_from_slice(&payload);
    dst
}

/// 编码一个 SSLRequest / CancelRequest 字节流（用于测试）。
pub fn encode_special_request(protocol: i32) -> BytesMut {
    let mut dst = BytesMut::new();
    dst.put_i32(8);
    dst.put_i32(protocol);
    dst
}

/// 编码一个 CancelRequest 完整字节流（含 pid + secret_key，用于测试）。
pub fn encode_cancel_request(pid: i32, secret_key: i32) -> BytesMut {
    let mut dst = BytesMut::new();
    dst.put_i32(16);
    dst.put_i32(PROTOCOL_CANCEL_REQUEST);
    dst.put_i32(pid);
    dst.put_i32(secret_key);
    dst
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pgwire::message::{
        Severity, MSG_AUTHENTICATION, MSG_BACKEND_KEY_DATA, MSG_PARAMETER_STATUS,
        MSG_READY_FOR_QUERY, STATUS_IDLE,
    };

    // ---- StartupMessage 解码测试 ----

    #[test]
    fn test_decode_startup_message_basic() {
        let params = StartupParams::new()
            .with("user", "alice")
            .with("database", "testdb")
            .with("client_encoding", "UTF8");
        let bytes = encode_startup_message(&params);
        let mut src = bytes.clone();
        let msg = StartupMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            StartupMessage::Startup(p) => {
                assert_eq!(p.user(), Some("alice"));
                assert_eq!(p.database(), Some("testdb"));
                assert_eq!(p.params.get("client_encoding"), Some(&"UTF8".to_string()));
            }
            other => panic!("expected Startup, got {other:?}"),
        }
        // 缓冲区应被完全消费
        assert!(src.is_empty());
    }

    #[test]
    fn test_decode_startup_message_minimal() {
        // 仅 user 参数
        let params = StartupParams::new().with("user", "bob");
        let bytes = encode_startup_message(&params);
        let mut src = bytes;
        let msg = StartupMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            StartupMessage::Startup(p) => {
                assert_eq!(p.user(), Some("bob"));
                // database 缺省回退到 user
                assert_eq!(p.database(), Some("bob"));
            }
            other => panic!("expected Startup, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_startup_message_missing_user_errors() {
        // 没有 user 参数，应返回错误
        let params = StartupParams::new().with("database", "testdb");
        let bytes = encode_startup_message(&params);
        let mut src = bytes;
        let err = StartupMessage::decode(&mut src).unwrap_err();
        assert_eq!(err, StartupError::MissingParam("user".into()));
    }

    #[test]
    fn test_decode_ssl_request() {
        let bytes = encode_special_request(PROTOCOL_SSL_REQUEST);
        let mut src = bytes;
        let msg = StartupMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        assert_eq!(msg, StartupMessage::SslRequest);
        assert!(src.is_empty());
    }

    #[test]
    fn test_decode_gssenc_request() {
        let bytes = encode_special_request(PROTOCOL_GSSNC_REQUEST);
        let mut src = bytes;
        let msg = StartupMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        assert_eq!(msg, StartupMessage::GssencRequest);
    }

    #[test]
    fn test_decode_cancel_request() {
        let bytes = encode_cancel_request(4242, -12345);
        let mut src = bytes;
        let msg = StartupMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        assert_eq!(
            msg,
            StartupMessage::CancelRequest {
                pid: 4242,
                secret_key: -12345,
            }
        );
        assert!(src.is_empty());
    }

    #[test]
    fn test_decode_returns_none_when_incomplete() {
        // 仅有 4 字节长度
        let mut src = BytesMut::from(&[0, 0, 0, 8][..]);
        assert!(StartupMessage::decode(&mut src).unwrap().is_none());

        // 长度声明 100 但实际只有 8 字节
        let mut src = BytesMut::new();
        src.put_i32(100);
        src.put_i32(PROTOCOL_VERSION_3_0);
        assert!(StartupMessage::decode(&mut src).unwrap().is_none());
    }

    #[test]
    fn test_decode_rejects_invalid_length() {
        // length < 8 非法
        let mut src = BytesMut::new();
        src.put_i32(4);
        src.put_i32(PROTOCOL_VERSION_3_0);
        let err = StartupMessage::decode(&mut src).unwrap_err();
        assert!(matches!(err, StartupError::InvalidMessage(_)));
    }

    #[test]
    fn test_decode_rejects_huge_length() {
        let mut src = BytesMut::new();
        src.put_i32(2_000_000); // > 1MB 非法
        src.put_i32(PROTOCOL_VERSION_3_0);
        let err = StartupMessage::decode(&mut src).unwrap_err();
        assert!(matches!(err, StartupError::InvalidMessage(_)));
    }

    #[test]
    fn test_decode_rejects_unsupported_protocol() {
        let mut src = BytesMut::new();
        src.put_i32(8);
        src.put_i32(0x0002_0000); // protocol 2.0 不支持
        let err = StartupMessage::decode(&mut src).unwrap_err();
        assert_eq!(err, StartupError::UnsupportedProtocol(0x0002_0000));
    }

    // ---- 握手响应构造测试 ----

    #[test]
    fn test_build_startup_response_includes_authentication_ok() {
        let mut dst = BytesMut::new();
        build_startup_response(1234, -5678, "14.0", None, &mut dst);

        // 第一条消息必须是 AuthenticationOk
        assert_eq!(dst[0], MSG_AUTHENTICATION);
        let len = i32::from_be_bytes([dst[1], dst[2], dst[3], dst[4]]);
        assert_eq!(len, 8);
        // AuthCode = 0 (AUTH_OK)
        assert_eq!(&dst[5..9], &[0, 0, 0, 0]);
    }

    #[test]
    fn test_build_startup_response_includes_parameter_statuses() {
        let mut dst = BytesMut::new();
        build_startup_response(1, 2, "14.0", None, &mut dst);

        // 提取所有 ParameterStatus 名称
        let mut param_names = Vec::new();
        let mut i = 0;
        while i < dst.len() {
            let msg_type = dst[i];
            let msg_len =
                i32::from_be_bytes([dst[i + 1], dst[i + 2], dst[i + 3], dst[i + 4]]) as usize;
            if msg_type == MSG_PARAMETER_STATUS {
                // payload 从 i+5 开始，name 直到 \0
                let payload = &dst[i + 5..i + 1 + msg_len];
                let nul = payload.iter().position(|&b| b == 0).unwrap();
                let name = String::from_utf8_lossy(&payload[..nul]).into_owned();
                param_names.push(name);
            }
            i += 1 + msg_len;
        }
        // 必须包含这些标准参数
        assert!(param_names.contains(&"server_version".to_string()));
        assert!(param_names.contains(&"client_encoding".to_string()));
        assert!(param_names.contains(&"DateStyle".to_string()));
        assert!(param_names.contains(&"TimeZone".to_string()));
        assert!(param_names.contains(&"integer_datetimes".to_string()));
        assert!(param_names.contains(&"standard_conforming_strings".to_string()));
        assert!(param_names.contains(&"server_encoding".to_string()));
    }

    #[test]
    fn test_build_startup_response_includes_application_name() {
        let mut dst = BytesMut::new();
        build_startup_response(1, 2, "14.0", Some("psql"), &mut dst);

        // 找到 application_name 的 ParameterStatus，验证 value=psql
        let mut i = 0;
        let mut found = false;
        while i < dst.len() {
            let msg_type = dst[i];
            let msg_len =
                i32::from_be_bytes([dst[i + 1], dst[i + 2], dst[i + 3], dst[i + 4]]) as usize;
            if msg_type == MSG_PARAMETER_STATUS {
                let payload = &dst[i + 5..i + 1 + msg_len];
                let nul = payload.iter().position(|&b| b == 0).unwrap();
                let name = String::from_utf8_lossy(&payload[..nul]).into_owned();
                if name == "application_name" {
                    let value_start = nul + 1;
                    let value_nul = payload[value_start..].iter().position(|&b| b == 0).unwrap();
                    let value =
                        String::from_utf8_lossy(&payload[value_start..value_start + value_nul])
                            .into_owned();
                    assert_eq!(value, "psql");
                    found = true;
                    break;
                }
            }
            i += 1 + msg_len;
        }
        assert!(found, "should include application_name ParameterStatus");
    }

    #[test]
    fn test_build_startup_response_includes_backend_key_data() {
        let mut dst = BytesMut::new();
        build_startup_response(1234, -5678, "14.0", None, &mut dst);

        // 找到 BackendKeyData 消息
        let mut i = 0;
        let mut found = false;
        while i < dst.len() {
            let msg_type = dst[i];
            let msg_len =
                i32::from_be_bytes([dst[i + 1], dst[i + 2], dst[i + 3], dst[i + 4]]) as usize;
            if msg_type == MSG_BACKEND_KEY_DATA {
                assert_eq!(msg_len, 12);
                let pid = i32::from_be_bytes([dst[i + 5], dst[i + 6], dst[i + 7], dst[i + 8]]);
                let secret =
                    i32::from_be_bytes([dst[i + 9], dst[i + 10], dst[i + 11], dst[i + 12]]);
                assert_eq!(pid, 1234);
                assert_eq!(secret, -5678);
                found = true;
                break;
            }
            i += 1 + msg_len;
        }
        assert!(found, "should include BackendKeyData");
    }

    #[test]
    fn test_build_startup_response_ends_with_ready_for_query() {
        let mut dst = BytesMut::new();
        build_startup_response(1, 2, "14.0", None, &mut dst);

        // 最后一条消息必须是 ReadyForQuery，状态 'I'
        assert_eq!(dst[dst.len() - 6], MSG_READY_FOR_QUERY);
        let len = i32::from_be_bytes([
            dst[dst.len() - 5],
            dst[dst.len() - 4],
            dst[dst.len() - 3],
            dst[dst.len() - 2],
        ]);
        assert_eq!(len, 5);
        assert_eq!(dst[dst.len() - 1], STATUS_IDLE);
    }

    #[test]
    fn test_build_startup_response_message_order() {
        // 验证消息顺序：AuthenticationOk → ParameterStatus* → BackendKeyData → ReadyForQuery
        let mut dst = BytesMut::new();
        build_startup_response(1, 2, "14.0", None, &mut dst);

        let mut types = Vec::new();
        let mut i = 0;
        while i < dst.len() {
            let msg_type = dst[i];
            let msg_len =
                i32::from_be_bytes([dst[i + 1], dst[i + 2], dst[i + 3], dst[i + 4]]) as usize;
            types.push(msg_type);
            i += 1 + msg_len;
        }
        // 第一条是 AuthenticationOk
        assert_eq!(types[0], MSG_AUTHENTICATION);
        // 中间全是 ParameterStatus
        for t in &types[1..types.len() - 2] {
            assert_eq!(*t, MSG_PARAMETER_STATUS);
        }
        // 倒数第二条是 BackendKeyData
        assert_eq!(types[types.len() - 2], MSG_BACKEND_KEY_DATA);
        // 最后一条是 ReadyForQuery
        assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
    }

    // ---- 错误响应构造测试 ----

    #[test]
    fn test_build_auth_error_response_is_fatal() {
        let mut dst = BytesMut::new();
        build_auth_error_response("password authentication failed", &mut dst);
        assert_eq!(dst[0], b'E'); // ErrorResponse
        let s = String::from_utf8_lossy(&dst);
        assert!(s.contains("FATAL"));
        assert!(s.contains("password authentication failed"));
        assert!(s.contains("28000")); // INVALID_AUTHORIZATION_SPECIFICATION
    }

    #[test]
    fn test_build_protocol_error_response_is_fatal() {
        let mut dst = BytesMut::new();
        build_protocol_error_response("bad protocol version", &mut dst);
        let s = String::from_utf8_lossy(&dst);
        assert!(s.contains("FATAL"));
        assert!(s.contains("08P01")); // PROTOCOL_VIOLATION
    }

    #[test]
    fn test_build_feature_not_supported_is_error() {
        let mut dst = BytesMut::new();
        build_feature_not_supported_response("SSL not supported", &mut dst);
        let s = String::from_utf8_lossy(&dst);
        // FEATURE_NOT_SUPPORTED 是 ERROR 级别，不是 FATAL
        assert!(s.contains("ERROR"));
        assert!(!s.contains("FATAL"));
        assert!(s.contains("0A000")); // FEATURE_NOT_SUPPORTED
    }

    // ---- encode_startup_message round-trip 测试 ----

    #[test]
    fn test_encode_startup_message_round_trip() {
        let params = StartupParams::new()
            .with("user", "test_user")
            .with("database", "test_db")
            .with("application_name", "my_app")
            .with("client_encoding", "UTF8");
        let bytes = encode_startup_message(&params);
        let mut src = bytes;
        let msg = StartupMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            StartupMessage::Startup(decoded) => {
                assert_eq!(decoded.params.len(), 4);
                assert_eq!(decoded.user(), Some("test_user"));
                assert_eq!(decoded.database(), Some("test_db"));
                assert_eq!(
                    decoded.params.get("application_name"),
                    Some(&"my_app".to_string())
                );
            }
            other => panic!("expected Startup, got {other:?}"),
        }
    }

    #[test]
    fn test_database_fallback_to_user() {
        // 不传 database，应回退到 user
        let params = StartupParams::new().with("user", "alice");
        assert_eq!(params.database(), Some("alice"));

        // 同时传 database，应优先 database
        let params = StartupParams::new()
            .with("user", "alice")
            .with("database", "alice_db");
        assert_eq!(params.database(), Some("alice_db"));
    }

    // ---- io::Error 转换 ----

    #[test]
    fn test_startup_error_from_io() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionReset, "reset");
        let startup_err: StartupError = io_err.into();
        assert!(matches!(startup_err, StartupError::Io(_)));
    }

    // ---- Severity 转换测试 ----

    #[test]
    fn test_severity_fatal_vs_error() {
        assert_eq!(Severity::Fatal.as_str(), "FATAL");
        assert_ne!(Severity::Fatal, Severity::Error);
    }
}
