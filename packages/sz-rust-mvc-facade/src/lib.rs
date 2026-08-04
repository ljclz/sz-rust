//! sz-rust-mvc-facade — MVC 层（P3 解耦）
//!
//! 从 sz-rust-core 提取的控制器 / 视图 / 守卫抽象（对齐 PHP ThinkPHP MVC 体系）：
//!
//! - [`controller`]：BaseController / SzController trait / 请求参数解析与校验
//! - [`view`]：视图渲染（模板 / 布局 / 继承，`respond_html` 由 http-facade 提供）
//! - [`guard`]：权限守卫（依赖 middleware-facade 的 [`auth`](sz_rust_middleware_facade::auth)）
//!
//! 依赖方向：`mvc-facade → http-facade + infra-facade + middleware-facade`（无环）。
//! sz-rust-core 通过 `pub use sz_rust_mvc_facade::{controller, guard, view}` 保留向后兼容路径。

pub mod controller;
pub mod guard;
pub mod i18n_error;
pub mod view;
