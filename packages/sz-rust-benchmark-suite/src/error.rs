//! sz-rust-benchmark-suite 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum BenchmarkError {
    #[error("内部错误: {0} (code: 17070)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17071)")]
    Config(String),
    #[error("参数无效: {0} (code: 17072)")]
    InvalidParam(String),
    #[error("性能回归: {0} (code: 17073)")]
    Regression(String),
    #[error("预算超限: {0} (code: 17074)")]
    BudgetExceeded(String),
}
