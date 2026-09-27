// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P3-3 多维度限流熔断集成测试

#![cfg(feature = "gateway-multidim")]

use std::time::Duration;

use sz_rust_api_gateway::{
    BreakerState, DegradeResponse, HalfOpenProbeLimit, MultiDimRateLimit, MultiDimRateLimitConfig,
    RateLimitAlgorithm, RateLimitDimensions, RequestContext, SlowCallBreaker,
    SlowCallBreakerConfig,
};

#[test]
fn test_four_dimensions_independent_counting() {
    let dims = RateLimitDimensions::USER
        .union(RateLimitDimensions::IP)
        .union(RateLimitDimensions::API);
    let config = MultiDimRateLimitConfig {
        dimensions: dims,
        algorithm: RateLimitAlgorithm::SlidingWindow,
        quota: 2,
        window: Duration::from_secs(10),
    };
    let rl = MultiDimRateLimit::new(config);

    let ctx_a = RequestContext::new()
        .with_user("alice")
        .with_ip("10.0.0.1")
        .with_api("/users");
    let ctx_b = RequestContext::new()
        .with_user("bob")
        .with_ip("10.0.0.2")
        .with_api("/orders");

    assert!(rl.check(&ctx_a).is_ok());
    assert!(rl.check(&ctx_a).is_ok());
    assert!(rl.check(&ctx_a).is_err(), "ctx_a should be limited");

    assert!(rl.check(&ctx_b).is_ok(), "ctx_b independent");
}

#[test]
fn test_sliding_window_algorithm() {
    let config = MultiDimRateLimitConfig {
        dimensions: RateLimitDimensions::USER,
        algorithm: RateLimitAlgorithm::SlidingWindow,
        quota: 3,
        window: Duration::from_millis(100),
    };
    let rl = MultiDimRateLimit::new(config);
    let ctx = RequestContext::new().with_user("alice");

    assert!(rl.check(&ctx).is_ok());
    assert!(rl.check(&ctx).is_ok());
    assert!(rl.check(&ctx).is_ok());
    assert!(rl.check(&ctx).is_err(), "4th should be rejected");

    std::thread::sleep(Duration::from_millis(110));
    assert!(rl.check(&ctx).is_ok(), "after window expiry, allow again");
}

#[test]
fn test_leaky_bucket_algorithm() {
    let config = MultiDimRateLimitConfig {
        dimensions: RateLimitDimensions::USER,
        algorithm: RateLimitAlgorithm::LeakyBucket,
        quota: 2,
        window: Duration::from_secs(1),
    };
    let rl = MultiDimRateLimit::new(config);
    let ctx = RequestContext::new().with_user("alice");

    assert!(rl.check(&ctx).is_ok());
    assert!(rl.check(&ctx).is_ok());
    assert!(rl.check(&ctx).is_err(), "bucket full");
}

#[test]
fn test_slow_call_breaker_opens_on_high_slow_rate() {
    let cfg = SlowCallBreakerConfig {
        slow_call_threshold_ms: 100,
        slow_call_rate_threshold: 0.5,
        window: Duration::from_secs(10),
        min_requests: 4,
        max_half_open_probes: 3,
        recovery_timeout: Duration::from_millis(50),
    };
    let breaker = SlowCallBreaker::new(cfg);

    breaker.record_latency(Duration::from_millis(200));
    breaker.record_latency(Duration::from_millis(200));
    breaker.record_latency(Duration::from_millis(200));
    breaker.record_latency(Duration::from_millis(50));

    assert_eq!(breaker.state(), BreakerState::Open);
    assert!(breaker.can_request().is_err());
}

#[test]
fn test_breaker_returns_degrade_response() {
    let cfg = SlowCallBreakerConfig {
        slow_call_threshold_ms: 100,
        slow_call_rate_threshold: 0.5,
        window: Duration::from_secs(10),
        min_requests: 2,
        max_half_open_probes: 3,
        recovery_timeout: Duration::from_millis(50),
    };
    let breaker = SlowCallBreaker::new(cfg);
    let degrade = DegradeResponse::default();

    breaker.record_latency(Duration::from_millis(500));
    breaker.record_latency(Duration::from_millis(500));

    let result = breaker.can_request();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Circuit broken"));
    assert!(degrade.to_error().to_string().contains("degraded"));
}

#[test]
fn test_half_open_probe_limit_max_10() {
    let limit = HalfOpenProbeLimit::new(15);
    assert_eq!(limit.max_probes, 10, "should be clamped to 10");
}

#[test]
fn test_half_open_probes_limited() {
    let cfg = SlowCallBreakerConfig {
        slow_call_threshold_ms: 100,
        slow_call_rate_threshold: 0.5,
        window: Duration::from_secs(10),
        min_requests: 2,
        max_half_open_probes: 3,
        recovery_timeout: Duration::from_millis(10),
    };
    let breaker = SlowCallBreaker::new(cfg);

    breaker.record_latency(Duration::from_millis(200));
    breaker.record_latency(Duration::from_millis(200));
    assert_eq!(breaker.state(), BreakerState::Open);

    std::thread::sleep(Duration::from_millis(15));
    assert!(breaker.can_request().is_ok(), "probe 1");
    assert!(breaker.can_request().is_ok(), "probe 2");
    assert!(breaker.can_request().is_ok(), "probe 3");
    assert!(breaker.can_request().is_err(), "probe 4 rejected");
}

#[test]
fn test_half_open_success_closes_breaker() {
    let cfg = SlowCallBreakerConfig {
        slow_call_threshold_ms: 100,
        slow_call_rate_threshold: 0.5,
        window: Duration::from_secs(10),
        min_requests: 2,
        max_half_open_probes: 3,
        recovery_timeout: Duration::from_millis(10),
    };
    let breaker = SlowCallBreaker::new(cfg);

    breaker.record_latency(Duration::from_millis(200));
    breaker.record_latency(Duration::from_millis(200));

    std::thread::sleep(Duration::from_millis(15));
    breaker.can_request().unwrap();
    breaker.record_latency(Duration::from_millis(50));
    breaker.can_request().unwrap();
    breaker.record_latency(Duration::from_millis(50));
    breaker.can_request().unwrap();
    breaker.record_latency(Duration::from_millis(50));

    assert_eq!(breaker.state(), BreakerState::Closed);
}

#[test]
fn test_half_open_failure_reopens_breaker() {
    let cfg = SlowCallBreakerConfig {
        slow_call_threshold_ms: 100,
        slow_call_rate_threshold: 0.5,
        window: Duration::from_secs(10),
        min_requests: 2,
        max_half_open_probes: 3,
        recovery_timeout: Duration::from_millis(10),
    };
    let breaker = SlowCallBreaker::new(cfg);

    breaker.record_latency(Duration::from_millis(200));
    breaker.record_latency(Duration::from_millis(200));

    std::thread::sleep(Duration::from_millis(15));
    breaker.can_request().unwrap();
    breaker.record_latency(Duration::from_millis(500));

    assert_eq!(breaker.state(), BreakerState::Open);
}

#[test]
fn test_degrade_response_custom() {
    let dr = DegradeResponse::new(429, r#"{"error":"rate_limited"}"#);
    assert_eq!(dr.status, 429);
    assert!(dr.body.contains("rate_limited"));
}
