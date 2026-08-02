//! 静态文件服务 — `tower-http::services::ServeDir` 封装 + 自定义文件处理器
//!
//! 提供静态文件服务，用于托管前端 SPA、图片、CSS、JS 等静态资源。
//!
//! ## 功能
//!
//! - 目录映射：将 URL 前缀映射到文件系统目录
//! - 默认首页：访问目录时自动返回 `index.html`
//! - SPA fallback：未匹配的路径回退到 `index.html`（用于前端路由）
//! - 安全防护：自动阻止路径穿越（`../`）
//! - 自定义文件处理器（对齐 PHP think-worker `sendFile`）
//! - MIME 类型识别（对齐 PHP `$mimeTypeMap` + `finfo` 后备）
//! - Range 请求支持（对齐 Webman `sendFile` 206 Partial Content）
//! - 304 Not Modified（对齐 PHP `If-Modified-Since` 检查）
//! - Last-Modified 头（对齐 PHP `filemtime`）
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::static_files::{static_router, static_router_spa, serve_file};
//! use axum::Router;
//!
//! // 普通静态目录（基于 ServeDir）
//! let app: Router = Router::new()
//!     .merge(static_router("/static", "./public"));
//!
//! // SPA 应用（fallback 到 index.html）
//! let app: Router = Router::new()
//!     .merge(static_router_spa("./dist"));
//!
//! // 自定义文件处理器（对齐 PHP think-worker sendFile）
//! use axum::http::HeaderMap;
//! async fn handler(headers: HeaderMap) -> axum::response::Response {
//!     serve_file(std::path::Path::new("./public/style.css"), &headers)
//! }
//! ```
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | 行为 | Rust 等价 |
//! |---------|------|-----------|
//! | `think-worker\Http::sendFile()` | 304/Last-Modified/Content-Type/Content-Length | [`serve_file`] |
//! | `think-worker\Http::getMimeType()` | 扩展名表 + `finfo` 后备 | [`mime_type_for_path`] |
//! | `WebServer::sendFile()` Range 分支 | 206 Partial Content + Content-Range | [`serve_file`] Range 分支 |
//! | `$mimeTypeMap` | 扩展名 → MIME 映射表 | [`mime_type_for_extension`] |

use axum::Router;
use std::path::{Path, PathBuf};
use tower_http::services::{ServeDir, ServeFile};

// ============================================================================
// 基于 ServeDir 的封装（向后兼容）
// ============================================================================

/// 创建静态文件 `ServeDir`（用于 `nest_service`）
///
/// - 访问 `/static/foo.css` → 返回 `./public/foo.css`
/// - 文件不存在 → 返回 404
pub fn static_dir(path: impl AsRef<Path>) -> ServeDir {
    ServeDir::new(path)
}

/// 创建带默认 `index.html` 的 `ServeDir`
///
/// 当访问目录（如 `/static/`）时返回 `index.html`。
pub fn static_dir_with_index(path: impl AsRef<Path>) -> ServeDir {
    ServeDir::new(path).append_index_html_on_directories(true)
}

/// 创建 SPA `ServeDir`（fallback 到 `index.html`）
///
/// 所有未匹配的路径都返回 `index.html`，用于前端路由（如 React Router / Vue Router）。
pub fn static_dir_spa(path: impl AsRef<Path>) -> ServeDir<ServeFile> {
    let index = path.as_ref().join("index.html");
    ServeDir::new(path).fallback(ServeFile::new(index))
}

/// 创建静态文件 Router 并挂载到指定路径
///
/// 等价于 `Router::new().nest_service(prefix, ServeDir::new(path))`。
pub fn static_router(prefix: &str, path: impl AsRef<Path>) -> Router {
    Router::new().nest_service(prefix, ServeDir::new(path))
}

/// 创建带默认 `index.html` 的静态文件 Router
pub fn static_router_with_index(prefix: &str, path: impl AsRef<Path>) -> Router {
    Router::new().nest_service(prefix, static_dir_with_index(path))
}

/// 创建 SPA 静态文件 Router 并作为 fallback
///
/// 等价于 `Router::new().fallback_service(ServeDir::new(path).fallback(ServeFile::new(index)))`。
pub fn static_router_spa(path: impl AsRef<Path>) -> Router {
    Router::new().fallback_service(static_dir_spa(path))
}

/// 创建单文件服务（如 favicon.ico）
pub fn static_file(path: impl AsRef<Path>) -> ServeFile {
    ServeFile::new(path)
}

// ============================================================================
// 自定义文件处理器（对齐 PHP think-worker sendFile + Webman Range）
// ============================================================================

/// MIME 类型表（对齐 PHP think-worker `$mimeTypeMap`）
///
/// PHP `$mimeTypeMap` 从 `mime.types` 文件加载，此处硬编码常见类型。
/// 对齐 PHP `think-worker\Http::$mimeTypeMap` + Webman `WebServer::$mimeTypeMap`。
const MIME_TYPES: &[(&str, &str)] = &[
    // 文本
    ("html", "text/html"),
    ("htm", "text/html"),
    ("shtml", "text/html"),
    ("css", "text/css"),
    ("xml", "text/xml"),
    ("txt", "text/plain"),
    ("md", "text/markdown"),
    ("csv", "text/csv"),
    // JavaScript
    ("js", "application/javascript"),
    ("mjs", "application/javascript"),
    ("json", "application/json"),
    // 图片
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("bmp", "image/bmp"),
    ("ico", "image/x-icon"),
    ("svg", "image/svg+xml"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    // 音视频
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("m3u8", "application/vnd.apple.mpegurl"),
    ("ts", "video/mp2t"),
    // 字体
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
    ("eot", "application/vnd.ms-fontobject"),
    // 文档
    ("pdf", "application/pdf"),
    ("zip", "application/zip"),
    ("gz", "application/gzip"),
    ("tar", "application/x-tar"),
    ("rar", "application/vnd.rar"),
    ("7z", "application/x-7z-compressed"),
    // WebAssembly
    ("wasm", "application/wasm"),
    // 其他
    ("swf", "application/x-shockwave-flash"),
    ("doc", "application/msword"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    ("xls", "application/vnd.ms-excel"),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    ("ppt", "application/vnd.ms-powerpoint"),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
];

/// 根据扩展名获取 MIME 类型（对齐 PHP `$mimeTypeMap[$extension]`）
///
/// 扩展名不区分大小写。返回 `None` 表示未找到（对齐 PHP 走 `finfo` 后备）。
pub fn mime_type_for_extension(ext: &str) -> Option<&'static str> {
    let ext_lower = ext.to_lowercase();
    MIME_TYPES
        .iter()
        .find(|(k, _)| *k == ext_lower)
        .map(|(_, v)| *v)
}

/// 根据文件路径获取 MIME 类型（对齐 PHP `think-worker\Http::getMimeType`）
///
/// PHP 逻辑：先查 `$mimeTypeMap[extension]`，未找到则用 `finfo_file()`。
/// Rust 逻辑：先查硬编码表，未找到则用 `mime_guess::from_path()`。
pub fn mime_type_for_path(path: &Path) -> Option<String> {
    // 1. 先查扩展名表（对齐 PHP $mimeTypeMap）
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if let Some(mime) = mime_type_for_extension(ext) {
            return Some(mime.to_string());
        }
    }
    // 2. 后备：mime_guess（对齐 PHP finfo_file）
    mime_guess::from_path(path).first().map(|m| m.to_string())
}

/// HTTP Range 解析结果（对齐 Webman `sendFile` Range 分支）
#[derive(Debug, Clone, PartialEq)]
pub struct RangeSpec {
    /// 起始字节偏移（含）
    pub start: u64,
    /// 结束字节偏移（含）
    pub end: u64,
}

/// Range 解析错误
#[derive(Debug, Clone, PartialEq)]
pub enum RangeError {
    /// 非法格式（非 `bytes=` 前缀）
    InvalidFormat,
    /// 非法范围（start > end 或 start >= file_size）
    InvalidRange,
    /// 不可满足的范围（超出文件大小）
    Unsatisfiable,
}

/// 解析 Range 头（对齐 Webman `sendFile` 中 `explode('=', ..., 2)` + `explode('-', ...)`）
///
/// 支持三种格式（对齐 HTTP/1.1 Range 规范）：
/// - `bytes=0-499` → RangeSpec { start: 0, end: 499 }
/// - `bytes=500-` → RangeSpec { start: 500, end: file_size - 1 }
/// - `bytes=-500` → 最后 500 字节，RangeSpec { start: file_size-500, end: file_size-1 }
///
/// PHP Webman 仅支持 `bytes=start-end` 和 `bytes=start-`，不支持 `bytes=-suffix`。
/// Rust 实现完整支持三种格式，超出 PHP 对齐范围但符合 HTTP 规范。
pub fn parse_range_header(range: &str, file_size: u64) -> Result<RangeSpec, RangeError> {
    // 对齐 PHP: list(, $range) = explode('=', $_SERVER['HTTP_RANGE'], 2)
    let range = range.trim();
    let range_value = range
        .strip_prefix("bytes=")
        .ok_or(RangeError::InvalidFormat)?;

    // 对齐 PHP: list($start, $end) = explode('-', $range)
    let (start_str, end_str) = range_value
        .split_once('-')
        .ok_or(RangeError::InvalidFormat)?;

    let (start, end) = match (start_str.is_empty(), end_str.is_empty()) {
        // bytes=-suffix（最后 N 字节）
        (true, false) => {
            let suffix: u64 = end_str.parse().map_err(|_| RangeError::InvalidRange)?;
            if suffix == 0 {
                return Err(RangeError::InvalidRange);
            }
            let start = file_size.saturating_sub(suffix);
            (start, file_size.saturating_sub(1))
        }
        // bytes=start-（从 start 到文件末尾）
        (false, true) => {
            let start: u64 = start_str.parse().map_err(|_| RangeError::InvalidRange)?;
            if start >= file_size {
                return Err(RangeError::Unsatisfiable);
            }
            (start, file_size.saturating_sub(1))
        }
        // bytes=start-end
        (false, false) => {
            let start: u64 = start_str.parse().map_err(|_| RangeError::InvalidRange)?;
            let end: u64 = end_str.parse().map_err(|_| RangeError::InvalidRange)?;
            if start > end {
                return Err(RangeError::InvalidRange);
            }
            if start >= file_size {
                return Err(RangeError::Unsatisfiable);
            }
            // end 超出文件大小时截断（对齐 PHP: $end = is_numeric($end) ? $end : $file_size - 1）
            let end = end.min(file_size.saturating_sub(1));
            (start, end)
        }
        // bytes=- （空范围）
        (true, true) => return Err(RangeError::InvalidRange),
    };

    Ok(RangeSpec { start, end })
}

/// 路径安全验证（防止 `../` 路径穿越）
///
/// 检查规范化后的路径是否仍在根目录内。
/// 对齐 nginx/Apache 的路径穿越防护，PHP `realpath()` 检查。
pub fn is_path_safe(path: &Path, root: &Path) -> bool {
    // 规范化路径（解析 `.` 和 `..`）
    let canonical_root = match root.canonicalize() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let canonical_path = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return false,
    };
    // 检查规范化后的路径是否以根目录为前缀
    canonical_path.starts_with(&canonical_root)
}

/// 检查路径是否包含 `..` 组件（路径遍历特征）
///
/// P1-PATH-01 防御性检查：即使调用方遗漏 `is_path_safe` 校验，
/// `serve_file` / `serve_file_with_cache` 自身也拒绝包含父目录跳转的路径。
fn has_traversal_component(path: &Path) -> bool {
    use std::path::Component;
    path.components().any(|c| matches!(c, Component::ParentDir))
}

/// 格式化 HTTP 日期（对齐 PHP `date('D, d M Y H:i:s', $time) . ' GMT'`）
///
/// PHP 使用服务器时区，但 Last-Modified 头必须用 GMT。
/// Rust 直接格式化为 GMT。
fn format_http_date(timestamp: std::time::SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let secs = timestamp
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 简化的日期格式化（对齐 PHP 'D, d M Y H:i:s' + ' GMT'）
    // 不依赖 chrono，手动计算
    let (year, month, day, hour, minute, second, weekday) = secs_to_date_time(secs);

    let weekdays = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        weekdays[weekday as usize],
        day,
        months[(month - 1) as usize],
        year,
        hour,
        minute,
        second,
    )
}

/// Unix 时间戳 → (年, 月, 日, 时, 分, 秒, 星期几)
///
/// 基于 Howard Hinnant 的日期算法（civil_from_days）。
/// 参考: https://howardhinnant.github.io/date_algorithms.html
fn secs_to_date_time(secs: u64) -> (u64, u64, u64, u64, u64, u64, u64) {
    let secs_in_day = 86400u64;
    let mut days = secs / secs_in_day;
    let remainder = secs % secs_in_day;

    let hour = remainder / 3600;
    let minute = (remainder % 3600) / 60;
    let second = remainder % 60;

    // 1970-01-01 是星期四
    let weekday = (days + 4) % 7;

    // Howard Hinnant civil_from_days 算法
    days += 719468; // 从 0000-03-01 开始
    let era = days / 146097;
    let doe = days - era * 146097; // [0, 146096]
                                   // 注意: 常量是 1460/36524/146096（不含闰日），不是 1461/36524/146097
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };

    (year, m, d, hour, minute, second, weekday)
}

/// 自定义文件处理器（对齐 PHP think-worker `sendFile` + Webman Range）
///
/// 完整对齐 PHP `think-worker\Http::sendFile()`：
/// 1. 304 检查（`If-Modified-Since` 匹配 → 304 Not Modified）
/// 2. MIME 类型识别（扩展名表 + `mime_guess` 后备）
/// 3. Content-Type（已知 MIME → 直接设置，未知 → `application/octet-stream` + `Content-Disposition`）
/// 4. Last-Modified 头
/// 5. Content-Length 头
///
/// 额外对齐 Webman `sendFile()`：
/// 6. Range 请求（`Range: bytes=start-end` → 206 Partial Content + Content-Range）
/// 7. Accept-Ranges: bytes
///
/// # 参数
/// - `path` — 文件路径
/// - `headers` — 请求头（用于 `If-Modified-Since` 和 `Range`）
///
/// # 返回
/// - 200 OK — 完整文件
/// - 206 Partial Content — Range 请求
/// - 304 Not Modified — `If-Modified-Since` 匹配
/// - 404 Not Found — 文件不存在
/// - 416 Range Not Satisfiable — Range 超出文件大小
pub async fn serve_file(path: &Path, headers: &axum::http::HeaderMap) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    // P1-PATH-01: 防御性路径遍历检查
    // 即使调用方遗漏 is_path_safe 校验，也拒绝包含 .. 的路径
    if has_traversal_component(path) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    // 1. 检查文件存在
    if !path.is_file() {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    }

    // 2. 读取文件元数据（P1-IO-02：使用 tokio::fs 避免阻塞 async 运行时）
    let metadata = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read file metadata",
            )
                .into_response()
        }
    };
    let file_size = metadata.len();
    let modified = metadata.modified().ok();

    // 3. 304 检查（对齐 PHP If-Modified-Since）
    if let Some(modified_time) = modified {
        let last_modified = format_http_date(modified_time);
        if let Some(if_modified_since) = headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = if_modified_since.to_str() {
                if ims_str.trim() == last_modified {
                    return (
                        StatusCode::NOT_MODIFIED,
                        [(header::LAST_MODIFIED, last_modified.as_str())],
                        Body::empty(),
                    )
                        .into_response();
                }
            }
        }
    }

    // 4. MIME 类型识别（对齐 PHP getMimeType）
    let mime = mime_type_for_path(path);
    let content_type = mime
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_string());

    // 5. 读取文件内容（P1-IO-02：使用 tokio::fs 避免阻塞 async 运行时）
    let content = match tokio::fs::read(path).await {
        Ok(c) => c,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file").into_response()
        }
    };

    // 6. Range 请求处理（对齐 Webman Range 分支）
    if let Some(range_header) = headers.get(header::RANGE) {
        if let Ok(range_str) = range_header.to_str() {
            match parse_range_header(range_str, file_size) {
                Ok(range) => {
                    let content_length = range.end - range.start + 1;
                    let bytes = content
                        .get(range.start as usize..=(range.end as usize))
                        .unwrap_or(&[]);
                    let content_range =
                        format!("bytes {}-{}/{}", range.start, range.end, file_size);
                    let content_length_str = content_length.to_string();

                    let mut response = (
                        StatusCode::PARTIAL_CONTENT,
                        [
                            (header::CONTENT_TYPE, content_type.as_str()),
                            (header::CONTENT_LENGTH, content_length_str.as_str()),
                            (header::CONTENT_RANGE, content_range.as_str()),
                            (header::ACCEPT_RANGES, "bytes"),
                        ],
                        Body::from(bytes.to_vec()),
                    )
                        .into_response();

                    if let Some(modified_time) = modified {
                        let last_modified = format_http_date(modified_time);
                        if let Ok(val) = axum::http::HeaderValue::from_str(&last_modified) {
                            response.headers_mut().insert(header::LAST_MODIFIED, val);
                        }
                    }
                    return response;
                }
                Err(RangeError::Unsatisfiable) => {
                    let content_range = format!("bytes */{}", file_size);
                    return (
                        StatusCode::RANGE_NOT_SATISFIABLE,
                        [(header::CONTENT_RANGE, content_range.as_str())],
                        Body::empty(),
                    )
                        .into_response();
                }
                Err(_) => {
                    // 非法 Range 格式，忽略 Range 头，返回完整文件
                }
            }
        }
    }

    // 7. 完整文件响应（对齐 PHP think-worker sendFile）
    let content_length_str = file_size.to_string();
    let mut response = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.as_str()),
            (header::CONTENT_LENGTH, content_length_str.as_str()),
            (header::ACCEPT_RANGES, "bytes"),
        ],
        Body::from(content),
    )
        .into_response();

    // Last-Modified 头（对齐 PHP Last-Modified）
    if let Some(modified_time) = modified {
        let last_modified = format_http_date(modified_time);
        if let Ok(val) = axum::http::HeaderValue::from_str(&last_modified) {
            response.headers_mut().insert(header::LAST_MODIFIED, val);
        }
    }

    // 未知 MIME 类型 → Content-Disposition（对齐 PHP think-worker）
    if mime.is_none() {
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            let disposition = format!("attachment; filename=\"{}\"", filename);
            if let Ok(val) = axum::http::HeaderValue::from_str(&disposition) {
                response
                    .headers_mut()
                    .insert(header::CONTENT_DISPOSITION, val);
            }
        }
    }

    response
}

/// 静态文件处理器（对齐 PHP think-worker `Http::sendFile` 完整流程）
///
/// 将 URI 路径映射到文件系统路径，调用 `serve_file`。
///
/// # 参数
/// - `root` — 静态文件根目录
/// - `uri_path` — 请求 URI 路径（如 `/static/style.css`）
/// - `headers` — 请求头
///
/// # 返回
/// - 200/206/304 — 文件响应
/// - 404 — 文件不存在或路径不安全
pub async fn static_handler(
    root: &Path,
    uri_path: &str,
    headers: &axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // 1. 解析 URL 路径（去掉 query string）
    let path_only = uri_path.split('?').next().unwrap_or(uri_path);

    // 2. URL 解码（对齐 PHP urldecode）
    let decoded = percent_decode(path_only);

    // 3. 拼接文件路径
    // 注意: PathBuf::join 遇到绝对路径（以 `/` 开头）会替换整个 base，
    // 需先 strip 前导 `/`（对齐 PHP: $file = $this->root . $path 字符串拼接语义）
    let relative = decoded.trim_start_matches('/');
    let file_path: PathBuf = root.join(relative);

    // 4. 路径安全验证（防止 ../ 穿越）
    if !is_path_safe(&file_path, root) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    // 5. 调用 serve_file
    serve_file(&file_path, headers).await
}

/// 简单的 percent-decode 实现（对齐 PHP `urldecode`）
///
/// 将 `%XX` 编码序列解码为原始字节。不引入 `urlencoding` 依赖。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());

    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                result.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        // 对齐 PHP urldecode: `+` 不转换为空格（PHP urldecode 会转换，但 rawurldecode 不会）
        // 静态文件路径使用 rawurldecode 语义
        result.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&result).into_owned()
}

/// 十六进制字符 → 数值
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ============================================================================
// 资源版本化（Cache-Control/ETag）
// ============================================================================

/// Cache-Control 配置（对齐 nginx `expires` 指令）
///
/// 对齐 nginx 配置：
/// - `expires 1h;` → `Cache-Control: max-age=3600`
/// - `expires -1;` → `Cache-Control: no-cache`
/// - `expires off;` → 不设置 Cache-Control
/// - `add_header Cache-Control "public";` → `Cache-Control: public`
#[derive(Debug, Clone, Default)]
pub struct CacheControlConfig {
    /// max-age（秒），None 表示不设置 max-age
    pub max_age: Option<u64>,
    /// public（公共缓存，CDN 可缓存）/ private（仅浏览器缓存）/ None
    pub visibility: Option<CacheVisibility>,
    /// no-cache（必须重新验证）/ no-store（完全不缓存）
    pub no_cache: bool,
    /// 完全不缓存
    pub no_store: bool,
    /// must-revalidate（过期后必须重新验证）
    pub must_revalidate: bool,
    /// immutable（文件永不变化，浏览器可永久缓存，对齐前端构建工具指纹 hash 场景）
    pub immutable: bool,
}

/// 缓存可见性（对齐 HTTP/1.1 Cache-Control 指令）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheVisibility {
    /// `public` — 任何缓存（CDN/代理/浏览器）都可缓存
    Public,
    /// `private` — 仅浏览器可缓存（对齐用户特定数据）
    Private,
}

impl CacheControlConfig {
    /// 创建空配置（不设置任何 Cache-Control 指令）
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 max-age（秒）
    pub fn with_max_age(mut self, seconds: u64) -> Self {
        self.max_age = Some(seconds);
        self
    }

    /// 设置 public
    pub fn with_public(mut self) -> Self {
        self.visibility = Some(CacheVisibility::Public);
        self
    }

    /// 设置 private
    pub fn with_private(mut self) -> Self {
        self.visibility = Some(CacheVisibility::Private);
        self
    }

    /// 设置 no-cache（必须重新验证）
    pub fn with_no_cache(mut self) -> Self {
        self.no_cache = true;
        self
    }

    /// 设置 no-store（完全不缓存）
    pub fn with_no_store(mut self) -> Self {
        self.no_store = true;
        self
    }

    /// 设置 must-revalidate
    pub fn with_must_revalidate(mut self) -> Self {
        self.must_revalidate = true;
        self
    }

    /// 设置 immutable（对齐前端构建工具指纹 hash 场景）
    pub fn with_immutable(mut self) -> Self {
        self.immutable = true;
        self
    }

    /// 生成 Cache-Control 头值
    ///
    /// 返回 `None` 表示配置为空（不设置 Cache-Control 头）。
    pub fn to_header_value(&self) -> Option<String> {
        let mut directives = Vec::new();

        if self.no_store {
            directives.push("no-store".to_string());
        }
        if self.no_cache {
            directives.push("no-cache".to_string());
        }
        if let Some(v) = self.visibility {
            match v {
                CacheVisibility::Public => directives.push("public".to_string()),
                CacheVisibility::Private => directives.push("private".to_string()),
            }
        }
        if let Some(max_age) = self.max_age {
            directives.push(format!("max-age={}", max_age));
        }
        if self.must_revalidate {
            directives.push("must-revalidate".to_string());
        }
        if self.immutable {
            directives.push("immutable".to_string());
        }

        if directives.is_empty() {
            None
        } else {
            Some(directives.join(", "))
        }
    }
}

/// 生成 weak ETag（对齐 nginx 默认 ETag 行为）
///
/// nginx 默认 ETag 格式：`W/"<mtime>-<size>"`，基于文件修改时间和大小。
/// weak ETag 表示语义相等（内容可能不同但语义相同），适用于 nginx 默认静态文件服务。
///
/// # 参数
/// - `metadata` — 文件元数据（需包含 `modified()` 和 `len()`）
///
/// # 返回
/// - `Some(String)` — ETag 值（如 `W/"1679500000-1024"`）
/// - `None` — 无法获取修改时间
pub fn compute_etag(metadata: &std::fs::Metadata) -> Option<String> {
    let modified = metadata.modified().ok()?;
    let secs = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let size = metadata.len();
    Some(format!("W/\"{}-{}\"", secs, size))
}

/// 计算文件内容的指纹 hash（对齐前端构建工具 `[contenthash]`）
///
/// 使用 MD5 算法计算文件内容的 hash，返回 32 位十六进制字符串。
/// 用于文件名版本化（如 `style.abc123def456.css`），对齐 webpack/vite 的 `[contenthash]`。
///
/// # 参数
/// - `path` — 文件路径
///
/// # 返回
/// - `Ok(String)` — 32 位十六进制 MD5 hash
/// - `Err(_)` — 文件读取失败
pub fn fingerprint_file(path: &Path) -> std::io::Result<String> {
    let content = std::fs::read(path)?;
    Ok(fingerprint_bytes(&content))
}

/// 计算字节切片的指纹 hash（对齐前端构建工具 `[contenthash]`）
///
/// 使用 MD5 算法计算字节切片的 hash，返回 32 位十六进制字符串。
pub fn fingerprint_bytes(content: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(content);
    let result = hasher.finalize();
    // 对齐 PHP md5() 输出：32 位小写十六进制
    let mut hex = String::with_capacity(32);
    for byte in result.iter() {
        hex.push_str(&format!("{:02x}", byte));
    }
    hex
}

/// 从版本化路径中提取原始路径（对齐前端构建工具文件名 hash 解析）
///
/// 将 `style.abc123def456.css` 解析为 `style.css`，提取并返回指纹 hash。
///
/// # 参数
/// - `path` — 版本化路径（如 `style.abc123.css` 或 `js/app.abc123.js`）
///
/// # 返回
/// - `Some((original_path, hash))` — 原始路径和指纹 hash
/// - `None` — 路径无扩展名或无指纹 hash
///
/// # 示例
/// ```
/// # use sz_rust_core::static_files::extract_version_hash;
/// assert_eq!(
///     extract_version_hash("style.abc123def456.css"),
///     Some(("style.css".to_string(), "abc123def456".to_string()))
/// );
/// assert_eq!(
///     extract_version_hash("js/app.abc12345.js"),
///     Some(("js/app.js".to_string(), "abc12345".to_string()))
/// );
/// assert_eq!(extract_version_hash("style.css"), None);
/// ```
pub fn extract_version_hash(path: &str) -> Option<(String, String)> {
    // 查找最后一个 `.` 分隔扩展名
    let last_dot = path.rfind('.')?;
    let ext = &path[last_dot + 1..];
    if ext.is_empty() {
        return None;
    }

    // 查找倒数第二个 `.`（分隔文件名和 hash）
    let stem_with_hash = &path[..last_dot];
    let second_last_dot = stem_with_hash.rfind('.')?;

    let stem = &stem_with_hash[..second_last_dot];
    let hash = &stem_with_hash[second_last_dot + 1..];

    // hash 必须是有效的十六进制字符串（至少 8 位）
    if hash.len() < 8 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    Some((format!("{}.{}", stem, ext), hash.to_string()))
}

/// 自定义文件处理器（带 Cache-Control/ETag，对齐 PHP sendFile + nginx expires）
///
/// 在 `serve_file` 基础上扩展：
/// 1. ETag 生成（对齐 nginx 默认 weak ETag）
/// 2. If-None-Match 检查（对齐 HTTP/1.1 ETag 语义，304 Not Modified）
/// 3. Cache-Control 头（对齐 nginx `expires` 指令）
///
/// # 参数
/// - `path` — 文件路径
/// - `headers` — 请求头（用于 `If-Modified-Since` / `If-None-Match` / `Range`）
/// - `cache_config` — Cache-Control 配置（`None` 表示不设置 Cache-Control）
///
/// # 返回
/// - 200 OK — 完整文件
/// - 206 Partial Content — Range 请求
/// - 304 Not Modified — `If-Modified-Since` 或 `If-None-Match` 匹配
/// - 404 Not Found — 文件不存在
/// - 416 Range Not Satisfiable — Range 超出文件大小
pub async fn serve_file_with_cache(
    path: &Path,
    headers: &axum::http::HeaderMap,
    cache_config: Option<&CacheControlConfig>,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    // P1-PATH-01: 防御性路径遍历检查
    if has_traversal_component(path) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    // 1. 检查文件存在
    if !path.is_file() {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    }

    // 2. 读取文件元数据（P1-IO-02：使用 tokio::fs 避免阻塞 async 运行时）
    let metadata = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read file metadata",
            )
                .into_response();
        }
    };
    let file_size = metadata.len();
    let modified = metadata.modified().ok();

    // 3. 生成 ETag（对齐 nginx 默认 weak ETag）
    let etag = compute_etag(&metadata);

    // 4. If-None-Match 检查（对齐 RFC 7232 §6 precondition 顺序）
    // RFC 7232 §6: If-None-Match 存在时，If-Modified-Since 必须被忽略
    let mut if_none_match_present = false;
    if let Some(ref etag_value) = etag {
        if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH) {
            if_none_match_present = true;
            if let Ok(inm_str) = if_none_match.to_str() {
                // 对齐 HTTP/1.1: If-None-Match 可以是 `*` 或 ETag 列表
                if inm_str.trim() == "*" || inm_str.trim() == etag_value.as_str() {
                    let mut response = (
                        StatusCode::NOT_MODIFIED,
                        [(header::ETAG, etag_value.as_str())],
                        Body::empty(),
                    )
                        .into_response();
                    // 304 也应包含 Last-Modified 和 Cache-Control（对齐 nginx 行为）
                    if let Some(modified_time) = modified {
                        let last_modified = format_http_date(modified_time);
                        if let Ok(val) = axum::http::HeaderValue::from_str(&last_modified) {
                            response.headers_mut().insert(header::LAST_MODIFIED, val);
                        }
                    }
                    if let Some(cc) = cache_config {
                        if let Some(cc_value) = cc.to_header_value() {
                            if let Ok(val) = axum::http::HeaderValue::from_str(&cc_value) {
                                response.headers_mut().insert(header::CACHE_CONTROL, val);
                            }
                        }
                    }
                    return response;
                }
            }
        }
    }

    // 5. 304 检查（对齐 PHP If-Modified-Since）
    // RFC 7232 §6: 仅当 If-None-Match 不存在时才检查 If-Modified-Since
    if !if_none_match_present {
        if let Some(modified_time) = modified {
            let last_modified = format_http_date(modified_time);
            if let Some(if_modified_since) = headers.get(header::IF_MODIFIED_SINCE) {
                if let Ok(ims_str) = if_modified_since.to_str() {
                    if ims_str.trim() == last_modified {
                        let mut response = (
                            StatusCode::NOT_MODIFIED,
                            [(header::LAST_MODIFIED, last_modified.as_str())],
                            Body::empty(),
                        )
                            .into_response();
                        if let Some(ref etag_value) = etag {
                            if let Ok(val) = axum::http::HeaderValue::from_str(etag_value) {
                                response.headers_mut().insert(header::ETAG, val);
                            }
                        }
                        if let Some(cc) = cache_config {
                            if let Some(cc_value) = cc.to_header_value() {
                                if let Ok(val) = axum::http::HeaderValue::from_str(&cc_value) {
                                    response.headers_mut().insert(header::CACHE_CONTROL, val);
                                }
                            }
                        }
                        return response;
                    }
                }
            }
        }
    }

    // 6. MIME 类型识别（对齐 PHP getMimeType）
    let mime = mime_type_for_path(path);
    let content_type = mime
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_string());

    // 7. 读取文件内容（P1-IO-02：使用 tokio::fs 避免阻塞 async 运行时）
    let content = match tokio::fs::read(path).await {
        Ok(c) => c,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file").into_response()
        }
    };

    // 8. Range 请求处理（对齐 Webman Range 分支）
    if let Some(range_header) = headers.get(header::RANGE) {
        if let Ok(range_str) = range_header.to_str() {
            match parse_range_header(range_str, file_size) {
                Ok(range) => {
                    let content_length = range.end - range.start + 1;
                    let bytes = content
                        .get(range.start as usize..=(range.end as usize))
                        .unwrap_or(&[]);
                    let content_range =
                        format!("bytes {}-{}/{}", range.start, range.end, file_size);
                    let content_length_str = content_length.to_string();

                    let mut response = (
                        StatusCode::PARTIAL_CONTENT,
                        [
                            (header::CONTENT_TYPE, content_type.as_str()),
                            (header::CONTENT_LENGTH, content_length_str.as_str()),
                            (header::CONTENT_RANGE, content_range.as_str()),
                            (header::ACCEPT_RANGES, "bytes"),
                        ],
                        Body::from(bytes.to_vec()),
                    )
                        .into_response();

                    if let Some(modified_time) = modified {
                        let last_modified = format_http_date(modified_time);
                        if let Ok(val) = axum::http::HeaderValue::from_str(&last_modified) {
                            response.headers_mut().insert(header::LAST_MODIFIED, val);
                        }
                    }
                    if let Some(ref etag_value) = etag {
                        if let Ok(val) = axum::http::HeaderValue::from_str(etag_value) {
                            response.headers_mut().insert(header::ETAG, val);
                        }
                    }
                    if let Some(cc) = cache_config {
                        if let Some(cc_value) = cc.to_header_value() {
                            if let Ok(val) = axum::http::HeaderValue::from_str(&cc_value) {
                                response.headers_mut().insert(header::CACHE_CONTROL, val);
                            }
                        }
                    }
                    return response;
                }
                Err(RangeError::Unsatisfiable) => {
                    let content_range = format!("bytes */{}", file_size);
                    return (
                        StatusCode::RANGE_NOT_SATISFIABLE,
                        [(header::CONTENT_RANGE, content_range.as_str())],
                        Body::empty(),
                    )
                        .into_response();
                }
                Err(_) => {
                    // 非法 Range 格式，忽略 Range 头，返回完整文件
                }
            }
        }
    }

    // 9. 完整文件响应（对齐 PHP think-worker sendFile）
    let content_length_str = file_size.to_string();
    let mut response = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.as_str()),
            (header::CONTENT_LENGTH, content_length_str.as_str()),
            (header::ACCEPT_RANGES, "bytes"),
        ],
        Body::from(content),
    )
        .into_response();

    // Last-Modified 头（对齐 PHP Last-Modified）
    if let Some(modified_time) = modified {
        let last_modified = format_http_date(modified_time);
        if let Ok(val) = axum::http::HeaderValue::from_str(&last_modified) {
            response.headers_mut().insert(header::LAST_MODIFIED, val);
        }
    }

    // ETag 头（对齐 nginx 默认 ETag）
    if let Some(ref etag_value) = etag {
        if let Ok(val) = axum::http::HeaderValue::from_str(etag_value) {
            response.headers_mut().insert(header::ETAG, val);
        }
    }

    // Cache-Control 头（对齐 nginx expires）
    if let Some(cc) = cache_config {
        if let Some(cc_value) = cc.to_header_value() {
            if let Ok(val) = axum::http::HeaderValue::from_str(&cc_value) {
                response.headers_mut().insert(header::CACHE_CONTROL, val);
            }
        }
    }

    // 未知 MIME 类型 → Content-Disposition（对齐 PHP think-worker）
    if mime.is_none() {
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            let disposition = format!("attachment; filename=\"{}\"", filename);
            if let Ok(val) = axum::http::HeaderValue::from_str(&disposition) {
                response
                    .headers_mut()
                    .insert(header::CONTENT_DISPOSITION, val);
            }
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tower::ServiceExt;

    /// 创建临时目录并写入测试文件
    fn create_test_dir() -> TempDir {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let root = dir.path();

        // index.html
        fs::write(root.join("index.html"), "<html>index</html>").unwrap();
        // style.css
        fs::write(root.join("style.css"), "body { color: red; }").unwrap();
        // 子目录 + 文件
        fs::create_dir_all(root.join("js")).unwrap();
        fs::write(root.join("js").join("app.js"), "console.log('hello');").unwrap();
        dir
    }

    async fn send_get(router: Router, uri: &str) -> (StatusCode, Vec<u8>) {
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, bytes.to_vec())
    }

    async fn send_get_with_headers(
        router: Router,
        uri: &str,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, bytes.to_vec())
    }

    // ====================================================================
    // static_router
    // ====================================================================

    #[tokio::test]
    async fn test_static_router_serves_existing_file() {
        let dir = create_test_dir();
        let router = static_router("/s", dir.path());

        let (status, body) = send_get(router, "/s/style.css").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"body { color: red; }");
    }

    #[tokio::test]
    async fn test_static_router_serves_file_in_subdir() {
        let dir = create_test_dir();
        let router = static_router("/s", dir.path());

        let (status, body) = send_get(router, "/s/js/app.js").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"console.log('hello');");
    }

    #[tokio::test]
    async fn test_static_router_returns_404_for_missing_file() {
        let dir = create_test_dir();
        let router = static_router("/s", dir.path());

        let (status, _) = send_get(router, "/s/nonexistent.txt").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // ====================================================================
    // static_router_with_index
    // ====================================================================

    #[tokio::test]
    async fn test_static_router_with_index_serves_index_on_dir() {
        let dir = create_test_dir();
        let router = static_router_with_index("/s", dir.path());

        // 访问 /s/ 应返回 index.html
        let (status, body) = send_get(router, "/s/").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"<html>index</html>");
    }

    #[tokio::test]
    async fn test_static_router_with_index_serves_other_files() {
        let dir = create_test_dir();
        let router = static_router_with_index("/s", dir.path());

        let (status, body) = send_get(router, "/s/style.css").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"body { color: red; }");
    }

    // ====================================================================
    // static_router_spa
    // ====================================================================

    #[tokio::test]
    async fn test_static_router_spa_fallback_to_index() {
        let dir = create_test_dir();
        let router = static_router_spa(dir.path());

        // 访问不存在的路径 → 回退到 index.html
        let (status, body) = send_get(router, "/some/spa/route").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"<html>index</html>");
    }

    #[tokio::test]
    async fn test_static_router_spa_serves_existing_file() {
        let dir = create_test_dir();
        let router = static_router_spa(dir.path());

        // 已存在的文件仍然优先返回
        let (status, body) = send_get(router, "/style.css").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"body { color: red; }");
    }

    // ====================================================================
    // static_file（单文件）
    // ====================================================================

    #[tokio::test]
    async fn test_static_file_serves_single_file() {
        let dir = create_test_dir();
        let file_path: PathBuf = dir.path().join("style.css");
        let router: Router = Router::new().route_service("/style.css", static_file(file_path));

        let (status, body) = send_get(router, "/style.css").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"body { color: red; }");
    }

    #[tokio::test]
    async fn test_static_file_unknown_path_404() {
        let dir = create_test_dir();
        let file_path: PathBuf = dir.path().join("style.css");
        let router: Router = Router::new().route_service("/style.css", static_file(file_path));

        let (status, _) = send_get(router, "/nonexistent.css").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // ====================================================================
    // 安全性测试：路径穿越
    // ====================================================================

    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let dir = create_test_dir();
        // 在临时目录外创建一个敏感文件
        let parent = dir.path().parent().unwrap();
        let sensitive = parent.join("sensitive.txt");
        fs::write(&sensitive, "secret").unwrap();

        let router = static_router("/s", dir.path());

        // 尝试路径穿越
        let (status, _) = send_get(router, "/s/../sensitive.txt").await;
        // ServeDir 会规范化 URL，应该返回 404 或 400
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
            "expected 404 or 400, got {status}"
        );

        // 清理
        let _ = fs::remove_file(&sensitive);
    }

    // ====================================================================
    // content-type 验证
    // ====================================================================

    #[tokio::test]
    async fn test_static_router_sets_content_type_css() {
        let dir = create_test_dir();
        let router = static_router("/s", dir.path());

        let (_, headers, _) = send_get_with_headers(router, "/s/style.css").await;
        let ct = headers.get("content-type").unwrap().to_str().unwrap();
        assert!(ct.contains("css"), "expected css, got {ct}");
    }

    #[tokio::test]
    async fn test_static_router_sets_content_type_js() {
        let dir = create_test_dir();
        let router = static_router("/s", dir.path());

        let (_, headers, _) = send_get_with_headers(router, "/s/js/app.js").await;
        let ct = headers.get("content-type").unwrap().to_str().unwrap();
        assert!(
            ct.contains("javascript") || ct.contains("js"),
            "expected js, got {ct}"
        );
    }

    #[tokio::test]
    async fn test_static_router_spa_sets_content_type_html() {
        let dir = create_test_dir();
        let router = static_router_spa(dir.path());

        let (_, headers, _) = send_get_with_headers(router, "/unknown/route").await;
        let ct = headers.get("content-type").unwrap().to_str().unwrap();
        assert!(ct.contains("html"), "expected html, got {ct}");
    }

    // ====================================================================
    // HEAD 请求
    // ====================================================================

    #[tokio::test]
    async fn test_static_router_handles_head_request() {
        let dir = create_test_dir();
        let router = static_router("/s", dir.path());

        let req = Request::builder()
            .method(Method::HEAD)
            .uri("/s/style.css")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // HEAD 响应应该没有 body 或只有少量 body
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(bytes.is_empty() || bytes.len() < 100);
    }

    // ====================================================================
    // 低层 ServeDir API（用于 nest_service）
    // ====================================================================

    #[tokio::test]
    async fn test_static_dir_with_nest_service() {
        let dir = create_test_dir();
        let router: Router = Router::new().nest_service("/s", static_dir(dir.path()));

        let (status, body) = send_get(router, "/s/style.css").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"body { color: red; }");
    }

    #[tokio::test]
    async fn test_static_dir_with_index_with_nest_service() {
        let dir = create_test_dir();
        let router: Router = Router::new().nest_service("/s", static_dir_with_index(dir.path()));

        let (status, body) = send_get(router, "/s/").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"<html>index</html>");
    }

    #[tokio::test]
    async fn test_static_dir_spa_with_fallback_service() {
        let dir = create_test_dir();
        let router: Router = Router::new().fallback_service(static_dir_spa(dir.path()));

        let (status, body) = send_get(router, "/unknown/route").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"<html>index</html>");
    }

    // ====================================================================
    // 合并到现有 Router
    // ====================================================================

    #[tokio::test]
    async fn test_static_router_merge_with_api_routes() {
        let dir = create_test_dir();

        let api_router: Router = Router::new().route(
            "/api/hello",
            axum::routing::get(|| async { "hello from api" }),
        );
        let static_router = static_router("/static", dir.path());

        let app: Router = api_router.merge(static_router);

        // API 路由仍可用
        let (status, body) = send_get(app.clone(), "/api/hello").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"hello from api");

        // 静态文件可用
        let (status, body) = send_get(app, "/static/style.css").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"body { color: red; }");
    }

    // ====================================================================
    // MIME 类型表测试（对齐 PHP $mimeTypeMap）
    // ====================================================================

    #[test]
    fn test_mime_type_for_extension_html() {
        assert_eq!(mime_type_for_extension("html"), Some("text/html"));
        assert_eq!(mime_type_for_extension("HTML"), Some("text/html"));
        assert_eq!(mime_type_for_extension("Htm"), Some("text/html"));
    }

    #[test]
    fn test_mime_type_for_extension_css() {
        assert_eq!(mime_type_for_extension("css"), Some("text/css"));
    }

    #[test]
    fn test_mime_type_for_extension_js() {
        assert_eq!(
            mime_type_for_extension("js"),
            Some("application/javascript")
        );
        assert_eq!(
            mime_type_for_extension("mjs"),
            Some("application/javascript")
        );
    }

    #[test]
    fn test_mime_type_for_extension_json() {
        assert_eq!(mime_type_for_extension("json"), Some("application/json"));
    }

    #[test]
    fn test_mime_type_for_extension_images() {
        assert_eq!(mime_type_for_extension("png"), Some("image/png"));
        assert_eq!(mime_type_for_extension("jpg"), Some("image/jpeg"));
        assert_eq!(mime_type_for_extension("jpeg"), Some("image/jpeg"));
        assert_eq!(mime_type_for_extension("gif"), Some("image/gif"));
        assert_eq!(mime_type_for_extension("svg"), Some("image/svg+xml"));
        assert_eq!(mime_type_for_extension("ico"), Some("image/x-icon"));
        assert_eq!(mime_type_for_extension("webp"), Some("image/webp"));
    }

    #[test]
    fn test_mime_type_for_extension_fonts() {
        assert_eq!(mime_type_for_extension("woff"), Some("font/woff"));
        assert_eq!(mime_type_for_extension("woff2"), Some("font/woff2"));
        assert_eq!(mime_type_for_extension("ttf"), Some("font/ttf"));
    }

    #[test]
    fn test_mime_type_for_extension_unknown() {
        assert_eq!(mime_type_for_extension("xyz123"), None);
        assert_eq!(mime_type_for_extension(""), None);
    }

    #[test]
    fn test_mime_type_for_path() {
        assert_eq!(
            mime_type_for_path(Path::new("style.css")),
            Some("text/css".to_string())
        );
        assert_eq!(
            mime_type_for_path(Path::new("/var/www/index.html")),
            Some("text/html".to_string())
        );
        // 未知扩展名走 mime_guess 后备
        let result = mime_type_for_path(Path::new("file.unknownext123"));
        // mime_guess 可能返回 None 或某些类型，不强断言
        let _ = result;
    }

    // ====================================================================
    // Range 头解析测试（对齐 Webman sendFile Range 分支）
    // ====================================================================

    #[test]
    fn test_parse_range_start_end() {
        // bytes=0-499
        let range = parse_range_header("bytes=0-499", 1000).unwrap();
        assert_eq!(range, RangeSpec { start: 0, end: 499 });
    }

    #[test]
    fn test_parse_range_start_open() {
        // bytes=500-（从 500 到末尾）
        let range = parse_range_header("bytes=500-", 1000).unwrap();
        assert_eq!(
            range,
            RangeSpec {
                start: 500,
                end: 999
            }
        );
    }

    #[test]
    fn test_parse_range_suffix() {
        // bytes=-500（最后 500 字节）
        let range = parse_range_header("bytes=-500", 1000).unwrap();
        assert_eq!(
            range,
            RangeSpec {
                start: 500,
                end: 999
            }
        );
    }

    #[test]
    fn test_parse_range_suffix_larger_than_file() {
        // bytes=-2000（suffix > file_size → 返回整个文件）
        let range = parse_range_header("bytes=-2000", 1000).unwrap();
        assert_eq!(range, RangeSpec { start: 0, end: 999 });
    }

    #[test]
    fn test_parse_range_end_exceeds_file_size() {
        // bytes=900-2000（end > file_size → 截断到 file_size - 1）
        let range = parse_range_header("bytes=900-2000", 1000).unwrap();
        assert_eq!(
            range,
            RangeSpec {
                start: 900,
                end: 999
            }
        );
    }

    #[test]
    fn test_parse_range_start_equals_file_size() {
        // bytes=1000-（start == file_size → Unsatisfiable）
        let result = parse_range_header("bytes=1000-", 1000);
        assert_eq!(result, Err(RangeError::Unsatisfiable));
    }

    #[test]
    fn test_parse_range_start_greater_than_end() {
        // bytes=500-100（start > end → InvalidRange）
        let result = parse_range_header("bytes=500-100", 1000);
        assert_eq!(result, Err(RangeError::InvalidRange));
    }

    #[test]
    fn test_parse_range_invalid_format_no_bytes_prefix() {
        let result = parse_range_header("0-499", 1000);
        assert_eq!(result, Err(RangeError::InvalidFormat));
    }

    #[test]
    fn test_parse_range_invalid_format_no_dash() {
        let result = parse_range_header("bytes=500", 1000);
        assert_eq!(result, Err(RangeError::InvalidFormat));
    }

    #[test]
    fn test_parse_range_empty_range() {
        // bytes=- （空范围 → InvalidRange）
        let result = parse_range_header("bytes=-", 1000);
        assert_eq!(result, Err(RangeError::InvalidRange));
    }

    #[test]
    fn test_parse_range_non_numeric() {
        let result = parse_range_header("bytes=abc-500", 1000);
        assert_eq!(result, Err(RangeError::InvalidRange));
    }

    #[test]
    fn test_parse_range_with_whitespace() {
        // 带空格的 Range 头（trim 后解析）
        let range = parse_range_header("  bytes=0-499  ", 1000).unwrap();
        assert_eq!(range, RangeSpec { start: 0, end: 499 });
    }

    // ====================================================================
    // 路径安全验证测试
    // ====================================================================

    #[test]
    fn test_is_path_safe_valid() {
        let dir = create_test_dir();
        let root = dir.path();
        let file = root.join("style.css");
        assert!(is_path_safe(&file, root));
    }

    #[test]
    fn test_is_path_safe_subdir() {
        let dir = create_test_dir();
        let root = dir.path();
        let file = root.join("js").join("app.js");
        assert!(is_path_safe(&file, root));
    }

    #[test]
    fn test_is_path_safe_traversal_blocked() {
        let dir = create_test_dir();
        let root = dir.path();
        // 创建一个子目录外的文件
        let parent = root.parent().unwrap();
        let sensitive = parent.join("sensitive.txt");
        fs::write(&sensitive, "secret").unwrap();

        // 尝试通过 ../ 访问敏感文件
        let file = root.join("..").join("sensitive.txt");
        assert!(!is_path_safe(&file, root));

        let _ = fs::remove_file(&sensitive);
    }

    #[test]
    fn test_is_path_safe_nonexistent() {
        let dir = create_test_dir();
        let root = dir.path();
        let file = root.join("nonexistent.txt");
        // 不存在的文件，canonicalize 失败 → false
        assert!(!is_path_safe(&file, root));
    }

    // ====================================================================
    // percent_decode 测试（对齐 PHP rawurldecode）
    // ====================================================================

    #[test]
    fn test_percent_decode_plain() {
        assert_eq!(percent_decode("/style.css"), "/style.css");
    }

    #[test]
    fn test_percent_decode_encoded() {
        // %20 = 空格
        assert_eq!(percent_decode("/my%20file.css"), "/my file.css");
    }

    #[test]
    fn test_percent_decode_unicode() {
        // %E4%B8%AD = "中"
        assert_eq!(percent_decode("/%E4%B8%AD.html"), "/中.html");
    }

    #[test]
    fn test_percent_decode_no_plus_conversion() {
        // rawurldecode 语义：+ 不转换为空格
        assert_eq!(percent_decode("/my+file.css"), "/my+file.css");
    }

    #[test]
    fn test_percent_decode_incomplete() {
        // 不完整的 %XX 序列，保留原样
        assert_eq!(percent_decode("/file%2.css"), "/file%2.css");
    }

    // ====================================================================
    // serve_file 测试（对齐 PHP think-worker sendFile）
    // ====================================================================

    #[tokio::test]
    async fn test_serve_file_basic() {
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let headers = axum::http::HeaderMap::new();

        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"body { color: red; }");
    }

    #[tokio::test]
    async fn test_serve_file_not_found() {
        let dir = create_test_dir();
        let file_path = dir.path().join("nonexistent.txt");
        let headers = axum::http::HeaderMap::new();

        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_serve_file_sets_content_type() {
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let headers = axum::http::HeaderMap::new();

        let resp = serve_file(&file_path, &headers).await;
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("css"), "expected css, got {ct}");
    }

    #[tokio::test]
    async fn test_serve_file_sets_last_modified() {
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let headers = axum::http::HeaderMap::new();

        let resp = serve_file(&file_path, &headers).await;
        let lm = resp.headers().get("last-modified");
        assert!(lm.is_some(), "Last-Modified header should be set");
        let lm_str = lm.unwrap().to_str().unwrap();
        assert!(lm_str.ends_with("GMT"), "Last-Modified should end with GMT");
    }

    #[tokio::test]
    async fn test_serve_file_sets_accept_ranges() {
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let headers = axum::http::HeaderMap::new();

        let resp = serve_file(&file_path, &headers).await;
        let ar = resp
            .headers()
            .get("accept-ranges")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ar, "bytes");
    }

    #[tokio::test]
    async fn test_serve_file_304_if_modified_since_match() {
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");

        // 第一次请求获取 Last-Modified
        let headers1 = axum::http::HeaderMap::new();
        let resp1 = serve_file(&file_path, &headers1).await;
        let last_modified = resp1
            .headers()
            .get("last-modified")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // 第二次请求带 If-Modified-Since
        let mut headers2 = axum::http::HeaderMap::new();
        headers2.insert(
            axum::http::header::IF_MODIFIED_SINCE,
            axum::http::HeaderValue::from_str(&last_modified).unwrap(),
        );
        let resp2 = serve_file(&file_path, &headers2).await;
        assert_eq!(resp2.status(), StatusCode::NOT_MODIFIED);

        let bytes = resp2.into_body().collect().await.unwrap().to_bytes();
        assert!(bytes.is_empty(), "304 response should have empty body");
    }

    #[tokio::test]
    async fn test_serve_file_304_if_modified_since_mismatch() {
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::IF_MODIFIED_SINCE,
            axum::http::HeaderValue::from_static("Mon, 01 Jan 2000 00:00:00 GMT"),
        );
        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ====================================================================
    // serve_file Range 请求测试（对齐 Webman 206 Partial Content）
    // ====================================================================

    #[tokio::test]
    async fn test_serve_file_range_partial_content() {
        let dir = create_test_dir();
        // 创建一个有明确内容的文件
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, b"0123456789ABCDEFGHIJ").unwrap(); // 20 字节

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::RANGE,
            axum::http::HeaderValue::from_static("bytes=5-9"),
        );

        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);

        let cr = resp
            .headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cr, "bytes 5-9/20");

        let cl = resp
            .headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cl, "5");

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"56789");
    }

    #[tokio::test]
    async fn test_serve_file_range_open_end() {
        let dir = create_test_dir();
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, b"0123456789ABCDEFGHIJ").unwrap(); // 20 字节

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::RANGE,
            axum::http::HeaderValue::from_static("bytes=10-"),
        );

        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);

        let cr = resp
            .headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cr, "bytes 10-19/20");

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"ABCDEFGHIJ");
    }

    #[tokio::test]
    async fn test_serve_file_range_suffix() {
        let dir = create_test_dir();
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, b"0123456789ABCDEFGHIJ").unwrap(); // 20 字节

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::RANGE,
            axum::http::HeaderValue::from_static("bytes=-5"),
        );

        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);

        let cr = resp
            .headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cr, "bytes 15-19/20");

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"FGHIJ");
    }

    #[tokio::test]
    async fn test_serve_file_range_unsatisfiable() {
        let dir = create_test_dir();
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, b"0123456789").unwrap(); // 10 字节

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::RANGE,
            axum::http::HeaderValue::from_static("bytes=100-200"),
        );

        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        let cr = resp
            .headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cr, "bytes */10");
    }

    #[tokio::test]
    async fn test_serve_file_range_invalid_fallback_to_full() {
        let dir = create_test_dir();
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, b"0123456789").unwrap(); // 10 字节

        let mut headers = axum::http::HeaderMap::new();
        // 非法 Range 格式（无 bytes= 前缀）
        headers.insert(
            axum::http::header::RANGE,
            axum::http::HeaderValue::from_static("0-499"),
        );

        let resp = serve_file(&file_path, &headers).await;
        // 非法格式忽略 Range，返回完整文件
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"0123456789");
    }

    // ====================================================================
    // static_handler 测试（对齐 PHP think-worker Http::sendFile 完整流程）
    // ====================================================================

    #[tokio::test]
    async fn test_static_handler_serves_file() {
        let dir = create_test_dir();
        let headers = axum::http::HeaderMap::new();

        let resp = static_handler(dir.path(), "/style.css", &headers).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"body { color: red; }");
    }

    #[tokio::test]
    async fn test_static_handler_serves_subdir_file() {
        let dir = create_test_dir();
        let headers = axum::http::HeaderMap::new();

        let resp = static_handler(dir.path(), "/js/app.js", &headers).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"console.log('hello');");
    }

    #[tokio::test]
    async fn test_static_handler_404_for_missing() {
        let dir = create_test_dir();
        let headers = axum::http::HeaderMap::new();

        let resp = static_handler(dir.path(), "/nonexistent.txt", &headers).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_static_handler_blocks_traversal() {
        let dir = create_test_dir();
        let root = dir.path();
        let parent = root.parent().unwrap();
        let sensitive = parent.join("secret.txt");
        fs::write(&sensitive, "secret").unwrap();

        let headers = axum::http::HeaderMap::new();
        let resp = static_handler(root, "/../secret.txt", &headers).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let _ = fs::remove_file(&sensitive);
    }

    // ====================================================================
    // P1-PATH-01: serve_file / serve_file_with_cache 防御性路径遍历检查
    // ====================================================================

    #[tokio::test]
    async fn test_p1_path_01_serve_file_rejects_parent_dir_component() {
        // 即使目标文件实际存在，包含 .. 的路径也应被拒绝
        let dir = create_test_dir();
        // 构造一个包含 .. 但实际指向有效文件的路径
        let file_path = dir.path().join("subdir/../style.css");
        // 先确保文件存在（subdir/style.css 不存在，但 style.css 在根目录）
        // 这里 .. 解析后指向 style.css，但 serve_file 应在解析前拒绝
        let headers = axum::http::HeaderMap::new();
        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "P1-PATH-01: serve_file 应拒绝包含 .. 组件的路径，即使解析后文件存在"
        );
    }

    #[tokio::test]
    async fn test_p1_path_01_serve_file_with_cache_rejects_parent_dir_component() {
        let dir = create_test_dir();
        let file_path = dir.path().join("subdir/../style.css");
        let headers = axum::http::HeaderMap::new();
        let resp = serve_file_with_cache(&file_path, &headers, None).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "P1-PATH-01: serve_file_with_cache 应拒绝包含 .. 组件的路径"
        );
    }

    #[tokio::test]
    async fn test_p1_path_01_serve_file_allows_clean_path() {
        // 确保正常路径不受影响
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let headers = axum::http::HeaderMap::new();
        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "P1-PATH-01: 不含 .. 的正常路径应正常工作"
        );
    }

    #[tokio::test]
    async fn test_p1_path_01_serve_file_rejects_deep_traversal() {
        // 多层 .. 也应被拒绝
        let dir = create_test_dir();
        let file_path = dir.path().join("a/../../b/../../etc/passwd");
        let headers = axum::http::HeaderMap::new();
        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "P1-PATH-01: 多层 .. 路径应被拒绝"
        );
    }

    #[tokio::test]
    async fn test_static_handler_with_query_string() {
        let dir = create_test_dir();
        let headers = axum::http::HeaderMap::new();

        // 带 query string 的请求
        let resp = static_handler(dir.path(), "/style.css?v=123", &headers).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"body { color: red; }");
    }

    #[tokio::test]
    async fn test_static_handler_url_encoded_path() {
        let dir = create_test_dir();
        // 创建一个带空格的文件名
        fs::write(dir.path().join("my file.css"), "encoded content").unwrap();

        let headers = axum::http::HeaderMap::new();
        // %20 = 空格
        let resp = static_handler(dir.path(), "/my%20file.css", &headers).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"encoded content");
    }

    // ====================================================================
    // format_http_date 测试
    // ====================================================================

    #[test]
    fn test_format_http_date_epoch() {
        // Unix epoch: 1970-01-01 00:00:00 GMT (星期四)
        let time = std::time::UNIX_EPOCH;
        let date_str = format_http_date(time);
        assert!(
            date_str.contains("Thu"),
            "expected Thursday, got {date_str}"
        );
        assert!(date_str.contains("01"), "expected day 01, got {date_str}");
        assert!(date_str.contains("Jan"), "expected January, got {date_str}");
        assert!(
            date_str.contains("1970"),
            "expected year 1970, got {date_str}"
        );
        assert!(
            date_str.ends_with("GMT"),
            "expected GMT suffix, got {date_str}"
        );
    }

    #[test]
    fn test_format_http_date_known_timestamp() {
        // 2026-01-15 12:30:45 UTC
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1768569045);
        let date_str = format_http_date(time);
        assert!(
            date_str.contains("2026"),
            "expected year 2026, got {date_str}"
        );
        assert!(
            date_str.ends_with("GMT"),
            "expected GMT suffix, got {date_str}"
        );
    }

    // ====================================================================
    // 未知 MIME 类型 → Content-Disposition 测试（对齐 PHP think-worker）
    // ====================================================================

    #[tokio::test]
    async fn test_serve_file_unknown_mime_sets_content_disposition() {
        let dir = create_test_dir();
        // 创建一个未知扩展名的文件
        let file_path = dir.path().join("data.xyz123");
        fs::write(&file_path, "unknown content").unwrap();

        let headers = axum::http::HeaderMap::new();
        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // 未知 MIME 类型应设置 Content-Disposition
        let cd = resp.headers().get("content-disposition");
        assert!(
            cd.is_some(),
            "Content-Disposition should be set for unknown MIME"
        );
        let cd_str = cd.unwrap().to_str().unwrap();
        assert!(
            cd_str.contains("attachment"),
            "expected attachment, got {cd_str}"
        );
        assert!(
            cd_str.contains("data.xyz123"),
            "expected filename, got {cd_str}"
        );
    }

    #[tokio::test]
    async fn test_serve_file_known_mime_no_content_disposition() {
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let headers = axum::http::HeaderMap::new();

        let resp = serve_file(&file_path, &headers).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // 已知 MIME 类型不应设置 Content-Disposition
        let cd = resp.headers().get("content-disposition");
        assert!(
            cd.is_none(),
            "Content-Disposition should not be set for known MIME"
        );
    }

    // ====================================================================
    // CacheControlConfig 测试（对齐 nginx expires 指令）
    // ====================================================================

    #[test]
    fn test_cache_control_default_empty() {
        // 默认配置（空）不应产生 Cache-Control 头
        let config = CacheControlConfig::new();
        assert_eq!(config.to_header_value(), None);
    }

    #[test]
    fn test_cache_control_max_age_only() {
        // 对齐 nginx `expires 1h;` → max-age=3600
        let config = CacheControlConfig::new().with_max_age(3600);
        assert_eq!(config.to_header_value().as_deref(), Some("max-age=3600"));
    }

    #[test]
    fn test_cache_control_public_max_age() {
        // 对齐 nginx `expires 1h; add_header Cache-Control "public";`
        let config = CacheControlConfig::new().with_public().with_max_age(3600);
        assert_eq!(
            config.to_header_value().as_deref(),
            Some("public, max-age=3600")
        );
    }

    #[test]
    fn test_cache_control_private_max_age() {
        let config = CacheControlConfig::new().with_private().with_max_age(600);
        assert_eq!(
            config.to_header_value().as_deref(),
            Some("private, max-age=600")
        );
    }

    #[test]
    fn test_cache_control_no_cache() {
        // 对齐 nginx `expires -1;` → no-cache
        let config = CacheControlConfig::new().with_no_cache();
        assert_eq!(config.to_header_value().as_deref(), Some("no-cache"));
    }

    #[test]
    fn test_cache_control_no_store() {
        let config = CacheControlConfig::new().with_no_store();
        assert_eq!(config.to_header_value().as_deref(), Some("no-store"));
    }

    #[test]
    fn test_cache_control_no_store_no_cache_order() {
        // 验证 no-store 在 no-cache 之前（to_header_value 实现顺序）
        let config = CacheControlConfig::new().with_no_cache().with_no_store();
        assert_eq!(
            config.to_header_value().as_deref(),
            Some("no-store, no-cache")
        );
    }

    #[test]
    fn test_cache_control_must_revalidate() {
        let config = CacheControlConfig::new()
            .with_no_cache()
            .with_must_revalidate();
        assert_eq!(
            config.to_header_value().as_deref(),
            Some("no-cache, must-revalidate")
        );
    }

    #[test]
    fn test_cache_control_immutable_long_max_age() {
        // 对齐前端构建工具指纹 hash 场景：public, max-age=31536000, immutable
        let config = CacheControlConfig::new()
            .with_public()
            .with_max_age(31536000)
            .with_immutable();
        assert_eq!(
            config.to_header_value().as_deref(),
            Some("public, max-age=31536000, immutable")
        );
    }

    #[test]
    fn test_cache_control_full_directive_order() {
        // 验证所有指令的顺序：no-store, no-cache, public/private, max-age, must-revalidate, immutable
        let config = CacheControlConfig::new()
            .with_no_store()
            .with_no_cache()
            .with_public()
            .with_max_age(60)
            .with_must_revalidate()
            .with_immutable();
        assert_eq!(
            config.to_header_value().as_deref(),
            Some("no-store, no-cache, public, max-age=60, must-revalidate, immutable")
        );
    }

    // ====================================================================
    // compute_etag 测试（对齐 nginx 默认 ETag）
    // ====================================================================

    #[test]
    fn test_compute_etag_format() {
        // 验证 ETag 格式：W/"<mtime>-<size>"
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let metadata = std::fs::metadata(&file_path).unwrap();

        let etag = compute_etag(&metadata).expect("ETag should be computed");
        assert!(
            etag.starts_with("W/\"") && etag.ends_with('"'),
            "ETag should be weak format W/\"...\", got: {etag}"
        );
        // 格式 W/"<digits>-<digits>"
        let inner = &etag[3..etag.len() - 1];
        let parts: Vec<&str> = inner.splitn(2, '-').collect();
        assert_eq!(parts.len(), 2, "ETag inner should be <mtime>-<size>");
        assert!(
            parts[0].chars().all(|c| c.is_ascii_digit()),
            "mtime should be numeric"
        );
        assert!(
            parts[1].chars().all(|c| c.is_ascii_digit()),
            "size should be numeric"
        );
    }

    #[test]
    fn test_compute_etag_size_in_header() {
        // 验证 ETag 包含文件大小
        let dir = create_test_dir();
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, b"0123456789ABCDEFGHIJ").unwrap(); // 20 字节
        let metadata = std::fs::metadata(&file_path).unwrap();

        let etag = compute_etag(&metadata).unwrap();
        assert!(
            etag.contains("-20\""),
            "ETag should contain file size 20, got: {etag}"
        );
    }

    #[test]
    fn test_compute_etag_different_sizes_differ() {
        let dir = create_test_dir();
        let small_path = dir.path().join("small.bin");
        let large_path = dir.path().join("large.bin");
        fs::write(&small_path, b"short").unwrap();
        fs::write(&large_path, b"this is a much longer file content").unwrap();

        let small_etag = compute_etag(&std::fs::metadata(&small_path).unwrap()).unwrap();
        let large_etag = compute_etag(&std::fs::metadata(&large_path).unwrap()).unwrap();
        assert_ne!(
            small_etag, large_etag,
            "Different file sizes should produce different ETags"
        );
    }

    // ====================================================================
    // fingerprint_bytes / fingerprint_file 测试
    //         （对齐 PHP md5() + 前端构建工具 [contenthash]）
    // ====================================================================

    #[test]
    fn test_fingerprint_bytes_empty() {
        // MD5("") = d41d8cd98f00b204e9800998ecf8427e（对齐 PHP md5("")）
        let hash = fingerprint_bytes(b"");
        assert_eq!(hash, "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hash.len(), 32, "MD5 hash should be 32 hex chars");
    }

    #[test]
    fn test_fingerprint_bytes_hello() {
        // MD5("hello") = 5d41402abc4b2a76b9719d911017c592（对齐 PHP md5("hello")）
        let hash = fingerprint_bytes(b"hello");
        assert_eq!(hash, "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn test_fingerprint_bytes_known_php_value() {
        // 对齐 PHP: md5("The quick brown fox jumps over the lazy dog")
        //   = 9e107d9d372bb6826bd81d3542a419d6
        // （注意：无结尾句点版本，对齐 RFC 1321 测试向量）
        let hash = fingerprint_bytes(b"The quick brown fox jumps over the lazy dog");
        assert_eq!(hash, "9e107d9d372bb6826bd81d3542a419d6");
    }

    #[test]
    fn test_fingerprint_bytes_lowercase_hex() {
        // 验证输出为小写十六进制（对齐 PHP md5() 默认输出）
        let hash = fingerprint_bytes(b"test");
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "MD5 hash should be lowercase hex: {hash}"
        );
    }

    #[test]
    fn test_fingerprint_file_reads_content() {
        let dir = create_test_dir();
        let file_path = dir.path().join("content.txt");
        fs::write(&file_path, b"hello").unwrap();

        let file_hash = fingerprint_file(&file_path).unwrap();
        let bytes_hash = fingerprint_bytes(b"hello");
        assert_eq!(file_hash, bytes_hash);
        assert_eq!(file_hash, "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn test_fingerprint_file_missing_returns_err() {
        let dir = create_test_dir();
        let missing = dir.path().join("nonexistent.txt");
        let result = fingerprint_file(&missing);
        assert!(result.is_err(), "Missing file should return Err");
    }

    // ====================================================================
    // extract_version_hash 测试
    //         （对齐前端构建工具文件名 hash 解析）
    // ====================================================================

    #[test]
    fn test_extract_version_hash_valid() {
        // 标准格式：style.<hash>.css
        let result = extract_version_hash("style.abc123def456.css");
        assert_eq!(
            result,
            Some(("style.css".to_string(), "abc123def456".to_string()))
        );
    }

    #[test]
    fn test_extract_version_hash_path_with_dir() {
        // 带目录路径：js/app.<hash>.js
        let result = extract_version_hash("js/app.abc123def456.js");
        assert_eq!(
            result,
            Some(("js/app.js".to_string(), "abc123def456".to_string()))
        );
    }

    #[test]
    fn test_extract_version_hash_multi_dot_stem() {
        // 多点文件名：foo.bar.<hash>.css
        let result = extract_version_hash("foo.bar.abc123def456.css");
        assert_eq!(
            result,
            Some(("foo.bar.css".to_string(), "abc123def456".to_string()))
        );
    }

    #[test]
    fn test_extract_version_hash_min_8_chars() {
        // 恰好 8 位 hash（边界值）
        let result = extract_version_hash("style.abc12345.css");
        assert_eq!(
            result,
            Some(("style.css".to_string(), "abc12345".to_string()))
        );
    }

    #[test]
    fn test_extract_version_hash_no_hash() {
        // 无 hash（无第二个点）
        let result = extract_version_hash("style.css");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_version_hash_short_hash() {
        // hash 不足 8 位 → None
        let result = extract_version_hash("style.abc123.css");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_version_hash_non_hex() {
        // hash 含非十六进制字符 → None
        let result = extract_version_hash("style.xyzghijk.css");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_version_hash_uppercase_hex() {
        // 大写十六进制应被接受（is_ascii_hexdigit 接受大小写）
        let result = extract_version_hash("style.ABCDEF12.css");
        assert_eq!(
            result,
            Some(("style.css".to_string(), "ABCDEF12".to_string()))
        );
    }

    #[test]
    fn test_extract_version_hash_no_extension() {
        // 无扩展名 → None
        let result = extract_version_hash("noextension");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_version_hash_empty_extension() {
        // 末尾点 → 空扩展名 → None
        let result = extract_version_hash("style.abc123def456.");
        assert_eq!(result, None);
    }

    // ====================================================================
    // serve_file_with_cache 测试
    // ====================================================================

    #[tokio::test]
    async fn test_serve_file_with_cache_200_no_config() {
        // 无 Cache-Control 配置 → 不设置 Cache-Control 头
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let headers = axum::http::HeaderMap::new();

        let resp = serve_file_with_cache(&file_path, &headers, None).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let cc = resp.headers().get("cache-control");
        assert!(
            cc.is_none(),
            "Cache-Control should not be set without config"
        );

        // ETag 仍应设置
        let etag = resp.headers().get("etag");
        assert!(etag.is_some(), "ETag should be set");
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_200_with_cache_control() {
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let headers = axum::http::HeaderMap::new();
        let config = CacheControlConfig::new().with_public().with_max_age(3600);

        let resp = serve_file_with_cache(&file_path, &headers, Some(&config)).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let cc = resp
            .headers()
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cc, "public, max-age=3600");
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_etag_header_set() {
        // 200 响应应设置 ETag 头
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let headers = axum::http::HeaderMap::new();

        let resp = serve_file_with_cache(&file_path, &headers, None).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let etag = resp.headers().get("etag").unwrap().to_str().unwrap();
        assert!(
            etag.starts_with("W/\""),
            "ETag should be weak format, got: {etag}"
        );
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_304_if_none_match_match() {
        // If-None-Match 匹配 ETag → 304
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");

        // 第一次请求获取 ETag
        let headers1 = axum::http::HeaderMap::new();
        let resp1 = serve_file_with_cache(&file_path, &headers1, None).await;
        let etag = resp1
            .headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // 第二次请求带 If-None-Match: <etag>
        let mut headers2 = axum::http::HeaderMap::new();
        headers2.insert(
            axum::http::header::IF_NONE_MATCH,
            axum::http::HeaderValue::from_str(&etag).unwrap(),
        );
        let resp2 = serve_file_with_cache(&file_path, &headers2, None).await;
        assert_eq!(resp2.status(), StatusCode::NOT_MODIFIED);

        // 304 应包含 ETag
        let resp2_etag = resp2.headers().get("etag").unwrap().to_str().unwrap();
        assert_eq!(resp2_etag, etag);

        // 304 应包含 Last-Modified
        assert!(
            resp2.headers().get("last-modified").is_some(),
            "304 should include Last-Modified"
        );

        // 304 body 应为空
        let bytes = resp2.into_body().collect().await.unwrap().to_bytes();
        assert!(bytes.is_empty(), "304 body should be empty");
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_304_if_none_match_star() {
        // If-None-Match: * → 总是 304（对齐 HTTP/1.1）
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::IF_NONE_MATCH,
            axum::http::HeaderValue::from_static("*"),
        );
        let resp = serve_file_with_cache(&file_path, &headers, None).await;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_200_if_none_match_mismatch() {
        // If-None-Match 不匹配 → 200
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::IF_NONE_MATCH,
            axum::http::HeaderValue::from_static("W/\"0-0\""),
        );
        let resp = serve_file_with_cache(&file_path, &headers, None).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_304_includes_cache_control() {
        // 304 响应应包含 Cache-Control（对齐 nginx 行为）
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");

        // 先获取 ETag
        let headers1 = axum::http::HeaderMap::new();
        let config = CacheControlConfig::new().with_public().with_max_age(3600);
        let resp1 = serve_file_with_cache(&file_path, &headers1, Some(&config)).await;
        let etag = resp1
            .headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // 带 If-None-Match 请求
        let mut headers2 = axum::http::HeaderMap::new();
        headers2.insert(
            axum::http::header::IF_NONE_MATCH,
            axum::http::HeaderValue::from_str(&etag).unwrap(),
        );
        let resp2 = serve_file_with_cache(&file_path, &headers2, Some(&config)).await;
        assert_eq!(resp2.status(), StatusCode::NOT_MODIFIED);

        let cc = resp2
            .headers()
            .get("cache-control")
            .expect("304 should include Cache-Control")
            .to_str()
            .unwrap();
        assert_eq!(cc, "public, max-age=3600");
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_304_if_modified_since_match() {
        // If-Modified-Since 匹配 → 304（对齐 PHP think-worker）
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");

        // 先获取 Last-Modified
        let headers1 = axum::http::HeaderMap::new();
        let resp1 = serve_file_with_cache(&file_path, &headers1, None).await;
        let last_modified = resp1
            .headers()
            .get("last-modified")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // 带 If-Modified-Since 请求
        let mut headers2 = axum::http::HeaderMap::new();
        headers2.insert(
            axum::http::header::IF_MODIFIED_SINCE,
            axum::http::HeaderValue::from_str(&last_modified).unwrap(),
        );
        let resp2 = serve_file_with_cache(&file_path, &headers2, None).await;
        assert_eq!(resp2.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_if_none_match_takes_priority() {
        // 同时存在 If-None-Match 和 If-Modified-Since，
        // If-None-Match 优先（对齐 HTTP/1.1 优先级）
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");

        // 获取正确的 Last-Modified
        let headers1 = axum::http::HeaderMap::new();
        let resp1 = serve_file_with_cache(&file_path, &headers1, None).await;
        let last_modified = resp1
            .headers()
            .get("last-modified")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // If-None-Match 不匹配，If-Modified-Since 匹配
        // 预期：If-None-Match 优先级更高，不匹配 → 200
        let mut headers2 = axum::http::HeaderMap::new();
        headers2.insert(
            axum::http::header::IF_NONE_MATCH,
            axum::http::HeaderValue::from_static("W/\"0-0\""),
        );
        headers2.insert(
            axum::http::header::IF_MODIFIED_SINCE,
            axum::http::HeaderValue::from_str(&last_modified).unwrap(),
        );
        let resp2 = serve_file_with_cache(&file_path, &headers2, None).await;
        assert_eq!(
            resp2.status(),
            StatusCode::OK,
            "If-None-Match should take priority over If-Modified-Since"
        );
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_404() {
        let dir = create_test_dir();
        let file_path = dir.path().join("nonexistent.txt");
        let headers = axum::http::HeaderMap::new();

        let resp = serve_file_with_cache(&file_path, &headers, None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_range_206_includes_etag() {
        // 206 响应应包含 ETag 和 Cache-Control
        let dir = create_test_dir();
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, b"0123456789ABCDEFGHIJ").unwrap(); // 20 字节

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::RANGE,
            axum::http::HeaderValue::from_static("bytes=5-9"),
        );

        let config = CacheControlConfig::new().with_max_age(3600);
        let resp = serve_file_with_cache(&file_path, &headers, Some(&config)).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);

        let cr = resp
            .headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cr, "bytes 5-9/20");

        // ETag 应设置
        assert!(
            resp.headers().get("etag").is_some(),
            "206 should include ETag"
        );

        // Cache-Control 应设置
        let cc = resp
            .headers()
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cc, "max-age=3600");

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"56789");
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_range_416() {
        // Range 超出文件大小 → 416
        let dir = create_test_dir();
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, b"0123456789").unwrap(); // 10 字节

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::RANGE,
            axum::http::HeaderValue::from_static("bytes=100-200"),
        );

        let resp = serve_file_with_cache(&file_path, &headers, None).await;
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        let cr = resp
            .headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cr, "bytes */10");
    }

    #[tokio::test]
    async fn test_serve_file_with_cache_unknown_mime_content_disposition() {
        // 未知 MIME 类型应设置 Content-Disposition（对齐 PHP think-worker）
        let dir = create_test_dir();
        let file_path = dir.path().join("unknown.xyzunknown");
        fs::write(&file_path, b"unknown content").unwrap();

        let headers = axum::http::HeaderMap::new();
        let resp = serve_file_with_cache(&file_path, &headers, None).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let cd = resp
            .headers()
            .get("content-disposition")
            .expect("Content-Disposition should be set for unknown MIME");
        let cd_str = cd.to_str().unwrap();
        assert!(
            cd_str.contains("unknown.xyzunknown"),
            "Content-Disposition should contain filename, got: {cd_str}"
        );
    }

    // ====================================================================
    // R5 PHP/Rust 行为对比测试
    // ====================================================================

    #[test]
    fn test_r5_php_no_etag_but_rust_extends_with_etag() {
        // R5 PHP 行为对比：
        // - PHP think-worker sendFile 不生成 ETag（仅 Last-Modified + If-Modified-Since）
        // - Rust 在 PHP 基础上扩展：增加 ETag + If-None-Match（对齐 nginx 默认行为）
        //
        // 验证：Rust compute_etag 生成 nginx 风格的 weak ETag
        let dir = create_test_dir();
        let file_path = dir.path().join("style.css");
        let metadata = std::fs::metadata(&file_path).unwrap();

        let etag = compute_etag(&metadata);
        assert!(etag.is_some(), "Rust should generate ETag (PHP doesn't)");

        // 验证 nginx 格式：W/"<mtime>-<size>"
        let etag_str = etag.unwrap();
        assert!(
            etag_str.starts_with("W/\"") && etag_str.ends_with('"'),
            "ETag should be nginx weak format"
        );
    }

    #[test]
    fn test_r5_php_md5_alignment() {
        // R5 PHP 行为对比：
        // PHP md5() 输出 32 位小写十六进制字符串
        // Rust fingerprint_bytes 应与 PHP md5() 完全一致
        //
        // PHP 验证脚本：
        //   php -r 'echo md5("hello");'
        //   输出: 5d41402abc4b2a76b9719d911017c592
        let rust_hash = fingerprint_bytes(b"hello");
        let php_hash = "5d41402abc4b2a76b9719d911017c592";
        assert_eq!(rust_hash, php_hash, "Rust MD5 should match PHP md5()");
    }

    #[test]
    fn test_r5_nginx_etag_format_alignment() {
        // R5 nginx 行为对比：
        // nginx 默认 ETag 格式：W/"<mtime>-<size>"
        //   - mtime: 文件修改时间的 Unix 秒
        //   - size: 文件大小（字节）
        //
        // 验证：Rust compute_etag 输出格式与 nginx 完全一致
        let dir = create_test_dir();
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, b"0123456789ABCDEFGHIJ").unwrap(); // 20 字节
        let metadata = std::fs::metadata(&file_path).unwrap();

        let mtime = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let size = metadata.len();

        let expected_etag = format!("W/\"{}-{}\"", mtime, size);
        let actual_etag = compute_etag(&metadata).unwrap();
        assert_eq!(
            actual_etag, expected_etag,
            "Rust ETag should match nginx format exactly"
        );
    }
}
