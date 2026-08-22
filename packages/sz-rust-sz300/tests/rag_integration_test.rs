//! RAG 接入 ai.rs 的 fallback 行为测试 + 生产 VectorStore 端到端测试

use sz_rust_ai_facade::embedding::{
    FileVectorStore, MemoryVectorStore, SimilarityMetric, VectorRecord, VectorStore,
};
use sz_rust_rag::facade::IndustryRag;
use sz_rust_rag::search::RagSearchRequest;

#[tokio::test]
async fn rag_not_initialized_returns_error() {
    let req = RagSearchRequest::new("生鲜存储温度", "sz300");
    let result = IndustryRag::search(req).await;
    assert!(result.is_err(), "RAG 未初始化时 search 必须返回 Err");
}

#[tokio::test]
async fn rag_fallback_preserves_original_prompt() {
    let original_prompt = "什么是冷链物流？";
    let enhanced_prompt =
        match IndustryRag::search(RagSearchRequest::new(original_prompt, "sz300")).await {
            Ok(result) if !result.content.is_empty() => {
                format!(
                    "行业知识上下文：\n{}\n\n用户问题：{}",
                    result.content, original_prompt
                )
            }
            _ => original_prompt.to_string(),
        };
    assert_eq!(
        enhanced_prompt, original_prompt,
        "RAG 未初始化时 fallback 必须保留原始 prompt"
    );
}

// ── 生产 VectorStore 端到端测试 ──

fn make_record(id: &str, vector: Vec<f32>, tenant: &str, text: &str) -> VectorRecord {
    VectorRecord::new(id, vector, tenant).with_metadata(serde_json::json!({"text": text}))
}

#[tokio::test]
async fn memory_store_upsert_query_delete_e2e() {
    let store = MemoryVectorStore::new();

    store
        .upsert(&[
            make_record("doc1", vec![1.0, 0.0, 0.0], "sz300", "冷链温度标准"),
            make_record("doc2", vec![0.0, 1.0, 0.0], "sz300", "生鲜保质期"),
            make_record("doc3", vec![0.9, 0.1, 0.0], "sz300", "冷冻库管理"),
            make_record("doc4", vec![1.0, 0.0, 0.0], "other", "其他租户"),
        ])
        .await
        .unwrap();

    let hits = store
        .query(&[1.0, 0.0, 0.0], 2, SimilarityMetric::Cosine, "sz300")
        .await
        .unwrap();
    assert_eq!(hits.len(), 2, "应返回 top-2 结果");
    assert_eq!(hits[0].id, "doc1", "最相似应为 doc1");
    assert!((hits[0].score - 1.0).abs() < 1e-6, "cosine 相似度应为 1.0");
    assert_eq!(hits[1].id, "doc3", "第二相似应为 doc3");

    store.delete(&["doc1"], "sz300").await.unwrap();
    let hits = store
        .query(&[1.0, 0.0, 0.0], 10, SimilarityMetric::Cosine, "sz300")
        .await
        .unwrap();
    assert_eq!(hits.len(), 2, "删除 doc1 后应剩 2 条");
    assert!(hits.iter().all(|h| h.id != "doc1"), "doc1 应已删除");

    let hits_other = store
        .query(&[1.0, 0.0, 0.0], 10, SimilarityMetric::Cosine, "other")
        .await
        .unwrap();
    assert_eq!(hits_other.len(), 1, "other 租户隔离");
    assert_eq!(hits_other[0].id, "doc4");
}

#[tokio::test]
async fn file_store_persistence() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();

    {
        let store = FileVectorStore::load(&path).await.unwrap();
        store
            .upsert(&[
                make_record("r1", vec![1.0, 0.0], "t1", "alpha"),
                make_record("r2", vec![0.0, 1.0], "t1", "beta"),
            ])
            .await
            .unwrap();
    }

    let store = FileVectorStore::load(&path).await.unwrap();
    let hits = store
        .query(&[1.0, 0.0], 2, SimilarityMetric::Cosine, "t1")
        .await
        .unwrap();
    assert_eq!(hits.len(), 2, "重载后应有 2 条记录");
    assert_eq!(hits[0].id, "r1", "最相似应为 r1");
    assert_eq!(hits[0].text, "alpha", "metadata text 应保留");
}

#[tokio::test]
async fn memory_store_all_metrics() {
    let store = MemoryVectorStore::new();
    store
        .upsert(&[make_record("r1", vec![3.0, 4.0], "t", "x")])
        .await
        .unwrap();

    let cosine_hits = store
        .query(&[3.0, 4.0], 1, SimilarityMetric::Cosine, "t")
        .await
        .unwrap();
    assert!(
        (cosine_hits[0].score - 1.0).abs() < 1e-6,
        "Cosine 自相似度 = 1.0"
    );

    let dot_hits = store
        .query(&[1.0, 1.0], 1, SimilarityMetric::Dot, "t")
        .await
        .unwrap();
    assert!((dot_hits[0].score - 7.0).abs() < 1e-6, "Dot(3,4)·(1,1) = 7");

    let l2_hits = store
        .query(&[3.0, 4.0], 1, SimilarityMetric::L2, "t")
        .await
        .unwrap();
    assert!((l2_hits[0].score - 0.0).abs() < 1e-6, "L2 自距离 = 0");
}
