//! 合规模式配置（Compliance Mode）— Phase 7c.7
//!
//! 对应 `SzRSQL技术实现方案.md` 12.3 节 — 合规模式配置。
//!
//! # 设计
//!
//! 合规模式将多个安全特性组合为预设配置，一键启用满足特定法规要求的安全能力：
//!
//! - **Standard** — 基础安全（密码策略 + 审计日志）
//! - **GDPR** — 审计日志 + 数据擦除 + 数据可移植性
//! - **HIPAA** — TDE + 审计日志 + 脱敏 + 加密备份
//! - **PciDss** — TDE + 审计日志 + SQL 防火墙 + 列级加密
//! - **Custom** — 自定义规则集合
//!
//! ## ComplianceRule 规则项
//!
//! 每个合规规则对应一个安全特性的开关：
//!
//! - `Tde` — 透明数据加密
//! - `AuditLog` — 审计日志
//! - `DataMasking` — 数据脱敏
//! - `ColumnEncryption` — 列级加密
//! - `SqlFirewall` — SQL 防火墙
//! - `DataErasure` — 数据擦除（被遗忘权）
//! - `DataPortability` — 数据可移植性（导出）
//! - `EncryptedBackup` — 加密备份
//! - `PasswordPolicy` — 密码策略
//! - `RowLevelSecurity` — 行级安全
//!
//! ## 验证
//!
//! - `ComplianceMode::HIPAA` 启用后 → 必须包含 TDE + 审计日志 + 脱敏 + 加密备份
//! - `ComplianceMode::GDPR` 启用后 → 必须包含 审计日志 + 数据擦除 + 数据可移植性
//! - `validate()` 检查当前配置是否满足指定模式的要求
//!
//! 对应 `SzRSQL实施进度.md` Phase 7c.7。

use std::collections::HashSet;

// =====================================================================
//  常量
// =====================================================================

/// 标准模式名称
pub const MODE_STANDARD: &str = "Standard";

/// GDPR 模式名称
pub const MODE_GDPR: &str = "GDPR";

/// HIPAA 模式名称
pub const MODE_HIPAA: &str = "HIPAA";

/// PCI-DSS 模式名称
pub const MODE_PCI_DSS: &str = "PCI-DSS";

/// 自定义模式名称
pub const MODE_CUSTOM: &str = "Custom";

// =====================================================================
//  错误类型
// =====================================================================

/// 合规错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ComplianceError {
    /// 缺少必需规则
    #[error("missing required rule: {rule} (mode: {mode})")]
    MissingRequiredRule {
        /// 缺失的规则名
        rule: String,
        /// 模式名
        mode: String,
    },
    /// 未知合规模式
    #[error("unknown compliance mode: {0}")]
    UnknownMode(String),
    /// 未知合规规则
    #[error("unknown compliance rule: {0}")]
    UnknownRule(String),
}

// =====================================================================
//  ComplianceRule — 合规规则项
// =====================================================================

/// 合规规则项 — 每个对应一个安全特性
///
/// 用于描述合规模式要求启用的安全能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComplianceRule {
    /// 透明数据加密（TDE）
    Tde,
    /// 审计日志
    AuditLog,
    /// 数据脱敏
    DataMasking,
    /// 列级加密
    ColumnEncryption,
    /// SQL 防火墙
    SqlFirewall,
    /// 数据擦除（GDPR 被遗忘权）
    DataErasure,
    /// 数据可移植性（GDPR 数据导出）
    DataPortability,
    /// 加密备份
    EncryptedBackup,
    /// 密码策略
    PasswordPolicy,
    /// 行级安全（RLS）
    RowLevelSecurity,
}

impl ComplianceRule {
    /// 转换为字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            ComplianceRule::Tde => "TDE",
            ComplianceRule::AuditLog => "AuditLog",
            ComplianceRule::DataMasking => "DataMasking",
            ComplianceRule::ColumnEncryption => "ColumnEncryption",
            ComplianceRule::SqlFirewall => "SqlFirewall",
            ComplianceRule::DataErasure => "DataErasure",
            ComplianceRule::DataPortability => "DataPortability",
            ComplianceRule::EncryptedBackup => "EncryptedBackup",
            ComplianceRule::PasswordPolicy => "PasswordPolicy",
            ComplianceRule::RowLevelSecurity => "RowLevelSecurity",
        }
    }

    /// 从字符串解析
    pub fn from_name(s: &str) -> Option<ComplianceRule> {
        match s {
            "TDE" => Some(ComplianceRule::Tde),
            "AuditLog" => Some(ComplianceRule::AuditLog),
            "DataMasking" => Some(ComplianceRule::DataMasking),
            "ColumnEncryption" => Some(ComplianceRule::ColumnEncryption),
            "SqlFirewall" => Some(ComplianceRule::SqlFirewall),
            "DataErasure" => Some(ComplianceRule::DataErasure),
            "DataPortability" => Some(ComplianceRule::DataPortability),
            "EncryptedBackup" => Some(ComplianceRule::EncryptedBackup),
            "PasswordPolicy" => Some(ComplianceRule::PasswordPolicy),
            "RowLevelSecurity" => Some(ComplianceRule::RowLevelSecurity),
            _ => None,
        }
    }

    /// 获取所有规则变体
    pub fn all() -> Vec<ComplianceRule> {
        vec![
            ComplianceRule::Tde,
            ComplianceRule::AuditLog,
            ComplianceRule::DataMasking,
            ComplianceRule::ColumnEncryption,
            ComplianceRule::SqlFirewall,
            ComplianceRule::DataErasure,
            ComplianceRule::DataPortability,
            ComplianceRule::EncryptedBackup,
            ComplianceRule::PasswordPolicy,
            ComplianceRule::RowLevelSecurity,
        ]
    }
}

impl std::fmt::Display for ComplianceRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =====================================================================
//  ComplianceMode — 合规模式
// =====================================================================

/// 合规模式 — 预设的安全配置组合
///
/// # 模式说明
///
/// - **Standard** — 基础安全（密码策略 + 审计日志）
/// - **GDPR** — 审计日志 + 数据擦除 + 数据可移植性 + 密码策略
/// - **HIPAA** — TDE + 审计日志 + 脱敏 + 加密备份 + 密码策略
/// - **PciDss** — TDE + 审计日志 + SQL 防火墙 + 列级加密 + 密码策略
/// - **Custom** — 自定义规则集合
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComplianceMode {
    /// 标准模式：基础安全
    Standard,
    /// GDPR 模式：启用审计日志 + 数据擦除 + 数据可移植性
    GDPR,
    /// HIPAA 模式：强制 TDE + 审计日志 + 脱敏 + 加密备份
    HIPAA,
    /// PCI-DSS 模式：强制 TDE + 审计日志 + SQL 防火墙 + 列级加密
    PciDss,
    /// 自定义模式：自定义规则集合
    Custom(Vec<ComplianceRule>),
}

impl ComplianceMode {
    /// 获取模式名称
    pub fn name(&self) -> &'static str {
        match self {
            ComplianceMode::Standard => MODE_STANDARD,
            ComplianceMode::GDPR => MODE_GDPR,
            ComplianceMode::HIPAA => MODE_HIPAA,
            ComplianceMode::PciDss => MODE_PCI_DSS,
            ComplianceMode::Custom(_) => MODE_CUSTOM,
        }
    }

    /// 获取该模式要求的规则集合
    ///
    /// 返回该合规模式强制要求的全部规则。
    pub fn required_rules(&self) -> Vec<ComplianceRule> {
        match self {
            ComplianceMode::Standard => {
                vec![ComplianceRule::PasswordPolicy, ComplianceRule::AuditLog]
            }
            ComplianceMode::GDPR => vec![
                ComplianceRule::AuditLog,
                ComplianceRule::DataErasure,
                ComplianceRule::DataPortability,
                ComplianceRule::PasswordPolicy,
            ],
            ComplianceMode::HIPAA => vec![
                ComplianceRule::Tde,
                ComplianceRule::AuditLog,
                ComplianceRule::DataMasking,
                ComplianceRule::EncryptedBackup,
                ComplianceRule::PasswordPolicy,
            ],
            ComplianceMode::PciDss => vec![
                ComplianceRule::Tde,
                ComplianceRule::AuditLog,
                ComplianceRule::SqlFirewall,
                ComplianceRule::ColumnEncryption,
                ComplianceRule::PasswordPolicy,
            ],
            ComplianceMode::Custom(rules) => rules.clone(),
        }
    }

    /// 从字符串解析模式（不含 Custom 的规则列表）
    ///
    /// 仅支持 Standard/GDPR/HIPAA/PciDss，其他返回 `None`。
    pub fn from_name(s: &str) -> Option<ComplianceMode> {
        match s {
            "Standard" | "standard" => Some(ComplianceMode::Standard),
            "GDPR" | "gdpr" => Some(ComplianceMode::GDPR),
            "HIPAA" | "hipaa" => Some(ComplianceMode::HIPAA),
            "PCI-DSS" | "PciDss" | "pci-dss" | "pci" => Some(ComplianceMode::PciDss),
            _ => None,
        }
    }
}

impl std::fmt::Display for ComplianceMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// =====================================================================
//  ComplianceConfig — 合规配置
// =====================================================================

/// 合规配置 — 当前已启用的规则集合 + 目标模式
///
/// # 工作流程
///
/// 1. `new()` 创建空配置
/// 2. `enable_rule(rule)` 启用规则
/// 3. `disable_rule(rule)` 禁用规则
/// 4. `validate(mode)` 验证当前配置是否满足指定模式
/// 5. `apply_mode(mode)` 一键应用模式（启用该模式要求的全部规则）
#[derive(Debug, Clone, Default)]
pub struct ComplianceConfig {
    /// 当前已启用的规则集合
    enabled_rules: HashSet<ComplianceRule>,
    /// 当前模式（若已应用）
    current_mode: Option<ComplianceMode>,
}

impl ComplianceConfig {
    /// 创建空配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 启用规则
    pub fn enable_rule(&mut self, rule: ComplianceRule) {
        self.enabled_rules.insert(rule);
    }

    /// 禁用规则
    pub fn disable_rule(&mut self, rule: ComplianceRule) {
        self.enabled_rules.remove(&rule);
    }

    /// 规则是否已启用
    pub fn is_enabled(&self, rule: ComplianceRule) -> bool {
        self.enabled_rules.contains(&rule)
    }

    /// 获取已启用规则数量
    pub fn enabled_count(&self) -> usize {
        self.enabled_rules.len()
    }

    /// 获取已启用规则列表
    pub fn enabled_rules(&self) -> Vec<ComplianceRule> {
        self.enabled_rules.iter().copied().collect()
    }

    /// 当前模式
    pub fn current_mode(&self) -> Option<&ComplianceMode> {
        self.current_mode.as_ref()
    }

    /// 清空所有规则
    pub fn clear(&mut self) {
        self.enabled_rules.clear();
        self.current_mode = None;
    }

    /// 应用合规模式 — 启用该模式要求的全部规则
    ///
    /// 注意：此操作会保留之前已启用的规则，仅追加新模式要求的规则。
    pub fn apply_mode(&mut self, mode: ComplianceMode) {
        for rule in mode.required_rules() {
            self.enabled_rules.insert(rule);
        }
        self.current_mode = Some(mode);
    }

    /// 验证当前配置是否满足指定模式
    ///
    /// 返回 `Ok(())` 表示当前已启用的规则覆盖了模式要求的全部规则。
    /// 返回 `Err(MissingRequiredRule)` 表示缺少必需规则。
    pub fn validate(&self, mode: &ComplianceMode) -> Result<(), ComplianceError> {
        let required: HashSet<ComplianceRule> = mode.required_rules().into_iter().collect();
        for rule in &required {
            if !self.enabled_rules.contains(rule) {
                return Err(ComplianceError::MissingRequiredRule {
                    rule: rule.as_str().to_string(),
                    mode: mode.name().to_string(),
                });
            }
        }
        Ok(())
    }

    /// 获取缺少的规则（相对于指定模式）
    pub fn missing_rules(&self, mode: &ComplianceMode) -> Vec<ComplianceRule> {
        mode.required_rules()
            .into_iter()
            .filter(|r| !self.enabled_rules.contains(r))
            .collect()
    }

    /// 检查是否满足指定模式（不返回错误详情）
    pub fn satisfies(&self, mode: &ComplianceMode) -> bool {
        self.validate(mode).is_ok()
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  ComplianceRule 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c7_rule_as_str() {
        assert_eq!(ComplianceRule::Tde.as_str(), "TDE");
        assert_eq!(ComplianceRule::AuditLog.as_str(), "AuditLog");
        assert_eq!(ComplianceRule::DataMasking.as_str(), "DataMasking");
        assert_eq!(
            ComplianceRule::ColumnEncryption.as_str(),
            "ColumnEncryption"
        );
        assert_eq!(ComplianceRule::SqlFirewall.as_str(), "SqlFirewall");
        assert_eq!(ComplianceRule::DataErasure.as_str(), "DataErasure");
        assert_eq!(ComplianceRule::DataPortability.as_str(), "DataPortability");
        assert_eq!(ComplianceRule::EncryptedBackup.as_str(), "EncryptedBackup");
        assert_eq!(ComplianceRule::PasswordPolicy.as_str(), "PasswordPolicy");
        assert_eq!(
            ComplianceRule::RowLevelSecurity.as_str(),
            "RowLevelSecurity"
        );
    }

    #[test]
    fn test_7c7_rule_from_name() {
        assert_eq!(ComplianceRule::from_name("TDE"), Some(ComplianceRule::Tde));
        assert_eq!(
            ComplianceRule::from_name("AuditLog"),
            Some(ComplianceRule::AuditLog)
        );
        assert_eq!(
            ComplianceRule::from_name("DataMasking"),
            Some(ComplianceRule::DataMasking)
        );
        assert_eq!(
            ComplianceRule::from_name("ColumnEncryption"),
            Some(ComplianceRule::ColumnEncryption)
        );
        assert_eq!(
            ComplianceRule::from_name("SqlFirewall"),
            Some(ComplianceRule::SqlFirewall)
        );
        assert_eq!(
            ComplianceRule::from_name("DataErasure"),
            Some(ComplianceRule::DataErasure)
        );
        assert_eq!(
            ComplianceRule::from_name("DataPortability"),
            Some(ComplianceRule::DataPortability)
        );
        assert_eq!(
            ComplianceRule::from_name("EncryptedBackup"),
            Some(ComplianceRule::EncryptedBackup)
        );
        assert_eq!(
            ComplianceRule::from_name("PasswordPolicy"),
            Some(ComplianceRule::PasswordPolicy)
        );
        assert_eq!(
            ComplianceRule::from_name("RowLevelSecurity"),
            Some(ComplianceRule::RowLevelSecurity)
        );
    }

    #[test]
    fn test_7c7_rule_from_name_unknown() {
        assert_eq!(ComplianceRule::from_name("Unknown"), None);
        assert_eq!(ComplianceRule::from_name(""), None);
    }

    #[test]
    fn test_7c7_rule_roundtrip() {
        for rule in ComplianceRule::all() {
            let s = rule.as_str();
            assert_eq!(ComplianceRule::from_name(s), Some(rule));
        }
    }

    #[test]
    fn test_7c7_rule_all_count() {
        assert_eq!(ComplianceRule::all().len(), 10);
    }

    #[test]
    fn test_7c7_rule_display() {
        assert_eq!(format!("{}", ComplianceRule::Tde), "TDE");
        assert_eq!(format!("{}", ComplianceRule::AuditLog), "AuditLog");
    }

    // -----------------------------------------------------------------
    //  ComplianceMode 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c7_mode_standard_required_rules() {
        let rules = ComplianceMode::Standard.required_rules();
        assert!(rules.contains(&ComplianceRule::PasswordPolicy));
        assert!(rules.contains(&ComplianceRule::AuditLog));
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn test_7c7_mode_gdpr_required_rules() {
        let rules = ComplianceMode::GDPR.required_rules();
        assert!(rules.contains(&ComplianceRule::AuditLog));
        assert!(rules.contains(&ComplianceRule::DataErasure));
        assert!(rules.contains(&ComplianceRule::DataPortability));
        assert!(rules.contains(&ComplianceRule::PasswordPolicy));
        assert_eq!(rules.len(), 4);
    }

    #[test]
    fn test_7c7_mode_hipaa_required_rules() {
        let rules = ComplianceMode::HIPAA.required_rules();
        assert!(rules.contains(&ComplianceRule::Tde));
        assert!(rules.contains(&ComplianceRule::AuditLog));
        assert!(rules.contains(&ComplianceRule::DataMasking));
        assert!(rules.contains(&ComplianceRule::EncryptedBackup));
        assert!(rules.contains(&ComplianceRule::PasswordPolicy));
        assert_eq!(rules.len(), 5);
    }

    #[test]
    fn test_7c7_mode_pci_dss_required_rules() {
        let rules = ComplianceMode::PciDss.required_rules();
        assert!(rules.contains(&ComplianceRule::Tde));
        assert!(rules.contains(&ComplianceRule::AuditLog));
        assert!(rules.contains(&ComplianceRule::SqlFirewall));
        assert!(rules.contains(&ComplianceRule::ColumnEncryption));
        assert!(rules.contains(&ComplianceRule::PasswordPolicy));
        assert_eq!(rules.len(), 5);
    }

    #[test]
    fn test_7c7_mode_custom_required_rules() {
        let custom = ComplianceMode::Custom(vec![ComplianceRule::Tde, ComplianceRule::AuditLog]);
        let rules = custom.required_rules();
        assert_eq!(rules.len(), 2);
        assert!(rules.contains(&ComplianceRule::Tde));
        assert!(rules.contains(&ComplianceRule::AuditLog));
    }

    #[test]
    fn test_7c7_mode_name() {
        assert_eq!(ComplianceMode::Standard.name(), "Standard");
        assert_eq!(ComplianceMode::GDPR.name(), "GDPR");
        assert_eq!(ComplianceMode::HIPAA.name(), "HIPAA");
        assert_eq!(ComplianceMode::PciDss.name(), "PCI-DSS");
        assert_eq!(ComplianceMode::Custom(vec![]).name(), "Custom");
    }

    #[test]
    fn test_7c7_mode_from_name() {
        assert_eq!(
            ComplianceMode::from_name("Standard"),
            Some(ComplianceMode::Standard)
        );
        assert_eq!(
            ComplianceMode::from_name("standard"),
            Some(ComplianceMode::Standard)
        );
        assert_eq!(
            ComplianceMode::from_name("GDPR"),
            Some(ComplianceMode::GDPR)
        );
        assert_eq!(
            ComplianceMode::from_name("gdpr"),
            Some(ComplianceMode::GDPR)
        );
        assert_eq!(
            ComplianceMode::from_name("HIPAA"),
            Some(ComplianceMode::HIPAA)
        );
        assert_eq!(
            ComplianceMode::from_name("hipaa"),
            Some(ComplianceMode::HIPAA)
        );
        assert_eq!(
            ComplianceMode::from_name("PCI-DSS"),
            Some(ComplianceMode::PciDss)
        );
        assert_eq!(
            ComplianceMode::from_name("PciDss"),
            Some(ComplianceMode::PciDss)
        );
        assert_eq!(
            ComplianceMode::from_name("pci-dss"),
            Some(ComplianceMode::PciDss)
        );
        assert_eq!(
            ComplianceMode::from_name("pci"),
            Some(ComplianceMode::PciDss)
        );
    }

    #[test]
    fn test_7c7_mode_from_name_unknown() {
        assert_eq!(ComplianceMode::from_name("Unknown"), None);
        assert_eq!(ComplianceMode::from_name(""), None);
    }

    #[test]
    fn test_7c7_mode_display() {
        assert_eq!(format!("{}", ComplianceMode::Standard), "Standard");
        assert_eq!(format!("{}", ComplianceMode::GDPR), "GDPR");
        assert_eq!(format!("{}", ComplianceMode::HIPAA), "HIPAA");
        assert_eq!(format!("{}", ComplianceMode::PciDss), "PCI-DSS");
    }

    // -----------------------------------------------------------------
    //  ComplianceConfig 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c7_config_new() {
        let config = ComplianceConfig::new();
        assert_eq!(config.enabled_count(), 0);
        assert!(config.current_mode().is_none());
    }

    #[test]
    fn test_7c7_config_default() {
        let config = ComplianceConfig::default();
        assert_eq!(config.enabled_count(), 0);
    }

    #[test]
    fn test_7c7_config_enable_rule() {
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::Tde);
        assert!(config.is_enabled(ComplianceRule::Tde));
        assert!(!config.is_enabled(ComplianceRule::AuditLog));
        assert_eq!(config.enabled_count(), 1);
    }

    #[test]
    fn test_7c7_config_disable_rule() {
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::Tde);
        config.enable_rule(ComplianceRule::AuditLog);
        config.disable_rule(ComplianceRule::Tde);
        assert!(!config.is_enabled(ComplianceRule::Tde));
        assert!(config.is_enabled(ComplianceRule::AuditLog));
        assert_eq!(config.enabled_count(), 1);
    }

    #[test]
    fn test_7c7_config_enable_duplicate() {
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::Tde);
        config.enable_rule(ComplianceRule::Tde);
        assert_eq!(config.enabled_count(), 1);
    }

    #[test]
    fn test_7c7_config_disable_not_enabled() {
        let mut config = ComplianceConfig::new();
        config.disable_rule(ComplianceRule::Tde);
        assert_eq!(config.enabled_count(), 0);
    }

    #[test]
    fn test_7c7_config_clear() {
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::Tde);
        config.enable_rule(ComplianceRule::AuditLog);
        config.apply_mode(ComplianceMode::HIPAA);
        config.clear();
        assert_eq!(config.enabled_count(), 0);
        assert!(config.current_mode().is_none());
    }

    #[test]
    fn test_7c7_config_enabled_rules() {
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::Tde);
        config.enable_rule(ComplianceRule::AuditLog);
        let rules = config.enabled_rules();
        assert_eq!(rules.len(), 2);
        assert!(rules.contains(&ComplianceRule::Tde));
        assert!(rules.contains(&ComplianceRule::AuditLog));
    }

    // -----------------------------------------------------------------
    //  apply_mode 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c7_apply_mode_standard() {
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::Standard);
        assert!(config.is_enabled(ComplianceRule::PasswordPolicy));
        assert!(config.is_enabled(ComplianceRule::AuditLog));
        assert_eq!(config.enabled_count(), 2);
        assert_eq!(config.current_mode(), Some(&ComplianceMode::Standard));
    }

    #[test]
    fn test_7c7_apply_mode_gdpr() {
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::GDPR);
        assert!(config.is_enabled(ComplianceRule::AuditLog));
        assert!(config.is_enabled(ComplianceRule::DataErasure));
        assert!(config.is_enabled(ComplianceRule::DataPortability));
        assert!(config.is_enabled(ComplianceRule::PasswordPolicy));
        assert_eq!(config.enabled_count(), 4);
    }

    #[test]
    fn test_7c7_apply_mode_hipaa() {
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::HIPAA);
        assert!(config.is_enabled(ComplianceRule::Tde));
        assert!(config.is_enabled(ComplianceRule::AuditLog));
        assert!(config.is_enabled(ComplianceRule::DataMasking));
        assert!(config.is_enabled(ComplianceRule::EncryptedBackup));
        assert!(config.is_enabled(ComplianceRule::PasswordPolicy));
        assert_eq!(config.enabled_count(), 5);
    }

    #[test]
    fn test_7c7_apply_mode_pci_dss() {
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::PciDss);
        assert!(config.is_enabled(ComplianceRule::Tde));
        assert!(config.is_enabled(ComplianceRule::AuditLog));
        assert!(config.is_enabled(ComplianceRule::SqlFirewall));
        assert!(config.is_enabled(ComplianceRule::ColumnEncryption));
        assert!(config.is_enabled(ComplianceRule::PasswordPolicy));
        assert_eq!(config.enabled_count(), 5);
    }

    #[test]
    fn test_7c7_apply_mode_custom() {
        let mut config = ComplianceConfig::new();
        let custom =
            ComplianceMode::Custom(vec![ComplianceRule::Tde, ComplianceRule::RowLevelSecurity]);
        config.apply_mode(custom.clone());
        assert!(config.is_enabled(ComplianceRule::Tde));
        assert!(config.is_enabled(ComplianceRule::RowLevelSecurity));
        assert_eq!(config.enabled_count(), 2);
    }

    #[test]
    fn test_7c7_apply_mode_preserves_existing_rules() {
        // apply_mode 应保留之前已启用的规则
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::RowLevelSecurity);
        config.apply_mode(ComplianceMode::Standard);
        // Standard 不包含 RowLevelSecurity，但应保留
        assert!(config.is_enabled(ComplianceRule::RowLevelSecurity));
        assert!(config.is_enabled(ComplianceRule::PasswordPolicy));
        assert!(config.is_enabled(ComplianceRule::AuditLog));
        assert_eq!(config.enabled_count(), 3);
    }

    // -----------------------------------------------------------------
    //  validate 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c7_validate_standard_satisfied() {
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::PasswordPolicy);
        config.enable_rule(ComplianceRule::AuditLog);
        assert!(config.validate(&ComplianceMode::Standard).is_ok());
    }

    #[test]
    fn test_7c7_validate_standard_missing_rule() {
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::PasswordPolicy);
        // 缺少 AuditLog
        let result = config.validate(&ComplianceMode::Standard);
        match result {
            Err(ComplianceError::MissingRequiredRule { rule, mode }) => {
                assert_eq!(rule, "AuditLog");
                assert_eq!(mode, "Standard");
            }
            _ => panic!("expected MissingRequiredRule error"),
        }
    }

    #[test]
    fn test_7c7_validate_gdpr_satisfied() {
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::GDPR);
        assert!(config.validate(&ComplianceMode::GDPR).is_ok());
    }

    #[test]
    fn test_7c7_validate_gdpr_missing_rules() {
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::AuditLog);
        // 缺少 DataErasure, DataPortability, PasswordPolicy
        let missing = config.missing_rules(&ComplianceMode::GDPR);
        assert_eq!(missing.len(), 3);
        assert!(missing.contains(&ComplianceRule::DataErasure));
        assert!(missing.contains(&ComplianceRule::DataPortability));
        assert!(missing.contains(&ComplianceRule::PasswordPolicy));
    }

    #[test]
    fn test_7c7_validate_hipaa_satisfied() {
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::HIPAA);
        assert!(config.validate(&ComplianceMode::HIPAA).is_ok());
    }

    #[test]
    fn test_7c7_validate_hipaa_missing_rules() {
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::AuditLog);
        config.enable_rule(ComplianceRule::PasswordPolicy);
        // 缺少 Tde, DataMasking, EncryptedBackup
        let missing = config.missing_rules(&ComplianceMode::HIPAA);
        assert_eq!(missing.len(), 3);
        assert!(missing.contains(&ComplianceRule::Tde));
        assert!(missing.contains(&ComplianceRule::DataMasking));
        assert!(missing.contains(&ComplianceRule::EncryptedBackup));
    }

    #[test]
    fn test_7c7_validate_pci_dss_satisfied() {
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::PciDss);
        assert!(config.validate(&ComplianceMode::PciDss).is_ok());
    }

    #[test]
    fn test_7c7_validate_pci_dss_missing_rules() {
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::AuditLog);
        // 缺少 Tde, SqlFirewall, ColumnEncryption, PasswordPolicy
        let missing = config.missing_rules(&ComplianceMode::PciDss);
        assert_eq!(missing.len(), 4);
    }

    #[test]
    fn test_7c7_validate_custom_satisfied() {
        let custom = ComplianceMode::Custom(vec![ComplianceRule::Tde, ComplianceRule::AuditLog]);
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::Tde);
        config.enable_rule(ComplianceRule::AuditLog);
        assert!(config.validate(&custom).is_ok());
    }

    #[test]
    fn test_7c7_validate_custom_missing() {
        let custom = ComplianceMode::Custom(vec![ComplianceRule::Tde, ComplianceRule::AuditLog]);
        let config = ComplianceConfig::new();
        assert!(config.validate(&custom).is_err());
    }

    #[test]
    fn test_7c7_validate_with_extra_rules() {
        // 启用的规则超出模式要求也算满足
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::Standard);
        config.enable_rule(ComplianceRule::Tde); // 额外规则
        assert!(config.validate(&ComplianceMode::Standard).is_ok());
    }

    // -----------------------------------------------------------------
    //  satisfies 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c7_satisfies_true() {
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::GDPR);
        assert!(config.satisfies(&ComplianceMode::GDPR));
    }

    #[test]
    fn test_7c7_satisfies_false() {
        let config = ComplianceConfig::new();
        assert!(!config.satisfies(&ComplianceMode::GDPR));
    }

    #[test]
    fn test_7c7_missing_rules_empty_when_satisfied() {
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::HIPAA);
        assert!(config.missing_rules(&ComplianceMode::HIPAA).is_empty());
    }

    // -----------------------------------------------------------------
    //  验证标准核心测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c7_full_workflow_gdpr_mode() {
        // 验证标准：GDPR 模式 → 审计日志 + 数据擦除 + 数据可移植性强制启用
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::GDPR);

        // 必需规则全部启用
        assert!(config.is_enabled(ComplianceRule::AuditLog));
        assert!(config.is_enabled(ComplianceRule::DataErasure));
        assert!(config.is_enabled(ComplianceRule::DataPortability));
        assert!(config.is_enabled(ComplianceRule::PasswordPolicy));

        // 验证通过
        assert!(config.validate(&ComplianceMode::GDPR).is_ok());
        assert!(config.satisfies(&ComplianceMode::GDPR));
    }

    #[test]
    fn test_7c7_full_workflow_hipaa_mode() {
        // 验证标准：HIPAA 模式 → TDE + 脱敏 + 加密备份强制
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::HIPAA);

        // 必需规则全部启用
        assert!(config.is_enabled(ComplianceRule::Tde));
        assert!(config.is_enabled(ComplianceRule::AuditLog));
        assert!(config.is_enabled(ComplianceRule::DataMasking));
        assert!(config.is_enabled(ComplianceRule::EncryptedBackup));
        assert!(config.is_enabled(ComplianceRule::PasswordPolicy));

        // 验证通过
        assert!(config.validate(&ComplianceMode::HIPAA).is_ok());
    }

    #[test]
    fn test_7c7_full_workflow_pci_dss_mode() {
        // 验证标准：PCI-DSS 模式 → TDE + 审计日志 + SQL 防火墙 + 列级加密强制
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::PciDss);

        assert!(config.is_enabled(ComplianceRule::Tde));
        assert!(config.is_enabled(ComplianceRule::AuditLog));
        assert!(config.is_enabled(ComplianceRule::SqlFirewall));
        assert!(config.is_enabled(ComplianceRule::ColumnEncryption));
        assert!(config.is_enabled(ComplianceRule::PasswordPolicy));

        assert!(config.validate(&ComplianceMode::PciDss).is_ok());
    }

    #[test]
    fn test_7c7_full_workflow_standard_mode() {
        // 标准模式：密码策略 + 审计日志
        let mut config = ComplianceConfig::new();
        config.apply_mode(ComplianceMode::Standard);

        assert!(config.is_enabled(ComplianceRule::PasswordPolicy));
        assert!(config.is_enabled(ComplianceRule::AuditLog));
        assert_eq!(config.enabled_count(), 2);
        assert!(config.validate(&ComplianceMode::Standard).is_ok());
    }

    #[test]
    fn test_7c7_full_workflow_mode_switch() {
        // 模式切换：Standard → HIPAA → GDPR
        let mut config = ComplianceConfig::new();

        // Standard
        config.apply_mode(ComplianceMode::Standard);
        assert!(config.satisfies(&ComplianceMode::Standard));

        // HIPAA（累积启用）
        config.apply_mode(ComplianceMode::HIPAA);
        assert!(config.satisfies(&ComplianceMode::HIPAA));
        assert!(config.satisfies(&ComplianceMode::Standard)); // 仍满足 Standard

        // GDPR（累积启用）
        config.apply_mode(ComplianceMode::GDPR);
        assert!(config.satisfies(&ComplianceMode::GDPR));
        assert!(config.satisfies(&ComplianceMode::HIPAA)); // 仍满足 HIPAA
        assert!(config.satisfies(&ComplianceMode::Standard));
    }

    #[test]
    fn test_7c7_full_workflow_custom_mode() {
        // 自定义模式：Tde + RowLevelSecurity + DataMasking
        let custom = ComplianceMode::Custom(vec![
            ComplianceRule::Tde,
            ComplianceRule::RowLevelSecurity,
            ComplianceRule::DataMasking,
        ]);
        let mut config = ComplianceConfig::new();
        config.apply_mode(custom.clone());

        assert_eq!(config.enabled_count(), 3);
        assert!(config.validate(&custom).is_ok());

        // 禁用一个规则后不满足
        config.disable_rule(ComplianceRule::Tde);
        assert!(config.validate(&custom).is_err());
    }

    #[test]
    fn test_7c7_full_workflow_validation_failure_then_fix() {
        // 验证失败后启用缺失规则 → 验证通过
        let mut config = ComplianceConfig::new();
        config.enable_rule(ComplianceRule::AuditLog);

        // HIPAA 验证失败（缺少 Tde/DataMasking/EncryptedBackup/PasswordPolicy）
        assert!(!config.satisfies(&ComplianceMode::HIPAA));

        // 启用缺失规则
        for rule in config.missing_rules(&ComplianceMode::HIPAA) {
            config.enable_rule(rule);
        }

        // 现在验证通过
        assert!(config.satisfies(&ComplianceMode::HIPAA));
    }

    #[test]
    fn test_7c7_full_workflow_all_modes_satisfied() {
        // 启用全部规则 → 所有模式都满足
        let mut config = ComplianceConfig::new();
        for rule in ComplianceRule::all() {
            config.enable_rule(rule);
        }

        assert!(config.satisfies(&ComplianceMode::Standard));
        assert!(config.satisfies(&ComplianceMode::GDPR));
        assert!(config.satisfies(&ComplianceMode::HIPAA));
        assert!(config.satisfies(&ComplianceMode::PciDss));
    }
}
