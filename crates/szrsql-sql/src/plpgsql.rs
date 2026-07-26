//! PL/pgSQL 解析器 — Phase 6.5
//!
//! 提供对 PostgreSQL PL/pgSQL 函数体的词法分析与语法解析。
//!
//! # 设计
//!
//! - **入口**：`parse_function_body(src: &str) -> Result<PlPgSqlBlock, PlPgSqlParseError>`
//! - **词法分析**：手写 tokenizer，生成 `PlPgSqlToken` 流；支持标识符、字符串字面量、
//!   数字字面量、运算符（`:=` / `..` / `||` / `<>` 等）、注释（`--` / `/* */`）、
//!   标签（`<<label>>`）
//! - **语法分析**：手写递归下降 parser，覆盖 PL/pgSQL 核心语句
//! - **表达式**：Phase 6.5 以 `String`（原始文本）存储表达式，Phase 6.6 解释器再委托
//!   SQL 执行器求值
//!
//! # 覆盖语句
//!
//! 变量声明 / 赋值 / IF / CASE / LOOP / WHILE / FOR / EXIT / CONTINUE /
//! RETURN / RETURN NEXT / RETURN QUERY / PERFORM / EXECUTE / RAISE / NULL /
//! 嵌套 BLOCK / EXCEPTION 处理 / SELECT INTO / GOTO
//!
//! # 与 Phase 6.6 的衔接
//!
//! Phase 6.5 仅做解析，不执行。Phase 6.6 将实现 `PlPgSqlInterpreter`，遍历 AST 并
//! 委托 SQL 执行器求值表达式与 SQL 语句。

use std::fmt;

// =====================================================================
//  错误类型
// =====================================================================

/// PL/pgSQL 解析错误
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlPgSqlParseError {
    /// 词法错误
    #[error("lex error at line {line}, col {col}: {msg}")]
    Lex {
        line: usize,
        col: usize,
        msg: String,
    },
    /// 语法错误
    #[error("parse error at line {line}, col {col}: expected {expected}, got {got}")]
    Parse {
        line: usize,
        col: usize,
        expected: String,
        got: String,
    },
    /// 不支持的语法
    #[error("unsupported syntax at line {line}, col {col}: {msg}")]
    Unsupported {
        line: usize,
        col: usize,
        msg: String,
    },
    /// 未期望的 EOF
    #[error("unexpected EOF at line {line}, col {col}: {msg}")]
    UnexpectedEof {
        line: usize,
        col: usize,
        msg: String,
    },
}

// =====================================================================
//  AST — PL/pgSQL 语句
// =====================================================================

/// PL/pgSQL 函数体（最外层块或嵌套块）
///
/// 结构：`[<<label>>] [DECLARE ...] BEGIN ... [EXCEPTION ...] END [label];`
#[derive(Debug, Clone, PartialEq)]
pub struct PlPgSqlBlock {
    /// 块标签（`<<label>>`）
    pub label: Option<String>,
    /// DECLARE 段中的变量/常量/别名声明
    pub declarations: Vec<PlPgSqlDeclaration>,
    /// BEGIN 与 END/EXCEPTION 之间的语句序列
    pub statements: Vec<PlPgSqlStatement>,
    /// EXCEPTION 处理器列表
    pub exception_handlers: Vec<PlPgSqlExceptionHandler>,
}

/// PL/pgSQL 声明（DECLARE 段）
#[derive(Debug, Clone, PartialEq)]
pub enum PlPgSqlDeclaration {
    /// `name [CONSTANT] type [NOT NULL] [:= expr | DEFAULT expr];`
    Variable {
        name: String,
        is_constant: bool,
        data_type: String,
        not_null: bool,
        default: Option<String>,
    },
    /// `name type%TYPE [NOT NULL] [:= expr];`
    /// `name table.column%TYPE [NOT NULL] [:= expr];`
    VariableTypeRef {
        name: String,
        is_constant: bool,
        type_ref: PlPgSqlTypeRef,
        not_null: bool,
        default: Option<String>,
    },
    /// `name ALIAS FOR $n;` 或 `name ALIAS FOR var;`
    Alias { name: String, target: String },
}

/// PL/pgSQL 类型引用（%TYPE / %ROWTYPE）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlPgSqlTypeRef {
    /// `table.column%TYPE`
    ColumnType { table: String, column: String },
    /// `var%TYPE`
    VariableType { var: String },
    /// `table%ROWTYPE`
    RowType { table: String },
}

/// PL/pgSQL 语句
#[derive(Debug, Clone, PartialEq)]
pub enum PlPgSqlStatement {
    /// `var := expr;` 或 `var := expr;`
    Assignment { target: String, value: String },
    /// `SELECT ... INTO target [, ...] FROM ...;`
    SelectInto { targets: Vec<String>, query: String },
    /// `PERFORM query;`
    Perform { query: String },
    /// `EXECUTE expr [INTO target [, ...]] [USING expr [, ...]];`
    Execute {
        query: String,
        into: Vec<String>,
        using: Vec<String>,
    },
    /// `RETURN [expr];`
    Return { value: Option<String> },
    /// `RETURN NEXT expr;`
    ReturnNext { value: String },
    /// `RETURN QUERY query;`
    ReturnQuery { query: String },
    /// `IF cond THEN ... [ELSIF cond THEN ...] ... [ELSE ...] END IF;`
    If {
        branches: Vec<PlPgSqlIfBranch>,
        else_branch: Option<Vec<PlPgSqlStatement>>,
    },
    /// `[CASE [selector]] WHEN cond THEN ... ... [ELSE ...] END CASE;`
    Case {
        selector: Option<String>,
        branches: Vec<PlPgSqlCaseBranch>,
        else_branch: Option<Vec<PlPgSqlStatement>>,
    },
    /// `[<<label>>] LOOP ... END LOOP [label];`
    Loop {
        label: Option<String>,
        body: Vec<PlPgSqlStatement>,
    },
    /// `[<<label>>] WHILE cond LOOP ... END LOOP [label];`
    While {
        label: Option<String>,
        cond: String,
        body: Vec<PlPgSqlStatement>,
    },
    /// `[<<label>>] FOR var IN [REVERSE] lower .. upper [BY step] LOOP ... END LOOP [label];`
    For {
        label: Option<String>,
        var: String,
        reverse: bool,
        lower: String,
        upper: String,
        step: Option<String>,
        body: Vec<PlPgSqlStatement>,
    },
    /// `[<<label>>] FOR var IN query LOOP ... END LOOP [label];`
    ForQuery {
        label: Option<String>,
        var: String,
        query: String,
        body: Vec<PlPgSqlStatement>,
    },
    /// `[<<label>>] FOREACH var [IN ARRAY] [SLICE n] LOOP ... END LOOP [label];`
    ForEach {
        label: Option<String>,
        var: String,
        slice: Option<u32>,
        body: Vec<PlPgSqlStatement>,
    },
    /// `EXIT [label] [WHEN cond];`
    Exit {
        label: Option<String>,
        cond: Option<String>,
    },
    /// `CONTINUE [label] [WHEN cond];`
    Continue {
        label: Option<String>,
        cond: Option<String>,
    },
    /// 嵌套块 `[<<label>>] [DECLARE ...] BEGIN ... [EXCEPTION ...] END [label];`
    Block(PlPgSqlBlock),
    /// `RAISE [level] [format] [, args ...] [USING option = expr [, ...]];`
    Raise {
        level: PlPgSqlRaiseLevel,
        format: Option<String>,
        args: Vec<String>,
        options: Vec<(String, String)>,
    },
    /// `NULL;`（空操作）
    Null,
    /// `GOTO label;`
    Goto { label: String },
    /// 无法归类的裸 SQL 语句（保留原始文本，Phase 6.6 委托 SQL 执行器执行）
    SqlStatement { sql: String },
}

/// IF 分支
#[derive(Debug, Clone, PartialEq)]
pub struct PlPgSqlIfBranch {
    pub cond: String,
    pub statements: Vec<PlPgSqlStatement>,
}

/// CASE 分支
#[derive(Debug, Clone, PartialEq)]
pub struct PlPgSqlCaseBranch {
    /// WHEN 子句表达式（selector 存在时为匹配值；不存在时为布尔条件）
    pub cond: String,
    pub statements: Vec<PlPgSqlStatement>,
}

/// RAISE 级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlPgSqlRaiseLevel {
    Debug,
    Log,
    Info,
    Notice,
    Warning,
    Exception,
}

impl fmt::Display for PlPgSqlRaiseLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlPgSqlRaiseLevel::Debug => write!(f, "DEBUG"),
            PlPgSqlRaiseLevel::Log => write!(f, "LOG"),
            PlPgSqlRaiseLevel::Info => write!(f, "INFO"),
            PlPgSqlRaiseLevel::Notice => write!(f, "NOTICE"),
            PlPgSqlRaiseLevel::Warning => write!(f, "WARNING"),
            PlPgSqlRaiseLevel::Exception => write!(f, "EXCEPTION"),
        }
    }
}

/// EXCEPTION 处理器
#[derive(Debug, Clone, PartialEq)]
pub struct PlPgSqlExceptionHandler {
    /// 异常条件列表（`WHEN cond1 OR cond2 THEN`），可为 `OTHERS`
    pub conditions: Vec<String>,
    /// 处理语句序列
    pub statements: Vec<PlPgSqlStatement>,
}

// =====================================================================
//  词法分析器（Tokenizer）
// =====================================================================

/// PL/pgSQL token
#[derive(Debug, Clone, PartialEq)]
pub enum PlPgSqlToken {
    /// 关键字（归一化为大写）
    Keyword(String),
    /// 标识符（原始大小写）
    Ident(String),
    /// 带引号的标识符（`"..."`，保留大小写）
    QuotedIdent(String),
    /// 字符串字面量（已反转义）
    String(String),
    /// 数字字面量
    Number(String),
    /// 标签（`<<name>>`）
    Label(String),
    /// `:=` 赋值运算符
    Assign,
    /// `..` 范围运算符
    DotDot,
    /// `||` 连接运算符
    Concat,
    /// `<>` 不等于
    NotEqualAlt,
    /// `!=` 不等于
    NotEqual,
    /// `<=` / `>=` / `<` / `>`
    LtEq,
    GtEq,
    Lt,
    Gt,
    /// `=` 等于
    Eq,
    /// `+` / `-` / `*` / `/` / `%`
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    /// `(` / `)` / `,` / `.` / `;` / `[` / `]`
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Semicolon,
    /// 其他运算符（如 `::`, `->`, `->>`, `#>`, `@>` 等，保留原始文本）
    OtherOp(String),
    /// 文件结束
    Eof,
}

/// Token 位置信息
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenPos {
    pub line: usize,
    pub col: usize,
    pub offset: usize,
}

/// 带位置的 token
#[derive(Debug, Clone, PartialEq)]
pub struct PositionedToken {
    pub token: PlPgSqlToken,
    pub pos: TokenPos,
}

/// PL/pgSQL 关键字表
const KEYWORDS: &[&str] = &[
    "BEGIN",
    "END",
    "DECLARE",
    "EXCEPTION",
    "IF",
    "THEN",
    "ELSIF",
    "ELSE",
    "CASE",
    "WHEN",
    "LOOP",
    "WHILE",
    "FOR",
    "IN",
    "REVERSE",
    "BY",
    "EXIT",
    "CONTINUE",
    "RETURN",
    "NEXT",
    "QUERY",
    "PERFORM",
    "EXECUTE",
    "INTO",
    "USING",
    "RAISE",
    "NULL",
    "GOTO",
    "AND",
    "OR",
    "NOT",
    "AS",
    "CONSTANT",
    "DEFAULT",
    "ALIAS",
    "TYPE",
    "ROWTYPE",
    "SLICE",
    "ARRAY",
    "FOREACH",
    "DEBUG",
    "LOG",
    "INFO",
    "NOTICE",
    "WARNING",
    "OTHERS",
    "SELECT",
    "FROM",
    "WHERE",
    "ORDER",
    "GROUP",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "DISTINCT",
    "ALL",
    "ANY",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "IS",
    "TRUE",
    "FALSE",
    "FOUND",
    "ROW_COUNT",
    "RESULT_OID",
    "SQLSTATE",
    "SQLERRM",
    "END_IF",
    "END_LOOP",
    "END_CASE",
    "END_FOREACH",
    "STRICT",
    "OBJECT",
    "RECORD",
    "TABLE",
];

/// 判断字符串是否为关键字
fn is_keyword(s: &str) -> bool {
    let upper = s.to_uppercase();
    KEYWORDS.iter().any(|kw| *kw == upper)
}

/// 词法分析器
pub struct Tokenizer<'a> {
    src: &'a str,
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Tokenizer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    /// 当前位置
    fn current_pos(&self) -> TokenPos {
        TokenPos {
            line: self.line,
            col: self.col,
            offset: self.pos,
        }
    }

    /// peek 下一个字符（不消费）
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// peek 第 n 个字符（不消费）
    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    /// 消费当前字符并前进
    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    /// 跳过空白与注释
    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.advance();
                }
                Some('-') if self.peek_at(1) == Some('-') => {
                    // 行注释
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    // 块注释（支持嵌套，PG 语义）
                    self.advance();
                    self.advance();
                    let mut depth = 1;
                    while depth > 0 {
                        match self.peek() {
                            None => break,
                            Some('/') if self.peek_at(1) == Some('*') => {
                                self.advance();
                                self.advance();
                                depth += 1;
                            }
                            Some('*') if self.peek_at(1) == Some('/') => {
                                self.advance();
                                self.advance();
                                depth -= 1;
                            }
                            Some(_) => {
                                self.advance();
                            }
                        }
                    }
                }
                _ => break,
            }
        }
    }

    /// 读取标识符或关键字
    fn read_ident_or_keyword(&mut self) -> PlPgSqlToken {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.advance();
            } else {
                break;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        if is_keyword(&text) {
            PlPgSqlToken::Keyword(text.to_uppercase())
        } else {
            PlPgSqlToken::Ident(text)
        }
    }

    /// 读取带引号的标识符 `"..."`（`""` 转义为 `"`）
    fn read_quoted_ident(&mut self) -> Result<PlPgSqlToken, PlPgSqlParseError> {
        self.advance(); // 消费开头的 "
        let mut result = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(PlPgSqlParseError::Lex {
                        line: self.line,
                        col: self.col,
                        msg: "unterminated quoted identifier".into(),
                    });
                }
                Some('"') if self.peek_at(1) == Some('"') => {
                    result.push('"');
                    self.advance();
                    self.advance();
                }
                Some('"') => {
                    self.advance();
                    break;
                }
                Some(c) => {
                    result.push(c);
                    self.advance();
                }
            }
        }
        Ok(PlPgSqlToken::QuotedIdent(result))
    }

    /// 读取字符串字面量 `'...'`（`''` 转义为 `'`）
    fn read_string(&mut self) -> Result<PlPgSqlToken, PlPgSqlParseError> {
        self.advance(); // 消费开头的 '
        let mut result = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(PlPgSqlParseError::Lex {
                        line: self.line,
                        col: self.col,
                        msg: "unterminated string literal".into(),
                    });
                }
                Some('\'') if self.peek_at(1) == Some('\'') => {
                    result.push('\'');
                    self.advance();
                    self.advance();
                }
                Some('\'') => {
                    self.advance();
                    break;
                }
                Some(c) => {
                    result.push(c);
                    self.advance();
                }
            }
        }
        Ok(PlPgSqlToken::String(result))
    }

    /// 读取数字字面量
    fn read_number(&mut self) -> PlPgSqlToken {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-' {
                // 注意：`.` 可能是范围运算符 `..` 的一部分，需特殊处理
                if c == '.' && self.peek_at(1) == Some('.') {
                    break; // 范围运算符，留给上层处理
                }
                // `e`/`E` 后跟 `+`/`-` 是科学计数法；其他位置的 `+`/`-` 不是数字部分
                if c == '+' || c == '-' {
                    let prev = self.chars.get(self.pos.wrapping_sub(1)).copied();
                    if !matches!(prev, Some('e') | Some('E')) {
                        break;
                    }
                }
                self.advance();
            } else {
                break;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        PlPgSqlToken::Number(text)
    }

    /// 读取位置参数 `$1`, `$2`, ... 返回 `Number` token（仅含数字部分，不含 `$`）
    ///
    /// 这样 `parse_alias_declaration` 中的 `PlPgSqlToken::Number(n)` 分支可直接匹配，
    /// 并通过 `format!("${}", n)` 还原完整形式。
    fn read_dollar_param(&mut self) -> PlPgSqlToken {
        self.advance(); // 消费 $
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        PlPgSqlToken::Number(text)
    }

    /// 读取标签 `<<name>>`
    fn read_label(&mut self) -> Result<PlPgSqlToken, PlPgSqlParseError> {
        self.advance(); // 消费第一个 <
        self.advance(); // 消费第二个 <
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.advance();
            } else {
                break;
            }
        }
        let name: String = self.chars[start..self.pos].iter().collect();
        if name.is_empty() {
            return Err(PlPgSqlParseError::Lex {
                line: self.line,
                col: self.col,
                msg: "empty label name".into(),
            });
        }
        // 期望 `>>`
        if self.peek() == Some('>') && self.peek_at(1) == Some('>') {
            self.advance();
            self.advance();
        } else {
            return Err(PlPgSqlParseError::Lex {
                line: self.line,
                col: self.col,
                msg: format!("expected '>>' to close label, got {:?}", self.peek()),
            });
        }
        Ok(PlPgSqlToken::Label(name))
    }

    /// 读取运算符
    fn read_operator(&mut self, first: char) -> Result<PlPgSqlToken, PlPgSqlParseError> {
        match first {
            ':' if self.peek_at(1) == Some('=') => {
                self.advance();
                self.advance();
                Ok(PlPgSqlToken::Assign)
            }
            ':' => {
                // `::` 类型转换运算符，或其他 `:` 开头的运算符
                self.advance();
                if self.peek() == Some(':') {
                    self.advance();
                    Ok(PlPgSqlToken::OtherOp("::".into()))
                } else {
                    Ok(PlPgSqlToken::OtherOp(":".into()))
                }
            }
            '.' if self.peek_at(1) == Some('.') => {
                self.advance();
                self.advance();
                Ok(PlPgSqlToken::DotDot)
            }
            '.' => {
                self.advance();
                Ok(PlPgSqlToken::Dot)
            }
            '|' if self.peek_at(1) == Some('|') => {
                self.advance();
                self.advance();
                Ok(PlPgSqlToken::Concat)
            }
            '<' if self.peek_at(1) == Some('>') => {
                self.advance();
                self.advance();
                Ok(PlPgSqlToken::NotEqualAlt)
            }
            '<' if self.peek_at(1) == Some('=') => {
                self.advance();
                self.advance();
                Ok(PlPgSqlToken::LtEq)
            }
            '<' => {
                self.advance();
                Ok(PlPgSqlToken::Lt)
            }
            '>' if self.peek_at(1) == Some('=') => {
                self.advance();
                self.advance();
                Ok(PlPgSqlToken::GtEq)
            }
            '>' => {
                self.advance();
                Ok(PlPgSqlToken::Gt)
            }
            '!' if self.peek_at(1) == Some('=') => {
                self.advance();
                self.advance();
                Ok(PlPgSqlToken::NotEqual)
            }
            '=' => {
                self.advance();
                Ok(PlPgSqlToken::Eq)
            }
            '+' => {
                self.advance();
                Ok(PlPgSqlToken::Plus)
            }
            '-' if self.peek_at(1) == Some('>') => {
                self.advance();
                self.advance();
                if self.peek() == Some('>') {
                    self.advance();
                    Ok(PlPgSqlToken::OtherOp("->>".into()))
                } else {
                    Ok(PlPgSqlToken::OtherOp("->".into()))
                }
            }
            '-' => {
                self.advance();
                Ok(PlPgSqlToken::Minus)
            }
            '*' => {
                self.advance();
                Ok(PlPgSqlToken::Star)
            }
            '/' => {
                self.advance();
                Ok(PlPgSqlToken::Slash)
            }
            '%' => {
                self.advance();
                Ok(PlPgSqlToken::Percent)
            }
            '(' => {
                self.advance();
                Ok(PlPgSqlToken::LParen)
            }
            ')' => {
                self.advance();
                Ok(PlPgSqlToken::RParen)
            }
            '[' => {
                self.advance();
                Ok(PlPgSqlToken::LBracket)
            }
            ']' => {
                self.advance();
                Ok(PlPgSqlToken::RBracket)
            }
            ',' => {
                self.advance();
                Ok(PlPgSqlToken::Comma)
            }
            ';' => {
                self.advance();
                Ok(PlPgSqlToken::Semicolon)
            }
            '#' if self.peek() == Some('>') => {
                self.advance();
                self.advance();
                Ok(PlPgSqlToken::OtherOp("#>".into()))
            }
            '#' => {
                self.advance();
                Ok(PlPgSqlToken::OtherOp("#".into()))
            }
            '@' if self.peek_at(1) == Some('>') => {
                self.advance();
                self.advance();
                Ok(PlPgSqlToken::OtherOp("@>".into()))
            }
            '&' => {
                self.advance();
                Ok(PlPgSqlToken::OtherOp("&".into()))
            }
            '|' => {
                self.advance();
                Ok(PlPgSqlToken::OtherOp("|".into()))
            }
            '~' => {
                self.advance();
                Ok(PlPgSqlToken::OtherOp("~".into()))
            }
            '^' => {
                self.advance();
                Ok(PlPgSqlToken::OtherOp("^".into()))
            }
            _ => Err(PlPgSqlParseError::Lex {
                line: self.line,
                col: self.col,
                msg: format!("unexpected character: {:?}", first),
            }),
        }
    }

    /// 获取下一个 token
    pub fn next_token(&mut self) -> Result<PositionedToken, PlPgSqlParseError> {
        self.skip_whitespace_and_comments();
        let pos = self.current_pos();

        let c = match self.peek() {
            None => {
                return Ok(PositionedToken {
                    token: PlPgSqlToken::Eof,
                    pos,
                });
            }
            Some(c) => c,
        };

        // 标签 `<<`
        if c == '<' && self.peek_at(1) == Some('<') {
            return Ok(PositionedToken {
                token: self.read_label()?,
                pos,
            });
        }

        // 标识符或关键字
        if c.is_ascii_alphabetic() || c == '_' {
            return Ok(PositionedToken {
                token: self.read_ident_or_keyword(),
                pos,
            });
        }

        // 带引号的标识符
        if c == '"' {
            return Ok(PositionedToken {
                token: self.read_quoted_ident()?,
                pos,
            });
        }

        // 字符串字面量
        if c == '\'' {
            return Ok(PositionedToken {
                token: self.read_string()?,
                pos,
            });
        }

        // 数字字面量
        if c.is_ascii_digit() {
            return Ok(PositionedToken {
                token: self.read_number(),
                pos,
            });
        }

        // 位置参数 `$1`, `$2`, ...
        if c == '$' && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            return Ok(PositionedToken {
                token: self.read_dollar_param(),
                pos,
            });
        }

        // 运算符与标点
        self.read_operator(c)
            .map(|token| PositionedToken { token, pos })
    }

    /// 分词为完整 token 列表
    pub fn tokenize(mut self) -> Result<Vec<PositionedToken>, PlPgSqlParseError> {
        let mut tokens = Vec::new();
        loop {
            let pt = self.next_token()?;
            let is_eof = pt.token == PlPgSqlToken::Eof;
            tokens.push(pt);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }
}

// =====================================================================
//  语法分析器（Recursive Descent Parser）
// =====================================================================

/// PL/pgSQL 递归下降 parser
pub struct PlPgSqlParser {
    tokens: Vec<PositionedToken>,
    idx: usize,
}

impl PlPgSqlParser {
    /// 从源码创建 parser
    pub fn from_src(src: &str) -> Result<Self, PlPgSqlParseError> {
        let tokens = Tokenizer::new(src).tokenize()?;
        Ok(Self { tokens, idx: 0 })
    }

    /// 从已有 token 列表创建 parser（用于测试）
    pub fn from_tokens(tokens: Vec<PositionedToken>) -> Self {
        Self { tokens, idx: 0 }
    }

    /// 当前 token（不消费）
    fn peek(&self) -> &PlPgSqlToken {
        &self.tokens[self.idx].token
    }

    /// 当前 token 的位置
    fn peek_pos(&self) -> &TokenPos {
        &self.tokens[self.idx].pos
    }

    /// 消费当前 token 并返回
    fn advance(&mut self) -> PlPgSqlToken {
        let token = self.tokens[self.idx].token.clone();
        if self.idx + 1 < self.tokens.len() {
            self.idx += 1;
        }
        token
    }

    /// 是否在 EOF
    fn is_eof(&self) -> bool {
        matches!(self.peek(), PlPgSqlToken::Eof)
    }

    /// 消费当前 token，检查是否为指定关键字
    fn expect_keyword(&mut self, kw: &str) -> Result<(), PlPgSqlParseError> {
        let pos = self.peek_pos().clone();
        match self.peek() {
            PlPgSqlToken::Keyword(k) if k == kw => {
                self.advance();
                Ok(())
            }
            other => Err(PlPgSqlParseError::Parse {
                line: pos.line,
                col: pos.col,
                expected: format!("keyword {}", kw),
                got: format!("{:?}", other),
            }),
        }
    }

    /// 消费当前 token，检查是否为指定 token 类型
    fn expect_token(&mut self, expected: &PlPgSqlToken) -> Result<(), PlPgSqlParseError> {
        let pos = self.peek_pos().clone();
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            Err(PlPgSqlParseError::Parse {
                line: pos.line,
                col: pos.col,
                expected: format!("{:?}", expected),
                got: format!("{:?}", self.peek()),
            })
        }
    }

    /// 当前是否为指定关键字
    fn check_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), PlPgSqlToken::Keyword(k) if k == kw)
    }

    /// 当前是否为任一指定关键字
    fn check_keywords(&self, kws: &[&str]) -> bool {
        matches!(self.peek(), PlPgSqlToken::Keyword(k) if kws.contains(&k.as_str()))
    }

    /// 如果当前是指定关键字则消费
    fn match_keyword(&mut self, kw: &str) -> bool {
        if self.check_keyword(kw) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// 如果当前是指定 token 则消费，返回是否匹配
    fn match_token(&mut self, expected: &PlPgSqlToken) -> bool {
        if self.peek() == expected {
            self.advance();
            true
        } else {
            false
        }
    }

    /// 读取标识符（返回名称）
    fn read_ident(&mut self) -> Result<String, PlPgSqlParseError> {
        let pos = self.peek_pos().clone();
        match self.peek().clone() {
            PlPgSqlToken::Ident(s) => {
                self.advance();
                Ok(s)
            }
            PlPgSqlToken::QuotedIdent(s) => {
                self.advance();
                Ok(s)
            }
            PlPgSqlToken::Keyword(k)
                if matches!(
                    k.as_str(),
                    "TYPE" | "ROWTYPE" | "SLICE" | "ARRAY" | "RECORD" | "TABLE" | "OBJECT"
                ) =>
            {
                // 某些关键字在特定上下文中可作为标识符（类型名等）
                self.advance();
                Ok(k.to_lowercase())
            }
            other => Err(PlPgSqlParseError::Parse {
                line: pos.line,
                col: pos.col,
                expected: "identifier".into(),
                got: format!("{:?}", other),
            }),
        }
    }

    /// 读取类型名（可能包含 `(`, `)`, `,`, 标识符, 数字等）
    fn read_type_name(&mut self) -> Result<String, PlPgSqlParseError> {
        let mut result = String::new();
        let mut paren_depth = 0;
        loop {
            match self.peek().clone() {
                PlPgSqlToken::Ident(s) => {
                    if !result.is_empty() {
                        result.push(' ');
                    }
                    result.push_str(&s);
                    self.advance();
                }
                PlPgSqlToken::Keyword(k)
                    if matches!(
                        k.as_str(),
                        "TYPE" | "ROWTYPE" | "RECORD" | "TABLE" | "OBJECT"
                    ) =>
                {
                    // 不消费 TYPE / ROWTYPE（由调用方判断 %TYPE / %ROWTYPE）
                    break;
                }
                PlPgSqlToken::Keyword(k)
                    if paren_depth > 0
                        && matches!(
                            k.as_str(),
                            "VARYING" | "WITH" | "WITHOUT" | "TIME" | "ZONE"
                        ) =>
                {
                    if !result.is_empty() {
                        result.push(' ');
                    }
                    result.push_str(&k);
                    self.advance();
                }
                PlPgSqlToken::Keyword(k) if result.is_empty() => {
                    // 第一个 token 可以是关键字充当类型名（如 `RECORD`）
                    if matches!(k.as_str(), "RECORD") {
                        result.push_str(&k);
                        self.advance();
                    } else {
                        break;
                    }
                }
                PlPgSqlToken::Number(n) => {
                    result.push_str(&n);
                    self.advance();
                }
                PlPgSqlToken::LParen => {
                    paren_depth += 1;
                    result.push('(');
                    self.advance();
                }
                PlPgSqlToken::RParen => {
                    if paren_depth == 0 {
                        break;
                    }
                    paren_depth -= 1;
                    result.push(')');
                    self.advance();
                }
                PlPgSqlToken::Comma if paren_depth > 0 => {
                    result.push_str(", ");
                    self.advance();
                }
                PlPgSqlToken::Dot => {
                    result.push('.');
                    self.advance();
                }
                _ => break,
            }
        }
        if result.is_empty() {
            let pos = self.peek_pos().clone();
            return Err(PlPgSqlParseError::Parse {
                line: pos.line,
                col: pos.col,
                expected: "type name".into(),
                got: format!("{:?}", self.peek()),
            });
        }
        Ok(result.trim().to_string())
    }

    /// 收集表达式文本直到遇到终止关键字、分号或（可选）顶层逗号
    ///
    /// `terminators` 是终止关键字列表（如 `THEN`, `LOOP`, `WHEN`, `BY`, `INTO`, `USING`）
    /// `stop_at_semicolon` 为 true 时遇到 `;` 也停止
    /// 顶层逗号（`paren_depth == 0`）总是作为表达式终止符（适用于参数列表）
    fn collect_expr(
        &mut self,
        terminators: &[&str],
        stop_at_semicolon: bool,
    ) -> Result<String, PlPgSqlParseError> {
        self.collect_expr_ext(terminators, stop_at_semicolon, false)
    }

    /// `collect_expr` 扩展版本：`stop_at_top_comma` 为 true 时顶层逗号也作为终止符
    fn collect_expr_ext(
        &mut self,
        terminators: &[&str],
        stop_at_semicolon: bool,
        stop_at_top_comma: bool,
    ) -> Result<String, PlPgSqlParseError> {
        let mut parts: Vec<String> = Vec::new();
        let mut paren_depth: i32 = 0;
        loop {
            match self.peek().clone() {
                PlPgSqlToken::Eof => {
                    let pos = self.peek_pos().clone();
                    return Err(PlPgSqlParseError::UnexpectedEof {
                        line: pos.line,
                        col: pos.col,
                        msg: "expected expression".into(),
                    });
                }
                PlPgSqlToken::Semicolon if stop_at_semicolon && paren_depth == 0 => {
                    break;
                }
                PlPgSqlToken::LParen => {
                    paren_depth += 1;
                    parts.push("(".into());
                    self.advance();
                }
                PlPgSqlToken::RParen => {
                    paren_depth -= 1;
                    parts.push(")".into());
                    self.advance();
                }
                PlPgSqlToken::LBracket => {
                    paren_depth += 1;
                    parts.push("[".into());
                    self.advance();
                }
                PlPgSqlToken::RBracket => {
                    paren_depth -= 1;
                    parts.push("]".into());
                    self.advance();
                }
                PlPgSqlToken::Keyword(k)
                    if paren_depth == 0 && terminators.contains(&k.as_str()) =>
                {
                    break;
                }
                PlPgSqlToken::Keyword(k) => {
                    parts.push(k);
                    self.advance();
                }
                PlPgSqlToken::Ident(s) => {
                    parts.push(s);
                    self.advance();
                }
                PlPgSqlToken::QuotedIdent(s) => {
                    parts.push(format!("\"{}\"", s));
                    self.advance();
                }
                PlPgSqlToken::String(s) => {
                    parts.push(format!("'{}'", s.replace('\'', "''")));
                    self.advance();
                }
                PlPgSqlToken::Number(n) => {
                    parts.push(n);
                    self.advance();
                }
                PlPgSqlToken::Assign => {
                    parts.push(":=".into());
                    self.advance();
                }
                PlPgSqlToken::DotDot => {
                    parts.push("..".into());
                    self.advance();
                }
                PlPgSqlToken::Concat => {
                    parts.push("||".into());
                    self.advance();
                }
                PlPgSqlToken::NotEqualAlt => {
                    parts.push("<>".into());
                    self.advance();
                }
                PlPgSqlToken::NotEqual => {
                    parts.push("!=".into());
                    self.advance();
                }
                PlPgSqlToken::LtEq => {
                    parts.push("<=".into());
                    self.advance();
                }
                PlPgSqlToken::GtEq => {
                    parts.push(">=".into());
                    self.advance();
                }
                PlPgSqlToken::Lt => {
                    parts.push("<".into());
                    self.advance();
                }
                PlPgSqlToken::Gt => {
                    parts.push(">".into());
                    self.advance();
                }
                PlPgSqlToken::Eq => {
                    parts.push("=".into());
                    self.advance();
                }
                PlPgSqlToken::Plus => {
                    parts.push("+".into());
                    self.advance();
                }
                PlPgSqlToken::Minus => {
                    parts.push("-".into());
                    self.advance();
                }
                PlPgSqlToken::Star => {
                    parts.push("*".into());
                    self.advance();
                }
                PlPgSqlToken::Slash => {
                    parts.push("/".into());
                    self.advance();
                }
                PlPgSqlToken::Percent => {
                    parts.push("%".into());
                    self.advance();
                }
                PlPgSqlToken::Comma if paren_depth == 0 && stop_at_top_comma => {
                    break;
                }
                PlPgSqlToken::Comma
                    if paren_depth == 0 && stop_at_semicolon && terminators.is_empty() =>
                {
                    break;
                }
                PlPgSqlToken::Comma => {
                    parts.push(",".into());
                    self.advance();
                }
                PlPgSqlToken::Dot => {
                    parts.push(".".into());
                    self.advance();
                }
                PlPgSqlToken::Semicolon => {
                    parts.push(";".into());
                    self.advance();
                }
                PlPgSqlToken::Label(s) => {
                    parts.push(format!("<<{}>>", s));
                    self.advance();
                }
                PlPgSqlToken::OtherOp(s) => {
                    parts.push(s);
                    self.advance();
                }
            }
        }
        let result = parts
            .iter()
            .enumerate()
            .fold(String::new(), |mut acc, (i, part)| {
                if i > 0 {
                    let prev = &parts[i - 1];
                    // 关键字（全大写字母）后跟 `(` 需要加空格，如 `IN (`、`BETWEEN (`
                    let prev_is_keyword =
                        !prev.is_empty() && prev.chars().all(|c| c.is_ascii_uppercase());
                    let part_is_paren = part.starts_with('(');
                    let needs_space = !prev.ends_with('(')
                        && !prev.ends_with('[')
                        && !prev.ends_with('.')
                        && !part.starts_with(')')
                        && !part.starts_with(']')
                        && !part.starts_with(',')
                        && !part.starts_with(';')
                        && !part.starts_with('.')
                        && !part.starts_with("::")
                        && prev != "("
                        && prev != "["
                        && part != ")"
                        && part != "]"
                        && part != ","
                        && part != ";"
                        && !prev.ends_with("::")
                        && !prev.ends_with("->")
                        && !prev.ends_with("->>");
                    if needs_space && (!part_is_paren || prev_is_keyword) && !part.starts_with('[')
                    {
                        acc.push(' ');
                    }
                }
                acc.push_str(part);
                acc
            });
        Ok(result.trim().to_string())
    }

    /// 解析块标签（如果存在）
    ///
    /// `<<label>>` 在块、循环之前出现
    fn parse_optional_label(&mut self) -> Option<String> {
        if let PlPgSqlToken::Label(name) = self.peek().clone() {
            self.advance();
            Some(name)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------
    //  块解析
    // -----------------------------------------------------------------

    /// 解析 PL/pgSQL 块：`[<<label>>] [DECLARE ...] BEGIN ... [EXCEPTION ...] END [label];`
    pub fn parse_block(&mut self) -> Result<PlPgSqlBlock, PlPgSqlParseError> {
        let label = self.parse_optional_label();

        // 可选 DECLARE
        let mut declarations = Vec::new();
        if self.match_keyword("DECLARE") {
            while !self.check_keyword("BEGIN") && !self.is_eof() {
                declarations.push(self.parse_declaration()?);
            }
        }

        // BEGIN
        self.expect_keyword("BEGIN")?;

        // 语句序列
        let mut statements = Vec::new();
        while !self.check_keyword("END") && !self.check_keyword("EXCEPTION") && !self.is_eof() {
            statements.push(self.parse_statement()?);
        }

        // 可选 EXCEPTION
        let mut exception_handlers = Vec::new();
        if self.match_keyword("EXCEPTION") {
            while !self.check_keyword("END") && !self.is_eof() {
                exception_handlers.push(self.parse_exception_handler()?);
            }
        }

        // END
        self.expect_keyword("END")?;

        // 可选标签名（END 后可跟标签名）
        if let PlPgSqlToken::Ident(_) | PlPgSqlToken::QuotedIdent(_) = self.peek() {
            // 消费可选标签名
            let _ = self.read_ident();
        }

        // 可选分号（顶层函数体最后的 END 可省略分号）
        self.match_token(&PlPgSqlToken::Semicolon);

        Ok(PlPgSqlBlock {
            label,
            declarations,
            statements,
            exception_handlers,
        })
    }

    /// 解析 EXCEPTION 处理器：`WHEN cond1 [OR cond2 ...] THEN statements ...`
    fn parse_exception_handler(&mut self) -> Result<PlPgSqlExceptionHandler, PlPgSqlParseError> {
        self.expect_keyword("WHEN")?;

        // 条件列表（以 OR 连接）
        let mut conditions = Vec::new();
        loop {
            let cond = self.parse_exception_condition()?;
            conditions.push(cond);
            if !self.match_keyword("OR") {
                break;
            }
        }

        self.expect_keyword("THEN")?;

        let mut statements = Vec::new();
        while !self.check_keyword("WHEN") && !self.check_keyword("END") && !self.is_eof() {
            statements.push(self.parse_statement()?);
        }

        Ok(PlPgSqlExceptionHandler {
            conditions,
            statements,
        })
    }

    /// 解析异常条件名
    fn parse_exception_condition(&mut self) -> Result<String, PlPgSqlParseError> {
        if self.check_keyword("OTHERS") {
            self.advance();
            return Ok("OTHERS".into());
        }
        // 异常条件名可能是标识符或 `SQLSTATE 'value'`
        if self.check_keyword("SQLSTATE") {
            self.advance();
            if let PlPgSqlToken::String(s) = self.peek().clone() {
                self.advance();
                return Ok(format!("SQLSTATE '{}'", s));
            }
        }
        // 普通异常名（如 `no_data_found`, `division_by_zero`）
        let name = self.read_ident()?;
        Ok(name)
    }

    // -----------------------------------------------------------------
    //  声明解析
    // -----------------------------------------------------------------

    /// 解析 DECLARE 段中的一条声明
    fn parse_declaration(&mut self) -> Result<PlPgSqlDeclaration, PlPgSqlParseError> {
        // 读取变量名（ALIAS 语法为 `name ALIAS FOR ...`，需先读 name 再判断）
        let name = self.read_ident()?;

        // ALIAS FOR
        if self.check_keyword("ALIAS") {
            return self.parse_alias_declaration(name);
        }

        let is_constant = self.match_keyword("CONSTANT");

        // 检测 %TYPE / %ROWTYPE
        if self.check_keyword("TYPE") || self.check_keyword("ROWTYPE") {
            // 这里不应到达 — %TYPE 由 read_type_name + 后续检查处理
        }

        // 先尝试读取类型名
        let data_type = self.read_type_name()?;

        // 检测 %TYPE / %ROWTYPE
        if self.peek() == &PlPgSqlToken::Percent {
            self.advance(); // 消费 %
            let type_kw = match self.peek().clone() {
                PlPgSqlToken::Keyword(k) if k == "TYPE" => {
                    self.advance();
                    PlPgSqlTypeRefKind::Type
                }
                PlPgSqlToken::Keyword(k) if k == "ROWTYPE" => {
                    self.advance();
                    PlPgSqlTypeRefKind::RowType
                }
                other => {
                    let pos = self.peek_pos().clone();
                    return Err(PlPgSqlParseError::Parse {
                        line: pos.line,
                        col: pos.col,
                        expected: "TYPE or ROWTYPE".into(),
                        got: format!("{:?}", other),
                    });
                }
            };
            let type_ref = self.build_type_ref(&data_type, type_kw)?;

            // NOT NULL
            let mut not_null = false;
            if self.match_keyword("NOT") {
                self.expect_keyword("NULL")?;
                not_null = true;
            }

            // 默认值
            let default = self.parse_optional_default()?;

            self.expect_token(&PlPgSqlToken::Semicolon)?;

            return Ok(PlPgSqlDeclaration::VariableTypeRef {
                name,
                is_constant,
                type_ref,
                not_null,
                default,
            });
        }

        // 普通 VARIABLE 声明
        // NOT NULL
        let mut not_null = false;
        if self.match_keyword("NOT") {
            self.expect_keyword("NULL")?;
            not_null = true;
        }

        // 默认值
        let default = self.parse_optional_default()?;

        self.expect_token(&PlPgSqlToken::Semicolon)?;

        Ok(PlPgSqlDeclaration::Variable {
            name,
            is_constant,
            data_type,
            not_null,
            default,
        })
    }

    /// 解析 ALIAS 声明（name 已读取）
    fn parse_alias_declaration(
        &mut self,
        name: String,
    ) -> Result<PlPgSqlDeclaration, PlPgSqlParseError> {
        self.expect_keyword("ALIAS")?;
        self.expect_keyword("FOR")?;
        // 目标可能是 `$1` 或变量名
        let target = match self.peek().clone() {
            PlPgSqlToken::Number(n) => {
                self.advance();
                format!("${}", n)
            }
            _ => self.read_ident()?,
        };
        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlDeclaration::Alias { name, target })
    }

    /// 构造类型引用
    fn build_type_ref(
        &self,
        data_type: &str,
        kind: PlPgSqlTypeRefKind,
    ) -> Result<PlPgSqlTypeRef, PlPgSqlParseError> {
        match kind {
            PlPgSqlTypeRefKind::Type => {
                // data_type 可能是 "table.column" 或 "var"
                if let Some(dot_pos) = data_type.find('.') {
                    let table = data_type[..dot_pos].trim().to_string();
                    let column = data_type[dot_pos + 1..].trim().to_string();
                    Ok(PlPgSqlTypeRef::ColumnType { table, column })
                } else {
                    Ok(PlPgSqlTypeRef::VariableType {
                        var: data_type.to_string(),
                    })
                }
            }
            PlPgSqlTypeRefKind::RowType => Ok(PlPgSqlTypeRef::RowType {
                table: data_type.trim().to_string(),
            }),
        }
    }

    /// 解析可选的默认值（`:= expr` 或 `DEFAULT expr`）
    fn parse_optional_default(&mut self) -> Result<Option<String>, PlPgSqlParseError> {
        if self.peek() == &PlPgSqlToken::Assign {
            self.advance();
            let expr = self.collect_expr(&[], true)?;
            Ok(Some(expr))
        } else if self.match_keyword("DEFAULT") {
            let expr = self.collect_expr(&[], true)?;
            Ok(Some(expr))
        } else {
            Ok(None)
        }
    }

    // -----------------------------------------------------------------
    //  语句解析
    // -----------------------------------------------------------------

    /// 解析单条 PL/pgSQL 语句
    pub fn parse_statement(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        // 先检查标签（块或循环的标签）
        let label = self.parse_optional_label();

        // 如果有标签，下一个 token 决定语句类型
        if label.is_some() {
            return self.parse_statement_with_label(label);
        }

        match self.peek().clone() {
            PlPgSqlToken::Keyword(k) => match k.as_str() {
                "IF" => self.parse_if(),
                "CASE" => self.parse_case(),
                "LOOP" => self.parse_loop(None),
                "WHILE" => self.parse_while(None),
                "FOR" => self.parse_for(None),
                "FOREACH" => self.parse_foreach(None),
                "EXIT" => self.parse_exit(),
                "CONTINUE" => self.parse_continue(),
                "RETURN" => self.parse_return(),
                "PERFORM" => self.parse_perform(),
                "EXECUTE" => self.parse_execute(),
                "RAISE" => self.parse_raise(),
                "NULL" => self.parse_null(),
                "GOTO" => self.parse_goto(),
                "BEGIN" => {
                    // 嵌套块
                    let block = self.parse_block()?;
                    Ok(PlPgSqlStatement::Block(block))
                }
                "DECLARE" => {
                    // 嵌套块带 DECLARE
                    let block = self.parse_block()?;
                    Ok(PlPgSqlStatement::Block(block))
                }
                "SELECT" => self.parse_select_or_assign(),
                _ => {
                    // 其他关键字开头的语句（如 INSERT/UPDATE/DELETE）视为裸 SQL
                    self.parse_sql_statement()
                }
            },
            PlPgSqlToken::Ident(_) | PlPgSqlToken::QuotedIdent(_) => {
                // 可能是赋值语句或裸 SQL
                self.parse_ident_statement()
            }
            _ => self.parse_sql_statement(),
        }
    }

    /// 解析带标签的语句
    fn parse_statement_with_label(
        &mut self,
        label: Option<String>,
    ) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        match self.peek().clone() {
            PlPgSqlToken::Keyword(k) => match k.as_str() {
                "LOOP" => self.parse_loop(label),
                "WHILE" => self.parse_while(label),
                "FOR" => self.parse_for(label),
                "FOREACH" => self.parse_foreach(label),
                "BEGIN" | "DECLARE" => {
                    let mut block = self.parse_block()?;
                    block.label = label.or(block.label);
                    Ok(PlPgSqlStatement::Block(block))
                }
                _ => {
                    let pos = self.peek_pos().clone();
                    Err(PlPgSqlParseError::Parse {
                        line: pos.line,
                        col: pos.col,
                        expected: "LOOP, WHILE, FOR, FOREACH, BEGIN, or DECLARE after label".into(),
                        got: format!("{:?}", self.peek()),
                    })
                }
            },
            _ => {
                let pos = self.peek_pos().clone();
                Err(PlPgSqlParseError::Parse {
                    line: pos.line,
                    col: pos.col,
                    expected: "keyword after label".into(),
                    got: format!("{:?}", self.peek()),
                })
            }
        }
    }

    /// 解析 IF 语句
    fn parse_if(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("IF")?;
        let cond = self.collect_expr(&["THEN"], false)?;
        self.expect_keyword("THEN")?;

        let mut statements = Vec::new();
        while !self.check_keyword("ELSIF")
            && !self.check_keyword("ELSE")
            && !self.check_keyword("END")
            && !self.is_eof()
        {
            statements.push(self.parse_statement()?);
        }

        let mut branches = vec![PlPgSqlIfBranch { cond, statements }];

        // ELSIF 分支
        while self.match_keyword("ELSIF") {
            let cond = self.collect_expr(&["THEN"], false)?;
            self.expect_keyword("THEN")?;
            let mut stmts = Vec::new();
            while !self.check_keyword("ELSIF")
                && !self.check_keyword("ELSE")
                && !self.check_keyword("END")
                && !self.is_eof()
            {
                stmts.push(self.parse_statement()?);
            }
            branches.push(PlPgSqlIfBranch {
                cond,
                statements: stmts,
            });
        }

        // ELSE 分支
        let mut else_branch = None;
        if self.match_keyword("ELSE") {
            let mut stmts = Vec::new();
            while !self.check_keyword("END") && !self.is_eof() {
                stmts.push(self.parse_statement()?);
            }
            else_branch = Some(stmts);
        }

        // END IF
        self.expect_keyword("END")?;
        self.expect_keyword("IF")?;
        self.expect_token(&PlPgSqlToken::Semicolon)?;

        Ok(PlPgSqlStatement::If {
            branches,
            else_branch,
        })
    }

    /// 解析 CASE 语句
    fn parse_case(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("CASE")?;

        // 可选 selector
        let selector = if self.check_keyword("WHEN") {
            None
        } else {
            let s = self.collect_expr(&["WHEN"], false)?;
            Some(s)
        };

        // WHEN 分支
        let mut branches = Vec::new();
        while self.match_keyword("WHEN") {
            let cond = self.collect_expr(&["THEN"], false)?;
            self.expect_keyword("THEN")?;
            let mut stmts = Vec::new();
            while !self.check_keyword("WHEN")
                && !self.check_keyword("ELSE")
                && !self.check_keyword("END")
                && !self.is_eof()
            {
                stmts.push(self.parse_statement()?);
            }
            branches.push(PlPgSqlCaseBranch {
                cond,
                statements: stmts,
            });
        }

        // ELSE
        let mut else_branch = None;
        if self.match_keyword("ELSE") {
            let mut stmts = Vec::new();
            while !self.check_keyword("END") && !self.is_eof() {
                stmts.push(self.parse_statement()?);
            }
            else_branch = Some(stmts);
        }

        // END CASE
        self.expect_keyword("END")?;
        self.expect_keyword("CASE")?;
        self.expect_token(&PlPgSqlToken::Semicolon)?;

        Ok(PlPgSqlStatement::Case {
            selector,
            branches,
            else_branch,
        })
    }

    /// 解析 LOOP 语句
    fn parse_loop(&mut self, label: Option<String>) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("LOOP")?;
        let mut body = Vec::new();
        while !self.check_keyword("END") && !self.is_eof() {
            body.push(self.parse_statement()?);
        }
        self.expect_keyword("END")?;
        self.expect_keyword("LOOP")?;
        // 可选标签
        if let PlPgSqlToken::Ident(_) | PlPgSqlToken::QuotedIdent(_) = self.peek() {
            let _ = self.read_ident();
        }
        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::Loop { label, body })
    }

    /// 解析 WHILE 语句
    fn parse_while(
        &mut self,
        label: Option<String>,
    ) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("WHILE")?;
        let cond = self.collect_expr(&["LOOP"], false)?;
        self.expect_keyword("LOOP")?;
        let mut body = Vec::new();
        while !self.check_keyword("END") && !self.is_eof() {
            body.push(self.parse_statement()?);
        }
        self.expect_keyword("END")?;
        self.expect_keyword("LOOP")?;
        if let PlPgSqlToken::Ident(_) | PlPgSqlToken::QuotedIdent(_) = self.peek() {
            let _ = self.read_ident();
        }
        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::While { label, cond, body })
    }

    /// 解析 FOR 语句
    fn parse_for(&mut self, label: Option<String>) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("FOR")?;
        let var = self.read_ident()?;

        // FOR var IN ...
        self.expect_keyword("IN")?;

        // 检测是否为 REVERSE
        let reverse = self.match_keyword("REVERSE");

        // 收集表达式直到 `..` 或 `LOOP`（BY 作为终止符，以便后续单独解析 step）
        // 先尝试读 lower 表达式，遇到 `..` 则为整数 FOR，遇到 `LOOP` 则为查询 FOR
        let first_expr = self.collect_expr(&["LOOP", "BY"], false)?;

        // 检查是否遇到 `..`（如果 collect_expr 在 LOOP 停止，但表达式末尾含 `..`，
        // 说明我们错过了。改用另一种策略：先检查是否为查询 FOR）
        // 实际上 `..` 在 collect_expr 中会被当作 DotDot 收集进表达式，
        // 所以需要用更精确的方法：先检查当前 token

        // 重新设计：在 collect_expr 之前检查
        // 但我们已经消费了 first_expr。需要重新思考。
        //
        // 让我改用另一种方法：在 collect_expr 中加入 `..` 作为终止符

        // 由于 first_expr 已收集（可能包含 `..`），我们需要判断它是否包含 `..`
        if first_expr.contains("..") {
            // 整数 FOR: lower .. upper [BY step]
            let parts: Vec<&str> = first_expr.splitn(2, "..").collect();
            let lower = parts[0].trim().to_string();
            let upper = if parts.len() > 1 {
                parts[1].trim().to_string()
            } else {
                String::new()
            };

            // BY step
            let step = if self.match_keyword("BY") {
                Some(self.collect_expr(&["LOOP"], false)?)
            } else {
                None
            };

            self.expect_keyword("LOOP")?;
            let mut body = Vec::new();
            while !self.check_keyword("END") && !self.is_eof() {
                body.push(self.parse_statement()?);
            }
            self.expect_keyword("END")?;
            self.expect_keyword("LOOP")?;
            if let PlPgSqlToken::Ident(_) | PlPgSqlToken::QuotedIdent(_) = self.peek() {
                let _ = self.read_ident();
            }
            self.expect_token(&PlPgSqlToken::Semicolon)?;

            return Ok(PlPgSqlStatement::For {
                label,
                var,
                reverse,
                lower,
                upper,
                step,
                body,
            });
        }

        // 查询 FOR: FOR var IN query LOOP
        // first_expr 是查询的开头部分
        // 继续收集查询的剩余部分
        let query = if self.check_keyword("LOOP") {
            first_expr
        } else {
            // 继续收集
            let rest = self.collect_expr(&["LOOP"], false)?;
            if rest.is_empty() {
                first_expr
            } else {
                format!("{} {}", first_expr, rest)
            }
        };

        self.expect_keyword("LOOP")?;
        let mut body = Vec::new();
        while !self.check_keyword("END") && !self.is_eof() {
            body.push(self.parse_statement()?);
        }
        self.expect_keyword("END")?;
        self.expect_keyword("LOOP")?;
        if let PlPgSqlToken::Ident(_) | PlPgSqlToken::QuotedIdent(_) = self.peek() {
            let _ = self.read_ident();
        }
        self.expect_token(&PlPgSqlToken::Semicolon)?;

        Ok(PlPgSqlStatement::ForQuery {
            label,
            var,
            query,
            body,
        })
    }

    /// 解析 FOREACH 语句
    fn parse_foreach(
        &mut self,
        label: Option<String>,
    ) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("FOREACH")?;
        let var = self.read_ident()?;

        // 可选 SLICE n
        let slice = if self.match_keyword("SLICE") {
            match self.peek().clone() {
                PlPgSqlToken::Number(n) => {
                    self.advance();
                    n.parse::<u32>().ok()
                }
                _ => None,
            }
        } else {
            None
        };

        // IN [ARRAY]
        self.expect_keyword("IN")?;
        self.match_keyword("ARRAY");

        // 表达式
        let _expr = self.collect_expr(&["LOOP"], false)?;

        self.expect_keyword("LOOP")?;
        let mut body = Vec::new();
        while !self.check_keyword("END") && !self.is_eof() {
            body.push(self.parse_statement()?);
        }
        self.expect_keyword("END")?;
        self.expect_keyword("LOOP")?;
        if let PlPgSqlToken::Ident(_) | PlPgSqlToken::QuotedIdent(_) = self.peek() {
            let _ = self.read_ident();
        }
        self.expect_token(&PlPgSqlToken::Semicolon)?;

        Ok(PlPgSqlStatement::ForEach {
            label,
            var,
            slice,
            body,
        })
    }

    /// 解析 EXIT 语句
    fn parse_exit(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("EXIT")?;

        // 可选标签
        let label = if let PlPgSqlToken::Ident(_) | PlPgSqlToken::QuotedIdent(_) = self.peek() {
            Some(self.read_ident()?)
        } else {
            None
        };

        // 可选 WHEN
        let cond = if self.match_keyword("WHEN") {
            Some(self.collect_expr(&[], true)?)
        } else {
            None
        };

        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::Exit { label, cond })
    }

    /// 解析 CONTINUE 语句
    fn parse_continue(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("CONTINUE")?;

        let label = if let PlPgSqlToken::Ident(_) | PlPgSqlToken::QuotedIdent(_) = self.peek() {
            Some(self.read_ident()?)
        } else {
            None
        };

        let cond = if self.match_keyword("WHEN") {
            Some(self.collect_expr(&[], true)?)
        } else {
            None
        };

        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::Continue { label, cond })
    }

    /// 解析 RETURN 语句
    fn parse_return(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("RETURN")?;

        // RETURN NEXT expr
        if self.match_keyword("NEXT") {
            let value = self.collect_expr(&[], true)?;
            self.expect_token(&PlPgSqlToken::Semicolon)?;
            return Ok(PlPgSqlStatement::ReturnNext { value });
        }

        // RETURN QUERY query
        if self.match_keyword("QUERY") {
            let query = self.collect_expr(&[], true)?;
            self.expect_token(&PlPgSqlToken::Semicolon)?;
            return Ok(PlPgSqlStatement::ReturnQuery { query });
        }

        // RETURN [expr]
        let value = if self.peek() == &PlPgSqlToken::Semicolon {
            None
        } else {
            Some(self.collect_expr(&[], true)?)
        };
        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::Return { value })
    }

    /// 解析 PERFORM 语句
    fn parse_perform(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("PERFORM")?;
        let query = self.collect_expr(&[], true)?;
        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::Perform { query })
    }

    /// 解析 EXECUTE 语句
    fn parse_execute(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("EXECUTE")?;
        let query = self.collect_expr(&["INTO", "USING"], true)?;

        // INTO
        let mut into = Vec::new();
        if self.match_keyword("INTO") {
            // STRICT?
            self.match_keyword("STRICT");
            loop {
                let target = self.read_ident()?;
                into.push(target);
                if !self.match_keyword("COMMA") && self.peek() != &PlPgSqlToken::Comma {
                    break;
                }
                if self.peek() == &PlPgSqlToken::Comma {
                    self.advance();
                }
            }
        }

        // USING
        let mut using = Vec::new();
        if self.match_keyword("USING") {
            loop {
                let expr = self.collect_expr_ext(&["USING"], true, true)?;
                using.push(expr);
                if self.peek() == &PlPgSqlToken::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::Execute { query, into, using })
    }

    /// 解析 RAISE 语句
    fn parse_raise(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("RAISE")?;

        // 级别
        let level = match self.peek().clone() {
            PlPgSqlToken::Keyword(k) if k == "DEBUG" => {
                self.advance();
                PlPgSqlRaiseLevel::Debug
            }
            PlPgSqlToken::Keyword(k) if k == "LOG" => {
                self.advance();
                PlPgSqlRaiseLevel::Log
            }
            PlPgSqlToken::Keyword(k) if k == "INFO" => {
                self.advance();
                PlPgSqlRaiseLevel::Info
            }
            PlPgSqlToken::Keyword(k) if k == "NOTICE" => {
                self.advance();
                PlPgSqlRaiseLevel::Notice
            }
            PlPgSqlToken::Keyword(k) if k == "WARNING" => {
                self.advance();
                PlPgSqlRaiseLevel::Warning
            }
            PlPgSqlToken::Keyword(k) if k == "EXCEPTION" => {
                self.advance();
                PlPgSqlRaiseLevel::Exception
            }
            _ => PlPgSqlRaiseLevel::Notice, // 默认级别
        };

        // 格式字符串（可选）
        let format = match self.peek().clone() {
            PlPgSqlToken::String(s) => {
                self.advance();
                Some(s)
            }
            PlPgSqlToken::Semicolon => None,
            _ => {
                // 可能是变量名或表达式作为消息
                let expr = self.collect_expr(&["USING"], true)?;
                if expr.is_empty() {
                    None
                } else {
                    Some(expr)
                }
            }
        };

        // 参数列表（每个参数以逗号分隔，遇 USING 或分号结束）
        let mut args = Vec::new();
        while self.peek() == &PlPgSqlToken::Comma {
            self.advance();
            let arg = self.collect_expr_ext(&["USING"], true, true)?;
            args.push(arg);
        }

        // USING 选项
        let mut options = Vec::new();
        if self.match_keyword("USING") {
            loop {
                let opt_name = self.read_ident()?;
                self.expect_token(&PlPgSqlToken::Eq)?;
                let opt_value = self.collect_expr(&["USING"], true)?;
                options.push((opt_name, opt_value));
                if self.peek() == &PlPgSqlToken::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::Raise {
            level,
            format,
            args,
            options,
        })
    }

    /// 解析 NULL 语句
    fn parse_null(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("NULL")?;
        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::Null)
    }

    /// 解析 GOTO 语句
    fn parse_goto(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        self.expect_keyword("GOTO")?;
        let label = self.read_ident()?;
        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::Goto { label })
    }

    /// 解析 SELECT 语句或赋值
    ///
    /// `SELECT ... INTO ...` 是 SELECT INTO
    /// 普通 SELECT 视为裸 SQL
    fn parse_select_or_assign(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        // 先收集 SELECT 语句直到 INTO / 分号
        let select_part = self.collect_expr(&["INTO"], true)?;

        if self.match_keyword("INTO") {
            // STRICT?
            self.match_keyword("STRICT");

            // 目标变量列表
            let mut targets = Vec::new();
            loop {
                let target = self.read_ident()?;
                targets.push(target);
                if self.peek() == &PlPgSqlToken::Comma {
                    self.advance();
                } else {
                    break;
                }
            }

            // 剩余查询部分（FROM ...）
            let rest = self.collect_expr(&[], true)?;
            let query = if rest.is_empty() {
                format!("SELECT {}", select_part)
            } else {
                format!("SELECT {} {}", select_part, rest)
            };

            self.expect_token(&PlPgSqlToken::Semicolon)?;
            return Ok(PlPgSqlStatement::SelectInto { targets, query });
        }

        // 普通 SELECT — 视为裸 SQL
        let rest = self.collect_expr(&[], true)?;
        let sql = if rest.is_empty() {
            format!("SELECT {}", select_part)
        } else {
            format!("SELECT {} {}", select_part, rest)
        };
        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::SqlStatement { sql })
    }

    /// 解析标识符开头的语句（赋值或裸 SQL）
    fn parse_ident_statement(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        // 读取第一个标识符（可能带 `.` 限定、`[]` 下标，如 `rec.field`、`arr[1]`、`arr[i].field`）
        let first = self.read_ident()?;
        let mut target = first;

        loop {
            match self.peek() {
                PlPgSqlToken::Dot => {
                    self.advance();
                    let field = self.read_ident()?;
                    target.push('.');
                    target.push_str(&field);
                }
                PlPgSqlToken::LBracket => {
                    // 收集下标 `[...]`（支持嵌套 `arr[1][2]`）
                    let mut depth = 0;
                    let mut sub = String::new();
                    while !self.is_eof() {
                        match self.peek() {
                            PlPgSqlToken::LBracket => {
                                depth += 1;
                                self.advance();
                                sub.push('[');
                            }
                            PlPgSqlToken::RBracket => {
                                depth -= 1;
                                self.advance();
                                sub.push(']');
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {
                                let part = self.collect_expr_until_bracket(depth)?;
                                if !part.is_empty() {
                                    if !sub.ends_with('[') {
                                        sub.push_str(", ");
                                    }
                                    sub.push_str(&part);
                                }
                            }
                        }
                    }
                    target.push_str(&sub);
                }
                _ => break,
            }
        }

        // 赋值 `target := expr;`
        if self.peek() == &PlPgSqlToken::Assign {
            self.advance();
            let value = self.collect_expr(&[], true)?;
            self.expect_token(&PlPgSqlToken::Semicolon)?;
            return Ok(PlPgSqlStatement::Assignment { target, value });
        }

        // 不是赋值 — 可能是裸 SQL 或其他
        // 收集剩余部分作为 SQL
        let rest = self.collect_expr(&[], true)?;
        let sql = if rest.is_empty() {
            target
        } else {
            format!("{} {}", target, rest)
        };
        self.expect_token(&PlPgSqlToken::Semicolon)?;
        Ok(PlPgSqlStatement::SqlStatement { sql })
    }

    /// 收集表达式直到匹配的 `]`（用于下标内容）
    ///
    /// `entering_depth` 是进入此调用时 `[` 的嵌套深度。
    /// 当遇到 `]` 使深度减到 `entering_depth - 1` 时停止。
    fn collect_expr_until_bracket(
        &mut self,
        entering_depth: i32,
    ) -> Result<String, PlPgSqlParseError> {
        let mut parts: Vec<String> = Vec::new();
        let mut depth = entering_depth;
        loop {
            match self.peek().clone() {
                PlPgSqlToken::Eof => break,
                PlPgSqlToken::RBracket => {
                    depth -= 1;
                    if depth < entering_depth {
                        break;
                    }
                    parts.push("]".into());
                    self.advance();
                }
                PlPgSqlToken::LBracket => {
                    depth += 1;
                    parts.push("[".into());
                    self.advance();
                }
                PlPgSqlToken::LParen => {
                    depth += 1;
                    parts.push("(".into());
                    self.advance();
                }
                PlPgSqlToken::RParen => {
                    depth -= 1;
                    parts.push(")".into());
                    self.advance();
                }
                PlPgSqlToken::Comma if depth == entering_depth => break,
                PlPgSqlToken::Comma => {
                    parts.push(",".into());
                    self.advance();
                }
                PlPgSqlToken::Keyword(k) => {
                    parts.push(k);
                    self.advance();
                }
                PlPgSqlToken::Ident(s) => {
                    parts.push(s);
                    self.advance();
                }
                PlPgSqlToken::QuotedIdent(s) => {
                    parts.push(format!("\"{}\"", s));
                    self.advance();
                }
                PlPgSqlToken::String(s) => {
                    parts.push(format!("'{}'", s));
                    self.advance();
                }
                PlPgSqlToken::Number(s) => {
                    parts.push(s);
                    self.advance();
                }
                PlPgSqlToken::Label(s) => {
                    parts.push(format!("<<{}>>", s));
                    self.advance();
                }
                PlPgSqlToken::OtherOp(s) => {
                    parts.push(s);
                    self.advance();
                }
                PlPgSqlToken::Assign => {
                    parts.push(":=".into());
                    self.advance();
                }
                PlPgSqlToken::DotDot => {
                    parts.push("..".into());
                    self.advance();
                }
                PlPgSqlToken::Concat => {
                    parts.push("||".into());
                    self.advance();
                }
                PlPgSqlToken::NotEqualAlt => {
                    parts.push("<>".into());
                    self.advance();
                }
                PlPgSqlToken::NotEqual => {
                    parts.push("!=".into());
                    self.advance();
                }
                PlPgSqlToken::LtEq => {
                    parts.push("<=".into());
                    self.advance();
                }
                PlPgSqlToken::GtEq => {
                    parts.push(">=".into());
                    self.advance();
                }
                PlPgSqlToken::Lt => {
                    parts.push("<".into());
                    self.advance();
                }
                PlPgSqlToken::Gt => {
                    parts.push(">".into());
                    self.advance();
                }
                PlPgSqlToken::Eq => {
                    parts.push("=".into());
                    self.advance();
                }
                PlPgSqlToken::Plus => {
                    parts.push("+".into());
                    self.advance();
                }
                PlPgSqlToken::Minus => {
                    parts.push("-".into());
                    self.advance();
                }
                PlPgSqlToken::Star => {
                    parts.push("*".into());
                    self.advance();
                }
                PlPgSqlToken::Slash => {
                    parts.push("/".into());
                    self.advance();
                }
                PlPgSqlToken::Percent => {
                    parts.push("%".into());
                    self.advance();
                }
                PlPgSqlToken::Dot => {
                    parts.push(".".into());
                    self.advance();
                }
                PlPgSqlToken::Semicolon => break,
            }
        }
        let result = parts
            .iter()
            .enumerate()
            .fold(String::new(), |mut acc, (i, part)| {
                if i > 0 {
                    let prev = &parts[i - 1];
                    let prev_is_keyword =
                        !prev.is_empty() && prev.chars().all(|c| c.is_ascii_uppercase());
                    let part_is_paren = part.starts_with('(');
                    let needs_space = !prev.ends_with('(')
                        && !prev.ends_with('[')
                        && !prev.ends_with('.')
                        && !part.starts_with(')')
                        && !part.starts_with(']')
                        && !part.starts_with(',')
                        && !part.starts_with(';')
                        && !part.starts_with('.')
                        && prev != "("
                        && prev != "["
                        && part != ")"
                        && part != "]"
                        && part != ",";
                    if needs_space && (!part_is_paren || prev_is_keyword) {
                        acc.push(' ');
                    }
                }
                acc.push_str(part);
                acc
            });
        Ok(result.trim().to_string())
    }

    /// 解析裸 SQL 语句
    fn parse_sql_statement(&mut self) -> Result<PlPgSqlStatement, PlPgSqlParseError> {
        let sql = self.collect_expr(&[], true)?;
        self.expect_token(&PlPgSqlToken::Semicolon)?;
        if sql.is_empty() {
            let pos = self.peek_pos().clone();
            return Err(PlPgSqlParseError::Parse {
                line: pos.line,
                col: pos.col,
                expected: "statement".into(),
                got: format!("{:?}", self.peek()),
            });
        }
        Ok(PlPgSqlStatement::SqlStatement { sql })
    }
}

/// 辅助枚举：类型引用种类
#[derive(Debug, Clone, Copy)]
enum PlPgSqlTypeRefKind {
    Type,
    RowType,
}

// =====================================================================
//  入口函数
// =====================================================================

/// 解析 PL/pgSQL 函数体
///
/// 输入为 `$$` 之间的原始文本（不含 `$$` 分隔符）。
///
/// # 示例
///
/// ```
/// use szrsql_sql::plpgsql::parse_function_body;
///
/// let src = "BEGIN RETURN 1; END;";
/// let block = parse_function_body(src).unwrap();
/// assert_eq!(block.statements.len(), 1);
/// ```
pub fn parse_function_body(src: &str) -> Result<PlPgSqlBlock, PlPgSqlParseError> {
    let mut parser = PlPgSqlParser::from_src(src)?;
    let block = parser.parse_block()?;
    // 确保所有 token 已消费（允许尾部空白/分号）
    if !parser.is_eof() {
        let pos = parser.peek_pos().clone();
        return Err(PlPgSqlParseError::Parse {
            line: pos.line,
            col: pos.col,
            expected: "EOF".into(),
            got: format!("{:?}", parser.peek()),
        });
    }
    Ok(block)
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_basic() {
        let tokens = Tokenizer::new("BEGIN RETURN 1; END;").tokenize().unwrap();
        assert!(tokens.len() >= 6);
        assert!(matches!(tokens[0].token, PlPgSqlToken::Keyword(ref k) if k == "BEGIN"));
        assert!(matches!(tokens[1].token, PlPgSqlToken::Keyword(ref k) if k == "RETURN"));
        assert!(matches!(tokens[2].token, PlPgSqlToken::Number(_)));
        assert!(matches!(tokens[3].token, PlPgSqlToken::Semicolon));
    }

    #[test]
    fn test_tokenize_assign() {
        let tokens = Tokenizer::new("x := 10;").tokenize().unwrap();
        assert!(matches!(tokens[1].token, PlPgSqlToken::Assign));
    }

    #[test]
    fn test_tokenize_dotdot() {
        let tokens = Tokenizer::new("1 .. 10").tokenize().unwrap();
        assert!(matches!(tokens[1].token, PlPgSqlToken::DotDot));
    }

    #[test]
    fn test_parse_simple_return() {
        let block = parse_function_body("BEGIN RETURN 1; END;").unwrap();
        assert_eq!(block.statements.len(), 1);
        assert!(matches!(
            &block.statements[0],
            PlPgSqlStatement::Return { value: Some(v) } if v == "1"
        ));
    }

    #[test]
    fn test_parse_assignment() {
        let block = parse_function_body("BEGIN x := 10; END;").unwrap();
        assert_eq!(block.statements.len(), 1);
        match &block.statements[0] {
            PlPgSqlStatement::Assignment { target, value } => {
                assert_eq!(target, "x");
                assert_eq!(value, "10");
            }
            other => panic!("expected Assignment, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_if() {
        let src = "BEGIN IF x > 10 THEN RETURN 1; ELSE RETURN 2; END IF; END;";
        let block = parse_function_body(src).unwrap();
        match &block.statements[0] {
            PlPgSqlStatement::If {
                branches,
                else_branch,
            } => {
                assert_eq!(branches.len(), 1);
                assert!(else_branch.is_some());
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_loop() {
        let src = "BEGIN LOOP EXIT; END LOOP; END;";
        let block = parse_function_body(src).unwrap();
        assert!(matches!(
            &block.statements[0],
            PlPgSqlStatement::Loop { .. }
        ));
    }

    #[test]
    fn test_parse_while() {
        let src = "BEGIN WHILE x < 10 LOOP x := x + 1; END LOOP; END;";
        let block = parse_function_body(src).unwrap();
        assert!(matches!(
            &block.statements[0],
            PlPgSqlStatement::While { .. }
        ));
    }

    #[test]
    fn test_parse_for_integer() {
        let src = "BEGIN FOR i IN 1 .. 10 LOOP RAISE NOTICE '%', i; END LOOP; END;";
        let block = parse_function_body(src).unwrap();
        match &block.statements[0] {
            PlPgSqlStatement::For {
                var, lower, upper, ..
            } => {
                assert_eq!(var, "i");
                assert_eq!(lower, "1");
                assert_eq!(upper, "10");
            }
            other => panic!("expected For, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_declaration() {
        let src = "DECLARE x INTEGER := 0; BEGIN x := x + 1; END;";
        let block = parse_function_body(src).unwrap();
        assert_eq!(block.declarations.len(), 1);
        match &block.declarations[0] {
            PlPgSqlDeclaration::Variable {
                name,
                data_type,
                default,
                ..
            } => {
                assert_eq!(name, "x");
                assert_eq!(data_type, "INTEGER");
                assert_eq!(default.as_deref(), Some("0"));
            }
            other => panic!("expected Variable, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_exception() {
        let src = "BEGIN RETURN 1; EXCEPTION WHEN OTHERS THEN RETURN 0; END;";
        let block = parse_function_body(src).unwrap();
        assert_eq!(block.exception_handlers.len(), 1);
        assert_eq!(block.exception_handlers[0].conditions, vec!["OTHERS"]);
    }

    #[test]
    fn test_parse_raise() {
        let src = "BEGIN RAISE NOTICE 'hello %', name; END;";
        let block = parse_function_body(src).unwrap();
        match &block.statements[0] {
            PlPgSqlStatement::Raise {
                level,
                format,
                args,
                ..
            } => {
                assert_eq!(*level, PlPgSqlRaiseLevel::Notice);
                assert_eq!(format.as_deref(), Some("hello %"));
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected Raise, got {:?}", other),
        }
    }
}
