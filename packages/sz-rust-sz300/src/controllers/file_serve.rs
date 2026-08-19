use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use std::path::PathBuf;
use tokio::fs;

/// 静态文件服务 — 根据路径返回上传目录中的文件，并按扩展名设置 Content-Type
///
/// # 安全
///
/// 采用三重路径遍历防护：
/// 1. 字符串层：拒绝 `..` 子串
/// 2. 路径层：`canonicalize` 解析符号链接与 `../`
/// 3. 边界层：canonicalize 结果必须以 uploads 目录为前缀
#[tracing::instrument(skip_all)]
pub async fn serve_file(
    State(_state): State<AppState>,
    path: axum::extract::Path<String>,
) -> Response {
    // 第一重：字符串层快速拒绝 `..`（含 URL 编码变体由 axum 自动解码后传入）
    if path.0.contains("..") {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }

    let base_dir = PathBuf::from("./uploads");
    let file_path = base_dir.join(&path.0);

    // 第二重 + 第三重：canonicalize 解析真实路径，并校验仍在 uploads 目录内
    // canonicalize 会解析符号链接、`../`、`.` 等路径变体，是路径遍历防护的权威手段
    let canonical_base = match base_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return (StatusCode::NOT_FOUND, "File not found").into_response(),
    };
    let canonical_file = match file_path.canonicalize() {
        Ok(p) => p,
        Err(_) => return (StatusCode::NOT_FOUND, "File not found").into_response(),
    };
    if !canonical_file.starts_with(&canonical_base) {
        // 路径逃逸出 uploads 目录 — 视为非法访问
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }

    match fs::read(&canonical_file).await {
        Ok(data) => {
            let ext = canonical_file
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let content_type = match ext.to_lowercase().as_str() {
                "jpg" | "jpeg" => "image/jpeg",
                "png" => "image/png",
                "gif" => "image/gif",
                "bmp" => "image/bmp",
                _ => "application/octet-stream",
            };
            ([(header::CONTENT_TYPE, content_type)], data).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "File not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::mock_app_state;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    #[tokio::test]
    async fn serve_file_rejects_path_traversal() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/uploads/{*path}", get(serve_file))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/uploads/../../etc/passwd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn serve_file_nonexistent_returns_not_found() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/uploads/{*path}", get(serve_file))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/uploads/nonexistent_file.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
