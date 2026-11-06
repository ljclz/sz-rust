// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use crate::services::auth_service;
use crate::services::row_to_json;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_core::middleware::csrf::{generate_token, CSRF_COOKIE_NAME};
use sz_rust_core::request::fetch_post_data;
use sz_rust_mvc_facade::controller::SzController; // C4: facade 直连迁移

struct AuthController;
impl SzController for AuthController {}

/// 在响应中附加 CSRF Cookie（登录成功时调用）
///
/// Cookie 属性：
/// - `HttpOnly=false`：允许前端 JS 读取，用于双提交 Cookie 模式
/// - `SameSite=Strict`：阻止跨站携带
/// - `Path=/`：全站可见
/// - `Max-Age=86400`：24 小时有效（与 JWT 过期时间对齐）
/// - `Secure`：仅通过 HTTPS 传输（生产环境强制，防止 MITM 截获 CSRF token）
/// - `HttpOnly=false`：JS 需读取 token 以执行双重提交 Cookie 模式
fn attach_csrf_cookie(response: &mut Response) {
    let token = generate_token();
    // 2026-10-28 P1-11：生产环境强制 Secure，环境变量不可关闭（防止明文 HTTP 泄漏 CSRF token）
    let secure = if crate::config::is_production() {
        if crate::config::env_bool("SZ300_COOKIE_SECURE") == Some(false) {
            tracing::warn!("生产环境忽略 SZ300_COOKIE_SECURE=false，CSRF Cookie 强制 Secure");
        }
        true
    } else {
        crate::config::env_bool("SZ300_COOKIE_SECURE").unwrap_or(true)
    };
    let cookie_value = format!(
        "{}={}; Path=/; Max-Age=86400; SameSite=Strict; {}{}HttpOnly=false",
        CSRF_COOKIE_NAME,
        token,
        if secure { "Secure; " } else { "" },
        ""
    );
    if let Ok(value) = cookie_value.parse() {
        response.headers_mut().append("set-cookie", value);
    }
}

/// 在响应中清除 CSRF Cookie（退出登录时调用）
fn clear_csrf_cookie(response: &mut Response) {
    let cookie_value = format!("{}=; Path=/; Max-Age=0; SameSite=Strict", CSRF_COOKIE_NAME);
    if let Ok(value) = cookie_value.parse() {
        response.headers_mut().append("set-cookie", value);
    }
}

/// 校验登录凭据是否非空（空用户名或空密码直接拒绝，无需访问 DB）
fn credentials_non_empty(username: &str, password: &str) -> bool {
    !username.is_empty() && !password.is_empty()
}

impl AuthController {
    /// 用户登录 — 仅负责解析请求、调用 service、格式化响应
    ///
    /// 重构说明（2026-07-26 P1-5）：
    /// - 移除控制器内嵌 SQL，下沉到 `auth_service::get_user_info_by_username`
    /// - 控制器不再直接 `state.db_pool.acquire()`，符合分层架构
    /// - 参数解析错误不返回内部细节，统一返回 "参数解析失败"
    ///
    /// # HTTP 状态码（F-2 修复，2026-11-05，doc-debt DB-2026-11-05-02）
    ///
    /// - 200：登录成功（信封 `code:1`）
    /// - 400：参数缺失/解析失败/**凭据经 URL 查询参数提交**（禁止——URL 进
    ///   access log / 代理日志 / 浏览器历史，构成泄密面）
    /// - 401：用户名或密码错误（RFC 7235；此前为 HTTP 200 + 失败信封）
    /// - 503：DB 不可用等暂时性故障（"服务暂时不可用"）
    ///
    /// 信封体保持 `code:1=成功 / code:0=失败`（PHP 对齐约定，mvc-facade
    /// controller.rs 文档），仅 HTTP 状态对齐 RFC 语义。
    pub async fn login(_state: &AppState, req: Request<Body>) -> Response {
        let ctrl = AuthController;

        // 凭据禁止经 query 提交：框架层 fetch_post_data 为业务端点保留
        // query+body 合并（ThinkPHP 对齐），认证端点例外，body-only
        let query_keys = sz_rust_http_facade::request::parse_query(req.uri().query().unwrap_or(""));
        if query_keys.contains_key("username") || query_keys.contains_key("password") {
            let mut resp = ctrl.render_error(
                "登录凭据不得经 URL 查询参数提交（安全要求），请改用 JSON 请求体",
                json!({}),
                0,
            );
            *resp.status_mut() = axum::http::StatusCode::BAD_REQUEST;
            return resp;
        }

        let data = match fetch_post_data(req).await {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(error = %e, "登录参数解析失败");
                let mut resp = ctrl.render_error("参数解析失败", json!({}), 0);
                *resp.status_mut() = axum::http::StatusCode::BAD_REQUEST;
                return resp;
            }
        };

        let username = data.get("username").and_then(|v| v.as_str()).unwrap_or("");
        let password = data.get("password").and_then(|v| v.as_str()).unwrap_or("");

        if !credentials_non_empty(username, password) {
            let mut resp = ctrl.render_error("用户名和密码不能为空", json!({}), 0);
            *resp.status_mut() = axum::http::StatusCode::BAD_REQUEST;
            return resp;
        }

        // 调用 service 层认证 — 控制器不接触 DB
        let (token, refresh_token) =
            match auth_service::authenticate_async(username, password).await {
                Ok((t, r)) => (t, r),
                Err(msg) => {
                    // 状态映射（与 auth_service 错误字符串一一对应，services_test
                    // 断言消息文本防漂移）：凭据错误 401，DB 不可用 503，其余 500
                    let status = match msg.as_str() {
                        "用户名或密码错误" => axum::http::StatusCode::UNAUTHORIZED,
                        "服务暂时不可用" => axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        _ => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    };
                    let mut resp = ctrl.render_error(&msg, json!({}), 0);
                    *resp.status_mut() = status;
                    return resp;
                }
            };

        // 调用 service 层查询用户完整信息 — 控制器不接触 DB
        let user_info = auth_service::get_user_info_by_username(username).await;

        let mut resp = if let Some(row) = user_info {
            let info = row_to_json(&row);
            ctrl.render_success(
                "登录成功",
                json!({
                    "token": token,
                    "refresh_token": refresh_token,
                    "username": username,
                    "user_id": info.get("user_id"),
                    "merchant_id": info.get("merchant_id"),
                    "merchant_name": info.get("merchant_name"),
                    "phone": info.get("phone"),
                    "role": info.get("role")
                }),
            )
        } else {
            // service 返回 None：DB 暂时不可用或用户记录丢失，但 token 已签发
            // 返回基础信息，前端可凭 token 调用 /me 获取完整信息
            ctrl.render_success(
                "登录成功",
                json!({
                    "token": token,
                    "refresh_token": refresh_token,
                    "username": username,
                    "user_id": 0
                }),
            )
        };
        attach_csrf_cookie(&mut resp);
        resp
    }

    /// 获取当前登录用户信息 — 仅负责解析请求、调用 service、格式化响应
    ///
    /// 重构说明（2026-07-26 P1-5）：
    /// - 移除控制器内嵌 SQL，下沉到 `auth_service::get_user_info_by_id`
    /// - 控制器不再直接 `state.db_pool.acquire()`，符合分层架构
    pub async fn me(_state: &AppState, req: Request<Body>) -> Response {
        let auth_header = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let token = auth_header.strip_prefix("Bearer ").unwrap_or("");

        if token.is_empty() {
            let ctrl = AuthController;
            return ctrl.render_error("未提供认证令牌", json!({}), 0);
        }

        let user = match auth_service::verify_token(token) {
            Ok(u) => u,
            Err(msg) => {
                let ctrl = AuthController;
                return ctrl.render_error(&msg, json!({}), 0);
            }
        };

        let ctrl = AuthController;

        // 调用 service 层查询用户完整信息 — 控制器不接触 DB
        match auth_service::get_user_info_by_id(user.id).await {
            Some(row) => {
                let info = row_to_json(&row);
                ctrl.render_success("ok", info)
            }
            None => {
                // service 返回 None：DB 暂时不可用或用户记录丢失
                // 回退到 JWT claims 中的基础信息
                ctrl.render_success(
                    "ok",
                    json!({
                        "user_id": user.id,
                        "username": user.username,
                        "roles": user.roles
                    }),
                )
            }
        }
    }
}

/// 用户登录（对齐 PHP AuthController::login）
#[tracing::instrument(skip(state, req))]
pub async fn login(State(state): State<AppState>, req: Request<Body>) -> Response {
    AuthController::login(&state, req).await
}

/// 获取当前登录用户信息（对齐 PHP AuthController::me）
#[tracing::instrument(skip(state, req))]
pub async fn me(State(state): State<AppState>, req: Request<Body>) -> Response {
    AuthController::me(&state, req).await
}

/// 刷新登录令牌（2026-10-28 安全修复：真实轮换 access token）
///
/// 请求体：`{ "refresh_token": "<token>" }`
/// 响应：新 access_token；refresh token 无效/已注销返回 401 语义错误。
#[tracing::instrument(skip(_state, req))]
pub async fn refresh(State(_state): State<AppState>, req: Request<Body>) -> Response {
    let ctrl = AuthController;
    match fetch_post_data(req).await {
        Ok(data) => {
            let refresh_token = data
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if refresh_token.is_empty() {
                return ctrl.render_error("缺少 refresh_token 参数", json!({}), 0);
            }
            match auth_service::refresh_access_token(refresh_token) {
                Ok(token) => ctrl.render_success(
                    "ok",
                    json!({
                        "token": token,
                        "token_type": "Bearer",
                    }),
                ),
                Err(msg) => ctrl.render_error(&msg, json!({}), 0),
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "刷新令牌参数解析失败");
            ctrl.render_error("参数解析失败", json!({}), 0)
        }
    }
}

/// 退出登录（2026-10-28 安全修复：真实吊销 token）
///
/// 从 Authorization 头提取 Bearer token 加入黑名单，并清除 CSRF Cookie。
#[tracing::instrument(skip(_state, req))]
pub async fn logout(State(_state): State<AppState>, req: Request<Body>) -> Response {
    let ctrl = AuthController;
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = auth_header
        .strip_prefix("Bearer ")
        .unwrap_or("")
        .to_string();
    if !token.is_empty() {
        auth_service::revoke_token(&token);
    }
    let mut resp = ctrl.render_success("已退出登录", json!({}));
    clear_csrf_cookie(&mut resp);
    resp
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// P0-SEC-08：CSRF Cookie 必须包含 Secure 标志（防止 MITM 截获）
    #[test]
    fn test_p0_sec_08_csrf_cookie_has_secure_flag() {
        let mut resp = Response::default();
        attach_csrf_cookie(&mut resp);

        let cookie = resp
            .headers()
            .get("set-cookie")
            .expect("attach_csrf_cookie 必须设置 set-cookie 头");

        let cookie_str = cookie.to_str().expect("cookie 值应为合法 ASCII");
        assert!(
            cookie_str.contains("Secure"),
            "P0-SEC-08: CSRF Cookie 缺少 Secure 标志 — 攻击者可通过 HTTP 截获 CSRF token\nCookie: {cookie_str}"
        );
    }

    /// P0-SEC-08：CSRF Cookie 必须包含 SameSite=Strict（阻止跨站携带）
    #[test]
    fn test_p0_sec_08_csrf_cookie_has_samesite_strict() {
        let mut resp = Response::default();
        attach_csrf_cookie(&mut resp);

        let cookie = resp.headers().get("set-cookie").unwrap();
        let cookie_str = cookie.to_str().unwrap();
        assert!(
            cookie_str.contains("SameSite=Strict"),
            "CSRF Cookie 缺少 SameSite=Strict 标志\nCookie: {cookie_str}"
        );
    }

    /// P0-SEC-08：CSRF Cookie 必须设置 HttpOnly=false（JS 需读取，双重提交 Cookie 模式）
    #[test]
    fn test_p0_sec_08_csrf_cookie_http_only_false() {
        let mut resp = Response::default();
        attach_csrf_cookie(&mut resp);

        let cookie = resp.headers().get("set-cookie").unwrap();
        let cookie_str = cookie.to_str().unwrap();
        assert!(
            cookie_str.contains("HttpOnly=false"),
            "CSRF Cookie 应设置 HttpOnly=false（双重提交 Cookie 模式需要 JS 读取）\nCookie: {cookie_str}"
        );
    }

    /// 退出登录时应清除 CSRF Cookie（Max-Age=0）
    #[test]
    fn test_clear_csrf_cookie_sets_max_age_zero() {
        let mut resp = Response::default();
        clear_csrf_cookie(&mut resp);

        let cookie = resp.headers().get("set-cookie").unwrap();
        let cookie_str = cookie.to_str().unwrap();
        assert!(
            cookie_str.contains("Max-Age=0"),
            "退出登录应清除 CSRF Cookie（Max-Age=0）\nCookie: {cookie_str}"
        );
    }

    /// 空用户名/密码应在进入 DB 前被拒绝（P0：无凭据登录防护）
    #[test]
    fn test_credentials_non_empty_rejects_empty() {
        assert!(
            !credentials_non_empty("", "pass"),
            "空用户名应判定为凭据无效"
        );
        assert!(!credentials_non_empty("user", ""), "空密码应判定为凭据无效");
        assert!(!credentials_non_empty("", ""), "全空应判定为凭据无效");
        assert!(
            credentials_non_empty("user", "pass"),
            "非空凭据应判定为有效"
        );
    }

    // ====================================================================
    // handler 分支测试（axum oneshot 模式，覆盖 error/early-return 路径）
    //
    // 契约引用：
    // - C-MOCK-APP-STATE@1: crate::state::mock_app_state()
    // - C-AXUM-ONESHOT@1: Router + tower::ServiceExt::oneshot
    // - C-SZ-CONTROLLER@1: SzController trait（render_success/render_error）
    // ====================================================================

    use crate::state::mock_app_state;
    use axum::routing::post;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// 环境变量测试互斥锁 — attach_csrf_cookie/clear_csrf_cookie 读 SZ300_ENV/
    /// SZ300_COOKIE_SECURE，与 router.rs/main.rs 中 set_var 测试并行可能 race。
    /// 用 tokio Mutex 串行化所有涉及环境变量读取的 handler 测试
    /// （guard 需跨 oneshot().await 持有，std Mutex 会触发 clippy::await_holding_lock）。
    static ENV_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// 收集 Response body 为 String（C-AXUM-ONESHOT@1）
    async fn body_string(response: Response) -> String {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        String::from_utf8(bytes.to_vec()).expect("UTF-8")
    }

    /// 构造 POST JSON 请求
    fn json_request(uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// 构造带 Authorization header 的 POST JSON 请求
    fn json_request_with_auth(uri: &str, body: &str, auth: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("Authorization", auth)
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// 1. login query 参数提交凭据（?username=x&password=y）→ HTTP 400
    ///
    /// 凭据禁止经 URL 查询参数提交（URL 进 access log/代理日志/浏览器历史，构成泄密面）。
    #[tokio::test]
    async fn login_query_params_rejected_with_400() {
        let _guard = ENV_MUTEX.lock().await;
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/auth/login", post(login))
            .with_state(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login?username=admin&password=secret")
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "query 参数提交凭据应返回 400"
        );
        let body = body_string(resp).await;
        assert!(
            body.contains("查询参数"),
            "应提示查询参数禁止, 实际: {body}"
        );
    }

    /// 2. login body 解析失败（非 JSON）→ HTTP 400
    #[tokio::test]
    async fn login_invalid_json_body_returns_400() {
        let _guard = ENV_MUTEX.lock().await;
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/auth/login", post(login))
            .with_state(state);
        let req = json_request("/api/v1/auth/login", "{not valid json");
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "非法 JSON body 应返回 400"
        );
        let body = body_string(resp).await;
        assert!(
            body.contains("参数解析失败"),
            "应提示参数解析失败, 实际: {body}"
        );
    }

    /// 3. login 空用户名或空密码 → HTTP 400 且 msg 含"不能为空"
    #[tokio::test]
    async fn login_empty_credentials_returns_400() {
        let _guard = ENV_MUTEX.lock().await;
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/auth/login", post(login))
            .with_state(state);
        let req = json_request("/api/v1/auth/login", r#"{"username":"","password":""}"#);
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "空凭据应返回 400"
        );
        let body = body_string(resp).await;
        assert!(
            body.contains("不能为空"),
            "应提示用户名和密码不能为空, 实际: {body}"
        );
    }

    /// 4. me 无 Authorization header → code:0 错误响应
    #[tokio::test]
    async fn me_no_auth_header_returns_error_envelope() {
        let _guard = ENV_MUTEX.lock().await;
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/auth/me", post(me))
            .with_state(state);
        let req = json_request("/api/v1/auth/me", "{}");
        let resp = router.oneshot(req).await.unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains("\"code\":0"),
            "无 Authorization 应返回 code:0, 实际: {body}"
        );
        assert!(
            body.contains("未提供认证令牌"),
            "应提示未提供认证令牌, 实际: {body}"
        );
    }

    /// 5. me 空 Bearer token（"Bearer "）→ "未提供认证令牌"错误
    #[tokio::test]
    async fn me_empty_bearer_token_returns_error() {
        let _guard = ENV_MUTEX.lock().await;
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/auth/me", post(me))
            .with_state(state);
        let req = json_request_with_auth("/api/v1/auth/me", "{}", "Bearer ");
        let resp = router.oneshot(req).await.unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains("未提供认证令牌"),
            "空 Bearer token 应提示未提供认证令牌, 实际: {body}"
        );
    }

    /// 6. refresh 空 refresh_token → "缺少 refresh_token 参数"错误
    #[tokio::test]
    async fn refresh_empty_token_returns_error() {
        let _guard = ENV_MUTEX.lock().await;
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/auth/refresh", post(refresh))
            .with_state(state);
        let req = json_request("/api/v1/auth/refresh", r#"{"refresh_token":""}"#);
        let resp = router.oneshot(req).await.unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains("缺少 refresh_token 参数"),
            "空 refresh_token 应提示缺少参数, 实际: {body}"
        );
    }

    /// 7. refresh body 解析失败 → "参数解析失败"错误
    #[tokio::test]
    async fn refresh_invalid_json_body_returns_error() {
        let _guard = ENV_MUTEX.lock().await;
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/auth/refresh", post(refresh))
            .with_state(state);
        let req = json_request("/api/v1/auth/refresh", "{broken json");
        let resp = router.oneshot(req).await.unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains("参数解析失败"),
            "非法 JSON body 应提示参数解析失败, 实际: {body}"
        );
    }

    /// 8. logout 有 Bearer token → 调用 revoke_token 并清除 CSRF Cookie（Max-Age=0）
    #[tokio::test]
    async fn logout_with_token_clears_csrf_cookie() {
        let _guard = ENV_MUTEX.lock().await;
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/auth/logout", post(logout))
            .with_state(state);
        let req =
            json_request_with_auth("/api/v1/auth/logout", "{}", "Bearer fake.token.for.logout");
        let resp = router.oneshot(req).await.unwrap();
        // body_string 会消耗 resp（into_body），先提取 set-cookie header
        let cookie = resp
            .headers()
            .get("set-cookie")
            .expect("logout 应设置 set-cookie 清除 CSRF")
            .to_str()
            .unwrap()
            .to_string();
        let body = body_string(resp).await;
        assert!(
            body.contains("已退出登录"),
            "logout 应返回成功, 实际: {body}"
        );
        assert!(
            cookie.contains("Max-Age=0"),
            "logout 应清除 CSRF Cookie (Max-Age=0), 实际: {cookie}"
        );
    }

    /// 9. logout 无 Authorization header → 成功响应且不调用 revoke_token
    ///
    /// 无 token 时 `if !token.is_empty()` 跳过 revoke_token，但仍清除 CSRF Cookie。
    #[tokio::test]
    async fn logout_without_token_succeeds_no_revoke() {
        let _guard = ENV_MUTEX.lock().await;
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/auth/logout", post(logout))
            .with_state(state);
        let req = json_request("/api/v1/auth/logout", "{}");
        let resp = router.oneshot(req).await.unwrap();
        let cookie = resp
            .headers()
            .get("set-cookie")
            .expect("logout 应设置 set-cookie")
            .to_str()
            .unwrap()
            .to_string();
        let body = body_string(resp).await;
        assert!(
            body.contains("已退出登录"),
            "无 token logout 仍应返回成功, 实际: {body}"
        );
        assert!(
            body.contains("\"code\":1"),
            "无 token logout 应返回成功信封 code:1, 实际: {body}"
        );
        assert!(
            cookie.contains("Max-Age=0"),
            "logout 应清除 CSRF Cookie, 实际: {cookie}"
        );
    }
}
