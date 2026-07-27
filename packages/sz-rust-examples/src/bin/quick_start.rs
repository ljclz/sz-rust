//! Hello World 端点验证
//!
//! 启动 axum HTTP 服务，提供 `GET /` 端点返回标准 JSON 响应：
//! `{ "code": 1, "msg": "hello", "data": {} }`
//!
//! ## 运行
//!
//! ```bash
//! cargo run -p sz-rust-examples --bin quick_start
//! ```
//!
//! 访问 http://127.0.0.1:9527/ 应返回：
//! ```json
//! {"code":1,"msg":"hello","data":{}}
//! ```

use sz_rust_core::config::AppConfig;
use sz_rust_core::container::App;
use sz_rust_core::log::LogFacade;
use sz_rust_examples::build_router;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化 tracing 日志
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 加载配置
    let config_dir = std::env::var("SZ_RUST_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("config"));
    let config = AppConfig::load_from_dir(&config_dir).unwrap_or_else(|e| {
        tracing::warn!("加载配置失败（使用默认配置）: {}", e);
        AppConfig::default()
    });

    // 初始化 App 容器
    let app = App::init(config);
    tracing::info!(
        "App 容器初始化完成，数据库连接: {:?}",
        app.db_connection_names()
    );

    // 初始化日志 facade
    let log_facade = LogFacade::init(&app.config().log);
    log_facade.info("SZ-Rust Hello World 端点启动中...");

    // 构建路由
    let router = build_router();

    // 启动 HTTP 服务
    let addr = "127.0.0.1:9527";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("HTTP 服务监听 http://{}/", addr);
    log_facade.info(&format!("HTTP 服务监听 http://{}/", addr));

    axum::serve(listener, router).await?;

    Ok(())
}
