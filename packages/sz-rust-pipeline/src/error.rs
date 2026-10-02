//! sz-rust-pipeline 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum PipelineError {
    #[error("内部错误: {0} (code: 17050)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17051)")]
    Config(String),
    #[error("参数无效: {0} (code: 17052)")]
    InvalidParam(String),
    #[error("背压超限 (code: 17053)")]
    BackpressureExceeded,
    #[error("阶段执行失败: {0} (code: 17054)")]
    StageFailed(String),
}
