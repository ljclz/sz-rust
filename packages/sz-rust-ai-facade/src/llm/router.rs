// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::common::AiError;
use crate::llm::provider::ProviderRef;
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;

pub struct ModelRouter {
    routes: ArcSwap<HashMap<String, ProviderRef>>,
    default_model: ArcSwap<String>,
}

impl ModelRouter {
    pub fn new(routes: HashMap<String, ProviderRef>, default_model: String) -> Self {
        Self {
            routes: ArcSwap::from_pointee(routes),
            default_model: ArcSwap::from_pointee(default_model),
        }
    }

    pub fn route(&self, model: Option<&str>) -> Result<ProviderRef, AiError> {
        let routes = self.routes.load();
        let default_model = self.default_model.load();
        let model_name = model.unwrap_or(default_model.as_str());
        routes.get(model_name).cloned().ok_or_else(|| {
            AiError::ConfigInvalid(format!("model '{}' not found in routing table", model_name))
        })
    }

    pub fn apply_update(&self, routes: HashMap<String, ProviderRef>, default_model: String) {
        self.routes.store(Arc::new(routes));
        self.default_model.store(Arc::new(default_model));
    }

    pub fn default_model(&self) -> String {
        self.default_model.load().as_str().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{
        ChatCompletion, ChatMessage, ChatRequest, LlmProvider, StreamDelta,
    };
    use async_trait::async_trait;
    use futures::stream::BoxStream;

    struct MockProvider;
    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
            Err(AiError::Internal("mock".into()))
        }
        async fn stream_completion(
            &self,
            _req: ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
            Err(AiError::Internal("mock".into()))
        }
        async fn token_count(&self, _messages: &[ChatMessage]) -> Result<u32, AiError> {
            Ok(0)
        }
        fn supported_models(&self) -> &[&str] {
            &[]
        }
    }

    fn make_router() -> ModelRouter {
        let mut routes = HashMap::new();
        routes.insert("gpt-4o".to_string(), Arc::new(MockProvider) as ProviderRef);
        routes.insert(
            "claude-3".to_string(),
            Arc::new(MockProvider) as ProviderRef,
        );
        ModelRouter::new(routes, "gpt-4o".to_string())
    }

    #[test]
    fn route_by_explicit_model() {
        let r = make_router();
        assert!(r.route(Some("gpt-4o")).is_ok());
        assert!(r.route(Some("claude-3")).is_ok());
    }

    #[test]
    fn route_by_default_model() {
        let r = make_router();
        assert!(r.route(None).is_ok());
    }

    #[test]
    fn route_unknown_model_errors() {
        let r = make_router();
        let result = r.route(Some("unknown-model"));
        match result {
            Err(e) => assert_eq!(e.error_code(), "AI_CONFIG_INVALID"),
            Ok(_) => panic!("expected error for unknown model"),
        }
    }

    #[test]
    fn apply_update_replaces_routes() {
        let r = make_router();
        assert!(r.route(Some("gpt-4o")).is_ok());
        let new_routes = HashMap::new();
        r.apply_update(new_routes, "new-default".to_string());
        assert_eq!(r.default_model(), "new-default");
        assert!(r.route(Some("gpt-4o")).is_err());
    }

    #[test]
    fn default_model_returns_correct_value() {
        let r = make_router();
        assert_eq!(r.default_model(), "gpt-4o");
    }
}
