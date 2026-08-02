//! MySQL binlog 解析源 — Batch 6.2
//!
//! 实现 MySQL binlog ROW 格式事件解析，作为 CDC 源端连接器。
//! 支持 WriteRows / UpdateRows / DeleteRows 事件。
//!
//! # 设计
//! - `MySqlBinlogSource` 实现 `SourceConnector` trait
//! - binlog 事件解析：纯 Rust 实现（不依赖外部 C 库）
//! - 连接 MySQL 9.6：`mysql://root:test123@127.0.0.1:3306/sz_orm_test`
//!
//! # binlog 事件格式（ROW 模式）
//! ```text
//! [header: 19 bytes]
//!   timestamp: u32
//!   event_type: u8
//!   server_id: u32
//!   event_length: u32
//!   next_position: u32
//!   flags: u16
//! [body: variable]
//! ```

use crate::decoder::DecodedRow;
use crate::source::{SourceConfig, SourceConnector, SourceError, SourceEvent, SourceOffset};
use crate::schema::TableSchema;
use szrsql_types::value::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// =====================================================================
//  binlog 事件类型常量
// =====================================================================

/// binlog 事件类型（MySQL 5.7+ / 8.0+）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BinlogEventType {
    /// 无知事件
    Unknown = 0,
    /// 查询事件（DDL / BEGIN / COMMIT）
    Query = 2,
    /// 停止事件
    Stop = 3,
    /// 旋转事件（binlog 文件切换）
    Rotate = 4,
    /// 格式描述事件
    FormatDescription = 15,
    /// 行更改事件（写入）
    WriteRows = 23,
    /// 行更改事件（更新）
    UpdateRows = 24,
    /// 行更改事件（删除）
    DeleteRows = 25,
    /// 表映射事件
    TableMap = 19,
    /// GTID 事件
    Gtid = 33,
}

impl BinlogEventType {
    /// 从 u8 解析
    pub fn from_u8(v: u8) -> Self {
        match v {
            2 => Self::Query,
            3 => Self::Stop,
            4 => Self::Rotate,
            15 => Self::FormatDescription,
            19 => Self::TableMap,
            23 => Self::WriteRows,
            24 => Self::UpdateRows,
            25 => Self::DeleteRows,
            33 => Self::Gtid,
            _ => Self::Unknown,
        }
    }

    /// 是否为行变更事件
    pub fn is_row_event(self) -> bool {
        matches!(self, Self::WriteRows | Self::UpdateRows | Self::DeleteRows)
    }
}

// =====================================================================
//  binlog 事件头
// =====================================================================

/// binlog 事件头（19 字节）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinlogEventHeader {
    /// 事件时间戳（秒级 Unix timestamp）
    pub timestamp: u32,
    /// 事件类型
    pub event_type: BinlogEventType,
    /// 服务器 ID
    pub server_id: u32,
    /// 事件总长度（含 header）
    pub event_length: u32,
    /// 下一个事件的文件位置
    pub next_position: u32,
    /// 标志位
    pub flags: u16,
}

impl BinlogEventHeader {
    /// 从字节序列解析事件头
    ///
    /// 需要至少 19 字节
    pub fn decode(data: &[u8]) -> Result<Self, SourceError> {
        if data.len() < 19 {
            return Err(SourceError::Internal(format!(
                "binlog header too short: {} < 19", data.len()
            )));
        }
        let timestamp = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let event_type = BinlogEventType::from_u8(data[4]);
        let server_id = u32::from_le_bytes([data[5], data[6], data[7], data[8]]);
        let event_length = u32::from_le_bytes([data[9], data[10], data[11], data[12]]);
        let next_position = u32::from_le_bytes([data[13], data[14], data[15], data[16]]);
        let flags = u16::from_le_bytes([data[17], data[18]]);
        Ok(Self { timestamp, event_type, server_id, event_length, next_position, flags })
    }
}

// =====================================================================
//  TableMapEvent — 表映射
// =====================================================================

/// 表映射事件 — 记录 table_id 到表名的映射
#[derive(Debug, Clone)]
pub struct TableMapEntry {
    /// 表 ID（binlog 内部编号）
    pub table_id: u64,
    /// 数据库名
    pub database: String,
    /// 表名
    pub table: String,
    /// 列类型编码
    pub column_types: Vec<u8>,
}

// =====================================================================
//  MySqlBinlogSource — MySQL binlog 源端连接器
// =====================================================================

/// MySQL binlog 源端连接器
///
/// 实现 `SourceConnector` trait，通过 MySQL binlog 协议捕获变更。
/// 当前为骨架实现，真实连接需启用 `real-mysql` feature。
pub struct MySqlBinlogSource {
    /// 源端配置
    config: SourceConfig,
    /// 停止标志
    stop_flag: Arc<AtomicBool>,
    /// 当前 binlog 文件名
    binlog_file: String,
    /// 当前 binlog 位置
    binlog_pos: u64,
    /// 表映射缓存
    table_map: std::collections::HashMap<u64, TableMapEntry>,
    /// 服务器 ID（伪造为 slave）
    server_id: u32,
}

impl MySqlBinlogSource {
    /// 创建 MySQL binlog 源端
    pub fn new(config: SourceConfig) -> Result<Self, SourceError> {
        Ok(Self {
            config,
            stop_flag: Arc::new(AtomicBool::new(false)),
            binlog_file: String::new(),
            binlog_pos: 4, // binlog 文件头 magic number 后的起始位置
            table_map: std::collections::HashMap::new(),
            server_id: 65535,
        })
    }

    /// 解析 binlog 事件并转换为 SourceEvent
    pub fn parse_event(&mut self, data: &[u8]) -> Result<Option<SourceEvent>, SourceError> {
        let header = BinlogEventHeader::decode(data)?;
        let body = &data[19..header.event_length.min(data.len() as u32) as usize];

        match header.event_type {
            BinlogEventType::TableMap => {
                self.parse_table_map(body)?;
                Ok(None)
            }
            BinlogEventType::WriteRows => {
                Ok(Some(self.parse_row_event(body, header.timestamp, crate::source::SourceEventOp::Insert)?))
            }
            BinlogEventType::UpdateRows => {
                Ok(Some(self.parse_row_event(body, header.timestamp, crate::source::SourceEventOp::Update)?))
            }
            BinlogEventType::DeleteRows => {
                Ok(Some(self.parse_row_event(body, header.timestamp, crate::source::SourceEventOp::Delete)?))
            }
            _ => Ok(None),
        }
    }

    /// 解析 TableMap 事件
    fn parse_table_map(&mut self, body: &[u8]) -> Result<(), SourceError> {
        // 简化解析：table_id(6) + flags(2) + db_name_len(1) + db_name + \0 + table_name_len(1) + table_name + \0 + ...
        if body.len() < 10 {
            return Err(SourceError::Internal("TableMap body too short".into()));
        }
        let table_id = u64::from_le_bytes([
            body[0], body[1], body[2], body[3], body[4], body[5], 0, 0,
        ]);
        // flags at [6..8]
        let db_len = body[8] as usize;
        if body.len() < 9 + db_len + 1 {
            return Err(SourceError::Internal("TableMap db_name truncated".into()));
        }
        let database = String::from_utf8_lossy(&body[9..9 + db_len]).to_string();
        let table_start = 9 + db_len + 1; // skip \0
        if body.len() < table_start + 1 {
            return Err(SourceError::Internal("TableMap table_name truncated".into()));
        }
        let tbl_len = body[table_start] as usize;
        let table = if body.len() >= table_start + 1 + tbl_len {
            String::from_utf8_lossy(&body[table_start + 1..table_start + 1 + tbl_len]).to_string()
        } else {
            String::new()
        };

        self.table_map.insert(table_id, TableMapEntry {
            table_id,
            database,
            table,
            column_types: Vec::new(),
        });
        Ok(())
    }

    /// 解析行变更事件（简化版）
    fn parse_row_event(
        &self,
        body: &[u8],
        timestamp: u32,
        op: crate::source::SourceEventOp,
    ) -> Result<SourceEvent, SourceError> {
        // 简化：将原始 body 包装为事件
        // 生产环境应根据 TableMap 解析各列
        let row = DecodedRow {
            columns: vec![("_raw".to_string(), Value::Blob(body.to_vec()))],
        };
        let ts = timestamp as u64 * 1000;
        Ok(SourceEvent {
            lsn: self.binlog_pos,
            op,
            schema_name: self.config.schema.clone().unwrap_or_default(),
            table_name: "unknown".to_string(),
            before: if op == crate::source::SourceEventOp::Insert { None } else { Some(row.clone()) },
            after: if op == crate::source::SourceEventOp::Delete { None } else { Some(row) },
            tx_id: None,
            timestamp: ts,
        })
    }

    /// 获取当前 binlog 位置
    pub fn position(&self) -> (String, u64) {
        (self.binlog_file.clone(), self.binlog_pos)
    }
}

impl SourceConnector for MySqlBinlogSource {
    fn source_type(&self) -> &str {
        "mysql"
    }

    fn connect(&self) -> Result<(), SourceError> {
        // 骨架实现：真实连接需启用 real-mysql feature
        #[cfg(feature = "real-mysql")]
        {
            // TODO: 使用 sqlx 连接 MySQL 并执行 COM_BINLOG_DUMP
            return Ok(());
        }
        #[cfg(not(feature = "real-mysql"))]
        {
            Err(SourceError::Unsupported(
                "MySQL binlog requires 'real-mysql' feature".into(),
            ))
        }
    }

    fn disconnect(&self) -> Result<(), SourceError> {
        self.stop_flag.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn discover_schemas(&self, _tables: &[String]) -> Result<Vec<TableSchema>, SourceError> {
        Err(SourceError::Unsupported("MySQL schema discovery not yet implemented".into()))
    }

    fn extract_snapshot(
        &self,
        _table: &str,
        _batch_size: usize,
        _callback: &dyn Fn(&[DecodedRow]) -> Result<(), SourceError>,
    ) -> Result<u64, SourceError> {
        Err(SourceError::Unsupported("MySQL snapshot not yet implemented".into()))
    }

    fn current_lsn(&self) -> Result<u64, SourceError> {
        Ok(self.binlog_pos)
    }

    fn start_cdc_stream(
        &self,
        _start_lsn: u64,
        _callback: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
    ) -> Result<(), SourceError> {
        Err(SourceError::Unsupported(
            "MySQL binlog stream requires 'real-mysql' feature".into(),
        ))
    }

    fn stop_cdc_stream(&self) -> Result<(), SourceError> {
        self.stop_flag.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn ack_offset(&self, _offset: &SourceOffset) -> Result<(), SourceError> {
        Ok(())
    }

    fn confirmed_offset(&self) -> Result<SourceOffset, SourceError> {
        Ok(SourceOffset::new(self.binlog_pos))
    }

    fn health_check(&self) -> Result<(), SourceError> {
        Ok(())
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binlog_event_type_from_u8() {
        assert_eq!(BinlogEventType::from_u8(23), BinlogEventType::WriteRows);
        assert_eq!(BinlogEventType::from_u8(24), BinlogEventType::UpdateRows);
        assert_eq!(BinlogEventType::from_u8(25), BinlogEventType::DeleteRows);
        assert_eq!(BinlogEventType::from_u8(19), BinlogEventType::TableMap);
        assert_eq!(BinlogEventType::from_u8(99), BinlogEventType::Unknown);
    }

    #[test]
    fn binlog_event_type_is_row_event() {
        assert!(BinlogEventType::WriteRows.is_row_event());
        assert!(BinlogEventType::UpdateRows.is_row_event());
        assert!(BinlogEventType::DeleteRows.is_row_event());
        assert!(!BinlogEventType::Query.is_row_event());
        assert!(!BinlogEventType::TableMap.is_row_event());
    }

    #[test]
    fn binlog_header_decode() {
        let mut data = vec![0u8; 19];
        data[0..4].copy_from_slice(&1000u32.to_le_bytes()); // timestamp
        data[4] = 23; // WriteRows
        data[5..9].copy_from_slice(&1u32.to_le_bytes()); // server_id
        data[9..13].copy_from_slice(&100u32.to_le_bytes()); // event_length
        data[13..17].copy_from_slice(&200u32.to_le_bytes()); // next_position
        data[17..19].copy_from_slice(&0u16.to_le_bytes()); // flags

        let header = BinlogEventHeader::decode(&data).unwrap();
        assert_eq!(header.timestamp, 1000);
        assert_eq!(header.event_type, BinlogEventType::WriteRows);
        assert_eq!(header.server_id, 1);
        assert_eq!(header.event_length, 100);
        assert_eq!(header.next_position, 200);
    }

    #[test]
    fn binlog_header_decode_too_short() {
        let data = vec![0u8; 10];
        assert!(BinlogEventHeader::decode(&data).is_err());
    }

    #[test]
    fn mysql_binlog_source_new() {
        let config = SourceConfig::mysql("mysql://root:test123@127.0.0.1:3306/sz_orm_test");
        let source = MySqlBinlogSource::new(config).unwrap();
        assert_eq!(source.source_type(), "mysql");
        assert_eq!(source.binlog_pos, 4);
    }

    #[test]
    fn mysql_binlog_source_connect_without_feature() {
        let config = SourceConfig::mysql("mysql://root:test123@127.0.0.1:3306/sz_orm_test");
        let source = MySqlBinlogSource::new(config).unwrap();
        // 未启用 real-mysql feature 时应返回 Unsupported
        let result = source.connect();
        assert!(result.is_err());
    }

    #[test]
    fn mysql_binlog_parse_table_map() {
        let config = SourceConfig::mysql("mysql://root:test123@127.0.0.1:3306/test");
        let mut source = MySqlBinlogSource::new(config).unwrap();

        // 构造 TableMap body: table_id(6) + flags(2) + db_len(1) + db + \0 + tbl_len(1) + tbl + \0
        let mut body = Vec::new();
        body.extend_from_slice(&[42, 0, 0, 0, 0, 0]); // table_id = 42
        body.extend_from_slice(&[0, 0]); // flags
        body.push(4); // db_len
        body.extend_from_slice(b"test"); // db name
        body.push(0); // null terminator
        body.push(5); // tbl_len
        body.extend_from_slice(b"users"); // table name
        body.push(0); // null terminator

        source.parse_table_map(&body).unwrap();
        assert!(source.table_map.contains_key(&42));
        let entry = &source.table_map[&42];
        assert_eq!(entry.database, "test");
        assert_eq!(entry.table, "users");
    }

    #[test]
    fn mysql_binlog_parse_write_rows_event() {
        let config = SourceConfig::mysql("mysql://root:test123@127.0.0.1:3306/test");
        let mut source = MySqlBinlogSource::new(config).unwrap();

        // 构造完整 binlog 事件
        let body = vec![1, 2, 3, 4, 5]; // 简化 body
        let event_length = (19 + body.len()) as u32;
        let mut data = vec![0u8; 19];
        data[0..4].copy_from_slice(&1000u32.to_le_bytes());
        data[4] = 23; // WriteRows
        data[5..9].copy_from_slice(&1u32.to_le_bytes());
        data[9..13].copy_from_slice(&event_length.to_le_bytes());
        data[13..17].copy_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&body);

        let event = source.parse_event(&data).unwrap().unwrap();
        assert_eq!(event.op, crate::source::SourceEventOp::Insert);
        assert!(event.after.is_some());
        assert!(event.before.is_none());
    }

    #[test]
    fn mysql_binlog_health_check() {
        let config = SourceConfig::mysql("mysql://localhost/db");
        let source = MySqlBinlogSource::new(config).unwrap();
        assert!(source.health_check().is_ok());
    }

    #[test]
    fn mysql_binlog_stop_flag() {
        let config = SourceConfig::mysql("mysql://localhost/db");
        let source = MySqlBinlogSource::new(config).unwrap();
        assert!(!source.stop_flag.load(Ordering::SeqCst));
        source.stop_cdc_stream().unwrap();
        assert!(source.stop_flag.load(Ordering::SeqCst));
    }
}
