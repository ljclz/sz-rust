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
                .map_err(|e| format!("创建目录失败: {}", e))?;
        }

        // 写入文件
        let file_path = dir_path.join(&new_filename);
        fs::write(&file_path, data)
            .await
            .map_err(|e| format!("文件写入失败: {}", e))?;

        tracing::info!("文件已保存: {:?}", file_path);

        // 返回访问路径
        Ok(format!("/uploads/{}/{}", date_dir, new_filename))
    }

    /// 删除文件
    pub async fn delete(url: &str) -> Result<(), String> {
        let relative_path = url.strip_prefix("/uploads/").unwrap_or(url);
        let file_path = PathBuf::from(UPLOAD_DIR).join(relative_path);

        if file_path.exists() {
            fs::remove_file(&file_path)
                .await
                .map_err(|e| format!("删除文件失败: {}", e))?;
        }
        Ok(())
    }
}
