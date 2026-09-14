// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use serde::{Deserialize, Serialize};

use super::node::{Node, NodeEdge};
use super::strategy::Transition;

/// 定义格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefinitionFormat {
    Yaml,
    Json,
}

impl DefinitionFormat {
    /// 按首字符猜测格式：`{` → JSON，其他 → YAML。
    pub fn detect(text: &str) -> Self {
        let trimmed = text.trim_start();
        if trimmed.starts_with('{') {
            Self::Json
        } else {
            Self::Yaml
        }
    }
}

/// 流程定义，对齐 design 2.3.2 类图。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowDefinition {
    /// 流程标识，小写字母/数字/下划线/点号，长度 1～64
    pub flow_key: String,
    /// 语义化版本
    pub version: semver::Version,
    /// 流程名称，长度 1～128
    pub name: String,
    /// 节点集合
    pub nodes: Vec<Node>,
    /// 起始节点 ID
    pub start_node: String,
    /// 是否为生效版本
    #[serde(default)]
    pub active: bool,
    /// 是否已弃用
    #[serde(default)]
    pub deprecated: bool,
    /// 状态机定义（可选，纯状态机流程）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<StateMachineDefinition>,
    /// 审批流定义（可选，纯审批流）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<ApprovalFlowDefinition>,
}

/// 状态机定义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateMachineDefinition {
    /// 初始状态
    pub initial_state: String,
    /// 所有状态集合
    pub states: Vec<String>,
    /// 迁移规则
    pub transitions: Vec<Transition>,
}

/// 审批流定义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalFlowDefinition {
    /// 节点边集合（描述节点间连接关系）
    pub edges: Vec<NodeEdge>,
}

impl FlowDefinition {
    /// 按 node_id 查找节点。
    pub fn find_node(&self, node_id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.node_id == node_id)
    }

    /// 获取所有 node_id 集合。
    pub fn node_ids(&self) -> Vec<&str> {
        self.nodes.iter().map(|n| n.node_id.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::node::{NodeConfig, NodeType};
    use super::*;

    fn sample_definition() -> FlowDefinition {
        FlowDefinition {
            flow_key: "leave_request".into(),
            version: semver::Version::new(1, 0, 0),
            name: "请假申请".into(),
            nodes: vec![
                Node {
                    node_id: "start".into(),
                    node_type: NodeType::Start,
                    config: NodeConfig::Start { next: "end".into() },
                },
                Node {
                    node_id: "end".into(),
                    node_type: NodeType::End,
                    config: NodeConfig::End,
                },
            ],
            start_node: "start".into(),
            active: true,
            deprecated: false,
            machine: None,
            flow: None,
        }
    }

    #[test]
    fn flow_definition_serde() {
        let def = sample_definition();
        let json = serde_json::to_string(&def).unwrap();
        let back: FlowDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(def, back);
    }

    #[test]
    fn flow_definition_yaml_serde() {
        let def = sample_definition();
        let yaml = serde_yaml::to_string(&def).unwrap();
        let back: FlowDefinition = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(def, back);
    }

    #[test]
    fn format_detect() {
        assert_eq!(
            DefinitionFormat::detect("{ \"flow_key\": \"x\" }"),
            DefinitionFormat::Json
        );
        assert_eq!(
            DefinitionFormat::detect("flow_key: x"),
            DefinitionFormat::Yaml
        );
        assert_eq!(DefinitionFormat::detect("  {"), DefinitionFormat::Json);
    }

    #[test]
    fn find_node() {
        let def = sample_definition();
        assert!(def.find_node("start").is_some());
        assert!(def.find_node("nonexistent").is_none());
    }

    #[test]
    fn node_ids() {
        let def = sample_definition();
        assert_eq!(def.node_ids(), vec!["start", "end"]);
    }

    #[test]
    fn state_machine_definition_serde() {
        let sm = StateMachineDefinition {
            initial_state: "draft".into(),
            states: vec!["draft".into(), "review".into(), "approved".into()],
            transitions: vec![Transition {
                from: "draft".into(),
                to: "review".into(),
                event: "submit".into(),
                guard: None,
            }],
        };
        let json = serde_json::to_string(&sm).unwrap();
        let back: StateMachineDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(sm, back);
    }

    #[test]
    fn approval_flow_definition_serde() {
        let af = ApprovalFlowDefinition {
            edges: vec![NodeEdge {
                from: "start".into(),
                to: "approve".into(),
                condition: None,
            }],
        };
        let json = serde_json::to_string(&af).unwrap();
        let back: ApprovalFlowDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(af, back);
    }

    #[test]
    fn full_definition_with_machine_and_flow() {
        let mut def = sample_definition();
        def.machine = Some(StateMachineDefinition {
            initial_state: "draft".into(),
            states: vec!["draft".into(), "done".into()],
            transitions: vec![],
        });
        def.flow = Some(ApprovalFlowDefinition { edges: vec![] });
        let json = serde_json::to_string(&def).unwrap();
        let back: FlowDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(def, back);
    }
}
