// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use crate::services::file_service;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::Multipart;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_core::controller::SzController;

struct FileController;
impl SzController for FileController {}

impl FileController {
    pub async fn upload(req: Request<Body>) -> Response {
        let ctrl = FileController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                // 从 JSON body 获取文件数据（base64 方式上传）
                let filename = data
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unnamed");
                let file_data = data.get("file").and_then(|v| v.as_str()).unwrap_or("");

                if file_data.is_empty() {
                    return ctrl.render_error("文件数据不能为空", json!({}), 0);
                }

                // base64 解码
                let bytes = match base64_decode(file_data) {
                    Ok(b) => b,
                    Err(e) => return ctrl.render_error(&e, json!({}), 0),
                };

                match file_service::FileService::save(filename, &bytes).await {
                    Ok(url) => ctrl.render_success("上传成功", json!({"url": url})),
                    Err(e) => ctrl.render_error(&e, json!({}), 0),
                }
            }
            Err(e) => ctrl.render_error(&e, json!({}), 0),
        }
    }

    pub async fn upload_multipart(mut multipart: Multipart) -> Response {
        let ctrl = FileController;

        let mut uploaded = Vec::new();
        while let Ok(Some(field)) = multipart.next_field().await {
            let filename = field.file_name().unwrap_or("unnamed").to_string();
            let data = match field.bytes().await {
                Ok(d) => d.to_vec(),
                Err(e) => return ctrl.render_error(format!("读取文件失败: {}", e), json!({}), 0),
            };

            match file_service::FileService::save(&filename, &data).await {
                Ok(url) => uploaded.push(json!({"filename": filename, "url": url})),
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            }
        }

        if uploaded.is_empty() {
            return ctrl.render_error("未接收到文件", json!({}), 0);
        }

        ctrl.render_success("上传成功", json!({"files": uploaded}))
    }
}

/// 文件上传（对齐 PHP FileController::upload）
#[tracing::instrument(skip(_state, req))]
pub async fn upload(State(_state): State<AppState>, req: Request<Body>) -> Response {
    FileController::upload(req).await
}

/// 多部分文件上传（对齐 PHP FileController::uploadMultipart）
#[tracing::instrument(skip(_state, multipart))]
pub async fn upload_multipart(State(_state): State<AppState>, multipart: Multipart) -> Response {
    FileController::upload_multipart(multipart).await
}

/// v1.8.0 增强上传（使用 UploadConfig 校验 + LocalStorage 存储）
#[cfg(feature = "v18-upload")]
#[tracing::instrument(skip(state, multipart))]
pub async fn upload_enhanced(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    use sz_rust_upload::multipart::{MultipartConfig, MultipartFile};
    use sz_rust_upload::storage::StorageBackend;

    let cfg = &state.upload_config;
    let mp_cfg = MultipartConfig::new()
        .with_max_file_size(cfg.max_size)
        .with_extension("");
    let mp_cfg = cfg
        .allowed_extensions
        .iter()
        .fold(mp_cfg, |c, ext| c.with_extension(ext));

    let mut uploaded = Vec::new();
    let mut errors = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let filename = field.file_name().unwrap_or("unnamed").to_string();
        let mime_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        let data = match field.bytes().await {
            Ok(d) => d.to_vec(),
            Err(e) => {
                errors.push(format!("读取文件 {filename} 失败: {e}"));
                continue;
            }
        };

        let file = MultipartFile::new(&filename, &mime_type, data);

        if let Err(e) = file.validate_file_name() {
            errors.push(format!("{filename}: {e}"));
            continue;
        }
        if let Err(e) = mp_cfg.validate(&filename, file.size, &mime_type) {
            errors.push(format!("{filename}: {e}"));
            continue;
        }

        let save_result = match &cfg.backend {
            StorageBackend::Local { root } => save_local(root, &filename, &file.data).await,
            StorageBackend::S3 { endpoint, bucket } => {
                Err(format!("S3 存储尚未实现: {endpoint}/{bucket}"))
            }
            StorageBackend::MinIO { endpoint, bucket } => {
                Err(format!("MinIO 存储尚未实现: {endpoint}/{bucket}"))
            }
            StorageBackend::OSS { endpoint, bucket } => {
                Err(format!("OSS 存储尚未实现: {endpoint}/{bucket}"))
            }
        };

        match save_result {
            Ok(url) => uploaded.push(json!({"filename": filename, "url": url, "size": file.size})),
            Err(e) => errors.push(format!("{filename}: {e}")),
        }
    }

    if uploaded.is_empty() && errors.is_empty() {
        return error_response("未接收到文件");
    }

    if !uploaded.is_empty() {
        success_response("上传成功", json!({"files": uploaded, "errors": errors}))
    } else {
        error_response(&errors.join("; "))
    }
}

/// v1.8.0 保存到本地存储
#[cfg(feature = "v18-upload")]
async fn save_local(root: &std::path::Path, filename: &str, data: &[u8]) -> Result<String, String> {
    use std::path::PathBuf;
    use uuid::Uuid;

    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");

    let date_dir = chrono::Local::now().format("%Y/%m/%d").to_string();
    let new_filename = format!(
        "{}_{}.{}",
        chrono::Local::now().format("%Y%m%d_%H%M%S"),
        &Uuid::new_v4().to_string()[..8],
        ext
    );

    let dir_path = PathBuf::from(root).join(&date_dir);
    tokio::fs::create_dir_all(&dir_path)
        .await
        .map_err(|e| format!("创建目录失败: {e}"))?;

    let file_path = dir_path.join(&new_filename);
    tokio::fs::write(&file_path, data)
        .await
        .map_err(|e| format!("写入文件失败: {e}"))?;

    let url = format!("/uploads/{}/{}", date_dir, new_filename);
    tracing::info!("文件已保存: {url}");
    Ok(url)
}

#[cfg(feature = "v18-upload")]
fn success_response(msg: &str, data: serde_json::Value) -> Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    (
        StatusCode::OK,
        axum::Json(json!({"code": 0, "msg": msg, "data": data})),
    )
        .into_response()
}

#[cfg(feature = "v18-upload")]
fn error_response(msg: &str) -> Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    (
        StatusCode::OK,
        axum::Json(json!({"code": 1, "msg": msg, "data": {}})),
    )
        .into_response()
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    // 兼容 data: URL 前缀
    let b64 = if let Some(comma_pos) = input.find(',') {
        &input[comma_pos + 1..]
    } else {
        input
    };

    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("base64 解码失败: {}", e))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::mock_app_state;
    use axum::routing::post;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_string(response: Response) -> String {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        String::from_utf8(bytes.to_vec()).expect("UTF-8")
    }

    fn json_request(uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    // ─────────────────────────────────────────────
    // base64_decode 纯逻辑
    // ─────────────────────────────────────────────

    /// base64_decode 应正确解码标准 base64
    #[test]
    fn base64_decode_standard_input() {
        let result = base64_decode("aGVsbG8=").unwrap();
        assert_eq!(result, b"hello");
    }

    /// base64_decode 应兼容 data: URL 前缀（逗号后取 base64 部分）
    #[test]
    fn base64_decode_strips_data_url_prefix() {
        let result = base64_decode("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!(result, b"hello");
    }

    /// base64_decode 对无效输入应返回 Err
    #[test]
    fn base64_decode_invalid_input_returns_err() {
        let result = base64_decode("!!!not-base64!!!");
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("base64 解码失败"),
            "应返回 base64 解码失败错误"
        );
    }

    /// base64_decode 空字符串应返回空 Vec
    #[test]
    fn base64_decode_empty_returns_empty_vec() {
        let result = base64_decode("").unwrap();
        assert!(result.is_empty());
    }

    // ─────────────────────────────────────────────
    // upload handler 参数解析路径
    // ─────────────────────────────────────────────

    /// upload 空文件数据 → render_error "文件数据不能为空"
    #[tokio::test]
    async fn upload_empty_file_data_returns_error() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/file/upload", post(upload))
            .with_state(state);
        let response = router
            .oneshot(json_request(
                "/api/v1/file/upload",
                r#"{"filename": "test.jpg", "file": ""}"#,
            ))
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(
            body.contains("文件数据不能为空"),
            "空文件数据应返回错误, 实际: {body}"
        );
    }

    /// upload 缺少 file 字段 → render_error "文件数据不能为空"
    #[tokio::test]
    async fn upload_missing_file_field_returns_error() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/file/upload", post(upload))
            .with_state(state);
        let response = router
            .oneshot(json_request(
                "/api/v1/file/upload",
                r#"{"filename": "test.jpg"}"#,
            ))
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(
            body.contains("文件数据不能为空"),
            "缺少 file 字段应返回错误, 实际: {body}"
        );
    }

    /// upload 无效 base64 → render_error "base64 解码失败"
    #[tokio::test]
    async fn upload_invalid_base64_returns_error() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/file/upload", post(upload))
            .with_state(state);
        let response = router
            .oneshot(json_request(
                "/api/v1/file/upload",
                r#"{"filename": "test.jpg", "file": "!!!not-base64!!!"}"#,
            ))
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(
            body.contains("base64 解码失败"),
            "无效 base64 应返回解码失败, 实际: {body}"
        );
    }

    /// upload 非法 JSON → render_error（post_data 解析失败）
    #[tokio::test]
    async fn upload_invalid_json_returns_error() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/file/upload", post(upload))
            .with_state(state);
        let response = router
            .oneshot(json_request("/api/v1/file/upload", "{not json"))
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(
            body.contains("\"code\":0"),
            "非法 JSON 应返回 code:0 错误, 实际: {body}"
        );
    }
}
