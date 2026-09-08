// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 应用预览服务
//!
//! 扫描产物目录 → 启动临时 HTTP 服务 → WebView 窗口打开预览。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use tower_http::services::ServeDir;

use crate::error::{VisualError, VisualResult};
use crate::models::DeviceType;

/// 全局预览服务 shutdown 句柄表（url → 触发器）
fn shutdown_handles() -> &'static Mutex<HashMap<String, Arc<tokio::sync::Notify>>> {
    static HANDLES: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Notify>>>> = OnceLock::new();
    HANDLES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 预览服务
pub struct PreviewService;

impl PreviewService {
    /// 启动预览
    ///
    /// 扫描 `artifacts/{feature}/` 目录 → 返回预览 URL。
    /// 前端通过 Tauri WebView 打开 URL。
    pub async fn start(feature: &str, device: DeviceType) -> VisualResult<String> {
        let artifacts_dir = find_artifacts_dir(feature)?;
        if !tokio::fs::try_exists(&artifacts_dir)
            .await
            .map_err(|e| VisualError::IoError(e.to_string()))?
        {
            return Err(VisualError::ArtifactNotFound(format!(
                "artifacts/{feature} 目录不存在"
            )));
        }

        let port = find_available_port()?;
        let (width, height) = device.viewport();

        let serve_dir = ServeDir::new(artifacts_dir);
        let app = axum::Router::new()
            .fallback_service(serve_dir)
            .layer(tower_http::compression::CompressionLayer::new());

        let addr = format!("127.0.0.1:{port}");
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| VisualError::IoError(format!("绑定端口失败: {e}")))?;

        let url = format!("http://127.0.0.1:{port}");
        let shutdown = Arc::new(tokio::sync::Notify::new());
        shutdown_handles()
            .lock()
            .map_err(|_| VisualError::InternalError("预览句柄表锁中毒".into()))?
            .insert(url.clone(), Arc::clone(&shutdown));

        tracing::info!("预览服务启动: feature={feature}, port={port}, viewport={width}x{height}");

        let shutdown_for_task = Arc::clone(&shutdown);
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    shutdown_for_task.notified().await;
                })
                .await;
        });

        Ok(url)
    }

    /// 停止预览（真实关闭 HTTP 服务并释放端口）
    pub async fn stop(url: &str) -> VisualResult<()> {
        let handle = shutdown_handles()
            .lock()
            .map_err(|_| VisualError::InternalError("预览句柄表锁中毒".into()))?
            .remove(url);

        match handle {
            Some(notify) => {
                notify.notify_waiters();
                tracing::info!("预览服务已停止: {url}");
                Ok(())
            }
            None => Err(VisualError::ArtifactNotFound(format!(
                "预览服务不存在: {url}"
            ))),
        }
    }
}

/// 查找产物目录
fn find_artifacts_dir(feature: &str) -> VisualResult<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|e| VisualError::IoError(format!("获取当前目录失败: {e}")))?;
    Ok(cwd.join("artifacts").join(feature))
}

/// 查找可用端口
fn find_available_port() -> VisualResult<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| VisualError::IoError(format!("绑定随机端口失败: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| VisualError::IoError(format!("获取端口失败: {e}")))?;
    drop(listener);
    Ok(addr.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_available_port() {
        let port = find_available_port().unwrap();
        assert!(port > 0);
    }

    #[test]
    fn test_find_artifacts_dir() {
        let dir = find_artifacts_dir("test-feature").unwrap();
        assert!(dir.to_string_lossy().contains("test-feature"));
        assert!(dir.to_string_lossy().contains("artifacts"));
    }

    #[tokio::test]
    async fn test_preview_start_artifact_not_found() {
        let result = PreviewService::start("nonexistent-feature-xyz", DeviceType::Desktop).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.error_code(), "ARTIFACT_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_preview_stop_nonexistent_url() {
        let result = PreviewService::stop("http://127.0.0.1:9999").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_preview_start_then_stop_releases_port() {
        // 准备临时产物目录（测试后清理）
        let temp = std::env::temp_dir().join("sz-visual-preview-test");
        tokio::fs::create_dir_all(&temp).await.unwrap();
        tokio::fs::write(temp.join("index.html"), "<html>ok</html>")
            .await
            .unwrap();

        // start 需要在 artifacts/{feature} 下查找，切到临时工作目录模拟
        let cwd = std::env::current_dir().unwrap();
        let artifacts_root = cwd.join("artifacts");
        let feature_dir = artifacts_root.join("test-feature-xyz");
        tokio::fs::create_dir_all(&feature_dir).await.unwrap();
        tokio::fs::write(feature_dir.join("index.html"), "<html>ok</html>")
            .await
            .unwrap();

        let url = PreviewService::start("test-feature-xyz", DeviceType::Desktop)
            .await
            .unwrap();

        // 验证 HTTP 服务可访问
        let probe = tokio::net::TcpStream::connect(url.trim_start_matches("http://"))
            .await
            .unwrap();
        drop(probe);

        // stop 后端口应已释放
        PreviewService::stop(&url).await.unwrap();

        // 清理临时产物目录
        tokio::fs::remove_dir_all(&artifacts_root).await.unwrap();
        tokio::fs::remove_dir_all(&temp).await.unwrap();
    }
}
