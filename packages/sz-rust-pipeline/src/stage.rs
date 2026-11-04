// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 流水线阶段定义（spec §5.8 规则 1）
//!
//! 阶段间通过消息传递，禁止共享可变状态。

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PipelineError;

/// 流水线阶段标识
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StageId(pub String);

impl StageId {
    /// 创建阶段标识
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// 获取标识字符串
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for StageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 流水线阶段定义
#[derive(Debug, Clone)]
pub struct StageDefinition {
    /// 阶段标识
    pub id: StageId,
    /// 依赖的前置阶段（无依赖则并行）
    pub dependencies: Vec<StageId>,
    /// 阶段超时（spec §6.8 规则 4）
    pub timeout: Duration,
}

impl StageDefinition {
    /// 创建阶段定义
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: StageId::new(id),
            dependencies: Vec::new(),
            timeout: Duration::from_secs(30),
        }
    }

    /// 添加依赖
    pub fn depends_on(mut self, dep: impl Into<String>) -> Self {
        self.dependencies.push(StageId::new(dep));
        self
    }

    /// 设置超时
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// 流水线阶段处理 trait
///
/// 阶段间通过消息传递，禁止共享可变状态。
#[async_trait]
pub trait StageHandler<I: Send + 'static, O: Send + 'static>: Send + Sync {
    /// 处理阶段输入，输出传递给下游
    async fn handle(&self, input: I) -> Result<O, PipelineError>;
}

/// 拓扑排序（Kahn 算法），检测环
pub fn topological_sort(stages: &[StageDefinition]) -> Result<Vec<StageId>, PipelineError> {
    use std::collections::{HashMap, HashSet, VecDeque};

    let mut in_degree: HashMap<StageId, usize> = HashMap::new();
    let mut graph: HashMap<StageId, Vec<StageId>> = HashMap::new();
    let mut all_ids: HashSet<StageId> = HashSet::new();

    for stage in stages {
        all_ids.insert(stage.id.clone());
        in_degree.entry(stage.id.clone()).or_insert(0);
        graph.entry(stage.id.clone()).or_default();
    }

    for stage in stages {
        for dep in &stage.dependencies {
            if !all_ids.contains(dep) {
                return Err(PipelineError::Config(format!(
                    "阶段 `{}` 依赖未知阶段 `{}`",
                    stage.id, dep
                )));
            }
            graph.entry(dep.clone()).or_default().push(stage.id.clone());
            *in_degree.entry(stage.id.clone()).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<StageId> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let mut sorted = Vec::new();
    while let Some(id) = queue.pop_front() {
        sorted.push(id.clone());
        if let Some(deps) = graph.get(&id) {
            for dep in deps {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }
    }

    if sorted.len() != stages.len() {
        let remaining: Vec<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg > 0)
            .map(|(id, _)| id.to_string())
            .collect();
        return Err(PipelineError::CyclicDependency(remaining.join(", ")));
    }

    Ok(sorted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_id() {
        let id = StageId::new("parse");
        assert_eq!(id.as_str(), "parse");
        assert_eq!(id.to_string(), "parse");
    }

    #[test]
    fn test_stage_definition_builder() {
        let stage = StageDefinition::new("route")
            .depends_on("parse")
            .with_timeout(Duration::from_secs(5));
        assert_eq!(stage.id, StageId::new("route"));
        assert_eq!(stage.dependencies, vec![StageId::new("parse")]);
        assert_eq!(stage.timeout, Duration::from_secs(5));
    }

    #[test]
    fn test_topological_sort_linear() {
        let stages = vec![
            StageDefinition::new("a"),
            StageDefinition::new("b").depends_on("a"),
            StageDefinition::new("c").depends_on("b"),
        ];
        let sorted = topological_sort(&stages).unwrap();
        assert_eq!(
            sorted,
            vec![StageId::new("a"), StageId::new("b"), StageId::new("c"),]
        );
    }

    #[test]
    fn test_topological_sort_parallel() {
        let stages = vec![
            StageDefinition::new("a"),
            StageDefinition::new("b"),
            StageDefinition::new("c").depends_on("a").depends_on("b"),
        ];
        let sorted = topological_sort(&stages).unwrap();
        assert_eq!(sorted[2], StageId::new("c"));
        assert!(sorted[0] == StageId::new("a") || sorted[0] == StageId::new("b"));
        assert!(sorted[1] == StageId::new("a") || sorted[1] == StageId::new("b"));
    }

    #[test]
    fn test_topological_sort_cycle() {
        let stages = vec![
            StageDefinition::new("a").depends_on("b"),
            StageDefinition::new("b").depends_on("a"),
        ];
        let result = topological_sort(&stages);
        assert!(matches!(result, Err(PipelineError::CyclicDependency(_))));
    }

    #[test]
    fn test_topological_sort_missing_dependency() {
        let stages = vec![StageDefinition::new("a").depends_on("nonexistent")];
        let result = topological_sort(&stages);
        assert!(matches!(result, Err(PipelineError::Config(_))));
    }
}
