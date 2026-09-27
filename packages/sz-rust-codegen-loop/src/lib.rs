// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! AIGC 代码生成闭环：需求解析 → AI 生成 → 安全扫描 → 编译验证 → 闭环迭代。

pub mod error;
pub mod file_guard;
pub mod generator;
pub mod human_review;
pub mod r#loop;
pub mod parser;
pub mod security;
pub mod validator;

pub use error::CodegenError;
pub use file_guard::FileGuard;
pub use generator::{GeneratedFile, Generator, GeneratorConfig};
pub use human_review::{ReviewCallback, ReviewDecision};
pub use parser::{parse_requirement, CodegenTask, Framework, Language};
pub use r#loop::{CodegenLoop, CodegenLoopConfig, CodegenResult, CodegenStatus};
pub use security::SecurityScanner;
pub use validator::{ValidationResult, Validator};
// ============================================================================
// v1.5.0 P2-1: 代码生成闭环增强
// ============================================================================
#[cfg(feature = "codegen-context")]
pub mod context_injector;

#[cfg(feature = "codegen-test")]
pub mod test_generator;

#[cfg(feature = "codegen-quality")]
pub mod quality_scorer;

#[cfg(feature = "codegen-context")]
pub use context_injector::{ContextInjector, InjectionResult};

#[cfg(feature = "codegen-test")]
pub use test_generator::{GeneratedTest, TestGenerator, TestType};

#[cfg(feature = "codegen-quality")]
pub use quality_scorer::{QualityReport, QualityScorer};
