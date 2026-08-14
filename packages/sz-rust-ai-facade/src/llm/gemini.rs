use crate::common::{AiError, AuditHttpClient};
use crate::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    ToolCall, Usage,
};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use std::sync::Arc;
use std::time::Duration;

pub struct GeminiProvider {
    api_key: String,
    base_url: String,
    timeout: Duration,
    http: Arc<AuditHttpClient>,
}

impl GeminiProvider {
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
        let mut system_instruction = None;
        let contents: Vec<serde_json::Value> = req
            .messages
            .iter()
            .filter_map(|m| match m.role {
                Role::System => {
                    system_instruction = Some(serde_json::json!({
                        "parts": [{"text": m.content}]
                    }));
                    None
                }
                Role::User => Some(serde_json::json!({
                    "role": "user",
                    "parts": [{"text": m.content}],
                })),
                Role::Assistant => Some(serde_json::json!({
                    "role": "model",
                    "parts": [{"text": m.content}],
                })),
                Role::Tool => Some(serde_json::json!({
                    "role": "function",
                    "parts": [{"text": m.content}],
                })),
            })
            .collect();

        let mut body = serde_json::json!({
            "contents": contents,
        });
        if let Some(si) = system_instruction {
            body["systemInstruction"] = si;
        }

        let mut gen_config = serde_json::json!({});
        if let Some(max_tokens) = req.max_tokens {
            gen_config["maxOutputTokens"] = serde_json::Value::Number(max_tokens.into());
        }
        if let Some(temp) = req.temperature {
            gen_config["temperature"] = serde_json::Number::from_f64(temp as f64)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null);
        }
        if let Some(ref tools) = req.tools {
            let declarations: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect();
            gen_config["tools"] = serde_json::json!({"functionDeclarations": declarations});
        }
        body["generationConfig"] = gen_config;
        body
    }

    fn parse_completion(resp: &serde_json::Value) -> Result<ChatCompletion, AiError> {
        let candidates = resp
            .get("candidates")
            .and_then(|v| v.as_array())
            .ok_or_else(|| AiError::Internal("Gemini response missing candidates".to_string()))?;

        let choices: Vec<Choice> = candidates
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let content = c
                    .get("content")
                    .and_then(|v| v.get("parts"))
                    .and_then(|v| v.as_array());
                let mut text_content = String::new();
                let mut tool_calls: Vec<ToolCall> = Vec::new();
                if let Some(parts) = content {
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                            text_content.push_str(text);
                        }
                        if let Some(fc) = part.get("functionCall") {
                            let name = fc
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args = fc.get("args").unwrap_or(&serde_json::Value::Null);
                            tool_calls.push(ToolCall {
                                id: uuid::Uuid::new_v4().to_string(),
                                name,
                                arguments: args.to_string(),
                            });
                        }
                    }
                }
                let finish_reason = c
                    .get("finishReason")
                    .and_then(|v| v.as_str())
                    .and_then(|s| match s {
                        "STOP" => Some(FinishReason::Stop),
                        "MAX_TOKENS" => Some(FinishReason::Length),
                        "SAFETY" => Some(FinishReason::ContentFilter),
                        _ => None,
                    });
                let tc = if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                };
                Choice {
                    index: i as u32,
                    message: ChatMessage {
                        role: Role::Assistant,
                        content: text_content,
                        tool_call_id: None,
                        tool_calls: tc,
                    },
                    finish_reason,
                }
            })
            .collect();

        let usage = resp
            .get("usageMetadata")
            .map(|u| Usage {
                prompt_tokens: u
                    .get("promptTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                completion_tokens: u
                    .get("candidatesTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                total_tokens: u
                    .get("totalTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
            })
            .unwrap_or(Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            });

        Ok(ChatCompletion {
            id: uuid::Uuid::new_v4().to_string(),
            model: String::new(),
            choices,
            usage,
        })
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        let body = self.build_request_body(&req);
        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.base_url, req.model, self.api_key
        );

        let http_req = self
            .http
            .client()
            .post(&url)
            .header("Content-Type", "application/json")
            .timeout(self.timeout)
            .json(&body)
            .build()
            .map_err(AiError::from)?;

        let resp = self
            .http
            .send_with_audit(http_req, "gemini", &req.model)
            .await?;
        let status = resp.status();

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(AiError::ProviderAuthFailed(format!("gemini: {}", text)));
            }
            if status.as_u16() == 429 {
                return Err(AiError::RateLimited {
                    retry_after_ms: 1000,
                });
            }
            return Err(AiError::ProviderUnavailable(format!(
                "gemini {}: {}",
                status, text
            )));
        }

        let json: serde_json::Value = resp.json().await.map_err(AiError::from)?;
        let mut completion = Self::parse_completion(&json)?;
        completion.model = req.model;
        Ok(completion)
    }

    async fn stream_completion(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        let body = self.build_request_body(&req);
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, req.model, self.api_key
        );

        let http_req = self
            .http
            .client()
            .post(&url)
            .header("Content-Type", "application/json")
            .timeout(self.timeout)
            .json(&body)
            .build()
            .map_err(AiError::from)?;

        let resp = self
            .http
            .send_with_audit(http_req, "gemini", &req.model)
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AiError::ProviderUnavailable(format!(
                "gemini stream {}: {}",
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
                                if let Some(candidate) = json
                                    .get("candidates")
                                    .and_then(|c| c.as_array())
                                    .and_then(|a| a.first())
                                {
                                    if let Some(parts) = candidate
                                        .get("content")
                                        .and_then(|c| c.get("parts"))
                                        .and_then(|v| v.as_array())
                                    {
                                        for part in parts {
                                            if let Some(text) =
                                                part.get("text").and_then(|v| v.as_str())
                                            {
                                                deltas.push(Ok(StreamDelta {
                                                    content_delta: text.to_string(),
                                                    finish_reason: None,
                                                    tool_call_delta: None,
                                                }));
                                            }
                                        }
                                    }
                                    if let Some(reason) =
                                        candidate.get("finishReason").and_then(|v| v.as_str())
                                    {
                                        let fr = match reason {
                                            "STOP" => Some(FinishReason::Stop),
                                            "MAX_TOKENS" => Some(FinishReason::Length),
                                            "SAFETY" => Some(FinishReason::ContentFilter),
                                            _ => None,
                                        };
                                        if fr.is_some() {
                                            deltas.push(Ok(StreamDelta {
                                                content_delta: String::new(),
                                                finish_reason: fr,
                                                tool_call_delta: None,
                                            }));
                                        }
                                    }
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
        &["gemini-2.0-flash", "gemini-1.5-pro", "gemini-1.5-flash"]
    }
}
