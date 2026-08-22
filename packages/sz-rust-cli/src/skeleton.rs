//! 插件骨架元数据与产物结构定义
//!
//! 对应 design.md 第 2.3.2 节，定义模板元数据规范与生成产物数据结构。

use serde::{Deserialize, Serialize};

/// 模板元数据（对应每个模板目录下的 `template.json`）
///
/// 声明模板的名称、版本、描述与所需变量列表。
/// `required_variables` 用于在渲染前校验上下文是否提供全部必需变量。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplateMeta {
    /// 模板名称（如 `"crud"`、`"master-slave"`）
    pub name: String,
    /// 模板版本（语义化版本，如 `"1.0.0"`）
    pub version: String,
    /// 模板描述
    pub description: String,
    /// 渲染所需变量名列表
    pub required_variables: Vec<String>,
}

impl TemplateMeta {
    /// 创建新的模板元数据
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        description: impl Into<String>,
        required_variables: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            description: description.into(),
            required_variables,
        }
    }

    /// 校验上下文是否包含全部必需变量
    ///
    /// 返回缺失变量列表（空表示全部满足）
    pub fn missing_variables(&self, provided: &[&str]) -> Vec<String> {
        self.required_variables
            .iter()
            .filter(|req| !provided.iter().any(|p| p == req))
            .cloned()
            .collect()
    }
}

/// 生成产物中的源代码文件
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// 相对路径（如 `src/model.rs`）
    pub path: String,
    /// 文件内容
    pub content: String,
}

/// 生成产物中的迁移文件
#[derive(Debug, Clone)]
pub struct MigrationFile {
    /// 迁移文件名（如 `20260811_create_users.sql`）
    pub name: String,
    /// SQL 内容
    pub content: String,
}

/// 插件骨架生成产物
#[derive(Debug, Clone)]
pub struct PluginSkeleton {
    /// 插件名称
    pub plugin_name: String,
    /// 模板类型
    pub template_type: String,
    /// 源代码文件列表
    pub source_files: Vec<SourceFile>,
    /// 迁移文件列表
    pub migrations: Vec<MigrationFile>,
    /// manifest.json 内容
    pub manifest: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_meta_new() {
        let meta = TemplateMeta::new(
            "crud",
            "1.0.0",
            "CRUD plugin template",
            vec!["plugin_name".to_string(), "table_name".to_string()],
        );
        assert_eq!(meta.name, "crud");
        assert_eq!(meta.version, "1.0.0");
        assert_eq!(meta.required_variables.len(), 2);
    }

    #[test]
    fn test_template_meta_serialize_deserialize() {
        let meta = TemplateMeta::new(
            "crud",
            "1.0.0",
            "CRUD plugin template",
            vec!["plugin_name".to_string()],
        );
        let json = serde_json::to_string(&meta).expect("serialize failed");
        let deserialized: TemplateMeta = serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(meta, deserialized);
    }

    #[test]
    fn test_missing_variables_all_provided() {
        let meta = TemplateMeta::new(
            "crud",
            "1.0.0",
            "",
            vec!["plugin_name".to_string(), "table_name".to_string()],
        );
        let provided = vec!["plugin_name", "table_name"];
        let missing = meta.missing_variables(&provided);
        assert!(missing.is_empty());
    }

    #[test]
    fn test_missing_variables_some_missing() {
        let meta = TemplateMeta::new(
            "crud",
            "1.0.0",
            "",
            vec![
                "plugin_name".to_string(),
                "table_name".to_string(),
                "fields".to_string(),
            ],
        );
        let provided = vec!["plugin_name"];
        let missing = meta.missing_variables(&provided);
        assert_eq!(missing.len(), 2);
        assert!(missing.contains(&"table_name".to_string()));
        assert!(missing.contains(&"fields".to_string()));
    }
}
