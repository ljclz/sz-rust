#![cfg(feature = "qdrant-integration")]

//! Qdrant 端到端集成测试
//!
//! 使用 testcontainers 启动 Qdrant 容器，验证 upsert → query → delete 全流程。
//!
//! **运行条件**：需 Docker 运行环境。
//! **运行命令**：`cargo test -p sz-rust-vector-db --features qdrant-integration --test qdrant_integration_test`

use sz_rust_ai_facade::embedding::{SimilarityMetric, VectorRecord, VectorStore};
use sz_rust_vector_db::QdrantVectorStore;
use testcontainers::core::IntoContainerPort;
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

const QDRANT_IMAGE: &str = "qdrant/qdrant";
const QDRANT_TAG: &str = "v1.12.4";
const QDRANT_PORT: u16 = 6333;

async fn start_qdrant() -> ContainerAsync<GenericImage> {
    let image = GenericImage::new(QDRANT_IMAGE, QDRANT_TAG)
        .with_wait_for(WaitFor::message_on_stdout("Qdrant HTTP listening on"))
        .with_mapped_port(QDRANT_PORT, QDRANT_PORT.tcp());

    image
        .start()
        .await
        .expect("Failed to start Qdrant container")
}

async fn create_store(container: &ContainerAsync<GenericImage>) -> QdrantVectorStore {
    let port = container
        .get_host_port_ipv4(QDRANT_PORT)
        .await
        .expect("Failed to get host port");
    let base_url = format!("http://127.0.0.1:{port}");
    let store = QdrantVectorStore::new(base_url, "test_collection");
    store
        .ensure_collection(3)
        .await
        .expect("ensure_collection failed");
    store
}

fn make_record(id: &str, vector: Vec<f32>, tenant: &str, text: &str) -> VectorRecord {
    VectorRecord::new(id, vector, tenant).with_metadata(serde_json::json!({"text": text}))
}

#[tokio::test]
async fn upsert_query_delete_full_flow() {
    let container = start_qdrant().await;
    let store = create_store(&container).await;

    let records = vec![
        make_record("r1", vec![1.0, 0.0, 0.0], "t1", "alpha"),
        make_record("r2", vec![0.0, 1.0, 0.0], "t1", "beta"),
        make_record("r3", vec![1.0, 0.1, 0.0], "t1", "gamma"),
        make_record("r4", vec![0.0, 0.0, 1.0], "t2", "delta"),
    ];
    store.upsert(&records).await.expect("upsert failed");

    let hits = store
        .query(&[1.0, 0.0, 0.0], 2, SimilarityMetric::Cosine, "t1")
        .await
        .expect("query failed");
    assert_eq!(hits.len(), 2, "should return top-2 results");
    assert_eq!(hits[0].id, "r1", "best match should be r1");
    assert!(
        (hits[0].score - 1.0).abs() < 1e-5,
        "cosine score should be ~1.0"
    );
    assert_eq!(hits[0].text, "alpha");

    store.delete(&["r1"], "t1").await.expect("delete failed");

    let hits_after = store
        .query(&[1.0, 0.0, 0.0], 10, SimilarityMetric::Cosine, "t1")
        .await
        .expect("query after delete failed");
    assert_eq!(hits_after.len(), 2, "should have 2 records left for t1");
    assert!(
        !hits_after.iter().any(|h| h.id == "r1"),
        "r1 should be deleted"
    );
}

#[tokio::test]
async fn tenant_isolation() {
    let container = start_qdrant().await;
    let store = create_store(&container).await;

    store
        .upsert(&[make_record("a1", vec![1.0, 0.0, 0.0], "tenant-a", "a")])
        .await
        .expect("upsert a failed");
    store
        .upsert(&[make_record("b1", vec![1.0, 0.0, 0.0], "tenant-b", "b")])
        .await
        .expect("upsert b failed");

    let hits_a = store
        .query(&[1.0, 0.0, 0.0], 10, SimilarityMetric::Cosine, "tenant-a")
        .await
        .expect("query tenant-a failed");
    assert_eq!(hits_a.len(), 1);
    assert_eq!(hits_a[0].id, "a1");

    let hits_b = store
        .query(&[1.0, 0.0, 0.0], 10, SimilarityMetric::Cosine, "tenant-b")
        .await
        .expect("query tenant-b failed");
    assert_eq!(hits_b.len(), 1);
    assert_eq!(hits_b[0].id, "b1");
}

#[tokio::test]
async fn upsert_overwrites_existing() {
    let container = start_qdrant().await;
    let store = create_store(&container).await;

    store
        .upsert(&[make_record("r1", vec![1.0, 0.0, 0.0], "t1", "old")])
        .await
        .expect("first upsert failed");
    store
        .upsert(&[make_record("r1", vec![0.0, 1.0, 0.0], "t1", "new")])
        .await
        .expect("second upsert failed");

    let hits = store
        .query(&[0.0, 1.0, 0.0], 1, SimilarityMetric::Cosine, "t1")
        .await
        .expect("query failed");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "r1");
    assert_eq!(hits[0].text, "new", "should have overwritten text");
}

#[tokio::test]
async fn query_empty_tenant_returns_empty() {
    let container = start_qdrant().await;
    let store = create_store(&container).await;

    let hits = store
        .query(
            &[1.0, 0.0, 0.0],
            10,
            SimilarityMetric::Cosine,
            "nonexistent",
        )
        .await
        .expect("query failed");
    assert!(hits.is_empty());
}
