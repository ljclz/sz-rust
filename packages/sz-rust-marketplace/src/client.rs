// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CLI 客户端
//!
//! MarketplaceClient 基于 reqwest，含 search/install/publish/uninstall/update/list/login。
//! token 持久化到 `~/.sz-rust/credentials.toml`。

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{MarketplaceError, MarketplaceResult};

/// 搜索结果项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSearchResult {
    pub name: String,
    pub title: String,
    pub author: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub price: f64,
}

/// 搜索响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub plugins: Vec<PluginSearchResult>,
    pub total: usize,
}

/// 登录响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
}

/// 登录请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub developer_id: i64,
    pub username: String,
    pub is_reviewer: bool,
}

/// 凭证文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub token: String,
    pub base_url: String,
}

/// 市场客户端
pub struct MarketplaceClient {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl MarketplaceClient {
    /// 创建客户端
    pub fn new(base_url: &str, token: Option<String>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// 从凭证文件创建客户端
    pub async fn from_credentials() -> MarketplaceResult<Self> {
        let path = credentials_path();
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("读取凭证失败: {e}")))?;
        let creds: Credentials = toml::from_str(&content)
            .map_err(|e| MarketplaceError::InternalError(format!("解析凭证失败: {e}")))?;
        Ok(Self::new(&creds.base_url, Some(creds.token)))
    }

    /// 搜索插件
    pub async fn search(
        &self,
        keyword: &str,
        tag: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> MarketplaceResult<SearchResponse> {
        let mut url = format!("{}/api/v1/plugins/search?q={}", self.base_url, keyword);
        if let Some(tag) = tag {
            url.push_str(&format!("&tag={tag}"));
        }
        url.push_str(&format!("&limit={limit}&offset={offset}"));

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| MarketplaceError::DownloadFailed(format!("搜索请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MarketplaceError::InternalError(format!(
                "搜索失败: HTTP {status}: {body}"
            )));
        }

        resp.json::<SearchResponse>()
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("解析搜索结果失败: {e}")))
    }

    /// 安装插件（下载归档）
    pub async fn install(&self, name: &str, version: &str) -> MarketplaceResult<Vec<u8>> {
        let url = format!("{}/api/v1/plugins/{name}/{version}/download", self.base_url);

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| MarketplaceError::DownloadFailed(format!("下载请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MarketplaceError::DownloadFailed(format!(
                "下载失败: HTTP {status}: {body}"
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| MarketplaceError::DownloadFailed(format!("读取响应体失败: {e}")))?;
        Ok(bytes.to_vec())
    }

    /// 发布插件
    pub async fn publish(&self, archive_path: &str) -> MarketplaceResult<()> {
        let token = self.require_token()?;
        let url = format!("{}/api/v1/plugins/publish", self.base_url);

        let file_bytes = tokio::fs::read(archive_path)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("读取归档失败: {e}")))?;

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name("plugin.tar.gz")
            .mime_str("application/gzip")
            .map_err(|e| MarketplaceError::InternalError(format!("multipart 构建失败: {e}")))?;
        let form = reqwest::multipart::Form::new().part("archive", part);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("发布请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MarketplaceError::InternalError(format!(
                "发布失败: HTTP {status}: {body}"
            )));
        }

        Ok(())
    }

    /// 卸载插件（从锁文件移除）
    pub async fn uninstall(&self, name: &str) -> MarketplaceResult<()> {
        let lockfile = lockfile_path();
        if lockfile.exists() {
            crate::lockfile::LockfileManager::remove(&lockfile, name).await?;
        }
        Ok(())
    }

    /// 更新插件（重新安装最新版本）
    pub async fn update(&self, name: &str) -> MarketplaceResult<Vec<u8>> {
        self.install(name, "latest").await
    }

    /// 列出已安装插件（读取锁文件）
    pub async fn list(&self) -> MarketplaceResult<Vec<crate::lockfile::LockfileEntry>> {
        let lockfile = lockfile_path();
        if !lockfile.exists() {
            return Ok(Vec::new());
        }
        let lock = crate::lockfile::LockfileManager::read(&lockfile).await?;
        Ok(lock.plugins)
    }

    /// 登录并保存 token
    pub async fn login(
        base_url: &str,
        developer_id: i64,
        username: &str,
        is_reviewer: bool,
    ) -> MarketplaceResult<String> {
        let url = format!("{base_url}/api/v1/auth/login");
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        let req = LoginRequest {
            developer_id,
            username: username.to_string(),
            is_reviewer,
        };

        let resp = http
            .post(&url)
            .json(&req)
            .send()
            .await
            .map_err(|e| MarketplaceError::DownloadFailed(format!("登录请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MarketplaceError::Unauthorized(format!(
                "登录失败: HTTP {status}: {body}"
            )));
        }

        let login_resp: LoginResponse = resp
            .json()
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("解析登录响应失败: {e}")))?;

        let creds = Credentials {
            token: login_resp.token.clone(),
            base_url: base_url.to_string(),
        };
        let toml_str = toml::to_string(&creds)
            .map_err(|e| MarketplaceError::InternalError(format!("序列化凭证失败: {e}")))?;

        let path = credentials_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| MarketplaceError::InternalError(format!("创建目录失败: {e}")))?;
        }
        tokio::fs::write(&path, toml_str)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("写入凭证失败: {e}")))?;

        Ok(login_resp.token)
    }

    /// 保存 token 到凭证文件
    pub async fn save_token(&self, token: &str) -> MarketplaceResult<()> {
        let creds = Credentials {
            token: token.to_string(),
            base_url: self.base_url.clone(),
        };
        let toml_str = toml::to_string(&creds)
            .map_err(|e| MarketplaceError::InternalError(format!("序列化凭证失败: {e}")))?;
        let path = credentials_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| MarketplaceError::InternalError(format!("创建目录失败: {e}")))?;
        }
        tokio::fs::write(&path, toml_str)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("写入凭证失败: {e}")))?;
        Ok(())
    }

    fn require_token(&self) -> MarketplaceResult<&str> {
        self.token.as_deref().ok_or_else(|| {
            MarketplaceError::Unauthorized("未登录，请先执行 plugin login".to_string())
        })
    }
}

fn credentials_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".sz-rust")
        .join("credentials.toml")
}

fn lockfile_path() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join("plugins.lock")
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_new() {
        let client = MarketplaceClient::new("http://localhost:8080/", None);
        assert_eq!(client.base_url, "http://localhost:8080");
        assert!(client.token.is_none());
    }

    #[test]
    fn test_client_new_with_token() {
        let client =
            MarketplaceClient::new("http://localhost:8080", Some("test-token".to_string()));
        assert_eq!(client.base_url, "http://localhost:8080");
        assert_eq!(client.token.as_deref(), Some("test-token"));
    }

    #[test]
    fn test_credentials_serde() {
        let creds = Credentials {
            token: "abc123".to_string(),
            base_url: "http://localhost:8080".to_string(),
        };
        let toml_str = toml::to_string(&creds).unwrap();
        let parsed: Credentials = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.token, "abc123");
        assert_eq!(parsed.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_search_result_serde() {
        let result = PluginSearchResult {
            name: "crm".to_string(),
            title: "CRM Plugin".to_string(),
            author: "alice".to_string(),
            description: Some("A CRM plugin".to_string()),
            tags: vec!["business".to_string()],
            price: 0.0,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: PluginSearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "crm");
    }

    #[test]
    fn test_require_token_none() {
        let client = MarketplaceClient::new("http://localhost:8080", None);
        let result = client.require_token();
        assert!(result.is_err());
    }

    #[test]
    fn test_require_token_some() {
        let client = MarketplaceClient::new("http://localhost:8080", Some("token".to_string()));
        let result = client.require_token();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "token");
    }

    #[test]
    fn test_credentials_path_returns_path() {
        let path = credentials_path();
        assert!(path.to_string_lossy().contains("credentials.toml"));
    }

    #[test]
    fn test_lockfile_path_returns_path() {
        let path = lockfile_path();
        assert!(path.to_string_lossy().contains("plugins.lock"));
    }

    #[tokio::test]
    async fn test_search_success_with_mock() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/v1/plugins/search")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"plugins":[{"name":"crm","title":"CRM","author":"alice","description":null,"tags":[],"price":0.0}],"total":1}"#)
            .create_async()
            .await;
        let client = MarketplaceClient::new(&server.url(), None);
        let result = client.search("crm", None, 10, 0).await;
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp.plugins.len(), 1);
        assert_eq!(resp.plugins[0].name, "crm");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_search_error_status_with_mock() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/v1/plugins/search")
            .match_query(mockito::Matcher::Any)
            .with_status(500)
            .with_body("internal error")
            .create_async()
            .await;
        let client = MarketplaceClient::new(&server.url(), None);
        let result = client.search("test", None, 10, 0).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_install_success_with_mock() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/v1/plugins/crm/1.0.0/download")
            .with_status(200)
            .with_body(b"archive bytes")
            .create_async()
            .await;
        let client = MarketplaceClient::new(&server.url(), None);
        let result = client.install("crm", "1.0.0").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"archive bytes");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_install_error_status_with_mock() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/v1/plugins/crm/1.0.0/download")
            .with_status(404)
            .with_body("not found")
            .create_async()
            .await;
        let client = MarketplaceClient::new(&server.url(), None);
        let result = client.install("crm", "1.0.0").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_calls_install_latest() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/v1/plugins/crm/latest/download")
            .with_status(200)
            .with_body(b"latest bytes")
            .create_async()
            .await;
        let client = MarketplaceClient::new(&server.url(), None);
        let result = client.update("crm").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"latest bytes");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_login_success_with_mock() {
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", temp.path());
        std::env::set_var("USERPROFILE", temp.path());
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v1/auth/login")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"token":"jwt-token-123"}"#)
            .create_async()
            .await;
        let result = MarketplaceClient::login(&server.url(), 1, "alice", true).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "jwt-token-123");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_login_error_status_with_mock() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/api/v1/auth/login")
            .with_status(401)
            .with_body("invalid credentials")
            .create_async()
            .await;
        let result = MarketplaceClient::login(&server.url(), 1, "bad", false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_publish_without_token_returns_error() {
        let server = mockito::Server::new_async().await;
        let client = MarketplaceClient::new(&server.url(), None);
        let temp = tempfile::NamedTempFile::new().unwrap();
        let result = client.publish(temp.path().to_str().unwrap()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_uninstall_no_lockfile_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", temp.path());
        std::env::set_var("USERPROFILE", temp.path());
        let client = MarketplaceClient::new("http://localhost:8080", None);
        let result = client.uninstall("nonexistent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_list_no_lockfile_returns_empty() {
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", temp.path());
        std::env::set_var("USERPROFILE", temp.path());
        let client = MarketplaceClient::new("http://localhost:8080", None);
        let result = client.list().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_save_token_writes_file() {
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", temp.path());
        std::env::set_var("USERPROFILE", temp.path());
        let client = MarketplaceClient::new("http://localhost:8080", None);
        let result = client.save_token("new-token").await;
        assert!(result.is_ok());
        let creds_path = credentials_path();
        let content = tokio::fs::read_to_string(&creds_path).await.unwrap();
        assert!(content.contains("new-token"));
    }
}
