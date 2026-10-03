//! sz-rust-monitor-panel 错误类型
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum MonitorPanelError {
    #[error("内部错误: {0} (code: 17180)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17181)")]
    Config(String),
    #[error("参数无效: {0} (code: 17182)")]
    InvalidParam(String),
    #[error("仪表盘不存在: {0} (code: 17183)")]
    DashboardNotFound(String),
    /// Prometheus 不可达（spec §5.26 异常 1）
    #[error("Prometheus 不可达: {0} (code: 17184)")]
    PrometheusUnreachable(String),
    /// Grafana 嵌入失败（spec §5.26 异常 2）
    #[error("Grafana 嵌入失败: {0} (code: 17185)")]
    GrafanaEmbedFailed(String),
}
