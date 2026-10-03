//! sz-rust-codegen-ui 错误类型
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum CodegenUiError {
    #[error("内部错误: {0} (code: 17190)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17191)")]
    Config(String),
    #[error("参数无效: {0} (code: 17192)")]
    InvalidParam(String),
    #[error("模板不存在: {0} (code: 17193)")]
    TemplateNotFound(String),
    /// 模板渲染失败（spec §5.27 异常 1）
    #[error("模板渲染失败: {0} (code: 17194)")]
    RenderFailed(String),
    /// 生成代码编译失败（spec §5.27 异常 2）
    #[error("生成代码编译失败: {0} (code: 17195)")]
    CompileFailed(String),
    /// 包含硬编码密钥（spec §5.27 禁止项）
    #[error("生成代码包含硬编码密钥/密码 (code: 17196)")]
    HardcodedSecret,
}
