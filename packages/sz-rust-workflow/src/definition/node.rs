// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use serde::{Deserialize, Serialize};

use super::strategy::{ApprovalStrategyType, CandidateStrategy, FaultStrategy};

/// 节点类型枚举，对齐 spec 6.3.2。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Start,
    Approval,
    Condition,
    Parallel,
    Plugin,
    End,
}

/// 节点定义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// 节点 ID，流程定义内唯一
    pub node_id: String,
    /// 节点类型
    pub node_type: NodeType,
    /// 节点配置（按类型不同）
    #[serde(flatten)]
    pub config: NodeConfig,
}

/// 节点配置枚举，按 [`NodeType`] 区分。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeConfig {
    Start {
        next: String,
    },
    Approval {
        approval_strategy: ApprovalStrategyType,
        candidate_strategy: CandidateStrategy,
        next: String,
    },
    Condition {
        branches: Vec<ConditionBranch>,
    },
    Parallel {
        branches: Vec<String>,
        join_node: String,
    },
    Plugin {
        /// 能力名，符合 `{plugin_name}.{capability_name}` 命名规范
        capability_name: String,
        /// 能力版本范围（semver）
        #[serde(default = "default_version_range")]
        capability_version_range: String,
        /// 参数映射（键为能力参数名，值为上下文路径或常量）
        #[serde(default)]
        args_mapping: serde_json::Value,
        /// 容错策略
        #[serde(default)]
        fault_strategy: FaultStrategy,
        /// 输出 Schema（JSON Schema，校验能力返回值）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_schema: Option<serde_json::Value>,
        /// 下一节点
        next: String,
    },
    End,
}

fn default_version_range() -> String {
    "*".to_string()
}

/// 条件分支，用于 [`NodeConfig::Condition`]。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionBranch {
    /// 守卫条件表达式
    pub condition: String,
    /// 满足条件时跳转的节点 ID
    pub next: String,
}

/// 节点边，用于 [`super::models::ApprovalFlowDefinition`]。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeEdge {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

impl Node {
    /// 获取节点的所有后继节点 ID（用于可达性分析）。
    pub fn successors(&self) -> Vec<&str> {
        match &self.config {
            NodeConfig::Start { next } => vec![next.as_str()],
            NodeConfig::Approval { next, .. } => vec![next.as_str()],
            NodeConfig::Condition { branches } => {
                branches.iter().map(|b| b.next.as_str()).collect()
            }
            NodeConfig::Parallel {
                branches,
                join_node,
            } => {
                let mut v: Vec<&str> = branches.iter().map(|s| s.as_str()).collect();
                v.push(join_node.as_str());
                v
            }
            NodeConfig::Plugin { next, .. } => vec![next.as_str()],
            NodeConfig::End => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_node_serde() {
        let n = Node {
            node_id: "start".into(),
            node_type: NodeType::Start,
            config: NodeConfig::Start { next: "n1".into() },
        };
        let json = serde_json::to_string(&n).unwrap();
        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn approval_node_serde() {
        let n = Node {
            node_id: "approve1".into(),
            node_type: NodeType::Approval,
            config: NodeConfig::Approval {
                approval_strategy: ApprovalStrategyType::OrSign,
                candidate_strategy: CandidateStrategy::Static {
                    users: vec!["u1".into(), "u2".into()],
                    roles: vec![],
                },
                next: "end".into(),
            },
        };
        let json = serde_json::to_string(&n).unwrap();
        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn plugin_node_serde() {
        let n = Node {
            node_id: "p1".into(),
            node_type: NodeType::Plugin,
            config: NodeConfig::Plugin {
                capability_name: "crm.search_customer".into(),
                capability_version_range: "^1.0".into(),
                args_mapping: serde_json::json!({"keyword": "$.keyword"}),
                fault_strategy: FaultStrategy::Retry,
                output_schema: None,
                next: "end".into(),
            },
        };
        let json = serde_json::to_string(&n).unwrap();
        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn condition_node_successors() {
        let n = Node {
            node_id: "c1".into(),
            node_type: NodeType::Condition,
            config: NodeConfig::Condition {
                branches: vec![
                    ConditionBranch {
                        condition: "$.x > 0".into(),
                        next: "a".into(),
                    },
                    ConditionBranch {
                        condition: "$.x <= 0".into(),
                        next: "b".into(),
                    },
                ],
            },
        };
        assert_eq!(n.successors(), vec!["a", "b"]);
    }

    #[test]
    fn parallel_node_successors() {
        let n = Node {
            node_id: "par1".into(),
            node_type: NodeType::Parallel,
            config: NodeConfig::Parallel {
                branches: vec!["b1".into(), "b2".into()],
                join_node: "join".into(),
            },
        };
        assert_eq!(n.successors(), vec!["b1", "b2", "join"]);
    }

    #[test]
    fn end_node_no_successors() {
        let n = Node {
            node_id: "end".into(),
            node_type: NodeType::End,
            config: NodeConfig::End,
        };
        assert!(n.successors().is_empty());
    }
}
