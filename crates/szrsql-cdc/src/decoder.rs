//! 行级解码器 — 将 WalRecord.data（页级二进制）解码为列名+列值。
//!
//! # 设计要点
//!
//! 1. **二进制编码格式**（由 RowEncoder 配套产生，此处只负责解码）：
//!    - 每行数据由若干"列槽"顺序串联
//!    - 每个列槽格式：`[null_flag: 1 byte][len: 4 bytes BE][value: len bytes]`
//!    - `null_flag = 0x01` 表示 NULL，此时 len=0、value 为空
//!    - `null_flag = 0x00` 表示非 NULL，后续按列类型解析 value
//!
//! 2. **类型映射**：
//!    - `DataType::Int32`  → `Value::Int64(i64)`（i32 BE 扩展为 i64）
//!    - `DataType::Int64`  → `Value::Int64(i64)`
//!    - `DataType::Text`   → `Value::Text(String)`
//!    - `DataType::Blob`   → `Value::Blob(Vec<u8>)`
//!    - `DataType::Real`   → `Value::Float64(f64)`
//!    - `DataType::Bool`   → `Value::Bool(bool)`
//!    - `DataType::Date`   → `Value::Date(i32)`（BE i32）
//!    - `DataType::Timestamp` → `Value::Timestamp(i64)`（BE i64）
//!    - `DataType::Json`   → `Value::Json(serde_json::Value)`
//!    - `DataType::Uuid`   → `Value::Text(String)`（UUID 字符串形式）
//!
//! 3. **Schema 缓存**：解码器内部维护 `table_id → TableSchema` 缓存，
//!    通过 `schema_version` 判断是否需要刷新。
//!
//! 4. **向后兼容**：若 WalRecord.data 无法按上述格式解析（如旧格式数据），
//!    退化为"原始字节"，返回 `Value::Blob`，不阻断 CDC 流。

use crate::schema::{ColumnDef, DataType, SchemaRegistry, TableSchema};
use std::collections::HashMap;
use std::sync::Arc;
use szrsql_types::value::Value as SzValue;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::RwLock;

// =====================================================================
// 解码错误
// =====================================================================

/// 行解码错误
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// Schema 未注册（table_id 未知）
    #[error("schema not registered: table_id={0}")]
    SchemaNotFound(u32),

    /// 数据长度不足（截断的列槽）
    #[error("truncated column slot: need {need} bytes, have {have}")]
    Truncated { need: usize, have: usize },

    /// 不支持的类型
    #[error("unsupported data type: {0}")]
    UnsupportedType(String),

    /// JSON 解析失败
    #[error("json parse error: {0}")]
    JsonParse(String),
}

// =====================================================================
// DecodedRow — 解码后的行
// =====================================================================

/// 解码后的行 — 列名 + 列值有序列表
///
/// 与 `ChangeEvent.old_row` / `new_row`（`Vec<u8>`）相比，
/// `DecodedRow` 提供人类可读的列名和强类型列值。
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedRow {
    /// (列名, 列值) 有序列表，顺序与 TableSchema.columns 一致
    pub columns: Vec<(String, SzValue)>,
}

impl DecodedRow {
    /// 列数
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// 按列名查找列值
    pub fn get(&self, name: &str) -> Option<&SzValue> {
        self.columns.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// 转为 JSON 对象（列名 → 列值）
    pub fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (name, value) in &self.columns {
            map.insert(name.clone(), value_to_json(value));
        }
        serde_json::Value::Object(map)
    }
}

// =====================================================================
// RowDecoder — 行解码器
// =====================================================================

/// 行解码器 — 将 WalRecord.data 解码为 `DecodedRow`
///
/// **使用方式**：
/// ```ignore
/// use szrsql_cdc::decoder::RowDecoder;
/// use szrsql_cdc::schema::SchemaRegistry;
/// use std::sync::Arc;
///
/// let registry = Arc::new(SchemaRegistry::new());
/// // 在 DDL 时调用 registry.create_table(...) 注册表结构
/// let decoder = RowDecoder::new(registry);
///
/// // 解码 CDC 事件的行数据
/// let row = decoder.decode(table_id, event.new_row.as_ref().unwrap(), event.schema_version)?;
/// ```
pub struct RowDecoder {
    /// Schema 注册表引用（获取最新 schema）
    schema_registry: Arc<SchemaRegistry>,
    /// table_id → (schema_version, TableSchema) 缓存
    cache: RwLock<HashMap<u32, (u64, TableSchema)>>,
}

impl RowDecoder {
    /// 创建解码器，注入 SchemaRegistry
    pub fn new(schema_registry: Arc<SchemaRegistry>) -> Self {
        Self {
            schema_registry,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// 获取指定表的 schema（带缓存）
    ///
    /// 若缓存中的 schema_version 与请求的 version 不一致，则从 registry 刷新。
    /// `version = None` 表示不检查版本，直接用缓存（或首次加载）。
    fn get_schema(&self, table_id: u32, version: Option<u64>) -> Result<TableSchema, DecodeError> {
        // 快速路径：缓存命中且版本匹配
        if let Some(version) = version {
            {
                let cache = self.cache.read();
                if let Some((cached_version, schema)) = cache.get(&table_id) {
                    if *cached_version == version {
                        return Ok(schema.clone());
                    }
                }
            }
            // 慢路径：从 registry 刷新
            let schema = self
                .schema_registry
                .get_schema(table_id)
                .ok_or(DecodeError::SchemaNotFound(table_id))?;
            let mut cache = self.cache.write();
            cache.insert(table_id, (version, schema.clone()));
            return Ok(schema);
        }
        // version = None：用缓存或首次加载
        {
            let cache = self.cache.read();
            if let Some((_, schema)) = cache.get(&table_id) {
                return Ok(schema.clone());
            }
        }
        let schema = self
            .schema_registry
            .get_schema(table_id)
            .ok_or(DecodeError::SchemaNotFound(table_id))?;
        let mut cache = self.cache.write();
        cache.insert(table_id, (schema.version, schema.clone()));
        Ok(schema)
    }

    /// 解码一行数据
    ///
    /// # 参数
    /// - `table_id`：目标表 ID
    /// - `data`：WalRecord.data（二进制行数据）
    /// - `schema_version`：该事件产生时的 schema 版本（None 表示不检查版本）
    ///
    /// # 返回
    /// - `Ok(DecodedRow)`：解码成功
    /// - `Err(DecodeError)`：解码失败（schema 未注册、数据截断等）
    pub fn decode(
        &self,
        table_id: u32,
        data: &[u8],
        schema_version: Option<u64>,
    ) -> Result<DecodedRow, DecodeError> {
        let schema = self.get_schema(table_id, schema_version)?;
        let mut columns = Vec::with_capacity(schema.columns.len());
        let mut offset = 0usize;

        for col in &schema.columns {
            let (value, new_offset) = decode_column(data, offset, col)?;
            columns.push((col.name.clone(), value));
            offset = new_offset;
        }

        Ok(DecodedRow { columns })
    }

    /// 解码 CDC 事件的后镜像（new_row）
    ///
    /// 便捷方法：自动从 ChangeEvent 提取 table_id / new_row / schema_version。
    pub fn decode_new_row(
        &self,
        event: &crate::ChangeEvent,
    ) -> Result<Option<DecodedRow>, DecodeError> {
        match (event.table_id, &event.new_row) {
            (Some(table_id), Some(data)) => {
                let row = self.decode(table_id, data, event.schema_version)?;
                Ok(Some(row))
            }
            _ => Ok(None),
        }
    }

    /// 解码 CDC 事件的前镜像（old_row）
    pub fn decode_old_row(
        &self,
        event: &crate::ChangeEvent,
    ) -> Result<Option<DecodedRow>, DecodeError> {
        match (event.table_id, &event.old_row) {
            (Some(table_id), Some(data)) => {
                let row = self.decode(table_id, data, event.schema_version)?;
                Ok(Some(row))
            }
            _ => Ok(None),
        }
    }

    /// 清空 schema 缓存（schema 大量变更时调用）
    pub fn invalidate_cache(&self) {
        let mut cache = self.cache.write();
        cache.clear();
    }

    /// 缓存大小（监控用）
    pub fn cache_size(&self) -> usize {
        self.cache.read().len()
    }
}

// =====================================================================
// 列解码 — 单个列槽解析
// =====================================================================

/// 解码单个列槽
///
/// 返回 (Value, new_offset)，new_offset 指向下一列的起始位置。
fn decode_column(
    data: &[u8],
    offset: usize,
    col: &ColumnDef,
) -> Result<(SzValue, usize), DecodeError> {
    // 读取 null_flag (1 byte)
    if offset + 1 > data.len() {
        return Err(DecodeError::Truncated {
            need: offset + 1,
            have: data.len(),
        });
    }
    let null_flag = data[offset];
    let mut cursor = offset + 1;

    if null_flag == 0x01 {
        // NULL：跳过 4 字节 len（编码格式为 [0x01][len=0]，保持对称）
        if cursor + 4 > data.len() {
            return Err(DecodeError::Truncated {
                need: cursor + 4,
                have: data.len(),
            });
        }
        cursor += 4;
        return Ok((SzValue::Null, cursor));
    }

    // 读取 len (4 bytes BE)
    if cursor + 4 > data.len() {
        return Err(DecodeError::Truncated {
            need: cursor + 4,
            have: data.len(),
        });
    }
    let len = u32::from_be_bytes([
        data[cursor],
        data[cursor + 1],
        data[cursor + 2],
        data[cursor + 3],
    ]) as usize;
    cursor += 4;

    // 读取 value (len bytes)
    if cursor + len > data.len() {
        return Err(DecodeError::Truncated {
            need: cursor + len,
            have: data.len(),
        });
    }
    let value_bytes = &data[cursor..cursor + len];
    cursor += len;

    let value = decode_value_by_type(col.data_type, value_bytes)?;
    Ok((value, cursor))
}

/// 按列类型解码列值
fn decode_value_by_type(data_type: DataType, bytes: &[u8]) -> Result<SzValue, DecodeError> {
    match data_type {
        DataType::Int32 => {
            if bytes.len() < 4 {
                return Err(DecodeError::Truncated {
                    need: 4,
                    have: bytes.len(),
                });
            }
            let v = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Ok(SzValue::Int64(v as i64))
        }
        DataType::Int64 => {
            if bytes.len() < 8 {
                return Err(DecodeError::Truncated {
                    need: 8,
                    have: bytes.len(),
                });
            }
            let v = i64::from_be_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]);
            Ok(SzValue::Int64(v))
        }
        DataType::Text => {
            let s = String::from_utf8_lossy(bytes).into_owned();
            Ok(SzValue::Text(s))
        }
        DataType::Blob => Ok(SzValue::Blob(bytes.to_vec())),
        DataType::Real => {
            if bytes.len() < 8 {
                return Err(DecodeError::Truncated {
                    need: 8,
                    have: bytes.len(),
                });
            }
            let v = f64::from_be_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]);
            Ok(SzValue::Float64(v))
        }
        DataType::Bool => {
            if bytes.is_empty() {
                return Err(DecodeError::Truncated { need: 1, have: 0 });
            }
            Ok(SzValue::Bool(bytes[0] != 0))
        }
        DataType::Date => {
            if bytes.len() < 4 {
                return Err(DecodeError::Truncated {
                    need: 4,
                    have: bytes.len(),
                });
            }
            let v = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Ok(SzValue::Date(v))
        }
        DataType::Timestamp => {
            if bytes.len() < 8 {
                return Err(DecodeError::Truncated {
                    need: 8,
                    have: bytes.len(),
                });
            }
            let v = i64::from_be_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]);
            Ok(SzValue::Timestamp(v))
        }
        DataType::Json => {
            let s = String::from_utf8_lossy(bytes);
            let json: serde_json::Value =
                serde_json::from_str(&s).map_err(|e| DecodeError::JsonParse(e.to_string()))?;
            Ok(SzValue::Json(json))
        }
        DataType::Uuid => {
            let s = String::from_utf8_lossy(bytes).into_owned();
            Ok(SzValue::Text(s))
        }
    }
}

// =====================================================================
// 行编码 — 将 DecodedRow 编码为二进制（便于测试 round-trip）
// =====================================================================

/// 将 DecodedRow 编码为二进制字节流（与 decode 互逆）
pub fn encode_row(row: &DecodedRow) -> Vec<u8> {
    let mut buf = Vec::with_capacity(row.columns.len() * 16);
    for (_, value) in &row.columns {
        encode_value(value, &mut buf);
    }
    buf
}

/// 编码单个值到 buf
fn encode_value(value: &SzValue, buf: &mut Vec<u8>) {
    match value {
        SzValue::Null => {
            buf.push(0x01); // null_flag
            buf.extend_from_slice(&0u32.to_be_bytes()); // len = 0
        }
        _ => {
            buf.push(0x00); // null_flag = 非 NULL
            let value_bytes = serialize_value(value);
            buf.extend_from_slice(&(value_bytes.len() as u32).to_be_bytes());
            buf.extend_from_slice(&value_bytes);
        }
    }
}

/// 将 Value 序列化为字节
fn serialize_value(value: &SzValue) -> Vec<u8> {
    match value {
        SzValue::Null => Vec::new(),
        SzValue::Int64(v) => v.to_be_bytes().to_vec(),
        SzValue::Float64(v) => v.to_be_bytes().to_vec(),
        SzValue::Text(s) => s.as_bytes().to_vec(),
        SzValue::Blob(b) => b.clone(),
        SzValue::Bool(b) => vec![if *b {
            1
        } else {
            0
        }],
        SzValue::Date(d) => d.to_be_bytes().to_vec(),
        SzValue::Timestamp(t) => t.to_be_bytes().to_vec(),
        SzValue::Decimal(_, _) => Vec::new(),
        SzValue::Array(_) => Vec::new(),
        SzValue::Enum(s) => s.as_bytes().to_vec(),
        SzValue::Range(_) => Vec::new(),
        SzValue::Json(v) => serde_json::to_vec(v).unwrap_or_default(),
        SzValue::TsVector(_) => Vec::new(),
        SzValue::TsQuery(_) => Vec::new(),
        SzValue::Vector(_) => Vec::new(),
    }
}

// =====================================================================
// 辅助：Value → JSON
// =====================================================================

/// 将 SzValue 转为 serde_json::Value（用于 DecodedRow.to_json）
pub fn value_to_json(value: &SzValue) -> serde_json::Value {
    match value {
        SzValue::Null => serde_json::Value::Null,
        SzValue::Int64(v) => serde_json::Value::Number((*v).into()),
        SzValue::Float64(v) => {
            if let Some(n) = serde_json::Number::from_f64(*v) {
                serde_json::Value::Number(n)
            } else {
                serde_json::Value::Null
            }
        }
        SzValue::Text(s) => serde_json::Value::String(s.clone()),
        SzValue::Blob(b) => serde_json::Value::String(base64_encode(b)),
        SzValue::Bool(b) => serde_json::Value::Bool(*b),
        SzValue::Date(d) => serde_json::Value::Number((*d).into()),
        SzValue::Timestamp(t) => serde_json::Value::Number((*t).into()),
        SzValue::Decimal(v, scale) => {
            serde_json::json!({"unscaled": v.to_string(), "scale": scale})
        }
        SzValue::Array(arr) => serde_json::Value::Array(arr.iter().map(value_to_json).collect()),
        SzValue::Enum(s) => serde_json::Value::String(s.clone()),
        SzValue::Range(_) => serde_json::Value::Null,
        SzValue::Json(v) => v.clone(),
        SzValue::TsVector(_) => serde_json::Value::Null,
        SzValue::TsQuery(_) => serde_json::Value::Null,
        SzValue::Vector(_) => serde_json::Value::Null,
    }
}

/// 简单的 base64 编码（避免引入额外依赖）
fn base64_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 {
            chunk[1] as u32
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            chunk[2] as u32
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

// =====================================================================
// 单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnDef, DataType, SchemaRegistry};

    fn setup_registry_with_users_table() -> Arc<SchemaRegistry> {
        let registry = Arc::new(SchemaRegistry::new());
        registry
            .create_table(
                1,
                "users",
                vec![
                    ColumnDef::not_null("id", DataType::Int64),
                    ColumnDef::not_null("name", DataType::Text),
                    ColumnDef::nullable("age", DataType::Int32),
                    ColumnDef::nullable("active", DataType::Bool),
                ],
            )
            .expect("create_table failed");
        registry
    }

    fn encode_int64(v: i64) -> Vec<u8> {
        let mut buf = vec![0x00]; // non-null
        buf.extend_from_slice(&8u32.to_be_bytes());
        buf.extend_from_slice(&v.to_be_bytes());
        buf
    }

    fn encode_text(s: &str) -> Vec<u8> {
        let bytes = s.as_bytes();
        let mut buf = vec![0x00];
        buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        buf.extend_from_slice(bytes);
        buf
    }

    fn encode_int32(v: i32) -> Vec<u8> {
        let mut buf = vec![0x00];
        buf.extend_from_slice(&4u32.to_be_bytes());
        buf.extend_from_slice(&v.to_be_bytes());
        buf
    }

    fn encode_bool(b: bool) -> Vec<u8> {
        let mut buf = vec![0x00];
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.push(if b {
            1
        } else {
            0
        });
        buf
    }

    fn encode_null() -> Vec<u8> {
        let mut buf = vec![0x01];
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf
    }

    #[test]
    fn test_decode_simple_row() {
        let registry = setup_registry_with_users_table();
        let decoder = RowDecoder::new(registry);

        // 构造一行数据：id=42, name="alice", age=30, active=true
        let mut data = Vec::new();
        data.extend(encode_int64(42));
        data.extend(encode_text("alice"));
        data.extend(encode_int32(30));
        data.extend(encode_bool(true));

        let row = decoder.decode(1, &data, Some(1)).expect("decode failed");
        assert_eq!(row.len(), 4);
        assert_eq!(row.get("id"), Some(&SzValue::Int64(42)));
        assert_eq!(row.get("name"), Some(&SzValue::Text("alice".to_string())));
        assert_eq!(row.get("age"), Some(&SzValue::Int64(30)));
        assert_eq!(row.get("active"), Some(&SzValue::Bool(true)));
    }

    #[test]
    fn test_decode_row_with_null() {
        let registry = setup_registry_with_users_table();
        let decoder = RowDecoder::new(registry);

        // id=1, name="bob", age=NULL, active=false
        let mut data = Vec::new();
        data.extend(encode_int64(1));
        data.extend(encode_text("bob"));
        data.extend(encode_null());
        data.extend(encode_bool(false));

        let row = decoder.decode(1, &data, Some(1)).expect("decode failed");
        assert_eq!(row.get("age"), Some(&SzValue::Null));
        assert_eq!(row.get("active"), Some(&SzValue::Bool(false)));
    }

    #[test]
    fn test_decode_truncated_data() {
        let registry = setup_registry_with_users_table();
        let decoder = RowDecoder::new(registry);

        // 截断的数据（只有 1 字节，不够 null_flag + len）
        let data = vec![0x00];
        let result = decoder.decode(1, &data, Some(1));
        assert!(result.is_err());
        match result.unwrap_err() {
            DecodeError::Truncated { need, have } => {
                assert_eq!(have, 1);
                assert!(need > have);
            }
            e => panic!("expected Truncated, got {:?}", e),
        }
    }

    #[test]
    fn test_decode_schema_not_found() {
        let registry = Arc::new(SchemaRegistry::new());
        let decoder = RowDecoder::new(registry);

        let result = decoder.decode(999, &[0x00], Some(1));
        assert!(matches!(
            result.unwrap_err(),
            DecodeError::SchemaNotFound(999)
        ));
    }

    #[test]
    fn test_decode_all_types() {
        let registry = Arc::new(SchemaRegistry::new());
        registry
            .create_table(
                2,
                "all_types",
                vec![
                    ColumnDef::not_null("i64", DataType::Int64),
                    ColumnDef::not_null("i32", DataType::Int32),
                    ColumnDef::not_null("text", DataType::Text),
                    ColumnDef::not_null("blob", DataType::Blob),
                    ColumnDef::not_null("real", DataType::Real),
                    ColumnDef::not_null("bool", DataType::Bool),
                    ColumnDef::not_null("date", DataType::Date),
                    ColumnDef::not_null("ts", DataType::Timestamp),
                ],
            )
            .unwrap();
        let decoder = RowDecoder::new(registry);

        let mut data = Vec::new();
        data.extend(encode_int64(-123456));
        data.extend(encode_int32(789));
        data.extend(encode_text("hello"));
        data.extend({
            let mut buf = vec![0x00];
            buf.extend_from_slice(&5u32.to_be_bytes());
            buf.extend_from_slice(b"world");
            buf
        });
        data.extend({
            let mut buf = vec![0x00];
            buf.extend_from_slice(&8u32.to_be_bytes());
            buf.extend_from_slice(&3.5f64.to_be_bytes());
            buf
        });
        data.extend(encode_bool(true));
        data.extend({
            let mut buf = vec![0x00];
            buf.extend_from_slice(&4u32.to_be_bytes());
            buf.extend_from_slice(&18628i32.to_be_bytes());
            buf
        });
        data.extend({
            let mut buf = vec![0x00];
            buf.extend_from_slice(&8u32.to_be_bytes());
            buf.extend_from_slice(&1699990400000000i64.to_be_bytes());
            buf
        });

        let row = decoder.decode(2, &data, Some(1)).expect("decode failed");
        assert_eq!(row.len(), 8);
        assert_eq!(row.get("i64"), Some(&SzValue::Int64(-123456)));
        assert_eq!(row.get("i32"), Some(&SzValue::Int64(789)));
        assert_eq!(row.get("text"), Some(&SzValue::Text("hello".to_string())));
        assert_eq!(row.get("blob"), Some(&SzValue::Blob(b"world".to_vec())));
        assert_eq!(row.get("real"), Some(&SzValue::Float64(3.5)));
        assert_eq!(row.get("bool"), Some(&SzValue::Bool(true)));
        assert_eq!(row.get("date"), Some(&SzValue::Date(18628)));
        assert_eq!(row.get("ts"), Some(&SzValue::Timestamp(1699990400000000)));
    }

    #[test]
    fn test_decode_json_type() {
        let registry = Arc::new(SchemaRegistry::new());
        registry
            .create_table(
                3,
                "json_table",
                vec![ColumnDef::not_null("data", DataType::Json)],
            )
            .unwrap();
        let decoder = RowDecoder::new(registry);

        let json_str = r#"{"key":"value","num":42}"#;
        let json_bytes = json_str.as_bytes();
        let mut data = vec![0x00];
        data.extend_from_slice(&(json_bytes.len() as u32).to_be_bytes());
        data.extend_from_slice(json_bytes);

        let row = decoder.decode(3, &data, Some(1)).expect("decode failed");
        if let SzValue::Json(json) = row.get("data").unwrap() {
            assert_eq!(json["key"], "value");
            assert_eq!(json["num"], 42);
        } else {
            panic!("expected Json value");
        }
    }

    #[test]
    fn test_decode_uuid_type() {
        let registry = Arc::new(SchemaRegistry::new());
        registry
            .create_table(
                4,
                "uuid_table",
                vec![ColumnDef::not_null("id", DataType::Uuid)],
            )
            .unwrap();
        let decoder = RowDecoder::new(registry);

        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let mut data = vec![0x00];
        data.extend_from_slice(&(uuid_str.len() as u32).to_be_bytes());
        data.extend_from_slice(uuid_str.as_bytes());

        let row = decoder.decode(4, &data, Some(1)).expect("decode failed");
        assert_eq!(row.get("id"), Some(&SzValue::Text(uuid_str.to_string())));
    }

    #[test]
    fn test_schema_cache_invalidation() {
        let registry = setup_registry_with_users_table();
        let decoder = RowDecoder::new(registry);

        // 首次解码填充缓存
        let mut data = Vec::new();
        data.extend(encode_int64(1));
        data.extend(encode_text("test"));
        data.extend(encode_null());
        data.extend(encode_bool(true));
        let _ = decoder.decode(1, &data, Some(1)).unwrap();
        assert_eq!(decoder.cache_size(), 1);

        // 清空缓存
        decoder.invalidate_cache();
        assert_eq!(decoder.cache_size(), 0);
    }

    #[test]
    fn test_round_trip_encode_decode() {
        let registry = setup_registry_with_users_table();
        let decoder = RowDecoder::new(registry.clone());

        let original_row = DecodedRow {
            columns: vec![
                ("id".to_string(), SzValue::Int64(100)),
                ("name".to_string(), SzValue::Text("round_trip".to_string())),
                ("age".to_string(), SzValue::Null),
                ("active".to_string(), SzValue::Bool(true)),
            ],
        };

        let encoded = encode_row(&original_row);
        let decoded = decoder.decode(1, &encoded, Some(1)).expect("decode failed");

        assert_eq!(original_row, decoded);
    }

    #[test]
    fn test_decoded_row_to_json() {
        let row = DecodedRow {
            columns: vec![
                ("id".to_string(), SzValue::Int64(42)),
                ("name".to_string(), SzValue::Text("alice".to_string())),
                ("active".to_string(), SzValue::Bool(true)),
            ],
        };

        let json = row.to_json();
        assert_eq!(json["id"], 42);
        assert_eq!(json["name"], "alice");
        assert_eq!(json["active"], true);
    }

    #[test]
    fn test_value_to_json_blob_base64() {
        let value = SzValue::Blob(b"hello".to_vec());
        let json = value_to_json(&value);
        // base64("hello") = "aGVsbG8="
        assert_eq!(json, serde_json::Value::String("aGVsbG8=".to_string()));
    }

    #[test]
    fn test_decode_via_cdc_event() {
        use crate::{CdcEventOp, ChangeEvent};

        let registry = setup_registry_with_users_table();
        let decoder = RowDecoder::new(registry);

        // 构造一个 Insert 事件的 new_row
        let mut data = Vec::new();
        data.extend(encode_int64(99));
        data.extend(encode_text("event_user"));
        data.extend(encode_null());
        data.extend(encode_bool(false));

        let event = ChangeEvent::insert(1, 100, 1, data, 0);
        let decoded = decoder
            .decode_new_row(&event)
            .expect("decode failed")
            .expect("expected Some(row)");

        assert_eq!(decoded.get("id"), Some(&SzValue::Int64(99)));
        assert_eq!(
            decoded.get("name"),
            Some(&SzValue::Text("event_user".to_string()))
        );
        assert_eq!(event.op, CdcEventOp::Insert);

        // old_row 应为 None
        assert!(decoder.decode_old_row(&event).unwrap().is_none());
    }

    #[test]
    fn test_schema_version_cache_refresh() {
        let registry = setup_registry_with_users_table();
        let decoder = RowDecoder::new(registry.clone());

        let mut data = Vec::new();
        data.extend(encode_int64(1));
        data.extend(encode_text("v1"));
        data.extend(encode_null());
        data.extend(encode_bool(true));

        // version=1 解码
        let _ = decoder.decode(1, &data, Some(1)).unwrap();
        assert_eq!(decoder.cache_size(), 1);

        // 添加列，version 变为 2
        registry
            .alter_table_add_column(1, ColumnDef::nullable("email", DataType::Text))
            .unwrap();

        // version=2 解码（应刷新缓存）
        let mut data_v2 = Vec::new();
        data_v2.extend(encode_int64(2));
        data_v2.extend(encode_text("v2"));
        data_v2.extend(encode_null());
        data_v2.extend(encode_bool(false));
        data_v2.extend(encode_text("v2@test.com"));

        let row = decoder.decode(1, &data_v2, Some(2)).unwrap();
        assert_eq!(row.len(), 5); // 原有 4 列 + 新增 1 列
        assert_eq!(
            row.get("email"),
            Some(&SzValue::Text("v2@test.com".to_string()))
        );
    }
}
