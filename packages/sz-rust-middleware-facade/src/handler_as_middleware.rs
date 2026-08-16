//! Handler=Middleware 统一设计 — 借鉴 Salvo，减少概念数量
//!
//! 对齐技术规范 7.6.0 节：「Salvo (Handler=Middleware)：任何 Handler 都可作为
//! Middleware，反之亦然，减少概念数量」。
//!
//! ## 设计背景
//!
//! axum 的 Handler 和 Middleware 签名不同：
//!
//! | 类型 | 签名 | 调用 next？ |
//! |------|------|------------|
//! | Handler | `Fn(Extractors...) -> Future<Output: IntoResponse>` | 否（终端） |
//! | Middleware | `Fn(Request, Next) -> Future<Output = Response>` | 是（继续链） |
//!
//! sz-rust 提供双向转换器，让两者可以互相转换，减少概念数量：
//!
//! | 转换器 | 输入 | 输出 | 调用 next？ |
//! |--------|------|------|------------|
//! | [`handler_as_middleware`] | `Fn(Request) -> Future<Output: IntoResponse>` | `Fn(Request, Next) -> Future<Output = Response>` | 否（Handler 作为终端处理器，不继续链） |
//! | [`middleware_as_handler`] | `Fn(Request) -> Future<Output = Response>` | `Fn(Request) -> Future<Output = Response>` | 否（Middleware 直接返回响应，不调用 next） |
//!
//! ## 用法
//!
//! ### Handler → Middleware（将控制器方法嵌入中间件链末端）
//!
//! ```ignore
//! use sz_rust_core::middleware::handler_as_middleware::handler_as_middleware;
//! use axum::Router;
//! use axum::extract::Request;
//! use axum::response::IntoResponse;
//!
//! async fn my_handler(req: Request) -> impl IntoResponse {
//!     "handler called"
//! }
//!
//! let app = Router::new()
//!     .route("/", axum::routing::get(|| async { "hello" }))
//!     .layer(axum::middleware::from_fn(handler_as_middleware(my_handler)));
//! ```
//!
//! ### Middleware → Handler（将中间件作为路由处理器）
//!
//! ```ignore
//! use sz_rust_core::middleware::handler_as_middleware::middleware_as_handler;
//! use axum::Router;
//! use axum::extract::Request;
//! use axum::response::Response;
//!
//! async fn my_middleware(req: Request) -> Response {
//!     axum::response::Response::new("middleware as handler".into())
//! }
//!
//! let app = Router::new()
//!     .route("/", axum::routing::get(middleware_as_handler(my_middleware)));
//! ```
//!
//! ## 设计决策
//!
//! 1. **Handler → Middleware 不调用 next**：axum 的 `Request` 是不可 `Clone` 的
//!    （`Body` 不可 `Clone`），Handler 消费 `Request` 后无法继续调用 `next`。
//!    因此 `handler_as_middleware` 让 Handler 作为终端处理器，不继续中间件链。
//!    这符合「将 Handler 嵌入中间件链末端」的语义。
//!
//! 2. **Middleware → Handler 不调用 next**：将 Middleware 作为路由处理器时，
//!    没有 `next` 可调用（路由处理器是终端）。因此 `middleware_as_handler` 接收
//!    一个「不带 Next 参数」的 middleware 函数（签名 `Fn(Request) -> Future<Output = Response>`），
//!    直接调用它返回响应。这符合「将 Middleware 作为终端处理器」的语义。
//!
//! 3. **与 PHP 对齐**：PHP 端中间件 `handle(Request $request, Closure $next): Response`
//!    和控制器方法 `public function index(): Response` 是分离的概念。sz-rust 的
//!    双向转换器提供了 PHP 端没有的灵活性，但不改变 PHP 端的语义——迁移 PHP
//!    控制器时仍按控制器方式实现，迁移 PHP 中间件时仍按中间件方式实现。
//!    转换器仅用于 sz-rust 自研中间件需要复用为控制器方法的场景。

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::future::Future;
use std::sync::Arc;

/// 将 Handler 转换为 Middleware（终端模式，不调用 next）
///
/// 对齐 Salvo 的 Handler=Middleware 设计：任何 Handler 都可作为 Middleware 使用。
///
/// 转换后的 Middleware 会调用 Handler 处理请求，**不调用 next**（Handler 作为
/// 终端处理器）。这等价于「将 Handler 嵌入中间件链末端」。
///
/// ## 设计理由
///
/// axum 的 `Request` 不可 `Clone`（`Body` 不可 `Clone`），Handler 消费 `Request`
/// 后无法继续调用 `next`。因此 `handler_as_middleware` 让 Handler 作为终端处理器，
/// 不继续中间件链。
///
/// ## 类型参数
///
/// - `H`: Handler 类型，`Fn(Request) -> F`
/// - `F`: Handler 返回的 Future，`Future<Output = R> + Send`
/// - `R`: Handler 返回值类型，`IntoResponse`
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::middleware::handler_as_middleware::handler_as_middleware;
/// use axum::Router;
/// use axum::extract::Request;
/// use axum::response::IntoResponse;
///
/// async fn log_handler(req: Request) -> impl IntoResponse {
///     "handler called"
/// }
///
/// let app = Router::new()
///     .route("/", axum::routing::get(|| async { "hello" }))
///     .layer(axum::middleware::from_fn(handler_as_middleware(log_handler)));
/// ```
pub fn handler_as_middleware<H, F, R>(
    handler: H,
) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
       + Clone
       + Send
       + Sync
       + 'static
where
    H: Fn(Request) -> F + Send + Sync + 'static,
    F: Future<Output = R> + Send + 'static,
    R: IntoResponse + 'static,
{
    // 用 Arc<H> 包装：让闭包变为 Fn + Clone，满足 axum::middleware::from_fn 的约束
    let handler = Arc::new(handler);
    move |req: Request, _next: Next| {
        // 不调用 _next：Handler 消费 Request 后作为终端处理器
        let handler = handler.clone();
        Box::pin(async move {
            let _ = _next;
            handler(req).await.into_response()
        })
    }
}

/// 将 Middleware（不带 Next 参数的变体）转换为 Handler
///
/// 对齐 Salvo 的 Handler=Middleware 设计：任何 Middleware 都可作为 Handler 使用。
///
/// 转换后的 Handler 会调用 Middleware 处理逻辑，**不调用 next**（Middleware 作为
/// 终端处理器）。这等价于「将 Middleware 作为路由处理器」。
///
/// ## 设计理由
///
/// 将 Middleware 作为路由处理器时，没有 `next` 可调用（路由处理器是终端）。
/// 因此 `middleware_as_handler` 接收一个「不带 Next 参数」的 middleware 函数
/// （签名 `Fn(Request) -> Future<Output = Response>`），直接调用它返回响应。
/// 这符合「将 Middleware 作为终端处理器」的语义。
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::middleware::handler_as_middleware::middleware_as_handler;
/// use axum::Router;
/// use axum::extract::Request;
/// use axum::response::Response;
///
/// async fn my_terminal_middleware(req: Request) -> Response {
///     axum::response::Response::new("middleware as handler".into())
/// }
///
/// let app = Router::new()
///     .route("/", axum::routing::get(middleware_as_handler(my_terminal_middleware)));
/// ```
pub fn middleware_as_handler<H, F>(
    middleware: H,
) -> impl Fn(Request) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
       + Clone
       + Send
       + Sync
       + 'static
where
    H: Fn(Request) -> F + Send + Sync + 'static,
    F: Future<Output = Response> + Send + 'static,
{
    // 用 Arc<H> 包装：让闭包变为 Fn + Clone，可被 axum::routing 多次调用
    let middleware = Arc::new(middleware);
    move |req: Request| {
        let middleware = middleware.clone();
        Box::pin(async move { middleware(req).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // ====================================================================
    // 辅助函数
    // ====================================================================

    async fn read_body(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).expect("响应体应为 UTF-8")
    }

    fn make_request(method: &str, uri: &str) -> Request {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    // ====================================================================
    // handler_as_middleware 集成测试（通过 Router 验证，不直接构造 Next）
    // ====================================================================

    #[tokio::test]
    async fn test_handler_as_middleware_calls_handler_returns_response() {
        // Handler 返回固定字符串，转换后的 Middleware 应调用 Handler
        async fn my_handler(_req: Request) -> &'static str {
            "handler called"
        }

        let app: Router = Router::new()
            .route("/", axum::routing::get(|| async { "route" }))
            .layer(axum::middleware::from_fn(handler_as_middleware(my_handler)));

        let resp = app
            .oneshot(make_request("GET", "/"))
            .await
            .expect("测试请求执行失败");
        let body = read_body(resp).await;
        // handler_as_middleware 不调用 next，所以返回 handler 的响应
        assert_eq!(body, "handler called");
    }

    #[tokio::test]
    async fn test_handler_as_middleware_does_not_call_route_handler() {
        // Handler 转换后的 Middleware 不应调用 next（即不应执行路由处理器）
        // 验证方式：路由处理器设置 AtomicBool，若被调用则标志位变 true
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let next_called = Arc::new(AtomicBool::new(false));
        let next_called_for_layer = next_called.clone();

        async fn my_handler(_req: Request) -> &'static str {
            "handler response"
        }

        // 路由处理器：若被调用，会设置 AtomicBool 为 true 并返回 "next called"
        // handler_as_middleware 不调用 next，所以路由处理器不应被调用
        let app: Router = Router::new()
            .route(
                "/",
                axum::routing::get(move || {
                    let next_called = next_called_for_layer.clone();
                    async move {
                        next_called.store(true, Ordering::SeqCst);
                        "next called"
                    }
                }),
            )
            .layer(axum::middleware::from_fn(handler_as_middleware(my_handler)));

        let resp = app
            .oneshot(make_request("GET", "/"))
            .await
            .expect("测试请求执行失败");
        let body = read_body(resp).await;
        assert_eq!(body, "handler response");
        assert!(
            !next_called.load(Ordering::SeqCst),
            "route handler should NOT be called in handler_as_middleware"
        );
    }

    #[tokio::test]
    async fn test_handler_as_middleware_with_json_response() {
        // Handler 返回 JSON 响应
        async fn json_handler(_req: Request) -> axum::Json<serde_json::Value> {
            axum::Json(serde_json::json!({"code": 1, "msg": "ok"}))
        }

        let app: Router = Router::new()
            .route("/", axum::routing::get(|| async { "route" }))
            .layer(axum::middleware::from_fn(handler_as_middleware(
                json_handler,
            )));

        let resp = app
            .oneshot(make_request("GET", "/"))
            .await
            .expect("测试请求执行失败");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert!(body.contains("\"code\":1"));
        assert!(body.contains("\"msg\":\"ok\""));
    }

    #[tokio::test]
    async fn test_handler_as_middleware_with_status_code() {
        // Handler 返回特定状态码
        async fn error_handler(_req: Request) -> (StatusCode, &'static str) {
            (StatusCode::NOT_FOUND, "not found")
        }

        let app: Router = Router::new()
            .route("/", axum::routing::get(|| async { "route" }))
            .layer(axum::middleware::from_fn(handler_as_middleware(
                error_handler,
            )));

        let resp = app
            .oneshot(make_request("GET", "/"))
            .await
            .expect("测试请求执行失败");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = read_body(resp).await;
        assert_eq!(body, "not found");
    }

    #[tokio::test]
    async fn test_handler_as_middleware_preserves_request_method() {
        // Middleware 应能接收到原始请求方法
        async fn method_handler(req: Request) -> String {
            req.method().to_string()
        }

        let app: Router = Router::new()
            .route("/", axum::routing::get(|| async { "route" }))
            .layer(axum::middleware::from_fn(handler_as_middleware(
                method_handler,
            )));

        let resp = app.oneshot(make_request("POST", "/")).await.unwrap();
        let body = read_body(resp).await;
        assert_eq!(body, "POST");
    }

    #[tokio::test]
    async fn test_handler_as_middleware_preserves_request_uri() {
        async fn uri_handler(req: Request) -> String {
            req.uri().to_string()
        }

        let app: Router = Router::new()
            .route("/api/test", axum::routing::get(|| async { "route" }))
            .layer(axum::middleware::from_fn(handler_as_middleware(
                uri_handler,
            )));

        let resp = app.oneshot(make_request("GET", "/api/test")).await.unwrap();
        let body = read_body(resp).await;
        assert_eq!(body, "/api/test");
    }

    #[tokio::test]
    async fn test_handler_as_middleware_with_request_body() {
        // Handler 应能读取请求 body
        async fn body_handler(req: Request) -> String {
            let bytes = req
                .into_body()
                .collect()
                .await
                .expect("响应体读取失败")
                .to_bytes();
            String::from_utf8(bytes.to_vec()).expect("响应体应为 UTF-8")
        }

        let app: Router = Router::new()
            .route("/", axum::routing::get(|| async { "route" }))
            .layer(axum::middleware::from_fn(handler_as_middleware(
                body_handler,
            )));

        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from("hello body"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = read_body(resp).await;
        assert_eq!(body, "hello body");
    }

    // ====================================================================
    // middleware_as_handler 集成测试
    // ====================================================================

    #[tokio::test]
    async fn test_middleware_as_handler_calls_middleware() {
        // Middleware（不调用 next）转换为 Handler
        async fn my_middleware(_req: Request) -> Response {
            Response::new(Body::from("middleware as handler"))
        }

        let app: Router = Router::new().route(
            "/",
            axum::routing::get(middleware_as_handler(my_middleware)),
        );

        let resp = app
            .oneshot(make_request("GET", "/"))
            .await
            .expect("测试请求执行失败");
        let body = read_body(resp).await;
        assert_eq!(body, "middleware as handler");
    }

    #[tokio::test]
    async fn test_middleware_as_handler_with_status_code() {
        async fn error_middleware(_req: Request) -> Response {
            let mut resp = Response::new(Body::from("forbidden"));
            *resp.status_mut() = StatusCode::FORBIDDEN;
            resp
        }

        let app: Router = Router::new().route(
            "/",
            axum::routing::get(middleware_as_handler(error_middleware)),
        );

        let resp = app
            .oneshot(make_request("GET", "/"))
            .await
            .expect("测试请求执行失败");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = read_body(resp).await;
        assert_eq!(body, "forbidden");
    }

    #[tokio::test]
    async fn test_middleware_as_handler_preserves_request_method() {
        async fn method_middleware(req: Request) -> Response {
            Response::new(Body::from(req.method().to_string()))
        }

        let app: Router = Router::new().route(
            "/",
            axum::routing::get(middleware_as_handler(method_middleware)),
        );

        let resp = app
            .oneshot(make_request("GET", "/"))
            .await
            .expect("测试请求执行失败");
        let body = read_body(resp).await;
        assert_eq!(body, "GET");
    }

    #[tokio::test]
    async fn test_middleware_as_handler_with_json_response() {
        async fn json_middleware(_req: Request) -> Response {
            let body = serde_json::json!({"code": 1, "msg": "ok"});
            Response::new(Body::from(body.to_string()))
        }

        let app: Router = Router::new().route(
            "/",
            axum::routing::get(middleware_as_handler(json_middleware)),
        );

        let resp = app
            .oneshot(make_request("GET", "/"))
            .await
            .expect("测试请求执行失败");
        let body = read_body(resp).await;
        assert!(body.contains("\"code\":1"));
        assert!(body.contains("\"msg\":\"ok\""));
    }

    // ====================================================================
    // 对齐 Salvo 设计验证
    // ====================================================================

    #[tokio::test]
    async fn test_handler_and_middleware_interchangeable() {
        // 验证 Handler 和 Middleware 可以互相转换（Salvo 设计核心）
        // 1. 定义一个 Handler
        async fn my_handler(_req: Request) -> &'static str {
            "handler"
        }

        // 2. 将 Handler 转换为 Middleware
        let app: Router = Router::new()
            .route("/", axum::routing::get(|| async { "route" }))
            .layer(axum::middleware::from_fn(handler_as_middleware(my_handler)));

        let resp = app
            .oneshot(make_request("GET", "/"))
            .await
            .expect("测试请求执行失败");
        let body = read_body(resp).await;
        assert_eq!(body, "handler"); // Middleware 不调用 next，返回 handler 响应
    }

    #[tokio::test]
    async fn test_middleware_as_handler_used_as_route_handler() {
        // Middleware 可直接作为路由处理器使用
        async fn terminal_middleware(_req: Request) -> Response {
            Response::new(Body::from("terminal middleware as handler"))
        }

        let app: Router = Router::new().route(
            "/api/test",
            axum::routing::get(middleware_as_handler(terminal_middleware)),
        );

        let resp = app.oneshot(make_request("GET", "/api/test")).await.unwrap();
        let body = read_body(resp).await;
        assert_eq!(body, "terminal middleware as handler");
    }

    #[tokio::test]
    async fn test_handler_as_middleware_layered_before_other_middleware() {
        // handler_as_middleware 作为中间件链中的一环
        // 验证链式调用：前置中间件调用 next → handler_as_middleware 不调用 next → 返回 handler 响应

        // 第一个中间件：调用 next，将请求继续传递
        async fn first_middleware(req: Request, next: Next) -> Response {
            next.run(req).await
        }

        // handler_as_middleware 作为终端中间件，不调用 next，直接返回 handler 响应
        async fn my_handler(_req: Request) -> String {
            "handler called at order 1".to_string()
        }

        let app: Router = Router::new()
            .route("/", axum::routing::get(|| async { "route" }))
            .layer(axum::middleware::from_fn(handler_as_middleware(my_handler)))
            .layer(axum::middleware::from_fn(first_middleware));

        let resp = app
            .oneshot(make_request("GET", "/"))
            .await
            .expect("测试请求执行失败");
        let body = read_body(resp).await;
        // first_middleware 调用 next → handler_as_middleware 不调用 next → 返回 handler 响应
        assert_eq!(body, "handler called at order 1");
    }
}
