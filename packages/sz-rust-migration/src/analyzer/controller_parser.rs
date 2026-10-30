// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::Path;

use crate::error::MigrationResult;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6Controller {
    pub class_name: String,
    pub actions: Vec<Tp6Action>,
    pub middleware_applied: Vec<String>,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6Action {
    pub name: String,
    pub params: Vec<Tp6Param>,
    pub return_type: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6Param {
    pub name: String,
    pub param_type: Option<String>,
    pub default_value: Option<String>,
}

pub struct Tp6ControllerParser {
    class_re: regex::Regex,
    method_re: regex::Regex,
    param_re: regex::Regex,
}

impl Tp6ControllerParser {
    pub fn new() -> Self {
        Self {
            class_re: regex::Regex::new(r"class\s+(\w+)\s+extends\s+Controller")
                .expect("class regex"),
            method_re: regex::Regex::new(r"public\s+function\s+(\w+)\s*\(([^)]*)\)")
                .expect("method regex"),
            param_re: regex::Regex::new(r#"(?:(\w+)\s+)?\$(\w+)(?:\s*=\s*([^,]+))?"#)
                .expect("param regex"),
        }
    }

    pub async fn parse(&self, controller_dir: &Path) -> MigrationResult<Vec<Tp6Controller>> {
        let mut controllers = Vec::new();
        if !controller_dir.exists() {
            return Ok(controllers);
        }
        let mut entries = tokio::fs::read_dir(controller_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "php") {
                let content = tokio::fs::read_to_string(&path).await?;
                if let Some(controller) = self.parse_content(&content) {
                    controllers.push(controller);
                }
            }
        }
        Ok(controllers)
    }

    fn parse_content(&self, content: &str) -> Option<Tp6Controller> {
        let class_caps = self.class_re.captures(content)?;
        let class_name = class_caps.get(1)?.as_str().to_string();
        let mut actions = Vec::new();
        for caps in self.method_re.captures_iter(content) {
            let method_name = caps
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            if method_name == "__construct" {
                continue;
            }
            let params_str = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let params = self.parse_params(params_str);
            actions.push(Tp6Action {
                name: method_name,
                params,
                return_type: None,
            });
        }
        Some(Tp6Controller {
            class_name,
            actions,
            middleware_applied: Vec::new(),
            dependencies: Vec::new(),
        })
    }

    fn parse_params(&self, params_str: &str) -> Vec<Tp6Param> {
        let mut params = Vec::new();
        for caps in self.param_re.captures_iter(params_str) {
            let name = caps
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            params.push(Tp6Param {
                name,
                param_type: caps.get(1).map(|m| m.as_str().to_string()),
                default_value: caps.get(3).map(|m| m.as_str().trim().to_string()),
            });
        }
        params
    }
}

impl Default for Tp6ControllerParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_controller_with_methods() {
        let parser = Tp6ControllerParser::new();
        let content = r#"
<?php
namespace app\controller;
use think\facade\Db;
class UserController extends Controller {
    public function info($id) {
        return Db::name('users')->find($id);
    }
    public function create($name, $email = '') {
        return Db::name('users')->insert(['name' => $name, 'email' => $email]);
    }
}
"#;
        let controller = parser.parse_content(content).unwrap();
        assert_eq!(controller.class_name, "UserController");
        assert_eq!(controller.actions.len(), 2);
        assert_eq!(controller.actions[0].name, "info");
        assert_eq!(controller.actions[0].params.len(), 1);
        assert_eq!(controller.actions[0].params[0].name, "id");
        assert_eq!(controller.actions[1].name, "create");
        assert_eq!(controller.actions[1].params.len(), 2);
        assert_eq!(controller.actions[1].params[1].name, "email");
        assert!(controller.actions[1].params[1].default_value.is_some());
    }

    #[test]
    fn parse_non_controller_returns_none() {
        let parser = Tp6ControllerParser::new();
        let content = "class Helper { }";
        assert!(parser.parse_content(content).is_none());
    }
}
