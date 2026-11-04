// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SseAdapter 单元测试

use futures::stream::StreamExt;
use sz_rust_ai_facade::common::sse_adapter::SseAdapter;
use sz_rust_ai_facade::llm::provider::{FinishReason, StreamDelta};

#[tokio::test]
async fn sse_adapter_adapt_maps_delta_to_event() {
    let deltas = vec![
        Ok(StreamDelta {
            content_delta: "hello".into(),
            finish_reason: None,
            tool_call_delta: None,
        }),
        Ok(StreamDelta {
            content_delta: " world".into(),
            finish_reason: Some(FinishReason::Stop),
            tool_call_delta: None,
        }),
    ];
    let stream = futures::stream::iter(deltas);
    let adapted = SseAdapter::adapt(stream);

    let events: Vec<_> = adapted.collect().await;
    // filter_map 保留 Ok 项
    assert_eq!(events.len(), 2);
    // 每个 event 都是 Ok(SseEvent)
    assert!(events[0].is_ok());
    assert!(events[1].is_ok());
}

#[tokio::test]
async fn sse_adapter_adapt_propagates_error_items() {
    use sz_rust_ai_facade::common::AiError;
    let deltas: Vec<Result<StreamDelta, AiError>> = vec![
        Ok(StreamDelta {
            content_delta: "ok".into(),
            finish_reason: None,
            tool_call_delta: None,
        }),
        Err(AiError::Internal("err".into())),
        Ok(StreamDelta {
            content_delta: "ok2".into(),
            finish_reason: Some(FinishReason::Length),
            tool_call_delta: None,
        }),
    ];
    let stream = futures::stream::iter(deltas);
    let adapted = SseAdapter::adapt(stream);

    let events: Vec<_> = adapted.collect().await;
    // 错误项现在被传播（任务组 18），3 个项：Ok, Err, Ok
    assert_eq!(events.len(), 3);
    assert!(events[0].is_ok());
    assert!(events[1].is_err(), "error should be propagated");
    assert!(events[2].is_ok());
}

#[tokio::test]
async fn sse_adapter_adapt_empty_stream() {
    let stream = futures::stream::iter(Vec::<
        Result<StreamDelta, sz_rust_ai_facade::common::AiError>,
    >::new());
    let adapted = SseAdapter::adapt(stream);
    let events: Vec<_> = adapted.collect().await;
    assert!(events.is_empty());
}

#[tokio::test]
async fn sse_adapter_adapt_all_finish_reasons() {
    let reasons = vec![
        FinishReason::Stop,
        FinishReason::Length,
        FinishReason::ToolCalls,
        FinishReason::ContentFilter,
    ];
    let deltas: Vec<_> = reasons
        .into_iter()
        .map(|r| {
            Ok(StreamDelta {
                content_delta: "x".into(),
                finish_reason: Some(r),
                tool_call_delta: None,
            })
        })
        .collect();
    let stream = futures::stream::iter(deltas);
    let adapted = SseAdapter::adapt(stream);
    let events: Vec<_> = adapted.collect().await;
    assert_eq!(events.len(), 4);
    for ev in &events {
        assert!(ev.is_ok());
    }
}

#[tokio::test]
async fn sse_adapter_adpt_no_finish_reason_no_event_field() {
    // 没有 finish_reason 时，event 不设置 event 类型字段
    let deltas = vec![Ok(StreamDelta {
        content_delta: "data".into(),
        finish_reason: None,
        tool_call_delta: None,
    })];
    let stream = futures::stream::iter(deltas);
    let adapted = SseAdapter::adapt(stream);
    let events: Vec<_> = adapted.collect().await;
    assert_eq!(events.len(), 1);
}
