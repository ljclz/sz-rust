// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 约定式路由 `parse_path` 演示（2026-10-25 架构评测新增）
//!
//! 运行：`cargo run -p sz-rust-examples --bin parse_path_demo`
//!
//! 演示 sz-rust 三层路由机制的 Layer 3「约定式路由」：
//! 将 `/app/controller/action` 形式的 URL 解析为 `(app, controller, action)` 三元组，
//! 并据此分发到对应处理函数（对齐 PHP `auto_multi_app` 的约定式路由）。
//!
//! 访问示例：
//! - `http://127.0.0.1:9533/oapc/customer/detail?id=3`
//! - `http://127.0.0.1:9533/oapc/order/index`
//! - `http://127.0.0.1:9533/customer/list`

use axum::extract::Path;
use axum::routing::get;
use axum::Router;
use sz_rust_core::router::parse_path;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // 约定式路由入口：所有 `/app/controller/action` 形式的请求都进入该 handler，
    // 由 parse_path 解析并分发。
    let app = Router::new().route("/{*path}", get(convention_handler));

    let addr = "127.0.0.1:9533";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("约定式路由演示启动: http://{addr}/oapc/customer/detail?id=3");
    axum::serve(listener, app).await?;
    Ok(())
}

/// 约定式路由处理器：解析路径并分发到「控制器/动作」
async fn convention_handler(Path(path): Path<String>) -> String {
    let parsed = parse_path(&path);
    tracing::info!(
        "parse_path({path:?}) => app={}, controller={}, action={}",
        parsed.app,
        parsed.controller,
        parsed.action
    );
    dispatch(
        parsed.app.as_ref(),
        parsed.controller.as_ref(),
        parsed.action.as_ref(),
    )
}

/// 简单的「控制器/动作」分发表
fn dispatch(app: &str, controller: &str, action: &str) -> String {
    match (controller, action) {
        ("Customer", "detail") => format!("[{app}] 客户详情页"),
        ("Customer", "list") => format!("[{app}] 客户列表"),
        ("Order", "index") => format!("[{app}] 订单首页"),
        _ => format!("未找到控制器 {controller} 的动作 {action}"),
    }
}
