//! 数据范围上下文 — 从 guard `UserContext` 转换而来的轻量级上下文
//!
//! orm-facade 不依赖 mvc-facade，因此定义独立上下文结构体。
//! 业务层在调用 `data_scope()` 前从 `UserContext` 转换为 `DataScopeContext`。

/// 数据范围上下文（从 guard `UserContext` 转换）
#[derive(Debug, Clone)]
pub struct DataScopeContext {
    /// 用户 ID
    pub user_id: i64,
    /// 部门 ID
    pub dept_id: i64,
    /// 是否超级管理员（绕过数据范围）
    pub is_super: bool,
}

impl DataScopeContext {
    /// 创建上下文
    pub fn new(user_id: i64, dept_id: i64, is_super: bool) -> Self {
        Self {
            user_id,
            dept_id,
            is_super,
        }
    }
}

impl Default for DataScopeContext {
    fn default() -> Self {
        Self::new(0, 0, false)
    }
}
