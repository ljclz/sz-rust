//! SzRSQL 安全模块：TDE/审计/RLS/密码策略。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.25 节。
//!
//! # 模块
//!
//! - [`tde`] — Phase 7c.1 TDE 透明数据加密
//!   - `TdeEngine` — AES-256-CTR 页级透明加密
//!   - `MasterKey` — 32 字节主密钥管理（from_bytes/from_passphrase/generate）
//!   - 密钥轮换（rotate_key）+ 统计追踪
//! - [`audit`] — Phase 7c.3 审计日志
//!   - `AuditLog` — 不可变 append-only 审计日志存储
//!   - `AuditHashChain` — SHA-256 哈希链防篡改
//!   - `AuditFilter`/`AuditQuery` — 记录/查询过滤器
//!   - `AuditReport` — CSV/JSON 报告导出
//! - [`column_enc`] — Phase 7c.4 列级加密
//!   - `ColumnKey` — 32 字节 AES-256 密钥（from_bytes/generate/from_passphrase）
//!   - `ColumnEncryptionRegistry` — (table, column) → config 映射
//!   - `ColumnEncryptionEngine` — AES-256-GCM 认证加密/解密
//!   - 无密钥用户无法解密，只能看到密文
//! - [`masking`] — Phase 7c.5 数据脱敏
//!   - `MaskingRule` — 8 种脱敏规则（Template/Email/Phone/IdCard/CreditCard/Hash/Fixed/Custom）
//!   - `MaskingPolicy` — 策略绑定 (table, column) + 授权角色集合
//!   - `MaskingEngine` — 根据上下文动态脱敏（授权用户见原文，未授权见脱敏值）
//! - [`firewall`] — Phase 7c.6 SQL 防火墙
//!   - `SqlFirewall` — 多层 SQL 安全检查（注入检测 + 禁止命令 + 白名单）
//!   - `FirewallCommand` — 13 种 SQL 命令类型枚举
//!   - 12 种 SQL 注入特征模式（恒真条件/UNION/注释/堆叠查询/时间盲注/信息泄露/HEX/CONCAT）
//! - [`compliance`] — Phase 7c.7 合规模式配置
//!   - `ComplianceMode` — 5 种预设模式（Standard/GDPR/HIPAA/PciDss/Custom）
//!   - `ComplianceRule` — 10 种安全特性规则项（Tde/AuditLog/DataMasking/...）
//!   - `ComplianceConfig` — 累积启用规则 + 模式验证
//!   - GDPR: 审计日志 + 数据擦除 + 数据可移植性；HIPAA: TDE + 脱敏 + 加密备份；PCI-DSS: TDE + 审计 + 防火墙 + 列加密
//! - [`password_profile`] — Phase 6.33 密码策略
//!   - `PasswordProfileRegistry` — 命名 Profile 注册表
//!   - 密码复杂度/有效期/历史/锁定

#![allow(dead_code)]

pub mod audit;
pub mod audit_hash;
pub mod column_enc;
pub mod compliance;
pub mod firewall;
pub mod masking;
pub mod password_profile;
pub mod sqli_detector;
pub mod tde;

/// 返回 crate 版本号，供 workspace 骨架冒烟测试使用。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_nonempty() {
        assert!(!version().is_empty());
    }
}
