//! 外部列存格式 — Phase 7f.1
//!
//! 对应 `SzRSQL技术实现方案.md` HTAP 外部格式读取设计。
//!
//! # 设计
//!
//! 支持读取 4 种外部格式，统一通过 `ExternalReader` trait 接口访问：
//!
//! - **Arrow IPC** — Apache Arrow 二进制列存格式（`arrow::ipc`）
//! - **Parquet** — 列式存储格式（`parquet::arrow`），支持谓词下推
//! - **CSV** — 逗号分隔文本格式（`arrow::csv`）
//! - **JSONLines** — JSON Lines 格式（每行一个 JSON 对象，`serde_json`）
//!
//! ## 核心特性
//!
//! 1. **列裁剪（Column Pruning）** — `ReadOptions::columns` 指定只读取部分列
//! 2. **谓词下推（Predicate Pushdown）** — `ReadOptions::predicate` 指定行过滤条件
//!    - Parquet 使用 `RowFilter` 原生谓词下推（在读数据时过滤，避免加载不满足条件的行）
//!    - 其他格式在读取后进行行过滤
//!
//! # 验证标准
//!
//! - `SELECT * FROM arrow('data.arrow')` → 读取 Arrow IPC
//! - `SELECT * FROM parquet('data.parquet') WHERE x > 10` → 谓词下推
//! - 列裁剪只读取查询列
//! - Arrow/Parquet/CSV/JSONLines 正确读取
//!
//! 对应 `SzRSQL实施进度.md` Phase 7f.1。

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, Write};
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

// =====================================================================
//  错误类型
// =====================================================================

/// 外部格式错误
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExternalFormatError {
    /// 文件不存在
    #[error("file not found: {0}")]
    FileNotFound(String),
    /// 不支持的格式
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    /// 格式解析错误
    #[error("parse error: {0}")]
    ParseError(String),
    /// 列不存在
    #[error("column not found: {0}")]
    ColumnNotFound(String),
    /// 类型不匹配
    #[error("type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },
    /// IO 错误
    #[error("io error: {0}")]
    IoError(String),
    /// Arrow 错误
    #[error("arrow error: {0}")]
    ArrowError(String),
    /// Parquet 错误
    #[error("parquet error: {0}")]
    ParquetError(String),
}

impl From<arrow::error::ArrowError> for ExternalFormatError {
    fn from(e: arrow::error::ArrowError) -> Self {
        ExternalFormatError::ArrowError(e.to_string())
    }
}

impl From<parquet::errors::ParquetError> for ExternalFormatError {
    fn from(e: parquet::errors::ParquetError) -> Self {
        ExternalFormatError::ParquetError(e.to_string())
    }
}

impl From<std::io::Error> for ExternalFormatError {
    fn from(e: std::io::Error) -> Self {
        ExternalFormatError::IoError(e.to_string())
    }
}

// =====================================================================
//  外部类型系统
// =====================================================================

/// 外部列类型（与 Arrow DataType 对应）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExternalType {
    /// 64 位整数
    Int64,
    /// 64 位浮点数
    Float64,
    /// 字符串
    Text,
    /// 布尔值
    Bool,
}

impl ExternalType {
    /// 转换为 Arrow DataType
    pub fn to_arrow(&self) -> DataType {
        match self {
            ExternalType::Int64 => DataType::Int64,
            ExternalType::Float64 => DataType::Float64,
            ExternalType::Text => DataType::Utf8,
            ExternalType::Bool => DataType::Boolean,
        }
    }

    /// 从 Arrow DataType 转换
    pub fn from_arrow(dt: &DataType) -> Result<Self, ExternalFormatError> {
        match dt {
            DataType::Int64 => Ok(ExternalType::Int64),
            DataType::Float64 => Ok(ExternalType::Float64),
            DataType::Utf8 => Ok(ExternalType::Text),
            DataType::Boolean => Ok(ExternalType::Bool),
            other => Err(ExternalFormatError::TypeMismatch {
                expected: "Int64/Float64/Utf8/Boolean".to_string(),
                actual: other.to_string(),
            }),
        }
    }

    /// 类型名称
    pub fn as_str(&self) -> &'static str {
        match self {
            ExternalType::Int64 => "Int64",
            ExternalType::Float64 => "Float64",
            ExternalType::Text => "Text",
            ExternalType::Bool => "Bool",
        }
    }
}

impl std::fmt::Display for ExternalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 外部列定义
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalColumn {
    /// 列名
    pub name: String,
    /// 列类型
    pub col_type: ExternalType,
}

impl ExternalColumn {
    /// 创建列定义
    pub fn new(name: impl Into<String>, col_type: ExternalType) -> Self {
        Self {
            name: name.into(),
            col_type,
        }
    }

    /// 转换为 Arrow Field
    pub fn to_field(&self) -> Field {
        Field::new(&self.name, self.col_type.to_arrow(), true)
    }

    /// 从 Arrow Field 转换
    pub fn from_field(field: &Field) -> Result<Self, ExternalFormatError> {
        Ok(Self {
            name: field.name().clone(),
            col_type: ExternalType::from_arrow(field.data_type())?,
        })
    }
}

/// 外部 Schema
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSchema {
    /// 列定义
    pub columns: Vec<ExternalColumn>,
}

impl ExternalSchema {
    /// 创建空 schema
    pub fn new() -> Self {
        Self {
            columns: Vec::new(),
        }
    }

    /// 从列定义创建
    pub fn from_columns(columns: Vec<ExternalColumn>) -> Self {
        Self { columns }
    }

    /// 列数
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// 列名列表
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.name.as_str()).collect()
    }

    /// 查找列索引
    pub fn find_column(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// 转换为 Arrow Schema
    pub fn to_arrow(&self) -> Schema {
        let fields: Vec<Field> = self.columns.iter().map(|c| c.to_field()).collect();
        Schema::new(fields)
    }

    /// 从 Arrow Schema 转换
    pub fn from_arrow(schema: &Schema) -> Result<Self, ExternalFormatError> {
        let columns: Vec<ExternalColumn> = schema
            .fields()
            .iter()
            .map(|f| ExternalColumn::from_field(f))
            .collect::<Result<_, _>>()?;
        Ok(Self { columns })
    }
}

impl Default for ExternalSchema {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  外部值与行
// =====================================================================

/// 外部值（单单元格）
#[derive(Debug, Clone, PartialEq)]
pub enum ExternalValue {
    /// 64 位整数
    Int64(i64),
    /// 64 位浮点数
    Float64(f64),
    /// 字符串
    Text(String),
    /// 布尔值
    Bool(bool),
    /// NULL
    Null,
}

impl ExternalValue {
    /// 是否为 NULL
    pub fn is_null(&self) -> bool {
        matches!(self, ExternalValue::Null)
    }

    /// 获取类型
    pub fn external_type(&self) -> Option<ExternalType> {
        match self {
            ExternalValue::Int64(_) => Some(ExternalType::Int64),
            ExternalValue::Float64(_) => Some(ExternalType::Float64),
            ExternalValue::Text(_) => Some(ExternalType::Text),
            ExternalValue::Bool(_) => Some(ExternalType::Bool),
            ExternalValue::Null => None,
        }
    }

    /// 比较运算（self > other）
    pub fn greater_than(&self, other: &ExternalValue) -> bool {
        match (self, other) {
            (ExternalValue::Int64(a), ExternalValue::Int64(b)) => a > b,
            (ExternalValue::Float64(a), ExternalValue::Float64(b)) => a > b,
            (ExternalValue::Text(a), ExternalValue::Text(b)) => a > b,
            (ExternalValue::Bool(a), ExternalValue::Bool(b)) => a & !b,
            _ => false,
        }
    }

    /// 比较运算（self < other）
    pub fn less_than(&self, other: &ExternalValue) -> bool {
        match (self, other) {
            (ExternalValue::Int64(a), ExternalValue::Int64(b)) => a < b,
            (ExternalValue::Float64(a), ExternalValue::Float64(b)) => a < b,
            (ExternalValue::Text(a), ExternalValue::Text(b)) => a < b,
            (ExternalValue::Bool(a), ExternalValue::Bool(b)) => !a & b,
            _ => false,
        }
    }

    /// 比较运算（self == other）
    pub fn equals(&self, other: &ExternalValue) -> bool {
        match (self, other) {
            (ExternalValue::Int64(a), ExternalValue::Int64(b)) => a == b,
            (ExternalValue::Float64(a), ExternalValue::Float64(b)) => a == b,
            (ExternalValue::Text(a), ExternalValue::Text(b)) => a == b,
            (ExternalValue::Bool(a), ExternalValue::Bool(b)) => a == b,
            (ExternalValue::Null, ExternalValue::Null) => false, // NULL != NULL
            _ => false,
        }
    }
}

/// 外部行
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalRow {
    /// 行数据（按 schema 列顺序）
    pub values: Vec<ExternalValue>,
}

impl ExternalRow {
    /// 创建空行
    pub fn new() -> Self {
        Self { values: Vec::new() }
    }

    /// 从值列表创建
    pub fn from_values(values: Vec<ExternalValue>) -> Self {
        Self { values }
    }

    /// 列数
    pub fn column_count(&self) -> usize {
        self.values.len()
    }

    /// 获取列值
    pub fn get(&self, index: usize) -> Option<&ExternalValue> {
        self.values.get(index)
    }
}

impl Default for ExternalRow {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  ReadOptions — 列裁剪 + 谓词下推
// =====================================================================

/// 谓词条件（WHERE 子句）
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    /// column > value
    Gt(String, ExternalValue),
    /// column < value
    Lt(String, ExternalValue),
    /// column = value
    Eq(String, ExternalValue),
    /// AND
    And(Box<Predicate>, Box<Predicate>),
    /// OR
    Or(Box<Predicate>, Box<Predicate>),
}

impl Predicate {
    /// 创建 `column > value` 谓词
    pub fn gt(column: impl Into<String>, value: ExternalValue) -> Self {
        Predicate::Gt(column.into(), value)
    }

    /// 创建 `column < value` 谓词
    pub fn lt(column: impl Into<String>, value: ExternalValue) -> Self {
        Predicate::Lt(column.into(), value)
    }

    /// 创建 `column = value` 谓词
    pub fn eq(column: impl Into<String>, value: ExternalValue) -> Self {
        Predicate::Eq(column.into(), value)
    }

    /// AND 组合
    pub fn and(self, other: Predicate) -> Self {
        Predicate::And(Box::new(self), Box::new(other))
    }

    /// OR 组合
    pub fn or(self, other: Predicate) -> Self {
        Predicate::Or(Box::new(self), Box::new(other))
    }

    /// 对行求值（是否满足谓词条件）
    pub fn evaluate(&self, row: &ExternalRow, schema: &ExternalSchema) -> bool {
        match self {
            Predicate::Gt(col, val) => {
                if let Some(idx) = schema.find_column(col) {
                    if let Some(cell) = row.get(idx) {
                        return cell.greater_than(val);
                    }
                }
                false
            }
            Predicate::Lt(col, val) => {
                if let Some(idx) = schema.find_column(col) {
                    if let Some(cell) = row.get(idx) {
                        return cell.less_than(val);
                    }
                }
                false
            }
            Predicate::Eq(col, val) => {
                if let Some(idx) = schema.find_column(col) {
                    if let Some(cell) = row.get(idx) {
                        return cell.equals(val);
                    }
                }
                false
            }
            Predicate::And(a, b) => a.evaluate(row, schema) && b.evaluate(row, schema),
            Predicate::Or(a, b) => a.evaluate(row, schema) || b.evaluate(row, schema),
        }
    }

    /// 获取谓词引用的所有列名
    pub fn referenced_columns(&self) -> Vec<String> {
        match self {
            Predicate::Gt(col, _) | Predicate::Lt(col, _) | Predicate::Eq(col, _) => {
                vec![col.clone()]
            }
            Predicate::And(a, b) | Predicate::Or(a, b) => {
                let mut cols = a.referenced_columns();
                cols.extend(b.referenced_columns());
                cols
            }
        }
    }
}

/// 读取选项 — 列裁剪 + 谓词下推
#[derive(Debug, Clone, Default)]
pub struct ReadOptions {
    /// 列裁剪：None = 读取所有列，Some = 只读取指定列
    pub columns: Option<Vec<String>>,
    /// 谓词下推：None = 读取所有行，Some = 只读取满足条件的行
    pub predicate: Option<Predicate>,
}

impl ReadOptions {
    /// 读取所有列、所有行
    pub fn all() -> Self {
        Self::default()
    }

    /// 只读取指定列
    pub fn with_columns(mut self, columns: Vec<String>) -> Self {
        self.columns = Some(columns);
        self
    }

    /// 添加谓词条件
    pub fn with_predicate(mut self, predicate: Predicate) -> Self {
        self.predicate = Some(predicate);
        self
    }

    /// 是否需要列裁剪
    pub fn needs_column_pruning(&self) -> bool {
        self.columns.is_some()
    }

    /// 是否需要谓词过滤
    pub fn needs_predicate_filter(&self) -> bool {
        self.predicate.is_some()
    }

    /// 获取需要读取的所有列（列裁剪 + 谓词引用列的并集）
    pub fn effective_columns(&self, schema: &ExternalSchema) -> Vec<String> {
        let mut cols: Vec<String> = self
            .columns
            .clone()
            .unwrap_or_else(|| schema.columns.iter().map(|c| c.name.clone()).collect());

        if let Some(ref pred) = self.predicate {
            for col in pred.referenced_columns() {
                if !cols.contains(&col) {
                    cols.push(col);
                }
            }
        }

        cols
    }
}

// =====================================================================
//  Arrow 转换辅助函数
// =====================================================================

/// 将 RecordBatch 转换为 Vec<ExternalRow>
fn record_batch_to_rows(batch: &RecordBatch) -> Result<Vec<ExternalRow>, ExternalFormatError> {
    let num_rows = batch.num_rows();
    let num_cols = batch.num_columns();
    let mut rows = Vec::with_capacity(num_rows);

    for row_idx in 0..num_rows {
        let mut values = Vec::with_capacity(num_cols);
        for col_idx in 0..num_cols {
            let array = batch.column(col_idx);
            let value = array_to_value(array.as_ref(), row_idx)?;
            values.push(value);
        }
        rows.push(ExternalRow { values });
    }

    Ok(rows)
}

/// 从 Arrow array 提取单个值
fn array_to_value(array: &dyn Array, index: usize) -> Result<ExternalValue, ExternalFormatError> {
    if array.is_null(index) {
        return Ok(ExternalValue::Null);
    }
    match array.data_type() {
        DataType::Int64 => {
            let arr = array.as_any().downcast_ref::<Int64Array>().ok_or_else(|| {
                ExternalFormatError::ArrowError("downcast Int64Array failed".into())
            })?;
            Ok(ExternalValue::Int64(arr.value(index)))
        }
        DataType::Float64 => {
            let arr = array
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| {
                    ExternalFormatError::ArrowError("downcast Float64Array failed".into())
                })?;
            Ok(ExternalValue::Float64(arr.value(index)))
        }
        DataType::Utf8 => {
            let arr = array
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| {
                    ExternalFormatError::ArrowError("downcast StringArray failed".into())
                })?;
            Ok(ExternalValue::Text(arr.value(index).to_string()))
        }
        DataType::Boolean => {
            let arr = array
                .as_any()
                .downcast_ref::<BooleanArray>()
                .ok_or_else(|| {
                    ExternalFormatError::ArrowError("downcast BooleanArray failed".into())
                })?;
            Ok(ExternalValue::Bool(arr.value(index)))
        }
        other => Err(ExternalFormatError::TypeMismatch {
            expected: "Int64/Float64/Utf8/Boolean".into(),
            actual: other.to_string(),
        }),
    }
}

/// 将 Vec<ExternalRow> 转换为 RecordBatch
fn rows_to_record_batch(
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<RecordBatch, ExternalFormatError> {
    let arrow_schema = Arc::new(schema.to_arrow());
    let num_cols = schema.column_count();
    let num_rows = rows.len();

    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(num_cols);

    for col_idx in 0..num_cols {
        let col_type = schema.columns[col_idx].col_type;
        let array: ArrayRef = match col_type {
            ExternalType::Int64 => {
                let mut builder = Int64Array::builder(num_rows);
                for row in rows {
                    match row.get(col_idx) {
                        Some(ExternalValue::Int64(v)) => builder.append_value(*v),
                        Some(ExternalValue::Null) | None => builder.append_null(),
                        Some(v) => {
                            return Err(ExternalFormatError::TypeMismatch {
                                expected: "Int64".into(),
                                actual: format!("{v:?}"),
                            })
                        }
                    }
                }
                Arc::new(builder.finish())
            }
            ExternalType::Float64 => {
                let mut builder = Float64Array::builder(num_rows);
                for row in rows {
                    match row.get(col_idx) {
                        Some(ExternalValue::Float64(v)) => builder.append_value(*v),
                        Some(ExternalValue::Null) | None => builder.append_null(),
                        Some(v) => {
                            return Err(ExternalFormatError::TypeMismatch {
                                expected: "Float64".into(),
                                actual: format!("{v:?}"),
                            })
                        }
                    }
                }
                Arc::new(builder.finish())
            }
            ExternalType::Text => {
                let mut values: Vec<Option<&str>> = Vec::with_capacity(num_rows);
                for row in rows {
                    match row.get(col_idx) {
                        Some(ExternalValue::Text(v)) => values.push(Some(v.as_str())),
                        Some(ExternalValue::Null) | None => values.push(None),
                        Some(v) => {
                            return Err(ExternalFormatError::TypeMismatch {
                                expected: "Text".into(),
                                actual: format!("{v:?}"),
                            })
                        }
                    }
                }
                Arc::new(StringArray::from(values))
            }
            ExternalType::Bool => {
                let mut builder = BooleanArray::builder(num_rows);
                for row in rows {
                    match row.get(col_idx) {
                        Some(ExternalValue::Bool(v)) => builder.append_value(*v),
                        Some(ExternalValue::Null) | None => builder.append_null(),
                        Some(v) => {
                            return Err(ExternalFormatError::TypeMismatch {
                                expected: "Bool".into(),
                                actual: format!("{v:?}"),
                            })
                        }
                    }
                }
                Arc::new(builder.finish())
            }
        };
        arrays.push(array);
    }

    RecordBatch::try_new(arrow_schema, arrays).map_err(ExternalFormatError::from)
}

/// 列裁剪：从 RecordBatch 中只选取指定列
fn prune_columns(
    batch: &RecordBatch,
    schema: &ExternalSchema,
    columns: &[String],
) -> Result<RecordBatch, ExternalFormatError> {
    let mut indices = Vec::with_capacity(columns.len());
    for col_name in columns {
        let idx = schema
            .find_column(col_name)
            .ok_or_else(|| ExternalFormatError::ColumnNotFound(col_name.clone()))?;
        indices.push(idx);
    }

    let pruned_fields: Vec<Field> = indices
        .iter()
        .map(|&i| schema.columns[i].to_field())
        .collect();
    let pruned_schema = Arc::new(Schema::new(pruned_fields));
    let pruned_arrays: Vec<ArrayRef> = indices.iter().map(|&i| batch.column(i).clone()).collect();

    RecordBatch::try_new(pruned_schema, pruned_arrays).map_err(ExternalFormatError::from)
}

/// 谓词过滤：过滤 RecordBatch 中不满足谓词条件的行
fn filter_rows_by_predicate(
    batch: &RecordBatch,
    schema: &ExternalSchema,
    predicate: &Predicate,
) -> Result<RecordBatch, ExternalFormatError> {
    let rows = record_batch_to_rows(batch)?;
    let filtered: Vec<&ExternalRow> = rows
        .iter()
        .filter(|row| predicate.evaluate(row, schema))
        .collect();

    let filtered_rows: Vec<ExternalRow> = filtered.iter().map(|r| (*r).clone()).collect();
    rows_to_record_batch(schema, &filtered_rows)
}

// =====================================================================
//  格式检测
// =====================================================================

/// 外部格式类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExternalFormat {
    /// Arrow IPC
    Arrow,
    /// Parquet
    Parquet,
    /// CSV
    Csv,
    /// JSON Lines
    JsonLines,
}

impl ExternalFormat {
    /// 从文件扩展名推断格式
    pub fn from_path(path: &str) -> Result<Self, ExternalFormatError> {
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| ExternalFormatError::UnsupportedFormat(path.to_string()))?;

        match ext.to_lowercase().as_str() {
            "arrow" | "ipc" => Ok(ExternalFormat::Arrow),
            "parquet" | "pq" => Ok(ExternalFormat::Parquet),
            "csv" | "tsv" => Ok(ExternalFormat::Csv),
            "jsonl" | "ndjson" | "json" => Ok(ExternalFormat::JsonLines),
            other => Err(ExternalFormatError::UnsupportedFormat(format!(".{other}"))),
        }
    }

    /// 格式名称
    pub fn as_str(&self) -> &'static str {
        match self {
            ExternalFormat::Arrow => "arrow",
            ExternalFormat::Parquet => "parquet",
            ExternalFormat::Csv => "csv",
            ExternalFormat::JsonLines => "jsonl",
        }
    }
}

// =====================================================================
//  ExternalReader trait
// =====================================================================

/// 外部格式读取器 trait
pub trait ExternalReader {
    /// 返回 schema
    fn schema(&self) -> &ExternalSchema;

    /// 读取数据（支持列裁剪 + 谓词下推）
    fn read(&self, options: &ReadOptions) -> Result<Vec<ExternalRow>, ExternalFormatError>;
}

// =====================================================================
//  Arrow IPC Reader / Writer
// =====================================================================

/// Arrow IPC 读取器
pub struct ArrowReader {
    schema: ExternalSchema,
    batches: Vec<RecordBatch>,
}

impl ArrowReader {
    /// 从文件读取 Arrow IPC
    pub fn from_file(path: &str) -> Result<Self, ExternalFormatError> {
        let file = File::open(path).map_err(|_| ExternalFormatError::FileNotFound(path.into()))?;
        Self::from_reader(BufReader::new(file))
    }

    /// 从字节读取 Arrow IPC
    pub fn from_bytes(data: &[u8]) -> Result<Self, ExternalFormatError> {
        let data = data.to_vec();
        Self::from_reader(std::io::Cursor::new(data))
    }

    /// 从任意 Read 读取 Arrow IPC
    pub fn from_reader<R: Read + Seek + Send + 'static>(
        reader: R,
    ) -> Result<Self, ExternalFormatError> {
        let mut arrow_reader = arrow::ipc::reader::FileReader::try_new(reader, None)?;
        let arrow_schema = arrow_reader.schema();
        let schema = ExternalSchema::from_arrow(arrow_schema.as_ref())?;

        let mut batches = Vec::new();
        for batch in arrow_reader.by_ref() {
            batches.push(batch?);
        }

        Ok(Self { schema, batches })
    }
}

impl ExternalReader for ArrowReader {
    fn schema(&self) -> &ExternalSchema {
        &self.schema
    }

    fn read(&self, options: &ReadOptions) -> Result<Vec<ExternalRow>, ExternalFormatError> {
        let mut all_rows = Vec::new();

        for batch in &self.batches {
            let processed = process_batch(batch, &self.schema, options)?;
            let rows = record_batch_to_rows(&processed)?;
            all_rows.extend(rows);
        }

        Ok(all_rows)
    }
}

/// 写入 Arrow IPC 文件
pub fn write_arrow_file(
    path: &str,
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<(), ExternalFormatError> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    write_arrow_to_writer(&mut writer, schema, rows)?;
    writer.flush()?;
    Ok(())
}

/// 将 Arrow IPC 写入字节
pub fn write_arrow_bytes(
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<Vec<u8>, ExternalFormatError> {
    let mut buf = Vec::new();
    write_arrow_to_writer(&mut buf, schema, rows)?;
    Ok(buf)
}

fn write_arrow_to_writer<W: Write + Send>(
    writer: W,
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<(), ExternalFormatError> {
    let arrow_schema = Arc::new(schema.to_arrow());
    let batch = rows_to_record_batch(schema, rows)?;

    let mut ipc_writer = arrow::ipc::writer::FileWriter::try_new(writer, arrow_schema.as_ref())?;
    if batch.num_rows() > 0 {
        ipc_writer.write(&batch)?;
    }
    ipc_writer.finish()?;
    Ok(())
}

// =====================================================================
//  Parquet Reader / Writer（含谓词下推）
// =====================================================================

/// Parquet 读取器
pub struct ParquetReader {
    schema: ExternalSchema,
    batches: Vec<RecordBatch>,
}

impl ParquetReader {
    /// 从文件读取 Parquet
    pub fn from_file(path: &str) -> Result<Self, ExternalFormatError> {
        let file = File::open(path).map_err(|_| ExternalFormatError::FileNotFound(path.into()))?;
        Self::from_chunk_reader(file)
    }

    /// 从字节读取 Parquet
    pub fn from_bytes(data: &[u8]) -> Result<Self, ExternalFormatError> {
        let bytes = bytes::Bytes::copy_from_slice(data);
        Self::from_chunk_reader(bytes)
    }

    /// 从任意 Read 读取 Parquet（读取全部数据到内存后解析）
    fn from_reader<R: Read + Send + 'static>(mut reader: R) -> Result<Self, ExternalFormatError> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        let bytes = bytes::Bytes::from(buf);
        Self::from_chunk_reader(bytes)
    }

    /// 从 ChunkReader 读取 Parquet（核心实现）
    pub fn from_chunk_reader<T: parquet::file::reader::ChunkReader + 'static>(
        reader: T,
    ) -> Result<Self, ExternalFormatError> {
        let builder =
            parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(reader)?;
        let arrow_schema = builder.schema().clone();
        let schema = ExternalSchema::from_arrow(arrow_schema.as_ref())?;

        let reader = builder.build()?;
        let mut batches = Vec::new();
        for batch in reader {
            batches.push(batch?);
        }

        Ok(Self { schema, batches })
    }

    /// 从文件读取 Parquet（带谓词下推）
    ///
    /// 使用 Parquet RowFilter 在读取数据时过滤行，避免加载不满足条件的行。
    pub fn from_file_with_predicate(
        path: &str,
        predicate: &Predicate,
    ) -> Result<Self, ExternalFormatError> {
        let file = File::open(path).map_err(|_| ExternalFormatError::FileNotFound(path.into()))?;
        Self::from_chunk_reader_with_predicate(file, predicate)
    }

    /// 从任意 Read 读取 Parquet（带谓词下推，读取全部数据到内存后解析）
    fn from_reader_with_predicate<R: Read + Send + 'static>(
        mut reader: R,
        predicate: &Predicate,
    ) -> Result<Self, ExternalFormatError> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        let bytes = bytes::Bytes::from(buf);
        Self::from_chunk_reader_with_predicate(bytes, predicate)
    }

    /// 从 ChunkReader 读取 Parquet（带谓词下推，核心实现）
    pub fn from_chunk_reader_with_predicate<T: parquet::file::reader::ChunkReader + 'static>(
        reader: T,
        predicate: &Predicate,
    ) -> Result<Self, ExternalFormatError> {
        let builder =
            parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(reader)?;
        let arrow_schema = builder.schema().clone();
        let schema = ExternalSchema::from_arrow(arrow_schema.as_ref())?;

        // 构建 Parquet RowFilter（谓词下推到 Parquet 读取器）
        let filter = build_parquet_row_filter(predicate, &schema)?;
        let reader = builder.with_row_filter(filter).build()?;

        let mut batches = Vec::new();
        for batch in reader {
            batches.push(batch?);
        }

        Ok(Self { schema, batches })
    }
}

impl ExternalReader for ParquetReader {
    fn schema(&self) -> &ExternalSchema {
        &self.schema
    }

    fn read(&self, options: &ReadOptions) -> Result<Vec<ExternalRow>, ExternalFormatError> {
        let mut all_rows = Vec::new();

        for batch in &self.batches {
            let processed = process_batch(batch, &self.schema, options)?;
            let rows = record_batch_to_rows(&processed)?;
            all_rows.extend(rows);
        }

        Ok(all_rows)
    }
}

/// 构建 Parquet RowFilter（将 Predicate 转换为 Arrow 布尔数组过滤器）
fn build_parquet_row_filter(
    predicate: &Predicate,
    schema: &ExternalSchema,
) -> Result<parquet::arrow::arrow_reader::RowFilter, ExternalFormatError> {
    let pred = PredicateArrowAdapter {
        predicate: predicate.clone(),
        schema: schema.clone(),
        projection: parquet::arrow::ProjectionMask::all(),
    };
    let filter = parquet::arrow::arrow_reader::RowFilter::new(vec![Box::new(pred)]);
    Ok(filter)
}

/// Predicate → ArrowPredicate 适配器（用于 Parquet 谓词下推）
struct PredicateArrowAdapter {
    predicate: Predicate,
    schema: ExternalSchema,
    projection: parquet::arrow::ProjectionMask,
}

impl parquet::arrow::arrow_reader::ArrowPredicate for PredicateArrowAdapter {
    fn projection(&self) -> &parquet::arrow::ProjectionMask {
        &self.projection
    }

    fn evaluate(&mut self, batch: RecordBatch) -> Result<BooleanArray, arrow::error::ArrowError> {
        let num_rows = batch.num_rows();
        let rows = record_batch_to_rows(&batch)
            .map_err(|e| arrow::error::ArrowError::ComputeError(e.to_string()))?;
        let ext_schema = ExternalSchema::from_arrow(batch.schema().as_ref())
            .map_err(|e| arrow::error::ArrowError::ComputeError(e.to_string()))?;

        let mut mask = BooleanArray::builder(num_rows);
        for row in &rows {
            mask.append_value(self.predicate.evaluate(row, &ext_schema));
        }
        Ok(mask.finish())
    }
}

/// 写入 Parquet 文件
pub fn write_parquet_file(
    path: &str,
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<(), ExternalFormatError> {
    let file = File::create(path)?;
    write_parquet_to_writer(file, schema, rows)?;
    Ok(())
}

/// 将 Parquet 写入字节
pub fn write_parquet_bytes(
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<Vec<u8>, ExternalFormatError> {
    let mut buf = Vec::new();
    write_parquet_to_writer(&mut buf, schema, rows)?;
    Ok(buf)
}

fn write_parquet_to_writer<W: Write + Send>(
    writer: W,
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<(), ExternalFormatError> {
    let arrow_schema = Arc::new(schema.to_arrow());
    let batch = rows_to_record_batch(schema, rows)?;

    let props = parquet::file::properties::WriterProperties::builder().build();
    let mut writer =
        parquet::arrow::arrow_writer::ArrowWriter::try_new(writer, arrow_schema, Some(props))?;

    if batch.num_rows() > 0 {
        writer.write(&batch)?;
    }
    writer.close()?;
    Ok(())
}

// =====================================================================
//  CSV Reader / Writer
// =====================================================================

/// CSV 读取器
pub struct CsvReader {
    schema: ExternalSchema,
    batches: Vec<RecordBatch>,
}

impl CsvReader {
    /// 从文件读取 CSV
    pub fn from_file(path: &str) -> Result<Self, ExternalFormatError> {
        let file = File::open(path).map_err(|_| ExternalFormatError::FileNotFound(path.into()))?;
        Self::from_reader(file)
    }

    /// 从字节读取 CSV
    pub fn from_bytes(data: &[u8]) -> Result<Self, ExternalFormatError> {
        let data = data.to_vec();
        Self::from_reader(std::io::Cursor::new(data))
    }

    /// 从任意 Read 读取 CSV
    pub fn from_reader<R: Read + Send + 'static>(
        mut reader: R,
    ) -> Result<Self, ExternalFormatError> {
        // 读取全部数据到内存（用于 schema 推断 + 数据读取）
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;

        // 推断 schema（使用 arrow::csv::reader::Format）
        let (inferred_schema, _) = arrow::csv::reader::Format::default()
            .with_header(true)
            .infer_schema(std::io::Cursor::new(buf.as_slice()), None)
            .map_err(|e| ExternalFormatError::ParseError(e.to_string()))?;

        let schema = ExternalSchema::from_arrow(&inferred_schema)?;

        // 用推断的 schema 构建 CSV reader
        let csv_reader = arrow::csv::ReaderBuilder::new(Arc::new(inferred_schema))
            .with_header(true)
            .build(std::io::Cursor::new(buf))?;

        let mut batches = Vec::new();
        for batch in csv_reader {
            batches.push(batch?);
        }

        Ok(Self { schema, batches })
    }
}

impl ExternalReader for CsvReader {
    fn schema(&self) -> &ExternalSchema {
        &self.schema
    }

    fn read(&self, options: &ReadOptions) -> Result<Vec<ExternalRow>, ExternalFormatError> {
        let mut all_rows = Vec::new();
        for batch in &self.batches {
            let processed = process_batch(batch, &self.schema, options)?;
            let rows = record_batch_to_rows(&processed)?;
            all_rows.extend(rows);
        }
        Ok(all_rows)
    }
}

/// 写入 CSV 文件
pub fn write_csv_file(
    path: &str,
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<(), ExternalFormatError> {
    let file = File::create(path)?;
    write_csv_to_writer(file, schema, rows)?;
    Ok(())
}

/// 将 CSV 写入字节
pub fn write_csv_bytes(
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<Vec<u8>, ExternalFormatError> {
    let mut buf = Vec::new();
    write_csv_to_writer(&mut buf, schema, rows)?;
    Ok(buf)
}

fn write_csv_to_writer<W: Write>(
    writer: W,
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<(), ExternalFormatError> {
    let batch = rows_to_record_batch(schema, rows)?;

    let mut csv_writer = arrow::csv::WriterBuilder::new()
        .with_header(true)
        .build(writer);

    csv_writer.write(&batch)?;
    // into_inner() 会 flush csv::Writer 内部缓冲区并返回底层 writer
    let mut inner = csv_writer.into_inner();
    inner.flush()?;
    Ok(())
}

// =====================================================================
//  JSONLines Reader / Writer
// =====================================================================

/// JSON Lines 读取器
pub struct JsonLinesReader {
    schema: ExternalSchema,
    rows: Vec<ExternalRow>,
}

impl JsonLinesReader {
    /// 从文件读取 JSON Lines
    pub fn from_file(path: &str) -> Result<Self, ExternalFormatError> {
        let file = File::open(path).map_err(|_| ExternalFormatError::FileNotFound(path.into()))?;
        Self::from_reader(file)
    }

    /// 从字节读取 JSON Lines
    pub fn from_bytes(data: &[u8]) -> Result<Self, ExternalFormatError> {
        Self::from_reader(std::io::Cursor::new(data))
    }

    /// 从任意 Read 读取 JSON Lines
    pub fn from_reader<R: Read>(reader: R) -> Result<Self, ExternalFormatError> {
        let buf_reader = BufReader::new(reader);
        let mut rows: Vec<ExternalRow> = Vec::new();
        let mut columns: Vec<ExternalColumn> = Vec::new();
        let mut columns_init = false;

        for line in buf_reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let obj: serde_json::Value = serde_json::from_str(&line)
                .map_err(|e| ExternalFormatError::ParseError(e.to_string()))?;

            if !columns_init {
                if let serde_json::Value::Object(ref map) = obj {
                    for (key, val) in map {
                        let col_type = infer_json_type(val);
                        columns.push(ExternalColumn::new(key, col_type));
                    }
                    columns_init = true;
                }
            }

            let row = json_value_to_row(&obj, &columns);
            rows.push(row);
        }

        let schema = ExternalSchema { columns };
        Ok(Self { schema, rows })
    }
}

impl ExternalReader for JsonLinesReader {
    fn schema(&self) -> &ExternalSchema {
        &self.schema
    }

    fn read(&self, options: &ReadOptions) -> Result<Vec<ExternalRow>, ExternalFormatError> {
        let mut result: Vec<ExternalRow> = Vec::new();

        // 确定输出列（列裁剪）
        let output_columns: Vec<usize> = if let Some(ref cols) = options.columns {
            cols.iter()
                .map(|name| {
                    self.schema
                        .find_column(name)
                        .ok_or_else(|| ExternalFormatError::ColumnNotFound(name.clone()))
                })
                .collect::<Result<_, _>>()?
        } else {
            (0..self.schema.column_count()).collect()
        };

        for row in &self.rows {
            // 谓词过滤
            if let Some(ref pred) = options.predicate {
                if !pred.evaluate(row, &self.schema) {
                    continue;
                }
            }

            // 列裁剪
            let pruned_values: Vec<ExternalValue> = output_columns
                .iter()
                .map(|&idx| row.values.get(idx).cloned().unwrap_or(ExternalValue::Null))
                .collect();
            result.push(ExternalRow {
                values: pruned_values,
            });
        }

        Ok(result)
    }
}

/// 写入 JSON Lines 文件
pub fn write_json_lines_file(
    path: &str,
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<(), ExternalFormatError> {
    let file = File::create(path)?;
    write_json_lines_to_writer(file, schema, rows)?;
    Ok(())
}

/// 将 JSON Lines 写入字节
pub fn write_json_lines_bytes(
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<Vec<u8>, ExternalFormatError> {
    let mut buf = Vec::new();
    write_json_lines_to_writer(&mut buf, schema, rows)?;
    Ok(buf)
}

fn write_json_lines_to_writer<W: Write>(
    mut writer: W,
    schema: &ExternalSchema,
    rows: &[ExternalRow],
) -> Result<(), ExternalFormatError> {
    for row in rows {
        let mut obj = serde_json::Map::new();
        for (idx, col) in schema.columns.iter().enumerate() {
            let value = row.get(idx).unwrap_or(&ExternalValue::Null);
            let json_val = external_value_to_json(value);
            obj.insert(col.name.clone(), json_val);
        }
        let line = serde_json::to_string(&serde_json::Value::Object(obj))
            .map_err(|e| ExternalFormatError::ParseError(e.to_string()))?;
        writeln!(writer, "{line}")?;
    }
    writer.flush()?;
    Ok(())
}

// =====================================================================
//  JSON 辅助函数
// =====================================================================

/// 从 JSON 值推断类型
fn infer_json_type(val: &serde_json::Value) -> ExternalType {
    match val {
        serde_json::Value::Bool(_) => ExternalType::Bool,
        serde_json::Value::Number(n) => {
            if n.is_i64() {
                ExternalType::Int64
            } else {
                ExternalType::Float64
            }
        }
        serde_json::Value::String(_) => ExternalType::Text,
        serde_json::Value::Null => ExternalType::Text, // NULL 默认 Text
        _ => ExternalType::Text,
    }
}

/// 将 JSON 值转换为 ExternalRow
fn json_value_to_row(obj: &serde_json::Value, columns: &[ExternalColumn]) -> ExternalRow {
    let mut values = Vec::with_capacity(columns.len());
    if let serde_json::Value::Object(map) = obj {
        for col in columns {
            let val = map.get(&col.name).unwrap_or(&serde_json::Value::Null);
            let ext_val = json_to_external_value(val, col.col_type);
            values.push(ext_val);
        }
    } else {
        for _ in columns.iter() {
            values.push(ExternalValue::Null);
        }
    }
    ExternalRow { values }
}

/// 将 JSON 值转换为 ExternalValue
fn json_to_external_value(val: &serde_json::Value, col_type: ExternalType) -> ExternalValue {
    match val {
        serde_json::Value::Null => ExternalValue::Null,
        serde_json::Value::Bool(b) => ExternalValue::Bool(*b),
        serde_json::Value::Number(n) => match col_type {
            ExternalType::Int64 => ExternalValue::Int64(n.as_i64().unwrap_or(0)),
            ExternalType::Float64 => ExternalValue::Float64(n.as_f64().unwrap_or(0.0)),
            _ => ExternalValue::Text(n.to_string()),
        },
        serde_json::Value::String(s) => ExternalValue::Text(s.clone()),
        _ => ExternalValue::Null,
    }
}

/// 将 ExternalValue 转换为 JSON 值
fn external_value_to_json(val: &ExternalValue) -> serde_json::Value {
    match val {
        ExternalValue::Int64(v) => serde_json::Value::Number((*v).into()),
        ExternalValue::Float64(v) => {
            if let Some(n) = serde_json::Number::from_f64(*v) {
                serde_json::Value::Number(n)
            } else {
                serde_json::Value::Null
            }
        }
        ExternalValue::Text(s) => serde_json::Value::String(s.clone()),
        ExternalValue::Bool(b) => serde_json::Value::Bool(*b),
        ExternalValue::Null => serde_json::Value::Null,
    }
}

// =====================================================================
//  通用批处理辅助函数
// =====================================================================

/// 对 RecordBatch 应用列裁剪和谓词过滤
fn process_batch(
    batch: &RecordBatch,
    schema: &ExternalSchema,
    options: &ReadOptions,
) -> Result<RecordBatch, ExternalFormatError> {
    let mut result = batch.clone();

    // 1. 谓词过滤（先过滤行，减少后续处理的数据量）
    if let Some(ref pred) = options.predicate {
        result = filter_rows_by_predicate(&result, schema, pred)?;
    }

    // 2. 列裁剪（后选列，只输出用户请求的列）
    if let Some(ref cols) = options.columns {
        result = prune_columns(&result, schema, cols)?;
    }

    Ok(result)
}

// =====================================================================
//  便捷函数
// =====================================================================

/// 从文件读取外部格式数据
///
/// 根据文件扩展名自动检测格式：
/// - `.arrow` / `.ipc` → Arrow IPC
/// - `.parquet` / `.pq` → Parquet
/// - `.csv` / `.tsv` → CSV
/// - `.jsonl` / `.ndjson` / `.json` → JSON Lines
pub fn read_external_file(
    path: &str,
    options: &ReadOptions,
) -> Result<(ExternalSchema, Vec<ExternalRow>), ExternalFormatError> {
    let format = ExternalFormat::from_path(path)?;
    match format {
        ExternalFormat::Arrow => {
            let reader = ArrowReader::from_file(path)?;
            let rows = reader.read(options)?;
            let schema = if options.needs_column_pruning() {
                prune_schema(&reader.schema, options.columns.as_ref().unwrap())
            } else {
                reader.schema.clone()
            };
            Ok((schema, rows))
        }
        ExternalFormat::Parquet => {
            // Parquet 支持谓词下推：如果有谓词条件，使用 RowFilter 读取
            let reader = if let Some(ref pred) = options.predicate {
                if !options.needs_column_pruning() {
                    // 仅谓词下推，无列裁剪
                    ParquetReader::from_file_with_predicate(path, pred)?
                } else {
                    // 同时有列裁剪和谓词：先谓词下推读取，再列裁剪
                    ParquetReader::from_file_with_predicate(path, pred)?
                }
            } else {
                ParquetReader::from_file(path)?
            };
            let rows = reader.read(options)?;
            let schema = if options.needs_column_pruning() {
                prune_schema(&reader.schema, options.columns.as_ref().unwrap())
            } else {
                reader.schema.clone()
            };
            Ok((schema, rows))
        }
        ExternalFormat::Csv => {
            let reader = CsvReader::from_file(path)?;
            let rows = reader.read(options)?;
            let schema = if options.needs_column_pruning() {
                prune_schema(&reader.schema, options.columns.as_ref().unwrap())
            } else {
                reader.schema.clone()
            };
            Ok((schema, rows))
        }
        ExternalFormat::JsonLines => {
            let reader = JsonLinesReader::from_file(path)?;
            let rows = reader.read(options)?;
            let schema = if options.needs_column_pruning() {
                prune_schema(&reader.schema, options.columns.as_ref().unwrap())
            } else {
                reader.schema.clone()
            };
            Ok((schema, rows))
        }
    }
}

/// 列裁剪 schema
fn prune_schema(schema: &ExternalSchema, columns: &[String]) -> ExternalSchema {
    let pruned: Vec<ExternalColumn> = columns
        .iter()
        .filter_map(|name| schema.columns.iter().find(|c| c.name == *name).cloned())
        .collect();
    ExternalSchema { columns: pruned }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  测试数据构造
    // -----------------------------------------------------------------

    /// 构造测试 schema: id(Int64), name(Text), price(Float64), active(Bool)
    fn test_schema() -> ExternalSchema {
        ExternalSchema::from_columns(vec![
            ExternalColumn::new("id", ExternalType::Int64),
            ExternalColumn::new("name", ExternalType::Text),
            ExternalColumn::new("price", ExternalType::Float64),
            ExternalColumn::new("active", ExternalType::Bool),
        ])
    }

    /// 构造 5 行测试数据
    fn test_rows() -> Vec<ExternalRow> {
        vec![
            ExternalRow::from_values(vec![
                ExternalValue::Int64(1),
                ExternalValue::Text("alice".to_string()),
                ExternalValue::Float64(9.99),
                ExternalValue::Bool(true),
            ]),
            ExternalRow::from_values(vec![
                ExternalValue::Int64(2),
                ExternalValue::Text("bob".to_string()),
                ExternalValue::Float64(19.99),
                ExternalValue::Bool(false),
            ]),
            ExternalRow::from_values(vec![
                ExternalValue::Int64(3),
                ExternalValue::Text("carol".to_string()),
                ExternalValue::Float64(5.50),
                ExternalValue::Bool(true),
            ]),
            ExternalRow::from_values(vec![
                ExternalValue::Int64(4),
                ExternalValue::Text("dave".to_string()),
                ExternalValue::Float64(50.0),
                ExternalValue::Bool(true),
            ]),
            ExternalRow::from_values(vec![
                ExternalValue::Int64(5),
                ExternalValue::Text("eve".to_string()),
                ExternalValue::Float64(15.0),
                ExternalValue::Bool(false),
            ]),
        ]
    }

    // -----------------------------------------------------------------
    //  1. 类型系统测试
    // -----------------------------------------------------------------

    #[test]
    fn test_external_type_to_arrow() {
        assert_eq!(ExternalType::Int64.to_arrow(), DataType::Int64);
        assert_eq!(ExternalType::Float64.to_arrow(), DataType::Float64);
        assert_eq!(ExternalType::Text.to_arrow(), DataType::Utf8);
        assert_eq!(ExternalType::Bool.to_arrow(), DataType::Boolean);
    }

    #[test]
    fn test_external_type_from_arrow() {
        assert_eq!(
            ExternalType::from_arrow(&DataType::Int64).unwrap(),
            ExternalType::Int64
        );
        assert_eq!(
            ExternalType::from_arrow(&DataType::Float64).unwrap(),
            ExternalType::Float64
        );
        assert_eq!(
            ExternalType::from_arrow(&DataType::Utf8).unwrap(),
            ExternalType::Text
        );
        assert_eq!(
            ExternalType::from_arrow(&DataType::Boolean).unwrap(),
            ExternalType::Bool
        );
    }

    #[test]
    fn test_external_type_from_arrow_unsupported() {
        let result = ExternalType::from_arrow(&DataType::Binary);
        assert!(result.is_err());
    }

    #[test]
    fn test_external_schema_to_arrow() {
        let schema = test_schema();
        let arrow_schema = schema.to_arrow();
        assert_eq!(arrow_schema.fields().len(), 4);
        assert_eq!(arrow_schema.field(0).name(), "id");
        assert_eq!(arrow_schema.field(0).data_type(), &DataType::Int64);
        assert_eq!(arrow_schema.field(1).name(), "name");
        assert_eq!(arrow_schema.field(1).data_type(), &DataType::Utf8);
    }

    #[test]
    fn test_external_schema_find_column() {
        let schema = test_schema();
        assert_eq!(schema.find_column("id"), Some(0));
        assert_eq!(schema.find_column("name"), Some(1));
        assert_eq!(schema.find_column("nonexistent"), None);
    }

    // -----------------------------------------------------------------
    //  2. ExternalValue 比较运算测试
    // -----------------------------------------------------------------

    #[test]
    fn test_value_greater_than() {
        assert!(ExternalValue::Int64(10).greater_than(&ExternalValue::Int64(5)));
        assert!(!ExternalValue::Int64(5).greater_than(&ExternalValue::Int64(10)));
        assert!(ExternalValue::Float64(3.15).greater_than(&ExternalValue::Float64(1.0)));
        assert!(ExternalValue::Text("b".to_string())
            .greater_than(&ExternalValue::Text("a".to_string())));
    }

    #[test]
    fn test_value_less_than() {
        assert!(ExternalValue::Int64(5).less_than(&ExternalValue::Int64(10)));
        assert!(!ExternalValue::Int64(10).less_than(&ExternalValue::Int64(5)));
    }

    #[test]
    fn test_value_equals() {
        assert!(ExternalValue::Int64(42).equals(&ExternalValue::Int64(42)));
        assert!(!ExternalValue::Int64(42).equals(&ExternalValue::Int64(43)));
        assert!(ExternalValue::Text("hello".to_string())
            .equals(&ExternalValue::Text("hello".to_string())));
    }

    // -----------------------------------------------------------------
    //  3. Predicate 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_predicate_gt() {
        let schema = test_schema();
        let rows = test_rows();
        let pred = Predicate::gt("price", ExternalValue::Float64(10.0));

        let filtered: Vec<&ExternalRow> =
            rows.iter().filter(|r| pred.evaluate(r, &schema)).collect();
        assert_eq!(filtered.len(), 3); // 19.99, 50.0, 15.0
    }

    #[test]
    fn test_predicate_lt() {
        let schema = test_schema();
        let rows = test_rows();
        let pred = Predicate::lt("price", ExternalValue::Float64(10.0));

        let filtered: Vec<&ExternalRow> =
            rows.iter().filter(|r| pred.evaluate(r, &schema)).collect();
        assert_eq!(filtered.len(), 2); // 9.99, 5.50
    }

    #[test]
    fn test_predicate_eq() {
        let schema = test_schema();
        let rows = test_rows();
        let pred = Predicate::eq("name", ExternalValue::Text("alice".to_string()));

        let filtered: Vec<&ExternalRow> =
            rows.iter().filter(|r| pred.evaluate(r, &schema)).collect();
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_predicate_and() {
        let schema = test_schema();
        let rows = test_rows();
        let pred = Predicate::gt("price", ExternalValue::Float64(10.0))
            .and(Predicate::eq("active", ExternalValue::Bool(true)));

        let filtered: Vec<&ExternalRow> =
            rows.iter().filter(|r| pred.evaluate(r, &schema)).collect();
        // price > 10: bob(19.99), dave(50.0), eve(15.0)
        // active == true: alice, carol, dave
        // AND: dave only
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_predicate_or() {
        let schema = test_schema();
        let rows = test_rows();
        let pred = Predicate::eq("id", ExternalValue::Int64(1))
            .or(Predicate::eq("id", ExternalValue::Int64(5)));

        let filtered: Vec<&ExternalRow> =
            rows.iter().filter(|r| pred.evaluate(r, &schema)).collect();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_predicate_referenced_columns() {
        let pred = Predicate::gt("price", ExternalValue::Float64(10.0))
            .and(Predicate::eq("active", ExternalValue::Bool(true)));
        let cols = pred.referenced_columns();
        assert!(cols.contains(&"price".to_string()));
        assert!(cols.contains(&"active".to_string()));
    }

    // -----------------------------------------------------------------
    //  4. ReadOptions 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_read_options_column_pruning() {
        let opts = ReadOptions::all().with_columns(vec!["id".into(), "name".into()]);
        assert!(opts.needs_column_pruning());
        assert!(!opts.needs_predicate_filter());
    }

    #[test]
    fn test_read_options_predicate() {
        let opts = ReadOptions::all().with_predicate(Predicate::gt("x", ExternalValue::Int64(10)));
        assert!(!opts.needs_column_pruning());
        assert!(opts.needs_predicate_filter());
    }

    #[test]
    fn test_read_options_effective_columns() {
        let schema = test_schema();
        let opts = ReadOptions::all()
            .with_columns(vec!["id".into()])
            .with_predicate(Predicate::gt("price", ExternalValue::Float64(10.0)));

        let cols = opts.effective_columns(&schema);
        assert!(cols.contains(&"id".to_string()));
        assert!(cols.contains(&"price".to_string())); // 谓词引用的列
    }

    // -----------------------------------------------------------------
    //  5. 格式检测测试
    // -----------------------------------------------------------------

    #[test]
    fn test_detect_format_by_extension() {
        assert_eq!(
            ExternalFormat::from_path("data.arrow").unwrap(),
            ExternalFormat::Arrow
        );
        assert_eq!(
            ExternalFormat::from_path("data.ipc").unwrap(),
            ExternalFormat::Arrow
        );
        assert_eq!(
            ExternalFormat::from_path("data.parquet").unwrap(),
            ExternalFormat::Parquet
        );
        assert_eq!(
            ExternalFormat::from_path("data.pq").unwrap(),
            ExternalFormat::Parquet
        );
        assert_eq!(
            ExternalFormat::from_path("data.csv").unwrap(),
            ExternalFormat::Csv
        );
        assert_eq!(
            ExternalFormat::from_path("data.jsonl").unwrap(),
            ExternalFormat::JsonLines
        );
        assert_eq!(
            ExternalFormat::from_path("data.ndjson").unwrap(),
            ExternalFormat::JsonLines
        );
    }

    #[test]
    fn test_detect_format_unsupported() {
        assert!(ExternalFormat::from_path("data.xyz").is_err());
        assert!(ExternalFormat::from_path("noext").is_err());
    }

    // -----------------------------------------------------------------
    //  6. Arrow IPC 读写测试
    // -----------------------------------------------------------------

    #[test]
    fn test_arrow_roundtrip() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_arrow_bytes(&schema, &rows).unwrap();
        let reader = ArrowReader::from_bytes(&bytes).unwrap();

        assert_eq!(reader.schema(), &schema);

        let read_rows = reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(read_rows.len(), 5);
        assert_eq!(read_rows[0].values[0], ExternalValue::Int64(1));
        assert_eq!(
            read_rows[0].values[1],
            ExternalValue::Text("alice".to_string())
        );
        assert_eq!(read_rows[4].values[0], ExternalValue::Int64(5));
    }

    #[test]
    fn test_arrow_column_pruning() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_arrow_bytes(&schema, &rows).unwrap();
        let reader = ArrowReader::from_bytes(&bytes).unwrap();

        let opts = ReadOptions::all().with_columns(vec!["id".into(), "price".into()]);
        let read_rows = reader.read(&opts).unwrap();

        assert_eq!(read_rows.len(), 5);
        assert_eq!(read_rows[0].values.len(), 2); // 只读取 2 列
        assert_eq!(read_rows[0].values[0], ExternalValue::Int64(1));
        assert_eq!(read_rows[0].values[1], ExternalValue::Float64(9.99));
    }

    #[test]
    fn test_arrow_predicate_filter() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_arrow_bytes(&schema, &rows).unwrap();
        let reader = ArrowReader::from_bytes(&bytes).unwrap();

        let opts =
            ReadOptions::all().with_predicate(Predicate::gt("price", ExternalValue::Float64(10.0)));
        let read_rows = reader.read(&opts).unwrap();

        assert_eq!(read_rows.len(), 3); // 19.99, 50.0, 15.0
    }

    #[test]
    fn test_arrow_predicate_and_column_pruning() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_arrow_bytes(&schema, &rows).unwrap();
        let reader = ArrowReader::from_bytes(&bytes).unwrap();

        let opts = ReadOptions::all()
            .with_columns(vec!["name".into()])
            .with_predicate(Predicate::gt("price", ExternalValue::Float64(10.0)));
        let read_rows = reader.read(&opts).unwrap();

        assert_eq!(read_rows.len(), 3);
        assert_eq!(read_rows[0].values.len(), 1); // 只读取 name 列
        assert_eq!(
            read_rows[0].values[0],
            ExternalValue::Text("bob".to_string())
        );
    }

    // -----------------------------------------------------------------
    //  7. Parquet 读写测试（含谓词下推）
    // -----------------------------------------------------------------

    #[test]
    fn test_parquet_roundtrip() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_parquet_bytes(&schema, &rows).unwrap();
        let reader = ParquetReader::from_bytes(&bytes).unwrap();

        assert_eq!(reader.schema(), &schema);

        let read_rows = reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(read_rows.len(), 5);
        assert_eq!(read_rows[0].values[0], ExternalValue::Int64(1));
        assert_eq!(
            read_rows[0].values[1],
            ExternalValue::Text("alice".to_string())
        );
    }

    #[test]
    fn test_parquet_column_pruning() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_parquet_bytes(&schema, &rows).unwrap();
        let reader = ParquetReader::from_bytes(&bytes).unwrap();

        let opts = ReadOptions::all().with_columns(vec!["id".into(), "active".into()]);
        let read_rows = reader.read(&opts).unwrap();

        assert_eq!(read_rows.len(), 5);
        assert_eq!(read_rows[0].values.len(), 2);
        assert_eq!(read_rows[0].values[0], ExternalValue::Int64(1));
        assert_eq!(read_rows[0].values[1], ExternalValue::Bool(true));
    }

    #[test]
    fn test_parquet_predicate_pushdown() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_parquet_bytes(&schema, &rows).unwrap();

        // 使用谓词下推读取（Parquet RowFilter）
        let reader = ParquetReader::from_reader_with_predicate(
            std::io::Cursor::new(bytes),
            &Predicate::gt("price", ExternalValue::Float64(10.0)),
        )
        .unwrap();

        let read_rows = reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(read_rows.len(), 3); // 19.99, 50.0, 15.0
    }

    #[test]
    fn test_parquet_predicate_and_column_pruning() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_parquet_bytes(&schema, &rows).unwrap();

        // 先谓词下推读取
        let reader = ParquetReader::from_reader_with_predicate(
            std::io::Cursor::new(bytes),
            &Predicate::gt("id", ExternalValue::Int64(2)),
        )
        .unwrap();

        // 再列裁剪
        let opts = ReadOptions::all().with_columns(vec!["name".into()]);
        let read_rows = reader.read(&opts).unwrap();

        assert_eq!(read_rows.len(), 3); // id > 2: carol, dave, eve
        assert_eq!(read_rows[0].values.len(), 1);
        assert_eq!(
            read_rows[0].values[0],
            ExternalValue::Text("carol".to_string())
        );
    }

    // -----------------------------------------------------------------
    //  8. CSV 读写测试
    // -----------------------------------------------------------------

    #[test]
    fn test_csv_roundtrip() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_csv_bytes(&schema, &rows).unwrap();
        let reader = CsvReader::from_bytes(&bytes).unwrap();

        let read_rows = reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(read_rows.len(), 5);
        // CSV 可能将数字读为 Int64 或 Float64，检查 id 列
        assert!(matches!(
            read_rows[0].values[0],
            ExternalValue::Int64(1) | ExternalValue::Float64(1.0)
        ));
    }

    #[test]
    fn test_csv_column_pruning() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_csv_bytes(&schema, &rows).unwrap();
        let reader = CsvReader::from_bytes(&bytes).unwrap();

        let opts = ReadOptions::all().with_columns(vec!["name".into()]);
        let read_rows = reader.read(&opts).unwrap();

        assert_eq!(read_rows.len(), 5);
        assert_eq!(read_rows[0].values.len(), 1);
    }

    #[test]
    fn test_csv_predicate_filter() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_csv_bytes(&schema, &rows).unwrap();
        let reader = CsvReader::from_bytes(&bytes).unwrap();

        let opts = ReadOptions::all().with_predicate(Predicate::eq(
            "name",
            ExternalValue::Text("alice".to_string()),
        ));
        let read_rows = reader.read(&opts).unwrap();

        assert_eq!(read_rows.len(), 1);
    }

    // -----------------------------------------------------------------
    //  9. JSONLines 读写测试
    // -----------------------------------------------------------------

    #[test]
    fn test_json_lines_roundtrip() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_json_lines_bytes(&schema, &rows).unwrap();
        let reader = JsonLinesReader::from_bytes(&bytes).unwrap();

        let read_rows = reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(read_rows.len(), 5);
        assert_eq!(read_rows[0].values[0], ExternalValue::Int64(1));
        assert_eq!(
            read_rows[0].values[1],
            ExternalValue::Text("alice".to_string())
        );
        assert_eq!(read_rows[0].values[2], ExternalValue::Float64(9.99));
        assert_eq!(read_rows[0].values[3], ExternalValue::Bool(true));
    }

    #[test]
    fn test_json_lines_column_pruning() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_json_lines_bytes(&schema, &rows).unwrap();
        let reader = JsonLinesReader::from_bytes(&bytes).unwrap();

        let opts = ReadOptions::all().with_columns(vec!["id".into(), "name".into()]);
        let read_rows = reader.read(&opts).unwrap();

        assert_eq!(read_rows.len(), 5);
        assert_eq!(read_rows[0].values.len(), 2);
        assert_eq!(read_rows[0].values[0], ExternalValue::Int64(1));
        assert_eq!(
            read_rows[0].values[1],
            ExternalValue::Text("alice".to_string())
        );
    }

    #[test]
    fn test_json_lines_predicate_filter() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_json_lines_bytes(&schema, &rows).unwrap();
        let reader = JsonLinesReader::from_bytes(&bytes).unwrap();

        let opts = ReadOptions::all().with_predicate(Predicate::gt("id", ExternalValue::Int64(3)));
        let read_rows = reader.read(&opts).unwrap();

        assert_eq!(read_rows.len(), 2); // id 4, 5
    }

    #[test]
    fn test_json_lines_predicate_and_column_pruning() {
        let schema = test_schema();
        let rows = test_rows();

        let bytes = write_json_lines_bytes(&schema, &rows).unwrap();
        let reader = JsonLinesReader::from_bytes(&bytes).unwrap();

        let opts = ReadOptions::all()
            .with_columns(vec!["name".into()])
            .with_predicate(Predicate::eq("active", ExternalValue::Bool(true)));
        let read_rows = reader.read(&opts).unwrap();

        assert_eq!(read_rows.len(), 3); // alice, carol, dave
        assert_eq!(read_rows[0].values.len(), 1);
        assert_eq!(
            read_rows[0].values[0],
            ExternalValue::Text("alice".to_string())
        );
    }

    // -----------------------------------------------------------------
    //  10. 文件 I/O 测试（临时文件）
    // -----------------------------------------------------------------

    #[test]
    fn test_arrow_file_io() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("szrsql_test_7f1.arrow");
        let path_str = path.to_str().unwrap();

        let schema = test_schema();
        let rows = test_rows();

        write_arrow_file(path_str, &schema, &rows).unwrap();

        let opts = ReadOptions::all();
        let (read_schema, read_rows) = read_external_file(path_str, &opts).unwrap();

        assert_eq!(read_schema.column_count(), 4);
        assert_eq!(read_rows.len(), 5);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_parquet_file_io() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("szrsql_test_7f1.parquet");
        let path_str = path.to_str().unwrap();

        let schema = test_schema();
        let rows = test_rows();

        write_parquet_file(path_str, &schema, &rows).unwrap();

        let opts = ReadOptions::all();
        let (read_schema, read_rows) = read_external_file(path_str, &opts).unwrap();

        assert_eq!(read_schema.column_count(), 4);
        assert_eq!(read_rows.len(), 5);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_csv_file_io() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("szrsql_test_7f1.csv");
        let path_str = path.to_str().unwrap();

        let schema = test_schema();
        let rows = test_rows();

        write_csv_file(path_str, &schema, &rows).unwrap();

        let opts = ReadOptions::all();
        let (_, read_rows) = read_external_file(path_str, &opts).unwrap();

        assert_eq!(read_rows.len(), 5);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_json_lines_file_io() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("szrsql_test_7f1.jsonl");
        let path_str = path.to_str().unwrap();

        let schema = test_schema();
        let rows = test_rows();

        write_json_lines_file(path_str, &schema, &rows).unwrap();

        let opts = ReadOptions::all();
        let (_, read_rows) = read_external_file(path_str, &opts).unwrap();

        assert_eq!(read_rows.len(), 5);

        let _ = std::fs::remove_file(path);
    }

    // -----------------------------------------------------------------
    //  11. read_external_file 集成测试（WHERE x > 10 谓词下推）
    // -----------------------------------------------------------------

    #[test]
    fn test_read_external_file_parquet_predicate_pushdown() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("szrsql_test_7f1_pred.parquet");
        let path_str = path.to_str().unwrap();

        let schema = test_schema();
        let rows = test_rows();

        write_parquet_file(path_str, &schema, &rows).unwrap();

        // SELECT * FROM parquet('data.parquet') WHERE price > 10
        let opts =
            ReadOptions::all().with_predicate(Predicate::gt("price", ExternalValue::Float64(10.0)));
        let (_, read_rows) = read_external_file(path_str, &opts).unwrap();

        assert_eq!(read_rows.len(), 3); // 19.99, 50.0, 15.0
        for row in &read_rows {
            if let ExternalValue::Float64(price) = row.values[2] {
                assert!(price > 10.0);
            }
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_read_external_file_arrow_where() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("szrsql_test_7f1_where.arrow");
        let path_str = path.to_str().unwrap();

        let schema = test_schema();
        let rows = test_rows();

        write_arrow_file(path_str, &schema, &rows).unwrap();

        // SELECT * FROM arrow('data.arrow') WHERE id > 2
        let opts = ReadOptions::all().with_predicate(Predicate::gt("id", ExternalValue::Int64(2)));
        let (_, read_rows) = read_external_file(path_str, &opts).unwrap();

        assert_eq!(read_rows.len(), 3); // id 3, 4, 5

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_read_external_file_column_pruning() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("szrsql_test_7f1_prune.parquet");
        let path_str = path.to_str().unwrap();

        let schema = test_schema();
        let rows = test_rows();

        write_parquet_file(path_str, &schema, &rows).unwrap();

        // 列裁剪只读取查询列
        let opts = ReadOptions::all().with_columns(vec!["id".into(), "name".into()]);
        let (read_schema, read_rows) = read_external_file(path_str, &opts).unwrap();

        assert_eq!(read_schema.column_count(), 2); // 只返回 2 列
        assert_eq!(read_rows.len(), 5);
        assert_eq!(read_rows[0].values.len(), 2);

        let _ = std::fs::remove_file(path);
    }

    // -----------------------------------------------------------------
    //  12. NULL 值处理测试
    // -----------------------------------------------------------------

    #[test]
    fn test_null_value_arrow_roundtrip() {
        let schema = ExternalSchema::from_columns(vec![
            ExternalColumn::new("id", ExternalType::Int64),
            ExternalColumn::new("name", ExternalType::Text),
        ]);

        let rows = vec![
            ExternalRow::from_values(vec![
                ExternalValue::Int64(1),
                ExternalValue::Text("alice".to_string()),
            ]),
            ExternalRow::from_values(vec![ExternalValue::Int64(2), ExternalValue::Null]),
            ExternalRow::from_values(vec![
                ExternalValue::Int64(3),
                ExternalValue::Text("carol".to_string()),
            ]),
        ];

        let bytes = write_arrow_bytes(&schema, &rows).unwrap();
        let reader = ArrowReader::from_bytes(&bytes).unwrap();
        let read_rows = reader.read(&ReadOptions::all()).unwrap();

        assert_eq!(read_rows.len(), 3);
        assert!(read_rows[1].values[1].is_null());
    }

    #[test]
    fn test_null_value_parquet_roundtrip() {
        let schema = ExternalSchema::from_columns(vec![
            ExternalColumn::new("id", ExternalType::Int64),
            ExternalColumn::new("score", ExternalType::Float64),
        ]);

        let rows = vec![
            ExternalRow::from_values(vec![ExternalValue::Int64(1), ExternalValue::Float64(95.5)]),
            ExternalRow::from_values(vec![ExternalValue::Int64(2), ExternalValue::Null]),
            ExternalRow::from_values(vec![ExternalValue::Int64(3), ExternalValue::Float64(88.0)]),
        ];

        let bytes = write_parquet_bytes(&schema, &rows).unwrap();
        let reader = ParquetReader::from_bytes(&bytes).unwrap();
        let read_rows = reader.read(&ReadOptions::all()).unwrap();

        assert_eq!(read_rows.len(), 3);
        assert!(read_rows[1].values[1].is_null());
    }

    // -----------------------------------------------------------------
    //  13. 空数据测试
    // -----------------------------------------------------------------

    #[test]
    fn test_empty_arrow() {
        let schema = test_schema();
        let rows: Vec<ExternalRow> = vec![];

        let bytes = write_arrow_bytes(&schema, &rows).unwrap();
        let reader = ArrowReader::from_bytes(&bytes).unwrap();
        let read_rows = reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(read_rows.len(), 0);
    }

    #[test]
    fn test_empty_parquet() {
        let schema = test_schema();
        let rows: Vec<ExternalRow> = vec![];

        let bytes = write_parquet_bytes(&schema, &rows).unwrap();
        let reader = ParquetReader::from_bytes(&bytes).unwrap();
        let read_rows = reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(read_rows.len(), 0);
    }

    // -----------------------------------------------------------------
    //  14. SchemaRef 辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_prune_schema() {
        let schema = test_schema();
        let pruned = prune_schema(&schema, &["id".to_string(), "active".to_string()]);
        assert_eq!(pruned.column_count(), 2);
        assert_eq!(pruned.columns[0].name, "id");
        assert_eq!(pruned.columns[1].name, "active");
    }

    #[test]
    fn test_json_value_conversion() {
        let json_val = serde_json::json!({"id": 42, "name": "test", "active": true});
        let columns = vec![
            ExternalColumn::new("id", ExternalType::Int64),
            ExternalColumn::new("name", ExternalType::Text),
            ExternalColumn::new("active", ExternalType::Bool),
        ];
        let row = json_value_to_row(&json_val, &columns);
        assert_eq!(row.values[0], ExternalValue::Int64(42));
        assert_eq!(row.values[1], ExternalValue::Text("test".to_string()));
        assert_eq!(row.values[2], ExternalValue::Bool(true));
    }

    #[test]
    fn test_external_value_to_json() {
        assert_eq!(
            external_value_to_json(&ExternalValue::Int64(42)),
            serde_json::json!(42)
        );
        assert_eq!(
            external_value_to_json(&ExternalValue::Text("hello".into())),
            serde_json::json!("hello")
        );
        assert_eq!(
            external_value_to_json(&ExternalValue::Bool(true)),
            serde_json::json!(true)
        );
        assert_eq!(
            external_value_to_json(&ExternalValue::Null),
            serde_json::Value::Null
        );
    }
}
