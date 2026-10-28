// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! 运行时信号处理
//!
//! Unix 平台：SIGUSR1/SIGHUP 触发配置重载，SIGUSR2 触发日志级别切换。
//! Windows 平台：no-op（仅 Ctrl+C 优雅关闭，由 axum graceful shutdown 处理）。

use tokio::sync::mpsc;

/// 日志级别循环切换
///
/// 在 `info` ↔ `debug` 间循环切换。
pub fn log_level_cycle(current: tracing::Level) -> tracing::Level {
    match current {
        tracing::Level::INFO => tracing::Level::DEBUG,
        _ => tracing::Level::INFO,
    }
}

/// 安装运行时信号处理
///
/// Unix 平台注册 SIGUSR1/SIGHUP（配置重载）+ SIGUSR2（日志级别切换）。
/// Windows 平台为 no-op。
///
/// 返回后台 task 的 JoinHandle，调用方可选择性 await。
pub fn install_runtime_signals(
    reload_tx: mpsc::Sender<()>,
    loglevel_tx: mpsc::Sender<()>,
) -> tokio::task::JoinHandle<()> {
    spawn_signal_handler(reload_tx, loglevel_tx)
}

#[cfg(unix)]
fn spawn_signal_handler(
    reload_tx: mpsc::Sender<()>,
    loglevel_tx: mpsc::Sender<()>,
) -> tokio::task::JoinHandle<()> {
    use tokio::signal::unix::{signal, SignalKind};

    tokio::spawn(async move {
        let mut sigusr1 = match signal(SignalKind::user_defined1()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("SIGUSR1 注册失败: {e}");
                return;
            }
        };
        let mut sighup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("SIGHUP 注册失败: {e}");
                return;
            }
        };
        let mut sigusr2 = match signal(SignalKind::user_defined2()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("SIGUSR2 注册失败: {e}");
                return;
            }
        };

        loop {
            tokio::select! {
                _ = sigusr1.recv() => {
                    tracing::info!("收到 SIGUSR1，触发配置重载");
                    let _ = reload_tx.try_send(());
                }
                _ = sighup.recv() => {
                    tracing::info!("收到 SIGHUP，触发配置重载");
                    let _ = reload_tx.try_send(());
                }
                _ = sigusr2.recv() => {
                    tracing::info!("收到 SIGUSR2，触发日志级别切换");
                    let _ = loglevel_tx.try_send(());
                }
            }
        }
    })
}

#[cfg(not(unix))]
fn spawn_signal_handler(
    _reload_tx: mpsc::Sender<()>,
    _loglevel_tx: mpsc::Sender<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_cycle_info_to_debug() {
        assert_eq!(log_level_cycle(tracing::Level::INFO), tracing::Level::DEBUG);
    }

    #[test]
    fn test_log_level_cycle_debug_to_info() {
        assert_eq!(log_level_cycle(tracing::Level::DEBUG), tracing::Level::INFO);
    }

    #[test]
    fn test_log_level_cycle_warn_to_info() {
        assert_eq!(log_level_cycle(tracing::Level::WARN), tracing::Level::INFO);
    }

    #[test]
    fn test_log_level_cycle_error_to_info() {
        assert_eq!(log_level_cycle(tracing::Level::ERROR), tracing::Level::INFO);
    }

    #[tokio::test]
    async fn test_install_runtime_signals_returns_handle() {
        let (reload_tx, _reload_rx) = mpsc::channel::<()>(1);
        let (loglevel_tx, _loglevel_rx) = mpsc::channel::<()>(1);
        let handle = install_runtime_signals(reload_tx, loglevel_tx);
        assert!(!handle.is_finished());
    }
}
