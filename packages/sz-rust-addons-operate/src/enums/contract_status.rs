//! ContractStatusEnum — 对齐 PHP `app\common\enum\oa\ContractStatusEnum`
//!
//! PHP 端继承 `MyCLabs\Enum\Enum`，使用静态方法返回数组。
//! Rust 端用强类型 enum + `from_value` + `name` 方法对齐。
//!
//! ## PHP 行为对齐
//!
//! PHP `xxxName($value)` 在 value 不存在时返回 `'未知'`；
//! 调用方（如 `getStatusTextAttr`）用 `!empty($data['status']) ? ... : ''` 包裹，
//! 即 status 为 0 或不存在时返回空字符串 `''`。
//!
//! Rust 端用 [`Self::from_value`] 返回 `Option<Self>`，[`Self::name`] 返回 `&'static str`，
//! 调用方在访问器中按 PHP 语义判空。

/// 商户状态 — 对齐 PHP `ContractStatusEnum::customerStatusData`
///
/// | value | name | color |
/// |-------|------|-------|
/// | 1 | 在租 | red |
/// | 2 | 撤场 | blue |
/// | 3 | 未签约 | orange |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomerStatus {
    InRent = 1,
    Withdrawn = 2,
    NotSigned = 3,
}

impl CustomerStatus {
    /// 从数值构造，无效值返回 `None`（对齐 PHP `isset($data[$value])`）
    pub fn from_value(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::InRent),
            2 => Some(Self::Withdrawn),
            3 => Some(Self::NotSigned),
            _ => None,
        }
    }

    /// 返回中文名（对齐 PHP `$data[$value]['name']`）
    pub fn name(&self) -> &'static str {
        match self {
            Self::InRent => "在租",
            Self::Withdrawn => "撤场",
            Self::NotSigned => "未签约",
        }
    }

    /// 取中文名或"未知"（对齐 PHP `customerStatusName` 在 value 不存在时返回 `'未知'`）
    pub fn name_or_unknown(value: i64) -> &'static str {
        Self::from_value(value).map(|v| v.name()).unwrap_or("未知")
    }
}

/// 合同状态 — 对齐 PHP `ContractStatusEnum::contractStatusData`
///
/// | value | name | color |
/// |-------|------|-------|
/// | 1 | 待生效 | red |
/// | 2 | 有效期 | blue |
/// | 3 | 已失效 | orange |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractStatus {
    Pending = 1,
    Active = 2,
    Expired = 3,
}

impl ContractStatus {
    pub fn from_value(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::Pending),
            2 => Some(Self::Active),
            3 => Some(Self::Expired),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Pending => "待生效",
            Self::Active => "有效期",
            Self::Expired => "已失效",
        }
    }

    pub fn name_or_unknown(value: i64) -> &'static str {
        Self::from_value(value).map(|v| v.name()).unwrap_or("未知")
    }
}

/// 签约状态 — 对齐 PHP `ContractStatusEnum::signingData`
///
/// | value | name | color |
/// |-------|------|-------|
/// | 1 | 待签约 | red |
/// | 2 | 已签约 | blue |
/// | 3 | 已解约 | orange |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningStatus {
    Pending = 1,
    Signed = 2,
    Terminated = 3,
}

impl SigningStatus {
    pub fn from_value(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::Pending),
            2 => Some(Self::Signed),
            3 => Some(Self::Terminated),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Pending => "待签约",
            Self::Signed => "已签约",
            Self::Terminated => "已解约",
        }
    }

    pub fn name_or_unknown(value: i64) -> &'static str {
        Self::from_value(value).map(|v| v.name()).unwrap_or("未知")
    }
}

/// 缴费状态 — 对齐 PHP `ContractStatusEnum::payStatusData`
///
/// | value | name | color |
/// |-------|------|-------|
/// | 1 | 待缴费 | red |
/// | 2 | 已缴费 | blue |
/// | 3 | 缴费中 | orange |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayStatus {
    Pending = 1,
    Paid = 2,
    Paying = 3,
}

impl PayStatus {
    pub fn from_value(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::Pending),
            2 => Some(Self::Paid),
            3 => Some(Self::Paying),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Pending => "待缴费",
            Self::Paid => "已缴费",
            Self::Paying => "缴费中",
        }
    }

    pub fn name_or_unknown(value: i64) -> &'static str {
        Self::from_value(value).map(|v| v.name()).unwrap_or("未知")
    }
}

/// 支付方式 — 对齐 PHP `ContractStatusEnum::payTypeData`
///
/// | value | name | color |
/// |-------|------|-------|
/// | 1 | 扫码转账 | red |
/// | 2 | 现金支付 | blue |
/// | 3 | 转账+现金 | orange |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayType {
    QrTransfer = 1,
    Cash = 2,
    Mixed = 3,
}

impl PayType {
    pub fn from_value(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::QrTransfer),
            2 => Some(Self::Cash),
            3 => Some(Self::Mixed),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::QrTransfer => "扫码转账",
            Self::Cash => "现金支付",
            Self::Mixed => "转账+现金",
        }
    }

    pub fn name_or_unknown(value: i64) -> &'static str {
        Self::from_value(value).map(|v| v.name()).unwrap_or("未知")
    }
}

/// 统一入口类型（对齐 PHP `ContractStatusEnum` 类名）
///
/// Rust 端将 PHP 单个类的 5 组枚举拆分为 5 个独立 enum，
/// 通过此入口结构体提供与 PHP 类静态方法同名的关联函数。
pub struct ContractStatusEnum;

impl ContractStatusEnum {
    /// 商户状态名（对齐 PHP `ContractStatusEnum::customerStatusName`）
    pub fn customer_status_name(value: i64) -> &'static str {
        CustomerStatus::name_or_unknown(value)
    }

    /// 合同状态名（对齐 PHP `ContractStatusEnum::contractStatusName`）
    pub fn contract_status_name(value: i64) -> &'static str {
        ContractStatus::name_or_unknown(value)
    }

    /// 签约状态名（对齐 PHP `ContractStatusEnum::signingName`）
    pub fn signing_name(value: i64) -> &'static str {
        SigningStatus::name_or_unknown(value)
    }

    /// 缴费状态名（对齐 PHP `ContractStatusEnum::payStatusName`）
    pub fn pay_status_name(value: i64) -> &'static str {
        PayStatus::name_or_unknown(value)
    }

    /// 支付方式名（对齐 PHP `ContractStatusEnum::payTypeName`）
    pub fn pay_type_name(value: i64) -> &'static str {
        PayType::name_or_unknown(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_customer_status_name_aligns_php() {
        assert_eq!(ContractStatusEnum::customer_status_name(1), "在租");
        assert_eq!(ContractStatusEnum::customer_status_name(2), "撤场");
        assert_eq!(ContractStatusEnum::customer_status_name(3), "未签约");
        // PHP isset 不存在 → '未知'
        assert_eq!(ContractStatusEnum::customer_status_name(99), "未知");
        assert_eq!(ContractStatusEnum::customer_status_name(0), "未知");
    }

    #[test]
    fn test_contract_status_name_aligns_php() {
        assert_eq!(ContractStatusEnum::contract_status_name(1), "待生效");
        assert_eq!(ContractStatusEnum::contract_status_name(2), "有效期");
        assert_eq!(ContractStatusEnum::contract_status_name(3), "已失效");
        assert_eq!(ContractStatusEnum::contract_status_name(99), "未知");
    }

    #[test]
    fn test_signing_name_aligns_php() {
        assert_eq!(ContractStatusEnum::signing_name(1), "待签约");
        assert_eq!(ContractStatusEnum::signing_name(2), "已签约");
        assert_eq!(ContractStatusEnum::signing_name(3), "已解约");
        assert_eq!(ContractStatusEnum::signing_name(99), "未知");
    }

    #[test]
    fn test_pay_status_name_aligns_php() {
        assert_eq!(ContractStatusEnum::pay_status_name(1), "待缴费");
        assert_eq!(ContractStatusEnum::pay_status_name(2), "已缴费");
        assert_eq!(ContractStatusEnum::pay_status_name(3), "缴费中");
        assert_eq!(ContractStatusEnum::pay_status_name(99), "未知");
    }

    #[test]
    fn test_pay_type_name_aligns_php() {
        assert_eq!(ContractStatusEnum::pay_type_name(1), "扫码转账");
        assert_eq!(ContractStatusEnum::pay_type_name(2), "现金支付");
        assert_eq!(ContractStatusEnum::pay_type_name(3), "转账+现金");
        assert_eq!(ContractStatusEnum::pay_type_name(99), "未知");
    }
}
