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

    #[test]
    fn test_language_parse_all() {
        assert_eq!(Language::parse("rust"), Language::Rust);
        assert_eq!(Language::parse("rs"), Language::Rust);
        assert_eq!(Language::parse("typescript"), Language::TypeScript);
        assert_eq!(Language::parse("ts"), Language::TypeScript);
        assert_eq!(Language::parse("javascript"), Language::JavaScript);
        assert_eq!(Language::parse("js"), Language::JavaScript);
        assert_eq!(Language::parse("html"), Language::Html);
        assert_eq!(Language::parse("css"), Language::Css);
        assert_eq!(Language::parse("json"), Language::Json);
        assert_eq!(Language::parse("sql"), Language::Sql);
        assert_eq!(Language::parse("toml"), Language::Toml);
        assert_eq!(Language::parse("yaml"), Language::Yaml);
        assert_eq!(Language::parse("yml"), Language::Yaml);
        assert_eq!(Language::parse("markdown"), Language::Markdown);
        assert_eq!(Language::parse("md"), Language::Markdown);
        assert_eq!(Language::parse("unknown"), Language::Plain);
    }

    #[test]
    fn test_language_as_str_all() {
        assert_eq!(Language::Rust.as_str(), "rust");
        assert_eq!(Language::TypeScript.as_str(), "typescript");
        assert_eq!(Language::JavaScript.as_str(), "javascript");
        assert_eq!(Language::Html.as_str(), "html");
        assert_eq!(Language::Css.as_str(), "css");
        assert_eq!(Language::Json.as_str(), "json");
        assert_eq!(Language::Sql.as_str(), "sql");
        assert_eq!(Language::Toml.as_str(), "toml");
        assert_eq!(Language::Yaml.as_str(), "yaml");
        assert_eq!(Language::Markdown.as_str(), "markdown");
        assert_eq!(Language::Plain.as_str(), "plain");
    }

    #[test]
    fn test_highlight_json() {
        let code = r#"{"key": "value"}"#;
        let result = SyntaxHighlighter::highlight(code, Language::Json);
        assert!(result.contains("string"));
    }

    #[test]
    fn test_highlight_other_languages() {
        let code = "some code";
        assert!(!SyntaxHighlighter::highlight(code, Language::Html).is_empty());
        assert!(!SyntaxHighlighter::highlight(code, Language::Css).is_empty());
        assert!(!SyntaxHighlighter::highlight(code, Language::Sql).is_empty());
        assert!(!SyntaxHighlighter::highlight(code, Language::Toml).is_empty());
        assert!(!SyntaxHighlighter::highlight(code, Language::Yaml).is_empty());
        assert!(!SyntaxHighlighter::highlight(code, Language::Markdown).is_empty());
        assert!(!SyntaxHighlighter::highlight(code, Language::TypeScript).is_empty());
        assert!(!SyntaxHighlighter::highlight(code, Language::JavaScript).is_empty());
    }

    #[test]
    fn test_highlight_plain_with_ampersand() {
        let code = "a & b";
        let result = SyntaxHighlighter::highlight(code, Language::Plain);
        assert!(result.contains("&amp;"));
    }

    #[test]
    fn test_preview_service_json() {
        let result = PreviewService::preview(r#"{"key": "val"}"#, "json");
        assert!(result.contains("string"));
    }

    #[test]
    fn test_preview_service_plain() {
        let result = PreviewService::preview("<b>text</b>", "plain");
        assert!(result.contains("&lt;b&gt;"));
    }

    #[test]
    fn test_line_count_empty() {
        assert_eq!(PreviewService::line_count(""), 0);
    }

    #[test]
    fn test_with_line_numbers_single() {
        let result = PreviewService::with_line_numbers("only line");
        assert!(result.contains("1 | only line"));
    }
}
