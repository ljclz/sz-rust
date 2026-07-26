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
            fs::create_dir_all(&dir_path)
                .await
                .map_err(|e| { tracing::error!(error = %e, "文件保存：创建目录失败"); "文件保存失败".to_string() })?;
        }

        // 写入文件
        let file_path = dir_path.join(&new_filename);
        fs::write(&file_path, data)
            .await
            .map_err(|e| { tracing::error!(error = %e, "文件保存：写入文件失败"); "文件保存失败".to_string() })?;

        tracing::info!("文件已保存: /uploads/{}/{}", date_dir, new_filename);

        // 返回访问路径
        Ok(format!("/uploads/{}/{}", date_dir, new_filename))
    }

    /// 删除文件 — 含路径遍历防护
    pub async fn delete(url: &str) -> Result<(), String> {
        let relative_path = url.strip_prefix("/uploads/").unwrap_or(url);

        // 路径遍历防护：canonicalize 后校验前缀
        let root = PathBuf::from(UPLOAD_DIR)
            .canonicalize()
            .map_err(|e| { tracing::error!(error = %e, "文件删除：上传目录不存在"); "文件删除失败".to_string() })?;
        let file_path = root.join(relative_path);

        // 校验解析后的路径仍在上传目录内
        let canonical = file_path
            .canonicalize()
            .map_err(|_| "文件不存在".to_string())?;
        if !canonical.starts_with(&root) {
            return Err("非法路径: 不允许访问上传目录外的文件".to_string());
        }

        if canonical.exists() {
            fs::remove_file(&canonical)
                .await
                .map_err(|e| { tracing::error!(error = %e, "文件删除：删除文件失败"); "文件删除失败".to_string() })?;
        }
        Ok(())
    }
}
