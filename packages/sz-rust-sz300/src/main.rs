//! 鲜视达 SZ-300 后端服务入口

#![forbid(unsafe_code)]

use std::sync::Arc;
use sz_rust_observability::MetricsRegistry;
use sz_rust_sz300::{config, db, router, services, state::AppState};
use tokio::signal;
use tokio::sync::watch;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志 — EnvFilter + JSON 格式（生产环境友好）
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,sz_rust_sz300=info"));
    fmt().with_env_filter(filter).json().init();

    tracing::info!("鲜视达 SZ-300 后端服务启动");

    // 初始化上传目录
    services::file_service::FileService::init().await?;

    // 校验 JWT 配置（SZ_JWT_SECRET 未设置时 panic，阻止启动）
    // 安全铁律：认证密钥必须配置，否则所有受保护端点可被未授权访问
    sz_rust_core::controller::validate_jwt_config();

    // 加载配置（从环境变量读取，密钥不硬编码）
    let config = config::load_config()?;

    // 加载优雅关闭配置
    let shutdown_config = config::ShutdownConfig::from_env();
    tracing::info!(
        "优雅关闭配置: shutdown_timeout={:?}, mqtt_timeout={:?}, force_abort={}",
        shutdown_config.shutdown_timeout,
        shutdown_config.mqtt_timeout(),
        shutdown_config.force_abort_on_timeout
    );

    // 尝试加载框架统一 AppConfig（YAML 配置文件，可选）
    // 对齐 sz-rust-core 的 AppConfig::load_from_dir()，实现框架级配置统一
    match sz_rust_core::config::AppConfig::load_from_dir("config").await {
        Ok(framework_config) => {
            tracing::info!("框架统一 AppConfig 加载成功（config/ 目录）");
            // 框架配置可用于后续框架级功能（缓存、插件、日志等）
            let _ = framework_config;
        }
        Err(e) => {
            tracing::warn!(
                "框架统一 AppConfig 加载失败（非致命，使用环境变量配置）: {}",
                e
            );
        }
    }

    // 初始化 Addon 热加载器（条件编译：启用 hot-reload feature 时生效）
    // 扫描 addons/ 目录中的 .dll/.so/.dylib 文件，动态加载并调用 addon_init 入口
    // 安全说明：libloading 的 unsafe 已收敛于 sz_rust_core::runtime::hot_reload 内部
    #[cfg(feature = "hot-reload")]
    {
        use sz_rust_core::runtime::hot_reload::HotAddonLoader;
        let mut loader = HotAddonLoader::new();
        loader.add_scan_dir("addons");
        let results = loader.scan().await;
        let loaded: Vec<_> = results
            .iter()
            .filter_map(
                |(name, r)| {
                    if r.is_ok() {
                        Some(name.clone())
                    } else {
                        None
                    }
                },
            )
            .collect();
        let failed: Vec<_> = results
            .iter()
            .filter_map(|(name, r)| {
                if let Err(e) = r {
                    Some(format!("{}: {}", name, e))
                } else {
                    None
                }
            })
            .collect();
        if loaded.is_empty() {
            tracing::info!("Addon 热加载：addons/ 目录中未找到共享库（.dll/.so/.dylib）");
        } else {
            tracing::info!(
                "Addon 热加载已启用，已加载 {} 个插件: {:?}",
                loaded.len(),
                loaded
            );
        }
        if !failed.is_empty() {
            tracing::warn!(
                "Addon 热加载：{} 个插件加载失败: {:?}",
                failed.len(),
                failed
            );
        }
    }
    #[cfg(not(feature = "hot-reload"))]
    {
        tracing::info!("Addon 热加载未启用（如需动态插件加载，启用 hot-reload feature）");
    }

    // 初始化 OTLP 分布式追踪（条件编译：启用 otlp / otlp-http feature 时生效）
    // 配置通过 OTEL_* 环境变量传入（对齐 OpenTelemetry 规范）
    #[cfg(feature = "otlp")]
    {
        let otlp_config = sz_rust_observability::otlp::OtlpConfig::from_env();
        match sz_rust_observability::otlp::init_otlp_tracer(&otlp_config) {
            Ok(()) => tracing::info!("OTLP 分布式追踪已启用（gRPC，端口 4317）"),
            Err(e) => tracing::warn!("OTLP 初始化失败（非致命，继续运行）: {}", e),
        }
    }
    #[cfg(not(feature = "otlp"))]
    {
        tracing::info!("OTLP 未启用（如需分布式追踪，启用 otlp feature）");
    }

    // 初始化可观测性 — Prometheus 指标注册中心
    let metrics_registry = Arc::new(MetricsRegistry::new());
    metrics_registry.register_counter("sz300_requests_total", "Total HTTP requests received");
    metrics_registry.register_gauge("sz300_active_connections", "Active database connections");
    metrics_registry.register_histogram(
        "sz300_request_duration_seconds",
        "HTTP request duration in seconds",
        vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0],
    );
    tracing::info!("可观测性模块初始化完成（Prometheus /metrics 端点已启用）");

    // 初始化 Capability Registry（能力注册表，用于 AI/MCP 能力发现与调用）
    let capability_registry = Arc::new(sz_rust_capability::CapabilityRegistry::new());
    tracing::info!("Capability Registry 初始化完成");

    // 初始化 AI facade（可选，从环境变量读取 API Key）
    // 若 SZ300_AI_API_KEY 未设置，AI 功能降级（/api/v1/ai/chat 返回 503）
    let ai = match std::env::var("SZ300_AI_API_KEY") {
        Ok(_api_key) => {
            tracing::info!("AI facade 初始化跳过（需配置 Provider，当前仅标记为可用）");
            None
        }
        Err(_) => {
            tracing::info!(
                "AI facade 未配置（SZ300_AI_API_KEY 未设置，/api/v1/ai/chat 将返回降级响应）"
            );
            None
        }
    };

    // 初始化事件总线（用于业务事件发布/订阅，如 order.created）
    let event_bus = Arc::new(sz_rust_core::plugin::event_bus::InMemoryEventBus::new());
    tracing::info!("事件总线初始化完成（InMemoryEventBus）");

    // 初始化缓存 facade（可选，默认使用内存驱动）
    // 若需 Redis 驱动，设置 SZ300_REDIS_URL 环境变量
    let cache = {
        let cache = sz_rust_cache_facade::Cache::new();
        cache.register_default(sz_rust_cache_facade::MemoryCacheDriver::new());
        Some(Arc::new(cache))
    };
    tracing::info!("缓存 facade 初始化完成（MemoryCacheDriver）");

    // 初始化 SLO 监控器（Google SRE 推荐：1h/5m Page + 6h/30m Ticket 双窗口）
    let slo_monitor = Arc::new(sz_rust_observability::slo::SloMonitor::new(
        sz_rust_observability::slo::SloConfig::default(),
    ));
    tracing::info!("SLO 监控器初始化完成（target=99.9%, Page=1h/5m, Ticket=6h/30m）");

    // 初始化 ORM 钩子注册表（16 事件生命周期钩子，对齐 PHP think-orm Model 钩子）
    let hook_registry = Arc::new(sz_rust_core::hooks::HookRegistry::new());
    tracing::info!("ORM 钩子注册表初始化完成（16 事件）");

    // 初始化数据库连接池
    let pool = Arc::new(db::init_pool(&config).await?);
    let pg_pool = match config::pg_config() {
        Ok(pg_cfg) => match db::init_pg_pool(&pg_cfg).await {
            Ok(p) => {
                tracing::info!("PostgreSQL 连接池初始化成功");
                Some(Arc::new(p))
            }
            Err(e) => {
                tracing::warn!("PostgreSQL 连接池初始化失败（非致命）: {}", e);
                None
            }
        },
        Err(e) => {
            tracing::warn!("PostgreSQL 配置加载失败（非致命）: {}", e);
            None
        }
    };
    let app_state = AppState {
        db_pool: pool.clone(),
        pg_pool,
        metrics_registry: metrics_registry.clone(),
        capability_registry: capability_registry.clone(),
        ai,
        event_bus,
        cache,
        slo_monitor,
        hook_registry,
        #[cfg(feature = "admin")]
        db_pool_stats: Arc::new(
            sz_rust_sz300::state::DbPoolStatsAdapter::new(pool.clone())
        ) as Arc<dyn sz_rust_observability::admin::DbPoolStats>,
        #[cfg(feature = "admin")]
        redis_stats: std::env::var("ADMIN_REDIS_URL")
            .ok()
            .and_then(|url| match sz_rust_sz300::state::RedisStatsAdapter::from_url(&url) {
                Ok(adapter) => {
                    Some(Arc::new(adapter) as Arc<dyn sz_rust_observability::admin::RedisStats>)
                }
                Err(e) => {
                    tracing::warn!("Admin Redis 适配器初始化失败（非致命，/api/admin/redis/info 将返回降级响应）: {}", e);
                    None
                }
            }),
    };

    // 初始化 JWT 认证（传入数据库连接池用于密码验证）
    // JWT 密钥从环境变量 SZ300_JWT_SECRET 读取（生产安全要求）
    let jwt_secret = std::env::var("SZ300_JWT_SECRET")
        .expect("SZ300_JWT_SECRET 环境变量未设置 — 请在启动前设置 JWT 密钥");
    services::auth_service::init_auth(&jwt_secret, "sz300", 86400, app_state.db_pool.clone());

    // 初始化 MQTT 消费者 — 带优雅退出信号
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let app_state_clone = app_state.clone();
    let mut mqtt_handle = tokio::spawn(async move {
        services::mqtt_listener::MqttDispatcher::start_consumer(app_state_clone, shutdown_rx).await;
    });

    // 注册路由
    let app = router::create_router(app_state);

    // 启动 HTTP 服务器
    let addr = format!("{}:{}", config.server.host, config.server.port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("监听地址: {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let ctrl_c = async {
                signal::ctrl_c()
                    .await
                    .expect("failed to install Ctrl+C handler");
            };

            #[cfg(unix)]
            let terminate = async {
                signal::unix::signal(signal::unix::SignalKind::terminate())
                    .expect("failed to install signal handler")
                    .recv()
                    .await;
            };

            #[cfg(not(unix))]
            let terminate = std::future::pending::<()>();

            tokio::select! {
                _ = ctrl_c => {},
                _ = terminate => {},
            }

            tracing::info!("收到关闭信号，正在优雅关闭...");
            // 通知 MQTT 消费者退出
            let _ = shutdown_tx.send(true);
            // 等待 MQTT 任务完成（可配置超时）
            let mqtt_timeout = shutdown_config.mqtt_timeout();
            match tokio::time::timeout(mqtt_timeout, &mut mqtt_handle).await {
                Ok(Ok(())) => {
                    tracing::info!("MQTT 消费者已正常退出，HTTP 服务器关闭中...");
                }
                Ok(Err(e)) => {
                    tracing::error!("MQTT 消费者退出异常: {e:?}，HTTP 服务器关闭中...");
                }
                Err(_) => {
                    if shutdown_config.force_abort_on_timeout {
                        tracing::warn!(
                            "MQTT_CONSUMER_FORCE_QUIT: MQTT 消费者在 {:?} 内未退出，强制中止",
                            mqtt_timeout
                        );
                        mqtt_handle.abort();
                    } else {
                        tracing::warn!(
                            "MQTT 消费者在 {:?} 内未退出，继续关闭 HTTP 服务器",
                            mqtt_timeout
                        );
                    }
                }
            }
        })
        .await?;

    Ok(())
}
