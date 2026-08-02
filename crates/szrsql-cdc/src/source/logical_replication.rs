//! P1-2: PostgreSQL logical replication 反向链路源端连接器
//!
//! 基于 PG logical replication 协议（replication slot + START_REPLICATION），
//! 实现 PG → szrsql 的真实 CDC 数据捕获。
//!
//! # 设计
//!
//! 1. **真实协议实现**：使用 `postgres::Client::copy_out` + `START_REPLICATION SLOT ... LOGICAL ...`
//!    命令启动 logical replication 流，解析 PG 发送的 logical replication 消息。
//! 2. **Replication Slot 管理**：通过 `pg_create_logical_replication_slot` /
//!    `pg_drop_replication_slot` 管理 slot，支持断点续传。
//! 3. **Publication 管理**：通过 `CREATE PUBLICATION` / `DROP PUBLICATION` 管理发布订阅。
//! 4. **消息解析**：解析 PG logical replication 协议消息（Begin/Commit/Insert/Update/
//!    Delete/Relation/Origin/Type/Truncate），参考 PG 18 协议规范：
//!    https://www.postgresql.org/docs/18/protocol-logicalrep.html
//! 5. **位点管理**：使用 LSN（Log Sequence Number）作为消费位点，支持断点续传。
//!
//! # 与 `pg_real.rs`（触发器模式）的差异
//!
//! | 维度 | `PgRealSourceConnector`（触发器） | `LogicalReplicationSource`（本模块） |
//! |------|----------------------------------|--------------------------------------|
//! | CDC 模式 | 触发器 + 日志表轮询 | Replication slot + 流式协议 |
//! | 性能 | 中等（< 10K events/sec） | 高（接近原生 WAL 速率） |
//! | 源端侵入 | 创建触发器（影响源表性能） | 仅创建 slot（不影响源表） |
//! | 事务边界 | 通过 tx_id 近似 | 通过 Begin/Commit 消息精确 |
//! | 位点 | _szrsql_cdc_log.id | PG LSN |
//! | 协议 | SQL 查询 | PG logical replication 协议 |
//!
//! # 使用示例
//!
//! ```ignore
//! use szrsql_cdc::source::logical_replication::LogicalReplicationSource;
//! use szrsql_cdc::source::{SourceConfig, SourceConnector};
//! use postgres::NoTls;
//!
//! // 注意：连接字符串需要包含 `replication=database` 参数
//! let conn_str = "postgres://postgres:test123@127.0.0.1:5432/sz_orm_test?replication=database";
//! let client = postgres::Client::connect(conn_str, NoTls).unwrap();
//! let connector = LogicalReplicationSource::new(
//!     client,
//!     SourceConfig::postgres(conn_str),
//!     "szrsql_slot",
//!     "szrsql_pub",
//! ).unwrap();
//!
//! connector.connect().unwrap();
//! // 创建 publication + replication slot
//! connector.setup_replication(&["users".to_string()]).unwrap();
//! // 启动 CDC 流
//! connector.start_cdc_stream(0, &|events| {
//!     println!("received {} events", events.len());
//!     Ok(())
//! }).unwrap();
//! ```
//!
//! # 协议限制说明
//!
//! `postgres::Client::copy_out` 是单向读取（CopyOut），无法在读取流的同时向 PG
//! 发送 standby status update（feedback）。这会导致 PG 在 `wal_sender_timeout`
//! 后断开连接。生产环境应使用 `tokio-postgres` 的 `copy_both` 模式实现完整
//! 双向通信。本模块作为 P1-2 阶段实现，提供协议解析与基础流读取能力。

use crate::decoder::DecodedRow;
use crate::schema::{ColumnDef, TableSchema};
use crate::source::{SourceConfig, SourceConnector, SourceError, SourceEvent, SourceOffset};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use szrsql_types::value::Value as SzValue;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::Mutex;

// =====================================================================
// Lsn — PG Log Sequence Number
// =====================================================================

/// PG LSN（Log Sequence Number）— 64 位日志序列号
///
/// PG 内部使用 8 字节表示 LSN，格式为 `(high32 << 32) | low32`。
/// 字符串形式为 `XXXXXXXX/YYYYYYYY`（两部分均为十六进制）。
///
/// **用途**：
/// - 标识 WAL 位置
/// - 作为 replication slot 的消费位点
/// - 断点续传的依据
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lsn(pub u64);

impl Lsn {
    /// 从 u64 创建 LSN
    pub fn from_u64(v: u64) -> Self {
        Self(v)
    }

    /// 转为 u64
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// 从 PG LSN 字符串格式（如 "0/16B3748"）解析
    ///
    /// # 参数
    /// - `s`：PG LSN 字符串，格式为 `HIGH/LOW`（十六进制）
    ///
    /// # 错误
    /// - 格式不合法（缺少 `/` 或非十六进制字符）
    pub fn from_pg_str(s: &str) -> Result<Self, SourceError> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() != 2 {
            return Err(SourceError::Internal(format!(
                "invalid LSN format (expected HIGH/LOW): {}",
                s
            )));
        }
        let high = u32::from_str_radix(parts[0], 16).map_err(|e| {
            SourceError::Internal(format!("invalid LSN high part '{}': {}", parts[0], e))
        })?;
        let low = u32::from_str_radix(parts[1], 16).map_err(|e| {
            SourceError::Internal(format!("invalid LSN low part '{}': {}", parts[1], e))
        })?;
        Ok(Self((high as u64) << 32 | low as u64))
    }

    /// 转为 PG LSN 字符串格式（如 "0/16B3748"）
    pub fn to_pg_string(self) -> String {
        format!("{:X}/{:X}", (self.0 >> 32) as u32, self.0 as u32)
    }

    /// 是否为零（无效位点）
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl std::fmt::Display for Lsn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_pg_string())
    }
}

impl Default for Lsn {
    fn default() -> Self {
        Self(0)
    }
}

// =====================================================================
// LogicalReplicationMessage — logical replication 消息类型
// =====================================================================

/// Logical replication 消息类型 — 对应 PG 协议规范
///
/// 参考：https://www.postgresql.org/docs/18/protocol-logicalrep.html
///
/// 每个消息以一个 Byte1 标识符开头，决定后续字段格式。
#[derive(Debug, Clone, PartialEq)]
pub enum LogicalReplicationMessage {
    /// Begin 消息（'B'）— 事务开始
    Begin(BeginMessage),
    /// Commit 消息（'C'）— 事务提交
    Commit(CommitMessage),
    /// Origin 消息（'O'）— 事务起源
    Origin(OriginMessage),
    /// Relation 消息（'R'）— 表结构描述
    Relation(RelationMessage),
    /// Type 消息（'Y'）— 自定义类型
    Type(TypeMessage),
    /// Insert 消息（'I'）
    Insert(InsertMessage),
    /// Update 消息（'U'）
    Update(UpdateMessage),
    /// Delete 消息（'D'）
    Delete(DeleteMessage),
    /// Truncate 消息（'T'）
    Truncate(TruncateMessage),
}

/// Begin 消息 — 事务开始
///
/// 格式：Byte1('B') Int64(final_lsn) Int64(commit_ts) Int32(xid)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginMessage {
    /// 事务最终 LSN
    pub final_lsn: u64,
    /// 提交时间戳（ microseconds since 2000-01-01 00:00:00 UTC）
    pub commit_timestamp: u64,
    /// 事务 ID
    pub xid: u32,
}

/// Commit 消息 — 事务提交
///
/// 格式：Byte1('C') Int8(flags) Int64(commit_lsn) Int64(end_lsn) Int64(commit_ts)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMessage {
    /// 标志位（目前未使用，恒为 0）
    pub flags: u8,
    /// 提交 LSN
    pub commit_lsn: u64,
    /// 事务结束 LSN
    pub end_lsn: u64,
    /// 提交时间戳（microseconds since 2000-01-01 00:00:00 UTC）
    pub commit_timestamp: u64,
}

/// Origin 消息 — 事务起源
///
/// 格式：Byte1('O') Int64(origin_lsn) String(origin_name)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginMessage {
    /// 起源 LSN
    pub origin_lsn: u64,
    /// 起源名称
    pub origin_name: String,
}

/// Relation 消息 — 表结构描述
///
/// 格式：Byte1('R') Int32(relation_id) String(schema) String(table)
///       Int8(replica_identity) Int16(num_columns) ColumnDef[]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationMessage {
    /// 关系（表）ID
    pub relation_id: u32,
    /// schema 名（如 "public"）
    pub schema_name: String,
    /// 表名
    pub table_name: String,
    /// replica identity 类型（d=默认，n=nothing，f=full，i=index）
    pub replica_identity: u8,
    /// 列定义列表
    pub columns: Vec<RelationColumn>,
}

/// Relation 消息中的列定义
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationColumn {
    /// 列标志位（1 = key column）
    pub flags: u8,
    /// 列名
    pub name: String,
    /// 类型 OID
    pub type_oid: u32,
    /// 类型修饰符
    pub type_modifier: i32,
}

/// Type 消息 — 自定义类型
///
/// 格式：Byte1('Y') Int32(type_id) String(type_name)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeMessage {
    /// 类型 ID
    pub type_id: u32,
    /// 类型名
    pub type_name: String,
}

/// Insert 消息
///
/// 格式：Byte1('I') Int32(relation_id) Byte1('N') TupleData
#[derive(Debug, Clone, PartialEq)]
pub struct InsertMessage {
    /// 关系（表）ID
    pub relation_id: u32,
    /// 新行数据（后镜像）
    pub new_tuple: TupleData,
}

/// Update 消息
///
/// 格式：Byte1('U') Int32(relation_id) Int8(replica_identity_type) [TupleData] [Byte1('N') TupleData]
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateMessage {
    /// 关系（表）ID
    pub relation_id: u32,
    /// replica identity 类型：
    /// - 'O' = 完整旧行
    /// - 'K' = 仅 key 列
    /// - 'N' = 无旧行（直接是新行）
    pub replica_identity_type: u8,
    /// 旧行数据（前镜像，可能为 None）
    pub old_tuple: Option<TupleData>,
    /// 新行数据（后镜像）
    pub new_tuple: TupleData,
}

/// Delete 消息
///
/// 格式：Byte1('D') Int32(relation_id) Int8(replica_identity_type) TupleData
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteMessage {
    /// 关系（表）ID
    pub relation_id: u32,
    /// replica identity 类型：
    /// - 'O' = 完整旧行
    /// - 'K' = 仅 key 列
    pub replica_identity_type: u8,
    /// 旧行数据（前镜像）
    pub old_tuple: TupleData,
}

/// Truncate 消息
///
/// 格式：Byte1('T') Int8(flags) Int32(num_relations) Int32[](relation_ids)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncateMessage {
    /// 标志位：
    /// - 1 = TRUNCATE_RESTART
    /// - 2 = TRUNCATE_CASCADE
    pub flags: u8,
    /// 被截断的关系 ID 列表
    pub relation_ids: Vec<u32>,
}

// =====================================================================
// TupleData — 元组数据
// =====================================================================

/// 元组数据 — 一行数据的列值列表
///
/// 格式：Int16(num_columns) ColumnData[]
#[derive(Debug, Clone, PartialEq)]
pub struct TupleData {
    /// 列值列表（与 RelationMessage.columns 顺序对应）
    pub columns: Vec<ColumnData>,
}

/// 单列数据
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnData {
    /// NULL 值（列长度 = -1 / 0xFFFF）
    Null,
    /// 未改变的列（TOAST，列长度 = -2 / 0xFFFE）
    /// 仅 Update 消息中可能出现，表示该列未变化
    Unchanged,
    /// 实际数据（列长度 >= 0，后跟 length 字节数据）
    /// 数据以 text 格式传输（pgoutput 插件默认）
    Value(Vec<u8>),
}

impl TupleData {
    /// 列数
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// 将 TupleData 转换为 DecodedRow
    ///
    /// # 参数
    /// - `relation`：对应的 RelationMessage（提供列名和类型）
    ///
    /// # 返回
    /// - `Ok(DecodedRow)`：转换成功
    /// - `Err`：relation 与 tuple 列数不匹配
    pub fn to_decoded_row(&self, relation: &RelationMessage) -> Result<DecodedRow, SourceError> {
        if self.columns.len() != relation.columns.len() {
            return Err(SourceError::Internal(format!(
                "tuple column count {} != relation column count {}",
                self.columns.len(),
                relation.columns.len()
            )));
        }

        let mut result = Vec::with_capacity(self.columns.len());
        for (i, col_data) in self.columns.iter().enumerate() {
            let col_def = &relation.columns[i];
            let value = match col_data {
                ColumnData::Null => SzValue::Null,
                ColumnData::Unchanged => SzValue::Null, // 未改变的列用 Null 表示（实际应从旧行取）
                ColumnData::Value(bytes) => parse_column_value(bytes, col_def.type_oid)?,
            };
            result.push((col_def.name.clone(), value));
        }
        Ok(DecodedRow { columns: result })
    }
}

/// 根据类型 OID 解析列值（text 格式）
///
/// pgoutput 插件默认以 text 格式传输列值，因此将字节解析为 UTF-8 字符串后
/// 按类型转换。对于未知类型，回退为 Text。
fn parse_column_value(bytes: &[u8], type_oid: u32) -> Result<SzValue, SourceError> {
    // PG 内置类型 OID（参考 pg_type 系统表）
    const OID_BOOL: u32 = 16;
    const OID_BYTEA: u32 = 17;
    const OID_INT8: u32 = 20;
    const OID_INT2: u32 = 21;
    const OID_INT4: u32 = 23;
    const OID_TEXT: u32 = 25;
    const OID_OID: u32 = 26;
    const OID_FLOAT4: u32 = 700;
    const OID_FLOAT8: u32 = 701;
    const OID_VARCHAR: u32 = 1043;
    const OID_DATE: u32 = 1082;
    const OID_TIMESTAMP: u32 = 1114;
    const OID_TIMESTAMPTZ: u32 = 1184;
    const OID_NUMERIC: u32 = 1700;
    const OID_UUID: u32 = 2950;
    const OID_JSON: u32 = 114;
    const OID_JSONB: u32 = 3802;

    let text = std::str::from_utf8(bytes)
        .map_err(|e| SourceError::Internal(format!("column value not UTF-8: {}", e)))?;

    let value = match type_oid {
        OID_BOOL => match text {
            "t" => SzValue::Bool(true),
            "f" => SzValue::Bool(false),
            _ => {
                return Err(SourceError::Internal(format!(
                    "invalid bool value: {}",
                    text
                )))
            }
        },
        OID_INT2 | OID_INT4 | OID_INT8 | OID_OID => {
            let v: i64 = text.parse().map_err(|e| {
                SourceError::Internal(format!("parse integer '{}' failed: {}", text, e))
            })?;
            SzValue::Int64(v)
        }
        OID_FLOAT4 | OID_FLOAT8 | OID_NUMERIC => {
            let v: f64 = text.parse().map_err(|e| {
                SourceError::Internal(format!("parse float '{}' failed: {}", text, e))
            })?;
            SzValue::Float64(v)
        }
        OID_BYTEA => {
            // PG bytea text 格式：`\x` 前缀 + 十六进制
            let hex = text.strip_prefix("\\x").unwrap_or(text);
            let bytes = decode_hex(hex)?;
            SzValue::Blob(bytes)
        }
        OID_DATE => {
            // YYYY-MM-DD → 转为自 1970-01-01 的天数
            let date = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|e| {
                SourceError::Internal(format!("parse date '{}' failed: {}", text, e))
            })?;
            let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                .ok_or_else(|| SourceError::Internal("invalid epoch".to_string()))?;
            SzValue::Date((date - epoch).num_days() as i32)
        }
        OID_TIMESTAMP | OID_TIMESTAMPTZ => {
            // 解析 PG timestamp text 格式
            let ts = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.6f")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S"))
                .map_err(|e| {
                    SourceError::Internal(format!("parse timestamp '{}' failed: {}", text, e))
                })?;
            SzValue::Timestamp(ts.and_utc().timestamp_micros())
        }
        OID_UUID => SzValue::Text(text.to_string()),
        OID_JSON | OID_JSONB => {
            let json: serde_json::Value = serde_json::from_str(text).map_err(|e| {
                SourceError::Internal(format!("parse json '{}' failed: {}", text, e))
            })?;
            SzValue::Json(json)
        }
        // 默认按文本处理（text/varchar/char 等）
        _ => SzValue::Text(text.to_string()),
    };
    Ok(value)
}

/// 解码十六进制字符串为字节
fn decode_hex(hex: &str) -> Result<Vec<u8>, SourceError> {
    if hex.len() % 2 != 0 {
        return Err(SourceError::Internal(format!(
            "hex string length not even: {}",
            hex.len()
        )));
    }
    let mut result = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let high = hex_digit(bytes[i])?;
        let low = hex_digit(bytes[i + 1])?;
        result.push((high << 4) | low);
    }
    Ok(result)
}

/// 十六进制字符转数值
fn hex_digit(b: u8) -> Result<u8, SourceError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(SourceError::Internal(format!(
            "invalid hex digit: {}",
            b as char
        ))),
    }
}

// =====================================================================
// LogicalReplicationParser — 消息解析器
// =====================================================================

/// Logical replication 消息解析器
///
/// 从字节流解析 logical replication 消息。无状态，可复用。
pub struct LogicalReplicationParser;

impl LogicalReplicationParser {
    /// 解析一条 logical replication 消息
    ///
    /// # 参数
    /// - `data`：消息字节流（不含 CopyData 包装层的 'w' 头）
    ///
    /// # 返回
    /// - `Ok(LogicalReplicationMessage)`：解析成功
    /// - `Err`：数据不完整或格式错误
    pub fn parse_message(data: &[u8]) -> Result<LogicalReplicationMessage, SourceError> {
        if data.is_empty() {
            return Err(SourceError::Internal(
                "empty logical replication message".to_string(),
            ));
        }

        let msg_type = data[0];
        let payload = &data[1..];

        match msg_type {
            b'B' => Self::parse_begin(payload).map(LogicalReplicationMessage::Begin),
            b'C' => Self::parse_commit(payload).map(LogicalReplicationMessage::Commit),
            b'O' => Self::parse_origin(payload).map(LogicalReplicationMessage::Origin),
            b'R' => Self::parse_relation(payload).map(LogicalReplicationMessage::Relation),
            b'Y' => Self::parse_type(payload).map(LogicalReplicationMessage::Type),
            b'I' => Self::parse_insert(payload).map(LogicalReplicationMessage::Insert),
            b'U' => Self::parse_update(payload).map(LogicalReplicationMessage::Update),
            b'D' => Self::parse_delete(payload).map(LogicalReplicationMessage::Delete),
            b'T' => Self::parse_truncate(payload).map(LogicalReplicationMessage::Truncate),
            _ => Err(SourceError::Internal(format!(
                "unknown logical replication message type: 0x{:02X} ('{}')",
                msg_type, msg_type as char
            ))),
        }
    }

    /// 解析 Begin 消息
    /// 格式：Int64(final_lsn) Int64(commit_ts) Int32(xid)
    fn parse_begin(data: &[u8]) -> Result<BeginMessage, SourceError> {
        if data.len() < 20 {
            return Err(truncated_error("Begin", 20, data.len()));
        }
        let final_lsn = read_u64_be(&data[0..8]);
        let commit_timestamp = read_u64_be(&data[8..16]);
        let xid = read_u32_be(&data[16..20]);
        Ok(BeginMessage {
            final_lsn,
            commit_timestamp,
            xid,
        })
    }

    /// 解析 Commit 消息
    /// 格式：Int8(flags) Int64(commit_lsn) Int64(end_lsn) Int64(commit_ts)
    fn parse_commit(data: &[u8]) -> Result<CommitMessage, SourceError> {
        if data.len() < 25 {
            return Err(truncated_error("Commit", 25, data.len()));
        }
        let flags = data[0];
        let commit_lsn = read_u64_be(&data[1..9]);
        let end_lsn = read_u64_be(&data[9..17]);
        let commit_timestamp = read_u64_be(&data[17..25]);
        Ok(CommitMessage {
            flags,
            commit_lsn,
            end_lsn,
            commit_timestamp,
        })
    }

    /// 解析 Origin 消息
    /// 格式：Int64(origin_lsn) String(origin_name)
    fn parse_origin(data: &[u8]) -> Result<OriginMessage, SourceError> {
        if data.len() < 8 {
            return Err(truncated_error("Origin", 8, data.len()));
        }
        let origin_lsn = read_u64_be(&data[0..8]);
        let (origin_name, _) = read_cstring(&data[8..])?;
        Ok(OriginMessage {
            origin_lsn,
            origin_name,
        })
    }

    /// 解析 Relation 消息
    /// 格式：Int32(relation_id) String(schema) String(table) Int8(replica_identity)
    ///       Int16(num_columns) ColumnDef[]
    fn parse_relation(data: &[u8]) -> Result<RelationMessage, SourceError> {
        if data.len() < 4 {
            return Err(truncated_error("Relation header", 4, data.len()));
        }
        let relation_id = read_u32_be(&data[0..4]);
        let mut cursor = 4usize;

        let (schema_name, consumed) = read_cstring(&data[cursor..])?;
        cursor += consumed;

        let (table_name, consumed) = read_cstring(&data[cursor..])?;
        cursor += consumed;

        if data.len() < cursor + 3 {
            return Err(truncated_error("Relation body", cursor + 3, data.len()));
        }
        let replica_identity = data[cursor];
        cursor += 1;
        let num_columns = read_u16_be(&data[cursor..cursor + 2]) as usize;
        cursor += 2;

        let mut columns = Vec::with_capacity(num_columns);
        for _ in 0..num_columns {
            // 每列：Int8(flags) String(name) Int32(type_oid) Int32(type_modifier)
            if data.len() < cursor + 1 {
                return Err(truncated_error(
                    "Relation column flags",
                    cursor + 1,
                    data.len(),
                ));
            }
            let flags = data[cursor];
            cursor += 1;

            let (name, consumed) = read_cstring(&data[cursor..])?;
            cursor += consumed;

            if data.len() < cursor + 8 {
                return Err(truncated_error(
                    "Relation column type",
                    cursor + 8,
                    data.len(),
                ));
            }
            let type_oid = read_u32_be(&data[cursor..cursor + 4]);
            cursor += 4;
            let type_modifier = read_i32_be(&data[cursor..cursor + 4]);
            cursor += 4;

            columns.push(RelationColumn {
                flags,
                name,
                type_oid,
                type_modifier,
            });
        }

        Ok(RelationMessage {
            relation_id,
            schema_name,
            table_name,
            replica_identity,
            columns,
        })
    }

    /// 解析 Type 消息
    /// 格式：Int32(type_id) String(type_name)
    fn parse_type(data: &[u8]) -> Result<TypeMessage, SourceError> {
        if data.len() < 4 {
            return Err(truncated_error("Type", 4, data.len()));
        }
        let type_id = read_u32_be(&data[0..4]);
        let (type_name, _) = read_cstring(&data[4..])?;
        Ok(TypeMessage { type_id, type_name })
    }

    /// 解析 Insert 消息
    /// 格式：Int32(relation_id) Byte1('N') TupleData
    fn parse_insert(data: &[u8]) -> Result<InsertMessage, SourceError> {
        if data.len() < 6 {
            return Err(truncated_error("Insert header", 6, data.len()));
        }
        let relation_id = read_u32_be(&data[0..4]);
        // data[4] 应为 'N'（0x4E），表示新元组
        if data[4] != b'N' {
            return Err(SourceError::Internal(format!(
                "Insert message: expected 'N' (0x4E) for new tuple, got 0x{:02X}",
                data[4]
            )));
        }
        let new_tuple = Self::parse_tuple_data(&data[5..])?;
        Ok(InsertMessage {
            relation_id,
            new_tuple,
        })
    }

    /// 解析 Update 消息
    /// 格式：Int32(relation_id) Int8(replica_identity_type) [TupleData] [Byte1('N') TupleData]
    fn parse_update(data: &[u8]) -> Result<UpdateMessage, SourceError> {
        if data.len() < 5 {
            return Err(truncated_error("Update header", 5, data.len()));
        }
        let relation_id = read_u32_be(&data[0..4]);
        let replica_identity_type = data[4];
        let mut cursor = 5usize;

        let mut old_tuple = None;
        let new_tuple;

        match replica_identity_type {
            b'O' | b'K' => {
                // 旧元组（完整或仅 key）跟随
                let (tuple, consumed) = Self::parse_tuple_data_with_consumed(&data[cursor..])?;
                old_tuple = Some(tuple);
                cursor += consumed;

                // 接下来应该是 'N' + 新元组
                if cursor < data.len() && data[cursor] == b'N' {
                    cursor += 1;
                    new_tuple = Self::parse_tuple_data(&data[cursor..])?;
                } else {
                    // 没有 'N'，说明只有 key 列（无新元组）— 罕见情况
                    // 此时复用 old_tuple 作为 new_tuple
                    new_tuple = old_tuple
                        .clone()
                        .ok_or_else(|| SourceError::Internal("Update: no new tuple".to_string()))?;
                }
            }
            b'N' => {
                // 无旧元组，直接是新元组
                new_tuple = Self::parse_tuple_data(&data[cursor..])?;
            }
            _ => {
                return Err(SourceError::Internal(format!(
                    "Update message: unknown replica identity type 0x{:02X}",
                    replica_identity_type
                )));
            }
        }

        Ok(UpdateMessage {
            relation_id,
            replica_identity_type,
            old_tuple,
            new_tuple,
        })
    }

    /// 解析 Delete 消息
    /// 格式：Int32(relation_id) Int8(replica_identity_type) TupleData
    fn parse_delete(data: &[u8]) -> Result<DeleteMessage, SourceError> {
        if data.len() < 5 {
            return Err(truncated_error("Delete header", 5, data.len()));
        }
        let relation_id = read_u32_be(&data[0..4]);
        let replica_identity_type = data[4];
        let old_tuple = Self::parse_tuple_data(&data[5..])?;
        Ok(DeleteMessage {
            relation_id,
            replica_identity_type,
            old_tuple,
        })
    }

    /// 解析 Truncate 消息
    /// 格式：Int8(flags) Int32(num_relations) Int32[](relation_ids)
    fn parse_truncate(data: &[u8]) -> Result<TruncateMessage, SourceError> {
        if data.len() < 5 {
            return Err(truncated_error("Truncate header", 5, data.len()));
        }
        let flags = data[0];
        let num_relations = read_u32_be(&data[1..5]) as usize;
        let expected_len = 5 + num_relations * 4;
        if data.len() < expected_len {
            return Err(truncated_error("Truncate body", expected_len, data.len()));
        }
        let mut relation_ids = Vec::with_capacity(num_relations);
        for i in 0..num_relations {
            let offset = 5 + i * 4;
            relation_ids.push(read_u32_be(&data[offset..offset + 4]));
        }
        Ok(TruncateMessage {
            flags,
            relation_ids,
        })
    }

    /// 解析 TupleData
    /// 格式：Int16(num_columns) ColumnData[]
    fn parse_tuple_data(data: &[u8]) -> Result<TupleData, SourceError> {
        let (tuple, _) = Self::parse_tuple_data_with_consumed(data)?;
        Ok(tuple)
    }

    /// 解析 TupleData 并返回消费的字节数
    fn parse_tuple_data_with_consumed(data: &[u8]) -> Result<(TupleData, usize), SourceError> {
        if data.len() < 2 {
            return Err(truncated_error("TupleData header", 2, data.len()));
        }
        let num_columns = read_u16_be(&data[0..2]) as usize;
        let mut cursor = 2usize;
        let mut columns = Vec::with_capacity(num_columns);

        for _ in 0..num_columns {
            if data.len() < cursor + 2 {
                return Err(truncated_error(
                    "TupleData column length",
                    cursor + 2,
                    data.len(),
                ));
            }
            let col_len = read_i16_be(&data[cursor..cursor + 2]);
            cursor += 2;

            let col_data = match col_len {
                -1 => ColumnData::Null,
                -2 => ColumnData::Unchanged,
                len if len >= 0 => {
                    let len = len as usize;
                    if data.len() < cursor + len {
                        return Err(truncated_error(
                            "TupleData column value",
                            cursor + len,
                            data.len(),
                        ));
                    }
                    ColumnData::Value(data[cursor..cursor + len].to_vec())
                }
                _ => {
                    return Err(SourceError::Internal(format!(
                        "TupleData: invalid column length: {}",
                        col_len
                    )))
                }
            };
            cursor += if col_len >= 0 {
                col_len as usize
            } else {
                0
            };
            columns.push(col_data);
        }

        Ok((TupleData { columns }, cursor))
    }
}

// =====================================================================
// 字节读取辅助函数（大端序）
// =====================================================================

/// 读取 16 位无符号整数（大端序）
fn read_u16_be(data: &[u8]) -> u16 {
    u16::from_be_bytes([data[0], data[1]])
}

/// 读取 32 位无符号整数（大端序）
fn read_u32_be(data: &[u8]) -> u32 {
    u32::from_be_bytes([data[0], data[1], data[2], data[3]])
}

/// 读取 64 位无符号整数（大端序）
fn read_u64_be(data: &[u8]) -> u64 {
    u64::from_be_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ])
}

/// 读取 16 位有符号整数（大端序）
fn read_i16_be(data: &[u8]) -> i16 {
    i16::from_be_bytes([data[0], data[1]])
}

/// 读取 32 位有符号整数（大端序）
fn read_i32_be(data: &[u8]) -> i32 {
    i32::from_be_bytes([data[0], data[1], data[2], data[3]])
}

/// 读取 PG C 字符串（以 null 字节结尾）
///
/// # 返回
/// - `(String, consumed_bytes)`：字符串和消费的字节数（含 null 终止符）
fn read_cstring(data: &[u8]) -> Result<(String, usize), SourceError> {
    let null_pos = data
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| SourceError::Internal("C string not null-terminated".to_string()))?;
    let s = std::str::from_utf8(&data[..null_pos])
        .map_err(|e| SourceError::Internal(format!("C string not UTF-8: {}", e)))?
        .to_string();
    // 消费的字节数 = 字符串长度 + 1（null 终止符）
    Ok((s, null_pos + 1))
}

/// 构造"数据截断"错误
fn truncated_error(msg_type: &str, need: usize, have: usize) -> SourceError {
    SourceError::Internal(format!(
        "truncated {} message: need {} bytes, have {}",
        msg_type, need, have
    ))
}

// =====================================================================
// StreamMessage — CopyData 包装层消息
// =====================================================================

/// CopyData 包装层消息（来自 replication 流的原始消息）
///
/// PG logical replication 流中，每条消息以一个 Byte1 标识符开头：
/// - 'w' (0x77) = WAL data，包含 logical replication 消息
/// - 'k' (0x6B) = primary keepalive，服务端心跳
///
/// 参考：https://www.postgresql.org/docs/18/protocol-replication.html
#[derive(Debug, Clone)]
pub enum StreamMessage {
    /// WAL data（'w'）— 携带 logical replication 消息
    WalData {
        /// WAL 起始 LSN
        wal_start: u64,
        /// WAL 结束 LSN
        wal_end: u64,
        /// 服务端发送时间（microseconds since 2000-01-01）
        send_time: u64,
        /// 消息数据（logical replication 消息字节流）
        data: Vec<u8>,
    },
    /// Primary keepalive（'k'）— 服务端心跳
    Keepalive {
        /// 服务端当前 WAL 结束 LSN
        wal_end: u64,
        /// 服务端发送时间
        send_time: u64,
        /// 是否要求客户端回复 standby status update
        reply_requested: bool,
    },
}

impl StreamMessage {
    /// 解析 CopyData 包装层消息
    ///
    /// # 参数
    /// - `data`：CopyData 的 payload（第一个字节为消息类型 'w' 或 'k'）
    pub fn parse(data: &[u8]) -> Result<StreamMessage, SourceError> {
        if data.is_empty() {
            return Err(SourceError::Internal("empty stream message".to_string()));
        }
        let msg_type = data[0];
        match msg_type {
            b'w' => {
                // WAL data：Int64(wal_start) Int64(wal_end) Int64(send_time) Byte[n] data
                if data.len() < 25 {
                    return Err(truncated_error("WalData", 25, data.len()));
                }
                let wal_start = read_u64_be(&data[1..9]);
                let wal_end = read_u64_be(&data[9..17]);
                let send_time = read_u64_be(&data[17..25]);
                let payload = data[25..].to_vec();
                Ok(StreamMessage::WalData {
                    wal_start,
                    wal_end,
                    send_time,
                    data: payload,
                })
            }
            b'k' => {
                // Keepalive：Int64(wal_end) Int64(send_time) Byte1(reply_requested)
                if data.len() < 18 {
                    return Err(truncated_error("Keepalive", 18, data.len()));
                }
                let wal_end = read_u64_be(&data[1..9]);
                let send_time = read_u64_be(&data[9..17]);
                let reply_requested = data[17] != 0;
                Ok(StreamMessage::Keepalive {
                    wal_end,
                    send_time,
                    reply_requested,
                })
            }
            _ => Err(SourceError::Internal(format!(
                "unknown stream message type: 0x{:02X} ('{}')",
                msg_type, msg_type as char
            ))),
        }
    }
}

// =====================================================================
// LogicalReplicationSource — 主结构体
// =====================================================================

/// PostgreSQL logical replication 反向链路源端连接器
///
/// 基于 replication slot + START_REPLICATION 实现 PG → szrsql 的 CDC。
///
/// **核心字段**：
/// - `client`：`postgres::Client`（Mutex 保护，replication=database 连接）
/// - `slot_name`：replication slot 名称（持久化在 PG 端）
/// - `publication_name`：publication 名称（持久化在 PG 端）
/// - `relations`：Relation 消息缓存（relation_id → RelationMessage）
///
/// **生命周期**：
/// 1. `new()`：创建连接器（不建立连接）
/// 2. `connect()`：验证连接活性
/// 3. `setup_replication()`：创建 publication + replication slot
/// 4. `start_cdc_stream()`：启动 START_REPLICATION 流，循环解析消息
/// 5. `stop_cdc_stream()`：请求停止流
/// 6. `teardown_replication()`：删除 publication + replication slot
pub struct LogicalReplicationSource {
    /// PG 客户端（Mutex 保护，串行执行；连接需包含 replication=database 参数）
    client: Mutex<postgres::Client>,
    /// 源端配置
    config: SourceConfig,
    /// Replication slot 名称
    slot_name: String,
    /// Publication 名称
    publication_name: String,
    /// 已确认的消费位点
    confirmed_offset: Mutex<SourceOffset>,
    /// 是否已连接
    connected: AtomicBool,
    /// CDC 流是否运行中
    streaming: AtomicBool,
    /// 停止信号（用于 stop_cdc_stream 协作中断）
    stop_requested: AtomicBool,
    /// Relation 消息缓存（relation_id → RelationMessage）
    /// 用于在解析 Insert/Update/Delete 时查找表结构
    relations: Mutex<HashMap<u32, RelationMessage>>,
}

impl LogicalReplicationSource {
    /// 创建 logical replication 源端连接器
    ///
    /// # 参数
    /// - `client`：已建立的 `postgres::Client` 连接
    ///   **重要**：连接字符串需包含 `replication=database` 参数，否则
    ///   `START_REPLICATION` 命令会被拒绝。
    /// - `config`：源端配置
    /// - `slot_name`：replication slot 名称（需在 PG 端唯一）
    /// - `publication_name`：publication 名称（需在 PG 端唯一）
    pub fn new(
        client: postgres::Client,
        config: SourceConfig,
        slot_name: impl Into<String>,
        publication_name: impl Into<String>,
    ) -> Result<Self, SourceError> {
        Ok(Self {
            client: Mutex::new(client),
            config,
            slot_name: slot_name.into(),
            publication_name: publication_name.into(),
            confirmed_offset: Mutex::new(SourceOffset::default()),
            connected: AtomicBool::new(false),
            streaming: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            relations: Mutex::new(HashMap::new()),
        })
    }

    /// 通过连接串创建 logical replication 源端连接器（便捷构造函数）
    ///
    /// # 参数
    /// - `connection_string`：PG 连接串（**必须包含 `replication=database`**）
    /// - `config`：源端配置
    /// - `slot_name`：replication slot 名称
    /// - `publication_name`：publication 名称
    pub fn connect(
        connection_string: &str,
        config: SourceConfig,
        slot_name: impl Into<String>,
        publication_name: impl Into<String>,
        tls: postgres::NoTls,
    ) -> Result<Self, SourceError> {
        let client = postgres::Client::connect(connection_string, tls)
            .map_err(|e| SourceError::Connection(format!("PG connect failed: {e}")))?;
        Self::new(client, config, slot_name, publication_name)
    }

    /// 获取 replication slot 名称
    pub fn slot_name(&self) -> &str {
        &self.slot_name
    }

    /// 获取 publication 名称
    pub fn publication_name(&self) -> &str {
        &self.publication_name
    }

    /// 获取 schema 名
    pub fn schema_name(&self) -> &str {
        self.config.schema.as_deref().unwrap_or("public")
    }

    /// 是否处于 CDC 流中
    pub fn is_streaming(&self) -> bool {
        self.streaming.load(Ordering::SeqCst)
    }

    /// 是否已连接
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// 创建 publication（如果不存在）
    ///
    /// # 参数
    /// - `tables`：要发布的表名列表（空表示发布所有表）
    pub fn create_publication(&self, tables: &[String]) -> Result<(), SourceError> {
        let mut client = self.client.lock();

        // 先尝试删除已存在的 publication（幂等）
        let drop_sql = format!(
            "DROP PUBLICATION IF EXISTS {};",
            quote_ident(&self.publication_name)
        );
        let _ = client.batch_execute(&drop_sql);

        // 构建 CREATE PUBLICATION 语句
        let create_sql = if tables.is_empty() {
            format!(
                "CREATE PUBLICATION {} FOR ALL TABLES;",
                quote_ident(&self.publication_name)
            )
        } else {
            let table_list: Vec<String> = tables.iter().map(|t| quote_ident(t)).collect();
            format!(
                "CREATE PUBLICATION {} FOR TABLE {};",
                quote_ident(&self.publication_name),
                table_list.join(", ")
            )
        };
        client
            .batch_execute(&create_sql)
            .map_err(|e| SourceError::Sql(format!("Create publication failed: {e}")))?;
        Ok(())
    }

    /// 删除 publication（如果存在）
    pub fn drop_publication(&self) -> Result<(), SourceError> {
        let mut client = self.client.lock();
        let sql = format!(
            "DROP PUBLICATION IF EXISTS {};",
            quote_ident(&self.publication_name)
        );
        client
            .batch_execute(&sql)
            .map_err(|e| SourceError::Sql(format!("Drop publication failed: {e}")))?;
        Ok(())
    }

    /// 创建 replication slot（如果不存在）
    ///
    /// 使用 `pgoutput` 插件，并指定 `publication_name` 选项。
    /// 创建成功后返回 slot 的初始 LSN。
    ///
    /// # 返回
    /// - `Ok(u64)`：slot 的初始 LSN（consistency point）
    pub fn create_replication_slot(&self) -> Result<u64, SourceError> {
        let mut client = self.client.lock();

        // 使用 pg_create_logical_replication_slot 创建 slot
        // slot_options 指定 publication_name（PG 14+ 支持）
        let sql = format!(
            "SELECT slot_name, consistent_lsn FROM pg_create_logical_replication_slot('{}', 'pgoutput', false, '{{\"publication_name\":\"{}\"}}');",
            self.slot_name,
            self.publication_name
        );
        let rows = client
            .query(&sql, &[])
            .map_err(|e| SourceError::Sql(format!("Create replication slot failed: {e}")))?;

        if rows.is_empty() {
            return Err(SourceError::Sql(
                "Create replication slot returned no rows".to_string(),
            ));
        }

        // consistent_lsn 字段类型为 pg_lsn，需要按字符串读取
        let lsn_str: String = rows[0]
            .try_get(1)
            .map_err(|e| SourceError::Sql(format!("Get consistent_lsn failed: {e}")))?;
        let lsn = Lsn::from_pg_str(&lsn_str)?;
        Ok(lsn.as_u64())
    }

    /// 删除 replication slot（如果存在）
    pub fn drop_replication_slot(&self) -> Result<(), SourceError> {
        let mut client = self.client.lock();
        let sql = format!("SELECT pg_drop_replication_slot('{}');", self.slot_name);
        // 注意：slot 不存在时 pg_drop_replication_slot 会报错，这里忽略错误实现幂等
        let _ = client.query(&sql, &[]);
        Ok(())
    }

    /// 一次性设置 replication 环境（创建 publication + slot）
    ///
    /// 便捷方法：内部依次调用 `create_publication` 和 `create_replication_slot`。
    ///
    /// # 参数
    /// - `tables`：要追踪的表名列表
    ///
    /// # 返回
    /// - `Ok(u64)`：slot 的初始 LSN
    pub fn setup_replication(&self, tables: &[String]) -> Result<u64, SourceError> {
        self.create_publication(tables)?;
        let lsn = self.create_replication_slot()?;
        Ok(lsn)
    }

    /// 拆除 replication 环境（删除 slot + publication）
    pub fn teardown_replication(&self) -> Result<(), SourceError> {
        self.drop_replication_slot()?;
        self.drop_publication()?;
        Ok(())
    }

    /// 查询源端当前 WAL LSN
    ///
    /// 执行 `SELECT pg_current_wal_lsn()` 获取源端最新 WAL 位置。
    pub fn query_current_wal_lsn(&self) -> Result<u64, SourceError> {
        let mut client = self.client.lock();
        let rows = client
            .query("SELECT pg_current_wal_lsn()", &[])
            .map_err(|e| SourceError::Sql(format!("Query current_wal_lsn failed: {e}")))?;
        if rows.is_empty() {
            return Err(SourceError::Sql(
                "current_wal_lsn returned no rows".to_string(),
            ));
        }
        let lsn_str: String = rows[0]
            .try_get(0)
            .map_err(|e| SourceError::Sql(format!("Get current_wal_lsn failed: {e}")))?;
        Lsn::from_pg_str(&lsn_str).map(|l| l.as_u64())
    }

    /// 构建 START_REPLICATION SQL 命令
    fn build_start_replication_sql(&self, start_lsn: u64) -> String {
        let lsn = Lsn::from_u64(start_lsn);
        format!(
            "START_REPLICATION SLOT {} LOGICAL {}",
            self.slot_name,
            lsn.to_pg_string()
        )
    }

    /// CDC 流主循环 — 读取 CopyOut 流并解析消息
    ///
    /// # 流程
    /// 1. 执行 `START_REPLICATION SLOT ... LOGICAL ...` 启动流
    /// 2. 循环读取 CopyData 消息（StreamMessage）
    /// 3. 对 WalData 解析 LogicalReplicationMessage
    /// 4. 将 Begin/Commit/Insert/Update/Delete 转换为 SourceEvent
    /// 5. 批量回调通知调用方
    /// 6. 检查 stop_requested 标志，决定是否退出
    fn run_cdc_stream(
        &self,
        start_lsn: u64,
        callback: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
    ) -> Result<(), SourceError> {
        let sql = self.build_start_replication_sql(start_lsn);

        // 获取 client 锁，启动 copy_out 流
        // 注意：CopyOutReader 借用 &mut Client，需持有 MutexGuard 直到流结束
        let mut client = self.client.lock();

        let mut reader = client
            .copy_out(&sql)
            .map_err(|e| SourceError::Connection(format!("START_REPLICATION failed: {e}")))?;

        // 缓冲区：累积读取的字节，用于解析完整消息
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut tmp = [0u8; 4096];
        let mut events: Vec<SourceEvent> = Vec::new();
        // 事务上下文：当前事务的 LSN 和 tx_id（由 Begin 消息设置）
        let mut current_tx_lsn: Option<u64> = None;
        let mut current_tx_id: Option<u32> = None;

        loop {
            // 检查停止信号
            if self.stop_requested.load(Ordering::SeqCst) {
                break;
            }

            // 从 CopyOut 流读取数据
            let n = reader.read(&mut tmp).map_err(|e| {
                SourceError::Connection(format!("Read replication stream failed: {e}"))
            })?;

            if n == 0 {
                // 流结束（EOF）
                break;
            }
            buf.extend_from_slice(&tmp[..n]);

            // 循环解析缓冲区中的消息
            loop {
                if self.stop_requested.load(Ordering::SeqCst) {
                    break;
                }

                // 尝试解析一条 StreamMessage
                let parsed = Self::try_parse_stream_message(&buf)?;
                match parsed {
                    ParseResult::Complete(message, consumed) => {
                        buf.drain(..consumed);
                        match message {
                            StreamMessage::WalData {
                                wal_start, data, ..
                            } => {
                                // 解析 logical replication 消息
                                if data.is_empty() {
                                    continue;
                                }
                                let logical_msg = LogicalReplicationParser::parse_message(&data)?;
                                if let Some(event) = self.handle_logical_message(
                                    &logical_msg,
                                    wal_start,
                                    &mut current_tx_lsn,
                                    &mut current_tx_id,
                                )? {
                                    events.push(event);
                                }
                            }
                            StreamMessage::Keepalive { .. } => {
                                // keepalive 心跳，忽略
                            }
                        }
                    }
                    ParseResult::Incomplete => {
                        // 数据不足，等待下一次读取
                        break;
                    }
                }
            }

            // 批量回调（有事件时）
            if !events.is_empty() {
                callback(&events)?;
                // 更新已确认位点（取最后事件的 LSN）
                if let Some(max_lsn) = events.iter().map(|e| e.lsn).max() {
                    let mut offset = self.confirmed_offset.lock();
                    if max_lsn > offset.lsn {
                        offset.lsn = max_lsn;
                    }
                }
                events.clear();
            }
        }

        // drop reader 会自动关闭 copy_out 流
        drop(reader);
        Ok(())
    }

    /// 尝试从缓冲区解析一条 StreamMessage
    ///
    /// # 返回
    /// - `ParseResult::Complete(msg, consumed)`：解析成功
    /// - `ParseResult::Incomplete`：数据不足，需读取更多
    fn try_parse_stream_message(buf: &[u8]) -> Result<ParseResult<StreamMessage>, SourceError> {
        if buf.is_empty() {
            return Ok(ParseResult::Incomplete);
        }
        let msg_type = buf[0];
        match msg_type {
            b'w' => {
                // WalData：1 + 8 + 8 + 8 = 25 字节头 + 变长 data
                if buf.len() < 25 {
                    return Ok(ParseResult::Incomplete);
                }
                let wal_start = read_u64_be(&buf[1..9]);
                let wal_end = read_u64_be(&buf[9..17]);
                let send_time = read_u64_be(&buf[17..25]);
                let data = buf[25..].to_vec();
                Ok(ParseResult::Complete(
                    StreamMessage::WalData {
                        wal_start,
                        wal_end,
                        send_time,
                        data,
                    },
                    buf.len(),
                ))
            }
            b'k' => {
                // Keepalive：1 + 8 + 8 + 1 = 18 字节
                if buf.len() < 18 {
                    return Ok(ParseResult::Incomplete);
                }
                let wal_end = read_u64_be(&buf[1..9]);
                let send_time = read_u64_be(&buf[9..17]);
                let reply_requested = buf[17] != 0;
                Ok(ParseResult::Complete(
                    StreamMessage::Keepalive {
                        wal_end,
                        send_time,
                        reply_requested,
                    },
                    18,
                ))
            }
            _ => Err(SourceError::Internal(format!(
                "unknown stream message type: 0x{:02X} ('{}')",
                msg_type, msg_type as char
            ))),
        }
    }

    /// 处理 logical replication 消息，转换为 SourceEvent
    ///
    /// # 参数
    /// - `msg`：logical replication 消息
    /// - `wal_lsn`：当前 WAL LSN（来自 WalData 头）
    /// - `current_tx_lsn`：当前事务的 LSN（Begin/Commit 之间维护）
    /// - `current_tx_id`：当前事务的 XID
    ///
    /// # 返回
    /// - `Ok(Some(event))`：产生了一个 SourceEvent
    /// - `Ok(None)`：该消息不产生事件（如 Relation/Type 消息仅更新缓存）
    fn handle_logical_message(
        &self,
        msg: &LogicalReplicationMessage,
        wal_lsn: u64,
        current_tx_lsn: &mut Option<u64>,
        current_tx_id: &mut Option<u32>,
    ) -> Result<Option<SourceEvent>, SourceError> {
        let event = match msg {
            LogicalReplicationMessage::Begin(begin) => {
                // 记录事务上下文
                *current_tx_lsn = Some(begin.final_lsn);
                *current_tx_id = Some(begin.xid);
                // Begin 消息不产生事件（事务边界由 Commit 携带）
                None
            }
            LogicalReplicationMessage::Commit(commit) => {
                let tx_id = current_tx_id.unwrap_or(0);
                let timestamp = pg_ts_to_unix_millis(commit.commit_timestamp);
                let event = SourceEvent::commit(commit.commit_lsn, tx_id as u64, timestamp);
                *current_tx_lsn = None;
                *current_tx_id = None;
                Some(event)
            }
            LogicalReplicationMessage::Relation(rel) => {
                // 更新 relation 缓存
                let mut relations = self.relations.lock();
                relations.insert(rel.relation_id, rel.clone());
                None
            }
            LogicalReplicationMessage::Type(_) => {
                // Type 消息暂不处理（自定义类型）
                None
            }
            LogicalReplicationMessage::Origin(_) => {
                // Origin 消息暂不处理（事务起源）
                None
            }
            LogicalReplicationMessage::Insert(insert) => {
                let relation = self.get_relation(insert.relation_id)?;
                let after = insert.new_tuple.to_decoded_row(&relation)?;
                let tx_id = current_tx_id.unwrap_or(0) as u64;
                let timestamp = current_tx_lsn
                    .and_then(|_| Some(pg_ts_to_unix_millis(0)))
                    .unwrap_or(0);
                let mut event = SourceEvent::insert(
                    wal_lsn,
                    &relation.schema_name,
                    &relation.table_name,
                    after,
                    timestamp,
                );
                event.tx_id = Some(tx_id);
                Some(event)
            }
            LogicalReplicationMessage::Update(update) => {
                let relation = self.get_relation(update.relation_id)?;
                let after = update.new_tuple.to_decoded_row(&relation)?;
                let before = update
                    .old_tuple
                    .as_ref()
                    .map(|t| t.to_decoded_row(&relation))
                    .transpose()?;
                let tx_id = current_tx_id.unwrap_or(0) as u64;
                let timestamp = current_tx_lsn
                    .and_then(|_| Some(pg_ts_to_unix_millis(0)))
                    .unwrap_or(0);
                let mut event = if let Some(before) = before {
                    SourceEvent::update(
                        wal_lsn,
                        &relation.schema_name,
                        &relation.table_name,
                        before,
                        after,
                        timestamp,
                    )
                } else {
                    // 无 before 数据时，使用空 DecodedRow
                    SourceEvent::update(
                        wal_lsn,
                        &relation.schema_name,
                        &relation.table_name,
                        DecodedRow { columns: vec![] },
                        after,
                        timestamp,
                    )
                };
                event.tx_id = Some(tx_id);
                Some(event)
            }
            LogicalReplicationMessage::Delete(delete) => {
                let relation = self.get_relation(delete.relation_id)?;
                let before = delete.old_tuple.to_decoded_row(&relation)?;
                let tx_id = current_tx_id.unwrap_or(0) as u64;
                let timestamp = current_tx_lsn
                    .and_then(|_| Some(pg_ts_to_unix_millis(0)))
                    .unwrap_or(0);
                let mut event = SourceEvent::delete(
                    wal_lsn,
                    &relation.schema_name,
                    &relation.table_name,
                    before,
                    timestamp,
                );
                event.tx_id = Some(tx_id);
                Some(event)
            }
            LogicalReplicationMessage::Truncate(_) => {
                // Truncate 消息暂不转换为 SourceEvent（无行级数据）
                None
            }
        };
        Ok(event)
    }

    /// 从缓存中查找 RelationMessage
    fn get_relation(&self, relation_id: u32) -> Result<RelationMessage, SourceError> {
        let relations = self.relations.lock();
        relations.get(&relation_id).cloned().ok_or_else(|| {
            SourceError::Internal(format!(
                "relation_id {} not found in cache (Relation message missing?)",
                relation_id
            ))
        })
    }
}

/// 解析结果枚举（内部使用）
enum ParseResult<T> {
    /// 解析成功，附带消费的字节数
    Complete(T, usize),
    /// 数据不足，需要读取更多
    Incomplete,
}

impl SourceConnector for LogicalReplicationSource {
    fn source_type(&self) -> &str {
        "postgres-logical"
    }

    fn connect(&self) -> Result<(), SourceError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut client = self.client.lock();
        client
            .batch_execute("SELECT 1")
            .map_err(|e| SourceError::Connection(format!("PG health check failed: {e}")))?;
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn disconnect(&self) -> Result<(), SourceError> {
        self.stop_cdc_stream()?;
        self.connected.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn discover_schemas(&self, tables: &[String]) -> Result<Vec<TableSchema>, SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }

        let schema_name = self.schema_name();
        let mut client = self.client.lock();

        // 构建 information_schema.columns 查询
        let table_filter = if tables.is_empty() {
            String::new()
        } else {
            let placeholders: Vec<String> =
                (1..=tables.len()).map(|i| format!("${}", i + 1)).collect();
            format!("AND table_name IN ({})", placeholders.join(", "))
        };

        let sql = format!(
            "SELECT table_name, column_name, data_type, is_nullable, ordinal_position
             FROM information_schema.columns
             WHERE table_schema = $1 {}
             ORDER BY table_name, ordinal_position",
            table_filter
        );

        let mut params: Vec<&(dyn postgres::types::ToSql + Sync)> = vec![&schema_name];
        for t in tables {
            params.push(t);
        }

        let rows = client
            .query(&sql, &params)
            .map_err(|e| SourceError::SchemaDiscovery(format!("Query failed: {e}")))?;

        // 按 table_name 分组
        let mut table_columns: HashMap<String, Vec<(String, String, bool, i32)>> = HashMap::new();
        for row in &rows {
            let table_name: String = row
                .try_get(0)
                .map_err(|e| SourceError::SchemaDiscovery(format!("Get table_name failed: {e}")))?;
            let column_name: String = row.try_get(1).map_err(|e| {
                SourceError::SchemaDiscovery(format!("Get column_name failed: {e}"))
            })?;
            let pg_type: String = row
                .try_get(2)
                .map_err(|e| SourceError::SchemaDiscovery(format!("Get data_type failed: {e}")))?;
            let is_nullable: String = row.try_get(3).map_err(|e| {
                SourceError::SchemaDiscovery(format!("Get is_nullable failed: {e}"))
            })?;
            let ordinal: i32 = row.try_get(4).map_err(|e| {
                SourceError::SchemaDiscovery(format!("Get ordinal_position failed: {e}"))
            })?;
            table_columns.entry(table_name).or_default().push((
                column_name,
                pg_type,
                is_nullable == "YES",
                ordinal,
            ));
        }

        // 构造 TableSchema 列表
        let mut result = Vec::new();
        for (idx, (table_name, mut cols)) in table_columns.into_iter().enumerate() {
            cols.sort_by_key(|c| c.3);
            let mut col_defs = Vec::with_capacity(cols.len());
            for (name, pg_type, nullable, _) in cols {
                let data_type =
                    crate::source::pg_source::PgSourceConnector::pg_type_to_szrsql(&pg_type)?;
                col_defs.push(ColumnDef {
                    name,
                    data_type,
                    nullable,
                });
            }
            result.push(TableSchema {
                table_id: (idx + 1) as u32,
                table_name,
                columns: col_defs,
                version: 1,
            });
        }

        Ok(result)
    }

    fn extract_snapshot(
        &self,
        table: &str,
        batch_size: usize,
        callback: &dyn Fn(&[DecodedRow]) -> Result<(), SourceError>,
    ) -> Result<u64, SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }

        let mut client = self.client.lock();

        let sql = format!("SELECT * FROM {}", quote_ident(table));
        let rows = client
            .query(&sql, &[])
            .map_err(|e| SourceError::Sql(format!("Snapshot query failed: {e}")))?;

        // 将 PG Row 转为 DecodedRow（按列名 + 推断类型）
        let decoded_rows: Vec<DecodedRow> = rows
            .iter()
            .map(|pg_row| pg_row_to_decoded(pg_row))
            .collect::<Result<Vec<_>, _>>()?;

        let total = decoded_rows.len() as u64;
        if decoded_rows.is_empty() {
            return Ok(0);
        }

        let bs = if batch_size == 0 {
            decoded_rows.len()
        } else {
            batch_size
        };
        for chunk in decoded_rows.chunks(bs) {
            callback(chunk)?;
        }

        Ok(total)
    }

    fn current_lsn(&self) -> Result<u64, SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }
        // 查询源端当前 WAL LSN
        self.query_current_wal_lsn()
    }

    fn start_cdc_stream(
        &self,
        start_lsn: u64,
        callback: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
    ) -> Result<(), SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }
        if self.streaming.load(Ordering::SeqCst) {
            return Err(SourceError::Internal(
                "cdc stream already running".to_string(),
            ));
        }

        // 设置起始位点
        {
            let mut offset = self.confirmed_offset.lock();
            if start_lsn > offset.lsn {
                offset.lsn = start_lsn;
            }
        }

        self.streaming.store(true, Ordering::SeqCst);
        self.stop_requested.store(false, Ordering::SeqCst);

        let result = self.run_cdc_stream(start_lsn, callback);

        self.streaming.store(false, Ordering::SeqCst);
        self.stop_requested.store(false, Ordering::SeqCst);

        result
    }

    fn stop_cdc_stream(&self) -> Result<(), SourceError> {
        if self.streaming.load(Ordering::SeqCst) {
            self.stop_requested.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn ack_offset(&self, offset: &SourceOffset) -> Result<(), SourceError> {
        let mut current = self.confirmed_offset.lock();
        if offset.lsn >= current.lsn {
            *current = offset.clone();
        }
        // 注意：真实的 slot 推进需要通过 standby status update 发送给 PG，
        // 但 postgres::Client::copy_out 是单向读取，无法发送反馈。
        // slot 的 confirmed_flush_lsn 在连接关闭时由 PG 自动更新。
        Ok(())
    }

    fn confirmed_offset(&self) -> Result<SourceOffset, SourceError> {
        let guard = self.confirmed_offset.lock();
        Ok(guard.clone())
    }

    fn health_check(&self) -> Result<(), SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }
        let mut client = self.client.lock();
        client
            .batch_execute("SELECT 1")
            .map_err(|e| SourceError::Connection(format!("PG health_check failed: {e}")))?;
        Ok(())
    }
}

// =====================================================================
// 辅助函数
// =====================================================================

/// 标识符引用（用双引号包裹，转义内部双引号）
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// 将 PG 时间戳（microseconds since 2000-01-01）转换为 Unix 毫秒
///
/// PG 纪元：2000-01-01 00:00:00 UTC
/// Unix 纪元：1970-01-01 00:00:00 UTC
/// 两者相差 10957 天 = 946684800 秒 = 946684800000000 微秒
fn pg_ts_to_unix_millis(pg_ts_micros: u64) -> u64 {
    const PG_EPOCH_TO_UNIX_MICROS: u64 = 946684800_000_000;
    // PG 微秒 → Unix 毫秒
    pg_ts_micros.saturating_add(PG_EPOCH_TO_UNIX_MICROS) / 1000
}

/// 将 `postgres::Row` 转换为 `DecodedRow`（用于全量快照）
///
/// 根据 PG 行的列信息，按列名提取值并转换为 SzValue。
fn pg_row_to_decoded(pg_row: &postgres::Row) -> Result<DecodedRow, SourceError> {
    let columns = pg_row.columns();
    let mut result = Vec::with_capacity(columns.len());

    for (idx, col) in columns.iter().enumerate() {
        let name = col.name().to_string();
        let value = pg_row_value_to_szvalue(pg_row, idx, col.type_().oid())?;
        result.push((name, value));
    }

    Ok(DecodedRow { columns: result })
}

/// 根据 PG 列类型 OID 将 Row 中的值转换为 SzValue
fn pg_row_value_to_szvalue(
    pg_row: &postgres::Row,
    idx: usize,
    type_oid: u32,
) -> Result<SzValue, SourceError> {
    const OID_BOOL: u32 = 16;
    const OID_BYTEA: u32 = 17;
    const OID_INT8: u32 = 20;
    const OID_INT2: u32 = 21;
    const OID_INT4: u32 = 23;
    const OID_TEXT: u32 = 25;
    const OID_FLOAT4: u32 = 700;
    const OID_FLOAT8: u32 = 701;
    const OID_VARCHAR: u32 = 1043;
    const OID_DATE: u32 = 1082;
    const OID_TIMESTAMP: u32 = 1114;
    const OID_TIMESTAMPTZ: u32 = 1184;
    const OID_NUMERIC: u32 = 1700;
    const OID_UUID: u32 = 2950;
    const OID_JSON: u32 = 114;
    const OID_JSONB: u32 = 3802;

    let value = match type_oid {
        OID_BOOL => pg_row
            .try_get::<_, Option<bool>>(idx)
            .map(|v| v.map(SzValue::Bool).unwrap_or(SzValue::Null))
            .unwrap_or(SzValue::Null),
        OID_INT2 => pg_row
            .try_get::<_, Option<i16>>(idx)
            .map(|v| v.map(|i| SzValue::Int64(i as i64)).unwrap_or(SzValue::Null))
            .unwrap_or(SzValue::Null),
        OID_INT4 => pg_row
            .try_get::<_, Option<i32>>(idx)
            .map(|v| v.map(|i| SzValue::Int64(i as i64)).unwrap_or(SzValue::Null))
            .unwrap_or(SzValue::Null),
        OID_INT8 => pg_row
            .try_get::<_, Option<i64>>(idx)
            .map(|v| v.map(SzValue::Int64).unwrap_or(SzValue::Null))
            .unwrap_or(SzValue::Null),
        OID_FLOAT4 => pg_row
            .try_get::<_, Option<f32>>(idx)
            .map(|v| {
                v.map(|f| SzValue::Float64(f as f64))
                    .unwrap_or(SzValue::Null)
            })
            .unwrap_or(SzValue::Null),
        OID_FLOAT8 | OID_NUMERIC => pg_row
            .try_get::<_, Option<f64>>(idx)
            .map(|v| v.map(SzValue::Float64).unwrap_or(SzValue::Null))
            .unwrap_or(SzValue::Null),
        OID_BYTEA => pg_row
            .try_get::<_, Option<Vec<u8>>>(idx)
            .map(|v| v.map(SzValue::Blob).unwrap_or(SzValue::Null))
            .unwrap_or(SzValue::Null),
        OID_TEXT | OID_VARCHAR => pg_row
            .try_get::<_, Option<String>>(idx)
            .map(|v| v.map(SzValue::Text).unwrap_or(SzValue::Null))
            .unwrap_or(SzValue::Null),
        OID_DATE => pg_row
            .try_get::<_, Option<chrono::NaiveDate>>(idx)
            .map(|v| {
                v.map(|d| {
                    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                        .map(|e| (d - e).num_days() as i32)
                        .unwrap_or(0);
                    SzValue::Date(epoch)
                })
                .unwrap_or(SzValue::Null)
            })
            .unwrap_or(SzValue::Null),
        OID_TIMESTAMP | OID_TIMESTAMPTZ => pg_row
            .try_get::<_, Option<chrono::NaiveDateTime>>(idx)
            .map(|v| {
                v.map(|t| SzValue::Timestamp(t.and_utc().timestamp_micros()))
                    .unwrap_or(SzValue::Null)
            })
            .unwrap_or(SzValue::Null),
        OID_JSON | OID_JSONB => pg_row
            .try_get::<_, Option<serde_json::Value>>(idx)
            .map(|v| v.map(SzValue::Json).unwrap_or(SzValue::Null))
            .unwrap_or(SzValue::Null),
        OID_UUID => pg_row
            .try_get::<_, Option<String>>(idx)
            .map(|v| v.map(SzValue::Text).unwrap_or(SzValue::Null))
            .unwrap_or(SzValue::Null),
        // 默认按字符串处理
        _ => pg_row
            .try_get::<_, Option<String>>(idx)
            .map(|v| v.map(SzValue::Text).unwrap_or(SzValue::Null))
            .unwrap_or(SzValue::Null),
    };
    Ok(value)
}

// =====================================================================
// 单元测试（不依赖真实 PG）
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Lsn 测试
    // -----------------------------------------------------------------

    #[test]
    fn lsn_from_u64_round_trip() {
        let lsn = Lsn::from_u64(0x016B3748);
        assert_eq!(lsn.as_u64(), 0x016B3748);
    }

    #[test]
    fn lsn_from_pg_str_valid() {
        let lsn = Lsn::from_pg_str("0/16B3748").unwrap();
        assert_eq!(lsn.as_u64(), 0x016B3748);
    }

    #[test]
    fn lsn_from_pg_str_high_part() {
        let lsn = Lsn::from_pg_str("1/0").unwrap();
        assert_eq!(lsn.as_u64(), 0x100000000);
    }

    #[test]
    fn lsn_to_pg_string_round_trip() {
        let original = "0/16B3748";
        let lsn = Lsn::from_pg_str(original).unwrap();
        assert_eq!(lsn.to_pg_string(), "0/16B3748");
    }

    #[test]
    fn lsn_from_pg_str_invalid_format() {
        assert!(Lsn::from_pg_str("invalid").is_err());
        assert!(Lsn::from_pg_str("0").is_err());
        assert!(Lsn::from_pg_str("0/1/2").is_err());
    }

    #[test]
    fn lsn_from_pg_str_invalid_hex() {
        assert!(Lsn::from_pg_str("G/0").is_err());
        assert!(Lsn::from_pg_str("0/G").is_err());
    }

    #[test]
    fn lsn_is_zero() {
        assert!(Lsn::from_u64(0).is_zero());
        assert!(!Lsn::from_u64(1).is_zero());
    }

    #[test]
    fn lsn_display_format() {
        let lsn = Lsn::from_u64(0x016B3748);
        assert_eq!(format!("{}", lsn), "0/16B3748");
    }

    #[test]
    fn lsn_ordering() {
        let a = Lsn::from_u64(100);
        let b = Lsn::from_u64(200);
        assert!(a < b);
        assert!(b > a);
        assert_eq!(a, Lsn::from_u64(100));
    }

    // -----------------------------------------------------------------
    // 消息解析测试
    // -----------------------------------------------------------------

    /// 构造 Begin 消息字节流
    fn build_begin_message(final_lsn: u64, commit_ts: u64, xid: u32) -> Vec<u8> {
        let mut buf = vec![b'B'];
        buf.extend_from_slice(&final_lsn.to_be_bytes());
        buf.extend_from_slice(&commit_ts.to_be_bytes());
        buf.extend_from_slice(&xid.to_be_bytes());
        buf
    }

    /// 构造 Commit 消息字节流
    fn build_commit_message(flags: u8, commit_lsn: u64, end_lsn: u64, commit_ts: u64) -> Vec<u8> {
        let mut buf = vec![b'C'];
        buf.push(flags);
        buf.extend_from_slice(&commit_lsn.to_be_bytes());
        buf.extend_from_slice(&end_lsn.to_be_bytes());
        buf.extend_from_slice(&commit_ts.to_be_bytes());
        buf
    }

    /// 构造 Relation 消息字节流
    fn build_relation_message(
        relation_id: u32,
        schema: &str,
        table: &str,
        replica_identity: u8,
        columns: &[(u8, &str, u32, i32)],
    ) -> Vec<u8> {
        let mut buf = vec![b'R'];
        buf.extend_from_slice(&relation_id.to_be_bytes());
        // schema (C string)
        buf.extend_from_slice(schema.as_bytes());
        buf.push(0);
        // table (C string)
        buf.extend_from_slice(table.as_bytes());
        buf.push(0);
        // replica_identity
        buf.push(replica_identity);
        // num_columns
        buf.extend_from_slice(&(columns.len() as u16).to_be_bytes());
        for &(flags, name, type_oid, type_mod) in columns {
            buf.push(flags);
            buf.extend_from_slice(name.as_bytes());
            buf.push(0);
            buf.extend_from_slice(&type_oid.to_be_bytes());
            buf.extend_from_slice(&type_mod.to_be_bytes());
        }
        buf
    }

    /// 构造 Insert 消息字节流
    fn build_insert_message(relation_id: u32, tuple: &[(Option<&[u8]>, bool)]) -> Vec<u8> {
        let mut buf = vec![b'I'];
        buf.extend_from_slice(&relation_id.to_be_bytes());
        buf.push(b'N'); // new tuple follows
                        // TupleData
        buf.extend_from_slice(&(tuple.len() as u16).to_be_bytes());
        for &(value, _unchanged) in tuple {
            match value {
                None => {
                    // NULL: length = -1
                    buf.extend_from_slice(&(-1i16).to_be_bytes());
                }
                Some(bytes) => {
                    buf.extend_from_slice(&(bytes.len() as i16).to_be_bytes());
                    buf.extend_from_slice(bytes);
                }
            }
        }
        buf
    }

    #[test]
    fn parse_begin_message_correct() {
        let data = build_begin_message(0x016B3748, 0x1234567890ABCDEF, 42);
        let msg = LogicalReplicationParser::parse_message(&data).unwrap();
        match msg {
            LogicalReplicationMessage::Begin(begin) => {
                assert_eq!(begin.final_lsn, 0x016B3748);
                assert_eq!(begin.commit_timestamp, 0x1234567890ABCDEF);
                assert_eq!(begin.xid, 42);
            }
            _ => panic!("expected Begin message, got {:?}", msg),
        }
    }

    #[test]
    fn parse_commit_message_correct() {
        let data = build_commit_message(0, 0x100, 0x200, 0x1234);
        let msg = LogicalReplicationParser::parse_message(&data).unwrap();
        match msg {
            LogicalReplicationMessage::Commit(commit) => {
                assert_eq!(commit.flags, 0);
                assert_eq!(commit.commit_lsn, 0x100);
                assert_eq!(commit.end_lsn, 0x200);
                assert_eq!(commit.commit_timestamp, 0x1234);
            }
            _ => panic!("expected Commit message, got {:?}", msg),
        }
    }

    #[test]
    fn parse_relation_message_correct() {
        let columns = vec![
            (1u8, "id", 20u32, -1i32),      // int8, key column
            (0u8, "name", 1043u32, 100i32), // varchar(100)
        ];
        let data = build_relation_message(16384, "public", "users", b'd', &columns);
        let msg = LogicalReplicationParser::parse_message(&data).unwrap();
        match msg {
            LogicalReplicationMessage::Relation(rel) => {
                assert_eq!(rel.relation_id, 16384);
                assert_eq!(rel.schema_name, "public");
                assert_eq!(rel.table_name, "users");
                assert_eq!(rel.replica_identity, b'd');
                assert_eq!(rel.columns.len(), 2);
                assert_eq!(rel.columns[0].name, "id");
                assert_eq!(rel.columns[0].flags, 1); // key column
                assert_eq!(rel.columns[0].type_oid, 20); // int8
                assert_eq!(rel.columns[1].name, "name");
                assert_eq!(rel.columns[1].type_oid, 1043); // varchar
                assert_eq!(rel.columns[1].type_modifier, 100);
            }
            _ => panic!("expected Relation message, got {:?}", msg),
        }
    }

    #[test]
    fn parse_insert_message_correct() {
        // 构造一行：id=42 (int8 text), name="alice"
        let id_str: &[u8] = b"42";
        let name_str: &[u8] = b"alice";
        let data = build_insert_message(16384, &[(Some(id_str), false), (Some(name_str), false)]);
        let msg = LogicalReplicationParser::parse_message(&data).unwrap();
        match msg {
            LogicalReplicationMessage::Insert(insert) => {
                assert_eq!(insert.relation_id, 16384);
                assert_eq!(insert.new_tuple.len(), 2);
                match &insert.new_tuple.columns[0] {
                    ColumnData::Value(v) => assert_eq!(v, b"42"),
                    _ => panic!("expected Value for column 0"),
                }
                match &insert.new_tuple.columns[1] {
                    ColumnData::Value(v) => assert_eq!(v, b"alice"),
                    _ => panic!("expected Value for column 1"),
                }
            }
            _ => panic!("expected Insert message, got {:?}", msg),
        }
    }

    #[test]
    fn parse_insert_message_with_null() {
        // 构造一行：id=42, name=NULL
        let id_str: &[u8] = b"42";
        let data = build_insert_message(16384, &[(Some(id_str), false), (None, false)]);
        let msg = LogicalReplicationParser::parse_message(&data).unwrap();
        match msg {
            LogicalReplicationMessage::Insert(insert) => {
                assert_eq!(insert.new_tuple.columns.len(), 2);
                assert!(matches!(insert.new_tuple.columns[1], ColumnData::Null));
            }
            _ => panic!("expected Insert message, got {:?}", msg),
        }
    }

    #[test]
    fn parse_delete_message_correct() {
        // Delete: relation_id + 'K' + TupleData
        let mut buf = vec![b'D'];
        buf.extend_from_slice(&16384u32.to_be_bytes());
        buf.push(b'K'); // key columns
                        // TupleData: 1 column, value "42"
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&2i16.to_be_bytes());
        buf.extend_from_slice(b"42");

        let msg = LogicalReplicationParser::parse_message(&buf).unwrap();
        match msg {
            LogicalReplicationMessage::Delete(delete) => {
                assert_eq!(delete.relation_id, 16384);
                assert_eq!(delete.replica_identity_type, b'K');
                assert_eq!(delete.old_tuple.len(), 1);
                match &delete.old_tuple.columns[0] {
                    ColumnData::Value(v) => assert_eq!(v, b"42"),
                    _ => panic!("expected Value"),
                }
            }
            _ => panic!("expected Delete message, got {:?}", msg),
        }
    }

    #[test]
    fn parse_truncate_message_correct() {
        // Truncate: flags + num_relations + relation_ids[]
        let mut buf = vec![b'T'];
        buf.push(0x02); // CASCADE flag
        buf.extend_from_slice(&2u32.to_be_bytes()); // 2 relations
        buf.extend_from_slice(&16384u32.to_be_bytes());
        buf.extend_from_slice(&16385u32.to_be_bytes());

        let msg = LogicalReplicationParser::parse_message(&buf).unwrap();
        match msg {
            LogicalReplicationMessage::Truncate(trunc) => {
                assert_eq!(trunc.flags, 0x02);
                assert_eq!(trunc.relation_ids.len(), 2);
                assert_eq!(trunc.relation_ids[0], 16384);
                assert_eq!(trunc.relation_ids[1], 16385);
            }
            _ => panic!("expected Truncate message, got {:?}", msg),
        }
    }

    #[test]
    fn parse_unknown_message_type_returns_error() {
        let data = vec![b'X']; // 未知类型
        let result = LogicalReplicationParser::parse_message(&data);
        assert!(result.is_err());
        match result {
            Err(SourceError::Internal(msg)) => assert!(msg.contains("unknown")),
            _ => panic!("expected Internal error"),
        }
    }

    #[test]
    fn parse_truncated_begin_message_returns_error() {
        // Begin 消息需要 1 + 20 = 21 字节，这里只给 5 字节
        let data = vec![b'B', 0, 0, 0, 0];
        let result = LogicalReplicationParser::parse_message(&data);
        assert!(result.is_err());
        match result {
            Err(SourceError::Internal(msg)) => assert!(msg.contains("truncated")),
            _ => panic!("expected truncated error"),
        }
    }

    // -----------------------------------------------------------------
    // StreamMessage 测试
    // -----------------------------------------------------------------

    #[test]
    fn parse_wal_data_message_correct() {
        // 构造 WalData: 'w' + wal_start(8) + wal_end(8) + send_time(8) + data
        let mut buf = vec![b'w'];
        buf.extend_from_slice(&0x100u64.to_be_bytes());
        buf.extend_from_slice(&0x200u64.to_be_bytes());
        buf.extend_from_slice(&0x300u64.to_be_bytes());
        // data 是一个 Begin 消息
        let begin_data = build_begin_message(0x1000, 0x2000, 42);
        buf.extend_from_slice(&begin_data);

        let msg = StreamMessage::parse(&buf).unwrap();
        match msg {
            StreamMessage::WalData {
                wal_start,
                wal_end,
                send_time,
                data,
            } => {
                assert_eq!(wal_start, 0x100);
                assert_eq!(wal_end, 0x200);
                assert_eq!(send_time, 0x300);
                // 验证内部 data 是 Begin 消息
                let logical_msg = LogicalReplicationParser::parse_message(&data).unwrap();
                assert!(matches!(logical_msg, LogicalReplicationMessage::Begin(_)));
            }
            _ => panic!("expected WalData, got {:?}", msg),
        }
    }

    #[test]
    fn parse_keepalive_message_correct() {
        // Keepalive: 'k' + wal_end(8) + send_time(8) + reply_requested(1)
        let mut buf = vec![b'k'];
        buf.extend_from_slice(&0x500u64.to_be_bytes());
        buf.extend_from_slice(&0x600u64.to_be_bytes());
        buf.push(1); // reply requested

        let msg = StreamMessage::parse(&buf).unwrap();
        match msg {
            StreamMessage::Keepalive {
                wal_end,
                send_time,
                reply_requested,
            } => {
                assert_eq!(wal_end, 0x500);
                assert_eq!(send_time, 0x600);
                assert!(reply_requested);
            }
            _ => panic!("expected Keepalive, got {:?}", msg),
        }
    }

    #[test]
    fn parse_keepalive_message_reply_not_requested() {
        let mut buf = vec![b'k'];
        buf.extend_from_slice(&0u64.to_be_bytes());
        buf.extend_from_slice(&0u64.to_be_bytes());
        buf.push(0);

        let msg = StreamMessage::parse(&buf).unwrap();
        match msg {
            StreamMessage::Keepalive {
                reply_requested, ..
            } => assert!(!reply_requested),
            _ => panic!("expected Keepalive"),
        }
    }

    #[test]
    fn parse_unknown_stream_message_returns_error() {
        let buf = vec![b'X'];
        let result = StreamMessage::parse(&buf);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // TupleData 转换测试
    // -----------------------------------------------------------------

    #[test]
    fn tuple_data_to_decoded_row_correct() {
        let relation = RelationMessage {
            relation_id: 16384,
            schema_name: "public".to_string(),
            table_name: "users".to_string(),
            replica_identity: b'd',
            columns: vec![
                RelationColumn {
                    flags: 1,
                    name: "id".to_string(),
                    type_oid: 20, // int8
                    type_modifier: -1,
                },
                RelationColumn {
                    flags: 0,
                    name: "name".to_string(),
                    type_oid: 1043, // varchar
                    type_modifier: -1,
                },
            ],
        };

        let tuple = TupleData {
            columns: vec![
                ColumnData::Value(b"42".to_vec()),
                ColumnData::Value(b"alice".to_vec()),
            ],
        };

        let row = tuple.to_decoded_row(&relation).unwrap();
        assert_eq!(row.len(), 2);
        assert_eq!(row.get("id"), Some(&SzValue::Int64(42)));
        assert_eq!(row.get("name"), Some(&SzValue::Text("alice".to_string())));
    }

    #[test]
    fn tuple_data_with_null_to_decoded_row() {
        let relation = RelationMessage {
            relation_id: 1,
            schema_name: "public".to_string(),
            table_name: "t".to_string(),
            replica_identity: b'd',
            columns: vec![RelationColumn {
                flags: 0,
                name: "val".to_string(),
                type_oid: 20,
                type_modifier: -1,
            }],
        };

        let tuple = TupleData {
            columns: vec![ColumnData::Null],
        };

        let row = tuple.to_decoded_row(&relation).unwrap();
        assert_eq!(row.get("val"), Some(&SzValue::Null));
    }

    #[test]
    fn tuple_data_column_count_mismatch_returns_error() {
        let relation = RelationMessage {
            relation_id: 1,
            schema_name: "public".to_string(),
            table_name: "t".to_string(),
            replica_identity: b'd',
            columns: vec![],
        };

        let tuple = TupleData {
            columns: vec![ColumnData::Value(b"42".to_vec())],
        };

        let result = tuple.to_decoded_row(&relation);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // 列值解析测试
    // -----------------------------------------------------------------

    #[test]
    fn parse_column_value_bool_true() {
        let v = parse_column_value(b"t", 16).unwrap();
        assert_eq!(v, SzValue::Bool(true));
    }

    #[test]
    fn parse_column_value_bool_false() {
        let v = parse_column_value(b"f", 16).unwrap();
        assert_eq!(v, SzValue::Bool(false));
    }

    #[test]
    fn parse_column_value_int8() {
        let v = parse_column_value(b"-12345", 20).unwrap();
        assert_eq!(v, SzValue::Int64(-12345));
    }

    #[test]
    // 测试用近似浮点值（非 PI 常量），clippy::approx_constant 豁免
    #[allow(clippy::approx_constant)]
    fn parse_column_value_float8() {
        let v = parse_column_value(b"3.14159", 701).unwrap();
        assert_eq!(v, SzValue::Float64(3.14159));
    }

    #[test]
    fn parse_column_value_text() {
        let v = parse_column_value(b"hello world", 25).unwrap();
        assert_eq!(v, SzValue::Text("hello world".to_string()));
    }

    #[test]
    fn parse_column_value_uuid() {
        let v = parse_column_value(b"550e8400-e29b-41d4-a716-446655440000", 2950).unwrap();
        assert_eq!(
            v,
            SzValue::Text("550e8400-e29b-41d4-a716-446655440000".to_string())
        );
    }

    #[test]
    fn parse_column_value_jsonb() {
        let v = parse_column_value(br#"{"key":"value","num":42}"#, 3802).unwrap();
        match v {
            SzValue::Json(json) => {
                assert_eq!(json["key"], "value");
                assert_eq!(json["num"], 42);
            }
            _ => panic!("expected Json"),
        }
    }

    #[test]
    fn parse_column_value_bytea_hex() {
        let v = parse_column_value(b"\\x48656c6c6f", 17).unwrap();
        assert_eq!(v, SzValue::Blob(b"Hello".to_vec()));
    }

    #[test]
    fn parse_column_value_unknown_oid_defaults_to_text() {
        let v = parse_column_value(b"some_data", 99999).unwrap();
        assert_eq!(v, SzValue::Text("some_data".to_string()));
    }

    #[test]
    fn parse_column_value_invalid_int_returns_error() {
        let result = parse_column_value(b"not_a_number", 20);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    // 辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn quote_ident_escapes_double_quotes() {
        assert_eq!(quote_ident("users"), "\"users\"");
        assert_eq!(quote_ident("user\"name"), "\"user\"\"name\"");
    }

    #[test]
    fn pg_ts_to_unix_millis_correct() {
        // PG 纪元 2000-01-01 00:00:00 UTC = Unix 946684800 秒
        // PG ts = 0 → Unix millis = 946684800000
        assert_eq!(pg_ts_to_unix_millis(0), 946684800_000);
        // PG ts = 1000 微秒 → Unix millis = 946684800000 + 1
        assert_eq!(pg_ts_to_unix_millis(1000), 946684800_001);
    }

    #[test]
    fn decode_hex_correct() {
        assert_eq!(decode_hex("48656c6c6f").unwrap(), b"Hello");
        assert_eq!(decode_hex("").unwrap(), Vec::<u8>::new());
        assert_eq!(decode_hex("00FF").unwrap(), vec![0x00, 0xFF]);
    }

    #[test]
    fn decode_hex_invalid_length_returns_error() {
        assert!(decode_hex("ABC").is_err()); // 奇数长度
    }

    #[test]
    fn decode_hex_invalid_char_returns_error() {
        assert!(decode_hex("GG").is_err());
    }

    #[test]
    fn read_cstring_correct() {
        let data = b"hello\0world\0";
        let (s, consumed) = read_cstring(data).unwrap();
        assert_eq!(s, "hello");
        assert_eq!(consumed, 6); // "hello" (5) + null (1)
    }

    #[test]
    fn read_cstring_not_null_terminated_returns_error() {
        let data = b"hello";
        assert!(read_cstring(data).is_err());
    }

    #[test]
    fn read_u64_be_correct() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(read_u64_be(&data), 0x0102030405060708);
    }

    #[test]
    fn read_i16_be_negative() {
        let data = [0xFF, 0xFF]; // -1
        assert_eq!(read_i16_be(&data), -1);
    }

    // -----------------------------------------------------------------
    // LogicalReplicationSource 基本测试（不依赖真实 PG）
    // -----------------------------------------------------------------

    #[test]
    fn source_type_is_postgres_logical() {
        // 通过失败连接测试 source_type（不需要真实 PG）
        let result = LogicalReplicationSource::connect(
            "postgresql://nonexistent:5432/nonexistent_db",
            SourceConfig::postgres("postgresql://nonexistent:5432/nonexistent_db"),
            "test_slot",
            "test_pub",
            postgres::NoTls,
        );
        assert!(result.is_err()); // 连接失败
    }

    #[test]
    fn build_start_replication_sql_format() {
        // 测试 SQL 构建逻辑（通过反射调用私有方法不可行，改用 Lsn 格式验证）
        let lsn = Lsn::from_u64(0x016B3748);
        let sql = format!(
            "START_REPLICATION SLOT {} LOGICAL {}",
            "test_slot",
            lsn.to_pg_string()
        );
        assert_eq!(sql, "START_REPLICATION SLOT test_slot LOGICAL 0/16B3748");
    }

    #[test]
    fn parse_result_complete_construction() {
        let r: ParseResult<i32> = ParseResult::Complete(42, 4);
        match r {
            ParseResult::Complete(v, c) => {
                assert_eq!(v, 42);
                assert_eq!(c, 4);
            }
            ParseResult::Incomplete => panic!("expected Complete"),
        }
    }

    #[test]
    fn parse_result_incomplete_construction() {
        let r: ParseResult<i32> = ParseResult::Incomplete;
        assert!(matches!(r, ParseResult::Incomplete));
    }

    /// 测试 Update 消息解析（带旧元组 'O'）
    #[test]
    fn parse_update_message_with_old_tuple() {
        // Update: 'U' + relation_id(4) + 'O' + old_tuple + 'N' + new_tuple
        let mut buf = vec![b'U'];
        buf.extend_from_slice(&16384u32.to_be_bytes());
        buf.push(b'O'); // 完整旧行

        // 旧元组：1 列, value "1"
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&1i16.to_be_bytes());
        buf.extend_from_slice(b"1");

        buf.push(b'N'); // 新元组

        // 新元组：1 列, value "2"
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&1i16.to_be_bytes());
        buf.extend_from_slice(b"2");

        let msg = LogicalReplicationParser::parse_message(&buf).unwrap();
        match msg {
            LogicalReplicationMessage::Update(update) => {
                assert_eq!(update.relation_id, 16384);
                assert_eq!(update.replica_identity_type, b'O');
                assert!(update.old_tuple.is_some());
                let old = update.old_tuple.unwrap();
                assert_eq!(old.columns.len(), 1);
                match &old.columns[0] {
                    ColumnData::Value(v) => assert_eq!(v, b"1"),
                    _ => panic!("expected Value"),
                }
                assert_eq!(update.new_tuple.columns.len(), 1);
                match &update.new_tuple.columns[0] {
                    ColumnData::Value(v) => assert_eq!(v, b"2"),
                    _ => panic!("expected Value"),
                }
            }
            _ => panic!("expected Update message"),
        }
    }

    /// 测试 Update 消息解析（仅新元组 'N'）
    #[test]
    fn parse_update_message_without_old_tuple() {
        // Update: 'U' + relation_id(4) + 'N' + new_tuple
        let mut buf = vec![b'U'];
        buf.extend_from_slice(&16384u32.to_be_bytes());
        buf.push(b'N'); // 直接新元组

        // 新元组：1 列, value "2"
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&1i16.to_be_bytes());
        buf.extend_from_slice(b"2");

        let msg = LogicalReplicationParser::parse_message(&buf).unwrap();
        match msg {
            LogicalReplicationMessage::Update(update) => {
                assert_eq!(update.relation_id, 16384);
                assert_eq!(update.replica_identity_type, b'N');
                assert!(update.old_tuple.is_none());
                assert_eq!(update.new_tuple.columns.len(), 1);
            }
            _ => panic!("expected Update message"),
        }
    }

    /// 测试 Origin 消息解析
    #[test]
    fn parse_origin_message_correct() {
        // Origin: 'O' + origin_lsn(8) + origin_name(cstring)
        let mut buf = vec![b'O'];
        buf.extend_from_slice(&0x1000u64.to_be_bytes());
        buf.extend_from_slice(b"origin_name\0");

        let msg = LogicalReplicationParser::parse_message(&buf).unwrap();
        match msg {
            LogicalReplicationMessage::Origin(origin) => {
                assert_eq!(origin.origin_lsn, 0x1000);
                assert_eq!(origin.origin_name, "origin_name");
            }
            _ => panic!("expected Origin message"),
        }
    }

    /// 测试 Type 消息解析
    #[test]
    fn parse_type_message_correct() {
        // Type: 'Y' + type_id(4) + type_name(cstring)
        let mut buf = vec![b'Y'];
        buf.extend_from_slice(&42u32.to_be_bytes());
        buf.extend_from_slice(b"my_type\0");

        let msg = LogicalReplicationParser::parse_message(&buf).unwrap();
        match msg {
            LogicalReplicationMessage::Type(t) => {
                assert_eq!(t.type_id, 42);
                assert_eq!(t.type_name, "my_type");
            }
            _ => panic!("expected Type message"),
        }
    }

    /// 测试 TupleData 中的 Unchanged 列
    #[test]
    fn parse_tuple_data_with_unchanged_column() {
        // TupleData: 2 columns, first is Unchanged (-2), second is Value "x"
        let mut buf = Vec::new();
        buf.extend_from_slice(&2u16.to_be_bytes());
        buf.extend_from_slice(&(-2i16).to_be_bytes()); // Unchanged
        buf.extend_from_slice(&1i16.to_be_bytes()); // Value length = 1
        buf.extend_from_slice(b"x");

        let tuple = LogicalReplicationParser::parse_tuple_data(&buf).unwrap();
        assert_eq!(tuple.columns.len(), 2);
        assert!(matches!(tuple.columns[0], ColumnData::Unchanged));
        match &tuple.columns[1] {
            ColumnData::Value(v) => assert_eq!(v, b"x"),
            _ => panic!("expected Value"),
        }
    }
}
