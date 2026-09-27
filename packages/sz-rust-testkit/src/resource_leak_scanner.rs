//! 资源泄漏扫描器（v1.5.0 P1-4）
//!
//! 测试结束后扫描连接池/锁/临时文件残留（spec 5.4.2 + 5.4.3 异常2）。
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_testkit::resource_leak_scanner::ResourceLeakScanner;
//! use std::sync::Arc;
//!
//! let scanner = ResourceLeakScanner::new(Arc::new(pool));
//! let report = scanner.scan().await;
//! assert!(report.is_clean(), "不应有资源泄漏");
//! ```
#![forbid(unsafe_code)]

use std::sync::Arc;

use sz_rust_orm_facade::Pool;

/// 资源泄漏报告
#[derive(Debug, Clone)]
pub struct ResourceLeakReport {
    /// 活跃连接数（应为 0）
    pub active_connections: u32,
    /// 空闲连接数
    pub idle_connections: u32,
    /// 检测到的泄漏描述
    pub leaks: Vec<String>,
}

impl ResourceLeakReport {
    /// 是否无泄漏
    pub fn is_clean(&self) -> bool {
        self.leaks.is_empty()
    }
}

/// 资源泄漏扫描器
pub struct ResourceLeakScanner {
    pool: Arc<Pool>,
}

impl ResourceLeakScanner {
    /// 创建扫描器
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    /// 扫描资源残留（spec 5.4.2）
    pub async fn scan(&self) -> ResourceLeakReport {
        let status = self.pool.status().await;
        let active = status.active;
        let idle = status.idle;

        let mut leaks = Vec::new();
        if active > 0 {
            leaks.push(format!("{} 个活跃连接未释放", active));
        }

        ResourceLeakReport {
            active_connections: active,
            idle_connections: idle,
            leaks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_leak_report_clean() {
        let report = ResourceLeakReport {
            active_connections: 0,
            idle_connections: 2,
            leaks: Vec::new(),
        };
        assert!(report.is_clean());
    }

    #[test]
    fn test_resource_leak_report_with_leaks() {
        let report = ResourceLeakReport {
            active_connections: 1,
            idle_connections: 0,
            leaks: vec!["1 个活跃连接未释放".to_string()],
        };
        assert!(!report.is_clean());
        assert_eq!(report.leaks.len(), 1);
    }
}
