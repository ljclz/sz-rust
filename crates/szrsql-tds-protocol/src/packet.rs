//! TDS 协议包编解码。
//!
//! TDS 协议包格式：
//! ```text
//! +-----------+------------+----------------+----------+
//! | type (1)  | status (1) | length (2, BE) | payload  |
//! +-----------+------------+----------------+----------+
//! ```
//!
//! - `type`：TDS 包类型（0x01 SQLBatch / 0x04 Response / 0x10 Login7 / 0x11 Pre-Login 等）
//! - `status`：状态标志位（0x01 = EOM, end of message）
//! - `length`：**整个包**长度（含 4 字节头部，大端序，最大 65535）
//! - `payload`：实际数据
//!
//! 当请求/响应体超过单包最大 payload（65531 字节）时，需分多个包发送，
//! 除最后一个包外其余包 status 不带 EOM 位。

use bytes::{BufMut, BytesMut};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// TDS 包头部长度（4 字节）。
pub const HEADER_LEN: usize = 4;

/// TDS 单包最大长度（u16 最大值 65535）。
pub const MAX_PACKET_LEN: usize = 0xFFFF;

/// TDS 单包最大 payload 长度（65535 - 4 字节头部）。
pub const MAX_PAYLOAD_LEN: usize = MAX_PACKET_LEN - HEADER_LEN;

/// TDS 包类型枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TdsPacketType {
    /// SQL Batch 请求（客户端 → 服务器）
    SqlBatch = 0x01,
    /// Pre-TDS7 Login（旧版登录，已废弃）
    PreTds7Login = 0x02,
    /// RPC 请求（客户端 → 服务器）
    Rpc = 0x03,
    /// 响应（服务器 → 客户端，承载 token 流）
    Response = 0x04,
    /// Login7（TDS 7.1+ 登录请求）
    Login7 = 0x10,
    /// Pre-Login 握手
    PreLogin = 0x11,
    /// Token-less 流（特殊用途）
    TokenlessStream = 0x12,
}

impl TdsPacketType {
    /// 从字节解析包类型。
    pub fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0x01 => TdsPacketType::SqlBatch,
            0x02 => TdsPacketType::PreTds7Login,
            0x03 => TdsPacketType::Rpc,
            0x04 => TdsPacketType::Response,
            0x10 => TdsPacketType::Login7,
            0x11 => TdsPacketType::PreLogin,
            0x12 => TdsPacketType::TokenlessStream,
            _ => return None,
        })
    }
}

/// TDS 包状态标志位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TdsPacketStatus(pub u8);

impl TdsPacketStatus {
    /// 正常状态（无 EOM，表示后续还有包）
    pub const NORMAL: TdsPacketStatus = TdsPacketStatus(0x00);
    /// End-of-Message：本包是消息的最后一个
    pub const EOM: TdsPacketStatus = TdsPacketStatus(0x01);
    /// 客户端期望 RESET（连接重置）
    pub const RESET_CONNECTION: TdsPacketStatus = TdsPacketStatus(0x04);
    /// RESET 且跳过事务
    pub const RESET_CONNECTION_SKIP_TRAN: TdsPacketStatus = TdsPacketStatus(0x08);

    /// 创建新状态标志。
    pub fn new(value: u8) -> Self {
        TdsPacketStatus(value)
    }

    /// 是否设置了 EOM 位。
    pub fn is_eom(self) -> bool {
        self.0 & 0x01 != 0
    }

    /// 设置/清除 EOM 位。
    pub fn with_eom(mut self, on: bool) -> Self {
        if on {
            self.0 |= 0x01;
        } else {
            self.0 &= !0x01;
        }
        self
    }
}

impl Default for TdsPacketStatus {
    fn default() -> Self {
        TdsPacketStatus::EOM
    }
}

/// TDS 协议包编解码错误。
#[derive(Debug, Error)]
pub enum PacketError {
    /// IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// payload 超过单包最大长度
    #[error("payload too large: {0} bytes (max {MAX_PAYLOAD_LEN})")]
    PayloadTooLarge(usize),
    /// 包不完整
    #[error("incomplete packet: expected {expected} bytes, got {got}")]
    Incomplete { expected: usize, got: usize },
    /// 长度字段小于头部（非法）
    #[error("invalid length {0}: smaller than header length {HEADER_LEN}")]
    InvalidLength(u16),
    /// 未知包类型
    #[error("unknown packet type: 0x{0:02X}")]
    UnknownType(u8),
}

/// TDS 协议包。
///
/// 一个完整的 TDS 协议包，包含类型、状态和 payload。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TdsPacket {
    /// 包类型
    pub packet_type: TdsPacketType,
    /// 状态标志
    pub status: TdsPacketStatus,
    /// 实际数据
    pub payload: Vec<u8>,
}

impl TdsPacket {
    /// 创建新包（默认 status = EOM）。
    ///
    /// 注意：`payload` 可超过单包最大 payload 长度（`MAX_PAYLOAD_LEN`），
    /// `PacketCodec::write_packet` 会自动分片发送。
    pub fn new(packet_type: TdsPacketType, payload: Vec<u8>) -> Result<Self, PacketError> {
        Ok(Self {
            packet_type,
            status: TdsPacketStatus::EOM,
            payload,
        })
    }

    /// 创建空 payload 包（仅用于控制信号）。
    pub fn empty(packet_type: TdsPacketType) -> Self {
        Self {
            packet_type,
            status: TdsPacketStatus::EOM,
            payload: Vec::new(),
        }
    }

    /// 显式设置 status。
    pub fn with_status(mut self, status: TdsPacketStatus) -> Self {
        self.status = status;
        self
    }

    /// 编码到 `BytesMut`（含 4 字节头部，大端序）。
    pub fn encode(&self) -> BytesMut {
        let total = HEADER_LEN + self.payload.len();
        let mut buf = BytesMut::with_capacity(total);
        // 1 字节类型
        buf.put_u8(self.packet_type as u8);
        // 1 字节 status
        buf.put_u8(self.status.0);
        // 2 字节大端长度（含头部）
        buf.put_u16(total as u16);
        // payload
        buf.put_slice(&self.payload);
        buf
    }

    /// 从字节切片解析单个包（假设数据已完整读取）。
    ///
    /// 返回 (包, 已消费字节数)。
    pub fn decode(buf: &[u8]) -> Result<(Self, usize), PacketError> {
        if buf.len() < HEADER_LEN {
            return Err(PacketError::Incomplete {
                expected: HEADER_LEN,
                got: buf.len(),
            });
        }
        let packet_type_byte = buf[0];
        let status_byte = buf[1];
        let total = u16::from_be_bytes([buf[2], buf[3]]) as usize;
        if total < HEADER_LEN {
            return Err(PacketError::InvalidLength(total as u16));
        }
        if buf.len() < total {
            return Err(PacketError::Incomplete {
                expected: total,
                got: buf.len(),
            });
        }
        let packet_type = TdsPacketType::from_byte(packet_type_byte)
            .ok_or(PacketError::UnknownType(packet_type_byte))?;
        let payload = buf[HEADER_LEN..total].to_vec();
        Ok((
            Self {
                packet_type,
                status: TdsPacketStatus(status_byte),
                payload,
            },
            total,
        ))
    }
}

/// TDS 协议包编解码器（异步流式）。
///
/// 提供对 `AsyncRead + AsyncWrite` 的扩展，可直接读写 TDS 包。
pub struct PacketCodec;

impl PacketCodec {
    /// 从流中读取一个完整的 TDS 消息（可能由多个包组成）。
    ///
    /// 持续读取直到遇到带 EOM 标志的包，将所有 payload 拼接后返回。
    /// 返回的 `TdsPacket` 中 `packet_type` 和 `status` 取自最后一个包，
    /// `payload` 是所有包 payload 的拼接结果。
    pub async fn read_packet<R: AsyncRead + Unpin>(
        reader: &mut R,
    ) -> Result<TdsPacket, PacketError> {
        let mut aggregated_payload = Vec::new();
        // 初始值在循环内首帧即被覆盖；Rust 无法证明 loop 至少执行一次，
        // 因此需要初始值以满足定赋值检查。此处允许 unused_assignments。
        #[allow(unused_assignments)]
        let mut last_packet_type: Option<TdsPacketType> = None;
        #[allow(unused_assignments)]
        let mut last_status = TdsPacketStatus::NORMAL;

        loop {
            let mut header = [0u8; HEADER_LEN];
            reader.read_exact(&mut header).await?;
            let packet_type_byte = header[0];
            let status_byte = header[1];
            let total = u16::from_be_bytes([header[2], header[3]]) as usize;
            if total < HEADER_LEN {
                return Err(PacketError::InvalidLength(total as u16));
            }
            let payload_len = total - HEADER_LEN;
            let mut payload = vec![0u8; payload_len];
            if payload_len > 0 {
                reader.read_exact(&mut payload).await?;
            }
            aggregated_payload.extend_from_slice(&payload);
            let packet_type = TdsPacketType::from_byte(packet_type_byte)
                .ok_or(PacketError::UnknownType(packet_type_byte))?;
            last_packet_type = Some(packet_type);
            last_status = TdsPacketStatus(status_byte);
            if last_status.is_eom() {
                break;
            }
        }

        Ok(TdsPacket {
            packet_type: last_packet_type.ok_or(PacketError::Incomplete {
                expected: 1,
                got: 0,
            })?,
            status: last_status,
            payload: aggregated_payload,
        })
    }

    /// 向流中写入一个 TDS 消息。
    ///
    /// 自动分片：当 payload > MAX_PAYLOAD_LEN 时拆分为多个包，
    /// 除最后一个包外其余包 status 不带 EOM 位。
    pub async fn write_packet<W: AsyncWrite + Unpin>(
        writer: &mut W,
        packet: &TdsPacket,
    ) -> Result<(), PacketError> {
        if packet.payload.len() <= MAX_PAYLOAD_LEN {
            let bytes = packet.encode();
            writer.write_all(&bytes).await?;
            writer.flush().await?;
            return Ok(());
        }
        // 分片写入
        let packet_type = packet.packet_type;
        let mut offset = 0usize;
        while offset < packet.payload.len() {
            let chunk_end = (offset + MAX_PAYLOAD_LEN).min(packet.payload.len());
            let chunk = &packet.payload[offset..chunk_end];
            let is_last = chunk_end == packet.payload.len();
            let status = if is_last {
                TdsPacketStatus::EOM
            } else {
                TdsPacketStatus::NORMAL
            };
            let total = HEADER_LEN + chunk.len();
            let mut buf = BytesMut::with_capacity(total);
            buf.put_u8(packet_type as u8);
            buf.put_u8(status.0);
            buf.put_u16(total as u16);
            buf.put_slice(chunk);
            writer.write_all(&buf).await?;
            offset = chunk_end;
        }
        writer.flush().await?;
        Ok(())
    }
}

/// 读取以 NUL（0x00）结尾的字符串（小端无符号长度前缀常见于 TDS 变长字段除外）。
pub fn read_nul_string(buf: &mut &[u8]) -> Option<String> {
    let pos = buf.iter().position(|&b| b == 0)?;
    let s = String::from_utf8_lossy(&buf[..pos]).to_string();
    *buf = &buf[pos + 1..];
    Some(s)
}

/// 写入以 NUL 结尾的字符串。
pub fn write_nul_string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
}

/// 读取以 1 字节无符号长度前缀的 ASCII 字符串（US-VARCHAR）。
pub fn read_us_varchar(buf: &mut &[u8]) -> Option<String> {
    if buf.is_empty() {
        return None;
    }
    let len = buf[0] as usize;
    *buf = &buf[1..];
    if buf.len() < len {
        return None;
    }
    let s = String::from_utf8_lossy(&buf[..len]).to_string();
    *buf = &buf[len..];
    Some(s)
}

/// 写入 1 字节长度前缀的 ASCII 字符串（US-VARCHAR）。
pub fn write_us_varchar(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    debug_assert!(bytes.len() <= 255, "US-VARCHAR 长度超过 255");
    buf.push(bytes.len() as u8);
    buf.extend_from_slice(bytes);
}

/// 读取 2 字节小端长度前缀的 ASCII 字符串（B-VARCHAR）。
pub fn read_b_varchar(buf: &mut &[u8]) -> Option<String> {
    if buf.len() < 2 {
        return None;
    }
    let len = u16::from_le_bytes([buf[0], buf[1]]) as usize;
    *buf = &buf[2..];
    if buf.len() < len {
        return None;
    }
    let s = String::from_utf8_lossy(&buf[..len]).to_string();
    *buf = &buf[len..];
    Some(s)
}

/// 写入 2 字节小端长度前缀的 ASCII 字符串（B-VARCHAR）。
pub fn write_b_varchar(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(bytes);
}

/// 读取 2 字节小端长度前缀的 UTF-16LE 字符串（B-VARCHAR，NCHAR/NVARCHAR）。
pub fn read_b_varchar_utf16(buf: &mut &[u8]) -> Option<String> {
    if buf.len() < 2 {
        return None;
    }
    let len = u16::from_le_bytes([buf[0], buf[1]]) as usize;
    *buf = &buf[2..];
    if buf.len() < len {
        return None;
    }
    let utf16_units: Vec<u16> = buf[..len]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    *buf = &buf[len..];
    String::from_utf16(&utf16_units).ok()
}

/// 写入 2 字节小端长度前缀的 UTF-16LE 字符串（B-VARCHAR）。
pub fn write_b_varchar_utf16(buf: &mut Vec<u8>, s: &str) {
    let utf16: Vec<u16> = s.encode_utf16().collect();
    let byte_len = utf16.len() * 2;
    buf.extend_from_slice(&(byte_len as u16).to_le_bytes());
    for unit in utf16 {
        buf.extend_from_slice(&unit.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_type_from_byte_known() {
        assert_eq!(
            TdsPacketType::from_byte(0x01),
            Some(TdsPacketType::SqlBatch)
        );
        assert_eq!(
            TdsPacketType::from_byte(0x04),
            Some(TdsPacketType::Response)
        );
        assert_eq!(TdsPacketType::from_byte(0x10), Some(TdsPacketType::Login7));
        assert_eq!(
            TdsPacketType::from_byte(0x11),
            Some(TdsPacketType::PreLogin)
        );
        assert_eq!(
            TdsPacketType::from_byte(0x12),
            Some(TdsPacketType::TokenlessStream)
        );
    }

    #[test]
    fn test_packet_type_from_byte_unknown() {
        assert_eq!(TdsPacketType::from_byte(0xFF), None);
        assert_eq!(TdsPacketType::from_byte(0x99), None);
        assert_eq!(TdsPacketType::from_byte(0x00), None);
    }

    #[test]
    fn test_packet_encode_decode_roundtrip() {
        let payload = b"SELECT 1".to_vec();
        let original = TdsPacket::new(TdsPacketType::SqlBatch, payload).unwrap();
        let encoded = original.encode();
        assert_eq!(encoded.len(), HEADER_LEN + 8);
        // 大端长度
        assert_eq!(&encoded[2..4], &(12u16).to_be_bytes());
        let (decoded, consumed) = TdsPacket::decode(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_packet_empty_payload() {
        let p = TdsPacket::empty(TdsPacketType::PreLogin);
        let encoded = p.encode();
        assert_eq!(encoded.len(), HEADER_LEN);
        assert_eq!(encoded[0], 0x11);
        assert_eq!(encoded[1], 0x01); // EOM
        assert_eq!(&encoded[2..4], &4u16.to_be_bytes());
        let (decoded, _) = TdsPacket::decode(&encoded).unwrap();
        assert_eq!(decoded.payload, Vec::<u8>::new());
        assert_eq!(decoded.packet_type, TdsPacketType::PreLogin);
        assert!(decoded.status.is_eom());
    }

    #[test]
    fn test_packet_decode_incomplete_header() {
        let buf = [0u8; 3];
        let result = TdsPacket::decode(&buf);
        assert!(matches!(result, Err(PacketError::Incomplete { .. })));
    }

    #[test]
    fn test_packet_decode_incomplete_payload() {
        let mut buf = vec![0x01u8, 0x01, 0x00, 0x10]; // length=16 但实际只有 0 字节 payload
        buf.extend_from_slice(&[0u8; 2]); // 不足 12 字节
        let result = TdsPacket::decode(&buf);
        assert!(matches!(result, Err(PacketError::Incomplete { .. })));
    }

    #[test]
    fn test_packet_decode_invalid_length() {
        // length=2 < HEADER_LEN=4
        let buf = [0x01u8, 0x01, 0x00, 0x02];
        let result = TdsPacket::decode(&buf);
        assert!(matches!(result, Err(PacketError::InvalidLength(_))));
    }

    #[test]
    fn test_packet_decode_unknown_type() {
        let buf = [0xABu8, 0x01, 0x00, 0x04];
        let result = TdsPacket::decode(&buf);
        assert!(matches!(result, Err(PacketError::UnknownType(0xAB))));
    }

    #[test]
    fn test_packet_new_accepts_large_payload() {
        // TdsPacket::new 接受超过单包最大 payload 的数据，
        // 由 PacketCodec::write_packet 自动分片发送。
        let huge = vec![0u8; MAX_PAYLOAD_LEN + 1];
        let result = TdsPacket::new(TdsPacketType::SqlBatch, huge.clone());
        assert!(result.is_ok());
        assert_eq!(result.unwrap().payload.len(), huge.len());
    }

    #[test]
    fn test_status_eom_flag() {
        let s = TdsPacketStatus::EOM;
        assert!(s.is_eom());
        let s2 = s.with_eom(false);
        assert!(!s2.is_eom());
        assert_eq!(s2.0, 0x00);
        let s3 = s2.with_eom(true);
        assert!(s3.is_eom());
        assert_eq!(s3.0, 0x01);
    }

    #[test]
    fn test_status_reset_flags() {
        let s = TdsPacketStatus::RESET_CONNECTION;
        assert!(!s.is_eom());
        assert_eq!(s.0, 0x04);
        let s2 = TdsPacketStatus::RESET_CONNECTION_SKIP_TRAN;
        assert_eq!(s2.0, 0x08);
    }

    #[test]
    fn test_nul_string_roundtrip() {
        let mut buf = Vec::new();
        write_nul_string(&mut buf, "tds_user");
        let mut slice = buf.as_slice();
        let s = read_nul_string(&mut slice).unwrap();
        assert_eq!(s, "tds_user");
        assert!(slice.is_empty());
    }

    #[test]
    fn test_us_varchar_roundtrip() {
        let mut buf = Vec::new();
        write_us_varchar(&mut buf, "instance");
        let mut slice = buf.as_slice();
        let s = read_us_varchar(&mut slice).unwrap();
        assert_eq!(s, "instance");
    }

    #[test]
    fn test_b_varchar_roundtrip() {
        let mut buf = Vec::new();
        write_b_varchar(&mut buf, "server_name");
        let mut slice = buf.as_slice();
        let s = read_b_varchar(&mut slice).unwrap();
        assert_eq!(s, "server_name");
    }

    #[test]
    fn test_b_varchar_utf16_roundtrip() {
        let mut buf = Vec::new();
        write_b_varchar_utf16(&mut buf, "中文测试");
        let mut slice = buf.as_slice();
        let s = read_b_varchar_utf16(&mut slice).unwrap();
        assert_eq!(s, "中文测试");
    }

    #[test]
    fn test_b_varchar_utf16_ascii() {
        let mut buf = Vec::new();
        write_b_varchar_utf16(&mut buf, "sa");
        let mut slice = buf.as_slice();
        let s = read_b_varchar_utf16(&mut slice).unwrap();
        assert_eq!(s, "sa");
    }

    #[tokio::test]
    async fn test_packet_codec_read_write_roundtrip() {
        use tokio::io::duplex;

        let (mut client, mut server) = duplex(1024);
        let original = TdsPacket::new(TdsPacketType::SqlBatch, b"SELECT 1".to_vec()).unwrap();
        PacketCodec::write_packet(&mut server, &original)
            .await
            .unwrap();

        let received = PacketCodec::read_packet(&mut client).await.unwrap();
        assert_eq!(received, original);
    }

    #[tokio::test]
    async fn test_packet_codec_multiple_packets() {
        use tokio::io::duplex;

        let (mut client, mut server) = duplex(4096);
        let p1 = TdsPacket::new(TdsPacketType::PreLogin, vec![0x01, 0x02]).unwrap();
        let p2 = TdsPacket::new(TdsPacketType::Login7, vec![0x10, 0x20]).unwrap();
        PacketCodec::write_packet(&mut server, &p1).await.unwrap();
        PacketCodec::write_packet(&mut server, &p2).await.unwrap();

        let r1 = PacketCodec::read_packet(&mut client).await.unwrap();
        let r2 = PacketCodec::read_packet(&mut client).await.unwrap();
        assert_eq!(r1, p1);
        assert_eq!(r2, p2);
    }

    #[tokio::test]
    async fn test_packet_codec_fragmented_payload() {
        // 模拟分片写入：超大 payload 应被拆分为多个包
        use tokio::io::duplex;

        let (mut client, mut server) = duplex(64 * 1024 * 2);
        let big_payload = vec![0xABu8; MAX_PAYLOAD_LEN + 100];
        let original = TdsPacket::new(TdsPacketType::Response, big_payload.clone()).unwrap();
        PacketCodec::write_packet(&mut server, &original)
            .await
            .unwrap();

        let received = PacketCodec::read_packet(&mut client).await.unwrap();
        assert_eq!(received.packet_type, TdsPacketType::Response);
        assert_eq!(received.payload, big_payload);
    }

    #[test]
    fn test_b_varchar_truncated() {
        let buf = [0x05u8, 0x00, b'a']; // 声明长度 5 但只有 1 字节
        let mut slice = &buf[..];
        assert!(read_b_varchar(&mut slice).is_none());
    }
}
