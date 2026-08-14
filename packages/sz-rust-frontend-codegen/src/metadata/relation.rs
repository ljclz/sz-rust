//! 关系元信息

use serde::{Deserialize, Serialize};

/// 关系类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// 一对一
    HasOne,
    /// 一对多
    HasMany,
    /// 多对一
    BelongsTo,
    /// 多对多
    ManyToMany,
}

/// 关系元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationMetadata {
    /// 关系类型
    pub kind: RelationKind,
    /// 目标模型名
    pub target_model: String,
    /// 外键字段
    pub foreign_key: Option<String>,
    /// 中间表（仅 ManyToMany）
    pub through_table: Option<String>,
}
