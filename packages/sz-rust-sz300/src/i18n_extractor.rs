// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 国际化语言提取器（spec §5.5.1 规则 1）
//!
//! 从请求头 `Accept-Language` 或查询参数 `lang` 提取客户端语言偏好，
//! 缺失回退默认语言 en（spec §5.5.3 异常 1）。

/// 语言代码（如 "zh-CN", "en", "ja"）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LanguageCode(pub String);

impl LanguageCode {
    /// 从字符串创建语言代码
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    /// 默认语言 en（spec §5.5.3 异常 1）
    pub fn default_en() -> Self {
        Self("en".to_string())
    }

    /// 获取语言代码字符串
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 是否为中文
    pub fn is_chinese(&self) -> bool {
        self.0.to_lowercase().starts_with("zh")
    }
}

impl std::fmt::Display for LanguageCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for LanguageCode {
    fn default() -> Self {
        Self::default_en()
    }
}

/// v1.9.0 国际化语言提取器（spec §5.5.1 规则 1）
pub struct I18nExtractor;

impl I18nExtractor {
    /// 从请求头和查询参数提取语言偏好（spec §5.5.1 规则 1）
    ///
    /// 优先级：
    /// 1. 查询参数 `lang=xxx`
    /// 2. 请求头 `Accept-Language` 第一个偏好
    /// 3. 默认语言 en（spec §5.5.3 异常 1）
    pub fn from_request(accept_language: Option<&str>, query: &str) -> LanguageCode {
        if let Some(lang) = Self::extract_from_query(query) {
            return lang;
        }
        if let Some(lang) = Self::extract_from_accept_language(accept_language) {
            return lang;
        }
        LanguageCode::default_en()
    }

    /// 从查询参数提取 `lang=xxx`
    fn extract_from_query(query: &str) -> Option<LanguageCode> {
        for pair in query.split('&') {
            let mut parts = pair.splitn(2, '=');
            if parts.next() == Some("lang") {
                if let Some(value) = parts.next() {
                    if !value.is_empty() {
                        return Some(LanguageCode::new(value));
                    }
                }
            }
        }
        None
    }

    /// 从 `Accept-Language` 头提取第一个语言偏好
    fn extract_from_accept_language(value: Option<&str>) -> Option<LanguageCode> {
        let value = value?;
        let first = value.split(',').next()?;
        let lang = first.split(';').next()?.trim();
        if lang.is_empty() || lang == "*" {
            return None;
        }
        Some(LanguageCode::new(lang))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_from_query_param() {
        let lang = I18nExtractor::from_request(None, "page=1&lang=zh-CN");
        assert_eq!(lang.as_str(), "zh-CN");
    }

    #[test]
    fn test_extract_from_accept_language_header() {
        let lang = I18nExtractor::from_request(Some("zh-CN,zh;q=0.9"), "");
        assert_eq!(lang.as_str(), "zh-CN");
    }

    #[test]
    fn test_extract_fallback_to_en() {
        let lang = I18nExtractor::from_request(None, "");
        assert_eq!(lang.as_str(), "en");
    }

    #[test]
    fn test_extract_query_takes_precedence_over_header() {
        let lang = I18nExtractor::from_request(Some("zh-CN"), "lang=en");
        assert_eq!(lang.as_str(), "en");
    }

    #[test]
    fn test_extract_accept_language_with_quality() {
        let lang = I18nExtractor::from_request(Some("en-US,en;q=0.9,zh-CN;q=0.8"), "");
        assert_eq!(lang.as_str(), "en-US");
    }

    #[test]
    fn test_language_code_is_chinese() {
        assert!(LanguageCode::new("zh-CN").is_chinese());
        assert!(LanguageCode::new("zh-cn").is_chinese());
        assert!(LanguageCode::new("zh").is_chinese());
        assert!(!LanguageCode::new("en").is_chinese());
        assert!(!LanguageCode::new("ja").is_chinese());
    }

    #[test]
    fn test_empty_lang_query_falls_through() {
        let lang = I18nExtractor::from_request(None, "lang=");
        assert_eq!(lang.as_str(), "en");
    }
}
