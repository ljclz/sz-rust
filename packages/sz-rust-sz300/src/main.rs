mod config;
mod controllers;
mod db;
mod middleware;
mod models;
mod router;
mod services;
mod state;

use state::AppState;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    tracing::info!("鲜视达 SZ-300 后端服务启动");

    // 初始化上传目录
    services::file_service::FileService::init().await?;

    // 加载配置
    let config = config::load_config()?;

    // 初始化数据库连接池
    let pool = db::init_pool(&config).await?;
    let pg_pool = match db::init_pg_pool(&config::pg_config()).await {
        Ok(p) => {
            tracing::info!("PostgreSQL 连接池初始化成功");
            Some(Arc::new(p))
        }
        Err(e) => {
            tracing::warn!("PostgreSQL 连接池初始化失败（非致命）: {}", e);
            None
        }
    };
    let app_state = AppState {
        db_pool: Arc::new(pool),
        pg_pool,
    };

    // 初始化 JWT 认证（传入数据库连接池用于密码验证）
    services::auth_service::init_auth(
        "sz300-jwt-secret",
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
    tracing::info!("监听地址: {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
