//! OpenTelemetry 集成 — Phase 7d.11
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.11 OpenTelemetry 集成设计。
//!
//! # 设计
//!
//! 实现 OpenTelemetry tracing SDK 核心能力，支持 SQL 查询链路追踪：
//! - **标识符** — TraceId(16B)/SpanId(8B)/TraceFlags(1B)，W3C traceparent 格式传播
//! - **Span 模型** — SpanContext/StatusCode/SpanKind/SpanEvent/SpanLink/SpanData
//! - **Span 处理器** — SimpleSpanProcessor（同步）/ BatchSpanProcessor（批量缓冲+阈值触发）
//! - **Tracer/TracerProvider** — Tracer 管理 + ID 生成器 + 采样 + 处理器
//! - **OTLP 导出器** — 导出 SpanData 为 OTLP JSON 格式（resourceSpans/scopeSpans）
//! - **Context Propagation** — W3C traceparent + baggage 头格式 inject/extract
//! - **SqlSpanBuilder** — SQL 查询专用 Span 构造器（db.statement/db.system/db.rows_scanned 等）
//!
//! ## 验证标准
//!
//! - OpenTelemetry SDK 集成 → Span 创建/传播 → 导出到 OTLP
//! - 查询时间/索引使用/JOIN 方式可追踪

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// =====================================================================
//  常量
// =====================================================================

/// TraceId 长度（字节）— OTel 规范 16 字节
pub const TRACE_ID_LEN: usize = 16;

/// SpanId 长度（字节）— OTel 规范 8 字节
pub const SPAN_ID_LEN: usize = 8;

/// 默认最大 Span 数（BatchSpanProcessor 缓冲区容量）
pub const DEFAULT_MAX_SPANS: usize = 10_000;

/// 默认批量导出大小
pub const DEFAULT_EXPORT_BATCH_SIZE: usize = 100;

// =====================================================================
//  TraceId — 16 字节追踪标识符
// =====================================================================

/// 16 字节追踪标识符 — OTel 规范
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceId([u8; TRACE_ID_LEN]);

impl TraceId {
    /// 全零 TraceId（无效）
    pub fn zero() -> Self {
        Self([0u8; TRACE_ID_LEN])
    }

    /// 从字节数组构造
    pub fn from_bytes(bytes: [u8; TRACE_ID_LEN]) -> Self {
        Self(bytes)
    }

    /// 从 hex 字符串构造（32 字符）
    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != TRACE_ID_LEN * 2 {
            return None;
        }
        let mut bytes = [0u8; TRACE_ID_LEN];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let hi = hex_digit(chunk[0])?;
            let lo = hex_digit(chunk[1])?;
            bytes[i] = (hi << 4) | lo;
        }
        Some(Self(bytes))
    }

    /// 是否无效（全零）
    pub fn is_invalid(&self) -> bool {
        self.0 == [0u8; TRACE_ID_LEN]
    }

    /// 转换为 hex 字符串（32 字符）
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(TRACE_ID_LEN * 2);
        for byte in &self.0 {
            out.push_str(&format!("{:02x}", byte));
        }
        out
    }

    /// 返回字节数组引用
    pub fn as_bytes(&self) -> &[u8; TRACE_ID_LEN] {
        &self.0
    }
}

impl fmt::Debug for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TraceId({})", self.to_hex())
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

// =====================================================================
//  SpanId — 8 字节 Span 标识符
// =====================================================================

/// 8 字节 Span 标识符 — OTel 规范
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanId([u8; SPAN_ID_LEN]);

impl SpanId {
    /// 全零 SpanId（无效）
    pub fn zero() -> Self {
        Self([0u8; SPAN_ID_LEN])
    }

    /// 从字节数组构造
    pub fn from_bytes(bytes: [u8; SPAN_ID_LEN]) -> Self {
        Self(bytes)
    }

    /// 从 hex 字符串构造（16 字符）
    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != SPAN_ID_LEN * 2 {
            return None;
        }
        let mut bytes = [0u8; SPAN_ID_LEN];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let hi = hex_digit(chunk[0])?;
            let lo = hex_digit(chunk[1])?;
            bytes[i] = (hi << 4) | lo;
        }
        Some(Self(bytes))
    }

    /// 是否无效（全零）
    pub fn is_invalid(&self) -> bool {
        self.0 == [0u8; SPAN_ID_LEN]
    }

    /// 转换为 hex 字符串（16 字符）
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(SPAN_ID_LEN * 2);
        for byte in &self.0 {
            out.push_str(&format!("{:02x}", byte));
        }
        out
    }

    /// 返回字节数组引用
    pub fn as_bytes(&self) -> &[u8; SPAN_ID_LEN] {
        &self.0
    }
}

impl fmt::Debug for SpanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SpanId({})", self.to_hex())
    }
}

impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// hex 字符转数值
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// =====================================================================
//  TraceFlags — 1 字节追踪标志
// =====================================================================

/// 1 字节追踪标志 — OTel 规范（bit 0 = sampled）
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceFlags(u8);

impl TraceFlags {
    /// 默认标志（未采样）
    pub const DEFAULT: Self = Self(0);

    /// 采样标志位
    pub const SAMPLED_FLAG: u8 = 0x01;

    /// 构造（默认未采样）
    pub fn new() -> Self {
        Self::DEFAULT
    }

    /// 是否采样
    pub fn is_sampled(&self) -> bool {
        (self.0 & Self::SAMPLED_FLAG) != 0
    }

    /// 设置采样位
    pub fn with_sampled(mut self, sampled: bool) -> Self {
        if sampled {
            self.0 |= Self::SAMPLED_FLAG;
        } else {
            self.0 &= !Self::SAMPLED_FLAG;
        }
        self
    }

    /// 返回 u8 值
    pub fn as_u8(&self) -> u8 {
        self.0
    }
}

impl Default for TraceFlags {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Debug for TraceFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TraceFlags(0x{:02x}, sampled={})",
            self.0,
            self.is_sampled()
        )
    }
}

// =====================================================================
//  SpanContext — Span 上下文
// =====================================================================

/// Span 上下文 — trace_id + span_id + trace_flags + is_remote
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanContext {
    /// Trace ID
    trace_id: TraceId,
    /// Span ID
    span_id: SpanId,
    /// 追踪标志
    trace_flags: TraceFlags,
    /// 是否远程（从其他进程传播而来）
    is_remote: bool,
}

impl SpanContext {
    /// 构造 Span 上下文
    pub fn new(
        trace_id: TraceId,
        span_id: SpanId,
        trace_flags: TraceFlags,
        is_remote: bool,
    ) -> Self {
        Self {
            trace_id,
            span_id,
            trace_flags,
            is_remote,
        }
    }

    /// 空 Span 上下文（无效）
    pub fn empty() -> Self {
        Self::new(TraceId::zero(), SpanId::zero(), TraceFlags::DEFAULT, false)
    }

    /// 是否有效（trace_id 和 span_id 都非零）
    pub fn is_valid(&self) -> bool {
        !self.trace_id.is_invalid() && !self.span_id.is_invalid()
    }

    /// 是否采样
    pub fn is_sampled(&self) -> bool {
        self.trace_flags.is_sampled()
    }

    /// 是否远程
    pub fn is_remote(&self) -> bool {
        self.is_remote
    }

    /// 返回 trace_id
    pub fn trace_id(&self) -> TraceId {
        self.trace_id
    }

    /// 返回 span_id
    pub fn span_id(&self) -> SpanId {
        self.span_id
    }

    /// 返回 trace_flags
    pub fn trace_flags(&self) -> TraceFlags {
        self.trace_flags
    }

    /// 转换为 W3C traceparent 字符串（00-<trace_id>-<span_id>-<flags>）
    pub fn to_traceparent(&self) -> String {
        format!(
            "00-{}-{}-{:02x}",
            self.trace_id.to_hex(),
            self.span_id.to_hex(),
            self.trace_flags.as_u8()
        )
    }

    /// 从 W3C traceparent 字符串解析
    pub fn from_traceparent(tp: &str) -> Option<Self> {
        let parts: Vec<&str> = tp.split('-').collect();
        if parts.len() != 4 {
            return None;
        }
        let trace_id = TraceId::from_hex(parts[1])?;
        let span_id = SpanId::from_hex(parts[2])?;
        let flags_byte = u8::from_str_radix(parts[3], 16).ok()?;
        Some(Self::new(
            trace_id,
            span_id,
            TraceFlags::new().with_sampled((flags_byte & 0x01) != 0),
            true,
        ))
    }
}

impl fmt::Debug for SpanContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpanContext")
            .field("trace_id", &self.trace_id.to_hex())
            .field("span_id", &self.span_id.to_hex())
            .field("sampled", &self.trace_flags.is_sampled())
            .field("is_remote", &self.is_remote)
            .finish()
    }
}

// =====================================================================
//  StatusCode — Span 状态码
// =====================================================================

/// Span 状态码 — OTel 规范 3 态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StatusCode {
    /// 未设置（默认）
    #[default]
    Unset,
    /// 成功
    Ok,
    /// 错误
    Error,
}

impl StatusCode {
    /// 名称
    pub fn as_str(&self) -> &'static str {
        match self {
            StatusCode::Unset => "UNSET",
            StatusCode::Ok => "OK",
            StatusCode::Error => "ERROR",
        }
    }

    /// 是否错误
    pub fn is_error(&self) -> bool {
        matches!(self, StatusCode::Error)
    }

    /// 是否成功
    pub fn is_ok(&self) -> bool {
        matches!(self, StatusCode::Ok)
    }
}

impl fmt::Display for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  SpanStatus — Span 状态
// =====================================================================

/// Span 状态 — 状态码 + 可选描述
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanStatus {
    /// 状态码
    code: StatusCode,
    /// 描述（仅 Error 时有意义）
    description: Option<String>,
}

impl SpanStatus {
    /// 未设置状态
    pub fn unset() -> Self {
        Self {
            code: StatusCode::Unset,
            description: None,
        }
    }

    /// 成功状态
    pub fn ok() -> Self {
        Self {
            code: StatusCode::Ok,
            description: None,
        }
    }

    /// 错误状态
    pub fn error(description: impl Into<String>) -> Self {
        Self {
            code: StatusCode::Error,
            description: Some(description.into()),
        }
    }

    /// 是否错误
    pub fn is_error(&self) -> bool {
        self.code.is_error()
    }

    /// 返回状态码
    pub fn code(&self) -> StatusCode {
        self.code
    }

    /// 返回描述
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

impl Default for SpanStatus {
    fn default() -> Self {
        Self::unset()
    }
}

// =====================================================================
//  SpanKind — Span 类型
// =====================================================================

/// Span 类型 — OTel 规范 5 变体
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SpanKind {
    /// 内部（默认）
    #[default]
    Internal,
    /// 服务端
    Server,
    /// 客户端
    Client,
    /// 生产者
    Producer,
    /// 消费者
    Consumer,
}

impl SpanKind {
    /// 名称
    pub fn as_str(&self) -> &'static str {
        match self {
            SpanKind::Internal => "INTERNAL",
            SpanKind::Server => "SERVER",
            SpanKind::Client => "CLIENT",
            SpanKind::Producer => "PRODUCER",
            SpanKind::Consumer => "CONSUMER",
        }
    }

    /// 是否内部
    pub fn is_internal(&self) -> bool {
        matches!(self, SpanKind::Internal)
    }

    /// 是否服务端
    pub fn is_server(&self) -> bool {
        matches!(self, SpanKind::Server)
    }

    /// 是否客户端
    pub fn is_client(&self) -> bool {
        matches!(self, SpanKind::Client)
    }
}

impl fmt::Display for SpanKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  SpanAttributeValue — Span 属性值
// =====================================================================

/// Span 属性值 — 6 种类型
#[derive(Debug, Clone, PartialEq)]
pub enum SpanAttributeValue {
    /// 字符串
    String(String),
    /// 整数（i64）
    Int(i64),
    /// 浮点数（f64）
    Float(f64),
    /// 布尔
    Bool(bool),
    /// 字节数组
    Bytes(Vec<u8>),
    /// 字符串数组
    StringArray(Vec<String>),
}

impl SpanAttributeValue {
    /// 是否字符串
    pub fn is_string(&self) -> bool {
        matches!(self, SpanAttributeValue::String(_))
    }

    /// 是否整数
    pub fn is_int(&self) -> bool {
        matches!(self, SpanAttributeValue::Int(_))
    }

    /// 是否浮点数
    pub fn is_float(&self) -> bool {
        matches!(self, SpanAttributeValue::Float(_))
    }

    /// 是否布尔
    pub fn is_bool(&self) -> bool {
        matches!(self, SpanAttributeValue::Bool(_))
    }

    /// 转换为 JSON 值字符串
    pub fn to_json_value(&self) -> String {
        match self {
            SpanAttributeValue::String(s) => format!("\"{}\"", escape_json_string(s)),
            SpanAttributeValue::Int(i) => i.to_string(),
            SpanAttributeValue::Float(f) => f.to_string(),
            SpanAttributeValue::Bool(b) => b.to_string(),
            SpanAttributeValue::Bytes(b) => {
                let hex: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
                format!("\"{}\"", hex)
            }
            SpanAttributeValue::StringArray(arr) => {
                let items: Vec<String> = arr
                    .iter()
                    .map(|s| format!("\"{}\"", escape_json_string(s)))
                    .collect();
                format!("[{}]", items.join(","))
            }
        }
    }
}

impl From<&str> for SpanAttributeValue {
    fn from(s: &str) -> Self {
        SpanAttributeValue::String(s.to_string())
    }
}

impl From<String> for SpanAttributeValue {
    fn from(s: String) -> Self {
        SpanAttributeValue::String(s)
    }
}

impl From<i64> for SpanAttributeValue {
    fn from(i: i64) -> Self {
        SpanAttributeValue::Int(i)
    }
}

impl From<f64> for SpanAttributeValue {
    fn from(f: f64) -> Self {
        SpanAttributeValue::Float(f)
    }
}

impl From<bool> for SpanAttributeValue {
    fn from(b: bool) -> Self {
        SpanAttributeValue::Bool(b)
    }
}

/// 转义 JSON 字符串
fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// =====================================================================
//  SpanEvent — Span 事件
// =====================================================================

/// Span 事件 — 带时间戳的命名事件
#[derive(Debug, Clone, PartialEq)]
pub struct SpanEvent {
    /// 事件名
    pub name: String,
    /// 时间戳（纳秒）
    pub timestamp_ns: u64,
    /// 事件属性
    pub attributes: HashMap<String, SpanAttributeValue>,
}

impl SpanEvent {
    /// 构造事件
    pub fn new(name: impl Into<String>, timestamp_ns: u64) -> Self {
        Self {
            name: name.into(),
            timestamp_ns,
            attributes: HashMap::new(),
        }
    }

    /// 添加属性
    pub fn with_attribute(
        mut self,
        key: impl Into<String>,
        value: impl Into<SpanAttributeValue>,
    ) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }
}

// =====================================================================
//  SpanLink — Span 链接
// =====================================================================

/// Span 链接 — 关联其他 SpanContext（不构成父子关系）
#[derive(Debug, Clone, PartialEq)]
pub struct SpanLink {
    /// 关联的 Span 上下文
    pub span_context: SpanContext,
    /// 链接属性
    pub attributes: HashMap<String, SpanAttributeValue>,
}

impl SpanLink {
    /// 构造链接
    pub fn new(span_context: SpanContext) -> Self {
        Self {
            span_context,
            attributes: HashMap::new(),
        }
    }

    /// 添加属性
    pub fn with_attribute(
        mut self,
        key: impl Into<String>,
        value: impl Into<SpanAttributeValue>,
    ) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }
}

// =====================================================================
//  SpanData — 已结束 Span 的不可变快照
// =====================================================================

/// 已结束 Span 的不可变快照
#[derive(Debug, Clone, PartialEq)]
pub struct SpanData {
    /// Span 名称
    pub name: String,
    /// Span 上下文
    pub span_context: SpanContext,
    /// 父 Span ID（None 表示根 Span）
    pub parent_span_id: Option<SpanId>,
    /// Span 类型
    pub kind: SpanKind,
    /// 开始时间（纳秒）
    pub start_time_ns: u64,
    /// 结束时间（纳秒）
    pub end_time_ns: u64,
    /// 属性
    pub attributes: HashMap<String, SpanAttributeValue>,
    /// 事件列表
    pub events: Vec<SpanEvent>,
    /// 链接列表
    pub links: Vec<SpanLink>,
    /// 状态
    pub status: SpanStatus,
    /// 资源（服务名等）
    pub resource: HashMap<String, String>,
}

impl SpanData {
    /// 持续时间（纳秒）
    pub fn duration_ns(&self) -> u64 {
        self.end_time_ns.saturating_sub(self.start_time_ns)
    }

    /// 持续时间（微秒）
    pub fn duration_us(&self) -> u64 {
        self.duration_ns() / 1_000
    }

    /// 持续时间（毫秒）
    pub fn duration_ms(&self) -> u64 {
        self.duration_ns() / 1_000_000
    }

    /// 是否根 Span（无父 Span）
    pub fn is_root(&self) -> bool {
        self.parent_span_id.is_none()
    }

    /// 是否错误
    pub fn is_error(&self) -> bool {
        self.status.is_error()
    }

    /// 获取字符串属性
    pub fn get_string_attribute(&self, key: &str) -> Option<&str> {
        match self.attributes.get(key) {
            Some(SpanAttributeValue::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// 获取整数属性
    pub fn get_int_attribute(&self, key: &str) -> Option<i64> {
        match self.attributes.get(key) {
            Some(SpanAttributeValue::Int(i)) => Some(*i),
            _ => None,
        }
    }

    /// 获取浮点数属性
    pub fn get_float_attribute(&self, key: &str) -> Option<f64> {
        match self.attributes.get(key) {
            Some(SpanAttributeValue::Float(f)) => Some(*f),
            _ => None,
        }
    }

    /// 获取布尔属性
    pub fn get_bool_attribute(&self, key: &str) -> Option<bool> {
        match self.attributes.get(key) {
            Some(SpanAttributeValue::Bool(b)) => Some(*b),
            _ => None,
        }
    }
}

// =====================================================================
//  SpanProcessor trait — Span 处理器
// =====================================================================

/// Span 处理器 trait — on_start/on_end 钩子
pub trait SpanProcessor: fmt::Debug + Send + Sync {
    /// Span 开始时调用
    fn on_start(&self, _span: &Span) {}

    /// Span 结束时调用
    fn on_end(&self, span: SpanData);
}

/// 简单 Span 处理器 — 同步收集所有 Span
#[derive(Debug, Default)]
pub struct SimpleSpanProcessor {
    spans: Mutex<Vec<SpanData>>,
}

impl SimpleSpanProcessor {
    /// 构造
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回已收集的 Span 列表
    pub fn spans(&self) -> Vec<SpanData> {
        self.spans.lock().unwrap().clone()
    }

    /// 已收集 Span 数
    pub fn len(&self) -> usize {
        self.spans.lock().unwrap().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.spans.lock().unwrap().is_empty()
    }

    /// 清空
    pub fn clear(&self) {
        self.spans.lock().unwrap().clear();
    }
}

impl SpanProcessor for SimpleSpanProcessor {
    fn on_end(&self, span: SpanData) {
        self.spans.lock().unwrap().push(span);
    }
}

/// 批量 Span 处理器 — 缓冲 Span，达到阈值时触发导出
#[derive(Debug)]
pub struct BatchSpanProcessor {
    /// 缓冲区
    buffer: Mutex<Vec<SpanData>>,
    /// 批量大小
    batch_size: usize,
    /// 已导出的 Span 列表
    exported: Mutex<Vec<SpanData>>,
    /// 导出次数
    export_count: AtomicU64,
}

impl BatchSpanProcessor {
    /// 构造
    pub fn new(batch_size: usize) -> Self {
        Self {
            buffer: Mutex::new(Vec::with_capacity(batch_size)),
            batch_size,
            exported: Mutex::new(Vec::new()),
            export_count: AtomicU64::new(0),
        }
    }

    /// 使用默认批量大小构造
    pub fn with_default() -> Self {
        Self::new(DEFAULT_EXPORT_BATCH_SIZE)
    }

    /// 强制刷新（导出所有缓冲的 Span）
    pub fn force_flush(&self) {
        let mut buffer = self.buffer.lock().unwrap();
        if buffer.is_empty() {
            return;
        }
        let batch: Vec<SpanData> = buffer.drain(..).collect();
        self.exported.lock().unwrap().extend(batch);
        self.export_count.fetch_add(1, Ordering::SeqCst);
    }

    /// 返回已导出的 Span 列表
    pub fn exported_spans(&self) -> Vec<SpanData> {
        self.exported.lock().unwrap().clone()
    }

    /// 缓冲区中 Span 数
    pub fn buffered_len(&self) -> usize {
        self.buffer.lock().unwrap().len()
    }

    /// 已导出 Span 数
    pub fn exported_len(&self) -> usize {
        self.exported.lock().unwrap().len()
    }

    /// 总 Span 数（缓冲 + 已导出）
    pub fn total_len(&self) -> usize {
        self.buffered_len() + self.exported_len()
    }

    /// 导出次数
    pub fn export_count(&self) -> u64 {
        self.export_count.load(Ordering::SeqCst)
    }

    /// 清空所有
    pub fn clear(&self) {
        self.buffer.lock().unwrap().clear();
        self.exported.lock().unwrap().clear();
    }

    /// 尝试触发导出（如果缓冲区达到阈值）
    fn try_export(&self) {
        let should_export = self.buffered_len() >= self.batch_size;
        if should_export {
            self.force_flush();
        }
    }
}

impl SpanProcessor for BatchSpanProcessor {
    fn on_end(&self, span: SpanData) {
        self.buffer.lock().unwrap().push(span);
        self.try_export();
    }
}

// =====================================================================
//  IdGenerator trait — ID 生成器
// =====================================================================

/// ID 生成器 trait
pub trait IdGenerator: fmt::Debug + Send + Sync {
    /// 生成新的 TraceId
    fn new_trace_id(&self) -> TraceId;

    /// 生成新的 SpanId
    fn new_span_id(&self) -> SpanId;
}

/// 顺序 ID 生成器 — 自增确定性（测试用）
#[derive(Debug)]
pub struct SequentialIdGenerator {
    counter: AtomicU64,
}

impl SequentialIdGenerator {
    /// 构造（从 1 开始）
    pub fn new() -> Self {
        Self {
            counter: AtomicU64::new(1),
        }
    }

    /// 从指定起始值构造
    pub fn with_start(start: u64) -> Self {
        Self {
            counter: AtomicU64::new(start),
        }
    }
}

impl Default for SequentialIdGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl IdGenerator for SequentialIdGenerator {
    fn new_trace_id(&self) -> TraceId {
        let value = self.counter.fetch_add(1, Ordering::SeqCst);
        let mut bytes = [0u8; TRACE_ID_LEN];
        bytes[8..16].copy_from_slice(&value.to_be_bytes());
        TraceId::from_bytes(bytes)
    }

    fn new_span_id(&self) -> SpanId {
        let value = self.counter.fetch_add(1, Ordering::SeqCst);
        let mut bytes = [0u8; SPAN_ID_LEN];
        bytes.copy_from_slice(&value.to_be_bytes());
        SpanId::from_bytes(bytes)
    }
}

// =====================================================================
//  TracerProvider — Tracer 管理器
// =====================================================================

/// Tracer 配置
#[derive(Debug, Clone)]
pub struct TracerConfig {
    /// 服务名
    pub service_name: String,
    /// 是否采样（返回 true 表示采样）
    pub should_sample: bool,
}

impl TracerConfig {
    /// 构造默认配置
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            should_sample: true,
        }
    }

    /// 设置采样
    pub fn with_sampling(mut self, should_sample: bool) -> Self {
        self.should_sample = should_sample;
        self
    }
}

impl Default for TracerConfig {
    fn default() -> Self {
        Self::new("szrsql")
    }
}

/// Tracer 提供者 — 管理 Tracer 和共享的 Span 处理器
pub struct TracerProvider {
    /// 配置
    config: TracerConfig,
    /// ID 生成器
    id_generator: Box<dyn IdGenerator>,
    /// Span 处理器（Arc 共享）
    processor: Option<Arc<dyn SpanProcessor>>,
}

impl fmt::Debug for TracerProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TracerProvider")
            .field("config", &self.config)
            .field("has_processor", &self.processor.is_some())
            .finish()
    }
}

impl TracerProvider {
    /// 构造
    pub fn new(config: TracerConfig) -> Self {
        Self {
            config,
            id_generator: Box::new(SequentialIdGenerator::new()),
            processor: None,
        }
    }

    /// 设置配置
    pub fn with_config(mut self, config: TracerConfig) -> Self {
        self.config = config;
        self
    }

    /// 设置 ID 生成器
    pub fn with_id_generator(mut self, gen: impl IdGenerator + 'static) -> Self {
        self.id_generator = Box::new(gen);
        self
    }

    /// 设置简单 Span 处理器
    pub fn with_simple_processor(self, processor: Arc<SimpleSpanProcessor>) -> Self {
        Self {
            config: self.config,
            id_generator: self.id_generator,
            processor: Some(processor),
        }
    }

    /// 设置批量 Span 处理器
    pub fn with_batch_processor(self, processor: Arc<BatchSpanProcessor>) -> Self {
        Self {
            config: self.config,
            id_generator: self.id_generator,
            processor: Some(processor),
        }
    }

    /// 返回处理器引用
    pub fn processor(&self) -> Option<&Arc<dyn SpanProcessor>> {
        self.processor.as_ref()
    }

    /// 返回 ID 生成器引用
    pub fn id_generator(&self) -> &dyn IdGenerator {
        self.id_generator.as_ref()
    }

    /// 是否采样
    pub fn should_sample(&self) -> bool {
        self.config.should_sample
    }

    /// 返回服务名
    pub fn service_name(&self) -> &str {
        &self.config.service_name
    }

    /// 创建 Tracer
    pub fn tracer(&self) -> ProviderTracer<'_> {
        ProviderTracer { provider: self }
    }
}

impl Default for TracerProvider {
    fn default() -> Self {
        Self::new(TracerConfig::default())
    }
}

// =====================================================================
//  ProviderTracer — 从 TracerProvider 创建的 Tracer
// =====================================================================

/// 从 TracerProvider 创建的 Tracer
pub struct ProviderTracer<'a> {
    provider: &'a TracerProvider,
}

impl<'a> ProviderTracer<'a> {
    /// 返回服务名
    pub fn service_name(&self) -> &str {
        self.provider.service_name()
    }

    /// 创建 SpanBuilder
    pub fn span_builder(&self, name: impl Into<String>) -> ProviderSpanBuilder<'a> {
        ProviderSpanBuilder {
            name: name.into(),
            parent_context: None,
            kind: SpanKind::Internal,
            attributes: HashMap::new(),
            links: Vec::new(),
            start_time_ns: 0,
            provider: self.provider,
        }
    }

    /// 创建带父上下文的 SpanBuilder
    pub fn span_builder_with_parent(
        &self,
        name: impl Into<String>,
        parent: SpanContext,
    ) -> ProviderSpanBuilder<'a> {
        ProviderSpanBuilder {
            name: name.into(),
            parent_context: Some(parent),
            kind: SpanKind::Internal,
            attributes: HashMap::new(),
            links: Vec::new(),
            start_time_ns: 0,
            provider: self.provider,
        }
    }

    /// 直接启动 Span
    pub fn start_span(&self, name: impl Into<String>) -> Span {
        self.span_builder(name).start()
    }
}

// =====================================================================
//  ProviderSpanBuilder — Span 构造器
// =====================================================================

/// Span 构造器（从 TracerProvider）
pub struct ProviderSpanBuilder<'a> {
    name: String,
    parent_context: Option<SpanContext>,
    kind: SpanKind,
    attributes: HashMap<String, SpanAttributeValue>,
    links: Vec<SpanLink>,
    start_time_ns: u64,
    provider: &'a TracerProvider,
}

impl<'a> ProviderSpanBuilder<'a> {
    /// 设置 Span 类型
    pub fn with_kind(mut self, kind: SpanKind) -> Self {
        self.kind = kind;
        self
    }

    /// 设置开始时间（纳秒）
    pub fn with_start_time(mut self, time_ns: u64) -> Self {
        self.start_time_ns = time_ns;
        self
    }

    /// 添加属性
    pub fn with_attribute(
        mut self,
        key: impl Into<String>,
        value: impl Into<SpanAttributeValue>,
    ) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// 添加链接
    pub fn with_link(mut self, link: SpanLink) -> Self {
        self.links.push(link);
        self
    }

    /// 启动 Span
    pub fn start(self) -> Span {
        let trace_id = match &self.parent_context {
            Some(ctx) if ctx.is_valid() => ctx.trace_id(),
            _ => self.provider.id_generator().new_trace_id(),
        };
        let span_id = self.provider.id_generator().new_span_id();
        let trace_flags = TraceFlags::new().with_sampled(self.provider.should_sample());
        let span_context = SpanContext::new(trace_id, span_id, trace_flags, false);
        let parent_span_id = self
            .parent_context
            .map(|ctx| ctx.span_id())
            .filter(|id| !id.is_invalid());

        let mut resource = HashMap::new();
        resource.insert(
            "service.name".to_string(),
            self.provider.service_name().to_string(),
        );

        let start_time = if self.start_time_ns > 0 {
            self.start_time_ns
        } else {
            current_time_ns()
        };

        Span {
            name: self.name,
            span_context,
            parent_span_id,
            kind: self.kind,
            start_time_ns: start_time,
            attributes: self.attributes,
            events: Vec::new(),
            links: self.links,
            status: SpanStatus::unset(),
            resource,
            processor: self.provider.processor().cloned(),
            ended: false,
        }
    }
}

// =====================================================================
//  Span — 活动中的 Span 句柄
// =====================================================================

/// 活动中的 Span 句柄 — end 后递交处理器
pub struct Span {
    /// Span 名称
    pub name: String,
    /// Span 上下文
    pub span_context: SpanContext,
    /// 父 Span ID
    pub parent_span_id: Option<SpanId>,
    /// Span 类型
    pub kind: SpanKind,
    /// 开始时间（纳秒）
    pub start_time_ns: u64,
    /// 属性
    pub attributes: HashMap<String, SpanAttributeValue>,
    /// 事件列表
    pub events: Vec<SpanEvent>,
    /// 链接列表
    pub links: Vec<SpanLink>,
    /// 状态
    pub status: SpanStatus,
    /// 资源
    pub resource: HashMap<String, String>,
    /// Span 处理器
    processor: Option<Arc<dyn SpanProcessor>>,
    /// 是否已结束
    ended: bool,
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Span")
            .field("name", &self.name)
            .field("span_context", &self.span_context)
            .field("kind", &self.kind)
            .field("start_time_ns", &self.start_time_ns)
            .field("ended", &self.ended)
            .finish()
    }
}

impl Span {
    /// 返回 Span 上下文
    pub fn span_context(&self) -> SpanContext {
        self.span_context
    }

    /// 是否正在记录（未结束）
    pub fn is_recording(&self) -> bool {
        !self.ended
    }

    /// 设置属性
    pub fn set_attribute(&mut self, key: impl Into<String>, value: impl Into<SpanAttributeValue>) {
        if self.ended {
            return;
        }
        self.attributes.insert(key.into(), value.into());
    }

    /// 添加事件
    pub fn add_event(&mut self, name: impl Into<String>) {
        if self.ended {
            return;
        }
        self.events.push(SpanEvent::new(name, current_time_ns()));
    }

    /// 添加带属性的事件
    pub fn add_event_with_attributes(
        &mut self,
        name: impl Into<String>,
        attributes: HashMap<String, SpanAttributeValue>,
    ) {
        if self.ended {
            return;
        }
        let mut event = SpanEvent::new(name, current_time_ns());
        event.attributes = attributes;
        self.events.push(event);
    }

    /// 添加链接
    pub fn add_link(&mut self, link: SpanLink) {
        if self.ended {
            return;
        }
        self.links.push(link);
    }

    /// 设置为成功状态
    pub fn set_ok(&mut self) {
        if self.ended {
            return;
        }
        self.status = SpanStatus::ok();
    }

    /// 设置为错误状态
    pub fn set_error(&mut self, description: impl Into<String>) {
        if self.ended {
            return;
        }
        self.status = SpanStatus::error(description);
    }

    /// 结束 Span（递交处理器）— 幂等
    pub fn end(mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        let end_time_ns = current_time_ns();
        let data = SpanData {
            name: std::mem::take(&mut self.name),
            span_context: self.span_context,
            parent_span_id: self.parent_span_id,
            kind: self.kind,
            start_time_ns: self.start_time_ns,
            end_time_ns,
            attributes: std::mem::take(&mut self.attributes),
            events: std::mem::take(&mut self.events),
            links: std::mem::take(&mut self.links),
            status: self.status,
            resource: std::mem::take(&mut self.resource),
        };
        if let Some(processor) = &self.processor {
            processor.on_end(data);
        }
    }

    /// 返回属性引用
    pub fn attributes(&self) -> &HashMap<String, SpanAttributeValue> {
        &self.attributes
    }

    /// 返回事件引用
    pub fn events(&self) -> &[SpanEvent] {
        &self.events
    }

    /// 返回链接引用
    pub fn links(&self) -> &[SpanLink] {
        &self.links
    }
}

/// 当前时间（纳秒）— 测试可替换为固定值
fn current_time_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

// =====================================================================
//  OtlpExporter — OTLP JSON 导出器
// =====================================================================

/// OTLP JSON 导出器 — 导出 SpanData 为 OTLP JSON 格式
pub struct OtlpExporter;

impl OtlpExporter {
    /// 导出单个 Span 为 OTLP JSON 字符串
    pub fn export_span(span: &SpanData) -> String {
        let mut out = String::new();
        out.push_str("{\"resourceSpans\":[{\"resource\":{\"attributes\":[");
        let mut first = true;
        for (k, v) in &span.resource {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"key\":\"{}\",\"value\":{{\"stringValue\":\"{}\"}}}}",
                escape_json_string(k),
                escape_json_string(v)
            ));
        }
        out.push_str("]},\"scopeSpans\":[{\"scope\":{\"name\":\"szrsql-ops\"},\"spans\":[");
        out.push_str(&Self::span_to_json(span));
        out.push_str("]}]}]}");
        out
    }

    /// 批量导出 Span 列表为 OTLP JSON 字符串
    pub fn export_batch(spans: &[SpanData]) -> String {
        if spans.is_empty() {
            return "{\"resourceSpans\":[]}".to_string();
        }
        let mut out = String::new();
        out.push_str("{\"resourceSpans\":[{\"resource\":{\"attributes\":[");
        let mut first = true;
        for (k, v) in &spans[0].resource {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"key\":\"{}\",\"value\":{{\"stringValue\":\"{}\"}}}}",
                escape_json_string(k),
                escape_json_string(v)
            ));
        }
        out.push_str("]},\"scopeSpans\":[{\"scope\":{\"name\":\"szrsql-ops\"},\"spans\":[");
        let mut first_span = true;
        for span in spans {
            if !first_span {
                out.push(',');
            }
            first_span = false;
            out.push_str(&Self::span_to_json(span));
        }
        out.push_str("]}]}]}");
        out
    }

    /// 单个 Span 转 JSON
    fn span_to_json(span: &SpanData) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{{\"traceId\":\"{}\"",
            span.span_context.trace_id().to_hex()
        ));
        out.push_str(&format!(
            ",\"spanId\":\"{}\"",
            span.span_context.span_id().to_hex()
        ));
        if let Some(parent) = span.parent_span_id {
            out.push_str(&format!(",\"parentSpanId\":\"{}\"", parent.to_hex()));
        }
        out.push_str(&format!(",\"name\":\"{}\"", escape_json_string(&span.name)));
        out.push_str(&format!(",\"kind\":\"SPAN_KIND_{}\"", span.kind));
        out.push_str(&format!(
            ",\"startTimeUnixNano\":\"{}\"",
            span.start_time_ns
        ));
        out.push_str(&format!(",\"endTimeUnixNano\":\"{}\"", span.end_time_ns));
        // 属性
        out.push_str(",\"attributes\":[");
        let mut first = true;
        for (k, v) in &span.attributes {
            if !first {
                out.push(',');
            }
            first = false;
            let type_name = match v {
                SpanAttributeValue::String(_) => "stringValue",
                SpanAttributeValue::Int(_) => "intValue",
                SpanAttributeValue::Float(_) => "doubleValue",
                SpanAttributeValue::Bool(_) => "boolValue",
                SpanAttributeValue::Bytes(_) => "bytesValue",
                SpanAttributeValue::StringArray(_) => "arrayValue",
            };
            out.push_str(&format!(
                "{{\"key\":\"{}\",\"value\":{{\"{}\":{}}}}}",
                escape_json_string(k),
                type_name,
                v.to_json_value()
            ));
        }
        out.push(']');
        // 事件
        if !span.events.is_empty() {
            out.push_str(",\"events\":[");
            let mut first = true;
            for event in &span.events {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&format!(
                    "{{\"name\":\"{}\",\"timeUnixNano\":\"{}\"}}",
                    escape_json_string(&event.name),
                    event.timestamp_ns
                ));
            }
            out.push(']');
        }
        // 状态
        if span.status.code() != StatusCode::Unset {
            out.push_str(&format!(
                ",\"status\":{{\"code\":\"{}\"",
                span.status.code()
            ));
            if let Some(desc) = span.status.description() {
                out.push_str(&format!(",\"message\":\"{}\"", escape_json_string(desc)));
            }
            out.push('}');
        }
        out.push('}');
        out
    }
}

// =====================================================================
//  Context — 上下文传播
// =====================================================================

/// 上下文 — 活动 Span + baggage
#[derive(Debug, Clone, Default)]
pub struct Context {
    active_span: Option<SpanContext>,
    baggage: HashMap<String, String>,
}

impl Context {
    /// 构造空上下文
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置活动 Span
    pub fn with_span(mut self, span: SpanContext) -> Self {
        self.active_span = Some(span);
        self
    }

    /// 设置活动 Span（可变）
    pub fn set_span(&mut self, span: SpanContext) {
        self.active_span = Some(span);
    }

    /// 返回活动 Span
    pub fn active_span(&self) -> Option<SpanContext> {
        self.active_span
    }

    /// 添加 baggage
    pub fn with_baggage(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.baggage.insert(key.into(), value.into());
        self
    }

    /// 注入到 W3C traceparent + baggage 头（inject）
    pub fn inject(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        if let Some(span_ctx) = self.active_span {
            if span_ctx.is_valid() {
                headers.insert("traceparent".to_string(), span_ctx.to_traceparent());
            }
        }
        if !self.baggage.is_empty() {
            let baggage_str: Vec<String> = self
                .baggage
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            headers.insert("baggage".to_string(), baggage_str.join(","));
        }
        headers
    }

    /// 从 W3C traceparent + baggage 头提取（extract）
    pub fn extract(headers: &HashMap<String, String>) -> Self {
        let mut ctx = Context::new();
        if let Some(tp) = headers.get("traceparent") {
            if let Some(span_ctx) = SpanContext::from_traceparent(tp) {
                if span_ctx.is_valid() {
                    ctx.active_span = Some(span_ctx);
                }
            }
        }
        if let Some(baggage_str) = headers.get("baggage") {
            for item in baggage_str.split(',') {
                if let Some((k, v)) = item.split_once('=') {
                    ctx.baggage
                        .insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        ctx
    }
}

// =====================================================================
//  sql_attributes — SQL 查询属性键常量
// =====================================================================

/// SQL 查询相关属性键（OTel 语义约定）
pub mod sql_attributes {
    /// SQL 语句文本
    pub const DB_STATEMENT: &str = "db.statement";
    /// 数据库系统（如 szrsql）
    pub const DB_SYSTEM: &str = "db.system";
    /// 数据库名
    pub const DB_NAME: &str = "db.name";
    /// 用户名
    pub const DB_USER: &str = "db.user";
    /// 操作类型（SELECT/INSERT/UPDATE/DELETE）
    pub const DB_OPERATION: &str = "db.operation";
    /// 表名
    pub const DB_TABLE: &str = "db.sql.table";
    /// 扫描行数
    pub const DB_ROWS_SCANNED: &str = "db.rows_scanned";
    /// 返回行数
    pub const DB_ROWS_RETURNED: &str = "db.rows_returned";
    /// 扫描字节数
    pub const DB_BYTES_SCANNED: &str = "db.bytes_scanned";
    /// 使用的索引
    pub const DB_INDEX_USED: &str = "db.index_used";
    /// JOIN 数量
    pub const DB_JOIN_COUNT: &str = "db.join_count";
    /// 是否全表扫描
    pub const DB_SEQ_SCAN: &str = "db.seq_scan";
    /// 查询计划
    pub const DB_PLAN: &str = "db.plan";
    /// 错误码
    pub const DB_ERROR_CODE: &str = "db.error_code";
}

// =====================================================================
//  SqlSpanBuilder — SQL 查询专用 Span 构造器
// =====================================================================

/// SQL 查询专用 Span 构造器
pub struct SqlSpanBuilder<'a> {
    builder: ProviderSpanBuilder<'a>,
}

impl<'a> SqlSpanBuilder<'a> {
    /// 构造
    pub fn new(provider: &'a TracerProvider, sql: impl Into<String>) -> Self {
        let builder = provider
            .tracer()
            .span_builder("sql.query")
            .with_attribute(
                sql_attributes::DB_STATEMENT,
                SpanAttributeValue::String(sql.into()),
            )
            .with_attribute(
                sql_attributes::DB_SYSTEM,
                SpanAttributeValue::String("szrsql".to_string()),
            );
        Self { builder }
    }

    /// 设置数据库名
    pub fn with_database(mut self, db: impl Into<String>) -> Self {
        self.builder = self.builder.with_attribute(
            sql_attributes::DB_NAME,
            SpanAttributeValue::String(db.into()),
        );
        self
    }

    /// 设置用户名
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.builder = self.builder.with_attribute(
            sql_attributes::DB_USER,
            SpanAttributeValue::String(user.into()),
        );
        self
    }

    /// 设置操作类型
    pub fn with_operation(mut self, op: impl Into<String>) -> Self {
        self.builder = self.builder.with_attribute(
            sql_attributes::DB_OPERATION,
            SpanAttributeValue::String(op.into()),
        );
        self
    }

    /// 设置表名
    pub fn with_table(mut self, table: impl Into<String>) -> Self {
        self.builder = self.builder.with_attribute(
            sql_attributes::DB_TABLE,
            SpanAttributeValue::String(table.into()),
        );
        self
    }

    /// 设置扫描行数
    pub fn with_rows_scanned(mut self, rows: i64) -> Self {
        self.builder = self.builder.with_attribute(
            sql_attributes::DB_ROWS_SCANNED,
            SpanAttributeValue::Int(rows),
        );
        self
    }

    /// 设置返回行数
    pub fn with_rows_returned(mut self, rows: i64) -> Self {
        self.builder = self.builder.with_attribute(
            sql_attributes::DB_ROWS_RETURNED,
            SpanAttributeValue::Int(rows),
        );
        self
    }

    /// 设置使用的索引
    pub fn with_index_used(mut self, index: impl Into<String>) -> Self {
        self.builder = self.builder.with_attribute(
            sql_attributes::DB_INDEX_USED,
            SpanAttributeValue::String(index.into()),
        );
        self
    }

    /// 设置 JOIN 数量
    pub fn with_join_count(mut self, count: i64) -> Self {
        self.builder = self.builder.with_attribute(
            sql_attributes::DB_JOIN_COUNT,
            SpanAttributeValue::Int(count),
        );
        self
    }

    /// 设置是否全表扫描
    pub fn with_seq_scan(mut self, seq_scan: bool) -> Self {
        self.builder = self.builder.with_attribute(
            sql_attributes::DB_SEQ_SCAN,
            SpanAttributeValue::Bool(seq_scan),
        );
        self
    }

    /// 设置查询计划
    pub fn with_plan(mut self, plan: impl Into<String>) -> Self {
        self.builder = self.builder.with_attribute(
            sql_attributes::DB_PLAN,
            SpanAttributeValue::String(plan.into()),
        );
        self
    }

    /// 设置为客户端类型
    pub fn as_client(mut self) -> Self {
        self.builder = self.builder.with_kind(SpanKind::Client);
        self
    }

    /// 设置为服务端类型
    pub fn as_server(mut self) -> Self {
        self.builder = self.builder.with_kind(SpanKind::Server);
        self
    }

    /// 启动 Span
    pub fn start(self) -> Span {
        self.builder.start()
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  TraceId 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_trace_id_zero() {
        let id = TraceId::zero();
        assert!(id.is_invalid());
        assert_eq!(id.to_hex(), "00000000000000000000000000000000");
    }

    #[test]
    fn test_trace_id_from_bytes() {
        let bytes = [1u8; TRACE_ID_LEN];
        let id = TraceId::from_bytes(bytes);
        assert!(!id.is_invalid());
        assert_eq!(id.as_bytes(), &bytes);
    }

    #[test]
    fn test_trace_id_from_hex() {
        let hex = "0123456789abcdef0123456789abcdef";
        let id = TraceId::from_hex(hex).unwrap();
        assert!(!id.is_invalid());
        assert_eq!(id.to_hex(), hex);
    }

    #[test]
    fn test_trace_id_from_hex_invalid_length() {
        assert!(TraceId::from_hex("0123").is_none());
        assert!(TraceId::from_hex("").is_none());
    }

    #[test]
    fn test_trace_id_from_hex_invalid_chars() {
        let hex = "0123456789abcdef0123456789abcdeg";
        assert!(TraceId::from_hex(hex).is_none());
    }

    #[test]
    fn test_trace_id_display() {
        let hex = "0123456789abcdef0123456789abcdef";
        let id = TraceId::from_hex(hex).unwrap();
        assert_eq!(format!("{}", id), hex);
    }

    // -----------------------------------------------------------------
    //  SpanId 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_span_id_zero() {
        let id = SpanId::zero();
        assert!(id.is_invalid());
        assert_eq!(id.to_hex(), "0000000000000000");
    }

    #[test]
    fn test_span_id_from_bytes() {
        let bytes = [42u8; SPAN_ID_LEN];
        let id = SpanId::from_bytes(bytes);
        assert!(!id.is_invalid());
        assert_eq!(id.as_bytes(), &bytes);
    }

    #[test]
    fn test_span_id_from_hex() {
        let hex = "0123456789abcdef";
        let id = SpanId::from_hex(hex).unwrap();
        assert!(!id.is_invalid());
        assert_eq!(id.to_hex(), hex);
    }

    #[test]
    fn test_span_id_from_hex_invalid_length() {
        assert!(SpanId::from_hex("0123").is_none());
    }

    #[test]
    fn test_span_id_display() {
        let hex = "0123456789abcdef";
        let id = SpanId::from_hex(hex).unwrap();
        assert_eq!(format!("{}", id), hex);
    }

    // -----------------------------------------------------------------
    //  TraceFlags 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_trace_flags_default() {
        let flags = TraceFlags::new();
        assert!(!flags.is_sampled());
        assert_eq!(flags.as_u8(), 0);
    }

    #[test]
    fn test_trace_flags_sampled() {
        let flags = TraceFlags::new().with_sampled(true);
        assert!(flags.is_sampled());
        assert_eq!(flags.as_u8(), 1);
    }

    #[test]
    fn test_trace_flags_not_sampled() {
        let flags = TraceFlags::new().with_sampled(false);
        assert!(!flags.is_sampled());
    }

    #[test]
    fn test_trace_flags_display() {
        let flags = TraceFlags::new().with_sampled(true);
        let s = format!("{:?}", flags);
        assert!(s.contains("sampled=true"));
    }

    // -----------------------------------------------------------------
    //  SpanContext 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_span_context_new() {
        let trace_id = TraceId::from_hex("0123456789abcdef0123456789abcdef").unwrap();
        let span_id = SpanId::from_hex("0123456789abcdef").unwrap();
        let ctx = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::new().with_sampled(true),
            false,
        );
        assert!(ctx.is_valid());
        assert!(ctx.is_sampled());
        assert!(!ctx.is_remote());
    }

    #[test]
    fn test_span_context_empty() {
        let ctx = SpanContext::empty();
        assert!(!ctx.is_valid());
    }

    #[test]
    fn test_span_context_remote() {
        let trace_id = TraceId::from_hex("0123456789abcdef0123456789abcdef").unwrap();
        let span_id = SpanId::from_hex("0123456789abcdef").unwrap();
        let ctx = SpanContext::new(trace_id, span_id, TraceFlags::DEFAULT, true);
        assert!(ctx.is_remote());
    }

    #[test]
    fn test_span_context_to_traceparent() {
        let trace_id = TraceId::from_hex("0123456789abcdef0123456789abcdef").unwrap();
        let span_id = SpanId::from_hex("0123456789abcdef").unwrap();
        let ctx = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::new().with_sampled(true),
            false,
        );
        let tp = ctx.to_traceparent();
        assert_eq!(
            tp,
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01"
        );
    }

    #[test]
    fn test_span_context_from_traceparent() {
        let tp = "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01";
        let ctx = SpanContext::from_traceparent(tp).unwrap();
        assert!(ctx.is_valid());
        assert!(ctx.is_sampled());
        assert!(ctx.is_remote());
    }

    #[test]
    fn test_span_context_from_traceparent_invalid() {
        assert!(SpanContext::from_traceparent("invalid").is_none());
        assert!(SpanContext::from_traceparent(
            "00-00000000000000000000000000000000-0000000000000000-00"
        )
        .is_some());
    }

    #[test]
    fn test_span_context_traceparent_roundtrip() {
        let trace_id = TraceId::from_hex("abcdef0123456789abcdef0123456789").unwrap();
        let span_id = SpanId::from_hex("abcdef0123456789").unwrap();
        let ctx = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::new().with_sampled(true),
            false,
        );
        let tp = ctx.to_traceparent();
        let ctx2 = SpanContext::from_traceparent(&tp).unwrap();
        // from_traceparent 提取的上下文 is_remote=true（OTel 规范），只比较关键字段
        assert_eq!(ctx.trace_id(), ctx2.trace_id());
        assert_eq!(ctx.span_id(), ctx2.span_id());
        assert_eq!(ctx.is_sampled(), ctx2.is_sampled());
        assert!(ctx2.is_remote());
    }

    // -----------------------------------------------------------------
    //  StatusCode / SpanStatus 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_status_code_as_str() {
        assert_eq!(StatusCode::Unset.as_str(), "UNSET");
        assert_eq!(StatusCode::Ok.as_str(), "OK");
        assert_eq!(StatusCode::Error.as_str(), "ERROR");
    }

    #[test]
    fn test_status_code_predicates() {
        assert!(StatusCode::Error.is_error());
        assert!(StatusCode::Ok.is_ok());
        assert!(!StatusCode::Unset.is_error());
        assert!(!StatusCode::Unset.is_ok());
    }

    #[test]
    fn test_span_status_unset() {
        let status = SpanStatus::unset();
        assert_eq!(status.code(), StatusCode::Unset);
        assert!(!status.is_error());
        assert!(status.description().is_none());
    }

    #[test]
    fn test_span_status_ok() {
        let status = SpanStatus::ok();
        assert_eq!(status.code(), StatusCode::Ok);
        assert!(!status.is_error());
    }

    #[test]
    fn test_span_status_error() {
        let status = SpanStatus::error("query failed");
        assert!(status.is_error());
        assert_eq!(status.description(), Some("query failed"));
    }

    // -----------------------------------------------------------------
    //  SpanKind 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_span_kind_as_str() {
        assert_eq!(SpanKind::Internal.as_str(), "INTERNAL");
        assert_eq!(SpanKind::Server.as_str(), "SERVER");
        assert_eq!(SpanKind::Client.as_str(), "CLIENT");
        assert_eq!(SpanKind::Producer.as_str(), "PRODUCER");
        assert_eq!(SpanKind::Consumer.as_str(), "CONSUMER");
    }

    #[test]
    fn test_span_kind_predicates() {
        assert!(SpanKind::Internal.is_internal());
        assert!(SpanKind::Server.is_server());
        assert!(SpanKind::Client.is_client());
    }

    // -----------------------------------------------------------------
    //  SpanAttributeValue 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_span_attribute_value_string() {
        let v = SpanAttributeValue::String("hello".to_string());
        assert!(v.is_string());
        assert_eq!(v.to_json_value(), "\"hello\"");
    }

    #[test]
    fn test_span_attribute_value_int() {
        let v = SpanAttributeValue::Int(42);
        assert!(v.is_int());
        assert_eq!(v.to_json_value(), "42");
    }

    #[test]
    fn test_span_attribute_value_float() {
        let v = SpanAttributeValue::Float(2.71);
        assert!(v.is_float());
        assert_eq!(v.to_json_value(), "2.71");
    }

    #[test]
    fn test_span_attribute_value_bool() {
        let v = SpanAttributeValue::Bool(true);
        assert!(v.is_bool());
        assert_eq!(v.to_json_value(), "true");
    }

    #[test]
    fn test_span_attribute_value_bytes() {
        let v = SpanAttributeValue::Bytes(vec![0x01, 0x02, 0xff]);
        assert_eq!(v.to_json_value(), "\"0102ff\"");
    }

    #[test]
    fn test_span_attribute_value_string_array() {
        let v = SpanAttributeValue::StringArray(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(v.to_json_value(), "[\"a\",\"b\"]");
    }

    #[test]
    fn test_span_attribute_value_from_str() {
        let v: SpanAttributeValue = "hello".into();
        assert!(v.is_string());
    }

    #[test]
    fn test_span_attribute_value_from_i64() {
        let v: SpanAttributeValue = 42i64.into();
        assert!(v.is_int());
    }

    #[test]
    fn test_span_attribute_value_from_f64() {
        let v: SpanAttributeValue = 2.71f64.into();
        assert!(v.is_float());
    }

    #[test]
    fn test_span_attribute_value_from_bool() {
        let v: SpanAttributeValue = true.into();
        assert!(v.is_bool());
    }

    #[test]
    fn test_escape_json_string() {
        assert_eq!(escape_json_string("hello"), "hello");
        assert_eq!(escape_json_string("a\"b"), "a\\\"b");
        assert_eq!(escape_json_string("a\\b"), "a\\\\b");
        assert_eq!(escape_json_string("a\nb"), "a\\nb");
        assert_eq!(escape_json_string("a\tb"), "a\\tb");
    }

    // -----------------------------------------------------------------
    //  SpanEvent / SpanLink 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_span_event_new() {
        let event = SpanEvent::new("query.start", 12345);
        assert_eq!(event.name, "query.start");
        assert_eq!(event.timestamp_ns, 12345);
        assert!(event.attributes.is_empty());
    }

    #[test]
    fn test_span_event_with_attribute() {
        let event = SpanEvent::new("query.end", 12345).with_attribute("rows", 100i64);
        assert_eq!(event.attributes.len(), 1);
    }

    #[test]
    fn test_span_link_new() {
        let ctx = SpanContext::new(
            TraceId::from_hex("0123456789abcdef0123456789abcdef").unwrap(),
            SpanId::from_hex("0123456789abcdef").unwrap(),
            TraceFlags::DEFAULT,
            false,
        );
        let link = SpanLink::new(ctx);
        assert_eq!(link.span_context, ctx);
        assert!(link.attributes.is_empty());
    }

    // -----------------------------------------------------------------
    //  SpanData 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_span_data_duration() {
        let span_data = SpanData {
            name: "test".to_string(),
            span_context: SpanContext::empty(),
            parent_span_id: None,
            kind: SpanKind::Internal,
            start_time_ns: 1_000_000,
            end_time_ns: 1_500_000,
            attributes: HashMap::new(),
            events: Vec::new(),
            links: Vec::new(),
            status: SpanStatus::unset(),
            resource: HashMap::new(),
        };
        assert_eq!(span_data.duration_ns(), 500_000);
        assert_eq!(span_data.duration_us(), 500);
        assert_eq!(span_data.duration_ms(), 0);
    }

    #[test]
    fn test_span_data_duration_ms() {
        let span_data = SpanData {
            name: "test".to_string(),
            span_context: SpanContext::empty(),
            parent_span_id: None,
            kind: SpanKind::Internal,
            start_time_ns: 0,
            end_time_ns: 1_500_000,
            attributes: HashMap::new(),
            events: Vec::new(),
            links: Vec::new(),
            status: SpanStatus::unset(),
            resource: HashMap::new(),
        };
        assert_eq!(span_data.duration_ms(), 1);
    }

    #[test]
    fn test_span_data_is_root() {
        let mut span_data = SpanData {
            name: "test".to_string(),
            span_context: SpanContext::empty(),
            parent_span_id: None,
            kind: SpanKind::Internal,
            start_time_ns: 0,
            end_time_ns: 0,
            attributes: HashMap::new(),
            events: Vec::new(),
            links: Vec::new(),
            status: SpanStatus::unset(),
            resource: HashMap::new(),
        };
        assert!(span_data.is_root());
        span_data.parent_span_id = Some(SpanId::from_bytes([1; SPAN_ID_LEN]));
        assert!(!span_data.is_root());
    }

    #[test]
    fn test_span_data_is_error() {
        let mut span_data = SpanData {
            name: "test".to_string(),
            span_context: SpanContext::empty(),
            parent_span_id: None,
            kind: SpanKind::Internal,
            start_time_ns: 0,
            end_time_ns: 0,
            attributes: HashMap::new(),
            events: Vec::new(),
            links: Vec::new(),
            status: SpanStatus::unset(),
            resource: HashMap::new(),
        };
        assert!(!span_data.is_error());
        span_data.status = SpanStatus::error("failed");
        assert!(span_data.is_error());
    }

    #[test]
    fn test_span_data_get_attributes() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "key_str".to_string(),
            SpanAttributeValue::String("value".to_string()),
        );
        attrs.insert("key_int".to_string(), SpanAttributeValue::Int(42));
        attrs.insert("key_float".to_string(), SpanAttributeValue::Float(2.71));
        attrs.insert("key_bool".to_string(), SpanAttributeValue::Bool(true));
        let span_data = SpanData {
            name: "test".to_string(),
            span_context: SpanContext::empty(),
            parent_span_id: None,
            kind: SpanKind::Internal,
            start_time_ns: 0,
            end_time_ns: 0,
            attributes: attrs,
            events: Vec::new(),
            links: Vec::new(),
            status: SpanStatus::unset(),
            resource: HashMap::new(),
        };
        assert_eq!(span_data.get_string_attribute("key_str"), Some("value"));
        assert_eq!(span_data.get_int_attribute("key_int"), Some(42));
        assert_eq!(span_data.get_float_attribute("key_float"), Some(2.71));
        assert_eq!(span_data.get_bool_attribute("key_bool"), Some(true));
        assert_eq!(span_data.get_string_attribute("missing"), None);
    }

    // -----------------------------------------------------------------
    //  Span 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_span_basic() {
        let provider = TracerProvider::default();
        let span = provider.tracer().start_span("test.span");
        assert!(span.is_recording());
        assert_eq!(span.name, "test.span");
        span.end();
    }

    #[test]
    fn test_span_set_attribute() {
        let provider = TracerProvider::default();
        let mut span = provider.tracer().start_span("test.span");
        span.set_attribute("key1", "value1");
        span.set_attribute("key2", 42i64);
        assert_eq!(span.attributes().len(), 2);
        span.end();
    }

    #[test]
    fn test_span_add_event() {
        let provider = TracerProvider::default();
        let mut span = provider.tracer().start_span("test.span");
        span.add_event("event1");
        assert_eq!(span.events().len(), 1);
        span.end();
    }

    #[test]
    fn test_span_add_event_with_attributes() {
        let provider = TracerProvider::default();
        let mut span = provider.tracer().start_span("test.span");
        let mut attrs = HashMap::new();
        attrs.insert("rows".to_string(), SpanAttributeValue::Int(100));
        span.add_event_with_attributes("event1", attrs);
        assert_eq!(span.events().len(), 1);
        assert_eq!(span.events()[0].attributes.len(), 1);
        span.end();
    }

    #[test]
    fn test_span_add_link() {
        let provider = TracerProvider::default();
        let mut span = provider.tracer().start_span("test.span");
        let ctx = SpanContext::empty();
        span.add_link(SpanLink::new(ctx));
        assert_eq!(span.links().len(), 1);
        span.end();
    }

    #[test]
    fn test_span_set_ok() {
        let provider = TracerProvider::default();
        let mut span = provider.tracer().start_span("test.span");
        span.set_ok();
        span.end();
    }

    #[test]
    fn test_span_set_error() {
        let provider = TracerProvider::default();
        let mut span = provider.tracer().start_span("test.span");
        span.set_error("query failed");
        span.end();
    }

    #[test]
    fn test_span_end_idempotent() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        let span = provider.tracer().start_span("test.span");
        span.end();
        // Span 已 end，无法再次 end（consume）
        assert_eq!(processor.len(), 1);
    }

    // -----------------------------------------------------------------
    //  SimpleSpanProcessor 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_simple_processor_new() {
        let processor = SimpleSpanProcessor::new();
        assert!(processor.is_empty());
        assert_eq!(processor.len(), 0);
    }

    #[test]
    fn test_simple_processor_with_provider() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        let span = provider.tracer().start_span("test.span");
        span.end();
        assert_eq!(processor.len(), 1);
        let spans = processor.spans();
        assert_eq!(spans[0].name, "test.span");
    }

    #[test]
    fn test_simple_processor_multiple_spans() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        for i in 0..5 {
            let span = provider.tracer().start_span(format!("span.{}", i));
            span.end();
        }
        assert_eq!(processor.len(), 5);
    }

    // -----------------------------------------------------------------
    //  BatchSpanProcessor 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_batch_processor_new() {
        let processor = BatchSpanProcessor::new(10);
        assert_eq!(processor.buffered_len(), 0);
        assert_eq!(processor.exported_len(), 0);
        assert_eq!(processor.total_len(), 0);
        assert_eq!(processor.export_count(), 0);
    }

    #[test]
    fn test_batch_processor_flush_on_threshold() {
        let processor = Arc::new(BatchSpanProcessor::new(3));
        let provider = TracerProvider::default().with_batch_processor(processor.clone());
        for i in 0..3 {
            let span = provider.tracer().start_span(format!("span.{}", i));
            span.end();
        }
        // 3 个 Span 达到阈值，自动导出
        assert_eq!(processor.buffered_len(), 0);
        assert_eq!(processor.exported_len(), 3);
        assert_eq!(processor.export_count(), 1);
    }

    #[test]
    fn test_batch_processor_partial_flush() {
        let processor = Arc::new(BatchSpanProcessor::new(10));
        let provider = TracerProvider::default().with_batch_processor(processor.clone());
        for i in 0..5 {
            let span = provider.tracer().start_span(format!("span.{}", i));
            span.end();
        }
        // 5 < 10，未达到阈值，未自动导出
        assert_eq!(processor.buffered_len(), 5);
        assert_eq!(processor.exported_len(), 0);
        // 强制刷新
        processor.force_flush();
        assert_eq!(processor.buffered_len(), 0);
        assert_eq!(processor.exported_len(), 5);
        assert_eq!(processor.export_count(), 1);
    }

    // -----------------------------------------------------------------
    //  IdGenerator 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sequential_id_generator_unique() {
        let gen = SequentialIdGenerator::new();
        let id1 = gen.new_trace_id();
        let id2 = gen.new_trace_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_sequential_id_generator_span_id() {
        let gen = SequentialIdGenerator::new();
        let id1 = gen.new_span_id();
        let id2 = gen.new_span_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_sequential_id_generator_with_start() {
        let gen = SequentialIdGenerator::with_start(100);
        let id = gen.new_trace_id();
        // 第一个 trace_id 的 counter=100，fetch_add 返回 100
        let bytes = id.as_bytes();
        let mut expected = [0u8; TRACE_ID_LEN];
        expected[8..16].copy_from_slice(&100u64.to_be_bytes());
        assert_eq!(bytes, &expected);
    }

    // -----------------------------------------------------------------
    //  Tracer / Provider 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_provider_new() {
        let provider = TracerProvider::new(TracerConfig::new("test-service"));
        assert_eq!(provider.service_name(), "test-service");
        assert!(provider.should_sample());
    }

    #[test]
    fn test_provider_with_config() {
        let config = TracerConfig::new("my-service").with_sampling(false);
        let provider = TracerProvider::default().with_config(config);
        assert_eq!(provider.service_name(), "my-service");
        assert!(!provider.should_sample());
    }

    #[test]
    fn test_provider_start_span() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        let span = provider.tracer().start_span("test");
        span.end();
        assert_eq!(processor.len(), 1);
        let spans = processor.spans();
        assert!(spans[0].is_root());
        assert!(spans[0].span_context.is_valid());
    }

    #[test]
    fn test_span_builder_with_kind() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        let span = provider
            .tracer()
            .span_builder("test")
            .with_kind(SpanKind::Server)
            .start();
        span.end();
        let spans = processor.spans();
        assert_eq!(spans[0].kind, SpanKind::Server);
    }

    #[test]
    fn test_span_builder_with_attributes() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        let span = provider
            .tracer()
            .span_builder("test")
            .with_attribute("key1", "value1")
            .with_attribute("key2", 42i64)
            .start();
        span.end();
        let spans = processor.spans();
        assert_eq!(spans[0].attributes.len(), 2);
    }

    #[test]
    fn test_child_span() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        let parent = provider.tracer().start_span("parent");
        let parent_ctx = parent.span_context();
        let child = provider
            .tracer()
            .span_builder_with_parent("child", parent_ctx)
            .start();
        let child_ctx = child.span_context();
        child.end();
        parent.end();
        let spans = processor.spans();
        // child 应该有相同的 trace_id 和 parent_span_id
        assert_eq!(spans[0].span_context.trace_id(), child_ctx.trace_id());
        assert_eq!(spans[0].span_context.trace_id(), parent_ctx.trace_id());
        assert_eq!(spans[0].parent_span_id, Some(parent_ctx.span_id()));
    }

    #[test]
    fn test_not_sampled() {
        let config = TracerConfig::default().with_sampling(false);
        let provider = TracerProvider::default().with_config(config);
        let span = provider.tracer().start_span("test");
        assert!(!span.span_context().is_sampled());
        span.end();
    }

    // -----------------------------------------------------------------
    //  OtlpExporter 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_otlp_export_span() {
        let span_data = SpanData {
            name: "test.span".to_string(),
            span_context: SpanContext::new(
                TraceId::from_hex("0123456789abcdef0123456789abcdef").unwrap(),
                SpanId::from_hex("0123456789abcdef").unwrap(),
                TraceFlags::new().with_sampled(true),
                false,
            ),
            parent_span_id: None,
            kind: SpanKind::Internal,
            start_time_ns: 1000,
            end_time_ns: 2000,
            attributes: HashMap::new(),
            events: Vec::new(),
            links: Vec::new(),
            status: SpanStatus::unset(),
            resource: HashMap::new(),
        };
        let json = OtlpExporter::export_span(&span_data);
        assert!(json.contains("\"resourceSpans\""));
        assert!(json.contains("\"traceId\":\"0123456789abcdef0123456789abcdef\""));
        assert!(json.contains("\"spanId\":\"0123456789abcdef\""));
        assert!(json.contains("\"name\":\"test.span\""));
    }

    #[test]
    fn test_otlp_export_with_parent() {
        let span_data = SpanData {
            name: "test.span".to_string(),
            span_context: SpanContext::new(
                TraceId::from_hex("0123456789abcdef0123456789abcdef").unwrap(),
                SpanId::from_hex("0123456789abcdef").unwrap(),
                TraceFlags::DEFAULT,
                false,
            ),
            parent_span_id: Some(SpanId::from_hex("fedcba9876543210").unwrap()),
            kind: SpanKind::Internal,
            start_time_ns: 1000,
            end_time_ns: 2000,
            attributes: HashMap::new(),
            events: Vec::new(),
            links: Vec::new(),
            status: SpanStatus::unset(),
            resource: HashMap::new(),
        };
        let json = OtlpExporter::export_span(&span_data);
        assert!(json.contains("\"parentSpanId\":\"fedcba9876543210\""));
    }

    #[test]
    fn test_otlp_export_with_attributes() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "db.statement".to_string(),
            SpanAttributeValue::String("SELECT 1".to_string()),
        );
        attrs.insert("db.rows".to_string(), SpanAttributeValue::Int(100));
        let span_data = SpanData {
            name: "test.span".to_string(),
            span_context: SpanContext::empty(),
            parent_span_id: None,
            kind: SpanKind::Internal,
            start_time_ns: 1000,
            end_time_ns: 2000,
            attributes: attrs,
            events: Vec::new(),
            links: Vec::new(),
            status: SpanStatus::unset(),
            resource: HashMap::new(),
        };
        let json = OtlpExporter::export_span(&span_data);
        assert!(json.contains("\"db.statement\""));
        assert!(json.contains("\"stringValue\":\"SELECT 1\""));
        assert!(json.contains("\"intValue\":100"));
    }

    #[test]
    fn test_otlp_export_with_events() {
        let span_data = SpanData {
            name: "test.span".to_string(),
            span_context: SpanContext::empty(),
            parent_span_id: None,
            kind: SpanKind::Internal,
            start_time_ns: 1000,
            end_time_ns: 2000,
            attributes: HashMap::new(),
            events: vec![SpanEvent::new("query.start", 1500)],
            links: Vec::new(),
            status: SpanStatus::unset(),
            resource: HashMap::new(),
        };
        let json = OtlpExporter::export_span(&span_data);
        assert!(json.contains("\"events\""));
        assert!(json.contains("\"query.start\""));
    }

    #[test]
    fn test_otlp_export_with_status_error() {
        let span_data = SpanData {
            name: "test.span".to_string(),
            span_context: SpanContext::empty(),
            parent_span_id: None,
            kind: SpanKind::Internal,
            start_time_ns: 1000,
            end_time_ns: 2000,
            attributes: HashMap::new(),
            events: Vec::new(),
            links: Vec::new(),
            status: SpanStatus::error("query failed"),
            resource: HashMap::new(),
        };
        let json = OtlpExporter::export_span(&span_data);
        assert!(json.contains("\"status\""));
        assert!(json.contains("\"ERROR\""));
        assert!(json.contains("\"query failed\""));
    }

    #[test]
    fn test_otlp_export_batch() {
        let span_data1 = SpanData {
            name: "span1".to_string(),
            span_context: SpanContext::empty(),
            parent_span_id: None,
            kind: SpanKind::Internal,
            start_time_ns: 1000,
            end_time_ns: 2000,
            attributes: HashMap::new(),
            events: Vec::new(),
            links: Vec::new(),
            status: SpanStatus::unset(),
            resource: HashMap::new(),
        };
        let span_data2 = SpanData {
            name: "span2".to_string(),
            ..span_data1.clone()
        };
        let json = OtlpExporter::export_batch(&[span_data1, span_data2]);
        assert!(json.contains("\"span1\""));
        assert!(json.contains("\"span2\""));
    }

    // -----------------------------------------------------------------
    //  Context Propagation 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_context_new() {
        let ctx = Context::new();
        assert!(ctx.active_span().is_none());
    }

    #[test]
    fn test_context_with_span() {
        let span_ctx = SpanContext::new(
            TraceId::from_hex("0123456789abcdef0123456789abcdef").unwrap(),
            SpanId::from_hex("0123456789abcdef").unwrap(),
            TraceFlags::new().with_sampled(true),
            false,
        );
        let ctx = Context::new().with_span(span_ctx);
        assert_eq!(ctx.active_span(), Some(span_ctx));
    }

    #[test]
    fn test_context_with_baggage() {
        let ctx = Context::new().with_baggage("key1", "value1");
        let headers = ctx.inject();
        assert!(headers.contains_key("baggage"));
        assert_eq!(headers.get("baggage").unwrap(), "key1=value1");
    }

    #[test]
    fn test_context_inject_extract_roundtrip() {
        let span_ctx = SpanContext::new(
            TraceId::from_hex("0123456789abcdef0123456789abcdef").unwrap(),
            SpanId::from_hex("0123456789abcdef").unwrap(),
            TraceFlags::new().with_sampled(true),
            false,
        );
        let ctx = Context::new()
            .with_span(span_ctx)
            .with_baggage("key1", "value1");
        let headers = ctx.inject();
        let extracted = Context::extract(&headers);
        let extracted_ctx = extracted.active_span().unwrap();
        // from_traceparent 提取的上下文 is_remote=true（OTel 规范），只比较关键字段
        assert_eq!(extracted_ctx.trace_id(), span_ctx.trace_id());
        assert_eq!(extracted_ctx.span_id(), span_ctx.span_id());
        assert_eq!(extracted_ctx.is_sampled(), span_ctx.is_sampled());
        assert!(extracted_ctx.is_remote());
    }

    #[test]
    fn test_context_inject_empty() {
        let ctx = Context::new();
        let headers = ctx.inject();
        assert!(headers.is_empty());
    }

    #[test]
    fn test_context_extract_empty() {
        let headers = HashMap::new();
        let ctx = Context::extract(&headers);
        assert!(ctx.active_span().is_none());
    }

    #[test]
    fn test_context_extract_invalid() {
        let mut headers = HashMap::new();
        headers.insert("traceparent".to_string(), "invalid".to_string());
        let ctx = Context::extract(&headers);
        assert!(ctx.active_span().is_none());
    }

    // -----------------------------------------------------------------
    //  SqlSpanBuilder 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sql_span_builder_basic() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        let span = SqlSpanBuilder::new(&provider, "SELECT * FROM users")
            .with_database("testdb")
            .with_user("admin")
            .with_operation("SELECT")
            .with_table("users")
            .start();
        span.end();
        let spans = processor.spans();
        assert_eq!(spans[0].name, "sql.query");
        assert_eq!(
            spans[0].get_string_attribute("db.statement"),
            Some("SELECT * FROM users")
        );
        assert_eq!(spans[0].get_string_attribute("db.system"), Some("szrsql"));
        assert_eq!(spans[0].get_string_attribute("db.name"), Some("testdb"));
        assert_eq!(spans[0].get_string_attribute("db.user"), Some("admin"));
        assert_eq!(
            spans[0].get_string_attribute("db.operation"),
            Some("SELECT")
        );
        assert_eq!(spans[0].get_string_attribute("db.sql.table"), Some("users"));
    }

    #[test]
    fn test_sql_span_builder_with_metrics() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        let span = SqlSpanBuilder::new(&provider, "SELECT * FROM big_table")
            .with_rows_scanned(1000000)
            .with_rows_returned(100)
            .with_index_used("idx_big_table_id")
            .with_join_count(2)
            .with_seq_scan(false)
            .start();
        span.end();
        let spans = processor.spans();
        assert_eq!(spans[0].get_int_attribute("db.rows_scanned"), Some(1000000));
        assert_eq!(spans[0].get_int_attribute("db.rows_returned"), Some(100));
        assert_eq!(
            spans[0].get_string_attribute("db.index_used"),
            Some("idx_big_table_id")
        );
        assert_eq!(spans[0].get_int_attribute("db.join_count"), Some(2));
        assert_eq!(spans[0].get_bool_attribute("db.seq_scan"), Some(false));
    }

    #[test]
    fn test_sql_span_builder_as_client() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        let span = SqlSpanBuilder::new(&provider, "SELECT 1")
            .as_client()
            .start();
        span.end();
        let spans = processor.spans();
        assert_eq!(spans[0].kind, SpanKind::Client);
    }

    #[test]
    fn test_sql_span_builder_as_server() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());
        let span = SqlSpanBuilder::new(&provider, "SELECT 1")
            .as_server()
            .start();
        span.end();
        let spans = processor.spans();
        assert_eq!(spans[0].kind, SpanKind::Server);
    }

    // -----------------------------------------------------------------
    //  集成测试
    // -----------------------------------------------------------------

    #[test]
    fn test_full_trace_workflow() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());

        // 根 Span
        let parent = provider.tracer().start_span("parent");
        let parent_ctx = parent.span_context();

        // 子 Span
        let mut child = provider
            .tracer()
            .span_builder_with_parent("child", parent_ctx)
            .start();
        child.set_attribute("key", "value");
        child.set_ok();
        child.end();

        parent.end();

        let spans = processor.spans();
        assert_eq!(spans.len(), 2);
        // 两个 Span 应该有相同的 trace_id
        assert_eq!(
            spans[0].span_context.trace_id(),
            spans[1].span_context.trace_id()
        );
    }

    #[test]
    fn test_sql_query_trace() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());

        let span = SqlSpanBuilder::new(&provider, "SELECT * FROM users WHERE id = 1")
            .with_database("testdb")
            .with_operation("SELECT")
            .with_table("users")
            .with_rows_scanned(1000)
            .with_rows_returned(1)
            .with_index_used("idx_users_id")
            .with_seq_scan(false)
            .as_server()
            .start();
        span.end();

        let spans = processor.spans();
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "sql.query");
        assert_eq!(span.kind, SpanKind::Server);
        assert_eq!(
            span.get_string_attribute("db.statement"),
            Some("SELECT * FROM users WHERE id = 1")
        );
        assert_eq!(span.get_int_attribute("db.rows_scanned"), Some(1000));
        assert_eq!(span.get_int_attribute("db.rows_returned"), Some(1));
        assert!(span.get_bool_attribute("db.seq_scan").is_some());
    }

    #[test]
    fn test_distributed_trace() {
        // 模拟跨进程传播
        let processor1 = Arc::new(SimpleSpanProcessor::new());
        let provider1 = TracerProvider::default().with_simple_processor(processor1.clone());

        // 进程 A：创建根 Span
        let span_a = provider1.tracer().start_span("service.a");
        let ctx_a = span_a.span_context();

        // 注入到 headers
        let context = Context::new().with_span(ctx_a);
        let headers = context.inject();
        assert!(headers.contains_key("traceparent"));

        // 进程 B：从 headers 提取
        let processor2 = Arc::new(SimpleSpanProcessor::new());
        let provider2 = TracerProvider::default().with_simple_processor(processor2.clone());
        let extracted = Context::extract(&headers);
        let parent_ctx = extracted.active_span().unwrap();

        // 创建子 Span
        let span_b = provider2
            .tracer()
            .span_builder_with_parent("service.b", parent_ctx)
            .start();
        span_b.end();
        span_a.end();

        // 两个 Span 应该有相同的 trace_id
        let spans_a = processor1.spans();
        let spans_b = processor2.spans();
        assert_eq!(
            spans_a[0].span_context.trace_id(),
            spans_b[0].span_context.trace_id()
        );
        assert_eq!(spans_b[0].parent_span_id, Some(ctx_a.span_id()));
    }

    #[test]
    fn test_otlp_export_full_workflow() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());

        let span = SqlSpanBuilder::new(&provider, "SELECT * FROM users")
            .with_rows_scanned(100)
            .start();
        span.end();

        let spans = processor.spans();
        let json = OtlpExporter::export_batch(&spans);
        assert!(json.contains("\"resourceSpans\""));
        assert!(json.contains("\"scopeSpans\""));
        assert!(json.contains("\"spans\""));
        assert!(json.contains("\"db.statement\""));
        assert!(json.contains("\"intValue\":100"));
    }

    #[test]
    fn test_error_trace() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());

        let mut span = SqlSpanBuilder::new(&provider, "INSERT INTO users VALUES (...)")
            .with_operation("INSERT")
            .start();
        span.set_error("duplicate key violation");
        span.end();

        let spans = processor.spans();
        assert!(spans[0].is_error());
        assert_eq!(spans[0].status.code(), StatusCode::Error);
        assert_eq!(
            spans[0].status.description(),
            Some("duplicate key violation")
        );
    }

    #[test]
    fn test_batch_export_full_workflow() {
        let processor = Arc::new(BatchSpanProcessor::new(5));
        let provider = TracerProvider::default().with_batch_processor(processor.clone());

        for i in 0..7 {
            let span = provider.tracer().start_span(format!("span.{}", i));
            span.end();
        }

        // 7 > 5，触发一次自动导出（5 个），剩余 2 个在缓冲区
        assert_eq!(processor.exported_len(), 5);
        assert_eq!(processor.buffered_len(), 2);

        // 强制刷新剩余
        processor.force_flush();
        assert_eq!(processor.exported_len(), 7);
        assert_eq!(processor.buffered_len(), 0);
        assert_eq!(processor.export_count(), 2);
    }

    #[test]
    fn test_nested_spans() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());

        let root = provider.tracer().start_span("root");
        let root_ctx = root.span_context();

        let child = provider
            .tracer()
            .span_builder_with_parent("child", root_ctx)
            .start();
        let child_ctx = child.span_context();

        let grandchild = provider
            .tracer()
            .span_builder_with_parent("grandchild", child_ctx)
            .start();
        grandchild.end();
        child.end();
        root.end();

        let spans = processor.spans();
        assert_eq!(spans.len(), 3);
        // 三个 Span 应该有相同的 trace_id
        let trace_id = spans[0].span_context.trace_id();
        for span in &spans {
            assert_eq!(span.span_context.trace_id(), trace_id);
        }
    }

    #[test]
    fn test_traceparent_propagation() {
        let trace_id = TraceId::from_hex("abcdef0123456789abcdef0123456789").unwrap();
        let span_id = SpanId::from_hex("abcdef0123456789").unwrap();
        let ctx = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::new().with_sampled(true),
            false,
        );

        let tp = ctx.to_traceparent();
        assert_eq!(
            tp,
            "00-abcdef0123456789abcdef0123456789-abcdef0123456789-01"
        );

        let parsed = SpanContext::from_traceparent(&tp).unwrap();
        assert_eq!(parsed.trace_id(), trace_id);
        assert_eq!(parsed.span_id(), span_id);
        assert!(parsed.is_sampled());
        assert!(parsed.is_remote());
    }

    #[test]
    fn test_query_with_join_trace() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());

        let span = SqlSpanBuilder::new(
            &provider,
            "SELECT * FROM users u JOIN orders o ON u.id = o.user_id",
        )
        .with_operation("SELECT")
        .with_table("users,orders")
        .with_join_count(1)
        .with_rows_scanned(50000)
        .with_index_used("idx_orders_user_id")
        .with_seq_scan(false)
        .start();
        span.end();

        let spans = processor.spans();
        let span = &spans[0];
        assert_eq!(span.get_int_attribute("db.join_count"), Some(1));
        assert_eq!(span.get_int_attribute("db.rows_scanned"), Some(50000));
        assert_eq!(
            span.get_string_attribute("db.index_used"),
            Some("idx_orders_user_id")
        );
    }

    #[test]
    fn test_multiple_queries_traced() {
        let processor = Arc::new(SimpleSpanProcessor::new());
        let provider = TracerProvider::default().with_simple_processor(processor.clone());

        let queries = vec![
            ("SELECT * FROM users", "SELECT", "users"),
            ("INSERT INTO logs VALUES (...)", "INSERT", "logs"),
            ("UPDATE users SET name = 'x'", "UPDATE", "users"),
            ("DELETE FROM logs WHERE id = 1", "DELETE", "logs"),
        ];

        for (sql, op, table) in queries {
            let span = SqlSpanBuilder::new(&provider, sql)
                .with_operation(op)
                .with_table(table)
                .start();
            span.end();
        }

        let spans = processor.spans();
        assert_eq!(spans.len(), 4);
        let expected_ops = ["SELECT", "INSERT", "UPDATE", "DELETE"];
        for (i, op) in expected_ops.iter().enumerate() {
            assert_eq!(spans[i].get_string_attribute("db.operation"), Some(*op));
        }
    }

    #[test]
    fn test_sql_attributes_constants() {
        assert_eq!(sql_attributes::DB_STATEMENT, "db.statement");
        assert_eq!(sql_attributes::DB_SYSTEM, "db.system");
        assert_eq!(sql_attributes::DB_NAME, "db.name");
        assert_eq!(sql_attributes::DB_USER, "db.user");
        assert_eq!(sql_attributes::DB_OPERATION, "db.operation");
        assert_eq!(sql_attributes::DB_TABLE, "db.sql.table");
        assert_eq!(sql_attributes::DB_ROWS_SCANNED, "db.rows_scanned");
        assert_eq!(sql_attributes::DB_ROWS_RETURNED, "db.rows_returned");
        assert_eq!(sql_attributes::DB_INDEX_USED, "db.index_used");
        assert_eq!(sql_attributes::DB_JOIN_COUNT, "db.join_count");
        assert_eq!(sql_attributes::DB_SEQ_SCAN, "db.seq_scan");
    }
}
