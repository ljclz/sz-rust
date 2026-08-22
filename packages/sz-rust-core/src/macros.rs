//! 宏模块 — `compact!` / `#[controller]` / `#[model]` 重导出
//!
//! H-6 修复：从 `sz-rust-macros`（proc-macro crate）重导出三个过程宏，
//! 使业务层可通过 `sz_rust_core::macros::compact!` 等路径访问，
//! 无需直接依赖 `sz-rust-macros` 包。
//!
//! ## 对齐 PHP
//!
//! | 宏 | 类型 | 对齐 PHP | 说明 |
//! |----|------|---------|------|
//! | `compact!` | 函数式宏 | `compact()` | 变量名 → 值映射，保序 |
//! | `#[controller]` | 属性宏 | 控制器声明 | 自动实现 `SzController` trait |
//! | `#[model]` | 属性宏 | 模型声明 | 自动实现 `Model` + `ModelExt` trait |
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::macros::{compact, controller, model};
//!
//! #[controller]
//! pub struct UserController;
//!
//! #[model(table = "users", pk = "user_id")]
//! pub struct User {
//!     pub user_id: i64,
//!     pub name: String,
//! }
//!
//! fn render() -> serde_json::Map<String, serde_json::Value> {
//!     let code = 0i32;
//!     let msg = "ok".to_string();
//!     compact!(code, msg)
//! }
//! ```

/// `compact!` 函数式宏 — 对齐 PHP `compact()`
///
/// 将变量名 → 值按声明顺序插入 `serde_json::Map<String, serde_json::Value>`。
/// 详见 `sz_rust_macros::compact` 文档。
pub use sz_rust_macros::compact;

/// `#[controller]` 属性宏 — 自动实现 `SzController` trait
///
/// 为控制器结构体自动实现 `sz_rust_core::controller::SzController` trait。
/// 详见 `sz_rust_macros::controller` 文档。
pub use sz_rust_macros::controller;

/// `#[model]` 属性宏 — 自动实现 `Model` + `ModelExt` trait
///
/// 为字段式结构体自动实现 `sz_orm_core::Model` + `sz_orm_core::ModelExt` trait。
/// 详见 `sz_rust_macros::model` 文档。
pub use sz_rust_macros::model;
