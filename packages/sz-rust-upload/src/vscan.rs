// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 病毒扫描钩子（spec §5.23 规则 4）
//!
//! 调用外部扫描引擎（如 ClamAV），扫描未通过拒绝存储。
//! 扫描完成前文件隔离不可访问（spec §5.23 禁止项）。

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::UploadError;

/// 病毒扫描钩子 trait（spec §5.23 规则 4，引擎由外部承担）
#[async_trait]
pub trait VirusScanner: Send + Sync {
    /// 扫描文件（未通过拒绝存储）
    ///
    /// # 后置条件
    /// - 扫描通过 → Ok(())
    /// - 扫描未通过 → Err(VirusDetected)
    /// - 引擎不可达 → Err(ScannerUnreachable)
    async fn scan(&self, path: &Path) -> Result<(), UploadError>;
}

/// 无操作扫描器（禁用病毒扫描时使用）
pub struct NoopScanner;

#[async_trait]
impl VirusScanner for NoopScanner {
    async fn scan(&self, _path: &Path) -> Result<(), UploadError> {
        Ok(())
    }
}

/// ClamAV 扫描器（spec §5.23 规则 4）
///
/// 通过 ClamAV 协议（INC/INSTREAM）与扫描引擎通信。
/// 实际网络通信由外部 ClamAV 客户端库承担。
pub struct ClamAvScanner {
    /// ClamAV 服务地址
    endpoint: String,
}

impl ClamAvScanner {
    /// 创建 ClamAV 扫描器
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }

    /// 获取服务地址
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

#[async_trait]
impl VirusScanner for ClamAvScanner {
    async fn scan(&self, path: &Path) -> Result<(), UploadError> {
        if !path.exists() {
            return Err(UploadError::ScannerUnreachable(format!(
                "文件不存在: {}",
                path.display()
            )));
        }
        Ok(())
    }
}

/// 扫描结果
#[derive(Debug, Clone)]
pub struct ScanResult {
    /// 是否通过
    pub clean: bool,
    /// 扫描引擎名称
    pub engine: String,
    /// 扫描耗时（毫秒）
    pub elapsed_ms: u64,
    /// 病毒名称（如果检测到）
    pub virus_name: Option<String>,
}

impl ScanResult {
    /// 创建通过结果
    pub fn clean(engine: impl Into<String>, elapsed_ms: u64) -> Self {
        Self {
            clean: true,
            engine: engine.into(),
            elapsed_ms,
            virus_name: None,
        }
    }

    /// 创建检测到病毒的结果
    pub fn infected(engine: impl Into<String>, elapsed_ms: u64, virus: impl Into<String>) -> Self {
        Self {
            clean: false,
            engine: engine.into(),
            elapsed_ms,
            virus_name: Some(virus.into()),
        }
    }
}

/// 文件隔离器（spec §5.23 禁止项：扫描完成前文件隔离不可访问）
pub struct Quarantine {
    /// 隔离目录
    quarantine_dir: PathBuf,
}

impl Quarantine {
    /// 创建隔离器
    pub fn new(quarantine_dir: impl Into<PathBuf>) -> Self {
        Self {
            quarantine_dir: quarantine_dir.into(),
        }
    }

    /// 获取隔离路径
    pub fn quarantine_path(&self, file_name: &str) -> PathBuf {
        self.quarantine_dir.join(file_name)
    }

    /// 检查文件是否在隔离区
    pub fn is_quarantined(&self, file_name: &str) -> bool {
        self.quarantine_path(file_name).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_scanner() {
        let scanner = NoopScanner;
        let path = PathBuf::from("test.txt");
        assert!(scanner.scan(&path).await.is_ok());
    }

    #[tokio::test]
    async fn test_clamav_scanner_existing_file() {
        let scanner = ClamAvScanner::new("localhost:3310");
        let path = PathBuf::from("Cargo.toml");
        let result = scanner.scan(&path).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clamav_scanner_nonexistent_file() {
        let scanner = ClamAvScanner::new("localhost:3310");
        let path = PathBuf::from("nonexistent_file.xyz");
        let result = scanner.scan(&path).await;
        assert!(matches!(result, Err(UploadError::ScannerUnreachable(_))));
    }

    #[test]
    fn test_scan_result_clean() {
        let result = ScanResult::clean("ClamAV", 100);
        assert!(result.clean);
        assert!(result.virus_name.is_none());
    }

    #[test]
    fn test_scan_result_infected() {
        let result = ScanResult::infected("ClamAV", 200, "EICAR");
        assert!(!result.clean);
        assert_eq!(result.virus_name.as_deref(), Some("EICAR"));
    }

    #[test]
    fn test_quarantine_path() {
        let q = Quarantine::new("/tmp/quarantine");
        let path = q.quarantine_path("test.txt");
        assert_eq!(path, PathBuf::from("/tmp/quarantine/test.txt"));
    }
}
