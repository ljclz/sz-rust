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

                // 保存文件（使用 sz_rust_core::upload 引擎时通过 StorageConfig 配置驱动）
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

    #[test]
    fn base64_decode_plain_string() {
        let result = base64_decode("aGVsbG8=").unwrap();
        assert_eq!(result, b"hello");
    }

    #[test]
    fn base64_decode_data_url_prefix() {
        let result = base64_decode("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!(result, b"hello");
    }

    #[test]
    fn base64_decode_empty_string() {
        let result = base64_decode("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn base64_decode_binary_data() {
        let original = vec![0xFF, 0x00, 0xAB, 0xCD];
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&original);
        let result = base64_decode(&encoded).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn base64_decode_invalid_returns_err() {
        let result = base64_decode("!!!invalid!!!");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("base64 解码失败"));
    }

    #[test]
    fn base64_decode_comma_only() {
        let result = base64_decode(",");
        assert!(result.unwrap().is_empty());
    }

    /// 覆盖 upload 空文件数据路径
    #[tokio::test]
    async fn upload_with_empty_file_returns_error() {
        use crate::state::mock_app_state;
        use axum::routing::post;
        use axum::Router;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/file/upload", post(upload))
            .with_state(state);
        let body = serde_json::json!({"filename": "test.txt", "file": ""}).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/file/upload")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let body = String::from_utf8(body_bytes.to_vec()).expect("UTF-8");
        assert!(body.contains("\"code\":0"), "空文件应返回错误: {}", body);
    }

    /// 覆盖 upload 无效 base64 路径
    #[tokio::test]
    async fn upload_with_invalid_base64_returns_error() {
        use crate::state::mock_app_state;
        use axum::routing::post;
        use axum::Router;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/file/upload", post(upload))
            .with_state(state);
        let body = serde_json::json!({"filename": "test.txt", "file": "!!!invalid!!!"}).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/file/upload")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let body = String::from_utf8(body_bytes.to_vec()).expect("UTF-8");
        assert!(
            body.contains("\"code\":0"),
            "无效 base64 应返回错误: {}",
            body
        );
    }
}
