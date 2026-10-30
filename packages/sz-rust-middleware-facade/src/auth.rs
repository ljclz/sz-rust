// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! @REVIEW_REQUIRED（铁律 R12）：人类必须审查此文件
//!
//! 审查要点：
//! - JWT 签名验证逻辑（算法白名单、密钥管理、过期处理）
//! - 白名单跳过逻辑（`allow_all` 列表是否完整、是否可被绕过）
//! - 错误路径是否泄漏敏感信息（如内部错误栈）
//!
//! 审查者签名：__________  日期：__________  结论：__________
//!
//! Auth 中间件 — JWT 校验 + 白名单跳过（对齐 PHP `addons\BaseController`）
//!
//! 对齐 PHP `addons\BaseController.php` 的鉴权流程：
//!
//! ```php
//! public function initialize() {
//!     $this->getRouteinfo();          // 解析当前路由
//!     $this->user = $this->getToken(); // JWT 校验
//!     $this->checkLogin();             // 验证登录状态
//! }
//!
//! public function getToken() {
//!     if (!$token = Token::getUserId(request()->header('Authorization'))) {
//!         if (in_array($this->routeUri, $this->allowAllAction)) {
//!             return true;             // 白名单放行
//!         } else {
//!             throw new BaseException(['msg' => '缺少必要的参数,请重新登陆!']);
//!         }
//!     }
//!     return $token;
//! }
//!
//! private function checkLogin(): void {
//!     if (in_array($this->routeUri, $this->allowAllAction)) {
//!         return;                      // 白名单放行
//!     }
//!     if (!empty($this->user) && $this->user['is_login'] == 1) {
//!         return;
//!     }
//!     throw new BaseException(['code' => -1, 'msg' => 'not_login']);
//! }
//! ```
//!
//! ## PHP 端默认白名单
//!
//! ```php
//! protected array $allowAllAction = [
//!     '/passport/login',
//!     '/task/task/userClerk',
//! ];
//! ```
//!
//! ## PHP JWT 配置（`app\common\service\jwt\Token`）
//!
//! - 签发人：`https://mall.ljclz.shop`
//! - 接收人：`https://mall.ljclz.shop`
//! - 密钥：通过 `SZ_JWT_SECRET` 环境变量提供（P1-SEC-09：不再硬编码）
//! - 有效期：30 天（`3600 * 24 * 30` 秒）
//! - 算法：HS256
//! - 自定义 claim：`user_id`
//!
//! PHP `Token::getUserId` 验证流程：
//! 1. 从 `Authorization` header 取 token（去除 `bearer` 前缀，大小写不敏感）
//! 2. 检查 token 是否在 `cache('delete_token')` 注销列表中（注销逻辑，缓存层实现）
//! 3. 解析 JWT
//! 4. 验证签发人（`IssuedBy`）
//! 5. 验证接收人（`PermittedFor`）
//! 6. 验证过期（`LooseValidAt`，时区 `Asia/Shanghai`）
//! 7. 取出 `user_id`
//!
//! ## Rust 端实现说明
//!
//! 复用 `sz-orm-auth` 的 `JwtEncoder::decode` 进行签名 + 过期校验，
//! 并在中间件层补充 PHP 端的额外校验：
//!
//! | PHP 验证项 | sz-orm-auth | Rust 中间件层补充 |
//! |-----------|-------------|------------------|
//! | 签名 | ✅ `JwtEncoder::decode` | — |
//! | 过期 | ✅ `JwtEncoder::decode` | — |
//! | 算法（HS256） | ✅ `JwtEncoder::decode` | — |
//! | 签发人（`iss`） | ❌ 不校验 | ✅ 本模块补充 |
//! | 接收人（`aud`） | ❌ 不校验 | ⚠️ 延迟到后续（Rust JwtClaims 无 `aud` 字段） |
//! | `bearer` 前缀去除 | ❌ | ✅ 本模块补充 |
//! | 白名单跳过 | ❌ | ✅ 本模块补充 |
//! | 注销列表 | ❌ | ⚠️ 延迟到缓存层 |
//!
//! ## 错误码对齐
//!
//! | 场景 | PHP code | PHP msg | Rust ErrorCode |
//! |------|---------|---------|---------------|
//! | 缺少 Authorization header + 非白名单 | -1 | `缺少必要的参数,请重新登陆!` | `NotLogin` |
//! | JWT 解析/校验失败 + 非白名单 | -1 | `缺少必要的参数,请重新登陆!` | `NotLogin` |
//! | user_id 缺失/无效 + 非白名单 | -1 | `not_login` | `NotLogin` |
//! | 白名单路由 | — | — | 放行（不校验） |
//!
//! PHP 端 `getToken()` 失败和 `checkLogin()` 失败都使用 `code = -1`，
//! Rust 端统一使用 `ErrorCode::NotLogin`（`-1`）。

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sz_rust_orm_facade::jwt::{JwtClaims, JwtEncoder};

use sz_rust_http_facade::{BaseException, ErrorCode};

/// PHP 端默认白名单（对齐 `addons\BaseController::$allowAllAction`）
///
/// ```php
/// protected array $allowAllAction = [
///     '/passport/login',
///     '/task/task/userClerk',
/// ];
/// ```
pub const DEFAULT_ALLOW_ALL_ACTION: &[&str] = &["/passport/login", "/task/task/userClerk"];

/// PHP JWT 默认签发人（对齐 `app\common\service\jwt\Token::$_config['issuer']`）
///
/// **注意**：仅用于测试与文档对照。生产环境必须通过 `SZ_JWT_ISSUER` 环境变量覆盖。
pub const DEFAULT_ISSUER: &str = "https://mall.ljclz.shop";

/// PHP JWT 默认密钥（对齐 `app\common\service\jwt\Token::$_config['sign']`）。
///
/// **安全警告**：此常量保留 PHP 原始值仅供**测试与文档对照**使用。
/// 生产环境必须通过 `SZ_JWT_SECRET` 环境变量提供密钥。
/// `AuthConfig::default()` 会优先从 `SZ_JWT_SECRET` 环境变量读取；
/// 仅在 `cfg(test)` 下回退到此常量以保持测试兼容性。
///
/// ## P1-SEC-09 修复说明
///
/// 旧版在 `cfg(test)` 下硬编码了 PHP JWT 密钥 `"shengzhuang"`，
/// 该字符串会被编译进所有测试二进制文件。若测试二进制泄漏，JWT 密钥即已知。
/// 修复方案：测试模式下改为运行时随机生成密钥（OsRng），不再包含任何硬编码秘密。
#[cfg(test)]
pub fn default_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    // 生成 64 字符十六进制随机密钥，每次测试运行唯一
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 生产环境占位符（任何构建下均为此值，不携带实际密钥）
///
/// 测试模式下 `AuthConfig::default()` 通过 `default_secret()` 运行时生成随机密钥，
/// 不使用此常量作为实际密钥。此常量仅用于文档和断言目的。
pub const DEFAULT_SECRET: &str = "<must-set-SZ_JWT_SECRET-env>";

/// PHP JWT 默认有效期（秒）（对齐 `app\common\service\jwt\Token::$_config['expire'] = 3600 * 24 * 30`）
pub const DEFAULT_EXPIRATION: u64 = 3600 * 24 * 30;

/// JWT 密钥最小长度（字节）
///
/// P1-SEC-04：密钥过短会导致暴力破解风险。HMAC-SHA256 要求密钥长度 ≥ 32 字节
///（即 256 位），与 SHA-256 输出长度匹配，确保完整的安全强度。
pub const MIN_SECRET_LEN: usize = 32;

/// Auth 中间件配置
///
/// 对齐 PHP `addons\BaseController` + `app\common\service\jwt\Token` 的配置。
///
/// ## P1-SEC-12 安全说明
///
/// `Debug` 手动实现：`secret` 字段始终脱敏为 `"[REDACTED]"`，
/// 防止 `{:?}` 格式化时将密钥泄漏到日志或 panic 信息中。
#[derive(Clone)]
pub struct AuthConfig {
    /// JWT 密钥（PHP `Token::$_config['sign']`）
    pub secret: String,
    /// JWT 签发人（PHP `Token::$_config['issuer']`）
    pub issuer: String,
    /// JWT 有效期（秒）（PHP `Token::$_config['expire']`）
    pub expiration: u64,
    /// 白名单路由列表（PHP `BaseController::$allowAllAction`）
    ///
    /// 支持通配符 `*`（对齐 PHP `AuthService::$allowAllAction` 中的 `/upload.library/*`）
    pub allow_all_action: Vec<String>,
}

impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthConfig")
            .field("secret", &"[REDACTED]")
            .field("issuer", &self.issuer)
            .field("expiration", &self.expiration)
            .field("allow_all_action", &self.allow_all_action)
            .finish()
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        // 生产环境：优先从环境变量读取 JWT 密钥
        // 测试环境：运行时随机生成密钥（P1-SEC-09：不再硬编码任何秘密值）
        let secret = std::env::var("SZ_JWT_SECRET").unwrap_or_else(|_| {
            #[cfg(test)]
            {
                default_secret()
            }
            #[cfg(not(test))]
            {
                panic!("SZ_JWT_SECRET 环境变量未设置 — 生产环境必须通过环境变量提供 JWT 密钥");
            }
        });
        // P1-SEC-04：密钥长度校验（fail-fast）
        validate_secret(&secret);
        let issuer = std::env::var("SZ_JWT_ISSUER").unwrap_or_else(|_| DEFAULT_ISSUER.to_string());
        Self {
            secret,
            issuer,
            expiration: DEFAULT_EXPIRATION,
            allow_all_action: DEFAULT_ALLOW_ALL_ACTION
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// 校验 JWT 密钥长度，过短则 panic（fail-fast，防止弱密钥上线）
///
/// P1-SEC-04：密钥长度 < 32 字节时 HMAC-SHA256 安全强度不足，必须在启动时拒绝。
fn validate_secret(secret: &str) {
    if secret.len() < MIN_SECRET_LEN {
        panic!(
            "SZ_JWT_SECRET 长度不足（当前 {} 字节，要求 ≥ {} 字节）— 请使用强随机密钥",
            secret.len(),
            MIN_SECRET_LEN
        );
    }
}

impl AuthConfig {
    /// 从环境变量构造配置（生产环境推荐用法）
    ///
    /// # Errors
    /// 当 `SZ_JWT_SECRET` 环境变量未设置时返回错误
    ///
    /// # Panics
    /// 当 `SZ_JWT_SECRET` 长度 < 32 字节时 panic（P1-SEC-04）
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let secret = std::env::var("SZ_JWT_SECRET")?;
        validate_secret(&secret);
        Ok(Self {
            secret,
            issuer: std::env::var("SZ_JWT_ISSUER").unwrap_or_else(|_| DEFAULT_ISSUER.to_string()),
            expiration: DEFAULT_EXPIRATION,
            allow_all_action: DEFAULT_ALLOW_ALL_ACTION
                .iter()
                .map(|s| s.to_string())
                .collect(),
        })
    }

    /// 创建带自定义白名单的配置
    pub fn with_allow_all_action(mut self, allow: Vec<String>) -> Self {
        self.allow_all_action = allow;
        self
    }

    /// 创建带自定义密钥的配置
    ///
    /// # Panics
    /// 当密钥长度 < 32 字节时 panic（P1-SEC-04）
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        let secret = secret.into();
        validate_secret(&secret);
        self.secret = secret;
        self
    }

    /// 创建带自定义签发人的配置
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = issuer.into();
        self
    }
}

/// Auth 中间件 — 对齐 PHP `addons\BaseController::initialize` 流程
///
/// ## 校验流程
///
/// 1. **路由白名单检查**：当前路由在 `allow_all_action` 中（含通配符匹配）→ 放行
/// 2. **取 Authorization header**：缺失 → `BaseException(['code' => -1, 'msg' => '缺少必要的参数,请重新登陆!'])`
/// 3. **去除 `bearer` 前缀**（大小写不敏感，对齐 PHP `Token::getRequestToken`）
/// 4. **JWT 校验**：`JwtEncoder::decode` + 签发人校验
///    - 解析失败 → `BaseException(['code' => -1, 'msg' => '缺少必要的参数,请重新登陆!'])`
///    - 过期 → `BaseException(['code' => -1, 'msg' => '缺少必要的参数,请重新登陆!'])`
///    - 签发人不匹配 → `BaseException(['code' => -1, 'msg' => '缺少必要的参数,请重新登陆!'])`
/// 5. **user_id 校验**：`claims.user_id` 为 `None` 或 `Some(0)` →
///    `BaseException(['code' => -1, 'msg' => 'not_login'])`
/// 6. **通过校验**：将 `user_id` 插入 request extensions，调用 `next`
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::middleware::auth::{auth_middleware, AuthConfig};
/// use axum::Router;
///
/// let config = AuthConfig::default();
/// let app: Router = Router::new()
///     .route("/", axum::routing::get(|| async { "ok" }))
///     .layer(axum::middleware::from_fn_with_state(config, auth_middleware));
/// ```
#[tracing::instrument(skip_all)]
pub async fn auth_middleware(
    axum::extract::State(config): axum::extract::State<AuthConfig>,
    req: Request,
    next: Next,
) -> Response {
    // 1. 路由白名单检查（对齐 PHP `checkLogin` 中的 `in_array($this->routeUri, $this->allowAllAction)`）
    let route_uri = extract_route_uri(&req);
    if is_route_allowed(&route_uri, &config.allow_all_action) {
        return next.run(req).await.into_response();
    }

    // 2. 取 Authorization header（对齐 PHP `Token::getUserId(request()->header('Authorization'))`）
    let auth_header = req.headers().get(axum::http::header::AUTHORIZATION);
    let token = match auth_header {
        Some(value) => {
            let raw = value.to_str().unwrap_or("");
            // 3. 去除 bearer 前缀（大小写不敏感，对齐 PHP `trim(str_ireplace('bearer', '', $header))`）
            extract_token_from_header(raw)
        }
        None => None,
    };

    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => {
            // PHP: `throw new BaseException(['msg' => '缺少必要的参数,请重新登陆!'])`
            return base_exception_to_response(BaseException::not_login(
                "缺少必要的参数,请重新登陆!",
            ));
        }
    };

    // 4. JWT 校验：JwtEncoder::decode + 签发人校验
    let encoder = JwtEncoder::new(&config.secret);
    let claims = match encoder.decode(&token) {
        Ok(c) => c,
        Err(_) => {
            // PHP 端 `Token::getUserId` 在任何 JWT 校验失败时都返回 null，
            // `BaseController::getToken` 把 null 当作「缺少必要的参数」处理
            return base_exception_to_response(BaseException::not_login(
                "缺少必要的参数,请重新登陆!",
            ));
        }
    };

    // 4.1 签发人校验（对齐 PHP `IssuedBy(self::$_config['issuer'])`）
    if !verify_issuer(&claims, &config.issuer) {
        return base_exception_to_response(BaseException::not_login("缺少必要的参数,请重新登陆!"));
    }

    // 5. user_id 校验（对齐 PHP `checkLogin` 中的 `$this->user['is_login'] == 1`）
    let user_id = match claims.user_id {
        Some(id) if id > 0 => id,
        _ => {
            // PHP: `throw new BaseException(['code' => -1, 'msg' => 'not_login'])`
            return base_exception_to_response(BaseException::not_login("not_login"));
        }
    };

    // 6. 通过校验：将 user_id 插入 request extensions，调用 next
    let mut req = req;
    req.extensions_mut().insert(AuthenticatedUser { user_id });
    next.run(req).await.into_response()
}

/// 将 `BaseException` 转换为 HTTP 响应
///
/// 使用 `BaseException::code` 对应的 HTTP 状态码（通过 `ErrorCode::from(code).http_status()`），
/// 响应体为标准 JSON 格式 `{"code":<code>,"msg":"<msg>","data":{}}`（对齐 PHP `renderJson`）。
pub fn base_exception_to_response(exc: BaseException) -> Response {
    let http_status = ErrorCode::from(exc.code).http_status();
    let body = exc.to_json().to_string();
    (
        StatusCode::from_u16(http_status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// 已认证用户信息（插入 request extensions，供后续 handler 使用）
#[derive(Debug, Clone, Copy)]
pub struct AuthenticatedUser {
    /// 用户 ID
    pub user_id: i64,
}

/// 从 Authorization header 值中提取 token（去除 `bearer` 前缀）
///
/// 对齐 PHP `Token::getRequestToken`：
/// ```php
/// $method = 'bearer';
/// return trim(str_ireplace($method, '', $header));
/// ```
///
/// PHP 的 `str_ireplace` 是大小写不敏感的字符串替换，会替换所有出现位置。
/// Rust 端去除前缀（`Bearer ` / `bearer` / `BEARER` 等），对齐 PHP 行为：
/// - `Bearer xxx` → `xxx`
/// - `bearer xxx` → `xxx`（大小写不敏感）
/// - `bearerxxx` → `xxx`（对齐 PHP `str_ireplace` 无空格替换）
/// - `xxx` → `xxx`（无 bearer 前缀时返回原值）
pub fn extract_token_from_header(header: &str) -> Option<String> {
    let trimmed = header.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 大小写不敏感匹配 bearer 前缀（对齐 PHP `str_ireplace`）
    // bearer 全部为 ASCII，使用 strip_prefix 后剩余部分用原始字符串截取以保留 token 大小写
    let lower = trimmed.to_lowercase();
    if let Some(suffix_len) = lower.strip_prefix("bearer ").map(|s| s.len()) {
        // 去除前缀后剩余部分（用原始字符串截取以保留大小写）
        let rest = &trimmed[trimmed.len() - suffix_len..];
        Some(rest.trim().to_string())
    } else if let Some(suffix_len) = lower.strip_prefix("bearer").map(|s| s.len()) {
        // 处理 `bearerxxx`（无空格）的情况，对齐 PHP `str_ireplace` 行为
        let rest = &trimmed[trimmed.len() - suffix_len..];
        Some(rest.trim().to_string())
    } else {
        // 无 bearer 前缀，直接返回原值（对齐 PHP：`str_ireplace` 找不到时不替换）
        Some(trimmed.to_string())
    }
}

/// 提取请求的路由 URI（用于白名单匹配）
///
/// 对齐 PHP `BaseController::getRouteinfo`：
/// ```php
/// $this->routeUri = '/' . $this->controller . '/' . $this->action;
/// ```
///
/// Rust 端使用 `req.uri().path()`，并去掉查询字符串。
pub fn extract_route_uri(req: &Request) -> String {
    req.uri().path().to_string()
}

/// 判断路由是否在白名单中（支持通配符 `*`）
///
/// 对齐 PHP `AuthService::$allowAllAction` 中的通配符匹配：
/// - 精确匹配：`/passport/login` == `/passport/login` → true
/// - 通配符匹配：`/upload.library/*` 匹配 `/upload.library/any` → true
/// - 通配符匹配：`/upload.library/*` 匹配 `/upload.library/sub/deep` → true
///   （PHP `fnmatch` 的 `*` 匹配任意字符包括 `/`）
pub fn is_route_allowed(route_uri: &str, allow_list: &[String]) -> bool {
    for pattern in allow_list {
        if pattern == route_uri {
            return true;
        }
        if pattern.contains('*') && wildcard_match(pattern, route_uri) {
            return true;
        }
    }
    false
}

/// 通配符匹配（`*` 匹配任意字符包括 `/`，对齐 PHP `fnmatch`）
///
/// 仅支持 `*` 通配符（对齐 PHP `AuthService` 白名单的实际使用场景）。
/// 不支持 `?`、`[`、`]` 等 fnmatch 特殊字符（PHP `fnmatch` 支持，
/// 但 PHP `AuthService` 的白名单中只用了 `*`）。
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    simple_wildcard_match(pattern, text)
}

/// 简单通配符匹配（仅支持 `*`，对齐 PHP `fnmatch` 中 `*` 的语义）
///
/// 算法：动态规划，时间复杂度 O(m*n)
fn simple_wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let m = p.len();
    let n = t.len();

    // dp[i][j] = pattern[0..i] 匹配 text[0..j]
    let mut dp = vec![vec![false; n + 1]; m + 1];
    dp[0][0] = true;

    // pattern 以 * 开头时可以匹配空字符串
    for i in 1..=m {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }

    for i in 1..=m {
        for j in 1..=n {
            if p[i - 1] == '*' {
                // * 匹配 0 个字符（dp[i-1][j]）或多个字符（dp[i][j-1]）
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if p[i - 1] == t[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }

    dp[m][n]
}

/// 校验 JWT 签发人（对齐 PHP `IssuedBy` constraint）
///
/// PHP 端使用 `Lcobucci\JWT\Validation\Constraint\IssuedBy`：
/// ```php
/// $issued = new IssuedBy(self::$_config['issuer']);
/// if (!$config->validator()->validate($token, $issued)) {
///     return null;
/// }
/// ```
///
/// Rust 端 `JwtEncoder::decode` 不校验签发人，需在本模块补充。
#[tracing::instrument(skip(claims))]
pub fn verify_issuer(claims: &JwtClaims, expected_issuer: &str) -> bool {
    match &claims.iss {
        Some(iss) => iss == expected_issuer,
        None => false,
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
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn make_request_with_uri(method: &str, uri: &str) -> Request {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn make_request_with_auth(method: &str, uri: &str, auth: &str) -> Request {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", auth)
            .body(Body::empty())
            .unwrap()
    }

    /// 生成有效 JWT token（用于测试）
    fn make_test_token(secret: &str, issuer: &str, user_id: i64, exp_offset_secs: i64) -> String {
        let encoder = JwtEncoder::new(secret);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims = JwtClaims::new("test_user", now + exp_offset_secs)
            .with_issuer(issuer)
            .with_user_id(user_id);
        encoder.encode(&claims).expect("encode token")
    }

    // ====================================================================
    // extract_token_from_header 单元测试
    // ====================================================================

    #[test]
    fn test_extract_token_from_header_with_bearer_prefix() {
        // 对齐 PHP: `Bearer xxx` → `xxx`
        let token = extract_token_from_header("Bearer abc123");
        assert_eq!(token, Some("abc123".to_string()));
    }

    #[test]
    fn test_extract_token_from_header_with_lowercase_bearer() {
        // 对齐 PHP: `bearer xxx` → `xxx`（大小写不敏感）
        let token = extract_token_from_header("bearer abc123");
        assert_eq!(token, Some("abc123".to_string()));
    }

    #[test]
    fn test_extract_token_from_header_with_uppercase_bearer() {
        // 对齐 PHP: `BEARER xxx` → `xxx`（大小写不敏感）
        let token = extract_token_from_header("BEARER abc123");
        assert_eq!(token, Some("abc123".to_string()));
    }

    #[test]
    fn test_extract_token_from_header_without_bearer_prefix() {
        // 对齐 PHP: 无 bearer 前缀时直接返回原值
        let token = extract_token_from_header("abc123");
        assert_eq!(token, Some("abc123".to_string()));
    }

    #[test]
    fn test_extract_token_from_header_with_empty_string() {
        let token = extract_token_from_header("");
        assert_eq!(token, None);
    }

    #[test]
    fn test_extract_token_from_header_with_only_whitespace() {
        let token = extract_token_from_header("   ");
        assert_eq!(token, None);
    }

    #[test]
    fn test_extract_token_from_header_with_bearer_no_space() {
        // 对齐 PHP `str_ireplace('bearer', '', 'bearerabc')` → `abc`
        let token = extract_token_from_header("bearerabc");
        assert_eq!(token, Some("abc".to_string()));
    }

    #[test]
    fn test_extract_token_from_header_trims_whitespace() {
        // 对齐 PHP `trim(...)` 去除首尾空白
        let token = extract_token_from_header("  Bearer   abc123  ");
        assert_eq!(token, Some("abc123".to_string()));
    }

    // ====================================================================
    // is_route_allowed / wildcard_match 单元测试
    // ====================================================================

    #[test]
    fn test_is_route_allowed_exact_match() {
        let allow = vec!["/passport/login".to_string()];
        assert!(is_route_allowed("/passport/login", &allow));
        assert!(!is_route_allowed("/passport/logout", &allow));
    }

    #[test]
    fn test_is_route_allowed_multiple_entries() {
        let allow = vec![
            "/passport/login".to_string(),
            "/task/task/userClerk".to_string(),
        ];
        assert!(is_route_allowed("/passport/login", &allow));
        assert!(is_route_allowed("/task/task/userClerk", &allow));
        assert!(!is_route_allowed("/passport/logout", &allow));
    }

    #[test]
    fn test_is_route_allowed_wildcard_suffix() {
        // 对齐 PHP `AuthService::$allowAllAction` 中的 `/upload.library/*`
        let allow = vec!["/upload.library/*".to_string()];
        assert!(is_route_allowed("/upload.library/any", &allow));
        assert!(is_route_allowed("/upload.library/sub/deep", &allow));
        assert!(!is_route_allowed("/upload.library", &allow)); // 缺少分隔
        assert!(!is_route_allowed("/other/path", &allow));
    }

    #[test]
    fn test_is_route_allowed_empty_list() {
        let allow: Vec<String> = vec![];
        assert!(!is_route_allowed("/any/path", &allow));
    }

    #[test]
    fn test_wildcard_match_plain() {
        assert!(wildcard_match("/upload/*", "/upload/any"));
        assert!(wildcard_match("/upload/*", "/upload/sub/deep"));
        assert!(!wildcard_match("/upload/*", "/other/any"));
    }

    #[test]
    fn test_wildcard_match_exact_no_star() {
        // 无 * 时退化为精确匹配
        assert!(wildcard_match("/passport/login", "/passport/login"));
        assert!(!wildcard_match("/passport/login", "/passport/logout"));
    }

    #[test]
    fn test_wildcard_match_multiple_stars() {
        assert!(wildcard_match("/*/*", "/a/b"));
        assert!(wildcard_match("/*/*", "/abc/def"));
        assert!(!wildcard_match("/*/*", "/a"));
    }

    #[test]
    fn test_wildcard_match_star_at_end() {
        assert!(wildcard_match("/api/*", "/api/v1/users"));
        assert!(wildcard_match("/api/*", "/api/"));
        assert!(!wildcard_match("/api/*", "/api"));
    }

    #[test]
    fn test_wildcard_match_empty_pattern_and_text() {
        assert!(wildcard_match("", ""));
        assert!(!wildcard_match("", "abc"));
        assert!(!wildcard_match("abc", ""));
    }

    #[test]
    fn test_wildcard_match_star_only() {
        // `*` 匹配任意字符串（包括空）
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*", "/path/to/anything"));
    }

    // ====================================================================
    // verify_issuer 单元测试
    // ====================================================================

    #[test]
    fn test_verify_issuer_matches() {
        let claims = JwtClaims::new("user", 9999999999).with_issuer("https://mall.ljclz.shop");
        assert!(verify_issuer(&claims, "https://mall.ljclz.shop"));
    }

    #[test]
    fn test_verify_issuer_mismatch() {
        let claims = JwtClaims::new("user", 9999999999).with_issuer("https://evil.com");
        assert!(!verify_issuer(&claims, "https://mall.ljclz.shop"));
    }

    #[test]
    fn test_verify_issuer_missing() {
        // 无签发人时校验失败（对齐 PHP `IssuedBy` 约束失败）
        let claims = JwtClaims::new("user", 9999999999);
        assert!(!verify_issuer(&claims, "https://mall.ljclz.shop"));
    }

    // ====================================================================
    // AuthConfig 单元测试
    // ====================================================================

    #[test]
    fn test_auth_config_default_matches_php() {
        // 对齐 PHP `Token::$_config` 默认值
        let config = AuthConfig::default();
        // P1-SEC-09: 测试模式下 secret 为运行时随机生成（不再硬编码 "shengzhuang"）
        // 验证其长度符合 OsRng 32 字节 → 64 hex 字符的预期
        assert_eq!(
            config.secret.len(),
            64,
            "测试模式 secret 应为 64 字符随机密钥"
        );
        assert_eq!(config.issuer, "https://mall.ljclz.shop");
        assert_eq!(config.expiration, 3600 * 24 * 30);
        // 对齐 PHP `BaseController::$allowAllAction`
        assert_eq!(
            config.allow_all_action,
            vec![
                "/passport/login".to_string(),
                "/task/task/userClerk".to_string(),
            ]
        );
    }

    #[test]
    fn test_auth_config_default_allow_all_action_constant() {
        // 验证常量与 Default 一致
        assert_eq!(DEFAULT_ALLOW_ALL_ACTION.len(), 2);
        assert_eq!(DEFAULT_ALLOW_ALL_ACTION[0], "/passport/login");
        assert_eq!(DEFAULT_ALLOW_ALL_ACTION[1], "/task/task/userClerk");
    }

    #[test]
    fn test_auth_config_builder_methods() {
        let config = AuthConfig::default()
            .with_secret("0123456789abcdef0123456789abcdef")
            .with_issuer("https://custom.com")
            .with_allow_all_action(vec!["/custom/login".to_string()]);

        assert_eq!(config.secret, "0123456789abcdef0123456789abcdef");
        assert_eq!(config.issuer, "https://custom.com");
        assert_eq!(config.allow_all_action, vec!["/custom/login".to_string()]);
    }

    /// P1-SEC-04：密钥过短应 panic
    #[test]
    #[should_panic(expected = "SZ_JWT_SECRET 长度不足")]
    fn test_secret_too_short_panics() {
        let _ = AuthConfig::default().with_secret("short");
    }

    /// P1-SEC-04：恰好 32 字节的密钥应通过
    #[test]
    fn test_secret_exactly_min_length_ok() {
        let config = AuthConfig::default().with_secret("0123456789abcdef0123456789abcdef");
        assert_eq!(config.secret.len(), 32);
    }

    #[test]
    fn test_auth_default_constants_match_php() {
        // 对齐 PHP `Token::$_config` 常量
        assert_eq!(DEFAULT_ISSUER, "https://mall.ljclz.shop");
        // P1-SEC-09: 生产占位符不再是实际密钥值
        assert_eq!(DEFAULT_SECRET, "<must-set-SZ_JWT_SECRET-env>");
        assert_eq!(DEFAULT_EXPIRATION, 3600 * 24 * 30);
    }

    // ====================================================================
    // extract_route_uri 单元测试
    // ====================================================================

    #[test]
    fn test_extract_route_uri_strips_query_string() {
        let req = Request::builder()
            .uri("/passport/login?foo=bar&baz=qux")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_route_uri(&req), "/passport/login");
    }

    #[test]
    fn test_extract_route_uri_no_query() {
        let req = Request::builder()
            .uri("/api/users")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_route_uri(&req), "/api/users");
    }

    #[test]
    fn test_extract_route_uri_root() {
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        assert_eq!(extract_route_uri(&req), "/");
    }

    // ====================================================================
    // auth_middleware 集成测试（通过 Router 验证）
    // ====================================================================

    /// 构建测试用 Router，使用给定 AuthConfig
    fn build_app(config: AuthConfig) -> Router {
        Router::new()
            .route("/protected", axum::routing::get(|| async { "protected" }))
            .route("/passport/login", axum::routing::get(|| async { "login" }))
            .route(
                "/upload.library/test",
                axum::routing::get(|| async { "upload" }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                auth_middleware,
            ))
    }

    #[tokio::test]
    async fn test_auth_middleware_allows_whitelisted_route() {
        // 对齐 PHP: 白名单 `/passport/login` 跳过 Auth 校验
        let app = build_app(AuthConfig::default());
        let resp = app
            .oneshot(make_request_with_uri("GET", "/passport/login"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "login");
    }

    #[tokio::test]
    async fn test_auth_middleware_rejects_missing_authorization_header() {
        // 对齐 PHP: 无 Authorization header → BaseException(['msg' => '缺少必要的参数,请重新登陆!'])
        let app = build_app(AuthConfig::default());
        let resp = app
            .oneshot(make_request_with_uri("GET", "/protected"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED); // NotLogin → 401
        let body = read_body(resp).await;
        assert!(body.contains("\"code\":-1"));
        assert!(body.contains("缺少必要的参数,请重新登陆!"));
    }

    #[tokio::test]
    async fn test_auth_middleware_rejects_empty_authorization_header() {
        // 对齐 PHP: Authorization header 为空字符串
        let app = build_app(AuthConfig::default());
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", ""))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = read_body(resp).await;
        assert!(body.contains("\"code\":-1"));
    }

    #[tokio::test]
    async fn test_auth_middleware_rejects_invalid_token() {
        // 对齐 PHP: JWT 解析失败 → BaseException(['msg' => '缺少必要的参数,请重新登陆!'])
        let app = build_app(AuthConfig::default());
        let resp = app
            .oneshot(make_request_with_auth(
                "GET",
                "/protected",
                "invalid.token.here",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = read_body(resp).await;
        assert!(body.contains("\"code\":-1"));
        assert!(body.contains("缺少必要的参数,请重新登陆!"));
    }

    #[tokio::test]
    async fn test_auth_middleware_rejects_expired_token() {
        // 对齐 PHP: token 过期 → BaseException(['msg' => '缺少必要的参数,请重新登陆!'])
        let config = AuthConfig::default();
        // 生成已过期的 token（exp = now - 3600）
        let token = make_test_token(&config.secret, &config.issuer, 1, -3600);
        let app = build_app(config.clone());
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", &token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = read_body(resp).await;
        assert!(body.contains("\"code\":-1"));
    }

    #[tokio::test]
    async fn test_auth_middleware_rejects_wrong_secret_token() {
        // 对齐 PHP: 用错误密钥签发的 token 无法通过签名校验
        let config = AuthConfig::default();
        let token = make_test_token("wrong-secret", &config.issuer, 1, 3600);
        let app = build_app(config.clone());
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", &token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_middleware_rejects_wrong_issuer_token() {
        // 对齐 PHP: 签发人不匹配 → IssuedBy 约束失败
        let config = AuthConfig::default();
        let token = make_test_token(&config.secret, "https://evil.com", 1, 3600);
        let app = build_app(config.clone());
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", &token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = read_body(resp).await;
        assert!(body.contains("缺少必要的参数,请重新登陆!"));
    }

    #[tokio::test]
    async fn test_auth_middleware_rejects_token_without_user_id() {
        // 对齐 PHP: user_id 缺失 → BaseException(['code' => -1, 'msg' => 'not_login'])
        let config = AuthConfig::default();
        let encoder = JwtEncoder::new(&config.secret);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        // 不设置 user_id
        let claims = JwtClaims::new("test_user", now + 3600).with_issuer(&config.issuer);
        let token = encoder.encode(&claims).unwrap();
        let app = build_app(config.clone());
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", &token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = read_body(resp).await;
        assert!(body.contains("\"code\":-1"));
        assert!(body.contains("not_login"));
    }

    #[tokio::test]
    async fn test_auth_middleware_rejects_token_with_zero_user_id() {
        // user_id = 0 视为无效（对齐 PHP `$this->user['is_login'] == 1` 检查）
        let config = AuthConfig::default();
        let token = make_test_token(&config.secret, &config.issuer, 0, 3600);
        let app = build_app(config.clone());
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", &token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = read_body(resp).await;
        assert!(body.contains("not_login"));
    }

    #[tokio::test]
    async fn test_auth_middleware_accepts_valid_token_with_bearer_prefix() {
        // 对齐 PHP: `Bearer <token>` 通过校验
        let config = AuthConfig::default();
        let token = make_test_token(&config.secret, &config.issuer, 42, 3600);
        let app = build_app(config.clone());
        let resp = app
            .oneshot(make_request_with_auth(
                "GET",
                "/protected",
                &format!("Bearer {}", token),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "protected");
    }

    #[tokio::test]
    async fn test_auth_middleware_accepts_valid_token_without_bearer_prefix() {
        // 对齐 PHP: `str_ireplace` 找不到 bearer 时直接返回原 token
        let config = AuthConfig::default();
        let token = make_test_token(&config.secret, &config.issuer, 42, 3600);
        let app = build_app(config.clone());
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", &token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_middleware_accepts_lowercase_bearer_prefix() {
        // 对齐 PHP: `bearer <token>` 大小写不敏感
        let config = AuthConfig::default();
        let token = make_test_token(&config.secret, &config.issuer, 42, 3600);
        let app = build_app(config.clone());
        let resp = app
            .oneshot(make_request_with_auth(
                "GET",
                "/protected",
                &format!("bearer {}", token),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_middleware_supports_wildcard_whitelist() {
        // 对齐 PHP `AuthService::$allowAllAction` 中的 `/upload.library/*`
        let config =
            AuthConfig::default().with_allow_all_action(vec!["/upload.library/*".to_string()]);
        let app = build_app(config);
        let resp = app
            .oneshot(make_request_with_uri("GET", "/upload.library/test"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "upload");
    }

    #[tokio::test]
    async fn test_auth_middleware_wildcard_does_not_overmatch() {
        // 通配符 `/upload.library/*` 不应匹配 `/upload.library`（无尾部路径）
        let config =
            AuthConfig::default().with_allow_all_action(vec!["/upload.library/*".to_string()]);
        let app = build_app(config);
        // `/upload.library/test` 在白名单中，但 `/protected` 不在
        let resp = app
            .oneshot(make_request_with_uri("GET", "/protected"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_middleware_injects_user_id_into_extensions() {
        // 验证通过校验后，user_id 被插入 request extensions
        let config = AuthConfig::default();
        let token = make_test_token(&config.secret, &config.issuer, 99, 3600);
        let app = Router::new()
            .route(
                "/protected",
                axum::routing::get(|req: Request| async move {
                    let user = req.extensions().get::<AuthenticatedUser>().unwrap();
                    format!("user_id:{}", user.user_id)
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config.clone(),
                auth_middleware,
            ));
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", &token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "user_id:99");
    }

    #[tokio::test]
    async fn test_auth_middleware_returns_correct_error_code_for_missing_token() {
        // 验证错误码对齐 PHP `BaseException(['code' => -1, ...])`
        let app = build_app(AuthConfig::default());
        let resp = app
            .oneshot(make_request_with_uri("GET", "/protected"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED); // ErrorCode::NotLogin → 401
        let body = read_body(resp).await;
        // JSON 响应格式：{"code":-1,"msg":"...","data":{}}
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["code"], -1);
        assert_eq!(json["msg"], "缺少必要的参数,请重新登陆!");
        assert_eq!(json["data"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn test_auth_middleware_returns_correct_error_code_for_not_login() {
        // 验证 user_id 缺失时错误码为 -1，msg 为 "not_login"
        let config = AuthConfig::default();
        let encoder = JwtEncoder::new(&config.secret);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims = JwtClaims::new("test_user", now + 3600).with_issuer(&config.issuer);
        let token = encoder.encode(&claims).unwrap();
        let app = build_app(config.clone());
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", &token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = read_body(resp).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["code"], -1);
        assert_eq!(json["msg"], "not_login");
    }

    #[tokio::test]
    async fn test_auth_middleware_custom_secret_and_issuer() {
        // 验证自定义密钥和签发人配置生效
        let config = AuthConfig::default()
            .with_secret("0123456789abcdef0123456789abcdef")
            .with_issuer("https://custom.com");
        let token = make_test_token(
            "0123456789abcdef0123456789abcdef",
            "https://custom.com",
            1,
            3600,
        );
        let app = build_app(config);
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", &token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_middleware_rejects_token_signed_with_default_secret_when_custom_configured()
    {
        // 自定义密钥后，用不同密钥签发的 token 应被拒绝
        let config = AuthConfig::default().with_secret("0123456789abcdef0123456789abcdef");
        let token = make_test_token("wrong-secret", &config.issuer, 1, 3600);
        let app = build_app(config);
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", &token))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_middleware_preserves_query_string_in_route_match() {
        // 验证带查询参数的白名单路由仍能匹配
        let app = build_app(AuthConfig::default());
        let resp = app
            .oneshot(make_request_with_uri(
                "GET",
                "/passport/login?redirect=/home",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_middleware_handles_token_with_only_bearer_prefix() {
        // `Bearer ` 后无内容 → 视为空 token
        let app = build_app(AuthConfig::default());
        let resp = app
            .oneshot(make_request_with_auth("GET", "/protected", "Bearer "))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ====================================================================
    // HeaderMap 与 PHP 对齐验证
    // ====================================================================

    #[test]
    fn test_authorization_header_name_aligns_with_php() {
        // PHP: `request()->header('Authorization')`
        // Rust: `req.headers().get(axum::http::header::AUTHORIZATION)`
        // 两者都使用标准 HTTP header 名 `Authorization`
        let header_name = axum::http::header::AUTHORIZATION;
        assert_eq!(header_name.as_str(), "authorization");
        // axum/http 的 header 名是 lowercase，PHP 端不区分大小写
    }

    // ====================================================================
    // PHP 行为对齐验证测试
    // ====================================================================

    #[test]
    fn test_php_default_allow_all_action_matches_rust() {
        // PHP `BaseController::$allowAllAction` 默认值
        let php_allow = vec!["/passport/login", "/task/task/userClerk"];
        // Rust `DEFAULT_ALLOW_ALL_ACTION`
        assert_eq!(php_allow, DEFAULT_ALLOW_ALL_ACTION);
    }

    #[test]
    fn test_php_jwt_config_matches_rust() {
        // PHP `Token::$_config`
        let php_issuer = "https://mall.ljclz.shop";
        let php_expire = 3600 * 24 * 30;

        // Rust 默认常量
        assert_eq!(php_issuer, DEFAULT_ISSUER);
        assert_eq!(php_expire, DEFAULT_EXPIRATION);
        // P1-SEC-09: DEFAULT_SECRET 在生产构建中为占位符，不再是硬编码密钥
        assert_eq!(DEFAULT_SECRET, "<must-set-SZ_JWT_SECRET-env>");
    }
}
