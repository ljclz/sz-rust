use crate::llm::provider::ChatMessage;
use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

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

    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance;
        self
    }

    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = embedding;
        self
    }
}

#[async_trait]
pub trait LongTermMemoryStore: Send + Sync {
    async fn store(&self, memory: LongTermMemory) -> Result<(), crate::common::AiError>;
    async fn retrieve(
        &self,
        agent_id: &str,
        tenant_id: &str,
        limit: usize,
    ) -> Result<Vec<LongTermMemory>, crate::common::AiError>;
    async fn decay(
        &self,
        agent_id: &str,
        lambda: f32,
        threshold: f32,
    ) -> Result<usize, crate::common::AiError>;
    async fn by_agent(&self, agent_id: &str)
        -> Result<Vec<LongTermMemory>, crate::common::AiError>;
}

pub struct FileLongTermMemoryStore {
    dir: PathBuf,
    cache: Arc<RwLock<HashMap<String, Vec<LongTermMemory>>>>,
}

impl FileLongTermMemoryStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn file_path(&self, agent_id: &str) -> PathBuf {
        self.dir.join(format!("{agent_id}.jsonl"))
    }

    async fn load_from_file(&self, agent_id: &str) -> Vec<LongTermMemory> {
        let path = self.file_path(agent_id);
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => content
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    async fn save_to_file(
        &self,
        agent_id: &str,
        memories: &[LongTermMemory],
    ) -> Result<(), crate::common::AiError> {
        let path = self.file_path(agent_id);
        let content: String = memories
            .iter()
            .map(|m| serde_json::to_string(m).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| crate::common::AiError::Internal(format!("file write: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl LongTermMemoryStore for FileLongTermMemoryStore {
    async fn store(&self, memory: LongTermMemory) -> Result<(), crate::common::AiError> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|e| crate::common::AiError::Internal(format!("create dir: {e}")))?;

        let agent_id = memory.agent_id.clone();
        let mut memories = self.load_from_file(&agent_id).await;
        memories.push(memory);

        {
            let mut cache = self.cache.write();
            cache.insert(agent_id.clone(), memories.clone());
        }

        self.save_to_file(&agent_id, &memories).await?;
        Ok(())
    }

    async fn retrieve(
        &self,
        agent_id: &str,
        tenant_id: &str,
        limit: usize,
    ) -> Result<Vec<LongTermMemory>, crate::common::AiError> {
        let memories = self.load_from_file(agent_id).await;
        let filtered: Vec<LongTermMemory> = memories
            .into_iter()
            .filter(|m| m.tenant_id == tenant_id)
            .take(limit)
            .collect();
        Ok(filtered)
    }

    async fn decay(
        &self,
        agent_id: &str,
        lambda: f32,
        threshold: f32,
    ) -> Result<usize, crate::common::AiError> {
        let mut memories = self.load_from_file(agent_id).await;
        let now = chrono::Utc::now();
        let original_count = memories.len();

        memories.retain(|m| {
            let age = (now - m.created_at).num_seconds() as f32 / 86400.0;
            let decayed_importance = m.importance * (-lambda * age).exp();
            decayed_importance >= threshold
        });

        let removed = original_count - memories.len();
        if removed > 0 {
            self.save_to_file(agent_id, &memories).await?;
            let mut cache = self.cache.write();
            cache.insert(agent_id.to_string(), memories);
        }

        Ok(removed)
    }

    async fn by_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<LongTermMemory>, crate::common::AiError> {
        Ok(self.load_from_file(agent_id).await)
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
        assert_eq!(mem.messages()[0].content.as_text(), Some("hello"));
    }

    #[test]
    fn short_term_memory_evicts_oldest() {
        let mut mem = ShortTermMemory::new(3);
        mem.push(msg("a"));
        mem.push(msg("b"));
        mem.push(msg("c"));
        mem.push(msg("d"));
        assert_eq!(mem.messages().len(), 3);
        assert_eq!(mem.messages()[0].content.as_text(), Some("b"));
        assert_eq!(mem.messages()[2].content.as_text(), Some("d"));
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
