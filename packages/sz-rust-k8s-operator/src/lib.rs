//! K8s Operator — 管理 sz300 应用部署
//!
//! 定义 `Sz300App` CRD，通过 reconcile loop 自动管理 Deployment + Service。
//!
//! ## CRD 定义
//!
//! ```yaml
//! apiVersion: sz-rust.dev/v1
//! kind: Sz300App
//! metadata:
//!   name: my-app
//! spec:
//!   image: sz300-server:latest
//!   replicas: 3
//!   port: 8080
//! ```

pub mod crd;
pub mod reconcile;

pub use crd::Sz300App;
pub use reconcile::{reconcile, ReconcileError};
