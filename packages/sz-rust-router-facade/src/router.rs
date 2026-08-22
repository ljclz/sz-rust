//! 路由模块 — 路由构建（基于 axum::Router）
//!
//! 对齐 PHP `with_route=true` + `auto_multi_app=true` + `app/controller/action` 路径解析。
//!
//! ## 功能
//!
//! - [`parse_path`]：将 URI 解析为 `(app, controller, action)` 三元组
//! - [`ParsedPath`]：解析结果结构体
//! - [`RouterBuilder`]：链式构建 axum::Router（支持 GET/POST/PUT/DELETE/资源路由）
//!
//! ## PHP 对齐
//!
//! 对齐 `config/app.php` + `config/route.php`：
//!
//! | PHP 配置项 | 值 | Rust 行为 |
//! |-----------|----|----------|
//! | `auto_multi_app` | `true` | 启用多应用解析 |
//! | `app_map` | `oapc/admin/api/farm/oapi/cashier/scene` | 应用白名单 |
//! | `default_app` | `index` | URI 无应用前缀时使用 |
//! | `default_controller` | `Index` | URI 无控制器时使用 |
//! | `default_action` | `index` | URI 无操作时使用 |
//! | `deny_app_list` | `['common']` | 拒绝访问 `common` |
//! | `controller_layer` | `controller` | 控制器层名 |
//! | `pathinfo_depr` | `/` | 路径分隔符 |
//! | `empty_controller` | `Error` | 空控制器名（暂未实现） |

use axum::routing::{delete as route_delete, get, head, patch, post, put};

use axum::Router;

use std::borrow::Cow;

/// 已注册的应用白名单
///
/// 对齐 PHP `app_map`：`oapc / admin / api / farm / oapi / cashier / scene`
///
/// v0.3.2 优化：改 const 数组 + 线性查找，消除 HashSet hash + LazyLock 初始化开销。
/// 7 元素线性查找比 HashSet 更快（无 hash 计算 + 无桶分布）。
pub const APP_LIST: &[&str] = &["oapc", "admin", "api", "farm", "oapi", "cashier", "scene"];

/// 禁止访问的应用列表
///
/// 对齐 PHP `deny_app_list = ['common']`
pub const DENY_LIST: &[&str] = &["common"];

/// 默认应用名
pub const DEFAULT_APP: &str = "index";
/// 默认控制器名
pub const DEFAULT_CONTROLLER: &str = "Index";
/// 默认操作名
pub const DEFAULT_ACTION: &str = "index";

/// 路径解析结果
///
/// 对应 PHP 自动多应用解析出的 `(app, controller, action)` 三元组。
///
/// ## 字段说明
///
/// - `app`：应用名（如 `oapc` / `admin` / `index`）
/// - `controller`：控制器名（PHP 习惯首字母大写，如 `Customer`）
/// - `action`：操作名（小驼峰，如 `index` / `getList`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPath<'a> {
    /// 应用名
    pub app: Cow<'a, str>,
    /// 控制器名（首字母大写）
    pub controller: Cow<'a, str>,
    /// 操作名（小驼峰）
    pub action: Cow<'a, str>,
}

impl<'a> ParsedPath<'a> {
    /// 构造函数（用于测试便捷）
    ///
    /// 接受 `impl Into<Cow<'a, str>>`，兼容 `&str`（→ Borrowed 零分配）和 `String`（→ Owned）。
    #[inline]
    pub fn new(
        app: impl Into<Cow<'a, str>>,
        controller: impl Into<Cow<'a, str>>,
        action: impl Into<Cow<'a, str>>,
    ) -> Self {
        Self {
            app: app.into(),
            controller: controller.into(),
            action: action.into(),
        }
    }

    /// 兼容层：消费 self 转为 owned `(String, String, String)`。
    ///
    /// 供需要 owned String 的调用方使用（如 `format!` 拼接、序列化）。
    #[inline]
    pub fn into_strings(self) -> (String, String, String) {
        (
            self.app.into_owned(),
            self.controller.into_owned(),
            self.action.into_owned(),
        )
    }
}

impl<'a> From<ParsedPath<'a>> for (String, String, String) {
    fn from(p: ParsedPath<'a>) -> Self {
        p.into_strings()
    }
}

/// 按 '/' 切分第一段（跳过空段）— P3 SIMD 加速
///
/// 返回 `(第一段, 剩余部分)`。跳过空段（连续 '/' 视为单个分隔符）。
/// 如果输入为空或全为 '/'，返回 `(None, "")`。
#[inline]
fn split_first_segment(s: &str) -> (Option<&str>, &str) {
    let bytes = s.as_bytes();
    let mut start = 0;

    // 跳过前导 '/'
    while start < bytes.len() && bytes[start] == b'/' {
        start += 1;
    }

    if start >= bytes.len() {
        return (None, "");
    }

    // 查找下一个 '/' — P3 SIMD 加速
    let rest = &s[start..];
    match crate::simd_str::find_separator_simd(rest.as_bytes(), b'/') {
        Some(pos) => (Some(&rest[..pos]), &rest[pos..]),
        None => (Some(rest), ""),
    }
}

/// 解析 URI 路径为 `(: `(app, controller, action)` 三元组
///
/// 对齐 PHP `auto_multi_app` 解析规则：
///
/// - `/` → `(index, Index, index)`
/// - `/foo` → `(index, Foo, index)`
/// - `/foo/bar` → `(index, Foo, bar)`
/// - `/oapc/foo/bar` → `(oapc, Foo, bar)`（当 `oapc` 在 `app_map` 中）
/// - `/common/foo/bar` → `(index, Common, foo)`（`common` 在 `deny_app_list` 中，当作控制器处理）
///
/// ## 参数
///
/// - `uri`：请求 URI（如 `/oapc/customer/index?id=1`），查询字符串会被自动剥离
///
/// ## 返回
///
/// 返回 [`ParsedPath`]，永不为空。
///
/// # Examples
///
/// ```rust
/// use sz_rust_router_facade::router::parse_path;
///
/// let p = parse_path("/oapc/customer/detail?id=3");
/// assert_eq!(p.app, "oapc");
/// assert_eq!(p.controller, "Customer");
/// assert_eq!(p.action, "detail");
///
/// // 单级路径 → 默认应用 + Index 控制器
/// let root = parse_path("/");
/// assert_eq!(root.controller, "Index");
/// ```
#[inline]
pub fn parse_path<'a>(uri: &'a str) -> ParsedPath<'a> {
    // 剥离查询字符串 — P3 SIMD 加速
    let path = match crate::simd_str::find_separator_simd(uri.as_bytes(), b'?') {
        Some(pos) => &uri[..pos],
        None => uri,
    };

    // 剥离去前导 '/'
    let path = path.trim_start_matches('/');

    // 空路径 → 全部默认
    if path.is_empty() {
        return ParsedPath::new(DEFAULT_APP, DEFAULT_CONTROLLER, DEFAULT_ACTION);
    }

    // 按路径分隔符切分 — P3 SIMD 加速
    // 使用 find_separator_simd 逐段切分，避免 split 迭代器开销
    let (seg0, rest) = split_first_segment(path);
    let (seg1, rest) = split_first_segment(rest);
    let (seg2, _rest) = split_first_segment(rest);

    match (seg0, seg1, seg2) {
        // 空路径（已被上面处理，此处防御式）
        (None, _, _) => ParsedPath::new(DEFAULT_APP, DEFAULT_CONTROLLER, DEFAULT_ACTION),
        // /foo → (index, Foo, index)
        (Some(s0), None, _) => ParsedPath::new(DEFAULT_APP, capitalize_first(s0), DEFAULT_ACTION),
        // /foo/bar 或 /app/foo/bar
        (Some(s0), Some(s1), None) => {
            if is_app_in_map(s0) {
                ParsedPath::new(s0, capitalize_first(s1), DEFAULT_ACTION)
            } else {
                ParsedPath::new(DEFAULT_APP, capitalize_first(s0), s1)
            }
        }
        // /app/foo/bar 或 /foo/bar/baz 或更多
        (Some(s0), Some(s1), Some(s2)) => {
            if is_app_in_map(s0) {
                ParsedPath::new(s0, capitalize_first(s1), s2)
            } else {
                ParsedPath::new(DEFAULT_APP, capitalize_first(s0), s1)
            }
        }
    }
}

/// 判断字符串是否在 `app_map` 中
///
/// v0.3.2 优化：const 数组线性查找，消除 HashSet hash 开销。
/// 7 元素线性比较 ~7ns，HashSet 查找 ~15ns（hash + 桶探测）。
#[inline]
pub fn is_app_in_map(name: &str) -> bool {
    APP_LIST.contains(&name) && !DENY_LIST.contains(&name)
}

/// 首字母大写（对齐 PHP 控制器命名）
///
/// `customer` → `Customer`，`Customer` → `Customer`，`get_list` → `Get_list`（仅首字母大写）
///
/// v0.3.3 优化：返回 `Cow<'_, str>`，ASCII 首字母已大写时零分配（`Cow::Borrowed`），
/// 未大写时 1 次分配（`Cow::Owned`），非 ASCII 回退到 `chars()` 迭代路径。
///
/// v0.3.4 优化：未大写分支改用 `Vec<u8>` 直接字节操作 + `String::from_utf8`，
/// 消除 `String::push` 的 char 编码开销和 `push_str` 的 UTF-8 验证开销。
///
/// v0.6.x P3 优化：x86_64 平台调用 `simd_str::capitalize_first_simd`，
/// 使用 SSE2 并行 ASCII 检测加速，非 x86_64 回退标量。
#[inline]
pub fn capitalize_first(s: &str) -> Cow<'_, str> {
    crate::simd_str::capitalize_first_simd(s)
}

/// 路由构建器
///
/// 对 axum::Router 的薄封装，提供链式 API（含 PHP 风格的
/// `group()` / `nest()` / `resource()` 等分组能力）。
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_router_facade::router::RouterBuilder;
/// use axum::Json;
///
/// let router = RouterBuilder::new()
///     .get("/ping", || async { "pong" })
///     .post("/echo", |Json(v): Json<serde_json::Value>| async move { Json(v) })
///     .build();
/// ```
/// 路由构建器（泛型状态支持）
///
/// `S` 为应用共享状态类型，由首次注册的路由 handler 推断。
/// 当 handler 使用 `axum::extract::State<S>` 时，`S` 自动确定为该状态类型。
///
/// ## 用法
///
/// ```rust,ignore
/// // 无状态路由
/// let router = RouterBuilder::new()
///     .get("/ping", ping_handler)
///     .build();
///
/// // 有状态路由（S 由 handler 推断为 AppState）
/// let router = RouterBuilder::new()
///     .get("/items", list_items)  // list_items 使用 State<AppState>
///     .with_state(AppState::default())
///     .build();
/// ```
pub struct RouterBuilder<S = ()> {
    inner: Router<S>,
}

impl<S> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    /// 创建空的 RouterBuilder
    pub fn new() -> Self {
        Self {
            inner: Router::new(),
        }
    }

    /// 从已有 Router 起步
    pub fn with_router(inner: Router<S>) -> Self {
        Self { inner }
    }

    /// 注入应用状态并转换状态类型
    ///
    /// 当注册的 handler 使用了 `axum::extract::State<S>` 时调用，
    /// 将 `RouterBuilder<S>` 转换为 `RouterBuilder<S2>`（通常 `S2 = S`）。
    ///
    /// ## 用法
    ///
    /// ```rust,ignore
    /// let builder = RouterBuilder::new()
    ///     .get("/items", list_items)
    ///     .with_state(AppState { db: pool });
    /// ```
    pub fn with_state<S2>(self, state: S) -> RouterBuilder<S2> {
        RouterBuilder {
            inner: self.inner.with_state(state),
        }
    }

    /// 注册 GET 路由
    pub fn get<H, T>(self, path: &str, handler: H) -> Self
    where
        H: axum::handler::Handler<T, S>,
        T: 'static,
    {
        Self {
            inner: self.inner.route(path, get(handler)),
        }
    }

    /// 注册 POST 路由
    pub fn post<H, T>(self, path: &str, handler: H) -> Self
    where
        H: axum::handler::Handler<T, S>,
        T: 'static,
    {
        Self {
            inner: self.inner.route(path, post(handler)),
        }
    }

    /// 注册 PUT 路由
    pub fn put<H, T>(self, path: &str, handler: H) -> Self
    where
        H: axum::handler::Handler<T, S>,
        T: 'static,
    {
        Self {
            inner: self.inner.route(path, put(handler)),
        }
    }

    /// 注册 DELETE 路由
    pub fn delete<H, T>(self, path: &str, handler: H) -> Self
    where
        H: axum::handler::Handler<T, S>,
        T: 'static,
    {
        Self {
            inner: self.inner.route(path, route_delete(handler)),
        }
    }

    /// 注册 PATCH 路由
    pub fn patch<H, T>(self, path: &str, handler: H) -> Self
    where
        H: axum::handler::Handler<T, S>,
        T: 'static,
    {
        Self {
            inner: self.inner.route(path, patch(handler)),
        }
    }

    /// 注册 HEAD 路由
    pub fn head<H, T>(self, path: &str, handler: H) -> Self
    where
        H: axum::handler::Handler<T, S>,
        T: 'static,
    {
        Self {
            inner: self.inner.route(path, head(handler)),
        }
    }

    /// 注册 OPTIONS 路由
    pub fn options<H, T>(self, path: &str, handler: H) -> Self
    where
        H: axum::handler::Handler<T, S>,
        T: 'static,
    {
        Self {
            inner: self.inner.route(
                path,
                axum::routing::on(axum::routing::MethodFilter::OPTIONS, handler),
            ),
        }
    }

    /// 注册 WebSocket 路由（原生框架集成，无需独立端口）
    ///
    /// 在主 HTTP 端口上处理 WebSocket 升级请求（如 `GET /ws/chat`）。
    /// 对齐 PHP Workerman `websocket://` 协议，但复用 HTTP 端口。
    ///
    /// ## 用法
    ///
    /// ```ignore
    /// use sz_rust_router_facade::router::RouterBuilder;
    /// use sz_rust_router_facade::websocket_route::EchoWsHandler;
    ///
    /// let router = RouterBuilder::new()
    ///     .ws("/ws/echo", EchoWsHandler::new())
    ///     .build();
    /// ```
    pub fn ws<H: crate::websocket_route::WsHandler>(self, path: &str, handler: H) -> Self {
        let mr: axum::routing::MethodRouter<()> = crate::websocket_route::ws_handler(handler);
        let mr_s: axum::routing::MethodRouter<S> = mr.with_state(());
        Self {
            inner: self.inner.route(path, mr_s),
        }
    }

    /// 应用一个 tower::Layer
    ///
    /// 约束与 `axum::Router::layer` 一致，便于直接链式调用任何 tower-http 中间件。
    pub fn layer<L>(self, layer: L) -> Self
    where
        L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: tower::Service<axum::extract::Request> + Clone + Send + Sync + 'static,
        <L::Service as tower::Service<axum::extract::Request>>::Response:
            axum::response::IntoResponse + 'static,
        <L::Service as tower::Service<axum::extract::Request>>::Error: Into<Infallible> + 'static,
        <L::Service as tower::Service<axum::extract::Request>>::Future: Send + 'static,
    {
        Self {
            inner: self.inner.layer(layer),
        }
    }

    /// 合并另一个 Router
    pub fn merge(self, other: Router<S>) -> Self {
        Self {
            inner: self.inner.merge(other),
        }
    }

    /// 嵌套另一个 Router 到指定前缀下（对齐 axum::Router::nest）
    ///
    /// ## 用法
    ///
    /// ```ignore
    /// let api_routes = RouterBuilder::new()
    ///     .get("/users", get_users)
    ///     .build();
    ///
    /// let router = RouterBuilder::new()
    ///     .nest("/api", api_routes)
    ///     .build();
    /// // GET /api/users -> get_users
    /// ```
    pub fn nest(self, prefix: &str, other: Router<S>) -> Self {
        Self {
            inner: self.inner.nest(prefix, other),
        }
    }

    /// 路由分组：返回一个新的 RouterBuilder，后续注册的路由自动添加前缀
    ///
    /// 对齐 PHP `Route::group()` + axum `Router::nest` 语义。
    /// 内部实现：创建子 RouterBuilder，通过 nest 挂载到前缀。
    ///
    /// ## 用法
    ///
    /// ```ignore
    /// let router = RouterBuilder::new()
    ///     .group("/api", |group| {
    ///         group.get("/users", get_users)
    ///              .post("/users", create_user)
    ///     })
    ///     .build();
    /// // GET /api/users -> get_users
    /// // POST /api/users -> create_user
    /// ```
    pub fn group<F>(self, prefix: &str, f: F) -> Self
    where
        F: FnOnce(RouterBuilder<S>) -> RouterBuilder<S>,
    {
        let sub_builder = f(RouterBuilder::new());
        self.nest(prefix, sub_builder.build())
    }

    /// 构建最终的 axum::Router
    pub fn build(self) -> Router<S> {
        self.inner
    }
}

impl Default for RouterBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// 仅供编译期验证 axum::Layer 类型约束使用
use std::convert::Infallible;

// ============================================================================
// RESTful 资源路由（1.2.5）
// ============================================================================

/// RESTful 资源路由的 7 个标准 handler（每个可选）
///
/// 借鉴 Rails `resources :users` + Laravel `Route::resource`，一行注册
/// 全部标准 RESTful 路由。
///
/// ## 标准路由映射
///
/// | Method   | Path                  | Handler   | 说明 |
/// |----------|----------------------|-----------|------|
/// | GET      | `/{name}`            | `index`   | 列表 |
/// | GET      | `/{name}/create`     | `create`  | 新建表单（PHP 风格） |
/// | POST     | `/{name}`            | `store`   | 保存 |
/// | GET      | `/{name}/{id}`       | `show`    | 详情 |
/// | GET      | `/{name}/{id}/edit`  | `edit`    | 编辑表单 |
/// | PUT      | `/{name}/{id}`       | `update`  | 更新 |
/// | DELETE   | `/{name}/{id}`       | `destroy` | 删除 |
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_router_facade::router::{resource, ResourceRoutes};
/// use axum::routing::{get, post, put, delete};
///
/// let routes = ResourceRoutes {
///     index: Some(get(|| async { "list" })),
///     show: Some(get(|| async { "show" })),
///     store: Some(post(|| async { "create" })),
///     update: Some(put(|| async { "update" })),
///     destroy: Some(delete(|| async { "delete" })),
///     ..Default::default()
/// };
/// let router = resource("users", routes);
/// ```
#[derive(Default)]
pub struct ResourceRoutes {
    /// GET `/{name}` — 列表
    pub index: Option<axum::routing::MethodRouter>,
    /// GET `/{name}/create` — 新建表单
    pub create: Option<axum::routing::MethodRouter>,
    /// POST `/{name}` — 保存
    pub store: Option<axum::routing::MethodRouter>,
    /// GET `/{name}/{id}` — 详情
    pub show: Option<axum::routing::MethodRouter>,
    /// GET `/{name}/{id}/edit` — 编辑表单
    pub edit: Option<axum::routing::MethodRouter>,
    /// PUT `/{name}/{id}` — 更新
    pub update: Option<axum::routing::MethodRouter>,
    /// DELETE `/{name}/{id}` — 删除
    pub destroy: Option<axum::routing::MethodRouter>,
}

impl ResourceRoutes {
    /// 创建空 routes（所有 handler 都为 None）
    pub fn new() -> Self {
        Self::default()
    }
}

/// 构造 RESTful 资源路由
///
/// 根据 `ResourceRoutes` 中提供的非 None handler，注册对应的标准路由。
/// 缺省的 handler（None）将不会被注册（请求返回 404）。
///
/// ## 参数
///
/// - `name`：资源名称（如 `"users"`），用于构造路径 `/{name}` 和 `/{name}/{id}`
/// - `routes`：7 个可选 handler
///
/// ## 返回
///
/// 一个独立的 `axum::Router`，可与其他 Router `merge()` 合并。
///
/// ## 路径模板
///
/// - `/{name}` → GET index + POST store（合并到同一 MethodRouter）
/// - `/{name}/create` → GET create
/// - `/{name}/{id}` → GET show + PUT update + DELETE destroy
/// - `/{name}/{id}/edit` → GET edit
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_router_facade::router::{resource, ResourceRoutes};
///
/// let routes = ResourceRoutes {
///     index: Some(axum::routing::get(|| async { "list" })),
///     store: Some(axum::routing::post(|| async { "create" })),
///     ..Default::default()
/// };
/// let router = resource("users", routes);
/// ```
pub fn resource(name: &str, routes: ResourceRoutes) -> axum::Router {
    let base = format!("/{name}");
    let with_id = format!("/{name}/{{id}}");
    let create_path = format!("/{name}/create");
    let edit_path = format!("/{name}/{{id}}/edit");

    let mut router = axum::Router::new();

    // /{name}: GET index + POST store
    let mut base_methods = axum::routing::MethodRouter::new();
    let mut has_base = false;
    if let Some(h) = routes.index {
        base_methods = base_methods.merge(h);
        has_base = true;
    }
    if let Some(h) = routes.store {
        base_methods = base_methods.merge(h);
        has_base = true;
    }
    if has_base {
        router = router.route(&base, base_methods);
    }

    // /{name}/create: GET create
    if let Some(h) = routes.create {
        router = router.route(&create_path, h);
    }

    // /{name}/{id}: GET show + PUT update + DELETE destroy
    let mut id_methods = axum::routing::MethodRouter::new();
    let mut has_id = false;
    if let Some(h) = routes.show {
        id_methods = id_methods.merge(h);
        has_id = true;
    }
    if let Some(h) = routes.update {
        id_methods = id_methods.merge(h);
        has_id = true;
    }
    if let Some(h) = routes.destroy {
        id_methods = id_methods.merge(h);
        has_id = true;
    }
    if has_id {
        router = router.route(&with_id, id_methods);
    }

    // /{name}/{id}/edit: GET edit
    if let Some(h) = routes.edit {
        router = router.route(&edit_path, h);
    }

    router
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // ====================================================================
    // parse_path 单元测试
    // ====================================================================

    #[test]
    fn test_parse_path_root() {
        let p = parse_path("/");
        assert_eq!(p, ParsedPath::new("index", "Index", "index"));
    }

    #[test]
    fn test_parse_path_empty() {
        let p = parse_path("");
        assert_eq!(p, ParsedPath::new("index", "Index", "index"));
    }

    #[test]
    fn test_parse_path_single_segment() {
        let p = parse_path("/customer");
        assert_eq!(p, ParsedPath::new("index", "Customer", "index"));
    }

    #[test]
    fn test_parse_path_two_segments_no_app() {
        let p = parse_path("/customer/list");
        assert_eq!(p, ParsedPath::new("index", "Customer", "list"));
    }

    #[test]
    fn test_parse_path_three_segments_with_app() {
        let p = parse_path("/oapc/customer/index");
        assert_eq!(p, ParsedPath::new("oapc", "Customer", "index"));
    }

    #[test]
    fn test_parse_path_app_in_map_two_segments() {
        let p = parse_path("/admin/login");
        assert_eq!(p, ParsedPath::new("admin", "Login", "index"));
    }

    #[test]
    fn test_parse_path_all_seven_apps() {
        for app in ["oapc", "admin", "api", "farm", "oapi", "cashier", "scene"] {
            let uri = format!("/{app}/customer/index");
            let p = parse_path(&uri);
            assert_eq!(p, ParsedPath::new(app, "Customer", "index"));
        }
    }

    #[test]
    fn test_parse_path_deny_common_app() {
        // common 在 deny_app_list 中，应当被当作控制器处理
        let p = parse_path("/common/customer/index");
        assert_eq!(p, ParsedPath::new("index", "Common", "customer"));
    }

    #[test]
    fn test_parse_path_with_query_string() {
        let p = parse_path("/oapc/customer/index?id=1&page=2");
        assert_eq!(p, ParsedPath::new("oapc", "Customer", "index"));
    }

    #[test]
    fn test_parse_path_with_trailing_slash() {
        let p = parse_path("/oapc/customer/index/");
        assert_eq!(p, ParsedPath::new("oapc", "Customer", "index"));
    }

    #[test]
    fn test_parse_path_double_slash() {
        let p = parse_path("//oapc//customer//index");
        assert_eq!(p, ParsedPath::new("oapc", "Customer", "index"));
    }

    #[test]
    fn test_parse_path_capitalize_first_only() {
        // 仅首字母大写，不强制后续小写
        let p = parse_path("/customerList");
        assert_eq!(p, ParsedPath::new("index", "CustomerList", "index"));
    }

    #[test]
    fn capitalize_first_empty() {
        assert_eq!(capitalize_first(""), "");
    }

    #[test]
    fn capitalize_first_ascii_upper() {
        assert_eq!(capitalize_first("Customer"), "Customer");
    }

    #[test]
    fn capitalize_first_ascii_lower_short() {
        assert_eq!(capitalize_first("customer"), "Customer");
    }

    #[test]
    fn capitalize_first_ascii_lower_exact_24() {
        let input = "aaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(input.len(), 24);
        assert_eq!(capitalize_first(input), "Aaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn capitalize_first_ascii_lower_overflow_25() {
        let input = "aaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(input.len(), 25);
        assert_eq!(capitalize_first(input), "Aaaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn capitalize_first_non_ascii() {
        assert_eq!(capitalize_first("中文"), "中文");
    }

    #[test]
    fn test_is_app_in_map_all_seven() {
        for app in ["oapc", "admin", "api", "farm", "oapi", "cashier", "scene"] {
            assert!(is_app_in_map(app), "{app} should be in app_map");
        }
    }

    #[test]
    fn test_is_app_in_map_deny_common() {
        assert!(!is_app_in_map("common"));
    }

    #[test]
    fn test_is_app_in_map_unknown_app() {
        assert!(!is_app_in_map("unknown"));
        assert!(!is_app_in_map(""));
    }

    // ====================================================================
    // RouterBuilder 单元测试
    // ====================================================================

    #[tokio::test]
    async fn test_router_builder_get() {
        let router = RouterBuilder::new()
            .get("/ping", || async { "pong" })
            .build();
        let request = Request::builder()
            .method(Method::GET)
            .uri("/ping")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"pong");
    }

    #[tokio::test]
    async fn test_router_builder_post() {
        let router = RouterBuilder::new()
            .post("/echo", |body: Body| async move {
                let bytes = body
                    .collect()
                    .await
                    .map_err(|_| ())
                    .map(|b| b.to_bytes())
                    .unwrap_or_default();
                String::from_utf8_lossy(&bytes).to_string()
            })
            .build();
        let request = Request::builder()
            .method(Method::POST)
            .uri("/echo")
            .body(Body::from("hello"))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"hello");
    }

    #[tokio::test]
    async fn test_router_builder_multiple_methods() {
        let router = RouterBuilder::new()
            .get("/items", || async { "list" })
            .post("/items", || async { "create" })
            .put("/items/1", || async { "update" })
            .delete("/items/1", || async { "delete" })
            .build();

        // GET
        let req = Request::builder()
            .method(Method::GET)
            .uri("/items")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // POST
        let req = Request::builder()
            .method(Method::POST)
            .uri("/items")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // PUT
        let req = Request::builder()
            .method(Method::PUT)
            .uri("/items/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // DELETE
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/items/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_router_builder_not_found() {
        let router = RouterBuilder::new()
            .get("/ping", || async { "pong" })
            .build();
        let request = Request::builder()
            .method(Method::GET)
            .uri("/unknown")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_router_builder_merge() {
        let r1 = RouterBuilder::new().get("/a", || async { "A" }).build();
        let r2 = RouterBuilder::new().get("/b", || async { "B" }).build();
        let router = RouterBuilder::new().merge(r1).merge(r2).build();

        for path in ["/a", "/b"] {
            let request = Request::builder()
                .method(Method::GET)
                .uri(path)
                .body(Body::empty())
                .unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn test_router_builder_default() {
        let router = RouterBuilder::default().build();
        let request = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ====================================================================
    // resource() RESTful 资源路由测试（1.2.5）
    // ====================================================================

    #[tokio::test]
    async fn test_resource_all_seven_handlers() {
        let routes = ResourceRoutes {
            index: Some(axum::routing::get(|| async { "index" })),
            create: Some(axum::routing::get(|| async { "create" })),
            store: Some(axum::routing::post(|| async { "store" })),
            show: Some(axum::routing::get(|| async { "show" })),
            edit: Some(axum::routing::get(|| async { "edit" })),
            update: Some(axum::routing::put(|| async { "update" })),
            destroy: Some(axum::routing::delete(|| async { "destroy" })),
        };
        let router = resource("users", routes);

        // GET /users → index
        let req = Request::builder()
            .method(Method::GET)
            .uri("/users")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // POST /users → store
        let req = Request::builder()
            .method(Method::POST)
            .uri("/users")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // GET /users/create → create
        let req = Request::builder()
            .method(Method::GET)
            .uri("/users/create")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // GET /users/1 → show
        let req = Request::builder()
            .method(Method::GET)
            .uri("/users/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // GET /users/1/edit → edit
        let req = Request::builder()
            .method(Method::GET)
            .uri("/users/1/edit")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // PUT /users/1 → update
        let req = Request::builder()
            .method(Method::PUT)
            .uri("/users/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // DELETE /users/1 → destroy
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/users/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_resource_partial_handlers_only_index_and_store() {
        let routes = ResourceRoutes {
            index: Some(axum::routing::get(|| async { "list" })),
            store: Some(axum::routing::post(|| async { "create" })),
            ..Default::default()
        };
        let router = resource("articles", routes);

        // GET /articles → 200
        let req = Request::builder()
            .method(Method::GET)
            .uri("/articles")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // POST /articles → 200
        let req = Request::builder()
            .method(Method::POST)
            .uri("/articles")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // GET /articles/1 → 404（show 未注册）
        let req = Request::builder()
            .method(Method::GET)
            .uri("/articles/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // GET /articles/create → 404
        let req = Request::builder()
            .method(Method::GET)
            .uri("/articles/create")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_resource_only_id_routes() {
        let routes = ResourceRoutes {
            show: Some(axum::routing::get(|| async { "show" })),
            update: Some(axum::routing::put(|| async { "update" })),
            destroy: Some(axum::routing::delete(|| async { "destroy" })),
            ..Default::default()
        };
        let router = resource("orders", routes);

        // GET /orders → 404（index/store 未注册）
        let req = Request::builder()
            .method(Method::GET)
            .uri("/orders")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // GET /orders/1 → 200
        let req = Request::builder()
            .method(Method::GET)
            .uri("/orders/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // PUT /orders/1 → 200
        let req = Request::builder()
            .method(Method::PUT)
            .uri("/orders/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // DELETE /orders/1 → 200
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/orders/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_resource_empty_routes() {
        // 全部 None → 不注册任何路由 → 所有请求 404
        let routes = ResourceRoutes::new();
        let router = resource("widgets", routes);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/widgets")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_resource_merged_into_router_builder() {
        let routes = ResourceRoutes {
            index: Some(axum::routing::get(|| async { "list" })),
            store: Some(axum::routing::post(|| async { "create" })),
            show: Some(axum::routing::get(|| async { "show" })),
            ..Default::default()
        };
        let resource_router = resource("users", routes);

        let router = RouterBuilder::new()
            .merge(resource_router)
            .get("/health", || async { "ok" })
            .build();

        // GET /health
        let req = Request::builder()
            .method(Method::GET)
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // GET /users
        let req = Request::builder()
            .method(Method::GET)
            .uri("/users")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // POST /users
        let req = Request::builder()
            .method(Method::POST)
            .uri("/users")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // GET /users/42
        let req = Request::builder()
            .method(Method::GET)
            .uri("/users/42")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_resource_body_content() {
        let routes = ResourceRoutes {
            index: Some(axum::routing::get(|| async { "user list" })),
            ..Default::default()
        };
        let router = resource("users", routes);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/users")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"user list");
    }

    // ====================================================================
    // parse_path 性能测试（v0.3.2 §5.2）
    // ====================================================================
    // 验证 const 数组 + 迭代器直接消费优化效果
    // design.md 理想目标：root ≤ 50ns / static ≤ 100ns / long ≤ 100ns
    // 实际测量（release）：root ~180ns / static ~200ns / long ~420ns
    // 差异原因：ParsedPath 字段为 String，capitalize_first + to_string 产生堆分配
    // 阈值设为保守上界（2x 测量值），确保 CI 不因性能波动失败
    // 仅 release 构建运行（debug 无优化，测量无意义）

    #[cfg(not(debug_assertions))]
    #[test]
    fn test_parse_path_perf_root() {
        use std::hint::black_box;
        use std::time::Instant;

        const N: usize = 100_000;
        let start = Instant::now();
        for _ in 0..N {
            let _ = black_box(parse_path(black_box("/")));
        }
        let elapsed = start.elapsed();
        let avg_ns = elapsed.as_nanos() as f64 / N as f64;
        assert!(
            avg_ns < 80.0,
            "parse_path('/') avg {avg_ns:.1}ns exceeds 80ns threshold"
        );
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn test_parse_path_perf_static() {
        use std::hint::black_box;
        use std::time::Instant;

        const N: usize = 100_000;
        let start = Instant::now();
        for _ in 0..N {
            let _ = black_box(parse_path(black_box("/admin/login")));
        }
        let elapsed = start.elapsed();
        let avg_ns = elapsed.as_nanos() as f64 / N as f64;
        assert!(
            avg_ns < 150.0,
            "parse_path('/admin/login') avg {avg_ns:.1}ns exceeds 150ns threshold"
        );
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn test_parse_path_perf_long() {
        use std::hint::black_box;
        use std::time::Instant;

        const N: usize = 100_000;
        let start = Instant::now();
        for _ in 0..N {
            let _ = black_box(parse_path(black_box("/oapc/customer/index?id=1&page=2")));
        }
        let elapsed = start.elapsed();
        let avg_ns = elapsed.as_nanos() as f64 / N as f64;
        assert!(
            avg_ns < 150.0,
            "parse_path('/oapc/customer/index?id=1&page=2') avg {avg_ns:.1}ns exceeds 150ns threshold"
        );
    }
}
