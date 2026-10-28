// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Request 静态门面
//!
//! 对齐 PHP `think\Request`，委托 `sz_rust_http_facade::request` 模块函数。

use std::collections::HashMap;

use axum::body::Body;
use serde_json::Value;

use sz_rust_http_facade::request as req_mod;

/// Request 静态门面（对齐 PHP `think\Request`）
///
/// HTTP Request 工具方法集合，函数为无状态转发。
pub struct Request;

impl Request {
    /// 解析查询参数（对齐 PHP `Request::param()`）
    pub fn fetch_query_data(req: &hyper::Request<Body>) -> Value {
        req_mod::fetch_query_data(req)
    }

    /// 按键获取查询参数
    pub fn fetch_query_data_by_key(req: &hyper::Request<Body>, key: &str) -> Option<Value> {
        req_mod::fetch_query_data_by_key(req, key)
    }

    /// 解析 query string
    pub fn parse_query(query: &str) -> HashMap<String, String> {
        req_mod::parse_query(query)
    }

    /// URL 解码
    pub fn url_decode(s: &str) -> String {
        req_mod::url_decode(s)
    }
}

/// 异步请求体读取便捷函数
pub async fn fetch_post_data(req: hyper::Request<Body>) -> Result<Value, String> {
    req_mod::fetch_post_data(req).await
}

/// 异步请求体按键读取
pub async fn fetch_post_data_by_key(
    req: hyper::Request<Body>,
    key: &str,
) -> Result<Option<Value>, String> {
    req_mod::fetch_post_data_by_key(req, key).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_query() {
        let map = Request::parse_query("a=1&b=2");
        assert_eq!(map.get("a"), Some(&"1".to_string()));
        assert_eq!(map.get("b"), Some(&"2".to_string()));
    }

    #[test]
    fn test_url_decode() {
        assert_eq!(Request::url_decode("hello%20world"), "hello world");
    }
}
