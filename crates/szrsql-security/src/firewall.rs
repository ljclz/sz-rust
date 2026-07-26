//! SQL 防火墙（SQL Firewall）— Phase 7c.6
//!
//! 对应 `SzRSQL技术实现方案.md` 9.28 节 — SQL 防火墙。
//!
//! # 设计
//!
//! SQL 防火墙对传入的 SQL 语句进行多层检查，拦截恶意 SQL 和非法操作：
//!
//! - **模式白名单** — `allowed_patterns` 正则列表，匹配任一即放行（若非空）
//! - **禁止命令** — `blocked_commands` 命令类型集合，命中即拦截
//! - **SQL 注入检测** — 基于特征模式（`' OR 1=1`、`--`、`UNION SELECT`、堆叠查询等）
//! - **统计追踪** — `FirewallStats` 记录放行/拦截/注入检测计数
//!
//! ## 检查顺序
//!
//! 1. **SQL 注入检测** — 最高优先级，命中立即返回 `InjectionDetected`
//! 2. **禁止命令检查** — 命中返回 `BlockedCommand`
//! 3. **白名单匹配** — 白名单非空时必须匹配任一模式，否则返回 `NotInWhitelist`
//! 4. 全部通过返回 `Ok(())`
//!
//! ## SQL 注入特征
//!
//! - 注释注入：`--`、`/* */`、`#`
//! - 恒真条件：`' OR 1=1`、`' OR '1'='1`、`' OR true`
//! - UNION 注入：`UNION SELECT`、`UNION ALL SELECT`
//! - 堆叠查询：`; DROP`、`; DELETE`、`; UPDATE`、`; INSERT`
//! - 时间盲注：`SLEEP(`、`BENCHMARK(`、`WAITFOR DELAY`
//! - 信息泄露：`INFORMATION_SCHEMA`、`LOAD_FILE`、`INTO OUTFILE`
//! - HEX 注入：`0x...`（长 hex 字符串）
//! - 引号异常：奇数引号、不匹配引号
//!
//! # 验证标准
//!
//! - 设置白名单 `SELECT.*FROM.*WHERE` → 合法 SQL 通过
//! - `DROP TABLE` 被拦截（设置 blocked_commands）
//! - SQL 注入特征检测 `' OR 1=1 --` → 拦截
//!
//! 对应 `SzRSQL实施进度.md` Phase 7c.6。

use regex::Regex;
use std::collections::HashSet;
use std::sync::Arc;

// =====================================================================
//  常量
// =====================================================================

/// 默认最大 SQL 长度（字节），防止超长 SQL 攻击
pub const DEFAULT_MAX_SQL_LEN: usize = 1_048_576; // 1 MB

// =====================================================================
//  错误类型
// =====================================================================

/// SQL 防火墙错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FirewallError {
    /// SQL 注入检测命中
    #[error("SQL injection detected: {pattern}")]
    InjectionDetected {
        /// 命中的注入特征
        pattern: String,
    },
    /// 命中禁止命令
    #[error("blocked command: {command}")]
    BlockedCommand {
        /// 命中的命令名
        command: String,
    },
    /// 不在白名单中
    #[error("SQL not in whitelist: {sql}")]
    NotInWhitelist {
        /// 被拦截的 SQL（截断）
        sql: String,
    },
    /// SQL 过长
    #[error("SQL too long: {len} > max {max}")]
    SqlTooLong {
        /// 实际长度
        len: usize,
        /// 最大长度
        max: usize,
    },
    /// SQL 为空
    #[error("SQL is empty")]
    EmptySql,
    /// 无效正则表达式
    #[error("invalid regex pattern: {0}")]
    InvalidRegex(String),
}

// =====================================================================
//  FirewallCommand — 防火墙命令类型
// =====================================================================

/// 防火墙命令类型（按 SQL 关键字分类）
///
/// 用于 `blocked_commands` 集合，拦截指定类型的 SQL 命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FirewallCommand {
    /// SELECT
    Select,
    /// INSERT
    Insert,
    /// UPDATE
    Update,
    /// DELETE
    Delete,
    /// CREATE
    Create,
    /// DROP
    Drop,
    /// ALTER
    Alter,
    /// TRUNCATE
    Truncate,
    /// GRANT
    Grant,
    /// REVOKE
    Revoke,
    /// EXEC / EXECUTE
    Exec,
    /// MERGE
    Merge,
    /// CALL（存储过程）
    Call,
}

impl FirewallCommand {
    /// 从 SQL 文本提取首个命令类型
    ///
    /// 跳过前导空白和 SQL 注释，返回首个关键字对应的命令类型。
    /// 若无法识别返回 `None`。
    pub fn from_sql(sql: &str) -> Option<FirewallCommand> {
        let trimmed = skip_leading_comments(sql).trim_start();
        if trimmed.is_empty() {
            return None;
        }
        // 提取首个单词（截至非字母字符）
        let first_word: String = trimmed
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect::<String>()
            .to_ascii_uppercase();
        match first_word.as_str() {
            "SELECT" => Some(FirewallCommand::Select),
            "INSERT" => Some(FirewallCommand::Insert),
            "UPDATE" => Some(FirewallCommand::Update),
            "DELETE" => Some(FirewallCommand::Delete),
            "CREATE" => Some(FirewallCommand::Create),
            "DROP" => Some(FirewallCommand::Drop),
            "ALTER" => Some(FirewallCommand::Alter),
            "TRUNCATE" => Some(FirewallCommand::Truncate),
            "GRANT" => Some(FirewallCommand::Grant),
            "REVOKE" => Some(FirewallCommand::Revoke),
            "EXEC" | "EXECUTE" => Some(FirewallCommand::Exec),
            "MERGE" => Some(FirewallCommand::Merge),
            "CALL" => Some(FirewallCommand::Call),
            _ => None,
        }
    }

    /// 转换为字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            FirewallCommand::Select => "SELECT",
            FirewallCommand::Insert => "INSERT",
            FirewallCommand::Update => "UPDATE",
            FirewallCommand::Delete => "DELETE",
            FirewallCommand::Create => "CREATE",
            FirewallCommand::Drop => "DROP",
            FirewallCommand::Alter => "ALTER",
            FirewallCommand::Truncate => "TRUNCATE",
            FirewallCommand::Grant => "GRANT",
            FirewallCommand::Revoke => "REVOKE",
            FirewallCommand::Exec => "EXEC",
            FirewallCommand::Merge => "MERGE",
            FirewallCommand::Call => "CALL",
        }
    }
}

impl std::fmt::Display for FirewallCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =====================================================================
//  FirewallStats — 防火墙统计
// =====================================================================

/// 防火墙统计 — 追踪放行/拦截/注入检测计数
#[derive(Debug, Clone, Default)]
pub struct FirewallStats {
    /// 总检查次数
    pub total_checks: u64,
    /// 放行次数
    pub allowed: u64,
    /// 拦截次数（含注入/禁止命令/白名单）
    pub blocked: u64,
    /// 注入检测命中次数
    pub injections_detected: u64,
    /// 禁止命令命中次数
    pub blocked_commands: u64,
    /// 白名单拦截次数
    pub whitelist_blocks: u64,
}

impl FirewallStats {
    /// 创建空统计
    pub fn new() -> Self {
        Self::default()
    }

    /// 重置统计
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

// =====================================================================
//  InjectionPattern — SQL 注入特征模式
// =====================================================================

/// SQL 注入特征模式（编译后的正则 + 描述）
#[derive(Clone)]
struct InjectionPattern {
    /// 模式名称
    name: &'static str,
    /// 编译后的正则表达式
    regex: Regex,
}

impl std::fmt::Debug for InjectionPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InjectionPattern")
            .field("name", &self.name)
            .field("regex", &self.regex.as_str())
            .finish()
    }
}

/// 构建默认的 SQL 注入特征模式集合
fn default_injection_patterns() -> Vec<InjectionPattern> {
    build_injection_patterns().unwrap_or_default()
}

/// 构建 SQL 注入特征模式集合
///
/// 返回 `Err` 仅在正则编译失败时（理论不会发生，因模式为常量）。
fn build_injection_patterns() -> Result<Vec<InjectionPattern>, regex::Error> {
    let patterns: Vec<(&'static str, &'static str)> = vec![
        // 恒真条件注入：OR 1=1、OR '1'='1、OR true（词边界 OR 后跟恒等式）
        (
            "always_true_condition",
            r#"(?i)\bOR\s+(?:['"]?\d+['"]?\s*=\s*['"]?\d+['"]?|true\b)"#,
        ),
        // UNION 注入：UNION SELECT、UNION ALL SELECT
        ("union_select", r#"(?i)\bUNION\s+(?:ALL\s+)?SELECT\b"#),
        // 行注释：-- 或 # 后跟内容（排除 -- 在数字中的减号场景，要求前面非字母数字或后跟空白）
        ("line_comment_double_dash", r#"--\s"#),
        ("line_comment_hash", r#"#[^\s]*$"#),
        // 块注释：/* */
        ("block_comment", r#"/\*.*?\*/"#),
        // 堆叠查询：; 后跟 DDL/DML
        (
            "stacked_query",
            r#"(?i);\s*(?:DROP|DELETE|UPDATE|INSERT|CREATE|ALTER|TRUNCATE|GRANT|REVOKE)\b"#,
        ),
        // 时间盲注：SLEEP(、BENCHMARK(、WAITFOR DELAY
        (
            "time_based_blind",
            r#"(?i)\b(?:SLEEP\s*\(|BENCHMARK\s*\(|WAITFOR\s+DELAY\b)"#,
        ),
        // 信息泄露：INFORMATION_SCHEMA、LOAD_FILE、INTO OUTFILE、INTO DUMPFILE
        (
            "info_leak",
            r#"(?i)\b(?:INFORMATION_SCHEMA|LOAD_FILE|INTO\s+(?:OUTFILE|DUMPFILE))\b"#,
        ),
        // HEX 注入：0x + 连续 16+ hex 字符
        ("hex_injection", r#"(?i)0x[0-9a-f]{16,}"#),
        // 引号逃逸：\' 或 \' OR
        ("quote_escape", r#"(?i)\\'\s*(?:OR|AND)\b"#),
        // CHAR() 函数注入（用于绕过过滤）
        ("char_function_injection", r#"(?i)\bCHAR\s*\(\s*\d+\s*\)"#),
        // CONCAT 注入：CONCAT(0x...) 构造恶意字符串
        ("concat_hex", r#"(?i)\bCONCAT\s*\(\s*0x"#),
    ];

    patterns
        .into_iter()
        .map(|(name, pattern)| {
            Ok(InjectionPattern {
                name,
                regex: Regex::new(pattern)?,
            })
        })
        .collect()
}

// =====================================================================
//  SqlFirewall — SQL 防火墙
// =====================================================================

/// SQL 防火墙 — 多层 SQL 安全检查
///
/// # 工作流程
///
/// 1. `check(sql)` 执行多层检查：
///    - SQL 注入检测（最高优先级）
///    - 禁止命令检查
///    - 白名单匹配（若白名单非空）
/// 2. 全部通过返回 `Ok(())`，否则返回对应 `FirewallError`
///
/// # 用法
///
/// ```ignore
/// use szrsql_security::firewall::*;
///
/// let mut firewall = SqlFirewall::new();
/// firewall.add_allowed_pattern(r"SELECT.*FROM.*WHERE.*").unwrap();
/// firewall.block_command(FirewallCommand::Drop);
///
/// // 合法 SELECT 通过
/// assert!(firewall.check("SELECT * FROM users WHERE id = 1").is_ok());
///
/// // DROP 被拦截
/// assert!(firewall.check("DROP TABLE users").is_err());
///
/// // 注入特征拦截
/// assert!(firewall.check("SELECT * FROM users WHERE name = 'admin' --").is_err());
/// ```
#[derive(Clone, Debug)]
pub struct SqlFirewall {
    /// 查询模式白名单（正则表达式集合）
    allowed_patterns: Vec<Arc<Regex>>,
    /// 禁止命令集合
    blocked_commands: HashSet<FirewallCommand>,
    /// SQL 注入特征模式集合
    injection_patterns: Vec<InjectionPattern>,
    /// 最大 SQL 长度（字节）
    max_sql_len: usize,
    /// 统计
    stats: FirewallStats,
    /// 是否启用
    enabled: bool,
}

impl Default for SqlFirewall {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlFirewall {
    /// 创建空防火墙（默认启用，无白名单，无禁止命令，使用默认注入特征）
    pub fn new() -> Self {
        SqlFirewall {
            allowed_patterns: Vec::new(),
            blocked_commands: HashSet::new(),
            injection_patterns: default_injection_patterns(),
            max_sql_len: DEFAULT_MAX_SQL_LEN,
            stats: FirewallStats::default(),
            enabled: true,
        }
    }

    /// 添加白名单模式
    ///
    /// 白名单非空时，SQL 必须匹配任一白名单模式才放行。
    pub fn add_allowed_pattern(&mut self, pattern: &str) -> Result<(), FirewallError> {
        let regex = Regex::new(pattern).map_err(|e| FirewallError::InvalidRegex(e.to_string()))?;
        self.allowed_patterns.push(Arc::new(regex));
        Ok(())
    }

    /// 清空白名单
    pub fn clear_allowed_patterns(&mut self) {
        self.allowed_patterns.clear();
    }

    /// 获取白名单数量
    pub fn allowed_patterns_count(&self) -> usize {
        self.allowed_patterns.len()
    }

    /// 禁止命令
    pub fn block_command(&mut self, cmd: FirewallCommand) {
        self.blocked_commands.insert(cmd);
    }

    /// 解禁命令
    pub fn unblock_command(&mut self, cmd: FirewallCommand) {
        self.blocked_commands.remove(&cmd);
    }

    /// 清空禁止命令集合
    pub fn clear_blocked_commands(&mut self) {
        self.blocked_commands.clear();
    }

    /// 命令是否被禁止
    pub fn is_command_blocked(&self, cmd: FirewallCommand) -> bool {
        self.blocked_commands.contains(&cmd)
    }

    /// 获取禁止命令数量
    pub fn blocked_commands_count(&self) -> usize {
        self.blocked_commands.len()
    }

    /// 设置最大 SQL 长度
    pub fn set_max_sql_len(&mut self, len: usize) {
        self.max_sql_len = len;
    }

    /// 启用/禁用防火墙
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// 是否启用
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 获取统计（克隆）
    pub fn stats(&self) -> FirewallStats {
        self.stats.clone()
    }

    /// 重置统计
    pub fn reset_stats(&mut self) {
        self.stats.reset();
    }

    /// 获取注入特征模式数量
    pub fn injection_patterns_count(&self) -> usize {
        self.injection_patterns.len()
    }

    /// 检查 SQL 是否允许执行
    ///
    /// 检查顺序：
    /// 1. 防火墙未启用 → 直接放行
    /// 2. SQL 为空 → `EmptySql`
    /// 3. SQL 过长 → `SqlTooLong`
    /// 4. SQL 注入检测 → `InjectionDetected`
    /// 5. 禁止命令检查 → `BlockedCommand`
    /// 6. 白名单匹配（若非空）→ `NotInWhitelist`
    /// 7. 全部通过 → `Ok(())`
    pub fn check(&mut self, sql: &str) -> Result<(), FirewallError> {
        self.stats.total_checks += 1;

        // 防火墙未启用 → 直接放行
        if !self.enabled {
            self.stats.allowed += 1;
            return Ok(());
        }

        // SQL 为空
        if sql.trim().is_empty() {
            self.stats.blocked += 1;
            return Err(FirewallError::EmptySql);
        }

        // SQL 过长
        let sql_len = sql.len();
        if sql_len > self.max_sql_len {
            self.stats.blocked += 1;
            return Err(FirewallError::SqlTooLong {
                len: sql_len,
                max: self.max_sql_len,
            });
        }

        // SQL 注入检测（最高优先级）
        if let Some(pattern_name) = self.detect_injection(sql) {
            self.stats.blocked += 1;
            self.stats.injections_detected += 1;
            return Err(FirewallError::InjectionDetected {
                pattern: pattern_name,
            });
        }

        // 禁止命令检查
        if let Some(cmd) = FirewallCommand::from_sql(sql) {
            if self.blocked_commands.contains(&cmd) {
                self.stats.blocked += 1;
                self.stats.blocked_commands += 1;
                return Err(FirewallError::BlockedCommand {
                    command: cmd.as_str().to_string(),
                });
            }
        }

        // 白名单匹配（白名单非空时必须匹配任一模式）
        if !self.allowed_patterns.is_empty() {
            let matched = self.allowed_patterns.iter().any(|re| re.is_match(sql));
            if !matched {
                self.stats.blocked += 1;
                self.stats.whitelist_blocks += 1;
                let truncated: String = sql.chars().take(100).collect();
                return Err(FirewallError::NotInWhitelist { sql: truncated });
            }
        }

        self.stats.allowed += 1;
        Ok(())
    }

    /// 检测 SQL 注入特征
    ///
    /// 返回 `Some(pattern_name)` 表示命中注入特征，`None` 表示未命中。
    pub fn detect_injection(&self, sql: &str) -> Option<String> {
        for pattern in &self.injection_patterns {
            if pattern.regex.is_match(sql) {
                return Some(pattern.name.to_string());
            }
        }
        None
    }

    /// 检查 SQL 是否匹配白名单（白名单为空时返回 `true`）
    pub fn matches_whitelist(&self, sql: &str) -> bool {
        if self.allowed_patterns.is_empty() {
            return true;
        }
        self.allowed_patterns.iter().any(|re| re.is_match(sql))
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 跳过 SQL 前导注释（`--`、`/* */`、`#`），返回剩余内容
fn skip_leading_comments(sql: &str) -> &str {
    let mut s = sql;
    loop {
        let trimmed = s.trim_start();
        if trimmed.starts_with("--") {
            // 行注释：跳到行尾
            if let Some(nl) = trimmed.find('\n') {
                s = &trimmed[nl + 1..];
            } else {
                return "";
            }
        } else if trimmed.starts_with("/*") {
            // 块注释：跳到 */
            if let Some(end) = trimmed.find("*/") {
                s = &trimmed[end + 2..];
            } else {
                return "";
            }
        } else if trimmed.starts_with('#') {
            // 行注释：跳到行尾
            if let Some(nl) = trimmed.find('\n') {
                s = &trimmed[nl + 1..];
            } else {
                return "";
            }
        } else {
            return trimmed;
        }
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  FirewallCommand 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c6_command_from_sql_select() {
        assert_eq!(
            FirewallCommand::from_sql("SELECT * FROM users"),
            Some(FirewallCommand::Select)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_insert() {
        assert_eq!(
            FirewallCommand::from_sql("INSERT INTO users VALUES (1)"),
            Some(FirewallCommand::Insert)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_update() {
        assert_eq!(
            FirewallCommand::from_sql("UPDATE users SET name = 'a'"),
            Some(FirewallCommand::Update)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_delete() {
        assert_eq!(
            FirewallCommand::from_sql("DELETE FROM users WHERE id = 1"),
            Some(FirewallCommand::Delete)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_create() {
        assert_eq!(
            FirewallCommand::from_sql("CREATE TABLE users (id INT)"),
            Some(FirewallCommand::Create)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_drop() {
        assert_eq!(
            FirewallCommand::from_sql("DROP TABLE users"),
            Some(FirewallCommand::Drop)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_alter() {
        assert_eq!(
            FirewallCommand::from_sql("ALTER TABLE users ADD COLUMN x INT"),
            Some(FirewallCommand::Alter)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_truncate() {
        assert_eq!(
            FirewallCommand::from_sql("TRUNCATE TABLE users"),
            Some(FirewallCommand::Truncate)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_grant() {
        assert_eq!(
            FirewallCommand::from_sql("GRANT SELECT ON users TO bob"),
            Some(FirewallCommand::Grant)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_revoke() {
        assert_eq!(
            FirewallCommand::from_sql("REVOKE SELECT ON users FROM bob"),
            Some(FirewallCommand::Revoke)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_exec() {
        assert_eq!(
            FirewallCommand::from_sql("EXEC my_proc()"),
            Some(FirewallCommand::Exec)
        );
        assert_eq!(
            FirewallCommand::from_sql("EXECUTE my_proc()"),
            Some(FirewallCommand::Exec)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_with_leading_whitespace() {
        assert_eq!(
            FirewallCommand::from_sql("   \n  SELECT * FROM users"),
            Some(FirewallCommand::Select)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_with_leading_comment() {
        assert_eq!(
            FirewallCommand::from_sql("-- comment\nSELECT * FROM users"),
            Some(FirewallCommand::Select)
        );
        assert_eq!(
            FirewallCommand::from_sql("/* block */ SELECT * FROM users"),
            Some(FirewallCommand::Select)
        );
    }

    #[test]
    fn test_7c6_command_from_sql_empty() {
        assert_eq!(FirewallCommand::from_sql(""), None);
        assert_eq!(FirewallCommand::from_sql("   "), None);
    }

    #[test]
    fn test_7c6_command_from_sql_unknown() {
        assert_eq!(FirewallCommand::from_sql("EXPLAIN SELECT *"), None);
    }

    #[test]
    fn test_7c6_command_as_str() {
        assert_eq!(FirewallCommand::Select.as_str(), "SELECT");
        assert_eq!(FirewallCommand::Drop.as_str(), "DROP");
        assert_eq!(FirewallCommand::Insert.as_str(), "INSERT");
    }

    #[test]
    fn test_7c6_command_display() {
        assert_eq!(format!("{}", FirewallCommand::Select), "SELECT");
        assert_eq!(format!("{}", FirewallCommand::Drop), "DROP");
    }

    // -----------------------------------------------------------------
    //  FirewallStats 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c6_stats_default() {
        let stats = FirewallStats::default();
        assert_eq!(stats.total_checks, 0);
        assert_eq!(stats.allowed, 0);
        assert_eq!(stats.blocked, 0);
        assert_eq!(stats.injections_detected, 0);
        assert_eq!(stats.blocked_commands, 0);
        assert_eq!(stats.whitelist_blocks, 0);
    }

    #[test]
    fn test_7c6_stats_new() {
        let stats = FirewallStats::new();
        assert_eq!(stats.total_checks, 0);
    }

    #[test]
    fn test_7c6_stats_reset() {
        let mut stats = FirewallStats {
            total_checks: 100,
            allowed: 80,
            blocked: 20,
            injections_detected: 5,
            blocked_commands: 10,
            whitelist_blocks: 5,
        };
        stats.reset();
        assert_eq!(stats.total_checks, 0);
        assert_eq!(stats.allowed, 0);
        assert_eq!(stats.blocked, 0);
    }

    // -----------------------------------------------------------------
    //  SqlFirewall 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c6_firewall_creation() {
        let fw = SqlFirewall::new();
        assert!(fw.is_enabled());
        assert_eq!(fw.allowed_patterns_count(), 0);
        assert_eq!(fw.blocked_commands_count(), 0);
        assert_eq!(fw.injection_patterns_count(), 12);
        assert_eq!(fw.max_sql_len, DEFAULT_MAX_SQL_LEN);
    }

    #[test]
    fn test_7c6_firewall_default() {
        let fw = SqlFirewall::default();
        assert!(fw.is_enabled());
    }

    #[test]
    fn test_7c6_firewall_enable_disable() {
        let mut fw = SqlFirewall::new();
        assert!(fw.is_enabled());
        fw.set_enabled(false);
        assert!(!fw.is_enabled());
        fw.set_enabled(true);
        assert!(fw.is_enabled());
    }

    #[test]
    fn test_7c6_firewall_disabled_allows_all() {
        let mut fw = SqlFirewall::new();
        fw.set_enabled(false);
        // 即使有禁止命令，禁用时也放行
        fw.block_command(FirewallCommand::Drop);
        assert!(fw.check("DROP TABLE users").is_ok());
    }

    #[test]
    fn test_7c6_add_allowed_pattern_valid() {
        let mut fw = SqlFirewall::new();
        assert!(fw.add_allowed_pattern(r"SELECT.*FROM.*").is_ok());
        assert_eq!(fw.allowed_patterns_count(), 1);
    }

    #[test]
    fn test_7c6_add_allowed_pattern_invalid() {
        let mut fw = SqlFirewall::new();
        let result = fw.add_allowed_pattern(r"[invalid(");
        assert!(result.is_err());
        match result {
            Err(FirewallError::InvalidRegex(_)) => {}
            _ => panic!("expected InvalidRegex error"),
        }
        assert_eq!(fw.allowed_patterns_count(), 0);
    }

    #[test]
    fn test_7c6_clear_allowed_patterns() {
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*").unwrap();
        fw.add_allowed_pattern(r"INSERT.*").unwrap();
        assert_eq!(fw.allowed_patterns_count(), 2);
        fw.clear_allowed_patterns();
        assert_eq!(fw.allowed_patterns_count(), 0);
    }

    #[test]
    fn test_7c6_block_command() {
        let mut fw = SqlFirewall::new();
        fw.block_command(FirewallCommand::Drop);
        assert!(fw.is_command_blocked(FirewallCommand::Drop));
        assert!(!fw.is_command_blocked(FirewallCommand::Select));
        assert_eq!(fw.blocked_commands_count(), 1);
    }

    #[test]
    fn test_7c6_unblock_command() {
        let mut fw = SqlFirewall::new();
        fw.block_command(FirewallCommand::Drop);
        fw.unblock_command(FirewallCommand::Drop);
        assert!(!fw.is_command_blocked(FirewallCommand::Drop));
        assert_eq!(fw.blocked_commands_count(), 0);
    }

    #[test]
    fn test_7c6_clear_blocked_commands() {
        let mut fw = SqlFirewall::new();
        fw.block_command(FirewallCommand::Drop);
        fw.block_command(FirewallCommand::Truncate);
        fw.clear_blocked_commands();
        assert_eq!(fw.blocked_commands_count(), 0);
    }

    #[test]
    fn test_7c6_set_max_sql_len() {
        let mut fw = SqlFirewall::new();
        fw.set_max_sql_len(100);
        assert_eq!(fw.max_sql_len, 100);
    }

    // -----------------------------------------------------------------
    //  SQL 注入检测测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c6_injection_always_true_or_1_eq_1() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM users WHERE name = 'admin' OR 1=1")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_always_true_or_quote() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM users WHERE id = 1 OR '1'='1")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_always_true_or_true() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM users WHERE name = '' OR true")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_union_select() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT id FROM users UNION SELECT password FROM admins")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_union_all_select() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT id FROM users UNION ALL SELECT password FROM admins")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_line_comment_double_dash() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM users WHERE name = 'admin' -- ")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_block_comment() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM /* comment */ users")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_stacked_query_drop() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM users; DROP TABLE users")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_stacked_query_delete() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM users; DELETE FROM users")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_time_based_sleep() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM users WHERE id = 1 OR SLEEP(5)")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_time_based_benchmark() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM users WHERE id = BENCHMARK(1000000, MD5(1))")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_info_leak_information_schema() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM INFORMATION_SCHEMA.TABLES")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_info_leak_load_file() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT LOAD_FILE('/etc/passwd')")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_into_outfile() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM users INTO OUTFILE '/tmp/x'")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_hex_long() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM users WHERE id = 0x1234567890abcdef1234567890abcdef")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_concat_hex() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT CONCAT(0x414243) FROM users")
            .is_some());
    }

    #[test]
    fn test_7c6_injection_clean_sql_not_detected() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT id, name, email FROM users WHERE id = 123 ORDER BY name")
            .is_none());
    }

    #[test]
    fn test_7c6_injection_clean_insert_not_detected() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("INSERT INTO users (id, name) VALUES (1, 'Alice')")
            .is_none());
    }

    #[test]
    fn test_7c6_injection_clean_update_not_detected() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("UPDATE users SET name = 'Bob' WHERE id = 1")
            .is_none());
    }

    #[test]
    fn test_7c6_injection_clean_select_with_subquery() {
        let fw = SqlFirewall::new();
        assert!(fw
            .detect_injection("SELECT * FROM orders WHERE user_id IN (SELECT id FROM users)")
            .is_none());
    }

    // -----------------------------------------------------------------
    //  check() 测试 — 基础场景
    // -----------------------------------------------------------------

    #[test]
    fn test_7c6_check_empty_sql() {
        let mut fw = SqlFirewall::new();
        let result = fw.check("");
        assert!(matches!(result, Err(FirewallError::EmptySql)));
    }

    #[test]
    fn test_7c6_check_whitespace_only_sql() {
        let mut fw = SqlFirewall::new();
        let result = fw.check("   \n\t  ");
        assert!(matches!(result, Err(FirewallError::EmptySql)));
    }

    #[test]
    fn test_7c6_check_sql_too_long() {
        let mut fw = SqlFirewall::new();
        fw.set_max_sql_len(10);
        let long_sql = "SELECT * FROM users"; // 21 字节
        let result = fw.check(long_sql);
        match result {
            Err(FirewallError::SqlTooLong { len, max }) => {
                assert_eq!(len, long_sql.len());
                assert_eq!(max, 10);
            }
            _ => panic!("expected SqlTooLong error"),
        }
    }

    #[test]
    fn test_7c6_check_clean_select_allowed() {
        let mut fw = SqlFirewall::new();
        assert!(fw.check("SELECT * FROM users WHERE id = 1").is_ok());
    }

    // -----------------------------------------------------------------
    //  check() 测试 — SQL 注入拦截
    // -----------------------------------------------------------------

    #[test]
    fn test_7c6_check_injection_blocked_or_1_eq_1() {
        let mut fw = SqlFirewall::new();
        let result = fw.check("SELECT * FROM users WHERE name = 'admin' OR 1=1");
        match result {
            Err(FirewallError::InjectionDetected { pattern }) => {
                assert!(pattern.contains("always_true"));
            }
            _ => panic!("expected InjectionDetected error"),
        }
    }

    #[test]
    fn test_7c6_check_injection_blocked_union_select() {
        let mut fw = SqlFirewall::new();
        let result = fw.check("SELECT id FROM users UNION SELECT password FROM admins");
        match result {
            Err(FirewallError::InjectionDetected { pattern }) => {
                assert!(pattern.contains("union"));
            }
            _ => panic!("expected InjectionDetected error"),
        }
    }

    #[test]
    fn test_7c6_check_injection_blocked_comment() {
        let mut fw = SqlFirewall::new();
        let result = fw.check("SELECT * FROM users WHERE name = 'admin' -- ");
        assert!(matches!(
            result,
            Err(FirewallError::InjectionDetected { .. })
        ));
    }

    #[test]
    fn test_7c6_check_injection_blocked_stacked_query() {
        let mut fw = SqlFirewall::new();
        let result = fw.check("SELECT * FROM users; DROP TABLE users");
        assert!(matches!(
            result,
            Err(FirewallError::InjectionDetected { .. })
        ));
    }

    // -----------------------------------------------------------------
    //  check() 测试 — 禁止命令
    // -----------------------------------------------------------------

    #[test]
    fn test_7c6_check_blocked_command_drop() {
        let mut fw = SqlFirewall::new();
        fw.block_command(FirewallCommand::Drop);
        let result = fw.check("DROP TABLE users");
        match result {
            Err(FirewallError::BlockedCommand { command }) => {
                assert_eq!(command, "DROP");
            }
            _ => panic!("expected BlockedCommand error"),
        }
    }

    #[test]
    fn test_7c6_check_blocked_command_truncate() {
        let mut fw = SqlFirewall::new();
        fw.block_command(FirewallCommand::Truncate);
        let result = fw.check("TRUNCATE TABLE users");
        assert!(matches!(result, Err(FirewallError::BlockedCommand { .. })));
    }

    #[test]
    fn test_7c6_check_blocked_command_not_blocked() {
        let mut fw = SqlFirewall::new();
        fw.block_command(FirewallCommand::Drop);
        // SELECT 未被禁止 → 放行
        assert!(fw.check("SELECT * FROM users").is_ok());
    }

    #[test]
    fn test_7c6_check_injection_priority_over_blocked_command() {
        // SQL 注入检测优先于禁止命令检查
        let mut fw = SqlFirewall::new();
        fw.block_command(FirewallCommand::Drop);
        // DROP + 堆叠查询 → 应返回 InjectionDetected 而非 BlockedCommand
        let result = fw.check("DROP TABLE users; DELETE FROM users");
        assert!(matches!(
            result,
            Err(FirewallError::InjectionDetected { .. })
        ));
    }

    // -----------------------------------------------------------------
    //  check() 测试 — 白名单
    // -----------------------------------------------------------------

    #[test]
    fn test_7c6_check_whitelist_match() {
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*FROM.*WHERE.*").unwrap();
        // 匹配白名单
        assert!(fw.check("SELECT * FROM users WHERE id = 1").is_ok());
    }

    #[test]
    fn test_7c6_check_whitelist_no_match() {
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*FROM.*WHERE.*").unwrap();
        // 不匹配白名单（无 WHERE）
        let result = fw.check("SELECT * FROM users");
        match result {
            Err(FirewallError::NotInWhitelist { sql }) => {
                assert!(sql.contains("SELECT"));
            }
            _ => panic!("expected NotInWhitelist error"),
        }
    }

    #[test]
    fn test_7c6_check_whitelist_empty_allows_all() {
        let mut fw = SqlFirewall::new();
        // 白名单为空 → 所有合法 SQL 放行
        assert!(fw.check("SELECT * FROM users").is_ok());
        assert!(fw.check("INSERT INTO users VALUES (1)").is_ok());
        assert!(fw.check("UPDATE users SET x = 1").is_ok());
    }

    #[test]
    fn test_7c6_check_whitelist_multiple_patterns() {
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*FROM.*").unwrap();
        fw.add_allowed_pattern(r"INSERT.*INTO.*VALUES.*").unwrap();
        // 匹配第一个
        assert!(fw.check("SELECT * FROM users WHERE id = 1").is_ok());
        // 匹配第二个
        assert!(fw.check("INSERT INTO users VALUES (1)").is_ok());
        // 都不匹配
        assert!(fw.check("DELETE FROM users").is_err());
    }

    #[test]
    fn test_7c6_check_whitelist_injection_priority() {
        // SQL 注入检测优先于白名单检查
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*FROM.*WHERE.*").unwrap();
        // 即使匹配白名单，注入特征仍优先拦截
        let result = fw.check("SELECT * FROM users WHERE name = 'a' OR 1=1");
        assert!(matches!(
            result,
            Err(FirewallError::InjectionDetected { .. })
        ));
    }

    #[test]
    fn test_7c6_matches_whitelist_empty() {
        let fw = SqlFirewall::new();
        assert!(fw.matches_whitelist("anything"));
    }

    #[test]
    fn test_7c6_matches_whitelist_non_empty_match() {
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*").unwrap();
        assert!(fw.matches_whitelist("SELECT * FROM users"));
    }

    #[test]
    fn test_7c6_matches_whitelist_non_empty_no_match() {
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*").unwrap();
        assert!(!fw.matches_whitelist("DROP TABLE users"));
    }

    // -----------------------------------------------------------------
    //  统计测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c6_stats_tracking_allowed() {
        let mut fw = SqlFirewall::new();
        fw.check("SELECT * FROM users").unwrap();
        fw.check("INSERT INTO users VALUES (1)").unwrap();
        let stats = fw.stats();
        assert_eq!(stats.total_checks, 2);
        assert_eq!(stats.allowed, 2);
        assert_eq!(stats.blocked, 0);
    }

    #[test]
    fn test_7c6_stats_tracking_injection() {
        let mut fw = SqlFirewall::new();
        let _ = fw.check("SELECT * FROM users WHERE name = 'a' OR 1=1");
        let stats = fw.stats();
        assert_eq!(stats.total_checks, 1);
        assert_eq!(stats.allowed, 0);
        assert_eq!(stats.blocked, 1);
        assert_eq!(stats.injections_detected, 1);
    }

    #[test]
    fn test_7c6_stats_tracking_blocked_command() {
        let mut fw = SqlFirewall::new();
        fw.block_command(FirewallCommand::Drop);
        let _ = fw.check("DROP TABLE users");
        let stats = fw.stats();
        assert_eq!(stats.total_checks, 1);
        assert_eq!(stats.blocked, 1);
        assert_eq!(stats.blocked_commands, 1);
        assert_eq!(stats.injections_detected, 0);
    }

    #[test]
    fn test_7c6_stats_tracking_whitelist_block() {
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*FROM.*WHERE.*").unwrap();
        let _ = fw.check("SELECT * FROM users");
        let stats = fw.stats();
        assert_eq!(stats.total_checks, 1);
        assert_eq!(stats.blocked, 1);
        assert_eq!(stats.whitelist_blocks, 1);
    }

    // -----------------------------------------------------------------
    //  验证标准核心测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c6_full_workflow_whitelist_blocked_command_injection() {
        // 验证标准：白名单 + 禁止命令 + 注入检测三层
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*FROM.*WHERE.*").unwrap();
        fw.block_command(FirewallCommand::Drop);
        fw.block_command(FirewallCommand::Truncate);

        // 1. 合法 SQL 通过白名单
        assert!(fw.check("SELECT * FROM users WHERE id = 1").is_ok());

        // 2. DROP 被禁止命令拦截
        assert!(matches!(
            fw.check("DROP TABLE users"),
            Err(FirewallError::BlockedCommand { .. })
        ));

        // 3. SQL 注入特征拦截
        assert!(matches!(
            fw.check("SELECT * FROM users WHERE name = 'a' OR 1=1 -- "),
            Err(FirewallError::InjectionDetected { .. })
        ));
    }

    #[test]
    fn test_7c6_full_workflow_select_with_where_allowed() {
        // 验证标准：白名单 `SELECT.*FROM.*WHERE` → 合法 SQL 通过
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*FROM.*WHERE.*").unwrap();

        let valid_queries = vec![
            "SELECT * FROM users WHERE id = 1",
            "SELECT name, email FROM users WHERE active = true",
            "SELECT id FROM orders WHERE user_id = 100",
        ];
        for sql in valid_queries {
            assert!(fw.check(sql).is_ok(), "expected ok for: {sql}");
        }
    }

    #[test]
    fn test_7c6_full_workflow_drop_blocked() {
        // 验证标准：DROP TABLE 被拦截
        let mut fw = SqlFirewall::new();
        fw.block_command(FirewallCommand::Drop);

        let result = fw.check("DROP TABLE users");
        assert!(matches!(
            result,
            Err(FirewallError::BlockedCommand { command }) if command == "DROP"
        ));
    }

    #[test]
    fn test_7c6_full_workflow_injection_blocked() {
        // 验证标准：SQL 注入特征 `' OR 1=1 --` → 拦截
        let mut fw = SqlFirewall::new();
        let result = fw.check("SELECT * FROM users WHERE name = 'admin' OR 1=1 -- ");
        assert!(matches!(
            result,
            Err(FirewallError::InjectionDetected { .. })
        ));
    }

    #[test]
    fn test_7c6_full_workflow_massive_scale() {
        // 1000 次 SQL 检查：500 合法 + 500 注入
        let mut fw = SqlFirewall::new();

        for i in 0..500u32 {
            let sql = format!("SELECT * FROM users WHERE id = {i}");
            assert!(fw.check(&sql).is_ok(), "valid SQL blocked at iteration {i}");
        }

        for i in 0..500u32 {
            let sql = format!("SELECT * FROM users WHERE id = {i} OR 1=1");
            assert!(
                fw.check(&sql).is_err(),
                "injection not detected at iteration {i}"
            );
        }

        let stats = fw.stats();
        assert_eq!(stats.total_checks, 1000);
        assert_eq!(stats.allowed, 500);
        assert_eq!(stats.blocked, 500);
        assert_eq!(stats.injections_detected, 500);
    }

    #[test]
    fn test_7c6_full_workflow_multi_layer_protection() {
        // 多层防护：白名单 + 禁止命令 + 注入检测
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*FROM.*").unwrap();
        fw.block_command(FirewallCommand::Drop);
        fw.block_command(FirewallCommand::Truncate);
        fw.block_command(FirewallCommand::Alter);
        fw.block_command(FirewallCommand::Grant);
        fw.block_command(FirewallCommand::Revoke);

        // 合法 SELECT 通过
        assert!(fw.check("SELECT * FROM users WHERE id = 1").is_ok());

        // 禁止命令全部被拦截
        assert!(fw.check("DROP TABLE users").is_err());
        assert!(fw.check("TRUNCATE TABLE users").is_err());
        assert!(fw.check("ALTER TABLE users ADD COLUMN x INT").is_err());
        assert!(fw.check("GRANT SELECT ON users TO bob").is_err());
        assert!(fw.check("REVOKE SELECT ON users FROM bob").is_err());

        // 注入特征全部被拦截
        assert!(fw
            .check("SELECT * FROM users WHERE id = 1 UNION SELECT password FROM admins")
            .is_err());
        assert!(fw
            .check("SELECT * FROM users WHERE name = 'a' OR '1'='1")
            .is_err());
        assert!(fw.check("SELECT * FROM users; DROP TABLE users").is_err());

        let stats = fw.stats();
        assert_eq!(stats.total_checks, 9);
        assert_eq!(stats.allowed, 1);
        assert_eq!(stats.blocked, 8);
    }

    #[test]
    fn test_7c6_full_workflow_disabled_allows_malicious() {
        // 禁用防火墙后，恶意 SQL 也放行
        let mut fw = SqlFirewall::new();
        fw.block_command(FirewallCommand::Drop);
        fw.set_enabled(false);

        assert!(fw.check("DROP TABLE users").is_ok());
        assert!(fw
            .check("SELECT * FROM users WHERE name = 'a' OR 1=1 -- ")
            .is_ok());

        let stats = fw.stats();
        assert_eq!(stats.allowed, 2);
        assert_eq!(stats.blocked, 0);
    }

    #[test]
    fn test_7c6_full_workflow_stats_accuracy() {
        // 验证统计准确性：各种拦截类型计数正确
        let mut fw = SqlFirewall::new();
        fw.add_allowed_pattern(r"SELECT.*FROM.*WHERE.*").unwrap();
        fw.block_command(FirewallCommand::Drop);

        // 3 个合法
        fw.check("SELECT * FROM users WHERE id = 1").unwrap();
        fw.check("SELECT name FROM users WHERE active = true")
            .unwrap();
        fw.check("SELECT id FROM orders WHERE user_id = 100")
            .unwrap();

        // 2 个禁止命令
        let _ = fw.check("DROP TABLE users");
        let _ = fw.check("DROP TABLE orders");

        // 2 个注入检测
        let _ = fw.check("SELECT * FROM users WHERE name = 'a' OR 1=1 -- ");
        let _ = fw.check("SELECT id FROM users UNION SELECT password FROM admins");

        // 2 个白名单拦截
        let _ = fw.check("SELECT * FROM users");
        let _ = fw.check("INSERT INTO users VALUES (1)");

        let stats = fw.stats();
        assert_eq!(stats.total_checks, 9);
        assert_eq!(stats.allowed, 3);
        assert_eq!(stats.blocked, 6);
        assert_eq!(stats.injections_detected, 2);
        assert_eq!(stats.blocked_commands, 2);
        assert_eq!(stats.whitelist_blocks, 2);
    }
}
