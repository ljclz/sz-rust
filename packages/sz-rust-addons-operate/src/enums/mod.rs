//! 枚举类型 — 对齐 PHP `app\common\enum\oa\`
//!
//! 迁移自：
//! - `e:\vue\test\鲜视达\server\app\common\enum\oa\ContractStatusEnum.php`
//! - `e:\vue\test\鲜视达\server\app\common\enum\oa\CustomerSyncTypeEnum.php`
//! - `e:\vue\test\鲜视达\server\app\common\enum\oa\CustomerPayTypeEnum.php`

pub mod contract_status;
pub mod customer_pay_type;
pub mod customer_sync_status;

pub use contract_status::ContractStatusEnum;
pub use customer_pay_type::CustomerPayType;
pub use customer_sync_status::CustomerSyncTypeEnum;
