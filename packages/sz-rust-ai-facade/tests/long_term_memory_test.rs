use sz_rust_ai_facade::agent::memory::{
    FileLongTermMemoryStore, LongTermMemory, LongTermMemoryStore,
};

#[tokio::test]
async fn ltm_store_and_retrieve_basic() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileLongTermMemoryStore::new(dir.path());

    let memory = LongTermMemory::new("agent-1", "hello world", "tenant-1");
    store.store(memory).await.unwrap();

    let retrieved = store.retrieve("agent-1", "tenant-1", 10).await.unwrap();
    assert_eq!(retrieved.len(), 1);
    assert_eq!(retrieved[0].content, "hello world");
    assert_eq!(retrieved[0].agent_id, "agent-1");
    assert_eq!(retrieved[0].tenant_id, "tenant-1");
}

#[tokio::test]
async fn ltm_store_multiple_and_retrieve_with_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileLongTermMemoryStore::new(dir.path());

    for i in 0..5 {
        let memory = LongTermMemory::new("agent-1", format!("content-{i}"), "tenant-1");
        store.store(memory).await.unwrap();
    }

    let retrieved = store.retrieve("agent-1", "tenant-1", 3).await.unwrap();
    assert_eq!(retrieved.len(), 3);
}

#[tokio::test]
async fn ltm_retrieve_filters_by_tenant() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileLongTermMemoryStore::new(dir.path());

    let m1 = LongTermMemory::new("agent-1", "content-a", "tenant-1");
    let m2 = LongTermMemory::new("agent-1", "content-b", "tenant-2");
    store.store(m1).await.unwrap();
    store.store(m2).await.unwrap();

    let t1 = store.retrieve("agent-1", "tenant-1", 10).await.unwrap();
    let t2 = store.retrieve("agent-1", "tenant-2", 10).await.unwrap();
    assert_eq!(t1.len(), 1);
    assert_eq!(t2.len(), 1);
    assert_eq!(t1[0].content, "content-a");
    assert_eq!(t2[0].content, "content-b");
}

#[tokio::test]
async fn ltm_by_agent_returns_all_memories() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileLongTermMemoryStore::new(dir.path());

    for i in 0..3 {
        let memory = LongTermMemory::new("agent-x", format!("msg-{i}"), "tenant-1");
        store.store(memory).await.unwrap();
    }

    let all = store.by_agent("agent-x").await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn ltm_decay_removes_low_importance_old_memories() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileLongTermMemoryStore::new(dir.path());

    let old_memory = LongTermMemory::new("agent-d", "old content", "tenant-1")
        .with_importance(0.1)
        .with_embedding(vec![1.0, 2.0, 3.0]);
    let old_time = chrono::Utc::now() - chrono::Duration::days(100);
    let mut old_memory = old_memory;
    old_memory.created_at = old_time;
    store.store(old_memory).await.unwrap();

    let new_memory = LongTermMemory::new("agent-d", "new content", "tenant-1").with_importance(0.9);
    store.store(new_memory).await.unwrap();

    let removed = store.decay("agent-d", 0.01, 0.05).await.unwrap();
    assert!(removed >= 1, "at least one memory should be decayed");

    let remaining = store.by_agent("agent-d").await.unwrap();
    assert!(
        remaining.len() < 2,
        "some memories should have been removed"
    );
}

#[tokio::test]
async fn ltm_decay_keeps_high_importance_memories() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileLongTermMemoryStore::new(dir.path());

    let memory =
        LongTermMemory::new("agent-k", "important content", "tenant-1").with_importance(1.0);
    store.store(memory).await.unwrap();

    let removed = store.decay("agent-k", 0.01, 0.01).await.unwrap();
    assert_eq!(removed, 0, "high importance memory should not be removed");

    let remaining = store.by_agent("agent-k").await.unwrap();
    assert_eq!(remaining.len(), 1);
}

#[tokio::test]
async fn ltm_decay_returns_zero_for_empty_agent() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileLongTermMemoryStore::new(dir.path());

    let removed = store.decay("nonexistent", 0.01, 0.01).await.unwrap();
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn ltm_retrieve_empty_for_nonexistent_agent() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileLongTermMemoryStore::new(dir.path());

    let retrieved = store.retrieve("nonexistent", "tenant-1", 10).await.unwrap();
    assert!(retrieved.is_empty());
}

#[tokio::test]
async fn ltm_with_importance_and_embedding() {
    let memory = LongTermMemory::new("agent-1", "content", "tenant-1")
        .with_importance(0.8)
        .with_embedding(vec![0.1, 0.2, 0.3]);
    assert!((memory.importance - 0.8).abs() < 1e-6);
    assert_eq!(memory.embedding, vec![0.1, 0.2, 0.3]);
}

#[tokio::test]
async fn ltm_persistence_across_store_instances() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();

    {
        let store = FileLongTermMemoryStore::new(&path);
        let memory = LongTermMemory::new("agent-p", "persistent content", "tenant-1");
        store.store(memory).await.unwrap();
    }

    {
        let store = FileLongTermMemoryStore::new(&path);
        let retrieved = store.retrieve("agent-p", "tenant-1", 10).await.unwrap();
        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].content, "persistent content");
    }
}
