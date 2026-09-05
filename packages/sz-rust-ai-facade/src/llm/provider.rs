// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::common::AiError;
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 多模态内容部分
///
/// `#[serde(untagged)]` 确保纯文本序列化为字符串（向后兼容），
/// 多模态序列化为对象。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ContentPart {
    Text(String),
    Image { url: String, detail: ImageDetail },
    ImageBase64 { data: String, mime_type: String },
}

impl ContentPart {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentPart::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn text_or_empty(&self) -> &str {
        self.as_text().unwrap_or("")
    }

    pub fn is_image(&self) -> bool {
        matches!(
            self,
            ContentPart::Image { .. } | ContentPart::ImageBase64 { .. }
        )
    }
}

impl From<String> for ContentPart {
    fn from(s: String) -> Self {
        ContentPart::Text(s)
    }
}

impl From<&str> for ContentPart {
    fn from(s: &str) -> Self {
        ContentPart::Text(s.to_string())
    }
}

impl std::fmt::Display for ContentPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentPart::Text(s) => write!(f, "{s}"),
            ContentPart::Image { url, .. } => write!(f, "[image:{url}]"),
            ContentPart::ImageBase64 { mime_type, .. } => write!(f, "[image:{mime_type}]"),
        }
    }
}

impl Default for ContentPart {
    fn default() -> Self {
        ContentPart::Text(String::new())
    }
}

impl std::hash::Hash for ContentPart {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.to_string().hash(state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageDetail {
    Low,
    High,
    Auto,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: ContentPart,
    pub tool_call_id: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub tools: Option<Vec<ToolDef>>,
    pub stream: bool,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            max_tokens: None,
            temperature: None,
            tools: None,
            stream: false,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatCompletion {
    pub id: String,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCallDelta {
    pub id: String,
    pub name: String,
    pub arguments_delta: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct StreamDelta {
    pub content_delta: String,
    pub finish_reason: Option<FinishReason>,
    pub tool_call_delta: Option<ToolCallDelta>,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError>;

    async fn stream_completion(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError>;

    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError>;

    fn supported_models(&self) -> &[&str];
}

pub type ProviderRef = Arc<dyn LlmProvider>;
