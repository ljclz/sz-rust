//! 模型元信息

use serde::{Deserialize, Serialize};

use super::{FieldMetadata, RelationMetadata, ValidationRule};

/// 模型元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadata {
    /// 模型名（结构体名）
    pub name: String,
    /// 表名
    pub table_name: String,
    /// 模块名（snake_case）
    pub module_name: String,
    /// 字段列表
    pub fields: Vec<FieldMetadata>,
    /// 关系列表
    pub relations: Vec<RelationMetadata>,
    /// 验证规则
    pub validations: Vec<ValidationRule>,
    /// 文档注释
    pub doc_comment: Option<String>,
}

impl ModelMetadata {
    /// 返回主键字段引用
    pub fn primary_key(&self) -> Option<&FieldMetadata> {
        self.fields.iter().find(|f| f.is_primary_key)
    }

    /// 返回可写字段（排除主键与自动时间戳）
    pub fn writable_fields(&self) -> Vec<&FieldMetadata> {
        self.fields
            .iter()
            .filter(|f| !f.is_primary_key && !f.is_auto_timestamp)
            .collect()
    }
}
