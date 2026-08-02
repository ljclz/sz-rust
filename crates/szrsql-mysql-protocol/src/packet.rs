//! MySQL Wire Protocol 包编解码。
//!
//! MySQL 协议包格式：
//! ```text
//! +------------+------------+----------+
//! | length (3) | seq_id (1) | payload  |
//! +------------+------------+----------+
//! ```
//!
//! - `length`：payload 长度（小端序，3 字节，最大 0xFFFFFF = 16MB-1）
//! - `seq_id`：序列号（每轮命令从 0 递增，握手阶段从 0 开始）
//! - `payload`：实际数据
//!
//! 当 payload >= 0xFFFFFF 时需分多个包发送，最后一个包长度 < 0xFFFFFF。

use bytes::{BufMut, BytesMut};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// MySQL 协议包最大 payload 长度（16MB - 1）。
pub const MAX_PAYLOAD_LEN: usize = 0xFF_FFFF;

/// MySQL 协议包头部长度（3 字节长度 + 1 字节序号）。
pub const HEADER_LEN: usize = 4;

/// MySQL 协议包编解码错误。
#[derive(Debug, Error)]
pub enum PacketError {
    /// IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// 包长度超过最大值
    #[error("payload too large: {0} bytes (max {MAX_PAYLOAD_LEN})")]
    PayloadTooLarge(usize),
    /// 包不完整
    #[error("incomplete packet: expected {expected} bytes, got {got}")]
    Incomplete { expected: usize, got: usize },
}

/// MySQL 协议包。
///
/// 一个完整的 MySQL 协议包，包含序号和 payload。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// 序列号（每个命令周期从 0 递增）
    pub seq_id: u8,
    /// 实际数据
    pub payload: Vec<u8>,
}

impl Packet {
    /// 创建新包。
    pub fn new(seq_id: u8, payload: Vec<u8>) -> Result<Self, PacketError> {
        if payload.len() > MAX_PAYLOAD_LEN {
            return Err(PacketError::PayloadTooLarge(payload.len()));
        }
        Ok(Self { seq_id, payload })
    }

    /// 创建空 payload 包（仅用于 EOF / OK 标记）。
    pub fn empty(seq_id: u8) -> Self {
        Self {
            seq_id,
            payload: Vec::new(),
        }
    }

    /// 编码到 `BytesMut`（含头部）。
    pub fn encode(&self) -> BytesMut {
        let len = self.payload.len();
        let mut buf = BytesMut::with_capacity(HEADER_LEN + len);
        // 3 字节小端长度
        buf.put_u8((len & 0xFF) as u8);
        buf.put_u8(((len >> 8) & 0xFF) as u8);
        buf.put_u8(((len >> 16) & 0xFF) as u8);
        // 1 字节序号
        buf.put_u8(self.seq_id);
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
        let mut len = 0usize;
        len |= buf[0] as usize;
        len |= (buf[1] as usize) << 8;
        len |= (buf[2] as usize) << 16;
        let seq_id = buf[3];
        let total = HEADER_LEN + len;
        if buf.len() < total {
            return Err(PacketError::Incomplete {
                expected: total,
                got: buf.len(),
            });
        }
        let payload = buf[HEADER_LEN..total].to_vec();
        Ok((Self { seq_id, payload }, total))
    }
}

/// MySQL 协议包编解码器（异步流式）。
///
/// 提供对 `AsyncRead + AsyncWrite` 的扩展，可直接读写 MySQL 包。
pub struct PacketCodec;

impl PacketCodec {
    /// 从流中读取一个完整的 MySQL 包。
    ///
    /// 内部处理分片包（payload >= 0xFFFFFF 的情况）。
    pub async fn read_packet<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Packet, PacketError> {
        let mut header = [0u8; HEADER_LEN];
        reader.read_exact(&mut header).await?;
        let mut len = 0usize;
        len |= header[0] as usize;
        len |= (header[1] as usize) << 8;
        len |= (header[2] as usize) << 16;
        let seq_id = header[3];

        let mut payload = vec![0u8; len];
        if len > 0 {
            reader.read_exact(&mut payload).await?;
        }

        // 处理分片包：当 payload 长度恰好为 MAX_PAYLOAD_LEN 时，可能还有后续包
        while len == MAX_PAYLOAD_LEN {
            // 尝试读取下一个头部
            let mut next_header = [0u8; HEADER_LEN];
            match reader.read_exact(&mut next_header).await {
                Ok(_) => {
                    let mut next_len = 0usize;
                    next_len |= next_header[0] as usize;
                    next_len |= (next_header[1] as usize) << 8;
                    next_len |= (next_header[2] as usize) << 16;
                    let mut next_payload = vec![0u8; next_len];
                    if next_len > 0 {
                        reader.read_exact(&mut next_payload).await?;
                    }
                    payload.extend_from_slice(&next_payload);
                    len = next_len;
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // 没有后续包，正常结束
                    break;
                }
                Err(e) => return Err(e.into()),
            }
        }

        Ok(Packet { seq_id, payload })
    }

    /// 向流中写入一个 MySQL 包。
    ///
    /// 自动处理分片：payload >= MAX_PAYLOAD_LEN 时拆分为多个包。
    pub async fn write_packet<W: AsyncWrite + Unpin>(
        writer: &mut W,
        packet: &Packet,
    ) -> Result<(), PacketError> {
        let bytes = packet.encode();
        writer.write_all(&bytes).await?;
        writer.flush().await?;
        Ok(())
    }
}

/// 读取长度编码整数（Length-Encoded Integer）。
///
/// MySQL 协议中整数使用变长编码：
/// - 0x00-0xFB：1 字节，值即该字节
/// - 0xFC：后跟 2 字节小端
/// - 0xFD：后跟 3 字节小端
/// - 0xFE：后跟 8 字节小端
/// - 0xFF：错误标记（不用于整数）
pub fn read_lenenc_int(buf: &mut &[u8]) -> Option<u64> {
    if buf.is_empty() {
        return None;
    }
    let first = buf[0];
    *buf = &buf[1..];
    match first {
        0x00..=0xFB => Some(first as u64),
        0xFC => {
            if buf.len() < 2 {
                return None;
            }
            let val = u16::from_le_bytes([buf[0], buf[1]]) as u64;
            *buf = &buf[2..];
            Some(val)
        }
        0xFD => {
            if buf.len() < 3 {
                return None;
            }
            let val = (buf[0] as u64) | ((buf[1] as u64) << 8) | ((buf[2] as u64) << 16);
            *buf = &buf[3..];
            Some(val)
        }
        0xFE => {
            if buf.len() < 8 {
                return None;
            }
            let val = u64::from_le_bytes([
                buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
            ]);
            *buf = &buf[8..];
            Some(val)
        }
        _ => None, // 0xFF 是错误标记
    }
}

/// 写入长度编码整数。
pub fn write_lenenc_int(buf: &mut Vec<u8>, value: u64) {
    if value < 251 {
        buf.push(value as u8);
    } else if value < 65536 {
        buf.push(0xFC);
        buf.extend_from_slice(&(value as u16).to_le_bytes());
    } else if value < 16777216 {
        buf.push(0xFD);
        buf.push((value & 0xFF) as u8);
        buf.push(((value >> 8) & 0xFF) as u8);
        buf.push(((value >> 16) & 0xFF) as u8);
    } else {
        buf.push(0xFE);
        buf.extend_from_slice(&value.to_le_bytes());
    }
}

/// 读取长度编码字符串（Length-Encoded String）。
pub fn read_lenenc_string(buf: &mut &[u8]) -> Option<Vec<u8>> {
    let len = read_lenenc_int(buf)? as usize;
    if buf.len() < len {
        return None;
    }
    let s = buf[..len].to_vec();
    *buf = &buf[len..];
    Some(s)
}

/// 写入长度编码字符串。
pub fn write_lenenc_string(buf: &mut Vec<u8>, s: &[u8]) {
    write_lenenc_int(buf, s.len() as u64);
    buf.extend_from_slice(s);
}

/// 读取以 NUL（0x00）结尾的字符串。
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

/// 读取剩余所有字节（EOF 字符串）。
pub fn read_eof_string(buf: &mut &[u8]) -> Vec<u8> {
    let s = buf.to_vec();
    *buf = &[];
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_encode_decode_roundtrip() {
        let payload = b"SELECT 1".to_vec();
        let original = Packet::new(0, payload).unwrap();
        let encoded = original.encode();
        let (decoded, consumed) = Packet::decode(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_packet_empty_payload() {
        let p = Packet::empty(5);
        let encoded = p.encode();
        assert_eq!(encoded.len(), HEADER_LEN);
        let (decoded, _) = Packet::decode(&encoded).unwrap();
        assert_eq!(decoded.payload, Vec::<u8>::new());
        assert_eq!(decoded.seq_id, 5);
    }

    #[test]
    fn test_packet_decode_incomplete() {
        let buf = [0u8; 3]; // 仅 3 字节，不够头部
        let result = Packet::decode(&buf);
        assert!(matches!(result, Err(PacketError::Incomplete { .. })));
    }

    #[test]
    fn test_packet_payload_too_large() {
        let huge = vec![0u8; MAX_PAYLOAD_LEN + 1];
        let result = Packet::new(0, huge);
        assert!(matches!(result, Err(PacketError::PayloadTooLarge(_))));
    }

    #[test]
    fn test_lenenc_int_small() {
        let mut buf = Vec::new();
        write_lenenc_int(&mut buf, 100);
        assert_eq!(buf, vec![100]);

        let mut slice = buf.as_slice();
        assert_eq!(read_lenenc_int(&mut slice), Some(100));
    }

    #[test]
    fn test_lenenc_int_two_bytes() {
        let mut buf = Vec::new();
        write_lenenc_int(&mut buf, 1000);
        assert_eq!(buf[0], 0xFC);

        let mut slice = buf.as_slice();
        assert_eq!(read_lenenc_int(&mut slice), Some(1000));
    }

    #[test]
    fn test_lenenc_int_three_bytes() {
        let mut buf = Vec::new();
        write_lenenc_int(&mut buf, 100000);
        assert_eq!(buf[0], 0xFD);

        let mut slice = buf.as_slice();
        assert_eq!(read_lenenc_int(&mut slice), Some(100000));
    }

    #[test]
    fn test_lenenc_int_eight_bytes() {
        let mut buf = Vec::new();
        write_lenenc_int(&mut buf, u64::MAX);
        assert_eq!(buf[0], 0xFE);

        let mut slice = buf.as_slice();
        assert_eq!(read_lenenc_int(&mut slice), Some(u64::MAX));
    }

    #[test]
    fn test_lenenc_string_roundtrip() {
        let mut buf = Vec::new();
        write_lenenc_string(&mut buf, b"hello world");
        let mut slice = buf.as_slice();
        let s = read_lenenc_string(&mut slice).unwrap();
        assert_eq!(s, b"hello world");
    }

    #[test]
    fn test_nul_string_roundtrip() {
        let mut buf = Vec::new();
        write_nul_string(&mut buf, "table_name");
        let mut slice = buf.as_slice();
        let s = read_nul_string(&mut slice).unwrap();
        assert_eq!(s, "table_name");
    }

    #[test]
    fn test_nul_string_with_special_chars() {
        let mut buf = Vec::new();
        write_nul_string(&mut buf, "中文表名");
        let mut slice = buf.as_slice();
        let s = read_nul_string(&mut slice).unwrap();
        assert_eq!(s, "中文表名");
    }

    #[test]
    fn test_eof_string() {
        let data = b"remaining data";
        let mut slice = data.as_slice();
        let s = read_eof_string(&mut slice);
        assert_eq!(s, data);
        assert!(slice.is_empty());
    }

    #[test]
    fn test_packet_seq_id_increments() {
        // 验证序号在握手阶段递增
        let p1 = Packet::new(0, vec![1]).unwrap();
        let p2 = Packet::new(1, vec![2]).unwrap();
        let p3 = Packet::new(2, vec![3]).unwrap();
        assert_eq!(p1.seq_id, 0);
        assert_eq!(p2.seq_id, 1);
        assert_eq!(p3.seq_id, 2);
    }

    #[tokio::test]
    async fn test_packet_codec_read_write_roundtrip() {
        // 使用内存流模拟读写
        use tokio::io::duplex;

        let (mut client, mut server) = duplex(1024);

        // 服务端写入一个包
        let original = Packet::new(0, b"COM_QUERY".to_vec()).unwrap();
        PacketCodec::write_packet(&mut server, &original)
            .await
            .unwrap();

        // 客户端读取
        let received = PacketCodec::read_packet(&mut client).await.unwrap();
        assert_eq!(received, original);
    }

    #[tokio::test]
    async fn test_packet_codec_multiple_packets() {
        use tokio::io::duplex;

        let (mut client, mut server) = duplex(4096);

        // 连续写两个包
        let p1 = Packet::new(0, b"first".to_vec()).unwrap();
        let p2 = Packet::new(1, b"second".to_vec()).unwrap();
        PacketCodec::write_packet(&mut server, &p1).await.unwrap();
        PacketCodec::write_packet(&mut server, &p2).await.unwrap();

        // 客户端依次读取
        let r1 = PacketCodec::read_packet(&mut client).await.unwrap();
        let r2 = PacketCodec::read_packet(&mut client).await.unwrap();
        assert_eq!(r1, p1);
        assert_eq!(r2, p2);
    }
}
