use crate::common::{AiError, AuditHttpClient};
use crate::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    ToolCall, Usage,
};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use std::sync::Arc;
use std::time::Duration;

pub struct ClaudeProvider {
    api_key: String,
    base_url: String,
    timeout: Duration,
    http: Arc<AuditHttpClient>,
}

impl ClaudeProvider {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        http: Arc<AuditHttpClient>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            timeout: Duration::from_secs(120),
            http,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn build_request_body(&self, req: &ChatRequest) -> serde_json::Value {
        let mut system_text = String::new();
        let messages: Vec<serde_json::Value> = req.messages.iter().filter_map(|m| {
            match m.role {
                Role::System => {
                    if !system_text.is_empty() {
                        system_text.push('\n');
                    }
                    system_text.push_str(&m.content);
                    None
                }
                Role::User => Some(serde_json::json!({
                    "role": "user",
                    "content": m.content,
                })),
                Role::Assistant => {
                    let mut msg = serde_json::json!({
                        "role": "assistant",
                        "content": m.content,
                    });
                    if let Some(ref tool_calls) = m.tool_calls {
                        let blocks: Vec<serde_json::Value> = tool_calls.iter().map(|tc| {
                            serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": serde_json::from_str::<serde_json::Value>(&tc.arguments).unwrap_or(serde_json::Value::Null),
                            })
                        }).collect();
                        msg["content"] = serde_json::Value::Array(blocks);
                    }
                    Some(msg)
                }
                Role::Tool => {
                    Some(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": m.tool_call_id.as_deref().unwrap_or(""),
                            "content": m.content,
                        }],
                    }))
                }
            }
        }).collect();

        let mut body = serde_json::json!({
            "model": req.model,
            "messages": messages,
            "max_tokens": req.max_tokens.unwrap_or(4096),
        });
        if !system_text.is_empty() {
            body["system"] = serde_json::Value::String(system_text);
        }
        if let Some(temp) = req.temperature {
            body["temperature"] = serde_json::Number::from_f64(temp as f64)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null);
        }
        if let Some(ref tools) = req.tools {
            let tool_defs: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tool_defs);
        }
        body
    }

    pub fn parse_completion(resp: &serde_json::Value) -> Result<ChatCompletion, AiError> {
        let id = resp
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let model = resp
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let content_blocks = resp
            .get("content")
            .and_then(|v| v.as_array())
            .ok_or_else(|| AiError::Internal("Claude response missing content".to_string()))?;

        let mut text_content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        for block in content_blocks {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        text_content.push_str(text);
                    }
                }
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = block.get("input").unwrap_or(&serde_json::Value::Null);
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments: input.to_string(),
                    });
                }
                _ => {}
            }
        }

        let stop_reason = resp
            .get("stop_reason")
            .and_then(|v| v.as_str())
            .and_then(|s| match s {
                "end_turn" => Some(FinishReason::Stop),
                "max_tokens" => Some(FinishReason::Length),
                "tool_use" => Some(FinishReason::ToolCalls),
                _ => None,
            });

        let usage = resp
            .get("usage")
            .map(|u| Usage {
                prompt_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                completion_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                    as u32,
                total_tokens: 0,
            })
            .unwrap_or(Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            });

        let mut total_tokens = usage.prompt_tokens + usage.completion_tokens;
        if usage.total_tokens == 0 {
            total_tokens = usage.prompt_tokens + usage.completion_tokens;
        }

        let tc = if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        };
        let choice = Choice {
            index: 0,
            message: ChatMessage {
                role: Role::Assistant,
                content: text_content,
                tool_call_id: None,
                tool_calls: tc,
            },
            finish_reason: stop_reason,
        };

        Ok(ChatCompletion {
            id,
            model,
            choices: vec![choice],
            usage: Usage {
                total_tokens,
                ..usage
            },
        })
    }
}

#[async_trait]
impl LlmProvider for ClaudeProvider {
    fn name(&self) -> &str {
        "claude"
    }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        let body = self.build_request_body(&req);
        let url = format!("{}/v1/messages", self.base_url);

        let http_req = self
            .http
            .client()
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .timeout(self.timeout)
            .json(&body)
            .build()
            .map_err(AiError::from)?;

        let resp = self
            .http
            .send_with_audit(http_req, "claude", &req.model)
            .await?;
        let status = resp.status();

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 401 {
                return Err(AiError::ProviderAuthFailed(format!("claude: {}", text)));
            }
            if status.as_u16() == 429 {
                return Err(AiError::RateLimited {
                    retry_after_ms: 1000,
                });
            }
            return Err(AiError::ProviderUnavailable(format!(
                "claude {}: {}",
                status, text
            )));
        }

        let json: serde_json::Value = resp.json().await.map_err(AiError::from)?;
        Self::parse_completion(&json)
    }

    async fn stream_completion(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        let mut body = self.build_request_body(&req);
        body["stream"] = serde_json::Value::Bool(true);
        let url = format!("{}/v1/messages", self.base_url);

        let http_req = self
            .http
            .client()
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .timeout(self.timeout)
            .json(&body)
            .build()
            .map_err(AiError::from)?;

        let resp = self
            .http
            .send_with_audit(http_req, "claude", &req.model)
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AiError::ProviderUnavailable(format!(
                "claude stream {}: {}",
                status, text
            )));
        }

        let byte_stream = resp.bytes_stream();
        let delta_stream = byte_stream
            .map(|chunk_result| match chunk_result {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    let mut deltas = Vec::new();
                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                let event_type =
                                    json.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                match event_type {
                                    "content_block_delta" => {
                                        if let Some(delta) = json.get("delta") {
                                            let delta_type = delta
                                                .get("type")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            if delta_type == "text_delta" {
                                                let content = delta
                                                    .get("text")
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                                deltas.push(Ok(StreamDelta {
                                                    content_delta: content,
                                                    finish_reason: None,
                                                    tool_call_delta: None,
                                                }));
                                            }
                                        }
                                    }
                                    "message_stop" => {
                                        deltas.push(Ok(StreamDelta {
                                            content_delta: String::new(),
                                            finish_reason: Some(FinishReason::Stop),
                                            tool_call_delta: None,
                                        }));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    futures::stream::iter(deltas)
                }
                Err(e) => futures::stream::iter(vec![Err(AiError::from(e))]),
            })
            .flatten();

        Ok(Box::pin(delta_stream))
    }

    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        let total_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        Ok((total_chars as f32 * 0.3) as u32)
    }

    fn supported_models(&self) -> &[&str] {
        &[
            "claude-3-5-sonnet-20241022",
            "claude-3-5-haiku-20241022",
            "claude-3-opus-20240229",
        ]
    }
}
