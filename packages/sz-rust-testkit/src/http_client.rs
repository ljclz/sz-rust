// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! HttpClient：reqwest wrapper，链式断言。

use serde::de::DeserializeOwned;
use serde::Serialize;

/// HTTP 响应封装。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    /// 断言状态码。
    pub fn expect_status(self, expected: u16) -> Self {
        assert_eq!(
            self.status, expected,
            "expected status {} but got {}: body={}",
            expected, self.status, self.body
        );
        self
    }

    /// 断言 JSON body 匹配。
    pub fn expect_json<T: DeserializeOwned + PartialEq + std::fmt::Debug>(
        self,
        expected: &T,
    ) -> Self {
        let actual: T = serde_json::from_str(&self.body).expect("failed to parse JSON body");
        assert_eq!(actual, *expected);
        self
    }

    /// 断言 body 包含子串。
    pub fn expect_body_contains(self, substr: &str) -> Self {
        assert!(
            self.body.contains(substr),
            "body does not contain '{}': {}",
            substr,
            self.body
        );
        self
    }
}

/// 链式 HTTP 客户端。
pub struct HttpClient {
    base_url: String,
    client: reqwest::Client,
}

impl HttpClient {
    /// 创建客户端。
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::new(),
        }
    }

    /// GET 请求。
    pub async fn get(&self, path: &str) -> Result<HttpResponse, crate::Error> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        Ok(HttpResponse { status, body })
    }

    /// POST 请求。
    pub async fn post<T: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<HttpResponse, crate::Error> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.client.post(&url).json(body).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        Ok(HttpResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_response_expect_status() {
        let resp = HttpResponse {
            status: 200,
            body: "ok".into(),
        };
        resp.expect_status(200);
    }

    #[test]
    #[should_panic]
    fn http_response_expect_status_fails() {
        let resp = HttpResponse {
            status: 404,
            body: "not found".into(),
        };
        resp.expect_status(200);
    }

    #[test]
    fn http_response_expect_body_contains() {
        let resp = HttpResponse {
            status: 200,
            body: "hello world".into(),
        };
        resp.expect_body_contains("hello");
    }

    #[test]
    fn http_response_expect_json() {
        #[derive(serde::Deserialize, serde::Serialize, PartialEq, Debug)]
        struct Data {
            name: String,
        }
        let resp = HttpResponse {
            status: 200,
            body: r#"{"name":"test"}"#.into(),
        };
        resp.expect_json(&Data {
            name: "test".into(),
        });
    }

    #[test]
    fn http_client_new() {
        let client = HttpClient::new("http://localhost:8080");
        assert_eq!(client.base_url, "http://localhost:8080");
    }
}
