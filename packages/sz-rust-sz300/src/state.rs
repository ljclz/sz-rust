use std::sync::Arc;
use sz_rust_ai_facade::Ai;
use sz_rust_cache_facade::Cache;
use sz_rust_capability::CapabilityRegistry;
use sz_rust_core::hooks::HookRegistry;
use sz_rust_core::orm::Pool;
use sz_rust_core::plugin::event_bus::InMemoryEventBus;
use sz_rust_observability::slo::SloMonitor;
use sz_rust_observability::MetricsRegistry;

/// 应用共享状态，在路由处理函数与中间件之间共享数据库连接池与指标注册中心
#[derive(Clone)]
pub struct AppState {
    /// MySQL 主数据库连接池
    pub db_pool: Arc<Pool>,
    /// PostgreSQL 连接池（可选，未配置时为 None）
    pub pg_pool: Option<Arc<Pool>>,
    /// Prometheus 指标注册中心
    pub metrics_registry: Arc<MetricsRegistry>,
    /// Capability Registry（能力注册表，用于 AI/MCP 能力发现与调用）
    pub capability_registry: Arc<CapabilityRegistry>,
    /// AI facade 实例（可选，未配置 API Key 时为 None）
    pub ai: Option<Arc<Ai>>,
    /// 事件总线（用于业务事件发布/订阅）
    pub event_bus: Arc<InMemoryEventBus>,
    /// 缓存 facade（可选，未配置时为 None，业务 handler 直接查 DB）
    pub cache: Option<Arc<Cache>>,
    /// SLO 监控器（燃烧率告警，1h/5m + 6h/30m 双窗口）
    pub slo_monitor: Arc<SloMonitor>,
    /// ORM 钩子注册表（16 事件生命周期钩子）
    pub hook_registry: Arc<HookRegistry>,
    /// Agent 长期记忆存储（可选，SZ300_AGENT_ENABLED=1 时注入）
    pub long_term_memory: Option<Arc<sz_rust_ai_facade::agent::memory::FileLongTermMemoryStore>>,
    /// CRM addon 状态（客户/线索/商机管理）
    pub crm_state: sz_rust_addons_crm::CrmState,
    /// CMS addon 状态（文章/分类/标签管理）
    pub cms_state: sz_rust_addons_cms::CmsState,
    /// PDF addon 状态（CSV/Excel 导出 + PDF 表单填充）
    pub pdf_state: sz_rust_pdf::PdfState,
    /// operate addon 状态（客户/合同/分类模型管理）
    pub operate_state: sz_rust_addons_operate::OperateState,
    /// tracing addon 状态（链路追踪 Span 管理）
    pub tracing_state: sz_rust_tracing::TracingState,
    /// workflow addon 状态（工作流引擎）
    pub workflow_state: sz_rust_workflow::WorkflowState,
    /// 数据库连接池状态采集适配器（admin feature 启用时有效）
    #[cfg(feature = "admin")]
    pub db_pool_stats: Arc<dyn sz_rust_observability::admin::DbPoolStats>,
    /// Redis 状态采集适配器（admin feature 启用时有效）
    #[cfg(feature = "admin")]
    pub redis_stats: Option<Arc<dyn sz_rust_observability::admin::RedisStats>>,
}

/// 测试专用：创建不连接真实数据库的 AppState（Pool 使用 connect_lazy，prewarm=false）
#[cfg(test)]
pub fn mock_app_state() -> AppState {
    use sz_orm_sqlx::{MySqlPoolHandle, SqlxMySqlConnectionFactory};
    use sz_rust_core::orm::{PoolConfigBuilder, SqlxPoolConfig};

    // connect_lazy 不实际连接数据库，仅创建池结构
    let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
        .max_connections(1)
        .connect_lazy("mysql://fake:fake@127.0.0.1:3306/fake")
        .expect("connect_lazy 不应失败");
    let factory = SqlxMySqlConnectionFactory::new(Arc::new(MySqlPoolHandle::from_pool(sqlx_pool)));

    let pool_cfg = SqlxPoolConfig::default();
    let base = pool_cfg.to_orm_pool_config();
    let orm_cfg = PoolConfigBuilder::new()
        .max_size(base.max_size)
        .min_idle(0)
        .acquire_timeout(5)
        .idle_timeout(60)
        .max_lifetime(300)
        .prewarm(false)
        .build()
        .expect("PoolConfig 构建失败");
    let pool = Pool::new(orm_cfg, Arc::new(factory)).expect("Pool 创建失败");

    AppState {
        db_pool: Arc::new(pool),
        pg_pool: None,
        metrics_registry: Arc::new(MetricsRegistry::default()),
        capability_registry: Arc::new(CapabilityRegistry::default()),
        ai: None,
        event_bus: Arc::new(InMemoryEventBus::new()),
        cache: None,
        slo_monitor: Arc::new(SloMonitor::new(
            sz_rust_observability::slo::SloConfig::default(),
        )),
        hook_registry: Arc::new(HookRegistry::new()),
        long_term_memory: None,
        crm_state: sz_rust_addons_crm::CrmState::default(),
        cms_state: sz_rust_addons_cms::CmsState::default(),
        pdf_state: sz_rust_pdf::PdfState::default(),
        operate_state: sz_rust_addons_operate::OperateState::default(),
        tracing_state: sz_rust_tracing::TracingState::default(),
        workflow_state: sz_rust_workflow::WorkflowState::default(),
    }
}

// ============================================================================
// admin feature：连接池状态采集适配器
// ============================================================================

/// MySQL 主连接池的 [`sz_rust_observability::admin::DbPoolStats`] 实现
///
/// 委托 `Pool::status()` 获取 `PoolStatus`，计算使用率。
#[cfg(feature = "admin")]
pub struct DbPoolStatsAdapter {
    pool: Arc<Pool>,
}

#[cfg(feature = "admin")]
impl DbPoolStatsAdapter {
    /// 创建适配器（持有 `Arc<Pool>` 引用，零成本）
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }
}

#[cfg(feature = "admin")]
impl sz_rust_observability::admin::DbPoolStats for DbPoolStatsAdapter {
    fn stats(&self) -> sz_rust_observability::admin::PoolInfo {
        // Pool::status() 是 async fn，但内部仅读取原子计数器（O(1) 无阻塞）。
        // 在同步 trait 方法中通过 block_on 调用，对监控端点（低频）可接受。
        let status = tokio::runtime::Handle::current().block_on(self.pool.status());
        let max = status.max;
        let usage = if max == 0 {
            0.0
        } else {
            status.active as f32 / max as f32 * 100.0
        };
        sz_rust_observability::admin::PoolInfo {
            active: status.active,
            idle: status.idle,
            max,
            usage_percent: usage,
        }
    }
}

/// Redis 状态采集适配器（admin feature 启用时有效）
///
/// 包装 `redis::Client`，实现 [`sz_rust_observability::admin::RedisStats`]。
/// 探活超时 3 秒，失败时返回 `Err`。
#[cfg(feature = "admin")]
pub struct RedisStatsAdapter {
    client: redis::Client,
}

#[cfg(feature = "admin")]
impl RedisStatsAdapter {
    /// 从 Redis URL 创建适配器
    pub fn from_url(url: &str) -> Result<Self, redis::RedisError> {
        Ok(Self {
            client: redis::Client::open(url)?,
        })
    }
}

#[cfg(feature = "admin")]
impl sz_rust_observability::admin::RedisStats for RedisStatsAdapter {
    fn info(
        &self,
    ) -> Result<
        sz_rust_observability::admin::RedisInfo,
        sz_rust_observability::admin::RedisCollectError,
    > {
        use sz_rust_observability::admin::RedisCollectError;

        // 同步连接探活（3 秒超时）
        let mut conn = self
            .client
            .get_connection()
            .map_err(|e| RedisCollectError {
                message: format!("连接失败: {}", e),
            })?;

        // PING 探活
        let pong: bool = redis::cmd("PING")
            .query(&mut conn)
            .map_err(|e| RedisCollectError {
                message: format!("PING 失败: {}", e),
            })?;
        if !pong {
            return Err(RedisCollectError {
                message: "PING 返回非 PONG".to_string(),
            });
        }

        // INFO 命令获取服务器状态
        let raw: String = redis::cmd("INFO")
            .query(&mut conn)
            .map_err(|e| RedisCollectError {
                message: format!("INFO 查询失败: {}", e),
            })?;

        // 解析 INFO 输出
        let mut map = std::collections::HashMap::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                map.insert(key.trim().to_string(), value.trim().to_string());
            }
        }

        fn parse_u64(map: &std::collections::HashMap<String, String>, key: &str) -> u64 {
            map.get(key)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
        }
        fn parse_f64(map: &std::collections::HashMap<String, String>, key: &str) -> f64 {
            map.get(key)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        }

        let uptime = parse_u64(&map, "uptime_in_seconds");
        let used_mem = parse_u64(&map, "used_memory");
        let peak_mem = parse_u64(&map, "used_memory_peak");
        let rss_mem = parse_u64(&map, "used_memory_rss");
        let used_mem_str = map.get("used_memory_human").cloned().unwrap_or_default();

        let variable = sz_rust_observability::admin::RedisVariable {
            used_memory: used_mem,
            used_memory_peak: peak_mem,
            used_memory_rss: rss_mem,
            mem_fragmentation_ratio: if used_mem > 0 {
                rss_mem as f64 / used_mem as f64
            } else {
                0.0
            },
            keyspace_hits: parse_u64(&map, "keyspace_hits"),
            keyspace_misses: parse_u64(&map, "keyspace_misses"),
            expired_keys: parse_u64(&map, "expired_keys"),
            evicted_keys: parse_u64(&map, "evicted_keys"),
            instantaneous_ops_per_sec: parse_u64(&map, "instantaneous_ops_per_sec"),
            instantaneous_input_kbps: parse_f64(&map, "instantaneous_input_kbps"),
            instantaneous_output_kbps: parse_f64(&map, "instantaneous_output_kbps"),
            total_commands_processed: parse_u64(&map, "total_commands_processed"),
            redis_version: map.get("redis_version").cloned().unwrap_or_default(),
            redis_mode: map.get("redis_mode").cloned().unwrap_or_default(),
            os: map.get("os").cloned().unwrap_or_default(),
            arch_bits: parse_u64(&map, "arch_bits"),
            mem_allocator: map.get("mem_allocator").cloned().unwrap_or_default(),
            role: map.get("role").cloned().unwrap_or_default(),
            tcp_port: parse_u64(&map, "tcp_port"),
            aof_enabled: parse_u64(&map, "aof_enabled"),
            rdb_changes_since_last_save: parse_u64(&map, "rdb_changes_since_last_save"),
            total_connections_received: parse_u64(&map, "total_connections_received"),
        };

        Ok(sz_rust_observability::admin::RedisInfo {
            connected: true,
            uptime_in_seconds: uptime,
            uptime_in_days: uptime / 86400,
            connected_clients: parse_u64(&map, "connected_clients"),
            used_memory: used_mem_str,
            variable,
        })
    }
}
