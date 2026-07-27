//! 文件上传校验模块 — 对齐 PHP `think\Validate` 文件校验规则 + `app\common\library\storage\Driver::validate`
//!
//! 本模块实现文件类型/大小校验机制，对齐 PHP：
//! - `think\Validate::fileSize` / `fileExt` / `fileMime` 三个校验规则
//!   （vendor `framework/src/think/Validate.php` 第 957-1062 行）
//! - `app\common\library\storage\Driver::validate` 业务层校验驱动
//!   （第 25-46 行，默认规则 + 自定义消息）
//! - `app\api\controller\file\Upload.php` 文件类型分类逻辑
//!   （第 62-69 行，image/video/file 三类）
//!
//! ## PHP 对齐
//!
//! ### 核心类映射
//!
//! | PHP 类/方法 | Rust 结构/方法 | 说明 |
//! |-------------|---------------|------|
//! | `Driver::validate($name, $fileInfo, $sence)` | [`FileValidator::validate_image`] | 图片校验驱动 |
//! | `validate([$name=>['fileSize'=>...]])` | [`FileValidateRule`] | 校验规则 |
//! | `$name.'.fileSize' => '最大可上传2M图片'` | [`FileValidateMessages`] | 校验消息 |
//! | `Validate::fileExt()` / `checkExt()` | [`FileValidator::check_ext`] | 扩展名校验 |
//! | `Validate::fileMime()` / `checkMime()` | [`FileValidator::check_mime`] | MIME 校验 |
//! | `Validate::fileSize()` / `checkSize()` | [`FileValidator::check_size`] | 大小校验 |
//! | `in_array($extension, [...])` 文件类型分类 | [`detect_file_type`] | 文件类型检测 |
//!
//! ### PHP 行为对齐（R5 硬约束）
//!
//! - **R5-9**：`checkExt` 使用 `strtolower($file->extension())` + `in_array($ext, explode(',', $rule))`
//!   （对齐 PHP Validate.php 第 957-964 行）
//! - **R5-10**：`checkMime` 使用 `strtolower($file->getMime())` + `in_array($mime, explode(',', $rule))`
//!   （对齐 PHP Validate.php 第 985-992 行）
//! - **R5-11**：`checkSize` 使用 `$file->getSize() <= (int) $size`
//!   （对齐 PHP Validate.php 第 973-976 行）
//! - **R5-12**：`Driver::validate` 仅 `sence == 'image'` 执行校验
//!   （对齐 PHP Driver.php 第 26 行）
//! - **R5-13**：默认图片规则 `fileSize=20971520, fileExt='jpg,jpeg,png,gif,bmp', fileMime='image/jpeg,image/png,image/gif,image/bmp'`
//!   （对齐 PHP Driver.php 第 30-32 行）
//! - **R5-14**：默认图片消息 `'最大可上传2M图片'` / `'只能上传jpg,jpeg,png,gif,bmp格式图片'`
//!   （对齐 PHP Driver.php 第 34-38 行）
//! - **R5-15**：文件类型分类 image 12 种 / video 13 种 / file 其他
//!   （对齐 PHP Upload.php 第 62-69 行）
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\Validate.php`
//!   - 第 957-964 行：`checkExt(File $file, $ext)`
//!   - 第 973-976 行：`checkSize(File $file, $size)`
//!   - 第 985-992 行：`checkMime(File $file, $mime)`
//!   - 第 1002-1016 行：`fileExt($file, $rule)`
//!   - 第 1025-1039 行：`fileMime($file, $rule)`
//!   - 第 1048-1062 行：`fileSize($file, $rule)`
//! - `e:\vue\test\鲜视达\server\app\common\library\storage\Driver.php`
//!   - 第 25-46 行：`validate($name, $fileInfo, $sence = 'image')`
//! - `e:\vue\test\鲜视达\server\app\api\controller\file\Upload.php`
//!   - 第 62-69 行：文件类型分类逻辑

use super::{File, UploadedFile};

// ============================================================================
// 错误类型
// ============================================================================

/// 文件校验错误 — 对齐 PHP `Driver::validate` 校验失败
///
/// PHP 行为：校验失败时抛出异常，`Driver::validate` 捕获后设置 `$this->engine->error` 并返回 `false`。
/// Rust 端使用 `Result<(), FileValidateError>` 表达校验失败。
#[derive(Debug, thiserror::Error)]
pub enum FileValidateError {
    /// 文件大小超过限制（对齐 PHP `fileSize` 规则失败）
    ///
    /// 错误消息对齐 PHP `Driver.php` 第 35 行：`'最大可上传2M图片'`
    #[error("{msg}")]
    SizeExceeded {
        /// 实际大小（字节）
        actual: u64,
        /// 最大允许大小（字节）
        max: u64,
        /// 错误消息（对齐 PHP 自定义消息）
        msg: String,
    },

    /// 扩展名不允许（对齐 PHP `fileExt` 规则失败）
    ///
    /// 错误消息对齐 PHP `Driver.php` 第 36 行：`'只能上传jpg,jpeg,png,gif,bmp格式图片'`
    #[error("{msg}")]
    ExtNotAllowed {
        /// 实际扩展名（小写）
        ext: String,
        /// 允许的扩展名列表
        allowed: Vec<String>,
        /// 错误消息
        msg: String,
    },

    /// MIME 类型不允许（对齐 PHP `fileMime` 规则失败）
    ///
    /// 错误消息对齐 PHP `Driver.php` 第 37 行：`'只能上传jpg,jpeg,png,gif,bmp格式图片'`
    #[error("{msg}")]
    MimeNotAllowed {
        /// 实际 MIME 类型（小写）
        mime: String,
        /// 允许的 MIME 列表
        allowed: Vec<String>,
        /// 错误消息
        msg: String,
    },

    /// 获取文件信息失败（IO 错误）
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// 获取 MIME 失败（对齐 PHP `finfo_file` 失败）
    #[error("获取 MIME 失败: {0}")]
    MimeDetect(String),
}

// ============================================================================
// 校验规则
// ============================================================================

/// 文件校验规则 — 对齐 PHP `Driver::validate` 的 `fileSize`/`fileExt`/`fileMime` 规则
///
/// PHP 原始规则（`app/common/library/storage/Driver.php` 第 29-33 行）：
/// ```php
/// validate([$name=>[
///     'fileSize' => 20971520,
///     'fileExt' => 'jpg,jpeg,png,gif,bmp',
///     'fileMime' => 'image/jpeg,image/png,image/gif,image/bmp',
/// ]])
/// ```
///
/// ## 字段语义
///
/// - `file_size`：`None` 表示不校验大小（对齐 PHP 未设置 `fileSize` 规则）
/// - `file_ext`：`None` 表示不校验扩展名
/// - `file_mime`：`None` 表示不校验 MIME
#[derive(Debug, Clone, Default)]
pub struct FileValidateRule {
    /// 最大文件大小（字节），None 表示不校验
    pub file_size: Option<u64>,
    /// 扩展名白名单（小写），None 表示不校验
    pub file_ext: Option<Vec<String>>,
    /// MIME 白名单（小写），None 表示不校验
    pub file_mime: Option<Vec<String>>,
}

impl FileValidateRule {
    /// 创建空规则（不校验任何项）
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置最大文件大小（对齐 PHP `'fileSize' => 20971520`）
    pub fn with_size(mut self, size: u64) -> Self {
        self.file_size = Some(size);
        self
    }

    /// 设置扩展名白名单（从逗号分隔字符串解析，对齐 PHP `explode(',', $ext)` + `strtolower`）
    pub fn with_ext(mut self, ext: &str) -> Self {
        self.file_ext = Some(parse_ext_list(ext));
        self
    }

    /// 设置扩展名白名单（从 Vec，自动 lowercase）
    pub fn with_ext_vec(mut self, ext: Vec<String>) -> Self {
        self.file_ext = Some(ext.into_iter().map(|e| e.to_lowercase()).collect());
        self
    }

    /// 设置 MIME 白名单（从逗号分隔字符串解析）
    pub fn with_mime(mut self, mime: &str) -> Self {
        self.file_mime = Some(parse_mime_list(mime));
        self
    }

    /// 设置 MIME 白名单（从 Vec，自动 lowercase）
    pub fn with_mime_vec(mut self, mime: Vec<String>) -> Self {
        self.file_mime = Some(mime.into_iter().map(|m| m.to_lowercase()).collect());
        self
    }

    /// 默认图片校验规则 — 对齐 PHP `Driver::validate` 第 30-32 行
    ///
    /// PHP 原始规则：
    /// ```php
    /// 'fileSize' => 20971520, // 20MB（PHP 注释错误标为 "2M"）
    /// 'fileExt' => 'jpg,jpeg,png,gif,bmp',
    /// 'fileMime' => 'image/jpeg,image/png,image/gif,image/bmp',
    /// ```
    pub fn default_image() -> Self {
        Self::new()
            .with_size(20 * 1024 * 1024)
            .with_ext("jpg,jpeg,png,gif,bmp")
            .with_mime("image/jpeg,image/png,image/gif,image/bmp")
    }
}

// ============================================================================
// 校验消息
// ============================================================================

/// 文件校验消息 — 对齐 PHP `Driver::validate` 第 34-38 行自定义消息
///
/// PHP 原始消息：
/// ```php
/// [
///     $name.'.fileSize' => '最大可上传2M图片',
///     $name.'.fileExt' => '只能上传jpg,jpeg,png,gif,bmp格式图片',
///     $name.'.fileMime' => '只能上传jpg,jpeg,png,gif,bmp格式图片'
/// ]
/// ```
#[derive(Debug, Clone)]
pub struct FileValidateMessages {
    /// 大小校验失败消息
    pub file_size: String,
    /// 扩展名校验失败消息
    pub file_ext: String,
    /// MIME 校验失败消息
    pub file_mime: String,
}

impl Default for FileValidateMessages {
    /// 默认消息 — 对齐 PHP `Validate.php` 第 110-112 行 `$typeMsg` + `zh-cn.php` 第 90-92 行翻译
    ///
    /// PHP 默认消息（中文环境）：
    /// - `fileSize` → `'上传文件大小不符！'`
    /// - `fileExt` → `'上传文件后缀不允许'`
    /// - `fileMime` → `'上传文件MIME类型不允许！'`
    fn default() -> Self {
        Self {
            file_size: "上传文件大小不符！".to_string(),
            file_ext: "上传文件后缀不允许".to_string(),
            file_mime: "上传文件MIME类型不允许！".to_string(),
        }
    }
}

impl FileValidateMessages {
    /// 默认图片校验消息 — 对齐 PHP `Driver::validate` 第 34-38 行
    pub fn default_image() -> Self {
        Self {
            file_size: "最大可上传2M图片".to_string(),
            file_ext: "只能上传jpg,jpeg,png,gif,bmp格式图片".to_string(),
            file_mime: "只能上传jpg,jpeg,png,gif,bmp格式图片".to_string(),
        }
    }
}

// ============================================================================
// 文件校验器
// ============================================================================

/// 文件校验器 — 对齐 PHP `app\common\library\storage\Driver::validate`
///
/// PHP 行为（第 25-46 行）：
/// ```php
/// public function validate($name, $fileInfo, $sence = 'image'){
///     if($sence == 'image'){
///         try{
///             validate([$name=>[...]], [$name.'.fileSize' => '...', ...])
///                 ->check([$name => $fileInfo]);
///             return true;
///         }catch(\Exception $e){
///             $this->engine->error = $e->getMessage();
///             return false;
///         }
///     }
///     return false;
/// }
/// ```
///
/// ## Rust 端语义
///
/// - PHP `sence == 'image'` 分支 → [`FileValidator::validate_image`]
/// - PHP `sence != 'image'` 直接 `return false` → Rust 端不提供此分支
///   （业务层应直接拒绝非 image 场景，或使用其他校验器）
#[derive(Debug, Clone)]
pub struct FileValidator {
    rule: FileValidateRule,
    messages: FileValidateMessages,
}

impl Default for FileValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl FileValidator {
    /// 创建校验器（使用默认图片规则和消息，对齐 PHP `Driver::validate` 默认行为）
    pub fn new() -> Self {
        Self {
            rule: FileValidateRule::default_image(),
            messages: FileValidateMessages::default_image(),
        }
    }

    /// 创建校验器（自定义规则和消息）
    pub fn with(rule: FileValidateRule, messages: FileValidateMessages) -> Self {
        Self { rule, messages }
    }

    /// 获取校验规则引用
    pub fn rule(&self) -> &FileValidateRule {
        &self.rule
    }

    /// 获取校验消息引用
    pub fn messages(&self) -> &FileValidateMessages {
        &self.messages
    }

    /// 校验扩展名 — 对齐 PHP `Validate::checkExt` 第 957-964 行
    ///
    /// PHP 行为：
    /// ```php
    /// protected function checkExt(File $file, $ext): bool {
    ///     if (is_string($ext)) { $ext = explode(',', $ext); }
    ///     return in_array(strtolower($file->extension()), $ext);
    /// }
    /// ```
    ///
    /// **注意**：`$file->extension()` 对 `UploadedFile` 会调用覆写版本（返回 `original_extension`）。
    pub fn check_ext(file: &UploadedFile, allowed: &[String]) -> bool {
        let ext = file.extension().to_lowercase();
        allowed.contains(&ext)
    }

    /// 校验 MIME — 对齐 PHP `Validate::checkMime` 第 985-992 行
    ///
    /// PHP 行为：
    /// ```php
    /// protected function checkMime(File $file, $mime): bool {
    ///     if (is_string($mime)) { $mime = explode(',', $mime); }
    ///     return in_array(strtolower($file->getMime()), $mime);
    /// }
    /// ```
    ///
    /// **注意**：`$file->getMime()` 使用服务器端 `finfo_file` 检测（Rust 端使用 `infer` crate）。
    pub fn check_mime(file: &File, allowed: &[String]) -> Result<bool, FileValidateError> {
        let mime = file
            .get_mime()
            .map_err(|e| FileValidateError::MimeDetect(e.to_string()))?;
        Ok(allowed.contains(&mime.to_lowercase()))
    }

    /// 校验大小 — 对齐 PHP `Validate::checkSize` 第 973-976 行
    ///
    /// PHP 行为：
    /// ```php
    /// protected function checkSize(File $file, $size): bool {
    ///     return $file->getSize() <= (int) $size;
    /// }
    /// ```
    pub fn check_size(file: &File, max: u64) -> Result<bool, FileValidateError> {
        let actual = file.path().metadata()?.len();
        Ok(actual <= max)
    }

    /// 校验文件 — 对齐 PHP `Driver::validate($name, $fileInfo, 'image')`
    ///
    /// 执行顺序：扩展名 → MIME → 大小（对齐 PHP `validate()->check()` 的规则遍历顺序）
    ///
    /// ## 返回值
    ///
    /// - `Ok(())`：校验通过
    /// - `Err(FileValidateError::*)`：校验失败，包含错误消息（对齐 PHP `$e->getMessage()`）
    pub fn validate_image(&self, file: &UploadedFile) -> Result<(), FileValidateError> {
        // 对齐 PHP checkExt：strtolower($file->extension())
        if let Some(ref allowed_ext) = self.rule.file_ext {
            if !Self::check_ext(file, allowed_ext) {
                return Err(FileValidateError::ExtNotAllowed {
                    ext: file.extension().to_lowercase(),
                    allowed: allowed_ext.clone(),
                    msg: self.messages.file_ext.clone(),
                });
            }
        }

        // 对齐 PHP checkMime：strtolower($file->getMime())
        if let Some(ref allowed_mime) = self.rule.file_mime {
            if !Self::check_mime(file.as_file(), allowed_mime)? {
                return Err(FileValidateError::MimeNotAllowed {
                    mime: file.as_file().get_mime().unwrap_or_default().to_lowercase(),
                    allowed: allowed_mime.clone(),
                    msg: self.messages.file_mime.clone(),
                });
            }
        }

        // 对齐 PHP checkSize：$file->getSize() <= (int) $size
        if let Some(max_size) = self.rule.file_size {
            let actual = file.as_file().path().metadata()?.len();
            if actual > max_size {
                return Err(FileValidateError::SizeExceeded {
                    actual,
                    max: max_size,
                    msg: self.messages.file_size.clone(),
                });
            }
        }

        Ok(())
    }
}

// ============================================================================
// 文件类型分类
// ============================================================================

/// 文件类型分类 — 对齐 PHP `app\api\controller\file\Upload.php` 第 62-69 行
///
/// PHP 原始逻辑：
/// ```php
/// if(in_array($extension,['jpg','png','jpeg','bmp','gif','icon','svg','tif','webp','tiff','avif','pjp'])){
///     $file_type = 'image';
/// } else if(in_array($extension,['mp4','m3u8','mp3','wmv','mpg','webm','mov','avi','m4v','mpeg','ogv','asx','ogm'])){
///     $file_type = 'video';
/// } else {
///     $file_type = 'file';
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    /// 图片（image） — 12 种扩展名
    Image,
    /// 视频（video） — 13 种扩展名
    Video,
    /// 其他文件（file）
    File,
}

impl FileType {
    /// 转为字符串（对齐 PHP `$file_type` 字符串值）
    pub fn as_str(self) -> &'static str {
        match self {
            FileType::Image => "image",
            FileType::Video => "video",
            FileType::File => "file",
        }
    }
}

/// 图片扩展名白名单 — 对齐 PHP `Upload.php` 第 63 行
const IMAGE_EXTS: &[&str] = &[
    "jpg", "png", "jpeg", "bmp", "gif", "icon", "svg", "tif", "webp", "tiff", "avif", "pjp",
];

/// 视频扩展名白名单 — 对齐 PHP `Upload.php` 第 65 行
const VIDEO_EXTS: &[&str] = &[
    "mp4", "m3u8", "mp3", "wmv", "mpg", "webm", "mov", "avi", "m4v", "mpeg", "ogv", "asx", "ogm",
];

/// 检测文件类型 — 对齐 PHP `in_array($extension, [...])` 分类逻辑
///
/// ## 参数
///
/// - `ext`：文件扩展名（不区分大小写，内部 lowercase 后匹配）
///
/// ## 返回值
///
/// - [`FileType::Image`]：扩展名在 `IMAGE_EXTS` 中
/// - [`FileType::Video`]：扩展名在 `VIDEO_EXTS` 中
/// - [`FileType::File`]：其他
pub fn detect_file_type(ext: &str) -> FileType {
    let ext_lower = ext.to_lowercase();
    if IMAGE_EXTS.contains(&ext_lower.as_str()) {
        FileType::Image
    } else if VIDEO_EXTS.contains(&ext_lower.as_str()) {
        FileType::Video
    } else {
        FileType::File
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 解析扩展名列表 — 对齐 PHP `explode(',', $ext)` + `strtolower`
///
/// PHP 行为（`Validate::checkExt` 第 959-961 行）：
/// ```php
/// if (is_string($ext)) {
///     $ext = explode(',', $ext);
/// }
/// ```
///
/// Rust 端额外处理：
/// - `trim()` 去除空格（PHP `explode` 不 trim）
/// - 过滤空字符串（PHP `explode` 会保留空元素，但 `in_array` 不会匹配空扩展名）
pub fn parse_ext_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect()
}

/// 解析 MIME 列表 — 对齐 PHP `explode(',', $mime)` + `strtolower`
///
/// PHP 行为（`Validate::checkMime` 第 987-989 行）：
/// ```php
/// if (is_string($mime)) {
///     $mime = explode(',', $mime);
/// }
/// ```
pub fn parse_mime_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect()
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upload::UploadedFile;
    use std::io::Write;

    // ---- 辅助函数 ----

    /// 创建临时文件并写入内容
    fn create_temp_file(content: &[u8], suffix: &str) -> tempfile::NamedTempFile {
        let mut temp = tempfile::Builder::new()
            .suffix(suffix)
            .tempfile()
            .expect("创建临时文件失败");
        temp.write_all(content).expect("写入临时文件失败");
        temp.flush().expect("flush 失败");
        temp
    }

    // ========================================================================
    // 组 1：FileValidateRule 基础
    // ========================================================================

    #[test]
    fn test_rule_new() {
        let rule = FileValidateRule::new();
        assert!(rule.file_size.is_none());
        assert!(rule.file_ext.is_none());
        assert!(rule.file_mime.is_none());
    }

    #[test]
    fn test_rule_with_size() {
        let rule = FileValidateRule::new().with_size(1024);
        assert_eq!(rule.file_size, Some(1024));
    }

    #[test]
    fn test_rule_with_ext() {
        let rule = FileValidateRule::new().with_ext("jpg,png,GIF");
        assert_eq!(
            rule.file_ext,
            Some(vec![
                "jpg".to_string(),
                "png".to_string(),
                "gif".to_string(),
            ])
        );
    }

    #[test]
    fn test_rule_with_mime() {
        let rule = FileValidateRule::new().with_mime("image/jpeg,image/png");
        assert_eq!(
            rule.file_mime,
            Some(vec!["image/jpeg".to_string(), "image/png".to_string(),])
        );
    }

    #[test]
    fn test_rule_default_image() {
        let rule = FileValidateRule::default_image();
        // 对齐 PHP Driver.php 第 30 行：fileSize = 20971520
        assert_eq!(rule.file_size, Some(20 * 1024 * 1024));
        // 对齐 PHP Driver.php 第 31 行：fileExt = 'jpg,jpeg,png,gif,bmp'
        assert_eq!(
            rule.file_ext,
            Some(vec![
                "jpg".to_string(),
                "jpeg".to_string(),
                "png".to_string(),
                "gif".to_string(),
                "bmp".to_string(),
            ])
        );
        // 对齐 PHP Driver.php 第 32 行：fileMime = 'image/jpeg,image/png,image/gif,image/bmp'
        assert_eq!(
            rule.file_mime,
            Some(vec![
                "image/jpeg".to_string(),
                "image/png".to_string(),
                "image/gif".to_string(),
                "image/bmp".to_string(),
            ])
        );
    }

    #[test]
    fn test_rule_with_ext_vec() {
        let rule = FileValidateRule::new().with_ext_vec(vec!["JPG".to_string(), "PNG".to_string()]);
        assert_eq!(
            rule.file_ext,
            Some(vec!["jpg".to_string(), "png".to_string(),])
        );
    }

    #[test]
    fn test_rule_with_mime_vec() {
        let rule = FileValidateRule::new()
            .with_mime_vec(vec!["IMAGE/JPEG".to_string(), "IMAGE/PNG".to_string()]);
        assert_eq!(
            rule.file_mime,
            Some(vec!["image/jpeg".to_string(), "image/png".to_string(),])
        );
    }

    // ========================================================================
    // 组 2：FileValidateMessages
    // ========================================================================

    #[test]
    fn test_messages_default() {
        let msgs = FileValidateMessages::default();
        // 对齐 PHP zh-cn.php 第 90-92 行
        assert_eq!(msgs.file_size, "上传文件大小不符！");
        assert_eq!(msgs.file_ext, "上传文件后缀不允许");
        assert_eq!(msgs.file_mime, "上传文件MIME类型不允许！");
    }

    #[test]
    fn test_messages_default_image() {
        let msgs = FileValidateMessages::default_image();
        // 对齐 PHP Driver.php 第 35-37 行
        assert_eq!(msgs.file_size, "最大可上传2M图片");
        assert_eq!(msgs.file_ext, "只能上传jpg,jpeg,png,gif,bmp格式图片");
        assert_eq!(msgs.file_mime, "只能上传jpg,jpeg,png,gif,bmp格式图片");
    }

    #[test]
    fn test_messages_custom() {
        let msgs = FileValidateMessages {
            file_size: "文件太大".to_string(),
            file_ext: "格式不对".to_string(),
            file_mime: "MIME不对".to_string(),
        };
        assert_eq!(msgs.file_size, "文件太大");
        assert_eq!(msgs.file_ext, "格式不对");
        assert_eq!(msgs.file_mime, "MIME不对");
    }

    // ========================================================================
    // 组 3：FileValidator 基础
    // ========================================================================

    #[test]
    fn test_validator_new() {
        let v = FileValidator::new();
        assert_eq!(v.rule().file_size, Some(20 * 1024 * 1024));
        assert_eq!(v.messages().file_size, "最大可上传2M图片");
    }

    #[test]
    fn test_validator_with_custom() {
        let rule = FileValidateRule::new().with_ext("pdf,doc");
        let msgs = FileValidateMessages::default();
        let v = FileValidator::with(rule, msgs);
        assert_eq!(
            v.rule().file_ext,
            Some(vec!["pdf".to_string(), "doc".to_string()])
        );
        assert_eq!(v.messages().file_ext, "上传文件后缀不允许");
    }

    // ========================================================================
    // 组 4：FileValidator::check_ext
    // ========================================================================

    #[test]
    fn test_check_ext_pass() {
        let temp = create_temp_file(b"hello", ".jpg");
        let file = UploadedFile::new(temp.path(), "photo.JPG", None, Some(0), true).unwrap();
        let allowed = parse_ext_list("jpg,jpeg,png,gif,bmp");
        // 对齐 PHP checkExt：strtolower($file->extension()) in_array
        assert!(FileValidator::check_ext(&file, &allowed));
    }

    #[test]
    fn test_check_ext_fail() {
        let temp = create_temp_file(b"hello", ".txt");
        let file = UploadedFile::new(temp.path(), "doc.txt", None, Some(0), true).unwrap();
        let allowed = parse_ext_list("jpg,jpeg,png,gif,bmp");
        assert!(!FileValidator::check_ext(&file, &allowed));
    }

    #[test]
    fn test_check_ext_case_insensitive() {
        // 对齐 PHP strtolower：扩展名大写也能匹配小写白名单
        let temp = create_temp_file(b"hello", ".jpg");
        let file = UploadedFile::new(temp.path(), "photo.JPEG", None, Some(0), true).unwrap();
        let allowed = parse_ext_list("jpg,jpeg");
        assert!(FileValidator::check_ext(&file, &allowed));
    }

    // ========================================================================
    // 组 5：FileValidator::check_size
    // ========================================================================

    #[test]
    fn test_check_size_pass() {
        let temp = create_temp_file(b"hello", ".txt");
        let file = File::new(temp.path(), false).unwrap();
        // 5 bytes <= 100
        assert!(FileValidator::check_size(&file, 100).unwrap());
    }

    #[test]
    fn test_check_size_equal() {
        // 对齐 PHP <= 语义：等于也算通过
        let temp = create_temp_file(b"hello", ".txt");
        let file = File::new(temp.path(), false).unwrap();
        // 5 bytes <= 5
        assert!(FileValidator::check_size(&file, 5).unwrap());
    }

    #[test]
    fn test_check_size_fail() {
        let temp = create_temp_file(b"hello world", ".txt");
        let file = File::new(temp.path(), false).unwrap();
        // 11 bytes > 5
        assert!(!FileValidator::check_size(&file, 5).unwrap());
    }

    // ========================================================================
    // 组 6：FileValidator::validate_image
    // ========================================================================

    #[test]
    fn test_validate_image_ext_pass() {
        let temp = create_temp_file(b"\x89PNG\r\n\x1a\n", ".png");
        let file = UploadedFile::new(temp.path(), "photo.png", None, Some(0), true).unwrap();
        let v = FileValidator::new();
        // 扩展名 png 在白名单中
        let result = v.validate_image(&file);
        // MIME 可能不匹配（因为是假 PNG），但扩展名应该通过
        // 这里只验证扩展名通过（可能因 MIME 失败，但不应该是 ExtNotAllowed）
        match result {
            Ok(()) => {}
            Err(FileValidateError::ExtNotAllowed { .. }) => panic!("扩展名应通过"),
            Err(_) => {}
        }
    }

    #[test]
    fn test_validate_image_ext_fail() {
        let temp = create_temp_file(b"hello", ".txt");
        let file = UploadedFile::new(temp.path(), "doc.txt", None, Some(0), true).unwrap();
        let v = FileValidator::new();
        let result = v.validate_image(&file);
        // 对齐 PHP：扩展名 txt 不在白名单 → ExtNotAllowed
        assert!(matches!(
            result,
            Err(FileValidateError::ExtNotAllowed { .. })
        ));
    }

    #[test]
    fn test_validate_image_size_fail() {
        // 创建一个扩展名和 MIME 都通过但大小超限的文件
        // 使用自定义规则：只校验大小
        let temp = create_temp_file(b"hello world, this is a long file", ".jpg");
        let file = UploadedFile::new(temp.path(), "photo.jpg", None, Some(0), true).unwrap();
        let rule = FileValidateRule::new().with_size(5); // 只校验大小 <= 5
        let v = FileValidator::with(rule, FileValidateMessages::default());
        let result = v.validate_image(&file);
        assert!(matches!(
            result,
            Err(FileValidateError::SizeExceeded { .. })
        ));
    }

    #[test]
    fn test_validate_image_all_pass() {
        // 创建一个真实的 PNG 文件
        let png_header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01";
        let temp = create_temp_file(png_header, ".png");
        let file = UploadedFile::new(temp.path(), "photo.png", None, Some(0), true).unwrap();
        let rule = FileValidateRule::new()
            .with_ext("png")
            .with_mime("image/png")
            .with_size(1024);
        let v = FileValidator::with(rule, FileValidateMessages::default());
        let result = v.validate_image(&file);
        assert!(result.is_ok(), "校验应通过: {:?}", result);
    }

    #[test]
    fn test_validate_image_no_rule_passes() {
        // 空规则 → 任何文件都通过
        let temp = create_temp_file(b"hello", ".txt");
        let file = UploadedFile::new(temp.path(), "doc.txt", None, Some(0), true).unwrap();
        let v = FileValidator::with(FileValidateRule::new(), FileValidateMessages::default());
        let result = v.validate_image(&file);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_image_error_messages() {
        // 对齐 PHP Driver.php 第 34-38 行自定义消息
        let temp = create_temp_file(b"hello", ".txt");
        let file = UploadedFile::new(temp.path(), "doc.txt", None, Some(0), true).unwrap();
        let v = FileValidator::new(); // 默认图片规则和消息
        let result = v.validate_image(&file);
        match result {
            Err(FileValidateError::ExtNotAllowed { msg, .. }) => {
                assert_eq!(msg, "只能上传jpg,jpeg,png,gif,bmp格式图片");
            }
            _ => panic!("应返回 ExtNotAllowed"),
        }
    }

    // ========================================================================
    // 组 7：detect_file_type
    // ========================================================================

    #[test]
    fn test_detect_file_type_image() {
        // 对齐 PHP Upload.php 第 63 行：12 种图片扩展名
        assert_eq!(detect_file_type("jpg"), FileType::Image);
        assert_eq!(detect_file_type("png"), FileType::Image);
        assert_eq!(detect_file_type("jpeg"), FileType::Image);
        assert_eq!(detect_file_type("bmp"), FileType::Image);
        assert_eq!(detect_file_type("gif"), FileType::Image);
        assert_eq!(detect_file_type("icon"), FileType::Image);
        assert_eq!(detect_file_type("svg"), FileType::Image);
        assert_eq!(detect_file_type("tif"), FileType::Image);
        assert_eq!(detect_file_type("webp"), FileType::Image);
        assert_eq!(detect_file_type("tiff"), FileType::Image);
        assert_eq!(detect_file_type("avif"), FileType::Image);
        assert_eq!(detect_file_type("pjp"), FileType::Image);
    }

    #[test]
    fn test_detect_file_type_video() {
        // 对齐 PHP Upload.php 第 65 行：13 种视频扩展名
        assert_eq!(detect_file_type("mp4"), FileType::Video);
        assert_eq!(detect_file_type("m3u8"), FileType::Video);
        assert_eq!(detect_file_type("mp3"), FileType::Video);
        assert_eq!(detect_file_type("wmv"), FileType::Video);
        assert_eq!(detect_file_type("mpg"), FileType::Video);
        assert_eq!(detect_file_type("webm"), FileType::Video);
        assert_eq!(detect_file_type("mov"), FileType::Video);
        assert_eq!(detect_file_type("avi"), FileType::Video);
        assert_eq!(detect_file_type("m4v"), FileType::Video);
        assert_eq!(detect_file_type("mpeg"), FileType::Video);
        assert_eq!(detect_file_type("ogv"), FileType::Video);
        assert_eq!(detect_file_type("asx"), FileType::Video);
        assert_eq!(detect_file_type("ogm"), FileType::Video);
    }

    #[test]
    fn test_detect_file_type_file() {
        // 其他扩展名 → File
        assert_eq!(detect_file_type("pdf"), FileType::File);
        assert_eq!(detect_file_type("doc"), FileType::File);
        assert_eq!(detect_file_type("xls"), FileType::File);
        assert_eq!(detect_file_type("zip"), FileType::File);
        assert_eq!(detect_file_type("exe"), FileType::File);
        assert_eq!(detect_file_type("php"), FileType::File);
    }

    #[test]
    fn test_detect_file_type_case_insensitive() {
        // 对齐 PHP in_array 默认区分大小写，但业务代码通常先 lowercase
        // Rust 端内部 lowercase，对齐 PHP 业务实践
        assert_eq!(detect_file_type("JPG"), FileType::Image);
        assert_eq!(detect_file_type("MP4"), FileType::Video);
        assert_eq!(detect_file_type("PDF"), FileType::File);
    }

    #[test]
    fn test_file_type_as_str() {
        assert_eq!(FileType::Image.as_str(), "image");
        assert_eq!(FileType::Video.as_str(), "video");
        assert_eq!(FileType::File.as_str(), "file");
    }

    // ========================================================================
    // 组 8：辅助函数
    // ========================================================================

    #[test]
    fn test_parse_ext_list_basic() {
        // 对齐 PHP explode(',', $ext)
        let list = parse_ext_list("jpg,jpeg,png,gif,bmp");
        assert_eq!(list, vec!["jpg", "jpeg", "png", "gif", "bmp"]);
    }

    #[test]
    fn test_parse_ext_list_lowercase() {
        // 对齐 PHP strtolower
        let list = parse_ext_list("JPG,JPEG,PNG");
        assert_eq!(list, vec!["jpg", "jpeg", "png"]);
    }

    #[test]
    fn test_parse_ext_list_trim() {
        // PHP explode 不 trim，但 Rust 端额外 trim（更宽松）
        let list = parse_ext_list("jpg, jpeg , png");
        assert_eq!(list, vec!["jpg", "jpeg", "png"]);
    }

    #[test]
    fn test_parse_ext_list_empty() {
        let list = parse_ext_list("");
        assert!(list.is_empty());
    }

    #[test]
    fn test_parse_mime_list_basic() {
        let list = parse_mime_list("image/jpeg,image/png,image/gif,image/bmp");
        assert_eq!(
            list,
            vec!["image/jpeg", "image/png", "image/gif", "image/bmp"]
        );
    }

    #[test]
    fn test_parse_mime_list_lowercase() {
        let list = parse_mime_list("IMAGE/JPEG,IMAGE/PNG");
        assert_eq!(list, vec!["image/jpeg", "image/png"]);
    }

    // ========================================================================
    // 组 9：PHP 行为对齐 R5
    // ========================================================================

    /// R5-13：默认图片规则对齐 PHP Driver.php 第 30-32 行
    #[test]
    fn test_php_behavior_default_image_rule() {
        let rule = FileValidateRule::default_image();
        // PHP Driver.php 第 30 行：fileSize = 20971520
        assert_eq!(rule.file_size, Some(20971520));
        // PHP Driver.php 第 31 行：fileExt = 'jpg,jpeg,png,gif,bmp'
        assert_eq!(
            rule.file_ext,
            Some(
                ["jpg", "jpeg", "png", "gif", "bmp"]
                    .iter()
                    .map(|&s| s.to_string())
                    .collect::<Vec<_>>()
            )
        );
        // PHP Driver.php 第 32 行：fileMime = 'image/jpeg,image/png,image/gif,image/bmp'
        assert_eq!(
            rule.file_mime,
            Some(
                ["image/jpeg", "image/png", "image/gif", "image/bmp"]
                    .iter()
                    .map(|&s| s.to_string())
                    .collect::<Vec<_>>()
            )
        );
    }

    /// R5-14：默认图片消息对齐 PHP Driver.php 第 34-38 行
    #[test]
    fn test_php_behavior_default_image_messages() {
        let msgs = FileValidateMessages::default_image();
        // PHP Driver.php 第 35 行
        assert_eq!(msgs.file_size, "最大可上传2M图片");
        // PHP Driver.php 第 36 行
        assert_eq!(msgs.file_ext, "只能上传jpg,jpeg,png,gif,bmp格式图片");
        // PHP Driver.php 第 37 行
        assert_eq!(msgs.file_mime, "只能上传jpg,jpeg,png,gif,bmp格式图片");
    }

    /// R5-9：checkExt 使用 strtolower + in_array
    #[test]
    fn test_php_behavior_check_ext_lowercase() {
        let temp = create_temp_file(b"hello", ".jpg");
        // 原始扩展名大写 → strtolower 后匹配
        let file = UploadedFile::new(temp.path(), "PHOTO.JPG", None, Some(0), true).unwrap();
        let allowed = parse_ext_list("jpg,jpeg,png,gif,bmp");
        // 对齐 PHP strtolower($file->extension()) in_array
        assert!(FileValidator::check_ext(&file, &allowed));
    }

    /// R5-10：checkMime 使用 strtolower + in_array
    #[test]
    fn test_php_behavior_check_mime_lowercase() {
        // 创建真实 PNG 文件
        let png_header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01";
        let temp = create_temp_file(png_header, ".png");
        let file = File::new(temp.path(), false).unwrap();
        // MIME 白名单小写
        let allowed = parse_mime_list("image/png,image/jpeg");
        // 对齐 PHP strtolower($file->getMime()) in_array
        assert!(FileValidator::check_mime(&file, &allowed).unwrap());
    }

    /// R5-11：checkSize 使用 <= 语义
    #[test]
    fn test_php_behavior_check_size_leq() {
        // 对齐 PHP $file->getSize() <= (int) $size
        let temp = create_temp_file(b"hello", ".txt"); // 5 bytes
        let file = File::new(temp.path(), false).unwrap();
        // 等于 → 通过
        assert!(FileValidator::check_size(&file, 5).unwrap());
        // 小于 → 通过
        assert!(FileValidator::check_size(&file, 10).unwrap());
        // 大于 → 失败
        assert!(!FileValidator::check_size(&file, 4).unwrap());
    }

    /// R5-15：文件类型分类 image 12 种 / video 13 种 / file 其他
    #[test]
    fn test_php_behavior_file_type_classification() {
        // 图片 12 种（对齐 PHP Upload.php 第 63 行）
        let image_count = [
            "jpg", "png", "jpeg", "bmp", "gif", "icon", "svg", "tif", "webp", "tiff", "avif", "pjp",
        ]
        .iter()
        .filter(|&&e| detect_file_type(e) == FileType::Image)
        .count();
        assert_eq!(image_count, 12);

        // 视频 13 种（对齐 PHP Upload.php 第 65 行）
        let video_count = [
            "mp4", "m3u8", "mp3", "wmv", "mpg", "webm", "mov", "avi", "m4v", "mpeg", "ogv", "asx",
            "ogm",
        ]
        .iter()
        .filter(|&&e| detect_file_type(e) == FileType::Video)
        .count();
        assert_eq!(video_count, 13);

        // 其他 → File
        assert_eq!(detect_file_type("pdf"), FileType::File);
        assert_eq!(detect_file_type("xyz"), FileType::File);
        assert_eq!(detect_file_type(""), FileType::File);
    }

    /// R5-12：Driver::validate 仅 sence == 'image' 执行校验
    /// Rust 端语义：FileValidator::validate_image 仅执行图片校验
    #[test]
    fn test_php_behavior_validate_image_only() {
        // 验证 FileValidator 只有 validate_image 方法（对齐 PHP sence == 'image' 分支）
        // 非 image 场景由业务层处理，对齐 PHP sence != 'image' return false
        let temp = create_temp_file(b"hello", ".txt");
        let file = UploadedFile::new(temp.path(), "doc.txt", None, Some(0), true).unwrap();
        let v = FileValidator::new();
        // txt 扩展名不在图片白名单 → 失败
        let result = v.validate_image(&file);
        assert!(matches!(
            result,
            Err(FileValidateError::ExtNotAllowed { .. })
        ));
    }

    /// 验证校验顺序：扩展名 → MIME → 大小
    #[test]
    fn test_validate_order_ext_first() {
        // 扩展名失败时不应检查 MIME/大小
        let temp = create_temp_file(b"hello", ".txt");
        let file = UploadedFile::new(temp.path(), "doc.txt", None, Some(0), true).unwrap();
        let rule = FileValidateRule::new()
            .with_ext("jpg")
            .with_mime("image/jpeg")
            .with_size(1); // 大小也会失败，但扩展名先失败
        let v = FileValidator::with(rule, FileValidateMessages::default());
        let result = v.validate_image(&file);
        // 应返回 ExtNotAllowed（而不是 SizeExceeded）
        assert!(matches!(
            result,
            Err(FileValidateError::ExtNotAllowed { .. })
        ));
    }

    /// 验证校验顺序：MIME 先于大小
    #[test]
    fn test_validate_order_mime_before_size() {
        // 扩展名通过，MIME 失败时不应检查大小
        // 使用 PNG 头内容 + .jpg 扩展名：infer 检测为 image/png，不匹配 image/jpeg 白名单
        let png_header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01";
        let temp = create_temp_file(png_header, ".jpg");
        let file = UploadedFile::new(temp.path(), "photo.jpg", None, Some(0), true).unwrap();
        let rule = FileValidateRule::new()
            .with_ext("jpg")
            .with_mime("image/jpeg") // 实际 MIME 是 image/png → 失败
            .with_size(1); // 大小也会失败，但 MIME 先失败
        let v = FileValidator::with(rule, FileValidateMessages::default());
        let result = v.validate_image(&file);
        // 应返回 MimeNotAllowed（而不是 SizeExceeded）
        assert!(matches!(
            result,
            Err(FileValidateError::MimeNotAllowed { .. })
        ));
    }
}
