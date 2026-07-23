use crate::services::auth_service;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use std::collections::HashMap;
use sz_orm_core::{value_to_json, Value};
use sz_rust_core::controller::SzController;
use sz_rust_core::request::fetch_post_data;

struct AuthController;
impl SzController for AuthController {}

fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

fn row_to_json(row: &HashMap<String, Value>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in row {
        obj.insert(k.clone(), value_to_json(v.clone()));
    }
    serde_json::Value::Object(obj)
}

impl AuthController {
    pub async fn login(state: &AppState, req: Request<Body>) -> Response {
        match fetch_post_data(req).await {
            Ok(data) => {
                let username = data.get("username").and_then(|v| v.as_str()).unwrap_or("");
                let password = data.get("password").and_then(|v| v.as_str()).unwrap_or("");

                if username.is_empty() || password.is_empty() {
                    let ctrl = AuthController;
                    return ctrl.render_error("用户名和密码不能为空", json!({}), 0);
                }

                match auth_service::authenticate(username, password) {
                    Ok(token) => {
                        let ctrl = AuthController;

                        // 查询用户完整信息
                        let mut conn = match state.db_pool.acquire().await {
                            Ok(c) => c,
                            Err(_) => {
                                return ctrl.render_success(
                                    "登录成功",
                                    json!({
                                        "token": token,
                                        "username": username,
                                        "user_id": 0
                                    }),
                                )
                            }
                        };

                        let sql = format!(
                            "SELECT u.user_id, u.username, u.merchant_id, u.phone, u.role, \
                             m.name as merchant_name \
                             FROM merchant_user u \
                             LEFT JOIN merchant m ON u.merchant_id = m.merchant_id \
                             WHERE u.username = '{}'",
                            sql_escape(username)
                        );
                        let rows = match conn.query(&sql).await {
                            Ok(rows) => rows,
                            Err(_) => {
                                return ctrl.render_success(
                                    "登录成功",
                                    json!({
                                        "token": token,
                                        "username": username,
                                        "user_id": 0
                                    }),
                                )
                            }
                        };

                        if let Some(row) = rows.first() {
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
                        }
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

        // 查询数据库获取完整用户信息
        let mut conn = match state.db_pool.acquire().await {
            Ok(c) => c,
            Err(e) => return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0),
        };

        let sql = format!(
            "SELECT u.user_id, u.username, u.merchant_id, u.phone as contact_phone, u.role, u.status, \
             u.last_login_at, u.created_at, \
             m.name as merchant_name \
             FROM merchant_user u \
             LEFT JOIN merchant m ON u.merchant_id = m.merchant_id \
             WHERE u.user_id = {}",
            user.id
        );
        let rows = match conn.query(&sql).await {
            Ok(rows) => rows,
            Err(e) => return ctrl.render_error(&format!("查询用户失败: {}", e), json!({}), 0),
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

pub async fn login(State(state): State<AppState>, req: Request<Body>) -> Response {
    AuthController::login(&state, req).await
}

pub async fn me(State(state): State<AppState>, req: Request<Body>) -> Response {
    AuthController::me(&state, req).await
}

pub async fn refresh(State(_state): State<AppState>, req: Request<Body>) -> Response {
    let _data = req;
    let ctrl = AuthController;
    ctrl.render_success("ok", json!({}))
}

pub async fn logout(State(_state): State<AppState>, req: Request<Body>) -> Response {
    let _data = req;
    let ctrl = AuthController;
    ctrl.render_success("已退出登录", json!({}))
}
