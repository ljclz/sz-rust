//! Capability 控制器 — 能力注册表查询接口
//!
//! 提供 `/api/v1/capabilities/list` 端点，列出 Capability Registry 中已注册的能力。
//! 用于验证 Capability Registry 已接入生产，并供 AI Agent 发现可用能力。

use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_core::controller::SzController;

struct CapabilityController;
impl SzController for CapabilityController {}

impl CapabilityController {
    /// 列出所有已注册能力（按标签过滤，可选）
    async fn list(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = CapabilityController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let tag = data.get("tag").and_then(|v| v.as_str());
                let caps = if let Some(t) = tag {
                    state.capability_registry.find_by_tags(&[t], None)
                } else {
                    state.capability_registry.list_all()
                };
                let items: Vec<serde_json::Value> = caps
                    .iter()
                    .map(|cap| {
                        json!({
                            "name": cap.name(),
                            "description": cap.description(),
                            "tags": cap.tags(),
                            "source": format!("{:?}", cap.source()),
                        })
                    })
                    .collect();
                ctrl.render_success(
                    "success",
                    json!({
                        "total": items.len(),
                        "capabilities": items,
                    }),
                )
            }
            Err(e) => ctrl.render_error(&e, json!({}), 0),
        }
    }
}

/// 能力列表接口
#[tracing::instrument(skip(state, req))]
pub async fn list(State(state): State<AppState>, req: Request<Body>) -> Response {
    CapabilityController::list(&state, req).await
}
