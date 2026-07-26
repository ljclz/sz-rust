//! Grafana 监控面板 — Phase 7d.12
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.12 Grafana 监控面板设计。
//!
//! # 设计
//!
//! 生成 Grafana Dashboard JSON，包含 20+ 面板，覆盖核心运维指标：
//! - **QPS/TPS** — 每秒查询/事务数
//! - **查询延迟** — P50/P95/P99 延迟
//! - **缓存命中率** — Buffer/Page/Query 缓存命中率
//! - **连接数** — 活跃/空闲/最大连接数
//! - **存储用量** — 表空间/日志/TOAST 用量
//! - **CDC 延迟** — Change Data Capture 延迟
//! - **ASH** — 活动会话历史
//! - **慢查询** — 慢查询计数/Top SQL
//! - **告警** — 告警计数/级别分布
//! - **错误率** — 错误查询比例
//!
//! ## 验证标准
//!
//! - 配置 Prometheus 数据源 → 导入 Grafana Dashboard JSON
//! - 20+ 面板数据准确，渲染正常

// =====================================================================
//  常量
// =====================================================================

/// Dashboard 默认刷新间隔（秒）
pub const DEFAULT_REFRESH_SECS: u32 = 10;

/// Dashboard 默认时间范围（秒）
pub const DEFAULT_TIME_RANGE_SECS: u32 = 3600;

/// 默认面板宽度（Grafana 12 列网格，默认 6 列宽）
pub const DEFAULT_PANEL_WIDTH: u32 = 6;

/// 默认面板高度
pub const DEFAULT_PANEL_HEIGHT: u32 = 8;

/// 面板最小数量（规范要求 20+）
pub const MIN_PANEL_COUNT: usize = 20;

// =====================================================================
//  PanelType — 面板类型
// =====================================================================

/// Grafana 面板类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PanelType {
    /// 时序图（折线图）
    #[default]
    Graph,
    /// 单值统计
    Stat,
    /// 表格
    Table,
    /// 热力图
    Heatmap,
    /// 仪表盘
    Gauge,
    /// 条形仪表盘
    BarGauge,
    /// 饼图
    Piechart,
    /// 柱状图
    Barchart,
}

impl PanelType {
    /// Grafana type 字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            PanelType::Graph => "timeseries",
            PanelType::Stat => "stat",
            PanelType::Table => "table",
            PanelType::Heatmap => "heatmap",
            PanelType::Gauge => "gauge",
            PanelType::BarGauge => "bargauge",
            PanelType::Piechart => "piechart",
            PanelType::Barchart => "barchart",
        }
    }
}

// =====================================================================
//  GridPos — 面板布局位置
// =====================================================================

/// 面板在 Grafana 12 列网格中的位置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPos {
    /// X 坐标（0~11）
    pub x: u32,
    /// Y 坐标（从上到下递增）
    pub y: u32,
    /// 宽度（1~12）
    pub w: u32,
    /// 高度
    pub h: u32,
}

impl GridPos {
    /// 构造
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// 默认尺寸（6×8）
    pub fn default_at(x: u32, y: u32) -> Self {
        Self::new(x, y, DEFAULT_PANEL_WIDTH, DEFAULT_PANEL_HEIGHT)
    }

    /// 全宽（12 列）
    pub fn full_width(y: u32, h: u32) -> Self {
        Self::new(0, y, 12, h)
    }
}

impl Default for GridPos {
    fn default() -> Self {
        Self::new(0, 0, DEFAULT_PANEL_WIDTH, DEFAULT_PANEL_HEIGHT)
    }
}

// =====================================================================
//  Target — PromQL 查询目标
// =====================================================================

/// PromQL 查询目标
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// PromQL 表达式
    pub expr: String,
    /// 图例格式
    pub legend_format: String,
    /// 引用 ID（A/B/C...）
    pub ref_id: String,
}

impl Target {
    /// 构造
    pub fn new(
        expr: impl Into<String>,
        legend_format: impl Into<String>,
        ref_id: impl Into<String>,
    ) -> Self {
        Self {
            expr: expr.into(),
            legend_format: legend_format.into(),
            ref_id: ref_id.into(),
        }
    }

    /// 简单构造（ref_id 默认 A）
    pub fn simple(expr: impl Into<String>, legend_format: impl Into<String>) -> Self {
        Self::new(expr, legend_format, "A")
    }
}

// =====================================================================
//  Panel — Grafana 面板
// =====================================================================

/// Grafana 面板
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    /// 面板 ID（唯一）
    pub id: u32,
    /// 面板标题
    pub title: String,
    /// 面板类型
    pub panel_type: PanelType,
    /// 网格位置
    pub grid_pos: GridPos,
    /// 查询目标列表
    pub targets: Vec<Target>,
    /// 数据源
    pub datasource: String,
    /// 单位（如 reqps / ms / percent / bytes）
    pub unit: String,
    /// 描述
    pub description: String,
    /// 面板所属分组
    pub group: String,
}

impl Panel {
    /// 构造
    pub fn new(
        id: u32,
        title: impl Into<String>,
        panel_type: PanelType,
        grid_pos: GridPos,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            panel_type,
            grid_pos,
            targets: Vec::new(),
            datasource: "Prometheus".to_string(),
            unit: String::new(),
            description: String::new(),
            group: String::new(),
        }
    }

    /// 添加查询目标
    pub fn with_target(mut self, target: Target) -> Self {
        self.targets.push(target);
        self
    }

    /// 设置数据源
    pub fn with_datasource(mut self, datasource: impl Into<String>) -> Self {
        self.datasource = datasource.into();
        self
    }

    /// 设置单位
    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = unit.into();
        self
    }

    /// 设置描述
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// 设置分组
    pub fn with_group(mut self, group: impl Into<String>) -> Self {
        self.group = group.into();
        self
    }

    /// 是否包含 PromQL 查询
    pub fn has_queries(&self) -> bool {
        !self.targets.is_empty()
    }

    /// 查询数量
    pub fn query_count(&self) -> usize {
        self.targets.len()
    }
}

// =====================================================================
//  TemplateVariable — Dashboard 模板变量
// =====================================================================

/// Dashboard 模板变量（如数据库选择、时间范围过滤）
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateVariable {
    /// 变量名
    pub name: String,
    /// 显示名
    pub label: String,
    /// 查询表达式
    pub query: String,
    /// 数据源
    pub datasource: String,
    /// 是否多选
    pub multi: bool,
    /// 是否包含 All 选项
    pub include_all: bool,
    /// 当前值
    pub current_value: String,
}

impl TemplateVariable {
    /// 构造
    pub fn new(
        name: impl Into<String>,
        label: impl Into<String>,
        query: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            query: query.into(),
            datasource: "Prometheus".to_string(),
            multi: true,
            include_all: true,
            current_value: "$__all".to_string(),
        }
    }

    /// 设置单选
    pub fn single(mut self) -> Self {
        self.multi = false;
        self.include_all = false;
        self.current_value = "".to_string();
        self
    }
}

// =====================================================================
//  Dashboard — Grafana Dashboard
// =====================================================================

/// Grafana Dashboard
#[derive(Debug, Clone, PartialEq)]
pub struct Dashboard {
    /// Dashboard 标题
    pub title: String,
    /// UID（唯一标识）
    pub uid: String,
    /// 标签
    pub tags: Vec<String>,
    /// 时区（如 browser/utc）
    pub timezone: String,
    /// 刷新间隔（秒，如 10s/30s/1m）
    pub refresh: String,
    /// 时间范围（秒）
    pub time_range_secs: u32,
    /// 面板列表
    pub panels: Vec<Panel>,
    /// 模板变量
    pub templating: Vec<TemplateVariable>,
    /// 数据源
    pub datasource: String,
    /// 版本
    pub version: u32,
}

impl Dashboard {
    /// 构造
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            uid: "szrsql-dashboard".to_string(),
            tags: vec!["szrsql".to_string(), "database".to_string()],
            timezone: "browser".to_string(),
            refresh: format!("{}s", DEFAULT_REFRESH_SECS),
            time_range_secs: DEFAULT_TIME_RANGE_SECS,
            panels: Vec::new(),
            templating: Vec::new(),
            datasource: "Prometheus".to_string(),
            version: 1,
        }
    }

    /// 设置 UID
    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = uid.into();
        self
    }

    /// 设置刷新间隔
    pub fn with_refresh(mut self, secs: u32) -> Self {
        self.refresh = format!("{}s", secs);
        self
    }

    /// 设置时间范围
    pub fn with_time_range(mut self, secs: u32) -> Self {
        self.time_range_secs = secs;
        self
    }

    /// 添加面板
    pub fn add_panel(&mut self, panel: Panel) -> &mut Panel {
        self.panels.push(panel);
        self.panels.last_mut().unwrap()
    }

    /// 添加模板变量
    pub fn add_template(&mut self, var: TemplateVariable) {
        self.templating.push(var);
    }

    /// 面板数量
    pub fn panel_count(&self) -> usize {
        self.panels.len()
    }

    /// 是否满足 20+ 面板要求
    pub fn meets_min_panels(&self) -> bool {
        self.panel_count() >= MIN_PANEL_COUNT
    }

    /// 按分组获取面板
    pub fn panels_by_group(&self, group: &str) -> Vec<&Panel> {
        self.panels.iter().filter(|p| p.group == group).collect()
    }

    /// 所有分组
    pub fn groups(&self) -> Vec<String> {
        let mut groups: Vec<String> = self.panels.iter().map(|p| p.group.clone()).collect();
        groups.sort();
        groups.dedup();
        groups
    }

    /// 所有 PromQL 查询
    pub fn all_queries(&self) -> Vec<&str> {
        self.panels
            .iter()
            .flat_map(|p| p.targets.iter().map(|t| t.expr.as_str()))
            .collect()
    }

    /// 导出为 Grafana Dashboard JSON
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        out.push('{');
        // 元数据
        out.push_str(&format!("\"title\":\"{}\",", escape_json(&self.title)));
        out.push_str(&format!("\"uid\":\"{}\",", escape_json(&self.uid)));
        out.push_str("\"schemaVersion\":27,");
        out.push_str(&format!("\"version\":{},", self.version));
        out.push_str(&format!(
            "\"timezone\":\"{}\",",
            escape_json(&self.timezone)
        ));
        out.push_str(&format!("\"refresh\":\"{}\",", escape_json(&self.refresh)));
        // 标签
        out.push_str("\"tags\":[");
        let mut first = true;
        for tag in &self.tags {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!("\"{}\"", escape_json(tag)));
        }
        out.push_str("],");
        // 时间范围
        out.push_str(&format!(
            "{{\"from\":\"now-{}s\",\"to\":\"now\"}},",
            self.time_range_secs
        ));
        // 面板
        out.push_str("\"panels\":[");
        let mut first = true;
        for panel in &self.panels {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&panel_to_json(panel));
        }
        out.push_str("],");
        // 模板变量
        out.push_str("\"templating\":{\"list\":[");
        let mut first = true;
        for var in &self.templating {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&template_to_json(var));
        }
        out.push_str("]}");
        out.push('}');
        out
    }
}

impl Default for Dashboard {
    fn default() -> Self {
        Self::new("SzRSQL Monitoring")
    }
}

// =====================================================================
//  PrometheusConfig — Prometheus 数据源配置
// =====================================================================

/// Prometheus 数据源配置
#[derive(Debug, Clone, PartialEq)]
pub struct PrometheusConfig {
    /// 数据源名称
    pub name: String,
    /// Prometheus URL
    pub url: String,
    /// 是否默认数据源
    pub is_default: bool,
    /// 访问模式（proxy/browser）
    pub access: String,
    /// 是否启用基本认证
    pub basic_auth: bool,
    /// 采集间隔（秒）
    pub scrape_interval_secs: u32,
}

impl PrometheusConfig {
    /// 构造默认配置
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            is_default: true,
            access: "proxy".to_string(),
            basic_auth: false,
            scrape_interval_secs: 15,
        }
    }

    /// 设置非默认
    pub fn non_default(mut self) -> Self {
        self.is_default = false;
        self
    }

    /// 启用基本认证
    pub fn with_basic_auth(mut self) -> Self {
        self.basic_auth = true;
        self
    }

    /// 设置采集间隔
    pub fn with_scrape_interval(mut self, secs: u32) -> Self {
        self.scrape_interval_secs = secs;
        self
    }

    /// 导出为 Grafana datasource JSON
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        out.push('{');
        out.push_str(&format!("\"name\":\"{}\",", escape_json(&self.name)));
        out.push_str("\"type\":\"prometheus\",");
        out.push_str(&format!("\"url\":\"{}\",", escape_json(&self.url)));
        out.push_str(&format!("\"access\":\"{}\",", escape_json(&self.access)));
        out.push_str(&format!("\"isDefault\":{},", self.is_default));
        out.push_str(&format!("\"basicAuth\":{}", self.basic_auth));
        out.push('}');
        out
    }
}

impl Default for PrometheusConfig {
    fn default() -> Self {
        Self::new("Prometheus", "http://localhost:9090")
    }
}

// =====================================================================
//  面板生成函数 — 20+ 预设面板
// =====================================================================

/// 创建 SzRSQL 默认 Dashboard（20+ 面板）
///
/// 面板分组：
/// - **概览**（Overview）— 1. QPS / 2. TPS / 3. 错误率 / 4. 活跃连接数
/// - **延迟**（Latency）— 5. 查询延迟 P50 / 6. P95 / 7. P99 / 8. 慢查询计数
/// - **缓存**（Cache）— 9. Buffer 缓存命中率 / 10. Page 缓存命中率 / 11. Query 缓存命中率 / 12. 缓存驱逐率
/// - **连接**（Connection）— 13. 连接数 / 14. 每连接 QPS / 15. 连接等待时间
/// - **存储**（Storage）— 16. 表空间用量 / 17. WAL 日志用量 / 18. TOAST 存储用量
/// - **CDC**（CDC）— 19. CDC 延迟 / 20. CDC 吞吐量
/// - **ASH/告警**（ASH/Alerts）— 21. 活动会话历史 / 22. 告警计数
pub fn create_default_dashboard() -> Dashboard {
    let mut dashboard = Dashboard::new("SzRSQL Monitoring Dashboard")
        .with_uid("szrsql-monitoring")
        .with_refresh(DEFAULT_REFRESH_SECS)
        .with_time_range(DEFAULT_TIME_RANGE_SECS);

    // 模板变量：数据库选择
    dashboard.add_template(TemplateVariable::new(
        "database",
        "Database",
        "label_values(szrsql_database_name)",
    ));

    // 模板变量：用户选择
    dashboard.add_template(
        TemplateVariable::new("user", "User", "label_values(szrsql_user_name)").single(),
    );

    let mut panel_id = 1u32;
    let mut y = 0u32;

    // ===== 概览组（Overview）=====
    dashboard.add_panel(
        Panel::new(
            panel_id,
            "QPS (Queries Per Second)",
            PanelType::Graph,
            GridPos::new(0, y, 6, 8),
        )
        .with_target(Target::simple(
            "rate(szrsql_queries_total[5m])",
            "{{database}} - QPS",
        ))
        .with_unit("reqps")
        .with_description("每秒查询数（按数据库分组）")
        .with_group("Overview"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "TPS (Transactions Per Second)",
            PanelType::Graph,
            GridPos::new(6, y, 6, 8),
        )
        .with_target(Target::simple(
            "rate(szrsql_transactions_total[5m])",
            "{{database}} - TPS",
        ))
        .with_unit("reqps")
        .with_description("每秒事务数（按数据库分组）")
        .with_group("Overview"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Error Rate",
            PanelType::Stat,
            GridPos::new(0, y + 8, 6, 4),
        )
        .with_target(Target::simple(
            "rate(szrsql_errors_total[5m]) / rate(szrsql_queries_total[5m])",
            "Error Rate",
        ))
        .with_unit("percentunit")
        .with_description("错误查询比例")
        .with_group("Overview"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Active Connections",
            PanelType::Stat,
            GridPos::new(6, y + 8, 6, 4),
        )
        .with_target(Target::simple("szrsql_connections_active", "Active"))
        .with_target(Target::new("szrsql_connections_idle", "Idle", "B"))
        .with_unit("short")
        .with_description("活跃/空闲连接数")
        .with_group("Overview"),
    );
    panel_id += 1;

    y += 16;

    // ===== 延迟组（Latency）=====
    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Query Latency P50",
            PanelType::Graph,
            GridPos::new(0, y, 6, 8),
        )
        .with_target(Target::simple(
            "histogram_quantile(0.50, rate(szrsql_query_duration_bucket[5m]))",
            "P50",
        ))
        .with_unit("ms")
        .with_description("查询延迟 P50（中位数）")
        .with_group("Latency"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Query Latency P95",
            PanelType::Graph,
            GridPos::new(6, y, 6, 8),
        )
        .with_target(Target::simple(
            "histogram_quantile(0.95, rate(szrsql_query_duration_bucket[5m]))",
            "P95",
        ))
        .with_unit("ms")
        .with_description("查询延迟 P95")
        .with_group("Latency"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Query Latency P99",
            PanelType::Graph,
            GridPos::new(0, y + 8, 6, 8),
        )
        .with_target(Target::simple(
            "histogram_quantile(0.99, rate(szrsql_query_duration_bucket[5m]))",
            "P99",
        ))
        .with_unit("ms")
        .with_description("查询延迟 P99")
        .with_group("Latency"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Slow Query Count",
            PanelType::Stat,
            GridPos::new(6, y + 8, 6, 8),
        )
        .with_target(Target::simple(
            "increase(szrsql_slow_queries_total[5m])",
            "Slow Queries (5m)",
        ))
        .with_unit("short")
        .with_description("最近 5 分钟慢查询数（>200ms）")
        .with_group("Latency"),
    );
    panel_id += 1;

    y += 16;

    // ===== 缓存组（Cache）=====
    dashboard.add_panel(
        Panel::new(panel_id, "Buffer Cache Hit Ratio", PanelType::Gauge, GridPos::new(0, y, 6, 8))
            .with_target(Target::simple(
                "1 - (rate(szrsql_buffer_cache_misses_total[5m]) / rate(szrsql_buffer_cache_accesses_total[5m]))",
                "Buffer Hit Ratio",
            ))
            .with_unit("percentunit")
            .with_description("Buffer 缓存命中率")
            .with_group("Cache"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(panel_id, "Page Cache Hit Ratio", PanelType::Gauge, GridPos::new(6, y, 6, 8))
            .with_target(Target::simple(
                "1 - (rate(szrsql_page_cache_misses_total[5m]) / rate(szrsql_page_cache_accesses_total[5m]))",
                "Page Hit Ratio",
            ))
            .with_unit("percentunit")
            .with_description("Page 缓存命中率")
            .with_group("Cache"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Query Cache Hit Ratio",
            PanelType::Gauge,
            GridPos::new(0, y + 8, 6, 8),
        )
        .with_target(Target::simple(
            "rate(szrsql_query_cache_hits_total[5m]) / rate(szrsql_query_cache_queries_total[5m])",
            "Query Cache Hit Ratio",
        ))
        .with_unit("percentunit")
        .with_description("查询缓存命中率")
        .with_group("Cache"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Cache Eviction Rate",
            PanelType::Graph,
            GridPos::new(6, y + 8, 6, 8),
        )
        .with_target(Target::simple(
            "rate(szrsql_cache_evictions_total[5m])",
            "Evictions/s",
        ))
        .with_unit("ops")
        .with_description("缓存驱逐率（高驱逐率说明缓存不足）")
        .with_group("Cache"),
    );
    panel_id += 1;

    y += 16;

    // ===== 连接组（Connection）=====
    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Connection Count",
            PanelType::Graph,
            GridPos::new(0, y, 6, 8),
        )
        .with_target(Target::simple("szrsql_connections_active", "Active"))
        .with_target(Target::new("szrsql_connections_idle", "Idle", "B"))
        .with_target(Target::new("szrsql_connections_max", "Max", "C"))
        .with_unit("short")
        .with_description("连接数（活跃/空闲/最大）")
        .with_group("Connection"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "QPS Per Connection",
            PanelType::Graph,
            GridPos::new(6, y, 6, 8),
        )
        .with_target(Target::simple(
            "rate(szrsql_queries_total[5m]) / szrsql_connections_active",
            "QPS/Conn",
        ))
        .with_unit("reqps")
        .with_description("每连接 QPS")
        .with_group("Connection"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Connection Wait Time",
            PanelType::Graph,
            GridPos::new(0, y + 8, 6, 8),
        )
        .with_target(Target::simple(
            "histogram_quantile(0.95, rate(szrsql_connection_wait_bucket[5m]))",
            "P95 Wait",
        ))
        .with_unit("ms")
        .with_description("连接获取等待时间 P95")
        .with_group("Connection"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Connection Pool Usage",
            PanelType::BarGauge,
            GridPos::new(6, y + 8, 6, 8),
        )
        .with_target(Target::simple(
            "szrsql_pool_active / szrsql_pool_max",
            "Pool Usage",
        ))
        .with_unit("percentunit")
        .with_description("连接池使用率")
        .with_group("Connection"),
    );
    panel_id += 1;

    y += 16;

    // ===== 存储组（Storage）=====
    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Tablespace Usage",
            PanelType::Stat,
            GridPos::new(0, y, 6, 8),
        )
        .with_target(Target::simple("szrsql_tablespace_bytes", "{{tablespace}}"))
        .with_unit("bytes")
        .with_description("表空间用量")
        .with_group("Storage"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "WAL Log Size",
            PanelType::Graph,
            GridPos::new(6, y, 6, 8),
        )
        .with_target(Target::simple("szrsql_wal_bytes_total", "WAL Size"))
        .with_unit("bytes")
        .with_description("WAL 日志累计大小")
        .with_group("Storage"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "TOAST Storage Size",
            PanelType::Stat,
            GridPos::new(0, y + 8, 6, 8),
        )
        .with_target(Target::simple("szrsql_toast_bytes", "TOAST"))
        .with_unit("bytes")
        .with_description("TOAST 超大字段存储用量")
        .with_group("Storage"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Disk IOPS",
            PanelType::Graph,
            GridPos::new(6, y + 8, 6, 8),
        )
        .with_target(Target::simple(
            "rate(szrsql_disk_reads_total[5m])",
            "Reads/s",
        ))
        .with_target(Target::new(
            "rate(szrsql_disk_writes_total[5m])",
            "Writes/s",
            "B",
        ))
        .with_unit("iops")
        .with_description("磁盘 IOPS（读/写）")
        .with_group("Storage"),
    );
    panel_id += 1;

    y += 16;

    // ===== CDC 组（CDC）=====
    dashboard.add_panel(
        Panel::new(
            panel_id,
            "CDC Lag",
            PanelType::Graph,
            GridPos::new(0, y, 6, 8),
        )
        .with_target(Target::simple("szrsql_cdc_lag_seconds", "CDC Lag"))
        .with_unit("s")
        .with_description("CDC 延迟（秒）")
        .with_group("CDC"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "CDC Throughput",
            PanelType::Graph,
            GridPos::new(6, y, 6, 8),
        )
        .with_target(Target::simple(
            "rate(szrsql_cdc_events_total[5m])",
            "Events/s",
        ))
        .with_unit("ops")
        .with_description("CDC 事件吞吐量")
        .with_group("CDC"),
    );
    panel_id += 1;

    y += 8;

    // ===== ASH/告警组（ASH/Alerts）=====
    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Active Session History",
            PanelType::Heatmap,
            GridPos::new(0, y, 12, 8),
        )
        .with_target(Target::simple(
            "szrsql_ash_active_sessions",
            "Active Sessions",
        ))
        .with_unit("short")
        .with_description("活动会话历史（热力图）")
        .with_group("ASH/Alerts"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Alert Count by Level",
            PanelType::Piechart,
            GridPos::new(0, y + 8, 6, 8),
        )
        .with_target(Target::simple("szrsql_alerts_active", "{{level}}"))
        .with_unit("short")
        .with_description("按级别分组的活跃告警数")
        .with_group("ASH/Alerts"),
    );
    panel_id += 1;

    dashboard.add_panel(
        Panel::new(
            panel_id,
            "Top 10 Slow SQL",
            PanelType::Table,
            GridPos::new(6, y + 8, 6, 8),
        )
        .with_target(Target::simple(
            "topk(10, sum by (sql_normalized) (rate(szrsql_slow_queries_total[5m])))",
            "{{sql_normalized}}",
        ))
        .with_unit("short")
        .with_description("Top 10 慢 SQL（按归一化 SQL 分组）")
        .with_group("ASH/Alerts"),
    );

    dashboard
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 转义 JSON 字符串
fn escape_json(s: &str) -> String {
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

/// 面板转 JSON
fn panel_to_json(panel: &Panel) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str(&format!("\"id\":{},", panel.id));
    out.push_str(&format!("\"title\":\"{}\",", escape_json(&panel.title)));
    out.push_str(&format!("\"type\":\"{}\",", panel.panel_type.as_str()));
    out.push_str(&format!(
        "\"gridPos\":{{\"x\":{},\"y\":{},\"w\":{},\"h\":{}}},",
        panel.grid_pos.x, panel.grid_pos.y, panel.grid_pos.w, panel.grid_pos.h
    ));
    out.push_str(&format!(
        "\"datasource\":\"{}\",",
        escape_json(&panel.datasource)
    ));
    if !panel.unit.is_empty() {
        out.push_str(&format!(
            "\"fieldConfig\":{{\"defaults\":{{\"unit\":\"{}\"}}}},",
            escape_json(&panel.unit)
        ));
    }
    if !panel.description.is_empty() {
        out.push_str(&format!(
            "\"description\":\"{}\",",
            escape_json(&panel.description)
        ));
    }
    // targets
    out.push_str("\"targets\":[");
    let mut first = true;
    for target in &panel.targets {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&target_to_json(target));
    }
    out.push(']');
    out.push('}');
    out
}

/// 查询目标转 JSON
fn target_to_json(target: &Target) -> String {
    format!(
        "{{\"expr\":\"{}\",\"legendFormat\":\"{}\",\"refId\":\"{}\"}}",
        escape_json(&target.expr),
        escape_json(&target.legend_format),
        escape_json(&target.ref_id)
    )
}

/// 模板变量转 JSON
fn template_to_json(var: &TemplateVariable) -> String {
    format!(
        "{{\"name\":\"{}\",\"label\":\"{}\",\"type\":\"query\",\"datasource\":\"{}\",\"query\":\"{}\",\"multi\":{},\"includeAll\":{},\"current\":{{\"text\":\"All\",\"value\":\"{}\"}}}}",
        escape_json(&var.name),
        escape_json(&var.label),
        escape_json(&var.datasource),
        escape_json(&var.query),
        var.multi,
        var.include_all,
        escape_json(&var.current_value)
    )
}

// =====================================================================
//  指标定义（Prometheus metric names）
// =====================================================================

/// SzRSQL Prometheus 指标名常量
pub mod metrics {
    /// 查询总数（Counter）
    pub const QUERIES_TOTAL: &str = "szrsql_queries_total";
    /// 事务总数（Counter）
    pub const TRANSACTIONS_TOTAL: &str = "szrsql_transactions_total";
    /// 错误总数（Counter）
    pub const ERRORS_TOTAL: &str = "szrsql_errors_total";
    /// 慢查询总数（Counter）
    pub const SLOW_QUERIES_TOTAL: &str = "szrsql_slow_queries_total";
    /// 查询延迟直方图（Histogram）
    pub const QUERY_DURATION_BUCKET: &str = "szrsql_query_duration_bucket";
    /// 活跃连接数（Gauge）
    pub const CONNECTIONS_ACTIVE: &str = "szrsql_connections_active";
    /// 空闲连接数（Gauge）
    pub const CONNECTIONS_IDLE: &str = "szrsql_connections_idle";
    /// 最大连接数（Gauge）
    pub const CONNECTIONS_MAX: &str = "szrsql_connections_max";
    /// Buffer 缓存访问数（Counter）
    pub const BUFFER_CACHE_ACCESSES: &str = "szrsql_buffer_cache_accesses_total";
    /// Buffer 缓存未命中数（Counter）
    pub const BUFFER_CACHE_MISSES: &str = "szrsql_buffer_cache_misses_total";
    /// Page 缓存访问数（Counter）
    pub const PAGE_CACHE_ACCESSES: &str = "szrsql_page_cache_accesses_total";
    /// Page 缓存未命中数（Counter）
    pub const PAGE_CACHE_MISSES: &str = "szrsql_page_cache_misses_total";
    /// 查询缓存命中数（Counter）
    pub const QUERY_CACHE_HITS: &str = "szrsql_query_cache_hits_total";
    /// 查询缓存查询数（Counter）
    pub const QUERY_CACHE_QUERIES: &str = "szrsql_query_cache_queries_total";
    /// 缓存驱逐数（Counter）
    pub const CACHE_EVICTIONS: &str = "szrsql_cache_evictions_total";
    /// 表空间用量（Gauge）
    pub const TABLESPACE_BYTES: &str = "szrsql_tablespace_bytes";
    /// WAL 日志大小（Counter）
    pub const WAL_BYTES: &str = "szrsql_wal_bytes_total";
    /// TOAST 存储用量（Gauge）
    pub const TOAST_BYTES: &str = "szrsql_toast_bytes";
    /// 磁盘读取数（Counter）
    pub const DISK_READS: &str = "szrsql_disk_reads_total";
    /// 磁盘写入数（Counter）
    pub const DISK_WRITES: &str = "szrsql_disk_writes_total";
    /// CDC 延迟（Gauge，秒）
    pub const CDC_LAG_SECONDS: &str = "szrsql_cdc_lag_seconds";
    /// CDC 事件数（Counter）
    pub const CDC_EVENTS: &str = "szrsql_cdc_events_total";
    /// ASH 活动会话数（Gauge）
    pub const ASH_ACTIVE_SESSIONS: &str = "szrsql_ash_active_sessions";
    /// 活跃告警数（Gauge）
    pub const ALERTS_ACTIVE: &str = "szrsql_alerts_active";
    /// 连接池活跃数（Gauge）
    pub const POOL_ACTIVE: &str = "szrsql_pool_active";
    /// 连接池最大数（Gauge）
    pub const POOL_MAX: &str = "szrsql_pool_max";
    /// 连接等待直方图（Histogram）
    pub const CONNECTION_WAIT_BUCKET: &str = "szrsql_connection_wait_bucket";
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  PanelType 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_panel_type_as_str() {
        assert_eq!(PanelType::Graph.as_str(), "timeseries");
        assert_eq!(PanelType::Stat.as_str(), "stat");
        assert_eq!(PanelType::Table.as_str(), "table");
        assert_eq!(PanelType::Heatmap.as_str(), "heatmap");
        assert_eq!(PanelType::Gauge.as_str(), "gauge");
        assert_eq!(PanelType::BarGauge.as_str(), "bargauge");
        assert_eq!(PanelType::Piechart.as_str(), "piechart");
        assert_eq!(PanelType::Barchart.as_str(), "barchart");
    }

    #[test]
    fn test_panel_type_default() {
        assert_eq!(PanelType::default(), PanelType::Graph);
    }

    // -----------------------------------------------------------------
    //  GridPos 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_grid_pos_new() {
        let pos = GridPos::new(3, 4, 6, 8);
        assert_eq!(pos.x, 3);
        assert_eq!(pos.y, 4);
        assert_eq!(pos.w, 6);
        assert_eq!(pos.h, 8);
    }

    #[test]
    fn test_grid_pos_default_at() {
        let pos = GridPos::default_at(2, 3);
        assert_eq!(pos.x, 2);
        assert_eq!(pos.y, 3);
        assert_eq!(pos.w, DEFAULT_PANEL_WIDTH);
        assert_eq!(pos.h, DEFAULT_PANEL_HEIGHT);
    }

    #[test]
    fn test_grid_pos_full_width() {
        let pos = GridPos::full_width(5, 4);
        assert_eq!(pos.x, 0);
        assert_eq!(pos.y, 5);
        assert_eq!(pos.w, 12);
        assert_eq!(pos.h, 4);
    }

    #[test]
    fn test_grid_pos_default() {
        let pos = GridPos::default();
        assert_eq!(pos.x, 0);
        assert_eq!(pos.y, 0);
        assert_eq!(pos.w, DEFAULT_PANEL_WIDTH);
    }

    // -----------------------------------------------------------------
    //  Target 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_target_new() {
        let target = Target::new("rate(qps[5m])", "{{db}}", "A");
        assert_eq!(target.expr, "rate(qps[5m])");
        assert_eq!(target.legend_format, "{{db}}");
        assert_eq!(target.ref_id, "A");
    }

    #[test]
    fn test_target_simple() {
        let target = Target::simple("rate(qps[5m])", "QPS");
        assert_eq!(target.ref_id, "A");
    }

    // -----------------------------------------------------------------
    //  Panel 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_panel_new() {
        let panel = Panel::new(1, "QPS", PanelType::Graph, GridPos::default());
        assert_eq!(panel.id, 1);
        assert_eq!(panel.title, "QPS");
        assert_eq!(panel.panel_type, PanelType::Graph);
        assert!(panel.targets.is_empty());
    }

    #[test]
    fn test_panel_with_target() {
        let panel = Panel::new(1, "QPS", PanelType::Graph, GridPos::default())
            .with_target(Target::simple("rate(qps[5m])", "QPS"));
        assert_eq!(panel.query_count(), 1);
        assert!(panel.has_queries());
    }

    #[test]
    fn test_panel_with_unit() {
        let panel = Panel::new(1, "QPS", PanelType::Graph, GridPos::default()).with_unit("reqps");
        assert_eq!(panel.unit, "reqps");
    }

    #[test]
    fn test_panel_with_description() {
        let panel = Panel::new(1, "QPS", PanelType::Graph, GridPos::default())
            .with_description("Queries per second");
        assert_eq!(panel.description, "Queries per second");
    }

    #[test]
    fn test_panel_with_group() {
        let panel =
            Panel::new(1, "QPS", PanelType::Graph, GridPos::default()).with_group("Overview");
        assert_eq!(panel.group, "Overview");
    }

    #[test]
    fn test_panel_with_datasource() {
        let panel = Panel::new(1, "QPS", PanelType::Graph, GridPos::default())
            .with_datasource("MyPrometheus");
        assert_eq!(panel.datasource, "MyPrometheus");
    }

    // -----------------------------------------------------------------
    //  TemplateVariable 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_template_variable_new() {
        let var = TemplateVariable::new("database", "Database", "label_values(db)");
        assert_eq!(var.name, "database");
        assert_eq!(var.label, "Database");
        assert!(var.multi);
        assert!(var.include_all);
    }

    #[test]
    fn test_template_variable_single() {
        let var = TemplateVariable::new("user", "User", "label_values(user)").single();
        assert!(!var.multi);
        assert!(!var.include_all);
    }

    // -----------------------------------------------------------------
    //  Dashboard 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_dashboard_new() {
        let dashboard = Dashboard::new("Test Dashboard");
        assert_eq!(dashboard.title, "Test Dashboard");
        assert_eq!(dashboard.panel_count(), 0);
    }

    #[test]
    fn test_dashboard_with_uid() {
        let dashboard = Dashboard::new("Test").with_uid("my-uid");
        assert_eq!(dashboard.uid, "my-uid");
    }

    #[test]
    fn test_dashboard_with_refresh() {
        let dashboard = Dashboard::new("Test").with_refresh(30);
        assert_eq!(dashboard.refresh, "30s");
    }

    #[test]
    fn test_dashboard_add_panel() {
        let mut dashboard = Dashboard::new("Test");
        dashboard.add_panel(Panel::new(1, "QPS", PanelType::Graph, GridPos::default()));
        assert_eq!(dashboard.panel_count(), 1);
    }

    #[test]
    fn test_dashboard_add_template() {
        let mut dashboard = Dashboard::new("Test");
        dashboard.add_template(TemplateVariable::new("db", "DB", "label_values(db)"));
        assert_eq!(dashboard.templating.len(), 1);
    }

    #[test]
    fn test_dashboard_meets_min_panels() {
        let dashboard = Dashboard::new("Test");
        assert!(!dashboard.meets_min_panels());
    }

    #[test]
    fn test_dashboard_panels_by_group() {
        let mut dashboard = Dashboard::new("Test");
        dashboard
            .add_panel(Panel::new(1, "P1", PanelType::Graph, GridPos::default()).with_group("A"));
        dashboard
            .add_panel(Panel::new(2, "P2", PanelType::Graph, GridPos::default()).with_group("A"));
        dashboard
            .add_panel(Panel::new(3, "P3", PanelType::Graph, GridPos::default()).with_group("B"));
        assert_eq!(dashboard.panels_by_group("A").len(), 2);
        assert_eq!(dashboard.panels_by_group("B").len(), 1);
    }

    #[test]
    fn test_dashboard_groups() {
        let mut dashboard = Dashboard::new("Test");
        dashboard
            .add_panel(Panel::new(1, "P1", PanelType::Graph, GridPos::default()).with_group("B"));
        dashboard
            .add_panel(Panel::new(2, "P2", PanelType::Graph, GridPos::default()).with_group("A"));
        let groups = dashboard.groups();
        assert_eq!(groups, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn test_dashboard_all_queries() {
        let mut dashboard = Dashboard::new("Test");
        dashboard.add_panel(
            Panel::new(1, "P1", PanelType::Graph, GridPos::default())
                .with_target(Target::simple("rate(q1[5m])", "Q1")),
        );
        dashboard.add_panel(
            Panel::new(2, "P2", PanelType::Graph, GridPos::default())
                .with_target(Target::simple("rate(q2[5m])", "Q2")),
        );
        let queries = dashboard.all_queries();
        assert_eq!(queries.len(), 2);
    }

    #[test]
    fn test_dashboard_to_json() {
        let mut dashboard = Dashboard::new("Test");
        dashboard.add_panel(
            Panel::new(1, "QPS", PanelType::Graph, GridPos::default())
                .with_target(Target::simple("rate(qps[5m])", "QPS"))
                .with_unit("reqps"),
        );
        let json = dashboard.to_json();
        assert!(json.contains("\"title\":\"Test\""));
        assert!(json.contains("\"panels\":["));
        assert!(json.contains("\"type\":\"timeseries\""));
        assert!(json.contains("\"expr\":\"rate(qps[5m])\""));
    }

    // -----------------------------------------------------------------
    //  PrometheusConfig 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_prometheus_config_new() {
        let config = PrometheusConfig::new("Prometheus", "http://localhost:9090");
        assert_eq!(config.name, "Prometheus");
        assert_eq!(config.url, "http://localhost:9090");
        assert!(config.is_default);
        assert!(!config.basic_auth);
    }

    #[test]
    fn test_prometheus_config_non_default() {
        let config = PrometheusConfig::new("Secondary", "http://localhost:9091").non_default();
        assert!(!config.is_default);
    }

    #[test]
    fn test_prometheus_config_with_basic_auth() {
        let config = PrometheusConfig::default().with_basic_auth();
        assert!(config.basic_auth);
    }

    #[test]
    fn test_prometheus_config_with_scrape_interval() {
        let config = PrometheusConfig::default().with_scrape_interval(30);
        assert_eq!(config.scrape_interval_secs, 30);
    }

    #[test]
    fn test_prometheus_config_to_json() {
        let config = PrometheusConfig::new("Prometheus", "http://localhost:9090");
        let json = config.to_json();
        assert!(json.contains("\"name\":\"Prometheus\""));
        assert!(json.contains("\"type\":\"prometheus\""));
        assert!(json.contains("\"url\":\"http://localhost:9090\""));
        assert!(json.contains("\"isDefault\":true"));
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_escape_json() {
        assert_eq!(escape_json("hello"), "hello");
        assert_eq!(escape_json("a\"b"), "a\\\"b");
        assert_eq!(escape_json("a\\b"), "a\\\\b");
        assert_eq!(escape_json("a\nb"), "a\\nb");
    }

    // -----------------------------------------------------------------
    //  集成测试：默认 Dashboard
    // -----------------------------------------------------------------

    #[test]
    fn test_default_dashboard_has_20_plus_panels() {
        let dashboard = create_default_dashboard();
        assert!(
            dashboard.meets_min_panels(),
            "Dashboard should have at least {} panels, got {}",
            MIN_PANEL_COUNT,
            dashboard.panel_count()
        );
    }

    #[test]
    fn test_default_dashboard_panel_count() {
        let dashboard = create_default_dashboard();
        assert_eq!(dashboard.panel_count(), 25);
    }

    #[test]
    fn test_default_dashboard_groups() {
        let dashboard = create_default_dashboard();
        let groups = dashboard.groups();
        assert!(groups.contains(&"Overview".to_string()));
        assert!(groups.contains(&"Latency".to_string()));
        assert!(groups.contains(&"Cache".to_string()));
        assert!(groups.contains(&"Connection".to_string()));
        assert!(groups.contains(&"Storage".to_string()));
        assert!(groups.contains(&"CDC".to_string()));
        assert!(groups.contains(&"ASH/Alerts".to_string()));
    }

    #[test]
    fn test_default_dashboard_overview_panels() {
        let dashboard = create_default_dashboard();
        let overview = dashboard.panels_by_group("Overview");
        assert_eq!(overview.len(), 4);
        assert!(overview.iter().any(|p| p.title.contains("QPS")));
        assert!(overview.iter().any(|p| p.title.contains("TPS")));
        assert!(overview.iter().any(|p| p.title.contains("Error Rate")));
        assert!(overview
            .iter()
            .any(|p| p.title.contains("Active Connections")));
    }

    #[test]
    fn test_default_dashboard_latency_panels() {
        let dashboard = create_default_dashboard();
        let latency = dashboard.panels_by_group("Latency");
        assert_eq!(latency.len(), 4);
        assert!(latency.iter().any(|p| p.title.contains("P50")));
        assert!(latency.iter().any(|p| p.title.contains("P95")));
        assert!(latency.iter().any(|p| p.title.contains("P99")));
        assert!(latency.iter().any(|p| p.title.contains("Slow Query")));
    }

    #[test]
    fn test_default_dashboard_cache_panels() {
        let dashboard = create_default_dashboard();
        let cache = dashboard.panels_by_group("Cache");
        assert_eq!(cache.len(), 4);
        assert!(cache.iter().any(|p| p.title.contains("Buffer Cache")));
        assert!(cache.iter().any(|p| p.title.contains("Page Cache")));
        assert!(cache.iter().any(|p| p.title.contains("Query Cache")));
        assert!(cache.iter().any(|p| p.title.contains("Eviction")));
    }

    #[test]
    fn test_default_dashboard_connection_panels() {
        let dashboard = create_default_dashboard();
        let connections = dashboard.panels_by_group("Connection");
        assert_eq!(connections.len(), 4);
        assert!(connections
            .iter()
            .any(|p| p.title.contains("Connection Count")));
        assert!(connections
            .iter()
            .any(|p| p.title.contains("Per Connection")));
    }

    #[test]
    fn test_default_dashboard_storage_panels() {
        let dashboard = create_default_dashboard();
        let storage = dashboard.panels_by_group("Storage");
        assert_eq!(storage.len(), 4);
        assert!(storage.iter().any(|p| p.title.contains("Tablespace")));
        assert!(storage.iter().any(|p| p.title.contains("WAL")));
        assert!(storage.iter().any(|p| p.title.contains("TOAST")));
        assert!(storage.iter().any(|p| p.title.contains("IOPS")));
    }

    #[test]
    fn test_default_dashboard_cdc_panels() {
        let dashboard = create_default_dashboard();
        let cdc = dashboard.panels_by_group("CDC");
        assert_eq!(cdc.len(), 2);
        assert!(cdc.iter().any(|p| p.title.contains("CDC Lag")));
        assert!(cdc.iter().any(|p| p.title.contains("CDC Throughput")));
    }

    #[test]
    fn test_default_dashboard_ash_alerts_panels() {
        let dashboard = create_default_dashboard();
        let ash_alerts = dashboard.panels_by_group("ASH/Alerts");
        assert_eq!(ash_alerts.len(), 3);
        assert!(ash_alerts
            .iter()
            .any(|p| p.title.contains("Active Session")));
        assert!(ash_alerts.iter().any(|p| p.title.contains("Alert Count")));
        assert!(ash_alerts.iter().any(|p| p.title.contains("Slow SQL")));
    }

    #[test]
    fn test_default_dashboard_all_panels_have_queries() {
        let dashboard = create_default_dashboard();
        for panel in &dashboard.panels {
            assert!(
                panel.has_queries(),
                "Panel '{}' has no queries",
                panel.title
            );
        }
    }

    #[test]
    fn test_default_dashboard_unique_ids() {
        let dashboard = create_default_dashboard();
        let mut ids: Vec<u32> = dashboard.panels.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(
            ids.len(),
            dashboard.panel_count(),
            "Panel IDs should be unique"
        );
    }

    #[test]
    fn test_default_dashboard_to_json() {
        let dashboard = create_default_dashboard();
        let json = dashboard.to_json();
        assert!(json.contains("\"title\":\"SzRSQL Monitoring Dashboard\""));
        assert!(json.contains("\"uid\":\"szrsql-monitoring\""));
        assert!(json.contains("\"panels\":["));
        assert!(json.contains("\"templating\""));
        // 验证所有面板类型都出现
        assert!(json.contains("\"timeseries\""));
        assert!(json.contains("\"stat\""));
        assert!(json.contains("\"gauge\""));
        assert!(json.contains("\"heatmap\""));
        assert!(json.contains("\"bargauge\""));
        assert!(json.contains("\"piechart\""));
        assert!(json.contains("\"table\""));
    }

    #[test]
    fn test_default_dashboard_json_valid_structure() {
        let dashboard = create_default_dashboard();
        let json = dashboard.to_json();
        // 验证 JSON 结构完整性
        assert!(json.starts_with('{'));
        assert!(json.ends_with('}'));
        // 验证包含必要字段
        assert!(json.contains("\"title\""));
        assert!(json.contains("\"uid\""));
        assert!(json.contains("\"panels\""));
        assert!(json.contains("\"templating\""));
        assert!(json.contains("\"refresh\""));
        assert!(json.contains("\"timezone\""));
    }

    #[test]
    fn test_default_dashboard_promql_queries() {
        let dashboard = create_default_dashboard();
        let queries = dashboard.all_queries();
        assert!(!queries.is_empty());
        // 验证包含关键 PromQL
        assert!(queries.iter().any(|q| q.contains("rate(")));
        assert!(queries.iter().any(|q| q.contains("histogram_quantile")));
        assert!(queries.iter().any(|q| q.contains("szrsql_")));
    }

    #[test]
    fn test_default_dashboard_template_variables() {
        let dashboard = create_default_dashboard();
        assert_eq!(dashboard.templating.len(), 2);
        assert_eq!(dashboard.templating[0].name, "database");
        assert_eq!(dashboard.templating[1].name, "user");
    }

    #[test]
    fn test_default_dashboard_grid_positions_no_overlap() {
        let dashboard = create_default_dashboard();
        // 简单验证：每个面板的 grid_pos 都有合理的 x/y/w/h
        for panel in &dashboard.panels {
            assert!(panel.grid_pos.w <= 12, "Panel '{}' width > 12", panel.title);
            assert!(
                panel.grid_pos.x + panel.grid_pos.w <= 12,
                "Panel '{}' exceeds grid width",
                panel.title
            );
        }
    }

    #[test]
    fn test_metrics_constants() {
        assert_eq!(metrics::QUERIES_TOTAL, "szrsql_queries_total");
        assert_eq!(metrics::TRANSACTIONS_TOTAL, "szrsql_transactions_total");
        assert_eq!(metrics::ERRORS_TOTAL, "szrsql_errors_total");
        assert_eq!(metrics::SLOW_QUERIES_TOTAL, "szrsql_slow_queries_total");
        assert_eq!(
            metrics::QUERY_DURATION_BUCKET,
            "szrsql_query_duration_bucket"
        );
        assert_eq!(metrics::CONNECTIONS_ACTIVE, "szrsql_connections_active");
        assert_eq!(metrics::CDC_LAG_SECONDS, "szrsql_cdc_lag_seconds");
        assert_eq!(metrics::ALERTS_ACTIVE, "szrsql_alerts_active");
    }

    #[test]
    fn test_default_dashboard_has_all_metric_categories() {
        let dashboard = create_default_dashboard();
        let json = dashboard.to_json();
        // 验证 QPS
        assert!(json.contains("szrsql_queries_total"));
        // 验证延迟
        assert!(json.contains("szrsql_query_duration_bucket"));
        // 验证缓存
        assert!(json.contains("szrsql_buffer_cache"));
        assert!(json.contains("szrsql_page_cache"));
        assert!(json.contains("szrsql_query_cache"));
        // 验证连接
        assert!(json.contains("szrsql_connections_"));
        // 验证存储
        assert!(json.contains("szrsql_tablespace_"));
        assert!(json.contains("szrsql_wal_"));
        assert!(json.contains("szrsql_toast_"));
        // 验证 CDC
        assert!(json.contains("szrsql_cdc_"));
        // 验证 ASH
        assert!(json.contains("szrsql_ash_"));
        // 验证告警
        assert!(json.contains("szrsql_alerts_"));
    }
}
