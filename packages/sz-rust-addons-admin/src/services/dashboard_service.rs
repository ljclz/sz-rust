// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::Arc;

use serde::Serialize;
use sz_rust_orm_facade::{Pool, Value};

use crate::error::AdminError;

#[derive(Debug, Clone, Serialize)]
pub struct DashboardStats {
    pub user_count: i64,
    pub role_count: i64,
    pub permission_count: i64,
    pub menu_count: i64,
    pub today_log_count: i64,
    pub system_status: Option<SystemStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemStatus {
    pub cpu_usage: f32,
    pub memory_usage: f32,
    pub disk_usage: f32,
}

pub struct SystemStatusCollector;

impl SystemStatusCollector {
    pub fn collect() -> Option<SystemStatus> {
        use sysinfo::{Disks, System};

        let mut sys = System::new_all();
        sys.refresh_all();

        let cpu_usage = sys.global_cpu_usage();
        let total_mem = sys.total_memory();
        let used_mem = sys.used_memory();
        let memory_usage = if total_mem > 0 {
            (used_mem as f32 / total_mem as f32) * 100.0
        } else {
            0.0
        };

        let disks = Disks::new_with_refreshed_list();
        let disk_usage = disks
            .list()
            .iter()
            .map(|d| {
                let total = d.total_space();
                if total > 0 {
                    ((total - d.available_space()) as f32 / total as f32) * 100.0
                } else {
                    0.0
                }
            })
            .next()
            .unwrap_or(0.0);

        Some(SystemStatus {
            cpu_usage,
            memory_usage,
            disk_usage,
        })
    }
}

#[derive(Clone)]
pub struct DashboardService {
    pool: Arc<Pool>,
}

impl DashboardService {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    pub async fn stats(&self, tenant_id: i64) -> Result<DashboardStats, AdminError> {
        let mut conn = self.pool.acquire().await?;

        let user_count = conn
            .query_with_params(
                "SELECT COUNT(*) AS cnt FROM users WHERE tenant_id = ?",
                &[Value::I64(tenant_id)],
            )
            .await?
            .first()
            .and_then(|r| r.get("cnt"))
            .and_then(Value::as_i64)
            .unwrap_or(0);

        let role_count = conn
            .query_with_params(
                "SELECT COUNT(*) AS cnt FROM roles WHERE tenant_id = ?",
                &[Value::I64(tenant_id)],
            )
            .await?
            .first()
            .and_then(|r| r.get("cnt"))
            .and_then(Value::as_i64)
            .unwrap_or(0);

        let permission_count = conn
            .query_with_params(
                "SELECT COUNT(*) AS cnt FROM permissions WHERE tenant_id = ? OR tenant_id = 0",
                &[Value::I64(tenant_id)],
            )
            .await?
            .first()
            .and_then(|r| r.get("cnt"))
            .and_then(Value::as_i64)
            .unwrap_or(0);

        let menu_count = conn
            .query_with_params(
                "SELECT COUNT(*) AS cnt FROM menus WHERE tenant_id = ?",
                &[Value::I64(tenant_id)],
            )
            .await?
            .first()
            .and_then(|r| r.get("cnt"))
            .and_then(Value::as_i64)
            .unwrap_or(0);

        let today_log_count = conn
            .query_with_params(
                "SELECT COUNT(*) AS cnt FROM operation_logs WHERE tenant_id = ? AND created_at >= CURRENT_DATE()",
                &[Value::I64(tenant_id)],
            )
            .await?
            .first()
            .and_then(|r| r.get("cnt"))
            .and_then(Value::as_i64)
            .unwrap_or(0);

        let system_status = SystemStatusCollector::collect();

        Ok(DashboardStats {
            user_count,
            role_count,
            permission_count,
            menu_count,
            today_log_count,
            system_status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_stats_has_six_metrics() {
        let stats = DashboardStats {
            user_count: 10,
            role_count: 3,
            permission_count: 17,
            menu_count: 5,
            today_log_count: 42,
            system_status: None,
        };
        assert_eq!(stats.user_count, 10);
        assert_eq!(stats.role_count, 3);
        assert_eq!(stats.permission_count, 17);
        assert_eq!(stats.menu_count, 5);
        assert_eq!(stats.today_log_count, 42);
        assert!(stats.system_status.is_none());
    }

    #[test]
    fn test_system_status_collector_returns_some() {
        let status = SystemStatusCollector::collect();
        assert!(status.is_some());
        let s = status.unwrap();
        assert!(s.cpu_usage >= 0.0);
        assert!(s.memory_usage >= 0.0);
        assert!(s.disk_usage >= 0.0);
    }
}
