// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! MarketplaceClient mockito 集成测试
//!
//! 使用 mockito mock HTTP 服务，验证 client 各方法正确调用 Web API。

use sz_rust_marketplace::client::MarketplaceClient;

#[tokio::test]
async fn test_client_search_mock() {
    let mut mock_server = mockito::Server::new_async().await;
    let mock = mock_server
        .mock("GET", "/api/v1/plugins/search")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("q".to_string(), "crm".to_string()),
            mockito::Matcher::UrlEncoded("limit".to_string(), "20".to_string()),
            mockito::Matcher::UrlEncoded("offset".to_string(), "0".to_string()),
        ]))
        .with_status(200)
        .with_header("Content-Type", "application/json")
        .with_body(
            r#"{"plugins":[{"name":"crm","title":"CRM","author":"alice","description":"CRM plugin","tags":["business"],"price":0.0}],"total":1}"#,
        )
        .create_async()
        .await;

    let client = MarketplaceClient::new(&mock_server.url(), None);
    let result = client.search("crm", None, 20, 0).await.unwrap();

    assert_eq!(result.total, 1);
    assert_eq!(result.plugins[0].name, "crm");
    assert_eq!(result.plugins[0].title, "CRM");
    assert_eq!(result.plugins[0].author, "alice");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_client_search_with_tag_mock() {
    let mut mock_server = mockito::Server::new_async().await;
    let mock = mock_server
        .mock("GET", "/api/v1/plugins/search")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("q".to_string(), "orm".to_string()),
            mockito::Matcher::UrlEncoded("tag".to_string(), "db".to_string()),
        ]))
        .with_status(200)
        .with_header("Content-Type", "application/json")
        .with_body(r#"{"plugins":[],"total":0}"#)
        .create_async()
        .await;

    let client = MarketplaceClient::new(&mock_server.url(), None);
    let result = client.search("orm", Some("db"), 20, 0).await.unwrap();

    assert_eq!(result.total, 0);
    assert!(result.plugins.is_empty());

    mock.assert_async().await;
}

#[tokio::test]
async fn test_client_search_error_status() {
    let mut mock_server = mockito::Server::new_async().await;
    mock_server
        .mock("GET", "/api/v1/plugins/search")
        .match_query(mockito::Matcher::Any)
        .with_status(500)
        .with_body("internal error")
        .create_async()
        .await;

    let client = MarketplaceClient::new(&mock_server.url(), None);
    let result = client.search("test", None, 20, 0).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_install_mock() {
    let mut mock_server = mockito::Server::new_async().await;
    let mock = mock_server
        .mock("GET", "/api/v1/plugins/crm/1.0.0/download")
        .with_status(200)
        .with_header("Content-Type", "application/gzip")
        .with_body(b"fake archive bytes")
        .create_async()
        .await;

    let client = MarketplaceClient::new(&mock_server.url(), None);
    let archive = client.install("crm", "1.0.0").await.unwrap();

    assert_eq!(archive, b"fake archive bytes");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_client_install_not_found() {
    let mut mock_server = mockito::Server::new_async().await;
    mock_server
        .mock("GET", "/api/v1/plugins/nonexistent/latest/download")
        .with_status(404)
        .with_body("not found")
        .create_async()
        .await;

    let client = MarketplaceClient::new(&mock_server.url(), None);
    let result = client.install("nonexistent", "latest").await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_uninstall_no_lockfile() {
    let temp = tempfile::tempdir().unwrap();
    let orig = std::env::current_dir().unwrap();
    std::env::set_current_dir(temp.path()).unwrap();

    let client = MarketplaceClient::new("http://localhost:9999", None);
    let result = client.uninstall("crm").await;
    assert!(result.is_ok());

    std::env::set_current_dir(orig).unwrap();
}

#[tokio::test]
async fn test_client_list_empty_no_lockfile() {
    let temp = tempfile::tempdir().unwrap();
    let orig = std::env::current_dir().unwrap();
    std::env::set_current_dir(temp.path()).unwrap();

    let client = MarketplaceClient::new("http://localhost:9999", None);
    let entries = client.list().await.unwrap();
    assert!(entries.is_empty());

    std::env::set_current_dir(orig).unwrap();
}

#[tokio::test]
async fn test_client_update_calls_install() {
    let mut mock_server = mockito::Server::new_async().await;
    let mock = mock_server
        .mock("GET", "/api/v1/plugins/crm/latest/download")
        .with_status(200)
        .with_body(b"updated archive")
        .create_async()
        .await;

    let client = MarketplaceClient::new(&mock_server.url(), None);
    let archive = client.update("crm").await.unwrap();

    assert_eq!(archive, b"updated archive");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_client_login_mock() {
    let mut mock_server = mockito::Server::new_async().await;
    let mock = mock_server
        .mock("POST", "/api/v1/auth/login")
        .with_status(200)
        .with_header("Content-Type", "application/json")
        .with_body(r#"{"token":"jwt-token-123"}"#)
        .create_async()
        .await;

    let temp = tempfile::tempdir().unwrap();
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", temp.path());

    let token = MarketplaceClient::login(&mock_server.url(), 1, "alice", false)
        .await
        .unwrap();

    assert_eq!(token, "jwt-token-123");

    mock.assert_async().await;

    // 清理
    if let Some(home) = orig_home {
        std::env::set_var("HOME", home);
    } else {
        std::env::remove_var("HOME");
    }
}
