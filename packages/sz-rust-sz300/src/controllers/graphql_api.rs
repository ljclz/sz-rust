//! GraphQL API 控制器 — sz300 业务 GraphQL 端点
//!
//! 提供 `/graphql`（POST 查询）和 `/graphiql`（GET IDE）端点。
//! Schema 定义：health 查询 + server_info 查询 + product 查询。

use async_graphql::{EmptyMutation, EmptySubscription, Object, Schema, SimpleObject};
use sz_rust_http_facade::graphql::{graphql_with_graphiql, GraphQLConfig};

/// 服务器健康状态
#[derive(SimpleObject)]
struct HealthGql {
    status: String,
    version: String,
    timestamp: String,
}

/// 服务器信息
#[derive(SimpleObject)]
struct ServerInfoGql {
    name: String,
    version: String,
    framework: String,
}

/// 商品 GraphQL 类型
#[derive(SimpleObject)]
struct ProductGql {
    good_id: i64,
    name: String,
    price: i64,
    status: i32,
}

/// GraphQL Query 根类型
struct QueryRoot;

#[Object]
impl QueryRoot {
    /// 健康检查
    async fn health(&self) -> HealthGql {
        HealthGql {
            status: "ok".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// 服务器信息
    async fn server_info(&self) -> ServerInfoGql {
        ServerInfoGql {
            name: "sz300-server".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            framework: "sz-rust".into(),
        }
    }

    /// 商品查询（示例：返回静态数据，生产环境可接入 ProductService）
    async fn product(&self, good_id: i64) -> Option<ProductGql> {
        if good_id <= 0 {
            return None;
        }
        Some(ProductGql {
            good_id,
            name: format!("商品#{}", good_id),
            price: 100i64.checked_mul(good_id)?,
            status: 1,
        })
    }
}

/// 构建 sz300 GraphQL 路由（POST /graphql + GET /graphiql）
pub fn graphql_router() -> axum::Router {
    let schema = Schema::build(QueryRoot, EmptyMutation, EmptySubscription).finish();
    graphql_with_graphiql(schema, GraphQLConfig::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_graphql_health_query() {
        let schema = Schema::build(QueryRoot, EmptyMutation, EmptySubscription).finish();
        let res = schema.execute("{ health { status version } }").await;
        let data = res.data.into_json().unwrap();
        assert_eq!(data["health"]["status"], "ok");
        assert!(data["health"]["version"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_graphql_server_info_query() {
        let schema = Schema::build(QueryRoot, EmptyMutation, EmptySubscription).finish();
        let res = schema.execute("{ serverInfo { name framework } }").await;
        let data = res.data.into_json().unwrap();
        assert_eq!(data["serverInfo"]["name"], "sz300-server");
        assert_eq!(data["serverInfo"]["framework"], "sz-rust");
    }

    #[tokio::test]
    async fn test_graphql_product_query() {
        let schema = Schema::build(QueryRoot, EmptyMutation, EmptySubscription).finish();
        let res = schema
            .execute("{ product(goodId: 1) { goodId name price } }")
            .await;
        let data = res.data.into_json().unwrap();
        assert_eq!(data["product"]["goodId"], 1);
        assert_eq!(data["product"]["name"], "商品#1");
        assert_eq!(data["product"]["price"], 100);
    }

    #[tokio::test]
    async fn test_graphql_product_not_found() {
        let schema = Schema::build(QueryRoot, EmptyMutation, EmptySubscription).finish();
        let res = schema.execute("{ product(goodId: 0) { goodId } }").await;
        let data = res.data.into_json().unwrap();
        assert!(data["product"].is_null());
    }

    #[tokio::test]
    async fn test_graphql_router_serves_graphiql() {
        let router = graphql_router();
        let res = router
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/graphiql")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_graphql_router_serves_query() {
        let router = graphql_router();
        let body = r#"{"query":"{ health { status } }"}"#;
        let res = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    // === 边界与极端测试 ===

    fn make_schema() -> Schema<QueryRoot, EmptyMutation, EmptySubscription> {
        Schema::build(QueryRoot, EmptyMutation, EmptySubscription).finish()
    }

    #[tokio::test]
    async fn test_product_good_id_max_returns_none() {
        let schema = make_schema();
        let res = schema
            .execute("{ product(goodId: 9223372036854775807) { goodId } }")
            .await;
        let data = res.data.into_json().unwrap();
        assert!(data["product"].is_null(), "i64::MAX 应溢出返回 null");
    }

    #[tokio::test]
    async fn test_product_good_id_min_returns_none() {
        let schema = make_schema();
        let res = schema
            .execute("{ product(goodId: -9223372036854775808) { goodId } }")
            .await;
        let data = res.data.into_json().unwrap();
        assert!(data["product"].is_null(), "i64::MIN <= 0 返回 null");
    }

    #[tokio::test]
    async fn test_product_good_id_negative_returns_none() {
        let schema = make_schema();
        let res = schema.execute("{ product(goodId: -1) { goodId } }").await;
        let data = res.data.into_json().unwrap();
        assert!(data["product"].is_null(), "负数 goodId 返回 null");
    }

    #[tokio::test]
    async fn test_product_good_id_critical_non_overflow() {
        let schema = make_schema();
        let critical = i64::MAX / 100;
        let query = format!("{{ product(goodId: {}) {{ goodId price }} }}", critical);
        let res = schema.execute(&query).await;
        let data = res.data.into_json().unwrap();
        assert_eq!(data["product"]["goodId"], critical);
        assert_eq!(data["product"]["price"], 100 * critical);
    }

    #[tokio::test]
    async fn test_health_stability_100_calls() {
        let schema = make_schema();
        for _ in 0..100 {
            let res = schema.execute("{ health { status version } }").await;
            let data = res.data.into_json().unwrap();
            assert_eq!(data["health"]["status"], "ok");
        }
    }

    #[tokio::test]
    async fn test_graphql_empty_query() {
        let router = graphql_router();
        let res = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"query":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_graphql_invalid_json() {
        let router = graphql_router();
        let res = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(res.status() != axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_graphql_long_query() {
        let router = graphql_router();
        let long_query = "x".repeat(1024 * 1024);
        let body = format!(r#"{{"query":"{long_query}"}}"#);
        let res = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_graphql_deep_nesting() {
        let router = graphql_router();
        let query = "{ health { status } }".repeat(100);
        let body = format!(r#"{{"query":"{query}"}}"#);
        let res = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_graphiql_content_type() {
        let router = graphql_router();
        let res = router
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/graphiql")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let ct = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("text/html"),
            "Content-Type 应含 text/html, got: {ct}"
        );
    }

    #[tokio::test]
    async fn test_graphql_error_response_format() {
        let schema = make_schema();
        let res = schema.execute("{ unknownField }").await;
        assert!(!res.errors.is_empty(), "未知字段应返回 errors");
    }
}
