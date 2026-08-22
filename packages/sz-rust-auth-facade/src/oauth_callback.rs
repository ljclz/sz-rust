//! OAuth2 回调中间件模块 — 需启用 `axum` feature
//!
//! 提供 [`OAuth2StateStore`] trait、[`OAuth2CallbackConfig`] 和 axum 中间件，
//! 处理 OAuth2 授权回调（authorization_code + implicit 流程）。

#[cfg(feature = "axum")]
use crate::oauth::OAuth2Provider;
#[cfg(feature = "axum")]
use std::sync::Arc;

// ============================================================================
// OAuth2StateStore — state 暂存 trait
// ============================================================================

/// OAuth2 state 暂存 trait — 用于 CSRF 防护
///
/// 在授权请求前保存 state，回调时校验 state 是否匹配。
/// 实现者保证 `Send + Sync`，可使用 session 或 Redis 作为后端。
pub trait OAuth2StateStore: Send + Sync {
    /// 保存 state（关联 client_id）
    fn save_state(&self, state: &str, client_id: &str);

    /// 获取 state 对应的 client_id（校验 state 是否有效）
    fn get_state(&self, state: &str) -> Option<String>;
}

// ============================================================================
// MemoryOAuth2StateStore — 测试用内存实现
// ============================================================================

use parking_lot::Mutex;
use std::collections::HashMap;

/// 内存 state 存储 — 用于测试和开发环境
#[derive(Default)]
pub struct MemoryOAuth2StateStore {
    states: Mutex<HashMap<String, String>>,
}

impl MemoryOAuth2StateStore {
    /// 创建新的内存 state 存储
    pub fn new() -> Self {
        Self::default()
    }
}

impl OAuth2StateStore for MemoryOAuth2StateStore {
    fn save_state(&self, state: &str, client_id: &str) {
        self.states
            .lock()
            .insert(state.to_string(), client_id.to_string());
    }

    fn get_state(&self, state: &str) -> Option<String> {
        self.states.lock().get(state).cloned()
    }
}

// ============================================================================
// axum 中间件实现
// ============================================================================

#[cfg(feature = "axum")]
mod axum_impl {
    use super::*;
    use axum::extract::{Query, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Redirect, Response};
    use axum::routing::get;
    use serde::Deserialize;

    /// OAuth2 回调配置
    pub struct OAuth2CallbackConfig {
        /// OAuth2 提供商
        pub provider: Arc<dyn OAuth2Provider>,
        /// state 暂存
        pub state_store: Arc<dyn OAuth2StateStore>,
        /// Token 存储（可选）
        #[cfg(feature = "redis-store")]
        pub token_store: Option<Arc<dyn crate::oauth_store::OAuth2TokenStore>>,
        /// 成功重定向 URL
        pub success_redirect: String,
    }

    /// 回调查询参数（authorization_code 流程）
    #[derive(Deserialize)]
    pub struct CallbackQuery {
        /// 授权码
        pub code: Option<String>,
        /// state 参数
        pub state: Option<String>,
        /// 错误码
        pub error: Option<String>,
    }

    /// OAuth2 回调处理函数
    ///
    /// 从回调 URL 提取 code + state，校验 state，换码，重定向。
    pub async fn oauth2_callback_handler(
        State(config): State<Arc<OAuth2CallbackConfig>>,
        Query(query): Query<CallbackQuery>,
    ) -> Response {
        // 检查错误回调
        if let Some(error) = &query.error {
            return (StatusCode::BAD_GATEWAY, format!("OAuth2 错误: {error}")).into_response();
        }

        // 提取 code 和 state
        let code = match &query.code {
            Some(c) if !c.is_empty() => c.clone(),
            _ => {
                return (StatusCode::BAD_REQUEST, "缺少授权码").into_response();
            }
        };

        let state = match &query.state {
            Some(s) if !s.is_empty() => s.clone(),
            _ => {
                return (StatusCode::BAD_REQUEST, "缺少 state 参数").into_response();
            }
        };

        // 校验 state（CSRF 防护）
        match config.state_store.get_state(&state) {
            Some(_) => {}
            None => {
                return (
                    StatusCode::FORBIDDEN,
                    "OAUTH2_CSRF_STATE_MISMATCH: state 校验失败",
                )
                    .into_response();
            }
        }

        // 用授权码换取用户信息
        match config.provider.user_from_token(&code) {
            Ok(_user) => {
                // 成功 → 重定向到 success_redirect
                Redirect::to(&config.success_redirect).into_response()
            }
            Err(err) => {
                // 失败 → 返回错误页
                (
                    StatusCode::BAD_GATEWAY,
                    format!("OAuth2 token 交换失败: {err}"),
                )
                    .into_response()
            }
        }
    }

    /// 创建 OAuth2 回调路由
    pub fn oauth2_callback_route(config: Arc<OAuth2CallbackConfig>) -> axum::Router {
        axum::Router::new()
            .route("/callback", get(oauth2_callback_handler))
            .with_state(config)
    }
}

#[cfg(feature = "axum")]
pub use axum_impl::*;

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 MemoryOAuth2StateStore save + get
    #[test]
    fn test_state_store_save_get() {
        let store = MemoryOAuth2StateStore::new();
        store.save_state("state123", "client1");

        let result = store.get_state("state123");
        assert_eq!(result.as_deref(), Some("client1"));
    }

    /// 测试 MemoryOAuth2StateStore get 不存在的 state
    #[test]
    fn test_state_store_get_nonexistent() {
        let store = MemoryOAuth2StateStore::new();
        let result = store.get_state("nonexistent");
        assert!(result.is_none());
    }

    /// 测试 MemoryOAuth2StateStore 不同 state 对应不同 client
    #[test]
    fn test_state_store_multiple() {
        let store = MemoryOAuth2StateStore::new();
        store.save_state("state1", "client1");
        store.save_state("state2", "client2");

        assert_eq!(store.get_state("state1").as_deref(), Some("client1"));
        assert_eq!(store.get_state("state2").as_deref(), Some("client2"));
    }
}
