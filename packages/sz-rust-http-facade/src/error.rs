// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 错误体系 — BaseException + 错误码映射
//!
//! 对齐 PHP `app\common\exception\BaseException`。
//!
//! ## PHP 错误码（从 PHP 后端代码提取）
//!
//! | code | 含义 | PHP 使用场景 |
//! |------|------|-------------|
//! | `1` | 成功 | `renderSuccess` 默认 |
//! | `0` | 失败 | `renderError` 默认 / `BaseException` 默认 |
//! | `-1` | 未登录/参数错误 | `not_login` / `缺少必要的参数` / `密钥不准确` |
//! | `-2` | 用户不存在/未绑定 | `没有找到用户信息` / `请先绑定,员工信息` |
//! | `-3` | 用户已禁用/已离职 | `员工信息待审核` / `您已离职` |
//! | `403` | 无权限 |（Rust 扩展） |
//! | `404` | 资源不存在 |（Rust 扩展） |
//! | `422` | 验证失败 |（Rust 扩展） |
//! | `413` | 请求体过大 |（Rust 扩展） |
//! | `500` | 数据库错误 |（Rust 扩展） |
//!
//! ## JSON 响应格式
//!
//! ```json
//! { "code": <code>, "msg": "<msg>", "data": {} }
//! ```

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

/// 错误码枚举（对齐 PHP BaseException 的 code 字段）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[repr(i32)]
pub enum ErrorCode {
    /// 成功（PHP renderSuccess 默认）
    Success = 1,
    /// 失败（PHP renderError 默认 / BaseException 默认）
    Failed = 0,
    /// 未登录/参数错误（PHP not_login / 缺少必要的参数 / 密钥不准确）
    NotLogin = -1,
    /// 用户不存在/未绑定（PHP 没有找到用户信息 / 请先绑定,员工信息）
    UserNotFound = -2,
    /// 用户已禁用/已离职/待审核（PHP 员工信息待审核 / 您已离职）
    UserDisabled = -3,
    /// 无权限（Rust 扩展，HTTP 403）
    Forbidden = 403,
    /// 资源不存在（Rust 扩展，HTTP 404）
    NotFound = 404,
    /// 验证失败（Rust 扩展，HTTP 422）
    ValidateFailed = 422,
    /// 请求体过大（Rust 扩展，HTTP 413）
    PayloadTooLarge = 413,
    /// 数据库错误（Rust 扩展，HTTP 500）
    DbError = 500,
}

impl ErrorCode {
    /// 转为 i32（对齐 PHP code 字段）
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// 对应的 HTTP 状态码
    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::Success => 200,
            ErrorCode::Failed => 200,
            ErrorCode::NotLogin => 401,
            ErrorCode::UserNotFound => 401,
            ErrorCode::UserDisabled => 403,
            ErrorCode::Forbidden => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::ValidateFailed => 422,
            ErrorCode::PayloadTooLarge => 413,
            ErrorCode::DbError => 500,
        }
    }
}

impl From<i32> for ErrorCode {
    fn from(code: i32) -> Self {
        match code {
            1 => ErrorCode::Success,
            0 => ErrorCode::Failed,
            -1 => ErrorCode::NotLogin,
            -2 => ErrorCode::UserNotFound,
            -3 => ErrorCode::UserDisabled,
            403 => ErrorCode::Forbidden,
            404 => ErrorCode::NotFound,
            422 => ErrorCode::ValidateFailed,
            413 => ErrorCode::PayloadTooLarge,
            500 => ErrorCode::DbError,
            _ => ErrorCode::Failed,
        }
    }
}

/// BaseException — 对齐 PHP `app\common\exception\BaseException`
///
/// PHP 原始实现：
/// ```php
/// class BaseException extends Exception {
///     public $code = 0;
///     public $message = 'invalid parameters';
///     public function __construct($params = []) {
///         if (array_key_exists('code', $params)) { $this->code = $params['code']; }
///         if (array_key_exists('msg', $params)) { $this->message = $params['msg']; }
///     }
/// }
/// ```
#[derive(Debug, Clone, Error)]
#[error("[{code}] {msg}")]
pub struct BaseException {
    /// 错误码（对齐 PHP `$code`）
    pub code: i32,
    /// 错误消息（对齐 PHP `$message`，PHP 用 `msg` 键传入）
    pub msg: String,
    /// 本地化消息键（i18n 翻译用，默认 None 表示 msg 已是最终文案）
    ///
    /// 设置后由上层（如 sz-rust-mvc-facade 的 `i18n_error::localize_exception`）
    /// 通过 i18n 模块翻译为指定语言文案。
    pub message_key: Option<String>,
}

impl BaseException {
    /// 创建 BaseException（对齐 PHP `new BaseException(['code' => x, 'msg' => y])`）
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            code: code.as_i32(),
            msg: msg.into(),
            message_key: None,
        }
    }

    /// 设置本地化消息键（i18n 翻译用）
    pub fn with_message_key(mut self, key: impl Into<String>) -> Self {
        self.message_key = Some(key.into());
        self
    }

    /// 获取本地化消息键（无则 None）
    pub fn message_key(&self) -> Option<&str> {
        self.message_key.as_deref()
    }

    /// 未登录快捷构造（对齐 PHP `throw new BaseException(['code' => -1, 'msg' => 'not_login'])`）
    pub fn not_login(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotLogin, msg)
    }

    /// 用户不存在快捷构造（对齐 PHP `throw new BaseException(['msg' => '没有找到用户信息', 'code' => -2])`）
    pub fn user_not_found(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::UserNotFound, msg)
    }

    /// 用户已禁用快捷构造（对齐 PHP `throw new BaseException(['msg' => '您已离职', 'code' => -3])`）
    pub fn user_disabled(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::UserDisabled, msg)
    }

    /// 失败快捷构造（对齐 PHP `renderError('error')`）
    pub fn failed(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Failed, msg)
    }

    /// 无权限快捷构造
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, msg)
    }

    /// 资源不存在快捷构造
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, msg)
    }

    /// 验证失败快捷构造
    pub fn validate_failed(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::ValidateFailed, msg)
    }

    /// 数据库错误快捷构造
    pub fn db_error(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::DbError, msg)
    }

    /// 请求体过大快捷构造（HTTP 413）
    pub fn payload_too_large(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::PayloadTooLarge, msg)
    }

    /// 转为 JSON 响应（对齐 PHP `renderJson(code, msg, data)`）
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "code": self.code,
            "msg": self.msg,
            "data": {}
        })
    }
}

impl Default for BaseException {
    fn default() -> Self {
        Self {
            code: ErrorCode::Failed.as_i32(),
            msg: "invalid parameters".to_string(),
            message_key: None,
        }
    }
}

// ============================================================================
// 结构化错误响应 — 对齐 ThinkPHP `{code, msg, data, debug}` 格式
// ============================================================================

/// 调试信息（仅 dev 模式包含，production 模式由 [`StructuredErrorResponse`]
/// 序列化时剔除）
///
/// 包含栈追踪、源文件路径、行号，用于开发期快速定位错误来源。
/// 生产环境不应暴露此信息，避免泄漏内部实现细节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugInfo {
    /// 栈追踪字符串（可选，由 `std::backtrace::Backtrace` 或 tracing 生成）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_trace: Option<String>,
    /// 源文件路径（可选，如 `src/handler/user.rs`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 源文件行号（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl DebugInfo {
    /// 创建空 DebugInfo（所有字段 None）
    pub fn new() -> Self {
        Self {
            stack_trace: None,
            file: None,
            line: None,
        }
    }

    /// 设置栈追踪
    pub fn with_stack_trace(mut self, trace: impl Into<String>) -> Self {
        self.stack_trace = Some(trace.into());
        self
    }

    /// 设置源文件路径
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// 设置源文件行号
    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }
}

impl Default for DebugInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// 结构化错误响应
///
/// 对齐 ThinkPHP 错误响应格式：`{code, msg, data, debug}`。
///
/// - `code`：业务错误码（对齐 PHP `BaseException::$code`）
/// - `msg`：用户可读消息
/// - `data`：附加数据（可选，默认 `null`）
/// - `debug`：调试信息（可选，仅 dev 模式包含）
///
/// ## production 模式
///
/// 当 `is_production = true` 时，序列化结果不包含 `debug` 字段，
/// 避免向客户端泄漏栈追踪、文件路径等内部信息。
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_http_facade::error::{StructuredErrorResponse, DebugInfo, ErrorCode};
///
/// // dev 模式：包含 debug
/// let resp = StructuredErrorResponse::new(ErrorCode::NotFound, "资源不存在")
///     .with_debug(DebugInfo::new().with_file("src/handler.rs").with_line(42));
///
/// // production 模式：剔除 debug
/// let resp = StructuredErrorResponse::new(ErrorCode::NotFound, "资源不存在")
///     .is_production(true);
/// ```
#[derive(Debug, Clone)]
pub struct StructuredErrorResponse {
    /// 业务错误码
    pub code: i32,
    /// 用户可读消息
    pub msg: String,
    /// 附加数据（可选）
    pub data: Option<Value>,
    /// 调试信息（可选，production 模式序列化时剔除）
    pub debug: Option<DebugInfo>,
    /// 是否为生产模式（true 时序列化剔除 debug 字段）
    is_production: bool,
}

impl StructuredErrorResponse {
    /// 创建结构化错误响应
    ///
    /// 默认 `data = None`、`debug = None`、`is_production = false`。
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            code: code.as_i32(),
            msg: msg.into(),
            data: None,
            debug: None,
            is_production: false,
        }
    }

    /// 从原始 i32 code 创建（用于未在 [`ErrorCode`] 枚举中定义的自定义错误码）
    pub fn from_raw(code: i32, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
            data: None,
            debug: None,
            is_production: false,
        }
    }

    /// 设置附加数据
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// 设置调试信息
    pub fn with_debug(mut self, debug: DebugInfo) -> Self {
        self.debug = Some(debug);
        self
    }

    /// 设置是否为生产模式
    ///
    /// `true` 时序列化结果不包含 `debug` 字段。
    pub fn is_production(mut self, is_prod: bool) -> Self {
        self.is_production = is_prod;
        self
    }

    /// 查询当前是否为生产模式
    pub fn is_production_mode(&self) -> bool {
        self.is_production
    }

    /// 序列化为 `serde_json::Value`（保证字段顺序 code → msg → data → debug）
    ///
    /// production 模式下不包含 `debug` 字段。
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("code".to_string(), Value::Number(self.code.into()));
        map.insert("msg".to_string(), Value::String(self.msg.clone()));
        map.insert("data".to_string(), self.data.clone().unwrap_or(Value::Null));
        if !self.is_production {
            if let Some(ref debug) = self.debug {
                map.insert(
                    "debug".to_string(),
                    serde_json::to_value(debug).unwrap_or(Value::Null),
                );
            }
        }
        Value::Object(map)
    }

    /// 序列化为 JSON 字符串
    pub fn to_json_string(&self) -> String {
        self.to_value().to_string()
    }

    /// 对应的 HTTP 状态码（基于 [`ErrorCode`] 映射）
    pub fn http_status(&self) -> u16 {
        ErrorCode::from(self.code).http_status()
    }
}

impl Serialize for StructuredErrorResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_value().serialize(serializer)
    }
}

/// 让 StructuredErrorResponse 可以直接作为 axum handler 返回值
///
/// 自动设置：
/// - HTTP 状态码：由 [`ErrorCode::http_status`] 映射（如 NotFound → 404）
/// - Content-Type: `application/json; charset=utf-8`
impl IntoResponse for StructuredErrorResponse {
    fn into_response(self) -> Response {
        let body = self.to_json_string();
        let status =
            StatusCode::from_u16(self.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (
            status,
            [(
                axum::http::header::CONTENT_TYPE,
                "application/json; charset=utf-8",
            )],
            body,
        )
            .into_response()
    }
}

impl From<BaseException> for StructuredErrorResponse {
    /// 将 [`BaseException`] 转为 [`StructuredErrorResponse`]
    ///
    /// 保留 code 与 msg，data/debug 均为 None，默认非生产模式。
    fn from(ex: BaseException) -> Self {
        Self {
            code: ex.code,
            msg: ex.msg,
            data: None,
            debug: None,
            is_production: false,
        }
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试错误码值与 PHP 一一对应
    #[test]
    fn test_error_code_values() {
        assert_eq!(ErrorCode::Success.as_i32(), 1);
        assert_eq!(ErrorCode::Failed.as_i32(), 0);
        assert_eq!(ErrorCode::NotLogin.as_i32(), -1);
        assert_eq!(ErrorCode::UserNotFound.as_i32(), -2);
        assert_eq!(ErrorCode::UserDisabled.as_i32(), -3);
        assert_eq!(ErrorCode::Forbidden.as_i32(), 403);
        assert_eq!(ErrorCode::NotFound.as_i32(), 404);
        assert_eq!(ErrorCode::ValidateFailed.as_i32(), 422);
        assert_eq!(ErrorCode::DbError.as_i32(), 500);
    }

    /// 测试 i32 → ErrorCode 转换
    #[test]
    fn test_from_i32() {
        assert_eq!(ErrorCode::from(1), ErrorCode::Success);
        assert_eq!(ErrorCode::from(0), ErrorCode::Failed);
        assert_eq!(ErrorCode::from(-1), ErrorCode::NotLogin);
        assert_eq!(ErrorCode::from(-2), ErrorCode::UserNotFound);
        assert_eq!(ErrorCode::from(-3), ErrorCode::UserDisabled);
        assert_eq!(ErrorCode::from(999), ErrorCode::Failed); // 未知码默认 Failed
    }

    /// 测试 HTTP 状态码映射
    #[test]
    fn test_http_status() {
        assert_eq!(ErrorCode::Success.http_status(), 200);
        assert_eq!(ErrorCode::Failed.http_status(), 200);
        assert_eq!(ErrorCode::NotLogin.http_status(), 401);
        assert_eq!(ErrorCode::UserNotFound.http_status(), 401);
        assert_eq!(ErrorCode::UserDisabled.http_status(), 403);
        assert_eq!(ErrorCode::Forbidden.http_status(), 403);
        assert_eq!(ErrorCode::NotFound.http_status(), 404);
        assert_eq!(ErrorCode::ValidateFailed.http_status(), 422);
        assert_eq!(ErrorCode::DbError.http_status(), 500);
    }

    /// 测试 BaseException 默认值（对齐 PHP `code=0, message='invalid parameters'`）
    #[test]
    fn test_default() {
        let ex = BaseException::default();
        assert_eq!(ex.code, 0);
        assert_eq!(ex.msg, "invalid parameters");
    }

    /// 测试 not_login 快捷构造（对齐 PHP `code=-1, msg='not_login'`）
    #[test]
    fn test_not_login() {
        let ex = BaseException::not_login("not_login");
        assert_eq!(ex.code, -1);
        assert_eq!(ex.msg, "not_login");
    }

    /// 测试 user_not_found 快捷构造（对齐 PHP `code=-2, msg='没有找到用户信息'`）
    #[test]
    fn test_user_not_found() {
        let ex = BaseException::user_not_found("没有找到用户信息");
        assert_eq!(ex.code, -2);
        assert_eq!(ex.msg, "没有找到用户信息");
    }

    /// 测试 user_disabled 快捷构造（对齐 PHP `code=-3, msg='您已离职'`）
    #[test]
    fn test_user_disabled() {
        let ex = BaseException::user_disabled("您已离职，无权使用本系统！");
        assert_eq!(ex.code, -3);
        assert_eq!(ex.msg, "您已离职，无权使用本系统！");
    }

    /// 测试 failed 快捷构造（对齐 PHP `renderError('error')` → `code=0`）
    #[test]
    fn test_failed() {
        let ex = BaseException::failed("操作失败");
        assert_eq!(ex.code, 0);
        assert_eq!(ex.msg, "操作失败");
    }

    /// 测试 to_json（对齐 PHP `renderJson(code, msg, data)`）
    #[test]
    fn test_to_json() {
        let ex = BaseException::not_login("not_login");
        let json = ex.to_json();
        assert_eq!(json["code"], -1);
        assert_eq!(json["msg"], "not_login");
        assert_eq!(json["data"], serde_json::json!({}));
    }

    /// 测试 Display trait
    #[test]
    fn test_display() {
        let ex = BaseException::not_login("not_login");
        assert_eq!(format!("{}", ex), "[-1] not_login");
    }

    /// 测试从 PHP 场景提取的错误码全覆盖
    /// PHP 代码中实际使用的错误码：1, 0, -1, -2, -3
    #[test]
    fn test_php_error_codes_coverage() {
        // PHP renderSuccess → code=1
        assert_eq!(ErrorCode::Success.as_i32(), 1);
        // PHP renderError → code=0
        assert_eq!(ErrorCode::Failed.as_i32(), 0);
        // PHP not_login → code=-1
        assert_eq!(ErrorCode::NotLogin.as_i32(), -1);
        // PHP 没有找到用户信息 → code=-2
        assert_eq!(ErrorCode::UserNotFound.as_i32(), -2);
        // PHP 您已离职 → code=-3
        assert_eq!(ErrorCode::UserDisabled.as_i32(), -3);
    }

    // ---------- DebugInfo 测试 ----------

    #[test]
    fn test_debug_info_new_all_none() {
        let info = DebugInfo::new();
        assert!(info.stack_trace.is_none());
        assert!(info.file.is_none());
        assert!(info.line.is_none());
    }

    #[test]
    fn test_debug_info_builders() {
        let info = DebugInfo::new()
            .with_stack_trace("fn1 -> fn2")
            .with_file("src/handler.rs")
            .with_line(42);
        assert_eq!(info.stack_trace.as_deref(), Some("fn1 -> fn2"));
        assert_eq!(info.file.as_deref(), Some("src/handler.rs"));
        assert_eq!(info.line, Some(42));
    }

    #[test]
    fn test_debug_info_default() {
        let info = DebugInfo::default();
        assert!(info.stack_trace.is_none());
        assert!(info.file.is_none());
        assert!(info.line.is_none());
    }

    #[test]
    fn test_debug_info_serialize_skip_none() {
        let info = DebugInfo::new().with_file("src/x.rs");
        let val = serde_json::to_value(&info).unwrap();
        assert!(val.get("stack_trace").is_none() || val["stack_trace"].is_null());
        assert_eq!(val["file"], "src/x.rs");
        assert!(val.get("line").is_none() || val["line"].is_null());
    }

    // ---------- StructuredErrorResponse 测试 ----------

    #[test]
    fn test_structured_error_basic() {
        let resp = StructuredErrorResponse::new(ErrorCode::NotFound, "资源不存在");
        assert_eq!(resp.code, 404);
        assert_eq!(resp.msg, "资源不存在");
        assert!(resp.data.is_none());
        assert!(resp.debug.is_none());
        assert!(!resp.is_production_mode());
    }

    #[test]
    fn test_structured_error_from_raw() {
        let resp = StructuredErrorResponse::from_raw(9999, "自定义错误");
        assert_eq!(resp.code, 9999);
        assert_eq!(resp.msg, "自定义错误");
    }

    #[test]
    fn test_structured_error_with_data() {
        let resp = StructuredErrorResponse::new(ErrorCode::ValidateFailed, "校验失败")
            .with_data(serde_json::json!({"field": "email"}));
        assert_eq!(resp.data, Some(serde_json::json!({"field": "email"})));
    }

    #[test]
    fn test_structured_error_with_debug() {
        let debug = DebugInfo::new().with_file("src/x.rs").with_line(10);
        let resp = StructuredErrorResponse::new(ErrorCode::DbError, "db error").with_debug(debug);
        assert!(resp.debug.is_some());
        assert_eq!(
            resp.debug.as_ref().unwrap().file.as_deref(),
            Some("src/x.rs")
        );
    }

    #[test]
    fn test_structured_error_to_value_dev_includes_debug() {
        let debug = DebugInfo::new().with_file("src/x.rs").with_line(10);
        let resp = StructuredErrorResponse::new(ErrorCode::NotFound, "not found").with_debug(debug);
        let val = resp.to_value();
        assert_eq!(val["code"], 404);
        assert_eq!(val["msg"], "not found");
        assert_eq!(val["data"], serde_json::Value::Null);
        assert!(val.get("debug").is_some());
        assert_eq!(val["debug"]["file"], "src/x.rs");
        assert_eq!(val["debug"]["line"], 10);
    }

    #[test]
    fn test_structured_error_to_value_production_strips_debug() {
        let debug = DebugInfo::new().with_stack_trace("secret trace");
        let resp = StructuredErrorResponse::new(ErrorCode::NotFound, "not found")
            .with_debug(debug)
            .is_production(true);
        assert!(resp.is_production_mode());
        let val = resp.to_value();
        assert_eq!(val["code"], 404);
        assert_eq!(val["msg"], "not found");
        assert_eq!(val["data"], serde_json::Value::Null);
        assert!(
            val.get("debug").is_none(),
            "production mode must not include debug field"
        );
    }

    #[test]
    fn test_structured_error_to_value_no_debug_when_none() {
        let resp = StructuredErrorResponse::new(ErrorCode::Failed, "fail");
        let val = resp.to_value();
        assert!(val.get("debug").is_none());
    }

    #[test]
    fn test_structured_error_field_order() {
        let resp = StructuredErrorResponse::new(ErrorCode::Failed, "fail")
            .with_data(serde_json::json!({"k": 1}));
        let json = resp.to_json_string();
        let code_pos = json.find("\"code\"").unwrap();
        let msg_pos = json.find("\"msg\"").unwrap();
        let data_pos = json.find("\"data\"").unwrap();
        assert!(code_pos < msg_pos);
        assert!(msg_pos < data_pos);
    }

    #[test]
    fn test_structured_error_http_status() {
        let resp = StructuredErrorResponse::new(ErrorCode::NotFound, "not found");
        assert_eq!(resp.http_status(), 404);
        let resp = StructuredErrorResponse::new(ErrorCode::NotLogin, "not login");
        assert_eq!(resp.http_status(), 401);
        let resp = StructuredErrorResponse::new(ErrorCode::DbError, "db error");
        assert_eq!(resp.http_status(), 500);
    }

    #[test]
    fn test_structured_error_from_base_exception() {
        let ex = BaseException::not_login("not_login");
        let resp: StructuredErrorResponse = ex.into();
        assert_eq!(resp.code, -1);
        assert_eq!(resp.msg, "not_login");
        assert!(resp.data.is_none());
        assert!(resp.debug.is_none());
    }

    #[test]
    fn test_structured_error_into_response_dev() {
        let debug = DebugInfo::new().with_file("src/x.rs").with_line(1);
        let resp = StructuredErrorResponse::new(ErrorCode::NotFound, "not found").with_debug(debug);
        let response = resp.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_structured_error_into_response_production() {
        let resp =
            StructuredErrorResponse::new(ErrorCode::ValidateFailed, "校验失败").is_production(true);
        let response = resp.into_response();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn test_structured_error_serialize_trait() {
        let resp = StructuredErrorResponse::new(ErrorCode::Failed, "fail")
            .with_data(serde_json::json!({"k": "v"}));
        let val = serde_json::to_value(&resp).unwrap();
        assert_eq!(val["code"], 0);
        assert_eq!(val["msg"], "fail");
        assert_eq!(val["data"]["k"], "v");
    }

    #[test]
    fn test_structured_error_production_chain() {
        let resp = StructuredErrorResponse::new(ErrorCode::Forbidden, "无权限")
            .with_data(serde_json::json!({"required": "admin"}))
            .is_production(true);
        assert!(resp.is_production_mode());
        let val = resp.to_value();
        assert_eq!(val["data"]["required"], "admin");
        assert!(val.get("debug").is_none());
    }
}
