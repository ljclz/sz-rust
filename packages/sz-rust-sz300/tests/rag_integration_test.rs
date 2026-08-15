//! RAG 接入 ai.rs 的 fallback 行为测试
//!
//! 验证：RAG 未初始化时，IndustryRag::search 返回 Err，
//! ai.rs 中的 `_ => prompt` 分支使用原始 prompt，不阻塞正常 chat 流程。

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
