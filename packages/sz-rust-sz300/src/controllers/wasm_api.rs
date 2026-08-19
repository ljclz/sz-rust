//! WASM 边缘计算控制器 — 提供 WASM 模块执行 HTTP 端点

use axum::body::Body;
use axum::extract::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_core::controller::SzController;
use sz_rust_wasm::{WasmRuntime, WasmValue};

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
                let module = match runtime.compile(&wasm_bytes) {
                    Ok(m) => m,
                    Err(e) => {
                        return ctrl.render_error("WASM 编译失败", json!({"err": e.to_string()}), 0)
                    }
                };

                match module.execute(&func_name, &args) {
                    Ok(results) => ctrl.render_success("success", json!({"results": results})),
                    Err(e) => ctrl.render_error("WASM 执行失败", json!({"err": e.to_string()}), 0),
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
