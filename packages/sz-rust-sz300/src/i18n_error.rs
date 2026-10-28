// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! sz300 业务错误消息本地化（C5 落地：i18n 应用到业务错误）
//!
//! 展示完整链路：语言包定义 → `BaseException::with_message_key` →
//! `sz_rust_mvc_facade::i18n_error::localize_exception` 翻译。
//!
//! 生产环境应从 `config/lang/zh-cn.yml` 加载语言包（`I18n::load_from_file`），
//! 此处为内存字典示例。

use sz_rust_state_facade::i18n::I18n;

/// 订单错误语言包（zh-cn 默认）
pub fn order_error_i18n() -> I18n {
    let i18n = I18n::new();
    i18n.set_default_lang("zh-cn");
    i18n.set(
        "zh-cn",
        "errors.order_id_invalid",
        "缺少有效的 order_id 参数",
    );
    i18n.set("zh-cn", "errors.order_not_found", "订单不存在");
    i18n.set(
        "en",
        "errors.order_id_invalid",
        "invalid order_id parameter",
    );
    i18n
}

/// v1.9.0 业务错误码枚举（spec §5.5.1 规则 2）
#[cfg(feature = "v19-i18n")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorCode {
    /// 用户名或密码错误
    InvalidCredentials,
    /// 未授权
    Unauthorized,
    /// 令牌过期
    TokenExpired,
    /// Audience 不匹配
    AudienceMismatch,
    /// 订单不存在
    OrderNotFound,
    /// 库存不足
    StockInsufficient,
    /// 无 audience 声明
    NoAudience,
    /// 参数验证失败
    ValidationFailed,
    /// 服务繁忙（连接池耗尽）
    ServiceBusy,
}

#[cfg(feature = "v19-i18n")]
impl ErrorCode {
    /// 映射到 i18n key
    pub fn i18n_key(&self) -> &'static str {
        match self {
            Self::InvalidCredentials => "error.invalid_credentials",
            Self::Unauthorized => "error.unauthorized",
            Self::TokenExpired => "error.token_expired",
            Self::AudienceMismatch => "error.audience_mismatch",
            Self::OrderNotFound => "error.order_not_found",
            Self::StockInsufficient => "error.stock_insufficient",
            Self::NoAudience => "error.no_audience",
            Self::ValidationFailed => "error.validation_failed",
            Self::ServiceBusy => "error.service_busy",
        }
    }
}

/// v1.9.0 翻译错误消息（spec §5.5.1 规则 2）
///
/// 将 `ErrorCode` 映射到 i18n key 并翻译为目标语言。
/// 缺失翻译回退默认语言 en + WARN 日志（spec §5.5.1 规则 3）。
#[cfg(feature = "v19-i18n")]
pub fn translate_error(i18n: &I18n, lang: &str, error_code: &ErrorCode) -> String {
    let key = error_code.i18n_key();
    if let Some(result) = i18n.get_simple(key, Some(lang)) {
        return result;
    }
    if let Some(default_result) = i18n.get_simple(key, Some("en")) {
        tracing::warn!(key = key, lang = lang, "缺失翻译: 回退到 en");
        return default_result;
    }
    tracing::warn!(key = key, lang = lang, "缺失翻译: 无 en 回退");
    format!("Error: {}", key)
}

/// v1.9.0 加载 sz300 错误消息资源（zh-CN + en）（spec §5.5.1 规则 2）
#[cfg(feature = "v19-i18n")]
pub fn load_error_i18n() -> I18n {
    let i18n = I18n::with_default_lang("en");
    let zh_entries = [
        ("error.invalid_credentials", "用户名或密码错误"),
        ("error.unauthorized", "未授权，请先登录"),
        ("error.token_expired", "令牌已过期，请重新登录"),
        ("error.audience_mismatch", "令牌目标服务不匹配"),
        ("error.order_not_found", "订单不存在"),
        ("error.stock_insufficient", "库存不足"),
        ("error.no_audience", "令牌缺少 audience 声明"),
        ("error.validation_failed", "参数验证失败"),
        ("error.service_busy", "服务繁忙，请稍后重试"),
    ];
    let en_entries = [
        ("error.invalid_credentials", "Invalid username or password"),
        ("error.unauthorized", "Unauthorized, please login first"),
        ("error.token_expired", "Token expired, please login again"),
        ("error.audience_mismatch", "Token audience mismatch"),
        ("error.order_not_found", "Order not found"),
        ("error.stock_insufficient", "Stock insufficient"),
        ("error.no_audience", "Token missing audience claim"),
        ("error.validation_failed", "Validation failed"),
        ("error.service_busy", "Service busy, please retry later"),
    ];
    for (key, value) in &zh_entries {
        i18n.set("zh-CN", key, value);
    }
    for (key, value) in &en_entries {
        i18n.set("en", key, value);
    }
    i18n
}

#[cfg(feature = "v19-i18n")]
#[cfg(test)]
mod v19_tests {
    use super::*;

    #[test]
    fn test_translate_zh_cn() {
        let i18n = load_error_i18n();
        let msg = translate_error(&i18n, "zh-CN", &ErrorCode::InvalidCredentials);
        assert_eq!(msg, "用户名或密码错误");
    }

    #[test]
    fn test_translate_en() {
        let i18n = load_error_i18n();
        let msg = translate_error(&i18n, "en", &ErrorCode::InvalidCredentials);
        assert_eq!(msg, "Invalid username or password");
    }

    #[test]
    fn test_translate_missing_lang_fallback_to_en() {
        let i18n = load_error_i18n();
        let msg = translate_error(&i18n, "ja", &ErrorCode::OrderNotFound);
        assert_eq!(msg, "Order not found");
    }

    #[test]
    fn test_error_code_i18n_key() {
        assert_eq!(
            ErrorCode::InvalidCredentials.i18n_key(),
            "error.invalid_credentials"
        );
        assert_eq!(
            ErrorCode::AudienceMismatch.i18n_key(),
            "error.audience_mismatch"
        );
        assert_eq!(
            ErrorCode::StockInsufficient.i18n_key(),
            "error.stock_insufficient"
        );
    }

    #[test]
    fn test_translate_all_error_codes_zh_cn() {
        let i18n = load_error_i18n();
        let codes = [
            ErrorCode::InvalidCredentials,
            ErrorCode::Unauthorized,
            ErrorCode::TokenExpired,
            ErrorCode::AudienceMismatch,
            ErrorCode::OrderNotFound,
            ErrorCode::StockInsufficient,
            ErrorCode::NoAudience,
            ErrorCode::ValidationFailed,
            ErrorCode::ServiceBusy,
        ];
        for code in &codes {
            let msg = translate_error(&i18n, "zh-CN", code);
            assert!(!msg.is_empty());
            assert!(!msg.starts_with("Error:"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_http_facade::{BaseException, ErrorCode};
    use sz_rust_mvc_facade::i18n_error::localize_exception;

    #[test]
    fn order_error_localizes_to_chinese() {
        let i18n = order_error_i18n();
        let err = BaseException::new(ErrorCode::ValidateFailed, "invalid order_id")
            .with_message_key("errors.order_id_invalid");
        assert_eq!(
            localize_exception(&err, &i18n, None),
            "缺少有效的 order_id 参数"
        );
        assert_eq!(
            localize_exception(&err, &i18n, Some("en")),
            "invalid order_id parameter"
        );
    }
}
