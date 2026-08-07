//! HTTP 请求/响应处理 — 基于 Web Fetch API
//!
//! 提供 WASM 环境下的 HTTP 请求和响应构建能力。

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

// ============================================================================
// HTTP 请求
// ============================================================================

/// 使用 Fetch API 发送 HTTP 请求并返回 JSON 响应
///
/// # 参数
///
/// - `url`: 请求 URL
/// - `method`: HTTP 方法（"GET"、"POST" 等）
/// - `body`: 请求体（可选，JSON 字符串）
///
/// # 返回
///
/// 成功返回 JSON 字符串，失败返回 JsValue 错误
pub async fn fetch_json(url: &str, method: &str, body: Option<&str>) -> Result<String, JsValue> {
    let opts = web_sys::RequestInit::new();
    opts.set_method(method);

    if let Some(body_str) = body {
        opts.set_body(&JsValue::from_str(body_str));
    }

    let request = web_sys::Request::new_with_str_and_init(url, &opts)?;

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request)).await?;
    let resp: web_sys::Response = resp_value.dyn_into()?;

    let text = JsFuture::from(resp.text()?).await?;
    Ok(text.as_string().unwrap_or_default())
}

// ============================================================================
// HTTP 响应
// ============================================================================

/// HTTP 响应构建器
pub struct HttpResponse {
    /// 响应体（JSON 字符串）
    pub body: String,
    /// HTTP 状态码
    pub status: u16,
    /// Content-Type
    pub content_type: String,
}

impl HttpResponse {
    /// 创建 JSON 响应
    pub fn json(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            status: 200,
            content_type: "application/json".to_string(),
        }
    }

    /// 创建成功响应
    pub fn ok(data: &impl serde::Serialize) -> Self {
        let body = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
        Self::json(body)
    }

    /// 创建错误响应
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        let body = serde_json::json!({ "error": message.into() }).to_string();
        Self {
            body,
            status,
            content_type: "application/json".to_string(),
        }
    }

    /// 设置状态码
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// 转换为 `web_sys::Response`
    pub fn to_web_response(&self) -> Result<web_sys::Response, JsValue> {
        let init = web_sys::ResponseInit::new();
        init.set_status(self.status);

        let response = web_sys::Response::new_with_opt_str_and_init(Some(&self.body), &init)?;

        let headers = response.headers();
        headers.set("Content-Type", &self.content_type)?;

        Ok(response)
    }
}

// ============================================================================
// 请求处理入口
// ============================================================================

/// 处理 HTTP 请求 — 通用入口函数
///
/// 接收 `web_sys::Request`，返回 `web_sys::Response`。
/// 业务侧可在此基础上实现路由分发。
pub async fn handle_request(req: web_sys::Request) -> Result<web_sys::Response, JsValue> {
    let url = req.url();
    let method = req.method();

    let response = if method == "GET" && url.contains("/health") {
        HttpResponse::ok(&serde_json::json!({ "status": "ok" }))
    } else if method == "GET" && url.contains("/info") {
        HttpResponse::ok(&serde_json::json!({
            "runtime": "wasm",
            "version": env!("CARGO_PKG_VERSION"),
        }))
    } else {
        HttpResponse::error(404, "Not Found")
    };

    response.to_web_response()
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_response_json() {
        let resp = HttpResponse::json(r#"{"hello":"world"}"#);
        assert_eq!(resp.status, 200);
        assert_eq!(resp.content_type, "application/json");
        assert_eq!(resp.body, r#"{"hello":"world"}"#);
    }

    #[test]
    fn test_http_response_ok() {
        let data = serde_json::json!({ "status": "ok" });
        let resp = HttpResponse::ok(&data);
        assert_eq!(resp.status, 200);
        assert!(resp.body.contains("ok"));
    }

    #[test]
    fn test_http_response_error() {
        let resp = HttpResponse::error(404, "Not Found");
        assert_eq!(resp.status, 404);
        assert!(resp.body.contains("Not Found"));
    }

    #[test]
    fn test_http_response_with_status() {
        let resp = HttpResponse::json("{}").with_status(201);
        assert_eq!(resp.status, 201);
    }
}