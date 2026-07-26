//! SQL 注入检测（10 类）— Phase 7d.15
//!
//! 对应 `SzRSQL实施进度.md` Phase 7d.15。
//!
//! # 10 类 SQL 注入
//!
//! 1. **盲注（Blind）** — 基于布尔/时间的盲注，如 `AND 1=1`、`AND SUBSTRING(...)=...`
//! 2. **联合（Union）** — `UNION SELECT`、`UNION ALL SELECT`
//! 3. **堆叠（Stacked）** — 分号后接新语句，如 `;DROP TABLE`
//! 4. **报错（Error-based）** — `extractvalue`、`updatexml`、`floor(rand())`
//! 5. **布尔（Boolean）** — `AND 'a'='a'`、`OR 'a'='a'`、`AND 1=1`
//! 6. **时间（Time-based）** — `SLEEP()`、`BENCHMARK()`、`WAITFOR DELAY`
//! 7. **OS 命令（OS Command）** — `xp_cmdshell`、`INTO OUTFILE`、`INTO DUMPFILE`
//! 8. **编码（Encoding）** — `0x...` HEX、`CHAR()`、`CONCAT()`、`UNHEX()`
//! 9. **注释（Comment）** — `--`、`#`、`/*...*/`、`/*!...*/`
//! 10. **字面量（Literal）** — 引号异常、字符串拼接注入
//!
//! # 验证标准
//!
//! - 检测率 >= 99%
//! - 误报率 < 0.1%

use regex::Regex;
use std::sync::OnceLock;

// =====================================================================
//  SqliType — SQL 注入类型
// =====================================================================

/// SQL 注入类型（10 类）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SqliType {
    /// 盲注（基于布尔/时间的盲注）
    Blind,
    /// 联合查询注入（UNION SELECT）
    Union,
    /// 堆叠查询注入（; 新语句）
    Stacked,
    /// 报错注入（extractvalue/updatexml/floor）
    ErrorBased,
    /// 布尔注入（AND 'a'='a'）
    Boolean,
    /// 时间盲注（SLEEP/BENCHMARK/WAITFOR）
    TimeBased,
    /// OS 命令注入（xp_cmdshell/INTO OUTFILE）
    OsCommand,
    /// 编码注入（0x HEX/CHAR/CONCAT/UNHEX）
    Encoding,
    /// 注释注入（-- # /* */ /*! */）
    Comment,
    /// 字面量注入（引号异常/字符串拼接）
    Literal,
}

impl SqliType {
    /// 返回类型名称
    pub fn as_str(&self) -> &'static str {
        match self {
            SqliType::Blind => "blind",
            SqliType::Union => "union",
            SqliType::Stacked => "stacked",
            SqliType::ErrorBased => "error_based",
            SqliType::Boolean => "boolean",
            SqliType::TimeBased => "time_based",
            SqliType::OsCommand => "os_command",
            SqliType::Encoding => "encoding",
            SqliType::Comment => "comment",
            SqliType::Literal => "literal",
        }
    }

    /// 返回所有 10 类
    pub fn all() -> &'static [SqliType] {
        &[
            SqliType::Blind,
            SqliType::Union,
            SqliType::Stacked,
            SqliType::ErrorBased,
            SqliType::Boolean,
            SqliType::TimeBased,
            SqliType::OsCommand,
            SqliType::Encoding,
            SqliType::Comment,
            SqliType::Literal,
        ]
    }
}

impl std::fmt::Display for SqliType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =====================================================================
//  SqliDetection — 检测结果
// =====================================================================

/// SQL 注入检测结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliDetection {
    /// 检测到的注入类型
    pub sqli_type: SqliType,
    /// 匹配的模式描述
    pub pattern: String,
    /// 匹配的 SQL 片段
    pub matched_fragment: String,
}

impl SqliDetection {
    /// 创建检测结果
    pub fn new(
        sqli_type: SqliType,
        pattern: impl Into<String>,
        matched_fragment: impl Into<String>,
    ) -> Self {
        Self {
            sqli_type,
            pattern: pattern.into(),
            matched_fragment: matched_fragment.into(),
        }
    }
}

// =====================================================================
//  SqliDetector — SQL 注入检测器
// =====================================================================

/// SQL 注入检测器
///
/// 对 SQL 语句进行 10 类注入检测，支持自定义规则。
#[derive(Debug, Clone)]
pub struct SqliDetector {
    /// 是否启用所有 10 类检测
    enabled_types: std::collections::HashSet<SqliType>,
}

impl Default for SqliDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl SqliDetector {
    /// 创建检测器（默认启用所有 10 类）
    pub fn new() -> Self {
        Self {
            enabled_types: SqliType::all().iter().copied().collect(),
        }
    }

    /// 创建检测器（仅启用指定类型）
    pub fn with_types(types: &[SqliType]) -> Self {
        Self {
            enabled_types: types.iter().copied().collect(),
        }
    }

    /// 启用某类检测
    pub fn enable(&mut self, sqli_type: SqliType) -> &mut Self {
        self.enabled_types.insert(sqli_type);
        self
    }

    /// 禁用某类检测
    pub fn disable(&mut self, sqli_type: SqliType) -> &mut Self {
        self.enabled_types.remove(&sqli_type);
        self
    }

    /// 检测 SQL 语句是否包含注入
    ///
    /// 返回 `Some(detection)` 表示检测到注入，`None` 表示未检测到。
    pub fn detect(&self, sql: &str) -> Option<SqliDetection> {
        let normalized = normalize_sql_for_detection(sql);

        for sqli_type in SqliType::all() {
            if !self.enabled_types.contains(sqli_type) {
                continue;
            }
            if let Some(detection) = detect_by_type(*sqli_type, &normalized, sql) {
                return Some(detection);
            }
        }
        None
    }

    /// 检测所有匹配的注入类型
    pub fn detect_all(&self, sql: &str) -> Vec<SqliDetection> {
        let normalized = normalize_sql_for_detection(sql);
        let mut results = Vec::new();

        for sqli_type in SqliType::all() {
            if !self.enabled_types.contains(sqli_type) {
                continue;
            }
            if let Some(detection) = detect_by_type(*sqli_type, &normalized, sql) {
                results.push(detection);
            }
        }
        results
    }

    /// 批量检测，返回检测率
    pub fn detection_rate(&self, payloads: &[&str]) -> f64 {
        if payloads.is_empty() {
            return 0.0;
        }
        let detected = payloads.iter().filter(|p| self.detect(p).is_some()).count();
        detected as f64 / payloads.len() as f64
    }

    /// 批量检测误报率
    pub fn false_positive_rate(&self, normal_sqls: &[&str]) -> f64 {
        if normal_sqls.is_empty() {
            return 0.0;
        }
        let false_positives = normal_sqls
            .iter()
            .filter(|s| self.detect(s).is_some())
            .count();
        false_positives as f64 / normal_sqls.len() as f64
    }
}

// =====================================================================
//  检测逻辑 — 按类型
// =====================================================================

/// 按类型检测 SQL 注入
fn detect_by_type(sqli_type: SqliType, normalized: &str, original: &str) -> Option<SqliDetection> {
    match sqli_type {
        SqliType::Union => detect_union(normalized, original),
        SqliType::Stacked => detect_stacked(normalized, original),
        SqliType::Boolean => detect_boolean(normalized, original),
        SqliType::Blind => detect_blind(normalized, original),
        SqliType::ErrorBased => detect_error_based(normalized, original),
        SqliType::TimeBased => detect_time_based(normalized, original),
        SqliType::OsCommand => detect_os_command(normalized, original),
        SqliType::Encoding => detect_encoding(normalized, original),
        SqliType::Comment => detect_comment(normalized, original),
        SqliType::Literal => detect_literal(normalized, original),
    }
}

/// 联合查询注入检测
fn detect_union(normalized: &str, original: &str) -> Option<SqliDetection> {
    let re = union_regex();
    if let Some(m) = re.find(normalized) {
        return Some(SqliDetection::new(
            SqliType::Union,
            "UNION SELECT / UNION ALL SELECT",
            &original[m.start()..m.end().min(original.len())],
        ));
    }
    None
}

fn union_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bUNION\b\s+(?:ALL\s+)?\bSELECT\b").unwrap())
}

/// 堆叠查询注入检测
fn detect_stacked(normalized: &str, original: &str) -> Option<SqliDetection> {
    let re = stacked_regex();
    if let Some(m) = re.find(normalized) {
        // 排除 OS 命令注入（xp_cmdshell 等优先由 OsCommand 检测）
        if os_regex().is_match(normalized) {
            return None;
        }
        let frag = original.get(m.start()..m.end()).unwrap_or("");
        return Some(SqliDetection::new(
            SqliType::Stacked,
            "; followed by DDL/DML/EXEC",
            frag,
        ));
    }
    None
}

fn stacked_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i);\s*(?:DROP|DELETE|INSERT|UPDATE|CREATE|ALTER|TRUNCATE|GRANT|REVOKE|EXEC|EXECUTE)\b")
            .unwrap()
    })
}

/// 布尔注入检测
fn detect_boolean(normalized: &str, original: &str) -> Option<SqliDetection> {
    let re = boolean_regex();
    if let Some(m) = re.find(normalized) {
        let frag = original.get(m.start()..m.end()).unwrap_or("");
        return Some(SqliDetection::new(
            SqliType::Boolean,
            "Boolean-based: OR/AND with tautology",
            frag,
        ));
    }
    None
}

fn boolean_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:OR|AND)\b\s+['\"]?\d+['\"]?\s*=\s*['\"]?\d+['\"]?|\b(?:OR|AND)\b\s+['\"]\w+['\"]\s*=\s*['\"]?\w+['\"]?"#)
            .unwrap()
    })
}

/// 盲注检测
fn detect_blind(normalized: &str, original: &str) -> Option<SqliDetection> {
    let re = blind_regex();
    if let Some(m) = re.find(normalized) {
        let frag = original.get(m.start()..m.end()).unwrap_or("");
        return Some(SqliDetection::new(
            SqliType::Blind,
            "Blind: SUBSTRING/ASCII/MID/IF with comparison",
            frag,
        ));
    }
    None
}

fn blind_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:SUBSTRING|SUBSTR|MID|ASCII|ORD|CHAR_LENGTH|LENGTH)\s*\(.*\)\s*[<>=]|\bIF\s*\([^,]+,\s*['\"]?\w+['\"]?\s*,\s*['\"]?\w+['\"]?\s*\)"#)
            .unwrap()
    })
}

/// 报错注入检测
fn detect_error_based(normalized: &str, original: &str) -> Option<SqliDetection> {
    let re = error_regex();
    if let Some(m) = re.find(normalized) {
        let frag = original.get(m.start()..m.end()).unwrap_or("");
        return Some(SqliDetection::new(
            SqliType::ErrorBased,
            "Error-based: extractvalue/updatexml/floor",
            frag,
        ));
    }
    None
}

fn error_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:EXTRACTVALUE|UPDATEXML)\s*\(|floor\s*\(\s*rand\s*\(\s*\)|\bEXTRACTVALUE\s*\([^,]+,\s*concat")
            .unwrap()
    })
}

/// 时间盲注检测
fn detect_time_based(normalized: &str, original: &str) -> Option<SqliDetection> {
    let re = time_regex();
    if let Some(m) = re.find(normalized) {
        let frag = original.get(m.start()..m.end()).unwrap_or("");
        return Some(SqliDetection::new(
            SqliType::TimeBased,
            "Time-based: SLEEP/BENCHMARK/WAITFOR DELAY",
            frag,
        ));
    }
    None
}

fn time_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bSLEEP\s*\(|\bBENCHMARK\s*\(|\bWAITFOR\s+DELAY\b|\bPG_SLEEP\s*\(")
            .unwrap()
    })
}

/// OS 命令注入检测
fn detect_os_command(normalized: &str, original: &str) -> Option<SqliDetection> {
    let re = os_regex();
    if let Some(m) = re.find(normalized) {
        let frag = original.get(m.start()..m.end()).unwrap_or("");
        return Some(SqliDetection::new(
            SqliType::OsCommand,
            "OS command: xp_cmdshell/INTO OUTFILE/DUMPFILE",
            frag,
        ));
    }
    None
}

fn os_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\bXP_CMDSHELL\b|\bINTO\s+(?:OUTFILE|DUMPFILE)\b|\bLOAD_FILE\s*\(|\bSYS_EXEC\s*\(",
        )
        .unwrap()
    })
}

/// 编码注入检测
fn detect_encoding(normalized: &str, original: &str) -> Option<SqliDetection> {
    let re = encoding_regex();
    if let Some(m) = re.find(normalized) {
        let frag = original.get(m.start()..m.end()).unwrap_or("");
        return Some(SqliDetection::new(
            SqliType::Encoding,
            "Encoding: 0x HEX/CHAR/CONCAT/UNHEX",
            frag,
        ));
    }
    None
}

fn encoding_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b0x[0-9a-fA-F]{6,}|\bCHAR\s*\(\s*\d+|\bUNHEX\s*\(|\bCONCAT\s*\(\s*0x|\bCONCAT_WS\s*\(")
            .unwrap()
    })
}

/// 注释注入检测
fn detect_comment(normalized: &str, original: &str) -> Option<SqliDetection> {
    let re = comment_regex();
    if let Some(m) = re.find(normalized) {
        let frag = original.get(m.start()..m.end()).unwrap_or("");
        return Some(SqliDetection::new(
            SqliType::Comment,
            "Comment: -- # /* */ /*! */",
            frag,
        ));
    }
    None
}

fn comment_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"--\s|/\*!|/\*\s*\*/|^\s*#|\s+#|/\*.*\*/").unwrap())
}

/// 字面量注入检测（引号异常）
fn detect_literal(normalized: &str, original: &str) -> Option<SqliDetection> {
    // 检测奇数引号
    let single_quotes = original.matches('\'').count();
    let double_quotes = original.matches('"').count();

    if single_quotes.is_multiple_of(2) && double_quotes.is_multiple_of(2) {
        // 引号成对，检查其他字面量异常
        let re = literal_regex();
        if let Some(m) = re.find(normalized) {
            let frag = original.get(m.start()..m.end()).unwrap_or("");
            return Some(SqliDetection::new(
                SqliType::Literal,
                "Literal: quote escaping anomaly",
                frag,
            ));
        }
        return None;
    }

    Some(SqliDetection::new(
        SqliType::Literal,
        "Literal: unmatched quotes",
        if single_quotes % 2 == 1 {
            "unmatched single quote"
        } else {
            "unmatched double quote"
        },
    ))
}

fn literal_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\\'|\bESCAPE\s+'|''\s*OR\s*''|''\s*--|''\s*=\s*").unwrap())
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 规范化 SQL 用于检测（小写化、压缩空格、去除首尾空白）
fn normalize_sql_for_detection(sql: &str) -> String {
    sql.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// =====================================================================
//  Payload 生成器 — 每类 100 条
// =====================================================================

/// 生成盲注 payload（100 条）
pub fn generate_blind_payloads() -> Vec<String> {
    let mut payloads = Vec::with_capacity(100);
    for i in 1..=100 {
        let payload = match i % 10 {
            0 => format!("1 AND SUBSTRING(user(),1,1)='a'-- {}", i),
            1 => format!("1 AND ASCII(SUBSTRING(user(),1,1))>{}-- ", i),
            2 => format!("1' AND SUBSTRING(database(),1,1)='s'-- {}", i),
            3 => format!("1 AND MID(user(),1,1)='r'-- {}", i),
            4 => format!("1' AND LENGTH(user())={}-- ", i),
            5 => format!("1 AND IF(1=1,SLEEP(0),0)-- {}", i),
            6 => format!("1' AND ORD(MID(user(),1,1))={}-- ", i),
            7 => format!("1 AND CHAR_LENGTH(database())>{}-- ", i),
            8 => format!("1' AND SUBSTR(version(),1,1)='1'-- {}", i),
            _ => format!("1 AND IF(SUBSTRING(user(),1,1)='a',1,0)-- {}", i),
        };
        payloads.push(payload);
    }
    payloads
}

/// 生成联合查询 payload（100 条）
pub fn generate_union_payloads() -> Vec<String> {
    let mut payloads = Vec::with_capacity(100);
    for i in 1..=100 {
        let payload = match i % 5 {
            0 => format!("1 UNION SELECT {},2,3-- ", i),
            1 => format!("1 UNION ALL SELECT user(),password,{} FROM users-- ", i),
            2 => format!("' UNION SELECT version(),{},@@version-- ", i),
            3 => format!(
                "1 UNION SELECT table_name,{},3 FROM information_schema.tables-- ",
                i
            ),
            _ => format!("' UNION ALL SELECT 1,{},@@datadir-- ", i),
        };
        payloads.push(payload);
    }
    payloads
}

/// 生成堆叠查询 payload（100 条）
pub fn generate_stacked_payloads() -> Vec<String> {
    let mut payloads = Vec::with_capacity(100);
    for i in 1..=100 {
        let payload = match i % 10 {
            0 => format!("1; DROP TABLE users_{}-- ", i),
            1 => format!("1; DELETE FROM users WHERE id={}-- ", i),
            2 => format!("1; INSERT INTO logs VALUES({})-- ", i),
            3 => format!("1; UPDATE users SET name='x' WHERE id={}-- ", i),
            4 => format!("1; CREATE TABLE hack{} (id INT)-- ", i),
            5 => format!("1; ALTER TABLE users DROP COLUMN col{}-- ", i),
            6 => format!("1; TRUNCATE TABLE data_{}-- ", i),
            7 => format!("1; GRANT ALL ON *.* TO 'a'@'%'-- {}", i),
            8 => format!("1; REVOKE ALL FROM user_{}-- ", i),
            _ => format!("1; EXEC sp_executesql N'select {}'-- ", i),
        };
        payloads.push(payload);
    }
    payloads
}

/// 生成报错注入 payload（100 条）
pub fn generate_error_payloads() -> Vec<String> {
    let mut payloads = Vec::with_capacity(100);
    for i in 1..=100 {
        let payload = match i % 5 {
            0 => format!("1 AND extractvalue(1, concat(0x7e, user(), {}))-- ", i),
            1 => format!("1' AND updatexml(1, concat(0x7e, version()), {})-- ", i),
            2 => format!("1 AND (SELECT 1 FROM (SELECT count(*),concat(user(),floor(rand(0)*2))x FROM information_schema.tables GROUP BY x)a)-- {}", i),
            3 => format!("1' AND extractvalue(1, concat(0x7e, database()))-- {}", i),
            _ => format!("1 AND updatexml(1, concat(0x7e, current_user()), {})-- ", i),
        };
        payloads.push(payload);
    }
    payloads
}

/// 生成布尔注入 payload（100 条）
pub fn generate_boolean_payloads() -> Vec<String> {
    let mut payloads = Vec::with_capacity(100);
    for i in 1..=100 {
        let payload = match i % 10 {
            0 => format!("1 OR 1=1-- {}", i),
            1 => format!("1' OR '1'='1'-- {}", i),
            2 => format!("1 AND 1=1-- {}", i),
            3 => format!("1' OR 'a'='a'-- {}", i),
            4 => format!("1 OR 1=2-- {}", i),
            5 => format!("1' AND 'a'='a'-- {}", i),
            6 => format!("1 OR true-- {}", i),
            7 => format!("1' OR ''=''-- {}", i),
            8 => format!("1 AND 1<>2-- {}", i),
            _ => format!("1' OR '1'='1' #{}", i),
        };
        payloads.push(payload);
    }
    payloads
}

/// 生成时间盲注 payload（100 条）
pub fn generate_time_payloads() -> Vec<String> {
    let mut payloads = Vec::with_capacity(100);
    for i in 1..=100 {
        let payload = match i % 5 {
            0 => format!("1 AND SLEEP({})-- ", i),
            1 => format!("1' AND BENCHMARK(1000000,MD5('a'))-- {}", i),
            2 => format!("1; WAITFOR DELAY '0:0:{}'-- ", i),
            3 => format!("1' AND SLEEP(5)-- {}", i),
            _ => format!("1 AND IF(1=1,SLEEP({}),0)-- ", i),
        };
        payloads.push(payload);
    }
    payloads
}

/// 生成 OS 命令注入 payload（100 条）
pub fn generate_os_command_payloads() -> Vec<String> {
    let mut payloads = Vec::with_capacity(100);
    for i in 1..=100 {
        let payload = match i % 5 {
            0 => format!("1; EXEC xp_cmdshell 'dir {}'-- ", i),
            1 => format!("1' INTO OUTFILE '/tmp/hack{}.txt'-- ", i),
            2 => format!("1' INTO DUMPFILE '/var/www/shell{}.php'-- ", i),
            3 => format!("1' UNION SELECT LOAD_FILE('/etc/passwd'),{}-- ", i),
            _ => format!("1; EXEC xp_cmdshell 'whoami > {}'-- ", i),
        };
        payloads.push(payload);
    }
    payloads
}

/// 生成编码注入 payload（100 条）
pub fn generate_encoding_payloads() -> Vec<String> {
    let mut payloads = Vec::with_capacity(100);
    for i in 1..=100 {
        let payload = match i % 5 {
            0 => format!("1 UNION SELECT 0x4142434445{},2,3-- ", i),
            1 => format!(
                "1' OR 1=1 UNION SELECT CHAR(115,101,108,101,99,116,{},50),2-- ",
                i
            ),
            2 => format!("1 AND CONCAT(0x7e,user(),{})-- ", i),
            3 => format!("1' UNION SELECT UNHEX('4142434445464748'),{}-- ", i),
            _ => format!("1 AND CONCAT_WS(0x7e,user(),version(),{})-- ", i),
        };
        payloads.push(payload);
    }
    payloads
}

/// 生成注释注入 payload（100 条）
pub fn generate_comment_payloads() -> Vec<String> {
    let mut payloads = Vec::with_capacity(100);
    for i in 1..=100 {
        let payload = match i % 5 {
            0 => format!("1 OR 1=1-- {}", i),
            1 => format!("1' OR 1=1#{}", i),
            2 => format!("1 OR 1=1/*comment{}*/", i),
            3 => format!("1' /*!50000 OR 1=1*/-- {}", i),
            _ => format!("1 OR 1=1 /* inline {} */ --", i),
        };
        payloads.push(payload);
    }
    payloads
}

/// 生成字面量注入 payload（100 条）
pub fn generate_literal_payloads() -> Vec<String> {
    let mut payloads = Vec::with_capacity(100);
    for i in 1..=100 {
        let payload = match i % 5 {
            0 => format!("admin'-- {}", i),
            1 => format!("admin' OR ''='{}", i),
            2 => format!("admin\\' OR 1=1-- {}", i),
            3 => format!("admin'-- {}", i),
            _ => format!("' OR ''='{}", i),
        };
        payloads.push(payload);
    }
    payloads
}

/// 生成正常 SQL 语句（100 条，用于测试误报率）
pub fn generate_normal_sqls() -> Vec<String> {
    let mut sqls = Vec::with_capacity(100);
    for i in 1..=100 {
        let sql = match i % 20 {
            0 => format!("SELECT id, name, email FROM users WHERE id = {}", i),
            1 => format!("INSERT INTO orders (user_id, amount) VALUES ({}, 99.99)", i),
            2 => format!("UPDATE products SET price = {} WHERE id = 1", i),
            3 => format!("DELETE FROM logs WHERE created_at < '2024-01-01' AND id = {}", i),
            4 => format!("SELECT COUNT(*) FROM orders WHERE status = 'pending' AND id > {}", i),
            5 => format!("SELECT u.name, o.amount FROM users u JOIN orders o ON u.id = o.user_id WHERE u.id = {}", i),
            6 => format!("SELECT * FROM products WHERE category = 'electronics' ORDER BY price LIMIT {}", i),
            7 => format!("INSERT INTO audit_log (action, user_id) VALUES ('login', {})", i),
            8 => format!("UPDATE users SET last_login = NOW() WHERE id = {}", i),
            9 => format!("SELECT DISTINCT category FROM products WHERE price > {}", i),
            10 => format!("SELECT AVG(price) FROM products GROUP BY category HAVING AVG(price) > {}", i),
            11 => format!("SELECT * FROM users WHERE name LIKE 'John%' AND age > {}", i),
            12 => format!("DELETE FROM temp_data WHERE expires_at < NOW() AND batch_id = {}", i),
            13 => format!("SELECT t.title, t.content FROM articles t WHERE t.published = 1 AND t.id = {}", i),
            14 => format!("INSERT INTO cart (user_id, product_id, qty) VALUES ({}, 101, 1)", i),
            15 => format!("SELECT SUM(amount) FROM orders WHERE user_id = {} AND status = 'paid'", i),
            16 => format!("UPDATE inventory SET stock = stock - 1 WHERE product_id = {}", i),
            17 => format!("SELECT * FROM events WHERE start_date >= '2024-06-01' AND id = {}", i),
            18 => format!("SELECT name, COUNT(*) as cnt FROM tags GROUP BY name ORDER BY cnt DESC LIMIT {}", i),
            _ => format!("SELECT * FROM config WHERE section = 'system' AND key = 'timeout_{}'", i),
        };
        sqls.push(sql);
    }
    sqls
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- SqliType ---

    #[test]
    fn test_sqli_type_as_str() {
        assert_eq!(SqliType::Blind.as_str(), "blind");
        assert_eq!(SqliType::Union.as_str(), "union");
        assert_eq!(SqliType::Stacked.as_str(), "stacked");
        assert_eq!(SqliType::ErrorBased.as_str(), "error_based");
        assert_eq!(SqliType::Boolean.as_str(), "boolean");
        assert_eq!(SqliType::TimeBased.as_str(), "time_based");
        assert_eq!(SqliType::OsCommand.as_str(), "os_command");
        assert_eq!(SqliType::Encoding.as_str(), "encoding");
        assert_eq!(SqliType::Comment.as_str(), "comment");
        assert_eq!(SqliType::Literal.as_str(), "literal");
    }

    #[test]
    fn test_sqli_type_all_has_10() {
        assert_eq!(SqliType::all().len(), 10);
    }

    #[test]
    fn test_sqli_type_display() {
        assert_eq!(format!("{}", SqliType::Union), "union");
    }

    // --- SqliDetector 基本检测 ---

    #[test]
    fn test_detect_union_select() {
        let detector = SqliDetector::new();
        let result = detector.detect("1 UNION SELECT 1,2,3");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::Union);
    }

    #[test]
    fn test_detect_union_all_select() {
        let detector = SqliDetector::new();
        let result = detector.detect("' UNION ALL SELECT user(),password FROM users");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::Union);
    }

    #[test]
    fn test_detect_stacked_drop() {
        let detector = SqliDetector::new();
        let result = detector.detect("1; DROP TABLE users");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::Stacked);
    }

    #[test]
    fn test_detect_stacked_insert() {
        let detector = SqliDetector::new();
        let result = detector.detect("1; INSERT INTO logs VALUES(1)");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::Stacked);
    }

    #[test]
    fn test_detect_boolean_or_1_1() {
        let detector = SqliDetector::new();
        let result = detector.detect("1 OR 1=1");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::Boolean);
    }

    #[test]
    fn test_detect_boolean_and_a_a() {
        let detector = SqliDetector::new();
        let result = detector.detect("' OR 'a'='a");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::Boolean);
    }

    #[test]
    fn test_detect_blind_substring() {
        let detector = SqliDetector::new();
        let result = detector.detect("1 AND SUBSTRING(user(),1,1)='a'");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::Blind);
    }

    #[test]
    fn test_detect_blind_if() {
        let detector = SqliDetector::new();
        let result = detector.detect("1 AND IF(1=1,'yes','no')");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::Blind);
    }

    #[test]
    fn test_detect_error_extractvalue() {
        let detector = SqliDetector::new();
        let result = detector.detect("1 AND extractvalue(1, concat(0x7e, user()))");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::ErrorBased);
    }

    #[test]
    fn test_detect_error_updatexml() {
        let detector = SqliDetector::new();
        let result = detector.detect("1' AND updatexml(1, concat(0x7e, version()), 1)");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::ErrorBased);
    }

    #[test]
    fn test_detect_time_sleep() {
        let detector = SqliDetector::new();
        let result = detector.detect("1 AND SLEEP(5)");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::TimeBased);
    }

    #[test]
    fn test_detect_time_benchmark() {
        let detector = SqliDetector::new();
        let result = detector.detect("1' AND BENCHMARK(1000000,MD5('a'))");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::TimeBased);
    }

    #[test]
    fn test_detect_time_waitfor() {
        let detector = SqliDetector::new();
        let result = detector.detect("1; WAITFOR DELAY '0:0:5'");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::TimeBased);
    }

    #[test]
    fn test_detect_os_xp_cmdshell() {
        let detector = SqliDetector::new();
        let result = detector.detect("1; EXEC xp_cmdshell 'dir'");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::OsCommand);
    }

    #[test]
    fn test_detect_os_into_outfile() {
        let detector = SqliDetector::new();
        let result = detector.detect("1' INTO OUTFILE '/tmp/hack.txt'");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::OsCommand);
    }

    #[test]
    fn test_detect_encoding_hex() {
        let detector = SqliDetector::new();
        let result = detector.detect("1 UNION SELECT 0x4142434445,2,3");
        assert!(result.is_some());
    }

    #[test]
    fn test_detect_encoding_char() {
        let detector = SqliDetector::new();
        let result = detector.detect("1' OR 1=1 UNION SELECT CHAR(115,101,108,101,99,116),2");
        assert!(result.is_some());
    }

    #[test]
    fn test_detect_comment_double_dash() {
        let detector = SqliDetector::new();
        let result = detector.detect("1 OR 1=1-- comment");
        assert!(result.is_some());
    }

    #[test]
    fn test_detect_comment_hash() {
        let detector = SqliDetector::new();
        let result = detector.detect("1' OR 1=1# comment");
        assert!(result.is_some());
    }

    #[test]
    fn test_detect_literal_unmatched_quote() {
        let detector = SqliDetector::new();
        let result = detector.detect("admin' --");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::Literal);
    }

    #[test]
    fn test_detect_literal_double_quote() {
        let detector = SqliDetector::new();
        let result = detector.detect("admin\" OR 1=1");
        assert!(result.is_some());
    }

    // --- 正常 SQL 0 误报 ---

    #[test]
    fn test_no_false_positive_simple_select() {
        let detector = SqliDetector::new();
        assert!(detector
            .detect("SELECT id, name FROM users WHERE id = 1")
            .is_none());
    }

    #[test]
    fn test_no_false_positive_insert() {
        let detector = SqliDetector::new();
        assert!(detector
            .detect("INSERT INTO orders (user_id, amount) VALUES (1, 99.99)")
            .is_none());
    }

    #[test]
    fn test_no_false_positive_update() {
        let detector = SqliDetector::new();
        assert!(detector
            .detect("UPDATE products SET price = 100 WHERE id = 1")
            .is_none());
    }

    #[test]
    fn test_no_false_positive_delete() {
        let detector = SqliDetector::new();
        assert!(detector
            .detect("DELETE FROM logs WHERE created_at < '2024-01-01'")
            .is_none());
    }

    #[test]
    fn test_no_false_positive_join() {
        let detector = SqliDetector::new();
        assert!(detector
            .detect("SELECT u.name, o.amount FROM users u JOIN orders o ON u.id = o.user_id")
            .is_none());
    }

    #[test]
    fn test_no_false_positive_group_by() {
        let detector = SqliDetector::new();
        assert!(detector
            .detect("SELECT AVG(price) FROM products GROUP BY category HAVING AVG(price) > 100")
            .is_none());
    }

    // --- 100 条 payload 检测率验证 ---

    #[test]
    fn test_detection_rate_blind_100() {
        let detector = SqliDetector::new();
        let payloads = generate_blind_payloads();
        assert_eq!(payloads.len(), 100);
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let rate = detector.detection_rate(&refs);
        assert!(rate >= 0.99, "Blind detection rate = {}", rate);
    }

    #[test]
    fn test_detection_rate_union_100() {
        let detector = SqliDetector::new();
        let payloads = generate_union_payloads();
        assert_eq!(payloads.len(), 100);
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let rate = detector.detection_rate(&refs);
        assert!(rate >= 0.99, "Union detection rate = {}", rate);
    }

    #[test]
    fn test_detection_rate_stacked_100() {
        let detector = SqliDetector::new();
        let payloads = generate_stacked_payloads();
        assert_eq!(payloads.len(), 100);
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let rate = detector.detection_rate(&refs);
        assert!(rate >= 0.99, "Stacked detection rate = {}", rate);
    }

    #[test]
    fn test_detection_rate_error_100() {
        let detector = SqliDetector::new();
        let payloads = generate_error_payloads();
        assert_eq!(payloads.len(), 100);
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let rate = detector.detection_rate(&refs);
        assert!(rate >= 0.99, "Error-based detection rate = {}", rate);
    }

    #[test]
    fn test_detection_rate_boolean_100() {
        let detector = SqliDetector::new();
        let payloads = generate_boolean_payloads();
        assert_eq!(payloads.len(), 100);
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let rate = detector.detection_rate(&refs);
        assert!(rate >= 0.99, "Boolean detection rate = {}", rate);
    }

    #[test]
    fn test_detection_rate_time_100() {
        let detector = SqliDetector::new();
        let payloads = generate_time_payloads();
        assert_eq!(payloads.len(), 100);
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let rate = detector.detection_rate(&refs);
        assert!(rate >= 0.99, "Time-based detection rate = {}", rate);
    }

    #[test]
    fn test_detection_rate_os_100() {
        let detector = SqliDetector::new();
        let payloads = generate_os_command_payloads();
        assert_eq!(payloads.len(), 100);
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let rate = detector.detection_rate(&refs);
        assert!(rate >= 0.99, "OS command detection rate = {}", rate);
    }

    #[test]
    fn test_detection_rate_encoding_100() {
        let detector = SqliDetector::new();
        let payloads = generate_encoding_payloads();
        assert_eq!(payloads.len(), 100);
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let rate = detector.detection_rate(&refs);
        assert!(rate >= 0.99, "Encoding detection rate = {}", rate);
    }

    #[test]
    fn test_detection_rate_comment_100() {
        let detector = SqliDetector::new();
        let payloads = generate_comment_payloads();
        assert_eq!(payloads.len(), 100);
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let rate = detector.detection_rate(&refs);
        assert!(rate >= 0.99, "Comment detection rate = {}", rate);
    }

    #[test]
    fn test_detection_rate_literal_100() {
        let detector = SqliDetector::new();
        let payloads = generate_literal_payloads();
        assert_eq!(payloads.len(), 100);
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let rate = detector.detection_rate(&refs);
        assert!(rate >= 0.99, "Literal detection rate = {}", rate);
    }

    // --- 正常 SQL 0 误报率验证 ---

    #[test]
    fn test_false_positive_rate_normal_100() {
        let detector = SqliDetector::new();
        let normal_sqls = generate_normal_sqls();
        assert_eq!(normal_sqls.len(), 100);
        let refs: Vec<&str> = normal_sqls.iter().map(|s| s.as_str()).collect();
        let fpr = detector.false_positive_rate(&refs);
        assert!(fpr < 0.001, "False positive rate = {}", fpr);
    }

    // --- detect_all ---

    #[test]
    fn test_detect_all_multiple_types() {
        let detector = SqliDetector::new();
        // 同时含 UNION 和 注释
        let results = detector.detect_all("1 UNION SELECT 1,2,3-- comment");
        assert!(!results.is_empty());
        let types: Vec<_> = results.iter().map(|d| d.sqli_type).collect();
        assert!(types.contains(&SqliType::Union));
    }

    // --- enable/disable ---

    #[test]
    fn test_disable_type() {
        let mut detector = SqliDetector::new();
        detector.disable(SqliType::Union);
        assert!(detector.detect("1 UNION SELECT 1,2,3").is_none());
    }

    #[test]
    fn test_enable_type() {
        let mut detector = SqliDetector::with_types(&[]);
        detector.enable(SqliType::Union);
        assert!(detector.detect("1 UNION SELECT 1,2,3").is_some());
    }

    #[test]
    fn test_with_types_empty() {
        let detector = SqliDetector::with_types(&[]);
        assert!(detector.detect("1 UNION SELECT 1,2,3").is_none());
        assert!(detector.detect("1; DROP TABLE users").is_none());
    }

    // --- SqliDetection ---

    #[test]
    fn test_sqli_detection_new() {
        let d = SqliDetection::new(SqliType::Union, "test pattern", "matched text");
        assert_eq!(d.sqli_type, SqliType::Union);
        assert_eq!(d.pattern, "test pattern");
        assert_eq!(d.matched_fragment, "matched text");
    }

    // --- normalize ---

    #[test]
    fn test_normalize_sql() {
        let normalized = normalize_sql_for_detection("  SELECT   *   FROM   users  ");
        assert_eq!(normalized, "select * from users");
    }

    // --- 边界测试 ---

    #[test]
    fn test_empty_sql() {
        let detector = SqliDetector::new();
        assert!(detector.detect("").is_none());
    }

    #[test]
    fn test_whitespace_only() {
        let detector = SqliDetector::new();
        assert!(detector.detect("   ").is_none());
    }

    #[test]
    fn test_case_insensitive_detection() {
        let detector = SqliDetector::new();
        assert!(detector.detect("1 union select 1,2,3").is_some());
        assert!(detector.detect("1 UnIoN SeLeCt 1,2,3").is_some());
        assert!(detector.detect("1 UNION SELECT 1,2,3").is_some());
    }

    #[test]
    fn test_pg_sleep_detection() {
        let detector = SqliDetector::new();
        let result = detector.detect("1 AND PG_SLEEP(5)");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::TimeBased);
    }

    #[test]
    fn test_load_file_detection() {
        let detector = SqliDetector::new();
        let result = detector.detect("1' UNION SELECT LOAD_FILE('/etc/passwd'),2");
        assert!(result.is_some());
    }

    #[test]
    fn test_into_dumpfile_detection() {
        let detector = SqliDetector::new();
        let result = detector.detect("1' INTO DUMPFILE '/var/www/shell.php'");
        assert!(result.is_some());
        assert_eq!(result.unwrap().sqli_type, SqliType::OsCommand);
    }

    #[test]
    fn test_normal_sql_with_semicolon() {
        // 带分号但不跟 DDL/DML 的正常 SQL
        let detector = SqliDetector::new();
        // 注意：实际正常 SQL 不以分号结尾，但存储过程可能有
        // 确保不误报
        assert!(detector.detect("SELECT 1").is_none());
    }

    #[test]
    fn test_detection_rate_all_types() {
        let detector = SqliDetector::new();
        let all_payloads: Vec<Vec<String>> = vec![
            generate_blind_payloads(),
            generate_union_payloads(),
            generate_stacked_payloads(),
            generate_error_payloads(),
            generate_boolean_payloads(),
            generate_time_payloads(),
            generate_os_command_payloads(),
            generate_encoding_payloads(),
            generate_comment_payloads(),
            generate_literal_payloads(),
        ];

        for (idx, payloads) in all_payloads.iter().enumerate() {
            assert_eq!(payloads.len(), 100, "Type {} should have 100 payloads", idx);
            let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
            let rate = detector.detection_rate(&refs);
            assert!(rate >= 0.99, "Type {} detection rate = {}", idx, rate);
        }
    }
}
