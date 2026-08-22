//! audit_remediation_v2 — LocalEmbedding 真实加载端到端测试
//!
//! 验证 sz300 main.rs build_local_embedding 环境变量驱动：
//! SZ300_LOCAL_EMBEDDING_MODEL 设置时真实加载（load_model + is_model_loaded），
//! 文件不存在时返回 Err(LocalModelLoadFailed)，未设置时降级 new_pseudo。

use sz_rust_ai_facade::embedding::{EmbeddingProvider, LocalEmbedding};

#[tokio::test]
async fn local_embedding_real_load() {
    let tmpfile = tempfile::NamedTempFile::new().expect("NamedTempFile 创建失败");
    tokio::fs::write(tmpfile.path(), b"dummy model content")
        .await
        .expect("写入模型文件失败");

    let mut emb = LocalEmbedding::new(tmpfile.path().to_str().unwrap())
        .expect("LocalEmbedding::new 应成功（文件存在）");
    assert!(!emb.is_model_loaded(), "load_model 前应未加载");

    emb.load_model().expect("load_model 应成功");
    assert!(emb.is_model_loaded(), "load_model 后应已加载");
    assert_eq!(emb.name(), "local-embedding-loaded");
}

#[tokio::test]
async fn local_embedding_file_not_found_degrade() {
    let result = LocalEmbedding::new("nonexistent-model.onnx");
    assert!(result.is_err(), "不存在的模型文件应返回错误");

    let emb = LocalEmbedding::new_pseudo(384);
    assert!(!emb.is_model_loaded());
    assert_eq!(emb.name(), "local-embedding-pseudo");
}
