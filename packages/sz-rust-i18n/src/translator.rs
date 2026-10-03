//! 翻译服务（spec §5.4 规则 2-3）
//!
//! 运行时语言切换 + 参数插值 + 缺失键降级 + 复数。

use std::collections::HashMap;

use parking_lot::RwLock;
use serde_json::Value;

use super::error::I18nError;
use super::fallback::{fallback_lookup, value_to_string, FallbackStrategy};
use super::plural::{parse_plural_forms, select_plural, PluralForms};
use super::resource::{parse_resource, LanguageCode, ResourceFormat};

/// i18n 服务
pub struct I18nService {
    /// 消息资源（语言 → 键 → 消息）
    messages: RwLock<HashMap<LanguageCode, HashMap<String, Value>>>,
    /// 默认语言
    default_language: RwLock<LanguageCode>,
    /// 降级策略
    fallback: FallbackStrategy,
    /// 缺失键日志
    missing_log: RwLock<Vec<String>>,
}

impl I18nService {
    /// 创建 i18n 服务
    pub fn new(default_language: LanguageCode, fallback: FallbackStrategy) -> Self {
        Self {
            messages: RwLock::new(HashMap::new()),
            default_language: RwLock::new(default_language),
            fallback,
            missing_log: RwLock::new(Vec::new()),
        }
    }

    /// 加载消息资源（TOML/JSON/YAML）
    pub fn load_resource(
        &self,
        lang: &LanguageCode,
        format: ResourceFormat,
        content: &str,
    ) -> Result<(), I18nError> {
        let map = parse_resource(format, content)?;
        let mut messages = self.messages.write();
        messages.insert(lang.clone(), map);
        Ok(())
    }

    /// 查询消息（含参数插值 + 缺失键降级）
    pub fn translate(
        &self,
        lang: &LanguageCode,
        key: &str,
        params: &HashMap<String, Value>,
    ) -> String {
        let messages = self.messages.read();
        let default_lang = self.default_language.read();

        let current_msg = messages.get(lang).and_then(|m| m.get(key));
        let default_msg = messages.get(&*default_lang).and_then(|m| m.get(key));

        let mut log = Vec::new();
        let result = fallback_lookup(current_msg, default_msg, key, self.fallback, &mut log);

        if !log.is_empty() {
            self.missing_log.write().extend(log);
        }

        match result {
            Some(v) => interpolate(&value_to_string(&v), params),
            None => key.to_string(),
        }
    }

    /// 复数查询（按 count 选择 one/other/zero/many）
    pub fn translate_plural(
        &self,
        lang: &LanguageCode,
        key: &str,
        count: u64,
        params: &HashMap<String, Value>,
    ) -> String {
        let messages = self.messages.read();
        let default_lang = self.default_language.read();

        let current_msg = messages.get(lang).and_then(|m| m.get(key));
        let default_msg = messages.get(&*default_lang).and_then(|m| m.get(key));

        let plural_value = current_msg.or(default_msg);
        let forms = plural_value.and_then(parse_plural_forms).or_else(|| {
            let lang_map = messages
                .get(lang)
                .or_else(|| messages.get(&*default_lang))?;
            build_plural_from_flattened(lang_map, key)
        });

        match forms {
            Some(forms) => {
                let mut full_params = params.clone();
                full_params.insert("count".to_string(), Value::Number(count.into()));
                interpolate(&select_plural(&forms, count), &full_params)
            }
            None => {
                self.missing_log
                    .write()
                    .push(format!("复数键 '{key}' 缺失"));
                key.to_string()
            }
        }
    }

    /// 运行时切换默认语言
    pub fn switch_default(&self, lang: LanguageCode) -> Result<(), I18nError> {
        let mut default = self.default_language.write();
        *default = lang;
        Ok(())
    }

    /// 获取缺失键日志快照
    pub fn missing_keys(&self) -> Vec<String> {
        self.missing_log.read().clone()
    }

    /// 清空缺失键日志
    pub fn clear_missing_log(&self) {
        self.missing_log.write().clear();
    }
}

/// 参数插值：将 `{name}` 替换为 params 中对应的值
fn interpolate(template: &str, params: &HashMap<String, Value>) -> String {
    let mut result = template.to_string();
    for (key, val) in params {
        let placeholder = format!("{{{key}}}");
        result = result.replace(&placeholder, &value_to_string(val));
    }
    result
}

/// 从展平的键（key.zero / key.one / key.other / key.many）构造 PluralForms
fn build_plural_from_flattened(map: &HashMap<String, Value>, key: &str) -> Option<PluralForms> {
    let get_str = |suffix: &str| {
        map.get(&format!("{key}.{suffix}"))
            .and_then(|v| v.as_str())
            .map(String::from)
    };
    let other = get_str("other")?;
    Some(PluralForms {
        zero: get_str("zero"),
        one: get_str("one"),
        other,
        many: get_str("many"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_service() -> I18nService {
        let default = LanguageCode::new("en").unwrap();
        I18nService::new(default, FallbackStrategy::DefaultLanguage)
    }

    #[test]
    fn test_load_and_translate() {
        let svc = make_service();
        let lang = LanguageCode::new("en").unwrap();
        svc.load_resource(
            &lang,
            ResourceFormat::Json,
            r#"{"greeting": "Hello, {name}!"}"#,
        )
        .unwrap();

        let mut params = HashMap::new();
        params.insert("name".to_string(), Value::String("World".to_string()));
        let result = svc.translate(&lang, "greeting", &params);
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_fallback_to_default_language() {
        let svc = make_service();
        let en = LanguageCode::new("en").unwrap();
        let zh = LanguageCode::new("zh").unwrap();
        svc.load_resource(&en, ResourceFormat::Json, r#"{"greeting": "Hello"}"#)
            .unwrap();
        svc.load_resource(&zh, ResourceFormat::Json, r#"{"other": "你好"}"#)
            .unwrap();

        let params = HashMap::new();
        let result = svc.translate(&zh, "greeting", &params);
        assert_eq!(result, "Hello");
        assert_eq!(svc.missing_keys().len(), 1);
    }

    #[test]
    fn test_switch_default_language() {
        let svc = make_service();
        let zh = LanguageCode::new("zh").unwrap();
        svc.switch_default(zh).unwrap();
        let en = LanguageCode::new("en").unwrap();
        svc.load_resource(&en, ResourceFormat::Json, r#"{"key": "English"}"#)
            .unwrap();
        let zh2 = LanguageCode::new("zh").unwrap();
        svc.load_resource(&zh2, ResourceFormat::Json, r#"{"key": "中文"}"#)
            .unwrap();

        let params = HashMap::new();
        let result = svc.translate(&zh2, "key", &params);
        assert_eq!(result, "中文");
    }

    #[test]
    fn test_plural_translation() {
        let svc = make_service();
        let en = LanguageCode::new("en").unwrap();
        svc.load_resource(
            &en,
            ResourceFormat::Json,
            r#"{"items": {"zero": "no items", "one": "one item", "other": "{count} items"}}"#,
        )
        .unwrap();

        let params = HashMap::new();
        assert_eq!(svc.translate_plural(&en, "items", 0, &params), "no items");
        assert_eq!(svc.translate_plural(&en, "items", 1, &params), "one item");
        assert_eq!(svc.translate_plural(&en, "items", 5, &params), "5 items");
    }

    #[test]
    fn test_interpolate_multiple_params() {
        let template = "{greeting}, {name}! You have {count} messages.";
        let mut params = HashMap::new();
        params.insert("greeting".to_string(), Value::String("Hi".to_string()));
        params.insert("name".to_string(), Value::String("Alice".to_string()));
        params.insert("count".to_string(), Value::Number(3.into()));
        let result = interpolate(template, &params);
        assert_eq!(result, "Hi, Alice! You have 3 messages.");
    }

    #[test]
    fn test_missing_key_returns_key_name() {
        let svc = I18nService::new(LanguageCode::new("en").unwrap(), FallbackStrategy::KeyName);
        let en = LanguageCode::new("en").unwrap();
        let params = HashMap::new();
        let result = svc.translate(&en, "nonexistent", &params);
        assert_eq!(result, "nonexistent");
    }

    #[test]
    fn test_missing_key_placeholder() {
        let svc = I18nService::new(
            LanguageCode::new("en").unwrap(),
            FallbackStrategy::Placeholder,
        );
        let en = LanguageCode::new("en").unwrap();
        let params = HashMap::new();
        let result = svc.translate(&en, "nonexistent", &params);
        assert_eq!(result, "[nonexistent]");
    }
}
