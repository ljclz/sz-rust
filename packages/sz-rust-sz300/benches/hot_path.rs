// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 热路径性能基准测试
//!
//! 测量 v1.8.0 中间件叠加后的热路径性能：
//! 1. /health 端点（含安全头 + CORS + CSRF 中间件链）
//! 2. /metrics 端点
//! 3. JWT 验证（含密钥轮换）
//! 4. RBAC 权限检查
//! 5. 审计链哈希计算
//! 6. SSE 事件发布
//! 7. WebSocket 连接管理
//! 8. 数据脱敏
//! 9. 安全头构建
//!
//! 运行方式:
//!   cargo bench --package sz-rust-sz300 --bench hot_path \
//!     --features "v18-security-headers,v18-data-mask,v18-rbac,v18-key-rotation,v18-audit-chain,v18-sse,v18-websocket"

use criterion::{black_box, Criterion};
use std::time::Duration;

// ============================================================================
// 辅助函数
// ============================================================================

async fn build_app_state() -> Option<sz_rust_sz300::state::AppState> {
    use std::sync::Arc;

    use sz_rust_observability::MetricsRegistry;
    use sz_rust_sz300::{config, db};

    let cfg = config::AppConfig {
        server: config::ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8300,
        },
        database: config::DatabaseConfig {
            host: "127.0.0.1".to_string(),
            port: 3306,
            username: "root".to_string(),
            password: "test123".to_string(),
            database: "sz_orm_test".to_string(),
        },
    };

    let pool = match db::init_pool(&cfg).await {
        Ok(p) => p,
        Err(_) => return None,
    };

    {
        let mut conn = pool.acquire().await.ok()?;
        conn.query("SELECT 1").await.ok()?;
    }

    Some(sz_rust_sz300::state::AppState {
        db_pool: Arc::new(pool),
        pg_pool: None,
        metrics_registry: Arc::new(MetricsRegistry::new()),
        #[cfg(feature = "v18-rbac")]
        rbac_engine: Arc::new(sz_rust_sz300::rbac::roles::init_rbac_engine()),
        #[cfg(feature = "v18-key-rotation")]
        key_manager: sz_rust_sz300::services::auth_service::default_key_manager_for_tests(),
        #[cfg(feature = "v18-audit-chain")]
        chain_auditor: Arc::new(sz_rust_middleware_facade::audit_chain::ChainHashAuditor::new()),
        #[cfg(feature = "v18-upload")]
        upload_config: sz_rust_sz300::config::upload_config(),
        #[cfg(feature = "v18-graphql")]
        graphql_schema: sz_rust_sz300::graphql::build_schema(),
        #[cfg(feature = "v18-websocket")]
        ws_manager: Arc::new(sz_rust_websocket::manager::ConnectionManager::with_defaults()),
        #[cfg(feature = "v18-websocket")]
        ws_rooms: Arc::new(sz_rust_websocket::room::RoomManager::new()),
        #[cfg(feature = "v18-sse")]
        sse_service: sz_rust_sz300::services::sse_service::SseService::with_defaults(),
    })
}

// ============================================================================
// 基准测试: /health 端点（完整中间件链）
// ============================================================================

fn bench_health_endpoint(c: &mut Criterion) {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    c.bench_function("hot_path/health_full_middleware", |b| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let state = rt.block_on(async { build_app_state().await });
        let Some(state) = state else {
            eprintln!("⚠️ MySQL 不可达，跳过 health endpoint benchmark");
            return;
        };
        let app = sz_rust_sz300::router::create_router(state);

        b.to_async(tokio::runtime::Runtime::new().unwrap())
            .iter(|| async {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri("/health")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                black_box(response);
            })
    });
}

fn bench_metrics_endpoint(c: &mut Criterion) {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    c.bench_function("hot_path/metrics_full_middleware", |b| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let state = rt.block_on(async { build_app_state().await });
        let Some(state) = state else {
            eprintln!("⚠️ MySQL 不可达，跳过 metrics endpoint benchmark");
            return;
        };
        let app = sz_rust_sz300::router::create_router(state);

        b.to_async(tokio::runtime::Runtime::new().unwrap())
            .iter(|| async {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri("/metrics")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                black_box(response);
            })
    });
}

// ============================================================================
// 基准测试: JWT 验证
// ============================================================================

fn bench_jwt_verify(c: &mut Criterion) {
    let secret = "bench-test-secret-key-00000000000000000000";
    let encoder = sz_rust_core::orm::jwt::JwtEncoder::new(secret);
    let token = encoder
        .encode(&sz_rust_core::orm::jwt::JwtClaims::new(
            "bench-user",
            9999999999,
        ))
        .expect("token 生成失败");

    let auth = sz_rust_core::orm::JwtAuthenticator::new(secret, "sz300", 86400);

    c.bench_function("hot_path/jwt_verify", |b| {
        b.iter(|| {
            let result = auth.verify_token(black_box(&token));
            let _ = black_box(result);
        })
    });
}

// ============================================================================
// 基准测试: RBAC 权限检查
// ============================================================================

#[cfg(feature = "v18-rbac")]
fn bench_rbac_check(c: &mut Criterion) {
    use sz_rust_auth_facade::{PermissionDecision, RbacEngine, Resource, RoleId};

    let engine = RbacEngine::new(10);
    let role = RoleId::new("admin");
    let resource = Resource::new("merchant", "*", "read");
    engine.grant_permission(role.clone(), resource.clone());
    engine.assign_role("bench-user", role);

    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("hot_path/rbac_check_allowed", |b| {
        b.to_async(&rt).iter(|| async {
            let result = engine
                .check(black_box("bench-user"), black_box(&resource))
                .await;
            assert!(matches!(result, Ok(PermissionDecision::Allow)));
        })
    });

    let denied_resource = Resource::new("merchant", "*", "delete");
    c.bench_function("hot_path/rbac_check_denied", |b| {
        b.to_async(&rt).iter(|| async {
            let result = engine
                .check(black_box("bench-user"), black_box(&denied_resource))
                .await;
            assert!(matches!(result, Ok(PermissionDecision::Deny)));
        })
    });
}

// ============================================================================
// 基准测试: 审计链哈希计算
// ============================================================================

#[cfg(feature = "v18-audit-chain")]
fn bench_audit_chain(c: &mut Criterion) {
    use sz_rust_middleware_facade::audit_chain::{AuditRecord, ChainHashAuditor};

    c.bench_function("hot_path/audit_chain_append", |b| {
        b.iter(|| {
            let auditor = ChainHashAuditor::new();
            let mut record = AuditRecord::new("bench-user", "create", "merchant:1", "success");
            auditor.append(&mut record);
            black_box(&record.chain_hash);
        })
    });

    let auditor = ChainHashAuditor::new();
    let mut records: Vec<AuditRecord> = Vec::with_capacity(100);
    for i in 0..100 {
        let mut r = AuditRecord::new("user", "action", format!("target:{i}"), "ok");
        auditor.append(&mut r);
        records.push(r);
    }

    c.bench_function("hot_path/audit_chain_verify_100", |b| {
        b.iter(|| {
            let result = auditor.verify_integrity(black_box(&records));
            let _ = black_box(result);
        })
    });
}

// ============================================================================
// 基准测试: SSE 事件发布
// ============================================================================

#[cfg(feature = "v18-sse")]
fn bench_sse(c: &mut Criterion) {
    let sse = sz_rust_sz300::services::sse_service::SseService::with_defaults();
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("hot_path/sse_publish", |b| {
        b.to_async(&rt).iter(|| async {
            let id = sse
                .publish("order_status", r#"{"order_no":"BENCH","status":1}"#)
                .await;
            black_box(id);
        })
    });

    c.bench_function("hot_path/sse_subscribe", |b| {
        b.to_async(&rt).iter(|| async {
            let (rx, missed) = sse.subscribe(Some(0)).await;
            black_box((rx, missed));
        })
    });
}

// ============================================================================
// 基准测试: WebSocket 连接管理
// ============================================================================

#[cfg(feature = "v18-websocket")]
fn bench_ws_connection(c: &mut Criterion) {
    use sz_rust_websocket::manager::ConnectionManager;
    use tokio::sync::mpsc;

    c.bench_function("hot_path/ws_register_unregister", |b| {
        b.iter(|| {
            let mgr = ConnectionManager::with_defaults();
            let (tx, _rx) = mpsc::channel(10);
            let conn_id = mgr.register(tx, Some("bench-user".into())).unwrap();
            mgr.unregister(&conn_id);
            black_box(conn_id);
        })
    });

    let mgr = ConnectionManager::with_defaults();
    let (tx, _rx) = mpsc::channel(10);
    let conn_id = mgr.register(tx, Some("bench-user".into())).unwrap();

    c.bench_function("hot_path/ws_contains_check", |b| {
        b.iter(|| {
            let result = mgr.contains(black_box(&conn_id));
            black_box(result);
        })
    });
}

// ============================================================================
// 基准测试: 数据脱敏
// ============================================================================

#[cfg(feature = "v18-data-mask")]
fn bench_data_mask(c: &mut Criterion) {
    use sz_rust_data_mask::{BuiltinMaskRule, MaskEngine, MaskRule, MaskScene};

    let mut engine = MaskEngine::new();
    engine.register(
        MaskScene::Response,
        MaskRule::builtin("phone", BuiltinMaskRule::Phone),
    );
    engine.register(
        MaskScene::Response,
        MaskRule::builtin("id_card", BuiltinMaskRule::IdCard),
    );

    c.bench_function("hot_path/data_mask_phone", |b| {
        b.iter(|| {
            let result = engine.mask(MaskScene::Response, "phone", black_box("13800138000"));
            let _ = black_box(result);
        })
    });

    c.bench_function("hot_path/data_mask_id_card", |b| {
        b.iter(|| {
            let result = engine.mask(
                MaskScene::Response,
                "id_card",
                black_box("110101199001011234"),
            );
            let _ = black_box(result);
        })
    });
}

// ============================================================================
// 基准测试: 安全头构建
// ============================================================================

#[cfg(feature = "v18-security-headers")]
fn bench_security_headers(c: &mut Criterion) {
    use sz_rust_security_headers::{SecurityHeadersConfig, SecurityHeadersMiddleware};

    let middleware = SecurityHeadersMiddleware::with_defaults().expect("安全头配置无效");

    c.bench_function("hot_path/security_headers_apply", |b| {
        b.iter(|| {
            let mut hdrs = Vec::new();
            middleware.apply(black_box(&mut hdrs));
            black_box(hdrs);
        })
    });

    let config = SecurityHeadersConfig::default();
    c.bench_function("hot_path/security_headers_config_validate", |b| {
        b.iter(|| {
            let result = black_box(&config).validate();
            let _ = black_box(result);
        })
    });
}

// ============================================================================
// 基准测试分组
// ============================================================================

fn main() {
    let mut criterion = Criterion::default()
        .sample_size(100)
        .warm_up_time(Duration::from_secs(2))
        .measurement_time(Duration::from_secs(5));

    bench_health_endpoint(&mut criterion);
    bench_metrics_endpoint(&mut criterion);
    bench_jwt_verify(&mut criterion);

    #[cfg(feature = "v18-rbac")]
    bench_rbac_check(&mut criterion);

    #[cfg(feature = "v18-audit-chain")]
    bench_audit_chain(&mut criterion);

    #[cfg(feature = "v18-sse")]
    bench_sse(&mut criterion);

    #[cfg(feature = "v18-websocket")]
    bench_ws_connection(&mut criterion);

    #[cfg(feature = "v18-data-mask")]
    bench_data_mask(&mut criterion);

    #[cfg(feature = "v18-security-headers")]
    bench_security_headers(&mut criterion);

    criterion.final_summary();
}
