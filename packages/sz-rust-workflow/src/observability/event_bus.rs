use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::error::WorkflowResult;

/// 工作流事件，对齐 design 2.3.2 事件模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowEvent {
    InstanceStarted {
        instance_id: String,
        flow_key: String,
        initiator: String,
        timestamp: DateTime<Utc>,
    },
    InstanceSuspended {
        instance_id: String,
        actor: String,
        timestamp: DateTime<Utc>,
    },
    InstanceResumed {
        instance_id: String,
        actor: String,
        timestamp: DateTime<Utc>,
    },
    InstanceTerminated {
        instance_id: String,
        actor: String,
        timestamp: DateTime<Utc>,
    },
    InstanceCompleted {
        instance_id: String,
        timestamp: DateTime<Utc>,
    },
    InstanceWithdrawn {
        instance_id: String,
        actor: String,
        timestamp: DateTime<Utc>,
    },
    TransitionFired {
        instance_id: String,
        from: String,
        to: String,
        event: String,
        timestamp: DateTime<Utc>,
    },
    NodeEntered {
        instance_id: String,
        node_id: String,
        timestamp: DateTime<Utc>,
    },
    NodeLeft {
        instance_id: String,
        node_id: String,
        timestamp: DateTime<Utc>,
    },
    TaskCreated {
        instance_id: String,
        task_id: String,
        node_id: String,
        candidates: Vec<String>,
        timestamp: DateTime<Utc>,
    },
    TaskHandled {
        instance_id: String,
        task_id: String,
        actor: String,
        action: String,
        timestamp: DateTime<Utc>,
    },
    PluginNodeCompleted {
        instance_id: String,
        node_id: String,
        capability_name: String,
        timestamp: DateTime<Utc>,
    },
}

impl WorkflowEvent {
    pub fn instance_id(&self) -> &str {
        match self {
            Self::InstanceStarted { instance_id, .. }
            | Self::InstanceSuspended { instance_id, .. }
            | Self::InstanceResumed { instance_id, .. }
            | Self::InstanceTerminated { instance_id, .. }
            | Self::InstanceCompleted { instance_id, .. }
            | Self::InstanceWithdrawn { instance_id, .. }
            | Self::TransitionFired { instance_id, .. }
            | Self::NodeEntered { instance_id, .. }
            | Self::NodeLeft { instance_id, .. }
            | Self::TaskCreated { instance_id, .. }
            | Self::TaskHandled { instance_id, .. }
            | Self::PluginNodeCompleted { instance_id, .. } => instance_id,
        }
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::InstanceStarted { timestamp, .. }
            | Self::InstanceSuspended { timestamp, .. }
            | Self::InstanceResumed { timestamp, .. }
            | Self::InstanceTerminated { timestamp, .. }
            | Self::InstanceCompleted { timestamp, .. }
            | Self::InstanceWithdrawn { timestamp, .. }
            | Self::TransitionFired { timestamp, .. }
            | Self::NodeEntered { timestamp, .. }
            | Self::NodeLeft { timestamp, .. }
            | Self::TaskCreated { timestamp, .. }
            | Self::TaskHandled { timestamp, .. }
            | Self::PluginNodeCompleted { timestamp, .. } => *timestamp,
        }
    }
}

/// 事件总线 trait。
#[async_trait]
pub trait WorkflowEventBus: Send + Sync + 'static {
    async fn publish(&self, event: WorkflowEvent) -> WorkflowResult<()>;
}

/// InMemory 事件总线（broadcast channel）。
pub struct InMemoryEventBus {
    sender: broadcast::Sender<WorkflowEvent>,
}

impl InMemoryEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    /// 订阅事件。
    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.sender.subscribe()
    }
}

impl Default for InMemoryEventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

#[async_trait]
impl WorkflowEventBus for InMemoryEventBus {
    async fn publish(&self, event: WorkflowEvent) -> WorkflowResult<()> {
        let _ = self.sender.send(event);
        Ok(())
    }
}

/// Noop 事件总线（测试用）。
pub struct NoopEventBus;

#[async_trait]
impl WorkflowEventBus for NoopEventBus {
    async fn publish(&self, _event: WorkflowEvent) -> WorkflowResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_event_bus_publish_subscribe() {
        let bus = InMemoryEventBus::new(16);
        let mut rx = bus.subscribe();
        let event = WorkflowEvent::InstanceStarted {
            instance_id: "i1".into(),
            flow_key: "test".into(),
            initiator: "u1".into(),
            timestamp: Utc::now(),
        };
        bus.publish(event.clone()).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received.instance_id(), "i1");
    }

    #[tokio::test]
    async fn noop_event_bus() {
        let bus = NoopEventBus;
        let event = WorkflowEvent::InstanceStarted {
            instance_id: "i1".into(),
            flow_key: "test".into(),
            initiator: "u1".into(),
            timestamp: Utc::now(),
        };
        bus.publish(event).await.unwrap();
    }

    #[test]
    fn event_instance_id_and_timestamp() {
        let ts = Utc::now();
        let event = WorkflowEvent::TransitionFired {
            instance_id: "i1".into(),
            from: "draft".into(),
            to: "review".into(),
            event: "submit".into(),
            timestamp: ts,
        };
        assert_eq!(event.instance_id(), "i1");
        assert_eq!(event.timestamp(), ts);
    }
}
