//! 控制器模块 — 对齐 PHP `addons/operate/controller/admin/`
//!
//! ## PHP 对齐
//!
//! | PHP 控制器 | Rust 控制器 | 方法数 | 说明 |
//! |-----------|------------|--------|------|
//! | `Company` | [`company::CompanyController`] | 6 | index/export/add/edit/del/detail |
//! | `Level` | [`level::LevelController`] | 4 | index/add/edit/delete |
//! | `Category` | [`category::CategoryController`] | 4 | index/add/edit/del |
//! | `ContractLog` | [`contract_log::ContractLogController`] | 3 | index/export/add |
//! | `Customer` | [`customer::CustomerController`] | 6 | index/add/edit/del/detail/export |
//! | `Contract` | [`contract::ContractController`] | 7 | index/export/add/edit/del/detail/renew |
//! | `Rentarea` | [`rentarea::RentareaController`] | 4 | index/add/edit/del |
//! | `CustomerPay` | [`customer_pay::CustomerPayController`] | 8 | index/detail/add/tradeNo/... |
//! | `Sync` | [`sync::SyncController`] | 3 | customerPay/customer/pay |
//!
//! ## 架构设计
//!
//! ### PHP → Rust 映射
//!
//! PHP 控制器通过 `$this->postData()` 获取请求参数，调用模型业务方法（`getList`/`detail`/`add`/`edit`/`setDelete`），
//! 通过 `$this->renderSuccess()`/`$this->renderError()` 返回 JSON 响应。
//!
//! Rust 控制器实现 `AddonsBaseController` trait（继承 `BaseController` + `SzController`），
//! 通过 `self.post_data(req).await` 获取参数，调用模型业务方法，通过 `self.render_success()`/`self.render_error()` 返回响应。
//!
//! ### Repository 注入
//!
//! PHP 通过 ThinkPHP 静态 facade 访问数据库；Rust 通过 `Repository` trait 注入。
//! 控制器方法接受 `&dyn Repository<Model, Key = Value>` 参数，
//! 由应用层（apps/oapc 等）在路由注册时注入具体实现（SQL Repository / InMemoryRepository）。
//!
//! ### 1:1 PHP 对齐
//!
//! 每个控制器方法严格对齐 PHP 同名方法：
//! - 方法名一致（PHP `index` → Rust `index`，PHP `del` → Rust `del`）
//! - 参数获取方式一致（`postData()` → `post_data(req).await`）
//! - 返回格式一致（`renderSuccess(msg, data)` → `render_success(msg, data)`）
//! - 业务逻辑委托给模型业务方法（与 PHP 一致）

pub mod category;
pub mod common;
pub mod company;
pub mod contract;
pub mod contract_log;
pub mod customer;
pub mod customer_pay;
pub mod level;
pub mod rentarea;
pub mod sync;

pub use category::CategoryController;
pub use company::CompanyController;
pub use contract::ContractController;
pub use contract_log::ContractLogController;
pub use customer::CustomerController;
pub use customer_pay::CustomerPayController;
pub use level::LevelController;
pub use rentarea::RentareaController;
pub use sync::SyncController;
