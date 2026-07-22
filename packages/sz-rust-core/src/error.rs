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
//! | `500` | 数据库错误 |（Rust 扩展） |
//!
//! ## JSON 响应格式
//!
//! ```json
//! { "code": <code>, "msg": "<msg>", "data": {} }
//! ```

use serde::Serialize;
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
}

impl BaseException {
    /// 创建 BaseException（对齐 PHP `new BaseException(['code' => x, 'msg' => y])`）
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            code: code.as_i32(),
            msg: msg.into(),
        }
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
}
