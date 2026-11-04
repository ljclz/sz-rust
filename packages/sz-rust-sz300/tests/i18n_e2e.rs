// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 国际化端到端测试（spec §5.5）
//!
//! 验证 5 个业务规则：
//! - 规则 1/2: zh-CN → 中文错误消息, en → 英文错误消息
//! - 规则 3: 缺失翻译回退默认语言 en + WARN
//! - 规则 4: 业务枚举国际化
//! - 异常 1: Accept-Language 缺失 → 返回英文

#![cfg(feature = "v19-i18n")]

use sz_rust_sz300::i18n_error::{load_error_i18n, translate_error, ErrorCode};
use sz_rust_sz300::i18n_extractor::I18nExtractor;

#[test]
fn test_rule1_zh_cn_login_error() {
    let i18n = load_error_i18n();
    let lang = I18nExtractor::from_request(Some("zh-CN"), "");
    assert_eq!(lang.as_str(), "zh-CN");
    let msg = translate_error(&i18n, lang.as_str(), &ErrorCode::InvalidCredentials);
    assert_eq!(msg, "用户名或密码错误");
}

#[test]
fn test_rule2_en_login_error() {
    let i18n = load_error_i18n();
    let lang = I18nExtractor::from_request(Some("en"), "");
    assert_eq!(lang.as_str(), "en");
    let msg = translate_error(&i18n, lang.as_str(), &ErrorCode::InvalidCredentials);
    assert_eq!(msg, "Invalid username or password");
}

#[test]
fn test_rule3_missing_translation_fallback_to_en() {
    let i18n = load_error_i18n();
    let lang = I18nExtractor::from_request(None, "lang=ja");
    assert_eq!(lang.as_str(), "ja");
    let msg = translate_error(&i18n, lang.as_str(), &ErrorCode::OrderNotFound);
    assert_eq!(msg, "Order not found");
}

#[test]
fn test_rule4_order_status_i18n() {
    let i18n = load_error_i18n();
    let zh_msg = translate_error(&i18n, "zh-CN", &ErrorCode::StockInsufficient);
    let en_msg = translate_error(&i18n, "en", &ErrorCode::StockInsufficient);
    assert_eq!(zh_msg, "库存不足");
    assert_eq!(en_msg, "Stock insufficient");
    assert_ne!(zh_msg, en_msg);
}

#[test]
fn test_exception1_missing_accept_language_fallback_en() {
    let lang = I18nExtractor::from_request(None, "");
    assert_eq!(lang.as_str(), "en");
    let i18n = load_error_i18n();
    let msg = translate_error(&i18n, lang.as_str(), &ErrorCode::Unauthorized);
    assert_eq!(msg, "Unauthorized, please login first");
}

#[test]
fn test_query_param_overrides_header() {
    let lang = I18nExtractor::from_request(Some("zh-CN"), "lang=en");
    assert_eq!(lang.as_str(), "en");
    let i18n = load_error_i18n();
    let msg = translate_error(&i18n, lang.as_str(), &ErrorCode::TokenExpired);
    assert_eq!(msg, "Token expired, please login again");
}

#[test]
fn test_all_error_codes_translated() {
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
        let zh = translate_error(&i18n, "zh-CN", code);
        let en = translate_error(&i18n, "en", code);
        assert!(!zh.is_empty(), "zh-CN 翻译不应为空: {:?}", code);
        assert!(!en.is_empty(), "en 翻译不应为空: {:?}", code);
        assert_ne!(zh, en, "zh-CN 和 en 翻译应不同: {:?}", code);
    }
}
