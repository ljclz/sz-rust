#![cfg(feature = "qdrant")]

//! sz300 × Qdrant 端到端接线测试
//!
//! 验证 sz-rust-vector-db 的 QdrantVectorStore 通过 sz-rust-ai-facade 的 VectorStore trait
//! 在 sz300 中可用，确保生产入口可达。
//!
//! **运行条件**：需 Docker 运行环境。
//! **运行命令**：`cargo test -p sz-rust-sz300 --features qdrant --test ai_vector_db_test`

use std::sync::Arc;

use sz_rust_ai_facade::embedding::{SimilarityMetric, VectorRecord, VectorStore};
use sz_rust_vector_db::QdrantVectorStore;
use testcontainers::core::IntoContainerPort;
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

const QDRANT_PORT: u16 = 6333;

async fn start_qdrant() -> ContainerAsync<GenericImage> {
    let image = GenericImage::new("qdrant/qdrant", "v1.12.4")
        .with_wait_for(WaitFor::message_on_stdout("Qdrant HTTP listening on"))
        .with_mapped_port(QDRANT_PORT, QDRANT_PORT.tcp());

    image.start().await.expect("Failed to start Qdrant")
}

#[tokio::test]
async fn qdrant_vector_store_wired_through_ai_facade() {
    let container = start_qdrant().await;
    let port = container
        .get_host_port_ipv4(QDRANT_PORT)
        .await
        .expect("Failed to get host port");

    let store = QdrantVectorStore::new(format!("http://127.0.0.1:{port}"), "sz300_e2e");
    store.ensure_collection(4).await.expect("ensure_collection");

    let vector_store: Arc<dyn VectorStore> = Arc::new(store);

    let records = vec![
        VectorRecord::new("doc1", vec![1.0, 0.0, 0.0, 0.0], "tenant-1")
            .with_metadata(serde_json::json!({"text": "hello world", "source": "doc1"})),
        VectorRecord::new("doc2", vec![0.0, 1.0, 0.0, 0.0], "tenant-1")
            .with_metadata(serde_json::json!({"text": "foo bar", "source": "doc2"})),
        VectorRecord::new("doc3", vec![0.0, 0.0, 1.0, 0.0], "tenant-2")
            .with_metadata(serde_json::json!({"text": "isolated", "source": "doc3"})),
    ];

    vector_store.upsert(&records).await.expect("upsert");

    let hits = vector_store
        .query(
            &[1.0, 0.0, 0.0, 0.0],
            2,
            SimilarityMetric::Cosine,
            "tenant-1",
        )
        .await
        .expect("query");

    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].id, "doc1");
    assert!((hits[0].score - 1.0).abs() < 1e-5);
    assert_eq!(hits[0].text, "hello world");

    let hits_t2 = vector_store
        .query(
            &[1.0, 0.0, 0.0, 0.0],
            10,
            SimilarityMetric::Cosine,
            "tenant-2",
        )
        .await
        .expect("query tenant-2");
    assert_eq!(hits_t2.len(), 1);
    assert_eq!(hits_t2[0].id, "doc3");

    vector_store
        .delete(&["doc1"], "tenant-1")
        .await
        .expect("delete");

    let hits_after = vector_store
        .query(
            &[1.0, 0.0, 0.0, 0.0],
            10,
            SimilarityMetric::Cosine,
            "tenant-1",
        )
        .await
        .expect("query after delete");
    assert_eq!(hits_after.len(), 1);
    assert_eq!(hits_after[0].id, "doc2");
}
