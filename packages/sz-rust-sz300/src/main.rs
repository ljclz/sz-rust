//! 鲜视达 SZ-300 后端服务入口

#![forbid(unsafe_code)]

use sz_rust_sz300::{config, db, router, services, state::AppState};
use std::sync::Arc;
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    tracing::info!("鲜视达 SZ-300 后端服务启动");

    // 初始化上传目录
    services::file_service::FileService::init().await?;

    // 加载配置（从环境变量读取，密钥不硬编码）
    let config = config::load_config()?;

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
    };

    // 初始化 JWT 认证（传入数据库连接池用于密码验证）
    // JWT 密钥从环境变量 SZ300_JWT_SECRET 读取（生产安全要求）
    let jwt_secret = std::env::var("SZ300_JWT_SECRET")
        .expect("SZ300_JWT_SECRET 环境变量未设置 — 请在启动前设置 JWT 密钥");
    services::auth_service::init_auth(
        &jwt_secret,
        "sz300",
        86400,
        app_state.db_pool.clone(),
    );

    // 初始化 MQTT 消费者
    let app_state_clone = app_state.clone();
    tokio::spawn(async move {
        services::mqtt_listener::MqttDispatcher::start_consumer(app_state_clone).await;
    });

    // 注册路由
    let app = router::create_router(app_state);

    // 启动 HTTP 服务器
    let addr = format!("{}:{}", config.server.host, config.server.port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("监听地址: {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
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
        })
        .await?;

    Ok(())
}