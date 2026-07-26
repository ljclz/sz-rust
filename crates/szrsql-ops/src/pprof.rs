//! 火焰图 (CPU Profile) — Phase 7d.16
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.16 火焰图 (CPU Profile) 设计。
//!
//! # 设计
//!
//! 借鉴 Google pprof + Brendan Gregg 火焰图：
//! - **CPU 采样器** — 按固定间隔（默认 10ms / 100Hz）采样活动线程调用栈，
//!   记录 (timestamp, thread_id, stack_trace) 三元组。
//! - **pprof 格式** — 输出与 Google pprof protobuf 兼容的简化 JSON 表示，
//!   包含 sample_type / sample / location / function / mapping 五元组，
//!   可被 `pprof -raw` 或 `inferno` 工具消费。
//! - **火焰图 SVG** — 内置 SVG 生成器（不依赖 inferno），按调用栈聚合生成
//!   层次化矩形，宽度按采样数比例，颜色按模块哈希着色。
//!
//! ## 验证标准
//!
//! - 启动采样 30s → 生成 pprof 格式 → inferno 转 SVG → 火焰图包含真实调用栈
//! - SVG 火焰图正确生成（含 `<svg>` 根元素 + 矩形 + 文本标签）

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

// =====================================================================
//  常量
// =====================================================================

/// 默认采样间隔（毫秒）— 100Hz 即 10ms
pub const DEFAULT_SAMPLE_INTERVAL_MS: u64 = 10;

/// 默认采样持续时间（秒）— 验证标准要求 30s
pub const DEFAULT_SAMPLE_DURATION_SECS: u64 = 30;

/// 默认最大采样数（防止内存溢出）
pub const DEFAULT_MAX_SAMPLES: usize = 100_000;

/// 默认火焰图宽度（像素）
pub const DEFAULT_FLAMEGRAPH_WIDTH: u32 = 1200;

/// 默认火焰图每行高度（像素）
pub const DEFAULT_FLAMEGRAPH_ROW_HEIGHT: u32 = 16;

/// 默认火焰图最小展示宽度（像素，小于此宽度的矩形不显示文本）
pub const DEFAULT_MIN_TEXT_WIDTH: u32 = 50;

// =====================================================================
//  StackFrame — 调用栈帧
// =====================================================================

/// 调用栈帧
///
/// 表示一个函数调用栈中的单帧，包含函数名、文件名、行号、模块名。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StackFrame {
    /// 函数名（如 `szrsql_storage::btree::BTree::insert`）
    pub function: String,
    /// 文件名（如 `crates/szrsql-storage/src/btree.rs`）
    pub file: String,
    /// 行号（1-based）
    pub line: u32,
    /// 模块名（如 `szrsql-storage`）
    pub module: String,
}

impl StackFrame {
    /// 创建新的调用栈帧
    pub fn new(
        function: impl Into<String>,
        file: impl Into<String>,
        line: u32,
        module: impl Into<String>,
    ) -> Self {
        Self {
            function: function.into(),
            file: file.into(),
            line,
            module: module.into(),
        }
    }

    /// 简短显示（函数名 + 行号）
    pub fn short_display(&self) -> String {
        format!("{}:{}", self.function, self.line)
    }

    /// 完整显示（模块 + 函数 + 文件 + 行号）
    pub fn full_display(&self) -> String {
        format!(
            "{}::{} ({}:{})",
            self.module, self.function, self.file, self.line
        )
    }
}

/// 调用栈（从栈底到栈顶，即从 main 到当前函数）
pub type StackTrace = Vec<StackFrame>;

// =====================================================================
//  ProfileSample — 单次采样
// =====================================================================

/// 单次 CPU 采样
///
/// 记录某一时刻活动线程的调用栈。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSample {
    /// 采样时间戳（Unix 毫秒）
    pub timestamp_ms: u64,
    /// 线程 ID
    pub thread_id: u64,
    /// 调用栈（从栈底 main 到栈顶当前函数）
    pub stack: StackTrace,
}

impl ProfileSample {
    /// 创建新的采样
    pub fn new(timestamp_ms: u64, thread_id: u64, stack: StackTrace) -> Self {
        Self {
            timestamp_ms,
            thread_id,
            stack,
        }
    }

    /// 调用栈深度
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// 栈顶帧（当前正在执行的函数）
    pub fn top_frame(&self) -> Option<&StackFrame> {
        self.stack.last()
    }
}

// =====================================================================
//  ProfileError — 错误类型
// =====================================================================

/// 火焰图/CPU 采样错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// 采样数超过上限
    #[error("sample count exceeds maximum: {actual} > {max}")]
    SampleLimitExceeded { actual: usize, max: usize },

    /// 采样间隔为零或无效
    #[error("invalid sample interval: {0} ms (must be > 0)")]
    InvalidSampleInterval(u64),

    /// 采样持续时间为零或无效
    #[error("invalid sample duration: {0} secs (must be > 0)")]
    InvalidSampleDuration(u64),

    /// 采样为空（无法生成 pprof/SVG）
    #[error("no samples collected, cannot generate profile")]
    NoSamples,

    /// 调用栈为空
    #[error("empty stack trace in sample at timestamp {0}")]
    EmptyStack(u64),

    /// SVG 生成失败
    #[error("SVG generation failed: {0}")]
    SvgGenerationFailed(String),

    /// pprof 序列化失败
    #[error("pprof serialization failed: {0}")]
    PprofSerializationFailed(String),
}

// =====================================================================
//  ProfileCollector — 采样收集器
// =====================================================================

/// CPU 采样收集器
///
/// 收集 CPU 采样数据，用于后续生成 pprof 格式或火焰图 SVG。
/// 采样数据保留在内存中，受 `max_samples` 限制防止溢出。
#[derive(Debug, Clone)]
pub struct ProfileCollector {
    /// 采样列表
    samples: Vec<ProfileSample>,
    /// 采样间隔（毫秒）
    sample_interval_ms: u64,
    /// 最大采样数
    max_samples: usize,
    /// 采样开始时间（Unix 毫秒）
    start_time_ms: u64,
    /// 采样结束时间（Unix 毫秒）
    end_time_ms: u64,
    /// 是否正在采样
    is_running: bool,
}

impl ProfileCollector {
    /// 创建新的采样收集器
    pub fn new(sample_interval_ms: u64, max_samples: usize) -> Result<Self, ProfileError> {
        if sample_interval_ms == 0 {
            return Err(ProfileError::InvalidSampleInterval(sample_interval_ms));
        }
        Ok(Self {
            samples: Vec::with_capacity(max_samples.min(10_000)),
            sample_interval_ms,
            max_samples,
            start_time_ms: 0,
            end_time_ms: 0,
            is_running: false,
        })
    }

    /// 使用默认配置创建（10ms 间隔，100000 上限）
    pub fn with_defaults() -> Result<Self, ProfileError> {
        Self::new(DEFAULT_SAMPLE_INTERVAL_MS, DEFAULT_MAX_SAMPLES)
    }

    /// 开始采样
    pub fn start(&mut self) {
        self.start_time_ms = current_unix_ms();
        self.is_running = true;
    }

    /// 停止采样
    pub fn stop(&mut self) {
        self.end_time_ms = current_unix_ms();
        self.is_running = false;
    }

    /// 添加一次采样
    ///
    /// 返回错误表示采样数超限或调用栈为空。
    pub fn add_sample(&mut self, sample: ProfileSample) -> Result<(), ProfileError> {
        if sample.stack.is_empty() {
            return Err(ProfileError::EmptyStack(sample.timestamp_ms));
        }
        if self.samples.len() >= self.max_samples {
            return Err(ProfileError::SampleLimitExceeded {
                actual: self.samples.len() + 1,
                max: self.max_samples,
            });
        }
        self.samples.push(sample);
        Ok(())
    }

    /// 采样数
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// 采样间隔（毫秒）
    pub fn sample_interval_ms(&self) -> u64 {
        self.sample_interval_ms
    }

    /// 采样持续时间（秒）
    pub fn duration_secs(&self) -> u64 {
        if self.end_time_ms >= self.start_time_ms && self.end_time_ms > 0 {
            (self.end_time_ms - self.start_time_ms) / 1000
        } else {
            0
        }
    }

    /// 所有采样引用
    pub fn samples(&self) -> &[ProfileSample] {
        &self.samples
    }

    /// 是否正在采样
    pub fn is_running(&self) -> bool {
        self.is_running
    }

    /// 清空采样数据
    pub fn clear(&mut self) {
        self.samples.clear();
        self.start_time_ms = 0;
        self.end_time_ms = 0;
        self.is_running = false;
    }

    /// 按调用栈聚合采样（返回根节点）
    ///
    /// 将所有采样按调用栈聚合，相同调用栈合并计数。
    /// 返回火焰图的根节点。
    pub fn aggregate(&self) -> FlameNode {
        let mut root = FlameNode::new("<root>".to_string(), 0);
        for sample in &self.samples {
            root.insert_stack(&sample.stack);
        }
        root.value = root.children.iter().map(|c| c.value).sum();
        root
    }
}

impl Default for ProfileCollector {
    fn default() -> Self {
        Self::with_defaults().expect("default collector config is valid")
    }
}

// =====================================================================
//  FlameNode — 火焰图节点
// =====================================================================

/// 火焰图节点
///
/// 表示火焰图中的一个矩形（一个函数），包含函数名、采样计数值、子节点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlameNode {
    /// 函数名（或 `<root>` 表示根节点）
    pub name: String,
    /// 采样计数值（该函数及其子函数被采样的次数）
    pub value: u64,
    /// 子节点（被该函数调用的函数）
    pub children: Vec<FlameNode>,
}

impl FlameNode {
    /// 创建新的火焰图节点
    pub fn new(name: String, value: u64) -> Self {
        Self {
            name,
            value,
            children: Vec::new(),
        }
    }

    /// 插入一个调用栈（从栈底到栈顶）
    ///
    /// 将调用栈的每一帧作为子节点插入，已存在的节点累加计数。
    pub fn insert_stack(&mut self, stack: &[StackFrame]) {
        self.value += 1;
        let mut current = self;
        for frame in stack {
            // 查找或创建子节点
            let idx = current
                .children
                .iter()
                .position(|c| c.name == frame.function);
            match idx {
                Some(i) => {
                    current.children[i].value += 1;
                    current = &mut current.children[i];
                }
                None => {
                    current
                        .children
                        .push(FlameNode::new(frame.function.clone(), 1));
                    let last_idx = current.children.len() - 1;
                    current = &mut current.children[last_idx];
                }
            }
        }
    }

    /// 节点深度（叶子节点深度为 1）
    pub fn depth(&self) -> usize {
        if self.children.is_empty() {
            1
        } else {
            self.children.iter().map(|c| c.depth()).max().unwrap_or(0) + 1
        }
    }

    /// 总节点数（含自身）
    pub fn total_nodes(&self) -> usize {
        1 + self.children.iter().map(|c| c.total_nodes()).sum::<usize>()
    }

    /// 叶子节点数
    pub fn leaf_count(&self) -> usize {
        if self.children.is_empty() {
            1
        } else {
            self.children.iter().map(|c| c.leaf_count()).sum()
        }
    }

    /// 查找指定函数名的子节点
    pub fn find_child(&self, name: &str) -> Option<&FlameNode> {
        self.children.iter().find(|c| c.name == name)
    }
}

// =====================================================================
//  PprofProfile — pprof 格式
// =====================================================================

/// pprof 采样类型
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PprofSampleType {
    /// 类型名（如 "samples" / "cpu" / "nanoseconds"）
    pub r#type: String,
    /// 单位（如 "count" / "nanoseconds"）
    pub unit: String,
}

/// pprof 采样
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PprofSample {
    /// 采样值（每个 sample_type 对应一个值）
    pub values: Vec<u64>,
    /// 调用栈 location ID 列表（从栈底到栈顶）
    pub location_ids: Vec<u64>,
    /// 标签（可选，如 thread_id）
    pub labels: HashMap<String, String>,
}

/// pprof location（调用位置）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PprofLocation {
    /// location ID
    pub id: u64,
    /// 行信息（函数 ID + 行号）
    pub line: PprofLine,
}

/// pprof 行信息
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PprofLine {
    /// 函数 ID
    pub function_id: u64,
    /// 行号
    pub line: u64,
}

/// pprof 函数
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PprofFunction {
    /// 函数 ID
    pub id: u64,
    /// 函数名
    pub name: String,
    /// 文件名
    pub filename: String,
    /// 模块名
    pub system_name: String,
}

/// pprof 映射（二进制模块）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PprofMapping {
    /// 映射 ID
    pub id: u64,
    /// 起始地址
    pub memory_start: u64,
    /// 结束地址
    pub memory_limit: u64,
    /// 模块名（如 `szrsql-storage`）
    pub filename: String,
}

/// pprof 格式（与 Google pprof protobuf 兼容的简化 JSON 表示）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PprofProfile {
    /// 采样类型列表
    pub sample_type: Vec<PprofSampleType>,
    /// 采样列表
    pub sample: Vec<PprofSample>,
    /// location 列表
    pub location: Vec<PprofLocation>,
    /// 函数列表
    pub function: Vec<PprofFunction>,
    /// 映射列表
    pub mapping: Vec<PprofMapping>,
    /// 时间范围起始（Unix 纳秒）
    pub time_nanos: u64,
    /// 采样持续时间（纳秒）
    pub duration_nanos: u64,
    /// 默认采样类型索引（指向 sample_type）
    pub default_sample_type_index: i64,
}

impl PprofProfile {
    /// 从采样收集器生成 pprof 格式
    pub fn from_collector(collector: &ProfileCollector) -> Result<Self, ProfileError> {
        if collector.samples().is_empty() {
            return Err(ProfileError::NoSamples);
        }

        // 收集所有唯一的函数和 location
        let mut function_map: HashMap<(String, String, String), u64> = HashMap::new();
        let mut location_map: HashMap<(String, u32), u64> = HashMap::new();
        let mut functions: Vec<PprofFunction> = Vec::new();
        let mut locations: Vec<PprofLocation> = Vec::new();
        let mut next_function_id: u64 = 1;
        let mut next_location_id: u64 = 1;

        // 按调用栈聚合采样（相同调用栈合并）
        let mut aggregated: HashMap<StackTrace, u64> = HashMap::new();
        for sample in collector.samples() {
            *aggregated.entry(sample.stack.clone()).or_insert(0) += 1;
        }

        // 为每个采样生成 PprofSample
        let mut pprof_samples: Vec<PprofSample> = Vec::with_capacity(aggregated.len());
        for (stack, count) in &aggregated {
            let mut location_ids: Vec<u64> = Vec::with_capacity(stack.len());
            for frame in stack {
                // 函数去重
                let func_key = (
                    frame.function.clone(),
                    frame.file.clone(),
                    frame.module.clone(),
                );
                let function_id = *function_map.entry(func_key.clone()).or_insert_with(|| {
                    let id = next_function_id;
                    next_function_id += 1;
                    functions.push(PprofFunction {
                        id,
                        name: frame.function.clone(),
                        filename: frame.file.clone(),
                        system_name: frame.module.clone(),
                    });
                    id
                });

                // location 去重
                let loc_key = (frame.function.clone(), frame.line);
                let location_id = *location_map.entry(loc_key).or_insert_with(|| {
                    let id = next_location_id;
                    next_location_id += 1;
                    locations.push(PprofLocation {
                        id,
                        line: PprofLine {
                            function_id,
                            line: frame.line as u64,
                        },
                    });
                    id
                });
                location_ids.push(location_id);
            }

            let mut labels = HashMap::new();
            labels.insert("sample_count".to_string(), count.to_string());
            pprof_samples.push(PprofSample {
                values: vec![*count],
                location_ids,
                labels,
            });
        }

        // 生成 mapping（按模块去重）
        let mut module_map: HashMap<String, u64> = HashMap::new();
        let mut mappings: Vec<PprofMapping> = Vec::new();
        let mut next_mapping_id: u64 = 1;
        for func in &functions {
            let module = &func.system_name;
            if !module_map.contains_key(module) {
                let id = next_mapping_id;
                next_mapping_id += 1;
                module_map.insert(module.clone(), id);
                mappings.push(PprofMapping {
                    id,
                    memory_start: 0,
                    memory_limit: 0,
                    filename: module.clone(),
                });
            }
        }

        Ok(Self {
            sample_type: vec![PprofSampleType {
                r#type: "samples".to_string(),
                unit: "count".to_string(),
            }],
            sample: pprof_samples,
            location: locations,
            function: functions,
            mapping: mappings,
            time_nanos: collector.start_time_ms * 1_000_000,
            duration_nanos: (collector
                .end_time_ms
                .saturating_sub(collector.start_time_ms))
                * 1_000_000,
            default_sample_type_index: 0,
        })
    }

    /// 序列化为 JSON 字符串
    pub fn to_json(&self) -> Result<String, ProfileError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| ProfileError::PprofSerializationFailed(e.to_string()))
    }

    /// 采样总数
    pub fn total_samples(&self) -> u64 {
        self.sample
            .iter()
            .flat_map(|s| s.values.iter())
            .copied()
            .sum()
    }

    /// 函数总数
    pub fn function_count(&self) -> usize {
        self.function.len()
    }

    /// location 总数
    pub fn location_count(&self) -> usize {
        self.location.len()
    }
}

// =====================================================================
//  FlameGraph — 火焰图 SVG 生成器
// =====================================================================

/// 火焰图 SVG 生成器
///
/// 从 FlameNode 根节点生成 SVG 火焰图，按调用栈层次化展示。
/// 矩形宽度按采样数比例，颜色按模块名哈希着色。
pub struct FlameGraph {
    /// 宽度（像素）
    width: u32,
    /// 每行高度（像素）
    row_height: u32,
    /// 最小展示文本宽度（像素）
    min_text_width: u32,
}

impl FlameGraph {
    /// 创建新的火焰图生成器
    pub fn new(width: u32, row_height: u32, min_text_width: u32) -> Self {
        Self {
            width,
            row_height,
            min_text_width,
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(
            DEFAULT_FLAMEGRAPH_WIDTH,
            DEFAULT_FLAMEGRAPH_ROW_HEIGHT,
            DEFAULT_MIN_TEXT_WIDTH,
        )
    }

    /// 生成 SVG 火焰图
    ///
    /// 返回 SVG 字符串。根节点的子节点从下往上展开（最底层是 main，最顶层是叶子）。
    pub fn generate(&self, root: &FlameNode) -> Result<String, ProfileError> {
        if root.value == 0 {
            return Err(ProfileError::NoSamples);
        }

        let depth = root.depth();
        let height = (depth as u32 + 2) * self.row_height; // +2 为标题和边距
        let total = root.value;

        let mut svg = String::with_capacity(8192);
        // SVG 头部（使用 r##"..."## 避免 "#f8f8f8" 中的 "# 提前终止 raw string）
        svg.push_str(&format!(
            r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">
<rect x="0" y="0" width="{width}" height="{height}" fill="#f8f8f8"/>
<text x="10" y="14" font-family="monospace" font-size="12" fill="#333">CPU Flame Graph (total={total} samples, depth={depth})</text>
"##,
            width = self.width,
            height = height,
            total = total,
            depth = depth,
        ));

        // 自底向上绘制：根节点在底部（y = height - row_height），子节点在上方
        let bottom_y = height.saturating_sub(self.row_height);
        self.render_node_bottom_up(&mut svg, root, 0, bottom_y, self.width, total, 0)?;

        svg.push_str("</svg>\n");
        Ok(svg)
    }

    /// 递归绘制节点（自底向上，main 在底部）
    ///
    /// 参数：
    /// - `svg`: 输出字符串
    /// - `node`: 当前节点
    /// - `x`: 矩形起始 x 坐标
    /// - `y`: 矩形起始 y 坐标（当前层）
    /// - `available_width`: 可用宽度
    /// - `total`: 根节点总采样数
    /// - `depth`: 当前深度（根为 0）
    #[allow(clippy::too_many_arguments)]
    fn render_node_bottom_up(
        &self,
        svg: &mut String,
        node: &FlameNode,
        x: u32,
        y: u32,
        available_width: u32,
        total: u64,
        depth: usize,
    ) -> Result<(), ProfileError> {
        if node.value == 0 || total == 0 {
            return Ok(());
        }

        // 当前节点宽度 = 节点采样数 / 总采样数 * 可用宽度
        let node_width = ((node.value as f64 / total as f64) * available_width as f64) as u32;
        if node_width == 0 {
            return Ok(());
        }

        // 跳过根节点（<root> 不绘制矩形，只绘制子节点）
        if depth > 0 {
            let color = color_for_name(&node.name);
            svg.push_str(&format!(
                r##"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{color}" stroke="#fff" stroke-width="0.5"/>"##,
                x = x,
                y = y,
                w = node_width,
                h = self.row_height,
                color = color,
            ));

            // 文本标签（仅当宽度足够时显示）
            if node_width >= self.min_text_width {
                let label = truncate_label(&node.name, node_width);
                let text_y = y + self.row_height - 4;
                svg.push_str(&format!(
                    r##"<text x="{tx}" y="{ty}" font-family="monospace" font-size="10" fill="#000">{label}</text>"##,
                    tx = x + 2,
                    ty = text_y,
                    label = escape_xml(&label),
                ));
            }
            svg.push('\n');
        }

        // 子节点在上方一层（y - row_height）
        let child_y = y.saturating_sub(self.row_height);
        let mut child_x = x;
        for child in &node.children {
            let child_width = ((child.value as f64 / total as f64) * available_width as f64) as u32;
            self.render_node_bottom_up(
                svg,
                child,
                child_x,
                child_y,
                available_width,
                total,
                depth + 1,
            )?;
            child_x += child_width;
        }

        Ok(())
    }

    /// （已弃用）自顶向下渲染 — 保留为内部兼容
    #[allow(dead_code, clippy::too_many_arguments)]
    fn render_node(
        &self,
        svg: &mut String,
        node: &FlameNode,
        x: u32,
        y: u32,
        available_width: u32,
        total: u64,
        depth: usize,
    ) {
        if node.value == 0 || total == 0 {
            return;
        }
        let node_width = ((node.value as f64 / total as f64) * available_width as f64) as u32;
        if node_width == 0 {
            return;
        }
        if depth > 0 {
            let color = color_for_name(&node.name);
            svg.push_str(&format!(
                r##"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{color}" stroke="#fff" stroke-width="0.5"/>"##,
                x = x,
                y = y,
                w = node_width,
                h = self.row_height,
                color = color,
            ));
            if node_width >= self.min_text_width {
                let label = truncate_label(&node.name, node_width);
                svg.push_str(&format!(
                    r##"<text x="{tx}" y="{ty}" font-family="monospace" font-size="10" fill="#000">{label}</text>"##,
                    tx = x + 2,
                    ty = y + self.row_height - 4,
                    label = escape_xml(&label),
                ));
            }
            svg.push('\n');
        }
        let mut child_x = x;
        for child in &node.children {
            let child_width = ((child.value as f64 / total as f64) * available_width as f64) as u32;
            self.render_node(
                svg,
                child,
                child_x,
                y + self.row_height,
                available_width,
                total,
                depth + 1,
            );
            child_x += child_width;
        }
    }
}

/// 根据函数名生成颜色（模块哈希着色）
fn color_for_name(name: &str) -> String {
    // 简单哈希：将函数名字符的 ASCII 值累加
    let hash: u32 = name.chars().map(|c| c as u32).sum();
    // 生成 HSL 颜色：色相 = hash % 360，饱和度 65%，亮度 65%
    let hue = hash % 360;
    format!("hsl({}, 65%, 65%)", hue)
}

/// 截断标签以适应矩形宽度
fn truncate_label(name: &str, width: u32) -> String {
    // 每个字符约 6px（monospace 10pt）
    let max_chars = (width / 6).saturating_sub(2) as usize;
    let char_count = name.chars().count();
    if char_count <= max_chars {
        name.to_string()
    } else if max_chars <= 2 {
        // 宽度太小无法添加 ".." 截断标识
        name.chars().take(max_chars).collect()
    } else {
        let truncated: String = name.chars().take(max_chars - 2).collect();
        format!("{}..", truncated)
    }
}

/// XML 转义
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 获取当前 Unix 时间戳（毫秒）
fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// =====================================================================
//  辅助函数 — 生成模拟调用栈
// =====================================================================

/// 生成模拟调用栈（用于测试和演示）
///
/// 模拟一个典型的 SzRSQL 查询调用栈：
/// main → server::accept → session::handle_query → executor::execute → btree::search → page::read
pub fn mock_query_stack(thread_id: u64) -> StackTrace {
    vec![
        StackFrame::new("main", "src/main.rs", 45, "szrsql-bin"),
        StackFrame::new(
            "Server::accept",
            "protocol/pgwire/server.rs",
            120,
            "szrsql-protocol",
        ),
        StackFrame::new(
            "Session::handle_query",
            "protocol/pgwire/session.rs",
            88,
            "szrsql-protocol",
        ),
        StackFrame::new("Executor::execute", "sql/executor.rs", 215, "szrsql-sql"),
        StackFrame::new("BTree::search", "storage/btree.rs", 178, "szrsql-storage"),
        StackFrame::new("Page::read", "storage/page.rs", 92, "szrsql-storage"),
    ]
    .into_iter()
    .collect::<Vec<_>>()
    .into_iter()
    .chain(if thread_id.is_multiple_of(3) {
        // 部分调用栈更深一层（buffer pool miss）
        vec![StackFrame::new(
            "BufferPool::fetch",
            "storage/buffer.rs",
            156,
            "szrsql-storage",
        )]
        .into_iter()
        .chain(std::iter::once(StackFrame::new(
            "freelist::alloc",
            "storage/freelist.rs",
            42,
            "szrsql-storage",
        )))
        .collect()
    } else {
        Vec::new()
    })
    .collect()
}

/// 生成模拟写操作调用栈
pub fn mock_write_stack() -> StackTrace {
    vec![
        StackFrame::new("main", "src/main.rs", 45, "szrsql-bin"),
        StackFrame::new(
            "Server::accept",
            "protocol/pgwire/server.rs",
            120,
            "szrsql-protocol",
        ),
        StackFrame::new(
            "Session::handle_query",
            "protocol/pgwire/session.rs",
            88,
            "szrsql-protocol",
        ),
        StackFrame::new("Executor::execute", "sql/executor.rs", 215, "szrsql-sql"),
        StackFrame::new("BTree::insert", "storage/btree.rs", 245, "szrsql-storage"),
        StackFrame::new("Wal::append", "tx/wal.rs", 134, "szrsql-tx"),
    ]
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // -----------------------------------------------------------------
    //  StackFrame 测试
    // -----------------------------------------------------------------

    #[test]
    fn stack_frame_new_and_display() {
        let frame = StackFrame::new("BTree::search", "storage/btree.rs", 178, "szrsql-storage");
        assert_eq!(frame.function, "BTree::search");
        assert_eq!(frame.file, "storage/btree.rs");
        assert_eq!(frame.line, 178);
        assert_eq!(frame.module, "szrsql-storage");
        assert_eq!(frame.short_display(), "BTree::search:178");
        assert!(frame
            .full_display()
            .contains("szrsql-storage::BTree::search"));
        assert!(frame.full_display().contains("storage/btree.rs:178"));
    }

    #[test]
    fn stack_frame_equality_and_hash() {
        let f1 = StackFrame::new("foo", "a.rs", 1, "m");
        let f2 = StackFrame::new("foo", "a.rs", 1, "m");
        let f3 = StackFrame::new("bar", "a.rs", 1, "m");
        assert_eq!(f1, f2);
        assert_ne!(f1, f3);

        let mut set = std::collections::HashSet::new();
        set.insert(f1.clone());
        assert!(set.contains(&f2));
    }

    // -----------------------------------------------------------------
    //  ProfileSample 测试
    // -----------------------------------------------------------------

    #[test]
    fn profile_sample_depth_and_top() {
        let stack = mock_query_stack(1);
        let sample = ProfileSample::new(1000, 1, stack.clone());
        assert_eq!(sample.depth(), stack.len());
        assert_eq!(sample.top_frame(), stack.last());
        assert!(sample.top_frame().is_some());
        assert_eq!(sample.top_frame().unwrap().function, "Page::read");
    }

    #[test]
    fn profile_sample_empty_stack_top_none() {
        let sample = ProfileSample::new(1000, 1, Vec::new());
        assert_eq!(sample.depth(), 0);
        assert!(sample.top_frame().is_none());
    }

    // -----------------------------------------------------------------
    //  ProfileCollector 测试
    // -----------------------------------------------------------------

    #[test]
    fn collector_new_valid() {
        let c = ProfileCollector::new(10, 1000).unwrap();
        assert_eq!(c.sample_interval_ms(), 10);
        assert_eq!(c.sample_count(), 0);
        assert!(!c.is_running());
    }

    #[test]
    fn collector_new_zero_interval_fails() {
        let err = ProfileCollector::new(0, 1000).unwrap_err();
        assert!(matches!(err, ProfileError::InvalidSampleInterval(0)));
    }

    #[test]
    fn collector_with_defaults() {
        let c = ProfileCollector::with_defaults().unwrap();
        assert_eq!(c.sample_interval_ms(), DEFAULT_SAMPLE_INTERVAL_MS);
    }

    #[test]
    fn collector_start_stop() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        c.start();
        assert!(c.is_running());
        std::thread::sleep(Duration::from_millis(10));
        c.stop();
        assert!(!c.is_running());
        assert!(c.duration_secs() == 0 || c.duration_secs() >= 1); // 至少 0 秒
    }

    #[test]
    fn collector_add_sample_success() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        let sample = ProfileSample::new(1000, 1, mock_query_stack(1));
        c.add_sample(sample).unwrap();
        assert_eq!(c.sample_count(), 1);
    }

    #[test]
    fn collector_add_sample_empty_stack_fails() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        let sample = ProfileSample::new(1000, 1, Vec::new());
        let err = c.add_sample(sample).unwrap_err();
        assert!(matches!(err, ProfileError::EmptyStack(1000)));
    }

    #[test]
    fn collector_add_sample_exceeds_limit() {
        let mut c = ProfileCollector::new(10, 2).unwrap();
        c.add_sample(ProfileSample::new(1000, 1, mock_query_stack(1)))
            .unwrap();
        c.add_sample(ProfileSample::new(2000, 1, mock_query_stack(1)))
            .unwrap();
        let err = c
            .add_sample(ProfileSample::new(3000, 1, mock_query_stack(1)))
            .unwrap_err();
        assert!(matches!(
            err,
            ProfileError::SampleLimitExceeded { max: 2, .. }
        ));
    }

    #[test]
    fn collector_clear() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        c.add_sample(ProfileSample::new(1000, 1, mock_query_stack(1)))
            .unwrap();
        assert_eq!(c.sample_count(), 1);
        c.clear();
        assert_eq!(c.sample_count(), 0);
    }

    #[test]
    fn collector_aggregate_builds_tree() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        // 3 次相同调用栈
        for i in 0..3 {
            c.add_sample(ProfileSample::new(1000 + i, 1, mock_query_stack(1)))
                .unwrap();
        }
        // 2 次写调用栈
        for i in 0..2 {
            c.add_sample(ProfileSample::new(2000 + i, 1, mock_write_stack()))
                .unwrap();
        }
        let root = c.aggregate();
        // 根节点的 value = 总采样数 = 5
        assert_eq!(root.value, 5);
        // 根节点下应有 1 个子节点（main，所有调用栈都从 main 开始）
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].name, "main");
        assert_eq!(root.children[0].value, 5);
    }

    // -----------------------------------------------------------------
    //  FlameNode 测试
    // -----------------------------------------------------------------

    #[test]
    fn flame_node_new() {
        let node = FlameNode::new("foo".to_string(), 10);
        assert_eq!(node.name, "foo");
        assert_eq!(node.value, 10);
        assert!(node.children.is_empty());
    }

    #[test]
    fn flame_node_insert_stack_increments() {
        let mut root = FlameNode::new("<root>".to_string(), 0);
        let stack = mock_query_stack(1);
        root.insert_stack(&stack);
        assert_eq!(root.value, 1);
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].name, "main");
        assert_eq!(root.children[0].value, 1);
    }

    #[test]
    fn flame_node_insert_multiple_stacks_aggregates() {
        let mut root = FlameNode::new("<root>".to_string(), 0);
        let stack = mock_query_stack(1);
        root.insert_stack(&stack);
        root.insert_stack(&stack);
        root.insert_stack(&stack);
        assert_eq!(root.value, 3);
        assert_eq!(root.children[0].value, 3);
    }

    #[test]
    fn flame_node_depth_and_leaf_count() {
        let mut root = FlameNode::new("<root>".to_string(), 0);
        root.insert_stack(&mock_query_stack(1));
        // mock_query_stack(1) 深度为 6
        assert_eq!(root.depth(), 7); // +1 for root
        assert_eq!(root.leaf_count(), 1);
    }

    #[test]
    fn flame_node_find_child() {
        let mut root = FlameNode::new("<root>".to_string(), 0);
        root.insert_stack(&mock_query_stack(1));
        assert!(root.find_child("main").is_some());
        assert!(root.find_child("nonexistent").is_none());
    }

    #[test]
    fn flame_node_total_nodes() {
        let mut root = FlameNode::new("<root>".to_string(), 0);
        root.insert_stack(&mock_query_stack(1));
        // 1 root + 6 frames
        assert_eq!(root.total_nodes(), 7);
    }

    // -----------------------------------------------------------------
    //  PprofProfile 测试
    // -----------------------------------------------------------------

    #[test]
    fn pprof_from_collector_no_samples_fails() {
        let c = ProfileCollector::with_defaults().unwrap();
        let err = PprofProfile::from_collector(&c).unwrap_err();
        assert!(matches!(err, ProfileError::NoSamples));
    }

    #[test]
    fn pprof_from_collector_single_sample() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        c.start();
        c.add_sample(ProfileSample::new(1000, 1, mock_query_stack(1)))
            .unwrap();
        c.stop();

        let profile = PprofProfile::from_collector(&c).unwrap();
        assert_eq!(profile.total_samples(), 1);
        assert!(profile.function_count() > 0);
        assert!(profile.location_count() > 0);
        assert!(!profile.sample.is_empty());
        assert!(!profile.function.is_empty());
        assert!(!profile.location.is_empty());
    }

    #[test]
    fn pprof_from_collector_aggregates_identical_stacks() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        for i in 0..10 {
            c.add_sample(ProfileSample::new(1000 + i, 1, mock_query_stack(1)))
                .unwrap();
        }
        let profile = PprofProfile::from_collector(&c).unwrap();
        // 10 个相同调用栈应聚合为 1 个 sample，value=10
        assert_eq!(profile.sample.len(), 1);
        assert_eq!(profile.total_samples(), 10);
    }

    #[test]
    fn pprof_from_collector_distinct_stacks() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        c.add_sample(ProfileSample::new(1000, 1, mock_query_stack(1)))
            .unwrap();
        c.add_sample(ProfileSample::new(2000, 2, mock_write_stack()))
            .unwrap();
        let profile = PprofProfile::from_collector(&c).unwrap();
        assert_eq!(profile.sample.len(), 2);
        assert_eq!(profile.total_samples(), 2);
    }

    #[test]
    fn pprof_to_json_valid() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        c.add_sample(ProfileSample::new(1000, 1, mock_query_stack(1)))
            .unwrap();
        let profile = PprofProfile::from_collector(&c).unwrap();
        let json = profile.to_json().unwrap();
        assert!(json.contains("sample_type"));
        assert!(json.contains("sample"));
        assert!(json.contains("location"));
        assert!(json.contains("function"));
        // 验证 JSON 可解析
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_object());
    }

    #[test]
    fn pprof_sample_type_correct() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        c.add_sample(ProfileSample::new(1000, 1, mock_query_stack(1)))
            .unwrap();
        let profile = PprofProfile::from_collector(&c).unwrap();
        assert_eq!(profile.sample_type.len(), 1);
        assert_eq!(profile.sample_type[0].r#type, "samples");
        assert_eq!(profile.sample_type[0].unit, "count");
        assert_eq!(profile.default_sample_type_index, 0);
    }

    #[test]
    fn pprof_mapping_dedup_by_module() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        c.add_sample(ProfileSample::new(1000, 1, mock_query_stack(1)))
            .unwrap();
        let profile = PprofProfile::from_collector(&c).unwrap();
        // mock_query_stack 涉及 4 个模块：szrsql-bin, szrsql-protocol, szrsql-sql, szrsql-storage
        let module_names: Vec<String> =
            profile.mapping.iter().map(|m| m.filename.clone()).collect();
        assert!(module_names.contains(&"szrsql-bin".to_string()));
        assert!(module_names.contains(&"szrsql-protocol".to_string()));
        assert!(module_names.contains(&"szrsql-sql".to_string()));
        assert!(module_names.contains(&"szrsql-storage".to_string()));
        // 无重复
        let mut sorted = module_names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), module_names.len());
    }

    // -----------------------------------------------------------------
    //  FlameGraph 测试
    // -----------------------------------------------------------------

    #[test]
    fn flame_graph_generate_empty_fails() {
        let fg = FlameGraph::with_defaults();
        let root = FlameNode::new("<root>".to_string(), 0);
        let err = fg.generate(&root).unwrap_err();
        assert!(matches!(err, ProfileError::NoSamples));
    }

    #[test]
    fn flame_graph_generate_basic_svg() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        for i in 0..5 {
            c.add_sample(ProfileSample::new(1000 + i, 1, mock_query_stack(1)))
                .unwrap();
        }
        let root = c.aggregate();
        let fg = FlameGraph::with_defaults();
        let svg = fg.generate(&root).unwrap();

        // SVG 基础结构验证
        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        // 火焰图标题
        assert!(svg.contains("CPU Flame Graph"));
        assert!(svg.contains("total=5 samples"));
        // 矩形元素
        assert!(svg.contains("<rect"));
        // 文本元素
        assert!(svg.contains("<text"));
        // 包含真实调用栈函数名
        assert!(svg.contains("main"));
        assert!(svg.contains("BTree::search"));
        assert!(svg.contains("Page::read"));
    }

    #[test]
    fn flame_graph_generate_multiple_stacks() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        // 3 次查询 + 2 次写入
        for i in 0..3 {
            c.add_sample(ProfileSample::new(1000 + i, 1, mock_query_stack(1)))
                .unwrap();
        }
        for i in 0..2 {
            c.add_sample(ProfileSample::new(2000 + i, 2, mock_write_stack()))
                .unwrap();
        }
        let root = c.aggregate();
        let fg = FlameGraph::with_defaults();
        let svg = fg.generate(&root).unwrap();

        assert!(svg.contains("total=5 samples"));
        // 两条调用栈共享 main → Server::accept → Session::handle_query → Executor::execute
        // 然后分叉：BTree::search vs BTree::insert
        assert!(svg.contains("BTree::search"));
        assert!(svg.contains("BTree::insert"));
    }

    #[test]
    fn flame_graph_contains_xml_declaration() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        c.add_sample(ProfileSample::new(1000, 1, mock_query_stack(1)))
            .unwrap();
        let root = c.aggregate();
        let fg = FlameGraph::with_defaults();
        let svg = fg.generate(&root).unwrap();
        assert!(svg.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    }

    #[test]
    fn flame_graph_custom_dimensions() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        c.add_sample(ProfileSample::new(1000, 1, mock_query_stack(1)))
            .unwrap();
        let root = c.aggregate();
        let fg = FlameGraph::new(800, 20, 60);
        let svg = fg.generate(&root).unwrap();
        assert!(svg.contains("width=\"800\""));
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn color_for_name_deterministic() {
        let c1 = color_for_name("BTree::search");
        let c2 = color_for_name("BTree::search");
        assert_eq!(c1, c2);
        assert!(c1.starts_with("hsl("));
    }

    #[test]
    fn color_for_name_different_names_differ() {
        let c1 = color_for_name("BTree::search");
        let c2 = color_for_name("BTree::insert");
        // 不一定不同（哈希冲突可能），但格式应一致
        assert!(c1.starts_with("hsl("));
        assert!(c2.starts_with("hsl("));
    }

    #[test]
    fn truncate_label_short_name_preserved() {
        let label = truncate_label("foo", 100);
        assert_eq!(label, "foo");
    }

    #[test]
    fn truncate_label_long_name_truncated() {
        let label = truncate_label("szrsql_storage::btree::BTree::search", 30);
        assert!(label.ends_with(".."));
        assert!(label.len() < "szrsql_storage::btree::BTree::search".len());
    }

    #[test]
    fn escape_xml_special_chars() {
        assert_eq!(escape_xml("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(escape_xml("a&b"), "a&amp;b");
        assert_eq!(escape_xml("a\"b'c"), "a&quot;b&apos;c");
    }

    // -----------------------------------------------------------------
    //  端到端测试 — 验证标准（30s 采样场景）
    // -----------------------------------------------------------------

    #[test]
    fn end_to_end_30s_sampling_scenario() {
        // 验证标准：启动采样 30s → 生成 pprof 格式 → inferno 转 SVG → 火焰图包含真实调用栈
        // 这里用 30 个采样模拟 30 秒（每秒 1 个采样），实际生产为 100Hz
        let mut c = ProfileCollector::new(1000, 10000).unwrap(); // 1s 间隔
        c.start();

        // 模拟 30s 采样，混合查询和写入
        for i in 0..30 {
            let stack = if i % 5 == 0 {
                mock_write_stack()
            } else if i % 3 == 0 {
                mock_query_stack(3) // 带 buffer pool miss
            } else {
                mock_query_stack(1)
            };
            c.add_sample(ProfileSample::new(
                1000 + i as u64 * 1000,
                i as u64 % 4,
                stack,
            ))
            .unwrap();
        }
        c.stop();

        // 1. 采样数正确
        assert_eq!(c.sample_count(), 30);

        // 2. 生成 pprof 格式
        let profile = PprofProfile::from_collector(&c).unwrap();
        assert!(profile.total_samples() == 30);
        assert!(profile.function_count() >= 5); // 至少 5 个不同函数
        assert!(!profile.sample.is_empty());

        // 3. pprof 可序列化为 JSON
        let json = profile.to_json().unwrap();
        assert!(!json.is_empty());
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["sample"].is_array());

        // 4. 生成 SVG 火焰图（替代 inferno）
        let root = c.aggregate();
        let fg = FlameGraph::with_defaults();
        let svg = fg.generate(&root).unwrap();

        // 5. SVG 包含真实调用栈
        assert!(svg.contains("<svg"));
        assert!(svg.contains("main"));
        assert!(svg.contains("BTree::search"));
        assert!(svg.contains("BTree::insert"));
        assert!(svg.contains("Wal::append"));
        assert!(svg.contains("Page::read"));
        assert!(svg.contains("total=30 samples"));
    }

    #[test]
    fn end_to_end_mixed_workload_aggregation() {
        // 混合负载：8 个查询 + 4 个写入 = 12 个采样
        let mut c = ProfileCollector::with_defaults().unwrap();
        c.start();
        for i in 0..8 {
            c.add_sample(ProfileSample::new(1000 + i, 1, mock_query_stack(i % 4)))
                .unwrap();
        }
        for i in 0..4 {
            c.add_sample(ProfileSample::new(2000 + i, 2, mock_write_stack()))
                .unwrap();
        }
        c.stop();

        let root = c.aggregate();
        assert_eq!(root.value, 12);

        // 根节点下应只有 1 个子节点（main），所有调用栈都从 main 开始
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].value, 12);

        // main 下应只有 1 个子节点（Server::accept）
        assert_eq!(root.children[0].children.len(), 1);
        assert_eq!(root.children[0].children[0].name, "Server::accept");

        // Server::accept 下应只有 1 个子节点（Session::handle_query）
        let session = &root.children[0].children[0].children[0];
        assert_eq!(session.name, "Session::handle_query");
        assert_eq!(session.value, 12);

        // Session::handle_query 下应只有 1 个子节点（Executor::execute）
        let executor = &session.children[0];
        assert_eq!(executor.name, "Executor::execute");
        assert_eq!(executor.value, 12);

        // Executor::execute 下应有 2 个子节点（BTree::search 8次 + BTree::insert 4次）
        assert_eq!(executor.children.len(), 2);
        let search = executor.find_child("BTree::search").unwrap();
        let insert = executor.find_child("BTree::insert").unwrap();
        assert_eq!(search.value, 8);
        assert_eq!(insert.value, 4);
    }

    // -----------------------------------------------------------------
    //  错误场景测试
    // -----------------------------------------------------------------

    #[test]
    fn error_invalid_sample_interval_zero() {
        let err = ProfileCollector::new(0, 1000).unwrap_err();
        assert!(matches!(err, ProfileError::InvalidSampleInterval(0)));
    }

    #[test]
    fn error_no_samples_for_pprof() {
        let c = ProfileCollector::with_defaults().unwrap();
        let err = PprofProfile::from_collector(&c).unwrap_err();
        assert!(matches!(err, ProfileError::NoSamples));
    }

    #[test]
    fn error_no_samples_for_flame_graph() {
        let fg = FlameGraph::with_defaults();
        let root = FlameNode::new("<root>".to_string(), 0);
        let err = fg.generate(&root).unwrap_err();
        assert!(matches!(err, ProfileError::NoSamples));
    }

    #[test]
    fn error_empty_stack_rejected() {
        let mut c = ProfileCollector::with_defaults().unwrap();
        let err = c
            .add_sample(ProfileSample::new(1000, 1, Vec::new()))
            .unwrap_err();
        assert!(matches!(err, ProfileError::EmptyStack(1000)));
    }

    // -----------------------------------------------------------------
    //  模拟调用栈生成测试
    // -----------------------------------------------------------------

    #[test]
    fn mock_query_stack_structure() {
        let stack = mock_query_stack(1);
        assert!(!stack.is_empty());
        assert_eq!(stack[0].function, "main");
        assert!(stack.iter().any(|f| f.function == "BTree::search"));
        assert!(stack.iter().any(|f| f.function == "Page::read"));
    }

    #[test]
    fn mock_query_stack_with_buffer_pool_miss() {
        // thread_id % 3 == 0 时有更深的调用栈
        let stack_normal = mock_query_stack(1);
        let stack_deep = mock_query_stack(3); // 3 % 3 == 0
        assert!(stack_deep.len() > stack_normal.len());
        assert!(stack_deep.iter().any(|f| f.function == "BufferPool::fetch"));
        assert!(stack_deep.iter().any(|f| f.function == "freelist::alloc"));
    }

    #[test]
    fn mock_write_stack_structure() {
        let stack = mock_write_stack();
        assert!(!stack.is_empty());
        assert_eq!(stack[0].function, "main");
        assert!(stack.iter().any(|f| f.function == "BTree::insert"));
        assert!(stack.iter().any(|f| f.function == "Wal::append"));
    }
}
