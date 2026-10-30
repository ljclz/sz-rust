// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::Path;

use crate::error::MigrationResult;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6Model {
    pub class_name: String,
    pub table_name: String,
    pub fields: Vec<Tp6Field>,
    pub relations: Vec<Tp6Relation>,
    pub validation_rules: Vec<Tp6ValidationRule>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6Field {
    pub name: String,
    pub field_type: String,
    pub nullable: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6Relation {
    pub name: String,
    pub relation_type: String,
    pub related_model: String,
    pub foreign_key: String,
    pub local_key: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6ValidationRule {
    pub field: String,
    pub rule: String,
}

pub struct Tp6ModelParser {
    class_re: regex::Regex,
    table_re: regex::Regex,
    has_one_re: regex::Regex,
    has_many_re: regex::Regex,
    belongs_to_re: regex::Regex,
}

impl Tp6ModelParser {
    pub fn new() -> Self {
        Self {
            class_re: regex::Regex::new(r"class\s+(\w+)\s+extends\s+Model").expect("class regex"),
            table_re: regex::Regex::new(r#"\$table\s*=\s*['"]([^'"]+)['"]"#).expect("table regex"),
            has_one_re: regex::Regex::new(
                r#"hasOne\s*\(\s*(\w+)::class\s*,\s*['"](\w+)['"]\s*,\s*['"](\w+)['"]\s*\)"#,
            )
            .expect("hasOne regex"),
            has_many_re: regex::Regex::new(
                r#"hasMany\s*\(\s*(\w+)::class\s*,\s*['"](\w+)['"]\s*,\s*['"](\w+)['"]\s*\)"#,
            )
            .expect("hasMany regex"),
            belongs_to_re: regex::Regex::new(
                r#"belongsTo\s*\(\s*(\w+)::class\s*,\s*['"](\w+)['"]\s*,\s*['"](\w+)['"]\s*\)"#,
            )
            .expect("belongsTo regex"),
        }
    }

    pub async fn parse(&self, model_dir: &Path) -> MigrationResult<Vec<Tp6Model>> {
        let mut models = Vec::new();
        if !model_dir.exists() {
            return Ok(models);
        }
        let mut entries = tokio::fs::read_dir(model_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "php") {
                let content = tokio::fs::read_to_string(&path).await?;
                if let Some(model) = self.parse_content(&content) {
                    models.push(model);
                }
            }
        }
        Ok(models)
    }

    fn parse_content(&self, content: &str) -> Option<Tp6Model> {
        let class_caps = self.class_re.captures(content)?;
        let class_name = class_caps.get(1)?.as_str().to_string();
        let table_name = self
            .table_re
            .captures(content)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .unwrap_or_else(|| format!("{}s", class_name.to_lowercase()));
        let mut relations = Vec::new();
        for caps in self.has_one_re.captures_iter(content) {
            relations.push(Tp6Relation {
                name: "hasOne".into(),
                relation_type: "hasOne".into(),
                related_model: caps
                    .get(1)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                foreign_key: caps
                    .get(2)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                local_key: caps
                    .get(3)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
            });
        }
        for caps in self.has_many_re.captures_iter(content) {
            relations.push(Tp6Relation {
                name: "hasMany".into(),
                relation_type: "hasMany".into(),
                related_model: caps
                    .get(1)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                foreign_key: caps
                    .get(2)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                local_key: caps
                    .get(3)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
            });
        }
        for caps in self.belongs_to_re.captures_iter(content) {
            relations.push(Tp6Relation {
                name: "belongsTo".into(),
                relation_type: "belongsTo".into(),
                related_model: caps
                    .get(1)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                foreign_key: caps
                    .get(2)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                local_key: caps
                    .get(3)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
            });
        }
        Some(Tp6Model {
            class_name,
            table_name,
            fields: Vec::new(),
            relations,
            validation_rules: Vec::new(),
        })
    }
}

impl Default for Tp6ModelParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_with_table() {
        let parser = Tp6ModelParser::new();
        let content = r#"
<?php
namespace app\model;
use think\Model;
class User extends Model {
    protected $table = 'users';
    public function profile() {
        return $this->hasOne(Profile::class, 'user_id', 'id');
    }
    public function orders() {
        return $this->hasMany(Order::class, 'user_id', 'id');
    }
}
"#;
        let model = parser.parse_content(content).unwrap();
        assert_eq!(model.class_name, "User");
        assert_eq!(model.table_name, "users");
        assert_eq!(model.relations.len(), 2);
        assert_eq!(model.relations[0].relation_type, "hasOne");
        assert_eq!(model.relations[0].related_model, "Profile");
        assert_eq!(model.relations[1].relation_type, "hasMany");
        assert_eq!(model.relations[1].related_model, "Order");
    }

    #[test]
    fn parse_model_without_table_defaults() {
        let parser = Tp6ModelParser::new();
        let content = r#"class Product extends Model { }"#;
        let model = parser.parse_content(content).unwrap();
        assert_eq!(model.class_name, "Product");
        assert_eq!(model.table_name, "products");
    }

    #[test]
    fn parse_non_model_returns_none() {
        let parser = Tp6ModelParser::new();
        let content = "class Utility { }";
        assert!(parser.parse_content(content).is_none());
    }
}
