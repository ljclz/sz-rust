//! 数据脱敏（Data Masking）— Phase 7c.5
//!
//! 对应 `SzRSQL技术实现方案.md` 12.2 节 — 数据脱敏（GDPR/CCPA/HIPAA）。
//!
//! # 设计
//!
//! 数据脱敏对指定列的查询结果进行动态掩码处理，授权用户可见原文，未授权用户只见脱敏值。
//!
//! - **MaskingPolicy** — 脱敏策略（表/列/规则 + 授权角色集合）
//! - **MaskingRule** — 脱敏规则（模板/邮箱/电话/身份证/信用卡/哈希/固定字符/自定义函数）
//! - **MaskingRegistry** — 策略注册表（(table, column) → policy 映射，支持重复注册检测）
//! - **MaskingContext** — 脱敏上下文（当前用户/角色集合，判断是否授权）
//! - **MaskingEngine** — 执行脱敏（根据上下文决定是否脱敏 + 应用规则 + 统计追踪）
//!
//! ## 脱敏规则
//!
//! 1. **Template** — 模板脱敏（`***@***.com`，`*` 为通配符保留原字符）
//! 2. **Email** — 邮箱脱敏（`u***@example.com`，保留首字符 + 域名）
//! 3. **Phone** — 电话脱敏（`138****1234`，保留前 3 后 4）
//! 4. **IdCard** — 身份证脱敏（`110***********1234`，保留前 3 后 4）
//! 5. **CreditCard** — 信用卡脱敏（`4532-****-****-1234`，保留前 4 后 4）
//! 6. **Hash** — 哈希脱敏（SHA-256 前 8 位，不可逆）
//! 7. **Fixed** — 固定字符脱敏（统一替换为 `***`）
//! 8. **Custom** — 自定义函数脱敏
//!
//! ## 授权判断
//!
//! - 策略配置 `authorized_roles`：授权角色集合
//! - 上下文 `MaskingContext`：当前用户角色集合
//! - 交集非空 = 授权 = 不脱敏；交集为空 = 未授权 = 脱敏
//!
//! # 验证标准
//!
//! - `CREATE MASKING POLICY ON t.email USING '***@***.com'` → 脱敏用户查询 email → 显示脱敏值
//! - 授权用户查询 → 显示原文
//!
//! 对应 `SzRSQL实施进度.md` Phase 7c.5。

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

// =====================================================================
//  常量
// =====================================================================

/// 默认授权角色（管理员）
pub const DEFAULT_AUTHORIZED_ROLE: &str = "admin";

/// 哈希脱敏输出长度（SHA-256 前 N 位 hex）
const HASH_MASK_LEN: usize = 8;

/// 固定脱敏占位符
const DEFAULT_FIXED_MASK: &str = "***";

// =====================================================================
//  错误类型
// =====================================================================

/// 数据脱敏错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MaskingError {
    /// 策略已存在（重复注册）
    #[error("masking policy already exists: {table}.{column}")]
    PolicyAlreadyExists {
        /// 表名
        table: String,
        /// 列名
        column: String,
    },
    /// 策略不存在
    #[error("masking policy not found: {table}.{column}")]
    PolicyNotFound {
        /// 表名
        table: String,
        /// 列名
        column: String,
    },
    /// 自定义脱敏函数失败
    #[error("custom masking function failed: {0}")]
    CustomMaskFailed(String),
}

// =====================================================================
//  MaskingRule — 脱敏规则
// =====================================================================

/// 自定义脱敏函数类型（输入原文 → 输出脱敏值）
pub type CustomMaskFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// 脱敏规则
///
/// 定义如何对数据进行脱敏处理。
#[derive(Clone)]
pub enum MaskingRule {
    /// 模板脱敏 — `*` 保留原字符，其他字符替换原字符
    ///
    /// 例：`***@***.com` 对 `user@example.com` → `use@***.com`
    ///
    /// 注意：`*` 数量与原文长度对应，超出部分用 `*` 填充。
    Template {
        /// 模板字符串（`*` 为通配符）
        template: String,
    },
    /// 邮箱脱敏 — 保留首字符 + `***` + `@` + 域名
    ///
    /// 例：`user@example.com` → `u***@example.com`
    Email,
    /// 电话脱敏 — 保留前 3 后 4，中间 `*`
    ///
    /// 例：`13812345678` → `138****5678`
    Phone,
    /// 身份证脱敏 — 保留前 3 后 4，中间 `*`
    ///
    /// 例：`110101199001011234` → `110***********1234`
    IdCard,
    /// 信用卡脱敏 — 保留前 4 后 4，中间 `*`
    ///
    /// 例：`4532-0000-0000-1234` → `4532-****-****-1234`
    CreditCard,
    /// 哈希脱敏 — SHA-256 前 8 位 hex（不可逆）
    ///
    /// 例：`password123` → `ef92b0e5`
    Hash,
    /// 固定字符脱敏 — 统一替换为固定字符串
    ///
    /// 例：所有值 → `***`
    Fixed {
        /// 固定脱敏值
        mask: String,
    },
    /// 自定义函数脱敏 — 用户提供闭包
    Custom {
        /// 自定义脱敏函数
        func: CustomMaskFn,
    },
}

impl std::fmt::Debug for MaskingRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MaskingRule::Template { template } => {
                write!(f, "MaskingRule::Template({template})")
            }
            MaskingRule::Email => write!(f, "MaskingRule::Email"),
            MaskingRule::Phone => write!(f, "MaskingRule::Phone"),
            MaskingRule::IdCard => write!(f, "MaskingRule::IdCard"),
            MaskingRule::CreditCard => write!(f, "MaskingRule::CreditCard"),
            MaskingRule::Hash => write!(f, "MaskingRule::Hash"),
            MaskingRule::Fixed { mask } => write!(f, "MaskingRule::Fixed({mask})"),
            MaskingRule::Custom { .. } => write!(f, "MaskingRule::Custom(<fn>)"),
        }
    }
}

impl MaskingRule {
    /// 创建模板规则
    pub fn template(template: impl Into<String>) -> Self {
        MaskingRule::Template {
            template: template.into(),
        }
    }

    /// 创建固定字符规则（默认 `***`）
    pub fn fixed() -> Self {
        MaskingRule::Fixed {
            mask: DEFAULT_FIXED_MASK.to_string(),
        }
    }

    /// 创建固定字符规则（自定义占位符）
    pub fn fixed_with(mask: impl Into<String>) -> Self {
        MaskingRule::Fixed { mask: mask.into() }
    }

    /// 创建自定义函数规则
    pub fn custom(func: impl Fn(&str) -> String + Send + Sync + 'static) -> Self {
        MaskingRule::Custom {
            func: Arc::new(func),
        }
    }

    /// 应用脱敏规则
    ///
    /// 输入原文，返回脱敏后的值。
    pub fn apply(&self, value: &str) -> String {
        match self {
            MaskingRule::Template { template } => apply_template(template, value),
            MaskingRule::Email => mask_email(value),
            MaskingRule::Phone => mask_phone(value),
            MaskingRule::IdCard => mask_id_card(value),
            MaskingRule::CreditCard => mask_credit_card(value),
            MaskingRule::Hash => mask_hash(value),
            MaskingRule::Fixed { mask } => mask.clone(),
            MaskingRule::Custom { func } => func(value),
        }
    }
}

// =====================================================================
//  脱敏函数实现
// =====================================================================

/// 模板脱敏 — `*` 用星号替换原文字符，其他字符为字面输出
///
/// 规则：遍历模板，遇到 `*` 输出 `*` 并消耗原文一个字符（无则输出 `*`），遇到其他字符直接输出字面。
///
/// 例：`***@***.com` 对 `user@example.com` → `***@***.com`
fn apply_template(template: &str, value: &str) -> String {
    let value_chars: Vec<char> = value.chars().collect();
    let mut value_idx = 0usize;
    let mut result = String::with_capacity(template.len());

    for tch in template.chars() {
        if tch == '*' {
            // 用星号替换原文当前字符（消耗原文一个字符）
            if value_idx < value_chars.len() {
                value_idx += 1;
            }
            result.push('*');
        } else {
            result.push(tch);
        }
    }
    result
}

/// 邮箱脱敏 — 保留首字符 + `***` + `@` + 域名
///
/// 例：`user@example.com` → `u***@example.com`
/// 无 `@` 时回退为 `***`。
fn mask_email(value: &str) -> String {
    if let Some(at_pos) = value.find('@') {
        let local = &value[..at_pos];
        let domain = &value[at_pos..]; // 含 @
        if local.is_empty() {
            return format!("***{domain}");
        }
        let first = local.chars().next().unwrap();
        format!("{first}***{domain}")
    } else {
        DEFAULT_FIXED_MASK.to_string()
    }
}

/// 电话脱敏 — 保留前 3 后 4，中间 `*`
///
/// 例：`13812345678` → `138****5678`
/// 长度 <= 7 时全部脱敏为 `***`。
fn mask_phone(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 7 {
        return DEFAULT_FIXED_MASK.to_string();
    }
    let head: String = chars.iter().take(3).collect();
    let tail: String = chars.iter().skip(chars.len() - 4).collect();
    let star_count = chars.len() - 7;
    let stars: String = "*".repeat(star_count);
    format!("{head}{stars}{tail}")
}

/// 身份证脱敏 — 保留前 3 后 4，中间 `*`
///
/// 例：`110101199001011234` → `110***********1234`
fn mask_id_card(value: &str) -> String {
    mask_keep_head_tail(value, 3, 4)
}

/// 信用卡脱敏 — 保留前 4 后 4，中间 `****`
///
/// 例：`4532-0000-0000-1234` → `4532-****-****-1234`
fn mask_credit_card(value: &str) -> String {
    // 去除非数字字符后脱敏，再按 4-4-4-4 格式输出
    let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() <= 8 {
        return DEFAULT_FIXED_MASK.to_string();
    }
    let head: String = digits.chars().take(4).collect();
    let tail: String = digits.chars().skip(digits.len() - 4).collect();
    format!("{head}-****-****-{tail}")
}

/// 哈希脱敏 — SHA-256 前 8 位 hex
fn mask_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    let hex = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    hex[..HASH_MASK_LEN].to_string()
}

/// 通用脱敏：保留前 head_len + `*` + 后 tail_len
fn mask_keep_head_tail(value: &str, head_len: usize, tail_len: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= head_len + tail_len {
        return DEFAULT_FIXED_MASK.to_string();
    }
    let head: String = chars.iter().take(head_len).collect();
    let tail: String = chars.iter().skip(chars.len() - tail_len).collect();
    let star_count = chars.len() - head_len - tail_len;
    let stars: String = "*".repeat(star_count);
    format!("{head}{stars}{tail}")
}

// =====================================================================
//  MaskingPolicy — 脱敏策略
// =====================================================================

/// 脱敏策略 — 绑定到 (table, column) 的脱敏规则 + 授权角色
#[derive(Clone, Debug)]
pub struct MaskingPolicy {
    /// 策略名称
    name: String,
    /// 表名
    table: String,
    /// 列名
    column: String,
    /// 脱敏规则
    rule: MaskingRule,
    /// 授权角色集合（这些角色的用户可见原文）
    authorized_roles: Vec<String>,
}

impl MaskingPolicy {
    /// 创建脱敏策略
    pub fn new(
        name: impl Into<String>,
        table: impl Into<String>,
        column: impl Into<String>,
        rule: MaskingRule,
    ) -> Self {
        MaskingPolicy {
            name: name.into(),
            table: table.into(),
            column: column.into(),
            rule,
            authorized_roles: vec![DEFAULT_AUTHORIZED_ROLE.to_string()],
        }
    }

    /// 创建脱敏策略并指定授权角色
    pub fn with_roles(
        name: impl Into<String>,
        table: impl Into<String>,
        column: impl Into<String>,
        rule: MaskingRule,
        roles: Vec<String>,
    ) -> Self {
        MaskingPolicy {
            name: name.into(),
            table: table.into(),
            column: column.into(),
            rule,
            authorized_roles: roles,
        }
    }

    /// 获取策略名称
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 获取表名
    pub fn table(&self) -> &str {
        &self.table
    }

    /// 获取列名
    pub fn column(&self) -> &str {
        &self.column
    }

    /// 获取脱敏规则
    pub fn rule(&self) -> &MaskingRule {
        &self.rule
    }

    /// 获取授权角色集合
    pub fn authorized_roles(&self) -> &[String] {
        &self.authorized_roles
    }

    /// 判断角色是否授权
    pub fn is_authorized(&self, roles: &[String]) -> bool {
        self.authorized_roles.iter().any(|r| roles.contains(r))
    }
}

// =====================================================================
//  MaskingContext — 脱敏上下文
// =====================================================================

/// 脱敏上下文 — 当前用户的角色信息
///
/// 用于判断是否对用户脱敏：授权用户不脱敏，未授权用户脱敏。
#[derive(Clone, Debug, Default)]
pub struct MaskingContext {
    /// 当前用户名
    user: String,
    /// 当前用户角色集合
    roles: Vec<String>,
}

impl MaskingContext {
    /// 创建脱敏上下文
    pub fn new(user: impl Into<String>, roles: Vec<String>) -> Self {
        MaskingContext {
            user: user.into(),
            roles,
        }
    }

    /// 创建未授权上下文（无角色 = 普通用户）
    pub fn unauthorized(user: impl Into<String>) -> Self {
        MaskingContext {
            user: user.into(),
            roles: vec![],
        }
    }

    /// 创建管理员上下文
    pub fn admin(user: impl Into<String>) -> Self {
        MaskingContext {
            user: user.into(),
            roles: vec![DEFAULT_AUTHORIZED_ROLE.to_string()],
        }
    }

    /// 获取用户名
    pub fn user(&self) -> &str {
        &self.user
    }

    /// 获取角色集合
    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    /// 是否有指定角色
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// 是否为管理员
    pub fn is_admin(&self) -> bool {
        self.has_role(DEFAULT_AUTHORIZED_ROLE)
    }
}

// =====================================================================
//  MaskingRegistry — 脱敏策略注册表
// =====================================================================

/// 脱敏策略注册表 — (table, column) → policy 映射
#[derive(Clone, Debug, Default)]
pub struct MaskingRegistry {
    /// (table, column) → policy
    policies: HashMap<(String, String), MaskingPolicy>,
}

impl MaskingRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册脱敏策略
    ///
    /// 重复注册返回 `PolicyAlreadyExists`。
    pub fn register(&mut self, policy: MaskingPolicy) -> Result<(), MaskingError> {
        let key = (policy.table().to_string(), policy.column().to_string());
        if self.policies.contains_key(&key) {
            return Err(MaskingError::PolicyAlreadyExists {
                table: key.0,
                column: key.1,
            });
        }
        self.policies.insert(key, policy);
        Ok(())
    }

    /// 注销脱敏策略
    pub fn unregister(&mut self, table: &str, column: &str) -> Result<MaskingPolicy, MaskingError> {
        let key = (table.to_string(), column.to_string());
        self.policies
            .remove(&key)
            .ok_or(MaskingError::PolicyNotFound {
                table: key.0,
                column: key.1,
            })
    }

    /// 查询策略
    pub fn get(&self, table: &str, column: &str) -> Option<&MaskingPolicy> {
        self.policies.get(&(table.to_string(), column.to_string()))
    }

    /// 检查列是否配置脱敏
    pub fn is_masked(&self, table: &str, column: &str) -> bool {
        self.policies
            .contains_key(&(table.to_string(), column.to_string()))
    }

    /// 获取所有策略
    pub fn policies(&self) -> Vec<&MaskingPolicy> {
        self.policies.values().collect()
    }

    /// 获取指定表的所有脱敏列
    pub fn masked_columns_for_table(&self, table: &str) -> Vec<String> {
        self.policies
            .iter()
            .filter(|((t, _), _)| t == table)
            .map(|((_, c), _)| c.clone())
            .collect()
    }

    /// 策略数量
    pub fn len(&self) -> usize {
        self.policies.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }
}

// =====================================================================
//  MaskingStats — 脱敏统计
// =====================================================================

/// 脱敏统计 — 追踪脱敏操作次数
#[derive(Debug, Clone, Default)]
pub struct MaskingStats {
    /// 总查询次数
    pub total_queries: u64,
    /// 实际脱敏次数（未授权用户）
    pub masked_count: u64,
    /// 未脱敏次数（授权用户）
    pub unmasked_count: u64,
}

// =====================================================================
//  MaskingEngine — 脱敏引擎
// =====================================================================

/// 脱敏引擎 — 根据上下文对值进行脱敏
///
/// 持有策略注册表和统计信息，根据上下文判断是否脱敏。
#[derive(Debug, Default)]
pub struct MaskingEngine {
    registry: MaskingRegistry,
    stats: MaskingStats,
}

impl MaskingEngine {
    /// 创建脱敏引擎
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册脱敏策略
    pub fn register(&mut self, policy: MaskingPolicy) -> Result<(), MaskingError> {
        self.registry.register(policy)
    }

    /// 注销脱敏策略
    pub fn unregister(&mut self, table: &str, column: &str) -> Result<MaskingPolicy, MaskingError> {
        self.registry.unregister(table, column)
    }

    /// 检查列是否配置脱敏
    pub fn is_masked(&self, table: &str, column: &str) -> bool {
        self.registry.is_masked(table, column)
    }

    /// 获取策略
    pub fn policy(&self, table: &str, column: &str) -> Option<&MaskingPolicy> {
        self.registry.get(table, column)
    }

    /// 获取注册表引用
    pub fn registry(&self) -> &MaskingRegistry {
        &self.registry
    }

    /// 获取统计信息
    pub fn stats(&self) -> &MaskingStats {
        &self.stats
    }

    /// 重置统计
    pub fn reset_stats(&mut self) {
        self.stats = MaskingStats::default();
    }

    /// 对单个值进行脱敏
    ///
    /// - 若列未配置脱敏 → 返回原值
    /// - 若用户授权（角色交集非空）→ 返回原值，`unmasked_count + 1`
    /// - 若用户未授权 → 应用规则，`masked_count + 1`
    pub fn mask_value(
        &mut self,
        table: &str,
        column: &str,
        value: &str,
        ctx: &MaskingContext,
    ) -> String {
        self.stats.total_queries += 1;

        let policy = match self.registry.get(table, column) {
            Some(p) => p,
            None => return value.to_string(),
        };

        if policy.is_authorized(ctx.roles()) {
            self.stats.unmasked_count += 1;
            value.to_string()
        } else {
            self.stats.masked_count += 1;
            policy.rule().apply(value)
        }
    }

    /// 批量脱敏 — 对一行数据中的多个列同时脱敏
    ///
    /// `row` 为 (column_name, value) 列表，返回脱敏后的 (column_name, masked_value) 列表。
    pub fn mask_row(
        &mut self,
        table: &str,
        row: &[(String, String)],
        ctx: &MaskingContext,
    ) -> Vec<(String, String)> {
        let mut result = Vec::with_capacity(row.len());
        for (col, val) in row {
            let masked = self.mask_value(table, col, val, ctx);
            result.push((col.clone(), masked));
        }
        result
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  脱敏函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c5_template_basic() {
        let rule = MaskingRule::template("***@***.com");
        // `*` 用星号替换原文，其他字符字面 → `***@***.com`
        assert_eq!(rule.apply("user@example.com"), "***@***.com");
    }

    #[test]
    fn test_7c5_template_short_value() {
        let rule = MaskingRule::template("***@***.com");
        // 原文比模板的 `*` 少：`ab` → 前 2 个 `*` 消耗 `ab`，第 3 个 `*` 无原文 → `***@***.com`
        assert_eq!(rule.apply("ab"), "***@***.com");
    }

    #[test]
    fn test_7c5_template_long_value() {
        let rule = MaskingRule::template("***@*.com");
        // `abcdefgh` → 前 3 个 `*` 消耗 `abc`，`@` 字面，`*` 消耗 `d`，`.com` 字面
        assert_eq!(rule.apply("abcdefgh"), "***@*.com");
    }

    #[test]
    fn test_7c5_template_no_wildcard() {
        let rule = MaskingRule::template("MASKED");
        assert_eq!(rule.apply("anything"), "MASKED");
    }

    #[test]
    fn test_7c5_email_standard() {
        let rule = MaskingRule::Email;
        assert_eq!(rule.apply("user@example.com"), "u***@example.com");
    }

    #[test]
    fn test_7c5_email_single_char_local() {
        let rule = MaskingRule::Email;
        assert_eq!(rule.apply("a@test.com"), "a***@test.com");
    }

    #[test]
    fn test_7c5_email_no_at() {
        let rule = MaskingRule::Email;
        assert_eq!(rule.apply("notanemail"), "***");
    }

    #[test]
    fn test_7c5_email_empty_local() {
        let rule = MaskingRule::Email;
        assert_eq!(rule.apply("@example.com"), "***@example.com");
    }

    #[test]
    fn test_7c5_phone_standard() {
        let rule = MaskingRule::Phone;
        assert_eq!(rule.apply("13812345678"), "138****5678");
    }

    #[test]
    fn test_7c5_phone_too_short() {
        let rule = MaskingRule::Phone;
        assert_eq!(rule.apply("1234567"), "***");
    }

    #[test]
    fn test_7c5_phone_boundary_8_chars() {
        let rule = MaskingRule::Phone;
        // 8 位：前 3 后 4，中间 1 个 *
        assert_eq!(rule.apply("12345678"), "123*5678");
    }

    #[test]
    fn test_7c5_id_card_standard() {
        let rule = MaskingRule::IdCard;
        assert_eq!(rule.apply("110101199001011234"), "110***********1234");
    }

    #[test]
    fn test_7c5_id_card_too_short() {
        let rule = MaskingRule::IdCard;
        assert_eq!(rule.apply("1234567"), "***");
    }

    #[test]
    fn test_7c5_credit_card_standard() {
        let rule = MaskingRule::CreditCard;
        assert_eq!(rule.apply("4532-0000-0000-1234"), "4532-****-****-1234");
    }

    #[test]
    fn test_7c5_credit_card_digits_only() {
        let rule = MaskingRule::CreditCard;
        assert_eq!(rule.apply("4532000000001234"), "4532-****-****-1234");
    }

    #[test]
    fn test_7c5_credit_card_too_short() {
        let rule = MaskingRule::CreditCard;
        assert_eq!(rule.apply("1234"), "***");
    }

    #[test]
    fn test_7c5_hash_deterministic() {
        let rule = MaskingRule::Hash;
        let v1 = rule.apply("password123");
        let v2 = rule.apply("password123");
        assert_eq!(v1, v2);
        assert_eq!(v1.len(), 8);
    }

    #[test]
    fn test_7c5_hash_different_inputs() {
        let rule = MaskingRule::Hash;
        assert_ne!(rule.apply("abc"), rule.apply("def"));
    }

    #[test]
    fn test_7c5_fixed_default() {
        let rule = MaskingRule::fixed();
        assert_eq!(rule.apply("anything"), "***");
    }

    #[test]
    fn test_7c5_fixed_custom() {
        let rule = MaskingRule::fixed_with("[REDACTED]");
        assert_eq!(rule.apply("anything"), "[REDACTED]");
    }

    #[test]
    fn test_7c5_custom_function() {
        let rule = MaskingRule::custom(|s| format!("[CUSTOM:{s}]"));
        assert_eq!(rule.apply("hello"), "[CUSTOM:hello]");
    }

    #[test]
    fn test_7c5_rule_debug_formats() {
        assert_eq!(format!("{:?}", MaskingRule::Email), "MaskingRule::Email");
        assert_eq!(
            format!("{:?}", MaskingRule::template("***")),
            "MaskingRule::Template(***)"
        );
        assert_eq!(
            format!("{:?}", MaskingRule::fixed_with("X")),
            "MaskingRule::Fixed(X)"
        );
        assert_eq!(format!("{:?}", MaskingRule::Hash), "MaskingRule::Hash");
    }

    // -----------------------------------------------------------------
    //  MaskingPolicy 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c5_policy_creation() {
        let policy = MaskingPolicy::new("email_mask", "users", "email", MaskingRule::Email);
        assert_eq!(policy.name(), "email_mask");
        assert_eq!(policy.table(), "users");
        assert_eq!(policy.column(), "email");
        assert!(matches!(policy.rule(), MaskingRule::Email));
        assert_eq!(policy.authorized_roles(), ["admin"]);
    }

    #[test]
    fn test_7c5_policy_with_roles() {
        let policy = MaskingPolicy::with_roles(
            "ssn_mask",
            "users",
            "ssn",
            MaskingRule::Fixed {
                mask: "***".to_string(),
            },
            vec!["hr".to_string(), "auditor".to_string()],
        );
        assert_eq!(policy.authorized_roles(), ["hr", "auditor"]);
        assert!(policy.is_authorized(&["hr".to_string()]));
        assert!(policy.is_authorized(&["auditor".to_string()]));
        assert!(!policy.is_authorized(&["dev".to_string()]));
    }

    #[test]
    fn test_7c5_policy_default_admin_authorized() {
        let policy = MaskingPolicy::new("p", "t", "c", MaskingRule::Hash);
        assert!(policy.is_authorized(&["admin".to_string()]));
        assert!(!policy.is_authorized(&[]));
        assert!(!policy.is_authorized(&["user".to_string()]));
    }

    // -----------------------------------------------------------------
    //  MaskingContext 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c5_context_new() {
        let ctx = MaskingContext::new("alice", vec!["admin".to_string(), "dev".to_string()]);
        assert_eq!(ctx.user(), "alice");
        assert_eq!(ctx.roles(), ["admin", "dev"]);
        assert!(ctx.has_role("admin"));
        assert!(ctx.has_role("dev"));
        assert!(!ctx.has_role("hr"));
        assert!(ctx.is_admin());
    }

    #[test]
    fn test_7c5_context_unauthorized() {
        let ctx = MaskingContext::unauthorized("guest");
        assert_eq!(ctx.user(), "guest");
        assert!(ctx.roles().is_empty());
        assert!(!ctx.is_admin());
    }

    #[test]
    fn test_7c5_context_admin() {
        let ctx = MaskingContext::admin("root");
        assert_eq!(ctx.user(), "root");
        assert!(ctx.is_admin());
    }

    // -----------------------------------------------------------------
    //  MaskingRegistry 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c5_registry_register() {
        let mut reg = MaskingRegistry::new();
        let policy = MaskingPolicy::new("p1", "users", "email", MaskingRule::Email);
        assert!(reg.register(policy).is_ok());
        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
    }

    #[test]
    fn test_7c5_registry_duplicate() {
        let mut reg = MaskingRegistry::new();
        let p1 = MaskingPolicy::new("p1", "users", "email", MaskingRule::Email);
        let p2 = MaskingPolicy::new("p2", "users", "email", MaskingRule::Hash);
        reg.register(p1).unwrap();
        let err = reg.register(p2).unwrap_err();
        assert!(matches!(
            err,
            MaskingError::PolicyAlreadyExists {
                table,
                column
            } if table == "users" && column == "email"
        ));
    }

    #[test]
    fn test_7c5_registry_unregister() {
        let mut reg = MaskingRegistry::new();
        reg.register(MaskingPolicy::new(
            "p1",
            "users",
            "email",
            MaskingRule::Email,
        ))
        .unwrap();
        let removed = reg.unregister("users", "email").unwrap();
        assert_eq!(removed.name(), "p1");
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn test_7c5_registry_unregister_not_found() {
        let mut reg = MaskingRegistry::new();
        let err = reg.unregister("x", "y").unwrap_err();
        assert!(matches!(err, MaskingError::PolicyNotFound { .. }));
    }

    #[test]
    fn test_7c5_registry_get() {
        let mut reg = MaskingRegistry::new();
        reg.register(MaskingPolicy::new(
            "p1",
            "users",
            "email",
            MaskingRule::Email,
        ))
        .unwrap();
        let p = reg.get("users", "email").unwrap();
        assert_eq!(p.name(), "p1");
        assert!(reg.get("users", "phone").is_none());
    }

    #[test]
    fn test_7c5_registry_is_masked() {
        let mut reg = MaskingRegistry::new();
        assert!(!reg.is_masked("users", "email"));
        reg.register(MaskingPolicy::new(
            "p1",
            "users",
            "email",
            MaskingRule::Email,
        ))
        .unwrap();
        assert!(reg.is_masked("users", "email"));
    }

    #[test]
    fn test_7c5_registry_masked_columns_for_table() {
        let mut reg = MaskingRegistry::new();
        reg.register(MaskingPolicy::new(
            "p1",
            "users",
            "email",
            MaskingRule::Email,
        ))
        .unwrap();
        reg.register(MaskingPolicy::new(
            "p2",
            "users",
            "phone",
            MaskingRule::Phone,
        ))
        .unwrap();
        reg.register(MaskingPolicy::new(
            "p3",
            "orders",
            "card",
            MaskingRule::CreditCard,
        ))
        .unwrap();
        let mut cols = reg.masked_columns_for_table("users");
        cols.sort();
        assert_eq!(cols, ["email", "phone"]);
        assert_eq!(reg.masked_columns_for_table("orders"), ["card"]);
        assert!(reg.masked_columns_for_table("missing").is_empty());
    }

    #[test]
    fn test_7c5_registry_policies() {
        let mut reg = MaskingRegistry::new();
        reg.register(MaskingPolicy::new(
            "p1",
            "users",
            "email",
            MaskingRule::Email,
        ))
        .unwrap();
        reg.register(MaskingPolicy::new(
            "p2",
            "users",
            "phone",
            MaskingRule::Phone,
        ))
        .unwrap();
        assert_eq!(reg.policies().len(), 2);
    }

    #[test]
    fn test_7c5_registry_len_is_empty() {
        let reg = MaskingRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    // -----------------------------------------------------------------
    //  MaskingEngine 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c5_engine_creation() {
        let engine = MaskingEngine::new();
        assert!(engine.registry().is_empty());
        assert_eq!(engine.stats().total_queries, 0);
    }

    #[test]
    fn test_7c5_engine_register_policy() {
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "p1",
                "users",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();
        assert!(engine.is_masked("users", "email"));
        assert!(!engine.is_masked("users", "phone"));
    }

    #[test]
    fn test_7c5_engine_unregister_policy() {
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "p1",
                "users",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();
        engine.unregister("users", "email").unwrap();
        assert!(!engine.is_masked("users", "email"));
    }

    // -----------------------------------------------------------------
    //  脱敏执行测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c5_mask_value_unauthorized_user() {
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "p1",
                "users",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();
        let ctx = MaskingContext::unauthorized("guest");
        let masked = engine.mask_value("users", "email", "alice@example.com", &ctx);
        assert_eq!(masked, "a***@example.com");
        assert_eq!(engine.stats().masked_count, 1);
        assert_eq!(engine.stats().unmasked_count, 0);
        assert_eq!(engine.stats().total_queries, 1);
    }

    #[test]
    fn test_7c5_mask_value_authorized_admin() {
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "p1",
                "users",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();
        let ctx = MaskingContext::admin("root");
        let result = engine.mask_value("users", "email", "alice@example.com", &ctx);
        assert_eq!(result, "alice@example.com");
        assert_eq!(engine.stats().unmasked_count, 1);
        assert_eq!(engine.stats().masked_count, 0);
    }

    #[test]
    fn test_7c5_mask_value_column_not_masked() {
        let mut engine = MaskingEngine::new();
        // 未注册策略 → 返回原值
        let ctx = MaskingContext::unauthorized("guest");
        let result = engine.mask_value("users", "name", "Alice", &ctx);
        assert_eq!(result, "Alice");
    }

    #[test]
    fn test_7c5_mask_value_authorized_role() {
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::with_roles(
                "ssn_mask",
                "users",
                "ssn",
                MaskingRule::IdCard,
                vec!["hr".to_string()],
            ))
            .unwrap();
        // HR 用户可见原文
        let hr_ctx = MaskingContext::new("hr_user", vec!["hr".to_string()]);
        let result = engine.mask_value("users", "ssn", "110101199001011234", &hr_ctx);
        assert_eq!(result, "110101199001011234");
        // 普通用户脱敏
        let guest_ctx = MaskingContext::unauthorized("guest");
        let masked = engine.mask_value("users", "ssn", "110101199001011234", &guest_ctx);
        assert_eq!(masked, "110***********1234");
    }

    // -----------------------------------------------------------------
    //  批量脱敏测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c5_mask_row_unauthorized() {
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "email_mask",
                "users",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();
        engine
            .register(MaskingPolicy::new(
                "phone_mask",
                "users",
                "phone",
                MaskingRule::Phone,
            ))
            .unwrap();
        let row = vec![
            ("name".to_string(), "Alice".to_string()),
            ("email".to_string(), "alice@example.com".to_string()),
            ("phone".to_string(), "13812345678".to_string()),
        ];
        let ctx = MaskingContext::unauthorized("guest");
        let masked = engine.mask_row("users", &row, &ctx);
        assert_eq!(masked[0], ("name".to_string(), "Alice".to_string()));
        assert_eq!(
            masked[1],
            ("email".to_string(), "a***@example.com".to_string())
        );
        assert_eq!(masked[2], ("phone".to_string(), "138****5678".to_string()));
    }

    #[test]
    fn test_7c5_mask_row_admin() {
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "email_mask",
                "users",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();
        let row = vec![
            ("name".to_string(), "Alice".to_string()),
            ("email".to_string(), "alice@example.com".to_string()),
        ];
        let ctx = MaskingContext::admin("root");
        let masked = engine.mask_row("users", &row, &ctx);
        assert_eq!(masked[0].1, "Alice");
        assert_eq!(masked[1].1, "alice@example.com");
    }

    // -----------------------------------------------------------------
    //  统计测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c5_stats_tracking() {
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "email_mask",
                "users",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();
        let guest = MaskingContext::unauthorized("guest");
        let admin = MaskingContext::admin("root");

        engine.mask_value("users", "email", "a@b.com", &guest);
        engine.mask_value("users", "email", "a@b.com", &guest);
        engine.mask_value("users", "email", "a@b.com", &admin);
        engine.mask_value("users", "name", "Alice", &guest);

        assert_eq!(engine.stats().total_queries, 4);
        assert_eq!(engine.stats().masked_count, 2);
        assert_eq!(engine.stats().unmasked_count, 1);
    }

    #[test]
    fn test_7c5_stats_reset() {
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "p1",
                "users",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();
        let guest = MaskingContext::unauthorized("guest");
        engine.mask_value("users", "email", "a@b.com", &guest);
        engine.reset_stats();
        assert_eq!(engine.stats().total_queries, 0);
        assert_eq!(engine.stats().masked_count, 0);
        assert_eq!(engine.stats().unmasked_count, 0);
    }

    // -----------------------------------------------------------------
    //  验证标准核心测试 — 完整工作流
    // -----------------------------------------------------------------

    #[test]
    fn test_7c5_full_workflow_masking_policy() {
        // 验证标准：CREATE MASKING POLICY ON t.email USING '***@***.com'
        //          → 脱敏用户查询 email → 显示脱敏值
        //          → 授权用户查询 → 显示原文
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "email_mask_policy",
                "users",
                "email",
                MaskingRule::template("***@***.com"),
            ))
            .unwrap();

        // 模拟数据
        let emails = vec!["alice@example.com", "bob@test.org", "charlie@demo.net"];

        // 脱敏用户查询 → 显示脱敏值（模板 `***@***.com` 所有 `*` 替换为星号）
        let guest = MaskingContext::unauthorized("guest");
        for email in &emails {
            let masked = engine.mask_value("users", "email", email, &guest);
            assert_ne!(masked, *email);
            assert_eq!(masked, "***@***.com");
        }

        // 授权用户查询 → 显示原文
        let admin = MaskingContext::admin("admin");
        for email in &emails {
            let result = engine.mask_value("users", "email", email, &admin);
            assert_eq!(result, *email);
        }

        // 统计验证
        assert_eq!(engine.stats().total_queries, 6);
        assert_eq!(engine.stats().masked_count, 3);
        assert_eq!(engine.stats().unmasked_count, 3);
    }

    #[test]
    fn test_7c5_full_workflow_multiple_columns() {
        // 多列脱敏工作流：email + phone + ssn + card
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "email_mask",
                "customers",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();
        engine
            .register(MaskingPolicy::new(
                "phone_mask",
                "customers",
                "phone",
                MaskingRule::Phone,
            ))
            .unwrap();
        engine
            .register(MaskingPolicy::with_roles(
                "ssn_mask",
                "customers",
                "ssn",
                MaskingRule::IdCard,
                vec!["hr".to_string(), "admin".to_string()],
            ))
            .unwrap();
        engine
            .register(MaskingPolicy::new(
                "card_mask",
                "customers",
                "card",
                MaskingRule::CreditCard,
            ))
            .unwrap();

        // 100 行数据
        for i in 0..100u32 {
            let row = vec![
                ("id".to_string(), format!("{i}")),
                ("email".to_string(), format!("user{i}@example.com")),
                ("phone".to_string(), format!("138{i:08}")),
                ("ssn".to_string(), format!("110{i:015}")),
                ("card".to_string(), format!("4532-0000-{i:04}-1234")),
            ];

            // 普通用户：email/phone/ssn/card 全部脱敏，id 不脱敏
            let guest = MaskingContext::unauthorized("guest");
            let masked = engine.mask_row("customers", &row, &guest);
            assert_eq!(masked[0].1, format!("{i}")); // id 不脱敏
            assert_eq!(masked[1].1, format!("u***@example.com")); // email
            assert!(masked[2].1.starts_with("138")); // phone 前 3
            assert!(masked[3].1.starts_with("110")); // ssn 前 3
            assert!(masked[4].1.starts_with("4532-")); // card 前 4

            // HR 用户：ssn 可见原文，其他仍脱敏
            let hr = MaskingContext::new("hr_user", vec!["hr".to_string()]);
            let masked_hr = engine.mask_row("customers", &row, &hr);
            assert_eq!(masked_hr[3].1, format!("110{i:015}")); // ssn 原文
            assert_eq!(masked_hr[1].1, format!("u***@example.com")); // email 仍脱敏

            // 管理员：全部可见原文
            let admin = MaskingContext::admin("admin");
            let masked_admin = engine.mask_row("customers", &row, &admin);
            for (j, (_, val)) in row.iter().enumerate() {
                assert_eq!(masked_admin[j].1, *val);
            }
        }

        // 统计验证：100 行 × 3 用户 × 5 列 = 1500 次查询
        assert_eq!(engine.stats().total_queries, 1500);
    }

    #[test]
    fn test_7c5_masking_policy_dynamically_added() {
        // 验证策略可动态添加
        let mut engine = MaskingEngine::new();
        let ctx = MaskingContext::unauthorized("guest");

        // 初始无策略 → 原文
        assert_eq!(
            engine.mask_value("users", "email", "alice@example.com", &ctx),
            "alice@example.com"
        );

        // 添加策略 → 脱敏
        engine
            .register(MaskingPolicy::new(
                "email_mask",
                "users",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();
        assert_eq!(
            engine.mask_value("users", "email", "alice@example.com", &ctx),
            "a***@example.com"
        );

        // 注销策略 → 原文
        engine.unregister("users", "email").unwrap();
        assert_eq!(
            engine.mask_value("users", "email", "alice@example.com", &ctx),
            "alice@example.com"
        );
    }

    #[test]
    fn test_7c5_authorized_user_sees_original() {
        // 验证标准：授权用户查询 → 显示原文
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "email_mask",
                "users",
                "email",
                MaskingRule::Email,
            ))
            .unwrap();

        let original = "alice@example.com";

        // admin 可见原文
        let admin = MaskingContext::admin("admin");
        assert_eq!(
            engine.mask_value("users", "email", original, &admin),
            original
        );

        // 自定义授权角色可见原文
        let auditor = MaskingContext::new("auditor", vec!["auditor".to_string()]);
        engine
            .register(MaskingPolicy::with_roles(
                "card_mask",
                "users",
                "card",
                MaskingRule::CreditCard,
                vec!["auditor".to_string()],
            ))
            .unwrap();
        assert_eq!(
            engine.mask_value("users", "card", "4532-0000-0000-1234", &auditor),
            "4532-0000-0000-1234"
        );
    }

    #[test]
    fn test_7c5_unauthorized_user_sees_masked() {
        // 验证标准：脱敏用户查询 email → 显示脱敏值
        let mut engine = MaskingEngine::new();
        engine
            .register(MaskingPolicy::new(
                "email_mask",
                "users",
                "email",
                MaskingRule::template("***@***.com"),
            ))
            .unwrap();

        let ctx = MaskingContext::unauthorized("guest");
        // 模板 `***@***.com` → 所有 `*` 用星号替换原文
        assert_eq!(
            engine.mask_value("users", "email", "alice@example.com", &ctx),
            "***@***.com"
        );
    }
}
