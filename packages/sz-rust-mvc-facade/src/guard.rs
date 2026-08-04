//! Guard 守卫模块 — 鉴权决策（借鉴 NestJS Guard + Spring Security）
//!
//! sz-rust 自研模块，PHP 端无直接对应物。PHP 端鉴权分散在各应用的 Controller 基类
//! （`addons\BaseController::checkLogin` / `app\<app>\controller\Base::checkAuth`），
//! sz-rust 将鉴权决策抽象为独立的 Guard 层，与 Middleware 分离。
//!
//! ## Guard vs Middleware
//!
//! | 维度 | Middleware | Guard |
//! |------|-----------|-------|
//! | 关注点 | 横切关注点（日志/CORS/追踪/限流） | 鉴权决策（allow/deny） |
//! | 返回值 | Response（可修改请求/响应） | `Result<(), GuardError>`（二元决策） |
//! | 执行时机 | 请求全生命周期 | Auth 中间件之后、handler 之前 |
//! | 组合语义 | 链式（顺序执行） | AND 语义（全部通过） |
//!
//! ## PHP 端鉴权调研
//!
//! PHP 端 9 个应用各自独立鉴权：
//! - `szoa` / `szoapc` / `szweb`：JWT + RBAC（`users → user_role → role → role_access → access`）
//! - `szadmin`：Basic Auth
//! - 其他应用：Cache token / Session
//!
//! PHP szoa RBAC 模型：
//! - `is_super=1` 绕过所有 RBAC 检查
//! - 错误码：`-1` not_login / `0` 无权限 / `-3` 用户已禁用
//! - 权限格式：`controller/action`（如 `user/list`、`user/save`）
//!
//! ## 执行顺序
//!
//! Guard 在 [`sz_rust_middleware_facade::order::DEFAULT_ORDER`] 的 Auth 中间件之后执行：
//!
//! ```text
//! Trace → Cors → Log → RateLimit → Auth → [Guard] → Handler
//! ```
//!
//! Auth 中间件负责 JWT 校验并注入 [`AuthenticatedUser`] 到 request extensions，
//! Guard 基于 `AuthenticatedUser` 和 [`UserContext`] 进行鉴权决策。
//!
//! ## 用法
//!
//! ### 单个 Guard
//!
//! ```ignore
//! use sz_rust_core::guard::{AuthGuard, Guard, guard_middleware};
//! use std::sync::Arc;
//! use axum::Router;
//! use axum::middleware::from_fn_with_state;
//!
//! let app: Router = Router::new()
//!     .route("/profile", axum::routing::get(handler))
//!     .layer(from_fn_with_state(
//!         Arc::new(AuthGuard) as Arc<dyn Guard>,
//!         guard_middleware,
//!     ));
//! ```
//!
//! ### Guard 链（AND 语义）
//!
//! ```ignore
//! use sz_rust_core::guard::{AuthGuard, AdminGuard, GuardChain, Guard, guard_middleware};
//! use std::sync::Arc;
//! use axum::Router;
//! use axum::middleware::from_fn_with_state;
//!
//! let chain = GuardChain::new()
//!     .with_guard(Arc::new(AuthGuard))
//!     .with_guard(Arc::new(AdminGuard));
//!
//! let app: Router = Router::new()
//!     .route("/admin", axum::routing::get(handler))
//!     .layer(from_fn_with_state(
//!         Arc::new(chain) as Arc<dyn Guard>,
//!         guard_middleware,
//!     ));
//! ```
//!
//! ### 权限 Guard
//!
//! ```ignore
//! use sz_rust_core::guard::{AuthGuard, PermissionGuard, GuardChain, Guard, guard_middleware};
//! use std::sync::Arc;
//!
//! let chain = GuardChain::new()
//!     .with_guard(Arc::new(AuthGuard))
//!     .with_guard(Arc::new(PermissionGuard::new("user/list")));
//! ```

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use sz_rust_http_facade::error::{BaseException, ErrorCode};
use sz_rust_middleware_facade::auth::{base_exception_to_response, AuthenticatedUser};

// ============================================================================
// GuardError — Guard 错误类型
// ============================================================================

/// Guard 错误类型 — 对齐 PHP 错误码
///
/// Guard 拒绝请求时返回此错误，由 [`IntoResponse`] 实现转换为 HTTP 响应。
///
/// ## 错误码对齐
///
/// | GuardError | ErrorCode | PHP 对应 | HTTP 状态码 |
/// |-----------|-----------|---------|------------|
/// | `not_login` | `NotLogin(-1)` | `not_login` | 401 |
/// | `forbidden` | `Forbidden(403)` | 无权限 | 403 |
/// | `user_disabled` | `UserDisabled(-3)` | `您已离职` | 403 |
///
/// ## 响应格式
///
/// 对齐 PHP `renderJson`：
/// ```json
/// { "code": <code>, "msg": "<msg>", "data": {} }
/// ```
#[derive(Debug, Clone)]
pub struct GuardError {
    /// 错误码（对齐 PHP BaseException 的 code 字段）
    pub code: ErrorCode,
    /// 错误消息（对齐 PHP BaseException 的 msg 字段）
    pub msg: String,
}

impl GuardError {
    /// 创建 GuardError
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
        }
    }

    /// 未登录快捷构造（对齐 PHP `code=-1, msg='not_login'`）
    pub fn not_login(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotLogin, msg)
    }

    /// 无权限快捷构造（HTTP 403）
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, msg)
    }

    /// 用户已禁用快捷构造（对齐 PHP `code=-3, msg='您已离职'`）
    pub fn user_disabled(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::UserDisabled, msg)
    }
}

impl IntoResponse for GuardError {
    /// 转换为 HTTP 响应
    ///
    /// 复用 [`base_exception_to_response`]，确保响应格式与 PHP `renderJson` 对齐：
    /// - HTTP 状态码由 [`ErrorCode::http_status`] 决定
    /// - 响应体：`{"code": <code>, "msg": "<msg>", "data": {}}`
    fn into_response(self) -> Response {
        let exc = BaseException::new(self.code, self.msg);
        base_exception_to_response(exc)
    }
}

impl From<GuardError> for BaseException {
    fn from(err: GuardError) -> Self {
        BaseException::new(err.code, err.msg)
    }
}

impl std::fmt::Display for GuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code.as_i32(), self.msg)
    }
}

impl std::error::Error for GuardError {}

// ============================================================================
// UserContext — 用户上下文（扩展 AuthenticatedUser，提供权限信息）
// ============================================================================

/// 用户上下文 — 扩展 [`AuthenticatedUser`]，提供权限信息
///
/// 由业务层在登录时构建并注入到 request extensions（在 Auth 中间件之后、Guard 之前）。
/// Guard 通过 `UserContext` 进行权限判断。
///
/// ## PHP 对齐
///
/// 对齐 PHP szoa RBAC 模型：
/// - `is_super=true` 绕过所有 RBAC（对齐 PHP `is_super=1`）
/// - `roles` 对齐 PHP `role` 表（通过 `user_role` 关联）
/// - `permissions` 对齐 PHP `access` 表（格式：`controller/action`，如 `user/list`）
///
/// ## 注入时机
///
/// ```text
/// Auth 中间件（注入 AuthenticatedUser）
///     ↓
/// 业务中间件（查 DB 获取 roles/permissions，注入 UserContext）
///     ↓
/// Guard（基于 UserContext 进行鉴权决策）
///     ↓
/// Handler
/// ```
///
/// 如果业务代码未注入 `UserContext`，[`AdminGuard`] / [`PermissionGuard`] / [`RoleGuard`]
/// 将返回 `Forbidden`（无权限访问）。
pub struct UserContext {
    /// 用户 ID（对齐 `AuthenticatedUser::user_id`）
    pub user_id: i64,
    /// 是否超级管理员（对齐 PHP `is_super=1`，绕过所有 RBAC）
    pub is_super: bool,
    /// 角色列表（对齐 PHP `role` 表）
    pub roles: Vec<String>,
    /// 权限列表（对齐 PHP `access` 表，格式：`controller/action`）
    pub permissions: Vec<String>,
}

impl UserContext {
    /// 创建 UserContext（默认非超级管理员，无角色，无权限）
    pub fn new(user_id: i64) -> Self {
        Self {
            user_id,
            is_super: false,
            roles: Vec::new(),
            permissions: Vec::new(),
        }
    }

    /// 设置是否超级管理员
    pub fn with_super(mut self, is_super: bool) -> Self {
        self.is_super = is_super;
        self
    }

    /// 设置角色列表
    pub fn with_roles(mut self, roles: Vec<String>) -> Self {
        self.roles = roles;
        self
    }

    /// 设置权限列表
    pub fn with_permissions(mut self, permissions: Vec<String>) -> Self {
        self.permissions = permissions;
        self
    }

    /// 检查是否具有指定角色
    ///
    /// 对齐 PHP `User::hasRole()` 角色 检查
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// 检查是否具有指定权限
    ///
    /// 对齐 PHP `AuthService::check()` 权限检查：
    /// - 精确匹配：`user/list` == `user/list` → true
    /// - 通配符匹配：`user/*` 匹配 `user/list` → true（对齐 PHP 通配符权限）
    ///
    /// 注意：`is_super=true` 的绕过逻辑由 Guard 负责，本方法仅检查权限列表。
    pub fn has_permission(&self, permission: &str) -> bool {
        // 1. 精确匹配
        if self.permissions.iter().any(|p| p == permission) {
            return true;
        }
        // 2. 通配符匹配（对齐 PHP `user/*` 匹配 `user/list`）
        for perm in &self.permissions {
            if perm.ends_with("/*") {
                let prefix = &perm[..perm.len() - 1]; // 去掉 `*`，保留 `user/`
                if permission.starts_with(prefix) {
                    return true;
                }
            }
        }
        false
    }
}

impl std::fmt::Debug for UserContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserContext")
            .field("user_id", &self.user_id)
            .field("is_super", &self.is_super)
            .field("roles", &self.roles)
            .field("permissions", &self.permissions)
            .finish()
    }
}

impl Clone for UserContext {
    fn clone(&self) -> Self {
        Self {
            user_id: self.user_id,
            is_super: self.is_super,
            roles: self.roles.clone(),
            permissions: self.permissions.clone(),
        }
    }
}

impl Default for UserContext {
    fn default() -> Self {
        Self::new(0)
    }
}

impl From<AuthenticatedUser> for UserContext {
    /// 从 [`AuthenticatedUser`] 创建 [`UserContext`]
    ///
    /// 默认非超级管理员，无角色，无权限（业务层需后续调用 `with_super` / `with_roles`
    /// / `with_permissions` 补充权限信息）。
    fn from(user: AuthenticatedUser) -> Self {
        Self::new(user.user_id)
    }
}

// ============================================================================
// Guard trait — NestJS 风格的守卫接口
// ============================================================================

/// Guard trait — NestJS 风格的守卫接口
///
/// 守卫在 Auth 中间件之后执行，用于鉴权决策（allow/deny）。
///
/// ## 同步 trait 设计
///
/// `check` 为同步方法，因为：
/// - 决策基于 request extensions（已在 Auth 中间件中预加载）
/// - 不需要 I/O（DB 查询由业务层中间件完成，结果存入 [`UserContext`]）
/// - 对齐 `sz-orm-auth` 的 `Authorizer` trait 设计
///
/// 如需异步 DB 查询，应在 Guard 之前的中间件中预加载权限信息到 `UserContext`。
///
/// ## PHP 对齐
///
/// PHP 端无直接对应物。PHP 鉴权分散在各应用的 Controller 基类：
/// - `addons\BaseController::checkLogin`：登录校验（对齐 [`AuthGuard`]）
/// - `app\szoa\controller\Base::checkAuth`：权限校验（对齐 [`PermissionGuard`]）
///
/// sz-rust 将鉴权抽象为独立 Guard 层，便于复用和组合。
pub trait Guard: Send + Sync {
    /// 检查请求是否通过守卫
    ///
    /// 返回 `Ok(())` 表示通过，`Err(GuardError)` 表示拒绝。
    ///
    /// ## 实现约定
    ///
    /// - 应先检查 [`AuthenticatedUser`] 是否存在（登录校验）
    /// - 再检查 [`UserContext`] 中的权限信息
    /// - `is_super=true` 应绕过所有权限检查（对齐 PHP `is_super=1`）
    fn check(&self, req: &Request) -> Result<(), GuardError>;
}

// ============================================================================
// AuthGuard — 登录校验
// ============================================================================

/// AuthGuard — 检查用户已登录（[`AuthenticatedUser`] 存在）
///
/// 对齐 PHP `addons\BaseController::checkLogin`：
/// ```php
/// private function checkLogin(): void {
///     if (!empty($this->user) && $this->user['is_login'] == 1) {
///         return;
///     }
///     throw new BaseException(['code' => -1, 'msg' => 'not_login']);
/// }
/// ```
///
/// Rust 端 `AuthenticatedUser` 存在于 extensions 即表示已登录（`is_login=1`）。
#[derive(Debug, Default)]
pub struct AuthGuard;

impl AuthGuard {
    /// 创建 AuthGuard
    pub fn new() -> Self {
        Self
    }
}

impl Guard for AuthGuard {
    fn check(&self, req: &Request) -> Result<(), GuardError> {
        if req.extensions().get::<AuthenticatedUser>().is_some() {
            Ok(())
        } else {
            Err(GuardError::not_login("not_login"))
        }
    }
}

// ============================================================================
// AdminGuard — 超级管理员校验
// ============================================================================

/// AdminGuard — 检查用户是超级管理员（`is_super=true`）
///
/// 对齐 PHP szoa `is_super=1` 绕过所有 RBAC：
/// ```php
/// if ($user['is_super'] == 1) {
///     return true; // 绕过所有权限检查
/// }
/// ```
///
/// 需要 [`UserContext`] extension（由业务层注入）。
/// 如果 `UserContext` 不存在，返回 `Forbidden`（无权限访问）。
#[derive(Debug, Default)]
pub struct AdminGuard;

impl AdminGuard {
    /// 创建 AdminGuard
    pub fn new() -> Self {
        Self
    }
}

impl Guard for AdminGuard {
    fn check(&self, req: &Request) -> Result<(), GuardError> {
        // 1. 先检查已登录（对齐 PHP checkLogin）
        let _user = req
            .extensions()
            .get::<AuthenticatedUser>()
            .ok_or_else(|| GuardError::not_login("not_login"))?;

        // 2. 检查 UserContext 中的 is_super
        let user_ctx = req
            .extensions()
            .get::<UserContext>()
            .ok_or_else(|| GuardError::forbidden("无权限访问"))?;

        if user_ctx.is_super {
            Ok(())
        } else {
            Err(GuardError::forbidden("无权限访问"))
        }
    }
}

// ============================================================================
// PermissionGuard — 权限校验
// ============================================================================

/// PermissionGuard — 检查用户具有特定权限
///
/// 对齐 PHP szoa RBAC `AuthService::check()`：
/// ```php
/// public function check($action): bool {
///     if ($user['is_super'] == 1) {
///         return true; // 超级管理员绕过
///     }
///     return in_array($action, $user['permissions']);
/// }
/// ```
///
/// ## 权限格式
///
/// 对齐 PHP `access` 表，格式：`controller/action`（如 `user/list`、`user/save`）。
///
/// 支持通配符：`user/*` 匹配 `user/list`、`user/save` 等。
///
/// ## is_super 绕过
///
/// `is_super=true` 绕过所有权限检查（对齐 PHP `is_super=1`）。
#[derive(Debug)]
pub struct PermissionGuard {
    /// 所需权限（格式：`controller/action`）
    pub permission: String,
}

impl PermissionGuard {
    /// 创建 PermissionGuard
    pub fn new(permission: impl Into<String>) -> Self {
        Self {
            permission: permission.into(),
        }
    }
}

impl Guard for PermissionGuard {
    fn check(&self, req: &Request) -> Result<(), GuardError> {
        // 1. 先检查已登录
        let _user = req
            .extensions()
            .get::<AuthenticatedUser>()
            .ok_or_else(|| GuardError::not_login("not_login"))?;

        // 2. 检查 UserContext
        let user_ctx = req
            .extensions()
            .get::<UserContext>()
            .ok_or_else(|| GuardError::forbidden("无权限访问"))?;

        // 3. is_super 绕过所有 RBAC（对齐 PHP is_super=1）
        if user_ctx.is_super {
            return Ok(());
        }

        // 4. 检查权限
        if user_ctx.has_permission(&self.permission) {
            Ok(())
        } else {
            Err(GuardError::forbidden("无权限访问"))
        }
    }
}

// ============================================================================
// RoleGuard — 角色校验
// ============================================================================

/// RoleGuard — 检查用户具有指定角色
///
/// 对齐 PHP szoa `user_role` 关联表的角色检查。
///
/// ## is_super 绕过
///
/// `is_super=true` 绕过所有角色检查（对齐 PHP `is_super=1`）。
#[derive(Debug)]
pub struct RoleGuard {
    /// 所需角色名
    pub role: String,
}

impl RoleGuard {
    /// 创建 RoleGuard
    pub fn new(role: impl Into<String>) -> Self {
        Self { role: role.into() }
    }
}

impl Guard for RoleGuard {
    fn check(&self, req: &Request) -> Result<(), GuardError> {
        // 1. 先检查已登录
        let _user = req
            .extensions()
            .get::<AuthenticatedUser>()
            .ok_or_else(|| GuardError::not_login("not_login"))?;

        // 2. 检查 UserContext
        let user_ctx = req
            .extensions()
            .get::<UserContext>()
            .ok_or_else(|| GuardError::forbidden("无权限访问"))?;

        // 3. is_super 绕过（对齐 PHP is_super=1）
        if user_ctx.is_super {
            return Ok(());
        }

        // 4. 检查角色
        if user_ctx.has_role(&self.role) {
            Ok(())
        } else {
            Err(GuardError::forbidden("无权限访问"))
        }
    }
}

// ============================================================================
// GuardChain — 守卫链（AND 语义组合）
// ============================================================================

/// GuardChain — 守卫链（AND 语义组合）
///
/// 所有 Guard 必须通过，任一失败则整个链失败。
/// 对齐 NestJS `UseGuards(...)` 多个 Guard 的 AND 语义。
///
/// ## 执行顺序
///
/// 按添加顺序执行（先添加先执行）。建议顺序：
/// 1. [`AuthGuard`]（先检查登录）
/// 2. [`AdminGuard`] / [`PermissionGuard`] / [`RoleGuard`]（再检查权限）
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::guard::{AuthGuard, AdminGuard, GuardChain};
/// use std::sync::Arc;
///
/// let chain = GuardChain::new()
///     .with_guard(Arc::new(AuthGuard))
///     .with_guard(Arc::new(AdminGuard));
/// ```
pub struct GuardChain {
    /// 守卫列表（按添加顺序执行）
    pub guards: Vec<Arc<dyn Guard>>,
}

impl GuardChain {
    /// 创建空的 GuardChain
    pub fn new() -> Self {
        Self { guards: Vec::new() }
    }

    /// 添加 Guard（builder 风格）
    pub fn with_guard(mut self, guard: Arc<dyn Guard>) -> Self {
        self.guards.push(guard);
        self
    }

    /// 从 Guard 列表创建 GuardChain
    pub fn from_guards(guards: Vec<Arc<dyn Guard>>) -> Self {
        Self { guards }
    }
}

impl Default for GuardChain {
    fn default() -> Self {
        Self::new()
    }
}

impl Guard for GuardChain {
    fn check(&self, req: &Request) -> Result<(), GuardError> {
        // AND 语义：所有 guard 必须通过
        // 顺序：按添加顺序执行（先 AuthGuard，再权限 Guard）
        for guard in &self.guards {
            guard.check(req)?;
        }
        Ok(())
    }
}

// ============================================================================
// guard_middleware — axum 中间件集成
// ============================================================================

/// Guard 中间件 — 将 Guard 集成到 axum 中间件链
///
/// 在 Auth 中间件之后执行，检查 Guard。Guard 通过则调用下游 handler，
/// Guard 拒绝则返回错误响应（对齐 PHP `renderJson` 格式）。
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::guard::{AuthGuard, Guard, guard_middleware};
/// use std::sync::Arc;
/// use axum::Router;
/// use axum::middleware::from_fn_with_state;
///
/// let app: Router = Router::new()
///     .route("/profile", axum::routing::get(handler))
///     .layer(from_fn_with_state(
///         Arc::new(AuthGuard) as Arc<dyn Guard>,
///         guard_middleware,
///     ));
/// ```
pub async fn guard_middleware(
    axum::extract::State(guard): axum::extract::State<Arc<dyn Guard>>,
    req: Request,
    next: Next,
) -> Response {
    match guard.check(&req) {
        Ok(()) => next.run(req).await,
        Err(err) => err.into_response(),
    }
}

/// 执行守卫检查（无中间件，纯函数）
///
/// 按 `guards` 顺序执行 AND 语义检查，任一失败则返回错误。
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::guard::{AuthGuard, Guard, check_guards};
/// use std::sync::Arc;
///
/// let guards: Vec<Arc<dyn Guard>> = vec![Arc::new(AuthGuard)];
/// let result = check_guards(&req, &guards);
/// ```
pub fn check_guards(req: &Request, guards: &[Arc<dyn Guard>]) -> Result<(), GuardError> {
    for guard in guards {
        guard.check(req)?;
    }
    Ok(())
}

// ============================================================================
// 单元测试
// ============================================================================

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

    /// 构建无 extensions 的 Request
    fn make_request() -> Request {
        Request::builder()
            .method("GET")
            .uri("/test")
            .body(Body::empty())
            .unwrap()
    }

    /// 构建带 AuthenticatedUser 的 Request
    fn make_request_with_user(user_id: i64) -> Request {
        let mut req = make_request();
        req.extensions_mut().insert(AuthenticatedUser { user_id });
        req
    }

    /// 构建带 AuthenticatedUser + UserContext 的 Request
    fn make_request_with_context(user_ctx: UserContext) -> Request {
        let mut req = make_request_with_user(user_ctx.user_id);
        req.extensions_mut().insert(user_ctx);
        req
    }

    /// 读取响应体为字符串
    async fn read_body(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// 构建测试 Router（带 Guard）
    fn build_app(guard: Arc<dyn Guard>) -> Router {
        Router::new()
            .route(
                "/protected",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                guard,
                guard_middleware,
            ))
    }

    // ====================================================================
    // GuardError 单元测试
    // ====================================================================

    #[test]
    fn test_guard_error_new() {
        let err = GuardError::new(ErrorCode::Forbidden, "无权限");
        assert_eq!(err.code, ErrorCode::Forbidden);
        assert_eq!(err.msg, "无权限");
    }

    #[test]
    fn test_guard_error_not_login() {
        let err = GuardError::not_login("not_login");
        assert_eq!(err.code, ErrorCode::NotLogin);
        assert_eq!(err.msg, "not_login");
        // 对齐 PHP code=-1
        assert_eq!(err.code.as_i32(), -1);
    }

    #[test]
    fn test_guard_error_forbidden() {
        let err = GuardError::forbidden("无权限访问");
        assert_eq!(err.code, ErrorCode::Forbidden);
        assert_eq!(err.msg, "无权限访问");
        assert_eq!(err.code.as_i32(), 403);
    }

    #[test]
    fn test_guard_error_user_disabled() {
        let err = GuardError::user_disabled("您已离职");
        assert_eq!(err.code, ErrorCode::UserDisabled);
        assert_eq!(err.msg, "您已离职");
        // 对齐 PHP code=-3
        assert_eq!(err.code.as_i32(), -3);
    }

    #[test]
    fn test_guard_error_display() {
        let err = GuardError::not_login("not_login");
        assert_eq!(format!("{}", err), "[-1] not_login");
    }

    #[test]
    fn test_guard_error_clone() {
        let err = GuardError::forbidden("无权限");
        let cloned = err.clone();
        assert_eq!(err.code, cloned.code);
        assert_eq!(err.msg, cloned.msg);
    }

    #[test]
    fn test_guard_error_into_response_not_login() {
        let err = GuardError::not_login("not_login");
        let resp = err.into_response();
        // NotLogin → HTTP 401
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_guard_error_into_response_forbidden() {
        let err = GuardError::forbidden("无权限访问");
        let resp = err.into_response();
        // Forbidden → HTTP 403
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_guard_error_into_response_user_disabled() {
        let err = GuardError::user_disabled("您已离职");
        let resp = err.into_response();
        // UserDisabled → HTTP 403
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_guard_error_into_base_exception() {
        let err = GuardError::not_login("not_login");
        let exc: BaseException = err.into();
        assert_eq!(exc.code, -1);
        assert_eq!(exc.msg, "not_login");
    }

    #[tokio::test]
    async fn test_guard_error_response_body_format() {
        // 对齐 PHP renderJson: {"code": <code>, "msg": "<msg>", "data": {}}
        let err = GuardError::not_login("not_login");
        let resp = err.into_response();
        let body = read_body(resp).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["code"], -1);
        assert_eq!(json["msg"], "not_login");
        assert_eq!(json["data"], serde_json::json!({}));
    }

    // ====================================================================
    // UserContext 单元测试
    // ====================================================================

    #[test]
    fn test_user_context_new() {
        let ctx = UserContext::new(100);
        assert_eq!(ctx.user_id, 100);
        assert!(!ctx.is_super);
        assert!(ctx.roles.is_empty());
        assert!(ctx.permissions.is_empty());
    }

    #[test]
    fn test_user_context_with_super() {
        let ctx = UserContext::new(1).with_super(true);
        assert!(ctx.is_super);
    }

    #[test]
    fn test_user_context_with_roles() {
        let ctx = UserContext::new(1).with_roles(vec!["admin".to_string(), "editor".to_string()]);
        assert_eq!(ctx.roles, vec!["admin", "editor"]);
    }

    #[test]
    fn test_user_context_with_permissions() {
        let ctx = UserContext::new(1)
            .with_permissions(vec!["user/list".to_string(), "user/save".to_string()]);
        assert_eq!(ctx.permissions, vec!["user/list", "user/save"]);
    }

    #[test]
    fn test_user_context_has_role() {
        let ctx = UserContext::new(1).with_roles(vec!["admin".to_string(), "editor".to_string()]);
        assert!(ctx.has_role("admin"));
        assert!(ctx.has_role("editor"));
        assert!(!ctx.has_role("guest"));
    }

    #[test]
    fn test_user_context_has_role_empty() {
        let ctx = UserContext::new(1);
        assert!(!ctx.has_role("admin"));
    }

    #[test]
    fn test_user_context_has_permission_exact() {
        let ctx = UserContext::new(1).with_permissions(vec!["user/list".to_string()]);
        assert!(ctx.has_permission("user/list"));
        assert!(!ctx.has_permission("user/save"));
    }

    #[test]
    fn test_user_context_has_permission_wildcard() {
        // 对齐 PHP 通配符权限：user/* 匹配 user/list, user/save 等
        let ctx = UserContext::new(1).with_permissions(vec!["user/*".to_string()]);
        assert!(ctx.has_permission("user/list"));
        assert!(ctx.has_permission("user/save"));
        assert!(ctx.has_permission("user/delete"));
        // 不匹配其他 controller
        assert!(!ctx.has_permission("order/list"));
    }

    #[test]
    fn test_user_context_has_permission_empty() {
        let ctx = UserContext::new(1);
        assert!(!ctx.has_permission("user/list"));
    }

    #[test]
    fn test_user_context_has_permission_multiple() {
        let ctx = UserContext::new(1).with_permissions(vec![
            "user/list".to_string(),
            "order/*".to_string(),
            "system/config".to_string(),
        ]);
        // 精确匹配
        assert!(ctx.has_permission("user/list"));
        assert!(ctx.has_permission("system/config"));
        // 通配符匹配
        assert!(ctx.has_permission("order/list"));
        assert!(ctx.has_permission("order/save"));
        // 不匹配
        assert!(!ctx.has_permission("user/save"));
        assert!(!ctx.has_permission("product/list"));
    }

    #[test]
    fn test_user_context_from_authenticated_user() {
        let user = AuthenticatedUser { user_id: 42 };
        let ctx = UserContext::from(user);
        assert_eq!(ctx.user_id, 42);
        assert!(!ctx.is_super);
        assert!(ctx.roles.is_empty());
        assert!(ctx.permissions.is_empty());
    }

    #[test]
    fn test_user_context_default() {
        let ctx = UserContext::default();
        assert_eq!(ctx.user_id, 0);
        assert!(!ctx.is_super);
    }

    #[test]
    fn test_user_context_clone() {
        let ctx = UserContext::new(1)
            .with_super(true)
            .with_roles(vec!["admin".to_string()])
            .with_permissions(vec!["user/list".to_string()]);
        let cloned = ctx.clone();
        assert_eq!(ctx.user_id, cloned.user_id);
        assert_eq!(ctx.is_super, cloned.is_super);
        assert_eq!(ctx.roles, cloned.roles);
        assert_eq!(ctx.permissions, cloned.permissions);
    }

    #[test]
    fn test_user_context_debug() {
        let ctx = UserContext::new(1).with_super(true);
        let debug_str = format!("{:?}", ctx);
        assert!(debug_str.contains("UserContext"));
        assert!(debug_str.contains("user_id"));
        assert!(debug_str.contains("is_super"));
    }

    #[test]
    fn test_user_context_builder_chain() {
        let ctx = UserContext::new(1)
            .with_super(false)
            .with_roles(vec!["editor".to_string()])
            .with_permissions(vec!["post/list".to_string(), "post/save".to_string()]);
        assert_eq!(ctx.user_id, 1);
        assert!(!ctx.is_super);
        assert_eq!(ctx.roles, vec!["editor"]);
        assert_eq!(ctx.permissions.len(), 2);
        assert!(ctx.has_role("editor"));
        assert!(ctx.has_permission("post/list"));
    }

    // ====================================================================
    // AuthGuard 单元测试
    // ====================================================================

    #[test]
    fn test_auth_guard_new() {
        let guard = AuthGuard::new();
        // 确保 new() 可调用
        let _ = format!("{:?}", guard);
    }

    #[test]
    fn test_auth_guard_passes_when_authenticated() {
        let guard = AuthGuard::new();
        let req = make_request_with_user(1);
        assert!(guard.check(&req).is_ok());
    }

    #[test]
    fn test_auth_guard_fails_when_not_authenticated() {
        let guard = AuthGuard::new();
        let req = make_request();
        let result = guard.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotLogin);
        assert_eq!(err.msg, "not_login");
    }

    // ====================================================================
    // AdminGuard 单元测试
    // ====================================================================

    #[test]
    fn test_admin_guard_new() {
        let _guard = AdminGuard::new();
    }

    #[test]
    fn test_admin_guard_fails_when_not_logged_in() {
        let guard = AdminGuard::new();
        let req = make_request();
        let result = guard.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // 未登录应返回 NotLogin（对齐 PHP checkLogin）
        assert_eq!(err.code, ErrorCode::NotLogin);
    }

    #[test]
    fn test_admin_guard_fails_when_logged_in_but_no_user_context() {
        let guard = AdminGuard::new();
        let req = make_request_with_user(1);
        let result = guard.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // 已登录但无 UserContext → Forbidden
        assert_eq!(err.code, ErrorCode::Forbidden);
    }

    #[test]
    fn test_admin_guard_fails_when_not_super() {
        let guard = AdminGuard::new();
        let user_ctx = UserContext::new(1).with_super(false);
        let req = make_request_with_context(user_ctx);
        let result = guard.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
    }

    #[test]
    fn test_admin_guard_passes_when_super() {
        let guard = AdminGuard::new();
        let user_ctx = UserContext::new(1).with_super(true);
        let req = make_request_with_context(user_ctx);
        assert!(guard.check(&req).is_ok());
    }

    // ====================================================================
    // PermissionGuard 单元测试
    // ====================================================================

    #[test]
    fn test_permission_guard_new() {
        let guard = PermissionGuard::new("user/list");
        assert_eq!(guard.permission, "user/list");
    }

    #[test]
    fn test_permission_guard_fails_when_not_logged_in() {
        let guard = PermissionGuard::new("user/list");
        let req = make_request();
        let result = guard.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotLogin);
    }

    #[test]
    fn test_permission_guard_fails_when_no_user_context() {
        let guard = PermissionGuard::new("user/list");
        let req = make_request_with_user(1);
        let result = guard.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
    }

    #[test]
    fn test_permission_guard_fails_when_no_permission() {
        let guard = PermissionGuard::new("user/delete");
        let user_ctx = UserContext::new(1).with_permissions(vec!["user/list".to_string()]);
        let req = make_request_with_context(user_ctx);
        let result = guard.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
    }

    #[test]
    fn test_permission_guard_passes_when_has_exact_permission() {
        let guard = PermissionGuard::new("user/list");
        let user_ctx = UserContext::new(1).with_permissions(vec!["user/list".to_string()]);
        let req = make_request_with_context(user_ctx);
        assert!(guard.check(&req).is_ok());
    }

    #[test]
    fn test_permission_guard_passes_when_has_wildcard_permission() {
        let guard = PermissionGuard::new("user/list");
        let user_ctx = UserContext::new(1).with_permissions(vec!["user/*".to_string()]);
        let req = make_request_with_context(user_ctx);
        assert!(guard.check(&req).is_ok());
    }

    #[test]
    fn test_permission_guard_passes_when_super() {
        // is_super=true 绕过所有 RBAC（对齐 PHP is_super=1）
        let guard = PermissionGuard::new("user/delete");
        let user_ctx = UserContext::new(1).with_super(true);
        let req = make_request_with_context(user_ctx);
        assert!(guard.check(&req).is_ok());
    }

    #[test]
    fn test_permission_guard_passes_when_super_without_permissions() {
        // is_super=true 即使 permissions 为空也通过
        let guard = PermissionGuard::new("system/config");
        let user_ctx = UserContext::new(1).with_super(true);
        let req = make_request_with_context(user_ctx);
        assert!(guard.check(&req).is_ok());
    }

    // ====================================================================
    // RoleGuard 单元测试
    // ====================================================================

    #[test]
    fn test_role_guard_new() {
        let guard = RoleGuard::new("admin");
        assert_eq!(guard.role, "admin");
    }

    #[test]
    fn test_role_guard_fails_when_not_logged_in() {
        let guard = RoleGuard::new("admin");
        let req = make_request();
        let result = guard.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotLogin);
    }

    #[test]
    fn test_role_guard_fails_when_no_user_context() {
        let guard = RoleGuard::new("admin");
        let req = make_request_with_user(1);
        let result = guard.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
    }

    #[test]
    fn test_role_guard_fails_when_no_role() {
        let guard = RoleGuard::new("admin");
        let user_ctx = UserContext::new(1).with_roles(vec!["editor".to_string()]);
        let req = make_request_with_context(user_ctx);
        let result = guard.check(&req);
        assert!(result.is_err());
    }

    #[test]
    fn test_role_guard_passes_when_has_role() {
        let guard = RoleGuard::new("admin");
        let user_ctx = UserContext::new(1).with_roles(vec!["admin".to_string()]);
        let req = make_request_with_context(user_ctx);
        assert!(guard.check(&req).is_ok());
    }

    #[test]
    fn test_role_guard_passes_when_super() {
        // is_super 绕过角色检查
        let guard = RoleGuard::new("admin");
        let user_ctx = UserContext::new(1).with_super(true);
        let req = make_request_with_context(user_ctx);
        assert!(guard.check(&req).is_ok());
    }

    // ====================================================================
    // GuardChain 单元测试
    // ====================================================================

    #[test]
    fn test_guard_chain_new() {
        let chain = GuardChain::new();
        assert!(chain.guards.is_empty());
    }

    #[test]
    fn test_guard_chain_default() {
        let chain = GuardChain::default();
        assert!(chain.guards.is_empty());
    }

    #[test]
    fn test_guard_chain_with_guard() {
        let chain = GuardChain::new()
            .with_guard(Arc::new(AuthGuard))
            .with_guard(Arc::new(AdminGuard));
        assert_eq!(chain.guards.len(), 2);
    }

    #[test]
    fn test_guard_chain_from_guards() {
        let guards: Vec<Arc<dyn Guard>> = vec![Arc::new(AuthGuard), Arc::new(AdminGuard)];
        let chain = GuardChain::from_guards(guards);
        assert_eq!(chain.guards.len(), 2);
    }

    #[test]
    fn test_guard_chain_empty_passes() {
        // 空链应通过（无 Guard 需要检查）
        let chain = GuardChain::new();
        let req = make_request();
        assert!(chain.check(&req).is_ok());
    }

    #[test]
    fn test_guard_chain_single_guard_passes() {
        let chain = GuardChain::new().with_guard(Arc::new(AuthGuard));
        let req = make_request_with_user(1);
        assert!(chain.check(&req).is_ok());
    }

    #[test]
    fn test_guard_chain_single_guard_fails() {
        let chain = GuardChain::new().with_guard(Arc::new(AuthGuard));
        let req = make_request();
        assert!(chain.check(&req).is_err());
    }

    #[test]
    fn test_guard_chain_and_semantics_all_pass() {
        let chain = GuardChain::new()
            .with_guard(Arc::new(AuthGuard))
            .with_guard(Arc::new(AdminGuard));
        let user_ctx = UserContext::new(1).with_super(true);
        let req = make_request_with_context(user_ctx);
        assert!(chain.check(&req).is_ok());
    }

    #[test]
    fn test_guard_chain_and_semantics_first_fails() {
        // AuthGuard 失败 → 整个链失败
        let chain = GuardChain::new()
            .with_guard(Arc::new(AuthGuard))
            .with_guard(Arc::new(AdminGuard));
        let req = make_request();
        let result = chain.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // 第一个 Guard 失败，返回 NotLogin
        assert_eq!(err.code, ErrorCode::NotLogin);
    }

    #[test]
    fn test_guard_chain_and_semantics_second_fails() {
        // AuthGuard 通过，AdminGuard 失败
        let chain = GuardChain::new()
            .with_guard(Arc::new(AuthGuard))
            .with_guard(Arc::new(AdminGuard));
        // 已登录但非超级管理员
        let user_ctx = UserContext::new(1).with_super(false);
        let req = make_request_with_context(user_ctx);
        let result = chain.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // 第二个 Guard 失败，返回 Forbidden
        assert_eq!(err.code, ErrorCode::Forbidden);
    }

    #[test]
    fn test_guard_chain_and_semantics_short_circuit() {
        // 短路：第一个失败后不执行后续 Guard
        struct FailGuard;
        impl Guard for FailGuard {
            fn check(&self, _req: &Request) -> Result<(), GuardError> {
                Err(GuardError::forbidden("fail_guard_called"))
            }
        }
        struct PanicGuard;
        impl Guard for PanicGuard {
            fn check(&self, _req: &Request) -> Result<(), GuardError> {
                panic!("PanicGuard should not be called due to short-circuit");
            }
        }
        let chain = GuardChain::new()
            .with_guard(Arc::new(FailGuard))
            .with_guard(Arc::new(PanicGuard));
        let req = make_request();
        let result = chain.check(&req);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().msg, "fail_guard_called");
    }

    #[test]
    fn test_guard_chain_order_matters() {
        // 顺序：先 AuthGuard，再 PermissionGuard
        let chain = GuardChain::new()
            .with_guard(Arc::new(AuthGuard))
            .with_guard(Arc::new(PermissionGuard::new("user/list")));
        // 未登录 → 应返回 NotLogin（而非 Forbidden）
        let req = make_request();
        let result = chain.check(&req);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotLogin);
    }

    #[test]
    fn test_guard_chain_multiple_permissions() {
        let chain = GuardChain::new()
            .with_guard(Arc::new(AuthGuard))
            .with_guard(Arc::new(PermissionGuard::new("user/list")))
            .with_guard(Arc::new(PermissionGuard::new("user/save")));
        let user_ctx = UserContext::new(1)
            .with_permissions(vec!["user/list".to_string(), "user/save".to_string()]);
        let req = make_request_with_context(user_ctx);
        assert!(chain.check(&req).is_ok());
    }

    #[test]
    fn test_guard_chain_mixed_guard_types() {
        let chain = GuardChain::new()
            .with_guard(Arc::new(AuthGuard))
            .with_guard(Arc::new(RoleGuard::new("editor")))
            .with_guard(Arc::new(PermissionGuard::new("post/list")));
        let user_ctx = UserContext::new(1)
            .with_roles(vec!["editor".to_string()])
            .with_permissions(vec!["post/list".to_string()]);
        let req = make_request_with_context(user_ctx);
        assert!(chain.check(&req).is_ok());
    }

    // ====================================================================
    // check_guards 函数测试
    // ====================================================================

    #[test]
    fn test_check_guards_empty() {
        let req = make_request();
        assert!(check_guards(&req, &[]).is_ok());
    }

    #[test]
    fn test_check_guards_all_pass() {
        let guards: Vec<Arc<dyn Guard>> = vec![Arc::new(AuthGuard)];
        let req = make_request_with_user(1);
        assert!(check_guards(&req, &guards).is_ok());
    }

    #[test]
    fn test_check_guards_fails() {
        let guards: Vec<Arc<dyn Guard>> = vec![Arc::new(AuthGuard)];
        let req = make_request();
        assert!(check_guards(&req, &guards).is_err());
    }

    // ====================================================================
    // guard_middleware 集成测试
    // ====================================================================

    #[tokio::test]
    async fn test_guard_middleware_passes() {
        let app = build_app(Arc::new(AuthGuard));
        // 模拟 Auth 中间件注入 AuthenticatedUser
        let req = Request::builder()
            .method("GET")
            .uri("/protected")
            .extension(AuthenticatedUser { user_id: 1 })
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_guard_middleware_fails_not_login() {
        let app = build_app(Arc::new(AuthGuard));
        let req = Request::builder()
            .method("GET")
            .uri("/protected")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // NotLogin → HTTP 401
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = read_body(resp).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["code"], -1);
        assert_eq!(json["msg"], "not_login");
    }

    #[tokio::test]
    async fn test_guard_middleware_fails_forbidden() {
        let app = build_app(Arc::new(AdminGuard));
        // 已登录但非超级管理员
        let req = Request::builder()
            .method("GET")
            .uri("/protected")
            .extension(AuthenticatedUser { user_id: 1 })
            .extension(UserContext::new(1).with_super(false))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Forbidden → HTTP 403
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = read_body(resp).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["code"], 403);
    }

    #[tokio::test]
    async fn test_guard_middleware_with_chain() {
        let chain = GuardChain::new()
            .with_guard(Arc::new(AuthGuard))
            .with_guard(Arc::new(AdminGuard));
        let app = build_app(Arc::new(chain));

        // 已登录 + 超级管理员 → 通过
        let req = Request::builder()
            .method("GET")
            .uri("/protected")
            .extension(AuthenticatedUser { user_id: 1 })
            .extension(UserContext::new(1).with_super(true))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 已登录 + 非超级管理员 → Forbidden
        let req = Request::builder()
            .method("GET")
            .uri("/protected")
            .extension(AuthenticatedUser { user_id: 2 })
            .extension(UserContext::new(2).with_super(false))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_guard_middleware_permission_guard() {
        let chain = GuardChain::new()
            .with_guard(Arc::new(AuthGuard))
            .with_guard(Arc::new(PermissionGuard::new("user/list")));
        let app = build_app(Arc::new(chain));

        // 有权限 → 通过
        let req = Request::builder()
            .method("GET")
            .uri("/protected")
            .extension(AuthenticatedUser { user_id: 1 })
            .extension(UserContext::new(1).with_permissions(vec!["user/list".to_string()]))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 无权限 → Forbidden
        let req = Request::builder()
            .method("GET")
            .uri("/protected")
            .extension(AuthenticatedUser { user_id: 2 })
            .extension(UserContext::new(2).with_permissions(vec!["order/list".to_string()]))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // ====================================================================
    // PHP 行为对齐验证
    // ====================================================================

    /// PHP is_super=1 绕过所有 RBAC
    /// 对齐 PHP szoa `is_super=1` 行为
    #[test]
    fn test_php_alignment_is_super_bypass() {
        let permission_guard = PermissionGuard::new("system/config");
        let role_guard = RoleGuard::new("admin");
        let admin_guard = AdminGuard::new();

        // is_super=true 即使无任何权限/角色也通过
        let user_ctx = UserContext::new(1).with_super(true);
        let req = make_request_with_context(user_ctx);

        assert!(permission_guard.check(&req).is_ok());
        assert!(role_guard.check(&req).is_ok());
        assert!(admin_guard.check(&req).is_ok());
    }

    /// PHP 错误码对齐
    /// - not_login → code=-1
    /// - 无权限 → code=403（Rust 扩展，PHP 端为 0）
    /// - 用户已禁用 → code=-3
    #[test]
    fn test_php_alignment_error_codes() {
        // not_login → code=-1（对齐 PHP）
        let err = GuardError::not_login("not_login");
        assert_eq!(err.code.as_i32(), -1);

        // 用户已禁用 → code=-3（对齐 PHP 您已离职）
        let err = GuardError::user_disabled("您已离职");
        assert_eq!(err.code.as_i32(), -3);

        // 无权限 → code=403（Rust 扩展，PHP 端为 0）
        // 注：PHP 端无统一无权限码，常用 0 表示失败；Rust 端使用 HTTP 403 更语义化
        let err = GuardError::forbidden("无权限访问");
        assert_eq!(err.code.as_i32(), 403);
    }

    /// PHP checkLogin 行为对齐
    /// 对齐 PHP `addons\BaseController::checkLogin`：
    /// - 未登录 → throw BaseException(['code' => -1, 'msg' => 'not_login'])
    /// - 已登录 → return（通过）
    #[test]
    fn test_php_alignment_check_login() {
        let guard = AuthGuard::new();

        // 未登录 → NotLogin
        let req = make_request();
        let result = guard.check(&req);
        assert!(matches!(
            result,
            Err(GuardError {
                code: ErrorCode::NotLogin,
                ..
            })
        ));

        // 已登录 → 通过
        let req = make_request_with_user(1);
        assert!(guard.check(&req).is_ok());
    }

    /// PHP 权限通配符对齐
    /// 对齐 PHP szoa `access` 表的通配符权限：`user/*` 匹配 `user/list` 等
    #[test]
    fn test_php_alignment_wildcard_permission() {
        let ctx = UserContext::new(1).with_permissions(vec!["user/*".to_string()]);
        assert!(ctx.has_permission("user/list"));
        assert!(ctx.has_permission("user/save"));
        assert!(ctx.has_permission("user/delete"));
        assert!(!ctx.has_permission("order/list"));
    }

    /// PHP RBAC 多角色组合对齐
    /// 对齐 PHP szoa `user_role` 多角色关联：用户可同时拥有多个角色，任一角色满足即通过
    #[test]
    fn test_php_alignment_multiple_roles() {
        let ctx = UserContext::new(1).with_roles(vec!["editor".to_string(), "viewer".to_string()]);
        assert!(ctx.has_role("editor"));
        assert!(ctx.has_role("viewer"));
        assert!(!ctx.has_role("admin"));
    }

    /// PHP 响应格式对齐
    /// 对齐 PHP `renderJson(code, msg, data)`：`{"code": <code>, "msg": "<msg>", "data": {}}`
    #[tokio::test]
    async fn test_php_alignment_response_format() {
        let err = GuardError::user_disabled("您已离职，无权使用本系统！");
        let resp = err.into_response();
        let body = read_body(resp).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        // 严格对齐 PHP renderJson 格式
        assert_eq!(json["code"], -3);
        assert_eq!(json["msg"], "您已离职，无权使用本系统！");
        assert_eq!(json["data"], serde_json::json!({}));
    }
}
