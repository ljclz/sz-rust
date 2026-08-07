//! SZ-Rust K8s Operator — 自动化 sz-rust 应用部署
//!
//! ## 架构说明
//!
//! 本包提供 K8s Operator，自动化管理 sz-rust 应用部署。
//!
//! - **CRD**：[`crd::SzRustApp`] 自定义资源，描述 sz-rust 应用
//! - **Controller**：[`controller::Reconciler`] watch CRD 变化，reconcile 到期望状态
//!
//! ## 用法
//!
//! ### 安装 CRD
//!
//! ```sh
//! kubectl apply -f <(sz-rust-operator crd)
//! ```
//!
//! ### 创建 SzRustApp 资源
//!
//! ```yaml
//! apiVersion: sz-rust.dev/v1
//! kind: SzRustApp
//! metadata:
//!   name: my-app
//! spec:
//!   image: ghcr.io/ljclz/sz-rust:latest
//!   replicas: 3
//!   port: 8080
//! ```
//!
//! ### 运行 Operator
//!
//! ```rust,ignore
//! use sz_rust_operator::controller::run_controller;
//!
//! # tokio_test::block_on(async {
//! let client = kube::Client::try_default().await.unwrap();
//! run_controller(client).await.unwrap();
//! # });
//! ```

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod controller;
pub mod crd;

pub use controller::{
    run_controller, ControllerError, ReconcileResult, Reconciler, ReconcilerStats,
};
pub use crd::{
    generate_crd_yaml, DatabaseConfig, RedisConfig, ResourceRequirements, SzRustApp,
    SzRustAppCondition, SzRustAppSpec, SzRustAppStatus,
};
