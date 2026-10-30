// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use crate::analyzer::route_parser::Tp6Route;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SddTask {
    pub task_id: String,
    pub description: String,
    pub interface_id: String,
    pub source_route: String,
    pub target_handler: String,
}

pub struct TaskBuilder;

impl TaskBuilder {
    pub fn build_task(route: &Tp6Route) -> SddTask {
        let interface_id = format!("{}::{}", route.controller, route.action);
        SddTask {
            task_id: uuid::Uuid::new_v4().to_string(),
            description: format!(
                "Migrate route {} {} → {}::{}",
                route.method, route.path_pattern, route.controller, route.action
            ),
            interface_id,
            source_route: format!("{} {}", route.method, route.path_pattern),
            target_handler: format!("{}_handler", route.action),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase::HttpMethod;

    #[test]
    fn build_task_from_route() {
        let route = Tp6Route {
            method: HttpMethod::Get,
            path_pattern: "user/info".into(),
            controller: "UserController".into(),
            action: "info".into(),
            middleware_chain: vec![],
        };
        let task = TaskBuilder::build_task(&route);
        assert_eq!(task.interface_id, "UserController::info");
        assert_eq!(task.source_route, "GET user/info");
        assert_eq!(task.target_handler, "info_handler");
    }
}
