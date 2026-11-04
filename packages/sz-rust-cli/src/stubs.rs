// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 代码生成模板 — 对齐 PHP `think\console\command\make\stubs\*.stub`
//!
//! ## PHP 对齐
//!
//! PHP `Make::buildClass()` 读取 stub 文件并替换占位符：
//!
//! - `{%className%}`：类名（如 `User`）
//! - `{%namespace%}`：命名空间（如 `app\model` 或 `app\controller`）
//! - `{%actionSuffix%}`：方法后缀（PHP 默认空字符串）
//! - `{%app_namespace%}`：应用命名空间（PHP 默认 `app`）
//!
//! Rust 端使用常量字符串存储模板，通过 `str::replace` 替换占位符。
//!
//! ## 模板列表
//!
//! | 常量 | 对齐 PHP stub | 用途 |
//! |------|--------------|------|
//! | `MODEL_STUB` | `model.stub` | `make:model` 生成 Model 骨架 |
//! | `CONTROLLER_STUB` | `controller.stub` | `make:controller` 生成 7 方法 Controller |
//! | `CONTROLLER_API_STUB` | `controller.api.stub` | `make:controller --api` 生成 5 方法 API Controller |
//! | `CONTROLLER_PLAIN_STUB` | `controller.plain.stub` | `make:controller --plain` 生成空 Controller |
//! | `MIGRATION_UP_STUB` | — | `make:migration` 生成 up SQL |
//! | `MIGRATION_DOWN_STUB` | — | `make:migration` 生成 down SQL |
//! | `SEED_STUB` | `make:seeder` | `make:seeder` 生成填充 SQL |
//! | `VALIDATE_STUB` | `make:validate` | `make:validate` 生成验证器骨架 |
//! | `EVENT_STUB` | `make:event` | `make:event` 生成事件类骨架 |
//! | `LISTENER_STUB` | `make:listener` | `make:listener` 生成监听器骨架 |
//! | `COMMAND_STUB` | `make:command` | `make:command` 生成自定义命令骨架 |
//! | `SERVICE_STUB` | `make:service` | `make:service` 生成服务类骨架 |

/// Model 模板（对齐 PHP `model.stub`）
///
/// PHP 原文：
/// ```php
/// <?php
/// namespace {%namespace%};
///
/// use think\Model;
///
/// /**
///  * @mixin \think\Model
///  */
/// class {%className%} extends Model
/// {
///     //
/// }
/// ```
pub const MODEL_STUB: &str = r#"//! {%namespace%}::{%className%}
//!
//! 由 `sz-rust make:model` 生成。

use sz_rust_core::orm::Model;

/// {%className%} 模型
///
/// 对齐 PHP `class {%className%} extends Model`
pub struct {%className%};

impl Model for {%className%} {
    fn table_name() -> &'static str {
        "{%table_name%}"
    }
}
"#;

/// Controller 模板（对齐 PHP `controller.stub`）
///
/// PHP 原文包含 7 个 RESTful 方法：index / create / save / read / edit / update / delete
pub const CONTROLLER_STUB: &str = r#"//! {%namespace%}::{%className%} 控制器
//!
//! 由 `sz-rust make:controller` 生成，对齐 PHP `controller.stub`（7 个 RESTful 方法）。

use sz_rust_core::controller::SzController;
use sz_rust_core::request::Request;
use sz_rust_core::response::Response;

/// {%className%} 控制器
pub struct {%className%};

impl {%className%} {
    /// 列表（GET /{%route%}）
    pub async fn index(_req: Request) -> Response {
        Response::success("index")
    }

    /// 新建表单（GET /{%route%}/create）
    pub async fn create(_req: Request) -> Response {
        Response::success("create")
    }

    /// 保存（POST /{%route%}）
    pub async fn save(_req: Request) -> Response {
        Response::success("save")
    }

    /// 详情（GET /{%route%}/{id}）
    pub async fn read(_req: Request) -> Response {
        Response::success("read")
    }

    /// 编辑表单（GET /{%route%}/{id}/edit）
    pub async fn edit(_req: Request) -> Response {
        Response::success("edit")
    }

    /// 更新（PUT /{%route%}/{id}）
    pub async fn update(_req: Request) -> Response {
        Response::success("update")
    }

    /// 删除（DELETE /{%route%}/{id}）
    pub async fn delete(_req: Request) -> Response {
        Response::success("delete")
    }
}
"#;

/// API Controller 模板（对齐 PHP `controller.api.stub`）
///
/// PHP 原文包含 5 个方法（无 create/edit）：index / save / read / update / delete
pub const CONTROLLER_API_STUB: &str = r#"//! {%namespace%}::{%className%} API 控制器
//!
//! 由 `sz-rust make:controller --api` 生成，对齐 PHP `controller.api.stub`（5 个方法）。

use sz_rust_core::request::Request;
use sz_rust_core::response::Response;

/// {%className%} API 控制器
pub struct {%className%};

impl {%className%} {
    /// 列表（GET /{%route%}）
    pub async fn index(_req: Request) -> Response {
        Response::success("index")
    }

    /// 保存（POST /{%route%}）
    pub async fn save(_req: Request) -> Response {
        Response::success("save")
    }

    /// 详情（GET /{%route%}/{id}）
    pub async fn read(_req: Request) -> Response {
        Response::success("read")
    }

    /// 更新（PUT /{%route%}/{id}）
    pub async fn update(_req: Request) -> Response {
        Response::success("update")
    }

    /// 删除（DELETE /{%route%}/{id}）
    pub async fn delete(_req: Request) -> Response {
        Response::success("delete")
    }
}
"#;

/// Plain Controller 模板（对齐 PHP `controller.plain.stub`）
///
/// PHP 原文为空类
pub const CONTROLLER_PLAIN_STUB: &str = r#"//! {%namespace%}::{%className%} 控制器
//!
//! 由 `sz-rust make:controller --plain` 生成，对齐 PHP `controller.plain.stub`（空类）。

/// {%className%} 控制器（空骨架）
pub struct {%className%};
"#;

/// 迁移 up SQL 模板（对齐 Phinx 风格）
pub const MIGRATION_UP_STUB: &str = r#"-- Migration: {%name%}
-- Direction: UP
-- Created: {%timestamp%}

-- 在此编写 up SQL
-- CREATE TABLE IF NOT EXISTS {%table_name%} (
--     id BIGINT PRIMARY KEY AUTO_INCREMENT,
--     created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
--     updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
-- );
"#;

/// 迁移 down SQL 模板（对齐 Phinx 风格）
pub const MIGRATION_DOWN_STUB: &str = r#"-- Migration: {%name%}
-- Direction: DOWN
-- Created: {%timestamp%}

-- 在此编写 down SQL
-- DROP TABLE IF EXISTS {%table_name%};
"#;

/// 填充 SQL 模板（对齐 PHP `make:seeder`）
///
/// 生成 `.sql` 文件骨架，供 `db:seed` 命令加载执行。
pub const SEED_STUB: &str = r#"-- Seed: {%name%}
-- Created: {%timestamp%}

-- 在此编写数据填充 SQL
-- INSERT INTO users (name, email) VALUES ('admin', 'admin@example.com');
"#;

/// 验证器模板（对齐 PHP `make:validate`）
///
/// 生成 `<Name>.rs` 验证器骨架，包含 `Validate` 结构体初始化与常见规则示例。
pub const VALIDATE_STUB: &str = r#"//! {%namespace%}::{%className%} 验证器
//!
//! 由 `sz-rust make:validate` 生成，对齐 PHP `make:validate`。
//! 业务实现 `rules()` 返回验证规则，调用 `validate(&data)` 执行验证。

use sz_rust_core::validate::Validate;

/// {%className%} 验证器
///
/// 对齐 PHP `class {%className%} extends \think\Validate`。
pub struct {%className%}Validate;

impl {%className%}Validate {
    /// 创建验证器实例（含规则定义）
    ///
    /// 对齐 PHP `protected $rule = [...]`。
    pub fn new() -> Validate {
        Validate::new()
            .rule("name", "require|max:50")
            .rule("age", "require|integer|gt:0")
            // 在此添加更多验证规则
    }
}

impl Default for {%className%}Validate {
    fn default() -> Self {
        Self
    }
}
"#;

/// Event 模板（对齐 PHP `make:event`）
///
/// PHP 原文生成继承 `think\Event` 的事件类，Rust 生成包含事件名与负载数据的结构体。
pub const EVENT_STUB: &str = r#"//! {%namespace%}::{%className%} 事件
//!
//! 由 `sz-rust make:event` 生成，对齐 PHP `make:event`。
//! 通过 `EventDispatcher::trigger({%className%}::name(), &payload)` 触发。

use serde_json::Value;

/// {%className%} 事件
///
/// 对齐 PHP `class {%className%} extends \think\Event`。
pub struct {%className%};

impl {%className%} {
    /// 事件名称（用于监听器注册与触发）
    pub fn name() -> &'static str {
        "{%event_name%}"
    }

    /// 构造事件负载
    ///
    /// 在此组装事件需要传递给监听器的数据。
    pub fn payload(&self) -> Value {
        Value::Null
    }
}
"#;

/// Listener 模板（对齐 PHP `make:listener`）
///
/// PHP 原文生成含 `handle($params)` 方法的监听器类，Rust 生成实现 `Listener` trait 的结构体。
pub const LISTENER_STUB: &str = r#"//! {%namespace%}::{%className%} 监听器
//!
//! 由 `sz-rust make:listener` 生成，对齐 PHP `make:listener`。
//! 通过 `EventDispatcher::listen("{%event_name%}", Arc::new({%className%}))` 注册。

use serde_json::Value;
use sz_rust_core::event::{EventError, Listener};

/// {%className%} 监听器
///
/// 对齐 PHP `class {%className%} { public function handle($params) {} }`。
pub struct {%className%};

impl Listener for {%className%} {
    /// 处理事件（对齐 PHP `Listener::handle($params)`）
    ///
    /// # 参数
    ///
    /// - `params`：事件负载
    ///
    /// # 返回
    ///
    /// - `Ok(Value::Null)`：继续执行后续监听器
    /// - `Ok(其他值)`：在 `once=true` 模式下停止后续监听器
    /// - `Err(_)`：停止后续监听器执行
    fn handle(&self, _params: &Value) -> Result<Value, EventError> {
        // 在此实现监听逻辑
        Ok(Value::Null)
    }
}
"#;

/// Command 模板（对齐 PHP `make:command`）
///
/// PHP 原文生成继承 `think\console\Command` 的命令类，Rust 生成实现 `Command` trait 的结构体。
pub const COMMAND_STUB: &str = r#"//! {%namespace%}::{%className%} 命令
//!
//! 由 `sz-rust make:command` 生成，对齐 PHP `make:command`。
//! 通过 `Console::register(Box::new({%className%}))` 注册后可被 `sz-rust {%command_name%}` 调用。

use sz_rust_cli::console::{Command, CommandSignature};
use sz_rust_cli::error::CliError;

/// {%className%} 命令
///
/// 对齐 PHP `class {%className%} extends \think\console\Command`。
pub struct {%className%};

impl Command for {%className%} {
    /// 命令签名（对齐 PHP `Command::configure()`）
    fn signature(&self) -> CommandSignature {
        CommandSignature::new(
            "{%command_name%}",
            "{%className%} 自定义命令",
        )
        .usage("sz-rust {%command_name%} [options]")
    }

    /// 执行命令（对齐 PHP `Command::execute(Input $input, Output $output)`）
    fn execute(&self, _args: &[String]) -> Result<i32, CliError> {
        println!("{%className%} executed");
        Ok(0)
    }
}
"#;

/// Service 模板（对齐 PHP `make:service`）
///
/// PHP 原文生成含 `__construct` 与业务方法的服务类，Rust 生成可注入容器的服务结构体。
pub const SERVICE_STUB: &str = r#"//! {%namespace%}::{%className%} 服务
//!
//! 由 `sz-rust make:service` 生成，对齐 PHP `make:service`。
//! 通过 `Container::singleton::<{%className%}>()` 注册后可在控制器中自动注入。

/// {%className%} 服务
///
/// 对齐 PHP `class {%className%} { public function __construct() {} }`。
pub struct {%className%};

impl {%className%} {
    /// 创建服务实例
    pub fn new() -> Self {
        Self
    }

    /// 业务方法示例
    ///
    /// 在此实现具体业务逻辑。
    pub fn execute(&self) -> Result<(), String> {
        // 在此实现业务逻辑
        Ok(())
    }
}

impl Default for {%className%} {
    fn default() -> Self {
        Self::new()
    }
}
"#;

/// 中间件模板（对齐 PHP `make:middleware`）
///
/// 生成基于 `SzMiddleware` trait 的中间件骨架。
pub const MIDDLEWARE_STUB: &str = r#"//! {%namespace%}::{%className%} 中间件
//!
//! 由 `sz-rust make:middleware` 生成。
//! 在中间件链配置中通过 `MiddlewareChain::add(MiddlewareKind::{%className%})` 注册。

use sz_rust_core::middleware::SzMiddleware;
use axum::{body::Body, http::Request, response::Response};

/// {%className%} 中间件
pub struct {%className%};

#[sz_rust_core::middleware]
impl SzMiddleware for {%className%} {
    async fn handle(
        &self,
        req: Request<Body>,
        next: impl FnOnce(Request<Body>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
    ) -> Response {
        // 前置处理
        // tracing::info!("请求进入 {%className%}: {}", req.uri().path());

        let response = next(req).await;

        // 后置处理
        // tracing::info!("响应离开 {%className%}: {}", response.status());

        response
    }
}
"#;

/// 替换模板占位符
///
/// 对齐 PHP `Make::buildClass()` 中的 `str_replace` 调用。
///
/// # 参数
///
/// - `template`：模板字符串
/// - `replacements`：占位符 → 替换值（如 `{%className%}` → `User`）
pub fn render_template(template: &str, replacements: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (placeholder, value) in replacements {
        result = result.replace(placeholder, value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_stub_contains_class_placeholder() {
        assert!(MODEL_STUB.contains("{%className%}"));
        assert!(MODEL_STUB.contains("{%namespace%}"));
        assert!(MODEL_STUB.contains("{%table_name%}"));
    }

    #[test]
    fn test_controller_stub_has_seven_methods() {
        assert!(CONTROLLER_STUB.contains("index"));
        assert!(CONTROLLER_STUB.contains("create"));
        assert!(CONTROLLER_STUB.contains("save"));
        assert!(CONTROLLER_STUB.contains("read"));
        assert!(CONTROLLER_STUB.contains("edit"));
        assert!(CONTROLLER_STUB.contains("update"));
        assert!(CONTROLLER_STUB.contains("delete"));
    }

    #[test]
    fn test_controller_api_stub_has_five_methods() {
        assert!(CONTROLLER_API_STUB.contains("index"));
        assert!(CONTROLLER_API_STUB.contains("save"));
        assert!(CONTROLLER_API_STUB.contains("read"));
        assert!(CONTROLLER_API_STUB.contains("update"));
        assert!(CONTROLLER_API_STUB.contains("delete"));
        // API 模板不应包含 create/edit
        assert!(!CONTROLLER_API_STUB.contains("pub async fn create"));
        assert!(!CONTROLLER_API_STUB.contains("pub async fn edit"));
    }

    #[test]
    fn test_controller_plain_stub_is_empty_class() {
        assert!(CONTROLLER_PLAIN_STUB.contains("struct {%className%}"));
    }

    #[test]
    fn test_migration_stubs_contain_placeholders() {
        assert!(MIGRATION_UP_STUB.contains("{%name%}"));
        assert!(MIGRATION_UP_STUB.contains("{%timestamp%}"));
        assert!(MIGRATION_DOWN_STUB.contains("{%name%}"));
        assert!(MIGRATION_DOWN_STUB.contains("{%timestamp%}"));
    }

    #[test]
    fn test_seed_stub_contains_placeholders() {
        assert!(SEED_STUB.contains("{%name%}"));
        assert!(SEED_STUB.contains("{%timestamp%}"));
    }

    #[test]
    fn test_seed_stub_render() {
        let result = render_template(
            SEED_STUB,
            &[
                ("{%name%}", "001_users"),
                ("{%timestamp%}", "2026-07-31 00:00:00 UTC"),
            ],
        );
        assert!(result.contains("001_users"));
        assert!(result.contains("2026-07-31"));
        // 渲染后不应残留占位符
        assert!(!result.contains("{%"));
    }

    #[test]
    fn test_validate_stub_contains_placeholders() {
        assert!(VALIDATE_STUB.contains("{%className%}"));
        assert!(VALIDATE_STUB.contains("{%namespace%}"));
    }

    #[test]
    fn test_validate_stub_render() {
        let result = render_template(
            VALIDATE_STUB,
            &[
                ("{%className%}", "User"),
                ("{%namespace%}", "app::validate"),
            ],
        );
        assert!(result.contains("UserValidate"));
        assert!(result.contains("app::validate"));
        assert!(result.contains("use sz_rust_core::validate::Validate"));
        // 渲染后不应残留占位符
        assert!(!result.contains("{%"));
    }

    #[test]
    fn test_render_template_basic() {
        let result = render_template("Hello {%name%}!", &[("{%name%}", "World")]);
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_render_template_multiple_placeholders() {
        let result = render_template(
            "{%className%} in {%namespace%}",
            &[("{%className%}", "User"), ("{%namespace%}", "app::model")],
        );
        assert_eq!(result, "User in app::model");
    }

    #[test]
    fn test_render_template_no_placeholders() {
        let result = render_template("no placeholders", &[]);
        assert_eq!(result, "no placeholders");
    }

    #[test]
    fn test_render_template_repeated_placeholders() {
        let result = render_template("{%x%} and {%x%}", &[("{%x%}", "A")]);
        assert_eq!(result, "A and A");
    }

    // ---------- 新增 make 命令模板测试 ----------

    #[test]
    fn test_event_stub_contains_placeholders() {
        assert!(EVENT_STUB.contains("{%className%}"));
        assert!(EVENT_STUB.contains("{%namespace%}"));
        assert!(EVENT_STUB.contains("{%event_name%}"));
    }

    #[test]
    fn test_event_stub_render() {
        let result = render_template(
            EVENT_STUB,
            &[
                ("{%className%}", "UserLogin"),
                ("{%namespace%}", "app::event"),
                ("{%event_name%}", "UserLogin"),
            ],
        );
        assert!(result.contains("UserLogin"));
        assert!(result.contains("app::event"));
        assert!(result.contains("use serde_json::Value"));
        // 渲染后不应残留占位符
        assert!(!result.contains("{%"));
    }

    #[test]
    fn test_listener_stub_contains_placeholders() {
        assert!(LISTENER_STUB.contains("{%className%}"));
        assert!(LISTENER_STUB.contains("{%namespace%}"));
        assert!(LISTENER_STUB.contains("{%event_name%}"));
    }

    #[test]
    fn test_listener_stub_render() {
        let result = render_template(
            LISTENER_STUB,
            &[
                ("{%className%}", "SendWelcomeEmail"),
                ("{%namespace%}", "app::listener"),
                ("{%event_name%}", "UserLogin"),
            ],
        );
        assert!(result.contains("SendWelcomeEmail"));
        assert!(result.contains("app::listener"));
        assert!(result.contains("use sz_rust_core::event::{EventError, Listener}"));
        assert!(result.contains("impl Listener for SendWelcomeEmail"));
        // 渲染后不应残留占位符
        assert!(!result.contains("{%"));
    }

    #[test]
    fn test_command_stub_contains_placeholders() {
        assert!(COMMAND_STUB.contains("{%className%}"));
        assert!(COMMAND_STUB.contains("{%namespace%}"));
        assert!(COMMAND_STUB.contains("{%command_name%}"));
    }

    #[test]
    fn test_command_stub_render() {
        let result = render_template(
            COMMAND_STUB,
            &[
                ("{%className%}", "SyncData"),
                ("{%namespace%}", "app::command"),
                ("{%command_name%}", "sync:data"),
            ],
        );
        assert!(result.contains("SyncData"));
        assert!(result.contains("app::command"));
        assert!(result.contains("sync:data"));
        assert!(result.contains("use sz_rust_cli::console::{Command, CommandSignature}"));
        assert!(result.contains("impl Command for SyncData"));
        // 渲染后不应残留占位符
        assert!(!result.contains("{%"));
    }

    #[test]
    fn test_service_stub_contains_placeholders() {
        assert!(SERVICE_STUB.contains("{%className%}"));
        assert!(SERVICE_STUB.contains("{%namespace%}"));
    }

    #[test]
    fn test_service_stub_render() {
        let result = render_template(
            SERVICE_STUB,
            &[
                ("{%className%}", "UserService"),
                ("{%namespace%}", "app::service"),
            ],
        );
        assert!(result.contains("UserService"));
        assert!(result.contains("app::service"));
        assert!(result.contains("impl Default for UserService"));
        // 渲染后不应残留占位符
        assert!(!result.contains("{%"));
    }
}
