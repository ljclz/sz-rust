// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 心跳保活（spec §5.21 规则 2，§6.21 规则 1-2）
//!
//! 定期发送 ping → 空闲超时连接自动关闭。

use std::time::{Duration, Instant};

use tokio::time::interval;

use crate::manager::{ConnectionId, ConnectionManager};

/// 心跳检测器（spec §5.21 规则 2）
pub struct Heartbeat;

impl Heartbeat {
    /// 执行一次心跳检测（spec §5.21 规则 2）
    ///
    /// 检查所有连接的活跃时间，空闲超时的连接返回其 ID。
    pub fn check_once(mgr: &ConnectionManager) -> Vec<ConnectionId> {
        mgr.idle_connections()
    }

    /// 心跳循环（spec §5.21 规则 2，§6.21 规则 1）
    ///
    /// 定期检查空闲连接，超时连接自动关闭。
    /// 调用者应在 tokio task 中运行此循环。
    pub async fn run_loop<F>(mgr: &ConnectionManager, on_timeout: F)
    where
        F: Fn(ConnectionId),
    {
        let mut ticker = interval(mgr.config().heartbeat_interval);
        loop {
            ticker.tick().await;
            let idle = Self::check_once(mgr);
            for conn_id in idle {
                on_timeout(conn_id);
            }
        }
    }

    /// 计算下次心跳时间
    pub fn next_heartbeat(last: Instant, interval: Duration) -> Instant {
        last + interval
    }

    /// 检查是否空闲超时（spec §6.21 规则 2）
    pub fn is_idle_timeout(last_active: Instant, timeout: Duration) -> bool {
        Instant::now().duration_since(last_active) > timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_check_once_no_idle() {
        let mgr = ConnectionManager::with_defaults();
        let (tx, _rx) = mpsc::channel(10);
        mgr.register(tx, None).unwrap();
        let idle = Heartbeat::check_once(&mgr);
        assert!(idle.is_empty());
    }

    #[tokio::test]
    async fn test_check_once_with_idle() {
        let config = crate::manager::WebSocketConfig {
            idle_timeout: Duration::from_millis(1),
            ..Default::default()
        };
        let mgr = ConnectionManager::new(config);
        let (tx, _rx) = mpsc::channel(10);
        let conn_id = mgr.register(tx, None).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        let idle = Heartbeat::check_once(&mgr);
        assert!(idle.contains(&conn_id));
    }

    #[test]
    fn test_is_idle_timeout() {
        let past = Instant::now() - Duration::from_secs(10);
        assert!(Heartbeat::is_idle_timeout(past, Duration::from_secs(5)));
        assert!(!Heartbeat::is_idle_timeout(
            Instant::now(),
            Duration::from_secs(5)
        ));
    }

    #[test]
    fn test_next_heartbeat() {
        let now = Instant::now();
        let next = Heartbeat::next_heartbeat(now, Duration::from_secs(30));
        assert!(next > now);
    }

    #[tokio::test]
    async fn test_run_loop_calls_on_timeout() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let config = crate::manager::WebSocketConfig {
            idle_timeout: Duration::from_millis(1),
            heartbeat_interval: Duration::from_millis(5),
            ..Default::default()
        };
        let mgr = ConnectionManager::new(config);
        let (tx, _rx) = mpsc::channel(10);
        let conn_id = mgr.register(tx, None).unwrap();

        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();
        let conn_id_clone = conn_id.clone();

        let handle = tokio::spawn(async move {
            Heartbeat::run_loop(&mgr, move |id| {
                if id == conn_id_clone {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        assert!(count.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn test_run_loop_no_idle_no_callback() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let mgr = ConnectionManager::with_defaults();
        let (tx, _rx) = mpsc::channel(10);
        mgr.register(tx, None).unwrap();

        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();

        let handle = tokio::spawn(async move {
            Heartbeat::run_loop(&mgr, move |_| {
                count_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
}
