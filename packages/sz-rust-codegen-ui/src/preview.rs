// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 代码预览 + 语法高亮（spec §5.27 规则 3）

/// 支持的语言
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    TypeScript,
    JavaScript,
    Html,
    Css,
    Json,
    Sql,
    Toml,
    Yaml,
    Markdown,
    Plain,
}

impl Language {
    /// 从字符串解析
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Self::Rust,
            "typescript" | "ts" => Self::TypeScript,
            "javascript" | "js" => Self::JavaScript,
            "html" => Self::Html,
            "css" => Self::Css,
            "json" => Self::Json,
            "sql" => Self::Sql,
            "toml" => Self::Toml,
            "yaml" | "yml" => Self::Yaml,
            "markdown" | "md" => Self::Markdown,
            _ => Self::Plain,
        }
    }

    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
            Self::Html => "html",
            Self::Css => "css",
            Self::Json => "json",
            Self::Sql => "sql",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::Markdown => "markdown",
            Self::Plain => "plain",
        }
    }
}

/// 语法高亮器（spec §5.27 规则 3）
pub struct SyntaxHighlighter;

impl SyntaxHighlighter {
    /// 生成 HTML 高亮代码（简单实现，用 `<span class="...">` 包裹关键字）
    pub fn highlight(code: &str, language: Language) -> String {
        match language {
            Language::Rust => Self::highlight_rust(code),
            Language::Json => Self::highlight_json(code),
            _ => Self::highlight_plain(code),
        }
    }

    /// Rust 语法高亮
    fn highlight_rust(code: &str) -> String {
        let keywords = [
            "fn", "struct", "enum", "impl", "pub", "use", "mod", "let", "const", "static", "mut",
            "ref", "match", "if", "else", "for", "while", "loop", "return", "break", "continue",
            "async", "await", "trait", "where", "Self", "self",
        ];
        let mut result = String::new();
        for word in code.split_whitespace() {
            let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
            if keywords.contains(&clean) {
                result.push_str(&format!("<span class=\"keyword\">{word}</span> "));
            } else {
                result.push_str(word);
                result.push(' ');
            }
        }
        result
    }

    /// JSON 语法高亮
    fn highlight_json(code: &str) -> String {
        code.replace('"', "<span class=\"string\">\"</span>")
    }

    /// 纯文本（转义 HTML）
    fn highlight_plain(code: &str) -> String {
        code.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

/// 代码预览服务（spec §5.27 规则 3）
pub struct PreviewService;

impl PreviewService {
    /// 代码预览（语法高亮，spec §5.27 规则 3）
    pub fn preview(code: &str, language: &str) -> String {
        let lang = Language::parse(language);
        SyntaxHighlighter::highlight(code, lang)
    }

    /// 获取行数
    pub fn line_count(code: &str) -> usize {
        code.lines().count()
    }

    /// 添加行号
    pub fn with_line_numbers(code: &str) -> String {
        code.lines()
            .enumerate()
            .map(|(i, line)| format!("{:>4} | {line}", i + 1))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_from_str() {
        assert_eq!(Language::parse("rust"), Language::Rust);
        assert_eq!(Language::parse("TS"), Language::TypeScript);
        assert_eq!(Language::parse("unknown"), Language::Plain);
    }

    #[test]
    fn test_highlight_rust() {
        let code = "fn main() { let x = 42; }";
        let highlighted = SyntaxHighlighter::highlight(code, Language::Rust);
        assert!(highlighted.contains("keyword"));
    }

    #[test]
    fn test_highlight_plain() {
        let code = "<script>alert('xss')</script>";
        let highlighted = SyntaxHighlighter::highlight(code, Language::Plain);
        assert!(highlighted.contains("&lt;script&gt;"));
    }

    #[test]
    fn test_preview_service() {
        let result = PreviewService::preview("fn main() {}", "rust");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_line_count() {
        assert_eq!(PreviewService::line_count("a\nb\nc"), 3);
        assert_eq!(PreviewService::line_count("single"), 1);
    }

    #[test]
    fn test_with_line_numbers() {
        let result = PreviewService::with_line_numbers("hello\nworld");
        assert!(result.contains("1 | hello"));
        assert!(result.contains("2 | world"));
    }
}
