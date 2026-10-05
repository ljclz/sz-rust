// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 GraphQL 端点模块
//!
//! 提供 Query（merchant/product/order 查询）和 Mutation（createProduct/updateProduct/createOrder/updateOrderStatus）。
//! 深度限制 ≤10，复杂度 ≤1000。

pub mod dataloaders;
pub mod schema;

pub use schema::{build_schema, GraphQLSchema};
