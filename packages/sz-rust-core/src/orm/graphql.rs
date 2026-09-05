// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! GraphQL Facade — sz-orm-graphql 的统一访问入口
//!
//! ## 设计目标
//!
//! 业务包通过 `sz_rust_core::orm::graphql::*` 访问 GraphQL 功能，
//! 而非直接依赖 `sz-orm-graphql`，保持 facade 收口。
//!
//! ## 启用方式
//!
//! 在 `sz-rust-core` 的 Cargo.toml 中启用 `graphql` feature：
//! ```toml
//! sz-rust-core = { version = "0.3", features = ["graphql"] }
//! ```
//!
//! ## 核心类型
//!
//! | 类型 | 说明 |
//! |------|------|
//! | [`GraphQLSchema`] | Schema 容器（types + queries + mutations） |
//! | [`GraphQLType`] / [`GraphQLField`] | 类型与字段定义 |
//! | [`GraphQLSchemaGenerator`] | 从模型名自动生成 Schema |
//! | [`GraphQLServer`] | GraphQL 服务（in-memory + 可选真实 async-graphql） |
//! | [`DbResolver`] | 真实 DB resolver trait（`real` feature） |
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use sz_rust_core::orm::graphql::{GraphQLSchemaGenerator, GraphQLServer};
//!
//! // 从模型名自动生成 Schema
//! let schema = GraphQLSchemaGenerator::generate_schema(&["users", "orders"]);
//!
//! // 启动 GraphQL 服务
//! let server = GraphQLServer::new(8080).with_schema(schema);
//! let url = server.start().expect("start");
//! // POST http://localhost:8080/graphql
//! ```

#[cfg(feature = "graphql")]
pub use sz_orm_graphql::{
    GraphQLField, GraphQLSchema, GraphQLSchemaGenerator, GraphQLServer, GraphQLType,
};

#[cfg(feature = "graphql")]
pub mod resolver {
    //! DB Resolver 子模块
    pub use sz_orm_graphql::resolver::DbResolver;
}

#[cfg(not(feature = "graphql"))]
compile_error!(
    "GraphQL facade requires the `graphql` feature. \
     Enable it in sz-rust-core: sz-rust-core = { features = [\"graphql\"] }"
);

#[cfg(all(feature = "graphql", test))]
mod tests {
    use super::*;

    #[test]
    fn test_graphql_facade_exports_schema_types() {
        // 验证 facade 正确重导出核心类型
        let schema = GraphQLSchema::new();
        assert!(schema.types.is_empty());
        assert!(schema.queries.is_empty());
        assert!(schema.mutations.is_empty());
    }

    #[test]
    fn test_graphql_schema_generator() {
        let schema = GraphQLSchemaGenerator::generate_schema(&["users", "orders"]);
        // 2 个模型 → 2 个类型 + 4 个查询
        assert_eq!(schema.types.len(), 2);
        assert_eq!(schema.queries.len(), 4);
    }

    #[test]
    fn test_graphql_server_creation() {
        let schema = GraphQLSchemaGenerator::generate_schema(&["users"]);
        let _server = GraphQLServer::new(9999).with_schema(schema);
        // GraphQLServer 构造成功即通过；port 为私有字段，不直接断言
    }

    #[tokio::test]
    async fn test_graphql_server_start_and_query() {
        let schema = GraphQLSchemaGenerator::generate_schema(&["products"]);
        let server = GraphQLServer::new(9998).with_schema(schema);
        let url = server.start().expect("server should start");
        assert!(url.contains("9998"));

        // 等待后台任务绑定端口
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 执行 mock 查询
        let result = server.execute_query("{ getProduct(id: 1) { id name } }");
        assert!(result.is_ok(), "query should succeed: {:?}", result);
        let v = result.unwrap();
        assert_eq!(v["id"], "1");
    }

    #[test]
    fn test_graphql_execute_query_list() {
        let schema = GraphQLSchemaGenerator::generate_schema(&["items"]);
        let server = GraphQLServer::new(9997).with_schema(schema);
        let result = server.execute_query("{ listItems { id name } }");
        assert!(result.is_ok());
        let v = result.unwrap();
        assert!(v.is_array());
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_graphql_unknown_query_returns_error() {
        let schema = GraphQLSchemaGenerator::generate_schema(&["users"]);
        let server = GraphQLServer::new(9996).with_schema(schema);
        let result = server.execute_query("{ unknownQuery { id } }");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknownQuery"));
    }

    #[test]
    fn test_graphql_schema_sdl() {
        let schema = GraphQLSchemaGenerator::generate_schema(&["users"]);
        let sdl = schema.to_sdl();
        assert!(sdl.contains("type User {"));
        assert!(sdl.contains("getUser: User"));
        assert!(sdl.contains("listUsers: [User!]!"));
    }
}
