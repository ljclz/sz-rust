// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 插件市场 CLI 子命令组
//!
//! 提供 `sz-rust plugin search/install/publish/uninstall/update/list/login` 七个子命令。

use clap::{Args, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use tabled::{Table, Tabled};

use crate::error::CliError;

/// 默认市场 URL
const DEFAULT_MARKET_URL: &str = "http://localhost:8080";

/// 插件命令组
#[derive(Subcommand, Debug)]
pub enum PluginCommand {
    /// 搜索插件
    Search(SearchArgs),
    /// 安装插件
    Install(InstallArgs),
    /// 发布插件
    Publish(PublishArgs),
    /// 卸载插件
    Uninstall(UninstallArgs),
    /// 更新插件
    Update(UpdateArgs),
    /// 列出已安装插件
    List,
    /// 登录市场
    Login(LoginArgs),
}

/// 搜索参数
#[derive(Args, Debug)]
pub struct SearchArgs {
    /// 搜索关键词
    pub keyword: String,
    /// 标签过滤（逗号分隔）
    #[arg(long, value_delimiter = ',')]
    pub tags: Vec<String>,
    /// 来源过滤
    #[arg(long)]
    pub source: Option<String>,
    /// 排序方式
    #[arg(long, default_value = "relevance")]
    pub sort: String,
    /// 页码
    #[arg(long, default_value_t = 1)]
    pub page: u32,
    /// 每页数量
    #[arg(long, default_value_t = 20)]
    pub page_size: u32,
}

/// 安装参数
#[derive(Args, Debug)]
pub struct InstallArgs {
    /// 插件标识符（可含 @version）
    pub identifier: String,
}

/// 发布参数
#[derive(Args, Debug)]
pub struct PublishArgs {
    /// 插件归档路径
    #[arg(short = 'p', long)]
    pub path: String,
    /// 签名私钥文件路径
    #[arg(short = 's', long)]
    pub sign: String,
    /// changelog
    #[arg(short = 'c', long)]
    pub changelog: Option<String>,
}

/// 卸载参数
#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// 插件标识符
    pub identifier: String,
}

/// 更新参数
#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// 插件标识符
    pub identifier: String,
    /// 目标版本（可选，默认最新）
    #[arg(long)]
    pub version: Option<String>,
}

/// 登录参数
#[derive(Args, Debug)]
pub struct LoginArgs {
    /// 直接提供 token
    #[arg(long)]
    pub token: Option<String>,
    /// 市场服务 URL
    #[arg(long)]
    pub url: Option<String>,
}

/// 执行插件命令
pub async fn execute(cmd: &PluginCommand) -> Result<i32, CliError> {
    match cmd {
        PluginCommand::Search(args) => execute_search(args).await,
        PluginCommand::Install(args) => execute_install(args).await,
        PluginCommand::Publish(args) => execute_publish(args).await,
        PluginCommand::Uninstall(args) => execute_uninstall(args).await,
        PluginCommand::Update(args) => execute_update(args).await,
        PluginCommand::List => execute_list().await,
        PluginCommand::Login(args) => execute_login(args).await,
    }
}

/// 解析标识符（name@version → (name, version)）
fn parse_identifier(id: &str) -> (&str, &str) {
    if let Some((name, version)) = id.split_once('@') {
        (name, version)
    } else {
        (id, "latest")
    }
}

/// 创建市场客户端（从凭证文件或默认 URL）
async fn create_client() -> Result<sz_rust_marketplace::client::MarketplaceClient, CliError> {
    match sz_rust_marketplace::client::MarketplaceClient::from_credentials().await {
        Ok(client) => Ok(client),
        Err(_) => Ok(sz_rust_marketplace::client::MarketplaceClient::new(
            DEFAULT_MARKET_URL,
            None,
        )),
    }
}

/// 搜索结果表格行
#[derive(Tabled)]
struct PluginRow {
    name: String,
    title: String,
    author: String,
    tags: String,
    price: String,
}

async fn execute_search(args: &SearchArgs) -> Result<i32, CliError> {
    let client = create_client().await?;

    let tag = args.tags.first().map(|s| s.as_str());
    let offset = ((args.page - 1) * args.page_size) as i64;

    let response = client
        .search(&args.keyword, tag, args.page_size as i64, offset)
        .await
        .map_err(|e| CliError::Marketplace(e.to_string()))?;

    if response.plugins.is_empty() {
        println!("未找到匹配插件");
        return Ok(0);
    }

    let rows: Vec<PluginRow> = response
        .plugins
        .iter()
        .map(|p| PluginRow {
            name: p.name.clone(),
            title: p.title.clone(),
            author: p.author.clone(),
            tags: p.tags.join(", "),
            price: if p.price == 0.0 {
                "免费".to_string()
            } else {
                format!("¥{:.2}", p.price)
            },
        })
        .collect();

    let table = Table::new(rows);
    println!("{table}");
    println!("\n共 {} 个插件", response.total);

    Ok(0)
}

async fn execute_install(args: &InstallArgs) -> Result<i32, CliError> {
    let (name, version) = parse_identifier(&args.identifier);
    let client = create_client().await?;

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner} [{elapsed}] {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.set_message(format!("下载 {name}@{version}..."));

    let archive = client
        .install(name, version)
        .await
        .map_err(|e| CliError::Marketplace(e.to_string()))?;

    pb.finish_with_message(format!("已下载 {name}@{version} ({} bytes)", archive.len()));
    println!("插件 {name}@{version} 安装成功");

    Ok(0)
}

async fn execute_publish(args: &PublishArgs) -> Result<i32, CliError> {
    let client = create_client().await?;

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner} [{elapsed}] {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.set_message("发布中...".to_string());

    client
        .publish(&args.path)
        .await
        .map_err(|e| CliError::Marketplace(e.to_string()))?;

    pb.finish_with_message("发布请求已提交");
    println!("插件已发布，等待审核");

    Ok(0)
}

async fn execute_uninstall(args: &UninstallArgs) -> Result<i32, CliError> {
    let (name, _) = parse_identifier(&args.identifier);
    let client = create_client().await?;

    client
        .uninstall(name)
        .await
        .map_err(|e| CliError::Marketplace(e.to_string()))?;

    println!("插件 {name} 已卸载");
    Ok(0)
}

async fn execute_update(args: &UpdateArgs) -> Result<i32, CliError> {
    let (name, _) = parse_identifier(&args.identifier);
    let client = create_client().await?;

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner} [{elapsed}] {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.set_message(format!("更新 {name}..."));

    let archive = client
        .update(name)
        .await
        .map_err(|e| CliError::Marketplace(e.to_string()))?;

    pb.finish_with_message(format!("已更新 {name} ({} bytes)", archive.len()));
    println!("插件 {name} 更新成功");

    Ok(0)
}

async fn execute_list() -> Result<i32, CliError> {
    let client = create_client().await?;

    let entries = client
        .list()
        .await
        .map_err(|e| CliError::Marketplace(e.to_string()))?;

    if entries.is_empty() {
        println!("未安装任何插件");
        return Ok(0);
    }

    #[derive(Tabled)]
    struct InstalledRow {
        name: String,
        version: String,
        sha256: String,
        installed_at: String,
    }

    let rows: Vec<InstalledRow> = entries
        .iter()
        .map(|e| InstalledRow {
            name: e.name.clone(),
            version: e.version.clone(),
            sha256: e.sha256.chars().take(16).collect(),
            installed_at: e.installed_at.format("%Y-%m-%d %H:%M").to_string(),
        })
        .collect();

    let table = Table::new(rows);
    println!("{table}");
    println!("\n共 {} 个已安装插件", entries.len());

    Ok(0)
}

async fn execute_login(args: &LoginArgs) -> Result<i32, CliError> {
    let url = args.url.as_deref().unwrap_or(DEFAULT_MARKET_URL);

    if let Some(token) = &args.token {
        let client = sz_rust_marketplace::client::MarketplaceClient::new(url, None);
        client
            .save_token(token)
            .await
            .map_err(|e| CliError::Marketplace(e.to_string()))?;
        println!("登录成功（token 已保存）");
        Ok(0)
    } else {
        println!("请提供 --token 参数");
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_list_no_lock() {
        let result = execute_list().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_login_with_token() {
        let args = LoginArgs {
            token: Some("test".into()),
            url: Some("http://localhost:9999".into()),
        };
        let result = execute_login(&args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_login_without_token() {
        let args = LoginArgs {
            token: None,
            url: None,
        };
        let result = execute_login(&args).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);
    }

    #[test]
    fn test_parse_identifier_with_version() {
        let (name, version) = parse_identifier("orm@1.0.0");
        assert_eq!(name, "orm");
        assert_eq!(version, "1.0.0");
    }

    #[test]
    fn test_parse_identifier_without_version() {
        let (name, version) = parse_identifier("orm");
        assert_eq!(name, "orm");
        assert_eq!(version, "latest");
    }

    #[test]
    fn test_plugin_row_tabled() {
        let row = PluginRow {
            name: "crm".to_string(),
            title: "CRM".to_string(),
            author: "alice".to_string(),
            tags: "business".to_string(),
            price: "免费".to_string(),
        };
        let table = Table::new(vec![row]);
        let s = table.to_string();
        assert!(s.contains("crm"));
        assert!(s.contains("CRM"));
    }
}
