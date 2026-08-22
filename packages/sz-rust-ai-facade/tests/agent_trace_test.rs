//! AgentTrace / AgentStep / TerminateReason 单元测试

use sz_rust_ai_facade::agent::trace::{AgentStep, AgentTrace, TerminateReason};
use sz_rust_ai_facade::llm::provider::ToolCall;

#[test]
fn agent_trace_new_defaults() {
    let t = AgentTrace::new();
    assert!(t.steps.is_empty());
    assert_eq!(t.total_tokens, 0);
    assert_eq!(t.total_duration_ms, 0);
    assert_eq!(t.terminated_by, TerminateReason::Natural);
}

#[test]
fn agent_trace_default_equals_new() {
    let t1 = AgentTrace::new();
    let t2 = AgentTrace::default();
    assert_eq!(t1.steps.len(), t2.steps.len());
    assert_eq!(t1.total_tokens, t2.total_tokens);
    assert_eq!(t1.terminated_by, t2.terminated_by);
}

#[test]
fn terminate_reason_serde_snake_case() {
    let pairs = [
        (TerminateReason::Natural, "\"natural\""),
        (TerminateReason::MaxSteps, "\"max_steps\""),
        (TerminateReason::MaxTokens, "\"max_tokens\""),
        (TerminateReason::Timeout, "\"timeout\""),
        (TerminateReason::Error, "\"error\""),
    ];
    for (reason, expected) in pairs {
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, expected);
        let de: TerminateReason = serde_json::from_str(expected).unwrap();
        assert_eq!(de, reason);
    }
}

#[test]
fn agent_trace_serde_roundtrip() {
    let mut t = AgentTrace::new();
    t.steps.push(AgentStep {
        thought: "thinking".into(),
        tool_call: Some(ToolCall {
            id: "tc1".into(),
            name: "search".into(),
            arguments: "{}".into(),
        }),
        tool_result: Some(serde_json::json!({"ok": true})),
        observation: "done".into(),
        duration_ms: 42,
    });
    t.total_tokens = 100;
    t.total_duration_ms = 500;
    t.terminated_by = TerminateReason::MaxSteps;

    let json = serde_json::to_string(&t).unwrap();
    let de: AgentTrace = serde_json::from_str(&json).unwrap();
    assert_eq!(de.steps.len(), 1);
    assert_eq!(de.steps[0].thought, "thinking");
    assert_eq!(de.steps[0].duration_ms, 42);
    assert_eq!(de.total_tokens, 100);
    assert_eq!(de.total_duration_ms, 500);
    assert_eq!(de.terminated_by, TerminateReason::MaxSteps);
}

#[test]
fn agent_step_clone_debug() {
    let s = AgentStep {
        thought: "t".into(),
        tool_call: None,
        tool_result: None,
        observation: "o".into(),
        duration_ms: 1,
    };
    let cloned = s.clone();
    assert_eq!(cloned.thought, s.thought);
    let _dbg = format!("{:?}", s);
}
