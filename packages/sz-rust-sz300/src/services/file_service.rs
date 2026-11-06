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

    // ─────────────────────────────────────────────
    // FileService::delete 错误路径（不实际删除文件，覆盖 canonicalize 失败路径）
    // �;────────────────────────────────────────────

    /// delete 对不存在的文件应返回 Err（文件不存在或上传目录不存在）
    #[tokio::test]
    async fn delete_nonexistent_file_returns_err() {
        let result = FileService::delete("/uploads/2099/01/01/nonexistent_test_file.jpg").await;
        assert!(result.is_err(), "删除不存在的文件应返回 Err");
        let msg = result.unwrap_err();
        // 具体消息依赖 ./uploads 是否存在：不存在→"文件删除失败"，存在→"文件不存在"
        assert!(
            msg.contains("文件不存在") || msg.contains("文件删除失败"),
            "应返回文件不存在或文件删除失败, 实际: {msg}"
        );
    }

    /// delete 对路径遍历输入应返回 Err（防护触发或文件不存在）
    #[tokio::test]
    async fn delete_traversal_path_returns_err() {
        let result = FileService::delete("/uploads/../../etc/passwd").await;
        assert!(result.is_err(), "路径遍历应返回 Err");
    }

    /// delete 对无 /uploads/ 前缀的 URL 应返回 Err（strip_prefix 保持原样，canonicalize 失败）
    #[tokio::test]
    async fn delete_no_uploads_prefix_returns_err() {
        let result = FileService::delete("/etc/passwd").await;
        assert!(result.is_err(), "无 /uploads/ 前缀应返回 Err");
    }

    /// delete 对空 URL 应返回 Err
    #[tokio::test]
    async fn delete_empty_url_returns_err() {
        let result = FileService::delete("").await;
        assert!(result.is_err(), "空 URL 应返回 Err");
    }

    // ─────────────────────────────────────────────
    // FileService::save 成功路径（用 tempdir 隔离，避免污染工作目录）
    // 共享 CWD_MUTEX 保证 save 测试串行执行（set_current_dir 是进程全局状态）
    // ─────────────────────────-────────────────────

    use std::sync::Mutex;
    static CWD_MUTEX: Mutex<()> = Mutex::new(());

    /// save 对合法 jpg 文件应成功，返回 /uploads/日期/文件名 路径
    #[tokio::test]
    async fn save_valid_jpg_returns_ok_url() {
        let _guard = CWD_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let temp = std::env::temp_dir().join(format!("sz300_file_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).expect("创建 tempdir 失败");
        let orig_cwd = std::env::current_dir().expect("获取 cwd 失败");
        std::env::set_current_dir(&temp).expect("切换 cwd 失败");

        let data = b"fake-jpg-content";
        let result = FileService::save("test.jpg", data).await;

        // 恢复 cwd
        std::env::set_current_dir(&orig_cwd).expect("恢复 cwd 失败");
        // cleanup
        let _ = std::fs::remove_dir_all(&temp);

        assert!(result.is_ok(), "save 合法 jpg 应成功: {:?}", result);
        let url = result.unwrap();
        assert!(
            url.starts_with("/uploads/"),
            "返回 URL 应以 /uploads/ 开头, 实际: {url}"
        );
        assert!(
            url.ends_with(".jpg"),
            "返回 URL 应以 .jpg 结尾, 实际: {url}"
        );
    }

    /// save 对大写扩展名 .PNG 应成功（ext.to_lowercase() 归一化）
    #[tokio::test]
    async fn save_uppercase_extension_returns_ok() {
        let _guard = CWD_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let temp = std::env::temp_dir().join(format!("sz300_file_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).expect("创建 tempdir 失败");
        let orig_cwd = std::env::current_dir().expect("获取 cwd 失败");
        std::env::set_current_dir(&temp).expect("切换 cwd 失败");

        let data = b"fake-png-content";
        let result = FileService::save("image.PNG", data).await;

        std::env::set_current_dir(&orig_cwd).expect("恢复 cwd 失败");
        let _ = std::fs::remove_dir_all(&temp);

        assert!(result.is_ok(), "save 大写 .PNG 应成功: {:?}", result);
        let url = result.unwrap();
        assert!(url.ends_with(".PNG"), "应保留原扩展名大小写, 实际: {url}");
    }
}
