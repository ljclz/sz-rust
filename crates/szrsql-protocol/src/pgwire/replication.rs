//! pgwire 流复制协议处理 — Batch 5.1
//!
//! # 设计
//!
//! 实现 PostgreSQL 流复制协议的服务端（walsender）：
//! - 客户端通过 startup 参数 `replication=true` 进入复制模式
//! - 客户端发送 `START_REPLICATION SLOT <slot> PHYSICAL <lsn>` 命令
//! - 服务端进入 CopyBoth 模式，持续推送 WAL 记录
//! - 支持心跳（Primary Keepalive）和备库回复（Standby Status Update）
//!
//! # 协议消息
//!
//! ## 服务端 → 客户端（CopyData 载荷）
//! - `'w'` WalData：WAL 记录数据
//! - `'k'` Primary Keepalive：心跳 + 当前 LSN + 是否要求回复
//!
//! ## 客户端 → 服务端（CopyData 载荷）
//! - `'r'` Standby Status Update：已接收/已刷盘/已回放 LSN
//!
//! # 集成
//!
//! 与 `szrsql-replication::stream::ReplicationPrimary` 配合：
//! - `ReplicationPrimary` 管理 WAL 记录流和备库连接
//! - 本模块负责 pgwire 协议编解码和 TCP 传输
//!
//! 参考文档：<https://www.postgresql.org/docs/current/protocol-replication.html>

use bytes::{Buf, BufMut, BytesMut};
use std::sync::Arc;
use szrsql_replication::stream::{ReplicationMessage, ReplicationPrimary};
use szrsql_tx::wal::WalRecord;
use thiserror::Error;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::trace;

// =====================================================================
//  错误类型
// =====================================================================

/// 复制协议错误
#[derive(Debug, Error)]
pub enum ReplicationProtocolError {
    /// I/O 错误
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// 协议解析错误
    #[error("protocol error: {0}")]
    Protocol(String),
    /// 通道关闭
    #[error("replication channel closed")]
    ChannelClosed,
    /// 无效的 START_REPLICATION 命令
    #[error("invalid START_REPLICATION command: {0}")]
    InvalidCommand(String),
}

// =====================================================================
//  常量
// =====================================================================

/// CopyData 消息类型标识：WAL 数据
const COPY_DATA_WAL: u8 = b'w';

/// CopyData 消息类型标识：Primary Keepalive
const COPY_DATA_KEEPALIVE: u8 = b'k';

/// CopyData 消息类型标识：Standby Status Update
const COPY_DATA_STANDBY_STATUS: u8 = b'r';

/// 默认心跳间隔（毫秒）
const DEFAULT_KEEPALIVE_INTERVAL_MS: u64 = 10_000;

// =====================================================================
//  复制模式检测
// =====================================================================

/// 检测 startup 参数是否请求复制模式。
///
/// PostgreSQL 支持 `replication` 参数值：
/// - `"true"` / `"on"` / `"1"` — 物理复制模式
/// - `"database"` — 逻辑复制模式（当前不支持，降级为物理）
/// - 不存在或 `"false"` — 普通模式
pub fn is_replication_mode(params: &std::collections::HashMap<String, String>) -> bool {
    match params.get("replication").map(|s| s.as_str()) {
        Some("true") | Some("on") | Some("1") | Some("database") => true,
        _ => false,
    }
}

// =====================================================================
//  START_REPLICATION 命令解析
// =====================================================================

/// START_REPLICATION 命令解析结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartReplication {
    /// 复制槽名称（可选）
    pub slot_name: Option<String>,
    /// 起始 LSN（格式：X/Y，如 0/16B3740）
    pub start_lsn: u64,
    /// 是否为物理复制（true）或逻辑复制（false）
    pub physical: bool,
}

/// 解析 `START_REPLICATION` SQL 命令。
///
/// 支持格式：
/// - `START_REPLICATION SLOT slot_name PHYSICAL 0/16B3740`
/// - `START_REPLICATION PHYSICAL 0/16B3740`
/// - `START_REPLICATION 0/16B3740`
pub fn parse_start_replication(sql: &str) -> Result<StartReplication, ReplicationProtocolError> {
    let tokens: Vec<&str> = sql.split_whitespace().collect();
    if tokens.is_empty() || !tokens[0].eq_ignore_ascii_case("START_REPLICATION") {
        return Err(ReplicationProtocolError::InvalidCommand(format!(
            "expected START_REPLICATION, got: {sql}"
        )));
    }

    let mut slot_name = None;
    let mut physical = true;
    let mut lsn_str = None;
    let mut i = 1;

    while i < tokens.len() {
        let token_upper = tokens[i].to_uppercase();
        match token_upper.as_str() {
            "SLOT" => {
                i += 1;
                if i >= tokens.len() {
                    return Err(ReplicationProtocolError::InvalidCommand(
                        "SLOT requires a name".into(),
                    ));
                }
                slot_name = Some(tokens[i].to_string());
            }
            "PHYSICAL" => {
                physical = true;
            }
            "LOGICAL" => {
                physical = false;
            }
            _ => {
                // 尝试解析为 LSN
                if tokens[i].contains('/') {
                    lsn_str = Some(tokens[i]);
                }
            }
        }
        i += 1;
    }

    let start_lsn = match lsn_str {
        Some(s) => parse_lsn(s).map_err(|e| {
            ReplicationProtocolError::InvalidCommand(format!("invalid LSN '{s}': {e}"))
        })?,
        None => 0, // 默认从头开始
    };

    Ok(StartReplication {
        slot_name,
        start_lsn,
        physical,
    })
}

/// 解析 LSN 字符串（格式：`X/Y`，如 `0/16B3740`）为 u64。
fn parse_lsn(s: &str) -> Result<u64, String> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 {
        return Err(format!("expected X/Y format, got '{s}'"));
    }
    let high = u32::from_str_radix(parts[0], 16).map_err(|e| format!("high part: {e}"))?;
    let low = u32::from_str_radix(parts[1], 16).map_err(|e| format!("low part: {e}"))?;
    Ok(((high as u64) << 32) | (low as u64))
}

/// 将 u64 LSN 格式化为 PostgreSQL 标准格式（`X/Y`）。
pub fn format_lsn(lsn: u64) -> String {
    format!("{:X}/{:X}", (lsn >> 32) as u32, lsn as u32)
}

// =====================================================================
//  协议消息编码
// =====================================================================

/// 编码 WalData 消息（CopyData 载荷）。
///
/// 格式：`'w' + Int64 wal_start + Int64 wal_end + Int64 send_time + bytes data`
pub fn encode_wal_data(wal_start: u64, wal_end: u64, data: &[u8]) -> BytesMut {
    let mut buf = BytesMut::with_capacity(1 + 8 + 8 + 8 + data.len());
    buf.put_u8(COPY_DATA_WAL);
    buf.put_i64(wal_start as i64);
    buf.put_i64(wal_end as i64);
    // send_time: PostgreSQL 使用 2000-01-01 起的微秒数，简化为 0
    buf.put_i64(0);
    buf.put_slice(data);
    buf
}

/// 编码 Primary Keepalive 消息（CopyData 载荷）。
///
/// 格式：`'k' + Int64 wal_end + Int64 send_time + Int8 reply_requested`
pub fn encode_keepalive(current_lsn: u64, reply_requested: bool) -> BytesMut {
    let mut buf = BytesMut::with_capacity(1 + 8 + 8 + 1);
    buf.put_u8(COPY_DATA_KEEPALIVE);
    buf.put_i64(current_lsn as i64);
    buf.put_i64(0); // send_time
    buf.put_u8(if reply_requested {
        1
    } else {
        0
    });
    buf
}

/// 解码 Standby Status Update（客户端 → 服务端）。
///
/// 格式：`'r' + Int64 written_lsn + Int64 flushed_lsn + Int64 applied_lsn + Int64 send_time + Int8 reply_requested`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandbyStatusUpdate {
    /// 备库已写入磁盘的 LSN
    pub written_lsn: u64,
    /// 备库已 fsync 的 LSN
    pub flushed_lsn: u64,
    /// 备库已回放的 LSN
    pub applied_lsn: u64,
    /// 是否请求服务端立即回复
    pub reply_requested: bool,
}

/// 解析 Standby Status Update 消息。
pub fn decode_standby_status(data: &[u8]) -> Result<StandbyStatusUpdate, ReplicationProtocolError> {
    if data.is_empty() || data[0] != COPY_DATA_STANDBY_STATUS {
        return Err(ReplicationProtocolError::Protocol(
            "expected Standby Status Update ('r')".into(),
        ));
    }
    if data.len() < 1 + 8 + 8 + 8 + 8 + 1 {
        return Err(ReplicationProtocolError::Protocol(
            "Standby Status Update too short".into(),
        ));
    }
    let mut cursor = &data[1..];
    let written_lsn = cursor.get_i64() as u64;
    let flushed_lsn = cursor.get_i64() as u64;
    let applied_lsn = cursor.get_i64() as u64;
    let _send_time = cursor.get_i64();
    let reply_requested = cursor.get_u8() != 0;

    Ok(StandbyStatusUpdate {
        written_lsn,
        flushed_lsn,
        applied_lsn,
        reply_requested,
    })
}

// =====================================================================
//  WalSender — 主库 WAL 发送器
// =====================================================================

/// WAL 发送器（walsender）— 主库侧流复制服务。
///
/// 负责：
/// 1. 从 `ReplicationPrimary` 接收 WAL 批次
/// 2. 编码为 pgwire CopyData 消息
/// 3. 定期发送心跳
/// 4. 处理备库的 Standby Status Update
pub struct WalSender {
    /// 主库复制管理器
    primary: Arc<ReplicationPrimary>,
    /// 备库 ID
    replica_id: String,
    /// 起始 LSN
    start_lsn: u64,
    /// 心跳间隔（毫秒）
    keepalive_interval_ms: u64,
    /// 备库最后确认的 LSN
    last_confirmed_lsn: u64,
}

impl WalSender {
    /// 创建 WAL 发送器。
    pub fn new(primary: Arc<ReplicationPrimary>, replica_id: &str, start_lsn: u64) -> Self {
        Self {
            primary,
            replica_id: replica_id.to_string(),
            start_lsn,
            keepalive_interval_ms: DEFAULT_KEEPALIVE_INTERVAL_MS,
            last_confirmed_lsn: start_lsn,
        }
    }

    /// 设置心跳间隔。
    pub fn with_keepalive_interval(mut self, ms: u64) -> Self {
        self.keepalive_interval_ms = ms;
        self
    }

    /// 获取备库 ID。
    pub fn replica_id(&self) -> &str {
        &self.replica_id
    }

    /// 获取备库最后确认的 LSN。
    pub fn last_confirmed_lsn(&self) -> u64 {
        self.last_confirmed_lsn
    }

    /// 注册备库到主库并获取接收通道。
    pub fn connect(
        &self,
    ) -> Result<UnboundedReceiver<ReplicationMessage>, ReplicationProtocolError> {
        self.primary
            .accept_replica(&self.replica_id, self.start_lsn)
            .map_err(|e| ReplicationProtocolError::Protocol(e.to_string()))
    }

    /// 处理备库状态更新。
    pub fn handle_standby_status(&mut self, status: &StandbyStatusUpdate) {
        self.last_confirmed_lsn = status.flushed_lsn;
        trace!(
            replica = %self.replica_id,
            flushed_lsn = format_lsn(status.flushed_lsn),
            applied_lsn = format_lsn(status.applied_lsn),
            "standby status updated"
        );
    }

    /// 将 WAL 记录批次编码为 CopyData 消息。
    pub fn encode_wal_batch(records: &[WalRecord], start_lsn: u64, end_lsn: u64) -> BytesMut {
        // 序列化 WAL 记录为二进制（简化：使用 bincode 风格编码）
        let data = encode_wal_records(records);
        encode_wal_data(start_lsn, end_lsn, &data)
    }

    /// 生成心跳消息。
    pub fn encode_heartbeat(&self) -> BytesMut {
        let current_lsn = self.primary.current_lsn();
        // 超过心跳间隔未收到备库回复时请求回复
        let reply_requested = self.last_confirmed_lsn < current_lsn;
        encode_keepalive(current_lsn, reply_requested)
    }
}

/// 将 WAL 记录序列化为二进制。
///
/// 格式（每条记录）：
/// `[u64 lsn][u32 tx_id][u8 op_type][u32 page_id][u32 data_len][data bytes]`
fn encode_wal_records(records: &[WalRecord]) -> Vec<u8> {
    let mut buf = Vec::new();
    for r in records {
        buf.extend_from_slice(&r.lsn.to_be_bytes());
        buf.extend_from_slice(&r.tx_id.to_be_bytes());
        buf.push(r.op_type as u8);
        buf.extend_from_slice(&r.page_id.to_be_bytes());
        buf.extend_from_slice(&(r.data.len() as u32).to_be_bytes());
        buf.extend_from_slice(&r.data);
    }
    buf
}

/// 从二进制反序列化 WAL 记录。
pub fn decode_wal_records(data: &[u8]) -> Vec<WalRecord> {
    let mut records = Vec::new();
    let mut offset = 0;
    while offset + 21 <= data.len() {
        let lsn = u64::from_be_bytes(data[offset..offset + 8].try_into().unwrap());
        offset += 8;
        let tx_id = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let op_type_byte = data[offset];
        offset += 1;
        let page_id = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let data_len = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if offset + data_len > data.len() {
            break;
        }
        let record_data = data[offset..offset + data_len].to_vec();
        offset += data_len;

        let op_type = match op_type_byte {
            0 => szrsql_tx::wal::WalOpType::Insert,
            1 => szrsql_tx::wal::WalOpType::Update,
            2 => szrsql_tx::wal::WalOpType::Delete,
            3 => szrsql_tx::wal::WalOpType::Commit,
            4 => szrsql_tx::wal::WalOpType::Abort,
            5 => szrsql_tx::wal::WalOpType::Checkpoint,
            6 => szrsql_tx::wal::WalOpType::FullPageImage,
            7 => szrsql_tx::wal::WalOpType::TableData,
            _ => szrsql_tx::wal::WalOpType::Insert,
        };

        records.push(WalRecord::new(lsn, tx_id, op_type, page_id, record_data));
    }
    records
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_replication_mode() {
        let mut params = std::collections::HashMap::new();
        assert!(!is_replication_mode(&params));

        params.insert("replication".into(), "true".into());
        assert!(is_replication_mode(&params));

        params.insert("replication".into(), "on".into());
        assert!(is_replication_mode(&params));

        params.insert("replication".into(), "1".into());
        assert!(is_replication_mode(&params));

        params.insert("replication".into(), "database".into());
        assert!(is_replication_mode(&params));

        params.insert("replication".into(), "false".into());
        assert!(!is_replication_mode(&params));
    }

    #[test]
    fn test_parse_start_replication_full() {
        let cmd = "START_REPLICATION SLOT my_slot PHYSICAL 0/16B3740";
        let result = parse_start_replication(cmd).unwrap();
        assert_eq!(result.slot_name, Some("my_slot".into()));
        assert!(result.physical);
        assert_eq!(result.start_lsn, 0x016B3740);
    }

    #[test]
    fn test_parse_start_replication_no_slot() {
        let cmd = "START_REPLICATION PHYSICAL 1/0";
        let result = parse_start_replication(cmd).unwrap();
        assert_eq!(result.slot_name, None);
        assert!(result.physical);
        assert_eq!(result.start_lsn, 0x1_00000000);
    }

    #[test]
    fn test_parse_start_replication_lsn_only() {
        let cmd = "START_REPLICATION 0/0";
        let result = parse_start_replication(cmd).unwrap();
        assert_eq!(result.slot_name, None);
        assert_eq!(result.start_lsn, 0);
    }

    #[test]
    fn test_parse_lsn() {
        assert_eq!(parse_lsn("0/0").unwrap(), 0);
        assert_eq!(parse_lsn("0/16B3740").unwrap(), 0x016B3740);
        assert_eq!(parse_lsn("1/0").unwrap(), 0x1_00000000);
        assert_eq!(
            parse_lsn("FF/FFFFFFFF").unwrap(),
            0xFF * (1u64 << 32) + 0xFFFFFFFF
        );
        assert_eq!(parse_lsn("FFFFFFFF/FFFFFFFF").unwrap(), u64::MAX);
        assert!(parse_lsn("invalid").is_err());
        assert!(parse_lsn("1/2/3").is_err());
    }

    #[test]
    fn test_format_lsn() {
        assert_eq!(format_lsn(0), "0/0");
        assert_eq!(format_lsn(0x016B3740), "0/16B3740");
        assert_eq!(format_lsn(0x1_00000000), "1/0");
    }

    #[test]
    fn test_encode_decode_keepalive() {
        let msg = encode_keepalive(0x1234, true);
        assert_eq!(msg[0], COPY_DATA_KEEPALIVE);
        // 解析
        let mut cursor = &msg[1..];
        let lsn = cursor.get_i64() as u64;
        assert_eq!(lsn, 0x1234);
        let _time = cursor.get_i64();
        let reply = cursor.get_u8();
        assert_eq!(reply, 1);
    }

    #[test]
    fn test_decode_standby_status() {
        let mut buf = Vec::new();
        buf.push(COPY_DATA_STANDBY_STATUS);
        buf.extend_from_slice(&100u64.to_be_bytes()); // written
        buf.extend_from_slice(&90u64.to_be_bytes()); // flushed
        buf.extend_from_slice(&80u64.to_be_bytes()); // applied
        buf.extend_from_slice(&0i64.to_be_bytes()); // time
        buf.push(1); // reply_requested

        let status = decode_standby_status(&buf).unwrap();
        assert_eq!(status.written_lsn, 100);
        assert_eq!(status.flushed_lsn, 90);
        assert_eq!(status.applied_lsn, 80);
        assert!(status.reply_requested);
    }

    #[test]
    fn test_wal_records_roundtrip() {
        use szrsql_tx::wal::WalOpType;

        let records = vec![
            WalRecord::new(1, 10, WalOpType::Insert, 5, vec![1, 2, 3]),
            WalRecord::new(2, 10, WalOpType::Update, 6, vec![4, 5, 6, 7]),
            WalRecord::new(3, 11, WalOpType::Commit, 0, vec![]),
        ];

        let encoded = encode_wal_records(&records);
        let decoded = decode_wal_records(&encoded);

        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].lsn, 1);
        assert_eq!(decoded[0].tx_id, 10);
        assert_eq!(decoded[0].page_id, 5);
        assert_eq!(decoded[0].data, vec![1, 2, 3]);
        assert_eq!(decoded[1].lsn, 2);
        assert_eq!(decoded[1].data, vec![4, 5, 6, 7]);
        assert_eq!(decoded[2].lsn, 3);
        assert_eq!(decoded[2].data, Vec::<u8>::new());
    }

    #[test]
    fn test_wal_sender_creation() {
        let primary = Arc::new(ReplicationPrimary::new("test_primary"));
        let sender = WalSender::new(primary.clone(), "replica_1", 0);
        assert_eq!(sender.replica_id(), "replica_1");
        assert_eq!(sender.last_confirmed_lsn(), 0);

        let rx = sender.connect();
        assert!(rx.is_ok());
    }

    #[test]
    fn test_wal_sender_heartbeat() {
        let primary = Arc::new(ReplicationPrimary::new("test_primary"));
        let sender = WalSender::new(primary.clone(), "replica_1", 0);
        let heartbeat = sender.encode_heartbeat();
        assert_eq!(heartbeat[0], COPY_DATA_KEEPALIVE);
    }

    #[test]
    fn test_encode_wal_batch() {
        use szrsql_tx::wal::WalOpType;
        let records = vec![WalRecord::new(1, 1, WalOpType::Insert, 0, vec![0xAA; 100])];
        let msg = WalSender::encode_wal_batch(&records, 0, 1);
        assert_eq!(msg[0], COPY_DATA_WAL);
        // 验证长度合理
        assert!(msg.len() > 1 + 8 + 8 + 8);
    }
}
