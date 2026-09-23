// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 路由匹配引擎（T027）
//!
//! 基于路径/Header/Host 匹配，支持 `*` 通配符

use std::collections::HashMap;

use crate::config::GatewayRoute;
use crate::error::GatewayError;

/// 匹配请求
#[derive(Debug, Clone)]
pub struct MatchRequest {
    /// 请求路径
    pub path: String,
    /// Host
    pub host: Option<String>,
    /// 请求 Header
    pub headers: HashMap<String, String>,
}

impl MatchRequest {
    /// 构造匹配请求
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            host: None,
            headers: HashMap::new(),
        }
    }

    /// 设置 Host
    pub fn with_host(mut self, host: &str) -> Self {
        self.host = Some(host.to_string());
        self
    }

    /// 添加 Header
    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.headers.insert(key.to_string(), value.to_string());
        self
    }
}

/// 路由匹配引擎
pub struct RouterEngine {
    routes: Vec<GatewayRoute>,
}

impl RouterEngine {
    /// 创建路由引擎
    pub fn new(routes: Vec<GatewayRoute>) -> Self {
        Self { routes }
    }

    /// 匹配请求，返回命中的路由
    pub fn match_route(&self, req: &MatchRequest) -> Result<&GatewayRoute, GatewayError> {
        let mut best_match: Option<&GatewayRoute> = None;
        let mut best_score: i32 = -1;

        for route in &self.routes {
            if let Some(score) = self.try_match(route, req) {
                if score > best_score {
                    best_score = score;
                    best_match = Some(route);
                }
            }
        }

        best_match.ok_or_else(|| GatewayError::NoRoute(req.path.clone()))
    }

    /// 获取所有路由
    pub fn routes(&self) -> &[GatewayRoute] {
        &self.routes
    }

    fn try_match(&self, route: &GatewayRoute, req: &MatchRequest) -> Option<i32> {
        let mut score: i32 = 0;

        if !Self::match_path(&route.match_path, &req.path) {
            return None;
        }
        score += Self::path_specificity(&route.match_path);

        if let Some(ref host) = route.match_host {
            match req.host.as_deref() {
                Some(h) if h == host => score += 100,
                _ => return None,
            }
        }

        for (key, value) in &route.match_headers {
            match req.headers.get(key) {
                Some(v) if v == value => score += 10,
                _ => return None,
            }
        }

        Some(score)
    }

    /// 路径匹配（支持 `*` 通配符）
    fn match_path(pattern: &str, path: &str) -> bool {
        if pattern == path {
            return true;
        }

        if let Some(prefix) = pattern.strip_suffix("*") {
            return path.starts_with(prefix);
        }

        false
    }

    /// 路径特异性评分（越具体分越高）
    fn path_specificity(path: &str) -> i32 {
        if let Some(prefix) = path.strip_suffix('*') {
            prefix.matches('/').count() as i32 * 10
        } else {
            path.matches('/').count() as i32 * 10 + 5
        }
    }

    /// 应用路径重写
    pub fn rewrite_path(route: &GatewayRoute, original_path: &str) -> String {
        match &route.path_rewrite {
            None => original_path.to_string(),
            Some(rewrite) => {
                if let (Some(pat_prefix), Some(rw_prefix)) = (
                    route.match_path.strip_suffix("*"),
                    rewrite.strip_suffix("*"),
                ) {
                    if let Some(suffix) = original_path.strip_prefix(pat_prefix) {
                        return format!("{rw_prefix}{suffix}");
                    }
                }
                rewrite.to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_route(id: &str, path: &str, service: &str) -> GatewayRoute {
        GatewayRoute::simple(id, path, service)
    }

    #[test]
    fn test_match_exact_path() {
        let engine = RouterEngine::new(vec![make_route("r1", "/api/users", "user-svc")]);
        let req = MatchRequest::new("/api/users");
        let route = engine.match_route(&req).unwrap();
        assert_eq!(route.route_id, "r1");
    }

    #[test]
    fn test_match_wildcard_path() {
        let engine = RouterEngine::new(vec![make_route("r1", "/api/*", "api-svc")]);
        let req = MatchRequest::new("/api/users/123");
        let route = engine.match_route(&req).unwrap();
        assert_eq!(route.route_id, "r1");
    }

    #[test]
    fn test_no_match() {
        let engine = RouterEngine::new(vec![make_route("r1", "/api/users", "svc")]);
        let req = MatchRequest::new("/api/orders");
        let result = engine.match_route(&req);
        assert!(matches!(result, Err(GatewayError::NoRoute(_))));
    }

    #[test]
    fn test_match_host() {
        let route = make_route("r1", "/api/*", "svc").with_host("api.example.com");
        let engine = RouterEngine::new(vec![route]);

        let req = MatchRequest::new("/api/data").with_host("api.example.com");
        assert!(engine.match_route(&req).is_ok());

        let req = MatchRequest::new("/api/data").with_host("other.com");
        assert!(engine.match_route(&req).is_err());
    }

    #[test]
    fn test_match_headers() {
        let route = make_route("r1", "/api/*", "svc").with_header_match("X-Version", "2");
        let engine = RouterEngine::new(vec![route]);

        let req = MatchRequest::new("/api/data").with_header("X-Version", "2");
        assert!(engine.match_route(&req).is_ok());

        let req = MatchRequest::new("/api/data").with_header("X-Version", "1");
        assert!(engine.match_route(&req).is_err());

        let req = MatchRequest::new("/api/data");
        assert!(engine.match_route(&req).is_err());
    }

    #[test]
    fn test_specificity_prefers_exact_over_wildcard() {
        let routes = vec![
            make_route("wildcard", "/api/*", "svc1"),
            make_route("exact", "/api/users", "svc2"),
        ];
        let engine = RouterEngine::new(routes);

        let req = MatchRequest::new("/api/users");
        let route = engine.match_route(&req).unwrap();
        assert_eq!(
            route.route_id, "exact",
            "exact match should win over wildcard"
        );
    }

    #[test]
    fn test_specificity_prefers_deeper_wildcard() {
        let routes = vec![
            make_route("shallow", "/api/*", "svc1"),
            make_route("deep", "/api/users/*", "svc2"),
        ];
        let engine = RouterEngine::new(routes);

        let req = MatchRequest::new("/api/users/123");
        let route = engine.match_route(&req).unwrap();
        assert_eq!(route.route_id, "deep", "deeper wildcard should win");
    }

    #[test]
    fn test_rewrite_path_none() {
        let route = make_route("r1", "/api/users", "svc");
        let rewritten = RouterEngine::rewrite_path(&route, "/api/users");
        assert_eq!(rewritten, "/api/users");
    }

    #[test]
    fn test_rewrite_path_wildcard() {
        let route = make_route("r1", "/api/*", "svc").with_rewrite("/v2/*");
        let rewritten = RouterEngine::rewrite_path(&route, "/api/users/123");
        assert_eq!(rewritten, "/v2/users/123");
    }

    #[test]
    fn test_rewrite_path_static() {
        let route = make_route("r1", "/old", "svc").with_rewrite("/new");
        let rewritten = RouterEngine::rewrite_path(&route, "/old");
        assert_eq!(rewritten, "/new");
    }

    #[test]
    fn test_empty_routes() {
        let engine = RouterEngine::new(vec![]);
        let req = MatchRequest::new("/anything");
        assert!(engine.match_route(&req).is_err());
    }

    #[test]
    fn test_multiple_criteria_match() {
        let route = make_route("r1", "/api/*", "svc")
            .with_host("api.example.com")
            .with_header_match("X-Env", "prod");
        let engine = RouterEngine::new(vec![route]);

        let req = MatchRequest::new("/api/data")
            .with_host("api.example.com")
            .with_header("X-Env", "prod");
        assert!(engine.match_route(&req).is_ok());
    }

    #[test]
    fn test_routes_accessor() {
        let routes = vec![make_route("r1", "/a", "s1"), make_route("r2", "/b", "s2")];
        let engine = RouterEngine::new(routes);
        assert_eq!(engine.routes().len(), 2);
    }

    #[test]
    fn test_match_request_builder() {
        let req = MatchRequest::new("/api")
            .with_host("example.com")
            .with_header("X-Key", "value");
        assert_eq!(req.path, "/api");
        assert_eq!(req.host.as_deref(), Some("example.com"));
        assert_eq!(req.headers.get("X-Key").unwrap(), "value");
    }
}
