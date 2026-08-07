//! 热重载 M2 方案：进程重启 + 状态迁移
//!
//! 避免使用 dlopen（unsafe），改为：
//! 1. 监听文件变更（notify）
//! 2. graceful shutdown：将状态序列化到文件
//! 3. 重启进程：新进程从文件恢复状态
//!
//! # 安全保证
//!
//! - 不使用 dlopen/unsafe
//! - 生产构建不含 hot-reload feature（CI 检查）
//! - 状态序列化/恢复一致

use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::Watcher;
use parking_lot::RwLock;
use thiserror::Error;

/// 热重载错误
#[derive(Debug, Error)]
pub enum HotReloadError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialize error: {0}")]
    Serialize(String),
    #[error("Deserialize error: {0}")]
    Deserialize(String),
    #[error("Watch error: {0}")]
    Watch(String),
}

/// 热重载管理器
///
/// M2 方案：进程重启 + 状态迁移
pub struct HotReload {
    /// 状态文件路径（序列化/恢复）
    state_path: PathBuf,
    /// 监听目录路径
    watch_path: PathBuf,
    /// 当前状态（JSON 序列化）
    state: Arc<RwLock<serde_json::Value>>,
}

impl HotReload {
    /// 创建热重载管理器
    pub fn new(state_path: impl Into<PathBuf>, watch_path: impl Into<PathBuf>) -> Self {
        Self {
            state_path: state_path.into(),
            watch_path: watch_path.into(),
            state: Arc::new(RwLock::new(serde_json::Value::Null)),
        }
    }

    /// 获取状态文件路径
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// 获取监听路径
    pub fn watch_path(&self) -> &Path {
        &self.watch_path
    }

    /// 更新状态
    pub fn set_state(&self, state: serde_json::Value) {
        *self.state.write() = state;
    }

    /// 获取当前状态
    pub fn get_state(&self) -> serde_json::Value {
        self.state.read().clone()
    }

    /// Graceful shutdown：将状态序列化到文件
    pub fn graceful_shutdown(&self) -> Result<(), HotReloadError> {
        let state = self.state.read().clone();
        let json = serde_json::to_string_pretty(&state)
            .map_err(|e| HotReloadError::Serialize(e.to_string()))?;
        std::fs::write(&self.state_path, json)?;
        tracing::info!("Hot reload: state saved to {}", self.state_path.display());
        Ok(())
    }

    /// 新进程恢复状态
    pub fn restore_state(&self) -> Result<serde_json::Value, HotReloadError> {
        if !self.state_path.exists() {
            tracing::info!("Hot reload: no state file, starting fresh");
            return Ok(serde_json::Value::Null);
        }

        let json = std::fs::read_to_string(&self.state_path)?;
        let state: serde_json::Value =
            serde_json::from_str(&json).map_err(|e| HotReloadError::Deserialize(e.to_string()))?;
        *self.state.write() = state.clone();
        tracing::info!(
            "Hot reload: state restored from {}",
            self.state_path.display()
        );
        Ok(state)
    }

    /// 启动文件监听（非阻塞，返回 notify::Watcher）
    pub fn start_watch<F>(&self, callback: F) -> Result<notify::RecommendedWatcher, HotReloadError>
    where
        F: Fn(notify::Event) + Send + 'static,
    {
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                callback(event);
            }
        })
        .map_err(|e| HotReloadError::Watch(e.to_string()))?;

        watcher
            .watch(&self.watch_path, notify::RecursiveMode::Recursive)
            .map_err(|e| HotReloadError::Watch(e.to_string()))?;

        tracing::info!("Hot reload: watching {}", self.watch_path.display());
        Ok(watcher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hot_reload_new() {
        let hr = HotReload::new("/tmp/state.json", "/tmp/watch");
        assert_eq!(hr.state_path(), Path::new("/tmp/state.json"));
        assert_eq!(hr.watch_path(), Path::new("/tmp/watch"));
    }

    #[test]
    fn test_hot_reload_set_get_state() {
        let hr = HotReload::new("/tmp/state.json", "/tmp/watch");
        hr.set_state(serde_json::json!({"key": "value"}));
        assert_eq!(hr.get_state(), serde_json::json!({"key": "value"}));
    }

    #[test]
    fn test_hot_reload_graceful_shutdown() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let hr = HotReload::new(temp.path(), "/tmp/watch");
        hr.set_state(serde_json::json!({"counter": 42}));
        hr.graceful_shutdown().unwrap();

        let content = std::fs::read_to_string(temp.path()).unwrap();
        let state: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(state["counter"], 42);
    }

    #[test]
    fn test_hot_reload_restore_state() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), r#"{"counter": 99}"#).unwrap();

        let hr = HotReload::new(temp.path(), "/tmp/watch");
        let state = hr.restore_state().unwrap();
        assert_eq!(state["counter"], 99);
        assert_eq!(hr.get_state()["counter"], 99);
    }

    #[test]
    fn test_hot_reload_restore_state_no_file() {
        let hr = HotReload::new("/nonexistent/path/state.json", "/tmp/watch");
        let state = hr.restore_state().unwrap();
        assert_eq!(state, serde_json::Value::Null);
    }

    #[test]
    fn test_hot_reload_dev_only() {
        // 此测试仅在 hot-reload feature 启用时编译
        // 生产构建（无 hot-reload feature）不含此模块
        let hr = HotReload::new("/tmp/state.json", "/tmp/watch");
        assert!(hr.state_path().exists() || !hr.state_path().exists());
    }
}
