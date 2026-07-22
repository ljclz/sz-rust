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

use sz_orm_core::model::Model;

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

-- TODO: 在此编写 up SQL
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

-- TODO: 在此编写 down SQL
-- DROP TABLE IF EXISTS {%table_name%};
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
}
