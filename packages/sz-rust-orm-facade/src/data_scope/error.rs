//! 数据范围错误类型

use thiserror::Error;

/// 数据范围控制错误
#[derive(Debug, Clone, Error)]
pub enum DataScopeError {
    /// 请求上下文缺失 `UserContext`（spec 5.3.1.8 + 4.3.3）
    #[error("missing user context in request extension")]
    MissingUserContext,

    /// 部门树不可用（缓存未初始化或 provider 返回错误）
    #[error("dept tree unavailable: {0}")]
    DeptTreeUnavailable(String),

    /// 规则无效（字段缺失或模式不匹配）
    #[error("invalid data scope rule: {0}")]
    InvalidRule(String),

    /// 自定义条件不安全（非参数化绑定）
    #[error("unsafe custom condition: {0}")]
    UnsafeCustomCondition(String),

    /// 自定义条件生成器未注册
    #[error("custom generator not found: {0}")]
    GeneratorNotFound(String),
}

impl DataScopeError {
    /// 错误码（用于指标 label 和日志）
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::MissingUserContext => "DATA_SCOPE_NO_USER_CONTEXT",
            Self::DeptTreeUnavailable(_) => "DATA_SCOPE_DEPT_TREE_UNAVAILABLE",
            Self::InvalidRule(_) => "DATA_SCOPE_INVALID_RULE",
            Self::UnsafeCustomCondition(_) => "DATA_SCOPE_UNSAFE_CUSTOM",
            Self::GeneratorNotFound(_) => "DATA_SCOPE_GENERATOR_NOT_FOUND",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes() {
        assert_eq!(
            DataScopeError::MissingUserContext.error_code(),
            "DATA_SCOPE_NO_USER_CONTEXT"
        );
        assert_eq!(
            DataScopeError::DeptTreeUnavailable("x".into()).error_code(),
            "DATA_SCOPE_DEPT_TREE_UNAVAILABLE"
        );
        assert_eq!(
            DataScopeError::InvalidRule("x".into()).error_code(),
            "DATA_SCOPE_INVALID_RULE"
        );
        assert_eq!(
            DataScopeError::UnsafeCustomCondition("x".into()).error_code(),
            "DATA_SCOPE_UNSAFE_CUSTOM"
        );
        assert_eq!(
            DataScopeError::GeneratorNotFound("x".into()).error_code(),
            "DATA_SCOPE_GENERATOR_NOT_FOUND"
        );
    }
}
