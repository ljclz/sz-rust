// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::Path;

use crate::error::MigrationResult;
use crate::phase::HttpMethod;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6Route {
    pub method: HttpMethod,
    pub path_pattern: String,
    pub controller: String,
    pub action: String,
    pub middleware_chain: Vec<String>,
}

pub struct Tp6RouteParser {
    route_re: regex::Regex,
}

impl Tp6RouteParser {
    pub fn new() -> Self {
        Self {
            route_re: regex::Regex::new(
                r#"Route::(get|post|put|delete|patch)\s*\(\s*['"]([^'"]+)['"]\s*,\s*\[?([^,\]]+)::class\s*,\s*['"]([^'"]+)['"]\]?\s*\)"#,
            ).expect("route regex"),
        }
    }

    pub async fn parse(&self, route_dir: &Path) -> MigrationResult<Vec<Tp6Route>> {
        let mut routes = Vec::new();
        if !route_dir.exists() {
            return Ok(routes);
        }
        let mut entries = tokio::fs::read_dir(route_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "php") {
                let content = tokio::fs::read_to_string(&path).await?;
                routes.extend(self.parse_content(&content));
            }
        }
        Ok(routes)
    }

    fn parse_content(&self, content: &str) -> Vec<Tp6Route> {
        let mut routes = Vec::new();
        for caps in self.route_re.captures_iter(content) {
            let method = match caps.get(1).map(|m| m.as_str()) {
                Some("get") => HttpMethod::Get,
                Some("post") => HttpMethod::Post,
                Some("put") => HttpMethod::Put,
                Some("delete") => HttpMethod::Delete,
                Some("patch") => HttpMethod::Patch,
                _ => continue,
            };
            routes.push(Tp6Route {
                method,
                path_pattern: caps
                    .get(2)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                controller: caps
                    .get(3)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                action: caps
                    .get(4)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                middleware_chain: Vec::new(),
            });
        }
        routes
    }
}

impl Default for Tp6RouteParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_route() {
        let parser = Tp6RouteParser::new();
        let content = r#"Route::get('user/info', [UserController::class, 'info']);"#;
        let routes = parser.parse_content(content);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].method, HttpMethod::Get);
        assert_eq!(routes[0].path_pattern, "user/info");
        assert_eq!(routes[0].controller, "UserController");
        assert_eq!(routes[0].action, "info");
    }

    #[test]
    fn parse_post_route() {
        let parser = Tp6RouteParser::new();
        let content = r#"Route::post('user/login', [AuthController::class, 'login']);"#;
        let routes = parser.parse_content(content);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].method, HttpMethod::Post);
    }

    #[test]
    fn parse_multiple_routes() {
        let parser = Tp6RouteParser::new();
        let content = r#"
Route::get('user/list', [UserController::class, 'list']);
Route::post('user/create', [UserController::class, 'create']);
Route::delete('user/delete', [UserController::class, 'delete']);
"#;
        let routes = parser.parse_content(content);
        assert_eq!(routes.len(), 3);
    }

    #[test]
    fn parse_no_routes() {
        let parser = Tp6RouteParser::new();
        let content = "echo 'hello';";
        let routes = parser.parse_content(content);
        assert!(routes.is_empty());
    }
}
