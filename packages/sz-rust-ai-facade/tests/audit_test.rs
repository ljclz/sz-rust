//! AuditHttpClient / RateLimitConfig 单元测试

use sz_rust_ai_facade::common::audit::{AuditHttpClient, RateLimitConfig};

#[test]
fn rate_limit_config_default() {
    let cfg = RateLimitConfig::default();
    assert_eq!(cfg.rps, 10);
    assert_eq!(cfg.burst, 20);
}

#[test]
fn audit_http_client_new_and_client() {
    let client = reqwest::Client::new();
    let cfg = RateLimitConfig::default();
    let audit = AuditHttpClient::new(client, cfg);
    // client() 返回内部 reqwest::Client 引用
    let _ = audit.client();
}

#[test]
fn audit_http_client_rate_limit_config_roundtrip() {
    let client = reqwest::Client::new();
    let cfg = RateLimitConfig { rps: 5, burst: 10 };
    let audit = AuditHttpClient::new(client, cfg);
    let got = audit.rate_limit_config();
    assert_eq!(got.rps, 5);
    assert_eq!(got.burst, 10);
}

#[test]
fn audit_http_client_update_rate_limit() {
    let client = reqwest::Client::new();
    let cfg = RateLimitConfig::default();
    let audit = AuditHttpClient::new(client, cfg);

    audit.update_rate_limit(RateLimitConfig {
        rps: 100,
        burst: 200,
    });
    let got = audit.rate_limit_config();
    assert_eq!(got.rps, 100);
    assert_eq!(got.burst, 200);
}

#[test]
fn audit_http_client_check_rate_limit_pass_initially() {
    let client = reqwest::Client::new();
    let cfg = RateLimitConfig { rps: 10, burst: 5 };
    let audit = AuditHttpClient::new(client, cfg);
    // burst=5，前 5 次应放行
    for _ in 0..5 {
        assert!(audit.check_rate_limit("openai").is_ok());
    }
}

#[test]
fn audit_http_client_check_rate_limit_throttle_after_burst() {
    let client = reqwest::Client::new();
    // rps=1, burst=1：第 1 次放行，第 2 次立即调用应被限流
    let cfg = RateLimitConfig { rps: 1, burst: 1 };
    let audit = AuditHttpClient::new(client, cfg);
    assert!(audit.check_rate_limit("openai").is_ok());
    // 立即第 2 次：tokens 不足 1.0，应返回 RateLimited
    let err = audit.check_rate_limit("openai").unwrap_err();
    assert_eq!(err.error_code(), "AI_RATE_LIMITED");
}

#[test]
fn audit_http_client_rate_limit_per_provider_isolation() {
    let client = reqwest::Client::new();
    let cfg = RateLimitConfig { rps: 1, burst: 1 };
    let audit = AuditHttpClient::new(client, cfg);
    // 不同 provider 独立桶
    assert!(audit.check_rate_limit("openai").is_ok());
    assert!(audit.check_rate_limit("claude").is_ok());
    // 同一 provider 第 2 次限流
    assert!(audit.check_rate_limit("openai").is_err());
    assert!(audit.check_rate_limit("claude").is_err());
}

#[test]
fn audit_http_client_update_rate_limit_affects_existing_bucket() {
    let client = reqwest::Client::new();
    let cfg = RateLimitConfig { rps: 1, burst: 1 };
    let audit = AuditHttpClient::new(client, cfg);
    // 先创建桶并耗尽
    assert!(audit.check_rate_limit("openai").is_ok());
    assert!(audit.check_rate_limit("openai").is_err());
    // 更新配置为高配额
    audit.update_rate_limit(RateLimitConfig {
        rps: 1000,
        burst: 1000,
    });
    // 更新后 tokens 仍为旧值（< 1.0），需等待 refill 使 tokens >= 1.0
    std::thread::sleep(std::time::Duration::from_millis(5));
    // 现在 rps=1000，5ms 可 refill 5 个 token，应放行
    assert!(audit.check_rate_limit("openai").is_ok());
}
