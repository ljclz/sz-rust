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
        .unwrap_or_else(|_| EnvFilter::new("info,sz_rust_sz300=debug"));
    fmt().with_env_filter(filter).json().init();

    tracing::info!("鲜视达 SZ-300 后端服务启动");

    // 初始化上传目录
    services::file_service::FileService::init().await?;

    // 校验 JWT 配置（SZ_JWT_SECRET 未设置时 panic，阻止启动）
    // 安全铁律：认证密钥必须配置，否则所有受保护端点可被未授权访问
    sz_rust_core::controller::validate_jwt_config();

    // 加载配置（从环境变量读取，密钥不硬编码）
    let config = config::load_config()?;

    // 尝试加载框架统一 AppConfig（YAML 配置文件，可选）
    // 对齐 sz-rust-core 的 AppConfig::load_from_dir()，实现框架级配置统一
    match sz_rust_core::config::AppConfig::load_from_dir("config") {
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
    let app_state = AppState {
        db_pool: Arc::new(pool),
        pg_pool,
        metrics_registry: metrics_registry.clone(),
    };

    // 初始化 JWT 认证（传入数据库连接池用于密码验证）
    // JWT 密钥从环境变量 SZ300_JWT_SECRET 读取（生产安全要求）
    let jwt_secret = std::env::var("SZ300_JWT_SECRET")
        .expect("SZ300_JWT_SECRET 环境变量未设置 — 请在启动前设置 JWT 密钥");
    services::auth_service::init_auth(&jwt_secret, "sz300", 86400, app_state.db_pool.clone());

    // 初始化 MQTT 消费者 — 带优雅退出信号
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let app_state_clone = app_state.clone();
    let mqtt_handle = tokio::spawn(async move {
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
            // 等待 MQTT 任务完成（最多 5 秒）
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), mqtt_handle).await;
            tracing::info!("MQTT 消费者已退出，HTTP 服务器关闭中...");
        })
        .await?;

    Ok(())
}
