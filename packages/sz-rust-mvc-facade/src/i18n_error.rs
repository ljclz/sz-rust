//! 错误消息本地化桥接（4.3 竞争力深化：错误消息支持中文 i18n）
//!
//! 将 [`sz_rust_http_facade::BaseException`] 的 `message_key` 通过
//! [`sz_rust_state_facade::i18n::I18n`] 翻译为指定语言文案。
//!
//! 设计：错误类型本身只携带"消息键"（框架无关），本地化由上层完成——
//! http-facade 不依赖 state-facade，避免基础层依赖膨胀。
//!
//! ```rust
//! use sz_rust_http_facade::BaseException;
//! use sz_rust_state_facade::i18n::I18n;
//! use sz_rust_mvc_facade::i18n_error::localize_exception;
//!
//! let i18n = I18n::new();
//! i18n.set_default_lang("zh-cn");
//! i18n.set("zh-cn", "errors.not_login", "请先登录");
//!
//! let err = BaseException::not_login("not_login").with_message_key("errors.not_login");
//! let msg = localize_exception(&err, &i18n, None);
//! assert_eq!(msg, "请先登录"); // message_key 命中翻译
//!
//! let plain = BaseException::not_login("自定义文案");
//! assert_eq!(localize_exception(&plain, &i18n, None), "自定义文案"); // 无 key 原样返回
//! ```

use std::collections::HashMap;

use sz_rust_http_facade::BaseException;
use sz_rust_state_facade::i18n::I18n;

/// 将异常消息本地化：message_key 存在且有翻译则返回翻译，否则返回原始 msg
///
/// - `ex`：待本地化的异常
/// - `i18n`：i18n 实例（需已加载语言包）
/// - `lang`：目标语言（None 时使用 i18n 的当前语言 → 默认语言回退链）
pub fn localize_exception(ex: &BaseException, i18n: &I18n, lang: Option<&str>) -> String {
    match ex.message_key() {
        Some(key) => i18n
            .get(key, &HashMap::new(), lang)
            .unwrap_or_else(|| ex.msg.clone()),
        None => ex.msg.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_http_facade::ErrorCode;

    #[test]
    fn localize_with_key_and_translation() {
        let i18n = I18n::new();
        i18n.set_default_lang("zh-cn");
        i18n.set("zh-cn", "errors.not_login", "请先登录");
        i18n.set("en", "errors.not_login", "Please sign in");

        let err = BaseException::new(ErrorCode::NotLogin, "fallback")
            .with_message_key("errors.not_login");
        assert_eq!(localize_exception(&err, &i18n, None), "请先登录");
        assert_eq!(
            localize_exception(&err, &i18n, Some("en")),
            "Please sign in"
        );
    }

    #[test]
    fn localize_with_key_missing_translation_falls_back_to_msg() {
        let i18n = I18n::new();
        i18n.set_default_lang("zh-cn");
        let err = BaseException::new(ErrorCode::Forbidden, "无权限")
            .with_message_key("errors.missing_key");
        assert_eq!(localize_exception(&err, &i18n, None), "无权限");
    }

    #[test]
    fn localize_without_key_returns_msg_as_is() {
        let i18n = I18n::new();
        i18n.set_default_lang("zh-cn");
        let err = BaseException::failed("系统繁忙");
        assert_eq!(localize_exception(&err, &i18n, None), "系统繁忙");
        assert!(err.message_key().is_none());
    }
}
