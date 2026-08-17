use crate::common::{AiError, AuditHttpClient};
use crate::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    ToolCall, Usage,
};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use std::sync::Arc;
use std::time::Duration;

pub struct OpenAiProvider {
    api_key: String,
    base_url: String,
    timeout: Duration,
    http: Arc<AuditHttpClient>,
}

impl OpenAiProvider {
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
        let messages: Vec<serde_json::Value> = req
            .messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "tool",
                };
                let mut msg = serde_json::json!({
                    "role": role,
                    "content": m.content,
                });
                if let Some(ref tool_call_id) = m.tool_call_id {
                    msg["tool_call_id"] = serde_json::Value::String(tool_call_id.clone());
                }
                if let Some(ref tool_calls) = m.tool_calls {
                    let calls: Vec<serde_json::Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments,
                                }
                            })
                        })
                        .collect();
                    msg["tool_calls"] = serde_json::Value::Array(calls);
                }
                msg
            })
            .collect();

        let mut body = serde_json::json!({
            "model": req.model,
            "messages": messages,
            "stream": req.stream,
        });
        if let Some(max_tokens) = req.max_tokens {
            body["max_tokens"] = serde_json::Value::Number(max_tokens.into());
        }
        if let Some(temp) = req.temperature {
            body["temperature"] = serde_json::Value::Number(
                serde_json::Number::from_f64(temp as f64).unwrap_or_else(|| 0.into()),
            );
        }
        if let Some(ref tools) = req.tools {
            let tool_defs: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
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

        let choices = resp
            .get("choices")
            .and_then(|v| v.as_array())
            .ok_or_else(|| AiError::Internal("OpenAI response missing choices".to_string()))?
            .iter()
            .map(|c| {
                let index = c.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let msg = c.get("message").ok_or_else(|| {
                    AiError::Internal("OpenAI choice missing message".to_string())
                })?;
                let content = msg
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let role = msg
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("assistant");
                let role_enum = match role {
                    "system" => Role::System,
                    "user" => Role::User,
                    "tool" => Role::Tool,
                    _ => Role::Assistant,
                };
                let tool_calls: Option<Vec<ToolCall>> =
                    msg.get("tool_calls").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|tc| {
                                let id = tc.get("id")?.as_str()?.to_string();
                                let func = tc.get("function")?;
                                let name = func.get("name")?.as_str()?.to_string();
                                let arguments = func.get("arguments")?.as_str()?.to_string();
                                Some(ToolCall {
                                    id,
                                    name,
                                    arguments,
                                })
                            })
                            .collect()
                    });
                let finish_reason = c
                    .get("finish_reason")
                    .and_then(|v| v.as_str())
                    .and_then(|s| match s {
                        "stop" => Some(FinishReason::Stop),
                        "length" => Some(FinishReason::Length),
                        "tool_calls" => Some(FinishReason::ToolCalls),
                        "content_filter" => Some(FinishReason::ContentFilter),
                        _ => None,
                    });
                Ok(Choice {
                    index,
                    message: ChatMessage {
                        role: role_enum,
                        content,
                        tool_call_id: None,
                        tool_calls,
                    },
                    finish_reason,
                })
            })
            .collect::<Result<Vec<_>, AiError>>()?;

        let usage = resp
            .get("usage")
            .map(|u| Usage {
                prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                completion_tokens: u
                    .get("completion_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            })
            .unwrap_or(Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            });

        Ok(ChatCompletion {
            id,
            model,
            choices,
            usage,
        })
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        let mut req = req;
        req.stream = false;
        let body = self.build_request_body(&req);
        let url = format!("{}/v1/chat/completions", self.base_url);

        let http_req = self
            .http
            .client()
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .timeout(self.timeout)
            .json(&body)
            .build()
            .map_err(AiError::from)?;

        let resp = self
            .http
            .send_with_audit(http_req, "openai", &req.model)
            .await?;
        let status = resp.status();

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 401 {
                return Err(AiError::ProviderAuthFailed(format!("openai: {}", text)));
            }
            if status.as_u16() == 429 {
                return Err(AiError::RateLimited {
                    retry_after_ms: 1000,
                });
            }
            return Err(AiError::ProviderUnavailable(format!(
                "openai {}: {}",
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
        let mut req = req;
        req.stream = true;
        let body = self.build_request_body(&req);
        let url = format!("{}/v1/chat/completions", self.base_url);

        let http_req = self
            .http
            .client()
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .timeout(self.timeout)
            .json(&body)
            .build()
            .map_err(AiError::from)?;

        let resp = self
            .http
            .send_with_audit(http_req, "openai", &req.model)
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AiError::ProviderUnavailable(format!(
                "openai stream {}: {}",
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
                            if data == "[DONE]" {
                                deltas.push(Ok(StreamDelta {
                                    content_delta: String::new(),
                                    finish_reason: Some(FinishReason::Stop),
                                    tool_call_delta: None,
                                }));
                                continue;
                            }
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                if let Some(choice) = json
                                    .get("choices")
                                    .and_then(|c| c.as_array())
                                    .and_then(|a| a.first())
                                {
                                    let content_delta = choice
                                        .get("delta")
                                        .and_then(|d| d.get("content"))
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let finish_reason = choice
                                        .get("finish_reason")
                                        .and_then(|v| v.as_str())
                                        .and_then(|s| match s {
                                            "stop" => Some(FinishReason::Stop),
                                            "length" => Some(FinishReason::Length),
                                            "tool_calls" => Some(FinishReason::ToolCalls),
                                            "content_filter" => Some(FinishReason::ContentFilter),
                                            _ => None,
                                        });
                                    deltas.push(Ok(StreamDelta {
                                        content_delta,
                                        finish_reason,
                                        tool_call_delta: None,
                                    }));
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
        Ok((total_chars as f32 * 0.25) as u32)
    }

    fn supported_models(&self) -> &[&str] {
        &["gpt-4o", "gpt-4o-mini", "gpt-4-turbo", "gpt-3.5-turbo"]
    }
}
