use std::collections::HashMap;

use crate::source::CapabilitySource;

/// Capability Registry 指标快照。
///
/// 由 [`CapabilityRegistry::metrics`](crate::CapabilityRegistry::metrics) 返回，
/// 可用于 Prometheus 指标暴露。
#[derive(Debug, Clone, serde::Serialize)]
pub struct CapMetrics {
    /// 已注册能力总数。
    pub total: usize,
    /// 按来源分组的能力数量。
    pub by_source: HashMap<CapabilitySource, usize>,
    /// 累计调用次数。
    pub call_total: u64,
}
