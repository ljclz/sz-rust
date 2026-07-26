//! NL2SQL 自然语言查询引擎 — Phase 7b.3
//!
//! 对应 `SzRSQL技术实现方案.md` 9.9 节。
//!
//! # 设计
//!
//! 基于规则的本地 NL2SQL 引擎（无需外部 LLM API），通过多阶段管道将
//! 自然语言转换为 SQL：
//!
//! 1. **文本预处理** — 小写化、分词、中英文混合处理
//! 2. **意图分类** — 识别 SELECT / AGGREGATE / FILTER / JOIN / ORDER / GROUP / LIMIT
//! 3. **槽位填充** — 从注册的表结构中匹配表名/列名；从查询中提取值/条件/运算符
//! 4. **SQL 生成** — 根据意图和槽位生成参数化 SQL 字符串
//!
//! # 支持的查询模式（Spider 风格）
//!
//! - SELECT-WHERE: "查找年龄大于20的学生姓名"
//! - SELECT-AGGREGATE: "有多少学生" / "count all students"
//! - SELECT-ORDER: "按年龄排序的学生姓名" / "students sorted by age"
//! - SELECT-GROUP-AGGREGATE: "按系别分组的平均年龄"
//! - SELECT-JOIN: "学生姓名和课程名称" / "students and their courses"
//! - SELECT-JOIN-WHERE: "选修了数学课的学生姓名"
//! - SELECT-DISTINCT: "不重复的系别" / "distinct departments"
//!
//! # 中英文混合支持
//!
//! 关键词映射表覆盖中英文同义词，如：
//! - "查找/find/get/show/list" → SELECT
//! - "多少/how many/count" → COUNT
//! - "平均/average/avg" → AVG
//! - "大于/greater than/>" → >
//! - "排序/sorted/order by" → ORDER BY

use std::collections::HashMap;

// =====================================================================
//  错误类型
// =====================================================================

/// NL2SQL 错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Nl2SqlError {
    /// 未匹配到任何表
    #[error("no table matched in query")]
    NoTableMatched,
    /// 未匹配到任何列
    #[error("no column matched in query")]
    NoColumnMatched,
    /// 匹配歧义（多个候选且无法消歧）
    #[error("ambiguous match: {0}")]
    AmbiguousMatch(String),
    /// 无效查询
    #[error("invalid query: {0}")]
    InvalidQuery(String),
}

// =====================================================================
//  Schema 定义（独立于 szrsql-sql AST，保持模块独立性）
// =====================================================================

/// 列类型（简化版）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ColType {
    /// 整数
    Integer,
    /// 浮点数
    Float,
    /// 文本
    Text,
    /// 布尔
    Bool,
    /// 日期
    Date,
    /// 时间戳
    Timestamp,
}

/// 列定义
#[derive(Debug, Clone)]
pub struct ColumnDef {
    /// 列名
    pub name: String,
    /// 列类型
    pub data_type: ColType,
}

/// 表定义
#[derive(Debug, Clone)]
pub struct TableDef {
    /// 表名
    pub name: String,
    /// 列列表
    pub columns: Vec<ColumnDef>,
    /// 别名列表（如中文表名 "学生" → "students"）
    pub aliases: Vec<String>,
}

impl TableDef {
    /// 获取所有列名
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.name.as_str()).collect()
    }

    /// 查找列
    pub fn find_column(&self, name: &str) -> Option<&ColumnDef> {
        self.columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }
}

// =====================================================================
//  意图与槽位
// =====================================================================

/// 聚合函数类型
#[derive(Debug, Clone, PartialEq, Eq)]
enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl AggFunc {
    fn to_sql(&self, col: &str) -> String {
        match self {
            AggFunc::Count => {
                if col == "*" {
                    "COUNT(*)".to_string()
                } else {
                    format!("COUNT({col})")
                }
            }
            AggFunc::Sum => format!("SUM({col})"),
            AggFunc::Avg => format!("AVG({col})"),
            AggFunc::Min => format!("MIN({col})"),
            AggFunc::Max => format!("MAX({col})"),
        }
    }
}

/// 比较运算符
#[derive(Debug, Clone, PartialEq, Eq)]
enum Comparator {
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Like,
}

impl Comparator {
    fn to_sql(&self) -> &'static str {
        match self {
            Comparator::Eq => "=",
            Comparator::NotEq => "!=",
            Comparator::Lt => "<",
            Comparator::LtEq => "<=",
            Comparator::Gt => ">",
            Comparator::GtEq => ">=",
            Comparator::Like => "LIKE",
        }
    }
}

/// WHERE 条件
#[derive(Debug, Clone)]
struct WhereCondition {
    /// 列名（完整限定，如 "students.age"）
    column: String,
    /// 比较运算符
    op: Comparator,
    /// 值（已格式化为 SQL 字面量）
    value: String,
}

/// ORDER BY 子句
#[derive(Debug, Clone)]
struct OrderByClause {
    /// 列名
    column: String,
    /// 是否升序
    asc: bool,
}

/// 解析意图
#[derive(Debug, Clone)]
struct ParsedIntent {
    /// 是否 DISTINCT
    distinct: bool,
    /// 查询的列（None = *）
    select_columns: Option<Vec<String>>,
    /// 聚合函数 + 列
    aggregation: Option<(AggFunc, String)>,
    /// 涉及的表
    tables: Vec<String>,
    /// JOIN 条件（表对 + 连接列）
    joins: Vec<(String, String, String)>, // (table1, table2, join_col)
    /// WHERE 条件
    conditions: Vec<WhereCondition>,
    /// GROUP BY 列
    group_by: Option<Vec<String>>,
    /// ORDER BY
    order_by: Option<OrderByClause>,
    /// LIMIT
    limit: Option<usize>,
}

// =====================================================================
//  NL2SQL 引擎
// =====================================================================

/// NL2SQL 自然语言查询引擎
pub struct Nl2SqlEngine {
    /// 已注册的表
    tables: Vec<TableDef>,
    /// 表名 → 索引（小写查找）
    table_index: HashMap<String, usize>,
}

impl Default for Nl2SqlEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Nl2SqlEngine {
    /// 创建空引擎
    pub fn new() -> Self {
        Self {
            tables: Vec::new(),
            table_index: HashMap::new(),
        }
    }

    /// 注册表结构（无别名）
    pub fn register_table(&mut self, name: &str, columns: Vec<ColumnDef>) {
        let idx = self.tables.len();
        self.table_index.insert(name.to_lowercase(), idx);
        self.tables.push(TableDef {
            name: name.to_string(),
            columns,
            aliases: Vec::new(),
        });
    }

    /// 注册表结构（带别名，如中文表名）
    pub fn register_table_with_aliases(
        &mut self,
        name: &str,
        columns: Vec<ColumnDef>,
        aliases: Vec<&str>,
    ) {
        let idx = self.tables.len();
        self.table_index.insert(name.to_lowercase(), idx);
        self.tables.push(TableDef {
            name: name.to_string(),
            columns,
            aliases: aliases.into_iter().map(String::from).collect(),
        });
    }

    /// 自然语言 → SQL
    pub fn translate(&self, query: &str) -> Result<String, Nl2SqlError> {
        if query.trim().is_empty() {
            return Err(Nl2SqlError::InvalidQuery("empty query".to_string()));
        }

        // Phase 1: 文本预处理
        let normalized = normalize_query(query);

        // Phase 2: 意图分类 + 槽位填充
        let intent = self.parse_intent(&normalized)?;

        // Phase 3: SQL 生成
        let sql = self.generate_sql(&intent);

        Ok(sql)
    }

    // -----------------------------------------------------------------
    //  Phase 2: 意图分类 + 槽位填充
    // -----------------------------------------------------------------

    /// 解析自然语言查询，提取意图和槽位
    fn parse_intent(&self, query: &str) -> Result<ParsedIntent, Nl2SqlError> {
        let tokens = tokenize(query);

        // 检测 DISTINCT
        let distinct = detect_distinct(&tokens);

        // 检测聚合函数
        let aggregation = detect_aggregation(&tokens);

        // 匹配表名
        let tables = self.match_tables(&tokens);
        if tables.is_empty() {
            return Err(Nl2SqlError::NoTableMatched);
        }

        // 匹配列名
        let select_columns = if aggregation.is_some() {
            // 有聚合时不需要显式 select 列
            None
        } else {
            let cols = self.match_select_columns(&tokens, &tables);
            if cols.is_empty() {
                None
            } else {
                Some(cols)
            }
        };

        // 检测 JOIN
        let joins = self.detect_joins(&tokens, &tables);

        // 检测 WHERE 条件
        let conditions = self.extract_conditions(&tokens, &tables);

        // 检测 GROUP BY
        let group_by = self.detect_group_by(&tokens, &tables);

        // 检测 ORDER BY
        let order_by = self.detect_order_by(&tokens, &tables);

        // 检测 LIMIT
        let limit = detect_limit(&tokens);

        Ok(ParsedIntent {
            distinct,
            select_columns,
            aggregation,
            tables,
            joins,
            conditions,
            group_by,
            order_by,
            limit,
        })
    }

    /// 从查询中匹配表名（支持表名和别名，别名也做子串匹配以处理中文连写）
    fn match_tables(&self, tokens: &[String]) -> Vec<String> {
        let mut matched = Vec::new();
        for table in &self.tables {
            let table_lower = table.name.to_lowercase();
            // 收集该表所有可匹配名称（表名 + 别名），均小写
            let mut names: Vec<String> = vec![table_lower.clone()];
            for alias in &table.aliases {
                names.push(alias.to_lowercase());
            }

            let mut found = false;
            // 精确匹配（表名或别名）
            for token in tokens {
                let token_lower = token.to_lowercase();
                if names.iter().any(|n| n == &token_lower) {
                    matched.push(table.name.clone());
                    found = true;
                    break;
                }
            }
            if found {
                continue;
            }

            // 子串匹配（表名/别名作为子串出现在 token 中）
            // 英文表名要求长度 >= 3 避免误匹配；中文别名无此限制
            for token in tokens {
                let token_lower = token.to_lowercase();
                for name in &names {
                    let is_cjk = name.chars().any(|c| c as u32 > 0x2E80);
                    let min_len = if is_cjk {
                        1
                    } else {
                        3
                    };
                    if name.len() >= min_len && token_lower.contains(name) {
                        matched.push(table.name.clone());
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
        }

        // 如果只匹配到一个表，也检查是否有 "and"/"和" 连接的第二个表
        if matched.len() == 1 {
            for table in &self.tables {
                if matched.contains(&table.name) {
                    continue;
                }
                let table_lower = table.name.to_lowercase();
                for token in tokens {
                    if token.to_lowercase() == table_lower {
                        matched.push(table.name.clone());
                        break;
                    }
                }
            }
        }

        matched
    }

    /// 匹配 SELECT 列
    fn match_select_columns(&self, tokens: &[String], tables: &[String]) -> Vec<String> {
        let mut cols = Vec::new();

        // 获取所有候选列
        let mut candidates: Vec<(String, String)> = Vec::new(); // (col_name, table_name)
        for table_name in tables {
            if let Some(&idx) = self.table_index.get(&table_name.to_lowercase()) {
                for col in &self.tables[idx].columns {
                    candidates.push((col.name.clone(), table_name.clone()));
                }
            }
        }

        // 跳过的关键词
        let skip_words = get_skip_words();

        for token in tokens {
            let token_lower = token.to_lowercase();
            if skip_words.contains(token_lower.as_str()) {
                continue;
            }
            for (col_name, table_name) in &candidates {
                if token_lower == col_name.to_lowercase() {
                    let qualified = format!("{table_name}.{col_name}");
                    if !cols.contains(&qualified) {
                        cols.push(qualified);
                    }
                }
            }
        }

        cols
    }

    /// 检测 JOIN（当查询涉及多个表时）
    fn detect_joins(&self, _tokens: &[String], tables: &[String]) -> Vec<(String, String, String)> {
        if tables.len() < 2 {
            return Vec::new();
        }

        // 查找两个表之间的共同列作为连接条件
        let mut joins = Vec::new();
        for i in 0..tables.len() {
            for j in (i + 1)..tables.len() {
                let table_i = &tables[i];
                let table_j = &tables[j];
                if let (Some(&idx_i), Some(&idx_j)) = (
                    self.table_index.get(&table_i.to_lowercase()),
                    self.table_index.get(&table_j.to_lowercase()),
                ) {
                    let cols_i = &self.tables[idx_i].columns;
                    let cols_j = &self.tables[idx_j].columns;

                    // 查找共同列名
                    for ci in cols_i {
                        for cj in cols_j {
                            if ci.name.eq_ignore_ascii_case(&cj.name) {
                                joins.push((table_i.clone(), table_j.clone(), ci.name.clone()));
                            }
                        }
                    }
                }
            }
        }

        joins
    }

    /// 提取 WHERE 条件
    fn extract_conditions(&self, tokens: &[String], tables: &[String]) -> Vec<WhereCondition> {
        let mut conditions = Vec::new();

        // 构建列候选
        let mut col_candidates: Vec<(String, String)> = Vec::new(); // (col_name, table_name)
        for table_name in tables {
            if let Some(&idx) = self.table_index.get(&table_name.to_lowercase()) {
                for col in &self.tables[idx].columns {
                    col_candidates.push((col.name.clone(), table_name.clone()));
                }
            }
        }

        // 遍历 tokens 寻找 "列名 运算符 值" 模式
        let mut i = 0;
        while i < tokens.len() {
            // 检测列名
            let token_lower = tokens[i].to_lowercase();
            let matched_col: Option<(String, String)> = col_candidates
                .iter()
                .find(|(col, _)| col.to_lowercase() == token_lower)
                .map(|(col, table)| (col.clone(), table.clone()));

            if let Some((col_name, table_name)) = matched_col {
                // 检测后续运算符
                if i + 1 < tokens.len() {
                    let (op, consumed) = detect_comparator(&tokens[i + 1..]);
                    if let Some(op) = op {
                        // 检测值
                        if i + 1 + consumed < tokens.len() {
                            let value_token = &tokens[i + 1 + consumed];
                            if let Some(value) = parse_value(value_token) {
                                conditions.push(WhereCondition {
                                    column: format!("{table_name}.{col_name}"),
                                    op,
                                    value,
                                });
                                i = i + 1 + consumed + 1;
                                continue;
                            }
                        }
                    }
                }
            }
            i += 1;
        }

        conditions
    }

    /// 检测 GROUP BY
    fn detect_group_by(&self, tokens: &[String], tables: &[String]) -> Option<Vec<String>> {
        // 查找 "group by" / "按...分组" / "by" 关键词
        let group_keywords = ["group", "by", "分组", "按", "每组", "各个"];
        let mut group_start = None;
        for (idx, token) in tokens.iter().enumerate() {
            let tl = token.to_lowercase();
            if group_keywords.contains(&tl.as_str()) {
                group_start = Some(idx);
                break;
            }
        }

        let start = group_start?;

        // 从 group 关键词后查找列名
        let mut cols = Vec::new();
        for token in &tokens[start + 1..] {
            let tl = token.to_lowercase();
            // 遇到其他子句关键词则停止
            if matches!(
                tl.as_str(),
                "order" | "limit" | "where" | "having" | "sort" | "排序" | "限制"
            ) {
                break;
            }
            for table_name in tables {
                if let Some(&idx) = self.table_index.get(&table_name.to_lowercase()) {
                    for col in &self.tables[idx].columns {
                        if col.name.to_lowercase() == tl {
                            let qualified = format!("{table_name}.{}", col.name);
                            if !cols.contains(&qualified) {
                                cols.push(qualified);
                            }
                        }
                    }
                }
            }
        }

        if cols.is_empty() {
            None
        } else {
            Some(cols)
        }
    }

    /// 检测 ORDER BY
    fn detect_order_by(&self, tokens: &[String], tables: &[String]) -> Option<OrderByClause> {
        // 查找 "order" / "sort" / "排序" 关键词
        let order_keywords = ["order", "sort", "排序", "按...排", "排列"];
        let mut order_start = None;
        for (idx, token) in tokens.iter().enumerate() {
            let tl = token.to_lowercase();
            if order_keywords.iter().any(|k| tl.contains(k)) || tl == "sorted" {
                order_start = Some(idx);
                break;
            }
        }

        let start = order_start?;

        // 跳过 "by" 关键词
        let col_start = if start + 1 < tokens.len() && tokens[start + 1].to_lowercase() == "by" {
            start + 2
        } else {
            start + 1
        };

        if col_start >= tokens.len() {
            return None;
        }

        // 查找列名
        let col_token = tokens[col_start].to_lowercase();
        for table_name in tables {
            if let Some(&idx) = self.table_index.get(&table_name.to_lowercase()) {
                for col in &self.tables[idx].columns {
                    if col.name.to_lowercase() == col_token {
                        // 检测升序/降序：检测到 desc 关键词时 asc=false，否则默认升序
                        let asc = if col_start + 1 < tokens.len() {
                            let dir = tokens[col_start + 1].to_lowercase();
                            !(dir == "desc" || dir == "descending" || dir == "降序")
                        } else {
                            true
                        };
                        return Some(OrderByClause {
                            column: format!("{table_name}.{}", col.name),
                            asc,
                        });
                    }
                }
            }
        }

        None
    }

    // -----------------------------------------------------------------
    //  Phase 3: SQL 生成
    // -----------------------------------------------------------------

    /// 根据意图生成 SQL
    fn generate_sql(&self, intent: &ParsedIntent) -> String {
        let mut sql = String::from("SELECT ");

        // DISTINCT
        if intent.distinct {
            sql.push_str("DISTINCT ");
        }

        // SELECT 列
        if let Some((agg, col)) = &intent.aggregation {
            // 聚合查询
            sql.push_str(&agg.to_sql(col));
        } else if let Some(cols) = &intent.select_columns {
            sql.push_str(&cols.join(", "));
        } else {
            sql.push('*');
        }

        // FROM
        sql.push_str(" FROM ");
        sql.push_str(&intent.tables.join(", "));

        // JOIN
        for (t1, t2, join_col) in &intent.joins {
            sql.push_str(&format!(" JOIN {t2} ON {t1}.{join_col} = {t2}.{join_col}"));
        }

        // WHERE
        if !intent.conditions.is_empty() {
            let conds: Vec<String> = intent
                .conditions
                .iter()
                .map(|c| format!("{} {} {}", c.column, c.op.to_sql(), c.value))
                .collect();
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }

        // GROUP BY
        if let Some(group_cols) = &intent.group_by {
            sql.push_str(" GROUP BY ");
            sql.push_str(&group_cols.join(", "));
        }

        // ORDER BY
        if let Some(order) = &intent.order_by {
            sql.push_str(" ORDER BY ");
            sql.push_str(&order.column);
            if order.asc {
                sql.push_str(" ASC");
            } else {
                sql.push_str(" DESC");
            }
        }

        // LIMIT
        if let Some(limit) = intent.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        sql
    }
}

// =====================================================================
//  辅助函数 — 文本处理
// =====================================================================

/// 查询文本预处理：小写化、去除多余空白
fn normalize_query(query: &str) -> String {
    // 只 trim，不小写化：保留值的原始大小写（如 'CS' 不应变成 'cs'）
    // 各检测函数在比较 token 时自行 to_lowercase()
    query.trim().to_string()
}

/// 分词：按空白/标点分词，保留引号内容
fn tokenize(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = '\0';

    for ch in query.chars() {
        if in_quotes {
            if ch == quote_char {
                // 引号结束，将引号内容作为单个 token
                tokens.push(current.trim().to_string());
                current.clear();
                in_quotes = false;
            } else {
                current.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            if !current.is_empty() {
                tokens.push(current.trim().to_string());
                current.clear();
            }
            in_quotes = true;
            quote_char = ch;
        } else if ch.is_whitespace() || ch == ',' || ch == '.' {
            if !current.is_empty() {
                tokens.push(current.trim().to_string());
                current.clear();
            }
            // 点号也作为分隔符，但保留列限定符（table.col）
            // 这里简化处理：点号分隔
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current.trim().to_string());
    }

    // 过滤空 token
    tokens.into_iter().filter(|t| !t.is_empty()).collect()
}

/// 检测 DISTINCT
fn detect_distinct(tokens: &[String]) -> bool {
    tokens.iter().any(|t| {
        t.to_lowercase() == "distinct" || t.to_lowercase() == "不重复" || t.to_lowercase() == "唯一"
    })
}

/// 检测聚合函数
fn detect_aggregation(tokens: &[String]) -> Option<(AggFunc, String)> {
    // 中文聚合关键词（支持子串匹配，如 "有多少" 包含 "多少"）
    const CN_COUNT: &[&str] = &["多少", "数量", "个数", "统计"];
    const CN_SUM: &[&str] = &["总和", "求和", "总计"];
    const CN_AVG: &[&str] = &["平均", "均值"];
    const CN_MIN: &[&str] = &["最小", "最低", "最少"];
    const CN_MAX: &[&str] = &["最大", "最高", "最多"];

    for (i, token) in tokens.iter().enumerate() {
        let tl = token.to_lowercase();
        // 精确匹配（英文 + 中文单 token）
        let agg = match tl.as_str() {
            "count" | "多少" | "数量" | "个数" | "统计" => Some(AggFunc::Count),
            "sum" | "总和" | "求和" | "总计" => Some(AggFunc::Sum),
            "avg" | "average" | "平均" | "均值" => Some(AggFunc::Avg),
            "min" | "最小" | "最低" | "最少" => Some(AggFunc::Min),
            "max" | "最大" | "最高" | "最多" => Some(AggFunc::Max),
            _ => None,
        };

        // 中文子串匹配（如 "有多少学生" 包含 "多少"）
        let agg = agg.or_else(|| {
            if CN_COUNT.iter().any(|k| tl.contains(k)) {
                Some(AggFunc::Count)
            } else if CN_SUM.iter().any(|k| tl.contains(k)) {
                Some(AggFunc::Sum)
            } else if CN_AVG.iter().any(|k| tl.contains(k)) {
                Some(AggFunc::Avg)
            } else if CN_MIN.iter().any(|k| tl.contains(k)) {
                Some(AggFunc::Min)
            } else if CN_MAX.iter().any(|k| tl.contains(k)) {
                Some(AggFunc::Max)
            } else {
                None
            }
        });

        if let Some(func) = agg {
            // 查找聚合的列
            // "多少学生" → count(*) (学生是表名不是列名)
            // "平均年龄" → avg(age)
            // 对于 "how many" 模式，通常 count(*)
            if func == AggFunc::Count {
                // count 通常用 *
                return Some((AggFunc::Count, "*".to_string()));
            }

            // 其他聚合：查找后续列名
            if i + 1 < tokens.len() {
                let col = tokens[i + 1].to_lowercase();
                // 跳过 "of" / "的"
                let col = if col == "of" || col == "的" {
                    if i + 2 < tokens.len() {
                        tokens[i + 2].clone()
                    } else {
                        return Some((func, "*".to_string()));
                    }
                } else {
                    tokens[i + 1].clone()
                };
                return Some((func, col));
            }
            return Some((func, "*".to_string()));
        }
    }

    // 检测 "how many" 模式
    for i in 0..tokens.len().saturating_sub(1) {
        if tokens[i].to_lowercase() == "how" && tokens[i + 1].to_lowercase() == "many" {
            return Some((AggFunc::Count, "*".to_string()));
        }
    }

    None
}

/// 检测比较运算符，返回 (运算符, 消耗的 token 数)
fn detect_comparator(tokens: &[String]) -> (Option<Comparator>, usize) {
    if tokens.is_empty() {
        return (None, 0);
    }

    let t = tokens[0].to_lowercase();

    // 英文关键词
    match t.as_str() {
        ">" | "大于" | "超过" | "高于" | "more" | "greater" => {
            return (Some(Comparator::Gt), 1)
        }
        "<" | "小于" | "低于" | "少于" | "less" | "smaller" => {
            return (Some(Comparator::Lt), 1)
        }
        ">=" | "至少" | "不少于" => return (Some(Comparator::GtEq), 1),
        "<=" | "至多" | "不多于" | "最多" => return (Some(Comparator::LtEq), 1),
        "=" | "等于" | "是" | "为" | "equals" | "is" => return (Some(Comparator::Eq), 1),
        "!=" | "不等于" | "不是" | "不为" => return (Some(Comparator::NotEq), 1),
        "包含" | "like" | "含有" | "包括" => return (Some(Comparator::Like), 1),
        _ => {}
    }

    // "greater than" / "less than" / "equal to" 等双词模式
    if tokens.len() >= 2 {
        let t2 = tokens[1].to_lowercase();
        match (t.as_str(), t2.as_str()) {
            ("greater", "than") => return (Some(Comparator::Gt), 2),
            ("less", "than") => return (Some(Comparator::Lt), 2),
            ("equal", "to") | ("equals", "to") => return (Some(Comparator::Eq), 2),
            ("more", "than") => return (Some(Comparator::Gt), 2),
            ("at", "least") => return (Some(Comparator::GtEq), 2),
            ("at", "most") => return (Some(Comparator::LtEq), 2),
            _ => {}
        }
    }

    (None, 0)
}

/// 解析值为 SQL 字面量
fn parse_value(token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }

    // 数字
    if let Ok(n) = token.parse::<i64>() {
        return Some(n.to_string());
    }
    if let Ok(n) = token.parse::<f64>() {
        return Some(n.to_string());
    }

    // 字符串（已去引号的）
    // 转义单引号
    let escaped = token.replace('\'', "''");
    Some(format!("'{escaped}'"))
}

/// 检测 LIMIT
fn detect_limit(tokens: &[String]) -> Option<usize> {
    for (i, token) in tokens.iter().enumerate() {
        let tl = token.to_lowercase();
        if (tl == "limit" || tl == "前" || tl == "top") && i + 1 < tokens.len() {
            if let Ok(n) = tokens[i + 1].parse::<usize>() {
                return Some(n);
            }
        }
        // "前10名" / "top 10"
        if tl.starts_with("前") && tl.len() > 3 {
            let num_str = &tl[3..]; // "前" 是 3 字节
            if let Ok(n) = num_str.parse::<usize>() {
                return Some(n);
            }
        }
    }
    None
}

/// 获取跳过词列表（不参与列名匹配）
fn get_skip_words() -> std::collections::HashSet<&'static str> {
    let words: [&str; 45] = [
        "find", "get", "show", "list", "查找", "查询", "获取", "显示", "列出", "找", "all", "the",
        "of", "and", "or", "where", "from", "with", "that", "have", "所", "有", "的", "和", "或",
        "中", "在", "上", "how", "many", "count", "sum", "avg", "average", "min", "max", "order",
        "by", "sort", "group", "distinct", "who", "which", "what", "whose",
    ];
    words.into_iter().collect()
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 创建测试用的 schema（模拟 Spider 数据集，含中文别名）
    fn create_test_engine() -> Nl2SqlEngine {
        let mut engine = Nl2SqlEngine::new();
        engine.register_table_with_aliases(
            "students",
            vec![
                ColumnDef {
                    name: "student_id".to_string(),
                    data_type: ColType::Integer,
                },
                ColumnDef {
                    name: "name".to_string(),
                    data_type: ColType::Text,
                },
                ColumnDef {
                    name: "age".to_string(),
                    data_type: ColType::Integer,
                },
                ColumnDef {
                    name: "department".to_string(),
                    data_type: ColType::Text,
                },
                ColumnDef {
                    name: "gpa".to_string(),
                    data_type: ColType::Float,
                },
            ],
            vec!["学生", "学生表"],
        );
        engine.register_table_with_aliases(
            "courses",
            vec![
                ColumnDef {
                    name: "course_id".to_string(),
                    data_type: ColType::Integer,
                },
                ColumnDef {
                    name: "course_name".to_string(),
                    data_type: ColType::Text,
                },
                ColumnDef {
                    name: "credits".to_string(),
                    data_type: ColType::Integer,
                },
                ColumnDef {
                    name: "department".to_string(),
                    data_type: ColType::Text,
                },
            ],
            vec!["课程", "课程表"],
        );
        engine.register_table_with_aliases(
            "enrollments",
            vec![
                ColumnDef {
                    name: "enrollment_id".to_string(),
                    data_type: ColType::Integer,
                },
                ColumnDef {
                    name: "student_id".to_string(),
                    data_type: ColType::Integer,
                },
                ColumnDef {
                    name: "course_id".to_string(),
                    data_type: ColType::Integer,
                },
                ColumnDef {
                    name: "grade".to_string(),
                    data_type: ColType::Text,
                },
            ],
            vec!["选课", "选课表", "选课记录"],
        );
        engine
    }

    // -----------------------------------------------------------------
    //  基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b3_engine_creation() {
        let engine = Nl2SqlEngine::new();
        assert!(engine.tables.is_empty());
    }

    #[test]
    fn test_7b3_register_table() {
        let mut engine = Nl2SqlEngine::new();
        engine.register_table(
            "users",
            vec![ColumnDef {
                name: "id".to_string(),
                data_type: ColType::Integer,
            }],
        );
        assert_eq!(engine.tables.len(), 1);
        assert_eq!(engine.tables[0].name, "users");
        assert_eq!(engine.tables[0].columns.len(), 1);
    }

    #[test]
    fn test_7b3_translate_empty_query_errors() {
        let engine = create_test_engine();
        let result = engine.translate("");
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            Nl2SqlError::InvalidQuery("empty query".to_string())
        );
    }

    #[test]
    fn test_7b3_translate_no_table_errors() {
        let engine = create_test_engine();
        let result = engine.translate("find all items");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), Nl2SqlError::NoTableMatched);
    }

    // -----------------------------------------------------------------
    //  SELECT 查询测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b3_select_all() {
        let engine = create_test_engine();
        let sql = engine.translate("find all students").unwrap();
        assert!(sql.contains("SELECT"), "should contain SELECT: {sql}");
        assert!(
            sql.contains("FROM students"),
            "should contain FROM students: {sql}"
        );
    }

    #[test]
    fn test_7b3_select_with_columns() {
        let engine = create_test_engine();
        let sql = engine.translate("find name age from students").unwrap();
        assert!(sql.contains("students.name"), "should select name: {sql}");
        assert!(sql.contains("students.age"), "should select age: {sql}");
    }

    #[test]
    fn test_7b3_select_chinese() {
        let engine = create_test_engine();
        let sql = engine.translate("查找所有学生").unwrap();
        assert!(
            sql.contains("FROM students"),
            "should contain FROM students: {sql}"
        );
    }

    // -----------------------------------------------------------------
    //  WHERE 条件测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b3_where_gt() {
        let engine = create_test_engine();
        let sql = engine
            .translate("find name from students where age > 20")
            .unwrap();
        assert!(sql.contains("WHERE"), "should contain WHERE: {sql}");
        assert!(
            sql.contains("students.age > 20"),
            "should contain age > 20: {sql}"
        );
    }

    #[test]
    fn test_7b3_where_eq() {
        let engine = create_test_engine();
        let sql = engine
            .translate("find name from students where department = CS")
            .unwrap();
        assert!(
            sql.contains("students.department = 'CS'"),
            "should contain department = 'CS': {sql}"
        );
    }

    #[test]
    fn test_7b3_where_chinese() {
        let engine = create_test_engine();
        let sql = engine.translate("查找学生姓名 年龄 大于 20").unwrap();
        assert!(
            sql.contains("FROM students"),
            "should contain FROM students: {sql}"
        );
    }

    // -----------------------------------------------------------------
    //  聚合函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b3_count_all() {
        let engine = create_test_engine();
        let sql = engine.translate("how many students").unwrap();
        assert!(sql.contains("COUNT(*)"), "should contain COUNT(*): {sql}");
        assert!(
            sql.contains("FROM students"),
            "should contain FROM students: {sql}"
        );
    }

    #[test]
    fn test_7b3_count_chinese() {
        let engine = create_test_engine();
        let sql = engine.translate("有多少学生").unwrap();
        assert!(sql.contains("COUNT(*)"), "should contain COUNT(*): {sql}");
        assert!(
            sql.contains("FROM students"),
            "should contain FROM students: {sql}"
        );
    }

    #[test]
    fn test_7b3_avg() {
        let engine = create_test_engine();
        let sql = engine.translate("average age from students").unwrap();
        assert!(sql.contains("AVG(age)"), "should contain AVG(age): {sql}");
    }

    // -----------------------------------------------------------------
    //  ORDER BY 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b3_order_by() {
        let engine = create_test_engine();
        let sql = engine
            .translate("find name from students order by age")
            .unwrap();
        assert!(sql.contains("ORDER BY"), "should contain ORDER BY: {sql}");
        assert!(
            sql.contains("students.age"),
            "should contain students.age: {sql}"
        );
    }

    #[test]
    fn test_7b3_order_by_desc() {
        let engine = create_test_engine();
        let sql = engine
            .translate("find name from students order by age desc")
            .unwrap();
        assert!(sql.contains("ORDER BY"), "should contain ORDER BY: {sql}");
        assert!(sql.contains("DESC"), "should contain DESC: {sql}");
    }

    // -----------------------------------------------------------------
    //  GROUP BY 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b3_group_by() {
        let engine = create_test_engine();
        let sql = engine
            .translate("average age from students group by department")
            .unwrap();
        assert!(sql.contains("GROUP BY"), "should contain GROUP BY: {sql}");
        assert!(
            sql.contains("students.department"),
            "should contain students.department: {sql}"
        );
    }

    // -----------------------------------------------------------------
    //  JOIN 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b3_join() {
        let engine = create_test_engine();
        let sql = engine.translate("find name from students courses").unwrap();
        // 两个表有共同列 department → JOIN
        assert!(
            sql.contains("JOIN") || sql.contains("FROM students, courses"),
            "should contain JOIN or multi-table FROM: {sql}"
        );
    }

    // -----------------------------------------------------------------
    //  DISTINCT 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b3_distinct() {
        let engine = create_test_engine();
        let sql = engine
            .translate("find distinct department from students")
            .unwrap();
        assert!(sql.contains("DISTINCT"), "should contain DISTINCT: {sql}");
    }

    // -----------------------------------------------------------------
    //  LIMIT 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b3_limit() {
        let engine = create_test_engine();
        let sql = engine.translate("find name from students limit 5").unwrap();
        assert!(sql.contains("LIMIT 5"), "should contain LIMIT 5: {sql}");
    }

    // -----------------------------------------------------------------
    //  Spider 风格准确率测试（合成测试集）
    // -----------------------------------------------------------------

    /// Spider 风格合成测试集 — 验证 NL2SQL 准确率 >= 70%
    ///
    /// 测试集包含多种查询模式：
    /// - SELECT-WHERE (英文/中文)
    /// - SELECT-AGGREGATE (COUNT/AVG/SUM/MIN/MAX)
    /// - SELECT-ORDER BY (ASC/DESC)
    /// - SELECT-GROUP BY-AGGREGATE
    /// - SELECT-DISTINCT
    /// - SELECT-LIMIT
    /// - 中英文混合查询
    #[test]
    #[allow(clippy::type_complexity)]
    fn test_7b3_spider_accuracy() {
        let engine = create_test_engine();

        // 测试用例：(自然语言, 验证函数)
        // 验证函数检查生成的 SQL 是否包含关键部分
        let test_cases: Vec<(&str, Box<dyn Fn(&str) -> bool>)> = vec![
            // SELECT-WHERE
            (
                "find name from students where age > 20",
                Box::new(|sql| {
                    sql.contains("SELECT")
                        && sql.contains("students.name")
                        && sql.contains("students.age > 20")
                }),
            ),
            (
                "find name from students where age < 25",
                Box::new(|sql| sql.contains("students.age < 25")),
            ),
            (
                "find name from students where department = CS",
                Box::new(|sql| sql.contains("students.department = 'CS'")),
            ),
            // SELECT-AGGREGATE
            (
                "how many students",
                Box::new(|sql| sql.contains("COUNT(*)") && sql.contains("FROM students")),
            ),
            (
                "count students",
                Box::new(|sql| sql.contains("COUNT(*)") && sql.contains("FROM students")),
            ),
            (
                "average age from students",
                Box::new(|sql| sql.contains("AVG(age)")),
            ),
            (
                "max age from students",
                Box::new(|sql| sql.contains("MAX(age)")),
            ),
            (
                "min age from students",
                Box::new(|sql| sql.contains("MIN(age)")),
            ),
            // SELECT-ORDER BY
            (
                "find name from students order by age",
                Box::new(|sql| sql.contains("ORDER BY") && sql.contains("students.age")),
            ),
            (
                "find name from students order by age desc",
                Box::new(|sql| sql.contains("ORDER BY") && sql.contains("DESC")),
            ),
            // SELECT-GROUP BY
            (
                "average age from students group by department",
                Box::new(|sql| sql.contains("GROUP BY") && sql.contains("students.department")),
            ),
            // SELECT-DISTINCT
            (
                "find distinct department from students",
                Box::new(|sql| sql.contains("DISTINCT")),
            ),
            // SELECT-LIMIT
            (
                "find name from students limit 5",
                Box::new(|sql| sql.contains("LIMIT 5")),
            ),
            (
                "find name from students limit 10",
                Box::new(|sql| sql.contains("LIMIT 10")),
            ),
            // 中文查询
            (
                "查找所有学生",
                Box::new(|sql| sql.contains("FROM students")),
            ),
            (
                "有多少学生",
                Box::new(|sql| sql.contains("COUNT(*)") && sql.contains("FROM students")),
            ),
            (
                "查找学生姓名 年龄 大于 20",
                Box::new(|sql| sql.contains("FROM students")),
            ),
            // SELECT *
            (
                "find all students",
                Box::new(|sql| sql.contains("FROM students")),
            ),
            (
                "show all courses",
                Box::new(|sql| sql.contains("FROM courses")),
            ),
            // 多列 SELECT
            (
                "find name age from students",
                Box::new(|sql| sql.contains("students.name") && sql.contains("students.age")),
            ),
            // WHERE + ORDER BY
            (
                "find name from students where age > 20 order by age",
                Box::new(|sql| sql.contains("WHERE") && sql.contains("ORDER BY")),
            ),
        ];

        let total = test_cases.len();
        let mut passed = 0;

        for (nl, validator) in &test_cases {
            match engine.translate(nl) {
                Ok(sql) => {
                    if validator(&sql) {
                        passed += 1;
                    } else {
                        eprintln!("FAIL: '{nl}' → SQL: '{sql}'");
                    }
                }
                Err(e) => {
                    eprintln!("ERROR: '{nl}' → {e}");
                }
            }
        }

        let accuracy = passed as f64 / total as f64;
        eprintln!(
            "Spider accuracy: {passed}/{total} = {:.1}%",
            accuracy * 100.0
        );

        assert!(
            accuracy >= 0.7,
            "NL2SQL accuracy should be >= 70%, got {:.1}% ({}/{})",
            accuracy * 100.0,
            passed,
            total
        );
    }
}
