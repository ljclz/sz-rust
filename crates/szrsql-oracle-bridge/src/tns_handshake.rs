//! TNS 握手协议 — Connect Request / Accept Response 编解码。
//!
//! TNS 握手发生在 TCP 连接建立后，客户端发送 Connect Request（包类型 0x01），
//! 服务器响应 Accept Response（包类型 0x02）或 Refuse（0x03）。
//!
//! # Connect Request（客户端 → 服务器）
//!
//! 固定部分（24 字节，大端）：
//! - Version: 2 字节（如 0x013A = 314，对应 Oracle 12c）
//! - Version Compatibility: 2 字节
//! - Service Options: 2 字节
//! - Session Data Unit Size (SDU): 2 字节
//! - Transaction Data Unit Size (TDU): 2 字节
//! - NT Protocol Characteristics: 2 字节
//! - Line Turnaround Value: 2 字节
//! - Value of 1 in Hardware: 2 字节
//! - Connection Data Length: 2 字节（服务名 ASCII 字节数）
//! - Connection Data Offset: 2 字节（固定部分长度，通常为 26 = 24 + 2）
//! - Max Receive Size: 4 字节
//!
//! 变长部分：
//! - Connection Data（服务名 ASCII）
//!
//! # Accept Response（服务器 → 客户端）
//!
//! 固定部分（12 字节，大端）：
//! - Version: 2 字节
//! - Service Options: 2 字节
//! - Session Data Unit Size (SDU): 2 字节
//! - Transaction Data Unit Size (TDU): 2 字节
//! - NT Protocol Characteristics: 2 字节
//! - Line Turnaround Value: 2 字节

use crate::tns_packet::{PacketType, TnsPacket, TnsPacketError};
use thiserror::Error;

/// TNS 协议版本：Oracle 12c (314)。
pub const TNS_VERSION_314: u16 = 314;
/// TNS 协议版本：Oracle 11g (312)。
pub const TNS_VERSION_312: u16 = 312;
/// TNS 协议版本：Oracle 18c+ (315)。
pub const TNS_VERSION_315: u16 = 315;

/// Connect Request 固定部分长度（不含服务名）。
pub const CONNECT_FIXED_LEN: usize = 24;
/// Connect Request 中 Connection Data Offset 字段的默认值（固定部分 + 2 字节 reserved）。
pub const CONNECT_DATA_OFFSET: u16 = 26;
/// Accept Response 固定部分长度。
pub const ACCEPT_FIXED_LEN: usize = 12;

/// 默认 SDU（Session Data Unit）大小：8192 字节。
pub const DEFAULT_SDU: u16 = 8192;
/// 默认 TDU（Transaction Data Unit）大小：8192 字节。
pub const DEFAULT_TDU: u16 = 8192;
/// 默认最大接收大小：8192 字节。
pub const DEFAULT_MAX_RECEIVE: u32 = 8192;

/// TNS 握手错误。
#[derive(Debug, Error)]
pub enum HandshakeError {
    /// 包格式错误（非 Connect/Accept 类型等）。
    #[error("invalid packet: expected {expected}, got {got:?}")]
    InvalidPacket {
        /// 期望的包类型描述
        expected: &'static str,
        /// 实际收到的包
        got: PacketType,
    },
    /// 包负载过短。
    #[error("payload too short: expected at least {expected} bytes, got {got}")]
    PayloadTooShort {
        /// 期望的最小字节数
        expected: usize,
        /// 实际字节数
        got: usize,
    },
    /// 服务名包含非 ASCII 字符。
    #[error("service name contains non-ASCII characters")]
    NonAsciiServiceName,
    /// 包编解码错误。
    #[error(transparent)]
    Packet(#[from] TnsPacketError),
}

/// Connect Request 负载。
///
/// 客户端发送给服务器，包含版本、SDU/TDU、服务名等信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    /// TNS 协议版本（如 314）
    pub version: u16,
    /// 版本兼容性
    pub version_compatibility: u16,
    /// 服务选项位标志
    pub service_options: u16,
    /// SDU（Session Data Unit）大小
    pub sdu: u16,
    /// TDU（Transaction Data Unit）大小
    pub tdu: u16,
    /// NT 协议特性
    pub nt_protocol_chars: u16,
    /// 行翻转值
    pub line_turnaround: u16,
    /// 硬件中 1 的值（大端 0x0001，小端 0x0100）
    pub value_of_1: u16,
    /// 最大接收大小
    pub max_receive_size: u32,
    /// 服务名（ASCII）
    pub service_name: String,
}

impl ConnectRequest {
    /// 构造一个默认参数的 Connect Request。
    ///
    /// 使用 Oracle 12c (314) 版本、8192 SDU/TDU，仅服务名需指定。
    pub fn new(service_name: impl Into<String>) -> Result<Self, HandshakeError> {
        let service_name = service_name.into();
        if !service_name.is_ascii() {
            return Err(HandshakeError::NonAsciiServiceName);
        }
        Ok(Self {
            version: TNS_VERSION_314,
            version_compatibility: TNS_VERSION_314,
            service_options: 0x0C41,
            sdu: DEFAULT_SDU,
            tdu: DEFAULT_TDU,
            nt_protocol_chars: 0x4F98,
            line_turnaround: 0x0001,
            value_of_1: 0x0001,
            max_receive_size: DEFAULT_MAX_RECEIVE,
            service_name,
        })
    }

    /// 设置 TNS 协议版本。
    pub fn with_version(mut self, version: u16) -> Self {
        self.version = version;
        self.version_compatibility = version;
        self
    }

    /// 设置 SDU/TDU 大小。
    pub fn with_sdu_tdu(mut self, sdu: u16, tdu: u16) -> Self {
        self.sdu = sdu;
        self.tdu = tdu;
        self
    }

    /// 编码为字节序列（Connect Request 的负载，不含 TNS 包头）。
    pub fn encode_payload(&self) -> Vec<u8> {
        let service_bytes = self.service_name.as_bytes();
        let conn_data_len = service_bytes.len() as u16;
        let mut buf = Vec::with_capacity(CONNECT_FIXED_LEN + service_bytes.len());
        // Version
        buf.extend_from_slice(&self.version.to_be_bytes());
        // Version Compatibility
        buf.extend_from_slice(&self.version_compatibility.to_be_bytes());
        // Service Options
        buf.extend_from_slice(&self.service_options.to_be_bytes());
        // SDU
        buf.extend_from_slice(&self.sdu.to_be_bytes());
        // TDU
        buf.extend_from_slice(&self.tdu.to_be_bytes());
        // NT Protocol Characteristics
        buf.extend_from_slice(&self.nt_protocol_chars.to_be_bytes());
        // Line Turnaround Value
        buf.extend_from_slice(&self.line_turnaround.to_be_bytes());
        // Value of 1 in Hardware
        buf.extend_from_slice(&self.value_of_1.to_be_bytes());
        // Connection Data Length
        buf.extend_from_slice(&conn_data_len.to_be_bytes());
        // Connection Data Offset
        buf.extend_from_slice(&CONNECT_DATA_OFFSET.to_be_bytes());
        // Max Receive Size (4 字节)
        buf.extend_from_slice(&self.max_receive_size.to_be_bytes());
        // Connection Data（服务名 ASCII）
        buf.extend_from_slice(service_bytes);
        buf
    }

    /// 编码为完整的 TNS 包（含包头）。
    pub fn encode(&self) -> TnsPacket {
        TnsPacket::new(PacketType::Connect, self.encode_payload())
    }

    /// 从 TNS 包解析 Connect Request。
    pub fn from_packet(packet: &TnsPacket) -> Result<Self, HandshakeError> {
        if packet.packet_type != PacketType::Connect {
            return Err(HandshakeError::InvalidPacket {
                expected: "Connect (0x01)",
                got: packet.packet_type,
            });
        }
        Self::decode_payload(&packet.data)
    }

    /// 从负载字节解析 Connect Request。
    pub fn decode_payload(data: &[u8]) -> Result<Self, HandshakeError> {
        if data.len() < CONNECT_FIXED_LEN {
            return Err(HandshakeError::PayloadTooShort {
                expected: CONNECT_FIXED_LEN,
                got: data.len(),
            });
        }
        let version = u16::from_be_bytes([data[0], data[1]]);
        let version_compatibility = u16::from_be_bytes([data[2], data[3]]);
        let service_options = u16::from_be_bytes([data[4], data[5]]);
        let sdu = u16::from_be_bytes([data[6], data[7]]);
        let tdu = u16::from_be_bytes([data[8], data[9]]);
        let nt_protocol_chars = u16::from_be_bytes([data[10], data[11]]);
        let line_turnaround = u16::from_be_bytes([data[12], data[13]]);
        let value_of_1 = u16::from_be_bytes([data[14], data[15]]);
        let conn_data_len = u16::from_be_bytes([data[16], data[17]]) as usize;
        let _conn_data_offset = u16::from_be_bytes([data[18], data[19]]);
        let max_receive_size = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        // Connection Data（变长）
        let service_name_end = CONNECT_FIXED_LEN + conn_data_len;
        if data.len() < service_name_end {
            return Err(HandshakeError::PayloadTooShort {
                expected: service_name_end,
                got: data.len(),
            });
        }
        let service_name_bytes = &data[CONNECT_FIXED_LEN..service_name_end];
        let service_name = String::from_utf8_lossy(service_name_bytes).into_owned();
        if !service_name.is_ascii() {
            return Err(HandshakeError::NonAsciiServiceName);
        }
        Ok(Self {
            version,
            version_compatibility,
            service_options,
            sdu,
            tdu,
            nt_protocol_chars,
            line_turnaround,
            value_of_1,
            max_receive_size,
            service_name,
        })
    }
}

/// Accept Response 负载。
///
/// 服务器在握手成功后响应客户端，确认版本与 SDU/TDU。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptResponse {
    /// TNS 协议版本（协商后）
    pub version: u16,
    /// 服务选项位标志
    pub service_options: u16,
    /// SDU 大小（协商后）
    pub sdu: u16,
    /// TDU 大小（协商后）
    pub tdu: u16,
    /// NT 协议特性
    pub nt_protocol_chars: u16,
    /// 行翻转值
    pub line_turnaround: u16,
}

impl AcceptResponse {
    /// 构造一个默认参数的 Accept Response。
    ///
    /// 使用请求版本或默认 314，8192 SDU/TDU。
    pub fn new(version: u16) -> Self {
        Self {
            version,
            service_options: 0x0C41,
            sdu: DEFAULT_SDU,
            tdu: DEFAULT_TDU,
            nt_protocol_chars: 0x4F98,
            line_turnaround: 0x0001,
        }
    }

    /// 设置协商后的 SDU/TDU。
    pub fn with_sdu_tdu(mut self, sdu: u16, tdu: u16) -> Self {
        self.sdu = sdu;
        self.tdu = tdu;
        self
    }

    /// 编码为字节序列（Accept Response 的负载，不含 TNS 包头）。
    pub fn encode_payload(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(ACCEPT_FIXED_LEN);
        buf.extend_from_slice(&self.version.to_be_bytes());
        buf.extend_from_slice(&self.service_options.to_be_bytes());
        buf.extend_from_slice(&self.sdu.to_be_bytes());
        buf.extend_from_slice(&self.tdu.to_be_bytes());
        buf.extend_from_slice(&self.nt_protocol_chars.to_be_bytes());
        buf.extend_from_slice(&self.line_turnaround.to_be_bytes());
        buf
    }

    /// 编码为完整的 TNS 包（含包头）。
    pub fn encode(&self) -> TnsPacket {
        TnsPacket::new(PacketType::Accept, self.encode_payload())
    }

    /// 从 TNS 包解析 Accept Response。
    pub fn from_packet(packet: &TnsPacket) -> Result<Self, HandshakeError> {
        if packet.packet_type != PacketType::Accept {
            return Err(HandshakeError::InvalidPacket {
                expected: "Accept (0x02)",
                got: packet.packet_type,
            });
        }
        Self::decode_payload(&packet.data)
    }

    /// 从负载字节解析 Accept Response。
    pub fn decode_payload(data: &[u8]) -> Result<Self, HandshakeError> {
        if data.len() < ACCEPT_FIXED_LEN {
            return Err(HandshakeError::PayloadTooShort {
                expected: ACCEPT_FIXED_LEN,
                got: data.len(),
            });
        }
        let version = u16::from_be_bytes([data[0], data[1]]);
        let service_options = u16::from_be_bytes([data[2], data[3]]);
        let sdu = u16::from_be_bytes([data[4], data[5]]);
        let tdu = u16::from_be_bytes([data[6], data[7]]);
        let nt_protocol_chars = u16::from_be_bytes([data[8], data[9]]);
        let line_turnaround = u16::from_be_bytes([data[10], data[11]]);
        Ok(Self {
            version,
            service_options,
            sdu,
            tdu,
            nt_protocol_chars,
            line_turnaround,
        })
    }
}

/// 根据客户端请求版本协商最终 TNS 版本。
///
/// 服务器支持的版本列表为 314 / 312 / 315。返回客户端请求版本中
/// 服务器支持的最大值；若全部不支持，返回默认 314。
pub fn negotiate_version(client_version: u16) -> u16 {
    match client_version {
        v if v >= TNS_VERSION_315 => TNS_VERSION_315,
        v if v >= TNS_VERSION_314 => TNS_VERSION_314,
        v if v >= TNS_VERSION_312 => TNS_VERSION_312,
        _ => TNS_VERSION_314,
    }
}

/// 协商 SDU 大小：取客户端请求与服务器默认的较小值。
pub fn negotiate_sdu(client_sdu: u16) -> u16 {
    client_sdu.min(DEFAULT_SDU).max(512)
}

/// 协商 TDU 大小：取客户端请求与服务器默认的较小值。
pub fn negotiate_tdu(client_tdu: u16) -> u16 {
    client_tdu.min(DEFAULT_TDU).max(512)
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_request_new_sets_defaults() {
        // 验证默认参数构造
        let req = ConnectRequest::new("ORCL").unwrap();
        assert_eq!(req.version, TNS_VERSION_314);
        assert_eq!(req.version_compatibility, TNS_VERSION_314);
        assert_eq!(req.sdu, DEFAULT_SDU);
        assert_eq!(req.tdu, DEFAULT_TDU);
        assert_eq!(req.service_name, "ORCL");
    }

    #[test]
    fn connect_request_rejects_non_ascii_service_name() {
        // 验证非 ASCII 服务名被拒绝
        let result = ConnectRequest::new("服务名");
        assert!(matches!(result, Err(HandshakeError::NonAsciiServiceName)));
    }

    #[test]
    fn connect_request_encode_decode_roundtrip() {
        // 验证 Connect Request 的编解码往返
        let original = ConnectRequest::new("ORCLPDB").unwrap();
        let payload = original.encode_payload();
        let decoded = ConnectRequest::decode_payload(&payload).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn connect_request_encode_as_tns_packet() {
        // 验证 Connect Request 编码为 TNS 包后的类型与负载
        let req = ConnectRequest::new("XE").unwrap();
        let packet = req.encode();
        assert_eq!(packet.packet_type, PacketType::Connect);
        let decoded = ConnectRequest::from_packet(&packet).unwrap();
        assert_eq!(decoded.service_name, "XE");
        assert_eq!(decoded.version, TNS_VERSION_314);
    }

    #[test]
    fn connect_request_decode_rejects_wrong_packet_type() {
        // 验证错误包类型返回 InvalidPacket
        let wrong_packet = TnsPacket::new(PacketType::Data, vec![0u8; 24]);
        let result = ConnectRequest::from_packet(&wrong_packet);
        assert!(matches!(result, Err(HandshakeError::InvalidPacket { .. })));
    }

    #[test]
    fn connect_request_decode_rejects_short_payload() {
        // 验证负载过短返回 PayloadTooShort
        let short_data = vec![0u8; 10];
        let result = ConnectRequest::decode_payload(&short_data);
        assert!(matches!(
            result,
            Err(HandshakeError::PayloadTooShort {
                expected: CONNECT_FIXED_LEN,
                got: 10
            })
        ));
    }

    #[test]
    fn connect_request_with_version_chain() {
        // 验证 builder 链式调用
        let req = ConnectRequest::new("ORCL")
            .unwrap()
            .with_version(TNS_VERSION_312)
            .with_sdu_tdu(4096, 4096);
        assert_eq!(req.version, TNS_VERSION_312);
        assert_eq!(req.sdu, 4096);
        assert_eq!(req.tdu, 4096);
    }

    #[test]
    fn accept_response_encode_decode_roundtrip() {
        // 验证 Accept Response 的编解码往返
        let original = AcceptResponse::new(TNS_VERSION_314).with_sdu_tdu(4096, 4096);
        let payload = original.encode_payload();
        let decoded = AcceptResponse::decode_payload(&payload).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn accept_response_encode_as_tns_packet() {
        // 验证 Accept Response 编码为 TNS 包后的类型与负载
        let resp = AcceptResponse::new(TNS_VERSION_315);
        let packet = resp.encode();
        assert_eq!(packet.packet_type, PacketType::Accept);
        assert_eq!(packet.data.len(), ACCEPT_FIXED_LEN);
        let decoded = AcceptResponse::from_packet(&packet).unwrap();
        assert_eq!(decoded.version, TNS_VERSION_315);
    }

    #[test]
    fn accept_response_decode_rejects_short_payload() {
        // 验证负载过短返回 PayloadTooShort
        let short_data = vec![0u8; 6];
        let result = AcceptResponse::decode_payload(&short_data);
        assert!(matches!(
            result,
            Err(HandshakeError::PayloadTooShort {
                expected: ACCEPT_FIXED_LEN,
                got: 6
            })
        ));
    }

    #[test]
    fn accept_response_decode_rejects_wrong_packet_type() {
        // 验证错误包类型返回 InvalidPacket
        let wrong_packet = TnsPacket::new(PacketType::Ok, vec![0u8; 12]);
        let result = AcceptResponse::from_packet(&wrong_packet);
        assert!(matches!(result, Err(HandshakeError::InvalidPacket { .. })));
    }

    #[test]
    fn negotiate_version_picks_highest_supported() {
        // 验证版本协商：返回客户端支持的最低且服务器支持的版本
        assert_eq!(negotiate_version(TNS_VERSION_315), TNS_VERSION_315);
        assert_eq!(negotiate_version(TNS_VERSION_314), TNS_VERSION_314);
        assert_eq!(negotiate_version(TNS_VERSION_312), TNS_VERSION_312);
        // 低于 312 的版本回退到 314
        assert_eq!(negotiate_version(300), TNS_VERSION_314);
        // 高于 315 的版本取 315
        assert_eq!(negotiate_version(400), TNS_VERSION_315);
    }

    #[test]
    fn negotiate_sdu_clamps_to_range() {
        // 验证 SDU 协商：取较小值，最小 512
        assert_eq!(negotiate_sdu(2048), 2048);
        assert_eq!(negotiate_sdu(DEFAULT_SDU), DEFAULT_SDU);
        assert_eq!(negotiate_sdu(DEFAULT_SDU + 1000), DEFAULT_SDU);
        assert_eq!(negotiate_sdu(100), 512);
    }

    #[test]
    fn negotiate_tdu_clamps_to_range() {
        // 验证 TDU 协商：取较小值，最小 512
        assert_eq!(negotiate_tdu(1024), 1024);
        assert_eq!(negotiate_tdu(DEFAULT_TDU), DEFAULT_TDU);
        assert_eq!(negotiate_tdu(DEFAULT_TDU + 2000), DEFAULT_TDU);
        assert_eq!(negotiate_tdu(50), 512);
    }

    #[test]
    fn connect_request_payload_layout_matches_spec() {
        // 验证 Connect Request 负载的字节布局符合协议规范
        let req = ConnectRequest::new("ORCL").unwrap();
        let payload = req.encode_payload();
        // 长度 = 24 + 服务名字节数
        assert_eq!(payload.len(), CONNECT_FIXED_LEN + 4);
        // Version 偏移 0
        assert_eq!(
            u16::from_be_bytes([payload[0], payload[1]]),
            TNS_VERSION_314
        );
        // Connection Data Length 偏移 16
        let conn_data_len = u16::from_be_bytes([payload[16], payload[17]]);
        assert_eq!(conn_data_len as usize, 4);
        // Connection Data Offset 偏移 18
        let conn_data_offset = u16::from_be_bytes([payload[18], payload[19]]);
        assert_eq!(conn_data_offset, CONNECT_DATA_OFFSET);
        // 服务名尾部
        assert_eq!(&payload[CONNECT_FIXED_LEN..], b"ORCL");
    }

    #[test]
    fn connect_request_decodes_real_world_layout() {
        // 构造一个符合规范的字节序列，验证可被正确解析
        let service_name = b"ORCLPDB";
        let mut buf = Vec::with_capacity(CONNECT_FIXED_LEN + service_name.len());
        // Version
        buf.extend_from_slice(&TNS_VERSION_314.to_be_bytes());
        // Version Compatibility
        buf.extend_from_slice(&TNS_VERSION_314.to_be_bytes());
        // Service Options
        buf.extend_from_slice(&0x0C41u16.to_be_bytes());
        // SDU
        buf.extend_from_slice(&DEFAULT_SDU.to_be_bytes());
        // TDU
        buf.extend_from_slice(&DEFAULT_TDU.to_be_bytes());
        // NT Protocol Chars
        buf.extend_from_slice(&0x4F98u16.to_be_bytes());
        // Line Turnaround
        buf.extend_from_slice(&0x0001u16.to_be_bytes());
        // Value of 1
        buf.extend_from_slice(&0x0001u16.to_be_bytes());
        // Connection Data Length = 服务名字节数
        buf.extend_from_slice(&(service_name.len() as u16).to_be_bytes());
        // Connection Data Offset
        buf.extend_from_slice(&CONNECT_DATA_OFFSET.to_be_bytes());
        // Max Receive Size
        buf.extend_from_slice(&DEFAULT_MAX_RECEIVE.to_be_bytes());
        // Service name
        buf.extend_from_slice(service_name);

        let req = ConnectRequest::decode_payload(&buf).unwrap();
        assert_eq!(req.version, TNS_VERSION_314);
        assert_eq!(req.service_name, "ORCLPDB");
        assert_eq!(req.sdu, DEFAULT_SDU);
    }

    #[test]
    fn accept_response_payload_layout_matches_spec() {
        // 验证 Accept Response 负载的字节布局符合协议规范
        let resp = AcceptResponse::new(TNS_VERSION_314);
        let payload = resp.encode_payload();
        assert_eq!(payload.len(), ACCEPT_FIXED_LEN);
        // Version 偏移 0
        assert_eq!(
            u16::from_be_bytes([payload[0], payload[1]]),
            TNS_VERSION_314
        );
        // SDU 偏移 4
        assert_eq!(u16::from_be_bytes([payload[4], payload[5]]), DEFAULT_SDU);
        // TDU 偏移 6
        assert_eq!(u16::from_be_bytes([payload[6], payload[7]]), DEFAULT_TDU);
    }

    #[test]
    fn encoded_connect_packet_total_length() {
        // 验证 Connect 包的总长度（含 TNS 头部）
        let req = ConnectRequest::new("ORCL").unwrap();
        let packet = req.encode();
        assert_eq!(
            packet.encoded_len(),
            crate::tns_packet::TNS_HEADER_LEN + CONNECT_FIXED_LEN + 4
        );
    }
}
