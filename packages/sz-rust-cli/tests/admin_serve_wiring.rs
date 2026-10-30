// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! CLI 级 admin 插件接线验证测试
//!
//! 验证 `sz-rust serve --with-admin` 命令的参数解析和 Router 构建：
//! - 参数解析：--with-admin 标志、--addr 默认值
//! - build_router_with_admin：Capability 注册（17 个）、端点可达
//! - 根路由隔离：admin 路由合并不影响主应用根路由

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use clap::Parser;
use http::StatusCode;
use http_body_util::BodyExt;
use sz_rust_cli::cmd::serve::build_router_with_admin;
use sz_rust_cli::{Cli, CliCommand};
use sz_rust_middleware_facade::data_scope::DataScopeUserContext;
use sz_rust_orm_facade::tenant::context::{TenantContext, TenantResolveSource};
use sz_rust_orm_facade::{Connection, ConnectionFactory, DbError, Pool, PoolConfig, Value};
use tower::ServiceExt;

// 复用 packages/sz-rust-examples/tests/admin_wiring.rs 的 mock 模式

type QueryRows = Vec<HashMap<String, Value>>;

struct MockConnection;

impl Connection for MockConnection {
    fn execute<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(0) })
    }
    fn query<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn execute_with_params<'a>(
        &'a mut self,
        _sql: &'a str,
        _params: &'a [Value],
    ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(1) })
    }
    fn query_with_params<'a>(
        &'a mut self,
        _sql: &'a str,
        _params: &'a [Value],
    ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn begin_transaction<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn commit<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn rollback<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn is_connected(&self) -> bool {
        true
    }
    fn ping<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async { true })
    }
    fn close<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

struct MockConnectionFactory;

#[async_trait]
impl ConnectionFactory for MockConnectionFactory {
    async fn create(&self) -> Result<Box<dyn Connection>, DbError> {
        Ok(Box::new(MockConnection))
    }
}

fn make_mock_pool() -> Arc<Pool> {
    let config = PoolConfig::default();
    let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory);
    Arc::new(Pool::new(config, factory).expect("mock pool creation should not fail"))
}

fn make_super_admin_request(method: &str, uri: &str) -> http::Request<axum::body::Body> {
    let user = DataScopeUserContext::new(1)
        .with_super(true)
        .with_roles(vec!["super_admin".to_string()]);
    let tenant = TenantContext::new(0, true, TenantResolveSource::Header);
    http::Request::builder()
        .method(method)
        .uri(uri)
        .extension(user)
        .extension(tenant)
        .body(axum::body::Body::empty())
        .unwrap()
}

/// 解析 `serve --with-admin --addr 127.0.0.1:0` 参数
#[test]
fn test_parse_serve_with_admin() {
    let cli = Cli::parse_from(["sz-rust", "serve", "--with-admin", "--addr", "127.0.0.1:0"]);
    match cli.command {
        Some(CliCommand::Serve {
            with_admin, addr, ..
        }) => {
            assert!(with_admin);
            assert_eq!(addr, "127.0.0.1:0");
        }
        _ => panic!("expected Serve command"),
    }
}

/// 解析 `serve` 默认参数（with_admin=false, addr=0.0.0.0:8080）
#[test]
fn test_parse_serve_defaults() {
    let cli = Cli::parse_from(["sz-rust", "serve"]);
    match cli.command {
        Some(CliCommand::Serve {
            with_admin, addr, ..
        }) => {
            assert!(!with_admin);
            assert_eq!(addr, "0.0.0.0:8080");
        }
        _ => panic!("expected Serve command"),
    }
}

/// build_router_with_admin 注册 17 个 Capability 且 dashboard 端点可达
#[tokio::test]
async fn test_build_router_with_admin_capabilities() {
    let pool = make_mock_pool();
    let (router, cap_count) = build_router_with_admin(pool, vec!["super_admin".to_string()]);

    assert_eq!(cap_count, 17);

    let response = router
        .oneshot(make_super_admin_request("GET", "/api/admin/dashboard"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("user_count").is_some());
}

/// admin 路由合并不影响主应用根路由
#[tokio::test]
async fn test_build_router_root_isolated() {
    let (router, _) = build_router_with_admin(make_mock_pool(), vec!["super_admin".to_string()]);

    let response = router
        .oneshot(
            http::Request::builder()
                .method("GET")
                .uri("/")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
