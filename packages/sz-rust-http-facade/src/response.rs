// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 响应模块 — renderJson/renderSuccess/renderError + ApiResponse
//!
//! 对齐 PHP `SzController::renderJson` / `renderSuccess` / `renderError`。
//! 响应格式：`{ "code": 1, "msg": "", "data": {} }`（字段顺序 code→msg→data）。
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | 行为 | Rust 等价 |
//! |---------|------|-----------|
//! | `renderJson($code, $msg, $data)` | 标准 JSON 响应 | [`ApiResponse::new`] + `ApiResponse::into_response` |
//! | `renderSuccess($msg, $data)` | `code=1` 成功响应 | [`ApiResponse::success`]（Rust 参数顺序：data, msg） |
//! | `renderError($msg, $data)` | `code=0` 失败响应 | [`ApiResponse::error`] |
//! | `renderError($msg, $data, $code)` | 自定义错误码 | [`ApiResponse::error_with_code`]（Rust 参数顺序：code, msg, data） |
//!
//! ## 字段顺序
//!
//! 严格遵循 PHP `renderJson` 的字段顺序：`code → msg → data`。
//! Rust 使用 `serde_json::Map`（`preserve_order` feature）来保证序列化顺序。
//! `serde_json` 默认启用 `preserve_order`，依赖 `indexmap`。
//!
//! ## Content-Type
//!
//! 所有响应自动附带 `Content-Type: application/json; charset=utf-8`。
//!
//! ## HTTP 状态码
//!
//! 业务成功（`code=1`）和业务失败（`code=0`）都返回 HTTP 200（对齐 PHP 行为）；
//! 异常场景（500/404）由 1.9 错误处理模块处理。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Map, Value};

/// 标准 API 响应结构体
///
/// 严格对齐 PHP `renderJson` 输出格式：`{code, msg, data}`，字段顺序固定。
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_http_facade::response::ApiResponse;
/// use serde_json::json;
///
/// // 成功响应
/// let resp = ApiResponse::success(json!({"id": 1}), "ok");
///
/// // 错误响应
/// let resp = ApiResponse::error("参数错误");
///
/// // 自定义 code
/// let resp = ApiResponse::new(-1, "未登录", json!({}));
/// ```
#[derive(Debug, Clone)]
pub struct ApiResponse {
    /// 业务状态码（1=成功，0=失败，-1=未登录，与 PHP BaseException 对齐）
    pub code: i32,
    /// 业务消息
    pub msg: String,
    /// 业务数据
    pub data: Value,
}

impl ApiResponse {
    /// 创建新的 ApiResponse
    pub fn new(code: i32, msg: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            msg: msg.into(),
            data,
        }
    }

    /// 创建成功响应（code=1）
    ///
    /// 对齐 PHP `renderSuccess($data = [], $msg = '')`。
    pub fn success(data: Value, msg: impl Into<String>) -> Self {
        Self::new(1, msg, data)
    }

    /// 创建成功响应（默认空 data + 空 msg）
    pub fn success_empty() -> Self {
        Self::success(Value::Object(Map::new()), "")
    }

    /// 创建错误响应（code=0）
    ///
    /// 对齐 PHP `renderError($msg = '', $data = [])`。
    pub fn error(msg: impl Into<String>) -> Self {
        Self::new(0, msg, Value::Object(Map::new()))
    }

    /// 创建带数据的错误响应（code=0）
    pub fn error_with_data(msg: impl Into<String>, data: Value) -> Self {
        Self::new(0, msg, data)
    }

    /// 创建带自定义错误码的错误响应
    ///
    /// 对齐 PHP `renderError($code, $msg, $data)`。
    pub fn error_with_code(code: i32, msg: impl Into<String>, data: Value) -> Self {
        Self::new(code, msg, data)
    }

    /// 序列化为 `serde_json::Value`（保证字段顺序 code → msg → data）
    ///
    /// 使用 `serde_json::Map`（启用 `preserve_order`）保证插入顺序。
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("code".to_string(), Value::Number(self.code.into()));
        map.insert("msg".to_string(), Value::String(self.msg.clone()));
        map.insert("data".to_string(), self.data.clone());
        Value::Object(map)
    }

    /// 序列化为 JSON 字符串
    pub fn to_json_string(&self) -> String {
        self.to_value().to_string()
    }

    /// 序列化为 `bytes::Bytes`（零拷贝引用计数字节容器）
    ///
    /// P3 优化：替代 `to_json_string` 的 String 分配，
    /// 使用 `serde_json::to_vec` 序列化到 `Vec<u8>` 后转为 `Bytes`，
    /// 避免 String 的 UTF-8 验证开销，支持零拷贝传递。
    ///
    /// 输出与 `to_json_string` 逐字节一致。
    pub fn to_json_bytes(&self) -> bytes::Bytes {
        let vec = serde_json::to_vec(&self.to_value())
            .expect("ApiResponse::to_json_bytes: serde_json::to_vec infallible for Value");
        bytes::Bytes::from(vec)
    }
}

impl Serialize for ApiResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_value().serialize(serializer)
    }
}

/// 让 ApiResponse 可以直接作为 axum handler 返回值
///
/// 自动设置：
/// - HTTP 状态码：200（无论业务 code 是 1 还是 0，HTTP 都是 200，对齐 PHP 行为）
/// - Content-Type: `application/json; charset=utf-8`
impl IntoResponse for ApiResponse {
    fn into_response(self) -> Response {
        let body = self.to_json_string();
        (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "application/json; charset=utf-8",
            )],
            body,
        )
            .into_response()
    }
}

/// 直接构建标准 JSON 响应（无需创建 ApiResponse 实例）
///
/// 对齐 PHP `renderJson($code, $msg, $data)`。
#[tracing::instrument(skip(msg, data))]
pub fn render_json(code: i32, msg: impl Into<String>, data: Value) -> Response {
    ApiResponse::new(code, msg, data).into_response()
}

/// 构建成功响应
///
/// 对齐 PHP `renderSuccess($data, $msg)`。
#[tracing::instrument(skip(data, msg))]
pub fn render_success(data: Value, msg: impl Into<String>) -> Response {
    ApiResponse::success(data, msg).into_response()
}

/// 构建错误响应
///
/// 对齐 PHP `renderError($msg, $data)`。
#[tracing::instrument(skip(msg))]
pub fn render_error(msg: impl Into<String>) -> Response {
    ApiResponse::error(msg).into_response()
}

/// 构建带自定义错误码的错误响应
///
/// 对齐 PHP `renderError($code, $msg, $data)`。
#[tracing::instrument(skip(msg, data))]
pub fn render_error_with_code(code: i32, msg: impl Into<String>, data: Value) -> Response {
    ApiResponse::error_with_code(code, msg, data).into_response()
}

// ============================================================================
// 前后端分离 JSON 默认返回（项目主策略）
//
// 对齐 PHP ThinkPHP 6 `Dispatch::autoResponse()` 行为，但将 JSON 设为默认响应类型
// （项目主策略：前后端分离）。PHP `autoResponse()` 根据 `$this->request->isJson()`
// 判断响应类型：isJson → JSON，否则 → HTML（数组会被输出为字面量 "Array"）。
//
// 本模块扩展三种策略：
// 1. `DefaultResponseType::Json` — 项目主策略：默认返回 JSON（不渲染模板）
// 2. `DefaultResponseType::Html` — 兜底场景：返回 HTML（模板渲染使用）
// 3. `DefaultResponseType::Auto` — 对齐 PHP autoResponse：根据 Accept 头判断
//
// ## PHP 源码参考
//
// ```php
// // vendor/topthink/framework/src/think/route/Dispatch.php:84-107
// protected function autoResponse($data): Response
// {
//     if ($data instanceof Response) {
//         $response = $data;
//     } elseif ($data instanceof ResponseInterface) {
//         $response = Response::create((string) $data->getBody(), 'html', $data->getStatusCode());
//         foreach ($data->getHeaders() as $header => $values) {
//             $response->header([$header => implode(", ", $values)]);
//         }
//     } elseif (!is_null($data)) {
//         // 默认自动识别响应输出类型
//         $type     = $this->request->isJson() ? 'json' : 'html';
//         $response = Response::create($data, $type);
//     } else {
//         $data = ob_get_clean();
//         $content  = false === $data ? '' : $data;
//         $status   = '' === $content && $this->request->isJson() ? 204 : 200;
//         $response = Response::create($content, 'html', $status);
//     }
//     return $response;
// }
// ```
//
// ```php
// // vendor/topthink/framework/src/think/Request.php:1557-1562
// public function isJson(): bool
// {
//     $acceptType = $this->type();
//     return false !== strpos($acceptType, 'json');
// }
// ```
// ============================================================================

use axum::http::{header, HeaderMap};

/// 默认响应类型策略（对齐 PHP `autoResponse` + 项目主策略扩展）
///
/// 项目主策略为前后端分离，因此默认使用 `Json`。`Auto` 严格对齐 PHP TP 6
/// `autoResponse()` 的行为（根据 Accept 头判断）。`Html` 用于兜底场景
/// （如模板渲染、PDF/Excel 导出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DefaultResponseType {
    /// 项目主策略：默认返回 JSON（Content-Type: application/json; charset=utf-8）
    ///
    /// 控制器返回 `Value`（数组/对象）时，自动序列化为 JSON 响应。
    /// 不渲染模板，不检查 Accept 头。
    #[default]
    Json,

    /// 兜底场景：返回 HTML（Content-Type: text/html; charset=utf-8）
    ///
    /// 用于模板渲染、PDF/Excel 导出等非 JSON 场景。
    Html,

    /// 对齐 PHP `autoResponse`：根据请求 `Accept` 头判断
    ///
    /// - Accept 含 `json` MIME → JSON 响应
    /// - Accept 不含 `json` MIME → HTML 响应（数组输出字面量 "Array"，对齐 PHP bug）
    Auto,
}

impl DefaultResponseType {
    /// 根据策略和请求数据生成响应
    ///
    /// # 参数
    /// - `data`：响应数据（`Value::Object` / `Value::Array` / `Value::String` 等）
    /// - `headers`：请求头（用于 `Auto` 策略判断 `Accept` 头）
    ///
    /// # 返回
    /// - `Json` → `respond(data)`
    /// - `Html` → `respond_html(data.to_string())`
    /// - `Auto` → `auto_respond(data, headers)`
    pub fn respond(&self, data: &Value, headers: &HeaderMap) -> Response {
        match self {
            DefaultResponseType::Json => respond(data),
            DefaultResponseType::Html => respond_html(data.to_string()),
            DefaultResponseType::Auto => auto_respond(data, headers),
        }
    }
}

/// 检查请求是否为 JSON 请求（对齐 PHP `Request::isJson()`）
///
/// PHP 逻辑：检查 `Accept` 请求头是否包含 `json` MIME 类型
/// （如 `application/json`、`text/json`、`application/vnd.api+json`）。
///
/// # PHP 对齐
///
/// ```php
/// // vendor/topthink/framework/src/think/Request.php:1557-1562
/// public function isJson(): bool
/// {
///     $acceptType = $this->type();
///     return false !== strpos($acceptType, 'json');
/// }
/// ```
///
/// # 参数
///
/// - `headers`：请求头
///
/// # 返回
///
/// - `true`：`Accept` 头存在且包含 `json` 子串
/// - `false`：`Accept` 头不存在或不包含 `json` 子串
pub fn is_json_request(headers: &HeaderMap) -> bool {
    if let Some(accept) = headers.get(header::ACCEPT) {
        if let Ok(accept_str) = accept.to_str() {
            // 对齐 PHP `strpos($acceptType, 'json') !== false`
            // PHP `type()` 方法从 Accept 头解析 MIME 类型，再检查是否包含 "json"
            // Rust 简化为直接检查 Accept 头是否包含 "json" 子串
            // （覆盖 application/json、text/json、application/vnd.api+json 等）
            return accept_str.to_lowercase().contains("json");
        }
    }
    false
}

/// 默认 JSON 响应（项目主策略：前后端分离）
///
/// 将任意 `Value` 序列化为 JSON 响应，Content-Type 为
/// `application/json; charset=utf-8`，HTTP 状态码 200。
///
/// # 项目主策略
///
/// 项目采用前后端分离架构，所有 API 响应默认为 JSON 格式。
/// 控制器方法可直接返回 `Value`，由本函数统一转换为 JSON 响应。
///
/// # 参数
///
/// - `data`：要序列化的数据（`Value::Object` / `Value::Array` / `Value::String` 等）
///
/// # 返回
///
/// `Response`，HTTP 200，Content-Type: application/json; charset=utf-8
#[tracing::instrument(skip(data))]
pub fn respond(data: &Value) -> Response {
    let body = data.to_string();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body,
    )
        .into_response()
}

/// HTML 响应（兜底场景：模板渲染、PDF/Excel 导出）
///
/// Content-Type 为 `text/html; charset=utf-8`，HTTP 状态码 200。
///
/// # 参数
///
/// - `content`：HTML 内容
///
/// # 返回
///
/// `Response`，HTTP 200，Content-Type: text/html; charset=utf-8
#[tracing::instrument(skip(content))]
pub fn respond_html(content: impl Into<String>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        content.into(),
    )
        .into_response()
}

/// 纯文本响应（Content-Type: text/plain; charset=utf-8）
///
/// 用于调试、健康检查等非 JSON/HTML 场景。
///
/// # 参数
///
/// - `content`：文本内容
///
/// # 返回
///
/// `Response`，HTTP 200，Content-Type: text/plain; charset=utf-8
#[tracing::instrument(skip(content))]
pub fn respond_text(content: impl Into<String>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        content.into(),
    )
        .into_response()
}

/// 自动响应（对齐 PHP `Dispatch::autoResponse()`）
///
/// 严格对齐 PHP ThinkPHP 6 `autoResponse()` 行为：
/// 1. 根据 `is_json_request(headers)` 判断响应类型
/// 2. JSON 请求 → JSON 响应（`respond(data)`）
/// 3. 非 JSON 请求 → HTML 响应
///    - `Value::Array` / `Value::Object` → 字面量 `"Array"`（对齐 PHP bug）
///    - `Value::String` → 字符串内容
///    - 其他类型 → `data.to_string()`
///
/// # PHP bug 复刻说明
///
/// PHP `Response::create($data, 'html')` 在 `$data` 为数组时，会通过 `print` 输出
/// 数组，导致输出字面量 `"Array"`。这是 PHP 的已知行为，本函数严格复刻此 bug
/// 以保证 R5（PHP/Rust 行为对比）一致性。
///
/// # PHP 对齐
///
/// ```php
/// // vendor/topthink/framework/src/think/route/Dispatch.php:96-97
/// $type     = $this->request->isJson() ? 'json' : 'html';
/// $response = Response::create($data, $type);
/// ```
///
/// # 参数
///
/// - `data`：响应数据
/// - `headers`：请求头（用于判断 `Accept` 头）
///
/// # 返回
///
/// - JSON 请求 → JSON 响应
/// - 非 JSON 请求 → HTML 响应（数组输出字面量 `"Array"`）
#[tracing::instrument(skip(data, headers))]
pub fn auto_respond(data: &Value, headers: &HeaderMap) -> Response {
    if is_json_request(headers) {
        // 对齐 PHP: $type = 'json'
        respond(data)
    } else {
        // 对齐 PHP: $type = 'html'
        // PHP bug 复刻：数组/对象输出字面量 "Array"
        let content = match data {
            Value::Array(_) | Value::Object(_) => "Array".to_string(),
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            _ => data.to_string(),
        };
        respond_html(content)
    }
}

/// JSON 响应包装器（项目主策略：默认 JSON 返回）
///
/// 由于 Rust 孤儿规则限制，无法直接为 `serde_json::Value` 实现 `IntoResponse`。
/// 本 newtype 包装 `Value`，使其可以直接作为 axum handler 返回值，
/// 默认返回 JSON 响应（Content-Type: application/json; charset=utf-8）。
///
/// # 用法
///
/// ```ignore
/// use sz_rust_http_facade::response::JsonResponse;
/// use serde_json::json;
///
/// async fn handler() -> JsonResponse {
///     JsonResponse(json!({"id": 1, "name": "alice"}))
/// }
/// ```
///
/// 也可通过 `From<Value>` 转换：
///
/// ```ignore
/// use sz_rust_http_facade::response::JsonResponse;
/// use serde_json::json;
///
/// async fn handler() -> JsonResponse {
///     json!({"id": 1}).into()
/// }
/// ```
///
/// # 注意
///
/// 此类型始终返回 JSON 响应（项目主策略）。若需根据 Accept 头判断，
/// 请使用 [`auto_respond`] 或 [`DefaultResponseType::Auto`]。
#[derive(Debug, Clone)]
pub struct JsonResponse(pub Value);

impl From<Value> for JsonResponse {
    fn from(v: Value) -> Self {
        JsonResponse(v)
    }
}

impl IntoResponse for JsonResponse {
    fn into_response(self) -> Response {
        respond(&self.0)
    }
}

// ============================================================================
// JSONP 响应 — 对齐 PHP `jsonp()` 返回类型
//
// JSONP（JSON with Padding）用于跨域请求，通过 <script> 标签加载。
// 响应格式：`callbackName({...});`
// Content-Type: application/javascript; charset=utf-8
//
// ## 安全说明
//
// - 回调函数名校验：仅允许 `[a-zA-Z0-9_.]` 字符，防止 XSS 注入
// - 对齐 PHP `Response::create($data, 'jsonp')` 行为
// ============================================================================

/// 回调函数名校验正则（仅允许字母、数字、下划线、点）
const JSONP_CALLBACK_PATTERN: &str = r"^[a-zA-Z_][a-zA-Z0-9_.]*$";

/// 校验 JSONP 回调函数名是否合法
///
/// 仅允许 `[a-zA-Z_][a-zA-Z0-9_.]*` 格式，防止 XSS 注入。
///
/// # 参数
///
/// - `callback`：回调函数名
///
/// # 返回
///
/// - `true`：合法
/// - `false`：非法（包含特殊字符或为空）
pub fn is_valid_jsonp_callback(callback: &str) -> bool {
    if callback.is_empty() || callback.len() > 128 {
        return false;
    }
    regex::Regex::new(JSONP_CALLBACK_PATTERN)
        .map(|re| re.is_match(callback))
        .unwrap_or(false)
}

/// 构建 JSONP 响应（对齐 PHP `json()` + `'jsonp'` 类型）
///
/// 将数据序列化为 JSON，包裹在回调函数调用中：`callback({...});`
/// Content-Type 为 `application/javascript; charset=utf-8`。
///
/// # 安全说明
///
/// 回调函数名会经过校验（[`is_valid_jsonp_callback`]），非法名称将返回 400 错误。
///
/// # 参数
///
/// - `callback`：回调函数名（如 `handleResponse`）
/// - `data`：要返回的数据
///
/// # 返回
///
/// `Response`，HTTP 200，Content-Type: application/javascript; charset=utf-8
#[tracing::instrument(skip(data))]
pub fn respond_jsonp(callback: &str, data: &Value) -> Response {
    if !is_valid_jsonp_callback(callback) {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Invalid JSONP callback name".to_string(),
        )
            .into_response();
    }

    let json_str = data.to_string();
    let body = format!("{callback}({json_str});");

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// JSONP 响应包装器
///
/// 由于 Rust 孤儿规则限制，无法直接为 `(String, Value)` 实现 `IntoResponse`。
/// 本 newtype 包装回调函数名和数据，使其可以直接作为 axum handler 返回值。
///
/// # 用法
///
/// ```ignore
/// use sz_rust_http_facade::response::JsonpResponse;
/// use serde_json::json;
///
/// async fn handler(callback: String) -> JsonpResponse {
///     JsonpResponse(callback, json!({"id": 1, "name": "alice"}))
/// }
/// ```
#[derive(Debug, Clone)]
pub struct JsonpResponse(pub String, pub Value);

impl IntoResponse for JsonpResponse {
    fn into_response(self) -> Response {
        respond_jsonp(&self.0, &self.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // ====================================================================
    // ApiResponse 单元测试
    // ====================================================================

    #[test]
    fn test_api_response_new() {
        let resp = ApiResponse::new(1, "ok", Value::Object(Map::new()));
        assert_eq!(resp.code, 1);
        assert_eq!(resp.msg, "ok");
        assert!(resp.data.is_object());
    }

    #[test]
    fn test_api_response_success() {
        let resp = ApiResponse::success(serde_json::json!({"id": 1}), "ok");
        assert_eq!(resp.code, 1);
        assert_eq!(resp.msg, "ok");
        assert_eq!(resp.data["id"], 1);
    }

    #[test]
    fn test_api_response_success_empty() {
        let resp = ApiResponse::success_empty();
        assert_eq!(resp.code, 1);
        assert_eq!(resp.msg, "");
        assert!(resp.data.is_object());
        assert!(resp.data.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_api_response_error() {
        let resp = ApiResponse::error("参数错误");
        assert_eq!(resp.code, 0);
        assert_eq!(resp.msg, "参数错误");
        assert!(resp.data.is_object());
    }

    #[test]
    fn test_api_response_error_with_data() {
        let resp = ApiResponse::error_with_data("失败", serde_json::json!({"field": "name"}));
        assert_eq!(resp.code, 0);
        assert_eq!(resp.msg, "失败");
        assert_eq!(resp.data["field"], "name");
    }

    #[test]
    fn test_api_response_error_with_code() {
        let resp = ApiResponse::error_with_code(-1, "未登录", Value::Object(Map::new()));
        assert_eq!(resp.code, -1);
        assert_eq!(resp.msg, "未登录");
    }

    #[test]
    fn test_api_response_to_value_field_order() {
        let resp = ApiResponse::new(1, "ok", serde_json::json!({"id": 1}));
        let value = resp.to_value();
        let obj = value.as_object().unwrap();

        // 字段顺序必须是 code → msg → data
        let keys: Vec<&String> = obj.keys().collect();
        assert_eq!(keys, vec!["code", "msg", "data"]);
    }

    #[test]
    fn test_api_response_to_value_content() {
        let resp = ApiResponse::new(1, "ok", serde_json::json!({"id": 1, "name": "alice"}));
        let value = resp.to_value();
        assert_eq!(value["code"], 1);
        assert_eq!(value["msg"], "ok");
        assert_eq!(value["data"]["id"], 1);
        assert_eq!(value["data"]["name"], "alice");
    }

    #[test]
    fn test_api_response_to_json_string() {
        let resp = ApiResponse::new(1, "ok", serde_json::json!({}));
        let json_str = resp.to_json_string();
        // 字段顺序必须是 code → msg → data
        let expected = r#"{"code":1,"msg":"ok","data":{}}"#;
        assert_eq!(json_str, expected);
    }

    #[test]
    fn test_api_response_to_json_string_with_data() {
        let resp = ApiResponse::success(serde_json::json!({"id": 1, "name": "alice"}), "ok");
        let json_str = resp.to_json_string();
        let expected = r#"{"code":1,"msg":"ok","data":{"id":1,"name":"alice"}}"#;
        assert_eq!(json_str, expected);
    }

    #[test]
    fn test_api_response_to_json_bytes() {
        let resp = ApiResponse::new(1, "ok", serde_json::json!({}));
        let json_bytes = resp.to_json_bytes();
        let expected = r#"{"code":1,"msg":"ok","data":{}}"#;
        assert_eq!(json_bytes.as_ref(), expected.as_bytes());
    }

    #[test]
    fn test_api_response_to_json_bytes_with_data() {
        let resp = ApiResponse::success(serde_json::json!({"id": 1, "name": "alice"}), "ok");
        let json_bytes = resp.to_json_bytes();
        let expected = r#"{"code":1,"msg":"ok","data":{"id":1,"name":"alice"}}"#;
        assert_eq!(json_bytes.as_ref(), expected.as_bytes());
    }

    #[test]
    fn test_api_response_to_json_bytes_matches_string() {
        // to_json_bytes 输出与 to_json_string 逐字节一致
        let resp = ApiResponse::new(-1, "未登录", serde_json::json!({"token": null}));
        let json_str = resp.to_json_string();
        let json_bytes = resp.to_json_bytes();
        assert_eq!(json_bytes.as_ref(), json_str.as_bytes());
    }

    #[test]
    fn test_api_response_to_json_bytes_empty() {
        let resp = ApiResponse::success_empty();
        let json_bytes = resp.to_json_bytes();
        let expected = r#"{"code":1,"msg":"","data":{}}"#;
        assert_eq!(json_bytes.as_ref(), expected.as_bytes());
    }

    #[test]
    fn test_api_response_serialize_via_serde() {
        let resp = ApiResponse::new(0, "失败", Value::Object(Map::new()));
        let json_str = serde_json::to_string(&resp).unwrap();
        assert_eq!(json_str, r#"{"code":0,"msg":"失败","data":{}}"#);
    }

    #[test]
    fn test_api_response_clone() {
        let resp = ApiResponse::success(serde_json::json!({"id": 1}), "ok");
        let cloned = resp.clone();
        assert_eq!(cloned.code, resp.code);
        assert_eq!(cloned.msg, resp.msg);
        assert_eq!(cloned.data, resp.data);
    }

    #[test]
    fn test_api_response_debug_format() {
        let resp = ApiResponse::new(1, "ok", Value::Object(Map::new()));
        let debug_str = format!("{resp:?}");
        assert!(debug_str.contains("ApiResponse"));
        assert!(debug_str.contains("code: 1"));
        assert!(debug_str.contains("\"ok\""));
    }

    // ====================================================================
    // 便捷函数测试
    // ====================================================================

    #[test]
    fn test_render_json_returns_response() {
        let resp = render_json(1, "ok", serde_json::json!({}));
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn test_render_success_returns_response() {
        let resp = render_success(serde_json::json!({"id": 1}), "ok");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn test_render_error_returns_response() {
        let resp = render_error("参数错误");
        assert_eq!(resp.status(), StatusCode::OK); // 业务错误 HTTP 仍 200
    }

    #[test]
    fn test_render_error_with_code_returns_response() {
        let resp = render_error_with_code(-1, "未登录", serde_json::json!({}));
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ====================================================================
    // 集成测试：通过 axum Router 验证完整响应
    // ====================================================================

    #[tokio::test]
    async fn test_api_response_as_handler_return() {
        async fn handler() -> ApiResponse {
            ApiResponse::success(serde_json::json!({"id": 1, "name": "alice"}), "ok")
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        let json: Value = serde_json::from_str(&body_str).unwrap();

        assert_eq!(json["code"], 1);
        assert_eq!(json["msg"], "ok");
        assert_eq!(json["data"]["id"], 1);
        assert_eq!(json["data"]["name"], "alice");
    }

    #[tokio::test]
    async fn test_render_error_handler_return() {
        async fn handler() -> Response {
            render_error("参数错误")
        }

        let router = axum::Router::new().route("/", axum::routing::post(handler));
        let req = Request::builder()
            .method(Method::POST)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        let json: Value = serde_json::from_str(&body_str).unwrap();

        assert_eq!(json["code"], 0);
        assert_eq!(json["msg"], "参数错误");
        assert!(json["data"].is_object());
    }

    #[tokio::test]
    async fn test_response_body_exact_format() {
        // 严格验证响应体格式：{code,msg,data}，与 PHP renderJson 完全一致
        async fn handler() -> ApiResponse {
            ApiResponse::success_empty()
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body_str, r#"{"code":1,"msg":"","data":{}}"#);
    }

    #[tokio::test]
    async fn test_response_with_complex_data() {
        async fn handler() -> ApiResponse {
            ApiResponse::success(
                serde_json::json!({
                    "list": [{"id": 1}, {"id": 2}],
                    "total": 2,
                    "page": 1,
                    "size": 10
                }),
                "查询成功",
            )
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        let json: Value = serde_json::from_str(&body_str).unwrap();

        assert_eq!(json["code"], 1);
        assert_eq!(json["msg"], "查询成功");
        assert_eq!(json["data"]["total"], 2);
        assert_eq!(json["data"]["list"][0]["id"], 1);
        assert_eq!(json["data"]["list"][1]["id"], 2);
    }

    #[tokio::test]
    async fn test_response_with_various_error_codes() {
        // 验证各种错误码（对齐 PHP BaseException）
        let test_cases = vec![
            (0, "业务失败"),
            (-1, "未登录"),
            (-2, "用户不存在"),
            (-3, "用户被禁用"),
            (403, "禁止访问"),
            (404, "资源不存在"),
            (422, "参数校验失败"),
            (500, "数据库错误"),
        ];

        for (code, msg) in test_cases {
            let resp = ApiResponse::error_with_code(code, msg, Value::Object(Map::new()));
            let json_str = resp.to_json_string();
            let json: Value = serde_json::from_str(&json_str).unwrap();
            assert_eq!(json["code"], code);
            assert_eq!(json["msg"], msg);
        }
    }

    // ====================================================================
    // PHP 一致性测试（R5 硬约束：PHP/Rust 行为对比）
    //
    // 对比 PHP `SzController::renderJson` / `renderSuccess` / `renderError`
    // 与 Rust `ApiResponse` / `render_json` / `render_success` / `render_error`
    // 的行为差异。
    //
    // PHP 源码（e:\vue\test\鲜视达\server\app\SzController.php）：
    //   protected function renderJson($code = 1, $msg = '', $data = [])
    //   {
    //       return compact('code', 'msg', 'data');
    //   }
    //
    //   protected function renderSuccess($msg = 'success', $data = [])
    //   {
    //       return json($this->renderJson(1, $msg, $data));
    //   }
    //
    //   protected function renderError($msg = 'error', $data = [], $code = 0)
    //   {
    //       return json($this->renderJson($code, $msg, $data));
    //   }
    // ====================================================================

    #[test]
    fn test_php_consistency_render_json_compact_field_order() {
        // PHP `renderJson` 通过 `compact('code', 'msg', 'data')` 返回数组，
        // `compact()` 严格按参数顺序保序序列化：code → msg → data。
        // Rust 使用 `serde_json::Map`（preserve_order）保证相同顺序。
        let resp = ApiResponse::new(1, "ok", serde_json::json!({"id": 1}));
        let value = resp.to_value();
        let obj = value.as_object().unwrap();
        let keys: Vec<&String> = obj.keys().collect();
        assert_eq!(
            keys,
            vec!["code", "msg", "data"],
            "字段顺序必须为 code → msg → data（对齐 PHP compact()）"
        );
        assert_eq!(value["code"], 1);
        assert_eq!(value["msg"], "ok");
        assert_eq!(value["data"]["id"], 1);
    }

    #[test]
    fn test_php_consistency_render_json_default_values() {
        // PHP `renderJson()` 默认值：$code=1, $msg='', $data=[]
        // 对齐 PHP：`return compact('code', 'msg', 'data');`
        let resp = ApiResponse::new(1, "", Value::Object(Map::new()));
        let json_str = resp.to_json_string();
        assert_eq!(
            json_str, r#"{"code":1,"msg":"","data":{}}"#,
            "默认值必须与 PHP renderJson() 一致：code=1, msg='', data={{}}"
        );
    }

    #[test]
    fn test_php_consistency_render_success_calls_render_json_with_code_1() {
        // PHP `renderSuccess($msg, $data)` 内部调用 `renderJson(1, $msg, $data)`，
        // 即 code 必须固定为 1。
        let resp = ApiResponse::success(serde_json::json!({"id": 1}), "ok");
        assert_eq!(
            resp.code, 1,
            "renderSuccess 必须 code=1（对齐 PHP renderJson(1, ...)）"
        );
        assert_eq!(resp.msg, "ok");
        assert_eq!(resp.data["id"], 1);

        // 验证完整 JSON 输出格式
        let json_str = resp.to_json_string();
        let expected = r#"{"code":1,"msg":"ok","data":{"id":1}}"#;
        assert_eq!(json_str, expected);
    }

    #[test]
    fn test_php_consistency_render_error_default_code_is_0() {
        // PHP `renderError($msg = 'error', $data = [], $code = 0)` 默认 $code=0
        // 内部调用 `renderJson($code, $msg, $data)`，即默认 code=0
        let resp = ApiResponse::error("参数错误");
        assert_eq!(
            resp.code, 0,
            "renderError 默认 code=0（对齐 PHP 默认参数 $code = 0）"
        );
        assert_eq!(resp.msg, "参数错误");
        assert!(
            resp.data.is_object(),
            "renderError 默认 data 为空对象（对齐 PHP $data = []）"
        );

        // 验证 HTTP 状态码始终为 200（对齐 PHP json() 响应）
        let response = render_error("参数错误");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn test_php_consistency_render_error_with_custom_code_aligns_base_exception() {
        // PHP `renderError($msg, $data, $code)` 支持自定义错误码
        // PHP BaseException 错误码约定：
        //   -1 = 未登录
        //   -2 = 用户不存在
        //   -3 = 用户被禁用
        // Rust 必须能复刻这些错误码
        let test_cases = vec![
            (-1i32, "未登录"),
            (-2, "用户不存在"),
            (-3, "用户被禁用"),
            (0, "业务失败"),
        ];

        for (code, msg) in test_cases {
            let resp = ApiResponse::error_with_code(code, msg, Value::Object(Map::new()));
            let json_str = resp.to_json_string();
            let json: Value = serde_json::from_str(&json_str).unwrap();
            assert_eq!(
                json["code"], code,
                "自定义错误码必须与 PHP BaseException 约定一致"
            );
            assert_eq!(json["msg"], msg);
            // data 字段必须存在（对齐 PHP compact('code', 'msg', 'data')）
            assert!(json.get("data").is_some(), "data 字段必须存在");
        }
    }

    // ====================================================================
    // 前后端分离 JSON 默认返回测试
    //
    // 测试维度：
    // 1. DefaultResponseType 枚举（3 种策略）
    // 2. is_json_request（Accept 头判断）
    // 3. respond（默认 JSON 响应）
    // 4. respond_html（HTML 响应）
    // 5. respond_text（纯文本响应）
    // 6. auto_respond（PHP autoResponse 对齐 + bug 复刻）
    // 7. IntoResponse for Value（Value 直接作为 handler 返回值）
    // 8. R5 PHP/Rust 行为对比（autoResponse + isJson + 数组字面量 "Array" bug）
    // ====================================================================

    // ---------- DefaultResponseType 枚举测试 ----------

    #[test]
    fn test_default_response_type_default_is_json() {
        // 项目主策略：默认为 Json
        let t = DefaultResponseType::default();
        assert_eq!(t, DefaultResponseType::Json);
    }

    #[test]
    fn test_default_response_type_variants_eq() {
        assert_eq!(DefaultResponseType::Json, DefaultResponseType::Json);
        assert_ne!(DefaultResponseType::Json, DefaultResponseType::Html);
        assert_ne!(DefaultResponseType::Json, DefaultResponseType::Auto);
        assert_ne!(DefaultResponseType::Html, DefaultResponseType::Auto);
    }

    #[test]
    fn test_default_response_type_clone_copy() {
        let t = DefaultResponseType::Json;
        let t2 = t; // Copy
        assert_eq!(t, t2);
        // DefaultResponseType 实现 Copy，无需 clone
        let t3 = t;
        assert_eq!(t, t3);
    }

    #[test]
    fn test_default_response_type_debug() {
        let debug = format!("{:?}", DefaultResponseType::Json);
        assert!(debug.contains("Json"));
        let debug = format!("{:?}", DefaultResponseType::Html);
        assert!(debug.contains("Html"));
        let debug = format!("{:?}", DefaultResponseType::Auto);
        assert!(debug.contains("Auto"));
    }

    #[test]
    fn test_default_response_type_respond_json() {
        // Json 策略：始终返回 JSON
        let headers = HeaderMap::new();
        let data = serde_json::json!({"id": 1});
        let resp = DefaultResponseType::Json.respond(&data, &headers);
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn test_default_response_type_respond_html() {
        // Html 策略：始终返回 HTML
        let headers = HeaderMap::new();
        let data = serde_json::json!({"id": 1});
        let resp = DefaultResponseType::Html.respond(&data, &headers);
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn test_default_response_type_respond_html_body() {
        let headers = HeaderMap::new();
        let data = serde_json::json!({"id": 1});
        let resp = DefaultResponseType::Html.respond(&data, &headers);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        // HTML 策略将 Value 序列化为字符串
        assert_eq!(body, r#"{"id":1}"#);
    }

    #[tokio::test]
    async fn test_default_response_type_respond_auto_with_json_accept() {
        // Auto 策略 + Accept: application/json → JSON 响应
        let mut headers = HeaderMap::new();
        headers.insert("accept", "application/json".parse().unwrap());
        let data = serde_json::json!({"id": 1});
        let resp = DefaultResponseType::Auto.respond(&data, &headers);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn test_default_response_type_respond_auto_with_html_accept() {
        // Auto 策略 + Accept: text/html → HTML 响应（数组字面量 "Array" bug 复刻）
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        let data = serde_json::json!({"id": 1});
        let resp = DefaultResponseType::Auto.respond(&data, &headers);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        // PHP bug 复刻：对象输出字面量 "Array"
        assert_eq!(body, "Array");
    }

    // ---------- is_json_request 测试 ----------

    #[test]
    fn test_is_json_request_with_application_json() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", "application/json".parse().unwrap());
        assert!(is_json_request(&headers));
    }

    #[test]
    fn test_is_json_request_with_text_json() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/json".parse().unwrap());
        assert!(is_json_request(&headers));
    }

    #[test]
    fn test_is_json_request_with_vnd_api_json() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", "application/vnd.api+json".parse().unwrap());
        assert!(is_json_request(&headers));
    }

    #[test]
    fn test_is_json_request_with_wildcard() {
        // Accept: */* 不包含 "json" 子串，应返回 false
        let mut headers = HeaderMap::new();
        headers.insert("accept", "*/*".parse().unwrap());
        assert!(!is_json_request(&headers));
    }

    #[test]
    fn test_is_json_request_with_text_html() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        assert!(!is_json_request(&headers));
    }

    #[test]
    fn test_is_json_request_no_accept_header() {
        let headers = HeaderMap::new();
        assert!(!is_json_request(&headers));
    }

    #[test]
    fn test_is_json_request_case_insensitive() {
        // 大小写不敏感（对齐 PHP strpos 在大小写不敏感场景的行为）
        let mut headers = HeaderMap::new();
        headers.insert("accept", "APPLICATION/JSON".parse().unwrap());
        assert!(is_json_request(&headers));
    }

    #[test]
    fn test_is_json_request_mixed_accept() {
        // 浏览器可能发送复杂 Accept 头
        let mut headers = HeaderMap::new();
        headers.insert(
            "accept",
            "text/html,application/xhtml+xml,application/json;q=0.9,*/*;q=0.8"
                .parse()
                .unwrap(),
        );
        assert!(is_json_request(&headers));
    }

    // ---------- respond 测试 ----------

    #[test]
    fn test_respond_returns_json_content_type() {
        let data = serde_json::json!({"id": 1});
        let resp = respond(&data);
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn test_respond_object_body() {
        let data = serde_json::json!({"id": 1, "name": "alice"});
        let resp = respond(&data);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, r#"{"id":1,"name":"alice"}"#);
    }

    #[tokio::test]
    async fn test_respond_array_body() {
        let data = serde_json::json!([1, 2, 3]);
        let resp = respond(&data);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, r#"[1,2,3]"#);
    }

    #[tokio::test]
    async fn test_respond_string_value() {
        let data = Value::String("hello".to_string());
        let resp = respond(&data);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        // Value::String 序列化为 JSON 字符串（带引号）
        assert_eq!(body, r#""hello""#);
    }

    #[tokio::test]
    async fn test_respond_null_value() {
        let resp = respond(&Value::Null);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "null");
    }

    #[tokio::test]
    async fn test_respond_number_value() {
        let resp = respond(&serde_json::json!(42));
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "42");
    }

    #[tokio::test]
    async fn test_respond_bool_value() {
        let resp = respond(&serde_json::json!(true));
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "true");
    }

    // ---------- respond_html 测试 ----------

    #[test]
    fn test_respond_html_content_type() {
        let resp = respond_html("<h1>Hello</h1>");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn test_respond_html_body() {
        let resp = respond_html("<p>test</p>");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "<p>test</p>");
    }

    #[tokio::test]
    async fn test_respond_html_empty() {
        let resp = respond_html("");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "");
    }

    #[tokio::test]
    async fn test_respond_html_with_unicode() {
        let resp = respond_html("<p>你好世界</p>");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "<p>你好世界</p>");
    }

    // ---------- respond_text 测试 ----------

    #[test]
    fn test_respond_text_content_type() {
        let resp = respond_text("plain text");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/plain; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn test_respond_text_body() {
        let resp = respond_text("OK");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "OK");
    }

    // ---------- auto_respond 测试 ----------

    #[tokio::test]
    async fn test_auto_respond_json_request_with_object() {
        // Accept: application/json + 对象 → JSON 响应
        let mut headers = HeaderMap::new();
        headers.insert("accept", "application/json".parse().unwrap());
        let data = serde_json::json!({"id": 1});
        let resp = auto_respond(&data, &headers);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, r#"{"id":1}"#);
    }

    #[tokio::test]
    async fn test_auto_respond_json_request_with_array() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", "application/json".parse().unwrap());
        let data = serde_json::json!([1, 2, 3]);
        let resp = auto_respond(&data, &headers);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, r#"[1,2,3]"#);
    }

    #[tokio::test]
    async fn test_auto_respond_html_request_with_object_returns_array_literal() {
        // PHP bug 复刻：Accept: text/html + 对象 → 字面量 "Array"
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        let data = serde_json::json!({"id": 1});
        let resp = auto_respond(&data, &headers);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "Array");
    }

    #[tokio::test]
    async fn test_auto_respond_html_request_with_array_returns_array_literal() {
        // PHP bug 复刻：Accept: text/html + 数组 → 字面量 "Array"
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        let data = serde_json::json!([1, 2, 3]);
        let resp = auto_respond(&data, &headers);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "Array");
    }

    #[tokio::test]
    async fn test_auto_respond_html_request_with_string_returns_string() {
        // Accept: text/html + 字符串 → 字符串内容（非字面量 "Array"）
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        let data = Value::String("hello".to_string());
        let resp = auto_respond(&data, &headers);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "hello");
    }

    #[tokio::test]
    async fn test_auto_respond_html_request_with_null_returns_empty() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        let resp = auto_respond(&Value::Null, &headers);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "");
    }

    #[tokio::test]
    async fn test_auto_respond_html_request_with_number_returns_number_string() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        let resp = auto_respond(&serde_json::json!(42), &headers);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "42");
    }

    #[tokio::test]
    async fn test_auto_respond_no_accept_header_returns_html() {
        // 无 Accept 头 → 视为非 JSON 请求 → HTML 响应
        let headers = HeaderMap::new();
        let data = serde_json::json!({"id": 1});
        let resp = auto_respond(&data, &headers);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn test_auto_respond_wildcard_accept_returns_html() {
        // Accept: */* 不包含 "json" → HTML 响应（对齐 PHP isJson() 返回 false）
        let mut headers = HeaderMap::new();
        headers.insert("accept", "*/*".parse().unwrap());
        let data = serde_json::json!({"id": 1});
        let resp = auto_respond(&data, &headers);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    // ---------- IntoResponse for JsonResponse 测试 ----------

    #[tokio::test]
    async fn test_json_response_into_response_object() {
        async fn handler() -> JsonResponse {
            JsonResponse(serde_json::json!({"id": 1, "name": "alice"}))
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, r#"{"id":1,"name":"alice"}"#);
    }

    #[tokio::test]
    async fn test_json_response_into_response_array() {
        async fn handler() -> JsonResponse {
            JsonResponse(serde_json::json!([1, 2, 3]))
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, r#"[1,2,3]"#);
    }

    #[tokio::test]
    async fn test_json_response_into_response_string() {
        async fn handler() -> JsonResponse {
            JsonResponse(Value::String("hello".to_string()))
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        // Value::String 通过 JsonResponse 返回 JSON 响应（带引号）
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, r#""hello""#);
    }

    #[tokio::test]
    async fn test_json_response_into_response_null() {
        async fn handler() -> JsonResponse {
            JsonResponse(Value::Null)
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "null");
    }

    #[tokio::test]
    async fn test_json_response_into_response_post_handler() {
        // 模拟前后端分离典型场景：POST 请求 → 处理 → 返回 JsonResponse
        async fn handler() -> JsonResponse {
            JsonResponse(serde_json::json!({
                "code": 1,
                "msg": "success",
                "data": {"id": 12345, "status": "paid"}
            }))
        }

        let router = axum::Router::new().route("/api/order", axum::routing::post(handler));
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/order")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        let json: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["code"], 1);
        assert_eq!(json["msg"], "success");
        assert_eq!(json["data"]["id"], 12345);
        assert_eq!(json["data"]["status"], "paid");
    }

    #[test]
    fn test_json_response_from_value() {
        // From<Value> 转换测试
        let value = serde_json::json!({"id": 1});
        let json_resp: JsonResponse = value.clone().into();
        assert_eq!(json_resp.0, value);
    }

    #[test]
    fn test_json_response_clone_debug() {
        let resp = JsonResponse(serde_json::json!({"id": 1}));
        let cloned = resp.clone();
        assert_eq!(resp.0, cloned.0);

        let debug = format!("{resp:?}");
        assert!(debug.contains("JsonResponse"));
    }

    // ---------- R5 PHP/Rust 行为对比测试 ----------
    //
    // 对比 PHP ThinkPHP 6 `Dispatch::autoResponse()` + `Request::isJson()` 的行为
    //
    // PHP 源码：
    //   vendor/topthink/framework/src/think/route/Dispatch.php:84-107 autoResponse()
    //   vendor/topthink/framework/src/think/Request.php:1557-1562 isJson()
    //
    // PHP autoResponse 行为：
    // 1. $data instanceof Response → 直接返回（Rust: Response → 直接返回）
    // 2. $data instanceof ResponseInterface → 转换为 Response
    // 3. $data !== null → isJson() ? 'json' : 'html'
    //    - isJson=true → JSON 响应（数组 → JSON 编码）
    //    - isJson=false → HTML 响应（数组 → 字面量 "Array"）
    // 4. $data === null → ob_get_clean + html
    //
    // PHP isJson 行为：
    // - 检查 Accept 头是否包含 "json" 子串
    // - 大小写敏感（PHP strpos 是大小写敏感的）
    // - 注意：PHP type() 方法使用 stristr（大小写不敏感），所以 PHP isJson() 实际是大小写不敏感的
    // ----------------------------------------------------------------

    #[test]
    fn test_r5_php_isjson_accept_application_json() {
        // PHP: Accept: application/json → isJson() 返回 true
        let mut headers = HeaderMap::new();
        headers.insert("accept", "application/json".parse().unwrap());
        assert!(
            is_json_request(&headers),
            "Accept: application/json 时 isJson() 必须返回 true（对齐 PHP）"
        );
    }

    #[test]
    fn test_r5_php_isjson_accept_text_html() {
        // PHP: Accept: text/html → isJson() 返回 false
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        assert!(
            !is_json_request(&headers),
            "Accept: text/html 时 isJson() 必须返回 false（对齐 PHP）"
        );
    }

    #[test]
    fn test_r5_php_isjson_accept_wildcard() {
        // PHP: Accept: */* → type() 返回 ''（无匹配 MIME）→ isJson() 返回 false
        let mut headers = HeaderMap::new();
        headers.insert("accept", "*/*".parse().unwrap());
        assert!(
            !is_json_request(&headers),
            "Accept: */* 时 isJson() 必须返回 false（对齐 PHP type() 无匹配 MIME）"
        );
    }

    #[test]
    fn test_r5_php_isjson_no_accept_header() {
        // PHP: 无 Accept 头 → type() 返回 '' → isJson() 返回 false
        let headers = HeaderMap::new();
        assert!(
            !is_json_request(&headers),
            "无 Accept 头时 isJson() 必须返回 false（对齐 PHP）"
        );
    }

    #[tokio::test]
    async fn test_r5_php_autoresponse_json_type_with_array() {
        // PHP: autoResponse($array) + isJson=true → Response::create($array, 'json')
        // → JSON 响应（数组被 json_encode）
        let mut headers = HeaderMap::new();
        headers.insert("accept", "application/json".parse().unwrap());
        let data = serde_json::json!([1, 2, 3]);
        let resp = auto_respond(&data, &headers);

        // 验证 Content-Type 为 JSON
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8",
            "PHP autoResponse + isJson=true 时必须返回 JSON 类型"
        );

        // 验证响应体为 JSON 编码的数组
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(
            body, "[1,2,3]",
            "PHP autoResponse + isJson=true 时数组必须被 json_encode"
        );
    }

    #[tokio::test]
    async fn test_r5_php_autoresponse_html_type_with_array_returns_array_literal() {
        // PHP bug 复刻：autoResponse($array) + isJson=false → Response::create($array, 'html')
        // → PHP `print($array)` 输出字面量 "Array"
        // Rust 严格对齐此 bug
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        let data = serde_json::json!([1, 2, 3]);
        let resp = auto_respond(&data, &headers);

        // 验证 Content-Type 为 HTML
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8",
            "PHP autoResponse + isJson=false 时必须返回 HTML 类型"
        );

        // 验证响应体为字面量 "Array"（PHP bug）
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(
            body, "Array",
            "PHP autoResponse + isJson=false 时数组必须输出字面量 'Array'（PHP bug 复刻）"
        );
    }

    #[tokio::test]
    async fn test_r5_php_autoresponse_html_type_with_object_returns_array_literal() {
        // PHP bug 复刻：autoResponse($assocArray) + isJson=false
        // PHP 中关联数组也是数组，print 输出 "Array"
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        let data = serde_json::json!({"name": "alice", "age": 30});
        let resp = auto_respond(&data, &headers);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(
            body, "Array",
            "PHP autoResponse + isJson=false 时关联数组也输出字面量 'Array'（PHP bug 复刻）"
        );
    }

    #[tokio::test]
    async fn test_r5_php_autoresponse_html_type_with_string_returns_string() {
        // PHP: autoResponse($string) + isJson=false → Response::create($string, 'html')
        // → 字符串原样输出
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html".parse().unwrap());
        let data = Value::String("Hello World".to_string());
        let resp = auto_respond(&data, &headers);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(
            body, "Hello World",
            "PHP autoResponse + isJson=false + 字符串时必须原样输出字符串内容"
        );
    }

    #[tokio::test]
    async fn test_r5_php_autoresponse_no_accept_header_returns_html() {
        // PHP: 无 Accept 头 → isJson=false → HTML 响应
        let headers = HeaderMap::new();
        let data = serde_json::json!({"id": 1});
        let resp = auto_respond(&data, &headers);

        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8",
            "无 Accept 头时 PHP isJson() 返回 false，必须返回 HTML 类型"
        );
    }

    #[tokio::test]
    async fn test_r5_php_autoresponse_wildcard_accept_returns_html() {
        // PHP: Accept: */* → type() 返回 ''（无匹配）→ isJson=false → HTML 响应
        let mut headers = HeaderMap::new();
        headers.insert("accept", "*/*".parse().unwrap());
        let data = serde_json::json!({"id": 1});
        let resp = auto_respond(&data, &headers);

        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8",
            "Accept: */* 时 PHP isJson() 返回 false，必须返回 HTML 类型"
        );
    }

    #[tokio::test]
    async fn test_r5_php_autoresponse_mixed_accept_with_json() {
        // PHP: Accept: text/html,application/xhtml+xml,application/json;q=0.9,*/*;q=0.8
        // → type() 匹配到 json → isJson=true → JSON 响应
        let mut headers = HeaderMap::new();
        headers.insert(
            "accept",
            "text/html,application/xhtml+xml,application/json;q=0.9,*/*;q=0.8"
                .parse()
                .unwrap(),
        );
        let data = serde_json::json!({"id": 1});
        let resp = auto_respond(&data, &headers);

        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8",
            "Accept 头含 json MIME 时 PHP isJson() 返回 true，必须返回 JSON 类型"
        );
    }

    #[test]
    fn test_r5_php_isjson_case_insensitive_alignment() {
        // PHP type() 方法使用 stristr（大小写不敏感），所以 isJson() 实际是大小写不敏感的
        // Rust 实现使用 to_lowercase().contains("json") 对齐此行为
        let mut headers_upper = HeaderMap::new();
        headers_upper.insert("accept", "APPLICATION/JSON".parse().unwrap());
        assert!(
            is_json_request(&headers_upper),
            "PHP isJson() 大小写不敏感（stristr），Rust 必须对齐"
        );

        let mut headers_mixed = HeaderMap::new();
        headers_mixed.insert("accept", "Application/Json".parse().unwrap());
        assert!(
            is_json_request(&headers_mixed),
            "PHP isJson() 大小写不敏感（stristr），Rust 必须对齐"
        );
    }

    #[tokio::test]
    async fn test_r5_php_default_response_type_json_is_project_main_strategy() {
        // 项目主策略：前后端分离 JSON 默认返回
        // 与 PHP 不同（PHP 依赖 Accept 头），Rust 项目主策略默认返回 JSON
        // 这使得即使没有 Accept: application/json 头，也返回 JSON
        let headers = HeaderMap::new(); // 无 Accept 头
        let data = serde_json::json!({"id": 1, "name": "alice"});

        // 使用 DefaultResponseType::Json（项目主策略）
        let resp = DefaultResponseType::Json.respond(&data, &headers);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8",
            "项目主策略：默认返回 JSON，不受 Accept 头影响"
        );

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(
            body, r#"{"id":1,"name":"alice"}"#,
            "项目主策略：默认返回 JSON 编码的内容"
        );
    }

    #[tokio::test]
    async fn test_r5_php_json_response_default_json_strategy() {
        // 项目主策略：JsonResponse 直接作为 handler 返回值 → 默认 JSON 响应
        // 这与 PHP 的 autoResponse 不同（PHP 会检查 Accept 头），
        // 但与项目实际开发约定一致（始终使用 renderSuccess/renderError 返回 JSON）
        async fn handler() -> JsonResponse {
            JsonResponse(serde_json::json!({"code": 1, "msg": "ok", "data": {"id": 1}}))
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));

        // 即使发送 Accept: text/html，JsonResponse 也返回 JSON（项目主策略）
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .header("accept", "text/html")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8",
            "项目主策略：JsonResponse IntoResponse 始终返回 JSON，不受 Accept 头影响"
        );

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, r#"{"code":1,"msg":"ok","data":{"id":1}}"#);
    }

    // ====================================================================
    // JSONP 响应测试
    //
    // 对齐 PHP `Response::create($data, 'jsonp')` 行为：
    // - 响应格式：`callbackName({...});`
    // - Content-Type: application/javascript; charset=utf-8
    // - 回调函数名校验：仅允许 `[a-zA-Z_][a-zA-Z0-9_.]*`，防止 XSS 注入
    // ====================================================================

    // ---------- is_valid_jsonp_callback 校验测试 ----------

    #[test]
    fn test_is_valid_jsonp_callback_simple_name() {
        assert!(is_valid_jsonp_callback("handleResponse"));
        assert!(is_valid_jsonp_callback("cb"));
        assert!(is_valid_jsonp_callback("a"));
    }

    #[test]
    fn test_is_valid_jsonp_callback_with_underscore() {
        assert!(is_valid_jsonp_callback("handle_response"));
        assert!(is_valid_jsonp_callback("_cb"));
    }

    #[test]
    fn test_is_valid_jsonp_callback_with_dot() {
        assert!(is_valid_jsonp_callback("module.callback"));
        assert!(is_valid_jsonp_callback("app.module.handle"));
    }

    #[test]
    fn test_is_valid_jsonp_callback_with_digits() {
        assert!(is_valid_jsonp_callback("cb1"));
        assert!(is_valid_jsonp_callback("handle123"));
    }

    #[test]
    fn test_is_valid_jsonp_callback_empty_is_invalid() {
        assert!(!is_valid_jsonp_callback(""));
    }

    #[test]
    fn test_is_valid_jsonp_callback_starting_with_digit_is_invalid() {
        // 首字符必须是字母或下划线
        assert!(!is_valid_jsonp_callback("1callback"));
        assert!(!is_valid_jsonp_callback("9cb"));
    }

    #[test]
    fn test_is_valid_jsonp_callback_with_special_chars_is_invalid() {
        // XSS 注入防御：禁止特殊字符
        assert!(!is_valid_jsonp_callback("alert(1)"));
        assert!(!is_valid_jsonp_callback("<script>"));
        assert!(!is_valid_jsonp_callback("cb;evil()"));
        assert!(!is_valid_jsonp_callback("cb'"));
        assert!(!is_valid_jsonp_callback("cb\""));
        assert!(!is_valid_jsonp_callback("cb-"));
        assert!(!is_valid_jsonp_callback("cb+"));
        assert!(!is_valid_jsonp_callback("cb space"));
    }

    #[test]
    fn test_is_valid_jsonp_callback_too_long_is_invalid() {
        // 超过 128 字符的回调名视为非法（防止缓冲区攻击）
        let long_name = "a".repeat(129);
        assert!(!is_valid_jsonp_callback(&long_name));
        // 刚好 128 字符是合法的
        let max_name = "a".repeat(128);
        assert!(is_valid_jsonp_callback(&max_name));
    }

    // ---------- respond_jsonp 响应构建测试 ----------

    #[tokio::test]
    async fn test_respond_jsonp_basic_format() {
        let data = serde_json::json!({"id": 1, "name": "alice"});
        let resp = respond_jsonp("handleResponse", &data);

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/javascript; charset=utf-8"
        );

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        // 格式：callbackName({...});
        assert!(body.starts_with("handleResponse("));
        assert!(body.ends_with(");"));
        // 内部数据为合法 JSON
        let json_str = &body["handleResponse(".len()..body.len() - ");".len()];
        let json: Value = serde_json::from_str(json_str).unwrap();
        assert_eq!(json["id"], 1);
        assert_eq!(json["name"], "alice");
    }

    #[tokio::test]
    async fn test_respond_jsonp_with_array_data() {
        let data = serde_json::json!([1, 2, 3]);
        let resp = respond_jsonp("cb", &data);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "cb([1,2,3]);");
    }

    #[tokio::test]
    async fn test_respond_jsonp_with_string_data() {
        let data = Value::String("hello".to_string());
        let resp = respond_jsonp("cb", &data);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        // 字符串序列化为带引号的 JSON
        assert_eq!(body, r#"cb("hello");"#);
    }

    #[tokio::test]
    async fn test_respond_jsonp_with_null_data() {
        let resp = respond_jsonp("cb", &Value::Null);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "cb(null);");
    }

    #[tokio::test]
    async fn test_respond_jsonp_with_empty_object() {
        let data = serde_json::json!({});
        let resp = respond_jsonp("cb", &data);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "cb({});");
    }

    #[tokio::test]
    async fn test_respond_jsonp_invalid_callback_returns_400() {
        let data = serde_json::json!({"id": 1});
        let resp = respond_jsonp("alert(1)", &data);

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/plain; charset=utf-8"
        );

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "Invalid JSONP callback name");
    }

    #[tokio::test]
    async fn test_respond_jsonp_empty_callback_returns_400() {
        let data = serde_json::json!({"id": 1});
        let resp = respond_jsonp("", &data);

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_respond_jsonp_xss_injection_blocked() {
        // 模拟 XSS 注入尝试：通过回调名注入脚本
        let data = serde_json::json!({"id": 1});
        let malicious_names = vec![
            "<script>alert(1)</script>",
            "cb;</script><script>alert(1)",
            "cb'+alert(1)+'",
            "cb\";alert(1);\"",
        ];

        for name in malicious_names {
            let resp = respond_jsonp(name, &data);
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "恶意回调名必须被拒绝: {name}"
            );
        }
    }

    // ---------- JsonpResponse 包装器测试 ----------

    #[tokio::test]
    async fn test_jsonp_response_wrapper_basic() {
        async fn handler() -> JsonpResponse {
            JsonpResponse("handleResponse".to_string(), serde_json::json!({"id": 1}))
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/javascript; charset=utf-8"
        );

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, r#"handleResponse({"id":1});"#);
    }

    #[tokio::test]
    async fn test_jsonp_response_wrapper_invalid_callback() {
        async fn handler() -> JsonpResponse {
            // 非法回调名
            JsonpResponse("1invalid".to_string(), serde_json::json!({}))
        }

        let router = axum::Router::new().route("/", axum::routing::get(handler));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_jsonp_response_clone_debug() {
        let resp = JsonpResponse("cb".to_string(), serde_json::json!({"id": 1}));
        let cloned = resp.clone();
        assert_eq!(cloned.0, "cb");
        assert_eq!(cloned.1["id"], 1);

        let debug = format!("{resp:?}");
        assert!(debug.contains("JsonpResponse"));
    }
}
