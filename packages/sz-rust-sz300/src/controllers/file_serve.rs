use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use std::path::PathBuf;
use tokio::fs;

#[tracing::instrument(skip_all)]
pub async fn serve_file(
    State(_state): State<AppState>,
    path: axum::extract::Path<String>,
) -> Response {
    // Reject paths containing .. to prevent directory traversal
    if path.0.contains("..") {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }

    let file_path = PathBuf::from("./uploads").join(path.0);

    match fs::read(&file_path).await {
        Ok(data) => {
            let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
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
