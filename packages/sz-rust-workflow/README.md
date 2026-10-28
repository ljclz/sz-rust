# sz-rust-workflow

SZ-Rust 工作流引擎 — 状态机/审批流/插件节点编排，含设计器 API 与可观测层。

## 功能概述

- **状态机引擎**：YAML/JSON 定义状态与迁移，守卫条件求值，乐观锁并发控制
- **审批流引擎**：会签/或签策略，候选人解析（静态/动态/能力），任务办理（审批/驳回/转办/加签/撤回）
- **插件节点集成**：能力调用与超时，容错策略（fail/skip/retry），敏感字段脱敏，卸载联动
- **流程实例管理**：启动/挂起/恢复/终止/查询/历史轨迹
- **设计器 API**：定义导入/导出/校验，版本管理（生效/弃用）
- **可观测层**：事件总线（broadcast channel），Prometheus 指标，结构化审计日志

## 核心概念

| 概念 | 说明 |
|------|------|
| `FlowDefinition` | 流程定义（节点/状态机/审批流） |
| `FlowInstance` | 流程实例（运行态） |
| `Task` | 审批任务（待办） |
| `StateMachineEngine` | 状态机引擎（事件触发迁移） |
| `ApprovalFlowEngine` | 审批流引擎（任务办理推进） |
| `PluginNodeExecutor` | 插件节点执行器（能力调用） |
| `WorkflowEngine` | 统一门面 |

## 快速开始

```rust
use sz_rust_workflow::{WorkflowEngine, WorkflowConfig, WorkflowDeps, DefinitionFormat};

#[tokio::main]
async fn main() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());

    // 导入定义
    let yaml = r#"
flow_key: leave_request
version: "1.0.0"
name: 请假申请
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
active: true
"#;
    engine.import_definition(yaml, DefinitionFormat::Yaml).await.unwrap();

    // 启动实例
    let summary = engine
        .start_instance("leave_request", serde_json::json!({}), "user1")
        .await
        .unwrap();

    // 查询实例
    let detail = engine.query_instance(&summary.instance_id).await.unwrap();
    println!("实例状态: {:?}", detail.instance.status);
}
```

## 配置项

| 配置 | 默认值 | 范围 | 说明 |
|------|--------|------|------|
| `max_node_hops` | 10000 | [100, 1000000] | 死循环防护 |
| `plugin_call_timeout` | 5s | [100ms, 60s] | 能力调用超时 |
| `plugin_retry_max` | 3 | [0, 10] | 重试上限 |
| `plugin_retry_backoff` | 100ms | [10ms, 10s] | 初始退避 |
| `guard_expr_max_length` | 1024 | [64, 8192] | 守卫表达式长度 |
| `context_max_size_kb` | 256 | [16, 16384] | 上下文体积 |
| `instance_recovery_batch` | 500 | [10, 10000] | 批量恢复 |

## 错误码

| 码 | 功能 | HTTP |
|----|------|------|
| WF_001-006 | 定义加载 | 400 |
| WF_010-016 | 状态机 | 400/404/409 |
| WF_020-026 | 审批流 | 400/403/409 |
| WF_030-033 | 插件节点 | 400 |
| WF_040-042 | 实例管理 | 403/409 |
| WF_050-051 | 设计器 API | 404 |

## 扩展点

| Trait | 用途 |
|-------|------|
| `GuardEvaluator` | 自定义守卫条件求值 |
| `CandidateResolver` | 自定义候选人解析 |
| `ApprovalStrategy` | 自定义审批策略 |
| `FaultStrategyHandler` | 自定义容错策略 |
| `InstanceRepository` | 自定义实例持久化 |
| `WorkflowEventBus` | 自定义事件总线 |

## 测试

```bash
cargo test -p sz-rust-workflow
```

- 单元测试：121 个
- 集成测试：10 个（状态机/审批流/插件/设计器 API）
- 总计：131 个，全部通过

## 与插件系统集成

工作流引擎通过 `sz-rust-capability::CapabilityRegistry` 调用插件能力，
通过 `sz-rust-addons-loader::AddonLoader` 校验插件可用性。

```rust
use std::sync::Arc;
use sz_rust_workflow::deps::WorkflowDepsBuilder;

let deps = WorkflowDepsBuilder::new()
    .capability_registry(Arc::new(capability_registry))
    .definition_repo(definition_repo)
    .instance_repo(instance_repo)
    .task_repo(task_repo)
    .history_repo(history_repo)
    .build()
    .unwrap();
```