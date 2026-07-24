//! 存储对接模块 — 对齐 PHP `storage\Driver` + `storage\engine\*`
//!
//! Phase 5.7 核心交付物。本模块实现 5 种存储驱动（本地/阿里云/腾讯云/七牛/S3），
//! 复用 `sz-orm-storage` crate 的底层实现，对齐 PHP `app\common\library\storage` 命名空间。
//!
//! ## PHP 对齐
//!
//! ### 核心类映射
//!
//! | PHP 类 | Rust 结构 | 说明 |
//! |---------|-----------|------|
//! | `storage\Driver` | [`StorageDriver`] | 存储驱动（工厂 + 代理） |
//! | `storage\engine\Server`（abstract） | [`StorageEngine`] trait | 存储引擎抽象 |
//! | `storage\engine\Local` | [`LocalStorageEngine`] | 本地存储引擎 |
//! | `storage\engine\Aliyun` | [`AliyunStorageEngine`] | 阿里云 OSS 引擎 |
//! | `storage\engine\Qcloud` | [`QcloudStorageEngine`] | 腾讯云 COS 引擎 |
//! | `storage\engine\Qiniu` | [`QiniuStorageEngine`] | 七牛云 Kodo 引擎 |
//! | （无 PHP 对应） | [`S3StorageEngine`] | AWS S3 兼容引擎 |
//!
//! ### 核心方法映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Driver::getEngineClass($storage)` | [`StorageDriver::new`] | 工厂方法 |
//! | `Server::setUploadFile($name)` | [`StorageEngine::set_upload_file`] | 设置上传文件 |
//! | `Server::setUploadFileByReal($filePath, $extension)` | [`StorageEngine::set_upload_file_by_real`] | 内部上传 |
//! | `Server::upload()` | [`StorageEngine::upload`] | 执行上传 |
//! | `Server::delete($fileName)` | [`StorageEngine::delete`] | 删除文件 |
//! | `Server::getFileName()` | [`StorageEngine::file_name`] | 获取文件名 |
//! | `Server::getFileInfo()` | [`StorageEngine::file_info`] | 获取文件信息 |
//! | `Server::getError()` | [`StorageEngine::error`] | 获取错误信息 |
//! | `Server::getRealPath()` | [`StorageEngine::real_path`] | 获取真实路径 |
//! | `Server::buildSaveName()`（private） | [`build_save_name`] | 生成保存文件名 |
//!
//! ## PHP 行为对齐（R5 硬约束）
//!
//! - **R5-16**：`buildSaveName` = `storage/{Ymd}/{YmdHis}{md5(realPath)[0..5]}{rand(0..9999) padded 4}.{ext}`
//!   （对齐 PHP `Server.php` 第 103-111 行）
//! - **R5-17**：`setUploadFileByReal` 的 `fileName` = `storage/{Ymd}/{basename}`（不用 YmdHis+md5+rand）
//!   （对齐 PHP `Server.php` 第 45-59 行）
//! - **R5-18**：`getRealPath` 区分 `isInternal`（内部用 `fileInfo.tmp_name`，外部用 `UploadedFile.path`）
//!   （对齐 PHP `Server.php` 第 84-91 行）
//! - **R5-19**：`Local::upload` 区分 `isInternal`（internal 用 `rename`，external 用 `putFile`）
//!   （对齐 PHP `Local.php` 第 20-23 行）
//! - **R5-20**：`Local::delete` Elvis 短路 `!file_exists($filePath) ?: unlink($filePath)`
//!   （对齐 PHP `Local.php` 第 59-64 行）
//! - **R5-21**：`Local::uploadByInternal` 失败设置 `error='upload write error'` 返回 `false`
//!   （对齐 PHP `Local.php` 第 43-54 行）
//! - **R5-22**：云存储 `upload` 成功返回 `true`（PHP 行为），Rust 端返回 `Ok(Some(save_name))`
//!   （对齐 PHP `Qcloud.php`/`Aliyun.php`/`Qiniu.php`）
//! - **R5-23**：`Local::uploadByExternal` 返回 `saveName`（`Filesystem::disk('public')->putFile` 返回路径）
//!   （对齐 PHP `Local.php` 第 28-38 行）
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\app\common\library\storage\Driver.php`（118 行）
//! - `e:\vue\test\鲜视达\server\app\common\library\storage\engine\Server.php`（112 行）
//! - `e:\vue\test\鲜视达\server\app\common\library\storage\engine\Local.php`（73 行）
//! - `e:\vue\test\鲜视达\server\app\common\library\storage\engine\Qcloud.php`（84 行）
//! - `e:\vue\test\鲜视达\server\app\common\library\storage\engine\Aliyun.php`（76 行）
//! - `e:\vue\test\鲜视达\server\app\common\library\storage\engine\Qiniu.php`（76 行）

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Local;
use md5::{Digest, Md5};
use rand::Rng;
use sz_orm_storage::Storage;

use super::{UploadError, UploadedFile};

// ============================================================================
// 存储引擎类型
// ============================================================================

/// 存储引擎类型 — 对齐 PHP `storage\Driver::getEngineClass($storage)` 的 `$storage` 参数
///
/// PHP 第 108-116 行：
/// ```php
/// private function getEngineClass($storage = 'qcloud') {
///     $engineName = is_null($storage) ? $this->config['default'] : $storage;
///     $classSpace = __NAMESPACE__ . '\\engine\\' . ucfirst($engineName);
///     if (!class_exists($classSpace)) {
///         throw new Exception('未找到存储引擎类: ' . $engineName);
///     }
///     return new $classSpace($this->config['engine'][$engineName]);
/// }
/// ```
///
/// Rust 端支持 5 种引擎（PHP 端 4 种 + S3）：
/// - `Local` → [`LocalStorageEngine`]（对齐 PHP `engine\Local`）
/// - `Aliyun` → [`AliyunStorageEngine`]（对齐 PHP `engine\Aliyun`）
/// - `Qcloud` → [`QcloudStorageEngine`]（对齐 PHP `engine\Qcloud`）
/// - `Qiniu` → [`QiniuStorageEngine`]（对齐 PHP `engine\Qiniu`）
/// - `S3` → [`S3StorageEngine`]（Rust 扩展，无 PHP 对应）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageEngineKind {
    /// 本地存储（对齐 PHP `engine\Local`）
    Local,
    /// 阿里云 OSS（对齐 PHP `engine\Aliyun`）
    Aliyun,
    /// 腾讯云 COS（对齐 PHP `engine\Qcloud`）
    Qcloud,
    /// 七牛云 Kodo（对齐 PHP `engine\Qiniu`）
    Qiniu,
    /// AWS S3 兼容（Rust 扩展）
    S3,
}

impl StorageEngineKind {
    /// 从字符串解析引擎类型 — 对齐 PHP `ucfirst($engineName)` + `class_exists` 检查
    ///
    /// PHP 行为：大小写不敏感（`ucfirst` 仅首字母大写），不匹配抛异常。
    /// Rust 端：大小写不敏感，不匹配返回 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "local" => Some(Self::Local),
            "aliyun" => Some(Self::Aliyun),
            "qcloud" => Some(Self::Qcloud),
            "qiniu" => Some(Self::Qiniu),
            "s3" => Some(Self::S3),
            _ => None,
        }
    }

    /// 获取引擎类型字符串
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Aliyun => "aliyun",
            Self::Qcloud => "qcloud",
            Self::Qiniu => "qiniu",
            Self::S3 => "s3",
        }
    }
}

// ============================================================================
// 引擎配置
// ============================================================================

/// 引擎配置 — 对齐 PHP `config['engine'][$engineName]` 配置数组
///
/// PHP 配置结构（`Setting.php` 第 201-228 行）：
/// ```php
/// 'storage' => [
///     'default' => 'local',
///     'engine' => [
///         'local' => [],
///         'qiniu' => ['bucket' => '', 'access_key' => '', 'secret_key' => '', 'domain' => 'http://'],
///         'aliyun' => ['bucket' => '', 'access_key_id' => '', 'access_key_secret' => '', 'domain' => 'http://'],
///         'qcloud' => ['bucket' => '', 'region' => '', 'secret_id' => '', 'secret_key' => '', 'domain' => 'http://'],
///     ],
/// ],
/// ```
///
/// Rust 端统一为一个结构体，不同引擎使用不同字段。
#[derive(Debug, Clone, Default)]
pub struct EngineConfig {
    /// 存储桶名称（对齐 PHP `bucket`）
    pub bucket: String,
    /// 地域（对齐 PHP `region`，仅 Qcloud 使用）
    pub region: String,
    /// 端点（对齐 PHP `domain`/`endpoint`，Aliyun 使用）
    pub endpoint: String,
    /// 自定义域名（对齐 PHP `domain`）
    pub domain: String,
    /// 阿里云 AccessKey ID（对齐 PHP `access_key_id`）
    pub access_key_id: String,
    /// 阿里云 AccessKey Secret（对齐 PHP `access_key_secret`）
    pub access_key_secret: String,
    /// 腾讯云 SecretId（对齐 PHP `secret_id`）
    pub secret_id: String,
    /// 腾讯云/七牛 SecretKey（对齐 PHP `secret_key`）
    pub secret_key: String,
    /// 七牛 AccessKey（对齐 PHP `access_key`）
    pub access_key: String,
    /// 本地存储基础路径（对齐 PHP `WEB_PATH`）
    pub base_path: String,
}

impl EngineConfig {
    /// 创建空配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 bucket
    pub fn with_bucket(mut self, bucket: impl Into<String>) -> Self {
        self.bucket = bucket.into();
        self
    }

    /// 设置 region
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = region.into();
        self
    }

    /// 设置 endpoint
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// 设置 domain
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = domain.into();
        self
    }

    /// 设置 base_path
    pub fn with_base_path(mut self, base_path: impl Into<String>) -> Self {
        self.base_path = base_path.into();
        self
    }

    /// 设置 access_key_id
    pub fn with_access_key_id(mut self, key: impl Into<String>) -> Self {
        self.access_key_id = key.into();
        self
    }

    /// 设置 access_key_secret
    pub fn with_access_key_secret(mut self, key: impl Into<String>) -> Self {
        self.access_key_secret = key.into();
        self
    }

    /// 设置 secret_id
    pub fn with_secret_id(mut self, key: impl Into<String>) -> Self {
        self.secret_id = key.into();
        self
    }

    /// 设置 secret_key
    pub fn with_secret_key(mut self, key: impl Into<String>) -> Self {
        self.secret_key = key.into();
        self
    }

    /// 设置 access_key（七牛）
    pub fn with_access_key(mut self, key: impl Into<String>) -> Self {
        self.access_key = key.into();
        self
    }
}

// ============================================================================
// 上传文件信息
// ============================================================================

/// 上传文件信息 — 对齐 PHP `Server::setUploadFileByReal` 中的 `$fileInfo` 数组
///
/// PHP 第 45-59 行：
/// ```php
/// public function setUploadFileByReal($filePath, $extension) {
///     $this->isInternal = true;
///     $this->fileInfo = [
///         'name' => basename($filePath),
///         'size' => filesize($filePath),
///         'extension' => $extension,
///         'tmp_name' => $filePath,
///         'error' => 0,
///         'isInternal' => $this->isInternal
///     ];
///     $this->fileName = 'storage/'.date('Ymd') ."/". $this->fileInfo['name'];
/// }
/// ```
#[derive(Debug, Clone)]
pub struct UploadFileInfo {
    /// 文件名（对齐 PHP `name` = `basename($filePath)`）
    pub name: String,
    /// 文件大小（字节，对齐 PHP `size` = `filesize($filePath)`）
    pub size: u64,
    /// 扩展名（对齐 PHP `extension`）
    pub extension: String,
    /// 临时文件路径（对齐 PHP `tmp_name`）
    pub tmp_name: PathBuf,
    /// 错误码（对齐 PHP `error`，0 表示无错误）
    pub error: i32,
    /// 是否内部上传（对齐 PHP `isInternal`）
    pub is_internal: bool,
}

impl UploadFileInfo {
    /// 从 `UploadedFile` 创建文件信息 — 对齐 PHP `Server::setUploadFile` 的隐式行为
    ///
    /// PHP `setUploadFile` 直接将 `Request::file($name)` 赋给 `$this->file`，
    /// 然后调用 `buildSaveName()` 生成文件名。
    /// Rust 端将 `UploadedFile` 转换为 `UploadFileInfo`。
    pub fn from_uploaded_file(file: &UploadedFile) -> Result<Self, UploadError> {
        let path = file.as_file().path();
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            name: file.original_name().to_string(),
            size,
            extension: file.original_extension(),
            tmp_name: path.to_path_buf(),
            error: file.error_code() as i32,
            is_internal: false,
        })
    }

    /// 从真实路径创建文件信息 — 对齐 PHP `Server::setUploadFileByReal`
    ///
    /// PHP 行为：`isInternal = true`，`name = basename($filePath)`，`size = filesize($filePath)`。
    pub fn from_real_path<P: AsRef<Path>>(path: P, extension: &str) -> Result<Self, UploadError> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(UploadError::FileNotFound(
                path.to_string_lossy().to_string(),
            ));
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            name,
            size,
            extension: extension.to_string(),
            tmp_name: path.to_path_buf(),
            error: 0,
            is_internal: true,
        })
    }
}

// ============================================================================
// 保存文件名生成
// ============================================================================

/// 生成保存文件名 — 对齐 PHP `Server::buildSaveName` 第 103-111 行
///
/// PHP 行为：
/// ```php
/// private function buildSaveName() {
///     $realPath = $this->file->getPathname();
///     $ext = $this->file->getOriginalExtension();
///     return 'storage/'.date('Ymd') ."/".date('YmdHis') . substr(md5($realPath), 0, 5)
///         . str_pad(rand(0, 9999), 4, '0', STR_PAD_LEFT) . ".{$ext}";
/// }
/// ```
///
/// Rust 端格式：`storage/{Ymd}/{YmdHis}{md5(realPath)[0..5]}{rand(0..9999) padded 4}.{ext}`
///
/// 注意：PHP `rand(0, 9999)` 是伪随机数，Rust 端使用 `rand::thread_rng().gen_range(0..=9999)`。
pub fn build_save_name(real_path: &Path, extension: &str) -> String {
    let now = Local::now();
    let ymd = now.format("%Y%m%d").to_string();
    let ymd_his = now.format("%Y%m%d%H%M%S").to_string();

    // 对齐 PHP `substr(md5($realPath), 0, 5)`
    let mut md5 = Md5::new();
    md5.update(real_path.to_string_lossy().as_bytes());
    let md5_hex = hex::encode(md5.finalize());
    let md5_prefix = &md5_hex[..5];

    // 对齐 PHP `str_pad(rand(0, 9999), 4, '0', STR_PAD_LEFT)`
    let rand_num: u32 = rand::thread_rng().gen_range(0..=9999);
    let rand_padded = format!("{:04}", rand_num);

    // 对齐 PHP `".{$ext}"` — 扩展名为空时不加点
    // 注意：PHP 格式为 `storage/{Ymd}/{YmdHis}{md5[5]}{rand[4]}.{ext}`（YmdHis 后无 `/`）
    if extension.is_empty() {
        format!("storage/{}/{}{}{}", ymd, ymd_his, md5_prefix, rand_padded)
    } else {
        format!(
            "storage/{}/{}{}{}.{}",
            ymd, ymd_his, md5_prefix, rand_padded, extension
        )
    }
}

/// 生成内部上传的保存文件名 — 对齐 PHP `Server::setUploadFileByReal` 第 58 行
///
/// PHP 行为：
/// ```php
/// $this->fileName = 'storage/'.date('Ymd') ."/". $this->fileInfo['name'];
/// ```
///
/// 注意：与 `buildSaveName` 不同，内部上传使用 `basename` 而非 `YmdHis+md5+rand`。
pub fn build_internal_save_name(file_path: &Path) -> String {
    let now = Local::now();
    let ymd = now.format("%Y%m%d").to_string();
    let basename = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    format!("storage/{}/{}", ymd, basename)
}

// ============================================================================
// StorageEngine trait
// ============================================================================

/// 存储引擎抽象 — 对齐 PHP `storage\engine\Server`（abstract）
///
/// PHP 抽象方法：`upload()` / `delete($fileName)` / `getFileName()`
/// PHP 具体方法：`setUploadFile` / `setUploadFileByReal` / `getFileInfo` / `getRealPath` / `getError` / `buildSaveName`
///
/// Rust 端使用 `async_trait` 因为 `upload`/`delete` 涉及 IO 操作。
#[async_trait]
pub trait StorageEngine: Send + Sync {
    /// 执行文件上传 — 对齐 PHP `Server::upload()`（abstract）
    ///
    /// 返回值：
    /// - `Ok(Some(save_name))`：Local 外部上传返回保存路径
    /// - `Ok(None)`：Local 内部上传或云存储上传成功（PHP 返回 `true`）
    /// - `Err(_)`：上传失败
    async fn upload(&mut self) -> Result<Option<String>, UploadError>;

    /// 执行文件删除 — 对齐 PHP `Server::delete($fileName)`（abstract）
    ///
    /// 返回值：
    /// - `Ok(true)`：删除成功或文件不存在（对齐 PHP Elvis 短路）
    /// - `Ok(false)`：删除失败（PHP 设置 `error` 返回 `false`）
    /// - `Err(_)`：系统错误
    async fn delete(&mut self, file_name: &str) -> Result<bool, UploadError>;

    /// 获取文件名 — 对齐 PHP `Server::getFileName()`
    fn file_name(&self) -> Option<&str>;

    /// 获取文件信息 — 对齐 PHP `Server::getFileInfo()`
    fn file_info(&self) -> Option<&UploadFileInfo>;

    /// 获取错误信息 — 对齐 PHP `Server::getError()`
    fn error(&self) -> Option<&str>;

    /// 设置上传文件 — 对齐 PHP `Server::setUploadFile($name)`
    ///
    /// PHP 行为：从 `Request::file($name)` 获取文件，调用 `buildSaveName()` 生成文件名。
    /// Rust 端：从 `UploadedFile` 设置，调用 `build_save_name()` 生成文件名。
    fn set_upload_file(&mut self, file: &UploadedFile) -> Result<(), UploadError>;

    /// 设置内部上传文件 — 对齐 PHP `Server::setUploadFileByReal($filePath, $extension)`
    ///
    /// PHP 行为：设置 `isInternal = true`，构造 `fileInfo`，文件名为 `storage/{Ymd}/{basename}`。
    fn set_upload_file_by_real(
        &mut self,
        file_path: &Path,
        extension: &str,
    ) -> Result<(), UploadError>;

    /// 是否内部上传
    fn is_internal(&self) -> bool;

    /// 获取真实路径 — 对齐 PHP `Server::getRealPath()` 第 84-91 行
    ///
    /// PHP 行为：
    /// - `isInternal == true` → 返回 `$this->fileInfo['tmp_name']`
    /// - `isInternal == false` → 返回 `request()->file('iFile')->getRealPath()`
    fn real_path(&self) -> Option<&Path> {
        self.file_info().map(|info| info.tmp_name.as_path())
    }
}

// ============================================================================
// LocalStorageEngine
// ============================================================================

/// 本地存储引擎 — 对齐 PHP `storage\engine\Local`
///
/// PHP 第 20-23 行：`upload` 区分 `isInternal`
/// PHP 第 28-38 行：`uploadByExternal` 用 `Filesystem::disk('public')->putFile`
/// PHP 第 43-54 行：`uploadByInternal` 用 `rename`
/// PHP 第 59-64 行：`delete` Elvis 短路 `!file_exists($filePath) ?: unlink($filePath)`
pub struct LocalStorageEngine {
    /// 引擎配置
    config: EngineConfig,
    /// 文件信息
    file_info: Option<UploadFileInfo>,
    /// 保存文件名
    file_name: Option<String>,
    /// 错误信息
    error: Option<String>,
    /// 是否内部上传
    is_internal: bool,
    /// 外部上传时的源文件路径（用于 copy）
    upload_source_path: Option<PathBuf>,
}

impl LocalStorageEngine {
    /// 创建本地存储引擎实例
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            file_info: None,
            file_name: None,
            error: None,
            is_internal: false,
            upload_source_path: None,
        }
    }

    /// 获取上传目录 — 对齐 PHP `WEB_PATH . 'uploads'`
    ///
    /// PHP 行为：`$uplodDir = WEB_PATH . 'uploads';`
    /// Rust 端：`config.base_path` + `uploads`
    pub fn upload_dir(&self) -> PathBuf {
        if self.config.base_path.is_empty() {
            PathBuf::from("uploads")
        } else {
            PathBuf::from(&self.config.base_path).join("uploads")
        }
    }

    /// 内部上传 — 对齐 PHP `Local::uploadByInternal` 第 43-54 行
    ///
    /// PHP 行为：
    /// ```php
    /// private function uploadByInternal() {
    ///     $uplodDir = WEB_PATH . 'uploads';
    ///     $realPath = $this->getRealPath();
    ///     if (!rename($realPath, "{$uplodDir}/$this->fileName")) {
    ///         $this->error = 'upload write error';
    ///         return false;
    ///     }
    ///     return true;
    /// }
    /// ```
    #[tracing::instrument(skip(self))]
    async fn upload_by_internal(&mut self) -> Result<bool, UploadError> {
        let target = self.upload_dir().join(
            self.file_name
                .as_ref()
                .ok_or_else(|| UploadError::UploadFailed("file name not set".to_string()))?
                .as_str(),
        );
        let real_path = self
            .file_info
            .as_ref()
            .ok_or_else(|| UploadError::UploadFailed("file info not set".to_string()))?
            .tmp_name
            .clone();

        // 对齐 PHP `rename($realPath, "{$uplodDir}/$this->fileName")`
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        match tokio::fs::rename(&real_path, &target).await {
            Ok(_) => Ok(true),
            Err(e) => {
                // 对齐 PHP `$this->error = 'upload write error'; return false;`
                self.error = Some("upload write error".to_string());
                Err(UploadError::MoveFailed {
                    from: real_path.to_string_lossy().to_string(),
                    to: target.to_string_lossy().to_string(),
                    error: e.to_string(),
                })
            }
        }
    }

    /// 外部上传 — 对齐 PHP `Local::uploadByExternal` 第 28-38 行
    ///
    /// PHP 行为：
    /// ```php
    /// private function uploadByExternal() {
    ///     $saveName = '';
    ///     try {
    ///         $saveName = Filesystem::disk('public')->putFile('', $this->file);
    ///     } catch (\Exception $e) {
    ///         log_write('文件上传异常:'.$e->getMessage());
    ///     }
    ///     return $saveName;
    /// }
    /// ```
    ///
    /// Rust 端：使用 `tokio::fs::copy` 模拟 `putFile`，返回保存路径。
    #[tracing::instrument(skip(self))]
    async fn upload_by_external(&mut self) -> Result<String, UploadError> {
        let file_name = self
            .file_name
            .as_ref()
            .ok_or_else(|| UploadError::UploadFailed("file name not set".to_string()))?
            .clone();
        let source = self
            .upload_source_path
            .as_ref()
            .or_else(|| self.file_info.as_ref().map(|info| &info.tmp_name))
            .cloned()
            .ok_or_else(|| UploadError::UploadFailed("source path not set".to_string()))?;

        let target = self.upload_dir().join(&file_name);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // 对齐 PHP `Filesystem::disk('public')->putFile('', $this->file)`
        // Rust 端用 copy（保留原文件，对齐 think Filesystem public disk 的 putFile 行为）
        tokio::fs::copy(&source, &target).await?;
        Ok(file_name)
    }
}

#[async_trait]
impl StorageEngine for LocalStorageEngine {
    #[tracing::instrument(skip(self))]
    async fn upload(&mut self) -> Result<Option<String>, UploadError> {
        // 对齐 PHP 第 20-23 行：`return $this->isInternal ? $this->uploadByInternal() : $this->uploadByExternal();`
        if self.is_internal {
            // 内部上传返回 true/false（PHP），Rust 端用 Ok(None) 表示成功
            self.upload_by_internal().await?;
            Ok(None)
        } else {
            // 外部上传返回 saveName
            let save_name = self.upload_by_external().await?;
            Ok(Some(save_name))
        }
    }

    #[tracing::instrument(skip(self))]
    async fn delete(&mut self, file_name: &str) -> Result<bool, UploadError> {
        // 对齐 PHP 第 59-64 行：
        // `$filePath = WEB_PATH . "uploads/{$fileName}";`
        // `return !file_exists($filePath) ?: unlink($filePath);`

        // 安全：防止路径遍历攻击（PHP 原始代码未做此检查）
        // 拒绝包含 `..` 或绝对路径的 file_name
        if file_name.contains("..") || std::path::Path::new(file_name).is_absolute() {
            return Err(UploadError::InvalidFileName(file_name.to_string()));
        }

        let file_path = self.upload_dir().join(file_name);

        // 安全：验证最终路径仍在 upload_dir 内（canonicalize 后比较）
        if let (Ok(upload_canon), Ok(file_canon)) = (
            std::fs::canonicalize(self.upload_dir()),
            std::fs::canonicalize(&file_path),
        ) {
            if !file_canon.starts_with(&upload_canon) {
                return Err(UploadError::InvalidFileName(file_name.to_string()));
            }
        }

        if !file_path.exists() {
            // 对齐 PHP `!file_exists($filePath)` 为 true 时短路返回 true
            return Ok(true);
        }
        // 对齐 PHP `unlink($filePath)`
        match tokio::fs::remove_file(&file_path).await {
            Ok(_) => Ok(true),
            Err(e) => {
                // PHP `unlink` 失败返回 false（不抛异常），Rust 端记录错误返回 false
                self.error = Some(e.to_string());
                Ok(false)
            }
        }
    }

    fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    fn file_info(&self) -> Option<&UploadFileInfo> {
        self.file_info.as_ref()
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn set_upload_file(&mut self, file: &UploadedFile) -> Result<(), UploadError> {
        // 对齐 PHP `Server::setUploadFile` 第 32-40 行：
        // `$this->file = Request::file($name);`
        // `$this->fileName = $this->buildSaveName();`
        let info = UploadFileInfo::from_uploaded_file(file)?;
        let save_name = build_save_name(&info.tmp_name, &info.extension);
        self.upload_source_path = Some(info.tmp_name.clone());
        self.file_info = Some(info);
        self.file_name = Some(save_name);
        self.is_internal = false;
        Ok(())
    }

    fn set_upload_file_by_real(
        &mut self,
        file_path: &Path,
        extension: &str,
    ) -> Result<(), UploadError> {
        // 对齐 PHP `Server::setUploadFileByReal` 第 45-59 行：
        // `$this->isInternal = true;`
        // `$this->fileInfo = [...]`
        // `$this->fileName = 'storage/'.date('Ymd') ."/". $this->fileInfo['name'];`
        let info = UploadFileInfo::from_real_path(file_path, extension)?;
        let save_name = build_internal_save_name(file_path);
        self.file_info = Some(info);
        self.file_name = Some(save_name);
        self.is_internal = true;
        Ok(())
    }

    fn is_internal(&self) -> bool {
        self.is_internal
    }
}

// ============================================================================
// AliyunStorageEngine
// ============================================================================

/// 阿里云 OSS 存储引擎 — 对齐 PHP `storage\engine\Aliyun`
///
/// PHP 行为：
/// - `upload`：`OssClient::uploadFile($bucket, $fileName, $realPath)`，成功返回 `true`，失败设置 `error` 返回 `false`
/// - `delete`：`OssClient::deleteObject($bucket, $fileName)`，成功返回 `true`，失败设置 `error` 返回 `false`
///
/// Rust 端复用 `sz-orm-storage::AliyunOssStorage`。
pub struct AliyunStorageEngine {
    /// 引擎配置
    config: EngineConfig,
    /// 文件信息
    file_info: Option<UploadFileInfo>,
    /// 保存文件名
    file_name: Option<String>,
    /// 错误信息
    error: Option<String>,
    /// 是否内部上传
    is_internal: bool,
}

impl AliyunStorageEngine {
    /// 创建阿里云存储引擎实例
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            file_info: None,
            file_name: None,
            error: None,
            is_internal: false,
        }
    }

    /// 创建底层存储
    fn create_storage(&self) -> sz_orm_storage::AliyunOssStorage {
        sz_orm_storage::AliyunOssStorage::new(
            self.config.bucket.clone(),
            self.config.endpoint.clone(),
        )
    }
}

#[async_trait]
impl StorageEngine for AliyunStorageEngine {
    #[tracing::instrument(skip(self))]
    async fn upload(&mut self) -> Result<Option<String>, UploadError> {
        // 对齐 PHP `Aliyun::upload` 第 27-46 行
        let file_name = self
            .file_name
            .as_ref()
            .ok_or_else(|| UploadError::UploadFailed("file name not set".to_string()))?
            .clone();
        let info = self
            .file_info
            .as_ref()
            .ok_or_else(|| UploadError::UploadFailed("file info not set".to_string()))?
            .clone();

        let storage = self.create_storage();
        let data = tokio::fs::read(&info.tmp_name).await?;
        let content_type = mime_guess::from_path(&info.tmp_name)
            .first_or_octet_stream()
            .to_string();

        match storage.put(&file_name, &data, &content_type).await {
            Ok(_) => {
                // 对齐 PHP `return true;`
                Ok(Some(file_name))
            }
            Err(e) => {
                // 对齐 PHP `$this->error = $e->getMessage(); return false;`
                let msg = e.to_string();
                self.error = Some(msg.clone());
                Err(UploadError::UploadFailed(msg))
            }
        }
    }

    #[tracing::instrument(skip(self))]
    async fn delete(&mut self, file_name: &str) -> Result<bool, UploadError> {
        // 对齐 PHP `Aliyun::delete` 第 51-66 行
        let storage = self.create_storage();
        match storage.delete(file_name).await {
            Ok(_) => Ok(true),
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(msg);
                Ok(false)
            }
        }
    }

    fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    fn file_info(&self) -> Option<&UploadFileInfo> {
        self.file_info.as_ref()
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn set_upload_file(&mut self, file: &UploadedFile) -> Result<(), UploadError> {
        let info = UploadFileInfo::from_uploaded_file(file)?;
        let save_name = build_save_name(&info.tmp_name, &info.extension);
        self.file_info = Some(info);
        self.file_name = Some(save_name);
        self.is_internal = false;
        Ok(())
    }

    fn set_upload_file_by_real(
        &mut self,
        file_path: &Path,
        extension: &str,
    ) -> Result<(), UploadError> {
        let info = UploadFileInfo::from_real_path(file_path, extension)?;
        let save_name = build_internal_save_name(file_path);
        self.file_info = Some(info);
        self.file_name = Some(save_name);
        self.is_internal = true;
        Ok(())
    }

    fn is_internal(&self) -> bool {
        self.is_internal
    }
}

// ============================================================================
// QcloudStorageEngine
// ============================================================================

/// 腾讯云 COS 存储引擎 — 对齐 PHP `storage\engine\Qcloud`
///
/// PHP 行为：
/// - `upload`：`cosClient->putObject(['Bucket', 'Key', 'Body' => fopen(...)])`，成功返回 `true`
/// - `delete`：`cosClient->deleteObject(['Bucket', 'Key'])`，成功返回 `true`
///
/// Rust 端复用 `sz-orm-storage::TencentCosStorage`。
pub struct QcloudStorageEngine {
    /// 引擎配置
    config: EngineConfig,
    /// 文件信息
    file_info: Option<UploadFileInfo>,
    /// 保存文件名
    file_name: Option<String>,
    /// 错误信息
    error: Option<String>,
    /// 是否内部上传
    is_internal: bool,
}

impl QcloudStorageEngine {
    /// 创建腾讯云存储引擎实例
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            file_info: None,
            file_name: None,
            error: None,
            is_internal: false,
        }
    }

    /// 创建底层存储
    fn create_storage(&self) -> sz_orm_storage::TencentCosStorage {
        sz_orm_storage::TencentCosStorage::new(
            self.config.bucket.clone(),
            self.config.region.clone(),
        )
    }
}

#[async_trait]
impl StorageEngine for QcloudStorageEngine {
    #[tracing::instrument(skip(self))]
    async fn upload(&mut self) -> Result<Option<String>, UploadError> {
        // 对齐 PHP `Qcloud::upload` 第 44-59 行
        let file_name = self
            .file_name
            .as_ref()
            .ok_or_else(|| UploadError::UploadFailed("file name not set".to_string()))?
            .clone();
        let info = self
            .file_info
            .as_ref()
            .ok_or_else(|| UploadError::UploadFailed("file info not set".to_string()))?
            .clone();

        let storage = self.create_storage();
        let data = tokio::fs::read(&info.tmp_name).await?;
        let content_type = mime_guess::from_path(&info.tmp_name)
            .first_or_octet_stream()
            .to_string();

        match storage.put(&file_name, &data, &content_type).await {
            Ok(_) => Ok(Some(file_name)),
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(msg.clone());
                Err(UploadError::UploadFailed(msg))
            }
        }
    }

    #[tracing::instrument(skip(self))]
    async fn delete(&mut self, file_name: &str) -> Result<bool, UploadError> {
        let storage = self.create_storage();
        match storage.delete(file_name).await {
            Ok(_) => Ok(true),
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(msg);
                Ok(false)
            }
        }
    }

    fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    fn file_info(&self) -> Option<&UploadFileInfo> {
        self.file_info.as_ref()
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn set_upload_file(&mut self, file: &UploadedFile) -> Result<(), UploadError> {
        let info = UploadFileInfo::from_uploaded_file(file)?;
        let save_name = build_save_name(&info.tmp_name, &info.extension);
        self.file_info = Some(info);
        self.file_name = Some(save_name);
        self.is_internal = false;
        Ok(())
    }

    fn set_upload_file_by_real(
        &mut self,
        file_path: &Path,
        extension: &str,
    ) -> Result<(), UploadError> {
        let info = UploadFileInfo::from_real_path(file_path, extension)?;
        let save_name = build_internal_save_name(file_path);
        self.file_info = Some(info);
        self.file_name = Some(save_name);
        self.is_internal = true;
        Ok(())
    }

    fn is_internal(&self) -> bool {
        self.is_internal
    }
}

// ============================================================================
// QiniuStorageEngine
// ============================================================================

/// 七牛云 Kodo 存储引擎 — 对齐 PHP `storage\engine\Qiniu`
///
/// PHP 行为：
/// - `upload`：`UploadManager::putFile($token, $fileName, $realPath)`，成功返回 `true`
/// - `delete`：`BucketManager::delete($bucket, $fileName)`，成功返回 `true`
///
/// Rust 端复用 `sz-orm-storage::QiniuKodoStorage`。
pub struct QiniuStorageEngine {
    /// 引擎配置
    config: EngineConfig,
    /// 文件信息
    file_info: Option<UploadFileInfo>,
    /// 保存文件名
    file_name: Option<String>,
    /// 错误信息
    error: Option<String>,
    /// 是否内部上传
    is_internal: bool,
}

impl QiniuStorageEngine {
    /// 创建七牛存储引擎实例
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            file_info: None,
            file_name: None,
            error: None,
            is_internal: false,
        }
    }

    /// 创建底层存储
    fn create_storage(&self) -> sz_orm_storage::QiniuKodoStorage {
        sz_orm_storage::QiniuKodoStorage::new(self.config.bucket.clone())
    }
}

#[async_trait]
impl StorageEngine for QiniuStorageEngine {
    #[tracing::instrument(skip(self))]
    async fn upload(&mut self) -> Result<Option<String>, UploadError> {
        // 对齐 PHP `Qiniu::upload` 第 28-50 行
        let file_name = self
            .file_name
            .as_ref()
            .ok_or_else(|| UploadError::UploadFailed("file name not set".to_string()))?
            .clone();
        let info = self
            .file_info
            .as_ref()
            .ok_or_else(|| UploadError::UploadFailed("file info not set".to_string()))?
            .clone();

        let storage = self.create_storage();
        let data = tokio::fs::read(&info.tmp_name).await?;
        let content_type = mime_guess::from_path(&info.tmp_name)
            .first_or_octet_stream()
            .to_string();

        match storage.put(&file_name, &data, &content_type).await {
            Ok(_) => Ok(Some(file_name)),
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(msg.clone());
                Err(UploadError::UploadFailed(msg))
            }
        }
    }

    #[tracing::instrument(skip(self))]
    async fn delete(&mut self, file_name: &str) -> Result<bool, UploadError> {
        let storage = self.create_storage();
        match storage.delete(file_name).await {
            Ok(_) => Ok(true),
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(msg);
                Ok(false)
            }
        }
    }

    fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    fn file_info(&self) -> Option<&UploadFileInfo> {
        self.file_info.as_ref()
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn set_upload_file(&mut self, file: &UploadedFile) -> Result<(), UploadError> {
        let info = UploadFileInfo::from_uploaded_file(file)?;
        let save_name = build_save_name(&info.tmp_name, &info.extension);
        self.file_info = Some(info);
        self.file_name = Some(save_name);
        self.is_internal = false;
        Ok(())
    }

    fn set_upload_file_by_real(
        &mut self,
        file_path: &Path,
        extension: &str,
    ) -> Result<(), UploadError> {
        let info = UploadFileInfo::from_real_path(file_path, extension)?;
        let save_name = build_internal_save_name(file_path);
        self.file_info = Some(info);
        self.file_name = Some(save_name);
        self.is_internal = true;
        Ok(())
    }

    fn is_internal(&self) -> bool {
        self.is_internal
    }
}

// ============================================================================
// S3StorageEngine
// ============================================================================

/// AWS S3 兼容存储引擎 — Rust 扩展（无 PHP 对应）
///
/// 复用 `sz-orm-storage::S3Storage`。
pub struct S3StorageEngine {
    /// 引擎配置
    config: EngineConfig,
    /// 文件信息
    file_info: Option<UploadFileInfo>,
    /// 保存文件名
    file_name: Option<String>,
    /// 错误信息
    error: Option<String>,
    /// 是否内部上传
    is_internal: bool,
}

impl S3StorageEngine {
    /// 创建 S3 存储引擎实例
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            file_info: None,
            file_name: None,
            error: None,
            is_internal: false,
        }
    }

    /// 创建底层存储
    fn create_storage(&self) -> sz_orm_storage::S3Storage {
        sz_orm_storage::S3Storage::new(self.config.bucket.clone(), self.config.region.clone())
    }
}

#[async_trait]
impl StorageEngine for S3StorageEngine {
    #[tracing::instrument(skip(self))]
    async fn upload(&mut self) -> Result<Option<String>, UploadError> {
        let file_name = self
            .file_name
            .as_ref()
            .ok_or_else(|| UploadError::UploadFailed("file name not set".to_string()))?
            .clone();
        let info = self
            .file_info
            .as_ref()
            .ok_or_else(|| UploadError::UploadFailed("file info not set".to_string()))?
            .clone();

        let storage = self.create_storage();
        let data = tokio::fs::read(&info.tmp_name).await?;
        let content_type = mime_guess::from_path(&info.tmp_name)
            .first_or_octet_stream()
            .to_string();

        match storage.put(&file_name, &data, &content_type).await {
            Ok(_) => Ok(Some(file_name)),
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(msg.clone());
                Err(UploadError::UploadFailed(msg))
            }
        }
    }

    #[tracing::instrument(skip(self))]
    async fn delete(&mut self, file_name: &str) -> Result<bool, UploadError> {
        let storage = self.create_storage();
        match storage.delete(file_name).await {
            Ok(_) => Ok(true),
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(msg);
                Ok(false)
            }
        }
    }

    fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    fn file_info(&self) -> Option<&UploadFileInfo> {
        self.file_info.as_ref()
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn set_upload_file(&mut self, file: &UploadedFile) -> Result<(), UploadError> {
        let info = UploadFileInfo::from_uploaded_file(file)?;
        let save_name = build_save_name(&info.tmp_name, &info.extension);
        self.file_info = Some(info);
        self.file_name = Some(save_name);
        self.is_internal = false;
        Ok(())
    }

    fn set_upload_file_by_real(
        &mut self,
        file_path: &Path,
        extension: &str,
    ) -> Result<(), UploadError> {
        let info = UploadFileInfo::from_real_path(file_path, extension)?;
        let save_name = build_internal_save_name(file_path);
        self.file_info = Some(info);
        self.file_name = Some(save_name);
        self.is_internal = true;
        Ok(())
    }

    fn is_internal(&self) -> bool {
        self.is_internal
    }
}

// ============================================================================
// StorageDriver — 工厂 + 代理
// ============================================================================

/// 存储驱动 — 对齐 PHP `storage\Driver`
///
/// PHP 第 12-23 行：构造方法实例化引擎
/// PHP 第 108-116 行：`getEngineClass` 工厂方法
///
/// Rust 端使用枚举分发，避免动态分发开销。
pub enum StorageDriver {
    /// 本地存储引擎
    Local(LocalStorageEngine),
    /// 阿里云 OSS 引擎
    Aliyun(AliyunStorageEngine),
    /// 腾讯云 COS 引擎
    Qcloud(QcloudStorageEngine),
    /// 七牛云 Kodo 引擎
    Qiniu(QiniuStorageEngine),
    /// AWS S3 兼容引擎
    S3(S3StorageEngine),
}

impl StorageDriver {
    /// 创建存储驱动 — 对齐 PHP `Driver::__construct($config, $storage = 'qcloud')`
    ///
    /// PHP 第 18-23 行：
    /// ```php
    /// public function __construct($config, $storage = 'qcloud') {
    ///     $this->config = $config;
    ///     $this->engine = $this->getEngineClass($storage);
    /// }
    /// ```
    pub fn new(kind: StorageEngineKind, config: EngineConfig) -> Self {
        match kind {
            StorageEngineKind::Local => Self::Local(LocalStorageEngine::new(config)),
            StorageEngineKind::Aliyun => Self::Aliyun(AliyunStorageEngine::new(config)),
            StorageEngineKind::Qcloud => Self::Qcloud(QcloudStorageEngine::new(config)),
            StorageEngineKind::Qiniu => Self::Qiniu(QiniuStorageEngine::new(config)),
            StorageEngineKind::S3 => Self::S3(S3StorageEngine::new(config)),
        }
    }

    /// 执行上传 — 对齐 PHP `Driver::upload()`
    #[tracing::instrument(skip(self))]
    pub async fn upload(&mut self) -> Result<Option<String>, UploadError> {
        match self {
            Self::Local(e) => e.upload().await,
            Self::Aliyun(e) => e.upload().await,
            Self::Qcloud(e) => e.upload().await,
            Self::Qiniu(e) => e.upload().await,
            Self::S3(e) => e.upload().await,
        }
    }

    /// 执行删除 — 对齐 PHP `Driver::delete($fileName)`
    #[tracing::instrument(skip(self))]
    pub async fn delete(&mut self, file_name: &str) -> Result<bool, UploadError> {
        match self {
            Self::Local(e) => e.delete(file_name).await,
            Self::Aliyun(e) => e.delete(file_name).await,
            Self::Qcloud(e) => e.delete(file_name).await,
            Self::Qiniu(e) => e.delete(file_name).await,
            Self::S3(e) => e.delete(file_name).await,
        }
    }

    /// 获取文件名 — 对齐 PHP `Driver::getFileName()`
    pub fn file_name(&self) -> Option<&str> {
        match self {
            Self::Local(e) => e.file_name(),
            Self::Aliyun(e) => e.file_name(),
            Self::Qcloud(e) => e.file_name(),
            Self::Qiniu(e) => e.file_name(),
            Self::S3(e) => e.file_name(),
        }
    }

    /// 获取文件信息 — 对齐 PHP `Driver::getFileInfo()`
    pub fn file_info(&self) -> Option<&UploadFileInfo> {
        match self {
            Self::Local(e) => e.file_info(),
            Self::Aliyun(e) => e.file_info(),
            Self::Qcloud(e) => e.file_info(),
            Self::Qiniu(e) => e.file_info(),
            Self::S3(e) => e.file_info(),
        }
    }

    /// 获取错误信息 — 对齐 PHP `Driver::getError()`
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Local(e) => e.error(),
            Self::Aliyun(e) => e.error(),
            Self::Qcloud(e) => e.error(),
            Self::Qiniu(e) => e.error(),
            Self::S3(e) => e.error(),
        }
    }

    /// 设置上传文件 — 对齐 PHP `Driver::setUploadFile($name)`
    pub fn set_upload_file(&mut self, file: &UploadedFile) -> Result<(), UploadError> {
        match self {
            Self::Local(e) => e.set_upload_file(file),
            Self::Aliyun(e) => e.set_upload_file(file),
            Self::Qcloud(e) => e.set_upload_file(file),
            Self::Qiniu(e) => e.set_upload_file(file),
            Self::S3(e) => e.set_upload_file(file),
        }
    }

    /// 设置内部上传文件 — 对齐 PHP `Driver::setUploadFileByReal($filePath, $extension)`
    pub fn set_upload_file_by_real(
        &mut self,
        file_path: &Path,
        extension: &str,
    ) -> Result<(), UploadError> {
        match self {
            Self::Local(e) => e.set_upload_file_by_real(file_path, extension),
            Self::Aliyun(e) => e.set_upload_file_by_real(file_path, extension),
            Self::Qcloud(e) => e.set_upload_file_by_real(file_path, extension),
            Self::Qiniu(e) => e.set_upload_file_by_real(file_path, extension),
            Self::S3(e) => e.set_upload_file_by_real(file_path, extension),
        }
    }

    /// 是否内部上传
    pub fn is_internal(&self) -> bool {
        match self {
            Self::Local(e) => e.is_internal(),
            Self::Aliyun(e) => e.is_internal(),
            Self::Qcloud(e) => e.is_internal(),
            Self::Qiniu(e) => e.is_internal(),
            Self::S3(e) => e.is_internal(),
        }
    }

    /// 获取引擎类型
    pub fn kind(&self) -> StorageEngineKind {
        match self {
            Self::Local(_) => StorageEngineKind::Local,
            Self::Aliyun(_) => StorageEngineKind::Aliyun,
            Self::Qcloud(_) => StorageEngineKind::Qcloud,
            Self::Qiniu(_) => StorageEngineKind::Qiniu,
            Self::S3(_) => StorageEngineKind::S3,
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ---------------------------------------------------------------------
    // 辅助函数
    // ---------------------------------------------------------------------

    /// 创建临时文件
    fn create_temp_file(name: &str, content: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sz_rust_storage_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content).unwrap();
        path
    }

    /// 创建临时目录作为 base_path
    fn create_temp_base_path() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sz_rust_storage_base_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ---------------------------------------------------------------------
    // 组 1：StorageEngineKind 基础测试
    // ---------------------------------------------------------------------

    #[test]
    fn test_storage_engine_kind_parse_local() {
        assert_eq!(
            StorageEngineKind::parse("local"),
            Some(StorageEngineKind::Local)
        );
    }

    #[test]
    fn test_storage_engine_kind_parse_aliyun() {
        assert_eq!(
            StorageEngineKind::parse("aliyun"),
            Some(StorageEngineKind::Aliyun)
        );
    }

    #[test]
    fn test_storage_engine_kind_parse_qcloud() {
        assert_eq!(
            StorageEngineKind::parse("qcloud"),
            Some(StorageEngineKind::Qcloud)
        );
    }

    #[test]
    fn test_storage_engine_kind_parse_qiniu() {
        assert_eq!(
            StorageEngineKind::parse("qiniu"),
            Some(StorageEngineKind::Qiniu)
        );
    }

    #[test]
    fn test_storage_engine_kind_parse_s3() {
        assert_eq!(StorageEngineKind::parse("s3"), Some(StorageEngineKind::S3));
    }

    #[test]
    fn test_storage_engine_kind_parse_case_insensitive() {
        // 对齐 PHP ucfirst 行为（大小写不敏感）
        assert_eq!(
            StorageEngineKind::parse("LOCAL"),
            Some(StorageEngineKind::Local)
        );
        assert_eq!(
            StorageEngineKind::parse("Aliyun"),
            Some(StorageEngineKind::Aliyun)
        );
    }

    #[test]
    fn test_storage_engine_kind_parse_invalid() {
        assert_eq!(StorageEngineKind::parse("invalid"), None);
        assert_eq!(StorageEngineKind::parse(""), None);
    }

    #[test]
    fn test_storage_engine_kind_as_str() {
        assert_eq!(StorageEngineKind::Local.as_str(), "local");
        assert_eq!(StorageEngineKind::Aliyun.as_str(), "aliyun");
        assert_eq!(StorageEngineKind::Qcloud.as_str(), "qcloud");
        assert_eq!(StorageEngineKind::Qiniu.as_str(), "qiniu");
        assert_eq!(StorageEngineKind::S3.as_str(), "s3");
    }

    // ---------------------------------------------------------------------
    // 组 2：EngineConfig builder 测试
    // ---------------------------------------------------------------------

    #[test]
    fn test_engine_config_builder() {
        let config = EngineConfig::new()
            .with_bucket("my-bucket")
            .with_region("us-east-1")
            .with_endpoint("oss-cn-hangzhou.aliyuncs.com")
            .with_domain("https://cdn.example.com")
            .with_base_path("/var/www/uploads")
            .with_access_key_id("akid")
            .with_access_key_secret("aksecret")
            .with_secret_id("sid")
            .with_secret_key("sk")
            .with_access_key("ak");
        assert_eq!(config.bucket, "my-bucket");
        assert_eq!(config.region, "us-east-1");
        assert_eq!(config.endpoint, "oss-cn-hangzhou.aliyuncs.com");
        assert_eq!(config.domain, "https://cdn.example.com");
        assert_eq!(config.base_path, "/var/www/uploads");
        assert_eq!(config.access_key_id, "akid");
        assert_eq!(config.access_key_secret, "aksecret");
        assert_eq!(config.secret_id, "sid");
        assert_eq!(config.secret_key, "sk");
        assert_eq!(config.access_key, "ak");
    }

    #[test]
    fn test_engine_config_default() {
        let config = EngineConfig::default();
        assert!(config.bucket.is_empty());
        assert!(config.region.is_empty());
        assert!(config.endpoint.is_empty());
        assert!(config.domain.is_empty());
        assert!(config.base_path.is_empty());
    }

    // ---------------------------------------------------------------------
    // 组 3：build_save_name 测试
    // ---------------------------------------------------------------------

    #[test]
    fn test_build_save_name_format() {
        let path = Path::new("/tmp/photo.jpg");
        let name = build_save_name(path, "jpg");
        // 格式：storage/{Ymd}/{YmdHis}{md5[0..5]}{rand padded 4}.jpg
        assert!(name.starts_with("storage/"), "name = {}", name);
        assert!(name.ends_with(".jpg"), "name = {}", name);
        // 验证日期部分（前 8 位是 Ymd）
        let parts: Vec<&str> = name.split('/').collect();
        assert_eq!(parts.len(), 3, "name = {}", name);
        let ymd_part = parts[1];
        assert_eq!(ymd_part.len(), 8, "Ymd should be 8 chars");
        assert!(ymd_part.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_build_save_name_md5_prefix_consistent() {
        // 相同路径 + 相同时间（秒级）生成的 md5 前缀应一致
        let path = Path::new("/tmp/test.png");
        let name1 = build_save_name(path, "png");
        let name2 = build_save_name(path, "png");
        // 提取 md5 前缀（去掉 storage/{Ymd}/{YmdHis} 前缀和随机部分）
        // 格式：storage/{Ymd}/{YmdHis}{md5[5]}{rand[4]}.ext
        // Ymd = 8, YmdHis = 14, md5 = 5, rand = 4
        let extract_md5 = |s: &str| -> String {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() != 3 {
                return String::new();
            }
            let last = parts[2];
            // 去掉 .ext 后缀
            let last = last.rsplit_once('.').map(|(l, _)| l).unwrap_or(last);
            // 去掉前 14 位（YmdHis）
            if last.len() > 14 {
                last[14..19].to_string()
            } else {
                String::new()
            }
        };
        let md5_1 = extract_md5(&name1);
        let md5_2 = extract_md5(&name2);
        assert_eq!(
            md5_1, md5_2,
            "md5 prefix should be consistent for same path"
        );
        assert_eq!(md5_1.len(), 5, "md5 prefix should be 5 chars");
    }

    #[test]
    fn test_build_save_name_empty_extension() {
        let path = Path::new("/tmp/noext");
        let name = build_save_name(path, "");
        // 扩展名为空时不加点
        assert!(!name.ends_with('.'), "name = {}", name);
        assert!(name.starts_with("storage/"));
    }

    #[test]
    fn test_build_internal_save_name_format() {
        let path = Path::new("/tmp/photo.jpg");
        let name = build_internal_save_name(path);
        // 格式：storage/{Ymd}/{basename}
        assert!(name.starts_with("storage/"), "name = {}", name);
        assert!(name.ends_with("photo.jpg"), "name = {}", name);
        let parts: Vec<&str> = name.split('/').collect();
        assert_eq!(parts.len(), 3, "name = {}", name);
        // Ymd = 8 位数字
        assert_eq!(parts[1].len(), 8);
        assert!(parts[1].chars().all(|c| c.is_ascii_digit()));
    }

    // ---------------------------------------------------------------------
    // 组 4：UploadFileInfo 测试
    // ---------------------------------------------------------------------

    #[test]
    fn test_upload_file_info_from_real_path() {
        let path = create_temp_file("test.txt", b"hello world");
        let info = UploadFileInfo::from_real_path(&path, "txt").unwrap();
        assert_eq!(info.name, "test.txt");
        assert_eq!(info.size, 11);
        assert_eq!(info.extension, "txt");
        assert_eq!(info.tmp_name, path);
        assert_eq!(info.error, 0);
        assert!(info.is_internal);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_upload_file_info_from_real_path_not_found() {
        let result = UploadFileInfo::from_real_path("/nonexistent/path.txt", "txt");
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------------
    // 组 5：LocalStorageEngine 测试
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn test_local_storage_engine_new() {
        let config = EngineConfig::new().with_base_path("/tmp/test");
        let engine = LocalStorageEngine::new(config);
        assert!(!engine.is_internal());
        assert!(engine.file_name().is_none());
        assert!(engine.file_info().is_none());
        assert!(engine.error().is_none());
    }

    #[tokio::test]
    async fn test_local_storage_upload_dir_default() {
        let config = EngineConfig::new();
        let engine = LocalStorageEngine::new(config);
        assert_eq!(engine.upload_dir(), PathBuf::from("uploads"));
    }

    #[tokio::test]
    async fn test_local_storage_upload_dir_with_base_path() {
        let base = create_temp_base_path();
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());
        let engine = LocalStorageEngine::new(config);
        let expected = base.join("uploads");
        assert_eq!(engine.upload_dir(), expected);
        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn test_local_storage_set_upload_file_by_real() {
        let path = create_temp_file("internal.txt", b"internal data");
        let base = create_temp_base_path();
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());
        let mut engine = LocalStorageEngine::new(config);

        engine
            .set_upload_file_by_real(&path, "txt")
            .expect("set_upload_file_by_real failed");

        // 对齐 R5-17：内部上传文件名 = storage/{Ymd}/{basename}
        assert!(engine.is_internal());
        let file_name = engine.file_name().expect("file name should be set");
        assert!(file_name.starts_with("storage/"));
        assert!(file_name.ends_with("internal.txt"));

        let info = engine.file_info().expect("file info should be set");
        assert_eq!(info.name, "internal.txt");
        assert!(info.is_internal);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn test_local_storage_upload_internal_uses_rename() {
        let path = create_temp_file("rename_test.txt", b"rename me");
        let base = create_temp_base_path();
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());
        let mut engine = LocalStorageEngine::new(config);

        engine
            .set_upload_file_by_real(&path, "txt")
            .expect("set_upload_file_by_real failed");
        let file_name = engine.file_name().unwrap().to_string();

        // 对齐 R5-19：内部上传用 rename
        let result = engine.upload().await;
        assert!(result.is_ok(), "upload should succeed");
        // 对齐 R5-22：内部上传返回 None
        assert!(result.unwrap().is_none());

        // 原文件应被 rename 走（不存在）
        assert!(!path.exists(), "original file should be renamed away");
        // 目标文件应存在
        let target = engine.upload_dir().join(&file_name);
        assert!(target.exists(), "target file should exist: {:?}", target);

        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn test_local_storage_upload_external_returns_save_name() {
        let path = create_temp_file("external.txt", b"external data");
        let base = create_temp_base_path();
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());
        let mut engine = LocalStorageEngine::new(config);

        // 对齐 R5-23：外部上传返回 saveName
        let file = UploadedFile::new(&path, "external.txt", Some("text/plain"), Some(0), true)
            .expect("UploadedFile::new failed");
        engine
            .set_upload_file(&file)
            .expect("set_upload_file failed");

        let result = engine.upload().await;
        assert!(result.is_ok(), "upload should succeed");
        let save_name = result
            .unwrap()
            .expect("external upload should return save name");
        assert!(save_name.starts_with("storage/"));

        // 源文件应保留（外部上传用 copy）
        assert!(path.exists(), "source file should be preserved");

        // 目标文件应存在
        let target = engine.upload_dir().join(&save_name);
        assert!(target.exists(), "target file should exist: {:?}", target);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn test_local_storage_delete_existing_file() {
        let base = create_temp_base_path();
        let uploads_dir = base.join("uploads").join("storage").join("20260101");
        std::fs::create_dir_all(&uploads_dir).unwrap();
        let file_path = uploads_dir.join("delete_me.txt");
        std::fs::write(&file_path, b"delete me").unwrap();

        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());
        let mut engine = LocalStorageEngine::new(config);

        // 对齐 R5-20：delete Elvis 短路
        let result = engine.delete("storage/20260101/delete_me.txt").await;
        assert!(result.is_ok());
        assert!(result.unwrap(), "delete should return true");
        assert!(!file_path.exists(), "file should be deleted");

        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn test_local_storage_delete_nonexistent_file() {
        let base = create_temp_base_path();
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());
        let mut engine = LocalStorageEngine::new(config);

        // 对齐 R5-20：!file_exists($filePath) 为 true 时短路返回 true
        let result = engine.delete("nonexistent.txt").await;
        assert!(result.is_ok());
        assert!(result.unwrap(), "delete nonexistent should return true");

        std::fs::remove_dir_all(&base).ok();
    }

    // ---------------------------------------------------------------------
    // 组 6：云存储引擎构造测试
    // ---------------------------------------------------------------------

    #[test]
    fn test_aliyun_storage_engine_new() {
        let config = EngineConfig::new()
            .with_bucket("aliyun-bucket")
            .with_endpoint("oss-cn-hangzhou.aliyuncs.com")
            .with_access_key_id("akid")
            .with_access_key_secret("aksecret");
        let engine = AliyunStorageEngine::new(config);
        assert!(!engine.is_internal());
        assert!(engine.file_name().is_none());
    }

    #[test]
    fn test_qcloud_storage_engine_new() {
        let config = EngineConfig::new()
            .with_bucket("cos-bucket")
            .with_region("ap-guangzhou")
            .with_secret_id("sid")
            .with_secret_key("sk");
        let engine = QcloudStorageEngine::new(config);
        assert!(!engine.is_internal());
    }

    #[test]
    fn test_qiniu_storage_engine_new() {
        let config = EngineConfig::new()
            .with_bucket("qiniu-bucket")
            .with_access_key("ak")
            .with_secret_key("sk");
        let engine = QiniuStorageEngine::new(config);
        assert!(!engine.is_internal());
    }

    #[test]
    fn test_s3_storage_engine_new() {
        let config = EngineConfig::new()
            .with_bucket("s3-bucket")
            .with_region("us-east-1");
        let engine = S3StorageEngine::new(config);
        assert!(!engine.is_internal());
    }

    // ---------------------------------------------------------------------
    // 组 7：云存储引擎 set_upload_file_by_real 测试
    // ---------------------------------------------------------------------

    #[test]
    fn test_aliyun_set_upload_file_by_real() {
        let path = create_temp_file("aliyun.txt", b"aliyun");
        let config = EngineConfig::new()
            .with_bucket("bucket")
            .with_endpoint("endpoint");
        let mut engine = AliyunStorageEngine::new(config);
        engine.set_upload_file_by_real(&path, "txt").unwrap();
        assert!(engine.is_internal());
        assert!(engine.file_name().unwrap().starts_with("storage/"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_qcloud_set_upload_file_by_real() {
        let path = create_temp_file("qcloud.txt", b"qcloud");
        let config = EngineConfig::new()
            .with_bucket("bucket")
            .with_region("region");
        let mut engine = QcloudStorageEngine::new(config);
        engine.set_upload_file_by_real(&path, "txt").unwrap();
        assert!(engine.is_internal());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_qiniu_set_upload_file_by_real() {
        let path = create_temp_file("qiniu.txt", b"qiniu");
        let config = EngineConfig::new().with_bucket("bucket");
        let mut engine = QiniuStorageEngine::new(config);
        engine.set_upload_file_by_real(&path, "txt").unwrap();
        assert!(engine.is_internal());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_s3_set_upload_file_by_real() {
        let path = create_temp_file("s3.txt", b"s3");
        let config = EngineConfig::new()
            .with_bucket("bucket")
            .with_region("region");
        let mut engine = S3StorageEngine::new(config);
        engine.set_upload_file_by_real(&path, "txt").unwrap();
        assert!(engine.is_internal());
        std::fs::remove_file(&path).ok();
    }

    // ---------------------------------------------------------------------
    // 组 8：StorageDriver 工厂测试
    // ---------------------------------------------------------------------

    #[test]
    fn test_storage_driver_new_local() {
        let config = EngineConfig::new();
        let driver = StorageDriver::new(StorageEngineKind::Local, config);
        assert_eq!(driver.kind(), StorageEngineKind::Local);
        assert!(!driver.is_internal());
    }

    #[test]
    fn test_storage_driver_new_aliyun() {
        let config = EngineConfig::new();
        let driver = StorageDriver::new(StorageEngineKind::Aliyun, config);
        assert_eq!(driver.kind(), StorageEngineKind::Aliyun);
    }

    #[test]
    fn test_storage_driver_new_qcloud() {
        let config = EngineConfig::new();
        let driver = StorageDriver::new(StorageEngineKind::Qcloud, config);
        assert_eq!(driver.kind(), StorageEngineKind::Qcloud);
    }

    #[test]
    fn test_storage_driver_new_qiniu() {
        let config = EngineConfig::new();
        let driver = StorageDriver::new(StorageEngineKind::Qiniu, config);
        assert_eq!(driver.kind(), StorageEngineKind::Qiniu);
    }

    #[test]
    fn test_storage_driver_new_s3() {
        let config = EngineConfig::new();
        let driver = StorageDriver::new(StorageEngineKind::S3, config);
        assert_eq!(driver.kind(), StorageEngineKind::S3);
    }

    // ---------------------------------------------------------------------
    // 组 9：StorageDriver 代理方法测试
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn test_storage_driver_local_upload_and_delete() {
        let path = create_temp_file("driver.txt", b"driver data");
        let base = create_temp_base_path();
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());
        let mut driver = StorageDriver::new(StorageEngineKind::Local, config);

        driver
            .set_upload_file_by_real(&path, "txt")
            .expect("set_upload_file_by_real failed");
        let file_name = driver.file_name().unwrap().to_string();

        let result = driver.upload().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());

        // 删除刚上传的文件
        let del_result = driver.delete(&file_name).await;
        assert!(del_result.is_ok());
        assert!(del_result.unwrap());

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn test_storage_driver_aliyun_upload() {
        let path = create_temp_file("aliyun_driver.txt", b"aliyun data");
        let config = EngineConfig::new()
            .with_bucket("test-bucket")
            .with_endpoint("oss-cn-hangzhou.aliyuncs.com");
        let mut driver = StorageDriver::new(StorageEngineKind::Aliyun, config);

        driver
            .set_upload_file_by_real(&path, "txt")
            .expect("set_upload_file_by_real failed");
        let result = driver.upload().await;
        assert!(result.is_ok(), "upload should succeed");
        // 对齐 R5-22：云存储返回 Some(save_name)
        assert!(result.unwrap().is_some());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_storage_driver_error_after_failed_delete() {
        // 测试云存储删除（内存模拟，删除不存在的 key 不会报错）
        let config = EngineConfig::new()
            .with_bucket("bucket")
            .with_endpoint("endpoint");
        let mut driver = StorageDriver::new(StorageEngineKind::Aliyun, config);
        let result = driver.delete("nonexistent.txt").await;
        assert!(result.is_ok());
    }

    // ---------------------------------------------------------------------
    // 组 10：PHP 行为对齐 R5 测试
    // ---------------------------------------------------------------------

    /// R5-16：`buildSaveName` 格式 = `storage/{Ymd}/{YmdHis}{md5(realPath)[0..5]}{rand(0..9999) padded 4}.{ext}`
    #[test]
    fn test_r5_16_build_save_name_format() {
        let path = Path::new("/tmp/r5_16_test.jpg");
        let name = build_save_name(path, "jpg");

        // 验证格式：storage/{Ymd}/{YmdHis}{md5[5]}{rand[4]}.jpg
        assert!(name.starts_with("storage/"), "name = {}", name);
        assert!(name.ends_with(".jpg"), "name = {}", name);

        let parts: Vec<&str> = name.split('/').collect();
        assert_eq!(parts.len(), 3, "should have 3 parts: storage, Ymd, rest");

        // Ymd = 8 位数字
        let ymd = parts[1];
        assert_eq!(ymd.len(), 8, "Ymd should be 8 digits");
        assert!(ymd.chars().all(|c| c.is_ascii_digit()));

        // 最后部分：YmdHis(14) + md5(5) + rand(4) + ".jpg"(4) = 27
        let last = parts[2];
        assert_eq!(
            last.len(),
            27,
            "last part length = {}, last = {}",
            last.len(),
            last
        );
    }

    /// R5-17：`setUploadFileByReal` 的 `fileName` = `storage/{Ymd}/{basename}`
    #[test]
    fn test_r5_17_internal_save_name_uses_basename() {
        let path = Path::new("/tmp/r5_17_test.png");
        let name = build_internal_save_name(path);

        // 格式：storage/{Ymd}/{basename}
        assert!(name.starts_with("storage/"));
        assert!(name.ends_with("r5_17_test.png"));

        let parts: Vec<&str> = name.split('/').collect();
        assert_eq!(parts.len(), 3, "should have 3 parts");
        // Ymd = 8 位数字
        assert_eq!(parts[1].len(), 8);
        assert!(parts[1].chars().all(|c| c.is_ascii_digit()));
        // basename 保持不变
        assert_eq!(parts[2], "r5_17_test.png");
    }

    /// R5-18：`getRealPath` 区分 isInternal（内部用 tmp_name，外部用 UploadedFile.path）
    #[tokio::test]
    async fn test_r5_18_real_path_internal_vs_external() {
        let internal_path = create_temp_file("internal.txt", b"internal");
        let external_path = create_temp_file("external.txt", b"external");
        let base = create_temp_base_path();
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());

        // 内部上传：real_path 应为 file_info.tmp_name（即传入的 file_path）
        let mut engine = LocalStorageEngine::new(config.clone());
        engine
            .set_upload_file_by_real(&internal_path, "txt")
            .unwrap();
        assert!(engine.is_internal());
        let real_path = engine.real_path().expect("real_path should be set");
        assert_eq!(real_path, internal_path);

        // 外部上传：real_path 应为 UploadedFile.path
        let mut engine2 = LocalStorageEngine::new(config);
        let file = UploadedFile::new(
            &external_path,
            "external.txt",
            Some("text/plain"),
            Some(0),
            true,
        )
        .expect("UploadedFile::new failed");
        engine2.set_upload_file(&file).unwrap();
        assert!(!engine2.is_internal());
        let real_path = engine2.real_path().expect("real_path should be set");
        assert_eq!(real_path, external_path);

        std::fs::remove_file(&internal_path).ok();
        std::fs::remove_file(&external_path).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    /// R5-19：`Local::upload` 区分 isInternal（internal 用 rename，external 用 copy）
    #[tokio::test]
    async fn test_r5_19_upload_internal_rename_external_copy() {
        let internal_path = create_temp_file("r5_19_internal.txt", b"internal");
        let external_path = create_temp_file("r5_19_external.txt", b"external");
        let base = create_temp_base_path();
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());

        // 内部上传：用 rename，原文件不存在
        let mut engine = LocalStorageEngine::new(config.clone());
        engine
            .set_upload_file_by_real(&internal_path, "txt")
            .unwrap();
        engine.upload().await.expect("internal upload failed");
        assert!(!internal_path.exists(), "rename should move file away");

        // 外部上传：用 copy，原文件保留
        let mut engine2 = LocalStorageEngine::new(config);
        let file = UploadedFile::new(
            &external_path,
            "r5_19_external.txt",
            Some("text/plain"),
            Some(0),
            true,
        )
        .expect("UploadedFile::new failed");
        engine2.set_upload_file(&file).unwrap();
        engine2.upload().await.expect("external upload failed");
        assert!(external_path.exists(), "copy should preserve source");

        std::fs::remove_file(&internal_path).ok();
        std::fs::remove_file(&external_path).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    /// R5-20：`Local::delete` Elvis 短路 `!file_exists($filePath) ?: unlink($filePath)`
    #[tokio::test]
    async fn test_r5_20_delete_elvis_short_circuit() {
        let base = create_temp_base_path();
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());
        let mut engine = LocalStorageEngine::new(config);

        // 文件不存在时：!file_exists 为 true，短路返回 true（不调用 unlink）
        let result = engine.delete("nonexistent_file.txt").await;
        assert!(result.is_ok());
        assert!(result.unwrap(), "delete nonexistent should return true");

        // 文件存在时：!file_exists 为 false，调用 unlink 返回结果
        let uploads_dir = base.join("uploads").join("storage").join("20260101");
        std::fs::create_dir_all(&uploads_dir).unwrap();
        let file_path = uploads_dir.join("exists.txt");
        std::fs::write(&file_path, b"exists").unwrap();

        let result = engine.delete("storage/20260101/exists.txt").await;
        assert!(result.is_ok());
        assert!(result.unwrap(), "delete existing should return true");
        assert!(!file_path.exists(), "file should be deleted");

        std::fs::remove_dir_all(&base).ok();
    }

    /// R5-21：`Local::uploadByInternal` 失败设置 `error='upload write error'`
    #[tokio::test]
    async fn test_r5_21_internal_upload_failure_sets_error() {
        // 创建一个不存在的源文件路径，触发 rename 失败
        let nonexistent_path = std::env::temp_dir().join(format!(
            "nonexistent_{}.txt",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        // 注意：不创建文件，直接调用 set_upload_file_by_real 会失败
        // 所以我们创建文件然后立即删除，模拟 rename 失败
        let path = create_temp_file("will_fail.txt", b"will fail");
        let base = create_temp_base_path();
        // 设置一个只读的 base_path 使 rename 失败（Windows 上可能不生效，改为其他方式）
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());
        let mut engine = LocalStorageEngine::new(config);

        engine
            .set_upload_file_by_real(&path, "txt")
            .expect("set_upload_file_by_real failed");

        // 删除源文件，使 rename 失败
        std::fs::remove_file(&path).unwrap();

        let result = engine.upload().await;
        // rename 应该失败（源文件不存在）
        assert!(result.is_err(), "upload should fail when source missing");
        // 错误信息应包含 'upload write error'（对齐 PHP）
        let error_msg = engine.error().unwrap_or("");
        assert!(
            error_msg.contains("upload write error"),
            "error should contain 'upload write error', got: {}",
            error_msg
        );

        let _ = nonexistent_path;
        std::fs::remove_dir_all(&base).ok();
    }

    /// R5-22：云存储 `upload` 成功返回 `true`（PHP），Rust 端返回 `Ok(Some(save_name))`
    #[tokio::test]
    async fn test_r5_22_cloud_upload_returns_save_name() {
        let path = create_temp_file("r5_22.txt", b"cloud upload");
        let config = EngineConfig::new()
            .with_bucket("r5_22_bucket")
            .with_endpoint("oss-cn-hangzhou.aliyuncs.com");
        let mut driver = StorageDriver::new(StorageEngineKind::Aliyun, config);

        driver
            .set_upload_file_by_real(&path, "txt")
            .expect("set_upload_file_by_real failed");
        let file_name = driver.file_name().unwrap().to_string();

        let result = driver.upload().await;
        assert!(result.is_ok(), "upload should succeed");
        let returned = result.unwrap().expect("cloud upload should return Some");
        assert_eq!(returned, file_name, "should return save name");

        std::fs::remove_file(&path).ok();
    }

    /// R5-23：`Local::uploadByExternal` 返回 `saveName`（putFile 返回路径）
    #[tokio::test]
    async fn test_r5_23_local_external_upload_returns_save_name() {
        let path = create_temp_file("r5_23.txt", b"external save name");
        let base = create_temp_base_path();
        let config = EngineConfig::new().with_base_path(base.to_string_lossy().to_string());
        let mut engine = LocalStorageEngine::new(config);

        let file = UploadedFile::new(&path, "r5_23.txt", Some("text/plain"), Some(0), true)
            .expect("UploadedFile::new failed");
        engine.set_upload_file(&file).unwrap();
        let expected_name = engine.file_name().unwrap().to_string();

        let result = engine.upload().await;
        assert!(result.is_ok(), "upload should succeed");
        let save_name = result
            .unwrap()
            .expect("external upload should return save name");
        assert_eq!(save_name, expected_name, "should return save name");

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir_all(&base).ok();
    }
}
