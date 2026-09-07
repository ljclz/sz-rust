// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 应用预览服务
//!
//! 扫描产物目录 → 启动临时 HTTP 服务 → WebView 窗口打开预览。

use std::path::PathBuf;

use tower_http::services::ServeDir;

use crate::error::{VisualError, VisualResult};
use crate::models::DeviceType;

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

        tracing::info!("预览服务启动: feature={feature}, port={port}, viewport={width}x{height}");

        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Ok(format!("http://127.0.0.1:{port}"))
    }

    /// 停止预览
    ///
    /// 前端关闭 WebView 窗口即可，HTTP 服务随 task 结束自动释放。
    pub async fn stop(_url: &str) -> VisualResult<()> {
        Ok(())
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
    async fn test_preview_stop() {
        let result = PreviewService::stop("http://127.0.0.1:9999").await;
        assert!(result.is_ok());
    }
}
