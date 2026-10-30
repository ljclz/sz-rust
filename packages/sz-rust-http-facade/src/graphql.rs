// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! GraphQL 支持 — 基于 async-graphql + axum 提供 GraphQL HTTP 端点
//!
//! ## 架构说明
//!
//! 本模块提供 GraphQL 端点集成，将 `async-graphql` 的 Schema 挂载到 axum 路由。
//! 支持两种 Schema 形式：
//!
//! - **动态 Schema**（[`async_graphql::dynamic::Schema`]）：运行时构建，适合从 ORM 模型自动生成
//! - **类型化 Schema**（[`async_graphql::Schema`]）：编译时定义，适合手写 Query/Mutation
//!
//! ## 用法
//!
//! ### 动态 Schema
//!
//! ```rust,ignore
//! use sz_rust_http_facade::graphql::{graphql_router_dynamic, GraphQLConfig};
//! use async_graphql::dynamic::{Schema, Field, Object};
//!
//! let query = Object::new("Query")
//!     .field(Field::new("hello", "String", |_| {
//!         FieldFuture::new(async move { Some(async_graphql::Value::from("world")) })
//!     }));
//! let schema = Schema::build(query, None, None).finish().unwrap();
//!
//! let router = graphql_router_dynamic(schema, GraphQLConfig::default());
//! // router 现在在 POST /graphql 提供 GraphQL 端点
//! ```
//!
//! ### 类型化 Schema
//!
//! ```rust,ignore
//! use sz_rust_http_facade::graphql::{graphql_router, GraphQLConfig};
//! use async_graphql::{Object, Schema, SimpleObject};
//!
//! #[derive(SimpleObject)]
//! struct Query { hello: String }
//!
//! let schema = Schema::build(Query { hello: "world".into() }, None, None).finish();
//! let router = graphql_router(schema, GraphQLConfig::default());
//! ```
//!
//! ### GraphiQL IDE
//!
//! ```rust,ignore
//! use sz_rust_http_facade::graphql::graphiql_route;
//!
//! let router = graphiql_route("/graphql");
//! // router 现在在 GET /graphiql 提供 GraphiQL IDE
//! ```

use axum::{response::Html, routing::get, Router};

// ============================================================================
// GraphQLConfig
// ============================================================================

/// GraphQL 端点配置
///
/// # 参数
///
/// - `path`: GraphQL POST 端点路径（默认 `/graphql`）
#[derive(Debug, Clone)]
pub struct GraphQLConfig {
    /// GraphQL POST 端点路径
    pub path: String,
}

impl Default for GraphQLConfig {
    fn default() -> Self {
        Self {
            path: "/graphql".to_string(),
        }
    }
}

impl GraphQLConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置端点路径
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }
}

// ============================================================================
// 动态 Schema 路由
// ============================================================================

/// 构建 GraphQL axum 路由（动态 Schema）
///
/// 将 `async_graphql::dynamic::Schema` 挂载到 axum 路由，在 `config.path`
/// 提供 POST 端点处理 GraphQL 请求。
///
/// # 参数
///
/// - `schema`: 动态 GraphQL Schema
/// - `config`: 端点配置
///
/// # 返回
///
/// axum Router，包含 GraphQL POST 端点
pub fn graphql_router_dynamic(
    schema: async_graphql::dynamic::Schema,
    config: GraphQLConfig,
) -> Router {
    async fn graphql_handler(
        axum::extract::State(schema): axum::extract::State<async_graphql::dynamic::Schema>,
        request: async_graphql_axum::GraphQLRequest,
    ) -> async_graphql_axum::GraphQLResponse {
        schema.execute(request.into_inner()).await.into()
    }

    Router::new()
        .route(&config.path, axum::routing::post(graphql_handler))
        .with_state(schema)
}

// ============================================================================
// 类型化 Schema 路由
// ============================================================================

/// 构建 GraphQL axum 路由（类型化 Schema）
///
/// 将 `async_graphql::Schema<Query, Mutation, Subscription>` 挂载到 axum 路由，
/// 在 `config.path` 提供 POST 端点处理 GraphQL 请求。
///
/// # 参数
///
/// - `schema`: 类型化 GraphQL Schema
/// - `config`: 端点配置
///
/// # 返回
///
/// axum Router，包含 GraphQL POST 端点
pub fn graphql_router<Q, M, S>(
    schema: async_graphql::Schema<Q, M, S>,
    config: GraphQLConfig,
) -> Router
where
    Q: async_graphql::ObjectType + 'static,
    M: async_graphql::ObjectType + 'static,
    S: async_graphql::SubscriptionType + 'static,
{
    async fn graphql_handler<Q, M, S>(
        axum::extract::State(schema): axum::extract::State<async_graphql::Schema<Q, M, S>>,
        request: async_graphql_axum::GraphQLRequest,
    ) -> async_graphql_axum::GraphQLResponse
    where
        Q: async_graphql::ObjectType + 'static,
        M: async_graphql::ObjectType + 'static,
        S: async_graphql::SubscriptionType + 'static,
    {
        schema.execute(request.into_inner()).await.into()
    }

    Router::new()
        .route(
            &config.path,
            axum::routing::post(graphql_handler::<Q, M, S>),
        )
        .with_state(schema)
}

// ============================================================================
// GraphiQL IDE
// ============================================================================

/// GraphiQL IDE HTML 模板
///
/// 参数为 GraphQL 端点 URL
fn graphiql_html(endpoint: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8"/>
  <title>GraphiQL — sz-rust</title>
  <style>
    body {{ height: 100%; margin: 0; overflow: hidden; }}
    #graphiql {{ height: 100vh; }}
  </style>
  <script src="https://cdn.jsdelivr.net/npm/react@18.2.0/umd/react.production.min.js"></script>
  <script src="https://cdn.jsdelivr.net/npm/react-dom@18.2.0/umd/react-dom.production.min.js"></script>
  <script src="https://cdn.jsdelivr.net/npm/graphiql@3.0.10/graphiql.min.js"></script>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/graphiql@3.0.10/graphiql.min.css"/>
</head>
<body>
  <div id="graphiql"></div>
  <script>
    ReactDOM.render(
      React.createElement(GraphiQL, {{ fetcher: GraphiQL.createFetcher({{ url: "{endpoint}" }}) }}),
      document.getElementById('graphiql'),
    );
  </script>
</body>
</html>"#,
        endpoint = endpoint
    )
}

/// 构建 GraphiQL IDE 路由
///
/// 在 `GET /graphiql` 提供 GraphiQL IDE，指向指定的 GraphQL 端点。
///
/// # 参数
///
/// - `graphql_endpoint`: GraphQL POST 端点 URL（如 `/graphql`）
///
/// # 返回
///
/// axum Router，包含 GraphiQL GET 端点
pub fn graphiql_route(graphql_endpoint: &str) -> Router {
    let html = graphiql_html(graphql_endpoint);
    Router::new().route("/graphiql", get(move || async move { Html(html.clone()) }))
}

/// 构建 GraphQL + GraphiQL 完整路由（动态 Schema）
///
/// 同时提供：
/// - `POST {config.path}` — GraphQL 端点
/// - `GET /graphiql` — GraphiQL IDE
///
/// # 参数
///
/// - `schema`: 动态 GraphQL Schema
/// - `config`: 端点配置
pub fn graphql_with_graphiql_dynamic(
    schema: async_graphql::dynamic::Schema,
    config: GraphQLConfig,
) -> Router {
    let endpoint = config.path.clone();
    graphql_router_dynamic(schema, config).merge(graphiql_route(&endpoint))
}

/// 构建 GraphQL + GraphiQL 完整路由（类型化 Schema）
///
/// 同时提供：
/// - `POST {config.path}` — GraphQL 端点
/// - `GET /graphiql` — GraphiQL IDE
///
/// # 参数
///
/// - `schema`: 类型化 GraphQL Schema
/// - `config`: 端点配置
pub fn graphql_with_graphiql<Q, M, S>(
    schema: async_graphql::Schema<Q, M, S>,
    config: GraphQLConfig,
) -> Router
where
    Q: async_graphql::ObjectType + 'static,
    M: async_graphql::ObjectType + 'static,
    S: async_graphql::SubscriptionType + 'static,
{
    let endpoint = config.path.clone();
    graphql_router(schema, config).merge(graphiql_route(&endpoint))
}

// ============================================================================
// GraphQL 请求/响应类型便捷重导出
// ============================================================================

/// GraphQL HTTP 请求（从 async-graphql-axum 重导出）
pub type GraphQLRequest = async_graphql_axum::GraphQLRequest;

/// GraphQL HTTP 响应（从 async-graphql-axum 重导出）
pub type GraphQLResponse = async_graphql_axum::GraphQLResponse;

// ============================================================================
// GraphQLExecutor — 深度/复杂度限制（spec §5.20，v1.7.0 新增）
// ============================================================================

/// GraphQL 执行器配置（spec §6.20）
#[derive(Debug, Clone)]
pub struct GraphQLExecutorConfig {
    /// 查询深度限制（默认 ≤ 10，spec §6.20 规则 1）
    pub max_depth: usize,
    /// 复杂度限制（spec §6.20 规则 2）
    pub max_complexity: usize,
    /// DataLoader 批量大小（默认 ≤ 100，spec §6.20 规则 3）
    pub dataloader_batch_size: usize,
}

impl Default for GraphQLExecutorConfig {
    fn default() -> Self {
        Self {
            max_depth: 10,
            max_complexity: 1000,
            dataloader_batch_size: 100,
        }
    }
}

impl GraphQLExecutorConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置最大深度
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// 设置最大复杂度
    pub fn with_max_complexity(mut self, max_complexity: usize) -> Self {
        self.max_complexity = max_complexity;
        self
    }

    /// 设置 DataLoader 批量大小
    pub fn with_dataloader_batch_size(mut self, batch_size: usize) -> Self {
        self.dataloader_batch_size = batch_size;
        self
    }

    /// 校验查询深度（spec §6.20 规则 1）
    pub fn validate_depth(&self, depth: usize) -> Result<(), GraphQLLimitError> {
        if depth > self.max_depth {
            Err(GraphQLLimitError::DepthExceeded {
                actual: depth,
                max: self.max_depth,
            })
        } else {
            Ok(())
        }
    }

    /// 校验查询复杂度（spec §6.20 规则 2）
    pub fn validate_complexity(&self, complexity: usize) -> Result<(), GraphQLLimitError> {
        if complexity > self.max_complexity {
            Err(GraphQLLimitError::ComplexityExceeded {
                actual: complexity,
                max: self.max_complexity,
            })
        } else {
            Ok(())
        }
    }
}

/// GraphQL 限制错误
#[derive(Debug, Clone, thiserror::Error, serde::Serialize)]
pub enum GraphQLLimitError {
    /// 查询深度超限（spec §5.20 规则 4）
    #[error("查询深度超限: {actual} > {max}")]
    DepthExceeded {
        /// 实际深度
        actual: usize,
        /// 最大允许深度
        max: usize,
    },
    /// 查询复杂度超限（spec §5.20 规则 4）
    #[error("查询复杂度超限: {actual} > {max}")]
    ComplexityExceeded {
        /// 实际复杂度
        actual: usize,
        /// 最大允许复杂度
        max: usize,
    },
}

/// 估算 GraphQL 查询深度（简单解析，spec §5.20 规则 1）
///
/// 通过大括号嵌套层数估算查询深度。
pub fn estimate_query_depth(query: &str) -> usize {
    let mut max_depth: usize = 0;
    let mut current_depth: usize = 0;
    for ch in query.chars() {
        if ch == '{' {
            current_depth += 1;
            if current_depth > max_depth {
                max_depth = current_depth;
            }
        } else if ch == '}' {
            current_depth = current_depth.saturating_sub(1);
        }
    }
    max_depth.saturating_sub(1)
}

/// 估算 GraphQL 查询复杂度（字段数 × 嵌套层数，spec §5.20 规则 4）
pub fn estimate_query_complexity(query: &str) -> usize {
    let depth = estimate_query_depth(query);
    let field_count = query
        .chars()
        .filter(|c| c.is_alphabetic() || *c == '_')
        .count();
    field_count * (depth + 1)
}

/// GraphQL 执行器（spec §5.20）
///
/// 封装 async-graphql Schema + 深度/复杂度限制。
pub struct GraphQLExecutor {
    config: GraphQLExecutorConfig,
}

impl GraphQLExecutor {
    /// 创建执行器
    pub fn new(config: GraphQLExecutorConfig) -> Self {
        Self { config }
    }

    /// 获取配置引用
    pub fn config(&self) -> &GraphQLExecutorConfig {
        &self.config
    }

    /// 校验查询（深度 + 复杂度，spec §5.20 规则 4）
    ///
    /// # 后置条件
    /// - 超深度/超复杂度 → 返回错误
    /// - 通过 → 返回 Ok
    pub fn validate_query(&self, query: &str) -> Result<(), GraphQLLimitError> {
        let depth = estimate_query_depth(query);
        self.config.validate_depth(depth)?;
        let complexity = estimate_query_complexity(query);
        self.config.validate_complexity(complexity)?;
        Ok(())
    }
}

impl Default for GraphQLExecutor {
    fn default() -> Self {
        Self::new(GraphQLExecutorConfig::default())
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    // ------------------------------------------------------------------------
    // GraphQLConfig 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_graphql_config_default() {
        let config = GraphQLConfig::default();
        assert_eq!(config.path, "/graphql");
    }

    #[test]
    fn test_graphql_config_builder() {
        let config = GraphQLConfig::new().with_path("/api/graphql");
        assert_eq!(config.path, "/api/graphql");
    }

    // ------------------------------------------------------------------------
    // 动态 Schema 路由测试
    // ------------------------------------------------------------------------

    /// 构建测试用动态 Schema（hello: String）
    fn test_dynamic_schema() -> async_graphql::dynamic::Schema {
        use async_graphql::dynamic::{Field, FieldFuture, Object, Schema, TypeRef};
        use async_graphql::Value;

        let query =
            Object::new("Query").field(Field::new("hello", TypeRef::named("String"), |_| {
                FieldFuture::from_value(Some(Value::from("world")))
            }));
        Schema::build("Query", None, None)
            .register(query)
            .finish()
            .unwrap()
    }

    #[tokio::test]
    async fn test_graphql_router_dynamic_hello_query() {
        use axum::body::Body;
        use http_body_util::BodyExt;

        let schema = test_dynamic_schema();
        let app = graphql_router_dynamic(schema, GraphQLConfig::default());

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":"{ hello }"}"#.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["hello"], "world");
    }

    #[tokio::test]
    async fn test_graphql_router_dynamic_custom_path() {
        use axum::body::Body;

        let schema = test_dynamic_schema();
        let app = graphql_router_dynamic(schema, GraphQLConfig::new().with_path("/api/gql"));

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/gql")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":"{ hello }"}"#.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_graphql_router_dynamic_invalid_query() {
        use axum::body::Body;
        use http_body_util::BodyExt;

        let schema = test_dynamic_schema();
        let app = graphql_router_dynamic(schema, GraphQLConfig::default());

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"query":"{ nonexistentField }"}"#.to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["errors"].is_array());
        assert!(!json["errors"].as_array().unwrap().is_empty());
    }

    // ------------------------------------------------------------------------
    // GraphiQL 测试
    // ----------------------------------------------------------------===========

    #[tokio::test]
    async fn test_graphiql_route_returns_html() {
        use axum::body::Body;
        use http_body_util::BodyExt;

        let app = graphiql_route("/graphql");

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/graphiql")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("GraphiQL"));
        assert!(html.contains("/graphql"));
    }

    // ------------------------------------------------------------------------
    // 完整路由测试（GraphQL + GraphiQL）
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_graphql_with_graphiql_dynamic() {
        use axum::body::Body;
        use http_body_util::BodyExt;

        let schema = test_dynamic_schema();
        let app = graphql_with_graphiql_dynamic(schema, GraphQLConfig::default());

        // GraphQL POST 端点
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":"{ hello }"}"#.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        // GraphiQL GET 端点
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/graphiql")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("GraphiQL"));
    }

    // ------------------------------------------------------------------------
    // GraphiQL HTML 模板测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_graphiql_html_contains_endpoint() {
        let html = graphiql_html("/my-graphql");
        assert!(html.contains("/my-graphql"));
        assert!(html.contains("GraphiQL"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn test_graphiql_html_contains_cdn_links() {
        let html = graphiql_html("/graphql");
        assert!(html.contains("react@18"));
        assert!(html.contains("graphiql@3"));
    }

    // ------------------------------------------------------------------------
    // GraphQLExecutor 测试（v1.7.0 新增）
    // ------------------------------------------------------------------------

    #[test]
    fn test_executor_config_default() {
        let config = GraphQLExecutorConfig::default();
        assert_eq!(config.max_depth, 10);
        assert_eq!(config.max_complexity, 1000);
        assert_eq!(config.dataloader_batch_size, 100);
    }

    #[test]
    fn test_executor_config_builder() {
        let config = GraphQLExecutorConfig::new()
            .with_max_depth(5)
            .with_max_complexity(500)
            .with_dataloader_batch_size(50);
        assert_eq!(config.max_depth, 5);
        assert_eq!(config.max_complexity, 500);
        assert_eq!(config.dataloader_batch_size, 50);
    }

    #[test]
    fn test_estimate_query_depth() {
        assert_eq!(estimate_query_depth("{ hello }"), 0);
        assert_eq!(estimate_query_depth("{ user { name } }"), 1);
        assert_eq!(estimate_query_depth("{ user { posts { title } } }"), 2);
        assert_eq!(estimate_query_depth("{ a { b { c { d } } } }"), 3);
    }

    #[test]
    fn test_validate_depth_ok() {
        let config = GraphQLExecutorConfig::new().with_max_depth(10);
        assert!(config.validate_depth(5).is_ok());
        assert!(config.validate_depth(10).is_ok());
    }

    #[test]
    fn test_validate_depth_exceeded() {
        let config = GraphQLExecutorConfig::new().with_max_depth(3);
        let err = config.validate_depth(5).unwrap_err();
        assert!(matches!(
            err,
            GraphQLLimitError::DepthExceeded { actual: 5, max: 3 }
        ));
    }

    #[test]
    fn test_validate_complexity_ok() {
        let config = GraphQLExecutorConfig::new().with_max_complexity(1000);
        assert!(config.validate_complexity(500).is_ok());
    }

    #[test]
    fn test_validate_complexity_exceeded() {
        let config = GraphQLExecutorConfig::new().with_max_complexity(10);
        let err = config.validate_complexity(50).unwrap_err();
        assert!(matches!(
            err,
            GraphQLLimitError::ComplexityExceeded {
                actual: 50,
                max: 10
            }
        ));
    }

    #[test]
    fn test_executor_validate_query_ok() {
        let executor = GraphQLExecutor::default();
        assert!(executor.validate_query("{ hello }").is_ok());
        assert!(executor.validate_query("{ user { name } }").is_ok());
    }

    #[test]
    fn test_executor_validate_query_depth_exceeded() {
        let config = GraphQLExecutorConfig::new()
            .with_max_depth(1)
            .with_max_complexity(10000);
        let executor = GraphQLExecutor::new(config);
        let deep_query = "{ a { b { c } } }";
        assert!(executor.validate_query(deep_query).is_err());
    }
}
