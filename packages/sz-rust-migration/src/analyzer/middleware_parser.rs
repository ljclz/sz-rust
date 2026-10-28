// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::Path;

use crate::error::MigrationResult;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6Middleware {
    pub class_name: String,
    pub execution_order: MiddlewareOrder,
    pub scope: MiddlewareScope,
    pub only_actions: Vec<String>,
    pub except_actions: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MiddlewareOrder {
    Before,
    After,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MiddlewareScope {
    Global,
    Route,
    Controller,
}

pub struct Tp6MiddlewareParser {
    class_re: regex::Regex,
    only_re: regex::Regex,
    except_re: regex::Regex,
}

impl Tp6MiddlewareParser {
    pub fn new() -> Self {
        Self {
            class_re: regex::Regex::new(r"class\s+(\w+)\s+extends\s+Middleware")
                .expect("class regex"),
            only_re: regex::Regex::new(r#"\$only\s*=\s*\[([^\]]*)\]"#).expect("only regex"),
            except_re: regex::Regex::new(r#"\$except\s*=\s*\[([^\]]*)\]"#).expect("except regex"),
        }
    }

    pub async fn parse(&self, middleware_dir: &Path) -> MigrationResult<Vec<Tp6Middleware>> {
        let mut middlewares = Vec::new();
        if !middleware_dir.exists() {
            return Ok(middlewares);
        }
        let mut entries = tokio::fs::read_dir(middleware_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "php") {
                let content = tokio::fs::read_to_string(&path).await?;
                if let Some(middleware) = self.parse_content(&content) {
                    middlewares.push(middleware);
                }
            }
        }
        Ok(middlewares)
    }

    fn parse_content(&self, content: &str) -> Option<Tp6Middleware> {
        let class_caps = self.class_re.captures(content)?;
        let class_name = class_caps.get(1)?.as_str().to_string();
        let only_actions = self
            .only_re
            .captures(content)
            .map(|c| self.parse_string_list(c.get(1).map(|m| m.as_str()).unwrap_or("")))
            .unwrap_or_default();
        let except_actions = self
            .except_re
            .captures(content)
            .map(|c| self.parse_string_list(c.get(1).map(|m| m.as_str()).unwrap_or("")))
            .unwrap_or_default();
        Some(Tp6Middleware {
            class_name,
            execution_order: MiddlewareOrder::Before,
            scope: MiddlewareScope::Route,
            only_actions,
            except_actions,
        })
    }

    fn parse_string_list(&self, list_str: &str) -> Vec<String> {
        let re = regex::Regex::new(r#"'([^']+)'"#).expect("string list regex");
        re.captures_iter(list_str)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect()
    }
}

impl Default for Tp6MiddlewareParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_middleware_with_scope() {
        let parser = Tp6MiddlewareParser::new();
        let content = r#"
<?php
namespace app\middleware;
class AuthMiddleware extends Middleware {
    protected $only = ['info', 'update'];
    protected $except = ['login'];
    public function handle($request, \Closure $next) {
        return $next($request);
    }
}
"#;
        let middleware = parser.parse_content(content).unwrap();
        assert_eq!(middleware.class_name, "AuthMiddleware");
        assert_eq!(middleware.only_actions, vec!["info", "update"]);
        assert_eq!(middleware.except_actions, vec!["login"]);
    }

    #[test]
    fn parse_non_middleware_returns_none() {
        let parser = Tp6MiddlewareParser::new();
        let content = "class Filter { }";
        assert!(parser.parse_content(content).is_none());
    }
}
