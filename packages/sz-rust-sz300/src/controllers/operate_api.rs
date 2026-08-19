use axum::response::Json;
use serde_json::{json, Value};

/// GET /api/operate/models — 列出 operate 插件所有模型及字段元数据
pub async fn list_models() -> Json<Value> {
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "plugin": "operate",
            "models": [
                {
                    "name": "Customer",
                    "table": "customer",
                    "fields": ["id", "name", "phone", "rentarea_ids", "level_id", "store_id", "company_id", "create_time", "update_time"]
                },
                {
                    "name": "Contract",
                    "table": "contract",
                    "fields": ["id", "contract_no", "customer_id", "product_id", "amount", "pay_detail", "start_date", "end_date", "status", "create_time"]
                },
                {
                    "name": "Category",
                    "table": "category",
                    "fields": ["id", "name", "pid", "sort", "status"]
                },
                {
                    "name": "Rentarea",
                    "table": "rentarea",
                    "fields": ["id", "name", "code", "pid"]
                },
                {
                    "name": "Dept",
                    "table": "dept",
                    "fields": ["id", "name", "pid", "sort"]
                },
                {
                    "name": "Company",
                    "table": "company",
                    "fields": ["id", "name", "code", "legal_person", "contact_phone"]
                },
                {
                    "name": "Store",
                    "table": "store",
                    "fields": ["id", "name", "company_id", "address", "phone"]
                },
                {
                    "name": "Level",
                    "table": "level",
                    "fields": ["id", "name", "sort", "discount"]
                }
            ]
        }
    }))
}

/// GET /api/operate/health — operate 插件健康检查（实例化模型验证链接）
pub async fn health() -> Json<Value> {
    let _customer = sz_rust_addons_operate::Customer::new();
    let _contract = sz_rust_addons_operate::Contract::new();
    let _category = sz_rust_addons_operate::Category::new();
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "plugin": "operate",
            "status": "active",
            "models_loaded": 8,
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_list_models() {
        let router = Router::new().route("/api/operate/models", get(list_models));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/operate/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let models = json["data"]["models"].as_array().unwrap();
        assert!(models.len() >= 5);
        let names: Vec<&str> = models.iter().map(|m| m["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"Customer"));
        assert!(names.contains(&"Contract"));
    }

    #[tokio::test]
    async fn test_health() {
        let router = Router::new().route("/api/operate/health", get(health));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/operate/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["status"], "active");
        assert_eq!(json["data"]["models_loaded"], 8);
    }
}
