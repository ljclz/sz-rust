//! SzRSQL Debezium AVRO 适配器（选）— 对应 `SzRSQL实施进度.md` Phase 2.5.5。
//!
//! 将 SzRSQL 内部的 `ChangeEvent` 转换为 AVRO 二进制格式，并实现轻量级
//! Schema Registry（注册/查询/兼容性检查）。
//!
//! # AVRO 二进制编码规范
//!
//! AVRO 使用紧凑的二进制编码，主要规则：
//!
//! - **int / long**：zigzag varint
//!   - zigzag：`((n << 1) ^ (n >> 63))`，将负数映射到正数轴
//!   - varint：每字节 7 位，最高位为 continuation flag
//! - **bytes / string**：长度前缀（zigzag varint）+ 原始字节
//! - **boolean**：1 字节（0 或 1）
//! - **null**：无数据
//! - **union**：tag（zigzag varint，0-based 索引）+ 值
//!
//! # ChangeEvent AVRO Schema
//!
//! ```json
//! {
//!   "type": "record",
//!   "name": "ChangeEvent",
//!   "namespace": "io.szrsql.cdc",
//!   "fields": [
//!     {"name": "tx_id", "type": "int"},
//!     {"name": "lsn", "type": "long"},
//!     {"name": "op", "type": "string"},
//!     {"name": "table_id", "type": ["null", "int"], "default": null},
//!     {"name": "old_row", "type": ["null", "bytes"], "default": null},
//!     {"name": "new_row", "type": ["null", "bytes"], "default": null},
//!     {"name": "timestamp", "type": "long"}
//!   ]
//! }
//! ```
//!
//! # 设计要点
//!
//! 1. **零依赖**：不引入 `apache-avro` 等重依赖，自行实现 AVRO 二进制编解码
//! 2. **Schema Registry**：内存实现，支持 register/lookup_by_subject/lookup_by_id/URL 生成
//! 3. **兼容性检查**：前向（FORWARD）+ 后向（BACKWARD）兼容性，基于字段存在性判断
//! 4. **与 Phase 2.5.4 JSON 适配器并行**：可独立使用，也可与 JSON 适配器组合

use crate::{CdcEventOp, ChangeEvent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::{Mutex, RwLock};

// =====================================================================
// AVRO 二进制编码原语
// =====================================================================

/// zigzag 编码：将 i64 映射为 u64（负数变正数轴）
///
/// - 0 → 0
/// - -1 → 1
/// - 1 → 2
/// - -2 → 3
/// - 2 → 4
fn zigzag_encode(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)) as u64
}

/// zigzag 解码：逆向映射 u64 → i64
fn zigzag_decode(n: u64) -> i64 {
    ((n >> 1) as i64) ^ -((n & 1) as i64)
}

/// 写入 varint（每字节 7 位，最高位 continuation）
fn write_varint(out: &mut Vec<u8>, mut n: u64) {
    while (n & !0x7F) != 0 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

/// 读取 varint
///
/// 返回 `None` 表示输入耗尽或溢出（>10 字节）
fn read_varint(input: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    loop {
        if *pos >= input.len() {
            return None;
        }
        let byte = input[*pos];
        *pos += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if (byte & 0x80) == 0 {
            break;
        }
        shift += 7;
        if shift > 63 {
            return None; // varint 溢出
        }
    }
    Some(result)
}

/// 写入 zigzag varint（int/long 通用）
fn write_zigzag_varint(out: &mut Vec<u8>, n: i64) {
    write_varint(out, zigzag_encode(n));
}

/// 读取 zigzag varint（int/long 通用）
fn read_zigzag_varint(input: &[u8], pos: &mut usize) -> Option<i64> {
    let n = read_varint(input, pos)?;
    Some(zigzag_decode(n))
}

/// 写入 AVRO bytes（长度前缀 + 数据）
fn write_avro_bytes(out: &mut Vec<u8>, data: &[u8]) {
    write_zigzag_varint(out, data.len() as i64);
    out.extend_from_slice(data);
}

/// 读取 AVRO bytes
fn read_avro_bytes(input: &[u8], pos: &mut usize) -> Option<Vec<u8>> {
    let len = read_zigzag_varint(input, pos)?;
    if len < 0 {
        return None;
    }
    let len = len as usize;
    if *pos + len > input.len() {
        return None;
    }
    let data = input[*pos..*pos + len].to_vec();
    *pos += len;
    Some(data)
}

/// 写入 AVRO string（UTF-8 编码的 bytes）
fn write_avro_string(out: &mut Vec<u8>, s: &str) {
    write_avro_bytes(out, s.as_bytes());
}

/// 读取 AVRO string
fn read_avro_string(input: &[u8], pos: &mut usize) -> Option<String> {
    let bytes = read_avro_bytes(input, pos)?;
    String::from_utf8(bytes).ok()
}

/// 写入 AVRO union（null | T）：tag=0 表示 null，tag=1 表示有值
fn write_avro_union_bytes(out: &mut Vec<u8>, value: Option<&[u8]>) {
    match value {
        None => write_zigzag_varint(out, 0),
        Some(data) => {
            write_zigzag_varint(out, 1);
            write_avro_bytes(out, data);
        }
    }
}

/// 读取 AVRO union（null | bytes）
fn read_avro_union_bytes(input: &[u8], pos: &mut usize) -> Option<Option<Vec<u8>>> {
    let tag = read_zigzag_varint(input, pos)?;
    match tag {
        0 => Some(None),
        1 => Some(Some(read_avro_bytes(input, pos)?)),
        _ => None,
    }
}

/// 写入 AVRO union（null | int）
fn write_avro_union_int(out: &mut Vec<u8>, value: Option<u32>) {
    match value {
        None => write_zigzag_varint(out, 0),
        Some(v) => {
            write_zigzag_varint(out, 1);
            write_zigzag_varint(out, v as i64);
        }
    }
}

/// 读取 AVRO union（null | int）
fn read_avro_union_int(input: &[u8], pos: &mut usize) -> Option<Option<u32>> {
    let tag = read_zigzag_varint(input, pos)?;
    match tag {
        0 => Some(None),
        1 => {
            let v = read_zigzag_varint(input, pos)?;
            if v < 0 || v > u32::MAX as i64 {
                return None;
            }
            Some(Some(v as u32))
        }
        _ => None,
    }
}

// =====================================================================
// ChangeEvent ↔ AVRO 二进制 转换
// =====================================================================

/// AVRO schema 字符串（ChangeEvent 的 AVRO schema 定义）
pub const CHANGE_EVENT_AVRO_SCHEMA: &str = r#"{
  "type": "record",
  "name": "ChangeEvent",
  "namespace": "io.szrsql.cdc",
  "fields": [
    {"name": "tx_id", "type": "int"},
    {"name": "lsn", "type": "long"},
    {"name": "op", "type": "string"},
    {"name": "table_id", "type": ["null", "int"], "default": null},
    {"name": "old_row", "type": ["null", "bytes"], "default": null},
    {"name": "new_row", "type": ["null", "bytes"], "default": null},
    {"name": "timestamp", "type": "long"}
  ]
}"#;

/// AVRO schema subject 名称（用于 Schema Registry 注册）
pub const CHANGE_EVENT_SUBJECT: &str = "io.szrsql.cdc.ChangeEvent";

/// 将 ChangeEvent 序列化为 AVRO 二进制
///
/// **字段顺序**（必须与 schema 一致）：
/// 1. tx_id (int)
/// 2. lsn (long)
/// 3. op (string，使用 CdcEventOp::as_str）
/// 4. table_id (union: null | int)
/// 5. old_row (union: null | bytes)
/// 6. new_row (union: null | bytes)
/// 7. timestamp (long)
pub fn to_avro(event: &ChangeEvent) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    write_zigzag_varint(&mut out, event.tx_id as i64);
    write_zigzag_varint(&mut out, event.lsn as i64);
    write_avro_string(&mut out, event.op.as_str());
    write_avro_union_int(&mut out, event.table_id);
    write_avro_union_bytes(&mut out, event.old_row.as_deref());
    write_avro_union_bytes(&mut out, event.new_row.as_deref());
    write_zigzag_varint(&mut out, event.timestamp as i64);
    out
}

/// 从 AVRO 二进制反序列化为 ChangeEvent
///
/// 返回 `None` 表示数据损坏（长度不足、tag 非法、UTF-8 解码失败等）
pub fn from_avro(bytes: &[u8]) -> Option<ChangeEvent> {
    let mut pos = 0;
    let tx_id = read_zigzag_varint(bytes, &mut pos)?;
    if tx_id < 0 || tx_id > u32::MAX as i64 {
        return None;
    }
    let tx_id = tx_id as u32;

    let lsn = read_zigzag_varint(bytes, &mut pos)?;
    if lsn < 0 {
        return None;
    }
    let lsn = lsn as u64;

    let op_str = read_avro_string(bytes, &mut pos)?;
    let op = match op_str.as_str() {
        "insert" => CdcEventOp::Insert,
        "update" => CdcEventOp::Update,
        "delete" => CdcEventOp::Delete,
        "commit" => CdcEventOp::Commit,
        "abort" => CdcEventOp::Abort,
        _ => return None,
    };

    let table_id = read_avro_union_int(bytes, &mut pos)?;
    let old_row = read_avro_union_bytes(bytes, &mut pos)?;
    let new_row = read_avro_union_bytes(bytes, &mut pos)?;

    let timestamp = read_zigzag_varint(bytes, &mut pos)?;
    if timestamp < 0 {
        return None;
    }
    let timestamp = timestamp as u64;

    // 确保所有字节都被消费（避免尾随垃圾）
    if pos != bytes.len() {
        return None;
    }

    Some(ChangeEvent {
        tx_id,
        lsn,
        op,
        table_id,
        old_row,
        new_row,
        timestamp,
        schema_version: None,
    })
}

// =====================================================================
// Schema Registry（内存实现）
// =====================================================================

/// Schema 兼容性级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompatibilityLevel {
    /// 向后兼容：旧 schema 能读取新 schema 写的数据
    Backward,
    /// 向前兼容：新 schema 能读取旧 schema 写的数据
    Forward,
    /// 双向兼容
    Full,
    /// 不检查兼容性
    None,
}

/// Schema Registry 中的 schema 条目
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaEntry {
    /// 唯一 ID（Schema Registry 分配）
    pub id: u32,
    /// Subject 名称（通常是 schema 全限定名）
    pub subject: String,
    /// 版本号（同一 subject 递增）
    pub version: u32,
    /// schema JSON 字符串
    pub schema: String,
    /// 注册时间戳（Unix 毫秒）
    pub registered_at: u64,
}

/// Schema Registry 错误
#[derive(Debug, thiserror::Error)]
pub enum SchemaRegistryError {
    /// Subject 未找到
    #[error("subject not found: {0}")]
    SubjectNotFound(String),
    /// Schema ID 未找到
    #[error("schema id not found: {0}")]
    IdNotFound(u32),
    /// 兼容性检查失败
    #[error("schema is not compatible: {0}")]
    Incompatible(String),
}

/// 内存 Schema Registry
///
/// **设计**：
/// - `by_id`：按 schema_id 查询（O(1)）
/// - `by_subject`：按 subject 查询所有版本（按 version 排序）
/// - `next_id`：原子计数器，分配唯一 ID
/// - 兼容性检查：基于字段名集合的差异判断
///
/// **API**：
/// - `register(subject, schema)` → schema_id
/// - `lookup_by_id(id)` → Option<SchemaEntry>
/// - `lookup_latest_by_subject(subject)` → Option<SchemaEntry>
/// - `lookup_by_subject_version(subject, version)` → Option<SchemaEntry>
/// - `schema_url(id)` → URL 字符串（兼容 Confluent Schema Registry URL 格式）
/// - `check_compatibility(old, new, level)` → Result<(), Error>
pub struct SchemaRegistry {
    by_id: RwLock<HashMap<u32, SchemaEntry>>,
    by_subject: RwLock<HashMap<String, Vec<SchemaEntry>>>,
    next_id: AtomicU32,
    /// Schema Registry base URL（用于生成 schema URL）
    base_url: String,
    /// 当前兼容性级别
    compatibility: Mutex<CompatibilityLevel>,
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::new("http://localhost:8081")
    }
}

impl SchemaRegistry {
    /// 创建 Schema Registry，指定 base URL
    pub fn new(base_url: &str) -> Self {
        Self {
            by_id: RwLock::new(HashMap::new()),
            by_subject: RwLock::new(HashMap::new()),
            next_id: AtomicU32::new(1),
            base_url: base_url.to_string(),
            compatibility: Mutex::new(CompatibilityLevel::Backward),
        }
    }

    /// 设置兼容性级别
    pub fn set_compatibility(&self, level: CompatibilityLevel) {
        *self.compatibility.lock() = level;
    }

    /// 获取当前兼容性级别
    pub fn compatibility(&self) -> CompatibilityLevel {
        *self.compatibility.lock()
    }

    /// 注册 schema
    ///
    /// **流程**：
    /// 1. 检查与已有最新版本的兼容性（如果存在）
    /// 2. 分配新 ID 和 version
    /// 3. 存入 by_id 和 by_subject
    ///
    /// 返回 schema_id
    pub fn register(&self, subject: &str, schema: &str) -> Result<u32, SchemaRegistryError> {
        // 检查与已有最新版本的兼容性
        if let Some(latest) = self.lookup_latest_by_subject(subject) {
            let level = self.compatibility();
            Self::check_schema_compatibility(&latest.schema, schema, level)?;
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let version = {
            let by_subject = self.by_subject.read();
            by_subject
                .get(subject)
                .map(|v| v.len() as u32 + 1)
                .unwrap_or(1)
        };

        let entry = SchemaEntry {
            id,
            subject: subject.to_string(),
            version,
            schema: schema.to_string(),
            registered_at: timestamp,
        };

        self.by_id.write().insert(id, entry.clone());
        self.by_subject
            .write()
            .entry(subject.to_string())
            .or_default()
            .push(entry);

        Ok(id)
    }

    /// 按 ID 查询 schema
    pub fn lookup_by_id(&self, id: u32) -> Option<SchemaEntry> {
        self.by_id.read().get(&id).cloned()
    }

    /// 按 subject 查询最新版本
    pub fn lookup_latest_by_subject(&self, subject: &str) -> Option<SchemaEntry> {
        let by_subject = self.by_subject.read();
        by_subject.get(subject).and_then(|v| v.last().cloned())
    }

    /// 按 subject + version 查询
    pub fn lookup_by_subject_version(&self, subject: &str, version: u32) -> Option<SchemaEntry> {
        let by_subject = self.by_subject.read();
        by_subject
            .get(subject)
            .and_then(|v| v.iter().find(|e| e.version == version).cloned())
    }

    /// 列出某 subject 的所有版本
    pub fn list_versions(&self, subject: &str) -> Vec<SchemaEntry> {
        self.by_subject
            .read()
            .get(subject)
            .cloned()
            .unwrap_or_default()
    }

    /// 生成 schema URL（兼容 Confluent Schema Registry URL 格式）
    ///
    /// 格式：`{base_url}/schemas/ids/{id}`
    pub fn schema_url(&self, id: u32) -> String {
        format!("{}/schemas/ids/{}", self.base_url, id)
    }

    /// 获取所有已注册的 subject 列表
    pub fn list_subjects(&self) -> Vec<String> {
        self.by_subject.read().keys().cloned().collect()
    }

    /// 获取已注册 schema 总数
    pub fn schema_count(&self) -> usize {
        self.by_id.read().len()
    }

    /// 检查两个 schema 的兼容性
    ///
    /// **简化规则**（基于字段名集合）：
    /// - **Backward**：新 schema 的所有必需字段（无 default）必须存在于旧 schema 中
    ///   - 即：新 schema 可以添加有 default 的字段，但不能添加无 default 的字段
    /// - **Forward**：旧 schema 的所有必需字段必须存在于新 schema 中
    ///   - 即：新 schema 可以删除字段，但不能新增无 default 的必需字段（旧消费者读不懂）
    /// - **Full**：同时满足 Backward 和 Forward
    /// - **None**：不检查
    ///
    /// **注**：本实现使用简化的字段名集合比较，不解析完整 AVRO schema 类型。
    /// 实际生产应使用 `apache-avro` crate 进行严格检查。
    pub fn check_schema_compatibility(
        old_schema: &str,
        new_schema: &str,
        level: CompatibilityLevel,
    ) -> Result<(), SchemaRegistryError> {
        if level == CompatibilityLevel::None {
            return Ok(());
        }

        let old_fields = extract_field_names(old_schema);
        let new_fields = extract_field_names(new_schema);

        // Backward：新 schema 中新增的必需字段必须有 default
        if matches!(
            level,
            CompatibilityLevel::Backward | CompatibilityLevel::Full
        ) {
            let added: Vec<_> = new_fields
                .iter()
                .filter(|(name, has_default)| {
                    !old_fields.iter().any(|(n, _)| n == name) && !*has_default
                })
                .collect();
            if !added.is_empty() {
                let names: Vec<_> = added.iter().map(|(n, _)| n.as_str()).collect();
                return Err(SchemaRegistryError::Incompatible(format!(
                    "Backward incompatible: new required fields without default: {}",
                    names.join(", ")
                )));
            }
        }

        // Forward：旧 schema 中被删除的必需字段
        if matches!(
            level,
            CompatibilityLevel::Forward | CompatibilityLevel::Full
        ) {
            let removed: Vec<_> = old_fields
                .iter()
                .filter(|(name, has_default)| {
                    !new_fields.iter().any(|(n, _)| n == name) && !*has_default
                })
                .collect();
            if !removed.is_empty() {
                let names: Vec<_> = removed.iter().map(|(n, _)| n.as_str()).collect();
                return Err(SchemaRegistryError::Incompatible(format!(
                    "Forward incompatible: removed required fields: {}",
                    names.join(", ")
                )));
            }
        }

        Ok(())
    }
}

/// 从 AVRO schema JSON 中提取字段名和是否有 default
///
/// **实现**：使用 `serde_json` 解析 schema，从 `"fields"` 数组中提取每个字段的
/// `name`（必需）和是否存在 `default` 键。返回 `Vec<(field_name, has_default)>`，
/// 按字段在 schema 中的声明顺序。
///
/// **注**：只提取 `fields` 数组内的字段，不会误匹配 record 级别的 `name`（如
/// `"name": "ChangeEvent"`）。若 schema 不是合法 JSON 或不含 `fields` 数组，
/// 返回空 Vec。
fn extract_field_names(schema: &str) -> Vec<(String, bool)> {
    let parsed: serde_json::Value = match serde_json::from_str(schema) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let fields_arr = match parsed.get("fields").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    fields_arr
        .iter()
        .filter_map(|field| {
            let name = field.get("name")?.as_str()?.to_string();
            let has_default = field.get("default").is_some();
            Some((name, has_default))
        })
        .collect()
}

// =====================================================================
// 注册 ChangeEvent 默认 schema 的便捷方法
// =====================================================================

impl SchemaRegistry {
    /// 注册 ChangeEvent 的默认 AVRO schema，返回 schema_id
    pub fn register_change_event_schema(&self) -> Result<u32, SchemaRegistryError> {
        self.register(CHANGE_EVENT_SUBJECT, CHANGE_EVENT_AVRO_SCHEMA)
    }

    /// 查询 ChangeEvent schema URL
    pub fn change_event_schema_url(&self) -> Option<String> {
        let entry = self.lookup_latest_by_subject(CHANGE_EVENT_SUBJECT)?;
        Some(self.schema_url(entry.id))
    }
}

// =====================================================================
// AVRO 编码辅助：将 schema_id 前缀加到 AVRO 数据（兼容 Confluent Wire Format）
// =====================================================================

/// Confluent AVRO wire format magic byte
pub const CONFLUENT_MAGIC_BYTE: u8 = 0x00;

/// 将 ChangeEvent 编码为 Confluent AVRO wire format
///
/// **格式**：magic byte (0x00) + schema_id (4 bytes big-endian) + AVRO binary
///
/// 这样消费者可以从 wire format 中提取 schema_id，从 Schema Registry 查询 schema，再解码。
pub fn to_confluent_avro(event: &ChangeEvent, schema_id: u32) -> Vec<u8> {
    let avro = to_avro(event);
    let mut out = Vec::with_capacity(5 + avro.len());
    out.push(CONFLUENT_MAGIC_BYTE);
    out.extend_from_slice(&schema_id.to_be_bytes());
    out.extend_from_slice(&avro);
    out
}

/// 从 Confluent AVRO wire format 解码 ChangeEvent
///
/// 返回 `(schema_id, ChangeEvent)` 或 `None`（格式错误）
pub fn from_confluent_avro(bytes: &[u8]) -> Option<(u32, ChangeEvent)> {
    if bytes.len() < 5 {
        return None;
    }
    if bytes[0] != CONFLUENT_MAGIC_BYTE {
        return None;
    }
    let schema_id = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    let event = from_avro(&bytes[5..])?;
    Some((schema_id, event))
}

// =====================================================================
// 共享 SchemaRegistry（便于测试）
// =====================================================================

/// 创建一个共享的 SchemaRegistry，已注册 ChangeEvent schema
pub fn shared_registry_with_change_event() -> Arc<SchemaRegistry> {
    let registry = Arc::new(SchemaRegistry::default());
    registry
        .register_change_event_schema()
        .expect("register change event schema");
    registry
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =================================================================
    // Phase 2.5.5: Debezium AVRO 适配器测试
    // =================================================================

    mod phase_2_5_5 {
        use super::*;

        // -----------------------------------------------------------------
        // 1. AVRO 二进制编码原语
        // -----------------------------------------------------------------

        #[test]
        fn zigzag_encode_decode_roundtrip() {
            for n in [
                0i64,
                -1,
                1,
                -2,
                2,
                -100,
                100,
                i64::MAX,
                i64::MIN,
                12345,
                -98765,
            ] {
                let encoded = zigzag_encode(n);
                let decoded = zigzag_decode(encoded);
                assert_eq!(decoded, n, "zigzag roundtrip failed for {}", n);
            }
        }

        #[test]
        fn zigzag_encode_known_values() {
            // AVRO spec 已知值
            assert_eq!(zigzag_encode(0), 0);
            assert_eq!(zigzag_encode(-1), 1);
            assert_eq!(zigzag_encode(1), 2);
            assert_eq!(zigzag_encode(-2), 3);
            assert_eq!(zigzag_encode(2), 4);
        }

        #[test]
        fn varint_encode_decode_roundtrip() {
            for n in [
                0u64,
                1,
                127,
                128,
                255,
                256,
                16384,
                65535,
                4294967295,
                u64::MAX,
            ] {
                let mut out = Vec::new();
                write_varint(&mut out, n);
                let mut pos = 0;
                let decoded = read_varint(&out, &mut pos).unwrap();
                assert_eq!(decoded, n, "varint roundtrip failed for {}", n);
                assert_eq!(pos, out.len(), "varint not fully consumed");
            }
        }

        #[test]
        fn varint_single_byte_for_small_values() {
            let mut out = Vec::new();
            write_varint(&mut out, 100);
            assert_eq!(out.len(), 1);
            assert_eq!(out[0], 100);
        }

        #[test]
        fn varint_multi_byte_for_large_values() {
            let mut out = Vec::new();
            write_varint(&mut out, 300);
            assert!(out.len() >= 2);
            let mut pos = 0;
            assert_eq!(read_varint(&out, &mut pos).unwrap(), 300);
        }

        #[test]
        fn varint_read_empty_returns_none() {
            let empty: Vec<u8> = vec![];
            let mut pos = 0;
            assert!(read_varint(&empty, &mut pos).is_none());
        }

        #[test]
        fn avro_bytes_roundtrip() {
            for data in [vec![], vec![1], vec![1, 2, 3], vec![0; 100], vec![255; 50]] {
                let mut out = Vec::new();
                write_avro_bytes(&mut out, &data);
                let mut pos = 0;
                let decoded = read_avro_bytes(&out, &mut pos).unwrap();
                assert_eq!(decoded, data);
                assert_eq!(pos, out.len());
            }
        }

        #[test]
        fn avro_string_roundtrip() {
            for s in ["", "hello", "你好", "🦀", "multi\nline\nstring"] {
                let mut out = Vec::new();
                write_avro_string(&mut out, s);
                let mut pos = 0;
                let decoded = read_avro_string(&out, &mut pos).unwrap();
                assert_eq!(decoded, s);
            }
        }

        #[test]
        fn avro_union_bytes_roundtrip() {
            // None
            let mut out = Vec::new();
            write_avro_union_bytes(&mut out, None);
            let mut pos = 0;
            assert_eq!(read_avro_union_bytes(&out, &mut pos).unwrap(), None);

            // Some
            let data = vec![1, 2, 3, 4, 5];
            let mut out = Vec::new();
            write_avro_union_bytes(&mut out, Some(&data));
            let mut pos = 0;
            assert_eq!(
                read_avro_union_bytes(&out, &mut pos).unwrap(),
                Some(data.clone())
            );
        }

        #[test]
        fn avro_union_int_roundtrip() {
            // None
            let mut out = Vec::new();
            write_avro_union_int(&mut out, None);
            let mut pos = 0;
            assert_eq!(read_avro_union_int(&out, &mut pos).unwrap(), None);

            // Some
            let mut out = Vec::new();
            write_avro_union_int(&mut out, Some(42));
            let mut pos = 0;
            assert_eq!(read_avro_union_int(&out, &mut pos).unwrap(), Some(42));
        }

        // -----------------------------------------------------------------
        // 2. ChangeEvent ↔ AVRO 二进制转换
        // -----------------------------------------------------------------

        #[test]
        fn to_avro_insert_roundtrip() {
            let original = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            let bytes = to_avro(&original);
            let restored = from_avro(&bytes).unwrap();

            assert_eq!(restored.op, CdcEventOp::Insert);
            assert_eq!(restored.tx_id, 1);
            assert_eq!(restored.lsn, 100);
            assert_eq!(restored.table_id, Some(42));
            assert_eq!(restored.old_row, None);
            assert_eq!(restored.new_row, Some(vec![1, 2, 3]));
            assert_eq!(restored.timestamp, 12345);
        }

        #[test]
        fn to_avro_update_roundtrip() {
            let original = ChangeEvent::update(7, 999, 88, vec![10, 20], vec![30, 40], 99999);
            let bytes = to_avro(&original);
            let restored = from_avro(&bytes).unwrap();

            assert_eq!(restored.op, CdcEventOp::Update);
            assert_eq!(restored.tx_id, 7);
            assert_eq!(restored.lsn, 999);
            assert_eq!(restored.table_id, Some(88));
            assert_eq!(restored.old_row, Some(vec![10, 20]));
            assert_eq!(restored.new_row, Some(vec![30, 40]));
            assert_eq!(restored.timestamp, 99999);
        }

        #[test]
        fn to_avro_delete_roundtrip() {
            let original = ChangeEvent::delete(3, 300, 77, vec![5, 6, 7], 55555);
            let bytes = to_avro(&original);
            let restored = from_avro(&bytes).unwrap();

            assert_eq!(restored.op, CdcEventOp::Delete);
            assert_eq!(restored.tx_id, 3);
            assert_eq!(restored.lsn, 300);
            assert_eq!(restored.table_id, Some(77));
            assert_eq!(restored.old_row, Some(vec![5, 6, 7]));
            assert_eq!(restored.new_row, None);
            assert_eq!(restored.timestamp, 55555);
        }

        #[test]
        fn to_avro_commit_roundtrip() {
            let original = ChangeEvent::commit(4, 400, 77777);
            let bytes = to_avro(&original);
            let restored = from_avro(&bytes).unwrap();

            assert_eq!(restored.op, CdcEventOp::Commit);
            assert_eq!(restored.tx_id, 4);
            assert_eq!(restored.lsn, 400);
            assert_eq!(restored.table_id, None);
            assert_eq!(restored.old_row, None);
            assert_eq!(restored.new_row, None);
            assert_eq!(restored.timestamp, 77777);
        }

        #[test]
        fn to_avro_abort_roundtrip() {
            let original = ChangeEvent::abort(5, 500, 33333);
            let bytes = to_avro(&original);
            let restored = from_avro(&bytes).unwrap();

            assert_eq!(restored.op, CdcEventOp::Abort);
            assert_eq!(restored.tx_id, 5);
            assert_eq!(restored.lsn, 500);
            assert_eq!(restored.timestamp, 33333);
        }

        #[test]
        fn to_avro_compact_size_for_small_event() {
            // Commit 事件：tx_id(1) + lsn(1) + op("commit"=7 bytes) + table_id(1) + old_row(1) + new_row(1) + timestamp(1)
            // = 1 + 1 + (1+6) + 1 + 1 + 1 + 1 = 13 bytes
            let event = ChangeEvent::commit(1, 1, 1);
            let bytes = to_avro(&event);
            assert!(
                bytes.len() < 20,
                "commit event should be compact, got {} bytes",
                bytes.len()
            );
        }

        #[test]
        fn to_avro_large_event_roundtrip() {
            let large_row = vec![0xAB; 10000];
            let original = ChangeEvent::insert(99999, 888888, 12345, large_row.clone(), 9999999);
            let bytes = to_avro(&original);
            let restored = from_avro(&bytes).unwrap();
            assert_eq!(restored, original);
        }

        // -----------------------------------------------------------------
        // 3. AVRO 解码错误处理
        // -----------------------------------------------------------------

        #[test]
        fn from_avro_empty_bytes_returns_none() {
            assert!(from_avro(&[]).is_none());
        }

        #[test]
        fn from_avro_truncated_bytes_returns_none() {
            let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            let bytes = to_avro(&event);
            // 截断最后几个字节
            let truncated = &bytes[..bytes.len() - 3];
            assert!(from_avro(truncated).is_none());
        }

        #[test]
        fn from_avro_invalid_op_returns_none() {
            // 手工构造一个 op 为非法字符串的事件
            let mut out = Vec::new();
            write_zigzag_varint(&mut out, 1); // tx_id
            write_zigzag_varint(&mut out, 100); // lsn
            write_avro_string(&mut out, "invalid_op"); // 非法 op
            write_avro_union_int(&mut out, Some(42)); // table_id
            write_avro_union_bytes(&mut out, None); // old_row
            write_avro_union_bytes(&mut out, Some(&[1, 2, 3])); // new_row
            write_zigzag_varint(&mut out, 12345); // timestamp
            assert!(from_avro(&out).is_none());
        }

        #[test]
        fn from_avro_trailing_garbage_returns_none() {
            let event = ChangeEvent::commit(1, 1, 1);
            let mut bytes = to_avro(&event);
            bytes.push(0xFF); // 尾随垃圾
            assert!(from_avro(&bytes).is_none());
        }

        // -----------------------------------------------------------------
        // 4. Schema Registry 基础操作
        // -----------------------------------------------------------------

        #[test]
        fn schema_registry_new_is_empty() {
            let registry = SchemaRegistry::default();
            assert_eq!(registry.schema_count(), 0);
            assert!(registry.list_subjects().is_empty());
        }

        #[test]
        fn schema_registry_register_returns_id() {
            let registry = SchemaRegistry::default();
            let id1 = registry.register("test.subject", "{}").unwrap();
            let id2 = registry.register("test.subject", "{}").unwrap();
            assert_eq!(id1, 1);
            assert_eq!(id2, 2);
        }

        #[test]
        fn schema_registry_lookup_by_id() {
            let registry = SchemaRegistry::default();
            let id = registry
                .register("test.subject", r#"{"type":"record"}"#)
                .unwrap();
            let entry = registry.lookup_by_id(id).unwrap();
            assert_eq!(entry.id, id);
            assert_eq!(entry.subject, "test.subject");
            assert_eq!(entry.schema, r#"{"type":"record"}"#);
            assert_eq!(entry.version, 1);
        }

        #[test]
        fn schema_registry_lookup_by_id_not_found() {
            let registry = SchemaRegistry::default();
            assert!(registry.lookup_by_id(999).is_none());
        }

        #[test]
        fn schema_registry_lookup_latest_by_subject() {
            let registry = SchemaRegistry::default();
            registry.register("test.subject", "v1").unwrap();
            let id2 = registry.register("test.subject", "v2").unwrap();

            let latest = registry.lookup_latest_by_subject("test.subject").unwrap();
            assert_eq!(latest.id, id2);
            assert_eq!(latest.version, 2);
            assert_eq!(latest.schema, "v2");
        }

        #[test]
        fn schema_registry_lookup_latest_by_subject_not_found() {
            let registry = SchemaRegistry::default();
            assert!(registry.lookup_latest_by_subject("nonexistent").is_none());
        }

        #[test]
        fn schema_registry_lookup_by_subject_version() {
            let registry = SchemaRegistry::default();
            registry.register("test.subject", "v1").unwrap();
            registry.register("test.subject", "v2").unwrap();

            let v1 = registry
                .lookup_by_subject_version("test.subject", 1)
                .unwrap();
            assert_eq!(v1.schema, "v1");
            assert_eq!(v1.version, 1);

            let v2 = registry
                .lookup_by_subject_version("test.subject", 2)
                .unwrap();
            assert_eq!(v2.schema, "v2");
            assert_eq!(v2.version, 2);

            assert!(registry
                .lookup_by_subject_version("test.subject", 99)
                .is_none());
        }

        #[test]
        fn schema_registry_list_versions() {
            let registry = SchemaRegistry::default();
            registry.register("test.subject", "v1").unwrap();
            registry.register("test.subject", "v2").unwrap();
            registry.register("test.subject", "v3").unwrap();

            let versions = registry.list_versions("test.subject");
            assert_eq!(versions.len(), 3);
            assert_eq!(versions[0].version, 1);
            assert_eq!(versions[1].version, 2);
            assert_eq!(versions[2].version, 3);
        }

        #[test]
        fn schema_registry_list_subjects() {
            let registry = SchemaRegistry::default();
            registry.register("subject.a", "schema").unwrap();
            registry.register("subject.b", "schema").unwrap();

            let mut subjects = registry.list_subjects();
            subjects.sort();
            assert_eq!(subjects, vec!["subject.a", "subject.b"]);
        }

        #[test]
        fn schema_registry_schema_url() {
            let registry = SchemaRegistry::new("http://registry:8081");
            let url = registry.schema_url(42);
            assert_eq!(url, "http://registry:8081/schemas/ids/42");
        }

        // -----------------------------------------------------------------
        // 5. ChangeEvent schema 便捷注册
        // -----------------------------------------------------------------

        #[test]
        fn register_change_event_schema_returns_id() {
            let registry = SchemaRegistry::default();
            let id = registry.register_change_event_schema().unwrap();
            assert_eq!(id, 1);

            let entry = registry.lookup_by_id(id).unwrap();
            assert_eq!(entry.subject, CHANGE_EVENT_SUBJECT);
            assert_eq!(entry.schema, CHANGE_EVENT_AVRO_SCHEMA);
        }

        #[test]
        fn change_event_schema_url_after_register() {
            let registry = SchemaRegistry::default();
            registry.register_change_event_schema().unwrap();
            let url = registry.change_event_schema_url().unwrap();
            assert!(url.contains("/schemas/ids/1"));
        }

        #[test]
        fn shared_registry_with_change_event_prepopulated() {
            let registry = shared_registry_with_change_event();
            assert_eq!(registry.schema_count(), 1);
            assert!(registry.change_event_schema_url().is_some());
        }

        // -----------------------------------------------------------------
        // 6. Confluent AVRO wire format
        // -----------------------------------------------------------------

        #[test]
        fn confluent_avro_roundtrip_insert() {
            let original = ChangeEvent::insert(7, 999, 42, vec![1, 2, 3], 12345);
            let wire = to_confluent_avro(&original, 42);

            // 验证格式：magic byte + 4 bytes schema_id + AVRO payload
            assert_eq!(wire[0], CONFLUENT_MAGIC_BYTE);
            let schema_id = u32::from_be_bytes([wire[1], wire[2], wire[3], wire[4]]);
            assert_eq!(schema_id, 42);

            let (restored_id, restored_event) = from_confluent_avro(&wire).unwrap();
            assert_eq!(restored_id, 42);
            assert_eq!(restored_event, original);
        }

        #[test]
        fn confluent_avro_roundtrip_all_ops() {
            let events = [
                ChangeEvent::insert(1, 100, 42, vec![1], 1000),
                ChangeEvent::update(2, 200, 99, vec![1], vec![2], 2000),
                ChangeEvent::delete(3, 300, 88, vec![5], 3000),
                ChangeEvent::commit(4, 400, 4000),
                ChangeEvent::abort(5, 500, 5000),
            ];

            for (i, original) in events.iter().enumerate() {
                let schema_id = (i + 1) as u32;
                let wire = to_confluent_avro(original, schema_id);
                let (restored_id, restored_event) = from_confluent_avro(&wire).unwrap();
                assert_eq!(restored_id, schema_id);
                assert_eq!(restored_event, *original);
            }
        }

        #[test]
        fn confluent_avro_invalid_magic_byte_returns_none() {
            let event = ChangeEvent::commit(1, 1, 1);
            let wire = to_confluent_avro(&event, 1);
            let mut bad = wire.clone();
            bad[0] = 0xFF; // 错误的 magic byte
            assert!(from_confluent_avro(&bad).is_none());
        }

        #[test]
        fn confluent_avro_too_short_returns_none() {
            assert!(from_confluent_avro(&[]).is_none());
            assert!(from_confluent_avro(&[0x00]).is_none());
            assert!(from_confluent_avro(&[0x00, 0x01, 0x02, 0x03]).is_none());
        }

        // -----------------------------------------------------------------
        // 7. 兼容性检查
        // -----------------------------------------------------------------

        #[test]
        fn compatibility_none_always_passes() {
            let old_schema = r#"{"fields":[{"name":"a"}]}"#;
            let new_schema = r#"{"fields":[{"name":"b"}]}"#;
            assert!(SchemaRegistry::check_schema_compatibility(
                old_schema,
                new_schema,
                CompatibilityLevel::None
            )
            .is_ok());
        }

        #[test]
        fn compatibility_backward_adding_field_with_default_ok() {
            let old_schema = r#"{"fields":[{"name":"a"}]}"#;
            // 新增字段 b 但有 default
            let new_schema = r#"{"fields":[{"name":"a"},{"name":"b","default":"x"}]}"#;
            assert!(SchemaRegistry::check_schema_compatibility(
                old_schema,
                new_schema,
                CompatibilityLevel::Backward
            )
            .is_ok());
        }

        #[test]
        fn compatibility_backward_adding_required_field_fails() {
            let old_schema = r#"{"fields":[{"name":"a"}]}"#;
            // 新增必需字段 b（无 default）
            let new_schema = r#"{"fields":[{"name":"a"},{"name":"b"}]}"#;
            assert!(SchemaRegistry::check_schema_compatibility(
                old_schema,
                new_schema,
                CompatibilityLevel::Backward
            )
            .is_err());
        }

        #[test]
        fn compatibility_forward_removing_required_field_fails() {
            let old_schema = r#"{"fields":[{"name":"a"},{"name":"b"}]}"#;
            // 删除必需字段 b
            let new_schema = r#"{"fields":[{"name":"a"}]}"#;
            assert!(SchemaRegistry::check_schema_compatibility(
                old_schema,
                new_schema,
                CompatibilityLevel::Forward
            )
            .is_err());
        }

        #[test]
        fn compatibility_forward_removing_optional_field_ok() {
            let old_schema = r#"{"fields":[{"name":"a"},{"name":"b","default":"x"}]}"#;
            // 删除有 default 的字段 b（旧消费者还能读新数据，因为 b 缺失可用 default）
            let new_schema = r#"{"fields":[{"name":"a"}]}"#;
            assert!(SchemaRegistry::check_schema_compatibility(
                old_schema,
                new_schema,
                CompatibilityLevel::Forward
            )
            .is_ok());
        }

        #[test]
        fn compatibility_full_both_directions() {
            let old_schema = r#"{"fields":[{"name":"a"}]}"#;
            // 新增有 default 的字段：Backward OK，Forward OK（旧 schema 字段都在新 schema 中）
            let new_schema = r#"{"fields":[{"name":"a"},{"name":"b","default":"x"}]}"#;
            assert!(SchemaRegistry::check_schema_compatibility(
                old_schema,
                new_schema,
                CompatibilityLevel::Full
            )
            .is_ok());
        }

        #[test]
        fn compatibility_full_fails_on_new_required_field() {
            let old_schema = r#"{"fields":[{"name":"a"}]}"#;
            let new_schema = r#"{"fields":[{"name":"a"},{"name":"b"}]}"#;
            assert!(SchemaRegistry::check_schema_compatibility(
                old_schema,
                new_schema,
                CompatibilityLevel::Full
            )
            .is_err());
        }

        #[test]
        fn schema_registry_register_incompatible_rejected() {
            let registry = SchemaRegistry::default();
            registry.set_compatibility(CompatibilityLevel::Backward);

            let v1 = r#"{"fields":[{"name":"a"}]}"#;
            registry.register("test.subject", v1).unwrap();

            // 新增必需字段 b（无 default）应被拒绝
            let v2 = r#"{"fields":[{"name":"a"},{"name":"b"}]}"#;
            let result = registry.register("test.subject", v2);
            assert!(result.is_err());
        }

        #[test]
        fn schema_registry_register_compatible_accepted() {
            let registry = SchemaRegistry::default();
            registry.set_compatibility(CompatibilityLevel::Backward);

            let v1 = r#"{"fields":[{"name":"a"}]}"#;
            let id1 = registry.register("test.subject", v1).unwrap();

            // 新增有 default 的字段 b 应被接受
            let v2 = r#"{"fields":[{"name":"a"},{"name":"b","default":"x"}]}"#;
            let id2 = registry.register("test.subject", v2).unwrap();

            assert!(id2 > id1);
            assert_eq!(
                registry
                    .lookup_latest_by_subject("test.subject")
                    .unwrap()
                    .version,
                2
            );
        }

        // -----------------------------------------------------------------
        // 8. 端到端：ChangeEvent → AVRO → Schema Registry
        // -----------------------------------------------------------------

        #[test]
        fn end_to_end_change_event_via_avro_with_registry() {
            // 1. 创建 SchemaRegistry，注册 ChangeEvent schema
            let registry = shared_registry_with_change_event();
            let schema_entry = registry
                .lookup_latest_by_subject(CHANGE_EVENT_SUBJECT)
                .unwrap();

            // 2. 编码 ChangeEvent 为 Confluent AVRO wire format
            let original = ChangeEvent::insert(42, 1000, 7, vec![0xCA, 0xFE], 99999);
            let wire = to_confluent_avro(&original, schema_entry.id);

            // 3. 解码：从 wire format 提取 schema_id，验证 schema 存在，反序列化事件
            let (extracted_id, restored) = from_confluent_avro(&wire).unwrap();
            assert_eq!(extracted_id, schema_entry.id);

            // 4. 通过 schema_id 查询 schema（模拟消费者行为）
            let looked_up = registry.lookup_by_id(extracted_id).unwrap();
            assert_eq!(looked_up.schema, CHANGE_EVENT_AVRO_SCHEMA);

            // 5. 验证事件内容
            assert_eq!(restored, original);
        }

        #[test]
        fn end_to_end_batch_events_via_avro() {
            let registry = shared_registry_with_change_event();
            let schema_id = registry
                .lookup_latest_by_subject(CHANGE_EVENT_SUBJECT)
                .unwrap()
                .id;

            let events = [
                ChangeEvent::insert(1, 100, 42, vec![1], 1000),
                ChangeEvent::update(1, 101, 42, vec![1], vec![2], 1001),
                ChangeEvent::delete(1, 102, 42, vec![2], 1002),
                ChangeEvent::commit(1, 103, 1003),
                ChangeEvent::abort(2, 200, 2000),
            ];

            // 批量编码
            let wires: Vec<Vec<u8>> = events
                .iter()
                .map(|e| to_confluent_avro(e, schema_id))
                .collect();

            // 批量解码
            let restored: Vec<ChangeEvent> = wires
                .iter()
                .filter_map(|w| from_confluent_avro(w).map(|(_, e)| e))
                .collect();

            assert_eq!(restored.len(), events.len());
            for (orig, rest) in events.iter().zip(restored.iter()) {
                assert_eq!(rest, orig);
            }
        }

        // -----------------------------------------------------------------
        // 9. 不变量：AVRO 编码与 JSON 编码语义一致
        // -----------------------------------------------------------------

        #[test]
        fn invariant_avro_preserves_all_fields() {
            let events = [
                ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 1000),
                ChangeEvent::update(2, 200, 99, vec![10], vec![20], 2000),
                ChangeEvent::delete(3, 300, 88, vec![5, 6, 7], 3000),
                ChangeEvent::commit(4, 400, 4000),
                ChangeEvent::abort(5, 500, 5000),
            ];

            for original in &events {
                let bytes = to_avro(original);
                let restored = from_avro(&bytes).unwrap();
                assert_eq!(
                    restored, *original,
                    "AVRO roundtrip failed for {:?}",
                    original.op
                );
            }
        }

        #[test]
        fn invariant_avro_idempotent() {
            // 多次编码同一事件应产生相同的字节序列
            let event = ChangeEvent::insert(42, 1000, 7, vec![0xCA, 0xFE], 99999);
            let bytes1 = to_avro(&event);
            let bytes2 = to_avro(&event);
            assert_eq!(bytes1, bytes2);
        }

        #[test]
        fn invariant_avro_schema_well_formed_json() {
            // CHANGE_EVENT_AVRO_SCHEMA 应是合法 JSON
            let parsed: serde_json::Value = serde_json::from_str(CHANGE_EVENT_AVRO_SCHEMA).unwrap();
            assert_eq!(parsed["type"], "record");
            assert_eq!(parsed["name"], "ChangeEvent");
            assert_eq!(parsed["namespace"], "io.szrsql.cdc");
            assert!(parsed["fields"].is_array());
            assert_eq!(parsed["fields"].as_array().unwrap().len(), 7);
        }

        // -----------------------------------------------------------------
        // 10. extract_field_names 辅助函数测试
        // -----------------------------------------------------------------

        #[test]
        fn extract_field_names_simple() {
            let schema = r#"{"fields":[{"name":"a"},{"name":"b"}]}"#;
            let fields = extract_field_names(schema);
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0], ("a".to_string(), false));
            assert_eq!(fields[1], ("b".to_string(), false));
        }

        #[test]
        fn extract_field_names_with_default() {
            let schema = r#"{"fields":[{"name":"a"},{"name":"b","default":"x"}]}"#;
            let fields = extract_field_names(schema);
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0], ("a".to_string(), false));
            assert_eq!(fields[1], ("b".to_string(), true));
        }

        #[test]
        fn extract_field_names_change_event_schema() {
            let fields = extract_field_names(CHANGE_EVENT_AVRO_SCHEMA);
            assert_eq!(fields.len(), 7);
            let names: Vec<_> = fields.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(
                names,
                vec![
                    "tx_id",
                    "lsn",
                    "op",
                    "table_id",
                    "old_row",
                    "new_row",
                    "timestamp",
                ]
            );
            // table_id/old_row/new_row 有 default
            assert!(fields[3].1, "table_id should have default");
            assert!(fields[4].1, "old_row should have default");
            assert!(fields[5].1, "new_row should have default");
        }

        // -----------------------------------------------------------------
        // 11. 性能验证：AVRO 比 JSON 紧凑
        // -----------------------------------------------------------------

        #[test]
        fn avro_more_compact_than_json_for_small_events() {
            let event = ChangeEvent::commit(1, 1, 1);
            let avro_bytes = to_avro(&event);
            let json_bytes = serde_json::to_vec(&event).unwrap();
            assert!(
                avro_bytes.len() < json_bytes.len(),
                "AVRO ({} bytes) should be smaller than JSON ({} bytes) for small events",
                avro_bytes.len(),
                json_bytes.len()
            );
        }

        #[test]
        fn avro_more_compact_than_json_for_large_events() {
            let large_row = vec![0xAB; 1000];
            let event = ChangeEvent::insert(1, 100, 42, large_row.clone(), 99999);
            let avro_bytes = to_avro(&event);
            let json_bytes = serde_json::to_vec(&event).unwrap();
            assert!(
                avro_bytes.len() < json_bytes.len(),
                "AVRO ({} bytes) should be smaller than JSON ({} bytes) for large events",
                avro_bytes.len(),
                json_bytes.len()
            );
        }
    }
}
