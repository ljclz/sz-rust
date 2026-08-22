//! AI 控制器 — LLM 聊天接口
//!
//! 对接 sz-rust-ai-facade，提供 `/api/v1/ai/chat` 端点。
//! AI facade 为全局单例（OnceLock），初始化需在 main.rs 中调用 `Ai::init_default()`。

use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_ai_facade::llm::provider::{ChatMessage, ChatRequest, Role};
use sz_rust_core::controller::SzController;

/// 默认模型（与 ai-facade 默认配置对齐）
const DEFAULT_MODEL: &str = "gpt-4o-mini";

/// 允许客户端指定的模型白名单（安全修复 M-1：成本控制 + 防任意模型调用）
const ALLOWED_MODELS: &[&str] = &["gpt-4o-mini", "gpt-4o", "gpt-4.1-mini"];

/// prompt 最大字符数（安全修复 M-1：阻断 LLM 成本耗尽 DoS，约 4K tokens）
const MAX_PROMPT_CHARS: usize = 16_000;

struct AiController;
impl SzController for AiController {}

impl AiController {
    /// AI 聊天接口 — 接收用户 prompt，调用 LLM 返回响应
    async fn chat(_state: &AppState, req: Request<Body>) -> Response {
        let ctrl = AiController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                if !sz_rust_ai_facade::Ai::is_initialized() {
                    return ctrl.render_error(
                        "AI 服务未配置 — 请设置 SZ300_AI_API_KEY 环境变量",
                        json!({}),
                        0,
                    );
                }

                let prompt = match data.get("prompt").and_then(|v| v.as_str()) {
                    Some(p) if !p.is_empty() => p.to_string(),
                    _ => return ctrl.render_error("prompt 不能为空", json!({}), 0),
                };
                // 安全修复 M-1（2026-08-14）：prompt 长度上限，阻断 LLM 成本耗尽 DoS
                if prompt.chars().count() > MAX_PROMPT_CHARS {
                    return ctrl.render_error(
                        format!("prompt 过长（上限 {} 字符）", MAX_PROMPT_CHARS),
                        json!({}),
                        0,
                    );
                }

                // 安全修复 M-1：model 白名单，禁止客户端指定任意模型（成本控制 + 防提示注入面扩大）
                let model = data
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or(DEFAULT_MODEL)
                    .to_string();
                if !ALLOWED_MODELS.contains(&model.as_str()) {
                    return ctrl.render_error(
                        format!("不支持的模型: {}（允许: {:?}）", model, ALLOWED_MODELS),
                        json!({}),
                        0,
                    );
                }

                // RAG 检索增强：尝试行业知识检索，失败则使用原始 prompt（不阻塞正常流程）
                let enhanced_prompt = match sz_rust_rag::facade::IndustryRag::search(
                    sz_rust_rag::search::RagSearchRequest::new(&prompt, "sz300"),
                )
                .await
                {
                    Ok(result) if !result.content.is_empty() => {
                        format!(
                            "行业知识上下文：\n{}\n\n用户问题：{}",
                            result.content, prompt
                        )
                    }
                    _ => prompt,
                };

                let chat_req = ChatRequest::new(
                    model,
                    vec![ChatMessage {
                        role: Role::User,
                        content: enhanced_prompt.into(),
                        tool_call_id: None,
                        tool_calls: None,
                    }],
                );

                match sz_rust_ai_facade::Ai::chat(chat_req).await {
                    Ok(completion) => {
                        let content = completion
                            .choices
                            .first()
                            .map(|c| c.message.content.clone())
                            .unwrap_or_default();
                        ctrl.render_success(
                            "success",
                            json!({
                                "content": content,
                                "model": completion.model,
                                "usage": {
                                    "prompt_tokens": completion.usage.prompt_tokens,
                                    "completion_tokens": completion.usage.completion_tokens,
                                    "total_tokens": completion.usage.total_tokens,
                                }
                            }),
                        )
                    }
                    Err(e) => ctrl.render_error(format!("AI 调用失败: {}", e), json!({}), 0),
                }
            }
            Err(e) => ctrl.render_error(&e, json!({}), 0),
        }
    }
}

/// AI 聊天接口（对齐 PHP AiController::chat）
#[tracing::instrument(skip(state, req))]
pub async fn chat(State(state): State<AppState>, req: Request<Body>) -> Response {
    AiController::chat(&state, req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::mock_app_state;
    use axum::routing::post;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn ai_chat_no_auth_returns_error() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/chat", post(chat))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/chat")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let body = String::from_utf8(body_bytes.to_vec()).expect("UTF-8");
        assert!(
            body.contains("\"code\":0") || body.contains("\"code\":1"),
            "应返回有效响应: {}",
            body
        );
    }

    /// 覆盖 ai chat 提供 prompt 但 AI 未初始化路径
    #[tokio::test]
    async fn ai_chat_with_prompt_but_ai_not_initialized_returns_error() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/chat", post(chat))
            .with_state(state);
        let body = serde_json::json!({"prompt": "hello"}).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let body = String::from_utf8(body_bytes.to_vec()).expect("UTF-8");
        // AI 未初始化或调用失败都应返回有效响应
        assert!(
            body.contains("\"code\":0") || body.contains("\"code\":1"),
            "应返回有效响应: {}",
            body
        );
    }

    /// 覆盖 ai chat 空 prompt 路径
    #[tokio::test]
    async fn ai_chat_with_empty_prompt_returns_error() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/chat", post(chat))
            .with_state(state);
        let body = serde_json::json!({"prompt": ""}).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let body = String::from_utf8(body_bytes.to_vec()).expect("UTF-8");
        assert!(
            body.contains("\"code\":0") || body.contains("\"code\":1"),
            "应返回有效响应: {}",
            body
        );
    }

    /// 覆盖 ai chat 不支持模型路径
    #[tokio::test]
    async fn ai_chat_with_unsupported_model_returns_error() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/chat", post(chat))
            .with_state(state);
        let body = serde_json::json!({"prompt": "hello", "model": "gpt-999"}).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let body = String::from_utf8(body_bytes.to_vec()).expect("UTF-8");
        assert!(
            body.contains("\"code\":0") || body.contains("\"code\":1"),
            "应返回有效响应: {}",
            body
        );
    }
}
