//! LLM 统一抽象：LlmProvider trait + OpenAI/Claude/Gemini Provider + 路由 + 故障切换

pub mod failover;
pub mod provider;
pub mod router;
pub mod token_counter;
pub mod truncator;

#[cfg(feature = "claude")]
pub mod claude;
#[cfg(feature = "gemini")]
pub mod gemini;
#[cfg(feature = "openai")]
pub mod openai;

pub use failover::ProviderFailover;
pub use provider::{
    ChatCompletion, ChatMessage, ChatRequest, FinishReason, LlmProvider, Role, StreamDelta,
    ToolCall, ToolDef, Usage,
};
pub use router::ModelRouter;
pub use token_counter::TokenCounter;
pub use truncator::ContextTruncator;
