// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 测试工具链：TestCase + HttpClient + ModelFactory + Fixture + Mock。
//!
//! Feature gate `test-utils` 控制，release 编译不含测试工具代码。

#[cfg(feature = "test-utils")]
mod error;

#[cfg(feature = "test-utils")]
pub use error::Error;

#[cfg(feature = "test-utils")]
pub mod factory;
#[cfg(feature = "test-utils")]
pub mod fixture;
#[cfg(feature = "test-utils")]
pub mod http_client;
#[cfg(feature = "test-utils")]
pub mod mock;
#[cfg(feature = "test-utils")]
pub mod test_case;

#[cfg(feature = "test-utils")]
pub use factory::{FactoryBuilder, ModelFactory};
#[cfg(feature = "test-utils")]
pub use fixture::Fixture;
#[cfg(feature = "test-utils")]
pub use http_client::{HttpClient, HttpResponse};
#[cfg(feature = "test-utils")]
pub use mock::MockInjector;
#[cfg(feature = "test-utils")]
pub use test_case::TestCase;
