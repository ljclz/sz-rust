// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use crate::controllers::{auth, device, file, file_serve, health, merchant, order, product};
use crate::middleware::auth_middleware;
use crate::openapi;
use crate::state::AppState;
use axum::{middleware, routing::get, routing::post, Router};
use sz_rust_core::middleware::cors::cors_layer;
use sz_rust_core::middleware::csrf::csrf_middleware;

/// 公开路径白名单 — 跳过 JWT 鉴权的路径（精确匹配，避免前缀绕过）
///
/// 安全说明（2026-07-26 P1 修复）：
/// - 旧版使用 `path.starts_with("/api/v1/auth/")` 前缀匹配，会绕过 `/api/v1/auth/me`、
///   `/api/v1/auth/logout` 等需要鉴权的接口
/// - 新版改为精确匹配，仅 `/api/v1/auth/login` 与 `/api/v1/auth/refresh` 跳过鉴权
pub const PUBLIC_PATHS: &[&str] = &[
    "/health",
    "/health/ready",
    "/health/startup",
    "/metrics",
    "/api/v1/auth/login",
    "/api/v1/auth/refresh",
    "/api-docs",
    "/api-docs/redoc",
    "/api-docs/openapi.json",
    "/ws",
    "/events",
];

/// 判断路径是否在公开白名单中（精确匹配，避免前缀绕过）
pub fn is_public_path(path: &str) -> bool {
    PUBLIC_PATHS.contains(&path)
}

/// 创建应用路由表，注册所有业务路由并叠加 CORS + CSRF + JWT 鉴权中间件
///
/// ## 中间件执行顺序（外层→内层）
///
/// 1. `cors_layer`：CORS 跨域处理（最外层，确保预检请求直接返回）
/// 2. `csrf_middleware`：CSRF 双提交 Cookie 校验（公开路径自动放行）
/// 3. `auth_middleware`：JWT 校验（公开路径自动放行）
/// 4. 业务 handler
///
/// 在 axum/tower 中，`.layer(A).layer(B)` 的执行顺序为 B → A → handler。
/// 因此此处注册顺序为 auth_middleware 在前（内层），csrf_middleware 在后（外层），
/// cors_layer 最后注册（最外层）。
pub fn create_router(state: AppState) -> Router {
    // 公开路由（不脱敏：健康检查、指标、API 文档、静态文件）
    let public_routes = Router::new()
        .route("/api-docs", get(openapi::swagger_ui))
        .route("/api-docs/redoc", get(openapi::redoc))
        .route("/api-docs/openapi.json", get(openapi::openapi_json))
        .route("/health", get(health::check))
        .route("/health/ready", get(health::readiness))
        .route("/health/startup", get(health::startup))
        .route("/metrics", get(health::metrics))
        .route("/uploads/{*path}", get(file_serve::serve_file));

    // API 路由（v1.8.0 启用脱敏）
    #[cfg(not(feature = "v18-rbac"))]
    let api_routes = Router::new()
        // 认证（公开接口）
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/refresh", post(auth::refresh))
        .route("/api/v1/auth/me", post(auth::me))
        .route("/api/v1/auth/logout", post(auth::logout))
        // 商户管理
        .route("/api/v1/merchant/list", post(merchant::list))
        .route("/api/v1/merchant/info", post(merchant::info))
        .route("/api/v1/merchant/create", post(merchant::create))
        .route("/api/v1/merchant/update", post(merchant::update))
        .route("/api/v1/merchant/delete", post(merchant::delete))
        // 商品管理
        .route("/api/v1/product/list", post(product::list))
        .route("/api/v1/product/info", post(product::info))
        .route("/api/v1/product/create", post(product::create))
        .route("/api/v1/product/update", post(product::update))
        .route("/api/v1/product/delete", post(product::delete))
        // 设备管理
        .route("/api/v1/device/list", post(device::list))
        .route("/api/v1/device/info", post(device::info))
        .route("/api/v1/device/bind", post(device::bind))
        .route("/api/v1/device/unbind", post(device::unbind))
        .route("/api/v1/device/ota", post(device::trigger_ota))
        .route("/api/v1/device/status_report", post(device::status_report))
        // 订单管理
        .route("/api/v1/order/list", post(order::list))
        .route("/api/v1/order/info", post(order::info))
        .route("/api/v1/order/create", post(order::create))
        // 文件上传
        .route("/api/v1/file/upload", post(file::upload))
        .route(
            "/api/v1/file/upload_multipart",
            post(file::upload_multipart),
        );

    // v1.8.0 增强上传端点（非 RBAC 模式，带大小限制中间件）
    #[cfg(all(feature = "v18-upload", not(feature = "v18-rbac")))]
    let api_routes = {
        let upload_max = std::sync::Arc::new(state.upload_config.max_size);
        api_routes.route(
            "/api/v1/file/upload_enhanced",
            post(file::upload_enhanced).layer(middleware::from_fn_with_state(
                upload_max,
                crate::middleware::upload_limit::upload_limit_middleware,
            )),
        )
    };

    // v1.8.0 GraphQL 端点（非 RBAC 模式）
    #[cfg(all(feature = "v18-graphql", not(feature = "v18-rbac")))]
    let api_routes = api_routes.route("/graphql", post(graphql_handler));

    // v1.8.0 WebSocket 端点
    #[cfg(feature = "v18-websocket")]
    let public_routes = public_routes.route("/ws", get(ws_handler));

    // v1.8.0 SSE 端点
    #[cfg(feature = "v18-sse")]
    let public_routes = public_routes.route("/events", get(sse_handler));

    // v1.8.0 RBAC：按权限分组路由，每组挂载 RbacGuard
    #[cfg(feature = "v18-rbac")]
    let api_routes = {
        use crate::rbac::{guard::rbac_guard, roles::perm};
        use axum::middleware;

        let engine = state.rbac_engine.clone();

        // 认证路由（无 RBAC）
        let auth_routes = Router::new()
            .route("/api/v1/auth/login", post(auth::login))
            .route("/api/v1/auth/refresh", post(auth::refresh))
            .route("/api/v1/auth/me", post(auth::me))
            .route("/api/v1/auth/logout", post(auth::logout));

        // 商户读路由
        let merchant_read = Router::new()
            .route("/api/v1/merchant/list", post(merchant::list))
            .route("/api/v1/merchant/info", post(merchant::info))
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::merchant("read")),
                rbac_guard,
            ));

        // 商户写路由
        let merchant_write = Router::new()
            .route("/api/v1/merchant/create", post(merchant::create))
            .route("/api/v1/merchant/update", post(merchant::update))
            .route("/api/v1/merchant/delete", post(merchant::delete))
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::merchant("create")),
                rbac_guard,
            ));

        // 商品读路由
        let product_read = Router::new()
            .route("/api/v1/product/list", post(product::list))
            .route("/api/v1/product/info", post(product::info))
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::product("read")),
                rbac_guard,
            ));

        // 商品写路由
        let product_write = Router::new()
            .route("/api/v1/product/create", post(product::create))
            .route("/api/v1/product/update", post(product::update))
            .route("/api/v1/product/delete", post(product::delete))
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::product("create")),
                rbac_guard,
            ));

        // 设备读路由
        let device_read = Router::new()
            .route("/api/v1/device/list", post(device::list))
            .route("/api/v1/device/info", post(device::info))
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::device("read")),
                rbac_guard,
            ));

        // 设备管理路由
        let device_manage = Router::new()
            .route("/api/v1/device/bind", post(device::bind))
            .route("/api/v1/device/unbind", post(device::unbind))
            .route("/api/v1/device/ota", post(device::trigger_ota))
            .route("/api/v1/device/status_report", post(device::status_report))
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::device("bind")),
                rbac_guard,
            ));

        // 订单读路由
        let order_read = Router::new()
            .route("/api/v1/order/list", post(order::list))
            .route("/api/v1/order/info", post(order::info))
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::order("read")),
                rbac_guard,
            ));

        // 订单创建路由
        let order_create = Router::new()
            .route("/api/v1/order/create", post(order::create))
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::order("create")),
                rbac_guard,
            ));

        // 文件上传路由
        let file_routes = Router::new()
            .route("/api/v1/file/upload", post(file::upload))
            .route(
                "/api/v1/file/upload_multipart",
                post(file::upload_multipart),
            )
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::file("upload")),
                rbac_guard,
            ));

        // v1.8.0 增强上传路由（RBAC + 大小限制）
        #[cfg(feature = "v18-upload")]
        let file_routes = {
            let upload_max = std::sync::Arc::new(state.upload_config.max_size);
            file_routes.route(
                "/api/v1/file/upload_enhanced",
                post(file::upload_enhanced)
                    .layer(middleware::from_fn_with_state(
                        upload_max,
                        crate::middleware::upload_limit::upload_limit_middleware,
                    ))
                    .layer(middleware::from_fn_with_state(
                        (engine.clone(), perm::file("upload")),
                        rbac_guard,
                    )),
            )
        };

        // v1.8.0 GraphQL 端点（RBAC 模式，使用 merchant:read 权限）
        #[cfg(feature = "v18-graphql")]
        let graphql_route = Router::new()
            .route("/graphql", post(graphql_handler))
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::merchant("read")),
                rbac_guard,
            ));

        #[cfg(feature = "v18-graphql")]
        let merged = auth_routes
            .merge(merchant_read)
            .merge(merchant_write)
            .merge(product_read)
            .merge(product_write)
            .merge(device_read)
            .merge(device_manage)
            .merge(order_read)
            .merge(order_create)
            .merge(file_routes)
            .merge(graphql_route);
        #[cfg(not(feature = "v18-graphql"))]
        let merged = auth_routes
            .merge(merchant_read)
            .merge(merchant_write)
            .merge(product_read)
            .merge(product_write)
            .merge(device_read)
            .merge(device_manage)
            .merge(order_read)
            .merge(order_create)
            .merge(file_routes);

        merged
    };

    // v1.8.0 数据脱敏中间件（仅对 API 路由生效，/health /metrics 不脱敏）
    #[cfg(feature = "v18-data-mask")]
    let api_routes = api_routes.layer(sz_rust_data_mask::DataMaskLayer::new(
        crate::config::mask_engine(),
        sz_rust_data_mask::MaskScene::Response,
    ));

    // v1.8.0 审计链式哈希中间件（仅 POST/PUT/DELETE，排除 /health /metrics）
    #[cfg(feature = "v18-audit-chain")]
    let chain_auditor = state.chain_auditor.clone();

    let router = public_routes
        .merge(api_routes)
        // JWT 鉴权中间件（公开路径自动跳过）— 内层
        .layer(middleware::from_fn(auth_middleware::auth_middleware))
        // CSRF 防护中间件（双提交 Cookie 模式）
        .layer(middleware::from_fn(csrf_middleware))
        // CORS 跨域中间件（最外层）
        .layer(cors_layer())
        .with_state(state);

    // v1.8.0 审计链式哈希中间件（仅 POST/PUT/DELETE，排除 /health /metrics）
    #[cfg(feature = "v18-audit-chain")]
    let router = router.layer(middleware::from_fn_with_state(
        chain_auditor,
        crate::middleware::audit_chain::audit_chain_middleware,
    ));

    // v1.8.0 安全头中间件（feature gate 控制，默认不启用）
    // 注入 CSP / HSTS / X-Frame-Options / X-Content-Type-Options / Referrer-Policy
    // 放在最外层，确保所有响应（含错误响应）都携带安全头
    #[cfg(feature = "v18-security-headers")]
    {
        let sec_config = crate::config::security_headers_config();
        match sz_rust_security_headers::security_headers_layer(sec_config) {
            Ok(layer) => router.layer(layer),
            Err(e) => {
                tracing::error!("安全头配置无效，跳过安全头中间件: {}", e);
                router
            }
        }
    }
    #[cfg(not(feature = "v18-security-headers"))]
    {
        router
    }
}
/// v1.8.0 GraphQL handler（DB-backed 模式，v1.9.0+）
#[cfg(all(feature = "v18-graphql", feature = "v19-graphql-persist"))]
async fn graphql_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: async_graphql_axum::GraphQLRequest,
) -> async_graphql_axum::GraphQLResponse {
    state
        .graphql_schema_db
        .execute(request.into_inner())
        .await
        .into()
}

/// v1.8.0 GraphQL handler（内存模式，v1.8.0）
#[cfg(all(feature = "v18-graphql", not(feature = "v19-graphql-persist")))]
async fn graphql_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: async_graphql_axum::GraphQLRequest,
) -> async_graphql_axum::GraphQLResponse {
    state
        .graphql_schema
        .execute(request.into_inner())
        .await
        .into()
}
/// v1.8.0 WebSocket handler
#[cfg(feature = "v18-websocket")]
async fn ws_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let mgr = state.ws_manager.clone();
    let rooms = state.ws_rooms.clone();

    ws.on_upgrade(move |socket| async move {
        use axum::extract::ws::Message;
        use futures::SinkExt;
        use futures::StreamExt;

        let (mut sender, mut receiver) = socket.split();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(100);
        let conn_id = match mgr.register(tx, None) {
            Ok(id) => id,
            Err(_) => return,
        };
        let _ = crate::services::ws_service::WsService::join_admin_room(&rooms, &conn_id);

        let mgr_send = mgr.clone();
        let conn_id_send = conn_id.clone();
        let send_task = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sender.send(Message::Text(msg.into())).await.is_err() {
                    break;
                }
            }
        });

        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let _ = mgr.touch(&conn_id);
                    if text.as_str() == "ping" {
                        let _ = mgr.send_to(&conn_id, "pong").await;
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }

        mgr_send.unregister(&conn_id_send);
        send_task.abort();
    })
    .into_response()
}
/// v1.8.0 SSE handler
#[cfg(feature = "v18-sse")]
async fn sse_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::response::sse::Sse<
    impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, Sse};
    use futures::stream::{self, StreamExt};

    let last_event_id = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let (receiver, missed) = state.sse_service.subscribe(last_event_id).await;

    let missed_stream = stream::iter(missed.into_iter().map(|e| {
        Ok(Event::default()
            .id(e.id.to_string())
            .event(e.event_type)
            .data(e.data))
    }));

    let live_stream = stream::unfold(receiver, |mut rx| async move {
        match rx.recv().await {
            Ok(event) => Some((
                Ok(Event::default()
                    .id(event.id.to_string())
                    .event(event.event_type)
                    .data(event.data)),
                rx,
            )),
            Err(_) => None,
        }
    });

    Sse::new(missed_stream.chain(live_stream)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}
