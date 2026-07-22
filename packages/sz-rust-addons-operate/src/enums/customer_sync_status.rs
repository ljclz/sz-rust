//! CustomerSyncTypeEnum — 对齐 PHP `app\common\enum\oa\CustomerSyncTypeEnum`
//!
//! PHP 端继承 `MyCLabs\Enum\Enum`，提供 5 组静态方法返回数组。
//! Rust 端用强类型 enum + 关联函数对齐。
//!
//! ## PHP 行为对齐
//!
//! PHP `xxxName($value)` 在 value 不存在时返回 `'未知'`；
//! 调用方（如 `getPayStatusTextAttr`）用 `!empty($data['xxx']) ? ... : ''` 包裹，
//! 即字段为 0/空/不存在时返回空字符串 `''`。
//!
//! ## paySource 特殊性
//!
//! PHP `paySourceData` 的 value 是字符串（`'icbc'`/`'ccb'`/`'fuiou'`/`'cash'`），
//! 而非数字。`paySourceName` 接收字符串参数，按字符串 key 查找。

/// 同步状态 — 对齐 PHP `CustomerSyncTypeEnum::syncStatusData`
///
/// | value | name | color |
/// |-------|------|-------|
/// | 10 | 待同步 | red |
/// | 20 | 无需同步 | blue |
/// | 30 | 已同步 | orange |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Pending = 10,
    NotNeeded = 20,
    Synced = 30,
}

impl SyncStatus {
    /// 从数值构造，无效值返回 `None`（对齐 PHP `isset($data[$value])`）
    pub fn from_value(value: i64) -> Option<Self> {
        match value {
            10 => Some(Self::Pending),
            20 => Some(Self::NotNeeded),
            30 => Some(Self::Synced),
            _ => None,
        }
    }

    /// 返回中文名（对齐 PHP `$data[$value]['name']`）
    pub fn name(&self) -> &'static str {
        match self {
            Self::Pending => "待同步",
            Self::NotNeeded => "无需同步",
            Self::Synced => "已同步",
        }
    }

    /// 取中文名或"未知"（对齐 PHP `syncStatusName` 在 value 不存在时返回 `'未知'`）
    pub fn name_or_unknown(value: i64) -> &'static str {
        Self::from_value(value).map(|v| v.name()).unwrap_or("未知")
    }
}

/// 支付状态 — 对齐 PHP `CustomerSyncTypeEnum::payStatusData`
///
/// | value | name | color |
/// |-------|------|-------|
/// | 10 | 未付款 | red |
/// | 20 | 已付款 | blue |
/// | 30 | 已退款 | orange |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomerPayStatus {
    Unpaid = 10,
    Paid = 20,
    Refunded = 30,
}

impl CustomerPayStatus {
    pub fn from_value(value: i64) -> Option<Self> {
        match value {
            10 => Some(Self::Unpaid),
            20 => Some(Self::Paid),
            30 => Some(Self::Refunded),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Unpaid => "未付款",
            Self::Paid => "已付款",
            Self::Refunded => "已退款",
        }
    }

    pub fn name_or_unknown(value: i64) -> &'static str {
        Self::from_value(value).map(|v| v.name()).unwrap_or("未知")
    }
}

/// 订单状态 — 对齐 PHP `CustomerSyncTypeEnum::orderStatusData`
///
/// | value | name | color |
/// |-------|------|-------|
/// | 10 | 进行中 | red |
/// | 20 | 已经取消 | blue |
/// | 30 | 已完成 | orange |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    InProgress = 10,
    Cancelled = 20,
    Completed = 30,
}

impl OrderStatus {
    pub fn from_value(value: i64) -> Option<Self> {
        match value {
            10 => Some(Self::InProgress),
            20 => Some(Self::Cancelled),
            30 => Some(Self::Completed),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::InProgress => "进行中",
            Self::Cancelled => "已经取消",
            Self::Completed => "已完成",
        }
    }

    pub fn name_or_unknown(value: i64) -> &'static str {
        Self::from_value(value).map(|v| v.name()).unwrap_or("未知")
    }
}

/// 支付来源 — 对齐 PHP `CustomerSyncTypeEnum::paySourceData`
///
/// **PHP 端 value 是字符串**，非数字。
///
/// | value | name | color |
/// |-------|------|-------|
/// | `"icbc"` | 工商银行 | red |
/// | `"ccb"` | 建设银行 | blue |
/// | `"fuiou"` | 富友支付 | orange |
/// | `"cash"` | 现金支付 | orange |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaySource {
    Icbc,
    Ccb,
    Fuiou,
    Cash,
}

impl PaySource {
    /// 从字符串构造，无效值返回 `None`（对齐 PHP `isset($data[$value])`）
    pub fn from_value(value: &str) -> Option<Self> {
        match value {
            "icbc" => Some(Self::Icbc),
            "ccb" => Some(Self::Ccb),
            "fuiou" => Some(Self::Fuiou),
            "cash" => Some(Self::Cash),
            _ => None,
        }
    }

    /// 返回中文名（对齐 PHP `$data[$value]['name']`）
    pub fn name(&self) -> &'static str {
        match self {
            Self::Icbc => "工商银行",
            Self::Ccb => "建设银行",
            Self::Fuiou => "富友支付",
            Self::Cash => "现金支付",
        }
    }

    /// 取中文名或"未知"（对齐 PHP `paySourceName` 在 value 不存在时返回 `'未知'`）
    pub fn name_or_unknown(value: &str) -> &'static str {
        Self::from_value(value).map(|v| v.name()).unwrap_or("未知")
    }
}

/// 统一入口类型（对齐 PHP `CustomerSyncTypeEnum` 类名）
///
/// Rust 端将 PHP 单个类的 5 组枚举拆分为 4 个独立 enum，
/// 通过此入口结构体提供与 PHP 类静态方法同名的关联函数。
///
/// 注意：`pay_type` 数据来自 [`super::contract_status::PayType`]（与 ContractStatusEnum 共享），
/// 因此本入口不重复定义 `pay_type_name`，调用方直接使用
/// [`crate::enums::ContractStatusEnum::pay_type_name`]。
pub struct CustomerSyncTypeEnum;

impl CustomerSyncTypeEnum {
    /// 同步状态名（对齐 PHP `CustomerSyncTypeEnum::syncStatusName`）
    pub fn sync_status_name(value: i64) -> &'static str {
        SyncStatus::name_or_unknown(value)
    }

    /// 支付状态名（对齐 PHP `CustomerSyncTypeEnum::payStatusName`）
    pub fn pay_status_name(value: i64) -> &'static str {
        CustomerPayStatus::name_or_unknown(value)
    }

    /// 订单状态名（对齐 PHP `CustomerSyncTypeEnum::orderStatusName`）
    pub fn order_status_name(value: i64) -> &'static str {
        OrderStatus::name_or_unknown(value)
    }

    /// 支付来源名（对齐 PHP `CustomerSyncTypeEnum::paySourceName`）
    ///
    /// **注意**：PHP 端 value 是字符串（`'icbc'`/`'ccb'`/`'fuiou'`/`'cash'`），
    /// 非数字。
    pub fn pay_source_name(value: &str) -> &'static str {
        PaySource::name_or_unknown(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_status_name_aligns_php() {
        assert_eq!(CustomerSyncTypeEnum::sync_status_name(10), "待同步");
        assert_eq!(CustomerSyncTypeEnum::sync_status_name(20), "无需同步");
        assert_eq!(CustomerSyncTypeEnum::sync_status_name(30), "已同步");
        // PHP isset 不存在 → '未知'
        assert_eq!(CustomerSyncTypeEnum::sync_status_name(99), "未知");
        assert_eq!(CustomerSyncTypeEnum::sync_status_name(0), "未知");
    }

    #[test]
    fn test_pay_status_name_aligns_php() {
        assert_eq!(CustomerSyncTypeEnum::pay_status_name(10), "未付款");
        assert_eq!(CustomerSyncTypeEnum::pay_status_name(20), "已付款");
        assert_eq!(CustomerSyncTypeEnum::pay_status_name(30), "已退款");
        assert_eq!(CustomerSyncTypeEnum::pay_status_name(99), "未知");
    }

    #[test]
    fn test_order_status_name_aligns_php() {
        assert_eq!(CustomerSyncTypeEnum::order_status_name(10), "进行中");
        assert_eq!(CustomerSyncTypeEnum::order_status_name(20), "已经取消");
        assert_eq!(CustomerSyncTypeEnum::order_status_name(30), "已完成");
        assert_eq!(CustomerSyncTypeEnum::order_status_name(99), "未知");
    }

    #[test]
    fn test_pay_source_name_aligns_php() {
        // PHP paySourceData value 是字符串
        assert_eq!(CustomerSyncTypeEnum::pay_source_name("icbc"), "工商银行");
        assert_eq!(CustomerSyncTypeEnum::pay_source_name("ccb"), "建设银行");
        assert_eq!(CustomerSyncTypeEnum::pay_source_name("fuiou"), "富友支付");
        assert_eq!(CustomerSyncTypeEnum::pay_source_name("cash"), "现金支付");
        // PHP $map[$value] ?? '未知'
        assert_eq!(CustomerSyncTypeEnum::pay_source_name("unknown"), "未知");
        assert_eq!(CustomerSyncTypeEnum::pay_source_name(""), "未知");
    }

    #[test]
    fn test_pay_source_from_value_string_key() {
        // PHP paySourceData 用字符串 value 作为 key
        assert_eq!(PaySource::from_value("icbc"), Some(PaySource::Icbc));
        assert_eq!(PaySource::from_value("invalid"), None);
    }
}
