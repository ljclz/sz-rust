//! sz-rust-upload 错误类型
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum UploadError {
    #[error("内部错误: {0} (code: 17150)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17151)")]
    Config(String),
    #[error("参数无效: {0} (code: 17152)")]
    InvalidParam(String),
    #[error("分片不完整 (code: 17153)")]
    IncompleteChunks,
    #[error("校验失败: {0} (code: 17154)")]
    ChecksumFailed(String),
    /// 文件超限（spec §5.23 规则 6）
    #[error("文件超限: {0} > {1} (code: 17155)")]
    SizeExceeded(u64, u64),
    /// 分片丢失（spec §5.23 异常 1）
    #[error("分片丢失: {0:?} (code: 17156)")]
    ChunksMissing(Vec<u32>),
    /// 病毒扫描未通过（spec §5.23 规则 4）
    #[error("病毒扫描未通过 (code: 17157)")]
    VirusDetected,
    /// 扫描引擎不可达（spec §5.23 异常 2）
    #[error("扫描引擎不可达: {0} (code: 17158)")]
    ScannerUnreachable(String),
    /// 存储后端错误
    #[error("存储错误: {0} (code: 17159)")]
    StorageError(String),
}
