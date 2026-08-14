//! Tera 自定义过滤器

use tera::{Result as TeraResult, Tera, Value};

/// 注册 7 个自定义过滤器
pub fn register_filters(tera: &mut Tera) {
    tera.register_filter("rust_to_ts_type", filter_rust_to_ts_type);
    tera.register_filter("snake_to_pascal", filter_snake_to_pascal);
    tera.register_filter("pascal_to_kebab", filter_pascal_to_kebab);
    tera.register_filter("snake_to_camel", filter_snake_to_camel);
    tera.register_filter("is_sensitive", filter_is_sensitive);
    tera.register_filter("pluralize", filter_pluralize);
    tera.register_filter("singularize", filter_singularize);
    tera.register_filter("capitalize", filter_capitalize);
}

fn filter_rust_to_ts_type(
    value: &Value,
    _args: &std::collections::HashMap<String, Value>,
) -> TeraResult<Value> {
    let s = value.as_str().unwrap_or("");
    Ok(Value::String(crate::model_parser::rust_to_ts_type(s)))
}

fn filter_snake_to_pascal(
    value: &Value,
    _args: &std::collections::HashMap<String, Value>,
) -> TeraResult<Value> {
    let s = value.as_str().unwrap_or("");
    let pascal: String = s
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect();
    Ok(Value::String(pascal))
}

fn filter_pascal_to_kebab(
    value: &Value,
    _args: &std::collections::HashMap<String, Value>,
) -> TeraResult<Value> {
    let s = value.as_str().unwrap_or("");
    let mut kebab = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            kebab.push('-');
        }
        kebab.push(ch.to_ascii_lowercase());
    }
    Ok(Value::String(kebab))
}

fn filter_snake_to_camel(
    value: &Value,
    _args: &std::collections::HashMap<String, Value>,
) -> TeraResult<Value> {
    let s = value.as_str().unwrap_or("");
    let parts: Vec<&str> = s.split('_').collect();
    let mut camel = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            camel.push_str(part);
        } else {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                camel.push_str(&first.to_uppercase().collect::<String>());
                camel.push_str(chars.as_str());
            }
        }
    }
    Ok(Value::String(camel))
}

fn filter_is_sensitive(
    value: &Value,
    _args: &std::collections::HashMap<String, Value>,
) -> TeraResult<Value> {
    let s = value.as_str().unwrap_or("");
    Ok(Value::Bool(crate::model_parser::is_sensitive_field(s)))
}

fn filter_pluralize(
    value: &Value,
    _args: &std::collections::HashMap<String, Value>,
) -> TeraResult<Value> {
    let s = value.as_str().unwrap_or("");
    let plural = if s.ends_with('y') && s.len() > 1 {
        format!("{}ies", &s[..s.len() - 1])
    } else if s.ends_with('s') {
        s.to_string()
    } else {
        format!("{s}s")
    };
    Ok(Value::String(plural))
}

fn filter_singularize(
    value: &Value,
    _args: &std::collections::HashMap<String, Value>,
) -> TeraResult<Value> {
    let s = value.as_str().unwrap_or("");
    let singular = if s.ends_with("ies") && s.len() > 3 {
        format!("{}y", &s[..s.len() - 3])
    } else if s.ends_with('s') && !s.ends_with("ss") {
        s[..s.len() - 1].to_string()
    } else {
        s.to_string()
    };
    Ok(Value::String(singular))
}

fn filter_capitalize(
    value: &Value,
    _args: &std::collections::HashMap<String, Value>,
) -> TeraResult<Value> {
    let s = value.as_str().unwrap_or("");
    let mut chars = s.chars();
    let result = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    Ok(Value::String(result))
}
