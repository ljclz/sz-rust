// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
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
    //
    // 默认 `info`（2026-10-29 性能修复）：旧默认 `info,sz_rust_sz300=debug` 会让
    // 业务热路径的 debug! 默认全部落盘，即使已把读路径日志降到 debug! 也照样输出。
    // 排障时用 `RUST_LOG=sz_rust_sz300=debug` 显式开启（k8s 部署为 `warn,sz_rust_sz300=info`）。
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).json().init();

    tracing::info!("鲜视达 SZ-300 后端服务启动");

    // 初始化上传目录
    services::file_service::FileService::init().await?;

    // 校验 JWT 配置（2026-10-28 修复：统一使用 SZ300_JWT_SECRET 作为唯一真实来源）
    // 安全铁律：认证密钥必须配置且强度足够，否则所有受保护端点可被未授权访问
    let jwt_secret = std::env::var("SZ300_JWT_SECRET")
        .expect("SZ300_JWT_SECRET 环境变量未设置 — 请在启动前设置 JWT 密钥");
    if jwt_secret.len() < 32 {
        panic!(
            "SZ300_JWT_SECRET 强度不足：当前 {} 字节，HS256 要求 ≥ 32 字节（256 位）。\
             请使用 `openssl rand -base64 32` 生成强密钥",
            jwt_secret.len()
        );
    }
    // 同步到框架级 SZ_JWT_SECRET，保证 addon 控制器（validate_jwt_config）使用同一密钥
    std::env::set_var("SZ_JWT_SECRET", jwt_secret.clone());
    sz_rust_core::controller::validate_jwt_config();

    // 加载配置（从环境变量读取，密钥不硬编码）
    let config = config::load_config()?;

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

    // /metrics 访问控制（2026-10-28 P1-2：fail-closed）
    // 配置 SZ300_METRICS_TOKEN → Bearer 鉴权；未配置时默认拒绝，需显式 SZ300_METRICS_PUBLIC=true 才公开。
    // 生产环境若既无令牌又未显式公开 → 启动失败，避免指标端点裸奔。
    let metrics_token = std::env::var("SZ300_METRICS_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());
    let metrics_public = config::env_bool("SZ300_METRICS_PUBLIC").unwrap_or(false);
    match &metrics_token {
        Some(_) => tracing::info!("可观测性 /metrics 端点已启用 Bearer 令牌鉴权"),
        None if metrics_public => tracing::warn!(
            "SZ300_METRICS_PUBLIC=true — /metrics 端点公开可访问（仅适用于网络隔离场景）"
        ),
        None if config::is_production() => {
            anyhow::bail!(
                "生产环境必须设置 SZ300_METRICS_TOKEN（或显式 SZ300_METRICS_PUBLIC=true）— \
                 /metrics 端点拒绝匿名访问"
            );
        }
        None => tracing::warn!(
            "SZ300_METRICS_TOKEN 未设置且未开启 SZ300_METRICS_PUBLIC — /metrics 返回 401"
        ),
    }

    // v2.0.0 合并企业版：AI facade 初始化（SZ300_AI_API_KEY 驱动）
    // 设置后 /api/v1/ai/chat|embed|stream 端点可用；未设置时端点返回友好错误提示。
    if std::env::var("SZ300_AI_API_KEY")
        .map(|k| !k.is_empty())
        .unwrap_or(false)
    {
        let api_key = std::env::var("SZ300_AI_API_KEY").unwrap_or_default();
        let base_url = std::env::var("SZ300_AI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let http = Arc::new(sz_rust_ai_facade::common::AuditHttpClient::new(
            reqwest::Client::new(),
            sz_rust_ai_facade::common::RateLimitConfig::default(),
        ));
        let provider: sz_rust_ai_facade::llm::provider::ProviderRef = Arc::new(
            sz_rust_ai_facade::llm::openai::OpenAiProvider::new(api_key, base_url, http),
        );
        let mut routes = std::collections::HashMap::new();
        for m in ["gpt-4o-mini", "gpt-4o", "gpt-4.1-mini"] {
            routes.insert(m.to_string(), provider.clone());
        }
        let router =
            sz_rust_ai_facade::llm::router::ModelRouter::new(routes, "gpt-4o-mini".to_string());
        match sz_rust_ai_facade::Ai::init_default(router, None, None, None, None) {
            Ok(()) => tracing::info!("AI facade 初始化完成（/api/v1/ai/* 端点可用）"),
            Err(e) => tracing::warn!("AI facade 初始化失败: {}", e),
        }
    } else {
        tracing::info!("SZ300_AI_API_KEY 未设置 — /api/v1/ai/* 端点将返回未配置提示");
    }

    // 初始化数据库连接池
    let pool = db::init_pool(&config).await?;
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

    // 初始化 JWT 认证（传入数据库连接池用于密码验证）
    // JWT 密钥已在上方从 SZ300_JWT_SECRET 读取并校验强度
    let db_pool = Arc::new(pool);
    services::auth_service::init_auth(&jwt_secret, "sz300", 86400, db_pool.clone());

    // v1.9.0：初始化 audience 校验配置（2026-10-28 默认启用）。
    // deny_no_audience=false：兼容历史无 aud 令牌（30 天宽限），新令牌均带 aud。
    #[cfg(feature = "v19-jwt-audience")]
    {
        use sz_rust_sz300::middleware::auth_middleware::init_audience_config;
        use sz_rust_sz300::services::auth_service::AudienceConfig;
        init_audience_config(AudienceConfig {
            service_id: "sz300-api".to_string(),
            grace_period_secs: 30 * 86400,
            deny_no_audience: false,
            enabled: true,
        });
    }

    // v1.8.0 初始化密钥轮换 + 启动定时轮换任务
    #[cfg(feature = "v18-key-rotation")]
    let key_manager = {
        let km = services::auth_service::init_key_rotation();
        // 启动定时轮换：每 30 天轮换一次
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(30 * 86400);
            loop {
                tokio::time::sleep(interval).await;
                match services::auth_service::rotate_key() {
                    Ok(v) => tracing::info!("密钥轮换成功，新版本: {}", v),
                    Err(e) => tracing::error!("密钥轮换失败: {}", e),
                }
            }
        });
        km
    };

    #[cfg(feature = "v18-websocket")]
    let (ws_manager, ws_rooms) = sz_rust_sz300::services::ws_service::WsService::default_managers();

    #[cfg(feature = "v18-sse")]
    let sse_service = sz_rust_sz300::services::sse_service::SseService::with_defaults();

    #[cfg(feature = "v19-graphql-persist")]
    let graphql_schema_db = sz_rust_sz300::graphql::build_schema_with_db(db_pool.clone());

    // v1.9.0 插件管理器（spec §5.10）
    #[cfg(feature = "v19-plugin-flow")]
    let plugin_manager = {
        let marketplace = sz_rust_sz300::services::plugin_manager::InMemoryMarketplace::new();
        Arc::new(sz_rust_sz300::services::plugin_manager::PluginManager::new(
            Arc::new(marketplace),
            std::env::var("PLUGIN_PUBLIC_KEY").unwrap_or_default(),
        ))
    };

    // v1.8.0 审计落库异步缓冲（有界通道 + 后台重试；丢弃计数暴露到 /metrics）
    #[cfg(feature = "v18-audit-chain")]
    let (audit_sink, audit_rx) = sz_rust_sz300::middleware::audit_chain::AuditSink::new(Some(
        metrics_registry.register_counter(
            "sz300_audit_dropped_total",
            "Audit records dropped due to full buffer or exhausted retries",
        ),
    ));
    #[cfg(feature = "v18-audit-chain")]
    sz_rust_sz300::middleware::audit_chain::spawn_audit_worker(
        audit_sink.clone(),
        audit_rx,
        db_pool.clone(),
    );

    // v1.9.0 查询缓存（order/list COUNT 结果缓存；订单写路径整体失效）
    let query_cache = Arc::new(sz_rust_sz300::query_cache::QueryCache::with_defaults());

    let app_state = AppState {
        db_pool,
        query_cache,
        pg_pool,
        metrics_registry: metrics_registry.clone(),
        metrics_token,
        metrics_public,
        #[cfg(feature = "v18-rbac")]
        rbac_engine: Arc::new(sz_rust_sz300::rbac::roles::init_rbac_engine()),
        #[cfg(feature = "v18-key-rotation")]
        key_manager,
        #[cfg(feature = "v18-audit-chain")]
        chain_auditor: Arc::new(sz_rust_middleware_facade::audit_chain::ChainHashAuditor::new()),
        #[cfg(feature = "v18-audit-chain")]
        audit_sink,
        #[cfg(feature = "v18-upload")]
        upload_config: config::upload_config(),
        #[cfg(feature = "v18-graphql")]
        graphql_schema: sz_rust_sz300::graphql::build_schema(),
        #[cfg(feature = "v19-graphql-persist")]
        graphql_schema_db,
        #[cfg(feature = "v18-websocket")]
        ws_manager,
        #[cfg(feature = "v18-websocket")]
        ws_rooms,
        #[cfg(feature = "v18-sse")]
        sse_service,
        #[cfg(feature = "v19-plugin-flow")]
        plugin_manager,
    };

    // 初始化 MQTT 消费者 — 带优雅退出信号
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let app_state_clone = app_state.clone();
    let mqtt_handle = tokio::spawn(async move {
        services::mqtt_listener::MqttDispatcher::start_consumer(app_state_clone, shutdown_rx).await;
    });

    // 事务性 Outbox（006_outbox.sql）：订单事件与订单同事务落库。
    // 只有在配置了真实下游 `SZ300_OUTBOX_WEBHOOK_URL` 时才启动投递循环 ——
    // 未配置时保持"只写不投递"，并显式告警，避免运维把"事件已写入"误读成"事件已送出"。
    match std::env::var("SZ300_OUTBOX_WEBHOOK_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    {
        Some(url) => {
            let dispatch_pool = (*app_state.db_pool).clone();
            let _dispatcher = services::outbox::spawn_dispatcher(
                dispatch_pool,
                services::outbox::webhook_handler(url),
                std::time::Duration::from_secs(5),
            );
            // 不打印 URL：可能携带下游访问令牌
            tracing::info!("Outbox 投递已启用（HTTP Webhook，轮询间隔 5s，仅 2xx 视为送达）");
        }
        None => tracing::warn!(
            "Outbox 只写不投递：未配置 SZ300_OUTBOX_WEBHOOK_URL；\
             order.created 会持续堆积在 outbox_event，配置下游后自动开始投递"
        ),
    }

    // 注册路由
    let app = router::create_router(app_state);

    // v1.8.0 安全/协议能力默认全关告警：防止生产部署漏配 feature 导致「裸奔但自检全绿」
    #[cfg(not(any(
        feature = "v18-security-headers",
        feature = "v18-data-mask",
        feature = "v18-rbac",
        feature = "v18-key-rotation",
        feature = "v18-audit-chain",
        feature = "v18-upload",
        feature = "v18-graphql",
        feature = "v18-websocket",
        feature = "v18-sse",
    )))]
    {
        tracing::warn!(
            "未启用任何 v1.8.0 安全/协议 feature（security-headers/data-mask/rbac/key-rotation/audit-chain/upload/graphql/websocket/sse）。生产部署请使用 --features 显式启用，否则相关保护不生效。"
        );
    }

    // 启动 HTTP 服务器
    let addr = format!("{}:{}", config.server.host, config.server.port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("监听地址: {}", addr);

    // 注入 socket 对端地址：限流中间件按真实对端 IP 分桶（trust_proxy=false 时
    // 不再把所有请求并进同一个 "unknown" 桶），见 sz-rust-middleware-facade/rate_limit.rs
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
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
        // 等待 MQTT 任务完成（最多 5 秒）
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), mqtt_handle).await;
        tracing::info!("MQTT 消费者已退出，HTTP 服务器关闭中...");
    })
    .await?;

    Ok(())
}
