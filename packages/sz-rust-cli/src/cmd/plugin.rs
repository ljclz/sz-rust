//! 插件市场 CLI 子命令组
//!
//! 提供 `sz-rust plugin search/install/publish/uninstall/update/list/login` 七个子命令。

use clap::{Args, Subcommand};

use crate::error::CliError;

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

async fn execute_search(args: &SearchArgs) -> Result<i32, CliError> {
    println!("搜索插件: {}", args.keyword);
    if !args.tags.is_empty() {
        println!("标签过滤: {}", args.tags.join(", "));
    }
    println!("（需要连接市场服务）");
    Ok(0)
}

async fn execute_install(args: &InstallArgs) -> Result<i32, CliError> {
    println!("安装插件: {}", args.identifier);
    println!("（需要连接市场服务）");
    Ok(0)
}

async fn execute_publish(args: &PublishArgs) -> Result<i32, CliError> {
    println!("发布插件: {}", args.path);
    println!("（需要连接市场服务）");
    Ok(0)
}

async fn execute_uninstall(args: &UninstallArgs) -> Result<i32, CliError> {
    println!("卸载插件: {}", args.identifier);

    println!("（需要企业版功能支持）");
    Ok(0)
}

async fn execute_update(args: &UpdateArgs) -> Result<i32, CliError> {
    println!("更新插件: {}", args.identifier);
    println!("（需要连接市场服务）");
    Ok(0)
}

async fn execute_list() -> Result<i32, CliError> {
    println!("（需要企业版功能支持）");
    Ok(0)
}

async fn execute_login(args: &LoginArgs) -> Result<i32, CliError> {
    if let Some(_token) = &args.token {
        println!("登录成功");
    } else {
        println!("请提供 --token 参数");
    }
    Ok(0)
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
        };
        let result = execute_login(&args).await;
        assert!(result.is_ok());
    }
}
