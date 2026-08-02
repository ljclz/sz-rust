//! BUG-006 回归测试：Bind 消息 pfc_count / rfc_count 负值远程 DoS 修复
//!
//! 原缺陷：pgwire/message.rs 解析 Bind 消息时，pfc_count（parameter format code count）
//! 和 rfc_count（result format code count）使用 i16::from_be_bytes as usize，
//! 负值经符号扩展为 usize::MAX，Vec::with_capacity(usize::MAX) panic（远程 DoS）。
//!
//! 修复：改用 u16::from_be_bytes，并增加 65535 上限校验。
//! 本测试直接调用 FrontendMessage::decode 验证不 panic 且返回 Err。

#![allow(clippy::approx_constant)]

use bytes::BytesMut;
use szrsql_protocol::pgwire::message::FrontendMessage;

/// 构造 Bind 消息并放入 BytesMut（Type='B' + Length + payload）
fn make_bind_buf(payload: &[u8]) -> BytesMut {
    let total_len = (4 + payload.len()) as u32;
    let mut msg = Vec::new();
    msg.push(b'B');
    msg.extend_from_slice(&total_len.to_be_bytes());
    msg.extend_from_slice(payload);
    BytesMut::from(&msg[..])
}

#[test]
fn test_bug006_negative_pfc_count_returns_err_not_panic() {
    // pfc_count = -1 (i16 = 0xFFFF，被修复后 u16 解析为 65535)
    let mut payload = Vec::new();
    payload.push(0); // portal name (empty cstring)
    payload.push(0); // statement name (empty cstring)
    payload.extend_from_slice(&(-1i16).to_be_bytes()); // pfc_count = 0xFFFF

    let mut buf = make_bind_buf(&payload);

    // 修复前：Vec::with_capacity(usize::MAX) panic
    // 修复后：返回 Err（pfc_count=65535 但 payload 不足）
    let result = FrontendMessage::decode(&mut buf);
    assert!(
        result.is_err(),
        "negative pfc_count should return Err, not panic; got: {result:?}"
    );
}

#[test]
fn test_bug006_negative_rfc_count_returns_err_not_panic() {
    // pfc_count = 0, param_count = 0, rfc_count = -1 (0xFFFF)
    let mut payload = Vec::new();
    payload.push(0); // portal name (empty cstring)
    payload.push(0); // statement name (empty cstring)
    payload.extend_from_slice(&0i16.to_be_bytes()); // pfc_count = 0
    payload.extend_from_slice(&0i16.to_be_bytes()); // param_count = 0
    payload.extend_from_slice(&(-1i16).to_be_bytes()); // rfc_count = 0xFFFF

    let mut buf = make_bind_buf(&payload);

    // 修复前：Vec::with_capacity(usize::MAX) panic
    // 修复后：返回 Err（rfc_count=65535 但 payload 不足）
    let result = FrontendMessage::decode(&mut buf);
    assert!(
        result.is_err(),
        "negative rfc_count should return Err, not panic; got: {result:?}"
    );
}

#[test]
fn test_bug006_normal_bind_still_works() {
    // 正常 Bind 消息：pfc=0, param_count=0, rfc=0（不应误伤合法请求）
    let mut payload = Vec::new();
    payload.push(0); // portal name (empty cstring)
    payload.push(0); // statement name (empty cstring)
    payload.extend_from_slice(&0i16.to_be_bytes()); // pfc_count = 0
    payload.extend_from_slice(&0i16.to_be_bytes()); // param_count = 0
    payload.extend_from_slice(&0i16.to_be_bytes()); // rfc_count = 0

    let mut buf = make_bind_buf(&payload);

    // 合法 Bind 应解析成功（Ok(Some(Bind{...}))）
    let result = FrontendMessage::decode(&mut buf);
    assert!(
        result.is_ok(),
        "normal bind with zero counts should parse ok; got err: {:?}",
        result.err()
    );
    let opt = result.unwrap();
    assert!(opt.is_some(), "should be Some(Bind)");
}
