// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Marketplace Web 服务二进制入口

use std::net::SocketAddr;
use std::sync::Arc;

use sz_rust_marketplace::repository::{
    DeveloperRepository, PluginRepository, ReviewRepository, VersionRepository,
};
use sz_rust_marketplace::service::MarketplaceService;
use sz_rust_marketplace::storage::LocalObjectStore;
use sz_rust_marketplace::web::{AppState, JwtConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://szrust:szrust@127.0.0.1:5432/szrust_marketplace".to_string()
    });
    let jwt_secret = std::env::var("JWT_SECRET")
        .unwrap_or_else(|_| "sz-rust-marketplace-dev-secret".to_string());
    let store_root = std::env::var("OBJECT_STORE_ROOT")
        .unwrap_or_else(|_| "/www/rust/marketplace-store".to_string());
    let listen_addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    tracing::info!("连接数据库: {database_url}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    tracing::info!("创建服务组件");
    let plugins = PluginRepository::new(pool.clone());
    let versions = VersionRepository::new(pool.clone());
    let reviews = ReviewRepository::new(pool.clone());
    let developers = DeveloperRepository::new(pool.clone());

    tokio::fs::create_dir_all(&store_root).await?;
    let store = Arc::new(LocalObjectStore::new(store_root.into()));

    let service = MarketplaceService::new(plugins, versions, reviews, developers, store);
    let jwt = Arc::new(JwtConfig::new(&jwt_secret));

    let state = AppState {
        pool,
        service: Arc::new(service),
        jwt,
    };

    let app = sz_rust_marketplace::web::build_router(state);
    let addr: SocketAddr = listen_addr.parse()?;
    tracing::info!("Marketplace Web 服务启动: http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
