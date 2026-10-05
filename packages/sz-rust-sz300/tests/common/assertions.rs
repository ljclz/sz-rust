// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 自定义断言 — 脱敏格式 + 安全头 + 审计链

#![allow(dead_code)]

use axum::http::{HeaderMap, StatusCode};

/// 断言响应包含安全头 CSP
pub fn assert_security_header_csp(headers: &HeaderMap) {
    let csp = headers
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("缺少 Content-Security-Policy 头"));
    assert!(
        csp.contains("default-src"),
        "CSP 应包含 default-src 指令, 实际: {csp}"
    );
}

/// 断言响应包含 HSTS 头
pub fn assert_security_header_hsts(headers: &HeaderMap) {
    let hsts = headers
        .get("strict-transport-security")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("缺少 Strict-Transport-Security 头"));
    assert!(
        hsts.contains("max-age="),
        "HSTS 应包含 max-age, 实际: {hsts}"
    );
}

/// 断言响应包含 X-Frame-Options
pub fn assert_security_header_x_frame(headers: &HeaderMap) {
    let x_frame = headers
        .get("x-frame-options")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("缺少 X-Frame-Options 头"));
    assert!(
        x_frame.eq_ignore_ascii_case("deny") || x_frame.eq_ignore_ascii_case("sameorigin"),
        "X-Frame-Options 应为 DENY 或 SAMEORIGIN, 实际: {x_frame}"
    );
}

/// 断言响应包含 X-Content-Type-Options
pub fn assert_security_header_x_content_type(headers: &HeaderMap) {
    let x_cto = headers
        .get("x-content-type-options")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("缺少 X-Content-Type-Options 头"));
    assert!(
        x_cto.eq_ignore_ascii_case("nosniff"),
        "X-Content-Type-Options 应为 nosniff, 实际: {x_cto}"
    );
}

/// 断言所有安全头存在
pub fn assert_all_security_headers(headers: &HeaderMap) {
    assert_security_header_csp(headers);
    assert_security_header_hsts(headers);
    assert_security_header_x_frame(headers);
    assert_security_header_x_content_type(headers);
}

/// 断言手机号已脱敏（中间 4 位用 * 替换）
pub fn assert_phone_masked(phone: &str) {
    assert!(
        phone.contains("****"),
        "手机号应脱敏为 **** 格式, 实际: {phone}"
    );
}

/// 断言银行卡号已脱敏（仅保留后 4 位）
pub fn assert_bank_account_masked(account: &str) {
    assert!(
        account.starts_with("****") || account.contains("****"),
        "银行卡号应脱敏, 实际: {account}"
    );
}

/// 断言身份证号已脱敏
pub fn assert_id_card_masked(id_card: &str) {
    assert!(
        id_card.contains("*"),
        "身份证号应脱敏包含 *, 实际: {id_card}"
    );
}

/// 断言响应状态码为指定值
pub fn assert_status(actual: StatusCode, expected: StatusCode, context: &str) {
    assert_eq!(
        actual, expected,
        "{context}: 状态码应为 {expected}, 实际 {actual}"
    );
}

/// 断言响应为 JSON content-type
pub fn assert_json_content_type(headers: &HeaderMap) {
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("application/json"),
        "content-type 应为 application/json, 实际: {ct}"
    );
}

/// 断言响应为 text/event-stream（SSE）
pub fn assert_sse_content_type(headers: &HeaderMap) {
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "content-type 应为 text/event-stream, 实际: {ct}"
    );
}

/// 断言 CORS 头存在
pub fn assert_cors_headers(headers: &HeaderMap) {
    let acao = headers
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(!acao.is_empty(), "Access-Control-Allow-Origin 不应为空");
}
