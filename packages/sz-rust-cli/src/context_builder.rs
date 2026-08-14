//! 模板上下文构建器
//!
//! 对应 design.md 第 2.2.2.5 节，从 CLI 参数构建 Tera 渲染上下文。

use tera::Context;

use crate::error::CliError;
use crate::field_parser::{Field, FieldParser};

/// `make:plugin` 命令参数
#[derive(Debug, Clone)]
pub struct PluginCommandArgs {
    /// 模板类型（必填，如 "crud" 或 "master-slave"）
    pub template: String,
    /// 插件名称（必填）
    pub name: String,
    /// 表名（可选，默认取插件名 snake_case）
    pub table: Option<String>,
    /// 字段定义（可选，如 "id:i32:pk,name:String"）
    pub fields: Option<String>,
    /// 是否强制覆盖已存在目录
    pub force: bool,
    /// 输出目录（可选，默认 plugins/<name>/）
    pub output: Option<String>,
    /// 主表名（主从模板，可选）
    pub master: Option<String>,
    /// 从表名（主从模板，可选）
    pub slave: Option<String>,
    /// 主表字段定义（主从模板，可选）
    pub master_fields: Option<String>,
    /// 从表字段定义（主从模板，可选）
    pub slave_fields: Option<String>,
    /// 外键字段名（主从模板，可选）
    pub foreign_key: Option<String>,
}

/// 模板上下文构建器
pub struct TemplateContextBuilder {
    args: PluginCommandArgs,
}

impl TemplateContextBuilder {
    /// 创建新的上下文构建器
    pub fn new(args: PluginCommandArgs) -> Self {
        Self { args }
    }

    /// 构建 CRUD 模板上下文
    ///
    /// 含 6 个必需变量：plugin_name, table_name, class_name, fields, module_path, template_version
    /// 外加 generated_at, primary_key_name, primary_key_type
    pub fn build(&self) -> Result<Context, CliError> {
        let table_name = self
            .args
            .table
            .clone()
            .unwrap_or_else(|| to_snake_case(&self.args.name));

        let class_name = to_pascal_case(&table_name);

        let fields_str = self
            .args
            .fields
            .as_deref()
            .unwrap_or("id:i32:pk,name:String");
        let fields = FieldParser::parse(fields_str)?;

        let (pk_name, pk_type) = find_primary_key(&fields);

        let module_path = format!("plugins::{}", to_snake_case(&self.args.name));

        let fields_json: Vec<serde_json::Value> = fields
            .iter()
            .map(|f| {
                serde_json::json!({
                    "name": f.name,
                    "rust_type": f.rust_type,
                    "sql_type": f.sql_type,
                    "is_nullable": f.is_nullable,
                    "is_primary_key": f.is_primary_key,
                    "is_indexed": f.is_indexed,
                })
            })
            .collect();

        let mut ctx = Context::new();
        ctx.insert("plugin_name", &self.args.name);
        ctx.insert("table_name", &table_name);
        ctx.insert("class_name", &class_name);
        ctx.insert("fields", &fields_json);
        ctx.insert("module_path", &module_path);
        ctx.insert("template_type", &self.args.template);
        ctx.insert("template_version", "1.0.0");
        ctx.insert("generated_at", &current_timestamp());
        ctx.insert("primary_key_name", &pk_name);
        ctx.insert("primary_key_type", &pk_type);

        Ok(ctx)
    }

    /// 构建主从模板上下文
    ///
    /// 额外含 master_table, slave_table, foreign_key, master_fields, slave_fields
    pub fn build_master_slave(&self) -> Result<Context, CliError> {
        let master_table = self.args.master.clone().ok_or_else(|| {
            CliError::Generic("--master is required for master-slave template".to_string())
        })?;

        let slave_table = self.args.slave.clone().ok_or_else(|| {
            CliError::Generic("--slave is required for master-slave template".to_string())
        })?;

        if master_table == slave_table {
            return Err(CliError::MasterSlaveSame);
        }

        let foreign_key = self.args.foreign_key.clone().ok_or_else(|| {
            CliError::Generic("--foreign-key is required for master-slave template".to_string())
        })?;

        let master_fields_str = self
            .args
            .master_fields
            .as_deref()
            .unwrap_or("id:i32:pk,name:String");
        let slave_fields_str = self.args.slave_fields.as_deref().unwrap_or("id:i32:pk");

        let master_fields = FieldParser::parse(master_fields_str)?;
        let slave_fields = FieldParser::parse(slave_fields_str)?;

        crate::validator::InputValidator::validate_foreign_key(&foreign_key, &slave_fields)?;

        let (master_pk_name, master_pk_type) = find_primary_key(&master_fields);

        let master_fields_json: Vec<serde_json::Value> = fields_to_json(&master_fields);
        let slave_fields_json: Vec<serde_json::Value> = fields_to_json(&slave_fields);

        let master_class_name = to_pascal_case(&master_table);
        let slave_class_name = to_pascal_case(&slave_table);
        let module_path = format!("plugins::{}", to_snake_case(&self.args.name));

        let mut ctx = Context::new();
        ctx.insert("plugin_name", &self.args.name);
        ctx.insert("table_name", &master_table);
        ctx.insert("class_name", &master_class_name);
        ctx.insert("fields", &master_fields_json);
        ctx.insert("module_path", &module_path);
        ctx.insert("template_type", &self.args.template);
        ctx.insert("template_version", "1.0.0");
        ctx.insert("generated_at", &current_timestamp());
        ctx.insert("primary_key_name", &master_pk_name);
        ctx.insert("primary_key_type", &master_pk_type);

        ctx.insert("master_table", &master_table);
        ctx.insert("slave_table", &slave_table);
        ctx.insert("master_class_name", &master_class_name);
        ctx.insert("slave_class_name", &slave_class_name);
        ctx.insert("master_fields", &master_fields_json);
        ctx.insert("slave_fields", &slave_fields_json);
        ctx.insert("foreign_key", &foreign_key);

        Ok(ctx)
    }

    /// 返回参数引用
    pub fn args(&self) -> &PluginCommandArgs {
        &self.args
    }
}

/// snake_case 转换
fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else if ch == '-' {
            result.push('_');
        } else {
            result.push(ch);
        }
    }
    result
}

/// PascalCase 转换
fn to_pascal_case(s: &str) -> String {
    let mut result = String::new();
    let mut next_upper = true;
    for ch in s.chars() {
        if ch == '_' || ch == '-' || ch == ' ' {
            next_upper = true;
        } else if next_upper {
            result.push(ch.to_ascii_uppercase());
            next_upper = false;
        } else {
            result.push(ch);
        }
    }
    result
}

/// 查找主键字段
fn find_primary_key(fields: &[Field]) -> (String, String) {
    for f in fields {
        if f.is_primary_key {
            return (f.name.clone(), f.rust_type.clone());
        }
    }
    ("id".to_string(), "i32".to_string())
}

/// 字段列表转 JSON
fn fields_to_json(fields: &[Field]) -> Vec<serde_json::Value> {
    fields
        .iter()
        .map(|f| {
            serde_json::json!({
                "name": f.name,
                "rust_type": f.rust_type,
                "sql_type": f.sql_type,
                "is_nullable": f.is_nullable,
                "is_primary_key": f.is_primary_key,
                "is_indexed": f.is_indexed,
            })
        })
        .collect()
}

/// 当前时间戳
fn current_timestamp() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_crud_args() -> PluginCommandArgs {
        PluginCommandArgs {
            template: "crud".to_string(),
            name: "user-management".to_string(),
            table: Some("users".to_string()),
            fields: Some("id:i32:pk,name:String,age:i32".to_string()),
            force: false,
            output: None,
            master: None,
            slave: None,
            master_fields: None,
            slave_fields: None,
            foreign_key: None,
        }
    }

    #[test]
    fn test_build_crud_context() {
        let builder = TemplateContextBuilder::new(make_crud_args());
        let ctx = builder.build().unwrap();

        assert_eq!(ctx.get("plugin_name").unwrap(), "user-management");
        assert_eq!(ctx.get("table_name").unwrap(), "users");
        assert_eq!(ctx.get("class_name").unwrap(), "Users");
        assert_eq!(ctx.get("template_type").unwrap(), "crud");
        assert!(ctx.get("generated_at").is_some());
        assert_eq!(ctx.get("primary_key_name").unwrap(), "id");
        assert_eq!(ctx.get("primary_key_type").unwrap(), "i32");
    }

    #[test]
    fn test_build_crud_default_table() {
        let mut args = make_crud_args();
        args.table = None;
        let builder = TemplateContextBuilder::new(args);
        let ctx = builder.build().unwrap();
        assert_eq!(ctx.get("table_name").unwrap(), "user_management");
    }

    #[test]
    fn test_build_crud_default_fields() {
        let mut args = make_crud_args();
        args.fields = None;
        let builder = TemplateContextBuilder::new(args);
        let ctx = builder.build().unwrap();
        assert!(ctx.get("fields").is_some());
    }

    #[test]
    fn test_build_master_slave_context() {
        let args = PluginCommandArgs {
            template: "master-slave".to_string(),
            name: "order-plugin".to_string(),
            table: None,
            fields: None,
            force: false,
            output: None,
            master: Some("users".to_string()),
            slave: Some("orders".to_string()),
            master_fields: Some("id:i32:pk,name:String".to_string()),
            slave_fields: Some("id:i32:pk,user_id:i32,total:f64".to_string()),
            foreign_key: Some("user_id".to_string()),
        };
        let builder = TemplateContextBuilder::new(args);
        let ctx = builder.build_master_slave().unwrap();

        assert_eq!(ctx.get("master_table").unwrap(), "users");
        assert_eq!(ctx.get("slave_table").unwrap(), "orders");
        assert_eq!(ctx.get("master_class_name").unwrap(), "Users");
        assert_eq!(ctx.get("slave_class_name").unwrap(), "Orders");
        assert_eq!(ctx.get("foreign_key").unwrap(), "user_id");
    }

    #[test]
    fn test_build_master_slave_same_table() {
        let args = PluginCommandArgs {
            template: "master-slave".to_string(),
            name: "test".to_string(),
            table: None,
            fields: None,
            force: false,
            output: None,
            master: Some("users".to_string()),
            slave: Some("users".to_string()),
            master_fields: Some("id:i32:pk".to_string()),
            slave_fields: Some("id:i32:pk".to_string()),
            foreign_key: Some("id".to_string()),
        };
        let builder = TemplateContextBuilder::new(args);
        let result = builder.build_master_slave();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CliError::MasterSlaveSame));
    }

    #[test]
    fn test_build_master_slave_fk_not_found() {
        let args = PluginCommandArgs {
            template: "master-slave".to_string(),
            name: "test".to_string(),
            table: None,
            fields: None,
            force: false,
            output: None,
            master: Some("users".to_string()),
            slave: Some("orders".to_string()),
            master_fields: Some("id:i32:pk,name:String".to_string()),
            slave_fields: Some("id:i32:pk,total:f64".to_string()),
            foreign_key: Some("user_id".to_string()),
        };
        let builder = TemplateContextBuilder::new(args);
        let result = builder.build_master_slave();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CliError::ForeignKeyNotFound(_)
        ));
    }

    #[test]
    fn test_to_snake_case() {
        assert_eq!(to_snake_case("UserManagement"), "user_management");
        assert_eq!(to_snake_case("user-management"), "user_management");
        assert_eq!(to_snake_case("user_management"), "user_management");
    }

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("users"), "Users");
        assert_eq!(to_pascal_case("user_orders"), "UserOrders");
        assert_eq!(to_pascal_case("user-orders"), "UserOrders");
        assert_eq!(to_pascal_case("UserOrders"), "UserOrders");
    }

    #[test]
    fn test_find_primary_key() {
        let fields = vec![
            Field {
                name: "name".to_string(),
                rust_type: "String".to_string(),
                sql_type: "VARCHAR(255)".to_string(),
                is_nullable: false,
                is_primary_key: false,
                is_indexed: false,
            },
            Field {
                name: "id".to_string(),
                rust_type: "i64".to_string(),
                sql_type: "BIGINT".to_string(),
                is_nullable: false,
                is_primary_key: true,
                is_indexed: false,
            },
        ];
        let (name, ty) = find_primary_key(&fields);
        assert_eq!(name, "id");
        assert_eq!(ty, "i64");
    }

    #[test]
    fn test_find_primary_key_default() {
        let fields = vec![Field {
            name: "name".to_string(),
            rust_type: "String".to_string(),
            sql_type: "VARCHAR(255)".to_string(),
            is_nullable: false,
            is_primary_key: false,
            is_indexed: false,
        }];
        let (name, ty) = find_primary_key(&fields);
        assert_eq!(name, "id");
        assert_eq!(ty, "i32");
    }
}
