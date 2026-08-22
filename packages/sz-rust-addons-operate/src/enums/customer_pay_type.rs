//! CustomerPayTypeEnum — 对齐 PHP `app\common\enum\oa\CustomerPayTypeEnum`
//!
//! PHP 端继承 `MyCLabs\Enum\Enum`，使用类常量定义 3 个支付类型。
//! Rust 端用强类型 enum 对齐。
//!
//! ## PHP 行为对齐
//!
//! PHP 类常量：
//! - `const EPAY = 1`（转账）
//! - `const CASH = 2`（现金）
//! - `const UNITE = 3`（转账+现金）
//!
//! CustomerPay 在 `onlinePayment` / `onPayBuy` 方法中用
//! `$data['pay_type'] == CustomerPayTypeEnum::EPAY` 做分支判断。

/// 客户支付类型 — 对齐 PHP `CustomerPayTypeEnum`
///
/// | 常量 | 值 | 含义 |
/// |------|----|------|
/// | `EPAY` | 1 | 扫码转账 |
/// | `CASH` | 2 | 现金支付 |
/// | `UNITE` | 3 | 转账+现金 |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomerPayType {
    /// 转账（PHP `CustomerPayTypeEnum::EPAY = 1`）
    Epay = 1,
    /// 现金（PHP `CustomerPayTypeEnum::CASH = 2`）
    Cash = 2,
    /// 转账+现金（PHP `CustomerPayTypeEnum::UNITE = 3`）
    Unite = 3,
}

impl CustomerPayType {
    /// 从数值构造，无效值返回 `None`
    pub fn from_value(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::Epay),
            2 => Some(Self::Cash),
            3 => Some(Self::Unite),
            _ => None,
        }
    }

    /// 是否为 EPAY（对齐 PHP `$data['pay_type'] == CustomerPayTypeEnum::EPAY`）
    pub fn is_epay(value: i64) -> bool {
        matches!(Self::from_value(value), Some(Self::Epay))
    }

    /// 是否为 UNITE（对齐 PHP `$data['pay_type'] == CustomerPayTypeEnum::UNITE`）
    pub fn is_unite(value: i64) -> bool {
        matches!(Self::from_value(value), Some(Self::Unite))
    }

    /// EPAY 或 UNITE（对齐 PHP `pay_type == EPAY || pay_type == UNITE`）
    pub fn is_epay_or_unite(value: i64) -> bool {
        Self::is_epay(value) || Self::is_unite(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_value_aligns_php_constants() {
        assert_eq!(CustomerPayType::from_value(1), Some(CustomerPayType::Epay));
        assert_eq!(CustomerPayType::from_value(2), Some(CustomerPayType::Cash));
        assert_eq!(CustomerPayType::from_value(3), Some(CustomerPayType::Unite));
        assert_eq!(CustomerPayType::from_value(99), None);
        assert_eq!(CustomerPayType::from_value(0), None);
    }

    #[test]
    fn test_is_epay_aligns_php_comparison() {
        // PHP: $data['pay_type'] == CustomerPayTypeEnum::EPAY
        assert!(CustomerPayType::is_epay(1));
        assert!(!CustomerPayType::is_epay(2));
        assert!(!CustomerPayType::is_epay(3));
        assert!(!CustomerPayType::is_epay(0));
    }

    #[test]
    fn test_is_unite_aligns_php_comparison() {
        // PHP: $data['pay_type'] == CustomerPayTypeEnum::UNITE
        assert!(CustomerPayType::is_unite(3));
        assert!(!CustomerPayType::is_unite(1));
        assert!(!CustomerPayType::is_unite(2));
    }

    #[test]
    fn test_is_epay_or_unite_aligns_php_or_condition() {
        // PHP: pay_type == EPAY || pay_type == UNITE
        assert!(CustomerPayType::is_epay_or_unite(1));
        assert!(CustomerPayType::is_epay_or_unite(3));
        assert!(!CustomerPayType::is_epay_or_unite(2));
        assert!(!CustomerPayType::is_epay_or_unite(0));
    }
}
