//! GraphQL Facade — sz-orm-graphql 的统一访问入口
//!
//! ## 设计目标
//!
//! 业务包通过 `sz_rust_orm_facade::graphql::*` 访问 GraphQL 功能，
//! 而非直接依赖 `sz-orm-graphql`，保持 facade 收口。
//!
//! ## 启用方式
//!
//! 在 `sz-rust-orm-facade` 的 Cargo.toml 中启用 `graphql` feature：
//! ```toml
//! sz-rust-orm-facade = { version = "0.3", features = ["graphql"] }
//! ```

#[cfg(feature = "graphql")]
pub use sz_orm_graphql::*;
