// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 内置模型注册：DeepSeek / OpenAI / KAT-Coder-Pro-V2.5 / 本地模型。
//!
//! 每个模型声明能力标签与优先级，调用 [`register_builtin_models`] 注册到 [`ModelRegistry`]。

use crate::llm::model_registry::{ModelCost, ModelEntry, ModelRegistry};
use crate::llm::provider::ProviderRef;

/// 内置模型名称常量。
pub const DEEPSEEK: &str = "deepseek-chat";
pub const OPENAI: &str = "gpt-4o";
pub const KAT_CODER_PRO: &str = "kat-coder-pro-v2.5";
pub const LOCAL_MODEL: &str = "local-qwen-7b";

/// 注册 4 个内置模型到注册表。
///
/// | 模型 | 能力标签 | 优先级 | 用途 |
/// |------|---------|--------|------|
/// | `deepseek-chat` | chat, codegen | 50 | 通用对话 + 代码生成备选 |
/// | `gpt-4o` | chat, vision | 80 | 高质量对话 + 多模态 |
/// | `kat-coder-pro-v2.5` | codegen | 100 | 代码生成首选 |
/// | `local-qwen-7b` | chat, codegen | 10 | 本地降级 |
pub fn register_builtin_models(registry: &ModelRegistry, providers: &BuiltinProviders) {
    registry.register(ModelEntry {
        name: DEEPSEEK.to_string(),
        capabilities: vec!["chat".into(), "codegen".into()],
        priority: 50,
        cost: ModelCost {
            input_per_1k: 0.001,
            output_per_1k: 0.002,
        },
        provider: providers.deepseek.clone(),
    });

    registry.register(ModelEntry {
        name: OPENAI.to_string(),
        capabilities: vec!["chat".into(), "vision".into()],
        priority: 80,
        cost: ModelCost {
            input_per_1k: 0.005,
            output_per_1k: 0.015,
        },
        provider: providers.openai.clone(),
    });

    registry.register(ModelEntry {
        name: KAT_CODER_PRO.to_string(),
        capabilities: vec!["codegen".into()],
        priority: 100,
        cost: ModelCost {
            input_per_1k: 0.003,
            output_per_1k: 0.006,
        },
        provider: providers.kat_coder.clone(),
    });

    registry.register(ModelEntry {
        name: LOCAL_MODEL.to_string(),
        capabilities: vec!["chat".into(), "codegen".into()],
        priority: 10,
        cost: ModelCost::default(),
        provider: providers.local.clone(),
    });
}

/// 内置模型的 Provider 引用集合。
#[derive(Clone)]
pub struct BuiltinProviders {
    pub deepseek: ProviderRef,
    pub openai: ProviderRef,
    pub kat_coder: ProviderRef,
    pub local: ProviderRef,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::test_support::MockProvider;
    use std::sync::Arc;

    fn make_providers() -> BuiltinProviders {
        BuiltinProviders {
            deepseek: Arc::new(MockProvider) as ProviderRef,
            openai: Arc::new(MockProvider) as ProviderRef,
            kat_coder: Arc::new(MockProvider) as ProviderRef,
            local: Arc::new(MockProvider) as ProviderRef,
        }
    }

    #[test]
    fn register_4_models() {
        let reg = ModelRegistry::new();
        register_builtin_models(&reg, &make_providers());
        assert_eq!(reg.len(), 4);
    }

    #[test]
    fn codegen_query_returns_kat_first() {
        let reg = ModelRegistry::new();
        register_builtin_models(&reg, &make_providers());
        let codegen_models = reg.query_by_capability("codegen");
        assert_eq!(codegen_models[0].name, KAT_CODER_PRO);
        assert!(codegen_models.iter().any(|e| e.name == DEEPSEEK));
        assert!(codegen_models.iter().any(|e| e.name == LOCAL_MODEL));
    }

    #[test]
    fn chat_query_returns_openai_first() {
        let reg = ModelRegistry::new();
        register_builtin_models(&reg, &make_providers());
        let chat_models = reg.query_by_capability("chat");
        assert_eq!(chat_models[0].name, OPENAI);
    }

    #[test]
    fn vision_query_returns_only_openai() {
        let reg = ModelRegistry::new();
        register_builtin_models(&reg, &make_providers());
        let vision_models = reg.query_by_capability("vision");
        assert_eq!(vision_models.len(), 1);
        assert_eq!(vision_models[0].name, OPENAI);
    }

    #[test]
    fn all_models_have_at_least_one_capability() {
        let reg = ModelRegistry::new();
        register_builtin_models(&reg, &make_providers());
        for entry in reg.list() {
            assert!(
                !entry.capabilities.is_empty(),
                "{} has no capabilities",
                entry.name
            );
        }
    }
}
