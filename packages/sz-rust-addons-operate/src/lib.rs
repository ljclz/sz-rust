//! sz-rust-addons-operate — 对齐 PHP `addons/operate` 插件
//!
//! 迁移自 `e:\vue\test\鲜视达\server\addons\operate\`。
//!
//! ## 当前迁移范围
//!
//! | 模型 | PHP 文件 | 元数据 | 访问器 | 修改器 | append | 关联 |
//! |------|---------|--------|--------|--------|--------|------|
//! | Customer | `addons/operate/model/Customer.php` | ✅ | ✅ 3 个 | ✅ 1 个 | ✅ 2 个 | ⬜ 待实现 |
//! | Contract | `addons/operate/model/Contract.php` | ✅ | ✅ 17 个 | ✅ 2 个 | ✅ 6 个 | ⬜ 待实现 |
//! | Rentarea | `addons/operate/model/Rentarea.php` | ✅ | ✅ 2 个 | ❌ | ✅ 2 个 | ⬜ 待实现 |
//! | Dept | `app/common/model/szoa/industry/IndustryDept.php` | ✅ | ❌ | ❌ | ❌ | ⬜ 待实现 |
//! | Category | `addons/operate/model/Category.php` | ✅ | ❌ | ❌ | ❌ | ⬜ 待实现 |
//!
//! ## 未实现特性（标 NOTE 等待后续实现）
//!
//! - **关联关系**（belongsTo/hasMany + bind/with/where/withoutField）→ NOTE(关联模块)
//! - **全局范围 `scopeApp_id`**（`$globalScope = ['app_id']`）→ NOTE(TenantModel 模块)
//! - **静态缓存**（`Cache::get/set`）→ NOTE(Cache 模块)
//! - **静态反查访问器**（`Customer::getRentareaTextAttr` 调 `Rentarea::where`、
//!   `Contract::getLogsAttr` 调 `ContractLog::getLogs`）→ NOTE(Repository 层)
//! - **业务方法**（detail/getList/add/edit/setDelete 等）→ NOTE(控制器层)
//!
//! ## 序列化策略
//!
//! PHP 端混用三种序列化：
//! 1. `iserializer($value)` / `unserialize($value)`（PHP serialize 格式）→ Rust 端统一用 JSON
//! 2. `json_encode($value)` / `json_decode($value, true)` → Rust 端用 `serde_json`
//! 3. 逗号分隔字符串（`rentarea_ids`）→ Rust 端用 `Vec<i64>` + 修改器
//!
//! **不兼容说明**：PHP `pay_detail` 字段使用 PHP serialize 格式，Rust 端改用 JSON。
//! 若需与 PHP 数据库现存量兼容，需在数据迁移层做格式转换。

#![forbid(unsafe_code)]
#![allow(missing_docs)]

pub mod controller;
pub mod enums;
pub mod model;
pub mod service;

pub use model::{
    category::Category, company::Company, contract::Contract, contract_log::ContractLog,
    crmlog::Crmlog, customer::Customer, customer_pay::CustomerPay, dept::Dept, level::Level,
    rentarea::Rentarea, store::Store,
};

// ============================================================================
// Addon 接线：OperateState + register_routes
// ============================================================================

use axum::response::Json;
use serde_json::json;
use sz_rust_core::router::RouterBuilder;

/// operate addon 状态
#[derive(Clone)]
pub struct OperateState {
    pub models: Vec<&'static str>,
    pub version: &'static str,
}

impl Default for OperateState {
    fn default() -> Self {
        Self {
            models: vec![
                "Customer", "Contract", "Category", "Rentarea", "Dept", "Company", "Store", "Level",
            ],
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// 注册 operate addon 路由到 sz300 RouterBuilder
pub fn register_routes<S>(builder: RouterBuilder<S>, state: OperateState) -> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    let builder = builder.get("/api/operate/models", {
        move || async move {
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "plugin": "operate",
                    "models": [
                        {"name": "Customer", "table": "customer", "fields": ["id", "name", "phone", "rentarea_ids", "level_id", "store_id", "company_id", "create_time", "update_time"]},
                        {"name": "Contract", "table": "contract", "fields": ["id", "contract_no", "customer_id", "product_id", "amount", "pay_detail", "start_date", "end_date", "status", "create_time"]},
                        {"name": "Category", "table": "category", "fields": ["id", "name", "pid", "sort", "status"]},
                        {"name": "Rentarea", "table": "rentarea", "fields": ["id", "name", "code", "pid"]},
                        {"name": "Dept", "table": "dept", "fields": ["id", "name", "pid", "sort"]},
                        {"name": "Company", "table": "company", "fields": ["id", "name", "code", "legal_person", "contact_phone"]},
                        {"name": "Store", "table": "store", "fields": ["id", "name", "company_id", "address", "phone"]},
                        {"name": "Level", "table": "level", "fields": ["id", "name", "sort", "discount"]}
                    ]
                }
            }))
        }
    });

    builder.get("/api/operate/health", {
        let s = state;
        move || async move {
            let _customer = Customer::new();
            let _contract = Contract::new();
            let _category = Category::new();
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "plugin": "operate",
                    "status": "active",
                    "models_loaded": s.models.len(),
                    "version": s.version
                }
            }))
        }
    })
}

pub mod capability;
pub use capability::OperatePlugin;
