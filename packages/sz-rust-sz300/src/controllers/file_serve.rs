// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use std::path::PathBuf;
use tokio::fs;
use tokio_util::io::ReaderStream;

/// 上传路径解析错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UploadPathError {
    /// 路径包含 `..` 或 canonicalize 后逃逸出上传目录
    Traversal,
    /// 上传根目录不存在
    BaseMissing,
    /// 文件不存在（canonicalize 失败）
    NotFound,
}

/// 上传根目录（`SZ300_UPLOAD_ROOT`，默认 `./uploads`）
///
/// 容器/多实例部署时必须显式配置，避免工作目录变化导致文件 404。
fn upload_root_from(configured: Option<String>) -> PathBuf {
    configured
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./uploads"))
}

pub(crate) fn upload_root() -> PathBuf {
    upload_root_from(std::env::var("SZ300_UPLOAD_ROOT").ok())
}

/// 解析并校验上传目录内的文件路径（路径遍历防护核心逻辑）
///
/// 三重防护：
/// 1. 字符串层：拒绝 `..` 子串（含 URL 编码变体由 axum 解码后在此命中）
/// 2. 路径层：`canonicalize` 解析符号链接与 `../`
/// 3. 边界层：canonicalize 结果必须以 base 目录为前缀
pub(crate) fn resolve_upload_path(
    base_dir: &std::path::Path,
    relative: &str,
) -> Result<std::path::PathBuf, UploadPathError> {
    // 第一重：字符串层快速拒绝 `..`
    if relative.contains("..") {
        return Err(UploadPathError::Traversal);
    }

    let candidate = base_dir.join(relative);

    // 第二重 + 第三重：canonicalize 解析真实路径，并校验仍在 base 目录内
    let canonical_base = base_dir
        .canonicalize()
        .map_err(|_| UploadPathError::BaseMissing)?;
    let canonical_file = candidate
        .canonicalize()
        .map_err(|_| UploadPathError::NotFound)?;
    if !canonical_file.starts_with(&canonical_base) {
        return Err(UploadPathError::Traversal);
    }
    Ok(canonical_file)
}

/// 按扩展名映射 Content-Type（白名单，未命中一律 octet-stream）
///
/// 刻意不把 `svg` 映射为 `image/svg+xml`：SVG 可内嵌脚本，作为同源静态资源
/// 直接返回会引入存储型 XSS。
fn content_type_for(path: &std::path::Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

/// 静态文件服务 — 流式返回上传目录中的文件，并按扩展名设置 Content-Type
///
/// # 安全
///
/// - 路径遍历防护见 [`resolve_upload_path`]（三重校验）。
/// - 流式读取，避免大文件整块进内存。
#[tracing::instrument(skip_all)]
pub async fn serve_file(
    State(_state): State<AppState>,
    path: axum::extract::Path<String>,
) -> Response {
    let base_dir = upload_root();

    let canonical_file = match resolve_upload_path(&base_dir, &path.0) {
        Ok(p) => p,
        Err(UploadPathError::Traversal) => {
            return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
        }
        Err(UploadPathError::BaseMissing) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Upload dir missing").into_response();
        }
        Err(UploadPathError::NotFound) => {
            return (StatusCode::NOT_FOUND, "File not found").into_response();
        }
    };

    let file = match fs::File::open(&canonical_file).await {
        Ok(f) => f,
        Err(_) => return (StatusCode::NOT_FOUND, "File not found").into_response(),
    };
    let length = file.metadata().await.map(|m| m.len()).unwrap_or(0);

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type_for(&canonical_file)),
    );
    if let Ok(value) = HeaderValue::from_str(&length.to_string()) {
        headers.insert(header::CONTENT_LENGTH, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );

    (headers, Body::from_stream(ReaderStream::new(file))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_root_defaults_and_trims() {
        assert_eq!(upload_root_from(None), PathBuf::from("./uploads"));
        assert_eq!(
            upload_root_from(Some("  ".into())),
            PathBuf::from("./uploads")
        );
        assert_eq!(
            upload_root_from(Some("/data/uploads".into())),
            PathBuf::from("/data/uploads")
        );
    }

    #[test]
    fn content_type_whitelist() {
        assert_eq!(content_type_for(std::path::Path::new("a.PNG")), "image/png");
        assert_eq!(
            content_type_for(std::path::Path::new("a.svg")),
            "application/octet-stream"
        );
        assert_eq!(
            content_type_for(std::path::Path::new("a.unknownext")),
            "application/octet-stream"
        );
    }

    #[test]
    fn traversal_is_rejected_before_io() {
        let base = std::path::Path::new(".");
        assert_eq!(
            resolve_upload_path(base, "../etc/passwd"),
            Err(UploadPathError::Traversal)
        );
        assert_eq!(
            resolve_upload_path(base, "a/../../b"),
            Err(UploadPathError::Traversal)
        );
    }
}
