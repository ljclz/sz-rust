use crate::llm::provider::ChatMessage;

#[derive(Debug, Clone)]
pub struct ShortTermMemory {
    messages: Vec<ChatMessage>,
    max_messages: usize,
}

impl ShortTermMemory {
    pub fn new(max_messages: usize) -> Self {
        Self {
            messages: Vec::new(),
            max_messages,
        }
    }

    pub fn push(&mut self, message: ChatMessage) {
        self.messages.push(message);
        if self.messages.len() > self.max_messages {
            self.messages.remove(0);
        }
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

impl Default for ShortTermMemory {
    fn default() -> Self {
        Self::new(100)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LongTermMemory {
    pub memory_id: String,
    pub agent_id: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub importance: f32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub tenant_id: String,
}

impl LongTermMemory {
    pub fn new(
        agent_id: impl Into<String>,
        content: impl Into<String>,
        tenant_id: impl Into<String>,
    ) -> Self {
        Self {
            memory_id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.into(),
            content: content.into(),
            embedding: Vec::new(),
            importance: 0.5,
            created_at: chrono::Utc::now(),
            tenant_id: tenant_id.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::Role;

    fn msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    #[test]
    fn short_term_memory_push_and_read() {
        let mut mem = ShortTermMemory::new(10);
        mem.push(msg("hello"));
        mem.push(msg("world"));
        assert_eq!(mem.messages().len(), 2);
        assert_eq!(mem.messages()[0].content, "hello");
    }

    #[test]
    fn short_term_memory_evicts_oldest() {
        let mut mem = ShortTermMemory::new(3);
        mem.push(msg("a"));
        mem.push(msg("b"));
        mem.push(msg("c"));
        mem.push(msg("d"));
        assert_eq!(mem.messages().len(), 3);
        assert_eq!(mem.messages()[0].content, "b");
        assert_eq!(mem.messages()[2].content, "d");
    }

    #[test]
    fn short_term_memory_clear() {
        let mut mem = ShortTermMemory::new(10);
        mem.push(msg("a"));
        mem.clear();
        assert!(mem.messages().is_empty());
    }

    #[test]
    fn long_term_memory_new_generates_id() {
        let m1 = LongTermMemory::new("agent1", "content1", "tenant1");
        let m2 = LongTermMemory::new("agent1", "content1", "tenant1");
        assert_ne!(m1.memory_id, m2.memory_id);
        assert_eq!(m1.agent_id, "agent1");
        assert_eq!(m1.content, "content1");
        assert_eq!(m1.tenant_id, "tenant1");
        assert!((m1.importance - 0.5).abs() < 1e-6);
        assert!(m1.embedding.is_empty());
    }
}
