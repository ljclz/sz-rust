//! SzRSQL Debezium JSON 适配器 — 对应 `SzRSQL实施进度.md` Phase 2.5.4。
//!
//! 将 SzRSQL 内部的 `ChangeEvent` 转换为 Debezium Connect 官方 JSON 格式，
//! 并支持反向解析。
//!
//! # Debezium JSON 格式
//!
//! ```json
//! {
//!   "schema": { ... },  // Schema 描述（可选，简化版本只包含 type="struct" 和字段名）
//!   "payload": {
//!     "before": { ... } | null,   // 前镜像（Insert 时为 null）
//!     "after": { ... } | null,    // 后镜像（Delete 时为 null）
//!     "source": {
//!       "version": "0.1.0",
//!       "connector": "szrsql",
//!       "name": "szrsql",
//!       "db": "szrsql",
//!       "table_id": 42,
//!       "ts_ms": 1234567890,
//!       "lsn": 100,
//!       "tx_id": 1
//!     },
//!     "op": "c|u|d|r",            // c=create, u=update, d=delete, r=snapshot read
//!     "ts_ms": 1234567890,        // 变更时间戳（毫秒）
//!     "transaction": {            // 事务信息（可选，仅 Commit/Abort 事件包含）
//!       "id": "tx-1",
//!       "status": "BEGIN|END|ABORT"
//!     }
//!   }
//! }
//! ```
//!
//! # 设计要点
//!
//! 1. **op 映射**：
//!    - SzRSQL `Insert` → Debezium `c` (create)
//!    - SzRSQL `Update` → Debezium `u` (update)
//!    - SzRSQL `Delete` → Debezium `d` (delete)
//!    - SzRSQL `Commit` → Debezium 不存在，映射为 op=`r` (snapshot read) + transaction.status=END
//!    - SzRSQL `Abort` → Debezium 不存在，映射为 op=`d` + transaction.status=ABORT
//!
//! 2. **简化 Schema**：完整 Debezium schema 包含详细字段类型（connect.name、field 等），
//!    本实现使用简化 schema（仅包含 type="struct" 和可选字段列表），便于测试和验证。
//!
//! 3. **数据格式**：before/after 中的行数据使用 JSON 对象表示，
//!    key 为列名（简化为 "col_0", "col_1", ...），value 为列值（base64 编码的 Vec<u8>）。
//!
//! 4. **source 字段**：包含 SzRSQL 特有信息（version、connector、name、db、table_id、ts_ms、lsn、tx_id）

use crate::{CdcEventOp, ChangeEvent};
use serde::{Deserialize, Serialize};

// =====================================================================
// Debezium 操作类型
// =====================================================================

/// Debezium 操作类型字符串
///
/// - `c` = create (Insert)
/// - `u` = update (Update)
/// - `d` = delete (Delete)
/// - `r` = snapshot read (用于 Commit 事件，简化映射)
pub const OP_CREATE: &str = "c";
pub const OP_UPDATE: &str = "u";
pub const OP_DELETE: &str = "d";
pub const OP_READ: &str = "r";

/// Debezium 事务状态
pub const TX_STATUS_BEGIN: &str = "BEGIN";
pub const TX_STATUS_END: &str = "END";
pub const TX_STATUS_ABORT: &str = "ABORT";

/// 将 CdcEventOp 转为 Debezium op 字符串
pub fn op_to_debezium(op: CdcEventOp) -> &'static str {
    match op {
        CdcEventOp::Insert => OP_CREATE,
        CdcEventOp::Update => OP_UPDATE,
        CdcEventOp::Delete => OP_DELETE,
        // Commit/Abort 在 Debezium 中没有直接对应，用 r (read) 表示事务结束事件
        CdcEventOp::Commit | CdcEventOp::Abort => OP_READ,
    }
}

/// 将 Debezium op 字符串转为 CdcEventOp
///
/// 注：Commit/Abort 无法从 Debezium op 还原（因为它们都被映射为 `r`），
/// 实际还原时需结合 transaction.status 字段判断。
/// 此函数仅根据 op 字段返回最接近的 CdcEventOp：
/// - `c` → Insert
/// - `u` → Update
/// - `d` → Delete
/// - `r` → Commit（默认）
pub fn op_from_debezium(op: &str) -> Option<CdcEventOp> {
    match op {
        "c" => Some(CdcEventOp::Insert),
        "u" => Some(CdcEventOp::Update),
        "d" => Some(CdcEventOp::Delete),
        "r" => Some(CdcEventOp::Commit), // 默认映射 r → Commit
        _ => None,
    }
}

// =====================================================================
// Debezium JSON 结构定义
// =====================================================================

/// Debezium source 元数据 — 描述变更源
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebeziumSource {
    /// SzRSQL 版本号
    pub version: String,
    /// 连接器名称（固定为 "szrsql"）
    pub connector: String,
    /// 逻辑名称
    pub name: String,
    /// 数据库名称
    pub db: String,
    /// 表 ID（SzRSQL 内部）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_id: Option<u32>,
    /// 变更时间戳（毫秒）
    pub ts_ms: u64,
    /// WAL 日志序列号
    pub lsn: u64,
    /// 事务 ID
    pub tx_id: u32,
}

impl Default for DebeziumSource {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            connector: "szrsql".to_string(),
            name: "szrsql".to_string(),
            db: "szrsql".to_string(),
            table_id: None,
            ts_ms: 0,
            lsn: 0,
            tx_id: 0,
        }
    }
}

/// Debezium 事务信息（仅 Commit/Abort 事件包含）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebeziumTransaction {
    /// 事务 ID（字符串形式）
    pub id: String,
    /// 事务状态：BEGIN | END | ABORT
    pub status: String,
}

/// Debezium payload — 变更数据负载
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebeziumPayload {
    /// 前镜像（Insert 时为 null；Commit/Abort 时为 null）
    ///
    /// **注**：不使用 `skip_serializing_if`，因为 Debezium 官方格式要求 `before` 字段
    /// 始终存在（null 或对象），便于下游消费者统一处理
    #[serde(default)]
    pub before: Option<serde_json::Value>,
    /// 后镜像（Delete 时为 null；Commit/Abort 时为 null）
    ///
    /// **注**：不使用 `skip_serializing_if`，原因同 `before`
    #[serde(default)]
    pub after: Option<serde_json::Value>,
    /// 变更源元数据
    pub source: DebeziumSource,
    /// 操作类型：c/u/d/r
    pub op: String,
    /// 变更时间戳（毫秒）
    pub ts_ms: u64,
    /// 事务信息（仅 Commit/Abort 事件包含）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction: Option<DebeziumTransaction>,
}

/// Debezium schema 字段描述（简化版）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebeziumSchemaField {
    /// 字段类型（如 "int32", "int64", "string", "bytes", "boolean"）
    #[serde(rename = "type")]
    pub field_type: String,
    /// 是否可选
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
    /// 字段名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

/// Debezium schema — 描述 payload 的结构
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebeziumSchema {
    /// schema 类型（固定 "struct"）
    #[serde(rename = "type")]
    pub schema_type: String,
    /// schema 名称（如 "io.szrsql.cdc.ChangeEvent"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 是否可选
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
    /// 字段列表
    pub fields: Vec<DebeziumSchemaField>,
}

impl Default for DebeziumSchema {
    fn default() -> Self {
        Self {
            schema_type: "struct".to_string(),
            name: Some("io.szrsql.cdc.ChangeEvent".to_string()),
            optional: Some(false),
            fields: vec![
                DebeziumSchemaField {
                    field_type: "struct".to_string(),
                    optional: Some(true),
                    field: Some("before".to_string()),
                },
                DebeziumSchemaField {
                    field_type: "struct".to_string(),
                    optional: Some(true),
                    field: Some("after".to_string()),
                },
                DebeziumSchemaField {
                    field_type: "struct".to_string(),
                    optional: Some(false),
                    field: Some("source".to_string()),
                },
                DebeziumSchemaField {
                    field_type: "string".to_string(),
                    optional: Some(false),
                    field: Some("op".to_string()),
                },
                DebeziumSchemaField {
                    field_type: "int64".to_string(),
                    optional: Some(false),
                    field: Some("ts_ms".to_string()),
                },
                DebeziumSchemaField {
                    field_type: "struct".to_string(),
                    optional: Some(true),
                    field: Some("transaction".to_string()),
                },
            ],
        }
    }
}

/// Debezium 事件 — 完整的 Debezium JSON 消息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebeziumEvent {
    /// schema 描述
    pub schema: DebeziumSchema,
    /// payload 负载
    pub payload: DebeziumPayload,
}

// =====================================================================
// 行数据编码（Vec<u8> ↔ serde_json::Value）
// =====================================================================

/// 将 Vec<u8> 编码为 JSON 值
///
/// **简化模型**：使用 base64 编码字符串（便于 JSON 表示二进制数据）
/// 实际生产中应根据列类型使用对应的 JSON 类型（int/string/boolean/...）
fn encode_row(row: &[u8]) -> serde_json::Value {
    use base64::Engine;
    serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(row))
}

/// 将 JSON 值解码为 Vec<u8>
fn decode_row(value: &serde_json::Value) -> Option<Vec<u8>> {
    use base64::Engine;
    if let serde_json::Value::String(s) = value {
        base64::engine::general_purpose::STANDARD.decode(s).ok()
    } else {
        None
    }
}

// =====================================================================
// ChangeEvent ↔ DebeziumEvent 转换
// =====================================================================

/// 将 SzRSQL ChangeEvent 转换为 Debezium JSON 事件
///
/// **转换规则**：
/// - Insert：before=null, after=row, op="c"
/// - Update：before=old_row, after=new_row, op="u"
/// - Delete：before=old_row, after=null, op="d"
/// - Commit：before=null, after=null, op="r", transaction.status="END"
/// - Abort：before=null, after=null, op="d", transaction.status="ABORT"
pub fn to_debezium(event: &ChangeEvent) -> DebeziumEvent {
    let op = op_to_debezium(event.op).to_string();

    let (before, after, transaction) = match event.op {
        CdcEventOp::Insert => (None, event.new_row.as_ref().map(|r| encode_row(r)), None),
        CdcEventOp::Update => (
            event.old_row.as_ref().map(|r| encode_row(r)),
            event.new_row.as_ref().map(|r| encode_row(r)),
            None,
        ),
        CdcEventOp::Delete => (event.old_row.as_ref().map(|r| encode_row(r)), None, None),
        CdcEventOp::Commit => (
            None,
            None,
            Some(DebeziumTransaction {
                id: format!("tx-{}", event.tx_id),
                status: TX_STATUS_END.to_string(),
            }),
        ),
        CdcEventOp::Abort => (
            None,
            None,
            Some(DebeziumTransaction {
                id: format!("tx-{}", event.tx_id),
                status: TX_STATUS_ABORT.to_string(),
            }),
        ),
    };

    let source = DebeziumSource {
        version: env!("CARGO_PKG_VERSION").to_string(),
        connector: "szrsql".to_string(),
        name: "szrsql".to_string(),
        db: "szrsql".to_string(),
        table_id: event.table_id,
        ts_ms: event.timestamp,
        lsn: event.lsn,
        tx_id: event.tx_id,
    };

    DebeziumEvent {
        schema: DebeziumSchema::default(),
        payload: DebeziumPayload {
            before,
            after,
            source,
            op,
            ts_ms: event.timestamp,
            transaction,
        },
    }
}

/// 将 Debezium JSON 事件转换回 SzRSQL ChangeEvent
///
/// **还原规则**：
/// - op="c"：Insert，new_row = after
/// - op="u"：Update，old_row = before, new_row = after
/// - op="d"：Delete，old_row = before（若无 transaction.status=ABORT）
/// - op="d" + transaction.status="ABORT"：Abort
/// - op="r" + transaction.status="END"：Commit
///
/// **注**：Commit/Abort 在 Debezium 中被映射为 op="r"/"d" + transaction.status，
/// 反向解析时需检查 transaction.status 字段
pub fn from_debezium(debezium: &DebeziumEvent) -> Option<ChangeEvent> {
    let payload = &debezium.payload;
    let source = &payload.source;

    // 优先检查 transaction.status（Commit/Abort 优先识别）
    if let Some(ref tx) = payload.transaction {
        match tx.status.as_str() {
            TX_STATUS_END => {
                // Commit 事件
                return Some(ChangeEvent::commit(source.tx_id, source.lsn, payload.ts_ms));
            }
            TX_STATUS_ABORT => {
                // Abort 事件
                return Some(ChangeEvent::abort(source.tx_id, source.lsn, payload.ts_ms));
            }
            TX_STATUS_BEGIN => {
                // BEGIN 事件不映射为 ChangeEvent（SzRSQL 不暴露 BEGIN）
                return None;
            }
            _ => {}
        }
    }

    // 根据 op 字段还原
    let op = op_from_debezium(&payload.op)?;
    match op {
        CdcEventOp::Insert => {
            let new_row = payload.after.as_ref().and_then(decode_row)?;
            Some(ChangeEvent::insert(
                source.tx_id,
                source.lsn,
                source.table_id.unwrap_or(0),
                new_row,
                payload.ts_ms,
            ))
        }
        CdcEventOp::Update => {
            let old_row = payload
                .before
                .as_ref()
                .and_then(decode_row)
                .unwrap_or_default();
            let new_row = payload.after.as_ref().and_then(decode_row)?;
            Some(ChangeEvent::update(
                source.tx_id,
                source.lsn,
                source.table_id.unwrap_or(0),
                old_row,
                new_row,
                payload.ts_ms,
            ))
        }
        CdcEventOp::Delete => {
            let old_row = payload
                .before
                .as_ref()
                .and_then(decode_row)
                .unwrap_or_default();
            Some(ChangeEvent::delete(
                source.tx_id,
                source.lsn,
                source.table_id.unwrap_or(0),
                old_row,
                payload.ts_ms,
            ))
        }
        // Commit/Abort 已通过 transaction.status 处理，这里不应到达
        CdcEventOp::Commit | CdcEventOp::Abort => None,
    }
}

// =====================================================================
// Debezium JSON 字符串序列化
// =====================================================================

/// 将 ChangeEvent 序列化为 Debezium JSON 字符串
pub fn to_debezium_json(event: &ChangeEvent) -> Result<String, serde_json::Error> {
    let debezium = to_debezium(event);
    serde_json::to_string(&debezium)
}

/// 从 Debezium JSON 字符串反序列化为 ChangeEvent
pub fn from_debezium_json(json: &str) -> Result<ChangeEvent, DebeziumError> {
    let debezium: DebeziumEvent = serde_json::from_str(json)?;
    from_debezium(&debezium).ok_or(DebeziumError::InvalidEvent)
}

/// Debezium 解析错误
#[derive(Debug, thiserror::Error)]
pub enum DebeziumError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid Debezium event")]
    InvalidEvent,
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =================================================================
    // Phase 2.5.4: Debezium JSON 适配器测试
    // =================================================================

    mod phase_2_5_4 {
        use super::*;

        // -----------------------------------------------------------------
        // 1. op 字符串映射
        // -----------------------------------------------------------------

        #[test]
        fn op_to_debezium_insert_maps_to_c() {
            assert_eq!(op_to_debezium(CdcEventOp::Insert), "c");
        }

        #[test]
        fn op_to_debezium_update_maps_to_u() {
            assert_eq!(op_to_debezium(CdcEventOp::Update), "u");
        }

        #[test]
        fn op_to_debezium_delete_maps_to_d() {
            assert_eq!(op_to_debezium(CdcEventOp::Delete), "d");
        }

        #[test]
        fn op_to_debezium_commit_maps_to_r() {
            assert_eq!(op_to_debezium(CdcEventOp::Commit), "r");
        }

        #[test]
        fn op_to_debezium_abort_maps_to_r() {
            assert_eq!(op_to_debezium(CdcEventOp::Abort), "r");
        }

        #[test]
        fn op_from_debezium_c_maps_to_insert() {
            assert_eq!(op_from_debezium("c"), Some(CdcEventOp::Insert));
        }

        #[test]
        fn op_from_debezium_u_maps_to_update() {
            assert_eq!(op_from_debezium("u"), Some(CdcEventOp::Update));
        }

        #[test]
        fn op_from_debezium_d_maps_to_delete() {
            assert_eq!(op_from_debezium("d"), Some(CdcEventOp::Delete));
        }

        #[test]
        fn op_from_debezium_r_maps_to_commit_default() {
            assert_eq!(op_from_debezium("r"), Some(CdcEventOp::Commit));
        }

        #[test]
        fn op_from_debezium_invalid_returns_none() {
            assert_eq!(op_from_debezium("x"), None);
        }

        // -----------------------------------------------------------------
        // 2. DebeziumSource 默认值
        // -----------------------------------------------------------------

        #[test]
        fn debezium_source_default_values() {
            let src = DebeziumSource::default();
            assert_eq!(src.connector, "szrsql");
            assert_eq!(src.name, "szrsql");
            assert_eq!(src.db, "szrsql");
            assert_eq!(src.table_id, None);
            assert_eq!(src.ts_ms, 0);
            assert_eq!(src.lsn, 0);
            assert_eq!(src.tx_id, 0);
        }

        // -----------------------------------------------------------------
        // 3. ChangeEvent → DebeziumEvent 转换
        // -----------------------------------------------------------------

        #[test]
        fn to_debezium_insert_event() {
            let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            let debezium = to_debezium(&event);

            assert_eq!(debezium.payload.op, "c");
            assert!(debezium.payload.before.is_none());
            assert!(debezium.payload.after.is_some());
            assert!(debezium.payload.transaction.is_none());
            assert_eq!(debezium.payload.source.tx_id, 1);
            assert_eq!(debezium.payload.source.lsn, 100);
            assert_eq!(debezium.payload.source.table_id, Some(42));
            assert_eq!(debezium.payload.ts_ms, 12345);
        }

        #[test]
        fn to_debezium_update_event() {
            let event = ChangeEvent::update(1, 100, 42, vec![1], vec![2], 12345);
            let debezium = to_debezium(&event);

            assert_eq!(debezium.payload.op, "u");
            assert!(debezium.payload.before.is_some());
            assert!(debezium.payload.after.is_some());
            assert!(debezium.payload.transaction.is_none());
        }

        #[test]
        fn to_debezium_delete_event() {
            let event = ChangeEvent::delete(1, 100, 42, vec![1, 2], 12345);
            let debezium = to_debezium(&event);

            assert_eq!(debezium.payload.op, "d");
            assert!(debezium.payload.before.is_some());
            assert!(debezium.payload.after.is_none());
            assert!(debezium.payload.transaction.is_none());
        }

        #[test]
        fn to_debezium_commit_event() {
            let event = ChangeEvent::commit(1, 100, 12345);
            let debezium = to_debezium(&event);

            assert_eq!(debezium.payload.op, "r");
            assert!(debezium.payload.before.is_none());
            assert!(debezium.payload.after.is_none());
            assert!(debezium.payload.transaction.is_some());
            assert_eq!(debezium.payload.transaction.as_ref().unwrap().status, "END");
            assert_eq!(debezium.payload.transaction.as_ref().unwrap().id, "tx-1");
        }

        #[test]
        fn to_debezium_abort_event() {
            let event = ChangeEvent::abort(1, 100, 12345);
            let debezium = to_debezium(&event);

            assert_eq!(debezium.payload.op, "r");
            assert!(debezium.payload.before.is_none());
            assert!(debezium.payload.after.is_none());
            assert!(debezium.payload.transaction.is_some());
            assert_eq!(
                debezium.payload.transaction.as_ref().unwrap().status,
                "ABORT"
            );
        }

        // -----------------------------------------------------------------
        // 4. DebeziumEvent → ChangeEvent 反向转换（roundtrip）
        // -----------------------------------------------------------------

        #[test]
        fn roundtrip_insert_event() {
            let original = ChangeEvent::insert(7, 999, 42, vec![1, 2, 3], 12345);
            let debezium = to_debezium(&original);
            let restored = from_debezium(&debezium).unwrap();

            assert_eq!(restored.op, CdcEventOp::Insert);
            assert_eq!(restored.tx_id, original.tx_id);
            assert_eq!(restored.lsn, original.lsn);
            assert_eq!(restored.table_id, original.table_id);
            assert_eq!(restored.new_row, original.new_row);
            assert_eq!(restored.timestamp, original.timestamp);
        }

        #[test]
        fn roundtrip_update_event() {
            let original = ChangeEvent::update(7, 999, 42, vec![1, 2], vec![3, 4], 12345);
            let debezium = to_debezium(&original);
            let restored = from_debezium(&debezium).unwrap();

            assert_eq!(restored.op, CdcEventOp::Update);
            assert_eq!(restored.tx_id, original.tx_id);
            assert_eq!(restored.lsn, original.lsn);
            assert_eq!(restored.old_row, original.old_row);
            assert_eq!(restored.new_row, original.new_row);
        }

        #[test]
        fn roundtrip_delete_event() {
            let original = ChangeEvent::delete(7, 999, 42, vec![1, 2, 3], 12345);
            let debezium = to_debezium(&original);
            let restored = from_debezium(&debezium).unwrap();

            assert_eq!(restored.op, CdcEventOp::Delete);
            assert_eq!(restored.tx_id, original.tx_id);
            assert_eq!(restored.lsn, original.lsn);
            assert_eq!(restored.old_row, original.old_row);
        }

        #[test]
        fn roundtrip_commit_event() {
            let original = ChangeEvent::commit(7, 999, 12345);
            let debezium = to_debezium(&original);
            let restored = from_debezium(&debezium).unwrap();

            assert_eq!(restored.op, CdcEventOp::Commit);
            assert_eq!(restored.tx_id, original.tx_id);
            assert_eq!(restored.lsn, original.lsn);
        }

        #[test]
        fn roundtrip_abort_event() {
            let original = ChangeEvent::abort(7, 999, 12345);
            let debezium = to_debezium(&original);
            let restored = from_debezium(&debezium).unwrap();

            assert_eq!(restored.op, CdcEventOp::Abort);
            assert_eq!(restored.tx_id, original.tx_id);
            assert_eq!(restored.lsn, original.lsn);
        }

        // -----------------------------------------------------------------
        // 5. JSON 字符串序列化
        // -----------------------------------------------------------------

        #[test]
        fn to_debezium_json_insert_serializes_correctly() {
            let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            let json = to_debezium_json(&event).unwrap();

            // 验证 JSON 包含 Debezium 必需字段
            assert!(json.contains("\"schema\""));
            assert!(json.contains("\"payload\""));
            assert!(json.contains("\"before\":null"));
            assert!(json.contains("\"after\""));
            assert!(json.contains("\"op\":\"c\""));
            assert!(json.contains("\"ts_ms\":12345"));
            assert!(json.contains("\"source\""));
            assert!(json.contains("\"tx_id\":1"));
            assert!(json.contains("\"lsn\":100"));
            assert!(json.contains("\"table_id\":42"));
        }

        #[test]
        fn to_debezium_json_update_serializes_correctly() {
            let event = ChangeEvent::update(1, 100, 42, vec![1], vec![2], 12345);
            let json = to_debezium_json(&event).unwrap();

            assert!(json.contains("\"op\":\"u\""));
            assert!(json.contains("\"before\""));
            assert!(json.contains("\"after\""));
        }

        #[test]
        fn to_debezium_json_delete_serializes_correctly() {
            let event = ChangeEvent::delete(1, 100, 42, vec![1], 12345);
            let json = to_debezium_json(&event).unwrap();

            assert!(json.contains("\"op\":\"d\""));
            assert!(json.contains("\"before\""));
            assert!(json.contains("\"after\":null"));
        }

        #[test]
        fn to_debezium_json_commit_includes_transaction() {
            let event = ChangeEvent::commit(1, 100, 12345);
            let json = to_debezium_json(&event).unwrap();

            assert!(json.contains("\"op\":\"r\""));
            assert!(json.contains("\"transaction\""));
            assert!(json.contains("\"status\":\"END\""));
            assert!(json.contains("\"id\":\"tx-1\""));
        }

        #[test]
        fn to_debezium_json_abort_includes_transaction() {
            let event = ChangeEvent::abort(1, 100, 12345);
            let json = to_debezium_json(&event).unwrap();

            assert!(json.contains("\"op\":\"r\""));
            assert!(json.contains("\"status\":\"ABORT\""));
        }

        // -----------------------------------------------------------------
        // 6. JSON 字符串反序列化（完整 roundtrip）
        // -----------------------------------------------------------------

        #[test]
        fn from_debezium_json_insert_roundtrip() {
            let original = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            let json = to_debezium_json(&original).unwrap();
            let restored = from_debezium_json(&json).unwrap();

            assert_eq!(restored.op, original.op);
            assert_eq!(restored.tx_id, original.tx_id);
            assert_eq!(restored.lsn, original.lsn);
            assert_eq!(restored.table_id, original.table_id);
            assert_eq!(restored.new_row, original.new_row);
            assert_eq!(restored.timestamp, original.timestamp);
        }

        #[test]
        fn from_debezium_json_update_roundtrip() {
            let original = ChangeEvent::update(2, 200, 99, vec![10, 20], vec![30, 40], 99999);
            let json = to_debezium_json(&original).unwrap();
            let restored = from_debezium_json(&json).unwrap();

            assert_eq!(restored.op, original.op);
            assert_eq!(restored.tx_id, original.tx_id);
            assert_eq!(restored.lsn, original.lsn);
            assert_eq!(restored.old_row, original.old_row);
            assert_eq!(restored.new_row, original.new_row);
        }

        #[test]
        fn from_debezium_json_delete_roundtrip() {
            let original = ChangeEvent::delete(3, 300, 88, vec![5, 6, 7], 55555);
            let json = to_debezium_json(&original).unwrap();
            let restored = from_debezium_json(&json).unwrap();

            assert_eq!(restored.op, original.op);
            assert_eq!(restored.tx_id, original.tx_id);
            assert_eq!(restored.lsn, original.lsn);
            assert_eq!(restored.old_row, original.old_row);
        }

        #[test]
        fn from_debezium_json_commit_roundtrip() {
            let original = ChangeEvent::commit(4, 400, 77777);
            let json = to_debezium_json(&original).unwrap();
            let restored = from_debezium_json(&json).unwrap();

            assert_eq!(restored.op, original.op);
            assert_eq!(restored.tx_id, original.tx_id);
            assert_eq!(restored.lsn, original.lsn);
        }

        #[test]
        fn from_debezium_json_abort_roundtrip() {
            let original = ChangeEvent::abort(5, 500, 33333);
            let json = to_debezium_json(&original).unwrap();
            let restored = from_debezium_json(&json).unwrap();

            assert_eq!(restored.op, original.op);
            assert_eq!(restored.tx_id, original.tx_id);
            assert_eq!(restored.lsn, original.lsn);
        }

        // -----------------------------------------------------------------
        // 7. Debezium schema 验证
        // -----------------------------------------------------------------

        #[test]
        fn debezium_schema_default_has_required_fields() {
            let schema = DebeziumSchema::default();
            assert_eq!(schema.schema_type, "struct");
            assert!(schema.name.is_some());
            let field_names: Vec<_> = schema
                .fields
                .iter()
                .map(|f| f.field.as_ref().unwrap().as_str())
                .collect();
            assert!(field_names.contains(&"before"));
            assert!(field_names.contains(&"after"));
            assert!(field_names.contains(&"source"));
            assert!(field_names.contains(&"op"));
            assert!(field_names.contains(&"ts_ms"));
            assert!(field_names.contains(&"transaction"));
        }

        #[test]
        fn debezium_schema_serializes_to_json() {
            let schema = DebeziumSchema::default();
            let json = serde_json::to_string(&schema).unwrap();
            assert!(json.contains("\"type\":\"struct\""));
            assert!(json.contains("\"name\":\"io.szrsql.cdc.ChangeEvent\""));
            assert!(json.contains("\"fields\""));
        }

        // -----------------------------------------------------------------
        // 8. DebeziumEvent 完整结构序列化
        // -----------------------------------------------------------------

        #[test]
        fn debezium_event_full_json_structure() {
            let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            let debezium = to_debezium(&event);
            let json = serde_json::to_string_pretty(&debezium).unwrap();

            // 验证完整的 Debezium JSON 结构
            assert!(json.contains("\"schema\""));
            assert!(json.contains("\"payload\""));
            assert!(json.contains("\"before\""));
            assert!(json.contains("\"after\""));
            assert!(json.contains("\"source\""));
            assert!(json.contains("\"version\""));
            assert!(json.contains("\"connector\""));
            assert!(json.contains("\"name\""));
            assert!(json.contains("\"db\""));
            assert!(json.contains("\"ts_ms\""));
            assert!(json.contains("\"lsn\""));
            assert!(json.contains("\"tx_id\""));
            assert!(json.contains("\"op\""));
        }

        // -----------------------------------------------------------------
        // 9. base64 行数据编码/解码
        // -----------------------------------------------------------------

        #[test]
        fn encode_row_uses_base64() {
            let row = vec![1, 2, 3, 4, 5];
            let encoded = encode_row(&row);
            // base64 of [1,2,3,4,5] = "AQIDBAU="
            assert_eq!(encoded, serde_json::Value::String("AQIDBAU=".to_string()));
        }

        #[test]
        fn decode_row_roundtrip() {
            let original = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
            let encoded = encode_row(&original);
            let decoded = decode_row(&encoded).unwrap();
            assert_eq!(decoded, original);
        }

        #[test]
        fn decode_row_empty_bytes() {
            let original: Vec<u8> = vec![];
            let encoded = encode_row(&original);
            let decoded = decode_row(&encoded).unwrap();
            assert_eq!(decoded, original);
        }

        #[test]
        fn decode_row_invalid_returns_none() {
            let invalid = serde_json::Value::String("!!!invalid base64!!!".to_string());
            assert!(decode_row(&invalid).is_none());

            let not_string = serde_json::Value::Number(42.into());
            assert!(decode_row(&not_string).is_none());
        }

        // -----------------------------------------------------------------
        // 10. 错误处理
        // -----------------------------------------------------------------

        #[test]
        fn from_debezium_json_invalid_json_returns_error() {
            let result = from_debezium_json("not valid json");
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), DebeziumError::Json(_)));
        }

        #[test]
        fn from_debezium_json_missing_payload_returns_error() {
            let json = r#"{"schema":{}}"#;
            let result = from_debezium_json(json);
            assert!(result.is_err());
        }

        // -----------------------------------------------------------------
        // 11. 批量转换
        // -----------------------------------------------------------------

        #[test]
        fn batch_to_debezium_json_roundtrip() {
            let events = [
                ChangeEvent::insert(1, 100, 42, vec![1], 1000),
                ChangeEvent::update(1, 101, 42, vec![1], vec![2], 1001),
                ChangeEvent::delete(1, 102, 42, vec![2], 1002),
                ChangeEvent::commit(1, 103, 1003),
            ];

            // 批量序列化
            let debezium_events: Vec<DebeziumEvent> = events.iter().map(to_debezium).collect();
            let json = serde_json::to_string(&debezium_events).unwrap();

            // 批量反序列化
            let restored_events: Vec<DebeziumEvent> = serde_json::from_str(&json).unwrap();
            let restored: Vec<ChangeEvent> =
                restored_events.iter().filter_map(from_debezium).collect();

            assert_eq!(restored.len(), events.len());
            for (orig, rest) in events.iter().zip(restored.iter()) {
                assert_eq!(rest.op, orig.op);
                assert_eq!(rest.tx_id, orig.tx_id);
                assert_eq!(rest.lsn, orig.lsn);
                assert_eq!(rest.timestamp, orig.timestamp);
            }
        }

        // -----------------------------------------------------------------
        // 12. 与 Debezium 官方格式兼容性验证
        // -----------------------------------------------------------------

        #[test]
        fn debezium_json_matches_official_format_insert() {
            // 验证 SzRSQL 输出的 JSON 结构与 Debezium 官方格式兼容
            let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            let json = to_debezium_json(&event).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

            // 必需字段：schema + payload
            assert!(parsed.get("schema").is_some());
            assert!(parsed.get("payload").is_some());

            let payload = parsed.get("payload").unwrap();
            // 必需 payload 字段：before, after, source, op, ts_ms
            assert!(payload.get("before").is_some());
            assert!(payload.get("after").is_some());
            assert!(payload.get("source").is_some());
            assert!(payload.get("op").is_some());
            assert!(payload.get("ts_ms").is_some());

            // op 必须是 c/u/d/r 之一
            let op = payload.get("op").unwrap().as_str().unwrap();
            assert!(matches!(op, "c" | "u" | "d" | "r"));
        }

        #[test]
        fn debezium_json_source_metadata_complete() {
            let event = ChangeEvent::insert(7, 999, 42, vec![1], 12345);
            let json = to_debezium_json(&event).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

            let source = parsed.get("payload").unwrap().get("source").unwrap();
            assert_eq!(
                source.get("version").unwrap().as_str().unwrap(),
                env!("CARGO_PKG_VERSION")
            );
            assert_eq!(source.get("connector").unwrap().as_str().unwrap(), "szrsql");
            assert_eq!(source.get("name").unwrap().as_str().unwrap(), "szrsql");
            assert_eq!(source.get("db").unwrap().as_str().unwrap(), "szrsql");
            assert_eq!(source.get("table_id").unwrap().as_u64().unwrap(), 42);
            assert_eq!(source.get("ts_ms").unwrap().as_u64().unwrap(), 12345);
            assert_eq!(source.get("lsn").unwrap().as_u64().unwrap(), 999);
            assert_eq!(source.get("tx_id").unwrap().as_u64().unwrap(), 7);
        }

        // -----------------------------------------------------------------
        // 13. 不变量：to_debezium 后所有字段可还原
        // -----------------------------------------------------------------

        #[test]
        fn invariant_to_debezium_preserves_all_roundtrip() {
            // 所有 op 类型的 roundtrip 不变量
            let events = [
                ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 1000),
                ChangeEvent::update(2, 200, 99, vec![10], vec![20], 2000),
                ChangeEvent::delete(3, 300, 88, vec![5, 6, 7], 3000),
                ChangeEvent::commit(4, 400, 4000),
                ChangeEvent::abort(5, 500, 5000),
            ];

            for original in &events {
                let debezium = to_debezium(original);
                let restored = from_debezium(&debezium).unwrap();
                assert_eq!(
                    restored.op, original.op,
                    "op mismatch for {:?}",
                    original.op
                );
                assert_eq!(restored.tx_id, original.tx_id, "tx_id mismatch");
                assert_eq!(restored.lsn, original.lsn, "lsn mismatch");
                assert_eq!(restored.timestamp, original.timestamp, "timestamp mismatch");

                // 行数据 roundtrip（Commit/Abort 无行数据）
                if original.op != CdcEventOp::Commit && original.op != CdcEventOp::Abort {
                    assert_eq!(
                        restored.old_row, original.old_row,
                        "old_row mismatch for {:?}",
                        original.op
                    );
                    assert_eq!(
                        restored.new_row, original.new_row,
                        "new_row mismatch for {:?}",
                        original.op
                    );
                    assert_eq!(
                        restored.table_id, original.table_id,
                        "table_id mismatch for {:?}",
                        original.op
                    );
                }
            }
        }
    }
}
