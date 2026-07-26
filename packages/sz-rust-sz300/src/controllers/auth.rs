use crate::services::auth_service;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use std::collections::HashMap;
use sz_rust_core::orm::{value_to_json, Value};
use sz_rust_core::controller::SzController;
use sz_rust_core::middleware::csrf::{generate_token, CSRF_COOKIE_NAME};
use sz_rust_core::request::fetch_post_data;

struct AuthController;
impl SzController for AuthController {}

fn row_to_json(row: &HashMap<String, Value>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in row {
        obj.insert(k.clone(), value_to_json(v.clone()));
    }
    serde_json::Value::Object(obj)
}

/// 在响应中附加 CSRF Cookie（登录成功时调用）
///
/// Cookie 属性：
/// - `HttpOnly=false`：允许前端 JS 读取，用于双提交 Cookie 模式
/// - `SameSite=Strict`：阻止跨站携带
/// - `Path=/`：全站可见
/// - `Max-Age=86400`：24 小时有效（与 JWT 过期时间对齐）
fn attach_csrf_cookie(response: &mut Response) {
    let token = generate_token();
    let cookie_value = format!(
        "{}={}; Path=/; Max-Age=86400; SameSite=Strict; HttpOnly=false",
        CSRF_COOKIE_NAME, token
    );
    if let Ok(value) = cookie_value.parse() {
        response.headers_mut().append("set-cookie", value);
    }
}

/// 在响应中清除 CSRF Cookie（退出登录时调用）
fn clear_csrf_cookie(response: &mut Response) {
    let cookie_value = format!(
        "{}=; Path=/; Max-Age=0; SameSite=Strict",
        CSRF_COOKIE_NAME
    );
    if let Ok(value) = cookie_value.parse() {
        response.headers_mut().append("set-cookie", value);
    }
}

impl AuthController {
    /// 用户登录 — 调用异步认证服务，成功后回查用户信息（参数化查询）
    ///
    /// 安全修复：
    /// 1. 使用 `auth_service::authenticate_async` 替代同步 `authenticate`，避免 block_in_place。
    /// 2. 用户信息查询使用 `query_with_params` + `?` 占位符，杜绝 SQL 注入。
    /// 3. DB 错误信息不返回客户端，仅记录日志。
    pub async fn login(state: &AppState, req: Request<Body>) -> Response {
        match fetch_post_data(req).await {
            Ok(data) => {
                let username = data.get("username").and_then(|v| v.as_str()).unwrap_or("");
                let password = data.get("password").and_then(|v| v.as_str()).unwrap_or("");

                if username.is_empty() || password.is_empty() {
                    let ctrl = AuthController;
                    return ctrl.render_error("用户名和密码不能为空", json!({}), 0);
                }

                // 异步认证 — 不再阻塞 tokio worker
                match auth_service::authenticate_async(username, password).await {
                    Ok(token) => {
                        let ctrl = AuthController;

                        // 参数化查询用户完整信息 — 杜绝 SQL 注入
                        let mut conn = match state.db_pool.acquire().await {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::error!(error = %e, "登录后获取 DB 连接失败");
                                let mut resp = ctrl.render_success(
                                    "登录成功",
                                    json!({
                                        "token": token,
                                        "username": username,
                                        "user_id": 0
                                    }),
                                );
                                attach_csrf_cookie(&mut resp);
                                return resp;
                            }
                        };

                        let sql = "SELECT u.user_id, u.username, u.merchant_id, u.phone, u.role, \
                                   m.name as merchant_name \
                                   FROM merchant_user u \
                                   LEFT JOIN merchant m ON u.merchant_id = m.merchant_id \
                                   WHERE u.username = ?";
                        let params = [Value::String(username.to_string())];
                        let rows = match conn.query_with_params(sql, &params).await {
                            Ok(rows) => rows,
                            Err(e) => {
                                tracing::error!(error = %e, "登录后查询用户信息失败");
                                let mut resp = ctrl.render_success(
                                    "登录成功",
                                    json!({
                                        "token": token,
                                        "username": username,
                                        "user_id": 0
                                    }),
                                );
                                attach_csrf_cookie(&mut resp);
                                return resp;
                            }
                        };

                        let mut resp = if let Some(row) = rows.first() {
                            let info = row_to_json(row);
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
                            ctrl.render_success(
                                "登录成功",
                                json!({
                                    "token": token,
                                    "username": username
                                }),
                            )
                        };
                        attach_csrf_cookie(&mut resp);
                        resp
                    }
                    Err(msg) => {
                        let ctrl = AuthController;
                        ctrl.render_error(&msg, json!({}), 0)
                    }
                }
            }
            Err(e) => {
                let ctrl = AuthController;
                ctrl.render_error("参数解析失败", json!({"error": e}), 0)
            }
        }
    }

    /// 获取当前登录用户信息 — 参数化查询，避免 SQL 注入
    pub async fn me(state: &AppState, req: Request<Body>) -> Response {
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

        // 查询数据库获取完整用户信息 — 参数化查询
        let mut conn = match state.db_pool.acquire().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "me 获取 DB 连接失败");
                return ctrl.render_error("服务暂时不可用", json!({}), 0);
            }
        };

        let sql = "SELECT u.user_id, u.username, u.merchant_id, u.phone as contact_phone, u.role, u.status, \
                   u.last_login_at, u.created_at, \
                   m.name as merchant_name \
                   FROM merchant_user u \
                   LEFT JOIN merchant m ON u.merchant_id = m.merchant_id \
                   WHERE u.user_id = ?";
        let params = [Value::I64(user.id)];
        let rows = match conn.query_with_params(sql, &params).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "me 查询用户失败");
                return ctrl.render_error("服务暂时不可用", json!({}), 0);
            }
        };

        if let Some(row) = rows.first() {
            let info = row_to_json(row);
            ctrl.render_success("ok", info)
        } else {
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
