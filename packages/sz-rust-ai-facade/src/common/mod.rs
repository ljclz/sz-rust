// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 公共基础层：错误类型 / 审计 HTTP 客户端 / SSE 适配器 / Prometheus 指标

pub mod audit;
pub mod error;
pub mod metrics;
pub mod sse_adapter;

pub use audit::{AuditHttpClient, RateLimitConfig};
pub use error::AiError;
pub use metrics::AiMetrics;
pub use sse_adapter::SseAdapter;
