//! 三层路由机制 — 属性宏 / 配置式 / 约定式
//!
//! 对齐 PHP `think-route` + `config/route.php` + `auto_multi_app`，提供三种渐进式路由注册方式。
//!
//! ## 三层架构
//!
//! | 层级 | 机制 | PHP 对齐 | 启用方式 | 适用场景 |
//! |------|------|---------|---------|---------|
//! | Layer 1 | 属性宏路由 | `#[Route]` 注解 | `#[controller]` + `#[get]` | 控制器内嵌路由声明 |
//! | Layer 2 | 配置式路由 | `config/route.php` | YAML/JSON 配置文件 | 路由与代码解耦 |
//! | Layer 3 | 约定式路由 | `auto_multi_app` + `app/controller/action` | `parse_path` 自动映射 | 快速原型 / 内部 API |
//!
//! ## 设计原则
//!
//! 1. **三层独立**：每层可独立使用，也可组合使用
//! 2. **优先级递减**：Layer 1 > Layer 2 > Layer 3（前层覆盖后层）
//! 3. **类型安全**：Layer 1 在编译期检查；Layer 2/3 在加载期检查
//! 4. **渐进迁移**：从 Layer 3 起步，逐步迁移到 Layer 2/1
//!
//! ## 用法示例
//!
//! ### Layer 2 - 配置式路由（推荐生产使用）
//!
//! ```ignore
//! use sz_rust_router_facade::routing::{RouteConfig, RouteRule, HttpMethod, load_routes_from_yaml_str};
//! use sz_rust_router_facade::router::RouterBuilder;
//!
//! let yaml = r#"
//! routes:
//!   - method: GET
//!     path: /users
//!     handler: User@list
//!   - method: POST
//!     path: /users
//!     handler: User@create
//! "#;
//! let config = load_routes_from_yaml_str(yaml).unwrap();
//! // config.routes 包含解析后的路由规则
//! ```
//!
//! ### Layer 3 - 约定式路由（已在 `multi_app::parse_path` 中实现）
//!
//! ```ignore
//! use sz_rust_router_facade::router::parse_path;
//!
//! let p = parse_path("/oapc/customer/index");
//! assert_eq!(p.app, "oapc");
//! assert_eq!(p.controller, "Customer");
//! assert_eq!(p.action, "index");
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::router::ParsedPath;

// ============================================================================
// 公共数据结构
// ============================================================================

/// HTTP 方法枚举
///
/// 对齐 PHP `think\Route::$method`，仅包含 RESTful 5 大方法 + OPTIONS。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    /// GET 方法
    GET,
    /// POST 方法
    POST,
    /// PUT 方法
    PUT,
    /// DELETE 方法
    DELETE,
    /// PATCH 方法
    PATCH,
    /// OPTIONS 方法（CORS 预检）
    OPTIONS,
}

impl HttpMethod {
    /// 从字符串解析 HTTP 方法（大小写不敏感）
    ///
    /// ```
    /// use sz_rust_router_facade::routing::HttpMethod;
    ///
    /// assert_eq!(HttpMethod::parse("get").unwrap(), HttpMethod::GET);
    /// assert_eq!(HttpMethod::parse("POST").unwrap(), HttpMethod::POST);
    /// assert!(HttpMethod::parse("invalid").is_err());
    /// ```
    pub fn parse(s: &str) -> Result<Self, RouteConfigError> {
        match s.to_uppercase().as_str() {
            "GET" => Ok(HttpMethod::GET),
            "POST" => Ok(HttpMethod::POST),
            "PUT" => Ok(HttpMethod::PUT),
            "DELETE" => Ok(HttpMethod::DELETE),
            "PATCH" => Ok(HttpMethod::PATCH),
            "OPTIONS" => Ok(HttpMethod::OPTIONS),
            other => Err(RouteConfigError::InvalidMethod(other.to_string())),
        }
    }

    /// 转换为 axum::http::Method
    pub fn to_axum_method(&self) -> axum::http::Method {
        match self {
            HttpMethod::GET => axum::http::Method::GET,
            HttpMethod::POST => axum::http::Method::POST,
            HttpMethod::PUT => axum::http::Method::PUT,
            HttpMethod::DELETE => axum::http::Method::DELETE,
            HttpMethod::PATCH => axum::http::Method::PATCH,
            HttpMethod::OPTIONS => axum::http::Method::OPTIONS,
        }
    }
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpMethod::GET => write!(f, "GET"),
            HttpMethod::POST => write!(f, "POST"),
            HttpMethod::PUT => write!(f, "PUT"),
            HttpMethod::DELETE => write!(f, "DELETE"),
            HttpMethod::PATCH => write!(f, "PATCH"),
            HttpMethod::OPTIONS => write!(f, "OPTIONS"),
        }
    }
}

/// Handler 引用 — `"Controller@action"` 格式
///
/// 对齐 PHP `Route::rule('path', 'Controller/action')` 的字符串引用方式。
///
/// ## 解析规则
///
/// - `"User@list"` → `HandlerRef { controller: "User", action: "list" }`
/// - `"User/list"` → 同上（兼容 PHP `/` 分隔符）
/// - `"User"` → `HandlerRef { controller: "User", action: "index" }`（默认 action）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandlerRef {
    /// 控制器名（首字母大写，如 `User`）
    pub controller: String,
    /// 操作名（小驼峰，如 `list` / `index`）
    pub action: String,
}

impl HandlerRef {
    /// 从 `"Controller@action"` 或 `"Controller/action"` 字符串解析
    ///
    /// ## 校验规则
    ///
    /// - controller / action 必须匹配 `[A-Za-z_][A-Za-z0-9_]*`（PHP 标识符规则）
    /// - 拒绝 `../`、空格、`@`、`/` 等可能引发路径穿越或解析歧义的字符
    ///
    /// ```
    /// use sz_rust_router_facade::routing::HandlerRef;
    ///
    /// let h = HandlerRef::parse("User@list").unwrap();
    /// assert_eq!(h.controller, "User");
    /// assert_eq!(h.action, "list");
    ///
    /// let h = HandlerRef::parse("User").unwrap();
    /// assert_eq!(h.controller, "User");
    /// assert_eq!(h.action, "index");
    /// ```
    pub fn parse(s: &str) -> Result<Self, RouteConfigError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(RouteConfigError::EmptyHandler);
        }

        // 优先按 '@' 分隔，其次按 '/' 分隔
        let (controller, action) = if let Some((c, a)) = s.split_once('@') {
            (c, a)
        } else if let Some((c, a)) = s.split_once('/') {
            (c, a)
        } else {
            (s, crate::router::DEFAULT_ACTION)
        };

        let controller = controller.trim();
        let action = action.trim();

        if controller.is_empty() {
            return Err(RouteConfigError::EmptyController);
        }
        if action.is_empty() {
            return Err(RouteConfigError::EmptyAction);
        }
        if !is_valid_identifier(controller) {
            return Err(RouteConfigError::InvalidController(controller.to_string()));
        }
        if !is_valid_identifier(action) {
            return Err(RouteConfigError::InvalidAction(action.to_string()));
        }

        Ok(Self {
            controller: controller.to_string(),
            action: action.to_string(),
        })
    }

    /// 转换为 `"Controller@action"` 字符串
    pub fn to_handler_string(&self) -> String {
        format!("{}@{}", self.controller, self.action)
    }
}

/// 校验是否为合法的 PHP 风格标识符
///
/// 规则：首字符必须是字母或下划线，其余字符必须是字母/数字/下划线。
/// 用于阻止 `../Secret`、`User@list@extra`、`path/to/file` 等注入字符。
fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl std::fmt::Display for HandlerRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.controller, self.action)
    }
}

// ============================================================================
// HandlerRefRef — 零拷贝借用版本（P3 优化）
// ============================================================================

/// Handler 引用（零拷贝借用版本）
///
/// 与 [`HandlerRef`] 语义完全一致，但使用 `&'a str` 切片而非 `String`，
/// 解析时零堆分配。适用于热路径（路由匹配）中临时解析 handler 字符串。
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_router_facade::routing::HandlerRefRef;
///
/// let h = HandlerRefRef::parse("User@list").unwrap();
/// assert_eq!(h.controller, "User");
/// assert_eq!(h.action, "list");
///
/// // 转为 owned 版本（1 次分配）
/// let owned: HandlerRef = h.to_owned();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandlerRefRef<'a> {
    /// 控制器名（首字母大写，如 `User`）
    pub controller: &'a str,
    /// 操作名（小驼峰，如 `list` / `index`）
    pub action: &'a str,
}

impl<'a> HandlerRefRef<'a> {
    /// 从 `"Controller@action"` 或 `"Controller/action"` 字符串解析（零分配）
    ///
    /// 使用 `split_once` 返回 `&str` 切片，不调用 `to_string()`。
    /// 校验规则与 [`HandlerRef::parse`] 完全一致。
    pub fn parse(s: &'a str) -> Result<Self, RouteConfigError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(RouteConfigError::EmptyHandler);
        }

        // 优先按 '@' 分隔，其次按 '/' 分隔
        let (controller, action) = if let Some((c, a)) = s.split_once('@') {
            (c, a)
        } else if let Some((c, a)) = s.split_once('/') {
            (c, a)
        } else {
            (s, crate::router::DEFAULT_ACTION)
        };

        let controller = controller.trim();
        let action = action.trim();

        if controller.is_empty() {
            return Err(RouteConfigError::EmptyController);
        }
        if action.is_empty() {
            return Err(RouteConfigError::EmptyAction);
        }
        if !is_valid_identifier(controller) {
            return Err(RouteConfigError::InvalidController(controller.to_string()));
        }
        if !is_valid_identifier(action) {
            return Err(RouteConfigError::InvalidAction(action.to_string()));
        }

        Ok(Self { controller, action })
    }

    /// 转为 owned 版本（1 次分配）
    pub fn to_owned(&self) -> HandlerRef {
        HandlerRef {
            controller: self.controller.to_string(),
            action: self.action.to_string(),
        }
    }

    /// 转换为 `"Controller@action"` 字符串
    pub fn to_handler_string(&self) -> String {
        format!("{}@{}", self.controller, self.action)
    }
}

impl<'a> From<HandlerRefRef<'a>> for HandlerRef {
    fn from(h: HandlerRefRef<'a>) -> Self {
        h.to_owned()
    }
}

impl<'a> std::fmt::Display for HandlerRefRef<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.controller, self.action)
    }
}

// ============================================================================
// Layer 2 - 配置式路由
// ============================================================================

/// 路由配置错误
#[derive(Debug, thiserror::Error)]
pub enum RouteConfigError {
    /// YAML 解析失败
    #[error("YAML parse error: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    /// JSON 解析失败
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// 无效的 HTTP 方法
    #[error("invalid HTTP method: {0}")]
    InvalidMethod(String),

    /// 空的 handler 字符串
    #[error("empty handler string")]
    EmptyHandler,

    /// 空的控制器名
    #[error("empty controller name in handler")]
    EmptyController,

    /// 空的 action 名
    #[error("empty action name in handler")]
    EmptyAction,

    /// 无效的控制器名（包含非 `[A-Za-z0-9_]` 字符或首字符不是字母/下划线）
    #[error("invalid controller name: {0}")]
    InvalidController(String),

    /// 无效的 action 名（包含非 `[A-Za-z0-9_]` 字符或首字符不是字母/下划线）
    #[error("invalid action name: {0}")]
    InvalidAction(String),

    /// handler 解析失败
    #[error("handler parse error: {0}")]
    HandlerParse(String),

    /// 路由冲突（同 method+path 重复）
    #[error("route conflict: {method} {path} already registered")]
    Conflict {
        /// 冲突的 HTTP 方法
        method: String,
        /// 冲突的路径
        path: String,
    },

    /// 文件读取失败
    #[error("failed to read route config file: {0}")]
    FileRead(#[source] std::io::Error),
}

/// 单条路由规则
///
/// 对齐 PHP `think\Route::rule($rule, $route, $method)` 的单条规则结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteRule {
    /// HTTP 方法
    pub method: HttpMethod,
    /// 路径模板，如 `/users/{id}`（对齐 axum 0.8 路由语法）
    pub path: String,
    /// Handler 引用，如 `User@list`
    pub handler: String,
    /// 中间件列表（按名称引用，运行时通过 middleware registry 解析）
    #[serde(default)]
    pub middleware: Vec<String>,
    /// 路由名称（可选，用于反向 URL 生成）
    #[serde(default)]
    pub name: Option<String>,
}

impl RouteRule {
    /// 创建新路由规则
    pub fn new(method: HttpMethod, path: impl Into<String>, handler: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            handler: handler.into(),
            middleware: Vec::new(),
            name: None,
        }
    }

    /// 解析 handler 字符串为 [`HandlerRef`]
    pub fn handler_ref(&self) -> Result<HandlerRef, RouteConfigError> {
        HandlerRef::parse(&self.handler)
    }

    /// 添加中间件
    pub fn with_middleware(mut self, name: impl Into<String>) -> Self {
        self.middleware.push(name.into());
        self
    }

    /// 设置路由名称
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// 路由配置文件
///
/// 对齐 PHP `config/route.php` 的整体配置结构。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RouteConfig {
    /// 路由规则列表
    #[serde(default)]
    pub routes: Vec<RouteRule>,
    /// 路由分组（每组有自己的前缀和中间件）
    #[serde(default)]
    pub groups: Vec<RouteGroup>,
}

impl RouteConfig {
    /// 创建空配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加单条路由
    pub fn add_route(&mut self, rule: RouteRule) {
        self.routes.push(rule);
    }

    /// 添加路由分组
    pub fn add_group(&mut self, group: RouteGroup) {
        self.groups.push(group);
    }

    /// 展开所有 group，返回扁平化后的路由列表
    ///
    /// group 内路由的最终 path = `{group.prefix}/{rule.path}`（去重 `/`）
    /// group 内路由继承 group 的中间件（追加在 rule.middleware 之前）
    pub fn flatten(&self) -> Vec<RouteRule> {
        let mut result = self.routes.clone();
        for group in &self.groups {
            for rule in &group.routes {
                let mut flattened = rule.clone();
                flattened.path = join_path(&group.prefix, &flattened.path);
                // group 中间件前置
                let mut mw = group.middleware.clone();
                mw.extend(flattened.middleware);
                flattened.middleware = mw;
                result.push(flattened);
            }
        }
        result
    }

    /// 检查路由冲突（同 method+path 重复）
    ///
    /// 返回冲突列表（空表示无冲突）
    pub fn find_conflicts(&self) -> Vec<(RouteRule, RouteRule)> {
        let flattened = self.flatten();
        let mut seen: HashMap<(String, String), usize> = HashMap::new();
        let mut conflicts = Vec::new();

        for (i, rule) in flattened.iter().enumerate() {
            let key = (rule.method.to_string(), rule.path.clone());
            if let Some(&prev_idx) = seen.get(&key) {
                conflicts.push((flattened[prev_idx].clone(), flattened[i].clone()));
            } else {
                seen.insert(key, i);
            }
        }

        conflicts
    }
}

/// 路由分组
///
/// 对齐 PHP `Route::group($prefix, $callback)`，给组内所有路由添加统一前缀和中间件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteGroup {
    /// 路径前缀，如 `/api/v1`
    pub prefix: String,
    /// 组内路由规则
    #[serde(default)]
    pub routes: Vec<RouteRule>,
    /// 组级中间件（应用到组内所有路由）
    #[serde(default)]
    pub middleware: Vec<String>,
}

impl RouteGroup {
    /// 创建新路由分组
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            routes: Vec::new(),
            middleware: Vec::new(),
        }
    }

    /// 添加路由
    pub fn add_route(&mut self, rule: RouteRule) -> &mut Self {
        self.routes.push(rule);
        self
    }

    /// 添加中间件
    pub fn with_middleware(mut self, name: impl Into<String>) -> Self {
        self.middleware.push(name.into());
        self
    }
}

/// 拼接两个路径片段，正确处理 `/` 分隔
fn join_path(prefix: &str, path: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        prefix.to_string()
    } else if prefix.is_empty() {
        format!("/{path}")
    } else {
        format!("{prefix}/{path}")
    }
}

// ============================================================================
// Layer 2 - 配置加载函数
// ============================================================================

/// 从 YAML 字符串加载路由配置
///
/// ## YAML 格式
///
/// ```yaml
/// routes:
///   - method: GET
///     path: /users
///     handler: User@list
///   - method: POST
///     path: /users
///     handler: User@create
/// groups:
///   - prefix: /api/v1
///     middleware: [auth, log]
///     routes:
///       - method: GET
///         path: /items
///         handler: Item@list
/// ```
#[tracing::instrument]
pub fn load_routes_from_yaml_str(yaml: &str) -> Result<RouteConfig, RouteConfigError> {
    let config: RouteConfig = serde_yaml::from_str(yaml)?;
    Ok(config)
}

/// 从 JSON 字符串加载路由配置
#[tracing::instrument]
pub fn load_routes_from_json_str(json: &str) -> Result<RouteConfig, RouteConfigError> {
    let config: RouteConfig = serde_json::from_str(json)?;
    Ok(config)
}

/// 从 YAML 文件加载路由配置
#[tracing::instrument(skip(path))]
pub async fn load_routes_from_yaml_file(
    path: impl AsRef<std::path::Path>,
) -> Result<RouteConfig, RouteConfigError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(RouteConfigError::FileRead)?;
    load_routes_from_yaml_str(&content)
}

/// 从 JSON 文件加载路由配置
#[tracing::instrument(skip(path))]
pub async fn load_routes_from_json_file(
    path: impl AsRef<std::path::Path>,
) -> Result<RouteConfig, RouteConfigError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(RouteConfigError::FileRead)?;
    load_routes_from_json_str(&content)
}

// ============================================================================
// Layer 1 - 属性宏路由契约（接口定义，实现见 controller_registry 模块）
// ============================================================================

/// 控制器路由契约
///
/// 由 `#[controller]` 属性宏自动实现，声明控制器内的所有路由。
///
/// ## 用法（属性宏实现后）
///
/// ```ignore
/// use sz_rust_router_facade::routing::ControllerRouter;
///
/// #[controller(prefix = "/users")]
/// struct UserController;
///
/// impl UserController {
///     #[get("/{id}")]
///     fn show(&self, id: i64) -> String { ... }
/// }
///
/// // ControllerRouter 由 #[controller] 自动实现
/// let routes = UserController.router_rules();
/// ```
pub trait ControllerRouter {
    /// 返回控制器内所有路由规则（已包含 prefix）
    fn router_rules(&self) -> Vec<RouteRule>;

    /// 控制器的路径前缀（如 `/users`）
    fn router_prefix(&self) -> &str {
        ""
    }

    /// 控制器级中间件（应用到控制器内所有路由）
    fn router_middleware(&self) -> Vec<String> {
        Vec::new()
    }
}

// ============================================================================
// Layer 3 - 约定式路由元数据
// ============================================================================

/// 约定式路由元数据
///
/// 基于 `parse_path` 的 `(app, controller, action)` 三元组生成。
/// 实际路由注册需要 ControllerRegistry（在 controller_registry 模块实现）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConventionRoute {
    /// 应用名
    pub app: String,
    /// 控制器名（首字母大写）
    pub controller: String,
    /// 操作名（小驼峰）
    pub action: String,
    /// HTTP 方法（约定式默认 GET，POST 也常见）
    pub method: HttpMethod,
    /// 生成的路径（如 `/oapc/customer/index`）
    pub path: String,
}

impl ConventionRoute {
    /// 从 URI 生成约定式路由元数据
    ///
    /// ```ignore
    /// use sz_rust_router_facade::routing::ConventionRoute;
    ///
    /// let r = ConventionRoute::from_uri("/oapc/customer/index").unwrap();
    /// assert_eq!(r.app, "oapc");
    /// assert_eq!(r.controller, "Customer");
    /// assert_eq!(r.action, "index");
    /// assert_eq!(r.path, "/oapc/customer/index");
    /// ```
    pub fn from_uri(uri: &str) -> Option<Self> {
        let parsed = crate::router::parse_path(uri);
        // 默认应用和默认控制器+action 的情况不生成约定式路由
        if parsed.app == crate::router::DEFAULT_APP
            && parsed.controller == crate::router::DEFAULT_CONTROLLER
            && parsed.action == crate::router::DEFAULT_ACTION
        {
            return None;
        }
        let path = format!(
            "/{}/{}/{}",
            parsed.app,
            parsed.controller.to_lowercase(),
            parsed.action
        );
        Some(Self {
            app: parsed.app.into_owned(),
            controller: parsed.controller.into_owned(),
            action: parsed.action.into_owned(),
            method: HttpMethod::GET,
            path,
        })
    }

    /// 从 ParsedPath 构造
    pub fn from_parsed<'a>(parsed: ParsedPath<'a>) -> Option<Self> {
        let uri = format!(
            "/{}/{}/{}",
            parsed.app,
            parsed.controller.to_lowercase(),
            parsed.action
        );
        Self::from_uri(&uri)
    }
}

// ============================================================================
// RouteRegistry - 三层路由汇总
// ============================================================================

/// 三层路由注册表
///
/// 收集三层路由的元数据，提供统一的查询和冲突检测接口。
///
/// ## 注意
///
/// 实际将路由注册到 `axum::Router` 需要 ControllerRegistry（在 controller_registry 模块实现）。
/// 本模块仅提供元数据管理和冲突检测。
#[derive(Debug, Clone, Default)]
pub struct RouteRegistry {
    /// Layer 1 - 属性宏路由
    pub attribute_routes: Vec<RouteRule>,
    /// Layer 2 - 配置式路由
    pub config_routes: Vec<RouteRule>,
    /// Layer 3 - 约定式路由
    pub convention_routes: Vec<ConventionRoute>,
}

impl RouteRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加属性宏路由
    pub fn add_attribute_route(&mut self, rule: RouteRule) -> &mut Self {
        self.attribute_routes.push(rule);
        self
    }

    /// 批量添加属性宏路由
    pub fn add_attribute_routes(
        &mut self,
        rules: impl IntoIterator<Item = RouteRule>,
    ) -> &mut Self {
        self.attribute_routes.extend(rules);
        self
    }

    /// 添加配置式路由
    pub fn add_config_routes(&mut self, config: &RouteConfig) -> &mut Self {
        self.config_routes.extend(config.flatten());
        self
    }

    /// 添加约定式路由
    pub fn add_convention_route(&mut self, route: ConventionRoute) -> &mut Self {
        self.convention_routes.push(route);
        self
    }

    /// 转换约定式路由为 RouteRule 列表
    ///
    /// 约定式路由的 handler 字段为 `Controller@action` 格式。
    #[tracing::instrument(skip(self))]
    pub fn convention_as_rules(&self) -> Vec<RouteRule> {
        self.convention_routes
            .iter()
            .map(|c| RouteRule {
                method: c.method.clone(),
                path: c.path.clone(),
                handler: format!("{}@{}", c.controller, c.action),
                middleware: Vec::new(),
                name: Some(format!(
                    "convention.{}.{}.{}",
                    c.app, c.controller, c.action
                )),
            })
            .collect()
    }

    /// 合并所有三层的路由规则（按优先级：attribute > config > convention）
    ///
    /// 冲突时优先级高的覆盖低的，返回最终路由列表。
    #[tracing::instrument(skip(self))]
    pub fn merged_rules(&self) -> Vec<RouteRule> {
        let mut seen: HashMap<(String, String), RouteRule> = HashMap::new();

        // 优先级低 → 高（后插入覆盖前插入）
        for rule in self.convention_as_rules() {
            let key = (rule.method.to_string(), rule.path.clone());
            seen.insert(key, rule);
        }
        for rule in &self.config_routes {
            let key = (rule.method.to_string(), rule.path.clone());
            seen.insert(key, rule.clone());
        }
        for rule in &self.attribute_routes {
            let key = (rule.method.to_string(), rule.path.clone());
            seen.insert(key, rule.clone());
        }

        seen.into_values().collect()
    }

    /// 检测属性宏层内部的冲突
    pub fn attribute_conflicts(&self) -> Vec<(RouteRule, RouteRule)> {
        find_conflicts_in(&self.attribute_routes)
    }

    /// 检测配置式层内部的冲突
    pub fn config_conflicts(&self) -> Vec<(RouteRule, RouteRule)> {
        find_conflicts_in(&self.config_routes)
    }

    /// 路由总数
    pub fn total_count(&self) -> usize {
        self.attribute_routes.len() + self.config_routes.len() + self.convention_routes.len()
    }
}

/// 在路由规则列表中检测冲突
fn find_conflicts_in(rules: &[RouteRule]) -> Vec<(RouteRule, RouteRule)> {
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    let mut conflicts = Vec::new();

    for (i, rule) in rules.iter().enumerate() {
        let key = (rule.method.to_string(), rule.path.clone());
        if let Some(&prev_idx) = seen.get(&key) {
            conflicts.push((rules[prev_idx].clone(), rules[i].clone()));
        } else {
            seen.insert(key, i);
        }
    }

    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;

    // ====================================================================
    // HttpMethod
    // ====================================================================

    #[test]
    fn test_http_method_parse_uppercase() {
        assert_eq!(HttpMethod::parse("GET").unwrap(), HttpMethod::GET);
        assert_eq!(HttpMethod::parse("POST").unwrap(), HttpMethod::POST);
        assert_eq!(HttpMethod::parse("PUT").unwrap(), HttpMethod::PUT);
        assert_eq!(HttpMethod::parse("DELETE").unwrap(), HttpMethod::DELETE);
        assert_eq!(HttpMethod::parse("PATCH").unwrap(), HttpMethod::PATCH);
        assert_eq!(HttpMethod::parse("OPTIONS").unwrap(), HttpMethod::OPTIONS);
    }

    #[test]
    fn test_http_method_parse_lowercase() {
        assert_eq!(HttpMethod::parse("get").unwrap(), HttpMethod::GET);
        assert_eq!(HttpMethod::parse("post").unwrap(), HttpMethod::POST);
    }

    #[test]
    fn test_http_method_parse_mixed_case() {
        assert_eq!(HttpMethod::parse("Get").unwrap(), HttpMethod::GET);
        assert_eq!(HttpMethod::parse("pOsT").unwrap(), HttpMethod::POST);
    }

    #[test]
    fn test_http_method_parse_invalid() {
        assert!(HttpMethod::parse("invalid").is_err());
        assert!(HttpMethod::parse("").is_err());
        assert!(HttpMethod::parse("CONNECT").is_err());
        assert!(HttpMethod::parse("TRACE").is_err());
    }

    #[test]
    fn test_http_method_to_axum() {
        assert_eq!(HttpMethod::GET.to_axum_method(), axum::http::Method::GET);
        assert_eq!(HttpMethod::POST.to_axum_method(), axum::http::Method::POST);
        assert_eq!(HttpMethod::PUT.to_axum_method(), axum::http::Method::PUT);
        assert_eq!(
            HttpMethod::DELETE.to_axum_method(),
            axum::http::Method::DELETE
        );
        assert_eq!(
            HttpMethod::PATCH.to_axum_method(),
            axum::http::Method::PATCH
        );
        assert_eq!(
            HttpMethod::OPTIONS.to_axum_method(),
            axum::http::Method::OPTIONS
        );
    }

    #[test]
    fn test_http_method_display() {
        assert_eq!(HttpMethod::GET.to_string(), "GET");
        assert_eq!(HttpMethod::POST.to_string(), "POST");
        assert_eq!(HttpMethod::PUT.to_string(), "PUT");
    }

    #[test]
    fn test_http_method_serde() {
        let json = serde_json::to_string(&HttpMethod::GET).unwrap();
        assert_eq!(json, "\"GET\"");

        let m: HttpMethod = serde_json::from_str("\"POST\"").unwrap();
        assert_eq!(m, HttpMethod::POST);
    }

    // ====================================================================
    // HandlerRef
    // ====================================================================

    #[test]
    fn test_handler_ref_parse_at_separator() {
        let h = HandlerRef::parse("User@list").unwrap();
        assert_eq!(h.controller, "User");
        assert_eq!(h.action, "list");
    }

    #[test]
    fn test_handler_ref_parse_slash_separator() {
        let h = HandlerRef::parse("User/list").unwrap();
        assert_eq!(h.controller, "User");
        assert_eq!(h.action, "list");
    }

    #[test]
    fn test_handler_ref_parse_only_controller() {
        let h = HandlerRef::parse("User").unwrap();
        assert_eq!(h.controller, "User");
        assert_eq!(h.action, "index"); // 默认 action
    }

    #[test]
    fn test_handler_ref_parse_with_whitespace() {
        let h = HandlerRef::parse("  User  @  list  ").unwrap();
        assert_eq!(h.controller, "User");
        assert_eq!(h.action, "list");
    }

    #[test]
    fn test_handler_ref_parse_empty() {
        assert!(HandlerRef::parse("").is_err());
        assert!(HandlerRef::parse("   ").is_err());
    }

    #[test]
    fn test_handler_ref_parse_empty_controller() {
        assert!(HandlerRef::parse("@list").is_err());
        assert!(HandlerRef::parse("/list").is_err());
    }

    #[test]
    fn test_handler_ref_parse_empty_action() {
        assert!(HandlerRef::parse("User@").is_err());
        assert!(HandlerRef::parse("User/").is_err());
    }

    #[test]
    fn test_handler_ref_to_string() {
        let h = HandlerRef {
            controller: "User".to_string(),
            action: "list".to_string(),
        };
        assert_eq!(h.to_string(), "User@list");
    }

    // ====================================================================
    // HandlerRefRef — 零拷贝借用版本（P3）
    // ====================================================================

    #[test]
    fn test_handler_ref_ref_parse_at_separator() {
        let h = HandlerRefRef::parse("User@list").unwrap();
        assert_eq!(h.controller, "User");
        assert_eq!(h.action, "list");
    }

    #[test]
    fn test_handler_ref_ref_parse_slash_separator() {
        let h = HandlerRefRef::parse("User/list").unwrap();
        assert_eq!(h.controller, "User");
        assert_eq!(h.action, "list");
    }

    #[test]
    fn test_handler_ref_ref_parse_only_controller() {
        let h = HandlerRefRef::parse("User").unwrap();
        assert_eq!(h.controller, "User");
        assert_eq!(h.action, "index");
    }

    #[test]
    fn test_handler_ref_ref_parse_with_whitespace() {
        let h = HandlerRefRef::parse("  User  @  list  ").unwrap();
        assert_eq!(h.controller, "User");
        assert_eq!(h.action, "list");
    }

    #[test]
    fn test_handler_ref_ref_parse_empty() {
        assert!(HandlerRefRef::parse("").is_err());
        assert!(HandlerRefRef::parse("   ").is_err());
    }

    #[test]
    fn test_handler_ref_ref_parse_empty_controller() {
        assert!(HandlerRefRef::parse("@list").is_err());
        assert!(HandlerRefRef::parse("/list").is_err());
    }

    #[test]
    fn test_handler_ref_ref_parse_empty_action() {
        assert!(HandlerRefRef::parse("User@").is_err());
        assert!(HandlerRefRef::parse("User/").is_err());
    }

    #[test]
    fn test_handler_ref_ref_parse_rejects_path_traversal() {
        assert!(HandlerRefRef::parse("../Secret@admin").is_err());
    }

    #[test]
    fn test_handler_ref_ref_parse_rejects_special_chars() {
        assert!(HandlerRefRef::parse("User$@list").is_err());
    }

    #[test]
    fn test_handler_ref_ref_to_owned_consistency() {
        let ref_ref = HandlerRefRef::parse("User@list").unwrap();
        let owned = ref_ref.to_owned();
        assert_eq!(owned.controller, "User");
        assert_eq!(owned.action, "list");
    }

    #[test]
    fn test_handler_ref_ref_from_into_handler_ref() {
        let ref_ref = HandlerRefRef::parse("Admin@dashboard").unwrap();
        let owned: HandlerRef = ref_ref.into();
        assert_eq!(owned.controller, "Admin");
        assert_eq!(owned.action, "dashboard");
    }

    #[test]
    fn test_handler_ref_ref_display() {
        let h = HandlerRefRef::parse("User@list").unwrap();
        assert_eq!(h.to_string(), "User@list");
    }

    #[test]
    fn test_handler_ref_ref_to_handler_string() {
        let h = HandlerRefRef::parse("User@list").unwrap();
        assert_eq!(h.to_handler_string(), "User@list");
    }

    // ====================================================================
    // S-1 回归测试：HandlerRef 注入字符校验
    // ====================================================================

    #[test]
    fn test_handler_ref_parse_rejects_path_traversal() {
        // `../Secret@admin` → controller="../Secret" → InvalidController
        assert!(matches!(
            HandlerRef::parse("../Secret@admin"),
            Err(RouteConfigError::InvalidController(_))
        ));
        // `..@admin` → controller=".." → InvalidController
        assert!(matches!(
            HandlerRef::parse("..@admin"),
            Err(RouteConfigError::InvalidController(_))
        ));
        // `User@../evil` → action="../evil" → InvalidAction
        assert!(matches!(
            HandlerRef::parse("User@../evil"),
            Err(RouteConfigError::InvalidAction(_))
        ));
    }

    #[test]
    fn test_handler_ref_parse_rejects_double_at() {
        // `User@list@extra` → split_once('@') 得 controller="User", action="list@extra"
        // action 包含 '@' → InvalidAction
        assert!(matches!(
            HandlerRef::parse("User@list@extra"),
            Err(RouteConfigError::InvalidAction(_))
        ));
    }

    #[test]
    fn test_handler_ref_parse_rejects_space_injection() {
        // 内部空格不应被允许（trim 仅处理首尾）
        assert!(matches!(
            HandlerRef::parse("Us er@list"),
            Err(RouteConfigError::InvalidController(_))
        ));
        assert!(matches!(
            HandlerRef::parse("User@li st"),
            Err(RouteConfigError::InvalidAction(_))
        ));
    }

    #[test]
    fn test_handler_ref_parse_rejects_leading_digit() {
        // PHP 标识符首字符不能是数字
        assert!(matches!(
            HandlerRef::parse("1User@list"),
            Err(RouteConfigError::InvalidController(_))
        ));
        assert!(matches!(
            HandlerRef::parse("User@1list"),
            Err(RouteConfigError::InvalidAction(_))
        ));
    }

    #[test]
    fn test_handler_ref_parse_accepts_underscore_and_alphanumeric() {
        let h = HandlerRef::parse("_Private@_index").unwrap();
        assert_eq!(h.controller, "_Private");
        assert_eq!(h.action, "_index");

        let h = HandlerRef::parse("User@action_1").unwrap();
        assert_eq!(h.controller, "User");
        assert_eq!(h.action, "action_1");

        // CamelCase 也合法
        let h = HandlerRef::parse("CustomerList@getListById").unwrap();
        assert_eq!(h.controller, "CustomerList");
        assert_eq!(h.action, "getListById");
    }

    #[test]
    fn test_handler_ref_parse_rejects_special_chars() {
        // 冒号、分号、反斜杠等都不允许
        assert!(HandlerRef::parse("User:list@action").is_err());
        assert!(HandlerRef::parse("User;list@action").is_err());
        assert!(HandlerRef::parse(r"User\list@action").is_err());
        assert!(HandlerRef::parse("User@act\nion").is_err());
    }

    // ====================================================================
    // RouteRule
    // ====================================================================

    #[test]
    fn test_route_rule_new() {
        let rule = RouteRule::new(HttpMethod::GET, "/users", "User@list");
        assert_eq!(rule.method, HttpMethod::GET);
        assert_eq!(rule.path, "/users");
        assert_eq!(rule.handler, "User@list");
        assert!(rule.middleware.is_empty());
        assert!(rule.name.is_none());
    }

    #[test]
    fn test_route_rule_handler_ref() {
        let rule = RouteRule::new(HttpMethod::GET, "/users", "User@list");
        let h = rule.handler_ref().unwrap();
        assert_eq!(h.controller, "User");
        assert_eq!(h.action, "list");
    }

    #[test]
    fn test_route_rule_with_middleware() {
        let rule = RouteRule::new(HttpMethod::GET, "/users", "User@list")
            .with_middleware("auth")
            .with_middleware("log");
        assert_eq!(rule.middleware, vec!["auth", "log"]);
    }

    #[test]
    fn test_route_rule_with_name() {
        let rule = RouteRule::new(HttpMethod::GET, "/users", "User@list").with_name("user.list");
        assert_eq!(rule.name, Some("user.list".to_string()));
    }

    // ====================================================================
    // RouteGroup
    // ====================================================================

    #[test]
    fn test_route_group_new() {
        let g = RouteGroup::new("/api/v1");
        assert_eq!(g.prefix, "/api/v1");
        assert!(g.routes.is_empty());
        assert!(g.middleware.is_empty());
    }

    #[test]
    fn test_route_group_add_route() {
        let mut g = RouteGroup::new("/api");
        g.add_route(RouteRule::new(HttpMethod::GET, "/items", "Item@list"));
        assert_eq!(g.routes.len(), 1);
    }

    #[test]
    fn test_route_group_with_middleware() {
        let g = RouteGroup::new("/api")
            .with_middleware("auth")
            .with_middleware("log");
        assert_eq!(g.middleware, vec!["auth", "log"]);
    }

    // ====================================================================
    // RouteConfig::flatten + join_path
    // ====================================================================

    #[test]
    fn test_join_path_basic() {
        assert_eq!(join_path("/api", "/users"), "/api/users");
        assert_eq!(join_path("/api/", "/users"), "/api/users");
        assert_eq!(join_path("/api", "users"), "/api/users");
        assert_eq!(join_path("/api/", "users"), "/api/users");
    }

    #[test]
    fn test_join_path_empty_prefix() {
        assert_eq!(join_path("", "/users"), "/users");
        assert_eq!(join_path("", "users"), "/users");
    }

    #[test]
    fn test_join_path_empty_path() {
        assert_eq!(join_path("/api", ""), "/api");
        assert_eq!(join_path("/api/", ""), "/api");
    }

    #[test]
    fn test_join_path_both_empty() {
        assert_eq!(join_path("", ""), "");
    }

    #[test]
    fn test_route_config_flatten_no_groups() {
        let mut config = RouteConfig::new();
        config.add_route(RouteRule::new(HttpMethod::GET, "/users", "User@list"));
        config.add_route(RouteRule::new(HttpMethod::POST, "/users", "User@create"));

        let flat = config.flatten();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].path, "/users");
        assert_eq!(flat[1].path, "/users");
    }

    #[test]
    fn test_route_config_flatten_with_group() {
        let mut config = RouteConfig::new();
        let mut group = RouteGroup::new("/api/v1");
        group.add_route(RouteRule::new(HttpMethod::GET, "/items", "Item@list"));
        group.add_route(RouteRule::new(HttpMethod::POST, "/items", "Item@create"));
        config.add_group(group);

        let flat = config.flatten();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].path, "/api/v1/items");
        assert_eq!(flat[1].path, "/api/v1/items");
    }

    #[test]
    fn test_route_config_flatten_group_middleware_prepended() {
        let mut config = RouteConfig::new();
        let mut group = RouteGroup::new("/api");
        group.middleware = vec!["auth".to_string(), "log".to_string()];
        let mut rule = RouteRule::new(HttpMethod::GET, "/items", "Item@list");
        rule.middleware = vec!["cache".to_string()];
        group.routes.push(rule);
        config.add_group(group);

        let flat = config.flatten();
        assert_eq!(flat[0].middleware, vec!["auth", "log", "cache"]);
    }

    #[test]
    fn test_route_config_flatten_mixed() {
        let mut config = RouteConfig::new();
        config.add_route(RouteRule::new(HttpMethod::GET, "/health", "Health@check"));
        let mut group = RouteGroup::new("/api");
        group.add_route(RouteRule::new(HttpMethod::GET, "/items", "Item@list"));
        config.add_group(group);

        let flat = config.flatten();
        assert_eq!(flat.len(), 2);
        assert!(flat.iter().any(|r| r.path == "/health"));
        assert!(flat.iter().any(|r| r.path == "/api/items"));
    }

    // ====================================================================
    // RouteConfig::find_conflicts
    // ====================================================================

    #[test]
    fn test_route_config_no_conflicts() {
        let mut config = RouteConfig::new();
        config.add_route(RouteRule::new(HttpMethod::GET, "/users", "User@list"));
        config.add_route(RouteRule::new(HttpMethod::POST, "/users", "User@create"));
        assert!(config.find_conflicts().is_empty());
    }

    #[test]
    fn test_route_config_conflict_same_method_path() {
        let mut config = RouteConfig::new();
        config.add_route(RouteRule::new(HttpMethod::GET, "/users", "User@list"));
        config.add_route(RouteRule::new(HttpMethod::GET, "/users", "User@all"));

        let conflicts = config.find_conflicts();
        assert_eq!(conflicts.len(), 1);
        let (a, b) = &conflicts[0];
        assert_eq!(a.handler, "User@list");
        assert_eq!(b.handler, "User@all");
    }

    #[test]
    fn test_route_config_no_conflict_different_method() {
        let mut config = RouteConfig::new();
        config.add_route(RouteRule::new(HttpMethod::GET, "/users", "User@list"));
        config.add_route(RouteRule::new(HttpMethod::DELETE, "/users", "User@delete"));
        assert!(config.find_conflicts().is_empty());
    }

    #[test]
    fn test_route_config_conflict_in_group() {
        let mut config = RouteConfig::new();
        let mut group = RouteGroup::new("/api");
        group.add_route(RouteRule::new(HttpMethod::GET, "/items", "Item@list"));
        group.add_route(RouteRule::new(HttpMethod::GET, "/items", "Item@all"));
        config.add_group(group);

        let conflicts = config.find_conflicts();
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn test_route_config_conflict_between_top_and_group() {
        let mut config = RouteConfig::new();
        // 顶层 /api/items
        config.add_route(RouteRule::new(HttpMethod::GET, "/api/items", "Item@list"));
        // group prefix /api + /items = /api/items
        let mut group = RouteGroup::new("/api");
        group.add_route(RouteRule::new(HttpMethod::GET, "/items", "Item@all"));
        config.add_group(group);

        let conflicts = config.find_conflicts();
        assert_eq!(conflicts.len(), 1);
    }

    // ====================================================================
    // YAML 加载
    // ====================================================================

    #[test]
    fn test_load_routes_from_yaml_str_simple() {
        let yaml = r#"
routes:
  - method: GET
    path: /users
    handler: User@list
  - method: POST
    path: /users
    handler: User@create
"#;
        let config = load_routes_from_yaml_str(yaml).unwrap();
        assert_eq!(config.routes.len(), 2);
        assert_eq!(config.routes[0].method, HttpMethod::GET);
        assert_eq!(config.routes[0].path, "/users");
        assert_eq!(config.routes[0].handler, "User@list");
        assert_eq!(config.routes[1].method, HttpMethod::POST);
    }

    #[test]
    fn test_load_routes_from_yaml_str_with_groups() {
        let yaml = r#"
routes:
  - method: GET
    path: /health
    handler: Health@check
groups:
  - prefix: /api/v1
    middleware: [auth, log]
    routes:
      - method: GET
        path: /items
        handler: Item@list
      - method: POST
        path: /items
        handler: Item@create
"#;
        let config = load_routes_from_yaml_str(yaml).unwrap();
        assert_eq!(config.routes.len(), 1);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].prefix, "/api/v1");
        assert_eq!(config.groups[0].middleware, vec!["auth", "log"]);
        assert_eq!(config.groups[0].routes.len(), 2);

        let flat = config.flatten();
        assert_eq!(flat.len(), 3);
        assert!(flat.iter().any(|r| r.path == "/health"));
        assert!(flat.iter().any(|r| r.path == "/api/v1/items"));
    }

    #[test]
    fn test_load_routes_from_yaml_str_with_name_and_middleware() {
        let yaml = r#"
routes:
  - method: GET
    path: /users/{id}
    handler: User@show
    middleware: [auth, cache]
    name: user.show
"#;
        let config = load_routes_from_yaml_str(yaml).unwrap();
        assert_eq!(config.routes.len(), 1);
        let rule = &config.routes[0];
        assert_eq!(rule.middleware, vec!["auth", "cache"]);
        assert_eq!(rule.name, Some("user.show".to_string()));
    }

    #[test]
    fn test_load_routes_from_yaml_str_empty() {
        let yaml = "";
        let config = load_routes_from_yaml_str(yaml).unwrap();
        assert_eq!(config.routes.len(), 0);
        assert_eq!(config.groups.len(), 0);
    }

    #[test]
    fn test_load_routes_from_yaml_str_invalid_method() {
        let yaml = r#"
routes:
  - method: INVALID
    path: /users
    handler: User@list
"#;
        let result = load_routes_from_yaml_str(yaml);
        // serde_yaml 会因为 INVALID 无法反序列化为 HttpMethod 而失败
        assert!(result.is_err());
    }

    #[test]
    fn test_load_routes_from_yaml_str_invalid_yaml() {
        let yaml = "not: valid: yaml: at: all";
        let result = load_routes_from_yaml_str(yaml);
        assert!(result.is_err());
    }

    // ====================================================================
    // JSON 加载
    // ====================================================================

    #[test]
    fn test_load_routes_from_json_str_simple() {
        let json = r#"{
  "routes": [
    {"method": "GET", "path": "/users", "handler": "User@list"},
    {"method": "POST", "path": "/users", "handler": "User@create"}
  ]
}"#;
        let config = load_routes_from_json_str(json).unwrap();
        assert_eq!(config.routes.len(), 2);
        assert_eq!(config.routes[0].method, HttpMethod::GET);
        assert_eq!(config.routes[1].method, HttpMethod::POST);
    }

    #[test]
    fn test_load_routes_from_json_str_with_groups() {
        let json = r#"{
  "routes": [
    {"method": "GET", "path": "/health", "handler": "Health@check"}
  ],
  "groups": [
    {
      "prefix": "/api",
      "middleware": ["auth"],
      "routes": [
        {"method": "GET", "path": "/items", "handler": "Item@list"}
      ]
    }
  ]
}"#;
        let config = load_routes_from_json_str(json).unwrap();
        assert_eq!(config.routes.len(), 1);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].prefix, "/api");
    }

    #[test]
    fn test_load_routes_from_json_str_empty() {
        let json = "{}";
        let config = load_routes_from_json_str(json).unwrap();
        assert_eq!(config.routes.len(), 0);
        assert_eq!(config.groups.len(), 0);
    }

    #[test]
    fn test_load_routes_from_json_str_invalid() {
        let json = "{not valid json";
        let result = load_routes_from_json_str(json);
        assert!(result.is_err());
    }

    // ====================================================================
    // ConventionRoute
    // ====================================================================

    #[test]
    fn test_convention_route_from_uri_with_app() {
        let r = ConventionRoute::from_uri("/oapc/customer/index").unwrap();
        assert_eq!(r.app, "oapc");
        assert_eq!(r.controller, "Customer");
        assert_eq!(r.action, "index");
        assert_eq!(r.path, "/oapc/customer/index");
        assert_eq!(r.method, HttpMethod::GET);
    }

    #[test]
    fn test_convention_route_from_uri_admin_app() {
        let r = ConventionRoute::from_uri("/admin/login/index").unwrap();
        assert_eq!(r.app, "admin");
        assert_eq!(r.controller, "Login");
        assert_eq!(r.action, "index");
    }

    #[test]
    fn test_convention_route_from_uri_root_returns_none() {
        // 根路径 → 默认应用+控制器+action → 不生成约定式路由
        assert!(ConventionRoute::from_uri("/").is_none());
        assert!(ConventionRoute::from_uri("").is_none());
    }

    #[test]
    fn test_convention_route_from_uri_single_segment() {
        // /foo → (index, Foo, index)
        // 不在 app_map，所以 app=index，但 controller=Foo，action=index
        // 这个 case 会生成约定式路由吗？看实现：app=index, controller=Foo, action=index
        // 由于 action=index 是默认值，但 controller=Foo 不是默认值，所以应生成
        let r = ConventionRoute::from_uri("/customer").unwrap();
        assert_eq!(r.app, "index");
        assert_eq!(r.controller, "Customer");
        assert_eq!(r.action, "index");
    }

    #[test]
    fn test_convention_route_from_parsed() {
        let parsed = ParsedPath::new("api", "User", "list");
        let r = ConventionRoute::from_parsed(parsed).unwrap();
        assert_eq!(r.app, "api");
        assert_eq!(r.controller, "User");
        assert_eq!(r.action, "list");
    }

    // ====================================================================
    // RouteRegistry
    // ====================================================================

    #[test]
    fn test_route_registry_new() {
        let r = RouteRegistry::new();
        assert!(r.attribute_routes.is_empty());
        assert!(r.config_routes.is_empty());
        assert!(r.convention_routes.is_empty());
        assert_eq!(r.total_count(), 0);
    }

    #[test]
    fn test_route_registry_add_attribute_route() {
        let mut r = RouteRegistry::new();
        r.add_attribute_route(RouteRule::new(HttpMethod::GET, "/users", "User@list"));
        assert_eq!(r.attribute_routes.len(), 1);
        assert_eq!(r.total_count(), 1);
    }

    #[test]
    fn test_route_registry_add_attribute_routes_batch() {
        let mut r = RouteRegistry::new();
        r.add_attribute_routes(vec![
            RouteRule::new(HttpMethod::GET, "/users", "User@list"),
            RouteRule::new(HttpMethod::POST, "/users", "User@create"),
        ]);
        assert_eq!(r.attribute_routes.len(), 2);
    }

    #[test]
    fn test_route_registry_add_config_routes() {
        let mut r = RouteRegistry::new();
        let mut config = RouteConfig::new();
        config.add_route(RouteRule::new(HttpMethod::GET, "/items", "Item@list"));
        config.add_route(RouteRule::new(HttpMethod::POST, "/items", "Item@create"));
        r.add_config_routes(&config);
        assert_eq!(r.config_routes.len(), 2);
    }

    #[test]
    fn test_route_registry_add_convention_route() {
        let mut r = RouteRegistry::new();
        let cr = ConventionRoute::from_uri("/oapc/customer/index").unwrap();
        r.add_convention_route(cr);
        assert_eq!(r.convention_routes.len(), 1);
    }

    #[test]
    fn test_route_registry_convention_as_rules() {
        let mut r = RouteRegistry::new();
        r.add_convention_route(ConventionRoute::from_uri("/oapc/customer/index").unwrap());
        r.add_convention_route(ConventionRoute::from_uri("/admin/login/index").unwrap());

        let rules = r.convention_as_rules();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].handler, "Customer@index");
        assert_eq!(rules[1].handler, "Login@index");
        assert_eq!(
            rules[0].name,
            Some("convention.oapc.Customer.index".to_string())
        );
    }

    #[test]
    fn test_route_registry_merged_rules_attribute_overrides_config() {
        let mut r = RouteRegistry::new();
        // Layer 2 - config
        r.add_config_routes(&RouteConfig {
            routes: vec![RouteRule::new(HttpMethod::GET, "/users", "User@old")],
            groups: vec![],
        });
        // Layer 1 - attribute (优先级更高，覆盖 config)
        r.add_attribute_route(RouteRule::new(HttpMethod::GET, "/users", "User@new"));

        let merged = r.merged_rules();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].handler, "User@new");
    }

    #[test]
    fn test_route_registry_merged_rules_config_overrides_convention() {
        let mut r = RouteRegistry::new();
        // Layer 3 - convention
        r.add_convention_route(ConventionRoute::from_uri("/oapc/customer/index").unwrap());
        // Layer 2 - config (优先级更高，覆盖 convention)
        r.add_config_routes(&RouteConfig {
            routes: vec![RouteRule::new(
                HttpMethod::GET,
                "/oapc/customer/index",
                "Customer@custom",
            )],
            groups: vec![],
        });

        let merged = r.merged_rules();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].handler, "Customer@custom");
    }

    #[test]
    fn test_route_registry_merged_rules_different_paths_no_override() {
        let mut r = RouteRegistry::new();
        r.add_attribute_route(RouteRule::new(HttpMethod::GET, "/users", "User@list"));
        r.add_config_routes(&RouteConfig {
            routes: vec![RouteRule::new(HttpMethod::GET, "/items", "Item@list")],
            groups: vec![],
        });
        r.add_convention_route(ConventionRoute::from_uri("/oapc/customer/index").unwrap());

        let merged = r.merged_rules();
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn test_route_registry_attribute_conflicts() {
        let mut r = RouteRegistry::new();
        r.add_attribute_route(RouteRule::new(HttpMethod::GET, "/users", "User@list"));
        r.add_attribute_route(RouteRule::new(HttpMethod::GET, "/users", "User@all"));

        let conflicts = r.attribute_conflicts();
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn test_route_registry_config_conflicts() {
        let mut r = RouteRegistry::new();
        r.add_config_routes(&RouteConfig {
            routes: vec![
                RouteRule::new(HttpMethod::GET, "/users", "User@list"),
                RouteRule::new(HttpMethod::GET, "/users", "User@all"),
            ],
            groups: vec![],
        });

        let conflicts = r.config_conflicts();
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn test_route_registry_no_conflicts() {
        let mut r = RouteRegistry::new();
        r.add_attribute_route(RouteRule::new(HttpMethod::GET, "/users", "User@list"));
        r.add_attribute_route(RouteRule::new(HttpMethod::POST, "/users", "User@create"));

        assert!(r.attribute_conflicts().is_empty());
    }

    #[test]
    fn test_route_registry_total_count() {
        let mut r = RouteRegistry::new();
        r.add_attribute_route(RouteRule::new(HttpMethod::GET, "/a", "A@index"));
        r.add_config_routes(&RouteConfig {
            routes: vec![RouteRule::new(HttpMethod::GET, "/b", "B@index")],
            groups: vec![],
        });
        r.add_convention_route(ConventionRoute::from_uri("/oapc/c/d").unwrap());

        assert_eq!(r.total_count(), 3);
    }

    // ====================================================================
    // 集成测试 - 完整三层路由流程
    // ====================================================================

    #[test]
    fn test_integration_three_layer_routing() {
        // Layer 1 - 属性宏路由
        let mut r = RouteRegistry::new();
        r.add_attribute_routes(vec![
            RouteRule::new(HttpMethod::GET, "/users", "User@list"),
            RouteRule::new(HttpMethod::POST, "/users", "User@create"),
            RouteRule::new(HttpMethod::GET, "/users/{id}", "User@show"),
        ]);

        // Layer 2 - 配置式路由
        let yaml = r#"
routes:
  - method: GET
    path: /items
    handler: Item@list
  - method: POST
    path: /items
    handler: Item@create
groups:
  - prefix: /api/v1
    middleware: [auth]
    routes:
      - method: GET
        path: /orders
        handler: Order@list
"#;
        let config = load_routes_from_yaml_str(yaml).unwrap();
        r.add_config_routes(&config);

        // Layer 3 - 约定式路由
        r.add_convention_route(ConventionRoute::from_uri("/oapc/customer/index").unwrap());
        r.add_convention_route(ConventionRoute::from_uri("/admin/login/index").unwrap());

        // 验证总数
        assert_eq!(r.attribute_routes.len(), 3);
        assert_eq!(r.config_routes.len(), 3); // 2 顶层 + 1 group
        assert_eq!(r.convention_routes.len(), 2);
        assert_eq!(r.total_count(), 8);

        // 合并后应无冲突（各层路径不重复）
        let merged = r.merged_rules();
        assert_eq!(merged.len(), 8);

        // 验证各层无内部冲突
        assert!(r.attribute_conflicts().is_empty());
        assert!(r.config_conflicts().is_empty());
    }

    #[test]
    fn test_integration_layer_override_priority() {
        // 三层都注册同一路径，attribute 应覆盖
        let mut r = RouteRegistry::new();

        // Layer 3 - convention
        r.add_convention_route(ConventionRoute {
            app: "index".to_string(),
            controller: "User".to_string(),
            action: "list".to_string(),
            method: HttpMethod::GET,
            path: "/users".to_string(),
        });

        // Layer 2 - config
        r.add_config_routes(&RouteConfig {
            routes: vec![RouteRule::new(HttpMethod::GET, "/users", "User@config")],
            groups: vec![],
        });

        // Layer 1 - attribute (优先级最高)
        r.add_attribute_route(RouteRule::new(HttpMethod::GET, "/users", "User@attribute"));

        let merged = r.merged_rules();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].handler, "User@attribute");
    }
}
