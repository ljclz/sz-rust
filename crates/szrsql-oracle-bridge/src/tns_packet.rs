//! TNS（Transparent Network Substrate）协议包格式编解码。
//!
//! TNS 是 Oracle Net 使用的网络协议，本模块实现其包格式编解码，
//! 为 L2 协议级兼容提供基础。
//!
//! # TNS 包格式（Oracle Net 12c+）
//!
//! ```text
//! +---------------------+---------------------+----------+--------+---------------------+----------+
//! | Packet Length (2B)  | Packet Chksum (2B)  | Type (1B)| Flg(1B)| Header Chksum (2B) | Data ... |
//! |     (big-endian)    |     (big-endian)    |          |        |     (big-endian)    |          |
//! +---------------------+---------------------+----------+--------+---------------------+----------+
//! ```
//!
//! - **Packet Length**：整个包的字节数（含头部），大端
//! - **Packet Checksum**：包数据校验和（Oracle 12c+ 通常为 0）
//! - **Packet Type**：见 [`PacketType`]
//! - **Flags**：`0x00` 普通，`0x04` 重定向
//! - **Header Checksum**：头部校验和（通常为 0）
//! - **Data**：变长负载

use bytes::{BufMut, BytesMut};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// TNS 包头部长度（8 字节）。
pub const TNS_HEADER_LEN: usize = 8;

/// TNS 包最大长度（约 16MB，与 Oracle SDU 上限对齐）。
pub const TNS_MAX_PACKET_LEN: usize = 0x00FF_FFFF;

/// TNS 包类型。
///
/// 参考 Oracle Net 协议规范。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PacketType {
    /// Connect：客户端连接请求
    Connect = 0x01,
    /// Accept：服务器接受连接
    Accept = 0x02,
    /// Refuse：服务器拒绝连接
    Refuse = 0x03,
    /// Redirect：服务器重定向
    Redirect = 0x04,
    /// Data：数据包（SQL 请求/响应）
    Data = 0x05,
    /// Control：控制包（如重置序列）
    Control = 0x06,
    /// RowData：行数据
    RowData = 0x0E,
    /// OK：操作成功
    Ok = 0x10,
    /// Error：操作失败
    Error = 0x12,
}

impl PacketType {
    /// 从原始字节构造 [`PacketType`]，未知值返回 `None`。
    pub fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0x01 => Some(Self::Connect),
            0x02 => Some(Self::Accept),
            0x03 => Some(Self::Refuse),
            0x04 => Some(Self::Redirect),
            0x05 => Some(Self::Data),
            0x06 => Some(Self::Control),
            0x0E => Some(Self::RowData),
            0x10 => Some(Self::Ok),
            0x12 => Some(Self::Error),
            _ => None,
        }
    }

    /// 返回原始字节值。
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// TNS 包标志位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketFlags(pub u8);

impl PacketFlags {
    /// 普通数据包。
    pub const NORMAL: u8 = 0x00;
    /// 重定向包。
    pub const REDIRECT: u8 = 0x04;

    /// 构造一个普通标志位。
    pub fn new() -> Self {
        Self(Self::NORMAL)
    }

    /// 设置为重定向标志。
    pub fn redirect() -> Self {
        Self(Self::REDIRECT)
    }

    /// 返回原始字节值。
    pub fn as_u8(self) -> u8 {
        self.0
    }
}

impl Default for PacketFlags {
    fn default() -> Self {
        Self::new()
    }
}

/// TNS 包编解码错误。
#[derive(Debug, Error)]
pub enum TnsPacketError {
    /// IO 错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// 包长度不足头部最小值。
    #[error("packet length {0} is less than header length {TNS_HEADER_LEN}")]
    HeaderTooShort(usize),
    /// 包长度超过最大值。
    #[error("packet length {0} exceeds max {TNS_MAX_PACKET_LEN}")]
    PacketTooLarge(usize),
    /// 包不完整（声明长度与实际字节数不匹配）。
    #[error("incomplete packet: declared {declared} bytes, got {got} bytes")]
    Incomplete {
        /// 声明的总长度（含头部）
        declared: usize,
        /// 实际接收的字节数
        got: usize,
    },
    /// 未知的包类型。
    #[error("unknown packet type: 0x{0:02X}")]
    UnknownType(u8),
}

/// TNS 包结构。
///
/// 包含包类型、标志位与负载。校验和字段在编解码时填 0
/// （Oracle 12c+ 默认禁用校验和，校验由 TCP/网络层保证）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TnsPacket {
    /// 包类型
    pub packet_type: PacketType,
    /// 标志位
    pub flags: PacketFlags,
    /// 负载数据
    pub data: Vec<u8>,
}

impl TnsPacket {
    /// 构造一个新的 TNS 包。
    pub fn new(packet_type: PacketType, data: Vec<u8>) -> Self {
        Self {
            packet_type,
            flags: PacketFlags::new(),
            data,
        }
    }

    /// 构造一个带指定标志位的 TNS 包。
    pub fn with_flags(packet_type: PacketType, flags: PacketFlags, data: Vec<u8>) -> Self {
        Self {
            packet_type,
            flags,
            data,
        }
    }

    /// 构造一个 Data 类型包。
    pub fn data_packet(data: Vec<u8>) -> Self {
        Self::new(PacketType::Data, data)
    }

    /// 计算包编码后的总长度（含头部）。
    pub fn encoded_len(&self) -> usize {
        TNS_HEADER_LEN + self.data.len()
    }

    /// 编码为 `BytesMut`（含头部）。
    ///
    /// 编码格式：
    /// - 2 字节大端总长度（含头部）
    /// - 2 字节包校验和（填 0）
    /// - 1 字节类型
    /// - 1 字节标志位
    /// - 2 字节头校验和（填 0）
    /// - 负载数据
    pub fn encode(&self) -> BytesMut {
        let total_len = self.encoded_len();
        let mut buf = BytesMut::with_capacity(total_len);
        // 包总长度（大端）
        buf.put_u16(total_len as u16);
        // 包校验和（0）
        buf.put_u16(0);
        // 类型
        buf.put_u8(self.packet_type.as_u8());
        // 标志位
        buf.put_u8(self.flags.as_u8());
        // 头校验和（0）
        buf.put_u16(0);
        // 负载
        buf.put_slice(&self.data);
        buf
    }

    /// 从字节切片解析单个包（假设数据已完整读取）。
    ///
    /// 返回 `(包, 已消费字节数)`。
    pub fn decode(buf: &[u8]) -> Result<(Self, usize), TnsPacketError> {
        if buf.len() < TNS_HEADER_LEN {
            return Err(TnsPacketError::HeaderTooShort(buf.len()));
        }
        let total_len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
        if total_len < TNS_HEADER_LEN {
            return Err(TnsPacketError::HeaderTooShort(total_len));
        }
        if total_len > TNS_MAX_PACKET_LEN {
            return Err(TnsPacketError::PacketTooLarge(total_len));
        }
        if buf.len() < total_len {
            return Err(TnsPacketError::Incomplete {
                declared: total_len,
                got: buf.len(),
            });
        }
        // 跳过 2 字节包校验和（偏移 2-3）
        let packet_type = PacketType::from_u8(buf[4])
            .ok_or_else(|| TnsPacketError::UnknownType(buf[4]))?;
        let flags = PacketFlags(buf[5]);
        // 跳过 2 字节头校验和（偏移 6-7）
        let data = buf[TNS_HEADER_LEN..total_len].to_vec();
        Ok((
            Self {
                packet_type,
                flags,
                data,
            },
            total_len,
        ))
    }
}

/// TNS 包异步编解码器。
///
/// 提供对 `AsyncRead + AsyncWrite` 的扩展，可直接读写 TNS 包。
pub struct TnsPacketCodec;

impl TnsPacketCodec {
    /// 从流中异步读取一个完整的 TNS 包。
    ///
    /// 先读取 8 字节头部，根据长度字段读取剩余负载。
    pub async fn read_packet<R: AsyncRead + Unpin>(
        reader: &mut R,
    ) -> Result<TnsPacket, TnsPacketError> {
        let mut header = [0u8; TNS_HEADER_LEN];
        reader.read_exact(&mut header).await?;
        let total_len = u16::from_be_bytes([header[0], header[1]]) as usize;
        if total_len < TNS_HEADER_LEN {
            return Err(TnsPacketError::HeaderTooShort(total_len));
        }
        if total_len > TNS_MAX_PACKET_LEN {
            return Err(TnsPacketError::PacketTooLarge(total_len));
        }
        let payload_len = total_len - TNS_HEADER_LEN;
        let mut data = vec![0u8; payload_len];
        if payload_len > 0 {
            reader.read_exact(&mut data).await?;
        }
        let packet_type = PacketType::from_u8(header[4])
            .ok_or_else(|| TnsPacketError::UnknownType(header[4]))?;
        let flags = PacketFlags(header[5]);
        Ok(TnsPacket {
            packet_type,
            flags,
            data,
        })
    }

    /// 向流中异步写入一个 TNS 包。
    pub async fn write_packet<W: AsyncWrite + Unpin>(
        writer: &mut W,
        packet: &TnsPacket,
    ) -> Result<(), TnsPacketError> {
        let bytes = packet.encode();
        writer.write_all(&bytes).await?;
        writer.flush().await?;
        Ok(())
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_type_from_u8_known_types() {
        // 验证已知类型可正确解析
        assert_eq!(PacketType::from_u8(0x01), Some(PacketType::Connect));
        assert_eq!(PacketType::from_u8(0x02), Some(PacketType::Accept));
        assert_eq!(PacketType::from_u8(0x03), Some(PacketType::Refuse));
        assert_eq!(PacketType::from_u8(0x04), Some(PacketType::Redirect));
        assert_eq!(PacketType::from_u8(0x05), Some(PacketType::Data));
        assert_eq!(PacketType::from_u8(0x06), Some(PacketType::Control));
        assert_eq!(PacketType::from_u8(0x0E), Some(PacketType::RowData));
        assert_eq!(PacketType::from_u8(0x10), Some(PacketType::Ok));
        assert_eq!(PacketType::from_u8(0x12), Some(PacketType::Error));
    }

    #[test]
    fn packet_type_from_u8_unknown_returns_none() {
        // 未知类型应返回 None
        assert_eq!(PacketType::from_u8(0x00), None);
        assert_eq!(PacketType::from_u8(0xFF), None);
        assert_eq!(PacketType::from_u8(0x99), None);
    }

    #[test]
    fn packet_type_as_u8_roundtrip() {
        // 验证 as_u8 与 from_u8 互逆
        for byte in [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x0E, 0x10, 0x12] {
            let pt = PacketType::from_u8(byte).expect("known type");
            assert_eq!(pt.as_u8(), byte);
        }
    }

    #[test]
    fn encode_decode_roundtrip_with_payload() {
        // 验证带负载的编解码往返
        let payload = b"SELECT 1 FROM dual".to_vec();
        let original = TnsPacket::data_packet(payload);
        let encoded = original.encode();
        // 头部 + 负载
        assert_eq!(encoded.len(), TNS_HEADER_LEN + 18);
        // 长度字段（大端）
        assert_eq!(&encoded[0..2], &(encoded.len() as u16).to_be_bytes());
        // 类型字段
        assert_eq!(encoded[4], PacketType::Data.as_u8());

        let (decoded, consumed) = TnsPacket::decode(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, original);
    }

    #[test]
    fn encode_decode_empty_payload() {
        // 验证空负载的编解码
        let original = TnsPacket::new(PacketType::Ok, Vec::new());
        let encoded = original.encode();
        assert_eq!(encoded.len(), TNS_HEADER_LEN);
        let (decoded, consumed) = TnsPacket::decode(&encoded).unwrap();
        assert_eq!(consumed, TNS_HEADER_LEN);
        assert_eq!(decoded, original);
        assert!(decoded.data.is_empty());
    }

    #[test]
    fn decode_rejects_short_header() {
        // 验证数据长度不足头部时返回错误
        let buf = [0u8; 4];
        let result = TnsPacket::decode(&buf);
        assert!(matches!(result, Err(TnsPacketError::HeaderTooShort(4))));
    }

    #[test]
    fn decode_rejects_incomplete_packet() {
        // 验证声明长度超过实际数据时返回 Incomplete 错误
        let mut buf = vec![0u8; TNS_HEADER_LEN];
        // 设置声明长度为 16，但实际只有 8 字节
        buf[0..2].copy_from_slice(&16u16.to_be_bytes());
        let result = TnsPacket::decode(&buf);
        assert!(matches!(
            result,
            Err(TnsPacketError::Incomplete {
                declared: 16,
                got: 8
            })
        ));
    }

    #[test]
    fn decode_rejects_unknown_packet_type() {
        // 验证未知类型返回 UnknownType 错误
        let mut buf = vec![0u8; TNS_HEADER_LEN];
        // 总长度 = 8
        buf[0..2].copy_from_slice(&8u16.to_be_bytes());
        // 类型设为 0xAA（未知）
        buf[4] = 0xAA;
        let result = TnsPacket::decode(&buf);
        assert!(matches!(result, Err(TnsPacketError::UnknownType(0xAA))));
    }

    #[test]
    fn packet_flags_default_and_redirect() {
        // 验证标志位的默认值与重定向值
        assert_eq!(PacketFlags::new().as_u8(), 0x00);
        assert_eq!(PacketFlags::redirect().as_u8(), 0x04);
        assert_eq!(PacketFlags::default().as_u8(), 0x00);
    }

    #[test]
    fn encode_decode_preserves_flags() {
        // 验证标志位在编解码往返中保持不变
        let original = TnsPacket::with_flags(
            PacketType::Redirect,
            PacketFlags::redirect(),
            b"redirect payload".to_vec(),
        );
        let encoded = original.encode();
        let (decoded, _) = TnsPacket::decode(&encoded).unwrap();
        assert_eq!(decoded.flags, original.flags);
        assert_eq!(decoded.packet_type, PacketType::Redirect);
    }

    #[test]
    fn encoded_len_includes_header() {
        // 验证 encoded_len 包含头部
        let pkt = TnsPacket::data_packet(vec![1, 2, 3, 4, 5]);
        assert_eq!(pkt.encoded_len(), TNS_HEADER_LEN + 5);
    }

    #[tokio::test]
    async fn async_read_write_roundtrip() {
        // 验证异步读写往返（用 duplex 管道）
        use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

        let original = TnsPacket::data_packet(b"hello tns".to_vec());
        let encoded = original.encode();

        let (mut tx, mut rx) = duplex(256);
        tx.write_all(&encoded).await.unwrap();
        tx.flush().await.unwrap();

        // 读取回来
        let mut buf = vec![0u8; encoded.len()];
        rx.read_exact(&mut buf).await.unwrap();
        let (decoded, _) = TnsPacket::decode(&buf).unwrap();
        assert_eq!(decoded, original);
    }

    #[tokio::test]
    async fn async_read_packet_rejects_short_read() {
        // 验证读取流不完整时返回错误（用 duplex 写入不足数据后关闭发送端）
        use tokio::io::{duplex, AsyncWriteExt};

        let (mut tx, mut rx) = duplex(256);
        // 仅 4 字节，不足头部
        tx.write_all(&[0u8; 4]).await.unwrap();
        tx.flush().await.unwrap();
        // 关闭发送端，让接收端收到 EOF，避免 read_exact 永久等待
        drop(tx);

        let result = TnsPacketCodec::read_packet(&mut rx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn async_read_packet_rejects_incomplete_payload() {
        // 验证负载不完整时返回错误
        use tokio::io::{duplex, AsyncWriteExt};

        let mut buf = vec![0u8; TNS_HEADER_LEN];
        // 声明总长度 16（负载 8 字节），但实际不提供
        buf[0..2].copy_from_slice(&16u16.to_be_bytes());
        buf[4] = PacketType::Data.as_u8();

        let (mut tx, mut rx) = duplex(256);
        tx.write_all(&buf).await.unwrap();
        tx.flush().await.unwrap();
        // 关闭发送端，让接收端在读取负载时收到 EOF
        drop(tx);

        let result = TnsPacketCodec::read_packet(&mut rx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn async_write_packet_writes_full_bytes() {
        // 验证 write_packet 写入完整包（用 duplex 读回验证）
        use tokio::io::{duplex, AsyncReadExt};

        let original = TnsPacket::data_packet(b"write test".to_vec());
        let expected = original.encode();

        let (mut tx, mut rx) = duplex(256);
        TnsPacketCodec::write_packet(&mut tx, &original).await.unwrap();

        let mut buf = vec![0u8; expected.len()];
        rx.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, expected.to_vec());
    }
}
