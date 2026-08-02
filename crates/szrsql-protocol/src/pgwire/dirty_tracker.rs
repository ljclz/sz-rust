//! P1-2：跨会话共享的脏表跟踪器（增量快照机制核心组件）。
//!
//! # 设计
//!
//! - Session 在事务 COMMIT 成功后调用 [`DirtyTableTracker::mark_dirty`] 标记修改过的表
//! - 后台周期性快照任务调用 [`DirtyTableTracker::take_dirty`] 取出脏表集合（原子清空）
//! - 仅对脏表集合中的表重新序列化，非脏表保留磁盘上已有快照内容
//!
//! # 线程安全
//!
//! 内部使用 `Arc<Mutex<HashSet<String>>>`，可安全跨会话共享。
//! `DirtyTableTracker` 自身实现 `Clone`（克隆时共享同一内部状态）。

use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 跨会话共享的脏表跟踪器。
///
/// 记录自上次快照保存以来被修改过的表名集合，供增量快照保存逻辑
/// 判断哪些表需要重新序列化，避免每次都对所有表做全量序列化。
#[derive(Debug, Clone)]
pub struct DirtyTableTracker {
    /// 内部共享状态：脏表名集合（受 tokio Mutex 保护，支持跨 await 持有）
    inner: Arc<Mutex<HashSet<String>>>,
}

impl DirtyTableTracker {
    /// 创建空的脏表跟踪器。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// 标记表为脏（下次快照保存时需要重新序列化）。
    ///
    /// 在事务 COMMIT 成功后调用，传入该事务修改过的所有表名。
    pub async fn mark_dirty(&self, table_name: &str) {
        let mut guard = self.inner.lock().await;
        guard.insert(table_name.to_string());
    }

    /// 批量标记多张表为脏。
    ///
    /// 接收任意 `IntoIterator<Item = impl AsRef<str>>`，便于直接传入 `HashSet<String>` 或 `Vec<&str>`。
    pub async fn mark_dirty_many(&self, table_names: impl IntoIterator<Item = impl AsRef<str>>) {
        let mut guard = self.inner.lock().await;
        for name in table_names {
            guard.insert(name.as_ref().to_string());
        }
    }

    /// 取出当前所有脏表（清空内部集合）。
    ///
    /// 在 `save_incremental_snapshot` 内部使用：取出脏表后立即清空，
    /// 保证保存期间新提交的事务会在下次保存时再次被记录。
    pub async fn take_dirty(&self) -> HashSet<String> {
        let mut guard = self.inner.lock().await;
        std::mem::take(&mut *guard)
    }

    /// 查询当前是否有脏表（不清空）。
    ///
    /// 用于关闭时判断是否需要执行最终快照保存。
    pub async fn is_dirty(&self) -> bool {
        let guard = self.inner.lock().await;
        !guard.is_empty()
    }
}

impl Default for DirtyTableTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mark_and_take_dirty() {
        let tracker = DirtyTableTracker::new();
        tracker.mark_dirty("users").await;
        tracker.mark_dirty("orders").await;
        assert!(tracker.is_dirty().await);

        let dirty = tracker.take_dirty().await;
        assert_eq!(dirty.len(), 2);
        assert!(dirty.contains("users"));
        assert!(dirty.contains("orders"));
        assert!(!tracker.is_dirty().await);
    }

    #[tokio::test]
    async fn test_mark_dirty_many() {
        let tracker = DirtyTableTracker::new();
        let names = vec!["t1", "t2", "t3"];
        tracker.mark_dirty_many(names).await;
        let dirty = tracker.take_dirty().await;
        assert_eq!(dirty.len(), 3);
    }

    #[tokio::test]
    async fn test_clone_shares_state() {
        let tracker = DirtyTableTracker::new();
        let cloned = tracker.clone();
        tracker.mark_dirty("shared_table").await;
        let dirty = cloned.take_dirty().await;
        assert_eq!(dirty.len(), 1);
        assert!(dirty.contains("shared_table"));
        // 原实例的状态应已被清空（共享同一内部 Mutex）
        assert!(!tracker.is_dirty().await);
    }

    #[tokio::test]
    async fn test_take_dirty_empty() {
        let tracker = DirtyTableTracker::new();
        let dirty = tracker.take_dirty().await;
        assert!(dirty.is_empty());
        assert!(!tracker.is_dirty().await);
    }

    #[tokio::test]
    async fn test_is_dirty_without_take() {
        let tracker = DirtyTableTracker::new();
        assert!(!tracker.is_dirty().await);
        tracker.mark_dirty("t").await;
        assert!(tracker.is_dirty().await);
        // 多次调用 is_dirty 不应清空状态
        assert!(tracker.is_dirty().await);
        let dirty = tracker.take_dirty().await;
        assert_eq!(dirty.len(), 1);
    }
}
