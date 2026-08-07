//! 简化版路由匹配 — WASM 环境下的轻量路由
//!
//! 不依赖 axum，提供基本的路径匹配和参数提取。

use std::collections::HashMap;

// ============================================================================
// 路由匹配
// ============================================================================

/// 路由匹配结果
#[derive(Debug, Clone)]
pub struct RouteMatch {
    /// 匹配的路径参数
    pub params: HashMap<String, String>,
}

impl RouteMatch {
    /// 创建空匹配
    pub fn new() -> Self {
        Self {
            params: HashMap::new(),
        }
    }

    /// 添加路径参数
    pub fn with_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }

    /// 获取路径参数
    pub fn param(&self, key: &str) -> Option<&String> {
        self.params.get(key)
    }
}

impl Default for RouteMatch {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// SimpleRouter
// ============================================================================

/// 简化版路由器
///
/// 支持路径参数（如 `/users/:id`），不支持通配符和正则。
///
/// # 用法
///
/// ```rust
/// use sz_rust_wasm::router::SimpleRouter;
///
/// let router = SimpleRouter::new()
///     .add_route("GET", "/users/:id", "get_user")
///     .add_route("POST", "/users", "create_user");
///
/// let matched = router.match_route("GET", "/users/123");
/// assert!(matched.is_some());
/// let (handler, m) = matched.unwrap();
/// assert_eq!(handler, "get_user");
/// assert_eq!(m.param("id"), Some(&"123".to_string()));
/// ```
pub struct SimpleRouter {
    /// 路由表：(method, path_pattern) → handler_name
    routes: Vec<(String, String, String)>,
}

impl SimpleRouter {
    /// 创建空路由器
    pub fn new() -> Self {
        Self { routes: vec![] }
    }

    /// 添加路由
    ///
    /// # 参数
    ///
    /// - `method`: HTTP 方法
    /// - `pattern`: 路径模式（支持 `:param` 参数）
    /// - `handler`: 处理器名称
    pub fn add_route(
        mut self,
        method: impl Into<String>,
        pattern: impl Into<String>,
        handler: impl Into<String>,
    ) -> Self {
        self.routes
            .push((method.into(), pattern.into(), handler.into()));
        self
    }

    /// 匹配路由
    ///
    /// # 返回
    ///
    /// 匹配成功返回 `(handler_name, RouteMatch)`，不匹配返回 `None`。
    pub fn match_route(&self, method: &str, path: &str) -> Option<(&str, RouteMatch)> {
        for (route_method, pattern, handler) in &self.routes {
            if route_method != method {
                continue;
            }
            if let Some(match_result) = match_pattern(pattern, path) {
                return Some((handler.as_str(), match_result));
            }
        }
        None
    }

    /// 获取路由数量
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }
}

impl Default for SimpleRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// 匹配路径模式
///
/// 支持路径参数（如 `:id`），不支持通配符。
fn match_pattern(pattern: &str, path: &str) -> Option<RouteMatch> {
    let pattern_parts: Vec<&str> = pattern.trim_start_matches('/').split('/').collect();
    let path_parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    if pattern_parts.len() != path_parts.len() {
        return None;
    }

    let mut match_result = RouteMatch::new();

    for (p_part, path_part) in pattern_parts.iter().zip(path_parts.iter()) {
        if let Some(param_name) = p_part.strip_prefix(':') {
            match_result = match_result.with_param(param_name, *path_part);
        } else if *p_part != *path_part {
            return None;
        }
    }

    Some(match_result)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_match_new() {
        let m = RouteMatch::new();
        assert!(m.params.is_empty());
    }

    #[test]
    fn test_route_match_with_param() {
        let m = RouteMatch::new()
            .with_param("id", "123")
            .with_param("name", "test");
        assert_eq!(m.param("id"), Some(&"123".to_string()));
        assert_eq!(m.param("name"), Some(&"test".to_string()));
        assert_eq!(m.param("nonexistent"), None);
    }

    #[test]
    fn test_simple_router_basic() {
        let router = SimpleRouter::new()
            .add_route("GET", "/users", "list_users")
            .add_route("POST", "/users", "create_user");

        let matched = router.match_route("GET", "/users");
        assert!(matched.is_some());
        let (handler, _) = matched.unwrap();
        assert_eq!(handler, "list_users");

        let matched = router.match_route("POST", "/users");
        assert!(matched.is_some());
        let (handler, _) = matched.unwrap();
        assert_eq!(handler, "create_user");
    }

    #[test]
    fn test_simple_router_with_params() {
        let router = SimpleRouter::new().add_route("GET", "/users/:id", "get_user");

        let matched = router.match_route("GET", "/users/123");
        assert!(matched.is_some());
        let (handler, m) = matched.unwrap();
        assert_eq!(handler, "get_user");
        assert_eq!(m.param("id"), Some(&"123".to_string()));
    }

    #[test]
    fn test_simple_router_multiple_params() {
        let router = SimpleRouter::new().add_route("GET", "/users/:userId/posts/:postId", "get_post");

        let matched = router.match_route("GET", "/users/42/posts/99");
        assert!(matched.is_some());
        let (handler, m) = matched.unwrap();
        assert_eq!(handler, "get_post");
        assert_eq!(m.param("userId"), Some(&"42".to_string()));
        assert_eq!(m.param("postId"), Some(&"99".to_string()));
    }

    #[test]
    fn test_simple_router_no_match() {
        let router = SimpleRouter::new().add_route("GET", "/users", "list_users");

        assert!(router.match_route("GET", "/posts").is_none());
        assert!(router.match_route("POST", "/users").is_none());
    }

    #[test]
    fn test_simple_router_method_mismatch() {
        let router = SimpleRouter::new().add_route("GET", "/users", "list_users");

        assert!(router.match_route("POST", "/users").is_none());
    }

    #[test]
    fn test_simple_router_empty() {
        let router = SimpleRouter::new();
        assert_eq!(router.route_count(), 0);
        assert!(router.match_route("GET", "/").is_none());
    }

    #[test]
    fn test_simple_router_route_count() {
        let router = SimpleRouter::new()
            .add_route("GET", "/a", "handler_a")
            .add_route("GET", "/b", "handler_b")
            .add_route("POST", "/c", "handler_c");
        assert_eq!(router.route_count(), 3);
    }

    #[test]
    fn test_match_pattern_root() {
        let result = match_pattern("/", "/");
        assert!(result.is_some());
    }

    #[test]
    fn test_match_pattern_different_lengths() {
        assert!(match_pattern("/a/b", "/a").is_none());
        assert!(match_pattern("/a", "/a/b").is_none());
    }

    #[test]
    fn test_match_pattern_static() {
        assert!(match_pattern("/users/list", "/users/list").is_some());
        assert!(match_pattern("/users/list", "/users/create").is_none());
    }
}