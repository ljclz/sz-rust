// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CLI plugin 命令行为集成测试
//!
//! 使用 mockito mock 市场 API，通过 credentials.toml 注入 mock URL，
//! 验证 execute() 各子命令的端到端行为。

use sz_rust_cli::cmd::plugin::{
    execute, InstallArgs, LoginArgs, PluginCommand, SearchArgs, UninstallArgs, UpdateArgs,
};

/// 测试辅助：设置临时 HOME 并写入 credentials.toml 指向 mock server
struct EnvGuard {
    orig_home: Option<String>,
    orig_userprofile: Option<String>,
}

impl EnvGuard {
    async fn setup_with_credentials(mock_url: &str) -> (tempfile::TempDir, Self) {
        let temp = tempfile::tempdir().unwrap();
        let sz_dir = temp.path().join(".sz-rust");
        tokio::fs::create_dir_all(&sz_dir).await.unwrap();
        let creds = format!(
            r#"token = "test-token"
base_url = "{}""#,
            mock_url
        );
        tokio::fs::write(sz_dir.join("credentials.toml"), creds)
            .await
            .unwrap();

        let orig_home = std::env::var("HOME").ok();
        let orig_userprofile = std::env::var("USERPROFILE").ok();
        unsafe {
            std::env::set_var("HOME", temp.path());
            std::env::set_var("USERPROFILE", temp.path());
        }

        let guard = Self {
            orig_home,
            orig_userprofile,
        };
        (temp, guard)
    }

    fn restore(&self) {
        unsafe {
            if let Some(h) = &self.orig_home {
                std::env::set_var("HOME", h);
            } else {
                std::env::remove_var("HOME");
            }
            if let Some(u) = &self.orig_userprofile {
                std::env::set_var("USERPROFILE", u);
            } else {
                std::env::remove_var("USERPROFILE");
            }
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[tokio::test]
async fn test_cli_search_with_results() {
    let mut mock = mockito::Server::new_async().await;
    mock.mock("GET", "/api/v1/plugins/search")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("Content-Type", "application/json")
        .with_body(
            r#"{"plugins":[{"name":"crm","title":"CRM","author":"alice","description":"CRM plugin","tags":["business"],"price":0.0}],"total":1}"#,
        )
        .create_async()
        .await;

    let (_temp, _guard) = EnvGuard::setup_with_credentials(&mock.url()).await;

    let args = SearchArgs {
        keyword: "crm".to_string(),
        tags: vec![],
        source: None,
        sort: "relevance".to_string(),
        page: 1,
        page_size: 20,
    };
    let result = execute(&PluginCommand::Search(args)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
}

#[tokio::test]
async fn test_cli_search_empty_results() {
    let mut mock = mockito::Server::new_async().await;
    mock.mock("GET", "/api/v1/plugins/search")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("Content-Type", "application/json")
        .with_body(r#"{"plugins":[],"total":0}"#)
        .create_async()
        .await;

    let (_temp, _guard) = EnvGuard::setup_with_credentials(&mock.url()).await;

    let args = SearchArgs {
        keyword: "nonexistent".to_string(),
        tags: vec![],
        source: None,
        sort: "relevance".to_string(),
        page: 1,
        page_size: 20,
    };
    let result = execute(&PluginCommand::Search(args)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
}

#[tokio::test]
async fn test_cli_search_server_error() {
    let mut mock = mockito::Server::new_async().await;
    mock.mock("GET", "/api/v1/plugins/search")
        .match_query(mockito::Matcher::Any)
        .with_status(500)
        .with_body("internal error")
        .create_async()
        .await;

    let (_temp, _guard) = EnvGuard::setup_with_credentials(&mock.url()).await;

    let args = SearchArgs {
        keyword: "test".to_string(),
        tags: vec![],
        source: None,
        sort: "relevance".to_string(),
        page: 1,
        page_size: 20,
    };
    let result = execute(&PluginCommand::Search(args)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_cli_install_success() {
    let mut mock = mockito::Server::new_async().await;
    mock.mock("GET", "/api/v1/plugins/crm/1.0.0/download")
        .with_status(200)
        .with_body(b"fake archive bytes")
        .create_async()
        .await;

    let (_temp, _guard) = EnvGuard::setup_with_credentials(&mock.url()).await;

    let args = InstallArgs {
        identifier: "crm@1.0.0".to_string(),
    };
    let result = execute(&PluginCommand::Install(args)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
}

#[tokio::test]
async fn test_cli_install_not_found() {
    let mut mock = mockito::Server::new_async().await;
    mock.mock("GET", "/api/v1/plugins/ghost/latest/download")
        .with_status(404)
        .with_body("not found")
        .create_async()
        .await;

    let (_temp, _guard) = EnvGuard::setup_with_credentials(&mock.url()).await;

    let args = InstallArgs {
        identifier: "ghost".to_string(),
    };
    let result = execute(&PluginCommand::Install(args)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_cli_uninstall_no_lockfile() {
    let mock = mockito::Server::new_async().await;
    let (_temp, _guard) = EnvGuard::setup_with_credentials(&mock.url()).await;

    let args = UninstallArgs {
        identifier: "crm".to_string(),
    };
    let result = execute(&PluginCommand::Uninstall(args)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
}

#[tokio::test]
async fn test_cli_update_success() {
    let mut mock = mockito::Server::new_async().await;
    mock.mock("GET", "/api/v1/plugins/crm/latest/download")
        .with_status(200)
        .with_body(b"updated archive")
        .create_async()
        .await;

    let (_temp, _guard) = EnvGuard::setup_with_credentials(&mock.url()).await;

    let args = UpdateArgs {
        identifier: "crm".to_string(),
        version: None,
    };
    let result = execute(&PluginCommand::Update(args)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
}

#[tokio::test]
async fn test_cli_login_with_token_flag() {
    let temp = tempfile::tempdir().unwrap();
    let orig_home = std::env::var("HOME").ok();
    let orig_userprofile = std::env::var("USERPROFILE").ok();
    unsafe {
        std::env::set_var("HOME", temp.path());
        std::env::set_var("USERPROFILE", temp.path());
    }

    let args = LoginArgs {
        token: Some("my-jwt-token".to_string()),
        url: Some("http://localhost:9999".to_string()),
    };
    let result = execute(&PluginCommand::Login(args)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);

    unsafe {
        if let Some(h) = orig_home {
            std::env::set_var("HOME", h);
        } else {
            std::env::remove_var("HOME");
        }
        if let Some(u) = orig_userprofile {
            std::env::set_var("USERPROFILE", u);
        } else {
            std::env::remove_var("USERPROFILE");
        }
    }
}

#[tokio::test]
async fn test_cli_login_without_token_returns_1() {
    let args = LoginArgs {
        token: None,
        url: None,
    };
    let result = execute(&PluginCommand::Login(args)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 1);
}
