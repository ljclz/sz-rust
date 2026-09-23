// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! LLM 统一抽象：LlmProvider trait + OpenAI/Claude/Gemini Provider + 路由 + 故障切换

pub mod builtin_models;
pub mod failover;
pub mod fallback;
pub mod model_registry;
pub mod provider;
pub mod router;
pub mod test_support;
pub mod token_counter;
pub mod truncator;

#[cfg(feature = "claude")]
pub mod claude;
#[cfg(feature = "gemini")]
pub mod gemini;
#[cfg(feature = "openai")]
pub mod openai;

pub use builtin_models::{register_builtin_models, BuiltinProviders};
pub use failover::ProviderFailover;
pub use fallback::{cost_optimal, FallbackChain, FallbackResult, RoundRobinLb};
pub use model_registry::{ModelCost, ModelEntry, ModelRegistry};
pub use provider::{
    ChatCompletion, ChatMessage, ChatRequest, FinishReason, LlmProvider, Role, StreamDelta,
    ToolCall, ToolDef, Usage,
};
pub use router::{ModelRouter, RoutingRecord, RoutingStrategy};
pub use token_counter::TokenCounter;
pub use truncator::ContextTruncator;
