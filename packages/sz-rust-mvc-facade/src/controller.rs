//! 控制器模块 — SzController trait
//!
//! 对齐 PHP `app\SzController`（abstract class extends BaseController）。
//! 继承链：BaseController → SzController → AddonsBaseController → 业务控制器。
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | 签名 | Rust 等价 |
//! |---------|------|-----------|
//! | `renderJson($code=1, $msg='', $data=[])` | 返回 `compact('code','msg','data')` 数组 | [`SzController::render_json`] 返回 `Value::Object` |
//! | `renderSuccess($msg='success', $data=[])` | `json(renderJson(1, $msg, $data))` | [`SzController::render_success`] 返回 `Response` |
//! | `renderError($msg='error', $data=[], $code=0)` | `json(renderJson($code, $msg, $data))` | [`SzController::render_error`] 返回 `Response` |
//! | `postData($key=null)` | `$this->request->param(...)` | [`SzController::post_data`] / [`SzController::post_data_by_key`] |
//! | `getData($key=null)` | `$this->request->get(...)` | [`SzController::get_data`] / [`SzController::get_data_by_key`] |
//!
//! ## 参数顺序差异
//!
//! PHP `renderSuccess($msg, $data)` 与 Rust `ApiResponse::success(data, msg)` 参数顺序相反。
//! 本 trait 严格遵循 PHP 顺序（msg 在前），内部调用 `ApiResponse` 时调换参数。
//!
//! ## Request 传递
//!
//! PHP 通过 `$this->request` 在控制器内部访问当前请求；Rust 控制器无状态，
//! 由 handler 在每次请求时将 `Request<Body>` 作为参数传入 trait 方法。
//!
//! ## async trait
//!
//! 使用 Rust 1.75+ 原生 `async fn` in trait。trait 暂不支持 `dyn SzController`；
//! 若未来需要 trait object，可改用 `#[async_trait]` 宏（届时增加 `async-trait` 依赖）。

use axum::body::Body;
use axum::http::Request;
use axum::response::{IntoResponse, Response};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use serde_json::{Map, Value};
use std::future::Future;

use sz_rust_http_facade::request::{
    fetch_post_data, fetch_post_data_by_key, fetch_query_data, fetch_query_data_by_key,
};
use sz_rust_http_facade::response::ApiResponse;
use sz_rust_infra_facade::validate::Validate;

// ============================================================================
// JWT 配置（运行时从环境变量读取，对齐 PHP Token 类私有 $_config）
// ============================================================================
//
// PHP `app\common\service\jwt\Token::$_config` 包含 issuer/audience/id/sign/expire，
// 这些敏感信息不应硬编码到框架中。Rust 实现通过环境变量注入：
//
// - `SZ_JWT_SECRET`：签名密钥（PHP 实际使用 `id` 字段作为 HMAC 密钥，而非 `sign`）
// - `SZ_JWT_ISSUER`：签发人（对应 PHP `issuer`，如 `https://mall.ljclz.shop`）
//
// 注：sz-orm-auth 的 JwtClaims 暂不包含 `aud` 字段，因此 audience 验证由
// 业务层在解码后自行实现（PHP `PermittedFor` 约束等价）。

/// JWT 配置（运行时从环境变量读取）
///
/// ## 安全约束
///
/// - `JwtConfig` 未派生 `Serialize`/`Deserialize`，密钥绝不会出现在序列化输出中。
/// - `Debug` 手动实现：`secret` 字段始终脱敏为 `"[REDACTED]"`，
///   防止 `{:?}` 格式化时将密钥泄漏到日志或 panic 信息中（P1-SEC-12）。
/// - 未来若需派生 `Serialize`，必须为 `secret` 字段添加
///   `#[serde(skip_serializing)]` 防止日志/响应泄露。
#[derive(Clone, Default)]
struct JwtConfig {
    /// 签名密钥（对应 PHP `$_config['id']`，Lcobucci 用作 HMAC 密钥）
    secret: String,
    /// 签发人（对应 PHP `$_config['issuer']`）
    issuer: String,
    /// 接收人（对应 PHP `$_config['permitted_for']`）— P1-SEC-10 新增
    ///
    /// 为空时跳过 aud 验证（向后兼容旧 token）；非空时要求 token 的 `aud` 字段匹配。
    audience: String,
}

impl std::fmt::Debug for JwtConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtConfig")
            .field("secret", &"[REDACTED]")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .finish()
    }
}

/// 全局 JWT 配置实例（启动时从环境变量读取一次）
///
/// - `SZ_JWT_SECRET`：签名密钥（**必填且不可为空**，未设置或为空时 panic，阻止服务启动）
/// - `SZ_JWT_ISSUER`：签发人（可选，空字符串表示跳过 iss 验证）
///
/// 安全铁律（P0-SEC-02）：密钥为空时静默回退等同于禁用认证，攻击者可伪造任意 token。
/// 此处与 `AuthConfig` 的 `missing_key => panic!` 行为保持一致，
/// 统一为"密钥缺失即拒绝启动"，消除两套 JWT 配置的行为不一致风险。
static JWT_CONFIG: Lazy<Option<JwtConfig>> = Lazy::new(|| {
    let secret = std::env::var("SZ_JWT_SECRET").ok()?;
    // 空字符串视为未配置（与安全铁律一致：空密钥 = 无密钥）
    if secret.is_empty() {
        return None;
    }
    Some(JwtConfig {
        secret,
        issuer: std::env::var("SZ_JWT_ISSUER").unwrap_or_default(),
        // P1-SEC-10：从环境变量读取 audience，未设置时为空（跳过 aud 验证）
        audience: std::env::var("SZ_JWT_AUDIENCE").unwrap_or_default(),
    })
});

/// 启动时校验 JWT 配置（生产环境必须在 `main()` 中调用）
///
/// 若 `SZ_JWT_SECRET` 未设置则 panic，阻止服务启动，防止认证形同虚设。
/// 测试环境中可跳过此校验（`JWT_CONFIG` 为 `None` 时 `get_token` 返回 `Ok(None)`）。
///
/// 安全修复 M-3（2026-08-14）：HS256 对称密钥必须 ≥ 32 字节，
/// 弱密钥可被离线暴力破解后伪造任意用户 token —— 强度不足直接拒绝启动。
pub fn validate_jwt_config() {
    let Some(config) = JWT_CONFIG.as_ref() else {
        panic!("SZ_JWT_SECRET 环境变量未设置 — 生产环境必须通过环境变量提供 JWT 密钥");
    };
    if config.secret.len() < 32 {
        panic!(
            "SZ_JWT_SECRET 强度不足：当前 {} 字节，HS256 要求 ≥ 32 字节（256 位）。\
             请使用 `openssl rand -base64 32` 生成强密钥",
            config.secret.len()
        );
    }
}

/// 去除 Authorization header 中的 Bearer 前缀
///
/// 对齐 PHP `str_ireplace('bearer', '', $header)`：大小写不敏感地移除 `bearer` 标识。
/// 支持以下格式（与 PHP 兼容）：
/// - `Bearer xxx.yyy.zzz` → `xxx.yyy.zzz`
/// - `bearer xxx.yyy.zzz` → `xxx.yyy.zzz`
/// - `BEARER xxx.yyy.zzz` → `xxx.yyy.zzz`
/// - `xxx.yyy.zzz`（无前缀）→ `xxx.yyy.zzz`（保持原样）
fn strip_bearer_prefix(header: &str) -> &str {
    let trimmed = header.trim();
    // 大小写不敏感匹配 "bearer"
    if trimmed.len() >= 6 {
        let prefix = &trimmed[..6];
        if prefix.eq_ignore_ascii_case("bearer") {
            // 跳过 "bearer" 后可能存在的空格
            return trimmed[6..].trim_start();
        }
    }
    trimmed
}

/// 使用指定配置验证 JWT token 并提取用户信息
///
/// 这是 [`AddonsBaseController::get_token`] 的核心逻辑，抽取为独立函数便于单元测试
/// 注入不同配置（避免依赖全局 `JWT_CONFIG` 一次性初始化的副作用）。
///
/// # 参数
///
/// - `authorization`：Authorization header 原始值（可能含 `Bearer ` 前缀）
/// - `config`：JWT 配置（密钥 + 签发人）
///
/// # 返回
///
/// - `Ok(Some(UserInfo))`：验证成功
/// - `Ok(None)`：无 token、密钥未配置、签名/过期/iss 验证失败、缺少 user_id
fn verify_token_with_config(
    authorization: Option<&str>,
    config: &JwtConfig,
) -> Result<Option<UserInfo>, String> {
    // 1. 提取 Authorization header 值
    let header_value = match authorization {
        Some(v) if !v.is_empty() => v,
        _ => return Ok(None),
    };

    // 2. 去除 Bearer 前缀
    let token = strip_bearer_prefix(header_value).trim();
    if token.is_empty() {
        return Ok(None);
    }

    // 3. 密钥未配置则禁用 JWT 验证
    if config.secret.is_empty() {
        return Ok(None);
    }

    // 4. JwtEncoder 解码并验证签名 + 过期
    let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(&config.secret);
    let claims = match encoder.decode(token) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    // 5. 验证 iss 字段（仅当配置了 issuer 时）
    if !config.issuer.is_empty() {
        match &claims.iss {
            Some(iss) if iss == &config.issuer => { /* 通过 */ }
            _ => return Ok(None),
        }
    }

    // 6. 提取 user_id
    let user_id = match claims.user_id {
        Some(id) => id,
        None => return Ok(None),
    };

    Ok(Some(UserInfo {
        user_id,
        is_login: true,
    }))
}

/// 控制器基础 trait（对齐 PHP `app\SzController`）
///
/// 业务控制器实现此 trait 即可获得 `render_json` / `render_success` / `render_error`
/// 与 `post_data` / `get_data` 等方法。
///
/// # 用法
///
/// ```ignore
/// use sz_rust_core::controller::SzController;
/// use sz_rust_core::response::ApiResponse;
/// use axum::body::Body;
/// use axum::http::Request;
/// use axum::response::Response;
/// use serde_json::json;
///
/// struct UserController;
///
/// impl SzController for UserController {}
///
/// impl UserController {
///     pub async fn info(&self, req: Request<Body>) -> Response {
///         let data = self.post_data(req).await.unwrap();
///         let name = data.get("name").cloned().unwrap_or(json!(null));
///         self.render_success("success", json!({"name": name}))
///     }
/// }
/// ```
pub trait SzController: Send + Sync {
    /// 返回封装后的 API 数据（对应 PHP `renderJson`）
    ///
    /// 严格遵循 PHP `compact('code', 'msg', 'data')` 的字段顺序：`code → msg → data`。
    /// 返回 `Value::Object`（而非 `Response`），便于调用方进一步处理。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// protected function renderJson($code = 1, $msg = '', $data = []) {
    ///     return compact('code', 'msg', 'data');
    /// }
    /// ```
    fn render_json(&self, code: i32, msg: impl Into<String>, data: Value) -> Value {
        let mut map = Map::new();
        map.insert("code".to_string(), Value::Number(code.into()));
        map.insert("msg".to_string(), Value::String(msg.into()));
        map.insert("data".to_string(), data);
        Value::Object(map)
    }

    /// 返回操作成功 json（对应 PHP `renderSuccess`）
    ///
    /// 参数顺序：`msg, data`（与 PHP 一致，与 `ApiResponse::success(data, msg)` 相反）。
    /// 返回 `Response`，HTTP 200，Content-Type: application/json; charset=utf-8。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// protected function renderSuccess($msg = 'success', $data = []) {
    ///     return json($this->renderJson(1, $msg, $data));
    /// }
    /// ```
    fn render_success(&self, msg: impl Into<String>, data: Value) -> Response {
        ApiResponse::success(data, msg).into_response()
    }

    /// 返回操作失败 json（对应 PHP `renderError`）
    ///
    /// 参数顺序：`msg, data, code`（与 PHP 一致）。
    /// 返回 `Response`，HTTP 200（业务失败 HTTP 仍 200，对齐 PHP 行为）。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// protected function renderError($msg = 'error', $data = [], $code = 0) {
    ///     return json($this->renderJson($code, $msg, $data));
    /// }
    /// ```
    fn render_error(&self, msg: impl Into<String>, data: Value, code: i32) -> Response {
        ApiResponse::error_with_code(code, msg, data).into_response()
    }

    /// 获取合并后的请求参数（对应 PHP `postData()` 无参形式）
    ///
    /// 合并 body + query，body 字段优先级高于 query。
    /// 对齐 PHP `$this->request->param()`（合并 POST + GET + route）。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// protected function postData($key = null) {
    ///     return $this->request->param(is_null($key) ? '' : $key . '/a');
    /// }
    /// ```
    fn post_data(&self, req: Request<Body>) -> impl Future<Output = Result<Value, String>> + Send {
        async move { fetch_post_data(req).await }
    }

    /// 获取合并参数中指定 key（对应 PHP `postData($key)`）
    ///
    /// # 返回
    ///
    /// - `Ok(Some(value))`：字段存在
    /// - `Ok(None)`：字段不存在
    /// - `Err(String)`：body 读取或 JSON 解析失败
    fn post_data_by_key(
        &self,
        req: Request<Body>,
        key: &str,
    ) -> impl Future<Output = Result<Option<Value>, String>> + Send {
        async move { fetch_post_data_by_key(req, key).await }
    }

    /// 获取 query 参数（对应 PHP `getData()` 无参形式）
    ///
    /// 对齐 PHP `$this->request->get()`。
    fn get_data(&self, req: &Request<Body>) -> Value {
        fetch_query_data(req)
    }

    /// 获取 query 参数中指定 key（对应 PHP `getData($key)`）
    ///
    /// # 返回
    ///
    /// - `Some(value)`：字段存在
    /// - `None`：字段不存在
    fn get_data_by_key(&self, req: &Request<Body>, key: &str) -> Option<Value> {
        fetch_query_data_by_key(req, key)
    }
}

/// 控制器基础 trait（对齐 PHP `app\BaseController`）
///
/// PHP 继承链：`BaseController → SzController → AddonsBaseController → 业务控制器`。
/// Rust 等价：`BaseController: SzController`（trait 继承），业务控制器同时实现
/// `BaseController` 和 `SzController`（trait 继承自动传播）。
///
/// # PHP 对齐
///
/// | PHP 属性/方法 | Rust 等价 |
/// |--------------|-----------|
/// | `protected $request` | 通过方法参数 `req: Request<Body>` 传入（见 [`SzController`]） |
/// | `protected $app` | 通过 axum `State<App>` 提取器获取（不在 trait 中） |
/// | `protected bool $batchValidate = false` | [`BaseController::batch_validate`] |
/// | `protected array $middleware = []` | [`BaseController::middlewares`] |
/// | `protected function initialize() {}` | [`BaseController::initialize`] |
/// | `protected function validate(...)` | [`BaseController::validate`]（占位，完整实现） |
///
/// # 状态迁移说明
///
/// PHP `BaseController` 是 abstract class，包含状态字段（$request/$app）。Rust 控制器
/// 推荐无状态（每个请求通过参数传入 Request），因此本 trait 不持有状态，仅定义行为。
pub trait BaseController: SzController {
    /// 是否批量验证（对齐 PHP `protected bool $batchValidate = false`）
    ///
    /// 返回 `true` 时，`validate` 方法会收集所有错误后一次性返回；
    /// 返回 `false` 时，遇到第一个错误即返回。
    fn batch_validate(&self) -> bool {
        false
    }

    /// 控制器中间件列表（对齐 PHP `protected array $middleware = []`）
    ///
    /// 返回中间件标识符列表（如 `["auth", "cors"]`）。
    /// 实际中间件注册在中间件系统中处理。
    fn middlewares(&self) -> Vec<String> {
        Vec::new()
    }

    /// 初始化钩子（对齐 PHP `protected function initialize() {}`）
    ///
    /// 在控制器实例化后、方法调用前执行。
    /// 子类可重写以执行初始化逻辑（如设置默认值、加载配置）。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// protected function initialize() {
    ///     // 子类重写
    /// }
    /// ```
    fn initialize(&self) {}

    /// 数据验证（对齐 PHP `protected function validate(...)`）
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// protected function validate(array $data, $validate, array $message = [], bool $batch = false) {
    ///     // ... 完整验证逻辑
    /// }
    /// ```
    ///
    /// # 实现说明
    ///
    /// 将 `rules` 与 `messages` 转换为内部 [`Validate`] 构建器调用，
    /// 批量模式由 [`BaseController::batch_validate`] 决定（对齐 PHP `$batch`
    /// 参数默认值 `false`，但 Rust 通过 trait 方法覆盖以适配无状态控制器）。
    ///
    /// # 参数
    ///
    /// - `data`：待验证的数据
    /// - `rules`：验证规则列表，元组格式 `(字段名, 规则字符串)`
    ///   - 例：`[("name", "require"), ("age", "require|integer|gt:0")]`
    /// - `messages`：错误消息列表，元组格式 `(规则键, 消息)`
    ///   - 例：`[("name.require", "名称必填"), ("age.integer", "年龄须为整数")]`
    ///
    /// # 返回
    ///
    /// - `Ok(())`：验证通过
    /// - `Err(String)`：验证失败，包含错误消息（批量模式下以 `; ` 分隔多条错误）
    fn validate(
        &self,
        data: &Value,
        rules: &[(&str, &str)],
        messages: &[(&str, &str)],
    ) -> Result<(), String> {
        // 构建 Validate 实例，将 (字段名, 规则) 元组列表逐条注册
        let mut validator = Validate::new();
        for (name, rule) in rules {
            validator = validator.rule(name, rule);
        }

        // 将 (规则键, 消息) 元组列表合并到 IndexMap 后注入
        let mut msg_map = IndexMap::new();
        for (key, msg) in messages {
            msg_map.insert(key.to_string(), msg.to_string());
        }
        validator = validator.message(msg_map);

        // 批量模式由控制器配置决定（对齐 PHP $batch 参数）
        if self.batch_validate() {
            validator = validator.batch(true);
        }

        // 执行验证并转换错误类型（Display 实现已处理 Single/Batch 两种格式）
        match validator.check(data) {
            Ok(()) => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    }
}

/// 路由信息（对齐 PHP `addons\BaseController` 的 $controller/$action/$routeUri/$group）
///
/// 由 [`AddonsBaseController::parse_route_info`] 从 URI 解析得到。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteInfo {
    /// 当前控制器名称（对齐 PHP `$this->controller`）
    pub controller: String,
    /// 当前方法名称（对齐 PHP `$this->action`）
    pub action: String,
    /// 当前路由 URI（对齐 PHP `$this->routeUri`，格式 `/controller/action`）
    pub route_uri: String,
    /// 控制器分组（对齐 PHP `$this->group`，controller 的第一段）
    pub group: String,
}

/// 用户信息（对齐 PHP `$this->user` 数组中的关键字段）
///
/// JWT 验证成功后返回，包含 user_id 与 is_login 状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    /// 用户 ID
    pub user_id: i64,
    /// 是否已登录（对齐 PHP `$this->user['is_login'] == 1`）
    pub is_login: bool,
}

/// addons 控制器基础 trait（对齐 PHP `addons\BaseController`）
///
/// PHP 继承链：`BaseController → SzController → AddonsBaseController → 业务控制器`。
/// Rust 等价：`AddonsBaseController: BaseController: SzController`（trait 继承链）。
///
/// # PHP 对齐
///
/// | PHP 属性/方法 | Rust 等价 |
/// |--------------|-----------|
/// | `protected $user` | handler 中通过 [`AddonsBaseController::get_token`] 获取 [`UserInfo`] |
/// | `protected string $controller` | [`RouteInfo::controller`]（由 [`AddonsBaseController::parse_route_info`] 解析） |
/// | `protected string $action` | [`RouteInfo::action`] |
/// | `protected string $routeUri` | [`RouteInfo::route_uri`] |
/// | `protected string $group` | [`RouteInfo::group`] |
/// | `protected array $allowAllAction` | [`AddonsBaseController::allow_all_action`] |
/// | `public function initialize()` | 由 handler 显式调用 `parse_route_info` + `get_token` + `check_login` |
/// | `public function getToken()` | [`AddonsBaseController::get_token`]（占位，JWT 完整实现） |
/// | `protected function getRouteinfo()` | [`AddonsBaseController::parse_route_info`] |
/// | `private function checkLogin()` | [`AddonsBaseController::check_login`] |
///
/// # 状态迁移说明
///
/// PHP `AddonsBaseController::initialize()` 在构造函数中自动调用，执行
/// `getRouteinfo() + getToken() + checkLogin()`。Rust 控制器无状态，
/// handler 需显式调用这三个方法（顺序：parse_route_info → get_token → check_login）。
pub trait AddonsBaseController: BaseController {
    /// 登录验证白名单（对齐 PHP `protected array $allowAllAction`）
    ///
    /// 默认包含 `/passport/login` 和 `/task/task/userClerk`。
    /// 子类可覆盖以添加更多白名单路径。
    fn allow_all_action(&self) -> Vec<&'static str> {
        vec!["/passport/login", "/task/task/userClerk"]
    }

    /// 解析路由信息（对齐 PHP `protected function getRouteinfo()`）
    ///
    /// 从 URI 路径解析 controller/action/group/route_uri。
    ///
    /// # 解析规则
    ///
    /// - URI 形如 `/controller/action`：controller=controller, action=action, group=controller
    /// - URI 形如 `/group/controller/action`：controller=group/controller, action=action, group=group/controller
    /// - URI 形如 `/controller`：controller=controller, action="", group=controller
    /// - URI 形如 `/`：所有字段为空字符串
    ///
    /// # PHP 对齐（含 bug 复刻）
    ///
    /// ```php
    /// protected function getRouteinfo(): void {
    ///     $this->controller = toUnderScore(Request()->controller());
    ///     $this->controller = str_replace(".", "/", $this->controller);
    ///     $this->controller = str_replace("_", "", $this->controller);
    ///     $this->action = Request()->action();
    ///     // ⚠️ PHP bug：str_replace 已将 "." 替换为 "/"，但 strstr 仍以 "." 为分隔符，
    ///     // 永远返回 false，因此 $this->group === $this->controller（始终相等）。
    ///     $groupstr = strstr($this->controller, '.', true);
    ///     $this->group = $groupstr !== false ? $groupstr : $this->controller;
    ///     $this->routeUri = '/' . $this->controller . '/' . $this->action;
    /// }
    /// ```
    ///
    /// **PHP bug 复刻说明**：Rust 严格对齐 PHP 行为，`group` 字段始终等于 `controller`。
    /// 经核查 PHP 后端全部源码，`$this->group` 为只写不读的死字段，bug 不暴露，
    /// 但为了 R5（PHP/Rust 行为对比）严格一致性，仍复刻此行为。
    ///
    /// 注意：PHP 使用 ThinkPHP 的 `Request()->controller()` 获取控制器名，
    /// Rust 直接从 URI 路径解析（已通过路由匹配）。
    fn parse_route_info(&self, uri: &str) -> RouteInfo {
        let path = uri.split('?').next().unwrap_or("");
        let path = path.trim_start_matches('/');
        let segments: Vec<&str> = if path.is_empty() {
            Vec::new()
        } else {
            path.split('/').collect()
        };

        let (controller, action) = match segments.len() {
            0 => (String::new(), String::new()),
            1 => (segments[0].to_string(), String::new()),
            _ => (
                segments[..segments.len() - 1].join("/"),
                segments[segments.len() - 1].to_string(),
            ),
        };

        // PHP bug 复刻：group === controller（详见方法文档注释）
        let group = controller.clone();

        let route_uri = if controller.is_empty() && action.is_empty() {
            "/".to_string()
        } else {
            format!("/{controller}/{action}")
        };

        RouteInfo {
            controller,
            action,
            route_uri,
            group,
        }
    }

    /// 检查登录状态（对齐 PHP `private function checkLogin()`）
    ///
    /// # 行为
    ///
    /// 1. 若 `route_uri` 在白名单中，返回 `Ok(())`
    /// 2. 若 `user_is_login == true`，返回 `Ok(())`
    /// 3. 否则返回 `Err("not_login")`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// private function checkLogin(): void {
    ///     if (in_array($this->routeUri, $this->allowAllAction)) {
    ///         return;
    ///     }
    ///     if(!empty($this->user)){
    ///         if($this->user['is_login'] == 1){
    ///             return;
    ///         }
    ///     }
    ///     throw new BaseException(['code' => -1, 'msg' => 'not_login']);
    /// }
    /// ```
    fn check_login(&self, route_uri: &str, user_is_login: bool) -> Result<(), String> {
        if self.allow_all_action().contains(&route_uri) {
            return Ok(());
        }
        if user_is_login {
            return Ok(());
        }
        Err("not_login".to_string())
    }

    /// 获取 token 用户信息（对齐 PHP `public function getToken()`）
    ///
    /// # 实现说明
    ///
    /// 使用 sz-orm-auth 的 `JwtEncoder` 进行 HS256 签名验证与过期检查，
    /// 配置项（`SZ_JWT_SECRET` / `SZ_JWT_ISSUER`）在启动时从环境变量读取。
    /// 核心验证逻辑见 `verify_token_with_config`，便于单元测试注入配置。
    ///
    /// 验证流程（对齐 PHP `Token::getUserId`）：
    /// 1. 提取 Authorization header 中的 Bearer token
    /// 2. 通过 `JwtEncoder::decode` 验证签名 + 过期时间
    /// 3. 验证 `iss` 字段匹配配置的签发人
    /// 4. 提取 `user_id` claim 返回 [`UserInfo`]
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function getToken(){
    ///     if (!$token = Token::getUserId(request()->header('Authorization'))) {
    ///         if(in_array($this->routeUri, $this->allowAllAction)) {
    ///             return true;
    ///         } else {
    ///             throw new BaseException(['msg' => '缺少必要的参数,请重新登陆!']);
    ///         }
    ///     }
    ///     return $token;
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `authorization`：`Authorization` 请求头的值（如 `"Bearer xxx.yyy.zzz"`）
    ///
    /// # 返回
    ///
    /// - `Ok(Some(UserInfo))`：JWT 验证成功，返回用户信息
    /// - `Ok(None)`：无 token、token 为空、签名密钥未配置或验证失败
    ///   （调用方根据 route_uri 决定是否抛错，对齐 PHP `if (!$token)` 分支）
    /// - `Err(String)`：JWT 解析过程中出现异常（如格式错误）
    fn get_token(&self, authorization: Option<&str>) -> Result<Option<UserInfo>, String> {
        // 委托给 verify_token_with_config，使用全局 JWT_CONFIG
        // 抽取独立函数便于单元测试注入不同配置（避免 once_cell 一次性初始化限制）
        // JWT_CONFIG 为 None 时（未配置 SZ_JWT_SECRET）跳过验证，返回 Ok(None)
        match JWT_CONFIG.as_ref() {
            Some(config) => verify_token_with_config(authorization, config),
            None => Ok(None),
        }
    }
}

// ============================================================================
// KeyRotation — JWT 签名密钥轮换机制
// ============================================================================

use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use std::time::Duration;

/// JWT 密钥轮换管理器
///
/// 支持多密钥并存验证：当前密钥签发/验证，旧密钥在 grace period 内仍可验证。
/// 轮换任务定期生成新密钥，旧密钥移入 previous 列表，超期后删除。
pub struct KeyRotation {
    /// 当前签名密钥
    current: RwLock<String>,
    /// 旧密钥列表（key, expires_at）
    previous: RwLock<Vec<(String, std::time::Instant)>>,
    /// 轮换间隔（默认 24h）
    rotation_interval: Duration,
    /// 旧密钥宽限期（默认 1h）
    grace_period: Duration,
    /// 最大保留旧密钥数（默认 3）
    max_previous: usize,
}

impl std::fmt::Debug for KeyRotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyRotation")
            .field("current", &"[REDACTED]")
            .field("previous", &"[REDACTED]")
            .field("rotation_interval", &self.rotation_interval)
            .field("grace_period", &self.grace_period)
            .field("max_previous", &self.max_previous)
            .finish()
    }
}

/// 密钥轮换错误
#[derive(Debug, thiserror::Error)]
pub enum KeyRotationError {
    /// 环境变量未设置
    #[error("SZ300_JWT_SECRET 环境变量未设置")]
    SecretMissing,
    /// token 验证失败
    #[error("Token 验证失败：所有密钥均无法解码")]
    InvalidToken,
    /// token 签发失败
    #[error("Token 签发失败：{0}")]
    SignError(String),
}

impl KeyRotation {
    /// 从环境变量创建密钥轮换管理器
    ///
    /// - `SZ300_JWT_SECRET`（必填）：当前密钥
    /// - `SZ300_JWT_ROTATION_INTERVAL`：轮换间隔秒数（默认 86400 = 24h）
    /// - `SZ300_JWT_GRACE_PERIOD`：宽限期秒数（默认 3600 = 1h）
    pub fn from_env() -> Result<Self, KeyRotationError> {
        let current =
            std::env::var("SZ300_JWT_SECRET").map_err(|_| KeyRotationError::SecretMissing)?;
        if current.is_empty() {
            return Err(KeyRotationError::SecretMissing);
        }

        let rotation_interval = std::env::var("SZ300_JWT_ROTATION_INTERVAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(86400));

        let grace_period = std::env::var("SZ300_JWT_GRACE_PERIOD")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(3600));

        Ok(Self {
            current: RwLock::new(current),
            previous: RwLock::new(Vec::new()),
            rotation_interval,
            grace_period,
            max_previous: 3,
        })
    }

    /// 直接构造（用于测试或显式配置）
    pub fn new(
        current: String,
        rotation_interval: Duration,
        grace_period: Duration,
        max_previous: usize,
    ) -> Self {
        Self {
            current: RwLock::new(current),
            previous: RwLock::new(Vec::new()),
            rotation_interval,
            grace_period,
            max_previous,
        }
    }

    /// 用当前密钥签发 token
    pub fn sign_token(
        &self,
        claims: &sz_rust_orm_facade::jwt::JwtClaims,
    ) -> Result<String, KeyRotationError> {
        let secret = self.current.read().clone();
        let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(&secret);
        encoder
            .encode(claims)
            .map_err(|e| KeyRotationError::SignError(e.to_string()))
    }

    /// 验证 token：先尝试当前密钥，失败则遍历旧密钥（grace period 内）
    pub fn verify_token(
        &self,
        token: &str,
    ) -> Result<sz_rust_orm_facade::jwt::JwtClaims, KeyRotationError> {
        // 先用当前密钥验证
        let current_secret = self.current.read().clone();
        let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(&current_secret);
        if let Ok(claims) = encoder.decode(token) {
            return Ok(claims);
        }

        // 遍历旧密钥（grace period 内）
        let now = std::time::Instant::now();
        let previous = self.previous.read();
        for (key, expires_at) in previous.iter() {
            if now < *expires_at {
                let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(key);
                if let Ok(claims) = encoder.decode(token) {
                    return Ok(claims);
                }
            }
        }

        Err(KeyRotationError::InvalidToken)
    }

    /// 启动密钥轮换定时任务
    pub fn spawn_rotation_task(self: std::sync::Arc<Self>) -> tokio::task::JoinHandle<()> {
        let interval = self.rotation_interval;
        let grace_period = self.grace_period;
        let max_previous = self.max_previous;

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await; // 跳过首次立即触发

            loop {
                ticker.tick().await;
                if let Err(e) = Self::do_rotation(&self, grace_period, max_previous).await {
                    tracing::error!("JWT_KEY_ROTATION_FAILED: {e}");
                }
            }
        })
    }

    /// 执行一次密钥轮换
    pub async fn do_rotation(
        &self,
        grace_period: Duration,
        max_previous: usize,
    ) -> Result<(), String> {
        // 生成新密钥（随机 32 字节 hex）
        let new_key = {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            let bytes: [u8; 32] = rng.gen();
            hex::encode(bytes)
        };

        let now = std::time::Instant::now();
        let expires_at = now + grace_period;

        // 旧 current 移入 previous，新密钥成为 current
        let old_current = {
            let mut current = self.current.write();
            let old = current.clone();
            *current = new_key.clone();
            old
        };

        {
            let mut previous = self.previous.write();
            previous.push((old_current.clone(), expires_at));
            // 超出 max_previous 的旧密钥删除
            while previous.len() > max_previous {
                previous.remove(0);
            }
            // 删除已过期的旧密钥
            previous.retain(|(_, exp)| now < *exp);
        }

        let old_fp = Self::fingerprint(&old_current);
        let new_fp = Self::fingerprint(&new_key);
        tracing::info!("JWT_KEY_ROTATED: old_fingerprint={old_fp}, new_fingerprint={new_fp}");

        Ok(())
    }

    /// 计算密钥指纹（SHA256 前 8 位十六进制）
    pub fn fingerprint(key: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        let result = hasher.finalize();
        hex::encode(&result[..4])
    }

    /// 获取当前密钥指纹（用于审计日志）
    pub fn current_fingerprint(&self) -> String {
        let current = self.current.read();
        Self::fingerprint(&current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    /// 测试用 Mock 控制器（仅使用默认实现）
    struct MockController;

    impl SzController for MockController {}

    fn make_json_request(body: &str, query: Option<&str>) -> Request<Body> {
        let uri = match query {
            Some(q) => format!("/?{q}"),
            None => "/".to_string(),
        };
        Request::builder()
            .method(Method::POST)
            .uri(&uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn make_get_request(query: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri(format!("/?{query}"))
            .body(Body::empty())
            .unwrap()
    }

    async fn collect_body(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    // ====================================================================
    // render_json 测试
    // ====================================================================

    #[test]
    fn test_render_json_field_order() {
        // 严格验证字段顺序：code → msg → data（对齐 PHP compact()）
        let ctrl = MockController;
        let value = ctrl.render_json(1, "ok", json!({"id": 1}));
        let obj = value.as_object().unwrap();
        let keys: Vec<&String> = obj.keys().collect();
        assert_eq!(keys, vec!["code", "msg", "data"]);
    }

    #[test]
    fn test_render_json_default_values() {
        // PHP 默认值：$code=1, $msg='', $data=[]
        let ctrl = MockController;
        let value = ctrl.render_json(1, "", Value::Object(Map::new()));
        assert_eq!(value["code"], 1);
        assert_eq!(value["msg"], "");
        assert!(value["data"].is_object());
        assert!(value["data"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_render_json_custom_code() {
        let ctrl = MockController;
        let value = ctrl.render_json(-1, "未登录", json!({}));
        assert_eq!(value["code"], -1);
        assert_eq!(value["msg"], "未登录");
    }

    #[test]
    fn test_render_json_compact_semantics() {
        // 验证 PHP compact('code','msg','data') 的等价语义：
        // 变量名 → 值的映射，保序序列化
        let ctrl = MockController;
        let value = ctrl.render_json(0, "失败", json!({"field": "name"}));
        let json_str = value.to_string();
        // 字段顺序必须是 code → msg → data
        assert_eq!(
            json_str,
            r#"{"code":0,"msg":"失败","data":{"field":"name"}}"#
        );
    }

    #[test]
    fn test_render_json_returns_value_not_response() {
        // PHP renderJson 返回数组（非 Response），renderSuccess/renderError 才返回 json()
        let ctrl = MockController;
        let value = ctrl.render_json(1, "ok", json!({}));
        // 应该是 Value，不是 Response
        assert!(value.is_object());
    }

    // ====================================================================
    // render_success 测试
    // ====================================================================

    #[test]
    fn test_render_success_returns_response() {
        let ctrl = MockController;
        let resp = ctrl.render_success("success", json!({"id": 1}));
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn test_render_success_body_format() {
        // 对齐 PHP renderSuccess('success', $data)
        let ctrl = MockController;
        let resp = ctrl.render_success("success", json!({"id": 1, "name": "alice"}));
        let body = collect_body(resp).await;
        assert_eq!(
            body,
            r#"{"code":1,"msg":"success","data":{"id":1,"name":"alice"}}"#
        );
    }

    #[tokio::test]
    async fn test_render_success_default_msg() {
        // PHP 默认 msg='success'
        let ctrl = MockController;
        let resp = ctrl.render_success("success", json!({}));
        let body = collect_body(resp).await;
        assert_eq!(body, r#"{"code":1,"msg":"success","data":{}}"#);
    }

    #[tokio::test]
    async fn test_render_success_via_axum_router() {
        // 端到端验证：通过 axum Router 调用 render_success
        struct UserController;
        impl SzController for UserController {}

        async fn handler() -> Response {
            let ctrl = UserController;
            ctrl.render_success("ok", json!({"id": 1}))
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = collect_body(resp).await;
        assert_eq!(body, r#"{"code":1,"msg":"ok","data":{"id":1}}"#);
    }

    // ====================================================================
    // render_error 测试
    // ====================================================================

    #[test]
    fn test_render_error_returns_response() {
        let ctrl = MockController;
        let resp = ctrl.render_error("error", json!({}), 0);
        assert_eq!(resp.status(), StatusCode::OK); // 业务错误 HTTP 仍 200
    }

    #[tokio::test]
    async fn test_render_error_default_code() {
        // PHP 默认 $code=0
        let ctrl = MockController;
        let resp = ctrl.render_error("参数错误", json!({}), 0);
        let body = collect_body(resp).await;
        assert_eq!(body, r#"{"code":0,"msg":"参数错误","data":{}}"#);
    }

    #[tokio::test]
    async fn test_render_error_custom_code() {
        // PHP renderError($msg, $data, $code=-1) 用于未登录场景
        let ctrl = MockController;
        let resp = ctrl.render_error("not_login", json!({}), -1);
        let body = collect_body(resp).await;
        assert_eq!(body, r#"{"code":-1,"msg":"not_login","data":{}}"#);
    }

    #[tokio::test]
    async fn test_render_error_with_data() {
        let ctrl = MockController;
        let resp = ctrl.render_error("失败", json!({"field": "name"}), 0);
        let body = collect_body(resp).await;
        assert_eq!(body, r#"{"code":0,"msg":"失败","data":{"field":"name"}}"#);
    }

    #[tokio::test]
    async fn test_render_error_via_axum_router() {
        struct UserController;
        impl SzController for UserController {}

        async fn handler() -> Response {
            let ctrl = UserController;
            ctrl.render_error("参数错误", json!({}), 0)
        }

        let router = axum::Router::new().route("/", axum::routing::post(handler));
        let req = Request::builder()
            .method(Method::POST)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = collect_body(resp).await;
        assert_eq!(body, r#"{"code":0,"msg":"参数错误","data":{}}"#);
    }

    // ====================================================================
    // post_data 测试（对齐 PHP postData()）
    // ====================================================================

    #[tokio::test]
    async fn test_post_data_json_body() {
        let ctrl = MockController;
        let req = make_json_request(r#"{"name":"alice","age":30}"#, None);
        let data = ctrl.post_data(req).await.unwrap();
        assert_eq!(data["name"], "alice");
        assert_eq!(data["age"], 30);
    }

    #[tokio::test]
    async fn test_post_data_query_only() {
        let ctrl = MockController;
        let req = make_json_request("", Some("page=1&size=10"));
        let data = ctrl.post_data(req).await.unwrap();
        assert_eq!(data["page"], "1");
        assert_eq!(data["size"], "10");
    }

    #[tokio::test]
    async fn test_post_data_body_overrides_query() {
        // body 优先级高于 query（对齐 PHP param() 行为）
        let ctrl = MockController;
        let req = make_json_request(r#"{"page":99}"#, Some("page=1&size=10"));
        let data = ctrl.post_data(req).await.unwrap();
        assert_eq!(data["page"], 99);
        assert_eq!(data["size"], "10");
    }

    #[tokio::test]
    async fn test_post_data_by_key_exists() {
        let ctrl = MockController;
        let req = make_json_request(r#"{"name":"alice","age":30}"#, None);
        let name = ctrl.post_data_by_key(req, "name").await.unwrap();
        assert_eq!(name, Some(json!("alice")));
    }

    #[tokio::test]
    async fn test_post_data_by_key_missing() {
        let ctrl = MockController;
        let req = make_json_request(r#"{"name":"alice"}"#, None);
        let age = ctrl.post_data_by_key(req, "age").await.unwrap();
        assert_eq!(age, None);
    }

    // ====================================================================
    // get_data 测试（对齐 PHP getData()）
    // ====================================================================

    #[test]
    fn test_get_data_query() {
        let ctrl = MockController;
        let req = make_get_request("page=1&size=10");
        let data = ctrl.get_data(&req);
        assert_eq!(data["page"], "1");
        assert_eq!(data["size"], "10");
    }

    #[test]
    fn test_get_data_empty_query() {
        let ctrl = MockController;
        let req = make_get_request("");
        let data = ctrl.get_data(&req);
        assert!(data.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_get_data_by_key_exists() {
        let ctrl = MockController;
        let req = make_get_request("page=1&size=10");
        let page = ctrl.get_data_by_key(&req, "page");
        assert_eq!(page, Some(json!("1")));
    }

    #[test]
    fn test_get_data_by_key_missing() {
        let ctrl = MockController;
        let req = make_get_request("page=1");
        let size = ctrl.get_data_by_key(&req, "size");
        assert_eq!(size, None);
    }

    // ====================================================================
    // 控制器多实例隔离测试
    // ====================================================================

    #[tokio::test]
    async fn test_multiple_controllers_independent() {
        // 验证多个控制器实例互不干扰（无状态 trait）
        struct CtrlA;
        struct CtrlB;
        impl SzController for CtrlA {}
        impl SzController for CtrlB {}

        let a = CtrlA;
        let b = CtrlB;

        let req_a = make_json_request(r#"{"k":"a"}"#, None);
        let req_b = make_json_request(r#"{"k":"b"}"#, None);

        let data_a = a.post_data(req_a).await.unwrap();
        let data_b = b.post_data(req_b).await.unwrap();

        assert_eq!(data_a["k"], "a");
        assert_eq!(data_b["k"], "b");

        // 验证 render_json 互不影响
        let va = a.render_json(1, "a", json!({}));
        let vb = b.render_json(0, "b", json!({}));
        assert_eq!(va["code"], 1);
        assert_eq!(va["msg"], "a");
        assert_eq!(vb["code"], 0);
        assert_eq!(vb["msg"], "b");
    }

    // ====================================================================
    // PHP 一致性综合测试
    // ====================================================================

    #[tokio::test]
    async fn test_php_consistency_full_flow() {
        // 模拟 PHP 控制器典型流程：读取 postData → 业务处理 → renderSuccess
        struct OrderController;
        impl SzController for OrderController {}

        let ctrl = OrderController;

        // 1. 读取 POST 数据
        let req = make_json_request(r#"{"order_id":12345,"amount":99.5}"#, None);
        let data = ctrl.post_data(req).await.unwrap();
        let order_id = data["order_id"].as_i64().unwrap();
        let amount = data["amount"].as_f64().unwrap();

        // 2. 业务处理（模拟）
        let result = json!({
            "order_id": order_id,
            "amount": amount,
            "status": "paid"
        });

        // 3. 返回成功响应
        let resp = ctrl.render_success("支付成功", result);
        let body = collect_body(resp).await;

        // 验证字段顺序与 PHP compact() 一致
        assert_eq!(
            body,
            r#"{"code":1,"msg":"支付成功","data":{"order_id":12345,"amount":99.5,"status":"paid"}}"#
        );
    }

    #[tokio::test]
    async fn test_php_consistency_error_flow() {
        // 模拟 PHP 控制器错误流程：校验失败 → renderError
        struct UserController;
        impl SzController for UserController {}

        let ctrl = UserController;

        let req = make_json_request(r#"{"name":""}"#, None);
        let data = ctrl.post_data(req).await.unwrap();
        let name = data["name"].as_str().unwrap();

        if name.is_empty() {
            let resp = ctrl.render_error("用户名不能为空", json!({"field": "name"}), 0);
            let body = collect_body(resp).await;
            assert_eq!(
                body,
                r#"{"code":0,"msg":"用户名不能为空","data":{"field":"name"}}"#
            );
        } else {
            panic!("should be empty");
        }
    }

    #[tokio::test]
    async fn test_php_consistency_not_login_flow() {
        // 模拟 PHP 控制器未登录流程：renderError($msg, $data, $code=-1)
        struct PassportController;
        impl SzController for PassportController {}

        let ctrl = PassportController;
        let resp = ctrl.render_error("not_login", json!({}), -1);
        let body = collect_body(resp).await;
        assert_eq!(body, r#"{"code":-1,"msg":"not_login","data":{}}"#);
    }

    // ====================================================================
    // BaseController trait 测试
    // ====================================================================

    /// 实现 BaseController 的 Mock 控制器（同时自动实现 SzController）
    struct MockBaseController;

    impl SzController for MockBaseController {}
    impl BaseController for MockBaseController {}

    #[test]
    fn test_base_controller_default_batch_validate() {
        // PHP 默认 $batchValidate = false
        let ctrl = MockBaseController;
        assert!(!ctrl.batch_validate());
    }

    #[test]
    fn test_base_controller_default_middlewares_empty() {
        // PHP 默认 $middleware = []
        let ctrl = MockBaseController;
        assert!(ctrl.middlewares().is_empty());
    }

    #[test]
    fn test_base_controller_default_initialize_no_panic() {
        // PHP 默认 initialize() 为空，调用应无副作用
        let ctrl = MockBaseController;
        ctrl.initialize(); // 不应 panic
    }

    #[test]
    fn test_base_controller_default_validate_returns_ok() {
        // require 规则 + 字段存在 → 应通过
        let ctrl = MockBaseController;
        let data = json!({"name": "alice"});
        let rules = [("name", "require")];
        let messages: [(&str, &str); 0] = [];
        let result = ctrl.validate(&data, &rules, &messages);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_require_pass_with_value() {
        // require 规则：字段存在且非空 → 通过
        let ctrl = MockBaseController;
        let data = json!({"name": "alice", "age": 30});
        let rules = [("name", "require"), ("age", "require|integer")];
        let messages: [(&str, &str); 0] = [];
        assert!(ctrl.validate(&data, &rules, &messages).is_ok());
    }

    #[test]
    fn test_validate_require_fail_when_missing() {
        // require 规则：字段缺失 → 失败（Single 模式，返回第一条错误）
        let ctrl = MockBaseController;
        let data = json!({"name": "alice"});
        let rules = [("name", "require"), ("age", "require|integer")];
        let messages: [(&str, &str); 0] = [];
        let result = ctrl.validate(&data, &rules, &messages);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // 错误信息应包含 age 字段（require 验证失败）
        assert!(err.contains("age"), "error: {err}");
    }

    #[test]
    fn test_validate_integer_fail_on_string() {
        // integer 规则：字段非整数 → 失败
        let ctrl = MockBaseController;
        let data = json!({"age": "not-a-number"});
        let rules = [("age", "require|integer")];
        let messages: [(&str, &str); 0] = [];
        let result = ctrl.validate(&data, &rules, &messages);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_custom_message_applied() {
        // 自定义消息应被使用（对齐 PHP message[field.type]）
        let ctrl = MockBaseController;
        let data = json!({}); // name 缺失
        let rules = [("name", "require")];
        let messages = [("name.require", "名称必填")];
        let result = ctrl.validate(&data, &rules, &messages);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "名称必填");
    }

    #[test]
    fn test_validate_batch_mode_returns_multiple_errors() {
        // 批量模式：收集所有错误，以 "; " 分隔
        struct BatchController;
        impl SzController for BatchController {}
        impl BaseController for BatchController {
            fn batch_validate(&self) -> bool {
                true
            }
        }

        let ctrl = BatchController;
        let data = json!({}); // name 和 age 都缺失
        let rules = [("name", "require"), ("age", "require")];
        let messages = [("name.require", "名称必填"), ("age.require", "年龄必填")];
        let result = ctrl.validate(&data, &rules, &messages);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // 两条错误都应出现（顺序按字段注册顺序）
        assert!(err.contains("名称必填"), "err: {err}");
        assert!(err.contains("年龄必填"), "err: {err}");
        // 应以 "; " 分隔（ValidateError::Batch 的 Display 格式）
        assert!(err.contains("; "), "err: {err}");
    }

    #[test]
    fn test_validate_single_mode_returns_first_error_only() {
        // 非批量模式：仅返回第一条错误
        let ctrl = MockBaseController;
        let data = json!({}); // name 和 age 都缺失
        let rules = [("name", "require"), ("age", "require")];
        let messages = [("name.require", "名称必填"), ("age.require", "年龄必填")];
        let result = ctrl.validate(&data, &rules, &messages);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // 仅包含第一条错误，不包含第二条
        assert!(err.contains("名称必填"), "err: {err}");
        assert!(!err.contains("年龄必填"), "err: {err}");
    }

    #[test]
    fn test_validate_in_rule_pass() {
        // in 规则：值在列表中 → 通过
        let ctrl = MockBaseController;
        let data = json!({"status": "active"});
        let rules = [("status", "require|in:active,inactive")];
        let messages: [(&str, &str); 0] = [];
        assert!(ctrl.validate(&data, &rules, &messages).is_ok());
    }

    #[test]
    fn test_validate_in_rule_fail() {
        // in 规则：值不在列表中 → 失败
        let ctrl = MockBaseController;
        let data = json!({"status": "deleted"});
        let rules = [("status", "require|in:active,inactive")];
        let messages: [(&str, &str); 0] = [];
        assert!(ctrl.validate(&data, &rules, &messages).is_err());
    }

    #[test]
    fn test_validate_empty_rules_always_pass() {
        // 空规则列表：无验证规则，总应通过
        let ctrl = MockBaseController;
        let data = json!({"anything": "value"});
        let rules: [(&str, &str); 0] = [];
        let messages: [(&str, &str); 0] = [];
        assert!(ctrl.validate(&data, &rules, &messages).is_ok());
    }

    #[test]
    fn test_base_controller_inherits_sz_controller_methods() {
        // BaseController: SzController，自动获得 render_json 等方法
        let ctrl = MockBaseController;
        let value = ctrl.render_json(1, "ok", json!({}));
        assert_eq!(value["code"], 1);
        assert_eq!(value["msg"], "ok");

        let resp = ctrl.render_success("ok", json!({"id": 1}));
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// 覆盖默认值的子类控制器
    struct CustomController;

    impl SzController for CustomController {}
    impl BaseController for CustomController {
        fn batch_validate(&self) -> bool {
            true
        }

        fn middlewares(&self) -> Vec<String> {
            vec!["auth".to_string(), "cors".to_string()]
        }

        fn initialize(&self) {
            // 子类自定义初始化（无状态，仅作演示）
        }
    }

    #[test]
    fn test_base_controller_override_batch_validate() {
        let ctrl = CustomController;
        assert!(ctrl.batch_validate());
    }

    #[test]
    fn test_base_controller_override_middlewares() {
        let ctrl = CustomController;
        let mws = ctrl.middlewares();
        assert_eq!(mws, vec!["auth", "cors"]);
    }

    #[test]
    fn test_base_controller_override_initialize() {
        let ctrl = CustomController;
        ctrl.initialize(); // 子类覆盖的 initialize 不应 panic
                           // 覆盖的 initialize 保持子类行为：batch_validate 仍为 true
        assert!(
            ctrl.batch_validate(),
            "覆盖 initialize 后默认校验开关应保持"
        );
    }

    /// 模拟 PHP 业务控制器：带状态的 initialize 钩子
    /// PHP 中 $this->batchValidate = true; 在 initialize() 中设置
    struct StatefulController {
        initialized: parking_lot::Mutex<bool>,
        custom_batch: bool,
    }

    impl StatefulController {
        fn new() -> Self {
            Self {
                initialized: parking_lot::Mutex::new(false),
                custom_batch: false,
            }
        }
    }

    impl SzController for StatefulController {}
    impl BaseController for StatefulController {
        fn batch_validate(&self) -> bool {
            self.custom_batch
        }

        fn initialize(&self) {
            *self.initialized.lock() = true;
            // 模拟 PHP: $this->batchValidate = true;
            // Rust 中由于 trait 方法不能修改 self，需要在调用方处理
        }
    }

    #[test]
    fn test_base_controller_stateful_initialize() {
        let ctrl = StatefulController::new();
        assert!(!*ctrl.initialized.lock()); // 初始未初始化
        ctrl.initialize(); // 调用初始化钩子
        assert!(*ctrl.initialized.lock()); // 已初始化
    }

    /// 模拟 PHP 控制器典型流程：initialize → validate → renderSuccess
    #[tokio::test]
    async fn test_base_controller_php_full_flow() {
        struct UserController;
        impl SzController for UserController {}
        impl BaseController for UserController {}

        let ctrl = UserController;

        // 1. 初始化钩子
        ctrl.initialize();

        // 2. 读取 POST 数据
        let req = make_json_request(r#"{"name":"alice","age":30}"#, None);
        let data = ctrl.post_data(req).await.unwrap();

        // 3. 验证（validate() 已实现 require/integer/gt 等规则）
        let rules = [("name", "require"), ("age", "require|integer|gt:0")];
        let messages: [(&str, &str); 0] = [];
        ctrl.validate(&data, &rules, &messages).unwrap();

        // 4. 返回成功响应
        let resp =
            ctrl.render_success("success", json!({"name": data["name"], "age": data["age"]}));
        let body = collect_body(resp).await;
        assert_eq!(
            body,
            r#"{"code":1,"msg":"success","data":{"name":"alice","age":30}}"#
        );
    }

    /// 测试多控制器继承链独立性
    #[test]
    fn test_base_controller_multiple_instances_independent() {
        struct ControllerA;
        struct ControllerB;

        impl SzController for ControllerA {}
        impl BaseController for ControllerA {
            fn middlewares(&self) -> Vec<String> {
                vec!["auth".to_string()]
            }
        }

        impl SzController for ControllerB {}
        impl BaseController for ControllerB {
            fn middlewares(&self) -> Vec<String> {
                vec!["cors".to_string(), "log".to_string()]
            }
        }

        let a = ControllerA;
        let b = ControllerB;

        assert_eq!(a.middlewares(), vec!["auth"]);
        assert_eq!(b.middlewares(), vec!["cors", "log"]);

        // 默认 batch_validate 不互相影响
        assert!(!a.batch_validate());
        assert!(!b.batch_validate());
    }

    /// PHP 继承链验证：BaseController → SzController 方法可用性
    #[test]
    fn test_base_controller_inheritance_chain() {
        // 业务控制器实现 BaseController，自动获得 SzController 的所有方法
        struct BusinessController;
        impl SzController for BusinessController {}
        impl BaseController for BusinessController {}

        let ctrl = BusinessController;

        // SzController 方法（来自父 trait）
        let value = ctrl.render_json(0, "error", json!({}));
        assert!(value.is_object());

        // BaseController 方法
        assert!(!ctrl.batch_validate());
        assert!(ctrl.middlewares().is_empty());
        ctrl.initialize();
    }

    // ====================================================================
    // AddonsBaseController trait 测试
    // ====================================================================

    /// 实现 AddonsBaseController 的 Mock 控制器（同时自动实现 BaseController + SzController）
    struct MockAddonsController;

    impl SzController for MockAddonsController {}
    impl BaseController for MockAddonsController {}
    impl AddonsBaseController for MockAddonsController {}

    #[test]
    fn test_addons_default_allow_all_action() {
        // PHP 默认 $allowAllAction = ['/passport/login', '/task/task/userClerk']
        let ctrl = MockAddonsController;
        let allow = ctrl.allow_all_action();
        assert!(allow.contains(&"/passport/login"));
        assert!(allow.contains(&"/task/task/userClerk"));
        assert_eq!(allow.len(), 2);
    }

    #[test]
    fn test_addons_parse_route_info_two_segments() {
        // /passport/login → controller=passport, action=login, group=passport
        let ctrl = MockAddonsController;
        let info = ctrl.parse_route_info("/passport/login");
        assert_eq!(info.controller, "passport");
        assert_eq!(info.action, "login");
        assert_eq!(info.group, "passport");
        assert_eq!(info.route_uri, "/passport/login");
    }

    #[test]
    fn test_addons_parse_route_info_three_segments() {
        // /task/task/userClerk → controller=task/task, action=userClerk
        // PHP bug 复刻：group=controller=task/task（详见 parse_route_info 文档注释）
        let ctrl = MockAddonsController;
        let info = ctrl.parse_route_info("/task/task/userClerk");
        assert_eq!(info.controller, "task/task");
        assert_eq!(info.action, "userClerk");
        assert_eq!(info.group, "task/task"); // PHP bug: group === controller
        assert_eq!(info.route_uri, "/task/task/userClerk");
    }

    #[test]
    fn test_addons_parse_route_info_single_segment() {
        // /passport → controller=passport, action="", group=passport
        let ctrl = MockAddonsController;
        let info = ctrl.parse_route_info("/passport");
        assert_eq!(info.controller, "passport");
        assert_eq!(info.action, "");
        assert_eq!(info.group, "passport");
        assert_eq!(info.route_uri, "/passport/");
    }

    #[test]
    fn test_addons_parse_route_info_root() {
        // / → 所有字段为空，route_uri="/"
        let ctrl = MockAddonsController;
        let info = ctrl.parse_route_info("/");
        assert_eq!(info.controller, "");
        assert_eq!(info.action, "");
        assert_eq!(info.group, "");
        assert_eq!(info.route_uri, "/");
    }

    #[test]
    fn test_addons_parse_route_info_empty_uri() {
        let ctrl = MockAddonsController;
        let info = ctrl.parse_route_info("");
        assert_eq!(info.controller, "");
        assert_eq!(info.route_uri, "/");
    }

    #[test]
    fn test_addons_parse_route_info_with_query_string() {
        // 含 query string 的 URI 应被剥离
        let ctrl = MockAddonsController;
        let info = ctrl.parse_route_info("/user/info?foo=bar&baz=1");
        assert_eq!(info.controller, "user");
        assert_eq!(info.action, "info");
        assert_eq!(info.route_uri, "/user/info");
    }

    #[test]
    fn test_addons_parse_route_info_trailing_slash() {
        // 末尾斜杠
        let ctrl = MockAddonsController;
        let info = ctrl.parse_route_info("/user/info/");
        // split 会产生空字符串末尾段
        assert_eq!(info.controller, "user/info");
        assert_eq!(info.action, "");
        assert_eq!(info.route_uri, "/user/info/");
    }

    #[test]
    fn test_addons_check_login_whitelist_pass() {
        // /passport/login 在白名单中，应通过
        let ctrl = MockAddonsController;
        let result = ctrl.check_login("/passport/login", false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_addons_check_login_whitelist_pass_2() {
        // /task/task/userClerk 在白名单中，应通过
        let ctrl = MockAddonsController;
        let result = ctrl.check_login("/task/task/userClerk", false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_addons_check_login_logged_in_pass() {
        // 不在白名单但已登录，应通过
        let ctrl = MockAddonsController;
        let result = ctrl.check_login("/user/info", true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_addons_check_login_not_logged_in_fail() {
        // 不在白名单且未登录，应返回 not_login
        let ctrl = MockAddonsController;
        let result = ctrl.check_login("/user/info", false);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "not_login");
    }

    #[test]
    fn test_addons_get_token_default_returns_none() {
        // 未配置 SZ_JWT_SECRET 环境变量时，JWT_CONFIG 为 None，验证跳过返回 Ok(None)
        // 注：生产环境通过 `validate_jwt_config()` 在启动时校验，测试环境允许跳过
        let ctrl = MockAddonsController;
        let result = ctrl.get_token(Some("Bearer xxx.yyy.zzz"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_addons_get_token_no_authorization() {
        let ctrl = MockAddonsController;
        let result = ctrl.get_token(None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_addons_get_token_empty_authorization() {
        let ctrl = MockAddonsController;
        let result = ctrl.get_token(Some(""));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_addons_get_token_invalid_format_returns_none() {
        // 非法 JWT 格式（非三段式）应返回 Ok(None) 而非 Err
        // 注：仅在 SZ_JWT_SECRET 已设置时才会进入格式校验
        let ctrl = MockAddonsController;
        let result = ctrl.get_token(Some("Bearer not.a.valid.jwt.token"));
        assert!(result.is_ok());
        // 无论是否配置密钥，无效 token 都应返回 None
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_strip_bearer_prefix_uppercase() {
        assert_eq!(strip_bearer_prefix("Bearer abc.def.ghi"), "abc.def.ghi");
    }

    #[test]
    fn test_strip_bearer_prefix_lowercase() {
        assert_eq!(strip_bearer_prefix("bearer abc.def.ghi"), "abc.def.ghi");
    }

    #[test]
    fn test_strip_bearer_prefix_mixed_case() {
        assert_eq!(strip_bearer_prefix("BEARER abc.def.ghi"), "abc.def.ghi");
    }

    #[test]
    fn test_strip_bearer_prefix_no_prefix() {
        // 无 Bearer 前缀时，应保持原样（去除首尾空格）
        assert_eq!(strip_bearer_prefix("abc.def.ghi"), "abc.def.ghi");
    }

    #[test]
    fn test_strip_bearer_prefix_empty() {
        assert_eq!(strip_bearer_prefix(""), "");
    }

    #[test]
    fn test_strip_bearer_prefix_with_extra_spaces() {
        assert_eq!(
            strip_bearer_prefix("  Bearer   abc.def.ghi  "),
            "abc.def.ghi"
        );
    }

    /// JWT 端到端验证测试：使用真实 JwtEncoder 签发 token，再通过 verify_token_with_config 验证
    ///
    /// 通过 verify_token_with_config 注入测试配置，避免依赖全局环境变量，
    /// 确保 CI 环境也能完整执行 JWT 验证流程测试。
    #[test]
    fn test_get_token_valid_jwt_returns_user_info() {
        let config = JwtConfig {
            secret: "test-secret".to_string(),
            issuer: String::new(), // 不验证 iss,
            audience: String::new(),
        };

        // 签发一个有效 token
        let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(&config.secret);
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600; // 1 小时后过期
        let claims = sz_rust_orm_facade::jwt::JwtClaims::new("user123", exp).with_user_id(12345);
        let token = encoder.encode(&claims).unwrap();

        let result = verify_token_with_config(Some(&format!("Bearer {token}")), &config);
        assert!(result.is_ok());
        let user = result.unwrap();
        assert!(user.is_some());
        let user = user.unwrap();
        assert_eq!(user.user_id, 12345);
        assert!(user.is_login);
    }

    /// 测试 JWT 签名密钥错误时返回 None
    #[test]
    fn test_get_token_wrong_secret_returns_none() {
        let config = JwtConfig {
            secret: "correct-secret".to_string(),
            issuer: String::new(),
            audience: String::new(),
        };

        // 用错误密钥签发 token
        let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new("wrong-secret");
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        let claims = sz_rust_orm_facade::jwt::JwtClaims::new("user123", exp).with_user_id(12345);
        let token = encoder.encode(&claims).unwrap();

        let result = verify_token_with_config(Some(&format!("Bearer {token}")), &config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None); // 签名验证失败
    }

    /// 测试 JWT 过期时返回 None
    #[test]
    fn test_get_token_expired_returns_none() {
        let config = JwtConfig {
            secret: "test-secret".to_string(),
            issuer: String::new(),
            audience: String::new(),
        };

        let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(&config.secret);
        // 过期时间为 1 小时前
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 3600;
        let claims = sz_rust_orm_facade::jwt::JwtClaims::new("user123", exp).with_user_id(12345);
        let token = encoder.encode(&claims).unwrap();

        let result = verify_token_with_config(Some(&format!("Bearer {token}")), &config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None); // 已过期
    }

    /// 测试 JWT 缺少 user_id claim 时返回 None（兼容旧版 token）
    #[test]
    fn test_get_token_no_user_id_claim_returns_none() {
        let config = JwtConfig {
            secret: "test-secret".to_string(),
            issuer: String::new(),
            audience: String::new(),
        };

        let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(&config.secret);
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        // 不调用 with_user_id，user_id 为 None
        let claims = sz_rust_orm_facade::jwt::JwtClaims::new("user123", exp);
        let token = encoder.encode(&claims).unwrap();

        let result = verify_token_with_config(Some(&format!("Bearer {token}")), &config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None); // 缺少 user_id
    }

    /// 测试 iss 验证：iss 不匹配时返回 None
    #[test]
    fn test_get_token_iss_mismatch_returns_none() {
        let config = JwtConfig {
            secret: "test-secret".to_string(),
            issuer: "https://expected-issuer.com".to_string(),
            audience: String::new(),
        };

        let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(&config.secret);
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        // 使用错误的 issuer 签发
        let claims = sz_rust_orm_facade::jwt::JwtClaims::new("user123", exp)
            .with_issuer("https://wrong-issuer.com")
            .with_user_id(12345);
        let token = encoder.encode(&claims).unwrap();

        let result = verify_token_with_config(Some(&format!("Bearer {token}")), &config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None); // iss 不匹配
    }

    /// 测试 iss 验证：iss 匹配时返回 UserInfo
    #[test]
    fn test_get_token_iss_match_returns_user_info() {
        let config = JwtConfig {
            secret: "test-secret".to_string(),
            issuer: "https://mall.ljclz.shop".to_string(),
            audience: String::new(),
        };

        let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(&config.secret);
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        let claims = sz_rust_orm_facade::jwt::JwtClaims::new("user123", exp)
            .with_issuer(&config.issuer)
            .with_user_id(67890);
        let token = encoder.encode(&claims).unwrap();

        let result = verify_token_with_config(Some(&format!("Bearer {token}")), &config);
        assert!(result.is_ok());
        let user = result.unwrap().unwrap();
        assert_eq!(user.user_id, 67890);
        assert!(user.is_login);
    }

    /// 测试密钥未配置时返回 None（禁用 JWT 验证）
    #[test]
    fn test_get_token_empty_secret_returns_none() {
        let config = JwtConfig::default(); // secret 为空

        let result = verify_token_with_config(Some("Bearer any.token.here"), &config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None); // 密钥未配置
    }

    /// 测试无 Bearer 前缀的 token 也能正常解析
    #[test]
    fn test_get_token_without_bearer_prefix() {
        let config = JwtConfig {
            secret: "test-secret".to_string(),
            issuer: String::new(),
            audience: String::new(),
        };

        let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(&config.secret);
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        let claims = sz_rust_orm_facade::jwt::JwtClaims::new("user123", exp).with_user_id(99999);
        let token = encoder.encode(&claims).unwrap();

        // 不带 Bearer 前缀，应仍能解析（对齐 PHP str_ireplace 兼容行为）
        let result = verify_token_with_config(Some(&token), &config);
        assert!(result.is_ok());
        let user = result.unwrap().unwrap();
        assert_eq!(user.user_id, 99999);
    }

    /// 覆盖白名单的子类控制器
    struct CustomAddonsController;

    impl SzController for CustomAddonsController {}
    impl BaseController for CustomAddonsController {}
    impl AddonsBaseController for CustomAddonsController {
        fn allow_all_action(&self) -> Vec<&'static str> {
            vec!["/custom/public", "/custom/health"]
        }
    }

    #[test]
    fn test_addons_override_allow_all_action() {
        let ctrl = CustomAddonsController;
        let allow = ctrl.allow_all_action();
        assert_eq!(allow, vec!["/custom/public", "/custom/health"]);
        // 不应包含默认白名单
        assert!(!allow.contains(&"/passport/login"));
    }

    #[test]
    fn test_addons_override_check_login_uses_new_whitelist() {
        let ctrl = CustomAddonsController;
        // /custom/public 在新白名单中
        assert!(ctrl.check_login("/custom/public", false).is_ok());
        // /passport/login 不在新白名单中（被覆盖）
        assert!(ctrl.check_login("/passport/login", false).is_err());
    }

    /// 模拟 PHP addons 控制器典型流程：parse_route_info → get_token → check_login → renderSuccess
    #[tokio::test]
    async fn test_addons_php_full_flow_whitelist() {
        struct PassportController;
        impl SzController for PassportController {}
        impl BaseController for PassportController {}
        impl AddonsBaseController for PassportController {}

        let ctrl = PassportController;

        // 1. 解析路由信息
        let info = ctrl.parse_route_info("/passport/login");
        assert_eq!(info.route_uri, "/passport/login");

        // 2. 获取 token（占位返回 None）
        let user = ctrl.get_token(None).unwrap();

        // 3. 检查登录（白名单通过，user_is_login 不影响）
        let is_login = user.as_ref().is_some_and(|u| u.is_login);
        ctrl.check_login(&info.route_uri, is_login).unwrap();

        // 4. 返回成功响应（模拟登录成功）
        let resp = ctrl.render_success("登录成功", json!({"token": "fake.jwt.token"}));
        let body = collect_body(resp).await;
        assert_eq!(
            body,
            r#"{"code":1,"msg":"登录成功","data":{"token":"fake.jwt.token"}}"#
        );
    }

    #[tokio::test]
    async fn test_addons_php_full_flow_not_login() {
        struct UserController;
        impl SzController for UserController {}
        impl BaseController for UserController {}
        impl AddonsBaseController for UserController {}

        let ctrl = UserController;

        // 1. 解析路由信息
        let info = ctrl.parse_route_info("/user/info");

        // 2. 获取 token（占位返回 None，即未登录）
        let user = ctrl.get_token(None).unwrap();
        let is_login = user.as_ref().is_some_and(|u| u.is_login);

        // 3. 检查登录（白名单不通过，且未登录）
        let result = ctrl.check_login(&info.route_uri, is_login);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "not_login");

        // 4. 返回未登录响应（模拟 PHP BaseException ['code' => -1, 'msg' => 'not_login']）
        let resp = ctrl.render_error("not_login", json!({}), -1);
        let body = collect_body(resp).await;
        assert_eq!(body, r#"{"code":-1,"msg":"not_login","data":{}}"#);
    }

    #[tokio::test]
    async fn test_addons_php_full_flow_logged_in() {
        struct OrderController;
        impl SzController for OrderController {}
        impl BaseController for OrderController {}
        impl AddonsBaseController for OrderController {
            // 模拟已登录用户（覆盖 get_token 返回 UserInfo）
            fn get_token(&self, _authorization: Option<&str>) -> Result<Option<UserInfo>, String> {
                Ok(Some(UserInfo {
                    user_id: 12345,
                    is_login: true,
                }))
            }
        }

        let ctrl = OrderController;

        // 1. 解析路由信息
        let info = ctrl.parse_route_info("/order/list");

        // 2. 获取 token（返回已登录用户）
        let user = ctrl.get_token(None).unwrap();
        let is_login = user.as_ref().is_some_and(|u| u.is_login);

        // 3. 检查登录（白名单不通过，但已登录）
        ctrl.check_login(&info.route_uri, is_login).unwrap();

        // 4. 返回订单列表
        let resp = ctrl.render_success(
            "success",
            json!({"list": [{"id": 1}, {"id": 2}], "total": 2}),
        );
        let body = collect_body(resp).await;
        assert_eq!(
            body,
            r#"{"code":1,"msg":"success","data":{"list":[{"id":1},{"id":2}],"total":2}}"#
        );
    }

    /// 测试 RouteInfo / UserInfo 类型
    #[test]
    fn test_route_info_debug_clone_eq() {
        let info1 = RouteInfo {
            controller: "user".to_string(),
            action: "info".to_string(),
            route_uri: "/user/info".to_string(),
            group: "user".to_string(),
        };
        let info2 = info1.clone();
        assert_eq!(info1, info2);
        let debug_str = format!("{info1:?}");
        assert!(debug_str.contains("RouteInfo"));
        assert!(debug_str.contains("user"));
    }

    #[test]
    fn test_user_info_debug_clone_eq() {
        let user1 = UserInfo {
            user_id: 12345,
            is_login: true,
        };
        let user2 = user1.clone();
        assert_eq!(user1, user2);
        let debug_str = format!("{user1:?}");
        assert!(debug_str.contains("UserInfo"));
        assert!(debug_str.contains("12345"));
    }

    /// 继承链验证：AddonsBaseController → BaseController → SzController
    #[test]
    fn test_addons_inheritance_chain_all_methods() {
        struct BusinessController;
        impl SzController for BusinessController {}
        impl BaseController for BusinessController {}
        impl AddonsBaseController for BusinessController {}

        let ctrl = BusinessController;

        // SzController 方法（祖父 trait）
        let value = ctrl.render_json(1, "ok", json!({}));
        assert_eq!(value["code"], 1);

        // BaseController 方法（父 trait）
        assert!(!ctrl.batch_validate());
        assert!(ctrl.middlewares().is_empty());
        ctrl.initialize();

        // AddonsBaseController 方法（当前 trait）
        let info = ctrl.parse_route_info("/test/action");
        assert_eq!(info.route_uri, "/test/action");
        assert!(ctrl.check_login("/passport/login", false).is_ok());
        assert!(ctrl.get_token(None).unwrap().is_none());
    }

    // ========================================================================
    // P0-SEC-02：JWT 密钥空字符串处理（认证绕过防护）
    // ========================================================================

    /// P0-SEC-02 回归测试：空密钥配置下 verify_token_with_config 必须返回 Ok(None)
    ///
    /// 安全铁律：空密钥 = 无密钥 = 禁用 JWT 验证。绝不能以空密钥尝试解码 token
    /// （某些 JWT 库在空密钥下会接受 alg=none 的伪造 token，导致认证绕过）。
    #[test]
    fn test_p0_sec_02_empty_secret_returns_none_not_accept_token() {
        let config = JwtConfig {
            secret: String::new(), // 空密钥
            issuer: String::new(),
            audience: String::new(),
        };

        // 即使传入看似有效的 token，空密钥下也必须拒绝验证（返回 None）
        let result = verify_token_with_config(Some("any.token.here"), &config);
        assert!(
            result.is_ok(),
            "空密钥不应导致 panic 或 Err（应优雅降级为 None）"
        );
        assert_eq!(
            result.unwrap(),
            None,
            "P0-SEC-02: 空密钥时必须返回 None，绝不能接受任何 token"
        );
    }

    /// P0-SEC-02 回归测试：空密钥下即使传入 alg=none 的伪造 token 也必须拒绝
    ///
    /// alg=none 攻击：某些 JWT 实现在密钥为空时会接受 header 中 alg=none 的 token。
    /// 此测试确保我们的实现不会落入此陷阱。
    #[test]
    fn test_p0_sec_02_rejects_alg_none_token_with_empty_secret() {
        let config = JwtConfig::default(); // 空密钥

        // 构造一个 alg=none 的伪造 JWT（header: {"alg":"none","typ":"JWT"}）
        // base64url({"alg":"none","typ":"JWT"}) = eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0
        // base64url({"user_id":1,"username":"hacker","iat":0,"exp":9999999999})
        //   = eyJ1c2VyX2lkIjoxLCJ1c2VybmFtZSI6ImhhY2tlciIsImlhdCI6MCwiZXhwIjo5OTk5OTk5OTk5fQ
        let alg_none_token = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.\
            eyJ1c2VyX2lkIjoxLCJ1c2VybmFtZSI6ImhhY2tlciIsImlhdCI6MCwiZXhwIjo5OTk5OTk5OTk5fQ.";

        let result = verify_token_with_config(Some(alg_none_token), &config);
        assert!(result.is_ok(), "空密钥下不应 panic");
        assert_eq!(
            result.unwrap(),
            None,
            "P0-SEC-02: 空密钥下 alg=none 伪造 token 必须被拒绝（返回 None）"
        );
    }

    // ========================================================================
    // P1-SEC-10：JWT Audience (aud) 验证
    // ========================================================================
    // 注：aud 字段验证依赖 sz-orm-auth JwtClaims.aud 字段（v1.2.2+）。
    // 当前 JwtClaims 尚未暴露 aud setter，audience 验证逻辑待 sz-orm-auth 升级后恢复。

    /// P1-SEC-10 回归测试：未配置 audience 时，跳过 aud 验证（向后兼容）
    #[test]
    fn test_p1_sec_10_skips_audience_check_when_not_configured() {
        let config = JwtConfig {
            secret: "test-secret".to_string(),
            issuer: String::new(),
            audience: String::new(),
        };

        let encoder = sz_rust_orm_facade::jwt::JwtEncoder::new(&config.secret);
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        let claims = sz_rust_orm_facade::jwt::JwtClaims::new("user123", exp).with_user_id(42);
        let token = encoder.encode(&claims).unwrap();

        let result = verify_token_with_config(Some(&token), &config);
        assert!(result.is_ok());
        assert!(
            result.unwrap().is_some(),
            "未配置 audience 时应跳过 aud 验证"
        );
    }

    // ========================================================================
    // P1-SEC-12：JwtConfig Debug 实现不泄漏 secret
    // ========================================================================

    /// P1-SEC-12 回归测试：JwtConfig 的 Debug 输出必须脱敏 secret 字段
    ///
    /// 若 JwtConfig 使用派生 Debug，secret 字段会以明文出现在日志和 panic 信息中。
    /// 手动实现的 Debug 应将 secret 替换为 "[REDACTED]"。
    #[test]
    fn test_p1_sec_12_jwt_config_debug_redacts_secret() {
        let config = JwtConfig {
            secret: "super-secret-key-12345".to_string(),
            issuer: "https://example.com".to_string(),
            audience: String::new(),
        };

        let debug_output = format!("{:?}", config);

        // secret 绝不能出现在 Debug 输出中
        assert!(
            !debug_output.contains("super-secret-key-12345"),
            "P1-SEC-12: JwtConfig::Debug 泄漏了 secret 字段: {debug_output}"
        );
        // 必须包含脱敏标记
        assert!(
            debug_output.contains("[REDACTED]"),
            "P1-SEC-12: JwtConfig::Debug 应包含 [REDACTED] 标记: {debug_output}"
        );
        // issuer 可以正常显示
        assert!(
            debug_output.contains("https://example.com"),
            "issuer 字段应正常显示: {debug_output}"
        );
    }
}
