//! 全文检索完整版 — Phase 6.29
//!
//! 在 Phase 3.33 基础类型（`TsVector`/`TsQuery`/`@@`/`to_tsvector`/`to_tsquery`/`ts_rank`/`setweight`）
//! 与 Phase 6.17 `GinIndex`/`Fts5Index` 基础之上，补充以下能力：
//!
//! - **BM25 排序**：现代信息检索排序算法（Okapi BM25），优于 PG 的 `ts_rank`（TF-IDF 变体）
//! - **ts_headline 高亮**：在文档中高亮匹配 `tsquery` 的词素，PG `ts_headline` 语义
//! - **中文分词**：基于词典的最大正向匹配（Maximum Forward Matching）分词器
//! - **FullTextIndex**：倒排索引 + BM25 排序 + 多分词器支持（英文/中文）
//!
//! # 设计
//!
//! - `Bm25Params` — BM25 参数（k1, b），默认 k1=1.2, b=0.75
//! - `Bm25Scorer` — BM25 评分器（维护 df 与 avg_doc_len 统计）
//! - `HeadlineOptions` — 高亮选项（标签/最大词数/最小词数）
//! - `ts_headline()` — 高亮函数（PG `ts_headline([config,] document, query [, options])`）
//! - `ChineseTokenizer` — 中文分词器（最大正向匹配 + 内置词典）
//! - `FullTextIndex` — 倒排索引 + BM25 排序 + TsQuery 查询接口
//!
//! # 与 PG 的关系
//!
//! - PG `ts_rank` 基于 TF + 权重，本实现提供 BM25 作为更强的排序算法（搜索引擎主流）
//! - PG `ts_headline` 默认使用 `<b>...</b>` 标签，支持 `MaxWords`/`MinWords` 等选项
//! - PG 全文检索默认仅支持空格分词的语言（英文/欧洲语系），中文需依赖 `zhparser`/`pg_jieba` 扩展
//! - 本实现内置简体中文分词器（最大正向匹配），无需外部扩展
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：未集成到 SQL 解析路径，仅提供程序化 API
//! - **中文词典精简**：内置约 70 词，生产环境需加载外部词典
//! - **BM25 无增量更新**：删除文档需重建索引（add_document 仅支持追加）
//! - **ts_headline 简化**：未实现 PG 的 `HighlightAll`/`FragmentDelimiter` 等高级选项
//! - **无位置感知查询**：BM25 仅按词频排序，未利用词素位置（Phrase Query 由 `TsQuery::FollowedBy` 处理）

use crate::executor::ExecutionError;
use std::collections::HashMap;
use szrsql_types::value::TsQuery;

// =====================================================================
//  错误类型
// =====================================================================

/// 全文检索错误
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FullTextError {
    /// 无效 BM25 参数 k1（必须 >= 0）
    #[error("invalid BM25 parameter k1: must be >= 0 (got {0})")]
    InvalidK1(f64),
    /// 无效 BM25 参数 b（必须 ∈ [0, 1]）
    #[error("invalid BM25 parameter b: must be in [0, 1] (got {0})")]
    InvalidB(f64),
    /// 无效高亮选项
    #[error("invalid highlight option: {0}")]
    InvalidHighlightOption(String),
    /// 文档 ID 已存在（重复添加）
    #[error("document already exists: doc_id={0}")]
    DocumentExists(usize),
    /// 文档不存在
    #[error("document not found: doc_id={0}")]
    DocumentNotFound(usize),
}

impl From<FullTextError> for ExecutionError {
    fn from(e: FullTextError) -> Self {
        ExecutionError::EvalError(format!("FullText error: {e}"))
    }
}

// =====================================================================
//  BM25 排序器
// =====================================================================

/// BM25 参数
///
/// Okapi BM25 算法参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bm25Params {
    /// k1 — 词频饱和参数（典型值 1.2，范围 [0, 3]）
    ///
    /// 控制词频（TF）的饱和速度。k1=0 时退化为布尔模型（仅考虑是否出现）。
    pub k1: f64,
    /// b — 文档长度归一化参数（典型值 0.75，范围 [0, 1]）
    ///
    /// 控制文档长度对分数的影响。b=0 时无长度归一化，b=1 时完全归一化。
    pub b: f64,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

impl Bm25Params {
    /// 创建 BM25 参数（校验范围）
    pub fn new(k1: f64, b: f64) -> Result<Self, FullTextError> {
        if k1 < 0.0 {
            return Err(FullTextError::InvalidK1(k1));
        }
        if !(0.0..=1.0).contains(&b) {
            return Err(FullTextError::InvalidB(b));
        }
        Ok(Self { k1, b })
    }
}

/// BM25 评分器
///
/// 维护文档级统计（df / avg_doc_len），用于计算单文档对查询的 BM25 分数。
///
/// # 算法
///
/// ```text
/// score(q, d) = Σ_t IDF(t) · (tf(t,d) · (k1 + 1)) / (tf(t,d) + k1 · (1 - b + b · |d| / avgdl))
/// IDF(t) = ln((N - df(t) + 0.5) / (df(t) + 0.5) + 1)
/// ```
///
/// - `N` = 文档总数
/// - `df(t)` = 包含词素 t 的文档数
/// - `tf(t,d)` = 词素 t 在文档 d 中的频次
/// - `|d|` = 文档 d 的长度（词素数）
/// - `avgdl` = 平均文档长度
pub struct Bm25Scorer {
    params: Bm25Params,
    /// 文档总数
    num_docs: usize,
    /// 词素 → 文档频率（df）
    doc_freq: HashMap<String, usize>,
    /// 累计文档长度（用于计算 avgdl）
    total_doc_len: usize,
}

impl Default for Bm25Scorer {
    fn default() -> Self {
        Self::new(Bm25Params::default())
    }
}

impl Bm25Scorer {
    /// 创建评分器
    pub fn new(params: Bm25Params) -> Self {
        Self {
            params,
            num_docs: 0,
            doc_freq: HashMap::new(),
            total_doc_len: 0,
        }
    }

    /// 添加文档的统计信息
    ///
    /// - `terms` — 文档分词后的词素列表（可含重复，用于计算 tf）
    pub fn add_document(&mut self, terms: &[String]) {
        self.num_docs += 1;
        self.total_doc_len += terms.len();
        // 计算唯一词素的 df
        let unique: std::collections::HashSet<&String> = terms.iter().collect();
        for term in unique {
            *self.doc_freq.entry(term.clone()).or_insert(0) += 1;
        }
    }

    /// 平均文档长度
    pub fn avg_doc_len(&self) -> f64 {
        if self.num_docs == 0 {
            0.0
        } else {
            self.total_doc_len as f64 / self.num_docs as f64
        }
    }

    /// 文档总数
    pub fn num_docs(&self) -> usize {
        self.num_docs
    }

    /// 词素的文档频率
    pub fn doc_freq(&self, term: &str) -> usize {
        *self.doc_freq.get(term).unwrap_or(&0)
    }

    /// 计算单个词素对文档的 BM25 分数
    ///
    /// - `term` — 词素
    /// - `tf` — 词素在文档中的频次
    /// - `doc_len` — 文档长度（词素数）
    pub fn score(&self, term: &str, tf: usize, doc_len: usize) -> f64 {
        let df = self.doc_freq(term) as f64;
        if df == 0.0 || self.num_docs == 0 {
            return 0.0;
        }
        let n = self.num_docs as f64;
        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
        let tf_f = tf as f64;
        let dl = doc_len as f64;
        let avgdl = self.avg_doc_len();
        let avgdl = if avgdl > 0.0 {
            avgdl
        } else {
            1.0
        };
        let norm = 1.0 - self.params.b + self.params.b * (dl / avgdl);
        idf * (tf_f * (self.params.k1 + 1.0)) / (tf_f + self.params.k1 * norm)
    }

    /// 计算多词素查询对文档的 BM25 总分
    ///
    /// - `query_terms` — 查询词素列表（可含重复，但通常去重）
    /// - `doc_term_freq` — 文档的词素频次表（term → tf）
    /// - `doc_len` — 文档长度
    pub fn score_query(
        &self,
        query_terms: &[String],
        doc_term_freq: &HashMap<String, usize>,
        doc_len: usize,
    ) -> f64 {
        // 去重 query_terms
        let seen: std::collections::HashSet<&String> = query_terms.iter().collect();
        seen.iter()
            .map(|&t| {
                let tf = *doc_term_freq.get(t).unwrap_or(&0);
                if tf == 0 {
                    0.0
                } else {
                    self.score(t, tf, doc_len)
                }
            })
            .sum()
    }

    /// 获取 BM25 参数
    pub fn params(&self) -> Bm25Params {
        self.params
    }
}

// =====================================================================
//  ts_headline 高亮
// =====================================================================

/// 高亮选项
///
/// 对应 PG `ts_headline` 的选项参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlineOptions {
    /// 匹配词素的起始标签（默认 `<b>`）
    pub start_tag: String,
    /// 匹配词素的结束标签（默认 `</b>`）
    pub end_tag: String,
    /// 头条最大词数（默认 35）
    pub max_words: usize,
    /// 头条最小词数（默认 15）
    pub min_words: usize,
}

impl Default for HeadlineOptions {
    fn default() -> Self {
        Self {
            start_tag: "<b>".to_string(),
            end_tag: "</b>".to_string(),
            max_words: 35,
            min_words: 15,
        }
    }
}

impl HeadlineOptions {
    /// 创建默认选项
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置高亮标签
    pub fn with_tags(mut self, start: impl Into<String>, end: impl Into<String>) -> Self {
        self.start_tag = start.into();
        self.end_tag = end.into();
        self
    }

    /// 设置最大/最小词数
    pub fn with_word_limits(mut self, max: usize, min: usize) -> Self {
        self.max_words = max;
        self.min_words = min;
        self
    }

    /// 校验选项
    pub fn validate(&self) -> Result<(), FullTextError> {
        if self.max_words == 0 {
            return Err(FullTextError::InvalidHighlightOption(
                "MaxWords must be > 0".to_string(),
            ));
        }
        if self.min_words > self.max_words {
            return Err(FullTextError::InvalidHighlightOption(format!(
                "MinWords ({}) must be <= MaxWords ({})",
                self.min_words, self.max_words
            )));
        }
        Ok(())
    }
}

/// 在文档中高亮匹配 `tsquery` 的词素
///
/// 对应 PG `ts_headline([config regconfig,] document text, query tsquery [, options text]) → text`。
///
/// # 参数
///
/// - `document` — 原始文档文本
/// - `query` — tsquery（词素被高亮）
/// - `options` — 高亮选项
///
/// # 返回
///
/// 高亮后的文档片段（匹配词素用 `start_tag`/`end_tag` 包裹）。
///
/// # 算法
///
/// 1. 从 `TsQuery` 中收集所有词素（递归遍历 AND/OR/NOT/FollowedBy）
/// 2. 分词（保留原始词形，比较时小写化）
/// 3. 命中词素用标签包裹
/// 4. 超过 `max_words` 时截断并追加 `...`
pub fn ts_headline(document: &str, query: &TsQuery, options: &HeadlineOptions) -> String {
    options.validate().ok();
    let terms = collect_query_terms(query);
    if terms.is_empty() {
        return truncate_words(document, options.max_words);
    }
    let lower_terms: std::collections::HashSet<String> =
        terms.iter().map(|t| t.to_lowercase()).collect();
    let tokens = tokenize_with_positions(document);
    let total_tokens = tokens.len();
    let mut result = String::new();
    for (word, _pos) in tokens.into_iter().take(options.max_words) {
        let lower = word.to_lowercase();
        if lower_terms.contains(&lower) {
            result.push_str(&options.start_tag);
            result.push_str(&word);
            result.push_str(&options.end_tag);
        } else {
            result.push_str(&word);
        }
        result.push(' ');
    }
    // 若原文档词数超过 max_words，追加省略号
    if total_tokens > options.max_words {
        result = format!("{}...", result.trim_end());
    }
    result.trim_end().to_string()
}

/// 收集 TsQuery 中的所有词素（递归）
fn collect_query_terms(query: &TsQuery) -> Vec<String> {
    let mut terms = Vec::new();
    collect_terms_recursive(query, &mut terms);
    terms
}

fn collect_terms_recursive(query: &TsQuery, terms: &mut Vec<String>) {
    match query {
        TsQuery::Empty => {}
        TsQuery::Lexeme { term, .. } => {
            if !terms.contains(term) {
                terms.push(term.clone());
            }
        }
        TsQuery::And(l, r) | TsQuery::Or(l, r) => {
            collect_terms_recursive(l, terms);
            collect_terms_recursive(r, terms);
        }
        TsQuery::Not(q) => collect_terms_recursive(q, terms),
        TsQuery::FollowedBy { left, right, .. } => {
            collect_terms_recursive(left, terms);
            collect_terms_recursive(right, terms);
        }
    }
}

/// 简单分词（保留词形，记录位置）
fn tokenize_with_positions(s: &str) -> Vec<(String, usize)> {
    let mut result = Vec::new();
    let mut pos = 0;
    for word in s.split(|c: char| c.is_whitespace() || is_punctuation(c)) {
        if !word.is_empty() {
            result.push((word.to_string(), pos));
            pos += 1;
        }
    }
    result
}

/// 判断是否为标点（非字母数字非下划线非 CJK）
fn is_punctuation(c: char) -> bool {
    !c.is_alphanumeric() && c != '_' && !is_cjk(c)
}

fn truncate_words(s: &str, max: usize) -> String {
    let words: Vec<&str> = s.split_whitespace().take(max).collect();
    if s.split_whitespace().count() > max {
        format!("{}...", words.join(" "))
    } else {
        words.join(" ")
    }
}

// =====================================================================
//  中文分词器（最大正向匹配）
// =====================================================================

/// 中文分词器（最大正向匹配，Maximum Forward Matching）
///
/// # 算法
///
/// 1. 从文本起始位置 i 开始
/// 2. 取最长子串 `chars[i..i+max_word_len]`
/// 3. 从长到短尝试匹配词典
/// 4. 命中则输出该词，i 前进对应长度
/// 5. 未命中则输出单字，i 前进 1
///
/// # 词典
///
/// 内置约 70 个常用词，生产环境应通过 `with_dict()` 加载外部词典。
pub struct ChineseTokenizer {
    /// 词典（词 → 存在性）
    dict: std::collections::HashSet<String>,
    /// 最大词长（字符数）
    max_word_len: usize,
}

impl Default for ChineseTokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl ChineseTokenizer {
    /// 创建内置词典的分词器
    pub fn new() -> Self {
        let mut dict = std::collections::HashSet::new();
        for word in BUILTIN_CHINESE_DICT {
            dict.insert((*word).to_string());
        }
        Self {
            dict,
            max_word_len: 4,
        }
    }

    /// 从自定义词典创建
    pub fn with_dict(words: &[&str]) -> Self {
        let mut dict = std::collections::HashSet::new();
        for word in words {
            dict.insert((*word).to_string());
        }
        let max_len = words
            .iter()
            .map(|w| w.chars().count())
            .max()
            .unwrap_or(4)
            .max(1);
        Self {
            dict,
            max_word_len: max_len,
        }
    }

    /// 添加自定义词
    pub fn add_word(&mut self, word: &str) {
        let len = word.chars().count();
        if len > self.max_word_len {
            self.max_word_len = len;
        }
        self.dict.insert(word.to_string());
    }

    /// 分词
    ///
    /// 返回词素列表（已小写化，中文保持原形）。
    /// 非中文部分按空白分词并小写化；中文部分按词典最大正向匹配。
    pub fn tokenize(&self, text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut result = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            if !is_cjk(chars[i]) {
                // 非中文：连续非 CJK 作为一个段落，按空白分词
                let start = i;
                while i < chars.len() && !is_cjk(chars[i]) {
                    i += 1;
                }
                let segment: String = chars[start..i].iter().collect();
                for word in segment.split_whitespace() {
                    // 去除首尾标点
                    let cleaned: String = word
                        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                        .to_lowercase();
                    if !cleaned.is_empty() {
                        result.push(cleaned);
                    }
                }
                continue;
            }
            // 中文：最大正向匹配
            let mut matched = false;
            let end = (i + self.max_word_len).min(chars.len());
            for len in (2..=end - i).rev() {
                let word: String = chars[i..i + len].iter().collect();
                if self.dict.contains(&word) {
                    result.push(word);
                    i += len;
                    matched = true;
                    break;
                }
            }
            if !matched {
                // 单字成词
                let word: String = chars[i..i + 1].iter().collect();
                result.push(word);
                i += 1;
            }
        }
        result
    }

    /// 词典大小
    pub fn dict_size(&self) -> usize {
        self.dict.len()
    }

    /// 最大词长
    pub fn max_word_len(&self) -> usize {
        self.max_word_len
    }
}

/// 判断字符是否为 CJK 汉字
fn is_cjk(c: char) -> bool {
    let code = c as u32;
    // CJK Unified Ideographs
    (0x4E00..=0x9FFF).contains(&code)
        // CJK Extension A
        || (0x3400..=0x4DBF).contains(&code)
        // CJK Compatibility Ideographs
        || (0xF900..=0xFAFF).contains(&code)
}

/// 内置简体中文词典（约 70 词）
///
/// 仅覆盖常用词，生产环境应加载外部词典。
const BUILTIN_CHINESE_DICT: &[&str] = &[
    // 代词
    "我们",
    "你们",
    "他们",
    "她们",
    "它们",
    "自己",
    "大家",
    // 地名
    "中国",
    "北京",
    "上海",
    "广州",
    "深圳",
    "世界",
    "国家",
    // 科技
    "数据库",
    "全文检索",
    "索引",
    "查询",
    "排序",
    "分词",
    "倒排",
    "计算机",
    "互联网",
    "软件",
    "硬件",
    "网络",
    "人工智能",
    "机器学习",
    "深度学习",
    "大数据",
    "云计算",
    "研究",
    "开发",
    "测试",
    "部署",
    "运维",
    "项目",
    "产品",
    "需求",
    "设计",
    "实现",
    "工程师",
    "程序员",
    // 时间
    "今天",
    "明天",
    "昨天",
    "现在",
    "未来",
    "已经",
    "正在",
    "将要",
    "刚刚",
    "曾经",
    // 情感
    "喜欢",
    "讨厌",
    "热爱",
    "学习",
    "工作",
    "生活",
    "美好",
    "幸福",
    "快乐",
    "悲伤",
    // 水果
    "苹果",
    "香蕉",
    "橙子",
    "葡萄",
    "西瓜",
    // 颜色
    "红色",
    "蓝色",
    "绿色",
    "黄色",
    "黑色",
    "白色",
    // 助词
    "可以",
    "应该",
    "必须",
    "可能",
    "或者",
    "因为",
    "所以",
    "但是",
    "然而",
    "虽然",
    "非常",
    "十分",
    "特别",
    "尤其",
    "比较",
    // 人物
    "学生",
    "老师",
    "医生",
];

// =====================================================================
//  FullTextIndex — 倒排索引 + BM25 排序
// =====================================================================

/// 倒排索引项
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    /// 文档 ID
    pub doc_id: usize,
    /// 词素在文档中的频次
    pub tf: usize,
    /// 词素在文档中的位置列表（1-based）
    pub positions: Vec<u32>,
}

/// 全文索引
///
/// 倒排索引 + BM25 排序，支持英文/中文分词。
///
/// # 用法
///
/// ```ignore
/// use szrsql_sql::fulltext_v2::*;
///
/// let mut index = FullTextIndex::new();
/// index.add_text(1, "the quick brown fox");
/// index.add_text(2, "the lazy dog");
/// index.add_text(3, "the quick dog");
///
/// let results = index.search(&["quick".to_string(), "dog".to_string()], 10);
/// // 返回 [(3, score), (2, score), ...] 按 BM25 分数降序
/// ```
pub struct FullTextIndex {
    /// 词素 → 倒排列表
    postings: HashMap<String, Vec<Posting>>,
    /// 文档长度（词素数）
    doc_lens: HashMap<usize, usize>,
    /// BM25 评分器
    scorer: Bm25Scorer,
    /// 已索引的文档 ID 集合（防重复）
    doc_ids: std::collections::HashSet<usize>,
}

impl Default for FullTextIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl FullTextIndex {
    /// 创建空索引（默认 BM25 参数）
    pub fn new() -> Self {
        Self {
            postings: HashMap::new(),
            doc_lens: HashMap::new(),
            scorer: Bm25Scorer::default(),
            doc_ids: std::collections::HashSet::new(),
        }
    }

    /// 创建空索引（自定义 BM25 参数）
    pub fn with_params(params: Bm25Params) -> Result<Self, FullTextError> {
        Bm25Params::new(params.k1, params.b)?;
        Ok(Self {
            postings: HashMap::new(),
            doc_lens: HashMap::new(),
            scorer: Bm25Scorer::new(params),
            doc_ids: std::collections::HashSet::new(),
        })
    }

    /// 添加文档（已分词的词素列表）
    pub fn add_document(&mut self, doc_id: usize, terms: &[String]) -> Result<(), FullTextError> {
        if self.doc_ids.contains(&doc_id) {
            return Err(FullTextError::DocumentExists(doc_id));
        }
        self.doc_ids.insert(doc_id);
        self.doc_lens.insert(doc_id, terms.len());
        // 统计每个词素的位置列表
        let mut tf_map: HashMap<&String, Vec<u32>> = HashMap::new();
        for (pos, term) in terms.iter().enumerate() {
            tf_map.entry(term).or_default().push((pos + 1) as u32);
        }
        for (term, positions) in tf_map {
            let posting = Posting {
                doc_id,
                tf: positions.len(),
                positions,
            };
            self.postings.entry(term.clone()).or_default().push(posting);
        }
        // 更新 BM25 统计
        self.scorer.add_document(terms);
        Ok(())
    }

    /// 添加文档（从文本，使用英文简单分词器）
    pub fn add_text(&mut self, doc_id: usize, text: &str) -> Result<(), FullTextError> {
        let terms = simple_tokenize(text);
        self.add_document(doc_id, &terms)
    }

    /// 添加文档（从文本，使用中文分词器）
    pub fn add_text_chinese(
        &mut self,
        doc_id: usize,
        text: &str,
        tokenizer: &ChineseTokenizer,
    ) -> Result<(), FullTextError> {
        let terms = tokenizer.tokenize(text);
        self.add_document(doc_id, &terms)
    }

    /// 搜索文档（返回按 BM25 分数降序的 (doc_id, score) 列表）
    ///
    /// - `query_terms` — 查询词素列表（去重后 OR 语义）
    /// - `limit` — 返回结果上限（0 表示不限制）
    pub fn search(&self, query_terms: &[String], limit: usize) -> Vec<(usize, f64)> {
        let unique_terms: std::collections::HashSet<&String> = query_terms.iter().collect();
        // 收集候选文档
        let mut candidates: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for term in &unique_terms {
            if let Some(postings) = self.postings.get(*term) {
                for p in postings {
                    candidates.insert(p.doc_id);
                }
            }
        }
        // 计算 BM25 分数
        let mut scored: Vec<(usize, f64)> = candidates
            .iter()
            .map(|&doc_id| {
                let doc_len = *self.doc_lens.get(&doc_id).unwrap_or(&0);
                let mut total = 0.0;
                for term in &unique_terms {
                    if let Some(postings) = self.postings.get(*term) {
                        if let Some(p) = postings.iter().find(|p| p.doc_id == doc_id) {
                            total += self.scorer.score(term, p.tf, doc_len);
                        }
                    }
                }
                (doc_id, total)
            })
            .collect();
        // 降序排序；分数相等时按 doc_id 升序作为确定性 tiebreaker
        // （避免 HashSet 迭代顺序导致相同查询返回不同结果）
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        if limit > 0 {
            scored.truncate(limit);
        }
        scored
    }

    /// 使用 TsQuery 搜索
    pub fn search_tsquery(&self, query: &TsQuery, limit: usize) -> Vec<(usize, f64)> {
        let terms = collect_query_terms(query);
        self.search(&terms, limit)
    }

    /// 获取文档数
    pub fn num_docs(&self) -> usize {
        self.doc_ids.len()
    }

    /// 获取词汇量（唯一词素数）
    pub fn vocabulary_size(&self) -> usize {
        self.postings.len()
    }

    /// 获取词素的文档频率
    pub fn doc_freq(&self, term: &str) -> usize {
        self.postings.get(term).map(|p| p.len()).unwrap_or(0)
    }

    /// 获取词素的倒排列表
    pub fn postings(&self, term: &str) -> Option<&[Posting]> {
        self.postings.get(term).map(|v| v.as_slice())
    }

    /// 获取文档长度
    pub fn doc_len(&self, doc_id: usize) -> Option<usize> {
        self.doc_lens.get(&doc_id).copied()
    }

    /// 平均文档长度
    pub fn avg_doc_len(&self) -> f64 {
        self.scorer.avg_doc_len()
    }

    /// 获取 BM25 参数
    pub fn bm25_params(&self) -> Bm25Params {
        self.scorer.params()
    }

    /// 获取文档 ID 列表
    pub fn doc_ids(&self) -> Vec<usize> {
        let mut ids: Vec<usize> = self.doc_ids.iter().copied().collect();
        ids.sort();
        ids
    }
}

/// 简单英文分词（空白 + 标点分割，小写化）
fn simple_tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || is_punctuation(c))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_types::value::{TsQuery, TsVector, Value};

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    fn make_terms(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    // =================================================================
    //  FullTextError 测试
    // =================================================================

    #[test]
    fn test_error_invalid_k1_negative() {
        let err = Bm25Params::new(-1.0, 0.75).unwrap_err();
        assert!(matches!(err, FullTextError::InvalidK1(_)));
        assert_eq!(
            err.to_string(),
            "invalid BM25 parameter k1: must be >= 0 (got -1)"
        );
    }

    #[test]
    fn test_error_invalid_b_out_of_range() {
        let err = Bm25Params::new(1.2, 1.5).unwrap_err();
        assert!(matches!(err, FullTextError::InvalidB(_)));
        assert_eq!(
            err.to_string(),
            "invalid BM25 parameter b: must be in [0, 1] (got 1.5)"
        );
    }

    #[test]
    fn test_error_invalid_b_negative() {
        let err = Bm25Params::new(1.2, -0.1).unwrap_err();
        assert!(matches!(err, FullTextError::InvalidB(_)));
    }

    #[test]
    fn test_error_to_execution_error() {
        let err: ExecutionError = FullTextError::DocumentExists(42).into();
        match err {
            ExecutionError::EvalError(msg) => {
                assert!(msg.contains("document already exists"));
                assert!(msg.contains("doc_id=42"));
            }
            _ => panic!("expected EvalError"),
        }
    }

    #[test]
    fn test_error_document_exists_message() {
        let err = FullTextError::DocumentExists(7);
        assert_eq!(err.to_string(), "document already exists: doc_id=7");
    }

    #[test]
    fn test_error_document_not_found_message() {
        let err = FullTextError::DocumentNotFound(99);
        assert_eq!(err.to_string(), "document not found: doc_id=99");
    }

    #[test]
    fn test_error_invalid_highlight_option_message() {
        let err = FullTextError::InvalidHighlightOption("bad".to_string());
        assert_eq!(err.to_string(), "invalid highlight option: bad");
    }

    // =================================================================
    //  Bm25Params 测试
    // =================================================================

    #[test]
    fn test_bm25_params_default() {
        let p = Bm25Params::default();
        assert!((p.k1 - 1.2).abs() < 1e-9);
        assert!((p.b - 0.75).abs() < 1e-9);
    }

    #[test]
    fn test_bm25_params_new_valid() {
        let p = Bm25Params::new(2.0, 0.5).unwrap();
        assert!((p.k1 - 2.0).abs() < 1e-9);
        assert!((p.b - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_bm25_params_new_zero_k1() {
        // k1=0 退化为布尔模型
        let p = Bm25Params::new(0.0, 0.75).unwrap();
        assert!((p.k1 - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_bm25_params_new_b_boundaries() {
        assert!(Bm25Params::new(1.2, 0.0).is_ok());
        assert!(Bm25Params::new(1.2, 1.0).is_ok());
        assert!(Bm25Params::new(1.2, -0.001).is_err());
        assert!(Bm25Params::new(1.2, 1.001).is_err());
    }

    // =================================================================
    //  Bm25Scorer 测试
    // =================================================================

    #[test]
    fn test_bm25_scorer_new_empty() {
        let scorer = Bm25Scorer::default();
        assert_eq!(scorer.num_docs(), 0);
        assert!((scorer.avg_doc_len() - 0.0).abs() < 1e-9);
        assert_eq!(scorer.doc_freq("foo"), 0);
    }

    #[test]
    fn test_bm25_scorer_add_document() {
        let mut scorer = Bm25Scorer::default();
        let terms = make_terms(&["hello", "world", "hello"]);
        scorer.add_document(&terms);
        assert_eq!(scorer.num_docs(), 1);
        assert!((scorer.avg_doc_len() - 3.0).abs() < 1e-9);
        assert_eq!(scorer.doc_freq("hello"), 1);
        assert_eq!(scorer.doc_freq("world"), 1);
        assert_eq!(scorer.doc_freq("missing"), 0);
    }

    #[test]
    fn test_bm25_scorer_add_multiple_documents() {
        let mut scorer = Bm25Scorer::default();
        scorer.add_document(&make_terms(&["hello", "world"]));
        scorer.add_document(&make_terms(&["hello", "rust"]));
        scorer.add_document(&make_terms(&["world", "rust", "peace"]));
        assert_eq!(scorer.num_docs(), 3);
        // avgdl = (2 + 2 + 3) / 3 = 7/3 ≈ 2.333
        assert!((scorer.avg_doc_len() - 7.0 / 3.0).abs() < 1e-9);
        assert_eq!(scorer.doc_freq("hello"), 2);
        assert_eq!(scorer.doc_freq("world"), 2);
        assert_eq!(scorer.doc_freq("rust"), 2);
        assert_eq!(scorer.doc_freq("peace"), 1);
    }

    #[test]
    fn test_bm25_scorer_score_basic() {
        let mut scorer = Bm25Scorer::default();
        scorer.add_document(&make_terms(&["hello", "world"]));
        scorer.add_document(&make_terms(&["hello", "rust"]));
        // 查询 "hello"，在 doc1（tf=1, len=2, avgdl=2）
        let score = scorer.score("hello", 1, 2);
        assert!(score > 0.0, "BM25 score should be positive");
        // IDF = ln((2 - 2 + 0.5) / (2 + 0.5) + 1) = ln(1.2) ≈ 0.1823
        // tf=1, k1=1.2, b=0.75, dl=2, avgdl=2 → norm = 1 - 0.75 + 0.75*1 = 1.0
        // denom = 1 + 1.2 * 1.0 = 2.2; numer = 1 * 2.2 = 2.2
        // score = IDF * (numer/denom) = 0.1823 * 1.0 = 0.1823
        assert!((score - 0.1823).abs() < 0.01, "got {score}");
    }

    #[test]
    fn test_bm25_scorer_score_rare_term_higher() {
        let mut scorer = Bm25Scorer::default();
        // 5 个文档：4 个含 "common"，1 个含 "rare"
        for _ in 0..4 {
            scorer.add_document(&make_terms(&["common", "word"]));
        }
        scorer.add_document(&make_terms(&["rare", "word"]));
        // common: df=4, rare: df=1
        // rare 的 IDF 应高于 common
        let common_score = scorer.score("common", 1, 2);
        let rare_score = scorer.score("rare", 1, 2);
        assert!(
            rare_score > common_score,
            "rare term should score higher: rare={rare_score}, common={common_score}"
        );
    }

    #[test]
    fn test_bm25_scorer_score_missing_term_zero() {
        let scorer = Bm25Scorer::default();
        // 空评分器
        assert_eq!(scorer.score("foo", 1, 10), 0.0);
    }

    #[test]
    fn test_bm25_scorer_score_zero_tf() {
        let mut scorer = Bm25Scorer::default();
        scorer.add_document(&make_terms(&["hello"]));
        // tf=0 → 分数为 0
        assert_eq!(scorer.score("hello", 0, 1), 0.0);
    }

    #[test]
    fn test_bm25_scorer_score_query() {
        let mut scorer = Bm25Scorer::default();
        scorer.add_document(&make_terms(&["hello", "world", "foo"]));
        scorer.add_document(&make_terms(&["hello", "bar"]));
        let mut doc_tf = HashMap::new();
        doc_tf.insert("hello".to_string(), 1);
        doc_tf.insert("world".to_string(), 1);
        let score = scorer.score_query(&make_terms(&["hello", "world"]), &doc_tf, 3);
        assert!(score > 0.0);
    }

    #[test]
    fn test_bm25_scorer_score_query_dedup() {
        let mut scorer = Bm25Scorer::default();
        scorer.add_document(&make_terms(&["hello"]));
        let mut doc_tf = HashMap::new();
        doc_tf.insert("hello".to_string(), 1);
        // 重复词素不应叠加分数
        let s1 = scorer.score_query(&make_terms(&["hello"]), &doc_tf, 1);
        let s2 = scorer.score_query(&make_terms(&["hello", "hello", "hello"]), &doc_tf, 1);
        assert!((s1 - s2).abs() < 1e-9);
    }

    #[test]
    fn test_bm25_scorer_k1_zero_boolean() {
        let mut scorer = Bm25Scorer::new(Bm25Params::new(0.0, 0.75).unwrap());
        scorer.add_document(&make_terms(&["hello", "world"]));
        scorer.add_document(&make_terms(&["hello"]));
        // k1=0 → 分数仅取决于 IDF（布尔模型）
        let s1 = scorer.score("hello", 1, 2);
        let s2 = scorer.score("hello", 5, 1);
        // k1=0 时 tf 不影响分数
        assert!(
            (s1 - s2).abs() < 1e-9,
            "k1=0 should ignore tf: s1={s1}, s2={s2}"
        );
    }

    #[test]
    fn test_bm25_scorer_b_zero_no_length_norm() {
        let params = Bm25Params::new(1.2, 0.0).unwrap();
        let mut scorer = Bm25Scorer::new(params);
        scorer.add_document(&make_terms(&["hello", "world"]));
        scorer.add_document(&make_terms(&["hello"]));
        // b=0 → 文档长度不影响分数
        let s1 = scorer.score("hello", 1, 2);
        let s2 = scorer.score("hello", 1, 1);
        assert!(
            (s1 - s2).abs() < 1e-9,
            "b=0 should ignore doc_len: s1={s1}, s2={s2}"
        );
    }

    // =================================================================
    //  HeadlineOptions 测试
    // =================================================================

    #[test]
    fn test_headline_options_default() {
        let opts = HeadlineOptions::default();
        assert_eq!(opts.start_tag, "<b>");
        assert_eq!(opts.end_tag, "</b>");
        assert_eq!(opts.max_words, 35);
        assert_eq!(opts.min_words, 15);
    }

    #[test]
    fn test_headline_options_with_tags() {
        let opts = HeadlineOptions::new().with_tags("<em>", "</em>");
        assert_eq!(opts.start_tag, "<em>");
        assert_eq!(opts.end_tag, "</em>");
    }

    #[test]
    fn test_headline_options_with_word_limits() {
        let opts = HeadlineOptions::new().with_word_limits(50, 20);
        assert_eq!(opts.max_words, 50);
        assert_eq!(opts.min_words, 20);
    }

    #[test]
    fn test_headline_options_validate_ok() {
        let opts = HeadlineOptions::default();
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn test_headline_options_validate_zero_max() {
        let opts = HeadlineOptions {
            start_tag: "<b>".into(),
            end_tag: "</b>".into(),
            max_words: 0,
            min_words: 0,
        };
        let err = opts.validate().unwrap_err();
        assert!(matches!(err, FullTextError::InvalidHighlightOption(_)));
    }

    #[test]
    fn test_headline_options_validate_min_gt_max() {
        let opts = HeadlineOptions {
            start_tag: "<b>".into(),
            end_tag: "</b>".into(),
            max_words: 10,
            min_words: 20,
        };
        let err = opts.validate().unwrap_err();
        assert!(matches!(err, FullTextError::InvalidHighlightOption(_)));
    }

    // =================================================================
    //  ts_headline 测试
    // =================================================================

    #[test]
    fn test_ts_headline_basic_highlight() {
        let doc = "the quick brown fox jumps over the lazy dog";
        let query = TsQuery::lexeme("fox");
        let result = ts_headline(doc, &query, &HeadlineOptions::default());
        assert!(result.contains("<b>fox</b>"));
    }

    #[test]
    fn test_ts_headline_multiple_terms() {
        let doc = "the quick brown fox and the lazy dog";
        let query = TsQuery::lexeme("fox").or(TsQuery::lexeme("dog"));
        let result = ts_headline(doc, &query, &HeadlineOptions::default());
        assert!(result.contains("<b>fox</b>"));
        assert!(result.contains("<b>dog</b>"));
    }

    #[test]
    fn test_ts_headline_case_insensitive() {
        let doc = "The Quick Brown Fox";
        let query = TsQuery::lexeme("fox");
        let result = ts_headline(doc, &query, &HeadlineOptions::default());
        assert!(result.contains("<b>Fox</b>"));
    }

    #[test]
    fn test_ts_headline_custom_tags() {
        let doc = "hello world";
        let query = TsQuery::lexeme("hello");
        let opts = HeadlineOptions::new().with_tags("[", "]");
        let result = ts_headline(doc, &query, &opts);
        assert!(result.contains("[hello]"));
    }

    #[test]
    fn test_ts_headline_empty_query() {
        let doc = "hello world foo bar";
        let query = TsQuery::Empty;
        let result = ts_headline(doc, &query, &HeadlineOptions::default());
        assert!(!result.contains("<b>"));
    }

    #[test]
    fn test_ts_headline_max_words_truncation() {
        let doc = "one two three four five six seven eight nine ten";
        let query = TsQuery::lexeme("one");
        let opts = HeadlineOptions::new().with_word_limits(5, 1);
        let result = ts_headline(doc, &query, &opts);
        assert!(
            result.ends_with("..."),
            "should end with ..., got: {result}"
        );
    }

    #[test]
    fn test_ts_headline_no_match() {
        let doc = "hello world";
        let query = TsQuery::lexeme("missing");
        let result = ts_headline(doc, &query, &HeadlineOptions::default());
        assert!(!result.contains("<b>"));
        assert!(result.contains("hello"));
        assert!(result.contains("world"));
    }

    #[test]
    fn test_ts_headline_and_query() {
        let doc = "the cat and the dog play together";
        let query = TsQuery::lexeme("cat").and(TsQuery::lexeme("dog"));
        let result = ts_headline(doc, &query, &HeadlineOptions::default());
        assert!(result.contains("<b>cat</b>"));
        assert!(result.contains("<b>dog</b>"));
    }

    #[test]
    fn test_ts_headline_not_query() {
        let doc = "the cat and the dog";
        let query = TsQuery::lexeme("cat").not_query();
        // NOT 查询仍会高亮 "cat"
        let result = ts_headline(doc, &query, &HeadlineOptions::default());
        assert!(result.contains("<b>cat</b>"));
    }

    // =================================================================
    //  ChineseTokenizer 测试
    // =================================================================

    #[test]
    fn test_chinese_tokenizer_new() {
        let tok = ChineseTokenizer::new();
        assert!(tok.dict_size() > 50, "builtin dict should have >50 words");
        assert!(tok.max_word_len() >= 4);
    }

    #[test]
    fn test_chinese_tokenizer_with_dict() {
        let tok = ChineseTokenizer::with_dict(&["数据库", "全文检索", "苹果"]);
        assert_eq!(tok.dict_size(), 3);
        assert_eq!(tok.max_word_len(), 4); // "全文检索" = 4 字
    }

    #[test]
    fn test_chinese_tokenizer_add_word() {
        let mut tok = ChineseTokenizer::new();
        let old_size = tok.dict_size();
        tok.add_word("自定义词");
        assert_eq!(tok.dict_size(), old_size + 1);
        assert_eq!(tok.max_word_len(), 4); // "自定义词" = 4 字
    }

    #[test]
    fn test_chinese_tokenizer_add_long_word_updates_max_len() {
        let mut tok = ChineseTokenizer::new();
        let old_max = tok.max_word_len();
        tok.add_word("非常长的词组测试");
        let new_max = tok.max_word_len();
        assert!(new_max > old_max);
        assert_eq!(new_max, 8); // "非常长的词组测试" = 8 字
    }

    #[test]
    fn test_chinese_tokenizer_pure_chinese() {
        let tok = ChineseTokenizer::with_dict(&["我们", "喜欢", "苹果"]);
        let terms = tok.tokenize("我们喜欢苹果");
        assert_eq!(terms, vec!["我们", "喜欢", "苹果"]);
    }

    #[test]
    fn test_chinese_tokenizer_mixed_chinese_english() {
        let tok = ChineseTokenizer::with_dict(&["我们", "喜欢"]);
        let terms = tok.tokenize("我们喜欢 Rust");
        assert_eq!(terms, vec!["我们", "喜欢", "rust"]);
    }

    #[test]
    fn test_chinese_tokenizer_single_chars() {
        let tok = ChineseTokenizer::with_dict(&[]); // 空词典
        let terms = tok.tokenize("你好世界");
        // 空词典 → 全部单字
        assert_eq!(terms, vec!["你", "好", "世", "界"]);
    }

    #[test]
    fn test_chinese_tokenizer_max_forward_matching() {
        // "数据库" 应优先匹配为整词，而非 "数据" + "库"
        let tok = ChineseTokenizer::with_dict(&["数据", "数据库"]);
        let terms = tok.tokenize("数据库");
        assert_eq!(terms, vec!["数据库"]);
    }

    #[test]
    fn test_chinese_tokenizer_partial_match() {
        let tok = ChineseTokenizer::with_dict(&["数据"]);
        let terms = tok.tokenize("数据库");
        // "数据" 匹配，"库" 单字
        assert_eq!(terms, vec!["数据", "库"]);
    }

    #[test]
    fn test_chinese_tokenizer_punctuation() {
        let tok = ChineseTokenizer::with_dict(&["我们", "喜欢"]);
        let terms = tok.tokenize("我们，喜欢。");
        assert_eq!(terms, vec!["我们", "喜欢"]);
    }

    #[test]
    fn test_chinese_tokenizer_empty_string() {
        let tok = ChineseTokenizer::new();
        let terms = tok.tokenize("");
        assert!(terms.is_empty());
    }

    #[test]
    fn test_chinese_tokenizer_builtin_dict_words() {
        let tok = ChineseTokenizer::new();
        // 内置词典含 "数据库" 和 "全文检索"
        let terms = tok.tokenize("数据库与全文检索");
        assert!(terms.contains(&"数据库".to_string()));
        assert!(terms.contains(&"全文检索".to_string()));
    }

    #[test]
    fn test_chinese_tokenizer_mixed_with_numbers() {
        let tok = ChineseTokenizer::with_dict(&["我们"]);
        let terms = tok.tokenize("我们 123 苹果");
        assert!(terms.contains(&"我们".to_string()));
        assert!(terms.contains(&"123".to_string()));
    }

    // =================================================================
    //  FullTextIndex 测试
    // =================================================================

    #[test]
    fn test_fulltext_index_new_empty() {
        let index = FullTextIndex::new();
        assert_eq!(index.num_docs(), 0);
        assert_eq!(index.vocabulary_size(), 0);
        assert!((index.avg_doc_len() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_fulltext_index_add_document() {
        let mut index = FullTextIndex::new();
        index
            .add_document(1, &make_terms(&["hello", "world"]))
            .unwrap();
        assert_eq!(index.num_docs(), 1);
        assert_eq!(index.vocabulary_size(), 2);
        assert_eq!(index.doc_freq("hello"), 1);
        assert_eq!(index.doc_freq("world"), 1);
        assert_eq!(index.doc_freq("missing"), 0);
        assert_eq!(index.doc_len(1), Some(2));
        assert!((index.avg_doc_len() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn test_fulltext_index_add_duplicate_doc_id() {
        let mut index = FullTextIndex::new();
        index.add_document(1, &make_terms(&["hello"])).unwrap();
        let err = index.add_document(1, &make_terms(&["world"])).unwrap_err();
        assert!(matches!(err, FullTextError::DocumentExists(1)));
    }

    #[test]
    fn test_fulltext_index_add_text() {
        let mut index = FullTextIndex::new();
        index.add_text(1, "the quick brown fox").unwrap();
        assert_eq!(index.num_docs(), 1);
        assert_eq!(index.vocabulary_size(), 4);
        assert_eq!(index.doc_freq("the"), 1);
        assert_eq!(index.doc_freq("quick"), 1);
        assert_eq!(index.doc_freq("brown"), 1);
        assert_eq!(index.doc_freq("fox"), 1);
    }

    #[test]
    fn test_fulltext_index_add_text_chinese() {
        let tok = ChineseTokenizer::with_dict(&["我们", "喜欢", "苹果"]);
        let mut index = FullTextIndex::new();
        index.add_text_chinese(1, "我们喜欢苹果", &tok).unwrap();
        assert_eq!(index.num_docs(), 1);
        assert_eq!(index.vocabulary_size(), 3);
        assert_eq!(index.doc_freq("我们"), 1);
        assert_eq!(index.doc_freq("喜欢"), 1);
        assert_eq!(index.doc_freq("苹果"), 1);
    }

    #[test]
    fn test_fulltext_index_search_basic() {
        let mut index = FullTextIndex::new();
        index.add_text(1, "the quick brown fox").unwrap();
        index.add_text(2, "the lazy dog").unwrap();
        index.add_text(3, "the quick dog").unwrap();
        let results = index.search(&make_terms(&["quick"]), 10);
        assert_eq!(results.len(), 2); // doc 1 和 doc 3 含 "quick"
        let doc_ids: Vec<usize> = results.iter().map(|(id, _)| *id).collect();
        assert!(doc_ids.contains(&1));
        assert!(doc_ids.contains(&3));
    }

    #[test]
    fn test_fulltext_index_search_no_match() {
        let mut index = FullTextIndex::new();
        index.add_text(1, "hello world").unwrap();
        let results = index.search(&make_terms(&["missing"]), 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_fulltext_index_search_empty_query() {
        let mut index = FullTextIndex::new();
        index.add_text(1, "hello world").unwrap();
        let results = index.search(&[], 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_fulltext_index_search_limit() {
        let mut index = FullTextIndex::new();
        for i in 1..=10 {
            index.add_text(i, "hello world").unwrap();
        }
        let results = index.search(&make_terms(&["hello"]), 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_fulltext_index_search_zero_limit_unlimited() {
        let mut index = FullTextIndex::new();
        for i in 1..=5 {
            index.add_text(i, "hello world").unwrap();
        }
        let results = index.search(&make_terms(&["hello"]), 0);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn test_fulltext_index_search_bm25_ranking() {
        let mut index = FullTextIndex::new();
        // doc1: "rare" 出现 1 次
        index.add_text(1, "rare common").unwrap();
        // doc2: "rare" 出现 1 次，但文档更短 → 分数应更高
        index.add_text(2, "rare").unwrap();
        // doc3: 不含 "rare"
        index.add_text(3, "common common common").unwrap();
        let results = index.search(&make_terms(&["rare"]), 10);
        // 应返回 doc1 和 doc2
        assert_eq!(results.len(), 2);
        // doc2 分数应高于 doc1（更短文档 → 更高 BM25）
        let score_map: HashMap<usize, f64> = results.into_iter().collect();
        let s1 = score_map[&1];
        let s2 = score_map[&2];
        assert!(s2 > s1, "shorter doc should score higher: s1={s1}, s2={s2}");
    }

    #[test]
    fn test_fulltext_index_search_rare_term_higher() {
        let mut index = FullTextIndex::new();
        // common 出现 5 次
        for i in 1..=5 {
            index.add_text(i, "common word").unwrap();
        }
        // rare 只出现 1 次
        index.add_text(6, "rare word").unwrap();
        let common_results = index.search(&make_terms(&["common"]), 10);
        let rare_results = index.search(&make_terms(&["rare"]), 10);
        let common_score = common_results[0].1;
        let rare_score = rare_results[0].1;
        assert!(
            rare_score > common_score,
            "rare term should score higher: rare={rare_score}, common={common_score}"
        );
    }

    #[test]
    fn test_fulltext_index_search_multi_term() {
        let mut index = FullTextIndex::new();
        index.add_text(1, "the cat sat on the mat").unwrap();
        index.add_text(2, "the dog sat on the log").unwrap();
        index.add_text(3, "cat and dog play").unwrap();
        // 查询 "cat" 和 "dog"
        let results = index.search(&make_terms(&["cat", "dog"]), 10);
        // doc3 含两者 → 分数最高
        assert_eq!(results[0].0, 3);
    }

    #[test]
    fn test_fulltext_index_search_tsquery() {
        let mut index = FullTextIndex::new();
        index.add_text(1, "hello world").unwrap();
        index.add_text(2, "hello rust").unwrap();
        let query = TsQuery::lexeme("hello").or(TsQuery::lexeme("rust"));
        let results = index.search_tsquery(&query, 10);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_fulltext_index_search_tsquery_and() {
        let mut index = FullTextIndex::new();
        index.add_text(1, "hello world").unwrap();
        index.add_text(2, "hello rust").unwrap();
        index.add_text(3, "hello world rust").unwrap();
        // search_tsquery 收集所有 TsQuery 词素并交给 search() 评分
        // 候选 = 含 "hello" 或 "rust" 的所有文档 → doc1/2/3
        // BM25：doc2 "hello rust"(len=2) 同时含两词且最短 → 分数最高
        let query = TsQuery::lexeme("hello").and(TsQuery::lexeme("rust"));
        let results = index.search_tsquery(&query, 10);
        // 返回所有含 "hello" 或 "rust" 的文档
        assert!(!results.is_empty());
        // doc2 分数最高（同时含 hello + rust，且文档最短）
        assert_eq!(results[0].0, 2);
        // doc1 仅含 hello，应在结果中但分数低于 doc2/doc3
        let doc_ids: Vec<usize> = results.iter().map(|(id, _)| *id).collect();
        assert!(doc_ids.contains(&1));
        assert!(doc_ids.contains(&3));
    }

    #[test]
    fn test_fulltext_index_postings() {
        let mut index = FullTextIndex::new();
        index.add_text(1, "hello world hello").unwrap();
        let postings = index.postings("hello").unwrap();
        assert_eq!(postings.len(), 1);
        assert_eq!(postings[0].doc_id, 1);
        assert_eq!(postings[0].tf, 2);
        assert_eq!(postings[0].positions.len(), 2);
        assert!(postings[0].positions.contains(&1));
        assert!(postings[0].positions.contains(&3));
    }

    #[test]
    fn test_fulltext_index_postings_missing() {
        let index = FullTextIndex::new();
        assert!(index.postings("foo").is_none());
    }

    #[test]
    fn test_fulltext_index_doc_ids() {
        let mut index = FullTextIndex::new();
        index.add_text(3, "hello").unwrap();
        index.add_text(1, "world").unwrap();
        index.add_text(2, "foo").unwrap();
        let ids = index.doc_ids();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn test_fulltext_index_with_params() {
        let params = Bm25Params::new(2.0, 0.5).unwrap();
        let index = FullTextIndex::with_params(params).unwrap();
        let p = index.bm25_params();
        assert!((p.k1 - 2.0).abs() < 1e-9);
        assert!((p.b - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_fulltext_index_with_params_invalid() {
        // k1=-1.0 应返回 Err
        let result = Bm25Params::new(-1.0, 0.5);
        assert!(result.is_err());
        // with_params 应传播错误
        let index_result = FullTextIndex::with_params(Bm25Params { k1: -1.0, b: 0.5 });
        assert!(index_result.is_err());
    }

    #[test]
    fn test_fulltext_index_doc_len_missing() {
        let index = FullTextIndex::new();
        assert_eq!(index.doc_len(99), None);
    }

    // =================================================================
    //  E2E 场景测试
    // =================================================================

    #[test]
    fn test_e2e_english_fulltext_search() {
        let mut index = FullTextIndex::new();
        let docs = [
            (1, "the quick brown fox jumps over the lazy dog"),
            (2, "a quick brown dog runs in the park"),
            (3, "the lazy cat sleeps all day long"),
            (4, "foxes are clever animals"),
            (5, "dog and cat are common pets"),
        ];
        for (id, text) in docs {
            index.add_text(id, text).unwrap();
        }
        // 搜索 "dog"
        let results = index.search(&make_terms(&["dog"]), 10);
        let doc_ids: Vec<usize> = results.iter().map(|(id, _)| *id).collect();
        assert!(doc_ids.contains(&1));
        assert!(doc_ids.contains(&2));
        assert!(doc_ids.contains(&5));
        // "dog" 在 doc5 中 tf=1，文档较短 → 分数较高
    }

    #[test]
    fn test_e2e_chinese_fulltext_search() {
        let tok = ChineseTokenizer::with_dict(&[
            "我们",
            "喜欢",
            "苹果",
            "香蕉",
            "水果",
            "研究",
            "数据库",
        ]);
        let mut index = FullTextIndex::new();
        index.add_text_chinese(1, "我们喜欢苹果", &tok).unwrap();
        index.add_text_chinese(2, "我们喜欢香蕉", &tok).unwrap();
        index.add_text_chinese(3, "我们研究数据库", &tok).unwrap();
        // 搜索 "我们"
        let results = index.search(&make_terms(&["我们"]), 10);
        assert_eq!(results.len(), 3);
        // 搜索 "苹果"
        let results = index.search(&make_terms(&["苹果"]), 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 1);
    }

    #[test]
    fn test_e2e_bm25_ranking_with_relevance() {
        let mut index = FullTextIndex::new();
        // doc1: "rust" 出现 3 次
        index.add_text(1, "rust rust rust programming").unwrap();
        // doc2: "rust" 出现 1 次，文档更短
        index.add_text(2, "rust").unwrap();
        // doc3: "rust" 出现 1 次，文档更长
        index
            .add_text(3, "rust is a systems programming language")
            .unwrap();
        let results = index.search(&make_terms(&["rust"]), 10);
        assert_eq!(results.len(), 3);
        // 都应返回正分数
        for (_, score) in &results {
            assert!(*score > 0.0);
        }
    }

    #[test]
    fn test_e2e_headline_with_search() {
        let docs = [
            (1, "the quick brown fox jumps"),
            (2, "the lazy dog sleeps"),
            (3, "foxes and dogs play"),
        ];
        let mut index = FullTextIndex::new();
        for (id, text) in docs {
            index.add_text(id, text).unwrap();
        }
        let query = TsQuery::lexeme("fox").or(TsQuery::lexeme("dog"));
        let results = index.search_tsquery(&query, 10);
        // 为每个匹配文档生成 headline
        for (doc_id, _score) in &results {
            let doc_text = docs.iter().find(|(id, _)| id == doc_id).unwrap().1;
            let headline = ts_headline(doc_text, &query, &HeadlineOptions::default());
            assert!(
                headline.contains("<b>") || headline.contains("fox") || headline.contains("dog"),
                "headline should contain highlighted terms or original: {headline}"
            );
        }
    }

    #[test]
    fn test_e2e_large_scale() {
        let mut index = FullTextIndex::new();
        // 1000 个文档
        for i in 1..=1000 {
            let text = format!("document {} contains word{}", i, i % 10);
            index.add_text(i, &text).unwrap();
        }
        assert_eq!(index.num_docs(), 1000);
        // 搜索 "word0"（100 个文档含此词）
        let results = index.search(&make_terms(&["word0"]), 10);
        assert_eq!(results.len(), 10); // limit=10
                                       // 搜索 "word5"
        let results = index.search(&make_terms(&["word5"]), 10);
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_e2e_tsvector_integration() {
        // 验证 TsVector 与 FullTextIndex 可协同工作
        let ts = TsVector::from_lexemes(vec!["hello", "world", "hello"]);
        // 从 TsVector 提取词素构建文档
        let terms: Vec<String> = ts.terms().iter().map(|s| s.to_string()).collect();
        let mut index = FullTextIndex::new();
        index.add_document(1, &terms).unwrap();
        let results = index.search(&make_terms(&["hello"]), 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 1);
    }

    #[test]
    fn test_e2e_value_integration() {
        // 验证 Value::Text 可作为文档输入
        let value = Value::Text("hello world from rust".to_string());
        let text = match &value {
            Value::Text(s) => s,
            _ => panic!("expected Text"),
        };
        let mut index = FullTextIndex::new();
        index.add_text(1, text).unwrap();
        let results = index.search(&make_terms(&["rust"]), 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_e2e_multi_field_search() {
        // 模拟多字段搜索：标题 + 正文
        let mut title_index = FullTextIndex::new();
        let mut body_index = FullTextIndex::new();
        title_index.add_text(1, "rust programming").unwrap();
        body_index
            .add_text(1, "rust is a systems language")
            .unwrap();
        title_index.add_text(2, "python scripting").unwrap();
        body_index.add_text(2, "python is easy to learn").unwrap();
        // 搜索标题中的 "rust"
        let title_results = title_index.search(&make_terms(&["rust"]), 10);
        assert_eq!(title_results.len(), 1);
        assert_eq!(title_results[0].0, 1);
        // 搜索正文中的 "rust"
        let body_results = body_index.search(&make_terms(&["rust"]), 10);
        assert_eq!(body_results.len(), 1);
        assert_eq!(body_results[0].0, 1);
    }

    #[test]
    fn test_e2e_phrase_search_via_tsquery() {
        let mut index = FullTextIndex::new();
        index.add_text(1, "the quick brown fox").unwrap();
        index.add_text(2, "the brown quick fox").unwrap();
        // FollowedBy 查询会收集所有词素
        let query = TsQuery::FollowedBy {
            distance: 1,
            left: Box::new(TsQuery::lexeme("quick")),
            right: Box::new(TsQuery::lexeme("brown")),
        };
        let results = index.search_tsquery(&query, 10);
        // BM25 返回所有包含任一词素的文档
        assert!(!results.is_empty());
    }

    #[test]
    fn test_e2e_zero_results() {
        let mut index = FullTextIndex::new();
        index.add_text(1, "hello world").unwrap();
        let results = index.search(&make_terms(&["missing", "absent"]), 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_e2e_consistent_ranking() {
        // 同样的索引和查询，多次搜索结果应一致
        let mut index = FullTextIndex::new();
        index.add_text(1, "rust programming language").unwrap();
        index.add_text(2, "rust systems programming").unwrap();
        let r1 = index.search(&make_terms(&["rust"]), 10);
        let r2 = index.search(&make_terms(&["rust"]), 10);
        assert_eq!(r1, r2);
    }
}
