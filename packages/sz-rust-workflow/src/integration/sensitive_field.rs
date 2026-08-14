use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::RwLock;
use serde_json::Value;

/// 敏感字段注册表，对齐 design 2.3.2 脱敏模型。
///
/// 系统字段（`instance_id`/`status`/`flow_key`）永不脱敏。
pub struct SensitiveFieldRegistry {
    fields: RwLock<HashSet<String>>,
}

/// 系统保留字段，永不脱敏。
const SYSTEM_FIELDS: &[&str] = &["instance_id", "status", "flow_key"];

impl SensitiveFieldRegistry {
    pub fn new() -> Self {
        Self {
            fields: RwLock::new(HashSet::new()),
        }
    }

    /// 注册敏感字段名。
    pub fn register(&self, field_name: impl Into<String>) {
        let name = field_name.into();
        if SYSTEM_FIELDS.contains(&name.as_str()) {
            tracing::warn!(field = %name, "系统字段不可注册为敏感字段，忽略");
            return;
        }
        self.fields.write().insert(name);
    }

    /// 批量注册。
    pub fn register_many(&self, fields: impl IntoIterator<Item = String>) {
        let mut guard = self.fields.write();
        for f in fields {
            if !SYSTEM_FIELDS.contains(&f.as_str()) {
                guard.insert(f);
            }
        }
    }

    /// 判断字段是否为敏感字段。
    pub fn is_sensitive(&self, field_name: &str) -> bool {
        if SYSTEM_FIELDS.contains(&field_name) {
            return false;
        }
        self.fields.read().contains(field_name)
    }

    /// 对 JSON 值递归脱敏。
    ///
    /// - object：对每个键判断是否敏感，敏感则 mask 值，否则递归
    /// - array：递归每个元素
    /// - 其他：原值返回（叶子值由 object 键决定是否 mask）
    pub fn mask(&self, value: &Value) -> Value {
        self.mask_value(value, false)
    }

    fn mask_value(&self, value: &Value, should_mask: bool) -> Value {
        if should_mask {
            return match value {
                Value::String(_) => Value::String("***".into()),
                Value::Number(n) if n.is_i64() => Value::Number(0.into()),
                Value::Number(n) if n.is_u64() => Value::Number(0.into()),
                Value::Number(_) => serde_json::Number::from_f64(0.0)
                    .map(Value::Number)
                    .unwrap_or(Value::Null),
                Value::Bool(_) => Value::Bool(false),
                Value::Null => Value::Null,
                Value::Array(arr) => {
                    Value::Array(arr.iter().map(|v| self.mask_value(v, true)).collect())
                }
                Value::Object(obj) => Value::Object(
                    obj.iter()
                        .map(|(k, v)| (k.clone(), self.mask_value(v, true)))
                        .collect(),
                ),
            };
        }
        match value {
            Value::Object(obj) => Value::Object(
                obj.iter()
                    .map(|(k, v)| {
                        let mask = self.is_sensitive(k);
                        (k.clone(), self.mask_value(v, mask))
                    })
                    .collect(),
            ),
            Value::Array(arr) => {
                Value::Array(arr.iter().map(|v| self.mask_value(v, false)).collect())
            }
            _ => value.clone(),
        }
    }

    /// 合并能力返回值到上下文，忽略保留字段并 warn。
    pub fn merge_capability_result(&self, context: &mut Value, result: Value) {
        if let (Value::Object(ctx), Value::Object(res)) = (context, &result) {
            for (k, v) in res {
                if SYSTEM_FIELDS.contains(&k.as_str()) {
                    tracing::warn!(field = %k, "能力返回值含保留字段，忽略");
                    continue;
                }
                ctx.insert(k.clone(), v.clone());
            }
        } else if let Value::Object(res) = &result {
            tracing::warn!("上下文非 object，无法合并能力返回值");
            let _ = res;
        }
    }
}

impl Default for SensitiveFieldRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 便捷构造 Arc 包装。
pub fn new_registry() -> Arc<SensitiveFieldRegistry> {
    Arc::new(SensitiveFieldRegistry::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_is_sensitive() {
        let r = SensitiveFieldRegistry::new();
        r.register("phone");
        assert!(r.is_sensitive("phone"));
        assert!(!r.is_sensitive("name"));
    }

    #[test]
    fn system_field_never_sensitive() {
        let r = SensitiveFieldRegistry::new();
        r.register("instance_id");
        r.register("status");
        assert!(!r.is_sensitive("instance_id"));
        assert!(!r.is_sensitive("status"));
    }

    #[test]
    fn mask_string() {
        let r = SensitiveFieldRegistry::new();
        r.register("phone");
        let val = serde_json::json!({"phone": "13800138000", "name": "张三"});
        let masked = r.mask(&val);
        assert_eq!(masked["phone"], "***");
        assert_eq!(masked["name"], "张三");
    }

    #[test]
    fn mask_number_and_bool() {
        let r = SensitiveFieldRegistry::new();
        r.register("salary");
        r.register("active");
        let val = serde_json::json!({"salary": 9999, "active": true});
        let masked = r.mask(&val);
        assert_eq!(masked["salary"], 0);
        assert_eq!(masked["active"], false);
    }

    #[test]
    fn mask_nested_object() {
        let r = SensitiveFieldRegistry::new();
        r.register("secret");
        let val = serde_json::json!({
            "outer": {"secret": "hidden", "public": "visible"},
            "secret": "top_hidden"
        });
        let masked = r.mask(&val);
        assert_eq!(masked["outer"]["secret"], "***");
        assert_eq!(masked["outer"]["public"], "visible");
        assert_eq!(masked["secret"], "***");
    }

    #[test]
    fn mask_array() {
        let r = SensitiveFieldRegistry::new();
        r.register("phone");
        let val = serde_json::json!([
            {"phone": "111", "name": "a"},
            {"phone": "222", "name": "b"}
        ]);
        let masked = r.mask(&val);
        assert_eq!(masked[0]["phone"], "***");
        assert_eq!(masked[0]["name"], "a");
        assert_eq!(masked[1]["phone"], "***");
    }

    #[test]
    fn system_field_not_masked() {
        let r = SensitiveFieldRegistry::new();
        r.register("instance_id");
        let val =
            serde_json::json!({"instance_id": "i1", "status": "running", "flow_key": "leave"});
        let masked = r.mask(&val);
        assert_eq!(masked["instance_id"], "i1");
        assert_eq!(masked["status"], "running");
        assert_eq!(masked["flow_key"], "leave");
    }

    #[test]
    fn merge_capability_result_ignores_reserved() {
        let r = SensitiveFieldRegistry::new();
        let mut ctx = serde_json::json!({"name": "张三"});
        let result = serde_json::json!({"phone": "138", "instance_id": "should_be_ignored"});
        r.merge_capability_result(&mut ctx, result);
        assert_eq!(ctx["phone"], "138");
        assert!(ctx.get("instance_id").is_none() || ctx["instance_id"] != "should_be_ignored");
    }

    #[test]
    fn register_many() {
        let r = SensitiveFieldRegistry::new();
        r.register_many(vec!["phone".into(), "email".into(), "instance_id".into()]);
        assert!(r.is_sensitive("phone"));
        assert!(r.is_sensitive("email"));
        assert!(!r.is_sensitive("instance_id"));
    }
}
