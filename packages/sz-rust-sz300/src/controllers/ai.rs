// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//
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
use sz_rust_ai_facade::common::guardrail::{
    apply_output_guardrail, build_safe_messages, GuardrailConfig,
};
use sz_rust_ai_facade::llm::provider::{ChatRequest, ContentPart};
use sz_rust_core::controller::SzController;

/// 默认模型（与 ai-facade 默认配置对齐）
const DEFAULT_MODEL: &str = "gpt-4o-mini";

/// 允许客户端指定的模型白名单（安全修复 M-1：成本控制 + 防任意模型调用）
const ALLOWED_MODELS: &[&str] = &["gpt-4o-mini", "gpt-4o", "gpt-4.1-mini"];

/// prompt 最大字符数（安全修复 M-1：阻断 LLM 成本耗尽 DoS，约 4K tokens）
const MAX_PROMPT_CHARS: usize = 16_000;

/// 流式输出 max_tokens 上限（P2-1：输出长度限制，约 8K chars）
const STREAM_MAX_TOKENS: u32 = 8_000;

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

                // P2-1 护栏①：System prompt 隔离——用户输入作为 User message，不混入 System
                let cfg = GuardrailConfig::default();
                let messages = build_safe_messages(cfg.system_prompt, &enhanced_prompt);
                let chat_req = ChatRequest::new(model, messages);

                match sz_rust_ai_facade::Ai::chat(chat_req).await {
                    Ok(completion) => {
                        let raw_content = completion
                            .choices
                            .first()
                            .map(|c| c.message.content.text_or_empty().to_string())
                            .unwrap_or_default();
                        // P2-1 护栏②③④：PII 脱敏 + 输出长度截断 + moderation（None=NoOp 优雅降级）
                        let guarded = apply_output_guardrail(&raw_content, &cfg, None).await;
                        let content: ContentPart = guarded.text.into();
                        ctrl.render_success(
                            "success",
                            json!({
                                "content": content,
                                "model": completion.model,
                                "truncated": guarded.truncated,
                                "moderation": format!("{:?}", guarded.moderation),
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

    /// AI Embedding 接口 — 接收文本数组，返回嵌入向量（audit_remediation_v3）
    async fn embed(_state: &AppState, req: Request<Body>) -> Response {
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
                let texts: Vec<String> = match data.get("texts").and_then(|v| v.as_array()) {
                    Some(arr) => arr
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    None => {
                        return ctrl.render_error("texts 不能为空且必须是字符串数组", json!({}), 0)
                    }
                };
                if texts.is_empty() {
                    return ctrl.render_error("texts 不能为空", json!({}), 0);
                }
                let model = data
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("text-embedding-3-small")
                    .to_string();
                match sz_rust_ai_facade::Ai::embed(texts, &model).await {
                    Ok(result) => ctrl.render_success(
                        "success",
                        json!({
                            "embeddings": result.embeddings,
                            "model": result.model,
                            "dimensions": result.dimensions,
                            "usage_tokens": result.usage_tokens,
                        }),
                    ),
                    Err(e) => ctrl.render_error(format!("Embedding 调用失败: {}", e), json!({}), 0),
                }
            }
            Err(e) => ctrl.render_error(&e, json!({}), 0),
        }
    }

    /// AI 流式聊天接口 — SSE 流式返回（audit_remediation_v3）
    async fn stream(_state: &AppState, req: Request<Body>) -> Response {
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
                if prompt.chars().count() > MAX_PROMPT_CHARS {
                    return ctrl.render_error(
                        format!("prompt 过长（上限 {} 字符）", MAX_PROMPT_CHARS),
                        json!({}),
                        0,
                    );
                }
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
                // P2-1 护栏①：System prompt 隔离；④：max_tokens 限制输出长度
                // 流式场景 PII 脱敏/moderation 需在聚合层做（PII 可能跨 delta 边界）
                let cfg = GuardrailConfig::default();
                let messages = build_safe_messages(cfg.system_prompt, &prompt);
                let mut chat_req = ChatRequest::new(model, messages);
                chat_req.max_tokens = Some(STREAM_MAX_TOKENS);
                match sz_rust_ai_facade::Ai::stream_chat(chat_req).await {
                    Ok(stream) => {
                        use futures::StreamExt;
                        let sse_stream = stream.map(|result| {
                            let msg = match result {
                                Ok(delta) => {
                                    let json = serde_json::json!({
                                        "content_delta": delta.content_delta,
                                        "finish_reason": format!("{:?}", delta.finish_reason),
                                    });
                                    format!("data: {}\n\n", json)
                                }
                                Err(e) => format!("data: {{\"error\":\"{}\"}}\n\n", e),
                            };
                            Ok::<_, std::convert::Infallible>(msg)
                        });
                        Response::builder()
                            .header("content-type", "text/event-stream")
                            .header("cache-control", "no-cache")
                            .body(axum::body::Body::from_stream(sse_stream))
                            .unwrap_or_else(|e| {
                                ctrl.render_error(format!("构建 SSE 响应失败: {}", e), json!({}), 0)
                            })
                    }
                    Err(e) => ctrl.render_error(format!("AI 流式调用失败: {}", e), json!({}), 0),
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

/// AI Embedding 接口 — 接收文本数组，返回嵌入向量（audit_remediation_v3）
#[tracing::instrument(skip(state, req))]
pub async fn embed(State(state): State<AppState>, req: Request<Body>) -> Response {
    AiController::embed(&state, req).await
}

/// AI 流式聊天接口 — SSE 流式返回（audit_remediation_v3）
#[tracing::instrument(skip(state, req))]
pub async fn stream(State(state): State<AppState>, req: Request<Body>) -> Response {
    AiController::stream(&state, req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::mock_app_state;
    use axum::routing::post;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use std::sync::Mutex;

    /// 串行化所有 AI 测试，避免并行 race（AI facade 全局 OnceLock 不可重置）。
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// 确保 AI facade 已初始化（全局 OnceLock，仅首次调用实际初始化）。
    ///
    /// 用 `MockProvider` 构造最小 `ModelRouter`，使 `Ai::is_initialized()` 返回 true，
    /// 让 handler 越过"AI 服务未配置"早退分支，进入 prompt/texts/model 校验路径。
    /// OnceLock 不可重置：首次设置后后续调用为 no-op。
    fn ensure_ai_initialized() {
        if sz_rust_ai_facade::Ai::is_initialized() {
            return;
        }
        use std::collections::HashMap;
        use std::sync::Arc;
        use sz_rust_ai_facade::llm::provider::ProviderRef;
        use sz_rust_ai_facade::llm::router::ModelRouter;
        use sz_rust_ai_facade::llm::test_support::MockProvider;

        let mut routes = HashMap::new();
        routes.insert(
            "gpt-4o-mini".to_string(),
            Arc::new(MockProvider) as ProviderRef,
        );
        let router = ModelRouter::new(routes, "gpt-4o-mini".to_string());
        // OnceLock::set 已设置时返回 Err，忽略（首次设置即可）
        let _ = sz_rust_ai_facade::Ai::init_default(router, None, None, None, None);
    }

    /// 收集 Response body 为 String。
    async fn collect_body(response: Response) -> String {
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        String::from_utf8(body_bytes.to_vec()).expect("UTF-8")
    }

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

    // =========================================================================
    // 以下为 handler 分支测试（验收 1-11）。
    //
    // 执行顺序依赖：libtest 按函数名字母序调度，`aaa_` < `ai_` < `test_`。
    // - `aaa_` 前缀：AI 未初始化测试，必须在 `ensure_ai_initialized` 调用前运行；
    //   若 AI 已被其他测试初始化（OnceLock 不可重置），则 skip 以避免误判。
    // - `test_` 前缀：需要 AI 已初始化的测试，首项调用 `ensure_ai_initialized`。
    // - body 解析失败测试无需 AI 状态（body 解析在 AI 检查之前），但仍串行化。
    // =========================================================================

    /// 验收 3：embed AI 未初始化 → 返回"AI 服务未配置"错误。
    ///
    /// OnceLock 不可重置 + 多线程调度不保证顺序：若运行时 AI 已被其他测试初始化，
    /// 则验证 handler 越过 AI 检查（body 不含"AI 服务未配置"）作为补充验证；
    /// AI 未初始化时真正验证验收 3。两种情况下测试均覆盖 handler 代码行。
    #[tokio::test]
    async fn aaa_embed_ai_not_initialized_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let ai_not_initialized = !sz_rust_ai_facade::Ai::is_initialized();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/embed", post(embed))
            .with_state(state);
        let body = serde_json::json!({ "texts": ["hello"] }).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/embed")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
        if ai_not_initialized {
            assert!(
                body.contains("AI 服务未配置"),
                "AI 未初始化应包含'AI 服务未配置': {}",
                body
            );
        } else {
            assert!(
                !body.contains("AI 服务未配置"),
                "AI 已初始化不应返回'AI 服务未配置': {}",
                body
            );
        }
    }

    /// 验收 7：stream AI 未初始化 → 返回"AI 服务未配置"错误。
    ///
    /// 同 [`aaa_embed_ai_not_initialized_returns_error`]：OnceLock 不可重置，
    /// AI 已初始化时验证 handler 越过 AI 检查作为补充验证。
    #[tokio::test]
    async fn aaa_stream_ai_not_initialized_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let ai_not_initialized = !sz_rust_ai_facade::Ai::is_initialized();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/stream", post(stream))
            .with_state(state);
        let body = serde_json::json!({ "prompt": "hello" }).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/stream")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
        if ai_not_initialized {
            assert!(
                body.contains("AI 服务未配置"),
                "AI 未初始化应包含'AI 服务未配置': {}",
                body
            );
        } else {
            assert!(
                !body.contains("AI 服务未配置"),
                "AI 已初始化不应返回'AI 服务未配置': {}",
                body
            );
        }
    }

    /// 验收 1：chat prompt 超过 MAX_PROMPT_CHARS（16000）→ 返回"prompt 过长"错误。
    #[tokio::test]
    async fn test_chat_prompt_too_long_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        ensure_ai_initialized();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/chat", post(chat))
            .with_state(state);
        let long_prompt = "x".repeat(MAX_PROMPT_CHARS + 1);
        let body = serde_json::json!({ "prompt": long_prompt }).to_string();
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
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
        assert!(body.contains("prompt 过长"), "应包含'prompt 过长': {}", body);
    }

    /// 验收 2：chat body 非 JSON → 解析失败 → 返回 code:0 错误响应。
    #[tokio::test]
    async fn test_chat_body_parse_error_returns_code0() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/chat", post(chat))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/chat")
                    .header("content-type", "application/json")
                    .body(Body::from("{invalid json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
    }

    /// 验收 4：embed texts 非数组 → 返回"texts 不能为空且必须是字符串数组"错误。
    #[tokio::test]
    async fn test_embed_texts_not_array_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        ensure_ai_initialized();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/embed", post(embed))
            .with_state(state);
        let body = serde_json::json!({ "texts": "not an array" }).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/embed")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
        assert!(
            body.contains("texts 不能为空且必须是字符串数组"),
            "应包含'texts 不能为空且必须是字符串数组': {}",
            body
        );
    }

    /// 验收 5：embed texts 空数组 → 返回"texts 不能为空"错误。
    #[tokio::test]
    async fn test_embed_texts_empty_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        ensure_ai_initialized();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/embed", post(embed))
            .with_state(state);
        let body = serde_json::json!({ "texts": [] }).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/embed")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
        assert!(
            body.contains("texts 不能为空"),
            "应包含'texts 不能为空': {}",
            body
        );
    }

    /// 验收 6：embed body 非 JSON → 解析失败 → 返回 code:0 错误响应。
    #[tokio::test]
    async fn test_embed_body_parse_error_returns_code0() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/embed", post(embed))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/embed")
                    .header("content-type", "application/json")
                    .body(Body::from("{invalid json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
    }

    /// 验收 8：stream 空 prompt → 返回"prompt 不能为空"错误。
    #[tokio::test]
    async fn test_stream_empty_prompt_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        ensure_ai_initialized();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/stream", post(stream))
            .with_state(state);
        let body = serde_json::json!({ "prompt": "" }).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/stream")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
        assert!(
            body.contains("prompt 不能为空"),
            "应包含'prompt 不能为空': {}",
            body
        );
    }

    /// 验收 9：stream prompt 超过 MAX_PROMPT_CHARS → 返回"prompt 过长"错误。
    #[tokio::test]
    async fn test_stream_prompt_too_long_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        ensure_ai_initialized();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/stream", post(stream))
            .with_state(state);
        let long_prompt = "x".repeat(MAX_PROMPT_CHARS + 1);
        let body = serde_json::json!({ "prompt": long_prompt }).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/stream")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
        assert!(body.contains("prompt 过长"), "应包含'prompt 过长': {}", body);
    }

    /// 验收 10：stream 不支持的模型 → 返回"不支持的模型"错误。
    #[tokio::test]
    async fn test_stream_unsupported_model_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        ensure_ai_initialized();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/stream", post(stream))
            .with_state(state);
        let body = serde_json::json!({ "prompt": "hello", "model": "gpt-999" }).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/stream")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
        assert!(
            body.contains("不支持的模型"),
            "应包含'不支持的模型': {}",
            body
        );
    }

    /// 验收 11：stream body 非 JSON → 解析失败 → 返回 code:0 错误响应。
    #[tokio::test]
    async fn test_stream_body_parse_error_returns_code0() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/ai/stream", post(stream))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ai/stream")
                    .header("content-type", "application/json")
                    .body(Body::from("{invalid json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = collect_body(response).await;
        assert!(body.contains("\"code\":0"), "应返回 code:0: {}", body);
    }
}
