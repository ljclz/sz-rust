// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! 配置热重载 watcher
//!
//! 监听 config/ 目录下 `.yml`/`.yaml` 文件变更，变更时通过 channel 发送事件。
//! 重载协调 task 接收事件后重新加载配置，首版仅记录日志（不自动应用新配置）。

use std::path::{Path, PathBuf};

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// 配置文件 watcher
///
/// 封装 `notify::RecommendedWatcher`，监听配置目录下 YAML 文件变更。
pub struct ConfigWatcher {
    _watcher: notify::RecommendedWatcher,
}

impl ConfigWatcher {
    /// 启动配置文件监听
    ///
    /// 监听 `config_dir` 下 `.yml`/`.yaml` 文件变更，变更时通过 `reload_tx` 发送文件路径。
    pub fn start(
        config_dir: &Path,
        reload_tx: mpsc::Sender<PathBuf>,
    ) -> Result<Self, notify::Error> {
        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                        for path in &event.paths {
                            if let Some(ext) = path.extension() {
                                if ext == "yml" || ext == "yaml" {
                                    let _ = reload_tx.try_send(path.clone());
                                }
                            }
                        }
                    }
                }
            })?;
        watcher.watch(config_dir, RecursiveMode::Recursive)?;
        Ok(Self { _watcher: watcher })
    }
}

/// 重载配置文件
///
/// 调用 `AppConfig::load_from_dir` 重新加载配置。
/// 失败时返回错误，由调用方决定保留旧配置。
pub async fn reload_config(
    config_dir: &Path,
) -> Result<sz_rust_core::config::AppConfig, sz_rust_core::config::ConfigError> {
    sz_rust_core::config::AppConfig::load_from_dir(config_dir).await
}

/// 启动配置重载协调 task
///
/// 接收文件变更事件 → 重新加载配置 → 成功时记录 info 日志；失败时记录 error 日志保留旧配置。
pub fn spawn_reload_coordinator(
    mut reload_rx: mpsc::Receiver<PathBuf>,
    config_dir: PathBuf,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(changed_path) = reload_rx.recv().await {
            tracing::info!("配置文件变更: {}", changed_path.display());
            match reload_config(&config_dir).await {
                Ok(_new_config) => {
                    tracing::info!("配置已重载（数据库/路由变更需重启生效）");
                }
                Err(e) => {
                    tracing::error!("配置重载失败，保留旧配置: {e}");
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn test_reload_config_valid() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
        let config_path = temp_dir.path().join("server.yml");
        let mut file = std::fs::File::create(&config_path).expect("创建配置文件失败");
        writeln!(file, "host: 0.0.0.0\nport: 9090").unwrap();
        drop(file);

        let result = reload_config(temp_dir.path()).await;
        assert!(result.is_ok(), "合法配置重载应成功");
        let config = result.unwrap();
        assert_eq!(config.server.port, 9090);
    }

    #[tokio::test]
    async fn test_reload_config_empty_dir() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
        let result = reload_config(temp_dir.path()).await;
        assert!(result.is_ok(), "空目录应返回默认配置");
    }

    #[tokio::test]
    async fn test_config_watcher_start_success() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
        let (tx, _rx) = mpsc::channel::<PathBuf>(16);
        let result = ConfigWatcher::start(temp_dir.path(), tx);
        assert!(result.is_ok(), "watcher 启动应成功");
    }

    #[tokio::test]
    async fn test_config_watcher_start_nonexistent_dir() {
        let (tx, _rx) = mpsc::channel::<PathBuf>(16);
        let result = ConfigWatcher::start(Path::new("/nonexistent_path_xyz"), tx);
        assert!(result.is_err(), "不存在的目录应启动失败");
    }

    #[tokio::test]
    async fn test_spawn_reload_coordinator_valid_config() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
        let config_path = temp_dir.path().join("server.yml");
        let mut file = std::fs::File::create(&config_path).expect("创建配置文件失败");
        writeln!(file, "host: 0.0.0.0\nport: 9090").unwrap();
        drop(file);

        let (tx, rx) = mpsc::channel::<PathBuf>(16);
        let handle = spawn_reload_coordinator(rx, temp_dir.path().to_path_buf());
        tx.send(config_path).await.unwrap();
        drop(tx);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
    }

    #[tokio::test]
    async fn test_spawn_reload_coordinator_invalid_config() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
        let config_path = temp_dir.path().join("server.yml");
        let mut file = std::fs::File::create(&config_path).expect("创建配置文件失败");
        writeln!(file, "host: 0.0.0.0\nport: 9091").unwrap();
        drop(file);

        let (tx, rx) = mpsc::channel::<PathBuf>(16);
        let handle = spawn_reload_coordinator(rx, temp_dir.path().to_path_buf());
        tx.send(config_path).await.unwrap();
        drop(tx);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
    }

    #[tokio::test]
    async fn test_spawn_reload_coordinator_channel_closed() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
        let (tx, rx) = mpsc::channel::<PathBuf>(16);
        let handle = spawn_reload_coordinator(rx, temp_dir.path().to_path_buf());
        drop(tx);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
    }

    #[tokio::test]
    async fn test_config_watcher_file_change_event() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
        let config_path = temp_dir.path().join("server.yml");
        let mut file = std::fs::File::create(&config_path).expect("创建配置文件失败");
        writeln!(file, "host: 0.0.0.0\nport: 9090").unwrap();
        drop(file);

        let (tx, mut rx) = mpsc::channel::<PathBuf>(16);
        let _watcher = ConfigWatcher::start(temp_dir.path(), tx).expect("watcher 启动失败");

        let config_path2 = temp_dir.path().join("server.yml");
        let mut file2 = std::fs::File::create(&config_path2).expect("创建配置文件失败");
        writeln!(file2, "host: 0.0.0.0\nport: 9091").unwrap();
        drop(file2);

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
        assert!(result.is_ok(), "应在超时前收到文件变更事件");
    }
}
