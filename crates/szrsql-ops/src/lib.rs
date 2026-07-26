//! SzRSQL 运维监控：ASH 采样 / AWR 报告 / 告警 / 慢查询 / OpenTelemetry / Grafana / 火焰图。
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.8 ~ 7d.12, 7d.16, 7d.20 节。

#![allow(dead_code)]

pub mod alerting;
pub mod ash;
pub mod grafana;
pub mod otel;
pub mod pprof;
pub mod query_stats;
pub mod slow_query;
pub mod wait_events;

/// 返回 crate 版本号，供 workspace 骨架冒烟测试使用。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_nonempty() {
        assert!(!version().is_empty());
    }
}
