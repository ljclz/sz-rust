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
