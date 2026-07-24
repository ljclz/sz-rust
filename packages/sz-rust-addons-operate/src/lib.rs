//! sz-rust-addons-operate — 对齐 PHP `addons/operate` 插件
//!
//! 迁移自 `e:\vue\test\鲜视达\server\addons\operate\`。
//!
//! ## 当前迁移范围（Phase 2.10）
//!
//! | 模型 | PHP 文件 | 元数据 | 访问器 | 修改器 | append | 关联 |
//! |------|---------|--------|--------|--------|--------|------|
//! | Customer | `addons/operate/model/Customer.php` | ✅ | ✅ 3 个 | ✅ 1 个 | ✅ 2 个 | ⬜ Phase 4 |
//! | Contract | `addons/operate/model/Contract.php` | ✅ | ✅ 17 个 | ✅ 2 个 | ✅ 6 个 | ⬜ Phase 4 |
//! | Rentarea | `addons/operate/model/Rentarea.php` | ✅ | ✅ 2 个 | ❌ | ✅ 2 个 | ⬜ Phase 4 |
//! | Dept | `app/common/model/szoa/industry/IndustryDept.php` | ✅ | ❌ | ❌ | ❌ | ⬜ Phase 4 |
//! | Category | `addons/operate/model/Category.php` | ✅ | ❌ | ❌ | ❌ | ⬜ Phase 4 |
//!
//! ## 未实现特性（标 NOTE 等待对应 Phase）
//!
//! - **关联关系**（belongsTo/hasMany + bind/with/where/withoutField）→ NOTE(Phase 4)
//! - **全局范围 `scopeApp_id`**（`$globalScope = ['app_id']`）→ NOTE(Phase 3 TenantModel)
//! - **静态缓存**（`Cache::get/set`）→ NOTE(Phase 6)
//! - **静态反查访问器**（`Customer::getRentareaTextAttr` 调 `Rentarea::where`、
//!   `Contract::getLogsAttr` 调 `ContractLog::getLogs`）→ NOTE(Phase 4 Repository)
//! - **业务方法**（detail/getList/add/edit/setDelete 等）→ NOTE(Phase 5+ 控制器层)
//!
//! ## 序列化策略
//!
//! PHP 端混用三种序列化：
//! 1. `iserializer($value)` / `unserialize($value)`（PHP serialize 格式）→ Rust 端统一用 JSON
//! 2. `json_encode($value)` / `json_decode($value, true)` → Rust 端用 `serde_json`
//! 3. 逗号分隔字符串（`rentarea_ids`）→ Rust 端用 `Vec<i64>` + 修改器
//!
//! **不兼容说明**：PHP `pay_detail` 字段使用 PHP serialize 格式，Rust 端改用 JSON。
//! 若需与 PHP 数据库现存量兼容，需在 Phase 6 数据迁移层做格式转换。

#![forbid(unsafe_code)]

pub mod controller;
pub mod enums;
pub mod model;
pub mod service;

pub use model::{
    category::Category, company::Company, contract::Contract, contract_log::ContractLog,
    crmlog::Crmlog, customer::Customer, customer_pay::CustomerPay, dept::Dept, level::Level,
    rentarea::Rentarea, store::Store,
};
