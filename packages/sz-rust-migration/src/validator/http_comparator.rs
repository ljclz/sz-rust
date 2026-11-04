// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::time::Duration;

use crate::error::MigrationResult;
use crate::validator::HttpRequestSample;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

pub struct HttpComparator;

impl HttpComparator {
    pub async fn send(
        base_url: &str,
        sample: &HttpRequestSample,
        timeout_secs: u64,
    ) -> MigrationResult<HttpResponse> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()?;
        let url = format!("{}{}", base_url, sample.path);
        let mut req = match sample.method {
            crate::phase::HttpMethod::Get => client.get(&url),
            crate::phase::HttpMethod::Post => client.post(&url),
            crate::phase::HttpMethod::Put => client.put(&url),
            crate::phase::HttpMethod::Delete => client.delete(&url),
            crate::phase::HttpMethod::Patch => client.patch(&url),
        };
        for (key, value) in &sample.headers {
            req = req.header(key, value);
        }
        if let Some(body) = &sample.body {
            req = req.json(body);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let body = resp.json().await.unwrap_or(serde_json::Value::Null);
        Ok(HttpResponse { status, body })
    }
}
