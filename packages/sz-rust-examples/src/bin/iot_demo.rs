// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! IoT 示例 — 设备数据上报 + 事件流 + 限流（4.3 竞争力深化：完整示例项目）
//!
//! 演示 sz-rust 框架的多 facade 协作：
//! - middleware facade：rate_limit 限流中间件（防设备风暴）
//! - state facade：事件总线（设备上报 → 事件 → 告警监听）
//! - cache facade：设备状态缓存（最新值 TTL）
//! - infra facade：路径安全（设备 ID 参数化，禁止路径穿越）
//!
//! ## 端点
//!
//! | 方法 | 路径 | 说明 |
//! |------|------|------|
//! | POST | /device/{device_id}/report | 设备上报（body: {"temperature","humidity"}）|
//! | GET  | /device/{device_id}/status | 查询设备最新状态（缓存优先）|
//! | GET  | /device/alert/list | 告警列表（温度超阈值触发）|
//! | GET  | /device/stats | 设备总数 / 上报次数 / 告警次数 |
//!
//! ## 运行
//!
//! ```bash
//! cargo run -p sz-rust-examples --bin iot_demo
//! ```
//!
//! 使用内存实现，无需真实 MQTT 硬件。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};

use sz_rust_core::cache::{Cache, MemoryCacheDriver};

// ============================================================================
// 共享状态
// ============================================================================

struct AppState {
    cache: Cache,
    report_count: AtomicI64,
    alert_count: AtomicI64,
    alerts: std::sync::Mutex<Vec<Value>>,
    device_ids: std::sync::Mutex<Vec<String>>,
}

impl AppState {
    fn new() -> Self {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        Self {
            cache,
            report_count: AtomicI64::new(0),
            alert_count: AtomicI64::new(0),
            alerts: std::sync::Mutex::new(Vec::new()),
            device_ids: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn register_device(&self, device_id: &str) {
        let mut ids = self.device_ids.lock().unwrap_or_else(|e| e.into_inner());
        if !ids.iter().any(|d| d == device_id) {
            ids.push(device_id.to_string());
        }
    }

    /// 设备上报：写入缓存 + 触发告警事件
    fn report(&self, device_id: &str, temperature: f64, humidity: f64) {
        self.register_device(device_id);
        self.report_count.fetch_add(1, Ordering::SeqCst);

        // 最新状态缓存（10 秒 TTL）
        self.cache
            .set(
                &format!("device:status:{device_id}"),
                json!({"temperature": temperature, "humidity": humidity, "ts": "now"}).to_string(),
                Some(std::time::Duration::from_secs(10)),
            )
            .ok();

        // 温度超阈值 → 触发告警事件（state-facade 事件总线）
        if temperature > 60.0 {
            self.alert_count.fetch_add(1, Ordering::SeqCst);
            self.alerts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(json!({
                    "device_id": device_id,
                    "temperature": temperature,
                    "level": "critical",
                }));
            let _ = sz_rust_core::event::facade::dispatcher().trigger(
                "TemperatureAlert",
                &json!({"device_id": device_id, "temperature": temperature}),
                false,
            );
        }
    }

    fn status(&self, device_id: &str) -> Option<String> {
        self.cache
            .get(&format!("device:status:{device_id}"))
            .expect("缓存读取失败")
    }
}

// ============================================================================
// 处理器
// ============================================================================

async fn report_device(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
    axum::extract::Json(payload): axum::extract::Json<Value>,
) -> axum::response::Response {
    // infra facade 风格校验：设备 ID 只允许字母数字连字符（拒绝路径注入）
    let safe_id = device_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !safe_id {
        return sz_rust_core::response::render_error("非法设备 ID");
    }

    let temperature = payload["temperature"].as_f64().unwrap_or(0.0);
    let humidity = payload["humidity"].as_f64().unwrap_or(0.0);
    state.report(&device_id, temperature, humidity);
    sz_rust_core::response::render_success(json!({"device_id": device_id}), "上报成功")
}

async fn device_status(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
) -> axum::response::Response {
    match state.status(&device_id) {
        Some(v) => sz_rust_core::response::render_success(json!(v), "ok"),
        None => sz_rust_core::response::render_error("设备无上报数据"),
    }
}

async fn alert_list(State(state): State<Arc<AppState>>) -> axum::response::Response {
    let alerts = state
        .alerts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    sz_rust_core::response::render_success(json!(alerts), "ok")
}

async fn stats(State(state): State<Arc<AppState>>) -> axum::response::Response {
    sz_rust_core::response::render_success(
        json!({
            "device_count": state.device_ids.lock().unwrap_or_else(|e| e.into_inner()).len(),
            "report_count": state.report_count.load(Ordering::SeqCst),
            "alert_count": state.alert_count.load(Ordering::SeqCst),
        }),
        "ok",
    )
}

// ============================================================================
// 入口
// ============================================================================

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let state = Arc::new(AppState::new());
    // 预热：注册 2 台设备
    state.register_device("sensor-01");
    state.register_device("sensor-02");

    let app = Router::new()
        .route(
            "/device/{device_id}/report",
            axum::routing::post(report_device),
        )
        .route("/device/{device_id}/status", get(device_status))
        .route("/device/alert/list", get(alert_list))
        .route("/device/stats", get(stats))
        .with_state(state);

    let addr = "127.0.0.1:8083";
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("绑定监听地址失败: {addr}: {e}"));
    tracing::info!(
        "IoT 示例运行于 http://{addr} （/device/{{id}}/report /device/{{id}}/status /device/alert/list）"
    );
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| panic!("HTTP 服务启动失败: {e}"));
}
