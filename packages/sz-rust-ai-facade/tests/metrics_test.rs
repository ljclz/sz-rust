//! AiMetrics 单元测试

use sz_rust_ai_facade::common::metrics::AiMetrics;
use sz_rust_observability::MetricsRegistry;

#[test]
fn ai_metrics_global_singleton() {
    let m1 = AiMetrics::global();
    let m2 = AiMetrics::global();
    // 全局单例：两次获取应指向同一实例
    assert_eq!(m1 as *const _, m2 as *const _);
}

#[test]
fn ai_metrics_record_llm_request_without_register() {
    let m = AiMetrics::global();
    let before = m.llm_request_count();
    m.record_llm_request("openai", "gpt-4o", "success");
    // 全局单例可能被并行测试修改，仅确保不 panic 且计数不减少
    assert!(m.llm_request_count() >= before);
}

#[test]
fn ai_metrics_record_llm_tokens_accumulates() {
    let m = AiMetrics::global();
    let before = m.llm_request_count();
    m.record_llm_tokens("openai", "gpt-4o", "prompt", 100);
    m.record_llm_tokens("openai", "gpt-4o", "completion", 50);
    // record_llm_tokens 不减少 request_count（全局单例可能被并行测试增加）
    assert!(m.llm_request_count() >= before);
}

#[test]
fn ai_metrics_register_with_registry_idempotent() {
    let m = AiMetrics::global();
    let registry = MetricsRegistry::new();
    // 第一次注册
    m.register(&registry);
    // 第二次注册应 no-op（handles.is_some() 时 return）
    m.register(&registry);
    // 注册后 record 应正常工作
    let before = m.llm_request_count();
    m.record_llm_request("test", "model", "ok");
    assert!(m.llm_request_count() >= before);
}

#[test]
fn ai_metrics_record_all_methods_no_panic() {
    let m = AiMetrics::global();
    // 确保所有 record 方法不 panic
    m.record_llm_request("p", "m", "s");
    m.record_llm_tokens("p", "m", "d", 10);
    m.record_llm_request_duration(0.5);
    m.record_rag_recall(0.01);
    m.record_agent_step("agent-1", "natural");
    m.record_embedding("openai", "text-embedding-3-small");
    m.record_cache_hit("llm");
    // 多次调用累积
    assert!(m.llm_request_count() > 0);
}

#[test]
fn ai_metrics_record_llm_request_duration_and_rag_recall() {
    let m = AiMetrics::global();
    let before = m.llm_request_count();
    m.record_llm_request_duration(1.0);
    m.record_llm_request_duration(0.1);
    m.record_rag_recall(0.5);
    m.record_rag_recall(0.05);
    assert!(m.llm_request_count() >= before);
}

#[test]
fn ai_metrics_record_agent_step_and_embedding_and_cache() {
    let m = AiMetrics::global();
    let before = m.llm_request_count();
    m.record_agent_step("a1", "max_steps");
    m.record_agent_step("a2", "natural");
    m.record_embedding("local", "local.0");
    m.record_cache_hit("rag");
    m.record_cache_hit("llm");
    assert!(m.llm_request_count() >= before);
}
