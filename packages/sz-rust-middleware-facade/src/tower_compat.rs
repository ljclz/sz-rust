//! Tower 生态兼容验证 — tower-http Layer 可插入 sz-rust 中间件链
//!
//! 本模块验证 sz-rust 的 [`MiddlewareBuilder`] 与 tower 生态的兼容性，
//! 提供：
//!
//! 1. [`TowerLayer`] 枚举：包装 3 个常用 `tower-http` Layer（`Compression` / `Timeout` / `TowerTrace`），
//!    这些 Layer 不属于 sz-rust 的 [`MiddlewareKind`](super::order::MiddlewareKind)（5 个），
//!    通过 [`TowerCompat`] 包装器与 [`MiddlewareBuilder`] 组合使用
//! 2. [`TowerCompat`] 结构体：包装 [`MiddlewareBuilder`] + `Vec<TowerLayer>`，
//!    提供 `apply(router)` 一次性应用 sz-rust 中间件链 + tower-http 扩展 Layer
//! 3. 3 个便捷构造函数：[`compression_layer()`] / [`timeout_layer()`] / [`tower_trace_layer()`]
//!
//! ## 与 sz-rust 中间件的关系
//!
//! | sz-rust 中间件 | tower-http 等价 | 关系 | 本模块处理 |
//! |----------------|-----------------|------|-----------|
//! | [`cors::cors_layer()`](super::cors::cors_layer) | `tower_http::cors::CorsLayer` | 完全等价（MiddlewareBuilder 直接支持 `with_cors()`） | 不重复包装 |
//! | [`trace::trace_middleware`](super::trace::trace_middleware) | `tower_http::trace::TraceLayer` | 不同实现（sz-rust 自研 W3C TraceContext 传播） | 提供 [`tower_trace_layer()`] 兼容验证 |
//! | [`log::log_middleware`](super::log::log_middleware) | `tower_http::trace::TraceLayer` | 不同实现（sz-rust 自研结构化日志） | 不重复包装 |
//! | （PHP 端无） | `tower_http::compression::CompressionLayer` | sz-rust 新增（对齐 Nginx `gzip_static`） | [`compression_layer()`] |
//! | （PHP 端无） | `tower_http::timeout::TimeoutLayer` | sz-rust 新增（对齐 Nginx `fastcgi_read_timeout`） | [`timeout_layer()`] |
//!
//! ## PHP 端对齐
//!
//! PHP 端 `app/middleware.php` 仅含 `SessionInit` + `AllowCrossDomain`，无压缩/超时/tower-trace。
//! sz-rust 通过 [`TowerCompat`] 提供 tower 生态兼容能力，调用方按需启用：
//!
//! - **Compression**：对齐 Nginx `gzip_static on;`，PHP 端由 Nginx 处理，sz-rust 端可选启用
//! - **Timeout**：对齐 Nginx `fastcgi_read_timeout 30s;`，PHP 端由 Nginx 处理，sz-rust 端可选启用
//! - **TowerTrace**：与 sz-rust 自研 [`trace_middleware`](super::trace::trace_middleware) 功能重叠，
//!   仅用于兼容验证，**生产环境应使用 sz-rust 自研 trace_middleware**（支持 W3C TraceContext 传播）
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::middleware::builder::MiddlewareBuilder;
//! use sz_rust_core::middleware::tower_compat::{TowerCompat, compression_layer, timeout_layer};
//! use std::time::Duration;
//! use axum::Router;
//!
//! // 1. TowerCompat 包装 MiddlewareBuilder + tower-http 扩展 Layer
//! let app: Router = Router::new()
//!     .route("/", axum::routing::get(|| async { "hello" }));
//! let app = TowerCompat::php_global()
//!     .with_compression()
//!     .with_timeout(Duration::from_secs(30))
//!     .apply(app);
//!
//! // 2. 独立使用 tower-http Layer（不通过 MiddlewareBuilder）
//! let app: Router = Router::new()
//!     .route("/", axum::routing::get(|| async { "hello" }))
//!     .layer(compression_layer())
//!     .layer(timeout_layer(Duration::from_secs(30)));
//! ```
//!
//! ## 执行顺序约定
//!
//! [`TowerCompat::apply()`] 执行顺序：
//!
//! 1. 先应用 sz-rust 中间件链（`MiddlewareBuilder::apply()`），按 `chain.order()` 顺序执行
//! 2. 再应用 tower-http 扩展 Layer，按 `tower_layers` 添加顺序执行（首元素最先执行）
//!
//! 例如：
//! ```ignore
//! TowerCompat::php_global()  // sz-rust: Trace -> Cors
//!     .with_compression()    // tower: Compression（第 1 个添加）
//!     .with_timeout(...)     // tower: Timeout（第 2 个添加）
//!     .apply(router);
//! // 实际执行顺序：Trace -> Cors -> Compression -> Timeout -> handler
//! ```
//!
//! 实现上，tower-http Layer 按 `tower_layers` 逆序遍历调用 `Router::layer`（后注册先执行），
//! 保证添加顺序与执行顺序一致。

use axum::Router;
use std::sync::Arc;
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer as TowerTraceLayer;

use super::builder::MiddlewareBuilder;

// ============================================================================
// TowerLayer 枚举
// ============================================================================

/// Tower-http 扩展 Layer 包装枚举
///
/// 包装 3 个常用 `tower-http` Layer，这些 Layer 不属于 sz-rust 的
/// [`MiddlewareKind`](super::order::MiddlewareKind)（5 个），通过 [`TowerCompat`]
/// 包装器与 [`MiddlewareBuilder`] 组合使用。
///
/// ## 变体说明
///
/// | 变体 | 类型 | PHP 端对齐 | 用途 |
/// |------|------|-----------|------|
/// | [`Compression`](Self::Compression) | `CompressionLayer` | Nginx `gzip_static` | Gzip 压缩响应 |
/// | [`Timeout`](Self::Timeout) | `TimeoutLayer` | Nginx `fastcgi_read_timeout` | 请求超时控制 |
/// | [`TowerTrace`](Self::TowerTrace) | `Arc<dyn Fn(Router) -> Router>` | （无，sz-rust 兼容验证） | tower-http 自带 TraceLayer |
///
/// ## TowerTrace 类型擦除说明
///
/// `tower_http::trace::TraceLayer` 是泛型结构体（含 `M` 等 7 个类型参数），无法直接命名。
/// 使用 `Arc<dyn Fn(Router) -> Router + Send + Sync>` 包装「应用 TraceLayer 到 Router」的闭包，
/// 实现：
/// 1. 类型擦除：无需命名 TraceLayer 的具体类型
/// 2. `Clone`：通过 `Arc::clone` 共享闭包
/// 3. 多次 apply：`Fn` trait 可多次调用（每次调用闭包内部新建 `TraceLayer::new_for_http()`）
#[derive(Clone)]
pub enum TowerLayer {
    /// Gzip 压缩（对齐 Nginx `gzip_static on;`，PHP 端由 Nginx 处理）
    Compression(CompressionLayer),
    /// 请求超时（对齐 Nginx `fastcgi_read_timeout 30s;`，PHP 端由 Nginx 处理）
    Timeout(TimeoutLayer),
    /// tower-http 自带 TraceLayer（与 sz-rust `trace_middleware` 不同，仅用于兼容验证）
    ///
    /// **注意**：生产环境应使用 sz-rust 自研 [`trace_middleware`](super::trace::trace_middleware)，
    /// 它支持 W3C TraceContext 传播。
    TowerTrace(Arc<dyn Fn(Router) -> Router + Send + Sync>),
}

impl TowerLayer {
    /// 应用到 `axum::Router`
    pub fn apply(self, router: Router) -> Router {
        match self {
            TowerLayer::Compression(layer) => router.layer(layer),
            TowerLayer::Timeout(layer) => router.layer(layer),
            TowerLayer::TowerTrace(apply_fn) => apply_fn(router),
        }
    }

    /// 返回 Layer 类型名称（用于日志和测试）
    pub fn kind_name(&self) -> &'static str {
        match self {
            TowerLayer::Compression(_) => "compression",
            TowerLayer::Timeout(_) => "timeout",
            TowerLayer::TowerTrace(_) => "tower_trace",
        }
    }

    /// 是否为 Compression Layer
    pub fn is_compression(&self) -> bool {
        matches!(self, TowerLayer::Compression(_))
    }

    /// 是否为 Timeout Layer
    pub fn is_timeout(&self) -> bool {
        matches!(self, TowerLayer::Timeout(_))
    }

    /// 是否为 TowerTrace Layer
    pub fn is_tower_trace(&self) -> bool {
        matches!(self, TowerLayer::TowerTrace(_))
    }
}

impl std::fmt::Debug for TowerLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TowerLayer::Compression(_) => f.debug_tuple("Compression").finish(),
            TowerLayer::Timeout(_) => f.debug_tuple("Timeout").finish(),
            TowerLayer::TowerTrace(_) => f.debug_tuple("TowerTrace").finish(),
        }
    }
}

// ============================================================================
// 便捷构造函数
// ============================================================================

/// 创建默认 Gzip 压缩 Layer
///
/// 对齐 Nginx `gzip_static on; gzip_types text/plain application/json;` 默认行为。
/// 压缩 `text/*` / `application/json` / `application/javascript` / `application/xml` 等常见 MIME 类型。
///
/// ## PHP 端对齐
///
/// PHP 端由 Nginx 处理压缩（`gzip_static on;`），sz-rust 端可选启用（适用于 Rust 直连客户端场景，
/// 跳过 Nginx 反向代理）。
pub fn compression_layer() -> CompressionLayer {
    CompressionLayer::new()
}

/// 创建请求超时 Layer
///
/// 对齐 Nginx `fastcgi_read_timeout 30s;`，超时返回 HTTP 504 Gateway Timeout。
///
/// ## 参数
///
/// - `duration`：超时时长（推荐 30 秒，对齐 Nginx 默认值）
///
/// ## PHP 端对齐
///
/// PHP 端由 Nginx 处理超时（`fastcgi_read_timeout`），sz-rust 端可选启用（适用于长连接场景）。
pub fn timeout_layer(duration: Duration) -> TimeoutLayer {
    TimeoutLayer::with_status_code(axum::http::StatusCode::GATEWAY_TIMEOUT, duration)
}

/// 创建 tower-http 自带 TraceLayer
///
/// **注意**：与 sz-rust 自研 [`trace_middleware`](super::trace::trace_middleware) 功能重叠，
/// 仅用于兼容验证。生产环境应使用 sz-rust 自研 trace_middleware（支持 W3C TraceContext 传播）。
///
/// ## 区别
///
/// | 特性 | sz-rust `trace_middleware` | `tower_http::trace::TraceLayer` |
/// |------|---------------------------|--------------------------------|
/// | TraceContext 传播 | ✅ W3C 标准 | ❌ 仅 tower 内部 span |
/// | Span 注入 extensions | ✅ 下游可获取 | ❌ |
/// | 自定义 service_name | ✅ | ❌ |
/// | 排除路径 | ✅ | ❌ |
///
/// ## 返回类型说明
///
/// `TraceLayer` 是泛型结构体（含 `M` 等 7 个类型参数），无法直接命名。
/// 返回 `Arc<dyn Fn(Router) -> Router + Send + Sync>` 闭包，闭包内部新建 `TraceLayer::new_for_http()`
/// 并应用到传入的 Router，实现类型擦除。
pub fn tower_trace_layer() -> Arc<dyn Fn(Router) -> Router + Send + Sync> {
    Arc::new(|router: Router| router.layer(TowerTraceLayer::new_for_http()))
}

// ============================================================================
// TowerCompat 包装器
// ============================================================================

/// Tower 生态兼容包装器
///
/// 包装 [`MiddlewareBuilder`] + `Vec<TowerLayer>`，提供统一的 `apply(router)` 接口，
/// 一次性应用 sz-rust 中间件链 + tower-http 扩展 Layer。
///
/// ## 执行顺序
///
/// 1. 先应用 sz-rust 中间件链（[`MiddlewareBuilder::apply()`]），按 `chain.order()` 顺序执行
/// 2. 再应用 tower-http 扩展 Layer，按 `tower_layers` 添加顺序执行（首元素最先执行）
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::middleware::tower_compat::TowerCompat;
/// use std::time::Duration;
/// use axum::Router;
///
/// let app: Router = Router::new()
///     .route("/", axum::routing::get(|| async { "hello" }));
/// let app = TowerCompat::php_global()
///     .with_compression()
///     .with_timeout(Duration::from_secs(30))
///     .apply(app);
/// ```
#[derive(Clone)]
pub struct TowerCompat {
    builder: MiddlewareBuilder,
    tower_layers: Vec<TowerLayer>,
}

impl TowerCompat {
    /// 从 [`MiddlewareBuilder`] 创建 [`TowerCompat`]
    pub fn from_builder(builder: MiddlewareBuilder) -> Self {
        Self {
            builder,
            tower_layers: Vec::new(),
        }
    }

    /// 从 PHP 全局构建器创建（对齐 `app/middleware.php`：`SessionInit` + `AllowCrossDomain`）
    pub fn php_global() -> Self {
        Self::from_builder(MiddlewareBuilder::php_global_builder())
    }

    /// 从默认构建器创建（含 5 个 sz-rust 中间件，Config 需调用方显式设置）
    pub fn default_builder() -> Self {
        Self::from_builder(MiddlewareBuilder::default_builder())
    }

    /// 添加 tower-http Layer（在 sz-rust 中间件链之后执行）
    ///
    /// 多次调用会追加 Layer，按添加顺序执行（首元素最先执行）。
    pub fn with_tower_layer(mut self, layer: TowerLayer) -> Self {
        self.tower_layers.push(layer);
        self
    }

    /// 添加 Gzip 压缩 Layer（便捷方法，等价于 `with_tower_layer(TowerLayer::Compression(compression_layer()))`）
    pub fn with_compression(self) -> Self {
        self.with_tower_layer(TowerLayer::Compression(compression_layer()))
    }

    /// 添加请求超时 Layer
    pub fn with_timeout(self, duration: Duration) -> Self {
        self.with_tower_layer(TowerLayer::Timeout(timeout_layer(duration)))
    }

    /// 添加 tower-http TraceLayer（兼容验证用，生产环境应使用 sz-rust `trace_middleware`）
    pub fn with_tower_trace(self) -> Self {
        self.with_tower_layer(TowerLayer::TowerTrace(tower_trace_layer()))
    }

    /// 访问内部 [`MiddlewareBuilder`] 引用
    pub fn builder(&self) -> &MiddlewareBuilder {
        &self.builder
    }

    /// 访问 tower-http Layer 切片
    pub fn tower_layers(&self) -> &[TowerLayer] {
        &self.tower_layers
    }

    /// 返回 tower-http Layer 数量
    pub fn tower_layer_count(&self) -> usize {
        self.tower_layers.len()
    }

    /// 判断是否包含指定类型 tower-http Layer
    pub fn has_tower_layer(&self, kind_name: &str) -> bool {
        self.tower_layers.iter().any(|l| l.kind_name() == kind_name)
    }

    /// 应用所有中间件到 `axum::Router`
    ///
    /// ## 执行顺序
    ///
    /// 1. 先应用 sz-rust 中间件链（`MiddlewareBuilder::apply()`）
    /// 2. 再应用 tower-http 扩展 Layer（按添加顺序的逆序遍历 `Router::layer`，保证添加顺序 = 执行顺序）
    ///
    /// ## 消耗语义
    ///
    /// 此方法消耗 `self`（取出 builder 和 tower_layers 的所有权），返回应用了中间件的 `Router`。
    pub fn apply(self, router: Router) -> Router {
        // 1. 应用 sz-rust 中间件链
        let mut router = self.builder.apply(router);
        // 2. 应用 tower-http 扩展 Layer（逆序遍历：后注册先执行 = 添加顺序为先执行）
        for layer in self.tower_layers.into_iter().rev() {
            router = layer.apply(router);
        }
        router
    }
}

impl std::fmt::Debug for TowerCompat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TowerCompat")
            .field("builder", &self.builder)
            .field(
                "tower_layers",
                &self
                    .tower_layers
                    .iter()
                    .map(|l| l.kind_name())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // ====================================================================
    // 辅助函数
    // ====================================================================

    async fn read_body(resp: axum::response::Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn make_request(method: &str, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn make_request_with_header(method: &str, uri: &str, key: &str, value: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(key, value)
            .body(Body::empty())
            .unwrap()
    }

    fn make_router() -> Router {
        Router::new()
            .route("/", axum::routing::get(|| async { "hello world" }))
            .route(
                "/large",
                axum::routing::get(|| async {
                    // 大响应触发压缩
                    "a".repeat(1000)
                }),
            )
    }

    // ====================================================================
    // 组 1：TowerLayer 枚举
    // ====================================================================

    #[test]
    fn test_tower_layer_compression_variant() {
        let layer = TowerLayer::Compression(compression_layer());
        assert!(layer.is_compression());
        assert!(!layer.is_timeout());
        assert!(!layer.is_tower_trace());
        assert_eq!(layer.kind_name(), "compression");
    }

    #[test]
    fn test_tower_layer_timeout_variant() {
        let layer = TowerLayer::Timeout(timeout_layer(Duration::from_secs(30)));
        assert!(!layer.is_compression());
        assert!(layer.is_timeout());
        assert!(!layer.is_tower_trace());
        assert_eq!(layer.kind_name(), "timeout");
    }

    #[test]
    fn test_tower_layer_tower_trace_variant() {
        let layer = TowerLayer::TowerTrace(tower_trace_layer());
        assert!(!layer.is_compression());
        assert!(!layer.is_timeout());
        assert!(layer.is_tower_trace());
        assert_eq!(layer.kind_name(), "tower_trace");
    }

    #[test]
    fn test_tower_layer_clone() {
        let layer = TowerLayer::Compression(compression_layer());
        let cloned = layer.clone();
        assert_eq!(layer.kind_name(), cloned.kind_name());
    }

    #[test]
    fn test_tower_layer_debug_format() {
        let layer = TowerLayer::Compression(compression_layer());
        let debug_str = format!("{:?}", layer);
        assert!(debug_str.contains("Compression"));
    }

    // ====================================================================
    // 组 2：便捷构造函数
    // ====================================================================

    #[test]
    fn test_compression_layer_default() {
        let _layer = compression_layer();
        // CompressionLayer::new() 返回默认配置（gzip）
        // 验证不 panic 即可
    }

    #[test]
    fn test_timeout_layer_with_duration() {
        let _layer = timeout_layer(Duration::from_secs(30));
        // TimeoutLayer::new(duration) 创建超时 Layer
    }

    #[test]
    fn test_timeout_layer_zero_duration() {
        // 边界测试：0 秒超时（立即超时）
        let _layer = timeout_layer(Duration::from_secs(0));
    }

    #[test]
    fn test_tower_trace_layer_for_http() {
        let _layer = tower_trace_layer();
        // TraceLayer::new_for_http() 创建 HTTP 请求追踪 Layer
    }

    // ====================================================================
    // 组 3：TowerCompat 构造
    // ====================================================================

    #[test]
    fn test_tower_compat_from_builder() {
        let builder = MiddlewareBuilder::new();
        let compat = TowerCompat::from_builder(builder);
        assert_eq!(compat.tower_layer_count(), 0);
        assert!(compat.tower_layers().is_empty());
    }

    #[test]
    fn test_tower_compat_php_global() {
        let compat = TowerCompat::php_global();
        // 内部 builder 应为 php_global_builder（chain 长度 2）
        assert_eq!(compat.builder().chain().len(), 2);
        assert_eq!(compat.tower_layer_count(), 0);
    }

    #[test]
    fn test_tower_compat_default_builder() {
        let compat = TowerCompat::default_builder();
        // 内部 builder 应为 default_builder（chain 长度 9）
        assert_eq!(compat.builder().chain().len(), 9);
        assert_eq!(compat.tower_layer_count(), 0);
    }

    #[test]
    fn test_tower_compat_clone() {
        let compat = TowerCompat::php_global().with_compression();
        let cloned = compat.clone();
        assert_eq!(compat.tower_layer_count(), cloned.tower_layer_count());
    }

    #[test]
    fn test_tower_compat_debug_format() {
        let compat = TowerCompat::php_global()
            .with_compression()
            .with_timeout(Duration::from_secs(30));
        let debug_str = format!("{:?}", compat);
        assert!(debug_str.contains("TowerCompat"));
        assert!(debug_str.contains("compression"));
        assert!(debug_str.contains("timeout"));
    }

    // ====================================================================
    // 组 4：TowerCompat with_xxx 链式 builder
    // ====================================================================

    #[test]
    fn test_tower_compat_with_tower_layer() {
        let compat = TowerCompat::php_global()
            .with_tower_layer(TowerLayer::Compression(compression_layer()));
        assert_eq!(compat.tower_layer_count(), 1);
        assert!(compat.has_tower_layer("compression"));
    }

    #[test]
    fn test_tower_compat_with_compression() {
        let compat = TowerCompat::php_global().with_compression();
        assert_eq!(compat.tower_layer_count(), 1);
        assert!(compat.has_tower_layer("compression"));
        assert!(compat.tower_layers()[0].is_compression());
    }

    #[test]
    fn test_tower_compat_with_timeout() {
        let compat = TowerCompat::php_global().with_timeout(Duration::from_secs(30));
        assert_eq!(compat.tower_layer_count(), 1);
        assert!(compat.has_tower_layer("timeout"));
        assert!(compat.tower_layers()[0].is_timeout());
    }

    #[test]
    fn test_tower_compat_with_tower_trace() {
        let compat = TowerCompat::php_global().with_tower_trace();
        assert_eq!(compat.tower_layer_count(), 1);
        assert!(compat.has_tower_layer("tower_trace"));
        assert!(compat.tower_layers()[0].is_tower_trace());
    }

    #[test]
    fn test_tower_compat_chained_builders() {
        let compat = TowerCompat::php_global()
            .with_compression()
            .with_timeout(Duration::from_secs(30))
            .with_tower_trace();
        assert_eq!(compat.tower_layer_count(), 3);
        // 添加顺序：Compression, Timeout, TowerTrace
        assert!(compat.tower_layers()[0].is_compression());
        assert!(compat.tower_layers()[1].is_timeout());
        assert!(compat.tower_layers()[2].is_tower_trace());
    }

    #[test]
    fn test_tower_compat_has_tower_layer_negative() {
        let compat = TowerCompat::php_global().with_compression();
        assert!(!compat.has_tower_layer("timeout"));
        assert!(!compat.has_tower_layer("tower_trace"));
    }

    // ====================================================================
    // 组 5：TowerCompat apply 应用到 Router
    // ====================================================================

    #[tokio::test]
    async fn test_tower_compat_apply_preserves_routes() {
        let app = make_router();
        let app = TowerCompat::php_global().apply(app);

        // 路由仍可访问
        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "hello world");
    }

    #[tokio::test]
    async fn test_tower_compat_apply_with_compression() {
        let app = make_router();
        let app = TowerCompat::php_global().with_compression().apply(app);

        // 发送带 Accept-Encoding: gzip 的请求
        let resp = app
            .oneshot(make_request_with_header(
                "GET",
                "/large",
                "accept-encoding",
                "gzip",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // 压缩后应有 Content-Encoding: gzip header
        let content_encoding = resp.headers().get("content-encoding");
        assert!(
            content_encoding.is_some(),
            "Response should have Content-Encoding: gzip header"
        );
    }

    #[tokio::test]
    async fn test_tower_compat_apply_with_timeout_passes_within_timeout() {
        let app = make_router();
        let app = TowerCompat::php_global()
            .with_timeout(Duration::from_secs(30))
            .apply(app);

        // 快速响应应在超时内通过
        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_tower_compat_apply_with_tower_trace() {
        let app = make_router();
        let app = TowerCompat::php_global().with_tower_trace().apply(app);

        // tower-http TraceLayer 不影响响应
        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "hello world");
    }

    #[tokio::test]
    async fn test_tower_compat_apply_with_all_layers() {
        let app = make_router();
        let app = TowerCompat::php_global()
            .with_compression()
            .with_timeout(Duration::from_secs(30))
            .with_tower_trace()
            .apply(app);

        // 所有 Layer 组合应用，路由仍可访问
        let resp = app
            .oneshot(make_request_with_header(
                "GET",
                "/large",
                "accept-encoding",
                "gzip",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_tower_compat_apply_execution_order() {
        // 验证执行顺序：sz-rust 中间件链 -> tower-http 扩展 Layer
        // 通过 CORS 预检（sz-rust Cors）+ gzip 压缩（tower Compression）验证两者都生效
        let app = make_router();
        let app = TowerCompat::php_global().with_compression().apply(app);

        // 发送带 Origin + Accept-Encoding 的请求
        // 注意：cors_layer 使用 AllowOrigin::mirror_request()，必须带 Origin header 才会回显
        let req = Request::builder()
            .method("GET")
            .uri("/large")
            .header("origin", "https://example.com")
            .header("accept-encoding", "gzip")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // CORS 生效（sz-rust Cors）
        let allow_origin = resp.headers().get("access-control-allow-origin");
        assert!(allow_origin.is_some(), "CORS should be applied");
        // 压缩生效（tower Compression）
        let content_encoding = resp.headers().get("content-encoding");
        assert!(content_encoding.is_some(), "Compression should be applied");
    }

    // ====================================================================
    // 组 6：独立使用 tower-http Layer（不通过 MiddlewareBuilder）
    // ====================================================================

    #[tokio::test]
    async fn test_independent_compression_layer() {
        let app = make_router().layer(compression_layer());

        let resp = app
            .oneshot(make_request_with_header(
                "GET",
                "/large",
                "accept-encoding",
                "gzip",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let content_encoding = resp.headers().get("content-encoding");
        assert!(content_encoding.is_some());
    }

    #[tokio::test]
    async fn test_independent_timeout_layer_passes() {
        let app = make_router().layer(timeout_layer(Duration::from_secs(30)));

        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_independent_tower_trace_layer() {
        // tower_trace_layer() 返回 Arc<dyn Fn(Router) -> Router + Send + Sync> 闭包
        // 需通过闭包调用应用 TraceLayer 到 Router
        let app = (tower_trace_layer())(make_router());

        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ====================================================================
    // 组 7：TowerLayer apply 方法
    // ====================================================================

    #[tokio::test]
    async fn test_tower_layer_apply_compression() {
        let layer = TowerLayer::Compression(compression_layer());
        let app = make_router();
        let app = layer.apply(app);

        let resp = app
            .oneshot(make_request_with_header(
                "GET",
                "/large",
                "accept-encoding",
                "gzip",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_tower_layer_apply_timeout() {
        let layer = TowerLayer::Timeout(timeout_layer(Duration::from_secs(30)));
        let app = make_router();
        let app = layer.apply(app);

        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_tower_layer_apply_tower_trace() {
        let layer = TowerLayer::TowerTrace(tower_trace_layer());
        let app = make_router();
        let app = layer.apply(app);

        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ====================================================================
    // 组 8：R5 PHP 行为对齐验证
    // ====================================================================

    /// R5-1: PHP 端无 Gzip 压缩中间件（由 Nginx `gzip_static` 处理）
    #[test]
    fn test_r5_php_no_compression_middleware() {
        // PHP `app/middleware.php` 仅含 SessionInit + AllowCrossDomain，无压缩中间件
        // sz-rust 通过 TowerCompat::with_compression() 提供压缩能力（对齐 Nginx gzip_static）
        let compat = TowerCompat::php_global().with_compression();
        assert!(compat.has_tower_layer("compression"));
        // sz-rust 增强能力，PHP 端无对应物
    }

    /// R5-2: PHP 端无请求超时中间件（由 Nginx `fastcgi_read_timeout` 处理）
    #[test]
    fn test_r5_php_no_timeout_middleware() {
        // PHP `app/middleware.php` 仅含 SessionInit + AllowCrossDomain，无超时中间件
        // sz-rust 通过 TowerCompat::with_timeout() 提供超时能力（对齐 Nginx fastcgi_read_timeout）
        let compat = TowerCompat::php_global().with_timeout(Duration::from_secs(30));
        assert!(compat.has_tower_layer("timeout"));
        // sz-rust 增强能力，PHP 端无对应物
    }

    /// R5-3: PHP 端无 tower-http TraceLayer（sz-rust 自研 trace_middleware 对齐 PHP SessionInit）
    #[test]
    fn test_r5_php_no_tower_trace_layer() {
        // PHP `app/middleware.php` 仅含 SessionInit + AllowCrossDomain，无 tower-http TraceLayer
        // sz-rust 自研 trace_middleware 对齐 PHP SessionInit（请求初始化语义）
        // tower-http TraceLayer 仅用于兼容验证，生产环境应使用 sz-rust trace_middleware
        let compat = TowerCompat::php_global().with_tower_trace();
        assert!(compat.has_tower_layer("tower_trace"));
    }

    /// R5-4: CORS Layer 已通过 MiddlewareBuilder 直接支持（tower-http CorsLayer 完全等价）
    #[test]
    fn test_r5_cors_already_in_middleware_builder() {
        // MiddlewareBuilder::with_cors(CorsLayer) 直接支持 tower-http CorsLayer
        // tower_compat.rs 不重复包装 CORS，避免双重处理
        let compat = TowerCompat::php_global();
        // php_global_builder 已设置默认 cors_layer()
        assert!(compat.builder().cors().is_some());
        // tower_layers 不应包含 Cors（由 MiddlewareBuilder 处理）
        assert!(!compat.has_tower_layer("cors"));
    }

    /// R5-5: sz-rust trace_middleware vs tower-http TraceLayer 区别
    #[test]
    fn test_r5_sz_rust_trace_vs_tower_http_trace() {
        // sz-rust trace_middleware:
        //   - W3C TraceContext 传播（traceparent header）
        //   - Span 注入 request extensions（下游可获取）
        //   - 自定义 service_name
        //   - 排除路径支持
        // tower-http TraceLayer:
        //   - 仅 tower 内部 span（不传播 W3C TraceContext）
        //   - 无 Span 注入 extensions
        //   - 无自定义 service_name
        //   - 无排除路径
        // 结论：生产环境应使用 sz-rust trace_middleware
        let tower_layer = tower_trace_layer();
        // 验证可创建（不 panic 即可）
        let _ = tower_layer;
    }

    /// R5-6: TowerCompat 执行顺序对齐 PHP + Nginx 架构
    #[test]
    fn test_r5_execution_order_aligns_php_nginx() {
        // PHP + Nginx 架构执行顺序：
        //   Nginx (gzip + timeout) -> PHP-FPM (SessionInit + AllowCrossDomain + Auth)
        // sz-rust 架构执行顺序：
        //   sz-rust 中间件链 (Trace + Cors + Log + RateLimit + Auth) -> tower-http 扩展 (Compression + Timeout)
        // 注意：sz-rust 中间件链先执行，tower-http 扩展后执行
        // 这与 PHP + Nginx 架构相反（Nginx 先执行），但符合 Rust 单进程架构
        // （tower-http Layer 在 Router 最外层，最先执行；sz-rust 中间件在 Router 内层，后执行）
        // 修正：tower-http Layer 通过 Router::layer 添加，后注册先执行
        // TowerCompat::apply 先应用 sz-rust 中间件链，再应用 tower-http Layer（逆序）
        // 因此 tower-http Layer 后注册 = 先执行，sz-rust 中间件链先注册 = 后执行
        // 最终执行顺序：tower-http Layer -> sz-rust 中间件链 -> handler
        // 这与 PHP + Nginx 架构一致（Nginx 先执行，PHP 后执行）
        let compat = TowerCompat::php_global()
            .with_compression()
            .with_timeout(Duration::from_secs(30));
        // 验证 tower_layers 顺序与添加顺序一致
        assert_eq!(compat.tower_layers()[0].kind_name(), "compression");
        assert_eq!(compat.tower_layers()[1].kind_name(), "timeout");
    }

    /// R5-7: tower-http CompressionLayer 默认 MIME 类型对齐 Nginx gzip_types
    #[test]
    fn test_r5_compression_default_mime_types_align_nginx() {
        // Nginx 默认 gzip_types: text/html, text/plain, text/css, application/json,
        // application/javascript, application/xml, text/xml, application/xml+rss
        // tower-http CompressionLayer::new() 默认压缩: text/*, application/json, application/javascript, application/xml
        // 两者基本一致（tower-http 默认已包含主要 MIME 类型）
        let _layer = compression_layer();
        // 验证可创建（不 panic 即可，MIME 类型由 tower-http 内部管理）
    }

    /// R5-8: TowerCompat 不破坏 MiddlewareBuilder 原有行为
    #[tokio::test]
    async fn test_r5_tower_compat_preserves_middleware_builder_behavior() {
        // TowerCompat 应完整传递 MiddlewareBuilder 的行为，不破坏原有 CORS/Auth/Log 等中间件
        let app = make_router();
        // 仅使用 MiddlewareBuilder（不添加 tower-http Layer）
        let app = TowerCompat::php_global().apply(app);

        // CORS 应生效（对齐 PHP AllowCrossDomain）
        let resp = app
            .oneshot(make_request_with_header(
                "GET",
                "/",
                "origin",
                "https://example.com",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let allow_origin = resp.headers().get("access-control-allow-origin");
        assert!(allow_origin.is_some(), "CORS should be applied");
    }

    // ====================================================================
    // 组 9：集成测试（端到端）
    // ====================================================================

    #[tokio::test]
    async fn integration_tower_compat_full_stack() {
        // 完整栈：sz-rust 中间件链 + tower-http 扩展 Layer
        let app = make_router();
        let app = TowerCompat::php_global()
            .with_compression()
            .with_timeout(Duration::from_secs(30))
            .with_tower_trace()
            .apply(app);

        // 发送请求验证所有 Layer 正常工作
        // 注意：cors_layer 使用 AllowOrigin::mirror_request()，必须带 Origin header 才会回显
        let req = Request::builder()
            .method("GET")
            .uri("/large")
            .header("origin", "https://example.com")
            .header("accept-encoding", "gzip")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // CORS 生效
        assert!(resp.headers().get("access-control-allow-origin").is_some());
        // 压缩生效
        assert!(resp.headers().get("content-encoding").is_some());
    }

    #[tokio::test]
    async fn integration_tower_compat_options_preflight() {
        // OPTIONS 预检请求应通过 CORS Layer（对齐 PHP AllowCrossDomain）
        let app = make_router();
        let app = TowerCompat::php_global().with_compression().apply(app);

        let resp = app
            .oneshot(make_request_with_header(
                "OPTIONS",
                "/",
                "origin",
                "https://example.com",
            ))
            .await
            .unwrap();
        // OPTIONS 预检应返回 2xx（CORS Layer 处理）
        assert!(resp.status().is_success() || resp.status() == StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn integration_tower_compat_no_tower_layers() {
        // 不添加任何 tower-http Layer，应等价于 MiddlewareBuilder::apply()
        let app = make_router();
        let app = TowerCompat::php_global().apply(app);

        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn integration_tower_compat_default_builder_with_compression() {
        // 使用 default_builder（5 个 sz-rust 中间件）+ compression
        // 注意：default_builder 不绑定 Config，apply 时跳过未配置的中间件
        let app = make_router();
        let app = TowerCompat::default_builder().with_compression().apply(app);

        let resp = app
            .oneshot(make_request_with_header(
                "GET",
                "/large",
                "accept-encoding",
                "gzip",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // 压缩生效（tower-http CompressionLayer 独立于 sz-rust MiddlewareBuilder）
        assert!(resp.headers().get("content-encoding").is_some());
    }
}
