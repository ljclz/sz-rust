// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 自定义仪表盘（spec §5.26 规则 2，§6.26 规则 2）
//!
//! 可配置指标/图表类型/布局。

use std::time::Duration;

/// 图表类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChartType {
    /// 折线图
    Line,
    /// 柱状图
    Bar,
    /// 饼图
    Pie,
    /// 仪表盘
    Gauge,
    /// 表格
    Table,
    /// 统计值
    Stat,
}

impl ChartType {
    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Bar => "bar",
            Self::Pie => "pie",
            Self::Gauge => "gauge",
            Self::Table => "table",
            Self::Stat => "stat",
        }
    }
}

/// 仪表盘面板
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DashboardPanel {
    /// 面板 ID
    pub id: String,
    /// 面板标题
    pub title: String,
    /// 图表类型
    pub chart_type: ChartType,
    /// 查询语句
    pub query: String,
    /// 位置 (x, y)
    pub position: (u32, u32),
    /// 大小 (width, height)
    pub size: (u32, u32),
}

impl DashboardPanel {
    /// 创建面板
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        chart_type: ChartType,
        query: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            chart_type,
            query: query.into(),
            position: (0, 0),
            size: (6, 4),
        }
    }

    /// 设置位置
    pub fn with_position(mut self, x: u32, y: u32) -> Self {
        self.position = (x, y);
        self
    }

    /// 设置大小
    pub fn with_size(mut self, w: u32, h: u32) -> Self {
        self.size = (w, h);
        self
    }
}

/// 自定义仪表盘配置（spec §5.26 规则 2）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Dashboard {
    /// 仪表盘 ID
    pub id: String,
    /// 仪表盘标题
    pub title: String,
    /// 面板列表
    pub panels: Vec<DashboardPanel>,
    /// 刷新间隔
    pub refresh_interval: Duration,
}

impl Dashboard {
    /// 创建仪表盘
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            panels: Vec::new(),
            refresh_interval: Duration::from_secs(30),
        }
    }

    /// 添加面板
    pub fn with_panel(mut self, panel: DashboardPanel) -> Self {
        self.panels.push(panel);
        self
    }

    /// 设置刷新间隔
    pub fn with_refresh_interval(mut self, interval: Duration) -> Self {
        self.refresh_interval = interval;
        self
    }

    /// 校验配置（spec §6.26 规则 2）
    pub fn validate(&self) -> Result<(), crate::error::MonitorPanelError> {
        if self.id.is_empty() {
            return Err(crate::error::MonitorPanelError::Internal(
                "仪表盘 ID 不能为空".into(),
            ));
        }
        if self.refresh_interval == Duration::ZERO {
            return Err(crate::error::MonitorPanelError::Internal(
                "刷新间隔不能为零".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chart_type_as_str() {
        assert_eq!(ChartType::Line.as_str(), "line");
        assert_eq!(ChartType::Bar.as_str(), "bar");
        assert_eq!(ChartType::Pie.as_str(), "pie");
    }

    #[test]
    fn test_dashboard_panel() {
        let panel = DashboardPanel::new("p1", "CPU 使用率", ChartType::Line, "rate(cpu_usage[5m])")
            .with_position(0, 0)
            .with_size(12, 6);
        assert_eq!(panel.id, "p1");
        assert_eq!(panel.position, (0, 0));
        assert_eq!(panel.size, (12, 6));
    }

    #[test]
    fn test_dashboard() {
        let dashboard = Dashboard::new("d1", "服务器监控")
            .with_panel(DashboardPanel::new("p1", "CPU", ChartType::Line, "cpu"))
            .with_panel(DashboardPanel::new(
                "p2",
                "内存",
                ChartType::Gauge,
                "memory",
            ))
            .with_refresh_interval(Duration::from_secs(10));
        assert_eq!(dashboard.id, "d1");
        assert_eq!(dashboard.panels.len(), 2);
        assert_eq!(dashboard.refresh_interval, Duration::from_secs(10));
    }

    #[test]
    fn test_dashboard_validate() {
        let dashboard = Dashboard::new("d1", "监控");
        assert!(dashboard.validate().is_ok());
    }

    #[test]
    fn test_dashboard_validate_empty_id() {
        let dashboard = Dashboard::new("", "监控");
        assert!(dashboard.validate().is_err());
    }
}
