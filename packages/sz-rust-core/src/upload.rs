//! 文件上传模块 — 对齐 PHP `think\File` + `think\file\UploadedFile`
//!
//! Phase 5.5 核心交付物。本模块实现文件上传机制，对齐 PHP `think\File` 类
//! 的 hash/move/hashName/extension/getMime 等核心方法，以及 `think\file\UploadedFile`
//! 的 isValid/move/getOriginalName/getOriginalExtension 等方法。
//!
//! ## PHP 对齐
//!
//! ### 核心类映射
//!
//! | PHP 类 | Rust 结构 | 说明 |
//! |---------|-----------|------|
//! | `think\File`（extends `SplFileInfo`） | [`File`] | 文件类 |
//! | `think\file\UploadedFile`（extends `File`） | [`UploadedFile`] | 上传文件类 |
//! | `think\exception\FileException` | [`UploadError`] | 文件异常 |
//!
//! ### 核心方法映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `File::hash($type)` | [`File::hash`] | 获取文件哈希 |
//! | `File::md5()` | [`File::md5`] | 获取文件 MD5 |
//! | `File::sha1()` | [`File::sha1`] | 获取文件 SHA1 |
//! | `File::getMime()` | [`File::get_mime`] | 获取文件 MIME |
//! | `File::move($dir, $name)` | [`File::move_to`] | 移动文件 |
//! | `File::extension()` | [`File::extension`] | 文件扩展名 |
//! | `File::setExtension($ext)` | [`File::set_extension`] | 设置扩展名 |
//! | `File::hashName($rule)` | [`File::hash_name`] | 生成哈希文件名 |
//! | `UploadedFile::isValid()` | [`UploadedFile::is_valid`] | 验证上传文件 |
//! | `UploadedFile::move($dir, $name)` | [`UploadedFile::move_to`] | 移动上传文件 |
//! | `UploadedFile::getOriginalMime()` | [`UploadedFile::original_mime`] | 原始 MIME |
//! | `UploadedFile::getOriginalName()` | [`UploadedFile::original_name`] | 原始文件名 |
//! | `UploadedFile::getOriginalExtension()` | [`UploadedFile::original_extension`] | 原始扩展名 |
//! | `UploadedFile::extension()` | [`UploadedFile::extension`] | 覆写父类扩展名 |
//!
//! ## PHP 行为对齐（R5 硬约束）
//!
//! - **R5-1**：`hashName` 默认规则 = `date('Ymd') . DIRECTORY_SEPARATOR . md5(microtime(true) . pathname)`
//!   （对齐 PHP 第 195 行）
//! - **R5-2**：`hashName` hash 算法规则 = `substr(hash, 0, 2) . DIRECTORY_SEPARATOR . substr(hash, 2)`
//!   （对齐 PHP 第 187-190 行）
//! - **R5-3**：`UploadedFile::isValid` = `error == UPLOAD_ERR_OK && is_uploaded_file(pathname)`
//!   （对齐 PHP 第 36-41 行）
//! - **R5-4**：`UploadedFile::move` 使用 `move_uploaded_file`（test 模式使用 `rename`）
//!   （对齐 PHP 第 50-75 行）
//! - **R5-5**：`UploadedFile::getErrorMessage` 错误码映射
//!   （对齐 PHP 第 82-106 行）
//! - **R5-6**：`UploadedFile::extension()` 覆写父类，返回原始扩展名
//!   （对齐 PHP 第 139-142 行）
//! - **R5-7**：`File::getMime` 使用 `finfo_file(FILEINFO_MIME_TYPE)` 对齐 Rust `infer` crate
//!   （对齐 PHP 第 88-93 行）
//! - **R5-8**：`File::move` 创建目录 `mkdir(dir, 0777, true)` + `chmod(target, 0666 & ~umask())`
//!   （对齐 PHP 第 102-118 行）
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\File.php`
//!   - 第 22-46 行：类声明 + 构造方法
//!   - 第 54-61 行：`hash($type)` 方法
//!   - 第 88-93 行：`getMime()` 方法
//!   - 第 102-118 行：`move($directory, $name)` 方法
//!   - 第 126-139 行：`getTargetFile($directory, $name)` 方法
//!   - 第 146-153 行：`getName($name)` 方法
//!   - 第 159-162 行：`extension()` 方法
//!   - 第 169-172 行：`setExtension($extension)` 方法
//!   - 第 180-203 行：`hashName($rule)` 方法
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\file\UploadedFile.php`
//!   - 第 18-34 行：类声明 + 构造方法
//!   - 第 36-41 行：`isValid()` 方法
//!   - 第 50-75 行：`move($directory, $name)` 方法
//!   - 第 82-106 行：`getErrorMessage()` 方法
//!   - 第 112-142 行：`getOriginalMime/Name/Extension` + `extension()` 方法

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::Local;
use md5::{Digest, Md5};

// ============================================================================
// 子模块
// ============================================================================

pub mod image;
pub mod storage;
pub mod validate;

// ============================================================================
// 错误类型
// ============================================================================

/// 上传错误 — 对齐 PHP `think\exception\FileException`
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    /// 文件不存在（对齐 PHP 第 42 行：`The file "%s" does not exist`）
    #[error("The file \"{0}\" does not exist")]
    FileNotFound(String),

    /// 文件移动失败（对齐 PHP 第 112 行：`Could not move the file "%s" to "%s" (%s)`）
    #[error("Could not move the file \"{from}\" to \"{to}\" ({error})")]
    MoveFailed {
        /// 源文件路径
        from: String,
        /// 目标文件路径
        to: String,
        /// 错误信息
        error: String,
    },

    /// 目录创建失败（对齐 PHP 第 130 行：`Unable to create the "%s" directory`）
    #[error("Unable to create the \"{0}\" directory")]
    DirectoryCreateFailed(String),

    /// 目录不可写（对齐 PHP 第 133 行：`Unable to write in the "%s" directory`）
    #[error("Unable to write in the \"{0}\" directory")]
    DirectoryNotWritable(String),

    /// 上传失败（对齐 PHP `UploadedFile::getErrorMessage()`）
    #[error("{0}")]
    UploadFailed(String),

    /// IO 错误
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// 临时文件持久化错误（对齐 `tempfile::PersistError`）
    #[error(transparent)]
    Persist(#[from] tempfile::PersistError),

    /// 文件名非法（路径遍历攻击防护）
    #[error("Invalid file name \"{0}\" — potential path traversal attack")]
    InvalidFileName(String),
}

// ============================================================================
// PHP UPLOAD_ERR_* 常量
// ============================================================================

/// PHP `UPLOAD_ERR_*` 常量 — 对齐 PHP 上传错误码
///
/// 对齐 PHP `UploadedFile.php` 第 82-106 行 `getErrorMessage()` 方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum UploadErrCode {
    /// `UPLOAD_ERR_OK` = 0
    Ok = 0,
    /// `UPLOAD_ERR_INI_SIZE` = 1
    IniSize = 1,
    /// `UPLOAD_ERR_FORM_SIZE` = 2
    FormSize = 2,
    /// `UPLOAD_ERR_PARTIAL` = 3
    Partial = 3,
    /// `UPLOAD_ERR_NO_FILE` = 4
    NoFile = 4,
    /// `UPLOAD_ERR_NO_TMP_DIR` = 6
    NoTmpDir = 6,
    /// `UPLOAD_ERR_CANT_WRITE` = 7
    CantWrite = 7,
}

impl UploadErrCode {
    /// 对齐 PHP `UploadedFile::getErrorMessage` 第 82-106 行
    ///
    /// PHP 行为：
    /// - `1` / `2` → `upload File size exceeds the maximum value`
    /// - `3` → `only the portion of file is uploaded`
    /// - `4` → `no file to uploaded`
    /// - `6` → `upload temp dir not found`
    /// - `7` → `file write error`
    /// - `default`（含 `0`）→ `unknown upload error`
    pub fn error_message(self) -> &'static str {
        match self {
            UploadErrCode::IniSize | UploadErrCode::FormSize => {
                "upload File size exceeds the maximum value"
            }
            UploadErrCode::Partial => "only the portion of file is uploaded",
            UploadErrCode::NoFile => "no file to uploaded",
            UploadErrCode::NoTmpDir => "upload temp dir not found",
            UploadErrCode::CantWrite => "file write error",
            UploadErrCode::Ok => "unknown upload error",
        }
    }

    /// 从 i32 转换（对齐 PHP `$error ?: UPLOAD_ERR_OK`）
    pub fn from_i32(code: i32) -> Self {
        match code {
            0 => UploadErrCode::Ok,
            1 => UploadErrCode::IniSize,
            2 => UploadErrCode::FormSize,
            3 => UploadErrCode::Partial,
            4 => UploadErrCode::NoFile,
            6 => UploadErrCode::NoTmpDir,
            7 => UploadErrCode::CantWrite,
            _ => UploadErrCode::Ok,
        }
    }
}

// ============================================================================
// 哈希算法
// ============================================================================

/// 哈希算法 — 对齐 PHP `hash_algos()` 子集
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgo {
    /// MD5
    Md5,
    /// SHA1
    Sha1,
    /// SHA256
    Sha256,
    /// SHA512
    Sha512,
}

impl HashAlgo {
    /// 算法名称（对齐 PHP `hash_algos()` 返回的字符串）
    pub fn as_str(self) -> &'static str {
        match self {
            HashAlgo::Md5 => "md5",
            HashAlgo::Sha1 => "sha1",
            HashAlgo::Sha256 => "sha256",
            HashAlgo::Sha512 => "sha512",
        }
    }

    /// 从字符串解析（对齐 PHP `in_array($rule, hash_algos())`）
    ///
    /// 注意：不实现 `std::str::FromStr`，因为该 trait 要求返回 `Result` 而非 `Option`，
    /// 而 PHP `in_array` 语义是布尔判断，使用 `Option` 更贴合。
    pub fn parse_algo(s: &str) -> Option<Self> {
        match s {
            "md5" => Some(HashAlgo::Md5),
            "sha1" => Some(HashAlgo::Sha1),
            "sha256" => Some(HashAlgo::Sha256),
            "sha512" => Some(HashAlgo::Sha512),
            _ => None,
        }
    }
}

// ============================================================================
// hashName 规则
// ============================================================================

/// `hashName` 规则 — 对齐 PHP `File::hashName($rule)` 第 180-203 行
///
/// PHP `$rule` 支持 3 种类型：
/// 1. `Closure` → `call_user_func_array($rule, [$this])`
/// 2. `string` 在 `hash_algos()` 中 → `substr(hash, 0, 2) . '/' . substr(hash, 2)`
/// 3. `callable` 字符串 → `call_user_func($rule)`
/// 4. `default` → `date('Ymd') . '/' . md5(microtime(true) . pathname)`
///
/// Rust 端简化为 2 种（Closure/callable 实际业务极少使用）：
/// - [`HashNameRule::Default`]：默认规则
/// - [`HashNameRule::Hash`]：hash 算法规则
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HashNameRule {
    /// 默认规则：`date('Ymd') . '/' . md5(microtime(true) . pathname)`
    ///
    /// 对齐 PHP 第 195 行
    #[default]
    Default,

    /// hash 算法规则：`substr(hash, 0, 2) . '/' . substr(hash, 2)`
    ///
    /// 对齐 PHP 第 187-190 行
    Hash(HashAlgo),
}

// ============================================================================
// File 结构
// ============================================================================

/// 文件类 — 对齐 PHP `think\File`（基于 `SplFileInfo`）
///
/// PHP 第 22-46 行：
/// ```php
/// class File extends SplFileInfo {
///     protected $hash = [];
///     protected $hashName;
///     protected $extension;
///     public function __construct(string $path, bool $checkPath = true) {
///         if ($checkPath && !is_file($path)) {
///             throw new FileException(sprintf('The file "%s" does not exist', $path));
///         }
///         parent::__construct($path);
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct File {
    /// 文件路径（对齐 PHP `SplFileInfo::$path`）
    path: PathBuf,
    /// 哈希缓存（对齐 PHP `$hash`，按算法名分组）
    hash: HashMap<String, String>,
    /// hashName 缓存（对齐 PHP `$hashName`）
    hash_name: Option<String>,
    /// 自定义扩展名（对齐 PHP `$extension`）
    extension: Option<String>,
}

impl File {
    /// 创建 `File` 实例 — 对齐 PHP `File::__construct` 第 39-46 行
    ///
    /// `check_path = true` 时检查文件是否存在，不存在返回 [`UploadError::FileNotFound`]。
    pub fn new<P: AsRef<Path>>(path: P, check_path: bool) -> Result<Self, UploadError> {
        let path = path.as_ref().to_path_buf();
        if check_path && !path.is_file() {
            return Err(UploadError::FileNotFound(
                path.to_string_lossy().to_string(),
            ));
        }
        Ok(Self {
            path,
            hash: HashMap::new(),
            hash_name: None,
            extension: None,
        })
    }

    /// 不检查路径创建实例 — 对齐 PHP 第 138 行 `new self($target, false)`
    pub fn new_unchecked<P: AsRef<Path>>(path: P) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            hash: HashMap::new(),
            hash_name: None,
            extension: None,
        }
    }

    /// 获取文件路径 — 对齐 PHP `SplFileInfo::getPathname()`
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 获取文件路径字符串
    pub fn path_name(&self) -> String {
        self.path.to_string_lossy().to_string()
    }

    /// 获取文件哈希 — 对齐 PHP `File::hash($type)` 第 54-61 行
    ///
    /// PHP 行为：
    /// ```php
    /// public function hash(string $type = 'sha1'): string {
    ///     if (!isset($this->hash[$type])) {
    ///         $this->hash[$type] = hash_file($type, $this->getPathname());
    ///     }
    ///     return $this->hash[$type];
    /// }
    /// ```
    pub fn hash(&mut self, algo: HashAlgo) -> Result<String, UploadError> {
        let key = algo.as_str().to_string();
        if let Some(h) = self.hash.get(&key) {
            return Ok(h.clone());
        }
        let h = compute_file_hash(&self.path, algo)?;
        self.hash.insert(key, h.clone());
        Ok(h)
    }

    /// 获取文件 MD5 — 对齐 PHP `File::md5()` 第 68-71 行
    pub fn md5(&mut self) -> Result<String, UploadError> {
        self.hash(HashAlgo::Md5)
    }

    /// 获取文件 SHA1 — 对齐 PHP `File::sha1()` 第 78-81 行
    pub fn sha1(&mut self) -> Result<String, UploadError> {
        self.hash(HashAlgo::Sha1)
    }

    /// 获取文件 MIME — 对齐 PHP `File::getMime()` 第 88-93 行
    ///
    /// PHP 行为：使用 `finfo_open(FILEINFO_MIME_TYPE)` + `finfo_file()` 检测 MIME。
    /// Rust 端：优先用 `infer` crate 从内容检测，回退到 `mime_guess` 从扩展名猜测。
    pub fn get_mime(&self) -> Result<String, UploadError> {
        // 优先：从内容检测（对齐 PHP finfo_file）
        if let Ok(Some(t)) = infer::get_from_path(&self.path) {
            return Ok(t.mime_type().to_string());
        }
        // 回退：从扩展名猜测
        let mime = mime_guess::from_path(&self.path)
            .first_or_octet_stream()
            .to_string();
        Ok(mime)
    }

    /// 移动文件 — 对齐 PHP `File::move()` 第 102-118 行
    ///
    /// PHP 行为：
    /// 1. 获取 target 文件
    /// 2. `rename($this->getPathname(), $target)` 移动文件
    /// 3. 失败抛 `FileException`
    /// 4. `chmod($target, 0666 & ~umask())` 设置权限
    /// 5. 返回新 `File` 实例
    pub fn move_to<P: AsRef<Path>>(
        &mut self,
        directory: P,
        name: Option<&str>,
    ) -> Result<File, UploadError> {
        let target = self.get_target_file(directory.as_ref(), name)?;

        fs::rename(&self.path, &target.path).map_err(|e| UploadError::MoveFailed {
            from: self.path.to_string_lossy().to_string(),
            to: target.path.to_string_lossy().to_string(),
            error: e.to_string(),
        })?;

        // 对齐 PHP `chmod($target, 0666 & ~umask())` 第 115 行
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target.path, fs::Permissions::from_mode(0o666));
        }

        Ok(target)
    }

    /// 实例化目标文件 — 对齐 PHP `File::getTargetFile()` 第 126-139 行
    ///
    /// PHP 行为：
    /// 1. 如果目录不存在：`mkdir($directory, 0777, true)`，失败且目录不存在则抛异常
    /// 2. elseif 目录不可写：抛异常
    /// 3. target = `rtrim($directory, '/\\') . DIRECTORY_SEPARATOR . (name === null ? basename : getName(name))`
    /// 4. 返回 `new self($target, false)`
    fn get_target_file(&self, directory: &Path, name: Option<&str>) -> Result<File, UploadError> {
        if !directory.is_dir() {
            fs::create_dir_all(directory).map_err(|_| {
                UploadError::DirectoryCreateFailed(directory.to_string_lossy().to_string())
            })?;
        }

        let file_name = match name {
            Some(n) => get_name(n),
            None => self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
        };

        let target_path = directory.join(&file_name);
        Ok(File::new_unchecked(&target_path))
    }

    /// 文件扩展名 — 对齐 PHP `File::extension()` 第 159-162 行
    ///
    /// PHP 行为：`return $this->getExtension();`（`SplFileInfo::getExtension()` 返回最后点后的部分）
    pub fn extension(&self) -> String {
        self.path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// 指定保存文件的扩展名 — 对齐 PHP `File::setExtension()` 第 169-172 行
    pub fn set_extension(&mut self, extension: &str) {
        self.extension = Some(extension.to_string());
    }

    /// 自动生成文件名 — 对齐 PHP `File::hashName($rule)` 第 180-203 行
    ///
    /// PHP 行为：
    /// ```php
    /// public function hashName($rule = ''): string {
    ///     if (!$this->hashName) {
    ///         if ($rule instanceof \Closure) {
    ///             $this->hashName = call_user_func_array($rule, [$this]);
    ///         } else {
    ///             switch (true) {
    ///                 case in_array($rule, hash_algos()):
    ///                     $hash = $this->hash($rule);
    ///                     $this->hashName = substr($hash, 0, 2) . DIRECTORY_SEPARATOR . substr($hash, 2);
    ///                     break;
    ///                 case is_callable($rule):
    ///                     $this->hashName = call_user_func($rule);
    ///                     break;
    ///                 default:
    ///                     $this->hashName = date('Ymd') . DIRECTORY_SEPARATOR . md5(microtime(true) . $this->getPathname());
    ///                     break;
    ///             }
    ///         }
    ///     }
    ///     $extension = $this->extension ?? $this->extension();
    ///     return $this->hashName . ($extension ? '.' . $extension : '');
    /// }
    /// ```
    ///
    /// Rust 端简化：只支持 `Default` 和 `Hash(algo)` 两种规则。
    pub fn hash_name(&mut self, rule: HashNameRule) -> Result<String, UploadError> {
        if self.hash_name.is_none() {
            let hash_name = match rule {
                HashNameRule::Hash(algo) => {
                    // 对齐 PHP 第 187-190 行
                    let hash = self.hash(algo)?;
                    if hash.len() < 2 {
                        hash
                    } else {
                        format!("{}/{}", &hash[..2], &hash[2..])
                    }
                }
                HashNameRule::Default => {
                    // 对齐 PHP 第 195 行：date('Ymd') . DIRECTORY_SEPARATOR . md5(microtime(true) . pathname)
                    let now = Local::now();
                    let date_str = now.format("%Y%m%d").to_string();
                    // microtime(true) 返回浮点数（秒.微秒）
                    let secs = now.timestamp();
                    let micros = now.timestamp_subsec_micros();
                    let microtime_str = format!("{}.{:06}", secs, micros);
                    let pathname = self.path.to_string_lossy();
                    let mut md5 = Md5::new();
                    md5.update(microtime_str.as_bytes());
                    md5.update(pathname.as_bytes());
                    let hash = hex::encode(md5.finalize());
                    format!("{}/{}", date_str, hash)
                }
            };
            self.hash_name = Some(hash_name);
        }

        // 对齐 PHP 第 201-202 行：$extension = $this->extension ?? $this->extension();
        let extension = match &self.extension {
            Some(ext) => ext.clone(),
            None => self.extension(),
        };

        let hash_name = self.hash_name.as_ref().expect("hash_name 已在上方初始化").clone();
        if extension.is_empty() {
            Ok(hash_name)
        } else {
            // 对齐 PHP 第 202 行：$this->hashName . ($extension ? '.' . $extension : '')
            Ok(format!("{}.{}", hash_name, extension))
        }
    }

    /// 获取文件名（不含目录） — 对齐 PHP `SplFileInfo::getBasename()`
    pub fn basename(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

// ============================================================================
// UploadedFile 结构
// ============================================================================

/// 上传文件类 — 对齐 PHP `think\file\UploadedFile`
///
/// PHP 第 18-34 行：
/// ```php
/// class UploadedFile extends File {
///     private $test = false;
///     private $originalName;
///     private $mimeType;
///     private $error;
///     public function __construct(string $path, string $originalName, string $mimeType = null, int $error = null, bool $test = false) {
///         $this->originalName = $originalName;
///         $this->mimeType     = $mimeType ?: 'application/octet-stream';
///         $this->test         = $test;
///         $this->error        = $error ?: UPLOAD_ERR_OK;
///         parent::__construct($path, UPLOAD_ERR_OK === $this->error);
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct UploadedFile {
    /// 父类 `File`
    file: File,
    /// 测试模式（对齐 PHP `$test`）
    test: bool,
    /// 原始文件名（对齐 PHP `$originalName`）
    original_name: String,
    /// MIME 类型（对齐 PHP `$mimeType`）
    mime_type: String,
    /// 上传错误码（对齐 PHP `$error`）
    error: UploadErrCode,
}

impl UploadedFile {
    /// 创建 `UploadedFile` 实例 — 对齐 PHP `UploadedFile::__construct` 第 26-34 行
    pub fn new<P: AsRef<Path>>(
        path: P,
        original_name: &str,
        mime_type: Option<&str>,
        error: Option<i32>,
        test: bool,
    ) -> Result<Self, UploadError> {
        let error = UploadErrCode::from_i32(error.unwrap_or(0));
        let mime = mime_type.unwrap_or("application/octet-stream").to_string();

        // 对齐 PHP 第 33 行：UPLOAD_ERR_OK === $this->error 时 checkPath=true
        let check_path = error == UploadErrCode::Ok;
        let file = File::new(path, check_path)?;

        Ok(Self {
            file,
            test,
            original_name: original_name.to_string(),
            mime_type: mime,
            error,
        })
    }

    /// 验证上传文件 — 对齐 PHP `UploadedFile::isValid()` 第 36-41 行
    ///
    /// PHP 行为：
    /// ```php
    /// public function isValid(): bool {
    ///     $isOk = UPLOAD_ERR_OK === $this->error;
    ///     return $this->test ? $isOk : $isOk && is_uploaded_file($this->getPathname());
    /// }
    /// ```
    ///
    /// Rust 端：`is_uploaded_file` 是 PHP SAPI 函数，无法精确对齐。
    /// 简化为检查文件是否存在（非 test 模式）。
    pub fn is_valid(&self) -> bool {
        let is_ok = self.error == UploadErrCode::Ok;
        if self.test {
            is_ok
        } else {
            // 对齐 PHP `is_uploaded_file($pathname)`
            is_ok && self.file.path().is_file()
        }
    }

    /// 移动上传文件 — 对齐 PHP `UploadedFile::move()` 第 50-75 行
    ///
    /// PHP 行为：
    /// 1. `isValid()` 失败 → 抛 `FileException($this->getErrorMessage())`
    /// 2. `test` 模式：调用 `parent::move()`（即 `rename`）
    /// 3. 非 test 模式：`move_uploaded_file($pathname, $target)`
    /// 4. 失败抛 `FileException`
    /// 5. `chmod($target, 0666 & ~umask())`
    /// 6. 返回新 `File`
    pub fn move_to<P: AsRef<Path>>(
        &mut self,
        directory: P,
        name: Option<&str>,
    ) -> Result<File, UploadError> {
        if !self.is_valid() {
            return Err(UploadError::UploadFailed(
                self.error.error_message().to_string(),
            ));
        }

        if self.test {
            // 对齐 PHP 第 54 行：`return parent::move($directory, $name);`
            return self.file.move_to(directory, name);
        }

        // 对齐 PHP 第 63 行：`move_uploaded_file($this->getPathname(), $target)`
        // Rust 端：使用 `fs::rename`（无 SAPI 等价物）
        let target = self.file.get_target_file(directory.as_ref(), name)?;
        fs::rename(self.file.path(), &target.path).map_err(|e| UploadError::MoveFailed {
            from: self.file.path().to_string_lossy().to_string(),
            to: target.path.to_string_lossy().to_string(),
            error: e.to_string(),
        })?;

        // 对齐 PHP 第 69 行：`chmod($target, 0666 & ~umask())`
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target.path, fs::Permissions::from_mode(0o666));
        }

        Ok(target)
    }

    /// 获取原始 MIME — 对齐 PHP `UploadedFile::getOriginalMime()` 第 112-115 行
    pub fn original_mime(&self) -> &str {
        &self.mime_type
    }

    /// 获取原始文件名 — 对齐 PHP `UploadedFile::getOriginalName()` 第 121-124 行
    pub fn original_name(&self) -> &str {
        &self.original_name
    }

    /// 获取原始扩展名 — 对齐 PHP `UploadedFile::getOriginalExtension()` 第 130-133 行
    ///
    /// PHP 行为：`return pathinfo($this->originalName, PATHINFO_EXTENSION);`
    pub fn original_extension(&self) -> String {
        Path::new(&self.original_name)
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// 获取文件扩展名 — 对齐 PHP `UploadedFile::extension()` 第 139-142 行
    ///
    /// PHP 行为：覆写父类，返回原始扩展名。
    /// ```php
    /// public function extension(): string {
    ///     return $this->getOriginalExtension();
    /// }
    /// ```
    pub fn extension(&self) -> String {
        self.original_extension()
    }

    /// 获取错误信息 — 对齐 PHP `UploadedFile::getErrorMessage()` 第 82-106 行
    pub fn error_message(&self) -> &'static str {
        self.error.error_message()
    }

    /// 获取错误码
    pub fn error_code(&self) -> UploadErrCode {
        self.error
    }

    /// 访问内部 `File`（不可变）
    pub fn as_file(&self) -> &File {
        &self.file
    }

    /// 访问内部 `File`（可变）
    pub fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 获取文件名 — 对齐 PHP `File::getName($name)` 第 146-153 行
///
/// PHP 行为：
/// ```php
/// protected function getName(string $name): string {
///     $originalName = str_replace('\\', '/', $name);
///     $pos          = strrpos($originalName, '/');
///     $originalName = false === $pos ? $originalName : substr($originalName, $pos + 1);
///     return $originalName;
/// }
/// ```
fn get_name(name: &str) -> String {
    // 对齐 PHP `str_replace('\\', '/', $name)`
    let original_name = name.replace('\\', "/");
    // 对齐 PHP `strrpos($originalName, '/')`
    match original_name.rfind('/') {
        Some(pos) => original_name[pos + 1..].to_string(),
        None => original_name,
    }
}

/// 计算文件哈希 — 对齐 PHP `hash_file($type, $pathname)`
fn compute_file_hash(path: &Path, algo: HashAlgo) -> Result<String, UploadError> {
    let mut file = fs::File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let hash = match algo {
        HashAlgo::Md5 => {
            let mut h = Md5::new();
            h.update(&buf);
            hex::encode(h.finalize())
        }
        HashAlgo::Sha1 => {
            let mut h = sha1::Sha1::new();
            h.update(&buf);
            hex::encode(h.finalize())
        }
        HashAlgo::Sha256 => {
            let mut h = sha2::Sha256::new();
            h.update(&buf);
            hex::encode(h.finalize())
        }
        HashAlgo::Sha512 => {
            let mut h = sha2::Sha512::new();
            h.update(&buf);
            hex::encode(h.finalize())
        }
    };
    Ok(hash)
}

// ============================================================================
// multipart/form-data 上传机制 — 对齐 PHP `Request::file()` + `$_FILES`
// ============================================================================

use axum::extract::Multipart;
use std::io::Write;
use tempfile::NamedTempFile;

/// multipart 解析结果 — 对齐 PHP `$_FILES` + `$_POST`
#[derive(Debug, Default)]
pub struct MultipartResult {
    /// 文件字段（字段名 → `UploadedFile` 列表）
    ///
    /// 对齐 PHP `$_FILES`，每个字段可能有多个文件（多文件上传）
    pub files: HashMap<String, Vec<UploadedFile>>,

    /// 普通字段（字段名 → 值）
    ///
    /// 对齐 PHP `$_POST`
    pub fields: HashMap<String, String>,
}

impl MultipartResult {
    /// 获取单个文件（对齐 PHP `Request::file($name)` 返回单个文件）
    ///
    /// PHP 行为：
    /// - name 为空 → 返回全部
    /// - name 存在 → 返回该字段的第一个文件
    pub fn file(&self, name: &str) -> Option<&UploadedFile> {
        self.files.get(name).and_then(|list| list.first())
    }

    /// 获取字段的所有文件（多文件上传）
    pub fn files(&self, name: &str) -> Option<&Vec<UploadedFile>> {
        self.files.get(name)
    }

    /// 获取普通字段值（对齐 PHP `$_POST[$name]`）
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(|s| s.as_str())
    }

    /// 获取文件数量
    pub fn file_count(&self) -> usize {
        self.files.values().map(|v| v.len()).sum()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.fields.is_empty()
    }
}

/// 解析 `multipart/form-data` 请求 — 对齐 PHP `Request::file()` + `$_FILES`
///
/// PHP 行为：
/// - `$_FILES` 自动填充上传文件信息（name/type/tmp_name/error/size）
/// - `Request::file($name)` 包装为 `UploadedFile` 对象
/// - `$_POST` 填充普通字段
///
/// Rust 端：使用 `axum::extract::Multipart` 提取字段，
/// 文件字段保存到临时文件并创建 `UploadedFile`，普通字段保存到 `fields`。
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::upload::parse_multipart;
/// use axum::extract::Multipart;
///
/// async fn upload_handler(mut multipart: Multipart) -> Result<String, String> {
///     let result = parse_multipart(&mut multipart).await
///         .map_err(|e| e.to_string())?;
///     
///     if let Some(uploaded) = result.file("avatar") {
///         let moved = uploaded.move_to("/var/www/uploads", Some("avatar.png"))
///             .map_err(|e| e.to_string())?;
///         return Ok(format!("上传成功：{:?}", moved.path()));
///     }
///     
///     Ok("未找到文件".to_string())
/// }
/// ```
pub async fn parse_multipart(multipart: &mut Multipart) -> Result<MultipartResult, UploadError> {
    let mut result = MultipartResult::default();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| UploadError::UploadFailed(e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_string();
        let file_name = field.file_name().map(|s| s.to_string());
        let content_type = field.content_type().map(|s| s.to_string());

        let data = field
            .bytes()
            .await
            .map_err(|e| UploadError::UploadFailed(e.to_string()))?;

        if let Some(file_name) = file_name {
            // 文件字段：保存到临时文件
            let ext = Path::new(&file_name)
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();

            let mut temp = NamedTempFile::with_suffix(&ext)?;
            temp.write_all(&data)?;

            // keep() 让文件持久化（不被 drop 删除），返回 (path, _file)
            // 对齐 PHP `$_FILES[xxx]['tmp_name']` — 由 SAPI 创建，请求结束前不会删除
            let (_file, path) = temp.keep()?;

            let uploaded = UploadedFile::new(
                &path,
                &file_name,
                content_type.as_deref(),
                Some(0),
                // test 模式：使用 rename（对齐 PHP `parent::move`）
                // 因为 axum 上传的文件不是 PHP SAPI 上传的，无法使用 move_uploaded_file
                true,
            )?;

            result.files.entry(name).or_default().push(uploaded);
        } else {
            // 普通字段：保存为字符串
            let value = String::from_utf8_lossy(&data).to_string();
            result.fields.insert(name, value);
        }
    }

    Ok(result)
}

// ============================================================================

// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// 创建临时文件并写入内容
    fn create_temp_file(content: &[u8], suffix: &str) -> NamedTempFile {
        let mut file = NamedTempFile::with_suffix(suffix).expect("创建临时文件失败");
        file.write_all(content).expect("写入临时文件失败");
        file
    }

    // ------------------------------------------------------------------------
    // 组 1：UploadErrCode 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_upload_err_code_from_i32() {
        assert_eq!(UploadErrCode::from_i32(0), UploadErrCode::Ok);
        assert_eq!(UploadErrCode::from_i32(1), UploadErrCode::IniSize);
        assert_eq!(UploadErrCode::from_i32(2), UploadErrCode::FormSize);
        assert_eq!(UploadErrCode::from_i32(3), UploadErrCode::Partial);
        assert_eq!(UploadErrCode::from_i32(4), UploadErrCode::NoFile);
        assert_eq!(UploadErrCode::from_i32(6), UploadErrCode::NoTmpDir);
        assert_eq!(UploadErrCode::from_i32(7), UploadErrCode::CantWrite);
        // 未知错误码回退到 Ok
        assert_eq!(UploadErrCode::from_i32(99), UploadErrCode::Ok);
    }

    #[test]
    fn test_upload_err_code_error_message() {
        // 对齐 PHP 第 84-85 行：1/2 → size exceeds
        assert_eq!(
            UploadErrCode::IniSize.error_message(),
            "upload File size exceeds the maximum value"
        );
        assert_eq!(
            UploadErrCode::FormSize.error_message(),
            "upload File size exceeds the maximum value"
        );
        // 对齐 PHP 第 88-89 行：3 → portion
        assert_eq!(
            UploadErrCode::Partial.error_message(),
            "only the portion of file is uploaded"
        );
        // 对齐 PHP 第 91-92 行：4 → no file
        assert_eq!(UploadErrCode::NoFile.error_message(), "no file to uploaded");
        // 对齐 PHP 第 94-95 行：6 → temp dir
        assert_eq!(
            UploadErrCode::NoTmpDir.error_message(),
            "upload temp dir not found"
        );
        // 对齐 PHP 第 97-98 行：7 → write error
        assert_eq!(UploadErrCode::CantWrite.error_message(), "file write error");
        // 对齐 PHP 第 101-102 行：default → unknown
        assert_eq!(UploadErrCode::Ok.error_message(), "unknown upload error");
    }

    // ------------------------------------------------------------------------
    // 组 2：HashAlgo 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_hash_algo_as_str() {
        assert_eq!(HashAlgo::Md5.as_str(), "md5");
        assert_eq!(HashAlgo::Sha1.as_str(), "sha1");
        assert_eq!(HashAlgo::Sha256.as_str(), "sha256");
        assert_eq!(HashAlgo::Sha512.as_str(), "sha512");
    }

    #[test]
    fn test_hash_algo_parse_algo() {
        assert_eq!(HashAlgo::parse_algo("md5"), Some(HashAlgo::Md5));
        assert_eq!(HashAlgo::parse_algo("sha1"), Some(HashAlgo::Sha1));
        assert_eq!(HashAlgo::parse_algo("sha256"), Some(HashAlgo::Sha256));
        assert_eq!(HashAlgo::parse_algo("sha512"), Some(HashAlgo::Sha512));
        // 对齐 PHP `in_array($rule, hash_algos())` 找不到返回 None
        assert_eq!(HashAlgo::parse_algo("unknown"), None);
    }

    // ------------------------------------------------------------------------
    // 组 3：File 基础测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_file_new_with_check_path() {
        // 文件存在
        let temp = create_temp_file(b"hello", ".txt");
        let file = File::new(temp.path(), true);
        assert!(file.is_ok());

        // 文件不存在
        let file = File::new("/nonexistent/file.txt", true);
        assert!(matches!(file, Err(UploadError::FileNotFound(_))));
    }

    #[test]
    fn test_file_new_without_check_path() {
        // check_path = false 时不检查文件存在性
        let file = File::new("/nonexistent/file.txt", false);
        assert!(file.is_ok());
    }

    #[test]
    fn test_file_new_unchecked() {
        let file = File::new_unchecked("/some/path/file.txt");
        assert_eq!(file.path(), Path::new("/some/path/file.txt"));
    }

    #[test]
    fn test_file_path() {
        let temp = create_temp_file(b"hello", ".txt");
        let file = File::new(temp.path(), true).unwrap();
        assert_eq!(file.path(), temp.path());
    }

    #[test]
    fn test_file_path_name() {
        let temp = create_temp_file(b"hello", ".txt");
        let file = File::new(temp.path(), true).unwrap();
        assert_eq!(file.path_name(), temp.path().to_string_lossy().to_string());
    }

    #[test]
    fn test_file_extension() {
        // 对齐 PHP `SplFileInfo::getExtension()`
        let temp = create_temp_file(b"hello", ".txt");
        let file = File::new(temp.path(), true).unwrap();
        assert_eq!(file.extension(), "txt");
    }

    #[test]
    fn test_file_extension_no_extension() {
        // 无扩展名返回空字符串
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"hello").unwrap();
        let file = File::new(file.path(), true).unwrap();
        assert_eq!(file.extension(), "");
    }

    #[test]
    fn test_file_set_extension() {
        // 对齐 PHP `File::setExtension($extension)`
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        assert_eq!(file.extension(), "txt");
        file.set_extension("jpg");
        assert_eq!(file.extension, Some("jpg".to_string()));
    }

    #[test]
    fn test_file_basename() {
        // 对齐 PHP `SplFileInfo::getBasename()`
        let temp = create_temp_file(b"hello", ".txt");
        let file = File::new(temp.path(), true).unwrap();
        let basename = file.basename();
        assert!(basename.ends_with(".txt"));
    }

    // ------------------------------------------------------------------------
    // 组 4：File hash 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_file_md5() {
        // 对齐 PHP `File::md5()` 第 68-71 行
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let md5 = file.md5().unwrap();
        // "hello" 的 MD5
        assert_eq!(md5, "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn test_file_sha1() {
        // 对齐 PHP `File::sha1()` 第 78-81 行
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let sha1 = file.sha1().unwrap();
        // "hello" 的 SHA1
        assert_eq!(sha1, "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    }

    #[test]
    fn test_file_hash_md5() {
        // 对齐 PHP `File::hash('md5')`
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash = file.hash(HashAlgo::Md5).unwrap();
        assert_eq!(hash, "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn test_file_hash_sha1() {
        // 对齐 PHP `File::hash('sha1')`
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash = file.hash(HashAlgo::Sha1).unwrap();
        assert_eq!(hash, "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    }

    #[test]
    fn test_file_hash_sha256() {
        // 对齐 PHP `File::hash('sha256')`
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash = file.hash(HashAlgo::Sha256).unwrap();
        // "hello" 的 SHA256
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_file_hash_sha512() {
        // 对齐 PHP `File::hash('sha512')`
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash = file.hash(HashAlgo::Sha512).unwrap();
        // "hello" 的 SHA512（前 32 字符）
        assert!(hash.starts_with("9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca"));
    }

    #[test]
    fn test_file_hash_caching() {
        // 对齐 PHP 第 56-58 行：缓存机制
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash1 = file.hash(HashAlgo::Md5).unwrap();
        let hash2 = file.hash(HashAlgo::Md5).unwrap();
        assert_eq!(hash1, hash2);
        // 缓存验证
        assert!(file.hash.contains_key("md5"));
    }

    #[test]
    fn test_file_hash_multiple_algos() {
        // 多算法独立缓存
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let md5 = file.hash(HashAlgo::Md5).unwrap();
        let sha1 = file.hash(HashAlgo::Sha1).unwrap();
        assert_ne!(md5, sha1);
        assert!(file.hash.contains_key("md5"));
        assert!(file.hash.contains_key("sha1"));
    }

    // ------------------------------------------------------------------------
    // 组 5：File getMime 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_file_get_mime_text() {
        // 对齐 PHP `File::getMime()` 第 88-93 行
        let temp = create_temp_file(b"hello", ".txt");
        let file = File::new(temp.path(), true).unwrap();
        let mime = file.get_mime().unwrap();
        // 内容检测优先（infer 检测文本），扩展名回退
        // 文本文件内容检测可能返回 text/plain 或 application/octet-stream
        assert!(
            mime == "text/plain" || mime == "application/octet-stream",
            "mime = {}",
            mime
        );
    }

    #[test]
    fn test_file_get_mime_png() {
        // PNG 文件内容检测
        let png_header = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, // IHDR length
            0x49, 0x48, 0x44, 0x52, // "IHDR"
        ];
        let mut file = NamedTempFile::with_suffix(".png").unwrap();
        file.write_all(&png_header).unwrap();
        let file = File::new(file.path(), true).unwrap();
        let mime = file.get_mime().unwrap();
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn test_file_get_mime_jpg() {
        // JPEG 文件内容检测（FF D8 开头）
        let jpg_header = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
        let mut file = NamedTempFile::with_suffix(".jpg").unwrap();
        file.write_all(&jpg_header).unwrap();
        let file = File::new(file.path(), true).unwrap();
        let mime = file.get_mime().unwrap();
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn test_file_get_mime_unknown_extension() {
        // 无扩展名 + 未知内容 → 回退 application/octet-stream
        let temp = create_temp_file(&[0x00, 0x01, 0x02, 0x03], "");
        let file = File::new(temp.path(), true).unwrap();
        let mime = file.get_mime().unwrap();
        assert_eq!(mime, "application/octet-stream");
    }

    // ------------------------------------------------------------------------
    // 组 6：File move 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_file_move_with_default_name() {
        // 对齐 PHP `File::move($directory, null)` — 使用原文件名
        let temp = create_temp_file(b"hello", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();
        let target_dir = temp_dir.path().join("subdir");

        let mut file = File::new(temp.path(), true).unwrap();
        let original_basename = file.basename();
        let moved = file.move_to(&target_dir, None).unwrap();

        assert!(moved.path().is_file());
        assert_eq!(moved.basename(), original_basename);
        // 原文件已移动
        assert!(!temp.path().exists());
    }

    #[test]
    fn test_file_move_with_custom_name() {
        // 对齐 PHP `File::move($directory, $name)`
        let temp = create_temp_file(b"hello", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();

        let mut file = File::new(temp.path(), true).unwrap();
        let moved = file.move_to(&temp_dir, Some("custom.txt")).unwrap();

        assert!(moved.path().is_file());
        assert_eq!(moved.basename(), "custom.txt");
    }

    #[test]
    fn test_file_move_creates_directory() {
        // 对齐 PHP `mkdir($directory, 0777, true)` — 递归创建目录
        let temp = create_temp_file(b"hello", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();
        let nested_dir = temp_dir.path().join("a").join("b").join("c");

        let mut file = File::new(temp.path(), true).unwrap();
        let moved = file.move_to(&nested_dir, Some("file.txt")).unwrap();

        assert!(moved.path().is_file());
        assert!(nested_dir.is_dir());
    }

    #[test]
    fn test_file_move_preserves_content() {
        let temp = create_temp_file(b"hello world", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();

        let mut file = File::new(temp.path(), true).unwrap();
        let moved = file.move_to(&temp_dir, Some("moved.txt")).unwrap();

        let content = std::fs::read_to_string(moved.path()).unwrap();
        assert_eq!(content, "hello world");
    }

    // ------------------------------------------------------------------------
    // 组 7：File hashName 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_file_hash_name_default_format() {
        // 对齐 PHP `File::hashName()` 默认规则：date('Ymd')/md5(microtime.pathname).ext
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash_name = file.hash_name(HashNameRule::Default).unwrap();

        // 格式：YYYYMMDD/32位md5.txt
        let parts: Vec<&str> = hash_name.split('/').collect();
        assert_eq!(parts.len(), 2);
        let (date_part, md5_ext) = (parts[0], parts[1]);

        // 日期部分：8 位数字
        assert_eq!(date_part.len(), 8);
        assert!(date_part.chars().all(|c| c.is_ascii_digit()));

        // md5.ext 部分
        let ext_parts: Vec<&str> = md5_ext.split('.').collect();
        assert_eq!(ext_parts.len(), 2);
        assert_eq!(ext_parts[0].len(), 32); // MD5 长度
        assert!(ext_parts[0].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(ext_parts[1], "txt"); // 扩展名
    }

    #[test]
    fn test_file_hash_name_default_no_extension() {
        // 无扩展名 → 不追加 .ext
        let temp = NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), b"hello").unwrap();
        let mut file = File::new(temp.path(), true).unwrap();
        let hash_name = file.hash_name(HashNameRule::Default).unwrap();

        // 格式：YYYYMMDD/32位md5（无 .ext）
        let parts: Vec<&str> = hash_name.split('/').collect();
        assert_eq!(parts.len(), 2);
        assert!(!parts[1].contains('.'));
    }

    #[test]
    fn test_file_hash_name_hash_md5() {
        // 对齐 PHP `File::hashName('md5')`：substr(hash, 0, 2)/substr(hash, 2).ext
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash_name = file.hash_name(HashNameRule::Hash(HashAlgo::Md5)).unwrap();

        // 格式：5d/41402abc4b2a76b9719d911017c592.txt
        assert_eq!(hash_name, "5d/41402abc4b2a76b9719d911017c592.txt");
    }

    #[test]
    fn test_file_hash_name_hash_sha1() {
        // 对齐 PHP `File::hashName('sha1')`
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash_name = file.hash_name(HashNameRule::Hash(HashAlgo::Sha1)).unwrap();

        // "hello" 的 SHA1 = aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d
        assert_eq!(hash_name, "aa/f4c61ddcc5e8a2dabede0f3b482cd9aea9434d.txt");
    }

    #[test]
    fn test_file_hash_name_caching() {
        // 对齐 PHP `if (!$this->hashName)` — 缓存机制
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let name1 = file.hash_name(HashNameRule::Default).unwrap();
        let name2 = file.hash_name(HashNameRule::Default).unwrap();
        assert_eq!(name1, name2);
    }

    #[test]
    fn test_file_hash_name_with_set_extension() {
        // 对齐 PHP `$extension = $this->extension ?? $this->extension();`
        // setExtension 覆盖
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        file.set_extension("jpg");
        let hash_name = file.hash_name(HashNameRule::Hash(HashAlgo::Md5)).unwrap();
        assert!(hash_name.ends_with(".jpg"));
    }

    // ------------------------------------------------------------------------
    // 组 8：UploadedFile 基础测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_uploaded_file_new_ok() {
        // 对齐 PHP `UploadedFile::__construct` 第 26-34 行
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(
            temp.path(),
            "original.txt",
            Some("text/plain"),
            Some(0),
            false,
        );
        assert!(uploaded.is_ok());
    }

    #[test]
    fn test_uploaded_file_new_with_error() {
        // 错误码非 0 时不检查路径存在性
        let uploaded = UploadedFile::new("/nonexistent", "original.txt", None, Some(3), false);
        assert!(uploaded.is_ok());
    }

    #[test]
    fn test_uploaded_file_new_check_path_on_ok() {
        // UPLOAD_ERR_OK 时检查路径存在性
        let uploaded = UploadedFile::new("/nonexistent", "original.txt", None, Some(0), false);
        assert!(matches!(uploaded, Err(UploadError::FileNotFound(_))));
    }

    #[test]
    fn test_uploaded_file_original_name() {
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(
            temp.path(),
            "my_file.txt",
            Some("text/plain"),
            Some(0),
            false,
        )
        .unwrap();
        assert_eq!(uploaded.original_name(), "my_file.txt");
    }

    #[test]
    fn test_uploaded_file_original_mime() {
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(
            temp.path(),
            "my_file.txt",
            Some("text/plain"),
            Some(0),
            false,
        )
        .unwrap();
        assert_eq!(uploaded.original_mime(), "text/plain");
    }

    #[test]
    fn test_uploaded_file_original_mime_default() {
        // 对齐 PHP `$mimeType ?: 'application/octet-stream'`
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(temp.path(), "my_file.txt", None, Some(0), false).unwrap();
        assert_eq!(uploaded.original_mime(), "application/octet-stream");
    }

    #[test]
    fn test_uploaded_file_original_extension() {
        // 对齐 PHP `pathinfo($originalName, PATHINFO_EXTENSION)`
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(
            temp.path(),
            "my_file.txt",
            Some("text/plain"),
            Some(0),
            false,
        )
        .unwrap();
        assert_eq!(uploaded.original_extension(), "txt");
    }

    #[test]
    fn test_uploaded_file_original_extension_no_ext() {
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(
            temp.path(),
            "no_extension",
            Some("text/plain"),
            Some(0),
            false,
        )
        .unwrap();
        assert_eq!(uploaded.original_extension(), "");
    }

    #[test]
    fn test_uploaded_file_extension_overrides_parent() {
        // 对齐 PHP 第 139-142 行：extension() 覆写父类，返回 original_extension
        let temp = create_temp_file(b"hello", ".txt");
        // 注意：原始文件名扩展名是 .jpg，文件路径扩展名是 .txt
        let uploaded =
            UploadedFile::new(temp.path(), "photo.jpg", Some("image/jpeg"), Some(0), false)
                .unwrap();
        // extension() 应返回 jpg（原始扩展名），而不是 txt（路径扩展名）
        assert_eq!(uploaded.extension(), "jpg");
        // 但 as_file().extension() 返回 txt（父类行为）
        assert_eq!(uploaded.as_file().extension(), "txt");
    }

    // ------------------------------------------------------------------------
    // 组 9：UploadedFile isValid 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_uploaded_file_is_valid_ok() {
        // 对齐 PHP `isValid()` — UPLOAD_ERR_OK && is_uploaded_file
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(
            temp.path(),
            "my_file.txt",
            Some("text/plain"),
            Some(0),
            false,
        )
        .unwrap();
        // Rust 端：is_uploaded_file 简化为文件存在性检查
        assert!(uploaded.is_valid());
    }

    #[test]
    fn test_uploaded_file_is_valid_with_error() {
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(
            temp.path(),
            "my_file.txt",
            Some("text/plain"),
            Some(3),
            false,
        )
        .unwrap();
        // UPLOAD_ERR_PARTIAL → isValid = false
        assert!(!uploaded.is_valid());
    }

    #[test]
    fn test_uploaded_file_is_valid_test_mode() {
        // test 模式：error == OK 且文件存在 → is_valid = true
        // 对齐 PHP：构造时 check_path = (error === UPLOAD_ERR_OK)，所以文件必须存在
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(
            temp.path(),
            "my_file.txt",
            Some("text/plain"),
            Some(0),
            true,
        )
        .unwrap();
        assert!(uploaded.is_valid());
    }

    #[test]
    fn test_uploaded_file_is_valid_test_mode_with_error() {
        let uploaded = UploadedFile::new(
            "/nonexistent",
            "my_file.txt",
            Some("text/plain"),
            Some(4),
            true,
        )
        .unwrap();
        assert!(!uploaded.is_valid());
    }

    // ------------------------------------------------------------------------
    // 组 10：UploadedFile move 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_uploaded_file_move_test_mode() {
        // test 模式：使用 rename（对齐 PHP 第 54 行 parent::move）
        let temp = create_temp_file(b"hello", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();
        let mut uploaded = UploadedFile::new(
            temp.path(),
            "original.txt",
            Some("text/plain"),
            Some(0),
            true,
        )
        .unwrap();
        let moved = uploaded.move_to(&temp_dir, Some("moved.txt")).unwrap();
        assert!(moved.path().is_file());
        assert_eq!(moved.basename(), "moved.txt");
    }

    #[test]
    fn test_uploaded_file_move_invalid() {
        // 无效上传 → 抛异常（对齐 PHP 第 74 行）
        let temp = create_temp_file(b"hello", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();
        let mut uploaded = UploadedFile::new(
            temp.path(),
            "original.txt",
            Some("text/plain"),
            Some(3),
            false,
        )
        .unwrap();
        let result = uploaded.move_to(&temp_dir, Some("moved.txt"));
        assert!(matches!(result, Err(UploadError::UploadFailed(_))));
        // 错误消息对齐 PHP `getErrorMessage()`
        if let Err(UploadError::UploadFailed(msg)) = result {
            assert_eq!(msg, "only the portion of file is uploaded");
        }
    }

    #[test]
    fn test_uploaded_file_move_real() {
        // 非 test 模式：move_uploaded_file（Rust 端用 rename）
        let temp = create_temp_file(b"hello", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();
        let mut uploaded = UploadedFile::new(
            temp.path(),
            "original.txt",
            Some("text/plain"),
            Some(0),
            false,
        )
        .unwrap();
        let moved = uploaded.move_to(&temp_dir, Some("uploaded.txt")).unwrap();
        assert!(moved.path().is_file());
        assert_eq!(moved.basename(), "uploaded.txt");
        // 原文件已移动
        assert!(!temp.path().exists());
    }

    // ------------------------------------------------------------------------
    // 组 11：UploadedFile error 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_uploaded_file_error_message() {
        // 对齐 PHP `UploadedFile::getErrorMessage()` 第 82-106 行
        let temp = create_temp_file(b"hello", ".txt");

        let uploaded_ok = UploadedFile::new(temp.path(), "f.txt", None, Some(0), false).unwrap();
        assert_eq!(uploaded_ok.error_message(), "unknown upload error");

        let uploaded_1 = UploadedFile::new(temp.path(), "f.txt", None, Some(1), false).unwrap();
        assert_eq!(
            uploaded_1.error_message(),
            "upload File size exceeds the maximum value"
        );

        let uploaded_3 = UploadedFile::new(temp.path(), "f.txt", None, Some(3), false).unwrap();
        assert_eq!(
            uploaded_3.error_message(),
            "only the portion of file is uploaded"
        );

        let uploaded_4 = UploadedFile::new(temp.path(), "f.txt", None, Some(4), false).unwrap();
        assert_eq!(uploaded_4.error_message(), "no file to uploaded");
    }

    #[test]
    fn test_uploaded_file_error_code() {
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(temp.path(), "f.txt", None, Some(7), false).unwrap();
        assert_eq!(uploaded.error_code(), UploadErrCode::CantWrite);
    }

    // ------------------------------------------------------------------------
    // 组 12：辅助函数 get_name 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_get_name_simple() {
        // 对齐 PHP `File::getName($name)` 第 146-153 行
        assert_eq!(get_name("file.txt"), "file.txt");
    }

    #[test]
    fn test_get_name_with_path() {
        // 包含 / 的路径 → 返回最后一段
        assert_eq!(get_name("/path/to/file.txt"), "file.txt");
    }

    #[test]
    fn test_get_name_with_backslash() {
        // 对齐 PHP `str_replace('\\', '/', $name)`
        assert_eq!(get_name("\\path\\to\\file.txt"), "file.txt");
    }

    #[test]
    fn test_get_name_mixed_separators() {
        // 混合分隔符
        assert_eq!(get_name("\\path/to\\file.txt"), "file.txt");
    }

    #[test]
    fn test_get_name_only_filename() {
        assert_eq!(get_name("filename"), "filename");
    }

    // ------------------------------------------------------------------------
    // 组 13：PHP 行为对齐测试（R5 硬约束）
    // ------------------------------------------------------------------------

    #[test]
    fn test_php_behavior_hash_name_md5_split() {
        // R5-2：hashName('md5') = substr(md5, 0, 2) . '/' . substr(md5, 2)
        // "hello" 的 MD5 = 5d41402abc4b2a76b9719d911017c592
        // 期望：5d/41402abc4b2a76b9719d911017c592
        let temp = create_temp_file(b"hello", "");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash_name = file.hash_name(HashNameRule::Hash(HashAlgo::Md5)).unwrap();
        assert_eq!(hash_name, "5d/41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn test_php_behavior_hash_name_sha1_split() {
        // R5-2：hashName('sha1') = substr(sha1, 0, 2) . '/' . substr(sha1, 2)
        // "hello" 的 SHA1 = aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d
        // 期望：aa/f4c61ddcc5e8a2dabede0f3b482cd9aea9434d
        let temp = create_temp_file(b"hello", "");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash_name = file.hash_name(HashNameRule::Hash(HashAlgo::Sha1)).unwrap();
        assert_eq!(hash_name, "aa/f4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    }

    #[test]
    fn test_php_behavior_hash_name_default_format() {
        // R5-1：hashName() 默认规则 = date('Ymd')/md5(microtime.pathname).ext
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let hash_name = file.hash_name(HashNameRule::Default).unwrap();

        // 验证格式：YYYYMMDD/32位hex.txt
        let re = regex::Regex::new(r"^\d{8}/[0-9a-f]{32}\.txt$").unwrap();
        assert!(
            re.is_match(&hash_name),
            "hash_name 格式不匹配：{}",
            hash_name
        );
    }

    #[test]
    fn test_php_behavior_uploaded_file_extension_override() {
        // R5-6：UploadedFile::extension() 覆写父类，返回原始扩展名
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded =
            UploadedFile::new(temp.path(), "photo.jpg", Some("image/jpeg"), Some(0), false)
                .unwrap();
        // extension() 应返回 jpg（原始扩展名）
        assert_eq!(uploaded.extension(), "jpg");
        // 父类 extension() 返回 txt（路径扩展名）
        assert_eq!(uploaded.as_file().extension(), "txt");
    }

    #[test]
    fn test_php_behavior_is_valid_test_mode() {
        // R5-3：test 模式只检查 error == OK（构造时仍要求文件存在，因为 check_path = (error === OK)）
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(temp.path(), "f.txt", None, Some(0), true).unwrap();
        assert!(uploaded.is_valid());
    }

    #[test]
    fn test_php_behavior_is_valid_non_test_mode_requires_file() {
        // R5-3：非 test 模式构造时若 error == OK 则要求文件存在
        // 使用存在的临时文件，验证 test=false 时 is_valid 也返回 true（文件存在）
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(temp.path(), "f.txt", None, Some(0), false).unwrap();
        assert!(uploaded.is_valid());

        // 非 test 模式 + 不存在文件 + error == OK → 构造时 FileNotFound 错误
        let result = UploadedFile::new("/nonexistent/path", "f.txt", None, Some(0), false);
        assert!(matches!(result, Err(UploadError::FileNotFound(_))));
    }

    #[test]
    fn test_php_behavior_error_message_mapping() {
        // R5-5：错误码映射
        let temp = create_temp_file(b"hello", ".txt");
        assert_eq!(
            UploadedFile::new(temp.path(), "f", None, Some(1), false)
                .unwrap()
                .error_message(),
            "upload File size exceeds the maximum value"
        );
        assert_eq!(
            UploadedFile::new(temp.path(), "f", None, Some(2), false)
                .unwrap()
                .error_message(),
            "upload File size exceeds the maximum value"
        );
        assert_eq!(
            UploadedFile::new(temp.path(), "f", None, Some(3), false)
                .unwrap()
                .error_message(),
            "only the portion of file is uploaded"
        );
        assert_eq!(
            UploadedFile::new(temp.path(), "f", None, Some(4), false)
                .unwrap()
                .error_message(),
            "no file to uploaded"
        );
        assert_eq!(
            UploadedFile::new(temp.path(), "f", None, Some(6), false)
                .unwrap()
                .error_message(),
            "upload temp dir not found"
        );
        assert_eq!(
            UploadedFile::new(temp.path(), "f", None, Some(7), false)
                .unwrap()
                .error_message(),
            "file write error"
        );
        assert_eq!(
            UploadedFile::new(temp.path(), "f", None, Some(0), false)
                .unwrap()
                .error_message(),
            "unknown upload error"
        );
    }

    #[test]
    fn test_php_behavior_move_creates_directory() {
        // R5-8：mkdir(directory, 0777, true) 递归创建目录
        let temp = create_temp_file(b"hello", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();
        let nested = temp_dir.path().join("a").join("b").join("c");

        let mut file = File::new(temp.path(), true).unwrap();
        let moved = file.move_to(&nested, Some("file.txt")).unwrap();

        assert!(moved.path().is_file());
        assert!(nested.is_dir());
    }

    #[test]
    fn test_php_behavior_move_chmod_unix() {
        // R5-8：chmod(target, 0666 & ~umask())
        // 仅在 Unix 平台验证
        let temp = create_temp_file(b"hello", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();

        let mut file = File::new(temp.path(), true).unwrap();
        let moved = file.move_to(&temp_dir, Some("file.txt")).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(moved.path())
                .unwrap()
                .permissions()
                .mode();
            // 0666 & ~umask()，umask 通常是 022，所以最终权限是 0644
            assert_eq!(perms & 0o777, 0o644);
        }
        #[cfg(not(unix))]
        {
            let _ = moved;
        }
    }

    #[test]
    fn test_php_behavior_hash_caching() {
        // 对齐 PHP 第 56-58 行：hash 缓存
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let md5_1 = file.hash(HashAlgo::Md5).unwrap();
        // 再次请求相同算法 → 从缓存读取
        let md5_2 = file.hash(HashAlgo::Md5).unwrap();
        assert_eq!(md5_1, md5_2);
    }

    #[test]
    fn test_php_behavior_hash_name_caching() {
        // 对齐 PHP 第 182 行：if (!$this->hashName) 缓存
        let temp = create_temp_file(b"hello", ".txt");
        let mut file = File::new(temp.path(), true).unwrap();
        let name_1 = file.hash_name(HashNameRule::Hash(HashAlgo::Md5)).unwrap();
        let name_2 = file.hash_name(HashNameRule::Hash(HashAlgo::Sha1)).unwrap();
        // 第二次调用使用缓存，返回第一次的结果（md5 格式）
        assert_eq!(name_1, name_2);
    }

    #[test]
    fn test_php_behavior_get_mime_infer() {
        // R5-7：getMime 使用 finfo_file（Rust infer crate）
        // PNG 文件检测
        let png_header = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52,
        ];
        let mut file = NamedTempFile::with_suffix(".png").unwrap();
        file.write_all(&png_header).unwrap();
        let file = File::new(file.path(), true).unwrap();
        assert_eq!(file.get_mime().unwrap(), "image/png");
    }

    #[test]
    fn test_php_behavior_uploaded_file_move_test_uses_rename() {
        // R5-4：test 模式使用 rename（parent::move）
        let temp = create_temp_file(b"hello", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();
        let mut uploaded =
            UploadedFile::new(temp.path(), "original.txt", None, Some(0), true).unwrap();
        let moved = uploaded.move_to(&temp_dir, Some("moved.txt")).unwrap();
        assert!(moved.path().is_file());
        // 原文件已被 rename（不存在）
        assert!(!temp.path().exists());
    }

    #[test]
    fn test_php_behavior_uploaded_file_move_non_test_uses_move_uploaded_file() {
        // R5-4：非 test 模式使用 move_uploaded_file（Rust 端用 rename）
        let temp = create_temp_file(b"hello", ".txt");
        let temp_dir = tempfile::tempdir().unwrap();
        let mut uploaded =
            UploadedFile::new(temp.path(), "original.txt", None, Some(0), false).unwrap();
        let moved = uploaded.move_to(&temp_dir, Some("moved.txt")).unwrap();
        assert!(moved.path().is_file());
        assert!(!temp.path().exists());
    }

    #[test]
    fn test_uploaded_file_as_file_access() {
        let temp = create_temp_file(b"hello", ".txt");
        let uploaded = UploadedFile::new(
            temp.path(),
            "original.txt",
            Some("text/plain"),
            Some(0),
            false,
        )
        .unwrap();
        // 不可变访问
        assert_eq!(uploaded.as_file().path(), temp.path());
        assert_eq!(uploaded.as_file().extension(), "txt");
    }

    #[test]
    fn test_uploaded_file_as_file_mut_access() {
        let temp = create_temp_file(b"hello", ".txt");
        let mut uploaded = UploadedFile::new(
            temp.path(),
            "original.txt",
            Some("text/plain"),
            Some(0),
            false,
        )
        .unwrap();
        // 可变访问 — 调用 File 的 hash 方法
        let md5 = uploaded.as_file_mut().md5().unwrap();
        assert_eq!(md5, "5d41402abc4b2a76b9719d911017c592");
    }
}
