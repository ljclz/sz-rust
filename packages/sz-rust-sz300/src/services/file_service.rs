// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing;
use uuid::Uuid;

const UPLOAD_DIR: &str = "./uploads";
const ALLOWED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp"];
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5MB

/// 文件服务（对齐 PHP FileService） — 负责上传文件的保存、校验与删除
pub struct FileService;

impl FileService {
    /// 初始化上传目录
    pub async fn init() -> std::io::Result<()> {
        if !Path::new(UPLOAD_DIR).exists() {
            fs::create_dir_all(UPLOAD_DIR).await?;
            tracing::info!("上传目录已创建: {}", UPLOAD_DIR);
        }
        Ok(())
    }

    /// 保存上传文件
    pub async fn save(filename: &str, data: &[u8]) -> Result<String, String> {
        // 检查文件大小
        if data.len() as u64 > MAX_FILE_SIZE {
            return Err(format!(
                "文件大小超过限制 ({}MB)",
                MAX_FILE_SIZE / 1024 / 1024
            ));
        }

        // 检查文件扩展名
        let ext = Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        if !ALLOWED_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
            return Err(format!("不支持的文件格式: .{}", ext));
        }

        // 生成唯一文件名
        let new_filename = format!(
            "{}_{}.{}",
            chrono::Local::now().format("%Y%m%d_%H%M%S"),
            &Uuid::new_v4().to_string()[..8],
            ext
        );

        // 创建日期子目录
        let date_dir = chrono::Local::now().format("%Y/%m/%d").to_string();
        let dir_path = PathBuf::from(UPLOAD_DIR).join(&date_dir);
        if !dir_path.exists() {
            fs::create_dir_all(&dir_path).await.map_err(|e| {
                tracing::error!(error = %e, "文件保存：创建目录失败");
                "文件保存失败".to_string()
            })?;
        }

        // 写入文件
        let file_path = dir_path.join(&new_filename);
        fs::write(&file_path, data).await.map_err(|e| {
            tracing::error!(error = %e, "文件保存：写入文件失败");
            "文件保存失败".to_string()
        })?;

        tracing::info!("文件已保存: /uploads/{}/{}", date_dir, new_filename);

        // 返回访问路径
        Ok(format!("/uploads/{}/{}", date_dir, new_filename))
    }

    /// 删除文件 — 含路径遍历防护
    pub async fn delete(url: &str) -> Result<(), String> {
        let relative_path = url.strip_prefix("/uploads/").unwrap_or(url);

        // 路径遍历防护：canonicalize 后校验前缀
        let root = PathBuf::from(UPLOAD_DIR).canonicalize().map_err(|e| {
            tracing::error!(error = %e, "文件删除：上传目录不存在");
            "文件删除失败".to_string()
        })?;
        let file_path = root.join(relative_path);

        // 校验解析后的路径仍在上传目录内
        let canonical = file_path
            .canonicalize()
            .map_err(|_| "文件不存在".to_string())?;
        if !canonical.starts_with(&root) {
            return Err("非法路径: 不允许访问上传目录外的文件".to_string());
        }

        if canonical.exists() {
            fs::remove_file(&canonical).await.map_err(|e| {
                tracing::error!(error = %e, "文件删除：删除文件失败");
                "文件删除失败".to_string()
            })?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────
    // FileService::save 错误路径纯逻辑（不实际写文件，P2-4 补 service 层覆盖率）
    // ─────────────────────────────────────────────

    /// 文件大小超过 MAX_FILE_SIZE（5MB）应直接返回 Err，不进入目录创建/写文件
    #[tokio::test]
    async fn save_rejects_oversized_file_without_writing() {
        let oversized = vec![0u8; (MAX_FILE_SIZE + 1) as usize];
        let result = FileService::save("test.jpg", &oversized).await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("文件大小超过限制"),
            "应返回大小超限错误，实际: {msg}"
        );
    }

    /// 不支持的扩展名应直接返回 Err，不进入目录创建/写文件
    #[tokio::test]
    async fn save_rejects_unsupported_extension_without_writing() {
        let data = b"hello";
        let result = FileService::save("malware.exe", data).await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("不支持的文件格式"),
            "应返回格式错误，实际: {msg}"
        );
        assert!(msg.contains(".exe"), "错误消息应包含扩展名: {msg}");
    }

    /// 无扩展名应返回格式错误
    #[tokio::test]
    async fn save_rejects_no_extension_without_writing() {
        let data = b"hello";
        let result = FileService::save("noext", data).await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("不支持的文件格式"), "无扩展名应被拒绝: {msg}");
    }

    /// 大小写扩展名应被接受（ext.to_lowercase() 归一化）
    /// 此处仅校验白名单逻辑，不实际写文件——通过传入空数据触发后续路径，
    /// 但空数据大小不超限、扩展名合法，会进入写文件路径。为避免写真实文件，
    /// 此用例仅断言 ALLOWED_EXTENSIONS 含大小写归一化后的扩展名。
    #[test]
    fn save_allowed_extensions_case_insensitive() {
        for ext in ["jpg", "JPG", "Jpg", "jpeg", "JPEG", "png", "PNG", "gif", "GIF", "bmp", "BMP"]
        {
            let lower = ext.to_lowercase();
            assert!(
                ALLOWED_EXTENSIONS.contains(&lower.as_str()),
                "扩展名 {ext} 归一化后应在白名单中"
            );
        }
    }

    /// MAX_FILE_SIZE 应为 5MB（锁定常量，防止误改）
    #[test]
    fn max_file_size_is_5mb() {
        assert_eq!(MAX_FILE_SIZE, 5 * 1024 * 1024);
    }

    /// UPLOAD_DIR 应为 ./uploads（锁定常量）
    #[test]
    fn upload_dir_constant() {
        assert_eq!(UPLOAD_DIR, "./uploads");
    }

    /// delete 的路径遍历防护：strip_prefix("/uploads/") 后的相对路径
    /// 此处锁定 strip_prefix 语义，不实际访问文件系统。
    #[test]
    fn delete_strips_uploads_prefix() {
        let url = "/uploads/2026/01/01/abc.jpg";
        let relative = url.strip_prefix("/uploads/").unwrap_or(url);
        assert_eq!(relative, "2026/01/01/abc.jpg");
    }

    #[test]
    fn delete_keeps_url_when_no_uploads_prefix() {
        let url = "/etc/passwd";
        let relative = url.strip_prefix("/uploads/").unwrap_or(url);
        assert_eq!(relative, "/etc/passwd", "无 /uploads/ 前缀时保持原样");
    }
}
