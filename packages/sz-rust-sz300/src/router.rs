// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
#[cfg(feature = "v19-plugin-flow")]
use crate::controllers::plugin;
use crate::controllers::{
    ai, auth, device, file, file_serve, health, merchant, order, product, view, wasm_api,
};
use crate::middleware::auth_middleware;
use crate::openapi;
use crate::state::AppState;
use axum::{middleware, routing::get, routing::post, Router};
use std::sync::Arc;
use sz_rust_core::middleware::cors::{cors_layer, cors_layer_with_config};
use sz_rust_core::middleware::csrf::csrf_middleware;
use sz_rust_core::middleware::panic_guard::{panic_guard_middleware, PanicGuardConfig};
use sz_rust_middleware_facade::circuit_breaker::{
    circuit_breaker_middleware, CircuitBreaker, CircuitBreakerConfig,
};
use sz_rust_middleware_facade::rate_limit::{rate_limit_middleware, token_bucket_config};
use tower_http::cors::{AllowOrigin, CorsLayer};

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
    // v2.0.0 合并企业版：WASM 边缘计算（2026-10-27 P0-1 安全修复：已移出公开白名单）
    // 原「公开，与生产版一致」条目移除原因：公开端点 + 无燃料/超时限制 = 单请求可占死
    // tokio worker（远程 DoS）。现由 auth_middleware 保护（需 JWT），运行时已加燃料计量、
    // 内存限制 + 5s 超时。
];

/// 判断路径是否在公开白名单中
///
/// - 普通路径：精确匹配，避免 `/api/v1/auth/` 之类前缀绕过。
/// - 静态资源：`/uploads/` 前缀放行（浏览器 `<img src>` 无法携带 Authorization）。
///   路径穿越由 [`crate::controllers::file_serve::resolve_upload_path`] 的三重校验负责，
///   越界请求返回 400/404，不会读到上传目录之外的文件。
pub fn is_public_path(path: &str) -> bool {
    if path == "/uploads" || path.starts_with("/uploads/") {
        return true;
    }
    PUBLIC_PATHS.contains(&path)
}

/// 构建 CORS 层
///
/// - 设置 `SZ300_CORS_ORIGIN` 时：仅允许该精确 Origin（不带凭据），拒绝其他跨域来源。
/// - 生产环境（`SZ300_ENV=production`）未配置时：不下发任何 `Allow-Origin`，
///   跨域请求全部被浏览器拒绝（fail-closed）。
/// - 非生产环境未配置时：回退 `Allow-Origin: *`（无凭据），便于本地联调。
fn cors_layer_from_env() -> CorsLayer {
    if let Ok(origin) = std::env::var("SZ300_CORS_ORIGIN") {
        if !origin.is_empty() {
            match axum::http::header::HeaderValue::from_str(&origin) {
                Ok(value) => {
                    return cors_layer_with_config(
                        AllowOrigin::exact(value),
                        false,
                        "GET,POST,PUT,DELETE,PATCH,OPTIONS",
                        "Authorization,Content-Type,X-CSRF-TOKEN",
                        1800,
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "SZ300_CORS_ORIGIN 不是合法 HeaderValue（{e}），回退到默认 CORS 配置"
                    );
                }
            }
        }
    }
    if crate::config::is_production() {
        tracing::error!("生产环境未配置 SZ300_CORS_ORIGIN — 已拒绝所有跨域请求（fail-closed）");
        return CorsLayer::new();
    }
    cors_layer()
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
    // 公开路由（真正无需 JWT：健康检查、指标、API 文档、静态文件）
    // 受保护端点一律放入 protected_routes 并合入 api_routes，避免"名为公开、实为受保护"的误导。
    let public_routes = Router::new()
        .route("/api-docs", get(openapi::swagger_ui))
        .route("/api-docs/redoc", get(openapi::redoc))
        .route("/api-docs/openapi.json", get(openapi::openapi_json))
        .route("/health", get(health::check))
        .route("/health/ready", get(health::readiness))
        .route("/health/startup", get(health::startup))
        .route("/metrics", get(health::metrics))
        .route("/uploads/{*path}", get(file_serve::serve_file));

    // 受保护端点（需 JWT）：与 api_routes 一起挂载，并享受数据脱敏
    let protected_routes = Router::new()
        // v2.0.0 合并企业版：AI 端点（受 JWT 保护）
        .route("/api/v1/ai/chat", post(ai::chat))
        .route("/api/v1/ai/embed", post(ai::embed))
        .route("/api/v1/ai/stream", post(ai::stream))
        // v2.0.0 合并企业版：WASM 边缘计算（燃料计量 + 内存限制 + 超时）
        .route("/api/wasm/execute", post(wasm_api::execute))
        // v2.0.0 合并企业版：视图模板渲染（受 JWT 保护）
        .route("/page/{template}", get(view::render_page));

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

    // v1.9.0 插件管理端点（spec §5.10，非 RBAC 模式）
    #[cfg(all(feature = "v19-plugin-flow", not(feature = "v18-rbac")))]
    let api_routes = api_routes
        .route("/api/v1/plugin/install", post(plugin::install))
        .route("/api/v1/plugin/uninstall", post(plugin::uninstall))
        .route("/api/v1/plugin/list", get(plugin::list));

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

        // v1.9.0 插件管理路由（RBAC 模式，使用 merchant:create 权限）
        #[cfg(feature = "v19-plugin-flow")]
        let plugin_routes = Router::new()
            .route("/api/v1/plugin/install", post(plugin::install))
            .route("/api/v1/plugin/uninstall", post(plugin::uninstall))
            .route("/api/v1/plugin/list", get(plugin::list))
            .layer(middleware::from_fn_with_state(
                (engine.clone(), perm::merchant("create")),
                rbac_guard,
            ));

        #[cfg(all(feature = "v18-graphql", feature = "v19-plugin-flow"))]
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
            .merge(graphql_route)
            .merge(plugin_routes);
        #[cfg(all(feature = "v18-graphql", not(feature = "v19-plugin-flow")))]
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
        #[cfg(all(not(feature = "v18-graphql"), feature = "v19-plugin-flow"))]
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
            .merge(plugin_routes);
        #[cfg(all(not(feature = "v18-graphql"), not(feature = "v19-plugin-flow")))]
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

    // 受保护端点合入 API 路由（数据脱敏与审计链对它们同样生效）
    let api_routes = api_routes.merge(protected_routes);

    // v1.8.0 数据脱敏中间件（仅对 API 路由生效，/health /metrics 不脱敏）
    #[cfg(feature = "v18-data-mask")]
    let api_routes = api_routes.layer(sz_rust_data_mask::DataMaskLayer::new(
        crate::config::mask_engine(),
        sz_rust_data_mask::MaskScene::Response,
    ));

    // v1.8.0 审计链式哈希中间件（仅 POST/PUT/DELETE，排除 /health /metrics）
    //
    // 2026-10-28：从最外层下移到 api_routes 内层（auth 之内）——
    // 鉴权中间件先注入 CurrentUser，审计才能记录真实操作人；
    // 连接池由最外层的 `Extension<Arc<Pool>>` 提供（见下方 .layer(Extension(..))）。
    #[cfg(feature = "v18-audit-chain")]
    let api_routes = api_routes.layer(middleware::from_fn_with_state(
        state.chain_auditor.clone(),
        crate::middleware::audit_chain::audit_chain_middleware,
    ));

    // v2.0.0 合并企业版：限流配置（令牌桶，从环境变量读取阈值，健康检查/metrics 排除）
    let rl_config = crate::config::RateLimitProductionConfig::from_env();
    let rate_limit_layer = token_bucket_config(rl_config.capacity, rl_config.refill_per_second)
        .with_exclude_paths(rl_config.exclude_paths)
        .with_key_prefix(rl_config.key_prefix)
        .with_trust_proxy_headers(rl_config.trust_proxy_headers);

    // v2.0.0 合并企业版：熔断器配置（从环境变量读取阈值）
    let cb_config = crate::config::CircuitBreakerProductionConfig::from_env();
    cb_config.validate().expect("熔断配置非法");
    let circuit_breaker = Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
        error_threshold: cb_config.error_threshold,
        cooldown: cb_config.cooldown,
        probe_requests: cb_config.probe_requests,
        stat_window: cb_config.stat_window,
    }));

    let router = public_routes
        .merge(api_routes)
        // 审计中间件所需连接池（审计层在 auth 之内，此层在 auth 之外，故可见）
        .layer(axum::Extension(state.db_pool.clone()))
        // JWT 鉴权中间件（公开路径自动跳过）— 内层
        .layer(middleware::from_fn(auth_middleware::auth_middleware))
        // v2.0.0 合并企业版：熔断器中间件（位于限流之后、auth 之前，Open 态返回 503）
        .layer(middleware::from_fn_with_state(
            circuit_breaker,
            circuit_breaker_middleware,
        ))
        // v2.0.0 合并企业版：限流中间件（令牌桶，auth 之前限流避免无效请求消耗鉴权开销）
        .layer(middleware::from_fn_with_state(
            rate_limit_layer,
            rate_limit_middleware,
        ))
        // CSRF 防护中间件（双提交 Cookie 模式）
        .layer(middleware::from_fn(csrf_middleware))
        // CORS 跨域中间件（最外层）。
        // 生产环境通过 SZ300_CORS_ORIGIN 配置可信来源（精确匹配，不带凭据）；
        // 未配置时回退 Allow-Origin: *（无凭据，适合开发环境，见 cors_layer 安全说明）。
        .layer(cors_layer_from_env())
        // panic 兜底中间件（最外层，2026-10-28 安全修复：handler panic → 500 且不泄露内部信息）
        .layer(middleware::from_fn_with_state(
            PanicGuardConfig::default(),
            panic_guard_middleware,
        ))
        .with_state(state);

    // v1.9.0 全链路追踪中间件（feature gate 控制，默认不启用）
    // 为每个请求生成 TraceID + Root Span，注入 X-Trace-Id 响应头
    #[cfg(feature = "v19-obs-closure")]
    let router = router.layer(middleware::from_fn(
        crate::middleware::tracing_middleware::tracing_middleware,
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
/// v1.8.0 WebSocket handler（2026-10-28 安全修复：连接前强制 JWT 校验）
#[cfg(feature = "v18-websocket")]
async fn ws_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    axum::extract::State(state): axum::extract::State<AppState>,
    req: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    // 安全修复：未认证连接不得建立。支持 Authorization 头或 ?token= 查询参数。
    let token = extract_bearer_token(&req);
    if token.is_empty() || crate::services::auth_service::verify_token(token).is_err() {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            "WebSocket 需要有效 JWT",
        )
            .into_response();
    }

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

/// 从请求提取 Bearer token（Authorization 头或 `?token=` 查询参数）
#[cfg(any(feature = "v18-websocket", feature = "v18-sse"))]
fn extract_bearer_token(req: &axum::http::Request<axum::body::Body>) -> &str {
    let header = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !header.is_empty() {
        return header;
    }
    req.uri()
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")))
        .unwrap_or("")
}

/// v1.8.0 SSE handler（2026-10-28 安全修复：订阅前强制 JWT 校验）
#[cfg(feature = "v18-sse")]
async fn sse_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    req: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    use axum::response::sse::Sse;
    use futures::stream::{self, StreamExt};

    // 安全修复：未认证不得订阅事件流
    let token = extract_bearer_token(&req);
    if token.is_empty() || crate::services::auth_service::verify_token(token).is_err() {
        return (axum::http::StatusCode::UNAUTHORIZED, "SSE 需要有效 JWT").into_response();
    }

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

#[cfg(test)]
mod public_path_tests {
    use super::is_public_path;

    #[test]
    fn exact_public_paths_are_public() {
        assert!(is_public_path("/health"));
        assert!(is_public_path("/metrics"));
        assert!(is_public_path("/api/v1/auth/login"));
        assert!(is_public_path("/api-docs/openapi.json"));
    }

    #[test]
    fn uploads_prefix_is_public_but_not_path_bypass() {
        assert!(is_public_path("/uploads"));
        assert!(is_public_path("/uploads/2026/10/28/a.png"));
        // 仅是前缀相似，不得命中
        assert!(!is_public_path("/uploadsX/secret"));
        // 带 `..` 的路径仍走公开前缀（不做鉴权），但文件层会返回 400，不会越界读取
        assert!(is_public_path("/uploads/../etc/passwd"));
    }

    #[test]
    fn protected_paths_are_not_public() {
        assert!(!is_public_path("/api/v1/auth/me"));
        assert!(!is_public_path("/api/v1/auth/logout"));
        assert!(!is_public_path("/api/v1/merchant/create"));
        assert!(!is_public_path("/api/wasm/execute"));
        assert!(!is_public_path("/api/v1/auth/login/extra"));
    }
}
