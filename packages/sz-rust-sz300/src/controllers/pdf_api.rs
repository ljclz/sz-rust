use axum::extract::Json as ExtractJson;
use axum::http::header;
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::{json, Value};

/// CSV 导出请求体
#[derive(Debug, Deserialize)]
pub struct CsvExportRequest {
    /// 导出文件名
    pub filename: String,
    /// CSV 表头列名
    pub headers: Vec<String>,
    /// CSV 数据行
    pub rows: Vec<Vec<String>>,
}

/// POST /api/pdf/export/csv — 导出 CSV（返回 JSON 元信息 + Base64 内容）
pub async fn export_csv(ExtractJson(req): ExtractJson<CsvExportRequest>) -> Json<Value> {
    let bytes =
        sz_rust_pdf::csv_export::export_csv_to_bytes(&req.headers, &req.rows).unwrap_or_default();

    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "filename": req.filename,
            "format": "csv",
            "size": bytes.len(),
            "content_base64": base64_encode(&bytes)
        }
    }))
}

/// POST /api/pdf/export/csv/download — 导出 CSV（直接下载二进制）
pub async fn export_csv_download(ExtractJson(req): ExtractJson<CsvExportRequest>) -> Response {
    let bytes =
        sz_rust_pdf::csv_export::export_csv_to_bytes(&req.headers, &req.rows).unwrap_or_default();

    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", req.filename).as_str(),
            ),
        ],
        bytes,
    )
        .into_response()
}

/// GET /api/pdf/health — PDF 服务健康检查
pub async fn health() -> Json<Value> {
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "plugin": "pdf",
            "status": "active",
            "modules": ["csv_export", "excel_export", "excel_import", "pdf_form"],
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((n >> 6) & 63) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(n & 63) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::{get, post};
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_export_csv() {
        let router = Router::new().route("/api/pdf/export/csv", post(export_csv));
        let req = json!({
            "filename": "test.csv",
            "headers": ["name", "age"],
            "rows": [["Alice", "30"], ["Bob", "25"]]
        });
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/pdf/export/csv")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&req).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], 1);
        assert!(json["data"]["size"].as_u64().unwrap() > 0);
        assert!(!json["data"]["content_base64"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_export_csv_download() {
        let router = Router::new().route("/api/pdf/export/csv/download", post(export_csv_download));
        let req = json!({
            "filename": "download.csv",
            "headers": ["col1", "col2"],
            "rows": [["a", "b"]]
        });
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/pdf/export/csv/download")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&req).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.contains("text/csv"));
    }

    #[tokio::test]
    async fn test_health() {
        let router = Router::new().route("/api/pdf/health", get(health));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/pdf/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["status"], "active");
        assert_eq!(json["data"]["plugin"], "pdf");
        let modules = json["data"]["modules"].as_array().unwrap();
        assert!(modules.len() >= 4);
    }
}
