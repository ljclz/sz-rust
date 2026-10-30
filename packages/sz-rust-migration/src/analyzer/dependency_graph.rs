// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::{HashMap, HashSet};

use crate::analyzer::controller_parser::Tp6Controller;
use crate::analyzer::middleware_parser::Tp6Middleware;
use crate::analyzer::model_parser::Tp6Model;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DependencyGraph {
    pub nodes: Vec<DependencyNode>,
    pub edges: Vec<DependencyEdge>,
    pub topological_order: Vec<String>,
    pub has_cycle: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DependencyNode {
    pub id: String,
    pub node_type: NodeType,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Controller,
    Model,
    Middleware,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DependencyEdge {
    pub from: String,
    pub to: String,
    pub edge_type: EdgeType,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    ControllerToModel,
    ModelToModel,
    ControllerToMiddleware,
}

pub struct DependencyGraphBuilder;

impl DependencyGraphBuilder {
    pub fn build(
        controllers: &[Tp6Controller],
        models: &[Tp6Model],
        middlewares: &[Tp6Middleware],
    ) -> DependencyGraph {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut adj: HashMap<String, HashSet<String>> = HashMap::new();

        for c in controllers {
            nodes.push(DependencyNode {
                id: c.class_name.clone(),
                node_type: NodeType::Controller,
            });
            adj.entry(c.class_name.clone()).or_default();
        }
        for m in models {
            nodes.push(DependencyNode {
                id: m.class_name.clone(),
                node_type: NodeType::Model,
            });
            adj.entry(m.class_name.clone()).or_default();
        }
        for mw in middlewares {
            nodes.push(DependencyNode {
                id: mw.class_name.clone(),
                node_type: NodeType::Middleware,
            });
            adj.entry(mw.class_name.clone()).or_default();
        }

        for m in models {
            for rel in &m.relations {
                edges.push(DependencyEdge {
                    from: m.class_name.clone(),
                    to: rel.related_model.clone(),
                    edge_type: EdgeType::ModelToModel,
                });
                adj.entry(m.class_name.clone())
                    .or_default()
                    .insert(rel.related_model.clone());
            }
        }

        for c in controllers {
            let model_name = c.class_name.trim_end_matches("Controller").to_string();
            if models.iter().any(|m| m.class_name == model_name) {
                edges.push(DependencyEdge {
                    from: c.class_name.clone(),
                    to: model_name.clone(),
                    edge_type: EdgeType::ControllerToModel,
                });
                adj.entry(c.class_name.clone())
                    .or_default()
                    .insert(model_name);
            }
            for mw in &c.middleware_applied {
                edges.push(DependencyEdge {
                    from: c.class_name.clone(),
                    to: mw.clone(),
                    edge_type: EdgeType::ControllerToMiddleware,
                });
                adj.entry(c.class_name.clone())
                    .or_default()
                    .insert(mw.clone());
            }
        }

        let (topological_order, has_cycle) = kahn_topological_sort(&nodes, &adj);

        DependencyGraph {
            nodes,
            edges,
            topological_order,
            has_cycle,
        }
    }
}

fn kahn_topological_sort(
    nodes: &[DependencyNode],
    adj: &HashMap<String, HashSet<String>>,
) -> (Vec<String>, bool) {
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    for n in nodes {
        in_degree.entry(n.id.clone()).or_insert(0);
    }
    for deps in adj.values() {
        for dep in deps {
            *in_degree.entry(dep.clone()).or_insert(0) += 1;
        }
    }

    let mut queue: Vec<String> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(k, _)| k.clone())
        .collect();
    queue.sort();

    let mut result = Vec::new();
    while let Some(node) = queue.pop() {
        result.push(node.clone());
        if let Some(deps) = adj.get(&node) {
            for dep in deps {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push(dep.clone());
                        queue.sort();
                    }
                }
            }
        }
    }

    let has_cycle = result.len() < nodes.len();
    (result, has_cycle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph() {
        let graph = DependencyGraphBuilder::build(&[], &[], &[]);
        assert!(graph.nodes.is_empty());
        assert!(!graph.has_cycle);
    }

    #[test]
    fn simple_dependency() {
        let controllers = vec![Tp6Controller {
            class_name: "UserController".into(),
            actions: vec![],
            middleware_applied: vec![],
            dependencies: vec![],
        }];
        let models = vec![Tp6Model {
            class_name: "User".into(),
            table_name: "users".into(),
            fields: vec![],
            relations: vec![],
            validation_rules: vec![],
        }];
        let graph = DependencyGraphBuilder::build(&controllers, &models, &[]);
        assert_eq!(graph.nodes.len(), 2);
        assert!(graph
            .edges
            .iter()
            .any(|e| e.from == "UserController" && e.to == "User"));
        assert!(!graph.has_cycle);
    }
}
