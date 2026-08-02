use crate::services::auth_service;
use crate::services::row_to_json;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_core::controller::SzController;
use sz_rust_core::middleware::csrf::{generate_token, CSRF_COOKIE_NAME};
use sz_rust_core::request::fetch_post_data;

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
    let cookie_value = format!(
        "{}={}; Path=/; Max-Age=86400; SameSite=Strict; Secure; HttpOnly=false",
        CSRF_COOKIE_NAME, token
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

impl AuthController {
    /// 用户登录 — 仅负责解析请求、调用 service、格式化响应
    ///
    /// 重构说明（2026-07-26 P1-5）：
    /// - 移除控制器内嵌 SQL，下沉到 `auth_service::get_user_info_by_username`
    /// - 控制器不再直接 `state.db_pool.acquire()`，符合分层架构
    /// - 参数解析错误不返回内部细节，统一返回 "参数解析失败"
    pub async fn login(_state: &AppState, req: Request<Body>) -> Response {
        let ctrl = AuthController;

        let data = match fetch_post_data(req).await {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(error = %e, "登录参数解析失败");
                return ctrl.render_error("参数解析失败", json!({}), 0);
            }
        };

        let username = data.get("username").and_then(|v| v.as_str()).unwrap_or("");
        let password = data.get("password").and_then(|v| v.as_str()).unwrap_or("");

        if username.is_empty() || password.is_empty() {
            return ctrl.render_error("用户名和密码不能为空", json!({}), 0);
        }

        // 调用 service 层认证 — 控制器不接触 DB
        let token = match auth_service::authenticate_async(username, password).await {
            Ok(t) => t,
            Err(msg) => return ctrl.render_error(&msg, json!({}), 0),
        };

        // 调用 service 层查询用户完整信息 — 控制器不接触 DB
        let user_info = auth_service::get_user_info_by_username(username).await;

        let mut resp = if let Some(row) = user_info {
            let info = row_to_json(&row);
            ctrl.render_success(
                "登录成功",
                json!({
                    "token": token,
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

/// 刷新登录令牌（对齐 PHP AuthController::refresh）
#[tracing::instrument(skip(_state, req))]
pub async fn refresh(State(_state): State<AppState>, req: Request<Body>) -> Response {
    let _data = req;
    let ctrl = AuthController;
    ctrl.render_success("ok", json!({}))
}

/// 退出登录（对齐 PHP AuthController::logout）
///
/// 清除客户端 CSRF Cookie，防止退出后 Cookie 残留。
#[tracing::instrument(skip(_state, req))]
pub async fn logout(State(_state): State<AppState>, req: Request<Body>) -> Response {
    let _data = req;
    let ctrl = AuthController;
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
}
