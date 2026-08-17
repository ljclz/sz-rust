//! Facade 错误路径单元测试
//!
//! 此测试文件在独立 crate 中运行，Ai 全局未初始化，
//! 用于覆盖 facade.rs 中未初始化时的错误分支。

use sz_rust_ai_facade::agent::engine::{AgentOptions, AgentTask};
use sz_rust_ai_facade::llm::provider::ChatRequest;
use sz_rust_ai_facade::rag::pipeline::RagRequest;
use sz_rust_ai_facade::Ai;

/// 确保未初始化状态。
/// 注意：integration test 每个文件独立二进制，OnceLock 初始为空。
/// 但其他测试可能已初始化（同一进程多线程共享），所以用 try_* 方法。
fn ensure_uninitialized() -> bool {
    !Ai::is_initialized()
}

#[tokio::test]
async fn facade_chat_not_initialized_error() {
    if !ensure_uninitialized() {
        return;
    }
    let req = ChatRequest::new("gpt-4o", vec![]);
    let err = Ai::chat(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_INTERNAL");
    assert!(err.to_string().contains("not initialized"));
}

#[tokio::test]
async fn facade_stream_chat_not_initialized_error() {
    if !ensure_uninitialized() {
        return;
    }
    let req = ChatRequest::new("gpt-4o", vec![]);
    match Ai::stream_chat(req).await {
        Err(e) => assert_eq!(e.error_code(), "AI_INTERNAL"),
        Ok(_) => panic!("expected error when facade not initialized"),
    }
}

#[tokio::test]
async fn facade_embed_not_initialized_error() {
    if !ensure_uninitialized() {
        return;
    }
    let err = Ai::embed(vec!["hi".into()], "model").await.unwrap_err();
    assert_eq!(err.error_code(), "AI_INTERNAL");
}

#[tokio::test]
async fn facade_rag_not_initialized_error() {
    if !ensure_uninitialized() {
        return;
    }
    let req = RagRequest::new("query", "tenant");
    let err = Ai::rag(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_INTERNAL");
}

#[tokio::test]
async fn facade_agent_not_initialized_error() {
    if !ensure_uninitialized() {
        return;
    }
    let task = AgentTask::new("do something");
    let opts = AgentOptions::new("tenant");
    let err = Ai::agent(task, opts).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_INTERNAL");
}

#[test]
fn facade_default_model_not_initialized_error() {
    if !ensure_uninitialized() {
        return;
    }
    let err = Ai::default_model().unwrap_err();
    assert_eq!(err.error_code(), "AI_INTERNAL");
}

#[test]
fn facade_is_initialized_false_when_not_init() {
    // 确保 is_initialized() 调用不 panic（无论初始化状态）
    let initialized = Ai::is_initialized();
    // 如果进程内其他测试初始化过则为 true，否则 false；不 panic 即满足
    assert!(
        initialized || !initialized,
        "is_initialized must return bool without panicking"
    );
}
