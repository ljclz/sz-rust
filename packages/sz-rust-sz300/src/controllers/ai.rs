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

struct AiController;
impl SzController for AiController {}

impl AiController {
    /// AI 聊天接口 — 接收用户 prompt，调用 LLM 返回响应
    async fn chat(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = AiController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                if state.ai.is_none() {
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

                let model = data
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("gpt-4o-mini")
                    .to_string();

                let chat_req = ChatRequest::new(
                    model,
                    vec![ChatMessage {
                        role: Role::User,
                        content: prompt,
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
                    Err(e) => ctrl.render_error(&format!("AI 调用失败: {}", e), json!({}), 0),
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
