// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! API 网关 — 路由转发 + 鉴权 + 限流熔断 + 协议转换 + LB

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod forwarder;
pub mod grpc_streaming;
pub mod middleware;
#[cfg(feature = "grpc")]
pub mod protocol_grpc;
pub mod protocol_http;
pub mod router_engine;

#[cfg(feature = "gateway-multidim")]
pub mod degrade_response;
#[cfg(feature = "gateway-multidim")]
pub mod leaky_bucket;
#[cfg(feature = "gateway-multidim")]
pub mod multi_dim_rate_limit;
#[cfg(feature = "gateway-multidim")]
pub mod sliding_window;
#[cfg(feature = "gateway-multidim")]
pub mod slow_call_breaker;

pub use config::{
    AuthConfig, BackendProtocol, CircuitBreakerConfig, GatewayConfig, GatewayRoute, RateLimitConfig,
};
pub use error::GatewayError;
pub use forwarder::{
    BackendInstance, ForwardRequest, ForwardResponse, GatewayHandler, RequestForwarder,
};
pub use middleware::{
    AuthMiddleware, AuthResult, CircuitBreakerMiddleware, CircuitState, RateLimitMiddleware,
};
#[cfg(feature = "grpc")]
#[doc(hidden)]
pub use protocol_grpc::GrpcFieldMapping;
pub use protocol_http::{transform_response, HttpTransform};
pub use router_engine::{MatchRequest, RouterEngine};

#[cfg(feature = "gateway-multidim")]
pub use degrade_response::DegradeResponse;
#[cfg(feature = "gateway-multidim")]
pub use multi_dim_rate_limit::{
    MultiDimRateLimit, MultiDimRateLimitConfig, RateLimitAlgorithm, RateLimitDimensions,
    RequestContext,
};
#[cfg(feature = "gateway-multidim")]
pub use slow_call_breaker::{
    BreakerState, HalfOpenProbeLimit, SlowCallBreaker, SlowCallBreakerConfig,
};
