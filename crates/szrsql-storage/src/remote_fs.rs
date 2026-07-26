//! 远程文件系统 — Phase 7f.2
//!
//! 支持 S3 和 HTTP/HTTPS 远程文件读取，集成 ExternalReader 实现谓词下推。
//!
//! # 设计
//!
//! - **S3** — 使用 AWS Signature V4 签名，支持 path-style URL
//! - **HTTP/HTTPS** — 标准 HTTP GET + Range requests
//! - **Parquet** — 通过 ChunkReader trait 实现原生 range requests（高效随机访问）
//! - **Arrow IPC / CSV / JSONLines** — 通过 RemoteFileReader 实现 Read + Seek
//!
//! # 验证标准
//!
//! - `SELECT * FROM s3('s3://bucket/data.parquet')` → 从 S3 读取 → 谓词下推到 Parquet
//! - HTTPFS 读取远程 CSV

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use bytes::Bytes;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::external_format::{
    ArrowReader, CsvReader, ExternalFormatError, ExternalReader, ExternalSchema, JsonLinesReader,
    ParquetReader, ReadOptions,
};

type HmacSha256 = Hmac<Sha256>;

/// 空 payload 的 SHA256 哈希（GET 请求无 body）
const EMPTY_PAYLOAD_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// 远程读取的 chunk 大小（8MB）
const CHUNK_SIZE: usize = 8 * 1024 * 1024;

// =====================================================================
//  错误类型
// =====================================================================

/// 远程文件系统错误
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RemoteFsError {
    /// URL 格式无效
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    /// 不支持的协议
    #[error("unsupported scheme: {0}")]
    UnsupportedScheme(String),
    /// HTTP 错误（状态码非 2xx）
    #[error("HTTP error: status={status}, message={message}")]
    HttpError { status: u16, message: String },
    /// 网络错误（连接失败、DNS 解析失败等）
    #[error("network error: {0}")]
    NetworkError(String),
    /// S3 错误
    #[error("S3 error: {0}")]
    S3Error(String),
    /// 需要认证（S3 但未提供配置）
    #[error("authentication required")]
    AuthenticationRequired,
    /// 认证失败
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),
    /// 请求的 range 超出文件大小
    #[error("range not satisfiable: requested={requested}, content_length={content_length}")]
    RangeNotSatisfiable {
        requested: String,
        content_length: u64,
    },
    /// IO 错误
    #[error("IO error: {0}")]
    IoError(String),
}

impl From<RemoteFsError> for ExternalFormatError {
    fn from(e: RemoteFsError) -> Self {
        ExternalFormatError::IoError(e.to_string())
    }
}

impl From<ExternalFormatError> for RemoteFsError {
    fn from(e: ExternalFormatError) -> Self {
        RemoteFsError::IoError(e.to_string())
    }
}

impl From<std::io::Error> for RemoteFsError {
    fn from(e: std::io::Error) -> Self {
        RemoteFsError::IoError(e.to_string())
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 将字节数组编码为 hex 字符串
fn hex_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// 计算 SHA256 并返回 hex 字符串
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(&hasher.finalize())
}

/// 计算 HMAC-SHA256
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// 从 ureq Response 读取全部字节
fn read_response_bytes(response: ureq::Response) -> Result<Vec<u8>, RemoteFsError> {
    let mut reader = response.into_reader();
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|e| RemoteFsError::NetworkError(e.to_string()))?;
    Ok(bytes)
}

// =====================================================================
//  RemotePath — URL 解析
// =====================================================================

/// 远程路径协议
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteScheme {
    /// S3 协议
    S3,
    /// HTTP 协议
    Http,
    /// HTTPS 协议
    Https,
}

/// 解析后的远程路径
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePath {
    scheme: RemoteScheme,
    bucket: Option<String>,
    host: Option<String>,
    key: String,
}

impl RemotePath {
    /// 解析远程 URL
    ///
    /// 支持格式：
    /// - `s3://bucket/key`
    /// - `s3://bucket/path/to/key`
    /// - `http://host/path`
    /// - `https://host/path`
    pub fn parse(url: &str) -> Result<Self, RemoteFsError> {
        if let Some(rest) = url.strip_prefix("s3://") {
            let (bucket, key) = rest
                .split_once('/')
                .ok_or_else(|| RemoteFsError::InvalidUrl(format!("S3 URL missing key: {url}")))?;
            if bucket.is_empty() || key.is_empty() {
                return Err(RemoteFsError::InvalidUrl(format!(
                    "S3 URL empty bucket or key: {url}"
                )));
            }
            Ok(Self {
                scheme: RemoteScheme::S3,
                bucket: Some(bucket.to_string()),
                host: None,
                key: key.to_string(),
            })
        } else if let Some(rest) = url.strip_prefix("https://") {
            let (host, key) = rest.split_once('/').ok_or_else(|| {
                RemoteFsError::InvalidUrl(format!("HTTPS URL missing path: {url}"))
            })?;
            Ok(Self {
                scheme: RemoteScheme::Https,
                bucket: None,
                host: Some(host.to_string()),
                key: format!("/{key}"),
            })
        } else if let Some(rest) = url.strip_prefix("http://") {
            let (host, key) = rest.split_once('/').ok_or_else(|| {
                RemoteFsError::InvalidUrl(format!("HTTP URL missing path: {url}"))
            })?;
            Ok(Self {
                scheme: RemoteScheme::Http,
                bucket: None,
                host: Some(host.to_string()),
                key: format!("/{key}"),
            })
        } else {
            Err(RemoteFsError::UnsupportedScheme(url.to_string()))
        }
    }

    /// 返回协议
    pub fn scheme(&self) -> RemoteScheme {
        self.scheme
    }

    /// 返回 bucket 名（仅 S3）
    pub fn bucket(&self) -> Option<&str> {
        self.bucket.as_deref()
    }

    /// 返回 host（仅 HTTP/HTTPS）
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// 返回 key/path
    pub fn key(&self) -> &str {
        &self.key
    }
}

// =====================================================================
//  S3Config — S3 配置
// =====================================================================

/// S3 配置（认证信息 + endpoint）
#[derive(Debug, Clone)]
pub struct S3Config {
    /// AWS Access Key ID
    pub access_key_id: String,
    /// AWS Secret Access Key
    pub secret_access_key: String,
    /// AWS Region
    pub region: String,
    /// 自定义 endpoint（MinIO 等兼容 S3 的服务）
    pub endpoint: Option<String>,
}

impl Default for S3Config {
    fn default() -> Self {
        Self {
            access_key_id: std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_default(),
            secret_access_key: std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default(),
            region: std::env::var("AWS_DEFAULT_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
            endpoint: std::env::var("AWS_ENDPOINT").ok(),
        }
    }
}

// =====================================================================
//  SigV4 — AWS Signature Version 4
// =====================================================================

/// 签名后的请求
#[derive(Debug, Clone)]
pub struct SignedRequest {
    /// Authorization header 值
    pub authorization: String,
    /// AWS 日期时间（YYYYMMDDTHHMMSSZ）
    pub amz_date: String,
}

/// AWS Signature V4 签名器
pub struct SigV4 {
    access_key: String,
    secret_key: String,
    region: String,
    service: String,
}

impl SigV4 {
    /// 创建签名器
    pub fn new(access_key: &str, secret_key: &str, region: &str, service: &str) -> Self {
        Self {
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            region: region.to_string(),
            service: service.to_string(),
        }
    }

    /// 派生签名密钥
    ///
    /// kDate = HMAC("AWS4" + SecretKey, Date)
    /// kRegion = HMAC(kDate, Region)
    /// kService = HMAC(kRegion, Service)
    /// kSigning = HMAC(kService, "aws4_request")
    fn signing_key(&self, date_short: &str) -> Vec<u8> {
        let k_date = hmac_sha256(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date_short.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, self.region.as_bytes());
        let k_service = hmac_sha256(&k_region, self.service.as_bytes());
        hmac_sha256(&k_service, b"aws4_request")
    }

    /// 对请求进行签名
    ///
    /// # 参数
    /// - `method` — HTTP 方法（GET/HEAD/PUT/POST）
    /// - `host` — Host header 值
    /// - `path` — Canonical URI（请求路径）
    /// - `query` — Canonical Query String（已排序）
    /// - `amz_date` — AWS 日期时间（YYYYMMDDTHHMMSSZ）
    /// - `payload_hash` — payload 的 SHA256 hex
    /// - `extra_headers` — 额外需要签名的 header（如 Range）
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        &self,
        method: &str,
        host: &str,
        path: &str,
        query: &str,
        amz_date: &str,
        payload_hash: &str,
        extra_headers: &[(String, String)],
    ) -> SignedRequest {
        let date_short = &amz_date[..8];

        // 构造 headers（小写，按 key 排序）
        let mut headers: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        headers.insert("host".to_string(), host.to_string());
        headers.insert("x-amz-content-sha256".to_string(), payload_hash.to_string());
        headers.insert("x-amz-date".to_string(), amz_date.to_string());
        for (k, v) in extra_headers {
            headers.insert(k.to_lowercase(), v.clone());
        }

        // Canonical Headers（每个 header 后跟 \n）
        let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();

        // Signed Headers（分号分隔，按 key 排序）
        let signed_headers: String = headers.keys().cloned().collect::<Vec<_>>().join(";");

        // Canonical Request
        let canonical_request = format!(
            "{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );

        let hashed_canonical_request = sha256_hex(canonical_request.as_bytes());

        // Credential Scope
        let credential_scope =
            format!("{date_short}/{}/{}/aws4_request", self.region, self.service);

        // String to Sign
        let string_to_sign =
            format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{hashed_canonical_request}");

        // Signing Key
        let signing_key = self.signing_key(date_short);

        // Signature
        let signature = hmac_sha256(&signing_key, string_to_sign.as_bytes());
        let signature_hex = hex_encode(&signature);

        // Authorization Header
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature_hex}",
            self.access_key
        );

        SignedRequest {
            authorization,
            amz_date: amz_date.to_string(),
        }
    }
}

// =====================================================================
//  RemoteBackend trait — 统一后端接口
// =====================================================================

/// 远程后端接口
///
/// 统一 S3 和 HTTP/HTTPS 后端的访问接口。
/// 实现此 trait 的类型可以作为 RemoteChunkReader 和 RemoteFileReader 的数据源。
pub trait RemoteBackend: Send + Sync {
    /// 返回文件总长度（字节）
    fn content_length(&self) -> Result<u64, RemoteFsError>;

    /// 获取指定范围的数据 [start, start+length)
    fn fetch_range(&self, start: u64, length: usize) -> Result<Bytes, RemoteFsError>;
}

// =====================================================================
//  HttpBackend — HTTP/HTTPS 后端
// =====================================================================

/// HTTP/HTTPS 后端
pub struct HttpBackend {
    url: String,
}

impl HttpBackend {
    /// 从 RemotePath 创建 HTTP 后端
    pub fn new(path: &RemotePath) -> Result<Self, RemoteFsError> {
        let host = path
            .host()
            .ok_or_else(|| RemoteFsError::InvalidUrl("HTTP URL missing host".into()))?;
        let scheme = match path.scheme() {
            RemoteScheme::Http => "http",
            RemoteScheme::Https => "https",
            _ => {
                return Err(RemoteFsError::UnsupportedScheme(
                    "expected http/https".into(),
                ))
            }
        };
        let url = format!("{scheme}://{host}{}", path.key());
        Ok(Self { url })
    }

    /// 直接从 URL 创建 HTTP 后端
    pub fn from_url(url: &str) -> Result<Self, RemoteFsError> {
        let path = RemotePath::parse(url)?;
        Self::new(&path)
    }
}

impl RemoteBackend for HttpBackend {
    fn content_length(&self) -> Result<u64, RemoteFsError> {
        let resp = ureq::head(&self.url).call();
        match resp {
            Ok(response) => {
                let len = response
                    .header("Content-Length")
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or_else(|| RemoteFsError::HttpError {
                        status: response.status(),
                        message: "missing Content-Length".into(),
                    })?;
                Ok(len)
            }
            Err(ureq::Error::Status(code, response)) => {
                let message = response.into_string().unwrap_or_default();
                Err(RemoteFsError::HttpError {
                    status: code,
                    message,
                })
            }
            Err(e) => Err(RemoteFsError::NetworkError(e.to_string())),
        }
    }

    fn fetch_range(&self, start: u64, length: usize) -> Result<Bytes, RemoteFsError> {
        let end = start + length as u64 - 1;
        let range_header = format!("bytes={start}-{end}");

        let resp = ureq::get(&self.url).set("Range", &range_header).call();
        match resp {
            Ok(response) => {
                let bytes = read_response_bytes(response)?;
                Ok(Bytes::from(bytes))
            }
            Err(ureq::Error::Status(code, response)) => {
                let message = response.into_string().unwrap_or_default();
                Err(RemoteFsError::HttpError {
                    status: code,
                    message,
                })
            }
            Err(e) => Err(RemoteFsError::NetworkError(e.to_string())),
        }
    }
}

// =====================================================================
//  S3Backend — S3 后端
// =====================================================================

/// S3 后端（使用 AWS SigV4 签名）
pub struct S3Backend {
    url: String,
    host: String,
    canonical_uri: String,
    config: S3Config,
}

impl S3Backend {
    /// 从 RemotePath 和 S3Config 创建 S3 后端
    pub fn new(path: &RemotePath, config: S3Config) -> Result<Self, RemoteFsError> {
        let bucket = path
            .bucket()
            .ok_or_else(|| RemoteFsError::InvalidUrl("S3 URL missing bucket".into()))?;

        if config.access_key_id.is_empty() || config.secret_access_key.is_empty() {
            return Err(RemoteFsError::AuthenticationRequired);
        }

        let (url_base, host) = if let Some(endpoint) = &config.endpoint {
            let endpoint = endpoint.trim_end_matches('/');
            let host = endpoint
                .strip_prefix("https://")
                .or_else(|| endpoint.strip_prefix("http://"))
                .unwrap_or(endpoint);
            (endpoint.to_string(), host.to_string())
        } else {
            let host = format!("s3.{}.amazonaws.com", config.region);
            (format!("https://{host}"), host)
        };

        let canonical_uri = format!("/{bucket}/{}", path.key());
        let url = format!("{url_base}{canonical_uri}");

        Ok(Self {
            url,
            host,
            canonical_uri,
            config,
        })
    }
}

impl RemoteBackend for S3Backend {
    fn content_length(&self) -> Result<u64, RemoteFsError> {
        let sigv4 = SigV4::new(
            &self.config.access_key_id,
            &self.config.secret_access_key,
            &self.config.region,
            "s3",
        );
        let amz_date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let signed = sigv4.sign(
            "HEAD",
            &self.host,
            &self.canonical_uri,
            "",
            &amz_date,
            EMPTY_PAYLOAD_HASH,
            &[],
        );

        let resp = ureq::head(&self.url)
            .set("Authorization", &signed.authorization)
            .set("x-amz-date", &signed.amz_date)
            .set("x-amz-content-sha256", EMPTY_PAYLOAD_HASH)
            .call();

        match resp {
            Ok(response) => {
                let len = response
                    .header("Content-Length")
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or_else(|| RemoteFsError::S3Error("missing Content-Length".into()))?;
                Ok(len)
            }
            Err(ureq::Error::Status(403, _)) => Err(RemoteFsError::AuthenticationFailed(
                "S3 returned 403 Forbidden".into(),
            )),
            Err(ureq::Error::Status(code, response)) => {
                let message = response.into_string().unwrap_or_default();
                Err(RemoteFsError::HttpError {
                    status: code,
                    message,
                })
            }
            Err(e) => Err(RemoteFsError::NetworkError(e.to_string())),
        }
    }

    fn fetch_range(&self, start: u64, length: usize) -> Result<Bytes, RemoteFsError> {
        let end = start + length as u64 - 1;
        let range_header = format!("bytes={start}-{end}");

        let sigv4 = SigV4::new(
            &self.config.access_key_id,
            &self.config.secret_access_key,
            &self.config.region,
            "s3",
        );
        let amz_date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let signed = sigv4.sign(
            "GET",
            &self.host,
            &self.canonical_uri,
            "",
            &amz_date,
            EMPTY_PAYLOAD_HASH,
            &[("range".to_string(), range_header.clone())],
        );

        let resp = ureq::get(&self.url)
            .set("Authorization", &signed.authorization)
            .set("x-amz-date", &signed.amz_date)
            .set("x-amz-content-sha256", EMPTY_PAYLOAD_HASH)
            .set("Range", &range_header)
            .call();

        match resp {
            Ok(response) => {
                let bytes = read_response_bytes(response)?;
                Ok(Bytes::from(bytes))
            }
            Err(ureq::Error::Status(403, _)) => Err(RemoteFsError::AuthenticationFailed(
                "S3 returned 403 Forbidden".into(),
            )),
            Err(ureq::Error::Status(code, response)) => {
                let message = response.into_string().unwrap_or_default();
                Err(RemoteFsError::HttpError {
                    status: code,
                    message,
                })
            }
            Err(e) => Err(RemoteFsError::NetworkError(e.to_string())),
        }
    }
}

// =====================================================================
//  RemoteChunkReader — Parquet ChunkReader 实现
// =====================================================================

/// 远程 ChunkReader（实现 parquet ChunkReader trait）
///
/// 用于 Parquet 读取器，支持原生 range requests（高效随机访问）。
/// Parquet 读取器通过 get_bytes/get_read 按需读取数据块，而非整个文件。
pub struct RemoteChunkReader {
    backend: Arc<dyn RemoteBackend>,
    length: u64,
}

impl RemoteChunkReader {
    /// 创建 RemoteChunkReader
    pub fn new(backend: Arc<dyn RemoteBackend>) -> Result<Self, RemoteFsError> {
        let length = backend.content_length()?;
        Ok(Self { backend, length })
    }
}

impl parquet::file::reader::Length for RemoteChunkReader {
    fn len(&self) -> u64 {
        self.length
    }
}

impl parquet::file::reader::ChunkReader for RemoteChunkReader {
    type T = std::io::Cursor<Bytes>;

    fn get_read(&self, start: u64) -> parquet::errors::Result<Self::T> {
        let remaining = self.length.saturating_sub(start);
        let bytes = self
            .backend
            .fetch_range(start, remaining as usize)
            .map_err(|e| parquet::errors::ParquetError::General(e.to_string()))?;
        Ok(std::io::Cursor::new(bytes))
    }

    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<Bytes> {
        self.backend
            .fetch_range(start, length)
            .map_err(|e| parquet::errors::ParquetError::General(e.to_string()))
    }
}

// =====================================================================
//  RemoteFileReader — Read + Seek 实现
// =====================================================================

/// 远程文件读取器（实现 Read + Seek）
///
/// 内部使用 chunk cache 策略：每次读取 CHUNK_SIZE (8MB) 数据。
/// Seek 时如果目标位置不在当前 chunk 范围内，清空 cache，下次 Read 时重新加载。
pub struct RemoteFileReader {
    backend: Arc<dyn RemoteBackend>,
    length: u64,
    pos: u64,
    chunk_start: u64,
    chunk: Option<Bytes>,
}

impl RemoteFileReader {
    /// 创建 RemoteFileReader
    pub fn new(backend: Arc<dyn RemoteBackend>) -> Result<Self, RemoteFsError> {
        let length = backend.content_length()?;
        Ok(Self {
            backend,
            length,
            pos: 0,
            chunk_start: 0,
            chunk: None,
        })
    }

    /// 返回文件总长度
    pub fn len(&self) -> u64 {
        self.length
    }

    /// 文件是否为空
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// 返回当前位置
    pub fn position(&self) -> u64 {
        self.pos
    }
}

impl Read for RemoteFileReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.length {
            return Ok(0);
        }

        // 如果当前位置不在缓冲区范围内，重新加载
        let need_reload = self.chunk.as_ref().is_none_or(|c| {
            self.pos < self.chunk_start || self.pos >= self.chunk_start + c.len() as u64
        });

        if need_reload {
            let remaining = (self.length - self.pos) as usize;
            let read_len = remaining.min(CHUNK_SIZE);
            let bytes = self
                .backend
                .fetch_range(self.pos, read_len)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            self.chunk_start = self.pos;
            self.chunk = Some(bytes);
        }

        let chunk = self.chunk.as_ref().expect("chunk loaded");
        let offset = (self.pos - self.chunk_start) as usize;
        let available = chunk.len() - offset;
        let to_read = available.min(buf.len());

        buf[..to_read].copy_from_slice(&chunk[offset..offset + to_read]);
        self.pos += to_read as u64;
        Ok(to_read)
    }
}

impl Seek for RemoteFileReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(p) => p,
            SeekFrom::End(p) => {
                if p >= 0 {
                    self.length.checked_add(p as u64).ok_or_else(|| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek overflow")
                    })?
                } else {
                    let abs = (-p) as u64;
                    self.length.checked_sub(abs).ok_or_else(|| {
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start")
                    })?
                }
            }
            SeekFrom::Current(p) => {
                if p < 0 {
                    let abs = (-p) as u64;
                    if abs > self.pos {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "seek before start",
                        ));
                    }
                    self.pos - abs
                } else {
                    self.pos + p as u64
                }
            }
        };

        if new_pos > self.length {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek beyond end",
            ));
        }

        // 如果新位置不在当前 chunk 范围内，清空 cache
        let in_chunk = self.chunk.as_ref().is_some_and(|c| {
            new_pos >= self.chunk_start && new_pos < self.chunk_start + c.len() as u64
        });

        if !in_chunk {
            self.chunk = None;
        }

        self.pos = new_pos;
        Ok(new_pos)
    }
}

// =====================================================================
//  RemoteFileFormat — 远程文件格式检测
// =====================================================================

/// 远程文件格式（根据 URL 扩展名推断）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteFileFormat {
    /// Arrow IPC 格式（.arrow）
    Arrow,
    /// Parquet 格式（.parquet）
    Parquet,
    /// CSV 格式（.csv）
    Csv,
    /// JSON Lines 格式（.jsonl / .json）
    JsonLines,
}

impl RemoteFileFormat {
    /// 根据文件扩展名推断格式
    ///
    /// 支持扩展名：`.arrow`、`.parquet`、`.csv`、`.jsonl`、`.json`
    pub fn from_extension(url: &str) -> Result<Self, RemoteFsError> {
        let lower = url.to_lowercase();
        if lower.ends_with(".arrow") || lower.ends_with(".ipc") {
            Ok(RemoteFileFormat::Arrow)
        } else if lower.ends_with(".parquet") || lower.ends_with(".pq") {
            Ok(RemoteFileFormat::Parquet)
        } else if lower.ends_with(".csv") {
            Ok(RemoteFileFormat::Csv)
        } else if lower.ends_with(".jsonl") || lower.ends_with(".ndjson") {
            Ok(RemoteFileFormat::JsonLines)
        } else {
            Err(RemoteFsError::InvalidUrl(format!(
                "无法识别文件扩展名: {url}"
            )))
        }
    }
}

// =====================================================================
//  read_remote_file — 统一入口函数
// =====================================================================

/// 从远程 URL 读取文件
///
/// 根据 URL 协议自动选择后端：
/// - `s3://` → S3Backend（需要 S3Config）
/// - `http://` / `https://` → HttpBackend
///
/// 根据文件扩展名自动选择格式：
/// - `.arrow` → ArrowReader
/// - `.parquet` → ParquetReader（支持谓词下推）
/// - `.csv` → CsvReader
/// - `.jsonl` → JsonLinesReader
///
/// # 参数
/// - `url` — 远程文件 URL
/// - `options` — 读取选项（列裁剪 + 谓词下推）
/// - `s3_config` — S3 配置（S3 URL 时必需）
pub fn read_remote_file(
    url: &str,
    options: &ReadOptions,
    s3_config: Option<&S3Config>,
) -> Result<(ExternalSchema, Vec<crate::external_format::ExternalRow>), RemoteFsError> {
    let path = RemotePath::parse(url)?;

    let backend: Arc<dyn RemoteBackend> = match path.scheme() {
        RemoteScheme::S3 => {
            let config = s3_config
                .cloned()
                .ok_or(RemoteFsError::AuthenticationRequired)?;
            Arc::new(S3Backend::new(&path, config)?)
        }
        RemoteScheme::Http | RemoteScheme::Https => Arc::new(HttpBackend::new(&path)?),
    };

    let format = RemoteFileFormat::from_extension(url)?;

    let reader: Box<dyn ExternalReader> = match format {
        RemoteFileFormat::Parquet => {
            let chunk_reader = RemoteChunkReader::new(backend)?;
            if let Some(predicate) = &options.predicate {
                Box::new(ParquetReader::from_chunk_reader_with_predicate(
                    chunk_reader,
                    predicate,
                )?)
            } else {
                Box::new(ParquetReader::from_chunk_reader(chunk_reader)?)
            }
        }
        RemoteFileFormat::Arrow => {
            let reader = RemoteFileReader::new(backend)?;
            Box::new(ArrowReader::from_reader(reader)?)
        }
        RemoteFileFormat::Csv => {
            let reader = RemoteFileReader::new(backend)?;
            Box::new(CsvReader::from_reader(reader)?)
        }
        RemoteFileFormat::JsonLines => {
            let reader = RemoteFileReader::new(backend)?;
            Box::new(JsonLinesReader::from_reader(reader)?)
        }
    };

    let schema = reader.schema().clone();
    let rows = reader.read(options)?;
    Ok((schema, rows))
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external_format::{
        write_arrow_file, write_csv_file, write_json_lines_file, write_parquet_file,
        ExternalColumn, ExternalRow, ExternalSchema, ExternalType, ExternalValue, Predicate,
        ReadOptions,
    };
    use std::fs;

    // -----------------------------------------------------------------
    //  MockBackend — 测试用的内存后端
    // -----------------------------------------------------------------

    struct MockBackend {
        data: Bytes,
    }

    impl MockBackend {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data: Bytes::from(data),
            }
        }
    }

    impl RemoteBackend for MockBackend {
        fn content_length(&self) -> Result<u64, RemoteFsError> {
            Ok(self.data.len() as u64)
        }

        fn fetch_range(&self, start: u64, length: usize) -> Result<Bytes, RemoteFsError> {
            let start = start as usize;
            if start > self.data.len() {
                return Err(RemoteFsError::RangeNotSatisfiable {
                    requested: format!("bytes={start}-{}", start + length - 1),
                    content_length: self.data.len() as u64,
                });
            }
            let end = (start + length).min(self.data.len());
            Ok(self.data.slice(start..end))
        }
    }

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    /// 创建临时文件路径
    fn create_temp_path(suffix: &str) -> String {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let filename = format!("szrsql_remote_test_{pid}_{timestamp}{suffix}");
        dir.join(filename).to_string_lossy().to_string()
    }

    /// 创建测试 schema（id: Int64, name: Text, active: Bool）
    fn test_schema() -> ExternalSchema {
        ExternalSchema::from_columns(vec![
            ExternalColumn::new("id", ExternalType::Int64),
            ExternalColumn::new("name", ExternalType::Text),
            ExternalColumn::new("active", ExternalType::Bool),
        ])
    }

    /// 创建测试行数据
    fn test_rows() -> Vec<ExternalRow> {
        vec![
            ExternalRow::from_values(vec![
                ExternalValue::Int64(1),
                ExternalValue::Text("alice".into()),
                ExternalValue::Bool(true),
            ]),
            ExternalRow::from_values(vec![
                ExternalValue::Int64(2),
                ExternalValue::Text("bob".into()),
                ExternalValue::Bool(false),
            ]),
            ExternalRow::from_values(vec![
                ExternalValue::Int64(3),
                ExternalValue::Text("charlie".into()),
                ExternalValue::Bool(true),
            ]),
            ExternalRow::from_values(vec![
                ExternalValue::Int64(4),
                ExternalValue::Text("dave".into()),
                ExternalValue::Bool(false),
            ]),
            ExternalRow::from_values(vec![
                ExternalValue::Int64(5),
                ExternalValue::Text("eve".into()),
                ExternalValue::Bool(true),
            ]),
        ]
    }

    // -----------------------------------------------------------------
    //  1. RemotePath URL 解析测试
    // -----------------------------------------------------------------

    #[test]
    fn test_parse_s3_url() {
        let path = RemotePath::parse("s3://mybucket/data.parquet").unwrap();
        assert_eq!(path.scheme(), RemoteScheme::S3);
        assert_eq!(path.bucket(), Some("mybucket"));
        assert_eq!(path.key(), "data.parquet");
    }

    #[test]
    fn test_parse_s3_url_with_nested_key() {
        let path = RemotePath::parse("s3://bucket/path/to/data.csv").unwrap();
        assert_eq!(path.scheme(), RemoteScheme::S3);
        assert_eq!(path.bucket(), Some("bucket"));
        assert_eq!(path.key(), "path/to/data.csv");
    }

    #[test]
    fn test_parse_https_url() {
        let path = RemotePath::parse("https://example.com/data.arrow").unwrap();
        assert_eq!(path.scheme(), RemoteScheme::Https);
        assert_eq!(path.host(), Some("example.com"));
        assert_eq!(path.key(), "/data.arrow");
    }

    #[test]
    fn test_parse_http_url() {
        let path = RemotePath::parse("http://localhost:8080/data.jsonl").unwrap();
        assert_eq!(path.scheme(), RemoteScheme::Http);
        assert_eq!(path.host(), Some("localhost:8080"));
        assert_eq!(path.key(), "/data.jsonl");
    }

    #[test]
    fn test_parse_invalid_url_no_scheme() {
        let result = RemotePath::parse("invalid-url");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RemoteFsError::UnsupportedScheme(_)
        ));
    }

    #[test]
    fn test_parse_s3_url_missing_key() {
        let result = RemotePath::parse("s3://bucket");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RemoteFsError::InvalidUrl(_)));
    }

    // -----------------------------------------------------------------
    //  2. SigV4 签名测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sigv4_sign_deterministic() {
        let sigv4 = SigV4::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "s3",
        );
        let signed1 = sigv4.sign(
            "GET",
            "example.amazonaws.com",
            "/bucket/key",
            "",
            "20250101T000000Z",
            EMPTY_PAYLOAD_HASH,
            &[],
        );
        let signed2 = sigv4.sign(
            "GET",
            "example.amazonaws.com",
            "/bucket/key",
            "",
            "20250101T000000Z",
            EMPTY_PAYLOAD_HASH,
            &[],
        );
        assert_eq!(signed1.authorization, signed2.authorization);
    }

    #[test]
    fn test_sigv4_sign_different_inputs() {
        let sigv4 = SigV4::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "s3",
        );
        let signed1 = sigv4.sign(
            "GET",
            "example.amazonaws.com",
            "/bucket/key1",
            "",
            "20250101T000000Z",
            EMPTY_PAYLOAD_HASH,
            &[],
        );
        let signed2 = sigv4.sign(
            "GET",
            "example.amazonaws.com",
            "/bucket/key2",
            "",
            "20250101T000000Z",
            EMPTY_PAYLOAD_HASH,
            &[],
        );
        assert_ne!(signed1.authorization, signed2.authorization);
    }

    #[test]
    fn test_sigv4_sign_format() {
        let sigv4 = SigV4::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "s3",
        );
        let signed = sigv4.sign(
            "GET",
            "example.amazonaws.com",
            "/bucket/key",
            "",
            "20250101T000000Z",
            EMPTY_PAYLOAD_HASH,
            &[],
        );

        // 验证 Authorization header 格式
        assert!(signed.authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20250101/us-east-1/s3/aws4_request,"
        ));
        assert!(signed.authorization.contains("SignedHeaders="));
        assert!(signed.authorization.contains("Signature="));

        // 提取 signature 并验证长度（64 位 hex）
        let sig = signed.authorization.rsplit("Signature=").next().unwrap();
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_sigv4_sign_with_extra_headers() {
        let sigv4 = SigV4::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "s3",
        );
        let signed_no_range = sigv4.sign(
            "GET",
            "example.amazonaws.com",
            "/bucket/key",
            "",
            "20250101T000000Z",
            EMPTY_PAYLOAD_HASH,
            &[],
        );
        let signed_with_range = sigv4.sign(
            "GET",
            "example.amazonaws.com",
            "/bucket/key",
            "",
            "20250101T000000Z",
            EMPTY_PAYLOAD_HASH,
            &[("range".to_string(), "bytes=0-1023".to_string())],
        );

        // 带 Range header 的签名应该不同
        assert_ne!(
            signed_no_range.authorization,
            signed_with_range.authorization
        );
        // 带 Range header 的签名应该包含 range 在 SignedHeaders 中
        assert!(signed_with_range
            .authorization
            .contains("SignedHeaders=host;range;x-amz-content-sha256;x-amz-date"));
    }

    #[test]
    fn test_sigv4_signing_key_derivation() {
        let sigv4 = SigV4::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "s3",
        );
        let key1 = sigv4.signing_key("20250101");
        let key2 = sigv4.signing_key("20250101");
        let key3 = sigv4.signing_key("20250102");

        // 相同日期派生相同密钥
        assert_eq!(key1, key2);
        // 不同日期派生不同密钥
        assert_ne!(key1, key3);
        // 密钥长度为 32 字节（SHA256 输出）
        assert_eq!(key1.len(), 32);
    }

    // -----------------------------------------------------------------
    //  3. MockBackend 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_mock_backend_content_length() {
        let backend = MockBackend::new(b"hello world".to_vec());
        assert_eq!(backend.content_length().unwrap(), 11);
    }

    #[test]
    fn test_mock_backend_fetch_range() {
        let backend = MockBackend::new(b"hello world".to_vec());
        let bytes = backend.fetch_range(0, 5).unwrap();
        assert_eq!(bytes, Bytes::from(b"hello".to_vec()));

        let bytes = backend.fetch_range(6, 5).unwrap();
        assert_eq!(bytes, Bytes::from(b"world".to_vec()));
    }

    #[test]
    fn test_mock_backend_fetch_range_out_of_bounds() {
        let backend = MockBackend::new(b"hello".to_vec());
        let result = backend.fetch_range(10, 5);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RemoteFsError::RangeNotSatisfiable { .. }
        ));
    }

    // -----------------------------------------------------------------
    //  4. RemoteFileReader Read + Seek 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_remote_file_reader_sequential_read() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let mut reader = RemoteFileReader::new(backend).unwrap();

        let mut buf = [0u8; 5];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, &[1, 2, 3, 4, 5]);

        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, &[6, 7, 8, 9, 10]);

        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn test_remote_file_reader_small_buffer() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let mut reader = RemoteFileReader::new(backend).unwrap();

        let mut buf = [0u8; 3];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf, &[1, 2, 3]);

        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf, &[4, 5, 6]);
    }

    #[test]
    fn test_remote_file_reader_seek_to_start() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let mut reader = RemoteFileReader::new(backend).unwrap();

        let mut buf = [0u8; 5];
        let _ = reader.read(&mut buf).unwrap();

        reader.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(reader.position(), 0);

        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_remote_file_reader_seek_to_position() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let mut reader = RemoteFileReader::new(backend).unwrap();

        reader.seek(SeekFrom::Start(5)).unwrap();
        assert_eq!(reader.position(), 5);

        let mut buf = [0u8; 5];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, &[6, 7, 8, 9, 10]);
    }

    #[test]
    fn test_remote_file_reader_seek_to_end() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let mut reader = RemoteFileReader::new(backend).unwrap();

        reader.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(reader.position(), 10);

        let mut buf = [0u8; 5];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn test_remote_file_reader_seek_beyond_end() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let mut reader = RemoteFileReader::new(backend).unwrap();

        let result = reader.seek(SeekFrom::Start(11));
        assert!(result.is_err());
    }

    #[test]
    fn test_remote_file_reader_seek_current() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let mut reader = RemoteFileReader::new(backend).unwrap();

        let mut buf = [0u8; 3];
        let _ = reader.read(&mut buf).unwrap();
        assert_eq!(reader.position(), 3);

        reader.seek(SeekFrom::Current(2)).unwrap();
        assert_eq!(reader.position(), 5);

        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf, &[6, 7, 8]);
    }

    // -----------------------------------------------------------------
    //  5. RemoteChunkReader 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_remote_chunk_reader_length() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let reader = RemoteChunkReader::new(backend).unwrap();

        use parquet::file::reader::Length;
        assert_eq!(reader.len(), 10);
    }

    #[test]
    fn test_remote_chunk_reader_get_bytes() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let reader = RemoteChunkReader::new(backend).unwrap();

        use parquet::file::reader::ChunkReader;
        let bytes = reader.get_bytes(2, 5).unwrap();
        assert_eq!(bytes, Bytes::from(vec![3, 4, 5, 6, 7]));
    }

    #[test]
    fn test_remote_chunk_reader_get_read() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let reader = RemoteChunkReader::new(backend).unwrap();

        use parquet::file::reader::ChunkReader;
        let mut read = reader.get_read(5).unwrap();
        let mut buf = Vec::new();
        read.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, vec![6, 7, 8, 9, 10]);
    }

    #[test]
    fn test_remote_chunk_reader_get_bytes_beyond_end() {
        let data: Vec<u8> = (1..=10).collect();
        let backend = Arc::new(MockBackend::new(data));
        let reader = RemoteChunkReader::new(backend).unwrap();

        use parquet::file::reader::ChunkReader;
        let result = reader.get_bytes(15, 5);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    //  6. 集成测试 — MockBackend + ExternalReader
    // -----------------------------------------------------------------

    #[test]
    fn test_read_remote_parquet() {
        let path = create_temp_path(".parquet");
        let schema = test_schema();
        let rows = test_rows();

        write_parquet_file(&path, &schema, &rows).unwrap();
        let data = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);

        let backend = Arc::new(MockBackend::new(data));
        let chunk_reader = RemoteChunkReader::new(backend).unwrap();
        let reader = ParquetReader::from_chunk_reader(chunk_reader).unwrap();

        assert_eq!(reader.schema().column_names(), schema.column_names());

        let rows = reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].get(0), Some(&ExternalValue::Int64(1)));
        assert_eq!(rows[2].get(1), Some(&ExternalValue::Text("charlie".into())));
    }

    #[test]
    fn test_read_remote_parquet_with_predicate() {
        let path = create_temp_path(".parquet");
        let schema = test_schema();
        let rows = test_rows();

        write_parquet_file(&path, &schema, &rows).unwrap();
        let data = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);

        let backend = Arc::new(MockBackend::new(data));
        let chunk_reader = RemoteChunkReader::new(backend).unwrap();

        // 谓词: id > 2
        let predicate = Predicate::gt("id", ExternalValue::Int64(2));
        let reader =
            ParquetReader::from_chunk_reader_with_predicate(chunk_reader, &predicate).unwrap();

        let rows = reader
            .read(&ReadOptions::all().with_predicate(predicate))
            .unwrap();
        // id > 2 的行: 3, 4, 5
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].get(0), Some(&ExternalValue::Int64(3)));
        assert_eq!(rows[2].get(0), Some(&ExternalValue::Int64(5)));
    }

    #[test]
    fn test_read_remote_parquet_column_pruning() {
        let path = create_temp_path(".parquet");
        let schema = test_schema();
        let rows = test_rows();

        write_parquet_file(&path, &schema, &rows).unwrap();
        let data = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);

        let backend = Arc::new(MockBackend::new(data));
        let chunk_reader = RemoteChunkReader::new(backend).unwrap();
        let reader = ParquetReader::from_chunk_reader(chunk_reader).unwrap();

        let options = ReadOptions::all().with_columns(vec!["id".to_string(), "name".to_string()]);
        let rows = reader.read(&options).unwrap();

        assert_eq!(rows.len(), 5);
        // 只读取了 id 和 name 两列
        assert_eq!(rows[0].values.len(), 2);
        assert_eq!(rows[0].get(0), Some(&ExternalValue::Int64(1)));
        assert_eq!(rows[0].get(1), Some(&ExternalValue::Text("alice".into())));
    }

    #[test]
    fn test_read_remote_arrow() {
        let path = create_temp_path(".arrow");
        let schema = test_schema();
        let rows = test_rows();

        write_arrow_file(&path, &schema, &rows).unwrap();
        let data = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);

        let backend = Arc::new(MockBackend::new(data));
        let reader = RemoteFileReader::new(backend).unwrap();
        let arrow_reader = ArrowReader::from_reader(reader).unwrap();

        let rows = arrow_reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].get(0), Some(&ExternalValue::Int64(1)));
    }

    #[test]
    fn test_read_remote_csv() {
        let path = create_temp_path(".csv");
        let schema = test_schema();
        let rows = test_rows();

        write_csv_file(&path, &schema, &rows).unwrap();
        let data = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);

        let backend = Arc::new(MockBackend::new(data));
        let reader = RemoteFileReader::new(backend).unwrap();
        let csv_reader = CsvReader::from_reader(reader).unwrap();

        let rows = csv_reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].get(0), Some(&ExternalValue::Int64(1)));
    }

    #[test]
    fn test_read_remote_jsonlines() {
        let path = create_temp_path(".jsonl");
        let schema = test_schema();
        let rows = test_rows();

        write_json_lines_file(&path, &schema, &rows).unwrap();
        let data = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);

        let backend = Arc::new(MockBackend::new(data));
        let reader = RemoteFileReader::new(backend).unwrap();
        let json_reader = JsonLinesReader::from_reader(reader).unwrap();

        let rows = json_reader.read(&ReadOptions::all()).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].get(0), Some(&ExternalValue::Int64(1)));
    }

    // -----------------------------------------------------------------
    //  7. read_remote_file 统一入口测试
    // -----------------------------------------------------------------

    #[test]
    fn test_read_remote_file_invalid_url() {
        let result = read_remote_file("invalid-url", &ReadOptions::all(), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_remote_file_s3_without_config() {
        let result = read_remote_file("s3://bucket/data.parquet", &ReadOptions::all(), None);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RemoteFsError::AuthenticationRequired
        ));
    }

    #[test]
    fn test_read_remote_file_s3_empty_credentials() {
        let config = S3Config {
            access_key_id: String::new(),
            secret_access_key: String::new(),
            region: "us-east-1".into(),
            endpoint: None,
        };
        let result = read_remote_file(
            "s3://bucket/data.parquet",
            &ReadOptions::all(),
            Some(&config),
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RemoteFsError::AuthenticationRequired
        ));
    }

    // -----------------------------------------------------------------
    //  8. HttpBackend / S3Backend 构造测试
    // -----------------------------------------------------------------

    #[test]
    fn test_http_backend_from_url() {
        let backend = HttpBackend::from_url("https://example.com/data.csv").unwrap();
        assert_eq!(backend.url, "https://example.com/data.csv");
    }

    #[test]
    fn test_http_backend_from_url_http() {
        let backend = HttpBackend::from_url("http://localhost:8080/data.jsonl").unwrap();
        assert_eq!(backend.url, "http://localhost:8080/data.jsonl");
    }

    #[test]
    fn test_s3_backend_aws_endpoint() {
        let path = RemotePath::parse("s3://mybucket/data.parquet").unwrap();
        let config = S3Config {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "secret".into(),
            region: "us-west-2".into(),
            endpoint: None,
        };
        let backend = S3Backend::new(&path, config).unwrap();
        assert_eq!(backend.host, "s3.us-west-2.amazonaws.com");
        assert_eq!(
            backend.url,
            "https://s3.us-west-2.amazonaws.com/mybucket/data.parquet"
        );
        assert_eq!(backend.canonical_uri, "/mybucket/data.parquet");
    }

    #[test]
    fn test_s3_backend_custom_endpoint() {
        let path = RemotePath::parse("s3://mybucket/data.parquet").unwrap();
        let config = S3Config {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "secret".into(),
            region: "us-east-1".into(),
            endpoint: Some("http://localhost:9000".into()),
        };
        let backend = S3Backend::new(&path, config).unwrap();
        assert_eq!(backend.host, "localhost:9000");
        assert_eq!(backend.url, "http://localhost:9000/mybucket/data.parquet");
        assert_eq!(backend.canonical_uri, "/mybucket/data.parquet");
    }

    // -----------------------------------------------------------------
    //  9. S3Config 默认值测试
    // -----------------------------------------------------------------

    #[test]
    fn test_s3_config_default_region() {
        // 不设置环境变量时，默认 region 为 us-east-1
        // （注意：如果环境已设置 AWS_DEFAULT_REGION，此测试可能不同）
        let config = S3Config::default();
        assert!(
            !config.region.is_empty(),
            "region should not be empty (default us-east-1)"
        );
    }

    // -----------------------------------------------------------------
    //  10. 辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_encode(&[0x00]), "00");
        assert_eq!(hex_encode(&[0xff]), "ff");
        assert_eq!(hex_encode(&[0xab, 0xcd, 0xef]), "abcdef");
    }

    #[test]
    fn test_sha256_hex_empty() {
        let hash = sha256_hex(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_sha256_hex_known() {
        let hash = sha256_hex(b"abc");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
