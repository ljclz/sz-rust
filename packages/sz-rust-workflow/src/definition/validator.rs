// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{WorkflowErrorCode, WorkflowResult};

use super::models::FlowDefinition;
use super::node::{NodeConfig, NodeType};

/// 插件可用性检查 trait（解耦 AddonLoader 具体类型）。
#[async_trait]
pub trait PluginChecker: Send + Sync + 'static {
    /// 检查插件是否已启用。
    async fn is_plugin_enabled(&self, plugin_name: &str) -> bool;
}

/// Noop 实现：所有插件均视为可用（测试用）。
pub struct NoopPluginChecker;

#[async_trait]
impl PluginChecker for NoopPluginChecker {
    async fn is_plugin_enabled(&self, _plugin_name: &str) -> bool {
        true
    }
}

/// 为 `sz_rust_addons_loader::AddonLoader` 实现 `PluginChecker`。
#[async_trait]
impl PluginChecker for sz_rust_addons_loader::AddonLoader {
    async fn is_plugin_enabled(&self, plugin_name: &str) -> bool {
        self.is_enabled(plugin_name).unwrap_or(false)
    }
}

/// 校验问题严重级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    Error,
    Warning,
}

/// 校验问题。
#[derive(Debug, Clone, Serialize)]
pub struct ValidationIssue {
    pub code: WorkflowErrorCode,
    pub severity: IssueSeverity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

impl ValidationIssue {
    fn error(code: WorkflowErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: IssueSeverity::Error,
            message: message.into(),
            node_id: None,
        }
    }

    fn error_at(
        code: WorkflowErrorCode,
        message: impl Into<String>,
        node_id: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: IssueSeverity::Error,
            message: message.into(),
            node_id: Some(node_id.into()),
        }
    }

    fn warning(
        code: WorkflowErrorCode,
        message: impl Into<String>,
        node_id: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: IssueSeverity::Warning,
            message: message.into(),
            node_id: Some(node_id.into()),
        }
    }
}

/// 流程定义校验器。
pub struct DefinitionValidator {
    plugin_checker: Arc<dyn PluginChecker>,
}

impl DefinitionValidator {
    pub fn new(plugin_checker: Arc<dyn PluginChecker>) -> Self {
        Self { plugin_checker }
    }

    /// 使用 noop checker 构造（不校验插件可用性）。
    pub fn new_noop() -> Self {
        Self::new(Arc::new(NoopPluginChecker))
    }

    /// 执行全部校验，返回所有 issue（Error 与 Warning）。
    ///
    /// 调用方根据是否存在 `IssueSeverity::Error` 决定是否接受定义。
    pub async fn validate(&self, def: &FlowDefinition) -> WorkflowResult<Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        issues.extend(self.validate_structure(def));
        issues.extend(self.validate_reachability(def));
        issues.extend(self.validate_termination(def));
        issues.extend(self.validate_plugin_refs(def).await?);
        Ok(issues)
    }

    /// 2.5 结构完整性校验。
    fn validate_structure(&self, def: &FlowDefinition) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        let mut id_set: HashSet<&str> = HashSet::new();

        for n in &def.nodes {
            if !id_set.insert(n.node_id.as_str()) {
                issues.push(ValidationIssue::error_at(
                    WorkflowErrorCode::StructureIncomplete,
                    format!("node_id 重复：{}", n.node_id),
                    &n.node_id,
                ));
            }
        }

        let has_start = def.nodes.iter().any(|n| n.node_type == NodeType::Start);
        let has_end = def.nodes.iter().any(|n| n.node_type == NodeType::End);
        if !has_start {
            issues.push(ValidationIssue::error(
                WorkflowErrorCode::StructureIncomplete,
                "缺少 start 节点",
            ));
        }
        if !has_end {
            issues.push(ValidationIssue::error(
                WorkflowErrorCode::StructureIncomplete,
                "缺少 end 节点",
            ));
        }

        if !id_set.contains(def.start_node.as_str()) {
            issues.push(ValidationIssue::error(
                WorkflowErrorCode::StructureIncomplete,
                format!("start_node 引用不存在的节点：{}", def.start_node),
            ));
        }

        issues
    }

    /// 2.6 可达性校验（BFS，识别不可达节点，Warning 级）。
    fn validate_reachability(&self, def: &FlowDefinition) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        let node_map: HashMap<&str, &super::node::Node> =
            def.nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();

        let mut reachable: HashSet<&str> = HashSet::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        if node_map.contains_key(def.start_node.as_str()) {
            queue.push_back(def.start_node.as_str());
            reachable.insert(def.start_node.as_str());
        }
        while let Some(cur) = queue.pop_front() {
            if let Some(node) = node_map.get(cur) {
                for succ in node.successors() {
                    if node_map.contains_key(succ) && reachable.insert(succ) {
                        queue.push_back(succ);
                    }
                }
            }
        }

        for n in &def.nodes {
            if !reachable.contains(n.node_id.as_str()) {
                issues.push(ValidationIssue::warning(
                    WorkflowErrorCode::UnreachableNode,
                    format!("节点不可达：{}", n.node_id),
                    &n.node_id,
                ));
            }
        }
        issues
    }

    /// 2.6 终止性校验（所有可达节点须能到达某 end 节点）。
    fn validate_termination(&self, def: &FlowDefinition) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        let node_map: HashMap<&str, &super::node::Node> =
            def.nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();
        let end_nodes: HashSet<&str> = def
            .nodes
            .iter()
            .filter(|n| n.node_type == NodeType::End)
            .map(|n| n.node_id.as_str())
            .collect();

        for n in &def.nodes {
            if end_nodes.contains(n.node_id.as_str()) {
                continue;
            }
            if !self.can_reach_end(n.node_id.as_str(), &node_map, &end_nodes) {
                issues.push(ValidationIssue::error_at(
                    WorkflowErrorCode::CannotTerminate,
                    format!("节点无法到达 end 节点：{}", n.node_id),
                    &n.node_id,
                ));
            }
        }
        issues
    }

    fn can_reach_end(
        &self,
        start: &str,
        node_map: &HashMap<&str, &super::node::Node>,
        end_nodes: &HashSet<&str>,
    ) -> bool {
        let mut visited: HashSet<&str> = HashSet::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        queue.push_back(start);
        visited.insert(start);
        while let Some(cur) = queue.pop_front() {
            if end_nodes.contains(cur) {
                return true;
            }
            if let Some(node) = node_map.get(cur) {
                for succ in node.successors() {
                    if visited.insert(succ) {
                        queue.push_back(succ);
                    }
                }
            }
        }
        false
    }

    /// 2.7 插件节点引用校验。
    async fn validate_plugin_refs(
        &self,
        def: &FlowDefinition,
    ) -> WorkflowResult<Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        for n in &def.nodes {
            if let NodeConfig::Plugin {
                capability_name,
                capability_version_range,
                ..
            } = &n.config
            {
                let parts: Vec<&str> = capability_name.splitn(2, '.').collect();
                if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
                    issues.push(ValidationIssue::error_at(
                        WorkflowErrorCode::PluginUnavailable,
                        format!("capability_name 不符合 {{plugin}}.{{capability}} 命名规范：{capability_name}"),
                        &n.node_id,
                    ));
                    continue;
                }
                let plugin_name = parts[0];
                if !self.plugin_checker.is_plugin_enabled(plugin_name).await {
                    issues.push(ValidationIssue::error_at(
                        WorkflowErrorCode::PluginUnavailable,
                        format!("插件未启用：{plugin_name}"),
                        &n.node_id,
                    ));
                }
                if let Err(e) = semver::VersionReq::parse(capability_version_range) {
                    issues.push(ValidationIssue::error_at(
                        WorkflowErrorCode::PluginUnavailable,
                        format!("非法版本范围：{capability_version_range} ({e})"),
                        &n.node_id,
                    ));
                }
            }
        }
        Ok(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::super::models::{DefinitionFormat, FlowDefinition};
    use super::*;

    use super::super::parser::DefinitionParser;

    fn parse(yaml: &str) -> FlowDefinition {
        DefinitionParser::new()
            .parse(yaml, DefinitionFormat::Yaml)
            .unwrap()
    }

    const VALID_DEF: &str = r#"
flow_key: leave_req
version: "1.0.0"
name: 请假
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
"#;

    #[tokio::test]
    async fn valid_definition_no_errors() {
        let def = parse(VALID_DEF);
        let v = DefinitionValidator::new_noop();
        let issues = v.validate(&def).await.unwrap();
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Error)
            .collect();
        assert!(errors.is_empty(), "不应有 Error 级 issue: {:?}", issues);
    }

    #[tokio::test]
    async fn missing_start_node() {
        let yaml = r#"
flow_key: leave_req
version: "1.0.0"
name: 请假
nodes:
  - node_id: end
    node_type: end
    kind: end
start_node: end
"#;
        let def = parse(yaml);
        let v = DefinitionValidator::new_noop();
        let issues = v.validate(&def).await.unwrap();
        assert!(issues.iter().any(
            |i| i.code == WorkflowErrorCode::StructureIncomplete && i.message.contains("start")
        ));
    }

    #[tokio::test]
    async fn duplicate_node_id() {
        let yaml = r#"
flow_key: leave_req
version: "1.0.0"
name: 请假
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
"#;
        let def = parse(yaml);
        let v = DefinitionValidator::new_noop();
        let issues = v.validate(&def).await.unwrap();
        assert!(issues.iter().any(
            |i| i.code == WorkflowErrorCode::StructureIncomplete && i.message.contains("重复")
        ));
    }

    #[tokio::test]
    async fn unreachable_node_warning() {
        let yaml = r#"
flow_key: leave_req
version: "1.0.0"
name: 请假
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: orphan
    node_type: approval
    kind: approval
    approval_strategy: and_sign
    candidate_strategy:
      type: static
      users: ["u1"]
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
"#;
        let def = parse(yaml);
        let v = DefinitionValidator::new_noop();
        let issues = v.validate(&def).await.unwrap();
        assert!(issues
            .iter()
            .any(|i| i.code == WorkflowErrorCode::UnreachableNode
                && i.severity == IssueSeverity::Warning));
    }

    #[tokio::test]
    async fn cannot_terminate() {
        let yaml = r#"
flow_key: leave_req
version: "1.0.0"
name: 请假
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: loop
  - node_id: loop
    node_type: approval
    kind: approval
    approval_strategy: and_sign
    candidate_strategy:
      type: static
      users: ["u1"]
    next: loop
start_node: start
"#;
        let def = parse(yaml);
        let v = DefinitionValidator::new_noop();
        let issues = v.validate(&def).await.unwrap();
        assert!(issues
            .iter()
            .any(|i| i.code == WorkflowErrorCode::CannotTerminate));
    }

    #[tokio::test]
    async fn plugin_bad_capability_name() {
        let yaml = r#"
flow_key: leave_req
version: "1.0.0"
name: 请假
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: p1
  - node_id: p1
    node_type: plugin
    kind: plugin
    capability_name: invalid_no_dot
    capability_version_range: "*"
    fault_strategy: fail
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
"#;
        let def = parse(yaml);
        let v = DefinitionValidator::new_noop();
        let issues = v.validate(&def).await.unwrap();
        assert!(issues
            .iter()
            .any(|i| i.code == WorkflowErrorCode::PluginUnavailable
                && i.message.contains("命名规范")));
    }

    #[tokio::test]
    async fn plugin_bad_version_range() {
        let yaml = r#"
flow_key: leave_req
version: "1.0.0"
name: 请假
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: p1
  - node_id: p1
    node_type: plugin
    kind: plugin
    capability_name: crm.search
    capability_version_range: "not_a_valid_range!!!"
    fault_strategy: fail
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
"#;
        let def = parse(yaml);
        let v = DefinitionValidator::new_noop();
        let issues = v.validate(&def).await.unwrap();
        assert!(issues
            .iter()
            .any(|i| i.code == WorkflowErrorCode::PluginUnavailable
                && i.message.contains("版本范围")));
    }

    struct DisabledChecker;
    #[async_trait]
    impl PluginChecker for DisabledChecker {
        async fn is_plugin_enabled(&self, _: &str) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn plugin_not_enabled() {
        let yaml = r#"
flow_key: leave_req
version: "1.0.0"
name: 请假
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: p1
  - node_id: p1
    node_type: plugin
    kind: plugin
    capability_name: crm.search
    capability_version_range: "^1.0"
    fault_strategy: fail
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
"#;
        let def = parse(yaml);
        let v = DefinitionValidator::new(Arc::new(DisabledChecker));
        let issues = v.validate(&def).await.unwrap();
        assert!(issues.iter().any(
            |i| i.code == WorkflowErrorCode::PluginUnavailable && i.message.contains("未启用")
        ));
    }
}
