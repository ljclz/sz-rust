// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::HashMap;

use crate::config::MappingConfig;

pub struct MigrationMappingTable {
    route_mapping: HashMap<String, String>,
    model_mapping: HashMap<String, String>,
    middleware_mapping: HashMap<String, String>,
}

impl MigrationMappingTable {
    pub fn from_config(config: &MappingConfig) -> Self {
        Self {
            route_mapping: config.route_mapping.clone(),
            model_mapping: config.model_mapping.clone(),
            middleware_mapping: config.middleware_mapping.clone(),
        }
    }

    pub fn map_route(&self, tp6_route: &str) -> String {
        self.route_mapping
            .get(tp6_route)
            .cloned()
            .unwrap_or_else(|| default_route_mapping(tp6_route))
    }

    pub fn map_model(&self, tp6_model: &str) -> String {
        self.model_mapping
            .get(tp6_model)
            .cloned()
            .unwrap_or_else(|| default_model_mapping(tp6_model))
    }

    pub fn map_middleware(&self, tp6_middleware: &str) -> String {
        self.middleware_mapping
            .get(tp6_middleware)
            .cloned()
            .unwrap_or_else(|| default_middleware_mapping(tp6_middleware))
    }
}

fn default_route_mapping(tp6_route: &str) -> String {
    format!("/{tp6_route}")
}

fn default_model_mapping(tp6_model: &str) -> String {
    tp6_model.to_string()
}

fn default_middleware_mapping(tp6_middleware: &str) -> String {
    tp6_middleware.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mappings() {
        let config = MappingConfig::default();
        let table = MigrationMappingTable::from_config(&config);
        assert_eq!(table.map_route("user/info"), "/user/info");
        assert_eq!(table.map_model("User"), "User");
        assert_eq!(table.map_middleware("Auth"), "Auth");
    }

    #[test]
    fn custom_mappings() {
        let mut config = MappingConfig::default();
        config
            .route_mapping
            .insert("user/info".into(), "/api/v1/user".into());
        let table = MigrationMappingTable::from_config(&config);
        assert_eq!(table.map_route("user/info"), "/api/v1/user");
    }
}
