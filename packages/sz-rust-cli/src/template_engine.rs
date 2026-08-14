//! Tera 模板引擎封装
//!
//! 对应 design.md 第 2.2.2.2 节，封装 Tera 引擎提供模板加载、渲染、校验能力。
//!
//! ## 模板命名约定
//!
//! 模板以相对路径注册到 Tera 实例，如 `plugin-crud/model.rs.tera`。
//! 跨目录继承通过 `{% extends "plugin-crud/model.rs.tera" %}` 引用。

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use tera::{Context, Tera};

use crate::error::CliError;

/// Tera 模板引擎封装
///
/// 扫描指定目录下的 `.tera` 文件并注册到内部 Tera 实例，
/// 提供模板渲染、类型校验等能力。
pub struct TemplateEngine {
    /// Tera 引擎实例
    tera: Tera,
    /// 模板根目录
    template_dir: PathBuf,
    /// 可用模板类型列表（模板根目录下的子目录名）
    template_types: Vec<String>,
}

impl fmt::Debug for TemplateEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemplateEngine")
            .field("template_dir", &self.template_dir)
            .field("template_types", &self.template_types)
            .finish()
    }
}

impl TemplateEngine {
    /// 初始化模板引擎
    ///
    /// 扫描 `template_dir` 下全部 `.tera` 文件并注册到 Tera 实例。
    /// 每个子目录代表一种模板类型（如 `plugin-crud`、`plugin-master-slave`）。
    ///
    /// # 错误
    ///
    /// - `CliError::TemplateMissing`：目录不存在
    /// - `CliError::TemplateSyntaxError`：模板语法错误（含文件名/行号/列号）
    pub async fn init(template_dir: &Path) -> Result<Self, CliError> {
        if !tokio::fs::try_exists(template_dir).await? {
            return Err(CliError::TemplateMissing(vec![template_dir
                .display()
                .to_string()]));
        }

        let mut tera = Tera::default();
        let mut template_types = Vec::new();
        let mut all_template_files = Vec::new();

        let mut root_entries = tokio::fs::read_dir(template_dir).await?;
        while let Some(entry) = root_entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let dir_name = entry.file_name().to_string_lossy().to_string();
                template_types.push(dir_name.clone());

                let mut sub_entries = tokio::fs::read_dir(&path).await?;
                while let Some(sub_entry) = sub_entries.next_entry().await? {
                    let sub_path = sub_entry.path();
                    if sub_path.is_file()
                        && sub_path
                            .extension()
                            .map(|ext| ext == "tera")
                            .unwrap_or(false)
                    {
                        all_template_files.push(sub_path);
                    }
                }
            }
        }

        template_types.sort();

        for file_path in &all_template_files {
            let relative = file_path.strip_prefix(template_dir).unwrap_or(file_path);
            let template_name = relative.to_string_lossy().replace('\\', "/");

            let content = tokio::fs::read_to_string(file_path).await?;

            tera.add_raw_template(&template_name, &content)
                .map_err(|e| map_syntax_error(e, &template_name))?;
        }

        tera.register_filter(
            "pascal_case",
            |value: &tera::Value, _: &HashMap<String, tera::Value>| {
                let s = value.as_str().unwrap_or("");
                let pascal: String = s
                    .split('_')
                    .map(|word| {
                        let mut chars = word.chars();
                        match chars.next() {
                            Some(first) => {
                                first.to_uppercase().collect::<String>() + chars.as_str()
                            }
                            None => String::new(),
                        }
                    })
                    .collect();
                Ok(tera::Value::String(pascal))
            },
        );

        tera.register_filter(
            "snake_case",
            |value: &tera::Value, _: &HashMap<String, tera::Value>| {
                let s = value.as_str().unwrap_or("");
                let snake = s.replace('-', "_").to_lowercase();
                Ok(tera::Value::String(snake))
            },
        );

        Ok(Self {
            tera,
            template_dir: template_dir.to_path_buf(),
            template_types,
        })
    }

    /// 渲染模板
    ///
    /// # 错误
    ///
    /// - `CliError::VarNotFound`：模板变量缺失（含变量名/引用文件/行号）
    /// - `CliError::Generic`：其他渲染错误
    pub fn render(&self, template_name: &str, context: &Context) -> Result<String, CliError> {
        self.tera
            .render(template_name, context)
            .map_err(|e| map_render_error(e, template_name))
    }

    /// 返回可用模板类型列表（模板根目录下的子目录名）
    pub fn list_templates(&self) -> Vec<String> {
        self.template_types.clone()
    }

    /// 校验模板类型是否存在
    ///
    /// # 错误
    ///
    /// - `CliError::UnknownTemplate`：模板类型不存在，附带可用模板列表
    pub fn validate_template_type(&self, template_type: &str) -> Result<(), CliError> {
        if self.template_types.iter().any(|t| t == template_type) {
            Ok(())
        } else {
            Err(CliError::UnknownTemplate {
                requested: template_type.to_string(),
                available: self.template_types.clone(),
            })
        }
    }

    /// 返回模板根目录
    pub fn template_dir(&self) -> &Path {
        &self.template_dir
    }
}

/// 将 Tera 语法错误映射为 `CliError::TemplateSyntaxError`
fn map_syntax_error(e: tera::Error, file: &str) -> CliError {
    let msg = e.to_string();
    let (line, col) = parse_line_col(&msg);
    CliError::TemplateSyntaxError {
        file: file.to_string(),
        line,
        col,
        msg,
    }
}

/// 将 Tera 渲染错误映射为 `CliError::VarNotFound` 或 `CliError::Generic`
fn map_render_error(e: tera::Error, template_name: &str) -> CliError {
    let msg = e.to_string();

    let mut full_msg = msg.clone();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        full_msg.push_str(&format!("\n  caused by: {s}"));
        source = std::error::Error::source(s);
    }

    if let Some(var) = extract_variable_name(&full_msg) {
        let line = parse_line_col(&full_msg).0;
        CliError::VarNotFound {
            var,
            file: template_name.to_string(),
            line,
        }
    } else {
        CliError::Generic(msg)
    }
}

/// 从错误消息中解析行号和列号
///
/// Tera/pest 错误消息通常含 `line: N` 或 `at line N` 格式
fn parse_line_col(msg: &str) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;

    if let Some(pos) = msg.find("line ") {
        let rest = &msg[pos + 5..];
        if let Some(num) = take_number(rest) {
            line = num;
        }
    }
    if let Some(pos) = msg.find("column ") {
        let rest = &msg[pos + 7..];
        if let Some(num) = take_number(rest) {
            col = num;
        }
    }

    (line, col)
}

/// 从错误消息中提取变量名
///
/// Tera 渲染错误通常含 `Variable \`xxx\` not found` 格式
fn extract_variable_name(msg: &str) -> Option<String> {
    let prefix = "Variable `";
    if let Some(start) = msg.find(prefix) {
        let rest = &msg[start + prefix.len()..];
        if let Some(end) = rest.find('`') {
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// 从字符串开头提取数字
fn take_number(s: &str) -> Option<usize> {
    let num_str: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if num_str.is_empty() {
        None
    } else {
        num_str.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 创建临时模板目录用于测试
    async fn setup_test_templates() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir failed");
        let template_dir = temp.path().to_path_buf();

        let crud_dir = template_dir.join("plugin-crud");
        tokio::fs::create_dir_all(&crud_dir)
            .await
            .expect("mkdir failed");

        tokio::fs::write(
            &crud_dir.join("model.rs.tera"),
            "// Model for {{ table_name }}\npub struct {{ class_name }} {\n    {% for field in fields %}{{ field.name }}: {{ field.rust_type }},\n    {% endfor %}}\n}\n",
        )
        .await
        .expect("write failed");

        tokio::fs::write(
            &crud_dir.join("controller.rs.tera"),
            "// Controller for {{ table_name }}\n",
        )
        .await
        .expect("write failed");

        let ms_dir = template_dir.join("plugin-master-slave");
        tokio::fs::create_dir_all(&ms_dir)
            .await
            .expect("mkdir failed");
        tokio::fs::write(
            &ms_dir.join("master_model.rs.tera"),
            "// Master: {{ master_table }}\n",
        )
        .await
        .expect("write failed");

        (temp, template_dir)
    }

    #[tokio::test]
    async fn test_init_loads_templates() {
        let (_temp, template_dir) = setup_test_templates().await;
        let engine = TemplateEngine::init(&template_dir).await;
        assert!(engine.is_ok(), "init should succeed");
        let engine = engine.unwrap();
        let templates = engine.list_templates();
        assert!(templates.contains(&"plugin-crud".to_string()));
        assert!(templates.contains(&"plugin-master-slave".to_string()));
    }

    #[tokio::test]
    async fn test_init_dir_not_exists() {
        let result = TemplateEngine::init(Path::new("/nonexistent/path/templates")).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CliError::TemplateMissing(_)));
    }

    #[tokio::test]
    async fn test_init_syntax_error() {
        let temp = tempfile::tempdir().expect("tempdir failed");
        let template_dir = temp.path().to_path_buf();
        let crud_dir = template_dir.join("bad-template");
        tokio::fs::create_dir_all(&crud_dir)
            .await
            .expect("mkdir failed");

        tokio::fs::write(
            &crud_dir.join("bad.rs.tera"),
            "{% for field in fields %}{{ field.name }}\n",
        )
        .await
        .expect("write failed");

        let result = TemplateEngine::init(&template_dir).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CliError::TemplateSyntaxError { .. }));
    }

    #[tokio::test]
    async fn test_render_success() {
        let (_temp, template_dir) = setup_test_templates().await;
        let engine = TemplateEngine::init(&template_dir)
            .await
            .expect("init failed");

        let mut ctx = Context::new();
        ctx.insert("table_name", "users");
        ctx.insert("class_name", "User");
        ctx.insert(
            "fields",
            &vec![serde_json::json!({"name": "id", "rust_type": "i32"})],
        );

        let result = engine.render("plugin-crud/model.rs.tera", &ctx);
        assert!(result.is_ok(), "render should succeed: {:?}", result);
        let output = result.unwrap();
        assert!(output.contains("users"));
        assert!(output.contains("User"));
        assert!(output.contains("id"));
    }

    #[tokio::test]
    async fn test_render_var_not_found() {
        let (_temp, template_dir) = setup_test_templates().await;
        let engine = TemplateEngine::init(&template_dir)
            .await
            .expect("init failed");

        let ctx = Context::new();
        let result = engine.render("plugin-crud/controller.rs.tera", &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CliError::VarNotFound { .. }));
    }

    #[tokio::test]
    async fn test_list_templates() {
        let (_temp, template_dir) = setup_test_templates().await;
        let engine = TemplateEngine::init(&template_dir)
            .await
            .expect("init failed");
        let templates = engine.list_templates();
        assert_eq!(templates.len(), 2);
        assert!(templates.contains(&"plugin-crud".to_string()));
        assert!(templates.contains(&"plugin-master-slave".to_string()));
    }

    #[tokio::test]
    async fn test_validate_template_type_valid() {
        let (_temp, template_dir) = setup_test_templates().await;
        let engine = TemplateEngine::init(&template_dir)
            .await
            .expect("init failed");
        assert!(engine.validate_template_type("plugin-crud").is_ok());
        assert!(engine.validate_template_type("plugin-master-slave").is_ok());
    }

    #[tokio::test]
    async fn test_validate_template_type_invalid() {
        let (_temp, template_dir) = setup_test_templates().await;
        let engine = TemplateEngine::init(&template_dir)
            .await
            .expect("init failed");
        let result = engine.validate_template_type("nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            CliError::UnknownTemplate {
                requested,
                available,
            } => {
                assert_eq!(requested, "nonexistent");
                assert!(available.contains(&"plugin-crud".to_string()));
            }
            _ => panic!("expected UnknownTemplate error"),
        }
    }

    #[test]
    fn test_parse_line_col() {
        let (line, col) = parse_line_col("error at line 5, column 3");
        assert_eq!(line, 5);
        assert_eq!(col, 3);
    }

    #[test]
    fn test_parse_line_col_no_match() {
        let (line, col) = parse_line_col("some generic error");
        assert_eq!(line, 0);
        assert_eq!(col, 0);
    }

    #[test]
    fn test_extract_variable_name() {
        let name = extract_variable_name("Variable `plugin_name` not found in context");
        assert_eq!(name, Some("plugin_name".to_string()));
    }

    #[test]
    fn test_extract_variable_name_no_match() {
        let name = extract_variable_name("some other error");
        assert_eq!(name, None);
    }

    /// Batch B 验收：加载真实模板目录（含跨目录继承）
    #[tokio::test]
    async fn test_load_real_templates() {
        let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
        let engine = TemplateEngine::init(&template_dir).await;
        assert!(
            engine.is_ok(),
            "Failed to load real templates: {:?}",
            engine.err()
        );
        let engine = engine.unwrap();

        let types = engine.list_templates();
        assert!(
            types.contains(&"plugin-crud".to_string()),
            "Missing plugin-crud"
        );
        assert!(
            types.contains(&"plugin-master-slave".to_string()),
            "Missing plugin-master-slave"
        );
    }

    /// Batch B 验收：渲染 CRUD model 模板
    #[tokio::test]
    async fn test_render_crud_model() {
        let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
        let engine = TemplateEngine::init(&template_dir)
            .await
            .expect("init failed");

        let mut ctx = Context::new();
        ctx.insert("plugin_name", "user-management");
        ctx.insert("table_name", "users");
        ctx.insert("class_name", "User");
        ctx.insert("template_type", "crud");
        ctx.insert("template_version", "1.0.0");
        ctx.insert("generated_at", "2026-08-11 10:00:00");
        ctx.insert("primary_key_name", "id");
        ctx.insert("primary_key_type", "i32");
        ctx.insert(
            "fields",
            &vec![
                serde_json::json!({"name": "id", "rust_type": "i32", "sql_type": "INT", "is_nullable": false, "is_primary_key": true, "is_indexed": false}),
                serde_json::json!({"name": "name", "rust_type": "String", "sql_type": "VARCHAR(255)", "is_nullable": false, "is_primary_key": false, "is_indexed": false}),
                serde_json::json!({"name": "age", "rust_type": "i32", "sql_type": "INT", "is_nullable": true, "is_primary_key": false, "is_indexed": false}),
            ],
        );

        let result = engine.render("plugin-crud/model.rs.tera", &ctx);
        assert!(result.is_ok(), "Render failed: {:?}", result.err());
        let output = result.unwrap();
        assert!(output.contains("pub struct User"));
        assert!(output.contains("pub id: i32"));
        assert!(output.contains("pub name: String"));
        assert!(output.contains("impl Model for User"));
        assert!(output.contains("\"users\""));
    }

    /// Batch B 验收：渲染 CRUD controller 模板
    #[tokio::test]
    async fn test_render_crud_controller() {
        let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
        let engine = TemplateEngine::init(&template_dir)
            .await
            .expect("init failed");

        let mut ctx = Context::new();
        ctx.insert("plugin_name", "user-management");
        ctx.insert("table_name", "users");
        ctx.insert("class_name", "User");
        ctx.insert("template_type", "crud");
        ctx.insert("template_version", "1.0.0");
        ctx.insert("generated_at", "2026-08-11 10:00:00");
        ctx.insert("primary_key_name", "id");
        ctx.insert("primary_key_type", "i32");
        ctx.insert("fields", &Vec::<serde_json::Value>::new());

        let result = engine.render("plugin-crud/controller.rs.tera", &ctx);
        assert!(result.is_ok(), "Render failed: {:?}", result.err());
        let output = result.unwrap();
        assert!(output.contains("pub struct UserController"));
        assert!(output.contains("async fn index"));
        assert!(output.contains("async fn create"));
        assert!(output.contains("async fn save"));
        assert!(output.contains("async fn read"));
        assert!(output.contains("async fn edit"));
        assert!(output.contains("async fn update"));
        assert!(output.contains("async fn delete"));
    }

    /// Batch B 验收：渲染主从 master_model 模板（跨目录继承）
    #[tokio::test]
    async fn test_render_master_model() {
        let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
        let engine = TemplateEngine::init(&template_dir)
            .await
            .expect("init failed");

        let mut ctx = Context::new();
        ctx.insert("plugin_name", "order-plugin");
        ctx.insert("template_type", "master-slave");
        ctx.insert("template_version", "1.0.0");
        ctx.insert("generated_at", "2026-08-11 10:00:00");
        ctx.insert("primary_key_name", "id");
        ctx.insert("primary_key_type", "i32");
        ctx.insert("master_table", "users");
        ctx.insert("slave_table", "orders");
        ctx.insert("master_class_name", "User");
        ctx.insert("slave_class_name", "Order");
        ctx.insert("foreign_key", "user_id");
        ctx.insert(
            "master_fields",
            &vec![
                serde_json::json!({"name": "id", "rust_type": "i32", "sql_type": "INT", "is_nullable": false, "is_primary_key": true, "is_indexed": false}),
                serde_json::json!({"name": "name", "rust_type": "String", "sql_type": "VARCHAR(255)", "is_nullable": false, "is_primary_key": false, "is_indexed": false}),
            ],
        );
        ctx.insert(
            "slave_fields",
            &vec![
                serde_json::json!({"name": "id", "rust_type": "i32", "sql_type": "INT", "is_nullable": false, "is_primary_key": true, "is_indexed": false}),
                serde_json::json!({"name": "user_id", "rust_type": "i32", "sql_type": "INT", "is_nullable": false, "is_primary_key": false, "is_indexed": false}),
                serde_json::json!({"name": "total", "rust_type": "f64", "sql_type": "DOUBLE", "is_nullable": false, "is_primary_key": false, "is_indexed": false}),
            ],
        );
        ctx.insert("fields", &Vec::<serde_json::Value>::new());

        let result = engine.render("plugin-master-slave/master_model.rs.tera", &ctx);
        assert!(result.is_ok(), "Render failed: {:?}", result.err());
        let output = result.unwrap();
        assert!(output.contains("pub struct User"));
        assert!(output.contains("\"users\""));
        assert!(output.contains("pub id: i32"));
        assert!(output.contains("pub name: String"));
    }

    /// Batch B 验收：渲染主从 slave_model 模板
    #[tokio::test]
    async fn test_render_slave_model() {
        let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
        let engine = TemplateEngine::init(&template_dir)
            .await
            .expect("init failed");

        let mut ctx = Context::new();
        ctx.insert("plugin_name", "order-plugin");
        ctx.insert("template_type", "master-slave");
        ctx.insert("template_version", "1.0.0");
        ctx.insert("generated_at", "2026-08-11 10:00:00");
        ctx.insert("primary_key_name", "id");
        ctx.insert("primary_key_type", "i32");
        ctx.insert("master_table", "users");
        ctx.insert("slave_table", "orders");
        ctx.insert("master_class_name", "User");
        ctx.insert("slave_class_name", "Order");
        ctx.insert("foreign_key", "user_id");
        ctx.insert("master_fields", &Vec::<serde_json::Value>::new());
        ctx.insert(
            "slave_fields",
            &vec![
                serde_json::json!({"name": "id", "rust_type": "i32", "sql_type": "INT", "is_nullable": false, "is_primary_key": true, "is_indexed": false}),
                serde_json::json!({"name": "user_id", "rust_type": "i32", "sql_type": "INT", "is_nullable": false, "is_primary_key": false, "is_indexed": false}),
            ],
        );
        ctx.insert("fields", &Vec::<serde_json::Value>::new());

        let result = engine.render("plugin-master-slave/slave_model.rs.tera", &ctx);
        assert!(result.is_ok(), "Render failed: {:?}", result.err());
        let output = result.unwrap();
        assert!(output.contains("pub struct Order"));
        assert!(output.contains("\"orders\""));
        assert!(output.contains("pub user_id: i32"));
    }

    /// Batch B 验收：渲染主从 migration 模板（含外键约束）
    #[tokio::test]
    async fn test_render_master_slave_migration() {
        let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
        let engine = TemplateEngine::init(&template_dir)
            .await
            .expect("init failed");

        let mut ctx = Context::new();
        ctx.insert("plugin_name", "order-plugin");
        ctx.insert("template_type", "master-slave");
        ctx.insert("template_version", "1.0.0");
        ctx.insert("generated_at", "2026-08-11 10:00:00");
        ctx.insert("primary_key_name", "id");
        ctx.insert("primary_key_type", "i32");
        ctx.insert("master_table", "users");
        ctx.insert("slave_table", "orders");
        ctx.insert("master_class_name", "User");
        ctx.insert("slave_class_name", "Order");
        ctx.insert("foreign_key", "user_id");
        ctx.insert(
            "master_fields",
            &vec![
                serde_json::json!({"name": "id", "rust_type": "i32", "sql_type": "INT", "is_nullable": false, "is_primary_key": true, "is_indexed": false}),
                serde_json::json!({"name": "name", "rust_type": "String", "sql_type": "VARCHAR(255)", "is_nullable": false, "is_primary_key": false, "is_indexed": false}),
            ],
        );
        ctx.insert(
            "slave_fields",
            &vec![
                serde_json::json!({"name": "id", "rust_type": "i32", "sql_type": "INT", "is_nullable": false, "is_primary_key": true, "is_indexed": false}),
                serde_json::json!({"name": "user_id", "rust_type": "i32", "sql_type": "INT", "is_nullable": false, "is_primary_key": false, "is_indexed": false}),
            ],
        );

        let result = engine.render("plugin-master-slave/migration.sql.tera", &ctx);
        assert!(result.is_ok(), "Render failed: {:?}", result.err());
        let output = result.unwrap();
        assert!(output.contains("CREATE TABLE IF NOT EXISTS users"));
        assert!(output.contains("CREATE TABLE IF NOT EXISTS orders"));
        assert!(output.contains("FOREIGN KEY (user_id)"));
        assert!(output.contains("REFERENCES users (id)"));
    }
}
