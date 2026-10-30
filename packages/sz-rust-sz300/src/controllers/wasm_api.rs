// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//
//! WASM 边缘计算控制器 — 提供 WASM 模块执行 HTTP 端点

use axum::body::Body;
use axum::extract::Request;
use axum::response::Response;
use serde_json::json;
use std::sync::{Arc, OnceLock};
use sz_rust_core::controller::SzController;
use sz_rust_wasm::{WasmRuntime, WasmValue};
use tokio::sync::Semaphore;

/// WASM 执行并发上限（2026-10-28 P1-4）
///
/// `spawn_blocking` 的阻塞线程池有限（默认 512），而 WASM 编译/执行不可中断，
/// 单请求可占用最长 5 秒。不限制并发会让少量恶意请求占满阻塞池，拖垮所有依赖
/// `spawn_blocking` 的路径（如 bcrypt 校验）。按 CPU 核数量级设上限，超出返回 429。
static WASM_LIMITER: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn wasm_limiter() -> Arc<Semaphore> {
    WASM_LIMITER
        .get_or_init(|| {
            let limit = std::env::var("SZ300_WASM_MAX_CONCURRENCY")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(4);
            Arc::new(Semaphore::new(limit))
        })
        .clone()
}

struct WasmController;
impl SzController for WasmController {}

impl WasmController {
    /// 执行 WASM 模块函数
    ///
    /// POST /api/wasm/execute
    /// Body: { "wasm": "<base64>", "function": "add", "args": [{"I32": 1}, {"I32": 2}] }
    async fn execute(req: Request<Body>) -> Response {
        let ctrl = WasmController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let wasm_b64 = match data.get("wasm").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => return ctrl.render_error("缺少 wasm 字段", json!({}), 0),
                };
                let func_name = match data.get("function").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => return ctrl.render_error("缺少 function 字段", json!({}), 0),
                };
                let wasm_bytes = match base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    wasm_b64,
                ) {
                    Ok(b) => b,
                    Err(e) => {
                        return ctrl.render_error(
                            "base64 解码失败",
                            json!({"err": e.to_string()}),
                            0,
                        )
                    }
                };

                let args: Vec<WasmValue> = data
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| {
                                if let Some(i) = v.get("I32").and_then(|x| x.as_i64()) {
                                    Some(WasmValue::I32(i as i32))
                                } else {
                                    v.get("I64").and_then(|x| x.as_i64()).map(WasmValue::I64)
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let runtime = WasmRuntime::new();
                let func_name = func_name.to_string();

                // 并发限流：许可随闭包移动，超时后阻塞任务仍在跑时不会提前释放名额
                let permit = match wasm_limiter().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        return ctrl.render_error("WASM 执行并发已满，请稍后重试", json!({}), 429)
                    }
                };

                // 2026-10-27 P0-1 安全修复：
                // WASM 编译与执行为 CPU 密集型同步操作，移入 spawn_blocking 避免阻塞
                // tokio worker；并用 5 秒超时兜底（配合 wasm 运行时燃料计量，无限循环
                // 模块会因 OutOfFuel 快速失败）。
                let task = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let module = runtime.compile(&wasm_bytes)?;
                    module.execute(&func_name, &args)
                });

                match tokio::time::timeout(std::time::Duration::from_secs(5), task).await {
                    // 三层 Result：timeout 结果 → JoinHandle 结果（JoinError）→ 闭包结果
                    Ok(Ok(Ok(results))) => {
                        ctrl.render_success("success", json!({"results": results}))
                    }
                    Ok(Ok(Err(e))) => {
                        ctrl.render_error("WASM 执行失败", json!({"err": e.to_string()}), 0)
                    }
                    Ok(Err(e)) => {
                        ctrl.render_error("WASM 执行任务异常", json!({"err": e.to_string()}), 0)
                    }
                    Err(_) => ctrl.render_error("WASM 执行超时", json!({"err": "timeout"}), 0),
                }
            }
            Err(e) => ctrl.render_error(&e, json!({}), 0),
        }
    }
}

/// POST /api/wasm/execute — 执行 WASM 模块函数
pub async fn execute(req: Request<Body>) -> Response {
    WasmController::execute(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_wasm_base64() -> String {
        let wasm = wat::parse_str(
            r#"
            (module
                (func (export "add") (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.add)
            )
        "#,
        )
        .unwrap();
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &wasm)
    }

    #[tokio::test]
    async fn test_wasm_execute_add() {
        let wasm_b64 = add_wasm_base64();
        let body = serde_json::json!({
            "wasm": wasm_b64,
            "function": "add",
            "args": [{"I32": 1}, {"I32": 2}]
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/wasm/execute")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let res = execute(req).await;
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_wasm_execute_missing_wasm() {
        let body = r#"{"function": "add", "args": []}"#;
        let req = Request::builder()
            .method("POST")
            .uri("/api/wasm/execute")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let res = execute(req).await;
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }
}
