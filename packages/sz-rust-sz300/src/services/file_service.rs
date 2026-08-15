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
    ///
    /// # 设计定位（2026-08-15 论证，替代原"建议迁移"注释）
    ///
    /// 本服务采用 **bytes 直写模型**（HTTP 上传本质是内存流：JSON base64 /
    /// Multipart field），与 `sz_rust_core::upload` 引擎的 **文件路径模型**
    /// （`UploadedFile` 包装磁盘路径 + rename/move）API 形态不兼容。直接迁移
    /// 需要：bytes → 临时文件往返（性能与失败清理成本）、URL 契约变更
    /// （`/uploads/YYYY/MM/DD/xxx` → 引擎的 `storage/Ymd/...` 格式）、以及
    /// M-4 magic bytes 内容校验下沉（upload 引擎 validate 仅有 ext/mime/size 规则）。
    /// 故保持本实现：sz300 专用上传路径，云存储驱动由 upload 引擎供插件/多租户场景使用。
    ///
    /// # 安全
    ///
    /// 三重校验：大小上限 5MB → 扩展名白名单 → M-4 magic bytes 内容嗅探
    /// （2026-08-14 黑帽审计加固，防伪装图片扩展名的恶意内容）。
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

        // 安全修复 M-4（2026-08-14）：magic bytes 内容校验
        // 防止攻击者上传伪装成图片扩展名的恶意内容（HTML/JS 等）
        if !magic_bytes_match(&ext.to_lowercase(), data) {
            return Err(format!(
                "文件内容与扩展名 .{} 不匹配（内容嗅探校验失败）",
                ext
            ));
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
}

/// 校验文件内容（magic bytes）与声明的扩展名一致（安全修复 M-4）
///
/// 支持的格式与文件头：
/// - jpg/jpeg：`FF D8 FF`
/// - png：`89 50 4E 47 0D 0A 1A 0A`
/// - gif：`47 49 46 38`（GIF8）
/// - bmp：`42 4D`（BM）
fn magic_bytes_match(ext: &str, data: &[u8]) -> bool {
    match ext {
        "jpg" | "jpeg" => data.starts_with(&[0xFF, 0xD8, 0xFF]),
        "png" => data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
        "gif" => data.starts_with(b"GIF8"),
        "bmp" => data.starts_with(b"BM"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_magic_bytes_jpeg() {
        assert!(magic_bytes_match("jpg", &[0xFF, 0xD8, 0xFF, 0xE0, 0x00]));
        assert!(magic_bytes_match("jpeg", &[0xFF, 0xD8, 0xFF, 0xE1]));
        assert!(!magic_bytes_match("jpg", b"<html>"));
        assert!(!magic_bytes_match("jpg", &[]));
    }

    #[test]
    fn test_magic_bytes_png() {
        assert!(magic_bytes_match(
            "png",
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00]
        ));
        assert!(!magic_bytes_match("png", b"GIF89a"));
    }

    #[test]
    fn test_magic_bytes_gif_bmp() {
        assert!(magic_bytes_match("gif", b"GIF89a"));
        assert!(magic_bytes_match("gif", b"GIF87a"));
        assert!(magic_bytes_match("bmp", b"BM\x00\x00"));
        assert!(!magic_bytes_match("gif", b"<html>"));
    }

    #[test]
    fn test_magic_bytes_unknown_ext() {
        assert!(!magic_bytes_match("txt", b"hello"));
    }
}
