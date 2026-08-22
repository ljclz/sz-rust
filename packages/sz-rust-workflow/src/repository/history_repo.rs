use async_trait::async_trait;

use crate::error::WorkflowResult;
use crate::instance::HistoryEntry;

/// 历史持久化 Repository。
#[async_trait]
pub trait HistoryRepository: Send + Sync + 'static {
    /// 追加历史条目（只增不删）。
    async fn append(&self, entry: &HistoryEntry) -> WorkflowResult<()>;

    /// 列出实例完整历史。
    async fn list_by_instance(&self, instance_id: &str) -> WorkflowResult<Vec<HistoryEntry>>;
}
