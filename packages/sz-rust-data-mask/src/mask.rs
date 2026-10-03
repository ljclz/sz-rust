// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 数据脱敏引擎（spec §5.16）
//!
//! 字段级脱敏规则 + 多场景脱敏 + 自定义函数 + 内置规则。

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::MaskError;

/// 脱敏场景（spec §6.16 规则 2）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MaskScene {
    /// 日志脱敏
    Log,
    /// 响应脱敏
    Response,
    /// 数据库脱敏
    Database,
}

/// 内置脱敏规则（spec §6.16 规则 4）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuiltinMaskRule {
    /// 手机号中间 4 位 *
    Phone,
    /// 身份证中间 *
    IdCard,
    /// 邮箱 @ 前 *
    Email,
    /// 银行卡中间 *
    BankCard,
}

impl BuiltinMaskRule {
    /// 应用内置脱敏规则
    pub fn apply(&self, value: &str) -> String {
        match self {
            BuiltinMaskRule::Phone => mask_phone(value),
            BuiltinMaskRule::IdCard => mask_id_card(value),
            BuiltinMaskRule::Email => mask_email(value),
            BuiltinMaskRule::BankCard => mask_bank_card(value),
        }
    }
}

/// 手机号脱敏：保留前 3 后 4，中间 4 位 *
fn mask_phone(value: &str) -> String {
    if value.len() < 7 {
        return "*".repeat(value.len());
    }
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    for (i, c) in chars.iter().enumerate() {
        if i < 3 || i >= chars.len() - 4 {
            result.push(*c);
        } else {
            result.push('*');
        }
    }
    result
}

/// 身份证脱敏：保留前 3 后 4，中间 *
fn mask_id_card(value: &str) -> String {
    if value.len() < 8 {
        return "*".repeat(value.len());
    }
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    for (i, c) in chars.iter().enumerate() {
        if i < 3 || i >= chars.len() - 4 {
            result.push(*c);
        } else {
            result.push('*');
        }
    }
    result
}

/// 邮箱脱敏：@ 前部分保留首尾字符，中间 *
fn mask_email(value: &str) -> String {
    if let Some(at_pos) = value.find('@') {
        let local = &value[..at_pos];
        let domain = &value[at_pos..];
        if local.len() <= 1 {
            format!("*{domain}")
        } else if local.len() == 2 {
            let last = local.chars().last().unwrap_or('*');
            format!("*{last}{domain}")
        } else {
            let first = local.chars().next().unwrap_or('*');
            let last = local.chars().last().unwrap_or('*');
            format!("{}{}{}{domain}", first, "*".repeat(local.len() - 2), last)
        }
    } else {
        "*".repeat(value.len())
    }
}

/// 银行卡脱敏：保留前 4 后 4，中间 *
fn mask_bank_card(value: &str) -> String {
    if value.len() < 8 {
        return "*".repeat(value.len());
    }
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    for (i, c) in chars.iter().enumerate() {
        if i < 4 || i >= chars.len() - 4 {
            result.push(*c);
        } else {
            result.push('*');
        }
    }
    result
}

/// 脱敏规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaskRule {
    /// 字段标识
    pub field: String,
    /// 内置规则
    pub builtin: Option<BuiltinMaskRule>,
    /// 自定义函数标识
    pub custom: Option<String>,
}

impl MaskRule {
    /// 创建内置规则
    pub fn builtin(field: impl Into<String>, rule: BuiltinMaskRule) -> Self {
        Self {
            field: field.into(),
            builtin: Some(rule),
            custom: None,
        }
    }

    /// 创建自定义规则
    pub fn custom(field: impl Into<String>, fn_name: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            builtin: None,
            custom: Some(fn_name.into()),
        }
    }
}

/// 自定义脱敏函数类型
pub type CustomMaskFn = Arc<dyn Fn(&str) -> Result<String, MaskError> + Send + Sync>;

/// 脱敏引擎
pub struct MaskEngine {
    /// 场景 → 字段 → 规则
    rules: HashMap<MaskScene, HashMap<String, MaskRule>>,
    /// 自定义函数注册表
    custom_fns: HashMap<String, CustomMaskFn>,
}

impl MaskEngine {
    /// 创建脱敏引擎
    pub fn new() -> Self {
        Self {
            rules: HashMap::new(),
            custom_fns: HashMap::new(),
        }
    }

    /// 注册脱敏规则
    pub fn register(&mut self, scene: MaskScene, rule: MaskRule) {
        self.rules
            .entry(scene)
            .or_default()
            .insert(rule.field.clone(), rule);
    }

    /// 注册自定义脱敏函数
    pub fn register_custom(&mut self, name: impl Into<String>, f: CustomMaskFn) {
        self.custom_fns.insert(name.into(), f);
    }

    /// 应用脱敏（按场景，spec §5.16 规则 1）
    ///
    /// # 后置条件
    /// - 敏感字段脱敏后输出
    /// - 规则缺失 → 原值返回（不脱敏）
    pub fn mask(&self, scene: MaskScene, field: &str, value: &str) -> Result<String, MaskError> {
        let Some(scene_rules) = self.rules.get(&scene) else {
            return Ok(value.to_string());
        };
        let Some(rule) = scene_rules.get(field) else {
            return Ok(value.to_string());
        };

        if let Some(ref builtin) = rule.builtin {
            return Ok(builtin.apply(value));
        }

        if let Some(ref fn_name) = rule.custom {
            let Some(f) = self.custom_fns.get(fn_name) else {
                return Err(MaskError::RuleNotFound(fn_name.clone()));
            };
            return f(value);
        }

        Ok(value.to_string())
    }

    /// 批量脱敏
    pub fn mask_fields(
        &self,
        scene: MaskScene,
        fields: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, MaskError> {
        let mut result = HashMap::with_capacity(fields.len());
        for (k, v) in fields {
            result.insert(k.clone(), self.mask(scene, k, v)?);
        }
        Ok(result)
    }
}

impl Default for MaskEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_phone() {
        assert_eq!(mask_phone("13812345678"), "138****5678");
        assert_eq!(mask_phone("123"), "***");
    }

    #[test]
    fn test_mask_id_card() {
        assert_eq!(mask_id_card("110101199001011234"), "110***********1234");
        assert_eq!(mask_id_card("12345"), "*****");
    }

    #[test]
    fn test_mask_email() {
        assert_eq!(mask_email("user@example.com"), "u**r@example.com");
        assert_eq!(mask_email("ab@test.com"), "*b@test.com");
        assert_eq!(mask_email("invalid"), "*******");
    }

    #[test]
    fn test_mask_bank_card() {
        assert_eq!(mask_bank_card("6222021234567890"), "6222********7890");
        assert_eq!(mask_bank_card("1234"), "****");
    }

    #[test]
    fn test_mask_engine_builtin() {
        let mut engine = MaskEngine::new();
        engine.register(
            MaskScene::Response,
            MaskRule::builtin("phone", BuiltinMaskRule::Phone),
        );
        let result = engine
            .mask(MaskScene::Response, "phone", "13812345678")
            .unwrap();
        assert_eq!(result, "138****5678");
    }

    #[test]
    fn test_mask_engine_no_rule() {
        let engine = MaskEngine::new();
        let result = engine
            .mask(MaskScene::Response, "phone", "13812345678")
            .unwrap();
        assert_eq!(result, "13812345678");
    }

    #[test]
    fn test_mask_engine_custom() {
        let mut engine = MaskEngine::new();
        engine.register(MaskScene::Log, MaskRule::custom("token", "hash_fn"));
        engine.register_custom(
            "hash_fn",
            Arc::new(|v: &str| Ok(format!("hash({})", v.len()))),
        );
        let result = engine.mask(MaskScene::Log, "token", "secret123").unwrap();
        assert_eq!(result, "hash(9)");
    }

    #[test]
    fn test_mask_engine_custom_not_found() {
        let mut engine = MaskEngine::new();
        engine.register(MaskScene::Log, MaskRule::custom("token", "missing_fn"));
        let result = engine.mask(MaskScene::Log, "token", "value");
        assert!(matches!(result, Err(MaskError::RuleNotFound(_))));
    }

    #[test]
    fn test_mask_fields() {
        let mut engine = MaskEngine::new();
        engine.register(
            MaskScene::Response,
            MaskRule::builtin("phone", BuiltinMaskRule::Phone),
        );
        engine.register(
            MaskScene::Response,
            MaskRule::builtin("email", BuiltinMaskRule::Email),
        );
        let fields = HashMap::from([
            ("phone".to_string(), "13812345678".to_string()),
            ("email".to_string(), "user@test.com".to_string()),
            ("name".to_string(), "张三".to_string()),
        ]);
        let result = engine.mask_fields(MaskScene::Response, &fields).unwrap();
        assert_eq!(result.get("phone").unwrap(), "138****5678");
        assert_eq!(result.get("email").unwrap(), "u**r@test.com");
        assert_eq!(result.get("name").unwrap(), "张三");
    }

    #[test]
    fn test_builtin_apply() {
        assert_eq!(BuiltinMaskRule::Phone.apply("13812345678"), "138****5678");
        assert_eq!(
            BuiltinMaskRule::Email.apply("user@test.com"),
            "u**r@test.com"
        );
    }
}
