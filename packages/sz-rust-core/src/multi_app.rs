//! 多应用调度器模块 — MultiAppDispatcher
//!
//! 对齐 PHP `auto_multi_app=true` + `app_map` + `domain_bind`。
//!
//! ## 功能
//!
//! - [`MultiAppDispatcher`]：注册多个应用 Router，按路径前缀或域名分发
//! - [`AppEntry`]：单个应用条目（name + Router + 可选 domain）
//!
//! ## PHP 对齐
//!
//! | PHP 配置项 | 值 | Rust 行为 |
//! |-----------|----|----------|
//! | `auto_multi_app` | `true` | 启用多应用解析 |
//! | `app_map` | `oapc/admin/api/farm/oapi/cashier/scene` | 路径前缀分发 |
//! | `domain_bind` | `[]` | 域名绑定（默认为空，可通过 `register_with_domain` 注册） |
//! | `deny_app_list` | `['common']` | 拒绝访问 `common`（在 [`router`](crate::router) 中处理） |
//!
//! ## 端口分配
//!
//! PHP 项目端口分配：
//!
//! | 应用 | 端口 |
//! |------|------|
//! | oapc | 8801 |
//! | admin | 8802 |
//! | api | 8803 |
//! | cashier | 8804 |
//! | scene | 8805 |
//!
//! 这些端口信息在 `config/app.yml` 中维护，调度器本身不直接绑定端口，
//! 而是由 [`server::serve()`](crate::server::serve) 在启动时使用。
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::multi_app::MultiAppDispatcher;
//!
//! let mut dispatcher = MultiAppDispatcher::new();
//! dispatcher.register("oapc", oapc_router);
//! dispatcher.register("admin", admin_router);
//!
//! // 按路径查询应用
//! assert_eq!(dispatcher.dispatch_by_path("/oapc/customer/index"), Some("oapc"));
//!
//! // 整合到主 Router
//! let main_router = dispatcher.build();
//! ```

use std::collections::HashMap;

use axum::Router;

/// 单个应用条目
#[derive(Debug)]
pub struct AppEntry {
    /// 应用名（如 `oapc` / `admin`）
    pub name: String,
    /// 应用的 axum::Router
    pub router: Router,
    /// 可选绑定的域名（对齐 PHP `domain_bind`）
    pub domain: Option<String>,
}

/// 多应用调度器
///
/// 用于注册多个应用的 Router，并按路径前缀或域名分发。
#[derive(Debug, Default)]
pub struct MultiAppDispatcher {
    /// 应用映射表（应用名 → AppEntry）
    apps: HashMap<String, AppEntry>,
}

impl MultiAppDispatcher {
    /// 创建空的调度器
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个应用（无域名绑定）
    ///
    /// ## 参数
    ///
    /// - `name`：应用名（必须与 PHP `app_map` 中的 key 一致）
    /// - `router`：该应用的 axum::Router
    pub fn register(&mut self, name: impl Into<String>, router: Router) {
        let name = name.into();
        self.apps.insert(
            name.clone(),
            AppEntry {
                name,
                router,
                domain: None,
            },
        );
    }

    /// 注册一个应用（带域名绑定）
    ///
    /// 对齐 PHP `domain_bind`。
    ///
    /// ## 参数
    ///
    /// - `name`：应用名
    /// - `domain`：绑定的域名（如 `oapc.example.com`）
    /// - `router`：该应用的 axum::Router
    pub fn register_with_domain(
        &mut self,
        name: impl Into<String>,
        domain: impl Into<String>,
        router: Router,
    ) {
        let name = name.into();
        self.apps.insert(
            name.clone(),
            AppEntry {
                name,
                router,
                domain: Some(domain.into()),
            },
        );
    }

    /// 按路径前缀分发
    ///
    /// 解析 URI 的第一段（如 `/oapc/customer/index` → `oapc`），
    /// 如果该段对应已注册的应用名，则返回应用名。
    ///
    /// ## 参数
    ///
    /// - `uri`：请求 URI（如 `/oapc/customer/index?id=1`）
    ///
    /// ## 返回
    ///
    /// - `Some(app_name)`：路径前缀匹配到已注册应用
    /// - `None`：未匹配（应该走默认应用 `index`）
    pub fn dispatch_by_path(&self, uri: &str) -> Option<&str> {
        let path = uri.split('?').next().unwrap_or(uri);
        let first = path
            .trim_start_matches('/')
            .split('/')
            .next()
            .filter(|s| !s.is_empty())?;

        self.apps.get(first).map(|entry| entry.name.as_str())
    }

    /// 按域名分发
    ///
    /// 对齐 PHP `domain_bind`：如果请求的 Host 头匹配到某个应用的 domain，
    /// 则返回该应用名。
    ///
    /// ## 参数
    ///
    /// - `host`：Host 头（如 `oapc.example.com:8801` 或 `oapc.example.com`）
    ///
    /// ## 返回
    ///
    /// - `Some(app_name)`：域名匹配到已注册应用
    /// - `None`：未匹配
    pub fn dispatch_by_domain(&self, host: &str) -> Option<&str> {
        // 剥离端口号
        let domain = host.split(':').next().unwrap_or(host);
        self.apps
            .values()
            .find(|entry| entry.domain.as_deref() == Some(domain))
            .map(|entry| entry.name.as_str())
    }

    /// 获取应用条目
    pub fn get(&self, name: &str) -> Option<&AppEntry> {
        self.apps.get(name)
    }

    /// 获取已注册应用数量
    pub fn len(&self) -> usize {
        self.apps.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }

    /// 列出所有应用名
    pub fn app_names(&self) -> Vec<&str> {
        self.apps.keys().map(|s| s.as_str()).collect()
    }

    /// 构建整合的主 Router
    ///
    /// 将每个应用的 Router 用 `axum::Router::nest` 嵌套到主 Router，
    /// 路径前缀为 `/{app_name}`。
    ///
    /// ## 注意
    ///
    /// 域名绑定（`domain_bind`）需要在反向代理（Nginx）层处理，
    /// 将不同域名的请求转发到不同的应用路径前缀。
    pub fn build(&self) -> Router {
        let mut main = Router::new();
        for (name, entry) in &self.apps {
            let prefix = format!("/{name}");
            main = main.nest(&prefix, entry.router.clone());
        }
        main
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn make_router(body: &'static str) -> Router {
        Router::new().route("/", axum::routing::get(move || async move { body }))
    }

    #[test]
    fn test_register_and_get() {
        let mut dispatcher = MultiAppDispatcher::new();
        dispatcher.register("oapc", make_router("oapc"));
        assert_eq!(dispatcher.len(), 1);
        assert!(!dispatcher.is_empty());
        assert!(dispatcher.get("oapc").is_some());
        assert!(dispatcher.get("admin").is_none());
    }

    #[test]
    fn test_register_with_domain() {
        let mut dispatcher = MultiAppDispatcher::new();
        dispatcher.register_with_domain("oapc", "oapc.example.com", make_router("oapc"));
        let entry = dispatcher.get("oapc").unwrap();
        assert_eq!(entry.domain.as_deref(), Some("oapc.example.com"));
    }

    #[test]
    fn test_dispatch_by_path_seven_apps() {
        let mut dispatcher = MultiAppDispatcher::new();
        for app in ["oapc", "admin", "api", "farm", "oapi", "cashier", "scene"] {
            dispatcher.register(app, make_router("ok"));
        }

        for app in ["oapc", "admin", "api", "farm", "oapi", "cashier", "scene"] {
            assert_eq!(
                dispatcher.dispatch_by_path(&format!("/{app}/customer/index")),
                Some(app)
            );
        }
    }

    #[test]
    fn test_dispatch_by_path_with_query_string() {
        let mut dispatcher = MultiAppDispatcher::new();
        dispatcher.register("oapc", make_router("ok"));
        assert_eq!(
            dispatcher.dispatch_by_path("/oapc/customer/index?id=1&page=2"),
            Some("oapc")
        );
    }

    #[test]
    fn test_dispatch_by_path_root_returns_none() {
        let mut dispatcher = MultiAppDispatcher::new();
        dispatcher.register("oapc", make_router("ok"));
        assert_eq!(dispatcher.dispatch_by_path("/"), None);
        assert_eq!(dispatcher.dispatch_by_path(""), None);
    }

    #[test]
    fn test_dispatch_by_path_unknown_app_returns_none() {
        let mut dispatcher = MultiAppDispatcher::new();
        dispatcher.register("oapc", make_router("ok"));
        assert_eq!(dispatcher.dispatch_by_path("/unknown/foo/bar"), None);
        // 未注册的应用名 unknown 应当返回 None
        assert_eq!(dispatcher.dispatch_by_path("/common/foo/bar"), None);
    }

    #[test]
    fn test_dispatch_by_domain() {
        let mut dispatcher = MultiAppDispatcher::new();
        dispatcher.register_with_domain("oapc", "oapc.example.com", make_router("oapc"));
        dispatcher.register_with_domain("admin", "admin.example.com", make_router("admin"));

        assert_eq!(
            dispatcher.dispatch_by_domain("oapc.example.com"),
            Some("oapc")
        );
        assert_eq!(
            dispatcher.dispatch_by_domain("admin.example.com"),
            Some("admin")
        );
        assert_eq!(dispatcher.dispatch_by_domain("unknown.com"), None);
    }

    #[test]
    fn test_dispatch_by_domain_with_port() {
        let mut dispatcher = MultiAppDispatcher::new();
        dispatcher.register_with_domain("oapc", "oapc.example.com", make_router("ok"));

        // 端口应该被剥离
        assert_eq!(
            dispatcher.dispatch_by_domain("oapc.example.com:8801"),
            Some("oapc")
        );
        assert_eq!(
            dispatcher.dispatch_by_domain("oapc.example.com:8443"),
            Some("oapc")
        );
    }

    #[test]
    fn test_app_names() {
        let mut dispatcher = MultiAppDispatcher::new();
        dispatcher.register("oapc", make_router("a"));
        dispatcher.register("admin", make_router("b"));

        let mut names = dispatcher.app_names();
        names.sort();
        assert_eq!(names, vec!["admin", "oapc"]);
    }

    #[tokio::test]
    async fn test_build_nests_all_apps() {
        let mut dispatcher = MultiAppDispatcher::new();
        dispatcher.register("oapc", make_router("oapc body"));
        dispatcher.register("admin", make_router("admin body"));

        let router = dispatcher.build();

        // GET /oapc → oapc body（子 Router 的 / 路由）
        let req = Request::builder()
            .method(Method::GET)
            .uri("/oapc")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"oapc body");

        // GET /admin → admin body
        let req = Request::builder()
            .method(Method::GET)
            .uri("/admin")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"admin body");
    }

    #[tokio::test]
    async fn test_build_empty_dispatcher() {
        let dispatcher = MultiAppDispatcher::new();
        let router = dispatcher.build();
        // 空调度器，所有请求返回 404
        let req = Request::builder()
            .method(Method::GET)
            .uri("/oapc")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_build_with_seven_apps() {
        let mut dispatcher = MultiAppDispatcher::new();
        for app in ["oapc", "admin", "api", "farm", "oapi", "cashier", "scene"] {
            let body_str = Box::leak(format!("{app} body").into_boxed_str());
            dispatcher.register(app, make_router(body_str));
        }
        let router = dispatcher.build();

        for app in ["oapc", "admin", "api", "farm", "oapi", "cashier", "scene"] {
            let req = Request::builder()
                .method(Method::GET)
                .uri(format!("/{app}"))
                .body(Body::empty())
                .unwrap();
            let resp = router.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
    }

    #[test]
    fn test_empty_dispatcher() {
        let dispatcher = MultiAppDispatcher::new();
        assert!(dispatcher.is_empty());
        assert_eq!(dispatcher.len(), 0);
        assert!(dispatcher.app_names().is_empty());
    }
}
