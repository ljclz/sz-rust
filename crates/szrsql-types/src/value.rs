//! SzRSQL 标量值类型 — 对应 `SzRSQL技术实现方案.md` 9.1 节。
//!
//! 设计要点：
//! - 16 个变体覆盖 SQL 标准类型 + 数组/枚举/范围/JSON/全文检索/向量
//! - `Decimal(i128, u8)` 用定点数表示，避免 f64 精度损失
//! - `Date(i32)` / `Timestamp(i64)` 用整数偏移，避免时区/日历库依赖
//! - `Json(serde_json::Value)` 直接复用生态，避免重新发明
//! - `TsVector` / `TsQuery` 用于 PG 全文检索（Phase 3.33）

use serde::{Deserialize, Serialize};

// =====================================================================
//  核心标量类型
// =====================================================================

/// SzRSQL 支持的标量类型枚举
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    /// SQL NULL
    Null,
    /// BIGINT / INT8 / INT64
    Int64(i64),
    /// DOUBLE PRECISION / FLOAT8 / FLOAT64
    Float64(f64),
    /// VARCHAR / TEXT / CHAR
    Text(String),
    /// BYTEA / BLOB / VARBINARY
    Blob(Vec<u8>),
    /// BOOLEAN
    Bool(bool),
    /// DATE — 自 1970-01-01 起的天数偏移（i32 足够覆盖 ±588 万年）
    Date(i32),
    /// TIMESTAMP — 微秒精度 UTC 时间戳（i64 足够覆盖 ±29 万年）
    Timestamp(i64),
    /// DECIMAL(precision, scale) — (未缩放值, 小数位数)
    /// 例如 123.45 表示为 `Decimal(12345, 2)`
    Decimal(i128, u8),
    /// 数组类型 — `INT[]`, `TEXT[]` 等，元素可为异构（运行时校验同构）
    Array(Vec<Value>),
    /// ENUM 类型值 — 存储枚举字面量字符串
    Enum(String),
    /// 范围类型 — int4range / tsrange / daterange 等
    Range(RangeValue),
    /// JSON / JSONB — 复用 serde_json 的 Value 类型
    Json(serde_json::Value),
    /// PG tsvector — 全文检索文档向量（Phase 3.33）
    TsVector(TsVector),
    /// PG tsquery — 全文检索查询表达式（Phase 3.33）
    TsQuery(TsQuery),
    /// 向量类型 — AI 嵌入向量（Phase P4-5）
    ///
    /// 存储为 `Vec<f64>`，支持余弦相似度、L2 距离、点积等运算。
    Vector(VectorValue),
    /// XML 类型 — SQL/XML 标准（Phase P4-2）
    ///
    /// 存储为 XML 文档字符串，支持 XMLELEMENT / XMLCONCAT / XMLCOMMENT 等函数。
    Xml(String),
}

// =====================================================================
//  范围类型辅助结构
// =====================================================================

/// 范围值表示 — 对应 PG int4range / numrange / tsrange / tstzrange / daterange
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RangeValue {
    /// 下界（None 表示无下界 / -∞）
    pub lower: Option<Box<Value>>,
    /// 上界（None 表示无上界 / +∞）
    pub upper: Option<Box<Value>>,
    /// 下界是否包含（true = `[`，false = `(`）
    pub lower_inc: bool,
    /// 上界是否包含（true = `]`，false = `)`）
    pub upper_inc: bool,
    /// 范围子类型
    pub range_type: RangeType,
}

/// 范围子类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RangeType {
    /// int4range — 整数范围
    Int4Range,
    /// numrange — 数值范围（含 float / decimal）
    NumRange,
    /// tsrange — 时间戳范围（不带时区）
    TsRange,
    /// tstzrange — 时间戳范围（带时区）
    TstzRange,
    /// daterange — 日期范围
    DateRange,
}

// =====================================================================
//  全文检索类型辅助结构（Phase 3.33）
// =====================================================================

/// PG tsvector — 全文检索文档向量
///
/// 表示已分词后的文档：词素列表 + 每个词素的位置/权重。
/// 例如 `'hello world'::tsvector` 解析为：
/// ```text
/// TsVector {
///     lexemes: [
///         TsLexeme { term: "hello",  positions: [TsLexemePosition { position: 1, weight: 0 }] },
///         TsLexeme { term: "world",  positions: [TsLexemePosition { position: 2, weight: 0 }] },
///     ],
/// }
/// ```
///
/// 词素按字典序排序去重，与 PG 行为一致。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct TsVector {
    /// 词素列表（按字典序升序、去重）
    pub lexemes: Vec<TsLexeme>,
}

/// tsvector 中的单个词素条目
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TsLexeme {
    /// 词素文本（已小写化）
    pub term: String,
    /// 位置列表（可多个，按位置升序）
    pub positions: Vec<TsLexemePosition>,
}

/// 词素的位置 + 权重
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TsLexemePosition {
    /// 位置（1-based）
    pub position: u32,
    /// 权重位掩码：bit0=D(1), bit1=C(2), bit2=B(4), bit3=A(8)；0 表示无权重
    pub weight: u8,
}

/// 权重常量 — 与 PG 的 A/B/C/D 对应
pub const TS_WEIGHT_D: u8 = 1;
/// 权重 C
pub const TS_WEIGHT_C: u8 = 2;
/// 权重 B
pub const TS_WEIGHT_B: u8 = 4;
/// 权重 A（最高）
pub const TS_WEIGHT_A: u8 = 8;

/// PG tsquery — 全文检索查询表达式
///
/// 词素布尔组合表达式树，支持：
/// - `Lexeme` — 单个词素（可选权重过滤）
/// - `And` / `Or` — 布尔组合
/// - `Not` — 否定（左有右无）
/// - `FollowedBy` — `a <N> b`，N 步内相邻
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TsQuery {
    /// 空查询（不匹配任何 tsvector）
    Empty,
    /// 单个词素
    Lexeme {
        /// 词素文本
        term: String,
        /// 权重过滤位掩码（0 表示不限制权重）
        weights: u8,
    },
    /// AND — 左右子查询都需匹配
    And(Box<TsQuery>, Box<TsQuery>),
    /// OR — 左右子查询任一匹配
    Or(Box<TsQuery>, Box<TsQuery>),
    /// NOT — 左匹配且右不匹配（PG 的 `a & !b` 语义）
    Not(Box<TsQuery>),
    /// FOLLOWED BY — 左右相邻匹配，距离不超过 `distance`
    FollowedBy {
        /// 距离（默认 1，即相邻）
        distance: u32,
        /// 左子查询
        left: Box<TsQuery>,
        /// 右子查询
        right: Box<TsQuery>,
    },
}

impl TsVector {
    /// 构造空 tsvector
    pub fn new() -> Self {
        Self::default()
    }

    /// 从分词迭代器构造 tsvector（自动分配位置、去重、排序）
    ///
    /// 输入：词素文本迭代器（顺序即文档顺序）
    /// 输出：按字典序排序、去重的 tsvector，每个词素带其出现位置列表
    pub fn from_lexemes<I, S>(lexemes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        use std::collections::BTreeMap;
        let mut map: BTreeMap<String, Vec<TsLexemePosition>> = BTreeMap::new();
        for (idx, lex) in lexemes.into_iter().enumerate() {
            let term = lex.into().to_lowercase();
            let pos = u32::try_from(idx + 1).unwrap_or(u32::MAX);
            let entry = map.entry(term).or_default();
            entry.push(TsLexemePosition {
                position: pos,
                weight: 0,
            });
        }
        let lexemes: Vec<TsLexeme> = map
            .into_iter()
            .map(|(term, mut positions)| {
                positions.sort_by_key(|p| p.position);
                TsLexeme { term, positions }
            })
            .collect();
        Self { lexemes }
    }

    /// 查询是否包含指定词素
    pub fn contains_term(&self, term: &str) -> bool {
        // lexemes 已按字典序排序，可二分查找
        self.lexemes
            .binary_search_by_key(&term.to_lowercase().as_str(), |l| l.term.as_str())
            .is_ok()
    }

    /// 获取所有词素文本
    pub fn terms(&self) -> Vec<&str> {
        self.lexemes.iter().map(|l| l.term.as_str()).collect()
    }

    /// 序列化为 PG 文本格式：`'hello:1 world:2A'`
    pub fn to_pg_string(&self) -> String {
        let parts: Vec<String> = self
            .lexemes
            .iter()
            .map(|l| {
                let pos_strs: Vec<String> = l
                    .positions
                    .iter()
                    .map(|p| {
                        let mut s = p.position.to_string();
                        if p.weight & TS_WEIGHT_A != 0 {
                            s.push('A');
                        }
                        if p.weight & TS_WEIGHT_B != 0 {
                            s.push('B');
                        }
                        if p.weight & TS_WEIGHT_C != 0 {
                            s.push('C');
                        }
                        if p.weight & TS_WEIGHT_D != 0 {
                            s.push('D');
                        }
                        s
                    })
                    .collect();
                format!("{}:{}", l.term, pos_strs.join(","))
            })
            .collect();
        parts.join(" ")
    }

    /// 从 PG 文本格式解析：`'hello:1 world:2A'` 或 `'hello world'`
    ///
    /// 简化解析：
    /// - 按空白分词
    /// - 每个词形如 `term` 或 `term:pos1,pos2,...`，pos 后可跟 A/B/C/D 权重字母
    /// - 解析失败时返回空 tsvector（与 PG 的宽松容错不同，这里严格）
    pub fn parse(s: &str) -> Result<Self, String> {
        use std::collections::BTreeMap;
        let mut map: BTreeMap<String, Vec<TsLexemePosition>> = BTreeMap::new();
        for token in s.split_whitespace() {
            if token.is_empty() {
                continue;
            }
            // 分离 term 与位置部分
            let (term, positions_str) = match token.find(':') {
                Some(idx) => (&token[..idx], Some(&token[idx + 1..])),
                None => (token, None),
            };
            let term = term.to_lowercase();
            if term.is_empty() {
                return Err(format!("empty term in token: {token}"));
            }
            // 若无位置，则按出现顺序自动分配（1-based）
            let next_pos = u32::try_from(map.len() + 1).unwrap_or(u32::MAX);
            let entry = map.entry(term.clone()).or_default();
            if let Some(pos_str) = positions_str {
                for pos_part in pos_str.split(',') {
                    if pos_part.is_empty() {
                        continue;
                    }
                    // 提取数字部分
                    let digits_end = pos_part
                        .bytes()
                        .position(|b| !b.is_ascii_digit())
                        .unwrap_or(pos_part.len());
                    if digits_end == 0 {
                        return Err(format!("invalid position (no digits): {pos_part}"));
                    }
                    let pos: u32 = pos_part[..digits_end]
                        .parse()
                        .map_err(|e| format!("invalid position '{pos_part}': {e}"))?;
                    // 剩余字符为权重
                    let mut weight = 0u8;
                    for c in pos_part[digits_end..].chars() {
                        match c {
                            'A' | 'a' => weight |= TS_WEIGHT_A,
                            'B' | 'b' => weight |= TS_WEIGHT_B,
                            'C' | 'c' => weight |= TS_WEIGHT_C,
                            'D' | 'd' => weight |= TS_WEIGHT_D,
                            _ => return Err(format!("invalid weight char '{c}' in {pos_part}")),
                        }
                    }
                    entry.push(TsLexemePosition {
                        position: pos,
                        weight,
                    });
                }
            } else {
                // 无位置声明时使用自动分配的位置（已计算 next_pos）
                entry.push(TsLexemePosition {
                    position: next_pos,
                    weight: 0,
                });
            }
        }
        let lexemes: Vec<TsLexeme> = map
            .into_iter()
            .map(|(term, mut positions)| {
                positions.sort_by_key(|p| p.position);
                TsLexeme { term, positions }
            })
            .collect();
        Ok(Self { lexemes })
    }
}

impl TsQuery {
    /// 构造空 tsquery
    pub fn empty() -> Self {
        Self::Empty
    }

    /// 构造简单词素查询（无权重过滤）
    pub fn lexeme<S: Into<String>>(term: S) -> Self {
        Self::Lexeme {
            term: term.into().to_lowercase(),
            weights: 0,
        }
    }

    /// AND 组合
    pub fn and(self, other: Self) -> Self {
        Self::And(Box::new(self), Box::new(other))
    }

    /// OR 组合
    pub fn or(self, other: Self) -> Self {
        Self::Or(Box::new(self), Box::new(other))
    }

    /// NOT 一元（不直接命名 `not` 以避免与 `std::ops::Not` trait 冲突）
    pub fn not_query(self) -> Self {
        Self::Not(Box::new(self))
    }

    /// 检查 tsquery 是否匹配 tsvector（`@@` 操作符）
    ///
    /// PG 语义：
    /// - Empty → false（空查询不匹配任何文档）
    /// - Lexeme(t) → tsvector 包含词素 t（且权重匹配）
    /// - And(l, r) → l 匹配 AND r 匹配
    /// - Or(l, r) → l 匹配 OR r 匹配
    /// - Not(q) → q 不匹配（注意 PG 的 `& !` 语义：NOT 仅在 AND 上下文中有意义）
    /// - FollowedBy(l, r, n) → l 和 r 都匹配，且存在位置对 (p_l, p_r) 使得
    ///   `p_r - p_l ∈ [1, n]`
    pub fn matches(&self, ts: &TsVector) -> bool {
        match self {
            Self::Empty => false,
            Self::Lexeme { term, weights } => {
                if let Ok(idx) = ts
                    .lexemes
                    .binary_search_by_key(&term.as_str(), |l| l.term.as_str())
                {
                    let lex = &ts.lexemes[idx];
                    if *weights == 0 {
                        // 无权重过滤 → 匹配
                        true
                    } else {
                        // 至少一个位置的权重命中过滤掩码
                        lex.positions.iter().any(|p| p.weight & weights != 0)
                    }
                } else {
                    false
                }
            }
            Self::And(l, r) => l.matches(ts) && r.matches(ts),
            Self::Or(l, r) => l.matches(ts) || r.matches(ts),
            Self::Not(q) => !q.matches(ts),
            Self::FollowedBy {
                distance,
                left,
                right,
            } => {
                // 收集左、右子查询匹配的位置列表
                let left_positions = collect_match_positions(left, ts);
                let right_positions = collect_match_positions(right, ts);
                // 检查是否存在 (lp, rp) 满足 1 <= rp - lp <= distance
                for &lp in &left_positions {
                    for &rp in &right_positions {
                        if rp > lp && rp - lp <= *distance {
                            return true;
                        }
                    }
                }
                false
            }
        }
    }

    /// 序列化为 PG 文本格式：`'hello & world'`
    pub fn to_pg_string(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Lexeme { term, weights } => {
                if *weights == 0 {
                    term.clone()
                } else {
                    let mut s = term.clone();
                    s.push(':');
                    if *weights & TS_WEIGHT_A != 0 {
                        s.push('A');
                    }
                    if *weights & TS_WEIGHT_B != 0 {
                        s.push('B');
                    }
                    if *weights & TS_WEIGHT_C != 0 {
                        s.push('C');
                    }
                    if *weights & TS_WEIGHT_D != 0 {
                        s.push('D');
                    }
                    s
                }
            }
            Self::And(l, r) => format!("({} & {})", l.to_pg_string(), r.to_pg_string()),
            Self::Or(l, r) => format!("({} | {})", l.to_pg_string(), r.to_pg_string()),
            Self::Not(q) => format!("!{}", q.to_pg_string()),
            Self::FollowedBy {
                distance,
                left,
                right,
            } => {
                if *distance == 1 {
                    format!("({} <-> {})", left.to_pg_string(), right.to_pg_string())
                } else {
                    format!(
                        "({} <{}> {})",
                        left.to_pg_string(),
                        distance,
                        right.to_pg_string()
                    )
                }
            }
        }
    }

    /// 从 PG 文本格式解析：`'hello & world'` 或 `'hello world'`
    ///
    /// 简化语法：
    /// - 词素由空白或 `&` 分隔
    /// - `|` 表示 OR
    /// - `!` 表示 NOT
    /// - `<->` 表示 FOLLOWED BY 1
    /// - 单词直接构造 Lexeme
    pub fn parse(s: &str) -> Result<Self, String> {
        let tokens = Self::tokenize(s)?;
        if tokens.is_empty() {
            return Ok(Self::Empty);
        }
        let mut parser = TsQueryParser { tokens, pos: 0 };
        let result = parser.parse_or()?;
        if parser.pos != parser.tokens.len() {
            return Err(format!(
                "unexpected trailing tokens at pos {}: {:?}",
                parser.pos,
                &parser.tokens[parser.pos..]
            ));
        }
        Ok(result)
    }

    /// 词法分析：切分为操作符 / 词素 token
    fn tokenize(s: &str) -> Result<Vec<TsQueryToken>, String> {
        let mut tokens = Vec::new();
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c.is_whitespace() {
                i += 1;
                continue;
            }
            match c {
                '&' => {
                    tokens.push(TsQueryToken::And);
                    i += 1;
                }
                '|' => {
                    tokens.push(TsQueryToken::Or);
                    i += 1;
                }
                '!' => {
                    tokens.push(TsQueryToken::Not);
                    i += 1;
                }
                '(' => {
                    tokens.push(TsQueryToken::LParen);
                    i += 1;
                }
                ')' => {
                    tokens.push(TsQueryToken::RParen);
                    i += 1;
                }
                '<' => {
                    // <-> 或 <N>
                    if i + 2 < bytes.len() && bytes[i + 1] == b'-' && bytes[i + 2] == b'>' {
                        tokens.push(TsQueryToken::FollowedBy(1));
                        i += 3;
                    } else if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                        // <N>
                        let mut j = i + 1;
                        while j < bytes.len() && bytes[j].is_ascii_digit() {
                            j += 1;
                        }
                        let num_str = std::str::from_utf8(&bytes[i + 1..j])
                            .map_err(|e| format!("invalid UTF-8 in number: {e}"))?;
                        let n: u32 = num_str
                            .parse()
                            .map_err(|e| format!("invalid distance '{num_str}': {e}"))?;
                        // 期望后面是 '>'
                        if j < bytes.len() && bytes[j] == b'>' {
                            tokens.push(TsQueryToken::FollowedBy(n));
                            i = j + 1;
                        } else {
                            return Err(format!("expected '>' after distance {n}"));
                        }
                    } else {
                        return Err(format!("unexpected '<' at pos {i}"));
                    }
                }
                _ => {
                    // 词素：读取直到遇到操作符字符或空白
                    let start = i;
                    while i < bytes.len() {
                        let ch = bytes[i] as char;
                        if ch.is_whitespace() || matches!(ch, '&' | '|' | '!' | '(' | ')' | '<') {
                            break;
                        }
                        i += 1;
                    }
                    let term = std::str::from_utf8(&bytes[start..i])
                        .map_err(|e| format!("invalid UTF-8 in term: {e}"))?
                        .to_string();
                    // 解析可选权重 `term:ABC`
                    let (term, weights) = if let Some(colon_idx) = term.find(':') {
                        let t = term[..colon_idx].to_string();
                        let w_str = &term[colon_idx + 1..];
                        let mut w = 0u8;
                        for wc in w_str.chars() {
                            match wc {
                                'A' | 'a' => w |= TS_WEIGHT_A,
                                'B' | 'b' => w |= TS_WEIGHT_B,
                                'C' | 'c' => w |= TS_WEIGHT_C,
                                'D' | 'd' => w |= TS_WEIGHT_D,
                                _ => return Err(format!("invalid weight char '{wc}' in {term}")),
                            }
                        }
                        (t, w)
                    } else {
                        (term, 0u8)
                    };
                    if term.is_empty() {
                        return Err(format!("empty term at pos {start}"));
                    }
                    tokens.push(TsQueryToken::Lexeme {
                        term: term.to_lowercase(),
                        weights,
                    });
                }
            }
        }
        Ok(tokens)
    }
}

/// 收集子查询在 tsvector 中匹配的所有位置（用于 FollowedBy 距离判断）
///
/// 作为自由函数实现以避免 `&self` 仅在递归中传递的 clippy 警告
/// （`only_used_in_recursion`）。
fn collect_match_positions(q: &TsQuery, ts: &TsVector) -> Vec<u32> {
    match q {
        TsQuery::Lexeme { term, weights } => {
            if let Ok(idx) = ts
                .lexemes
                .binary_search_by_key(&term.as_str(), |l| l.term.as_str())
            {
                let lex = &ts.lexemes[idx];
                lex.positions
                    .iter()
                    .filter(|p| *weights == 0 || p.weight & weights != 0)
                    .map(|p| p.position)
                    .collect()
            } else {
                Vec::new()
            }
        }
        // 对于组合查询，递归收集子位置（简化：未实际过滤权重，
        // 但 FollowedBy 的常见用例 `a <1> b` 已正确支持）
        TsQuery::And(l, r) => {
            let mut v = collect_match_positions(l, ts);
            v.extend(collect_match_positions(r, ts));
            v
        }
        TsQuery::Or(l, r) => {
            let mut v = collect_match_positions(l, ts);
            v.extend(collect_match_positions(r, ts));
            v
        }
        TsQuery::Not(_) => Vec::new(),
        TsQuery::FollowedBy {
            left,
            right,
            distance,
        } => {
            // 递归处理嵌套 FollowedBy：返回右子查询位置（简化）
            let _ = (left, distance);
            collect_match_positions(right, ts)
        }
        TsQuery::Empty => Vec::new(),
    }
}

/// tsquery 词法 token
#[derive(Debug, Clone, PartialEq, Eq)]
enum TsQueryToken {
    Lexeme { term: String, weights: u8 },
    And,
    Or,
    Not,
    LParen,
    RParen,
    FollowedBy(u32),
}

/// tsquery 简单递归下降解析器
struct TsQueryParser {
    tokens: Vec<TsQueryToken>,
    pos: usize,
}

impl TsQueryParser {
    fn peek(&self) -> Option<&TsQueryToken> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<TsQueryToken> {
        if self.pos < self.tokens.len() {
            let t = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(t)
        } else {
            None
        }
    }

    /// parse_or := parse_and ( '|' parse_and )*
    fn parse_or(&mut self) -> Result<TsQuery, String> {
        let mut left = self.parse_and()?;
        while let Some(TsQueryToken::Or) = self.peek() {
            self.next();
            let right = self.parse_and()?;
            left = TsQuery::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// parse_and := parse_followed_by ( '&' parse_followed_by )*
    ///             | parse_followed_by '&' '!' parse_followed_by  (PG `& !` 语义)
    fn parse_and(&mut self) -> Result<TsQuery, String> {
        let mut left = self.parse_followed_by()?;
        while let Some(TsQueryToken::And) = self.peek() {
            self.next();
            let right = self.parse_followed_by()?;
            left = TsQuery::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// parse_followed_by := parse_unary ( ('<->' | '<N>') parse_unary )*
    fn parse_followed_by(&mut self) -> Result<TsQuery, String> {
        let mut left = self.parse_unary()?;
        while let Some(TsQueryToken::FollowedBy(n)) = self.peek() {
            let n = *n;
            self.next();
            let right = self.parse_unary()?;
            left = TsQuery::FollowedBy {
                distance: n,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// parse_unary := '!' parse_unary | '(' parse_or ')' | Lexeme
    fn parse_unary(&mut self) -> Result<TsQuery, String> {
        match self.peek() {
            Some(TsQueryToken::Not) => {
                self.next();
                let inner = self.parse_unary()?;
                Ok(TsQuery::Not(Box::new(inner)))
            }
            Some(TsQueryToken::LParen) => {
                self.next();
                let inner = self.parse_or()?;
                match self.next() {
                    Some(TsQueryToken::RParen) => Ok(inner),
                    other => Err(format!("expected ')' got {other:?}")),
                }
            }
            Some(TsQueryToken::Lexeme { .. }) => {
                if let Some(TsQueryToken::Lexeme { term, weights }) = self.next() {
                    Ok(TsQuery::Lexeme { term, weights })
                } else {
                    Err("unreachable".into())
                }
            }
            other => Err(format!("expected lexeme/!/( got {other:?}")),
        }
    }
}

// =====================================================================
//  向量类型（Phase P4-5）
// =====================================================================

/// AI 嵌入向量 — 存储为 `Vec<f64>`，支持相似度运算
///
/// 语法：`CAST('[1.0, 2.0, 3.0]' AS VECTOR(3))` 或列定义 `emb VECTOR(3)`。
/// 维度在 `ColumnType::Vector(dims)` 中声明，运行时校验实际向量维度匹配。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorValue {
    /// 向量分量（f64 列表）
    pub data: Vec<f64>,
}

impl VectorValue {
    /// 从 f64 向量构造
    pub fn new(data: Vec<f64>) -> Self {
        Self { data }
    }

    /// 维度（分量数）
    pub fn dims(&self) -> usize {
        self.data.len()
    }

    /// 从文本字面量解析 — 支持 `'[1.0, 2.0, 3.0]'` 和 `'[1,2,3]'` 格式
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if !s.starts_with('[') || !s.ends_with(']') {
            return Err(format!("vector literal must be bracketed: {s}"));
        }
        let inner = s[1..s.len() - 1].trim();
        if inner.is_empty() {
            return Ok(Self { data: Vec::new() });
        }
        let mut data = Vec::new();
        for part in inner.split(',') {
            let trimmed = part.trim();
            let v: f64 = trimmed.parse().map_err(|e| {
                format!("cannot parse '{trimmed}' as f64 in vector: {e}")
            })?;
            data.push(v);
        }
        Ok(Self { data })
    }

    /// 格式化为 `[1.0, 2.0, 3.0]` 形式
    pub fn to_string(&self) -> String {
        format!(
            "[{}]",
            self.data
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    /// 余弦距离：`1 - cos(θ)`，范围 [0, 2]，越小越相似
    pub fn cosine_distance(&self, other: &Self) -> f64 {
        if self.data.is_empty() || other.data.is_empty() {
            return f64::NAN;
        }
        let dot: f64 = self.data.iter().zip(other.data.iter()).map(|(a, b)| a * b).sum();
        let mag_a: f64 = self.data.iter().map(|a| a * a).sum::<f64>().sqrt();
        let mag_b: f64 = other.data.iter().map(|b| b * b).sum::<f64>().sqrt();
        if mag_a == 0.0 || mag_b == 0.0 {
            return f64::NAN;
        }
        1.0 - dot / (mag_a * mag_b)
    }

    /// L2（欧氏）距离：`sqrt(Σ(aᵢ - bᵢ)²)`
    pub fn l2_distance(&self, other: &Self) -> f64 {
        self.data
            .iter()
            .zip(other.data.iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            .sqrt()
    }

    /// 点积（内积）：`Σ(aᵢ × bᵢ)`
    pub fn dot_product(&self, other: &Self) -> f64 {
        self.data.iter().zip(other.data.iter()).map(|(a, b)| a * b).sum()
    }
}

// =====================================================================
//  列类型（用于类型转换的目标类型）
// =====================================================================
///
/// 注意：与 `Value` 不同，`ColumnType` 描述的是"类型"而不是"值"。
/// 例如 `ColumnType::Decimal { precision: 10, scale: 2 }` 描述一个
/// DECIMAL(10,2) 列，而 `Value::Decimal(12345, 2)` 是该列中的一个具体值。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColumnType {
    Null,
    Int64,
    Float64,
    Text,
    Blob,
    Bool,
    Date,
    Timestamp,
    /// DECIMAL(precision, scale)
    Decimal {
        /// 总位数（整数 + 小数部分）
        precision: u8,
        /// 小数位数
        scale: u8,
    },
    /// 数组元素类型包裹在 Box 中（递归类型）
    Array(Box<ColumnType>),
    /// ENUM 类型，附带可选值列表
    Enum(Vec<String>),
    /// 范围类型
    Range(RangeType),
    /// JSON / JSONB
    Json,
    /// PG tsvector — 全文检索文档向量（Phase 3.33）
    TsVector,
    /// PG tsquery — 全文检索查询表达式（Phase 3.33）
    TsQuery,
    /// AI 嵌入向量 — `VECTOR(dims)`，dims 为维度数（Phase P4-5）
    Vector(usize),
    /// XML 文档 — SQL/XML 标准（Phase P4-2）
    Xml,
}

// =====================================================================
//  类型转换错误
// =====================================================================

/// 类型转换错误
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
pub enum CastError {
    /// 隐式转换不允许，需要显式 CAST
    #[error("implicit cast not allowed between these types")]
    ImplicitNotAllowed,
    /// 转换不可能（如非数字文本转 INT）
    #[error("cast impossible: {reason}")]
    Impossible {
        /// 失败原因
        reason: String,
    },
    /// 精度损失（显式转换可接受，隐式转换拒绝）
    #[error("precision loss: {detail}")]
    PrecisionLoss {
        /// 损失详情
        detail: String,
    },
    /// 溢出（目标类型无法表示该值）
    #[error("value overflow: {detail}")]
    Overflow {
        /// 溢出详情
        detail: String,
    },
}

// =====================================================================
//  类型转换实现
// =====================================================================

/// 每天对应的微秒数（86_400 秒 × 1_000_000）
const MICROS_PER_DAY: i64 = 86_400_000_000;

impl Value {
    /// 返回该值对应的 `ColumnType`
    ///
    /// 用于类型转换前的同类型检查（如 `CAST(x AS BIGINT)` 当 x 已是 Int64 时直接返回）。
    pub fn column_type(&self) -> ColumnType {
        match self {
            Value::Null => ColumnType::Null,
            Value::Int64(_) => ColumnType::Int64,
            Value::Float64(_) => ColumnType::Float64,
            Value::Text(_) => ColumnType::Text,
            Value::Blob(_) => ColumnType::Blob,
            Value::Bool(_) => ColumnType::Bool,
            Value::Date(_) => ColumnType::Date,
            Value::Timestamp(_) => ColumnType::Timestamp,
            Value::Decimal(_, scale) => ColumnType::Decimal {
                precision: 38,
                scale: *scale,
            },
            Value::Array(_) => ColumnType::Array(Box::new(ColumnType::Null)),
            Value::Enum(_) => ColumnType::Enum(Vec::new()),
            Value::Range(r) => ColumnType::Range(r.range_type),
            Value::Json(_) => ColumnType::Json,
            Value::TsVector(_) => ColumnType::TsVector,
            Value::TsQuery(_) => ColumnType::TsQuery,
            Value::Vector(v) => ColumnType::Vector(v.dims()),
            Value::Xml(_) => ColumnType::Xml,
        }
    }

    /// 隐式类型转换（自动提升，无精度损失）
    ///
    /// 仅允许"安全"的转换路径：
    /// - 整数 → 浮点 / 文本 / Decimal
    /// - 浮点 → 文本 / Decimal（用 round 消除表示误差）
    /// - 布尔 → 整数 / 文本
    /// - 文本 → 数值 / 日期 / 时间戳（解析失败返回 `Impossible`）
    /// - 日期 ↔ 时间戳（精确换算）
    /// - Decimal → 浮点 / 文本
    /// - Enum → Text
    /// - Null → 任何类型（仍为 Null）
    pub fn cast_implicit(self, target: &ColumnType) -> Result<Value, CastError> {
        // NULL 隐式转换为任何类型仍是 NULL
        if matches!(self, Value::Null) {
            return Ok(Value::Null);
        }
        // 同类型直接返回（如 Int64 → Int64）
        // 注意：Decimal 需比较 scale，所以单独处理
        if let Value::Decimal(_, scale) = &self {
            if let ColumnType::Decimal {
                scale: target_scale,
                ..
            } = target
            {
                if scale == target_scale {
                    return Ok(self);
                }
            }
        } else if self.column_type() == *target {
            return Ok(self);
        }
        match (self, target) {
            // Int64 → Float64
            (Value::Int64(v), ColumnType::Float64) => Ok(Value::Float64(v as f64)),
            // Int64 → Text
            (Value::Int64(v), ColumnType::Text) => Ok(Value::Text(v.to_string())),
            // Int64 → Decimal { scale }
            (Value::Int64(v), ColumnType::Decimal { scale, .. }) => {
                let scaled = i128::from(v)
                    .checked_mul(10_i128.pow(u32::from(*scale)))
                    .ok_or_else(|| CastError::Overflow {
                        detail: format!("Int64 {v} × 10^{scale} overflows i128"),
                    })?;
                Ok(Value::Decimal(scaled, *scale))
            }
            // Float64 → Text（Rust 默认 to_string 已是最短表示）
            (Value::Float64(v), ColumnType::Text) => Ok(Value::Text(v.to_string())),
            // Float64 → Decimal { scale }（用 round 消除浮点表示误差）
            (Value::Float64(v), ColumnType::Decimal { scale, .. }) => {
                let factor = 10_f64.powi(i32::from(*scale));
                Ok(Value::Decimal((v * factor).round() as i128, *scale))
            }
            // Bool → Int64
            (Value::Bool(b), ColumnType::Int64) => Ok(Value::Int64(if b {
                1
            } else {
                0
            })),
            // Bool → Text
            (Value::Bool(b), ColumnType::Text) => Ok(Value::Text(b.to_string())),
            // Text → Int64
            (Value::Text(s), ColumnType::Int64) => {
                s.parse::<i64>()
                    .map(Value::Int64)
                    .map_err(|_| CastError::Impossible {
                        reason: format!("cannot parse '{s}' as Int64"),
                    })
            }
            // Text → Float64
            (Value::Text(s), ColumnType::Float64) => {
                s.parse::<f64>()
                    .map(Value::Float64)
                    .map_err(|_| CastError::Impossible {
                        reason: format!("cannot parse '{s}' as Float64"),
                    })
            }
            // Text → Bool
            (Value::Text(s), ColumnType::Bool) => match s.to_lowercase().as_str() {
                "true" | "t" | "1" => Ok(Value::Bool(true)),
                "false" | "f" | "0" => Ok(Value::Bool(false)),
                _ => Err(CastError::Impossible {
                    reason: format!("cannot parse '{s}' as Bool"),
                }),
            },
            // Text → Date（ISO 8601 YYYY-MM-DD）
            (Value::Text(s), ColumnType::Date) => {
                parse_iso_date(&s)
                    .map(Value::Date)
                    .ok_or_else(|| CastError::Impossible {
                        reason: format!("cannot parse '{s}' as Date (expect YYYY-MM-DD)"),
                    })
            }
            // Text → Timestamp（RFC 3339 / ISO 8601）
            (Value::Text(s), ColumnType::Timestamp) => parse_iso_timestamp(&s)
                .map(Value::Timestamp)
                .ok_or_else(|| CastError::Impossible {
                    reason: format!("cannot parse '{s}' as Timestamp (expect RFC 3339)"),
                }),
            // Date → Timestamp（天 → 微秒）
            (Value::Date(days), ColumnType::Timestamp) => {
                let us = i64::from(days).checked_mul(MICROS_PER_DAY).ok_or_else(|| {
                    CastError::Overflow {
                        detail: format!("Date {days} × {MICROS_PER_DAY} overflows i64"),
                    }
                })?;
                Ok(Value::Timestamp(us))
            }
            // Date → Text
            (Value::Date(days), ColumnType::Text) => Ok(Value::Text(format_iso_date(days))),
            // Timestamp → Text
            (Value::Timestamp(us), ColumnType::Text) => Ok(Value::Text(format_iso_timestamp(us))),
            // Decimal → Float64
            (Value::Decimal(v, scale), ColumnType::Float64) => {
                let divisor = 10_f64.powi(i32::from(scale));
                Ok(Value::Float64(v as f64 / divisor))
            }
            // Decimal → Text
            (Value::Decimal(v, scale), ColumnType::Text) => {
                Ok(Value::Text(format_decimal(v, scale)))
            }
            // Enum → Text
            (Value::Enum(s), ColumnType::Text) => Ok(Value::Text(s)),
            // TsVector → Text（PG 文本格式）
            (Value::TsVector(t), ColumnType::Text) => Ok(Value::Text(t.to_pg_string())),
            // TsQuery → Text（PG 文本格式）
            (Value::TsQuery(q), ColumnType::Text) => Ok(Value::Text(q.to_pg_string())),
            // Vector → Text（`[1.0, 2.0, 3.0]` 格式）
            (Value::Vector(v), ColumnType::Text) => Ok(Value::Text(v.to_string())),
            // Xml → Text（XML 文档字符串）
            (Value::Xml(x), ColumnType::Text) => Ok(Value::Text(x)),
            // 其他组合 → 隐式不允许
            _ => Err(CastError::ImplicitNotAllowed),
        }
    }

    /// 显式类型转换（CAST 表达式，允许精度损失）
    ///
    /// 在隐式转换基础上额外允许：
    /// - 浮点 → 整数（截断小数）
    /// - 整数 → 日期 / 时间戳 / 布尔
    /// - 文本 ↔ Blob（UTF-8 编解码）
    /// - 文本 → Decimal / Json
    /// - Blob → 文本（要求合法 UTF-8）
    /// - 布尔 → 浮点
    /// - 日期/时间戳 → 整数
    /// - 时间戳 → 日期（按微秒截断到天）
    /// - Decimal → 整数（截断小数）
    /// - Json → 文本
    pub fn cast_explicit(self, target: &ColumnType) -> Result<Value, CastError> {
        // NULL 显式转换为任何类型仍是 NULL
        if matches!(self, Value::Null) {
            return Ok(Value::Null);
        }
        // 先尝试隐式（隐式是显式的子集，避免重复实现）
        let snapshot = self.clone();
        if let Ok(v) = snapshot.cast_implicit(target) {
            return Ok(v);
        }
        // 显式额外允许的转换路径
        match (self, target) {
            // Float64 → Int64（向零截断）
            (Value::Float64(v), ColumnType::Int64) => Ok(Value::Int64(v as i64)),
            // Float64 → Bool（非零为真）
            (Value::Float64(v), ColumnType::Bool) => Ok(Value::Bool(v != 0.0)),
            // 注：Float64 → Decimal 已由 cast_implicit 处理（先于显式分支短路），此处无需重复
            // Int64 → Date（整数视作自 epoch 的天数）
            (Value::Int64(v), ColumnType::Date) => Ok(Value::Date(v as i32)),
            // Int64 → Timestamp（整数视作微秒）
            (Value::Int64(v), ColumnType::Timestamp) => Ok(Value::Timestamp(v)),
            // Int64 → Bool（非零为真）
            (Value::Int64(v), ColumnType::Bool) => Ok(Value::Bool(v != 0)),
            // Text → Blob（UTF-8 编码）
            (Value::Text(s), ColumnType::Blob) => Ok(Value::Blob(s.into_bytes())),
            // Text → Decimal
            (Value::Text(s), ColumnType::Decimal { scale, .. }) => parse_decimal(&s, *scale)
                .map(|v| Value::Decimal(v, *scale))
                .ok_or_else(|| CastError::Impossible {
                    reason: format!("cannot parse '{s}' as Decimal(scale={scale})"),
                }),
            // Text → Json
            (Value::Text(s), ColumnType::Json) => serde_json::from_str::<serde_json::Value>(&s)
                .map(Value::Json)
                .map_err(|e| CastError::Impossible {
                    reason: format!("cannot parse JSON: {e}"),
                }),
            // Blob → Text（要求合法 UTF-8）
            (Value::Blob(b), ColumnType::Text) => {
                String::from_utf8(b)
                    .map(Value::Text)
                    .map_err(|_| CastError::Impossible {
                        reason: "invalid UTF-8 in Blob".to_string(),
                    })
            }
            // Bool → Float64
            (Value::Bool(b), ColumnType::Float64) => Ok(Value::Float64(if b {
                1.0
            } else {
                0.0
            })),
            // Date → Int64
            (Value::Date(d), ColumnType::Int64) => Ok(Value::Int64(i64::from(d))),
            // Timestamp → Int64
            (Value::Timestamp(us), ColumnType::Int64) => Ok(Value::Int64(us)),
            // Timestamp → Date（微秒截断到天）
            (Value::Timestamp(us), ColumnType::Date) => {
                let days = us.div_euclid(MICROS_PER_DAY) as i32;
                Ok(Value::Date(days))
            }
            // Decimal → Int64（向零截断小数部分）
            (Value::Decimal(v, scale), ColumnType::Int64) => {
                let divisor = 10_i128.pow(u32::from(scale));
                Ok(Value::Int64((v / divisor) as i64))
            }
            // Json → Text
            (Value::Json(j), ColumnType::Text) => serde_json::to_string(&j)
                .map(Value::Text)
                .map_err(|e| CastError::Impossible {
                    reason: format!("cannot serialize JSON: {e}"),
                }),
            // Text → TsVector（PG 文本格式解析）
            (Value::Text(s), ColumnType::TsVector) => TsVector::parse(&s)
                .map(Value::TsVector)
                .map_err(|e| CastError::Impossible {
                    reason: format!("cannot parse '{s}' as tsvector: {e}"),
                }),
            // Text → TsQuery（PG 文本格式解析）
            (Value::Text(s), ColumnType::TsQuery) => TsQuery::parse(&s)
                .map(Value::TsQuery)
                .map_err(|e| CastError::Impossible {
                    reason: format!("cannot parse '{s}' as tsquery: {e}"),
                }),
            // Text → Vector（`'[1.0, 2.0, 3.0]'` 格式解析）
            (Value::Text(s), ColumnType::Vector(_)) => VectorValue::parse(&s)
                .map(Value::Vector)
                .map_err(|e| CastError::Impossible {
                    reason: format!("cannot parse '{s}' as vector: {e}"),
                }),
            // Text → Xml（直接存储为 XML 文档字符串）
            (Value::Text(s), ColumnType::Xml) => Ok(Value::Xml(s)),
            // 其他组合即使显式也不允许（如 Int64 → Array）
            _ => Err(CastError::ImplicitNotAllowed),
        }
    }
}

// =====================================================================
//  格式化与解析辅助函数
// =====================================================================

/// 格式化 Decimal 为字符串：Decimal(12345, 2) → "123.45"
///
/// 规则：
/// - scale=0 时直接输出整数
/// - 小数部分按 scale 补前导零（如 Decimal(5, 3) → "0.005"）
/// - 负数符号在整数部分前（如 Decimal(-5, 3) → "-0.005"）
fn format_decimal(v: i128, scale: u8) -> String {
    if scale == 0 {
        return v.to_string();
    }
    let scale_u32 = u32::from(scale);
    let scale_usize = usize::from(scale);
    let divisor = 10_i128.pow(scale_u32);
    let int_part = v / divisor;
    let frac_part = (v % divisor).abs();
    // 当值为负且整数部分为 0 时，int_part.to_string() 不含负号，需手动补上
    let sign = if v < 0 && int_part == 0 {
        "-"
    } else {
        ""
    };
    format!("{sign}{int_part}.{frac_part:0scale_usize$}")
}

/// 解析十进制字符串为 i128 unscaled value
///
/// 例如 `parse_decimal("123.45", 2)` 返回 `Some(12345)`
/// - 支持前导 +/-
/// - 小数位数不足 scale 时补零（"1.2" scale=4 → 12000）
/// - 小数位数超过 scale 时截断（"1.2345" scale=2 → 123，注意不是四舍五入）
fn parse_decimal(s: &str, scale: u8) -> Option<i128> {
    let s = s.trim();
    let (neg, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };

    let (int_str, frac_str) = match s.find('.') {
        Some(pos) => (&s[..pos], &s[pos + 1..]),
        None => (s, ""),
    };

    let int_val: i128 = if int_str.is_empty() {
        0
    } else {
        int_str.parse().ok()?
    };

    let scale_u32 = u32::from(scale);
    let scale_divisor = 10_i128.pow(scale_u32);

    // 计算小数部分对 unscaled value 的贡献
    let frac_val: i128 = if frac_str.is_empty() {
        0
    } else if frac_str.len() >= scale as usize {
        // 截断到 scale 位
        frac_str[..scale as usize].parse().ok()?
    } else {
        // 不足 scale 位，需要补零
        let padded = format!("{:0<width$}", frac_str, width = scale as usize);
        padded.parse().ok()?
    };

    let mut result = int_val.checked_mul(scale_divisor)?.checked_add(frac_val)?;
    if neg {
        result = -result;
    }
    Some(result)
}

/// 解析 ISO 8601 日期 `YYYY-MM-DD` 为自 1970-01-01 起的天数
fn parse_iso_date(s: &str) -> Option<i32> {
    use chrono::NaiveDate;
    let date = NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()?;
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
    Some(date.signed_duration_since(epoch).num_days() as i32)
}

/// 将自 1970-01-01 起的天数格式化为 ISO 8601 日期 `YYYY-MM-DD`
///
/// 对于超出 chrono `NaiveDate` 表示范围的极端值，返回占位字符串而非 panic。
fn format_iso_date(days: i32) -> String {
    use chrono::NaiveDate;
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("1970-01-01 is always valid");
    let days_i64 = i64::from(days);
    let abs_days = days_i64.unsigned_abs();
    let date = if days_i64 >= 0 {
        epoch.checked_add_days(chrono::Days::new(abs_days))
    } else {
        epoch.checked_sub_days(chrono::Days::new(abs_days))
    };
    match date {
        Some(d) => d.format("%Y-%m-%d").to_string(),
        None => format!("<invalid date: {days} days>"),
    }
}

/// 解析 RFC 3339 / ISO 8601 时间戳为微秒精度 UTC 时间戳
///
/// 支持的格式：
/// - `1970-01-01T00:00:00Z`（RFC 3339 UTC）
/// - `1970-01-01T00:00:00+00:00`（RFC 3339 带偏移）
/// - `1970-01-01T00:00:00`（无时区，按 UTC 处理）
fn parse_iso_timestamp(s: &str) -> Option<i64> {
    use chrono::{DateTime, NaiveDateTime};
    let s = s.trim();
    // 优先尝试 RFC 3339（带时区）
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_micros());
    }
    // 无时区格式：支持 T 或空格分隔符（MySQL/PostgreSQL 常见格式 '2026-03-02 09:40:10'）
    // 将首个空格替换为 T，统一为 ISO 8601 格式
    let normalized = if s.contains(' ') && !s.contains('T') {
        s.replacen(' ', "T", 1)
    } else {
        s.to_string()
    };
    // 尝试带小数秒的格式（如 2026-03-02T09:40:10.123456）
    if let Ok(dt) = NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(dt.and_utc().timestamp_micros());
    }
    // 尝试不带小数秒的格式
    if let Ok(dt) = NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S") {
        return Some(dt.and_utc().timestamp_micros());
    }
    None
}

/// 将微秒精度 UTC 时间戳格式化为 RFC 3339 字符串 `YYYY-MM-DDTHH:MM:SSZ`
fn format_iso_timestamp(us: i64) -> String {
    use chrono::{DateTime, Utc};
    let secs = us.div_euclid(1_000_000);
    let nanos = us.rem_euclid(1_000_000) as u32 * 1_000;
    match DateTime::<Utc>::from_timestamp(secs, nanos) {
        Some(dt) => dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        None => format!("<invalid timestamp: {us} μs>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =================================================================
    //  构造测试 — 验证每个变体都能正确构造
    // =================================================================

    #[test]
    fn construct_null() {
        let v = Value::Null;
        assert!(matches!(v, Value::Null));
    }

    #[test]
    fn construct_int64() {
        let v = Value::Int64(42);
        assert!(matches!(v, Value::Int64(42)));
    }

    #[test]
    fn construct_float64() {
        let v = Value::Float64(2.5);
        assert!(matches!(v, Value::Float64(x) if (x - 2.5).abs() < f64::EPSILON));
    }

    #[test]
    fn construct_text() {
        let v = Value::Text("hello".to_string());
        assert!(matches!(v, Value::Text(s) if s == "hello"));
    }

    #[test]
    fn construct_blob() {
        let v = Value::Blob(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(matches!(v, Value::Blob(b) if b == vec![0xDE, 0xAD, 0xBE, 0xEF]));
    }

    #[test]
    fn construct_bool() {
        assert!(matches!(Value::Bool(true), Value::Bool(true)));
        assert!(matches!(Value::Bool(false), Value::Bool(false)));
    }

    #[test]
    fn construct_date() {
        // 1970-01-01 = 0 偏移；2026-01-01 ≈ 20454 天
        let v = Value::Date(20454);
        assert!(matches!(v, Value::Date(20454)));
    }

    #[test]
    fn construct_timestamp() {
        // 微秒精度：1 秒 = 1_000_000 微秒
        let v = Value::Timestamp(1_700_000_000_000_000);
        assert!(matches!(v, Value::Timestamp(1_700_000_000_000_000)));
    }

    #[test]
    fn construct_decimal() {
        // 123.45 = Decimal(12345, 2)
        let v = Value::Decimal(12345, 2);
        assert!(matches!(v, Value::Decimal(12345, 2)));
    }

    #[test]
    fn construct_array() {
        let v = Value::Array(vec![Value::Int64(1), Value::Int64(2), Value::Int64(3)]);
        assert!(matches!(&v, Value::Array(arr) if arr.len() == 3));
    }

    #[test]
    fn construct_enum_value() {
        let v = Value::Enum("active".to_string());
        assert!(matches!(v, Value::Enum(s) if s == "active"));
    }

    #[test]
    fn construct_range() {
        let r = RangeValue {
            lower: Some(Box::new(Value::Int64(1))),
            upper: Some(Box::new(Value::Int64(10))),
            lower_inc: true,
            upper_inc: false,
            range_type: RangeType::Int4Range,
        };
        let v = Value::Range(r);
        assert!(matches!(v, Value::Range(_)));
    }

    #[test]
    fn construct_json() {
        let v = Value::Json(serde_json::json!({"key": "value", "n": 42}));
        assert!(matches!(v, Value::Json(_)));
    }

    // =================================================================
    //  比较测试 — PartialEq 行为
    // =================================================================

    #[test]
    fn equality_null_equals_null() {
        assert_eq!(Value::Null, Value::Null);
    }

    #[test]
    fn equality_int64_same_value() {
        assert_eq!(Value::Int64(42), Value::Int64(42));
        assert_ne!(Value::Int64(42), Value::Int64(43));
    }

    #[test]
    fn equality_float64_nan_is_not_nan() {
        // IEEE 754 标准：NaN != NaN（数据库通常遵循此行为）
        assert_ne!(Value::Float64(f64::NAN), Value::Float64(f64::NAN));
    }

    #[test]
    fn equality_float64_infinity() {
        assert_eq!(Value::Float64(f64::INFINITY), Value::Float64(f64::INFINITY));
        assert_eq!(
            Value::Float64(f64::NEG_INFINITY),
            Value::Float64(f64::NEG_INFINITY)
        );
        assert_ne!(
            Value::Float64(f64::INFINITY),
            Value::Float64(f64::NEG_INFINITY)
        );
    }

    #[test]
    fn equality_text_case_sensitive() {
        assert_eq!(
            Value::Text("abc".to_string()),
            Value::Text("abc".to_string())
        );
        assert_ne!(
            Value::Text("abc".to_string()),
            Value::Text("ABC".to_string())
        );
    }

    #[test]
    fn equality_blob_byte_wise() {
        assert_eq!(Value::Blob(vec![1, 2, 3]), Value::Blob(vec![1, 2, 3]));
        assert_ne!(Value::Blob(vec![1, 2, 3]), Value::Blob(vec![1, 2, 4]));
    }

    #[test]
    fn equality_bool_strict() {
        assert_eq!(Value::Bool(true), Value::Bool(true));
        assert_ne!(Value::Bool(true), Value::Bool(false));
    }

    #[test]
    fn equality_decimal_same_scale() {
        assert_eq!(Value::Decimal(12345, 2), Value::Decimal(12345, 2));
        assert_ne!(Value::Decimal(12345, 2), Value::Decimal(12345, 3));
    }

    #[test]
    fn equality_array_element_wise() {
        let a = Value::Array(vec![Value::Int64(1), Value::Int64(2)]);
        let b = Value::Array(vec![Value::Int64(1), Value::Int64(2)]);
        let c = Value::Array(vec![Value::Int64(1), Value::Int64(3)]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn equality_json_deep_compare() {
        let a = Value::Json(serde_json::json!({"a": 1, "b": [2, 3]}));
        let b = Value::Json(serde_json::json!({"a": 1, "b": [2, 3]}));
        let c = Value::Json(serde_json::json!({"a": 1, "b": [2, 4]}));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn equality_cross_variant_never_equal() {
        // 不同变体之间永不相等（强类型语义）
        assert_ne!(Value::Int64(1), Value::Float64(1.0));
        assert_ne!(Value::Text("1".to_string()), Value::Int64(1));
        assert_ne!(Value::Bool(true), Value::Int64(1));
    }

    // =================================================================
    //  序列化测试 — serde 双向一致性
    // =================================================================

    #[test]
    fn serde_null_roundtrip() {
        let v = Value::Null;
        let json = serde_json::to_string(&v).expect("serialize Null");
        let back: Value = serde_json::from_str(&json).expect("deserialize Null");
        assert_eq!(v, back);
    }

    #[test]
    fn serde_int64_roundtrip() {
        let v = Value::Int64(42);
        let json = serde_json::to_string(&v).expect("serialize Int64");
        let back: Value = serde_json::from_str(&json).expect("deserialize Int64");
        assert_eq!(v, back);
    }

    #[test]
    fn serde_text_roundtrip() {
        let v = Value::Text("hello 世界 🌍".to_string());
        let json = serde_json::to_string(&v).expect("serialize Text");
        let back: Value = serde_json::from_str(&json).expect("deserialize Text");
        assert_eq!(v, back);
    }

    #[test]
    fn serde_blob_roundtrip() {
        let v = Value::Blob(vec![0x00, 0xFF, 0xDE, 0xAD]);
        let json = serde_json::to_string(&v).expect("serialize Blob");
        let back: Value = serde_json::from_str(&json).expect("deserialize Blob");
        assert_eq!(v, back);
    }

    #[test]
    fn serde_bool_roundtrip() {
        for b in [true, false] {
            let v = Value::Bool(b);
            let json = serde_json::to_string(&v).expect("serialize Bool");
            let back: Value = serde_json::from_str(&json).expect("deserialize Bool");
            assert_eq!(v, back);
        }
    }

    #[test]
    fn serde_date_roundtrip() {
        let v = Value::Date(20454);
        let json = serde_json::to_string(&v).expect("serialize Date");
        let back: Value = serde_json::from_str(&json).expect("deserialize Date");
        assert_eq!(v, back);
    }

    #[test]
    fn serde_timestamp_roundtrip() {
        let v = Value::Timestamp(1_700_000_000_000_000);
        let json = serde_json::to_string(&v).expect("serialize Timestamp");
        let back: Value = serde_json::from_str(&json).expect("deserialize Timestamp");
        assert_eq!(v, back);
    }

    #[test]
    fn serde_decimal_roundtrip() {
        let v = Value::Decimal(12345, 2);
        let json = serde_json::to_string(&v).expect("serialize Decimal");
        let back: Value = serde_json::from_str(&json).expect("deserialize Decimal");
        assert_eq!(v, back);
    }

    #[test]
    fn serde_array_roundtrip() {
        let v = Value::Array(vec![
            Value::Int64(1),
            Value::Text("x".to_string()),
            Value::Null,
        ]);
        let json = serde_json::to_string(&v).expect("serialize Array");
        let back: Value = serde_json::from_str(&json).expect("deserialize Array");
        assert_eq!(v, back);
    }

    #[test]
    fn serde_enum_roundtrip() {
        let v = Value::Enum("active".to_string());
        let json = serde_json::to_string(&v).expect("serialize Enum");
        let back: Value = serde_json::from_str(&json).expect("deserialize Enum");
        assert_eq!(v, back);
    }

    #[test]
    fn serde_range_roundtrip() {
        let v = Value::Range(RangeValue {
            lower: Some(Box::new(Value::Int64(1))),
            upper: Some(Box::new(Value::Int64(10))),
            lower_inc: true,
            upper_inc: false,
            range_type: RangeType::Int4Range,
        });
        let json = serde_json::to_string(&v).expect("serialize Range");
        let back: Value = serde_json::from_str(&json).expect("deserialize Range");
        assert_eq!(v, back);
    }

    #[test]
    fn serde_json_roundtrip() {
        let v = Value::Json(serde_json::json!({"a": 1, "b": [2.5, 3.5], "c": null}));
        let json = serde_json::to_string(&v).expect("serialize Json");
        let back: Value = serde_json::from_str(&json).expect("deserialize Json");
        assert_eq!(v, back);
    }

    // =================================================================
    //  边界值测试
    // =================================================================

    #[test]
    fn boundary_i64_max_min() {
        let max = Value::Int64(i64::MAX);
        let min = Value::Int64(i64::MIN);
        let json_max = serde_json::to_string(&max).expect("serialize i64::MAX");
        let json_min = serde_json::to_string(&min).expect("serialize i64::MIN");
        let back_max: Value = serde_json::from_str(&json_max).expect("deserialize i64::MAX");
        let back_min: Value = serde_json::from_str(&json_min).expect("deserialize i64::MIN");
        assert_eq!(max, back_max);
        assert_eq!(min, back_min);
        assert_ne!(max, min);
    }

    #[test]
    fn boundary_f64_max_min() {
        let max = Value::Float64(f64::MAX);
        let min = Value::Float64(f64::MIN_POSITIVE);
        let json_max = serde_json::to_string(&max).expect("serialize f64::MAX");
        let json_min = serde_json::to_string(&min).expect("serialize f64::MIN_POSITIVE");
        let back_max: Value = serde_json::from_str(&json_max).expect("deserialize f64::MAX");
        let back_min: Value =
            serde_json::from_str(&json_min).expect("deserialize f64::MIN_POSITIVE");
        assert_eq!(max, back_max);
        assert_eq!(min, back_min);
    }

    #[test]
    fn boundary_decimal_i128_max_min() {
        let max = Value::Decimal(i128::MAX, 0);
        let min = Value::Decimal(i128::MIN, 0);
        let json_max = serde_json::to_string(&max).expect("serialize i128::MAX");
        let json_min = serde_json::to_string(&min).expect("serialize i128::MIN");
        let back_max: Value = serde_json::from_str(&json_max).expect("deserialize i128::MAX");
        let back_min: Value = serde_json::from_str(&json_min).expect("deserialize i128::MIN");
        assert_eq!(max, back_max);
        assert_eq!(min, back_min);
    }

    #[test]
    fn boundary_decimal_max_scale() {
        // i128 最大支持 38 位十进制数字，scale 上限为 u8::MAX 但语义上无意义
        // 测试 scale = 38（接近上限）
        let v = Value::Decimal(1, 38);
        let json = serde_json::to_string(&v).expect("serialize Decimal(1, 38)");
        let back: Value = serde_json::from_str(&json).expect("deserialize Decimal(1, 38)");
        assert_eq!(v, back);
    }

    #[test]
    fn boundary_empty_text_and_blob() {
        let empty_text = Value::Text(String::new());
        let empty_blob = Value::Blob(Vec::new());
        let json_t = serde_json::to_string(&empty_text).expect("serialize empty Text");
        let json_b = serde_json::to_string(&empty_blob).expect("serialize empty Blob");
        let back_t: Value = serde_json::from_str(&json_t).expect("deserialize empty Text");
        let back_b: Value = serde_json::from_str(&json_b).expect("deserialize empty Blob");
        assert_eq!(empty_text, back_t);
        assert_eq!(empty_blob, back_b);
    }

    #[test]
    fn boundary_empty_array() {
        let v = Value::Array(Vec::new());
        let json = serde_json::to_string(&v).expect("serialize empty Array");
        let back: Value = serde_json::from_str(&json).expect("deserialize empty Array");
        assert_eq!(v, back);
    }

    #[test]
    fn boundary_nested_array() {
        // 嵌套数组：[[1, 2], [3, 4]]
        let v = Value::Array(vec![
            Value::Array(vec![Value::Int64(1), Value::Int64(2)]),
            Value::Array(vec![Value::Int64(3), Value::Int64(4)]),
        ]);
        let json = serde_json::to_string(&v).expect("serialize nested Array");
        let back: Value = serde_json::from_str(&json).expect("deserialize nested Array");
        assert_eq!(v, back);
    }

    #[test]
    fn boundary_deeply_nested_json() {
        // 深度嵌套 JSON：{"a": {"b": {"c": {"d": [1, 2, 3]}}}}
        let v = Value::Json(serde_json::json!({
            "a": {"b": {"c": {"d": [1, 2, 3]}}}
        }));
        let json = serde_json::to_string(&v).expect("serialize nested JSON");
        let back: Value = serde_json::from_str(&json).expect("deserialize nested JSON");
        assert_eq!(v, back);
    }

    #[test]
    fn boundary_unbounded_range() {
        // 无界范围：(−∞, +∞)
        let v = Value::Range(RangeValue {
            lower: None,
            upper: None,
            lower_inc: false,
            upper_inc: false,
            range_type: RangeType::NumRange,
        });
        let json = serde_json::to_string(&v).expect("serialize unbounded Range");
        let back: Value = serde_json::from_str(&json).expect("deserialize unbounded Range");
        assert_eq!(v, back);
    }

    #[test]
    fn boundary_large_text() {
        // 1MB 文本
        let large = "x".repeat(1024 * 1024);
        let v = Value::Text(large.clone());
        let json = serde_json::to_string(&v).expect("serialize large Text");
        let back: Value = serde_json::from_str(&json).expect("deserialize large Text");
        assert_eq!(v, back);
    }

    // =================================================================
    //  ColumnType 测试
    // =================================================================

    #[test]
    fn column_type_equality() {
        assert_eq!(ColumnType::Int64, ColumnType::Int64);
        assert_ne!(ColumnType::Int64, ColumnType::Float64);
        assert_eq!(
            ColumnType::Decimal {
                precision: 10,
                scale: 2
            },
            ColumnType::Decimal {
                precision: 10,
                scale: 2
            }
        );
        assert_ne!(
            ColumnType::Decimal {
                precision: 10,
                scale: 2
            },
            ColumnType::Decimal {
                precision: 10,
                scale: 3
            }
        );
    }

    #[test]
    fn column_type_serde_roundtrip() {
        let types = vec![
            ColumnType::Null,
            ColumnType::Int64,
            ColumnType::Float64,
            ColumnType::Text,
            ColumnType::Blob,
            ColumnType::Bool,
            ColumnType::Date,
            ColumnType::Timestamp,
            ColumnType::Decimal {
                precision: 38,
                scale: 10,
            },
            ColumnType::Array(Box::new(ColumnType::Int64)),
            ColumnType::Enum(vec!["a".to_string(), "b".to_string()]),
            ColumnType::Range(RangeType::Int4Range),
            ColumnType::Json,
        ];
        for ty in types {
            let json = serde_json::to_string(&ty).expect("serialize ColumnType");
            let back: ColumnType = serde_json::from_str(&json).expect("deserialize ColumnType");
            assert_eq!(ty, back, "ColumnType roundtrip failed: {ty:?}");
        }
    }

    // =================================================================
    //  CastError 测试
    // =================================================================

    #[test]
    fn cast_error_display() {
        let e = CastError::Impossible {
            reason: "not a number".to_string(),
        };
        assert!(format!("{e}").contains("not a number"));
        assert!(format!("{e}").contains("cast impossible"));

        let e2 = CastError::ImplicitNotAllowed;
        assert!(format!("{e2}").contains("implicit cast not allowed"));
    }

    #[test]
    fn cast_error_equality() {
        assert_eq!(CastError::ImplicitNotAllowed, CastError::ImplicitNotAllowed);
        assert_ne!(
            CastError::Impossible {
                reason: "a".to_string()
            },
            CastError::Impossible {
                reason: "b".to_string()
            }
        );
    }

    // =================================================================
    //  隐式类型转换测试（20 个组合）— 自动提升，无精度损失
    // =================================================================

    #[test]
    fn cast_implicit_int64_to_float64() {
        let v = Value::Int64(42).cast_implicit(&ColumnType::Float64);
        assert_eq!(v, Ok(Value::Float64(42.0)));
    }

    #[test]
    fn cast_implicit_int64_to_text() {
        let v = Value::Int64(42).cast_implicit(&ColumnType::Text);
        assert_eq!(v, Ok(Value::Text("42".to_string())));
    }

    #[test]
    fn cast_implicit_int64_to_decimal() {
        let v = Value::Int64(42).cast_implicit(&ColumnType::Decimal {
            precision: 10,
            scale: 2,
        });
        assert_eq!(v, Ok(Value::Decimal(4200, 2)));
    }

    #[test]
    fn cast_implicit_float64_to_text() {
        let v = Value::Float64(3.5).cast_implicit(&ColumnType::Text);
        assert_eq!(v, Ok(Value::Text("3.5".to_string())));
    }

    #[test]
    fn cast_implicit_float64_to_decimal() {
        let v = Value::Float64(1.25).cast_implicit(&ColumnType::Decimal {
            precision: 10,
            scale: 2,
        });
        assert_eq!(v, Ok(Value::Decimal(125, 2)));
    }

    #[test]
    fn cast_implicit_bool_to_int64() {
        assert_eq!(
            Value::Bool(true).cast_implicit(&ColumnType::Int64),
            Ok(Value::Int64(1))
        );
        assert_eq!(
            Value::Bool(false).cast_implicit(&ColumnType::Int64),
            Ok(Value::Int64(0))
        );
    }

    #[test]
    fn cast_implicit_bool_to_text() {
        assert_eq!(
            Value::Bool(true).cast_implicit(&ColumnType::Text),
            Ok(Value::Text("true".to_string()))
        );
        assert_eq!(
            Value::Bool(false).cast_implicit(&ColumnType::Text),
            Ok(Value::Text("false".to_string()))
        );
    }

    #[test]
    fn cast_implicit_text_to_int64() {
        let v = Value::Text("42".to_string()).cast_implicit(&ColumnType::Int64);
        assert_eq!(v, Ok(Value::Int64(42)));
    }

    #[test]
    fn cast_implicit_text_to_int64_invalid() {
        let v = Value::Text("not a number".to_string()).cast_implicit(&ColumnType::Int64);
        assert!(matches!(v, Err(CastError::Impossible { .. })));
    }

    #[test]
    fn cast_implicit_text_to_float64() {
        let v = Value::Text("2.5".to_string()).cast_implicit(&ColumnType::Float64);
        assert!(matches!(v, Ok(Value::Float64(x)) if (x - 2.5).abs() < f64::EPSILON));
    }

    #[test]
    fn cast_implicit_text_to_bool() {
        assert_eq!(
            Value::Text("true".to_string()).cast_implicit(&ColumnType::Bool),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            Value::Text("false".to_string()).cast_implicit(&ColumnType::Bool),
            Ok(Value::Bool(false))
        );
    }

    #[test]
    fn cast_implicit_text_to_date() {
        // ISO 8601 日期：1970-01-01 = 偏移 0
        let v = Value::Text("1970-01-01".to_string()).cast_implicit(&ColumnType::Date);
        assert_eq!(v, Ok(Value::Date(0)));
    }

    #[test]
    fn cast_implicit_text_to_timestamp() {
        // ISO 8601 时间戳：1970-01-01T00:00:00Z = 0 微秒
        let v =
            Value::Text("1970-01-01T00:00:00Z".to_string()).cast_implicit(&ColumnType::Timestamp);
        assert_eq!(v, Ok(Value::Timestamp(0)));
    }

    #[test]
    fn cast_implicit_date_to_timestamp() {
        // 1 天 = 86_400 秒 = 86_400_000_000 微秒
        let v = Value::Date(1).cast_implicit(&ColumnType::Timestamp);
        assert_eq!(v, Ok(Value::Timestamp(86_400_000_000)));
    }

    #[test]
    fn cast_implicit_date_to_text() {
        let v = Value::Date(0).cast_implicit(&ColumnType::Text);
        assert_eq!(v, Ok(Value::Text("1970-01-01".to_string())));
    }

    #[test]
    fn cast_implicit_timestamp_to_text() {
        let v = Value::Timestamp(0).cast_implicit(&ColumnType::Text);
        assert_eq!(v, Ok(Value::Text("1970-01-01T00:00:00Z".to_string())));
    }

    #[test]
    fn cast_implicit_decimal_to_float64() {
        let v = Value::Decimal(125, 2).cast_implicit(&ColumnType::Float64);
        assert!(matches!(v, Ok(Value::Float64(x)) if (x - 1.25).abs() < f64::EPSILON));
    }

    #[test]
    fn cast_implicit_decimal_to_text() {
        let v = Value::Decimal(12345, 2).cast_implicit(&ColumnType::Text);
        assert_eq!(v, Ok(Value::Text("123.45".to_string())));
    }

    #[test]
    fn cast_implicit_enum_to_text() {
        let v = Value::Enum("active".to_string()).cast_implicit(&ColumnType::Text);
        assert_eq!(v, Ok(Value::Text("active".to_string())));
    }

    #[test]
    fn cast_implicit_null_to_any_type() {
        // NULL 隐式转换为任何类型仍是 NULL
        assert_eq!(
            Value::Null.cast_implicit(&ColumnType::Int64),
            Ok(Value::Null)
        );
        assert_eq!(
            Value::Null.cast_implicit(&ColumnType::Text),
            Ok(Value::Null)
        );
        assert_eq!(
            Value::Null.cast_implicit(&ColumnType::Bool),
            Ok(Value::Null)
        );
    }

    #[test]
    fn cast_implicit_disallow_text_to_blob() {
        // Text → Blob 不是隐式转换（需要显式 CAST）
        let v = Value::Text("hello".to_string()).cast_implicit(&ColumnType::Blob);
        assert_eq!(v, Err(CastError::ImplicitNotAllowed));
    }

    // =================================================================
    //  显式类型转换测试（20 个组合）— 允许精度损失
    // =================================================================

    #[test]
    fn cast_explicit_float64_to_int64_truncate() {
        let v = Value::Float64(3.99).cast_explicit(&ColumnType::Int64);
        assert_eq!(v, Ok(Value::Int64(3)));
    }

    #[test]
    fn cast_explicit_float64_negative_to_int64_truncate() {
        let v = Value::Float64(-2.7).cast_explicit(&ColumnType::Int64);
        assert_eq!(v, Ok(Value::Int64(-2)));
    }

    #[test]
    fn cast_explicit_float64_to_bool() {
        assert_eq!(
            Value::Float64(1.5).cast_explicit(&ColumnType::Bool),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            Value::Float64(0.0).cast_explicit(&ColumnType::Bool),
            Ok(Value::Bool(false))
        );
    }

    #[test]
    fn cast_explicit_float64_to_decimal() {
        let v = Value::Float64(1.25).cast_explicit(&ColumnType::Decimal {
            precision: 10,
            scale: 2,
        });
        assert_eq!(v, Ok(Value::Decimal(125, 2)));
    }

    #[test]
    fn cast_explicit_int64_to_date() {
        // Int64 → Date：把整数当作自 1970-01-01 的天数偏移
        let v = Value::Int64(1).cast_explicit(&ColumnType::Date);
        assert_eq!(v, Ok(Value::Date(1)));
    }

    #[test]
    fn cast_explicit_int64_to_timestamp() {
        // Int64 → Timestamp：把整数当作微秒时间戳
        let v = Value::Int64(1_000_000).cast_explicit(&ColumnType::Timestamp);
        assert_eq!(v, Ok(Value::Timestamp(1_000_000)));
    }

    #[test]
    fn cast_explicit_int64_to_bool() {
        assert_eq!(
            Value::Int64(1).cast_explicit(&ColumnType::Bool),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            Value::Int64(0).cast_explicit(&ColumnType::Bool),
            Ok(Value::Bool(false))
        );
    }

    #[test]
    fn cast_explicit_text_to_blob() {
        let v = Value::Text("hello".to_string()).cast_explicit(&ColumnType::Blob);
        assert_eq!(v, Ok(Value::Blob(vec![b'h', b'e', b'l', b'l', b'o'])));
    }

    #[test]
    fn cast_explicit_blob_to_text() {
        let v = Value::Blob(vec![b'h', b'i']).cast_explicit(&ColumnType::Text);
        assert_eq!(v, Ok(Value::Text("hi".to_string())));
    }

    #[test]
    fn cast_explicit_blob_to_text_invalid_utf8() {
        let v = Value::Blob(vec![0xFF, 0xFE]).cast_explicit(&ColumnType::Text);
        assert!(matches!(v, Err(CastError::Impossible { .. })));
    }

    #[test]
    fn cast_explicit_text_to_json() {
        let v = Value::Text(r#"{"key": 42}"#.to_string()).cast_explicit(&ColumnType::Json);
        assert_eq!(v, Ok(Value::Json(serde_json::json!({"key": 42}))));
    }

    #[test]
    fn cast_explicit_text_to_json_invalid() {
        let v = Value::Text("{not valid json".to_string()).cast_explicit(&ColumnType::Json);
        assert!(matches!(v, Err(CastError::Impossible { .. })));
    }

    #[test]
    fn cast_explicit_json_to_text() {
        let v = Value::Json(serde_json::json!({"a": 1})).cast_explicit(&ColumnType::Text);
        let back = v.expect("json to text should succeed");
        // JSON 文本表示可能有不同顺序，解析回 JSON 比较更稳健
        assert!(
            matches!(back, Value::Text(s) if serde_json::from_str::<serde_json::Value>(&s).is_ok())
        );
    }

    #[test]
    fn cast_explicit_bool_to_float64() {
        assert_eq!(
            Value::Bool(true).cast_explicit(&ColumnType::Float64),
            Ok(Value::Float64(1.0))
        );
        assert_eq!(
            Value::Bool(false).cast_explicit(&ColumnType::Float64),
            Ok(Value::Float64(0.0))
        );
    }

    #[test]
    fn cast_explicit_date_to_int64() {
        let v = Value::Date(42).cast_explicit(&ColumnType::Int64);
        assert_eq!(v, Ok(Value::Int64(42)));
    }

    #[test]
    fn cast_explicit_timestamp_to_int64() {
        let v = Value::Timestamp(1_000_000).cast_explicit(&ColumnType::Int64);
        assert_eq!(v, Ok(Value::Int64(1_000_000)));
    }

    #[test]
    fn cast_explicit_timestamp_to_date() {
        // 86_400_000_000 微秒 = 1 天 → Date(1)
        let v = Value::Timestamp(86_400_000_000).cast_explicit(&ColumnType::Date);
        assert_eq!(v, Ok(Value::Date(1)));
    }

    #[test]
    fn cast_explicit_decimal_to_int64_truncate() {
        // Decimal(12345, 2) = 123.45 → 截断为 123
        let v = Value::Decimal(12345, 2).cast_explicit(&ColumnType::Int64);
        assert_eq!(v, Ok(Value::Int64(123)));
    }

    #[test]
    fn cast_explicit_decimal_negative_to_int64_truncate() {
        // Decimal(-12345, 2) = -123.45 → 截断为 -123
        let v = Value::Decimal(-12345, 2).cast_explicit(&ColumnType::Int64);
        assert_eq!(v, Ok(Value::Int64(-123)));
    }

    #[test]
    fn cast_explicit_text_to_decimal() {
        let v = Value::Text("123.45".to_string()).cast_explicit(&ColumnType::Decimal {
            precision: 10,
            scale: 2,
        });
        assert_eq!(v, Ok(Value::Decimal(12345, 2)));
    }

    #[test]
    fn cast_explicit_null_to_any_type() {
        // NULL 显式转换为任何类型仍是 NULL
        assert_eq!(
            Value::Null.cast_explicit(&ColumnType::Int64),
            Ok(Value::Null)
        );
        assert_eq!(
            Value::Null.cast_explicit(&ColumnType::Json),
            Ok(Value::Null)
        );
    }

    #[test]
    fn cast_explicit_disallow_int64_to_array() {
        // Int64 → Array 不是合法转换（即使是显式）
        let v = Value::Int64(42).cast_explicit(&ColumnType::Array(Box::new(ColumnType::Int64)));
        assert!(matches!(v, Err(CastError::ImplicitNotAllowed)));
    }

    // =================================================================
    //  补充测试：覆盖存活的 match arm 删除突变体
    // =================================================================

    #[test]
    fn cast_implicit_tsvector_to_text() {
        // TsVector → Text（PG 文本格式）
        let ts = TsVector::from_lexemes(["hello", "world"]);
        let v = Value::TsVector(ts.clone()).cast_implicit(&ColumnType::Text);
        assert_eq!(v, Ok(Value::Text(ts.to_pg_string())));
    }

    #[test]
    fn cast_implicit_tsquery_to_text() {
        // TsQuery → Text（PG 文本格式）
        let q = TsQuery::lexeme("hello").and(TsQuery::lexeme("world"));
        let v = Value::TsQuery(q.clone()).cast_implicit(&ColumnType::Text);
        assert_eq!(v, Ok(Value::Text(q.to_pg_string())));
    }

    #[test]
    fn cast_explicit_text_to_tsvector() {
        // Text → TsVector（PG 文本格式解析）
        let s = "hello:1 world:2";
        let v = Value::Text(s.to_string()).cast_explicit(&ColumnType::TsVector);
        let expected = TsVector::parse(s).expect("parse should succeed");
        assert_eq!(v, Ok(Value::TsVector(expected)));
    }

    #[test]
    fn cast_explicit_text_to_tsvector_invalid() {
        // 非法 tsvector 文本应返回 Impossible 错误
        // 使用一个会触发 parse 错误的输入
        let v = Value::Text(":123".to_string()).cast_explicit(&ColumnType::TsVector);
        assert!(matches!(v, Err(CastError::Impossible { .. })));
    }

    #[test]
    fn cast_explicit_text_to_tsquery() {
        // Text → TsQuery（PG 文本格式解析）
        let s = "hello & world";
        let v = Value::Text(s.to_string()).cast_explicit(&ColumnType::TsQuery);
        let expected = TsQuery::parse(s).expect("parse should succeed");
        assert_eq!(v, Ok(Value::TsQuery(expected)));
    }

    #[test]
    fn cast_explicit_text_to_tsquery_invalid() {
        // 非法 tsquery 文本应返回 Impossible 错误
        // 使用不匹配括号
        let v = Value::Text("(hello".to_string()).cast_explicit(&ColumnType::TsQuery);
        assert!(matches!(v, Err(CastError::Impossible { .. })));
    }

    #[test]
    fn cast_explicit_float64_to_decimal_exact() {
        // 精确值验证，检测 * → + 或 / 的运算符突变
        let v = Value::Float64(1.25).cast_explicit(&ColumnType::Decimal {
            precision: 10,
            scale: 2,
        });
        assert_eq!(v, Ok(Value::Decimal(125, 2)));

        // 验证更多值以确保乘法被正确执行
        let v2 = Value::Float64(3.5).cast_explicit(&ColumnType::Decimal {
            precision: 10,
            scale: 3,
        });
        assert_eq!(v2, Ok(Value::Decimal(3500, 3)));

        // 负数
        let v3 = Value::Float64(-2.5).cast_explicit(&ColumnType::Decimal {
            precision: 10,
            scale: 1,
        });
        assert_eq!(v3, Ok(Value::Decimal(-25, 1)));
    }

    #[test]
    fn cast_implicit_int64_to_decimal_exact_overflow() {
        // 测试溢出路径：i64::MAX * 10^scale 会溢出 i128（scale 足够大时）
        // i64::MAX ≈ 9.2 × 10^18，i128::MAX ≈ 1.7 × 10^38
        // scale=20 时需要乘 10^20，约 9.2 × 10^38 > i128::MAX
        let v = Value::Int64(i64::MAX).cast_implicit(&ColumnType::Decimal {
            precision: 38,
            scale: 20,
        });
        assert!(matches!(v, Err(CastError::Overflow { .. })));
    }

    #[test]
    fn format_iso_timestamp_negative() {
        // 负时间戳（1970 年之前）的格式化
        // 验证 * 运算符未被替换（rem_euclid 用于计算纳秒）
        let s = format_iso_timestamp(-1_500_000_000);
        // 应该是一个合法的 RFC 3339 字符串
        assert!(s.contains('T'));
        assert!(s.ends_with('Z'));
    }

    #[test]
    fn parse_decimal_boundary_scale_zero() {
        // scale=0 时应直接解析整数
        let v = parse_decimal("42", 0);
        assert_eq!(v, Some(42));
    }

    #[test]
    fn parse_decimal_frac_shorter_than_scale() {
        // 小数位数 < scale 时补零
        let v = parse_decimal("1.2", 4);
        assert_eq!(v, Some(12000));
    }

    #[test]
    fn parse_decimal_frac_longer_than_scale() {
        // 小数位数 >= scale 时截断
        let v = parse_decimal("1.2345", 2);
        assert_eq!(v, Some(123));
    }

    #[test]
    fn parse_decimal_negative() {
        let v = parse_decimal("-123.45", 2);
        assert_eq!(v, Some(-12345));
    }

    #[test]
    fn parse_iso_date_non_epoch() {
        // 非 1970-01-01 的日期，检测返回值是否被替换为 Some(0)
        let v = parse_iso_date("2024-01-01");
        assert_ne!(v, Some(0));
        assert!(v.is_some());
        // 2024-01-01 距 1970-01-01 约 19723 天
        assert_eq!(v, Some(19723));
    }

    #[test]
    fn parse_iso_timestamp_non_zero() {
        // 非零时间戳，检测返回值是否被替换为 Some(0)
        let v = parse_iso_timestamp("2024-01-01T00:00:00Z");
        assert_ne!(v, Some(0));
        assert!(v.is_some());
        // 2024-01-01 ≈ 1704067200 秒 = 1704067200000000 微秒
        assert_eq!(v, Some(1704067200000000));
    }

    // =================================================================
    //  TsVector 全方法覆盖测试（杀死 match arm / 运算符替换变异体）
    // =================================================================

    #[test]
    fn tsvector_from_lexemes_assigns_sequential_positions() {
        // 验证位置从 1 开始递增，且 + 替换为 * 会被检测出
        let ts = TsVector::from_lexemes(["hello", "world", "foo"]);
        assert_eq!(ts.lexemes.len(), 3);
        // 按字典序：foo, hello, world
        assert_eq!(ts.lexemes[0].term, "foo");
        assert_eq!(ts.lexemes[0].positions[0].position, 3);
        assert_eq!(ts.lexemes[1].term, "hello");
        assert_eq!(ts.lexemes[1].positions[0].position, 1);
        assert_eq!(ts.lexemes[2].term, "world");
        assert_eq!(ts.lexemes[2].positions[0].position, 2);
    }

    #[test]
    fn tsvector_from_lexemes_dedup_and_sort() {
        // 重复词素应合并位置列表
        let ts = TsVector::from_lexemes(["a", "b", "a", "c", "b"]);
        assert_eq!(ts.lexemes.len(), 3);
        assert_eq!(ts.lexemes[0].term, "a");
        assert_eq!(ts.lexemes[0].positions.len(), 2);
        assert_eq!(ts.lexemes[0].positions[0].position, 1);
        assert_eq!(ts.lexemes[0].positions[1].position, 3);
    }

    #[test]
    fn tsvector_from_lexemes_empty() {
        let ts = TsVector::from_lexemes::<_, &str>([]);
        assert!(ts.lexemes.is_empty());
    }

    #[test]
    fn tsvector_contains_term_basic() {
        let ts = TsVector::from_lexemes(["hello", "world"]);
        assert!(ts.contains_term("hello"));
        assert!(ts.contains_term("world"));
        assert!(!ts.contains_term("foo"));
        // contains_term 内部 to_lowercase 输入，HELLO 应匹配已小写化的 hello
        assert!(ts.contains_term("HELLO"));
    }

    #[test]
    fn tsvector_contains_term_case_insensitive() {
        let ts = TsVector::from_lexemes(["hello"]);
        // contains_term 内部 to_lowercase 输入，所以 "HELLO" 应匹配
        assert!(ts.contains_term("HELLO"));
        assert!(ts.contains_term("Hello"));
        assert!(ts.contains_term("HeLLo"));
    }

    #[test]
    fn tsvector_terms_returns_all() {
        let ts = TsVector::from_lexemes(["hello", "world", "foo"]);
        let terms = ts.terms();
        assert_eq!(terms.len(), 3);
        // 按字典序
        assert_eq!(terms, vec!["foo", "hello", "world"]);
    }

    #[test]
    fn tsvector_terms_empty() {
        let ts = TsVector::new();
        let terms = ts.terms();
        assert!(terms.is_empty());
    }

    #[test]
    fn tsvector_to_pg_string_basic() {
        let ts = TsVector::from_lexemes(["hello", "world"]);
        let s = ts.to_pg_string();
        // 应包含两个词素及其位置
        assert!(s.contains("hello:1"));
        assert!(s.contains("world:2"));
    }

    #[test]
    fn tsvector_to_pg_string_with_weights() {
        // 构造带权重的 tsvector
        let mut ts = TsVector::from_lexemes(["hello"]);
        ts.lexemes[0].positions[0].weight = TS_WEIGHT_A | TS_WEIGHT_B;
        let s = ts.to_pg_string();
        assert!(s.contains("hello:1AB"), "got: {s}");
    }

    #[test]
    fn tsvector_to_pg_string_all_weights() {
        let mut ts = TsVector::from_lexemes(["hello"]);
        ts.lexemes[0].positions[0].weight = TS_WEIGHT_A | TS_WEIGHT_B | TS_WEIGHT_C | TS_WEIGHT_D;
        let s = ts.to_pg_string();
        assert!(s.contains("hello:1ABCD"), "got: {s}");
    }

    #[test]
    fn tsvector_to_pg_string_no_weight_zero() {
        // weight=0 时不应该输出权重字母
        let ts = TsVector::from_lexemes(["hello"]);
        let s = ts.to_pg_string();
        assert!(s.contains("hello:1"));
        // 不应该有 A/B/C/D 字母
        assert!(!s.contains("A"));
        assert!(!s.contains("B"));
        assert!(!s.contains("C"));
        assert!(!s.contains("D"));
    }

    #[test]
    fn tsvector_parse_basic() {
        let ts = TsVector::parse("hello world").unwrap();
        assert_eq!(ts.lexemes.len(), 2);
        assert!(ts.contains_term("hello"));
        assert!(ts.contains_term("world"));
    }

    #[test]
    fn tsvector_parse_with_positions() {
        let ts = TsVector::parse("hello:1 world:2").unwrap();
        assert_eq!(ts.lexemes.len(), 2);
        assert_eq!(ts.lexemes[0].term, "hello");
        assert_eq!(ts.lexemes[0].positions[0].position, 1);
        assert_eq!(ts.lexemes[1].term, "world");
        assert_eq!(ts.lexemes[1].positions[0].position, 2);
    }

    #[test]
    fn tsvector_parse_with_weights() {
        let ts = TsVector::parse("hello:1A world:2B").unwrap();
        assert_eq!(ts.lexemes[0].positions[0].weight, TS_WEIGHT_A);
        assert_eq!(ts.lexemes[1].positions[0].weight, TS_WEIGHT_B);
    }

    #[test]
    fn tsvector_parse_weight_uppercase_and_lowercase() {
        // 权重字符大小写不敏感
        let ts = TsVector::parse("hello:1a world:2b").unwrap();
        assert_eq!(ts.lexemes[0].positions[0].weight, TS_WEIGHT_A);
        assert_eq!(ts.lexemes[1].positions[0].weight, TS_WEIGHT_B);
    }

    #[test]
    fn tsvector_parse_all_weight_chars() {
        let ts = TsVector::parse("hello:1ABCD").unwrap();
        assert_eq!(
            ts.lexemes[0].positions[0].weight,
            TS_WEIGHT_A | TS_WEIGHT_B | TS_WEIGHT_C | TS_WEIGHT_D
        );
    }

    #[test]
    fn tsvector_parse_multiple_positions_per_lexeme() {
        let ts = TsVector::parse("hello:1,3 world:2").unwrap();
        assert_eq!(ts.lexemes[0].term, "hello");
        assert_eq!(ts.lexemes[0].positions.len(), 2);
        assert_eq!(ts.lexemes[0].positions[0].position, 1);
        assert_eq!(ts.lexemes[0].positions[1].position, 3);
    }

    #[test]
    fn tsvector_parse_invalid_weight_char() {
        let result = TsVector::parse("hello:1X");
        assert!(result.is_err());
    }

    #[test]
    fn tsvector_parse_invalid_position_no_digits() {
        let result = TsVector::parse("hello:A");
        assert!(result.is_err());
    }

    #[test]
    fn tsvector_parse_empty_string() {
        let ts = TsVector::parse("").unwrap();
        assert!(ts.lexemes.is_empty());
    }

    #[test]
    fn tsvector_parse_roundtrip_with_to_pg_string() {
        let original = "hello:1A world:2B";
        let ts = TsVector::parse(original).unwrap();
        let pg_string = ts.to_pg_string();
        // 重新解析应得到相同的 tsvector
        let reparsed = TsVector::parse(&pg_string).unwrap();
        assert_eq!(ts, reparsed);
    }

    // =================================================================
    //  TsQuery::matches 全分支覆盖
    // =================================================================

    #[test]
    fn tsquery_matches_empty_returns_false() {
        let ts = TsVector::from_lexemes(["hello"]);
        assert!(!TsQuery::Empty.matches(&ts));
    }

    #[test]
    fn tsquery_matches_lexeme_present() {
        let ts = TsVector::from_lexemes(["hello", "world"]);
        assert!(TsQuery::lexeme("hello").matches(&ts));
        assert!(TsQuery::lexeme("world").matches(&ts));
    }

    #[test]
    fn tsquery_matches_lexeme_absent() {
        let ts = TsVector::from_lexemes(["hello"]);
        assert!(!TsQuery::lexeme("foo").matches(&ts));
    }

    #[test]
    fn tsquery_matches_lexeme_with_weight_filter_matched() {
        let mut ts = TsVector::from_lexemes(["hello"]);
        ts.lexemes[0].positions[0].weight = TS_WEIGHT_A;
        let q = TsQuery::Lexeme {
            term: "hello".to_string(),
            weights: TS_WEIGHT_A,
        };
        assert!(q.matches(&ts));
    }

    #[test]
    fn tsquery_matches_lexeme_with_weight_filter_not_matched() {
        let mut ts = TsVector::from_lexemes(["hello"]);
        ts.lexemes[0].positions[0].weight = TS_WEIGHT_B;
        let q = TsQuery::Lexeme {
            term: "hello".to_string(),
            weights: TS_WEIGHT_A,
        };
        assert!(!q.matches(&ts));
    }

    #[test]
    fn tsquery_matches_lexeme_weight_zero_matches_any_weight() {
        let mut ts = TsVector::from_lexemes(["hello"]);
        ts.lexemes[0].positions[0].weight = TS_WEIGHT_A;
        let q = TsQuery::Lexeme {
            term: "hello".to_string(),
            weights: 0,
        };
        assert!(q.matches(&ts));
    }

    #[test]
    fn tsquery_matches_and() {
        let ts = TsVector::from_lexemes(["hello", "world"]);
        let q = TsQuery::lexeme("hello").and(TsQuery::lexeme("world"));
        assert!(q.matches(&ts));

        let q = TsQuery::lexeme("hello").and(TsQuery::lexeme("foo"));
        assert!(!q.matches(&ts));
    }

    #[test]
    fn tsquery_matches_or() {
        let ts = TsVector::from_lexemes(["hello"]);
        let q = TsQuery::lexeme("hello").or(TsQuery::lexeme("foo"));
        assert!(q.matches(&ts));

        let q = TsQuery::lexeme("bar").or(TsQuery::lexeme("foo"));
        assert!(!q.matches(&ts));
    }

    #[test]
    fn tsquery_matches_not() {
        let ts = TsVector::from_lexemes(["hello"]);
        // !foo → foo 不匹配 → !foo 匹配
        let q = TsQuery::lexeme("foo").not_query();
        assert!(q.matches(&ts));
        // !hello → hello 匹配 → !hello 不匹配
        let q = TsQuery::lexeme("hello").not_query();
        assert!(!q.matches(&ts));
    }

    #[test]
    fn tsquery_matches_followed_by_distance_1_match() {
        let ts = TsVector::from_lexemes(["hello", "world"]);
        // hello 在位置 1, world 在位置 2, distance=1 应匹配
        let q = TsQuery::FollowedBy {
            distance: 1,
            left: Box::new(TsQuery::lexeme("hello")),
            right: Box::new(TsQuery::lexeme("world")),
        };
        assert!(q.matches(&ts));
    }

    #[test]
    fn tsquery_matches_followed_by_distance_too_far() {
        let ts = TsVector::from_lexemes(["hello", "world"]);
        // hello 在位置 1, world 在位置 2, distance=0 应不匹配（差值=1 > 0）
        let q = TsQuery::FollowedBy {
            distance: 0,
            left: Box::new(TsQuery::lexeme("hello")),
            right: Box::new(TsQuery::lexeme("world")),
        };
        assert!(!q.matches(&ts));
    }

    #[test]
    fn tsquery_matches_followed_by_distance_2_match() {
        let ts = TsVector::from_lexemes(["a", "b", "c"]);
        // a=1, c=3, distance=2 应匹配
        let q = TsQuery::FollowedBy {
            distance: 2,
            left: Box::new(TsQuery::lexeme("a")),
            right: Box::new(TsQuery::lexeme("c")),
        };
        assert!(q.matches(&ts));
    }

    #[test]
    fn tsquery_matches_followed_by_wrong_order() {
        let ts = TsVector::from_lexemes(["hello", "world"]);
        // world 在位置 2, hello 在位置 1，但 left=world, right=hello
        // rp - lp = 1 - 2 = -1，不满足 rp > lp
        let q = TsQuery::FollowedBy {
            distance: 5,
            left: Box::new(TsQuery::lexeme("world")),
            right: Box::new(TsQuery::lexeme("hello")),
        };
        assert!(!q.matches(&ts));
    }

    // =================================================================
    //  TsQuery::to_pg_string 全变体覆盖
    // =================================================================

    #[test]
    fn tsquery_to_pg_string_empty() {
        assert_eq!(TsQuery::Empty.to_pg_string(), "");
    }

    #[test]
    fn tsquery_to_pg_string_lexeme_no_weight() {
        let q = TsQuery::lexeme("hello");
        assert_eq!(q.to_pg_string(), "hello");
    }

    #[test]
    fn tsquery_to_pg_string_lexeme_with_weight_a() {
        let q = TsQuery::Lexeme {
            term: "hello".to_string(),
            weights: TS_WEIGHT_A,
        };
        assert_eq!(q.to_pg_string(), "hello:A");
    }

    #[test]
    fn tsquery_to_pg_string_lexeme_with_weight_b() {
        let q = TsQuery::Lexeme {
            term: "hello".to_string(),
            weights: TS_WEIGHT_B,
        };
        assert_eq!(q.to_pg_string(), "hello:B");
    }

    #[test]
    fn tsquery_to_pg_string_lexeme_with_weight_c() {
        let q = TsQuery::Lexeme {
            term: "hello".to_string(),
            weights: TS_WEIGHT_C,
        };
        assert_eq!(q.to_pg_string(), "hello:C");
    }

    #[test]
    fn tsquery_to_pg_string_lexeme_with_weight_d() {
        let q = TsQuery::Lexeme {
            term: "hello".to_string(),
            weights: TS_WEIGHT_D,
        };
        assert_eq!(q.to_pg_string(), "hello:D");
    }

    #[test]
    fn tsquery_to_pg_string_lexeme_with_all_weights() {
        let q = TsQuery::Lexeme {
            term: "hello".to_string(),
            weights: TS_WEIGHT_A | TS_WEIGHT_B | TS_WEIGHT_C | TS_WEIGHT_D,
        };
        assert_eq!(q.to_pg_string(), "hello:ABCD");
    }

    #[test]
    fn tsquery_to_pg_string_and() {
        let q = TsQuery::lexeme("a").and(TsQuery::lexeme("b"));
        assert_eq!(q.to_pg_string(), "(a & b)");
    }

    #[test]
    fn tsquery_to_pg_string_or() {
        let q = TsQuery::lexeme("a").or(TsQuery::lexeme("b"));
        assert_eq!(q.to_pg_string(), "(a | b)");
    }

    #[test]
    fn tsquery_to_pg_string_not() {
        let q = TsQuery::lexeme("a").not_query();
        assert_eq!(q.to_pg_string(), "!a");
    }

    #[test]
    fn tsquery_to_pg_string_followed_by_distance_1() {
        let q = TsQuery::FollowedBy {
            distance: 1,
            left: Box::new(TsQuery::lexeme("a")),
            right: Box::new(TsQuery::lexeme("b")),
        };
        assert_eq!(q.to_pg_string(), "(a <-> b)");
    }

    #[test]
    fn tsquery_to_pg_string_followed_by_distance_n() {
        let q = TsQuery::FollowedBy {
            distance: 3,
            left: Box::new(TsQuery::lexeme("a")),
            right: Box::new(TsQuery::lexeme("b")),
        };
        assert_eq!(q.to_pg_string(), "(a <3> b)");
    }

    // =================================================================
    //  TsQuery::parse / tokenize 全操作符覆盖
    // =================================================================

    #[test]
    fn tsquery_parse_empty() {
        let q = TsQuery::parse("").unwrap();
        assert_eq!(q, TsQuery::Empty);
    }

    #[test]
    fn tsquery_parse_single_lexeme() {
        let q = TsQuery::parse("hello").unwrap();
        assert_eq!(q, TsQuery::lexeme("hello"));
    }

    #[test]
    fn tsquery_parse_and() {
        let q = TsQuery::parse("hello & world").unwrap();
        assert_eq!(q, TsQuery::lexeme("hello").and(TsQuery::lexeme("world")));
    }

    #[test]
    fn tsquery_parse_or() {
        let q = TsQuery::parse("hello | world").unwrap();
        assert_eq!(q, TsQuery::lexeme("hello").or(TsQuery::lexeme("world")));
    }

    #[test]
    fn tsquery_parse_not() {
        let q = TsQuery::parse("!hello").unwrap();
        assert_eq!(q, TsQuery::lexeme("hello").not_query());
    }

    #[test]
    fn tsquery_parse_parentheses() {
        let q = TsQuery::parse("(hello | world) & foo").unwrap();
        let expected = TsQuery::Or(
            Box::new(TsQuery::lexeme("hello")),
            Box::new(TsQuery::lexeme("world")),
        )
        .and(TsQuery::lexeme("foo"));
        assert_eq!(q, expected);
    }

    #[test]
    fn tsquery_parse_followed_by_distance_1() {
        let q = TsQuery::parse("hello <-> world").unwrap();
        assert_eq!(
            q,
            TsQuery::FollowedBy {
                distance: 1,
                left: Box::new(TsQuery::lexeme("hello")),
                right: Box::new(TsQuery::lexeme("world")),
            }
        );
    }

    #[test]
    fn tsquery_parse_followed_by_distance_n() {
        let q = TsQuery::parse("hello <3> world").unwrap();
        assert_eq!(
            q,
            TsQuery::FollowedBy {
                distance: 3,
                left: Box::new(TsQuery::lexeme("hello")),
                right: Box::new(TsQuery::lexeme("world")),
            }
        );
    }

    #[test]
    fn tsquery_parse_lexeme_with_weight() {
        let q = TsQuery::parse("hello:A").unwrap();
        assert_eq!(
            q,
            TsQuery::Lexeme {
                term: "hello".to_string(),
                weights: TS_WEIGHT_A,
            }
        );
    }

    #[test]
    fn tsquery_parse_lexeme_with_all_weights() {
        let q = TsQuery::parse("hello:ABCD").unwrap();
        assert_eq!(
            q,
            TsQuery::Lexeme {
                term: "hello".to_string(),
                weights: TS_WEIGHT_A | TS_WEIGHT_B | TS_WEIGHT_C | TS_WEIGHT_D,
            }
        );
    }

    #[test]
    fn tsquery_parse_lowercase_weight_chars() {
        let q = TsQuery::parse("hello:abcd").unwrap();
        assert_eq!(
            q,
            TsQuery::Lexeme {
                term: "hello".to_string(),
                weights: TS_WEIGHT_A | TS_WEIGHT_B | TS_WEIGHT_C | TS_WEIGHT_D,
            }
        );
    }

    #[test]
    fn tsquery_parse_invalid_weight_char() {
        let result = TsQuery::parse("hello:X");
        assert!(result.is_err());
    }

    #[test]
    fn tsquery_parse_invalid_distance_non_digit() {
        let result = TsQuery::parse("hello <abc> world");
        assert!(result.is_err());
    }

    #[test]
    fn tsquery_parse_unexpected_lt() {
        let result = TsQuery::parse("hello < world");
        assert!(result.is_err());
    }

    #[test]
    fn tsquery_parse_roundtrip() {
        let cases = [
            "hello",
            "hello & world",
            "hello | world",
            "!hello",
            "(a | b) & c",
            "a <-> b",
            "a <3> b",
            "hello:A",
            "hello:ABCD",
        ];
        for case in cases {
            let q = TsQuery::parse(case).unwrap();
            let s = q.to_pg_string();
            // 重新解析应得到相同结果
            let q2 = TsQuery::parse(&s).unwrap();
            assert_eq!(q, q2, "roundtrip failed for: {case} -> {s}");
        }
    }

    // =================================================================
    //  format_decimal 边界测试（杀死 < 替换为 <= 的变异）
    // =================================================================

    #[test]
    fn format_decimal_zero_with_scale() {
        // 0 不应该有负号
        let s = format_decimal(0, 3);
        assert_eq!(s, "0.000");
    }

    #[test]
    fn format_decimal_positive_with_zero_int_part() {
        // Decimal(5, 3) = 0.005，正数不应有负号
        let s = format_decimal(5, 3);
        assert_eq!(s, "0.005");
    }

    #[test]
    fn format_decimal_negative_with_zero_int_part() {
        // Decimal(-5, 3) = -0.005，负数应有负号
        let s = format_decimal(-5, 3);
        assert_eq!(s, "-0.005");
    }

    #[test]
    fn format_decimal_negative_with_nonzero_int_part() {
        // Decimal(-12345, 2) = -123.45
        let s = format_decimal(-12345, 2);
        assert_eq!(s, "-123.45");
    }

    #[test]
    fn format_decimal_scale_zero() {
        assert_eq!(format_decimal(0, 0), "0");
        assert_eq!(format_decimal(42, 0), "42");
        assert_eq!(format_decimal(-42, 0), "-42");
    }

    #[test]
    fn format_decimal_large_scale() {
        // 测试大 scale 下的格式化
        let s = format_decimal(1, 10);
        assert_eq!(s, "0.0000000001");
    }

    // =================================================================
    //  format_iso_timestamp 测试（覆盖整秒/微秒/极端值）
    // =================================================================
    //
    // 注意：当前格式字符串 "%Y-%m-%dT%H:%M:%SZ" 不输出纳秒部分，
    // 所以微秒精度在文本输出中不可见。但 DateTime 内部的 nanos
    // 计算必须正确，否则在极端值下 from_timestamp 可能返回 None。
    // 这里通过测试 sec 边界（rem_euclid 接近 0/999999）来覆盖。

    #[test]
    fn format_iso_timestamp_microseconds_within_second() {
        // 1 微秒：仍在第 0 秒内，输出应为 "1970-01-01T00:00:00Z"
        let s = format_iso_timestamp(1);
        assert_eq!(s, "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_iso_timestamp_half_second() {
        // 500000 微秒：仍在第 0 秒内
        let s = format_iso_timestamp(500_000);
        assert_eq!(s, "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_iso_timestamp_full_second() {
        // 整秒：1_000_000 微秒 = 1 秒
        let s = format_iso_timestamp(1_000_000);
        assert_eq!(s, "1970-01-01T00:00:01Z");
    }

    #[test]
    fn format_iso_timestamp_max_microseconds() {
        // 999999 微秒：仍在第 0 秒内
        let s = format_iso_timestamp(999_999);
        assert_eq!(s, "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_iso_timestamp_second_boundary() {
        // 测试秒边界：sec=1, nanos=0
        let s = format_iso_timestamp(1_000_000);
        assert_eq!(s, "1970-01-01T00:00:01Z");
        // sec=2
        let s = format_iso_timestamp(2_000_000);
        assert_eq!(s, "1970-01-01T00:00:02Z");
    }

    #[test]
    fn format_iso_timestamp_negative_rem() {
        // 负微秒：div_euclid/rem_euclid 应正确处理
        // -1 微秒 → secs = -1, rem = 999999 → nanos = 999999000
        // 即 -1 秒 + 999999 微秒 = -1 微秒
        let s = format_iso_timestamp(-1);
        // chrono from_timestamp(-1, 999999000) = 1969-12-31T23:59:59.999999Z
        // 但格式不输出纳秒，所以是 "1969-12-31T23:59:59Z"
        assert_eq!(s, "1969-12-31T23:59:59Z");
    }

    // =================================================================
    //  cast_explicit Float64 → Decimal 详细测试
    // =================================================================

    #[test]
    fn cast_explicit_float64_to_decimal_rounding() {
        // 1.2345 with scale=2 应四舍五入到 123（即 1.23）
        let v = Value::Float64(1.2345);
        let result = v
            .cast_explicit(&ColumnType::Decimal {
                precision: 38,
                scale: 2,
            })
            .unwrap();
        // 1.2345 * 100 = 123.45, round = 123
        assert_eq!(result, Value::Decimal(123, 2));
    }

    #[test]
    fn cast_explicit_float64_to_decimal_negative() {
        let v = Value::Float64(-1.2345);
        let result = v
            .cast_explicit(&ColumnType::Decimal {
                precision: 38,
                scale: 2,
            })
            .unwrap();
        // -1.2345 * 100 = -123.45, round = -123
        assert_eq!(result, Value::Decimal(-123, 2));
    }

    #[test]
    fn cast_explicit_float64_to_decimal_zero() {
        let v = Value::Float64(0.0);
        let result = v
            .cast_explicit(&ColumnType::Decimal {
                precision: 38,
                scale: 3,
            })
            .unwrap();
        assert_eq!(result, Value::Decimal(0, 3));
    }

    #[test]
    fn cast_explicit_float64_to_decimal_scale_zero() {
        let v = Value::Float64(42.7);
        let result = v
            .cast_explicit(&ColumnType::Decimal {
                precision: 38,
                scale: 0,
            })
            .unwrap();
        // scale=0: 10^0 = 1, 42.7 * 1 = 42.7, round = 43
        assert_eq!(result, Value::Decimal(43, 0));
    }

    #[test]
    fn cast_explicit_float64_to_decimal_half_up() {
        // 0.5 应四舍五入到 1（scale=0）
        let v = Value::Float64(0.5);
        let result = v
            .cast_explicit(&ColumnType::Decimal {
                precision: 38,
                scale: 0,
            })
            .unwrap();
        assert_eq!(result, Value::Decimal(1, 0));
    }

    // =================================================================
    //  collect_match_positions 间接测试（通过 FollowedBy 嵌套）
    // =================================================================

    #[test]
    fn collect_match_positions_and_combination() {
        // 通过 FollowedBy 嵌套 And 来测试 collect_match_positions 的 And 分支
        let ts = TsVector::from_lexemes(["a", "b", "c"]);
        // (a & b) <-> c
        let q = TsQuery::FollowedBy {
            distance: 5,
            left: Box::new(TsQuery::lexeme("a").and(TsQuery::lexeme("b"))),
            right: Box::new(TsQuery::lexeme("c")),
        };
        // a=1, b=2, c=3, (a&b) 位置为 [1, 2], c 位置为 [3]
        // rp - lp = 3 - 2 = 1 <= 5，匹配
        assert!(q.matches(&ts));
    }

    #[test]
    fn collect_match_positions_or_combination() {
        let ts = TsVector::from_lexemes(["a", "b"]);
        // (a | x) <-> b
        let q = TsQuery::FollowedBy {
            distance: 5,
            left: Box::new(TsQuery::lexeme("a").or(TsQuery::lexeme("x"))),
            right: Box::new(TsQuery::lexeme("b")),
        };
        // a=1, b=2, x 不匹配，(a|x) 位置为 [1], b 位置为 [2]
        assert!(q.matches(&ts));
    }

    #[test]
    fn collect_match_positions_not_returns_empty() {
        let ts = TsVector::from_lexemes(["a", "b"]);
        // !a <-> b —— !a 位置为空，FollowedBy 不匹配
        let q = TsQuery::FollowedBy {
            distance: 100,
            left: Box::new(TsQuery::lexeme("a").not_query()),
            right: Box::new(TsQuery::lexeme("b")),
        };
        assert!(!q.matches(&ts));
    }

    #[test]
    fn collect_match_positions_with_weight_filter() {
        let mut ts = TsVector::from_lexemes(["a", "b"]);
        // a 位置 1 加权重 A，b 位置 2 不加权
        ts.lexemes[0].positions[0].weight = TS_WEIGHT_A;
        // a:A <-> b 应匹配
        let q = TsQuery::FollowedBy {
            distance: 5,
            left: Box::new(TsQuery::Lexeme {
                term: "a".to_string(),
                weights: TS_WEIGHT_A,
            }),
            right: Box::new(TsQuery::lexeme("b")),
        };
        assert!(q.matches(&ts));

        // a:B <-> b 应不匹配（a 没有权重 B）
        let q2 = TsQuery::FollowedBy {
            distance: 5,
            left: Box::new(TsQuery::Lexeme {
                term: "a".to_string(),
                weights: TS_WEIGHT_B,
            }),
            right: Box::new(TsQuery::lexeme("b")),
        };
        assert!(!q2.matches(&ts));
    }

    // =================================================================
    //  parse_decimal / parse_iso_date 边界覆盖
    // =================================================================

    #[test]
    fn parse_decimal_zero() {
        assert_eq!(parse_decimal("0", 2), Some(0));
        assert_eq!(parse_decimal("0.00", 2), Some(0));
        assert_eq!(parse_decimal("-0", 2), Some(0));
        assert_eq!(parse_decimal("+0", 2), Some(0));
    }

    #[test]
    fn parse_decimal_large_scale() {
        assert_eq!(parse_decimal("0.0000000001", 10), Some(1));
        assert_eq!(parse_decimal("1.0000000001", 10), Some(10_000_000_001));
    }

    #[test]
    fn parse_decimal_invalid() {
        // "abc" 不是有效数字
        assert_eq!(parse_decimal("abc", 2), None);
        // "1.2.3" 小数部分 "2.3" 截断到 "2." 后无法解析
        assert_eq!(parse_decimal("1.2.3", 2), None);
    }

    #[test]
    fn parse_decimal_empty_string_returns_zero() {
        // 空字符串视为 0（int_str 和 frac_str 都为空，结果为 0）
        assert_eq!(parse_decimal("", 2), Some(0));
        assert_eq!(parse_decimal("   ", 2), Some(0));
    }

    #[test]
    fn parse_iso_date_epoch() {
        assert_eq!(parse_iso_date("1970-01-01"), Some(0));
    }

    #[test]
    fn parse_iso_date_invalid_format() {
        assert_eq!(parse_iso_date("not-a-date"), None);
        assert_eq!(parse_iso_date("2024/01/01"), None);
        assert_eq!(parse_iso_date(""), None);
    }

    #[test]
    fn format_iso_date_epoch() {
        assert_eq!(format_iso_date(0), "1970-01-01");
    }

    #[test]
    fn format_iso_date_negative() {
        // -1 = 1969-12-31
        assert_eq!(format_iso_date(-1), "1969-12-31");
    }

    #[test]
    fn format_iso_date_extreme_value() {
        // 极端值不应 panic，返回占位字符串
        let s = format_iso_date(i32::MAX);
        assert!(!s.is_empty());
        let s = format_iso_date(i32::MIN);
        assert!(!s.is_empty());
    }

    #[test]
    fn format_iso_date_roundtrip() {
        for days in [0i32, 1, -1, 365, -365, 19723, -19723] {
            let s = format_iso_date(days);
            let parsed = parse_iso_date(&s);
            if let Some(parsed_days) = parsed {
                assert_eq!(parsed_days, days, "roundtrip failed for {days}: {s}");
            }
            // 极端值可能解析失败，但不应该 panic
        }
    }

    #[test]
    fn parse_iso_timestamp_rfc3339_with_offset() {
        // 带时区偏移
        let v = parse_iso_timestamp("2024-01-01T00:00:00+00:00");
        assert_eq!(v, Some(1704067200000000));
    }

    #[test]
    fn parse_iso_timestamp_no_timezone() {
        let v = parse_iso_timestamp("2024-01-01T00:00:00");
        assert_eq!(v, Some(1704067200000000));
    }

    #[test]
    fn parse_iso_timestamp_space_separator() {
        // MySQL/PostgreSQL 常见格式：空格分隔符
        let v = parse_iso_timestamp("2024-01-01 00:00:00");
        assert_eq!(v, Some(1704067200000000));
    }

    #[test]
    fn parse_iso_timestamp_with_fractional_seconds() {
        // 带小数秒的格式：0.500000 秒 = 500000 微秒
        let v = parse_iso_timestamp("2024-01-01T00:00:00.500000");
        assert_eq!(v, Some(1704067200500000));
    }

    #[test]
    fn parse_iso_timestamp_space_separator_with_fractional() {
        // 空格分隔符 + 小数秒
        let v = parse_iso_timestamp("2024-01-01 00:00:00.500000");
        assert_eq!(v, Some(1704067200500000));
    }

    #[test]
    fn parse_iso_timestamp_invalid() {
        assert_eq!(parse_iso_timestamp("not-a-timestamp"), None);
        assert_eq!(parse_iso_timestamp(""), None);
    }

    #[test]
    fn format_iso_timestamp_zero() {
        assert_eq!(format_iso_timestamp(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_iso_timestamp_negative_microsecond() {
        // -1 微秒应正常格式化（不 panic）
        let s = format_iso_timestamp(-1);
        assert!(!s.is_empty());
    }

    // =================================================================
    //  针对 cargo-mutants 存活变异体的补充测试
    // =================================================================
    //
    // 以下测试针对 `mutants.out/missed.txt` 中列出的存活变异体，
    // 通过边界输入让变异体产生与原始代码不同的可观察行为（panic
    // 或返回值差异），从而被变异测试工具"杀死"。

    // ---- value.rs:276  replace + with * in TsVector::parse ----
    //
    // 原始: `let next_pos = u32::try_from(map.len() + 1)`
    // 变异: `let next_pos = u32::try_from(map.len() * 1)`
    //
    // 当 map 为空时，原始 next_pos = 1，变异 next_pos = 0。
    // 通过 to_pg_string() 可观察位置差异。

    #[test]
    fn tsvector_parse_auto_position_is_one_based() {
        // 单词无显式位置时应自动分配位置 1（1-based）
        let ts = TsVector::parse("hello").unwrap();
        assert_eq!(ts.lexemes.len(), 1);
        assert_eq!(ts.lexemes[0].term, "hello");
        assert_eq!(ts.lexemes[0].positions.len(), 1);
        // 关键断言：自动分配的位置必须是 1，而非 0
        assert_eq!(ts.lexemes[0].positions[0].position, 1);
    }

    #[test]
    fn tsvector_parse_auto_position_increments_per_distinct_term() {
        // 多个不同单词时，自动分配的位置应递增：1, 2, 3...
        let ts = TsVector::parse("hello world foo").unwrap();
        assert_eq!(ts.lexemes.len(), 3);
        // lexemes 按字典序排序：foo, hello, world
        assert_eq!(ts.lexemes[0].term, "foo");
        assert_eq!(ts.lexemes[0].positions[0].position, 3);
        assert_eq!(ts.lexemes[1].term, "hello");
        assert_eq!(ts.lexemes[1].positions[0].position, 1);
        assert_eq!(ts.lexemes[2].term, "world");
        assert_eq!(ts.lexemes[2].positions[0].position, 2);
    }

    #[test]
    fn tsvector_parse_auto_position_to_pg_string() {
        // 通过 to_pg_string 验证位置输出
        let ts = TsVector::parse("hello world").unwrap();
        let s = ts.to_pg_string();
        // 原始: "hello:1 world:2"
        // 变异(*): "hello:0 world:1"
        assert_eq!(s, "hello:1 world:2");
    }

    // ---- value.rs:402  replace > with >= in TsQuery::matches (FollowedBy) ----
    //
    // 原始: `if rp > lp && rp - lp <= *distance`
    // 变异: `if rp >= lp && rp - lp <= *distance`
    //
    // 当 rp == lp（左右词素在同一位置）时，原始返回 false，变异返回 true。

    #[test]
    fn tsquery_followed_by_same_position_does_not_match() {
        // 构造 tsvector，hello 和 world 在同一位置 1
        // (PG 中 tsvector 不允许同位置不同词素，但这里通过显式 parse 构造)
        let ts = TsVector::parse("hello:1 world:1").unwrap();
        // FollowedBy(hello, world, distance=1)
        let q = TsQuery::FollowedBy {
            distance: 1,
            left: Box::new(TsQuery::lexeme("hello")),
            right: Box::new(TsQuery::lexeme("world")),
        };
        // 原始: rp=1, lp=1, 1 > 1 = false → 不匹配
        // 变异(>=): 1 >= 1 = true, 1-1=0 <= 1 = true → 匹配（错误）
        assert!(
            !q.matches(&ts),
            "FollowedBy with same position should NOT match"
        );
    }

    // ---- value.rs:518  TsQuery::tokenize 解析 `<->` 时的边界检查 ----
    //
    // 原始: `if i + 2 < bytes.len() && bytes[i + 1] == b'-' && bytes[i + 2] == b'>'`
    // 变异1: `i - 2` (replace + with -) → i<2 时下溢 panic
    // 变异2: `i + 2 <= bytes.len()` (replace < with <=) → 长度刚好时越界 panic
    // 变异3: `i * 2` (replace + with *) → 边界检查错误

    #[test]
    fn tsquery_tokenize_incomplete_arrow_panics_or_errors() {
        // 输入 `<-` (长度 2):
        //   原始: `0 + 2 < 2` = false → 走 else if → "unexpected '<'"
        //   变异1(-): `0 - 2` 下溢 panic
        //   变异2(<=): `0 + 2 <= 2` = true → bytes[2] 越界 panic
        //   变异3(*): `0 * 2 < 2` = true → bytes[2] 越界 panic
        let result = TsQuery::parse("<-");
        assert!(result.is_err(), "parsing '<-' must return Err");
    }

    #[test]
    fn tsquery_tokenize_lt_at_end_of_string_errors() {
        // 输入 `abc<` (长度 4), i=3 时遇到 '<':
        //   原始: else if `3 + 1 < 4` = false → "unexpected '<'"
        //   变异1(i-1): `3 - 1 = 2 < 4` = true → bytes[4] 越界 panic
        //   变异2(i*1): `3 * 1 = 3 < 4` = true → bytes[4] 越界 panic
        //   变异3(<=): `3 + 1 <= 4` = true → bytes[4] 越界 panic
        //   变异4(||): `3+1<4 || bytes[4]...` → bytes[4] 越界 panic
        let result = TsQuery::parse("abc<");
        assert!(result.is_err(), "parsing 'abc<' must return Err");
    }

    #[test]
    fn tsquery_tokenize_incomplete_distance_errors() {
        // 输入 `<5` (长度 2), i=0:
        //   原始: 进入 <N> 解析分支，j 扫描数字到 j=2，
        //         循环 `2 < 2` = false 退出，
        //         `2 < 2 && ...` = false → "expected '>' after distance 5"
        //   变异(524<=): `2 <= 2` = true → bytes[2] 越界 panic
        //   变异(533<=): `2 <= 2 && bytes[2]...` → 越界 panic
        //   变异(533||): `2 < 2 || bytes[2]...` → 越界 panic
        let result = TsQuery::parse("<5");
        assert!(result.is_err(), "parsing '<5' must return Err");
    }

    #[test]
    fn tsquery_tokenize_digit_after_lt_no_panic() {
        // 输入 `<5>` (长度 3): tokenize 成功产生 FollowedBy(5)，
        // 但 parser 期望 lexeme 在 FollowedBy 之前，故返回 Err。
        //   原始: tokenize → [FollowedBy(5)], parse → Err("expected lexeme")
        //   变异(i-1 at 521): `0-1` 下溢 panic → 测试失败（kill）
        //   变异(i*1 at 521): `0*1=0 < 3` = true, bytes[1]='5' digit → 同原始路径
        //                     （此变异由 abc< 测试杀死）
        let result = TsQuery::parse("<5>");
        // 原始返回 Err（parser 拒绝），变异可能 panic（也导致测试失败）
        assert!(result.is_err(), "parsing '<5>' must return Err");
    }

    // ---- value.rs:863  replace == with != in Value::cast_implicit ----
    //
    // 原始: `if scale == target_scale { return Ok(self); }`
    // 变异: `if scale != target_scale { return Ok(self); }`
    //
    // 当 scale 不同时，原始继续走 match 分支（无 Decimal→Decimal arm → Err），
    // 变异提前返回 Ok(self)（错误：scale 未转换）。
    // 当 scale 相同时，原始提前返回 Ok(self)，变异继续走 match → Err。

    #[test]
    fn cast_implicit_decimal_same_scale_returns_self() {
        // scale 相同时应直接返回原值
        let v = Value::Decimal(12345, 2);
        let result = v.clone().cast_implicit(&ColumnType::Decimal {
            precision: 10,
            scale: 2,
        });
        // 原始: scale 相等 → Ok(self)
        // 变异(!=): scale 相等 → 条件 false → 走 match → Err(ImplicitNotAllowed)
        assert_eq!(result, Ok(v));
    }

    #[test]
    fn cast_implicit_decimal_different_scale_not_allowed() {
        // scale 不同时，隐式转换不允许（无 Decimal→Decimal 重缩放 arm）
        let v = Value::Decimal(12345, 2);
        let result = v.cast_implicit(&ColumnType::Decimal {
            precision: 10,
            scale: 3,
        });
        // 原始: scale 不同 → 条件 false → 走 match → _ → Err(ImplicitNotAllowed)
        // 变异(!=): scale 不同 → 条件 true → Ok(self)（错误：scale 未变）
        assert!(
            result.is_err(),
            "implicit Decimal→Decimal with different scale must error"
        );
    }

    // ---- value.rs:1000-1003  cast_explicit 中 Float64→Decimal arm ----
    //
    // 该 arm 与 cast_implicit 中的 Float64→Decimal arm 完全相同，
    // 由于 cast_explicit 先调用 cast_implicit 并在成功时提前返回，
    // 此 arm 永远不会被执行（死代码）。补充测试以验证行为一致性，
    // 同时通过验证 cast_implicit 路径来间接保护此 arm。
    //
    // 注：mutant "delete arm" 无法被任何测试杀死（删除死代码无效果），
    // 这是变异测试工具的已知局限。我们通过验证等价行为来记录意图。

    #[test]
    fn cast_explicit_float64_to_decimal_uses_implicit_path() {
        // 验证 cast_explicit 与 cast_implicit 对 Float64→Decimal 行为一致
        let v = Value::Float64(1.2345);
        let explicit = v.clone().cast_explicit(&ColumnType::Decimal {
            precision: 38,
            scale: 2,
        });
        let implicit = v.cast_implicit(&ColumnType::Decimal {
            precision: 38,
            scale: 2,
        });
        assert_eq!(explicit, implicit);
        assert_eq!(explicit, Ok(Value::Decimal(123, 2)));
    }

    #[test]
    fn cast_explicit_float64_to_decimal_large_value() {
        // 大数 + scale=0 时的行为
        let v = Value::Float64(1e15);
        let result = v.cast_explicit(&ColumnType::Decimal {
            precision: 38,
            scale: 0,
        });
        assert!(result.is_ok());
        if let Ok(Value::Decimal(val, scale)) = result {
            assert_eq!(scale, 0);
            // 1e15 * 10^0 = 1e15, round = 1e15
            assert_eq!(val, 1_000_000_000_000_000_i128);
        } else {
            panic!("expected Decimal");
        }
    }

    // ---- value.rs:1200  format_iso_timestamp 中 nanos 计算 ----
    //
    // 原始: `let nanos = us.rem_euclid(1_000_000) as u32 * 1_000;`
    // 变异: `let nanos = us.rem_euclid(1_000_000) as u32 / 1_000;`
    //
    // 当前格式字符串 "%Y-%m-%dT%H:%M:%SZ" 不输出纳秒，
    // 所以 nanos 值不影响输出 → 变异等价（无法杀死）。
    //
    // 但 from_timestamp(secs, nanos) 要求 nanos < 1_000_000_000，
    // 原始 nanos ∈ [0, 999_999_000] 合法，变异 nanos ∈ [0, 999] 也合法。
    // 两者都不会触发 None 返回，因此输出始终相同。
    //
    // 补充测试验证 format_iso_timestamp 在各种 us 输入下不返回 None
    // 占位串，间接验证 from_timestamp 接受计算后的 nanos。

    #[test]
    fn format_iso_timestamp_various_microsecond_inputs() {
        // 遍历微秒边界值，确保不返回 "<invalid timestamp: ...>" 占位串
        let test_cases: &[i64] = &[
            0,
            1,
            999,
            1_000,
            999_999,
            1_000_000,
            1_500_000,
            59_999_999,
            3_600_000_000,
            -1,
            -1_500_000,
            -1_000_000,
            i64::MIN / 1_000_000, // 极端负值，但不溢出 div_euclid
            i64::MAX / 1_000_000, // 极端正值
        ];
        for &us in test_cases {
            let s = format_iso_timestamp(us);
            assert!(
                !s.starts_with("<invalid timestamp"),
                "format_iso_timestamp({us}) returned invalid placeholder: {s}"
            );
        }
    }
}
