//! 数据比对 — 行级数据一致性比对
//!
//! # 设计要点
//!
//! 1. **行级比对**：按主键逐行比对源端和目标端数据，找出差异
//! 2. **批量扫描**：分批读取两端数据，避免内存溢出
//! 3. **三类差异**：
//!    - `SourceOnly`：源端有，目标端无（缺失行）
//!    - `TargetOnly`：目标端有，源端无（多余行）
//!    - `ContentMismatch`：两端都有但内容不同（不一致）
//! 4. **Checksum 优化**：先比对行级 checksum（MD5/XXHash），checksum 一致则跳过逐字段比对
//! 5. **进度报告**：实时报告已比对行数、差异行数、预估进度
//!
//! # 流程
//!
//! ```text
//! 1. 获取两端表 schema（主键列、所有列）
//! 2. 按 pk 升序分批读取两端数据
//! 3. 双指针合并比对：
//!    - pk 相等：比对内容 → 相同 / ContentMismatch
//!    - 源端 pk < 目标端 pk：SourceOnly（缺失）
//!    - 源端 pk > 目标端 pk：TargetOnly（多余）
//! 4. 记录差异，输出比对报告
//! ```

use crate::decoder::DecodedRow;
use crate::schema::TableSchema;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use szrsql_types::value::Value as SzValue;

// =====================================================================
// 行 checksum 计算（基于 FNV-1a 64bit，无外部依赖）
// =====================================================================

/// 计算 DecodedRow 的 checksum（FNV-1a 64bit）
///
/// **设计**：
/// - 对每列 (name, value) 依次哈希
/// - 列顺序固定（schema 顺序），保证源端和目标端计算结果一致
/// - NULL、整数、浮点、字符串、字节数组等都有明确的哈希方式
///
/// **用途**：在双指针比对 pk 相等时，先比对 checksum，若一致则跳过逐字段比对，
/// 显著减少 CPU 开销（典型场景下可减少 80%+ 的字段比较）。
pub fn row_checksum(row: &DecodedRow) -> u64 {
    let mut hasher = Fnv1a64::default();
    for (name, value) in &row.columns {
        name.hash(&mut hasher);
        hash_value(value, &mut hasher);
    }
    hasher.finish()
}

/// 将 SzValue 哈希到 hasher
fn hash_value<H: Hasher>(value: &SzValue, hasher: &mut H) {
    // 用 discriminant 区分类型，避免 Int64(1) 和 Float64(1.0) 哈希冲突
    std::mem::discriminant(value).hash(hasher);
    match value {
        SzValue::Null => {}
        SzValue::Int64(v) => v.hash(hasher),
        SzValue::Float64(v) => v.to_bits().hash(hasher),
        SzValue::Text(s) => s.hash(hasher),
        SzValue::Blob(b) => b.hash(hasher),
        SzValue::Bool(b) => b.hash(hasher),
        SzValue::Date(d) => d.hash(hasher),
        SzValue::Timestamp(t) => t.hash(hasher),
        SzValue::Decimal(unscaled, scale) => {
            unscaled.hash(hasher);
            scale.hash(hasher);
        }
        SzValue::Array(arr) => {
            for v in arr {
                hash_value(v, hasher);
            }
        }
        SzValue::Enum(s) => s.hash(hasher),
        SzValue::Range(r) => {
            r.lower_inc.hash(hasher);
            r.upper_inc.hash(hasher);
            r.range_type.hash(hasher);
            if let Some(b) = &r.lower {
                hash_value(b, hasher);
            }
            if let Some(b) = &r.upper {
                hash_value(b, hasher);
            }
        }
        SzValue::Json(v) => {
            // JSON 值按规范化字符串哈希
            if let Ok(s) = serde_json::to_string(v) {
                s.hash(hasher);
            }
        }
        SzValue::TsVector(t) => {
            for lex in &t.lexemes {
                lex.term.hash(hasher);
            }
        }
        SzValue::TsQuery(_) => {
            // TsQuery 哈希占位（实际使用少）
            0u8.hash(hasher);
        }
    }
}

/// FNV-1a 64bit 哈希实现（无外部依赖）
struct Fnv1a64 {
    state: u64,
}

impl Fnv1a64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    fn new() -> Self {
        Self {
            state: Self::OFFSET_BASIS,
        }
    }
}

impl Default for Fnv1a64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher for Fnv1a64 {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state ^= b as u64;
            self.state = self.state.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(&self) -> u64 {
        self.state
    }
}

// =====================================================================
// ComparisonError — 比对错误
// =====================================================================

#[derive(Debug, thiserror::Error)]
pub enum ComparisonError {
    #[error("source read error: {0}")]
    SourceRead(String),
    #[error("target read error: {0}")]
    TargetRead(String),
    #[error("schema mismatch: {0}")]
    SchemaMismatch(String),
    #[error("primary key not found in row")]
    PkNotFound,
    #[error("internal error: {0}")]
    Internal(String),
}

// =====================================================================
// RowDifference — 行差异类型
// =====================================================================

/// 行差异类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// 源端有，目标端无
    SourceOnly,
    /// 目标端有，源端无
    TargetOnly,
    /// 两端都有但内容不同
    ContentMismatch,
}

/// 单行差异
#[derive(Debug, Clone)]
pub struct RowDifference {
    /// 差异类型
    pub kind: DiffKind,
    /// 主键值（字符串形式，便于日志）
    pub pk_value: String,
    /// 源端行（SourceOnly / ContentMismatch 时有值）
    pub source_row: Option<DecodedRow>,
    /// 目标端行（TargetOnly / ContentMismatch 时有值）
    pub target_row: Option<DecodedRow>,
    /// 不一致的字段列表（仅 ContentMismatch 时有值）
    pub mismatched_columns: Vec<String>,
}

// =====================================================================
// TableComparisonResult — 单表比对结果
// =====================================================================

/// 单表比对结果
#[derive(Debug, Clone)]
pub struct TableComparisonResult {
    /// 表名
    pub table_name: String,
    /// 源端行数
    pub source_rows: u64,
    /// 目标端行数
    pub target_rows: u64,
    /// 已比对行数
    pub compared_rows: u64,
    /// 差异数量
    pub diff_count: u64,
    /// 差异列表（限制大小，前 N 条）
    pub differences: Vec<RowDifference>,
    /// 是否一致
    pub is_consistent: bool,
    /// 用时（毫秒）
    pub elapsed_ms: u64,
}

/// 整体比对结果
#[derive(Debug, Clone)]
pub struct ComparisonResult {
    /// 各表比对结果
    pub tables: Vec<TableComparisonResult>,
    /// 总行数（源端）
    pub total_source_rows: u64,
    /// 总行数（目标端）
    pub total_target_rows: u64,
    /// 总差异行数
    pub total_diffs: u64,
    /// 是否全部一致
    pub all_consistent: bool,
    /// 用时（毫秒）
    pub elapsed_ms: u64,
}

impl Default for ComparisonResult {
    fn default() -> Self {
        Self {
            tables: Vec::new(),
            total_source_rows: 0,
            total_target_rows: 0,
            total_diffs: 0,
            all_consistent: true, // 空结果视为一致
            elapsed_ms: 0,
        }
    }
}

// =====================================================================
// ComparisonConfig — 比对配置
// =====================================================================

/// 比对配置
#[derive(Debug, Clone)]
pub struct ComparisonConfig {
    /// 批量大小
    pub batch_size: usize,
    /// 最大差异样本数（超出不再记录，只统计 count）
    pub max_diff_samples: usize,
    /// 是否启用 checksum 优化（先比对 hash）
    pub use_checksum: bool,
    /// 表过滤（None 表示比对所有表）
    pub table_filter: Option<Vec<String>>,
}

impl Default for ComparisonConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            max_diff_samples: 1000,
            use_checksum: true,
            table_filter: None,
        }
    }
}

// =====================================================================
// RowSource — 行数据源（复用 snapshot 的 trait 概念）
// =====================================================================

/// 比对用的行数据源 — 提供按主键升序的批量读取
pub trait ComparisonSource: Send + Sync {
    /// 获取所有表的 schema
    fn list_tables(&self) -> Result<Vec<TableSchema>, ComparisonError>;

    /// 读取一批数据（按主键升序）
    ///
    /// # 参数
    /// - `schema`：表 schema
    /// - `last_pk`：上一批最后一个主键值（None 表示从头开始）
    /// - `batch_size`：批量大小
    fn read_batch(
        &self,
        schema: &TableSchema,
        last_pk: Option<&SzValue>,
        batch_size: usize,
    ) -> Result<Vec<DecodedRow>, ComparisonError>;

    /// 获取表行数（用于预估进度）
    fn count_rows(&self, table_name: &str) -> Result<u64, ComparisonError>;
}

// =====================================================================
// DataComparison — 数据比对器
// =====================================================================

/// 数据比对器
pub struct DataComparison {
    /// 源端数据源
    source: Box<dyn ComparisonSource>,
    /// 目标端数据源
    target: Box<dyn ComparisonSource>,
    /// 配置
    config: ComparisonConfig,
}

impl DataComparison {
    /// 创建比对器
    pub fn new(
        source: impl ComparisonSource + 'static,
        target: impl ComparisonSource + 'static,
        config: ComparisonConfig,
    ) -> Self {
        Self {
            source: Box::new(source),
            target: Box::new(target),
            config,
        }
    }

    /// 执行比对
    pub fn compare(&self) -> Result<ComparisonResult, ComparisonError> {
        let start = std::time::Instant::now();

        let source_schemas = self.source.list_tables()?;
        let target_schemas = self.target.list_tables()?;

        // 过滤表（仅比对两端都有的表）
        let source_map: HashMap<&str, &TableSchema> = source_schemas
            .iter()
            .map(|s| (s.table_name.as_str(), s))
            .collect();
        let target_map: HashMap<&str, &TableSchema> = target_schemas
            .iter()
            .map(|s| (s.table_name.as_str(), s))
            .collect();

        let common_tables: Vec<&&TableSchema> = source_map
            .keys()
            .filter(|name| target_map.contains_key(**name))
            .map(|name| source_map.get(*name).unwrap())
            .filter(|s| {
                if let Some(filter) = &self.config.table_filter {
                    filter.contains(&s.table_name)
                } else {
                    true
                }
            })
            .collect();

        let mut result = ComparisonResult {
            tables: Vec::with_capacity(common_tables.len()),
            all_consistent: true,
            ..Default::default()
        };

        for schema in common_tables {
            let table_result = self.compare_table(schema)?;
            result.total_source_rows += table_result.source_rows;
            result.total_target_rows += table_result.target_rows;
            result.total_diffs += table_result.diff_count;
            if !table_result.is_consistent {
                result.all_consistent = false;
            }
            result.tables.push(table_result);
        }

        result.elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// 比对单张表
    fn compare_table(
        &self,
        schema: &TableSchema,
    ) -> Result<TableComparisonResult, ComparisonError> {
        let start = std::time::Instant::now();
        let pk_name = schema
            .columns
            .first()
            .map(|c| c.name.as_str())
            .ok_or_else(|| {
                ComparisonError::SchemaMismatch(format!(
                    "table {} has no columns",
                    schema.table_name
                ))
            })?;

        let source_count = self.source.count_rows(&schema.table_name)?;
        let target_count = self.target.count_rows(&schema.table_name)?;

        let mut compared = 0u64;
        let mut diff_count = 0u64;
        let mut differences = Vec::new();

        let mut source_pk: Option<SzValue> = None;
        let mut target_pk: Option<SzValue> = None;
        let mut source_batch: Vec<DecodedRow> = Vec::new();
        let mut target_batch: Vec<DecodedRow> = Vec::new();
        let mut source_idx = 0usize;
        let mut target_idx = 0usize;
        // exhausted 标志：一端 read_batch 返回空后标记为 true，避免后续重新从头读取导致死循环
        let mut source_exhausted = false;
        let mut target_exhausted = false;

        loop {
            // 补充源端批次（仅当未耗尽且当前批次已消费完时）
            if !source_exhausted && source_idx >= source_batch.len() {
                source_batch =
                    self.source
                        .read_batch(schema, source_pk.as_ref(), self.config.batch_size)?;
                source_idx = 0;
                if source_batch.is_empty() {
                    source_exhausted = true;
                }
            }

            // 补充目标端批次（仅当未耗尽且当前批次已消费完时）
            if !target_exhausted && target_idx >= target_batch.len() {
                target_batch =
                    self.target
                        .read_batch(schema, target_pk.as_ref(), self.config.batch_size)?;
                target_idx = 0;
                if target_batch.is_empty() {
                    target_exhausted = true;
                }
            }

            // 两端都读完
            if source_exhausted && target_exhausted {
                break;
            }

            // 一端读完，另一端剩余全部为差异
            if source_exhausted {
                // 目标端剩余全部 TargetOnly
                while target_idx < target_batch.len() {
                    let row = &target_batch[target_idx];
                    if differences.len() < self.config.max_diff_samples {
                        differences.push(RowDifference {
                            kind: DiffKind::TargetOnly,
                            pk_value: pk_string(row, pk_name),
                            source_row: None,
                            target_row: Some(row.clone()),
                            mismatched_columns: Vec::new(),
                        });
                    }
                    diff_count += 1;
                    compared += 1;
                    target_idx += 1;
                }
                target_exhausted = true;
                continue;
            }

            if target_exhausted {
                // 源端剩余全部 SourceOnly
                while source_idx < source_batch.len() {
                    let row = &source_batch[source_idx];
                    if differences.len() < self.config.max_diff_samples {
                        differences.push(RowDifference {
                            kind: DiffKind::SourceOnly,
                            pk_value: pk_string(row, pk_name),
                            source_row: Some(row.clone()),
                            target_row: None,
                            mismatched_columns: Vec::new(),
                        });
                    }
                    diff_count += 1;
                    compared += 1;
                    source_idx += 1;
                }
                source_exhausted = true;
                continue;
            }

            // 双指针比对
            let src_row = &source_batch[source_idx];
            let tgt_row = &target_batch[target_idx];
            let src_pk = pk_value(src_row, pk_name).ok_or(ComparisonError::PkNotFound)?;
            let tgt_pk = pk_value(tgt_row, pk_name).ok_or(ComparisonError::PkNotFound)?;

            match compare_values(&src_pk, &tgt_pk) {
                std::cmp::Ordering::Equal => {
                    // pk 相等，比对内容
                    // 启用 checksum 优化：先比对 checksum，若一致则跳过逐字段比对
                    let mismatched = if self.config.use_checksum {
                        let src_cksum = row_checksum(src_row);
                        let tgt_cksum = row_checksum(tgt_row);
                        if src_cksum == tgt_cksum {
                            Vec::new()
                        } else {
                            compare_row_content(src_row, tgt_row)
                        }
                    } else {
                        compare_row_content(src_row, tgt_row)
                    };
                    if !mismatched.is_empty() {
                        if differences.len() < self.config.max_diff_samples {
                            differences.push(RowDifference {
                                kind: DiffKind::ContentMismatch,
                                pk_value: pk_string(src_row, pk_name),
                                source_row: Some(src_row.clone()),
                                target_row: Some(tgt_row.clone()),
                                mismatched_columns: mismatched,
                            });
                        }
                        diff_count += 1;
                    }
                    compared += 1;
                    source_pk = Some(src_pk);
                    target_pk = Some(tgt_pk);
                    source_idx += 1;
                    target_idx += 1;
                }
                std::cmp::Ordering::Less => {
                    // 源端 pk < 目标端 pk → 源端缺失（SourceOnly）
                    if differences.len() < self.config.max_diff_samples {
                        differences.push(RowDifference {
                            kind: DiffKind::SourceOnly,
                            pk_value: pk_string(src_row, pk_name),
                            source_row: Some(src_row.clone()),
                            target_row: None,
                            mismatched_columns: Vec::new(),
                        });
                    }
                    diff_count += 1;
                    compared += 1;
                    source_pk = Some(src_pk);
                    source_idx += 1;
                }
                std::cmp::Ordering::Greater => {
                    // 源端 pk > 目标端 pk → 目标端多余（TargetOnly）
                    if differences.len() < self.config.max_diff_samples {
                        differences.push(RowDifference {
                            kind: DiffKind::TargetOnly,
                            pk_value: pk_string(tgt_row, pk_name),
                            source_row: None,
                            target_row: Some(tgt_row.clone()),
                            mismatched_columns: Vec::new(),
                        });
                    }
                    diff_count += 1;
                    compared += 1;
                    target_pk = Some(tgt_pk);
                    target_idx += 1;
                }
            }
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(TableComparisonResult {
            table_name: schema.table_name.clone(),
            source_rows: source_count,
            target_rows: target_count,
            compared_rows: compared,
            diff_count,
            differences,
            is_consistent: diff_count == 0,
            elapsed_ms,
        })
    }
}

// =====================================================================
// 辅助函数
// =====================================================================

/// 从行中提取主键值
fn pk_value(row: &DecodedRow, pk_name: &str) -> Option<SzValue> {
    row.columns
        .iter()
        .find(|(n, _)| n == pk_name)
        .map(|(_, v)| v.clone())
}

/// 主键转字符串（用于日志）
fn pk_string(row: &DecodedRow, pk_name: &str) -> String {
    match pk_value(row, pk_name) {
        Some(SzValue::Null) => "NULL".to_string(),
        Some(SzValue::Int64(v)) => v.to_string(),
        Some(SzValue::Float64(v)) => v.to_string(),
        Some(SzValue::Text(s)) => s,
        Some(SzValue::Bool(b)) => b.to_string(),
        Some(v) => format!("{v:?}"),
        None => "<no pk>".to_string(),
    }
}

/// 比较两个值（用于主键排序）
fn compare_values(a: &SzValue, b: &SzValue) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (SzValue::Int64(x), SzValue::Int64(y)) => x.cmp(y),
        (SzValue::Float64(x), SzValue::Float64(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (SzValue::Text(x), SzValue::Text(y)) => x.cmp(y),
        (SzValue::Bool(x), SzValue::Bool(y)) => x.cmp(y),
        (SzValue::Date(x), SzValue::Date(y)) => x.cmp(y),
        (SzValue::Timestamp(x), SzValue::Timestamp(y)) => x.cmp(y),
        _ => {
            // 退化：按字符串形式比较
            let s1 = format!("{a:?}");
            let s2 = format!("{b:?}");
            s1.cmp(&s2)
        }
    }
}

/// 比对两行内容，返回不一致的列名列表
fn compare_row_content(source: &DecodedRow, target: &DecodedRow) -> Vec<String> {
    let mut mismatched = Vec::new();
    let target_map: HashMap<&str, &SzValue> = target
        .columns
        .iter()
        .map(|(n, v)| (n.as_str(), v))
        .collect();

    for (name, src_value) in &source.columns {
        if let Some(tgt_value) = target_map.get(name.as_str()) {
            if !values_equal(src_value, tgt_value) {
                mismatched.push(name.clone());
            }
        } else {
            mismatched.push(name.clone());
        }
    }
    mismatched
}

/// 值相等判断（处理浮点精度等）
fn values_equal(a: &SzValue, b: &SzValue) -> bool {
    match (a, b) {
        (SzValue::Null, SzValue::Null) => true,
        (SzValue::Int64(x), SzValue::Int64(y)) => x == y,
        (SzValue::Float64(x), SzValue::Float64(y)) => {
            // 浮点数用 ulps 比较避免精度问题
            (x - y).abs() < f64::EPSILON || x.to_bits() == y.to_bits()
        }
        (SzValue::Text(x), SzValue::Text(y)) => x == y,
        (SzValue::Bool(x), SzValue::Bool(y)) => x == y,
        (SzValue::Date(x), SzValue::Date(y)) => x == y,
        (SzValue::Timestamp(x), SzValue::Timestamp(y)) => x == y,
        (SzValue::Blob(x), SzValue::Blob(y)) => x == y,
        (SzValue::Decimal(x, xs), SzValue::Decimal(y, ys)) => x == y && xs == ys,
        (SzValue::Json(x), SzValue::Json(y)) => x == y,
        (SzValue::Enum(x), SzValue::Enum(y)) => x == y,
        (SzValue::Array(x), SzValue::Array(y)) => x == y,
        _ => false,
    }
}

// =====================================================================
// MemoryComparisonSource — 内存比对数据源（测试用）
// =====================================================================

/// 内存比对数据源（测试用）
pub struct MemoryComparisonSource {
    /// 表 schema
    schemas: Vec<TableSchema>,
    /// 表名 → 行数据
    data: HashMap<String, Vec<DecodedRow>>,
}

impl MemoryComparisonSource {
    /// 创建内存数据源
    pub fn new(schemas: Vec<TableSchema>) -> Self {
        Self {
            schemas,
            data: HashMap::new(),
        }
    }

    /// 添加表数据
    pub fn with_data(mut self, table_name: impl Into<String>, mut rows: Vec<DecodedRow>) -> Self {
        // 按 pk 升序排序
        rows.sort_by(|a, b| {
            let pk_a = a
                .columns
                .first()
                .map(|(_, v)| v.clone())
                .unwrap_or(SzValue::Null);
            let pk_b = b
                .columns
                .first()
                .map(|(_, v)| v.clone())
                .unwrap_or(SzValue::Null);
            compare_values(&pk_a, &pk_b)
        });
        self.data.insert(table_name.into(), rows);
        self
    }
}

impl ComparisonSource for MemoryComparisonSource {
    fn list_tables(&self) -> Result<Vec<TableSchema>, ComparisonError> {
        Ok(self.schemas.clone())
    }

    fn read_batch(
        &self,
        schema: &TableSchema,
        last_pk: Option<&SzValue>,
        batch_size: usize,
    ) -> Result<Vec<DecodedRow>, ComparisonError> {
        let rows = self
            .data
            .get(&schema.table_name)
            .cloned()
            .unwrap_or_default();

        let pk_name = schema
            .columns
            .first()
            .map(|c| c.name.as_str())
            .ok_or_else(|| {
                ComparisonError::SchemaMismatch(format!(
                    "table {} has no columns",
                    schema.table_name
                ))
            })?;

        let start_idx = match last_pk {
            None => 0,
            Some(last_pk_value) => rows
                .iter()
                .position(|r| {
                    pk_value(r, pk_name)
                        .map(|v| compare_values(&v, last_pk_value) == std::cmp::Ordering::Greater)
                        .unwrap_or(false)
                })
                .unwrap_or(rows.len()),
        };

        let end_idx = (start_idx + batch_size).min(rows.len());
        if start_idx >= rows.len() {
            return Ok(Vec::new());
        }
        Ok(rows[start_idx..end_idx].to_vec())
    }

    fn count_rows(&self, table_name: &str) -> Result<u64, ComparisonError> {
        Ok(self
            .data
            .get(table_name)
            .map(|v| v.len() as u64)
            .unwrap_or(0))
    }
}

// =====================================================================
// RepairAction — 修复动作（P5-4 自动修复）
// =====================================================================

/// 修复动作类型
#[derive(Debug, Clone, PartialEq)]
pub enum RepairAction {
    /// 在目标端插入缺失行（对应 SourceOnly）
    Insert { table: String, row: DecodedRow },
    /// 在目标端更新不一致行（对应 ContentMismatch）
    Update { table: String, row: DecodedRow },
    /// 在目标端删除多余行（对应 TargetOnly）
    Delete { table: String, pk_value: SzValue },
}

/// 修复策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairStrategy {
    /// 源端胜：以源端为准，目标端向源端对齐
    /// - SourceOnly → Insert（目标端补行）
    /// - ContentMismatch → Update（目标端更新）
    /// - TargetOnly → Delete（目标端删行）
    SourceWins,
    /// 目标端胜：以目标端为准，源端向目标端对齐
    /// - SourceOnly → Delete（源端删行）
    /// - ContentMismatch → Update（源端更新）
    /// - TargetOnly → Insert（源端补行）
    TargetWins,
    /// 仅生成修复计划，不执行（用于预览）
    DryRun,
}

/// 修复计划
#[derive(Debug, Clone, Default)]
pub struct RepairPlan {
    /// 修复动作列表
    pub actions: Vec<RepairAction>,
    /// 插入数
    pub total_inserts: u64,
    /// 更新数
    pub total_updates: u64,
    /// 删除数
    pub total_deletes: u64,
}

impl RepairPlan {
    /// 总修复动作数
    pub fn total_actions(&self) -> u64 {
        self.total_inserts + self.total_updates + self.total_deletes
    }

    /// 是否为空（无需修复）
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

impl DataComparison {
    /// 根据比对结果生成修复计划
    ///
    /// # 参数
    /// - `result`：比对结果
    /// - `strategy`：修复策略
    ///
    /// # 返回
    /// 修复计划，包含所有修复动作
    pub fn generate_repair_plan(
        &self,
        result: &ComparisonResult,
        strategy: RepairStrategy,
    ) -> RepairPlan {
        let mut plan = RepairPlan::default();

        for table_result in &result.tables {
            for diff in &table_result.differences {
                let action = match (diff.kind, strategy) {
                    // 源端胜策略
                    (DiffKind::SourceOnly, RepairStrategy::SourceWins) => {
                        diff.source_row.as_ref().map(|row| RepairAction::Insert {
                            table: table_result.table_name.clone(),
                            row: row.clone(),
                        })
                    }
                    (DiffKind::ContentMismatch, RepairStrategy::SourceWins) => {
                        diff.source_row.as_ref().map(|row| RepairAction::Update {
                            table: table_result.table_name.clone(),
                            row: row.clone(),
                        })
                    }
                    (DiffKind::TargetOnly, RepairStrategy::SourceWins) => {
                        // 从 target_row 提取 pk 值（pk_value 字段是字符串，需从 row 提取）
                        diff.target_row.as_ref().and_then(|row| {
                            row.columns.first().map(|(_, v)| RepairAction::Delete {
                                table: table_result.table_name.clone(),
                                pk_value: v.clone(),
                            })
                        })
                    }
                    // 目标端胜策略（反向）
                    (DiffKind::SourceOnly, RepairStrategy::TargetWins) => {
                        diff.source_row.as_ref().and_then(|row| {
                            row.columns.first().map(|(_, v)| RepairAction::Delete {
                                table: table_result.table_name.clone(),
                                pk_value: v.clone(),
                            })
                        })
                    }
                    (DiffKind::ContentMismatch, RepairStrategy::TargetWins) => {
                        diff.target_row.as_ref().map(|row| RepairAction::Update {
                            table: table_result.table_name.clone(),
                            row: row.clone(),
                        })
                    }
                    (DiffKind::TargetOnly, RepairStrategy::TargetWins) => {
                        diff.target_row.as_ref().map(|row| RepairAction::Insert {
                            table: table_result.table_name.clone(),
                            row: row.clone(),
                        })
                    }
                    // DryRun 不生成实际动作
                    (_, RepairStrategy::DryRun) => None,
                };

                if let Some(act) = action {
                    match &act {
                        RepairAction::Insert { .. } => plan.total_inserts += 1,
                        RepairAction::Update { .. } => plan.total_updates += 1,
                        RepairAction::Delete { .. } => plan.total_deletes += 1,
                    }
                    plan.actions.push(act);
                }
            }
        }

        plan
    }
}

// =====================================================================
// IncrementalComparison — 增量比对（P5-4）
// =====================================================================

/// 增量比对配置
#[derive(Debug, Clone, Default)]
pub struct IncrementalConfig {
    /// 上次比对的最大 pk 值（仅比对 pk > last_pk 的行）
    /// None 表示全量比对
    pub last_compared_pk: Option<SzValue>,
    /// 上次比对的时间戳（Unix 毫秒）
    pub last_compared_at: u64,
}

/// 增量比对结果
#[derive(Debug, Clone)]
pub struct IncrementalResult {
    /// 基础比对结果
    pub base: ComparisonResult,
    /// 本次增量比对的起始 pk
    pub since_pk: Option<SzValue>,
    /// 增量比对覆盖的行数
    pub incremental_rows: u64,
}

impl DataComparison {
    /// 执行增量比对
    ///
    /// 仅比对 `pk > last_compared_pk` 的行，避免全表扫描。
    /// 适用于"上次比对后仅有少量变更"的场景。
    ///
    /// # 参数
    /// - `config`：增量比对配置
    ///
    /// # 返回
    /// 增量比对结果
    pub fn compare_incremental(
        &self,
        config: &IncrementalConfig,
    ) -> Result<IncrementalResult, ComparisonError> {
        let start = std::time::Instant::now();

        let source_schemas = self.source.list_tables()?;
        let target_schemas = self.target.list_tables()?;

        let source_map: HashMap<&str, &TableSchema> = source_schemas
            .iter()
            .map(|s| (s.table_name.as_str(), s))
            .collect();
        let target_map: HashMap<&str, &TableSchema> = target_schemas
            .iter()
            .map(|s| (s.table_name.as_str(), s))
            .collect();

        let common_tables: Vec<&&TableSchema> = source_map
            .keys()
            .filter(|name| target_map.contains_key(**name))
            .map(|name| source_map.get(*name).unwrap())
            .filter(|s| {
                if let Some(filter) = &self.config.table_filter {
                    filter.contains(&s.table_name)
                } else {
                    true
                }
            })
            .collect();

        let mut result = ComparisonResult {
            tables: Vec::with_capacity(common_tables.len()),
            all_consistent: true,
            ..Default::default()
        };
        let mut incremental_rows = 0u64;

        for schema in common_tables {
            let table_result =
                self.compare_table_incremental(schema, config.last_compared_pk.as_ref())?;
            incremental_rows += table_result.compared_rows;
            result.total_source_rows += table_result.source_rows;
            result.total_target_rows += table_result.target_rows;
            result.total_diffs += table_result.diff_count;
            if !table_result.is_consistent {
                result.all_consistent = false;
            }
            result.tables.push(table_result);
        }

        result.elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(IncrementalResult {
            base: result,
            since_pk: config.last_compared_pk.clone(),
            incremental_rows,
        })
    }

    /// 增量比对单张表（仅比对 pk > since_pk 的行）
    fn compare_table_incremental(
        &self,
        schema: &TableSchema,
        since_pk: Option<&SzValue>,
    ) -> Result<TableComparisonResult, ComparisonError> {
        // 增量比对复用全量比对逻辑，但 read_batch 时传入 since_pk 作为起始点
        // 注意：增量比对的 last_pk 语义是"上次比对的最大 pk"，本次从 pk > since_pk 开始
        let start = std::time::Instant::now();
        let pk_name = schema
            .columns
            .first()
            .map(|c| c.name.as_str())
            .ok_or_else(|| {
                ComparisonError::SchemaMismatch(format!(
                    "table {} has no columns",
                    schema.table_name
                ))
            })?;

        // 增量模式下，count_rows 无法精确反映增量行数，这里用全表行数作为参考
        let source_count = self.source.count_rows(&schema.table_name)?;
        let target_count = self.target.count_rows(&schema.table_name)?;

        let mut compared = 0u64;
        let mut diff_count = 0u64;
        let mut differences = Vec::new();

        // 起始 pk：since_pk 表示"从此 pk 之后开始比对"
        // read_batch 的 last_pk 语义是"上一批最后一个 pk"，返回 pk > last_pk 的行
        // 所以直接传 since_pk 即可
        let mut source_pk: Option<SzValue> = since_pk.cloned();
        let mut target_pk: Option<SzValue> = since_pk.cloned();
        let mut source_batch: Vec<DecodedRow> = Vec::new();
        let mut target_batch: Vec<DecodedRow> = Vec::new();
        let mut source_idx = 0usize;
        let mut target_idx = 0usize;
        // exhausted 标志：一端 read_batch 返回空后标记为 true，避免后续重新从头读取导致死循环
        let mut source_exhausted = false;
        let mut target_exhausted = false;

        loop {
            // 补充源端批次（仅当未耗尽且当前批次已消费完时）
            if !source_exhausted && source_idx >= source_batch.len() {
                source_batch =
                    self.source
                        .read_batch(schema, source_pk.as_ref(), self.config.batch_size)?;
                source_idx = 0;
                if source_batch.is_empty() {
                    source_exhausted = true;
                }
            }

            // 补充目标端批次（仅当未耗尽且当前批次已消费完时）
            if !target_exhausted && target_idx >= target_batch.len() {
                target_batch =
                    self.target
                        .read_batch(schema, target_pk.as_ref(), self.config.batch_size)?;
                target_idx = 0;
                if target_batch.is_empty() {
                    target_exhausted = true;
                }
            }

            // 两端都读完
            if source_exhausted && target_exhausted {
                break;
            }

            // 一端读完，另一端剩余全部为差异
            if source_exhausted {
                while target_idx < target_batch.len() {
                    let row = &target_batch[target_idx];
                    if differences.len() < self.config.max_diff_samples {
                        differences.push(RowDifference {
                            kind: DiffKind::TargetOnly,
                            pk_value: pk_string(row, pk_name),
                            source_row: None,
                            target_row: Some(row.clone()),
                            mismatched_columns: Vec::new(),
                        });
                    }
                    diff_count += 1;
                    compared += 1;
                    target_idx += 1;
                }
                target_exhausted = true;
                continue;
            }

            if target_exhausted {
                while source_idx < source_batch.len() {
                    let row = &source_batch[source_idx];
                    if differences.len() < self.config.max_diff_samples {
                        differences.push(RowDifference {
                            kind: DiffKind::SourceOnly,
                            pk_value: pk_string(row, pk_name),
                            source_row: Some(row.clone()),
                            target_row: None,
                            mismatched_columns: Vec::new(),
                        });
                    }
                    diff_count += 1;
                    compared += 1;
                    source_idx += 1;
                }
                source_exhausted = true;
                continue;
            }

            let src_row = &source_batch[source_idx];
            let tgt_row = &target_batch[target_idx];
            let src_pk = pk_value(src_row, pk_name).ok_or(ComparisonError::PkNotFound)?;
            let tgt_pk = pk_value(tgt_row, pk_name).ok_or(ComparisonError::PkNotFound)?;

            match compare_values(&src_pk, &tgt_pk) {
                std::cmp::Ordering::Equal => {
                    let mismatched = if self.config.use_checksum {
                        let src_cksum = row_checksum(src_row);
                        let tgt_cksum = row_checksum(tgt_row);
                        if src_cksum == tgt_cksum {
                            Vec::new()
                        } else {
                            compare_row_content(src_row, tgt_row)
                        }
                    } else {
                        compare_row_content(src_row, tgt_row)
                    };
                    if !mismatched.is_empty() {
                        if differences.len() < self.config.max_diff_samples {
                            differences.push(RowDifference {
                                kind: DiffKind::ContentMismatch,
                                pk_value: pk_string(src_row, pk_name),
                                source_row: Some(src_row.clone()),
                                target_row: Some(tgt_row.clone()),
                                mismatched_columns: mismatched,
                            });
                        }
                        diff_count += 1;
                    }
                    compared += 1;
                    source_pk = Some(src_pk);
                    target_pk = Some(tgt_pk);
                    source_idx += 1;
                    target_idx += 1;
                }
                std::cmp::Ordering::Less => {
                    if differences.len() < self.config.max_diff_samples {
                        differences.push(RowDifference {
                            kind: DiffKind::SourceOnly,
                            pk_value: pk_string(src_row, pk_name),
                            source_row: Some(src_row.clone()),
                            target_row: None,
                            mismatched_columns: Vec::new(),
                        });
                    }
                    diff_count += 1;
                    compared += 1;
                    source_pk = Some(src_pk);
                    source_idx += 1;
                }
                std::cmp::Ordering::Greater => {
                    if differences.len() < self.config.max_diff_samples {
                        differences.push(RowDifference {
                            kind: DiffKind::TargetOnly,
                            pk_value: pk_string(tgt_row, pk_name),
                            source_row: None,
                            target_row: Some(tgt_row.clone()),
                            mismatched_columns: Vec::new(),
                        });
                    }
                    diff_count += 1;
                    compared += 1;
                    target_pk = Some(tgt_pk);
                    target_idx += 1;
                }
            }
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(TableComparisonResult {
            table_name: schema.table_name.clone(),
            source_rows: source_count,
            target_rows: target_count,
            compared_rows: compared,
            diff_count,
            differences,
            is_consistent: diff_count == 0,
            elapsed_ms,
        })
    }
}

// =====================================================================
// ComparisonReport — 比对报告输出（P5-4）
// =====================================================================

/// 比对报告
pub struct ComparisonReport {
    /// 比对结果
    pub result: ComparisonResult,
    /// 生成时间戳（Unix 毫秒）
    pub generated_at: u64,
}

impl ComparisonReport {
    /// 创建报告
    pub fn new(result: ComparisonResult) -> Self {
        Self {
            result,
            generated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        }
    }

    /// 输出 Markdown 格式报告
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# 数据比对报告\n\n");
        md.push_str(&format!("- 生成时间: {} (Unix ms)\n", self.generated_at));
        md.push_str(&format!(
            "- 总体一致性: {}\n",
            if self.result.all_consistent {
                "✅ 一致"
            } else {
                "❌ 存在差异"
            }
        ));
        md.push_str(&format!(
            "- 源端总行数: {}\n",
            self.result.total_source_rows
        ));
        md.push_str(&format!(
            "- 目标端总行数: {}\n",
            self.result.total_target_rows
        ));
        md.push_str(&format!("- 差异行数: {}\n", self.result.total_diffs));
        md.push_str(&format!("- 比对耗时: {} ms\n\n", self.result.elapsed_ms));

        if self.result.tables.is_empty() {
            md.push_str("（无比对表）\n");
            return md;
        }

        md.push_str("## 各表比对详情\n\n");
        md.push_str("| 表名 | 源端行数 | 目标端行数 | 已比对 | 差异数 | 一致性 | 耗时(ms) |\n");
        md.push_str("|------|---------|-----------|--------|--------|--------|----------|\n");
        for t in &self.result.tables {
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                t.table_name,
                t.source_rows,
                t.target_rows,
                t.compared_rows,
                t.diff_count,
                if t.is_consistent {
                    "✅"
                } else {
                    "❌"
                },
                t.elapsed_ms,
            ));
        }

        // 差异样本（前 10 条）
        let mut sample_count = 0;
        for t in &self.result.tables {
            for d in &t.differences {
                if sample_count >= 10 {
                    break;
                }
                if sample_count == 0 {
                    md.push_str("\n## 差异样本（前 10 条）\n\n");
                }
                let kind_str = match d.kind {
                    DiffKind::SourceOnly => "源端缺失",
                    DiffKind::TargetOnly => "目标端多余",
                    DiffKind::ContentMismatch => "内容不一致",
                };
                md.push_str(&format!(
                    "- [{}] 表={} pk={} {}\n",
                    kind_str,
                    t.table_name,
                    d.pk_value,
                    if d.mismatched_columns.is_empty() {
                        String::new()
                    } else {
                        format!("不一致列: {}", d.mismatched_columns.join(", "))
                    },
                ));
                sample_count += 1;
            }
            if sample_count >= 10 {
                break;
            }
        }

        md
    }

    /// 输出 JSON 格式报告
    pub fn to_json(&self) -> Result<String, ComparisonError> {
        let json = serde_json::json!({
            "generated_at": self.generated_at,
            "all_consistent": self.result.all_consistent,
            "total_source_rows": self.result.total_source_rows,
            "total_target_rows": self.result.total_target_rows,
            "total_diffs": self.result.total_diffs,
            "elapsed_ms": self.result.elapsed_ms,
            "tables": self.result.tables.iter().map(|t| {
                serde_json::json!({
                    "table_name": t.table_name,
                    "source_rows": t.source_rows,
                    "target_rows": t.target_rows,
                    "compared_rows": t.compared_rows,
                    "diff_count": t.diff_count,
                    "is_consistent": t.is_consistent,
                    "elapsed_ms": t.elapsed_ms,
                    "differences": t.differences.iter().map(|d| {
                        serde_json::json!({
                            "kind": format!("{:?}", d.kind),
                            "pk_value": d.pk_value,
                            "mismatched_columns": d.mismatched_columns,
                        })
                    }).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        });
        serde_json::to_string_pretty(&json)
            .map_err(|e| ComparisonError::Internal(format!("JSON serialization failed: {e}")))
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnDef, DataType};

    fn make_schema(table_id: u32, name: &str) -> TableSchema {
        TableSchema {
            table_id,
            table_name: name.to_string(),
            columns: vec![
                ColumnDef::not_null("id", DataType::Int64),
                ColumnDef::nullable("name", DataType::Text),
            ],
            version: 1,
        }
    }

    fn make_row(id: i64, name: &str) -> DecodedRow {
        DecodedRow {
            columns: vec![
                ("id".to_string(), SzValue::Int64(id)),
                ("name".to_string(), SzValue::Text(name.to_string())),
            ],
        }
    }

    #[test]
    fn comparison_config_default() {
        let cfg = ComparisonConfig::default();
        assert_eq!(cfg.batch_size, 1000);
        assert!(cfg.use_checksum);
    }

    #[test]
    fn memory_source_basic() {
        let schema = make_schema(1, "users");
        let source = MemoryComparisonSource::new(vec![schema.clone()])
            .with_data("users", vec![make_row(1, "A"), make_row(2, "B")]);

        let tables = source.list_tables().unwrap();
        assert_eq!(tables.len(), 1);

        let batch = source.read_batch(&schema, None, 10).unwrap();
        assert_eq!(batch.len(), 2);

        let count = source.count_rows("users").unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn comparison_consistent_tables() {
        let schema = make_schema(1, "users");
        let rows = vec![make_row(1, "A"), make_row(2, "B"), make_row(3, "C")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", rows.clone());
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        assert!(result.all_consistent);
        assert_eq!(result.total_source_rows, 3);
        assert_eq!(result.total_target_rows, 3);
        assert_eq!(result.total_diffs, 0);
    }

    #[test]
    fn comparison_source_only() {
        let schema = make_schema(1, "users");
        let source_rows = vec![make_row(1, "A"), make_row(2, "B"), make_row(3, "C")];
        let target_rows = vec![make_row(1, "A"), make_row(2, "B")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        assert!(!result.all_consistent);
        assert_eq!(result.total_diffs, 1);
        assert_eq!(result.tables[0].differences.len(), 1);
        assert_eq!(result.tables[0].differences[0].kind, DiffKind::SourceOnly);
    }

    #[test]
    fn comparison_target_only() {
        let schema = make_schema(1, "users");
        let source_rows = vec![make_row(1, "A"), make_row(2, "B")];
        let target_rows = vec![make_row(1, "A"), make_row(2, "B"), make_row(3, "C")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        assert!(!result.all_consistent);
        assert_eq!(result.total_diffs, 1);
        assert_eq!(result.tables[0].differences[0].kind, DiffKind::TargetOnly);
    }

    #[test]
    fn comparison_content_mismatch() {
        let schema = make_schema(1, "users");
        let source_rows = vec![make_row(1, "Alice"), make_row(2, "Bob")];
        let target_rows = vec![make_row(1, "Alice"), make_row(2, "Bobby")]; // name 不同

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        assert!(!result.all_consistent);
        assert_eq!(result.total_diffs, 1);
        assert_eq!(
            result.tables[0].differences[0].kind,
            DiffKind::ContentMismatch
        );
        assert!(result.tables[0].differences[0]
            .mismatched_columns
            .contains(&"name".to_string()));
    }

    #[test]
    fn comparison_multiple_diffs() {
        let schema = make_schema(1, "users");
        let source_rows = vec![make_row(1, "A"), make_row(3, "C"), make_row(5, "E")];
        let target_rows = vec![make_row(2, "B"), make_row(3, "C"), make_row(6, "F")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        // id=1 SourceOnly, id=2 TargetOnly, id=3 相同, id=5 SourceOnly, id=6 TargetOnly
        assert_eq!(result.total_diffs, 4);
    }

    #[test]
    fn comparison_empty_tables() {
        let schema = make_schema(1, "users");
        let source = MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", vec![]);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", vec![]);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        assert!(result.all_consistent);
        assert_eq!(result.total_diffs, 0);
    }

    #[test]
    fn comparison_table_filter() {
        let schema1 = make_schema(1, "users");
        let schema2 = make_schema(2, "orders");

        let source = MemoryComparisonSource::new(vec![schema1, schema2.clone()])
            .with_data("users", vec![make_row(1, "A")])
            .with_data("orders", vec![make_row(10, "o1")]);
        let target = MemoryComparisonSource::new(vec![make_schema(1, "users"), schema2])
            .with_data("users", vec![make_row(1, "A")])
            .with_data("orders", vec![make_row(10, "o1")]);

        let config = ComparisonConfig {
            table_filter: Some(vec!["users".to_string()]),
            ..Default::default()
        };
        let cmp = DataComparison::new(source, target, config);
        let result = cmp.compare().unwrap();

        assert_eq!(result.tables.len(), 1);
        assert_eq!(result.tables[0].table_name, "users");
    }

    #[test]
    fn comparison_max_diff_samples() {
        let schema = make_schema(1, "users");
        let source_rows: Vec<DecodedRow> = (1..=100).map(|i| make_row(i, "src")).collect();
        let target_rows: Vec<DecodedRow> = vec![];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let config = ComparisonConfig {
            max_diff_samples: 10,
            ..Default::default()
        };
        let cmp = DataComparison::new(source, target, config);
        let result = cmp.compare().unwrap();

        assert_eq!(result.total_diffs, 100);
        assert_eq!(result.tables[0].differences.len(), 10); // 限制样本数
    }

    #[test]
    fn comparison_batch_iteration() {
        let schema = make_schema(1, "users");
        let source_rows: Vec<DecodedRow> =
            (1..=25).map(|i| make_row(i, &format!("u{i}"))).collect();
        let target_rows = source_rows.clone();

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let config = ComparisonConfig {
            batch_size: 10,
            ..Default::default()
        };
        let cmp = DataComparison::new(source, target, config);
        let result = cmp.compare().unwrap();

        assert_eq!(result.tables[0].compared_rows, 25);
        assert!(result.all_consistent);
    }

    #[test]
    fn comparison_multi_table() {
        let schema1 = make_schema(1, "users");
        let schema2 = make_schema(2, "orders");

        let source = MemoryComparisonSource::new(vec![schema1, schema2.clone()])
            .with_data("users", vec![make_row(1, "A")])
            .with_data("orders", vec![make_row(10, "o1")]);
        let target = MemoryComparisonSource::new(vec![make_schema(1, "users"), schema2])
            .with_data("users", vec![make_row(1, "A")])
            .with_data("orders", vec![make_row(10, "o1")]);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        assert_eq!(result.tables.len(), 2);
        assert!(result.all_consistent);
    }

    #[test]
    fn comparison_source_only_table() {
        let schema1 = make_schema(1, "users");
        let schema2 = make_schema(2, "orders");

        // 源端有两张表，目标端只有 users
        let source = MemoryComparisonSource::new(vec![schema1.clone(), schema2])
            .with_data("users", vec![make_row(1, "A")])
            .with_data("orders", vec![make_row(10, "o1")]);
        let target =
            MemoryComparisonSource::new(vec![schema1]).with_data("users", vec![make_row(1, "A")]);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        // orders 表不在目标端，跳过
        assert_eq!(result.tables.len(), 1);
    }

    #[test]
    fn values_equal_basic() {
        assert!(values_equal(&SzValue::Null, &SzValue::Null));
        assert!(values_equal(&SzValue::Int64(42), &SzValue::Int64(42)));
        assert!(!values_equal(&SzValue::Int64(42), &SzValue::Int64(43)));
        assert!(values_equal(
            &SzValue::Text("hi".to_string()),
            &SzValue::Text("hi".to_string())
        ));
        assert!(values_equal(&SzValue::Bool(true), &SzValue::Bool(true)));
        assert!(!values_equal(&SzValue::Bool(true), &SzValue::Bool(false)));
    }

    #[test]
    fn compare_values_ordering() {
        use std::cmp::Ordering;
        assert_eq!(
            compare_values(&SzValue::Int64(1), &SzValue::Int64(2)),
            Ordering::Less
        );
        assert_eq!(
            compare_values(&SzValue::Int64(2), &SzValue::Int64(1)),
            Ordering::Greater
        );
        assert_eq!(
            compare_values(&SzValue::Int64(1), &SzValue::Int64(1)),
            Ordering::Equal
        );
    }

    #[test]
    fn pk_string_formats() {
        let row = make_row(42, "Alice");
        assert_eq!(pk_string(&row, "id"), "42");

        let row_null = DecodedRow {
            columns: vec![("id".to_string(), SzValue::Null)],
        };
        assert_eq!(pk_string(&row_null, "id"), "NULL");
    }

    #[test]
    fn comparison_result_default() {
        let r = ComparisonResult::default();
        assert!(r.tables.is_empty());
        assert!(r.all_consistent);
        assert_eq!(r.total_diffs, 0);
    }

    // =================================================================
    // checksum 优化测试
    // =================================================================

    #[test]
    fn row_checksum_identical_rows() {
        let row1 = make_row(42, "Alice");
        let row2 = make_row(42, "Alice");
        assert_eq!(row_checksum(&row1), row_checksum(&row2));
    }

    #[test]
    fn row_checksum_different_rows() {
        let row1 = make_row(42, "Alice");
        let row2 = make_row(42, "Bob");
        assert_ne!(row_checksum(&row1), row_checksum(&row2));
    }

    #[test]
    fn row_checksum_different_pk() {
        let row1 = make_row(1, "Alice");
        let row2 = make_row(2, "Alice");
        assert_ne!(row_checksum(&row1), row_checksum(&row2));
    }

    #[test]
    fn row_checksum_null_vs_value() {
        let row1 = DecodedRow {
            columns: vec![("id".to_string(), SzValue::Null)],
        };
        let row2 = DecodedRow {
            columns: vec![("id".to_string(), SzValue::Int64(0))],
        };
        assert_ne!(row_checksum(&row1), row_checksum(&row2));
    }

    #[test]
    fn row_checksum_int_vs_float_no_collision() {
        // Int64(1) 和 Float64(1.0) 不应哈希冲突
        let row1 = DecodedRow {
            columns: vec![("v".to_string(), SzValue::Int64(1))],
        };
        let row2 = DecodedRow {
            columns: vec![("v".to_string(), SzValue::Float64(1.0))],
        };
        assert_ne!(row_checksum(&row1), row_checksum(&row2));
    }

    #[test]
    fn comparison_with_checksum_disabled() {
        let schema = make_schema(1, "users");
        let source_rows = vec![make_row(1, "Alice"), make_row(2, "Bob")];
        let target_rows = vec![make_row(1, "Alice"), make_row(2, "Bobby")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let config = ComparisonConfig {
            use_checksum: false,
            ..Default::default()
        };
        let cmp = DataComparison::new(source, target, config);
        let result = cmp.compare().unwrap();

        assert!(!result.all_consistent);
        assert_eq!(result.total_diffs, 1);
    }

    #[test]
    fn comparison_checksum_optimization_path() {
        // 大量一致行 + 少量差异行，验证 checksum 优化正确识别差异
        let schema = make_schema(1, "users");
        let mut source_rows = Vec::new();
        let mut target_rows = Vec::new();
        for i in 1..=1000 {
            source_rows.push(make_row(i, &format!("user{i}")));
            if i == 500 {
                target_rows.push(make_row(i, "modified")); // 差异
            } else {
                target_rows.push(make_row(i, &format!("user{i}")));
            }
        }

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        assert_eq!(result.total_diffs, 1);
        assert_eq!(
            result.tables[0].differences[0].kind,
            DiffKind::ContentMismatch
        );
    }

    // =================================================================
    // P5-4 增量比对测试
    // =================================================================

    #[test]
    fn incremental_compare_from_pk() {
        // 增量比对：从 pk > 5 开始，应只比对 6..=10 的行
        let schema = make_schema(1, "users");
        let source_rows: Vec<DecodedRow> =
            (1..=10).map(|i| make_row(i, &format!("u{i}"))).collect();
        let target_rows = source_rows.clone();

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let cfg = IncrementalConfig {
            last_compared_pk: Some(SzValue::Int64(5)),
            last_compared_at: 0,
        };
        let result = cmp.compare_incremental(&cfg).unwrap();

        // 增量覆盖行数应为 5（pk 6..=10）
        assert_eq!(result.incremental_rows, 5);
        assert!(result.base.all_consistent);
        assert_eq!(result.since_pk, Some(SzValue::Int64(5)));
    }

    #[test]
    fn incremental_compare_full_when_no_last_pk() {
        // last_compared_pk = None 表示全量比对
        let schema = make_schema(1, "users");
        let source_rows: Vec<DecodedRow> = (1..=5).map(|i| make_row(i, "u")).collect();
        let target_rows = source_rows.clone();

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let cfg = IncrementalConfig::default(); // last_compared_pk = None
        let result = cmp.compare_incremental(&cfg).unwrap();

        assert_eq!(result.incremental_rows, 5);
        assert!(result.base.all_consistent);
        assert!(result.since_pk.is_none());
    }

    #[test]
    fn incremental_compare_detects_diffs_after_pk() {
        // pk <= 5 一致；pk > 5 中 pk=7 有差异
        let schema = make_schema(1, "users");
        let source_rows: Vec<DecodedRow> =
            (1..=10).map(|i| make_row(i, &format!("u{i}"))).collect();
        let mut target_rows: Vec<DecodedRow> =
            (1..=10).map(|i| make_row(i, &format!("u{i}"))).collect();
        target_rows[6] = make_row(7, "modified"); // 修改 pk=7 的行

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let cfg = IncrementalConfig {
            last_compared_pk: Some(SzValue::Int64(5)),
            last_compared_at: 0,
        };
        let result = cmp.compare_incremental(&cfg).unwrap();

        assert!(!result.base.all_consistent);
        assert_eq!(result.base.total_diffs, 1);
        assert_eq!(
            result.base.tables[0].differences[0].kind,
            DiffKind::ContentMismatch
        );
    }

    #[test]
    fn incremental_compare_skips_diffs_before_pk() {
        // pk=3 有差异，但增量从 pk > 5 开始，应检测不到该差异
        let schema = make_schema(1, "users");
        let source_rows: Vec<DecodedRow> = (1..=10).map(|i| make_row(i, "src")).collect();
        let mut target_rows: Vec<DecodedRow> = (1..=10).map(|i| make_row(i, "src")).collect();
        target_rows[2] = make_row(3, "modified"); // pk=3 差异（在增量范围外）

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let cfg = IncrementalConfig {
            last_compared_pk: Some(SzValue::Int64(5)),
            last_compared_at: 0,
        };
        let result = cmp.compare_incremental(&cfg).unwrap();

        // 增量范围 (6..=10) 内一致，pk=3 的差异被跳过
        assert!(result.base.all_consistent);
        assert_eq!(result.base.total_diffs, 0);
    }

    // =================================================================
    // P5-4 自动修复测试
    // =================================================================

    #[test]
    fn repair_plan_source_wins() {
        // 源端胜策略：
        // - SourceOnly → Insert（目标端补行）
        // - ContentMismatch → Update（目标端更新）
        // - TargetOnly → Delete（目标端删行）
        let schema = make_schema(1, "users");
        // 源端：1, 3, 5
        // 目标端：1, 2, 5(modified)
        // 差异：2=TargetOnly, 3=SourceOnly, 5=ContentMismatch
        let source_rows = vec![make_row(1, "A"), make_row(3, "C"), make_row(5, "E")];
        let target_rows = vec![make_row(1, "A"), make_row(2, "B"), make_row(5, "modified")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();
        assert_eq!(result.total_diffs, 3);

        let plan = cmp.generate_repair_plan(&result, RepairStrategy::SourceWins);
        // SourceOnly(3)→Insert, ContentMismatch(5)→Update, TargetOnly(2)→Delete
        assert_eq!(plan.total_inserts, 1);
        assert_eq!(plan.total_updates, 1);
        assert_eq!(plan.total_deletes, 1);
        assert_eq!(plan.actions.len(), 3);
        assert_eq!(plan.total_actions(), 3);
        assert!(!plan.is_empty());

        // 验证动作类型
        let has_insert = plan
            .actions
            .iter()
            .any(|a| matches!(a, RepairAction::Insert { .. }));
        let has_update = plan
            .actions
            .iter()
            .any(|a| matches!(a, RepairAction::Update { .. }));
        let has_delete = plan
            .actions
            .iter()
            .any(|a| matches!(a, RepairAction::Delete { .. }));
        assert!(has_insert && has_update && has_delete);
    }

    #[test]
    fn repair_plan_target_wins() {
        // 目标端胜策略（反向）：
        // - SourceOnly → Delete（源端删行）
        // - ContentMismatch → Update（源端更新）
        // - TargetOnly → Insert（源端补行）
        let schema = make_schema(1, "users");
        let source_rows = vec![make_row(1, "A"), make_row(3, "C"), make_row(5, "E")];
        let target_rows = vec![make_row(1, "A"), make_row(2, "B"), make_row(5, "modified")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        let plan = cmp.generate_repair_plan(&result, RepairStrategy::TargetWins);
        // SourceOnly(3)→Delete, ContentMismatch(5)→Update, TargetOnly(2)→Insert
        assert_eq!(plan.total_inserts, 1); // TargetOnly → Insert
        assert_eq!(plan.total_updates, 1); // ContentMismatch → Update
        assert_eq!(plan.total_deletes, 1); // SourceOnly → Delete
    }

    #[test]
    fn repair_plan_dry_run() {
        // DryRun 策略：不生成任何动作
        let schema = make_schema(1, "users");
        let source_rows = vec![make_row(1, "A"), make_row(3, "C")];
        let target_rows = vec![make_row(1, "A"), make_row(2, "B")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();
        assert!(result.total_diffs > 0);

        let plan = cmp.generate_repair_plan(&result, RepairStrategy::DryRun);
        assert!(plan.is_empty());
        assert_eq!(plan.total_actions(), 0);
        assert!(plan.actions.is_empty());
    }

    #[test]
    fn repair_plan_empty_when_consistent() {
        // 无差异时修复计划应为空
        let schema = make_schema(1, "users");
        let rows = vec![make_row(1, "A"), make_row(2, "B")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", rows.clone());
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();
        assert!(result.all_consistent);

        let plan = cmp.generate_repair_plan(&result, RepairStrategy::SourceWins);
        assert!(plan.is_empty());
    }

    #[test]
    fn repair_plan_source_wins_insert_action_fields() {
        // 验证 Insert 动作的字段正确性
        let schema = make_schema(1, "users");
        let source_rows = vec![make_row(42, "Alice")];
        let target_rows: Vec<DecodedRow> = vec![];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();
        let plan = cmp.generate_repair_plan(&result, RepairStrategy::SourceWins);

        assert_eq!(plan.actions.len(), 1);
        match &plan.actions[0] {
            RepairAction::Insert { table, row } => {
                assert_eq!(table, "users");
                assert_eq!(row.columns.len(), 2);
                // 验证 pk 值
                let pk = row.columns.iter().find(|(n, _)| n == "id");
                assert!(matches!(pk, Some((_, SzValue::Int64(42)))));
            }
            other => panic!("expected Insert, got {other:?}"),
        }
    }

    #[test]
    fn repair_plan_source_wins_delete_action_fields() {
        // 验证 Delete 动作的 pk_value 正确提取
        let schema = make_schema(1, "users");
        let source_rows: Vec<DecodedRow> = vec![];
        let target_rows = vec![make_row(99, "Bob")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();
        let plan = cmp.generate_repair_plan(&result, RepairStrategy::SourceWins);

        assert_eq!(plan.actions.len(), 1);
        match &plan.actions[0] {
            RepairAction::Delete { table, pk_value } => {
                assert_eq!(table, "users");
                assert!(matches!(pk_value, SzValue::Int64(99)));
            }
            other => panic!("expected Delete, got {other:?}"),
        }
    }

    // =================================================================
    // P5-4 报告输出测试
    // =================================================================

    #[test]
    fn report_markdown_consistent() {
        let schema = make_schema(1, "users");
        let rows = vec![make_row(1, "A"), make_row(2, "B")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", rows.clone());
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        let report = ComparisonReport::new(result);
        let md = report.to_markdown();

        assert!(md.contains("# 数据比对报告"));
        assert!(md.contains("总体一致性: ✅ 一致"));
        assert!(md.contains("源端总行数: 2"));
        assert!(md.contains("目标端总行数: 2"));
        assert!(md.contains("差异行数: 0"));
        assert!(md.contains("users"));
    }

    #[test]
    fn report_markdown_with_diffs() {
        let schema = make_schema(1, "users");
        let source_rows = vec![make_row(1, "Alice"), make_row(3, "C")];
        let target_rows = vec![make_row(1, "Alice"), make_row(2, "B")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        let report = ComparisonReport::new(result);
        let md = report.to_markdown();

        assert!(md.contains("❌ 存在差异"));
        assert!(md.contains("差异行数: 2"));
        // 差异样本应包含差异类型描述
        assert!(md.contains("差异样本") || md.contains("源端缺失") || md.contains("目标端多余"));
    }

    #[test]
    fn report_json_format() {
        let schema = make_schema(1, "users");
        let rows = vec![make_row(1, "A")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", rows.clone());
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        let report = ComparisonReport::new(result);
        let json = report.to_json().unwrap();

        // 验证 JSON 关键字段
        assert!(json.contains("\"all_consistent\""));
        assert!(json.contains("\"total_source_rows\""));
        assert!(json.contains("\"total_target_rows\""));
        assert!(json.contains("\"tables\""));
        assert!(json.contains("\"table_name\""));
        assert!(json.contains("\"users\""));

        // 验证可被解析为有效 JSON
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["all_consistent"], true);
        assert_eq!(parsed["total_source_rows"], 1);
    }

    #[test]
    fn report_json_with_diffs() {
        let schema = make_schema(1, "users");
        let source_rows = vec![make_row(1, "Alice"), make_row(3, "C")];
        let target_rows = vec![make_row(1, "Alice"), make_row(2, "B")];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        let report = ComparisonReport::new(result);
        let json = report.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["all_consistent"], false);
        assert_eq!(parsed["total_diffs"], 2);
        // 差异列表应非空
        let diffs = &parsed["tables"][0]["differences"];
        assert!(diffs.is_array());
        assert!(diffs.as_array().unwrap().len() == 2);
    }

    #[test]
    fn report_markdown_empty_tables() {
        // 没有共同表的场景
        let schema1 = make_schema(1, "table_only_in_source");
        let schema2 = make_schema(2, "table_only_in_target");

        let source = MemoryComparisonSource::new(vec![schema1])
            .with_data("table_only_in_source", vec![make_row(1, "A")]);
        let target = MemoryComparisonSource::new(vec![schema2])
            .with_data("table_only_in_target", vec![make_row(1, "A")]);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());
        let result = cmp.compare().unwrap();

        let report = ComparisonReport::new(result);
        let md = report.to_markdown();
        assert!(md.contains("（无比对表）"));
    }

    #[test]
    fn report_generated_at_nonzero() {
        let result = ComparisonResult::default();
        let report = ComparisonReport::new(result);
        // 生成时间应为有效的 Unix 毫秒时间戳（> 0）
        assert!(report.generated_at > 0);
    }

    // =================================================================
    // P5-4 综合场景测试
    // =================================================================

    #[test]
    fn end_to_end_compare_repair_report() {
        // 端到端：比对 → 生成修复计划 → 输出报告
        let schema = make_schema(1, "users");
        let source_rows = vec![
            make_row(1, "Alice"),
            make_row(2, "Bob"),
            make_row(3, "Charlie"),
        ];
        // 目标端：缺 2，多 4，3 的内容不一致
        let target_rows = vec![
            make_row(1, "Alice"),
            make_row(3, "modified"),
            make_row(4, "Dave"),
        ];

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());

        // 1. 全量比对
        let result = cmp.compare().unwrap();
        assert!(!result.all_consistent);
        assert_eq!(result.total_diffs, 3);

        // 2. 生成 SourceWins 修复计划
        let plan = cmp.generate_repair_plan(&result, RepairStrategy::SourceWins);
        // SourceOnly(2)→Insert, ContentMismatch(3)→Update, TargetOnly(4)→Delete
        assert_eq!(plan.total_inserts, 1);
        assert_eq!(plan.total_updates, 1);
        assert_eq!(plan.total_deletes, 1);

        // 3. 输出 Markdown 报告
        let report = ComparisonReport::new(result.clone());
        let md = report.to_markdown();
        assert!(md.contains("❌ 存在差异"));
        assert!(md.contains("差异行数: 3"));

        // 4. 输出 JSON 报告
        let json = report.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["total_diffs"], 3);
    }

    #[test]
    fn incremental_then_repair_workflow() {
        // 增量比对 + 修复计划工作流
        let schema = make_schema(1, "users");
        // 源端 1..=20，目标端 1..=20，但 pk=15 内容不一致
        let source_rows: Vec<DecodedRow> =
            (1..=20).map(|i| make_row(i, &format!("u{i}"))).collect();
        let mut target_rows: Vec<DecodedRow> =
            (1..=20).map(|i| make_row(i, &format!("u{i}"))).collect();
        target_rows[14] = make_row(15, "modified");

        let source =
            MemoryComparisonSource::new(vec![schema.clone()]).with_data("users", source_rows);
        let target = MemoryComparisonSource::new(vec![schema]).with_data("users", target_rows);

        let cmp = DataComparison::new(source, target, ComparisonConfig::default());

        // 增量比对：从 pk > 10 开始
        let cfg = IncrementalConfig {
            last_compared_pk: Some(SzValue::Int64(10)),
            last_compared_at: 0,
        };
        let result = cmp.compare_incremental(&cfg).unwrap();

        // 增量范围 (11..=20) 内应检测到 1 个差异（pk=15）
        assert!(!result.base.all_consistent);
        assert_eq!(result.base.total_diffs, 1);
        assert_eq!(result.incremental_rows, 10);

        // 生成修复计划
        let plan = cmp.generate_repair_plan(&result.base, RepairStrategy::SourceWins);
        assert_eq!(plan.total_updates, 1);
        assert!(plan
            .actions
            .iter()
            .all(|a| matches!(a, RepairAction::Update { .. })));
    }
}
